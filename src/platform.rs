//! Diferenças de plataforma: abrir uma URL no navegador (inclusive o do Windows, pelo WSL).

use std::process::{Command, Stdio};

use anyhow::{Result, bail};

pub fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease").is_ok_and(|s| s.to_lowercase().contains("microsoft"))
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Programa e argumentos para abrir `url`. No WSL, o `rundll32 url.dll` do Windows recebe a
/// URL inteira como um argumento, com `&` e tudo (o `cmd /c start` exigiria escape).
fn opener(url: &str) -> Option<(String, Vec<String>)> {
    let s = |x: &str| x.to_string();
    if which("wslview") {
        return Some((s("wslview"), vec![s(url)]));
    }
    if is_wsl() && which("rundll32.exe") {
        return Some((s("rundll32.exe"), vec![s("url.dll,FileProtocolHandler"), s(url)]));
    }
    if cfg!(target_os = "macos") {
        return Some((s("open"), vec![s(url)]));
    }
    if which("xdg-open") {
        return Some((s("xdg-open"), vec![s(url)]));
    }
    None
}

pub fn open_url(url: &str) -> Result<()> {
    let Some((prog, args)) = opener(url) else { bail!("não achei como abrir o navegador (wslview, xdg-open)") };
    Command::new(&prog).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opener_keeps_the_url_as_one_argument() {
        if let Some((_, args)) = opener("https://x.test/?a=1&b=2") {
            assert!(args.iter().any(|a| a == "https://x.test/?a=1&b=2"));
        }
    }
}
