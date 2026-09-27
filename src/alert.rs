//! Alerta de reunião. Função pura: recebe a hora e os eventos, devolve o que mostrar.
//! A UI chama a cada segundo; nada aqui faz I/O.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};

use crate::{config::CalendarConfig, providers::calendar::Event};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nada perto (ou nada na agenda).
    Calm,
    /// Faltam até `soon_minutes`: aviso amarelo.
    Soon,
    /// Faltam até `alert_minutes`, ou começou há até `alert_after_start_minutes`:
    /// tela vermelha até o usuário apertar uma tecla.
    Imminent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alert<'a> {
    pub level: Level,
    /// A reunião que o hero mostra: a próxima a começar (ou a que acabou de começar);
    /// sem nenhuma, a que está em andamento.
    pub event: Option<&'a Event>,
    /// O alerta vermelho foi silenciado (soneca ou tecla) e virou amarelo.
    pub silenced: bool,
}

/// `dismissed` guarda `Event::key()` das reuniões cujo alerta o usuário já parou.
pub fn evaluate<'a>(
    events: &'a [Event],
    now: DateTime<Utc>,
    cfg: &CalendarConfig,
    snoozed_until: Option<DateTime<Utc>>,
    dismissed: &HashSet<String>,
) -> Alert<'a> {
    let after = Duration::minutes(cfg.alert_after_start_minutes);
    let timed = || events.iter().filter(|e| !e.all_day && !e.declined && e.end > now);
    // A próxima que ainda pode alertar; uma reunião longa em andamento não pode esconder
    // a que começa daqui a 3 minutos.
    let upcoming = timed().filter(|e| e.start + after > now).min_by_key(|e| e.start);
    let Some(ev) = upcoming.or_else(|| timed().filter(|e| e.start <= now).max_by_key(|e| e.start)) else {
        return Alert { level: Level::Calm, event: None, silenced: false };
    };

    let until = ev.start - now;
    let level = if until <= Duration::minutes(cfg.alert_minutes) && until > -after {
        Level::Imminent
    } else if until <= Duration::minutes(cfg.soon_minutes) && until > Duration::zero() {
        Level::Soon
    } else {
        Level::Calm
    };
    let silenced = level == Level::Imminent && (snoozed_until.is_some_and(|t| t > now) || dismissed.contains(&ev.key()));
    Alert { level: if silenced { Level::Soon } else { level }, event: Some(ev), silenced }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 28, h, m, 0).unwrap()
    }

    fn ev(id: &str, start: DateTime<Utc>, mins: i64) -> Event {
        Event { id: id.into(), title: id.into(), start, end: start + Duration::minutes(mins), ..Default::default() }
    }

    fn eval(events: &[Event], now: DateTime<Utc>) -> (Level, Option<String>) {
        let a = evaluate(events, now, &CalendarConfig::default(), None, &HashSet::new());
        (a.level, a.event.map(|e| e.id.clone()))
    }

    #[test]
    fn calm_then_soon_then_imminent_then_calm() {
        let evs = [ev("daily", at(16, 0), 30)];
        assert_eq!(eval(&evs, at(15, 30)).0, Level::Calm);
        assert_eq!(eval(&evs, at(15, 45)).0, Level::Soon, "15 min antes");
        assert_eq!(eval(&evs, at(15, 50)).0, Level::Soon);
        assert_eq!(eval(&evs, at(15, 55)).0, Level::Imminent, "5 min antes");
        assert_eq!(eval(&evs, at(16, 0)).0, Level::Imminent, "começando");
        assert_eq!(eval(&evs, at(16, 4)).0, Level::Imminent, "começou há 4 min");
        assert_eq!(eval(&evs, at(16, 5)), (Level::Calm, Some("daily".into())), "em andamento, sem alerta");
        assert_eq!(eval(&evs, at(16, 30)), (Level::Calm, None), "acabou");
    }

    #[test]
    fn long_meeting_does_not_hide_the_next_one() {
        let evs = [ev("workshop", at(14, 0), 180), ev("1:1", at(15, 0), 30)];
        assert_eq!(eval(&evs, at(14, 30)), (Level::Calm, Some("1:1".into())));
        assert_eq!(eval(&evs, at(14, 57)), (Level::Imminent, Some("1:1".into())));
    }

    #[test]
    fn all_day_and_declined_never_alert() {
        let mut feriado = ev("feriado", at(15, 58), 24 * 60);
        feriado.all_day = true;
        let mut recusada = ev("recusada", at(15, 58), 30);
        recusada.declined = true;
        assert_eq!(eval(&[feriado, recusada], at(15, 57)), (Level::Calm, None));
    }

    #[test]
    fn snooze_and_dismiss_turn_red_into_yellow() {
        let evs = [ev("daily", at(16, 0), 30)];
        let cfg = CalendarConfig::default();
        let a = evaluate(&evs, at(15, 57), &cfg, Some(at(16, 2)), &HashSet::new());
        assert_eq!((a.level, a.silenced), (Level::Soon, true));
        // a soneca venceu e a reunião ainda está na janela: volta o vermelho
        let a = evaluate(&evs, at(16, 3), &cfg, Some(at(16, 2)), &HashSet::new());
        assert_eq!(a.level, Level::Imminent);
        let dismissed: HashSet<String> = [evs[0].key()].into();
        let a = evaluate(&evs, at(15, 57), &cfg, None, &dismissed);
        assert_eq!((a.level, a.silenced), (Level::Soon, true));
    }

    #[test]
    fn empty_agenda() {
        assert_eq!(eval(&[], at(10, 0)), (Level::Calm, None));
    }
}
