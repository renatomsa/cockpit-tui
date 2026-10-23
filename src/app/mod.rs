//! Loop principal: junta teclado, tick de relógio e providers num único `select!`.
//! Só desenha quando algo mudou; parado, acorda uma vez por segundo.

pub mod state;
pub mod update;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use self::{
    state::AppState,
    update::{Action, Effect, update},
};
use crate::{
    config::PersistedState,
    providers::{self, Snapshot},
    ui,
};

/// Roda até o usuário sair.
pub async fn run(mut state: AppState) -> Result<()> {
    let (geo_tx, mut geo_rx) = watch::channel(Snapshot::default());
    providers::geo::spawn(state.config.location.clone(), state.paths.cache_dir.join("geo.json"), geo_tx);
    // O provider com `city` fixa publica e termina: pega o valor já disponível.
    let mut geo_alive = true;
    if geo_rx.has_changed().unwrap_or(false) {
        update(&mut state, Action::Geo(geo_rx.borrow_and_update().clone()));
    }

    let (sys_tx, mut sys_rx) = watch::channel(Snapshot::default());
    providers::system::spawn(state.config.system.clone(), sys_tx);
    let (wt_tx, mut wt_rx) = watch::channel(Snapshot::default());
    providers::git::spawn(state.config.worktrees.clone(), wt_tx);
    let (sess_tx, mut sess_rx) = watch::channel(Snapshot::default());
    providers::claude::spawn(state.config.claude.clone(), state.paths.state_dir.clone(), sess_tx);
    let (usage_tx, mut usage_rx) = watch::channel(Snapshot::default());
    providers::usage::spawn(state.config.claude.clone(), usage_tx);
    let (wx_tx, mut wx_rx) = watch::channel(Snapshot::default());
    providers::weather::spawn(state.config.weather.clone(), geo_rx.clone(), wx_tx);
    let mut wx_alive = state.config.weather.enabled;
    let (cal_tx, mut cal_rx) = watch::channel(Snapshot::default());
    if state.demo_agenda {
        update(&mut state, Action::Calendar(Snapshot::ok(providers::calendar::demo(Utc::now()))));
    } else {
        providers::calendar::spawn(state.config.calendar.clone(), state.paths.google_token(), state.paths.ical_store(), cal_tx);
    }
    let mut cal_alive = !state.demo_agenda;
    let (mut sys_alive, mut wt_alive, mut sess_alive, mut usage_alive) = (true, true, true, true);
    // Resultados de efeitos (ex.: o kill) voltam como ações.
    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<Action>();
    // Chats embutidos: processos aqui; a tela de cada um vai para o estado.
    let mut hub = crate::chat::Hub::new(act_tx.clone());
    let chat_dirty = hub.dirty.clone();
    // Ditado: o processo do gravador fica aqui; o texto volta como ação.
    let mut voice = crate::voice::Voice::new(act_tx.clone());

    // Recarga a quente: config.toml e a pasta de temas.
    let (cfg_tx, mut cfg_rx) = mpsc::channel::<()>(1);
    let config_name = state.config_path.file_name().map(|n| n.to_os_string());
    let mut cfg_watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && providers::is_change(&ev)
            && ev.paths.iter().any(|p| p.extension().is_some_and(|x| x == "toml") || p.file_name().map(|n| n.to_os_string()) == config_name)
        {
            let _ = cfg_tx.try_send(());
        }
    })
    .map_err(|e| tracing::warn!("sem recarga a quente: {e}"))
    .ok();
    if let Some(w) = cfg_watcher.as_mut() {
        use notify::Watcher;
        let _ = std::fs::create_dir_all(state.paths.themes_dir());
        for dir in [state.config_path.parent().map(|p| p.to_path_buf()), Some(state.paths.themes_dir())].into_iter().flatten() {
            let _ = w.watch(&dir, notify::RecursiveMode::NonRecursive);
        }
    }

    let mut terminal = ratatui::init();
    // Colar chega como um evento só, e não como uma tecla por caractere (o Enter de cada
    // linha mandaria a mensagem do chat pela metade).
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    // Mouse para a roda e os cliques chegarem ao chat (o `claude` em tela cheia rola com
    // eles). Selecionar texto do resto do cockpit passa a ser com Shift+arrastar.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let mut events = EventStream::new();
    let mut redraw = true;

    let result = loop {
        if redraw {
            // O PTY dos chats acompanha o tamanho do painel.
            if let Ok(size) = terminal.size()
                && let Some(r) = ui::layout::compute(ratatui::layout::Rect::new(0, 0, size.width, size.height), &state.config.layout).chat
            {
                hub.resize(r.height.saturating_sub(2), r.width.saturating_sub(2), &state.chats);
            }
            if let Err(e) = terminal.draw(|f| ui::draw(f, &state)) {
                break Err(e.into());
            }
            redraw = false;
        }

        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => Action::Key(k),
                Some(Ok(Event::Resize(..))) => Action::Resize,
                Some(Ok(Event::Paste(text))) => Action::Paste(text),
                Some(Ok(Event::Mouse(m))) => match chat_screen(&terminal, &state) {
                    Some(r) if r.contains(ratatui::layout::Position::new(m.column, m.row)) => {
                        Action::ChatMouse(crossterm::event::MouseEvent { column: m.column - r.x, row: m.row - r.y, ..m })
                    }
                    _ => continue,
                },
                Some(Ok(_)) => continue,
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            _ = tokio::time::sleep_until(next_tick(state.animating())) => Action::Tick(Utc::now()),
            changed = geo_rx.changed(), if geo_alive => match changed {
                Ok(()) => Action::Geo(geo_rx.borrow_and_update().clone()),
                Err(_) => { geo_alive = false; continue }
            },
            changed = sys_rx.changed(), if sys_alive => match changed {
                Ok(()) => Action::System(Box::new(sys_rx.borrow_and_update().clone())),
                Err(_) => { sys_alive = false; continue }
            },
            changed = wt_rx.changed(), if wt_alive => match changed {
                Ok(()) => Action::Worktrees(wt_rx.borrow_and_update().clone()),
                Err(_) => { wt_alive = false; continue }
            },
            changed = sess_rx.changed(), if sess_alive => match changed {
                Ok(()) => Action::Sessions(sess_rx.borrow_and_update().clone()),
                Err(_) => { sess_alive = false; continue }
            },
            changed = usage_rx.changed(), if usage_alive => match changed {
                Ok(()) => Action::Usage(usage_rx.borrow_and_update().clone()),
                Err(_) => { usage_alive = false; continue }
            },
            changed = cal_rx.changed(), if cal_alive => match changed {
                Ok(()) => Action::Calendar(cal_rx.borrow_and_update().clone()),
                Err(_) => { cal_alive = false; continue }
            },
            changed = wx_rx.changed(), if wx_alive => match changed {
                Ok(()) => Action::Weather(wx_rx.borrow_and_update().clone()),
                Err(_) => { wx_alive = false; continue }
            },
            Some(()) = cfg_rx.recv() => {
                // Editores salvam em várias escritas: espera assentar e relê fora do loop.
                let (path, themes_dir, truecolor, tx) = (state.config_path.clone(), state.paths.themes_dir(), state.truecolor, act_tx.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let r = tokio::task::spawn_blocking(move || update::Reloaded {
                        config: crate::config::load(Some(path)),
                        themes: crate::theme::ThemeRegistry::load(&themes_dir, truecolor),
                    })
                    .await;
                    if let Ok(r) = r {
                        let _ = tx.send(Action::Reloaded(Box::new(r)));
                    }
                });
                while cfg_rx.try_recv().is_ok() {}
                continue;
            }
            Some(a) = act_rx.recv() => a,
            () = chat_dirty.notified() => {
                // Junta a saída de alguns milissegundos num quadro só.
                tokio::time::sleep(Duration::from_millis(8)).await;
                redraw = true;
                continue;
            }
        };

        let out = update(&mut state, action);
        redraw |= out.redraw;
        match out.effect {
            Some(Effect::Chat(cmd)) => {
                if let Some(a) = hub.apply(cmd) {
                    redraw |= update(&mut state, a).redraw;
                }
            }
            Some(Effect::Voice(cmd)) => {
                if let Some(a) = voice.apply(cmd, &state.config.voice) {
                    redraw |= update(&mut state, a).redraw;
                }
            }
            Some(effect) => run_effect(&state, effect, act_tx.clone()),
            None => {}
        }
        if state.quit {
            break Ok(());
        }
    };

    drop(hub);
    drop(voice);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture, crossterm::event::DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Tela do chat na janela (o painel por dentro da moldura), se o layout mostra o chat.
fn chat_screen(terminal: &ratatui::DefaultTerminal, state: &AppState) -> Option<ratatui::layout::Rect> {
    let size = terminal.size().ok()?;
    let r = ui::layout::compute(ratatui::layout::Rect::new(0, 0, size.width, size.height), &state.config.layout).chat?;
    Some(ratatui::layout::Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2)))
}

/// Próxima virada de segundo do relógio de parede (+2 ms de folga), para os
/// segundos do relógio grande mudarem no instante certo e não "pularem".
/// Com `fast`, vira a cada meio segundo (pisca-pisca a 2 Hz).
fn next_tick(fast: bool) -> Instant {
    let period: u64 = if fast { 500_000_000 } else { 1_000_000_000 };
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let into = since.subsec_nanos() as u64 % period;
    Instant::now() + Duration::from_nanos(period - into) + Duration::from_millis(2)
}

fn run_effect(state: &AppState, effect: Effect, results: mpsc::UnboundedSender<Action>) {
    match effect {
        Effect::PersistTheme(name) => {
            let paths = state.paths.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = (PersistedState { theme: Some(name) }).save(&paths) {
                    tracing::warn!("não gravei o tema escolhido: {e:#}");
                }
            });
        }
        // Executados no loop, que é o dono do `Hub` e do `Voice`.
        Effect::Chat(_) | Effect::Voice(_) => {}
        Effect::Copy(text) => crate::shell::osc52_copy(&text),
        Effect::Permission(p, allow) => {
            let state_dir = state.paths.state_dir.clone();
            tokio::task::spawn_blocking(move || {
                let msg = match crate::claude_hook::decide(&state_dir, &p, allow) {
                    Ok(()) => format!("{} {}: {}", if allow { "permitido" } else { "negado" }, p.tool, crate::ui::truncate(&p.summary, 60)),
                    Err(e) => format!("não consegui responder o pedido: {e}"),
                };
                let _ = results.send(Action::Notice(msg));
            });
        }
        Effect::OpenUrl(url) => {
            tokio::task::spawn_blocking(move || {
                let msg = match crate::platform::open_url(&url) {
                    Ok(()) => format!("abrindo {url}"),
                    Err(e) => format!("não consegui abrir o navegador: {e:#}"),
                };
                let _ = results.send(Action::Notice(msg));
            });
        }
        Effect::Kill(t) => {
            tokio::task::spawn_blocking(move || {
                let res = std::process::Command::new("kill").args(["-TERM", &t.pid.to_string()]).output();
                let msg = match res {
                    Ok(o) if o.status.success() => format!("SIGTERM enviado para {} (pid {})", t.name, t.pid),
                    Ok(o) => format!("kill {} falhou: {}", t.pid, String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => format!("kill {} falhou: {e}", t.pid),
                };
                tracing::info!("{msg}");
                let _ = results.send(Action::Notice(msg));
            });
        }
    }
}
