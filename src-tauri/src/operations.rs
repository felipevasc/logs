//! Cooperative cancellation shared by desktop and MCP operations.
//! Legacy generation cancellation remains available for Cancel all. Named
//! operations add isolated tokens so cancelling one query never restarts others.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

static GENERATION: AtomicU64 = AtomicU64::new(0);
static UPDATE_PAUSED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicUsize = AtomicUsize::new(0);
static INTERACTIVE: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static START: Cell<Option<u64>> = const { Cell::new(None) };
    static LOCAL: RefCell<Option<Arc<Local>>> = const { RefCell::new(None) };
    static REPORT_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static REPORTER: RefCell<Option<Reporter>> = const { RefCell::new(None) };
    static REPORT_START: Cell<Option<Instant>> = const { Cell::new(None) };
    static REPORT_PHASE: RefCell<Option<Arc<Mutex<PhaseClock>>>> = const { RefCell::new(None) };
    static CHILD_STOPS: RefCell<Vec<Arc<AtomicBool>>> = const { RefCell::new(Vec::new()) };
}
type PhaseClock = Option<((&'static str, &'static str, &'static str), Instant)>;
pub(crate) type Reporter = Arc<dyn Fn(Progress) + Send + Sync>;
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    pub operation_id: Option<String>,
    pub operation: &'static str,
    pub phase_id: &'static str,
    pub phase: &'static str,
    pub completed: usize,
    pub total: usize,
    pub unit: &'static str,
    pub selected: usize,
    pub elapsed_ms: u64,
    pub phase_elapsed_ms: u64,
    pub cancellable: bool,
}
pub(crate) fn progress(phase_id: &'static str, phase: &'static str, completed: usize, total: usize, selected: usize) {
    report_progress("análise", phase_id, phase, completed, total, "candidatos", selected);
}
pub(crate) fn elapsed_ms() -> Option<u64> {
    REPORT_START.with(|s| s.get().map(|at| at.elapsed().as_millis() as u64))
}
pub(crate) fn report_progress(operation: &'static str, phase_id: &'static str, phase: &'static str, completed: usize, total: usize, unit: &'static str, selected: usize) {
    let reporter = REPORTER.with(|r| r.borrow().clone());
    if let Some(reporter) = reporter {
        let clock = REPORT_PHASE.with(|state| state.borrow_mut().get_or_insert_with(|| Arc::new(Mutex::new(None))).clone());
        let phase_elapsed_ms = {
            let mut state = clock.lock().unwrap_or_else(|e| e.into_inner());
            let key = (operation, phase_id, unit);
            if state.as_ref().is_none_or(|(old, _)| *old != key) { *state = Some((key, Instant::now())); }
            state.as_ref().unwrap().1.elapsed().as_millis() as u64
        };
        reporter(Progress {
            operation_id: current_id(), operation, phase_id, phase,
            completed, total, unit, selected,
            elapsed_ms: elapsed_ms().unwrap_or(0),
            phase_elapsed_ms,
            cancellable: current_generation().is_some(),
        });
    }
}
pub(crate) fn with_reporter<T>(reporter: Reporter, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<Reporter>);
    impl Drop for Reset { fn drop(&mut self) { REPORTER.with(|r| *r.borrow_mut() = self.0.take()); } }
    let _reset = Reset(REPORTER.with(|r| r.replace(Some(reporter))));
    f()
}
struct Local {
    id: String,
    cancelled: AtomicBool,
}
const MAX_PENDING_CANCELS: usize = 4096;
const PENDING_CANCEL_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct PendingCancellations {
    ids: HashMap<String, Instant>,
}
impl PendingCancellations {
    fn prune(&mut self, now: Instant) {
        self.ids
            .retain(|_, at| now.duration_since(*at) < PENDING_CANCEL_TTL);
    }
    fn insert(&mut self, id: &str, now: Instant) {
        self.prune(now);
        if self.ids.contains_key(id) {
            return;
        }
        if self.ids.len() >= MAX_PENDING_CANCELS {
            if let Some(oldest) = self
                .ids
                .iter()
                .min_by_key(|(_, at)| *at)
                .map(|(id, _)| id.clone())
            {
                self.ids.remove(&oldest);
            }
        }
        self.ids.insert(id.to_string(), now);
    }
    fn take(&mut self, id: &str, now: Instant) -> bool {
        self.prune(now);
        self.ids.remove(id).is_some()
    }
}

#[derive(Default)]
struct NamedRegistry {
    live: HashMap<String, Weak<Local>>,
    pending: PendingCancellations,
}
static NAMED: LazyLock<Mutex<NamedRegistry>> =
    LazyLock::new(|| Mutex::new(NamedRegistry::default()));

#[derive(Clone)]
pub(crate) struct Cancellation {
    generation: Option<u64>,
    local: Option<Arc<Local>>,
    report_id: Option<String>,
    reporter: Option<Reporter>,
    report_start: Option<Instant>,
    report_phase: Option<Arc<Mutex<PhaseClock>>>,
    child_stops: Vec<Arc<AtomicBool>>,
}
impl Cancellation {
    pub(crate) fn cancelled(&self) -> bool {
        self.child_stops.iter().any(|stop| stop.load(Ordering::Relaxed))
            || self.generation.is_some_and(|g| g != generation())
            || self
                .local
                .as_ref()
                .is_some_and(|local| local.cancelled.load(Ordering::Relaxed))
    }
    pub(crate) fn with_stop(mut self, stop: Arc<AtomicBool>) -> Self { self.child_stops.push(stop); self }
}

/// Foreground page/detail work can ask optional preparation to yield. Unlike
/// the update barrier, short status polls and unrelated operations do not
/// count as interaction and cannot starve a background accelerator forever.
pub(crate) struct Interaction;
impl Drop for Interaction {
    fn drop(&mut self) { INTERACTIVE.fetch_sub(1, Ordering::Release); }
}
pub(crate) fn interactive() -> Interaction {
    INTERACTIVE.fetch_add(1, Ordering::AcqRel);
    Interaction
}
pub(crate) fn interactive_active() -> bool { INTERACTIVE.load(Ordering::Acquire) != 0 }

/// The updater owns this reversible admission barrier until the installer
/// takes over. Failure/timeout drops it and leaves the source usable.
pub(crate) struct UpdatePause;
impl Drop for UpdatePause {
    fn drop(&mut self) {
        let _registry = NAMED.lock().unwrap_or_else(|e| e.into_inner());
        UPDATE_PAUSED.store(false, Ordering::SeqCst);
    }
}
pub(crate) fn update_paused() -> bool { UPDATE_PAUSED.load(Ordering::Acquire) }
pub(crate) fn pause_for_update() -> Result<UpdatePause, String> {
    let _registry = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    if UPDATE_PAUSED.swap(true, Ordering::SeqCst) { return Err("Uma atualização já está preparando o encerramento.".into()); }
    cancel();
    Ok(UpdatePause)
}
pub(crate) fn update_work_active() -> bool {
    let mut registry = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    registry.live.retain(|_, token| token.strong_count() > 0);
    !registry.live.is_empty() || RUNNING.load(Ordering::Acquire) != 0
}
struct RunningOperation;
impl Drop for RunningOperation {
    fn drop(&mut self) { RUNNING.fetch_sub(1, Ordering::Release); }
}
fn admit_running() -> Result<RunningOperation, String> {
    let _registry = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    if update_paused() { return Err("Atualização em preparação; aguarde ou tente novamente após o aviso.".into()); }
    RUNNING.fetch_add(1, Ordering::AcqRel);
    Ok(RunningOperation)
}

/// Register before queueing blocking work. Tauri can dispatch its async
/// command future after a synchronous cancel_task request, so consume an
/// early cancellation before allowing that future to queue work. The bounded
/// five-minute cache covers normal dispatch delay, not indefinitely stalled
/// futures; callers must use a unique ID for each request.
pub(crate) fn token(id: Option<String>) -> Result<Cancellation, String> {
    let id = id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if id.len() > 160 {
        return Err("Identificador de operação inválido.".into());
    }
    let mut named = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    if update_paused() { return Err("Atualização em preparação; aguarde ou tente novamente após o aviso.".into()); }
    named.live.retain(|_, token| token.strong_count() > 0);
    if named.live.get(&id).and_then(Weak::upgrade).is_some() {
        return Err("Identificador de operação já está em uso.".into());
    }
    if named.pending.take(&id, Instant::now()) {
        return Err("Operação cancelada.".into());
    }
    let local = Arc::new(Local {
        id: id.clone(),
        cancelled: AtomicBool::new(false),
    });
    named.live.insert(id, Arc::downgrade(&local));
    Ok(Cancellation {
        generation: Some(generation()),
        local: Some(local),
        report_id: None,
        reporter: None,
        report_start: None,
        report_phase: None,
        child_stops: Vec::new(),
    })
}
pub(crate) fn current_token() -> Cancellation {
    Cancellation {
        generation: current_generation(),
        local: LOCAL.with(|s| s.borrow().clone()),
        report_id: current_id(),
        reporter: REPORTER.with(|s| s.borrow().clone()),
        report_start: REPORT_START.with(Cell::get),
        report_phase: REPORT_PHASE.with(|s| s.borrow().clone()),
        child_stops: CHILD_STOPS.with(|s| s.borrow().clone()),
    }
}
pub(crate) fn current_id() -> Option<String> {
    REPORT_ID.with(|s| s.borrow().clone()).or_else(|| LOCAL.with(|s| s.borrow().as_ref().map(|s| s.id.clone())))
}
pub(crate) fn cancel_id(id: &str) -> bool {
    // Empty IDs are replaced by generated IDs at registration and cannot
    // address an operation. Enforce token()'s size limit before retaining one.
    if id.is_empty() || id.len() > 160 {
        return false;
    }
    let mut named = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(local) = named.live.get(id).and_then(Weak::upgrade) {
        local.cancelled.store(true, Ordering::Relaxed);
    } else {
        // Use the registration mutex too: checking for a live token and
        // storing the early cancellation must be atomic with token creation.
        named.pending.insert(id, Instant::now());
    }
    true
}
pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}
pub fn current_generation() -> Option<u64> {
    START.with(Cell::get)
}
pub fn cancelled() -> bool {
    current_token().cancelled()
}
/// For legacy worker callers. New workers should capture `current_token()`.
pub fn cancelled_for(generation: Option<u64>) -> bool {
    generation.is_some_and(|g| g != self::generation())
}
pub fn check() -> Result<(), String> {
    if cancelled() {
        Err("Operação cancelada.".into())
    } else {
        Ok(())
    }
}
/// The operation has reached its atomic publication boundary.
pub fn commit() {
    START.with(|s| s.set(None));
    LOCAL.with(|s| *s.borrow_mut() = None);
    CHILD_STOPS.with(|s| s.borrow_mut().clear());
}
pub(crate) fn run_with_token<T>(token: Cancellation, f: impl FnOnce() -> T) -> Result<T, String> {
    let _running = admit_running()?;
    struct Reset(Option<u64>, Option<Arc<Local>>, Option<String>, Option<Reporter>, Option<Instant>, Option<Arc<Mutex<PhaseClock>>>, Vec<Arc<AtomicBool>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            START.with(|s| s.set(self.0));
            LOCAL.with(|s| *s.borrow_mut() = self.1.take());
            REPORT_ID.with(|s| *s.borrow_mut() = self.2.take());
            REPORTER.with(|s| *s.borrow_mut() = self.3.take());
            REPORT_START.with(|s| s.set(self.4));
            REPORT_PHASE.with(|s| *s.borrow_mut() = self.5.take());
            CHILD_STOPS.with(|s| *s.borrow_mut() = std::mem::take(&mut self.6));
        }
    }
    let report_id = token.report_id.or_else(|| token.local.as_ref().map(|local| local.id.clone()));
    let _reset = Reset(
        START.with(|s| s.replace(token.generation)),
        LOCAL.with(|s| s.replace(token.local)),
        REPORT_ID.with(|s| s.replace(report_id)),
        REPORTER.with(|s| s.replace(token.reporter)),
        REPORT_START.with(|s| s.replace(Some(token.report_start.unwrap_or_else(Instant::now)))),
        REPORT_PHASE.with(|s| s.replace(Some(token.report_phase.unwrap_or_else(|| Arc::new(Mutex::new(None)))))),
        CHILD_STOPS.with(|s| s.replace(token.child_stops)),
    );
    check()?;
    let result = f();
    check()?;
    Ok(result)
}
pub fn run<T>(generation: u64, f: impl FnOnce() -> T) -> Result<T, String> {
    run_with_token(
        Cancellation {
            generation: Some(generation),
            local: LOCAL.with(|s| s.borrow().clone()),
            report_id: current_id(),
            reporter: REPORTER.with(|s| s.borrow().clone()),
            report_start: REPORT_START.with(Cell::get),
            report_phase: REPORT_PHASE.with(|s| s.borrow().clone()),
            child_stops: CHILD_STOPS.with(|s| s.borrow().clone()),
        },
        f,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interaction_priority_is_nested_scoped_and_unwind_safe() {
        assert!(!interactive_active());
        let foreground = interactive();
        assert!(interactive_active());
        { let nested = interactive(); drop(nested); }
        assert!(interactive_active(), "nested completion must not clear outer work");
        drop(foreground);
        assert!(!interactive_active());
        let _ = std::panic::catch_unwind(|| { let _foreground = interactive(); panic!("controlled interaction panic"); });
        assert!(!interactive_active());
    }

    #[test]
    fn update_pause_cancels_queued_work_blocks_new_admission_and_recovers() {
        let queued = token(Some(format!("update-queued-{}", uuid::Uuid::new_v4()))).unwrap();
        let pause = pause_for_update().unwrap();
        assert!(queued.cancelled());
        assert!(update_work_active(), "queued work is still owned until it settles");
        assert!(token(None).is_err());
        assert!(pause_for_update().is_err());
        assert!(run_with_token(queued, || panic!("paused work must not start")).is_err());
        assert!(!update_work_active());
        drop(pause);
        assert!(!update_paused());
        assert_eq!(run_with_token(token(None).unwrap(), || 9).unwrap(), 9);
    }

    #[test]
    fn update_wait_tracks_committed_finalization_and_panic_unwind() {
        let result = std::panic::catch_unwind(|| run_with_token(token(None).unwrap(), || {
            commit();
            assert!(update_work_active(), "commit releases cancellation, not the running-work lease");
            panic!("controlled operation panic");
        }));
        assert!(result.is_err());
        assert!(!update_work_active(), "unwinding releases the running-work lease");
    }

    #[test]
    fn named_cancellation_isolated_and_visible_to_workers() {
        let a = token(Some("isolated-a".into())).unwrap();
        let b = token(Some("isolated-b".into())).unwrap();
        assert!(cancel_id("isolated-a"));
        assert!(a.cancelled());
        assert!(!b.cancelled());
        assert!(run_with_token(a, || panic!("cancelled work must not run")).is_err());
        assert_eq!(run_with_token(b, || 7).unwrap(), 7);
    }
    #[test]
    fn cancellation_before_async_registration_rejects_only_that_request() {
        let id = format!("early-{}", uuid::Uuid::new_v4());
        assert!(cancel_id(&id));
        assert!(matches!(token(Some(id)), Err(error) if error == "Operação cancelada."));
        let other = token(Some(format!("other-{}", uuid::Uuid::new_v4()))).unwrap();
        assert_eq!(run_with_token(other, || 17).unwrap(), 17);
    }

    #[test]
    fn early_cancellation_rejects_invalid_ids_before_retaining_them() {
        assert!(!cancel_id(""));
        assert!(!cancel_id(&"x".repeat(161)));
    }

    #[test]
    fn pending_cancellation_expiry_is_monotonic_and_does_not_extend_on_duplicates() {
        let now = Instant::now();
        let mut pending = PendingCancellations::default();
        pending.insert("first", now);
        pending.insert("second", now + Duration::from_secs(1));
        pending.insert("first", now + Duration::from_secs(100));
        assert!(!pending.take("first", now + PENDING_CANCEL_TTL));
        assert!(pending.take("second", now + PENDING_CANCEL_TTL));
        assert!(pending.ids.is_empty());
    }

    #[test]
    fn pending_cancellation_capacity_evicts_oldest_without_affecting_other_ids() {
        let now = Instant::now();
        let mut pending = PendingCancellations::default();
        for n in 0..=MAX_PENDING_CANCELS {
            pending.insert(&format!("id-{n}"), now + Duration::from_micros(n as u64));
        }
        assert_eq!(pending.ids.len(), MAX_PENDING_CANCELS);
        let later = now + Duration::from_secs(1);
        assert!(!pending.take("id-0", later));
        assert!(!pending.take("not-cancelled", later));
        assert!(pending.take("id-1", later));
        assert!(pending.take(&format!("id-{MAX_PENDING_CANCELS}"), later));
        assert_eq!(pending.ids.len(), MAX_PENDING_CANCELS - 2);
    }

    #[test]
    fn duplicate_live_ids_are_rejected() {
        let _a = token(Some("same-operation".into())).unwrap();
        assert!(token(Some("same-operation".into())).is_err());
    }

    #[test]
    fn committed_progress_keeps_identity_without_reactivating_cancellation() {
        let id = "progress-after-publication";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        let result = run_with_token(token(Some(id.into())).unwrap(), || {
            with_reporter(Arc::new(move |event| capture.lock().unwrap().push(event)), || {
                progress("prepare", "Preparando", 1, 0, 0);
                commit();
                cancel_id(id);
                progress("ready", "Pronto", 1, 1, 1);
                42
            })
        });
        assert_eq!(result.unwrap(), 42);
        let events = seen.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].cancellable);
        assert_eq!(events[1].operation_id.as_deref(), Some(id));
        assert!(!events[1].cancellable);
        assert!(current_id().is_none());
    }
    #[test]
    fn progress_phase_clock_resets_without_resetting_operation_clock() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        run_with_token(token(Some("phase-clock-test".into())).unwrap(), || {
            REPORT_START.with(|s| s.set(Some(Instant::now() - Duration::from_secs(10))));
            REPORT_PHASE.with(|s| *s.borrow_mut() = Some(Arc::new(Mutex::new(Some((("análise", "verify", "candidatos"), Instant::now() - Duration::from_secs(2)))))));
            with_reporter(Arc::new(move |event| capture.lock().unwrap().push(event)), || {
                progress("verify", "Verificando", 10, 0, 5);
                progress("sql", "Calculando", 0, 0, 5);
            });
        }).unwrap();
        let events = seen.lock().unwrap();
        assert!(events[0].elapsed_ms >= 10_000 && events[0].phase_elapsed_ms >= 2_000);
        assert!(events[1].elapsed_ms >= 10_000 && events[1].phase_elapsed_ms < 1_000);
    }
    #[test]
    fn producer_inherits_reporting_clocks_and_its_stop_does_not_cancel_parent() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        run_with_token(token(Some("stream-report-parent".into())).unwrap(), || {
            REPORT_START.with(|s| s.set(Some(Instant::now() - Duration::from_secs(10))));
            with_reporter(Arc::new(move |event| capture.lock().unwrap().push(event)), || {
                progress("stream", "Lendo", 0, 0, 0);
                let stop = Arc::new(AtomicBool::new(false));
                let child = current_token().with_stop(Arc::clone(&stop));
                assert!(std::thread::spawn(move || run_with_token(child, || {
                    progress("stream", "Lendo", 1, 0, 1);
                    stop.store(true, Ordering::Relaxed);
                })).join().unwrap().is_err());
                assert!(!cancelled());
                progress("done", "Pronto", 1, 1, 1);
            });
        }).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|event| event.operation_id.as_deref() == Some("stream-report-parent") && event.elapsed_ms >= 10_000));
        assert!(seen[1].phase_elapsed_ms >= seen[0].phase_elapsed_ms);
    }
}
