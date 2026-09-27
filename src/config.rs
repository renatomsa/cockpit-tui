//! `~/.config/cockpit/config.toml`. Tudo tem default: o app abre mesmo sem arquivo.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use chrono_tz::Tz;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const EXAMPLE: &str = include_str!("../config.example.toml");

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: String,
    pub locale: Locale,
    pub clock: ClockConfig,
    pub location: LocationConfig,
    pub layout: LayoutConfig,
    pub system: SystemConfig,
    pub worktrees: WorktreesConfig,
    pub claude: ClaudeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "dracula".into(),
            locale: Locale::PtBr,
            clock: ClockConfig::default(),
            location: LocationConfig::default(),
            layout: LayoutConfig::default(),
            system: SystemConfig::default(),
            worktrees: WorktreesConfig::default(),
            claude: ClaudeConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Locale {
    #[default]
    #[serde(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "en")]
    En,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    pub seconds: bool,
    pub hour12: bool,
    pub extra: Vec<ExtraClock>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self { seconds: true, hour12: false, extra: Vec::new() }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraClock {
    pub label: String,
    pub tz: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocationConfig {
    /// Cidade fixa. Quando definida, não há geolocalização por IP.
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    /// Fuso IANA (ex.: `America/Sao_Paulo`). Sem ele, vale o da geolocalização ou o do sistema.
    pub timezone: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub refresh_minutes: u64,
}

impl Default for LocationConfig {
    fn default() -> Self {
        Self {
            city: None,
            region: None,
            country: None,
            timezone: None,
            lat: None,
            lon: None,
            refresh_minutes: 60,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutConfig {
    /// A partir de quantas colunas o layout vira duas colunas.
    pub wide_min_cols: u16,
    /// Abaixo de quantas linhas só o hero aparece.
    pub compact_max_rows: u16,
    /// Largura da coluna esquerda no layout largo, em %.
    pub left_percent: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self { wide_min_cols: 110, compact_max_rows: 30, left_percent: 62 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SystemConfig {
    /// Alvo do ping. Sem ICMP sem root, mede o tempo de conexão TCP (`host:porta`).
    pub ping_target: String,
    pub ping_interval_secs: u64,
    /// Pontos de montagem mostrados. Vazio = automático (`/`, `/home`, `/mnt/<letra>`, `/media/*`).
    pub disks: Vec<String>,
    /// Quantos processos em cada lista de maiores ofensores.
    pub top_n: usize,
    pub danger: DangerConfig,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            ping_target: "1.1.1.1:443".into(),
            ping_interval_secs: 5,
            disks: Vec::new(),
            top_n: 5,
            danger: DangerConfig::default(),
        }
    }
}

/// Limiares do detector de travamento, em % (PSI em % de tempo travado, média de 10 s).
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DangerConfig {
    pub mem_warn: f64,
    pub mem_crit: f64,
    pub swap_warn: f64,
    pub swap_crit: f64,
    pub psi_mem_warn: f64,
    pub psi_mem_crit: f64,
    pub cpu_warn: f64,
    pub cpu_crit: f64,
    /// Por quantos segundos a CPU precisa ficar alta para contar.
    pub cpu_window_secs: usize,
    pub disk_warn: f64,
    pub disk_crit: f64,
    /// Quanto o valor precisa cair abaixo do limiar para o alerta baixar (evita pisca-pisca).
    pub hysteresis: f64,
}

impl Default for DangerConfig {
    fn default() -> Self {
        Self {
            mem_warn: 85.0,
            mem_crit: 93.0,
            swap_warn: 50.0,
            swap_crit: 80.0,
            psi_mem_warn: 5.0,
            psi_mem_crit: 20.0,
            cpu_warn: 90.0,
            cpu_crit: 98.0,
            cpu_window_secs: 10,
            disk_warn: 90.0,
            disk_crit: 97.0,
            hysteresis: 3.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorktreesConfig {
    /// Pastas varridas atrás de repositórios git (`~` é expandido).
    pub workspaces: Vec<String>,
    /// Profundidade máxima da varredura dentro de cada pasta.
    pub depth: usize,
    /// Só lista repositórios que têm worktrees além da principal.
    pub only_with_worktrees: bool,
    /// Calcula o tamanho em disco de cada worktree (em segundo plano, a cada 10 min).
    pub sizes: bool,
    pub refresh_secs: u64,
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        Self { workspaces: vec!["~".into()], depth: 2, only_with_worktrees: false, sizes: true, refresh_secs: 30 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Pasta de dados do Claude Code. Padrão: `$CLAUDE_CONFIG_DIR` ou `~/.claude`.
    pub dir: Option<String>,
    /// Conversas encerradas há menos de N horas aparecem como "recentes" (para `r` retomar).
    pub recent_hours: u64,
    pub recent_max: usize,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self { dir: None, recent_hours: 24, recent_max: 5 }
    }
}

/// Expande `~` e `~/…` para o home.
pub fn expand_home(p: &str) -> PathBuf {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    match (p, home) {
        ("~", Some(h)) => h,
        (p, Some(h)) if p.starts_with("~/") => h.join(&p[2..]),
        (p, _) => PathBuf::from(p),
    }
}

/// Caminho para exibição, com o home abreviado para `~`.
pub fn tilde(p: &std::path::Path) -> String {
    if let Some(h) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
        && let Ok(rest) = p.strip_prefix(&h)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    p.display().to_string()
}

impl Config {
    pub fn parse(src: &str) -> Result<Self> {
        Ok(toml::from_str(src)?)
    }

    /// Relógios extras com fuso válido; os inválidos voltam como aviso.
    pub fn extra_clocks(&self) -> (Vec<(String, Tz)>, Vec<String>) {
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        for c in &self.clock.extra {
            match c.tz.parse::<Tz>() {
                Ok(tz) => ok.push((c.label.clone(), tz)),
                Err(_) => bad.push(format!("fuso inválido em clock.extra `{}`: {}", c.label, c.tz)),
            }
        }
        (ok, bad)
    }
}

/// Resultado do carregamento: um config com erro não derruba o app, vira aviso na barra.
pub struct Loaded {
    pub config: Config,
    pub path: PathBuf,
    pub warning: Option<String>,
}

pub fn load(path: Option<PathBuf>) -> Loaded {
    let path = path.unwrap_or_else(|| Paths::get().config_file());
    match fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Loaded { config: Config::default(), path, warning: None }
        }
        Err(e) => Loaded {
            config: Config::default(),
            warning: Some(format!("não li {}: {e}", path.display())),
            path,
        },
        Ok(src) => match Config::parse(&src) {
            Ok(config) => Loaded { config, path, warning: None },
            Err(e) => Loaded {
                config: Config::default(),
                warning: Some(format!("config inválido, usando defaults: {}", first_line(&e.to_string()))),
                path,
            },
        },
    }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim().to_string()
}

/// Diretórios XDG do app.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn get() -> Self {
        let dirs = ProjectDirs::from("", "", "cockpit").expect("sem diretório home");
        Self {
            config_dir: dirs.config_dir().to_path_buf(),
            state_dir: dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir()).to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.toml")
    }
}

/// O que o app lembra entre execuções (hoje: o último tema escolhido com `t`).
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PersistedState {
    pub theme: Option<String>,
}

impl PersistedState {
    pub fn load(paths: &Paths) -> Self {
        fs::read_to_string(paths.state_file())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        fs::create_dir_all(&paths.state_dir)?;
        fs::write(paths.state_file(), toml::to_string(self)?).context("gravar state.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let c = Config::parse(EXAMPLE).unwrap();
        let (clocks, bad) = c.extra_clocks();
        assert!(bad.is_empty());
        assert!(!clocks.is_empty());
    }

    #[test]
    fn empty_config_is_default() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.theme, "dracula");
        assert!(c.clock.seconds);
        assert_eq!(c.locale, Locale::PtBr);
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(Config::parse("temaa = \"x\"").is_err());
    }

    #[test]
    fn invalid_tz_is_reported() {
        let c = Config::parse("[[clock.extra]]\nlabel = \"X\"\ntz = \"Marte/Base\"").unwrap();
        let (ok, bad) = c.extra_clocks();
        assert!(ok.is_empty());
        assert_eq!(bad.len(), 1);
    }
}
