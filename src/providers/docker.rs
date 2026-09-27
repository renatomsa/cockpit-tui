//! Containers do Docker rodando (o equivalente ao `docker ps`, sem `-a`).
//!
//! A listagem fala direto com o socket do daemon (`GET /containers/json`), o que custa
//! ~1 ms, em vez de rodar a CLI a cada poucos segundos. As ações (parar um, derrubar
//! tudo) usam a CLI, como o usuário faria.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
    sync::watch,
};

use super::{Backoff, Snapshot};

const EVERY: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockerSnapshot {
    pub containers: Vec<Container>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// "Up 3 hours (healthy)"
    pub status: String,
    pub health: Health,
    /// Portas publicadas no host, sem repetição.
    pub ports: Vec<u16>,
    pub compose: Option<Compose>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    #[default]
    None,
    Starting,
    Healthy,
    Unhealthy,
}

/// Projeto do docker compose, lido das labels que o compose grava no container.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Compose {
    pub project: String,
    pub service: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub config_files: Vec<String>,
}

pub fn spawn(tx: watch::Sender<Snapshot<DockerSnapshot>>) {
    tokio::spawn(async move {
        let mut backoff = Backoff::new(Duration::from_secs(10), Duration::from_secs(120));
        loop {
            let delay = match list().await {
                Ok(c) => {
                    backoff.reset();
                    if tx.send(Snapshot::ok(DockerSnapshot { containers: c })).is_err() {
                        return;
                    }
                    EVERY
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    tx.send_modify(|s| {
                        s.error = Some(msg);
                        s.data = None;
                    });
                    backoff.next_delay()
                }
            };
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(delay).await;
        }
    });
}

fn socket_path() -> Option<PathBuf> {
    match std::env::var("DOCKER_HOST") {
        Ok(h) if h.starts_with("unix://") => Some(PathBuf::from(&h["unix://".len()..])),
        Ok(h) if !h.is_empty() => None, // tcp:// etc.: fica com a CLI
        _ => Some(PathBuf::from("/var/run/docker.sock")),
    }
}

async fn list() -> Result<Vec<Container>> {
    let mut out = match socket_path() {
        Some(p) if p.exists() => list_via_socket(&p).await?,
        _ => list_via_cli().await?,
    };
    out.sort_by(|a, b| a.compose.cmp(&b.compose).then(a.name.cmp(&b.name)));
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiContainer {
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    image: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    ports: Vec<ApiPort>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiPort {
    public_port: Option<u16>,
}

async fn list_via_socket(path: &std::path::Path) -> Result<Vec<Container>> {
    let fut = async {
        let mut s = UnixStream::connect(path).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => anyhow!("sem permissão no {} (seu usuário está no grupo docker?)", path.display()),
            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => anyhow!("o daemon do Docker não está rodando"),
            _ => anyhow!("{}: {e}", path.display()),
        })?;
        // HTTP/1.0: o daemon responde sem chunked e fecha a conexão no fim.
        s.write_all(b"GET /containers/json HTTP/1.0\r\nHost: docker\r\n\r\n").await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok::<_, anyhow::Error>(buf)
    };
    let buf = tokio::time::timeout(Duration::from_secs(5), fut).await.map_err(|_| anyhow!("o Docker não respondeu"))??;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| anyhow!("resposta inválida do Docker"))?;
    let status = head.lines().next().unwrap_or("");
    if !status.contains(" 200 ") {
        bail!("Docker respondeu {status}");
    }
    parse_api(body)
}

fn parse_api(body: &str) -> Result<Vec<Container>> {
    let raw: Vec<ApiContainer> = serde_json::from_str(body)?;
    Ok(raw
        .into_iter()
        .map(|c| {
            let mut ports: Vec<u16> = c.ports.iter().filter_map(|p| p.public_port).collect();
            ports.sort_unstable();
            ports.dedup();
            Container {
                id: c.id.chars().take(12).collect(),
                name: c.names.first().map(|n| n.trim_start_matches('/').to_string()).unwrap_or_default(),
                health: health(&c.status),
                image: c.image,
                status: c.status,
                ports,
                compose: compose_from(&c.labels),
            }
        })
        .collect())
}

/// Sem acesso ao socket (DOCKER_HOST remoto): `docker ps` com uma linha JSON por container.
async fn list_via_cli() -> Result<Vec<Container>> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("docker").args(["ps", "--no-trunc", "--format", "{{json .}}"]).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| anyhow!("docker ps passou de 10 s"))?
    .map_err(|_| anyhow!("docker não encontrado"))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(parse_cli(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_cli(s: &str) -> Vec<Container> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Row {
        #[serde(rename = "ID")]
        id: String,
        #[serde(default)]
        names: String,
        #[serde(default)]
        image: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        ports: String,
        #[serde(default)]
        labels: String,
    }
    s.lines()
        .filter_map(|l| serde_json::from_str::<Row>(l).ok())
        .map(|r| {
            let labels: BTreeMap<String, String> =
                r.labels.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            // "0.0.0.0:5432->5432/tcp, :::5432->5432/tcp"
            let mut ports: Vec<u16> = r
                .ports
                .split(", ")
                .filter_map(|p| p.split_once("->")?.0.rsplit(':').next()?.parse().ok())
                .collect();
            ports.sort_unstable();
            ports.dedup();
            Container {
                id: r.id.chars().take(12).collect(),
                name: r.names.split(',').next().unwrap_or("").to_string(),
                health: health(&r.status),
                image: r.image,
                status: r.status,
                ports,
                compose: compose_from(&labels),
            }
        })
        .collect()
}

fn health(status: &str) -> Health {
    if status.contains("(healthy)") {
        Health::Healthy
    } else if status.contains("(unhealthy)") {
        Health::Unhealthy
    } else if status.contains("health: starting") {
        Health::Starting
    } else {
        Health::None
    }
}

fn compose_from(labels: &BTreeMap<String, String>) -> Option<Compose> {
    let project = labels.get("com.docker.compose.project")?.clone();
    Some(Compose {
        project,
        service: labels.get("com.docker.compose.service").cloned(),
        working_dir: labels.get("com.docker.compose.project.working_dir").map(PathBuf::from),
        config_files: labels
            .get("com.docker.compose.project.config_files")
            .map(|f| f.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

/// Um comando da CLI do Docker, para rodar ou mostrar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerCmd {
    pub args: Vec<String>,
}

impl DockerCmd {
    pub fn display(&self) -> String {
        format!("docker {}", self.args.join(" "))
    }
}

/// Para um container (graceful: SIGTERM, e SIGKILL depois de 10 s).
pub fn stop_cmd(c: &Container) -> DockerCmd {
    DockerCmd { args: vec!["stop".into(), c.id.clone()] }
}

/// Derrubar tudo: `compose down --remove-orphans` em cada projeto (com o diretório e os
/// arquivos do projeto, lidos das labels, quando ainda existem) e `stop` nos soltos.
pub fn down_all_plan(cs: &[Container]) -> Vec<DockerCmd> {
    let mut projects: BTreeMap<&str, &Compose> = BTreeMap::new();
    let mut loose: Vec<String> = Vec::new();
    for c in cs {
        match &c.compose {
            Some(p) => {
                projects.entry(p.project.as_str()).or_insert(p);
            }
            None => loose.push(c.id.clone()),
        }
    }
    let mut out = Vec::new();
    for (name, p) in projects {
        let mut args = vec!["compose".to_string(), "-p".into(), name.to_string()];
        if let Some(dir) = p.working_dir.as_ref().filter(|d| d.is_dir()) {
            let files: Vec<PathBuf> = p.config_files.iter().map(|f| dir.join(f)).collect();
            if !files.is_empty() && files.iter().all(|f| f.is_file()) {
                args.push("--project-directory".into());
                args.push(dir.display().to_string());
                for f in files {
                    args.push("-f".into());
                    args.push(f.display().to_string());
                }
            }
        }
        args.extend(["down".into(), "--remove-orphans".into()]);
        out.push(DockerCmd { args });
    }
    if !loose.is_empty() {
        let mut args = vec!["stop".to_string()];
        args.extend(loose);
        out.push(DockerCmd { args });
    }
    out
}

/// Roda os comandos em sequência e resume o resultado numa frase.
pub async fn run(cmds: &[DockerCmd]) -> Result<()> {
    for c in cmds {
        tracing::info!("{}", c.display());
        let out = tokio::time::timeout(Duration::from_secs(120), Command::new("docker").args(&c.args).kill_on_drop(true).output())
            .await
            .map_err(|_| anyhow!("{} passou de 2 min", c.display()))?
            .map_err(|e| anyhow!("docker não executou: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("{} falhou: {}", c.display(), err.lines().last().unwrap_or("").trim());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const API: &str = r#"[
      {"Id":"0df82a44f020abcdef","Names":["/v360_redis"],"Image":"redis:latest","State":"running","Status":"Up 4 weeks",
       "Ports":[{"PrivatePort":6379,"PublicPort":6379,"Type":"tcp","IP":"0.0.0.0"},{"PrivatePort":6379,"PublicPort":6379,"Type":"tcp","IP":"::"}],
       "Labels":{"com.docker.compose.project":"v360","com.docker.compose.service":"redis","com.docker.compose.project.working_dir":"/nao/existe","com.docker.compose.project.config_files":"docker-compose-backstage.yml"}},
      {"Id":"aaaabbbbcccc1111","Names":["/solto"],"Image":"alpine","Status":"Up 2 minutes (unhealthy)","Ports":[],"Labels":{}}
    ]"#;

    #[test]
    fn parses_api_listing() {
        let c = parse_api(API).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].id.as_str(), c[0].name.as_str(), c[0].ports.as_slice()), ("0df82a44f020", "v360_redis", &[6379][..]));
        let p = c[0].compose.as_ref().unwrap();
        assert_eq!((p.project.as_str(), p.service.as_deref()), ("v360", Some("redis")));
        assert_eq!(p.config_files, ["docker-compose-backstage.yml"]);
        assert_eq!(c[1].health, Health::Unhealthy);
        assert!(c[1].compose.is_none());
    }

    #[test]
    fn parses_cli_listing() {
        let s = r#"{"ID":"5c874e963335","Image":"postgres:17","Names":"v360_db","Ports":"0.0.0.0:5432->5432/tcp, :::5432->5432/tcp","Status":"Up 3 hours (healthy)","Labels":"com.docker.compose.project=v360,com.docker.compose.service=db"}"#;
        let c = parse_cli(s);
        assert_eq!(c[0].ports, [5432]);
        assert_eq!(c[0].health, Health::Healthy);
        assert_eq!(c[0].compose.as_ref().unwrap().service.as_deref(), Some("db"));
    }

    #[test]
    fn down_all_uses_compose_files_when_they_exist() {
        let dir = std::env::temp_dir().join(format!("cockpit-docker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("compose.yml"), "services: {}").unwrap();
        let mk = |id: &str, project: Option<&str>, wd: &std::path::Path| Container {
            id: id.into(),
            compose: project.map(|p| Compose {
                project: p.into(),
                service: None,
                working_dir: Some(wd.to_path_buf()),
                config_files: vec!["compose.yml".into()],
            }),
            ..Default::default()
        };
        let cs = vec![
            mk("a1", Some("app"), &dir),
            mk("a2", Some("app"), &dir),
            mk("b1", Some("velho"), std::path::Path::new("/nao/existe")),
            mk("c1", None, &dir),
            mk("c2", None, &dir),
        ];
        let plan: Vec<String> = down_all_plan(&cs).iter().map(DockerCmd::display).collect();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            plan,
            [
                format!("docker compose -p app --project-directory {d} -f {d}/compose.yml down --remove-orphans", d = dir.display()),
                "docker compose -p velho down --remove-orphans".to_string(),
                "docker stop c1 c2".to_string(),
            ]
        );
        assert!(down_all_plan(&[]).is_empty());
    }
}
