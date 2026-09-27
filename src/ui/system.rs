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

/// Largura da coluna de rótulo ("DISCO ") e da de percentual (" 100%  ").
const LABEL_W: usize = 7;
const PCT_W: usize = 7;

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

/// Prioridade das linhas em branco entre grupos: são as primeiras a sumir quando falta altura.
const SPACER: u8 = 9;

fn metric_rows(s: &AppState, d: &SystemSnapshot, w: usize) -> Vec<(u8, Line<'static>)> {
    let t = s.theme();
    let blink = s.blink_on();
    // Grade fixa: rótulo | barra | % | detalhe. Todas as barras começam e terminam na mesma coluna.
    let bar_w = w.saturating_sub(LABEL_W + PCT_W + 22).clamp(8, 36);
    let indent = || Span::raw(" ".repeat(LABEL_W));
    let dz = &d.danger;
    let mut rows: Vec<(u8, Line<'static>)> = Vec::new();
    let spacer = || (SPACER, Line::raw(""));

    // ── CPU
    let mut l = gauge(t, "CPU", d.cpu as f64, bar_w, dz.cpu, blink);
    l.spans.push(muted(t, format!("load {:.2} {:.2} {:.2}", d.load.0, d.load.1, d.load.2)));
    rows.push((0, l));
    let hist: Vec<f64> = d.cpu_hist.iter().map(|v| *v as f64).collect();
    let hist_w = bar_w + PCT_W;
    let secs = hist.len().min(hist_w * 2);
    rows.push((
        3,
        Line::from(vec![
            indent(),
            Span::styled(braille(&hist, 100.0, hist_w), Style::new().fg(t.spark)),
            muted(t, format!("  últimos {secs}s")),
        ]),
    ));
    let mut cores = vec![indent(), muted(t, "núcleos".into())];
    let room = w.saturating_sub(LABEL_W + 7) / 4;
    for (i, c) in d.cores.iter().enumerate() {
        if i == room && d.cores.len() > room {
            cores.push(muted(t, " …".into()));
            break;
        }
        let c = *c as f64;
        cores.push(Span::styled(format!("{c:>4.0}"), Style::new().fg(gauge_color(t, c, Level::Ok))));
    }
    rows.push((4, Line::from(cores)));
    rows.push(spacer());

    // ── Memória
    let mut l = gauge(t, "MEM", d.mem_pct(), bar_w, dz.mem, blink);
    l.spans.push(muted(t, format!("{} / {}", bytes(d.mem_used), bytes(d.mem_total))));
    rows.push((0, l));
    match d.swap_pct() {
        Some(p) => {
            let mut l = gauge(t, "SWAP", p, bar_w, dz.swap, blink);
            l.spans.push(muted(t, format!("{} / {}", bytes(d.swap_used), bytes(d.swap_total))));
            rows.push((1, l));
        }
        None => rows.push((1, Line::from(vec![label(t, "SWAP", Level::Ok, blink), muted(t, "sem swap".into())]))),
    }
    if let Some(p) = d.psi {
        rows.push((
            3,
            Line::from(vec![
                label(t, "PSI", dz.psi, blink),
                muted(t, format!("cpu {:<6.1}", p.cpu)),
                Span::styled(format!("mem {:<6.1}", p.mem), if dz.psi > Level::Ok { level_style(t, dz.psi, blink) } else { Style::new().fg(t.fg) }),
                muted(t, format!("io {:.1}", p.io)),
            ]),
        ));
    }
    rows.push(spacer());

    // ── Discos
    let mount_w = d.disks.iter().map(|x| x.mount.chars().count()).max().unwrap_or(1).min(10);
    for (i, disk) in d.disks.iter().enumerate() {
        let lvl = dz.disks.get(&disk.mount).copied().unwrap_or_default();
        let mut l = gauge(t, if i == 0 { "DISCO" } else { "" }, disk.pct(), bar_w, lvl, blink);
        l.spans.push(Span::styled(format!("{:<mount_w$}  ", truncate(&disk.mount, mount_w)), Style::new().fg(t.fg)));
        l.spans.push(muted(t, format!("{} livres", bytes(disk.total - disk.used))));
        rows.push((2, l));
    }
    if !d.disks.is_empty() {
        rows.push(spacer());
    }

    // ── Rede. O texto ocupa a largura da barra + %, e o gráfico começa na coluna
    // dos detalhes, igual para REDE e PING.
    let text_w = bar_w + PCT_W;
    let graph_w = w.saturating_sub(LABEL_W + text_w + 1).min(24);
    let pad = |s: String| format!("{s:<text_w$}");
    match &d.net {
        Some(n) => {
            let head = pad(format!("↓ {:<9} ↑ {:<9} {}", rate(n.rx), rate(n.tx), n.iface));
            let max = n.rx_hist.iter().chain(&n.tx_hist).copied().fold(1.0, f64::max);
            rows.push((
                1,
                Line::from(vec![
                    label(t, "REDE", Level::Ok, blink),
                    Span::styled(head, Style::new().fg(t.fg)),
                    Span::styled(braille(&n.rx_hist, max, graph_w), Style::new().fg(t.spark)),
                ]),
            ));
        }
        None => rows.push((1, Line::from(vec![label(t, "REDE", Level::Ok, blink), muted(t, "medindo…".into())]))),
    }
    match &s.ping.data {
        Some(p) => {
            let (txt, color) = match p.last_ms {
                Some(ms) => (format!("{ms:.0}ms"), if ms < 60.0 { t.ok } else if ms < 150.0 { t.warn } else { t.alert }),
                None => ("sem resposta".into(), t.alert),
            };
            let jit = p.jitter_ms.map(|j| format!("±{j:.0}")).unwrap_or_default();
            let host = p.target.trim_end_matches(":443").to_string();
            let mid = format!(" {jit:<4} perda {:.0}%  {host}", p.loss_pct);
            let mid = format!("{mid:<w$}", w = text_w.saturating_sub(txt.chars().count()));
            let vals: Vec<f64> = p.hist.iter().map(|x| x.unwrap_or(0.0)).collect();
            let max = vals.iter().copied().fold(1.0, f64::max);
            rows.push((
                1,
                Line::from(vec![
                    label(t, "PING", Level::Ok, blink),
                    Span::styled(txt, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                    muted(t, mid),
                    Span::styled(braille(&vals, max, graph_w), Style::new().fg(t.spark)),
                ]),
            ));
        }
        None => rows.push((1, Line::from(vec![label(t, "PING", Level::Ok, blink), muted(t, "medindo…".into())]))),
    }

    // ── Rodapé
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
    rows.push((4, Line::from(vec![indent(), muted(t, info)])));
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

/// `RÓTULO ━━━━━━━━━━━━━━━  43%  ` — o detalhe entra depois, pelo chamador.
fn gauge(t: &Theme, name: &str, pct: f64, bar_w: usize, lvl: Level, blink: bool) -> Line<'static> {
    let (filled, empty) = bar(pct, bar_w);
    let pct_style = if lvl == Level::Ok { Style::new().fg(t.fg).add_modifier(Modifier::BOLD) } else { level_style(t, lvl, blink) };
    Line::from(vec![
        label(t, name, lvl, blink),
        Span::styled(filled, Style::new().fg(gauge_color(t, pct, lvl))),
        Span::styled(empty, Style::new().fg(t.border)),
        Span::styled(format!(" {pct:>3.0}%"), pct_style),
        Span::raw("  "),
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

/// Barra de linha grossa: parte cheia e trilho, com meia célula de resolução (`╸`).
/// Evita os blocos parciais (▁▕▏), que em muitas fontes encostam na linha vizinha.
fn bar(pct: f64, width: usize) -> (String, String) {
    let halves = ((pct.clamp(0.0, 100.0) / 100.0) * (width * 2) as f64).round() as usize;
    let (full, half) = (halves / 2, halves % 2 == 1);
    let mut filled = "━".repeat(full);
    if half {
        filled.push('╸');
    }
    let used = full + usize::from(half);
    (filled, "━".repeat(width - used))
}

/// Gráfico em braille: 2 amostras por célula, 4 níveis de altura cada. Os pontos ficam
/// dentro da célula, então o gráfico não se mistura com as linhas de cima e de baixo.
/// Alinhado à direita (a amostra mais recente fica na ponta).
fn braille(values: &[f64], max: f64, width: usize) -> String {
    const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
    let n = width * 2;
    let vals = &values[values.len().saturating_sub(n)..];
    let pad = n - vals.len();
    let level = |i: usize| -> usize {
        if i < pad {
            return 0;
        }
        let v = vals[i - pad];
        if v <= 0.0 { 0 } else { ((v / max.max(f64::EPSILON)) * 4.0).ceil().clamp(1.0, 4.0) as usize }
    };
    (0..width)
        .map(|c| {
            let (l, r) = (level(c * 2), level(c * 2 + 1));
            if l == 0 && r == 0 {
                return ' ';
            }
            let bits = LEFT[..l].iter().sum::<u32>() + RIGHT[..r].iter().sum::<u32>();
            char::from_u32(0x2800 + bits).unwrap_or(' ')
        })
        .collect()
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
    fn bars_and_braille() {
        assert_eq!(bar(0.0, 4), (String::new(), "━━━━".into()));
        assert_eq!(bar(100.0, 4), ("━━━━".into(), String::new()));
        assert_eq!(bar(50.0, 4), ("━━".into(), "━━".into()));
        assert_eq!(bar(62.5, 4), ("━━╸".into(), "━".into()));
        // 0 → vazio, 50% → 2 pontos, 100% → coluna cheia; alinhado à direita
        assert_eq!(braille(&[0.0, 50.0, 100.0], 100.0, 2), " ⣼");
        assert_eq!(braille(&[100.0, 100.0], 100.0, 1), "⣿");
        assert_eq!(braille(&[1.0], 100.0, 1), "⢀");
        assert_eq!(braille(&[], 100.0, 3), "   ");
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
