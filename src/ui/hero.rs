//! Hero: abas de cidade/relógios extras, relógio grande, data e a próxima reunião.
//! Quando a reunião está para começar, o hero inteiro fica vermelho até uma tecla.

use chrono::Timelike;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, Paragraph,
        canvas::{Canvas, Map, MapResolution},
    },
};

use super::{bigclock::BigText, panel_block, truncate};
use crate::{
    alert::{Alert, Level},
    app::state::{AppState, Panel},
    locale,
    providers::{calendar::not_connected_hint, weather},
};

/// Cores do hero: as do tema, ou tudo em `alert_fg` sobre `alert_bg` no alerta.
struct Palette {
    fg: Color,
    muted: Color,
    accent: Color,
    clock: Color,
    border: Color,
}

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let alert = s.alert();
    let red = alert.level == Level::Imminent;
    let p = if red {
        Palette { fg: t.alert_fg, muted: t.alert_fg, accent: t.alert_fg, clock: t.alert_fg, border: t.alert_bg }
    } else {
        Palette { fg: t.fg, muted: t.muted, accent: t.accent, clock: t.clock, border: t.border }
    };
    let mut block: Block = panel_block(s, Panel::Hero, None);
    if red {
        block = block.style(Style::new().bg(t.alert_bg).fg(t.alert_fg)).border_style(Style::new().fg(t.alert_bg));
    } else if alert.level == Level::Soon {
        block = block.border_style(Style::new().fg(t.warn));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let local = s.local_now();
    let cfg = &s.config.clock;

    // Linha 1: abas "Cidade HH:MM │ Extra HH:MM │ …"
    let tab_style = if red {
        Style::new().fg(t.alert_bg).bg(t.alert_fg).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(p.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)
    };
    let mut tabs = vec![Span::styled(format!(" {} {} ", s.city_label(), locale::short_time(&local, cfg.hour12)), tab_style)];
    for (label, tz) in &s.extra_clocks {
        tabs.push(Span::styled(" │ ", Style::new().fg(p.border)));
        tabs.push(Span::styled(format!("{label} {}", locale::short_time(&s.now.with_timezone(tz), cfg.hour12)), Style::new().fg(p.fg)));
    }
    f.render_widget(Paragraph::new(Line::from(tabs)), Rect { height: 1, ..inner });

    // No alerta, a última linha vira o rodapé com as teclas.
    let footer_h = u16::from(red && inner.height >= 6);
    let mut body = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1 + footer_h), ..inner };

    // Mapa à direita, se houver largura e coordenadas.
    let coords = s.geo.data.as_ref().and_then(|g| Some((g.lat?, g.lon?)));
    if cfg.map
        && inner.width >= 100
        && body.height >= 6
        && let Some((lat, lon)) = coords
    {
        let map_w = (inner.width * 30 / 100).clamp(30, 52);
        let map = Rect { x: body.x + body.width - map_w, width: map_w, ..body };
        body.width -= map_w + 1;
        draw_map(f, map, s, lat, lon, &p, red);
    }

    let clock_text = locale::big_time(&local, cfg.seconds, cfg.hour12);
    const TEXT_LINES: u16 = 5; // lugar, respiro, respiro, data, reunião
    let scale = BigText::fit(&clock_text, body.width.saturating_sub(2), body.height.saturating_sub(TEXT_LINES), 3);
    let clock_h = scale.map_or(1, |sc| BigText::size(&clock_text, sc).1);
    // O relógio tem prioridade; o clima ocupa o que sobrar (até 2 linhas).
    let wx = weather_lines(s, &p, body.width as usize);
    let wx_h = (wx.len() as u16).min(body.height.saturating_sub(TEXT_LINES + clock_h));

    let total = (TEXT_LINES + clock_h + wx_h).min(body.height);
    let mut y = body.y + body.height.saturating_sub(total) / 2;
    let line_at = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let bottom = body.y + body.height;
    let centered = |f: &mut Frame, y: u16, line: Line| {
        if y < bottom {
            f.render_widget(Paragraph::new(line).alignment(Alignment::Center), line_at(y));
        }
    };

    centered(f, y, Line::styled(s.place_line(), Style::new().fg(p.fg).add_modifier(Modifier::BOLD)));
    y += 2;

    let clock_style = Style::new().fg(p.clock).add_modifier(Modifier::BOLD);
    match scale {
        Some(sc) => {
            let (w, h) = BigText::size(&clock_text, sc);
            let x = body.x + body.width.saturating_sub(w) / 2;
            let h = h.min(bottom.saturating_sub(y));
            f.render_widget(BigText { text: &clock_text, scale: sc, style: clock_style }, Rect { x, y, width: w, height: h });
            if cfg.hour12 {
                let ampm = if local.hour12().0 { "PM" } else { "AM" };
                let ax = x + w + 1;
                if ax + 2 <= body.x + body.width {
                    f.render_widget(Paragraph::new(Span::styled(ampm, Style::new().fg(p.muted))), Rect { x: ax, y, width: 2, height: 1 });
                }
            }
        }
        None => centered(f, y, Line::styled(clock_text.clone(), clock_style)),
    }
    y += clock_h + 1;

    centered(
        f,
        y,
        Line::styled(
            format!("{} · {}", locale::long_date(&local, s.config.locale), locale::utc_offset(&local)),
            Style::new().fg(p.fg),
        ),
    );
    y += 1;
    centered(f, y, meeting_line(s, &alert, &p, body.width as usize));
    for l in wx.into_iter().take(wx_h as usize) {
        y += 1;
        centered(f, y, l);
    }

    if footer_h == 1 {
        let key = |k: &'static str| Span::styled(k, Style::new().fg(t.alert_fg).add_modifier(Modifier::BOLD));
        let lbl = |l: &'static str| Span::styled(l, Style::new().fg(t.alert_fg));
        let mut spans = vec![key("qualquer tecla"), lbl(" parar   "), key("z"), lbl(" soneca ")];
        spans.push(Span::styled(format!("{} min", s.config.calendar.snooze_minutes), Style::new().fg(t.alert_fg)));
        if alert.event.is_some_and(|e| e.link().is_some()) {
            spans.extend([lbl("   "), key("o"), lbl(" abrir a chamada")]);
        }
        let r = Rect { y: inner.y + inner.height - 1, height: 1, ..inner };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

/// Mapa-múndi em braille com a cidade marcada.
fn draw_map(f: &mut Frame, area: Rect, s: &AppState, lat: f64, lon: f64, p: &Palette, red: bool) {
    let t = s.theme();
    let bg = if red { t.alert_bg } else { t.bg };
    let city = s.city_label();
    let (x0, x1, y0, y1) = (-180.0, 180.0, -58.0, 84.0);
    // O rótulo vai à direita do ponto; perto da borda leste, vai para a esquerda.
    let deg_per_cell = (x1 - x0) / area.width.max(1) as f64;
    let label_deg = (city.chars().count() as f64 + 2.0) * deg_per_cell;
    let label_x = if lon + label_deg + 2.0 * deg_per_cell > x1 { lon - label_deg - deg_per_cell } else { lon + 2.0 * deg_per_cell };
    let dot = Style::new().fg(if red { t.alert_fg } else { t.accent }).add_modifier(Modifier::BOLD);
    let label = if red {
        Style::new().fg(t.alert_bg).bg(t.alert_fg).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)
    };
    let land = p.muted;
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .background_color(bg)
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(move |ctx| {
            ctx.draw(&Map { color: land, resolution: MapResolution::High });
            ctx.layer();
            ctx.print(lon, lat.clamp(y0, y1), Span::styled("●", dot));
            ctx.print(label_x, lat.clamp(y0, y1), Span::styled(format!(" {city} "), label));
        });
    f.render_widget(canvas, area);
}

/// "Garoa 24°C · chuva 87% às 1h" e "0h 24° 86%   1h 24° 87%   …".
fn weather_lines(s: &AppState, p: &Palette, w: usize) -> Vec<Line<'static>> {
    let t = s.theme();
    let Some(wx) = &s.weather.data else { return Vec::new() };
    let fg = Style::new().fg(p.fg);
    let muted = Style::new().fg(p.muted);
    let mut first = vec![
        Span::styled(weather::describe(wx.code, wx.is_day).to_string(), fg),
        Span::styled(format!(" {:.0}°C", wx.temp), Style::new().fg(p.fg).add_modifier(Modifier::BOLD)),
    ];
    if let Some((h, pct)) = wx.rain_peak {
        first.push(Span::styled(format!(" · chuva {pct}% às {h}h"), fg));
    }
    let mut hours: Vec<Span> = Vec::new();
    let mut used = 0;
    for h in wx.hours.iter().take(s.config.weather.hours) {
        let item = format!("{}h {:.0}° {}", h.hour, h.temp, h.precip.map(|x| format!("{x}%")).unwrap_or_default());
        let add = item.chars().count() + if hours.is_empty() { 0 } else { 3 };
        if used + add > w {
            break;
        }
        if !hours.is_empty() {
            hours.push(Span::raw("   "));
        }
        let rainy = h.precip.is_some_and(|x| x >= 30);
        hours.push(Span::styled(format!("{}h ", h.hour), muted));
        hours.push(Span::styled(format!("{:.0}° ", h.temp), fg));
        hours.push(Span::styled(
            h.precip.map(|x| format!("{x}%")).unwrap_or_default(),
            Style::new().fg(if rainy && p.fg != t.alert_fg { t.spark } else { p.muted }),
        ));
        used += add;
    }
    let mut out = vec![Line::from(first)];
    if !hours.is_empty() {
        out.push(Line::from(hours));
    }
    out
}

/// "▶ 16:00 Daily · em 4 min", "▶ agora: Daily · começou há 2 min" ou o estado da agenda.
fn meeting_line(s: &AppState, alert: &Alert, p: &Palette, w: usize) -> Line<'static> {
    let t = s.theme();
    let Some(ev) = alert.event else {
        let msg = match (&s.calendar.data, &s.calendar.error) {
            (_, Some(e)) if not_connected_hint(e) => "agenda não conectada · rode `cockpit auth google`".to_string(),
            (None, Some(e)) => format!("agenda: {e}"),
            (None, None) if s.config.calendar.enabled => "lendo a agenda…".into(),
            (None, None) => String::new(),
            (Some(_), _) => format!("nenhuma reunião nas próximas {} h", s.config.calendar.lookahead_hours),
        };
        return Line::styled(truncate(&msg, w), Style::new().fg(p.muted));
    };
    let until = ev.start - s.now;
    let start = locale::short_time(&ev.start.with_timezone(&s.tz), s.config.clock.hour12);
    let (head, tail) = if until.num_seconds() > 0 {
        (format!("▶ {start} "), format!(" · em {}", until_text(until)))
    } else {
        let ago = -until.num_minutes();
        let end = locale::short_time(&ev.end.with_timezone(&s.tz), s.config.clock.hour12);
        let when = if ago < 1 { "começando agora".to_string() } else { format!("começou há {ago} min · até {end}") };
        ("▶ agora: ".to_string(), format!(" · {when}"))
    };
    let style = match alert.level {
        Level::Imminent if s.blink_on() => Style::new().fg(t.alert_bg).bg(t.alert_fg).add_modifier(Modifier::BOLD),
        Level::Imminent => Style::new().fg(t.alert_fg).add_modifier(Modifier::BOLD),
        Level::Soon => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        Level::Calm => Style::new().fg(p.fg),
    };
    let title = truncate(&ev.title, w.saturating_sub(head.chars().count() + tail.chars().count() + 2));
    Line::from(vec![Span::styled(format!("{head}{title}{tail}"), style)])
}

/// "menos de 1 min", "4 min", "1h 05m".
pub(crate) fn until_text(d: chrono::Duration) -> String {
    let m = d.num_minutes();
    match m {
        ..1 => "menos de 1 min".into(),
        1..60 => format!("{m} min"),
        _ => format!("{}h {:02}m", m / 60, m % 60),
    }
}
