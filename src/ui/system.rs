//! Painel de sistema: um htop enxuto. As linhas têm prioridade; se faltar altura,
//! somem as menos importantes. Em nível crítico, métrica e ofensores piscam.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::panel_block;
use crate::{
    app::state::{AppState, Panel, ProcCol},
    danger::Level,
    providers::system::{ProcGroup, SystemSnapshot},
    theme::Theme,
};

const LABEL_W: usize = 5;

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
        let style = level_style(t, worst, s.blink_on());
        block = block
            .border_style(Style::new().fg(if worst == Level::Critical { t.alert } else { t.warn }))
            .title(Span::styled(format!(" ⚠ {reason} "), style));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }

    let w = inner.width as usize;
    let h = inner.height as usize;
    let n = s.config.system.top_n;
    let has_procs = !d.top_cpu.is_empty() || !d.top_mem.is_empty();
    let top_rows = if has_procs && h >= 10 { (n + 1).min(h - 8) } else { 0 };

    let rows = metric_rows(s, d, w);
    let lines = fit_by_priority(rows, h - top_rows);
    let metrics_h = lines.len() as u16;
    f.render_widget(Paragraph::new(lines), Rect { height: metrics_h, ..inner });

    if top_rows > 0 {
        // Logo abaixo das métricas (com uma linha de respiro, se couber).
        let gap = u16::from(metrics_h + (top_rows as u16) < inner.height);
        let y = inner.y + metrics_h + gap;
        let r = Rect { y, height: top_rows as u16, ..inner };
        f.render_widget(Paragraph::new(top_lines(s, d, w, top_rows - 1)), r);
    }
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

fn metric_rows(s: &AppState, d: &SystemSnapshot, w: usize) -> Vec<(u8, Line<'static>)> {
    let t = s.theme();
    let blink = s.blink_on();
    let bar_w = ((w.saturating_sub(LABEL_W + 2 + 5)) / 2).clamp(6, 40);
    let dz = &d.danger;
    let mut rows: Vec<(u8, Line<'static>)> = Vec::new();

    // CPU
    let cpu = d.cpu as f64;
    let mut l = gauge(t, "CPU", cpu, bar_w, dz.cpu, blink);
    l.spans.push(muted(t, format!("  {} núcleos · load {:.2} {:.2} {:.2}", d.cores.len(), d.load.0, d.load.1, d.load.2)));
    rows.push((0, l));
    let hist: Vec<f64> = d.cpu_hist.iter().map(|v| *v as f64).collect();
    let sw = w.saturating_sub(LABEL_W + 4);
    rows.push((3, Line::from(vec![Span::raw(" ".repeat(LABEL_W)), Span::styled(spark(&hist, 100.0, sw), Style::new().fg(t.spark)), muted(t, " 2m".into())])));
    let mut cores = vec![Span::raw(" ".repeat(LABEL_W))];
    for c in &d.cores {
        let c = *c as f64;
        cores.push(Span::styled(eighth(c / 100.0).to_string(), Style::new().fg(gauge_color(t, c, Level::Ok))));
    }
    cores.push(muted(t, " por núcleo".into()));
    rows.push((4, Line::from(cores)));

    // Memória e swap
    let mut l = gauge(t, "MEM", d.mem_pct(), bar_w, dz.mem, blink);
    l.spans.push(muted(t, format!("  {} / {}", bytes(d.mem_used), bytes(d.mem_total))));
    rows.push((0, l));
    match d.swap_pct() {
        Some(p) => {
            let mut l = gauge(t, "SWP", p, bar_w, dz.swap, blink);
            l.spans.push(muted(t, format!("  {} / {}", bytes(d.swap_used), bytes(d.swap_total))));
            rows.push((1, l));
        }
        None => rows.push((1, Line::from(vec![label(t, "SWP", Level::Ok, blink), muted(t, "sem swap".into())]))),
    }
    if let Some(p) = d.psi {
        rows.push((
            3,
            Line::from(vec![
                label(t, "PSI", dz.psi, blink),
                muted(t, format!("cpu {:.1}  ", p.cpu)),
                Span::styled(format!("mem {:.1}", p.mem), level_style(t, dz.psi, blink)),
                muted(t, format!("  io {:.1}  (% do tempo travado, 10s)", p.io)),
            ]),
        ));
    }

    // Discos
    let mount_w = d.disks.iter().map(|x| x.mount.chars().count()).max().unwrap_or(1).min(10);
    for (i, disk) in d.disks.iter().enumerate() {
        let lvl = dz.disks.get(&disk.mount).copied().unwrap_or_default();
        let name = if i == 0 { "DSK" } else { "" };
        let pct = disk.pct();
        let dbar = bar_w.saturating_sub(mount_w + 1).max(4);
        rows.push((
            2,
            Line::from(vec![
                label(t, name, lvl, blink),
                Span::styled(format!("{:<mount_w$} ", truncate(&disk.mount, mount_w)), Style::new().fg(t.fg)),
                Span::styled(format!("▕{}▏", bar(pct, dbar)), Style::new().fg(gauge_color(t, pct, lvl))),
                Span::styled(format!(" {pct:>3.0}%"), Style::new().fg(t.fg)),
                muted(t, format!("  {} livres de {}", bytes(disk.total - disk.used), bytes(disk.total))),
            ]),
        ));
    }

    // Rede
    match &d.net {
        Some(n) => {
            let head = format!("{} ↓ {:>7} ↑ {:>7} ", n.iface, rate(n.rx), rate(n.tx));
            let max = n.rx_hist.iter().chain(&n.tx_hist).copied().fold(1.0, f64::max);
            let sw = w.saturating_sub(LABEL_W + head.chars().count());
            rows.push((
                1,
                Line::from(vec![
                    label(t, "NET", Level::Ok, blink),
                    Span::styled(head, Style::new().fg(t.fg)),
                    Span::styled(spark(&n.rx_hist, max, sw), Style::new().fg(t.spark)),
                ]),
            ));
        }
        None => rows.push((1, Line::from(vec![label(t, "NET", Level::Ok, blink), muted(t, "medindo…".into())]))),
    }

    // Ping
    if let Some(p) = &s.ping.data {
        let (txt, color) = match p.last_ms {
            Some(ms) => (format!("{ms:.0}ms"), if ms < 60.0 { t.ok } else if ms < 150.0 { t.warn } else { t.alert }),
            None => ("sem resposta".into(), t.alert),
        };
        let jit = p.jitter_ms.map(|j| format!(" ±{j:.0}")).unwrap_or_default();
        let host = p.target.trim_end_matches(":443");
        let tail = format!("{jit} · perda {:.0}% ", p.loss_pct);
        let vals: Vec<f64> = p.hist.iter().map(|x| x.unwrap_or(0.0)).collect();
        let max = vals.iter().copied().fold(1.0, f64::max);
        let used = LABEL_W + host.chars().count() + 1 + txt.chars().count() + tail.chars().count();
        rows.push((
            1,
            Line::from(vec![
                label(t, "PING", Level::Ok, blink),
                muted(t, format!("{host} ")),
                Span::styled(txt, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                muted(t, tail),
                Span::styled(spark(&vals, max, w.saturating_sub(used)), Style::new().fg(t.spark)),
            ]),
        ));
    } else {
        rows.push((1, Line::from(vec![label(t, "PING", Level::Ok, blink), muted(t, "medindo…".into())])));
    }

    // Rodapé
    let mut info = format!("up {} · {} processos", uptime(d.uptime), d.proc_count);
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
    rows.push((4, Line::from(vec![Span::raw(" ".repeat(LABEL_W)), muted(t, info)])));
    rows
}

/// Cabeçalho + `n` linhas: TOP CPU à esquerda, TOP MEM à direita (ou só a coluna ativa, se estreito).
fn top_lines(s: &AppState, d: &SystemSnapshot, w: usize, n: usize) -> Vec<Line<'static>> {
    let t = s.theme();
    let blink = s.blink_on();
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
        let mut style = if offender && lvl > Level::Ok { level_style(t, lvl, blink) } else { Style::new().fg(t.fg) };
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

fn gauge(t: &Theme, name: &str, pct: f64, bar_w: usize, lvl: Level, blink: bool) -> Line<'static> {
    Line::from(vec![
        label(t, name, lvl, blink),
        Span::styled(format!("▕{}▏", bar(pct, bar_w)), Style::new().fg(gauge_color(t, pct, lvl))),
        Span::styled(format!(" {pct:>3.0}%"), level_style(t, lvl, blink)),
    ])
}

fn label(t: &Theme, name: &str, lvl: Level, blink: bool) -> Span<'static> {
    let style = if lvl == Level::Ok { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { level_style(t, lvl, blink) };
    Span::styled(format!("{name:<LABEL_W$}"), style)
}

fn muted(t: &Theme, s: String) -> Span<'static> {
    Span::styled(s, Style::new().fg(t.muted))
}

/// Amarelo no aviso; vermelho piscando (alterna fundo) no crítico.
pub(crate) fn level_style(t: &Theme, lvl: Level, blink: bool) -> Style {
    match lvl {
        Level::Ok => Style::new().fg(t.fg),
        Level::Warn => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        Level::Critical if blink => Style::new().fg(t.alert_fg).bg(t.alert_bg).add_modifier(Modifier::BOLD),
        Level::Critical => Style::new().fg(t.alert).add_modifier(Modifier::BOLD),
    }
}

fn gauge_color(t: &Theme, pct: f64, lvl: Level) -> Color {
    match lvl {
        Level::Critical => t.alert,
        Level::Warn => t.warn,
        Level::Ok if pct < 60.0 => t.gauge_low,
        Level::Ok if pct < 85.0 => t.gauge_mid,
        Level::Ok => t.gauge_high,
    }
}

const EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Barra horizontal com resolução de 1/8 de célula.
fn bar(pct: f64, width: usize) -> String {
    let total = ((pct.clamp(0.0, 100.0) / 100.0) * (width * 8) as f64).round() as usize;
    let (full, rest) = (total / 8, total % 8);
    let mut s = "█".repeat(full);
    if full < width {
        s.push(EIGHTHS[rest]);
        s.push_str(&" ".repeat(width - full - 1));
    }
    s
}

/// Um caractere vertical (▁…█) para uma fração 0–1.
fn eighth(frac: f64) -> char {
    SPARK[((frac.clamp(0.0, 1.0) * 7.0).round() as usize).min(7)]
}

/// Sparkline dos últimos `width` valores, escalada por `max`.
fn spark(values: &[f64], max: f64, width: usize) -> String {
    let start = values.len().saturating_sub(width);
    values[start..].iter().map(|v| eighth(v / max.max(f64::EPSILON))).collect()
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
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

fn rate(bps: f64) -> String {
    format!("{}/s", bytes(bps as u64))
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_sparks() {
        assert_eq!(bar(0.0, 4), "    ");
        assert_eq!(bar(100.0, 4), "████");
        assert_eq!(bar(50.0, 4), "██  ");
        assert_eq!(bar(56.25, 4).chars().nth(2), Some('▎'));
        assert_eq!(bar(50.0, 4).chars().count(), 4);
        assert_eq!(spark(&[0.0, 50.0, 100.0], 100.0, 2), "▅█");
        assert_eq!(spark(&[], 100.0, 5), "");
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
