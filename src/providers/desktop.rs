//! Conversas do Claude Desktop (Windows), vistas pelo WSL.
//!
//! O app guarda um `.json` por sessão de Code (`claude-code-sessions`) e por tarefa do
//! modo agente/Cowork (`local-agent-mode-sessions`), com título, pasta, modelo e última
//! atividade. O estado ao vivo (esperando, trabalhando) vem de `~/.claude/sessions` do
//! Windows, e os tokens da rodada, das transcrições em `~/.claude/projects` do Windows.
//! As conversas comuns de chat ficam no servidor do claude.ai e não aparecem aqui.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use tokio::sync::watch;

use super::{
    Snapshot,
    claude::Status,
    transcript::{Tracker, TurnStats},
};

const EVERY: Duration = Duration::from_secs(20);
/// Sessões com atividade nos últimos N dias.
const RECENT_DAYS: i64 = 7;
const MAX: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Aba Code do app (Claude Code rodando no Windows).
    Code,
    /// Modo agente / Cowork.
    Cowork,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopSession {
    pub kind: Kind,
    pub title: String,
    /// Pasta no Windows (C:\…), quando é uma sessão de Code.
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub last_activity: DateTime<Utc>,
    pub cli_session_id: Option<String>,
    /// Estado ao vivo, se a sessão está aberta no app agora.
    pub status: Option<Status>,
    pub waiting_for: Option<String>,
    /// PID do processo no Windows (para conferir se ainda existe).
    pub win_pid: Option<u32>,
    pub turn: Option<TurnStats>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopSnapshot {
    pub sessions: Vec<DesktopSession>,
    pub app_dir: PathBuf,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct MetaFile {
    cli_session_id: Option<String>,
    cwd: Option<String>,
    last_activity_at: Option<i64>,
    created_at: Option<i64>,
    model: Option<String>,
    is_archived: bool,
    title: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct LiveFile {
    pid: Option<u32>,
    session_id: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
}

/// Pastas do app no Windows: a da Microsoft Store (pacote) ou a do instalador comum.
pub fn find_app_dir() -> Option<(PathBuf, PathBuf)> {
    let users = fs::read_dir("/mnt/c/Users").ok()?;
    let me = std::env::var("USER").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = users.flatten().map(|e| e.path()).collect();
    // O usuário do Windows com o nome mais parecido com o do Linux vem primeiro.
    candidates.sort_by_key(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with(&me.to_lowercase())));
    for home in candidates {
        let win_claude = home.join(".claude");
        if let Ok(pkgs) = fs::read_dir(home.join("AppData/Local/Packages")) {
            for p in pkgs.flatten() {
                if p.file_name().to_string_lossy().starts_with("Claude_") {
                    let app = p.path().join("LocalCache/Roaming/Claude");
                    if app.is_dir() {
                        return Some((app, win_claude));
                    }
                }
            }
        }
        let app = home.join("AppData/Roaming/Claude");
        if app.join("claude-code-sessions").is_dir() {
            return Some((app, win_claude));
        }
    }
    None
}

pub fn spawn(tx: watch::Sender<Snapshot<DesktopSnapshot>>) {
    tokio::spawn(async move {
        let Some((app, win_claude)) = tokio::task::spawn_blocking(find_app_dir).await.ok().flatten() else {
            tx.send_replace(Snapshot { error: Some("Claude Desktop não encontrado no Windows".into()), ..Default::default() });
            return;
        };
        let mut cache = Cache::default();
        loop {
            let (a, w) = (app.clone(), win_claude.clone());
            let taken = std::mem::take(&mut cache);
            let (snap, back) = tokio::task::spawn_blocking(move || {
                let mut c = taken;
                let s = read(&a, &w, &mut c);
                (s, c)
            })
            .await
            .unwrap_or_else(|_| (DesktopSnapshot::default(), Cache::default()));
            cache = back;
            if tx.send(Snapshot::ok(snap)).is_err() {
                return;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Tamanho e data do arquivo quando foi lido, e o que ele continha.
type MetaEntry = (u64, SystemTime, Option<(Kind, MetaFile)>);

/// Metadados já lidos (por caminho, tamanho e data) e transcrições acompanhadas.
#[derive(Default)]
struct Cache {
    meta: HashMap<PathBuf, MetaEntry>,
    trackers: HashMap<String, Tracker>,
    transcripts: HashMap<String, PathBuf>,
}

fn read(app: &Path, win_claude: &Path, cache: &mut Cache) -> DesktopSnapshot {
    let cutoff = Utc::now() - chrono::Duration::days(RECENT_DAYS);
    let mut metas: Vec<(Kind, MetaFile)> = Vec::new();
    for (sub, kind) in [("claude-code-sessions", Kind::Code), ("local-agent-mode-sessions", Kind::Cowork)] {
        for f in glob2(&app.join(sub)) {
            let Ok(m) = fs::metadata(&f) else { continue };
            let (len, mtime) = (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH));
            // Arquivo parado há mais tempo que a janela: nem abre.
            if mtime < SystemTime::UNIX_EPOCH + Duration::from_millis(cutoff.timestamp_millis().max(0) as u64) {
                continue;
            }
            let fresh = cache.meta.get(&f).is_some_and(|(l, t, _)| *l == len && *t == mtime);
            if !fresh {
                let parsed = fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<MetaFile>(&s).ok());
                cache.meta.insert(f.clone(), (len, mtime, parsed.map(|p| (kind, p))));
            }
            if let Some((_, _, Some((k, meta)))) = cache.meta.get(&f) {
                metas.push((*k, meta.clone()));
            }
        }
    }

    // Estado ao vivo, pelo id da sessão do Claude Code.
    let live: HashMap<String, LiveFile> = fs::read_dir(win_claude.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str::<LiveFile>(&fs::read_to_string(e.path()).ok()?).ok())
        .filter_map(|l| Some((l.session_id.clone()?, l)))
        .collect();

    let mut out: Vec<DesktopSession> = metas
        .into_iter()
        .filter(|(_, m)| !m.is_archived)
        .filter_map(|(kind, m)| {
            let at = m.last_activity_at.or(m.created_at)?;
            let last = Utc.timestamp_millis_opt(at).single()?;
            (last >= cutoff).then_some((kind, m, last))
        })
        .map(|(kind, m, last)| {
            let l = m.cli_session_id.as_ref().and_then(|id| live.get(id));
            let status = l.and_then(|l| l.status.as_deref()).map(|s| match s {
                "waiting" => Status::Waiting,
                "busy" => Status::Busy,
                "shell" => Status::Shell,
                _ => Status::Idle,
            });
            DesktopSession {
                kind,
                title: m.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "(sem título)".into()),
                cwd: m.cwd.clone(),
                model: m.model.clone(),
                last_activity: last,
                cli_session_id: m.cli_session_id.clone(),
                status,
                waiting_for: l.and_then(|l| l.waiting_for.clone()),
                win_pid: l.and_then(|l| l.pid),
                turn: None,
            }
        })
        .collect();
    out.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    out.truncate(MAX);

    // Tokens da rodada, das transcrições do Windows.
    for s in &mut out {
        let Some(id) = &s.cli_session_id else { continue };
        if !cache.transcripts.contains_key(id) {
            let file = format!("{id}.jsonl");
            if let Some(p) = fs::read_dir(win_claude.join("projects")).ok().and_then(|rd| rd.flatten().map(|e| e.path().join(&file)).find(|p| p.is_file())) {
                cache.transcripts.insert(id.clone(), p);
            }
        }
        if let Some(p) = cache.transcripts.get(id) {
            let t = cache.trackers.entry(id.clone()).or_insert_with(|| Tracker::new(p.clone()));
            t.refresh();
            s.turn = t.info().turn.filter(|x| x.calls > 0);
        }
    }
    DesktopSnapshot { sessions: out, app_dir: app.to_path_buf() }
}

/// `<dir>/<conta>/<org>/local_*.json`
fn glob2(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in fs::read_dir(dir).into_iter().flatten().flatten() {
        for b in fs::read_dir(a.path()).into_iter().flatten().flatten() {
            for f in fs::read_dir(b.path()).into_iter().flatten().flatten() {
                let p = f.path();
                if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("local_")) && p.extension().is_some_and(|x| x == "json") {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_code_and_cowork_sessions_with_live_status() {
        let root = std::env::temp_dir().join(format!("cockpit-desktop-{}", std::process::id()));
        let app = root.join("app");
        let win = root.join("win");
        let now = Utc::now().timestamp_millis();
        let code = app.join("claude-code-sessions/acc/org");
        let cowork = app.join("local-agent-mode-sessions/acc/org");
        fs::create_dir_all(&code).unwrap();
        fs::create_dir_all(&cowork).unwrap();
        fs::create_dir_all(win.join("sessions")).unwrap();
        fs::write(code.join("local_a.json"), format!(r#"{{"cliSessionId":"cli-a","cwd":"C:\\w\\proj","lastActivityAt":{now},"model":"opus","isArchived":false,"title":"Refatorar o parser","enabledMcpTools":{{"x":true}}}}"#)).unwrap();
        fs::write(code.join("local_b.json"), format!(r#"{{"cliSessionId":"cli-b","lastActivityAt":{},"isArchived":true,"title":"Arquivada"}}"#, now - 1000)).unwrap();
        fs::write(cowork.join("local_c.json"), format!(r#"{{"cliSessionId":"cli-c","lastActivityAt":{},"isArchived":false,"title":"Planilha de vendas","systemPrompt":"…"}}"#, now - 60_000)).unwrap();
        fs::write(cowork.join("cowork-gb-cache.json"), "{}").unwrap();
        fs::write(win.join("sessions/123.json"), r#"{"pid":123,"sessionId":"cli-a","status":"waiting","waitingFor":"permission prompt","entrypoint":"claude-desktop"}"#).unwrap();

        let mut cache = Cache::default();
        let s = read(&app, &win, &mut cache);
        fs::remove_dir_all(&root).ok();
        assert_eq!(s.sessions.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Refatorar o parser", "Planilha de vendas"]);
        let a = &s.sessions[0];
        assert_eq!((a.kind, a.status, a.win_pid), (Kind::Code, Some(Status::Waiting), Some(123)));
        assert_eq!(a.waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(s.sessions[1].kind, Kind::Cowork);
        assert!(s.sessions[1].status.is_none());
    }
}
