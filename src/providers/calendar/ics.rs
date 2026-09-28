//! Agenda por endereço iCal (.ics): o "endereço secreto no formato iCal" do Google
//! Calendar, ou qualquer outro feed ou arquivo .ics. Sem Google Cloud nem OAuth.
//!
//! A leitura segue a abordagem do meridian (github.com/victorlcampos/meridian, MIT):
//! desdobra as linhas (RFC 5545), lê cada VEVENT, expande recorrências com a crate
//! `rrule` no fuso do próprio evento (para manter a hora de parede no horário de verão),
//! aplica EXDATE e as ocorrências remarcadas uma a uma (RECURRENCE-ID) e descarta
//! canceladas. Além disso, guarda o fim, os eventos de dia inteiro e o link da chamada,
//! e marca como recusadas as reuniões que o dono da agenda recusou.

use std::collections::HashSet;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use super::Event;

struct Property {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Property {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
struct Component {
    uid: String,
    summary: String,
    start: Option<Property>,
    end: Option<Property>,
    duration: Option<String>,
    rule: Option<String>,
    exdates: Vec<Property>,
    recurrence_id: Option<Property>,
    cancelled: bool,
    location: Option<String>,
    description: Option<String>,
    url: Option<String>,
    conference: Option<String>,
    /// E-mails (minúsculos) de quem recusou.
    declined: Vec<String>,
}

#[derive(Clone, Copy)]
enum Moment {
    AllDay(NaiveDate),
    At(DateTime<Utc>, Tz),
}

impl Moment {
    /// Instante e fuso para expandir recorrências; dia inteiro vale da meia-noite local.
    fn instant(self, floating: Tz) -> Option<(DateTime<Utc>, Tz)> {
        match self {
            Moment::At(t, tz) => Some((t, tz)),
            Moment::AllDay(d) => Some((local(floating, d.and_hms_opt(0, 0, 0)?)?, floating)),
        }
    }
}

fn local(tz: Tz, t: NaiveDateTime) -> Option<DateTime<Utc>> {
    // Hora pulada pelo horário de verão acontece uma hora depois.
    tz.from_local_datetime(&t).earliest().or_else(|| tz.from_local_datetime(&(t + Duration::hours(1))).earliest()).map(|d| d.with_timezone(&Utc))
}

/// Dono da agenda: o e-mail do endereço secreto do Google
/// (`/calendar/ical/<email>/private-…/basic.ics`), para achar os convites recusados.
pub fn owner_from_address(address: &str) -> Option<String> {
    let rest = address.split("/calendar/ical/").nth(1)?;
    let email = rest.split('/').next()?.replace("%40", "@").to_lowercase();
    email.contains('@').then_some(email)
}

/// Eventos que terminam depois de `from` e começam antes de `until`, do mais cedo ao
/// mais tarde. Horas sem fuso seguem o fuso da agenda (X-WR-TIMEZONE) ou `system`.
pub fn events(ics: &str, from: DateTime<Utc>, until: DateTime<Utc>, system: Tz, owner: Option<&str>) -> (String, Vec<Event>) {
    let mut components = Vec::new();
    let mut owner = owner.map(str::to_lowercase);
    let mut floating = system;
    let mut cal_name = String::new();
    let mut current: Option<Component> = None;
    let mut nested = 0;
    for line in unfold(ics) {
        let Some(p) = parse_property(&line) else { continue };
        match (p.name.as_str(), current.as_mut()) {
            ("BEGIN", None) if p.value.eq_ignore_ascii_case("VEVENT") => current = Some(Component::default()),
            ("BEGIN", Some(_)) => nested += 1, // VALARM etc.
            ("END", Some(_)) if nested > 0 => nested -= 1,
            ("END", Some(_)) => components.extend(current.take()),
            (_, Some(_)) if nested > 0 => {}
            (_, Some(ev)) => read_property(ev, p),
            ("X-WR-CALNAME", None) => {
                cal_name = unescape(&p.value);
                if owner.is_none() && p.value.contains('@') {
                    owner = Some(p.value.to_lowercase());
                }
            }
            ("X-WR-TIMEZONE", None) => {
                if let Ok(tz) = p.value.parse() {
                    floating = tz;
                }
            }
            _ => {}
        }
    }

    let replaced: HashSet<(String, i64)> = components
        .iter()
        .filter_map(|c| {
            let (t, _) = parse_moment(c.recurrence_id.as_ref()?, floating)?.instant(floating)?;
            Some((c.uid.clone(), t.timestamp()))
        })
        .collect();

    let mut out = Vec::new();
    for c in &components {
        let Some(start_m) = c.start.as_ref().and_then(|s| parse_moment(s, floating)) else { continue };
        if c.cancelled {
            continue;
        }
        let all_day = matches!(start_m, Moment::AllDay(_));
        let Some((start, tz)) = start_m.instant(floating) else { continue };
        let length = event_length(c, start, floating, all_day);
        // Busca começos um pouco antes de `from` para pegar o que já está em andamento.
        let starts = match (&c.rule, &c.recurrence_id) {
            (Some(rule), None) => expand(rule, start, tz, &c.exdates, floating, from - length, until),
            _ => vec![start],
        };
        let declined = owner.as_ref().is_some_and(|o| c.declined.contains(o));
        for s in starts {
            let moved = c.recurrence_id.is_none() && replaced.contains(&(c.uid.clone(), s.timestamp()));
            let end = s + length;
            if end > from && s < until && !moved {
                out.push(Event {
                    id: c.uid.clone(),
                    calendar: cal_name.clone(),
                    title: if c.summary.trim().is_empty() { "(sem título)".into() } else { c.summary.clone() },
                    start: s,
                    end,
                    all_day,
                    meet: meeting_link(c),
                    html_link: c.url.clone(),
                    location: c.location.clone(),
                    declined,
                });
            }
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then(a.title.cmp(&b.title)));
    (cal_name, out)
}

fn read_property(ev: &mut Component, p: Property) {
    match p.name.as_str() {
        "UID" => ev.uid = p.value,
        "SUMMARY" => ev.summary = unescape(&p.value),
        "DTSTART" => ev.start = Some(p),
        "DTEND" => ev.end = Some(p),
        "DURATION" => ev.duration = Some(p.value),
        "RRULE" => ev.rule = Some(p.value),
        "EXDATE" => ev.exdates.push(p),
        "RECURRENCE-ID" => ev.recurrence_id = Some(p),
        "STATUS" => ev.cancelled = p.value.eq_ignore_ascii_case("CANCELLED"),
        "LOCATION" => ev.location = Some(unescape(&p.value)).filter(|s| !s.is_empty()),
        "DESCRIPTION" => ev.description = Some(unescape(&p.value)),
        "URL" => ev.url = Some(p.value),
        "X-GOOGLE-CONFERENCE" => ev.conference = Some(p.value),
        "ATTENDEE" => {
            let declined = p.param("PARTSTAT").is_some_and(|s| s.eq_ignore_ascii_case("DECLINED"));
            if declined && let Some(email) = p.value.to_lowercase().strip_prefix("mailto:") {
                ev.declined.push(email.to_string());
            }
        }
        _ => {}
    }
}

/// Duração pelo DTEND, pelo DURATION ou, sem os dois, 1 dia (dia inteiro) ou 0.
fn event_length(c: &Component, start: DateTime<Utc>, floating: Tz, all_day: bool) -> Duration {
    if let Some(end) = c.end.as_ref().and_then(|e| parse_moment(e, floating)).and_then(|m| m.instant(floating)) {
        let d = end.0 - start;
        if d > Duration::zero() {
            return d;
        }
    }
    if let Some(d) = c.duration.as_deref().and_then(parse_duration) {
        return d;
    }
    if all_day { Duration::days(1) } else { Duration::zero() }
}

/// `PT1H30M`, `P1D`, `-PT15M`…
fn parse_duration(s: &str) -> Option<Duration> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut total, mut num, mut in_time) = (0i64, String::new(), false);
    for ch in s.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => num.push(ch),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (ch, in_time) {
                    ('W', _) => 7 * 86_400,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(Duration::seconds(if neg { -total } else { total }))
}

/// Link da chamada: o Meet do Google, senão o primeiro link de reunião conhecido no
/// local, na descrição ou na URL do evento.
fn meeting_link(c: &Component) -> Option<String> {
    if let Some(u) = &c.conference {
        return Some(u.clone());
    }
    const HOSTS: [&str; 5] = ["meet.google.com/", "zoom.us/", "teams.microsoft.com/", "teams.live.com/", "whereby.com/"];
    let texts = [c.location.as_deref(), c.description.as_deref(), c.url.as_deref()];
    for text in texts.into_iter().flatten() {
        for word in text.split(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"') {
            let w = word.trim_end_matches([',', ';', ')', '.']);
            if w.starts_with("https://") && HOSTS.iter().any(|h| w.contains(h)) {
                return Some(w.to_string());
            }
        }
    }
    None
}

/// Ocorrências de uma série entre `from` e `until`, no fuso do próprio evento.
/// Regra ilegível deixa só a primeira ocorrência.
fn expand(rule: &str, start: DateTime<Utc>, tz: Tz, exdates: &[Property], floating: Tz, from: DateTime<Utc>, until: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let rtz = rrule::Tz::Tz(tz);
    let Ok(mut set) = rule.parse::<rrule::RRule<rrule::Unvalidated>>().and_then(|r| r.build(start.with_timezone(&rtz))) else {
        return vec![start];
    };
    for ex in exdates {
        for value in ex.value.split(',') {
            let single = Property { name: ex.name.clone(), params: ex.params.clone(), value: value.to_string() };
            if let Some((t, _)) = parse_moment(&single, floating).and_then(|m| m.instant(floating)) {
                set = set.exdate(t.with_timezone(&rtz));
            }
        }
    }
    set.after((from - Duration::seconds(1)).with_timezone(&rtz))
        .before(until.with_timezone(&rtz))
        .all(1000)
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect()
}

/// `20260925T140000Z` (UTC), `20260925T140000` com TZID ou flutuante, ou data de dia inteiro.
fn parse_moment(p: &Property, floating: Tz) -> Option<Moment> {
    let v = p.value.trim();
    if p.param("VALUE").is_some_and(|k| k.eq_ignore_ascii_case("DATE")) || v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(Moment::AllDay);
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(Moment::At(t.and_utc(), chrono_tz::UTC));
    }
    let t = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz = p.param("TZID").and_then(|n| n.parse().ok()).unwrap_or(floating);
    Some(Moment::At(local(tz, t)?, tz))
}

/// Linhas longas remontadas (a RFC 5545 as dobra com um espaço no começo).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ if !line.is_empty() => lines.push(line.to_string()),
            _ => {}
        }
    }
    lines
}

fn parse_property(line: &str) -> Option<Property> {
    let (head, value) = split_once_unquoted(line, ':')?;
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts.filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string())).collect();
    Some(Property { name, params, value: value.to_string() })
}

fn split_once_unquoted(text: &str, sep: char) -> Option<(&str, &str)> {
    let mut quoted = false;
    let (i, _) = text.char_indices().find(|&(_, ch)| {
        if ch == '"' {
            quoted = !quoted;
        }
        ch == sep && !quoted
    })?;
    Some((&text[..i], &text[i + 1..]))
}

fn split_unquoted(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some((part, tail)) = split_once_unquoted(rest, sep) {
        parts.push(part);
        rest = tail;
    }
    parts.push(rest);
    parts
}

/// Escapes de texto: `\,` `\;` `\n` `\\`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Endereço seguro de mostrar: domínio e arquivo, sem o trecho secreto do meio.
pub fn describe_address(address: &str) -> String {
    let rest = address.split_once("://").map_or(address, |(_, r)| r);
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let first = parts.next().unwrap_or_default();
    match parts.next_back() {
        Some(last) => format!("{first}/…/{last}"),
        None => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    const FEED: &str = "BEGIN:VCALENDAR\r
X-WR-CALNAME:renato@example.com\r
X-WR-TIMEZONE:America/Sao_Paulo\r
BEGIN:VEVENT\r
UID:daily\r
SUMMARY:Daily do time\r
DTSTART;TZID=America/Sao_Paulo:20260928T090000\r
DTEND;TZID=America/Sao_Paulo:20260928T091500\r
RRULE:FREQ=DAILY;COUNT=5\r
EXDATE;TZID=America/Sao_Paulo:20260930T090000\r
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
TRIGGER:-PT10M\r
END:VALARM\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:daily\r
RECURRENCE-ID;TZID=America/Sao_Paulo:20261001T090000\r
SUMMARY:Daily (remarcada)\r
DTSTART;TZID=America/Sao_Paulo:20261001T110000\r
DURATION:PT30M\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:recusada\r
SUMMARY:Reunião recusada\r
DTSTART:20260928T150000Z\r
DTEND:20260928T160000Z\r
ATTENDEE;CN=Renato;PARTSTAT=DECLINED:mailto:Renato@Example.com\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:cancelada\r
SUMMARY:Cancelada\r
STATUS:CANCELLED\r
DTSTART:20260928T170000Z\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:feriado\r
SUMMARY:Feriado\r
DTSTART;VALUE=DATE:20261012\r
DTEND;VALUE=DATE:20261013\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:zoom\r
SUMMARY:Cliente\\, kickoff\r
DTSTART:20260929T180000Z\r
DTEND:20260929T190000Z\r
DESCRIPTION:Entrar: https://zoom.us/j/123?pwd=x\\nObrigado\r
LOCATION:Sala 3\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn reads_a_google_style_feed() {
        let (name, evs) = events(FEED, utc(2026, 9, 28, 0, 0), utc(2026, 10, 14, 0, 0), chrono_tz::UTC, None);
        assert_eq!(name, "renato@example.com");
        let titles: Vec<&str> = evs.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Daily do time", "Reunião recusada", "Daily do time", "Cliente, kickoff", "Daily (remarcada)", "Daily do time", "Feriado"],
            "30/09 tirado pelo EXDATE; 01/10 remarcado; cancelada fora"
        );
        // 09:00 em São Paulo = 12:00 UTC; 15 min de duração
        assert_eq!((evs[0].start, evs[0].end), (utc(2026, 9, 28, 12, 0), utc(2026, 9, 28, 12, 15)));
        assert_eq!(evs[0].meet.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert!(evs[1].declined, "recusada pelo dono (X-WR-CALNAME)");
        assert_eq!(evs[3].meet.as_deref(), Some("https://zoom.us/j/123?pwd=x"));
        let remarcada = &evs[4];
        assert_eq!((remarcada.start, remarcada.end), (utc(2026, 10, 1, 14, 0), utc(2026, 10, 1, 14, 30)));
        let feriado = evs.last().unwrap();
        assert!(feriado.all_day);
        assert_eq!(feriado.end - feriado.start, Duration::days(1));
    }

    #[test]
    fn keeps_meetings_already_in_progress() {
        let (_, evs) = events(FEED, utc(2026, 9, 28, 12, 10), utc(2026, 9, 28, 13, 0), chrono_tz::UTC, None);
        assert_eq!(evs.len(), 1, "a Daily das 12:00–12:15 UTC ainda está em andamento às 12:10");
    }

    #[test]
    fn ignores_what_it_cannot_read() {
        let broken = "BEGIN:VEVENT\nDTSTART:20260101T100000Z\nRRULE:FREQ=SOMETIMES\nUID:x\nSUMMARY:Uma vez\nEND:VEVENT\nBEGIN:VEVENT\nUID:y\nDTSTART:lixo\nEND:VEVENT\n";
        let (_, evs) = events(broken, utc(2025, 12, 31, 0, 0), utc(2026, 1, 2, 0, 0), chrono_tz::UTC, None);
        assert_eq!(evs.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Uma vez"]);
    }

    #[test]
    fn owner_and_masked_address() {
        let a = "https://calendar.google.com/calendar/ical/renato.serrano%40virtual360.io/private-abc123/basic.ics";
        assert_eq!(owner_from_address(a).as_deref(), Some("renato.serrano@virtual360.io"));
        assert_eq!(describe_address(a), "calendar.google.com/…/basic.ics");
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1D"), Some(Duration::days(1)));
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
    }
}
