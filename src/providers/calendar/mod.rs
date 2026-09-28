//! Google Calendar: próximos eventos das agendas visíveis, lidos pela API v3 a cada minuto.
//! A contagem regressiva e o alerta são recalculados pela UI a cada segundo, sem rede.

pub mod ics;
pub mod oauth;

use std::{collections::HashSet, path::PathBuf, time::Duration};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use tokio::sync::{RwLock, watch};

use super::{Backoff, Snapshot, http_client};
use crate::config::CalendarConfig;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalendarSnapshot {
    /// Ordenados pelo início.
    pub events: Vec<Event>,
    pub calendars: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    pub id: String,
    pub calendar: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    /// Link da videochamada (Meet, Zoom, Teams…), se houver.
    pub meet: Option<String>,
    /// Link do evento no Google Calendar.
    pub html_link: Option<String>,
    pub location: Option<String>,
    pub declined: bool,
}

impl Event {
    /// Identifica a ocorrência (eventos recorrentes repetem o id base em outras datas).
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.start.timestamp())
    }

    pub fn link(&self) -> Option<&str> {
        self.meet.as_deref().or(self.html_link.as_deref())
    }
}

/// Por que não há agenda: cada caso pede uma ação diferente do usuário.
pub fn not_connected_hint(err: &str) -> bool {
    err.starts_with("não conectado")
}

pub fn load_ical_store(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_ical_store(path: &std::path::Path, list: &[String]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(serde_json::to_string_pretty(list)?.as_bytes())?;
    Ok(())
}

/// `webcal://` vira `https://`; caminhos com `~` são expandidos.
pub fn normalize_source(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => s.to_string(),
    }
}

/// Baixa (ou lê) e interpreta um endereço iCal.
pub async fn fetch_ical(http: &reqwest::Client, source: &str, from: DateTime<Utc>, until: DateTime<Utc>) -> Result<(String, Vec<Event>)> {
    let src = normalize_source(source);
    let text = if src.starts_with("http://") || src.starts_with("https://") {
        let resp = http.get(&src).send().await.map_err(|e| anyhow!("{}: {e}", ics::describe_address(&src)))?;
        if !resp.status().is_success() {
            bail!("{} respondeu {}", ics::describe_address(&src), resp.status());
        }
        resp.text().await?
    } else {
        let p = crate::config::expand_home(&src);
        tokio::fs::read_to_string(&p).await.map_err(|e| anyhow!("{}: {e}", p.display()))?
    };
    if !text.contains("BEGIN:VCALENDAR") {
        bail!("{} não é um arquivo iCal", ics::describe_address(&src));
    }
    let owner = ics::owner_from_address(&src);
    let system: chrono_tz::Tz = iana_time_zone::get_timezone().ok().and_then(|n| n.parse().ok()).unwrap_or(chrono_tz::UTC);
    let r = tokio::task::spawn_blocking(move || ics::events(&text, from, until, system, owner.as_deref())).await?;
    Ok(r)
}

async fn fetch_all_ical(http: &reqwest::Client, cfg: &CalendarConfig, sources: &[String]) -> Result<(CalendarSnapshot, Option<String>)> {
    let now = Utc::now();
    let until = now + chrono::Duration::hours(cfg.lookahead_hours.clamp(1, 24 * 14));
    let mut events = Vec::new();
    let mut calendars = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for s in sources {
        match fetch_ical(http, s, now, until).await {
            Ok((name, evs)) => {
                calendars.push(if name.is_empty() { ics::describe_address(s) } else { name });
                for e in evs {
                    if (cfg.show_declined || !e.declined) && seen.insert(e.key()) {
                        events.push(e);
                    }
                }
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if calendars.is_empty() {
        bail!("{}", errors.join("; "));
    }
    events.sort_by(|a, b| a.start.cmp(&b.start).then(a.all_day.cmp(&b.all_day).reverse()));
    let partial = (!errors.is_empty()).then(|| format!("{} de {} agendas falharam: {}", errors.len(), sources.len(), errors.join("; ")));
    Ok((CalendarSnapshot { events, calendars }, partial))
}

pub fn spawn(cfg: CalendarConfig, token_path: PathBuf, ical_store: PathBuf, tx: watch::Sender<Snapshot<CalendarSnapshot>>) {
    if !cfg.enabled {
        return;
    }
    tokio::spawn(async move {
        let http = http_client();
        // Feeds .ics do Google podem ter anos de eventos: prazo maior que o padrão.
        let ical_http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_else(|_| http.clone());
        let token = RwLock::new(None::<oauth::Token>);
        let mut backoff = Backoff::new(Duration::from_secs(30), Duration::from_secs(600));
        loop {
            // Relido a cada volta: um `cockpit calendar add` em outro terminal vale sozinho.
            let mut sources: Vec<String> = cfg.ical.clone();
            sources.extend(load_ical_store(&ical_store));
            let result = if sources.is_empty() {
                fetch(&http, &cfg, &token_path, &token).await.map(|s| (s, None, Duration::from_secs(cfg.refresh_secs.max(30))))
            } else {
                fetch_all_ical(&ical_http, &cfg, &sources).await.map(|(s, p)| (s, p, Duration::from_secs(cfg.ical_refresh_secs.max(60))))
            };
            let delay = match result {
                Ok((snap, partial, every)) => {
                    backoff.reset();
                    let mut out = Snapshot::ok(snap);
                    out.error = partial;
                    tx.send_replace(out);
                    every
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    tracing::info!("calendar: {msg}");
                    // Sem login, relê o arquivo a cada 15 s: pega o `cockpit auth google`
                    // rodado em outro terminal sem precisar reabrir o cockpit.
                    let wait = if not_connected_hint(&msg) { Duration::from_secs(15) } else { backoff.next_delay() };
                    tx.send_modify(|s| s.error = Some(msg));
                    wait
                }
            };
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(delay).await;
        }
    });
}

async fn fetch(
    http: &reqwest::Client,
    cfg: &CalendarConfig,
    token_path: &std::path::Path,
    cache: &RwLock<Option<oauth::Token>>,
) -> Result<CalendarSnapshot> {
    let client_path = oauth::client_path(cfg);
    let client = oauth::load_client(&client_path)
        .map_err(|_| anyhow!("não conectado: rode `cockpit calendar add '<endereço iCal>'` (veja o README)"))?;
    let mut t = match cache.read().await.clone().or_else(|| oauth::load_token(token_path)) {
        Some(t) => t,
        None => bail!("não conectado: rode `cockpit auth google` (ou use um endereço iCal: `cockpit calendar add`)"),
    };
    if !t.fresh() {
        t = oauth::refresh(http, &client, &t).await?;
        oauth::save_token(token_path, &t)?;
    }
    *cache.write().await = Some(t.clone());

    let cals = calendar_list(http, &t.access_token).await?;
    let wanted: Vec<&CalendarItem> = cals
        .iter()
        .filter(|c| {
            let name = c.name();
            let matches = |list: &[String]| list.iter().any(|x| *x == c.id || x.eq_ignore_ascii_case(name));
            let included = if cfg.calendars.is_empty() { c.selected.unwrap_or(false) || c.primary.unwrap_or(false) } else { matches(&cfg.calendars) };
            included && !matches(&cfg.exclude)
        })
        .collect();

    let now = Utc::now();
    // timeMin filtra pelo FIM do evento: com `now`, as reuniões em andamento também vêm.
    let time_min = now;
    let time_max = now + chrono::Duration::hours(cfg.lookahead_hours.clamp(1, 24 * 14));
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    for c in &wanted {
        for e in list_events(http, &t.access_token, &c.id, time_min, time_max).await? {
            if let Some(ev) = e.into_event(c.name())
                && (cfg.show_declined || !ev.declined)
                && seen.insert(ev.key())
            {
                events.push(ev);
            }
        }
    }
    events.sort_by(|a, b| a.start.cmp(&b.start).then(a.all_day.cmp(&b.all_day).reverse()));
    Ok(CalendarSnapshot { events, calendars: wanted.iter().map(|c| c.name().to_string()).collect() })
}

const API: &str = "https://www.googleapis.com/calendar/v3";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarItem {
    id: String,
    summary: Option<String>,
    summary_override: Option<String>,
    selected: Option<bool>,
    primary: Option<bool>,
}

impl CalendarItem {
    fn name(&self) -> &str {
        self.summary_override.as_deref().or(self.summary.as_deref()).unwrap_or(&self.id)
    }
}

async fn get_json<T: for<'de> Deserialize<'de>>(http: &reqwest::Client, token: &str, url: reqwest::Url) -> Result<T> {
    let resp = http.get(url).bearer_auth(token).send().await?;
    let status = resp.status();
    if status.as_u16() == 401 {
        bail!("o Google recusou o token; rode `cockpit auth google` de novo");
    }
    if status.as_u16() == 403 {
        let body = resp.text().await.unwrap_or_default();
        if body.contains("accessNotConfigured") || body.contains("SERVICE_DISABLED") {
            bail!("a Google Calendar API não está ativada no projeto do Google Cloud");
        }
        bail!("sem permissão (403)");
    }
    if !status.is_success() {
        bail!("Google Calendar respondeu {status}");
    }
    Ok(resp.json().await?)
}

async fn calendar_list(http: &reqwest::Client, token: &str) -> Result<Vec<CalendarItem>> {
    #[derive(Deserialize)]
    struct List {
        #[serde(default)]
        items: Vec<CalendarItem>,
    }
    let url = reqwest::Url::parse_with_params(
        &format!("{API}/users/me/calendarList"),
        &[("minAccessRole", "reader"), ("fields", "items(id,summary,summaryOverride,selected,primary)")],
    )?;
    Ok(get_json::<List>(http, token, url).await?.items)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ApiEvent {
    id: String,
    status: Option<String>,
    summary: Option<String>,
    start: Option<ApiTime>,
    end: Option<ApiTime>,
    hangout_link: Option<String>,
    html_link: Option<String>,
    location: Option<String>,
    event_type: Option<String>,
    conference_data: Option<ConferenceData>,
    attendees: Vec<Attendee>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ApiTime {
    date_time: Option<DateTime<chrono::FixedOffset>>,
    date: Option<NaiveDate>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ConferenceData {
    entry_points: Vec<EntryPoint>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct EntryPoint {
    entry_point_type: String,
    uri: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Attendee {
    #[serde(rename = "self")]
    is_self: bool,
    response_status: Option<String>,
}

impl ApiEvent {
    fn into_event(self, calendar: &str) -> Option<Event> {
        if self.status.as_deref() == Some("cancelled") || self.event_type.as_deref() == Some("workingLocation") {
            return None;
        }
        let at = |t: &ApiTime| -> Option<(DateTime<Utc>, bool)> {
            if let Some(dt) = t.date_time {
                return Some((dt.with_timezone(&Utc), false));
            }
            let d = t.date?;
            let local = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
            Some((local.with_timezone(&Utc), true))
        };
        let (start, all_day) = at(self.start.as_ref()?)?;
        let (end, _) = at(self.end.as_ref()?)?;
        // Link da chamada: o do Meet, senão o primeiro ponto de entrada de vídeo (Zoom, Teams…).
        let meet = self.hangout_link.clone().or_else(|| {
            self.conference_data.as_ref()?.entry_points.iter().find(|e| e.entry_point_type == "video").map(|e| e.uri.clone())
        });
        let meet = meet.or_else(|| self.location.as_deref().and_then(find_url));
        let declined = self.attendees.iter().any(|a| a.is_self && a.response_status.as_deref() == Some("declined"));
        Some(Event {
            id: self.id,
            calendar: calendar.to_string(),
            title: self.summary.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "(sem título)".into()),
            start,
            end,
            all_day,
            meet,
            html_link: self.html_link,
            location: self.location,
            declined,
        })
    }
}

/// Link de reunião escrito no campo "local" (comum com Zoom/Teams).
fn find_url(s: &str) -> Option<String> {
    s.split_whitespace().find(|w| w.starts_with("https://")).map(|w| w.trim_end_matches([',', ';', ')']).to_string())
}

async fn list_events(http: &reqwest::Client, token: &str, cal: &str, min: DateTime<Utc>, max: DateTime<Utc>) -> Result<Vec<ApiEvent>> {
    #[derive(Deserialize)]
    struct List {
        #[serde(default)]
        items: Vec<ApiEvent>,
    }
    let mut url = reqwest::Url::parse(API)?;
    url.path_segments_mut().map_err(|_| anyhow!("url"))?.extend(["calendars", cal, "events"]);
    url.query_pairs_mut()
        .append_pair("singleEvents", "true")
        .append_pair("orderBy", "startTime")
        .append_pair("timeMin", &min.to_rfc3339())
        .append_pair("timeMax", &max.to_rfc3339())
        .append_pair("maxResults", "100")
        .append_pair(
            "fields",
            "items(id,status,summary,start,end,hangoutLink,htmlLink,location,eventType,conferenceData/entryPoints(entryPointType,uri),attendees(self,responseStatus))",
        );
    Ok(get_json::<List>(http, token, url).await?.items)
}

/// Eventos falsos para ver o alerta sem conectar o Google (`cockpit --demo-agenda`).
pub fn demo(now: DateTime<Utc>) -> CalendarSnapshot {
    let m = |min: i64| now + chrono::Duration::minutes(min);
    let today = Local::now().date_naive();
    let midnight = Local.from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap().with_timezone(&Utc);
    let ev = |id: &str, title: &str, start, end, meet: bool| Event {
        id: id.into(),
        calendar: "demo".into(),
        title: title.into(),
        start,
        end,
        all_day: false,
        meet: meet.then(|| "https://meet.google.com/abc-defg-hij".into()),
        html_link: Some("https://calendar.google.com/".into()),
        location: None,
        declined: false,
    };
    let mut all_day = ev("d0", "Aniversário da empresa", midnight, midnight + chrono::Duration::days(1), false);
    all_day.all_day = true;
    CalendarSnapshot {
        events: vec![
            all_day,
            ev("d1", "Conhecimento fiscal & Reforma Tributária | Saiba+", m(4), m(64), true),
            ev("d2", "Daily do time", m(95), m(110), true),
            ev("d3", "Planning da sprint", m(60 * 20), m(60 * 21), true),
        ],
        calendars: vec!["demo".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_api_events() {
        let json = r#"[
          {"id":"a","summary":"Daily","start":{"dateTime":"2026-09-28T09:00:00-03:00"},"end":{"dateTime":"2026-09-28T09:15:00-03:00"},
           "hangoutLink":"https://meet.google.com/xyz","attendees":[{"self":true,"responseStatus":"accepted"}]},
          {"id":"b","summary":"Recusada","start":{"dateTime":"2026-09-28T10:00:00Z"},"end":{"dateTime":"2026-09-28T11:00:00Z"},
           "attendees":[{"email":"x"},{"self":true,"responseStatus":"declined"}]},
          {"id":"c","status":"cancelled","start":{"dateTime":"2026-09-28T10:00:00Z"},"end":{"dateTime":"2026-09-28T11:00:00Z"}},
          {"id":"d","summary":"Feriado","start":{"date":"2026-10-12"},"end":{"date":"2026-10-13"}},
          {"id":"e","summary":"Zoom","location":"Sala 3, https://zoom.us/j/123;","start":{"dateTime":"2026-09-28T12:00:00Z"},"end":{"dateTime":"2026-09-28T13:00:00Z"},
           "conferenceData":{"entryPoints":[{"entryPointType":"phone","uri":"tel:+1"}]}},
          {"id":"f","eventType":"workingLocation","start":{"date":"2026-09-28"},"end":{"date":"2026-09-29"}}
        ]"#;
        let raw: Vec<ApiEvent> = serde_json::from_str(json).unwrap();
        let evs: Vec<Event> = raw.into_iter().filter_map(|e| e.into_event("Trabalho")).collect();
        assert_eq!(evs.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a", "b", "d", "e"]);
        assert_eq!(evs[0].start.to_rfc3339(), "2026-09-28T12:00:00+00:00");
        assert_eq!(evs[0].meet.as_deref(), Some("https://meet.google.com/xyz"));
        assert!(evs[1].declined && !evs[0].declined);
        assert!(evs[2].all_day);
        assert_eq!(evs[3].meet.as_deref(), Some("https://zoom.us/j/123"));
        assert_eq!(evs[0].calendar, "Trabalho");
    }

    #[test]
    fn demo_has_an_imminent_meeting() {
        let now = Utc::now();
        let d = demo(now);
        assert!(d.events.iter().any(|e| !e.all_day && (e.start - now).num_minutes() == 4));
    }
}
