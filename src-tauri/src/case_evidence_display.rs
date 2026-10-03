//! Complete historical display planning. Only compact native locators survive a
//! record callback; pages and group expansion contain no analytical Events.
use crate::{
    case_evidence::{
        self as evidence, EvidenceOwner, EvidenceReference, MemberHandle, RawJson, StoreStamp,
    },
    case_evidence_display_policy as policy,
    case_evidence_history::{
        self as history, HistoricalItem, HistoryAuthority, Response, TextPreview,
    },
    case_work_budget::{self, Lease},
    model::Event,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

const INVALID: &str = "CASE_DISPLAY_IDENTITY_CHANGED";
const LIMIT: &str = "CASE_DISPLAY_LIMIT";
const MAX_ROWS: usize = 100_000;
const MAX_ANCHORS: usize = 10_000;
const MAX_EDGES: usize = 200_000;
const MAX_NOTE_EDGES: usize = 100_000;
const MAX_ASSOCIATION_WORK: u64 = 64 << 20;
const ASSOCIATION_LIMIT: &str = "CASE_DISPLAY_ASSOCIATION_LIMIT";
const PAGE: usize = 128;
const WIRE: usize = 1 << 20;
// Includes worst-case JSON escaping of a 4 KiB authored entry ID, owner
// strings, fixed fields and continuation tokens before adding complete rows.
const PAGE_HEADER_BYTES: usize = 32 << 10;
fn reserve(bytes: usize) -> Result<Lease, String> {
    crate::case_cache::reserve_work(case_work_budget::global(), bytes)
}
fn check() -> Result<(), String> {
    crate::operations::check()
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DisplayRequest {
    pub store: StoreStamp,
    pub owner: EvidenceOwner,
    pub evidence_signature: String,
    pub authored_view_json: String,
    pub station_id: Option<String>,
    #[serde(default)]
    pub filters: Vec<crate::query::Filter>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    #[serde(default = "yes")]
    pub include_untimed: bool,
    #[serde(default)]
    pub display_query: String,
    pub cursor: Option<String>,
}
impl DisplayRequest {
    fn history(&self) -> history::TimelineRequest {
        history::TimelineRequest {
            store: self.store.identity(),
            owner: self.owner.clone(),
            evidence_signature: self.evidence_signature.clone(),
            authored_view_json: self.authored_view_json.clone(),
            station_id: self.station_id.clone(),
            filters: self.filters.clone(),
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            include_untimed: self.include_untimed,
            cursor: None,
            aliases: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MembersRequest {
    pub scope: DisplayRequest,
    pub display_signature: String,
    pub member_token: String,
    pub cursor: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisplayEntry {
    pub entry_id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub start_ms: Option<f64>,
    pub end_ms: Option<f64>,
    pub timing: &'static str,
    pub title: TextPreview,
    pub detail: TextPreview,
    pub source: TextPreview,
    pub member_count: u32,
    pub first_member_entry_id: Option<String>,
    pub first_member_edit_key: Option<String>,
    pub member_token: Option<String>,
    pub item_index: Option<u32>,
    pub group_index: Option<u32>,
    pub manual_index: Option<u32>,
    pub note_indices: Vec<u32>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisplayPage {
    pub kind: &'static str,
    pub owner: EvidenceOwner,
    pub evidence_signature: String,
    pub authored_view_signature: String,
    pub display_signature: String,
    pub preserved_count: u32,
    pub scope_occurrence_count: u32,
    pub unavailable_count: u32,
    pub display_entry_count: u32,
    pub matching_display_entry_count: u32,
    pub entries: Vec<DisplayEntry>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisplayMember {
    pub reference: EvidenceReference,
    pub member: MemberHandle,
    pub item_index: u32,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MembersPage {
    pub kind: &'static str,
    pub display_signature: String,
    pub entry_id: String,
    pub member_count: u32,
    pub members: Vec<DisplayMember>,
    pub next_cursor: Option<String>,
}
/// Random access uses the CURRENT manifest ordinal, never immutable originPosition.
/// Read callbacks and metadata access retain the same admitted authority.
pub(crate) trait DisplayAuthority: HistoryAuthority {
    fn validate_stamp(&self, stamp: &StoreStamp) -> Result<(), String>;
    fn item(&self, item: u32) -> Result<&Value, String>;
    fn member(&self, item: u32, ordinal: u32) -> Result<(EvidenceReference, MemberHandle), String>;
    fn read(
        &self,
        item: u32,
        ordinal: u32,
        visitor: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String>;
}
#[derive(Clone, Copy)]
struct Row {
    container: [u8; 16],
    item: u32,
    ordinal: u32,
    time: Option<i64>,
    timing: u8,
    interpretable: bool,
}
#[derive(Clone, Copy)]
enum Kind {
    Event,
    Auto,
    Group(u32),
    Manual(u32),
}
struct Entry {
    kind: Kind,
    begin: u32,
    len: u32,
    start: Option<f64>,
    end: Option<f64>,
    timing: u8,
    order: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: String,
    offset: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    binding: String,
    entry: u32,
}
struct Plan {
    document: Value,
    authored: history::AuthoredView,
    rows: Vec<Row>,
    slots: Vec<u32>,
    entries: Vec<Entry>,
    notes: BTreeMap<u32, Vec<u32>>,
    edit_keys: BTreeMap<u32, String>,
    association_costs: RefCell<BTreeMap<u32, u64>>,
    association_work: Cell<u64>,
    matches: Vec<u32>,
    signature: String,
    preserved: u32,
    unavailable: u32,
    // Credits outlive every allocation above, including captured authored trees.
    _document_credit: Lease,
    _plan_credit: Lease,
}
fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(v)) => Ok(v),
        _ => Err(policy::UNSUPPORTED.into()),
    }
}
fn text_or<'a>(value: Option<&'a Value>, fallback: &'a str) -> Result<&'a str, String> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(fallback),
        Some(Value::String(s)) => Ok(if s.is_empty() { fallback } else { s }),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Ok(fallback),
        _ => Err(policy::UNSUPPORTED.into()),
    }
}
fn required_id(value: Option<&Value>) -> Result<&str, String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| policy::UNSUPPORTED.into())
}
fn manual_time(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && n.abs() <= 8_640_000_000_000_000.0)
}
fn event_id(authority: &impl DisplayAuthority, row: Row) -> Result<String, String> {
    Ok(history::entry_id(
        &authority.member(row.item, row.ordinal)?.1,
    ))
}
fn entry_id(
    authority: &impl DisplayAuthority,
    plan: &Plan,
    entry: &Entry,
) -> Result<String, String> {
    match entry.kind {
        Kind::Group(i) => {
            Ok(required_id(plan.document["timeline"]["groups"][i as usize].get("id"))?.into())
        }
        Kind::Manual(i) => Ok(format!(
            "m:{}",
            required_id(plan.document["manual"][i as usize].get("id"))?
        )),
        Kind::Event | Kind::Auto => {
            let id = event_id(
                authority,
                plan.rows[plan.slots[entry.begin as usize] as usize],
            )?;
            Ok(if matches!(entry.kind, Kind::Auto) {
                format!("a:{id}")
            } else {
                id
            })
        }
    }
}
fn row_title<'a>(event: &'a Event, authored: &'a history::AuthoredView, id: &str) -> &'a str {
    authored
        .titles
        .get(id)
        .map(String::as_str)
        .unwrap_or_else(|| {
            [&event.name, &event.message, &event.code]
                .into_iter()
                .find(|s| !s.is_empty())
                .map(String::as_str)
                .unwrap_or("Evento")
        })
}
fn with_event<T>(
    authority: &impl DisplayAuthority,
    row: Row,
    action: impl FnOnce(&Event, &str) -> Result<T, String>,
) -> Result<T, String> {
    let mut action = Some(action);
    let mut result = None;
    authority.read(row.item, row.ordinal, &mut |raw| {
        if result.is_some() {
            return Err(INVALID.into());
        }
        let decode = evidence::preflight_envelope(raw)?;
        let _credit = reserve(decode.materialization_credit)?;
        let event = evidence::materialize_event(raw, decode)?;
        result = Some(action.take().ok_or(INVALID)?(&event, raw)?);
        Ok(())
    })?;
    result.ok_or_else(|| INVALID.into())
}
fn key(
    authority: &impl DisplayAuthority,
    row: Row,
    authored: &history::AuthoredView,
) -> Result<String, String> {
    let id = event_id(authority, row)?;
    with_event(authority, row, |event, _| {
        let key = if !event.code.is_empty() {
            &event.code
        } else if !event.name.is_empty() {
            &event.name
        } else {
            return policy::redact_event_text(row_title(event, authored, &id));
        };
        if key.len() > policy::MAX_TEXT_BYTES {
            return Err(policy::TEXT_LIMIT.into());
        }
        Ok(key.clone())
    })
}
struct Texts {
    title: String,
    detail: String,
    source: String,
}
fn texts(authority: &impl DisplayAuthority, plan: &Plan, entry: &Entry) -> Result<Texts, String> {
    if let Kind::Manual(i) = entry.kind {
        let m = &plan.document["manual"][i as usize];
        return Ok(Texts {
            title: text_or(m.get("name"), "Marco")?.into(),
            detail: text_or(m.get("description"), "")?.into(),
            source: "Marco do caso".into(),
        });
    }
    let row = plan.rows[plan.slots[entry.begin as usize] as usize];
    let item = authority.item(row.item)?;
    let source = policy::redact_event_text(text_or(
        item.get("label"),
        text_or(item.get("name"), "Item do caso")?,
    )?)?;
    if let Kind::Group(i) = entry.kind {
        return Ok(Texts {
            title: text_or(
                plan.document["timeline"]["groups"][i as usize].get("name"),
                "Sequência",
            )?
            .into(),
            detail: format!("{} eventos", entry.len),
            source,
        });
    }
    if !row.interpretable {
        return Ok(Texts {
            title: "Registro preservado indisponível".into(),
            detail: String::new(),
            source,
        });
    }
    let id = event_id(authority, row)?;
    with_event(authority, row, |event, raw| {
        let title = policy::redact_event_text(row_title(event, &plan.authored, &id))?;
        let detail = if matches!(entry.kind, Kind::Auto) {
            format!("{} eventos na sequência", entry.len)
        } else {
            let mut event_ref = None;
            let mut fields = RawJson::checked(raw)?.members()?;
            while let Some(field) = fields.next()? {
                if field.key()? == "event_ref" {
                    event_ref = Some(event.event_ref.as_str());
                }
            }
            let detections = list(item, "detections")?;
            admit_associations(plan, row.item, detections)?;
            let context = policy::event_context(detections, event_ref)?;
            policy::event_detail(&event.message, &event.description, &context)?
        };
        Ok(Texts {
            title,
            detail,
            source,
        })
    })
}
// event_context preserves declaration order by making two complete passes.
// Count a conservative byte/comparison bound once per item, then debit it for
// every displayed singleton, including repeated page materialization.
fn admit_associations(plan: &Plan, item: u32, detections: &[Value]) -> Result<(), String> {
    let cached = plan.association_costs.borrow().get(&item).copied();
    let cost = if let Some(cost) = cached {
        cost
    } else {
        let mut units = 0u64;
        for detection in detections {
            check()?;
            units = units.checked_add(64).ok_or(ASSOCIATION_LIMIT)?;
            // A short matching reference can amplify a large saved name into
            // each singleton's context and redaction input. Charge that output
            // possibility even when this particular Event does not match.
            units = units
                .checked_add(
                    detection
                        .get("name")
                        .and_then(Value::as_str)
                        .map_or(0, str::len) as u64,
                )
                .ok_or(ASSOCIATION_LIMIT)?;
            match detection.get("event_refs") {
                Some(Value::Array(refs)) => {
                    for (index, value) in refs.iter().enumerate() {
                        if index % 256 == 0 {
                            check()?;
                        }
                        units = units
                            .checked_add(1 + value.as_str().map_or(0, str::len) as u64)
                            .ok_or(ASSOCIATION_LIMIT)?;
                        if units > MAX_ASSOCIATION_WORK {
                            return Err(ASSOCIATION_LIMIT.into());
                        }
                    }
                }
                Some(Value::String(refs)) => {
                    units = units
                        .checked_add(refs.len() as u64)
                        .ok_or(ASSOCIATION_LIMIT)?
                }
                _ => (),
            }
            if units > MAX_ASSOCIATION_WORK {
                return Err(ASSOCIATION_LIMIT.into());
            }
        }
        let cost = units.checked_mul(2).ok_or(ASSOCIATION_LIMIT)?;
        plan.association_costs.borrow_mut().insert(item, cost);
        cost
    };
    let next = plan
        .association_work
        .get()
        .checked_add(cost)
        .filter(|n| *n <= MAX_ASSOCIATION_WORK)
        .ok_or(ASSOCIATION_LIMIT)?;
    plan.association_work.set(next);
    Ok(())
}
fn wanted_insert(wanted: &mut BTreeSet<String>, id: &str) -> Result<(), String> {
    if id.len() > 4096 || wanted.len() >= MAX_ANCHORS && !wanted.contains(id) {
        return Err(LIMIT.into());
    }
    wanted.insert(id.into());
    Ok(())
}
#[derive(Default)]
struct Hit {
    count: u32,
    row: Option<u32>,
}
fn plan(authority: &impl DisplayAuthority, request: &DisplayRequest) -> Result<Plan, String> {
    let request_bytes = wire_len(request, evidence::REQUEST_BYTES)?;
    if request.authored_view_json.len() > WIRE
        || request.owner.store_id != request.store.store_id
        || request.evidence_signature.len() != 64
        || request.owner.case_id.len() > 1024
        || request.station_id.as_ref().is_some_and(|s| s.len() > 4096)
        || request
            .from_ms
            .is_some_and(|n| history::safe_timestamp(Some(n)).is_none())
        || request
            .to_ms
            .is_some_and(|n| history::safe_timestamp(Some(n)).is_none())
        || request
            .from_ms
            .zip(request.to_ms)
            .is_some_and(|(a, b)| a > b)
    {
        return Err(INVALID.into());
    }
    authority.validate_stamp(&request.store)?;
    let _request_credit = reserve(request_bytes.checked_mul(8).ok_or(LIMIT)?)?;
    let history_request = request.history();
    let preserved = authority.validate(&history_request)?;
    let authored = history::authored_view(authority, &history_request, preserved)?;
    let raw = RawJson::checked(&request.authored_view_json)?;
    let decode = evidence::preflight_value(raw)?;
    let document_credit = reserve(decode.materialization_credit)?;
    let document = evidence::materialize_value(raw, decode)?;
    let config = &document["timeline"];
    let groups = list(config, "groups")?;
    let annotations = list(config, "annotations")?;
    let manuals = list(&document, "manual")?;
    let group_slots = groups.iter().try_fold(0usize, |n, g| {
        n.checked_add(list(g, "ids")?.len())
            .ok_or_else(|| LIMIT.to_string())
    })?;
    if groups.len() > MAX_ANCHORS
        || annotations.len() > MAX_ANCHORS
        || manuals.len() > MAX_ANCHORS
        || group_slots > MAX_EDGES
    {
        return Err(LIMIT.into());
    }
    // Ceiling reservations precede vector/map/string allocations; they cover
    // transient wanted indexes and note closures as well as retained plan data.
    let row_cap = (preserved as usize).min(MAX_ROWS);
    let entry_cap = row_cap
        .checked_add(groups.len())
        .and_then(|n| n.checked_add(manuals.len()))
        .ok_or(LIMIT)?;
    let bytes = row_cap
        .checked_mul(64)
        .and_then(|n| n.checked_add(entry_cap.checked_mul(96)?))
        .and_then(|n| n.checked_add((row_cap + group_slots).checked_mul(8)?))
        .and_then(|n| n.checked_add(request.authored_view_json.len().checked_mul(8)?))
        .and_then(|n| {
            n.checked_add(
                (if annotations.is_empty() {
                    0
                } else {
                    MAX_NOTE_EDGES * 128
                }) + MAX_ANCHORS * 256,
            )
        })
        .ok_or(LIMIT)?;
    let plan_credit = reserve(bytes)?;
    let _scratch = reserve(evidence::ENVELOPE_BYTES + 6 * policy::MAX_TEXT_BYTES)?;
    let query = policy::normalize_query(&request.display_query)?;
    let filters = history::prepare_filters(&request.filters)?;
    let mut wanted = BTreeSet::new();
    for g in groups {
        wanted_insert(&mut wanted, required_id(g.get("id"))?)?;
        for id in list(g, "ids")? {
            wanted_insert(&mut wanted, required_id(Some(id))?)?;
        }
    }
    if let Some(edits) = config.get("edits") {
        for id in edits.as_object().ok_or(policy::UNSUPPORTED)?.keys() {
            wanted_insert(&mut wanted, id)?;
        }
    }
    for n in annotations {
        required_id(n.get("id"))?;
        if let Some(id) = n.get("anchor").filter(|v| !v.is_null()) {
            wanted_insert(&mut wanted, required_id(Some(id))?)?;
        }
        for link in list(n, "links")? {
            wanted_insert(&mut wanted, required_id(link.get("targetId"))?)?;
        }
    }
    let mut hits: BTreeMap<String, Hit> = wanted
        .iter()
        .filter(|id| {
            id.starts_with("n:")
                || id.starts_with("a:n:")
                || id.starts_with("e:")
                || id.starts_with("a:e:")
        })
        .filter(|id| authority.blocked_alias(id).is_none())
        .map(|id| (id.clone(), Hit::default()))
        .collect();
    let mut legacy: BTreeMap<[u8; 32], Vec<String>> = BTreeMap::new();
    for id in hits.keys() {
        let canonical = id.strip_prefix("a:").unwrap_or(id);
        if canonical.starts_with("e:") {
            legacy
                .entry(Sha256::digest(canonical.as_bytes()).into())
                .or_default()
                .push(id.clone());
        }
    }
    let mut rows = Vec::with_capacity(row_cap);
    let mut ordinal_by_item = BTreeMap::<u32, u32>::new();
    let mut visited = 0u32;
    let mut unavailable = 0u32;
    authority.visit(&mut |item, member, envelope| {
        check()?;
        visited = visited.checked_add(1).ok_or(LIMIT)?;
        if visited > preserved {
            return Err(INVALID.into());
        }
        let ordinal = ordinal_by_item.entry(item.item_index).or_default();
        let current = *ordinal;
        *ordinal = ordinal.checked_add(1).ok_or(LIMIT)?;
        let id = history::entry_id(&member);
        let automatic = format!("a:{id}");
        let alias = if legacy.is_empty() {
            None
        } else {
            let suffix = history::legacy_suffix(envelope, item.original_row_index)?;
            Some(history::alias_digest(
                item.item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined"),
                &suffix,
            ))
        };
        let mut eligible = None;
        if !request
            .station_id
            .as_deref()
            .is_some_and(|s| item.item.get("stationId").and_then(Value::as_str) != Some(s))
        {
            let decoded = match evidence::preflight_envelope(envelope) {
                Ok(p) => {
                    let c = reserve(p.materialization_credit)?;
                    match evidence::materialize_event(envelope, p) {
                        Ok(e) => Some((e, c)),
                        Err(e) if history::unavailable_interpretation(&e) => None,
                        Err(e) => return Err(e),
                    }
                }
                Err(e) if history::unavailable_interpretation(&e) => None,
                Err(e) => return Err(e),
            };
            if decoded.is_none() && !filters.filters.is_empty() {
                return Err("CASE_HISTORY_FILTER_UNAVAILABLE".into());
            }
            if decoded
                .as_ref()
                .is_none_or(|(event, _)| filters.filters.iter().all(|f| f.matches(event)))
            {
                let event = decoded.as_ref().map(|(e, _)| e);
                let time = event.and_then(|e| history::safe_timestamp(e.timestamp));
                let unavailable_time = event.is_none()
                    || event.is_some_and(|e| e.timestamp.is_some() && time.is_none());
                // Graphs exclude untimed rows but still need the diagnostic
                // count for preserved records they cannot plot.
                if unavailable_time {
                    unavailable += 1;
                }
                if time.map_or(request.include_untimed, |t| {
                    !request.from_ms.is_some_and(|f| t < f)
                        && !request.to_ms.is_some_and(|to| t > to)
                }) {
                    if rows.len() == MAX_ROWS {
                        return Err(LIMIT.into());
                    }
                    let timing = if unavailable_time {
                        2
                    } else if time.is_none() {
                        1
                    } else {
                        0
                    };
                    eligible = Some(rows.len() as u32);
                    rows.push(Row {
                        container: *uuid::Uuid::parse_str(&member.container_id)
                            .map_err(|_| INVALID)?
                            .as_bytes(),
                        item: item.item_index,
                        ordinal: current,
                        time,
                        timing,
                        interpretable: event.is_some(),
                    });
                }
            }
        }
        for key in [Some(id.as_str()), Some(automatic.as_str())]
            .into_iter()
            .flatten()
            .chain(
                alias
                    .as_ref()
                    .and_then(|a| legacy.get(a))
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            )
        {
            if let Some(hit) = hits.get_mut(key) {
                hit.count += 1;
                hit.row = if hit.count == 1 { eligible } else { None };
            }
        }
        Ok(())
    })?;
    if visited != preserved {
        return Err(INVALID.into());
    }
    let resolve = |id: &str| {
        if id.starts_with("a:") {
            None
        } else {
            hits.get(id).filter(|h| h.count == 1).and_then(|h| h.row)
        }
    };
    let mut order: Vec<u32> = (0..rows.len() as u32).collect();
    order.sort_unstable_by_key(|i| {
        let r = rows[*i as usize];
        (r.timing, r.time.unwrap_or(0), r.item, r.ordinal)
    });
    let mut consumed = vec![false; rows.len()];
    let mut slots = Vec::with_capacity(row_cap + group_slots);
    let mut entries = Vec::with_capacity(entry_cap);
    for (index, g) in groups.iter().enumerate() {
        check()?;
        let begin = slots.len();
        for id in list(g, "ids")? {
            if let Some(row) = resolve(required_id(Some(id))?) {
                if rows[row as usize].timing == 0 {
                    slots.push(row);
                }
            }
        }
        if slots.len() - begin < 2 {
            slots.truncate(begin);
            continue;
        }
        if slots.len() > MAX_EDGES {
            return Err(LIMIT.into());
        }
        let mut start = f64::INFINITY;
        let mut end = f64::NEG_INFINITY;
        for i in &slots[begin..] {
            consumed[*i as usize] = true;
            let t = rows[*i as usize].time.ok_or(INVALID)? as f64;
            start = start.min(t);
            end = end.max(t);
        }
        entries.push(Entry {
            kind: Kind::Group(index as u32),
            begin: begin as u32,
            len: (slots.len() - begin) as u32,
            start: Some(start),
            end: Some(end),
            timing: 0,
            order: entries.len() as u32,
        });
    }
    order.retain(|i| !consumed[*i as usize]);
    let compact = config.get("compact") != Some(&Value::Bool(false));
    let mut i = 0;
    while i < order.len() {
        check()?;
        let first = rows[order[i] as usize];
        let mut j = i + 1;
        if compact && first.timing == 0 {
            let signature = key(authority, first, &authored)?;
            while j < order.len() && j - i < 250 {
                let row = rows[order[j] as usize];
                let previous = rows[order[j - 1] as usize];
                if row.timing != 0
                    || row.container != first.container
                    || row.time.unwrap() - previous.time.unwrap() > 300_000
                    || key(authority, row, &authored)? != signature
                {
                    break;
                }
                j += 1;
            }
        }
        if slots.len().checked_add(j - i).ok_or(LIMIT)? > MAX_EDGES {
            return Err(LIMIT.into());
        }
        let begin = slots.len();
        slots.extend_from_slice(&order[i..j]);
        entries.push(Entry {
            kind: if j - i > 1 { Kind::Auto } else { Kind::Event },
            begin: begin as u32,
            len: (j - i) as u32,
            start: first.time.map(|t| t as f64),
            end: rows[order[j - 1] as usize].time.map(|t| t as f64),
            timing: first.timing,
            order: entries.len() as u32,
        });
        i = j;
    }
    for (index, m) in manuals.iter().enumerate() {
        check()?;
        let Some(start) = manual_time(m.get("start")) else {
            continue;
        };
        required_id(m.get("id"))?;
        let end = manual_time(m.get("end"))
            .filter(|end| *end > start)
            .unwrap_or(start);
        entries.push(Entry {
            kind: Kind::Manual(index as u32),
            begin: 0,
            len: 0,
            start: Some(start),
            end: Some(end),
            timing: 0,
            order: entries.len() as u32,
        });
    }
    entries.sort_unstable_by(|a, b| {
        a.timing
            .cmp(&b.timing)
            .then_with(|| {
                a.start
                    .unwrap_or(0.0)
                    .partial_cmp(&b.start.unwrap_or(0.0))
                    .unwrap()
            })
            .then_with(|| {
                a.end
                    .unwrap_or(0.0)
                    .partial_cmp(&b.end.unwrap_or(0.0))
                    .unwrap()
            })
            .then(a.order.cmp(&b.order))
    });
    let signature = history::binding(&(
        policy::POLICY_VERSION,
        &request.store,
        &request.owner,
        &request.evidence_signature,
        &authored.signature,
        &request.station_id,
        &request.filters,
        request.from_ms,
        request.to_ms,
        request.include_untimed,
        &request.display_query,
    ))?;
    let mut edit_keys = BTreeMap::<u32, String>::new();
    if let Some(edits) = config.get("edits").and_then(Value::as_object) {
        for id in edits.keys() {
            if let Some(row) = hits.get(id).filter(|h| h.count == 1).and_then(|h| h.row) {
                let priority = |key: &str| {
                    if key.starts_with("n:") {
                        0
                    } else if key.starts_with("e:") {
                        1
                    } else {
                        2
                    }
                };
                if edit_keys
                    .get(&row)
                    .is_none_or(|old| priority(id) < priority(old))
                {
                    edit_keys.insert(row, id.clone());
                }
            }
        }
    }
    let mut result = Plan {
        document,
        authored,
        rows,
        slots,
        entries,
        notes: BTreeMap::new(),
        edit_keys,
        association_costs: RefCell::new(BTreeMap::new()),
        association_work: Cell::new(0),
        matches: vec![],
        signature,
        preserved,
        unavailable,
        _document_credit: document_credit,
        _plan_credit: plan_credit,
    };
    result.notes = resolve_notes(authority, &result, &wanted, &hits)?;
    result.matches = Vec::with_capacity(result.entries.len());
    for (index, entry) in result.entries.iter().enumerate() {
        check()?;
        let matched = if query.is_empty() {
            true
        } else {
            let display = texts(authority, &result, entry)?;
            let mut matched = false;
            for text in [&display.title, &display.detail, &display.source] {
                if policy::matches_normalized_query(text, &query)? {
                    matched = true;
                    break;
                }
            }
            if !matched {
                for note in result.notes.get(&(index as u32)).into_iter().flatten() {
                    let text = text_or(
                        result.document["timeline"]["annotations"][*note as usize].get("text"),
                        "",
                    )?;
                    if policy::matches_normalized_query(text, &query)? {
                        matched = true;
                        break;
                    }
                }
            }
            matched
        };
        if matched {
            result.matches.push(index as u32);
        }
    }
    if authority.validate(&history_request)? != preserved {
        return Err(INVALID.into());
    }
    authority.validate_stamp(&request.store)?;
    check()?;
    Ok(result)
}

fn resolve_notes(
    authority: &impl DisplayAuthority,
    plan: &Plan,
    wanted: &BTreeSet<String>,
    hits: &BTreeMap<String, Hit>,
) -> Result<BTreeMap<u32, Vec<u32>>, String> {
    let config = &plan.document["timeline"];
    let groups = list(config, "groups")?;
    let annotations = list(config, "annotations")?;
    let mut by_row = BTreeMap::<u32, Vec<&str>>::new();
    for (id, hit) in hits {
        if hit.count == 1 {
            if let Some(row) = hit.row {
                by_row.entry(row).or_default().push(id);
            }
        }
    }
    let mut anchors = BTreeMap::<String, u32>::new();
    for (index, entry) in plan.entries.iter().enumerate() {
        check()?;
        let id = entry_id(authority, plan, entry)?;
        if wanted.contains(&id) {
            anchors.insert(id, index as u32);
        }
        for slot in &plan.slots[entry.begin as usize..(entry.begin + entry.len) as usize] {
            for alias in by_row.get(slot).into_iter().flatten() {
                anchors.insert((*alias).into(), index as u32);
            }
        }
    }
    // Fallback is applied after complete display ordering, exactly as noteMap.
    for group in groups {
        let id = required_id(group.get("id"))?;
        if anchors.contains_key(id) {
            continue;
        }
        for member in list(group, "ids")? {
            if let Some(target) = anchors.get(required_id(Some(member))?).copied() {
                anchors.insert(id.into(), target);
                break;
            }
        }
    }
    let mut first_notes = BTreeMap::<&str, usize>::new();
    for (index, note) in annotations.iter().enumerate() {
        first_notes
            .entry(required_id(note.get("id"))?)
            .or_insert(index);
    }
    let mut memo = BTreeMap::<String, Option<u32>>::new();
    let mut attached = BTreeMap::<String, Vec<u32>>::new();
    let mut target_keys = BTreeMap::<String, u32>::new();
    let mut attached_ids = BTreeSet::<(u32, &str)>::new();
    let mut edges = 0usize;
    for (index, note) in annotations.iter().enumerate() {
        check()?;
        let mut targets = BTreeSet::new();
        let primary = if note.get("arrowHidden") == Some(&Value::Bool(true)) {
            None
        } else {
            note.get("anchor").and_then(Value::as_str)
        };
        for target in primary.into_iter().chain(
            list(note, "links")?
                .iter()
                .map(|link| link.get("targetId").and_then(Value::as_str).unwrap_or("")),
        ) {
            if target.is_empty() {
                continue;
            }
            let target =
                resolve_note_target(target, &anchors, &first_notes, annotations, &mut memo)?;
            if let Some(target) = target {
                targets.insert(entry_id(authority, plan, &plan.entries[target as usize])?);
            }
        }
        for target in targets {
            let note_id = required_id(note.get("id"))?;
            let next_key = target_keys.len() as u32;
            let target_key = *target_keys.entry(target.clone()).or_insert(next_key);
            if attached_ids.insert((target_key, note_id)) {
                edges += 1;
                if edges > MAX_NOTE_EDGES {
                    return Err(LIMIT.into());
                }
                attached.entry(target).or_default().push(index as u32);
            }
        }
    }
    let mut result = BTreeMap::new();
    let mut total = 0usize;
    for (index, entry) in plan.entries.iter().enumerate() {
        check()?;
        if let Some(notes) = attached.get(&entry_id(authority, plan, entry)?) {
            total = total.checked_add(notes.len()).ok_or(LIMIT)?;
            if total > MAX_NOTE_EDGES {
                return Err(LIMIT.into());
            }
            result.insert(index as u32, notes.clone());
        }
    }
    Ok(result)
}
fn resolve_note_target(
    target: &str,
    anchors: &BTreeMap<String, u32>,
    first: &BTreeMap<&str, usize>,
    annotations: &[Value],
    memo: &mut BTreeMap<String, Option<u32>>,
) -> Result<Option<u32>, String> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = target;
    let answer = loop {
        check()?;
        if current.is_empty() {
            break None;
        }
        if let Some(index) = anchors.get(current) {
            break Some(*index);
        }
        if let Some(resolved) = memo.get(current) {
            break *resolved;
        }
        if !seen.insert(current) {
            break None;
        }
        if path.len() >= MAX_ANCHORS {
            return Err(LIMIT.into());
        }
        path.push(current);
        let Some(index) = first.get(current) else {
            break None;
        };
        current = annotations[*index]
            .get("anchor")
            .and_then(Value::as_str)
            .unwrap_or("");
    };
    for id in path {
        memo.insert(id.into(), answer);
    }
    Ok(answer)
}
struct Counter {
    bytes: usize,
    limit: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other(LIMIT))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn wire_len(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    let mut writer = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| LIMIT.to_string())?;
    Ok(writer.bytes)
}
fn offset(cursor: Option<&str>, binding: &str, len: usize) -> Result<usize, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    if cursor.len() > 4096 {
        return Err(INVALID.into());
    }
    let cursor: Cursor = serde_json::from_str(cursor).map_err(|_| INVALID)?;
    if cursor.binding != binding || cursor.offset as usize >= len {
        return Err(INVALID.into());
    }
    Ok(cursor.offset as usize)
}
fn continuation(binding: &str, offset: usize, len: usize) -> Result<Option<String>, String> {
    if offset >= len {
        return Ok(None);
    }
    Ok(Some(
        serde_json::to_string(&Cursor {
            binding: binding.into(),
            offset: offset as u32,
        })
        .map_err(|e| e.to_string())?,
    ))
}
fn final_validate(
    authority: &impl DisplayAuthority,
    request: &DisplayRequest,
    preserved: u32,
) -> Result<(), String> {
    let _credit = reserve(
        wire_len(request, evidence::REQUEST_BYTES)?
            .checked_mul(8)
            .ok_or(LIMIT)?,
    )?;
    if authority.validate(&request.history())? != preserved {
        return Err(INVALID.into());
    }
    authority.validate_stamp(&request.store)?;
    check()
}
pub(crate) fn timeline(
    authority: &impl DisplayAuthority,
    request: &DisplayRequest,
) -> Result<Response<DisplayPage>, String> {
    let response_credit = reserve(4 << 20)?;
    let plan = plan(authority, request)?;
    let _scratch = reserve(evidence::ENVELOPE_BYTES + 6 * policy::MAX_TEXT_BYTES)?;
    let start = offset(
        request.cursor.as_deref(),
        &plan.signature,
        plan.matches.len(),
    )?;
    let mut entries = Vec::with_capacity(PAGE);
    let mut bytes = PAGE_HEADER_BYTES;
    for index in plan.matches.iter().skip(start).take(PAGE) {
        check()?;
        let entry = &plan.entries[*index as usize];
        let text = texts(authority, &plan, entry)?;
        let id = entry_id(authority, &plan, entry)?;
        if id.len() > 4096 {
            return Err(LIMIT.into());
        }
        let value = DisplayEntry {
            entry_id: id,
            kind: match entry.kind {
                Kind::Event => "event",
                Kind::Auto => "auto",
                Kind::Group(_) => "group",
                Kind::Manual(_) => "manual",
            },
            start_ms: entry.start,
            end_ms: entry.end,
            timing: match entry.timing {
                0 => "timed",
                1 => "untimed",
                _ => "unavailable",
            },
            title: history::preview(&text.title, 4096),
            detail: history::preview(&text.detail, 4096),
            source: history::preview(&text.source, 4096),
            member_count: entry.len,
            first_member_entry_id: if entry.len == 0 {
                None
            } else {
                Some(event_id(
                    authority,
                    plan.rows[plan.slots[entry.begin as usize] as usize],
                )?)
            },
            first_member_edit_key: if entry.len == 0 {
                None
            } else {
                plan.edit_keys
                    .get(&plan.slots[entry.begin as usize])
                    .cloned()
            },
            member_token: if entry.len == 0 {
                None
            } else {
                Some(
                    serde_json::to_string(&Token {
                        binding: plan.signature.clone(),
                        entry: *index,
                    })
                    .map_err(|e| e.to_string())?,
                )
            },
            item_index: if entry.len > 0 {
                Some(plan.rows[plan.slots[entry.begin as usize] as usize].item)
            } else {
                None
            },
            group_index: match entry.kind {
                Kind::Group(i) => Some(i),
                _ => None,
            },
            manual_index: match entry.kind {
                Kind::Manual(i) => Some(i),
                _ => None,
            },
            note_indices: plan.notes.get(index).cloned().unwrap_or_default(),
        };
        let size = wire_len(&value, WIRE)? + 1;
        if bytes + size > WIRE {
            if entries.is_empty() {
                return Err(LIMIT.into());
            }
            break;
        }
        bytes += size;
        entries.push(value);
    }
    let next_cursor = continuation(&plan.signature, start + entries.len(), plan.matches.len())?;
    let page = DisplayPage {
        kind: "native_case_display_page",
        owner: request.owner.clone(),
        evidence_signature: request.evidence_signature.clone(),
        authored_view_signature: plan.authored.signature.clone(),
        display_signature: plan.signature.clone(),
        preserved_count: plan.preserved,
        scope_occurrence_count: plan.rows.len() as u32,
        unavailable_count: plan.unavailable,
        display_entry_count: plan.entries.len() as u32,
        matching_display_entry_count: plan.matches.len() as u32,
        entries,
        next_cursor,
    };
    final_validate(authority, request, plan.preserved)?;
    history::finish(page, response_credit)
}
pub(crate) fn members(
    authority: &impl DisplayAuthority,
    request: &MembersRequest,
) -> Result<Response<MembersPage>, String> {
    if request.scope.cursor.is_some() || request.member_token.len() > 4096 {
        return Err(INVALID.into());
    }
    let response_credit = reserve(4 << 20)?;
    let plan = plan(authority, &request.scope)?;
    let token: Token = serde_json::from_str(&request.member_token).map_err(|_| INVALID)?;
    if request.display_signature != plan.signature
        || token.binding != plan.signature
        || !plan.matches.contains(&token.entry)
    {
        return Err(INVALID.into());
    }
    let entry = plan.entries.get(token.entry as usize).ok_or(INVALID)?;
    if entry.len == 0 {
        return Err(INVALID.into());
    }
    let cursor_binding = history::binding(&(&plan.signature, token.entry))?;
    let start = offset(
        request.cursor.as_deref(),
        &cursor_binding,
        entry.len as usize,
    )?;
    let mut members = Vec::with_capacity(PAGE);
    let mut bytes = PAGE_HEADER_BYTES;
    for index in plan.slots[entry.begin as usize..(entry.begin + entry.len) as usize]
        .iter()
        .skip(start)
        .take(PAGE)
    {
        check()?;
        let row = plan.rows[*index as usize];
        let (reference, member) = authority.member(row.item, row.ordinal)?;
        let value = DisplayMember {
            reference,
            member,
            item_index: row.item,
        };
        let size = wire_len(&value, WIRE)? + 1;
        if bytes + size > WIRE {
            if members.is_empty() {
                return Err(LIMIT.into());
            }
            break;
        }
        bytes += size;
        members.push(value);
    }
    let next_cursor = continuation(&cursor_binding, start + members.len(), entry.len as usize)?;
    let page = MembersPage {
        kind: "native_case_display_members",
        display_signature: plan.signature.clone(),
        entry_id: entry_id(authority, &plan, entry)?,
        member_count: entry.len,
        members,
        next_cursor,
    };
    final_validate(authority, &request.scope, plan.preserved)?;
    history::finish(page, response_credit)
}

/// The small container index borrows the already charged native authority. Root
/// leases remain operation-scoped inside validation; no live profile switch exists.
pub(crate) struct NativeAuthority<'a> {
    root: &'a Path,
    lease: &'a evidence::CaseAuthorityLease,
    containers: BTreeMap<u32, &'a evidence::VerifiedContainer>,
    _credit: Lease,
}
impl<'a> NativeAuthority<'a> {
    pub(crate) fn new(
        root: &'a Path,
        lease: &'a evidence::CaseAuthorityLease,
    ) -> Result<Self, String> {
        let count = lease.case().containers().count();
        let credit = reserve(count.checked_mul(128).ok_or(LIMIT)?)?;
        let mut containers = BTreeMap::new();
        for (location, container) in lease.case().containers() {
            if let evidence::ContainerLocation::ItemRows { index } = location {
                containers.insert(*index, container.as_ref());
            }
        }
        Ok(Self {
            root,
            lease,
            containers,
            _credit: credit,
        })
    }
}
impl HistoryAuthority for NativeAuthority<'_> {
    fn blocked_alias(&self, alias: &str) -> Option<crate::case_evidence_anchors::BlockedState> {
        self.lease.blocked_alias(alias)
    }
    fn validate(&self, request: &history::TimelineRequest) -> Result<u32, String> {
        HistoryAuthority::validate(self.lease, request)
    }
    fn visit(
        &self,
        visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        HistoryAuthority::visit(self.lease, visitor)
    }
}
impl DisplayAuthority for NativeAuthority<'_> {
    fn validate_stamp(&self, stamp: &StoreStamp) -> Result<(), String> {
        evidence::validate_view_stamp(self.root, stamp)
    }
    fn item(&self, index: u32) -> Result<&Value, String> {
        self.lease
            .case()
            .metadata()
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.get(index as usize))
            .ok_or_else(|| INVALID.into())
    }
    fn member(&self, item: u32, ordinal: u32) -> Result<(EvidenceReference, MemberHandle), String> {
        let container = self.containers.get(&item).ok_or(INVALID)?;
        let member = container
            .manifest()
            .members
            .get(ordinal as usize)
            .ok_or(INVALID)?;
        let reference = container.reference();
        Ok((
            EvidenceReference::Committed(reference.clone()),
            MemberHandle {
                container_id: reference.container_id.clone(),
                manifest_id: reference.manifest_id.clone(),
                occurrence_id: member.occurrence_id.clone(),
            },
        ))
    }
    fn read(
        &self,
        item: u32,
        ordinal: u32,
        visitor: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        let container = self.containers.get(&item).ok_or(INVALID)?;
        container.visit_envelopes(ordinal as usize..ordinal as usize + 1, |_, raw| {
            visitor(raw)?;
            Ok(evidence::Visit::Continue)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn uuid(n: u128) -> String {
        uuid::Uuid::from_u128(n).to_string()
    }
    struct Fixture {
        request: DisplayRequest,
        items: Vec<Value>,
        records: Vec<Vec<String>>,
        origins: Vec<Vec<u32>>,
        validations: Cell<usize>,
        invalidate_at: Cell<usize>,
        blocked: BTreeSet<String>,
    }
    impl Fixture {
        fn new(count: usize) -> Self {
            let request = DisplayRequest {
                store: StoreStamp {
                    store_id: uuid(1),
                    epoch: uuid(2),
                    revision: "1".into(),
                },
                owner: EvidenceOwner {
                    store_id: uuid(1),
                    analysis_id: uuid(3),
                    case_id: "case".into(),
                },
                evidence_signature: "a".repeat(64),
                authored_view_json:
                    json!({"timeline":{"groups":[],"annotations":[],"edits":{}},"manual":[]})
                        .to_string(),
                station_id: None,
                filters: vec![],
                from_ms: None,
                to_ms: None,
                include_untimed: true,
                display_query: String::new(),
                cursor: None,
            };
            let mut fixture = Self {
                request,
                items: vec![json!({"id":"i","label":"source"})],
                records: vec![vec![]],
                origins: vec![vec![]],
                validations: Cell::new(0),
                invalidate_at: Cell::new(usize::MAX),
                blocked: BTreeSet::new(),
            };
            for i in 0..count {
                fixture.push(0, Some(i as i64), "same", "first", "body");
            }
            fixture
        }
        fn push(&mut self, item: usize, time: Option<i64>, code: &str, name: &str, message: &str) {
            let i = self.records[item].len();
            let mut event = Event::empty();
            event.id = i;
            event.event_ref = format!("r{i}");
            event.timestamp = time;
            event.code = code.into();
            event.name = name.into();
            event.message = message.into();
            self.records[item].push(serde_json::to_string(&event).unwrap());
            self.origins[item].push(i as u32);
        }
        fn view(&mut self, config: Value, manual: Value) {
            self.request.authored_view_json =
                json!({"timeline":config,"manual":manual}).to_string();
        }
        fn native(&self, item: u32, ordinal: u32) -> String {
            history::entry_id(&self.member(item, ordinal).unwrap().1)
        }
        fn page(&self) -> Response<DisplayPage> {
            timeline(self, &self.request).unwrap()
        }
    }
    impl HistoryAuthority for Fixture {
        fn blocked_alias(&self, alias: &str) -> Option<crate::case_evidence_anchors::BlockedState> {
            self.blocked
                .contains(alias)
                .then_some(crate::case_evidence_anchors::BlockedState::Ambiguous)
        }
        fn validate(&self, request: &history::TimelineRequest) -> Result<u32, String> {
            if request.store != self.request.store.identity()
                || request.owner != self.request.owner
                || request.evidence_signature != self.request.evidence_signature
            {
                return Err(INVALID.into());
            }
            Ok(self.records.iter().map(Vec::len).sum::<usize>() as u32)
        }
        fn visit(
            &self,
            visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
        ) -> Result<(), String> {
            for (item, records) in self.records.iter().enumerate() {
                for (ordinal, raw) in records.iter().enumerate() {
                    visitor(
                        HistoricalItem {
                            item_index: item as u32,
                            original_row_index: self.origins[item][ordinal],
                            item: &self.items[item],
                        },
                        self.member(item as u32, ordinal as u32)?.1,
                        raw,
                    )?;
                }
            }
            Ok(())
        }
    }
    impl DisplayAuthority for Fixture {
        fn validate_stamp(&self, stamp: &StoreStamp) -> Result<(), String> {
            self.validations.set(self.validations.get() + 1);
            if stamp != &self.request.store || self.validations.get() >= self.invalidate_at.get() {
                Err(INVALID.into())
            } else {
                Ok(())
            }
        }
        fn item(&self, item: u32) -> Result<&Value, String> {
            self.items.get(item as usize).ok_or_else(|| INVALID.into())
        }
        fn member(
            &self,
            item: u32,
            ordinal: u32,
        ) -> Result<(EvidenceReference, MemberHandle), String> {
            if self
                .records
                .get(item as usize)
                .and_then(|v| v.get(ordinal as usize))
                .is_none()
            {
                return Err(INVALID.into());
            }
            let reference = evidence::EvidenceRef {
                kind: evidence::CommittedKind::NativeEvidence,
                schema_version: 1,
                owner: self.request.owner.clone(),
                container_id: uuid(100 + item as u128),
                manifest_id: uuid(200 + item as u128),
                manifest_sha256: "b".repeat(64),
                member_count: self.records[item as usize].len() as u32,
            };
            let member = MemberHandle {
                container_id: reference.container_id.clone(),
                manifest_id: reference.manifest_id.clone(),
                occurrence_id: uuid(1000 + ordinal as u128),
            };
            Ok((EvidenceReference::Committed(reference), member))
        }
        fn read(
            &self,
            item: u32,
            ordinal: u32,
            visitor: &mut dyn FnMut(&str) -> Result<(), String>,
        ) -> Result<(), String> {
            visitor(
                self.records
                    .get(item as usize)
                    .and_then(|v| v.get(ordinal as usize))
                    .ok_or(INVALID)?,
            )
        }
    }
    #[test]
    fn complete_auto_runs_search_after_collapse_and_expand_exact_slots() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(251);
        f.origins[0] = (0..251).map(|i| i * 7).collect();
        let page = f.page();
        assert_eq!(
            (page.scope_occurrence_count, page.display_entry_count),
            (251, 2)
        );
        assert_eq!(
            page.entries
                .iter()
                .map(|e| (e.kind, e.member_count))
                .collect::<Vec<_>>(),
            vec![("auto", 250), ("event", 1)]
        );
        let token = page.entries[0].member_token.clone().unwrap();
        let signature = page.display_signature.clone();
        drop(page);
        let mut request = MembersRequest {
            scope: f.request.clone(),
            display_signature: signature,
            member_token: token,
            cursor: None,
        };
        let first = members(&f, &request).unwrap();
        assert_eq!(first.members.len(), 128);
        assert_eq!(first.members[127].member.occurrence_id, uuid(1127));
        request.cursor = first.next_cursor.clone();
        drop(first);
        let second = members(&f, &request).unwrap();
        assert_eq!(second.members.len(), 122);
        assert_eq!(second.members[0].member.occurrence_id, uuid(1128));
        assert!(second.next_cursor.is_none());
        drop(second);
        f.request.display_query = "body".into();
        let page = f.page();
        assert_eq!(page.matching_display_entry_count, 1);
        assert_eq!(page.entries[0].member_count, 1);
        drop(page);
        f.request.display_query = "250 eventos na sequência".into();
        let page = f.page();
        assert_eq!(page.matching_display_entry_count, 1);
        assert_eq!(page.entries[0].member_count, 250);
    }
    #[test]
    fn adjacent_gaps_and_full_text_search_are_global_and_preview_independent() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(0);
        for i in 0..4 {
            f.push(0, Some(i * 300_000), "same", "repeat", "body");
        }
        assert_eq!(f.page().entries[0].member_count, 4);
        f.view(json!({"compact":false}), json!([]));
        for i in 4..260 {
            f.push(0, Some(i * 300_000), "same", "repeat", "body");
        }
        let last = f.native(0, 259);
        f.view(json!({"compact":false,"edits":{last.clone():{"title":format!("{}needle", "x".repeat(5000))}}}),json!([]));
        f.request.display_query = "needle".into();
        let page = f.page();
        assert_eq!(page.matching_display_entry_count, 1);
        assert_eq!(page.entries[0].entry_id, last);
        assert!(!page.entries[0].title.complete);
        assert!(!page.entries[0].title.text.contains("needle"));
    }
    #[test]
    fn explicit_order_overlap_notes_duplicates_and_hidden_parents_match_legacy() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(3);
        f.items
            .push(json!({"id":"j","label":"authored first source"}));
        f.records.push(vec![]);
        f.origins.push(vec![]);
        f.push(1, Some(3), "other", "third", "body");
        let a = f.native(0, 0);
        let b = f.native(0, 1);
        let c = f.native(0, 2);
        let d = f.native(1, 0);
        f.view(json!({"groups":[{"id":"g1","ids":[d,a,b],"name":"first"},{"id":"g2","ids":[b,c,c],"name":"second"}],"annotations":[{"id":"parent","anchor":b,"arrowHidden":true,"text":"hidden"},{"id":"child","anchor":"parent","text":"note needle"},{"id":"child","anchor":"g1","text":"duplicate first per entry"},{"id":"cycle-a","anchor":"cycle-b","text":"bad"},{"id":"cycle-b","anchor":"cycle-a","text":"bad"}]}),json!([]));
        let page = f.page();
        assert_eq!(page.entries.len(), 2);
        let first = page.entries.iter().find(|e| e.entry_id == "g1").unwrap();
        assert_eq!(first.source.text, "authored first source");
        assert_eq!(first.first_member_entry_id.as_deref(), Some(d.as_str()));
        assert_eq!(first.note_indices, vec![2]);
        let second = page.entries.iter().find(|e| e.entry_id == "g2").unwrap();
        assert_eq!(second.note_indices, vec![1]);
        assert_eq!(second.member_count, 3);
        drop(page);
        f.request.display_query = "note needle".into();
        let page = f.page();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].entry_id, "g2");
    }
    #[test]
    fn original_filters_frozen_aliases_and_missing_group_note_fallback() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(3);
        f.blocked.insert("e:i:r1".into());
        f.view(json!({"compact":false,"groups":[{"id":"missing","ids":["e:i:r0","e:i:r1"]}],"annotations":[{"id":"note","anchor":"missing","text":"fallback"}]}),json!([]));
        f.request.display_query = "fallback".into();
        let page = f.page();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].entry_id, f.native(0, 0));
        drop(page);
        f.request.filters.push(crate::query::Filter {
            column: "event_ref".into(),
            op: "equals".into(),
            value: "r2".into(),
            value2: None,
        });
        assert_eq!(f.page().matching_display_entry_count, 0);
    }
    #[test]
    fn dates_keep_boundaries_extremes_and_fractional_manual_milestones() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(0);
        for t in [
            Some(i64::MIN),
            Some(-8_640_000_000_000_001),
            Some(-8_640_000_000_000_000),
            Some(0),
            Some(8_640_000_000_000_000),
            Some(8_640_000_000_000_001),
            Some(i64::MAX),
            None,
        ] {
            f.push(0, t, "same", "date record", "body");
        }
        f.view(json!({"compact":false}),json!([{"id":"fraction","name":"fractional milestone","start":0.5,"end":1.75},{"id":"invalid","start":9e15}]));
        let page = f.page();
        assert_eq!(
            (
                page.scope_occurrence_count,
                page.unavailable_count,
                page.display_entry_count
            ),
            (8, 4, 9)
        );
        assert_eq!(
            page.entries
                .iter()
                .filter(|e| e.timing == "unavailable")
                .count(),
            4
        );
        assert!(page
            .entries
            .iter()
            .filter(|e| e.timing == "unavailable")
            .all(|e| e.start_ms.is_none() && e.title.text == "date record"));
        let manual = page.entries.iter().find(|e| e.kind == "manual").unwrap();
        assert_eq!(
            (manual.start_ms, manual.end_ms, manual.member_count),
            (Some(0.5), Some(1.75), 0)
        );
        assert!(manual.member_token.is_none());
        drop(page);
        f.request.display_query = "fractional milestone".into();
        assert_eq!(f.page().entries[0].kind, "manual");
        f.request.include_untimed = false;
        let graph = f.page();
        assert_eq!(
            (graph.scope_occurrence_count, graph.unavailable_count),
            (3, 4)
        );
        assert!(graph.entries.iter().all(|entry| entry.start_ms.is_some()));
    }
    #[test]
    fn query_and_metadata_revisions_bind_pages_expansion_and_final_validation() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(130);
        f.view(json!({"compact":false}), json!([]));
        let page = f.page();
        let cursor = page.next_cursor.clone().unwrap();
        let token = page.entries[0].member_token.clone().unwrap();
        let signature = page.display_signature.clone();
        drop(page);
        f.request.cursor = Some(cursor.clone());
        assert_eq!(f.page().entries.len(), 2);
        let mut changed = f.request.clone();
        changed.display_query = "first".into();
        assert!(timeline(&f, &changed).is_err());
        changed = f.request.clone();
        changed.store.revision = "2".into();
        assert!(timeline(&f, &changed).is_err());
        f.request.cursor = None;
        f.request.display_query = "body".into();
        assert!(members(
            &f,
            &MembersRequest {
                scope: f.request.clone(),
                display_signature: signature,
                member_token: token,
                cursor: None
            }
        )
        .is_err());
        f.validations.set(0);
        f.invalidate_at.set(3);
        assert!(timeline(&f, &f.request).is_err());
    }
    #[test]
    fn manual_only_matches_and_saved_detection_context_use_full_display_policy() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(1);
        f.items[0]["detections"] =
            json!([{"name":"saved detection needle","evidence_level":3,"event_refs":["r0"]}]);
        f.request.display_query = "saved detection needle".into();
        let page = f.page();
        assert_eq!(page.entries.len(), 1);
        assert!(page.entries[0]
            .detail
            .text
            .contains("saved detection needle"));
        drop(page);
        let mut empty = Fixture::new(0);
        empty.view(
            json!({}),
            json!([{"id":"only","start":0,"name":"manual only"}]),
        );
        empty.request.display_query = "manual only".into();
        let page = empty.page();
        assert_eq!(
            (
                page.preserved_count,
                page.scope_occurrence_count,
                page.matching_display_entry_count
            ),
            (0, 0, 1)
        );
        assert_eq!(page.entries[0].member_count, 0);
    }
    #[test]
    fn explicit_limits_and_cancellation_never_publish_partial_search() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(2);
        f.request.display_query = "x".repeat(201);
        assert!(timeline(&f, &f.request).is_err());
        f.request.display_query = String::new();
        let id = format!("display-cancel-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            crate::operations::cancel_id(&id);
            timeline(&f, &f.request)
        });
        assert!(result.is_err());
    }
    #[test]
    fn equal_dates_preserve_current_manifest_order_with_reversed_origins() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(0);
        f.push(0, Some(7), "same", "current first", "first body");
        f.push(0, Some(7), "same", "current second", "second body");
        f.origins[0] = vec![9, 2];
        let page = f.page();
        assert_eq!(page.entries[0].title.text, "current first");
        assert_eq!(
            page.entries[0].first_member_entry_id.as_deref(),
            Some(f.native(0, 0).as_str())
        );
        drop(page);
        let history = history::timeline(&f, &f.request.history()).unwrap();
        assert_eq!(
            history
                .entries
                .iter()
                .map(|e| e.original_row_index)
                .collect::<Vec<_>>(),
            vec![9, 2]
        );
    }
    #[test]
    fn saved_detection_association_work_exhaustion_is_explicit_and_cumulative() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(130);
        f.view(json!({"compact":false}), json!([]));
        f.items[0]["detections"] =
            json!([{"name":"unmatched","event_refs": ["x".repeat(300_000)]}]);
        f.request.display_query = "does not match".into();
        let error = timeline(&f, &f.request).err().unwrap();
        assert_eq!(error, ASSOCIATION_LIMIT);
    }
    #[test]
    fn interleaved_containers_and_color_only_legacy_edits_keep_native_identity() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(0);
        f.push(0, Some(0), "same", "first", "body");
        f.push(0, Some(2), "same", "last", "body");
        f.items.push(json!({"id":"j","label":"other source"}));
        f.records.push(vec![]);
        f.origins.push(vec![]);
        f.push(1, Some(1), "same", "middle", "body");
        f.view(json!({"edits":{"e:i:r0":{"color":"#123456"}}}), json!([]));
        let page = f.page();
        assert_eq!(page.entries.len(), 3);
        assert!(page.entries.iter().all(|entry| entry.kind == "event"));
        assert_eq!(
            page.entries[0].first_member_edit_key.as_deref(),
            Some("e:i:r0")
        );
        assert_eq!(
            page.entries[0].first_member_entry_id.as_deref(),
            Some(f.native(0, 0).as_str())
        );
        drop(page);
        f.blocked.insert("e:i:r0".into());
        assert!(f.page().entries[0].first_member_edit_key.is_none());
    }
    #[test]
    fn composed_display_wire_keeps_three_hundred_members_and_manual_note_search() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(300);
        let ids: Vec<_> = (0..300).map(|i| format!("e:i:r{i}")).collect();
        let config = json!({"groups":[{"id":"large-group","ids":ids,"name":"Large group"}],"edits":{"e:i:r0":{"color":"#123456"}},"annotations":[{"id":"manual-note","anchor":"m:milestone","text":"manual note needle"}]});
        let manual = json!([{"id":"milestone","name":"Authored milestone","start":400.5,"description":"manual detail"}]);
        f.view(config.clone(), manual.clone());
        let page = f.page();
        assert_eq!(page.entries[0].member_count, 300);
        assert_eq!(
            page.entries[0].first_member_edit_key.as_deref(),
            Some("e:i:r0")
        );
        let page_json = serde_json::to_value(&page).unwrap();
        let mut request = MembersRequest {
            scope: f.request.clone(),
            display_signature: page.display_signature.clone(),
            member_token: page.entries[0].member_token.clone().unwrap(),
            cursor: None,
        };
        drop(page);
        let mut member_pages = Vec::new();
        loop {
            let page = members(&f, &request).unwrap();
            request.cursor = page.next_cursor.clone();
            member_pages.push(serde_json::to_value(&page).unwrap());
            if request.cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            member_pages
                .iter()
                .map(|page| page["members"].as_array().unwrap().len())
                .collect::<Vec<_>>(),
            vec![128, 128, 44]
        );
        let reference = f.member(0, 0).unwrap().0;
        let case_view = evidence::CaseViewDocument {
            evidence_view_version: 1,
            store: f.request.store.clone(),
            active: Some(f.request.owner.case_id.clone()),
            cases: vec![
                json!({"id":f.request.owner.case_id,"name":"Display parity","timeline":config,"manual":manual,"items":[{"id":"i","label":"source","rows":{"kind":"native_evidence_container","reference":reference,"preservedCount":300,"preview":null}}]}),
            ],
            case_evidence: vec![evidence::CaseEvidenceState::Ready(
                evidence::CaseEvidenceSummary {
                    owner: f.request.owner.clone(),
                    evidence_signature: f.request.evidence_signature.clone(),
                    preserved_count: 300,
                    legacy_item_aliases: vec![],
                },
            )],
            diagnostics: vec![],
        };
        let scope = f.request.clone();
        f.request.display_query = "manual note needle".into();
        let manual_page = f.page();
        assert_eq!(manual_page.entries.len(), 1);
        assert_eq!(manual_page.entries[0].kind, "manual");
        if let Some(path) = std::env::var_os("LOGS_DISPLAY_WIRE_FIXTURE") {
            let fixture = json!({"caseView":case_view,"request":scope,"page":page_json,"memberPages":member_pages,"manualRequest":f.request,"manualPage":serde_json::to_value(&manual_page).unwrap()});
            std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
        }
    }
    #[test]
    fn saved_detection_large_names_consume_association_allowance_before_copying() {
        let _lock = LOCK.lock().unwrap();
        let f = Fixture::new(0);
        let plan = plan(&f, &f.request).unwrap();
        let detections = vec![json!({"event_refs":[""],"name":"x".repeat(1<<20)})];
        for _ in 0..31 {
            admit_associations(&plan, 0, &detections).unwrap();
        }
        assert_eq!(
            admit_associations(&plan, 0, &detections).unwrap_err(),
            ASSOCIATION_LIMIT
        );
    }
    #[test]
    fn manual_negative_zero_ties_keep_the_legacy_creation_order() {
        let _lock = LOCK.lock().unwrap();
        let mut f = Fixture::new(1);
        f.view(
            json!({"compact":false}),
            json!([{"id":"zero","name":"manual zero","start":-0.0}]),
        );
        let page = f.page();
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            vec!["event", "manual"]
        );
        assert_eq!(
            page.entries[1].start_ms.unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
    }
}
