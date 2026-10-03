//! Explicit, reversible activation of the embedded disk-backed search engine.
use crate::{big_data::BigDataIndex, AppState, SourceData};
use serde::Serialize;
use std::sync::{atomic::Ordering, Arc};
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeStatus {
    pub enabled: bool,
    pub ready: bool,
    pub event_count: usize,
    pub index_bytes: u64,
    pub reused: bool,
    pub build_ms: u64,
    pub engine: &'static str,
}

fn status_for(enabled: bool, source: &SourceData) -> ModeStatus {
    let mut status = ModeStatus {
        enabled,
        ready: false,
        event_count: match source {
            SourceData::Indexed(idx) => idx.lines.len(),
            SourceData::Memory(events) => events.len(),
            SourceData::None => 0,
        },
        index_bytes: 0,
        reused: false,
        build_ms: 0,
        engine: "tantivy",
    };
    if let SourceData::Indexed(idx) = source {
        if let Some(index) = &idx.big_data {
            let info = index.info();
            status.ready = enabled;
            status.event_count = info.event_count;
            status.index_bytes = info.index_bytes;
            status.reused = info.reused;
            status.build_ms = info.build_ms;
        }
    }
    status
}

pub(crate) fn status_impl(state: &AppState) -> ModeStatus {
    let source = state.source.read();
    status_for(state.big_data_enabled.load(Ordering::Acquire), &source)
}

/// Settings changed the meaning of a parsed event. The next explicit activation
/// builds/reuses the matching generation; existing operations retain full behavior.
pub(crate) fn invalidate(state: &AppState) {
    let mut source = state.source.write();
    if let SourceData::Indexed(idx) = &mut *source {
        idx.big_data = None;
    }
    clear_results();
}

pub(crate) fn clear_results() {
    crate::query::clear_match_cache();
    crate::detections::clear_cache();
}

pub(crate) fn set_mode_impl(
    state: &AppState,
    enabled: bool,
    app: Option<&AppHandle>,
) -> Result<ModeStatus, String> {
    set_mode_in(
        state,
        enabled,
        app,
        &crate::config_dir().join("big-data-v1"),
    )
}

fn set_mode_in(
    state: &AppState,
    enabled: bool,
    app: Option<&AppHandle>,
    dir: &std::path::Path,
) -> Result<ModeStatus, String> {
    set_mode_with_lock(state, enabled, app, dir, || state.source.write())
}

fn set_mode_with_lock<'a>(
    state: &'a AppState,
    enabled: bool,
    app: Option<&AppHandle>,
    dir: &std::path::Path,
    lock: impl FnOnce() -> parking_lot::RwLockWriteGuard<'a, SourceData>,
) -> Result<ModeStatus, String> {
    crate::operations::check()?;
    // Use the same lock order as queries: source, catalogs, then derived fields.
    // A completed generation is published together with its source/enable flag.
    let mut source = lock();
    // Cancellation may arrive while another operation owns the source lock.
    crate::operations::check()?;
    if !enabled {
        crate::operations::check()?;
        crate::operations::commit();
        if let SourceData::Indexed(idx) = &mut *source {
            idx.big_data = None;
        }
        state.big_data_enabled.store(false, Ordering::Release);
        clear_results();
        return Ok(status_for(false, &source));
    }
    if matches!(&*source, SourceData::None) {
        crate::operations::check()?;
        crate::operations::commit();
        state.big_data_enabled.store(true, Ordering::Release);
        return Ok(status_for(true, &source));
    }
    if let SourceData::Memory(events) = &*source {
        // Indexed rows use positional IDs. Keep legacy memory sources intact if
        // their producer uses another numbering scheme; evidence IDs must survive.
        if events.is_empty()
            || events
                .iter()
                .enumerate()
                .any(|(row, event)| event.id != row)
        {
            crate::operations::check()?;
            crate::operations::commit();
            state.big_data_enabled.store(true, Ordering::Release);
            return Ok(status_for(true, &source));
        }
    }
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    let progress = |done, total| {
        crate::emit_progress(
            app,
            "bigdata",
            "Indexando Big Data",
            done,
            total,
            "eventos",
            true,
        );
    };
    crate::emit_progress(app, "bigdata", "Preparando Big Data", 0, 0, "eventos", true);
    match &mut *source {
        SourceData::Indexed(idx) => {
            let index =
                BigDataIndex::open_or_build(idx, &codes, &system, &derived, dir, Some(&progress))?;
            crate::operations::check()?;
            crate::operations::commit();
            idx.big_data = Some(Arc::new(index));
        }
        SourceData::Memory(events) => {
            let mut idx = crate::workspace::index_events(events)?;
            let index =
                BigDataIndex::open_or_build(&idx, &codes, &system, &derived, dir, Some(&progress))?;
            crate::operations::check()?;
            crate::operations::commit();
            idx.big_data = Some(Arc::new(index));
            *source = SourceData::Indexed(idx);
        }
        SourceData::None => unreachable!(),
    }
    state.big_data_enabled.store(true, Ordering::Release);
    clear_results();
    let status = status_for(true, &source);
    crate::emit_progress(
        app,
        "bigdata",
        "Big Data pronto",
        status.event_count,
        status.event_count,
        "eventos",
        false,
    );
    Ok(status)
}

#[tauri::command]
pub async fn set_big_data_mode(enabled: bool, app: AppHandle) -> Result<ModeStatus, String> {
    crate::offload(move || set_mode_impl(app.state::<AppState>().inner(), enabled, Some(&app)))
        .await?
}

#[tauri::command]
pub async fn big_data_status(app: AppHandle) -> Result<ModeStatus, String> {
    crate::offload(move || status_impl(app.state::<AppState>().inner())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState {
            source: parking_lot::RwLock::new(SourceData::None),
            big_data_enabled: std::sync::atomic::AtomicBool::new(false),
            source_names: parking_lot::RwLock::new(vec![]),
            codes: parking_lot::RwLock::new(Default::default()),
            system_codes: parking_lot::RwLock::new(Default::default()),
            derived: parking_lot::RwLock::new(vec![]),
            case_store_lock: parking_lot::Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        }
    }

    #[test]
    fn empty_corpus_toggle_preserves_readiness_and_count() {
        let state = state();
        let enabled = set_mode_impl(&state, true, None).unwrap();
        assert!(enabled.enabled);
        assert!(!enabled.ready);
        assert_eq!(enabled.event_count, 0);
        let disabled = set_mode_impl(&state, false, None).unwrap();
        assert!(!disabled.enabled);
        assert!(!disabled.ready);
        assert_eq!(status_impl(&state).index_bytes, 0);
    }

    #[test]
    fn legacy_memory_ids_are_never_renumbered() {
        let state = state();
        let mut event = crate::model::Event::empty();
        event.id = 42;
        *state.source.write() = SourceData::Memory(vec![event]);
        let mode = set_mode_impl(&state, true, None).unwrap();
        assert!(mode.enabled && !mode.ready);
        let rows = crate::query_events_impl(&state, vec![], "id", "asc", 0, 10);
        assert_eq!(rows.rows[0].id, 42);
    }

    #[test]
    fn cancellation_waiting_for_source_lock_never_changes_the_mode() {
        // The lock factory announces entry after the initial cancellation check.
        // A rendezvous channel and the held guard order cancellation/publication
        // deterministically without depending on worker timing or sleeps.
        for requested in [false, true] {
            let state = Arc::new(state());
            let previous = !requested;
            state.big_data_enabled.store(previous, Ordering::Release);
            let held = state.source.write();
            let worker_state = state.clone();
            let generation = crate::operations::generation();
            let (entered, wait_for_entry) = std::sync::mpsc::sync_channel(0);
            let worker = std::thread::spawn(move || {
                crate::operations::run(generation, || {
                    set_mode_with_lock(
                        &worker_state,
                        requested,
                        None,
                        std::path::Path::new("unused"),
                        || {
                            entered.send(()).unwrap();
                            worker_state.source.write()
                        },
                    )
                })
            });
            wait_for_entry.recv().unwrap();
            crate::operations::cancel();
            drop(held);
            assert!(worker.join().unwrap().is_err());
            assert_eq!(state.big_data_enabled.load(Ordering::Acquire), previous);
            assert!(matches!(&*state.source.read(), SourceData::None));
        }
    }

    #[test]
    fn activation_reuse_and_semantic_invalidation_keep_original_rows() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("events.jsonl");
        std::fs::write(
            &log,
            concat!(
                "{\"timestamp\":1700000000000,\"source\":\"app\",\"message\":\"alpha\"}\n",
                "{\"timestamp\":1700000001000,\"source\":\"app\",\"message\":\"beta\"}\n",
            ),
        )
        .unwrap();
        let state = state();
        *state.source.write() = SourceData::Indexed(
            crate::sources::index_file(&log.to_string_lossy(), "jsonl", None, None, None).unwrap(),
        );
        let index_dir = dir.path().join("index");
        let enabled = set_mode_in(&state, true, None, &index_dir).unwrap();
        assert!(enabled.enabled && enabled.ready);
        assert_eq!(enabled.event_count, 2);
        assert!(enabled.index_bytes > 0);
        let disabled = set_mode_in(&state, false, None, &index_dir).unwrap();
        assert!(!disabled.enabled && !disabled.ready);
        assert_eq!(disabled.event_count, 2);
        let reopened = set_mode_in(&state, true, None, &index_dir).unwrap();
        assert!(reopened.ready && reopened.reused);
        invalidate(&state);
        let stale = status_impl(&state);
        assert!(stale.enabled && !stale.ready);
        assert_eq!(stale.event_count, 2);
        let rows = crate::query_events_impl(&state, vec![], "id", "asc", 0, 10);
        assert_eq!(rows.total, 2);
        assert_eq!(rows.rows[0].message, "alpha");
        assert_eq!(rows.rows[1].message, "beta");
    }
}
