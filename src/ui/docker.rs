//! Painel do Docker: containers rodando (o `docker ps` simples), com `K` para parar um
//! e `X` para derrubar tudo.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{pad, panel_block, truncate};
use crate::{
    app::state::{AppState, Panel},
    providers::docker::{Container, Health},
    theme::Theme,
};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let focused = s.focus == Panel::Docker;
    let n = s.docker.data.as_ref().map_or(0, |d| d.containers.len());
    let title = if n > 0 { format!("Docker · {n} rodando") } else { "Docker".to_string() };
    let mut block = panel_block(s, Panel::Docker, Some(&title));
    if n > 0 {
        let hint = if focused { " K parar · X derrubar tudo " } else { " Tab até aqui: K parar · X derrubar tudo " };
        block = block.title_top(Line::styled(hint, Style::new().fg(t.muted)).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 20 {
        return;
    }
    let w = inner.width as usize;

    let Some(d) = &s.docker.data else {
        let msg = match &s.docker.error {
            Some(e) => format!("Docker indisponível: {e}"),
            None => "consultando o Docker…".into(),
        };
        f.render_widget(Paragraph::new(Line::styled(truncate(&msg, w), Style::new().fg(t.muted))), inner);
        return;
    };
    if d.containers.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("nenhum container rodando", Style::new().fg(t.muted))), inner);
        return;
    }

    let name_w = d.containers.iter().map(|c| c.name.chars().count()).max().unwrap_or(8).clamp(8, 28);
    let image_w = d.containers.iter().map(|c| c.image.chars().count()).max().unwrap_or(8).clamp(8, 34);
    let status_w = d.containers.iter().map(|c| short_status(&c.status).chars().count()).max().unwrap_or(6).clamp(6, 22);
    let ports_w = d.containers.iter().map(|c| ports(c).chars().count()).max().unwrap_or(0).min(18);
    // encolhe a imagem primeiro, depois o nome, se não couber
    let fixed = 4 + status_w + 2 + ports_w + 2 + 10;
    let mut image_w = image_w;
    let mut name_w = name_w;
    while name_w + image_w + 4 + fixed > w && image_w > 10 {
        image_w -= 1;
    }
    while name_w + image_w + 4 + fixed > w && name_w > 10 {
        name_w -= 1;
    }

    let lines: Vec<Line> = d
        .containers
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let selected = focused && i == s.docker_sel;
            let mut line = Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                health_dot(t, c.health),
                Span::raw(" "),
                Span::styled(pad(&truncate(&c.name, name_w), name_w + 2), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
                Span::styled(pad(&truncate(&c.image, image_w), image_w + 2), Style::new().fg(t.muted)),
                Span::styled(pad(&truncate(&short_status(&c.status), status_w), status_w + 2), Style::new().fg(t.fg)),
                Span::styled(pad(&truncate(&ports(c), ports_w), ports_w + 2), Style::new().fg(t.highlight)),
                Span::styled(c.compose.as_ref().map(|p| p.project.clone()).unwrap_or_default(), Style::new().fg(t.muted)),
            ]);
            if selected {
                line = line.style(Style::new().bg(t.border));
            }
            line
        })
        .collect();

    let h = inner.height as usize;
    let sel = if focused { s.docker_sel } else { 0 };
    let offset = (sel + 1).saturating_sub(h);
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn health_dot(t: &Theme, h: Health) -> Span<'static> {
    let color = match h {
        Health::Healthy => t.ok,
        Health::None => t.accent,
        Health::Starting => t.warn,
        Health::Unhealthy => t.alert,
    };
    Span::styled("●", Style::new().fg(color))
}

/// "Up 4 weeks (healthy)" → "Up 4 weeks": a saúde já está na cor da bolinha.
fn short_status(s: &str) -> String {
    s.split(" (").next().unwrap_or(s).to_string()
}

fn ports(c: &Container) -> String {
    c.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
}
