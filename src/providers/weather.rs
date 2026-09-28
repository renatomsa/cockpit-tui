//! Clima da cidade atual pela Open-Meteo (sem chave): agora e as próximas horas.
//! Depende das coordenadas da geolocalização; rebusca quando a cidade muda.

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Deserialize;
use tokio::sync::watch;

use super::{Backoff, Snapshot, geo::Geo, http_client};
use crate::config::WeatherConfig;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weather {
    pub temp: f64,
    pub code: u8,
    pub is_day: bool,
    /// Próximas horas (a partir da hora cheia seguinte).
    pub hours: Vec<Hour>,
    /// Maior chance de chuva nas próximas 12 h: (hora local, %).
    pub rain_peak: Option<(u32, u8)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hour {
    /// Hora local (0–23).
    pub hour: u32,
    pub temp: f64,
    pub precip: Option<u8>,
    pub code: u8,
}

pub fn spawn(cfg: WeatherConfig, mut geo: watch::Receiver<Snapshot<Geo>>, tx: watch::Sender<Snapshot<Weather>>) {
    if !cfg.enabled {
        return;
    }
    tokio::spawn(async move {
        let http = http_client();
        let every = Duration::from_secs(cfg.refresh_minutes.max(5) * 60);
        let mut backoff = Backoff::new(Duration::from_secs(60), Duration::from_secs(900));
        loop {
            let coords = geo.borrow().data.as_ref().and_then(|g| Some((g.lat?, g.lon?)));
            let delay = match coords {
                None => Duration::from_secs(3600), // sem coordenadas: espera a geo mudar
                Some((lat, lon)) => match fetch(&http, lat, lon).await {
                    Ok(w) => {
                        backoff.reset();
                        tx.send_replace(Snapshot::ok(w));
                        every
                    }
                    Err(e) => {
                        tracing::info!("clima: {e:#}");
                        tx.send_modify(|s| s.error = Some(format!("{e:#}")));
                        backoff.next_delay()
                    }
                },
            };
            if tx.is_closed() {
                return;
            }
            // acorda no prazo ou quando a cidade mudar
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                r = geo.changed() => if r.is_err() { tokio::time::sleep(delay).await },
            }
        }
    });
}

#[derive(Deserialize)]
struct Resp {
    current: Current,
    hourly: Hourly,
}

#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: f64,
    weather_code: u8,
    #[serde(default)]
    is_day: u8,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<Option<f64>>,
    precipitation_probability: Vec<Option<u8>>,
    weather_code: Vec<Option<u8>>,
}

async fn fetch(http: &reqwest::Client, lat: f64, lon: f64) -> Result<Weather> {
    let url = reqwest::Url::parse_with_params(
        "https://api.open-meteo.com/v1/forecast",
        &[
            ("latitude", format!("{lat:.4}")),
            ("longitude", format!("{lon:.4}")),
            ("current", "temperature_2m,weather_code,is_day".into()),
            ("hourly", "temperature_2m,precipitation_probability,weather_code".into()),
            ("forecast_hours", "14".into()),
            ("timezone", "auto".into()),
        ],
    )?;
    let r: Resp = http.get(url).send().await?.error_for_status()?.json().await?;
    parse(r).ok_or_else(|| anyhow!("resposta incompleta da Open-Meteo"))
}

/// "2026-09-27T23:00" → 23
fn hour_of(t: &str) -> Option<u32> {
    t.split_once('T')?.1.get(..2)?.parse().ok()
}

fn parse(r: Resp) -> Option<Weather> {
    let now_hour = hour_of(&r.current.time)?;
    let h = &r.hourly;
    // As horas vêm em hora local, começando pela hora corrente: pula a atual.
    let start = h.time.iter().position(|t| hour_of(t) != Some(now_hour)).unwrap_or(0);
    let hours: Vec<Hour> = (start..h.time.len())
        .filter_map(|i| {
            Some(Hour {
                hour: hour_of(&h.time[i])?,
                temp: h.temperature_2m.get(i).copied().flatten()?,
                precip: h.precipitation_probability.get(i).copied().flatten(),
                code: h.weather_code.get(i).copied().flatten().unwrap_or(0),
            })
        })
        .collect();
    let rain_peak =
        hours.iter().take(12).filter_map(|x| Some((x.hour, x.precip?))).filter(|(_, p)| *p >= 20).max_by_key(|(_, p)| *p);
    Some(Weather { temp: r.current.temperature_2m, code: r.current.weather_code, is_day: r.current.is_day == 1, hours, rain_peak })
}

/// Descrição curta do código WMO que a Open-Meteo usa.
pub fn describe(code: u8, is_day: bool) -> &'static str {
    match code {
        0 if is_day => "Céu limpo",
        0 => "Noite limpa",
        1 => "Predomínio de sol",
        2 => "Parcialmente nublado",
        3 => "Nublado",
        45 | 48 => "Neblina",
        51 | 53 | 55 => "Garoa",
        56 | 57 => "Garoa congelante",
        61 => "Chuva fraca",
        63 => "Chuva",
        65 => "Chuva forte",
        66 | 67 => "Chuva congelante",
        71 | 73 | 75 | 77 => "Neve",
        80 | 81 => "Pancadas de chuva",
        82 => "Pancadas fortes",
        85 | 86 => "Pancadas de neve",
        95 => "Trovoadas",
        96 | 99 => "Trovoadas com granizo",
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forecast_and_rain_peak() {
        let json = r#"{"current":{"time":"2026-09-27T23:00","temperature_2m":24.3,"weather_code":51,"is_day":0},
          "hourly":{"time":["2026-09-27T23:00","2026-09-28T00:00","2026-09-28T01:00","2026-09-28T02:00"],
                    "temperature_2m":[24.3,24.2,24.1,null],"precipitation_probability":[82,86,87,85],"weather_code":[51,51,61,51]}}"#;
        let w = parse(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!((w.temp, w.code, w.is_day), (24.3, 51, false));
        assert_eq!(w.hours.iter().map(|h| h.hour).collect::<Vec<_>>(), [0, 1], "pula a hora atual e a sem temperatura");
        assert_eq!(w.rain_peak, Some((1, 87)));
        assert_eq!(describe(w.code, w.is_day), "Garoa");
        assert_eq!(describe(0, false), "Noite limpa");
    }
}
