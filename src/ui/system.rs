//! Painel de sistema: um htop enxuto. As linhas têm prioridade; se faltar altura,
//! somem as menos importantes. Em nível crítico, métrica e ofensores ficam em vermelho negrito.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{panel_block, truncate};
use crate::{
    app::state::{AppState, Panel, ProcCol},
    danger::Level,
    locale,
    providers::{
        system::{ProcGroup, SystemSnapshot},
        usage::{Limit, Severity},
    },
    theme::Theme,
};

/// Largura da coluna de rótulo ("REDE  ").
const LABEL_W: usize = 7;

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let mut block = panel_block(s, Panel::System, Some("Sistema"));
    let Some(d) = &s.system.data else {
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(Paragraph::new(Line::styled("coletando métricas…", Style::new().fg(t.muted))), inner);
        return;
    };

    let worst = d.danger.worst();
    if worst > Level::Ok {
        let reason = d.danger.reasons.first().map(|r| r.1.as_str()).unwrap_or("");
        let style = level_style(t, worst);
        block = block
            .border_style(Style::new().fg(if worst == Level::Critical { t.alert } else { t.warn }))
            .title(Span::styled(format!(" ⚠ {reason} "), style));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }

    // Caixas de CPU e Memória no topo, a de Disco logo abaixo; o resto fica embaixo delas.
    let mut boxes_h = super::sysboxes::draw(f, inner, s, d);
    let rest = Rect { y: inner.y + boxes_h, height: inner.height.saturating_sub(boxes_h), ..inner };
    boxes_h += super::sysboxes::draw_disk(f, rest, s, d);
    let rest = Rect { y: inner.y + boxes_h, height: inner.height.saturating_sub(boxes_h), ..inner };
    boxes_h += super::sysboxes::draw_net(f, rest, s, d) + 1;
    let inner = Rect { y: inner.y + boxes_h, height: inner.height.saturating_sub(boxes_h), ..inner };
    if inner.height == 0 {
        return;
    }

    let w = inner.width as usize;
    let h = inner.height as usize;
    let n = s.config.system.top_n;
    let has_procs = !d.top_cpu.is_empty() || !d.top_mem.is_empty();
    // Especificações no rodapé: completas (cabeçalho + 5) com espaço, compactas (+ 3) se
    // apertado, e nada abaixo disso.
    let specs = spec_lines(s, w, h);
    let specs_h = specs.len();
    let h_rest = h - specs_h;
    // Limites do plano do Claude logo abaixo dos TOP, com uma linha de respiro; somem
    // antes dos processos quando falta altura.
    let limits = limit_lines(s, t, w);
    let limits_h = if !limits.is_empty() && h_rest >= limits.len() + 1 + 10 { limits.len() } else { 0 };
    let h_rest = h_rest - if limits_h > 0 { limits_h + 1 } else { 0 };
    let top_rows = if has_procs && h_rest >= 10 { (n + 1).min(h_rest - 8) } else { 0 };
    if specs_h > 0 {
        let y = inner.y + (h - specs_h) as u16;
        f.render_widget(Paragraph::new(specs), Rect { y, height: specs_h as u16, ..inner });
    }
    let h = h_rest;
    let inner = Rect { height: h as u16, ..inner };

    let rows = metric_rows(s, d);
    let lines = fit_by_priority(rows, h - top_rows);
    let metrics_h = lines.len() as u16;
    f.render_widget(Paragraph::new(lines), Rect { height: metrics_h, ..inner });

    let mut bottom = inner.y + metrics_h;
    if top_rows > 0 {
        // Logo abaixo das métricas (com uma linha de respiro, se couber).
        let gap = u16::from(metrics_h + (top_rows as u16) < inner.height);
        let y = inner.y + metrics_h + gap;
        let r = Rect { y, height: top_rows as u16, ..inner };
        f.render_widget(Paragraph::new(top_lines(s, d, w, top_rows - 1)), r);
        bottom = y + top_rows as u16;
    }
    if limits_h > 0 {
        let r = Rect { y: bottom + 1, height: limits_h as u16, ..inner };
        f.render_widget(Paragraph::new(limits), r);
    }
}

/// Bloco "LIMITES DO CLAUDE": "Sessão 5h ━━━━━━━━  45%  reinicia 19:50 (em 2h 03m)", um por limite.
fn limit_lines(s: &AppState, t: &Theme, w: usize) -> Vec<Line<'static>> {
    if !s.config.claude.usage_limits {
        return Vec::new();
    }
    let muted = Style::new().fg(t.muted);
    let head = "LIMITES DO CLAUDE ";
    let mut out = vec![Line::from(vec![
        Span::styled(head, muted),
        Span::styled("─".repeat(w.saturating_sub(head.chars().count())), Style::new().fg(t.border)),
    ])];
    let Some(d) = &s.usage.data else {
        let msg = match &s.usage.error {
            Some(e) => e.clone(),
            None => "consultando…".into(),
        };
        out.push(Line::styled(truncate(&msg, w), muted));
        return out;
    };
    let local_now = s.local_now();
    let resets: Vec<String> = d
        .limits
        .iter()
        .map(|l| l.resets_at.map(|r| format!("reinicia {}", locale::reset_when(&local_now, &r.with_timezone(&s.tz), s.config.locale))).unwrap_or_default())
        .collect();
    let label_w = d.limits.iter().map(|l| l.label.chars().count()).max().unwrap_or(6) + 2;
    let reset_w = resets.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    let bar_w = w.saturating_sub(label_w + 7 + reset_w).clamp(6, 24);
    for (l, reset) in d.limits.iter().zip(resets) {
        let color = limit_color(t, l);
        let (filled, empty) = bar(l.percent, bar_w);
        let mut line = Line::from(vec![
            Span::styled(format!("{:<label_w$}", l.label), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(filled, Style::new().fg(color)),
            Span::styled(empty, Style::new().fg(t.border)),
            Span::styled(format!(" {:>3.0}%  ", l.percent), Style::new().fg(if l.severity == Severity::Normal { t.fg } else { color }).add_modifier(Modifier::BOLD)),
            Span::styled(reset, muted),
        ]);
        // sem espaço, some o "reinicia" antes da barra
        if line.width() > w {
            line.spans.pop();
        }
        out.push(line);
    }
    if let Some(e) = &s.usage.error {
        out.push(Line::styled(truncate(&format!("desatualizado: {e}"), w), Style::new().fg(t.warn)));
    }
    out
}

fn limit_color(t: &Theme, l: &Limit) -> Color {
    match l.severity {
        Severity::Critical => t.alert,
        Severity::Warning => t.warn,
        Severity::Normal if l.percent < 60.0 => t.gauge_low,
        Severity::Normal if l.percent < 85.0 => t.gauge_mid,
        Severity::Normal => t.gauge_high,
    }
}

/// Bloco "MÁQUINA": SO, processador, núcleos, placa de vídeo e memória. No WSL, os
/// números são da máquina e, entre parênteses, quanto o WSL recebe.
fn spec_lines(s: &AppState, w: usize, h: usize) -> Vec<Line<'static>> {
    let Some(sp) = &s.specs.data else { return Vec::new() };
    let t = s.theme();
    let compact = h < 24;
    if h < 16 {
        return Vec::new();
    }
    let lbl = |x: &str| Span::styled(format!("{x:<9}"), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    let val = |x: String| Span::styled(x, Style::new().fg(t.fg));
    let dim = |x: String| Span::styled(x, Style::new().fg(t.muted));

    let so = match (&sp.host_os, sp.wsl) {
        (Some(host), true) => format!("{} · WSL2 no {host}", sp.os),
        (None, true) => format!("{} · WSL2", sp.os),
        _ => sp.os.clone(),
    };
    let cores = match sp.cores {
        Some(c) => format!("{c} físicos · {} threads", sp.threads),
        None => format!("{} threads", sp.threads),
    };
    let cores_wsl = (sp.wsl && sp.visible_threads != sp.threads).then(|| format!("  (WSL usa {})", sp.visible_threads));
    let gpu = if sp.gpus.is_empty() { "não identificada".to_string() } else { sp.gpus.join(" · ") };
    let mem = bytes(sp.mem_total.unwrap_or(sp.visible_mem));
    let mem_wsl = (sp.wsl && sp.mem_total.is_some()).then(|| format!("  (WSL usa {})", bytes(sp.visible_mem)));
    let fit = |line: Line<'static>| -> Line<'static> {
        // corta o último pedaço se não couber
        let mut line = line;
        while line.width() > w && line.spans.len() > 2 {
            line.spans.pop();
        }
        line
    };

    let head = "MÁQUINA ";
    let mut out = vec![Line::from(vec![
        Span::styled(head, Style::new().fg(t.muted)),
        Span::styled("─".repeat(w.saturating_sub(head.chars().count())), Style::new().fg(t.border)),
    ])];
    let so_line = Line::from(vec![lbl("SO"), val(truncate(&so, w.saturating_sub(9)))]);
    if compact {
        out.push(so_line);
        out.push(fit(Line::from(vec![lbl("CPU"), val(truncate(&sp.cpu, w.saturating_sub(9))), dim(format!(" · {cores}"))])));
        out.push(fit(Line::from(vec![lbl("GPU"), val(truncate(&gpu, w.saturating_sub(9))), dim(format!(" · RAM {mem}"))])));
    } else {
        out.push(so_line);
        out.push(Line::from(vec![lbl("CPU"), val(truncate(&sp.cpu, w.saturating_sub(9)))]));
        out.push(fit(Line::from(vec![lbl("NÚCLEOS"), val(cores)].into_iter().chain(cores_wsl.map(dim)).collect::<Vec<_>>())));
        out.push(Line::from(vec![lbl("GPU"), val(truncate(&gpu, w.saturating_sub(9)))]));
        out.push(fit(Line::from(vec![lbl("MEMÓRIA"), val(mem)].into_iter().chain(mem_wsl.map(dim)).collect::<Vec<_>>())));
    }
    out
}

/// Mantém as linhas de menor número de prioridade que cabem, na ordem original.
fn fit_by_priority(rows: Vec<(u8, Line<'static>)>, budget: usize) -> Vec<Line<'static>> {
    let mut idx: Vec<usize> = (0..rows.len()).collect();
    idx.sort_by_key(|&i| rows[i].0); // estável: empate mantém a ordem
    let mut keep = vec![false; rows.len()];
    for &i in idx.iter().take(budget) {
        keep[i] = true;
    }
    rows.into_iter().zip(keep).filter(|(_, k)| *k).map(|((_, l), _)| l).collect()
}

/// Prioridade das linhas em branco entre grupos: são as primeiras a sumir quando falta altura.
const SPACER: u8 = 9;

fn metric_rows(s: &AppState, d: &SystemSnapshot) -> Vec<(u8, Line<'static>)> {
    let t = s.theme();
    // Grade fixa: rótulo | barra | % | detalhe. Todas as barras começam e terminam na mesma coluna.
    let indent = || Span::raw(" ".repeat(LABEL_W));
    let dz = &d.danger;
    let mut rows: Vec<(u8, Line<'static>)> = Vec::new();
    let spacer = || (SPACER, Line::raw(""));

    if let Some(p) = d.psi {
        rows.push((
            3,
            Line::from(vec![
                label(t, "PSI", dz.psi),
                muted(t, format!("cpu {:<6.1}", p.cpu)),
                Span::styled(format!("mem {:<6.1}", p.mem), if dz.psi > Level::Ok { level_style(t, dz.psi) } else { Style::new().fg(t.fg) }),
                muted(t, format!("io {:.1}", p.io)),
            ]),
        ));
    }
    rows.push(spacer());

    // ── Rodapé
    let mut info = format!("ligado há {}", uptime(d.uptime));
    if let Some(b) = &d.battery {
        let st = match b.status.as_str() {
            "Charging" => "carregando",
            "Discharging" => "na bateria",
            "Full" => "cheia",
            "Not charging" => "na tomada",
            other => other,
        };
        info.push_str(&format!(" · bateria {}% {st}", b.pct));
    }
    rows.push((4, Line::from(vec![indent(), muted(t, info)])));
    rows
}

/// Cabeçalho + `n` linhas: TOP CPU à esquerda, TOP MEM à direita (ou só a coluna ativa, se estreito).
fn top_lines(s: &AppState, d: &SystemSnapshot, w: usize, n: usize) -> Vec<Line<'static>> {
    let t = s.theme();
    let focused = s.focus == Panel::System;
    let two = w >= 44;
    let col_w = if two { (w - 3) / 2 } else { w };
    let cpu_lvl = d.danger.cpu;
    let mem_lvl = d.danger.memory_pressure();
    let sel = s.proc_sel;

    let head = |txt: &str, col: ProcCol| {
        let active = focused && sel.col == col;
        let style = if active { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.muted) };
        let fill = col_w.saturating_sub(txt.chars().count() + 1);
        Span::styled(format!("{txt} {}", "─".repeat(fill)), style)
    };
    let mut out = Vec::with_capacity(n + 1);
    let mut h = Vec::new();
    if two || sel.col == ProcCol::Cpu {
        h.push(head("TOP CPU", ProcCol::Cpu));
    }
    if two {
        h.push(Span::raw("   "));
    }
    if two || sel.col == ProcCol::Mem {
        h.push(head("TOP MEM", ProcCol::Mem));
    }
    out.push(Line::from(h));

    let cell = |g: Option<&ProcGroup>, i: usize, col: ProcCol| -> Span<'static> {
        let Some(g) = g else { return Span::raw(" ".repeat(col_w)) };
        let (value, offender, lvl) = match col {
            ProcCol::Cpu => (format!("{:>5.0}%", g.cpu), i == 0 || g.cpu >= 50.0, cpu_lvl),
            ProcCol::Mem => {
                (format!("{:>6}", bytes(g.mem)), i == 0 || g.mem * 10 >= d.mem_total, mem_lvl)
            }
        };
        let count = if g.count > 1 { format!(" ×{}", g.count) } else { String::new() };
        let name_w = col_w.saturating_sub(value.chars().count() + count.chars().count() + 1);
        let text = format!("{:<name_w$}{count} {value}", truncate(&g.name, name_w));
        let mut style = if offender && lvl > Level::Ok { level_style(t, lvl) } else { Style::new().fg(t.fg) };
        if focused && sel.col == col && sel.row == i {
            style = style.add_modifier(Modifier::REVERSED);
        }
        Span::styled(text, style)
    };

    for i in 0..n {
        let mut spans = Vec::new();
        if two || sel.col == ProcCol::Cpu {
            spans.push(cell(d.top_cpu.get(i), i, ProcCol::Cpu));
        }
        if two {
            spans.push(Span::raw("   "));
        }
        if two || sel.col == ProcCol::Mem {
            spans.push(cell(d.top_mem.get(i), i, ProcCol::Mem));
        }
        out.push(Line::from(spans));
    }
    out
}

fn label(t: &Theme, name: &str, lvl: Level) -> Span<'static> {
    let style = if lvl == Level::Ok { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { level_style(t, lvl) };
    Span::styled(format!("{name:<LABEL_W$}"), style)
}

fn muted(t: &Theme, s: String) -> Span<'static> {
    Span::styled(s, Style::new().fg(t.muted))
}

/// Amarelo no aviso; vermelho em negrito no crítico. Sem pisca: piscar é só do alerta
/// de reunião, para não competir com ele.
pub(crate) fn level_style(t: &Theme, lvl: Level) -> Style {
    match lvl {
        Level::Ok => Style::new().fg(t.fg),
        Level::Warn => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        Level::Critical => Style::new().fg(t.alert).add_modifier(Modifier::BOLD),
    }
}

pub(crate) fn gauge_color(t: &Theme, pct: f64, lvl: Level) -> Color {
    match lvl {
        Level::Critical => t.alert,
        Level::Warn => t.warn,
        Level::Ok if pct < 60.0 => t.gauge_low,
        Level::Ok if pct < 85.0 => t.gauge_mid,
        Level::Ok => t.gauge_high,
    }
}

/// Barra de linha grossa: parte cheia e trilho, com meia célula de resolução (`╸`).
/// Evita os blocos parciais (▁▕▏), que em muitas fontes encostam na linha vizinha.
pub(crate) fn bar(pct: f64, width: usize) -> (String, String) {
    let halves = ((pct.clamp(0.0, 100.0) / 100.0) * (width * 2) as f64).round() as usize;
    let (full, half) = (halves / 2, halves % 2 == 1);
    let mut filled = "━".repeat(full);
    if half {
        filled.push('╸');
    }
    let used = full + usize::from(half);
    (filled, "━".repeat(width - used))
}

/// Tamanho em base 1024 com sufixo curto: 512K, 11.2G.
pub(crate) fn bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{v:.0}{}", U[i]) } else { format!("{v:.1}{}", U[i]) }
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars() {
        assert_eq!(bar(0.0, 4), (String::new(), "━━━━".into()));
        assert_eq!(bar(100.0, 4), ("━━━━".into(), String::new()));
        assert_eq!(bar(50.0, 4), ("━━".into(), "━━".into()));
        assert_eq!(bar(62.5, 4), ("━━╸".into(), "━".into()));
    }

    #[test]
    fn formats() {
        assert_eq!(bytes(512), "512B");
        assert_eq!(bytes(11 * 1024 * 1024 * 1024 + 200 * 1024 * 1024), "11.2G");
        assert_eq!(bytes(349 * 1024 * 1024), "349M");
        assert_eq!(uptime(3 * 86_400 + 4 * 3600), "3d 4h");
        assert_eq!(uptime(125 * 60), "2h 5m");
        assert_eq!(truncate("processable-file", 8), "process…");
    }

    #[test]
    fn priority_fit_keeps_order() {
        let l = |s: &'static str| Line::raw(s);
        let rows = vec![(0, l("a")), (3, l("b")), (1, l("c")), (0, l("d"))];
        let got: Vec<String> = fit_by_priority(rows, 3).iter().map(|l| l.to_string()).collect();
        assert_eq!(got, ["a", "c", "d"]);
    }
}
