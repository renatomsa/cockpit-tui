//! Status de cada worktree no fluxo da melhoria (shape → wip → revisão manual → deploy →
//! finalizado). Quem escolhe é a pessoa: `s` no painel de worktrees ou `cockpit status` no
//! terminal, de dentro da worktree. Fica em `worktree-status.toml` na pasta de estado,
//! pelo caminho da worktree; o cockpit aberto observa o arquivo e atualiza a coluna.

use std::{
    collections::BTreeMap,
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{Paths, tilde};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WtStatus {
    Shape,
    Wip,
    RevisaoManual,
    Deploy,
    Finalizado,
}

/// Opções do seletor: os cinco status na ordem do fluxo e, por último, "sem status".
pub const CHOICES: [Option<WtStatus>; 6] = [
    Some(WtStatus::Shape),
    Some(WtStatus::Wip),
    Some(WtStatus::RevisaoManual),
    Some(WtStatus::Deploy),
    Some(WtStatus::Finalizado),
    None,
];

/// Status gravado de cada worktree, pelo caminho dela.
pub type Statuses = BTreeMap<PathBuf, WtStatus>;

impl WtStatus {
    /// Na ordem do fluxo; é a ordem do seletor e o número que escolhe (1 a 5).
    pub const ALL: [WtStatus; 5] = [Self::Shape, Self::Wip, Self::RevisaoManual, Self::Deploy, Self::Finalizado];

    pub fn label(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::Wip => "wip",
            Self::RevisaoManual => "revisão manual",
            Self::Deploy => "deploy",
            Self::Finalizado => "finalizado",
        }
    }

    /// Texto da coluna, curto para não roubar espaço do nome e da branch.
    pub fn short(self) -> &'static str {
        match self {
            Self::RevisaoManual => "revisão",
            s => s.label(),
        }
    }

    /// Aceita o nome com ou sem acento, só o começo ("rev", "fin") ou o número do seletor.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace('ã', "a").replace(['-', '_'], " ");
        if let Ok(n) = s.parse::<usize>() {
            return Self::ALL.get(n.checked_sub(1)?).copied();
        }
        if s.is_empty() {
            return None;
        }
        let plain = |w: Self| w.label().replace('ã', "a");
        Self::ALL.into_iter().find(|w| plain(*w) == s).or_else(|| {
            let mut hits = Self::ALL.into_iter().filter(|w| plain(*w).starts_with(&s));
            let first = hits.next()?;
            hits.next().is_none().then_some(first)
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    worktrees: BTreeMap<String, WtStatus>,
}

/// Arquivo ausente ou ilegível vale como "nenhum status".
pub fn load(path: &Path) -> Statuses {
    let file: File = fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
    file.worktrees.into_iter().map(|(k, v)| (PathBuf::from(k), v)).collect()
}

/// Relê o arquivo, troca só esta worktree e grava (cockpit e terminal escrevem no mesmo
/// arquivo; regravar o mapa inteiro da memória apagaria o que o outro acabou de mudar).
pub fn set(path: &Path, worktree: &Path, status: Option<WtStatus>) -> Result<Statuses> {
    let mut all = load(path);
    match status {
        Some(s) => all.insert(worktree.to_path_buf(), s),
        None => all.remove(worktree),
    };
    let file = File { worktrees: all.iter().map(|(k, v)| (k.to_string_lossy().into_owned(), *v)).collect() };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Grava ao lado e renomeia: quem observa o arquivo nunca lê ele pela metade.
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, toml::to_string(&file)?).with_context(|| format!("gravar {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("gravar {}", path.display()))?;
    Ok(all)
}

/// `cockpit status [estado]`: sem estado e num terminal, abre o seletor.
pub fn cli(state: Option<String>, dir: Option<PathBuf>, list: bool, paths: &Paths) -> Result<()> {
    let file = paths.wt_status_file();
    if list {
        let all = load(&file);
        if all.is_empty() {
            println!("nenhuma worktree com status (use `cockpit status` dentro de uma)");
        }
        for (p, s) in &all {
            println!("{:<15} {}", s.label(), tilde(p));
        }
        return Ok(());
    }
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let wt = toplevel(&dir)?;
    let current = load(&file).get(&wt).copied();
    let name = tilde(&wt);
    let choice = match state.as_deref() {
        Some("nenhum" | "limpar" | "-" | "0") => Some(None),
        Some(s) => match WtStatus::parse(s) {
            Some(st) => Some(Some(st)),
            None => bail!("status `{s}` não existe; use {}, ou nenhum para limpar", names()),
        },
        None if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() => pick(&name, current)?,
        None => {
            println!("{}", current.map_or("sem status", WtStatus::label));
            return Ok(());
        }
    };
    let Some(choice) = choice else {
        println!("nada mudou");
        return Ok(());
    };
    set(&file, &wt, choice)?;
    match choice {
        Some(s) => println!("{name}: {}", s.label()),
        None => println!("{name}: sem status"),
    }
    Ok(())
}

fn names() -> String {
    WtStatus::ALL.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
}

/// Raiz da worktree que contém `dir` (o mesmo caminho que o `git worktree list` mostra).
fn toplevel(dir: &Path) -> Result<PathBuf> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--show-toplevel"]).output().context("git não executou")?;
    if !out.status.success() {
        bail!("{} não está numa worktree git (use --dir para apontar uma)", dir.display());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Seletor no próprio terminal: ↑↓ e Enter, ou o número direto. `None` = cancelou;
/// `Some(None)` = limpar.
fn pick(name: &str, current: Option<WtStatus>) -> Result<Option<Option<WtStatus>>> {
    use crossterm::{
        cursor::MoveUp,
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute,
        terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode},
    };
    let options = CHOICES;
    let mut sel = current.and_then(|c| options.iter().position(|o| *o == Some(c))).unwrap_or(0);
    let mut out = std::io::stdout();
    println!("status de {name}  (↑↓ escolhe · Enter grava · 1-5 direto · 0 limpa · Esc cancela)");
    let draw = |out: &mut std::io::Stdout, sel: usize, first: bool| -> Result<()> {
        if !first {
            execute!(out, MoveUp(options.len() as u16))?;
        }
        for (i, o) in options.iter().enumerate() {
            let text = match o {
                Some(s) => format!("{} {}", i + 1, s.label()),
                None => "0 sem status".into(),
            };
            let mark = if *o == current { "  (atual)" } else { "" };
            let line = if i == sel { format!("\x1b[1m▸ {text}{mark}\x1b[0m") } else { format!("  {text}{mark}") };
            execute!(out, Clear(ClearType::CurrentLine))?;
            write!(out, "\r{line}\r\n")?;
        }
        out.flush()?;
        Ok(())
    };
    enable_raw_mode()?;
    let res = (|| -> Result<Option<Option<WtStatus>>> {
        draw(&mut out, sel, true)?;
        loop {
            let Event::Key(k) = event::read()? else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => sel = (sel + 1).min(options.len() - 1),
                KeyCode::Enter => return Ok(Some(options[sel])),
                KeyCode::Char('0') | KeyCode::Backspace | KeyCode::Delete => return Ok(Some(None)),
                KeyCode::Char(c) => {
                    if let Some(s) = c.to_digit(10).and_then(|n| WtStatus::parse(&n.to_string())) {
                        return Ok(Some(Some(s)));
                    }
                    continue;
                }
                _ => continue,
            }
            draw(&mut out, sel, false)?;
        }
    })();
    disable_raw_mode()?;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_accents_prefixes_and_numbers() {
        assert_eq!(WtStatus::parse("revisão manual"), Some(WtStatus::RevisaoManual));
        assert_eq!(WtStatus::parse("revisao-manual"), Some(WtStatus::RevisaoManual));
        assert_eq!(WtStatus::parse("rev"), Some(WtStatus::RevisaoManual));
        assert_eq!(WtStatus::parse("WIP"), Some(WtStatus::Wip));
        assert_eq!(WtStatus::parse("1"), Some(WtStatus::Shape));
        assert_eq!(WtStatus::parse("5"), Some(WtStatus::Finalizado));
        assert_eq!(WtStatus::parse("6"), None);
        assert_eq!(WtStatus::parse("0"), None);
        assert_eq!(WtStatus::parse("xyz"), None);
    }

    #[test]
    fn set_rereads_the_file_and_keeps_other_worktrees() {
        let dir = std::env::temp_dir().join(format!("cockpit-wt-status-{}", std::process::id()));
        let file = dir.join("worktree-status.toml");
        set(&file, Path::new("/h/v360"), Some(WtStatus::Wip)).unwrap();
        set(&file, Path::new("/h/v360/.claude/worktrees/a"), Some(WtStatus::Deploy)).unwrap();
        let all = set(&file, Path::new("/h/v360"), None).unwrap();
        assert_eq!(all, load(&file));
        assert_eq!(all.into_iter().collect::<Vec<_>>(), [(PathBuf::from("/h/v360/.claude/worktrees/a"), WtStatus::Deploy)]);
        let _ = fs::remove_dir_all(dir);
    }
}
