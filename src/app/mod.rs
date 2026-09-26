//! Loop principal: junta teclado, tick de relógio e providers num único `select!`.
//! Só desenha quando algo mudou; parado, acorda uma vez por segundo.

pub mod state;
pub mod update;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::{sync::watch, time::Instant};

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
            _ = tokio::time::sleep_until(next_tick()) => Action::Tick(Utc::now()),
            changed = geo_rx.changed(), if geo_alive => match changed {
                Ok(()) => Action::Geo(geo_rx.borrow_and_update().clone()),
                Err(_) => { geo_alive = false; continue }
            },
        };

        let out = update(&mut state, action);
        redraw |= out.redraw;
        if let Some(effect) = out.effect {
            run_effect(&state, effect);
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
fn next_tick() -> Instant {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let into = since.subsec_nanos() as u64;
    Instant::now() + Duration::from_nanos(1_000_000_000 - into) + Duration::from_millis(2)
}

fn run_effect(state: &AppState, effect: Effect) {
    match effect {
        Effect::PersistTheme(name) => {
            let paths = state.paths.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = (PersistedState { theme: Some(name) }).save(&paths) {
                    tracing::warn!("não gravei o tema escolhido: {e:#}");
                }
            });
        }
    }
}
