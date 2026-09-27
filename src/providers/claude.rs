//! Conversas do Claude Code. Lê `~/.claude/sessions/<pid>.json` (um arquivo por sessão
//! aberta) e enriquece com o fim da transcrição em `~/.claude/projects/*/<id>.jsonl`.
//!
//! Esses formatos são internos e sem documentação: todo campo é opcional e, se algo
//! mudar, a sessão aparece só com pasta e status em vez de sumir.

use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, TimeZone, Utc};
use notify::{RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};

use super::Snapshot;
use crate::config::{ClaudeConfig, expand_home};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionsSnapshot {
    /// Processos vivos, na ordem de atenção: esperando você, trabalhando, parados.
    pub live: Vec<Session>,
    /// Encerradas há pouco (dá para retomar com `r`), da mais recente para a mais antiga.
    pub recent: Vec<Session>,
}

impl SessionsSnapshot {
    pub fn count(&self, st: Status) -> usize {
        self.live.iter().filter(|s| s.status == st).count()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Esperando o usuário (permissão, pergunta, diálogo).
    Waiting,
    Busy,
    /// Rodando um comando de shell.
    Shell,
    #[default]
    Idle,
    Ended,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub pid: u32,
    pub id: String,
    pub background: bool,
    pub status: Status,
    /// Motivo da espera, quando o Claude informa (ex.: "dialog open").
    pub waiting_for: Option<String>,
    pub title: String,
    /// Pasta atual (a transcrição vence o arquivo de sessão: o Claude pode ter entrado numa worktree).
    pub cwd: PathBuf,
    /// Recapitulação do Claude ("Seu objetivo era…") ou o último pedido.
    pub summary: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SessionFile {
    pid: Option<u32>,
    session_id: Option<String>,
    cwd: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    name: Option<String>,
    job_id: Option<String>,
    proc_start: Option<String>,
    updated_at: Option<i64>,
    status_updated_at: Option<i64>,
    started_at: Option<i64>,
    waiting_for: Option<String>,
}

pub fn claude_dir(cfg: &ClaudeConfig) -> PathBuf {
    if let Some(d) = &cfg.dir {
        return expand_home(d);
    }
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => expand_home("~/.claude"),
    }
}

pub fn spawn(cfg: ClaudeConfig, tx: watch::Sender<Snapshot<SessionsSnapshot>>) {
    let dir = claude_dir(&cfg);
    tokio::spawn(async move {
        let (ev_tx, mut ev_rx) = mpsc::channel::<()>(1);
        let _watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok_and(|ev| super::is_change(&ev)) {
                let _ = ev_tx.try_send(());
            }
        })
        .and_then(|mut w| w.watch(&dir.join("sessions"), RecursiveMode::NonRecursive).map(|_| w))
        .map_err(|e| tracing::warn!("sem inotify em {}/sessions: {e}", dir.display()))
        .ok();

        let mut cache = TranscriptCache::default();
        loop {
            let (d, c) = (dir.clone(), cfg.clone());
            let taken = std::mem::take(&mut cache);
            let (snap, back) = tokio::task::spawn_blocking(move || {
                let mut cache = taken;
                let snap = read_sessions(&d, &c, &mut cache, SystemTime::now());
                (snap, cache)
            })
            .await
            .unwrap_or_else(|e| (Err(format!("leitura das sessões falhou: {e}")), TranscriptCache::default()));
            cache = back;
            let out = match snap {
                Ok(s) => Snapshot::ok(s),
                Err(e) => Snapshot { error: Some(e), ..Default::default() },
            };
            if tx.send(out).is_err() {
                return;
            }
            // Status muda no arquivo (inotify pega na hora); processo que morre não
            // escreve nada, então há também uma checagem periódica.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                Some(()) = ev_rx.recv() => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    while ev_rx.try_recv().is_ok() {}
                }
            }
        }
    });
}

fn read_sessions(dir: &Path, cfg: &ClaudeConfig, cache: &mut TranscriptCache, now: SystemTime) -> Result<SessionsSnapshot, String> {
    let sessions_dir = dir.join("sessions");
    let entries = fs::read_dir(&sessions_dir).map_err(|e| format!("{}: {e}", sessions_dir.display()))?;
    let now_ms = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let recent_ms = cfg.recent_hours as i64 * 3_600_000;

    let mut live = Vec::new();
    let mut recent = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(f) = fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<SessionFile>(&s).ok()) else {
            continue;
        };
        let (Some(pid), Some(id)) = (f.pid, f.session_id.clone()) else { continue };
        let alive = is_alive(pid, f.proc_start.as_deref());
        let updated = f.status_updated_at.or(f.updated_at).or(f.started_at);
        if !alive && updated.is_none_or(|u| now_ms - u > recent_ms) {
            continue;
        }
        let t = cache.get(dir, &id);
        let mut s = build(&f, pid, id, t);
        if alive {
            live.push(s);
        } else {
            s.status = Status::Ended;
            s.waiting_for = None;
            recent.push(s);
        }
    }
    live.sort_by(|a, b| a.status.cmp(&b.status).then(b.updated_at.cmp(&a.updated_at)));
    recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    // A mesma conversa pode ter um arquivo antigo (pid morto) e um vivo: fica o vivo.
    recent.retain(|r| !live.iter().any(|l| l.id == r.id));
    recent.dedup_by(|a, b| a.id == b.id);
    recent.truncate(cfg.recent_max);
    cache.retain(live.iter().chain(&recent).map(|s| s.id.as_str()));
    Ok(SessionsSnapshot { live, recent })
}

fn build(f: &SessionFile, pid: u32, id: String, t: Option<&Transcript>) -> Session {
    let status = match f.status.as_deref() {
        Some("waiting") => Status::Waiting,
        Some("busy") => Status::Busy,
        Some("shell") => Status::Shell,
        _ => Status::Idle,
    };
    let short = id.chars().take(8).collect::<String>();
    // O `name` às vezes é só o jobId/prefixo do id: não serve de título.
    let name = f.name.clone().filter(|n| !n.is_empty() && *n != short && Some(n) != f.job_id.as_ref());
    let title = t
        .and_then(|t| t.title.clone())
        .or(name)
        .or_else(|| t.and_then(|t| t.last_prompt.clone()))
        .unwrap_or_else(|| short.clone());
    let summary = t.and_then(|t| t.summary.clone().or_else(|| t.last_prompt.as_ref().map(|p| format!("› {p}"))));
    let cwd = t.and_then(|t| t.cwd.clone()).or_else(|| f.cwd.as_ref().map(PathBuf::from)).unwrap_or_default();
    let updated = f.status_updated_at.or(f.updated_at).or(f.started_at).and_then(|ms| Utc.timestamp_millis_opt(ms).single());
    Session {
        pid,
        id,
        background: f.kind.as_deref() == Some("bg"),
        status,
        waiting_for: f.waiting_for.clone().filter(|w| !w.is_empty()),
        title: one_line(&title, 120),
        cwd,
        summary: summary.map(|s| one_line(&s, 300)),
        updated_at: updated,
    }
}

/// O processo existe e é o mesmo que criou o arquivo (o PID pode ter sido reciclado).
fn is_alive(pid: u32, proc_start: Option<&str>) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    match proc_start {
        Some(expected) => proc_start_time(&stat).is_some_and(|s| s == expected),
        None => true,
    }
}

/// Campo 22 (`starttime`) de /proc/<pid>/stat. O nome do processo (campo 2) pode ter
/// espaços e parênteses, então a contagem começa depois do último `)`.
fn proc_start_time(stat: &str) -> Option<&str> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)
}

fn one_line(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches("(disable recaps in /config)").trim_end().to_string();
    if s.chars().count() > max { s.chars().take(max - 1).chain(['…']).collect() } else { s }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Transcript {
    title: Option<String>,
    summary: Option<String>,
    last_prompt: Option<String>,
    cwd: Option<PathBuf>,
}

/// Transcrições lidas, por id; só relê quando o arquivo cresce.
#[derive(Default)]
struct TranscriptCache {
    paths: HashMap<String, PathBuf>,
    parsed: HashMap<String, (u64, Transcript)>,
}

impl TranscriptCache {
    fn get(&mut self, dir: &Path, id: &str) -> Option<&Transcript> {
        if !self.paths.contains_key(id) {
            let file = format!("{id}.jsonl");
            let found = fs::read_dir(dir.join("projects")).ok()?.flatten().map(|e| e.path().join(&file)).find(|p| p.is_file())?;
            self.paths.insert(id.to_string(), found);
        }
        let path = self.paths.get(id)?.clone();
        let len = fs::metadata(&path).ok()?.len();
        if self.parsed.get(id).is_none_or(|(l, _)| *l != len) {
            let t = read_transcript(&path).unwrap_or_default();
            self.parsed.insert(id.to_string(), (len, t));
        }
        self.parsed.get(id).map(|(_, t)| t)
    }

    fn retain<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        let keep: std::collections::HashSet<&str> = ids.collect();
        self.paths.retain(|k, _| keep.contains(k.as_str()));
        self.parsed.retain(|k, _| keep.contains(k.as_str()));
    }
}

const TAIL: u64 = 64 * 1024;
const HEAD: u64 = 16 * 1024;

/// Lê só o fim (64 KB) do arquivo; o título, se não estiver lá, vem do começo (16 KB).
fn read_transcript(path: &Path) -> std::io::Result<Transcript> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    // Começando no meio do arquivo, a primeira linha está cortada.
    let body = if start > 0 { text.split_once('\n').map_or("", |x| x.1) } else { &text };
    let mut t = parse_lines(body.lines().rev());
    if t.title.is_none() && start > 0 {
        let mut head = vec![0u8; HEAD.min(len) as usize];
        f.seek(SeekFrom::Start(0))?;
        f.read_exact(&mut head)?;
        let head = String::from_utf8_lossy(&head);
        t.title = parse_lines(head.lines()).title;
    }
    Ok(t)
}

/// Pega o primeiro valor de cada campo na ordem dada (do fim para o começo, no caso do tail).
fn parse_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Transcript {
    let mut t = Transcript::default();
    for line in lines {
        if t.title.is_some() && t.summary.is_some() && t.last_prompt.is_some() && t.cwd.is_some() {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
        match s("type").as_deref() {
            Some("custom-title") if t.title.is_none() => t.title = s("customTitle").or_else(|| s("title")),
            Some("ai-title") if t.title.is_none() => t.title = s("aiTitle"),
            Some("last-prompt") if t.last_prompt.is_none() => t.last_prompt = s("lastPrompt"),
            Some("system") if t.summary.is_none() && s("subtype").as_deref() == Some("away_summary") => {
                t.summary = s("content")
            }
            _ => {}
        }
        if t.cwd.is_none() {
            t.cwd = s("cwd").map(PathBuf::from);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_time_skips_tricky_comm() {
        let stat = "51898 (claude (x) y) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 18723 20 21";
        assert_eq!(proc_start_time(stat), Some("18723"));
    }

    #[test]
    fn our_own_process_is_alive() {
        let pid = std::process::id();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let start = proc_start_time(&stat).unwrap().to_string();
        assert!(is_alive(pid, Some(&start)));
        assert!(!is_alive(pid, Some("1")), "PID reciclado não pode contar como vivo");
        assert!(!is_alive(u32::MAX - 1, None));
    }

    #[test]
    fn transcript_tail_wins() {
        let lines = [
            r#"{"type":"ai-title","aiTitle":"Título antigo"}"#,
            r#"{"type":"user","cwd":"/home/u/v360"}"#,
            r#"lixo não-json"#,
            r#"{"type":"ai-title","aiTitle":"Shape técnico"}"#,
            r#"{"type":"last-prompt","lastPrompt":"continue"}"#,
            r#"{"type":"system","subtype":"away_summary","content":"Estou no meio do rebase. (disable recaps in /config)","cwd":"/home/u/v360/.claude/worktrees/x"}"#,
        ];
        let t = parse_lines(lines.iter().rev().copied());
        assert_eq!(t.title.as_deref(), Some("Shape técnico"));
        assert_eq!(t.last_prompt.as_deref(), Some("continue"));
        assert_eq!(t.cwd, Some(PathBuf::from("/home/u/v360/.claude/worktrees/x")));
        assert_eq!(one_line(t.summary.as_deref().unwrap(), 300), "Estou no meio do rebase.");
    }

    fn fixture() -> (PathBuf, u32, String) {
        let root = std::env::temp_dir().join(format!("cockpit-claude-{}-{:?}", std::process::id(), std::thread::current().id()));
        fs::create_dir_all(root.join("sessions")).unwrap();
        fs::create_dir_all(root.join("projects/-home-u-v360")).unwrap();
        let pid = std::process::id();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        (root, pid, proc_start_time(&stat).unwrap().to_string())
    }

    #[test]
    fn lists_live_and_recent_sessions() {
        let (root, pid, start) = fixture();
        let now = SystemTime::now();
        let ms = now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let w = |name: &str, body: String| fs::write(root.join("sessions").join(name), body).unwrap();
        w("a.json", format!(r#"{{"pid":{pid},"sessionId":"aaaa-1","cwd":"/home/u/v360","kind":"interactive","status":"idle","name":"v360-d9","procStart":"{start}","updatedAt":{ms}}}"#));
        w("b.json", format!(r#"{{"pid":{pid},"sessionId":"bbbb-2","cwd":"/home/u","kind":"bg","status":"waiting","waitingFor":"dialog open","jobId":"bbbb","name":"bbbb","procStart":"{start}","updatedAt":{}}}"#, ms - 1000));
        w("c.json", format!(r#"{{"pid":1,"sessionId":"cccc-3","cwd":"/home/u","status":"idle","procStart":"999999999","updatedAt":{}}}"#, ms - 3_600_000));
        w("d.json", format!(r#"{{"pid":1,"sessionId":"dddd-4","cwd":"/home/u","status":"idle","procStart":"999999999","updatedAt":{}}}"#, ms - 48 * 3_600_000));
        w("lixo.json", "{não é json".into());
        fs::write(
            root.join("projects/-home-u-v360/aaaa-1.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"Shape técnico\"}\n{\"type\":\"user\",\"cwd\":\"/home/u/v360/.claude/worktrees/x\"}\n",
        )
        .unwrap();

        let mut cache = TranscriptCache::default();
        let snap = read_sessions(&root, &ClaudeConfig::default(), &mut cache, now).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(snap.live.len(), 2);
        // esperando vem primeiro
        assert_eq!(snap.live[0].id, "bbbb-2");
        assert_eq!(snap.live[0].status, Status::Waiting);
        assert!(snap.live[0].background);
        assert_eq!(snap.live[0].title, "bbbb-2", "nome = jobId não serve: cai no id curto");
        assert_eq!(snap.live[0].waiting_for.as_deref(), Some("dialog open"));
        assert_eq!(snap.live[1].title, "Shape técnico");
        assert_eq!(snap.live[1].cwd, PathBuf::from("/home/u/v360/.claude/worktrees/x"));
        // morta há 1h entra em recentes; há 48h não
        assert_eq!(snap.recent.len(), 1);
        assert_eq!(snap.recent[0].id, "cccc-3");
        assert_eq!(snap.recent[0].status, Status::Ended);
        assert_eq!(snap.count(Status::Waiting), 1);
    }
}
