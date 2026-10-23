//! Legenda (tecla `i`): o que significa cada símbolo do painel em foco, desenhado com o
//! mesmo estilo que ele tem no painel.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use super::system::level_style;
use crate::{
    app::state::{AppState, Panel},
    danger::Level,
    theme::Theme,
};

/// (símbolo como aparece no painel, explicação)
type Entry = (Span<'static>, &'static str);

fn st(text: &'static str, style: Style) -> Span<'static> {
    Span::styled(text, style)
}

fn entries(panel: Panel, t: &Theme) -> (&'static str, Vec<Entry>) {
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    match panel {
        Panel::Hero => (
            "Relógio",
            vec![
                (st(" Recife 16:15 ", Style::new().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)), "cidade atual (pelo IP ou pelo config) e a hora local"),
                (st("Orlando 15:15", fg), "relógios extras, de [[clock.extra]] no config"),
                (st("UTC-03:00", fg), "fuso da cidade atual"),
                (st("☀ / ☾", Style::new().fg(t.warn)), "dia ou noite na cidade, pelo nascer e pôr do sol de hoje"),
                (st("geo ipinfo.io", muted), "(barra de status) de onde veio a cidade: config, cache ou serviço de IP"),
                (st("▶ 16:00 Daily · em 4 min", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)), "próxima reunião; amarelo quando falta pouco"),
                (st("Garoa 24°C", fg), "clima agora (Open-Meteo); ao lado, a hora com mais chance de chuva"),
                (st("1h 24° 87%", muted), "próximas horas: temperatura e chance de chuva"),
                (st(" Recife ", Style::new().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)), "no mapa: onde você está (pelo IP ou pelo config)"),
                (st(" tela vermelha ", Style::new().fg(t.alert_fg).bg(t.alert_bg).add_modifier(Modifier::BOLD)), "reunião começando: qualquer tecla para, `z` adia, `o` abre a chamada"),
            ],
        ),
        Panel::Chat => (
            "Chat",
            vec![
                (st(" 1 v360 ", Style::new().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)), "chat selecionado; o número troca direto (Alt+número enquanto digita)"),
                (st(" 2 api ✕ ", Style::new().fg(t.muted).add_modifier(Modifier::CROSSED_OUT)), "o claude desse chat encerrou; x fecha a aba"),
                (st("borda", Style::new().fg(t.highlight)), "digitando: as teclas vão para o claude, até o Ctrl+Q"),
                (st("Shift+PgUp", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "rola o chat (ou a roda do mouse); digitar volta ao fim"),
                (st(" ● gravando 0:07 ", Style::new().fg(t.bg).bg(t.alert).add_modifier(Modifier::BOLD)), "ditado: `v` (Alt+V digitando) para e transcreve; o texto entra no chat sem Enter"),
            ],
        ),
        Panel::Agenda => (
            "Agenda",
            vec![
                (st("hoje", Style::new().fg(t.accent)), "dia da reunião (hoje, amanhã, depois o dia da semana)"),
                (st("agora · até 16:30", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "reunião em andamento"),
                (st("em 12 min", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)), "começa em breve (dentro de soon_minutes)"),
                (st("Meet", Style::new().fg(t.highlight)), "tem link de chamada (Meet, Zoom, Teams); `o` abre, `y` copia"),
                (st("dia todo", muted), "evento de dia inteiro: nunca dispara alerta"),
                (st(" ▶ 16:00 Daily ", Style::new().fg(t.alert_fg).bg(t.alert_bg).add_modifier(Modifier::BOLD)), "no relógio: vermelho quando faltam até 5 min (ou começou há até 5)"),
                (st("z", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "no alerta vermelho: adia 5 min; qualquer outra tecla para; `o` abre a chamada"),
            ],
        ),
        Panel::System => (
            "Sistema",
            vec![
                (st("━━━━", Style::new().fg(t.gauge_low)), "uso abaixo de 60%"),
                (st("━━━━", Style::new().fg(t.gauge_mid)), "entre 60% e 85%"),
                (st("━━━━", Style::new().fg(t.gauge_high)), "acima de 85%"),
                (st("MEM  93%", level_style(t, Level::Warn)), "aviso: perto do limite configurado"),
                (st("MEM  96%", level_style(t, Level::Critical)), "crítico, em vermelho negrito: risco de travar; os maiores ofensores também ficam vermelhos"),
                (st("load 1.2 0.9", muted), "processos na fila: média de 1, 5 e 15 min (mais que o nº de núcleos = fila)"),
                (st("×10", fg), "processos com o mesmo nome, somados"),
                (st("107%", fg), "CPU em % de um núcleo (passa de 100 quando usa vários)"),
                (st("Sessão 5h ━━", Style::new().fg(t.gauge_low)), "limites do Claude: quanto do limite de 5 h do plano já foi usado (o mesmo do /usage)"),
                (st("Semana ━━", Style::new().fg(t.gauge_low)), "quanto do limite semanal já foi usado; ao lado, quando reinicia"),
            ],
        ),
    }
}

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let (title, list) = entries(s.focus, t);
    let sym_w = list.iter().map(|(sp, _)| sp.content.chars().count()).max().unwrap_or(4) + 3;
    let mut lines: Vec<Line> = list
        .into_iter()
        .map(|(sym, desc)| {
            let gap = sym_w.saturating_sub(sym.content.chars().count());
            Line::from(vec![Span::raw(" "), sym, Span::raw(" ".repeat(gap)), Span::styled(desc, Style::new().fg(t.fg))])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(" qualquer tecla fecha", Style::new().fg(t.muted)));

    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(40) as u16;
    let w = (content_w + 4).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(t.accent))
                .title(Span::styled(format!(" legenda · {title} "), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
                .style(Style::new().bg(t.bg)),
        ),
        r,
    );
}
