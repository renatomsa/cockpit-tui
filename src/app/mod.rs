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

/// Roda até o usuário sair. Devolve o destino escolhido (Enter numa worktree/sessão).
pub async fn run(mut state: AppState) -> Result<Option<crate::shell::ExitTarget>> {
    let (geo_tx, mut geo_rx) = watch::channel(Snapshot::default());
    providers::geo::spawn(state.config.location.clone(), state.paths.cache_dir.join("geo.json"), geo_tx);
    // O provider com `city` fixa publica e termina: pega o valor já disponível.
    let mut geo_alive = true;
    if geo_rx.has_changed().unwrap_or(false) {
        update(&mut state, Action::Geo(geo_rx.borrow_and_update().clone()));
    }

    let (sys_tx, mut sys_rx) = watch::channel(Snapshot::default());
    providers::system::spawn(state.config.system.clone(), sys_tx);
    let (ping_tx, mut ping_rx) = watch::channel(Snapshot::default());
    providers::ping::spawn(state.config.system.ping_target.clone(), state.config.system.ping_interval_secs, ping_tx);
    let (wt_tx, mut wt_rx) = watch::channel(Snapshot::default());
    providers::git::spawn(state.config.worktrees.clone(), wt_tx);
    let (sess_tx, mut sess_rx) = watch::channel(Snapshot::default());
    providers::claude::spawn(state.config.claude.clone(), sess_tx);
    let (usage_tx, mut usage_rx) = watch::channel(Snapshot::default());
    providers::usage::spawn(state.config.claude.clone(), usage_tx);
    let (specs_tx, mut specs_rx) = watch::channel(Snapshot::default());
    providers::specs::spawn(specs_tx);
    let (net_tx, mut net_rx) = watch::channel(Snapshot::default());
    let host_net = state.config.network.wsl_host && crate::platform::is_wsl();
    if host_net {
        providers::hostnet::spawn(net_tx);
    }
    let mut net_alive = host_net;
    let (wx_tx, mut wx_rx) = watch::channel(Snapshot::default());
    providers::weather::spawn(state.config.weather.clone(), geo_rx.clone(), wx_tx);
    let mut wx_alive = state.config.weather.enabled;
    let (docker_tx, mut docker_rx) = watch::channel(Snapshot::default());
    providers::docker::spawn(docker_tx);
    let (cal_tx, mut cal_rx) = watch::channel(Snapshot::default());
    if state.demo_agenda {
        update(&mut state, Action::Calendar(Snapshot::ok(providers::calendar::demo(Utc::now()))));
    } else {
        providers::calendar::spawn(state.config.calendar.clone(), state.paths.google_token(), cal_tx);
    }
    let mut cal_alive = !state.demo_agenda;
    let (mut sys_alive, mut ping_alive, mut wt_alive, mut sess_alive, mut usage_alive, mut specs_alive, mut docker_alive) =
        (true, true, true, true, true, true, true);
    // Resultados de efeitos (ex.: o kill) voltam como ações.
    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<Action>();

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
    let mut events = EventStream::new();
    let mut redraw = true;

    let result = loop {
        if redraw {
            if let Err(e) = terminal.draw(|f| ui::draw(f, &state)) {
                break Err(e.into());
            }
            redraw = false;
        }

        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => Action::Key(k),
                Some(Ok(Event::Resize(..))) => Action::Resize,
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
            changed = ping_rx.changed(), if ping_alive => match changed {
                Ok(()) => Action::Ping(ping_rx.borrow_and_update().clone()),
                Err(_) => { ping_alive = false; continue }
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
            changed = specs_rx.changed(), if specs_alive => match changed {
                Ok(()) => Action::Specs(specs_rx.borrow_and_update().clone()),
                Err(_) => { specs_alive = false; continue }
            },
            changed = docker_rx.changed(), if docker_alive => match changed {
                Ok(()) => Action::Docker(docker_rx.borrow_and_update().clone()),
                Err(_) => { docker_alive = false; continue }
            },
            changed = cal_rx.changed(), if cal_alive => match changed {
                Ok(()) => Action::Calendar(cal_rx.borrow_and_update().clone()),
                Err(_) => { cal_alive = false; continue }
            },
            changed = net_rx.changed(), if net_alive => match changed {
                Ok(()) => Action::HostNet(net_rx.borrow_and_update().clone()),
                Err(_) => { net_alive = false; continue }
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
        };

        let out = update(&mut state, action);
        redraw |= out.redraw;
        if let Some(effect) = out.effect {
            run_effect(&state, effect, act_tx.clone());
        }
        if state.quit {
            break Ok(());
        }
    };

    ratatui::restore();
    result.map(|()| state.exit.take())
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
        Effect::Copy(text) => crate::shell::osc52_copy(&text),
        Effect::SpeedTest => {
            let cfg = state.config.network.clone();
            tokio::spawn(async move {
                crate::providers::speedtest::run(cfg, |ev| {
                    let _ = results.send(Action::Speed(ev));
                })
                .await;
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
        Effect::OpenTab { launcher, dir, cmd, title } => {
            tokio::task::spawn_blocking(move || {
                let msg = match crate::terminal::open(&launcher, &dir, &cmd, &title) {
                    Ok(()) => format!("{cmd} aberto numa aba nova ({})", launcher.name()),
                    Err(e) => format!("não consegui abrir a aba: {e:#}"),
                };
                let _ = results.send(Action::Notice(msg));
            });
        }
        Effect::RemoveWorktree(r) => {
            tokio::spawn(async move {
                let path = r.path.display().to_string();
                let args: Vec<&str> = if r.prunable {
                    vec!["worktree", "prune"]
                } else if r.dirty > 0 {
                    vec!["worktree", "remove", "--force", &path]
                } else {
                    vec!["worktree", "remove", &path]
                };
                let out = tokio::process::Command::new("git").arg("-C").arg(&r.repo).args(&args).output().await;
                let msg = match out {
                    Ok(o) if o.status.success() => format!("worktree {} excluída", r.name),
                    Ok(o) => format!("git worktree falhou: {}", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => format!("git não executou: {e}"),
                };
                tracing::info!("{msg}");
                let _ = results.send(Action::Notice(msg));
            });
        }
        Effect::Docker { cmds, label } => {
            let _ = results.send(Action::Notice(format!("{label}…")));
            tokio::spawn(async move {
                let msg = match crate::providers::docker::run(&cmds).await {
                    Ok(()) => format!("{label}: feito"),
                    Err(e) => format!("{label} falhou: {e:#}"),
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
