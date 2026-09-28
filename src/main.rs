mod alert;
mod app;
mod config;
mod danger;
mod doctor;
mod locale;
mod platform;
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
    /// Mostra reuniões de exemplo (uma começando em 4 min) em vez do Google Calendar,
    /// para ver o alerta sem conectar nada.
    #[arg(long)]
    demo_agenda: bool,
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
    /// Confere cada peça (config, temas, git, Claude, Docker, Google, rede, WSL) e diz como resolver.
    Doctor,
    /// Conecta serviços externos.
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
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
enum AuthCmd {
    /// Autoriza a leitura do Google Calendar (abre o navegador).
    Google {
        /// JSON do cliente OAuth "App para computador" (padrão: [calendar].client_secret).
        #[arg(long)]
        client_secret: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ThemeCmd {
    /// Lista os temas disponíveis.
    List,
    /// Instala um tema a partir de um arquivo .toml ou de uma URL (links do GitHub valem).
    Install {
        /// Caminho ou URL do .toml
        source: String,
        /// Substitui um tema instalado com o mesmo nome.
        #[arg(long)]
        force: bool,
    },
    /// Remove um tema instalado.
    Remove { name: String },
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
        Some(Cmd::Theme { cmd: ThemeCmd::Install { source, force } }) => theme_install(&source, force, &paths),
        Some(Cmd::Theme { cmd: ThemeCmd::Remove { name } }) => {
            let p = theme::remove(&name, &paths.themes_dir())?;
            println!("removido {}", p.display());
            Ok(())
        }
        Some(Cmd::Config { cmd: ConfigCmd::Path }) => {
            println!("config  {}", cli.config.unwrap_or_else(|| paths.config_file()).display());
            println!("temas   {}", paths.themes_dir().display());
            println!("estado  {}", paths.state_file().display());
            println!("log     {}", paths.state_dir.join("cockpit.log").display());
            Ok(())
        }
        Some(Cmd::Config { cmd: ConfigCmd::Init }) => config_init(cli.config.unwrap_or_else(|| paths.config_file())),
        Some(Cmd::Auth { cmd: AuthCmd::Google { client_secret } }) => auth_google(cli.config, client_secret, &paths),
        Some(Cmd::Doctor) => doctor::run(cli.config, &paths),
        Some(Cmd::ShellInit { shell }) => {
            print!("{}", shell::init_script(&shell)?);
            Ok(())
        }
        None => run(cli.config, cli.theme, cli.cd_file, cli.demo_agenda, paths),
    }
}

fn run(
    config_path: Option<PathBuf>,
    theme_arg: Option<String>,
    cd_file: Option<PathBuf>,
    demo_agenda: bool,
    paths: Paths,
) -> Result<()> {
    let _log_guard = init_logging(&paths);
    let loaded = config::load(config_path);
    let mut notices: Vec<String> = loaded.warning.clone().into_iter().collect();
    let truecolor = theme::truecolor_supported();

    let themes = ThemeRegistry::load(&paths.themes_dir(), truecolor);
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
    let config_path = loaded.path.clone();
    let mut state = AppState::new(loaded.config, paths, themes, theme_idx, notices);
    state.config_path = config_path;
    state.truecolor = truecolor;
    state.has_cd_file = cd_file.is_some();
    state.demo_agenda = demo_agenda;
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

fn auth_google(config_path: Option<PathBuf>, client_secret: Option<PathBuf>, paths: &Paths) -> Result<()> {
    use providers::calendar::oauth;
    let loaded = config::load(config_path);
    let client_path = client_secret.unwrap_or_else(|| oauth::client_path(&loaded.config.calendar));
    let client = oauth::load_client(&client_path).map_err(|e| {
        anyhow::anyhow!(
            "{e:#}\n\nCrie um cliente OAuth do tipo \"App para computador\" no Google Cloud, baixe o JSON e salve em\n  {}\n(passo a passo no README, seção Google Calendar).",
            client_path.display()
        )
    })?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let token_path = paths.google_token();
    rt.block_on(async {
        let http = providers::http_client();
        oauth::authorize(&http, &client, &token_path).await
    })?;
    println!("\nPronto: agenda conectada (somente leitura). Token em {}.", token_path.display());
    println!("Um cockpit já aberto pega a agenda sozinho em até 15 s.");
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

fn theme_install(source: &str, force: bool, paths: &Paths) -> Result<()> {
    let (text, fallback) = if source.starts_with("http://") || source.starts_with("https://") {
        let url = theme::raw_url(source);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let text = rt.block_on(async {
            let resp = providers::http_client().get(&url).send().await?.error_for_status()?;
            if resp.content_length().is_some_and(|n| n > 256 * 1024) {
                bail!("arquivo grande demais para ser um tema");
            }
            Ok::<_, anyhow::Error>(resp.text().await?)
        })?;
        let stem = url.rsplit('/').next().unwrap_or("tema").trim_end_matches(".toml").to_string();
        (text, stem)
    } else {
        let p = PathBuf::from(source);
        let text = fs::read_to_string(&p).with_context(|| format!("não li {}", p.display()))?;
        (text, p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "tema".into()))
    };
    let i = theme::install(&text, &fallback, &paths.themes_dir(), force)?;
    println!("instalado {} → {}", i.name, i.path.display());
    if i.overrides_builtin {
        println!("(substitui o tema embutido de mesmo nome)");
    }
    println!("Num cockpit aberto ele aparece na hora: aperte `t` até chegar nele, ou ponha theme = \"{}\" no config.", i.name);
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
