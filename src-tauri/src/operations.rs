//! Cooperative cancellation shared by desktop and MCP operations.
//! Legacy generation cancellation remains available for Cancel all. Named
//! operations add isolated tokens so cancelling one query never restarts others.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

static GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static START: Cell<Option<u64>> = const { Cell::new(None) };
    static LOCAL: RefCell<Option<Arc<Local>>> = const { RefCell::new(None) };
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
}
impl Cancellation {
    pub(crate) fn cancelled(&self) -> bool {
        self.generation.is_some_and(|g| g != generation())
            || self
                .local
                .as_ref()
                .is_some_and(|local| local.cancelled.load(Ordering::Relaxed))
    }
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
    })
}
pub(crate) fn current_token() -> Cancellation {
    Cancellation {
        generation: current_generation(),
        local: LOCAL.with(|s| s.borrow().clone()),
    }
}
pub(crate) fn current_id() -> Option<String> {
    LOCAL.with(|s| s.borrow().as_ref().map(|s| s.id.clone()))
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
}
pub(crate) fn run_with_token<T>(token: Cancellation, f: impl FnOnce() -> T) -> Result<T, String> {
    struct Reset(Option<u64>, Option<Arc<Local>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            START.with(|s| s.set(self.0));
            LOCAL.with(|s| *s.borrow_mut() = self.1.take());
        }
    }
    let _reset = Reset(
        START.with(|s| s.replace(token.generation)),
        LOCAL.with(|s| s.replace(token.local)),
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
        },
        f,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
