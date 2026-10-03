//! Process-wide admission for managed CPU work. A root owns one reservation;
//! streaming children transfer its execution lane while their parent waits.
//! Rayon waves may borrow only idle global reservations (never wait while
//! holding application locks). DuckDB and text writers execute synchronously.
//! Runtime, UI, I/O watchers and library housekeeping threads are not an OS
//! thread quota. No application lock is acquired by this scheduler.
use parking_lot::{Condvar, Mutex};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Priority { Background, #[default] Normal, Interactive }
struct Waiting { id: u64, priority: Priority, since: Instant }
#[derive(Default)]
struct State { used: usize, peak: usize, next: u64, queue: VecDeque<Waiting> }
pub(crate) struct Scheduler { limit: usize, state: Mutex<State>, changed: Condvar }
impl Scheduler {
    pub(crate) fn new(limit: usize) -> Arc<Self> { Arc::new(Self { limit: limit.max(1), state: Mutex::new(State::default()), changed: Condvar::new() }) }
    fn acquire(self: &Arc<Self>, priority: Priority, cancelled: &dyn Fn() -> bool) -> Result<Reservation, String> {
        // Cancellation callbacks can acquire application locks, so never invoke
        // them while holding the admission mutex.
        if cancelled() { return Err("Operação cancelada.".into()); }
        let id = { let mut s = self.state.lock(); let id = s.next; s.next += 1; s.queue.push_back(Waiting { id, priority, since: Instant::now() }); id };
        struct Queued<'a>(&'a Scheduler, u64);
        impl Drop for Queued<'_> { fn drop(&mut self) { self.0.state.lock().queue.retain(|q| q.id != self.1); self.0.changed.notify_all(); } }
        let queued = Queued(self, id);
        loop {
            if cancelled() { return Err("Operação cancelada.".into()); }
            let mut s = self.state.lock();
            // After 250ms the oldest waiter wins, even under a continuous
            // stream of page requests. Equal-priority admission is FIFO.
            let next = s.queue.front().filter(|q| q.since.elapsed() >= Duration::from_millis(250)).map(|q| q.id)
                .or_else(|| s.queue.iter().max_by_key(|q| (q.priority, std::cmp::Reverse(q.id))).map(|q| q.id));
            if s.used < self.limit && next == Some(id) {
                s.used += 1; s.peak = s.peak.max(s.used);
                s.queue.retain(|q| q.id != id);
                drop(s); drop(queued); self.changed.notify_all();
                return Ok(Reservation { scheduler: Arc::clone(self), count: 1 });
            }
            self.changed.wait_for(&mut s, Duration::from_millis(20));
        }
    }
    fn borrow_idle(self: &Arc<Self>, wanted: usize) -> Reservation {
        let mut s = self.state.lock();
        let count = if s.queue.is_empty() { wanted.min(self.limit - s.used) } else { 0 };
        s.used += count; s.peak = s.peak.max(s.used);
        Reservation { scheduler: Arc::clone(self), count }
    }
}
struct Reservation { scheduler: Arc<Scheduler>, count: usize }
impl Drop for Reservation { fn drop(&mut self) { let mut s = self.scheduler.state.lock(); s.used -= self.count; drop(s); self.scheduler.changed.notify_all(); } }
struct Lanes { width: usize, active: Mutex<usize>, changed: Condvar, _extra: Option<Reservation> }
impl Lanes {
    fn new(width: usize, extra: Option<Reservation>) -> Arc<Self> { Arc::new(Self { width, active: Mutex::new(0), changed: Condvar::new(), _extra: extra }) }
    fn enter(&self, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        loop {
            if cancelled() { return Err("Operação cancelada.".into()); }
            let mut active = self.active.lock();
            if *active < self.width { *active += 1; return Ok(()); }
            self.changed.wait_for(&mut active, Duration::from_millis(20));
        }
    }
    fn leave(&self) { let mut active = self.active.lock(); *active -= 1; drop(active); self.changed.notify_all(); }
}
struct Group { scheduler: Arc<Scheduler>, reservation: Mutex<Option<Reservation>>, priority: Priority, root: Arc<Lanes> }
#[derive(Clone)]
pub(crate) struct Execution { group: Arc<Group>, lanes: Arc<Lanes> }
#[derive(Clone)]
struct Context { execution: Execution, held: bool }
thread_local! { static CURRENT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
fn scheduler() -> Arc<Scheduler> {
    #[cfg(test)] if let Some(scheduler) = TEST_SCHEDULER.with(|s| s.borrow().clone()) { return scheduler; }
    static SCHEDULER: OnceLock<Arc<Scheduler>> = OnceLock::new();
    Arc::clone(SCHEDULER.get_or_init(|| Scheduler::new(crate::resources::global_parallelism())))
}
pub(crate) fn current() -> Option<Execution> { CURRENT.with(|s| s.borrow().as_ref().map(|c| c.execution.clone())) }
struct Entered { previous: Option<Context> }
impl Drop for Entered {
    fn drop(&mut self) {
        let old = CURRENT.with(|c| c.replace(self.previous.take()));
        if let Some(old) = old.filter(|c| c.held) { old.execution.lanes.leave(); }
    }
}
fn enter<T>(execution: Execution, cancelled: &dyn Fn() -> bool, f: impl FnOnce() -> T) -> Result<T, String> {
    execution.lanes.enter(cancelled)?;
    let _entered = Entered { previous: CURRENT.with(|c| c.replace(Some(Context { execution, held: true }))) };
    Ok(f())
}
pub(crate) fn run<T>(inherited: Option<Execution>, priority: Priority, cancelled: &dyn Fn() -> bool, f: impl FnOnce() -> T) -> Result<T, String> {
    if CURRENT.with(|c| c.borrow().as_ref().is_some_and(|c| c.held)) {
        // Same-thread nesting does not debit twice. A cloned token on another
        // thread enters the execution lane normally; it never gets this bypass.
        if cancelled() { return Err("Operação cancelada.".into()); }
        return Ok(f());
    }
    let execution = if let Some(execution) = inherited { execution } else {
        let scheduler = scheduler();
        let reservation = scheduler.acquire(priority, cancelled)?;
        let root = Lanes::new(1, None);
        Execution { group: Arc::new(Group { scheduler, reservation: Mutex::new(Some(reservation)), priority, root: Arc::clone(&root) }), lanes: root }
    };
    enter(execution, cancelled, f)
}

/// Transfer execution to a child during a blocking channel/join, retaining the
/// root's global reservation. Otherwise an unrelated writer can take the last
/// slot and wait on a source guard held by the parent, preventing its child.
pub(crate) fn blocking<T>(f: impl FnOnce() -> T) -> T {
    let execution = CURRENT.with(|c| { let mut c = c.borrow_mut(); let c = c.as_mut()?; if !c.held { return None; } c.held = false; Some(c.execution.clone()) });
    struct Resume(Option<Execution>);
    impl Drop for Resume { fn drop(&mut self) { if let Some(execution) = &self.0 { execution.lanes.enter(&|| false).expect("uncancellable lane restore"); CURRENT.with(|c| { if let Some(c) = c.borrow_mut().as_mut() { c.held = true; } }); } } }
    if let Some(execution) = &execution { execution.lanes.leave(); }
    let _resume = Resume(execution);
    f()
}

/// Network calls with an exclusively owned root and no application guards
/// may release the reservation, unlike producer/consumer handoffs. Call only
/// at reviewed lock-free I/O boundaries, never around arbitrary callbacks.
/// TLS and socket/runtime housekeeping are outside the managed CPU quota.
pub(crate) fn unreserved_io<T>(f: impl FnOnce() -> T) -> T {
    let Some(execution) = current() else { return f(); };
    if !Arc::ptr_eq(&execution.lanes, &execution.group.root)
        || Arc::strong_count(&execution.group) != 2 {
        return blocking(f);
    }
    struct Restore(Execution);
    impl Drop for Restore {
        fn drop(&mut self) {
            // Restore even on panic or cancellation: caller cleanup may still
            // execute. The operation rechecks cancellation before JSON/CPU.
            let reservation = self.0.group.scheduler.acquire(self.0.group.priority, &|| false)
                .expect("uncancellable I/O restoration");
            *self.0.group.reservation.lock() = Some(reservation);
        }
    }
    let reservation = execution.group.reservation.lock().take();
    drop(reservation);
    let _restore = Restore(execution);
    f()
}

/// A bounded ordered Rayon wave. All expensive closures consume a counted
/// lane. Nested waves surrender their calling lane before joining, so a
/// one-slot budget and recursive parallel work both make progress.
pub(crate) fn map<T: Send, R: Send>(items: impl IntoIterator<Item=T>, f: impl Fn(T) -> R + Sync + Send) -> Vec<R> {
    use rayon::prelude::*;
    let items: Vec<T> = items.into_iter().collect();
    if items.is_empty() { return Vec::new(); }
    let Some(parent) = current() else {
        // Direct low-level library/test calls are serial, never an unaccounted
        // fan-out. Production command and background entrypoints are admitted.
        return items.into_iter().map(f).collect();
    };
    let token = crate::operations::current_token();
    if !Arc::ptr_eq(&parent.lanes, &parent.group.root) {
        return blocking(|| items.into_par_iter().map(|item| enter(parent.clone(), &|| false,
            || crate::operations::with_context(token.clone(), || f(item))).expect("uncancellable wave lane")).collect());
    }
    // Keep the prior parser fan-out as well as the global ceiling. Raising
    // the aggregate setting must not enlarge one wave's working buffers.
    let extra = parent.group.scheduler.borrow_idle(items.len().min(wave_width()).saturating_sub(1));
    let width = extra.count + 1;
    if width == 1 { return items.into_iter().map(f).collect(); }
    let execution = Execution { group: Arc::clone(&parent.group), lanes: Lanes::new(width, Some(extra)) };
    // The root lane stays owned throughout the wave. Its caller only waits;
    // the wave's first lane represents that same reservation, not an extra one.
    items.into_par_iter().map(|item| enter(execution.clone(), &|| false,
        || crate::operations::with_context(token.clone(), || f(item))).expect("uncancellable wave lane")).collect()
}

fn wave_width() -> usize {
    #[cfg(test)] if let Some(width) = TEST_WAVE_WIDTH.with(std::cell::Cell::get) { return width; }
    crate::resources::workers()
}

pub(crate) fn interactive_waiting() -> bool {
    let scheduler = current().map(|e| Arc::clone(&e.group.scheduler)).unwrap_or_else(scheduler);
    let waiting = scheduler.state.lock().queue.iter().any(|q| q.priority == Priority::Interactive);
    waiting
}

/// Only optional builders call this, between bounded batches and without an
/// application mutex/connection statement. Foreground operations retain their
/// reservations across locks; releasing them there could introduce a cycle.
pub(crate) fn yield_background(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let Some(execution) = current() else { return if cancelled() { Err("Operação cancelada.".into()) } else { Ok(()) }; };
    if execution.group.priority != Priority::Background || !Arc::ptr_eq(&execution.lanes, &execution.group.root) { return Ok(()); }
    if cancelled() { return Err("Operação cancelada.".into()); }
    if execution.group.scheduler.state.lock().queue.is_empty() { return Ok(()); }
    let reservation = execution.group.reservation.lock().take();
    drop(reservation);
    // Admission is cancellation-aware. On cancellation the body immediately
    // returns and its RAII guards release files/lanes without further work.
    let reservation = execution.group.scheduler.acquire(Priority::Background, cancelled)?;
    *execution.group.reservation.lock() = Some(reservation);
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_SCHEDULER: RefCell<Option<Arc<Scheduler>>> = const { RefCell::new(None) };
    static TEST_WAVE_WIDTH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn with_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    struct Width(Option<usize>);
    impl Drop for Width { fn drop(&mut self) { TEST_WAVE_WIDTH.with(|w| w.set(self.0)); } }
    let _width = Width(TEST_WAVE_WIDTH.with(|w| w.replace(Some(limit.max(1)))));
    with_scheduler(Scheduler::new(limit), f)
}
#[cfg(test)]
pub(crate) fn with_scheduler<T>(scheduler: Arc<Scheduler>, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<Scheduler>>);
    impl Drop for Reset { fn drop(&mut self) { TEST_SCHEDULER.with(|s| *s.borrow_mut() = self.0.take()); } }
    let _reset = Reset(TEST_SCHEDULER.with(|s| s.replace(Some(scheduler))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    fn using<T>(scheduler: Arc<Scheduler>, f: impl FnOnce() -> T) -> T {
        struct Reset(Option<Arc<Scheduler>>);
        impl Drop for Reset { fn drop(&mut self) { TEST_SCHEDULER.with(|s| *s.borrow_mut() = self.0.take()); } }
        let _reset = Reset(TEST_SCHEDULER.with(|s| s.replace(Some(scheduler)))); f()
    }
    fn wait_queued(scheduler: &Scheduler, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while scheduler.state.lock().queue.len() != count {
            assert!(Instant::now() < deadline, "worker did not reach admission");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn operation<T>(f: impl FnOnce() -> T) -> Result<T, String> {
        crate::operations::run_with_token(crate::operations::token(None)?, f)
    }
    #[test]
    fn actual_nested_rayon_and_native_queries_share_aggregate_budget() {
        for limit in [1, 2, 4] {
            let scheduler = Scheduler::new(limit);
            let live = Arc::new(AtomicUsize::new(0)); let peak = Arc::new(AtomicUsize::new(0));
            let jobs: Vec<_> = (0..4).map(|_| {
                let scheduler = Arc::clone(&scheduler); let live = Arc::clone(&live); let peak = Arc::clone(&peak);
                std::thread::spawn(move || using(scheduler, || operation(|| {
                    map(0..4, |_| map(0..2, |_| {
                        let n = live.fetch_add(1, Ordering::SeqCst) + 1; peak.fetch_max(n, Ordering::SeqCst);
                        let conn = duckdb::Connection::open_in_memory().unwrap();
                        conn.execute_batch("SET threads=1").unwrap();
                        let sum: i64 = conn.query_row("SELECT sum(range)::BIGINT FROM range(4096)", [], |r| r.get(0)).unwrap();
                        live.fetch_sub(1, Ordering::SeqCst); sum
                    })).into_iter().flatten().sum::<i64>()
                }).unwrap()))
            }).collect();
            for job in jobs { assert_eq!(job.join().unwrap(), 8 * (4095 * 4096 / 2)); }
            assert!(peak.load(Ordering::SeqCst) <= limit, "actual CPU closures exceeded global slots");
            assert!(scheduler.state.lock().peak <= limit);
            assert_eq!(scheduler.state.lock().used, 0);
            // With no competing root, force the borrowed-wave path too.
            let nested = using(Arc::clone(&scheduler), || operation(|| map(0..4, |outer| {
                map(0..4, |inner| outer * 4 + inner)
            })).unwrap());
            assert_eq!(nested.into_iter().flatten().collect::<Vec<_>>(), (0..16).collect::<Vec<_>>());
            assert_eq!(scheduler.state.lock().used, 0);
        }
    }
    #[test]
    fn parallel_waves_preserve_case_quotas_cancellation_and_reporting_without_tls_leaks() {
        use crate::case_resources::{Policy, Preferences, Mode};
        let identity = |name: &str| crate::analysis_context::Identity {
            case_id: format!("scheduler-{name}-{}", uuid::Uuid::new_v4()), analysis_id: "analysis".into(), config_revision: 0, visibility_revision: 0,
        };
        let policy_a = Policy::capture(Some(&identity("a")), &Preferences { schema_version: 1, mode: Mode::Custom, work_limit_mib: Some(8) }).unwrap();
        let policy_b = Policy::capture(Some(&identity("b")), &Preferences { schema_version: 1, mode: Mode::Custom, work_limit_mib: Some(16) }).unwrap();
        let empty_context = crate::operations::current_token();
        let security = crate::case_security::Settings { custom_rules_json: Some("outer-scope".into()), ..Default::default() };
        crate::case_resources::with(Arc::clone(&policy_a), || crate::case_security::compiling(&security, || {
            crate::operations::with_context(empty_context, || {
                assert!(crate::case_resources::current().is_none());
                assert!(crate::case_security::with(|settings| settings.custom_rules_json.is_none()));
            });
            assert!(Arc::ptr_eq(&crate::case_resources::current().unwrap(), &policy_a));
            assert_eq!(crate::case_security::with(|settings| settings.custom_rules_json.clone()), Some("outer-scope".into()));
        }));
        let _held = policy_a.selection_pool().reserve(7 << 20).unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        pool.install(|| with_limit(2, || {
            for (policy, allowed) in [(&policy_a, false), (&policy_b, true)] {
                let id = format!("wave-context-{}", uuid::Uuid::new_v4());
                let reported = Arc::new(AtomicUsize::new(0)); let output = Arc::clone(&reported);
                let token = crate::case_resources::with(Arc::clone(policy), || crate::operations::token(Some(id.clone())).unwrap());
                let result = crate::operations::run_with_token(token, || crate::operations::with_reporter(Arc::new(move |_| { output.fetch_add(1, Ordering::SeqCst); }), || {
                    let barrier = std::sync::Barrier::new(2);
                    let workers = map(0..2, |_| {
                        barrier.wait(); // Requires two real governed Rayon lanes.
                        assert_eq!(crate::operations::current_id().as_deref(), Some(id.as_str()));
                        assert!(Arc::ptr_eq(&crate::case_resources::current().unwrap(), policy));
                        assert_eq!(crate::resources::selection_bytes(), policy.selection_bytes());
                        let credit = crate::case_resources::current_selection_pool().reserve(2 << 20);
                        assert_eq!(credit.is_ok(), allowed, "Case quota must survive the worker hop");
                        crate::operations::progress("wave", "Wave", 1, 2, 1);
                        std::thread::current().id()
                    });
                    assert_ne!(workers[0], workers[1], "test must exercise a real parallel wave");
                    assert_eq!(reported.load(Ordering::SeqCst), 2);
                    crate::operations::cancel_id(&id);
                    assert_eq!(map(0..2, |_| crate::operations::cancelled()), vec![true, true]);
                }));
                assert!(result.is_err());
                assert!(crate::operations::current_id().is_none());
                assert!(crate::case_resources::current().is_none());
            }
        }));
        pool.broadcast(|_| {
            assert!(crate::operations::current_id().is_none());
            assert!(crate::case_resources::current().is_none());
        });
    }

    #[test]
    fn waiting_operation_cancels_without_running_or_leaking_and_panic_releases() {
        let scheduler = Scheduler::new(1);
        let held = scheduler.acquire(Priority::Normal, &|| false).unwrap();
        let id = format!("scheduler-wait-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let other = Arc::clone(&scheduler);
        let waiter = std::thread::spawn(move || using(other, || crate::operations::run_with_token(token, || panic!("cancelled waiter executed"))));
        wait_queued(&scheduler, 1); crate::operations::cancel_id(&id);
        assert!(waiter.join().unwrap().is_err());
        assert!(scheduler.state.lock().queue.is_empty()); drop(held);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| using(Arc::clone(&scheduler), || operation(|| panic!("controlled root panic")))));
        assert!(result.is_err()); assert_eq!(scheduler.state.lock().used, 0);
        assert_eq!(using(Arc::clone(&scheduler), || operation(|| operation(|| 19)).unwrap().unwrap()), 19);
        assert_eq!(scheduler.state.lock().used, 0);
    }
    #[test]
    fn page_priority_and_aged_background_have_bounded_admission_order() {
        let scheduler = Scheduler::new(1);
        let held = scheduler.acquire(Priority::Normal, &|| false).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let mut jobs = Vec::new();
        for (priority, label) in [(Priority::Background, "build"), (Priority::Normal, "analysis"), (Priority::Interactive, "page")] {
            let other = Arc::clone(&scheduler); let send = send.clone();
            jobs.push(std::thread::spawn(move || { let _permit = other.acquire(priority, &|| false).unwrap(); send.send(label).unwrap(); }));
            wait_queued(&scheduler, jobs.len());
        }
        drop(held);
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), "page");
        for job in jobs { job.join().unwrap(); }
        assert_eq!(scheduler.state.lock().used, 0);
        // Deterministic aging avoids testing a race against a wall-clock sleep.
        let held = scheduler.acquire(Priority::Normal, &|| false).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let a = Arc::clone(&scheduler); let out = send.clone();
        let background = std::thread::spawn(move || { let _p = a.acquire(Priority::Background, &|| false).unwrap(); out.send("build").unwrap(); });
        wait_queued(&scheduler, 1);
        scheduler.state.lock().queue.front_mut().unwrap().since = Instant::now() - Duration::from_secs(1);
        let a = Arc::clone(&scheduler);
        let page = std::thread::spawn(move || { let _p = a.acquire(Priority::Interactive, &|| false).unwrap(); send.send("page").unwrap(); });
        wait_queued(&scheduler, 2); drop(held);
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), "build");
        background.join().unwrap(); page.join().unwrap();
    }
    #[test]
    fn stream_handoff_retains_reservation_ahead_of_writer_waiting_on_parent_lock() {
        let scheduler = Scheduler::new(1);
        let lock = Arc::new(parking_lot::RwLock::new(0));
        let done = Arc::new(AtomicBool::new(false));
        using(Arc::clone(&scheduler), || operation(|| {
            let read = lock.read();
            let token = crate::operations::current_token();
            let (send, receive) = std::sync::mpsc::sync_channel(0);
            let child = std::thread::spawn(move || crate::operations::run_with_token(token, || {
                let conn = duckdb::Connection::open_in_memory().unwrap(); conn.execute_batch("SET threads=1").unwrap();
                let value: i64 = conn.query_row("SELECT 42", [], |r| r.get(0)).unwrap();
                blocking(|| send.send(value)).unwrap();
            }));
            let other_scheduler = Arc::clone(&scheduler); let lock = Arc::clone(&lock); let writer_done = Arc::clone(&done);
            let writer = std::thread::spawn(move || using(other_scheduler, || operation(|| { *lock.write() = 1; writer_done.store(true, Ordering::SeqCst); }).unwrap()));
            wait_queued(&scheduler, 1);
            assert_eq!(blocking(|| receive.recv_timeout(Duration::from_secs(5))).unwrap(), 42);
            blocking(|| child.join()).unwrap().unwrap();
            assert!(!done.load(Ordering::SeqCst), "writer cannot steal the parent's reserved lane");
            drop(read);
            writer
        }).unwrap()).join().unwrap();
        assert!(done.load(Ordering::SeqCst)); assert_eq!(scheduler.state.lock().used, 0);
    }
    #[test]
    fn background_batch_yields_to_page_without_restarting_or_leaking() {
        let scheduler = Scheduler::new(1);
        let (send, receive) = std::sync::mpsc::channel();
        let page = using(Arc::clone(&scheduler), || run(None, Priority::Background, &|| false, || {
            let other = Arc::clone(&scheduler);
            let page = std::thread::spawn(move || using(other, || {
                let token = crate::operations::token(None).unwrap().with_priority(Priority::Interactive);
                crate::operations::run_with_token(token, || send.send("page").unwrap()).unwrap();
            }));
            wait_queued(&scheduler, 1);
            yield_background(&|| false).unwrap();
            assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), "page");
            assert_eq!(map(0..5, |n| n + 1), vec![1, 2, 3, 4, 5]); page
        }).unwrap());
        page.join().unwrap(); assert_eq!(scheduler.state.lock().used, 0);
    }
}
