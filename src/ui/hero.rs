//! Hero: abas de cidade/relógios extras, relógio grande, data e a próxima reunião.
//! Quando a reunião está para começar, o hero inteiro fica vermelho até uma tecla.

use chrono::Timelike;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use super::{bigclock::BigText, panel_block, truncate};
use crate::{
    alert::{Alert, Level},
    app::state::{AppState, Panel},
    locale,
    providers::calendar::not_connected_hint,
};

/// Cores do hero: as do tema, ou tudo em `alert_fg` sobre `alert_bg` no alerta.
struct Palette {
    fg: Color,
    muted: Color,
    accent: Color,
    clock: Color,
    border: Color,
}

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let alert = s.alert();
    let red = alert.level == Level::Imminent;
    let p = if red {
        Palette { fg: t.alert_fg, muted: t.alert_fg, accent: t.alert_fg, clock: t.alert_fg, border: t.alert_bg }
    } else {
        Palette { fg: t.fg, muted: t.muted, accent: t.accent, clock: t.clock, border: t.border }
    };
    let mut block: Block = panel_block(s, Panel::Hero, None);
    if red {
        block = block.style(Style::new().bg(t.alert_bg).fg(t.alert_fg)).border_style(Style::new().fg(t.alert_bg));
    } else if alert.level == Level::Soon {
        block = block.border_style(Style::new().fg(t.warn));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let local = s.local_now();
    let cfg = &s.config.clock;

    // Linha 1: abas "Cidade HH:MM │ Extra HH:MM │ …"
    let tab_style = if red {
        Style::new().fg(t.alert_bg).bg(t.alert_fg).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(p.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)
    };
    let mut tabs = vec![Span::styled(format!(" {} {} ", s.city_label(), locale::short_time(&local, cfg.hour12)), tab_style)];
    for (label, tz) in &s.extra_clocks {
        tabs.push(Span::styled(" │ ", Style::new().fg(p.border)));
        tabs.push(Span::styled(format!("{label} {}", locale::short_time(&s.now.with_timezone(tz), cfg.hour12)), Style::new().fg(p.fg)));
    }
    f.render_widget(Paragraph::new(Line::from(tabs)), Rect { height: 1, ..inner });

    // No alerta, a última linha vira o rodapé com as teclas.
    let footer_h = u16::from(red && inner.height >= 6);
    let body = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1 + footer_h), ..inner };
    let clock_text = locale::big_time(&local, cfg.seconds, cfg.hour12);
    const TEXT_LINES: u16 = 5; // lugar, respiro, respiro, data, reunião
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

    centered(f, y, Line::styled(s.place_line(), Style::new().fg(p.fg).add_modifier(Modifier::BOLD)));
    y += 2;

    let clock_style = Style::new().fg(p.clock).add_modifier(Modifier::BOLD);
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
                    f.render_widget(Paragraph::new(Span::styled(ampm, Style::new().fg(p.muted))), Rect { x: ax, y, width: 2, height: 1 });
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
            Style::new().fg(p.fg),
        ),
    );
    y += 1;
    centered(f, y, meeting_line(s, &alert, &p, body.width as usize));

    if footer_h == 1 {
        let key = |k: &'static str| Span::styled(k, Style::new().fg(t.alert_fg).add_modifier(Modifier::BOLD));
        let lbl = |l: &'static str| Span::styled(l, Style::new().fg(t.alert_fg));
        let mut spans = vec![key("qualquer tecla"), lbl(" parar   "), key("z"), lbl(" soneca ")];
        spans.push(Span::styled(format!("{} min", s.config.calendar.snooze_minutes), Style::new().fg(t.alert_fg)));
        if alert.event.is_some_and(|e| e.link().is_some()) {
            spans.extend([lbl("   "), key("o"), lbl(" abrir a chamada")]);
        }
        let r = Rect { y: inner.y + inner.height - 1, height: 1, ..inner };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

/// "▶ 16:00 Daily · em 4 min", "▶ agora: Daily · começou há 2 min" ou o estado da agenda.
fn meeting_line(s: &AppState, alert: &Alert, p: &Palette, w: usize) -> Line<'static> {
    let t = s.theme();
    let Some(ev) = alert.event else {
        let msg = match (&s.calendar.data, &s.calendar.error) {
            (_, Some(e)) if not_connected_hint(e) => "agenda não conectada · rode `cockpit auth google`".to_string(),
            (None, Some(e)) => format!("agenda: {e}"),
            (None, None) if s.config.calendar.enabled => "lendo a agenda…".into(),
            (None, None) => String::new(),
            (Some(_), _) => format!("nenhuma reunião nas próximas {} h", s.config.calendar.lookahead_hours),
        };
        return Line::styled(truncate(&msg, w), Style::new().fg(p.muted));
    };
    let until = ev.start - s.now;
    let start = locale::short_time(&ev.start.with_timezone(&s.tz), s.config.clock.hour12);
    let (head, tail) = if until.num_seconds() > 0 {
        (format!("▶ {start} "), format!(" · em {}", until_text(until)))
    } else {
        let ago = -until.num_minutes();
        let end = locale::short_time(&ev.end.with_timezone(&s.tz), s.config.clock.hour12);
        let when = if ago < 1 { "começando agora".to_string() } else { format!("começou há {ago} min · até {end}") };
        ("▶ agora: ".to_string(), format!(" · {when}"))
    };
    let style = match alert.level {
        Level::Imminent if s.blink_on() => Style::new().fg(t.alert_bg).bg(t.alert_fg).add_modifier(Modifier::BOLD),
        Level::Imminent => Style::new().fg(t.alert_fg).add_modifier(Modifier::BOLD),
        Level::Soon => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        Level::Calm => Style::new().fg(p.fg),
    };
    let title = truncate(&ev.title, w.saturating_sub(head.chars().count() + tail.chars().count() + 2));
    Line::from(vec![Span::styled(format!("{head}{title}{tail}"), style)])
}

/// "menos de 1 min", "4 min", "1h 05m".
pub(crate) fn until_text(d: chrono::Duration) -> String {
    let m = d.num_minutes();
    match m {
        ..1 => "menos de 1 min".into(),
        1..60 => format!("{m} min"),
        _ => format!("{}h {:02}m", m / 60, m % 60),
    }
}
