//! Bounded analytical interpretation of preserved, syntax-checked JSON spans.
//! Numeric leaves use Rust's exact integer/binary64 parsing; the global serde
//! parser and its feature set remain unchanged.
use super::{raw_json::RawJson, ENVELOPE_BYTES, EVENT_OWNED_BYTES};
use serde_json::{Map, Number, Value};
const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 32;
const LIMIT: &str = "CASE_EVIDENCE_MATERIALIZATION_LIMIT";
const UNSUPPORTED: &str = "CASE_EVIDENCE_UNSUPPORTED_RECORD";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordPlan {
    pub encoded_bytes: usize,
    pub materialization_credit: usize,
    pub nodes: usize,
}
struct Budget {
    nodes: usize,
    bytes: usize,
    node_limit: usize,
    byte_limit: usize,
}
impl Budget {
    fn event(bytes: usize) -> Self {
        Self {
            nodes: 0,
            bytes,
            node_limit: MAX_NODES,
            byte_limit: EVENT_OWNED_BYTES,
        }
    }
    fn metadata(bytes: usize) -> Self {
        Self {
            nodes: 0,
            bytes,
            node_limit: 131_072,
            byte_limit: 64 << 20,
        }
    }
    fn node(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH || self.nodes >= self.node_limit {
            return Err(LIMIT.into());
        }
        self.nodes += 1;
        self.add(256)?;
        if self.nodes % 256 == 0 {
            crate::operations::check()?;
        }
        Ok(())
    }
    fn add(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.byte_limit)
            .ok_or(LIMIT)?;
        Ok(())
    }
    fn string(&mut self, encoded_bytes: usize) -> Result<(), String> {
        self.add(
            encoded_bytes
                .checked_mul(2)
                .and_then(|n| n.checked_add(64))
                .ok_or(LIMIT)?,
        )
    }
}
fn inspect(raw: RawJson<'_>, depth: usize, budget: &mut Budget) -> Result<(), String> {
    budget.node(depth)?;
    match raw.kind() {
        b'{' => {
            let mut fields = raw.members()?;
            while let Some(field) = fields.next()? {
                budget.string(field.key.len())?;
                inspect(field.value, depth + 1, budget)?;
            }
        }
        b'[' => {
            let mut rows = raw.elements()?;
            while let Some(row) = rows.next()? {
                inspect(row, depth + 1, budget)?;
            }
        }
        b'"' => budget.string(raw.get().len())?,
        _ => (),
    }
    Ok(())
}
pub(crate) fn preflight_envelope(envelope: &str) -> Result<RecordPlan, String> {
    crate::operations::check()?;
    if envelope.len() > ENVELOPE_BYTES {
        return Err(LIMIT.into());
    }
    let raw = RawJson::checked(envelope)?;
    if raw.kind() != b'{' {
        return Err(UNSUPPORTED.into());
    }
    // Credit encoded scratch before any key or string is decoded. The scanner
    // and syntax-only validation do not construct a JSON tree.
    let scratch = envelope
        .len()
        .checked_mul(2)
        .and_then(|n| n.checked_add(1024))
        .filter(|n| *n <= EVENT_OWNED_BYTES)
        .ok_or(LIMIT)?;
    let mut budget = Budget::event(scratch);
    inspect(raw, 0, &mut budget)?;
    Ok(RecordPlan {
        encoded_bytes: envelope.len(),
        materialization_credit: budget.bytes,
        nodes: budget.nodes,
    })
}
fn exact_raw(raw: RawJson<'_>, depth: usize, budget: &mut Budget) -> Result<Value, String> {
    budget.node(depth)?;
    Ok(match raw.kind() {
        b'{' => {
            let mut values = Map::new();
            let mut fields = raw.members()?;
            while let Some(field) = fields.next()? {
                budget.string(field.key.len())?;
                let key = field.key()?;
                if values.contains_key(&key) {
                    return Err("CASE_EVIDENCE_DUPLICATE_KEY".into());
                }
                values.insert(key, exact_raw(field.value, depth + 1, budget)?);
            }
            Value::Object(values)
        }
        b'[' => {
            let mut values = Vec::new();
            let mut rows = raw.elements()?;
            while let Some(row) = rows.next()? {
                values.push(exact_raw(row, depth + 1, budget)?);
            }
            Value::Array(values)
        }
        b'"' => {
            budget.string(raw.get().len())?;
            Value::String(serde_json::from_str(raw.get()).map_err(|e| e.to_string())?)
        }
        b't' => Value::Bool(true),
        b'f' => Value::Bool(false),
        b'n' => Value::Null,
        _ => {
            let text = raw.get();
            let number = if text == "-0" || text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
                Number::from_f64(text.parse::<f64>().map_err(|_| UNSUPPORTED)?)
                    .ok_or(UNSUPPORTED)?
            } else if text.starts_with('-') {
                Number::from(text.parse::<i64>().map_err(|_| UNSUPPORTED)?)
            } else {
                Number::from(text.parse::<u64>().map_err(|_| UNSUPPORTED)?)
            };
            Value::Number(number)
        }
    })
}
fn exact(text: &str, depth: usize, budget: &mut Budget) -> Result<Value, String> {
    exact_raw(RawJson::checked(text)?, depth, budget)
}
/// Runtime must reserve the entire plan credit before calling. The plan is
/// recomputed to reject undercharging; unsupported views keep raw authority.
pub(crate) fn materialize_event(
    envelope: &str,
    plan: RecordPlan,
) -> Result<crate::model::Event, String> {
    if preflight_envelope(envelope)? != plan {
        return Err(LIMIT.into());
    }
    let value = exact(envelope, 0, &mut Budget::event(1024))?;
    let event = super::typed::event_from_value(value)
        .map_err(|e| format!("{UNSUPPORTED}: {e}"))?;
    if crate::query::event_payload_bytes(&event) > plan.materialization_credit {
        return Err(LIMIT.into());
    }
    crate::operations::check()?;
    Ok(event)
}
/// Readiness classification is independent from exact native recovery metadata.
pub(crate) fn metadata_transport_safe(value: &Value) -> Result<(), String> {
    fn visit(value: &Value, depth: usize, left: &mut usize) -> Result<(), String> {
        if depth > MAX_DEPTH || *left == 0 {
            return Err(LIMIT.into());
        }
        *left -= 1;
        if *left % 256 == 0 {
            crate::operations::check()?;
        }
        match value {
            Value::Number(n) => {
                let safe = if n.is_f64() {
                    n.as_f64()
                        .is_some_and(|v| v.is_finite() && v.fract() != 0.0)
                } else {
                    n.as_u64().is_some_and(|v| v <= 9_007_199_254_740_991)
                        || n.as_i64().is_some_and(|v| {
                            (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&v)
                        })
                };
                if !safe {
                    return Err("CASE_EVIDENCE_METADATA_NUMBER".into());
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, depth + 1, left)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    visit(value, depth + 1, left)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    visit(value, 0, &mut 131_072)
}
pub(super) fn preflight_metadata(raw: RawJson<'_>) -> Result<RecordPlan, String> {
    let mut budget = Budget::metadata(1024);
    budget.add(raw.get().len().checked_mul(2).ok_or(LIMIT)?)?;
    inspect(raw, 0, &mut budget)?;
    Ok(RecordPlan {
        encoded_bytes: raw.get().len(),
        materialization_credit: budget.bytes,
        nodes: budget.nodes,
    })
}
/// Callers reserve the full reported credit before decoding. This shared
/// reader retains integer/float kind and binary64 bits without global features.
pub(crate) fn preflight_value(raw: RawJson<'_>) -> Result<RecordPlan, String> {
    crate::operations::check()?;
    preflight_metadata(raw)
}
pub(crate) fn materialize_value(raw: RawJson<'_>, plan: RecordPlan) -> Result<Value, String> {
    if preflight_value(raw)? != plan {
        return Err(LIMIT.into());
    }
    MetadataDecoder::new().value(raw)
}
pub(crate) struct ExactValue {
    value: Value,
    credit: crate::case_work_budget::Lease,
}
impl ExactValue {
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
    /// Any caller-owned growth must be charged before mutation.
    pub(crate) fn value_mut(&mut self) -> &mut Value {
        &mut self.value
    }
    pub(crate) fn into_parts(self) -> (Value, crate::case_work_budget::Lease) {
        (self.value, self.credit)
    }
}
/// Ordinary reference/projection values use the conservative 4MiB owned
/// interpretation ceiling. Opaque envelope preservation is a separate path.
pub(crate) fn parse_exact_value(text: &str) -> Result<ExactValue, String> {
    let raw = RawJson::checked(text)?;
    let plan = preflight_value(raw)?;
    if plan.materialization_credit > EVENT_OWNED_BYTES {
        return Err(LIMIT.into());
    }
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        plan.materialization_credit,
    )?;
    let value = materialize_value(raw, plan)?;
    Ok(ExactValue { value, credit })
}
pub(super) struct MetadataDecoder {
    budget: Budget,
}
impl MetadataDecoder {
    pub(super) fn new() -> Self {
        Self {
            budget: Budget::metadata(1024),
        }
    }
    pub(super) fn value(&mut self, raw: RawJson<'_>) -> Result<Value, String> {
        // The bounded structural pass precedes recursive numeric inspection.
        let mut planned = Budget {
            nodes: self.budget.nodes,
            bytes: self.budget.bytes,
            node_limit: self.budget.node_limit,
            byte_limit: self.budget.byte_limit,
        };
        planned.add(raw.get().len().checked_mul(2).ok_or(LIMIT)?)?;
        inspect(raw, 0, &mut planned)?;
        self.budget
            .add(raw.get().len().checked_mul(2).ok_or(LIMIT)?)?;
        exact_raw(raw, 0, &mut self.budget)
    }
}
/// The inner JSON text crosses Tauri as a String, avoiding its ordinary float
/// deserializer before this exact boundary. No global parser feature changes.
pub(crate) fn parse_view_document(text: &str) -> Result<super::CaseViewDocument, String> {
    crate::operations::check()?;
    let raw = RawJson::checked_with_limit(text, super::VIEW_DOCUMENT_BYTES)?;
    let value = MetadataDecoder::new().value(raw)?;
    metadata_transport_safe(&value)?;
    serde_json::from_value(value).map_err(|e| format!("CASE_EVIDENCE_VIEW: {e}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inner_document_string_preserves_fraction_and_rejects_outer_stamp_mismatch() {
        let text = r#"{"evidenceViewVersion":1,"store":{"storeId":"s","epoch":"e","revision":"7"},"active":"c","cases":[{"id":"c","weight":2.547114365375239e-8}],"caseEvidence":[],"diagnostics":[]}"#;
        let document = parse_view_document(text).unwrap();
        assert_eq!(
            document.cases[0]["weight"].as_f64().unwrap().to_bits(),
            0x3e5b597464455d8a
        );
        let request = super::super::SaveViewRequest {
            request_id: "request".into(),
            expected_store: super::super::StoreStamp {
                store_id: "s".into(),
                epoch: "e".into(),
                revision: "8".into(),
            },
            document_json: text.into(),
        };
        assert!(request.parse_document().is_err());
    }
    #[test]
    fn incompatible_metadata_is_not_silently_emitted_as_an_editable_value() {
        for text in ["1.0", "-0.0", "9007199254740993"] {
            let raw = RawJson::checked(text).unwrap();
            assert!(
                metadata_transport_safe(&MetadataDecoder::new().value(raw).unwrap())
                    .unwrap_err()
                    .contains("CASE_EVIDENCE_METADATA_NUMBER")
            );
        }
    }
    #[test]
    fn literal_private_keys_and_fraction_survive_actual_event_materialization() {
        let mut event = crate::model::Event::empty();
        event.fields.insert(
            "literal".into(),
            serde_json::json!({"$serde_json::private::RawValue":"1","sibling":2}),
        );
        event.fields.insert(
            "fraction".into(),
            Value::Number(Number::from_f64(f64::from_bits(0x3e5b597464455d8a)).unwrap()),
        );
        let envelope = serde_json::to_string(&event).unwrap();
        let decoded = materialize_event(&envelope, preflight_envelope(&envelope).unwrap()).unwrap();
        assert_eq!(decoded.fields["literal"], event.fields["literal"]);
        assert_eq!(
            decoded.fields["fraction"].as_f64().unwrap().to_bits(),
            0x3e5b597464455d8a
        );
    }
    #[test]
    fn exact_values_keep_integer_float_negative_zero_and_unknown_structure() {
        let text = r#"{"wide":18446744073709551615,"integer":1,"float":1.0,"exponent":1e0,"zero":-0.0,"shortZero":-0,"fraction":51.248178375505404,"unknown":[true,null,"x"]}"#;
        let value = exact(text, 0, &mut Budget::event(1024)).unwrap();
        assert_eq!(value["wide"].as_u64(), Some(u64::MAX));
        assert!(value["integer"].is_u64());
        assert!(value["float"].is_f64());
        assert!(value["exponent"].is_f64());
        for key in ["zero", "shortZero"] {
            assert_eq!(value[key].as_f64().unwrap().to_bits(), (-0.0f64).to_bits());
        }
        assert_eq!(
            value["fraction"].as_f64().unwrap().to_bits(),
            "51.248178375505404".parse::<f64>().unwrap().to_bits()
        );
        let witness = exact("2.547114365375239e-8", 0, &mut Budget::event(1024)).unwrap();
        assert_eq!(witness.as_f64().unwrap().to_bits(), 0x3e5b597464455d8a);
    }
    #[test]
    fn duplicate_keys_and_unrepresentable_numbers_are_not_sanitized_for_analysis() {
        for text in [
            r#"{"x":1,"x":2}"#,
            r#"{"x":1,"\u0078":2}"#,
            r#"{"x":18446744073709551616}"#,
            r#"{"x":1e999}"#,
        ] {
            assert!(exact(text, 0, &mut Budget::event(1024)).is_err());
        }
    }
    #[test]
    fn preflight_declines_large_tree_before_event_allocation() {
        let text = format!(
            "{{\"fields\":{{\"wide\":[{}]}}}}",
            std::iter::repeat_n("0", MAX_NODES + 1)
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(preflight_envelope(&text).is_err());
        let mut deep = "0".to_string();
        for _ in 0..40 {
            deep = format!("[{deep}]");
        }
        assert!(preflight_envelope(&format!("{{\"x\":{deep}}}")).is_err());
    }
}
