//! Explicit reads of preserved Case members. These APIs never consult current
//! analysis overlays, exclusion state or the active Dataset, and never emit Events.
use crate::{
    case_evidence::{self, EvidenceOwner, EvidenceReference, MemberHandle, RawJson, StoreIdentity},
    case_work_budget::{self, Lease},
    model::Event,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{borrow::Cow, collections::BTreeMap, io::Write, ops::Deref};

const INVALID: &str = "CASE_HISTORY_IDENTITY_CHANGED";
const LIMIT: &str = "CASE_HISTORY_LIMIT";
const FILTER_UNAVAILABLE: &str = "CASE_HISTORY_FILTER_UNAVAILABLE";
const RESPONSE_BYTES: usize = 1 << 20;
const RESPONSE_CREDIT: usize = 4 << 20;
const PAGE_ROWS: usize = 128;
const CELL_BYTES: usize = 4 << 10;
const PREVIEW_BYTES: usize = 256 << 10;
const MAX_TIMESTAMP: i64 = 8_640_000_000_000_000;
const FIXED_COLUMNS: &[&str] = &[
    "id",
    "event_ref",
    "timestamp",
    "source",
    "level",
    "code",
    "name",
    "description",
    "message",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemberRequest {
    pub store: StoreIdentity,
    pub reference: EvidenceReference,
    pub member: MemberHandle,
}
/// Implemented only by the storage resolver. Admission is a verified reference,
/// including prepared/undo authority, rather than a dense analytical row index.
pub(crate) trait MemberAuthority {
    fn validate(&self, request: &MemberRequest) -> Result<(), String>;
    fn envelope(&self, member: &MemberHandle) -> Result<String, String>;
}

/// Ordered preview reads are tied to one committed or still-pending manifest.
pub(crate) trait PreviewAuthority: MemberAuthority {
    fn validate_preview(&self, request: &case_evidence::PreviewRequest) -> Result<usize, String>;
    fn member_at(&self, index: usize) -> Result<MemberHandle, String>;
}

/// IPC serialization retains the response allocation's live-work reservation.
/// Dropping temporary decoded records cannot release this returned allocation.
pub(crate) struct Response<T> {
    value: T,
    _credit: Lease,
}
impl<T> Deref for Response<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T: Serialize> Serialize for Response<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextPreview {
    pub text: String,
    pub complete: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldPreview {
    pub column: String,
    #[serde(rename = "type")]
    pub value_type: &'static str,
    pub text: String,
    pub complete: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberDetail {
    pub kind: &'static str,
    pub member: MemberHandle,
    pub fields: Vec<FieldPreview>,
    pub fields_total: u32,
    pub next_cursor: Option<String>,
    pub raw: TextPreview,
    pub envelope: TextPreview,
    pub java_trace_available: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldText {
    pub kind: &'static str,
    pub member: MemberHandle,
    pub column: String,
    pub present: bool,
    #[serde(rename = "type")]
    pub value_type: Option<&'static str>,
    pub text: Option<String>,
    pub complete: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JavaDetail {
    pub member: MemberHandle,
    pub state: &'static str,
    pub trace: Option<crate::java_stacktrace::Trace>,
    pub reason: Option<&'static str>,
}

struct CountWriter {
    bytes: usize,
    limit: usize,
}
impl Write for CountWriter {
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
fn encoded_len(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    let mut count = CountWriter { bytes: 0, limit };
    serde_json::to_writer(&mut count, value).map_err(|_| LIMIT.to_string())?;
    Ok(count.bytes)
}
fn reserve(bytes: usize) -> Result<Lease, String> {
    crate::case_cache::reserve_work(case_work_budget::global(), bytes)
}
pub(crate) fn finish<T: Serialize>(value: T, credit: Lease) -> Result<Response<T>, String> {
    encoded_len(&value, RESPONSE_BYTES)?;
    Ok(Response {
        value,
        _credit: credit,
    })
}
pub(crate) fn preview(text: &str, limit: usize) -> TextPreview {
    // Budget the JSON string escaping, not just the raw UTF-8 prefix.
    let mut wire = 2usize;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let bytes = match ch {
            '"' | '\\' => 2,
            '\u{0}'..='\u{1f}' => 6,
            _ => ch.len_utf8(),
        };
        if wire.saturating_add(bytes) > limit {
            break;
        }
        wire += bytes;
        end = index + ch.len_utf8();
    }
    TextPreview {
        text: text[..end].into(),
        complete: end == text.len(),
    }
}
fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
fn canonical<'a>(
    event: &'a Event,
    column: &str,
) -> Result<(Option<&'static str>, Option<Cow<'a, str>>), String> {
    if column.is_empty() || column == "*" || column.len() > 1024 {
        return Err(LIMIT.into());
    }
    if column == "timestamp" && event.timestamp.is_none() {
        return Ok((None, None));
    }
    if let Some(value @ (Value::Array(_) | Value::Object(_))) = event
        .fields
        .get(column)
        .filter(|_| !FIXED_COLUMNS.contains(&column) && column != "raw")
    {
        // col_ref allocates canonical structured text. Check before it does.
        encoded_len(value, RESPONSE_BYTES)?;
    }
    let text = event.col_ref(column);
    let kind = match column {
        "id" | "timestamp" => Some("number"),
        col if FIXED_COLUMNS.contains(&col) || col == "raw" => Some("string"),
        _ => event
            .fields
            .get(column)
            .map(value_type)
            .or_else(|| text.as_ref().map(|_| "string")),
    };
    Ok((kind, text))
}
fn validate_member_request(request: &MemberRequest) -> Result<(), String> {
    if request.store.store_id != request.reference.owner().store_id
        || request.reference.container_id() != request.member.container_id
        || request.reference.manifest_id() != request.member.manifest_id
        || request.reference.owner().case_id.len() > 1024
    {
        return Err(INVALID.into());
    }
    for id in [
        &request.store.store_id,
        &request.store.epoch,
        &request.member.container_id,
        &request.member.manifest_id,
        &request.member.occurrence_id,
        &request.reference.owner().analysis_id,
    ] {
        uuid::Uuid::parse_str(id).map_err(|_| INVALID)?;
    }
    Ok(())
}
struct DecodedMember {
    envelope: String,
    event: Event,
    _encoded: Lease,
    _decoded: Lease,
}
fn decode_member(
    authority: &impl MemberAuthority,
    request: &MemberRequest,
) -> Result<DecodedMember, String> {
    validate_member_request(request)?;
    authority.validate(request)?;
    let encoded = reserve(case_evidence::ENVELOPE_BYTES)?;
    let envelope = authority.envelope(&request.member)?;
    let plan = case_evidence::preflight_envelope(&envelope)?;
    let decoded = reserve(plan.materialization_credit)?;
    let event = case_evidence::materialize_event(&envelope, plan)?;
    Ok(DecodedMember {
        envelope,
        event,
        _encoded: encoded,
        _decoded: decoded,
    })
}
pub(crate) fn binding(value: &impl Serialize) -> Result<String, String> {
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
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", writer.0.finalize()))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailCursor {
    binding: String,
    offset: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewCursor {
    binding: String,
    offset: usize,
}

pub(crate) fn preview_page(
    authority: &impl PreviewAuthority,
    request: &case_evidence::PreviewRequest,
) -> Result<Response<case_evidence::EvidencePreviewPage>, String> {
    use crate::page_projection::{Cell, ValueType};
    encoded_len(request, case_evidence::REQUEST_BYTES)?;
    if request.limit == 0
        || request.limit > case_evidence::PREVIEW_ROWS
        || request.columns.len() > case_evidence::PREVIEW_COLUMNS
        || request
            .columns
            .iter()
            .any(|c| c.is_empty() || c == "*" || c.len() > 1024)
        || request
            .columns
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != request.columns.len()
    {
        return Err(LIMIT.into());
    }
    let count = authority.validate_preview(request)?;
    let expected = binding(&(
        &request.store,
        &request.reference,
        &request.columns,
        request.limit,
    ))?;
    let offset = if let Some(cursor) = &request.cursor {
        if cursor.len() > 256 {
            return Err(INVALID.into());
        }
        let parsed: PreviewCursor = serde_json::from_str(cursor).map_err(|_| INVALID)?;
        if parsed.binding != expected {
            return Err(INVALID.into());
        }
        parsed.offset
    } else {
        0
    };
    if offset > count {
        return Err(INVALID.into());
    }
    let credit = reserve(RESPONSE_CREDIT)?;
    let mut page = case_evidence::EvidencePreviewPage {
        kind: "evidence_preview_page",
        columns: request.columns.clone(),
        rows: Vec::new(),
        total: u32::try_from(count).map_err(|_| LIMIT)?,
        next_cursor: None,
    };
    for index in offset..count.min(offset.saturating_add(request.limit as usize)) {
        crate::operations::check()?;
        let member = authority.member_at(index)?;
        let member_request = MemberRequest {
            store: request.store.clone(),
            reference: request.reference.clone(),
            member: member.clone(),
        };
        let decoded = decode_member(authority, &member_request)?;
        let mut cells = Vec::with_capacity(request.columns.len());
        for column in &request.columns {
            let (kind, value) = canonical(&decoded.event, column)?;
            let cell = match kind {
                None => Cell::Missing,
                Some("null") => Cell::Null,
                Some(kind) => {
                    let value_type = match kind {
                        "number" => ValueType::Number,
                        "boolean" => ValueType::Boolean,
                        "array" => ValueType::Array,
                        "object" => ValueType::Object,
                        _ => ValueType::String,
                    };
                    let text = preview(value.as_deref().unwrap_or(""), CELL_BYTES - 128);
                    Cell::Preview {
                        value_type,
                        text: text.text,
                        incomplete: !text.complete,
                    }
                }
            };
            cells.push(cell);
        }
        page.rows.push(case_evidence::EvidencePreviewRow {
            kind: "evidence_preview",
            member,
            cells,
        });
    }
    let next = offset + page.rows.len();
    if next < count {
        page.next_cursor = Some(
            serde_json::to_string(&PreviewCursor {
                binding: expected,
                offset: next,
            })
            .map_err(|e| e.to_string())?,
        );
    }
    if authority.validate_preview(request)? != count {
        return Err(INVALID.into());
    }
    crate::operations::check()?;
    finish(page, credit)
}

pub(crate) fn detail(
    authority: &impl MemberAuthority,
    request: &MemberRequest,
    cursor: Option<&str>,
) -> Result<Response<MemberDetail>, String> {
    let record = decode_member(authority, request)?;
    let credit = reserve(RESPONSE_CREDIT)?;
    let expected = binding(request)?;
    let offset = if let Some(cursor) = cursor {
        if cursor.len() > 256 {
            return Err(INVALID.into());
        }
        let cursor: DetailCursor = serde_json::from_str(cursor).map_err(|_| INVALID)?;
        if cursor.binding != expected {
            return Err(INVALID.into());
        }
        cursor.offset
    } else {
        0
    };
    let columns = || {
        FIXED_COLUMNS.iter().copied().chain(
            record
                .event
                .fields
                .keys()
                .map(String::as_str)
                .filter(|column| !FIXED_COLUMNS.contains(column) && *column != "raw"),
        )
    };
    let fields_total = u32::try_from(columns().count()).map_err(|_| LIMIT)?;
    if offset > fields_total as usize {
        return Err(INVALID.into());
    }
    // This optional probe must not prevent viewing a preserved non-Java row
    // whose first raw line exceeds Java's inspection cap. Explicit Java reads
    // still return the parser's precise refusal.
    let java_trace_available = crate::sources::java_trace_for_event(&record.event)
        .map(|trace| trace.is_some())
        .unwrap_or(false);
    let mut response = MemberDetail {
        kind: "preserved_member_detail",
        member: request.member.clone(),
        fields: Vec::new(),
        fields_total,
        next_cursor: None,
        raw: preview(&record.event.raw, PREVIEW_BYTES),
        envelope: preview(&record.envelope, PREVIEW_BYTES),
        java_trace_available,
    };
    let mut remaining = RESPONSE_BYTES
        .checked_sub(encoded_len(&response, RESPONSE_BYTES)? + 512)
        .ok_or(LIMIT)?;
    for column in columns().skip(offset).take(PAGE_ROWS) {
        let (kind, text) = canonical(&record.event, column)?;
        let text = preview(text.as_deref().unwrap_or(""), CELL_BYTES);
        let field = FieldPreview {
            column: column.into(),
            value_type: kind.unwrap_or("missing"),
            text: text.text,
            complete: text.complete,
        };
        let bytes = encoded_len(&field, RESPONSE_BYTES)?.saturating_add(1);
        if bytes > remaining {
            break;
        }
        remaining -= bytes;
        response.fields.push(field);
    }
    let next = offset + response.fields.len();
    if next < fields_total as usize {
        if next == offset {
            return Err(LIMIT.into());
        }
        response.next_cursor = Some(
            serde_json::to_string(&DetailCursor {
                binding: expected,
                offset: next,
            })
            .map_err(|e| e.to_string())?,
        );
    }
    crate::operations::check()?;
    authority.validate(request)?;
    finish(response, credit)
}
pub(crate) fn field_text(
    authority: &impl MemberAuthority,
    request: &MemberRequest,
    column: &str,
) -> Result<Response<FieldText>, String> {
    let record = decode_member(authority, request)?;
    let credit = reserve(RESPONSE_CREDIT)?;
    let (kind, text) = canonical(&record.event, column)?;
    let mut response = FieldText {
        kind: "preserved_field_text",
        member: request.member.clone(),
        column: column.into(),
        present: text.is_some(),
        value_type: kind,
        text: None,
        complete: true,
    };
    let available = RESPONSE_BYTES
        .checked_sub(encoded_len(&response, RESPONSE_BYTES)?)
        .and_then(|n| n.checked_add(4))
        .ok_or(LIMIT)?;
    encoded_len(&text.as_deref(), available)?;
    response.text = text.map(Cow::into_owned);
    crate::operations::check()?;
    authority.validate(request)?;
    finish(response, credit)
}
pub(crate) fn java(
    authority: &impl MemberAuthority,
    request: &MemberRequest,
) -> Result<Response<JavaDetail>, String> {
    let record = decode_member(authority, request)?;
    let credit = reserve(RESPONSE_CREDIT)?;
    let trace = crate::sources::java_trace_for_event(&record.event)?;
    let reason = if trace.is_some() {
        None
    } else if record.event.raw.is_empty() {
        Some("raw_unavailable")
    } else {
        Some("not_java")
    };
    let response = JavaDetail {
        member: request.member.clone(),
        state: if trace.is_some() {
            "available"
        } else {
            "unavailable"
        },
        trace,
        reason,
    };
    crate::operations::check()?;
    authority.validate(request)?;
    finish(response, credit)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimelineRequest {
    pub store: StoreIdentity,
    pub owner: EvidenceOwner,
    pub evidence_signature: String,
    /// Exact captured authored view; no preserved record values enter this JSON.
    pub authored_view_json: String,
    pub station_id: Option<String>,
    #[serde(default)]
    pub filters: Vec<crate::query::Filter>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    #[serde(default = "include_untimed")]
    pub include_untimed: bool,
    pub cursor: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}
fn include_untimed() -> bool {
    true
}
pub(crate) struct HistoricalItem<'a> {
    pub item_index: u32,
    pub original_row_index: u32,
    pub item: &'a Value,
}
/// Storage supplies every ordered items[].rows occurrence across ALL stations.
/// The core validates the full-scope count before publishing alias resolutions.
pub(crate) trait HistoryAuthority {
    /// One-time original-scope ambiguity stays authoritative after removals.
    fn blocked_alias(&self, _alias: &str) -> Option<crate::case_evidence_anchors::BlockedState> {
        None
    }
    fn validate(&self, request: &TimelineRequest) -> Result<u32, String>;
    fn visit(
        &self,
        visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
    ) -> Result<(), String>;
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineEntry {
    pub entry_id: String,
    pub member: MemberHandle,
    pub container_id: String,
    pub item_index: u32,
    pub item_id: Option<String>,
    pub original_row_index: u32,
    pub timestamp_ms: Option<i64>,
    pub timing: &'static str,
    pub title: TextPreview,
    pub detail: TextPreview,
    pub source: TextPreview,
    pub lane_key: String,
    pub lane_label: TextPreview,
    pub grouping_key: String,
    pub grouping_uses_title: bool,
    pub event_ref: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AliasResolution {
    pub alias: String,
    pub state: &'static str,
    pub entry_id: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelinePage {
    pub kind: &'static str,
    pub owner: EvidenceOwner,
    pub evidence_signature: String,
    pub authored_view_signature: String,
    /// Complete Case items[].rows count before station/filter/range selection.
    pub preserved_count: u32,
    /// The remaining summaries cover the complete matched scope, not a page.
    pub scope_count: u32,
    pub timed_count: u32,
    pub untimed_count: u32,
    pub unavailable_count: u32,
    pub min_timestamp_ms: Option<i64>,
    pub max_timestamp_ms: Option<i64>,
    pub entries: Vec<TimelineEntry>,
    pub next_cursor: Option<String>,
    pub aliases: Vec<AliasResolution>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey(u8, i64, u32, u32);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineCursor {
    binding: String,
    after: SortKey,
}
fn timeline_binding(request: &TimelineRequest) -> Result<String, String> {
    binding(&(
        &request.store,
        &request.owner,
        &request.evidence_signature,
        &request.authored_view_json,
        &request.station_id,
        &request.filters,
        request.from_ms,
        request.to_ms,
        request.include_untimed,
    ))
}
pub(crate) fn entry_id(member: &MemberHandle) -> String {
    // Membership versions change manifest IDs; an unchanged occurrence keeps
    // its authored annotations when another occurrence is added or removed.
    format!("n:{}:{}", member.container_id, member.occurrence_id)
}
pub(crate) fn safe_timestamp(timestamp: Option<i64>) -> Option<i64> {
    timestamp.filter(|value| (-MAX_TIMESTAMP..=MAX_TIMESTAMP).contains(value))
}
fn top<'a>(raw: RawJson<'a>, key: &str) -> Result<Option<RawJson<'a>>, String> {
    if raw.kind() != b'{' {
        return Ok(None);
    }
    let mut result = None;
    let mut members = raw.members()?;
    while let Some(member) = members.next()? {
        if member.key()? == key {
            if result.is_some() {
                return Err("CASE_HISTORY_ALIAS_UNAVAILABLE".into());
            }
            result = Some(member.value);
        }
    }
    Ok(result)
}
pub(crate) fn legacy_suffix(envelope: &str, original_row_index: u32) -> Result<String, String> {
    let raw = RawJson::checked(envelope)?;
    if let Some(reference) = top(raw, "event_ref")? {
        if reference.kind() == b'"' {
            let value: String = serde_json::from_str(reference.get()).map_err(|_| INVALID)?;
            if !value.is_empty() {
                return Ok(value);
            }
        } else if !matches!(reference.kind(), b'n' | b'f') && reference.get() != "0" {
            // Nonstring identities were never admitted as a native Event and
            // cannot establish a unique old JavaScript alias by coercion.
            return Err("CASE_HISTORY_ALIAS_UNAVAILABLE".into());
        }
    }
    if let Some(id) = top(raw, "id")? {
        if id.kind() == b'"' {
            return serde_json::from_str(id.get()).map_err(|_| INVALID.into());
        }
        if id.kind() != b'n' {
            if let Ok(id) = id.get().parse::<u64>() {
                return Ok(id.to_string());
            }
            return Err("CASE_HISTORY_ALIAS_UNAVAILABLE".into());
        }
    }
    Ok(original_row_index.to_string())
}
pub(crate) fn alias_digest(item_id: &str, suffix: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"e:");
    digest.update(item_id.as_bytes());
    digest.update(b":");
    digest.update(suffix.as_bytes());
    digest.finalize().into()
}
pub(crate) fn unavailable_interpretation(error: &str) -> bool {
    [
        "CASE_EVIDENCE_MATERIALIZATION_LIMIT",
        "CASE_EVIDENCE_UNSUPPORTED_RECORD",
        "CASE_EVIDENCE_DUPLICATE_KEY",
        "CASE_EVIDENCE_JSON_LIMIT",
    ]
    .iter()
    .any(|prefix| error.starts_with(prefix))
}
fn add_count(count: &mut u32) -> Result<(), String> {
    *count = count.checked_add(1).ok_or(LIMIT)?;
    Ok(())
}

pub(crate) enum HistoricalFilter {
    Ordinary(crate::query::PreparedFilter),
    Query(crate::querylang::Expr),
}
impl HistoricalFilter {
    pub(crate) fn matches(&self, event: &Event) -> bool {
        match self {
            Self::Ordinary(filter) => crate::query::matches(event, filter),
            Self::Query(expr) => expr.matches(event),
        }
    }
}
pub(crate) struct PreparedHistoryFilters {
    pub(crate) filters: Vec<HistoricalFilter>,
    _base: Lease,
    _programs: std::rc::Rc<std::cell::RefCell<Vec<Lease>>>,
}
fn compile_admitted<T>(
    credits: &std::rc::Rc<std::cell::RefCell<Vec<Lease>>>,
    run: impl FnOnce() -> T,
) -> Result<T, String> {
    let credits = credits.clone();
    crate::query_regex::with_compilation_admission(
        move |bytes| {
            credits.borrow_mut().push(reserve(bytes)?);
            Ok(())
        },
        run,
    )
}
pub(crate) fn prepare_filters(
    filters: &[crate::query::Filter],
) -> Result<PreparedHistoryFilters, String> {
    if filters.len() > 200 {
        return Err(LIMIT.into());
    }
    encoded_len(&filters, case_evidence::REQUEST_BYTES)?;
    let mut bytes: usize = 64 << 10;
    for filter in filters {
        if filter.column.len() > 1024
            || filter.op.len() > 32
            || filter.value.len() > 1_000_000
            || filter
                .value2
                .as_ref()
                .is_some_and(|value| value.len() > 1_000_000)
        {
            return Err(LIMIT.into());
        }
        if matches!(filter.op.as_str(), "threat_rule" | "detection") {
            return Err("CASE_HISTORY_FILTER_CATALOG_UNAVAILABLE".into());
        }
        // Before chars/AST construction: input copies, Unicode casing, node
        // vectors and bounded parser scratch. Regex programs are admitted by
        // the scoped compiler hook individually, including cache hits.
        let needed = if filter.op == "query" {
            crate::querylang::historical_ast_credit(&filter.value)?
        } else {
            let text = filter
                .value
                .len()
                .checked_add(filter.value2.as_ref().map_or(0, String::len))
                .ok_or(LIMIT)?;
            let nodes = match filter.op.as_str() {
                "in" | "not_in" | "cidr" | "not_cidr" => crate::query::list_values(&filter.value)
                    .flat_map(str::split_whitespace)
                    .count(),
                "in_exact" => filter
                    .value
                    .lines()
                    .filter(|value| !value.is_empty())
                    .count(),
                _ => 0,
            };
            text.checked_mul(8)
                .and_then(|n| {
                    nodes
                        .checked_mul(256)
                        .and_then(|nodes| n.checked_add(nodes))
                })
                .and_then(|n| n.checked_add(4096))
                .ok_or(LIMIT)?
        };
        bytes = bytes.checked_add(needed).ok_or(LIMIT)?;
    }
    let base = reserve(bytes)?;
    let programs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut prepared = Vec::with_capacity(filters.len());
    for filter in filters {
        crate::operations::check()?;
        if filter.op == "query" {
            let expr = compile_admitted(&programs, || {
                crate::querylang::compile_without_catalogs(&filter.value)
            })??;
            prepared.push(HistoricalFilter::Query(expr));
        } else {
            // Validation can compile a regex too; its temporary credits drop
            // before the execution preparation. Neither swallowed .ok() nor a
            // valid empty result can conceal a latched admission failure.
            {
                let validation = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                compile_admitted(&validation, || {
                    crate::workspace::validate(std::slice::from_ref(filter))
                })??;
            }
            let mut one = compile_admitted(&programs, || {
                crate::query::prepare(std::slice::from_ref(filter))
            })?;
            prepared.push(HistoricalFilter::Ordinary(one.pop().ok_or(INVALID)?));
        }
    }
    Ok(PreparedHistoryFilters {
        filters: prepared,
        _base: base,
        _programs: programs,
    })
}

const AUTHORED_VIEW_BYTES: usize = 1 << 20;
pub(crate) struct AuthoredView {
    pub(crate) signature: String,
    pub(crate) titles: BTreeMap<String, String>,
    _credit: Lease,
}
pub(crate) fn authored_view(
    authority: &impl HistoryAuthority,
    request: &TimelineRequest,
    count: u32,
) -> Result<AuthoredView, String> {
    const VIEW_INVALID: &str = "CASE_HISTORY_AUTHORED_VIEW_INVALID";
    if request.authored_view_json.len() > AUTHORED_VIEW_BYTES {
        return Err(LIMIT.into());
    }
    let raw = RawJson::checked(&request.authored_view_json)?;
    let plan = case_evidence::preflight_value(raw)?;
    let _decoded_credit = reserve(plan.materialization_credit)?;
    let document = case_evidence::materialize_value(raw, plan)?;
    let root = document.as_object().ok_or(VIEW_INVALID)?;
    if root.len() != 2 || !root.get("manual").is_some_and(Value::is_array) {
        return Err(VIEW_INVALID.into());
    }
    let config = root
        .get("timeline")
        .and_then(Value::as_object)
        .ok_or(VIEW_INVALID)?;
    let edits = match config.get("edits") {
        None => None,
        Some(value) => Some(value.as_object().ok_or(VIEW_INVALID)?),
    };
    if edits.is_some_and(|edits| edits.len() > 10_000) {
        return Err(LIMIT.into());
    }
    let bytes = request
        .authored_view_json
        .len()
        .checked_mul(4)
        .and_then(|n| n.checked_add(edits.map_or(0, |edits| edits.len()).checked_mul(256)?))
        .ok_or(LIMIT)?;
    let credit = reserve(bytes)?;
    let mut titles = BTreeMap::new();
    let mut legacy: BTreeMap<[u8; 32], (Option<&str>, u32, Option<String>)> = BTreeMap::new();
    for (id, edit) in edits.into_iter().flat_map(|edits| edits.iter()) {
        if id.len() > 4096 {
            return Err(LIMIT.into());
        }
        let Some(title) = edit.get("title").filter(|v| !v.is_null()) else {
            continue;
        };
        let title = title.as_str().ok_or(VIEW_INVALID)?;
        if title.is_empty() {
            continue;
        }
        if let Some(native) = id.strip_prefix("n:") {
            let (container, occurrence) = native.split_once(':').ok_or(VIEW_INVALID)?;
            uuid::Uuid::parse_str(container).map_err(|_| VIEW_INVALID)?;
            uuid::Uuid::parse_str(occurrence).map_err(|_| VIEW_INVALID)?;
            titles.insert(id.clone(), title.into());
        } else {
            let original = id.strip_prefix("a:").unwrap_or(id);
            if !original.starts_with("e:") || authority.blocked_alias(id).is_some() {
                continue;
            }
            let digest: [u8; 32] = Sha256::digest(original.as_bytes()).into();
            let entry = legacy.entry(digest).or_insert((Some(title), 0, None));
            // Two authored spellings with different titles must not silently
            // select one after migration or a membership change.
            if entry.0 != Some(title) {
                entry.0 = None;
            }
        }
    }
    if !legacy.is_empty() {
        let mut visited = 0u32;
        let _encoded = reserve(case_evidence::ENVELOPE_BYTES)?;
        let _scratch = reserve(2 * case_evidence::ENVELOPE_BYTES)?;
        authority.visit(&mut |item, member, envelope| {
            crate::operations::check()?;
            add_count(&mut visited)?;
            if visited > count {
                return Err(INVALID.into());
            }
            let suffix = legacy_suffix(envelope, item.original_row_index)?;
            let item_id = item
                .item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("undefined");
            if let Some((_, count, target)) = legacy.get_mut(&alias_digest(item_id, &suffix)) {
                *count = count.saturating_add(1);
                *target = if *count == 1 {
                    Some(entry_id(&member))
                } else {
                    None
                };
            }
            Ok(())
        })?;
        if visited != count || authority.validate(request)? != count {
            return Err(INVALID.into());
        }
        for (_, (title, count, target)) in legacy {
            if let (Some(title), 1, Some(target)) = (title, count, target) {
                titles.entry(target).or_insert_with(|| title.into());
            }
        }
    }
    let signature = format!(
        "{:x}",
        Sha256::digest(request.authored_view_json.as_bytes())
    );
    Ok(AuthoredView {
        signature,
        titles,
        _credit: credit,
    })
}

pub(crate) fn timeline(
    authority: &impl HistoryAuthority,
    request: &TimelineRequest,
) -> Result<Response<TimelinePage>, String> {
    encoded_len(request, case_evidence::REQUEST_BYTES)?;
    if request.store.store_id != request.owner.store_id
        || request.evidence_signature.len() != 64
        || request.owner.case_id.len() > 1024
        || request
            .station_id
            .as_ref()
            .is_some_and(|station| station.len() > 4096)
        || request.aliases.len() > 256
        || request.aliases.iter().any(|alias| alias.len() > 2048)
        || request
            .from_ms
            .is_some_and(|value| safe_timestamp(Some(value)).is_none())
        || request
            .to_ms
            .is_some_and(|value| safe_timestamp(Some(value)).is_none())
        || request
            .from_ms
            .zip(request.to_ms)
            .is_some_and(|(from, to)| from > to)
    {
        return Err(INVALID.into());
    }
    for id in [
        &request.store.store_id,
        &request.store.epoch,
        &request.owner.analysis_id,
    ] {
        uuid::Uuid::parse_str(id).map_err(|_| INVALID)?;
    }
    let preserved_count = authority.validate(request)?;
    let authored = authored_view(authority, request, preserved_count)?;
    let expected = timeline_binding(request)?;
    let after = if let Some(cursor) = &request.cursor {
        if cursor.len() > 512 {
            return Err(INVALID.into());
        }
        let cursor: TimelineCursor = serde_json::from_str(cursor).map_err(|_| INVALID)?;
        if cursor.binding != expected {
            return Err(INVALID.into());
        }
        Some(cursor.after)
    } else {
        None
    };
    let credit = reserve(RESPONSE_CREDIT)?;
    let _encoded = reserve(case_evidence::ENVELOPE_BYTES)?;
    // Covers one alias/string decode and temporary canonical text/filter work.
    let _scratch = reserve(2 * case_evidence::ENVELOPE_BYTES)?;
    let prepared = prepare_filters(&request.filters)?;
    let filters = &prepared.filters;
    let mut page = TimelinePage {
        kind: "preserved_timeline",
        owner: request.owner.clone(),
        evidence_signature: request.evidence_signature.clone(),
        authored_view_signature: authored.signature.clone(),
        preserved_count,
        scope_count: 0,
        timed_count: 0,
        untimed_count: 0,
        unavailable_count: 0,
        min_timestamp_ms: None,
        max_timestamp_ms: None,
        entries: Vec::new(),
        next_cursor: None,
        aliases: Vec::new(),
    };
    let mut aliases: BTreeMap<[u8; 32], (u32, Option<String>)> = BTreeMap::new();
    let mut frozen_aliases = std::collections::BTreeSet::new();
    for alias in &request.aliases {
        let original = alias.strip_prefix("a:").unwrap_or(alias);
        if !original.starts_with("e:") {
            return Err("CASE_HISTORY_ALIAS_KIND".into());
        }
        let digest: [u8; 32] = Sha256::digest(original.as_bytes()).into();
        if let Some(state) = authority.blocked_alias(alias) {
            let count = match state {
                crate::case_evidence_anchors::BlockedState::Missing => 0,
                crate::case_evidence_anchors::BlockedState::Ambiguous => 2,
            };
            frozen_aliases.insert(digest);
            aliases.insert(digest, (count, None));
        } else {
            aliases.entry(digest).or_insert((0, None));
        }
    }
    let dynamic_aliases = aliases.keys().any(|key| !frozen_aliases.contains(key));
    let mut selected: BTreeMap<SortKey, TimelineEntry> = BTreeMap::new();
    let mut candidates = 0u32;
    let mut visited = 0u32;
    let mut previous: Option<(u32, u32)> = None;
    authority.visit(&mut |item, member, envelope| {
        crate::operations::check()?;
        let ordinal = match previous {
            Some((index, ordinal)) if index == item.item_index => {
                ordinal.checked_add(1).ok_or(LIMIT)?
            }
            Some((index, _)) if index > item.item_index => return Err(INVALID.into()),
            _ => 0,
        };
        previous = Some((item.item_index, ordinal));
        add_count(&mut visited)?;
        if visited > preserved_count {
            return Err(INVALID.into());
        }
        let item_id = item.item.get("id").and_then(Value::as_str);
        if item_id.is_some_and(|id| id.len() > 1024) {
            return Err(LIMIT.into());
        }
        for id in [
            &member.container_id,
            &member.manifest_id,
            &member.occurrence_id,
        ] {
            uuid::Uuid::parse_str(id).map_err(|_| INVALID)?;
        }
        let stable_id = entry_id(&member);
        if dynamic_aliases {
            let suffix = legacy_suffix(envelope, item.original_row_index)?;
            let digest = alias_digest(item_id.unwrap_or("undefined"), &suffix);
            if let Some((count, resolved)) = aliases
                .get_mut(&digest)
                .filter(|_| !frozen_aliases.contains(&digest))
            {
                *count = count.saturating_add(1);
                *resolved = if *count == 1 {
                    Some(stable_id.clone())
                } else {
                    None
                };
            }
        }
        // Alias uniqueness is complete before any station, filter or page cut.
        if request.station_id.as_deref().is_some_and(|station| {
            item.item.get("stationId").and_then(Value::as_str) != Some(station)
        }) {
            return Ok(());
        }
        let decoded = match case_evidence::preflight_envelope(envelope) {
            Ok(plan) => {
                let row_credit = reserve(plan.materialization_credit)?;
                match case_evidence::materialize_event(envelope, plan) {
                    Ok(event) => Some((event, row_credit)),
                    Err(error) if unavailable_interpretation(&error) => None,
                    Err(error) => return Err(error),
                }
            }
            Err(error) if unavailable_interpretation(&error) => None,
            Err(error) => return Err(error),
        };
        let event = decoded.as_ref().map(|(event, _)| event);
        if !filters.is_empty() && event.is_none() {
            return Err(FILTER_UNAVAILABLE.into());
        }
        if event.is_some_and(|event| !filters.iter().all(|filter| filter.matches(event))) {
            return Ok(());
        }
        let timestamp = event.and_then(|event| safe_timestamp(event.timestamp));
        if let Some(timestamp) = timestamp {
            if request.from_ms.is_some_and(|from| timestamp < from)
                || request.to_ms.is_some_and(|to| timestamp > to)
            {
                return Ok(());
            }
        } else if !request.include_untimed {
            return Ok(());
        }
        add_count(&mut page.scope_count)?;
        let (rank, timing) = if event.is_none()
            || event.is_some_and(|event| event.timestamp.is_some() && timestamp.is_none())
        {
            add_count(&mut page.unavailable_count)?;
            (2, "unavailable")
        } else if let Some(timestamp) = timestamp {
            add_count(&mut page.timed_count)?;
            page.min_timestamp_ms = Some(
                page.min_timestamp_ms
                    .map_or(timestamp, |old| old.min(timestamp)),
            );
            page.max_timestamp_ms = Some(
                page.max_timestamp_ms
                    .map_or(timestamp, |old| old.max(timestamp)),
            );
            (0, "timed")
        } else {
            add_count(&mut page.untimed_count)?;
            (1, "untimed")
        };
        let key = SortKey(rank, timestamp.unwrap_or(0), item.item_index, ordinal);
        if after.as_ref().is_some_and(|after| key <= *after) {
            return Ok(());
        }
        add_count(&mut candidates)?;
        if selected.len() == PAGE_ROWS
            && selected
                .last_key_value()
                .is_some_and(|(last, _)| key >= *last)
        {
            return Ok(());
        }
        let title = authored
            .titles
            .get(&stable_id)
            .map(String::as_str)
            .or_else(|| {
                event.map(|event| {
                    [&event.name, &event.message, &event.code]
                        .into_iter()
                        .find(|text| !text.is_empty())
                        .map(String::as_str)
                        .unwrap_or("Evento")
                })
            })
            .unwrap_or("Registro preservado indisponível");
        let detail = event
            .map(|event| {
                if event.message.is_empty() {
                    event.description.as_str()
                } else {
                    event.message.as_str()
                }
            })
            .unwrap_or("");
        let source = item
            .item
            .get("label")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .or_else(|| {
                item.item
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
            })
            .unwrap_or("Item do caso");
        let group = event
            .map(|event| {
                if !event.code.is_empty() {
                    event.code.as_str()
                } else if !event.name.is_empty() {
                    event.name.as_str()
                } else {
                    title
                }
            })
            .unwrap_or(&stable_id);
        // Hashes use complete native text and container identity, never previews.
        let grouping_key = binding(&(&member.container_id, group))?;
        let entry = TimelineEntry {
            entry_id: stable_id,
            container_id: member.container_id.clone(),
            lane_key: member.container_id.clone(),
            member,
            item_index: item.item_index,
            item_id: item_id.map(str::to_string),
            original_row_index: item.original_row_index,
            timestamp_ms: timestamp,
            timing,
            title: preview(title, 1024),
            detail: preview(detail, 2048),
            source: preview(source, 512),
            lane_label: preview(source, 512),
            grouping_key,
            grouping_uses_title: event
                .is_some_and(|event| event.code.is_empty() && event.name.is_empty()),
            event_ref: event
                .filter(|event| !event.event_ref.is_empty() && event.event_ref.len() <= 1024)
                .map(|event| event.event_ref.clone()),
        };
        selected.insert(key, entry);
        if selected.len() > PAGE_ROWS {
            selected.pop_last();
        }
        Ok(())
    })?;
    if visited != preserved_count || authority.validate(request)? != preserved_count {
        return Err(INVALID.into());
    }
    if candidates as usize > selected.len() {
        let after = selected.last_key_value().ok_or(INVALID)?.0.clone();
        page.next_cursor = Some(
            serde_json::to_string(&TimelineCursor {
                binding: expected,
                after,
            })
            .map_err(|e| e.to_string())?,
        );
    }
    page.entries = selected.into_values().collect();
    for alias in &request.aliases {
        let original = alias.strip_prefix("a:").unwrap_or(alias);
        let digest: [u8; 32] = Sha256::digest(original.as_bytes()).into();
        let (count, resolved) = aliases.get(&digest).ok_or(INVALID)?;
        page.aliases.push(AliasResolution {
            alias: alias.clone(),
            state: match count {
                0 => "missing",
                1 => "unique",
                _ => "ambiguous",
            },
            entry_id: resolved.clone(),
        });
    }
    crate::operations::check()?;
    finish(page, credit)
}

// Concrete storage adapters. Opening these leases is the command's admission
// step; the pure readers above repeat their live authority/file validation.
impl MemberAuthority for case_evidence::ReferenceAuthorityLease {
    fn validate(&self, request: &MemberRequest) -> Result<(), String> {
        if self.store() != &request.store || self.reference() != &request.reference {
            return Err(INVALID.into());
        }
        case_evidence::ReferenceAuthorityLease::validate(self)
    }
    fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
        case_evidence::ReferenceAuthorityLease::envelope(self, member)
    }
}
impl PreviewAuthority for case_evidence::ReferenceAuthorityLease {
    fn validate_preview(&self, request: &case_evidence::PreviewRequest) -> Result<usize, String> {
        if self.store() != &request.store || self.reference() != &request.reference {
            return Err(INVALID.into());
        }
        self.validate()?;
        Ok(self.member_count())
    }
    fn member_at(&self, index: usize) -> Result<MemberHandle, String> {
        case_evidence::ReferenceAuthorityLease::member_at(self, index)
    }
}
impl HistoryAuthority for case_evidence::CaseAuthorityLease {
    fn blocked_alias(&self, alias: &str) -> Option<crate::case_evidence_anchors::BlockedState> {
        self.anchors().blocked_event(alias).map(|entry| entry.state)
    }
    fn validate(&self, request: &TimelineRequest) -> Result<u32, String> {
        if self.store() != &request.store || self.case().owner() != &request.owner {
            return Err(INVALID.into());
        }
        case_evidence::CaseAuthorityLease::validate(self)?;
        let case_evidence::CaseEvidenceState::Ready(state) = self.case().evidence_state()? else {
            return Err(INVALID.into());
        };
        if state.evidence_signature != request.evidence_signature {
            return Err(INVALID.into());
        }
        Ok(state.preserved_count)
    }
    fn visit(
        &self,
        visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        let items = self
            .case()
            .metadata()
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for (location, container) in self.case().containers() {
            let case_evidence::ContainerLocation::ItemRows { index } = location else {
                continue;
            };
            let item = items.get(*index as usize).ok_or(INVALID)?;
            container.visit_envelopes_with_origin(
                0..container.reference().member_count as usize,
                |member, original_row_index, envelope| {
                    visitor(
                        HistoricalItem {
                            item_index: *index,
                            original_row_index,
                            item,
                        },
                        member,
                        envelope,
                    )?;
                    Ok(case_evidence::Visit::Continue)
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn owner() -> EvidenceOwner {
        EvidenceOwner {
            store_id: uuid(),
            case_id: "case".into(),
            analysis_id: uuid(),
        }
    }
    fn member() -> MemberHandle {
        MemberHandle {
            container_id: uuid(),
            manifest_id: uuid(),
            occurrence_id: uuid(),
        }
    }
    fn event(id: usize, timestamp: Option<i64>) -> Event {
        let mut event = Event::empty();
        event.id = id;
        event.timestamp = timestamp;
        event.event_ref = format!("event:{id}");
        event.name = format!("name {id}");
        event.message = "preserved message".into();
        event
    }
    struct MemberFixture {
        request: MemberRequest,
        envelope: String,
        validations: Cell<usize>,
        reads: Cell<usize>,
        invalidate_final: bool,
    }
    impl MemberFixture {
        fn new(event: Event) -> Self {
            let owner = owner();
            let member = member();
            let reference = EvidenceReference::Committed(case_evidence::EvidenceRef {
                kind: case_evidence::CommittedKind::NativeEvidence,
                schema_version: 1,
                owner: owner.clone(),
                container_id: member.container_id.clone(),
                manifest_id: member.manifest_id.clone(),
                manifest_sha256: "a".repeat(64),
                member_count: 1,
            });
            Self {
                request: MemberRequest {
                    store: StoreIdentity {
                        store_id: owner.store_id.clone(),
                        epoch: uuid(),
                    },
                    reference,
                    member,
                },
                envelope: serde_json::to_string(&event).unwrap(),
                validations: Cell::new(0),
                reads: Cell::new(0),
                invalidate_final: false,
            }
        }
    }
    impl MemberAuthority for MemberFixture {
        fn validate(&self, request: &MemberRequest) -> Result<(), String> {
            let count = self.validations.get();
            self.validations.set(count + 1);
            if request.store != self.request.store
                || request.reference != self.request.reference
                || request.member != self.request.member
                || (self.invalidate_final && count > 0)
            {
                return Err(INVALID.into());
            }
            Ok(())
        }
        fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
            self.reads.set(self.reads.get() + 1);
            if member != &self.request.member {
                return Err(INVALID.into());
            }
            Ok(self.envelope.clone())
        }
    }
    #[test]
    fn exact_fields_keep_native_numbers_null_missing_and_timestamp_zero() {
        let mut source = event(17, Some(0));
        source
            .fields
            .insert("wide".into(), serde_json::json!(u64::MAX));
        source.fields.insert("float".into(), serde_json::json!(1.0));
        source.fields.insert("null".into(), Value::Null);
        source
            .fields
            .insert("object".into(), serde_json::json!({"10":1.0,"2":u64::MAX}));
        let fixture = MemberFixture::new(source);
        for (column, kind, expected) in [
            ("wide", "number", "18446744073709551615"),
            ("float", "number", "1.0"),
            ("null", "null", "null"),
            (
                "object",
                "object",
                "{\"10\":1.0,\"2\":18446744073709551615}",
            ),
        ] {
            let value = field_text(&fixture, &fixture.request, column).unwrap();
            assert!(value.present && value.complete);
            assert_eq!(value.value_type, Some(kind));
            assert_eq!(value.text.as_deref(), Some(expected));
            assert_eq!(value.member, fixture.request.member);
        }
        let zero = field_text(&fixture, &fixture.request, "timestamp").unwrap();
        assert!(zero.present);
        assert_eq!(zero.value_type, Some("number"));
        assert_eq!(zero.text.as_deref(), Some("1970-01-01T00:00:00+00:00"));
        let missing = field_text(&fixture, &fixture.request, "missing").unwrap();
        assert!(!missing.present);
        assert_eq!(missing.value_type, None);
        assert_eq!(missing.text, None);
        let absent = MemberFixture::new(event(1, None));
        let missing = field_text(&absent, &absent.request, "timestamp").unwrap();
        assert!(!missing.present);
        assert_eq!(missing.value_type, None);
        assert_eq!(missing.text, None);
    }
    #[test]
    fn exact_actions_refuse_escaped_response_overflow_and_stale_authority() {
        let mut source = event(0, None);
        source.fields.insert(
            "escaped".into(),
            serde_json::json!({"text":"\\".repeat(270_000)}),
        );
        let fixture = MemberFixture::new(source);
        assert!(field_text(&fixture, &fixture.request, "escaped").is_err());
        let mut changed = MemberFixture::new(event(0, None));
        changed.invalidate_final = true;
        assert!(field_text(&changed, &changed.request, "message").is_err());
        assert_eq!(changed.reads.get(), 1);
        assert_eq!(changed.validations.get(), 2);
        let mut wrong = changed.request.clone();
        wrong.member.occurrence_id = uuid();
        assert!(java(&changed, &wrong).is_err());
        assert_eq!(changed.reads.get(), 1);
    }
    #[test]
    fn detail_pages_are_bounded_and_cursors_bind_the_exact_member() {
        let mut source = event(7, Some(1));
        source.raw = "日".repeat(95_000);
        for index in 0..160 {
            source.fields.insert(
                format!("field{index:03}"),
                Value::String("x".repeat(if index == 0 { 5_000 } else { 1_000 })),
            );
        }
        let fixture = MemberFixture::new(source);
        let first = detail(&fixture, &fixture.request, None).unwrap();
        assert!(first.fields.len() <= 128);
        assert_eq!(first.fields_total, 169);
        assert!(!first.raw.complete);
        assert!(first
            .fields
            .iter()
            .all(|field| encoded_len(&field.text, CELL_BYTES).is_ok()));
        assert!(first.fields.iter().any(|field| !field.complete));
        assert!(encoded_len(&first.value, RESPONSE_BYTES).is_ok());
        let next = first.next_cursor.clone().unwrap();
        let second = detail(&fixture, &fixture.request, Some(&next)).unwrap();
        assert!(second.next_cursor.is_none());
        assert_eq!(first.fields.len() + second.fields.len(), 169);
        let another = MemberFixture::new(event(7, Some(1)));
        assert!(detail(&another, &another.request, Some(&next)).is_err());
        let wire = serde_json::to_value(&first).unwrap();
        assert_eq!(wire["kind"], "preserved_member_detail");
        assert!(wire.get("event").is_none());
        assert!(wire.get("id").is_none());
    }
    #[test]
    fn native_java_returns_structured_trace_and_original_member() {
        let mut source = event(2, None);
        source.raw = "2026-07-08 00:00:00,000 WARN  [br.app.Classe] (thread) java.lang.IllegalStateException: broken\n\tat br.app.Classe.run(Classe.java:10)".into();
        let fixture = MemberFixture::new(source);
        let trace = java(&fixture, &fixture.request).unwrap();
        assert_eq!(trace.state, "available");
        assert!(trace.trace.is_some());
        assert_eq!(trace.member, fixture.request.member);
        let empty = MemberFixture::new(event(2, None));
        let unavailable = java(&empty, &empty.request).unwrap();
        assert_eq!(unavailable.state, "unavailable");
        assert_eq!(unavailable.reason, Some("raw_unavailable"));
    }
    struct HistoryFixture {
        request: TimelineRequest,
        rows: Vec<(Value, u32, u32, MemberHandle, String)>,
        invalid_final: bool,
        validations: Cell<usize>,
    }
    impl HistoryFixture {
        fn new() -> Self {
            let owner = owner();
            Self {
                request: TimelineRequest {
                    store: StoreIdentity {
                        store_id: owner.store_id.clone(),
                        epoch: uuid(),
                    },
                    owner,
                    evidence_signature: "a".repeat(64),
                    authored_view_json: "{\"timeline\":{},\"manual\":[]}".into(),
                    station_id: None,
                    filters: Vec::new(),
                    from_ms: None,
                    to_ms: None,
                    include_untimed: true,
                    cursor: None,
                    aliases: Vec::new(),
                },
                rows: Vec::new(),
                invalid_final: false,
                validations: Cell::new(0),
            }
        }
        fn push(&mut self, item: Value, item_index: u32, index: u32, event: Event) {
            let mut handle = member();
            if let Some((_, _, _, previous, _)) =
                self.rows.iter().find(|(_, i, _, _, _)| *i == item_index)
            {
                handle.container_id = previous.container_id.clone();
                handle.manifest_id = previous.manifest_id.clone();
            }
            self.rows.push((
                item,
                item_index,
                index,
                handle,
                serde_json::to_string(&event).unwrap(),
            ));
        }
    }
    impl HistoryAuthority for HistoryFixture {
        fn validate(&self, request: &TimelineRequest) -> Result<u32, String> {
            let count = self.validations.get();
            self.validations.set(count + 1);
            if request.store != self.request.store
                || request.owner != self.request.owner
                || request.evidence_signature != self.request.evidence_signature
                || (self.invalid_final && count > 0)
            {
                return Err(INVALID.into());
            }
            Ok(self.rows.len() as u32)
        }
        fn visit(
            &self,
            visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
        ) -> Result<(), String> {
            for (item, item_index, index, member, envelope) in &self.rows {
                visitor(
                    HistoricalItem {
                        item_index: *item_index,
                        original_row_index: *index,
                        item,
                    },
                    member.clone(),
                    envelope,
                )?;
            }
            Ok(())
        }
    }
    #[test]
    fn timeline_aliases_are_unique_across_off_page_and_other_station_occurrences() {
        let mut fixture = HistoryFixture::new();
        for index in 0..130 {
            fixture.push(
                serde_json::json!({"id":"same","stationId":"A","label":"Item A"}),
                0,
                index,
                event(index as usize, Some(1000 + index as i64)),
            );
        }
        let mut duplicate = event(0, Some(0));
        duplicate.event_ref = "event:0".into();
        fixture.push(
            serde_json::json!({"id":"same","stationId":"B","label":"Item B"}),
            1,
            0,
            duplicate,
        );
        fixture.request.station_id = Some("A".into());
        fixture.request.aliases = vec![
            "e:same:event:0".into(),
            "a:e:same:event:129".into(),
            "e:same:missing".into(),
        ];
        let first = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(first.preserved_count, 131);
        assert_eq!(first.scope_count, 130);
        assert_eq!(first.timed_count, 130);
        assert_eq!(first.entries.len(), 128);
        assert_eq!(first.min_timestamp_ms, Some(1000));
        assert_eq!(first.max_timestamp_ms, Some(1129));
        assert_eq!(first.aliases[0].state, "ambiguous");
        assert_eq!(first.aliases[0].entry_id, None);
        assert_eq!(first.aliases[1].state, "unique");
        assert_eq!(
            first.aliases[1].entry_id.as_deref(),
            Some(entry_id(&fixture.rows[129].3).as_str())
        );
        assert_eq!(first.aliases[2].state, "missing");
        let mut request = fixture.request.clone();
        request.cursor = first.next_cursor.clone();
        let second = timeline(&fixture, &request).unwrap();
        assert_eq!(second.entries.len(), 2);
        assert_eq!(second.entries[0].original_row_index, 128);
        assert_eq!(second.scope_count, 130);
        assert!(second.next_cursor.is_none());
        request.from_ms = Some(1100);
        assert!(timeline(&fixture, &request).is_err());
    }
    #[test]
    fn timeline_keeps_counts_sort_and_native_keys_independent_of_short_labels() {
        let mut fixture = HistoryFixture::new();
        let long = "same prefix".repeat(300);
        for (index, timestamp) in [Some(9), Some(2), Some(2), None].into_iter().enumerate() {
            let mut source = event(index, timestamp);
            source.name = format!("{long}{index}");
            fixture.push(
                serde_json::json!({"id":"same","label":"same label"}),
                0,
                index as u32,
                source,
            );
        }
        fixture.rows.push((
            serde_json::json!({"id":"same"}),
            1,
            0,
            member(),
            "{\"id\":999,\"unsupported\":true}".into(),
        ));
        let result = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(
            (
                result.scope_count,
                result.timed_count,
                result.untimed_count,
                result.unavailable_count
            ),
            (5, 3, 1, 1)
        );
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.original_row_index)
                .collect::<Vec<_>>(),
            [1, 2, 0, 3, 0]
        );
        assert_eq!(result.entries[0].title.text, result.entries[1].title.text);
        assert_ne!(
            result.entries[0].grouping_key,
            result.entries[1].grouping_key
        );
        assert!(!result.entries[0].title.complete);
        assert!(!result.entries[0].grouping_uses_title);
        assert_eq!(result.entries[4].timing, "unavailable");
        assert_eq!(result.entries[4].timestamp_ms, None);
        let old = result.entries[0].entry_id.clone();
        fixture.rows[1].3.manifest_id = uuid();
        let changed_manifest = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(changed_manifest.entries[0].entry_id, old);
        let mut fallback = event(1, Some(2));
        fallback.name.clear();
        fallback.code.clear();
        fixture.rows[1].4 = serde_json::to_string(&fallback).unwrap();
        let fallback = timeline(&fixture, &fixture.request).unwrap();
        assert!(fallback.entries[0].grouping_uses_title);
        assert!(!fallback.entries[4].grouping_uses_title);
        let mut code = event(1, Some(2));
        code.name.clear();
        code.code = "native-code".into();
        fixture.rows[1].4 = serde_json::to_string(&code).unwrap();
        let code = timeline(&fixture, &fixture.request).unwrap();
        assert!(!code.entries[0].grouping_uses_title);
    }
    #[test]
    fn timeline_reuses_native_filters_and_refuses_uninterpretable_or_unsupported_clauses() {
        let mut fixture = HistoryFixture::new();
        let mut source = event(1, Some(100));
        source.message = "UpperCASE".into();
        fixture.push(serde_json::json!({"id":"i"}), 0, 0, source);
        fixture.request.filters.push(crate::query::Filter {
            column: "message".into(),
            op: "equals".into(),
            value: "uppercase".into(),
            value2: None,
        });
        assert_eq!(timeline(&fixture, &fixture.request).unwrap().scope_count, 1);
        fixture.request.filters[0].op = "equals_exact".into();
        assert_eq!(timeline(&fixture, &fixture.request).unwrap().scope_count, 0);
        fixture.request.filters[0].op = "query".into();
        assert_eq!(timeline(&fixture, &fixture.request).unwrap().scope_count, 1);
        fixture.request.filters[0].value = "id>0 AND message:uppercase".into();
        assert_eq!(timeline(&fixture, &fixture.request).unwrap().scope_count, 1);
        fixture.request.filters[0].value = "rule:must-not-load".into();
        assert!(timeline(&fixture, &fixture.request)
            .err()
            .unwrap()
            .contains("CATALOG_UNAVAILABLE"));
        fixture.request.filters[0].value = "uppercase".into();
        fixture.request.filters[0].op = "equals".into();
        fixture.rows[0].4 = "{\"unsupported\":true}".into();
        assert!(timeline(&fixture, &fixture.request)
            .err()
            .unwrap()
            .contains("FILTER_UNAVAILABLE"));
        fixture.request.filters.clear();
        fixture.request.include_untimed = false;
        assert_eq!(timeline(&fixture, &fixture.request).unwrap().scope_count, 0);
    }
    #[test]
    fn installed_two_hundred_short_filters_fit_the_bounded_native_contract() {
        let filters = (0..200)
            .map(|_| crate::query::Filter {
                column: "message".into(),
                op: "contains".into(),
                value: "x".into(),
                value2: None,
            })
            .collect::<Vec<_>>();
        let prepared = prepare_filters(&filters).unwrap();
        assert_eq!(prepared.filters.len(), 200);
        let mut too_many = filters;
        too_many.push(too_many[0].clone());
        assert!(prepare_filters(&too_many).is_err());
    }
    #[test]
    fn empty_history_is_authoritative_and_invalid_time_is_explicitly_unavailable() {
        let mut fixture = HistoryFixture::new();
        let empty = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(empty.preserved_count, 0);
        assert_eq!(empty.scope_count, 0);
        assert!(empty.entries.is_empty());
        fixture.push(
            serde_json::json!({"id":"i"}),
            0,
            0,
            event(0, Some(i64::MAX)),
        );
        let invalid_time = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(invalid_time.scope_count, 1);
        assert_eq!(invalid_time.unavailable_count, 1);
        assert_eq!(invalid_time.entries[0].timing, "unavailable");
        assert_eq!(invalid_time.entries[0].timestamp_ms, None);
    }
    #[test]
    fn final_authority_change_and_incomplete_scope_never_publish_partial_history() {
        let mut fixture = HistoryFixture::new();
        fixture.push(serde_json::json!({"id":"i"}), 0, 0, event(1, Some(100)));
        fixture.invalid_final = true;
        assert!(timeline(&fixture, &fixture.request).is_err());
        struct Partial<'a>(&'a HistoryFixture);
        impl HistoryAuthority for Partial<'_> {
            fn validate(&self, _: &TimelineRequest) -> Result<u32, String> {
                Ok(2)
            }
            fn visit(
                &self,
                visitor: &mut dyn FnMut(
                    HistoricalItem<'_>,
                    MemberHandle,
                    &str,
                ) -> Result<(), String>,
            ) -> Result<(), String> {
                self.0.visit(visitor)
            }
        }
        assert!(timeline(&Partial(&fixture), &fixture.request).is_err());
    }
    #[test]
    fn authored_alias_batches_preserve_every_anchor_with_explicit_full_scan_count() {
        struct Counted<'a> {
            inner: &'a HistoryFixture,
            scans: Cell<usize>,
            visits: Cell<usize>,
        }
        impl HistoryAuthority for Counted<'_> {
            fn validate(&self, r: &TimelineRequest) -> Result<u32, String> {
                self.inner.validate(r)
            }
            fn visit(
                &self,
                v: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
            ) -> Result<(), String> {
                self.scans.set(self.scans.get() + 1);
                self.inner.visit(&mut |i, m, e| {
                    self.visits.set(self.visits.get() + 1);
                    v(i, m, e)
                })
            }
        }
        let mut fixture = HistoryFixture::new();
        for index in 0..520 {
            let mut event = Event::empty();
            event.id = index;
            event.event_ref = format!("record:{index}");
            event.timestamp = Some(index as i64);
            fixture.push(
                serde_json::json!({"id":"item:with:colons"}),
                0,
                index as u32,
                event,
            );
        }
        let counted = Counted {
            inner: &fixture,
            scans: Cell::new(0),
            visits: Cell::new(0),
        };
        let authored: Vec<_> = (0..520)
            .map(|i| format!("e:item:with:colons:record:{i}"))
            .collect();
        let mut resolved = Vec::new();
        let before = case_work_budget::global().used();
        for batch in authored.chunks(256) {
            let mut request = fixture.request.clone();
            request.aliases = batch.to_vec();
            let result = timeline(&counted, &request).unwrap();
            assert_eq!(result.preserved_count, 520);
            assert_eq!(result.scope_count, 520);
            assert_eq!(result.aliases.len(), batch.len());
            for (alias, expected) in result.aliases.iter().zip(batch) {
                assert_eq!(&alias.alias, expected);
                assert_eq!(alias.state, "unique");
                assert!(alias.entry_id.is_some());
                resolved.push(alias.alias.clone());
            }
        }
        assert_eq!(resolved, authored);
        assert_eq!(counted.scans.get(), 3);
        assert_eq!(counted.visits.get(), 1_560);
        assert_eq!(case_work_budget::global().used(), before);
    }
    #[test]
    fn frozen_original_ambiguity_and_missing_override_later_unique_membership() {
        struct Frozen<'a> {
            inner: &'a HistoryFixture,
            anchors: crate::case_evidence_anchors::AnchorMap,
        }
        impl HistoryAuthority for Frozen<'_> {
            fn blocked_alias(
                &self,
                alias: &str,
            ) -> Option<crate::case_evidence_anchors::BlockedState> {
                self.anchors.blocked_event(alias).map(|e| e.state)
            }
            fn validate(&self, r: &TimelineRequest) -> Result<u32, String> {
                self.inner.validate(r)
            }
            fn visit(
                &self,
                v: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
            ) -> Result<(), String> {
                self.inner.visit(v)
            }
        }
        use crate::case_evidence_anchors::{AnchorMap, BlockedAlias, BlockedState};
        let mut fixture = HistoryFixture::new();
        let mut event = Event::empty();
        event.id = 0;
        event.event_ref = "same".into();
        fixture.push(serde_json::json!({"id":"item"}), 0, 0, event);
        fixture.request.aliases = vec![
            "e:item:same".into(),
            "a:e:item:same".into(),
            "e:item:removed".into(),
        ];
        let authority = Frozen {
            inner: &fixture,
            anchors: AnchorMap {
                schema_version: 1,
                event_aliases: vec![
                    BlockedAlias {
                        alias: "e:item:removed".into(),
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
            },
        };
        let page = timeline(&authority, &fixture.request).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.aliases.iter().map(|a| a.state).collect::<Vec<_>>(),
            ["ambiguous", "ambiguous", "missing"]
        );
        assert!(page.aliases.iter().all(|a| a.entry_id.is_none()));
    }
    struct PreviewFixture {
        inner: MemberFixture,
        members: Vec<MemberHandle>,
        request: case_evidence::PreviewRequest,
        checks: Cell<usize>,
        stale_at_exit: bool,
    }
    impl PreviewFixture {
        fn new(event: Event, count: usize) -> Self {
            let mut inner = MemberFixture::new(event);
            if let EvidenceReference::Committed(reference) = &mut inner.request.reference {
                reference.member_count = count as u32;
            }
            let members = (0..count)
                .map(|_| MemberHandle {
                    occurrence_id: uuid(),
                    ..inner.request.member.clone()
                })
                .collect();
            let request = case_evidence::PreviewRequest {
                store: inner.request.store.clone(),
                reference: inner.request.reference.clone(),
                cursor: None,
                limit: 16,
                columns: vec![
                    "wide".into(),
                    "float".into(),
                    "null".into(),
                    "missing".into(),
                    "message".into(),
                ],
            };
            Self {
                inner,
                members,
                request,
                checks: Cell::new(0),
                stale_at_exit: false,
            }
        }
    }
    impl MemberAuthority for PreviewFixture {
        fn validate(&self, request: &MemberRequest) -> Result<(), String> {
            if request.store != self.request.store
                || request.reference != self.request.reference
                || !self.members.iter().any(|member| *member == request.member)
            {
                return Err(INVALID.into());
            }
            Ok(())
        }
        fn envelope(&self, member: &MemberHandle) -> Result<String, String> {
            if !self.members.contains(member) {
                return Err(INVALID.into());
            }
            self.inner.reads.set(self.inner.reads.get() + 1);
            Ok(self.inner.envelope.clone())
        }
    }
    impl PreviewAuthority for PreviewFixture {
        fn validate_preview(
            &self,
            request: &case_evidence::PreviewRequest,
        ) -> Result<usize, String> {
            let check = self.checks.get();
            self.checks.set(check + 1);
            if request.store != self.request.store
                || request.reference != self.request.reference
                || self.stale_at_exit && check > 0
            {
                return Err(INVALID.into());
            }
            Ok(self.members.len())
        }
        fn member_at(&self, index: usize) -> Result<MemberHandle, String> {
            self.members
                .get(index)
                .cloned()
                .ok_or_else(|| INVALID.into())
        }
    }
    #[test]
    fn native_preview_keeps_exact_text_and_bounded_ordered_occurrence_pages() {
        let before = case_work_budget::global().used();
        let mut source = event(1234, None);
        source
            .fields
            .insert("wide".into(), serde_json::json!(u64::MAX));
        source
            .fields
            .insert("float".into(), serde_json::json!(-0.0));
        source.fields.insert("null".into(), Value::Null);
        source.message = "\n\"".repeat(4000);
        let fixture = PreviewFixture::new(source, 17);
        let page = preview_page(&fixture, &fixture.request).unwrap();
        assert_eq!(page.total, 17);
        assert_eq!(page.rows.len(), 16);
        assert_eq!(fixture.inner.reads.get(), 16);
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["rows"][0]["cells"][0]["text"], u64::MAX.to_string());
        assert_eq!(json["rows"][0]["cells"][1]["text"], "-0.0");
        assert_eq!(json["rows"][0]["cells"][2]["state"], "null");
        assert_eq!(json["rows"][0]["cells"][3]["state"], "missing");
        assert_eq!(json["rows"][0]["cells"][4]["incomplete"], true);
        for row in json["rows"].as_array().unwrap() {
            for cell in row["cells"].as_array().unwrap() {
                assert!(encoded_len(cell, CELL_BYTES).is_ok());
            }
        }
        assert!(case_work_budget::global().used() > before);
        let mut next = fixture.request.clone();
        next.cursor = page.next_cursor.clone();
        let tail = preview_page(&fixture, &next).unwrap();
        assert_eq!(tail.rows.len(), 1);
        assert_eq!(tail.rows[0].member, fixture.members[16]);
        assert!(tail.next_cursor.is_none());
        next.columns.reverse();
        assert!(preview_page(&fixture, &next).is_err());
        drop(page);
        drop(tail);
        assert_eq!(case_work_budget::global().used(), before);
    }
    #[test]
    fn native_preview_refuses_invalid_limits_cursors_and_final_authority_change() {
        let mut fixture = PreviewFixture::new(event(0, None), 1);
        for count in [0, 17] {
            let mut request = fixture.request.clone();
            request.limit = count;
            assert!(preview_page(&fixture, &request).is_err());
        }
        assert_eq!(fixture.inner.reads.get(), 0);
        let mut duplicate = fixture.request.clone();
        duplicate.columns.push(duplicate.columns[0].clone());
        assert!(preview_page(&fixture, &duplicate).is_err());
        let mut forged = fixture.request.clone();
        forged.cursor = Some("{}".into());
        assert!(preview_page(&fixture, &forged).is_err());
        fixture.checks.set(0);
        fixture.stale_at_exit = true;
        assert!(preview_page(&fixture, &fixture.request).is_err());
        assert_eq!(fixture.inner.reads.get(), 1);
        let empty = PreviewFixture::new(event(0, None), 0);
        let page = preview_page(&empty, &empty.request).unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.total, 0);
        assert!(page.next_cursor.is_none());
    }
    #[test]
    fn authored_titles_bind_every_page_without_rewriting_original_time_or_record_filters() {
        let mut fixture = HistoryFixture::new();
        for index in 0..130u32 {
            let mut record = event(index as usize, Some(index as i64));
            record.name.clear();
            record.code.clear();
            fixture.push(serde_json::json!({"id":"item"}), 0, index, record);
        }
        let target = entry_id(&fixture.rows[128].3);
        let title = "authored full grouping title";
        fixture.request.authored_view_json = serde_json::json!({"timeline":{"edits":{(target.clone()):{"title":title,"color":"blue"}}},"manual":[{"id":"milestone","start":-100,"name":"authored milestone"}]}).to_string();
        let first = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(first.entries.len(), 128);
        assert_eq!(first.scope_count, 130);
        assert_eq!(
            first.authored_view_signature,
            format!(
                "{:x}",
                Sha256::digest(fixture.request.authored_view_json.as_bytes())
            )
        );
        let mut tail = fixture.request.clone();
        tail.cursor = first.next_cursor.clone();
        let page = timeline(&fixture, &tail).unwrap();
        assert_eq!(page.authored_view_signature, first.authored_view_signature);
        assert_eq!(page.entries[0].entry_id, target);
        assert_eq!(page.entries[0].timestamp_ms, Some(128));
        assert_eq!(page.entries[0].title.text, title);
        assert_eq!(
            page.entries[0].grouping_key,
            binding(&(&page.entries[0].container_id, title)).unwrap()
        );
        let mut changed = tail.clone();
        changed.authored_view_json.push(' ');
        assert!(timeline(&fixture, &changed).is_err());
        let mut filtered = fixture.request.clone();
        filtered.filters = vec![crate::query::Filter {
            column: "message".into(),
            op: "contains".into(),
            value: "authored full".into(),
            value2: None,
        }];
        assert_eq!(timeline(&fixture, &filtered).unwrap().scope_count, 0);
    }
    #[test]
    fn authored_legacy_titles_require_full_case_uniqueness_and_view_limits() {
        let mut fixture = HistoryFixture::new();
        for (index, reference) in ["duplicate", "duplicate", "unique"].into_iter().enumerate() {
            let mut record = event(index, Some(index as i64));
            record.event_ref = reference.into();
            fixture.push(serde_json::json!({"id":"item"}), 0, index as u32, record);
        }
        fixture.request.authored_view_json = serde_json::json!({"timeline":{"edits":{"e:item:duplicate":{"title":"ambiguous title"},"e:item:unique":{"title":"unique title"}}},"manual":[]}).to_string();
        let page = timeline(&fixture, &fixture.request).unwrap();
        assert_eq!(page.entries[0].title.text, "name 0");
        assert_eq!(page.entries[1].title.text, "name 1");
        assert_eq!(page.entries[2].title.text, "unique title");
        let mut malformed = fixture.request.clone();
        malformed.authored_view_json = "{}".into();
        assert!(timeline(&fixture, &malformed).is_err());
        malformed.authored_view_json = " ".repeat(AUTHORED_VIEW_BYTES + 1);
        assert!(timeline(&fixture, &malformed).is_err());
    }
}
