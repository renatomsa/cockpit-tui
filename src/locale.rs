//! Datas e horas por extenso, sem depender de locale do sistema.

use chrono::{DateTime, Datelike, Offset, TimeZone, Timelike};

use crate::config::Locale;

const DIAS_PT: [&str; 7] =
    ["segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado", "domingo"];
const MESES_PT: [&str; 12] = [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro",
    "novembro", "dezembro",
];

/// "sábado, 26 de setembro de 2026" / "Saturday, September 26, 2026".
pub fn long_date<Tz: TimeZone>(dt: &DateTime<Tz>, locale: Locale) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match locale {
        Locale::PtBr => format!(
            "{}, {} de {} de {}",
            DIAS_PT[dt.weekday().num_days_from_monday() as usize],
            dt.day(),
            MESES_PT[dt.month0() as usize],
            dt.year()
        ),
        Locale::En => dt.format("%A, %B %-d, %Y").to_string(),
    }
}

const DIAS_CURTOS_PT: [&str; 7] = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];

/// Quando um limite reinicia: "19:50 (em 2h 03m)" se for nas próximas 24 h,
/// senão "sex 03:00".
pub fn reset_when<Tz: TimeZone>(now: &DateTime<Tz>, at: &DateTime<Tz>, locale: Locale) -> String
where
    Tz::Offset: std::fmt::Display,
{
    // A API manda 22:49:59.9: arredonda para o minuto mais próximo, como o /usage.
    let at = at.clone() + chrono::Duration::seconds(30);
    let hm = format!("{:02}:{:02}", at.hour(), at.minute());
    let mins = (at.clone() - now.clone()).num_minutes();
    if mins <= 0 {
        return "agora".into();
    }
    if mins < 24 * 60 {
        let left = if mins >= 60 { format!("{}h {:02}m", mins / 60, mins % 60) } else { format!("{mins}m") };
        let word = if locale == Locale::En { "in" } else { "em" };
        return format!("{hm} ({word} {left})");
    }
    let day = match locale {
        Locale::PtBr => DIAS_CURTOS_PT[at.weekday().num_days_from_monday() as usize].to_string(),
        Locale::En => at.format("%a").to_string(),
    };
    format!("{day} {hm}")
}

/// "UTC-03:00".
pub fn utc_offset<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    let secs = dt.offset().fix().local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let m = secs.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// Hora curta para as abas de cidade: "15:55" ou "3:55 PM".
pub fn short_time<Tz: TimeZone>(dt: &DateTime<Tz>, hour12: bool) -> String {
    if hour12 {
        let (pm, h) = dt.hour12();
        format!("{h}:{:02} {}", dt.minute(), if pm { "PM" } else { "AM" })
    } else {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    }
}

/// Texto do relógio grande: "15:55:44", "15:55" ou "3:55:44" (12 h; AM/PM vai à parte).
pub fn big_time<Tz: TimeZone>(dt: &DateTime<Tz>, seconds: bool, hour12: bool) -> String {
    let h = if hour12 { dt.hour12().1 } else { dt.hour() };
    let hh = if hour12 { h.to_string() } else { format!("{h:02}") };
    if seconds {
        format!("{hh}:{:02}:{:02}", dt.minute(), dt.second())
    } else {
        format!("{hh}:{:02}", dt.minute())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Sao_Paulo;

    fn sample() -> DateTime<chrono_tz::Tz> {
        Sao_Paulo.with_ymd_and_hms(2026, 9, 26, 15, 5, 4).unwrap()
    }

    #[test]
    fn formats_pt_br() {
        assert_eq!(long_date(&sample(), Locale::PtBr), "sábado, 26 de setembro de 2026");
        assert_eq!(utc_offset(&sample()), "UTC-03:00");
        assert_eq!(short_time(&sample(), false), "15:05");
        assert_eq!(big_time(&sample(), true, false), "15:05:04");
    }

    #[test]
    fn formats_en_and_12h() {
        assert_eq!(long_date(&sample(), Locale::En), "Saturday, September 26, 2026");
        assert_eq!(short_time(&sample(), true), "3:05 PM");
        assert_eq!(big_time(&sample(), false, true), "3:05");
    }

    #[test]
    fn reset_times() {
        let now = Sao_Paulo.with_ymd_and_hms(2026, 9, 27, 17, 47, 0).unwrap();
        let soon = Sao_Paulo.with_ymd_and_hms(2026, 9, 27, 19, 50, 0).unwrap();
        assert_eq!(reset_when(&now, &soon, Locale::PtBr), "19:50 (em 2h 03m)");
        let later = Sao_Paulo.with_ymd_and_hms(2026, 10, 3, 3, 0, 0).unwrap();
        assert_eq!(reset_when(&now, &later, Locale::PtBr), "sáb 03:00");
        let min = Sao_Paulo.with_ymd_and_hms(2026, 9, 27, 18, 0, 0).unwrap();
        assert_eq!(reset_when(&now, &min, Locale::PtBr), "18:00 (em 13m)");
        let almost = Sao_Paulo.with_ymd_and_hms(2026, 9, 27, 19, 49, 59).unwrap();
        assert_eq!(reset_when(&now, &almost, Locale::PtBr), "19:50 (em 2h 03m)");
        assert_eq!(reset_when(&soon, &now, Locale::PtBr), "agora");
    }

    #[test]
    fn positive_offsets() {
        let dt = chrono_tz::Asia::Kolkata.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(utc_offset(&dt), "UTC+05:30");
    }
}
