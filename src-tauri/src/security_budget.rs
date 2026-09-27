//! Shared working-memory supervision for the local detection worker.
//! It measures the native process, including SQLite/allocator overhead. Loaded
//! source data is the baseline; browser transport is bounded separately.
use parking_lot::{Mutex, MutexGuard};
use std::cell::RefCell;
const LIMIT: u64 = 256 * 1024 * 1024;
static LANE: Mutex<()> = Mutex::new(());
#[derive(Clone, Copy, Default)]
struct State {
    baseline: u64,
    peak: u64,
    ticks: usize,
    active: bool,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
pub struct Guard {
    _lane: MutexGuard<'static, ()>,
}
pub fn enter() -> Guard {
    let lane = LANE.lock();
    let baseline = memory_bytes();
    STATE.with(|s| *s.borrow_mut() = State { baseline, peak: baseline, ticks: 0, active: true });
    Guard { _lane: lane }
}
impl Drop for Guard {
    fn drop(&mut self) {
        STATE.with(|s| s.borrow_mut().active = false);
    }
}
pub fn check() -> Result<(), String> {
    STATE.with(|s|{let mut s=s.borrow_mut();if !s.active{return Ok(());}s.ticks+=1;if s.ticks%128!=0{return Ok(());}
        let used=memory_bytes();s.peak=s.peak.max(used);
        if used>s.baseline.saturating_add(LIMIT) {return Err("Orçamento global de trabalho de 256 MiB excedido (inclui SQLite e alocador nativo). Análise cancelada sem publicar resultados parciais.".into());}Ok(())})
}
pub fn snapshot() -> serde_json::Value {
    STATE.with(|s|{let mut s=s.borrow_mut();s.peak=s.peak.max(memory_bytes());serde_json::json!({"budget_bytes":LIMIT,"baseline_bytes":s.baseline,"peak_bytes":s.peak,"working_peak_bytes":s.peak.saturating_sub(s.baseline),"measured":s.baseline>0})})
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
        fn GetProcessMemoryInfo(process: *mut std::ffi::c_void, counters: *mut Counters, size: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    // OS fills a fixed C structure; no log data enters this call.
    unsafe {
        let mut counters: Counters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<Counters>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of::<Counters>() as u32) != 0 {
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
