//! Immutable Case publications and their accounted working ownership. Native
//! rows retain their original member handles; previews cannot enter this cache.
use crate::{
    case_evidence::{EvidenceOwner, MemberHandle, NativeCaseReceipt, StoreIdentity},
    case_work_budget::{self, Lease, Pool, MATERIALIZATION_LIMIT},
    model::Event,
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write, ops::Deref, sync::Arc};

pub const MISS: &str = "CASE_CACHE_MISS";
pub(crate) const CHANGED: &str =
    "CASE_CACHE_CHANGED: As evidências do Caso mudaram; recarregue a consulta.";
const NATIVE_PREFIX: &str = "native-evidence:";
const DEDUP_ENTRY_BYTES: usize = 96;

type Owner = Option<(String, String)>;
fn owner(identity: Option<&crate::analysis_context::Identity>) -> Owner {
    identity.map(|identity| (identity.case_id.clone(), identity.analysis_id.clone()))
}

#[derive(Clone, Debug)]
pub(crate) struct NativeAuthority {
    pub store: StoreIdentity,
    pub owner: EvidenceOwner,
    pub case_evidence_signature: String,
    pub evidence_signature: String,
    pub station_id: Option<String>,
    pub preserved_count: u32,
}
pub(crate) struct NativePublication {
    pub authority: NativeAuthority,
    /// Dense analytical position -> first original occurrence. No transformed
    /// Event becomes the authority for a subsequent Case-origin capture.
    members: Vec<MemberHandle>,
    guard: NativeGuard,
}
#[derive(Clone)]
pub(crate) struct NativeGuard {
    pub validate: Arc<dyn Fn(&NativeAuthority) -> Result<(), String> + Send + Sync>,
    pub lease: Arc<dyn std::any::Any + Send + Sync>,
    /// Logical retained size of the actual lease already owned by `lease`.
    /// Its pool reservation is made by storage before allocation, never here.
    pub storage_bytes: usize,
}
impl std::fmt::Debug for NativePublication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePublication")
            .field("authority", &self.authority)
            .field("members", &self.members.len())
            .finish_non_exhaustive()
    }
}
impl NativePublication {
    fn validate(&self) -> Result<(), String> {
        (self.guard.validate)(&self.authority)
    }
}
#[derive(Debug)]
pub(crate) struct Records {
    events: Vec<Event>,
    native: Option<NativePublication>,
    credit: Lease,
}
impl Deref for Records {
    type Target = Vec<Event>;
    fn deref(&self) -> &Self::Target {
        &self.events
    }
}
impl serde::Serialize for Records {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.events, serializer)
    }
}
impl Records {
    pub(crate) fn as_slice(&self) -> &[Event] {
        &self.events
    }
    pub(crate) fn native(&self) -> Option<&NativePublication> {
        self.native.as_ref()
    }
    pub(crate) fn pool(&self) -> &Arc<Pool> {
        self.credit.pool()
    }
    pub(crate) fn accounted_bytes(&self) -> usize {
        self.credit.bytes()
    }
    fn storage_credit(&self) -> Option<(&Arc<dyn std::any::Any + Send + Sync>, usize)> {
        self.native
            .as_ref()
            .map(|native| (&native.guard.lease, native.guard.storage_bytes))
    }
    fn retained_bytes(&self) -> usize {
        self.accounted_bytes()
            .saturating_add(self.storage_credit().map_or(0, |(_, bytes)| bytes))
    }
    fn legacy(events: Vec<Event>) -> Result<Self, String> {
        // Legacy IPC allocation already happened. This is a retention check;
        // NativeBuilder separately reserves before native Event decoding.
        let bytes = payload_bytes(&events)?;
        let pool = case_work_budget::global();
        if bytes > pool.limits().materialized {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        Ok(Self {
            events,
            native: None,
            credit: reserve_work(pool, bytes)?,
        })
    }
    pub(crate) fn clone_for_work(&self) -> Result<(Vec<Event>, Lease), String> {
        let credit = reserve_work(self.pool(), payload_bytes(&self.events)?)?;
        crate::operations::check()?;
        Ok((self.events.clone(), credit))
    }
    pub(crate) fn into_work(self) -> (Vec<Event>, Lease) {
        (self.events, self.credit)
    }
    pub(crate) fn original_member(
        &self,
        id: usize,
        event_ref: Option<&str>,
    ) -> Result<&MemberHandle, String> {
        let native = self
            .native
            .as_ref()
            .ok_or("CASE_NATIVE_AUTHORITY_REQUIRED")?;
        // Only NativeBuilder can construct this publication and seals dense
        // ids together with its member map. Never scan or fall back to a
        // duplicate numeric id from an arbitrary Event array.
        let position = id;
        let event = self
            .events
            .get(position)
            .ok_or("CASE_EVIDENCE_RECORD_ABSENT")?;
        if event.id != id || event_ref.is_some_and(|expected| expected != event.event_ref) {
            return Err("CASE_EVIDENCE_RECORD_CHANGED".into());
        }
        native
            .members
            .get(position)
            .ok_or_else(|| "CASE_EVIDENCE_ORIGIN_ABSENT".into())
    }
}

pub(crate) fn payload_bytes(events: &Vec<Event>) -> Result<usize, String> {
    let spare = events
        .capacity()
        .saturating_sub(events.len())
        .checked_mul(std::mem::size_of::<Event>())
        .ok_or(MATERIALIZATION_LIMIT)?;
    events.iter().try_fold(spare, |bytes, event| {
        bytes
            .checked_add(crate::query::event_payload_bytes(event))
            .ok_or_else(|| MATERIALIZATION_LIMIT.to_string())
    })
}
fn member_bytes(member: &MemberHandle) -> Result<usize, String> {
    [
        &member.container_id,
        &member.manifest_id,
        &member.occurrence_id,
    ]
    .iter()
    .try_fold(0usize, |bytes, value| {
        bytes
            .checked_add(value.capacity())
            .ok_or_else(|| MATERIALIZATION_LIMIT.to_string())
    })
}

/// Authored item inputs for the existing no-event_ref identity. The caller
/// traverses only stored ordered items[].rows; all other containers stay outside
/// initial analytics while remaining fully preserved by storage.
pub(crate) struct AnalyticalItemContext<'a> {
    pub station_id: Option<&'a str>,
    pub artifact_id: Option<&'a serde_json::Value>,
    pub origin: Option<&'a serde_json::Value>,
}
fn truthy(value: &&serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
fn record_key(event: &Event, item: &AnalyticalItemContext<'_>) -> Result<[u8; 32], String> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    if !event.event_ref.is_empty() {
        writer.0.update(event.event_ref.as_bytes());
    } else {
        let empty = serde_json::Value::String(String::new());
        let origin = event
            .fields
            .get("caminho")
            .filter(truthy)
            .or_else(|| item.artifact_id.filter(truthy))
            .or_else(|| item.origin.filter(truthy))
            .unwrap_or(&empty);
        serde_json::to_writer(
            &mut writer,
            &(
                origin,
                event.id,
                event.timestamp,
                &event.source,
                &event.code,
                &event.message,
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(writer.0.finalize().into())
}

pub(crate) struct RecordCredit {
    credit: Lease,
    materialization: usize,
}
#[derive(Clone)]
pub(crate) struct RecordAdmission {
    pool: Arc<Pool>,
    encoded_scratch: Arc<Lease>,
}
impl RecordAdmission {
    pub(crate) fn reserve(
        &self,
        member: &MemberHandle,
        encoded_bytes: usize,
        materialization_credit: usize,
    ) -> Result<RecordCredit, String> {
        if encoded_bytes > self.encoded_scratch.bytes()
            || materialization_credit > crate::case_evidence::EVENT_OWNED_BYTES
        {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        let bytes = materialization_credit
            .checked_add(member_bytes(member)?)
            .and_then(|n| n.checked_add(DEDUP_ENTRY_BYTES))
            .ok_or(MATERIALIZATION_LIMIT)?;
        Ok(RecordCredit {
            credit: reserve_work(&self.pool, bytes)?,
            materialization: materialization_credit,
        })
    }
}
pub(crate) struct NativeBuilder {
    events: Vec<Event>,
    members: Vec<MemberHandle>,
    seen: BTreeSet<[u8; 32]>,
    visited: u32,
    failed: bool,
    authority: NativeAuthority,
    guard: NativeGuard,
    credit: Lease,
    // Reserved before storage reads its first encoded envelope, which precedes
    // the RecordPlan callback. Remains held through all visitor callbacks.
    _encoded_scratch: Arc<Lease>,
}
impl NativeBuilder {
    pub(crate) fn new(authority: NativeAuthority, guard: NativeGuard) -> Result<Self, String> {
        Self::with_pool(
            authority,
            guard,
            case_work_budget::global().request_pool(),
            crate::case_evidence::ENVELOPE_BYTES,
        )
    }
    fn with_pool(
        authority: NativeAuthority,
        guard: NativeGuard,
        pool: Arc<Pool>,
        scratch: usize,
    ) -> Result<Self, String> {
        if authority.store.store_id != authority.owner.store_id {
            return Err("CASE_EVIDENCE_OWNER_CHANGED".into());
        }
        (guard.validate)(&authority)?;
        let header = std::mem::size_of::<Records>()
            + std::mem::size_of::<NativePublication>()
            + authority.store.store_id.capacity()
            + authority.store.epoch.capacity()
            + authority.owner.store_id.capacity()
            + authority.owner.case_id.capacity()
            + authority.owner.analysis_id.capacity()
            + authority.case_evidence_signature.capacity()
            + authority.evidence_signature.capacity()
            + authority.station_id.as_ref().map_or(0, String::capacity);
        if header > pool.limits().materialized {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        let credit = reserve_work(&pool, header)?;
        let encoded_scratch = Arc::new(reserve_work(&pool, scratch)?);
        Ok(Self {
            events: Vec::new(),
            members: Vec::new(),
            seen: BTreeSet::new(),
            visited: 0,
            failed: false,
            authority,
            guard,
            credit,
            _encoded_scratch: encoded_scratch,
        })
    }
    /// Separate from the mutable builder so storage can own independent reserve
    /// and visit closures without overlapping immutable/mutable borrows.
    pub(crate) fn record_admission(&self) -> RecordAdmission {
        RecordAdmission {
            pool: Arc::clone(self.credit.pool()),
            encoded_scratch: Arc::clone(&self._encoded_scratch),
        }
    }
    pub(crate) fn push(
        &mut self,
        item: &AnalyticalItemContext<'_>,
        member: MemberHandle,
        event: Event,
        record: RecordCredit,
    ) -> Result<(), String> {
        if self.failed {
            return Err("CASE_EVIDENCE_BUILD_FAILED".into());
        }
        let result = self.push_inner(item, member, event, record);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(
        &mut self,
        item: &AnalyticalItemContext<'_>,
        member: MemberHandle,
        mut event: Event,
        mut record: RecordCredit,
    ) -> Result<(), String> {
        crate::operations::check()?;
        if self
            .authority
            .station_id
            .as_deref()
            .is_some_and(|station| item.station_id != Some(station))
        {
            return Err("CASE_EVIDENCE_STATION_CHANGED".into());
        }
        self.visited = self
            .visited
            .checked_add(1)
            .filter(|count| *count <= self.authority.preserved_count)
            .ok_or("CASE_EVIDENCE_COUNT_CHANGED")?;
        let owned = crate::query::event_payload_bytes(&event);
        if owned > record.materialization {
            return Err("CASE_EVIDENCE_RECORD_PLAN_EXCEEDED".into());
        }
        let key = record_key(&event, item)?;
        if self.seen.contains(&key) {
            return Ok(());
        }
        let retained = owned
            .checked_sub(std::mem::size_of::<Event>())
            .and_then(|n| n.checked_add(member_bytes(&member).ok()?))
            .and_then(|n| n.checked_add(DEDUP_ENTRY_BYTES))
            .ok_or(MATERIALIZATION_LIMIT)?;
        if self.events.len() == self.events.capacity()
            || self.members.len() == self.members.capacity()
        {
            let capacity = self
                .events
                .len()
                .checked_add(32)
                .ok_or(MATERIALIZATION_LIMIT)?;
            let event_slots = capacity.saturating_sub(self.events.capacity());
            let member_slots = capacity.saturating_sub(self.members.capacity());
            let additional = event_slots
                .checked_mul(std::mem::size_of::<Event>())
                .and_then(|n| {
                    member_slots
                        .checked_mul(std::mem::size_of::<MemberHandle>())
                        .and_then(|m| n.checked_add(m))
                })
                .ok_or(MATERIALIZATION_LIMIT)?;
            let next = self
                .credit
                .bytes()
                .checked_add(additional)
                .and_then(|n| n.checked_add(retained))
                .ok_or(MATERIALIZATION_LIMIT)?;
            if next > self.credit.pool().limits().materialized {
                return Err(MATERIALIZATION_LIMIT.into());
            }
            self.credit
                .merge(reserve_work(self.credit.pool(), additional)?)?;
            self.events
                .try_reserve_exact(capacity - self.events.len())
                .map_err(|e| e.to_string())?;
            self.members
                .try_reserve_exact(capacity - self.members.len())
                .map_err(|e| e.to_string())?;
        }
        if self
            .credit
            .bytes()
            .checked_add(retained)
            .is_none_or(|bytes| bytes > self.credit.pool().limits().materialized)
        {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        record.credit.resize(retained)?;
        self.credit.merge(record.credit)?;
        self.seen.insert(key);
        event.id = self.events.len();
        self.events.push(event);
        self.members.push(member);
        Ok(())
    }
    pub(crate) fn finish(mut self) -> Result<Records, String> {
        crate::operations::check()?;
        if self.failed {
            return Err("CASE_EVIDENCE_BUILD_FAILED".into());
        }
        if self.visited != self.authority.preserved_count {
            return Err("CASE_EVIDENCE_COUNT_CHANGED".into());
        }
        let dedup_bytes = self
            .seen
            .len()
            .checked_mul(DEDUP_ENTRY_BYTES)
            .ok_or(MATERIALIZATION_LIMIT)?;
        drop(self.seen);
        self.credit.resize(
            self.credit
                .bytes()
                .checked_sub(dedup_bytes)
                .ok_or(MATERIALIZATION_LIMIT)?,
        )?;
        (self.guard.validate)(&self.authority)?;
        Ok(Records {
            events: self.events,
            native: Some(NativePublication {
                authority: self.authority,
                members: self.members,
                guard: self.guard,
            }),
            credit: self.credit,
        })
    }
}

struct Entry {
    owner: Owner,
    key: String,
    publication: String,
    events: Arc<Records>,
}
static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// Release only inactive retention on credit pressure. Existing consumers keep
/// their publication token and reservation; destruction runs after unlocking.
pub(crate) fn reserve_work(pool: &Arc<Pool>, bytes: usize) -> Result<Lease, String> {
    let pool = pool.request_pool();
    let mut trimmed_selections = 0u8;
    loop {
        match pool.reserve(bytes) {
            Ok(credit) => return Ok(credit),
            Err(error) if error == case_work_budget::WORK_BUSY => {
                // Do not evict another Case to fix A's own lower quota. A
                // global eviction is justified only by a confirmed root cap.
                let Some(pressure) = pool.pressure(bytes) else { return Err(error); };
                let retired = {
                    let mut cache = CACHE.lock();
                    cache
                        .iter()
                        .position(|entry| {
                            Arc::strong_count(&entry.events) == 1
                                && pool.releases_pressure(entry.events.pool(), pressure)
                        })
                        .map(|position| cache.remove(position))
                };
                if retired.is_none() {
                    let bit = match pressure { case_work_budget::Pressure::Owner => 1, case_work_budget::Pressure::Aggregate => 2 };
                    if trimmed_selections & bit == 0 {
                        trimmed_selections |= bit;
                        crate::engine::trim_inactive_selection_caches(None, Some((&pool, pressure)))?;
                        continue;
                    }
                    return Err(error);
                }
                drop(retired);
            }
            Err(error) => return Err(error),
        }
    }
}

/// The tiny retention set may contain several station publications backed by
/// one immutable authority lease. Count that shared allocation once, while
/// independently allocated authorities continue to count independently.
fn retained_bytes<'a>(records: impl Iterator<Item = &'a Records> + Clone) -> usize {
    records
        .clone()
        .enumerate()
        .fold(0usize, |total, (index, record)| {
            let storage = record.storage_credit().map_or(0, |(lease, bytes)| {
                let prior = records
                    .clone()
                    .take(index)
                    .filter_map(Records::storage_credit)
                    .filter(|(other, _)| Arc::ptr_eq(lease, other))
                    .map(|(_, bytes)| bytes)
                    .max()
                    .unwrap_or(0);
                bytes.saturating_sub(prior)
            });
            total
                .saturating_add(record.accounted_bytes())
                .saturating_add(storage)
        })
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReceipt {
    pub case_content_token: String,
}

/// Adopted and recovery-protected owners can only use issued native evidence.
/// This check is outside CACHE locks and never initializes or migrates storage.
pub(crate) fn require_legacy_owner(
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<(), String> {
    if let Some(identity) = identity {
        if crate::case_evidence::native_owner(&crate::config_dir(), identity)? {
            return Err("CASE_NATIVE_AUTHORITY_REQUIRED".into());
        }
    }
    Ok(())
}

pub fn store(key: String, events: Vec<Event>) -> Result<(), String> {
    store_for(key, events, None).map(|_| ())
}
fn store_for(
    key: String,
    events: Vec<Event>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<String, String> {
    if key.starts_with(NATIVE_PREFIX) {
        return Err("CASE_NATIVE_AUTHORITY_REQUIRED".into());
    }
    require_legacy_owner(identity)?;
    let publication = store_records(key, Records::legacy(events)?, owner(identity))?;
    require_legacy_owner(identity)?;
    Ok(publication)
}
fn store_records(key: String, events: Records, owner: Owner) -> Result<String, String> {
    let publication = uuid::Uuid::new_v4().to_string();
    let limit = events.pool().limits().retained;
    if events.retained_bytes() > limit {
        return Err(MATERIALIZATION_LIMIT.into());
    }
    let shared = Arc::new(events);
    let mut retired = Vec::with_capacity(3);
    let mut cache = CACHE.lock();
    if shared.native.is_none()
        && cache
            .iter()
            .any(|entry| entry.owner == owner && entry.key == key && entry.events.native.is_some())
    {
        return Err("CASE_NATIVE_AUTHORITY_REQUIRED".into());
    }
    #[cfg(test)]
    if let Some(hook) = BEFORE_STORE_PUBLICATION.with(|hook| hook.borrow_mut().take()) { hook(); }
    // Payload accounting/reservation may be expensive. Recheck cancellation
    // under the publication guard before replacing or evicting any entry.
    crate::operations::check()?;
    for position in (0..cache.len()).rev() {
        if cache[position].owner == owner && cache[position].key == key {
            retired.push(cache.remove(position));
        }
    }
    let aggregate_limit = case_work_budget::global().base_limits().retained;
    while !cache.is_empty() {
        let owner_pressure = retained_bytes(cache.iter().filter(|entry| entry.owner == owner)
            .map(|entry| entry.events.as_ref()).chain(std::iter::once(shared.as_ref()))) > limit;
        let aggregate_pressure = retained_bytes(cache.iter().map(|entry| entry.events.as_ref())
            .chain(std::iter::once(shared.as_ref()))) > aggregate_limit;
        if cache.len() < 3 && !owner_pressure && !aggregate_pressure { break; }
        // A's custom retention limit cannot evict B unless a genuinely shared
        // application limit (or the small global entry bound) is also reached.
        let position = if owner_pressure {
            cache.iter().position(|entry| entry.owner == owner).unwrap_or(0)
        } else { 0 };
        retired.push(cache.remove(position));
    }
    cache.push(Entry {
        owner,
        key,
        publication: publication.clone(),
        events: shared,
    });
    drop(cache);
    drop(retired);
    Ok(publication)
}
pub(crate) fn publish_native(key: String, records: Records) -> Result<NativeCaseReceipt, String> {
    if !key.starts_with(NATIVE_PREFIX) || key.len() > 512 {
        return Err("CASE_NATIVE_KEY_INVALID".into());
    }
    let native = records
        .native
        .as_ref()
        .ok_or("CASE_NATIVE_AUTHORITY_REQUIRED")?;
    let mut receipt = NativeCaseReceipt {
        case_key: key.clone(),
        case_content_token: String::new(),
        case_evidence_signature: native.authority.case_evidence_signature.clone(),
        evidence_signature: native.authority.evidence_signature.clone(),
        preserved_count: native.authority.preserved_count,
        analytical_count: u32::try_from(records.len()).map_err(|_| MATERIALIZATION_LIMIT)?,
    };
    let owner = Some((
        native.authority.owner.case_id.clone(),
        native.authority.owner.analysis_id.clone(),
    ));
    receipt.case_content_token = store_records(key, records, owner)?;
    Ok(receipt)
}

pub fn resolve(
    events: Option<Vec<Event>>,
    key: Option<String>,
) -> Result<Option<Arc<Records>>, String> {
    resolve_for(
        events,
        key,
        crate::analysis_runtime::current()
            .as_ref()
            .and_then(|admitted| admitted.identity.as_ref()),
    )
}
fn resolve_for(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<Option<Arc<Records>>, String> {
    resolve_for_with_token(events, key, identity).map(|(events, _)| events)
}
/// Capture only the publication token. Native storage validation and payload
/// ownership are acquired later, under global work admission.
pub(crate) fn pin_publication(key: &str, identity: Option<&crate::analysis_context::Identity>) -> Result<String, String> {
    let owner = owner(identity);
    CACHE.lock().iter().find(|entry| entry.owner == owner && entry.key == key)
        .map(|entry| entry.publication.clone()).ok_or_else(|| MISS.to_string())
}

pub(crate) fn resolve_for_with_token(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<(Option<Arc<Records>>, Option<String>), String> {
    if let Some(events) = events {
        require_legacy_owner(identity)?;
        return Ok((
            Some(Arc::new(Records::legacy(events)?)),
            Some(uuid::Uuid::new_v4().to_string()),
        ));
    }
    let Some(key) = key else {
        return Ok((None, None));
    };
    let owner = owner(identity);
    let (records, publication) = CACHE
        .lock()
        .iter()
        .find(|entry| entry.owner == owner && entry.key == key)
        .map(|entry| (Arc::clone(&entry.events), entry.publication.clone()))
        .ok_or_else(|| MISS.to_string())?;
    if let Some(native) = records.native() {
        native.validate()?;
    } else {
        require_legacy_owner(identity)?;
    }
    validate_token_current(&key, &publication, &owner)?;
    Ok((Some(records), Some(publication)))
}
pub(crate) fn record_position(
    events: &[Event],
    id: usize,
    event_ref: Option<&str>,
) -> Result<Option<usize>, String> {
    crate::operations::check()?;
    let mut first_id = None;
    let mut found = None;
    for (position, event) in events.iter().enumerate() {
        if position % 256 == 0 {
            crate::operations::check()?;
        }
        if event.id != id {
            continue;
        }
        first_id.get_or_insert(position);
        if event_ref.is_none_or(|expected| event.event_ref == expected) {
            if found.is_some() {
                return Err("A referência exata do registro é ambígua neste Caso.".into());
            }
            found = Some(position);
            if event_ref.is_none() {
                break;
            }
        }
    }
    crate::operations::check()?;
    Ok(found.or(first_id))
}
#[cfg(test)]
fn select_record(
    events: Option<&[Event]>,
    id: usize,
    event_ref: Option<&str>,
) -> Result<Option<Vec<Event>>, String> {
    let Some(events) = events else {
        crate::operations::check()?;
        return Ok(None);
    };
    let position = record_position(events, id, event_ref)?;
    let selected = position
        .map(|position| events[position].clone())
        .into_iter()
        .collect();
    crate::operations::check()?;
    Ok(Some(selected))
}
pub(crate) fn validate_token(
    key: &str,
    expected: &str,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<(), String> {
    let owner = owner(identity);
    let records = {
        let cache = CACHE.lock();
        let entry = cache
            .iter()
            .find(|entry| entry.owner == owner && entry.key == key)
            .ok_or(MISS)?;
        if entry.publication != expected {
            return Err(CHANGED.into());
        }
        Arc::clone(&entry.events)
    };
    if let Some(native) = records.native() {
        native.validate()?;
    } else {
        require_legacy_owner(identity)?;
    }
    // Storage verification may block. A replacement that happened during it
    // cannot inherit the older token, even when the caller reused its key.
    validate_token_current(key, expected, &owner)
}
fn validate_token_current(key: &str, expected: &str, owner: &Owner) -> Result<(), String> {
    let cache = CACHE.lock();
    let entry = cache
        .iter()
        .find(|entry| &entry.owner == owner && entry.key == key)
        .ok_or(MISS)?;
    if entry.publication != expected {
        return Err(CHANGED.into());
    }
    Ok(())
}
pub fn take(events: Option<Vec<Event>>, key: Option<String>) -> Result<Option<Records>, String> {
    take_for(
        events,
        key,
        crate::analysis_runtime::current()
            .as_ref()
            .and_then(|admitted| admitted.identity.as_ref()),
    )
}
pub(crate) fn take_for(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<Option<Records>, String> {
    take_for_with_token(events, key, identity).map(|(events, _)| events)
}
pub(crate) fn take_for_with_token(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<(Option<Records>, Option<String>), String> {
    let (events, publication) = resolve_for_with_token(events, key, identity)?;
    let events = events
        .map(|shared| match Arc::try_unwrap(shared) {
            Ok(records) => Ok(records),
            Err(shared) => shared.clone_for_work().map(|(events, credit)| Records {
                events,
                native: None,
                credit,
            }),
        })
        .transpose()?;
    Ok((events, publication))
}
#[cfg(test)]
thread_local! {
    static AFTER_SYNC_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    static BEFORE_STORE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

#[tauri::command]
pub async fn case_sync(
    key: String,
    events: Vec<Event>,
    analysis_context: Option<crate::analysis_context::Identity>,
) -> Result<SyncReceipt, String> {
    if key.is_empty() || key.len() > 512 {
        return Err("Chave de caso inválida.".into());
    }
    // Keep only bounded argument validation on the async command thread.
    // The moved identity is revalidated after global queue admission, never
    // resolved from whichever Case happens to be active when this starts.
    crate::offload(move || case_sync_impl(key, events, analysis_context)).await?
}

fn case_sync_impl(key: String, events: Vec<Event>, analysis_context: Option<crate::analysis_context::Identity>) -> Result<SyncReceipt, String> {
    crate::operations::check()?;
    let policy = if let Some(identity) = &analysis_context {
        let snapshot = crate::analysis_runtime::validate_identity(identity)?;
        let preferences = snapshot.interpretation.as_ref().map(|settings| settings.resources.clone()).unwrap_or_default();
        crate::case_resources::Policy::capture(Some(identity), &preferences)?
    } else {
        crate::case_resources::Policy::capture(None, &Default::default())?
    };
    crate::operations::check()?;
    let case_content_token = crate::case_resources::with(policy, || store_for(key, events, analysis_context.as_ref()))?;
    #[cfg(test)]
    if let Some(hook) = AFTER_SYNC_PUBLICATION.with(|hook| hook.borrow_mut().take()) { hook(); }
    // Once the cache changed, return its committed token even if cancellation
    // arrived at that boundary. A retry must not hide a successful publication.
    crate::operations::commit();
    Ok(SyncReceipt { case_content_token })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct OwnerDirectory {
        root: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl OwnerDirectory {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("LOGINSIGHT_DATA_DIR");
            std::env::set_var("LOGINSIGHT_DATA_DIR", root.path());
            let conn =
                rusqlite::Connection::open(root.path().join("investigations.sqlite3")).unwrap();
            conn.execute_batch("CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE case_analysis(case_id TEXT PRIMARY KEY,body TEXT NOT NULL);CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);").unwrap();
            Self { root, previous }
        }
        fn adopt(&self, identity: &crate::analysis_context::Identity, native: bool) {
            let conn = rusqlite::Connection::open(self.root.path().join("investigations.sqlite3"))
                .unwrap();
            conn.execute("INSERT INTO cases VALUES(?1,'{}',0)", [&identity.case_id])
                .unwrap();
            let body = serde_json::json!({"schemaVersion":1,"caseId":identity.case_id,"analysisId":identity.analysis_id,"configRevision":identity.config_revision,"visibilityRevision":identity.visibility_revision,"config":{"derivedFields":[],"references":[]},"migrationDiagnostics":[],"legacyRaw":null});
            conn.execute(
                "INSERT INTO case_analysis VALUES(?1,?2)",
                rusqlite::params![identity.case_id, body.to_string()],
            )
            .unwrap();
            if native {
                conn.execute_batch("CREATE TABLE native_evidence_cases(case_id TEXT PRIMARY KEY,analysis_id TEXT NOT NULL);").unwrap();
                conn.execute(
                    "INSERT INTO native_evidence_cases VALUES(?1,?2)",
                    rusqlite::params![identity.case_id, identity.analysis_id],
                )
                .unwrap();
            } else {
                conn.execute_batch(
                    "CREATE TABLE case_recovery_protected(case_id TEXT PRIMARY KEY);",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO case_recovery_protected VALUES(?1)",
                    [&identity.case_id],
                )
                .unwrap();
            }
        }
    }
    impl Drop for OwnerDirectory {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    struct HeldSyncSlot {
        release: Option<std::sync::mpsc::Sender<()>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for HeldSyncSlot {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() { let _ = release.send(()); }
            if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
        }
    }
    #[test]
    fn legacy_sync_waits_for_global_admission_and_queued_cancel_cannot_publish() {
        let _directory = OwnerDirectory::new();
        for cancelled in [false, true] {
            let scheduler = crate::global_scheduler::Scheduler::new(1);
            let blocker_scheduler = Arc::clone(&scheduler);
            let (ready, started) = std::sync::mpsc::channel();
            let (release, waiting) = std::sync::mpsc::channel();
            let blocker = std::thread::spawn(move || crate::global_scheduler::with_scheduler(blocker_scheduler, || {
                crate::operations::run_with_token(crate::operations::token(None).unwrap(), || {
                    ready.send(()).unwrap();
                    let _ = waiting.recv();
                }).unwrap();
            }));
            let slot = HeldSyncSlot { release: Some(release), worker: Some(blocker) };
            started.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let key = format!("queued-sync-{}", uuid::Uuid::new_v4());
            let sync_key = key.clone();
            let token = crate::operations::token(Some(id.clone())).unwrap().with_priority(crate::global_scheduler::Priority::Interactive);
            let worker_scheduler = Arc::clone(&scheduler);
            let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let seen = Arc::clone(&entered);
            let worker = std::thread::spawn(move || crate::global_scheduler::with_scheduler(worker_scheduler, || {
                crate::operations::run_with_token(token, || {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                    assert!(crate::global_scheduler::current().is_some());
                    case_sync_impl(sync_key, vec![Event::empty()], None)
                }).and_then(|result| result)
            }));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !crate::global_scheduler::with_scheduler(Arc::clone(&scheduler), crate::global_scheduler::interactive_waiting) {
                assert!(std::time::Instant::now() < deadline, "legacy sync did not reach admission");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
            assert!(matches!(pin_publication(&key, None), Err(error) if error == MISS));
            if cancelled {
                crate::operations::cancel_id(&id);
                assert!(matches!(worker.join().unwrap(), Err(error) if error == "Operação cancelada."));
                assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
                assert!(matches!(pin_publication(&key, None), Err(error) if error == MISS));
                drop(slot);
            } else {
                drop(slot);
                let receipt = worker.join().unwrap().unwrap();
                assert_eq!(pin_publication(&key, None).unwrap(), receipt.case_content_token);
            }
        }
    }

    #[test]
    fn legacy_sync_cancel_after_accounting_preserves_the_previous_publication() {
        let _directory = OwnerDirectory::new();
        let key = format!("preserved-sync-{}", uuid::Uuid::new_v4());
        let previous = case_sync_impl(key.clone(), vec![Event::empty()], None).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let mut replacement = Event::empty(); replacement.raw = "replacement payload ".repeat(100_000);
        let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&reached);
        let result = crate::global_scheduler::with_limit(1, || crate::operations::run_with_token(token, || {
            BEFORE_STORE_PUBLICATION.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                // The hook runs after Records::legacy accounted/reserved the
                // replacement and after CACHE was acquired, before mutation.
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
                assert!(crate::operations::cancel_id(&id));
            })));
            case_sync_impl(key.clone(), vec![replacement], None)
        }));
        assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
        assert!(matches!(result, Err(error) if error == "Operação cancelada."));
        assert_eq!(pin_publication(&key, None).unwrap(), previous.case_content_token);
        let (records, _) = resolve_for_with_token(None, Some(key), None).unwrap();
        assert!(records.unwrap()[0].raw.is_empty(), "cancelled replacement leaked into the cache");
    }

    #[test]
    fn successful_legacy_sync_returns_its_receipt_after_late_cancellation() {
        let _directory = OwnerDirectory::new();
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let key = format!("committed-sync-{}", uuid::Uuid::new_v4());
        let expected_key = key.clone();
        let receipt = crate::global_scheduler::with_limit(1, || crate::operations::run_with_token(token, || {
            AFTER_SYNC_PUBLICATION.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                assert!(pin_publication(&expected_key, None).is_ok(), "publication must precede the simulated cancellation");
                assert!(crate::operations::cancel_id(&id));
                assert!(crate::operations::cancelled());
            })));
            case_sync_impl(key.clone(), vec![Event::empty()], None)
        })).unwrap().unwrap();
        assert_eq!(pin_publication(&key, None).unwrap(), receipt.case_content_token);
    }

    #[test]
    fn adopted_and_restored_owners_reject_fresh_inline_and_pre_adoption_legacy_publications() {
        for native in [false, true] {
            let directory = OwnerDirectory::new();
            let identity = crate::analysis_context::Identity {
                case_id: format!("guard-{}", uuid::Uuid::new_v4()),
                analysis_id: uuid::Uuid::new_v4().to_string(),
                config_revision: 0,
                visibility_revision: 0,
            };
            let key = format!("legacy-{}", uuid::Uuid::new_v4());
            let token = store_for(key.clone(), vec![Event::empty()], Some(&identity)).unwrap();
            assert!(resolve_for_with_token(None, Some(key.clone()), Some(&identity)).is_ok());
            directory.adopt(&identity, native);
            let before = case_work_budget::global().used();
            for records in [vec![], vec![Event::empty()]] {
                assert!(store_for(
                    format!("fresh-{}", uuid::Uuid::new_v4()),
                    records.clone(),
                    Some(&identity)
                )
                .unwrap_err()
                .contains("CASE_NATIVE_AUTHORITY_REQUIRED"));
                assert!(resolve_for_with_token(Some(records), None, Some(&identity))
                    .unwrap_err()
                    .contains("CASE_NATIVE_AUTHORITY_REQUIRED"));
            }
            assert!(
                resolve_for_with_token(None, Some(key.clone()), Some(&identity))
                    .unwrap_err()
                    .contains("CASE_NATIVE_AUTHORITY_REQUIRED")
            );
            assert!(validate_token(&key, &token, Some(&identity))
                .unwrap_err()
                .contains("CASE_NATIVE_AUTHORITY_REQUIRED"));
            assert_eq!(case_work_budget::global().used(), before);
            assert!(resolve_for_with_token(None, None, Some(&identity))
                .unwrap()
                .0
                .is_none());
            let store = StoreIdentity {
                store_id: uuid::Uuid::new_v4().to_string(),
                epoch: uuid::Uuid::new_v4().to_string(),
            };
            let authority = NativeAuthority {
                owner: EvidenceOwner {
                    store_id: store.store_id.clone(),
                    case_id: identity.case_id.clone(),
                    analysis_id: identity.analysis_id.clone(),
                },
                store,
                case_evidence_signature: "a".repeat(64),
                evidence_signature: "b".repeat(64),
                station_id: None,
                preserved_count: 0,
            };
            let guard = NativeGuard {
                validate: Arc::new(|_| Ok(())),
                lease: Arc::new(()),
                storage_bytes: 0,
            };
            let records = NativeBuilder::new(authority, guard)
                .unwrap()
                .finish()
                .unwrap();
            let published =
                publish_native(format!("native-evidence:{}", uuid::Uuid::new_v4()), records)
                    .unwrap();
            let records = resolve_for_with_token(None, Some(published.case_key), Some(&identity))
                .unwrap()
                .0
                .unwrap();
            assert!(records.is_empty());
            assert!(records.native().is_some());
        }
    }

    #[test]
    fn shared_single_record_capture_retains_one_copy_and_distinguishes_absent_case_from_dataset() {
        let key = format!("single-record-{}", uuid::Uuid::new_v4());
        let events: Vec<_> = (0..32)
            .map(|id| {
                let mut event = Event::empty();
                event.id = id;
                event.event_ref = format!("single:{id}");
                event.raw = "x".repeat(16_384);
                event
            })
            .collect();
        store_for(key.clone(), events, None).unwrap();
        let (first, first_token) = resolve_for_with_token(None, Some(key.clone()), None).unwrap();
        let (second, second_token) = resolve_for_with_token(None, Some(key), None).unwrap();
        let first = first.unwrap();
        let second = second.unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "admission shares the immutable payload"
        );
        assert_eq!(first_token, second_token);
        let mut chosen = select_record(Some(first.as_slice()), 17, Some("single:17"))
            .unwrap()
            .unwrap();
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].id, 17);
        assert_eq!(
            chosen.iter().map(|event| event.raw.len()).sum::<usize>(),
            16_384
        );
        assert_eq!(
            first.iter().map(|event| event.raw.len()).sum::<usize>(),
            32 * 16_384
        );
        chosen[0].raw.push_str("changed copy");
        assert_eq!(
            first[17].raw.len(),
            16_384,
            "transforming the selected copy cannot mutate evidence"
        );
        assert!(select_record(Some(first.as_slice()), 100, None)
            .unwrap()
            .unwrap()
            .is_empty());
        assert!(select_record(None, 100, None).unwrap().is_none());
        let mut sparse = Event::empty();
        sparse.id = 500;
        assert_eq!(
            select_record(Some(&[sparse]), 500, None).unwrap().unwrap()[0].id,
            500
        );
        let token = crate::operations::token(Some("single-record-cancel".into())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("single-record-cancel");
            select_record(Some(first.as_slice()), 17, Some("single:17"))
        })
        .is_err());
    }
    #[test]
    fn single_record_selection_preserves_legacy_order_and_rejects_duplicate_exact_handles() {
        let mut first = Event::empty();
        first.id = 1;
        first.event_ref = "A".into();
        first.raw = "first".into();
        let mut second = first.clone();
        second.event_ref = "B".into();
        second.raw = "second".into();
        let records = vec![first.clone(), second];
        assert_eq!(
            select_record(Some(&records), 1, None).unwrap().unwrap()[0].raw,
            "first"
        );
        assert_eq!(
            select_record(Some(&records), 1, Some("B"))
                .unwrap()
                .unwrap()[0]
                .raw,
            "second"
        );
        let mut duplicate = first.clone();
        duplicate.raw = "different body".into();
        assert!(select_record(Some(&[first, duplicate]), 1, Some("A"))
            .unwrap_err()
            .contains("ambígua"));
    }
    #[test]
    fn identical_keys_never_cross_case_or_recreated_analysis_owners() {
        let _directory = OwnerDirectory::new();
        let identity = |case_id: &str, analysis_id: &str| crate::analysis_context::Identity {
            case_id: case_id.into(),
            analysis_id: analysis_id.into(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let a = identity("a", "analysis-a");
        let b = identity("b", "analysis-b");
        let recreated = identity("a", "replacement-a");
        let key = format!("shared-key-{}", uuid::Uuid::new_v4());
        store_for(key.clone(), vec![Event::empty()], Some(&a)).unwrap();
        assert!(resolve_for(None, Some(key.clone()), Some(&b)).is_err());
        assert!(resolve_for(None, Some(key.clone()), Some(&recreated)).is_err());
        assert!(resolve_for(None, Some(key.clone()), None).is_err());
        let mut revised = a.clone();
        revised.config_revision = 10;
        assert_eq!(
            resolve_for(None, Some(key.clone()), Some(&revised))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        store_for(key.clone(), vec![], Some(&b)).unwrap();
        assert_eq!(
            resolve_for(None, Some(key.clone()), Some(&a))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        assert!(resolve_for(None, Some(key), Some(&b))
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn same_key_replacement_changes_publication_without_rebinding_old_payloads() {
        let _directory = OwnerDirectory::new();
        let identity = crate::analysis_context::Identity {
            case_id: "token-case".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let key = format!("same-key-{}", uuid::Uuid::new_v4());
        let mut original = Event::empty();
        original.id = 7;
        original.event_ref = "same:7".into();
        original.message = "original".into();
        store_for(key.clone(), vec![original.clone()], Some(&identity)).unwrap();
        let (old, token) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        let (_, repeated) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        assert_eq!(token, repeated);
        let mut replacement = original.clone();
        replacement.message = "replacement".into();
        store_for(key.clone(), vec![replacement], Some(&identity)).unwrap();
        let (new, changed) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        assert_ne!(token, changed);
        assert_eq!(
            validate_token(&key, token.as_deref().unwrap(), Some(&identity)).unwrap_err(),
            CHANGED
        );
        assert!(validate_token(&key, changed.as_deref().unwrap(), Some(&identity)).is_ok());
        assert_eq!(old.unwrap()[0].message, "original");
        assert_eq!(new.unwrap()[0].message, "replacement");
        let (_, explicit_a) = take_for_with_token(
            Some(vec![original.clone()]),
            Some(key.clone()),
            Some(&identity),
        )
        .unwrap();
        let (_, explicit_b) =
            take_for_with_token(Some(vec![original]), Some(key.clone()), Some(&identity)).unwrap();
        assert_ne!(explicit_a, explicit_b);
        for suffix in ["a", "b", "c"] {
            store_for(format!("{key}-{suffix}"), Vec::new(), Some(&identity)).unwrap();
        }
        assert_eq!(
            take_for_with_token(None, Some(key), Some(&identity)).unwrap_err(),
            MISS
        );
    }

    #[test]
    fn keys_resolve_until_evicted() {
        store("a".into(), vec![Event::empty()]).unwrap();
        assert_eq!(resolve(None, Some("a".into())).unwrap().unwrap().len(), 1);
        assert!(resolve(None, None).unwrap().is_none());
        for k in ["b", "c", "d"] {
            store(k.into(), vec![]).unwrap();
        }
        assert_eq!(resolve(None, Some("a".into())).unwrap_err(), MISS);
        assert_eq!(
            resolve(Some(vec![]), Some("zzz".into()))
                .unwrap()
                .unwrap()
                .len(),
            0
        );
    }
}

#[cfg(test)]
mod native_publication_tests {
    use super::*;
    use crate::case_work_budget::Limits;

    fn authority(count: u32) -> NativeAuthority {
        let store = uuid::Uuid::new_v4().to_string();
        NativeAuthority {
            store: StoreIdentity {
                store_id: store.clone(),
                epoch: uuid::Uuid::new_v4().to_string(),
            },
            owner: EvidenceOwner {
                store_id: store,
                case_id: uuid::Uuid::new_v4().to_string(),
                analysis_id: uuid::Uuid::new_v4().to_string(),
            },
            case_evidence_signature: "complete-case".into(),
            evidence_signature: "station-subset".into(),
            station_id: None,
            preserved_count: count,
        }
    }
    fn guard() -> NativeGuard {
        NativeGuard {
            validate: Arc::new(|_| Ok(())),
            lease: Arc::new(()),
            storage_bytes: 0,
        }
    }
    fn pool() -> Arc<Pool> {
        Pool::new(Limits {
            materialized: 64 << 10,
            retained: 64 << 10,
            live: 512 << 10,
        })
    }
    fn member(name: &str) -> MemberHandle {
        MemberHandle {
            container_id: "container".into(),
            manifest_id: "manifest".into(),
            occurrence_id: name.into(),
        }
    }
    fn push(
        builder: &mut NativeBuilder,
        name: &str,
        event: Event,
        item: &AnalyticalItemContext<'_>,
    ) -> Result<(), String> {
        let member = member(name);
        let encoded = serde_json::to_vec(&event).unwrap().len();
        let plan = crate::query::event_payload_bytes(&event) + 256;
        let credit = builder.record_admission().reserve(&member, encoded, plan)?;
        builder.push(item, member, event, credit)
    }
    fn context() -> AnalyticalItemContext<'static> {
        AnalyticalItemContext {
            station_id: None,
            artifact_id: None,
            origin: None,
        }
    }
    fn retire(key: &str) {
        let removed = {
            let mut cache = CACHE.lock();
            cache
                .iter()
                .position(|entry| entry.key == key)
                .map(|position| cache.remove(position))
        };
        drop(removed);
    }

    #[test]
    fn owner_pressure_preserves_other_case_cache_until_aggregate_is_full() {
        use crate::case_work_budget::OwnerCounter;
        let limits = Limits { materialized: 64, retained: 64, live: 64 };
        let root = Pool::new(limits);
        let owner_a = Arc::new(OwnerCounter::default());
        let a = Pool::child(limits, Arc::clone(&root), Arc::clone(&owner_a), 24);
        let b = Pool::child(limits, Arc::clone(&root), Arc::new(OwnerCounter::default()), 32);
        let active_a = a.reserve(24).unwrap();
        let key = format!("owner-pressure-{}", uuid::Uuid::new_v4());
        store_records(key.clone(), Records { events: Vec::new(), native: None, credit: b.reserve(32).unwrap() }, Some((key.clone(), "b".into()))).unwrap();
        assert!(reserve_work(&a, 1).is_err());
        assert!(CACHE.lock().iter().any(|entry| entry.key == key), "A's local pressure must leave B cached");
        assert_eq!(root.used(), 56);
        let expanded_a = Pool::child(limits, Arc::clone(&root), owner_a, 48);
        let added = reserve_work(&expanded_a, 16).unwrap();
        assert!(!CACHE.lock().iter().any(|entry| entry.key == key), "confirmed aggregate pressure can evict inactive B");
        assert_eq!(root.used(), 40);
        drop((active_a, added));
        assert_eq!(root.used(), 0);
    }

    #[test]
    fn native_first_occurrences_keep_exact_values_dense_ids_and_original_members() {
        let pool = pool();
        let mut builder =
            NativeBuilder::with_pool(authority(4), guard(), Arc::clone(&pool), 64 << 10).unwrap();
        let mut first = Event::empty();
        first.id = 9001;
        first.event_ref = "same-exact-reference".into();
        first
            .fields
            .insert("unsafe".into(), serde_json::json!(9007199254740993u64));
        first
            .fields
            .insert("forms".into(), serde_json::json!([1, 1.0, "1", -0.0]));
        push(&mut builder, "first", first.clone(), &context()).unwrap();
        let mut duplicate = first.clone();
        duplicate.raw = "a duplicate occurrence is still preserved in storage".into();
        push(&mut builder, "duplicate", duplicate, &context()).unwrap();
        let mut legacy = Event::empty();
        legacy.id = 77;
        legacy.message = "same legacy event".into();
        let artifact_a = serde_json::json!("a");
        let artifact_b = serde_json::json!("b");
        push(
            &mut builder,
            "artifact-a",
            legacy.clone(),
            &AnalyticalItemContext {
                artifact_id: Some(&artifact_a),
                ..context()
            },
        )
        .unwrap();
        push(
            &mut builder,
            "artifact-b",
            legacy,
            &AnalyticalItemContext {
                artifact_id: Some(&artifact_b),
                ..context()
            },
        )
        .unwrap();
        let records = builder.finish().unwrap();
        assert_eq!(
            records.iter().map(|event| event.id).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            records[0].fields, first.fields,
            "no JS number serialization or transformed projection is involved"
        );
        assert_eq!(
            records
                .original_member(0, Some("same-exact-reference"))
                .unwrap()
                .occurrence_id,
            "first"
        );
        assert_eq!(
            records.original_member(1, Some("")).unwrap().occurrence_id,
            "artifact-a"
        );
        assert_eq!(
            records.original_member(2, Some("")).unwrap().occurrence_id,
            "artifact-b"
        );
        assert_eq!(records.native().unwrap().authority.preserved_count, 4);
        assert_eq!(
            pool.used(),
            records.accounted_bytes(),
            "encoded scratch and the dedup table are released after construction"
        );
        drop(records);
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn caminho_precedes_artifact_and_station_selection_is_never_silently_ignored() {
        let mut builder =
            NativeBuilder::with_pool(authority(2), guard(), pool(), 64 << 10).unwrap();
        let mut event = Event::empty();
        event.id = 50;
        event
            .fields
            .insert("caminho".into(), serde_json::json!("same path"));
        let a = serde_json::json!("artifact-a");
        let b = serde_json::json!("artifact-b");
        push(
            &mut builder,
            "a",
            event.clone(),
            &AnalyticalItemContext {
                artifact_id: Some(&a),
                ..context()
            },
        )
        .unwrap();
        push(
            &mut builder,
            "b",
            event,
            &AnalyticalItemContext {
                artifact_id: Some(&b),
                ..context()
            },
        )
        .unwrap();
        assert_eq!(builder.finish().unwrap().len(), 1);
        let mut scoped = authority(1);
        scoped.station_id = Some("station-a".into());
        let mut builder = NativeBuilder::with_pool(scoped, guard(), pool(), 64 << 10).unwrap();
        assert_eq!(
            push(
                &mut builder,
                "wrong",
                Event::empty(),
                &AnalyticalItemContext {
                    station_id: Some("station-b"),
                    ..context()
                }
            )
            .unwrap_err(),
            "CASE_EVIDENCE_STATION_CHANGED"
        );
        assert!(
            builder.finish().is_err(),
            "a refused row cannot be published as an empty successful Case"
        );
    }

    #[test]
    fn native_materialization_rejection_releases_every_temporary_and_preserves_no_partial_publication(
    ) {
        let pool = pool();
        let mut builder =
            NativeBuilder::with_pool(authority(3), guard(), Arc::clone(&pool), 64 << 10).unwrap();
        for index in 0..2 {
            let mut event = Event::empty();
            event.id = index;
            event.event_ref = format!("row-{index}");
            event.raw = "x".repeat(20 << 10);
            push(&mut builder, &format!("member-{index}"), event, &context()).unwrap();
        }
        let mut event = Event::empty();
        event.event_ref = "row-2".into();
        event.raw = "x".repeat(20 << 10);
        assert_eq!(
            push(&mut builder, "member-2", event, &context()).unwrap_err(),
            MATERIALIZATION_LIMIT
        );
        assert!(pool.used() <= pool.limits().live);
        assert!(
            builder.finish().is_err(),
            "ignoring a failed row must not seal a partial publication"
        );
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn shared_authority_metadata_is_retained_once_and_lives_until_last_publication() {
        let pool = pool();
        let held = Arc::new(pool.reserve(40 << 10).unwrap());
        let metadata_bytes = held.bytes();
        let native_guard = NativeGuard {
            validate: Arc::new(|_| Ok(())),
            lease: held,
            storage_bytes: metadata_bytes,
        };
        let first =
            NativeBuilder::with_pool(authority(0), native_guard.clone(), Arc::clone(&pool), 1024)
                .unwrap()
                .finish()
                .unwrap();
        let second = NativeBuilder::with_pool(authority(0), native_guard, Arc::clone(&pool), 1024)
            .unwrap()
            .finish()
            .unwrap();
        let expected = first.accounted_bytes() + second.accounted_bytes() + metadata_bytes;
        assert_eq!(retained_bytes([&first, &second].into_iter()), expected);
        assert_eq!(
            pool.used(),
            expected,
            "reporting the metadata must never reserve it twice"
        );
        drop(first);
        assert_eq!(pool.used(), second.accounted_bytes() + metadata_bytes);
        drop(second);
        assert_eq!(pool.used(), 0);
    }
    #[test]
    fn separate_authorities_are_charged_separately_and_oversized_retention_is_refused() {
        let pool = pool();
        let build = |bytes| {
            let held = Arc::new(pool.reserve(bytes).unwrap());
            NativeBuilder::with_pool(
                authority(0),
                NativeGuard {
                    validate: Arc::new(|_| Ok(())),
                    lease: held,
                    storage_bytes: bytes,
                },
                Arc::clone(&pool),
                1024,
            )
            .unwrap()
            .finish()
            .unwrap()
        };
        let first = build(20 << 10);
        let second = build(20 << 10);
        assert_eq!(
            retained_bytes([&first, &second].into_iter()),
            first.retained_bytes() + second.retained_bytes()
        );
        drop(first);
        drop(second);
        let oversized = build(pool.limits().retained);
        assert_eq!(
            publish_native(
                format!("{NATIVE_PREFIX}{}", uuid::Uuid::new_v4()),
                oversized
            )
            .unwrap_err(),
            MATERIALIZATION_LIMIT
        );
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn authoritative_empty_case_keeps_native_scope_and_its_storage_lease() {
        let pool = pool();
        let held = Arc::new(());
        let weak = Arc::downgrade(&held);
        let records = NativeBuilder::with_pool(
            authority(0),
            NativeGuard {
                validate: Arc::new(|_| Ok(())),
                lease: held,
                storage_bytes: 0,
            },
            Arc::clone(&pool),
            1024,
        )
        .unwrap()
        .finish()
        .unwrap();
        let shared = Arc::new(records);
        let active = Arc::clone(&shared);
        drop(shared);
        assert!(active.is_empty());
        assert!(active.native().is_some());
        assert!(weak.upgrade().is_some());
        assert!(active.original_member(0, None).is_err());
        assert!(pool.used() > 0);
        drop(active);
        assert!(weak.upgrade().is_none());
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn cache_eviction_keeps_live_payload_and_native_lease_charged() {
        let pool = pool();
        let base = authority(1);
        let identity = crate::analysis_context::Identity {
            case_id: base.owner.case_id.clone(),
            analysis_id: base.owner.analysis_id.clone(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let keys: Vec<_> = (0..3)
            .map(|_| format!("{NATIVE_PREFIX}{}", uuid::Uuid::new_v4()))
            .collect();
        let make = || {
            let mut builder =
                NativeBuilder::with_pool(base.clone(), guard(), Arc::clone(&pool), 64 << 10)
                    .unwrap();
            let mut event = Event::empty();
            event.raw = "x".repeat(28 << 10);
            push(&mut builder, "member", event, &context()).unwrap();
            builder.finish().unwrap()
        };
        publish_native(keys[0].clone(), make()).unwrap();
        let first = resolve_for(None, Some(keys[0].clone()), Some(&identity))
            .unwrap()
            .unwrap();
        let original_bytes = first.accounted_bytes();
        publish_native(keys[1].clone(), make()).unwrap();
        assert_eq!(
            resolve_for(None, Some(keys[0].clone()), Some(&identity)).unwrap_err(),
            MISS,
            "byte retention can evict before the third entry"
        );
        assert!(pool.used() >= original_bytes * 2);
        assert_eq!(first[0].raw.len(), 28 << 10);
        drop(first);
        assert_eq!(pool.used(), original_bytes);
        for key in keys {
            retire(&key);
        }
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn native_validation_runs_outside_cache_lock_and_rechecks_replacement_afterward() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let armed = Arc::new(AtomicBool::new(false));
        let (entered, wait_entered) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let waiting = Arc::new(Mutex::new(wait_release));
        let callback = {
            let armed = Arc::clone(&armed);
            let waiting = Arc::clone(&waiting);
            Arc::new(move |_: &NativeAuthority| {
                if armed.swap(false, Ordering::AcqRel) {
                    entered.send(()).unwrap();
                    waiting.lock().recv().unwrap();
                }
                Ok(())
            }) as Arc<dyn Fn(&NativeAuthority) -> Result<(), String> + Send + Sync>
        };
        let authority = authority(0);
        let identity = crate::analysis_context::Identity {
            case_id: authority.owner.case_id.clone(),
            analysis_id: authority.owner.analysis_id.clone(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let key = format!("{NATIVE_PREFIX}{}", uuid::Uuid::new_v4());
        let original = NativeBuilder::with_pool(
            authority.clone(),
            NativeGuard {
                validate: callback,
                lease: Arc::new(()),
                storage_bytes: 0,
            },
            pool(),
            1024,
        )
        .unwrap()
        .finish()
        .unwrap();
        let receipt = publish_native(key.clone(), original).unwrap();
        armed.store(true, Ordering::Release);
        let handle = {
            let key = key.clone();
            let identity = identity.clone();
            std::thread::spawn(move || {
                validate_token(&key, &receipt.case_content_token, Some(&identity))
            })
        };
        wait_entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let replacement = NativeBuilder::with_pool(authority, guard(), pool(), 1024)
            .unwrap()
            .finish()
            .unwrap();
        publish_native(key.clone(), replacement).unwrap();
        release.send(()).unwrap();
        assert_eq!(handle.join().unwrap().unwrap_err(), CHANGED);
        assert!(store_for(key.clone(), Vec::new(), Some(&identity))
            .unwrap_err()
            .contains("NATIVE_AUTHORITY"));
        retire(&key);
    }

    #[test]
    fn work_pressure_evicts_disposable_retention_but_never_releases_live_consumers() {
        let pool = Pool::new(Limits {
            materialized: 64 << 10,
            retained: 64 << 10,
            live: 96 << 10,
        });
        let authority = authority(1);
        let identity = crate::analysis_context::Identity {
            case_id: authority.owner.case_id.clone(),
            analysis_id: authority.owner.analysis_id.clone(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let mut builder =
            NativeBuilder::with_pool(authority, guard(), Arc::clone(&pool), 32 << 10).unwrap();
        let mut event = Event::empty();
        event.raw = "x".repeat(28 << 10);
        push(&mut builder, "member", event, &context()).unwrap();
        let key = format!("{NATIVE_PREFIX}{}", uuid::Uuid::new_v4());
        publish_native(key.clone(), builder.finish().unwrap()).unwrap();
        let active = resolve_for(None, Some(key.clone()), Some(&identity))
            .unwrap()
            .unwrap();
        assert!(reserve_work(&pool, 70 << 10)
            .unwrap_err()
            .contains("CASE_WORK_BUSY"));
        assert_eq!(
            resolve_for(None, Some(key.clone()), Some(&identity))
                .unwrap()
                .unwrap()[0]
                .raw
                .len(),
            28 << 10
        );
        drop(active);
        let work = reserve_work(&pool, 70 << 10).unwrap();
        assert_eq!(pool.used(), 70 << 10);
        assert_eq!(
            resolve_for(None, Some(key), Some(&identity)).unwrap_err(),
            MISS
        );
        drop(work);
        assert_eq!(pool.used(), 0);
    }
}
