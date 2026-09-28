//! Rede do Windows por trás do WSL: o Wi-Fi de verdade (SSID, sinal, canal) e o tráfego
//! da máquina inteira, que o `eth0` do Linux não vê.
//!
//! Abrir um `powershell.exe` custa ~1 s, então um único PowerShell fica aberto em loop:
//! a cada 2 s imprime os contadores dos adaptadores (API .NET, barata) e, a cada 10 s,
//! a saída do `netsh wlan show interfaces`. Morre junto com o cockpit.

use std::{collections::HashMap, time::Duration};

use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::watch,
    time::Instant,
};

use super::Snapshot;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostNet {
    /// Adaptador principal (o de mais tráfego; Wi-Fi no empate).
    pub adapter: String,
    pub wireless: bool,
    /// Bytes por segundo, da máquina inteira.
    pub rx: f64,
    pub tx: f64,
    /// Velocidade do link em Mbps (a que o adaptador negociou).
    pub link_mbps: Option<f64>,
    pub wifi: Option<Wifi>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub signal: Option<u8>,
    pub rssi: Option<i32>,
    pub channel: Option<u32>,
    pub radio: Option<String>,
    pub band: Option<String>,
    pub rx_mbps: Option<f64>,
    pub tx_mbps: Option<f64>,
}

impl Wifi {
    /// "5 GHz", do campo Faixa/Band ou deduzida do canal.
    pub fn band_label(&self) -> Option<String> {
        if let Some(b) = &self.band {
            return Some(b.clone());
        }
        self.channel.map(|c| if c <= 14 { "2,4 GHz".into() } else { "5 GHz".into() })
    }
}

const SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
$i=0
while ($true) {
  $a=@([System.Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces() | Where-Object {
      $_.OperationalStatus -eq 'Up' -and ($_.NetworkInterfaceType -eq 'Wireless80211' -or $_.NetworkInterfaceType -eq 'Ethernet') -and
      $_.Name -notlike 'vEthernet*' -and $_.Description -notlike '*Hyper-V*' -and $_.Description -notlike '*Virtual*' -and $_.Description -notlike '*VPN*'
    } | ForEach-Object { $s=$_.GetIPStatistics(); [pscustomobject]@{name=$_.Name; kind=[string]$_.NetworkInterfaceType; rx=$s.BytesReceived; tx=$s.BytesSent; speed=$_.Speed} })
  $w=$null
  if ($i % 5 -eq 0) { $w=(netsh wlan show interfaces) -join "`n" }
  $j=[pscustomobject]@{adapters=$a; wlan=$w} | ConvertTo-Json -Compress -Depth 4
  [regex]::Replace($j,'[^\x00-\x7F]',{ param($m) '\u{0:x4}' -f [int][char]$m.Value })
  $i++
  Start-Sleep -Seconds 2
}"#;

/// `-EncodedCommand` recebe o script em UTF-16LE + base64: sem problema de aspas.
fn encoded() -> String {
    let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    crate::shell::base64(&utf16)
}

pub fn spawn(tx: watch::Sender<Snapshot<HostNet>>) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run(&tx).await {
                tracing::info!("rede do Windows: {e:#}");
                tx.send_modify(|s| s.error = Some(format!("{e:#}")));
            }
            if tx.is_closed() {
                return;
            }
            // o PowerShell morreu (suspensão, WSL reiniciado…): tenta de novo
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

async fn run(tx: &watch::Sender<Snapshot<HostNet>>) -> anyhow::Result<()> {
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let out = child.stdout.take().ok_or_else(|| anyhow::anyhow!("sem stdout"))?;
    let mut lines = BufReader::new(out).lines();
    let mut prev: HashMap<String, (Instant, u64, u64)> = HashMap::new();
    let mut wifi: Option<Wifi> = None;
    while let Some(line) = lines.next_line().await? {
        let Some(sample) = parse_sample(&line) else { continue };
        if let Some(text) = &sample.wlan {
            wifi = parse_netsh(text);
        }
        let now = Instant::now();
        let mut best: Option<HostNet> = None;
        for a in &sample.adapters {
            let rates = prev.get(&a.name).map(|(t, rx, tx)| {
                let dt = now.duration_since(*t).as_secs_f64().max(0.001);
                (a.rx.saturating_sub(*rx) as f64 / dt, a.tx.saturating_sub(*tx) as f64 / dt)
            });
            prev.insert(a.name.clone(), (now, a.rx, a.tx));
            let Some((rx, txr)) = rates else { continue };
            let wireless = a.kind == "Wireless80211";
            let cand = HostNet {
                adapter: a.name.clone(),
                wireless,
                rx,
                tx: txr,
                link_mbps: a.speed.filter(|s| *s > 0).map(|s| s as f64 / 1e6),
                wifi: if wireless { wifi.clone() } else { None },
            };
            let better = match &best {
                None => true,
                Some(b) => (cand.rx + cand.tx, cand.wireless) > (b.rx + b.tx, b.wireless),
            };
            if better {
                best = Some(cand);
            }
        }
        if let Some(b) = best
            && tx.send(Snapshot::ok(b)).is_err()
        {
            return Ok(()); // a UI fechou; o kill_on_drop encerra o PowerShell
        }
    }
    anyhow::bail!("o PowerShell parou de responder")
}

#[derive(Deserialize)]
struct Sample {
    #[serde(default, deserialize_with = "one_or_many")]
    adapters: Vec<Adapter>,
    wlan: Option<String>,
}

#[derive(Deserialize)]
struct Adapter {
    name: String,
    kind: String,
    rx: u64,
    tx: u64,
    speed: Option<u64>,
}

/// O PowerShell 5 às vezes serializa uma lista de um item como objeto solto.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Adapter>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<Adapter>),
        One(Adapter),
        Null(()),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::Many(v) => v,
        OneOrMany::One(a) => vec![a],
        OneOrMany::Null(()) => Vec::new(),
    })
}

fn parse_sample(line: &str) -> Option<Sample> {
    serde_json::from_str(line.trim()).ok()
}

/// `netsh wlan show interfaces`, em português ou inglês. Os rótulos são comparados pelo
/// começo, só com ASCII: os acentos chegam numa codificação que varia com o Windows.
fn parse_netsh(text: &str) -> Option<Wifi> {
    let mut w = Wifi::default();
    let mut connected = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let key = k.trim().to_lowercase();
        let v = v.trim();
        let num = |s: &str| s.trim_end_matches('%').trim().replace(',', ".").parse::<f64>().ok();
        match () {
            _ if key == "ssid" && w.ssid.is_empty() => w.ssid = v.to_string(),
            _ if key.starts_with("estado") || key == "state" => {
                connected = v.to_lowercase().starts_with("conectado") || v.to_lowercase() == "connected"
            }
            _ if key.starts_with("sinal") || key == "signal" => w.signal = num(v).map(|x| x as u8),
            _ if key == "rssi" => w.rssi = v.parse().ok(),
            _ if key.starts_with("canal") || key == "channel" => w.channel = v.parse().ok(),
            _ if key.starts_with("tipo de r") || key == "radio type" => w.radio = Some(v.to_string()),
            _ if key.starts_with("faixa") || key == "band" => w.band = Some(v.replace('.', ",")),
            _ if key.starts_with("taxa de recep") || key.starts_with("receive rate") => w.rx_mbps = num(v),
            _ if key.starts_with("taxa de transmiss") || key.starts_with("transmit rate") => w.tx_mbps = num(v),
            _ => {}
        }
    }
    (connected && !w.ssid.is_empty()).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PT: &str = "\n Há 1 interface no sistema:\n\n    Nome                   : Wi-Fi\n    Descri\u{fffd}\u{fffd}o            : Intel(R) Wireless-AC 9462\n    Estado                  : Conectado\n    SSID                   : RMB2601\n    AP BSSID               : aa:bb\n    Tipo de r\u{fffd}dio             : 802.11ac\n    Canal                : 52\n    Taxa de recep\u{fffd}\u{fffd}o (Mbps)  : 325\n    Taxa de transmiss\u{fffd}o (Mbps) : 390\n    Sinal                 : 85% \n    Rssi                   : -55\n";

    #[test]
    fn parses_netsh_in_portuguese_with_broken_accents() {
        let w = parse_netsh(PT).unwrap();
        assert_eq!(w.ssid, "RMB2601");
        assert_eq!((w.signal, w.rssi, w.channel), (Some(85), Some(-55), Some(52)));
        assert_eq!(w.radio.as_deref(), Some("802.11ac"));
        assert_eq!((w.rx_mbps, w.tx_mbps), (Some(325.0), Some(390.0)));
        assert_eq!(w.band_label().as_deref(), Some("5 GHz"));
    }

    #[test]
    fn parses_netsh_in_english_and_disconnected() {
        let en = "    Name : Wi-Fi\n    State : connected\n    SSID : Casa\n    Band : 2.4 GHz\n    Channel : 6\n    Signal : 60%\n";
        let w = parse_netsh(en).unwrap();
        assert_eq!((w.ssid.as_str(), w.band.as_deref(), w.signal), ("Casa", Some("2,4 GHz"), Some(60)));
        assert!(parse_netsh("    Estado : Desconectado\n").is_none());
    }

    #[test]
    fn parses_samples_with_one_or_many_adapters() {
        let many = r#"{"adapters":[{"name":"Wi-Fi","kind":"Wireless80211","rx":1652295989,"tx":188092029,"speed":325000000}],"wlan":null}"#;
        assert_eq!(parse_sample(many).unwrap().adapters.len(), 1);
        let one = r#"{"adapters":{"name":"Ethernet","kind":"Ethernet","rx":1,"tx":2,"speed":1000000000},"wlan":"x"}"#;
        let s = parse_sample(one).unwrap();
        assert_eq!((s.adapters.len(), s.wlan.as_deref()), (1, Some("x")));
        let none = r#"{"adapters":null,"wlan":null}"#;
        assert!(parse_sample(none).unwrap().adapters.is_empty());
        assert!(!encoded().is_empty());
    }
}
