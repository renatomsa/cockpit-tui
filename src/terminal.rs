//! Abre uma aba/janela nova de terminal rodando um comando numa pasta, sem fechar o
//! cockpit. Detecta o ambiente (tmux, Windows Terminal pelo WSL, kitty, WezTerm,
//! gnome-terminal) ou usa o modelo de `[terminal].new_tab` do config.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launcher {
    /// Janela nova na sessão tmux atual.
    Tmux,
    /// Aba nova na janela atual do Windows Terminal, entrando na distro do WSL.
    WindowsTerminal { distro: String },
    Kitty,
    Wezterm,
    GnomeTerminal,
    /// Modelo do usuário, rodado com `sh -c`; `{dir}`, `{cmd}` e `{title}` são substituídos
    /// (já com aspas de shell).
    Custom(String),
}

impl Launcher {
    pub fn name(&self) -> &str {
        match self {
            Launcher::Tmux => "tmux",
            Launcher::WindowsTerminal { .. } => "Windows Terminal",
            Launcher::Kitty => "kitty",
            Launcher::Wezterm => "WezTerm",
            Launcher::GnomeTerminal => "gnome-terminal",
            Launcher::Custom(_) => "comando do config",
        }
    }
}

/// Primeiro o que o usuário configurou; depois o terminal em que o cockpit está rodando.
pub fn detect(custom: &str) -> Option<Launcher> {
    if !custom.trim().is_empty() {
        return Some(Launcher::Custom(custom.to_string()));
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if env("TMUX").is_some() && which("tmux") {
        return Some(Launcher::Tmux);
    }
    if env("WT_SESSION").is_some()
        && let Some(distro) = env("WSL_DISTRO_NAME")
        && which("wt.exe")
    {
        return Some(Launcher::WindowsTerminal { distro });
    }
    if env("KITTY_WINDOW_ID").is_some() && which("kitty") {
        return Some(Launcher::Kitty);
    }
    if env("WEZTERM_PANE").is_some() && which("wezterm") {
        return Some(Launcher::Wezterm);
    }
    if which("gnome-terminal") && (env("DISPLAY").is_some() || env("WAYLAND_DISPLAY").is_some()) {
        return Some(Launcher::GnomeTerminal);
    }
    None
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// O que roda dentro da aba nova: o comando num shell interativo (para ter o PATH e os
/// aliases do usuário) e, quando ele termina, um shell normal, para a aba não fechar.
/// Sem `;`: o Windows Terminal usa `;` para separar os próprios subcomandos.
pub fn inner_script(shell: &str, cmd: &str) -> String {
    format!("{cmd} || true && exec {shell} -l")
}

/// Programa e argumentos para abrir `cmd` em `dir` com o lançador dado.
pub fn build(l: &Launcher, dir: &Path, cmd: &str, title: &str, shell: &str) -> (String, Vec<String>) {
    let d = dir.display().to_string();
    let inner = inner_script(shell, cmd);
    let s = |x: &str| x.to_string();
    match l {
        Launcher::Tmux => (s("tmux"), vec![s("new-window"), s("-c"), d, s("-n"), s(title), s(shell), s("-lic"), inner]),
        Launcher::WindowsTerminal { distro } => (
            s("wt.exe"),
            vec![
                s("-w"),
                s("0"),
                s("new-tab"),
                s("--title"),
                s(title),
                s("wsl.exe"),
                s("-d"),
                s(distro),
                s("--cd"),
                d,
                s("-e"),
                s(shell),
                s("-lic"),
                inner,
            ],
        ),
        Launcher::Kitty => (
            s("kitty"),
            vec![s("@"), s("launch"), s("--type=tab"), s("--tab-title"), s(title), s("--cwd"), d, s(shell), s("-lic"), inner],
        ),
        Launcher::Wezterm => (s("wezterm"), vec![s("cli"), s("spawn"), s("--cwd"), d, s("--"), s(shell), s("-lic"), inner]),
        Launcher::GnomeTerminal => (
            s("gnome-terminal"),
            vec![s("--tab"), s("--title"), s(title), s("--working-directory"), d, s("--"), s(shell), s("-lic"), inner],
        ),
        Launcher::Custom(tpl) => {
            let filled = tpl.replace("{dir}", &quote(&d)).replace("{cmd}", &quote(cmd)).replace("{title}", &quote(title));
            (s("sh"), vec![s("-c"), filled])
        }
    }
}

/// Abre a aba e espera só o lançador responder (o `wt.exe`, o `tmux` etc. voltam na hora).
pub fn open(l: &Launcher, dir: &Path, cmd: &str, title: &str) -> Result<()> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
    let (prog, args) = build(l, dir, cmd, title, &shell);
    tracing::info!(?prog, ?args, "abrindo aba nova");
    // Nada do lançador pode escrever na tela do cockpit.
    let out = Command::new(&prog)
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("não consegui rodar {prog}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!("{prog} saiu com {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_terminal_opens_a_tab_in_the_distro() {
        let (prog, args) = build(
            &Launcher::WindowsTerminal { distro: "Ubuntu-24.04".into() },
            Path::new("/home/u/v360/.claude/worktrees/cfs"),
            "claude --continue",
            "claude · cfs",
            "/usr/bin/zsh",
        );
        assert_eq!(prog, "wt.exe");
        assert_eq!(
            args,
            [
                "-w", "0", "new-tab", "--title", "claude · cfs", "wsl.exe", "-d", "Ubuntu-24.04", "--cd",
                "/home/u/v360/.claude/worktrees/cfs", "-e", "/usr/bin/zsh", "-lic",
                "claude --continue || true && exec /usr/bin/zsh -l",
            ]
        );
        assert!(!args.iter().any(|a| a.contains(';')), "o wt.exe trata `;` como separador");
    }

    #[test]
    fn tmux_and_custom() {
        let (prog, args) = build(&Launcher::Tmux, Path::new("/x"), "claude --resume ab-1", "t", "/bin/bash");
        assert_eq!(prog, "tmux");
        assert_eq!(args[..5], ["new-window", "-c", "/x", "-n", "t"]);
        let (prog, args) = build(
            &Launcher::Custom("alacritty --working-directory {dir} -e sh -c {cmd}".into()),
            Path::new("/tmp/it's"),
            "claude --continue",
            "t",
            "/bin/sh",
        );
        assert_eq!(prog, "sh");
        assert_eq!(args[1], r"alacritty --working-directory '/tmp/it'\''s' -e sh -c 'claude --continue'");
    }

    #[test]
    fn custom_template_wins() {
        assert_eq!(detect("meu-term {dir}"), Some(Launcher::Custom("meu-term {dir}".into())));
    }
}
