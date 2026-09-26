//! Layout responsivo: largo (duas colunas), empilhado ou compacto (só o hero).

use ratatui::layout::{Constraint, Layout, Rect};

use crate::config::LayoutConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Wide,
    Stacked,
    Compact,
}

#[derive(Debug)]
pub struct Areas {
    #[cfg_attr(not(test), allow(dead_code))]
    pub mode: Mode,
    pub hero: Rect,
    pub agenda: Option<Rect>,
    pub worktrees: Option<Rect>,
    pub system: Option<Rect>,
    pub sessions: Option<Rect>,
    pub status: Rect,
}

pub fn compute(area: Rect, cfg: &LayoutConfig) -> Areas {
    let [main, status] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    if area.height < cfg.compact_max_rows {
        return Areas { mode: Mode::Compact, hero: main, agenda: None, worktrees: None, system: None, sessions: None, status };
    }

    let hero_h = if main.height >= 45 { 16 } else { 12 };

    if area.width >= cfg.wide_min_cols {
        let left_pct = cfg.left_percent.clamp(30, 80);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(left_pct), Constraint::Percentage(100 - left_pct)]).areas(main);
        let [hero, agenda, worktrees] =
            Layout::vertical([Constraint::Length(hero_h), Constraint::Length(7), Constraint::Min(5)]).areas(left);
        let [system, sessions] = Layout::vertical([Constraint::Percentage(55), Constraint::Min(5)]).areas(right);
        return Areas {
            mode: Mode::Wide,
            hero,
            agenda: Some(agenda),
            worktrees: Some(worktrees),
            system: Some(system),
            sessions: Some(sessions),
            status,
        };
    }

    let [hero, agenda, system, worktrees, sessions] = Layout::vertical([
        Constraint::Length(12),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Fill(2),
        Constraint::Fill(1),
    ])
    .areas(main);
    Areas {
        mode: Mode::Stacked,
        hero,
        agenda: Some(agenda),
        worktrees: Some(worktrees),
        system: Some(system),
        sessions: Some(sessions),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_mode_by_size() {
        let cfg = LayoutConfig::default();
        assert_eq!(compute(Rect::new(0, 0, 200, 50), &cfg).mode, Mode::Wide);
        assert_eq!(compute(Rect::new(0, 0, 100, 45), &cfg).mode, Mode::Stacked);
        assert_eq!(compute(Rect::new(0, 0, 200, 24), &cfg).mode, Mode::Compact);
    }

    #[test]
    fn wide_split_and_status_line() {
        let a = compute(Rect::new(0, 0, 200, 50), &LayoutConfig::default());
        assert_eq!(a.status, Rect::new(0, 49, 200, 1));
        assert_eq!(a.hero.width, 124);
        assert_eq!(a.hero.height, 16);
        assert_eq!(a.system.unwrap().x, 124);
    }
}
