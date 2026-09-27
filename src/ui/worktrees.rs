//! Painel de worktrees: repositórios como cabeçalho e, embaixo, cada worktree com branch,
//! alterações, ↑↓ do upstream, idade do último commit, tamanho e as sessões do Claude nela.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{ago, panel_block, pad, system::bytes, truncate};
use crate::{
    app::state::{AppState, Panel},
    config::tilde,
    providers::{
        claude::Status,
        git::{Repo, Worktree},
    },
    theme::Theme,
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let block = panel_block(s, Panel::Worktrees, Some("Worktrees"));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }
    let w = inner.width as usize;

    let Some(data) = &s.worktrees.data else {
        let msg = match &s.worktrees.error {
            Some(e) => format!("erro: {e}"),
            None => format!("procurando repositórios em {}…", s.config.worktrees.workspaces.join(", ")),
        };
        f.render_widget(Paragraph::new(Line::styled(msg, Style::new().fg(t.muted))), inner);
        return;
    };
    if data.repos.is_empty() {
        let msg = format!(
            "nenhum repositório git em {} (profundidade {}). Ajuste [worktrees].workspaces no config.",
            s.config.worktrees.workspaces.join(", "),
            s.config.worktrees.depth
        );
        f.render_widget(Paragraph::new(Line::styled(msg, Style::new().fg(t.muted))), inner);
        return;
    }

    let mut head: Vec<Line> = vec![summary_line(s, t, data.worktree_count())];
    if s.filter_editing || !s.filter.is_empty() {
        let cursor = if s.filter_editing { "▏" } else { "" };
        head.push(Line::from(vec![
            Span::styled("/ ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{}{cursor}", s.filter), Style::new().fg(t.fg)),
            Span::styled(format!("   {} de {}", s.visible_worktrees().len(), data.worktree_count()), Style::new().fg(t.muted)),
            Span::styled(if s.filter_editing { "   Enter aplica · Esc limpa" } else { "   Esc limpa" }, Style::new().fg(t.muted)),
        ]));
    }
    head.push(Line::raw(""));
    let head_h = (head.len() as u16).min(inner.height);
    f.render_widget(Paragraph::new(head), Rect { height: head_h, ..inner });

    let visible = s.visible_worktrees();
    let cols = Columns::fit(s, &visible, w);
    let mut lines: Vec<Line> = Vec::new();
    let mut sel_line = 0;
    let mut last_repo = None;
    for (i, (ri, wi)) in visible.iter().enumerate() {
        let repo = &data.repos[*ri];
        if last_repo != Some(*ri) {
            if last_repo.is_some() {
                lines.push(Line::raw(""));
            }
            lines.push(repo_line(t, repo, w));
            last_repo = Some(*ri);
        }
        let is_last = visible.get(i + 1).is_none_or(|(r, _)| r != ri);
        let selected = i == s.wt_sel;
        if selected {
            sel_line = lines.len();
        }
        lines.push(worktree_line(s, t, &repo.worktrees[*wi], &cols, is_last, selected));
    }
    if visible.is_empty() {
        lines.push(Line::styled("nada casa com o filtro", Style::new().fg(t.muted)));
    }

    let list = Rect { y: inner.y + head_h, height: inner.height - head_h, ..inner };
    let h = list.height as usize;
    // Rola só o necessário para a seleção caber, deixando o cabeçalho do repo à vista.
    let offset = if sel_line < h { 0 } else { sel_line + 2 - h };
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), list);
}

/// "12 worktrees  ◆ 1 esperando você  ⠿ 2 trabalhando  ● 3 paradas  ·  892G livres"
fn summary_line(s: &AppState, t: &Theme, total: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{total} worktrees"), Style::new().fg(t.fg).add_modifier(Modifier::BOLD))];
    if let Some(d) = &s.sessions.data {
        let count = |st| d.count(st);
        let idle = count(Status::Idle);
        let parts = [
            (count(Status::Waiting), "◆", "esperando você", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)),
            (count(Status::Busy) + count(Status::Shell), "⠿", "trabalhando", Style::new().fg(t.accent)),
            (idle, "●", if idle == 1 { "parada" } else { "paradas" }, Style::new().fg(t.muted)),
        ];
        for (n, icon, label, style) in parts {
            if n > 0 {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(format!("{icon} {n} {label}"), style));
            }
        }
    }
    if let Some(root) = s.system.data.as_ref().and_then(|d| d.disks.iter().find(|d| d.mount == "/")) {
        spans.push(Span::styled(format!("   ·   {} livres", bytes(root.total - root.used)), Style::new().fg(t.muted)));
    }
    Line::from(spans)
}

fn repo_line(t: &Theme, r: &Repo, w: usize) -> Line<'static> {
    let name = format!("{}  ", r.name);
    let path = tilde(&r.path);
    let n = r.worktrees.len();
    let total: u64 = r.worktrees.iter().filter_map(|w| w.size).sum();
    let mut right = format!("{n} worktree{}", if n == 1 { "" } else { "s" });
    if total > 0 {
        right.push_str(&format!(" · {}", bytes(total)));
    }
    let used = name.chars().count() + path.chars().count() + right.chars().count() + 2;
    let fill = w.saturating_sub(used);
    Line::from(vec![
        Span::styled(name, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(path, Style::new().fg(t.muted)),
        Span::styled(format!(" {} ", "─".repeat(fill)), Style::new().fg(t.border)),
        Span::styled(right, Style::new().fg(t.muted)),
    ])
}

/// Larguras das colunas, cortando primeiro a branch e depois o nome quando falta espaço.
struct Columns {
    name: usize,
    branch: usize,
}

const FIXED: usize = 4 + 2 + 6 + 10 + 5 + 7 + 6; // árvore, gaps, ✎, ↑↓, idade, tamanho, sessões

impl Columns {
    fn fit(s: &AppState, visible: &[(usize, usize)], w: usize) -> Self {
        let data = s.worktrees.data.as_ref();
        let wt = |&(r, i): &(usize, usize)| data.map(|d| &d.repos[r].worktrees[i]);
        let mut name = visible.iter().filter_map(wt).map(|w| label(w).chars().count()).max().unwrap_or(8).clamp(8, 36);
        let mut branch =
            visible.iter().filter_map(wt).map(|w| branch_text(w).chars().count()).max().unwrap_or(6).clamp(6, 48);
        let room = w.saturating_sub(FIXED);
        while name + branch + 2 > room && branch > 12 {
            branch -= 1;
        }
        while name + branch + 2 > room && name > 10 {
            name -= 1;
        }
        Self { name, branch }
    }
}

fn label(w: &Worktree) -> String {
    if w.is_main { format!("{} (principal)", w.name) } else { w.name.clone() }
}

fn branch_text(w: &Worktree) -> String {
    match (&w.branch, w.detached) {
        (Some(b), _) => b.clone(),
        (None, true) => "(HEAD solta)".into(),
        (None, false) => "—".into(),
    }
}

fn worktree_line(s: &AppState, t: &Theme, w: &Worktree, c: &Columns, is_last: bool, selected: bool) -> Line<'static> {
    let focused = s.focus == Panel::Worktrees;
    let tree = if is_last { "└ " } else { "├ " };
    let pointer = if selected && focused { "▸ " } else { "  " };
    let name_style = if w.prunable {
        Style::new().fg(t.muted).add_modifier(Modifier::CROSSED_OUT)
    } else if w.is_main {
        Style::new().fg(t.fg)
    } else {
        Style::new().fg(t.fg).add_modifier(Modifier::BOLD)
    };

    let dirty = match w.dirty {
        Some(0) => Span::styled(pad("✓", 6), Style::new().fg(t.ok)),
        Some(n) => Span::styled(pad(&format!("✎ {n}"), 6), Style::new().fg(t.warn)),
        None if w.prunable => Span::styled(pad("sumiu", 6), Style::new().fg(t.alert)),
        None => Span::styled(pad("·", 6), Style::new().fg(t.muted)),
    };
    let mut ab = String::new();
    if let Some(a) = w.ahead.filter(|a| *a > 0) {
        ab.push_str(&format!("↑{a} "));
    }
    if let Some(b) = w.behind.filter(|b| *b > 0) {
        ab.push_str(&format!("↓{b}"));
    }
    let age = w.last_commit.map(|ts| ago(s.now.timestamp() - ts)).unwrap_or_default();
    let size = match w.size {
        Some(b) => bytes(b),
        None if s.config.worktrees.sizes && !w.prunable => "…".into(),
        None => String::new(),
    };

    let mut spans = vec![
        Span::styled(pointer, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(tree, Style::new().fg(t.border)),
        Span::styled(pad(&truncate(&label(w), c.name), c.name + 2), name_style),
        Span::styled(pad(&truncate(&branch_text(w), c.branch), c.branch + 2), Style::new().fg(t.highlight)),
        dirty,
        Span::styled(pad(&ab, 10), Style::new().fg(t.accent)),
        Span::styled(format!("{age:>4} "), Style::new().fg(t.muted)),
        Span::styled(format!("{size:>6} "), Style::new().fg(t.muted)),
    ];
    // Uma marca por sessão do Claude aberta nesta worktree.
    if let (Some(idx), Some(d)) = (s.wt_sessions.get(&w.path), &s.sessions.data) {
        for &i in idx.iter().take(5) {
            let Some(sess) = d.live.get(i) else { continue };
            spans.push(status_icon(t, sess.status));
        }
    }
    let mut line = Line::from(spans);
    if selected && focused {
        line = line.style(Style::new().bg(t.border));
    }
    line
}

/// ◆ esperando você · ⠿ trabalhando · $ rodando shell · ● parada · ○ encerrada
pub(crate) fn status_icon(t: &Theme, st: Status) -> Span<'static> {
    match st {
        Status::Waiting => Span::styled("◆", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)),
        Status::Busy => Span::styled("⠿", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Status::Shell => Span::styled("$", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Status::Idle => Span::styled("●", Style::new().fg(t.muted)),
        Status::Ended => Span::styled("○", Style::new().fg(t.muted)),
    }
}
