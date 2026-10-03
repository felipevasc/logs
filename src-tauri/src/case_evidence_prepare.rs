//! Bounded native capture preparations. Tokens own immutable preparations, not
//! client paths. Preparation never writes a Case body or advances its revision.
use super::*;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING: usize = 8;
const MAX_PENDING_BYTES: u64 = 64 << 20;
const INVALID:&str="CASE_EVIDENCE_PREPARATION_CHANGED: A preparação não pertence mais a esta investigação; atualize a visualização.";
const EXPIRED:&str="CASE_EVIDENCE_PREPARATION_EXPIRED: A preparação expirou ou não está disponível. Prepare novamente os registros; nenhum Caso foi alterado.";
const BUSY: &str =
    "CASE_EVIDENCE_PREPARATION_BUSY: Salve ou descarte outra preparação antes de continuar.";
static SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static PENDING: LazyLock<Mutex<BTreeMap<String, Arc<Pending>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub(super) enum PreparedPayload {
    Capture(Arc<PreparedContainer>),
    Membership(Arc<assemble::PreparedSet>),
}
pub(super) struct Pending {
    pub root: PathBuf,
    pub reference: PendingEvidenceRef,
    pub payload: PreparedPayload,
    pub fingerprint: String,
    pub created: Instant,
}
impl Pending {
    pub(super) fn committed_reference(&self) -> &EvidenceRef {
        match &self.payload {
            PreparedPayload::Capture(value) => &value.reference,
            PreparedPayload::Membership(value) => &value.reference,
        }
    }
    pub(super) fn manifest(&self) -> &ContainerManifest {
        match &self.payload {
            PreparedPayload::Capture(value) => value.manifest(),
            PreparedPayload::Membership(value) => &value.manifest,
        }
    }
    pub(super) fn manifest_bytes(&self) -> u64 {
        match &self.payload {
            PreparedPayload::Capture(value) => value.manifest_bytes(),
            PreparedPayload::Membership(value) => value.manifest_bytes,
        }
    }
    pub(super) fn batches(&self) -> Vec<BatchRef> {
        match &self.payload {
            PreparedPayload::Capture(value) => vec![value.batch.clone()],
            PreparedPayload::Membership(value) => value.batches.clone(),
        }
    }
    /// Explicit file publication before BEGIN IMMEDIATE; no SQL authority is
    /// changed here. Failed later CAS leaves only UUID-owned immutable orphans.
    pub(super) fn publish_files(&self, root: &Path) -> Result<(), String> {
        if root.canonicalize().map_err(|e| e.to_string())? != self.root {
            return Err(INVALID.into());
        }
        match &self.payload {
            PreparedPayload::Capture(value) => value.publish_files(root),
            PreparedPayload::Membership(value) => value.container.validate(),
        }
    }
    /// Opens only already-published files. May hash/read; call before SQL.
    pub(super) fn container(&self) -> Result<Arc<VerifiedContainer>, String> {
        match &self.payload {
            PreparedPayload::Capture(value) => Ok(Arc::new(VerifiedContainer::open_sized(
                &self.root,
                &value.reference,
                [value.batch.clone()],
                value.manifest_bytes(),
            )?)),
            PreparedPayload::Membership(value) => {
                value.container.validate()?;
                Ok(Arc::clone(&value.container))
            }
        }
    }
    pub(super) fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
        if member.container_id != self.reference.container_id
            || member.manifest_id != self.reference.manifest_id
        {
            return Err(INVALID.into());
        }
        match &self.payload {
            PreparedPayload::Capture(value) => {
                let original = value
                    .manifest()
                    .members
                    .iter()
                    .find(|m| m.occurrence_id == member.occurrence_id)
                    .ok_or(INVALID)?;
                if original.record.batch_id != value.batch.batch_id {
                    return Err(INVALID.into());
                }
                value.open_batch()?.envelope(original.record.ordinal)
            }
            PreparedPayload::Membership(value) => value.container.envelope_for_member(member),
        }
    }
    /// Metadata only. publish_files() must have completed before the transaction.
    pub(super) fn publish_authority(&self, tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
        let batches = self.batches();
        for batch in &batches {
            let body = serde_json::to_string(batch).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO native_evidence_batches(batch_id,body) VALUES(?1,?2) ON CONFLICT(batch_id) DO NOTHING",rusqlite::params![batch.batch_id,body]).map_err(|e|e.to_string())?;
            if authority::batch(tx, &batch.batch_id)? != *batch {
                return Err(INVALID.into());
            }
        }
        let metadata = db::ManifestMetadata {
            reference: self.committed_reference().clone(),
            bytes: self.manifest_bytes(),
            batches: batches.iter().map(|b| b.batch_id.clone()).collect(),
        };
        tx.execute("INSERT INTO native_evidence_manifests(manifest_id,body) VALUES(?1,?2) ON CONFLICT(manifest_id) DO NOTHING",rusqlite::params![metadata.reference.manifest_id,serde_json::to_string(&metadata).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        authority::manifest(tx, self.committed_reference())?;
        Ok(())
    }
}
fn report(phase: &str, done: u64, total: Option<u64>) {
    let (id, label) = match phase {
        "staging" => ("evidence_staging", "Preservando registros nativos"),
        _ => ("evidence_validation", "Conferindo a preparação nativa"),
    };
    crate::operations::report_progress(
        "case_evidence_prepare",
        id,
        label,
        done.min(usize::MAX as u64) as usize,
        total.unwrap_or(0).min(usize::MAX as u64) as usize,
        "registros",
        0,
    );
}
pub(super) fn root_work() -> crate::case_recovery::Work<'static> {
    crate::case_recovery::Work {
        cancelled: &crate::operations::cancelled,
        progress: &report,
    }
}
pub(super) fn fingerprint(value: &impl Serialize) -> Result<String, String> {
    struct Writer {
        hash: Sha256,
        bytes: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > REQUEST_BYTES.saturating_sub(self.bytes) {
                return Err(std::io::Error::other("CASE_EVIDENCE_REQUEST_LIMIT"));
            }
            self.bytes += bytes.len();
            self.hash.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        hash: Sha256::new(),
        bytes: 0,
    };
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", writer.hash.finalize()))
}
pub(super) fn request_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 160 || value.chars().any(char::is_control) {
        Err(INVALID.into())
    } else {
        Ok(())
    }
}
fn cleanup_expired() {
    let retired = {
        let mut pending = PENDING.lock();
        let expired: Vec<_> = pending
            .iter()
            .filter(|(_, p)| p.created.elapsed() >= TTL)
            .map(|(key, _)| key.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|id| pending.remove(&id))
            .collect::<Vec<_>>()
    };
    drop(retired);
}
pub(super) fn serialized_guard() -> Result<parking_lot::MutexGuard<'static, ()>, String> {
    loop {
        crate::operations::check()?;
        if let Some(guard) = SERIAL.try_lock() {
            return Ok(guard);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn target(root: &Path, request: &CaptureRequest) -> Result<(), String> {
    let conn = authority::connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    db::require_stamp(&conn, &request.expected_store)?;
    authority::require_owner(&conn, &request.target)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(())
}
/// The producer resolves exact rows. After checksum/fsync verification, the
/// final callback revalidates source/config/visibility/catalog before a token
/// can be acknowledged. Identical pending replays never revisit the source.
pub(crate) fn prepare_capture(
    root: &Path,
    request: &CaptureRequest,
    produce: impl FnOnce(&mut RecordSink) -> Result<(), String>,
    validate_sealed: impl FnOnce() -> Result<(), String>,
) -> Result<PendingEvidenceRef, String> {
    request_id(&request.request_id)?;
    if request.rows.is_empty() || request.rows.len() > CAPTURE_RECORDS {
        return Err("CASE_EVIDENCE_CAPTURE_LIMIT".into());
    }
    let fingerprint = fingerprint(request)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let _root = crate::case_recovery::RootLease::shared(&root, &root_work())?;
    let _serial = serialized_guard()?;
    cleanup_expired();
    let durable = {
        let conn = authority::connect_readonly(&root)?;
        receipts::lookup(
            &conn,
            &request.request_id,
            &fingerprint,
            receipts::Kind::Capture,
        )?
    };
    let replay = {
        let pending = PENDING.lock();
        pending
            .values()
            .find(|p| p.root == root && p.reference.request_id == request.request_id)
            .cloned()
    };
    if let Some(existing) = replay {
        if existing.fingerprint != fingerprint {
            return Err(INVALID.into());
        }
        let conn = authority::connect_readonly(&root)?;
        if db::stamp(&conn)?.identity() != request.expected_store.identity() {
            return Err(INVALID.into());
        }
        authority::require_owner(&conn, &request.target)?;
        _root.validate()?;
        return Ok(existing.reference.clone());
    }
    if durable.is_some() {
        return Err(EXPIRED.into());
    }
    if PENDING.lock().len() >= MAX_PENDING {
        return Err(BUSY.into());
    }
    target(&root, request)?;
    report("staging", 0, Some(request.rows.len() as u64));
    let origin = serde_json::json!({"requestId":request.request_id,"source":request.source,"target":request.target});
    let capture = Arc::new(stage_records(&root, &request.target, &origin, |sink| {
        produce(sink)?;
        if sink.records() != request.rows.len() {
            return Err("CASE_EVIDENCE_CAPTURE_INCOMPLETE: A preparação não contém exatamente os registros solicitados.".into());
        }
        target(&root, request)
    })?);
    validate_sealed()?;
    _root.validate()?;
    target(&root, request)?;
    let bytes = capture
        .batch
        .bytes
        .checked_add(capture.manifest_bytes())
        .ok_or(BUSY)?;
    {
        let pending = PENDING.lock();
        let used = pending
            .values()
            .try_fold(0u64, |sum, p| sum.checked_add(p.reference.bytes))
            .ok_or(BUSY)?;
        if bytes > MAX_PENDING_BYTES.saturating_sub(used) {
            return Err(BUSY.into());
        }
    }
    // Disk files remain an unpublished preparation. cases_save_view publishes
    // immutable destinations only after validating the complete save request.
    let reference = PendingEvidenceRef {
        kind: PendingKind::PendingNativeEvidence,
        purpose: PreparedPurpose::NewContainer {},
        schema_version: SCHEMA_VERSION,
        token: uuid::Uuid::new_v4().to_string(),
        request_id: request.request_id.clone(),
        owner: request.target.clone(),
        container_id: capture.reference.container_id.clone(),
        manifest_id: capture.reference.manifest_id.clone(),
        manifest_sha256: capture.reference.manifest_sha256.clone(),
        member_count: capture.reference.member_count,
        bytes,
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(TTL.as_secs() as i64))
            .to_rfc3339(),
    };
    persist_preparation(
        &root,
        &request.request_id,
        &fingerprint,
        &request.expected_store,
        &request.target,
        receipts::Kind::Capture,
        serde_json::to_value(&reference).map_err(|e| e.to_string())?,
    )?;
    PENDING.lock().insert(
        reference.token.clone(),
        Arc::new(Pending {
            root,
            reference: reference.clone(),
            payload: PreparedPayload::Capture(capture),
            fingerprint,
            created: Instant::now(),
        }),
    );
    crate::operations::commit();
    report(
        "validated",
        request.rows.len() as u64,
        Some(request.rows.len() as u64),
    );
    Ok(reference)
}
pub(super) fn pin(root: &Path, reference: &PendingEvidenceRef) -> Result<Arc<Pending>, String> {
    crate::operations::check()?;
    let pending = PENDING
        .lock()
        .get(&reference.token)
        .cloned()
        .ok_or(EXPIRED)?;
    if pending.created.elapsed() >= TTL {
        return Err(EXPIRED.into());
    }
    if pending.root != root.canonicalize().map_err(|e| e.to_string())?
        || pending.reference != *reference
    {
        return Err(INVALID.into());
    }
    Ok(pending)
}
/// Only after a durable save receipt is committed. In-flight Arc pins keep the
/// preparation alive; a failed SQL transaction must not consume these tokens.
pub(super) fn retire(tokens: &[String]) {
    let retired = {
        let mut pending = PENDING.lock();
        tokens
            .iter()
            .filter_map(|token| pending.remove(token))
            .collect::<Vec<_>>()
    };
    drop(retired);
}

/// Cancellation releases only a still-pending token. The same serialization
/// gate is held by save publication, so a commit cannot lose its preparation.
pub(crate) fn discard_prepared(
    root: &Path,
    request: &DiscardRequest,
) -> Result<DiscardReceipt, String> {
    crate::operations::check()?;
    uuid::Uuid::parse_str(&request.reference.token).map_err(|_| INVALID)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let guard = crate::case_recovery::RootLease::shared(&root, &root_work())?;
    let Some(_serial) = SERIAL.try_lock() else {
        return Ok(DiscardReceipt { discarded: false });
    };
    let conn = authority::connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if db::stamp(&conn)?.identity() != request.store {
        return Err(INVALID.into());
    }
    authority::require_owner(&conn, &request.reference.owner)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    guard.validate()?;
    let retired = {
        let mut pending = PENDING.lock();
        match pending.get(&request.reference.token) {
            None => None,
            Some(value) if value.root == root && value.reference == request.reference => {
                pending.remove(&request.reference.token)
            }
            Some(_) => return Err(INVALID.into()),
        }
    };
    let discarded = retired.is_some();
    drop(retired);
    crate::operations::commit();
    Ok(DiscardReceipt { discarded })
}

fn persist_preparation(
    root: &Path,
    request_id: &str,
    fingerprint: &str,
    expected: &StoreStamp,
    owner: &EvidenceOwner,
    kind: receipts::Kind,
    value: Value,
) -> Result<(), String> {
    let mut conn = authority::connect_readwrite(root)?;
    db::transaction(
        &mut conn,
        &[],
        |tx| {
            db::require_stamp(tx, expected)?;
            authority::require_owner(tx, owner)
        },
        |tx| {
            receipts::insert(
                tx,
                request_id,
                fingerprint,
                &receipts::Entry {
                    version: 1,
                    kind,
                    expected_store: Some(expected.clone()),
                    publication_store: expected.identity(),
                    value,
                },
            )
        },
    )
}

/// Replacement preparation preserves immutable record locators and occurrence
/// origins. Only cases_save_view may publish the returned membership authority.
pub(crate) fn prepare_membership(
    root: &Path,
    request: &MembershipEdit,
) -> Result<MembershipReceipt, String> {
    request_id(&request.request_id)?;
    let fingerprint = fingerprint(request)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &root_work())?;
    let _serial = serialized_guard()?;
    cleanup_expired();
    let conn = authority::connect_readonly(&root)?;
    if let Some(replay) = receipts::lookup(
        &conn,
        &request.request_id,
        &fingerprint,
        receipts::Kind::Membership,
    )? {
        let receipt: MembershipReceipt =
            serde_json::from_value(replay.entry.value).map_err(|e| e.to_string())?;
        pin(&root, &receipt.reference)?;
        root_guard.validate()?;
        return Ok(receipt);
    }
    db::require_stamp(&conn, &request.expected_store)?;
    authority::require_owner(&conn, request.reference.owner())?;
    if PENDING.lock().len() >= MAX_PENDING {
        return Err(BUSY.into());
    }
    // Committed edits start at the current binding. A historical manifest may
    // be an explicit restore target, never an implicit current replacement.
    if let EvidenceReference::Committed(reference) = &request.reference {
        let (case, _credit) = authority::read_case(
            &conn,
            &root,
            &reference.owner,
            authority::OpenScope::Metadata,
        )?;
        if !case.references().any(|(_, current)| current == reference) {
            return Err(INVALID.into());
        }
    }
    drop(conn);
    let current = open_reference(
        &root,
        &request.expected_store.identity(),
        &request.reference,
    )?;
    let selected = match &request.action {
        MembershipAction::Remove { .. } => None,
        MembershipAction::Restore { target } => {
            if target.owner() != request.reference.owner()
                || target.container_id() != request.reference.container_id()
            {
                return Err(INVALID.into());
            }
            Some(open_reference(
                &root,
                &request.expected_store.identity(),
                target,
            )?)
        }
    };
    let source = selected.as_ref().unwrap_or(&current);
    source.publish_prepared_files()?;
    let estimate = source
        .manifest()
        .members
        .len()
        .checked_mul(1536)
        .and_then(|n| n.checked_add(2 << 20))
        .ok_or(BUSY)?;
    let credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), estimate)?;
    let mut members = source.manifest().members.clone();
    if let MembershipAction::Remove { members: removals } = &request.action {
        if removals.is_empty() || removals.len() > MANIFEST_MEMBERS {
            return Err(INVALID.into());
        }
        let mut ids = std::collections::HashSet::new();
        for member in removals {
            crate::operations::check()?;
            if member.container_id != request.reference.container_id()
                || member.manifest_id != request.reference.manifest_id()
                || !ids.insert(member.occurrence_id.as_str())
            {
                return Err(INVALID.into());
            }
        }
        let before = members.len();
        members.retain(|member| !ids.contains(member.occurrence_id.as_str()));
        if before - members.len() != ids.len() {
            return Err(INVALID.into());
        }
    }
    let old_ids = current
        .manifest()
        .members
        .iter()
        .map(|member| member.occurrence_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let new_ids = members
        .iter()
        .map(|member| member.occurrence_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let changed_count = old_ids.symmetric_difference(&new_ids).count() as u32;
    let remaining_count = members.len() as u32;
    let used = members
        .iter()
        .map(|member| member.record.batch_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let batches = source
        .batches()
        .into_iter()
        .filter(|batch| used.contains(batch.batch_id.as_str()))
        .collect();
    let prepared = Arc::new(assemble::seal_manifest(
        &root,
        request.reference.owner(),
        request.reference.container_id().to_string(),
        members,
        batches,
        credit,
    )?);
    current.validate()?;
    if let Some(selected) = &selected {
        selected.validate()?;
    }
    root_guard.validate()?;
    let purpose = match &request.reference {
        EvidenceReference::Committed(reference) => PreparedPurpose::ReplaceContainer {
            base_manifest_id: reference.manifest_id.clone(),
            base_manifest_sha256: reference.manifest_sha256.clone(),
        },
        EvidenceReference::Prepared(reference) => reference.purpose.clone(),
    };
    let bytes = prepared.manifest_bytes;
    let used_bytes = PENDING
        .lock()
        .values()
        .try_fold(0u64, |sum, p| sum.checked_add(p.reference.bytes))
        .ok_or(BUSY)?;
    if bytes > MAX_PENDING_BYTES.saturating_sub(used_bytes) {
        return Err(BUSY.into());
    }
    let reference = PendingEvidenceRef {
        kind: PendingKind::PendingNativeEvidence,
        purpose,
        schema_version: SCHEMA_VERSION,
        token: uuid::Uuid::new_v4().to_string(),
        request_id: request.request_id.clone(),
        owner: request.reference.owner().clone(),
        container_id: prepared.reference.container_id.clone(),
        manifest_id: prepared.reference.manifest_id.clone(),
        manifest_sha256: prepared.reference.manifest_sha256.clone(),
        member_count: remaining_count,
        bytes,
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(TTL.as_secs() as i64))
            .to_rfc3339(),
    };
    let receipt = MembershipReceipt {
        reference: reference.clone(),
        undo: request.reference.clone(),
        changed_count,
        remaining_count,
    };
    persist_preparation(
        &root,
        &request.request_id,
        &fingerprint,
        &request.expected_store,
        request.reference.owner(),
        receipts::Kind::Membership,
        serde_json::to_value(&receipt).map_err(|e| e.to_string())?,
    )?;
    PENDING.lock().insert(
        reference.token.clone(),
        Arc::new(Pending {
            root,
            reference,
            payload: PreparedPayload::Membership(prepared),
            fingerprint,
            created: Instant::now(),
        }),
    );
    crate::operations::commit();
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clear_root(root: &Path) {
        let root = root.canonicalize().unwrap();
        let ids = PENDING
            .lock()
            .values()
            .filter(|p| p.root == root)
            .map(|p| p.reference.token.clone())
            .collect::<Vec<_>>();
        retire(&ids);
    }
    #[test]
    fn repeated_unsaved_remove_restore_reaches_bounded_capacity_without_losing_prior_tokens() {
        let (dir, _, references) = authority::tests::fixture();
        let conn = authority::connect_readonly(dir.path()).unwrap();
        let stamp = db::stamp(&conn).unwrap();
        let original = EvidenceReference::Committed(references[1].clone());
        let mut current = original.clone();
        let mut receipts = Vec::new();
        for step in 0..MAX_PENDING {
            let source = open_reference(dir.path(), &stamp.identity(), &current).unwrap();
            let action = if source.manifest().members.is_empty() {
                MembershipAction::Restore {
                    target: original.clone(),
                }
            } else {
                MembershipAction::Remove {
                    members: vec![MemberHandle {
                        container_id: current.container_id().into(),
                        manifest_id: current.manifest_id().into(),
                        occurrence_id: source.manifest().members[0].occurrence_id.clone(),
                    }],
                }
            };
            let request = MembershipEdit {
                request_id: format!("membership-{step}"),
                expected_store: stamp.clone(),
                reference: current.clone(),
                action,
            };
            let receipt = prepare_membership(dir.path(), &request).unwrap();
            assert_eq!(receipt.changed_count, 1);
            assert_eq!(receipt.remaining_count, (step % 2) as u32);
            assert_eq!(receipt.undo, current);
            assert_eq!(
                prepare_membership(dir.path(), &request).unwrap().reference,
                receipt.reference
            );
            current = EvidenceReference::Prepared(receipt.reference.clone());
            receipts.push(receipt);
        }
        let request = MembershipEdit {
            request_id: "membership-overflow".into(),
            expected_store: stamp.clone(),
            reference: current,
            action: MembershipAction::Restore { target: original },
        };
        assert!(prepare_membership(dir.path(), &request)
            .unwrap_err()
            .starts_with("CASE_EVIDENCE_PREPARATION_BUSY"));
        for receipt in &receipts {
            assert!(pin(dir.path(), &receipt.reference).is_ok());
        }
        let first = receipts[0].reference.clone();
        assert!(
            discard_prepared(
                dir.path(),
                &DiscardRequest {
                    store: stamp.identity(),
                    reference: first.clone()
                }
            )
            .unwrap()
            .discarded
        );
        assert!(
            !discard_prepared(
                dir.path(),
                &DiscardRequest {
                    store: stamp.identity(),
                    reference: first
                }
            )
            .unwrap()
            .discarded
        );
        assert!(prepare_membership(dir.path(), &request).is_ok());
        clear_root(dir.path());
    }
    #[test]
    fn expired_membership_request_is_never_reinterpreted_as_a_fresh_preparation() {
        let (dir, _, references) = authority::tests::fixture();
        let conn = authority::connect_readonly(dir.path()).unwrap();
        let stamp = db::stamp(&conn).unwrap();
        let reference = EvidenceReference::Committed(references[1].clone());
        let opened = open_reference(dir.path(), &stamp.identity(), &reference).unwrap();
        let member = &opened.manifest().members[0];
        let request = MembershipEdit {
            request_id: "durable-membership".into(),
            expected_store: stamp.clone(),
            reference: reference.clone(),
            action: MembershipAction::Remove {
                members: vec![MemberHandle {
                    container_id: reference.container_id().into(),
                    manifest_id: reference.manifest_id().into(),
                    occurrence_id: member.occurrence_id.clone(),
                }],
            },
        };
        let first = prepare_membership(dir.path(), &request).unwrap();
        retire(&[first.reference.token]); // Simulate process-local token loss.
        assert!(prepare_membership(dir.path(), &request)
            .unwrap_err()
            .starts_with("CASE_EVIDENCE_PREPARATION_EXPIRED"));
        let mut changed = request;
        changed.action = MembershipAction::Restore { target: reference };
        assert!(prepare_membership(dir.path(), &changed)
            .unwrap_err()
            .starts_with("CASE_EVIDENCE_REQUEST_REUSED"));
        clear_root(dir.path());
    }
}
