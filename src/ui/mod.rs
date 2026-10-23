//! Desenho. Só lê `AppState`; nunca faz I/O nem muda estado.

mod agenda;
mod bigclock;
mod chat;
pub(crate) mod hero;
pub mod layout;
mod legend;
mod system;
mod sysboxes;

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
    if let Some(r) = a.agenda {
        agenda::draw(f, r, s);
    }
    if let Some(r) = a.chat {
        chat::draw(f, r, s);
    }

    statusbar(f, a.status, s);
    if s.show_help {
        help(f, area, s);
    }
    if s.show_legend {
        legend::draw(f, area, s);
    }
    if let Some(p) = &s.chat_pick {
        chat_pick_dialog(f, area, s, p);
    }
    if let Some(c) = &s.confirm {
        confirm_dialog(f, area, s, c);
    }
}

/// Seletor de pasta do chat novo (tecla `n`).
fn chat_pick_dialog(f: &mut Frame, area: Rect, s: &AppState, p: &crate::app::state::ChatPick) {
    let t = s.theme();
    let dirs = s.chat_dirs(&p.query);
    let label_w = dirs.iter().map(|d| d.label.chars().count()).max().unwrap_or(1).min(40);
    // Cabe na tela: rola para manter a pasta escolhida à vista.
    let max_rows = (area.height as usize).saturating_sub(10).max(3);
    let first = p.sel.saturating_sub(max_rows - 1);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("pasta: ", Style::new().fg(t.muted)),
            Span::styled(p.query.clone(), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
            Span::styled("▏", Style::new().fg(t.accent)),
        ]),
        Line::raw(""),
    ];
    if dirs.is_empty() {
        lines.push(Line::styled("nenhuma pasta com esse nome", Style::new().fg(t.muted)));
    }
    for (i, d) in dirs.iter().enumerate().skip(first).take(max_rows) {
        let selected = i == p.sel;
        let line = Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<label_w$}", truncate(&d.label, label_w)), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", d.branch.as_deref().unwrap_or("")), Style::new().fg(t.highlight)),
            Span::styled(format!("  {}", crate::config::tilde(&d.path)), Style::new().fg(t.muted)),
        ]);
        lines.push(if selected { line.style(Style::new().bg(t.border)) } else { line });
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("digite", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" filtra · ", Style::new().fg(t.muted)),
        Span::styled("↑↓", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" escolhe · ", Style::new().fg(t.muted)),
        Span::styled("Enter", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" abre o chat · ", Style::new().fg(t.muted)),
        Span::styled("Esc", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" cancela", Style::new().fg(t.muted)),
    ]));
    let title = " chat novo em… ";
    // Caminhos longos não alargam o diálogo além de 100 colunas: o fim é cortado.
    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(20).clamp(50, 96);
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
        Confirm::CloseChat { title, .. } => (
            " fechar chat ".to_string(),
            vec![Line::from(vec![
                Span::styled("Fechar o chat ", Style::new().fg(t.fg)),
                Span::styled(title.clone(), Style::new().fg(t.alert).add_modifier(Modifier::BOLD)),
                Span::styled("? O claude dele é encerrado (dá para retomar pela aba Claude Code).", Style::new().fg(t.fg)),
            ])],
        ),
        Confirm::Quit { chats } => (
            " sair do cockpit ".to_string(),
            vec![Line::styled(
                format!("{chats} chat{} aberto{} será encerrado{}. Dá para retomar pela aba Claude Code.", pl(*chats as u32), pl(*chats as u32), pl(*chats as u32)),
                Style::new().fg(t.fg),
            )],
        ),
    };
    let (key, rest) = ("y", " confirma · qualquer outra tecla cancela");
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(key, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(rest, Style::new().fg(t.muted)),
    ]));
    // Pergunta curta fica centralizada; avisos com detalhes, alinhados à esquerda.
    let align = if matches!(c, Confirm::Kill(_) | Confirm::Quit { .. }) { Alignment::Center } else { Alignment::Left };
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
    // Ditado aparece aqui também: no modo compacto o chat some.
    if let Some(label) = voice_label(s) {
        right.push(Span::styled(label, Style::new().fg(t.alert).add_modifier(Modifier::BOLD)));
        right.push(lbl("  ·  "));
    }
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
            let hints: &[(&'static str, &'static str)] = match s.focus {
                _ if s.chat_pick.is_some() => &[("digite", " filtra  "), ("↑↓", " escolhe  "), ("Enter", " abre o chat  "), ("Esc", " cancela")],
                Panel::System => &[("←→", " coluna  "), ("↑↓", " processo  "), ("K", " encerrar  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Chat if s.chat_typing => &[("Ctrl+Q", " volta ao cockpit  "), ("Alt+V", " ditar  "), ("Alt+1-9", " troca de chat  "), ("Shift+PgUp/PgDn", " rola o histórico")],
                Panel::Chat => &[("Enter", " digitar  "), ("v", " ditar  "), ("n", " chat novo  "), ("←→", " troca  "), ("x", " fecha  "), ("Tab", " foco  "), ("q", " sair")],
                Panel::Agenda => &[("↑↓", " escolher  "), ("o", " abrir a chamada  "), ("y", " copiar link  "), ("i", " legenda  "), ("Tab", " foco  "), ("q", " sair")],
                _ => &[("Tab", " foco  "), ("t", " tema  "), ("v", " ditar  "), ("i", " legenda  "), ("?", " ajuda  "), ("q", " sair")],
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

/// "● gravando 0:07 · v para · Esc descarta" ou "… transcrevendo"; nada parado.
pub(crate) fn voice_label(s: &AppState) -> Option<String> {
    use crate::app::state::VoiceState;
    let stop = if s.focus == Panel::Chat && s.chat_typing { "Alt+V" } else { "v" };
    match s.voice {
        VoiceState::Idle => None,
        VoiceState::Recording { since } => {
            let secs = (s.now - since).num_seconds().max(0);
            Some(format!("● gravando {}:{:02} · {stop} para · Esc descarta", secs / 60, secs % 60))
        }
        VoiceState::Transcribing => Some("… transcrevendo · Esc descarta".into()),
    }
}

fn lbl_owned(text: String, color: ratatui::style::Color) -> Span<'static> {
    Span::styled(text, Style::new().fg(color))
}

fn help(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let rows = [
        ("Tab / Shift+Tab", "alterna o painel em foco"),
        ("t / T", "próximo / anterior tema (fica lembrado)"),
        ("v / Alt+V", "ditado: grava / para e escreve no chat"),
        ("n / Enter (Chat)", "abre um chat do Claude Code (escolhendo a pasta) / digita nele"),
        ("Ctrl+Q (Chat)", "para de digitar e devolve as teclas ao cockpit"),
        ("← → · x (Chat)", "troca de chat · fecha o chat (encerra o claude dele)"),
        ("Shift+PgUp/PgDn", "rola o histórico do chat (digitando)"),
        ("o / y (Agenda)", "abre a chamada da reunião / copia o link"),
        ("z (alerta)", "adia o alerta vermelho de reunião; qualquer outra tecla o para"),
        ("a", "permite desta vez o pedido de permissão do Claude Code (com o hook instalado)"),
        ("i", "legenda dos símbolos do painel em foco"),
        ("←→ ↑↓ (Sistema)", "escolhe coluna e processo"),
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
        assert!(out.contains("Sistema") && out.contains("Chat") && out.contains("Agenda"));
    }

    #[test]
    fn claude_limits_sit_below_the_top_processes() {
        use crate::providers::{
            system::{ProcGroup, SystemSnapshot},
            usage::{Limit, Severity, UsageSnapshot},
        };
        let mut s = state();
        let g = |name: &str| ProcGroup { name: name.into(), count: 1, cpu: 10.0, mem: 1 << 30, top_pid: 1 };
        s.system = Snapshot::ok(SystemSnapshot { top_cpu: vec![g("claude")], top_mem: vec![g("dockerd")], mem_total: 16 << 30, ..Default::default() });
        let lim = |kind: &str, label: &str, pct| Limit { kind: kind.into(), label: label.into(), percent: pct, severity: Severity::Normal, resets_at: None };
        s.usage = Snapshot::ok(UsageSnapshot { limits: vec![lim("session", "Sessão 5h", 22.0), lim("week", "Semana", 54.0)], plan: None });
        let out = render(200, 50, &s);
        let row = |needle: &str| out.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("sem {needle}:\n{out}"));
        assert!(row("TOP CPU") < row("LIMITES DO CLAUDE"), "{out}");
        assert!(row("LIMITES DO CLAUDE") < row("Sessão 5h") && row("Sessão 5h") < row("Semana"));
    }

    #[test]
    fn compact_mode_shows_only_hero() {
        let out = render(120, 24, &state());
        assert!(!out.contains("Chat"));
        assert!(out.contains("Rio de Janeiro"));
    }
}
