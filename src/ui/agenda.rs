//! Painel Agenda: as reuniões das próximas horas, agrupadas por dia, com `o` para abrir a
//! chamada e `y` para copiar o link.

use chrono::{Datelike, Duration};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{hero::until_text, pad, panel_block, truncate};
use crate::{
    app::state::{AppState, Panel},
    config::Locale,
    locale,
    providers::calendar::{Event, not_connected_hint},
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let focused = s.focus == Panel::Agenda;
    let today = s.local_now().date_naive();
    let events = s.events();
    let today_n = events.iter().filter(|e| e.start.with_timezone(&s.tz).date_naive() == today && e.end > s.now).count();
    let title = match (&s.calendar.data, s.demo_agenda) {
        (Some(_), true) => "Agenda · demonstração".to_string(),
        (Some(_), false) if today_n > 0 => format!("Agenda · {today_n} hoje"),
        _ => "Agenda".to_string(),
    };
    let mut block = panel_block(s, Panel::Agenda, Some(&title));
    if focused && !events.is_empty() {
        block = block.title_top(Line::styled(" o abrir · y copiar link ", Style::new().fg(t.muted)).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }
    let w = inner.width as usize;
    let muted = Style::new().fg(t.muted);

    if s.calendar.data.is_none() {
        let lines = match &s.calendar.error {
            Some(e) if not_connected_hint(e) => vec![
                Line::styled("Google Calendar não conectado.", Style::new().fg(t.fg)),
                Line::styled(truncate("1. crie o cliente OAuth \"App para computador\" (README, seção Google Calendar)", w), muted),
                Line::styled(truncate(&format!("2. salve o JSON em {}", s.config.calendar.client_secret), w), muted),
                Line::styled("3. rode `cockpit auth google`   (para só ver o alerta: `cockpit --demo-agenda`)", muted),
            ],
            Some(e) => vec![Line::styled(truncate(&format!("agenda: {e}"), w), Style::new().fg(t.warn))],
            None if !s.config.calendar.enabled => vec![Line::styled("agenda desligada ([calendar].enabled = false)", muted)],
            None => vec![Line::styled("lendo a agenda…", muted)],
        };
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }
    if events.is_empty() {
        let msg = format!("nenhuma reunião nas próximas {} h", s.config.calendar.lookahead_hours);
        f.render_widget(Paragraph::new(Line::styled(msg, muted)), inner);
        return;
    }

    let mut lines = Vec::new();
    let mut last_day = None;
    for (i, ev) in events.iter().enumerate() {
        let day = ev.start.with_timezone(&s.tz).date_naive();
        let label = if last_day == Some(day) { String::new() } else { day_label(day, today, s.config.locale) };
        last_day = Some(day);
        lines.push(row(s, ev, &label, w, focused && i == s.agenda_sel));
    }
    let h = inner.height as usize;
    let sel = if focused { s.agenda_sel } else { 0 };
    let offset = (sel + 1).saturating_sub(h);
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate, loc: Locale) -> String {
    const DIAS: [&str; 7] = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];
    match (day - today).num_days() {
        0 => "hoje".into(),
        1 => if loc == Locale::En { "tomorrow".into() } else { "amanhã".into() },
        _ => format!("{} {:02}", DIAS[day.weekday().num_days_from_monday() as usize], day.day()),
    }
}

fn provider(url: &str) -> &'static str {
    if url.contains("meet.google.com") {
        "Meet"
    } else if url.contains("zoom.us") {
        "Zoom"
    } else if url.contains("teams.microsoft") || url.contains("teams.live") {
        "Teams"
    } else {
        "link"
    }
}

fn row(s: &AppState, ev: &Event, day: &str, w: usize, selected: bool) -> Line<'static> {
    let t = s.theme();
    let h12 = s.config.clock.hour12;
    let time = if ev.all_day {
        "dia todo".to_string()
    } else {
        format!(
            "{}–{}",
            locale::short_time(&ev.start.with_timezone(&s.tz), h12),
            locale::short_time(&ev.end.with_timezone(&s.tz), h12)
        )
    };
    let now = s.now;
    let (rel, rel_style) = if ev.all_day {
        (String::new(), Style::new().fg(t.muted))
    } else if ev.start <= now && ev.end > now {
        (format!("agora · até {}", locale::short_time(&ev.end.with_timezone(&s.tz), h12)), Style::new().fg(t.accent).add_modifier(Modifier::BOLD))
    } else if ev.start - now <= Duration::hours(12) {
        let soon = ev.start - now <= Duration::minutes(s.config.calendar.soon_minutes);
        (format!("em {}", until_text(ev.start - now)), if soon { Style::new().fg(t.warn).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.muted) })
    } else {
        (String::new(), Style::new().fg(t.muted))
    };
    let link = ev.meet.as_deref().map(provider).unwrap_or("");
    let time_w = 13;
    let right_w = 22;
    let title_w = w.saturating_sub(2 + 8 + time_w + right_w + 6);
    let title_style = if ev.declined {
        Style::new().fg(t.muted).add_modifier(Modifier::CROSSED_OUT)
    } else if ev.all_day {
        Style::new().fg(t.muted)
    } else {
        Style::new().fg(t.fg).add_modifier(Modifier::BOLD)
    };
    let mut line = Line::from(vec![
        Span::styled(if selected { "▸ " } else { "  " }, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(pad(day, 8), Style::new().fg(t.accent)),
        Span::styled(pad(&time, time_w), Style::new().fg(t.fg)),
        Span::styled(pad(&truncate(&ev.title, title_w), title_w + 1), title_style),
        Span::styled(format!("{:>w$} ", truncate(&rel, right_w), w = right_w), rel_style),
        Span::styled(pad(link, 5), Style::new().fg(t.highlight)),
    ]);
    if selected {
        line = line.style(Style::new().bg(t.border));
    }
    line
}
