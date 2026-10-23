//! Chats do Claude Code embutidos no cockpit. Cada chat é um `claude` num pseudo-terminal:
//! a saída passa por um emulador (vt100), que o painel Chat desenha, e as teclas voltam
//! como os bytes que um terminal mandaria.
//!
//! O `AppState` guarda só a tela de cada chat (`ChatView`); os processos e os PTYs ficam
//! no `Hub`, dono do loop principal, para o `update` continuar sem I/O.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{Notify, mpsc};

use crate::app::update::Action;

/// Linhas guardadas acima da tela, para rolar com Shift+PgUp ou a roda do mouse. Só vale
/// na tela normal: programas em tela cheia (o `claude` com `tui = "fullscreen"`) usam a
/// tela alternativa, que não tem histórico, e rolam por conta própria.
const SCROLLBACK: usize = 5000;

/// Tela de um chat, compartilhada entre a thread que lê o PTY e o desenho.
#[derive(Clone)]
pub struct ChatView {
    pub id: u64,
    pub title: String,
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// O `claude` saiu (o PTY fechou).
    pub exited: bool,
}

impl std::fmt::Debug for ChatView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatView").field("id", &self.id).field("title", &self.title).field("exited", &self.exited).finish()
    }
}

impl PartialEq for ChatView {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.exited == other.exited
    }
}

/// Pedido do `update` ao `Hub`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCmd {
    /// Abre um `claude` com `args` em `dir`.
    Open { dir: PathBuf, args: Vec<String>, title: String },
    /// Bytes para o PTY (teclas ou texto colado).
    Input(u64, Vec<u8>),
    /// Encerra o processo e esquece o chat.
    Close(u64),
}

struct Proc {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// Dono dos processos. Ao ser descartado (o cockpit fechou), encerra todos.
pub struct Hub {
    procs: HashMap<u64, Proc>,
    next_id: u64,
    /// (linhas, colunas) do painel Chat, por dentro da moldura.
    size: (u16, u16),
    events: mpsc::UnboundedSender<Action>,
    /// Acordado a cada pedaço de saída, para o loop redesenhar.
    pub dirty: Arc<Notify>,
    /// Programa de cada chat (`claude`; os testes trocam por `sh`).
    program: String,
}

impl Hub {
    pub fn new(events: mpsc::UnboundedSender<Action>) -> Self {
        Self { procs: HashMap::new(), next_id: 1, size: (24, 80), events, dirty: Arc::new(Notify::new()), program: "claude".into() }
    }

    /// Executa o pedido; abrir devolve a ação que registra o chat no estado.
    pub fn apply(&mut self, cmd: ChatCmd) -> Option<Action> {
        match cmd {
            ChatCmd::Open { dir, args, title } => Some(match self.open(&dir, &args, &title) {
                Ok(view) => Action::ChatOpened(view),
                Err(e) => Action::Notice(format!("não consegui abrir o chat: {e:#}")),
            }),
            ChatCmd::Input(id, bytes) => {
                if let Some(p) = self.procs.get_mut(&id) {
                    let _ = p.writer.write_all(&bytes).and_then(|()| p.writer.flush());
                }
                None
            }
            ChatCmd::Close(id) => {
                if let Some(mut p) = self.procs.remove(&id) {
                    let _ = p.child.kill();
                    let _ = p.child.wait();
                }
                None
            }
        }
    }

    fn open(&mut self, dir: &Path, args: &[String], title: &str) -> Result<ChatView> {
        let (rows, cols) = self.size;
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("sem pseudo-terminal")?;
        let mut cmd = CommandBuilder::new(&self.program);
        cmd.args(args);
        cmd.cwd(dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).context("o `claude` não executou (está no PATH?)")?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let id = self.next_id;
        self.next_id += 1;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let (p, dirty, events) = (parser.clone(), self.dirty.clone(), self.events.clone());
        std::thread::Builder::new().name(format!("chat-{id}")).spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut p) = p.lock() {
                            p.process(&buf[..n]);
                        }
                        dirty.notify_one();
                    }
                }
            }
            let _ = events.send(Action::ChatExited(id));
        })?;

        self.procs.insert(id, Proc { master: pair.master, writer, child });
        Ok(ChatView { id, title: title.to_string(), parser, exited: false })
    }

    /// Acompanha o tamanho do painel: PTY (o `claude` redesenha) e emulador.
    pub fn resize(&mut self, rows: u16, cols: u16, views: &[ChatView]) {
        if (rows, cols) == self.size || rows < 2 || cols < 10 {
            return;
        }
        self.size = (rows, cols);
        for v in views {
            if let Ok(mut p) = v.parser.lock() {
                p.screen_mut().set_size(rows, cols);
            }
            if let Some(proc) = self.procs.get(&v.id) {
                let _ = proc.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        for p in self.procs.values_mut() {
            let _ = p.child.kill();
        }
    }
}

/// Bytes que um xterm mandaria para a tecla. `app_cursor`: o programa pediu setas no
/// modo aplicação (DECCKM), como fazem os editores.
pub fn encode_key(k: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = k.modifiers;
    let alt = m.contains(KeyModifiers::ALT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    // Parâmetro de modificador do xterm: 1 + shift + 2·alt + 4·ctrl.
    let modp = 1 + u8::from(m.contains(KeyModifiers::SHIFT)) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |end: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{end}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modp > 1 { format!("\x1b[{n};{modp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let with_alt = |mut b: Vec<u8>| {
        if alt {
            b.insert(0, 0x1b);
        }
        b
    };
    let bytes = match k.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                _ => return None,
            };
            with_alt(vec![b])
        }
        KeyCode::Char(c) => with_alt(c.to_string().into_bytes()),
        KeyCode::Enter => with_alt(b"\r".to_vec()),
        KeyCode::Backspace => with_alt(if ctrl { vec![0x08] } else { vec![0x7f] }),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        _ => return None,
    };
    Some(bytes)
}

/// Bytes que um xterm mandaria para o evento de mouse, no modo e na codificação que o
/// programa pediu; `None` quando o modo não reporta esse evento. `ev` já vem com a
/// coluna e a linha relativas à tela do chat.
pub fn encode_mouse(ev: MouseEvent, mode: vt100::MouseProtocolMode, enc: vt100::MouseProtocolEncoding) -> Option<Vec<u8>> {
    use vt100::{MouseProtocolEncoding as Enc, MouseProtocolMode as Mode};
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    // (código do botão, soltou?)
    let (code, release) = match ev.kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) if mode != Mode::Press => (if enc == Enc::Sgr { button(b) } else { 3 }, true),
        MouseEventKind::Drag(b) if matches!(mode, Mode::ButtonMotion | Mode::AnyMotion) => (32 + button(b), false),
        MouseEventKind::Moved if mode == Mode::AnyMotion => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
        _ => return None,
    };
    if mode == Mode::None {
        return None;
    }
    let m = ev.modifiers;
    let code = code
        + 4 * u16::from(m.contains(KeyModifiers::SHIFT))
        + 8 * u16::from(m.contains(KeyModifiers::ALT))
        + 16 * u16::from(m.contains(KeyModifiers::CONTROL));
    let (x, y) = (ev.column + 1, ev.row + 1);
    Some(match enc {
        Enc::Sgr => format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes(),
        Enc::Utf8 => {
            let mut b = b"\x1b[M".to_vec();
            for v in [code, x, y] {
                b.extend(char::from_u32(32 + u32::from(v)).unwrap_or(' ').to_string().into_bytes());
            }
            b
        }
        // Um byte por valor: além da coluna 223 não cabe.
        Enc::Default => [27, b'[', b'M', (32 + code).min(255) as u8, (32 + x).min(255) as u8, (32 + y).min(255) as u8].to_vec(),
    })
}

/// Texto colado: entre os marcadores de bracketed paste quando o programa pediu, para
/// as quebras de linha não virarem Enter.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{text}\x1b[201~").into_bytes() } else { text.into_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn encodes_keys_like_xterm() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode_key(k(KeyCode::Char('a'), none), false), Some(b"a".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Char('ç'), none), false), Some("ç".as_bytes().to_vec()));
        assert_eq!(encode_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), false), Some(vec![3]));
        assert_eq!(encode_key(k(KeyCode::Char('b'), KeyModifiers::ALT), false), Some(b"\x1bb".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Enter, none), false), Some(b"\r".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Backspace, none), false), Some(vec![0x7f]));
        assert_eq!(encode_key(k(KeyCode::BackTab, KeyModifiers::SHIFT), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Up, none), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Up, none), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Left, KeyModifiers::CONTROL), true), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(encode_key(k(KeyCode::Delete, none), false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode_key(k(KeyCode::F(1), none), false), Some(b"\x1bOP".to_vec()));
        assert_eq!(encode_key(k(KeyCode::F(5), none), false), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn runs_a_program_in_a_pty_and_reports_exit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut hub = Hub::new(tx);
        hub.program = "sh".into();
        let open = ChatCmd::Open { dir: std::env::temp_dir(), args: vec!["-c".into(), "read x; echo \"oi $x\"".into()], title: "t".into() };
        let Some(Action::ChatOpened(view)) = hub.apply(open) else { panic!("não abriu") };
        hub.apply(ChatCmd::Input(view.id, b"mundo\r".to_vec()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(Action::ChatExited(id)) = rx.try_recv() {
                assert_eq!(id, view.id);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "o sh não saiu");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let text = view.parser.lock().unwrap().screen().contents();
        assert!(text.contains("oi mundo"), "{text:?}");
    }

    #[test]
    fn encodes_mouse_like_xterm() {
        use vt100::{MouseProtocolEncoding as Enc, MouseProtocolMode as Mode};
        let ev = |kind| MouseEvent { kind, column: 4, row: 9, modifiers: KeyModifiers::NONE };
        let wheel = ev(MouseEventKind::ScrollUp);
        assert_eq!(encode_mouse(wheel, Mode::PressRelease, Enc::Sgr), Some(b"\x1b[<64;5;10M".to_vec()));
        assert_eq!(encode_mouse(wheel, Mode::PressRelease, Enc::Default), Some(vec![27, b'[', b'M', 96, 37, 42]));
        assert_eq!(encode_mouse(wheel, Mode::None, Enc::Sgr), None, "sem modo de mouse, nada vai");
        let up = ev(MouseEventKind::Up(MouseButton::Left));
        assert_eq!(encode_mouse(up, Mode::PressRelease, Enc::Sgr), Some(b"\x1b[<0;5;10m".to_vec()));
        assert_eq!(encode_mouse(up, Mode::Press, Enc::Sgr), None);
        let drag = ev(MouseEventKind::Drag(MouseButton::Left));
        assert_eq!(encode_mouse(drag, Mode::ButtonMotion, Enc::Sgr), Some(b"\x1b[<32;5;10M".to_vec()));
        assert_eq!(encode_mouse(drag, Mode::PressRelease, Enc::Sgr), None);
    }

    #[test]
    fn paste_is_bracketed_when_asked() {
        assert_eq!(encode_paste("a\nb", true), b"\x1b[200~a\rb\x1b[201~".to_vec());
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb".to_vec());
    }
}
