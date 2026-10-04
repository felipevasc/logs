//! Shared working-memory supervision for the local detection worker.
//! It measures the native process, including SQLite/allocator overhead. Loaded
//! source data is the baseline; browser transport is bounded separately.
use parking_lot::{Mutex, MutexGuard};
use std::cell::RefCell;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const LIMIT: u64 = 256 * 1024 * 1024;
static LANE: Mutex<()> = Mutex::new(());
static DISK: Mutex<BTreeMap<u64, (PathBuf, bool)>> = Mutex::new(BTreeMap::new());
static DISK_NEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Clone)]
struct Supervisor {
    baseline: u64,
    peak: std::sync::Arc<AtomicU64>,
    phase: std::sync::Arc<Mutex<&'static str>>,
    failure: std::sync::Arc<Mutex<Option<String>>>,
}
static ACTIVE: Mutex<Option<Supervisor>> = Mutex::new(None);
pub fn failure() -> Option<String> {
    ACTIVE
        .lock()
        .as_ref()
        .and_then(|s| s.failure.lock().clone())
}
fn supervise(s: &Supervisor) -> Result<(), String> {
    if let Some(error) = s.failure.lock().clone() {
        return Err(error);
    }
    let used = memory_bytes();
    s.peak.fetch_max(used, Ordering::Relaxed);
    let result = if used > s.baseline.saturating_add(LIMIT) {
        Err(format!("Orçamento global de trabalho de 256 MiB excedido em {} (pico {} bytes; base {} bytes). Análise cancelada sem publicar resultados parciais.",*s.phase.lock(),s.peak.load(Ordering::Relaxed),s.baseline))
    } else {
        check_disk()
    };
    if let Err(error) = &result {
        *s.failure.lock() = Some(error.clone());
    }
    result
}
fn disk_limit() -> u64 {
    std::env::var("LOGINSIGHT_SECURITY_DISK_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(16384)
        .clamp(512, 262144)
        * 1024
        * 1024
}
fn path_bytes(path: &Path, directory: bool) -> Result<u64, String> {
    if directory {
        let mut total = 0u64;
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            if meta.is_dir() {
                total = total.saturating_add(path_bytes(&entry.path(), true)?);
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
        Ok(total)
    } else {
        let mut total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        for suffix in ["-journal", "-wal", "-shm"] {
            total = total.saturating_add(
                std::fs::metadata(format!("{}{suffix}", path.to_string_lossy()))
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
        }
        Ok(total)
    }
}
pub struct DiskLease {
    id: u64,
}
impl DiskLease {
    pub fn register(path: &Path, directory: bool) -> Self {
        let id = DISK_NEXT.fetch_add(1, Ordering::Relaxed);
        DISK.lock().insert(id, (path.to_path_buf(), directory));
        Self { id }
    }
}
impl Drop for DiskLease {
    fn drop(&mut self) {
        DISK.lock().remove(&self.id);
    }
}
pub fn disk_snapshot() -> Result<serde_json::Value, String> {
    let registry = DISK.lock();
    let mut bytes = 0u64;
    let mut seen = std::collections::HashSet::new();
    for (path, directory) in registry.values() {
        if seen.insert(path)
            && !registry
                .values()
                .any(|(parent, recursive)| *recursive && parent != path && path.starts_with(parent))
        {
            bytes = bytes.saturating_add(path_bytes(path, *directory)?);
        }
    }
    Ok(
        serde_json::json!({"budget_bytes":disk_limit(),"owned_bytes":bytes,"tracked_stores":registry.len(),"scope":"live security facts, checkpoints, population, spools and result stores; original dataset is separate"}),
    )
}
fn check_disk() -> Result<(), String> {
    let disk = disk_snapshot()?;
    if disk["owned_bytes"].as_u64().unwrap_or(0) > disk_limit() {
        return Err("Orçamento agregado de disco da investigação excedido; nenhum resultado parcial foi publicado. Ajuste LOGINSIGHT_SECURITY_DISK_MB ou o universo analisado.".into());
    }
    let registry = DISK.lock();
    let mut checked = std::collections::HashSet::new();
    for (path, directory) in registry.values() {
        let parent = if *directory {
            path.as_path()
        } else {
            path.parent().unwrap_or(path)
        };
        if checked.insert(parent)
            && parent.exists()
            && fs2::available_space(parent).map_err(|e| e.to_string())? < 64 * 1024 * 1024
        {
            return Err("Disco da investigação com menos de 64 MiB disponíveis; análise interrompida antes de publicar resultado parcial".into());
        }
    }
    Ok(())
}
/// All security SQLite stores are named, leased, bounded and cancellable. Their
/// files remain observable to the aggregate disk supervisor until the last
/// result owner closes them.
pub struct TrackedFile {
    _path: tempfile::TempPath,
    _disk: DiskLease,
}
impl TrackedFile {
    pub fn new() -> Result<(std::fs::File, Self), String> {
        let file = tempfile::Builder::new()
            .prefix("security-spool-")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let (file, path) = file.into_parts();
        let disk = DiskLease::register(&path, false);
        Ok((
            file,
            Self {
                _path: path,
                _disk: disk,
            },
        ))
    }
}
pub struct TrackedConnection {
    db: rusqlite::Connection,
    _file: Option<tempfile::TempPath>,
    _disk: DiskLease,
}
impl std::ops::Deref for TrackedConnection {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
impl TrackedConnection {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref();
        let file = if path.as_os_str().is_empty() {
            Some(
                tempfile::Builder::new()
                    .prefix("security-")
                    .suffix(".sqlite")
                    .tempfile()
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .into_temp_path(),
            )
        } else {
            None
        };
        let actual = file.as_deref().unwrap_or(path);
        let db = rusqlite::Connection::open(actual)?;
        db.execute_batch(&format!(
            "PRAGMA cache_size=-2048;PRAGMA mmap_size=0;PRAGMA max_page_count={};",
            disk_limit() / 4096
        ))?;
        db.progress_handler(
            10000,
            Some(|| crate::operations::cancelled() || check().is_err()),
        );
        let disk = DiskLease::register(actual, false);
        Ok(Self {
            db,
            _file: file,
            _disk: disk,
        })
    }
}
#[derive(Clone, Copy, Default)]
struct State {
    baseline: u64,
    peak: u64,
    ticks: usize,
    active: bool,
    phase: &'static str,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
pub struct Guard {
    _lane: MutexGuard<'static, ()>,
    _stores: Vec<DiskLease>,
}
pub fn enter() -> Result<Guard, String> {
    let lane = loop {
        crate::operations::check()?;
        if let Some(lane) = LANE.try_lock_for(std::time::Duration::from_millis(100)) {
            break lane;
        }
    };
    let baseline = memory_bytes();
    *ACTIVE.lock() = Some(Supervisor {
        baseline,
        peak: std::sync::Arc::new(AtomicU64::new(baseline)),
        phase: std::sync::Arc::new(Mutex::new("initialization")),
        failure: std::sync::Arc::new(Mutex::new(None)),
    });
    STATE.with(|s| {
        *s.borrow_mut() = State {
            baseline,
            peak: baseline,
            ticks: 0,
            active: true,
            phase: "initialization",
        }
    });
    let mut stores = Vec::new();
    for name in [
        crate::security_facts::VERSION,
        crate::security_checkpoints::VERSION,
        "environment-history",
    ] {
        let path = crate::config_dir().join(name);
        if path.is_dir() {
            stores.push(DiskLease::register(&path, true));
        }
    }
    Ok(Guard {
        _lane: lane,
        _stores: stores,
    })
}
impl Drop for Guard {
    fn drop(&mut self) {
        STATE.with(|s| s.borrow_mut().active = false);
        *ACTIVE.lock() = None;
    }
}
pub fn phase(value: &'static str) {
    STATE.with(|s| s.borrow_mut().phase = value);
    if let Some(s) = ACTIVE.lock().as_ref() {
        *s.phase.lock() = value;
    }
}
pub fn check() -> Result<(), String> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if !s.active {
            return Ok(());
        }
        s.ticks += 1;
        if s.ticks % 128 != 0 {
            return Ok(());
        }
        let used = memory_bytes();
        s.peak = s.peak.max(used);
        if let Some(supervisor) = ACTIVE.lock().clone() {
            supervise(&supervisor)
        } else {
            Ok(())
        }
    })
}
pub fn check_now() -> Result<(), String> {
    if let Some(supervisor) = ACTIVE.lock().clone() {
        supervise(&supervisor)
    } else {
        Ok(())
    }
}
pub fn snapshot() -> serde_json::Value {
    STATE.with(|s|{let mut s=s.borrow_mut();s.peak=s.peak.max(memory_bytes());if let Some(supervisor)=ACTIVE.lock().as_ref(){s.peak=s.peak.max(supervisor.peak.load(Ordering::Relaxed));}serde_json::json!({"budget_bytes":LIMIT,"baseline_bytes":s.baseline,"peak_bytes":s.peak,"working_peak_bytes":s.peak.saturating_sub(s.baseline),"measured":s.baseline>0,"disk":disk_snapshot().ok()})})
}
/// A connection-specific watcher is stopped and joined before that connection
/// is dropped. Cancellation can interrupt a long columnar statement itself.
pub struct DuckWatch {
    stop: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl DuckWatch {
    pub fn new(connection: &duckdb::Connection) -> Result<Option<Self>, String> {
        crate::operations::check()?;
        let budget = ACTIVE.lock().clone();
        if budget.is_none()
            && crate::operations::current_generation().is_none()
            && crate::operations::current_id().is_none()
        {
            return Ok(None);
        }
        let token = crate::operations::current_token();
        let interrupt = connection.interrupt_handle();
        let (stop, finished) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("investigation-cancel".into())
            .spawn(move || {
                let mut ticks = 0;
                loop {
                    match finished.recv_timeout(std::time::Duration::from_millis(25)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            ticks += 1;
                            if token.cancelled()
                                || (ticks % 10 == 0
                                    && budget.as_ref().is_some_and(|s| supervise(s).is_err()))
                            {
                                interrupt.interrupt();
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Some(Self {
            stop,
            worker: Some(worker),
        }))
    }
}
impl Drop for DuckWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_investigation_sql_cancel_interrupts_the_query_and_stops_its_watcher() {
        let db = duckdb::Connection::open_in_memory().unwrap();
        let token = crate::operations::token(Some("v013-behavior-cancel".into())).unwrap();
        let (ready, started) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.recv().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(crate::operations::cancel_id("v013-behavior-cancel"));
        });
        let result = crate::operations::run_with_token(token, || {
            let _watch = DuckWatch::new(&db).unwrap();
            ready.send(()).unwrap();
            db.query_row(
                "SELECT sum(sqrt(i::DOUBLE)) FROM range(1000000000000) t(i)",
                [],
                |r| r.get::<_, f64>(0),
            )
        });
        worker.join().unwrap();
        assert!(result.is_err());
        assert_eq!(
            db.query_row("SELECT 7", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }
}
#[cfg(windows)]
fn memory_bytes() -> u64 {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private_usage: usize,
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    // OS fills a fixed C structure; no log data enters this call.
    unsafe {
        let mut counters: Counters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<Counters>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<Counters>() as u32,
        ) != 0
        {
            counters.private_usage as u64
        } else {
            0
        }
    }
}
#[cfg(target_os = "linux")]
fn memory_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}
#[cfg(not(any(windows, target_os = "linux")))]
fn memory_bytes() -> u64 {
    0
}
