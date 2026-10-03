//! Actual AppState vertical gate. No browser records, mocked source admission,
//! or fixture-only native producer participates in these flows.
use super::*;
use crate::{source_publication, SourceData};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};

struct Directory {
    root: tempfile::TempDir,
    inputs: tempfile::TempDir,
    exports: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}
impl Directory {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("LOGINSIGHT_DATA_DIR");
        std::env::set_var("LOGINSIGHT_DATA_DIR", root.path());
        Self {
            root,
            inputs: tempfile::tempdir().unwrap(),
            exports: tempfile::tempdir().unwrap(),
            previous,
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
            None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
        }
    }
}
fn app_state() -> AppState {
    AppState {
        source: RwLock::new(SourceData::None),
        source_publication: RwLock::new(Default::default()),
        source_names: RwLock::new(Vec::new()),
        codes: RwLock::new(Default::default()),
        system_codes: RwLock::new(Default::default()),
        derived: RwLock::new(Vec::new()),
        case_store_lock: Mutex::new(()),
        codes_path: Default::default(),
        system_codes_path: Default::default(),
    }
}
fn save(root: &Path, mutate: impl FnOnce(&mut Value)) -> evidence::PublishedSave {
    let loaded = evidence::load_view(root).unwrap();
    let expected_store = loaded.document().store.clone();
    let mut document = serde_json::to_value(&loaded).unwrap();
    drop(loaded);
    mutate(&mut document);
    evidence::save_view(
        root,
        &SaveViewRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_store,
            document_json: document.to_string(),
        },
    )
    .unwrap()
}
fn owner_and_context(root: &Path, id: &str) -> (EvidenceOwner, Identity, String, StoreStamp) {
    let loaded = evidence::load_view(root).unwrap();
    let case = loaded
        .document()
        .cases
        .iter()
        .find(|case| case["id"] == id)
        .unwrap();
    let snapshot: crate::analysis_context::Snapshot =
        serde_json::from_value(case["analysisContext"].clone()).unwrap();
    let ready = loaded
        .document()
        .case_evidence
        .iter()
        .find_map(|state| match state {
            CaseEvidenceState::Ready(ready) if ready.owner.case_id == id => Some(ready),
            _ => None,
        })
        .unwrap();
    (
        ready.owner.clone(),
        snapshot.identity(),
        ready.evidence_signature.clone(),
        loaded.document().store.clone(),
    )
}
fn open(root: &Path, id: &str) -> (Identity, NativeCaseReceipt) {
    let (_, identity, signature, store) = owner_and_context(root, id);
    let opened = open_native_case(
        root,
        &NativeCaseOpen {
            store: store.identity(),
            analysis_context: identity.clone(),
            station_id: None,
            evidence_signature: signature,
        },
    )
    .unwrap();
    let NativeCaseOpenResult::Ready { publication } = opened else {
        panic!("small verified fixture must materialize")
    };
    (identity, publication)
}
fn attach(root: &Path, case_id: &str, item_id: &str, reference: &PendingEvidenceRef) {
    save(root, |document| {
        let case = document["cases"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|case| case["id"] == case_id)
            .unwrap();
        case["items"].as_array_mut().unwrap().push(json!({"id":item_id,"label":item_id,
            "rows":{"kind":"native_evidence_container","reference":reference,"preservedCount":reference.member_count,"preview":null}}));
    });
}
fn reference(root: &Path, case_id: &str, item_id: &str) -> (StoreIdentity, EvidenceReference) {
    let loaded = evidence::load_view(root).unwrap();
    let case = loaded
        .document()
        .cases
        .iter()
        .find(|case| case["id"] == case_id)
        .unwrap();
    let item = case["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == item_id)
        .unwrap();
    (
        loaded.document().store.identity(),
        serde_json::from_value(item["rows"]["reference"].clone()).unwrap(),
    )
}

#[test]
fn new_owner_dataset_capture_case_recapture_native_analysis_and_export_are_exact() {
    let directory = Directory::new();
    let root = directory.root.path();
    let original_envelope = r#"{ "id":42,"event_ref":"legacy:42","timestamp":null,"source":"legacy","level":"info","code":"","name":"legacy original","description":"","message":"legacy message","raw":"raw","fields":{"wide":18446744073709551615,"whole":1.00,"negative":-0.0,"fraction":2.547114365375239e-8},"unknownFuture":{"keep":[1.0,"1",{"fraction":2.547114365375239e-8}]} }"#;
    let legacy = format!(
        r#"{{"active":null,"cases":[{{"id":"legacy-source","name":"Original","items":[{{"id":"original","rows":[{original_envelope}]}}]}}]}}"#
    );
    std::fs::write(root.join("cases.json"), &legacy).unwrap();
    let loaded = load_view_after_verified_adoption(root).unwrap();
    assert_eq!(loaded.document().cases.len(), 1);
    drop(loaded);
    assert_eq!(
        std::fs::read_to_string(root.join("cases.json")).unwrap(),
        legacy
    );

    let path = directory.inputs.path().join("numbers.jsonl");
    std::fs::write(&path, "{\"timestamp\":\"2024-01-01T00:00:00Z\",\"message\":\"dataset original\",\"wide_a\":18446744073709551615,\"wide_b\":18446744073709551614,\"whole\":1.0,\"negative\":-0.0,\"unknown\":{\"child\":2.547114365375239e-8}}\n").unwrap();
    // This was loaded while there was no active owner. A later first save
    // must not silently transfer that existing publication into the new Case.
    let anonymous = app_state();
    let anonymous_index = crate::index_source_file(path.to_str().unwrap(), "jsonl", None).unwrap();
    let anonymous_admission =
        analysis_runtime::capture(&anonymous, None, None, analysis_runtime::Mode::Publish).unwrap();
    let anonymous_source = analysis_runtime::with(Some(anonymous_admission), || {
        source_publication::publish(
            &anonymous,
            anonymous_index,
            vec![path.display().to_string()],
            vec![],
            false,
        )
    })
    .unwrap();

    // A new owner exists only after the actual transactional metadata save.
    let created = save(root, |document| {
        document["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"new-target","name":"New Case","items":[]}));
        document["active"] = json!("new-target");
    });
    let issued = created
        .receipt
        .analysis_contexts
        .iter()
        .find(|snapshot| snapshot.case_id == "new-target")
        .unwrap()
        .identity();
    let (target, identity, _, stamp) = owner_and_context(root, "new-target");
    assert_eq!(issued, identity);
    assert_eq!(target.analysis_id, identity.analysis_id);
    assert!(
        capture_source(
            &anonymous,
            &SourceRequest {
                analysis_context: identity.clone(),
                source_generation: Some(anonymous_source.publication.generation),
                case_key: None,
                case_content_token: None,
            }
        )
        .is_err(),
        "a first metadata save cannot borrow an anonymous Dataset publication"
    );
    let (_, empty) = open(root, "new-target");
    assert_eq!((empty.preserved_count, empty.analytical_count), (0, 0));
    drop(created);

    // Real indexed Dataset publication owns the source identity and generation.
    let state = app_state();
    let index = crate::index_source_file(path.to_str().unwrap(), "jsonl", None).unwrap();
    assert_eq!(index.lines.len(), 1);
    let original = crate::sources::event_at(
        &index,
        0,
        &state.codes.read(),
        &state.system_codes.read(),
        &[],
    );
    assert_eq!(original.fields["wide_a"].as_u64(), Some(u64::MAX));
    assert_eq!(original.fields["wide_b"].as_u64(), Some(u64::MAX - 1));
    assert!(matches!(&original.fields["whole"], Value::Number(number) if number.is_f64()));
    assert_eq!(original.fields["negative"].to_string(), "-0.0");
    let publisher = analysis_runtime::capture(
        &state,
        Some(identity.clone()),
        None,
        analysis_runtime::Mode::Publish,
    )
    .unwrap();
    let source = analysis_runtime::with(Some(publisher), || {
        source_publication::publish(
            &state,
            index,
            vec![path.display().to_string()],
            vec![source_publication::Input::File {
                paths: vec![path.display().to_string()],
                format: "jsonl".into(),
            }],
            false,
        )
    })
    .unwrap();
    let (_, other_identity, _, _) = owner_and_context(root, "legacy-source");
    assert!(
        capture_source(
            &state,
            &SourceRequest {
                analysis_context: other_identity,
                source_generation: Some(source.publication.generation),
                case_key: None,
                case_content_token: None,
            }
        )
        .is_err(),
        "a different Case cannot borrow the published Dataset identity"
    );
    let receipt = capture_source(
        &state,
        &SourceRequest {
            analysis_context: identity.clone(),
            source_generation: Some(source.publication.generation),
            case_key: None,
            case_content_token: None,
        },
    )
    .unwrap()
    .receipt;
    let pending = prepare_capture_from_source(
        &state,
        root,
        &CaptureRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_store: stamp,
            target: target.clone(),
            source: receipt,
            rows: vec![RowHandle {
                id: original.id,
                event_ref: original.event_ref.clone(),
            }],
        },
    )
    .unwrap();
    assert_eq!(pending.member_count, 1);
    attach(root, "new-target", "dataset", &pending);

    // A Case-origin recapture copies the original envelope, including unknown
    // top-level members and number spellings absent from an analytical Event.
    let (legacy_identity, legacy_publication) = open(root, "legacy-source");
    let receipt = capture_source(
        &state,
        &SourceRequest {
            analysis_context: legacy_identity,
            source_generation: None,
            case_key: Some(legacy_publication.case_key),
            case_content_token: Some(legacy_publication.case_content_token),
        },
    )
    .unwrap()
    .receipt;
    let (_, _, _, current) = owner_and_context(root, "new-target");
    let copied = prepare_capture_from_source(
        &state,
        root,
        &CaptureRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_store: current,
            target,
            source: receipt,
            rows: vec![RowHandle {
                id: 0,
                event_ref: "legacy:42".into(),
            }],
        },
    )
    .unwrap();
    attach(root, "new-target", "case-copy", &copied);
    let (store, committed) = reference(root, "new-target", "case-copy");
    let lease = evidence::open_reference(root, &store, &committed).unwrap();
    let member = lease.member_at(0).unwrap();
    assert_eq!(lease.envelope(&member).unwrap(), original_envelope);
    drop(lease);

    let (identity, publication) = open(root, "new-target");
    assert_eq!(
        (publication.preserved_count, publication.analytical_count),
        (2, 2)
    );
    let (admitted, records) = analysis_runtime::capture_case_shared(
        &state,
        Some(identity),
        None,
        None,
        Some(publication.case_key),
    )
    .unwrap();
    let records = records.unwrap();
    assert!(records.native().is_some());
    let dataset = records
        .iter()
        .find(|event| event.event_ref == original.event_ref)
        .unwrap();
    for field in ["wide_a", "wide_b", "whole", "negative", "unknown.child"] {
        assert_eq!(dataset.fields[field], original.fields[field]);
        assert_eq!(
            dataset.fields[field].to_string(),
            original.fields[field].to_string()
        );
    }
    let (cloned, _clone_credit) = records.clone_for_work().unwrap();
    let visible = analysis_runtime::with(Some(admitted.clone()), || {
        admitted.prepare_visibility(Some(cloned))
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        crate::query::count_memory(
            &visible,
            &[crate::query::Filter {
                column: "wide_a".into(),
                op: "equals".into(),
                value: u64::MAX.to_string(),
                value2: None
            }]
        ),
        1
    );
    assert_eq!(
        crate::query::count_memory(
            &visible,
            &[crate::query::Filter {
                column: "wide_a".into(),
                op: "equals".into(),
                value: (u64::MAX - 1).to_string(),
                value2: None
            }]
        ),
        0
    );
    admitted.validate_native_case_authority().unwrap();

    let (store, committed) = reference(root, "new-target", "dataset");
    let lease = evidence::open_reference(root, &store, &committed).unwrap();
    let member = lease.member_at(0).unwrap();
    let request = history::MemberRequest {
        store,
        reference: committed,
        member,
    };
    for (column, expected) in [
        ("wide_a", u64::MAX.to_string()),
        ("wide_b", (u64::MAX - 1).to_string()),
        ("whole", "1.0".into()),
        ("negative", "-0.0".into()),
        (
            "unknown.child",
            original.fields["unknown.child"].to_string(),
        ),
    ] {
        let exact = history::field_text(&lease, &request, column).unwrap();
        assert_eq!(exact.text.as_deref(), Some(expected.as_str()));
        assert!(exact.complete);
    }
    let loaded = evidence::load_view(root).unwrap();
    let path = directory.exports.path().join("exact.json");
    let exported = crate::case_portable_native::export_at(
        root,
        crate::case_portable_native::ExportRequest {
            store: loaded.document().store.clone(),
            document_json: serde_json::to_string(&loaded).unwrap(),
            path: path.display().to_string(),
            mask: false,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .unwrap();
    assert_eq!(exported.records, Some(3));
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(text.matches(original_envelope).count(), 2);
    assert!(text.contains("\"wide_a\":18446744073709551615"));
    assert!(text.contains("\"wide_b\":18446744073709551614"));
    assert!(text.contains("\"whole\":1.0"));
    assert!(text.contains("\"negative\":-0.0"));
}
