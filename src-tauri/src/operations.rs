//! Cooperative cancellation shared by desktop and MCP operations.
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static GENERATION: AtomicU64 = AtomicU64::new(0);
thread_local! { static START: Cell<Option<u64>> = const { Cell::new(None) }; }

pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}
pub fn cancelled() -> bool {
    START.with(|s| s.get().is_some_and(|g| g != generation()))
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
}
pub fn run<T>(generation: u64, f: impl FnOnce() -> T) -> Result<T, String> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            START.with(|s| s.set(None));
        }
    }
    START.with(|s| s.set(Some(generation)));
    let _reset = Reset;
    check()?;
    let result = f();
    check()?;
    Ok(result)
}
