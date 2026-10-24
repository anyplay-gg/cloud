//! The machine, and what the processes and store pools use while a scenario runs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::target::Target;

#[derive(Clone, Debug, Serialize)]
pub struct Machine {
    pub cpu: String,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    pub memory_gb: f64,
    pub os: String,
}

pub fn machine() -> Machine {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    Machine {
        cpu: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_owned())
            .unwrap_or_default(),
        logical_cores: sys.cpus().len(),
        physical_cores: System::physical_core_count(),
        memory_gb: (sys.total_memory() as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0,
        os: System::long_os_version().unwrap_or_else(|| "unknown".into()),
    }
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    /// Accumulated CPU time of the process in milliseconds.
    cpu_ms: u64,
    rss: u64,
}

struct Watched {
    label: String,
    pid: Pid,
    samples: Vec<Sample>,
}

#[derive(Default)]
struct PoolSamples {
    busy: Vec<(Instant, u32)>,
    size: u32,
}

/// Reads memory and CPU of the harness and of external cloud processes once a second, and the
/// busy store connections of in-process nodes four times a second.
pub struct Sampler {
    watched: Mutex<Vec<Watched>>,
    pools: Mutex<PoolSamples>,
    system: Mutex<System>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessUse {
    pub process: String,
    /// CPU time over wall time: 1.0 is one core fully used.
    pub cpu_avg_cores: f64,
    pub cpu_peak_cores: f64,
    pub rss_peak_mb: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolUse {
    pub pool_size_max: u32,
    pub busy_avg: f64,
    pub busy_peak: u32,
    /// The share of samples in which every connection of some node was busy.
    pub saturated_share: f64,
}

impl Sampler {
    pub fn start(opts_pids: &[u32], in_process: bool) -> Arc<Self> {
        let mut watched = vec![Watched {
            label: if in_process {
                "harness + in-process cloud".into()
            } else {
                "harness".into()
            },
            pid: Pid::from_u32(std::process::id()),
            samples: Vec::new(),
        }];
        for pid in opts_pids {
            watched.push(Watched {
                label: format!("cloud pid {pid}"),
                pid: Pid::from_u32(*pid),
                samples: Vec::new(),
            });
        }
        Arc::new(Self {
            watched: Mutex::new(watched),
            pools: Mutex::default(),
            system: Mutex::new(System::new()),
        })
    }

    /// Takes one reading of every watched process.
    pub fn read(&self) {
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        let mut system = self.system.lock().unwrap_or_else(|e| e.into_inner());
        let pids: Vec<Pid> = watched.iter().map(|w| w.pid).collect();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let at = Instant::now();
        for w in watched.iter_mut() {
            if let Some(process) = system.process(w.pid) {
                w.samples.push(Sample {
                    at,
                    cpu_ms: process.accumulated_cpu_time(),
                    rss: process.memory(),
                });
            }
        }
    }

    /// Runs the sampling until the process ends.
    pub fn run(self: &Arc<Self>, target: Arc<Target>) {
        let sampler = self.clone();
        tokio::spawn(async move {
            let mut tick = 0u32;
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                {
                    let mut pools = sampler.pools.lock().unwrap_or_else(|e| e.into_inner());
                    let at = Instant::now();
                    for node in &target.nodes {
                        if let Some((busy, size)) = node.pool_busy() {
                            pools.size = pools.size.max(size);
                            pools.busy.push((at, busy));
                        }
                    }
                }
                tick += 1;
                if tick.is_multiple_of(4) {
                    let sampler = sampler.clone();
                    let _ = tokio::task::spawn_blocking(move || sampler.read()).await;
                }
            }
        });
    }

    /// Memory and CPU of each process between two instants.
    pub fn usage(&self, from: Instant, to: Instant) -> Vec<ProcessUse> {
        let watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        watched
            .iter()
            .filter_map(|w| {
                let inside: Vec<&Sample> = w
                    .samples
                    .iter()
                    .filter(|s| s.at >= from && s.at <= to)
                    .collect();
                let (first, last) = (inside.first()?, inside.last()?);
                let wall = last.at.duration_since(first.at).as_secs_f64();
                let cores = |a: &Sample, b: &Sample| {
                    let wall = b.at.duration_since(a.at).as_secs_f64();
                    if wall <= 0.0 {
                        0.0
                    } else {
                        (b.cpu_ms - a.cpu_ms.min(b.cpu_ms)) as f64 / 1000.0 / wall
                    }
                };
                let peak = inside
                    .windows(2)
                    .map(|p| cores(p[0], p[1]))
                    .fold(0.0, f64::max);
                Some(ProcessUse {
                    process: w.label.clone(),
                    cpu_avg_cores: if wall > 0.0 { cores(first, last) } else { 0.0 },
                    cpu_peak_cores: peak,
                    rss_peak_mb: inside.iter().map(|s| s.rss).max().unwrap_or(0) as f64
                        / (1 << 20) as f64,
                })
            })
            .collect()
    }

    pub fn pool_use(&self, from: Instant, to: Instant) -> Option<PoolUse> {
        let pools = self.pools.lock().unwrap_or_else(|e| e.into_inner());
        let inside: Vec<u32> = pools
            .busy
            .iter()
            .filter(|(at, _)| *at >= from && *at <= to)
            .map(|(_, busy)| *busy)
            .collect();
        if inside.is_empty() || pools.size == 0 {
            return None;
        }
        let cap = pools.size;
        Some(PoolUse {
            pool_size_max: cap,
            busy_avg: inside.iter().map(|b| *b as f64).sum::<f64>() / inside.len() as f64,
            busy_peak: *inside.iter().max().unwrap_or(&0),
            saturated_share: inside.iter().filter(|b| **b >= cap).count() as f64
                / inside.len() as f64,
        })
    }
}
