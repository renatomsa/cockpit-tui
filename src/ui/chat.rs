//! Painel Chat: abas com os chats embutidos e a tela do selecionado, como num terminal.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use tui_term::widget::{Cursor, PseudoTerminal};

use super::truncate;
use crate::app::state::{AppState, Panel};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState) {
    let t = s.theme();
    let focused = s.focus == Panel::Chat;
    let typing = focused && s.chat_typing;
    let border = if typing { t.highlight } else if focused { t.accent } else { t.border };

    let mut title = vec![Span::styled(" Chat ", if focused { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.fg) })];
    for (i, c) in s.chats.iter().enumerate() {
        let active = i == s.chat_sel;
        let label = format!(" {} {}{} ", i + 1, truncate(&c.title, 18), if c.exited { " ✕" } else { "" });
        let style = match (active, c.exited) {
            (true, _) => Style::new().fg(t.bg).bg(if typing { t.highlight } else { t.accent }).add_modifier(Modifier::BOLD),
            (false, true) => Style::new().fg(t.muted).add_modifier(Modifier::CROSSED_OUT),
            (false, false) => Style::new().fg(t.fg),
        };
        title.push(Span::styled(label, style));
        title.push(Span::raw(" "));
    }
    let hint = match (focused, typing) {
        (true, true) => " digitando · Alt+V dita · Ctrl+Q volta ao cockpit ",
        (true, false) if s.chats.is_empty() => " n ou Enter abre um chat ",
        (true, false) => " Enter digita · v dita · n novo · ←→ troca · x fecha ",
        (false, _) => "",
    };
    // Ditando, o aviso fica no lugar da dica, mesmo sem foco: o texto vem para cá.
    let hint_style = Style::new().fg(if typing { t.highlight } else { t.muted });
    let hint = match super::voice_label(s) {
        Some(label) => Span::styled(format!(" {label} "), Style::new().fg(t.bg).bg(t.alert).add_modifier(Modifier::BOLD)),
        None => Span::styled(hint, hint_style),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(Line::from(title))
        .title_top(Line::from(hint).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(chat) = s.selected_chat() else {
        let lines = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled("n", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" abre um chat do Claude Code: escolha a pasta (digite para filtrar) e Enter", Style::new().fg(t.muted)),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+Q", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" sai do chat e devolve as teclas ao cockpit (Tab, q…)", Style::new().fg(t.muted)),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    };

    let Ok(parser) = chat.parser.lock() else { return };
    let term = PseudoTerminal::new(parser.screen()).cursor(Cursor::default().visibility(typing).style(Style::new().fg(t.fg)));
    f.render_widget(term, inner);
    drop(parser);
    // Cor "padrão" do programa vira a cor do tema, para o chat não destoar do resto.
    let buf = f.buffer_mut();
    for y in inner.top()..inner.bottom() {
        for x in inner.left()..inner.right() {
            let cell = &mut buf[(x, y)];
            if cell.fg == Color::Reset {
                cell.fg = t.fg;
            }
            if cell.bg == Color::Reset {
                cell.bg = t.bg;
            }
        }
    }
    if chat.exited && inner.height > 0 {
        let msg = " o claude encerrou · x fecha a aba · n abre outro chat ";
        let w = (msg.chars().count() as u16).min(inner.width);
        let r = Rect { x: inner.x + (inner.width - w) / 2, y: inner.bottom() - 1, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(msg, Style::new().fg(t.bg).bg(t.warn).add_modifier(Modifier::BOLD))), r);
    }
}
