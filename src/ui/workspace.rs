//! Painel que junta Worktrees e as conversas do Claude em duas abas (← → troca).
//! O título mostra as duas abas com contagem; à direita, o resumo dos limites de uso,
//! para eles continuarem à vista na aba Worktrees.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{panel_block, sessions, worktrees};
use crate::{
    app::state::{AppState, Panel, WtTab},
    providers::usage::Severity,
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let focused = s.focus == Panel::Worktrees;
    let waiting = s.waiting_count();

    let tab = |label: String, active: bool| {
        let style = match (active, focused) {
            (true, true) => Style::new().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            (true, false) => Style::new().fg(t.fg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            (false, _) => Style::new().fg(t.muted),
        };
        Span::styled(label, style)
    };
    let n_wt = s.worktrees.data.as_ref().map_or(0, |d| d.worktree_count());
    let n_claude = s.sessions.data.as_ref().map_or(0, |d| d.live.len());
    let mut title = vec![
        Span::raw(" "),
        tab(format!("Worktrees {n_wt}"), s.wt_tab == WtTab::Worktrees),
        Span::styled("  │  ", Style::new().fg(t.border)),
        tab(format!("Claude {n_claude}"), s.wt_tab == WtTab::Claude),
    ];
    if waiting > 0 {
        title.push(Span::styled(format!(" ◆ {waiting} esperando você"), Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
    }
    title.push(Span::raw(" "));

    let mut block = panel_block(s, Panel::Worktrees, None).title(Line::from(title));
    if waiting > 0 {
        block = block.border_style(Style::new().fg(t.warn));
    }
    // Resumo dos limites do plano: fica à vista nas duas abas.
    let mut right: Vec<Span> = Vec::new();
    if let Some(u) = &s.usage.data {
        for l in u.limits.iter().take(2) {
            let color = match l.severity {
                Severity::Critical => t.alert,
                Severity::Warning => t.warn,
                Severity::Normal => t.muted,
            };
            let short = if l.kind == "session" { "5h".to_string() } else { l.label.to_lowercase() };
            if !right.is_empty() {
                right.push(Span::styled(" · ", Style::new().fg(t.muted)));
            }
            right.push(Span::styled(format!("{short} {:.0}%", l.percent), Style::new().fg(color)));
        }
    }
    if focused {
        if !right.is_empty() {
            right.push(Span::styled("   ", Style::new()));
        }
        right.push(Span::styled("←→ aba", Style::new().fg(t.muted)));
    }
    if !right.is_empty() {
        right.insert(0, Span::raw(" "));
        right.push(Span::raw(" "));
        block = block.title_top(Line::from(right).right_aligned());
    }

    let inner = block.inner(area);
    f.render_widget(block, area);
    match s.wt_tab {
        WtTab::Worktrees => worktrees::body(f, inner, s),
        WtTab::Claude => sessions::body(f, inner, s),
    }
}
