//! Leitura incremental das transcrições do Claude Code (`<id>.jsonl`). Na primeira vez
//! o arquivo é lido inteiro; depois, só o que foi acrescentado desde a última leitura.
//!
//! Além de título, resumo e pasta, acompanha o gasto de tokens da rodada atual: tudo
//! desde o último pedido do usuário, somando os subagentes disparados nele
//! (`<id>/subagents/agent-*.jsonl`, ligados pelo `promptId`).

use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde_json::Value;

/// Na primeira leitura, no máximo os últimos 32 MB (transcrições gigantes).
const MAX_INITIAL: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    fn from_json(u: &Value) -> Self {
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        }
    }

    /// Tudo o que a chamada processou (entrada, cache lido e escrito, saída).
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// Tamanho do contexto enviado nessa chamada.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

/// Gasto da rodada atual (ou da última, se a conversa estiver parada).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnStats {
    pub started_at: Option<DateTime<Utc>>,
    /// Tokens gerados (inclui raciocínio).
    pub output: u64,
    /// Tokens processados em todas as chamadas da rodada, inclusive cache.
    pub total: u64,
    /// Contexto da última chamada da conversa principal.
    pub context: u64,
    /// Chamadas à API (a conversa principal e os subagentes).
    pub calls: usize,
    pub subagents: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub last_prompt: Option<String>,
    pub cwd: Option<PathBuf>,
    pub turn: Option<TurnStats>,
}

/// Lê um `.jsonl` em pedaços, guardando a linha incompleta do fim para a próxima vez.
#[derive(Default)]
struct LineReader {
    offset: u64,
    carry: Vec<u8>,
}

impl LineReader {
    /// Linhas completas novas. `reset` = o arquivo encolheu (foi reescrito): recomeçar.
    fn read(&mut self, path: &Path, max_initial: u64) -> std::io::Result<(Vec<String>, bool)> {
        let mut f = fs::File::open(path)?;
        let len = f.metadata()?.len();
        let mut reset = false;
        if len < self.offset {
            *self = Self::default();
            reset = true;
        }
        let mut skip_first = false;
        if self.offset == 0 && len > max_initial {
            self.offset = len - max_initial;
            skip_first = true; // começa no meio de uma linha
        }
        if len == self.offset {
            return Ok((Vec::new(), reset));
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = std::mem::take(&mut self.carry);
        (&mut f).take(len - self.offset).read_to_end(&mut buf)?;
        self.offset = len;
        let complete = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        self.carry = buf.split_off(complete);
        let mut lines: Vec<String> = String::from_utf8_lossy(&buf).lines().map(str::to_string).collect();
        if skip_first && !lines.is_empty() {
            lines.remove(0);
        }
        Ok((lines, reset))
    }
}

/// Subagente: só interessa o `promptId` (primeira linha) e o gasto.
#[derive(Default)]
struct SubTracker {
    reader: LineReader,
    prompt_id: Option<String>,
    msgs: HashMap<String, Usage>,
}

impl SubTracker {
    fn refresh(&mut self, path: &Path) {
        let Ok((lines, reset)) = self.reader.read(path, u64::MAX) else { return };
        if reset {
            *self = Self { reader: std::mem::take(&mut self.reader), ..Default::default() };
        }
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if self.prompt_id.is_none() {
                self.prompt_id = str_of(&v, "promptId");
            }
            if let Some((id, u)) = assistant_usage(&v) {
                self.msgs.insert(id, u);
            }
        }
    }
}

#[derive(Default)]
pub struct Tracker {
    path: PathBuf,
    reader: LineReader,
    ai_title: Option<String>,
    custom_title: Option<String>,
    summary: Option<String>,
    last_prompt: Option<String>,
    cwd: Option<PathBuf>,
    prompt_id: Option<String>,
    started_at: Option<DateTime<Utc>>,
    /// Chamadas da rodada atual por `message.id`: a mesma resposta aparece em várias
    /// linhas (uma por bloco de conteúdo), sempre com o mesmo `usage`.
    msgs: HashMap<String, Usage>,
    last_ctx: u64,
    subs: HashMap<PathBuf, SubTracker>,
}

impl Tracker {
    pub fn new(path: PathBuf) -> Self {
        Self { path, ..Default::default() }
    }

    pub fn refresh(&mut self) {
        match self.reader.read(&self.path, MAX_INITIAL) {
            Ok((lines, reset)) => {
                if reset {
                    *self = Self { path: std::mem::take(&mut self.path), reader: std::mem::take(&mut self.reader), ..Default::default() };
                }
                for line in lines {
                    self.line(&line);
                }
            }
            Err(e) => tracing::debug!("transcrição {}: {e}", self.path.display()),
        }
        if self.prompt_id.is_some() {
            self.refresh_subagents();
        }
    }

    fn refresh_subagents(&mut self) {
        let dir = self.path.with_extension("").join("subagents");
        let Ok(entries) = fs::read_dir(&dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "jsonl") {
                let sub = self.subs.entry(p.clone()).or_default();
                // Subagente de uma rodada antiga não muda mais: não precisa reler.
                if sub.prompt_id.is_none() || sub.prompt_id == self.prompt_id {
                    sub.refresh(&p);
                }
            }
        }
    }

    fn line(&mut self, line: &str) {
        if !line.contains("\"type\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if let Some(c) = str_of(&v, "cwd") {
            self.cwd = Some(PathBuf::from(c));
        }
        let sidechain = v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => self.ai_title = str_of(&v, "aiTitle"),
            Some("custom-title") => self.custom_title = str_of(&v, "customTitle").or_else(|| str_of(&v, "title")),
            Some("last-prompt") => self.last_prompt = str_of(&v, "lastPrompt").or(self.last_prompt.take()),
            Some("system") if str_of(&v, "subtype").as_deref() == Some("away_summary") => self.summary = str_of(&v, "content"),
            Some("user") if !sidechain && !v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) => {
                if let Some(text) = prompt_text(v.get("message").and_then(|m| m.get("content"))) {
                    self.prompt_id = str_of(&v, "promptId").or_else(|| str_of(&v, "uuid"));
                    self.started_at = str_of(&v, "timestamp").and_then(|t| t.parse().ok());
                    self.msgs.clear();
                    self.last_ctx = 0;
                    self.last_prompt = Some(text);
                    // o resumo era de antes deste pedido
                    self.summary = None;
                }
            }
            Some("assistant") if !sidechain => {
                if let Some((id, u)) = assistant_usage(&v) {
                    self.last_ctx = u.context();
                    self.msgs.insert(id, u);
                }
            }
            _ => {}
        }
    }

    pub fn info(&self) -> Info {
        let turn = self.prompt_id.as_ref().map(|pid| {
            let subs: Vec<&SubTracker> = self.subs.values().filter(|s| s.prompt_id.as_ref() == Some(pid)).collect();
            let all = self.msgs.values().chain(subs.iter().flat_map(|s| s.msgs.values()));
            let (mut output, mut total, mut calls) = (0, 0, 0);
            for u in all {
                output += u.output;
                total += u.total();
                calls += 1;
            }
            TurnStats { started_at: self.started_at, output, total, context: self.last_ctx, calls, subagents: subs.len() }
        });
        Info {
            title: self.custom_title.clone().or_else(|| self.ai_title.clone()),
            summary: self.summary.clone(),
            last_prompt: self.last_prompt.clone(),
            cwd: self.cwd.clone(),
            turn,
        }
    }
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn assistant_usage(v: &Value) -> Option<(String, Usage)> {
    let m = v.get("message")?;
    Some((m.get("id")?.as_str()?.to_string(), Usage::from_json(m.get("usage")?)))
}

/// O texto, se a mensagem é um pedido de fato. Não conta: resultado de ferramenta,
/// saída de comando local e o aviso de interrupção.
fn prompt_text(content: Option<&Value>) -> Option<String> {
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
                return None;
            }
            let t: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if t.is_empty() {
                return None;
            }
            t.join(" ")
        }
        _ => return None,
    };
    let trimmed = text.trim_start();
    let not_prompt = ["<local-command-stdout>", "<local-command-stderr>", "<local-command-caveat>", "[Request interrupted"];
    if trimmed.is_empty() || not_prompt.iter().any(|p| trimmed.starts_with(p)) {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn user(prompt_id: &str, text: &str) -> String {
        format!(r#"{{"type":"user","promptId":"{prompt_id}","timestamp":"2026-09-27T18:00:00Z","cwd":"/w","message":{{"role":"user","content":"{text}"}}}}"#)
    }
    fn tool_result(prompt_id: &str) -> String {
        format!(r#"{{"type":"user","promptId":"{prompt_id}","message":{{"content":[{{"type":"tool_result","content":"ok"}}]}}}}"#)
    }
    fn assistant(id: &str, input: u64, out: u64, cr: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","usage":{{"input_tokens":{input},"output_tokens":{out},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":10}}}}}}"#
        )
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cockpit-tr-{}-{name}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d.join("sess.jsonl")
    }

    fn append(p: &Path, s: &str) {
        fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(s.as_bytes()).unwrap();
    }

    #[test]
    fn counts_the_current_turn_once_per_message() {
        let p = tmp("turn");
        let lines = [
            r#"{"type":"ai-title","aiTitle":"Shape técnico"}"#.to_string(),
            user("p1", "primeiro pedido"),
            assistant("m1", 5, 100, 1000),
            user("p2", "segundo pedido"),
            assistant("m2", 2, 300, 2000),
            assistant("m2", 2, 300, 2000), // mesma resposta, outro bloco
            tool_result("p2"),
            assistant("m3", 1, 50, 2500),
        ];
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        let mut t = Tracker::new(p.clone());
        t.refresh();
        let i = t.info();
        let turn = i.turn.unwrap();
        assert_eq!(turn.calls, 2, "m2 conta uma vez; m1 é da rodada anterior");
        assert_eq!(turn.output, 350);
        assert_eq!(turn.total, (2 + 300 + 2000 + 10) + (1 + 50 + 2500 + 10));
        assert_eq!(turn.context, 1 + 2500 + 10);
        assert_eq!(i.title.as_deref(), Some("Shape técnico"));
        assert_eq!(i.last_prompt.as_deref(), Some("segundo pedido"));
        assert_eq!(i.cwd, Some(PathBuf::from("/w")));
        fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn reads_incrementally_and_keeps_partial_lines() {
        let p = tmp("incr");
        fs::write(&p, user("p1", "oi") + "\n").unwrap();
        let mut t = Tracker::new(p.clone());
        t.refresh();
        assert_eq!(t.info().turn.unwrap().calls, 0);
        // linha escrita pela metade: ainda não conta
        let line = assistant("m1", 1, 10, 100);
        let (a, b) = line.split_at(30);
        append(&p, a);
        t.refresh();
        assert_eq!(t.info().turn.unwrap().calls, 0);
        append(&p, &format!("{b}\n"));
        t.refresh();
        assert_eq!(t.info().turn.unwrap().output, 10);
        // pedido novo zera a rodada e o resumo antigo
        append(&p, "{\"type\":\"system\",\"subtype\":\"away_summary\",\"content\":\"resumo\"}\n");
        t.refresh();
        assert_eq!(t.info().summary.as_deref(), Some("resumo"));
        append(&p, &(user("p2", "de novo") + "\n"));
        t.refresh();
        let i = t.info();
        assert_eq!(i.turn.unwrap().calls, 0);
        assert!(i.summary.is_none());
        fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn adds_subagents_of_the_current_prompt() {
        let p = tmp("sub");
        fs::write(&p, format!("{}\n{}\n", user("p1", "revisa"), assistant("m1", 1, 100, 0))).unwrap();
        let subs = p.with_extension("").join("subagents");
        fs::create_dir_all(&subs).unwrap();
        let sub_line = |pid: &str, id: &str| {
            format!(
                "{{\"type\":\"user\",\"isSidechain\":true,\"promptId\":\"{pid}\",\"message\":{{\"content\":\"tarefa\"}}}}\n{}\n",
                assistant(id, 1, 40, 0).replace("\"type\":\"assistant\"", "\"type\":\"assistant\",\"isSidechain\":true")
            )
        };
        fs::write(subs.join("agent-a.jsonl"), sub_line("p1", "s1")).unwrap();
        fs::write(subs.join("agent-b.jsonl"), sub_line("p0", "s2")).unwrap(); // de outra rodada
        let mut t = Tracker::new(p.clone());
        t.refresh();
        let turn = t.info().turn.unwrap();
        assert_eq!(turn.subagents, 1);
        assert_eq!(turn.output, 140);
        assert_eq!(turn.calls, 2);
        fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn what_counts_as_a_prompt() {
        let s = |x: &str| Some(Value::String(x.into()));
        assert!(prompt_text(s("faça a F2").as_ref()).is_some());
        assert!(prompt_text(s("<local-command-stdout>ok</local-command-stdout>").as_ref()).is_none());
        assert!(prompt_text(s("[Request interrupted by user]").as_ref()).is_none());
        let tool: Value = serde_json::from_str(r#"[{"type":"tool_result","content":"x"}]"#).unwrap();
        assert!(prompt_text(Some(&tool)).is_none());
        let img: Value = serde_json::from_str(r#"[{"type":"image"},{"type":"text","text":"olha isso"}]"#).unwrap();
        assert_eq!(prompt_text(Some(&img)).as_deref(), Some("olha isso"));
    }
}
