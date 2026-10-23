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
    pub chat: Option<Rect>,
    pub system: Option<Rect>,
    pub status: Rect,
}

/// Altura que o painel Sistema precisa: caixas de CPU e memória, TOP CPU/MEM e limites
/// do Claude.
const SYSTEM_H: u16 = 27;

pub fn compute(area: Rect, cfg: &LayoutConfig) -> Areas {
    let [main, status] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    if area.height < cfg.compact_max_rows {
        return Areas {
            mode: Mode::Compact,
            hero: main,
            agenda: None,
            chat: None,
            system: None,
            status,
        };
    }

    if area.width >= cfg.wide_min_cols {
        let left_pct = cfg.left_percent.clamp(30, 80);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(left_pct), Constraint::Percentage(100 - left_pct)]).areas(main);
        // À esquerda, o chat com quase tudo e a agenda embaixo; à direita, o relógio em
        // cima e o sistema embaixo. O relógio grande (18 linhas, com clima) só quando sobra
        // altura para o sistema inteiro.
        let hero_h = if right.height >= 18 + SYSTEM_H { 18 } else { 12 };
        let [chat, agenda] = Layout::vertical([Constraint::Min(8), Constraint::Length(7)]).areas(left);
        let [hero, system] = Layout::vertical([Constraint::Length(hero_h), Constraint::Min(5)]).areas(right);
        return Areas { mode: Mode::Wide, hero, agenda: Some(agenda), chat: Some(chat), system: Some(system), status };
    }

    let [chat, agenda, hero, system] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(14),
    ])
    .areas(main);
    Areas { mode: Mode::Stacked, hero, agenda: Some(agenda), chat: Some(chat), system: Some(system), status }
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
        // chat no alto à esquerda, agenda embaixo dele
        let chat = a.chat.unwrap();
        assert_eq!((chat.x, chat.y, chat.width, chat.height), (0, 0, 124, 42));
        assert_eq!(a.agenda.unwrap().y, 42);
        // relógio no alto à direita, sistema embaixo
        assert_eq!((a.hero.x, a.hero.y, a.hero.height), (124, 0, 18));
        assert_eq!(a.system.unwrap().y, 18);
        assert_eq!(compute(Rect::new(0, 0, 200, 40), &LayoutConfig::default()).hero.height, 12);
    }
}
