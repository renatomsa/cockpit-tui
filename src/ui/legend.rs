//! Legenda (tecla `i`): o que significa cada símbolo do painel em foco, desenhado com o
//! mesmo estilo que ele tem no painel.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use super::{system::level_style, worktrees::status_icon};
use crate::{
    app::state::{AppState, Panel},
    danger::Level,
    providers::claude::Status,
    theme::Theme,
};

/// (símbolo como aparece no painel, explicação)
type Entry = (Span<'static>, &'static str);

fn st(text: &'static str, style: Style) -> Span<'static> {
    Span::styled(text, style)
}

fn sessions(t: &Theme) -> Vec<Entry> {
    vec![
        (status_icon(t, Status::Waiting), "esperando você: permissão, pergunta ou diálogo aberto"),
        (status_icon(t, Status::Busy), "trabalhando"),
        (status_icon(t, Status::Shell), "rodando um comando de shell"),
        (status_icon(t, Status::Idle), "parada, esperando um novo pedido"),
    ]
}

fn entries(panel: Panel, t: &Theme, blink: bool) -> (&'static str, Vec<Entry>) {
    let muted = Style::new().fg(t.muted);
    let fg = Style::new().fg(t.fg);
    match panel {
        Panel::Hero => (
            "Relógio",
            vec![
                (st(" Recife 16:15 ", Style::new().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)), "cidade atual (pelo IP ou pelo config) e a hora local"),
                (st("Orlando 15:15", fg), "relógios extras, de [[clock.extra]] no config"),
                (st("UTC-03:00", fg), "fuso da cidade atual"),
                (st("geo ipinfo.io", muted), "(barra de status) de onde veio a cidade: config, cache ou serviço de IP"),
            ],
        ),
        Panel::Agenda => (
            "Agenda",
            vec![(st("—", muted), "o Google Calendar chega na F3: próximas reuniões e o alerta vermelho antes delas")],
        ),
        Panel::Worktrees => {
            let mut v = vec![
                (st("▸", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "worktree selecionada"),
                (st("├ └", Style::new().fg(t.border)), "worktrees do repositório acima"),
                (st("(principal)", fg), "a pasta principal do repositório"),
                (st("feat/x", Style::new().fg(t.highlight)), "branch em que a worktree está"),
                (st("✎ 4", Style::new().fg(t.warn)), "arquivos alterados ou novos, ainda sem commit"),
                (st("✓", Style::new().fg(t.ok)), "nada pendente"),
                (st("↑2", Style::new().fg(t.accent)), "commits seus que ainda não subiram"),
                (st("↓5", Style::new().fg(t.accent)), "commits do remoto que você ainda não puxou"),
                (st("22h", muted), "idade do último commit (s, m, h, d; mo = meses; a = anos)"),
                (st("3.1G", muted), "espaço em disco (… = ainda calculando)"),
                (st("sumiu", Style::new().fg(t.alert)), "a pasta foi apagada; `git worktree prune` limpa a lista"),
            ];
            v.extend(sessions(t));
            ("Worktrees", v)
        }
        Panel::System => (
            "Sistema",
            vec![
                (st("━━━━", Style::new().fg(t.gauge_low)), "uso abaixo de 60%"),
                (st("━━━━", Style::new().fg(t.gauge_mid)), "entre 60% e 85%"),
                (st("━━━━", Style::new().fg(t.gauge_high)), "acima de 85%"),
                (st("MEM  93%", level_style(t, Level::Warn, blink)), "aviso: perto do limite configurado"),
                (st("MEM  96%", level_style(t, Level::Critical, blink)), "crítico, piscando: risco de travar; os maiores ofensores piscam junto"),
                (st("load 1.2 0.9", muted), "processos na fila: média de 1, 5 e 15 min (mais que o nº de núcleos = fila)"),
                (st("PSI", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "% do tempo em que algo esperou CPU, memória ou disco (10 s); mem alta = travamento chegando"),
                (st("↓ ↑", Style::new().fg(t.spark)), "download e upload agora"),
                (st("±4", muted), "variação do ping (jitter), em ms"),
                (st("perda 0%", muted), "tentativas sem resposta entre as últimas 30"),
                (st("×10", fg), "processos com o mesmo nome, somados"),
                (st("107%", fg), "CPU em % de um núcleo (passa de 100 quando usa vários)"),
                (st("MÁQUINA", muted), "especificações; no WSL, da máquina real, com o que o WSL recebe entre parênteses"),
                (st("(WSL usa 6)", muted), "núcleos ou memória que a VM do WSL enxerga (ajuste em .wslconfig)"),
            ],
        ),
        Panel::Sessions => {
            let mut v = sessions(t);
            v.extend([
                (status_icon(t, Status::Ended), "encerrada há pouco; `r` retoma numa aba nova"),
                (st("bg", muted), "conversa em segundo plano (claude --bg)"),
                (st("›", muted), "último pedido que você fez"),
                (st("v360/cfs", muted), "repositório/worktree em que a conversa está"),
                (st("há 8m", muted), "desde a última mudança de status"),
                (st("Sessão 5h ━━", Style::new().fg(t.gauge_low)), "quanto do limite de 5 h do plano já foi usado (o mesmo do /usage)"),
                (st("Semana ━━", Style::new().fg(t.gauge_low)), "quanto do limite semanal já foi usado; ao lado, quando reinicia"),
                (st("⟳ rodando", Style::new().fg(t.accent)), "tokens da rodada em andamento, desde o seu último pedido"),
                (st("última rodada", muted), "tokens da rodada que terminou"),
                (st("3.2k saída", Style::new().fg(t.spark)), "tokens gerados pelo Claude (texto, código e raciocínio)"),
                (st("1.4M processados", fg), "tudo que passou pelas chamadas da rodada, incluindo cache lido"),
                (st("contexto 145k", fg), "tamanho da conversa enviada na última chamada"),
                (st("2 subagentes", muted), "subagentes disparados na rodada (o gasto deles entra na soma)"),
            ]);
            ("Claude", v)
        }
    }
}

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let (title, list) = entries(s.focus, t, s.blink_on());
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
