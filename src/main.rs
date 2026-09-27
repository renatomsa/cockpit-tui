mod app;
mod config;
mod danger;
mod locale;
mod providers;
mod shell;
mod terminal;
mod theme;
mod ui;

use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::{
    app::state::AppState,
    config::{Paths, PersistedState},
    theme::ThemeRegistry,
};

/// Cockpit de terminal: relógio, agenda, sistema, worktrees e sessões do Claude.
#[derive(Parser)]
#[command(name = "cockpit", version, about)]
struct Cli {
    /// Arquivo de config (padrão: ~/.config/cockpit/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Tema desta execução (não fica gravado).
    #[arg(long)]
    theme: Option<String>,
    /// Arquivo onde gravar a pasta escolhida ao sair (usado pela função do `shell-init`).
    #[arg(long, hide = true)]
    cd_file: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Temas embutidos e instalados.
    Theme {
        #[command(subcommand)]
        cmd: ThemeCmd,
    },
    /// Função de shell que faz o Enter trocar de pasta. Ponha no ~/.zshrc:
    /// eval "$(cockpit shell-init zsh)"
    ShellInit {
        /// zsh, bash ou fish
        shell: String,
    },
    /// Arquivo de configuração.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ThemeCmd {
    /// Lista os temas disponíveis.
    List,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Mostra onde ficam config, temas, estado e log.
    Path,
    /// Cria o config.toml comentado (não sobrescreve um existente).
    Init,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::get();

    match cli.cmd {
        Some(Cmd::Theme { cmd: ThemeCmd::List }) => theme_list(&paths),
        Some(Cmd::Config { cmd: ConfigCmd::Path }) => {
            println!("config  {}", cli.config.unwrap_or_else(|| paths.config_file()).display());
            println!("temas   {}", paths.themes_dir().display());
            println!("estado  {}", paths.state_file().display());
            println!("log     {}", paths.state_dir.join("cockpit.log").display());
            Ok(())
        }
        Some(Cmd::Config { cmd: ConfigCmd::Init }) => config_init(cli.config.unwrap_or_else(|| paths.config_file())),
        Some(Cmd::ShellInit { shell }) => {
            print!("{}", shell::init_script(&shell)?);
            Ok(())
        }
        None => run(cli.config, cli.theme, cli.cd_file, paths),
    }
}

fn run(config_path: Option<PathBuf>, theme_arg: Option<String>, cd_file: Option<PathBuf>, paths: Paths) -> Result<()> {
    let _log_guard = init_logging(&paths);
    let loaded = config::load(config_path);
    let mut notices: Vec<String> = loaded.warning.into_iter().collect();

    let themes = ThemeRegistry::load(&paths.themes_dir(), theme::truecolor_supported());
    notices.extend(themes.errors.iter().map(|e| format!("tema ignorado: {e}")));

    // Prioridade: --theme > último escolhido com `t` > config.
    let persisted = PersistedState::load(&paths).theme;
    let wanted = [theme_arg.as_deref(), persisted.as_deref(), Some(loaded.config.theme.as_str())];
    let theme_idx = wanted.iter().flatten().find_map(|n| themes.index_of(n));
    let theme_idx = match theme_idx {
        Some(i) => i,
        None => {
            notices.push(format!("tema `{}` não existe, usando {}", loaded.config.theme, themes.get(0).name));
            0
        }
    };
    if let Some(name) = &theme_arg
        && themes.index_of(name).is_none()
    {
        bail!("tema `{name}` não existe (veja `cockpit theme list`)");
    }

    tracing::info!(config = %loaded.path.display(), "iniciando");
    let mut state = AppState::new(loaded.config, paths, themes, theme_idx, notices);
    state.has_cd_file = cd_file.is_some();
    state.launcher = terminal::detect(&state.config.terminal.new_tab);
    tracing::info!(launcher = ?state.launcher, "aba nova");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("cockpit")
        .build()?;
    let exit = rt.block_on(app::run(state))?;
    // Sem esperar tarefas pendentes (git, du): o usuário quer o prompt de volta já.
    rt.shutdown_background();
    if let (Some(file), Some(target)) = (cd_file, exit) {
        shell::write_cd_file(&file, &target)?;
    }
    Ok(())
}

fn theme_list(paths: &Paths) -> Result<()> {
    let themes = ThemeRegistry::load(&paths.themes_dir(), true);
    for e in &themes.entries {
        println!("{:<20} {}", e.theme.name, if e.builtin { "embutido" } else { "instalado" });
    }
    for err in &themes.errors {
        eprintln!("ignorado: {err}");
    }
    Ok(())
}

fn config_init(path: PathBuf) -> Result<()> {
    if path.exists() {
        bail!("{} já existe; não vou sobrescrever", path.display());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, config::EXAMPLE).with_context(|| format!("gravar {}", path.display()))?;
    println!("criado {}", path.display());
    Ok(())
}

/// Log só em arquivo: qualquer escrita em stdout/stderr bagunçaria a tela.
fn init_logging(paths: &Paths) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    fs::create_dir_all(&paths.state_dir).ok()?;
    let file = tracing_appender::rolling::never(&paths.state_dir, "cockpit.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = tracing_subscriber::EnvFilter::try_from_env("COCKPIT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_writer(writer).with_ansi(false).with_env_filter(filter).init();
    Some(guard)
}
