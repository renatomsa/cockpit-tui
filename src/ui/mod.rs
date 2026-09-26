//! Desenho. Só lê `AppState`; nunca faz I/O nem muda estado.

mod bigclock;
mod hero;
pub mod layout;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};

use crate::app::state::{AppState, Panel};

pub fn draw(f: &mut Frame, s: &AppState) {
    let t = s.theme();
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), area);

    let a = layout::compute(area, &s.config.layout);
    hero::draw(f, a.hero, s);

    let placeholders = [
        (a.agenda, Panel::Agenda, "Agenda", "Próximas reuniões do Google Calendar · F3 (`cockpit auth google`)"),
        (a.system, Panel::System, "Sistema", "CPU, memória, swap, disco, rede, ping e maiores ofensores · F1"),
        (a.worktrees, Panel::Worktrees, "Worktrees", "Worktrees do Claude por projeto, com seletor que dá `cd` · F2"),
        (a.sessions, Panel::Sessions, "Claude", "Conversas do Claude em aberto (esperando / trabalhando) · F2"),
    ];
    for (rect, panel, title, text) in placeholders {
        if let Some(r) = rect {
            placeholder(f, r, s, panel, title, text);
        }
    }

    statusbar(f, a.status, s);
    if s.show_help {
        help(f, area, s);
    }
}

/// Moldura padrão de painel; o painel em foco ganha a cor de destaque.
pub(crate) fn panel_block<'a>(s: &AppState, panel: Panel, title: Option<&'a str>) -> Block<'a> {
    let t = s.theme();
    let focused = s.focus == panel;
    let mut b = Block::bordered()
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .border_style(Style::new().fg(if focused { t.accent } else { t.border }));
    if let Some(title) = title {
        let style = if focused { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.fg) };
        b = b.title(Span::styled(format!(" {title} "), style));
    }
    b
}

fn placeholder(f: &mut Frame, area: Rect, s: &AppState, panel: Panel, title: &str, text: &str) {
    let block = panel_block(s, panel, Some(title));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(Line::styled(text, Style::new().fg(s.theme().muted))).wrap(Wrap { trim: true }),
        inner,
    );
}

fn statusbar(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let key = |k: &'static str| Span::styled(k, Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    let lbl = |l: &'static str| Span::styled(l, Style::new().fg(t.muted));

    let left: Line = match s.notices.first() {
        Some(n) => Line::from(vec![
            Span::styled(format!(" ⚠ {n} "), Style::new().fg(t.warn)),
            key("x"),
            lbl(" dispensar"),
        ]),
        None => Line::from(vec![
            Span::raw(" "),
            key("Tab"),
            lbl(" foco  "),
            key("t"),
            lbl(" tema  "),
            key("?"),
            lbl(" ajuda  "),
            key("q"),
            lbl(" sair"),
        ]),
    };

    let geo = match (&s.geo.data, &s.geo.error) {
        (None, Some(_)) => Span::styled("geo offline", Style::new().fg(t.warn)),
        (Some(g), Some(_)) => {
            let age = s.geo.updated_at.map(|u| (s.now - u).num_minutes().max(0)).unwrap_or(0);
            Span::styled(format!("geo {} · offline, dado de {age} min", g.source.label()), Style::new().fg(t.warn))
        }
        (Some(g), None) => lbl_owned(format!("geo {}", g.source.label()), t.muted),
        (None, None) => lbl_owned("geo …".into(), t.muted),
    };
    let right = Line::from(vec![geo, lbl("  ·  "), Span::styled(t.name.clone(), Style::new().fg(t.accent)), Span::raw(" ")]);

    let rw = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(rw)]).areas(area);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}

fn lbl_owned(text: String, color: ratatui::style::Color) -> Span<'static> {
    Span::styled(text, Style::new().fg(color))
}

fn help(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let rows = [
        ("Tab / Shift+Tab", "alterna o painel em foco"),
        ("t / T", "próximo / anterior tema (fica lembrado)"),
        ("x", "dispensa o aviso da barra"),
        ("?", "abre/fecha esta ajuda"),
        ("q / Ctrl+C", "sai"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>16}  "), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(*d, Style::new().fg(t.fg)),
            ])
        })
        .collect();
    let w = 60.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(t.accent))
                .title(" atalhos ")
                .style(Style::new().bg(t.bg)),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Config, Paths},
        providers::{
            Snapshot,
            geo::{Geo, GeoSource},
        },
        theme::ThemeRegistry,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn state() -> AppState {
        let paths = Paths { config_dir: "/x".into(), state_dir: "/x".into(), cache_dir: "/x".into() };
        let themes = ThemeRegistry::load(std::path::Path::new("/nao/existe"), true);
        let cfg = Config::parse(crate::config::EXAMPLE).unwrap();
        let mut s = AppState::new(cfg, paths, themes, 0, vec![]);
        s.geo = Snapshot::ok(Geo {
            city: "Rio de Janeiro".into(),
            region: Some("Rio de Janeiro".into()),
            country: Some("Brazil".into()),
            lat: None,
            lon: None,
            timezone: Some("America/Sao_Paulo".into()),
            source: GeoSource::Ip("ipapi.co".into()),
        });
        s.recompute_tz();
        s
    }

    fn render(w: u16, h: u16, s: &AppState) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, s)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn renders_every_layout_without_panicking() {
        let mut s = state();
        for (w, h) in [(200, 50), (120, 40), (80, 24), (60, 12), (20, 5), (1, 1)] {
            let out = render(w, h, &s);
            if w >= 80 {
                assert!(out.contains("Rio de Janeiro · Brazil"), "{w}x{h}:\n{out}");
                assert!(out.contains("Orlando"), "{w}x{h}");
            }
        }
        s.show_help = true;
        assert!(render(120, 40, &s).contains("atalhos"));
    }

    #[test]
    fn big_clock_is_drawn_with_blocks() {
        let out = render(200, 50, &state());
        assert!(out.contains('█'));
        assert!(out.contains("UTC-03:00"));
        assert!(out.contains("Sistema") && out.contains("Worktrees") && out.contains("Claude"));
    }

    #[test]
    fn compact_mode_shows_only_hero() {
        let out = render(120, 24, &state());
        assert!(!out.contains("Worktrees"));
        assert!(out.contains("Rio de Janeiro"));
    }
}
