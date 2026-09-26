//! Hero: abas de cidade/relógios extras, relógio grande, data e a próxima agenda.

use chrono::Timelike;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use super::{bigclock::BigText, panel_block};
use crate::{
    app::state::{AppState, Panel},
    locale,
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let block: Block = panel_block(s, Panel::Hero, None);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let local = s.local_now();
    let cfg = &s.config.clock;

    // Linha 1: abas "Cidade HH:MM │ Extra HH:MM │ …"
    let mut tabs = vec![Span::styled(
        format!(" {} {} ", s.city_label(), locale::short_time(&local, cfg.hour12)),
        Style::new().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD),
    )];
    for (label, tz) in &s.extra_clocks {
        tabs.push(Span::styled(" │ ", Style::new().fg(t.border)));
        tabs.push(Span::styled(
            format!("{label} {}", locale::short_time(&s.now.with_timezone(tz), cfg.hour12)),
            Style::new().fg(t.fg),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(tabs)), Rect { height: 1, ..inner });

    let body = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let clock_text = locale::big_time(&local, cfg.seconds, cfg.hour12);
    const TEXT_LINES: u16 = 5; // lugar, respiro, respiro, data, agenda
    let scale = BigText::fit(&clock_text, body.width.saturating_sub(2), body.height.saturating_sub(TEXT_LINES), 3);
    let clock_h = scale.map_or(1, |sc| BigText::size(&clock_text, sc).1);

    let total = (TEXT_LINES + clock_h).min(body.height);
    let mut y = body.y + body.height.saturating_sub(total) / 2;
    let line_at = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let bottom = body.y + body.height;

    let centered = |f: &mut Frame, y: u16, line: Line| {
        if y < bottom {
            f.render_widget(Paragraph::new(line).alignment(Alignment::Center), line_at(y));
        }
    };

    centered(f, y, Line::styled(s.place_line(), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)));
    y += 2;

    let clock_style = Style::new().fg(t.clock).add_modifier(Modifier::BOLD);
    match scale {
        Some(sc) => {
            let (w, h) = BigText::size(&clock_text, sc);
            let x = body.x + body.width.saturating_sub(w) / 2;
            let h = h.min(bottom.saturating_sub(y));
            f.render_widget(BigText { text: &clock_text, scale: sc, style: clock_style }, Rect { x, y, width: w, height: h });
            if cfg.hour12 {
                let ampm = if local.hour12().0 { "PM" } else { "AM" };
                let ax = x + w + 1;
                if ax + 2 <= body.x + body.width {
                    f.render_widget(Paragraph::new(Span::styled(ampm, Style::new().fg(t.muted))), Rect { x: ax, y, width: 2, height: 1 });
                }
            }
        }
        None => centered(f, y, Line::styled(clock_text.clone(), clock_style)),
    }
    y += clock_h + 1;

    centered(
        f,
        y,
        Line::styled(
            format!("{} · {}", locale::long_date(&local, s.config.locale), locale::utc_offset(&local)),
            Style::new().fg(t.fg),
        ),
    );
    y += 1;

    // A próxima reunião entra aqui na F3 (Google Calendar).
    centered(
        f,
        y,
        Line::styled("agenda ainda não conectada · Google Calendar chega na F3", Style::new().fg(t.muted)),
    );
}
