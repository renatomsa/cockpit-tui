//! `update`: única função que muda o estado. Pura (sem I/O); efeitos voltam como `Effect`.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::state::AppState;
use crate::providers::{Snapshot, geo::Geo};

#[derive(Debug)]
pub enum Action {
    Tick(DateTime<Utc>),
    Key(KeyEvent),
    Resize,
    Geo(Snapshot<Geo>),
}

/// Efeito colateral pedido pelo `update`, executado pelo loop fora da thread de desenho.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    PersistTheme(String),
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
    if s.show_help {
        s.show_help = false;
        return Outcome::redraw();
    }
    match k.code {
        KeyCode::Char('q') => s.quit = true,
        KeyCode::Char('?') => s.show_help = true,
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
        update(&mut s, key(KeyCode::Tab));
        assert_eq!(s.focus, Panel::Agenda);
        update(&mut s, key(KeyCode::BackTab));
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
