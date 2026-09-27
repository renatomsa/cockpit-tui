//! Painel das conversas do Claude: abertas (esperando você primeiro) e encerradas há pouco.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{ago, panel_block, truncate, worktrees::status_icon};
use crate::{
    app::state::{AppState, Panel},
    providers::claude::{Session, Status},
    theme::Theme,
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let waiting = s.waiting_count();
    let mut block = panel_block(s, Panel::Sessions, Some("Claude"));
    if waiting > 0 {
        block = block
            .border_style(Style::new().fg(t.warn))
            .title(Span::styled(format!(" ◆ {waiting} esperando você "), Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }
    let w = inner.width as usize;

    let Some(d) = &s.sessions.data else {
        let msg = s.sessions.error.clone().unwrap_or_else(|| "lendo sessões…".into());
        f.render_widget(Paragraph::new(Line::styled(msg, Style::new().fg(t.muted))), inner);
        return;
    };

    let focused = s.focus == Panel::Sessions;
    let mut lines: Vec<Line> = Vec::new();
    let mut sel_line = 0;
    if d.live.is_empty() {
        lines.push(Line::styled("nenhuma conversa aberta", Style::new().fg(t.muted)));
    }
    for (i, sess) in d.live.iter().enumerate() {
        let selected = focused && i == s.sess_sel;
        if selected {
            sel_line = lines.len();
        }
        lines.extend(live_lines(s, t, sess, w, selected));
    }
    if !d.recent.is_empty() {
        lines.push(Line::raw(""));
        let head = "encerradas há pouco · r retoma ";
        lines.push(Line::from(vec![
            Span::styled(head, Style::new().fg(t.muted)),
            Span::styled("─".repeat(w.saturating_sub(head.chars().count())), Style::new().fg(t.border)),
        ]));
        for (j, sess) in d.recent.iter().enumerate() {
            let selected = focused && d.live.len() + j == s.sess_sel;
            if selected {
                sel_line = lines.len();
            }
            lines.push(recent_line(s, t, sess, w, selected));
        }
    }

    let h = inner.height as usize;
    let offset = (sel_line + 2).saturating_sub(h);
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn when(s: &AppState, sess: &Session) -> String {
    sess.updated_at.map(|u| format!("há {}", ago((s.now - u).num_seconds()))).unwrap_or_default()
}

/// "lugar · há 8m [· bg]" em no máximo `max` colunas: corta o lugar, nunca a idade.
fn right_text(s: &AppState, sess: &Session, max: usize) -> String {
    let mut tail = format!(" · {}", when(s, sess));
    if sess.background {
        tail.push_str(" · bg");
    }
    let place = truncate(&s.session_place(sess), max.saturating_sub(tail.chars().count()).max(4));
    format!("{place}{tail}")
}

/// Duas linhas: status + título (e onde está, à direita); embaixo, o resumo.
fn live_lines(s: &AppState, t: &Theme, sess: &Session, w: usize, selected: bool) -> Vec<Line<'static>> {
    let right = right_text(s, sess, w / 2);
    let right_w = right.chars().count();
    let title_w = w.saturating_sub(right_w + 4);
    let title_style = match sess.status {
        Status::Waiting => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        _ => Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
    };
    let title = truncate(&sess.title, title_w);
    let gap = w.saturating_sub(2 + title.chars().count() + right_w);
    let mut first = Line::from(vec![
        status_icon(t, sess.status),
        Span::raw(" "),
        Span::styled(title, title_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(truncate(&right, right_w), Style::new().fg(t.muted)),
    ]);

    let detail = match (sess.status, &sess.waiting_for, &sess.summary) {
        (Status::Waiting, Some(why), _) => Span::styled(format!("esperando: {why}"), Style::new().fg(t.warn)),
        (Status::Waiting, None, None) => Span::styled("esperando sua resposta", Style::new().fg(t.warn)),
        (_, _, Some(sum)) => Span::styled(sum.clone(), Style::new().fg(t.muted)),
        _ => Span::styled(crate::config::tilde(&sess.cwd), Style::new().fg(t.muted)),
    };
    let detail = Span::styled(truncate(&detail.content, w.saturating_sub(2)), detail.style);
    let mut second = Line::from(vec![Span::raw("  "), detail]);
    if selected {
        first = first.style(Style::new().bg(t.border));
        second = second.style(Style::new().bg(t.border));
    }
    vec![first, second]
}

fn recent_line(s: &AppState, t: &Theme, sess: &Session, w: usize, selected: bool) -> Line<'static> {
    let right = right_text(s, sess, w / 2);
    let right_w = right.chars().count();
    let title = truncate(&sess.title, w.saturating_sub(right_w + 4));
    let gap = w.saturating_sub(2 + title.chars().count() + right_w);
    let mut line = Line::from(vec![
        status_icon(t, Status::Ended),
        Span::raw(" "),
        Span::styled(title, Style::new().fg(t.muted)),
        Span::raw(" ".repeat(gap)),
        Span::styled(truncate(&right, right_w), Style::new().fg(t.muted)),
    ]);
    if selected {
        line = line.style(Style::new().bg(t.border));
    }
    line
}
