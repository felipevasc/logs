//! Native management publication. Client documents contain authored metadata
//! and checked references only; exact record envelopes never come from IPC.
use super::*;
use crate::case_work_budget::Lease;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

const INVALID: &str =
    "CASE_EVIDENCE_SAVE_INVALID: A evidência ou o documento mudou; reabra o Caso.";
const LIMIT: &str =
    "CASE_EVIDENCE_SAVE_LIMIT: A alteração excede o orçamento; nenhum Caso foi alterado.";
const MAX_CASES: usize = 1000;
const MAX_CONTAINERS: usize = 10_000;
const MIRROR_BYTES: usize = 256 << 20;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportFingerprint {
    pub sha256: String,
    pub bytes: u64,
    pub format: ImportFormat,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImportFormat {
    Json,
    Licase,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptData {
    request_id: String,
    committed_store: StoreStamp,
    current_store: StoreStamp,
    evidence: Vec<EvidenceRef>,
    #[serde(deserialize_with = "native_deserialize_snapshots")]
    analysis_contexts: Vec<crate::analysis_context::Snapshot>,
    case_evidence: Vec<CaseEvidenceState>,
    replayed: bool,
    reconcile_required: bool,
}
impl ReceiptData {
    fn into_receipt(self, current: StoreStamp) -> SaveViewReceipt {
        SaveViewReceipt {
            request_id: self.request_id,
            reconcile_required: self.committed_store != current,
            committed_store: self.committed_store,
            current_store: current,
            evidence: self.evidence,
            analysis_contexts: self.analysis_contexts,
            case_evidence: self.case_evidence,
            replayed: true,
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportData {
    imported_case_id: Option<String>,
    imported_case_ids: Vec<String>,
    #[serde(deserialize_with = "native_deserialize_values")]
    case_views: Vec<Value>,
    receipt: ReceiptData,
}
/// Replay keeps the original committed views alive under their admitted
/// storage credit. Later edits never masquerade as the old import result.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommittedImport {
    pub imported_case_id: Option<String>,
    pub imported_case_ids: Vec<String>,
    pub case_views: Vec<Value>,
    pub receipt: SaveViewReceipt,
    #[serde(skip)]
    _credit: Lease,
}
fn import_fingerprint(
    expected: &StoreStamp,
    request_id: &str,
    input: &ImportFingerprint,
) -> Result<String, String> {
    prepare::request_id(request_id)?;
    if input.sha256.len() != 64
        || !input.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || input.bytes == 0
    {
        return Err(INVALID.into());
    }
    let bytes = serde_json::to_vec(&("native-case-import-v1", expected, request_id, input))
        .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub(crate) fn replay_import(
    root: &Path,
    expected: &StoreStamp,
    request_id: &str,
    input: &ImportFingerprint,
) -> Result<Option<CommittedImport>, String> {
    let hash = import_fingerprint(expected, request_id, input)?;
    let guard = crate::case_recovery::RootLease::shared(root, &prepare::root_work())?;
    let conn = authority::connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let Some(replay) = receipts::lookup(&conn, request_id, &hash, receipts::Kind::Import)? else {
        // A pruned receipt cannot cause recapture: its original CAS must still
        // match before a new import starts preparing immutable files.
        db::require_stamp(&conn, expected)?;
        guard.validate()?;
        return Ok(None);
    };
    if replay.entry.expected_store.as_ref() != Some(expected) {
        return Err(INVALID.into());
    }
    let (entry, credit) = replay.into_parts();
    let value: ImportData = serde_json::from_value(entry.value).map_err(|e| e.to_string())?;
    if value.receipt.request_id != request_id
        || value.imported_case_ids.len() > MAX_CASES
        || value.case_views.len() != value.imported_case_ids.len()
    {
        return Err(INVALID.into());
    }
    let mut ids = HashSet::new();
    for (id, view) in value.imported_case_ids.iter().zip(&value.case_views) {
        if !ids.insert(id) || case_id(view)? != id {
            return Err(INVALID.into());
        }
    }
    if value
        .imported_case_id
        .as_ref()
        .is_some_and(|id| !ids.contains(id))
    {
        return Err(INVALID.into());
    }
    let receipt = value.receipt.into_receipt(db::stamp(&conn)?);
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    guard.validate()?;
    Ok(Some(CommittedImport {
        imported_case_id: value.imported_case_id,
        imported_case_ids: value.imported_case_ids,
        case_views: value.case_views,
        receipt,
        _credit: credit,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmittedContainer {
    kind: String,
    reference: EvidenceReference,
    preserved_count: u32,
    preview: Option<Value>,
}
pub(super) struct SubmittedCase {
    pub(super) metadata: Value,
    pub(super) references: Vec<(ContainerLocation, EvidenceReference)>,
}
pub(super) fn case_id(value: &Value) -> Result<&str, String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control))
        .ok_or_else(|| INVALID.into())
}
fn take_containers(
    object: &mut serde_json::Map<String, Value>,
    item: Option<u32>,
    output: &mut Vec<(ContainerLocation, EvidenceReference)>,
) -> Result<(), String> {
    for key in ["rows", "events"] {
        let Some(value) = object.get_mut(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        // Even an empty inline array is not an issued native container. New
        // authored items omit record members; capture issues their references.
        if !value.is_object() || output.len() >= MAX_CONTAINERS {
            return Err(INVALID.into());
        }
        let submitted: SubmittedContainer =
            serde_json::from_value(value.take()).map_err(|_| INVALID)?;
        if submitted.kind != "native_evidence_container" || submitted.preview.is_some() {
            return Err(INVALID.into());
        }
        let count = match &submitted.reference {
            EvidenceReference::Committed(r) => r.member_count,
            EvidenceReference::Prepared(r) => r.member_count,
        };
        if submitted.preserved_count != count {
            return Err(INVALID.into());
        }
        let location = match (item, key) {
            (None, "rows") => ContainerLocation::CaseRows,
            (None, _) => ContainerLocation::CaseEvents,
            (Some(index), "rows") => ContainerLocation::ItemRows { index },
            (Some(index), _) => ContainerLocation::ItemEvents { index },
        };
        output.push((location, submitted.reference));
    }
    Ok(())
}
pub(super) fn submitted_case(mut value: Value) -> Result<SubmittedCase, String> {
    case_id(&value)?;
    let object = value.as_object_mut().ok_or(INVALID)?;
    if object.contains_key("imageAssets") || object.contains_key(crate::case_archive::TOKEN_FIELD) {
        return Err(INVALID.into());
    }
    // Contexts have their own owner/config/visibility CAS. A stale management
    // view cannot overwrite them through an ordinary note save.
    object.remove("analysisContext");
    let mut references = Vec::new();
    take_containers(object, None, &mut references)?;
    if let Some(items) = object.get_mut("items") {
        let items = items.as_array_mut().ok_or(INVALID)?;
        if items.len() > MAX_CONTAINERS {
            return Err(LIMIT.into());
        }
        for (index, item) in items.iter_mut().enumerate() {
            crate::operations::check()?;
            if let Some(item) = item.as_object_mut() {
                take_containers(item, Some(index as u32), &mut references)?;
            }
        }
    }
    let mut seen = HashSet::new();
    for (_, reference) in &references {
        if !seen.insert(reference.container_id()) {
            return Err(INVALID.into());
        }
    }
    Ok(SubmittedCase {
        metadata: value,
        references,
    })
}

pub(super) fn request_credit(request: &SaveViewRequest) -> Result<Lease, String> {
    prepare::request_id(&request.request_id)?;
    if request.document_json.len() > VIEW_DOCUMENT_BYTES {
        return Err(LIMIT.into());
    }
    let raw = RawJson::checked_with_limit(&request.document_json, VIEW_DOCUMENT_BYTES)?;
    let bytes = decode::preflight_metadata(raw)?
        .materialization_credit
        .checked_add(request.document_json.len().checked_mul(2).ok_or(LIMIT)?)
        .ok_or(LIMIT)?;
    crate::case_cache::reserve_work(crate::case_work_budget::global(), bytes)
}
fn fingerprint(
    request: &SaveViewRequest,
    import: Option<&ImportFingerprint>,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"native-case-publication-v1\0");
    let prefix = serde_json::to_vec(&(&request.request_id, &request.expected_store, import))
        .map_err(|e| e.to_string())?;
    hash.update((prefix.len() as u64).to_le_bytes());
    hash.update(prefix);
    // Preserve the exact authored request on a save retry. Import replay has
    // its separate pre-preparation fingerprint and stores its original views.
    hash.update(request.document_json.as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}
fn connect_write(root: &Path) -> Result<Connection, String> {
    let path = root.join("investigations.sqlite3");
    let before = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let after = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !after.is_file() || after.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(INVALID.into());
        }
    }
    conn.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

struct Existing {
    owner: EvidenceOwner,
    position: i64,
}
fn existing(conn: &Connection, store: &StoreStamp) -> Result<BTreeMap<String, Existing>, String> {
    let mut stmt = conn.prepare("SELECT CASE WHEN octet_length(c.id)<=4096 THEN c.id END,c.position,CASE WHEN octet_length(n.analysis_id)=36 THEN n.analysis_id END FROM cases c LEFT JOIN native_evidence_cases n ON n.case_id=c.id ORDER BY c.position,c.id")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut result = BTreeMap::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        if result.len() >= MAX_CASES {
            return Err(LIMIT.into());
        }
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let position = row.get(1).map_err(|e| e.to_string())?;
        let analysis_id: String = row.get(2).map_err(|_| "CASE_EVIDENCE_MIGRATION_REQUIRED")?;
        uuid::Uuid::parse_str(&analysis_id).map_err(|_| INVALID)?;
        result.insert(
            id.clone(),
            Existing {
                owner: EvidenceOwner {
                    store_id: store.store_id.clone(),
                    case_id: id,
                    analysis_id,
                },
                position,
            },
        );
    }
    Ok(result)
}

struct StagedCase {
    id: String,
    owner: EvidenceOwner,
    position: i64,
    metadata: String,
    bindings: String,
    signature: String,
    state: CaseEvidenceState,
    references: Vec<EvidenceRef>,
    mirror: Option<body::StagedMirror>,
    new_context: Option<crate::analysis_context::Snapshot>,
    pending: Vec<Arc<prepare::Pending>>,
    verified_pending: Vec<Arc<VerifiedContainer>>,
    preserve: bool,
    _credit: Lease,
    _pending_credit: Lease,
}
fn encoded<T: Serialize>(value: &T, limit: usize, credit: &mut Lease) -> Result<String, String> {
    let bytes = view::json_size(value, limit)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        bytes.checked_mul(2).ok_or(LIMIT)?,
    )?)?;
    serde_json::to_string(value).map_err(|e| e.to_string())
}
fn stage_case(
    conn: &Connection,
    root: &Path,
    store: &StoreStamp,
    value: Value,
    position: i64,
    previous: Option<&Existing>,
) -> Result<StagedCase, String> {
    let id = case_id(&value)?.to_owned();
    if value.get("kind").and_then(Value::as_str) == Some("preserved_case_unavailable") {
        let previous = previous.ok_or(INVALID)?;
        let (current, state, credit) = view::case_view(conn, root, &previous.owner)?;
        if current != value || !matches!(state, CaseEvidenceState::Unavailable(_)) {
            return Err(INVALID.into());
        }
        return Ok(StagedCase {
            id,
            owner: previous.owner.clone(),
            position,
            metadata: String::new(),
            bindings: String::new(),
            signature: String::new(),
            state,
            references: Vec::new(),
            mirror: None,
            new_context: None,
            pending: Vec::new(),
            verified_pending: Vec::new(),
            preserve: true,
            _credit: credit,
            _pending_credit: crate::case_cache::reserve_work(crate::case_work_budget::global(), 0)?,
        });
    }
    let submitted = submitted_case(value)?;
    let (owner, context) = match previous {
        Some(previous) => (previous.owner.clone(), None),
        None => {
            if !submitted.references.is_empty() {
                return Err(INVALID.into());
            }
            let context = crate::analysis_context::prepare_new_case(&id)?;
            (
                EvidenceOwner {
                    store_id: store.store_id.clone(),
                    case_id: id.clone(),
                    analysis_id: context.analysis_id.clone(),
                },
                Some(context),
            )
        }
    };
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 64 << 10)?;
    // Metadata-only reads do not allocate all of an unrelated station's row
    // manifests. An actual changed mirror opens its references one Case at a time.
    let previous_case = if previous.is_some() {
        Some(authority::read_case(
            conn,
            root,
            &owner,
            authority::OpenScope::Metadata,
        )?)
    } else {
        None
    };
    let current_refs = previous_case
        .as_ref()
        .map(|(case, _)| {
            case.references()
                .map(|(_, r)| (r.container_id.as_str(), r))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut references = Vec::new();
    let mut pending = Vec::new();
    for (location, reference) in submitted.references {
        crate::operations::check()?;
        if reference.owner() != &owner {
            return Err(INVALID.into());
        }
        let committed = match reference {
            EvidenceReference::Committed(reference) => {
                match current_refs.get(reference.container_id.as_str()).copied() {
                    Some(current) if current != &reference => return Err(INVALID.into()),
                    Some(_) => (),
                    None => {
                        // Whole-item Undo can reattach registered immutable
                        // history when the container is absent everywhere.
                        // Attached replacements still require a prepared CAS.
                        authority::manifest(conn, &reference)?;
                    }
                }
                reference
            }
            EvidenceReference::Prepared(reference) => {
                match &reference.purpose {
                    PreparedPurpose::NewContainer {} => {
                        if current_refs.contains_key(reference.container_id.as_str()) {
                            return Err(INVALID.into());
                        }
                    }
                    PreparedPurpose::ReplaceContainer {
                        base_manifest_id,
                        base_manifest_sha256,
                    } => {
                        let base = current_refs
                            .get(reference.container_id.as_str())
                            .ok_or(INVALID)?;
                        if &base.manifest_id != base_manifest_id
                            || &base.manifest_sha256 != base_manifest_sha256
                        {
                            return Err(INVALID.into());
                        }
                    }
                }
                let pinned = prepare::pin(root, &reference)?;
                let committed = pinned.committed_reference().clone();
                pending.push(pinned);
                committed
            }
        };
        references.push((location, committed));
    }
    references.sort_by(|a, b| a.0.cmp(&b.0));
    let metadata_unchanged = previous_case.as_ref().is_some_and(|(case, _)| {
        case.metadata() == &submitted.metadata
            && case.references().eq(references.iter().map(|(l, r)| (l, r)))
    });
    let mut containers = Vec::new();
    let mut verified_pending = Vec::new();
    let mut pending_credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 0)?;
    let mut opened_budget = crate::case_evidence_budget::StorageBudget::begin(0, 0)?;
    if !metadata_unchanged {
        for (location, reference) in &references {
            let container = if let Some(pin) = pending
                .iter()
                .find(|p| p.reference.container_id == reference.container_id)
            {
                let bytes = (reference.member_count as usize)
                    .checked_mul(512)
                    .and_then(|n| n.checked_add((pin.manifest_bytes() as usize).checked_mul(4)?))
                    .and_then(|n| n.checked_add(4 << 20))
                    .ok_or(LIMIT)?;
                pending_credit.merge(crate::case_cache::reserve_work(
                    pending_credit.pool(),
                    bytes,
                )?)?;
                // File copy/checksum/fsync happens here, before BEGIN IMMEDIATE.
                // The returned lease is retained for cheap final stamp checks.
                pin.publish_files(root)?;
                let opened = pin.container()?;
                verified_pending.push(Arc::clone(&opened));
                opened
            } else {
                authority::container(conn, root, reference, &mut opened_budget)?
            };
            containers.push((location.clone(), container));
        }
    }
    let case = VerifiedCase::from_selected_parts(
        submitted.metadata,
        owner.clone(),
        references.clone(),
        containers,
    )?;
    let mut state = case.evidence_state()?;
    if previous.is_some() {
        credit.merge(anchor_store::decorate(conn, &owner, &mut state)?)?;
    }
    let signature = match &state {
        CaseEvidenceState::Ready(v) => v.evidence_signature.clone(),
        _ => return Err(INVALID.into()),
    };
    let mirror = if metadata_unchanged {
        None
    } else {
        Some(body::stage_mirror(root, |sink| case.write_to(sink, true))?)
    };
    let metadata = encoded(case.metadata(), VIEW_DOCUMENT_BYTES, &mut credit)?;
    let binding_values = references
        .into_iter()
        .map(|(location, reference)| authority::Binding {
            location,
            reference,
        })
        .collect::<Vec<_>>();
    let references = binding_values.iter().map(|v| v.reference.clone()).collect();
    let bindings = encoded(&binding_values, MANIFEST_BYTES as usize, &mut credit)?;
    // Opened rows/manifests can now be dropped. The staged compatibility mirror
    // and immutable file descriptors in the pending pins own publication work.
    Ok(StagedCase {
        id,
        owner,
        position,
        metadata,
        bindings,
        signature,
        state,
        references,
        mirror,
        new_context: context,
        pending,
        verified_pending,
        preserve: false,
        _credit: credit,
        _pending_credit: pending_credit,
    })
}

fn publish_case(tx: &Transaction<'_>, case: &StagedCase) -> Result<(), String> {
    crate::operations::check()?;
    for pending in &case.pending {
        pending.publish_authority(tx)?;
    }
    if let Some(mirror) = &case.mirror {
        body::write_mirror(tx, &case.id, case.position, mirror)?;
    } else {
        let changed = tx
            .execute(
                "UPDATE cases SET position=?2 WHERE id=?1",
                params![case.id, case.position],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err(INVALID.into());
        }
    }
    if case.preserve {
        return Ok(());
    }
    if let Some(context) = &case.new_context {
        crate::analysis_context::insert_portable_snapshot(tx, context)?;
        anchor_store::insert(
            tx,
            &case.owner,
            &crate::case_evidence_anchors::AnchorMap::default(),
        )?;
    }
    let hash = match &case.mirror {
        Some(mirror) => mirror.sha256().to_owned(),
        None => tx
            .query_row(
                "SELECT body_sha256 FROM native_evidence_cases WHERE case_id=?1",
                [&case.id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?,
    };
    tx.execute("INSERT INTO native_evidence_cases(case_id,analysis_id,metadata,bindings,evidence_signature,body_sha256,recovery_id) VALUES(?1,?2,?3,?4,?5,?6,'') ON CONFLICT(case_id) DO UPDATE SET analysis_id=excluded.analysis_id,metadata=excluded.metadata,bindings=excluded.bindings,evidence_signature=excluded.evidence_signature,body_sha256=excluded.body_sha256",
        params![case.id,case.owner.analysis_id,case.metadata,case.bindings,case.signature,hash]).map_err(|e|e.to_string())?;
    db::protect(tx, &case.owner)
}

/// Wire-identical to SaveViewReceipt, retaining its admitted allocation until
/// the command serializer finishes. No Event or preview enters the receipt.
pub(crate) struct PublishedSave {
    pub receipt: SaveViewReceipt,
    _credit: Lease,
}
impl Serialize for PublishedSave {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.receipt.serialize(serializer)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptBorrow<'a> {
    request_id: &'a str,
    committed_store: &'a StoreStamp,
    current_store: &'a StoreStamp,
    evidence: Vec<&'a EvidenceRef>,
    analysis_contexts: Vec<&'a crate::analysis_context::Snapshot>,
    case_evidence: Vec<&'a CaseEvidenceState>,
    replayed: bool,
    reconcile_required: bool,
}
fn prepared_receipt(
    request: &SaveViewRequest,
    stamp: &StoreStamp,
    cases: &[StagedCase],
    imported: &[PreparedCase],
) -> Result<(PublishedSave, Value), String> {
    let count = cases
        .iter()
        .try_fold(0usize, |n, c| {
            n.checked_add(c.references.len()).ok_or(LIMIT)
        })?
        .checked_add(imported.iter().map(|c| c.bindings().len()).sum::<usize>())
        .ok_or(LIMIT)?;
    let mut credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        count
            .checked_mul(32)
            .and_then(|n| n.checked_add(cases.len().checked_mul(256)?))
            .ok_or(LIMIT)?,
    )?;
    let borrowed = ReceiptBorrow {
        request_id: &request.request_id,
        committed_store: stamp,
        current_store: stamp,
        evidence: cases
            .iter()
            .flat_map(|c| c.references.iter())
            .chain(
                imported
                    .iter()
                    .flat_map(|c| c.bindings().iter().map(|b| &b.reference)),
            )
            .collect(),
        analysis_contexts: cases
            .iter()
            .filter_map(|c| c.new_context.as_ref())
            .chain(
                imported
                    .iter()
                    .filter(|c| matches!(c.evidence(), CaseEvidenceState::Ready(_)))
                    .map(|c| c.snapshot()),
            )
            .collect(),
        case_evidence: cases
            .iter()
            .map(|c| &c.state)
            .chain(imported.iter().map(|c| c.evidence()))
            .collect(),
        replayed: false,
        reconcile_required: false,
    };
    let bytes = view::json_size(&borrowed, VIEW_DOCUMENT_BYTES)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        bytes.checked_mul(2).ok_or(LIMIT)?,
    )?)?;
    let text = serde_json::to_string(&borrowed).map_err(|e| e.to_string())?;
    let raw = RawJson::checked_with_limit(&text, VIEW_DOCUMENT_BYTES)?;
    let plan = decode::preflight_metadata(raw)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
    )?)?;
    let value = decode::MetadataDecoder::new().value(raw)?;
    let parsed: ReceiptData = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let mut receipt = parsed.into_receipt(stamp.clone());
    receipt.replayed = false;
    Ok((
        PublishedSave {
            receipt,
            _credit: credit,
        },
        value,
    ))
}
fn next_stamp(current: &StoreStamp) -> Result<StoreStamp, String> {
    let revision = current.revision.parse::<u64>().map_err(|_| INVALID)?;
    if revision.to_string() != current.revision {
        return Err(INVALID.into());
    }
    Ok(StoreStamp {
        revision: revision.checked_add(1).ok_or(LIMIT)?.to_string(),
        ..current.clone()
    })
}

/// Count the actual post-write management envelope on this transaction, one
/// Case at a time. Failure rolls back publication before its receipt commits.
fn validate_view_size(conn: &Connection, root: &Path, limit: usize) -> Result<(), String> {
    let store = db::stamp(conn)?;
    let active:Option<String>=conn.query_row("SELECT CASE WHEN octet_length(value)<=8192 THEN value END FROM metadata WHERE key='active'",[],|row|row.get(0))
        .optional().map_err(|e|e.to_string())?.unwrap_or_else(||Some("null".into()));
    let active = serde_json::from_str(active.as_deref().ok_or(INVALID)?).map_err(|_| INVALID)?;
    let envelope = CaseViewDocument {
        evidence_view_version: SCHEMA_VERSION,
        store: store.clone(),
        active,
        cases: Vec::new(),
        case_evidence: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut total = view::json_size(&envelope, limit)?;
    let mut count = 0usize;
    let mut diagnostics = 0usize;
    let owners = existing(conn, &store)?;
    for prior in owners.values() {
        crate::operations::check()?;
        let (case, state, _credit) = view::case_view(conn, root, &prior.owner)?;
        let mut add = view::json_size(&case, limit)?
            .checked_add(view::json_size(&state, limit)?)
            .ok_or(LIMIT)?;
        if count > 0 {
            add = add.checked_add(2).ok_or(LIMIT)?;
        }
        if let CaseEvidenceState::Unavailable(diagnostic) = &state {
            add = add
                .checked_add(view::json_size(diagnostic, limit)?)
                .ok_or(LIMIT)?;
            if diagnostics > 0 {
                add = add.checked_add(1).ok_or(LIMIT)?;
            }
            diagnostics += 1;
        }
        count += 1;
        total = total
            .checked_add(add)
            .filter(|n| *n <= limit)
            .ok_or(LIMIT)?;
    }
    Ok(())
}

struct StagedDocument {
    cases: Vec<StagedCase>,
    ids: HashSet<String>,
    existing: BTreeMap<String, Existing>,
    active: Option<String>,
    mirror_bytes: usize,
}
fn stage_document(
    conn: &Connection,
    root: &Path,
    request: &SaveViewRequest,
) -> Result<StagedDocument, String> {
    let mut document = request.parse_document()?;
    if document.evidence_view_version != SCHEMA_VERSION || document.cases.len() > MAX_CASES {
        return Err(INVALID.into());
    }
    let mut image_document = Value::Object(serde_json::Map::from_iter([(
        "cases".into(),
        Value::Array(std::mem::take(&mut document.cases)),
    )]));
    crate::case_images::references(&image_document)?;
    document.cases = match image_document
        .as_object_mut()
        .and_then(|map| map.remove("cases"))
    {
        Some(Value::Array(cases)) => cases,
        _ => return Err(INVALID.into()),
    };
    let existing = existing(conn, &request.expected_store)?;
    let mut ids = HashSet::new();
    let mut cases = Vec::new();
    let mut mirror_bytes = 0usize;
    for (position, value) in document.cases.into_iter().enumerate() {
        crate::operations::check()?;
        let id = case_id(&value)?.to_owned();
        if !ids.insert(id.clone()) {
            return Err(INVALID.into());
        }
        let case = stage_case(
            conn,
            root,
            &request.expected_store,
            value,
            position as i64,
            existing.get(&id),
        )?;
        mirror_bytes = mirror_bytes
            .checked_add(case.mirror.as_ref().map_or(0, |m| m.bytes()))
            .ok_or(LIMIT)?;
        if mirror_bytes > MIRROR_BYTES {
            return Err(LIMIT.into());
        }
        cases.push(case);
    }
    if document.active.as_ref().is_some_and(|id| !ids.contains(id)) {
        return Err(INVALID.into());
    }
    for (id, prior) in &existing {
        if !ids.contains(id) {
            let (_, state, _credit) = view::case_view(conn, root, &prior.owner)?;
            if matches!(state, CaseEvidenceState::Unavailable(_)) {
                return Err("CASE_EVIDENCE_PRESERVED_CASE: Preserve a entrada indisponível ao salvar outros Casos.".into());
            }
        }
    }
    Ok(StagedDocument {
        cases,
        ids,
        existing,
        active: document.active,
        mirror_bytes,
    })
}

pub(crate) fn save_view(root: &Path, request: &SaveViewRequest) -> Result<PublishedSave, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let _serial = prepare::serialized_guard()?;
    let _request_credit = request_credit(request)?;
    let hash = fingerprint(request, None)?;
    let conn = authority::connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if let Some(replay) = receipts::lookup(&conn, &request.request_id, &hash, receipts::Kind::Save)?
    {
        let current = db::stamp(&conn)?;
        let (entry, credit) = replay.into_parts();
        if entry.expected_store.as_ref() != Some(&request.expected_store) {
            return Err(INVALID.into());
        }
        let saved: ReceiptData = serde_json::from_value(entry.value).map_err(|e| e.to_string())?;
        if saved.request_id != request.request_id {
            return Err(INVALID.into());
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        root_guard.validate()?;
        return Ok(PublishedSave {
            receipt: saved.into_receipt(current),
            _credit: credit,
        });
    }
    db::require_stamp(&conn, &request.expected_store)?;
    let StagedDocument {
        cases,
        ids,
        existing,
        active,
        ..
    } = stage_document(&conn, &root, request)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let stamp = next_stamp(&request.expected_store)?;
    let (response, value) = prepared_receipt(request, &stamp, &cases, &[])?;
    let mut permits = existing
        .keys()
        .cloned()
        .chain(ids.iter().cloned())
        .collect::<Vec<_>>();
    permits.sort();
    permits.dedup();
    let tokens = cases
        .iter()
        .flat_map(|c| c.pending.iter().map(|p| p.reference.token.clone()))
        .collect::<Vec<_>>();
    let mut conn = connect_write(&root)?;
    db::transaction(
        &mut conn,
        &permits,
        |tx| {
            root_guard.validate()?;
            db::require_stamp(tx, &request.expected_store)?;
            for previous in existing.values() {
                authority::require_owner(tx, &previous.owner)?;
            }
            for case in &cases {
                for container in &case.verified_pending {
                    container.validate()?;
                }
            }
            Ok(())
        },
        |tx| {
            for case in &cases {
                publish_case(tx, case)?;
            }
            for id in existing.keys().filter(|id| !ids.contains(*id)) {
                crate::operations::check()?;
                tx.execute("DELETE FROM native_evidence_cases WHERE case_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM case_analysis WHERE case_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM cases WHERE id=?1", [id])
                    .map_err(|e| e.to_string())?;
                // Immutable payloads and protected owner tombstones survive deletion.
            }
            tx.execute(
                "UPDATE metadata SET value=?1 WHERE key='revision'",
                [&stamp.revision],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO metadata(key,value) VALUES('active',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(&active).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
            validate_view_size(tx, &root, VIEW_DOCUMENT_BYTES)?;
            receipts::insert(
                tx,
                &request.request_id,
                &hash,
                &receipts::Entry {
                    version: 1,
                    kind: receipts::Kind::Save,
                    expected_store: Some(request.expected_store.clone()),
                    publication_store: stamp.identity(),
                    value,
                },
            )
        },
    )?;
    prepare::retire(&tokens);
    Ok(response)
}

fn import_response(
    request: &SaveViewRequest,
    stamp: &StoreStamp,
    cases: &[StagedCase],
    imported: &[PreparedCase],
    selected: Option<&str>,
) -> Result<(CommittedImport, Value), String> {
    let (saved, _) = prepared_receipt(request, stamp, cases, imported)?;
    let mut views = Vec::new();
    for prepared in imported {
        views.push(prepared.committed_view()?);
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Borrow<'a> {
        imported_case_id: Option<&'a str>,
        imported_case_ids: Vec<&'a str>,
        case_views: Vec<&'a Value>,
        receipt: &'a SaveViewReceipt,
    }
    let borrowed = Borrow {
        imported_case_id: selected,
        imported_case_ids: imported
            .iter()
            .map(|c| c.owner().case_id.as_str())
            .collect(),
        case_views: views.iter().map(|v| v.value()).collect(),
        receipt: &saved.receipt,
    };
    let bytes = view::json_size(&borrowed, VIEW_DOCUMENT_BYTES)?;
    let mut credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(imported.len() * 128))
            .ok_or(LIMIT)?,
    )?;
    let text = serde_json::to_string(&borrowed).map_err(|e| e.to_string())?;
    let raw = RawJson::checked_with_limit(&text, VIEW_DOCUMENT_BYTES)?;
    let plan = decode::preflight_metadata(raw)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
    )?)?;
    let value = decode::MetadataDecoder::new().value(raw)?;
    let parsed: ImportData = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let mut receipt = parsed.receipt.into_receipt(stamp.clone());
    receipt.replayed = false;
    Ok((
        CommittedImport {
            imported_case_id: parsed.imported_case_id,
            imported_case_ids: parsed.imported_case_ids,
            case_views: parsed.case_views,
            receipt,
            _credit: credit,
        },
        value,
    ))
}

/// Append fresh native Cases and their sidecar ownership in one transaction.
/// The command has only store/path/requestId inputs. Its management document
/// must be the unchanged authoritative view, and selection comes from the
/// hashed file. Input hashing/replay precedes fresh IDs and is rechecked here.
pub(crate) fn commit_import(
    root: &Path,
    request: &SaveViewRequest,
    imported: &[PreparedCase],
    input: &ImportFingerprint,
    selected_case_id: Option<&str>,
    publish_sidecars: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
) -> Result<CommittedImport, String> {
    let hash = import_fingerprint(&request.expected_store, &request.request_id, input)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let _serial = prepare::serialized_guard()?;
    if let Some(replay) = replay_import(&root, &request.expected_store, &request.request_id, input)?
    {
        return Ok(replay);
    }
    let _request_credit = request_credit(request)?;
    if imported.is_empty() || imported.len() > MAX_CASES {
        return Err(INVALID.into());
    }
    let conn = authority::connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    db::require_stamp(&conn, &request.expected_store)?;
    let StagedDocument {
        cases,
        mut ids,
        existing,
        active,
        mut mirror_bytes,
    } = stage_document(&conn, &root, request)?;
    // Import is deliberately append-only. Authored edits use save_view first;
    // they cannot be smuggled into an import whose replay key has no document.
    let mut ordered = existing.iter().collect::<Vec<_>>();
    ordered.sort_by(|(id, a), (other, b)| (a.position, *id).cmp(&(b.position, *other)));
    if cases.len() != existing.len()
        || cases.iter().zip(&ordered).any(|(case, (id, _))| {
            &case.id != *id
                || case.mirror.is_some()
                || case.new_context.is_some()
                || !case.pending.is_empty()
        })
    {
        return Err(
            "CASE_EVIDENCE_IMPORT_VIEW_CHANGED: Salve as alterações do Caso antes de importar."
                .into(),
        );
    }
    let current_active:Option<String> = conn.query_row("SELECT CASE WHEN octet_length(value)<=8192 THEN value END FROM metadata WHERE key='active'",[],|r|r.get(0))
        .optional().map_err(|e|e.to_string())?.unwrap_or_else(||Some("null".into()));
    let current_active: Option<String> =
        serde_json::from_str(current_active.as_deref().ok_or(INVALID)?).map_err(|_| INVALID)?;
    if active != current_active {
        return Err(INVALID.into());
    }
    let first_position = ordered.last().map_or(Ok(0), |(_, value)| {
        value.position.checked_add(1).ok_or(LIMIT)
    })?;
    if cases
        .len()
        .checked_add(imported.len())
        .is_none_or(|n| n > MAX_CASES)
    {
        return Err(LIMIT.into());
    }
    let mut imported_ids = HashSet::new();
    for prepared in imported {
        crate::operations::check()?;
        if prepared.expected_store() != &request.expected_store
            || prepared.owner().store_id != request.expected_store.store_id
            || existing.contains_key(&prepared.owner().case_id)
            || !ids.insert(prepared.owner().case_id.clone())
        {
            return Err(INVALID.into());
        }
        imported_ids.insert(prepared.owner().case_id.as_str());
        prepared.validate(&root)?;
        mirror_bytes = mirror_bytes
            .checked_add(prepared.mirror_bytes())
            .ok_or(LIMIT)?;
        if mirror_bytes > MIRROR_BYTES {
            return Err(LIMIT.into());
        }
    }
    if selected_case_id.is_some_and(|id| !imported_ids.contains(id)) {
        return Err(INVALID.into());
    }
    let selected =
        selected_case_id.or_else(|| imported.first().map(|c| c.owner().case_id.as_str()));
    let active = selected.map(str::to_owned).or(active);
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let stamp = next_stamp(&request.expected_store)?;
    let (response, value) = import_response(request, &stamp, &cases, imported, selected)?;
    let mut permits = imported
        .iter()
        .map(|c| c.owner().case_id.clone())
        .collect::<Vec<_>>();
    permits.sort();
    permits.dedup();
    let tokens = cases
        .iter()
        .flat_map(|c| c.pending.iter().map(|p| p.reference.token.clone()))
        .collect::<Vec<_>>();
    let mut conn = connect_write(&root)?;
    db::transaction(
        &mut conn,
        &permits,
        |tx| {
            root_guard.validate()?;
            db::require_stamp(tx, &request.expected_store)?;
            for prior in existing.values() {
                authority::require_owner(tx, &prior.owner)?;
            }
            for case in &cases {
                for container in &case.verified_pending {
                    container.validate()?;
                }
            }
            for prepared in imported {
                prepared.validate(&root)?;
                if tx
                    .query_row::<bool, _, _>(
                        "SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",
                        [&prepared.owner().case_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err(INVALID.into());
                }
            }
            Ok(())
        },
        |tx| {
            for (index, prepared) in imported.iter().enumerate() {
                prepared.publish_authority(
                    tx,
                    first_position.checked_add(index as i64).ok_or(LIMIT)?,
                    "",
                )?;
            }
            publish_sidecars(tx)?;
            // The sidecar hook may publish assets, never replace a prepared owner
            // or change the snapshot that the committed import receipt acknowledges.
            for prepared in imported {
                prepared.validate_snapshot(tx)?;
            }
            tx.execute(
                "UPDATE metadata SET value=?1 WHERE key='revision'",
                [&stamp.revision],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO metadata(key,value) VALUES('active',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(&active).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
            validate_view_size(tx, &root, VIEW_DOCUMENT_BYTES)?;
            receipts::insert(
                tx,
                &request.request_id,
                &hash,
                &receipts::Entry {
                    version: 1,
                    kind: receipts::Kind::Import,
                    expected_store: Some(request.expected_store.clone()),
                    publication_store: stamp.identity(),
                    value,
                },
            )
        },
    )?;
    prepare::retire(&tokens);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn inline_rows_previews_and_duplicate_container_aliases_are_rejected() {
        for rows in [
            json!([]),
            json!([{"id":1}]),
            json!({"kind":"evidence_preview"}),
        ] {
            assert!(submitted_case(json!({"id":"c","items":[{"rows":rows}]})).is_err());
        }
        assert!(submitted_case(json!({"id":"c","items":[]})).is_ok());
    }
    #[test]
    fn authored_note_save_strips_only_server_context() {
        let value = json!({"id":"c","note":"keep","unknown":{"value":1.25},"analysisContext":{"analysisId":"stale"},"items":[]});
        let parsed = submitted_case(value).unwrap();
        assert_eq!(parsed.metadata["unknown"]["value"], json!(1.25));
        assert!(parsed.metadata.get("analysisContext").is_none());
    }
    fn request(root: &Path, id: &str, edit: impl FnOnce(&mut Value)) -> SaveViewRequest {
        let loaded = load_view(root).unwrap();
        let expected_store = loaded.document().store.clone();
        let mut value = serde_json::to_value(&loaded).unwrap();
        edit(&mut value);
        SaveViewRequest {
            request_id: id.into(),
            expected_store,
            document_json: value.to_string(),
        }
    }
    fn mirror(root: &Path) -> String {
        let conn = authority::connect_readonly(root).unwrap();
        body::read_case_body(&conn, "c").unwrap().text().to_owned()
    }
    fn exact_fixture() -> (tempfile::TempDir, String) {
        let (root, open, refs) = authority::tests::fixture();
        let owner = refs[0].owner.clone();
        let raw = r#"{ "id":9,"fields":{"a":18446744073709551615,"b":18446744073709551614,"whole":1.00,"negative":-0.0,"fraction":2.547114365375239e-8},"unknownFuture":{"keep":[1.0,"1"]} }"#.to_owned();
        let staged = stage_records(root.path(), &owner, &Value::Null, |sink| {
            sink.push_envelope(&raw)?;
            sink.push_envelope(&raw)
        })
        .unwrap();
        staged.publish_files(root.path()).unwrap();
        let mut conn = connect_write(root.path()).unwrap();
        conn.execute(
            "INSERT INTO native_evidence_batches VALUES(?1,?2)",
            params![
                staged.batch.batch_id,
                serde_json::to_string(&staged.batch).unwrap()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_evidence_manifests VALUES(?1,?2)",
            params![
                staged.reference.manifest_id,
                serde_json::to_string(&db::ManifestMetadata {
                    reference: staged.reference.clone(),
                    bytes: staged.manifest_bytes(),
                    batches: vec![staged.batch.batch_id.clone()]
                })
                .unwrap()
            ],
        )
        .unwrap();
        let text: String = conn
            .query_row(
                "SELECT bindings FROM native_evidence_cases WHERE case_id='c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut bindings: Vec<authority::Binding> = serde_json::from_str(&text).unwrap();
        bindings
            .iter_mut()
            .find(|b| b.location == ContainerLocation::CaseRows)
            .unwrap()
            .reference = staged.reference;
        conn.execute(
            "UPDATE native_evidence_cases SET bindings=?1 WHERE case_id='c'",
            [serde_json::to_string(&bindings).unwrap()],
        )
        .unwrap();
        let (case, _credit) =
            authority::read_case(&conn, root.path(), &owner, authority::OpenScope::All).unwrap();
        let staged = body::stage_mirror(root.path(), |out| case.write_to(out, true)).unwrap();
        db::transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| {
                body::write_mirror(tx, "c", 0, &staged)?;
                tx.execute(
                    "UPDATE native_evidence_cases SET body_sha256=?1 WHERE case_id='c'",
                    [staged.sha256()],
                )
                .map_err(|e| e.to_string())?;
                db::protect(tx, &owner)
            },
        )
        .unwrap();
        drop(open);
        (root, raw)
    }
    #[test]
    fn notes_save_reopen_preserves_exact_envelopes_and_old_writer_barrier() {
        let (root, raw) = exact_fixture();
        let before = mirror(root.path());
        assert_eq!(before.matches(&raw).count(), 2);
        let request = request(root.path(), "notes", |d| {
            d["cases"][0]["note"] = json!("edited")
        });
        let receipt = save_view(root.path(), &request).unwrap();
        assert_eq!(receipt.receipt.committed_store.revision, "2");
        assert_eq!(mirror(root.path()).matches(&raw).count(), 2);
        let view = load_view(root.path()).unwrap();
        assert_eq!(view.document().cases[0]["note"], "edited");
        assert_eq!(view.document().cases[0]["rows"]["preservedCount"], 2);
        let conn = connect_write(root.path()).unwrap();
        assert!(conn
            .execute("UPDATE cases SET body='{}' WHERE id='c'", [])
            .is_err());
        assert!(conn.execute("DELETE FROM cases WHERE id='c'", []).is_err());
        assert_eq!(
            conn.query_row::<u64, _, _>("SELECT count(*) FROM native_evidence_permits", [], |r| r
                .get(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn save_replay_keeps_original_receipt_and_reports_later_state() {
        let (root, raw) = exact_fixture();
        let a = request(root.path(), "a", |d| d["cases"][0]["note"] = json!("first"));
        let first = save_view(root.path(), &a).unwrap();
        let replay = save_view(root.path(), &a).unwrap();
        assert!(replay.receipt.replayed);
        assert!(!replay.receipt.reconcile_required);
        assert_eq!(
            first.receipt.committed_store,
            replay.receipt.committed_store
        );
        let b = request(root.path(), "b", |d| {
            d["cases"][0]["note"] = json!("second")
        });
        save_view(root.path(), &b).unwrap();
        let replay = save_view(root.path(), &a).unwrap();
        assert!(replay.receipt.reconcile_required);
        assert_eq!(replay.receipt.committed_store.revision, "2");
        assert_eq!(replay.receipt.current_store.revision, "3");
        assert!(mirror(root.path()).contains("second"));
        assert_eq!(mirror(root.path()).matches(&raw).count(), 2);
        let mut changed = a;
        changed.document_json.push(' ');
        assert!(save_view(root.path(), &changed).is_err());
    }
    #[test]
    fn literal_private_metadata_survives_notes_save_reopen_and_replay() {
        let (root, _) = exact_fixture();
        let literal = json!({"$serde_json::private::RawValue":"1","nested":[{"$serde_json::private::Number":"123"},{"$serde_json::private::RawValue":"null"}]});
        let save = request(root.path(), "literal-metadata", |document| {
            document["cases"][0]["futureMetadata"] = literal.clone();
            document["cases"][0]["note"] = json!("preserved note");
        });
        let first = save_view(root.path(), &save).unwrap();
        let replay = save_view(root.path(), &save).unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.receipt.committed_store, first.receipt.committed_store);
        let loaded = load_view(root.path()).unwrap();
        assert_eq!(loaded.document().cases[0]["futureMetadata"], literal);
        let text = mirror(root.path());
        let raw = checked_document(&text).unwrap();
        assert_eq!(materialize_value(raw, preflight_value(raw).unwrap()).unwrap()["futureMetadata"], literal);
    }
    #[test]
    fn sql_failure_rolls_back_mirror_authority_receipt_and_permits_then_retry_succeeds() {
        let (root, _) = exact_fixture();
        let before = mirror(root.path());
        let conn = connect_write(root.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_native_receipt BEFORE INSERT ON native_evidence_receipts BEGIN SELECT RAISE(ABORT,'test publication failure'); END;").unwrap();
        let request = request(root.path(), "fault", |d| {
            d["cases"][0]["note"] = json!("new")
        });
        assert!(save_view(root.path(), &request).is_err());
        assert_eq!(mirror(root.path()), before);
        assert_eq!(db::stamp(&conn).unwrap().revision, "1");
        assert_eq!(
            conn.query_row::<u64, _, _>("SELECT count(*) FROM native_evidence_permits", [], |r| r
                .get(0))
                .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER fail_native_receipt")
            .unwrap();
        assert_eq!(
            save_view(root.path(), &request)
                .unwrap()
                .receipt
                .committed_store
                .revision,
            "2"
        );
    }
    #[test]
    fn stale_document_or_forged_container_cannot_replace_exact_rows() {
        let (root, _) = exact_fixture();
        let stale = request(root.path(), "stale", |_| {});
        let forged = request(root.path(), "forged", |d| {
            d["cases"][0]["rows"]["reference"]["manifestSha256"] = json!("f".repeat(64))
        });
        let before = mirror(root.path());
        assert!(save_view(root.path(), &forged).is_err());
        assert_eq!(mirror(root.path()), before);
        let fresh = request(root.path(), "fresh", |d| {
            d["cases"][0]["note"] = json!("one")
        });
        save_view(root.path(), &fresh).unwrap();
        assert!(save_view(root.path(), &stale).is_err());
    }
    #[test]
    fn native_import_is_atomic_and_replays_original_ids_and_views_after_later_edits() {
        let (root, _) = exact_fixture();
        let save = request(root.path(), "import-atomic", |_| {});
        let source = r#"{"id":"foreign","name":"Imported","futureMetadata":{"$serde_json::private::RawValue":"1","nested":[{"$serde_json::private::RawValue":"null"}]},"items":[{"id":"i","rows":[{"id":1,"fields":{"exact":18446744073709551615,"typed":1.0},"future":[-0.0,1e-40]}]}]}"#;
        let mut snapshot = crate::analysis_context::prepare_new_case("imported-local").unwrap();
        snapshot.legacy_raw = Some(json!({"$serde_json::private::RawValue":"1","sibling":2}));
        let prepared =
            prepare_import_case(root.path(), &save.expected_store, snapshot, source).unwrap();
        let input = ImportFingerprint {
            sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
            bytes: source.len() as u64,
            format: ImportFormat::Json,
        };
        assert!(
            replay_import(root.path(), &save.expected_store, &save.request_id, &input)
                .unwrap()
                .is_none()
        );
        let prepared = [prepared];
        let conn = connect_write(root.path()).unwrap();
        let changed_document = request(root.path(), "import-atomic", |d| {
            d["cases"][0]["note"] = json!("not part of import")
        });
        assert!(commit_import(
            root.path(),
            &changed_document,
            &prepared,
            &input,
            Some("imported-local"),
            |_| Ok(())
        )
        .is_err());
        assert_eq!(db::stamp(&conn).unwrap(), save.expected_store);
        conn.execute_batch("CREATE TABLE test_native_sidecars(id TEXT PRIMARY KEY)")
            .unwrap();
        assert!(commit_import(
            root.path(),
            &save,
            &prepared,
            &input,
            Some("imported-local"),
            |tx| {
                tx.execute("INSERT INTO test_native_sidecars VALUES('asset')", [])
                    .map_err(|e| e.to_string())?;
                Err("sidecar publication failure".into())
            }
        )
        .is_err());
        assert_eq!(db::stamp(&conn).unwrap(), save.expected_store);
        assert_eq!(
            conn.query_row::<u64, _, _>("SELECT count(*) FROM cases", [], |r| r.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<u64, _, _>("SELECT count(*) FROM test_native_sidecars", [], |r| r
                .get(0))
                .unwrap(),
            0
        );
        let first = commit_import(
            root.path(),
            &save,
            &prepared,
            &input,
            Some("imported-local"),
            |tx| {
                tx.execute("INSERT INTO test_native_sidecars VALUES('asset')", [])
                    .map_err(|e| e.to_string())?;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(first.imported_case_ids, vec!["imported-local"]);
        assert_eq!(first.case_views[0]["name"], "Imported");
        assert_eq!(first.case_views[0]["futureMetadata"]["$serde_json::private::RawValue"], "1");
        assert_eq!(first.receipt.analysis_contexts.iter().find(|snapshot| snapshot.case_id == "imported-local").unwrap().legacy_raw,
            Some(json!({"$serde_json::private::RawValue":"1","sibling":2})));
        let original = body::read_case_body(&conn, "imported-local").unwrap();
        assert!(original
            .text()
            .contains(r#""exact":18446744073709551615,"typed":1.0"#));
        assert!(original.text().contains(r#""future":[-0.0,1e-40]"#));
        let changed = request(root.path(), "later-note", |d| {
            d["cases"][1]["name"] = json!("Later name")
        });
        save_view(root.path(), &changed).unwrap();
        let replay = replay_import(root.path(), &save.expected_store, &save.request_id, &input)
            .unwrap()
            .unwrap();
        assert!(replay.receipt.replayed && replay.receipt.reconcile_required);
        assert_eq!(replay.case_views[0]["name"], "Imported");
        assert_eq!(replay.case_views[0]["futureMetadata"], first.case_views[0]["futureMetadata"]);
        assert_eq!(replay.receipt.analysis_contexts.iter().find(|snapshot| snapshot.case_id == "imported-local").unwrap().legacy_raw,
            Some(json!({"$serde_json::private::RawValue":"1","sibling":2})));
        assert_eq!(replay.imported_case_ids, first.imported_case_ids);
        let other = ImportFingerprint {
            sha256: "f".repeat(64),
            ..input
        };
        assert!(
            replay_import(root.path(), &save.expected_store, &save.request_id, &other).is_err()
        );
    }
    #[test]
    fn empty_new_case_receives_its_own_context_and_deleted_id_stays_protected() {
        let (root, _) = exact_fixture();
        let save = request(root.path(), "new-case", |d| {
            d["cases"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"new","items":[],"note":"fresh"}))
        });
        let result = save_view(root.path(), &save).unwrap();
        assert_eq!(result.receipt.analysis_contexts.len(), 1);
        assert_eq!(result.receipt.analysis_contexts[0].case_id, "new");
        let remove = request(root.path(), "delete-new", |d| {
            d["cases"]
                .as_array_mut()
                .unwrap()
                .retain(|c| c["id"] != "new")
        });
        save_view(root.path(), &remove).unwrap();
        let conn = connect_write(root.path()).unwrap();
        assert!(conn
            .execute("INSERT INTO cases VALUES('new','{}',1)", [])
            .is_err());
        assert_eq!(
            conn.query_row::<u64, _, _>(
                "SELECT count(*) FROM native_evidence_protected WHERE case_id='new'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn fresh_capture_publication_precedes_sql_and_survives_failed_save_retry() {
        let (root, _) = exact_fixture();
        let conn = connect_write(root.path()).unwrap();
        let store = db::stamp(&conn).unwrap();
        let identity = db::identity_for(&conn, "c").unwrap();
        let owner = EvidenceOwner {
            store_id: store.store_id.clone(),
            case_id: "c".into(),
            analysis_id: identity.analysis_id.clone(),
        };
        let source=serde_json::from_value(json!({"analysisContext":identity,"sourceGeneration":1,"caseKey":null,"caseContentToken":null,"catalogSignature":"a".repeat(64),"catalogEpoch":0})).unwrap();
        let capture = CaptureRequest {
            request_id: "new-capture".into(),
            expected_store: store,
            target: owner,
            source,
            rows: vec![crate::page_projection::RowHandle {
                id: 9,
                event_ref: "captured:9".into(),
            }],
        };
        let raw = r#"{"id":9,"event_ref":"captured:9","fields":{"wide":18446744073709551615,"float":1.0},"future":true}"#;
        let prepared = prepare_capture(
            root.path(),
            &capture,
            |sink| sink.push_envelope(raw),
            || Ok(()),
        )
        .unwrap();
        let manifest = root
            .path()
            .join("evidence-v1")
            .join(format!("{}.manifest", prepared.manifest_id));
        assert!(
            !manifest.exists(),
            "preparation must not publish a Case manifest"
        );
        let save = request(root.path(), "capture-save", |d| {
            d["cases"][0]["items"].as_array_mut().unwrap().push(json!({"id":"new-captured-item","rows":{"kind":"native_evidence_container","reference":prepared,"preservedCount":1,"preview":null}}))
        });
        let before = mirror(root.path());
        conn.execute_batch("CREATE TRIGGER fail_capture_save BEFORE INSERT ON native_evidence_receipts WHEN NEW.request_id='capture-save' BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
        assert!(save_view(root.path(), &save).is_err());
        assert!(
            manifest.exists(),
            "immutable files were staged before the failed transaction"
        );
        assert_eq!(mirror(root.path()), before);
        assert!(prepare::pin(root.path(), &prepared).is_ok());
        assert_eq!(db::stamp(&conn).unwrap().revision, "1");
        conn.execute_batch("DROP TRIGGER fail_capture_save")
            .unwrap();
        let result = save_view(root.path(), &save).unwrap();
        assert_eq!(result.receipt.committed_store.revision, "2");
        assert!(mirror(root.path()).contains(raw));
        assert!(prepare::pin(root.path(), &prepared).is_err());
        assert!(save_view(root.path(), &save).unwrap().receipt.replayed);
    }
    #[test]
    fn removed_item_undo_reattaches_only_same_owner_registered_history() {
        let (root, _) = exact_fixture();
        let loaded = load_view(root.path()).unwrap();
        let removed = loaded.document().cases[0]["items"][0].clone();
        let conn = connect_write(root.path()).unwrap();
        let visibility = db::identity_for(&conn, "c").unwrap();
        let remove = request(root.path(), "remove-item", |d| {
            d["cases"][0]["items"].as_array_mut().unwrap().remove(0);
        });
        save_view(root.path(), &remove).unwrap();
        let stale = request(root.path(), "stale-undo", |d| {
            d["cases"][0]["items"]
                .as_array_mut()
                .unwrap()
                .insert(0, removed.clone())
        });
        let note = request(root.path(), "intervening-note", |d| {
            d["cases"][0]["note"] = json!("keep")
        });
        save_view(root.path(), &note).unwrap();
        assert!(save_view(root.path(), &stale).is_err());
        let bad = request(root.path(), "foreign-undo", |d| {
            let mut item = removed.clone();
            item["rows"]["reference"]["owner"]["analysisId"] =
                json!(uuid::Uuid::new_v4().to_string());
            d["cases"][0]["items"]
                .as_array_mut()
                .unwrap()
                .insert(0, item);
        });
        assert!(save_view(root.path(), &bad).is_err());
        let undo = request(root.path(), "undo-item", |d| {
            d["cases"][0]["items"]
                .as_array_mut()
                .unwrap()
                .insert(0, removed.clone())
        });
        save_view(root.path(), &undo).unwrap();
        let restored = load_view(root.path()).unwrap();
        assert_eq!(restored.document().cases[0]["items"][0], removed);
        assert_eq!(restored.document().cases[0]["note"], "keep");
        assert_eq!(
            db::identity_for(&conn, "c").unwrap(),
            visibility,
            "reattachment cannot restore or reset exclusion/config state"
        );
        let duplicate = request(root.path(), "duplicate-location", |d| {
            d["cases"][0]["items"]
                .as_array_mut()
                .unwrap()
                .push(removed.clone())
        });
        assert!(save_view(root.path(), &duplicate).is_err());
    }
}
