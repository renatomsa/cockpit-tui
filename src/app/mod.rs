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
    let (ping_tx, mut ping_rx) = watch::channel(Snapshot::default());
    providers::ping::spawn(state.config.system.ping_target.clone(), state.config.system.ping_interval_secs, ping_tx);
    let (mut sys_alive, mut ping_alive) = (true, true);
    // Resultados de efeitos (ex.: o kill) voltam como ações.
    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<Action>();

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
    result
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
