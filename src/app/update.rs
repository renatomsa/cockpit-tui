//! `update`: única função que muda o estado. Pura (sem I/O); efeitos voltam como `Effect`.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::state::{AppState, ChatPick, Confirm, KillTarget, Panel, ProcCol, VoiceState};
use crate::{
    chat::ChatCmd,
    voice::VoiceCmd,
    providers::{
        Snapshot,
        calendar::CalendarSnapshot,
        claude::SessionsSnapshot,
        geo::Geo,
        git::WorktreesSnapshot,
        weather::Weather,
        system::SystemSnapshot,
        usage::UsageSnapshot,
    },
};

#[derive(Debug)]
pub enum Action {
    Tick(DateTime<Utc>),
    Key(KeyEvent),
    Resize,
    Geo(Snapshot<Geo>),
    /// Em `Box`: o snapshot de sistema é bem maior que as outras ações.
    System(Box<Snapshot<SystemSnapshot>>),
    Worktrees(Snapshot<WorktreesSnapshot>),
    Sessions(Snapshot<SessionsSnapshot>),
    Usage(Snapshot<UsageSnapshot>),
    Calendar(Snapshot<CalendarSnapshot>),
    Weather(Snapshot<Weather>),
    /// Um chat embutido abriu (o `Hub` já criou o processo).
    ChatOpened(crate::chat::ChatView),
    /// O `claude` de um chat saiu.
    ChatExited(u64),
    /// Texto colado no terminal (bracketed paste).
    Paste(String),
    /// Mouse sobre a tela do chat, com coluna e linha relativas a ela.
    ChatMouse(MouseEvent),
    /// Config e temas relidos do disco (o arquivo mudou).
    Reloaded(Box<Reloaded>),
    /// Mensagem para a barra de status (ex.: resultado de um efeito).
    Notice(String),
    /// Texto ditado (ou o erro) da gravação `take`.
    VoiceText { take: u64, text: Result<String, String> },
}

#[derive(Debug)]
pub struct Reloaded {
    pub config: crate::config::Loaded,
    pub themes: crate::theme::ThemeRegistry,
}

/// Efeito colateral pedido pelo `update`, executado pelo loop fora da thread de desenho.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    PersistTheme(String),
    /// SIGTERM, já confirmado pelo usuário.
    Kill(KillTarget),
    /// Responde um pedido de permissão do Claude Code (true = permitir desta vez).
    Permission(crate::claude_hook::Pending, bool),
    /// Pedido para os chats embutidos; o loop principal executa no `Hub`.
    Chat(crate::chat::ChatCmd),
    /// Abre uma URL no navegador (link da reunião).
    OpenUrl(String),
    /// Copia para a área de transferência pelo terminal (OSC 52).
    Copy(String),
    /// Gravação do ditado; o loop principal executa no `Voice`.
    Voice(VoiceCmd),
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub redraw: bool,
    pub effect: Option<Effect>,
}

impl Outcome {
    fn redraw() -> Self {
        Self { redraw: true, effect: None }
    }
}

pub fn update(s: &mut AppState, action: Action) -> Outcome {
    match action {
        Action::Tick(now) => {
            s.now = now;
            // A gravação para sozinha no limite do config.
            if let VoiceState::Recording { since } = s.voice
                && (now - since).num_seconds() >= s.config.voice.max_seconds
            {
                s.voice = VoiceState::Transcribing;
                s.notices.insert(0, format!("ditado: parei no limite de {} s", s.config.voice.max_seconds));
                return Outcome { redraw: true, effect: Some(Effect::Voice(VoiceCmd::Stop)) };
            }
            Outcome::redraw()
        }
        Action::Resize => Outcome::redraw(),
        Action::Geo(snap) => {
            s.geo = snap;
            s.recompute_tz();
            Outcome::redraw()
        }
        Action::System(snap) => {
            s.system = *snap;
            let n = s.selected_procs().len();
            s.proc_sel.row = s.proc_sel.row.min(n.saturating_sub(1));
            Outcome::redraw()
        }
        Action::Worktrees(snap) => {
            s.worktrees = snap;
            // o seletor de pasta aberto acompanha a lista nova
            if let Some(p) = &s.chat_pick {
                let n = s.chat_dirs(&p.query).len();
                let sel = p.sel.min(n.saturating_sub(1));
                s.chat_pick.as_mut().unwrap().sel = sel;
            }
            Outcome::redraw()
        }
        Action::Sessions(snap) => {
            s.sessions = snap;
            Outcome::redraw()
        }
        Action::Reloaded(r) => {
            let Reloaded { config, themes } = *r;
            s.apply_themes(themes);
            match config.warning {
                // Config com erro não é aplicado: fica o que estava rodando.
                Some(w) => s.notices.insert(0, format!("{} (mantive o config anterior)", w.replace(", usando defaults", ""))),
                None => {
                    let restart = s.apply_config(config.config);
                    let msg = if restart.is_empty() {
                        "config recarregado".to_string()
                    } else {
                        format!("config recarregado; {} vale{} ao reabrir o cockpit", restart.join(" "), if restart.len() == 1 { "" } else { "m" })
                    };
                    s.notices.insert(0, msg);
                }
            }
            s.notices.truncate(5);
            Outcome::redraw()
        }
        Action::Weather(snap) => {
            s.weather = snap;
            Outcome::redraw()
        }
        Action::Calendar(snap) => {
            s.calendar = snap;
            // esquece as reuniões que já saíram da agenda
            let keys: std::collections::HashSet<String> = s.events().iter().map(|e| e.key()).collect();
            s.dismissed.retain(|k| keys.contains(k));
            s.agenda_sel = s.agenda_sel.min(s.events().len().saturating_sub(1));
            Outcome::redraw()
        }
        Action::Usage(snap) => {
            s.usage = snap;
            Outcome::redraw()
        }
        Action::Notice(n) => {
            s.notices.insert(0, n);
            s.notices.truncate(5);
            Outcome::redraw()
        }
        Action::Key(k) => on_key(s, k),
        Action::ChatOpened(view) => {
            s.chats.push(view);
            s.chat_sel = s.chats.len() - 1;
            s.focus = Panel::Chat;
            s.chat_typing = true;
            Outcome::redraw()
        }
        Action::ChatExited(id) => {
            if let Some(c) = s.chats.iter_mut().find(|c| c.id == id) {
                c.exited = true;
                s.notices.insert(0, format!("o chat {} encerrou · x fecha a aba", c.title));
            }
            if s.selected_chat().is_some_and(|c| c.id == id) {
                s.chat_typing = false;
            }
            Outcome::redraw()
        }
        Action::ChatMouse(ev) => on_chat_mouse(s, ev),
        Action::VoiceText { take, text } => on_voice_text(s, take, text),
        Action::Paste(text) => match s.selected_chat() {
            Some(c) if s.focus == Panel::Chat && s.chat_typing && !c.exited => {
                let bracketed = c.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
                let bytes = crate::chat::encode_paste(&text, bracketed);
                Outcome { redraw: false, effect: Some(Effect::Chat(ChatCmd::Input(c.id, bytes))) }
            }
            _ => Outcome::default(),
        },
    }
}

fn on_key(s: &mut AppState, k: KeyEvent) -> Outcome {
    if k.kind != KeyEventKind::Press {
        return Outcome::default();
    }
    if s.focus == Panel::Chat
        && s.chat_typing
        && s.confirm.is_none()
        && s.alert().level != crate::alert::Level::Imminent
    {
        return on_chat_typing(s, k);
    }
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        return quit(s);
    }
    if let Some(c) = s.confirm.take() {
        let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        if !yes {
            return Outcome::redraw();
        }
        let effect = match c {
            Confirm::Kill(t) => Some(Effect::Kill(t)),
            Confirm::CloseChat { id, .. } => Some(close_chat(s, id)),
            Confirm::Quit { .. } => {
                s.quit = true;
                None
            }
        };
        return Outcome { redraw: true, effect };
    }
    // Tela vermelha de reunião: qualquer tecla para o alerta; z adia; o abre a chamada.
    if s.alert().level == crate::alert::Level::Imminent {
        let ev = s.alert().event.cloned();
        let Some(ev) = ev else { return Outcome::default() };
        return match k.code {
            KeyCode::Char('z') => {
                s.snoozed_until = Some(s.now + chrono::Duration::minutes(s.config.calendar.snooze_minutes));
                s.notices.insert(0, format!("alerta adiado {} min", s.config.calendar.snooze_minutes));
                Outcome::redraw()
            }
            KeyCode::Char('o') => {
                s.dismissed.insert(ev.key());
                match ev.link() {
                    Some(url) => Outcome { redraw: true, effect: Some(Effect::OpenUrl(url.to_string())) },
                    None => {
                        s.notices.insert(0, "a reunião não tem link".into());
                        Outcome::redraw()
                    }
                }
            }
            _ => {
                s.dismissed.insert(ev.key());
                Outcome::redraw()
            }
        };
    }
    if let Some(p) = s.chat_pick.take() {
        return on_chat_pick(s, p, k.code);
    }
    if s.show_help || s.show_legend {
        s.show_help = false;
        s.show_legend = false;
        return Outcome::redraw();
    }
    if s.focus == Panel::System && on_system_key(s, k.code) {
        return Outcome::redraw();
    }
    if s.focus == Panel::Agenda
        && let Some(out) = on_agenda_key(s, k.code)
    {
        return out;
    }
    if s.focus == Panel::Chat
        && let Some(out) = on_chat_key(s, k.code)
    {
        return out;
    }
    // `a`: permitir desta vez o pedido de permissão, quando há um só pendente.
    if k.code == KeyCode::Char('a') && !s.pending_permissions().is_empty() {
        let pend = s.pending_permissions();
        if pend.len() == 1 {
            return Outcome { redraw: true, effect: Some(Effect::Permission(pend[0].clone(), true)) };
        }
        s.notices.insert(0, format!("{} pedidos de permissão: responda em cada conversa", pend.len()));
        return Outcome::redraw();
    }
    match k.code {
        KeyCode::Char('q') => return quit(s),
        KeyCode::Char('?') => s.show_help = true,
        KeyCode::Char('i') => s.show_legend = true,
        KeyCode::Tab => s.focus = s.focus.next(),
        KeyCode::BackTab => s.focus = s.focus.prev(),
        KeyCode::Char('t') | KeyCode::Char('T') => {
            let n = s.themes.len();
            s.theme_idx = if k.code == KeyCode::Char('t') { (s.theme_idx + 1) % n } else { (s.theme_idx + n - 1) % n };
            return Outcome { redraw: true, effect: Some(Effect::PersistTheme(s.theme().name.clone())) };
        }
        KeyCode::Char('x') if !s.notices.is_empty() => {
            s.notices.remove(0);
        }
        KeyCode::Char('v' | 'V') => return toggle_voice(s),
        KeyCode::Esc if s.voice != VoiceState::Idle => return cancel_voice(s),
        _ => return Outcome::default(),
    }
    Outcome::redraw()
}

/// `v` / Alt+V: começa a gravar; de novo, para e transcreve.
fn toggle_voice(s: &mut AppState) -> Outcome {
    let cmd = match s.voice {
        VoiceState::Idle => {
            s.voice_take += 1;
            s.voice = VoiceState::Recording { since: s.now };
            VoiceCmd::Start(s.voice_take)
        }
        VoiceState::Recording { .. } => {
            s.voice = VoiceState::Transcribing;
            VoiceCmd::Stop
        }
        VoiceState::Transcribing => {
            s.notices.insert(0, "ditado: ainda transcrevendo (Esc descarta)".into());
            return Outcome::redraw();
        }
    };
    Outcome { redraw: true, effect: Some(Effect::Voice(cmd)) }
}

/// Esc com o ditado ligado: descarta a gravação ou o texto que viria dela.
fn cancel_voice(s: &mut AppState) -> Outcome {
    s.voice = VoiceState::Idle;
    s.notices.insert(0, "ditado descartado".into());
    Outcome { redraw: true, effect: Some(Effect::Voice(VoiceCmd::Cancel)) }
}

/// Texto ditado: vai para o chat selecionado, sem Enter, para revisar antes de mandar.
/// Sem chat aberto, vai para a área de transferência.
fn on_voice_text(s: &mut AppState, take: u64, text: Result<String, String>) -> Outcome {
    // Descartado com Esc, ou de uma gravação anterior.
    if take != s.voice_take || s.voice == VoiceState::Idle {
        return Outcome::default();
    }
    s.voice = VoiceState::Idle;
    let text = match text {
        Ok(t) if t.is_empty() => {
            s.notices.insert(0, "ditado: não ouvi nada".into());
            return Outcome::redraw();
        }
        Ok(t) => t,
        Err(e) => {
            s.notices.insert(0, format!("ditado falhou: {e}"));
            return Outcome::redraw();
        }
    };
    match s.selected_chat() {
        Some(c) if !c.exited => {
            let id = c.id;
            if let Ok(mut p) = c.parser.lock() {
                p.screen_mut().set_scrollback(0);
            }
            s.focus = Panel::Chat;
            s.chat_typing = true;
            Outcome { redraw: true, effect: Some(Effect::Chat(ChatCmd::Input(id, text.into_bytes()))) }
        }
        _ => {
            s.notices.insert(0, format!("sem chat aberto: copiei o ditado ({})", crate::ui::truncate(&text, 40)));
            Outcome { redraw: true, effect: Some(Effect::Copy(text)) }
        }
    }
}

/// Move a seleção numa lista de `n` itens.
fn step(sel: &mut usize, n: usize, code: KeyCode) -> bool {
    let last = n.saturating_sub(1);
    *sel = match code {
        KeyCode::Up | KeyCode::Char('k') => sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => (*sel + 1).min(last),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => last,
        KeyCode::PageUp => sel.saturating_sub(10),
        KeyCode::PageDown => (*sel + 10).min(last),
        _ => return false,
    };
    true
}

fn on_agenda_key(s: &mut AppState, code: KeyCode) -> Option<Outcome> {
    let n = s.events().len();
    if step(&mut s.agenda_sel, n, code) {
        return Some(Outcome::redraw());
    }
    let ev = s.events().get(s.agenda_sel)?.clone();
    match code {
        KeyCode::Char('o') | KeyCode::Enter => match ev.link() {
            Some(url) => Some(Outcome { redraw: true, effect: Some(Effect::OpenUrl(url.to_string())) }),
            None => {
                s.notices.insert(0, format!("\"{}\" não tem link", ev.title));
                Some(Outcome::redraw())
            }
        },
        KeyCode::Char('y') => match ev.link() {
            Some(url) => {
                s.notices.insert(0, format!("link copiado: {url}"));
                Some(Outcome { redraw: true, effect: Some(Effect::Copy(url.to_string())) })
            }
            None => None,
        },
        _ => None,
    }
}

fn open_chat(dir: std::path::PathBuf, args: Vec<String>, title: String) -> Outcome {
    Outcome { redraw: true, effect: Some(Effect::Chat(ChatCmd::Open { dir, args, title })) }
}

/// Esquece o chat e encerra o processo dele.
fn close_chat(s: &mut AppState, id: u64) -> Effect {
    s.chats.retain(|c| c.id != id);
    s.chat_sel = s.chat_sel.min(s.chats.len().saturating_sub(1));
    s.chat_typing = false;
    Effect::Chat(ChatCmd::Close(id))
}

/// `q` / Ctrl+C: com chats rodando, pergunta antes, porque sair encerra todos.
fn quit(s: &mut AppState) -> Outcome {
    match s.live_chats() {
        0 => s.quit = true,
        chats => s.confirm = Some(Confirm::Quit { chats }),
    }
    Outcome::redraw()
}

/// Digitando no chat: tudo vai para o `claude`, menos Ctrl+Q (volta ao cockpit),
/// Shift+PgUp/PgDn (rola o histórico), Alt+1-9 (troca de chat), Alt+V (ditado) e, com
/// o ditado ligado, Esc (descarta).
fn on_chat_typing(s: &mut AppState, mut k: KeyEvent) -> Outcome {
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Char('v' | 'V') if k.modifiers.contains(KeyModifiers::ALT) => return toggle_voice(s),
        KeyCode::Esc if s.voice != VoiceState::Idle => return cancel_voice(s),
        KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            s.chat_typing = false;
            return Outcome::redraw();
        }
        KeyCode::Char(c @ '1'..='9') if k.modifiers.contains(KeyModifiers::ALT) => {
            let i = c as usize - '1' as usize;
            if i < s.chats.len() {
                s.chat_sel = i;
                s.chat_typing = !s.chats[i].exited;
            }
            return Outcome::redraw();
        }
        _ => {}
    }
    let Some(c) = s.selected_chat() else {
        s.chat_typing = false;
        return Outcome::redraw();
    };
    if c.exited {
        s.chat_typing = false;
        return Outcome::redraw();
    }
    let id = c.id;
    let Ok(mut p) = c.parser.lock() else { return Outcome::default() };
    // Na tela alternativa não há histórico aqui: o PgUp/PgDn vai para o programa rolar.
    if shift && matches!(k.code, KeyCode::PageUp | KeyCode::PageDown) && !p.screen().alternate_screen() {
        let half = (p.screen().size().0 / 2).max(1) as usize;
        let now = p.screen().scrollback();
        let to = if k.code == KeyCode::PageUp { now + half } else { now.saturating_sub(half) };
        p.screen_mut().set_scrollback(to);
        return Outcome::redraw();
    }
    if shift && matches!(k.code, KeyCode::PageUp | KeyCode::PageDown) {
        k.modifiers.remove(KeyModifiers::SHIFT);
    }
    let app_cursor = p.screen().application_cursor();
    // Digitar volta para o fim do histórico, como num terminal.
    let scrolled = p.screen().scrollback() > 0;
    p.screen_mut().set_scrollback(0);
    drop(p);
    match crate::chat::encode_key(k, app_cursor) {
        Some(bytes) => Outcome { redraw: scrolled, effect: Some(Effect::Chat(ChatCmd::Input(id, bytes))) },
        None => Outcome::default(),
    }
}

/// Mouse sobre o chat. Se o programa pediu mouse (o `claude` em tela cheia pede), o evento
/// vai para ele, que rola e seleciona sozinho; senão a roda rola o histórico do emulador.
/// Clicar entra no modo digitando, como clicar num terminal.
fn on_chat_mouse(s: &mut AppState, ev: MouseEvent) -> Outcome {
    let Some(c) = s.selected_chat() else { return Outcome::default() };
    let (id, exited) = (c.id, c.exited);
    let Ok(mut p) = c.parser.lock() else { return Outcome::default() };
    let screen = p.screen();
    let bytes = (!exited).then(|| crate::chat::encode_mouse(ev, screen.mouse_protocol_mode(), screen.mouse_protocol_encoding())).flatten();
    let mut redraw = false;
    if bytes.is_none() && matches!(ev.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
        let now = screen.scrollback();
        let to = if ev.kind == MouseEventKind::ScrollUp { now + 3 } else { now.saturating_sub(3) };
        p.screen_mut().set_scrollback(to);
        redraw = p.screen().scrollback() != now;
    }
    drop(p);
    if ev.kind == MouseEventKind::Down(MouseButton::Left) && !exited && !(s.focus == Panel::Chat && s.chat_typing) {
        s.focus = Panel::Chat;
        s.chat_typing = true;
        redraw = true;
    }
    Outcome { redraw, effect: bytes.map(|b| Effect::Chat(ChatCmd::Input(id, b))) }
}

/// Painel Chat sem estar digitando: abas, abrir e fechar.
fn on_chat_key(s: &mut AppState, code: KeyCode) -> Option<Outcome> {
    match code {
        KeyCode::Enter => match s.selected_chat() {
            Some(c) if !c.exited => {
                s.chat_typing = true;
                Some(Outcome::redraw())
            }
            Some(c) => {
                s.notices.insert(0, format!("o chat {} encerrou: x fecha, n abre outro", c.title));
                Some(Outcome::redraw())
            }
            None => Some(new_chat(s)),
        },
        KeyCode::Char('n') => Some(new_chat(s)),
        KeyCode::Left | KeyCode::Right if !s.chats.is_empty() => {
            let n = s.chats.len();
            s.chat_sel = if code == KeyCode::Right { (s.chat_sel + 1) % n } else { (s.chat_sel + n - 1) % n };
            Some(Outcome::redraw())
        }
        KeyCode::Char(c @ '1'..='9') => {
            let i = c as usize - '1' as usize;
            (i < s.chats.len()).then(|| {
                s.chat_sel = i;
                Outcome::redraw()
            })
        }
        KeyCode::Char('x') => {
            let c = s.selected_chat()?;
            let (id, title, exited) = (c.id, c.title.clone(), c.exited);
            if exited {
                let effect = close_chat(s, id);
                return Some(Outcome { redraw: true, effect: Some(effect) });
            }
            s.confirm = Some(Confirm::CloseChat { id, title });
            Some(Outcome::redraw())
        }
        _ => None,
    }
}

/// Chat novo: abre o seletor de pasta.
fn new_chat(s: &mut AppState) -> Outcome {
    s.chat_pick = Some(ChatPick::default());
    Outcome::redraw()
}

/// Seletor de pasta do chat novo: digitar filtra, ↑↓ escolhe, Enter abre, Esc cancela.
fn on_chat_pick(s: &mut AppState, mut p: ChatPick, code: KeyCode) -> Outcome {
    let dirs = s.chat_dirs(&p.query);
    match code {
        KeyCode::Esc => return Outcome::redraw(),
        KeyCode::Enter => {
            return match dirs.into_iter().nth(p.sel) {
                Some(d) => open_chat(d.path, vec![], d.label),
                None => Outcome::redraw(),
            };
        }
        KeyCode::Up => p.sel = p.sel.saturating_sub(1),
        KeyCode::Down => p.sel = (p.sel + 1).min(dirs.len().saturating_sub(1)),
        KeyCode::Backspace => {
            p.query.pop();
            p.sel = 0;
        }
        KeyCode::Char(c) => {
            p.query.push(c);
            p.sel = 0;
        }
        _ => {}
    }
    s.chat_pick = Some(p);
    Outcome::redraw()
}

/// Navegação e ações do painel de sistema. `true` = tecla consumida.
fn on_system_key(s: &mut AppState, code: KeyCode) -> bool {
    let n = s.selected_procs().len();
    match code {
        KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
            s.proc_sel.col = match s.proc_sel.col {
                ProcCol::Cpu => ProcCol::Mem,
                ProcCol::Mem => ProcCol::Cpu,
            };
            s.proc_sel.row = s.proc_sel.row.min(s.selected_procs().len().saturating_sub(1));
        }
        KeyCode::Up | KeyCode::Char('k') => s.proc_sel.row = s.proc_sel.row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => s.proc_sel.row = (s.proc_sel.row + 1).min(n.saturating_sub(1)),
        KeyCode::Char('K') => {
            let Some(p) = s.selected_procs().get(s.proc_sel.row) else { return true };
            if p.top_pid <= 1 || p.top_pid == s.self_pid {
                s.notices.insert(0, format!("não encerro {} (pid {})", p.name, p.top_pid));
            } else {
                s.confirm = Some(Confirm::Kill(KillTarget { pid: p.top_pid, name: p.name.clone() }));
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::Panel,
        config::{Config, Paths},
        providers::geo::GeoSource,
        theme::ThemeRegistry,
    };
    use std::path::PathBuf;

    fn state() -> AppState {
        let paths = Paths { config_dir: "/x".into(), state_dir: "/x".into(), cache_dir: "/x".into() };
        let themes = ThemeRegistry::load(&PathBuf::from("/nao/existe"), true);
        let mut s = AppState::new(Config::default(), paths, themes, 0, vec![]);
        // Os testes de teclas partem da agenda; o padrão (chat) tem teste próprio.
        s.focus = Panel::Agenda;
        s
    }

    fn key(c: KeyCode) -> Action {
        Action::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    #[test]
    fn t_cycles_themes_and_persists() {
        let mut s = state();
        let first = s.theme().name.clone();
        let out = update(&mut s, key(KeyCode::Char('t')));
        assert_ne!(s.theme().name, first);
        assert_eq!(out.effect, Some(Effect::PersistTheme(s.theme().name.clone())));
        update(&mut s, key(KeyCode::Char('T')));
        assert_eq!(s.theme().name, first);
        // volta ao início depois de percorrer todos
        for _ in 0..s.themes.len() {
            update(&mut s, key(KeyCode::Char('t')));
        }
        assert_eq!(s.theme().name, first);
    }

    #[test]
    fn tab_moves_focus_and_q_quits() {
        let paths = Paths { config_dir: "/x".into(), state_dir: "/x".into(), cache_dir: "/x".into() };
        let fresh = AppState::new(Config::default(), paths, ThemeRegistry::load(&PathBuf::from("/nao/existe"), true), 0, vec![]);
        assert_eq!(fresh.focus, Panel::Chat, "o foco começa no chat");
        // na ordem da tela: chat, agenda, relógio, sistema
        let mut s = fresh;
        for want in [Panel::Agenda, Panel::Hero, Panel::System, Panel::Chat] {
            update(&mut s, key(KeyCode::Tab));
            assert_eq!(s.focus, want);
        }
        update(&mut s, key(KeyCode::BackTab));
        assert_eq!(s.focus, Panel::System);
        update(&mut s, key(KeyCode::Char('q')));
        assert!(s.quit);
    }

    #[test]
    fn help_swallows_next_key() {
        let mut s = state();
        update(&mut s, key(KeyCode::Char('?')));
        assert!(s.show_help);
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.show_help && !s.quit);
    }

    fn with_procs(s: &mut AppState) {
        use crate::providers::system::ProcGroup;
        let g = |name: &str, pid| ProcGroup { name: name.into(), count: 1, cpu: 10.0, mem: 10, top_pid: pid };
        let snap = SystemSnapshot {
            top_cpu: vec![g("node", 100), g("cargo", 200)],
            top_mem: vec![g("code", 300)],
            ..Default::default()
        };
        update(s, Action::System(Box::new(Snapshot::ok(snap))));
    }

    #[test]
    fn kill_needs_confirmation() {
        let mut s = state();
        with_procs(&mut s);
        s.focus = Panel::System;
        update(&mut s, key(KeyCode::Down));
        let out = update(&mut s, key(KeyCode::Char('K')));
        assert!(out.effect.is_none());
        assert_eq!(s.confirm, Some(Confirm::Kill(KillTarget { pid: 200, name: "cargo".into() })));
        // qualquer tecla que não seja y cancela
        let out = update(&mut s, key(KeyCode::Char('n')));
        assert!(out.effect.is_none() && s.confirm.is_none());
        update(&mut s, key(KeyCode::Char('K')));
        let out = update(&mut s, key(KeyCode::Char('y')));
        assert_eq!(out.effect, Some(Effect::Kill(KillTarget { pid: 200, name: "cargo".into() })));
    }

    #[test]
    fn column_switch_clamps_row_and_refuses_self() {
        let mut s = state();
        with_procs(&mut s);
        s.focus = Panel::System;
        update(&mut s, key(KeyCode::Char('j')));
        assert_eq!(s.proc_sel.row, 1);
        update(&mut s, key(KeyCode::Right));
        assert_eq!((s.proc_sel.col, s.proc_sel.row), (ProcCol::Mem, 0));
        s.self_pid = 300;
        update(&mut s, key(KeyCode::Char('K')));
        assert!(s.confirm.is_none());
        assert!(s.notices[0].contains("não encerro"));
    }

    #[test]
    fn system_keys_only_when_focused() {
        let mut s = state();
        with_procs(&mut s);
        update(&mut s, key(KeyCode::Char('K')));
        assert!(s.confirm.is_none());
    }

    fn with_worktrees(s: &mut AppState) {
        use crate::providers::git::{Repo, Worktree};
        let wt = |name: &str, path: &str, main: bool| Worktree {
            name: name.into(),
            path: path.into(),
            is_main: main,
            branch: Some(format!("b-{name}")),
            ..Default::default()
        };
        let snap = WorktreesSnapshot {
            repos: vec![Repo {
                name: "v360".into(),
                path: "/h/v360".into(),
                worktrees: vec![
                    wt("v360", "/h/v360", true),
                    wt("processable-file-cfs", "/h/v360/.claude/worktrees/processable-file-cfs", false),
                    wt("build-s4", "/h/v360/.claude/worktrees/build-s4", false),
                ],
            }],
        };
        update(s, Action::Worktrees(Snapshot::ok(snap)));
    }

    #[test]
    fn i_opens_the_legend_and_any_key_closes() {
        let mut s = state();
        update(&mut s, key(KeyCode::Char('i')));
        assert!(s.show_legend);
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.show_legend && !s.quit);
    }

    fn with_meeting_in(s: &mut AppState, mins: i64) {
        use crate::providers::calendar::Event;
        let start = s.now + chrono::Duration::minutes(mins);
        let ev = Event {
            id: "daily".into(),
            title: "Daily".into(),
            start,
            end: start + chrono::Duration::minutes(15),
            meet: Some("https://meet.google.com/abc".into()),
            ..Default::default()
        };
        update(s, Action::Calendar(Snapshot::ok(CalendarSnapshot { events: vec![ev], calendars: vec![] })));
    }

    fn with_chat(s: &mut AppState) -> u64 {
        let parser = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(24, 80, 100)));
        update(s, Action::ChatOpened(crate::chat::ChatView { id: 7, title: "v360".into(), parser, exited: false }));
        7
    }

    #[test]
    fn chat_typing_forwards_keys_until_ctrl_q() {
        let mut s = state();
        let id = with_chat(&mut s);
        assert_eq!((s.focus, s.chat_typing), (Panel::Chat, true), "chat novo já recebe as teclas");
        // q e Ctrl+C vão para o claude, não fecham o cockpit
        let out = update(&mut s, key(KeyCode::Char('q')));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"q".to_vec()))));
        let out = update(&mut s, Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, vec![3]))));
        assert!(!s.quit);
        let out = update(&mut s, Action::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"\t".to_vec()))));
        update(&mut s, Action::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)));
        assert!(!s.chat_typing);
        update(&mut s, key(KeyCode::Tab));
        assert_eq!(s.focus, Panel::Agenda, "fora do modo digitando, Tab volta a trocar o foco");
    }

    #[test]
    fn mouse_wheel_goes_to_the_chat_or_scrolls_its_history() {
        let mut s = state();
        let id = with_chat(&mut s);
        let wheel = |kind| Action::ChatMouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE });
        // tela normal com histórico: a roda rola o emulador
        s.chats[0].parser.lock().unwrap().process(&"linha\r\n".repeat(40).into_bytes());
        let out = update(&mut s, wheel(MouseEventKind::ScrollUp));
        assert!(out.redraw && out.effect.is_none());
        assert_eq!(s.chats[0].parser.lock().unwrap().screen().scrollback(), 3);
        // tela cheia com mouse ligado (como o claude): o evento vai para o programa
        s.chats[0].parser.lock().unwrap().process(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        let out = update(&mut s, wheel(MouseEventKind::ScrollUp));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"\x1b[<64;1;1M".to_vec()))));
        // e o Shift+PgUp vira PgUp puro para ele
        let out = update(&mut s, Action::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"\x1b[5~".to_vec()))));
    }

    #[test]
    fn quitting_with_open_chats_asks_first() {
        let mut s = state();
        with_chat(&mut s);
        s.chat_typing = false;
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.quit && matches!(s.confirm, Some(Confirm::Quit { chats: 1 })));
        update(&mut s, key(KeyCode::Char('n')));
        assert!(!s.quit && s.confirm.is_none(), "qualquer outra tecla cancela");
        update(&mut s, key(KeyCode::Char('q')));
        update(&mut s, key(KeyCode::Char('y')));
        assert!(s.quit);
    }

    #[test]
    fn closing_a_running_chat_asks_and_kills_it() {
        let mut s = state();
        let id = with_chat(&mut s);
        s.chat_typing = false;
        update(&mut s, key(KeyCode::Char('x')));
        assert!(matches!(s.confirm, Some(Confirm::CloseChat { .. })));
        let out = update(&mut s, key(KeyCode::Char('y')));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Close(id))));
        assert!(s.chats.is_empty());
        // encerrado sozinho: fecha sem perguntar
        let id = with_chat(&mut s);
        update(&mut s, Action::ChatExited(id));
        assert!(!s.chat_typing);
        let out = update(&mut s, key(KeyCode::Char('x')));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Close(id))));
    }

    #[test]
    fn n_picks_the_folder_then_opens_the_chat() {
        let mut s = state();
        with_worktrees(&mut s);
        s.focus = Panel::Chat;
        update(&mut s, key(KeyCode::Char('n')));
        assert!(s.chat_pick.is_some());
        let labels: Vec<String> = s.chat_dirs("").into_iter().map(|d| d.label).collect();
        assert_eq!(labels, ["~", "v360", "v360/processable-file-cfs", "v360/build-s4"]);
        // digitar filtra (até o n, que não abre outro seletor), Enter abre
        for c in "cfs".chars() {
            update(&mut s, key(KeyCode::Char(c)));
        }
        assert_eq!(s.chat_pick.as_ref().unwrap().query, "cfs");
        let out = update(&mut s, key(KeyCode::Enter));
        assert!(s.chat_pick.is_none());
        match out.effect {
            Some(Effect::Chat(ChatCmd::Open { dir, title, .. })) => {
                assert_eq!(dir, PathBuf::from("/h/v360/.claude/worktrees/processable-file-cfs"));
                assert_eq!(title, "v360/processable-file-cfs");
            }
            other => panic!("{other:?}"),
        }
        // Esc cancela sem abrir nada
        update(&mut s, key(KeyCode::Char('n')));
        let out = update(&mut s, key(KeyCode::Esc));
        assert!(out.effect.is_none() && s.chat_pick.is_none());
    }

    #[test]
    fn any_key_stops_the_red_alert() {
        let mut s = state();
        with_meeting_in(&mut s, 3);
        assert_eq!(s.alert().level, crate::alert::Level::Imminent);
        assert!(s.animating(), "pisca a 2 Hz");
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.quit, "a tecla só para o alerta");
        assert_eq!(s.alert().level, crate::alert::Level::Soon);
        update(&mut s, key(KeyCode::Char('q')));
        assert!(s.quit);
    }

    #[test]
    fn z_snoozes_and_o_opens_the_call() {
        let mut s = state();
        with_meeting_in(&mut s, 3);
        update(&mut s, key(KeyCode::Char('z')));
        assert!(s.alert().silenced);
        // soneca de 5 min vence com a reunião ainda na janela
        s.now += chrono::Duration::minutes(6);
        assert_eq!(s.alert().level, crate::alert::Level::Imminent);
        let out = update(&mut s, key(KeyCode::Char('o')));
        assert_eq!(out.effect, Some(Effect::OpenUrl("https://meet.google.com/abc".into())));
        assert_eq!(s.alert().level, crate::alert::Level::Soon);
    }

    #[test]
    fn agenda_panel_opens_and_copies_links() {
        let mut s = state();
        with_meeting_in(&mut s, 60);
        s.focus = Panel::Agenda;
        let out = update(&mut s, key(KeyCode::Char('y')));
        assert_eq!(out.effect, Some(Effect::Copy("https://meet.google.com/abc".into())));
        let out = update(&mut s, key(KeyCode::Char('o')));
        assert_eq!(out.effect, Some(Effect::OpenUrl("https://meet.google.com/abc".into())));
    }

    #[test]
    fn reload_applies_live_settings_and_flags_the_rest() {
        let mut s = state();
        let mut cfg = s.config.clone();
        cfg.theme = "nord".into();
        cfg.calendar.alert_minutes = 2; // vale na hora
        cfg.worktrees.depth = 5; // só ao reabrir
        let themes = ThemeRegistry::load(&PathBuf::from("/nao/existe"), true);
        let loaded = crate::config::Loaded { config: cfg, path: "/x/config.toml".into(), warning: None };
        update(&mut s, Action::Reloaded(Box::new(Reloaded { config: loaded, themes })));
        assert_eq!(s.theme().name, "nord");
        assert_eq!(s.config.calendar.alert_minutes, 2);
        assert!(s.notices[0].contains("[worktrees]"), "{}", s.notices[0]);
        // config com erro: mantém o anterior
        let themes = ThemeRegistry::load(&PathBuf::from("/nao/existe"), true);
        let bad = crate::config::Loaded { config: Config::default(), path: "/x".into(), warning: Some("config inválido".into()) };
        update(&mut s, Action::Reloaded(Box::new(Reloaded { config: bad, themes })));
        assert_eq!(s.config.worktrees.depth, 5);
        assert!(s.notices[0].contains("mantive"));
    }

    fn with_pending(s: &mut AppState, pids: &[u32]) {
        use crate::{claude_hook::Pending, providers::claude::{Session, Status}};
        let live = pids
            .iter()
            .enumerate()
            .map(|(i, pid)| Session {
                id: format!("s{i}"),
                pid: *pid,
                status: Status::Waiting,
                pending: Some(Pending { id: format!("p{i}"), nonce: "n".into(), claude_pid: *pid, tool: "Bash".into(), summary: format!("cmd{i}"), created_at: i as i64, ..Default::default() }),
                ..Default::default()
            })
            .collect();
        update(s, Action::Sessions(Snapshot::ok(SessionsSnapshot { live, recent: vec![] })));
    }

    #[test]
    fn a_allows_the_only_pending_request_from_anywhere() {
        let mut s = state();
        with_pending(&mut s, &[10]);
        s.focus = Panel::Agenda;
        let out = update(&mut s, key(KeyCode::Char('a')));
        assert!(matches!(out.effect, Some(Effect::Permission(ref p, true)) if p.id == "p0"));
    }

    #[test]
    fn a_with_several_pending_only_explains() {
        let mut s = state();
        with_pending(&mut s, &[10, 11]);
        s.focus = Panel::Agenda;
        let out = update(&mut s, key(KeyCode::Char('a')));
        assert!(out.effect.is_none() && s.notices[0].contains("2 pedidos"));
    }

    #[test]
    fn v_records_and_the_text_lands_in_the_chat_without_enter() {
        let mut s = state();
        let id = with_chat(&mut s);
        s.chat_typing = false;
        s.focus = Panel::Agenda;
        let out = update(&mut s, key(KeyCode::Char('v')));
        assert_eq!(out.effect, Some(Effect::Voice(VoiceCmd::Start(1))));
        assert!(matches!(s.voice, VoiceState::Recording { .. }));
        let out = update(&mut s, key(KeyCode::Char('v')));
        assert_eq!(out.effect, Some(Effect::Voice(VoiceCmd::Stop)));
        assert_eq!(s.voice, VoiceState::Transcribing);
        let out = update(&mut s, Action::VoiceText { take: 1, text: Ok("roda os testes".into()) });
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"roda os testes".to_vec()))));
        assert_eq!(s.voice, VoiceState::Idle);
        assert_eq!((s.focus, s.chat_typing), (Panel::Chat, true), "o chat fica pronto para o Enter");
    }

    #[test]
    fn alt_v_dictates_while_typing_and_esc_discards() {
        let mut s = state();
        let id = with_chat(&mut s);
        let alt_v = Action::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT));
        let out = update(&mut s, alt_v);
        assert_eq!(out.effect, Some(Effect::Voice(VoiceCmd::Start(1))));
        // v puro continua indo para o claude
        let out = update(&mut s, key(KeyCode::Char('v')));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"v".to_vec()))));
        // Esc descarta em vez de ir para o claude; o texto que chegar depois é ignorado
        let out = update(&mut s, key(KeyCode::Esc));
        assert_eq!(out.effect, Some(Effect::Voice(VoiceCmd::Cancel)));
        let out = update(&mut s, Action::VoiceText { take: 1, text: Ok("tarde demais".into()) });
        assert!(out.effect.is_none());
        // parado, Esc volta a ir para o claude
        let out = update(&mut s, key(KeyCode::Esc));
        assert_eq!(out.effect, Some(Effect::Chat(ChatCmd::Input(id, b"\x1b".to_vec()))));
    }

    #[test]
    fn dictation_without_a_chat_is_copied_and_errors_become_notices() {
        let mut s = state();
        update(&mut s, key(KeyCode::Char('v')));
        update(&mut s, key(KeyCode::Char('v')));
        let out = update(&mut s, Action::VoiceText { take: 1, text: Ok("olá".into()) });
        assert_eq!(out.effect, Some(Effect::Copy("olá".into())));
        update(&mut s, key(KeyCode::Char('v')));
        let out = update(&mut s, Action::VoiceText { take: 2, text: Err("o gravador não gravou nada".into()) });
        assert!(out.effect.is_none() && s.voice == VoiceState::Idle);
        assert!(s.notices[0].contains("ditado falhou"), "{}", s.notices[0]);
    }

    #[test]
    fn recording_stops_itself_at_max_seconds() {
        let mut s = state();
        update(&mut s, key(KeyCode::Char('v')));
        let start = s.now;
        let out = update(&mut s, Action::Tick(start + chrono::Duration::seconds(10)));
        assert!(out.effect.is_none());
        let max = s.config.voice.max_seconds;
        let out = update(&mut s, Action::Tick(start + chrono::Duration::seconds(max)));
        assert_eq!(out.effect, Some(Effect::Voice(VoiceCmd::Stop)));
        assert_eq!(s.voice, VoiceState::Transcribing);
    }

    #[test]
    fn geo_sets_timezone() {
        let mut s = state();
        let g = Geo {
            city: "Tóquio".into(),
            region: None,
            country: Some("Japan".into()),
            lat: None,
            lon: None,
            timezone: Some("Asia/Tokyo".into()),
            source: GeoSource::Ip("teste".into()),
        };
        update(&mut s, Action::Geo(Snapshot::ok(g)));
        assert_eq!(s.tz, chrono_tz::Asia::Tokyo);
        assert_eq!(s.city_label(), "Tóquio");
        assert_eq!(s.place_line(), "Tóquio · Japan");
    }
}
