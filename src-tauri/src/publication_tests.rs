//! Publication gates are tested with a real blocked parking_lot writer, not a
//! sleep that could accidentally cancel before work reaches its write lock.
use crate::{model::Event, operations, AppState, SourceData};
use parking_lot::{Mutex, RwLock};
use std::time::{Duration, Instant};

fn state() -> AppState {
    let mut event = Event::empty();
    event.message = "original source".into();
    AppState {
        source: RwLock::new(SourceData::Memory(vec![event])),
        source_names: RwLock::new(vec!["original source".into()]),
        codes: RwLock::new(Default::default()),
        system_codes: RwLock::new(Default::default()),
        derived: RwLock::new(Vec::new()),
        case_store_lock: Mutex::new(()),
        codes_path: Default::default(),
        system_codes_path: Default::default(),
    }
}

fn wait_for_writer(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    // With this test's existing read guard still held, parking_lot refuses
    // new nonrecursive readers once the writer is queued. Cancellation below
    // therefore happens after the operation reached its blocking write lock.
    while state.source.try_read().is_some() {
        assert!(
            Instant::now() < deadline,
            "publication writer did not queue"
        );
        std::thread::yield_now();
    }
}

fn assert_original(state: &AppState) {
    let source = state.source.read();
    let SourceData::Memory(events) = &*source else {
        panic!("cancelled publication replaced the original source");
    };
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "original source");
    assert_eq!(&*state.source_names.read(), &["original source"]);
}

#[test]
fn publication_gate_rechecks_named_cancellation_after_waiting_for_readers() {
    let state = state();
    let id = format!("publication-{}", uuid::Uuid::new_v4());
    let token = operations::token(Some(id.clone())).unwrap();
    std::thread::scope(|scope| {
        let reader = state.source.read();
        let state = &state;
        let worker = scope.spawn(move || {
            operations::run_with_token(token, || {
                // This is the gate shared by file/files/event-log/bundle loads.
                let mut source = crate::source_write_checked(state)?;
                // Model the first mutation of merge/replacement. A cancelled
                // writer must never reach it, even though commit follows later.
                *source = SourceData::None;
                operations::commit();
                Ok::<(), String>(())
            })
        });
        wait_for_writer(state);
        assert!(operations::cancel_id(&id));
        drop(reader);
        assert!(worker.join().unwrap().is_err());
    });
    assert_original(&state);
}

#[test]
fn clear_waiting_for_readers_preserves_source_on_named_or_global_cancellation() {
    for cancel_all in [false, true] {
        let state = state();
        let id = format!("clear-{}", uuid::Uuid::new_v4());
        let token = operations::token(Some(id.clone())).unwrap();
        std::thread::scope(|scope| {
            let reader = state.source.read();
            let state = &state;
            let worker = scope.spawn(move || {
                operations::run_with_token(token, || crate::clear_events_impl(state))
            });
            wait_for_writer(state);
            if cancel_all {
                operations::cancel();
            } else {
                assert!(operations::cancel_id(&id));
            }
            drop(reader);
            assert!(worker.join().unwrap().is_err());
        });
        assert_original(&state);
    }
}

#[test]
fn cancelled_publication_stops_waiting_without_interrupting_the_existing_reader() {
    let state = state();
    let id = format!("waiting-publication-{}", uuid::Uuid::new_v4());
    let token = operations::token(Some(id.clone())).unwrap();
    std::thread::scope(|scope| {
        let reader = state.source.read();
        let state = &state;
        let (done, finished) = std::sync::mpsc::channel();
        let worker = scope.spawn(move || {
            let result = operations::run_with_token(token, || {
                let _source = crate::source_write_checked(state)?;
                Ok::<(), String>(())
            });
            let _ = done.send(result.is_err());
        });
        wait_for_writer(state);
        assert!(operations::cancel_id(&id));
        // Keep the unrelated reader locked until cancellation is acknowledged.
        // The timeout is only a test watchdog; synchronization is the channel.
        assert!(finished.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(&*reader, SourceData::Memory(_)));
        drop(reader);
        worker.join().unwrap();
    });
    assert_original(&state);
}

#[test]
fn completed_clear_is_not_reported_cancelled_after_its_commit_boundary() {
    let state = state();
    let token = operations::token(None).unwrap();
    let result = operations::run_with_token(token, || {
        crate::clear_events_impl(&state);
        // A late Cancel All must not change the already-completed outcome.
        operations::cancel();
    });
    assert!(result.is_ok());
    assert!(matches!(*state.source.read(), SourceData::None));
    assert!(state.source_names.read().is_empty());
}
