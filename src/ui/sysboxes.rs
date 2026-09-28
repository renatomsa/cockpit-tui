//! Caixas de CPU e Memória no topo do painel Sistema: barra empilhada por tipo de uso,
//! legenda com quadradinhos e os detalhes (carga, threads, pressão, swap).

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Padding, Paragraph},
};

use super::{
    pad,
    system::{bytes, gauge_color, level_style},
    truncate,
};
use crate::{
    app::state::AppState,
    config::Locale,
    danger::Level,
    providers::system::SystemSnapshot,
    theme::Theme,
};

/// Desenha as duas caixas no topo de `area` e devolve a altura usada.
pub fn draw(f: &mut Frame, area: Rect, s: &AppState, d: &SystemSnapshot) -> u16 {
    let side_by_side = area.width >= 60;
    let box_w = if side_by_side { (area.width - 1) / 2 } else { area.width };
    let inner_w = box_w.saturating_sub(4) as usize; // bordas + padding
    let cpu = cpu_lines(s, d, inner_w);
    let mem = mem_lines(s, d, inner_w);
    let t = s.theme();

    let cpu_box = boxed(t, "CPU", d.cpu as f64, d.danger.cpu);
    let mem_box = boxed(t, "Memória", d.mem_pct(), d.danger.memory_pressure());
    if side_by_side {
        let h = (cpu.len().max(mem.len()) as u16 + 2).min(area.height);
        let [a, _, b] = Layout::horizontal([Constraint::Length(box_w), Constraint::Length(1), Constraint::Length(box_w)])
            .areas(Rect { height: h, ..area });
        f.render_widget(Paragraph::new(cpu).block(cpu_box), a);
        f.render_widget(Paragraph::new(mem).block(mem_box), b);
        h
    } else {
        let (hc, hm) = (cpu.len() as u16 + 2, mem.len() as u16 + 2);
        let hc = hc.min(area.height);
        let hm = hm.min(area.height - hc);
        f.render_widget(Paragraph::new(cpu).block(cpu_box), Rect { height: hc, ..area });
        f.render_widget(Paragraph::new(mem).block(mem_box), Rect { y: area.y + hc, height: hm, ..area });
        hc + hm
    }
}

/// Caixa Disco, na largura toda: uma barra por volume e a E/S do momento.
pub fn draw_disk(f: &mut Frame, area: Rect, s: &AppState, d: &SystemSnapshot) -> u16 {
    if d.disks.is_empty() || area.height < 3 {
        return 0;
    }
    let t = s.theme();
    let loc = s.config.locale;
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    let w = area.width.saturating_sub(4) as usize;

    let mount_w = d.disks.iter().map(|x| x.mount.chars().count()).max().unwrap_or(1).min(12);
    let details: Vec<(String, String)> = d.disks.iter().map(|x| (size(x.total - x.used, loc), size(x.total, loc))).collect();
    let free_w = details.iter().map(|(f, _)| f.chars().count()).max().unwrap_or(4);
    let total_w = details.iter().map(|(_, t)| t.chars().count()).max().unwrap_or(4);
    // "892G livres de 1007G", alinhado entre as linhas
    let detail_w = free_w + " livres de ".len() + total_w;
    let show_detail = w >= mount_w + 2 + 12 + 6 + detail_w;
    let bar_w = w.saturating_sub(mount_w + 2 + 6 + if show_detail { detail_w + 2 } else { 0 }).max(6);

    let mut lines: Vec<Line> = Vec::new();
    for (disk, (free, total)) in d.disks.iter().zip(&details) {
        let lvl = d.danger.disks.get(&disk.mount).copied().unwrap_or_default();
        let pct = disk.pct();
        let used_cells = ((pct.clamp(0.0, 100.0) / 100.0) * bar_w as f64).round() as usize;
        let mut spans = vec![
            Span::styled(pad(&truncate(&disk.mount, mount_w), mount_w + 2), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("█".repeat(used_cells), Style::new().fg(gauge_color(t, pct, lvl))),
            Span::styled("█".repeat(bar_w - used_cells), Style::new().fg(t.border)),
            Span::styled(format!(" {pct:>3.0}%"), if lvl == Level::Ok { fg.add_modifier(Modifier::BOLD) } else { level_style(t, lvl) }),
        ];
        if show_detail {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(format!("{free:>free_w$}"), fg));
            spans.push(Span::styled(" livres de ", muted));
            spans.push(Span::styled(format!("{total:>total_w$}"), fg));
        }
        lines.push(Line::from(spans));
    }
    if let Some(io) = d.disk_io {
        let rate = |b: f64| format!("{}/s", size(b as u64, loc));
        let items = [(t.spark, "leitura", rate(io.read)), (t.highlight, "escrita", rate(io.write))];
        let mut l = legend(t, &items, w);
        if let Some(first) = l.first_mut()
            && first.width() + 22 <= w
        {
            first.spans.push(Span::styled("   (discos do Linux)", muted));
        }
        lines.extend(l);
    }

    let worst = d.danger.disk();
    let border = match worst {
        Level::Critical => t.alert,
        Level::Warn => t.warn,
        Level::Ok => t.border,
    };
    let n = d.disks.len();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Disco ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_top(Line::from(Span::styled(format!(" {n} volume{} ", if n == 1 { "" } else { "s" }), muted)).right_aligned());
    let h = (lines.len() as u16 + 2).min(area.height);
    f.render_widget(Paragraph::new(lines).block(block), Rect { height: h, ..area });
    h
}

/// Caixa Rede: tráfego agora, Wi-Fi (no WSL, o do Windows), ping e teste de velocidade.
pub fn draw_net(f: &mut Frame, area: Rect, s: &AppState, d: &SystemSnapshot) -> u16 {
    if area.height < 3 {
        return 0;
    }
    let t = s.theme();
    let loc = s.config.locale;
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    let w = area.width.saturating_sub(4) as usize;
    let rate = |b: f64| format!("{}/s", size(b as u64, loc));
    let mbps = |m: f64| if m >= 100.0 { format!("{m:.0} Mbps") } else { format!("{} Mbps", dec(m, 1, loc)) };
    let host = s.host_net.data.as_ref();
    let mut lines: Vec<Line> = Vec::new();

    // Tráfego: o do Windows inteiro quando há, senão o da interface do Linux.
    let (rx, tx, origin) = match (host, &d.net) {
        (Some(h), _) => (Some(h.rx), Some(h.tx), "todo o Windows".to_string()),
        (None, Some(n)) => (Some(n.rx), Some(n.tx), n.iface.clone()),
        (None, None) => (None, None, String::new()),
    };
    match (rx, tx) {
        (Some(rx), Some(tx)) => {
            let mut l = vec![
                Span::styled(format!("↓ {:<10}", rate(rx)), Style::new().fg(t.spark).add_modifier(Modifier::BOLD)),
                Span::styled(format!("↑ {:<10}", rate(tx)), Style::new().fg(t.highlight).add_modifier(Modifier::BOLD)),
                Span::styled(origin, muted),
            ];
            if host.is_some()
                && let Some(n) = &d.net
            {
                let extra = format!("  · WSL ↓ {} ↑ {}", rate(n.rx), rate(n.tx));
                if Line::from(l.clone()).width() + extra.chars().count() <= w {
                    l.push(Span::styled(extra, muted));
                }
            }
            lines.push(Line::from(l));
        }
        _ => lines.push(Line::styled("medindo…", muted)),
    }

    // Wi-Fi
    if let Some(wifi) = host.and_then(|h| h.wifi.as_ref()) {
        let mut l = vec![Span::styled(wifi.ssid.clone(), Style::new().fg(t.fg).add_modifier(Modifier::BOLD))];
        if let Some(sig) = wifi.signal {
            let color = if sig >= 70 { t.ok } else if sig >= 40 { t.warn } else { t.alert };
            let cells = 10usize;
            let on = (sig as usize * cells).div_ceil(100).min(cells);
            l.extend([
                Span::styled("  sinal ", muted),
                Span::styled("█".repeat(on), Style::new().fg(color)),
                Span::styled("█".repeat(cells - on), Style::new().fg(t.border)),
                Span::styled(format!(" {sig}%"), Style::new().fg(color).add_modifier(Modifier::BOLD)),
            ]);
        }
        let mut info: Vec<String> = Vec::new();
        if let Some(r) = &wifi.radio {
            info.push(r.clone());
        }
        if let Some(c) = wifi.channel {
            info.push(format!("canal {c}"));
        }
        if let Some(b) = wifi.band_label() {
            info.push(b);
        }
        if let Some(r) = wifi.rx_mbps.or(host.and_then(|h| h.link_mbps)) {
            info.push(format!("link {}", mbps(r)));
        }
        let tail = format!("  · {}", info.join(" · "));
        if Line::from(l.clone()).width() + tail.chars().count() <= w {
            l.push(Span::styled(tail, muted));
        } else {
            lines.push(Line::from(l));
            l = vec![Span::styled(info.join(" · "), muted)];
        }
        lines.push(Line::from(l));
    } else if let Some(h) = host {
        let link = h.link_mbps.map(|m| format!(" · link {}", mbps(m))).unwrap_or_default();
        lines.push(Line::styled(format!("{}{link}", h.adapter), muted));
    }

    // Ping
    match &s.ping.data {
        Some(p) => {
            let (txt, color) = match p.last_ms {
                Some(ms) => (format!("{ms:.0}ms"), if ms < 60.0 { t.ok } else if ms < 150.0 { t.warn } else { t.alert }),
                None => ("sem resposta".into(), t.alert),
            };
            let jit = p.jitter_ms.map(|j| format!(" ±{j:.0}")).unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled("ping ", muted),
                Span::styled(txt, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{jit} · perda {:.0}% · {}", p.loss_pct, p.target.trim_end_matches(":443")), muted),
            ]));
        }
        None => lines.push(Line::styled("ping medindo…", muted)),
    }

    // Teste de velocidade
    let sp = &s.speed;
    let line = match (&sp.running, &sp.last) {
        (Some((phase, now)), _) => {
            let what = match phase {
                crate::providers::speedtest::Phase::Download => "download",
                crate::providers::speedtest::Phase::Upload => "upload",
            };
            let so_far = if *now > 0.0 { format!(" {}", mbps(*now)) } else { String::new() };
            Line::from(vec![Span::styled(format!("velocidade: medindo {what}…"), Style::new().fg(t.accent)), Span::styled(so_far, fg)])
        }
        (None, Some(r)) => Line::from(vec![
            Span::styled("velocidade ", muted),
            Span::styled(format!("↓ {}", mbps(r.down_mbps)), Style::new().fg(t.spark).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("↑ {}", mbps(r.up_mbps)), Style::new().fg(t.highlight).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · há {} · s mede de novo", super::ago((s.now - r.at).num_seconds())), muted),
        ]),
        (None, None) => Line::from(vec![
            Span::styled("velocidade: ", muted),
            Span::styled("s", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(" mede download e upload (no painel Sistema)", muted),
        ]),
    };
    lines.push(line);
    if let Some(e) = &sp.error {
        lines.push(Line::styled(truncate(&format!("último teste falhou: {e}"), w), Style::new().fg(t.warn)));
    }

    let right = match host {
        Some(h) => match h.wifi.as_ref() {
            Some(wf) => format!(" Wi-Fi · {} ", wf.ssid),
            None => format!(" {} ", h.adapter),
        },
        None => d.net.as_ref().map(|n| format!(" {} ", n.iface)).unwrap_or_default(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Rede ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_top(Line::from(Span::styled(right, muted)).right_aligned());
    let h = (lines.len() as u16 + 2).min(area.height);
    f.render_widget(Paragraph::new(lines).block(block), Rect { height: h, ..area });
    h
}

fn boxed(t: &Theme, title: &'static str, pct: f64, lvl: Level) -> Block<'static> {
    let border = match lvl {
        Level::Critical => t.alert,
        Level::Warn => t.warn,
        Level::Ok => t.border,
    };
    let pct_style = if lvl == Level::Ok { Style::new().fg(t.fg).add_modifier(Modifier::BOLD) } else { level_style(t, lvl) };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_top(Line::from(Span::styled(format!(" {pct:.0}% "), pct_style)).right_aligned())
}

/// Barra de largura `w` com um segmento por parte (em %) e trilho no que sobra.
fn stacked(t: &Theme, parts: &[(f64, Color)], w: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let (mut cum, mut drawn) = (0.0, 0usize);
    for (pct, color) in parts {
        cum += pct.max(0.0) / 100.0 * w as f64;
        let end = (cum.round() as usize).min(w);
        if end > drawn {
            spans.push(Span::styled("█".repeat(end - drawn), Style::new().fg(*color)));
            drawn = end;
        }
    }
    if drawn < w {
        spans.push(Span::styled("█".repeat(w - drawn), Style::new().fg(t.border)));
    }
    Line::from(spans)
}

/// Itens "■ rótulo valor" quebrados em linhas de até `w` colunas.
fn legend(t: &Theme, items: &[(Color, &str, String)], w: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut used = 0;
    for (color, label, value) in items {
        let item_w = 2 + label.chars().count() + 1 + value.chars().count();
        let gap = if cur.is_empty() { 0 } else { 3 };
        if !cur.is_empty() && used + gap + item_w > w {
            lines.push(Line::from(std::mem::take(&mut cur)));
            used = 0;
        }
        if !cur.is_empty() {
            cur.push(Span::raw("   "));
            used += 3;
        }
        cur.push(Span::styled("■ ", Style::new().fg(*color)));
        cur.push(Span::styled(format!("{label} "), Style::new().fg(t.muted)));
        cur.push(Span::styled(value.clone(), Style::new().fg(t.fg)));
        used += item_w;
    }
    if !cur.is_empty() {
        lines.push(Line::from(cur));
    }
    lines
}

/// Decimal com vírgula em pt-BR: 6.54 → "6,54".
fn dec(v: f64, places: usize, loc: Locale) -> String {
    let s = format!("{v:.places$}");
    if loc == Locale::PtBr { s.replace('.', ",") } else { s }
}

fn size(b: u64, loc: Locale) -> String {
    let s = bytes(b);
    if loc == Locale::PtBr { s.replace('.', ",") } else { s }
}

fn cpu_lines(s: &AppState, d: &SystemSnapshot, w: usize) -> Vec<Line<'static>> {
    let t = s.theme();
    let loc = s.config.locale;
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    let mut out = Vec::new();
    match d.cpu_parts {
        Some(p) => {
            out.push(stacked(
                t,
                &[(p.user, t.gauge_low), (p.system, t.alert), (p.nice, t.warn), (p.iowait, t.highlight), (p.steal, t.spark)],
                w,
            ));
            let mut items = vec![
                (t.gauge_low, "usuário", format!("{:.0}%", p.user)),
                (t.alert, "sistema", format!("{:.0}%", p.system)),
                (t.warn, "nice", format!("{:.0}%", p.nice)),
            ];
            if p.iowait >= 0.5 {
                items.push((t.highlight, "E/S", format!("{:.0}%", p.iowait)));
            }
            if p.steal >= 0.5 {
                items.push((t.spark, "steal", format!("{:.0}%", p.steal)));
            }
            out.extend(legend(t, &items, w));
            let mut idle = vec![Span::styled("ociosa ", muted), Span::styled(format!("{:.0}%", p.idle), fg)];
            if let Some(c) = d.temp_c {
                idle.push(Span::styled(" · ", muted));
                idle.push(Span::styled(format!("{c:.0}°C"), Style::new().fg(if c >= 90.0 { t.alert } else if c >= 75.0 { t.warn } else { t.fg })));
            }
            out.push(Line::from(idle));
        }
        None => out.push(stacked(t, &[(d.cpu as f64, t.gauge_low)], w)),
    }
    let cores = d.cores.max(1);
    let load_pct = d.load.0 / cores as f64 * 100.0;
    let load = vec![
        Span::styled("carga ", muted),
        Span::styled(format!("{} {} {}", dec(d.load.0, 2, loc), dec(d.load.1, 2, loc), dec(d.load.2, 2, loc)), fg),
    ];
    let share = vec![
        Span::styled(format!("{load_pct:.0}%"), Style::new().fg(if load_pct >= 100.0 { t.warn } else { t.fg })),
        Span::styled(format!(" de {cores} núcleos"), muted),
    ];
    let one: Line = Line::from(load.iter().cloned().chain([Span::styled(" · ", muted)]).chain(share.iter().cloned()).collect::<Vec<_>>());
    if one.width() <= w {
        out.push(one);
    } else {
        // não cabe numa linha: a fração dos núcleos vai para baixo
        out.push(Line::from(load));
        out.push(Line::from(std::iter::once(Span::styled("      ", muted)).chain(share).collect::<Vec<_>>()));
    }
    let mut procs = vec![Span::styled(format!("{}", d.proc_count), fg), Span::styled(" processos", muted)];
    if let Some(th) = d.threads {
        procs.extend([Span::styled(" · ", muted), Span::styled(th.to_string(), fg), Span::styled(" threads", muted)]);
    }
    out.push(Line::from(procs));
    out
}

fn mem_lines(s: &AppState, d: &SystemSnapshot, w: usize) -> Vec<Line<'static>> {
    let t = s.theme();
    let loc = s.config.locale;
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    let mut out = Vec::new();

    let mut head = vec![
        Span::styled(size(d.mem_used, loc), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
        Span::styled(" de ", muted),
        Span::styled(size(d.mem_total, loc), fg),
    ];
    if let Some(psi) = d.psi {
        let (label, color) = if psi.mem >= 10.0 {
            ("alta", t.alert)
        } else if psi.mem >= 1.0 {
            ("média", t.warn)
        } else {
            ("baixa", t.ok)
        };
        head.extend([Span::styled(" · pressão ", muted), Span::styled(label, Style::new().fg(color).add_modifier(Modifier::BOLD))]);
    }

    match d.mem_parts {
        Some(m) => {
            let pct = |x: u64| x as f64 * 100.0 / m.total as f64;
            out.push(stacked(
                t,
                &[
                    (pct(m.apps), t.gauge_low),
                    (pct(m.kernel), t.alert),
                    (pct(m.shared), t.warn),
                    (pct(m.compressed), t.highlight),
                    (pct(m.cache), t.spark),
                ],
                w,
            ));
            out.push(Line::from(head));
            let mut items = vec![
                (t.gauge_low, "apps", size(m.apps, loc)),
                (t.alert, "kernel", size(m.kernel, loc)),
                (t.warn, "compartilhada", size(m.shared, loc)),
            ];
            if m.compressed > 0 {
                items.push((t.highlight, "comprimida", size(m.compressed, loc)));
            }
            items.push((t.spark, "cache", size(m.cache, loc)));
            out.extend(legend(t, &items, w));
            out.push(Line::from(vec![
                Span::styled("livre ", muted),
                Span::styled(size(m.free, loc), fg),
                Span::styled(" · disponível ", muted),
                Span::styled(size(m.available, loc), fg),
            ]));
        }
        None => {
            out.push(stacked(t, &[(d.mem_pct(), t.gauge_low)], w));
            out.push(Line::from(head));
        }
    }
    match d.swap_pct() {
        Some(p) => out.push(Line::from(vec![
            Span::styled("swap ", muted),
            Span::styled(size(d.swap_used, loc), if d.danger.swap > Level::Ok { level_style(t, d.danger.swap) } else { fg }),
            Span::styled(format!(" de {} ({p:.0}%)", size(d.swap_total, loc)), muted),
        ])),
        None => out.push(Line::styled("sem swap", muted)),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn theme() -> Theme {
        Theme::parse("[colors]\nfg = \"#ffffff\"", "t").unwrap()
    }

    #[test]
    fn stacked_bar_fills_exactly_the_width() {
        let t = theme();
        for parts in [vec![(12.0, Color::Green), (10.0, Color::Red), (0.4, Color::Yellow)], vec![(100.0, Color::Green)], vec![]] {
            let l = stacked(&t, &parts, 30);
            assert_eq!(l.width(), 30);
        }
        let l = stacked(&t, &[(50.0, Color::Green), (25.0, Color::Red)], 20);
        assert_eq!(l.spans[0].content.chars().count(), 10);
        assert_eq!(l.spans[1].content.chars().count(), 5);
    }

    #[test]
    fn legend_wraps_by_width() {
        let t = theme();
        let items = vec![(Color::Green, "usuário", "12%".to_string()), (Color::Red, "sistema", "10%".into()), (Color::Yellow, "nice", "0%".into())];
        assert_eq!(legend(&t, &items, 80).len(), 1);
        let wrapped = legend(&t, &items, 30);
        assert_eq!(wrapped.len(), 2);
        assert!(wrapped.iter().all(|l| l.width() <= 30));
    }

    #[test]
    fn decimals_follow_locale() {
        assert_eq!(dec(6.544, 2, Locale::PtBr), "6,54");
        assert_eq!(dec(6.544, 2, Locale::En), "6.54");
        assert_eq!(size(13 * 1024 * 1024 * 1024 + 400 * 1024 * 1024, Locale::PtBr), "13,4G");
    }
}
