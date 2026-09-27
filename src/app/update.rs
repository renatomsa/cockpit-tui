//! `update`: única função que muda o estado. Pura (sem I/O); efeitos voltam como `Effect`.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::state::{AppState, Confirm, KillTarget, Panel, ProcCol};
use crate::{
    providers::{
        Snapshot,
        calendar::CalendarSnapshot,
        claude::{SessionsSnapshot, Status},
        docker::{self, DockerCmd, DockerSnapshot},
        geo::Geo,
        git::WorktreesSnapshot,
        ping::PingSnapshot,
        specs::Specs,
        system::SystemSnapshot,
        usage::UsageSnapshot,
    },
    shell::{ExitTarget, Then},
};

#[derive(Debug)]
pub enum Action {
    Tick(DateTime<Utc>),
    Key(KeyEvent),
    Resize,
    Geo(Snapshot<Geo>),
    /// Em `Box`: o snapshot de sistema é bem maior que as outras ações.
    System(Box<Snapshot<SystemSnapshot>>),
    Ping(Snapshot<PingSnapshot>),
    Worktrees(Snapshot<WorktreesSnapshot>),
    Sessions(Snapshot<SessionsSnapshot>),
    Usage(Snapshot<UsageSnapshot>),
    Specs(Snapshot<Specs>),
    Docker(Snapshot<DockerSnapshot>),
    Calendar(Snapshot<CalendarSnapshot>),
    /// Mensagem para a barra de status (ex.: resultado de um efeito).
    Notice(String),
}

/// Efeito colateral pedido pelo `update`, executado pelo loop fora da thread de desenho.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    PersistTheme(String),
    /// SIGTERM, já confirmado pelo usuário.
    Kill(KillTarget),
    /// Abre uma URL no navegador (link da reunião).
    OpenUrl(String),
    /// Comandos do Docker já confirmados; `label` resume a ação na barra de status.
    Docker { cmds: Vec<DockerCmd>, label: String },
    /// Copia para a área de transferência pelo terminal (OSC 52).
    Copy(String),
    /// Abre uma aba nova de terminal rodando `cmd` em `dir`; o cockpit continua aberto.
    OpenTab { launcher: crate::terminal::Launcher, dir: std::path::PathBuf, cmd: String, title: String },
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
        Action::Ping(snap) => {
            s.ping = snap;
            Outcome::redraw()
        }
        Action::Worktrees(snap) => {
            s.worktrees = snap;
            s.recompute_wt_sessions();
            s.wt_sel = s.wt_sel.min(s.visible_worktrees().len().saturating_sub(1));
            Outcome::redraw()
        }
        Action::Sessions(snap) => {
            // A ordem muda quando o status muda: a seleção segue a conversa, não a posição.
            let selected = s.session_list().get(s.sess_sel).map(|x| x.id.clone());
            s.sessions = snap;
            s.recompute_wt_sessions();
            let list = s.session_list();
            s.sess_sel = selected
                .and_then(|id| list.iter().position(|x| x.id == id))
                .unwrap_or(s.sess_sel)
                .min(list.len().saturating_sub(1));
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
        Action::Docker(snap) => {
            s.docker = snap;
            let n = s.docker.data.as_ref().map_or(0, |d| d.containers.len());
            s.docker_sel = s.docker_sel.min(n.saturating_sub(1));
            Outcome::redraw()
        }
        Action::Specs(snap) => {
            s.specs = snap;
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
    }
}

fn on_key(s: &mut AppState, k: KeyEvent) -> Outcome {
    if k.kind != KeyEventKind::Press {
        return Outcome::default();
    }
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        s.quit = true;
        return Outcome::default();
    }
    if let Some(c) = s.confirm.take() {
        let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        let effect = yes.then_some(match c {
            Confirm::Kill(t) => Effect::Kill(t),
            Confirm::Docker { title, cmds, .. } => Effect::Docker { cmds, label: title },
        });
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
    if s.show_help || s.show_legend {
        s.show_help = false;
        s.show_legend = false;
        return Outcome::redraw();
    }
    if s.filter_editing {
        return on_filter_key(s, k.code);
    }
    if s.focus == Panel::System && on_system_key(s, k.code) {
        return Outcome::redraw();
    }
    if s.focus == Panel::Agenda
        && let Some(out) = on_agenda_key(s, k.code)
    {
        return out;
    }
    if s.focus == Panel::Docker
        && let Some(out) = on_docker_key(s, k.code)
    {
        return out;
    }
    if s.focus == Panel::Worktrees
        && let Some(out) = on_worktrees_key(s, k.code)
    {
        return out;
    }
    if s.focus == Panel::Sessions
        && let Some(out) = on_sessions_key(s, k.code)
    {
        return out;
    }
    match k.code {
        KeyCode::Char('q') => s.quit = true,
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
        _ => return Outcome::default(),
    }
    Outcome::redraw()
}

/// Digitando o filtro das worktrees: todas as teclas vão para o texto.
fn on_filter_key(s: &mut AppState, code: KeyCode) -> Outcome {
    match code {
        KeyCode::Char(c) => s.filter.push(c),
        KeyCode::Backspace => {
            if s.filter.pop().is_none() {
                s.filter_editing = false;
            }
        }
        KeyCode::Esc => {
            s.filter.clear();
            s.filter_editing = false;
        }
        KeyCode::Enter => s.filter_editing = false,
        KeyCode::Up => s.wt_sel = s.wt_sel.saturating_sub(1),
        KeyCode::Down => s.wt_sel += 1,
        _ => return Outcome::default(),
    }
    s.wt_sel = s.wt_sel.min(s.visible_worktrees().len().saturating_sub(1));
    Outcome::redraw()
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

fn on_docker_key(s: &mut AppState, code: KeyCode) -> Option<Outcome> {
    let containers = s.docker.data.as_ref().map(|d| d.containers.clone()).unwrap_or_default();
    if step(&mut s.docker_sel, containers.len(), code) {
        return Some(Outcome::redraw());
    }
    match code {
        KeyCode::Char('K') => {
            let c = containers.get(s.docker_sel)?;
            s.confirm = Some(Confirm::Docker {
                title: format!("parar {}", c.name),
                lines: vec![format!("docker stop {}  ({})", c.name, c.image)],
                cmds: vec![docker::stop_cmd(c)],
            });
            Some(Outcome::redraw())
        }
        KeyCode::Char('X') if !containers.is_empty() => {
            let cmds = docker::down_all_plan(&containers);
            let mut lines: Vec<String> = Vec::new();
            let mut projects: Vec<&str> = containers.iter().filter_map(|c| c.compose.as_ref().map(|p| p.project.as_str())).collect();
            projects.dedup();
            for p in &projects {
                let n = containers.iter().filter(|c| c.compose.as_ref().is_some_and(|x| x.project == *p)).count();
                lines.push(format!("compose {p}: down --remove-orphans ({n} container{})", if n == 1 { "" } else { "s" }));
            }
            let loose = containers.iter().filter(|c| c.compose.is_none()).count();
            if loose > 0 {
                lines.push(format!("{loose} container{} fora de compose: docker stop", if loose == 1 { "" } else { "s" }));
            }
            s.confirm = Some(Confirm::Docker { title: format!("derrubar {} containers", containers.len()), lines, cmds });
            Some(Outcome::redraw())
        }
        _ => None,
    }
}

fn on_worktrees_key(s: &mut AppState, code: KeyCode) -> Option<Outcome> {
    let n = s.visible_worktrees().len();
    if step(&mut s.wt_sel, n, code) {
        return Some(Outcome::redraw());
    }
    match code {
        KeyCode::Char('/') => {
            s.filter_editing = true;
            Some(Outcome::redraw())
        }
        KeyCode::Esc if !s.filter.is_empty() => {
            s.filter.clear();
            s.wt_sel = 0;
            Some(Outcome::redraw())
        }
        KeyCode::Enter | KeyCode::Char('C') => {
            let w = s.selected_worktree()?;
            if w.prunable {
                s.notices.insert(0, format!("{} não existe mais em disco (git worktree prune)", w.name));
                return Some(Outcome::redraw());
            }
            let (dir, name) = (w.path.clone(), w.name.clone());
            if code == KeyCode::Enter {
                return Some(go_to(s, ExitTarget { dir, then: None }));
            }
            Some(open_claude(s, dir, "claude --continue".into(), format!("claude · {name}"), Then::Continue))
        }
        _ => None,
    }
}

fn on_sessions_key(s: &mut AppState, code: KeyCode) -> Option<Outcome> {
    let n = s.session_list().len();
    if step(&mut s.sess_sel, n, code) {
        return Some(Outcome::redraw());
    }
    let sess = (*s.session_list().get(s.sess_sel)?).clone();
    match code {
        KeyCode::Enter => Some(go_to(s, ExitTarget { dir: sess.cwd, then: None })),
        KeyCode::Char('r') if sess.status == Status::Ended => {
            let cmd = format!("claude --resume {}", sess.id);
            let title = format!("claude · {}", crate::ui::truncate(&sess.title, 30));
            Some(open_claude(s, sess.cwd, cmd, title, Then::Resume(sess.id)))
        }
        KeyCode::Char('r') => {
            s.notices.insert(0, format!("\"{}\" já está aberta (pid {}); Enter vai para a pasta dela", sess.title, sess.pid));
            Some(Outcome::redraw())
        }
        _ => None,
    }
}

/// Abre o Claude numa aba nova, sem fechar o cockpit. Sem como abrir aba (terminal não
/// reconhecido), cai no comportamento de sair e trocar de pasta.
fn open_claude(s: &mut AppState, dir: std::path::PathBuf, cmd: String, title: String, then: Then) -> Outcome {
    match s.launcher.clone() {
        Some(launcher) => Outcome { redraw: true, effect: Some(Effect::OpenTab { launcher, dir, cmd, title }) },
        None => go_to(s, ExitTarget { dir, then: Some(then) }),
    }
}

/// Com o wrapper do shell, sai e deixa o `cd` para ele. Sem wrapper, não dá para mudar a
/// pasta do shell: copia o comando e explica como ligar o wrapper.
fn go_to(s: &mut AppState, target: ExitTarget) -> Outcome {
    if s.has_cd_file {
        s.exit = Some(target);
        s.quit = true;
        return Outcome::default();
    }
    let cmd = target.command_line();
    s.notices.insert(0, format!("copiado: {cmd} · para o Enter trocar de pasta, ponha no ~/.zshrc: eval \"$(cockpit shell-init zsh)\""));
    Outcome { redraw: true, effect: Some(Effect::Copy(cmd)) }
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
        AppState::new(Config::default(), paths, themes, 0, vec![])
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
        let mut s = state();
        assert_eq!(s.focus, Panel::Worktrees, "o foco começa no seletor de worktrees");
        update(&mut s, key(KeyCode::Tab));
        assert_eq!(s.focus, Panel::System);
        update(&mut s, key(KeyCode::Tab));
        update(&mut s, key(KeyCode::Tab));
        assert_eq!(s.focus, Panel::Hero);
        update(&mut s, key(KeyCode::BackTab));
        assert_eq!(s.focus, Panel::Sessions);
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

    fn with_sessions(s: &mut AppState) {
        use crate::providers::claude::Session;
        let sess = |id: &str, cwd: &str, st: Status| Session {
            id: id.into(),
            pid: 10,
            cwd: cwd.into(),
            status: st,
            title: format!("t-{id}"),
            ..Default::default()
        };
        let snap = SessionsSnapshot {
            live: vec![
                sess("a", "/h/v360/.claude/worktrees/processable-file-cfs/src", Status::Waiting),
                sess("b", "/h/v360", Status::Busy),
            ],
            recent: vec![sess("c", "/h/v360/.claude/worktrees/build-s4", Status::Ended)],
        };
        update(s, Action::Sessions(Snapshot::ok(snap)));
    }

    #[test]
    fn enter_on_worktree_exits_with_cd_when_wrapped() {
        let mut s = state();
        s.has_cd_file = true;
        with_worktrees(&mut s);
        update(&mut s, key(KeyCode::Down));
        update(&mut s, key(KeyCode::Enter));
        assert!(s.quit);
        assert_eq!(s.exit, Some(ExitTarget { dir: "/h/v360/.claude/worktrees/processable-file-cfs".into(), then: None }));
    }

    #[test]
    fn continue_opens_a_new_tab_and_keeps_running() {
        let mut s = state();
        s.has_cd_file = true;
        s.launcher = Some(crate::terminal::Launcher::Tmux);
        with_worktrees(&mut s);
        update(&mut s, key(KeyCode::Down));
        let out = update(&mut s, key(KeyCode::Char('C')));
        assert!(!s.quit, "o cockpit continua aberto");
        assert_eq!(
            out.effect,
            Some(Effect::OpenTab {
                launcher: crate::terminal::Launcher::Tmux,
                dir: "/h/v360/.claude/worktrees/processable-file-cfs".into(),
                cmd: "claude --continue".into(),
                title: "claude · processable-file-cfs".into(),
            })
        );
    }

    #[test]
    fn i_opens_the_legend_and_any_key_closes() {
        let mut s = state();
        update(&mut s, key(KeyCode::Char('i')));
        assert!(s.show_legend);
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.show_legend && !s.quit);
    }

    #[test]
    fn continue_key_and_no_wrapper_copies() {
        let mut s = state();
        with_worktrees(&mut s);
        let out = update(&mut s, key(KeyCode::Char('C')));
        assert!(!s.quit, "sem wrapper não sai");
        assert_eq!(out.effect, Some(Effect::Copy("cd /h/v360 && claude --continue".into())));
        assert!(s.notices[0].starts_with("copiado"));
    }

    #[test]
    fn filter_types_everything_and_narrows() {
        let mut s = state();
        with_worktrees(&mut s);
        update(&mut s, key(KeyCode::Char('/')));
        update(&mut s, key(KeyCode::Char('q')));
        assert!(!s.quit, "q dentro do filtro é texto");
        assert!(s.visible_worktrees().is_empty());
        update(&mut s, key(KeyCode::Backspace));
        for c in "cfs".chars() {
            update(&mut s, key(KeyCode::Char(c)));
        }
        // "cfs" casa com processable-file-cfs e também com b-processable-file-cfs; build-s4 não
        let names: Vec<String> = s.visible_worktrees().iter().map(|(r, w)| s.worktrees.data.as_ref().unwrap().repos[*r].worktrees[*w].name.clone()).collect();
        assert_eq!(names, ["processable-file-cfs"]);
        update(&mut s, key(KeyCode::Enter));
        assert!(!s.filter_editing && s.filter == "cfs");
        update(&mut s, key(KeyCode::Esc));
        assert_eq!(s.visible_worktrees().len(), 3);
    }

    #[test]
    fn sessions_map_to_the_deepest_worktree() {
        let mut s = state();
        with_worktrees(&mut s);
        with_sessions(&mut s);
        assert_eq!(s.wt_sessions.get(std::path::Path::new("/h/v360/.claude/worktrees/processable-file-cfs")), Some(&vec![0]));
        assert_eq!(s.wt_sessions.get(std::path::Path::new("/h/v360")), Some(&vec![1]));
        assert_eq!(s.waiting_count(), 1);
        let first = s.session_list()[0].clone();
        assert_eq!(s.session_place(&first), "v360/processable-file-cfs");
    }

    #[test]
    fn resume_only_for_ended_sessions() {
        let mut s = state();
        s.has_cd_file = true;
        with_worktrees(&mut s);
        with_sessions(&mut s);
        s.focus = Panel::Sessions;
        update(&mut s, key(KeyCode::Char('r')));
        assert!(!s.quit && s.notices[0].contains("já está aberta"));
        update(&mut s, key(KeyCode::Char('G')));
        // sem lançador de aba: sai e retoma pelo wrapper
        update(&mut s, key(KeyCode::Char('r')));
        assert_eq!(
            s.exit,
            Some(ExitTarget { dir: "/h/v360/.claude/worktrees/build-s4".into(), then: Some(Then::Resume("c".into())) })
        );
        // com lançador: aba nova, cockpit segue aberto
        let mut s2 = state();
        s2.launcher = Some(crate::terminal::Launcher::Tmux);
        with_worktrees(&mut s2);
        with_sessions(&mut s2);
        s2.focus = Panel::Sessions;
        update(&mut s2, key(KeyCode::Char('G')));
        let out = update(&mut s2, key(KeyCode::Char('r')));
        assert!(!s2.quit);
        assert!(matches!(out.effect, Some(Effect::OpenTab { ref cmd, .. }) if cmd == "claude --resume c"));
    }

    #[test]
    fn session_selection_follows_the_conversation() {
        let mut s = state();
        with_sessions(&mut s);
        s.sess_sel = 1; // "b"
        use crate::providers::claude::Session;
        let snap = SessionsSnapshot {
            live: vec![
                Session { id: "b".into(), status: Status::Waiting, ..Default::default() },
                Session { id: "a".into(), status: Status::Idle, ..Default::default() },
            ],
            recent: vec![],
        };
        update(&mut s, Action::Sessions(Snapshot::ok(snap)));
        assert_eq!(s.session_list()[s.sess_sel].id, "b");
    }

    fn with_containers(s: &mut AppState) {
        use crate::providers::docker::{Compose, Container};
        let c = |id: &str, name: &str, project: Option<&str>| Container {
            id: id.into(),
            name: name.into(),
            image: "img".into(),
            compose: project.map(|p| Compose { project: p.into(), ..Default::default() }),
            ..Default::default()
        };
        let snap = DockerSnapshot { containers: vec![c("a1", "v360_db", Some("v360")), c("a2", "v360_redis", Some("v360")), c("b1", "solto", None)] };
        update(s, Action::Docker(Snapshot::ok(snap)));
        s.focus = Panel::Docker;
    }

    #[test]
    fn docker_stop_one_needs_confirmation() {
        let mut s = state();
        with_containers(&mut s);
        update(&mut s, key(KeyCode::Down));
        update(&mut s, key(KeyCode::Char('K')));
        assert!(matches!(&s.confirm, Some(Confirm::Docker { title, .. }) if title == "parar v360_redis"));
        let out = update(&mut s, key(KeyCode::Char('y')));
        match out.effect {
            Some(Effect::Docker { cmds, .. }) => assert_eq!(cmds[0].display(), "docker stop a2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn docker_down_all_explains_and_cancels() {
        let mut s = state();
        with_containers(&mut s);
        update(&mut s, key(KeyCode::Char('X')));
        let Some(Confirm::Docker { title, lines, cmds }) = s.confirm.clone() else { panic!() };
        assert_eq!(title, "derrubar 3 containers");
        assert_eq!(lines, ["compose v360: down --remove-orphans (2 containers)", "1 container fora de compose: docker stop"]);
        assert_eq!(cmds.iter().map(|c| c.display()).collect::<Vec<_>>(), ["docker compose -p v360 down --remove-orphans", "docker stop b1"]);
        let out = update(&mut s, key(KeyCode::Char('n')));
        assert!(out.effect.is_none() && s.confirm.is_none(), "qualquer tecla que não y cancela");
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
