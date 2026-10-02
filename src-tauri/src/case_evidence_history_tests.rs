//! Coupled native evidence tests. Include as a test child of case_evidence so
//! setup can register synthetic authority without exposing fixture APIs to IPC.
use super::*;
use crate::{case_cache, case_evidence_history as history, model::Event};
use rusqlite::Connection;
use std::{path::Path, sync::Arc};

fn event(id: usize, reference: &str, timestamp: Option<i64>) -> Event {
    let mut event = Event::empty();
    event.id = id;
    event.event_ref = reference.into();
    event.timestamp = timestamp;
    event.message = "preserved original".into();
    event
}
fn install_items(
    root: &Path,
    request: &mut NativeCaseOpen,
    rows: Vec<(Value, Vec<String>)>,
) -> Vec<EvidenceRef> {
    let conn = Connection::open(root.join("investigations.sqlite3")).unwrap();
    let owner = EvidenceOwner {
        store_id: request.store.store_id.clone(),
        case_id: request.analysis_context.case_id.clone(),
        analysis_id: request.analysis_context.analysis_id.clone(),
    };
    let mut items = Vec::new();
    let mut bindings = Vec::new();
    let mut opened = Vec::new();
    let mut references = Vec::new();
    for (index, (mut item, rows)) in rows.into_iter().enumerate() {
        item["rows"] = Value::Null;
        items.push(item);
        let staged = stage_records(root, &owner, &Value::Null, |sink| {
            for row in &rows {
                sink.push_envelope(row)?;
            }
            Ok(())
        })
        .unwrap();
        staged.publish_files(root).unwrap();
        conn.execute(
            "INSERT INTO native_evidence_batches VALUES(?1,?2)",
            rusqlite::params![
                staged.batch.batch_id,
                serde_json::to_string(&staged.batch).unwrap()
            ],
        )
        .unwrap();
        let metadata = db::ManifestMetadata {
            reference: staged.reference.clone(),
            bytes: staged.manifest_bytes(),
            batches: vec![staged.batch.batch_id.clone()],
        };
        conn.execute(
            "INSERT INTO native_evidence_manifests VALUES(?1,?2)",
            rusqlite::params![
                staged.reference.manifest_id,
                serde_json::to_string(&metadata).unwrap()
            ],
        )
        .unwrap();
        let location = ContainerLocation::ItemRows {
            index: index as u32,
        };
        bindings.push(authority::Binding {
            location: location.clone(),
            reference: staged.reference.clone(),
        });
        references.push(staged.reference.clone());
        opened.push((
            location,
            Arc::new(
                VerifiedContainer::open(root, &staged.reference, [staged.batch.clone()]).unwrap(),
            ),
        ));
    }
    let metadata = serde_json::json!({"id":owner.case_id,"items":items});
    let case = VerifiedCase::from_parts(metadata.clone(), owner, opened).unwrap();
    let CaseEvidenceState::Ready(state) = case.evidence_state().unwrap() else {
        panic!()
    };
    conn.execute("UPDATE native_evidence_cases SET metadata=?1,bindings=?2,evidence_signature=?3 WHERE case_id=?4",rusqlite::params![metadata.to_string(),serde_json::to_string(&bindings).unwrap(),state.evidence_signature,request.analysis_context.case_id]).unwrap();
    request.evidence_signature = state.evidence_signature;
    references
}
fn scope(request: &NativeCaseOpen) -> history::TimelineRequest {
    history::TimelineRequest {
        store: request.store.clone(),
        owner: EvidenceOwner {
            store_id: request.store.store_id.clone(),
            case_id: request.analysis_context.case_id.clone(),
            analysis_id: request.analysis_context.analysis_id.clone(),
        },
        evidence_signature: request.evidence_signature.clone(),
        authored_view_json: "{\"timeline\":{},\"manual\":[]}".into(),
        station_id: None,
        filters: Vec::new(),
        from_ms: None,
        to_ms: None,
        include_untimed: true,
        cursor: None,
        aliases: Vec::new(),
    }
}
fn first_member(root: &Path, request: &NativeCaseOpen, reference: &EvidenceRef) -> MemberHandle {
    let lease = open_history(
        root,
        &request.store,
        &reference.owner,
        &request.evidence_signature,
    )
    .unwrap();
    let container = lease
        .case()
        .containers()
        .find(|(_, container)| container.reference() == reference)
        .unwrap()
        .1;
    MemberHandle {
        container_id: reference.container_id.clone(),
        manifest_id: reference.manifest_id.clone(),
        occurrence_id: container.manifest().members[0].occurrence_id.clone(),
    }
}
fn build(
    lease: Arc<CaseAuthorityLease>,
    request: &NativeCaseOpen,
) -> Result<case_cache::Records, String> {
    let (signature, count) = lease
        .case()
        .station_signature(request.station_id.as_deref())?;
    let authority = case_cache::NativeAuthority {
        store: request.store.clone(),
        owner: lease.case().owner().clone(),
        case_evidence_signature: request.evidence_signature.clone(),
        evidence_signature: signature,
        station_id: request.station_id.clone(),
        preserved_count: count,
    };
    let validate = Arc::clone(&lease);
    let traversal = Arc::clone(&lease);
    let expected = authority.clone();
    let guard = case_cache::NativeGuard {
        storage_bytes: lease.storage_bytes(),
        lease,
        validate: Arc::new(move |authority| {
            if authority.store != expected.store
                || authority.owner != expected.owner
                || authority.case_evidence_signature != expected.case_evidence_signature
                || authority.evidence_signature != expected.evidence_signature
                || authority.station_id != expected.station_id
            {
                return Err("changed authority".into());
            }
            validate.validate()
        }),
    };
    let mut builder = case_cache::NativeBuilder::new(authority, guard)?;
    let admission = builder.record_admission();
    let items = traversal
        .case()
        .metadata()
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for (location, container) in traversal.case().containers() {
        let ContainerLocation::ItemRows { index } = location else {
            continue;
        };
        let item = items.get(*index as usize).ok_or("missing item")?;
        let context = case_cache::AnalyticalItemContext {
            station_id: item.get("stationId").and_then(Value::as_str),
            artifact_id: item.get("artifactId"),
            origin: item.get("origin"),
        };
        container.visit_events(
            0..container.reference().member_count as usize,
            |member, plan| {
                admission.reserve(member, plan.encoded_bytes, plan.materialization_credit)
            },
            |member, event, _plan, credit| {
                builder.push(&context, member, event, credit)?;
                Ok(Visit::Continue)
            },
        )?;
    }
    builder.finish()
}

#[test]
fn actual_reference_history_and_publication_preserve_exact_values_and_original_occurrences() {
    let (root, mut request, _) = authority::tests::fixture();
    request.station_id = None;
    let exact = serde_json::to_string(&event(50, "same", Some(0)))
        .unwrap()
        .replace(
            "\"fields\":{}",
            "\"fields\":{\"wide\":18446744073709551615,\"float\":1.0,\"negative\":-0.0}",
        );
    let other = serde_json::to_string(&event(9000, "other", Some(20))).unwrap();
    let refs = install_items(
        root.path(),
        &mut request,
        vec![
            (
                serde_json::json!({"id":"duplicate","stationId":"A"}),
                vec![exact.clone(), exact],
            ),
            (
                serde_json::json!({"id":"duplicate","stationId":"B"}),
                vec![other],
            ),
        ],
    );
    let native = Arc::new(open_case(root.path(), &request).unwrap());
    let weak = Arc::downgrade(&native);
    let storage_bytes = native.storage_bytes();
    let records = build(native, &request).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, 0);
    assert_eq!(records[1].id, 1);
    assert_eq!(records[0].fields["wide"].as_u64(), Some(u64::MAX));
    assert_eq!(records[0].fields["float"].to_string(), "1.0");
    assert_eq!(records[0].fields["negative"].to_string(), "-0.0");
    let original = records.original_member(0, Some("same")).unwrap().clone();
    assert_eq!(original, first_member(root.path(), &request, &refs[0]));
    let publication =
        case_cache::publish_native(format!("native-evidence:{}", uuid::Uuid::new_v4()), records)
            .unwrap();
    assert_eq!(
        (publication.preserved_count, publication.analytical_count),
        (3, 2)
    );
    let (held, _) = case_cache::resolve_for_with_token(
        None,
        Some(publication.case_key.clone()),
        Some(&request.analysis_context),
    )
    .unwrap();
    let held = held.unwrap();
    let source = EvidenceReference::Committed(refs[0].clone());
    let reference = open_reference(root.path(), &request.store, &source).unwrap();
    let member_request = history::MemberRequest {
        store: request.store.clone(),
        reference: source,
        member: original,
    };
    assert_eq!(
        history::field_text(&reference, &member_request, "id")
            .unwrap()
            .text
            .as_deref(),
        Some("50")
    );
    assert_eq!(
        history::field_text(&reference, &member_request, "wide")
            .unwrap()
            .text
            .as_deref(),
        Some("18446744073709551615")
    );
    let mut historical = scope(&request);
    historical.aliases = vec!["e:duplicate:same".into()];
    let history_lease = open_history(
        root.path(),
        &request.store,
        &historical.owner,
        &historical.evidence_signature,
    )
    .unwrap();
    let page = history::timeline(&history_lease, &historical).unwrap();
    assert_eq!(page.scope_count, 3);
    assert_eq!(page.entries.len(), 3);
    assert_eq!(page.aliases[0].state, "ambiguous");
    assert_ne!(
        page.entries[0].member.occurrence_id,
        page.entries[1].member.occurrence_id
    );
    for index in 0..3 {
        case_cache::store(
            format!("coupled-evict-{}-{index}", uuid::Uuid::new_v4()),
            vec![],
        )
        .unwrap();
    }
    assert!(
        weak.upgrade().is_some(),
        "eviction must keep a live analytical consumer's metadata charged"
    );
    assert!(crate::case_work_budget::global().used() >= storage_bytes + held.accounted_bytes());
    drop(held);
    assert!(weak.upgrade().is_none());
}

#[test]
fn committed_member_survives_membership_replacement_and_history_ignores_current_config() {
    let (root, mut request, _) = authority::tests::fixture();
    let refs = install_items(
        root.path(),
        &mut request,
        vec![(
            serde_json::json!({"id":"old","stationId":"A"}),
            vec![serde_json::to_string(&event(77, "old", Some(100))).unwrap()],
        )],
    );
    let member = first_member(root.path(), &request, &refs[0]);
    let source = EvidenceReference::Committed(refs[0].clone());
    let old = open_reference(root.path(), &request.store, &source).unwrap();
    let original = history::MemberRequest {
        store: request.store.clone(),
        reference: source,
        member,
    };
    install_items(root.path(), &mut request, vec![]);
    let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
    let mut changed = request.analysis_context.clone();
    changed.config_revision += 1;
    changed.visibility_revision += 1;
    conn.execute(
        "UPDATE case_analysis SET body=?1 WHERE case_id=?2",
        rusqlite::params![serde_json::to_string(&changed).unwrap(), changed.case_id],
    )
    .unwrap();
    conn.execute("UPDATE metadata SET value='999' WHERE key='revision'", [])
        .unwrap();
    assert_eq!(
        history::field_text(&old, &original, "id")
            .unwrap()
            .text
            .as_deref(),
        Some("77")
    );
    let historical = scope(&request);
    let current = open_history(
        root.path(),
        &request.store,
        &historical.owner,
        &historical.evidence_signature,
    )
    .unwrap();
    let empty = history::timeline(&current, &historical).unwrap();
    assert_eq!(empty.preserved_count, 0);
    assert!(empty.entries.is_empty());
    let mut wrong = original.clone();
    wrong.member.manifest_id = uuid::Uuid::new_v4().to_string();
    assert!(history::field_text(&old, &wrong, "id").is_err());
}

#[test]
fn actual_epoch_change_between_read_and_final_validation_refuses_member_output() {
    let (root, mut request, _) = authority::tests::fixture();
    let refs = install_items(
        root.path(),
        &mut request,
        vec![(
            serde_json::json!({"id":"i","stationId":"A"}),
            vec![serde_json::to_string(&event(1, "one", Some(1))).unwrap()],
        )],
    );
    let member = first_member(root.path(), &request, &refs[0]);
    let reference = EvidenceReference::Committed(refs[0].clone());
    let lease = open_reference(root.path(), &request.store, &reference).unwrap();
    struct Changing {
        lease: ReferenceAuthorityLease,
        root: std::path::PathBuf,
    }
    impl history::MemberAuthority for Changing {
        fn validate(&self, request: &history::MemberRequest) -> Result<(), String> {
            history::MemberAuthority::validate(&self.lease, request)
        }
        fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
            let original = self.lease.envelope(member)?;
            let mut conn = Connection::open(self.root.join("investigations.sqlite3")).unwrap();
            let tx = conn.transaction().unwrap();
            restore_epoch(&tx, &uuid::Uuid::new_v4().to_string()).unwrap();
            tx.commit().unwrap();
            Ok(original)
        }
    }
    let request = history::MemberRequest {
        store: request.store,
        reference,
        member,
    };
    let changing = Changing {
        lease,
        root: root.path().to_path_buf(),
    };
    assert!(history::field_text(&changing, &request, "message").is_err());
    assert!(open_reference(root.path(), &request.store, &request.reference).is_err());
}

#[test]
fn prepared_member_is_exact_only_while_the_issued_token_remains_live() {
    let (root, request, refs) = authority::tests::fixture();
    let row = event(17, "prepared", Some(0));
    let expected_store =
        db::stamp(&Connection::open(root.path().join("investigations.sqlite3")).unwrap()).unwrap();
    let source=serde_json::from_value(serde_json::json!({"analysisContext":request.analysis_context,"sourceGeneration":7,"caseKey":null,"caseContentToken":null,"catalogSignature":"a".repeat(64),"catalogEpoch":1})).unwrap();
    let capture = CaptureRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        expected_store,
        target: refs[0].owner.clone(),
        source,
        rows: vec![crate::page_projection::RowHandle {
            id: 17,
            event_ref: "prepared".into(),
        }],
    };
    let pending = prepare_capture(
        root.path(),
        &capture,
        |sink| sink.push_event(&row),
        || Ok(()),
    )
    .unwrap();
    let pinned = prepare::pin(root.path(), &pending).unwrap();
    let member = MemberHandle {
        container_id: pending.container_id.clone(),
        manifest_id: pending.manifest_id.clone(),
        occurrence_id: pinned.manifest().members[0].occurrence_id.clone(),
    };
    drop(pinned);
    let reference = EvidenceReference::Prepared(pending.clone());
    let lease = open_reference(root.path(), &request.store, &reference).unwrap();
    let member_request = history::MemberRequest {
        store: request.store.clone(),
        reference,
        member,
    };
    assert_eq!(
        history::field_text(&lease, &member_request, "id")
            .unwrap()
            .text
            .as_deref(),
        Some("17")
    );
    assert!(
        discard_prepared(
            root.path(),
            &DiscardRequest {
                store: request.store,
                reference: pending
            }
        )
        .unwrap()
        .discarded
    );
    assert!(history::detail(&lease, &member_request, None).is_err());
}

#[test]
fn real_root_quiescence_and_payload_change_are_visible_to_history_leases() {
    use fs2::FileExt;
    let (root, request, refs) = authority::tests::fixture();
    let reference = EvidenceReference::Committed(refs[1].clone());
    let lease = open_reference(root.path(), &request.store, &reference).unwrap();
    let member = first_member(root.path(), &request, &refs[1]);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("case-recovery-root.lock"))
        .unwrap();
    FileExt::try_lock_exclusive(&lock).unwrap();
    assert!(lease.validate().unwrap_err().contains("BUSY"));
    drop(lock);
    lease.validate().unwrap();
    let request = history::MemberRequest {
        store: request.store,
        reference,
        member,
    };
    assert!(history::detail(&lease, &request, None).is_ok());
    std::fs::write(
        root.path()
            .join("evidence-v1")
            .join(format!("{}.manifest", refs[1].manifest_id)),
        b"changed",
    )
    .unwrap();
    assert!(history::detail(&lease, &request, None).is_err());
}

#[test]
fn low_profile_keeps_large_capture_and_declines_oversized_analytical_publication() {
    if crate::case_work_budget::global().limits().materialized != 16 << 20 {
        assert!(
            std::env::var_os("LOGINSIGHT_HISTORY_LOW_PROFILE_CHILD").is_none(),
            "the isolated low profile must be effective before pool initialization"
        );
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_, module)| module);
        let name = format!("{module}::low_profile_keeps_large_capture_and_declines_oversized_analytical_publication");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(name)
            .arg("--exact")
            .arg("--test-threads=1")
            .env("LOGINSIGHT_MEMORY_LIMIT_MB", "128")
            .env("LOGINSIGHT_HISTORY_LOW_PROFILE_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    assert_eq!(crate::case_work_budget::global().limits().live, 64 << 20);
    let (root, mut request, _) = authority::tests::fixture();
    request.station_id = None;
    let mut row = event(0, "", Some(1));
    row.message = "x".repeat(700 << 10);
    let rows = (0..30)
        .map(|index| {
            row.id = index;
            row.event_ref = format!("large:{index}");
            serde_json::to_string(&row).unwrap()
        })
        .collect::<Vec<_>>();
    let refs = install_items(
        root.path(),
        &mut request,
        vec![(serde_json::json!({"id":"large","stationId":"A"}), rows)],
    );
    let original_count = refs[0].member_count;
    let lease = Arc::new(open_case(root.path(), &request).unwrap());
    let error = build(lease, &request).unwrap_err();
    assert_eq!(error, crate::case_work_budget::MATERIALIZATION_LIMIT);
    let preserved = open_history(
        root.path(),
        &request.store,
        &refs[0].owner,
        &request.evidence_signature,
    )
    .unwrap();
    let (signature, count) = preserved.case().station_signature(None).unwrap();
    assert!(!signature.is_empty());
    assert_eq!(count, original_count);
    assert_eq!(count, 30);
    let member = first_member(root.path(), &request, &refs[0]);
    let reference = EvidenceReference::Committed(refs[0].clone());
    let single = open_reference(root.path(), &request.store, &reference).unwrap();
    let request = history::MemberRequest {
        store: request.store,
        reference,
        member,
    };
    let detail = history::detail(&single, &request, None).unwrap();
    assert!(detail
        .fields
        .iter()
        .any(|field| field.column == "message" && !field.complete));
}

#[test]
fn authoritative_role_actions_use_unclipped_native_values_and_keep_literal_null_fields() {
    let (root, mut request, _) = authority::tests::fixture();
    let full = format!("{}Z", "a".repeat(4000));
    let mut row = event(1, "roles", Some(1));
    row.fields
        .insert("user_agent".into(), Value::String(full.clone()));
    row.fields.insert("literal_null".into(), Value::Null);
    let mut legacy_preview = row.clone();
    crate::entities::annotate(&mut legacy_preview);
    assert_eq!(
        legacy_preview.fields["@user_agent"].as_str().unwrap().len(),
        4000
    );
    assert_eq!(row.col_ref("@user_agent").unwrap(), full);
    let refs = install_items(
        root.path(),
        &mut request,
        vec![(
            serde_json::json!({"id":"roles","stationId":"A"}),
            vec![serde_json::to_string(&row).unwrap()],
        )],
    );
    let member = first_member(root.path(), &request, &refs[0]);
    let reference = EvidenceReference::Committed(refs[0].clone());
    let lease = open_reference(root.path(), &request.store, &reference).unwrap();
    let request = history::MemberRequest {
        store: request.store,
        reference,
        member,
    };
    let role = history::field_text(&lease, &request, "@user_agent").unwrap();
    assert_eq!(role.text.as_deref(), Some(full.as_str()));
    assert!(role.complete);
    let null = history::field_text(&lease, &request, "literal_null").unwrap();
    assert!(null.present);
    assert_eq!(null.value_type, Some("null"));
    assert_eq!(null.text.as_deref(), Some("null"));
    let missing = history::field_text(&lease, &request, "absent").unwrap();
    assert!(!missing.present);
    assert_eq!(missing.text, None);
}

#[test]
fn missing_id_alias_uses_immutable_origin_position_after_membership_removal() {
    use sha2::{Digest, Sha256};
    let (root, mut request, _) = authority::tests::fixture();
    let rows = (0..3)
        .map(|index| {
            let mut raw = serde_json::to_value(event(index, "", Some(index as i64))).unwrap();
            raw.as_object_mut().unwrap().remove("id");
            serde_json::to_string(&raw).unwrap()
        })
        .collect::<Vec<_>>();
    let refs = install_items(
        root.path(),
        &mut request,
        vec![(serde_json::json!({"id":"anonymous","stationId":"A"}), rows)],
    );
    let analytical = open_case(root.path(), &request).unwrap();
    assert!(build(Arc::new(analytical), &request)
        .unwrap_err()
        .contains("UNSUPPORTED_RECORD"));
    let mut query = scope(&request);
    query.aliases = vec!["e:anonymous:0".into(), "e:anonymous:2".into()];
    let lease = open_history(
        root.path(),
        &request.store,
        &query.owner,
        &query.evidence_signature,
    )
    .unwrap();
    let before = history::timeline(&lease, &query).unwrap();
    let old_target = before.aliases[1].entry_id.clone().unwrap();
    assert_eq!(before.aliases[0].state, "unique");
    let container = lease.case().containers().next().unwrap().1;
    let mut manifest = container.manifest().clone();
    assert_eq!(
        manifest
            .members
            .iter()
            .map(|member| member.origin_position)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    manifest
        .members
        .retain(|member| member.origin_position == 2);
    manifest.manifest_id = uuid::Uuid::new_v4().to_string();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(
        root.path()
            .join("evidence-v1")
            .join(format!("{}.manifest", manifest.manifest_id)),
        &bytes,
    )
    .unwrap();
    let mut reference = refs[0].clone();
    reference.manifest_id = manifest.manifest_id.clone();
    reference.manifest_sha256 = format!("{:x}", Sha256::digest(&bytes));
    reference.member_count = 1;
    let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
    let prior = authority::manifest(&conn, &refs[0]).unwrap();
    let metadata = db::ManifestMetadata {
        reference: reference.clone(),
        bytes: bytes.len() as u64,
        batches: prior.batches.clone(),
    };
    conn.execute(
        "INSERT INTO native_evidence_manifests VALUES(?1,?2)",
        rusqlite::params![
            reference.manifest_id,
            serde_json::to_string(&metadata).unwrap()
        ],
    )
    .unwrap();
    let batches = prior
        .batches
        .iter()
        .map(|id| authority::batch(&conn, id).unwrap())
        .collect::<Vec<_>>();
    let replacement = Arc::new(VerifiedContainer::open(root.path(), &reference, batches).unwrap());
    let changed = VerifiedCase::from_parts(
        lease.case().metadata().clone(),
        query.owner.clone(),
        [(ContainerLocation::ItemRows { index: 0 }, replacement)],
    )
    .unwrap();
    let CaseEvidenceState::Ready(state) = changed.evidence_state().unwrap() else {
        panic!()
    };
    let binding = authority::Binding {
        location: ContainerLocation::ItemRows { index: 0 },
        reference,
    };
    conn.execute(
        "UPDATE native_evidence_cases SET bindings=?1,evidence_signature=?2 WHERE case_id=?3",
        rusqlite::params![
            serde_json::to_string(&[binding]).unwrap(),
            state.evidence_signature,
            query.owner.case_id
        ],
    )
    .unwrap();
    assert!(lease.validate().is_err());
    request.evidence_signature = state.evidence_signature;
    query.evidence_signature = request.evidence_signature.clone();
    let changed = open_history(
        root.path(),
        &request.store,
        &query.owner,
        &query.evidence_signature,
    )
    .unwrap();
    let after = history::timeline(&changed, &query).unwrap();
    assert_eq!(after.preserved_count, 1);
    assert_eq!(after.entries[0].original_row_index, 2);
    assert_eq!(after.aliases[0].state, "missing");
    assert_eq!(after.aliases[0].entry_id, None);
    assert_eq!(after.aliases[1].state, "unique");
    assert_eq!(
        after.aliases[1].entry_id.as_deref(),
        Some(old_target.as_str())
    );
    assert_eq!(after.entries[0].entry_id, old_target);
}

#[test]
fn actual_member_lookup_uses_durable_source_proof_and_exact_container_ownership() {
    use crate::{
        analysis_visibility::EvidenceProvenance,
        case_evidence_members as members,
        exclusion_store::{Locator, SourceDescriptor},
    };
    let (root, mut open, _) = authority::tests::fixture();
    let provenance = EvidenceProvenance {
        source: SourceDescriptor {
            version: "source-version-1".into(),
            record_space: "parser-v1".into(),
            label: "file".into(),
            event_ref_prefix: Some("durable".into()),
        },
        locator: Locator::ByteOffset(12),
    };
    let mut one = event(99, "durable:12", Some(10));
    one.evidence_provenance = Some(provenance.clone());
    let mut two = one.clone();
    two.id = 100;
    two.message = "different saved catalog/overlay".into();
    let refs = install_items(
        root.path(),
        &mut open,
        vec![
            (
                serde_json::json!({"id":"duplicate","stationId":"one"}),
                vec![serde_json::to_string(&one).unwrap()],
            ),
            (
                serde_json::json!({"id":"duplicate","stationId":"two"}),
                vec![serde_json::to_string(&two).unwrap()],
            ),
        ],
    );
    let lease = open_history(
        root.path(),
        &open.store,
        &refs[0].owner,
        &open.evidence_signature,
    )
    .unwrap();
    let mut request=members::FindMembersRequest {
        store:open.store.clone(),owner:refs[0].owner.clone(),case_evidence_signature:open.evidence_signature.clone(),
        source:serde_json::from_value(serde_json::json!({"analysisContext":{"caseId":"source-case","analysisId":uuid::Uuid::new_v4().to_string(),"configRevision":0,"visibilityRevision":0},"sourceGeneration":1,"caseKey":null,"caseContentToken":null,"catalogSignature":"a".repeat(64),"catalogEpoch":0})).unwrap(),
        rows:vec![crate::page_projection::RowHandle{id:17,event_ref:"durable:12".into()}],station_id:None,
    };
    let result = members::find_members(&lease, &request, &mut |_| {
        members::FindMemberProof::provenance("durable:12", &provenance)
    })
    .unwrap();
    assert_eq!(result.rows[0].state, "ambiguous");
    assert_eq!(result.rows[0].matches.len(), 2);
    assert_eq!(result.rows[0].matches[0].container_id, refs[0].container_id);
    assert_eq!(result.rows[0].matches[1].container_id, refs[1].container_id);
    assert_eq!(result.rows[0].matches[0].item_index, 0);
    assert_eq!(result.rows[0].matches[1].item_index, 1);
    assert_ne!(
        result.rows[0].matches[0].member,
        result.rows[0].matches[1].member
    );
    let original = result.rows[0].matches[1].member.clone();
    drop(result);
    let exact = members::find_members(&lease, &request, &mut |_| {
        members::FindMemberProof::member(&original)
    })
    .unwrap();
    assert_eq!(exact.rows[0].state, "unique");
    assert_eq!(exact.rows[0].matches[0].item_index, 1);
    drop(exact);
    request.station_id = Some("one".into());
    let station = members::find_members(&lease, &request, &mut |_| {
        members::FindMemberProof::provenance("durable:12", &provenance)
    })
    .unwrap();
    assert_eq!(station.rows[0].state, "unique");
    assert_eq!(
        station.rows[0].matches[0].container_id,
        refs[0].container_id
    );
    drop(station);
    request.store.epoch = uuid::Uuid::new_v4().to_string();
    assert!(members::find_members(&lease, &request, &mut |_| panic!(
        "stale authority must precede source resolution"
    ))
    .is_err());
}

#[test]
fn actual_committed_and_pending_previews_keep_ordered_members_and_revocable_tokens() {
    let (root, mut open, _) = authority::tests::fixture();
    let rows = (0..3)
        .map(|id| {
            let mut row = event(id, &format!("row:{id}"), Some(id as i64));
            row.fields
                .insert("wide".into(), serde_json::json!(u64::MAX));
            serde_json::to_string(&row).unwrap()
        })
        .collect();
    let refs = install_items(
        root.path(),
        &mut open,
        vec![(serde_json::json!({"id":"preview","stationId":"A"}), rows)],
    );
    let reference = EvidenceReference::Committed(refs[0].clone());
    let mut request = PreviewRequest {
        store: open.store.clone(),
        reference: reference.clone(),
        cursor: None,
        limit: 2,
        columns: vec!["event_ref".into(), "wide".into()],
    };
    let lease = open_reference(root.path(), &request.store, &reference).unwrap();
    let first = history::preview_page(&lease, &request).unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.rows.len(), 2);
    let removed = first.rows[0].member.clone();
    let surviving = first.rows[1].member.occurrence_id.clone();
    assert_eq!(
        serde_json::to_value(&first).unwrap()["rows"][0]["cells"][1]["text"],
        u64::MAX.to_string()
    );
    request.cursor = first.next_cursor.clone();
    assert_eq!(
        history::preview_page(&lease, &request).unwrap().rows.len(),
        1
    );
    drop(first);
    drop(lease);
    let conn = authority::connect_readonly(root.path()).unwrap();
    let stamp = db::stamp(&conn).unwrap();
    drop(conn);
    let changed = prepare_membership(
        root.path(),
        &MembershipEdit {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_store: stamp,
            reference,
            action: MembershipAction::Remove {
                members: vec![removed],
            },
        },
    )
    .unwrap();
    request.reference = EvidenceReference::Prepared(changed.reference.clone());
    request.cursor = None;
    let pending = open_reference(root.path(), &request.store, &request.reference).unwrap();
    let after = history::preview_page(&pending, &request).unwrap();
    assert_eq!(after.total, 2);
    assert_eq!(after.rows[0].member.occurrence_id, surviving);
    assert_eq!(
        after.rows[0].member.manifest_id,
        changed.reference.manifest_id
    );
    assert!(after.next_cursor.is_none());
    assert!(
        discard_prepared(
            root.path(),
            &DiscardRequest {
                store: request.store.clone(),
                reference: changed.reference
            }
        )
        .unwrap()
        .discarded
    );
    assert!(history::preview_page(&pending, &request).is_err());
}

#[test]
fn stored_frozen_alias_map_precedes_current_unique_history_resolution() {
    use crate::case_evidence_anchors::{AnchorMap, BlockedAlias, BlockedState};
    let (root, mut open, _) = authority::tests::fixture();
    install_items(
        root.path(),
        &mut open,
        vec![(
            serde_json::json!({"id":"item","stationId":"A"}),
            vec![serde_json::to_string(&event(0, "same", None)).unwrap()],
        )],
    );
    let map = AnchorMap {
        schema_version: 1,
        event_aliases: vec![
            BlockedAlias {
                alias: "e:item:missing".into(),
                state: BlockedState::Missing,
                reason: None,
            },
            BlockedAlias {
                alias: "e:item:same".into(),
                state: BlockedState::Ambiguous,
                reason: None,
            },
        ],
        item_aliases: vec![],
    };
    let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
    conn.execute(
        "UPDATE native_evidence_anchors SET body=?1 WHERE case_id=?2 AND analysis_id=?3",
        rusqlite::params![
            serde_json::to_string(&map).unwrap(),
            open.analysis_context.case_id,
            open.analysis_context.analysis_id
        ],
    )
    .unwrap();
    drop(conn);
    let mut request = scope(&open);
    request.aliases = vec![
        "e:item:same".into(),
        "a:e:item:same".into(),
        "e:item:missing".into(),
    ];
    request.authored_view_json = serde_json::json!({"timeline":{"edits":{"e:item:same":{"title":"must remain unresolved"}}},"manual":[]}).to_string();
    let lease = open_history(
        root.path(),
        &open.store,
        &request.owner,
        &request.evidence_signature,
    )
    .unwrap();
    let response = history::timeline(&lease, &request).unwrap();
    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.entries[0].title.text, "preserved original");
    assert_eq!(
        response
            .aliases
            .iter()
            .map(|alias| alias.state)
            .collect::<Vec<_>>(),
        ["ambiguous", "ambiguous", "missing"]
    );
    assert!(response
        .aliases
        .iter()
        .all(|alias| alias.entry_id.is_none()));
}
