//! Métricas da máquina. Roda numa thread própria (a leitura de /proc é síncrona)
//! e publica um snapshot por segundo.

use std::{
    collections::HashMap,
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use sysinfo::{Disks, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::sync::watch;

use super::Snapshot;
use crate::{
    config::SystemConfig,
    danger::{Danger, Evaluator, Sample},
};

#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    /// Uso total de CPU, 0–100.
    pub cpu: f32,
    pub load: (f64, f64, f64),
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub psi: Option<Psi>,
    pub net: Option<NetRates>,
    pub disks: Vec<DiskInfo>,
    pub top_cpu: Vec<ProcGroup>,
    pub top_mem: Vec<ProcGroup>,
    pub proc_count: usize,
    pub uptime: u64,
    pub battery: Option<Battery>,
    pub danger: Danger,
    /// Divisão do uso de CPU desde a última leitura (de /proc/stat).
    pub cpu_parts: Option<CpuParts>,
    pub cores: usize,
    /// Threads do sistema inteiro (de /proc/loadavg).
    pub threads: Option<u64>,
    /// Temperatura da CPU em °C, quando há sensor (no WSL não há).
    pub temp_c: Option<f64>,
    pub mem_parts: Option<MemParts>,
    /// Leitura e escrita em disco agora (discos inteiros de /proc/diskstats).
    pub disk_io: Option<DiskIo>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiskIo {
    /// Bytes por segundo.
    pub read: f64,
    pub write: f64,
}

/// Percentuais do tempo de CPU por tipo (somam ~100).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuParts {
    pub user: f64,
    pub nice: f64,
    /// Kernel, incluindo interrupções.
    pub system: f64,
    /// Esperando disco/rede.
    pub iowait: f64,
    /// Roubado pelo hipervisor (VM, WSL).
    pub steal: f64,
    pub idle: f64,
}

/// Divisão da RAM, em bytes (de /proc/meminfo).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemParts {
    pub total: u64,
    /// Memória anônima dos programas (heap, pilha).
    pub apps: u64,
    /// Do kernel e não recuperável (slab fixo, pilhas, tabelas de página).
    pub kernel: u64,
    /// Compartilhada (tmpfs, memória compartilhada entre processos).
    pub shared: u64,
    /// Cache de arquivos e buffers, que o kernel devolve quando falta.
    pub cache: u64,
    /// Guardada comprimida no zswap.
    pub compressed: u64,
    pub free: u64,
    pub available: u64,
}

impl SystemSnapshot {
    pub fn mem_pct(&self) -> f64 {
        pct(self.mem_used, self.mem_total)
    }

    pub fn swap_pct(&self) -> Option<f64> {
        (self.swap_total > 0).then(|| pct(self.swap_used, self.swap_total))
    }
}

/// PSI "some avg10": % do tempo, nos últimos 10 s, em que alguma tarefa esperou pelo recurso.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Psi {
    pub cpu: f64,
    pub mem: f64,
    pub io: f64,
}

#[derive(Clone, Debug, Default)]
pub struct NetRates {
    pub iface: String,
    /// Bytes por segundo.
    pub rx: f64,
    pub tx: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiskInfo {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

impl DiskInfo {
    pub fn pct(&self) -> f64 {
        pct(self.used, self.total)
    }
}

/// Processos agrupados pelo nome (vários `node` viram uma linha).
#[derive(Clone, Debug, PartialEq)]
pub struct ProcGroup {
    pub name: String,
    pub count: usize,
    /// Soma do uso de CPU, em % de um núcleo (como o htop: pode passar de 100).
    pub cpu: f32,
    pub mem: u64,
    /// O processo mais pesado do grupo: é ele que o `K` encerra.
    pub top_pid: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub pct: u8,
    pub status: String,
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 }
}

pub fn spawn(cfg: SystemConfig, tx: watch::Sender<Snapshot<SystemSnapshot>>) {
    thread::Builder::new()
        .name("cockpit-sys".into())
        .spawn(move || run(cfg, tx))
        .expect("thread de sistema");
}

fn run(cfg: SystemConfig, tx: watch::Sender<Snapshot<SystemSnapshot>>) {
    let mut sys = System::new();
    let mut eval = Evaluator::new(cfg.danger.clone());
    let mut net = NetCounter::default();
    let mut last_stat: Option<[u64; 8]> = None;
    let mut last_io: Option<(Instant, u64, u64)> = None;
    let mut snap = SystemSnapshot::default();
    // Sem `without_tasks` o sysinfo lista cada thread como processo (libuv-worker ×16…).
    // O exe é lido uma vez por processo: dá nome melhor que o `comm` ("MainThread", 15 chars).
    let proc_kind =
        ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet).without_tasks();

    // A primeira leitura de CPU precisa de uma anterior para ter delta.
    sys.refresh_cpu_usage();
    thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);

    let mut tick: u64 = 0;
    loop {
        let started = Instant::now();

        sys.refresh_cpu_usage();
        sys.refresh_memory();
        snap.cpu = sys.global_cpu_usage();
        let l = System::load_average();
        snap.load = (l.one, l.five, l.fifteen);
        snap.mem_total = sys.total_memory();
        snap.mem_used = sys.total_memory().saturating_sub(sys.available_memory());
        snap.swap_total = sys.total_swap();
        snap.swap_used = sys.used_swap();
        snap.uptime = System::uptime();
        snap.psi = read_psi();
        snap.cores = snap_cores(&sys);
        if let Some(now) = fs::read_to_string("/proc/stat").ok().and_then(|s| parse_stat_cpu(&s)) {
            if let Some(prev) = last_stat {
                snap.cpu_parts = cpu_parts(&prev, &now);
            }
            last_stat = Some(now);
        }
        snap.threads = fs::read_to_string("/proc/loadavg").ok().and_then(|s| parse_loadavg_threads(&s));
        snap.mem_parts = fs::read_to_string("/proc/meminfo").ok().and_then(|s| parse_meminfo(&s));
        if let Some((r, w)) = fs::read_to_string("/proc/diskstats").ok().map(|s| parse_diskstats(&s, is_whole_disk)) {
            let now = Instant::now();
            if let Some((t, pr, pw)) = last_io {
                let dt = now.duration_since(t).as_secs_f64().max(0.001);
                snap.disk_io = Some(DiskIo { read: r.saturating_sub(pr) as f64 / dt, write: w.saturating_sub(pw) as f64 / dt });
            }
            last_io = Some((now, r, w));
        }
        snap.net = net.sample();

        // Processos no mesmo ritmo da CPU global: o sysinfo calcula o % de cada processo
        // contra o intervalo da última leitura global, e ritmos diferentes inflam o valor.
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind);
        let procs = || sys.processes().values().filter(|p| p.thread_kind().is_none());
        snap.proc_count = procs().count();
        let (by_cpu, by_mem) =
            top_groups(procs().map(|p| (display_name(p), p.pid().as_u32(), p.cpu_usage(), p.memory())), cfg.top_n);
        snap.top_cpu = by_cpu;
        snap.top_mem = by_mem;

        if tick % 30 == 0 {
            // Relista a cada vez: pega montagens novas (pendrive, /mnt/d) sem custo relevante.
            let disks = Disks::new_with_refreshed_list();
            snap.disks = pick_disks(
                disks.iter().map(|d| (d.mount_point().to_string_lossy().to_string(), d.total_space(), d.available_space())),
                &cfg.disks,
            );
            snap.battery = read_battery(Path::new("/sys/class/power_supply"));
            snap.temp_c = read_cpu_temp();
        }

        snap.danger = eval.eval(&Sample {
            cpu: snap.cpu as f64,
            mem: snap.mem_pct(),
            swap: snap.swap_pct(),
            psi_mem: snap.psi.map(|p| p.mem),
            disks: snap.disks.iter().map(|d| (d.mount.clone(), d.pct())).collect(),
        });

        if tx.send(Snapshot::ok(snap.clone())).is_err() {
            return; // a UI fechou
        }
        tick += 1;
        thread::sleep(Duration::from_secs(1).saturating_sub(started.elapsed()));
    }
}

fn display_name(p: &Process) -> std::borrow::Cow<'_, str> {
    let exe = p.exe().and_then(|e| e.file_name()).map(|n| n.to_string_lossy());
    pick_name(p.name().to_string_lossy(), exe)
}

/// O `comm` do kernel é o melhor nome na maioria dos casos (o exe do Claude, por exemplo,
/// se chama `2.1.283`). Troca pelo nome do executável só quando o `comm` foi truncado
/// em 15 caracteres ou é genérico, como o `MainThread` do Electron.
fn pick_name<'a>(comm: std::borrow::Cow<'a, str>, exe: Option<std::borrow::Cow<'a, str>>) -> std::borrow::Cow<'a, str> {
    const GENERIC: &[&str] = &["MainThread", "Web Content", "exe"];
    match exe {
        Some(e) if comm.len() == 15 && e.starts_with(comm.as_ref()) => e,
        Some(e) if GENERIC.contains(&comm.as_ref()) && !e.is_empty() => e,
        _ => comm,
    }
}

/// Agrupa por nome e devolve os `n` maiores por CPU e por memória.
fn top_groups<'a>(
    procs: impl Iterator<Item = (std::borrow::Cow<'a, str>, u32, f32, u64)>,
    n: usize,
) -> (Vec<ProcGroup>, Vec<ProcGroup>) {
    let mut groups: HashMap<String, (ProcGroup, f32)> = HashMap::new();
    for (name, pid, cpu, mem) in procs {
        let weight = cpu + mem as f32 / 1e9; // desempate: o mais pesado em CPU, depois em RAM
        let e = groups.entry(name.to_string()).or_insert_with(|| {
            (ProcGroup { name: name.to_string(), count: 0, cpu: 0.0, mem: 0, top_pid: pid }, f32::MIN)
        });
        e.0.count += 1;
        e.0.cpu += cpu;
        e.0.mem += mem;
        if weight > e.1 {
            e.1 = weight;
            e.0.top_pid = pid;
        }
    }
    let mut all: Vec<ProcGroup> = groups.into_values().map(|(g, _)| g).collect();
    all.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then_with(|| a.name.cmp(&b.name)));
    let by_cpu = all.iter().take(n).cloned().collect();
    all.sort_by(|a, b| b.mem.cmp(&a.mem).then_with(|| a.name.cmp(&b.name)));
    let by_mem = all.into_iter().take(n).collect();
    (by_cpu, by_mem)
}

/// Escolhe os discos a mostrar: a lista do config ou, vazia, os que interessam a um dev.
fn pick_disks(all: impl Iterator<Item = (String, u64, u64)>, wanted: &[String]) -> Vec<DiskInfo> {
    let auto = |m: &str| {
        m == "/"
            || m == "/home"
            || (m.len() == 6 && m.starts_with("/mnt/") && m.as_bytes()[5].is_ascii_lowercase())
            || m.starts_with("/media/")
            || m.starts_with("/run/media/")
    };
    let mut out: Vec<DiskInfo> = Vec::new();
    for (mount, total, avail) in all {
        let keep = if wanted.is_empty() { auto(&mount) } else { wanted.contains(&mount) };
        if keep && total > 0 && !out.iter().any(|d| d.mount == mount) {
            out.push(DiskInfo { mount, used: total.saturating_sub(avail), total });
        }
    }
    if !wanted.is_empty() {
        out.sort_by_key(|d| wanted.iter().position(|w| *w == d.mount));
    } else {
        out.sort_by(|a, b| a.mount.cmp(&b.mount));
    }
    out
}

fn snap_cores(sys: &System) -> usize {
    sys.cpus().len().max(1)
}

/// Primeira linha de /proc/stat: user nice system idle iowait irq softirq steal (em ticks).
fn parse_stat_cpu(s: &str) -> Option<[u64; 8]> {
    let line = s.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line.split_whitespace().take(8).filter_map(|x| x.parse().ok()).collect();
    (v.len() >= 5).then(|| {
        let mut a = [0u64; 8];
        a[..v.len()].copy_from_slice(&v);
        a
    })
}

fn cpu_parts(prev: &[u64; 8], now: &[u64; 8]) -> Option<CpuParts> {
    let d: Vec<f64> = now.iter().zip(prev).map(|(n, p)| n.saturating_sub(*p) as f64).collect();
    let total: f64 = d.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let pct = |x: f64| x * 100.0 / total;
    Some(CpuParts {
        user: pct(d[0]),
        nice: pct(d[1]),
        system: pct(d[2] + d[5] + d[6]),
        idle: pct(d[3]),
        iowait: pct(d[4]),
        steal: pct(d[7]),
    })
}

/// Disco inteiro (não partição, loop, ram ou device-mapper): só esses entram na soma
/// de E/S, senão a mesma escrita contaria duas vezes.
fn is_whole_disk(name: &str) -> bool {
    ["sd", "nvme", "vd", "xvd", "mmcblk", "hd"].iter().any(|p| name.starts_with(p)) && Path::new("/sys/block").join(name).exists()
}

/// Bytes lidos e escritos desde o boot, somando os discos aceitos por `keep`.
/// Campos: maj min nome lidas_ok lidas_merge setores_lidos ms escritas_ok merge setores_escritos …
fn parse_diskstats(s: &str, keep: impl Fn(&str) -> bool) -> (u64, u64) {
    let (mut r, mut w) = (0u64, 0u64);
    for line in s.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !keep(f[2]) {
            continue;
        }
        r += f[5].parse::<u64>().unwrap_or(0) * 512;
        w += f[9].parse::<u64>().unwrap_or(0) * 512;
    }
    (r, w)
}

/// "1.71 0.73 0.55 3/744 834793" → 744 (entidades agendáveis = threads).
fn parse_loadavg_threads(s: &str) -> Option<u64> {
    s.split_whitespace().nth(3)?.split_once('/')?.1.parse().ok()
}

fn parse_meminfo(s: &str) -> Option<MemParts> {
    let mut m = std::collections::HashMap::new();
    for line in s.lines() {
        let (k, rest) = line.split_once(':')?;
        let kb: u64 = rest.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0);
        m.insert(k.trim(), kb * 1024);
    }
    let g = |k: &str| m.get(k).copied().unwrap_or(0);
    let total = g("MemTotal");
    if total == 0 {
        return None;
    }
    let shared = g("Shmem");
    Some(MemParts {
        total,
        apps: g("AnonPages"),
        kernel: g("SUnreclaim") + g("KernelStack") + g("PageTables"),
        shared,
        cache: (g("Buffers") + g("Cached") + g("SReclaimable")).saturating_sub(shared),
        compressed: g("Zswap"),
        free: g("MemFree"),
        available: g("MemAvailable"),
    })
}

/// Sensor da CPU: hwmon (coretemp/k10temp/zenpower/cpu_thermal) ou zona térmica do pacote.
fn read_cpu_temp() -> Option<f64> {
    let read_c = |p: &Path| fs::read_to_string(p).ok()?.trim().parse::<f64>().ok().map(|m| m / 1000.0);
    for e in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let name = fs::read_to_string(e.path().join("name")).unwrap_or_default();
        if ["coretemp", "k10temp", "zenpower", "cpu_thermal", "acpitz"].contains(&name.trim())
            && let Some(c) = read_c(&e.path().join("temp1_input"))
        {
            return Some(c);
        }
    }
    for e in fs::read_dir("/sys/class/thermal").ok()?.flatten() {
        let ty = fs::read_to_string(e.path().join("type")).unwrap_or_default();
        if (ty.contains("x86_pkg_temp") || ty.contains("cpu")) && let Some(c) = read_c(&e.path().join("temp")) {
            return Some(c);
        }
    }
    None
}

fn read_psi() -> Option<Psi> {
    let read = |r: &str| fs::read_to_string(format!("/proc/pressure/{r}")).ok().and_then(|s| parse_psi_avg10(&s));
    Some(Psi { cpu: read("cpu")?, mem: read("memory")?, io: read("io").unwrap_or(0.0) })
}

fn parse_psi_avg10(s: &str) -> Option<f64> {
    let line = s.lines().find(|l| l.starts_with("some"))?;
    line.split_whitespace().find_map(|kv| kv.strip_prefix("avg10=")).and_then(|v| v.parse().ok())
}

fn read_battery(dir: &Path) -> Option<Battery> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let read = |f: &str| fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
        if read("type").as_deref() == Some("Battery") {
            let pct = read("capacity")?.parse().ok()?;
            return Some(Battery { pct, status: read("status").unwrap_or_default() });
        }
    }
    None
}

/// Taxa de rede pela diferença dos contadores de /proc/net/dev na interface da rota padrão.
#[derive(Default)]
struct NetCounter {
    last: Option<(Instant, String, u64, u64)>,
}

impl NetCounter {
    fn sample(&mut self) -> Option<NetRates> {
        let iface = fs::read_to_string("/proc/net/route").ok().and_then(|s| default_iface(&s));
        let dev = fs::read_to_string("/proc/net/dev").ok()?;
        let counters = parse_net_dev(&dev);
        let (name, rx, tx) = match iface {
            Some(i) => counters.into_iter().find(|c| c.0 == i)?,
            None => {
                // sem rota padrão: soma das interfaces físicas
                let (rx, tx) = counters.iter().filter(|c| !is_virtual(&c.0)).fold((0, 0), |a, c| (a.0 + c.1, a.1 + c.2));
                ("total".to_string(), rx, tx)
            }
        };
        let now = Instant::now();
        let rates = match &self.last {
            Some((t, n, prx, ptx)) if *n == name => {
                let dt = now.duration_since(*t).as_secs_f64().max(0.001);
                Some((rx.saturating_sub(*prx) as f64 / dt, tx.saturating_sub(*ptx) as f64 / dt))
            }
            _ => None,
        };
        self.last = Some((now, name.clone(), rx, tx));
        let (rrx, rtx) = rates?;
        Some(NetRates {
            iface: name,
            rx: rrx,
            tx: rtx,
        })
    }
}

fn is_virtual(name: &str) -> bool {
    name == "lo" || ["veth", "docker", "br-", "virbr", "vnet", "tun", "tap"].iter().any(|p| name.starts_with(p))
}

fn default_iface(route: &str) -> Option<String> {
    route.lines().skip(1).find_map(|l| {
        let mut f = l.split_whitespace();
        let iface = f.next()?;
        (f.next()? == "00000000").then(|| iface.to_string())
    })
}

/// `(interface, bytes recebidos, bytes enviados)`.
fn parse_net_dev(s: &str) -> Vec<(String, u64, u64)> {
    s.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            Some((name.trim().to_string(), *f.first()?, *f.get(8)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn parses_psi() {
        let s = "some avg10=7.37 avg60=2.77 avg300=0.91 total=12065007\nfull avg10=1.00 avg60=0 avg300=0 total=1\n";
        assert_eq!(parse_psi_avg10(s), Some(7.37));
        assert_eq!(parse_psi_avg10("lixo"), None);
    }

    #[test]
    fn parses_cpu_breakdown() {
        let a = parse_stat_cpu("cpu  100 0 50 800 10 0 20 0 0 0\ncpu0 1 2 3\n").unwrap();
        let b = parse_stat_cpu("cpu  160 10 80 900 20 0 30 0 0 0\n").unwrap();
        let p = cpu_parts(&a, &b).unwrap();
        // deltas: user 60, nice 10, system 30+irq 0+softirq 10, idle 100, iowait 10 → 220
        assert!((p.user - 60.0 * 100.0 / 220.0).abs() < 1e-9);
        assert!((p.system - 40.0 * 100.0 / 220.0).abs() < 1e-9);
        assert!((p.user + p.nice + p.system + p.idle + p.iowait + p.steal - 100.0).abs() < 1e-9);
        assert!(cpu_parts(&a, &a).is_none());
        assert_eq!(parse_loadavg_threads("1.71 0.73 0.55 3/744 834793"), Some(744));
    }

    #[test]
    fn parses_diskstats_whole_disks_only() {
        let s = "   8       0 sda 100 0 2000 10 50 0 4000 20 0 0 0\n   8       1 sda1 90 0 1800 9 40 0 3000 10 0 0 0\n   7       0 loop0 5 0 80 0 0 0 0 0 0 0 0\n   8      32 sdc 10 0 200 1 5 0 100 0 0 0 0\n";
        let (r, w) = parse_diskstats(s, |n| n == "sda" || n == "sdc");
        assert_eq!((r, w), ((2000 + 200) * 512, (4000 + 100) * 512));
    }

    #[test]
    fn parses_memory_breakdown() {
        let s = "MemTotal: 12249204 kB\nMemFree: 232816 kB\nMemAvailable: 5853332 kB\nBuffers: 547200 kB\nCached: 4458232 kB\nAnonPages: 5535596 kB\nShmem: 108188 kB\nSReclaimable: 939896 kB\nSUnreclaim: 148844 kB\nKernelStack: 12096 kB\nPageTables: 49732 kB\nHugePages_Total: 0\n";
        let m = parse_meminfo(s).unwrap();
        let kb = |x: u64| x * 1024;
        assert_eq!(m.total, kb(12249204));
        assert_eq!(m.apps, kb(5535596));
        assert_eq!(m.kernel, kb(148844 + 12096 + 49732));
        assert_eq!(m.cache, kb(547200 + 4458232 + 939896 - 108188));
        assert_eq!(m.compressed, 0);
    }

    #[test]
    fn parses_net_dev_and_route() {
        let dev = "Inter-|   Receive |  Transmit\n face |bytes    packets\n    lo: 634 88 0 0 0 0 0 0 634 88 0 0 0 0 0 0\n  eth0: 45846993   55689    0    0    0     0          0       259  4407531   23408    0    0    0     0       0          0\n";
        assert_eq!(parse_net_dev(dev), vec![("lo".into(), 634, 634), ("eth0".into(), 45846993, 4407531)]);
        let route = "Iface\tDestination\tGateway\neth0\t00000000\t0100A8C0\neth0\t0000A8C0\t00000000\n";
        assert_eq!(default_iface(route), Some("eth0".into()));
        assert!(is_virtual("docker0") && is_virtual("lo") && !is_virtual("wlan0"));
    }

    #[test]
    fn groups_processes_by_name() {
        let procs = vec![
            (Cow::from("node"), 10, 40.0, 100),
            (Cow::from("node"), 11, 60.0, 300),
            (Cow::from("code"), 20, 5.0, 2_000),
            (Cow::from("zsh"), 30, 0.0, 10),
        ];
        let (cpu, mem) = top_groups(procs.into_iter(), 2);
        assert_eq!(cpu[0], ProcGroup { name: "node".into(), count: 2, cpu: 100.0, mem: 400, top_pid: 11 });
        assert_eq!(cpu.len(), 2);
        assert_eq!(mem[0].name, "code");
        assert_eq!(mem[1].name, "node");
    }

    #[test]
    fn process_names() {
        let n = |c: &'static str, e: Option<&'static str>| pick_name(Cow::from(c), e.map(Cow::from)).to_string();
        assert_eq!(n("claude", Some("2.1.283")), "claude");
        assert_eq!(n("zed-remote-serv", Some("zed-remote-server-stable-0.200")), "zed-remote-server-stable-0.200");
        assert_eq!(n("MainThread", Some("code")), "code");
        assert_eq!(n("MainThread", None), "MainThread");
        assert_eq!(n("node", Some("node")), "node");
    }

    #[test]
    fn picks_dev_disks() {
        let all = vec![
            ("/".to_string(), 100, 40),
            ("/mnt/wslg/distro".to_string(), 100, 40),
            ("/mnt/c".to_string(), 500, 100),
            ("/usr/lib/wsl/drivers".to_string(), 500, 100),
            ("/var/lib/docker".to_string(), 100, 40),
        ];
        let got = pick_disks(all.clone().into_iter(), &[]);
        assert_eq!(got.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/", "/mnt/c"]);
        assert_eq!(got[1].used, 400);
        let got = pick_disks(all.into_iter(), &["/var/lib/docker".into(), "/".into()]);
        assert_eq!(got.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/var/lib/docker", "/"]);
    }

    #[test]
    fn reads_battery() {
        let dir = std::env::temp_dir().join(format!("cockpit-bat-{}", std::process::id()));
        fs::create_dir_all(dir.join("AC1")).unwrap();
        fs::create_dir_all(dir.join("BAT1")).unwrap();
        fs::write(dir.join("AC1/type"), "Mains\n").unwrap();
        fs::write(dir.join("BAT1/type"), "Battery\n").unwrap();
        fs::write(dir.join("BAT1/capacity"), "87\n").unwrap();
        fs::write(dir.join("BAT1/status"), "Discharging\n").unwrap();
        let b = read_battery(&dir);
        fs::remove_dir_all(&dir).ok();
        assert_eq!(b, Some(Battery { pct: 87, status: "Discharging".into() }));
    }
}
