//! `update`: única função que muda o estado. Pura (sem I/O); efeitos voltam como `Effect`.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::state::{AppState, KillTarget, Panel, ProcCol};
use crate::providers::{Snapshot, geo::Geo, ping::PingSnapshot, system::SystemSnapshot};

#[derive(Debug)]
pub enum Action {
    Tick(DateTime<Utc>),
    Key(KeyEvent),
    Resize,
    Geo(Snapshot<Geo>),
    /// Em `Box`: o snapshot de sistema é bem maior que as outras ações.
    System(Box<Snapshot<SystemSnapshot>>),
    Ping(Snapshot<PingSnapshot>),
    /// Mensagem para a barra de status (ex.: resultado de um efeito).
    Notice(String),
}

/// Efeito colateral pedido pelo `update`, executado pelo loop fora da thread de desenho.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    PersistTheme(String),
    /// SIGTERM, já confirmado pelo usuário.
    Kill(KillTarget),
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
    if let Some(target) = s.confirm_kill.take() {
        let effect = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')).then_some(Effect::Kill(target));
        return Outcome { redraw: true, effect };
    }
    if s.show_help {
        s.show_help = false;
        return Outcome::redraw();
    }
    if s.focus == Panel::System && on_system_key(s, k.code) {
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
                s.confirm_kill = Some(KillTarget { pid: p.top_pid, name: p.name.clone() });
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
        assert_eq!(s.confirm_kill, Some(KillTarget { pid: 200, name: "cargo".into() }));
        // qualquer tecla que não seja y cancela
        let out = update(&mut s, key(KeyCode::Char('n')));
        assert!(out.effect.is_none() && s.confirm_kill.is_none());
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
        assert!(s.confirm_kill.is_none());
        assert!(s.notices[0].contains("não encerro"));
    }

    #[test]
    fn system_keys_only_when_focused() {
        let mut s = state();
        with_procs(&mut s);
        update(&mut s, key(KeyCode::Char('K')));
        assert!(s.confirm_kill.is_none());
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
