//! How much of the machine heavy work may use. Worker threads leave one
//! logical core free and run below normal priority, so the interface and the
//! rest of the system stay responsive on modest machines while strong
//! machines still use nearly all their cores. Memory budgets scale with the
//! installed memory.
use std::sync::OnceLock;

/// Threads for parallel work: every logical core but one.
pub fn workers() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .saturating_sub(1)
            .max(1)
    })
}

/// Installed memory in bytes (8 GiB when the system does not tell).
pub fn total_memory() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| detect_memory().filter(|&m| m > 0).unwrap_or(8 << 30))
}

/// Machines with little memory get smaller batches and fewer writers.
pub fn low_memory() -> bool {
    total_memory() < 8 << 30
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
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
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
