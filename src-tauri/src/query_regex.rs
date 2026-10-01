//! Reuse immutable UTF-8 regex programs across ordinary query preparation.
//! No expressions, catalogs, SQL plans or match results are retained. Clones
//! share compiled state, but each caller can allocate its own search scratch.
use regex::{Regex, RegexBuilder};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

const MIB: usize = 1 << 20;

/// Every builder setting is part of the key. This module only returns the
/// UTF-8 `regex::Regex` type; byte regexes cannot enter this cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Profile {
    policy: u8,
    case_insensitive: bool,
    multi_line: bool,
    dot_matches_new_line: bool,
    crlf: bool,
    line_terminator: u8,
    swap_greed: bool,
    ignore_whitespace: bool,
    unicode: bool,
    octal: bool,
    nest_limit: u32,
    size_limit: usize,
    dfa_size_limit: usize,
}

// Preserve the locked regex 1.13.1 defaults explicitly, including syntax limits.
pub(crate) const ORDINARY: Profile = Profile {
    policy: 1,
    case_insensitive: false,
    multi_line: false,
    dot_matches_new_line: false,
    crlf: false,
    line_terminator: b'\n',
    swap_greed: false,
    ignore_whitespace: false,
    unicode: true,
    octal: false,
    nest_limit: 250,
    size_limit: 10 * MIB,
    dfa_size_limit: 2 * MIB,
};
pub(crate) const QUERY_LITERAL: Profile = Profile {
    case_insensitive: true,
    size_limit: 4 * MIB,
    ..ORDINARY
};

impl Profile {
    fn build(self, pattern: &str) -> Result<Regex, regex::Error> {
        RegexBuilder::new(pattern)
            .case_insensitive(self.case_insensitive)
            .multi_line(self.multi_line)
            .dot_matches_new_line(self.dot_matches_new_line)
            .crlf(self.crlf)
            .line_terminator(self.line_terminator)
            .swap_greed(self.swap_greed)
            .ignore_whitespace(self.ignore_whitespace)
            .unicode(self.unicode)
            .octal(self.octal)
            .nest_limit(self.nest_limit)
            .size_limit(self.size_limit)
            .dfa_size_limit(self.dfa_size_limit)
            .build()
    }

    fn credit(self) -> usize {
        // Conservative admission credits, not a measurement or an RSS bound.
        // Reserve twice the configured program + DFA limits per retained item;
        // callers, temporary compilation and library overhead are additional.
        self.size_limit.saturating_add(self.dfa_size_limit).saturating_mul(2)
    }
}

// A synchronous historical compilation scope admits work before any cache-hit
// clone or miss allocation. Ordinary callers have no scope and keep the same
// API/behavior. The first failure survives callers that discard regex errors.
type Admission = std::rc::Rc<std::cell::RefCell<CompilationAdmission>>;
struct CompilationAdmission {
    reserve: Box<dyn FnMut(usize) -> Result<(), String>>,
    failure: Option<String>,
}
thread_local! {
    static ADMISSION: std::cell::RefCell<Option<Admission>> = const { std::cell::RefCell::new(None) };
}
pub(crate) fn with_compilation_admission<T>(
    reserve: impl FnMut(usize) -> Result<(), String> + 'static,
    run: impl FnOnce() -> T,
) -> Result<T, String> {
    struct Reset(Option<Admission>);
    impl Drop for Reset {
        fn drop(&mut self) { ADMISSION.with(|slot| *slot.borrow_mut() = self.0.take()); }
    }
    let admission = std::rc::Rc::new(std::cell::RefCell::new(CompilationAdmission { reserve: Box::new(reserve), failure: None }));
    let _reset = Reset(ADMISSION.with(|slot| slot.replace(Some(admission.clone()))));
    let result = run();
    let failure = admission.borrow().failure.clone();
    match failure { Some(error) => Err(error), None => Ok(result) }
}
fn admit_compilation(pattern: &str, profile: Profile) -> Result<(), regex::Error> {
    let Some(admission) = ADMISSION.with(|slot| slot.borrow().clone()) else { return Ok(()); };
    let mut admission = admission.borrow_mut();
    if admission.failure.is_none() {
        let result = crate::operations::check().and_then(|()| {
            let bytes = pattern.len().checked_mul(4).and_then(|text| profile.credit().checked_add(text))
                .ok_or_else(|| "CASE_HISTORY_LIMIT".to_string())?;
            (admission.reserve)(bytes)
        });
        if let Err(error) = result { admission.failure = Some(error); }
    }
    if admission.failure.is_some() { Err(regex::Error::Syntax("QUERY_REGEX_ADMISSION".into())) } else { Ok(()) }
}

#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    key_bytes: usize,
    credits: usize,
}

impl Limits {
    fn for_budget(bytes: usize) -> Self {
        Self {
            entries: 64,
            key_bytes: (bytes / 2048).min(256 << 10),
            credits: (bytes / 8).min(96 * MIB),
        }
    }
}

struct Entry {
    pattern: Box<str>,
    profile: Profile,
    program: Arc<Regex>,
}

#[derive(Default)]
struct Retained {
    entries: VecDeque<Entry>,
    key_bytes: usize,
    credits: usize,
}

struct ProgramCache {
    limits: Limits,
    retained: Mutex<Retained>,
    #[cfg(test)]
    builds: std::sync::atomic::AtomicUsize,
}

impl ProgramCache {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            retained: Mutex::new(Retained::default()),
            #[cfg(test)]
            builds: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn compile(&self, pattern: &str, profile: Profile, mut cancelled: impl FnMut() -> bool) -> Result<Regex, regex::Error> {
        admit_compilation(pattern, profile)?;
        let hit = {
            let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
            retained.entries.iter().position(|entry| entry.profile == profile && entry.pattern.as_ref() == pattern)
                .map(|index| {
                    let entry = retained.entries.remove(index).unwrap();
                    let program = Arc::clone(&entry.program);
                    retained.entries.push_back(entry);
                    program
                })
        };
        if let Some(program) = hit {
            // Regex cloning may allocate caller scratch; keep it outside the lock.
            return Ok(program.as_ref().clone());
        }
        #[cfg(test)]
        self.builds.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let program = profile.build(pattern)?;
        #[cfg(test)]
        testing::after_build();
        let credit = profile.credit();
        if cancelled() || self.limits.entries == 0 || credit > self.limits.credits || pattern.len() > self.limits.key_bytes {
            // Cache policy must never turn a valid program into an error or
            // silently lower its original compilation limits.
            return Ok(program);
        }
        let candidate = Entry { pattern: pattern.into(), profile, program: Arc::new(program.clone()) };
        let mut candidate = Some(candidate);
        let mut retired = Vec::new();
        {
            let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
            // Concurrent misses may compile independently. Only one is retained.
            // Recheck cancellation at publication, including after lock contention.
            if !cancelled() && !retained.entries.iter().any(|entry| entry.profile == profile && entry.pattern.as_ref() == pattern) {
                while retained.entries.len() >= self.limits.entries
                    || retained.key_bytes.saturating_add(pattern.len()) > self.limits.key_bytes
                    || retained.credits.saturating_add(credit) > self.limits.credits
                {
                    let entry = retained.entries.pop_front().unwrap();
                    retained.key_bytes -= entry.pattern.len();
                    retained.credits -= entry.profile.credit();
                    retired.push(entry);
                }
                retained.key_bytes += pattern.len();
                retained.credits += credit;
                retained.entries.push_back(candidate.take().unwrap());
            }
        }
        // Evicted programs and losing concurrent candidates can be expensive to
        // destroy. Neither is dropped while holding the cache mutex.
        drop(retired);
        drop(candidate);
        Ok(program)
    }
}

pub(crate) fn compile(pattern: &str, profile: Profile) -> Result<Regex, regex::Error> {
    #[cfg(test)]
    if let Some(cache) = testing::current() {
        return cache.compile(pattern, profile, crate::operations::cancelled);
    }
    static CACHE: OnceLock<ProgramCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        // DuckDB gets one quarter of the configured effective resource budget.
        // Derive retention from that same budget, including a 128 MiB override.
        // At 128 MiB, 16 MiB credits retain one literal but no ordinary program.
        let bytes = crate::resources::duckdb_memory_mb().saturating_mul(4 << 20);
        ProgramCache::new(Limits::for_budget(usize::try_from(bytes).unwrap_or(usize::MAX)))
    }).compile(pattern, profile, crate::operations::cancelled)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<Option<Arc<ProgramCache>>> = const { RefCell::new(None) };
        static AFTER_BUILD: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
    }
    pub(super) fn current() -> Option<Arc<ProgramCache>> { CACHE.with(|cache| cache.borrow().clone()) }
    pub(super) fn after_build() { AFTER_BUILD.with(|hook| { if let Some(hook) = &mut *hook.borrow_mut() { hook(); } }); }
    pub(crate) fn run<T>(credits: usize, f: impl FnOnce() -> T) -> (T, usize) {
        struct Reset(Option<Arc<ProgramCache>>);
        impl Drop for Reset { fn drop(&mut self) { CACHE.with(|cache| *cache.borrow_mut() = self.0.take()); } }
        let cache = Arc::new(ProgramCache::new(Limits { credits, key_bytes: 256 << 10, entries: 64 }));
        let _reset = Reset(CACHE.with(|slot| slot.replace(Some(Arc::clone(&cache)))));
        let value = f();
        (value, cache.builds.load(std::sync::atomic::Ordering::Relaxed))
    }
    pub(crate) fn with_after_build<T>(hook: impl FnMut() + 'static, f: impl FnOnce() -> T) -> T {
        struct Reset(Option<Box<dyn FnMut()>>);
        impl Drop for Reset { fn drop(&mut self) { AFTER_BUILD.with(|hook| *hook.borrow_mut() = self.0.take()); } }
        let _reset = Reset(AFTER_BUILD.with(|slot| slot.replace(Some(Box::new(hook)))));
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn make_cache(entries: usize, credits: usize, key_bytes: usize) -> ProgramCache {
        ProgramCache::new(Limits { entries, credits, key_bytes })
    }
    fn builds(cache: &ProgramCache) -> usize { cache.builds.load(Ordering::Relaxed) }
    fn compile(cache: &ProgramCache, pattern: &str, profile: Profile) -> Regex { cache.compile(pattern, profile, || false).unwrap() }

    #[test]
    fn repeated_programs_keep_exact_flags_and_profiles() {
        let cache = make_cache(16, 256 * MIB, 4096);
        for _ in 0..4 {
            assert!(!compile(&cache, "test", ORDINARY).is_match("TEST"));
            assert!(compile(&cache, "test", QUERY_LITERAL).is_match("TEST"));
            assert!(!compile(&cache, "(?-i:test)", QUERY_LITERAL).is_match("TEST"));
        }
        assert_eq!(builds(&cache), 3);
        for profile in [
            Profile { size_limit: 9 * MIB, ..ORDINARY },
            Profile { dfa_size_limit: MIB, ..ORDINARY },
            Profile { nest_limit: 249, ..ORDINARY },
            Profile { case_insensitive: true, ..ORDINARY },
            Profile { policy: 2, ..ORDINARY },
        ] {
            compile(&cache, "test", profile);
            compile(&cache, "test", profile);
        }
        assert_eq!(builds(&cache), 8);
    }

    #[test]
    fn builder_defaults_and_invalid_errors_match_the_previous_constructors() {
        let cache = make_cache(8, 96 * MIB, 4096);
        for pattern in ["[", r"\1", "(?-u:.)", "(", "(?q:a)"] {
            for _ in 0..2 {
                assert_eq!(cache.compile(pattern, ORDINARY, || false).unwrap_err().to_string(), Regex::new(pattern).unwrap_err().to_string());
                assert_eq!(cache.compile(pattern, QUERY_LITERAL, || false).unwrap_err().to_string(), RegexBuilder::new(pattern).case_insensitive(true).size_limit(4 * MIB).build().unwrap_err().to_string());
            }
        }
        assert_eq!(builds(&cache), 20, "failed builds are never retained");
        assert!(cache.retained.lock().unwrap().entries.is_empty());
        for pattern in [r"\p{Greek}+", "(?is)^a.*z$", "(?m)^a$", "(?-i:abc)", r"a\x00b"] {
            let actual = compile(&cache, pattern, ORDINARY);
            let previous = Regex::new(pattern).unwrap();
            for text in ["αβ", "A\nz", "a\nb", "ABC", "abc", "a\0b"] {
                assert_eq!(actual.find(text), previous.find(text), "{pattern} / {text:?}");
            }
        }
    }

    #[test]
    fn lru_and_capacity_plus_one_are_bounded_without_changing_results() {
        let cache = make_cache(2, 96 * MIB, 4096);
        let held = compile(&cache, "a", ORDINARY);
        compile(&cache, "b", ORDINARY);
        compile(&cache, "a", ORDINARY); // refresh a
        compile(&cache, "c", ORDINARY); // evict b
        compile(&cache, "a", ORDINARY);
        assert_eq!(builds(&cache), 3);
        compile(&cache, "b", ORDINARY);
        assert_eq!(builds(&cache), 4);
        assert!(held.is_match("a"), "eviction cannot invalidate active callers");
        let thrashing = make_cache(2, 96 * MIB, 4096);
        for _ in 0..3 { for pattern in ["a", "b", "c"] { assert!(compile(&thrashing, pattern, ORDINARY).is_match(pattern)); } }
        assert_eq!(builds(&thrashing), 9, "capacity+one remains a disclosed LRU limitation");
        let retained = thrashing.retained.lock().unwrap();
        assert_eq!(retained.entries.len(), 2);
        assert_eq!(retained.credits, 48 * MIB);
    }

    #[test]
    fn key_credit_and_disabled_limits_compile_successfully_uncached() {
        for limits in [Limits { entries: 0, credits: 96 * MIB, key_bytes: 4096 }, Limits { entries: 8, credits: 23 * MIB, key_bytes: 4096 }, Limits { entries: 8, credits: 96 * MIB, key_bytes: 3 }] {
            let cache = ProgramCache::new(limits);
            for _ in 0..3 { assert!(compile(&cache, "hello", ORDINARY).is_match("hello")); }
            assert_eq!(builds(&cache), 3);
            assert!(cache.retained.lock().unwrap().entries.is_empty());
        }
        let credit_limited = make_cache(8, 24 * MIB, 4096);
        compile(&credit_limited, "a", ORDINARY);
        compile(&credit_limited, "b", ORDINARY);
        {
            let retained = credit_limited.retained.lock().unwrap();
            assert_eq!(retained.entries.len(), 1, "credit eviction is independent of entry count");
            assert_eq!(retained.credits, 24 * MIB);
        }
        let cache = make_cache(8, 96 * MIB, 4);
        compile(&cache, "aaa", ORDINARY);
        compile(&cache, "bb", ORDINARY);
        let retained = cache.retained.lock().unwrap();
        assert_eq!(retained.key_bytes, 2);
        assert_eq!(retained.entries.len(), 1);
    }

    #[test]
    fn small_budgets_disclose_useful_and_no_retention_edges() {
        for (credits, ordinary_builds, mixed_builds) in [(16, 4, 4), (24, 1, 2), (48, 1, 2)] {
            let ordinary = make_cache(64, credits * MIB, 4096);
            for _ in 0..4 { compile(&ordinary, "ordinary", ORDINARY); }
            assert_eq!(builds(&ordinary), ordinary_builds);
            let mixed = make_cache(64, credits * MIB, 4096);
            for _ in 0..2 { for pattern in ["literal-a", "literal-b"] { compile(&mixed, pattern, QUERY_LITERAL); } }
            assert_eq!(builds(&mixed), mixed_builds);
            eprintln!("credits={credits}MiB repeated ordinary builds={ordinary_builds}/4 mixed literal builds={mixed_builds}/4");
        }
        let low = Limits::for_budget(128 * MIB);
        assert_eq!(low.credits, 16 * MIB);
        let cache = ProgramCache::new(low);
        for _ in 0..4 { compile(&cache, "literal", QUERY_LITERAL); }
        assert_eq!(builds(&cache), 1, "the 128 MiB profile still reuses one literal");
    }

    #[test]
    fn cancellation_before_publication_never_retains_a_program() {
        let cache = make_cache(4, 96 * MIB, 4096);
        for cancelled_at in [1, 2] {
            let mut checks = 0;
            assert!(cache.compile("hello", ORDINARY, || { checks += 1; checks >= cancelled_at }).unwrap().is_match("hello"));
            assert!(cache.retained.lock().unwrap().entries.is_empty());
        }
        assert_eq!(builds(&cache), 2);
        compile(&cache, "hello", ORDINARY);
        assert_eq!(builds(&cache), 3, "a later uncancelled operation can retain it");
    }

    #[test]
    fn concurrent_misses_publish_one_entry_and_clones_search_independently() {
        let cache = Arc::new(make_cache(8, 96 * MIB, 4096));
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4).map(|_| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut first = true;
                let re = cache.compile("(?i)shared", ORDINARY, || { if first { first = false; barrier.wait(); } false }).unwrap();
                assert!(re.is_match("SHARED"));
            })
        }).collect();
        for worker in workers { worker.join().unwrap(); }
        assert_eq!(builds(&cache), 4, "simultaneous misses compile outside the lock");
        assert_eq!(cache.retained.lock().unwrap().entries.len(), 1);
        assert!(compile(&cache, "(?i)shared", ORDINARY).is_match("shared"));
        assert_eq!(builds(&cache), 4);
    }
}

#[cfg(test)]
mod compilation_admission_tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    #[test]
    fn cache_hits_require_admission_before_the_caller_clone() {
        let ((), builds) = testing::run(96 << 20, || {
            compile("cached-history", ORDINARY).unwrap();
            let called = Rc::new(Cell::new(0)); let captured = called.clone();
            let error = with_compilation_admission(move |_| { captured.set(captured.get()+1); Err("CASE_WORK_BUSY".into()) }, || compile("cached-history", ORDINARY).ok()).unwrap_err();
            assert_eq!(error, "CASE_WORK_BUSY"); assert_eq!(called.get(),1);
            assert!(compile("cached-history", ORDINARY).is_ok());
        });
        assert_eq!(builds,1);
    }
    #[test]
    fn invalid_regex_and_latched_budget_failure_remain_distinct() {
        let invalid = with_compilation_admission(|_| Ok(()), || compile("[", ORDINARY)).unwrap().unwrap_err();
        assert_eq!(invalid.to_string(), Regex::new("[").unwrap_err().to_string());
        let called = Rc::new(Cell::new(0)); let captured = called.clone();
        let denied = with_compilation_admission(move |_| {captured.set(captured.get()+1); Err("credit refused".into())}, || {
            assert!(compile("valid", ORDINARY).ok().is_none());
            assert!(compile("another", ORDINARY).ok().is_none());
            Vec::<usize>::new()
        }).unwrap_err();
        assert_eq!(denied,"credit refused"); assert_eq!(called.get(),1);
    }
    #[test]
    fn scoped_cancellation_cannot_be_swallowed_by_prepare_ok() {
        let id = format!("history-regex-{}",uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            assert!(crate::operations::cancel_id(&id));
            let failure = with_compilation_admission(|_| panic!("cancel must precede reserve"), || compile("cancelled", ORDINARY).ok()).unwrap_err();
            assert!(failure.contains("cancel"));
        });
        assert!(result.unwrap_err().contains("cancel"));
        assert!(compile("normal-after-scope", ORDINARY).is_ok());
    }
}
