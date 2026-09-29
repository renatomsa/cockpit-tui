//! "Permitir desta vez" pelo cockpit, com o hook `PermissionRequest` do Claude Code.
//!
//! Como funciona (verificado com uma sessão real):
//! - Quando o Claude vai pedir permissão, ele roda `cockpit claude-hook` e mostra o
//!   diálogo normal ao mesmo tempo. Quem responder primeiro vale.
//! - O hook grava o pedido em `~/.local/state/cockpit/permissions/<id>.json` (pasta 0700)
//!   e espera. A tecla `a` do cockpit grava `<id>.decision` com o código do pedido, e o
//!   hook responde "allow" ao Claude.
//! - Se você responder no terminal, o Claude não encerra o hook: por isso ele vigia o
//!   status da sessão e sai calado quando ela deixa de esperar permissão.
//! - Qualquer erro faz o hook sair sem resposta, e o diálogo normal segue valendo.

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Pedido de permissão esperando resposta, como o cockpit lê.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    /// Só uma decisão com este código vale para este pedido.
    pub nonce: String,
    pub session_id: String,
    pub claude_pid: u32,
    pub tool: String,
    /// O que vai rodar: o comando, o arquivo, a URL…
    pub summary: String,
    pub created_at: i64,
}

pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("permissions")
}

/// Pedidos pendentes cujo processo do Claude ainda existe, do mais antigo ao mais novo.
pub fn pending(state_dir: &Path) -> Vec<Pending> {
    let Ok(rd) = fs::read_dir(dir(state_dir)) else { return Vec::new() };
    let mut out: Vec<Pending> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| serde_json::from_str::<Pending>(&fs::read_to_string(p).ok()?).ok())
        .filter(|p| Path::new(&format!("/proc/{}", p.claude_pid)).exists())
        .filter(|p| Utc::now().timestamp() - p.created_at < 900)
        .collect();
    out.sort_by_key(|p| p.created_at);
    out
}

/// A resposta do cockpit (`a`): o hook que espera por este pedido responde ao Claude.
pub fn decide(state_dir: &Path, p: &Pending, allow: bool) -> Result<()> {
    let path = dir(state_dir).join(format!("{}.decision", p.id));
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    f.write_all(format!("{} {}", p.nonce, if allow { "allow" } else { "deny" }).as_bytes())?;
    Ok(())
}

/// Texto curto do que o pedido vai fazer, para você decidir sem abrir o terminal.
pub fn summarize(tool: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let one_line = |t: String| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = match tool {
        "Bash" => s("command"),
        "Edit" | "Write" | "MultiEdit" | "Read" | "NotebookEdit" => s("file_path").or_else(|| s("notebook_path")),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        _ => None,
    }
    .or_else(|| {
        // Ferramentas de MCP e outras: o primeiro argumento de texto.
        input.as_object()?.values().find_map(|v| v.as_str().map(str::to_string))
    })
    .unwrap_or_default();
    let text = one_line(text);
    if text.chars().count() > 300 { text.chars().take(299).chain(['…']).collect() } else { text }
}

/// Sobe a árvore de processos até achar o do Claude (o que tem `sessions/<pid>.json`).
fn find_claude(claude_dir: &Path) -> Option<u32> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..8 {
        if claude_dir.join("sessions").join(format!("{pid}.json")).is_file() {
            return Some(pid);
        }
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        pid = stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()?;
        if pid <= 1 {
            return None;
        }
    }
    None
}

fn session_waiting(claude_dir: &Path, pid: u32) -> Option<bool> {
    let v: Value = serde_json::from_str(&fs::read_to_string(claude_dir.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    Some(v.get("status").and_then(Value::as_str) == Some("waiting"))
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Corpo do `cockpit claude-hook`. Nunca falha para o Claude: sem decisão, sai calado.
pub fn run_hook(state_dir: &Path, claude_dir: &Path) {
    if let Some(out) = hook(state_dir, claude_dir).unwrap_or(None) {
        println!("{out}");
    }
}

fn hook(state_dir: &Path, claude_dir: &Path) -> Result<Option<String>> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let input: Value = serde_json::from_str(&raw)?;
    if input.get("hook_event_name").and_then(Value::as_str) != Some("PermissionRequest") {
        return Ok(None);
    }
    let Some(claude_pid) = find_claude(claude_dir) else { return Ok(None) };
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("?").to_string();
    let p = Pending {
        id: random_hex(8),
        nonce: random_hex(16),
        session_id: input.get("session_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        claude_pid,
        summary: summarize(&tool, input.get("tool_input").unwrap_or(&Value::Null)),
        tool,
        created_at: Utc::now().timestamp(),
    };

    let d = dir(state_dir);
    fs::create_dir_all(&d)?;
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
    let req = d.join(format!("{}.json", p.id));
    let decision = d.join(format!("{}.decision", p.id));
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&req)?.write_all(serde_json::to_string(&p)?.as_bytes())?;
    let cleanup = || {
        let _ = fs::remove_file(&req);
        let _ = fs::remove_file(&decision);
    };

    let start = Instant::now();
    let mut seen_waiting = false;
    let mut not_waiting = 0;
    let mut last_check = Instant::now() - Duration::from_secs(1);
    let answer = loop {
        if let Ok(text) = fs::read_to_string(&decision) {
            let mut it = text.split_whitespace();
            if it.next() == Some(p.nonce.as_str()) {
                break match it.next() {
                    Some("allow") => Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}})),
                    Some("deny") => Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "deny", "message": "negado pelo cockpit"}}})),
                    _ => None,
                };
            }
        }
        if last_check.elapsed() >= Duration::from_millis(500) {
            last_check = Instant::now();
            match session_waiting(claude_dir, claude_pid) {
                None => break None, // o Claude saiu
                Some(true) => {
                    seen_waiting = true;
                    not_waiting = 0;
                }
                // Respondido no terminal (ou aprovado por outra regra): sai calado.
                Some(false) if seen_waiting => {
                    not_waiting += 1;
                    if not_waiting >= 2 {
                        break None;
                    }
                }
                // O diálogo nem chegou a abrir.
                Some(false) if start.elapsed() > Duration::from_secs(15) => break None,
                Some(false) => {}
            }
        }
        if start.elapsed() > Duration::from_secs(590) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    cleanup();
    Ok(answer.map(|v| v.to_string()))
}

// ── Instalação no ~/.claude/settings.json

const MARK: &str = "claude-hook";

fn hook_command() -> Result<String> {
    let exe = std::env::current_exe()?.display().to_string();
    Ok(if exe.contains(' ') { format!("'{exe}' {MARK}") } else { format!("{exe} {MARK}") })
}

fn is_ours(entry: &Value) -> bool {
    entry.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
        hs.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains("cockpit") && c.ends_with(MARK)))
    })
}

pub fn installed(settings: &Path) -> bool {
    let Ok(v) = fs::read_to_string(settings).map(|s| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null)) else { return false };
    v.pointer("/hooks/PermissionRequest").and_then(Value::as_array).is_some_and(|a| a.iter().any(is_ours))
}

/// Acrescenta o hook ao settings.json, preservando o resto (e a ordem das chaves).
/// Guarda uma cópia em `settings.json.cockpit-backup` antes de gravar.
pub fn install(settings: &Path) -> Result<bool> {
    let text = fs::read_to_string(settings).unwrap_or_else(|_| "{}".into());
    let mut v: Value = serde_json::from_str(&text).with_context(|| format!("{} não é JSON válido", settings.display()))?;
    let root = v.as_object_mut().ok_or_else(|| anyhow!("{} não é um objeto JSON", settings.display()))?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let list = hooks.as_object_mut().ok_or_else(|| anyhow!("`hooks` não é um objeto"))?.entry("PermissionRequest").or_insert_with(|| json!([]));
    let arr = list.as_array_mut().ok_or_else(|| anyhow!("`hooks.PermissionRequest` não é uma lista"))?;
    if arr.iter().any(is_ours) {
        return Ok(false);
    }
    arr.push(json!({"matcher": "*", "hooks": [{"type": "command", "command": hook_command()?, "timeout": 600}]}));
    if settings.exists() {
        fs::copy(settings, settings.with_extension("json.cockpit-backup"))?;
    } else if let Some(d) = settings.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(settings, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(true)
}

pub fn uninstall(settings: &Path) -> Result<bool> {
    let text = fs::read_to_string(settings)?;
    let mut v: Value = serde_json::from_str(&text)?;
    let Some(arr) = v.pointer_mut("/hooks/PermissionRequest").and_then(Value::as_array_mut) else { return Ok(false) };
    let before = arr.len();
    arr.retain(|e| !is_ours(e));
    if arr.len() == before {
        return Ok(false);
    }
    if arr.is_empty()
        && let Some(h) = v.get_mut("hooks").and_then(Value::as_object_mut)
    {
        h.remove("PermissionRequest");
    }
    fs::copy(settings, settings.with_extension("json.cockpit-backup"))?;
    fs::write(settings, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_show_what_will_run() {
        assert_eq!(summarize("Bash", &json!({"command": "git push\n  origin main", "description": "x"})), "git push origin main");
        assert_eq!(summarize("Edit", &json!({"file_path": "/w/src/a.rs", "old_string": "x"})), "/w/src/a.rs");
        assert_eq!(summarize("WebFetch", &json!({"url": "https://x.dev"})), "https://x.dev");
        assert_eq!(summarize("mcp__slack__send", &json!({"channel": "#dev", "text": "oi"})), "#dev");
        assert_eq!(summarize("Outra", &json!({})), "");
    }

    #[test]
    fn install_preserves_settings_and_is_idempotent() {
        let d = std::env::temp_dir().join(format!("cockpit-hookinst-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("settings.json");
        fs::write(&p, r#"{"model":"opus","permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#).unwrap();
        assert!(install(&p).unwrap());
        assert!(!install(&p).unwrap(), "segunda vez não duplica");
        assert!(installed(&p));
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "permissions", "hooks"], "a ordem das chaves é mantida");
        assert!(v.pointer("/hooks/Stop").is_some(), "outros hooks ficam");
        assert!(d.join("settings.json.cockpit-backup").exists());
        assert!(uninstall(&p).unwrap());
        assert!(!installed(&p));
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert!(v.pointer("/hooks/PermissionRequest").is_none() && v.pointer("/hooks/Stop").is_some());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn decisions_need_the_nonce() {
        let d = std::env::temp_dir().join(format!("cockpit-hookdec-{}", std::process::id()));
        let p = Pending { id: "abc".into(), nonce: "n1".into(), claude_pid: std::process::id(), created_at: Utc::now().timestamp(), ..Default::default() };
        fs::create_dir_all(dir(&d)).unwrap();
        fs::write(dir(&d).join("abc.json"), serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(pending(&d), vec![p.clone()]);
        decide(&d, &p, true).unwrap();
        assert_eq!(fs::read_to_string(dir(&d).join("abc.decision")).unwrap(), "n1 allow");
        // pedido de processo que já morreu não aparece
        let dead = Pending { id: "old".into(), claude_pid: u32::MAX - 1, created_at: Utc::now().timestamp(), ..Default::default() };
        fs::write(dir(&d).join("old.json"), serde_json::to_string(&dead).unwrap()).unwrap();
        assert_eq!(pending(&d).len(), 1);
        fs::remove_dir_all(&d).ok();
    }
}
