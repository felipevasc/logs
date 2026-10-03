//! Coordinated budgets for parsing, foreground SQL and background indexing.
//! These limit engine buffers; source metadata and library overhead remain
//! additional allocations. Worker threads run below normal priority.
use std::sync::OnceLock;

const MIB: u64 = 1 << 20;
// Persisted MiB must round-trip through JSON and convert to bytes without
// overflow, independently of the machine/architecture that opens the profile.
pub(crate) const MAX_MEMORY_PREFERENCE_MIB: u64 = u64::MAX / MIB;
// Tantivy's segment arena has 32-bit addresses even in a 64-bit process.
// Stay below its u32::MAX - 1,000,000 per-writer bound. This component cap
// does not restrict the application's configurable detected-memory budget.
const MAX_TEXT_WRITER_BYTES: u64 = u32::MAX as u64 - MIB;

/// Allocation-sized components must never wrap on 32-bit targets. Their
/// address-space cap is separate from the user-visible physical-memory limit.
pub(crate) fn allocation_bytes(bytes: u64) -> usize {
    bounded_allocation_bytes(bytes, isize::MAX as u64) as usize
}
fn bounded_allocation_bytes(bytes: u64, addressable: u64) -> u64 {
    bytes.min(addressable)
}

#[derive(Clone, Copy, Debug)]
struct Budget {
    effective_bytes: u64,
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
        // Automatic leaves room for mapped sources, metadata, the UI and OS.
        // An explicit preference may use all detected memory (including the
        // cgroup ceiling). It is a reference budget, never a RAM reservation.
        let bytes = requested_mib
            .and_then(|m| m.checked_mul(MIB))
            .unwrap_or(memory / 3)
            .max(128 * MIB)
            .min(memory);
        let available = cores.saturating_sub(1).max(1);
        let parser_threads = available.div_ceil(2).min(8);
        let query_threads = available.saturating_sub(parser_threads).max(1).min(8);
        let text_bytes = allocation_bytes((bytes / 8).min(MAX_TEXT_WRITER_BYTES)) as u64;
        // Tantivy requires at least 15 MiB per indexing worker.
        let text_threads = (parser_threads / 4)
            .clamp(1, 2)
            .min((text_bytes / (16 * MIB)) as usize)
            .max(1);
        // Each queue's Arrow batch also has in-flight parser/string copies.
        // Byte-aware batches and shallow queues avoid a row-count-only budget.
        let batch_bytes = (bytes / 128).clamp(256 << 10, 4 * MIB) as usize;
        Self {
            effective_bytes: bytes,
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
        // A command only saves next-start preferences. Never re-read that file
        // here or modify process environment while engines are already running.
        let requested = environment_memory()
            .value
            .or(crate::resource_settings::startup_memory_mib());
        Budget::for_machine(
            total_memory(),
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2),
            requested,
        )
    })
}

struct EnvironmentMemory {
    value: Option<u64>,
    invalid: bool,
}
fn parse_requested_memory(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n >= 128 && n.checked_mul(MIB).is_some())
}
fn environment_memory() -> &'static EnvironmentMemory {
    static VALUE: OnceLock<EnvironmentMemory> = OnceLock::new();
    VALUE.get_or_init(|| {
        let raw = std::env::var_os("LOGINSIGHT_MEMORY_LIMIT_MB");
        let value = raw.as_ref().and_then(|s| s.to_str()).and_then(parse_requested_memory);
        let invalid = raw.is_some() && value.is_none();
        if invalid { eprintln!("[recursos] LOGINSIGHT_MEMORY_LIMIT_MB inválido; usando a preferência salva ou automática"); }
        EnvironmentMemory { value, invalid }
    })
}

pub(crate) fn maximum_memory_mib(memory: u64) -> u64 {
    memory / MIB
}

/// Nominal limits of the individual engines. Concurrent SQL sessions can
/// multiply their allocations; this is not an aggregate process/RSS quota.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    memory_available_mib: u64,
    memory_budget_mib: u64,
    duckdb_per_instance_mib: u64,
    text_index_mib: u64,
    selection_cache_mib: u64,
    parser_threads: usize,
    query_threads_per_session: usize,
    text_threads: usize,
    environment_override_mib: Option<u64>,
    invalid_environment_override: bool,
    conservative_builder: bool,
    global_parallelism: usize,
    maximum_parallelism: usize,
}
pub(crate) fn snapshot() -> Snapshot {
    let b = budget();
    Snapshot {
        memory_available_mib: total_memory() / MIB,
        memory_budget_mib: b.effective_bytes / MIB,
        duckdb_per_instance_mib: b.duckdb_bytes / MIB,
        text_index_mib: b.text_bytes / MIB,
        selection_cache_mib: application_selection_cache_bytes() / MIB,
        parser_threads: workers(),
        query_threads_per_session: query_threads(),
        text_threads: text_threads(),
        environment_override_mib: environment_memory().value,
        invalid_environment_override: environment_memory().invalid,
        conservative_builder: low_memory(),
        global_parallelism: global_parallelism(),
        maximum_parallelism: maximum_parallelism(),
    }
}

/// Maximum width of a parser wave, further bounded by global admission.
pub fn workers() -> usize {
    // Preserve existing byte/row wave sizes; more global slots must not
    // silently multiply parser metadata buffers.
    budget().parser_threads.min(global_parallelism())
}
pub(crate) fn query_threads() -> usize {
    // All SQL sessions share global admission; native parallelism must not
    // multiply it. Query CPU executes in the calling reservation.
    1
}
pub(crate) fn maximum_parallelism() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 64)
}
pub(crate) fn global_parallelism() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| crate::resource_settings::startup_parallelism_limit()
        .unwrap_or_else(|| maximum_parallelism().saturating_sub(1).max(1))
        .clamp(1, maximum_parallelism()))
}
pub(crate) fn duckdb_memory_mb() -> u64 {
    budget().duckdb_bytes / MIB
}
pub(crate) fn effective_bytes() -> u64 {
    budget().effective_bytes
}
pub(crate) fn text_threads() -> usize {
    1
}
pub(crate) fn text_memory_bytes() -> usize {
    allocation_bytes(budget().text_bytes)
}
pub(crate) fn batch_bytes() -> usize {
    budget().batch_bytes
}
pub(crate) fn queue_batches() -> usize {
    budget().queue_batches
}

/// These are accounted payload limits, not claims about total allocator usage.
/// A normal 50M-row BIGINT selection (400 MB) remains admissible, but is too
/// large to retain in the interactive cache on typical desktop budgets.
pub(crate) fn selection_bytes() -> u64 {
    #[cfg(test)]
    if let Some(limit) = TEST_SELECTION_LIMIT.with(std::cell::Cell::get) {
        return limit;
    }
    crate::case_resources::current().map_or_else(application_selection_bytes, |policy| policy.selection_bytes())
}
pub(crate) fn application_selection_bytes() -> u64 {
    configured_bytes(
        "LOGINSIGHT_SELECTION_LIMIT_MB",
        (total_memory() / 2).clamp(1 << 30, 8 << 30),
    )
}
#[cfg(test)]
thread_local! { static TEST_SELECTION_LIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_selection_limit<T>(limit: u64, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_SELECTION_LIMIT.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(TEST_SELECTION_LIMIT.with(|c| c.replace(Some(limit))));
    f()
}
pub(crate) fn selection_cache_bytes() -> u64 {
    crate::case_resources::current().map_or_else(application_selection_cache_bytes, |policy| policy.selection_cache_bytes())
}
pub(crate) fn application_selection_cache_bytes() -> u64 {
    // One 50M-ID hot selection fits on the measured 10 GiB configuration.
    // The weighted LRU evicts older broad filters instead of retaining eight.
    configured_bytes(
        "LOGINSIGHT_SELECTION_CACHE_MB",
        (budget().duckdb_bytes / 2).min(512 * MIB),
    )
}
pub(crate) fn collected_ids_bytes() -> usize {
    #[cfg(test)]
    if let Some(limit) = TEST_COLLECTED_LIMIT.with(std::cell::Cell::get) {
        return limit;
    }
    crate::case_resources::current().map_or_else(application_collected_ids_bytes, |policy| policy.collected_ids_bytes())
}
pub(crate) fn application_collected_ids_bytes() -> usize {
    allocation_bytes(configured_bytes(
        "LOGINSIGHT_COLLECTED_IDS_MB",
        (budget().effective_bytes / 32).min(128 * MIB),
    ))
}
#[cfg(test)]
thread_local! { static TEST_COLLECTED_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_collected_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_COLLECTED_LIMIT.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(TEST_COLLECTED_LIMIT.with(|c| c.replace(Some(limit))));
    f()
}
pub(crate) fn analytics_bytes() -> usize {
    #[cfg(test)]
    if let Some(limit) = TEST_ANALYTICS_LIMIT.with(std::cell::Cell::get) {
        return limit;
    }
    crate::case_resources::current().map_or_else(application_analytics_bytes, |policy| policy.analytics_bytes())
}
pub(crate) fn application_analytics_bytes() -> usize {
    allocation_bytes(configured_bytes(
        "LOGINSIGHT_ANALYTICS_LIMIT_MB",
        (budget().effective_bytes / 32).min(128 * MIB),
    ))
}
#[cfg(test)]
thread_local! { static TEST_ANALYTICS_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_analytics_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_ANALYTICS_LIMIT.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(TEST_ANALYTICS_LIMIT.with(|c| c.replace(Some(limit))));
    f()
}
fn configured_bytes(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|n| n.checked_mul(MIB))
        .filter(|&n| n > 0)
        .unwrap_or(fallback)
}

/// A limit on DuckDB spill, separate from source/checkpoint files and memory.
/// Two foreground sessions and one builder may coexist. This is a ceiling,
/// not a disk reservation; another process can still exhaust available space.
pub(crate) fn spill_bytes(build: bool, directory: &std::path::Path) -> Result<u64, String> {
    let available = fs2::available_space(directory).map_err(|e| e.to_string())?;
    let total = configured_bytes("LOGINSIGHT_TEMP_LIMIT_MB", (available / 2).min(8 << 30)).max(MIB);
    let query = configured_bytes("LOGINSIGHT_QUERY_SPILL_MB", total / 3);
    let builder = configured_bytes("LOGINSIGHT_BUILD_SPILL_MB", total / 3);
    if query.saturating_mul(2).saturating_add(builder) > total {
        return Err("Orçamento temporário inválido: duas sessões de consulta e a preparação excedem LOGINSIGHT_TEMP_LIMIT_MB.".into());
    }
    Ok(if build { builder } else { query })
}

/// Effective memory in bytes, respecting a Linux container memory ceiling.
/// Source metadata and library overhead are additional to the engine budgets.
pub fn total_memory() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| {
        let physical = detect_memory().filter(|&m| m > 0).unwrap_or(8 << 30);
        effective_memory(physical, container_memory_limit())
    })
}

fn effective_memory(physical: u64, container: Option<u64>) -> u64 {
    container.map_or(physical, |limit| physical.min(limit))
}

pub fn low_memory() -> bool {
    conservative_builder(total_memory(), effective_bytes())
}
fn conservative_builder(memory: u64, budget: u64) -> bool {
    memory < 8 << 30 || budget < (8u64 << 30) / 3
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
            let (root, filename) = if controllers.is_empty() {
                (std::path::Path::new("/sys/fs/cgroup"), "memory.max")
            } else if controllers.split(',').any(|c| c == "memory") {
                (std::path::Path::new("/sys/fs/cgroup/memory"), "memory.limit_in_bytes")
            } else {
                continue;
            };
            // A nested group can be unlimited while an ancestor is limited.
            // Include every visible ancestor, not only the leaf and root.
            candidates.extend(cgroup_limit_paths(root, path, filename));
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
#[cfg(any(target_os = "linux", test))]
fn cgroup_limit_paths(root: &std::path::Path, group: &str, filename: &str) -> Vec<std::path::PathBuf> {
    let group = std::path::Path::new(group.trim_start_matches('/'));
    if group.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Vec::new();
    }
    group.ancestors().map(|ancestor| root.join(ancestor).join(filename)).collect()
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
    kib.checked_mul(1024)
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
        .num_threads(global_parallelism())
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
    fn explicit_low_profile_reserves_transient_analytics_and_id_buffers() {
        let b = Budget::for_machine(10 << 30, 9, Some(128));
        let transient = b.effective_bytes / 32;
        assert_eq!(transient, 4 * MIB);
        // Three SQL instances, text writer, analytical values, collected IDs,
        // retained line IDs, and two queued batches with parser copies.
        assert!(
            b.duckdb_bytes * 3 + b.text_bytes + transient * 3 + b.batch_bytes as u64 * 4
                <= b.effective_bytes
        );
        let measured = Budget::for_machine(10 << 30, 9, None);
        assert!((measured.duckdb_bytes / 2).min(512 * MIB) >= 50_000_000 * 8);
    }
    #[test]
    fn container_limits_reject_unlimited_and_invalid_values() {
        assert_eq!(parse_memory_limit("1073741824\n"), Some(1 << 30));
        for value in ["max", "0", "garbage", "9223372036854771712"] {
            assert_eq!(parse_memory_limit(value), None);
        }
    }
    #[test]
    fn reduced_budget_on_large_machine_uses_conservative_builder() {
        assert!(conservative_builder(64 << 30, 256 * MIB));
        assert!(!conservative_builder(8 << 30, (8 << 30) / 3));
        assert!(conservative_builder(4 << 30, 8 << 30));
        assert_eq!(maximum_memory_mib(4 << 30), 4096);
        assert_eq!(maximum_memory_mib(64 << 30), 65536);
        assert_eq!(maximum_memory_mib(128 << 30), 131072);
        assert_eq!(maximum_memory_mib(128 * MIB), 128);
    }
    #[test]
    fn custom_budget_reaches_detected_total_without_changing_automatic_headroom() {
        for gib in [1u64, 4, 8, 16, 64, 128] {
            let memory = gib << 30;
            let maximum = maximum_memory_mib(memory);
            let custom = Budget::for_machine(memory, 16, Some(maximum));
            assert_eq!(custom.effective_bytes, memory);
            assert_eq!(Budget::for_machine(memory, 16, Some(maximum + 1)).effective_bytes, memory);
            assert_eq!(Budget::for_machine(memory, 16, None).effective_bytes, memory / 3);
            let transient = (custom.effective_bytes / 32).min(128 * MIB);
            assert!(custom.duckdb_bytes * 3 + custom.text_bytes + transient * 3 + custom.batch_bytes as u64 * 4 <= memory);
            assert!(custom.text_bytes >= custom.text_threads as u64 * 15 * MIB);
            assert!(custom.text_bytes < u32::MAX as u64 - 1_000_000, "Tantivy's arena must remain addressable on large machines");
        }
        let moved = Budget::for_machine(4 << 30, 4, Some(131072));
        assert_eq!(moved.effective_bytes, 4 << 30);
        assert_eq!(maximum_memory_mib(128 * MIB - 1), 127, "never round detected capacity up");
        assert!(Budget::for_machine(64 * MIB, 1, None).effective_bytes <= 64 * MIB);
    }
    #[test]
    fn cgroup_total_and_ancestor_limits_do_not_use_free_memory() {
        assert_eq!(effective_memory(128 << 30, Some(4 << 30)), 4 << 30);
        assert_eq!(effective_memory(4 << 30, Some(128 << 30)), 4 << 30);
        assert_eq!(effective_memory(64 << 30, None), 64 << 30);
        let paths = cgroup_limit_paths(std::path::Path::new("/sys/fs/cgroup"), "team/job", "memory.max");
        assert_eq!(paths, ["/sys/fs/cgroup/team/job/memory.max", "/sys/fs/cgroup/team/memory.max", "/sys/fs/cgroup/memory.max"].map(std::path::PathBuf::from));
        assert!(cgroup_limit_paths(std::path::Path::new("/sys/fs/cgroup"), "../../outside", "memory.max").is_empty());
    }
    #[test]
    fn allocation_components_saturate_instead_of_wrapping_on_32_bit() {
        let max32 = i32::MAX as u64;
        for bytes in [4u64 << 30, 64 << 30, 128 << 30, u64::MAX] {
            assert_eq!(bounded_allocation_bytes(bytes, max32), max32);
        }
        assert_eq!(bounded_allocation_bytes(16 * MIB, max32), 16 * MIB);
        assert_eq!(allocation_bytes(u64::MAX), isize::MAX as usize);
        assert!(MAX_MEMORY_PREFERENCE_MIB.checked_mul(MIB).is_some());
        assert!((MAX_MEMORY_PREFERENCE_MIB + 1).checked_mul(MIB).is_none());
    }
    #[test]
    fn environment_memory_rejects_overflow_and_invalid_limits() {
        assert_eq!(parse_requested_memory("512"), Some(512));
        assert_eq!(parse_requested_memory("131072"), Some(131072));
        for value in ["0", "127", "-1", "garbage", "18446744073709551615"] {
            assert_eq!(parse_requested_memory(value), None);
        }
    }
}
