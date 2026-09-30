//! Coordinated budgets for parsing, foreground SQL and background indexing.
//! These limit engine buffers; source metadata and library overhead remain
//! additional allocations. Worker threads run below normal priority.
use std::sync::OnceLock;

const MIB: u64 = 1 << 20;

#[derive(Clone, Copy, Debug)]
struct Budget {
    parser_threads: usize,
    query_threads: usize,
    text_threads: usize,
    duckdb_bytes: u64,
    text_bytes: u64,
    batch_bytes: usize,
    queue_batches: usize,
}

impl Budget {
    fn for_machine(memory: u64, cores: usize, requested_mib: Option<u64>) -> Self {
        // Reserve room for the mapped source, its line metadata, the UI and OS.
        // Three databases can coexist (base + derived query sessions + background build).
        let ceiling = (memory / 2).max(128 * MIB);
        let bytes = requested_mib
            .and_then(|m| m.checked_mul(MIB))
            .unwrap_or(memory / 3)
            .clamp(128 * MIB, ceiling.min(8 << 30));
        let available = cores.saturating_sub(1).max(1);
        let parser_threads = available.div_ceil(2).min(8);
        let query_threads = available.saturating_sub(parser_threads).max(1).min(8);
        let text_bytes = (bytes / 8).max(16 * MIB);
        // Tantivy requires at least 15 MiB per indexing worker.
        let text_threads = (parser_threads / 4)
            .clamp(1, 2)
            .min((text_bytes / (16 * MIB)) as usize)
            .max(1);
        // Each queue's Arrow batch also has in-flight parser/string copies.
        // Byte-aware batches and shallow queues avoid a row-count-only budget.
        let batch_bytes = (bytes / 128).clamp(256 << 10, 4 * MIB) as usize;
        Self {
            parser_threads,
            query_threads,
            text_threads,
            duckdb_bytes: bytes / 4,
            text_bytes,
            batch_bytes,
            queue_batches: 2,
        }
    }
}

fn budget() -> &'static Budget {
    static BUDGET: OnceLock<Budget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let requested = std::env::var("LOGINSIGHT_MEMORY_LIMIT_MB").ok().and_then(|v| {
            let parsed = v.parse::<u64>().ok().filter(|v| *v >= 128);
            if parsed.is_none() { eprintln!("[recursos] LOGINSIGHT_MEMORY_LIMIT_MB inválido; usando orçamento automático"); }
            parsed
        });
        Budget::for_machine(total_memory(), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2), requested)
    })
}

/// Shared parser pool, bounded independently of the foreground SQL executor.
pub fn workers() -> usize {
    budget().parser_threads
}
pub(crate) fn query_threads() -> usize {
    budget().query_threads
}
pub(crate) fn duckdb_memory_mb() -> u64 {
    budget().duckdb_bytes / MIB
}
pub(crate) fn text_threads() -> usize {
    budget().text_threads
}
pub(crate) fn text_memory_bytes() -> usize {
    budget().text_bytes as usize
}
pub(crate) fn batch_bytes() -> usize {
    budget().batch_bytes
}
pub(crate) fn queue_batches() -> usize {
    budget().queue_batches
}

/// Effective memory in bytes, respecting a Linux container memory ceiling.
/// Source metadata and library overhead are additional to the engine budgets.
pub fn total_memory() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| {
        let physical = detect_memory().filter(|&m| m > 0).unwrap_or(8 << 30);
        container_memory_limit().map_or(physical, |limit| physical.min(limit))
    })
}

pub fn low_memory() -> bool {
    total_memory() < 8 << 30
}

fn parse_memory_limit(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n < (1u64 << 60))
}

#[cfg(target_os = "linux")]
fn container_memory_limit() -> Option<u64> {
    let mut candidates = vec![
        std::path::PathBuf::from("/sys/fs/cgroup/memory.max"),
        std::path::PathBuf::from("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ];
    if let Ok(groups) = std::fs::read_to_string("/proc/self/cgroup") {
        for line in groups.lines() {
            let mut fields = line.splitn(3, ':');
            let _ = fields.next();
            let controllers = fields.next().unwrap_or("");
            let path = fields.next().unwrap_or("").trim_start_matches('/');
            if path.split('/').any(|part| part == "..") {
                continue;
            }
            if controllers.is_empty() {
                candidates.push(
                    std::path::Path::new("/sys/fs/cgroup")
                        .join(path)
                        .join("memory.max"),
                );
            } else if controllers.split(',').any(|c| c == "memory") {
                candidates.push(
                    std::path::Path::new("/sys/fs/cgroup/memory")
                        .join(path)
                        .join("memory.limit_in_bytes"),
                );
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| parse_memory_limit(&s))
        })
        .min()
}
#[cfg(not(target_os = "linux"))]
fn container_memory_limit() -> Option<u64> {
    None
}

#[cfg(windows)]
fn detect_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullTotalPhys)
}

#[cfg(not(windows))]
fn detect_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Lowers the calling thread below normal priority (background work).
pub fn lower_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        // On Linux the nice value of PRIO_PROCESS 0 applies to the calling thread.
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// Configures the shared thread pool once, before any parallel work.
pub fn init() {
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(workers())
        .thread_name(|i| format!("loginsight-worker-{i}"))
        .start_handler(|_| lower_priority())
        .build_global();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinated_memory_budget_leaves_source_and_ui_headroom() {
        for gib in [1u64, 4, 8, 16, 64, 128] {
            let memory = gib << 30;
            let b = Budget::for_machine(memory, 16, None);
            assert!(b.duckdb_bytes * 3 + b.text_bytes <= memory / 2);
            assert!((256 << 10..=4 << 20).contains(&b.batch_bytes));
            assert_eq!(b.queue_batches, 2);
            assert!(b.parser_threads <= 8 && b.query_threads <= 8 && b.text_threads <= 2);
        }
    }
    #[test]
    fn one_core_and_explicit_limits_stay_usable() {
        let b = Budget::for_machine(4 << 30, 1, Some(128));
        assert_eq!(b.parser_threads, 1);
        assert_eq!(b.query_threads, 1);
        assert_eq!(b.text_threads, 1);
        assert_eq!(b.duckdb_bytes, 32 * MIB);
        let many_cores = Budget::for_machine(64 << 30, 64, Some(128));
        assert!(many_cores.text_bytes >= many_cores.text_threads as u64 * 15 * MIB);
        assert!(Budget::for_machine(4 << 30, 64, Some(u64::MAX)).duckdb_bytes <= 2 << 30);
    }
    #[test]
    fn container_limits_reject_unlimited_and_invalid_values() {
        assert_eq!(parse_memory_limit("1073741824\n"), Some(1 << 30));
        for value in ["max", "0", "garbage", "9223372036854771712"] {
            assert_eq!(parse_memory_limit(value), None);
        }
    }
}
