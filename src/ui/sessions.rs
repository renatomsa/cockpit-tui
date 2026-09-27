//! Painel das conversas do Claude: abertas (esperando você primeiro) e encerradas há pouco.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{ago, panel_block, system::bar, truncate, worktrees::status_icon};
use crate::{
    app::state::{AppState, Panel},
    locale,
    providers::{
        claude::{Session, Status},
        transcript::TurnStats,
        usage::{Limit, Severity},
    },
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

    // Limites do plano ficam fixos no topo; a lista de conversas rola embaixo.
    let head = limit_lines(s, t, w);
    let head_h = (head.len() as u16).min(inner.height);
    f.render_widget(Paragraph::new(head), Rect { height: head_h, ..inner });
    let inner = Rect { y: inner.y + head_h, height: inner.height - head_h, ..inner };
    if inner.height == 0 {
        return;
    }

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

/// "Sessão 5h ━━━━━━━━━━━━━━━  45%  reinicia 19:50 (em 2h 03m)", uma por limite.
fn limit_lines(s: &AppState, t: &Theme, w: usize) -> Vec<Line<'static>> {
    if !s.config.claude.usage_limits {
        return Vec::new();
    }
    let muted = Style::new().fg(t.muted);
    let Some(d) = &s.usage.data else {
        let msg = match &s.usage.error {
            Some(e) => format!("limites: {e}"),
            None => "limites: consultando…".into(),
        };
        return vec![Line::styled(truncate(&msg, w), muted), Line::raw("")];
    };
    let label_w = d.limits.iter().map(|l| l.label.chars().count()).max().unwrap_or(6) + 2;
    let bar_w = w.saturating_sub(label_w + 6 + 28).clamp(6, 24);
    let local_now = s.local_now();
    let mut out: Vec<Line> = d
        .limits
        .iter()
        .map(|l| {
            let color = limit_color(t, l);
            let (filled, empty) = bar(l.percent, bar_w);
            let reset = l
                .resets_at
                .map(|r| format!("reinicia {}", locale::reset_when(&local_now, &r.with_timezone(&s.tz), s.config.locale)))
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(format!("{:<label_w$}", l.label), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(filled, Style::new().fg(color)),
                Span::styled(empty, Style::new().fg(t.border)),
                Span::styled(format!(" {:>3.0}%  ", l.percent), Style::new().fg(if l.severity == Severity::Normal { t.fg } else { color }).add_modifier(Modifier::BOLD)),
                Span::styled(reset, muted),
            ])
        })
        .collect();
    if let Some(e) = &s.usage.error {
        out.push(Line::styled(truncate(&format!("desatualizado: {e}"), w), Style::new().fg(t.warn)));
    }
    out.push(Line::raw(""));
    out
}

fn limit_color(t: &Theme, l: &Limit) -> ratatui::style::Color {
    match l.severity {
        Severity::Critical => t.alert,
        Severity::Warning => t.warn,
        Severity::Normal if l.percent < 60.0 => t.gauge_low,
        Severity::Normal if l.percent < 85.0 => t.gauge_mid,
        Severity::Normal => t.gauge_high,
    }
}

/// 950 → "950", 12_300 → "12.3k", 666_500 → "667k", 1_400_000 → "1.4M".
pub(crate) fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..99_950 => format!("{:.1}k", n as f64 / 1e3).replace(".0k", "k"),
        99_950..999_500 => format!("{}k", (n + 500) / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6).replace(".0M", "M"),
    }
}

/// "agora há 3m · 3.2k saída · 1.4M processados · contexto 145k · 2 subagentes"
fn turn_line(s: &AppState, t: &Theme, sess: &Session, turn: &TurnStats, w: usize) -> Line<'static> {
    let running = matches!(sess.status, Status::Busy | Status::Shell);
    let muted = Style::new().fg(t.muted);
    let num = Style::new().fg(t.fg);
    let head = match (running, turn.started_at) {
        (true, Some(start)) => format!("⟳ rodando há {} · ", ago((s.now - start).num_seconds())),
        (true, None) => "⟳ rodando · ".into(),
        (false, _) => "última rodada · ".into(),
    };
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(head, if running { Style::new().fg(t.accent) } else { muted }),
        Span::styled(tokens(turn.output), Style::new().fg(t.spark).add_modifier(Modifier::BOLD)),
        Span::styled(" saída · ", muted),
        Span::styled(tokens(turn.total), num),
        Span::styled(" processados · contexto ", muted),
        Span::styled(tokens(turn.context), num),
    ];
    if turn.subagents > 0 {
        spans.push(Span::styled(format!(" · {} subagente{}", turn.subagents, if turn.subagents == 1 { "" } else { "s" }), muted));
    }
    // corta pela direita se não couber
    let mut line = Line::from(spans);
    while line.width() > w && line.spans.len() > 3 {
        line.spans.pop();
    }
    line
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
    let mut third = sess.turn.as_ref().filter(|x| x.calls > 0).map(|turn| turn_line(s, t, sess, turn, w));
    if selected {
        let bg = Style::new().bg(t.border);
        first = first.style(bg);
        second = second.style(bg);
        third = third.map(|l| l.style(bg));
    }
    let mut out = vec![first, second];
    out.extend(third);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(12_300), "12.3k");
        assert_eq!(tokens(145_000), "145k");
        assert_eq!(tokens(666_500), "667k");
        assert_eq!(tokens(999_600), "1M");
        assert_eq!(tokens(1_400_000), "1.4M");
        assert_eq!(tokens(2_000_000), "2M");
    }
}
