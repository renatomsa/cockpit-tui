use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{
    config::{Config, Paths},
    danger::Level,
    providers::{Snapshot, geo::Geo, ping::PingSnapshot, system::SystemSnapshot},
    theme::{Theme, ThemeRegistry},
};

/// Painéis que recebem foco com Tab, na ordem de navegação.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Hero,
    Agenda,
    Worktrees,
    System,
    Sessions,
}

impl Panel {
    const ORDER: [Panel; 5] = [Panel::Hero, Panel::Agenda, Panel::Worktrees, Panel::System, Panel::Sessions];

    pub fn next(self) -> Self {
        let i = Self::ORDER.iter().position(|p| *p == self).unwrap();
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ORDER.iter().position(|p| *p == self).unwrap();
        Self::ORDER[(i + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Coluna de processos selecionada no painel de sistema.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcCol {
    #[default]
    Cpu,
    Mem,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcSel {
    pub col: ProcCol,
    pub row: usize,
}

/// Processo aguardando confirmação do `K`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillTarget {
    pub pid: u32,
    pub name: String,
}

/// Todo o estado da tela. Só `update` escreve aqui; só `ui::draw` lê.
pub struct AppState {
    pub now: DateTime<Utc>,
    pub config: Config,
    pub paths: Paths,
    pub themes: ThemeRegistry,
    pub theme_idx: usize,
    pub geo: Snapshot<Geo>,
    pub system: Snapshot<SystemSnapshot>,
    pub ping: Snapshot<PingSnapshot>,
    pub proc_sel: ProcSel,
    pub confirm_kill: Option<KillTarget>,
    pub self_pid: u32,
    /// Fuso da cidade atual (geolocalização → config → sistema → UTC).
    pub tz: Tz,
    pub system_tz: Tz,
    pub extra_clocks: Vec<(String, Tz)>,
    pub focus: Panel,
    pub show_help: bool,
    pub notices: Vec<String>,
    pub quit: bool,
}

impl AppState {
    pub fn new(config: Config, paths: Paths, themes: ThemeRegistry, theme_idx: usize, notices: Vec<String>) -> Self {
        let system_tz = iana_time_zone::get_timezone().ok().and_then(|n| n.parse().ok()).unwrap_or(Tz::UTC);
        let (extra_clocks, mut bad) = config.extra_clocks();
        let mut notices = notices;
        notices.append(&mut bad);
        let mut s = Self {
            now: Utc::now(),
            config,
            paths,
            themes,
            theme_idx,
            geo: Snapshot::default(),
            system: Snapshot::default(),
            ping: Snapshot::default(),
            proc_sel: ProcSel::default(),
            confirm_kill: None,
            self_pid: std::process::id(),
            tz: system_tz,
            system_tz,
            extra_clocks,
            focus: Panel::Hero,
            show_help: false,
            notices,
            quit: false,
        };
        s.recompute_tz();
        s
    }

    pub fn theme(&self) -> &Theme {
        self.themes.get(self.theme_idx)
    }

    /// Fase do pisca-pisca (2 Hz): o loop acorda a cada meio segundo enquanto `animating()`.
    pub fn blink_on(&self) -> bool {
        self.now.timestamp_subsec_millis() < 500
    }

    /// Algo na tela pisca e precisa de tick de meio segundo.
    pub fn animating(&self) -> bool {
        self.system.data.as_ref().is_some_and(|d| d.danger.worst() == Level::Critical)
    }

    /// Lista de processos da coluna selecionada.
    pub fn selected_procs(&self) -> &[crate::providers::system::ProcGroup] {
        match (&self.system.data, self.proc_sel.col) {
            (Some(d), ProcCol::Cpu) => &d.top_cpu,
            (Some(d), ProcCol::Mem) => &d.top_mem,
            (None, _) => &[],
        }
    }

    pub fn recompute_tz(&mut self) {
        let from_geo = self.geo.data.as_ref().and_then(|g| g.timezone.as_deref());
        self.tz = from_geo
            .or(self.config.location.timezone.as_deref())
            .and_then(|n| n.parse().ok())
            .unwrap_or(self.system_tz);
    }

    pub fn local_now(&self) -> DateTime<Tz> {
        self.now.with_timezone(&self.tz)
    }

    /// Nome exibido da cidade atual. Sem geo, deriva do fuso (`America/Sao_Paulo` → "Sao Paulo").
    pub fn city_label(&self) -> String {
        if let Some(g) = &self.geo.data {
            return g.city.clone();
        }
        self.tz.name().rsplit('/').next().unwrap_or("UTC").replace('_', " ")
    }

    /// "Rio de Janeiro · Brazil", com região quando ela difere da cidade.
    pub fn place_line(&self) -> String {
        match &self.geo.data {
            Some(g) => {
                let mut parts = vec![g.city.clone()];
                if let Some(r) = g.region.as_ref().filter(|r| **r != g.city && !r.is_empty()) {
                    parts.push(r.clone());
                }
                if let Some(c) = g.country.as_ref().filter(|c| !c.is_empty()) {
                    parts.push(c.clone());
                }
                parts.join(" · ")
            }
            None if self.geo.is_loading() => "localizando…".into(),
            None => format!("{} · fuso do sistema", self.city_label()),
        }
    }
}
