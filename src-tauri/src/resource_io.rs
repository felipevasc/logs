//! Cumulative, fallible process counters. The caller restricts PIDs to the
//! application's process tree. No process discovery or app listing occurs here.
//!
//! Windows reports process read/write I/O transfers (not only physical disks).
//! Linux reports storage-layer read_bytes/write_bytes; cached reads can add zero,
//! and writes are accounted when pages are dirtied, before eventual writeback.
//! Failed, unsupported, missing or malformed readings return None, never zero.
//! Windows virtual_bytes is private commit; Linux virtual_bytes is virtual address
//! space size. Resident memory includes shared pages and is not exclusive usage.

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterSnapshot {
    pub cpu_time_ns: Option<u64>,
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    pub read_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
}

pub fn process_io(pid: u32) -> Option<(u64, u64)> {
    let counters = process_counters(pid);
    Some((counters.read_bytes?, counters.written_bytes?))
}

#[cfg(windows)]
pub fn process_counters(pid: u32) -> CounterSnapshot {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operations: u64,
        write_operations: u64,
        other_operations: u64,
        read_transfers: u64,
        write_transfers: u64,
        other_transfers: u64,
    }
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
    #[repr(C)]
    #[derive(Default)]
    struct MemoryCounters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        peak_paged_pool: usize,
        paged_pool: usize,
        peak_nonpaged_pool: usize,
        nonpaged_pool: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetProcessIoCounters(process: *mut c_void, counters: *mut IoCounters) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut MemoryCounters,
            size: u32,
        ) -> i32;
    }
    struct OwnedHandle(*mut c_void);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // The query-only handle is owned once and closed on every return path.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let mut result = CounterSnapshot::default();
    if pid == 0 {
        return result;
    }
    // OS API with a numeric PID already scoped by the caller. Neither privilege
    // escalation nor process memory reading is requested.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return result;
    }
    let handle = OwnedHandle(raw);
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let timed =
        unsafe { GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user) };
    if timed != 0 {
        result.cpu_time_ns = kernel
            .ticks()
            .checked_add(user.ticks())
            .and_then(|ticks| ticks.checked_mul(100));
    }
    let mut memory = MemoryCounters::default();
    memory.cb = std::mem::size_of::<MemoryCounters>() as u32;
    let measured = unsafe {
        GetProcessMemoryInfo(
            handle.0,
            &mut memory,
            std::mem::size_of::<MemoryCounters>() as u32,
        )
    };
    if measured != 0 {
        result.resident_bytes = Some(memory.working_set as u64);
        result.virtual_bytes = Some(memory.private_usage as u64);
    }
    let mut counters = IoCounters::default();
    let success = unsafe { GetProcessIoCounters(handle.0, &mut counters) };
    if success != 0 {
        result.read_bytes = Some(counters.read_transfers);
        result.written_bytes = Some(counters.write_transfers);
    }
    result
}

#[cfg(target_os = "linux")]
pub fn process_counters(pid: u32) -> CounterSnapshot {
    use std::sync::OnceLock;
    static UNITS: OnceLock<(Option<u64>, Option<u64>)> = OnceLock::new();
    let mut result = CounterSnapshot::default();
    if pid == 0 {
        return result;
    }
    let (ticks_per_second, page_bytes) = *UNITS.get_or_init(|| {
        // POSIX sysconf is the OS source of tick/page sizes; neither is assumed.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let pages = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        (
            (ticks > 0).then_some(ticks as u64),
            (pages > 0).then_some(pages as u64),
        )
    });
    if let Some(text) = read_proc_file(pid, "stat") {
        if let Some(stat) = parse_proc_stat(&text, pid, ticks_per_second, page_bytes) {
            result = stat;
        }
    }
    if let Some(text) = read_proc_file(pid, "statm") {
        if let Some((virtual_bytes, resident_bytes)) = parse_proc_statm(&text, page_bytes) {
            result.virtual_bytes = virtual_bytes;
            result.resident_bytes = resident_bytes;
        }
    }
    if let Some(text) = read_proc_file(pid, "io") {
        if let Some((read, written)) = parse_proc_io(&text) {
            result.read_bytes = Some(read);
            result.written_bytes = Some(written);
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn read_proc_file(pid: u32, name: &str) -> Option<String> {
    use std::io::Read;
    const MAX_PROC_BYTES: u64 = 8192;
    let file = std::fs::File::open(format!("/proc/{pid}/{name}")).ok()?;
    let mut text = String::new();
    file.take(MAX_PROC_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_PROC_BYTES {
        return None;
    }
    Some(text)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn process_counters(_pid: u32) -> CounterSnapshot {
    CounterSnapshot::default()
}

#[cfg(any(target_os = "linux", test))]
fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(
    text: &str,
    expected_pid: u32,
    ticks_per_second: Option<u64>,
    page_bytes: Option<u64>,
) -> Option<CounterSnapshot> {
    // comm can contain spaces, newlines and parentheses; the last ')' ends it.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if open >= close || decimal(text[..open].trim())? != expected_pid as u64 {
        return None;
    }
    let fields: Vec<_> = text[close + 1..].split_whitespace().collect();
    let cpu_time_ns = (|| {
        let ticks = decimal(fields.get(11)?)?.checked_add(decimal(fields.get(12)?)?)?;
        let hz = ticks_per_second.filter(|hz| *hz > 0)?;
        ((ticks as u128).checked_mul(1_000_000_000)? / hz as u128)
            .try_into()
            .ok()
    })();
    let virtual_bytes = fields.get(20).and_then(|value| decimal(value));
    let resident_bytes = fields
        .get(21)
        .and_then(|value| decimal(value))
        .and_then(|pages| pages.checked_mul(page_bytes?));
    Some(CounterSnapshot {
        cpu_time_ns,
        virtual_bytes,
        resident_bytes,
        ..CounterSnapshot::default()
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_statm(text: &str, page_bytes: Option<u64>) -> Option<(Option<u64>, Option<u64>)> {
    let mut fields = text.split_whitespace();
    let page_bytes = page_bytes?;
    if page_bytes == 0 {
        return None;
    }
    let virtual_pages = fields.next()?;
    let resident_pages = fields.next()?;
    Some((
        decimal(virtual_pages).and_then(|pages| pages.checked_mul(page_bytes)),
        decimal(resident_pages).and_then(|pages| pages.checked_mul(page_bytes)),
    ))
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_io(text: &str) -> Option<(u64, u64)> {
    let mut read = None;
    let mut written = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let target = match key {
            "read_bytes" => &mut read,
            "write_bytes" => &mut written,
            _ => continue,
        };
        // Reject duplicate keys and non-decimal, negative or overflowing values.
        let value = value.trim();
        if target.is_some() {
            return None;
        }
        *target = Some(decimal(value)?);
    }
    Some((read?, written?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_io_preserves_measured_zero_and_ignores_logical_io_counters() {
        assert_eq!(parse_proc_io("rchar: 999999\nwchar: 88888\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 4096\n"), Some((0, 4096)));
        assert_eq!(
            parse_proc_io("write_bytes: 0\nread_bytes: 0\n"),
            Some((0, 0))
        );
    }

    #[test]
    fn missing_or_malformed_proc_counters_are_unavailable() {
        for text in [
            "",
            "rchar: 10\nwchar: 20\n",
            "read_bytes: 1\n",
            "read_bytes: -1\nwrite_bytes: 2\n",
            "read_bytes: 18446744073709551616\nwrite_bytes: 2\n",
            "read_bytes: 1 bytes\nwrite_bytes: 2\n",
            "read_bytes: 1\nread_bytes: 1\nwrite_bytes: 2\n",
            "read_bytes: +1\nwrite_bytes: 2\n",
        ] {
            assert_eq!(parse_proc_io(text), None, "{text}");
        }
    }

    #[test]
    fn invalid_pid_is_unavailable() {
        assert_eq!(process_io(0), None);
    }

    #[test]
    fn process_stat_handles_parentheses_and_uses_actual_tick_and_page_units() {
        let mut fields = vec!["0"; 22];
        fields[0] = "R";
        fields[11] = "123";
        fields[12] = "27";
        fields[20] = "123456";
        fields[21] = "10";
        let text = format!("42 (worker (task) with spaces) {}", fields.join(" "));
        let counters = parse_proc_stat(&text, 42, Some(100), Some(4096)).unwrap();
        assert_eq!(counters.cpu_time_ns, Some(1_500_000_000));
        assert_eq!(counters.virtual_bytes, Some(123456));
        assert_eq!(counters.resident_bytes, Some(40960));
        assert!(parse_proc_stat(&text, 43, Some(100), Some(4096)).is_none());
        let unavailable_units = parse_proc_stat(&text, 42, None, None).unwrap();
        assert_eq!(unavailable_units.cpu_time_ns, None);
        assert_eq!(unavailable_units.resident_bytes, None);
        assert_eq!(unavailable_units.virtual_bytes, Some(123456));
    }

    #[test]
    fn malformed_statm_has_per_metric_null_and_keeps_measured_zero() {
        assert_eq!(
            parse_proc_statm("100 0 0", Some(4096)),
            Some((Some(409600), Some(0)))
        );
        assert_eq!(
            parse_proc_statm("bad 2 0", Some(4096)),
            Some((None, Some(8192)))
        );
        assert_eq!(
            parse_proc_statm("18446744073709551615 2", Some(4096)),
            Some((None, Some(8192)))
        );
        assert_eq!(parse_proc_statm("100 2", None), None);
    }

    #[test]
    #[cfg(windows)]
    fn current_process_io_is_readable_without_elevated_access() {
        assert!(process_io(std::process::id()).is_some());
    }
}
