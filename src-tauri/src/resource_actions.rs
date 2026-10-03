//! Bounded timing of native operations, measured on their initiating worker.
//!
//! Wall time includes waits; thread CPU excludes work on other threads and
//! the WebView. Nested actions are inclusive, so their CPU must not be summed.
//! A completed record means the scope ended, including errors or unwinding; it
//! does not report success. Construct the guard inside the worker being measured.
use parking_lot::Mutex;
use serde::Serialize;
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_ACTIVE: usize = 128;
const MAX_RECENT: usize = 128;
const MAX_LABEL_CHARS: usize = 256;
const MAX_PHASE_CHARS: usize = 128;
const CPU_BASIS: &str = "Tempo de CPU da thread iniciadora (usuário + kernel), incluindo motores executados nessa thread; exclui workers paralelos e WebView. Operações aninhadas incluem o trabalho interno e não devem ser somadas.";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
thread_local! { static CURRENT_ACTION: Cell<Option<u64>> = const { Cell::new(None) }; }

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionState {
    Active,
    Completed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionProgress {
    pub operation: String,
    pub phase: String,
    pub completed: usize,
    pub total: usize,
    pub unit: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSample {
    pub id: u64,
    pub label: String,
    pub state: ActionState,
    pub started_at_ms: Option<u64>,
    pub elapsed_ms: f64,
    pub thread_cpu_ms: Option<f64>,
    pub progress: Option<ActionProgress>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRegistrySnapshot {
    pub active: Vec<ActionSample>,
    pub recent: Vec<ActionSample>,
    pub untracked_active: usize,
    pub total_completed: u64,
    pub cpu_basis: &'static str,
}

#[derive(Clone)]
struct ActiveAction {
    id: u64,
    label: String,
    started_at_ms: Option<u64>,
    started: Instant,
    clock: Option<Arc<ThreadCpuClock>>,
    initial_cpu_ns: Option<u64>,
    progress: Option<ActionProgress>,
}

struct ThreadCpuClock {
    native: platform::ThreadCpuClock,
    alive: AtomicBool,
}

impl ThreadCpuClock {
    fn current() -> Option<Self> {
        Some(Self {
            native: platform::ThreadCpuClock::current()?,
            alive: AtomicBool::new(true),
        })
    }

    fn read_ns(&self) -> Option<u64> {
        if !self.alive.load(Ordering::Acquire) {
            return None;
        }
        let cpu = self.native.read_ns()?;
        // A snapshot may retain an Arc after the guard finishes. In particular,
        // Linux CPU clock IDs use a TID that can be reused after thread exit.
        // Reject a reading if completion raced with the native clock query.
        self.alive.load(Ordering::Acquire).then_some(cpu)
    }

    fn invalidate(&self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl ActiveAction {
    fn sample(&self, state: ActionState, now: Instant) -> ActionSample {
        ActionSample {
            id: self.id,
            label: self.label.clone(),
            state,
            started_at_ms: self.started_at_ms,
            elapsed_ms: now.saturating_duration_since(self.started).as_secs_f64() * 1000.0,
            thread_cpu_ms: self.initial_cpu_ns.and_then(|initial| {
                self.clock
                    .as_ref()?
                    .read_ns()?
                    .checked_sub(initial)
                    .map(|ns| ns as f64 / 1_000_000.0)
            }),
            progress: self.progress.clone(),
        }
    }
}

#[derive(Default)]
struct Registry {
    active: BTreeMap<u64, ActiveAction>,
    recent: VecDeque<ActionSample>,
    untracked_active: usize,
    total_completed: u64,
}

impl Registry {
    fn active_count(&self) -> usize {
        self.active.len().saturating_add(self.untracked_active)
    }

    fn start(&mut self, id: u64, label: &str) -> bool {
        if self.active.len() >= MAX_ACTIVE {
            self.untracked_active = self.untracked_active.saturating_add(1);
            return false;
        }
        let started = Instant::now();
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| time.as_millis().try_into().ok());
        let clock = ThreadCpuClock::current().map(Arc::new);
        let initial_cpu_ns = clock.as_ref().and_then(|clock| clock.read_ns());
        self.active.insert(
            id,
            ActiveAction {
                id,
                label: bounded(label, MAX_LABEL_CHARS),
                started_at_ms,
                started,
                clock,
                initial_cpu_ns,
                progress: None,
            },
        );
        true
    }

    fn finish(&mut self, id: u64, tracked: bool) {
        self.total_completed = self.total_completed.saturating_add(1);
        if !tracked {
            self.untracked_active = self.untracked_active.saturating_sub(1);
            return;
        }
        if let Some(action) = self.active.remove(&id) {
            if self.recent.len() == MAX_RECENT {
                self.recent.pop_back();
            }
            let sample = action.sample(ActionState::Completed, Instant::now());
            // Freeze the completed CPU measurement before invalidating readers.
            if let Some(clock) = &action.clock {
                clock.invalidate();
            }
            self.recent.push_front(sample);
        }
    }
}

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn bounded(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// Records its initiating thread while the operation's scope remains alive.
/// The guard deliberately cannot be moved to another thread.
#[must_use = "dropping the guard ends the operation measurement"]
pub struct ActionGuard {
    id: u64,
    tracked: bool,
    previous: Option<u64>,
    _same_thread: PhantomData<Rc<()>>,
}

pub fn begin(label: impl AsRef<str>) -> ActionGuard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let tracked = registry().lock().start(id, label.as_ref());
    let previous = CURRENT_ACTION.with(|current| current.replace(Some(id)));
    ActionGuard {
        id,
        tracked,
        previous,
        _same_thread: PhantomData,
    }
}

impl Drop for ActionGuard {
    fn drop(&mut self) {
        registry().lock().finish(self.id, self.tracked);
        CURRENT_ACTION.with(|current| {
            if current.get() == Some(self.id) {
                current.set(self.previous);
            }
        });
    }
}

/// Updates the currently measured operation from its own worker thread.
/// Progress is advisory and keeps the producer's counts, including unknown total 0.
pub fn phase(operation: &str, phase: &str, completed: usize, total: usize, unit: &str) {
    CURRENT_ACTION.with(|current| {
        let Some(id) = current.get() else {
            return;
        };
        let mut registry = registry().lock();
        let Some(action) = registry.active.get_mut(&id) else {
            return;
        };
        action.progress = Some(ActionProgress {
            operation: bounded(operation, MAX_PHASE_CHARS),
            phase: bounded(phase, MAX_PHASE_CHARS),
            completed,
            total,
            unit: bounded(unit, MAX_PHASE_CHARS),
        });
    });
}

pub fn snapshot() -> ActionRegistrySnapshot {
    snapshot_registry(registry())
}

/// The continuous history needs only a count. Avoid cloning completed records
/// and querying every active thread clock when no action details were requested.
pub fn active_count() -> usize {
    registry().lock().active_count()
}

fn snapshot_registry(registry: &Mutex<Registry>) -> ActionRegistrySnapshot {
    // Clone at most 128 entries, then release the lock before OS clock reads.
    // Owned Windows handles stay valid through these Arc clones.
    let (actions, recent, untracked_active, total_completed) = {
        let registry = registry.lock();
        (
            registry.active.values().rev().cloned().collect::<Vec<_>>(),
            registry.recent.iter().cloned().collect(),
            registry.untracked_active,
            registry.total_completed,
        )
    };
    let now = Instant::now();
    ActionRegistrySnapshot {
        active: actions
            .iter()
            .map(|action| action.sample(ActionState::Active, now))
            .collect(),
        recent,
        untracked_active,
        total_completed,
        cpu_basis: CPU_BASIS,
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn ticks(&self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    // An owned kernel thread handle can be queried from the sampling thread.
    // Storing its opaque address permits sharing without sharing mutable memory.
    pub struct ThreadCpuClock {
        handle: usize,
    }
    impl ThreadCpuClock {
        pub fn current() -> Option<Self> {
            const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
            // No input comes from logs: request query-only access to this thread.
            let handle =
                unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, GetCurrentThreadId()) };
            if handle.is_null() {
                None
            } else {
                Some(Self {
                    handle: handle as usize,
                })
            }
        }
        pub fn read_ns(&self) -> Option<u64> {
            let mut creation = FileTime::default();
            let mut exit = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            // Fixed output structs, with the owned handle valid until Drop.
            let success = unsafe {
                GetThreadTimes(
                    self.handle as *mut c_void,
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            };
            if success == 0 {
                return None;
            }
            kernel.ticks().checked_add(user.ticks())?.checked_mul(100)
        }
    }
    impl Drop for ThreadCpuClock {
        fn drop(&mut self) {
            // Exactly one clock owns this handle; Arc extends that lifetime.
            unsafe {
                CloseHandle(self.handle as *mut c_void);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        seconds: c_long,
        nanos: c_long,
    }
    #[link(name = "pthread")]
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getcpuclockid(thread: usize, clock: *mut c_int) -> c_int;
    }
    extern "C" {
        fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
    }
    pub struct ThreadCpuClock {
        clock: c_int,
    }
    impl ThreadCpuClock {
        pub fn current() -> Option<Self> {
            let mut clock = 0;
            // The guard is thread-bound. A per-thread clock ID lets the separate
            // monitor query this thread, unlike CLOCK_THREAD_CPUTIME_ID itself.
            let status = unsafe { pthread_getcpuclockid(pthread_self(), &mut clock) };
            if status != 0 {
                None
            } else {
                Some(Self { clock })
            }
        }
        pub fn read_ns(&self) -> Option<u64> {
            let mut time = Timespec {
                seconds: 0,
                nanos: 0,
            };
            let status = unsafe { clock_gettime(self.clock, &mut time) };
            if status != 0 || time.seconds < 0 || !(0..1_000_000_000).contains(&time.nanos) {
                return None;
            }
            (time.seconds as u64)
                .checked_mul(1_000_000_000)?
                .checked_add(time.nanos as u64)
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub struct ThreadCpuClock;
    impl ThreadCpuClock {
        pub fn current() -> Option<Self> {
            None
        }
        pub fn read_ns(&self) -> Option<u64> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_bounds_active_and_history_without_evicting_active() {
        let mut registry = Registry::default();
        for id in 1..=MAX_ACTIVE as u64 {
            assert!(registry.start(id, "consulta"));
        }
        assert!(!registry.start(500, "overflow"));
        assert_eq!(registry.active.len(), MAX_ACTIVE);
        assert_eq!(registry.untracked_active, 1);
        assert_eq!(registry.active_count(), MAX_ACTIVE + 1);
        registry.finish(500, false);
        assert_eq!(registry.untracked_active, 0);
        assert_eq!(registry.active_count(), MAX_ACTIVE);
        for id in 1..=MAX_ACTIVE as u64 {
            registry.finish(id, true);
        }
        for id in 1000..1200 {
            assert!(registry.start(id, "consulta"));
            registry.finish(id, true);
        }
        assert!(registry.active.is_empty());
        assert_eq!(registry.active_count(), 0);
        assert_eq!(registry.recent.len(), MAX_RECENT);
        assert_eq!(registry.recent.front().unwrap().id, 1199);
        assert_eq!(registry.total_completed, MAX_ACTIVE as u64 + 201);
    }

    #[test]
    #[ignore = "manual performance measurement: filled action registry"]
    fn active_count_benchmark() {
        let mut filled = Registry::default();
        let label = "operação interna ".repeat(16);
        for id in 1..=MAX_RECENT as u64 {
            assert!(filled.start(id, &label));
            filled.finish(id, true);
        }
        for id in 1000..1000 + MAX_ACTIVE as u64 {
            assert!(filled.start(id, &label));
        }
        assert!(!filled.start(2000, "overflow"));
        let registry = Mutex::new(filled);
        assert_eq!(snapshot_registry(&registry).active.len(), MAX_ACTIVE);
        assert_eq!(registry.lock().active_count(), MAX_ACTIVE + 1);

        let started = Instant::now();
        for _ in 0..1_000 {
            let snapshot = snapshot_registry(std::hint::black_box(&registry));
            std::hint::black_box(snapshot.active.len() + snapshot.untracked_active);
        }
        let full_ns = started.elapsed().as_nanos() as f64 / 1_000.0;
        let started = Instant::now();
        for _ in 0..100_000 {
            std::hint::black_box(std::hint::black_box(&registry).lock().active_count());
        }
        let count_ns = started.elapsed().as_nanos() as f64 / 100_000.0;
        eprintln!("resource action registry: snapshot={full_ns:.0} ns/op; count={count_ns:.0} ns/op; ratio={:.1}x", full_ns / count_ns);
    }

    #[test]
    fn unavailable_cpu_serializes_as_null_and_labels_are_utf8_bounded() {
        let action = ActiveAction {
            id: 42,
            label: bounded(&"ação🎯".repeat(100), MAX_LABEL_CHARS),
            started_at_ms: None,
            started: Instant::now(),
            clock: None,
            initial_cpu_ns: None,
            progress: None,
        };
        let sample = action.sample(ActionState::Completed, Instant::now());
        assert_eq!(sample.label.chars().count(), MAX_LABEL_CHARS);
        let json = serde_json::to_value(&sample).unwrap();
        assert!(json["threadCpuMs"].is_null());
        assert!(json["startedAtMs"].is_null());
        assert_eq!(json["state"], "completed");
        assert!(sample.elapsed_ms >= 0.0);
    }

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn own_thread_clock_can_be_sampled_from_another_thread() {
        let clock = Arc::new(ThreadCpuClock::current().expect("own thread clock"));
        let before = clock.read_ns().expect("initial CPU time");
        let other = Arc::clone(&clock);
        let from_monitor = std::thread::spawn(move || other.read_ns())
            .join()
            .unwrap()
            .expect("cross-thread CPU time");
        let after = clock.read_ns().expect("final CPU time");
        assert!(before <= from_monitor && from_monitor <= after);
    }

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn finished_action_invalidates_retained_clock_after_freezing_final_cpu() {
        let mut registry = Registry::default();
        assert!(registry.start(42, "consulta"));
        let retained = registry
            .active
            .get(&42)
            .unwrap()
            .clock
            .as_ref()
            .unwrap()
            .clone();
        assert!(retained.read_ns().is_some());
        registry.finish(42, true);
        assert!(registry.recent.front().unwrap().thread_cpu_ms.is_some());
        assert!(retained.read_ns().is_none());
    }
}
