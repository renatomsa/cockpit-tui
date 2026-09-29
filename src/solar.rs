//! Nascer e pôr do sol pela equação solar (a aproximação da NOAA), a partir de latitude,
//! longitude e data. Erro típico de 1 a 2 minutos, o bastante para decidir entre ☀ e ☾.

use chrono::{DateTime, TimeZone, Utc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sun {
    /// Nascer e pôr do sol do dia (UTC).
    Times { rise: DateTime<Utc>, set: DateTime<Utc> },
    /// O sol não se põe (verão polar).
    AlwaysUp,
    /// O sol não nasce (inverno polar).
    AlwaysDown,
}

/// Nascer e pôr do sol do dia solar que contém `at`, em `lat`/`lon` graus (leste positivo).
pub fn sun(lat: f64, lon: f64, at: DateTime<Utc>) -> Sun {
    let rad = f64::to_radians;
    let deg = f64::to_degrees;
    let jd = at.timestamp() as f64 / 86_400.0 + 2_440_587.5;
    // O dia cujo meio-dia solar local fica mais perto de agora: o trânsito cai em
    // ~ n − lon/360, então n ≈ jd − J2000 + lon/360.
    let n = (jd - 2_451_545.0 - 0.0008 + lon / 360.0).round();
    let j_star = n - lon / 360.0;
    let m = (357.5291 + 0.985_600_28 * j_star).rem_euclid(360.0);
    let c = 1.9148 * rad(m).sin() + 0.0200 * rad(2.0 * m).sin() + 0.0003 * rad(3.0 * m).sin();
    let lambda = (m + c + 180.0 + 102.9372).rem_euclid(360.0);
    let transit = 2_451_545.0 + j_star + 0.0053 * rad(m).sin() - 0.0069 * rad(2.0 * lambda).sin();
    let decl = (rad(lambda).sin() * rad(23.4397).sin()).asin();
    // -0,833°: refração da atmosfera e o raio do disco solar.
    let cos_w = (rad(-0.833).sin() - rad(lat).sin() * decl.sin()) / (rad(lat).cos() * decl.cos());
    if cos_w > 1.0 {
        return Sun::AlwaysDown;
    }
    if cos_w < -1.0 {
        return Sun::AlwaysUp;
    }
    let w = deg(cos_w.acos());
    let to_utc = |j: f64| Utc.timestamp_opt(((j - 2_440_587.5) * 86_400.0).round() as i64, 0).single().unwrap_or(at);
    Sun::Times { rise: to_utc(transit - w / 360.0), set: to_utc(transit + w / 360.0) }
}

/// Está de dia em `lat`/`lon` agora?
pub fn is_day(lat: f64, lon: f64, now: DateTime<Utc>) -> bool {
    match sun(lat, lon, now) {
        Sun::Times { rise, set } => now >= rise && now < set,
        Sun::AlwaysUp => true,
        Sun::AlwaysDown => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use chrono_tz::America::Sao_Paulo;

    fn local_hm(t: DateTime<Utc>) -> (u32, u32) {
        let l = t.with_timezone(&Sao_Paulo);
        (l.hour(), l.minute())
    }

    #[test]
    fn rio_in_late_september() {
        // Rio de Janeiro, 29/09: nascer ≈ 05:36 e pôr ≈ 17:52 (hora de Brasília)
        let noon = Sao_Paulo.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap().with_timezone(&Utc);
        let Sun::Times { rise, set } = sun(-22.9068, -43.1729, noon) else { panic!() };
        let (rh, rm) = local_hm(rise);
        let (sh, sm) = local_hm(set);
        let rise_min = rh * 60 + rm;
        let set_min = sh * 60 + sm;
        assert!((5 * 60 + 25..=5 * 60 + 45).contains(&rise_min), "nascer {rh}:{rm:02}");
        assert!((17 * 60 + 40..=18 * 60 + 2).contains(&set_min), "pôr {sh}:{sm:02}");
    }

    #[test]
    fn day_and_night() {
        let at = |h| Sao_Paulo.with_ymd_and_hms(2026, 9, 29, h, 0, 0).unwrap().with_timezone(&Utc);
        assert!(!is_day(-22.9, -43.2, at(4)));
        assert!(is_day(-22.9, -43.2, at(7)));
        assert!(is_day(-22.9, -43.2, at(17)));
        assert!(!is_day(-22.9, -43.2, at(19)));
        // a mesma conta vale do outro lado do mundo (Tóquio, 12:00 local = 03:00 UTC)
        assert!(is_day(35.68, 139.69, Utc.with_ymd_and_hms(2026, 9, 29, 3, 0, 0).unwrap()));
    }

    #[test]
    fn polar_day_and_night() {
        let tromso = (69.65, 18.96);
        assert_eq!(sun(tromso.0, tromso.1, Utc.with_ymd_and_hms(2026, 12, 21, 12, 0, 0).unwrap()), Sun::AlwaysDown);
        assert_eq!(sun(tromso.0, tromso.1, Utc.with_ymd_and_hms(2026, 6, 21, 12, 0, 0).unwrap()), Sun::AlwaysUp);
    }
}
