//! Legenda (tecla `i`): o que significa cada símbolo do painel em foco, desenhado com o
//! mesmo estilo que ele tem no painel.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use super::{
    system::level_style,
    worktrees::{status_icon, status_style},
};
use crate::{
    app::state::{AppState, Panel},
    danger::Level,
    providers::claude::Status,
    theme::Theme,
    wt_status::WtStatus,
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
        Panel::Docker => (
            "Docker",
            vec![
                (st("●", Style::new().fg(t.ok)), "rodando e saudável (healthcheck ok)"),
                (st("●", Style::new().fg(t.accent)), "rodando, sem healthcheck"),
                (st("●", Style::new().fg(t.warn)), "healthcheck ainda iniciando"),
                (st("●", Style::new().fg(t.alert)), "rodando, mas o healthcheck falha"),
                (st("5432,6379", Style::new().fg(t.highlight)), "portas publicadas no host"),
                (st("v360", muted), "projeto do docker compose (vazio = container solto)"),
                (st("Up 4 weeks", muted), "há quanto tempo está de pé, como no docker ps"),
                (st("K", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "para o container selecionado (docker stop, pede confirmação)"),
                (st("X", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "derruba tudo: compose down --remove-orphans em cada projeto e stop nos soltos"),
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
        Panel::Worktrees => {
            let mut v = vec![
                (st("▸", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "worktree selecionada"),
                (st("├ └", Style::new().fg(t.border)), "worktrees do repositório acima"),
                (st("(principal)", fg), "a pasta principal do repositório"),
                (st("feat/x", Style::new().fg(t.highlight)), "branch em que a worktree está"),
                (st("wip", status_style(t, WtStatus::Wip)), "status da melhoria (`s` escolhe; `cockpit status` no terminal)"),
                (st("shape", status_style(t, WtStatus::Shape)), "desenhando a solução, antes do código"),
                (st("revisão", status_style(t, WtStatus::RevisaoManual)), "revisão manual: esperando alguém testar ou revisar"),
                (st("deploy", status_style(t, WtStatus::Deploy)), "indo para produção"),
                (st("finalizado", status_style(t, WtStatus::Finalizado)), "entregue"),
                (st("✎ 4", Style::new().fg(t.warn)), "arquivos alterados ou novos, ainda sem commit"),
                (st("✓", Style::new().fg(t.ok)), "nada pendente"),
                (st("↑2", Style::new().fg(t.accent)), "commits seus que ainda não subiram"),
                (st("↓5", Style::new().fg(t.accent)), "commits do remoto que você ainda não puxou"),
                (st("22h", muted), "idade do último commit (s, m, h, d; mo = meses; a = anos)"),
                (st("3.1G", muted), "espaço em disco (… = ainda calculando)"),
                (st("sumiu", Style::new().fg(t.alert)), "a pasta foi apagada; `git worktree prune` limpa a lista"),
                (st("D", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "exclui a worktree (y no 1º aviso, Enter no 2º); a branch continua"),
                (st("← →", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "troca para a aba Claude"),
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
                (st("MEM  93%", level_style(t, Level::Warn)), "aviso: perto do limite configurado"),
                (st("MEM  96%", level_style(t, Level::Critical)), "crítico, em vermelho negrito: risco de travar; os maiores ofensores também ficam vermelhos"),
                (st("load 1.2 0.9", muted), "processos na fila: média de 1, 5 e 15 min (mais que o nº de núcleos = fila)"),
                (st("PSI", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "% do tempo em que algo esperou CPU, memória ou disco (10 s); mem alta = travamento chegando"),
                (st("↓ ↑", Style::new().fg(t.spark)), "download e upload agora (no WSL, do Windows inteiro; o do WSL vem ao lado)"),
                (st("sinal ██ 85%", Style::new().fg(t.ok)), "força do Wi-Fi do Windows, com canal, faixa e velocidade do link"),
                (st("s", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), "teste de velocidade: baixa e envia alguns MB para a Cloudflare"),
                (st("±4", muted), "variação do ping (jitter), em ms"),
                (st("perda 0%", muted), "tentativas sem resposta entre as últimas 30"),
                (st("×10", fg), "processos com o mesmo nome, somados"),
                (st("107%", fg), "CPU em % de um núcleo (passa de 100 quando usa vários)"),
                (st("Sessão 5h ━━", Style::new().fg(t.gauge_low)), "limites do Claude: quanto do limite de 5 h do plano já foi usado (o mesmo do /usage)"),
                (st("Semana ━━", Style::new().fg(t.gauge_low)), "quanto do limite semanal já foi usado; ao lado, quando reinicia"),
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
                (st(" a permitir desta vez ", Style::new().fg(t.bg).bg(t.warn).add_modifier(Modifier::BOLD)), "pedido de permissão (Bash, Edit…): `a` aprova só esta vez; o terminal continua valendo"),
                (st("Claude Desktop", fg), "aba ao lado: sessões de Code e Cowork do app do Windows (só leitura)"),
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
    let (title, list) = entries(s.active_panel(), t);
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
