//! Especificações da máquina: SO, processador, núcleos, placa de vídeo e memória.
//! Coletadas uma vez na partida. No WSL, o Linux só enxerga a VM; a máquina de verdade
//! (Windows, núcleos físicos, RAM total, GPU) vem de uma única chamada ao PowerShell.

use std::{fs, time::Duration};

use serde::Deserialize;
use tokio::{process::Command, sync::watch};

use super::Snapshot;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Specs {
    /// "Ubuntu 24.04.2 LTS"
    pub os: String,
    pub kernel: String,
    pub wsl: bool,
    /// Windows por trás do WSL: "Windows 11 Pro".
    pub host_os: Option<String>,
    pub cpu: String,
    /// Núcleos físicos e threads da máquina (no WSL, do Windows).
    pub cores: Option<usize>,
    pub threads: usize,
    /// Threads que o Linux enxerga (no WSL pode ser menos que a máquina).
    pub visible_threads: usize,
    pub gpus: Vec<String>,
    /// RAM total da máquina (no WSL, do Windows).
    pub mem_total: Option<u64>,
    /// RAM que o Linux enxerga.
    pub visible_mem: u64,
}

pub fn spawn(tx: watch::Sender<Snapshot<Specs>>) {
    tokio::spawn(async move {
        let mut specs = tokio::task::spawn_blocking(local).await.unwrap_or_default();
        tx.send_replace(Snapshot::ok(specs.clone()));

        if specs.wsl {
            match windows_host().await {
                Some(h) => {
                    specs.host_os = h.os.map(|o| clean(o.trim_start_matches("Microsoft ")));
                    specs.cores = h.cores.or(specs.cores);
                    specs.threads = h.threads.unwrap_or(specs.threads);
                    specs.mem_total = h.ram.or(specs.mem_total);
                    if let Some(name) = h.cpu.filter(|c| !c.trim().is_empty()) {
                        specs.cpu = clean(&name);
                    }
                    if specs.gpus.is_empty() {
                        specs.gpus = h.gpus.into_iter().flatten().filter_map(|g| g.name).map(|n| clean(&n)).collect();
                    }
                }
                None => tracing::info!("sem dados do Windows (powershell.exe indisponível?)"),
            }
        }
        if specs.gpus.is_empty() {
            specs.gpus = linux_gpus().await;
        }
        tx.send_replace(Snapshot::ok(specs));
    });
}

/// O que o Linux sabe sozinho, sem rodar nada.
fn local() -> Specs {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os = os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .or_else(sysinfo::System::long_os_version)
        .unwrap_or_else(|| "Linux".into());
    let kernel = sysinfo::System::kernel_version().unwrap_or_default();
    let wsl = kernel.to_lowercase().contains("microsoft") || std::env::var_os("WSL_DISTRO_NAME").is_some();
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| clean(v)))
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let visible_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    Specs {
        os,
        kernel,
        wsl,
        host_os: None,
        cpu,
        cores: sysinfo::System::physical_core_count(),
        threads: visible_threads,
        visible_threads,
        gpus: Vec::new(),
        mem_total: None,
        visible_mem: sys.total_memory(),
    }
}

/// Tira as marcas registradas e espaços repetidos: "Intel(R) Iris(R) Xe Graphics" →
/// "Intel Iris Xe Graphics".
fn clean(s: &str) -> String {
    let s = s.replace("(R)", "").replace("(r)", "").replace("(TM)", "").replace("(tm)", "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize, Default)]
struct Host {
    os: Option<String>,
    cpu: Option<String>,
    cores: Option<usize>,
    threads: Option<usize>,
    ram: Option<u64>,
    gpus: Option<Vec<Gpu>>,
}

#[derive(Deserialize)]
struct Gpu {
    #[serde(rename = "Name")]
    name: Option<String>,
}

const PS: &str = "$ErrorActionPreference='SilentlyContinue';\
$o=Get-CimInstance Win32_OperatingSystem;$c=Get-CimInstance Win32_ComputerSystem;\
$p=@(Get-CimInstance Win32_Processor);$g=@(Get-CimInstance Win32_VideoController|Select-Object Name);\
[pscustomobject]@{os=$o.Caption;cpu=$p[0].Name;cores=($p|Measure-Object NumberOfCores -Sum).Sum;\
threads=($p|Measure-Object NumberOfLogicalProcessors -Sum).Sum;ram=[uint64]$c.TotalPhysicalMemory;gpus=$g}\
|ConvertTo-Json -Compress";

async fn windows_host() -> Option<Host> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", PS]).kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_host(&String::from_utf8_lossy(&out.stdout))
}

fn parse_host(s: &str) -> Option<Host> {
    // O PowerShell devolve um objeto quando há uma GPU só; normaliza para lista.
    let mut v: serde_json::Value = serde_json::from_str(s.trim()).ok()?;
    if let Some(g) = v.get_mut("gpus")
        && g.is_object()
    {
        *g = serde_json::Value::Array(vec![g.take()]);
    }
    serde_json::from_value(v).ok()
}

/// Linux nativo: `nvidia-smi`, senão `lspci`.
async fn linux_gpus() -> Vec<String> {
    let run = |prog: &'static str, args: &'static [&'static str]| async move {
        let out = tokio::time::timeout(Duration::from_secs(5), Command::new(prog).args(args).kill_on_drop(true).output()).await;
        match out {
            Ok(Ok(o)) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
            _ => None,
        }
    };
    if let Some(s) = run("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]).await {
        let v: Vec<String> = s.lines().map(clean).filter(|l| !l.is_empty()).collect();
        if !v.is_empty() {
            return v;
        }
    }
    run("lspci", &["-mm"]).await.map(|s| parse_lspci(&s)).unwrap_or_default()
}

/// `lspci -mm`: `00:02.0 "VGA compatible controller" "Intel Corporation" "Iris Xe" …`
fn parse_lspci(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| l.contains("\"VGA compatible controller\"") || l.contains("\"3D controller\"") || l.contains("\"Display controller\""))
        .filter_map(|l| {
            let q: Vec<&str> = l.split('"').collect();
            // [slot, classe, _, fabricante, _, dispositivo, …]
            let vendor = q.get(3)?.replace(" Corporation", "").replace(" Corp.", "");
            let dev = q.get(5)?;
            Some(clean(&format!("{vendor} {dev}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_trademarks() {
        assert_eq!(clean(" 11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz"), "11th Gen Intel Core i5-1135G7 @ 2.40GHz");
        assert_eq!(clean("Intel(R) Iris(R) Xe Graphics"), "Intel Iris Xe Graphics");
    }

    #[test]
    fn parses_windows_host() {
        let one = r#"{"os":"Microsoft Windows 11 Pro","cpu":"11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz","cores":4,"threads":8,"ram":16889192448,"gpus":{"Name":"Intel(R) Iris(R) Xe Graphics"}}"#;
        let h = parse_host(one).unwrap();
        assert_eq!((h.cores, h.threads, h.ram), (Some(4), Some(8), Some(16889192448)));
        assert_eq!(h.gpus.unwrap()[0].name.as_deref(), Some("Intel(R) Iris(R) Xe Graphics"));
        let two = r#"{"os":"x","gpus":[{"Name":"A"},{"Name":"B"}]}"#;
        assert_eq!(parse_host(two).unwrap().gpus.unwrap().len(), 2);
        assert!(parse_host("").is_none());
    }

    #[test]
    fn parses_lspci() {
        let s = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"TigerLake-LP GT2 [Iris Xe Graphics]\" -r01 \"Dell\" \"Device 0a1f\"\n\
                 01:00.0 \"3D controller\" \"NVIDIA Corporation\" \"GA107M [GeForce RTX 3050 Mobile]\" -ra1 \"Dell\" \"x\"\n\
                 00:1f.3 \"Audio device\" \"Intel Corporation\" \"Tiger Lake-LP Smart Sound\" -r20 \"Dell\" \"y\"\n";
        assert_eq!(parse_lspci(s), ["Intel TigerLake-LP GT2 [Iris Xe Graphics]", "NVIDIA GA107M [GeForce RTX 3050 Mobile]"]);
    }

    #[test]
    fn local_specs_are_sane() {
        let s = local();
        assert!(!s.os.is_empty() && !s.cpu.is_empty());
        assert!(s.visible_threads >= 1 && s.visible_mem > 0);
    }
}
