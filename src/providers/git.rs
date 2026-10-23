//! Repositórios e worktrees: as pastas que o seletor do chat novo oferece. Varre as
//! pastas do config atrás de repositórios e lista as worktrees de cada um; relê quando
//! uma worktree é criada ou removida, ou a cada `refresh_secs`.

use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use notify::{RecursiveMode, Watcher};
use tokio::{
    process::Command,
    sync::{Semaphore, mpsc, watch},
    time::Instant,
};

use super::Snapshot;
use crate::config::{WorktreesConfig, expand_home};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorktreesSnapshot {
    pub repos: Vec<Repo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub worktrees: Vec<Worktree>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Nome da pasta; na principal, o nome do repositório.
    pub name: String,
    pub is_main: bool,
    pub branch: Option<String>,
    pub detached: bool,
    /// A pasta sumiu, mas o git ainda lista (`git worktree prune` resolve).
    pub prunable: bool,
}

/// Intervalo mínimo entre varreduras disparadas por mudança em disco.
const MIN_EVENT_GAP: Duration = Duration::from_secs(3);

pub fn spawn(cfg: WorktreesConfig, tx: watch::Sender<Snapshot<WorktreesSnapshot>>) {
    tokio::spawn(async move {
        let (ev_tx, mut ev_rx) = mpsc::channel::<()>(1);
        // Só interessa worktree criada/removida ou troca de branch; escrita no índice
        // acontece o tempo todo e não deve disparar nada.
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && super::is_change(&ev)
                && ev.paths.iter().any(|p| {
                    p.components().any(|c| c.as_os_str() == "worktrees") || p.file_name().is_some_and(|n| n == "HEAD")
                })
            {
                let _ = ev_tx.try_send(());
            }
        })
        .map_err(|e| tracing::warn!("sem inotify para worktrees: {e}"))
        .ok();
        let mut watched: HashSet<PathBuf> = HashSet::new();

        let sem = Arc::new(Semaphore::new(2));
        let refresh = Duration::from_secs(cfg.refresh_secs.max(5));
        loop {
            let workspaces: Vec<PathBuf> = cfg.workspaces.iter().map(|w| expand_home(w)).collect();
            let depth = cfg.depth;
            let repos = tokio::task::spawn_blocking(move || discover_repos(&workspaces, depth)).await.unwrap_or_default();
            let snap = scan(&repos, &cfg, &sem).await;

            if let Some(w) = watcher.as_mut() {
                for r in &repos {
                    for p in [r.join(".git"), r.join(".git/worktrees")] {
                        if p.is_dir() && watched.insert(p.clone()) {
                            let _ = w.watch(&p, RecursiveMode::NonRecursive);
                        }
                    }
                }
            }
            if tx.send(Snapshot::ok(snap)).is_err() {
                return;
            }

            let wait_started = Instant::now();
            tokio::select! {
                _ = tokio::time::sleep(refresh) => {}
                Some(()) = ev_rx.recv() => {
                    // espera o git terminar de escrever antes de reler
                    let gap = MIN_EVENT_GAP.saturating_sub(wait_started.elapsed()).max(Duration::from_millis(500));
                    tokio::time::sleep(gap).await;
                    while ev_rx.try_recv().is_ok() {}
                }
            }
        }
    });
}

/// Pastas que nunca têm repositório dentro (e custam caro para varrer).
const SKIP_DIRS: &[&str] = &["node_modules", "target", "vendor", "venv", "__pycache__", "dist", "build", "snap", "go", "tmp"];

/// Repositórios principais (com `.git` pasta) até `depth` níveis abaixo de cada workspace.
/// Um `.git` arquivo é uma worktree ligada: ela aparece pelo repositório dono.
pub fn discover_repos(workspaces: &[PathBuf], depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for ws in workspaces {
        let mut queue = VecDeque::from([(ws.clone(), 0usize)]);
        while let Some((dir, d)) = queue.pop_front() {
            if dir.join(".git").is_dir() {
                if seen.insert(dir.clone()) {
                    out.push(dir);
                }
                continue;
            }
            if d >= depth {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut subs: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    !n.starts_with('.') && !SKIP_DIRS.contains(&n.as_ref())
                })
                .map(|e| e.path())
                .collect();
            subs.sort();
            queue.extend(subs.into_iter().map(|p| (p, d + 1)));
        }
    }
    out
}

async fn scan(repos: &[PathBuf], cfg: &WorktreesConfig, sem: &Arc<Semaphore>) -> WorktreesSnapshot {
    let mut out = Vec::new();
    for repo in repos {
        let Some(list) = git(repo, &["worktree", "list", "--porcelain"], sem).await else { continue };
        let wts = parse_worktree_list(&list, repo);
        if cfg.only_with_worktrees && wts.len() < 2 {
            continue;
        }
        let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Repo { name, path: repo.clone(), worktrees: wts });
    }
    WorktreesSnapshot { repos: out }
}

async fn git(dir: &Path, args: &[&str], sem: &Semaphore) -> Option<String> {
    let _permit = sem.acquire().await.ok()?;
    let fut = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(10), fut).await {
        Ok(Ok(o)) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(Ok(o)) => {
            tracing::debug!("git {args:?} em {}: {}", dir.display(), String::from_utf8_lossy(&o.stderr).trim());
            None
        }
        Ok(Err(e)) => {
            tracing::warn!("git não executou: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("git {args:?} em {} passou de 10s", dir.display());
            None
        }
    }
}

/// `git worktree list --porcelain`. A primeira entrada é sempre a principal.
fn parse_worktree_list(s: &str, repo: &Path) -> Vec<Worktree> {
    let repo_name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut out: Vec<Worktree> = Vec::new();
    for block in s.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut w = Worktree::default();
        let mut bare = false;
        for line in block.lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "worktree" => w.path = PathBuf::from(v),
                "branch" => w.branch = Some(v.strip_prefix("refs/heads/").unwrap_or(v).to_string()),
                "detached" => w.detached = true,
                "prunable" => w.prunable = true,
                "bare" => bare = true,
                _ => {}
            }
        }
        if bare || w.path.as_os_str().is_empty() {
            continue;
        }
        w.is_main = out.is_empty();
        w.name = if w.is_main {
            repo_name.clone()
        } else {
            w.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        };
        out.push(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "worktree /home/u/v360\nHEAD f15a\nbranch refs/heads/release\n\nworktree /home/u/v360/.claude/worktrees/cfs\nHEAD cb0e\nbranch refs/heads/feat/cfs\n\nworktree /home/u/v360/.claude/worktrees/old\nHEAD 1234\ndetached\nprunable gitdir file points to non-existent location\n\n";

    #[test]
    fn parses_worktree_list() {
        let w = parse_worktree_list(LIST, Path::new("/home/u/v360"));
        assert_eq!(w.len(), 3);
        assert!(w[0].is_main);
        assert_eq!(w[0].name, "v360");
        assert_eq!(w[0].branch.as_deref(), Some("release"));
        assert_eq!(w[1].name, "cfs");
        assert_eq!(w[1].branch.as_deref(), Some("feat/cfs"));
        assert!(w[2].detached && w[2].prunable && w[2].branch.is_none());
    }

    #[test]
    fn discovers_repos_and_skips_linked_worktrees() {
        let root = std::env::temp_dir().join(format!("cockpit-git-{}", std::process::id()));
        let mk = |p: &str| fs::create_dir_all(root.join(p)).unwrap();
        mk("a/.git");
        mk("grupo/b/.git");
        mk("grupo/node_modules/c/.git");
        mk(".escondido/d/.git");
        mk("fundo/x/y/e/.git"); // além da profundidade
        mk("wt");
        fs::write(root.join("wt/.git"), "gitdir: /outro").unwrap(); // worktree ligada
        let got = discover_repos(std::slice::from_ref(&root), 2);
        let names: Vec<String> =
            got.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string()).collect();
        fs::remove_dir_all(&root).ok();
        assert_eq!(names, ["a", "grupo/b"]);
    }
}
