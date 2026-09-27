//! Detector de travamento: transforma amostras de sistema em níveis Ok/Warn/Critical,
//! com histerese para o alerta não piscar a cada oscilação.

use std::collections::{BTreeMap, VecDeque};

use crate::config::DangerConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    #[default]
    Ok,
    Warn,
    Critical,
}

/// Uma leitura do sistema, já em %.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub cpu: f64,
    pub mem: f64,
    /// `None` quando não há swap.
    pub swap: Option<f64>,
    /// PSI memory "some" avg10; `None` sem suporte no kernel.
    pub psi_mem: Option<f64>,
    pub disks: Vec<(String, f64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Danger {
    pub cpu: Level,
    pub mem: Level,
    pub swap: Level,
    pub psi: Level,
    pub disks: BTreeMap<String, Level>,
    /// Frases curtas do que está em alerta, do mais grave ao menos grave.
    pub reasons: Vec<(Level, String)>,
}

impl Danger {
    pub fn worst(&self) -> Level {
        [self.cpu, self.mem, self.swap, self.psi, self.disk()].into_iter().max().unwrap_or_default()
    }

    pub fn disk(&self) -> Level {
        self.disks.values().copied().max().unwrap_or_default()
    }

    /// Pressão de memória (memória, swap ou PSI): os ofensores são os que mais usam RAM.
    pub fn memory_pressure(&self) -> Level {
        self.mem.max(self.swap).max(self.psi)
    }
}

#[derive(Default)]
struct Hyst(Level);

impl Hyst {
    /// Sobe na hora; só desce quando o valor cai `h` pontos abaixo do limiar.
    fn step(&mut self, v: f64, warn: f64, crit: f64, h: f64) -> Level {
        let classify = |v: f64, w: f64, c: f64| {
            if v > c {
                Level::Critical
            } else if v > w {
                Level::Warn
            } else {
                Level::Ok
            }
        };
        let up = classify(v, warn, crit);
        self.0 = if up >= self.0 { up } else { classify(v, warn - h, crit - h).min(self.0) };
        self.0
    }
}

pub struct Evaluator {
    cfg: DangerConfig,
    cpu_window: VecDeque<f64>,
    swap_hist: VecDeque<f64>,
    cpu: Hyst,
    mem: Hyst,
    swap: Hyst,
    psi: Hyst,
    disks: BTreeMap<String, Hyst>,
}

/// Janela para decidir se a swap está subindo.
const SWAP_TREND_SAMPLES: usize = 30;

impl Evaluator {
    pub fn new(cfg: DangerConfig) -> Self {
        Self {
            cfg,
            cpu_window: VecDeque::new(),
            swap_hist: VecDeque::new(),
            cpu: Hyst::default(),
            mem: Hyst::default(),
            swap: Hyst::default(),
            psi: Hyst::default(),
            disks: BTreeMap::new(),
        }
    }

    pub fn eval(&mut self, s: &Sample) -> Danger {
        let c = &self.cfg;
        let h = c.hysteresis;
        let mut reasons = Vec::new();

        // CPU: só conta a média da janela inteira ("sustentada por N s").
        let window = c.cpu_window_secs.max(1);
        self.cpu_window.push_back(s.cpu);
        while self.cpu_window.len() > window {
            self.cpu_window.pop_front();
        }
        let cpu_avg = if self.cpu_window.len() == window {
            self.cpu_window.iter().sum::<f64>() / window as f64
        } else {
            0.0
        };
        let cpu = self.cpu.step(cpu_avg, c.cpu_warn, c.cpu_crit, h);
        if cpu > Level::Ok {
            reasons.push((cpu, format!("CPU {cpu_avg:.0}% por {window}s")));
        }

        let mem = self.mem.step(s.mem, c.mem_warn, c.mem_crit, h);
        if mem > Level::Ok {
            reasons.push((mem, format!("memória {:.0}%", s.mem)));
        }

        // Swap: o amarelo só entra se ela estiver subindo; swap alta e parada é resto de pico antigo.
        let swap = match s.swap {
            Some(v) => {
                self.swap_hist.push_back(v);
                while self.swap_hist.len() > SWAP_TREND_SAMPLES {
                    self.swap_hist.pop_front();
                }
                let min = self.swap_hist.iter().copied().fold(f64::INFINITY, f64::min);
                let rising = v - min >= 1.0;
                let eff = if rising || self.swap.0 > Level::Ok || v > c.swap_crit { v } else { v.min(c.swap_warn) };
                let lvl = self.swap.step(eff, c.swap_warn, c.swap_crit, h);
                if lvl > Level::Ok {
                    reasons.push((lvl, format!("swap {v:.0}%{}", if rising { " ↑" } else { "" })));
                }
                lvl
            }
            None => Level::Ok,
        };

        let psi = match s.psi_mem {
            Some(v) => {
                let lvl = self.psi.step(v, c.psi_mem_warn, c.psi_mem_crit, h.min(c.psi_mem_warn / 2.0));
                if lvl > Level::Ok {
                    reasons.push((lvl, format!("pressão de memória {v:.0}%")));
                }
                lvl
            }
            None => Level::Ok,
        };

        let mut disks = BTreeMap::new();
        for (mount, pct) in &s.disks {
            let lvl = self.disks.entry(mount.clone()).or_default().step(*pct, c.disk_warn, c.disk_crit, h);
            if lvl > Level::Ok {
                reasons.push((lvl, format!("disco {mount} {pct:.0}%")));
            }
            disks.insert(mount.clone(), lvl);
        }

        reasons.sort_by(|a, b| b.0.cmp(&a.0));
        Danger { cpu, mem, swap, psi, disks, reasons }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_mem(e: &mut Evaluator, mem: f64) -> Level {
        e.eval(&Sample { mem, ..Default::default() }).mem
    }

    #[test]
    fn memory_levels_with_hysteresis() {
        let mut e = Evaluator::new(DangerConfig::default());
        assert_eq!(eval_mem(&mut e, 50.0), Level::Ok);
        assert_eq!(eval_mem(&mut e, 86.0), Level::Warn);
        assert_eq!(eval_mem(&mut e, 94.0), Level::Critical);
        // 91 < 93, mas ainda > 93-3: continua crítico
        assert_eq!(eval_mem(&mut e, 91.0), Level::Critical);
        assert_eq!(eval_mem(&mut e, 89.0), Level::Warn);
        // 84 < 85, mas > 82: continua amarelo
        assert_eq!(eval_mem(&mut e, 84.0), Level::Warn);
        assert_eq!(eval_mem(&mut e, 81.0), Level::Ok);
    }

    #[test]
    fn cpu_needs_a_full_window() {
        let mut e = Evaluator::new(DangerConfig { cpu_window_secs: 3, ..Default::default() });
        let cpu = |e: &mut Evaluator, v| e.eval(&Sample { cpu: v, ..Default::default() }).cpu;
        assert_eq!(cpu(&mut e, 100.0), Level::Ok);
        assert_eq!(cpu(&mut e, 100.0), Level::Ok);
        assert_eq!(cpu(&mut e, 100.0), Level::Critical);
        // um pico isolado não dispara
        let mut e = Evaluator::new(DangerConfig { cpu_window_secs: 3, ..Default::default() });
        for v in [10.0, 10.0, 100.0] {
            assert_eq!(cpu(&mut e, v), Level::Ok);
        }
    }

    #[test]
    fn swap_warns_only_when_rising() {
        let mut e = Evaluator::new(DangerConfig::default());
        let swap = |e: &mut Evaluator, v| e.eval(&Sample { swap: Some(v), ..Default::default() }).swap;
        // swap alta e parada: não avisa
        for _ in 0..5 {
            assert_eq!(swap(&mut e, 60.0), Level::Ok);
        }
        // subindo: avisa
        assert_eq!(swap(&mut e, 62.0), Level::Warn);
        // acima do crítico avisa sempre
        let mut e = Evaluator::new(DangerConfig::default());
        assert_eq!(swap(&mut e, 85.0), Level::Critical);
    }

    #[test]
    fn worst_and_reasons() {
        let mut e = Evaluator::new(DangerConfig::default());
        let d = e.eval(&Sample {
            mem: 95.0,
            psi_mem: Some(6.0),
            disks: vec![("/".into(), 91.0), ("/mnt/c".into(), 50.0)],
            ..Default::default()
        });
        assert_eq!(d.worst(), Level::Critical);
        assert_eq!(d.disk(), Level::Warn);
        assert_eq!(d.memory_pressure(), Level::Critical);
        assert_eq!(d.reasons[0], (Level::Critical, "memória 95%".into()));
        assert_eq!(d.reasons.len(), 3);
    }
}
