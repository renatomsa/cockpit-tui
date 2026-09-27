//! Limites do plano do Claude (os mesmos do `/usage`): sessão de 5 h e semana.
//!
//! Usa o login OAuth que o Claude Code guarda em `~/.claude/.credentials.json` e consulta
//! o mesmo endpoint que ele. O token só é lido: nunca renovado nem gravado aqui, porque
//! renovar trocaria o refresh token por baixo do Claude Code. Com o login vencido, o
//! painel pede para abrir o `claude`, que renova sozinho.

use std::{path::Path, time::Duration};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::watch;

use super::{Backoff, Snapshot, http_client};
use crate::config::ClaudeConfig;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageSnapshot {
    pub limits: Vec<Limit>,
    /// Plano do login (ex.: "team", "max").
    pub plan: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub kind: String,
    pub label: String,
    pub percent: f64,
    pub severity: Severity,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    #[default]
    Normal,
    Warning,
    Critical,
}

pub fn spawn(cfg: ClaudeConfig, tx: watch::Sender<Snapshot<UsageSnapshot>>) {
    if !cfg.usage_limits {
        return;
    }
    let creds = super::claude::claude_dir(&cfg).join(".credentials.json");
    let every = Duration::from_secs(cfg.usage_refresh_secs.max(60));
    tokio::spawn(async move {
        let client = http_client();
        let mut backoff = Backoff::new(Duration::from_secs(60), Duration::from_secs(900));
        loop {
            let delay = match fetch(&client, &creds).await {
                Ok(snap) => {
                    backoff.reset();
                    tx.send_replace(Snapshot::ok(snap));
                    every
                }
                Err(e) => {
                    tracing::info!("limites de uso: {e:#}");
                    tx.send_modify(|s| s.error = Some(format!("{e:#}")));
                    backoff.next_delay()
                }
            };
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(delay).await;
        }
    });
}

async fn fetch(client: &reqwest::Client, creds: &Path) -> Result<UsageSnapshot> {
    // Relido a cada vez: o Claude Code troca o token quando renova.
    let raw = tokio::fs::read_to_string(creds)
        .await
        .map_err(|_| anyhow!("sem login do claude.ai em {} (com API key não há limite de plano)", creds.display()))?;
    let v: Value = serde_json::from_str(&raw)?;
    let oauth = v.get("claudeAiOauth").ok_or_else(|| anyhow!("sem login do claude.ai (com API key não há limite de plano)"))?;
    let token = oauth.get("accessToken").and_then(Value::as_str).ok_or_else(|| anyhow!("login sem token"))?;
    let plan = oauth.get("subscriptionType").and_then(Value::as_str).map(str::to_string);
    if let Some(exp) = oauth.get("expiresAt").and_then(Value::as_i64)
        && exp <= Utc::now().timestamp_millis() + 30_000
    {
        bail!("login do Claude venceu; abra o `claude` para renovar");
    }

    let resp = client
        .get(ENDPOINT)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 => bail!("login do Claude venceu; abra o `claude` para renovar"),
        429 => bail!("muitas consultas; tento de novo em breve"),
        code => bail!("HTTP {code}"),
    }
    let body: Value = resp.json().await?;
    let limits = parse(&body);
    if limits.is_empty() {
        bail!("resposta sem limites");
    }
    Ok(UsageSnapshot { limits, plan })
}

/// Lê a lista `limits`; se ela não vier, os campos antigos (`five_hour`, `seven_day`…).
fn parse(v: &Value) -> Vec<Limit> {
    let reset = |x: &Value| x.get("resets_at").and_then(Value::as_str).and_then(|s| s.parse().ok());
    let mut out: Vec<Limit> = v
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|l| {
            let kind = l.get("kind")?.as_str()?.to_string();
            let percent = l.get("percent").and_then(Value::as_f64)?;
            let sev = severity(l.get("severity").and_then(Value::as_str), percent);
            Some(Limit { label: label(&kind), kind, percent, severity: sev, resets_at: reset(l) })
        })
        .collect();
    if out.is_empty() {
        for (key, kind) in [("five_hour", "session"), ("seven_day", "weekly_all"), ("seven_day_opus", "weekly_opus"), ("seven_day_sonnet", "weekly_sonnet")] {
            let Some(o) = v.get(key).filter(|o| o.is_object()) else { continue };
            let Some(percent) = o.get("utilization").and_then(Value::as_f64) else { continue };
            out.push(Limit { kind: kind.into(), label: label(kind), percent, severity: severity(None, percent), resets_at: reset(o) });
        }
    }
    out
}

fn label(kind: &str) -> String {
    match kind {
        "session" => "Sessão 5h".into(),
        "weekly_all" => "Semana".into(),
        "weekly_opus" => "Semana Opus".into(),
        "weekly_sonnet" => "Semana Sonnet".into(),
        other => {
            let s = other.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

fn severity(server: Option<&str>, percent: f64) -> Severity {
    let from_server = match server {
        Some("warning" | "high" | "elevated") => Severity::Warning,
        Some("critical" | "exceeded" | "blocked" | "limited") => Severity::Critical,
        _ => Severity::Normal,
    };
    let local = if percent >= 100.0 {
        Severity::Critical
    } else if percent >= 90.0 {
        Severity::Warning
    } else {
        Severity::Normal
    };
    from_server.max(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_limits_list() {
        let v: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":45.0,"resets_at":"2026-09-27T22:50:00.3+00:00"},
                "limits":[
                  {"kind":"session","group":"session","percent":45,"severity":"normal","resets_at":"2026-09-27T22:50:00.318628+00:00","is_active":true},
                  {"kind":"weekly_all","group":"weekly","percent":92,"severity":"normal","resets_at":"2026-10-03T06:00:00+00:00","is_active":false},
                  {"kind":"weekly_opus","percent":100,"severity":"exceeded","resets_at":null}
                ]}"#,
        )
        .unwrap();
        let l = parse(&v);
        assert_eq!(l.len(), 3);
        assert_eq!((l[0].label.as_str(), l[0].percent, l[0].severity), ("Sessão 5h", 45.0, Severity::Normal));
        assert_eq!(l[0].resets_at.unwrap().to_rfc3339(), "2026-09-27T22:50:00.318628+00:00");
        assert_eq!(l[1].severity, Severity::Warning, "92% já é aviso");
        assert_eq!(l[2].severity, Severity::Critical);
        assert!(l[2].resets_at.is_none());
    }

    #[test]
    fn falls_back_to_named_fields() {
        let v: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":45.0,"resets_at":"2026-09-27T22:50:00+00:00"},"seven_day":{"utilization":16.0,"resets_at":null},"seven_day_opus":null}"#,
        )
        .unwrap();
        let l = parse(&v);
        assert_eq!(l.iter().map(|x| (x.label.as_str(), x.percent)).collect::<Vec<_>>(), [("Sessão 5h", 45.0), ("Semana", 16.0)]);
        assert_eq!(label("seven_day_cowork"), "Seven day cowork");
    }

    #[tokio::test]
    async fn missing_or_expired_login_is_explained() {
        let dir = std::env::temp_dir().join(format!("cockpit-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let client = http_client();
        let e = fetch(&client, &dir.join("nao-existe.json")).await.unwrap_err().to_string();
        assert!(e.contains("sem login"), "{e}");
        let p = dir.join("c.json");
        std::fs::write(&p, r#"{"claudeAiOauth":{"accessToken":"x","expiresAt":1000}}"#).unwrap();
        let e = fetch(&client, &p).await.unwrap_err().to_string();
        std::fs::remove_dir_all(&dir).ok();
        assert!(e.contains("venceu"), "vencido não chama a API: {e}");
    }
}
