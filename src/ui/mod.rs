//! Desenho. Só lê `AppState`; nunca faz I/O nem muda estado.

mod agenda;
mod bigclock;
mod desktop;
mod docker;
pub(crate) mod hero;
pub mod layout;
mod legend;
pub(crate) mod sessions;
mod system;
mod sysboxes;
mod workspace;
mod worktrees;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};

use crate::app::state::{AppState, Panel};

pub fn draw(f: &mut Frame, s: &AppState) {
    let t = s.theme();
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), area);

    let a = layout::compute(area, &s.config.layout);
    hero::draw(f, a.hero, s);
    if let Some(r) = a.system {
        system::draw(f, r, s);
    }
    if let Some(r) = a.docker {
        docker::draw(f, r, s);
    }
    if let Some(r) = a.worktrees {
        workspace::draw(f, r, s);
    }
    if let Some(r) = a.agenda {
        agenda::draw(f, r, s);
    }

    statusbar(f, a.status, s);
    if s.show_help {
        help(f, area, s);
    }
    if s.show_legend {
        legend::draw(f, area, s);
    }
    if let Some(p) = &s.status_pick {
        status_dialog(f, area, s, p);
    }
    if let Some(c) = &s.confirm {
        confirm_dialog(f, area, s, c);
    }
}

/// Seletor de status da worktree (tecla `s`).
fn status_dialog(f: &mut Frame, area: Rect, s: &AppState, p: &crate::app::state::StatusPick) {
    use crate::wt_status::CHOICES;
    let t = s.theme();
    let current = s.wt_status.get(&p.path).copied();
    let mut lines: Vec<Line> = CHOICES
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let selected = i == p.sel;
            let (num, label, style) = match c {
                Some(st) => (format!("{}", i + 1), st.label(), worktrees::status_style(t, *st)),
                None => ("0".into(), "sem status", Style::new().fg(t.muted)),
            };
            let mut spans = vec![
                Span::styled(if selected { "▸ " } else { "  " }, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{num}  "), Style::new().fg(t.muted)),
                Span::styled(label, style),
            ];
            if *c == current {
                spans.push(Span::styled("  (atual)", Style::new().fg(t.muted)));
            }
            let line = Line::from(spans);
            if selected { line.style(Style::new().bg(t.border)) } else { line }
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" grava · ", Style::new().fg(t.muted)),
        Span::styled("1-5", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" direto · ", Style::new().fg(t.muted)),
        Span::styled("0", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" limpa · ", Style::new().fg(t.muted)),
        Span::styled("Esc", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" cancela", Style::new().fg(t.muted)),
    ]));
    let title = format!(" status · {} ", truncate(&p.name, 40));
    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(20).max(title.chars().count());
    let w = (content_w as u16 + 4).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(t.accent))
                .title(Span::styled(title, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
                .padding(Padding::horizontal(1))
                .style(Style::new().bg(t.bg)),
        ),
        r,
    );
}

/// Textos dos dois avisos de exclusão de worktree.
fn remove_worktree_lines(s: &AppState, r: &crate::app::state::RemoveWt) -> (String, Vec<Line<'static>>) {
    let t = s.theme();
    let fg = Style::new().fg(t.fg);
    let muted = Style::new().fg(t.muted);
    let bold = Style::new().fg(t.alert).add_modifier(Modifier::BOLD);
    let path = crate::config::tilde(&r.path);
    let branch = r.branch.clone().unwrap_or_else(|| "(HEAD solta)".into());
    if r.step == 1 {
        let mut lines = vec![
            Line::from(vec![Span::styled("Excluir a worktree ", fg), Span::styled(r.name.clone(), bold), Span::styled("?", fg)]),
            Line::styled(format!("pasta   {path}"), muted),
            Line::styled(format!("branch  {branch}  (continua existindo)"), muted),
        ];
        if r.prunable {
            lines.push(Line::styled("a pasta já sumiu do disco: só limpa a lista (git worktree prune)", muted));
        }
        return (" excluir worktree ".into(), lines);
    }
    let mut lines = vec![Line::from(vec![Span::styled("A pasta ", fg), Span::styled(path.clone(), bold), Span::styled(" será apagada do disco.", fg)])];
    if r.dirty > 0 {
        lines.push(Line::styled(
            format!("{} arquivo{} alterado{} ou novo{}, sem commit, será{} PERDIDO{}.", r.dirty, pl(r.dirty), pl(r.dirty), pl(r.dirty), if r.dirty == 1 { "" } else { "ão" }, if r.dirty == 1 { "" } else { "S" }),
            bold,
        ));
    } else {
        lines.push(Line::styled("Não há alterações pendentes: tudo já está em commits.", fg));
    }
    if r.sessions > 0 {
        lines.push(Line::styled(
            format!("{} conversa{} do Claude aberta{} nela vai{} ficar sem pasta.", r.sessions, pl(r.sessions as u32), pl(r.sessions as u32), if r.sessions == 1 { "" } else { "ão" }),
            Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::styled(format!("A branch {branch} não é apagada (git branch -D {branch} remove)."), muted));
    lines.push(Line::styled("Não dá para desfazer.", bold));
    (" ⚠ exclusão definitiva ".into(), lines)
}

fn pl(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn confirm_dialog(f: &mut Frame, area: Rect, s: &AppState, c: &crate::app::state::Confirm) {
    use crate::app::state::Confirm;
    let t = s.theme();
    let (title, mut lines) = match c {
        Confirm::Kill(k) => (
            " encerrar processo ".to_string(),
            vec![Line::from(vec![
                Span::styled("Encerrar ", Style::new().fg(t.fg)),
                Span::styled(k.name.clone(), Style::new().fg(t.alert).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" (pid {}) com SIGTERM?", k.pid), Style::new().fg(t.fg)),
            ])],
        ),
        Confirm::Docker { title, lines, .. } => (
            format!(" {title} "),
            lines.iter().map(|l| Line::styled(l.clone(), Style::new().fg(t.fg))).collect(),
        ),
        Confirm::RemoveWorktree(r) => remove_worktree_lines(s, r),
    };
    let (key, rest) = match c {
        Confirm::RemoveWorktree(r) if r.step == 1 => ("y", " continua · qualquer outra tecla cancela"),
        Confirm::RemoveWorktree(_) => ("Enter", " exclui de vez · qualquer outra tecla cancela"),
        _ => ("y", " confirma · qualquer outra tecla cancela"),
    };
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(key, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(rest, Style::new().fg(t.muted)),
    ]));
    // Pergunta curta fica centralizada; avisos com detalhes, alinhados à esquerda.
    let align = if matches!(c, Confirm::Kill(_)) { Alignment::Center } else { Alignment::Left };
    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(20).max(title.chars().count());
    let w = (content_w as u16 + 4).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).alignment(align).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(t.alert))
                .title(Span::styled(title, Style::new().fg(t.alert).add_modifier(Modifier::BOLD)))
                .style(Style::new().bg(t.bg)),
        ),
        r,
    );
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

fn statusbar(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let key = |k: &'static str| Span::styled(k, Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    let lbl = |l: &'static str| Span::styled(l, Style::new().fg(t.muted));

    let geo = match (&s.geo.data, &s.geo.error) {
        (None, Some(_)) => Span::styled("geo offline", Style::new().fg(t.warn)),
        (Some(g), Some(_)) => {
            let age = s.geo.updated_at.map(|u| (s.now - u).num_minutes().max(0)).unwrap_or(0);
            Span::styled(format!("geo {} · offline, dado de {age} min", g.source.label()), Style::new().fg(t.warn))
        }
        (Some(g), None) => lbl_owned(format!("geo {}", g.source.label()), t.muted),
        (None, None) => lbl_owned("geo …".into(), t.muted),
    };
    let mut right = Vec::new();
    let waiting = s.waiting_count();
    if waiting > 0 {
        right.push(Span::styled(format!("◆ {waiting} esperando você"), Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
        right.push(lbl("  ·  "));
    }
    // O alerta de sistema aparece aqui também: no modo compacto o painel some.
    if let Some(d) = &s.system.data
        && let Some((lvl, reason)) = d.danger.reasons.first()
    {
        right.push(Span::styled(format!(" ⚠ {reason} "), system::level_style(t, *lvl)));
        right.push(lbl("  ·  "));
    }
    right.extend([geo, lbl("  ·  "), Span::styled(t.name.clone(), Style::new().fg(t.accent)), Span::raw(" ")]);
    let right = Line::from(right);

    let rw = right.width() as u16;
    let left: Line = match s.notices.first() {
        Some(n) => Line::from(vec![
            // cabe entre a margem e o lado direito, deixando lugar para "x dispensar"
            {
                // Aviso de problema em amarelo com ⚠; confirmação de ação em tom neutro.
                let bad = ["não ", "erro", "falhou", "inválid", "ignorad"].iter().any(|w| n.to_lowercase().contains(w));
                let (icon, color) = if bad { ("⚠", t.warn) } else { ("›", t.accent) };
                Span::styled(
                    format!(" {icon} {} ", truncate(n, (area.width as usize).saturating_sub(rw as usize + 18))),
                    Style::new().fg(color),
                )
            },
            key("x"),
            lbl(" dispensar"),
        ]),
        None => {
            let hints: &[(&'static str, &'static str)] = match s.active_panel() {
                _ if s.status_pick.is_some() => &[("↑↓", " escolher  "), ("Enter", " gravar  "), ("1-5", " direto  "), ("0", " limpar  "), ("Esc", " cancelar")],
                _ if s.filter_editing => &[("digite", " para filtrar  "), ("↑↓", " escolher  "), ("Enter", " aplicar  "), ("Esc", " limpar")],
                Panel::System => &[("s", " velocidade  "), ("←→", " coluna  "), ("↑↓", " processo  "), ("K", " encerrar  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Worktrees => &[
                    ("←→", " aba  "),
                    ("↑↓", " escolher  "),
                    ("Enter", " ir para a pasta  "),
                    ("C", " claude numa aba nova  "),
                    ("s", " status  "),
                    ("D", " excluir  "),
                    ("/", " filtrar  "),
                    ("i", " legenda  "),
                    ("Tab", " foco  "),
                    ("q", " sair"),
                ],
                Panel::Agenda => &[("↑↓", " escolher  "), ("o", " abrir a chamada  "), ("y", " copiar link  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Docker => &[("↑↓", " escolher  "), ("K", " parar container  "), ("X", " derrubar tudo  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Sessions if s.wt_tab == crate::app::state::WtTab::Desktop => &[("←→", " aba  "), ("↑↓", " escolher  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Sessions => &[("←→", " aba  "), ("↑↓", " escolher  "), ("a", " permitir  "), ("Enter", " ir para a pasta  "), ("r", " retomar numa aba nova  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                _ => &[("Tab", " foco  "), ("t", " tema  "), ("i", " legenda  "), ("?", " ajuda  "), ("q", " sair")],
            };
            let mut spans = vec![Span::raw(" ")];
            for (k, l) in hints {
                spans.push(key(k));
                spans.push(lbl(l));
            }
            Line::from(spans)
        }
    };

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
        ("Enter (Worktrees)", "sai e vai para a worktree (com o shell-init)"),
        ("C (Worktrees)", "abre `claude --continue` numa aba nova, na worktree"),
        ("s (Worktrees)", "status: shape, wip, revisão manual, deploy, finalizado"),
        ("/ (Worktrees)", "filtra por nome, branch ou status; Esc limpa"),
        ("D (Worktrees)", "exclui a worktree selecionada (dois avisos antes)"),
        ("← → (Worktrees)", "troca entre as abas Worktrees e Claude"),
        ("Enter (Claude)", "sai e vai para a pasta da conversa"),
        ("r (Claude)", "retoma uma conversa encerrada numa aba nova"),
        ("o / y (Agenda)", "abre a chamada da reunião / copia o link"),
        ("z (alerta)", "adia o alerta vermelho de reunião; qualquer outra tecla o para"),
        ("K (Docker)", "para o container selecionado (docker stop)"),
        ("X (Docker)", "derruba tudo: compose down --remove-orphans por projeto"),
        ("a", "permite desta vez o pedido de permissão do Claude Code (com o hook instalado)"),
        ("i", "legenda dos símbolos do painel em foco"),
        ("←→ ↑↓ (Sistema)", "escolhe coluna e processo"),
        ("s (Sistema)", "mede a velocidade da internet (download e upload)"),
        ("K (Sistema)", "encerra o processo com SIGTERM (pede confirmação)"),
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

/// Tempo relativo curto: 45s, 12m, 3h, 5d, 2mo, 1a.
pub(crate) fn ago(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..2_592_000 => format!("{}d", s / 86_400),
        2_592_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}a", s / 31_536_000),
    }
}

/// Completa com espaços até `w` colunas (contando caracteres, não bytes).
pub(crate) fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w { s.to_string() } else { format!("{s}{}", " ".repeat(w - n)) }
}

pub(crate) fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
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
    fn status_column_and_picker() {
        use crate::{
            app::state::StatusPick,
            providers::git::{Repo, Worktree, WorktreesSnapshot},
            wt_status::WtStatus,
        };
        let mut s = state();
        let wt = |name: &str, path: &str| Worktree { name: name.into(), path: path.into(), branch: Some("main".into()), ..Default::default() };
        s.worktrees = Snapshot::ok(WorktreesSnapshot {
            repos: vec![Repo { name: "v360".into(), path: "/h/v360".into(), worktrees: vec![wt("build-s4", "/h/a"), wt("cfs", "/h/b")] }],
        });
        assert!(!render(200, 50, &s).contains("revisão"), "sem status, sem coluna");
        s.wt_status.insert("/h/a".into(), WtStatus::RevisaoManual);
        let out = render(200, 50, &s);
        assert!(out.lines().any(|l| l.contains("build-s4") && l.contains("revisão")), "{out}");
        s.status_pick = Some(StatusPick { path: "/h/a".into(), name: "build-s4".into(), sel: 2 });
        let out = render(200, 50, &s);
        assert!(out.contains("status · build-s4") && out.contains("revisão manual  (atual)"), "{out}");
    }

    #[test]
    fn compact_mode_shows_only_hero() {
        let out = render(120, 24, &state());
        assert!(!out.contains("Worktrees"));
        assert!(out.contains("Rio de Janeiro"));
    }
}
