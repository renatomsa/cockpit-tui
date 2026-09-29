use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{
    config::{Config, Paths},
    providers::{
        Snapshot,
        calendar::{CalendarSnapshot, Event},
        claude::{Session, SessionsSnapshot, Status},
        desktop::DesktopSnapshot,
        docker::DockerSnapshot,
        geo::Geo,
        git::{Worktree, WorktreesSnapshot},
        hostnet::HostNet,
        speedtest::{Phase, SpeedResult},
        weather::Weather,
        ping::PingSnapshot,
        specs::Specs,
        system::SystemSnapshot,
        usage::UsageSnapshot,
    },
    shell::ExitTarget,
    terminal::Launcher,
    theme::{Theme, ThemeRegistry},
};

/// Painéis que recebem foco com Tab, na ordem de navegação.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Hero,
    Agenda,
    Docker,
    Worktrees,
    System,
    Sessions,
}

impl Panel {
    /// `Sessions` não recebe foco direto: é a aba "Claude" do painel de Worktrees.
    const ORDER: [Panel; 5] = [Panel::Hero, Panel::Agenda, Panel::Docker, Panel::Worktrees, Panel::System];

    pub fn next(self) -> Self {
        let i = Self::ORDER.iter().position(|p| *p == self).unwrap();
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ORDER.iter().position(|p| *p == self).unwrap();
        Self::ORDER[(i + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Aba do painel que junta worktrees e conversas do Claude.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WtTab {
    #[default]
    Worktrees,
    /// Claude Code (CLI) no WSL.
    Claude,
    /// Claude Desktop (Windows).
    Desktop,
}

/// Teste de velocidade: o que está rodando agora e o último resultado.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeedState {
    pub running: Option<(Phase, f64)>,
    pub last: Option<SpeedResult>,
    pub error: Option<String>,
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

/// Ação destrutiva esperando o `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// SIGTERM num processo (painel Sistema).
    Kill(KillTarget),
    /// Comandos do Docker: título do diálogo, linhas explicando o que vai acontecer.
    Docker { title: String, lines: Vec<String>, cmds: Vec<crate::providers::docker::DockerCmd> },
    /// Excluir uma worktree: passo 1 pergunta (`y`), passo 2 avisa o que se perde (Enter).
    RemoveWorktree(RemoveWt),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveWt {
    pub step: u8,
    pub name: String,
    pub path: PathBuf,
    /// Repositório principal, onde o `git worktree remove` roda.
    pub repo: PathBuf,
    pub branch: Option<String>,
    /// Arquivos alterados ou novos, sem commit.
    pub dirty: u32,
    /// Conversas do Claude abertas dentro dela.
    pub sessions: usize,
    /// A pasta já não existe: basta `git worktree prune`.
    pub prunable: bool,
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
    pub confirm: Option<Confirm>,
    pub self_pid: u32,
    pub worktrees: Snapshot<WorktreesSnapshot>,
    pub sessions: Snapshot<SessionsSnapshot>,
    pub usage: Snapshot<UsageSnapshot>,
    pub specs: Snapshot<Specs>,
    pub docker: Snapshot<DockerSnapshot>,
    pub calendar: Snapshot<CalendarSnapshot>,
    /// `--demo-agenda`: eventos de exemplo em vez do Google.
    pub demo_agenda: bool,
    /// Até quando o alerta de reunião está adiado (`z`).
    pub snoozed_until: Option<DateTime<Utc>>,
    /// Reuniões cujo alerta o usuário já parou (`Event::key`).
    pub dismissed: HashSet<String>,
    pub agenda_sel: usize,
    pub wt_tab: WtTab,
    pub desktop: Snapshot<DesktopSnapshot>,
    pub desktop_sel: usize,
    /// Wi-Fi e tráfego do Windows (só no WSL).
    pub host_net: Snapshot<HostNet>,
    pub weather: Snapshot<Weather>,
    pub speed: SpeedState,
    /// Arquivo de config em uso (a recarga a quente observa ele).
    pub config_path: PathBuf,
    pub truecolor: bool,
    pub docker_sel: usize,
    /// Sessões vivas em cada worktree (caminho da worktree → índices em `sessions.live`).
    pub wt_sessions: HashMap<PathBuf, Vec<usize>>,
    /// Índice na lista filtrada de worktrees.
    pub wt_sel: usize,
    pub filter: String,
    pub filter_editing: bool,
    /// Índice em `live` seguido de `recent`.
    pub sess_sel: usize,
    /// Rodando pelo wrapper do shell (`--cd-file`): Enter pode sair e trocar a pasta.
    pub has_cd_file: bool,
    /// Destino escolhido; o `main` grava no arquivo do wrapper ao sair.
    pub exit: Option<ExitTarget>,
    /// Como abrir uma aba nova de terminal (detectado na partida).
    pub launcher: Option<Launcher>,
    /// Legenda dos símbolos do painel em foco (tecla `i`).
    pub show_legend: bool,
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
            confirm: None,
            self_pid: std::process::id(),
            worktrees: Snapshot::default(),
            sessions: Snapshot::default(),
            usage: Snapshot::default(),
            specs: Snapshot::default(),
            docker: Snapshot::default(),
            calendar: Snapshot::default(),
            demo_agenda: false,
            snoozed_until: None,
            dismissed: HashSet::new(),
            agenda_sel: 0,
            wt_tab: WtTab::Worktrees,
            desktop: Snapshot::default(),
            desktop_sel: 0,
            host_net: Snapshot::default(),
            weather: Snapshot::default(),
            speed: SpeedState::default(),
            config_path: PathBuf::new(),
            truecolor: true,
            docker_sel: 0,
            wt_sessions: HashMap::new(),
            wt_sel: 0,
            filter: String::new(),
            filter_editing: false,
            sess_sel: 0,
            has_cd_file: false,
            exit: None,
            launcher: None,
            show_legend: false,
            tz: system_tz,
            system_tz,
            extra_clocks,
            focus: Panel::Worktrees,
            show_help: false,
            notices,
            quit: false,
        };
        s.recompute_tz();
        s
    }

    /// Painel que recebe as teclas: no painel de worktrees, depende da aba.
    pub fn active_panel(&self) -> Panel {
        match (self.focus, self.wt_tab) {
            (Panel::Worktrees, WtTab::Claude | WtTab::Desktop) => Panel::Sessions,
            (p, _) => p,
        }
    }

    /// Aplica um config relido do disco. Devolve as seções que só valem ao reabrir
    /// (lidas pelos providers na partida).
    pub fn apply_config(&mut self, new: Config) -> Vec<&'static str> {
        let old = &self.config;
        let d = |x: &dyn std::fmt::Debug| format!("{x:?}");
        let mut restart = Vec::new();
        let checks: [(&'static str, bool); 7] = [
            ("[system]", d(&old.system) != d(&new.system)),
            ("[worktrees]", d(&old.worktrees) != d(&new.worktrees)),
            ("[location]", d(&old.location) != d(&new.location)),
            ("[network]", d(&old.network) != d(&new.network)),
            (
                "[claude]",
                (&old.claude.dir, old.claude.usage_limits, old.claude.usage_refresh_secs, old.claude.recent_hours, old.claude.recent_max)
                    != (&new.claude.dir, new.claude.usage_limits, new.claude.usage_refresh_secs, new.claude.recent_hours, new.claude.recent_max),
            ),
            (
                "[calendar]",
                d(&(&old.calendar.enabled, &old.calendar.client_secret, &old.calendar.calendars, &old.calendar.exclude, old.calendar.refresh_secs, old.calendar.lookahead_hours, old.calendar.show_declined))
                    != d(&(&new.calendar.enabled, &new.calendar.client_secret, &new.calendar.calendars, &new.calendar.exclude, new.calendar.refresh_secs, new.calendar.lookahead_hours, new.calendar.show_declined)),
            ),
            ("[weather]", (old.weather.enabled, old.weather.refresh_minutes) != (new.weather.enabled, new.weather.refresh_minutes)),
        ];
        for (name, changed) in checks {
            if changed {
                restart.push(name);
            }
        }
        // Tema trocado no config vence o último escolhido com `t`.
        if !new.theme.eq_ignore_ascii_case(&old.theme)
            && let Some(i) = self.themes.index_of(&new.theme)
        {
            self.theme_idx = i;
        }
        let (clocks, bad) = new.extra_clocks();
        self.extra_clocks = clocks;
        self.notices.extend(bad);
        self.config = new;
        self.recompute_tz();
        restart
    }

    /// Troca a lista de temas mantendo o tema atual (pelo nome), se ele ainda existir.
    pub fn apply_themes(&mut self, themes: ThemeRegistry) {
        let current = self.theme().name.clone();
        self.theme_idx = themes.index_of(&current).or_else(|| themes.index_of(&self.config.theme)).unwrap_or(0);
        self.notices.extend(themes.errors.iter().map(|e| format!("tema ignorado: {e}")));
        self.themes = themes;
    }

    pub fn theme(&self) -> &Theme {
        self.themes.get(self.theme_idx)
    }

    /// Fase do pisca-pisca (2 Hz): o loop acorda a cada meio segundo enquanto `animating()`.
    pub fn blink_on(&self) -> bool {
        self.now.timestamp_subsec_millis() < 500
    }

    /// Algo na tela pisca e precisa de tick de meio segundo (só o alerta de reunião).
    pub fn animating(&self) -> bool {
        self.alert().level == crate::alert::Level::Imminent
    }

    /// Está de dia na cidade atual? Pelo nascer e pôr do sol das coordenadas; sem
    /// elas, pelo que a Open-Meteo disse; sem as duas, das 6h às 18h.
    pub fn is_day(&self) -> bool {
        if let Some((lat, lon)) = self.geo.data.as_ref().and_then(|g| Some((g.lat?, g.lon?))) {
            return crate::solar::is_day(lat, lon, self.now);
        }
        if let Some(w) = &self.weather.data {
            return w.is_day;
        }
        (6..18).contains(&chrono::Timelike::hour(&self.local_now()))
    }

    pub fn events(&self) -> &[Event] {
        self.calendar.data.as_ref().map(|d| d.events.as_slice()).unwrap_or(&[])
    }

    /// Alerta de reunião agora (recalculado a cada tick, sem rede).
    pub fn alert(&self) -> crate::alert::Alert<'_> {
        crate::alert::evaluate(self.events(), self.now, &self.config.calendar, self.snoozed_until, &self.dismissed)
    }

    /// Lista de processos da coluna selecionada.
    pub fn selected_procs(&self) -> &[crate::providers::system::ProcGroup] {
        match (&self.system.data, self.proc_sel.col) {
            (Some(d), ProcCol::Cpu) => &d.top_cpu,
            (Some(d), ProcCol::Mem) => &d.top_mem,
            (None, _) => &[],
        }
    }

    /// Worktrees visíveis (com o filtro aplicado), como (repo, worktree).
    pub fn visible_worktrees(&self) -> Vec<(usize, usize)> {
        let Some(d) = &self.worktrees.data else { return Vec::new() };
        let needle = self.filter.to_lowercase();
        let mut out = Vec::new();
        for (ri, r) in d.repos.iter().enumerate() {
            for (wi, w) in r.worktrees.iter().enumerate() {
                let hay = format!("{}/{} {}", r.name, w.name, w.branch.as_deref().unwrap_or("")).to_lowercase();
                if fuzzy_match(&needle, &hay) {
                    out.push((ri, wi));
                }
            }
        }
        out
    }

    pub fn selected_worktree(&self) -> Option<&Worktree> {
        let (ri, wi) = *self.visible_worktrees().get(self.wt_sel)?;
        self.worktrees.data.as_ref()?.repos.get(ri)?.worktrees.get(wi)
    }

    /// Vivas e depois recentes, na ordem em que aparecem no painel.
    pub fn session_list(&self) -> Vec<&Session> {
        self.sessions.data.as_ref().map(|d| d.live.iter().chain(&d.recent).collect()).unwrap_or_default()
    }

    /// Pedidos de permissão que o cockpit pode aprovar agora, do mais antigo ao mais novo.
    pub fn pending_permissions(&self) -> Vec<&crate::claude_hook::Pending> {
        let mut v: Vec<_> = self.sessions.data.as_ref().map(|d| d.live.iter().filter_map(|s| s.pending.as_ref()).collect()).unwrap_or_default();
        v.sort_by_key(|p| p.created_at);
        v
    }

    pub fn waiting_count(&self) -> usize {
        self.sessions.data.as_ref().map_or(0, |d| d.count(Status::Waiting))
    }

    /// Associa cada sessão viva à worktree mais específica que contém o `cwd` dela
    /// (a principal contém as de `.claude/worktrees`, então ganha o prefixo mais longo).
    pub fn recompute_wt_sessions(&mut self) {
        self.wt_sessions.clear();
        let (Some(w), Some(sess)) = (&self.worktrees.data, &self.sessions.data) else { return };
        let paths: Vec<&PathBuf> = w.repos.iter().flat_map(|r| r.worktrees.iter().map(|w| &w.path)).collect();
        for (i, s) in sess.live.iter().enumerate() {
            if let Some(best) = paths.iter().filter(|p| s.cwd.starts_with(p)).max_by_key(|p| p.as_os_str().len()) {
                self.wt_sessions.entry((*best).clone()).or_default().push(i);
            }
        }
    }

    /// Nome curto de onde a sessão está: a worktree, se estiver numa, ou a pasta.
    pub fn session_place(&self, s: &Session) -> String {
        if let Some(w) = &self.worktrees.data {
            let best = w
                .repos
                .iter()
                .flat_map(|r| r.worktrees.iter().map(move |w| (r, w)))
                .filter(|(_, w)| s.cwd.starts_with(&w.path))
                .max_by_key(|(_, w)| w.path.as_os_str().len());
            if let Some((r, w)) = best {
                return if w.is_main { r.name.clone() } else { format!("{}/{}", r.name, w.name) };
            }
        }
        s.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| crate::config::tilde(&s.cwd))
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

/// Todas as letras do filtro aparecem na ordem, não necessariamente juntas ("cfs" casa
/// com "processable-file-cfs"). Filtro vazio casa com tudo.
pub fn fuzzy_match(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().filter(|c| !c.is_whitespace()).all(|n| it.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(fuzzy_match("", "qualquer"));
        assert!(fuzzy_match("cfs", "v360/processable-file-cfs feat/x"));
        assert!(fuzzy_match("v36 cfs", "v360/processable-file-cfs"));
        assert!(!fuzzy_match("sfc", "cfs"));
    }
}
