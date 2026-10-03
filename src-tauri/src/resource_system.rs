//! OS counters for this application's process family. No foreign process data
//! is exposed: the full process list is used only to discover descendants.
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{
    CpuRefreshKind, DiskRefreshKind, Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System,
};

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResources {
    pub os: String,
    pub cpu_brand: String,
    pub logical_cpus: usize,
    pub physical_cores: Option<usize>,
    pub cpu_frequency_mhz: Option<u64>,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub total_swap_bytes: u64,
    pub used_swap_bytes: u64,
    pub cpu_percent: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppResources {
    pub pid: u32,
    pub process_count: usize,
    pub cpu_percent: Option<f64>,
    pub one_core_cpu_percent: Option<f64>,
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    pub memory_percent: Option<f64>,
    pub read_bytes_per_sec: Option<f64>,
    pub written_bytes_per_sec: Option<f64>,
    pub total_read_bytes: Option<u64>,
    pub total_written_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResources {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub role: String,
    pub started_at_seconds: u64,
    pub cpu_percent: Option<f64>,
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    pub read_bytes_per_sec: Option<f64>,
    pub written_bytes_per_sec: Option<f64>,
    pub total_read_bytes: Option<u64>,
    pub total_written_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskResources {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub kind: String,
    pub read_bytes_per_sec: Option<f64>,
    pub written_bytes_per_sec: Option<f64>,
    pub is_app_volume: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSnapshot {
    pub sampled_at_ms: u64,
    pub warming_up: bool,
    pub memory_virtual_label: &'static str,
    pub host: HostResources,
    pub app: AppResources,
    pub processes: Vec<ProcessResources>,
    pub disks: Vec<DiskResources>,
}

#[derive(Clone, Copy, Debug)]
struct ProcessIdentity {
    pid: u32,
    parent: Option<u32>,
    started: u64,
}

// Retain verified descendants after their intermediate parent exits, while
// rejecting recycled PIDs. Never include an unrelated WebView by name alone.
fn family(root: u32, list: &[ProcessIdentity], previous: &HashMap<u32, u64>) -> HashSet<u32> {
    let mut selected = HashSet::from([root]);
    for p in list {
        if previous.get(&p.pid) == Some(&p.started) {
            selected.insert(p.pid);
        }
    }
    let identities: HashMap<_, _> = list.iter().map(|p| (p.pid, p.started)).collect();
    loop {
        let before = selected.len();
        for p in list {
            if let Some(parent) = p.parent {
                if selected.contains(&parent)
                    && identities
                        .get(&parent)
                        .is_some_and(|start| p.started >= *start)
                {
                    selected.insert(p.pid);
                }
            }
        }
        if selected.len() == before {
            return selected;
        }
    }
}

fn rate(now: u64, old: u64, elapsed: Duration) -> Option<f64> {
    (elapsed >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
        .then(|| {
            now.checked_sub(old)
                .map(|delta| delta as f64 / elapsed.as_secs_f64())
        })
        .flatten()
}

fn percent(part: f64, whole: f64) -> Option<f64> {
    (part.is_finite() && part >= 0.0 && whole.is_finite() && whole > 0.0)
        .then(|| 100.0 * part / whole)
}

fn role(pid: u32, root: u32, name: &str) -> String {
    if pid == root {
        return "Backend · LogInsight".into();
    }
    let lower = name.to_lowercase();
    if lower.contains("webview")
        || lower.contains("webkit")
        || lower.contains("webprocess")
        || lower.contains("networkprocess")
    {
        "Interface · WebView".into()
    } else {
        "Processo auxiliar do aplicativo".into()
    }
}

#[derive(Clone, Copy)]
struct LastProcess {
    started: u64,
    cpu_ns: Option<u64>,
    read: Option<u64>,
    written: Option<u64>,
    sampled: Instant,
}

pub struct SystemSampler {
    system: System,
    disks: Disks,
    root: u32,
    app_path: PathBuf,
    previous: HashMap<u32, LastProcess>,
    previous_disks: HashMap<String, (u64, u64, Instant)>,
    last_sample: Option<Instant>,
    last_disk_list: Option<Instant>,
    physical_cores: Option<usize>,
    os: String,
}

impl SystemSampler {
    pub fn new(app_path: PathBuf) -> Self {
        Self {
            system: System::new(),
            disks: Disks::new(),
            root: std::process::id(),
            app_path,
            previous: HashMap::new(),
            previous_disks: HashMap::new(),
            last_sample: None,
            last_disk_list: None,
            physical_cores: System::physical_core_count(),
            os: format!(
                "{} {} ({})",
                System::name().unwrap_or_else(|| std::env::consts::OS.into()),
                System::os_version().unwrap_or_default(),
                System::cpu_arch()
            ),
        }
    }

    pub fn sample(&mut self) -> SystemSnapshot {
        let now = Instant::now();
        let ready = self
            .last_sample
            .is_some_and(|last| now.duration_since(last) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        self.system.refresh_memory();
        self.system
            .refresh_cpu_specifics(CpuRefreshKind::everything());
        // Discover identities cheaply; do not collect command lines, environment,
        // user information or the thousands of worker tasks of other programs.
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        let identities: Vec<_> = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| ProcessIdentity {
                pid: pid.as_u32(),
                parent: p.parent().map(|v| v.as_u32()),
                started: p.start_time(),
            })
            .collect();
        let previous_identities = self
            .previous
            .iter()
            .map(|(pid, p)| (*pid, p.started))
            .collect();
        let selected = family(self.root, &identities, &previous_identities);
        let pids: Vec<_> = selected.iter().map(|p| Pid::from_u32(*p)).collect();
        let cpus = self.system.cpus().len().max(1);
        let total_memory = self.system.total_memory();
        let mut next_previous = HashMap::new();
        let mut processes = Vec::new();
        let mut core_cpu = 0.0;
        let mut cpu_complete = ready;
        let mut io_complete = ready;
        let mut read_rate = 0.0;
        let mut written_rate = 0.0;
        for pid in pids {
            let Some(process) = self.system.process(pid) else {
                continue;
            };
            let started = process.start_time();
            let previous = self
                .previous
                .get(&pid.as_u32())
                .filter(|p| p.started == started);
            let counters = crate::resource_io::process_counters(pid.as_u32());
            let cpu = previous
                .and_then(|p| {
                    rate(
                        counters.cpu_time_ns?,
                        p.cpu_ns?,
                        now.duration_since(p.sampled),
                    )
                })
                .map(|ns_per_sec| ns_per_sec / 10_000_000.0);
            if let Some(cpu) = cpu {
                core_cpu += cpu;
            } else {
                cpu_complete = false;
            }
            let read = previous
                .and_then(|p| rate(counters.read_bytes?, p.read?, now.duration_since(p.sampled)));
            let written = previous.and_then(|p| {
                rate(
                    counters.written_bytes?,
                    p.written?,
                    now.duration_since(p.sampled),
                )
            });
            if let (Some(read), Some(written)) = (read, written) {
                read_rate += read;
                written_rate += written;
            } else {
                io_complete = false;
            }
            let name = process.name().to_string_lossy().into_owned();
            processes.push(ProcessResources {
                pid: pid.as_u32(),
                parent_pid: process.parent().map(|v| v.as_u32()),
                role: role(pid.as_u32(), self.root, &name),
                name,
                started_at_seconds: started,
                cpu_percent: cpu
                    .and_then(|v| percent(v, 100.0 * cpus as f64))
                    .map(|v| v.min(100.0)),
                resident_bytes: counters.resident_bytes,
                virtual_bytes: counters.virtual_bytes,
                read_bytes_per_sec: read,
                written_bytes_per_sec: written,
                total_read_bytes: counters.read_bytes,
                total_written_bytes: counters.written_bytes,
            });
            next_previous.insert(
                pid.as_u32(),
                LastProcess {
                    started,
                    cpu_ns: counters.cpu_time_ns,
                    read: counters.read_bytes,
                    written: counters.written_bytes,
                    sampled: now,
                },
            );
        }
        processes.sort_by(|a, b| {
            (b.pid == self.root)
                .cmp(&(a.pid == self.root))
                .then_with(|| b.resident_bytes.cmp(&a.resident_bytes))
                .then_with(|| a.pid.cmp(&b.pid))
        });
        let sum = |field: fn(&ProcessResources) -> Option<u64>| {
            if processes.is_empty() {
                return None;
            }
            processes
                .iter()
                .try_fold(0u64, |n, p| n.checked_add(field(p)?))
        };
        let resident = sum(|p| p.resident_bytes);
        let app = AppResources {
            pid: self.root,
            process_count: processes.len(),
            cpu_percent: (cpu_complete && !processes.is_empty())
                .then(|| percent(core_cpu, 100.0 * cpus as f64))
                .flatten()
                .map(|v| v.min(100.0)),
            one_core_cpu_percent: (cpu_complete && !processes.is_empty()).then_some(core_cpu),
            resident_bytes: resident,
            virtual_bytes: sum(|p| p.virtual_bytes),
            memory_percent: resident
                .and_then(|resident| percent(resident as f64, total_memory as f64)),
            read_bytes_per_sec: (io_complete && !processes.is_empty()).then_some(read_rate),
            written_bytes_per_sec: (io_complete && !processes.is_empty()).then_some(written_rate),
            total_read_bytes: sum(|p| p.total_read_bytes),
            total_written_bytes: sum(|p| p.total_written_bytes),
        };
        self.previous = next_previous;
        let frequency: Vec<_> = self
            .system
            .cpus()
            .iter()
            .map(|p| p.frequency())
            .filter(|v| *v > 0)
            .collect();
        let host = HostResources {
            os: self.os.clone(),
            cpu_brand: self
                .system
                .cpus()
                .first()
                .map(|v| v.brand().to_owned())
                .unwrap_or_default(),
            logical_cpus: cpus,
            physical_cores: self.physical_cores,
            cpu_frequency_mhz: (!frequency.is_empty())
                .then(|| frequency.iter().sum::<u64>() / frequency.len() as u64),
            total_memory_bytes: total_memory,
            available_memory_bytes: self.system.available_memory(),
            used_memory_bytes: self.system.used_memory(),
            total_swap_bytes: self.system.total_swap(),
            used_swap_bytes: self.system.used_swap(),
            cpu_percent: ready.then(|| self.system.global_cpu_usage() as f64),
        };
        let disks = self.sample_disks(now);
        self.last_sample = Some(now);
        SystemSnapshot {
            sampled_at_ms: unix_ms(),
            warming_up: !ready,
            memory_virtual_label: if cfg!(windows) {
                "Commit privado"
            } else {
                "Memória virtual"
            },
            host,
            app,
            processes,
            disks,
        }
    }

    fn sample_disks(&mut self, now: Instant) -> Vec<DiskResources> {
        if self
            .last_disk_list
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(30))
        {
            self.disks
                .refresh_specifics(true, DiskRefreshKind::everything());
            self.last_disk_list = Some(now);
        } else {
            for disk in self.disks.list_mut() {
                disk.refresh_specifics(DiskRefreshKind::everything());
            }
        }
        let app_path = self
            .app_path
            .canonicalize()
            .unwrap_or_else(|_| self.app_path.clone());
        let app_mount = self
            .disks
            .iter()
            .filter(|disk| path_on_volume(&app_path, disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
            .map(|disk| disk.mount_point().to_path_buf());
        let mut next_previous = HashMap::new();
        let disks = self
            .disks
            .iter()
            .map(|disk| {
                let key = format!(
                    "{}:{}",
                    disk.name().to_string_lossy(),
                    disk.mount_point().display()
                );
                let usage = disk.usage();
                let previous = self.previous_disks.get(&key);
                // Disk I/O support differs by OS and mount. No cumulative evidence
                // means unavailable, rather than a made-up throughput of zero.
                let readable = usage.total_read_bytes > 0 || usage.total_written_bytes > 0;
                let read = readable
                    .then(|| {
                        previous.and_then(|(r, _, at)| {
                            rate(usage.total_read_bytes, *r, now.duration_since(*at))
                        })
                    })
                    .flatten();
                let written = readable
                    .then(|| {
                        previous.and_then(|(_, w, at)| {
                            rate(usage.total_written_bytes, *w, now.duration_since(*at))
                        })
                    })
                    .flatten();
                next_previous.insert(
                    key,
                    (usage.total_read_bytes, usage.total_written_bytes, now),
                );
                DiskResources {
                    name: disk.name().to_string_lossy().into_owned(),
                    mount_point: disk.mount_point().to_string_lossy().into_owned(),
                    total_bytes: disk.total_space(),
                    available_bytes: disk.available_space(),
                    kind: disk.kind().to_string(),
                    read_bytes_per_sec: read,
                    written_bytes_per_sec: written,
                    is_app_volume: app_mount.as_deref() == Some(disk.mount_point()),
                }
            })
            .collect();
        self.previous_disks = next_previous;
        disks
    }
}

fn path_on_volume(path: &Path, mount: &Path) -> bool {
    // Windows canonical paths may start with the extended-length prefix.
    #[cfg(windows)]
    {
        let path = path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase();
        let mount = mount
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase();
        Path::new(&path).starts_with(Path::new(&mount))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(mount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_keeps_own_descendants_and_rejects_foreign_and_recycled_pids() {
        let list = [
            ProcessIdentity {
                pid: 10,
                parent: Some(1),
                started: 100,
            },
            ProcessIdentity {
                pid: 12,
                parent: Some(11),
                started: 102,
            },
            ProcessIdentity {
                pid: 11,
                parent: Some(10),
                started: 101,
            },
            ProcessIdentity {
                pid: 13,
                parent: Some(1),
                started: 103,
            },
            ProcessIdentity {
                pid: 14,
                parent: Some(1),
                started: 500,
            },
            ProcessIdentity {
                pid: 15,
                parent: Some(10),
                started: 50,
            },
            ProcessIdentity {
                pid: 99,
                parent: Some(1),
                started: 1,
            },
        ];
        assert_eq!(
            family(10, &list, &HashMap::from([(13, 103), (14, 104)])),
            HashSet::from([10, 11, 12, 13])
        );
    }

    #[test]
    fn rates_use_elapsed_time_and_never_underflow_on_counter_reset() {
        assert_eq!(rate(2048, 1024, Duration::from_secs(2)), Some(512.0));
        assert_eq!(rate(10, 1024, Duration::from_secs(1)), None);
        assert_eq!(rate(1024, 1024, Duration::ZERO), None);
        assert_eq!(percent(200.0, 2000.0), Some(10.0));
        assert_eq!(percent(1.0, 0.0), None);
    }

    #[test]
    fn live_sampler_only_returns_own_family_and_warms_up() {
        let mut sampler = SystemSampler::new(std::env::temp_dir());
        let first = sampler.sample();
        assert!(first.warming_up);
        assert_eq!(first.app.cpu_percent, None);
        assert!(first.processes.iter().any(|p| p.pid == std::process::id()));
        assert!(first.host.total_memory_bytes > 0);
        assert!(first.app.resident_bytes.is_some_and(|v| v > 0));
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL + Duration::from_millis(20));
        let second = sampler.sample();
        assert!(!second.warming_up);
        assert!(second
            .app
            .cpu_percent
            .is_some_and(|v| (0.0..=100.0).contains(&v)));
    }
}
