//! Repositórios e worktrees. Varre as pastas do config atrás de repositórios, lista as
//! worktrees de cada um e lê o `git status` de cada worktree.
//!
//! O `git` roda com `--no-optional-locks`: ler o status não pode disputar o lock do
//! índice com o git do usuário. Tamanho em disco é calculado à parte, a cada 10 min.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    os::unix::fs::MetadataExt,
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

impl WorktreesSnapshot {
    pub fn worktree_count(&self) -> usize {
        self.repos.iter().map(|r| r.worktrees.len()).sum()
    }
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
    /// Arquivos modificados + não rastreados.
    pub dirty: Option<u32>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// Epoch (s) do último commit.
    pub last_commit: Option<i64>,
    pub size: Option<u64>,
}

const SIZE_EVERY: Duration = Duration::from_secs(600);
/// Contar não rastreados é ~75% do custo do `git status` num repo grande: vai a cada
/// 10 varreduras (5 min no padrão) ou quando algo muda no `.git`.
const UNTRACKED_EVERY: u64 = 10;
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
        let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
        let (size_tx, mut size_rx) = mpsc::channel::<HashMap<PathBuf, u64>>(1);
        let mut size_due = Instant::now();
        let mut sizing = false;
        let mut untracked: HashMap<PathBuf, u32> = HashMap::new();
        let mut round: u64 = 0;
        let mut full = true;
        let refresh = Duration::from_secs(cfg.refresh_secs.max(5));
        loop {
            let workspaces: Vec<PathBuf> = cfg.workspaces.iter().map(|w| expand_home(w)).collect();
            let depth = cfg.depth;
            let repos = tokio::task::spawn_blocking(move || discover_repos(&workspaces, depth)).await.unwrap_or_default();
            let mut last = scan(&repos, &cfg, &sem, &sizes, &mut untracked, full).await;
            round += 1;
            full = round % UNTRACKED_EVERY == 0;

            if let Some(w) = watcher.as_mut() {
                for r in &repos {
                    for p in [r.join(".git"), r.join(".git/worktrees")] {
                        if p.is_dir() && watched.insert(p.clone()) {
                            let _ = w.watch(&p, RecursiveMode::NonRecursive);
                        }
                    }
                }
            }
            if tx.send(Snapshot::ok(last.clone())).is_err() {
                return;
            }

            if cfg.sizes && !sizing && Instant::now() >= size_due {
                sizing = true;
                let paths: Vec<PathBuf> =
                    last.repos.iter().flat_map(|r| r.worktrees.iter()).filter(|w| !w.prunable).map(|w| w.path.clone()).collect();
                let size_tx = size_tx.clone();
                tokio::task::spawn_blocking(move || {
                    let map = worktree_sizes(&paths);
                    let _ = size_tx.blocking_send(map);
                });
            }

            let wait_started = Instant::now();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(refresh) => break,
                    Some(()) = ev_rx.recv() => {
                        full = true;
                        // espera o git terminar de escrever antes de reler
                        let gap = MIN_EVENT_GAP.saturating_sub(wait_started.elapsed()).max(Duration::from_millis(500));
                        tokio::time::sleep(gap).await;
                        while ev_rx.try_recv().is_ok() {}
                        break;
                    }
                    Some(map) = size_rx.recv() => {
                        sizing = false;
                        size_due = Instant::now() + SIZE_EVERY;
                        sizes = map;
                        apply_sizes(&mut last, &sizes);
                        if tx.send(Snapshot::ok(last.clone())).is_err() {
                            return;
                        }
                    }
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

/// `full` também conta os não rastreados; sem ele, reaproveita a última contagem.
async fn scan(
    repos: &[PathBuf],
    cfg: &WorktreesConfig,
    sem: &Arc<Semaphore>,
    sizes: &HashMap<PathBuf, u64>,
    untracked: &mut HashMap<PathBuf, u32>,
    full: bool,
) -> WorktreesSnapshot {
    let mut out = Vec::new();
    for repo in repos {
        let Some(list) = git(repo, &["worktree", "list", "--porcelain"], sem).await else { continue };
        let mut wts = parse_worktree_list(&list, repo);
        if cfg.only_with_worktrees && wts.len() < 2 {
            continue;
        }
        let handles: Vec<_> = wts
            .iter()
            .filter(|w| !w.prunable)
            .map(|w| {
                let (p, sem) = (w.path.clone(), sem.clone());
                // sem contagem anterior desta worktree, conta agora
                let full = full || !untracked.contains_key(&p);
                tokio::spawn(async move {
                    let mode = if full { "-unormal" } else { "-uno" };
                    let status = git(&p, &["status", "--porcelain=v2", "--branch", mode], &sem).await;
                    let log = git(&p, &["log", "-1", "--format=%ct"], &sem).await;
                    (p, status, log, full)
                })
            })
            .collect();
        for h in handles {
            let Ok((p, status, log, counted)) = h.await else { continue };
            let Some(w) = wts.iter_mut().find(|w| w.path == p) else { continue };
            if let Some(st) = status {
                let s = parse_status(&st);
                if counted {
                    untracked.insert(p.clone(), s.untracked);
                }
                w.dirty = Some(s.tracked + untracked.get(&p).copied().unwrap_or(0));
                w.ahead = s.ahead;
                w.behind = s.behind;
                if w.branch.is_none() && !w.detached {
                    w.branch = s.head;
                }
            }
            w.last_commit = log.and_then(|l| l.trim().parse().ok());
        }
        let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Repo { name, path: repo.clone(), worktrees: wts });
    }
    let mut snap = WorktreesSnapshot { repos: out };
    apply_sizes(&mut snap, sizes);
    snap
}

fn apply_sizes(snap: &mut WorktreesSnapshot, sizes: &HashMap<PathBuf, u64>) {
    for w in snap.repos.iter_mut().flat_map(|r| r.worktrees.iter_mut()) {
        w.size = sizes.get(&w.path).copied();
    }
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

#[derive(Debug, Default, PartialEq)]
struct Status {
    head: Option<String>,
    tracked: u32,
    /// Só vale quando o status rodou com `-unormal`.
    untracked: u32,
    ahead: Option<u32>,
    behind: Option<u32>,
}

/// `git status --porcelain=v2 --branch`.
fn parse_status(s: &str) -> Status {
    let mut st = Status::default();
    for line in s.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            st.head = (h != "(detached)").then(|| h.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            st.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok());
            st.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if line.starts_with('?') {
            st.untracked += 1;
        } else if matches!(line.as_bytes().first(), Some(b'1' | b'2' | b'u')) {
            st.tracked += 1;
        }
    }
    st
}

/// Soma do espaço ocupado (blocos alocados) de cada worktree, sem sair do sistema de
/// arquivos e sem seguir symlinks. As pastas de outras worktrees dentro dela (a principal
/// contém `.claude/worktrees/*`) não entram na conta.
fn worktree_sizes(paths: &[PathBuf]) -> HashMap<PathBuf, u64> {
    let all: HashSet<&Path> = paths.iter().map(|p| p.as_path()).collect();
    let mut out = HashMap::new();
    for root in paths {
        let Ok(meta) = fs::symlink_metadata(root) else { continue };
        let dev = meta.dev();
        let mut total: u64 = 0;
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let Ok(m) = e.metadata() else { continue };
                total += m.blocks() * 512;
                if m.is_dir() && m.dev() == dev {
                    let p = e.path();
                    if !all.contains(p.as_path()) {
                        stack.push(p);
                    }
                }
            }
        }
        out.insert(root.clone(), total);
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
    fn parses_status() {
        let s = "# branch.oid abc\n# branch.head feat/x\n# branch.upstream origin/feat/x\n# branch.ab +2 -5\n1 .M N... 100644 100644 100644 a b src/a.rs\n? novo.txt\nu UU N... x\n";
        assert_eq!(
            parse_status(s),
            Status { head: Some("feat/x".into()), tracked: 2, untracked: 1, ahead: Some(2), behind: Some(5) }
        );
        let s = "# branch.head (detached)\n";
        assert_eq!(parse_status(s), Status::default());
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

    #[test]
    fn sizes_exclude_nested_worktrees() {
        let root = std::env::temp_dir().join(format!("cockpit-size-{}", std::process::id()));
        let nested = root.join(".claude/worktrees/x");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("a.bin"), vec![0u8; 64 * 1024]).unwrap();
        fs::write(nested.join("b.bin"), vec![0u8; 256 * 1024]).unwrap();
        let sizes = worktree_sizes(&[root.clone(), nested.clone()]);
        fs::remove_dir_all(&root).ok();
        let (r, n) = (sizes[&root], sizes[&nested]);
        assert!(n >= 256 * 1024, "{n}");
        assert!(r < 256 * 1024, "principal não pode somar a worktree aninhada: {r}");
    }
}
