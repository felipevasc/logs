//! Prepared Case authority. All record/mirror I/O completes before the short
//! investigation transaction. No frontend record representation is accepted.
use super::*;
use crate::{analysis_context::Snapshot, case_work_budget::Lease};
use rusqlite::{params, Connection, Transaction};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
const INVALID: &str =
    "CASE_EVIDENCE_PREPARED_CASE: O Caso preparado não corresponde à investigação.";

pub(crate) fn snapshot_transport_safe(snapshot: &Snapshot) -> Result<(), String> {
    const SAFE: u64 = 9_007_199_254_740_991;
    if snapshot.config_revision > SAFE
        || snapshot.visibility_revision > SAFE
        || snapshot.migration_diagnostics.iter().any(|d| {
            d.definition_index
                .is_some_and(|index| index as u128 > SAFE as u128)
        })
    {
        return Err("CASE_EVIDENCE_METADATA_NUMBER".into());
    }
    for value in snapshot
        .config
        .derived_fields
        .iter()
        .chain(snapshot.legacy_raw.iter())
    {
        metadata_transport_safe(value)?;
    }
    Ok(())
}

/// Only explicitly referenced native targets are transported. Ordinals refer
/// to the exported array order, never the sparse original occurrence position.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableContainerMap {
    pub container_id: String,
    pub location: ContainerLocation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableMemberMap {
    pub container_id: String,
    pub occurrence_id: String,
    pub record_ordinal: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableCaseMap {
    pub case_ordinal: u32,
    pub anchors: crate::case_evidence_anchors::AnchorMap,
    pub containers: Vec<PortableContainerMap>,
    pub members: Vec<PortableMemberMap>,
}
impl PortableCaseMap {
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.anchors.validate()?;
        if self.case_ordinal >= 1000
            || self
                .containers
                .len()
                .checked_add(self.members.len())
                .is_none_or(|n| n > 10_000)
        {
            return Err(INVALID.into());
        }
        view::json_size(self, 4 << 20)?;
        let count = self.containers.len() + self.members.len();
        let _scratch = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            count
                .checked_mul(512)
                .and_then(|n| n.checked_add(64 << 10))
                .ok_or(INVALID)?,
        )?;
        let mut containers = std::collections::HashSet::new();
        let mut locations = std::collections::BTreeSet::new();
        for value in &self.containers {
            if uuid::Uuid::parse_str(&value.container_id).is_err()
                || !containers.insert(value.container_id.as_str())
                || !locations.insert(value.location.clone())
            {
                return Err(INVALID.into());
            }
        }
        let mut members = std::collections::HashSet::new();
        let mut positions = std::collections::HashSet::new();
        for value in &self.members {
            if !containers.contains(value.container_id.as_str())
                || uuid::Uuid::parse_str(&value.occurrence_id).is_err()
                || value.record_ordinal as usize >= MANIFEST_MEMBERS
                || !members.insert((value.container_id.as_str(), value.occurrence_id.as_str()))
                || !positions.insert((value.container_id.as_str(), value.record_ordinal))
            {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
}

pub(crate) struct CommittedCaseView {
    value: Value,
    _credit: Lease,
}
impl CommittedCaseView {
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
}
impl Serialize for CommittedCaseView {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(s)
    }
}

pub(crate) struct PreparedCase {
    root: PathBuf,
    expected_store: StoreStamp,
    snapshot: Snapshot,
    owner: EvidenceOwner,
    metadata: Value,
    bindings: Vec<authority::Binding>,
    sets: Vec<assemble::PreparedSet>,
    mirror: body::StagedMirror,
    evidence: CaseEvidenceState,
    anchors: crate::case_evidence_anchors::AnchorMap,
    _credit: Lease,
}
impl PreparedCase {
    pub(super) fn mirror_bytes(&self) -> usize {
        self.mirror.bytes()
    }
    pub(super) fn validate_snapshot(&self, tx: &Transaction<'_>) -> Result<(), String> {
        let bytes = view::json_size(&self.snapshot, 4 << 20)?;
        let _scratch = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            bytes.checked_mul(3).ok_or(INVALID)?,
        )?;
        let expected = serde_json::to_string(&self.snapshot).map_err(|e| e.to_string())?;
        let matches: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1 AND body=?2)",
                params![self.owner.case_id, expected],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !matches {
            return Err(INVALID.into());
        }
        Ok(())
    }
    pub(crate) fn owner(&self) -> &EvidenceOwner {
        &self.owner
    }
    pub(crate) fn anchors(&self) -> &crate::case_evidence_anchors::AnchorMap {
        &self.anchors
    }
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub(super) fn metadata(&self) -> &Value {
        &self.metadata
    }
    pub(super) fn bindings(&self) -> &[authority::Binding] {
        &self.bindings
    }
    pub(super) fn expected_store(&self) -> &StoreStamp {
        &self.expected_store
    }
    pub(super) fn evidence(&self) -> &CaseEvidenceState {
        &self.evidence
    }
    pub(super) fn validate(&self, root: &Path) -> Result<(), String> {
        if root.canonicalize().map_err(|e| e.to_string())? != self.root {
            return Err(INVALID.into());
        }
        for set in &self.sets {
            set.container.validate()?;
        }
        Ok(())
    }
    pub(crate) fn committed_view(&self) -> Result<CommittedCaseView, String> {
        let bytes = view::json_size(&self.metadata, VIEW_DOCUMENT_BYTES)?
            .checked_add(view::json_size(&self.snapshot, 4 << 20)?)
            .ok_or(INVALID)?;
        let reserve = bytes
            .checked_mul(8)
            .and_then(|n| n.checked_add(self.bindings.len() * 4096 + 64 * 1024))
            .ok_or(INVALID)?;
        let credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), reserve)?;
        let mut metadata = self.metadata.clone();
        if !matches!(self.evidence, CaseEvidenceState::Unavailable(_)) {
            for binding in &self.bindings {
                *metadata
                    .pointer_mut(&binding.location.pointer())
                    .ok_or(INVALID)? = serde_json::to_value(ContainerView {
                    kind: "native_evidence_container",
                    reference: EvidenceReference::Committed(binding.reference.clone()),
                    preserved_count: binding.reference.member_count,
                    preview: None,
                })
                .map_err(|e| e.to_string())?;
            }
            metadata.as_object_mut().ok_or(INVALID)?.insert(
                "analysisContext".into(),
                serde_json::to_value(&self.snapshot).map_err(|e| e.to_string())?,
            );
        } else if let CaseEvidenceState::Unavailable(diagnostic) = &self.evidence {
            metadata = serde_json::to_value(UnavailableCaseStub {
                kind: UnavailableCaseKind::PreservedCaseUnavailable,
                id: self.owner.case_id.clone(),
                code: diagnostic.code.clone(),
            })
            .map_err(|e| e.to_string())?;
        }
        view::json_size(&metadata, VIEW_DOCUMENT_BYTES)?;
        Ok(CommittedCaseView {
            value: metadata,
            _credit: credit,
        })
    }
    /// For a fresh import. The caller owns the StoreStamp CAS, permit and COMMIT.
    /// Sidecar insertion may follow; its resulting Snapshot must remain equal.
    pub(super) fn publish_authority(
        &self,
        tx: &Transaction<'_>,
        position: i64,
        recovery_id: &str,
    ) -> Result<(), String> {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",
                [&self.owner.case_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            return Err(INVALID.into());
        }
        body::write_mirror(tx, &self.owner.case_id, position, &self.mirror)?;
        self.insert_authority(tx, recovery_id)
    }
    /// Verified adoption of an existing body. The caller has compared the full
    /// copied authority and installed a transaction-scoped permit already.
    pub(super) fn publish_existing(
        &self,
        tx: &Transaction<'_>,
        position: i64,
        recovery_id: &str,
    ) -> Result<(), String> {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",
                [&self.owner.case_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err(INVALID.into());
        }
        body::write_mirror(tx, &self.owner.case_id, position, &self.mirror)?;
        self.insert_authority(tx, recovery_id)
    }
    pub(super) fn insert_authority(
        &self,
        tx: &Transaction<'_>,
        recovery_id: &str,
    ) -> Result<(), String> {
        self.insert_authority_with_hash(tx, recovery_id, self.mirror.sha256())
    }
    pub(super) fn insert_authority_with_hash(
        &self,
        tx: &Transaction<'_>,
        recovery_id: &str,
        body_sha256: &str,
    ) -> Result<(), String> {
        if db::stamp(tx)?.identity() != self.expected_store.identity() {
            return Err(INVALID.into());
        }
        let actual = crate::analysis_context::insert_portable_snapshot(tx, &self.snapshot)?;
        if actual != self.snapshot {
            return Err(INVALID.into());
        }
        for set in &self.sets {
            for batch in &set.batches {
                let encoded = serde_json::to_string(batch).map_err(|e| e.to_string())?;
                tx.execute("INSERT INTO native_evidence_batches(batch_id,body) VALUES(?1,?2) ON CONFLICT(batch_id) DO NOTHING",params![batch.batch_id,encoded]).map_err(|e|e.to_string())?;
                if authority::batch(tx, &batch.batch_id)? != *batch {
                    return Err(INVALID.into());
                }
            }
            let entry = db::ManifestMetadata {
                reference: set.reference.clone(),
                bytes: set.manifest_bytes,
                batches: set.batches.iter().map(|b| b.batch_id.clone()).collect(),
            };
            let encoded = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO native_evidence_manifests(manifest_id,body) VALUES(?1,?2)",
                params![set.reference.manifest_id, encoded],
            )
            .map_err(|e| e.to_string())?;
        }
        let signature = match &self.evidence {
            CaseEvidenceState::Ready(value) => value.evidence_signature.clone(),
            CaseEvidenceState::Unavailable(_) => {
                format!("{:x}", Sha256::digest(self.mirror.sha256().as_bytes()))
            }
        };
        let metadata = serde_json::to_string(&self.metadata).map_err(|e| e.to_string())?;
        let bindings = serde_json::to_string(&self.bindings).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO native_evidence_cases(case_id,analysis_id,metadata,bindings,evidence_signature,body_sha256,recovery_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![self.owner.case_id,self.owner.analysis_id,metadata,bindings,signature,body_sha256,recovery_id]).map_err(|e|e.to_string())?;
        anchor_store::insert(tx, &self.owner, &self.anchors)?;
        db::protect(tx, &self.owner)
    }
}

fn write_opaque_case(
    raw: RawJson<'_>,
    owner: &EvidenceOwner,
    writer: &mut dyn Write,
) -> Result<(), String> {
    writer.write_all(b"{\"id\":").map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut *writer, &owner.case_id).map_err(|e| e.to_string())?;
    let mut fields = raw.members()?;
    while let Some(field) = fields.next()? {
        crate::operations::check()?;
        let key = if field.key.len() <= 128 {
            Some(field.key()?)
        } else {
            None
        };
        if key
            .as_deref()
            .is_some_and(|key| matches!(key, "id" | "analysisContext" | "portableImportToken"))
        {
            continue;
        }
        writer
            .write_all(b",")
            .and_then(|_| writer.write_all(field.key.as_bytes()))
            .and_then(|_| writer.write_all(b":"))
            .and_then(|_| writer.write_all(field.value.get().as_bytes()))
            .map_err(|e| e.to_string())?;
    }
    writer.write_all(b"}").map_err(|e| e.to_string())
}

pub(crate) fn prepare_import_case(
    root: &Path,
    expected_store: &StoreStamp,
    snapshot: Snapshot,
    original_case_json: &str,
) -> Result<PreparedCase, String> {
    prepare_import_case_inner(root, expected_store, snapshot, original_case_json, None)
}
pub(crate) fn prepare_import_case_with_transfer(
    root: &Path,
    expected_store: &StoreStamp,
    snapshot: Snapshot,
    original_case_json: &str,
    transfer: &PortableCaseMap,
) -> Result<PreparedCase, String> {
    transfer.validate()?;
    prepare_import_case_inner(
        root,
        expected_store,
        snapshot,
        original_case_json,
        Some(transfer),
    )
}
fn prepare_import_case_inner(
    root: &Path,
    expected_store: &StoreStamp,
    snapshot: Snapshot,
    original_case_json: &str,
    transfer: Option<&PortableCaseMap>,
) -> Result<PreparedCase, String> {
    crate::operations::check()?;
    crate::analysis_context::validate_snapshot_size(&snapshot)?;
    let owner = EvidenceOwner {
        store_id: expected_store.store_id.clone(),
        case_id: snapshot.case_id.clone(),
        analysis_id: snapshot.analysis_id.clone(),
    };
    if owner.case_id.is_empty()
        || owner.case_id.len() > 4096
        || uuid::Uuid::parse_str(&owner.analysis_id).is_err()
    {
        return Err(INVALID.into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let conn = authority::connect_readonly(&root)?;
    db::require_stamp(&conn, expected_store)?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",
            [&owner.case_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists {
        return Err(INVALID.into());
    }
    drop(conn);
    prepare_case_inner(
        &root,
        expected_store,
        snapshot,
        original_case_json,
        &guard,
        transfer,
    )
}
pub(super) fn prepare_case(
    root: &Path,
    expected_store: &StoreStamp,
    snapshot: Snapshot,
    original_case_json: &str,
    guard: &crate::case_recovery::RootLease,
) -> Result<PreparedCase, String> {
    prepare_case_inner(
        root,
        expected_store,
        snapshot,
        original_case_json,
        guard,
        None,
    )
}
fn prepare_case_inner(
    root: &Path,
    expected_store: &StoreStamp,
    snapshot: Snapshot,
    original_case_json: &str,
    guard: &crate::case_recovery::RootLease,
    transfer: Option<&PortableCaseMap>,
) -> Result<PreparedCase, String> {
    let raw = checked_document(original_case_json)?;
    if raw.kind() != b'{' {
        return Err(INVALID.into());
    }
    let owner = EvidenceOwner {
        store_id: expected_store.store_id.clone(),
        case_id: snapshot.case_id.clone(),
        analysis_id: snapshot.analysis_id.clone(),
    };
    // Record bodies remain borrowed spans. Admit metadata/tree inventory,
    // plus one bounded envelope buffer, before constructing the extracted view.
    let plan = document::preflight_case(original_case_json);
    let estimate = plan
        .as_ref()
        .copied()
        .unwrap_or(256 << 10)
        .checked_add(ENVELOPE_BYTES)
        .ok_or(INVALID)?;
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), estimate)?;
    let mut anchors = if let Some(transfer) = transfer {
        let bytes = view::json_size(transfer, 4 << 20)?;
        credit.merge(crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            bytes.checked_mul(8).ok_or(INVALID)?,
        )?)?;
        transfer.anchors.clone()
    } else {
        crate::case_evidence_anchors::AnchorMap::default()
    };
    let mut sets = Vec::new();
    let mut bindings = Vec::new();
    let extracted = match plan {
        Ok(_) => extract_case(original_case_json),
        Err(error) => Err(error),
    };
    let (metadata, mirror, evidence) = match extracted {
        Ok(mut extracted) => {
            extracted.metadata["id"] = Value::String(owner.case_id.clone());
            extracted
                .metadata
                .as_object_mut()
                .ok_or(INVALID)?
                .remove("analysisContext");
            extracted
                .metadata
                .as_object_mut()
                .ok_or(INVALID)?
                .remove(crate::case_archive::TOKEN_FIELD);
            for source in extracted.containers {
                let set = assemble::assemble_envelopes(root, &owner, &source.records)?;
                bindings.push(authority::Binding {
                    location: source.location,
                    reference: set.reference.clone(),
                });
                sets.push(set);
            }
            if let Some(transfer) = transfer {
                let mut container_targets = std::collections::BTreeMap::new();
                for mapping in &transfer.containers {
                    let index = bindings
                        .iter()
                        .position(|binding| binding.location == mapping.location)
                        .ok_or(INVALID)?;
                    container_targets.insert(mapping.container_id.as_str(), index);
                }
                let mut member_targets = std::collections::BTreeMap::new();
                for mapping in &transfer.members {
                    let index = *container_targets
                        .get(mapping.container_id.as_str())
                        .ok_or(INVALID)?;
                    let member = sets[index]
                        .manifest
                        .members
                        .get(mapping.record_ordinal as usize)
                        .ok_or(INVALID)?;
                    member_targets.insert(
                        (
                            mapping.container_id.as_str(),
                            mapping.occurrence_id.as_str(),
                        ),
                        (
                            sets[index].reference.container_id.clone(),
                            member.occurrence_id.clone(),
                        ),
                    );
                }
                let remapped = crate::case_evidence_anchors::remap_native(
                    &mut extracted.metadata,
                    &anchors,
                    &mut |container, occurrence| {
                        Ok(member_targets.get(&(container, occurrence)).cloned())
                    },
                    &mut |container| {
                        Ok(container_targets
                            .get(container)
                            .map(|index| sets[*index].reference.container_id.clone()))
                    },
                )?;
                credit.merge(remapped.credit)?;
                anchors = remapped.anchors;
            } else {
                // A legacy document has no verified native identity map. Any
                // carried native anchor stays explicitly unresolved, never
                // eligible to bind to a later coincidental occurrence.
                let remapped = crate::case_evidence_anchors::remap_native(
                    &mut extracted.metadata,
                    &anchors,
                    &mut |_, _| Ok(None),
                    &mut |_| Ok(None),
                )?;
                credit.merge(remapped.credit)?;
                anchors = remapped.anchors;
            }
            let item_bindings = bindings
                .iter()
                .filter_map(|binding| match binding.location {
                    ContainerLocation::ItemRows { index } => {
                        Some(crate::case_evidence_anchors::ItemBinding {
                            item_index: index,
                            container_id: binding.reference.container_id.clone(),
                            member_count: binding.reference.member_count,
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let frozen = crate::case_evidence_anchors::freeze(
                &mut extracted.metadata,
                &item_bindings,
                &anchors,
                |visit| {
                    for (binding, set) in bindings.iter().zip(&sets) {
                        if let ContainerLocation::ItemRows { index } = binding.location {
                            set.container.visit_envelopes_with_origin(
                                0..set.reference.member_count as usize,
                                |member, origin, text| {
                                    visit(index, origin, member, text)?;
                                    Ok(Visit::Continue)
                                },
                            )?;
                        }
                    }
                    Ok(())
                },
            )?;
            credit.merge(frozen.credit)?;
            anchors = frozen.anchors;
            let containers = bindings
                .iter()
                .zip(&sets)
                .map(|(binding, set)| (binding.location.clone(), Arc::clone(&set.container)));
            let verified = VerifiedCase::from_parts(extracted.metadata, owner.clone(), containers)?;
            let unsafe_metadata = metadata_transport_safe(verified.metadata()).is_err()
                || snapshot_transport_safe(&snapshot).is_err();
            // This is native serialization even for a JS-unavailable view.
            // Keep exact typed metadata and remapped anchors; record fragments
            // are still emitted verbatim and never pass through JavaScript.
            let mirror = body::stage_mirror(root, |writer| verified.write_to(writer, true))?;
            let mut evidence = if unsafe_metadata {
                CaseEvidenceState::Unavailable(PreservedCaseDiagnostic{case_id:owner.case_id.clone(),owner:Some(owner.clone()),code:"metadata_number_unavailable".into(),message:"Metadados originais preservados; este Caso não pode atravessar uma visualização numérica sem perda.".into(),preserved_count:match verified.evidence_state()?{CaseEvidenceState::Ready(state)=>Some(state.preserved_count),_=>None},readiness:CaseReadiness::PreservedOnly})
            } else {
                verified.evidence_state()?
            };
            if let CaseEvidenceState::Ready(state) = &mut evidence {
                let bytes = view::json_size(&anchors.item_aliases, 1 << 20)?;
                credit.merge(crate::case_cache::reserve_work(
                    crate::case_work_budget::global(),
                    bytes.checked_mul(4).ok_or(INVALID)?,
                )?)?;
                state.legacy_item_aliases = anchors.item_aliases.clone();
            }
            let metadata = if let CaseEvidenceState::Unavailable(diagnostic) = &evidence {
                serde_json::to_value(UnavailableCaseStub {
                    kind: UnavailableCaseKind::PreservedCaseUnavailable,
                    id: owner.case_id.clone(),
                    code: diagnostic.code.clone(),
                })
                .map_err(|e| e.to_string())?
            } else {
                verified.metadata().clone()
            };
            (metadata, mirror, evidence)
        }
        Err(error) => {
            crate::operations::check()?;
            if transfer.is_some_and(|map| !map.containers.is_empty() || !map.members.is_empty()) {
                return Err("CASE_EVIDENCE_TRANSFER_OPAQUE: O documento foi preservado, mas seus vínculos nativos não podem ser remapeados com segurança.".into());
            }
            if !error.starts_with("CASE_EVIDENCE_DOCUMENT")
                && !error.starts_with("CASE_EVIDENCE_MATERIALIZATION")
                && !error.starts_with("CASE_EVIDENCE_UNSUPPORTED")
                && !error.starts_with("CASE_EVIDENCE_DUPLICATE")
            {
                return Err(error);
            }
            let code = "document_interpretation_unavailable";
            let metadata = serde_json::to_value(UnavailableCaseStub {
                kind: UnavailableCaseKind::PreservedCaseUnavailable,
                id: owner.case_id.clone(),
                code: code.into(),
            })
            .map_err(|e| e.to_string())?;
            let mirror = body::stage_mirror(root, |writer| write_opaque_case(raw, &owner, writer))?;
            let evidence=CaseEvidenceState::Unavailable(PreservedCaseDiagnostic{case_id:owner.case_id.clone(),owner:Some(owner.clone()),code:code.into(),message:"Documento original preservado; sua estrutura excede os limites de interpretação atuais.".into(),preserved_count:None,readiness:CaseReadiness::PreservedOnly});
            (metadata, mirror, evidence)
        }
    };
    guard.validate()?;
    crate::operations::check()?;
    Ok(PreparedCase {
        root: root.to_path_buf(),
        expected_store: expected_store.clone(),
        snapshot,
        owner,
        metadata,
        bindings,
        sets,
        mirror,
        evidence,
        anchors,
        _credit: credit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imported_context_numbers_are_classified_with_the_whole_management_view() {
        let (dir, _, _) = authority::tests::fixture();
        let mut conn = Connection::open(dir.path().join("investigations.sqlite3")).unwrap();
        let stamp = db::stamp(&conn).unwrap();
        let fraction = "2.547114365375239e-8".parse::<f64>().unwrap();
        for (number, safe) in [
            (json!(u64::MAX), false),
            (json!(1.0), false),
            (json!(-0.0), false),
            (json!(fraction), true),
        ] {
            let id = uuid::Uuid::new_v4().to_string();
            let mut snapshot = crate::analysis_context::prepare_new_case(&id).unwrap();
            snapshot
                .config
                .derived_fields
                .push(json!({"name":"x","source":"message","pattern":"(.*)","future":number}));
            let prepared = prepare_import_case(dir.path(), &stamp, snapshot.clone(), r#"{"id":"foreign","name":"Notes remain preserved","items":[{"id":"item","rows":[{"id":0,"future":18446744073709551615}]}]}"#).unwrap();
            assert_eq!(
                matches!(prepared.evidence(), CaseEvidenceState::Ready(_)),
                safe
            );
            let view = prepared.committed_view().unwrap();
            if safe {
                assert_eq!(
                    view.value()["analysisContext"]["config"]["derivedFields"][0]["future"]
                        .as_f64()
                        .unwrap()
                        .to_bits(),
                    0x3e5b597464455d8a
                );
            } else {
                assert_eq!(view.value()["kind"], "preserved_case_unavailable");
                assert!(view.value().get("analysisContext").is_none());
            }
            let tx = conn.transaction().unwrap();
            prepared.publish_authority(&tx, 10, "fixture").unwrap();
            prepared.validate_snapshot(&tx).unwrap();
            tx.commit().unwrap();
            let reopened = crate::analysis_context::read(&conn, &id).unwrap();
            assert_eq!(reopened, snapshot);
            let body: String = conn
                .query_row("SELECT body FROM cases WHERE id=?1", [&id], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(body.contains("18446744073709551615"));
            assert_eq!(
                crate::analysis_context::insert_portable_snapshot(
                    &conn.transaction().unwrap(),
                    &snapshot
                )
                .unwrap(),
                snapshot
            );
        }
    }

    #[test]
    fn sparse_portable_targets_remap_native_anchors_before_sealing() {
        let (root, _, _) = authority::tests::fixture();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let stamp = db::stamp(&conn).unwrap();
        let old_container = uuid::Uuid::new_v4().to_string();
        let old_member = uuid::Uuid::new_v4().to_string();
        let removed = uuid::Uuid::new_v4().to_string();
        let live = format!("n:{old_container}:{old_member}");
        let missing = format!("n:{old_container}:{removed}");
        let body = json!({"id":"foreign","items":[{"id":"i","rows":[{"id":1,"event_ref":"v:7"}]}],"timeline":{"annotations":[{"anchor":live},{"anchor":missing}]},"caseTrails":[{"itemIds":["i"],"itemRefs":[{"kind":"container","containerId":old_container}]}]});
        let transfer = PortableCaseMap {
            case_ordinal: 0,
            anchors: crate::case_evidence_anchors::AnchorMap::default(),
            containers: vec![PortableContainerMap {
                container_id: old_container.clone(),
                location: ContainerLocation::ItemRows { index: 0 },
            }],
            members: vec![PortableMemberMap {
                container_id: old_container,
                occurrence_id: old_member,
                record_ordinal: 0,
            }],
        };
        let snapshot = crate::analysis_context::prepare_new_case("local").unwrap();
        let prepared = prepare_import_case_with_transfer(
            root.path(),
            &stamp,
            snapshot,
            &body.to_string(),
            &transfer,
        )
        .unwrap();
        let set = &prepared.sets[0];
        let new_id = format!(
            "n:{}:{}",
            set.reference.container_id, set.manifest.members[0].occurrence_id
        );
        let view = prepared.committed_view().unwrap();
        assert_eq!(view.value()["timeline"]["annotations"][0]["anchor"], new_id);
        assert_eq!(
            view.value()["timeline"]["annotations"][1]["anchor"],
            missing
        );
        assert_eq!(
            view.value()["caseTrails"][0]["itemRefs"][0]["containerId"],
            set.reference.container_id
        );
        assert_eq!(
            prepared.anchors().blocked_event(&missing).unwrap().state,
            crate::case_evidence_anchors::BlockedState::Missing
        );
        let legacy = prepare_import_case(
            root.path(),
            &stamp,
            crate::analysis_context::prepare_new_case("legacy-native-text").unwrap(),
            &body.to_string(),
        )
        .unwrap();
        assert_eq!(
            legacy.anchors().blocked_event(&live).unwrap().state,
            crate::case_evidence_anchors::BlockedState::Missing
        );
        assert_eq!(
            legacy.committed_view().unwrap().value()["caseTrails"][0]["itemRefs"][0]["kind"],
            "unresolved_native_container"
        );
        let mut invalid = transfer;
        invalid.members[0].record_ordinal = 1;
        assert!(prepare_import_case_with_transfer(
            root.path(),
            &stamp,
            crate::analysis_context::prepare_new_case("invalid").unwrap(),
            &body.to_string(),
            &invalid
        )
        .is_err());
    }

    #[test]
    fn unavailable_management_metadata_keeps_remapped_native_mirror_and_opaque_transfer_refuses() {
        let (root, _, _) = authority::tests::fixture();
        let mut conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let stamp = db::stamp(&conn).unwrap();
        let old_container = uuid::Uuid::new_v4().to_string();
        let old_member = uuid::Uuid::new_v4().to_string();
        let old_anchor = format!("n:{old_container}:{old_member}");
        let body = json!({"id":"foreign","unsafeMetadata":u64::MAX,"items":[{"id":"i","rows":[{"id":1,"event_ref":"v:7","future":1.0}]}],"timeline":{"annotations":[{"anchor":old_anchor}]}});
        let transfer = PortableCaseMap {
            case_ordinal: 0,
            anchors: crate::case_evidence_anchors::AnchorMap::default(),
            containers: vec![PortableContainerMap {
                container_id: old_container.clone(),
                location: ContainerLocation::ItemRows { index: 0 },
            }],
            members: vec![PortableMemberMap {
                container_id: old_container,
                occurrence_id: old_member,
                record_ordinal: 0,
            }],
        };
        let prepared = prepare_import_case_with_transfer(
            root.path(),
            &stamp,
            crate::analysis_context::prepare_new_case("wide").unwrap(),
            &body.to_string(),
            &transfer,
        )
        .unwrap();
        assert!(matches!(
            prepared.evidence(),
            CaseEvidenceState::Unavailable(_)
        ));
        assert_eq!(
            prepared.committed_view().unwrap().value()["kind"],
            "preserved_case_unavailable"
        );
        let target = format!(
            "n:{}:{}",
            prepared.sets[0].reference.container_id,
            prepared.sets[0].manifest.members[0].occurrence_id
        );
        let tx = conn.transaction().unwrap();
        prepared.publish_authority(&tx, 9, "").unwrap();
        tx.commit().unwrap();
        let mirror = read_case_body(&conn, "wide").unwrap();
        assert!(mirror.text().contains(&target));
        assert!(!mirror.text().contains(&old_anchor));
        assert!(mirror.text().contains("18446744073709551615"));
        assert!(mirror.text().contains("\"future\":1.0"));
        let opaque = body.to_string().replace(
            "18446744073709551615",
            "1844674407370955161600000000000000000000",
        );
        let refused = prepare_import_case_with_transfer(
            root.path(),
            &stamp,
            crate::analysis_context::prepare_new_case("opaque").unwrap(),
            &opaque,
            &transfer,
        );
        assert!(refused
            .err()
            .unwrap()
            .starts_with("CASE_EVIDENCE_TRANSFER_OPAQUE"));
        assert_eq!(
            conn.query_row::<u64, _, _>(
                "SELECT count(*) FROM cases WHERE id='opaque'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn adoption_canonicalizes_legacy_effective_fraction_before_native_readiness() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT);CREATE TABLE case_analysis(case_id TEXT PRIMARY KEY,body TEXT);INSERT INTO cases VALUES('legacy','{}');").unwrap();
        let mut original = crate::analysis_context::prepare_new_case("legacy").unwrap();
        original
            .config
            .derived_fields
            .push(json!({"future":"2.547114365375239e-8".parse::<f64>().unwrap()}));
        let raw = serde_json::to_string(&original).unwrap();
        conn.execute("INSERT INTO case_analysis VALUES('legacy',?1)", [&raw])
            .unwrap();
        let legacy = crate::analysis_context::read(&conn, "legacy").unwrap();
        let ordinary: Snapshot = serde_json::from_str(&raw).unwrap();
        assert_eq!(legacy, ordinary);
        let canonical = serde_json::to_string(&legacy).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "UPDATE case_analysis SET body=?1 WHERE case_id='legacy'",
            [&canonical],
        )
        .unwrap();
        tx.execute_batch(
            "CREATE TABLE native_evidence_cases(case_id TEXT PRIMARY KEY,analysis_id TEXT);",
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_evidence_cases VALUES('legacy',?1)",
            [&legacy.analysis_id],
        )
        .unwrap();
        tx.commit().unwrap();
        let native = crate::analysis_context::read(&conn, "legacy").unwrap();
        assert_eq!(
            native.config.derived_fields[0]["future"]
                .as_f64()
                .unwrap()
                .to_bits(),
            legacy.config.derived_fields[0]["future"]
                .as_f64()
                .unwrap()
                .to_bits()
        );
    }
}
