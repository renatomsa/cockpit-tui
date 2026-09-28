//! Teste de velocidade sob demanda (tecla `s`), contra os endpoints da Cloudflare:
//! baixa N MB de `__down` e envia N MB para `__up`. Nunca roda sozinho, para não gastar
//! banda. O download é medido em streaming e reporta a velocidade parcial ao vivo.

use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::config::NetworkConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Download,
    Upload,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeedResult {
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpeedEvent {
    /// Fase em andamento e a velocidade medida até agora (0 no começo).
    Progress(Phase, f64),
    Done(SpeedResult),
    Failed(String),
}

const DOWN: &str = "https://speed.cloudflare.com/__down";
const UP: &str = "https://speed.cloudflare.com/__up";

pub async fn run(cfg: NetworkConfig, send: impl Fn(SpeedEvent)) {
    match measure(&cfg, &send).await {
        Ok(r) => send(SpeedEvent::Done(r)),
        Err(e) => send(SpeedEvent::Failed(format!("{e:#}"))),
    }
}

fn mbps(bytes: u64, secs: f64) -> f64 {
    bytes as f64 * 8.0 / secs.max(1e-3) / 1e6
}

async fn measure(cfg: &NetworkConfig, send: &impl Fn(SpeedEvent)) -> Result<SpeedResult> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .user_agent(concat!("cockpit/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let down_bytes = cfg.speedtest_down_mb.clamp(1, 500) * 1_000_000;
    send(SpeedEvent::Progress(Phase::Download, 0.0));
    let mut resp = http.get(DOWN).query(&[("bytes", down_bytes)]).send().await?.error_for_status()?;
    // O relógio começa no primeiro byte: a latência de conexão não entra na conta.
    let mut first: Option<Instant> = None;
    let mut got: u64 = 0;
    let mut last_report = Instant::now();
    while let Some(chunk) = resp.chunk().await? {
        let now = Instant::now();
        let start = *first.get_or_insert(now);
        got += chunk.len() as u64;
        if now.duration_since(last_report) >= Duration::from_millis(250) {
            send(SpeedEvent::Progress(Phase::Download, mbps(got, now.duration_since(start).as_secs_f64())));
            last_report = now;
        }
    }
    let Some(start) = first else { bail!("o download veio vazio") };
    let down = mbps(got, start.elapsed().as_secs_f64());

    let up_bytes = cfg.speedtest_up_mb.clamp(1, 200) as usize * 1_000_000;
    send(SpeedEvent::Progress(Phase::Upload, 0.0));
    let body = vec![0u8; up_bytes];
    let t = Instant::now();
    http.post(UP).body(body).send().await?.error_for_status()?;
    let up = mbps(up_bytes as u64, t.elapsed().as_secs_f64());

    Ok(SpeedResult { down_mbps: down, up_mbps: up, at: Utc::now() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_megabits() {
        // 10 MB em 2 s = 40 Mbps
        assert!((mbps(10_000_000, 2.0) - 40.0).abs() < 1e-9);
        assert!(mbps(1, 0.0).is_finite());
    }
}
