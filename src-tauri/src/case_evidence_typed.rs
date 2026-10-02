//! Native-only typed decoding of already admitted exact Values. serde_json's
//! optional raw_value feature recognizes a private object key in Value's own
//! Deserialize implementation. Dynamic user objects must use this literal
//! visitor instead. No legacy model or global serde behavior is changed.
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use std::{collections::BTreeMap, fmt};

struct Literal(Value);
impl<'de> Deserialize<'de> for Literal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        deserialize_value(d).map(Self)
    }
}
/// Call only after the native reader has admitted depth, nodes and owned bytes.
pub(crate) fn deserialize_value<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    struct LiteralVisitor;
    impl<'de> Visitor<'de> for LiteralVisitor {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_unit<E>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }
        fn visit_none<E>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }
        fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
            Ok(Value::Bool(v))
        }
        fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
            Ok(Value::Number(v.into()))
        }
        fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
            Ok(Value::Number(v.into()))
        }
        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
            Number::from_f64(v)
                .map(Value::Number)
                .ok_or_else(|| E::custom("nonfinite JSON number"))
        }
        fn visit_str<E>(self, v: &str) -> Result<Value, E> {
            Ok(Value::String(v.into()))
        }
        fn visit_string<E>(self, v: String) -> Result<Value, E> {
            Ok(Value::String(v))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
            let mut values = Vec::new();
            while let Some(Literal(value)) = a.next_element()? {
                values.push(value);
            }
            Ok(Value::Array(values))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
            let mut values = Map::new();
            while let Some((key, Literal(value))) = a.next_entry::<String, Literal>()? {
                if values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("CASE_EVIDENCE_DUPLICATE_KEY"));
                }
            }
            Ok(Value::Object(values))
        }
    }
    d.deserialize_any(LiteralVisitor)
}
pub(crate) fn deserialize_values<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    Vec::<Literal>::deserialize(d).map(|v| v.into_iter().map(|v| v.0).collect())
}
fn deserialize_map<'de, D: Deserializer<'de>>(d: D) -> Result<Map<String, Value>, D::Error> {
    match deserialize_value(d)? {
        Value::Object(value) => Ok(value),
        _ => Err(serde::de::Error::custom("expected a JSON object")),
    }
}
fn deserialize_optional_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Option::<Literal>::deserialize(d).map(|v| v.map(|v| v.0))
}

#[derive(Deserialize)]
#[serde(
    remote = "crate::model::DerivedOriginal",
    tag = "state",
    content = "value",
    rename_all = "snake_case"
)]
enum OriginalDef {
    Missing,
    Present(#[serde(deserialize_with = "deserialize_value")] Value),
}
#[derive(Deserialize)]
struct Original(#[serde(with = "OriginalDef")] crate::model::DerivedOriginal);
fn deserialize_originals<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, crate::model::DerivedOriginal>, D::Error> {
    BTreeMap::<String, Original>::deserialize(d)
        .map(|v| v.into_iter().map(|(key, value)| (key, value.0)).collect())
}
// Remote derives construct the real production types. Missing required fields,
// defaults and enum validation remain typed; adding a model field requires this
// native adapter to be updated at compile time.
#[derive(Deserialize)]
#[serde(remote = "crate::model::Event")]
struct EventDef {
    id: usize,
    #[serde(default)]
    event_ref: String,
    #[serde(default)]
    evidence_provenance: Option<crate::analysis_visibility::EvidenceProvenance>,
    #[serde(default)]
    parse_status: String,
    timestamp: Option<i64>,
    source: String,
    level: String,
    code: String,
    name: String,
    description: String,
    message: String,
    raw: String,
    #[serde(default, deserialize_with = "deserialize_map")]
    fields: Map<String, Value>,
    #[serde(default)]
    derived_diagnostics: Vec<crate::model::DerivedDiagnostic>,
    #[serde(default, deserialize_with = "deserialize_originals")]
    derived_originals: BTreeMap<String, crate::model::DerivedOriginal>,
}
pub(crate) fn event_from_value(value: Value) -> Result<crate::model::Event, serde_json::Error> {
    EventDef::deserialize(value)
}

#[derive(Deserialize)]
#[serde(
    remote = "crate::analysis_context::Config",
    rename_all = "camelCase",
    deny_unknown_fields
)]
struct ConfigDef {
    #[serde(default, deserialize_with = "deserialize_values")]
    derived_fields: Vec<Value>,
    #[serde(default)]
    references: Vec<crate::analysis_context::ReferenceDescriptor>,
}
#[derive(Deserialize)]
#[serde(
    remote = "crate::analysis_context::Snapshot",
    rename_all = "camelCase",
    deny_unknown_fields
)]
struct SnapshotDef {
    schema_version: u32,
    case_id: String,
    analysis_id: String,
    config_revision: u64,
    visibility_revision: u64,
    #[serde(with = "ConfigDef")]
    config: crate::analysis_context::Config,
    #[serde(default)]
    migration_diagnostics: Vec<crate::analysis_context::Diagnostic>,
    #[serde(default, deserialize_with = "deserialize_optional_value")]
    legacy_raw: Option<Value>,
}
pub(crate) fn deserialize_snapshot<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<crate::analysis_context::Snapshot, D::Error> {
    SnapshotDef::deserialize(d)
}
pub(crate) fn snapshot_from_value(
    value: Value,
) -> Result<crate::analysis_context::Snapshot, serde_json::Error> {
    deserialize_snapshot(value)
}
#[derive(Deserialize)]
struct Snapshot(#[serde(with = "SnapshotDef")] crate::analysis_context::Snapshot);
pub(crate) fn deserialize_snapshots<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Vec<crate::analysis_context::Snapshot>, D::Error> {
    Vec::<Snapshot>::deserialize(d).map(|v| v.into_iter().map(|v| v.0).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn literal_fixture() -> Value {
        // Token-first maps test silent single-key reinterpretation as well as
        // sibling errors. Deliberately nonalphabetic order detects reordering.
        let text = r#"{"z":{"$serde_json::private::RawValue":"1"},"a":{"$serde_json::private::RawValue":"1","sibling":2},"array":[{"$serde_json::private::RawValue":"null"},{"$serde_json::private::Number":"18446744073709551615"}],"fraction":2.547114365375239e-8,"integer":18446744073709551615,"float":1.0,"zero":-0.0}"#;
        let raw = super::super::RawJson::checked(text).unwrap();
        super::super::decode::MetadataDecoder::new()
            .value(raw)
            .unwrap()
    }
    fn snapshot() -> crate::analysis_context::Snapshot {
        crate::analysis_context::Snapshot {
            schema_version: 1,
            case_id: "c".into(),
            analysis_id: "a".into(),
            config_revision: 0,
            visibility_revision: 0,
            config: crate::analysis_context::Config {
                derived_fields: vec![literal_fixture()],
                references: vec![],
            },
            migration_diagnostics: vec![],
            legacy_raw: Some(literal_fixture()),
        }
    }
    #[test]
    fn event_dynamic_objects_and_originals_are_literal_with_typed_numbers() {
        let mut event = crate::model::Event::empty();
        event.fields.insert("payload".into(), literal_fixture());
        event.derived_originals.insert(
            "payload".into(),
            crate::model::DerivedOriginal::Present(literal_fixture()),
        );
        event
            .derived_originals
            .insert("absent".into(), crate::model::DerivedOriginal::Missing);
        let text = serde_json::to_string(&event).unwrap();
        let decoded = super::super::materialize_event(
            &text,
            super::super::preflight_envelope(&text).unwrap(),
        )
        .unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), text);
        assert!(decoded.fields["payload"]["z"].is_object());
        assert_eq!(
            decoded.fields["payload"]["fraction"]
                .as_f64()
                .unwrap()
                .to_bits(),
            0x3e5b597464455d8a
        );
        assert!(decoded.fields["payload"]["float"]
            .as_number()
            .unwrap()
            .is_f64());
        assert_eq!(
            decoded.fields["payload"]["zero"]
                .as_f64()
                .unwrap()
                .to_bits(),
            (-0.0f64).to_bits()
        );
    }
    #[test]
    fn native_event_adapter_keeps_required_fields_defaults_and_enum_validation() {
        let original = serde_json::to_value(crate::model::Event::empty()).unwrap();
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove("id");
        assert!(event_from_value(value).is_err());
        let mut value = original.clone();
        value["fields"] = Value::Null;
        assert!(event_from_value(value).is_err());
        let mut value = original.clone();
        value["derived_originals"] = serde_json::json!({"x":{"state":"invented","value":1}});
        assert!(event_from_value(value).is_err());
        let mut value = original;
        for key in [
            "fields",
            "derived_originals",
            "derived_diagnostics",
            "event_ref",
            "parse_status",
            "evidence_provenance",
        ] {
            value.as_object_mut().unwrap().remove(key);
        }
        let decoded = event_from_value(value).unwrap();
        assert!(decoded.fields.is_empty() && decoded.derived_originals.is_empty());
        assert!(decoded.event_ref.is_empty() && decoded.parse_status.is_empty());
    }
    #[test]
    fn native_context_dynamic_objects_survive_typed_conversion() {
        let expected = snapshot();
        let text = serde_json::to_string(&expected).unwrap();
        let raw = super::super::RawJson::checked(&text).unwrap();
        let value = super::super::decode::MetadataDecoder::new()
            .value(raw)
            .unwrap();
        let result = snapshot_from_value(value).unwrap();
        assert_eq!(serde_json::to_string(&result).unwrap(), text);
        assert_eq!(
            result.config.derived_fields[0]["fraction"]
                .as_f64()
                .unwrap()
                .to_bits(),
            0x3e5b597464455d8a
        );
    }
    #[test]
    fn original_content_before_discriminator_keeps_literal_object() {
        let text = r#"{"x":{"value":{"$serde_json::private::RawValue":"1","sibling":2},"state":"present"}}"#;
        let raw = super::super::RawJson::checked(text).unwrap();
        let value = super::super::decode::MetadataDecoder::new()
            .value(raw)
            .unwrap();
        let originals = deserialize_originals(value).unwrap();
        let crate::model::DerivedOriginal::Present(value) = &originals["x"] else {
            panic!("present original");
        };
        assert_eq!(value["$serde_json::private::RawValue"], "1");
        assert_eq!(value["sibling"], 2);
    }
    #[test]
    fn native_view_and_receipt_preserve_literal_metadata_and_contexts() {
        use super::super::*;
        let text = r#"{"evidenceViewVersion":1,"store":{"storeId":"s","epoch":"e","revision":"1"},"active":"c","cases":[{"id":"c","note":{"$serde_json::private::RawValue":"1"},"array":[{"$serde_json::private::RawValue":"1","x":2}],"fraction":2.547114365375239e-8}],"caseEvidence":[],"diagnostics":[]}"#;
        let document = parse_view_document(text).unwrap();
        assert!(document.cases[0]["note"].is_object());
        assert_eq!(document.cases[0]["array"][0]["x"], 2);
        assert_eq!(
            document.cases[0]["fraction"].as_f64().unwrap().to_bits(),
            0x3e5b597464455d8a
        );
        let stamp = document.store;
        let receipt = SaveViewReceipt {
            request_id: "r".into(),
            committed_store: stamp.clone(),
            current_store: stamp.clone(),
            evidence: vec![],
            analysis_contexts: vec![snapshot()],
            case_evidence: vec![],
            replayed: false,
            reconcile_required: false,
        };
        let expected = serde_json::to_string(&receipt).unwrap();
        let entry = receipts::Entry {
            version: 1,
            kind: receipts::Kind::Save,
            expected_store: Some(stamp.clone()),
            publication_store: stamp.identity(),
            value: serde_json::to_value(&receipt).unwrap(),
        };
        let encoded = serde_json::to_string(&entry).unwrap();
        let raw = RawJson::checked(&encoded).unwrap();
        let value = decode::MetadataDecoder::new().value(raw).unwrap();
        let entry: receipts::Entry = serde_json::from_value(value).unwrap();
        let decoded: SaveViewReceipt = serde_json::from_value(entry.value).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), expected);
    }
}
