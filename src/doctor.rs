//! `cockpit doctor`: confere cada peça do cockpit e diz, em português, o que falta e
//! como resolver. Só lê: não instala nem altera nada.

use std::{path::Path, time::Duration};

use crate::{
    config::{self, Paths},
    platform,
    providers::{self, calendar::oauth, claude, git},
    theme::{self, ThemeRegistry},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Ok,
    Warn,
    Fail,
    Info,
}

struct Check {
    area: &'static str,
    st: St,
    msg: String,
    hint: Option<String>,
}

fn ok(area: &'static str, msg: impl Into<String>) -> Check {
    Check { area, st: St::Ok, msg: msg.into(), hint: None }
}
fn warn(area: &'static str, msg: impl Into<String>, hint: impl Into<String>) -> Check {
    Check { area, st: St::Warn, msg: msg.into(), hint: Some(hint.into()) }
}
fn fail(area: &'static str, msg: impl Into<String>, hint: impl Into<String>) -> Check {
    Check { area, st: St::Fail, msg: msg.into(), hint: Some(hint.into()) }
}
fn info(area: &'static str, msg: impl Into<String>) -> Check {
    Check { area, st: St::Info, msg: msg.into(), hint: None }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

pub fn run(config_path: Option<std::path::PathBuf>, paths: &Paths) -> anyhow::Result<()> {
    let loaded = config::load(config_path);
    let cfg = loaded.config.clone();
    let mut checks: Vec<Check> = Vec::new();

    // ── Config e temas
    checks.push(match &loaded.warning {
        Some(w) => fail("config", w.clone(), format!("corrija {}; o cockpit abre com os padrões enquanto isso", loaded.path.display())),
        None if loaded.path.exists() => ok("config", format!("{}", loaded.path.display())),
        None => info("config", format!("sem {} (usando os padrões; `cockpit config init` cria um comentado)", loaded.path.display())),
    });
    let themes = ThemeRegistry::load(&paths.themes_dir(), true);
    let installed = themes.entries.iter().filter(|e| !e.builtin).count();
    checks.push(if themes.errors.is_empty() {
        ok("temas", format!("{} disponíveis ({installed} instalados)", themes.len()))
    } else {
        warn("temas", format!("{} com erro: {}", themes.errors.len(), themes.errors.join("; ")), "corrija ou remova o arquivo com `cockpit theme remove`")
    });
    if themes.index_of(&cfg.theme).is_none() {
        checks.push(warn("temas", format!("o tema `{}` do config não existe", cfg.theme), "veja `cockpit theme list`"));
    }
    checks.push(if theme::truecolor_supported() {
        ok("terminal", "cores de 24 bits")
    } else {
        warn("terminal", "sem truecolor detectado: cores convertidas para 256", "defina COLORTERM=truecolor se o seu terminal suporta")
    });

    // ── Shell
    let shell = std::env::var("SHELL").unwrap_or_default();
    let rc = if shell.ends_with("zsh") {
        Some(".zshrc")
    } else if shell.ends_with("bash") {
        Some(".bashrc")
    } else if shell.ends_with("fish") {
        Some(".config/fish/config.fish")
    } else {
        None
    };
    let name = shell.rsplit('/').next().unwrap_or("zsh").to_string();
    if let Some(rc) = rc {
        let p = config::expand_home(&format!("~/{rc}"));
        let has = std::fs::read_to_string(&p).is_ok_and(|s| s.contains("cockpit shell-init"));
        checks.push(if has {
            ok("shell", format!("wrapper do `cd` em ~/{rc}"))
        } else {
            warn("shell", "sem o wrapper: o Enter nas worktrees só copia o `cd`", format!("echo 'eval \"$(cockpit shell-init {name})\"' >> ~/{rc}"))
        });
    }
    checks.push(match crate::terminal::detect(&cfg.terminal.new_tab) {
        Some(l) => ok("abas", format!("C e r abrem aba nova via {}", l.name())),
        None => warn("abas", "terminal não reconhecido: C e r saem do cockpit em vez de abrir aba", "defina [terminal].new_tab no config"),
    });

    // ── Sistema
    checks.push(if Path::new("/proc/pressure/memory").exists() {
        ok("sistema", "PSI disponível (detector de travamento completo)")
    } else {
        warn("sistema", "kernel sem PSI: o detector usa só memória, swap, CPU e disco", "kernel 4.20+ com CONFIG_PSI")
    });

    // ── Git e worktrees
    checks.push(match std::process::Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => {
            let repos = git::discover_repos(&cfg.worktrees.workspaces.iter().map(|w| config::expand_home(w)).collect::<Vec<_>>(), cfg.worktrees.depth);
            if repos.is_empty() {
                warn("worktrees", format!("nenhum repositório em {}", cfg.worktrees.workspaces.join(", ")), "ajuste [worktrees].workspaces e depth")
            } else {
                ok("worktrees", format!("{} repositórios em {}", repos.len(), cfg.worktrees.workspaces.join(", ")))
            }
        }
        _ => fail("worktrees", "git não encontrado", "instale o git"),
    });

    // ── Claude
    let dir = claude::claude_dir(&cfg.claude);
    let sessions = dir.join("sessions");
    checks.push(match std::fs::read_dir(&sessions) {
        Ok(rd) => ok("claude", format!("{} arquivos de sessão em {}", rd.count(), sessions.display())),
        Err(_) => warn("claude", format!("sem {}", sessions.display()), "abra o `claude` uma vez; ou ajuste [claude].dir"),
    });

    // ── WSL
    if platform::is_wsl() {
        checks.push(if which("powershell.exe") {
            ok("wsl", "powershell.exe acessível (Wi-Fi do Windows, especificações)")
        } else {
            warn("wsl", "powershell.exe não está no PATH", "ative appendWindowsPath no /etc/wsl.conf")
        });
        if let Some(c) = wslconfig() {
            checks.push(c);
        }
    }

    // ── Rede, Docker, Google, limites: em paralelo, com prazo
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let net = rt.block_on(network_checks(&cfg, paths));
    checks.extend(net);

    print(&checks);
    Ok(())
}

async fn network_checks(cfg: &config::Config, paths: &Paths) -> Vec<Check> {
    let http = providers::http_client();
    let t = |d| Duration::from_secs(d);

    let geo = async {
        if cfg.location.city.is_some() {
            return ok("cidade", format!("fixa no config: {}", cfg.location.city.clone().unwrap_or_default()));
        }
        match tokio::time::timeout(t(8), http.get("https://ipinfo.io/json").send()).await {
            Ok(Ok(r)) if r.status().is_success() => {
                let v: serde_json::Value = r.json().await.unwrap_or_default();
                let city = v.get("city").and_then(|c| c.as_str()).unwrap_or("?");
                warn("cidade", format!("pelo IP: {city}"), "se estiver errada, fixe em [location].city no config")
            }
            _ => warn("cidade", "geolocalização por IP indisponível", "fixe em [location].city no config"),
        }
    };
    let ping = async {
        let target = if cfg.system.ping_target.contains(':') { cfg.system.ping_target.clone() } else { format!("{}:443", cfg.system.ping_target) };
        let start = std::time::Instant::now();
        match tokio::time::timeout(t(3), tokio::net::TcpStream::connect(&target)).await {
            Ok(Ok(_)) => ok("ping", format!("{target} em {:.0} ms", start.elapsed().as_secs_f64() * 1000.0)),
            _ => fail("ping", format!("{target} não respondeu"), "confira a internet ou mude [system].ping_target"),
        }
    };
    let weather = async {
        if !cfg.weather.enabled {
            return info("clima", "desligado ([weather].enabled = false)");
        }
        match tokio::time::timeout(t(8), http.get("https://api.open-meteo.com/v1/forecast?latitude=0&longitude=0&current=temperature_2m").send()).await {
            Ok(Ok(r)) if r.status().is_success() => ok("clima", "Open-Meteo responde"),
            _ => warn("clima", "Open-Meteo não respondeu", "o painel tenta de novo sozinho"),
        }
    };
    let docker = async {
        match std::os::unix::net::UnixStream::connect("/var/run/docker.sock") {
            Ok(_) => ok("docker", "socket acessível"),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                warn("docker", "sem permissão no /var/run/docker.sock", "sudo usermod -aG docker $USER e abra um shell novo")
            }
            Err(_) if which("docker") => warn("docker", "o daemon não está rodando", "sudo service docker start"),
            Err(_) => info("docker", "Docker não instalado (o painel fica vazio)"),
        }
    };
    let usage = async {
        if !cfg.claude.usage_limits {
            return info("limites", "desligados ([claude].usage_limits = false)");
        }
        let creds = claude::claude_dir(&cfg.claude).join(".credentials.json");
        let raw = std::fs::read_to_string(&creds).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        match v.get("claudeAiOauth").and_then(|o| o.get("expiresAt")).and_then(|e| e.as_i64()) {
            None => info("limites", "sem login do claude.ai (com API key não há limite de plano)"),
            Some(exp) if exp < chrono::Utc::now().timestamp_millis() => warn("limites", "login do Claude vencido", "abra o `claude`, que ele renova"),
            Some(_) => ok("limites", "login do Claude válido (sessão 5 h e semana)"),
        }
    };
    let calendar = async {
        if !cfg.calendar.enabled {
            return info("agenda", "desligada ([calendar].enabled = false)");
        }
        let cp = oauth::client_path(&cfg.calendar);
        let Ok(client) = oauth::load_client(&cp) else {
            return warn("agenda", format!("sem cliente OAuth em {}", cp.display()), "README, seção Google Calendar (ou `cockpit --demo-agenda` para ver o alerta)");
        };
        let Some(mut tok) = oauth::load_token(&paths.google_token()) else {
            return warn("agenda", "cliente OAuth ok, mas não conectado", "rode `cockpit auth google`");
        };
        if !tok.fresh() {
            match tokio::time::timeout(t(10), oauth::refresh(&http, &client, &tok)).await {
                Ok(Ok(n)) => tok = n,
                Ok(Err(e)) => return fail("agenda", format!("{e:#}"), "rode `cockpit auth google` de novo"),
                Err(_) => return warn("agenda", "Google não respondeu", "tente de novo"),
            }
        }
        let r = http.get("https://www.googleapis.com/calendar/v3/users/me/calendarList?maxResults=1&fields=items(id)").bearer_auth(&tok.access_token).send();
        match tokio::time::timeout(t(10), r).await {
            Ok(Ok(r)) if r.status().is_success() => ok("agenda", "Google Calendar conectado"),
            Ok(Ok(r)) => fail("agenda", format!("Google Calendar respondeu {}", r.status()), "ative a Calendar API no projeto ou rode `cockpit auth google`"),
            _ => warn("agenda", "Google não respondeu", "tente de novo"),
        }
    };
    let (a, b, c, d, e, f) = tokio::join!(geo, ping, weather, docker, usage, calendar);
    vec![a, b, c, d, e, f]
}

/// Memória do WSL: sem limite e sem autoMemoryReclaim, a VM segura o cache do Linux e o
/// Windows compensa aumentando o pagefile.
fn wslconfig() -> Option<Check> {
    let users = std::fs::read_dir("/mnt/c/Users").ok()?;
    let home = std::env::var("HOME").unwrap_or_default();
    let user = home.rsplit('/').next().unwrap_or("");
    let path = users
        .flatten()
        .map(|e| e.path().join(".wslconfig"))
        .filter(|p| p.is_file())
        .find(|p| p.to_string_lossy().contains(user))
        .or_else(|| std::fs::read_dir("/mnt/c/Users").ok()?.flatten().map(|e| e.path().join(".wslconfig")).find(|p| p.is_file()))?;
    let text = std::fs::read_to_string(&path).ok()?.to_lowercase();
    let reclaim = text.lines().any(|l| l.replace(' ', "").starts_with("automemoryreclaim=") && !l.contains("disabled"));
    // memory=8GB, 8192MB ou em bytes (8589934592)
    let mem = text.lines().find_map(|l| l.replace(' ', "").strip_prefix("memory=").map(str::to_string)).map(|m| match m.parse::<u64>() {
        Ok(b) => format!("{:.0}GB", b as f64 / (1u64 << 30) as f64),
        Err(_) => m.to_uppercase(),
    });
    Some(match (mem, reclaim) {
        (_, true) => ok("wsl", "autoMemoryReclaim ligado no .wslconfig"),
        (Some(m), false) => info("wsl", format!("WSL com memory={m} e sem autoMemoryReclaim: o cache do Linux fica preso na VM (pode inflar o pagefile do Windows)")),
        (None, false) => info("wsl", "sem autoMemoryReclaim no .wslconfig: o cache do Linux fica preso na VM"),
    })
}

fn print(checks: &[Check]) {
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let paint = |code: &str, s: &str| if tty { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() };
    let mut worst = St::Ok;
    for c in checks {
        let (icon, color) = match c.st {
            St::Ok => ("✓", "32"),
            St::Warn => ("⚠", "33"),
            St::Fail => ("✗", "31"),
            St::Info => ("·", "2"),
        };
        if matches!(c.st, St::Fail) || (matches!(c.st, St::Warn) && worst != St::Fail) {
            worst = c.st;
        }
        println!("{} {:<10} {}", paint(color, icon), c.area, c.msg);
        if let Some(h) = &c.hint {
            println!("  {:<10} {}", "", paint("2", &format!("→ {h}")));
        }
    }
    println!();
    println!(
        "{}",
        match worst {
            St::Fail => paint("31", "Há itens com ✗: o painel correspondente não vai funcionar."),
            St::Warn => paint("33", "Tudo roda; os itens com ⚠ deixam alguma parte limitada."),
            _ => paint("32", "Tudo certo."),
        }
    );
}
