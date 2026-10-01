//! Opt-in table transport. A projected row is deliberately not an Event and
//! must never be used as an exact filter, transform input, or saved evidence.
use crate::{
    analysis_context::Identity, analysis_runtime::Admitted, model::Event, query::SelectedPage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;

pub(crate) const MAX_ROWS: usize = 2_000;
const MAX_COLUMNS: usize = 64;
const MAX_CELL_BYTES: usize = 4 << 10;
const MAX_RESPONSE_BYTES: usize = 2 << 20;
const MIN_CELL_BYTES: usize = 128;
const MAX_HANDLE_BYTES: usize = 2 << 10;
const CURSOR_BYTES: usize = 128 << 10;
const MAX_RECORD_BYTES: usize = 2 << 20;
const MAX_RECORD_OWNED: usize = 4 << 20;
const MAX_DERIVED: usize = 128;
const MAX_VALUE_DEPTH: usize = 32;
const MAX_VALUE_NODES: usize = 16_384;
const MAX_HYDRATE_ROWS: usize = 16;
const MAX_HYDRATE_BYTES: usize = 2 << 20;
const MAX_JS_INTEGER: u64 = 9_007_199_254_740_991;
const FIELD_LIMIT: &str = "PROJECTED_FIELD_LIMIT: O texto exato excede o orçamento de 2 MiB. Use exportação nativa/bruta; nenhum valor foi truncado.";
const NUMBER_ERROR: &str = "PROJECTED_HYDRATION_NUMBER: Uma representação numérica deste registro não pode atravessar JSON/JavaScript sem perda (inteiro amplo ou ponto flutuante integral). Use exportação nativa/bruta para preservar os valores exatos.";
const INVALID: &str = "PROJECTED_PAGE_LIMIT: Reduza as linhas/colunas ou aumente o orçamento solicitado dentro dos limites da política.";
const EXACT_LIMIT: &str = "PROJECTED_HYDRATION_LIMIT: Os registros completos excedem o limite de 16 linhas/2 MiB; solicite um recorte menor ou use uma exportação explícita.";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectionRequest {
    pub projection_version: u8,
    pub columns: Vec<String>,
    pub cell_bytes: Option<usize>,
    pub response_bytes: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Receipt {
    pub analysis_context: Identity,
    pub source_generation: Option<u64>,
    pub case_key: Option<String>,
    pub case_content_token: Option<String>,
    pub catalog_signature: String,
    pub catalog_epoch: u64,
}
impl Receipt {
    pub(crate) fn from_admitted(
        admitted: &Admitted,
        codes: &crate::model::CodesConfig,
        system: &crate::model::CodesConfig,
    ) -> Result<Self, String> {
        let catalog = crate::engine::catalog_token(codes, system);
        let receipt = Self {
            analysis_context: admitted
                .identity
                .clone()
                .ok_or("PROJECTED_PAGE_ADMISSION: Identidade da análise ausente.")?,
            source_generation: admitted.source_generation,
            case_key: admitted.case_key.clone(),
            case_content_token: admitted.case_content_token.clone(),
            catalog_signature: catalog.signature,
            catalog_epoch: catalog.epoch,
        };
        receipt.validate()?;
        Ok(receipt)
    }
    fn validate(&self) -> Result<(), String> {
        if self.analysis_context.config_revision > MAX_JS_INTEGER
            || self.analysis_context.visibility_revision > MAX_JS_INTEGER
            || self.source_generation.is_some_and(|n| n > MAX_JS_INTEGER)
            || self.catalog_epoch > MAX_JS_INTEGER
        {
            return Err(
                "PROJECTED_PAGE_NUMBER: Identidade numérica excede a precisão exata do cliente."
                    .into(),
            );
        }
        if self.analysis_context.case_id.is_empty()
            || self.analysis_context.analysis_id.is_empty()
            || !matches!(
                (self.source_generation, self.case_key.as_deref(), self.case_content_token.as_deref()),
                (Some(_), None, None) | (None, Some(_), Some(_))
            )
            || self.case_key.as_deref().is_some_and(|key| key.is_empty() || key.len() > 512)
            || self.case_content_token.as_deref().is_some_and(|token| token.is_empty() || token.len() > 128)
            || self.catalog_signature.len() != 64
            || !self
                .catalog_signature
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(
                "PROJECTED_PAGE_ADMISSION: Fonte ou chave imutável das evidências ausente.".into(),
            );
        }
        // Bound even adversarial escaped identity strings before copying them
        // into every response. Never truncate identity to make it fit.
        serialized_len(self, 16 << 10).map(|_| ())
    }
    pub(crate) fn validate_admitted(
        &self,
        admitted: &Admitted,
        codes: &crate::model::CodesConfig,
        system: &crate::model::CodesConfig,
    ) -> Result<(), String> {
        self.validate_captured(&Self::from_admitted(admitted, codes, system)?)
    }
    pub(crate) fn validate_catalogs(
        &self,
        codes: &crate::model::CodesConfig,
        system: &crate::model::CodesConfig,
    ) -> Result<(), String> {
        let catalog = crate::engine::catalog_token(codes, system);
        if self.catalog_signature != catalog.signature || self.catalog_epoch != catalog.epoch {
            return Err(crate::analysis_runtime::STALE.into());
        }
        Ok(())
    }
    fn validate_captured(&self, captured: &Self) -> Result<(), String> {
        self.validate()?;
        if self != captured {
            return Err(crate::analysis_runtime::STALE.into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RowHandle {
    pub id: usize,
    pub event_ref: String,
}
impl RowHandle {
    fn checked(id: usize, event_ref: &str) -> Result<Self, String> {
        if id as u128 > u128::from(MAX_JS_INTEGER) {
            return Err("PROJECTED_PAGE_NUMBER: ID excede a precisão exata do cliente.".into());
        }
        if event_ref.is_empty() || event_ref.len() > MAX_HANDLE_BYTES {
            return Err(
                "PROJECTED_PAGE_IDENTITY: Referência estável ausente ou excede o limite.".into(),
            );
        }
        let row = Self {
            id,
            event_ref: event_ref.into(),
        };
        serialized_len(&row, MAX_HANDLE_BYTES)?;
        Ok(row)
    }
    fn matches(&self, event: &Event) -> bool {
        self.id == event.id && self.event_ref == event.event_ref
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueType {
    Null,
    String,
    Number,
    Boolean,
    Array,
    Object,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Cell {
    Missing,
    Null,
    Complete {
        #[serde(rename = "valueType")]
        value_type: ValueType,
        value: Value,
    },
    Preview {
        #[serde(rename = "valueType")]
        value_type: ValueType,
        text: String,
        incomplete: bool,
    },
    Unavailable {
        reason: &'static str,
    },
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FieldPresence {
    Missing,
    Null,
    Present,
}

/// Exact query-comparator text, not a typed Event or a preview. Clients use
/// canonical_text verbatim for copy/filter/transform input; never stringify a
/// separately parsed value. Number/object spelling survives JS round trips.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExactField {
    pub kind: &'static str,
    pub version: u8,
    pub receipt: Receipt,
    pub row: RowHandle,
    pub column: String,
    pub presence: FieldPresence,
    pub value_type: Option<ValueType>,
    pub canonical_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectedRow {
    pub kind: &'static str,
    pub version: u8,
    pub row: RowHandle,
    pub cells: Vec<Cell>,
}

pub(crate) fn is_projected_row(value: &Value) -> bool {
    value.get("kind").and_then(Value::as_str) == Some("projected_row")
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectedPage {
    pub kind: &'static str,
    pub version: u8,
    pub receipt: Receipt,
    pub columns: Vec<String>,
    pub items: Vec<ProjectedRow>,
    pub total: Option<usize>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub engine: String,
    pub warning: Option<String>,
    pub projection_route: &'static str,
    pub cell_bytes: usize,
    pub response_bytes: usize,
}

pub(crate) struct ProjectionPlan {
    pub(crate) limit: usize,
    columns: Vec<String>,
    receipt: Receipt,
    cell_bytes: usize,
    response_bytes: usize,
}
impl ProjectionPlan {
    /// Reserve every requested row and cell before query membership is selected.
    /// The cursor budget is explicit: a larger legacy cursor fails the whole
    /// projected request rather than changing its keys or skipping records.
    pub(crate) fn new(
        request: ProjectionRequest,
        limit: usize,
        receipt: Receipt,
    ) -> Result<Self, String> {
        receipt.validate()?;
        if request.projection_version != 1 {
            return Err("PROJECTED_PAGE_VERSION: Versão de projeção não suportada.".into());
        }
        if limit == 0
            || limit > MAX_ROWS
            || request.columns.is_empty()
            || request.columns.len() > MAX_COLUMNS
            || request
                .columns
                .iter()
                .any(|c| c.is_empty() || c == "*" || c.len() > 1_024)
        {
            return Err(INVALID.into());
        }
        let mut unique = std::collections::HashSet::new();
        if request.columns.iter().any(|column| !unique.insert(column)) {
            return Err("PROJECTED_PAGE_COLUMNS: Colunas duplicadas.".into());
        }
        let response_bytes = request.response_bytes.unwrap_or(MAX_RESPONSE_BYTES);
        let requested_cell = request.cell_bytes.unwrap_or(MAX_CELL_BYTES);
        if response_bytes > MAX_RESPONSE_BYTES
            || !(MIN_CELL_BYTES..=MAX_CELL_BYTES).contains(&requested_cell)
        {
            return Err(INVALID.into());
        }
        let envelope = serialized_len(&(&receipt, &request.columns), 128 << 10)?;
        let reserve = envelope
            .checked_add(CURSOR_BYTES + 4_096)
            .and_then(|n| n.checked_add(limit.checked_mul(MAX_HANDLE_BYTES + 64)?))
            .ok_or(INVALID)?;
        let slots = limit.checked_mul(request.columns.len()).ok_or(INVALID)?;
        let per_cell = response_bytes.checked_sub(reserve).ok_or(INVALID)? / slots;
        // Array separators are charged separately from each complete cell.
        let cell_bytes = requested_cell.min(per_cell.saturating_sub(1));
        if cell_bytes < MIN_CELL_BYTES {
            return Err(INVALID.into());
        }
        Ok(Self {
            limit,
            columns: request.columns,
            receipt,
            cell_bytes,
            response_bytes,
        })
    }
    pub(crate) fn unavailable(&self, row: RowHandle, reason: &'static str) -> ProjectedRow {
        ProjectedRow {
            kind: "projected_row",
            version: 1,
            row,
            cells: self
                .columns
                .iter()
                .map(|_| Cell::Unavailable { reason })
                .collect(),
        }
    }
    pub(crate) fn project(&self, event: &Event) -> Result<ProjectedRow, String> {
        let row = RowHandle::checked(event.id, &event.event_ref)?;
        if !event_work_bounded(event) {
            return Ok(self.unavailable(row, "record_work_limit"));
        }
        if crate::query::event_payload_bytes(event) > MAX_RECORD_OWNED {
            return Ok(self.unavailable(row, "record_too_wide"));
        }
        let cells = self
            .columns
            .iter()
            .map(|column| event_cell(event, column, self.cell_bytes))
            .collect();
        Ok(ProjectedRow {
            kind: "projected_row",
            version: 1,
            row,
            cells,
        })
    }
    pub(crate) fn project_memory(&self, event: &Event) -> Result<ProjectedRow, String> {
        // Borrow the original record. Legacy page annotation clips synthetic
        // role aliases; projected complete cells must use canonical query values.
        self.project(event)
    }
    pub(crate) fn finish(
        &self,
        selected: SelectedPage,
        items: Vec<ProjectedRow>,
    ) -> Result<ProjectedPage, String> {
        if selected.ids.len() != items.len() || items.len() > self.limit {
            return Err("PROJECTED_PAGE_MEMBERSHIP: Página incompleta.".into());
        }
        if selected
            .total
            .is_some_and(|n| n as u128 > u128::from(MAX_JS_INTEGER))
        {
            return Err(
                "PROJECTED_PAGE_NUMBER: Contagem excede a precisão exata do cliente.".into(),
            );
        }
        serialized_len(&selected.next_cursor, CURSOR_BYTES)?;
        let page = ProjectedPage {
            kind: "projected_page",
            version: 1,
            receipt: self.receipt.clone(),
            columns: self.columns.clone(),
            items,
            total: selected.total,
            has_more: selected.has_more,
            next_cursor: selected.next_cursor,
            engine: selected.engine,
            warning: selected.warning,
            projection_route: "one_record_fallback",
            cell_bytes: self.cell_bytes,
            response_bytes: self.response_bytes,
        };
        // Last invariant, not the first budget check. Construction above already
        // bounds row/cell/identity allocations and reserves all selected rows.
        serialized_len(&page, self.response_bytes)?;
        Ok(page)
    }
}

/// Projection only; exact selection and residual verification are separate.
/// Oversized snapshots fail because their stored event_ref cannot be guessed.
pub(crate) fn project_indexed_row(
    idx: &crate::sources::FileIndex,
    id: usize,
    codes: &crate::model::CodesConfig,
    system: &crate::model::CodesConfig,
    derived: &[crate::sources::CompiledDerived],
    plan: &ProjectionPlan,
) -> Result<ProjectedRow, String> {
    crate::operations::check()?;
    if id >= idx.lines.len() {
        return Err("PROJECTED_PAGE_IDENTITY: Registro fora da fonte.".into());
    }
    let reason = if idx.lines.at(id).len as usize > MAX_RECORD_BYTES {
        Some("record_too_wide")
    } else if derived.len() > MAX_DERIVED {
        Some("record_work_limit")
    } else {
        None
    };
    if let Some(reason) = reason {
        let part = idx.part_at(id);
        if part.format == "snapshot" {
            return Err("PROJECTED_PAGE_IDENTITY: Snapshot excede o orçamento para resolver a referência exata.".into());
        }
        let reference = format!(
            "{}:{}",
            part.event_identity.as_deref().unwrap_or(&part.identity),
            idx.lines.at(id).offset - part.base
        );
        return Ok(plan.unavailable(RowHandle::checked(id, &reference)?, reason));
    }
    let event = crate::sources::event_at(idx, id, codes, system, derived);
    plan.project(&event)
}

fn value_type(value: &Value) -> ValueType {
    match value {
        Value::String(_) => ValueType::String,
        Value::Number(_) => ValueType::Number,
        Value::Bool(_) => ValueType::Boolean,
        Value::Array(_) => ValueType::Array,
        Value::Object(_) => ValueType::Object,
        Value::Null => unreachable!("null has its own cell"),
    }
}
fn bounded_shape(value: &Value, depth: usize, nodes: &mut usize) -> bool {
    if depth > MAX_VALUE_DEPTH || *nodes == 0 {
        return false;
    }
    *nodes -= 1;
    match value {
        Value::Array(values) => values.iter().all(|v| bounded_shape(v, depth + 1, nodes)),
        Value::Object(values) => values.values().all(|v| bounded_shape(v, depth + 1, nodes)),
        _ => true,
    }
}
fn event_work_bounded(event: &Event) -> bool {
    let mut nodes = MAX_VALUE_NODES;
    event
        .fields
        .values()
        .all(|v| bounded_shape(v, 0, &mut nodes))
        && event.derived_originals.values().all(|v| match v {
            crate::model::DerivedOriginal::Missing => {
                if nodes == 0 {
                    false
                } else {
                    nodes -= 1;
                    true
                }
            }
            crate::model::DerivedOriginal::Present(value) => bounded_shape(value, 0, &mut nodes),
        })
        && event.derived_diagnostics.len() <= MAX_VALUE_NODES
}
fn safe_signed_integer(value: i64) -> bool {
    (-(MAX_JS_INTEGER as i64)..=MAX_JS_INTEGER as i64).contains(&value)
}

/// Call only after the structural depth/node guard. Floating point Values
/// use binary64, but integral f64 spelling is not retained by JS stringify.
/// Integer Values also have to remain in its exact safe-integer domain.
fn value_numbers_lossless(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            // JS serializes 1.0/-0.0 as 1/0. Reimport then changes serde's
            // numeric representation, which exact reference keys distinguish.
            // Conservatively reject integral f64 values in this first tier.
            (!number.is_f64() || number.as_f64().is_some_and(|n| n.fract() != 0.0))
                && number.as_i64().is_none_or(safe_signed_integer)
                && number.as_u64().is_none_or(|n| n <= MAX_JS_INTEGER)
        }
        Value::Array(values) => values.iter().all(value_numbers_lossless),
        Value::Object(values) => values.values().all(value_numbers_lossless),
        _ => true,
    }
}
fn event_numbers_lossless(event: &Event) -> bool {
    event.id as u128 <= u128::from(MAX_JS_INTEGER)
        && event.timestamp.is_none_or(safe_signed_integer)
        && event.fields.values().all(value_numbers_lossless)
        && event
            .derived_originals
            .values()
            .all(|original| match original {
                crate::model::DerivedOriginal::Missing => true,
                crate::model::DerivedOriginal::Present(value) => value_numbers_lossless(value),
            })
        && event
            .evidence_provenance
            .as_ref()
            .is_none_or(|proof| match proof.locator {
                crate::exclusion_store::Locator::ByteOffset(offset) => offset <= MAX_JS_INTEGER,
                crate::exclusion_store::Locator::StableRecord(_) => true,
            })
}

fn event_cell(event: &Event, column: &str, max: usize) -> Cell {
    match column {
        "id" => typed_cell(&Value::from(event.id), max),
        "timestamp" => event
            .timestamp
            .map(|n| typed_cell(&Value::from(n), max))
            .unwrap_or(Cell::Null),
        "event_ref" => text_cell(&event.event_ref, max),
        "source" => text_cell(&event.source, max),
        "level" => text_cell(&event.level, max),
        "code" => text_cell(&event.code, max),
        "name" => text_cell(&event.name, max),
        "description" => text_cell(&event.description, max),
        "message" => text_cell(&event.message, max),
        "raw" => text_cell(&event.raw, max),
        other => match event.fields.get(other) {
            Some(value) => typed_cell(value, max),
            None if other.starts_with('@') => crate::entities::column_value(event, other)
                .map(|text| text_cell(&text, max))
                .unwrap_or(Cell::Missing),
            None => Cell::Missing,
        },
    }
}
fn text_cell(text: &str, max: usize) -> Cell {
    // Serialize a borrowed str first: no wide String clone precedes the cap.
    let mut writer = BoundedWriter::new(max.saturating_sub(64));
    if serde_json::to_writer(&mut writer, text).is_ok() {
        Cell::Complete {
            value_type: ValueType::String,
            value: Value::from(text),
        }
    } else {
        Cell::Preview {
            value_type: ValueType::String,
            text: display_prefix(text, max.saturating_sub(96)),
            incomplete: true,
        }
    }
}
fn typed_cell(value: &Value, max: usize) -> Cell {
    if value.is_null() {
        return Cell::Null;
    }
    if let Value::String(text) = value {
        return text_cell(text, max);
    }
    let mut nodes = MAX_VALUE_NODES;
    if !bounded_shape(value, 0, &mut nodes) {
        return Cell::Unavailable {
            reason: "value_work_limit",
        };
    }
    if !value_numbers_lossless(value) {
        return Cell::Unavailable {
            reason: "number_not_lossless_in_js",
        };
    }
    let value_type = value_type(value);
    let mut writer = BoundedWriter::new(max.saturating_sub(64));
    if serde_json::to_writer(&mut writer, value).is_ok() {
        Cell::Complete {
            value_type,
            value: value.clone(),
        }
    } else {
        // A JSON prefix is display text, never a partly typed Value.
        let end = match std::str::from_utf8(&writer.bytes) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        let text = std::str::from_utf8(&writer.bytes[..end]).expect("verified UTF-8");
        Cell::Preview {
            value_type,
            text: display_prefix(text, max.saturating_sub(96)),
            incomplete: true,
        }
    }
}
fn display_prefix(text: &str, json_bytes: usize) -> String {
    let mut used = 0usize;
    let mut end = 0usize;
    for (offset, ch) in text.char_indices() {
        let cost = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
            c if c <= '\u{1f}' => 6,
            c => c.len_utf8(),
        };
        if used + cost > json_bytes {
            break;
        }
        used += cost;
        end = offset + ch.len_utf8();
    }
    text[..end].into()
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max: usize,
}
impl BoundedWriter {
    fn new(max: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max,
        }
    }
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let left = self.max.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(left)]);
        if bytes.len() > left {
            Err(std::io::Error::other("Limite de JSON excedido"))
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct Counter {
    used: usize,
    max: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.used) {
            return Err(std::io::Error::other("Limite de JSON excedido"));
        }
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn serialized_len(value: &impl Serialize, max: usize) -> Result<usize, String> {
    let mut counter = Counter { used: 0, max };
    serde_json::to_writer(&mut counter, value).map_err(|_| INVALID.to_string())?;
    Ok(counter.used)
}

/// The caller captures fresh admission from the receipt, prepares visibility,
/// and executes in that exact admission's worker. Archived records are excluded;
/// their separate verified archive capability must be used instead.
pub(crate) fn hydrate_projected_rows(
    state: &crate::AppState,
    admitted: &Admitted,
    receipt: &Receipt,
    handles: &[RowHandle],
    case_events: Option<&[Event]>,
) -> Result<Vec<Event>, String> {
    with_exact_rows(
        state,
        admitted,
        receipt,
        handles,
        case_events,
        MAX_HYDRATE_BYTES,
        |resolve| hydrate_batch(handles, resolve),
    )
}

pub(crate) fn hydrate_projected_field(
    state: &crate::AppState,
    admitted: &Admitted,
    receipt: &Receipt,
    handle: &RowHandle,
    column: &str,
    case_events: Option<&[Event]>,
) -> Result<ExactField, String> {
    validate_exact_column(column)?;
    with_exact_rows(
        state,
        admitted,
        receipt,
        std::slice::from_ref(handle),
        case_events,
        MAX_RECORD_OWNED,
        |resolve| {
            let event = resolve(handle)?;
            exact_field(&event, receipt, column)
        },
    )
}

/// Both exact actions use the same immutable receipt, row/provenance resolver,
/// record limits, visibility check and final admission/catalog validation.
fn with_exact_rows<T>(
    state: &crate::AppState,
    admitted: &Admitted,
    receipt: &Receipt,
    handles: &[RowHandle],
    case_events: Option<&[Event]>,
    record_owned_cap: usize,
    run: impl FnOnce(&mut dyn FnMut(&RowHandle) -> Result<Event, String>) -> Result<T, String>,
) -> Result<T, String> {
    receipt.validate()?;
    let current = crate::analysis_runtime::current()
        .ok_or("PROJECTED_PAGE_ADMISSION: Consulta fora da admissão capturada.")?;
    if !std::ptr::eq(current.as_ref(), admitted) {
        return Err(crate::analysis_runtime::STALE.into());
    }
    admitted.validate(state)?;
    admitted.validate_visibility()?;
    if handles.is_empty() || handles.len() > MAX_HYDRATE_ROWS {
        return Err(EXACT_LIMIT.into());
    }
    for handle in handles {
        RowHandle::checked(handle.id, &handle.event_ref)?;
    }
    if case_events.is_some() != receipt.case_key.is_some() {
        return Err(crate::analysis_runtime::STALE.into());
    }
    let source = crate::analysis_runtime::source(state);
    let codes = state.codes.read();
    let system = state.system_codes.read();
    receipt.validate_admitted(admitted, &codes, &system)?;
    let derived = crate::analysis_runtime::derived(state);
    if derived.len() > MAX_DERIVED {
        return Err(EXACT_LIMIT.into());
    }
    let result = {
        let mut resolve = |handle: &RowHandle| {
            let memory = case_events.or_else(|| match &*source {
                crate::SourceData::Memory(events) => Some(events.as_slice()),
                _ => None,
            });
            let event = if let Some(memory) = memory {
                let mut found = memory.iter().filter(|event| handle.matches(event));
                let event = found.next().ok_or(crate::analysis_runtime::STALE)?;
                if found.next().is_some() || !event_work_bounded(event) {
                    return Err(EXACT_LIMIT.into());
                }
                if crate::query::event_payload_bytes(event) > record_owned_cap {
                    return Err(EXACT_LIMIT.into());
                }
                event.clone()
            } else if let crate::SourceData::Indexed(idx) = &*source {
                if handle.id >= idx.lines.len()
                    || !crate::analysis_runtime::row_visible(idx, handle.id)?
                {
                    return Err(crate::analysis_runtime::STALE.into());
                }
                if idx.lines.at(handle.id).len as usize > MAX_RECORD_BYTES {
                    return Err(EXACT_LIMIT.into());
                }
                let mut event = crate::sources::event_at(idx, handle.id, &codes, &system, &derived);
                if !handle.matches(&event) {
                    return Err(crate::analysis_runtime::STALE.into());
                }
                if !event_work_bounded(&event)
                    || crate::query::event_payload_bytes(&event) > record_owned_cap
                {
                    return Err(EXACT_LIMIT.into());
                }
                crate::analysis_runtime::attach_provenance(idx, &mut event)?;
                event
            } else {
                return Err(crate::analysis_runtime::STALE.into());
            };
            if !handle.matches(&event) {
                return Err(crate::analysis_runtime::STALE.into());
            }
            Ok(event)
        };
        run(&mut resolve)?
    };
    // Revalidation can acquire the live source lock; release catalog/source
    // read guards first to preserve the mutation paths' lock ordering.
    drop(codes);
    drop(system);
    drop(source);
    // No partial success if an identity or visibility mutation raced hydration.
    admitted.validate(state)?;
    admitted.validate_visibility()?;
    receipt.validate_catalogs(&state.codes.read(), &state.system_codes.read())?;
    Ok(result)
}

fn validate_exact_column(column: &str) -> Result<(), String> {
    if column.is_empty() || column == "*" || column.len() > 1_024 {
        return Err("PROJECTED_FIELD_COLUMN: Informe uma coluna exata de até 1.024 bytes.".into());
    }
    Ok(())
}

fn exact_field(event: &Event, receipt: &Receipt, column: &str) -> Result<ExactField, String> {
    validate_exact_column(column)?;
    receipt.validate()?;
    let row = RowHandle::checked(event.id, &event.event_ref)?;
    if !event_work_bounded(event) || crate::query::event_payload_bytes(event) > MAX_RECORD_OWNED {
        return Err(FIELD_LIMIT.into());
    }
    let standard = matches!(
        column,
        "id" | "event_ref"
            | "timestamp"
            | "source"
            | "level"
            | "code"
            | "name"
            | "description"
            | "message"
            | "raw"
    );
    let original = (!standard).then(|| event.fields.get(column)).flatten();
    // col_ref serializes structured dynamic Values. Count first with the same
    // serde serializer, bounding allocation before invoking the actual resolver.
    if let Some(value @ (Value::Array(_) | Value::Object(_))) = original {
        serialized_len(value, MAX_HYDRATE_BYTES).map_err(|_| FIELD_LIMIT.to_string())?;
    }
    let text = event.col_ref(column);
    let (presence, value_type) = match column {
        "timestamp" if event.timestamp.is_none() => (FieldPresence::Null, Some(ValueType::Null)),
        "id" | "timestamp" => (FieldPresence::Present, Some(ValueType::Number)),
        _ if standard => (FieldPresence::Present, Some(ValueType::String)),
        _ => match original {
            Some(Value::Null) => (FieldPresence::Null, Some(ValueType::Null)),
            Some(value) => (FieldPresence::Present, Some(value_type(value))),
            None if text.is_some() => (FieldPresence::Present, Some(ValueType::String)),
            None => (FieldPresence::Missing, None),
        },
    };
    let mut response = ExactField {
        kind: "exact_field",
        version: 1,
        receipt: receipt.clone(),
        row,
        column: column.into(),
        presence,
        value_type,
        canonical_text: None,
    };
    let envelope =
        serialized_len(&response, MAX_HYDRATE_BYTES).map_err(|_| FIELD_LIMIT.to_string())?;
    // Replacing the four-byte null with a JSON string must include escaping.
    let available = MAX_HYDRATE_BYTES
        .checked_sub(envelope)
        .and_then(|n| n.checked_add(4))
        .ok_or(FIELD_LIMIT)?;
    serialized_len(&text.as_deref(), available).map_err(|_| FIELD_LIMIT.to_string())?;
    response.canonical_text = text.map(std::borrow::Cow::into_owned);
    serialized_len(&response, MAX_HYDRATE_BYTES).map_err(|_| FIELD_LIMIT.to_string())?;
    Ok(response)
}

/// Materialize synthetic role columns exactly for this new full-value API.
/// Resolve all values against the original record before inserting aliases;
/// explicit @role fields, including null/objects, always remain authoritative.
fn annotate_exact_roles(event: &mut Event) -> Result<(), String> {
    let mut remaining = MAX_HYDRATE_BYTES
        .checked_sub(crate::query::event_payload_bytes(event))
        .ok_or(EXACT_LIMIT)?;
    let mut values = Vec::new();
    for info in crate::entities::ROLES {
        if event.fields.contains_key(info.column) {
            continue;
        }
        if let Some(value) = crate::entities::column_value(event, info.column) {
            let bytes = value
                .len()
                .saturating_add(info.column.len())
                .saturating_add(64);
            remaining = remaining.checked_sub(bytes).ok_or(EXACT_LIMIT)?;
            values.push((info.column, value));
        }
    }
    let values: Vec<_> = values
        .into_iter()
        .map(|(column, value)| (column, Value::String(value.into_owned())))
        .collect();
    for (column, value) in values {
        event.fields.insert(column.into(), value);
    }
    Ok(())
}

/// Shared all-or-error evidence boundary. A resolver may keep at most one new
/// full record live before this boundary charges it; no evidence is stripped.
fn hydrate_batch(
    handles: &[RowHandle],
    mut resolve: impl FnMut(&RowHandle) -> Result<Event, String>,
) -> Result<Vec<Event>, String> {
    if handles.is_empty() || handles.len() > MAX_HYDRATE_ROWS {
        return Err(EXACT_LIMIT.into());
    }
    let mut events = Vec::with_capacity(handles.len());
    let mut owned = 0usize;
    let mut wire = 2usize; // JSON array envelope
    for handle in handles {
        RowHandle::checked(handle.id, &handle.event_ref)?;
        crate::operations::check()?;
        let mut event = resolve(handle)?;
        if !handle.matches(&event) {
            return Err(crate::analysis_runtime::STALE.into());
        }
        if !event_work_bounded(&event) {
            return Err(EXACT_LIMIT.into());
        }
        if !event_numbers_lossless(&event) {
            return Err(NUMBER_ERROR.into());
        }
        annotate_exact_roles(&mut event)?;
        owned = owned.saturating_add(crate::query::event_payload_bytes(&event));
        if owned > MAX_HYDRATE_BYTES {
            return Err(EXACT_LIMIT.into());
        }
        wire = wire
            .saturating_add(
                serialized_len(&event, MAX_HYDRATE_BYTES.saturating_sub(wire))
                    .map_err(|_| EXACT_LIMIT.to_string())?,
            )
            .saturating_add(1);
        if wire > MAX_HYDRATE_BYTES {
            return Err(EXACT_LIMIT.into());
        }
        events.push(event);
    }
    serialized_len(&events, MAX_HYDRATE_BYTES).map_err(|_| EXACT_LIMIT.to_string())?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt() -> Receipt {
        let catalog = crate::engine::catalog_token(
            &crate::model::CodesConfig::default(),
            &crate::model::CodesConfig::default(),
        );
        Receipt {
            analysis_context: Identity {
                case_id: "case".into(),
                analysis_id: "analysis".into(),
                config_revision: 7,
                visibility_revision: 3,
            },
            source_generation: Some(4),
            case_key: None,
            case_content_token: None,
            catalog_signature: catalog.signature,
            catalog_epoch: catalog.epoch,
        }
    }
    fn request(columns: &[&str]) -> ProjectionRequest {
        ProjectionRequest {
            projection_version: 1,
            columns: columns.iter().map(|s| s.to_string()).collect(),
            cell_bytes: None,
            response_bytes: None,
        }
    }
    fn plan(columns: &[&str], rows: usize) -> ProjectionPlan {
        ProjectionPlan::new(request(columns), rows, receipt()).unwrap()
    }
    fn event(id: usize) -> Event {
        let mut event = Event::empty();
        event.id = id;
        event.event_ref = format!("source:{id}");
        event.message = "small message".into();
        event
    }
    fn selection(ids: Vec<usize>, total: Option<usize>, cursor: Option<&str>) -> SelectedPage {
        SelectedPage {
            ids,
            total,
            has_more: cursor.is_some(),
            next_cursor: cursor.map(str::to_string),
            engine: "test".into(),
            warning: None,
        }
    }

    #[test]
    fn missing_null_empty_and_structured_types_remain_distinct() {
        let mut event = event(9);
        event.fields.insert("null".into(), Value::Null);
        event.fields.insert("empty".into(), json!(""));
        event.fields.insert(
            "object".into(),
            json!({"count": 2, "ready": true, "items": [null, ""]}),
        );
        event.fields.insert("number".into(), json!(42));
        let row = plan(
            &["absent", "null", "empty", "object", "number", "timestamp"],
            1,
        )
        .project(&event)
        .unwrap();
        let json = serde_json::to_value(row).unwrap();
        assert_eq!(json["cells"][0]["state"], "missing");
        assert_eq!(json["cells"][1]["state"], "null");
        assert_eq!(json["cells"][2]["value"], "");
        assert_eq!(json["cells"][3]["value"], event.fields["object"]);
        assert_eq!(json["cells"][3]["valueType"], "object");
        assert_eq!(json["cells"][4]["value"], 42);
        assert_eq!(json["cells"][5]["state"], "null");
        assert!(is_projected_row(&json));
        assert!(serde_json::from_value::<Event>(json).is_err());
    }

    #[test]
    fn hidden_wide_fields_do_not_enter_wire_and_selected_text_is_explicit_preview() {
        let mut event = event(1);
        event
            .fields
            .insert("body".into(), json!("日".repeat(24_000)));
        event.derived_originals.insert(
            "hidden".into(),
            crate::model::DerivedOriginal::Present(json!("original".repeat(20_000))),
        );
        let plan = plan(&["message", "body"], 100);
        let items = (0..100).map(|_| plan.project(&event).unwrap()).collect();
        let page = plan
            .finish(selection(vec![1; 100], Some(100), None), items)
            .unwrap();
        let wire = serde_json::to_vec(&page).unwrap();
        assert!(wire.len() <= MAX_RESPONSE_BYTES);
        assert!(wire.len() < 500_000);
        assert!(!String::from_utf8(wire)
            .unwrap()
            .contains("originaloriginal"));
        assert!(matches!(
            &page.items[0].cells[1],
            Cell::Preview {
                incomplete: true,
                value_type: ValueType::String,
                ..
            }
        ));
        assert!(event.fields["body"].as_str().unwrap().len() > MAX_CELL_BYTES);
    }

    #[test]
    fn escaped_unicode_previews_and_complete_cells_obey_actual_json_cap() {
        for max in [MIN_CELL_BYTES, 256, MAX_CELL_BYTES] {
            for text in ["plain", "日🦀", "\"\\\n\u{0}"] {
                let text = text.repeat(20_000);
                let cell = text_cell(&text, max);
                assert!(serde_json::to_vec(&cell).unwrap().len() <= max);
                if let Cell::Preview { text: prefix, .. } = cell {
                    assert!(text.starts_with(&prefix));
                } else {
                    panic!("expected preview");
                }
            }
            let value = json!({"\"\\\u{0}": ["日🦀".repeat(20_000)]});
            assert!(serde_json::to_vec(&typed_cell(&value, max)).unwrap().len() <= max);
            let complete = typed_cell(&json!([true, 7, {"a": "日"}]), max);
            assert!(matches!(complete, Cell::Complete { .. }));
            assert!(serde_json::to_vec(&complete).unwrap().len() <= max);
        }
    }

    #[test]
    fn deep_or_many_values_fail_work_guard_without_serializing_every_child() {
        let mut deep = json!(1);
        for _ in 0..MAX_VALUE_DEPTH + 2 {
            deep = Value::Array(vec![deep]);
        }
        assert!(matches!(
            typed_cell(&deep, MAX_CELL_BYTES),
            Cell::Unavailable {
                reason: "value_work_limit"
            }
        ));
        let many = Value::Array(vec![Value::Null; MAX_VALUE_NODES + 1]);
        assert!(matches!(
            typed_cell(&many, MAX_CELL_BYTES),
            Cell::Unavailable { .. }
        ));
    }

    #[test]
    fn admission_budget_rejects_infeasible_rows_and_escaped_identity_or_columns() {
        assert!(ProjectionPlan::new(request(&["x"]), 0, receipt()).is_err());
        assert!(ProjectionPlan::new(request(&["x"]), MAX_ROWS, receipt()).is_err());
        assert!(ProjectionPlan::new(request(&["*"]), 1, receipt()).is_err());
        assert!(ProjectionPlan::new(request(&["x", "x"]), 1, receipt()).is_err());
        let mut small = request(&["x"]);
        small.response_bytes = Some(100);
        assert!(ProjectionPlan::new(small, 1, receipt()).is_err());
        let mut long = receipt();
        long.analysis_context.case_id = "\u{0}".repeat(4_000);
        assert!(ProjectionPlan::new(request(&["x"]), 1, long).is_err());
        let mut columns = request(&["x"]);
        columns.columns = (0..MAX_COLUMNS)
            .map(|i| format!("{}{}", "\u{0}".repeat(1_000), i))
            .collect();
        assert!(ProjectionPlan::new(columns, 1, receipt()).is_err());
        assert!(RowHandle::checked(1, &"\u{0}".repeat(1_000)).is_err());
    }

    #[test]
    fn unavailable_cells_keep_every_handle_and_preserve_cursor_metadata() {
        let plan = plan(&["body"], 3);
        let ids = vec![8, 2, 7];
        let items = ids
            .iter()
            .map(|&id| {
                plan.unavailable(
                    RowHandle::checked(id, &format!("source:{id}")).unwrap(),
                    "record_too_wide",
                )
            })
            .collect();
        let page = plan
            .finish(
                selection(ids.clone(), None, Some("exact-original-cursor")),
                items,
            )
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.row.id)
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(page.next_cursor.as_deref(), Some("exact-original-cursor"));
        assert_eq!(page.total, None);
        assert!(page.has_more);
        assert!(plan
            .finish(selection(vec![1], Some(1), None), vec![])
            .is_err());
        assert!(plan
            .finish(
                selection(vec![], None, Some(&"x".repeat(CURSOR_BYTES))),
                vec![]
            )
            .is_err());
    }

    #[test]
    fn exact_hydration_is_ordered_unclipped_and_all_or_error() {
        let mut first = event(1);
        first.raw = "exact raw".into();
        first
            .fields
            .insert("body".into(), json!("日".repeat(10_000)));
        first.derived_originals.insert(
            "body".into(),
            crate::model::DerivedOriginal::Present(json!({"original": true})),
        );
        let second = event(2);
        let handles = [
            RowHandle::checked(2, "source:2").unwrap(),
            RowHandle::checked(1, "source:1").unwrap(),
        ];
        let rows = hydrate_batch(&handles, |handle| {
            Ok(if handle.id == 1 {
                first.clone()
            } else {
                second.clone()
            })
        })
        .unwrap();
        assert_eq!(rows.iter().map(|e| e.id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(rows[1].raw, first.raw);
        assert_eq!(rows[1].fields["body"], first.fields["body"]);
        assert_eq!(
            serde_json::to_value(&rows[1].derived_originals).unwrap(),
            serde_json::to_value(&first.derived_originals).unwrap()
        );
        assert!(hydrate_batch(&handles, |_| Ok(first.clone())).is_err());
        let mut changed = first.clone();
        changed.event_ref = "changed:1".into();
        assert!(hydrate_batch(&handles[1..], |_| Ok(changed.clone())).is_err());
        let mut wide = first.clone();
        wide.raw = "x".repeat(MAX_HYDRATE_BYTES);
        assert!(hydrate_batch(&handles[1..], |_| Ok(wide.clone())).is_err());
        let mut escaped = first.clone();
        escaped.raw = "\u{0}".repeat(MAX_HYDRATE_BYTES / 5);
        assert!(hydrate_batch(&handles[1..], |_| Ok(escaped.clone())).is_err());
        assert!(
            hydrate_batch(&vec![handles[0].clone(); MAX_HYDRATE_ROWS + 1], |_| Ok(
                second.clone()
            ))
            .is_err()
        );
    }

    #[test]
    fn exact_field_text_preserves_actual_query_spelling_through_json_roundtrip() {
        let mut event = event(1);
        event.timestamp = Some(0);
        event.fields.insert("float".into(), json!(1.0));
        // Deliberately use the opposite of JavaScript's numeric-key order.
        let mut object = serde_json::Map::new();
        object.insert("10".into(), json!("tenth"));
        object.insert("2".into(), json!("second"));
        object.insert("nested".into(), json!([u64::MAX, 1.0]));
        event.fields.insert("object".into(), Value::Object(object));
        event.fields.insert("unsafe".into(), json!(u64::MAX));
        event.fields.insert("null".into(), Value::Null);
        event.fields.insert("empty".into(), json!(""));
        event.fields.insert(
            "user_agent".into(),
            json!(format!("{}Z", "a".repeat(4_000))),
        );
        for column in [
            "float",
            "object",
            "unsafe",
            "null",
            "empty",
            "absent",
            "@user_agent",
            "timestamp",
            "id",
        ] {
            let expected = event.col_ref(column).map(std::borrow::Cow::into_owned);
            let field = exact_field(&event, &receipt(), column).unwrap();
            let wire = serde_json::to_vec(&field).unwrap();
            let roundtrip: Value = serde_json::from_slice(&wire).unwrap();
            assert_eq!(
                roundtrip["canonicalText"].as_str(),
                expected.as_deref(),
                "{column}"
            );
            assert_eq!(field.canonical_text, expected);
        }
        assert_eq!(
            exact_field(&event, &receipt(), "float")
                .unwrap()
                .canonical_text
                .as_deref(),
            Some("1.0")
        );
        assert_eq!(
            exact_field(&event, &receipt(), "unsafe")
                .unwrap()
                .canonical_text
                .as_deref(),
            Some("18446744073709551615")
        );
        assert!(exact_field(&event, &receipt(), "object")
            .unwrap()
            .canonical_text
            .unwrap()
            .starts_with("{\"10\":\"tenth\",\"2\":\"second\""));
        assert!(matches!(
            exact_field(&event, &receipt(), "absent").unwrap().presence,
            FieldPresence::Missing
        ));
        let null = exact_field(&event, &receipt(), "null").unwrap();
        assert!(matches!(null.presence, FieldPresence::Null));
        assert_eq!(null.canonical_text.as_deref(), Some("null"));
        event
            .fields
            .insert("@user_agent".into(), json!(["one", "two"]));
        assert_eq!(
            exact_field(&event, &receipt(), "@user_agent")
                .unwrap()
                .canonical_text
                .as_deref(),
            Some("[\"one\",\"two\"]")
        );
    }

    #[test]
    fn exact_field_rejects_escaped_wire_overflow_and_never_returns_a_prefix() {
        let mut event = event(1);
        event.fields.insert(
            "escaped".into(),
            json!("\u{0}".repeat(MAX_HYDRATE_BYTES / 5)),
        );
        let error = exact_field(&event, &receipt(), "escaped").unwrap_err();
        assert!(error.starts_with("PROJECTED_FIELD_LIMIT:"));
        event.fields.insert(
            "object".into(),
            json!({"escaped":"\u{0}".repeat(MAX_HYDRATE_BYTES / 5)}),
        );
        assert!(exact_field(&event, &receipt(), "object").is_err());
        assert!(exact_field(&event, &receipt(), "*").is_err());
    }

    #[test]
    fn integral_float_roundtrips_do_not_silently_change_reference_key_type() {
        let original: Value = serde_json::from_str("1.0").unwrap();
        let js_roundtrip: Value = serde_json::from_str("1").unwrap();
        assert!(original.as_number().unwrap().is_f64());
        assert!(js_roundtrip.as_number().unwrap().is_u64());
        assert_ne!(original.to_string(), js_roundtrip.to_string());
        for value in [
            original,
            json!(-0.0),
            json!(1e20),
            json!({"nested":[1.0, -0.0]}),
        ] {
            assert!(matches!(
                typed_cell(&value, MAX_CELL_BYTES),
                Cell::Unavailable {
                    reason: "number_not_lossless_in_js"
                }
            ));
            let mut event = event(1);
            event.fields.insert("value".into(), value.clone());
            let handle = RowHandle::checked(1, "source:1").unwrap();
            assert!(hydrate_batch(&[handle], |_| Ok(event.clone()))
                .unwrap_err()
                .starts_with("PROJECTED_HYDRATION_NUMBER:"));
            let field = exact_field(&event, &receipt(), "value").unwrap();
            assert_eq!(
                field.canonical_text.as_deref(),
                Some(value.to_string().as_str())
            );
        }
        assert!(matches!(
            typed_cell(&json!(1.25), MAX_CELL_BYTES),
            Cell::Complete { .. }
        ));
    }

    #[test]
    fn unsafe_integers_are_unavailable_recursively_and_exact_hydration_rejects_them() {
        for value in [
            json!(MAX_JS_INTEGER),
            json!(-(MAX_JS_INTEGER as i64)),
            json!(1.5),
            json!("18446744073709551615"),
        ] {
            assert!(matches!(
                typed_cell(&value, MAX_CELL_BYTES),
                Cell::Complete { .. }
            ));
        }
        for value in [
            json!(MAX_JS_INTEGER + 1),
            json!(-(MAX_JS_INTEGER as i64) - 1),
            json!(i64::MIN),
            json!(i64::MAX),
            json!(u64::MAX),
            json!({"array":[1, {"id":u64::MAX}]}),
            json!([null, i64::MIN]),
        ] {
            assert!(matches!(
                typed_cell(&value, MAX_CELL_BYTES),
                Cell::Unavailable {
                    reason: "number_not_lossless_in_js"
                }
            ));
            let mut event = event(1);
            event.fields.insert("unsafe".into(), value.clone());
            let handle = RowHandle::checked(1, "source:1").unwrap();
            assert!(hydrate_batch(&[handle.clone()], |_| Ok(event.clone()))
                .unwrap_err()
                .starts_with("PROJECTED_HYDRATION_NUMBER:"));
            event.fields.clear();
            event.derived_originals.insert(
                "unsafe".into(),
                crate::model::DerivedOriginal::Present(value),
            );
            assert!(hydrate_batch(&[handle], |_| Ok(event.clone()))
                .unwrap_err()
                .starts_with("PROJECTED_HYDRATION_NUMBER:"));
        }
        let mut event = event(1);
        event.timestamp = Some(i64::MAX);
        let handle = RowHandle::checked(1, "source:1").unwrap();
        assert!(hydrate_batch(&[handle.clone()], |_| Ok(event.clone())).is_err());
        event.timestamp = None;
        event.evidence_provenance = Some(crate::analysis_visibility::EvidenceProvenance {
            source: crate::exclusion_store::SourceDescriptor {
                version: "source".into(),
                record_space: "records".into(),
                label: "fixture".into(),
                event_ref_prefix: None,
            },
            locator: crate::exclusion_store::Locator::ByteOffset(u64::MAX),
        });
        assert!(hydrate_batch(&[handle], |_| Ok(event.clone())).is_err());
        if usize::BITS >= 64 {
            assert!(RowHandle::checked((MAX_JS_INTEGER + 1) as usize, "source:large").is_err());
        }
        let mut receipt = receipt();
        receipt.analysis_context.config_revision = MAX_JS_INTEGER + 1;
        assert!(ProjectionPlan::new(request(&["id"]), 1, receipt).is_err());
    }

    #[test]
    fn synthetic_role_projection_and_hydration_preserve_exact_query_value() {
        let mut event = event(1);
        let full = format!("{}Z", "a".repeat(4_000));
        event.fields.insert("user_agent".into(), json!(full));
        assert_eq!(event.col_ref("@user_agent").as_deref(), Some(full.as_str()));
        let row = plan(&["@user_agent"], 1).project_memory(&event).unwrap();
        assert_eq!(serde_json::to_value(&row.cells[0]).unwrap()["value"], full);
        let handle = RowHandle::checked(1, "source:1").unwrap();
        let exact = hydrate_batch(&[handle.clone()], |_| Ok(event.clone())).unwrap();
        assert_eq!(exact[0].fields["@user_agent"], full);
        assert_eq!(exact[0].fields["user_agent"], full);
        let mut legacy = event.clone();
        crate::entities::annotate(&mut legacy);
        assert_eq!(legacy.fields["@user_agent"].as_str().unwrap().len(), 4_000);
        // An explicit original field wins over inference and retains its type.
        event.fields.insert("@user_agent".into(), Value::Null);
        let row = plan(&["@user_agent"], 1).project_memory(&event).unwrap();
        assert!(matches!(row.cells[0], Cell::Null));
        let exact = hydrate_batch(&[handle], |_| Ok(event.clone())).unwrap();
        assert_eq!(exact[0].fields["@user_agent"], Value::Null);
    }

    #[test]
    fn multi_valued_roles_keep_typed_originals_without_invented_joining() {
        let mut event = event(1);
        let values = json!(["agent A", "agent B"]);
        event.fields.insert("user_agent".into(), values.clone());
        assert!(crate::entities::column_value(&event, "@user_agent").is_none());
        let row = plan(&["@user_agent", "user_agent"], 1)
            .project(&event)
            .unwrap();
        // The canonical virtual role is absent; the original array is exact.
        assert!(matches!(row.cells[0], Cell::Missing));
        assert_eq!(
            serde_json::to_value(&row.cells[1]).unwrap()["value"],
            values
        );
        let handle = RowHandle::checked(1, "source:1").unwrap();
        let exact = hydrate_batch(&[handle.clone()], |_| Ok(event.clone())).unwrap();
        assert!(!exact[0].fields.contains_key("@user_agent"));
        assert_eq!(exact[0].fields["user_agent"], values);
        event.fields.insert("@user_agent".into(), values.clone());
        let row = plan(&["@user_agent"], 1).project(&event).unwrap();
        assert_eq!(
            serde_json::to_value(&row.cells[0]).unwrap()["valueType"],
            "array"
        );
        assert_eq!(
            serde_json::to_value(&row.cells[0]).unwrap()["value"],
            values
        );
        let exact = hydrate_batch(&[handle], |_| Ok(event.clone())).unwrap();
        assert_eq!(exact[0].fields["@user_agent"], values);
    }

    #[test]
    fn catalog_signature_tracks_both_catalogs_and_frames_control_characters() {
        use crate::model::{CodeInfo, CodesConfig};
        let mut codes = CodesConfig::default();
        codes.sources.entry("source".into()).or_default().insert(
            "1".into(),
            CodeInfo {
                name: "name\u{1}part".into(),
                description: "description".into(),
            },
        );
        let empty = CodesConfig::default();
        let mut receipt = receipt();
        receipt.catalog_signature = crate::engine::catalog_content_signature(&codes, &empty);
        assert!(receipt.validate_catalogs(&codes, &empty).is_ok());
        let mut changed = codes.clone();
        changed
            .sources
            .get_mut("source")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .name = "name".into();
        changed
            .sources
            .get_mut("source")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .description = "part\u{1}description".into();
        // These values collide under separator-only name/description framing.
        assert!(receipt.validate_catalogs(&changed, &empty).is_err());
        assert!(receipt.validate_catalogs(&empty, &codes).is_err());
        let mut system = empty.clone();
        system.sources.entry("source".into()).or_default().insert(
            "2".into(),
            CodeInfo {
                name: "system".into(),
                description: "changed".into(),
            },
        );
        assert!(receipt.validate_catalogs(&codes, &system).is_err());
        let mut reordered = CodesConfig::default();
        for code in ["2", "1"] {
            reordered
                .sources
                .entry("source".into())
                .or_default()
                .insert(
                    code.into(),
                    CodeInfo {
                        name: code.into(),
                        description: String::new(),
                    },
                );
        }
        let mut forward = CodesConfig::default();
        for code in ["1", "2"] {
            forward.sources.entry("source".into()).or_default().insert(
                code.into(),
                CodeInfo {
                    name: code.into(),
                    description: String::new(),
                },
            );
        }
        assert_eq!(
            crate::engine::catalog_content_signature(&reordered, &empty),
            crate::engine::catalog_content_signature(&forward, &empty)
        );
    }

    #[test]
    fn any_receipt_axis_change_rejects_stale_action() {
        let original = receipt();
        assert!(original.validate_captured(&original).is_ok());
        let mut changed = original.clone();
        changed.source_generation = Some(5);
        assert!(original.validate_captured(&changed).is_err());
        changed = original.clone();
        changed.analysis_context.config_revision += 1;
        assert!(original.validate_captured(&changed).is_err());
        changed = original.clone();
        changed.analysis_context.visibility_revision += 1;
        assert!(original.validate_captured(&changed).is_err());
        changed = original.clone();
        changed.analysis_context.analysis_id = "replacement".into();
        assert!(original.validate_captured(&changed).is_err());
        changed = original.clone();
        changed.catalog_signature = "f".repeat(64);
        assert!(original.validate_captured(&changed).is_err());
        changed = original.clone();
        changed.catalog_epoch += 1;
        assert!(original.validate_captured(&changed).is_err());
        let case = Receipt {
            source_generation: None,
            case_key: Some("evidence:1".into()),
            case_content_token: Some("publication:1".into()),
            ..original
        };
        changed = case.clone();
        changed.case_key = Some("evidence:2".into());
        assert!(case.validate_captured(&changed).is_err());
        assert!(case.validate_captured(&case).is_ok());
        changed = case.clone(); changed.case_content_token = Some("publication:2".into());
        assert!(case.validate_captured(&changed).is_err());
    }
}

#[cfg(test)]
mod admitted_action_tests {
    use super::*;
    use crate::{analysis_runtime::{self, Mode}, model::CodesConfig, AppState, SourceData};
    use parking_lot::{Mutex, RwLock};
    use serde_json::json;
    use std::sync::Arc;

    struct Fixture {
        directory: tempfile::TempDir,
        previous_directory: Option<std::ffi::OsString>,
        state: AppState,
        identity: Identity,
    }
    impl Fixture {
        fn new(events: Vec<Event>) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let previous_directory = std::env::var_os("LOGINSIGHT_DATA_DIR");
            std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
            crate::case_store::save_at(directory.path(), json!({"cases":[{"id":"projection-admission"}],"active":null})).unwrap();
            let identity = crate::analysis_context::snapshot("projection-admission").unwrap().identity();
            let state = AppState {
                source: RwLock::new(SourceData::Memory(events)),
                source_publication: RwLock::new(Default::default()),
                source_names: RwLock::new(vec!["projection fixture".into()]),
                codes: RwLock::new(CodesConfig::default()),
                system_codes: RwLock::new(CodesConfig::default()),
                derived: RwLock::new(Vec::new()),
                case_store_lock: Mutex::new(()),
                codes_path: directory.path().join("codes.json"),
                system_codes_path: directory.path().join("system-codes.json"),
            };
            // Publish a real Dataset generation owned by this analysis.
            let owner = analysis_runtime::capture(&state, Some(identity.clone()), Some(0), Mode::Publish).unwrap();
            analysis_runtime::with(Some(owner), || {
                let _source = crate::source_write_checked(&state).unwrap();
                let next = crate::source_publication::prepare_touch_locked(&state).unwrap();
                crate::source_publication::commit_touch_locked(&state, next);
            });
            Self { directory, previous_directory, state, identity }
        }
        fn capture(&self) -> (Arc<Admitted>, Receipt) {
            let generation = {
                let _source = self.state.source.read();
                crate::source_publication::receipt_locked(&self.state).generation
            };
            let admitted = analysis_runtime::capture(&self.state, Some(self.identity.clone()), Some(generation), Mode::Dataset).unwrap();
            let receipt = Receipt::from_admitted(&admitted, &self.state.codes.read(), &self.state.system_codes.read()).unwrap();
            analysis_runtime::with(Some(admitted.clone()), || admitted.prepare_visibility(None)).unwrap();
            (admitted, receipt)
        }
        fn capture_case(&self, key: &str, events: Vec<Event>) -> (Arc<Admitted>, Receipt, Vec<Event>) {
            tauri::async_runtime::block_on(crate::case_cache::case_sync(key.into(), events, Some(self.identity.clone()))).unwrap();
            let (admitted, events) = analysis_runtime::capture_case(&self.state, Some(self.identity.clone()), None, None, Some(key.into())).unwrap();
            let receipt = Receipt::from_admitted(&admitted, &self.state.codes.read(), &self.state.system_codes.read()).unwrap();
            let events = analysis_runtime::with(Some(admitted.clone()), || admitted.prepare_visibility(events)).unwrap().unwrap();
            (admitted, receipt, events)
        }
        fn assert_stale_actions(&self, admitted: &Arc<Admitted>, receipt: &Receipt, row: &RowHandle) {
            analysis_runtime::with(Some(admitted.clone()), || {
                assert!(hydrate_projected_rows(&self.state, admitted, receipt, std::slice::from_ref(row), None).is_err());
                assert!(hydrate_projected_field(&self.state, admitted, receipt, row, "message", None).is_err());
            });
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.previous_directory {
                Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    fn event(id: usize) -> Event {
        let mut event = Event::empty();
        event.id = id;
        event.event_ref = format!("source:{id}");
        event.source = "service".into();
        event.code = "200".into();
        event.message = format!("record {id}");
        event.raw = format!("original record {id}");
        event
    }
    fn row(id: usize) -> RowHandle { RowHandle::checked(id, &format!("source:{id}")).unwrap() }

    #[test]
    fn real_admission_preserves_requested_order_and_rejects_wrong_row_reference() {
        let fixture = Fixture::new(vec![event(0), event(1)]);
        let (admitted, receipt) = fixture.capture();
        analysis_runtime::with(Some(admitted.clone()), || {
            let exact = hydrate_projected_rows(&fixture.state, &admitted, &receipt, &[row(1), row(0)], None).unwrap();
            assert_eq!(exact.iter().map(|event| event.id).collect::<Vec<_>>(), vec![1, 0]);
            assert_eq!(exact[1].raw, "original record 0");
            let invalid = RowHandle::checked(1, "replacement:1").unwrap();
            assert!(hydrate_projected_rows(&fixture.state, &admitted, &receipt, &[row(0), invalid.clone()], None).is_err());
            assert!(hydrate_projected_field(&fixture.state, &admitted, &receipt, &invalid, "message", None).is_err());
        });
        let (other, _) = fixture.capture();
        analysis_runtime::with(Some(other), || {
            assert!(hydrate_projected_rows(&fixture.state, &admitted, &receipt, &[row(0)], None).is_err(), "the helper must execute inside its own captured admission");
        });
    }

    #[test]
    fn real_source_replacement_rejects_previously_admitted_exact_actions() {
        let fixture = Fixture::new(vec![event(0)]);
        let (admitted, receipt) = fixture.capture();
        {
            let mut source = fixture.state.source.write();
            let mut replacement = event(0);
            replacement.message = "new source bytes".into();
            *source = SourceData::Memory(vec![replacement]);
            let next = crate::source_publication::prepare_touch_locked(&fixture.state).unwrap();
            crate::source_publication::commit_touch_locked(&fixture.state, next);
        }
        fixture.assert_stale_actions(&admitted, &receipt, &row(0));
        assert!(analysis_runtime::capture(&fixture.state, Some(fixture.identity.clone()), receipt.source_generation, Mode::Dataset).is_err());
    }

    #[test]
    fn real_catalog_publication_rejects_old_receipt_and_fresh_receipt_reads_new_value() {
        let fixture = Fixture::new(vec![event(0)]);
        let (admitted, receipt) = fixture.capture();
        crate::save_codes_impl(&fixture.state, r#"{"service":{"200":{"name":"updated","description":"new catalog"}}}"#).unwrap();
        fixture.assert_stale_actions(&admitted, &receipt, &row(0));
        let (fresh, current) = fixture.capture();
        assert_ne!(current.catalog_signature, receipt.catalog_signature);
        assert_ne!(current.catalog_epoch, receipt.catalog_epoch);
        analysis_runtime::with(Some(fresh.clone()), || {
            let field = hydrate_projected_field(&fixture.state, &fresh, &current, &row(0), "name", None).unwrap();
            assert_eq!(field.canonical_text.as_deref(), Some("updated"));
        });
    }

    #[test]
    fn real_config_and_visibility_revisions_reject_both_exact_action_types() {
        for visibility in [false, true] {
            let fixture = Fixture::new(vec![event(0)]);
            let (admitted, receipt) = fixture.capture();
            if visibility {
                let (updated, ()) = crate::analysis_context::visibility_update_at(fixture.directory.path(), &fixture.identity, &|| false, |_| Ok(())).unwrap();
                assert_eq!(updated.visibility_revision, fixture.identity.visibility_revision + 1);
            } else {
                let config: crate::analysis_context::Config = serde_json::from_value(json!({"derivedFields":[{"name":"tag","source":"message","rules":[{"pattern":"(.*)","template":"changed"}]}]})).unwrap();
                let updated = crate::analysis_context::update(&fixture.identity, config).unwrap();
                assert_eq!(updated.config_revision, fixture.identity.config_revision + 1);
            }
            fixture.assert_stale_actions(&admitted, &receipt, &row(0));
            assert!(analysis_runtime::capture(&fixture.state, Some(receipt.analysis_context.clone()), receipt.source_generation, Mode::Dataset).is_err());
        }
    }

    #[test]
    fn case_exact_actions_require_the_captured_evidence_key() {
        let fixture = Fixture::new(vec![event(0)]);
        let (admitted, receipt, events) = fixture.capture_case("evidence-a", vec![event(0)]);
        analysis_runtime::with(Some(admitted.clone()), || {
            assert_eq!(hydrate_projected_field(&fixture.state, &admitted, &receipt, &row(0), "message", Some(&events)).unwrap().canonical_text.as_deref(), Some("record 0"));
        });
        let (changed, _, changed_events) = fixture.capture_case("evidence-b", vec![event(0)]);
        analysis_runtime::with(Some(changed.clone()), || {
            assert!(hydrate_projected_rows(&fixture.state, &changed, &receipt, &[row(0)], Some(&changed_events)).is_err());
            assert!(hydrate_projected_field(&fixture.state, &changed, &receipt, &row(0), "message", Some(&changed_events)).is_err());
        });
    }

    #[test]
    fn same_key_case_replacement_invalidates_exact_actions_and_cache_namespace() {
        let fixture = Fixture::new(vec![event(0)]);
        let (old, receipt, _) = fixture.capture_case("same-case-key", vec![event(0)]);
        let old_namespace = analysis_runtime::with(Some(old), analysis_runtime::cache_namespace);
        let mut replacement = event(0); replacement.message = "replacement content".into();
        let (current, current_receipt, events) = fixture.capture_case("same-case-key", vec![replacement]);
        assert_eq!(receipt.analysis_context, current_receipt.analysis_context);
        assert_eq!(receipt.case_key, current_receipt.case_key);
        assert_ne!(receipt.case_content_token, current_receipt.case_content_token);
        analysis_runtime::with(Some(current.clone()), || {
            assert_ne!(old_namespace, analysis_runtime::cache_namespace());
            assert!(hydrate_projected_rows(&fixture.state, &current, &receipt, &[row(0)], Some(&events)).is_err());
            assert!(hydrate_projected_field(&fixture.state, &current, &receipt, &row(0), "message", Some(&events)).is_err());
            let fresh = hydrate_projected_field(&fixture.state, &current, &current_receipt, &row(0), "message", Some(&events)).unwrap();
            assert_eq!(fresh.canonical_text.as_deref(), Some("replacement content"));
        });
        for key in ["evict-a", "evict-b", "evict-c"] {
            tauri::async_runtime::block_on(crate::case_cache::case_sync(key.into(), Vec::new(), Some(fixture.identity.clone()))).unwrap();
        }
        match analysis_runtime::capture_case(&fixture.state, Some(fixture.identity.clone()), None, None, Some("same-case-key".into())) {
            Err(error) => assert_eq!(error, crate::case_cache::MISS),
            Ok(_) => panic!("expired Case content must not silently resolve a replacement"),
        }
    }

    #[test]
    fn synchronized_case_replacement_between_resolution_and_final_check_is_rejected() {
        let fixture = Fixture::new(vec![event(0)]);
        let (admitted, receipt, events) = fixture.capture_case("racing-case-key", vec![event(0)]);
        let result = analysis_runtime::with(Some(admitted.clone()), || {
            with_exact_rows(&fixture.state, &admitted, &receipt, &[row(0)], Some(&events), MAX_RECORD_OWNED, |resolve| {
                let resolved = resolve(&row(0))?;
                let completed = exact_field(&resolved, &receipt, "message")?;
                let mut replacement = event(0); replacement.message = "published after resolution".into();
                tauri::async_runtime::block_on(crate::case_cache::case_sync("racing-case-key".into(), vec![replacement], Some(fixture.identity.clone())))?;
                Ok(completed)
            })
        });
        assert_eq!(result.unwrap_err(), crate::case_cache::CHANGED);

        // Ordinary explicit-input commands still own a request-local snapshot;
        // they are not forced to look up a cache entry they never synchronized.
        let (explicit, values) = analysis_runtime::capture_case(&fixture.state, Some(fixture.identity.clone()), None, Some(vec![event(0)]), Some("racing-case-key".into())).unwrap();
        analysis_runtime::with(Some(explicit.clone()), || {
            explicit.prepare_visibility(values).unwrap();
            tauri::async_runtime::block_on(crate::case_cache::case_sync("racing-case-key".into(), Vec::new(), Some(fixture.identity.clone()))).unwrap();
            assert!(explicit.validate(&fixture.state).is_ok());
            assert!(explicit.validate_visibility().is_ok());
        });
    }

    #[test]
    fn parsed_nginx_integral_latency_uses_exact_field_text_but_rejects_event_roundtrip() {
        let records = [
            r#"{"source":"nginx","code":200,"request_method":"GET","request_uri":"/health","message":"GET /health","request_time":1.000}"#,
            r#"{"source":"nginx","code":200,"request_method":"GET","request_uri":"/health","message":"GET /health","request_time":1.250}"#,
        ];
        let events: Vec<_> = records.iter().enumerate().map(|(id, raw)| {
            let mut event = crate::sources::parse_line(raw.as_bytes(), "jsonl", None, &[]);
            event.id = id;
            event.event_ref = format!("source:{id}");
            event
        }).collect();
        assert!(events[0].fields["request_time"].as_number().unwrap().is_f64());
        assert_eq!(events[0].col_ref("request_time").as_deref(), Some("1.0"));
        let fixture = Fixture::new(events);
        let (admitted, receipt) = fixture.capture();
        analysis_runtime::with(Some(admitted.clone()), || {
            let source = analysis_runtime::source(&fixture.state);
            let SourceData::Memory(events) = &*source else { panic!() };
            let plan = ProjectionPlan::new(ProjectionRequest { projection_version: 1, columns: vec!["request_time".into()], cell_bytes: None, response_bytes: None }, 2, receipt.clone()).unwrap();
            let page = crate::query::query_projected_memory(events, &[], "id", "asc", 0, &plan).unwrap();
            assert!(matches!(page.items[0].cells[0], Cell::Unavailable { reason: "number_not_lossless_in_js" }));
            assert!(matches!(page.items[1].cells[0], Cell::Complete { value_type: ValueType::Number, .. }));
            drop(source);
            let field = hydrate_projected_field(&fixture.state, &admitted, &receipt, &row(0), "request_time", None).unwrap();
            assert_eq!(field.canonical_text.as_deref(), Some("1.0"));
            let error = hydrate_projected_rows(&fixture.state, &admitted, &receipt, &[row(0)], None).unwrap_err();
            assert!(error.starts_with("PROJECTED_HYDRATION_NUMBER:"));
            let exact = hydrate_projected_rows(&fixture.state, &admitted, &receipt, &[row(1)], None).unwrap();
            assert_eq!(exact[0].fields["request_time"], json!(1.25));
            assert_eq!(exact[0].raw, records[1]);
        });
    }
}
