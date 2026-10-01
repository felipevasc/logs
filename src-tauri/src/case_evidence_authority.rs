//! Read-only native Case authority. Body/note revision does not participate in
//! analytical signatures; live store epoch, owner and membership always do.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const INVALID: &str =
    "CASE_EVIDENCE_AUTHORITY: O Caso ou sua evidência mudou; reabra a visualização.";
const UNAVAILABLE:&str="CASE_EVIDENCE_UNAVAILABLE: Os registros foram preservados, mas esta visualização não está disponível.";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub location: ContainerLocation,
    pub reference: EvidenceRef,
}

pub(super) fn connect_readonly(root: &Path) -> Result<Connection, String> {
    connect_checked(root, false)
}
pub(super) fn connect_readwrite(root: &Path) -> Result<Connection, String> {
    connect_checked(root, true)
}
fn connect_checked(root: &Path, writable: bool) -> Result<Connection, String> {
    crate::operations::check()?;
    let path = root.join("investigations.sqlite3");
    // Never open/close an ordinary File on an active SQLite database: POSIX
    // record locks belong to the process and that close can release another
    // connection's locks. Let SQLite's VFS own every live database handle.
    let before = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    let conn = Connection::open_with_flags(
        &path,
        (if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        }) | OpenFlags::SQLITE_OPEN_NO_MUTEX
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
    conn.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}
/// Historical display metadata uses the complete body StoreStamp, separate
/// from analytical evidence identity. Never initializes or migrates a profile.
pub(crate) fn validate_view_stamp(root: &Path, expected: &StoreStamp) -> Result<(), String> {
    let guard = crate::case_recovery::RootLease::shared(root, &prepare::root_work())?;
    let conn = connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    db::require_stamp(&conn, expected)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    guard.validate()
}

/// Core legacy transports must refuse native or restored authority before
/// retrieving a body. The caller holds its read/write transaction already.
pub(crate) fn require_legacy_store(conn: &Connection) -> Result<(), String> {
    let denied =
        "CASE_NATIVE_EVIDENCE_REQUIRED: Use a visualização e publicação de evidências nativas.";
    if db::exists(conn, "native_evidence_store")? {
        return Err(denied.into());
    }
    for table in ["native_evidence_protected", "case_recovery_protected"] {
        if db::exists(conn, table)?
            && conn
                .query_row::<bool, _, _>(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?
        {
            return Err(denied.into());
        }
    }
    Ok(())
}
/// No migrating connector or Event/Value body read. Admission callers use this
/// before accepting an inline legacy publication for an adopted owner.
pub(crate) fn native_owner(
    root: &Path,
    expected: &crate::analysis_context::Identity,
) -> Result<bool, String> {
    let guard = crate::case_recovery::RootLease::shared(root, &prepare::root_work())?;
    let conn = connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let native = if db::exists(&conn, "native_evidence_cases")? {
        conn.query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM native_evidence_cases WHERE case_id=?1)",
            [&expected.case_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?
    } else {
        false
    };
    if native && db::identity_for(&conn, &expected.case_id)? != *expected {
        return Err(INVALID.into());
    }
    let mut protected = false;
    if !native {
        for table in ["native_evidence_protected", "case_recovery_protected"] {
            if db::exists(&conn, table)? {
                protected |= conn
                    .query_row::<bool, _, _>(
                        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE case_id=?1)"),
                        [&expected.case_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    guard.validate()?;
    Ok(native || protected)
}

fn current_owner(conn: &Connection, case_id: &str) -> Result<(String, String), String> {
    let current:(String,String)=conn.query_row("SELECT CASE WHEN octet_length(n.analysis_id)=36 THEN n.analysis_id END,CASE WHEN octet_length(n.evidence_signature)=64 THEN n.evidence_signature END FROM native_evidence_cases n JOIN cases c ON c.id=n.case_id WHERE n.case_id=?1",[case_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?.ok_or(INVALID)?;
    // Identity-only decoding skips configuration trees. Avoid json_extract on
    // an unadmitted context body, which could materialize it inside SQLite.
    if db::identity_for(conn, case_id)?.analysis_id != current.0 {
        return Err(INVALID.into());
    }
    Ok(current)
}
pub(super) fn require_owner(conn: &Connection, owner: &EvidenceOwner) -> Result<(), String> {
    if db::stamp(conn)?.store_id != owner.store_id
        || current_owner(conn, &owner.case_id)?.0 != owner.analysis_id
    {
        return Err(INVALID.into());
    }
    Ok(())
}
pub(super) fn batch(conn: &Connection, id: &str) -> Result<BatchRef, String> {
    uuid::Uuid::parse_str(id).map_err(|_| INVALID)?;
    let mut stmt = conn
        .prepare(
            "SELECT octet_length(body),CASE WHEN octet_length(body)<=65536 THEN body END FROM native_evidence_batches WHERE batch_id=?1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([id]).map_err(|e| e.to_string())?;
    let row = rows.next().map_err(|e| e.to_string())?.ok_or(INVALID)?;
    if row.get::<_, u64>(0).map_err(|e| e.to_string())? > 64 << 10 {
        return Err(INVALID.into());
    }
    let text: String = row.get(1).map_err(|e| e.to_string())?;
    let value: BatchRef = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if value.batch_id != id {
        return Err(INVALID.into());
    }
    Ok(value)
}
pub(super) fn manifest(
    conn: &Connection,
    reference: &EvidenceRef,
) -> Result<db::ManifestMetadata, String> {
    uuid::Uuid::parse_str(&reference.manifest_id).map_err(|_| INVALID)?;
    let mut stmt=conn.prepare("SELECT octet_length(body),CASE WHEN octet_length(body)<=65536 THEN body END FROM native_evidence_manifests WHERE manifest_id=?1").map_err(|e|e.to_string())?;
    let mut rows = stmt
        .query([&reference.manifest_id])
        .map_err(|e| e.to_string())?;
    let row = rows.next().map_err(|e| e.to_string())?.ok_or(INVALID)?;
    if row.get::<_, u64>(0).map_err(|e| e.to_string())? > 64 << 10 {
        return Err(INVALID.into());
    }
    let text: String = row.get(1).map_err(|e| e.to_string())?;
    let metadata: db::ManifestMetadata = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if metadata.reference != *reference
        || metadata.bytes > MANIFEST_BYTES
        || metadata.batches.len() > 1024
    {
        return Err(INVALID.into());
    }
    Ok(metadata)
}
pub(super) fn container(
    conn: &Connection,
    root: &Path,
    reference: &EvidenceRef,
    budget: &mut crate::case_evidence_budget::StorageBudget,
) -> Result<Arc<VerifiedContainer>, String> {
    let metadata = manifest(conn, reference)?;
    let mut batch_metadata_bytes = 0usize;
    for id in &metadata.batches {
        crate::operations::check()?;
        let bytes: usize = conn
            .query_row(
                "SELECT octet_length(body) FROM native_evidence_batches WHERE batch_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if bytes > 64 << 10 {
            return Err(INVALID.into());
        }
        batch_metadata_bytes = batch_metadata_bytes.checked_add(bytes).ok_or(UNAVAILABLE)?;
    }
    budget.admit_manifest(
        metadata.bytes as usize,
        reference.member_count as usize,
        batch_metadata_bytes,
    )?;
    let mut batches = Vec::with_capacity(metadata.batches.len());
    for id in metadata.batches {
        batches.push(batch(conn, &id)?);
    }
    Ok(Arc::new(VerifiedContainer::open_sized(
        root,
        reference,
        batches,
        metadata.bytes,
    )?))
}
pub(super) enum OpenScope<'a> {
    All,
    Metadata,
    Analytical(Option<&'a str>),
}
pub(super) fn read_case(
    conn: &Connection,
    root: &Path,
    owner: &EvidenceOwner,
    scope: OpenScope<'_>,
) -> Result<(VerifiedCase, crate::case_work_budget::Lease), String> {
    require_owner(conn, owner)?;
    // Reserve before SQLite produces the admitted TEXT values and before the
    // exact decoder constructs any metadata or manifest tree.
    let (metadata_bytes, binding_bytes): (usize, usize) = conn.query_row(
        "SELECT octet_length(metadata),octet_length(bindings) FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",
        rusqlite::params![owner.case_id,owner.analysis_id], |row|Ok((row.get(0)?,row.get(1)?))
    ).map_err(|e|e.to_string())?;
    let mut budget =
        crate::case_evidence_budget::StorageBudget::begin(metadata_bytes, binding_bytes)?;
    let (metadata, bindings, signature): (String,String,String) = conn.query_row(
        "SELECT CASE WHEN octet_length(metadata)=?3 THEN metadata END,CASE WHEN octet_length(bindings)=?4 THEN bindings END,evidence_signature FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",
        rusqlite::params![owner.case_id,owner.analysis_id,metadata_bytes,binding_bytes],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))
    ).map_err(|e|e.to_string())?;
    let raw = RawJson::checked_with_limit(&metadata, VIEW_DOCUMENT_BYTES)?;
    budget.admit_metadata(decode::preflight_metadata(raw)?.materialization_credit)?;
    let metadata = decode::MetadataDecoder::new().value(raw)?;
    if metadata.get("kind").and_then(Value::as_str) == Some("preserved_case_unavailable") {
        return Err(UNAVAILABLE.into());
    }
    let bindings: Vec<Binding> = serde_json::from_str(&bindings).map_err(|e| e.to_string())?;
    if bindings.len() > 10_000 {
        return Err(UNAVAILABLE.into());
    }
    let mut verified = Vec::new();
    let mut references = Vec::with_capacity(bindings.len());
    for binding in bindings {
        crate::operations::check()?;
        if binding.reference.owner != *owner {
            return Err(INVALID.into());
        }
        let selected = match &scope {
            OpenScope::All => true,
            OpenScope::Metadata => false,
            OpenScope::Analytical(station) => match binding.location {
                ContainerLocation::ItemRows { index } => station.is_none_or(|id| {
                    metadata
                        .get("items")
                        .and_then(Value::as_array)
                        .and_then(|items| items.get(index as usize))
                        .and_then(|item| item.get("stationId"))
                        .and_then(Value::as_str)
                        == Some(id)
                }),
                _ => false,
            },
        };
        if selected {
            verified.push((
                binding.location.clone(),
                container(conn, root, &binding.reference, &mut budget)?,
            ));
        }
        references.push((binding.location, binding.reference));
    }
    let case = VerifiedCase::from_selected_parts(metadata, owner.clone(), references, verified)?;
    let CaseEvidenceState::Ready(state) = case.evidence_state()? else {
        return Err(UNAVAILABLE.into());
    };
    if state.evidence_signature != signature {
        return Err(INVALID.into());
    }
    Ok((case, budget.finish()))
}

pub(crate) struct CaseAuthorityLease {
    root: PathBuf,
    store: StoreIdentity,
    case: Arc<VerifiedCase>,
    case_evidence_signature: String,
    anchors: crate::case_evidence_anchors::AnchorMap,
    storage_credit: crate::case_work_budget::Lease,
}
impl CaseAuthorityLease {
    pub(crate) fn anchors(&self) -> &crate::case_evidence_anchors::AnchorMap {
        &self.anchors
    }
    pub(crate) fn case(&self) -> &VerifiedCase {
        &self.case
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.storage_credit.bytes()
    }
    pub(crate) fn store(&self) -> &StoreIdentity {
        &self.store
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        validate_case_authority(
            &self.root,
            &self.store,
            self.case.owner(),
            &self.case_evidence_signature,
        )?;
        self.case.validate()
    }
}
/// Snapshot admission/final validation remains the command's responsibility;
/// this proof is owner+evidence identity, independent of note/body revisions.
pub(crate) fn validate_case_authority(
    root: &Path,
    store: &StoreIdentity,
    owner: &EvidenceOwner,
    evidence_signature: &str,
) -> Result<(), String> {
    crate::operations::check()?;
    let root_guard = crate::case_recovery::RootLease::shared(root, &prepare::root_work())?;
    let conn = connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if db::stamp(&conn)?.identity() != *store || owner.store_id != store.store_id {
        return Err(INVALID.into());
    }
    let (analysis, signature) = current_owner(&conn, &owner.case_id)?;
    if analysis != owner.analysis_id || signature != evidence_signature {
        return Err(INVALID.into());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    root_guard.validate()?;
    Ok(())
}
pub(crate) fn open_case(
    root: &Path,
    request: &NativeCaseOpen,
) -> Result<CaseAuthorityLease, String> {
    crate::operations::check()?;
    if request.evidence_signature.len() != 64
        || request.station_id.as_ref().is_some_and(|s| s.len() > 4096)
    {
        return Err(INVALID.into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let conn = connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if db::stamp(&conn)?.identity() != request.store {
        return Err(INVALID.into());
    }
    // Verify the complete admitted identity, not just the analysis UUID.
    let actual = db::identity_for(&conn, &request.analysis_context.case_id)?;
    if actual != request.analysis_context {
        return Err(INVALID.into());
    }
    let owner = EvidenceOwner {
        store_id: request.store.store_id.clone(),
        case_id: actual.case_id,
        analysis_id: actual.analysis_id,
    };
    let (case, mut storage_credit) = read_case(
        &conn,
        &root,
        &owner,
        OpenScope::Analytical(request.station_id.as_deref()),
    )?;
    let (anchors, anchor_credit) = anchor_store::read(&conn, &owner)?;
    storage_credit.merge(anchor_credit)?;
    let case = Arc::new(case);
    let CaseEvidenceState::Ready(state) = case.evidence_state()? else {
        return Err(UNAVAILABLE.into());
    };
    if state.evidence_signature != request.evidence_signature {
        return Err(INVALID.into());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let lease = CaseAuthorityLease {
        root,
        store: request.store.clone(),
        case,
        storage_credit,
        case_evidence_signature: request.evidence_signature.clone(),
        anchors,
    };
    lease.validate()?;
    root_guard.validate()?;
    Ok(lease)
}

/// Preserved Timeline scope includes only ordered item rows, across stations.
/// It is verified by native evidence identity, without applying current filters
/// or silently deriving a new analytical configuration.
pub(crate) fn open_history(
    root: &Path,
    store: &StoreIdentity,
    owner: &EvidenceOwner,
    signature: &str,
) -> Result<CaseAuthorityLease, String> {
    if signature.len() != 64 {
        return Err(INVALID.into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let conn = connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if db::stamp(&conn)?.identity() != *store {
        return Err(INVALID.into());
    }
    let (case, mut storage_credit) = read_case(&conn, &root, owner, OpenScope::Analytical(None))?;
    let (anchors, anchor_credit) = anchor_store::read(&conn, owner)?;
    storage_credit.merge(anchor_credit)?;
    let CaseEvidenceState::Ready(state) = case.evidence_state()? else {
        return Err(UNAVAILABLE.into());
    };
    if state.evidence_signature != signature {
        return Err(INVALID.into());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let lease = CaseAuthorityLease {
        root,
        store: store.clone(),
        case: Arc::new(case),
        case_evidence_signature: signature.into(),
        anchors,
        storage_credit,
    };
    lease.validate()?;
    root_guard.validate()?;
    Ok(lease)
}

enum ReferenceBacking {
    Committed(Arc<VerifiedContainer>),
    Prepared(Arc<prepare::Pending>),
}
pub(crate) struct ReferenceAuthorityLease {
    root: PathBuf,
    store: StoreIdentity,
    reference: EvidenceReference,
    backing: ReferenceBacking,
    storage_credit: crate::case_work_budget::Lease,
}
impl ReferenceAuthorityLease {
    /// Ordered metadata only. The command validates the retained lease before
    /// and after its bounded page and reserves the response before cloning IDs.
    pub(crate) fn member_at(&self, index: usize) -> Result<MemberHandle, String> {
        let manifest = self.manifest();
        let member = manifest.members.get(index).ok_or(INVALID)?;
        Ok(MemberHandle {
            container_id: manifest.container_id.clone(),
            manifest_id: manifest.manifest_id.clone(),
            occurrence_id: member.occurrence_id.clone(),
        })
    }
    pub(crate) fn member_count(&self) -> usize {
        self.manifest().members.len()
    }
    pub(super) fn publish_prepared_files(&self) -> Result<(), String> {
        match &self.backing {
            ReferenceBacking::Committed(container) => container.validate(),
            ReferenceBacking::Prepared(pending) => pending.publish_files(&self.root),
        }
    }
    pub(super) fn manifest(&self) -> &ContainerManifest {
        match &self.backing {
            ReferenceBacking::Committed(container) => container.manifest(),
            ReferenceBacking::Prepared(pending) => pending.manifest(),
        }
    }
    /// Caller reserves retained clone credit before requesting these descriptors.
    pub(super) fn batches(&self) -> Vec<BatchRef> {
        match &self.backing {
            ReferenceBacking::Committed(container) => {
                container.batch_references().cloned().collect()
            }
            ReferenceBacking::Prepared(pending) => pending.batches(),
        }
    }
    pub(crate) fn store(&self) -> &StoreIdentity {
        &self.store
    }
    pub(crate) fn reference(&self) -> &EvidenceReference {
        &self.reference
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.storage_credit.bytes()
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        let root_guard =
            crate::case_recovery::RootLease::shared(&self.root, &prepare::root_work())?;
        let conn = connect_readonly(&self.root)?;
        conn.execute_batch("BEGIN DEFERRED")
            .map_err(|e| e.to_string())?;
        if db::stamp(&conn)?.identity() != self.store {
            return Err(INVALID.into());
        }
        require_owner(&conn, self.reference.owner())?;
        match (&self.reference, &self.backing) {
            (EvidenceReference::Committed(reference), ReferenceBacking::Committed(container)) => {
                manifest(&conn, reference)?;
                container.validate()?;
            }
            (EvidenceReference::Prepared(reference), ReferenceBacking::Prepared(pinned)) => {
                let current = prepare::pin(&self.root, reference)?;
                if !Arc::ptr_eq(&current, pinned) {
                    return Err(INVALID.into());
                }
            }
            _ => return Err(INVALID.into()),
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        root_guard.validate()
    }
    pub(crate) fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
        self.validate()?;
        let envelope = match &self.backing {
            ReferenceBacking::Committed(container) => container.envelope_for_member(member)?,
            ReferenceBacking::Prepared(pending) => {
                if member.container_id != pending.reference.container_id
                    || member.manifest_id != pending.reference.manifest_id
                {
                    return Err(INVALID.into());
                }
                pending.envelope(member)?
            }
        };
        self.validate()?;
        Ok(envelope)
    }
}
/// Historical envelopes are independent of current analytical overlays. A
/// committed manifest must remain registered under its original live owner;
/// prepared references additionally require the exact unexpired local token.
pub(crate) fn open_reference(
    root: &Path,
    store: &StoreIdentity,
    reference: &EvidenceReference,
) -> Result<ReferenceAuthorityLease, String> {
    crate::operations::check()?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let conn = connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if db::stamp(&conn)?.identity() != *store {
        return Err(INVALID.into());
    }
    require_owner(&conn, reference.owner())?;
    let mut budget = crate::case_evidence_budget::StorageBudget::begin(0, 0)?;
    let backing = match reference {
        EvidenceReference::Committed(reference) => {
            ReferenceBacking::Committed(container(&conn, &root, reference, &mut budget)?)
        }
        EvidenceReference::Prepared(reference) => {
            let pinned = prepare::pin(&root, reference)?;
            // The registry already owns these bytes, but admission explicitly
            // charges this operation's retained metadata too; no hidden bypass.
            budget.admit_manifest(
                pinned.manifest_bytes() as usize,
                pinned.manifest().members.len(),
                64 << 10,
            )?;
            ReferenceBacking::Prepared(pinned)
        }
    };
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let lease = ReferenceAuthorityLease {
        root,
        store: store.clone(),
        reference: reference.clone(),
        backing,
        storage_credit: budget.finish(),
    };
    lease.validate()?;
    root_guard.validate()?;
    Ok(lease)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(crate) fn fixture() -> (tempfile::TempDir, NativeCaseOpen, Vec<EvidenceRef>) {
        let root = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch("CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE case_analysis(case_id TEXT PRIMARY KEY,body TEXT NOT NULL);CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO metadata VALUES('revision','1');INSERT INTO cases VALUES('c','{}',0);").unwrap();
        db::schema(&conn).unwrap();
        let store = StoreIdentity {
            store_id: uuid::Uuid::new_v4().to_string(),
            epoch: uuid::Uuid::new_v4().to_string(),
        };
        let identity = crate::analysis_context::Identity {
            case_id: "c".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 2,
            visibility_revision: 3,
        };
        let mut snapshot = crate::analysis_context::prepare_new_case("c").unwrap();
        snapshot.analysis_id = identity.analysis_id.clone();
        snapshot.config_revision = identity.config_revision;
        snapshot.visibility_revision = identity.visibility_revision;
        conn.execute(
            "INSERT INTO case_analysis VALUES('c',?1)",
            [serde_json::to_string(&snapshot).unwrap()],
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        db::initialize_identity(&tx, &store).unwrap();
        tx.commit().unwrap();
        let owner = EvidenceOwner {
            store_id: store.store_id.clone(),
            case_id: "c".into(),
            analysis_id: identity.analysis_id.clone(),
        };
        conn.execute(
            "INSERT INTO native_evidence_anchors VALUES(?1,?2,?3)",
            rusqlite::params![
                owner.case_id,
                owner.analysis_id,
                serde_json::to_string(&crate::case_evidence_anchors::AnchorMap::default()).unwrap()
            ],
        )
        .unwrap();
        let metadata = serde_json::json!({"id":"c","name":"case","rows":null,"items":[{"id":"a","stationId":"A","rows":null},{"id":"b","stationId":"B","rows":null}]});
        let mut bindings = Vec::new();
        let mut opened = Vec::new();
        let mut references = Vec::new();
        for location in [
            ContainerLocation::CaseRows,
            ContainerLocation::ItemRows { index: 0 },
            ContainerLocation::ItemRows { index: 1 },
        ] {
            let staged = stage_records(root.path(), &owner, &Value::Null, |sink| {
                sink.push_event(&crate::model::Event::empty())
            })
            .unwrap();
            staged.publish_files(root.path()).unwrap();
            conn.execute(
                "INSERT INTO native_evidence_batches VALUES(?1,?2)",
                rusqlite::params![
                    staged.batch.batch_id,
                    serde_json::to_string(&staged.batch).unwrap()
                ],
            )
            .unwrap();
            let manifest = db::ManifestMetadata {
                reference: staged.reference.clone(),
                bytes: staged.manifest_bytes(),
                batches: vec![staged.batch.batch_id.clone()],
            };
            conn.execute(
                "INSERT INTO native_evidence_manifests VALUES(?1,?2)",
                rusqlite::params![
                    staged.reference.manifest_id,
                    serde_json::to_string(&manifest).unwrap()
                ],
            )
            .unwrap();
            bindings.push(Binding {
                location: location.clone(),
                reference: staged.reference.clone(),
            });
            references.push(staged.reference.clone());
            opened.push((
                location,
                Arc::new(
                    VerifiedContainer::open(root.path(), &staged.reference, [staged.batch.clone()])
                        .unwrap(),
                ),
            ));
        }
        let case = VerifiedCase::from_parts(metadata.clone(), owner, opened).unwrap();
        let CaseEvidenceState::Ready(state) = case.evidence_state().unwrap() else {
            panic!()
        };
        conn.execute(
            "INSERT INTO native_evidence_cases VALUES('c',?1,?2,?3,?4,'body-hash','recovery')",
            rusqlite::params![
                identity.analysis_id,
                metadata.to_string(),
                serde_json::to_string(&bindings).unwrap(),
                state.evidence_signature
            ],
        )
        .unwrap();
        (
            root,
            NativeCaseOpen {
                store,
                analysis_context: identity,
                station_id: Some("A".into()),
                evidence_signature: state.evidence_signature,
            },
            references,
        )
    }
    #[test]
    fn historical_view_stamp_rejects_metadata_revision_changes_without_rewriting_evidence() {
        let (root, request, _) = fixture();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let before = db::stamp(&conn).unwrap();
        validate_view_stamp(root.path(), &before).unwrap();
        conn.execute("UPDATE metadata SET value='2' WHERE key='revision'", [])
            .unwrap();
        assert!(validate_view_stamp(root.path(), &before).is_err());
        validate_case_authority(
            root.path(),
            &request.store,
            &EvidenceOwner {
                store_id: request.store.store_id.clone(),
                case_id: request.analysis_context.case_id.clone(),
                analysis_id: request.analysis_context.analysis_id.clone(),
            },
            &request.evidence_signature,
        )
        .unwrap();
        validate_view_stamp(root.path(), &db::stamp(&conn).unwrap()).unwrap();
    }
    #[test]
    fn station_open_does_not_read_unselected_or_nonanalytical_payloads() {
        let (root, request, refs) = fixture();
        for reference in [&refs[0], &refs[2]] {
            std::fs::remove_file(
                root.path()
                    .join("evidence-v1")
                    .join(format!("{}.manifest", reference.manifest_id)),
            )
            .unwrap();
        }
        let lease = open_case(root.path(), &request).unwrap();
        assert_eq!(lease.case().containers().count(), 1);
        let (_, count) = lease.case().station_signature(Some("A")).unwrap();
        assert_eq!(count, 1);
        let CaseEvidenceState::Ready(full) = lease.case().evidence_state().unwrap() else {
            panic!()
        };
        assert_eq!(full.preserved_count, 2);
        let mut output = Vec::new();
        assert!(lease.case().write_to(&mut output, false).is_err());
        assert!(output.is_empty());
        let all = NativeCaseOpen {
            station_id: None,
            ..request
        };
        assert!(open_case(root.path(), &all).is_err());
    }
    #[test]
    fn guard_ignores_note_revision_but_rejects_changed_membership() {
        let (root, request, _) = fixture();
        let lease = open_case(root.path(), &request).unwrap();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute("UPDATE metadata SET value='2' WHERE key='revision'", [])
            .unwrap();
        assert!(lease.validate().is_ok());
        conn.execute(
            "UPDATE native_evidence_cases SET evidence_signature=?1 WHERE case_id='c'",
            ["f".repeat(64)],
        )
        .unwrap();
        assert!(lease.validate().is_err());
    }
}
