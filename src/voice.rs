//! Ditado: grava o microfone num WAV com um gravador externo (`pw-record`, `arecord`…) e
//! passa o arquivo para um transcritor (whisper.cpp, por padrão), que imprime o texto.
//!
//! O estado (gravando, transcrevendo) fica no `AppState`; o processo do gravador fica
//! aqui, no `Voice`, dono do loop principal, como os chats ficam no `Hub`.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::{app::update::Action, config::VoiceConfig};

/// Pedido do `update` ao `Voice`. O número é o da gravação (`AppState::voice_take`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceCmd {
    Start(u64),
    /// Para de gravar e transcreve.
    Stop,
    /// Descarta a gravação.
    Cancel,
}

/// Gravadores conhecidos, na ordem de preferência, gravando WAV 16 kHz mono (o que o
/// whisper espera).
const RECORDERS: [&[&str]; 4] = [
    &["pw-record", "--rate", "16000", "--channels", "1", "--format", "s16", "{wav}"],
    &["parecord", "--rate=16000", "--channels=1", "--format=s16le", "--file-format=wav", "{wav}"],
    &["arecord", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "wav", "{wav}"],
    &["rec", "-q", "-r", "16000", "-c", "1", "-b", "16", "{wav}"],
];

/// Frases que o whisper "ouve" no silêncio (vícios das legendas com que foi treinado).
const GHOSTS: [&str; 5] = ["legendas pela comunidade amara.org", "obrigado.", "obrigada.", "tchau.", "..."];

struct Recording {
    take: u64,
    child: Child,
    wav: PathBuf,
}

/// Dono do processo de gravação. Ao ser descartado, encerra a gravação e apaga o áudio.
pub struct Voice {
    rec: Option<Recording>,
    events: mpsc::UnboundedSender<Action>,
}

impl Voice {
    pub fn new(events: mpsc::UnboundedSender<Action>) -> Self {
        Self { rec: None, events }
    }

    /// Executa o pedido; uma falha imediata (gravador ausente) volta como ação.
    pub fn apply(&mut self, cmd: VoiceCmd, cfg: &VoiceConfig) -> Option<Action> {
        match cmd {
            VoiceCmd::Start(take) => {
                self.discard();
                match start(take, cfg) {
                    Ok(rec) => {
                        self.rec = Some(rec);
                        None
                    }
                    Err(e) => Some(Action::VoiceText { take, text: Err(e) }),
                }
            }
            VoiceCmd::Stop => {
                let rec = self.rec.take()?;
                let (events, transcribe) = (self.events.clone(), cfg.transcribe.clone());
                std::thread::Builder::new()
                    .name(format!("voice-{}", rec.take))
                    .spawn(move || {
                        let take = rec.take;
                        let text = finish(rec, &transcribe);
                        let _ = events.send(Action::VoiceText { take, text });
                    })
                    .ok();
                None
            }
            VoiceCmd::Cancel => {
                self.discard();
                None
            }
        }
    }

    fn discard(&mut self) {
        if let Some(mut r) = self.rec.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
            let _ = std::fs::remove_file(&r.wav);
        }
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.discard();
    }
}

fn which(bin: &str) -> bool {
    Path::new(bin).is_file() || std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Comando de gravação: o do config, ou o primeiro gravador conhecido instalado.
pub fn recorder(cfg: &VoiceConfig) -> Option<Vec<String>> {
    if !cfg.record.is_empty() {
        return Some(cfg.record.clone());
    }
    RECORDERS.iter().find(|r| which(r[0])).map(|r| r.iter().map(|a| a.to_string()).collect())
}

/// Contexto de áudio do whisper (quadros de 20 ms) do tamanho da gravação, com folga.
/// O padrão (1500) processa sempre 30 s: numa frase curta, encolher corta o tempo pela
/// metade. Menor que o áudio, o whisper corta o fim ou repete frases, daí a folga.
pub fn audio_ctx(wav_bytes: u64) -> u32 {
    // WAV 16 kHz, mono, 16 bits: 32000 bytes por segundo, depois do cabeçalho de 44.
    let secs = wav_bytes.saturating_sub(44) as f64 / 32000.0;
    (((secs + 3.0) * 50.0).ceil() as u32).clamp(384, 1500)
}

/// Troca `{wav}` pelo arquivo (ou acrescenta no fim, se o comando não usa) e
/// `{audio_ctx}` pelo contexto do tamanho dele; expande `~`.
pub fn fill(cmd: &[String], wav: &Path) -> Vec<String> {
    let ctx = audio_ctx(std::fs::metadata(wav).map(|m| m.len()).unwrap_or(u64::MAX)).to_string();
    let wav = wav.display().to_string();
    let mut out: Vec<String> = cmd
        .iter()
        .map(|a| if a.starts_with("~/") { crate::config::expand_home(a).display().to_string() } else { a.clone() })
        .map(|a| a.replace("{wav}", &wav).replace("{audio_ctx}", &ctx))
        .collect();
    if !cmd.iter().any(|a| a.contains("{wav}")) {
        out.push(wav);
    }
    out
}

fn start(take: u64, cfg: &VoiceConfig) -> Result<Recording, String> {
    let cmd = recorder(cfg).ok_or("nenhum gravador encontrado (instale pw-record, arecord ou sox)")?;
    let wav = std::env::temp_dir().join(format!("cockpit-voz-{}-{take}.wav", std::process::id()));
    let args = fill(&cmd, &wav);
    // Nada do gravador pode ir para o terminal: ele está desenhando o cockpit.
    let child = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("o gravador `{}` não executou: {e}", args[0]))?;
    Ok(Recording { take, child, wav })
}

/// Para o gravador com SIGINT (ele fecha o WAV direito), espera e transcreve.
fn finish(mut rec: Recording, transcribe: &[String]) -> Result<String, String> {
    let text = (|| {
        let still_running = matches!(rec.child.try_wait(), Ok(None));
        if still_running {
            let _ = Command::new("kill").args(["-INT", &rec.child.id().to_string()]).status();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while matches!(rec.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = rec.child.kill();
        let _ = rec.child.wait();
        let size = std::fs::metadata(&rec.wav).map(|m| m.len()).unwrap_or(0);
        // Só o cabeçalho (44 bytes) ou quase: o gravador não pegou áudio.
        if size < 1024 {
            let mut err = String::new();
            if let Some(mut e) = rec.child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            return Err(match last_line(&err) {
                Some(l) => format!("o gravador não gravou: {l}"),
                None => "o gravador não gravou nada (microfone?)".into(),
            });
        }
        run_transcriber(transcribe, &rec.wav)
    })();
    let _ = std::fs::remove_file(&rec.wav);
    text
}

fn run_transcriber(cmd: &[String], wav: &Path) -> Result<String, String> {
    if cmd.is_empty() {
        return Err("[voice].transcribe está vazio".into());
    }
    let args = fill(cmd, wav);
    let out = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("o transcritor `{}` não executou: {e}", args[0]))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("o transcritor falhou: {}", last_line(&err).unwrap_or("sem mensagem")));
    }
    Ok(clean(&String::from_utf8_lossy(&out.stdout)))
}

fn last_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).rfind(|l| !l.is_empty())
}

/// Texto pronto para o chat: uma linha só (cada quebra viraria Enter), sem as marcas
/// do whisper (`[BLANK_AUDIO]`, `[Música]`) e sem as frases fantasma do silêncio.
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth = 0usize;
    for ch in raw.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            c => out.push(c),
        }
    }
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if GHOSTS.contains(&text.to_lowercase().as_str()) { String::new() } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn fill_puts_the_wav_in_place_or_at_the_end() {
        let wav = Path::new("/tmp/a.wav");
        assert_eq!(fill(&v(&["w", "-f", "{wav}", "-l", "pt"]), wav), v(&["w", "-f", "/tmp/a.wav", "-l", "pt"]));
        assert_eq!(fill(&v(&["w", "-l", "pt"]), wav), v(&["w", "-l", "pt", "/tmp/a.wav"]));
        let home = crate::config::expand_home("~/m.bin").display().to_string();
        assert_eq!(fill(&v(&["w", "-m", "~/m.bin", "{wav}"]), wav), v(&["w", "-m", &home, "/tmp/a.wav"]));
    }

    #[test]
    fn audio_ctx_follows_the_recording_length() {
        assert_eq!(audio_ctx(44), 384, "piso para gravação curtíssima");
        assert_eq!(audio_ctx(44 + 10 * 32000), 650, "10 s + 3 s de folga");
        assert_eq!(audio_ctx(44 + 60 * 32000), 1500, "teto: a janela inteira do whisper");
        assert_eq!(audio_ctx(u64::MAX), 1500);
    }

    #[test]
    fn clean_joins_lines_and_drops_whisper_marks() {
        assert_eq!(clean("\n Olá, tudo bem?\n Vamos rodar os testes.\n"), "Olá, tudo bem? Vamos rodar os testes.");
        assert_eq!(clean("[BLANK_AUDIO]\n"), "");
        assert_eq!(clean(" [Música] abre o arquivo [risos]"), "abre o arquivo");
        assert_eq!(clean(" Legendas pela comunidade Amara.org\n"), "");
        assert_eq!(clean("Obrigado pela ajuda."), "Obrigado pela ajuda.");
    }

    fn wait_text(rx: &mut mpsc::UnboundedReceiver<Action>) -> (u64, Result<String, String>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Action::VoiceText { take, text }) = rx.try_recv() {
                return (take, text);
            }
            assert!(Instant::now() < deadline, "o ditado não terminou");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn records_then_transcribes_with_external_commands() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut voice = Voice::new(tx);
        // "gravador" que escreve 2 KB e espera o SIGINT; "transcritor" que lê o tamanho
        let cfg = VoiceConfig {
            record: v(&["sh", "-c", "head -c 2048 /dev/zero > \"$0\"; exec sleep 10", "{wav}"]),
            transcribe: v(&["sh", "-c", "printf ' [BLANK_AUDIO]\\n %s bytes\\n' $(wc -c < \"$0\")", "{wav}"]),
            max_seconds: 60,
        };
        assert!(voice.apply(VoiceCmd::Start(4), &cfg).is_none());
        std::thread::sleep(Duration::from_millis(200));
        assert!(voice.apply(VoiceCmd::Stop, &cfg).is_none());
        assert_eq!(wait_text(&mut rx), (4, Ok("2048 bytes".into())));
    }

    #[test]
    fn empty_recording_and_missing_recorder_are_errors() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut voice = Voice::new(tx);
        let cfg = VoiceConfig { record: v(&["sh", "-c", "echo sem microfone >&2", "{wav}"]), transcribe: v(&["true"]), max_seconds: 60 };
        voice.apply(VoiceCmd::Start(1), &cfg);
        std::thread::sleep(Duration::from_millis(100));
        voice.apply(VoiceCmd::Stop, &cfg);
        let (_, text) = wait_text(&mut rx);
        assert!(text.unwrap_err().contains("sem microfone"));

        let cfg = VoiceConfig { record: v(&["/nao/existe/gravador"]), ..cfg };
        assert!(matches!(voice.apply(VoiceCmd::Start(2), &cfg), Some(Action::VoiceText { take: 2, text: Err(_) })));
    }
}
