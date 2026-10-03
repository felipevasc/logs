//! Command transaction coverage with the real indexed source and Case ledger.
//! The full native suite runs with --test-threads=1 because other modules also
//! change process-wide configuration. SERIAL additionally isolates these tests.
use super::*;
use crate::analysis_context;
use crate::{analysis_runtime, model::Event, sources, AppState, SourceData};
use parking_lot::{Mutex, RwLock};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn batch_summary_keeps_decoded_metadata_credit_through_serialization() {
    let fixture = Fixture::new();
    let receipt = fixture.exclude(&[0]);
    let mut batches = fixture.batches();
    assert_eq!(batches.len(), 1);
    let batch = batches.pop().unwrap();
    assert_eq!(batch.id, receipt.batch_id);
    let expected_scope = serde_json::to_string(&batch.scope).unwrap();
    let pool = crate::case_work_budget::global();
    let before = pool.used();
    let summary = BatchSummary::from(batch);
    assert_eq!(pool.used(), before);
    let wire = serde_json::to_value(&summary).unwrap();
    assert!(wire.get("_credit").is_none());
    assert_eq!(serde_json::to_string(&wire["scope"]).unwrap(), expected_scope);
    assert_eq!(pool.used(), before);
    let owned = summary._credit.bytes();
    drop(summary);
    assert_eq!(pool.used(), before - owned);
}

struct Fixture {
    state: AppState,
    originals: Vec<Event>,
    source_path: PathBuf,
    source_bytes: Vec<u8>,
    dir: tempfile::TempDir,
    previous_data: Option<std::ffi::OsString>,
    previous_engine: Option<std::ffi::OsString>,
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        crate::testkit::init_resources();
        let dir = tempfile::tempdir().unwrap();
        let previous_data = std::env::var_os("LOGINSIGHT_DATA_DIR");
        let previous_engine = std::env::var_os("LOGINSIGHT_ENGINE");
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        std::env::set_var("LOGINSIGHT_ENGINE", "0");
        *PREVIEWS.lock() = PreviewRegistry::new();
        crate::case_store::save(json!({
            "cases": [
                {"id":"a", "name":"Original investigation", "notes":"Retain these notes"},
                {"id":"b", "name":"Other investigation"}
            ],
            "active":"a"
        }))
        .unwrap();
        let source_path = dir.path().join("events.jsonl");
        let source_bytes = (0..6)
            .map(|id| {
                json!({
                    "timestamp":1700000000000i64 + id * 1000,
                    "level":if id % 2 == 0 { "ERROR" } else { "INFO" },
                    "message":format!("original event {id}"),
                    "group":if id < 3 { "A" } else { "B" }
                })
                .to_string()
                    + "\n"
            })
            .collect::<String>()
            .into_bytes();
        std::fs::write(&source_path, &source_bytes).unwrap();
        let index =
            sources::index_file(source_path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let binding = crate::analysis_visibility::SourceSet::new(&index).unwrap();
        let originals = (0..index.lines.len())
            .map(|id| {
                let mut event =
                    sources::event_at(&index, id, &Default::default(), &Default::default(), &[]);
                analysis_runtime::attach_provenance_with(&index, &binding, &mut event).unwrap();
                event
            })
            .collect();
        let state = AppState {
            source: RwLock::new(SourceData::None),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(Vec::new()),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        };
        let admitted = analysis_runtime::capture(
            &state,
            Some(analysis_context::snapshot("a").unwrap().identity()),
            Some(0),
            analysis_runtime::Mode::Publish,
        )
        .unwrap();
        analysis_runtime::with(Some(admitted), || {
            crate::source_publication::publish(&state, index, vec!["fixture".into()], vec![], false)
        })
        .unwrap();
        Self {
            state,
            originals,
            source_path,
            source_bytes,
            dir,
            previous_data,
            previous_engine,
            _serial: serial,
        }
    }

    fn identity(&self) -> Identity {
        analysis_context::snapshot("a").unwrap().identity()
    }

    fn generation(&self) -> Option<u64> {
        Some(
            crate::source_publication::snapshot(&self.state)
                .receipt
                .generation,
        )
    }

    fn prepare(&self, scope: PreviewScope) -> Result<PreviewReply, String> {
        let admitted = analysis_runtime::capture(
            &self.state,
            Some(self.identity()),
            self.generation(),
            analysis_runtime::Mode::Dataset,
        )?;
        admitted.validate(&self.state)?;
        admitted.prepare_visibility(None)?;
        analysis_runtime::with(Some(Arc::clone(&admitted)), || {
            prepare_preview(&self.state, admitted, None, scope)
        })
    }

    fn preview(&self, scope: PreviewScope) -> PreviewReply {
        operation(|| self.prepare(scope)).unwrap()
    }

    fn commit(&self, preview: &PreviewReply) -> Result<Receipt, String> {
        commit_preview(
            &self.state,
            &preview.preview_token,
            &preview.analysis_context,
            preview.source_generation,
            "Reviewed batch",
            "test reason",
        )
    }

    fn exclude(&self, ids: &[usize]) -> Receipt {
        let preview = self.preview(PreviewScope::Selected { ids: ids.to_vec() });
        operation(|| self.commit(&preview)).unwrap()
    }

    fn member(&self, id: usize) -> Member {
        let event = &self.originals[id];
        let proof = event.evidence_provenance.as_ref().unwrap();
        Member {
            key: exclusion_store::Key {
                source_key: proof.source.key().unwrap(),
                locator: proof.locator.clone(),
            },
            event_ref: event.event_ref.clone(),
        }
    }

    fn excluded_ids(&self) -> Vec<usize> {
        let view = exclusion_store::visibility(
            self.dir.path(),
            &self.identity(),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        self.originals
            .iter()
            .filter(|event| view.contains(&self.member(event.id)).unwrap())
            .map(|event| event.id)
            .collect()
    }

    fn visible_ids(&self) -> Vec<usize> {
        operation(|| {
            let admitted = analysis_runtime::capture(
                &self.state,
                Some(self.identity()),
                self.generation(),
                analysis_runtime::Mode::Dataset,
            )?;
            admitted.prepare_visibility(None)?;
            analysis_runtime::with(Some(admitted), || {
                let page = crate::query_events_impl(&self.state, vec![], "id", "asc", 0, 100)?;
                assert_eq!(page.total, page.rows.len());
                Ok(page.rows.iter().map(|event| event.id).collect())
            })
        })
        .unwrap()
    }

    fn batches(&self) -> Vec<exclusion_store::RetainedBatchInfo> {
        exclusion_store::list(self.dir.path(), &self.identity(), None, 100).unwrap()
    }

    fn archive(&self, batch: &str) -> exclusion_store::ArchivePage {
        exclusion_store::archive_page(self.dir.path(), &self.identity(), batch, None, 500, &work())
            .unwrap()
    }

    // Existing/imported ledgers can overlap even though a new ordinary preview
    // correctly rejects rows already excluded from the visible selection.
    fn seed_overlap(&self, ids: &[usize]) -> Receipt {
        operation(|| {
            let source = self.state.source.read();
            let SourceData::Indexed(index) = &*source else {
                panic!("indexed fixture")
            };
            let binding = crate::analysis_visibility::SourceSet::new(index)?;
            let staged = exclusion_store::stage(
                self.dir.path(),
                Admission {
                    analysis: self.identity(),
                    source_receipt: json!({"generation":self.generation()}),
                },
                exclusion_store::Purpose::Exclude,
                binding.descriptors(),
                ids.iter()
                    .map(|&id| binding.member_row(index, id, &Default::default())),
                &Budget::default(),
                &work(),
            )?;
            let receipt = exclusion_store::publish(
                self.dir.path(),
                staged,
                "Existing overlap",
                "",
                json!({"kind":"selected","ids":ids}),
                &Budget::default(),
                &work(),
            )?;
            crate::operations::commit();
            Ok(receipt)
        })
        .unwrap()
    }

    fn restore_batch(&self, batch: &str) -> Receipt {
        operation(|| {
            let receipt = exclusion_store::restore_batch(
                self.dir.path(),
                &Admission {
                    analysis: self.identity(),
                    source_receipt: json!({"kind":"archive","batchId":batch}),
                },
                batch,
                &work(),
            )?;
            crate::operations::commit();
            Ok(receipt)
        })
        .unwrap()
    }

    fn assert_originals_retained(&self) {
        assert_eq!(std::fs::read(&self.source_path).unwrap(), self.source_bytes);
        let source = self.state.source.read();
        let SourceData::Indexed(index) = &*source else {
            panic!("source was replaced")
        };
        assert_eq!(index.lines.len(), self.originals.len());
        for expected in &self.originals {
            let actual = sources::event_at(
                index,
                expected.id,
                &Default::default(),
                &Default::default(),
                &[],
            );
            assert_eq!(actual.event_ref, expected.event_ref);
            assert_eq!(actual.message, expected.message);
        }
        let saved = crate::case_store::load_at(self.dir.path()).unwrap();
        assert_eq!(saved["cases"][0]["name"], "Original investigation");
        assert_eq!(saved["cases"][0]["notes"], "Retain these notes");
    }

    fn pending_directories(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("exclusions-v1"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("pending-"))
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        *PREVIEWS.lock() = PreviewRegistry::new();
        for (name, previous) in [
            ("LOGINSIGHT_DATA_DIR", &self.previous_data),
            ("LOGINSIGHT_ENGINE", &self.previous_engine),
        ] {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn operation<T>(run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    crate::testkit::with_operation(&uuid::Uuid::new_v4().to_string(), |_| {}, run)?
}

fn cancel_on_phase<T>(
    phase: &'static str,
    run: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let seen = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&seen);
    let id = uuid::Uuid::new_v4().to_string();
    let cancel_id = id.clone();
    let result = crate::testkit::with_operation(
        &id,
        move |progress| {
            if progress["phaseId"] == phase {
                observed.store(true, Ordering::SeqCst);
                crate::testkit::cancel_named(&cancel_id);
            }
        },
        run,
    )
    .and_then(|result| result);
    assert!(
        seen.load(Ordering::SeqCst),
        "expected cancellation boundary {phase}"
    );
    result
}

fn filter(op: &str, value: &str) -> PreviewScope {
    PreviewScope::Filtered {
        filters: vec![crate::query::Filter {
            column: "level".into(),
            op: op.into(),
            value: value.into(),
            value2: None,
        }],
    }
}

#[test]
fn selected_and_filtered_previews_publish_exact_stable_membership() {
    let fixture = Fixture::new();
    // Standard levels are normalized for the same decoded-field comparisons
    // used by the UI: raw JSON ERROR becomes the canonical label Erro.
    assert_eq!(
        fixture
            .originals
            .iter()
            .filter(|event| event.level == "Erro")
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![0, 2, 4]
    );
    let before = fixture.identity();
    let selected = fixture.preview(PreviewScope::Selected { ids: vec![4, 1, 1] });
    assert_eq!(
        selected.selected_members, 2,
        "duplicate selected IDs count once"
    );
    assert!(
        fixture.batches().is_empty(),
        "preview cannot publish ledger metadata"
    );
    assert_eq!(fixture.identity(), before);
    assert_eq!(fixture.visible_ids(), vec![0, 1, 2, 3, 4, 5]);
    let receipt = operation(|| fixture.commit(&selected)).unwrap();
    let archive = fixture.archive(&receipt.batch_id);
    let actual: BTreeSet<_> = archive.rows.iter().map(|row| row.key.clone()).collect();
    let expected: BTreeSet<_> = [1, 4]
        .map(|id| fixture.member(id).key)
        .into_iter()
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        archive.batch.scope,
        json!({"kind":"selected","ids":[4,1,1]})
    );
    assert_eq!(
        archive.batch.source_receipt["generation"],
        json!(selected.source_generation)
    );
    assert_eq!(
        archive.batch.source_receipt["analysisContext"],
        json!(selected.analysis_context)
    );
    assert_eq!(archive.batch.label, "Reviewed batch");
    assert_eq!(archive.batch.reason, "test reason");
    let filtered = fixture.preview(filter("equals_exact", "Erro"));
    assert_eq!(
        filtered.selected_members, 2,
        "already hidden rows must not reenter filtered scope"
    );
    let filtered_receipt = operation(|| fixture.commit(&filtered)).unwrap();
    let filtered_archive = fixture.archive(&filtered_receipt.batch_id);
    assert_eq!(
        filtered_archive
            .rows
            .iter()
            .map(|row| row.key.clone())
            .collect::<BTreeSet<_>>(),
        [0, 2]
            .map(|id| fixture.member(id).key)
            .into_iter()
            .collect()
    );
    assert_eq!(fixture.excluded_ids(), vec![0, 1, 2, 4]);
    assert_eq!(fixture.visible_ids(), vec![3, 5]);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn preview_owner_generation_and_discard_protect_the_real_payload() {
    let fixture = Fixture::new();
    let preview = fixture.preview(PreviewScope::Selected { ids: vec![2] });
    let foreign = analysis_context::snapshot("b").unwrap().identity();
    assert!(operation(|| commit_preview(
        &fixture.state,
        &preview.preview_token,
        &foreign,
        preview.source_generation,
        "",
        ""
    ))
    .is_err());
    assert!(operation(|| commit_preview(
        &fixture.state,
        &preview.preview_token,
        &preview.analysis_context,
        Some(999),
        "",
        ""
    ))
    .is_err());
    assert!(!PREVIEWS
        .lock()
        .discard(&preview.preview_token, &foreign, preview.source_generation));
    assert_eq!(fixture.pending_directories(), 1);
    assert!(fixture.batches().is_empty());
    assert!(PREVIEWS.lock().discard(
        &preview.preview_token,
        &preview.analysis_context,
        preview.source_generation
    ));
    assert_eq!(fixture.pending_directories(), 0);
    assert!(operation(|| fixture.commit(&preview)).is_err());
    assert_eq!(fixture.identity().visibility_revision, 0);
    fixture.assert_originals_retained();
}

#[test]
fn repeated_successful_token_replays_one_durable_receipt() {
    let fixture = Fixture::new();
    let preview = fixture.preview(PreviewScope::Selected { ids: vec![1, 2] });
    let first = operation(|| fixture.commit(&preview)).unwrap();
    let second = operation(|| fixture.commit(&preview)).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert_eq!(fixture.batches().len(), 1);
    assert_eq!(fixture.identity().visibility_revision, 1);
    assert_eq!(fixture.excluded_ids(), vec![1, 2]);
    let durable = fixture.archive(&first.batch_id);
    assert_eq!(durable.rows.len(), 2);
    assert!(!PREVIEWS.lock().discard(
        &preview.preview_token,
        &preview.analysis_context,
        preview.source_generation
    ));
    fixture.assert_originals_retained();
}

#[test]
fn source_and_config_changes_reject_prepared_payloads_before_publication() {
    let fixture = Fixture::new();
    let source_preview = fixture.preview(PreviewScope::Selected { ids: vec![0] });
    {
        let _source = fixture.state.source.write();
        let receipt = crate::source_publication::prepare_touch_locked(&fixture.state).unwrap();
        crate::source_publication::commit_touch_locked(&fixture.state, receipt);
    }
    assert!(operation(|| fixture.commit(&source_preview)).is_err());
    assert_eq!(fixture.pending_directories(), 0);
    let config_preview = fixture.preview(PreviewScope::Selected { ids: vec![2] });
    analysis_context::update(&fixture.identity(), Default::default()).unwrap();
    assert!(operation(|| fixture.commit(&config_preview)).is_err());
    assert!(fixture.batches().is_empty());
    assert_eq!(fixture.identity().visibility_revision, 0);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn invalid_and_hidden_selections_never_publish_or_retain_pending_payloads() {
    let fixture = Fixture::new();
    assert!(operation(|| fixture.prepare(filter("unsupported", "ERROR"))).is_err());
    assert!(operation(|| fixture.prepare(PreviewScope::Selected { ids: vec![999] })).is_err());
    assert!(operation(|| fixture.prepare(PreviewScope::Selected { ids: vec![] })).is_err());
    let first = fixture.exclude(&[0]);
    assert!(operation(|| fixture.prepare(PreviewScope::Selected { ids: vec![0, 1] })).is_err());
    assert_eq!(fixture.batches().len(), 1);
    assert_eq!(fixture.identity(), first.analysis_context.identity());
    assert_eq!(fixture.pending_directories(), 0);
    assert_eq!(fixture.excluded_ids(), vec![0]);
    fixture.assert_originals_retained();
}

#[test]
fn cancellation_during_prepare_and_before_publication_retains_prior_ledger() {
    let fixture = Fixture::new();
    let prior = fixture.exclude(&[0]);
    assert!(cancel_on_phase("exclusion-stage", || fixture
        .prepare(PreviewScope::Selected { ids: vec![1, 2] }))
    .is_err());
    assert_eq!(fixture.pending_directories(), 0);
    assert_eq!(fixture.identity(), prior.analysis_context.identity());
    let pending = fixture.preview(PreviewScope::Selected { ids: vec![1, 2] });
    assert!(cancel_on_phase("exclusion-publish", || fixture.commit(&pending)).is_err());
    assert!(
        operation(|| fixture.commit(&pending)).is_err(),
        "failed tokens cannot publish on retry"
    );
    assert_eq!(fixture.identity(), prior.analysis_context.identity());
    assert_eq!(fixture.batches().len(), 1);
    assert_eq!(fixture.batches()[0].id, prior.batch_id);
    assert_eq!(fixture.excluded_ids(), vec![0]);
    assert_eq!(fixture.visible_ids(), vec![1, 2, 3, 4, 5]);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn cancellation_after_durable_publication_keeps_success_and_replay() {
    let fixture = Fixture::new();
    let pending = fixture.preview(PreviewScope::Selected { ids: vec![3] });
    let receipt = cancel_on_phase("exclusion-published", || fixture.commit(&pending)).unwrap();
    let replay = operation(|| fixture.commit(&pending)).unwrap();
    assert_eq!(
        serde_json::to_value(&receipt).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
    assert_eq!(fixture.identity(), receipt.analysis_context.identity());
    assert_eq!(fixture.batches().len(), 1);
    assert_eq!(fixture.excluded_ids(), vec![3]);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn a_case_deleted_and_recreated_cannot_publish_its_old_preview() {
    let fixture = Fixture::new();
    let pending = fixture.preview(PreviewScope::Selected { ids: vec![2] });
    let mut saved = crate::case_store::load_at(fixture.dir.path()).unwrap();
    let original_case = saved["cases"][0].clone();
    saved["cases"].as_array_mut().unwrap().remove(0);
    saved["active"] = json!("b");
    crate::case_store::save(saved).unwrap();
    assert!(analysis_context::snapshot("a").is_err());
    let mut saved = crate::case_store::load_at(fixture.dir.path()).unwrap();
    saved["cases"]
        .as_array_mut()
        .unwrap()
        .insert(0, original_case);
    saved["active"] = json!("a");
    crate::case_store::save(saved).unwrap();
    assert_ne!(
        fixture.identity().analysis_id,
        pending.analysis_context.analysis_id
    );
    assert!(operation(|| fixture.commit(&pending)).is_err());
    assert!(fixture.batches().is_empty());
    assert_eq!(fixture.identity().visibility_revision, 0);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn changed_source_bytes_reject_a_prepared_token_without_publishing() {
    use std::io::Write;
    let fixture = Fixture::new();
    let pending = fixture.preview(PreviewScope::Selected { ids: vec![2] });
    std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.source_path)
        .unwrap()
        .write_all(b"{\"message\":\"new external record\"}\n")
        .unwrap();
    assert!(operation(|| fixture.commit(&pending)).is_err());
    assert!(fixture.batches().is_empty());
    assert_eq!(fixture.identity().visibility_revision, 0);
    assert_eq!(fixture.pending_directories(), 0);
}

#[test]
fn config_race_at_the_publication_boundary_preserves_existing_batches() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new();
    let prior = fixture.exclude(&[0]);
    let pending = fixture.preview(PreviewScope::Selected { ids: vec![2] });
    let owner = fixture.identity();
    let changed = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&changed);
    let result = crate::testkit::with_operation(
        &uuid::Uuid::new_v4().to_string(),
        move |progress| {
            if progress["phaseId"] == "exclusion-publish" && !observed.swap(true, Ordering::SeqCst)
            {
                analysis_context::update(&owner, Default::default()).unwrap();
            }
        },
        || fixture.commit(&pending),
    )
    .unwrap();
    assert!(changed.load(Ordering::SeqCst));
    assert!(result.is_err());
    assert_eq!(
        fixture.identity().visibility_revision,
        prior.analysis_context.visibility_revision
    );
    assert_eq!(
        fixture.identity().config_revision,
        prior.analysis_context.config_revision + 1
    );
    assert_eq!(fixture.batches().len(), 1);
    assert_eq!(fixture.batches()[0].id, prior.batch_id);
    assert_eq!(fixture.excluded_ids(), vec![0]);
    assert_eq!(fixture.pending_directories(), 0);
    fixture.assert_originals_retained();
}

#[test]
fn archive_subset_and_whole_restore_respect_overlap_and_keep_original_rows() {
    let fixture = Fixture::new();
    let first = fixture.exclude(&[0, 1, 2]);
    let second = fixture.seed_overlap(&[2, 3]);
    assert_eq!(fixture.excluded_ids(), vec![0, 1, 2, 3]);
    let subset = operation(|| {
        restore_selected(
            fixture.identity(),
            first.batch_id.clone(),
            vec![fixture.member(0), fixture.member(2), fixture.member(2)],
        )
    })
    .unwrap();
    assert_eq!(
        subset.selected_members, 2,
        "duplicate restore members count once"
    );
    assert_eq!(
        subset.newly_visible, None,
        "selected count is not an overlap-adjusted visibility count"
    );
    assert_eq!(fixture.excluded_ids(), vec![1, 2, 3]);
    assert_eq!(fixture.visible_ids(), vec![0, 4, 5]);
    let archive = fixture.archive(&first.batch_id);
    assert_eq!(archive.rows.len(), 3);
    let restored: BTreeSet<_> = archive
        .rows
        .iter()
        .filter(|row| row.restored_from_batch)
        .map(|row| row.key.clone())
        .collect();
    assert_eq!(
        restored,
        [0, 2]
            .map(|id| fixture.member(id).key)
            .into_iter()
            .collect()
    );
    let source = analysis_runtime::capture_archive_case(
        &fixture.state,
        fixture.identity(),
        fixture.generation(),
        None,
        None,
    )
    .unwrap();
    let resolved = source.resolve(&fixture.state, &archive).unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|record| record.event.as_ref().unwrap().id)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(resolved
        .iter()
        .all(|record| record.unavailable_reason.is_none()));
    let before_invalid = fixture.identity();
    assert!(operation(|| restore_selected(
        fixture.identity(),
        first.batch_id.clone(),
        vec![fixture.member(4)]
    ))
    .is_err());
    assert!(operation(|| restore_selected(
        fixture.identity(),
        first.batch_id.clone(),
        vec![fixture.member(0)]
    ))
    .is_err());
    assert_eq!(fixture.identity(), before_invalid);
    fixture.restore_batch(&first.batch_id);
    assert_eq!(fixture.excluded_ids(), vec![2, 3]);
    let first_archive = fixture.archive(&first.batch_id);
    assert_eq!(first_archive.active_members, Some(0));
    assert!(first_archive.rows.iter().all(|row| row.restored_from_batch));
    fixture.restore_batch(&second.batch_id);
    assert!(fixture.excluded_ids().is_empty());
    assert_eq!(fixture.visible_ids(), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(
        fixture.batches().len(),
        2,
        "restoration preserves archive history"
    );
    assert!(fixture.batches().iter().all(|batch| !batch.active));
    fixture.assert_originals_retained();
}
