//! Cidade, país, coordenadas e fuso atuais.
//! Ordem: `city` fixa no config (sem rede) → cache em disco (abre instantâneo) → IP.

use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::{Backoff, Snapshot, http_client};
use crate::config::LocationConfig;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Geo {
    pub city: String,
    pub region: Option<String>,
    pub country: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub timezone: Option<String>,
    pub source: GeoSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeoSource {
    Config,
    Cache,
    Ip(String),
}

impl GeoSource {
    pub fn label(&self) -> &str {
        match self {
            GeoSource::Config => "config",
            GeoSource::Cache => "cache",
            GeoSource::Ip(s) => s.as_str(),
        }
    }
}

pub fn spawn(cfg: LocationConfig, cache_file: PathBuf, tx: watch::Sender<Snapshot<Geo>>) {
    if let Some(city) = cfg.city.clone() {
        tx.send_replace(Snapshot::ok(Geo {
            city,
            region: cfg.region,
            country: cfg.country,
            lat: cfg.lat,
            lon: cfg.lon,
            timezone: cfg.timezone,
            source: GeoSource::Config,
        }));
        return;
    }

    tokio::spawn(async move {
        if let Some(mut cached) = read_cache(&cache_file) {
            cached.source = GeoSource::Cache;
            tx.send_replace(Snapshot::ok(cached));
        }
        let client = http_client();
        let refresh = Duration::from_secs(cfg.refresh_minutes.max(5) * 60);
        let mut backoff = Backoff::new(Duration::from_secs(30), Duration::from_secs(300));
        loop {
            let delay = match locate(&client).await {
                Ok(mut geo) => {
                    backoff.reset();
                    // Um fuso fixo no config vale mais que o do IP (VPN costuma mentir).
                    if cfg.timezone.is_some() {
                        geo.timezone = cfg.timezone.clone();
                    }
                    write_cache(&cache_file, &geo);
                    tx.send_replace(Snapshot::ok(geo));
                    refresh
                }
                Err(e) => {
                    tracing::warn!("geolocalização falhou: {e:#}");
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

async fn locate(client: &reqwest::Client) -> Result<Geo> {
    match ipapi_co(client).await {
        Ok(g) => Ok(g),
        Err(e1) => ipinfo(client).await.map_err(|e2| anyhow!("ipapi.co: {e1:#}; ipinfo.io: {e2:#}")),
    }
}

#[derive(Deserialize)]
struct IpapiCo {
    #[serde(default)]
    error: bool,
    reason: Option<String>,
    city: Option<String>,
    region: Option<String>,
    country_name: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<String>,
}

async fn ipapi_co(client: &reqwest::Client) -> Result<Geo> {
    let r: IpapiCo = client.get("https://ipapi.co/json/").send().await?.error_for_status()?.json().await?;
    if r.error {
        bail!("{}", r.reason.unwrap_or_else(|| "erro".into()));
    }
    Ok(Geo {
        city: r.city.filter(|c| !c.is_empty()).ok_or_else(|| anyhow!("sem cidade"))?,
        region: r.region,
        country: r.country_name,
        lat: r.latitude,
        lon: r.longitude,
        timezone: r.timezone,
        source: GeoSource::Ip("ipapi.co".into()),
    })
}

#[derive(Deserialize)]
struct IpInfo {
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
    loc: Option<String>,
    timezone: Option<String>,
}

async fn ipinfo(client: &reqwest::Client) -> Result<Geo> {
    let r: IpInfo = client.get("https://ipinfo.io/json").send().await?.error_for_status()?.json().await?;
    let (lat, lon) = parse_loc(r.loc.as_deref());
    Ok(Geo {
        city: r.city.filter(|c| !c.is_empty()).ok_or_else(|| anyhow!("sem cidade"))?,
        region: r.region,
        country: r.country.map(|c| country_name(&c).unwrap_or(&c).to_string()),
        lat,
        lon,
        timezone: r.timezone,
        source: GeoSource::Ip("ipinfo.io".into()),
    })
}

fn parse_loc(loc: Option<&str>) -> (Option<f64>, Option<f64>) {
    let Some((a, b)) = loc.and_then(|l| l.split_once(',')) else { return (None, None) };
    (a.trim().parse().ok(), b.trim().parse().ok())
}

/// O ipinfo devolve só o código ISO; os mais prováveis ganham nome.
fn country_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "BR" => "Brazil",
        "US" => "United States",
        "PT" => "Portugal",
        "AR" => "Argentina",
        "CL" => "Chile",
        "MX" => "Mexico",
        "CA" => "Canada",
        "GB" => "United Kingdom",
        "DE" => "Germany",
        "ES" => "Spain",
        "FR" => "France",
        "IT" => "Italy",
        _ => return None,
    })
}

fn read_cache(path: &PathBuf) -> Option<Geo> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &PathBuf, geo: &Geo) {
    let res = path
        .parent()
        .map(fs::create_dir_all)
        .transpose()
        .map_err(anyhow::Error::from)
        .and_then(|_| Ok(fs::write(path, serde_json::to_string(geo)?)?));
    if let Err(e) = res {
        tracing::warn!("não gravei cache de geo: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipinfo_loc() {
        assert_eq!(parse_loc(Some("-22.9068,-43.1729")), (Some(-22.9068), Some(-43.1729)));
        assert_eq!(parse_loc(Some("lixo")), (None, None));
        assert_eq!(parse_loc(None), (None, None));
    }

    #[test]
    fn cache_roundtrip() {
        let p = std::env::temp_dir().join(format!("cockpit-geo-{}.json", std::process::id()));
        let g = Geo {
            city: "Rio de Janeiro".into(),
            region: None,
            country: Some("Brazil".into()),
            lat: Some(-22.9),
            lon: Some(-43.2),
            timezone: Some("America/Sao_Paulo".into()),
            source: GeoSource::Ip("ipapi.co".into()),
        };
        write_cache(&p, &g);
        let back = read_cache(&p);
        fs::remove_file(&p).ok();
        assert_eq!(back.unwrap().city, "Rio de Janeiro");
    }

    #[tokio::test]
    async fn fixed_city_skips_network() {
        let (tx, rx) = watch::channel(Snapshot::default());
        let cfg = LocationConfig { city: Some("Lisboa".into()), timezone: Some("Europe/Lisbon".into()), ..Default::default() };
        spawn(cfg, PathBuf::from("/nao/existe"), tx);
        let s = rx.borrow();
        let g = s.data.as_ref().unwrap();
        assert_eq!(g.city, "Lisboa");
        assert_eq!(g.source, GeoSource::Config);
    }
}
