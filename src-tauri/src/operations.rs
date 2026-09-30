//! Cooperative cancellation shared by desktop and MCP operations.
//! Legacy generation cancellation remains available for Cancel all. Named
//! operations add isolated tokens so cancelling one query never restarts others.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

static GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static START: Cell<Option<u64>> = const { Cell::new(None) };
    static LOCAL: RefCell<Option<Arc<Local>>> = const { RefCell::new(None) };
}
struct Local {
    id: String,
    cancelled: AtomicBool,
}
static NAMED: LazyLock<Mutex<HashMap<String, Weak<Local>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

/// Register before queueing blocking work, so a quick Cancel is not lost.
pub(crate) fn token(id: Option<String>) -> Result<Cancellation, String> {
    let id = id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if id.len() > 160 {
        return Err("Identificador de operação inválido.".into());
    }
    let mut named = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    named.retain(|_, token| token.strong_count() > 0);
    if named.get(&id).and_then(Weak::upgrade).is_some() {
        return Err("Identificador de operação já está em uso.".into());
    }
    let local = Arc::new(Local {
        id: id.clone(),
        cancelled: AtomicBool::new(false),
    });
    named.insert(id, Arc::downgrade(&local));
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
    let local = NAMED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .and_then(Weak::upgrade);
    if let Some(local) = local {
        local.cancelled.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
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
    fn duplicate_live_ids_are_rejected() {
        let _a = token(Some("same-operation".into())).unwrap();
        assert!(token(Some("same-operation".into())).is_err());
    }
}
