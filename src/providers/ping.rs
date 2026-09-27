//! Latência de rede. Sem ICMP sem root (o WSL vem com `ping_group_range = 1 0`), mede o
//! tempo do handshake TCP até `host:porta`, o que acompanha de perto o RTT do ping.

use std::{collections::VecDeque, net::SocketAddr, time::Duration};

use tokio::{net::TcpStream, sync::watch, time::Instant};

use super::Snapshot;

const SAMPLES: usize = 30;
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PingSnapshot {
    pub target: String,
    /// Último RTT em ms; `None` se a última tentativa falhou.
    pub last_ms: Option<f64>,
    /// Histórico (mais antigo primeiro); `None` = perdido.
    pub hist: Vec<Option<f64>>,
    pub jitter_ms: Option<f64>,
    pub loss_pct: f64,
}

pub fn spawn(target: String, interval_secs: u64, tx: watch::Sender<Snapshot<PingSnapshot>>) {
    let target = if target.contains(':') { target } else { format!("{target}:443") };
    let every = Duration::from_secs(interval_secs.max(1));
    tokio::spawn(async move {
        let mut hist: VecDeque<Option<f64>> = VecDeque::with_capacity(SAMPLES);
        let mut addr: Option<SocketAddr> = None;
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if addr.is_none() {
                // Resolve uma vez; o DNS não entra na conta do RTT.
                addr = tokio::net::lookup_host(&target).await.ok().and_then(|mut a| a.next());
            }
            let rtt = match addr {
                Some(a) => probe(a).await,
                None => None,
            };
            if rtt.is_none() {
                addr = None; // tenta resolver de novo na próxima (rede mudou?)
            }
            if hist.len() == SAMPLES {
                hist.pop_front();
            }
            hist.push_back(rtt);
            let h: Vec<Option<f64>> = hist.iter().copied().collect();
            let snap = PingSnapshot {
                target: target.clone(),
                last_ms: rtt,
                jitter_ms: jitter(&h),
                loss_pct: loss(&h),
                hist: h,
            };
            let mut out = Snapshot::ok(snap);
            if rtt.is_none() {
                out.error = Some(format!("sem resposta de {target}"));
            }
            if tx.send(out).is_err() {
                return;
            }
        }
    });
}

async fn probe(addr: SocketAddr) -> Option<f64> {
    let start = Instant::now();
    match tokio::time::timeout(TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Some(start.elapsed().as_secs_f64() * 1000.0),
        _ => None,
    }
}

/// Média da diferença absoluta entre amostras consecutivas bem-sucedidas (como o RFC 3550, simplificado).
fn jitter(h: &[Option<f64>]) -> Option<f64> {
    let ok: Vec<f64> = h.iter().flatten().copied().collect();
    if ok.len() < 2 {
        return None;
    }
    Some(ok.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (ok.len() - 1) as f64)
}

fn loss(h: &[Option<f64>]) -> f64 {
    if h.is_empty() { 0.0 } else { h.iter().filter(|x| x.is_none()).count() as f64 * 100.0 / h.len() as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_and_loss() {
        let h = [Some(10.0), Some(14.0), None, Some(12.0)];
        assert_eq!(jitter(&h), Some(3.0));
        assert_eq!(loss(&h), 25.0);
        assert_eq!(jitter(&[Some(1.0)]), None);
        assert_eq!(loss(&[]), 0.0);
    }

    #[tokio::test]
    async fn probe_measures_local_listener() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { while l.accept().await.is_ok() {} });
        let rtt = probe(addr).await.unwrap();
        assert!((0.0..1000.0).contains(&rtt));
    }
}
