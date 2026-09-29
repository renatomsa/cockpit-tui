//! Aba Claude Desktop: sessões de Code e tarefas do modo agente (Cowork) do app do
//! Windows, com o estado ao vivo e os tokens da rodada quando a transcrição existe.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{ago, sessions::turn_line, truncate, worktrees::status_icon};
use crate::{
    app::state::{AppState, Panel},
    providers::desktop::{DesktopSession, Kind},
};

pub fn body(f: &mut Frame, inner: Rect, s: &AppState) {
    let t = s.theme();
    if inner.height == 0 || inner.width < 20 {
        return;
    }
    let w = inner.width as usize;
    let muted = Style::new().fg(t.muted);
    let Some(d) = &s.desktop.data else {
        let msg = match &s.desktop.error {
            Some(e) => e.clone(),
            None if !crate::platform::is_wsl() => "o Claude Desktop só é lido no WSL (pelo disco do Windows)".into(),
            None => "lendo o Claude Desktop…".into(),
        };
        f.render_widget(Paragraph::new(Line::styled(msg, muted)), inner);
        return;
    };
    let mut lines = vec![
        Line::styled(truncate("sessões de Code e do modo agente do app, últimos 7 dias · as conversas de chat ficam no claude.ai", w), muted),
        Line::raw(""),
    ];
    if d.sessions.is_empty() {
        lines.push(Line::styled("nenhuma sessão recente", muted));
    }
    let focused = s.active_panel() == Panel::Sessions;
    let mut sel_line = 0;
    for (i, sess) in d.sessions.iter().enumerate() {
        let selected = focused && i == s.desktop_sel;
        if selected {
            sel_line = lines.len();
        }
        lines.extend(rows(s, sess, w, selected));
    }
    let h = inner.height as usize;
    let offset = (sel_line + 2).saturating_sub(h);
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn rows(s: &AppState, sess: &DesktopSession, w: usize, selected: bool) -> Vec<Line<'static>> {
    let t = s.theme();
    let muted = Style::new().fg(t.muted);
    let icon = match sess.status {
        Some(st) => status_icon(t, st),
        None => Span::styled("·", muted),
    };
    let kind = match sess.kind {
        Kind::Code => "Code",
        Kind::Cowork => "Cowork",
    };
    // Pasta do Windows: só o último pedaço (C:\Users\…\projeto → projeto).
    let place = sess.cwd.as_deref().and_then(|c| c.rsplit(['\\', '/']).find(|p| !p.is_empty())).map(|p| format!(" · {p}")).unwrap_or_default();
    let right = format!("{kind}{place} · há {}", ago((s.now - sess.last_activity).num_seconds()));
    let right = truncate(&right, w / 2);
    let title_style = match sess.status {
        Some(crate::providers::claude::Status::Waiting) => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        Some(_) => Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        None => Style::new().fg(t.fg),
    };
    let title = truncate(&sess.title, w.saturating_sub(right.chars().count() + 4));
    let gap = w.saturating_sub(2 + title.chars().count() + right.chars().count());
    let mut first = Line::from(vec![
        icon,
        Span::raw(" "),
        Span::styled(title, title_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(right, muted),
    ]);
    let mut second = match (&sess.turn, &sess.waiting_for) {
        (_, Some(why)) if sess.status == Some(crate::providers::claude::Status::Waiting) => {
            Line::from(vec![Span::raw("  "), Span::styled(format!("esperando: {why} (responda no app)"), Style::new().fg(t.warn))])
        }
        (Some(turn), _) => turn_line(s, t, sess.status, turn, w),
        (None, _) => Line::from(vec![Span::raw("  "), Span::styled(sess.model.clone().unwrap_or_default(), muted)]),
    };
    if selected {
        first = first.style(Style::new().bg(t.border));
        second = second.style(Style::new().bg(t.border));
    }
    vec![first, second]
}
