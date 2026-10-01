//! Atomic receipts for the source that actually committed, independent of the
//! order in which asynchronous import responses reach the WebView.
use crate::{operations, sources, AppState, LoadSummary, SourceData};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Input {
    File {
        paths: Vec<String>,
        format: String,
    },
    Eventlog {
        channel: String,
        #[serde(rename = "maxEvents")]
        max_events: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Receipt {
    pub generation: u64,
    pub operation_id: Option<String>,
}

#[derive(Default)]
pub(crate) struct Publication {
    receipt: Receipt,
    sources: Vec<Input>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    #[serde(flatten)]
    pub receipt: Receipt,
    pub count: usize,
    pub columns: Vec<String>,
    pub source_desc: String,
    pub source_names: Vec<String>,
    pub sources: Vec<Input>,
}

pub(crate) fn snapshot(state: &AppState) -> Snapshot {
    // Every publisher holds source for writing while changing all three
    // pieces. Never read names/receipt before obtaining this consistency lock.
    let source = state.source.read();
    let names = state.source_names.read().clone();
    let publication = state.source_publication.read();
    let (count, columns) = match &*source {
        SourceData::None => (0, Vec::new()),
        SourceData::Memory(events) => (events.len(), crate::all_columns(events)),
        SourceData::Indexed(index) => (index.lines.len(), index.columns.clone()),
    };
    Snapshot {
        receipt: publication.receipt.clone(),
        count,
        columns,
        source_desc: names.join(" + "),
        source_names: names,
        sources: publication.sources.clone(),
    }
}

fn next(publication: &Publication) -> Result<Receipt, String> {
    Ok(Receipt {
        generation: publication
            .receipt
            .generation
            .checked_add(1)
            .ok_or("Limite de revisões da fonte atingido; reinicie o aplicativo.")?,
        operation_id: operations::current_id(),
    })
}

/// Sharing immutable source metadata is cheap. The merge operates exclusively
/// on this replacement, so append/cancellation/conversion failure cannot take
/// the currently published source or its file list away.
fn prepare_merge(
    current: &SourceData,
    incoming: sources::FileIndex,
    append: impl FnOnce(&mut sources::FileIndex, sources::FileIndex) -> Result<(), String>,
) -> Result<sources::FileIndex, String> {
    let mut previous = match current {
        SourceData::Indexed(index) => sources::FileIndex {
            parts: index.parts.clone(),
            lines: index.lines.clone(),
            columns: index.columns.clone(),
            time_order: std::sync::OnceLock::new(),
        },
        SourceData::Memory(events) => crate::workspace::index_events(events)?,
        SourceData::None => return Ok(incoming),
    };
    append(&mut previous, incoming)?;
    Ok(previous)
}

pub(crate) fn publish(
    state: &AppState,
    incoming: sources::FileIndex,
    mut names: Vec<String>,
    mut inputs: Vec<Input>,
    merge: bool,
) -> Result<LoadSummary, String> {
    let mut source = crate::source_write_checked(state)?;
    let index = if merge {
        prepare_merge(&source, incoming, |previous, incoming| {
            previous.append(incoming)?;
            operations::check()
        })?
    } else {
        incoming
    };
    operations::check()?;
    let mut publication = state.source_publication.write();
    if merge {
        let mut previous_names = state.source_names.read().clone();
        previous_names.append(&mut names);
        names = previous_names;
        let mut previous_inputs = publication.sources.clone();
        previous_inputs.append(&mut inputs);
        inputs = previous_inputs;
    }
    let receipt = next(&publication)?;
    let summary = LoadSummary {
        count: index.lines.len(),
        columns: index.columns.clone(),
        source_desc: names.join(" + "),
        publication: receipt.clone(),
    };
    // No fallible preparation remains after this boundary. A late cancel may
    // not report failure after the new source and its receipt have committed.
    operations::commit();
    crate::engine::source_published(Some(&index));
    *source = SourceData::Indexed(index);
    *state.source_names.write() = names;
    *publication = Publication {
        receipt,
        sources: inputs,
    };
    Ok(summary)
}

pub(crate) fn clear(state: &AppState) -> Result<(), String> {
    let mut source = crate::source_write_checked(state)?;
    let mut publication = state.source_publication.write();
    let receipt = next(&publication)?;
    operations::commit();
    crate::engine::source_published(None);
    *source = SourceData::None;
    state.source_names.write().clear();
    *publication = Publication {
        receipt,
        sources: Vec::new(),
    };
    crate::query::clear_match_cache();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::{Mutex, RwLock};
    use serde_json::{json, Value};
    use std::path::Path;

    fn state() -> AppState {
        AppState {
            source: RwLock::new(SourceData::None),
            source_publication: RwLock::new(Publication::default()),
            source_names: RwLock::new(Vec::new()),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        }
    }

    fn fixture(directory: &Path, name: &str, count: usize) -> sources::FileIndex {
        let path = directory.join(name);
        let data = (0..count)
            .map(|n| {
                json!({"message":format!("{name} record {n}"),"marker":name}).to_string() + "\n"
            })
            .collect::<String>();
        std::fs::write(&path, data).unwrap();
        sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap()
    }

    fn input(path: &Path, format: &str) -> Input {
        Input::File {
            paths: vec![path.to_string_lossy().into_owned()],
            format: format.into(),
        }
    }

    fn value(state: &AppState) -> Value {
        serde_json::to_value(snapshot(state)).unwrap()
    }

    fn activate(
        state: &AppState,
        index: sources::FileIndex,
        name: &str,
        inputs: Vec<Input>,
        merge: bool,
    ) -> LoadSummary {
        let id = format!("source-publication-{name}-{}", uuid::Uuid::new_v4());
        operations::run_with_token(operations::token(Some(id)).unwrap(), || {
            publish(state, index, vec![name.into()], inputs, merge)
        })
        .unwrap()
        .unwrap()
    }

    #[test]
    fn failed_imports_keep_committed_source_names_receipt_and_descriptors() {
        let directory = tempfile::tempdir().unwrap();
        struct RestoreDataDir(Option<std::ffi::OsString>);
        impl Drop for RestoreDataDir {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        let _restore = RestoreDataDir(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path().join("data"));
        let state = state();
        let index = fixture(directory.path(), "old.jsonl", 2);
        activate(
            &state,
            index,
            "old",
            vec![input(
                &directory.path().join("old.jsonl"),
                "custom:preserve-name",
            )],
            false,
        );
        let before = value(&state);
        let missing = directory
            .path()
            .join("missing.jsonl")
            .to_string_lossy()
            .into_owned();
        assert!(crate::load_file_impl(&state, &missing, "auto", Some(false), None).is_err());
        assert_eq!(value(&state), before);
        #[cfg(not(windows))]
        {
            assert!(crate::load_event_log_impl(
                &state,
                "missing-test-channel",
                1,
                Some(true),
                None
            )
            .is_err());
            assert_eq!(value(&state), before);
        }
        let _good = fixture(directory.path(), "new.jsonl", 3);
        let good = directory
            .path()
            .join("new.jsonl")
            .to_string_lossy()
            .into_owned();
        assert!(crate::load_files_impl(
            &state,
            &[good.clone(), missing.clone()],
            "auto",
            Some(true),
            None
        )
        .is_err());
        assert_eq!(value(&state), before);
        assert!(crate::workspace::load_bundle_impl(
            &state,
            vec![crate::workspace::ImportSource::File {
                paths: vec![good, missing],
                format: "auto".into()
            }],
            None
        )
        .is_err());
        assert_eq!(value(&state), before);
    }

    #[test]
    fn failed_staged_append_never_takes_ownership_of_the_live_source() {
        let directory = tempfile::tempdir().unwrap();
        let state = state();
        activate(
            &state,
            fixture(directory.path(), "old.jsonl", 2),
            "old",
            vec![input(&directory.path().join("old.jsonl"), "auto")],
            false,
        );
        let before = value(&state);
        let incoming = fixture(directory.path(), "new.jsonl", 3);
        let result = prepare_merge(&state.source.read(), incoming, |staged, incoming| {
            staged.append(incoming)?;
            Err("injected append/publication failure".into())
        });
        assert!(result.is_err());
        assert_eq!(value(&state), before);
        let source = state.source.read();
        let SourceData::Indexed(index) = &*source else {
            panic!("old source disappeared")
        };
        assert_eq!(index.parts.len(), 1);
        assert_eq!(index.lines.len(), 2);
    }

    #[test]
    fn delayed_success_then_newer_failure_reconciles_to_the_actual_commit() {
        let directory = tempfile::tempdir().unwrap();
        let state = state();
        let old = activate(
            &state,
            fixture(directory.path(), "c.jsonl", 1),
            "C",
            vec![input(&directory.path().join("c.jsonl"), "auto")],
            false,
        );
        // A committed but its response is still delayed in transport/UI work.
        let delayed = activate(
            &state,
            fixture(directory.path(), "a.jsonl", 2),
            "A",
            vec![input(&directory.path().join("a.jsonl"), "custom:nginx")],
            false,
        );
        let missing = directory
            .path()
            .join("b-missing.log")
            .to_string_lossy()
            .into_owned();
        assert!(crate::load_file_impl(&state, &missing, "auto", None, None).is_err());
        let actual = snapshot(&state);
        assert_eq!(actual.receipt, delayed.publication);
        assert!(actual.receipt.generation > old.publication.generation);
        assert_eq!(actual.count, 2);
        assert_eq!(actual.source_names, ["A"]);
        assert_eq!(
            actual.sources,
            [input(&directory.path().join("a.jsonl"), "custom:nginx")]
        );
        let latest = activate(
            &state,
            fixture(directory.path(), "d.jsonl", 3),
            "D",
            vec![input(&directory.path().join("d.jsonl"), "jsonl")],
            false,
        );
        assert_eq!(snapshot(&state).receipt, latest.publication);
        assert!(snapshot(&state).receipt.generation > delayed.publication.generation);
    }

    #[test]
    fn successful_merge_and_clear_publish_receipts_and_exact_reopen_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let state = state();
        let first_input = input(&directory.path().join("a.jsonl"), "custom:my-format");
        activate(
            &state,
            fixture(directory.path(), "a.jsonl", 2),
            "A",
            vec![first_input.clone()],
            false,
        );
        let eventlog = Input::Eventlog {
            channel: "Security".into(),
            max_events: 1234,
        };
        let merged = activate(
            &state,
            fixture(directory.path(), "eventlog.jsonl", 3),
            "Event Log: Security",
            vec![eventlog.clone()],
            true,
        );
        let actual = snapshot(&state);
        assert_eq!(actual.count, 5);
        assert_eq!(actual.sources, [first_input, eventlog]);
        assert_eq!(actual.source_names, ["A", "Event Log: Security"]);
        assert_eq!(actual.receipt, merged.publication);
        let id = format!("clear-receipt-{}", uuid::Uuid::new_v4());
        operations::run_with_token(operations::token(Some(id.clone())).unwrap(), || {
            clear(&state).unwrap();
            assert!(operations::cancel_id(&id));
        })
        .unwrap();
        let empty = snapshot(&state);
        assert_eq!(empty.receipt.generation, merged.publication.generation + 1);
        assert_eq!(empty.receipt.operation_id.as_deref(), Some(id.as_str()));
        assert_eq!(empty.count, 0);
        assert!(empty.sources.is_empty() && empty.source_names.is_empty());
    }

    #[test]
    fn cancelled_publication_preserves_receipt_as_well_as_previous_records() {
        let directory = tempfile::tempdir().unwrap();
        let state = state();
        activate(
            &state,
            fixture(directory.path(), "a.jsonl", 2),
            "A",
            vec![input(&directory.path().join("a.jsonl"), "auto")],
            false,
        );
        let before = value(&state);
        let incoming = fixture(directory.path(), "b.jsonl", 3);
        let id = format!("cancel-receipt-{}", uuid::Uuid::new_v4());
        let token = operations::token(Some(id.clone())).unwrap();
        operations::cancel_id(&id);
        assert!(operations::run_with_token(token, || publish(
            &state,
            incoming,
            vec!["B".into()],
            vec![],
            true
        ))
        .is_err());
        assert_eq!(value(&state), before);
    }

    #[test]
    fn source_snapshot_never_combines_names_or_receipt_from_another_commit() {
        let directory = tempfile::tempdir().unwrap();
        let state = state();
        let fixtures: Vec<_> = (1..=20)
            .map(|n| fixture(directory.path(), &format!("part-{n}.jsonl"), n))
            .collect();
        std::thread::scope(|threads| {
            let worker = threads.spawn(|| {
                for (n, index) in fixtures.into_iter().enumerate() {
                    let name = format!("source-{}", n + 1);
                    publish(&state, index, vec![name], vec![], false).unwrap();
                }
            });
            while !worker.is_finished() {
                let current = snapshot(&state);
                assert_eq!(current.count as u64, current.receipt.generation);
                if current.receipt.generation > 0 {
                    assert_eq!(
                        current.source_names,
                        [format!("source-{}", current.receipt.generation)]
                    );
                }
                std::thread::yield_now();
            }
            worker.join().unwrap();
        });
        assert_eq!(snapshot(&state).receipt.generation, 20);
    }
}
