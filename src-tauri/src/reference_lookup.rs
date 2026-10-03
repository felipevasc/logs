//! First runtime tier for Case-owned reference enrichment. Preparation streams
//! a verified projection into a strictly capped immutable byte table. Event
//! evaluation never opens SQLite, reads a file, or consults the active Case.
use crate::{analysis_context::ReferenceDescriptor, model::Event, reference_store, sources};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

pub(crate) const VERSION: &str = "reference-lookup-1";
pub(crate) const MAX_RETAINED_BYTES: usize = 8 << 20;
pub(crate) const MAX_RETAINED_ROWS: usize = 100_000;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreparationError {
    Unavailable { reference_id: String },
    RetainedBytes,
    RetainedRows,
    Cancelled,
    Invalid(String),
}
impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable { .. } => f.write_str("Os bytes da referência não estão disponíveis neste Caso."),
            Self::RetainedBytes => f.write_str("As consultas à referência excedem 8 MiB de chaves, valores e índices retidos por Caso. Reduza as referências usadas."),
            Self::RetainedRows => f.write_str("As consultas à referência excedem 100.000 linhas retidas por Caso. Reduza as referências usadas."),
            Self::Cancelled => f.write_str("Preparação da referência cancelada."),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}
impl From<String> for PreparationError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}
fn preparation_error(error: reference_store::Error, reference_id: &str) -> PreparationError {
    match error {
        reference_store::Error::Unavailable => PreparationError::Unavailable {
            reference_id: reference_id.into(),
        },
        reference_store::Error::Cancelled => PreparationError::Cancelled,
        error => PreparationError::Invalid(error.to_string()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Definition {
    pub schema_version: u32,
    pub reference_id: String,
    pub keys: Vec<KeyBinding>,
    pub value_column: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KeyBinding {
    pub reference_column: String,
    pub source_field: String,
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}
impl Definition {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || !valid_text(&self.reference_id, 256)
            || !valid_text(&self.value_column, 4096)
            || self.keys.is_empty()
            || self.keys.len() > 16
            || self.keys.iter().any(|key| {
                !valid_text(&key.reference_column, 4096) || !valid_text(&key.source_field, 4096)
            })
            || self
                .keys
                .iter()
                .map(|key| &key.reference_column)
                .collect::<HashSet<_>>()
                .len()
                != self.keys.len()
        {
            return Err("Definição de consulta à referência inválida.".into());
        }
        Ok(())
    }
}

/// Existing wire clients may retain an exact first-key source alias and empty
/// rules/steps. They cannot combine extraction/transforms with this branch.
pub(crate) fn parse_definition(value: &Value) -> Result<Option<Definition>, String> {
    let Some(raw) = value.get("lookup") else {
        return Ok(None);
    };
    let definition: Definition = serde_json::from_value(raw.clone())
        .map_err(|_| "Definição de consulta à referência inválida.".to_string())?;
    definition.validate()?;
    let object = value.as_object().ok_or("Campo de referência inválido.")?;
    if object
        .keys()
        .any(|key| !["id", "name", "source", "rules", "steps", "lookup"].contains(&key.as_str()))
        || value
            .get("source")
            .is_some_and(|source| source.as_str() != Some(definition.keys[0].source_field.as_str()))
        || ["rules", "steps"].iter().any(|key| {
            value
                .get(*key)
                .is_some_and(|values| !values.as_array().is_some_and(Vec::is_empty))
        })
    {
        return Err(
            "Uma consulta à referência não pode conter regras, transformações ou outra fonte."
                .into(),
        );
    }
    Ok(Some(definition))
}

fn descriptor<'a>(
    definition: &Definition,
    references: &'a [ReferenceDescriptor],
) -> Result<&'a ReferenceDescriptor, String> {
    definition.validate()?;
    let found = references
        .iter()
        .filter(|reference| reference.id == definition.reference_id)
        .collect::<Vec<_>>();
    let [reference] = found.as_slice() else {
        return Err("A referência não pertence a esta configuração do Caso.".into());
    };
    if reference.format != "jsonl"
        || reference.duplicate_policy != "reject"
        || !reference.columns.contains(&definition.value_column)
        || definition.keys.len() != reference.key_columns.len()
        || reference.key_columns.iter().any(|column| {
            !definition
                .keys
                .iter()
                .any(|key| &key.reference_column == column)
        })
    {
        return Err("As chaves ou a coluna de saída não correspondem à referência do Caso.".into());
    }
    Ok(reference)
}
pub(crate) fn validate_reference(
    definition: &Definition,
    references: &[ReferenceDescriptor],
) -> Result<(), String> {
    descriptor(definition, references).map(|_| ())
}

#[derive(Clone, Copy)]
struct Entry {
    key_start: u32,
    key_len: u32,
    value_start: u32,
    value_len: u32,
}
struct Table {
    version: reference_store::Version,
    bytes: Box<[u8]>,
    entries: Box<[Entry]>,
    _lease: reference_store::PortableSource,
}
impl Table {
    fn retained_bytes(&self) -> usize {
        self.bytes.len() + self.entries.len() * std::mem::size_of::<Entry>()
    }
    fn key(&self, entry: &Entry) -> &[u8] {
        &self.bytes[entry.key_start as usize..(entry.key_start + entry.key_len) as usize]
    }
    fn lookup(&self, key: &[u8]) -> Result<Option<Value>, reference_store::Error> {
        let Ok(index) = self
            .entries
            .binary_search_by(|entry| self.key(entry).cmp(key))
        else {
            return Ok(None);
        };
        let entry = &self.entries[index];
        reference_store::interpreted_value(
            &self.bytes[entry.value_start as usize..(entry.value_start + entry.value_len) as usize],
            self.version.store_version,
        )
        .map(Some)
    }
}

#[derive(Clone)]
pub(crate) struct Compiled {
    pub definition: Definition,
    prepared: Option<Arc<Table>>,
}
impl Compiled {
    /// No file I/O: safe during immutable admission capture and validation.
    pub(crate) fn new(definition: Definition) -> Self {
        Self {
            definition,
            prepared: None,
        }
    }
    pub(crate) fn version(&self) -> Option<&reference_store::Version> {
        self.prepared.as_ref().map(|table| &table.version)
    }
    pub(crate) fn evaluate(&self, event: &Event) -> Result<Option<Value>, String> {
        let table = self
            .prepared
            .as_ref()
            .ok_or("A referência ainda não foi preparada para esta consulta.")?;
        let mut keys = Vec::with_capacity(self.definition.keys.len());
        for key in &self.definition.keys {
            let name = &key.source_field;
            let canonical = [
                "id",
                "event_ref",
                "timestamp",
                "source",
                "level",
                "code",
                "name",
                "description",
                "message",
                "raw",
            ]
            .contains(&name.as_str());
            let value = match (!canonical).then(|| event.fields.get(name)).flatten() {
                Some(value) => Cow::Borrowed(value),
                None => match event.col_ref(name) {
                    Some(value) => {
                        if value.len() > 64 << 10 {
                            return Err("A chave da referência excede 64 KiB.".into());
                        }
                        Cow::Owned(Value::String(value.into_owned()))
                    }
                    None => return Ok(None),
                },
            };
            keys.push(value);
        }
        let keys = keys.iter().map(Cow::as_ref).collect::<Vec<_>>();
        let key = reference_store::encode_key(&keys, self.definition.keys.len())
            .map_err(|error| error.to_string())?;
        table.lookup(&key).map_err(|error| error.to_string())
    }
}

/// Call in a worker before any record evaluation. Failed preparation returns
/// no partial bindings and never changes the input definitions or Case/source.
pub(crate) fn prepare_fields(
    root: &Path,
    owner: &reference_store::Owner,
    references: &[ReferenceDescriptor],
    fields: &[sources::CompiledDerived],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<sources::CompiledDerived>, String> {
    prepare_fields_detailed(root, owner, references, fields, cancelled)
        .map_err(|error| error.to_string())
}
pub(crate) fn prepare_fields_detailed(
    root: &Path,
    owner: &reference_store::Owner,
    references: &[ReferenceDescriptor],
    fields: &[sources::CompiledDerived],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<sources::CompiledDerived>, PreparationError> {
    prepare_with_limits(
        root,
        owner,
        references,
        fields,
        cancelled,
        MAX_RETAINED_BYTES,
        MAX_RETAINED_ROWS,
    )
}
fn prepare_with_limits(
    root: &Path,
    owner: &reference_store::Owner,
    references: &[ReferenceDescriptor],
    fields: &[sources::CompiledDerived],
    cancelled: &dyn Fn() -> bool,
    byte_limit: usize,
    row_limit: usize,
) -> Result<Vec<sources::CompiledDerived>, PreparationError> {
    let mut projected: HashMap<(String, String), Arc<Table>> = HashMap::new();
    let mut retained = 0usize;
    let mut rows = 0usize;
    let mut prepared = Vec::with_capacity(fields.len());
    for field in fields {
        if cancelled() {
            return Err(PreparationError::Cancelled);
        }
        let mut field = field.clone();
        if let Some(lookup) = &mut field.lookup {
            let reference = descriptor(&lookup.definition, references)?;
            let identity = (reference.id.clone(), lookup.definition.value_column.clone());
            let table = if let Some(table) = projected.get(&identity) {
                Arc::clone(table)
            } else {
                let reader = reference_store::open(root, owner, reference, cancelled)
                    .map_err(|error| preparation_error(error, &reference.id))?;
                if reader.prepared().row_count > row_limit.saturating_sub(rows) as u64 {
                    return Err(PreparationError::RetainedRows);
                }
                let mut bytes = Vec::new();
                let mut entries: Vec<Entry> = Vec::new();
                let mut byte_limit_hit = false;
                let mut row_limit_hit = false;
                reader
                    .visit_selected(&lookup.definition.value_column, cancelled, |key, value| {
                        if rows >= row_limit {
                            row_limit_hit = true;
                            return Err(reference_store::Error::Limit(
                                "100.000 linhas de referência retidas por Caso",
                            ));
                        }
                        let cost = key
                            .len()
                            .checked_add(value.len())
                            .and_then(|n| n.checked_add(std::mem::size_of::<Entry>()))
                            .ok_or(reference_store::Error::Limit("memória das referências"))?;
                        if cost > byte_limit.saturating_sub(retained) {
                            byte_limit_hit = true;
                            return Err(reference_store::Error::Limit(
                                "8 MiB de chaves, valores e índices de referência retidos por Caso",
                            ));
                        }
                        if let Some(previous) = entries.last() {
                            let old = &bytes[previous.key_start as usize
                                ..(previous.key_start + previous.key_len) as usize];
                            if old >= key {
                                return Err(reference_store::Error::Corrupt);
                            }
                        }
                        let key_start = bytes.len() as u32;
                        bytes.extend_from_slice(key);
                        let value_start = bytes.len() as u32;
                        bytes.extend_from_slice(value);
                        entries.push(Entry {
                            key_start,
                            key_len: key.len() as u32,
                            value_start,
                            value_len: value.len() as u32,
                        });
                        retained += cost;
                        rows += 1;
                        Ok(())
                    })
                    .map_err(|error| {
                        if byte_limit_hit {
                            PreparationError::RetainedBytes
                        } else if row_limit_hit {
                            PreparationError::RetainedRows
                        } else {
                            preparation_error(error, &reference.id)
                        }
                    })?;
                if entries.len() as u64 != reader.prepared().row_count {
                    return Err(PreparationError::Invalid(
                        reference_store::Error::Corrupt.to_string(),
                    ));
                }
                let lease = reader
                    .into_portable()
                    .map_err(|error| preparation_error(error, &reference.id))?;
                let table = Arc::new(Table {
                    version: lease.prepared().version.clone(),
                    bytes: bytes.into_boxed_slice(),
                    entries: entries.into_boxed_slice(),
                    _lease: lease,
                });
                projected.insert(identity, Arc::clone(&table));
                table
            };
            // This order is part of the canonical key encoding contract.
            lookup.definition.keys = reference
                .key_columns
                .iter()
                .map(|column| {
                    lookup
                        .definition
                        .keys
                        .iter()
                        .find(|key| &key.reference_column == column)
                        .expect("validated binding")
                        .clone()
                })
                .collect();
            lookup.prepared = Some(table);
        }
        prepared.push(field);
    }
    if cancelled() {
        return Err(PreparationError::Cancelled);
    }
    Ok(prepared)
}

pub(crate) fn retained_bytes(fields: &[sources::CompiledDerived]) -> usize {
    let mut seen = HashSet::new();
    fields
        .iter()
        .filter_map(|field| field.lookup.as_ref()?.prepared.as_ref())
        .filter(|table| seen.insert(Arc::as_ptr(table)))
        .map(|table| table.retained_bytes())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    fn fixture(
        root: &Path,
        case: &str,
        data: &[u8],
    ) -> (reference_store::Owner, ReferenceDescriptor) {
        let owner = reference_store::Owner {
            case_id: case.into(),
            analysis_id: "analysis".into(),
        };
        let descriptor = ReferenceDescriptor {
            interpretation_version: 1, schema_version: 1,
            id: "ref".into(),
            name: "Ref".into(),
            content_sha256: format!("{:x}", Sha256::digest(data)),
            format: "jsonl".into(),
            columns: vec!["a".into(), "b".into(), "value".into()],
            key_columns: vec!["a".into(), "b".into()],
            duplicate_policy: "reject".into(),
        };
        reference_store::prepare_jsonl(
            root,
            &owner,
            &descriptor,
            data,
            Default::default(),
            &|| false,
        )
        .unwrap();
        (owner, descriptor)
    }
    fn definition() -> Definition {
        Definition {
            schema_version: 1,
            reference_id: "ref".into(),
            keys: vec![
                KeyBinding {
                    reference_column: "b".into(),
                    source_field: "tenant".into(),
                },
                KeyBinding {
                    reference_column: "a".into(),
                    source_field: "key".into(),
                },
            ],
            value_column: "value".into(),
        }
    }
    fn field(name: &str) -> sources::CompiledDerived {
        sources::CompiledDerived {
            name: name.into(),
            source: String::new(),
            rules: Vec::new(),
            steps: Vec::new(),
            lookup: Some(Compiled::new(definition())),
        }
    }
    #[test]
    fn typed_projection_is_pinned_shared_and_usable_without_database_reads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Compiled>();
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"a\":1,\"b\":\"t\",\"value\":\"integer\"}\n{\"a\":1.0,\"b\":\"t\",\"value\":\"float\"}\n{\"a\":\"1\",\"b\":\"t\",\"value\":null}\n{\"a\":true,\"b\":\"t\",\"value\":{\"x\":2}}\n";
        let (owner, descriptor) = fixture(dir.path(), "case-a", data);
        let fields = [field("one"), field("two")];
        let prepared = prepare_fields(dir.path(), &owner, &[descriptor.clone()], &fields, &|| {
            false
        })
        .unwrap();
        assert_eq!(retained_bytes(&prepared), retained_bytes(&prepared[..1]));
        assert!(fields[0].lookup.as_ref().unwrap().version().is_none());
        // Remove our synthetic source/index after preparation: the immutable
        // request snapshot must no longer consult the disk store at all.
        #[cfg(unix)]
        std::fs::remove_dir_all(dir.path().join("references-v1")).unwrap();
        let lookup = prepared[0].lookup.as_ref().unwrap();
        for (key, expected) in [
            (json!(1), Some(json!("integer"))),
            (json!(1.0), Some(json!("float"))),
            (json!("1"), Some(Value::Null)),
            (json!(true), Some(json!({"x":2}))),
            (json!("missing"), None),
        ] {
            let mut event = Event::empty();
            event.fields.insert("key".into(), key);
            event.fields.insert("tenant".into(), json!("t"));
            assert_eq!(lookup.evaluate(&event).unwrap(), expected);
        }
        let mut event = Event::empty();
        event.fields.insert("key".into(), Value::Null);
        event.fields.insert("tenant".into(), json!("t"));
        assert!(lookup.evaluate(&event).is_err());
        event.fields.remove("key");
        assert_eq!(lookup.evaluate(&event).unwrap(), None);
        assert_eq!(lookup.version().unwrap().owner, owner);
    }
    #[test]
    fn preparation_rejects_other_owner_limits_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, descriptor) = fixture(
            dir.path(),
            "case-a",
            b"{\"a\":1,\"b\":\"t\",\"value\":\"owner-a\"}\n",
        );
        let fields = [field("one")];
        let foreign = reference_store::Owner {
            case_id: "case-b".into(),
            analysis_id: owner.analysis_id.clone(),
        };
        assert!(matches!(
            prepare_fields_detailed(
                dir.path(),
                &foreign,
                &[descriptor.clone()],
                &fields,
                &|| false
            ),
            Err(PreparationError::Unavailable { .. })
        ));
        assert!(matches!(
            prepare_with_limits(
                dir.path(),
                &owner,
                &[descriptor.clone()],
                &fields,
                &|| false,
                1,
                100
            ),
            Err(PreparationError::RetainedBytes)
        ));
        assert!(matches!(
            prepare_with_limits(
                dir.path(),
                &owner,
                &[descriptor.clone()],
                &fields,
                &|| false,
                1000,
                0
            ),
            Err(PreparationError::RetainedRows)
        ));
        assert!(matches!(
            prepare_fields_detailed(dir.path(), &owner, &[descriptor], &fields, &|| true),
            Err(PreparationError::Cancelled)
        ));
        assert!(matches!(
            prepare_fields_detailed(dir.path(), &owner, &[], &fields, &|| false),
            Err(PreparationError::Invalid(_))
        ));
        assert!(fields[0].lookup.as_ref().unwrap().version().is_none());
    }
    #[test]
    fn wire_branch_and_key_mapping_are_unambiguous() {
        let lookup = definition();
        assert_eq!(
            parse_definition(&json!({"id":"d","name":"field","lookup":lookup})).unwrap(),
            Some(lookup.clone())
        );
        let mut raw =
            json!({"name":"field","source":"tenant","rules":[],"steps":[],"lookup":lookup});
        assert!(parse_definition(&raw).is_ok());
        raw["source"] = json!("other");
        assert!(parse_definition(&raw).is_err());
        raw["source"] = json!("tenant");
        raw["steps"] = json!(["parse_json"]);
        assert!(parse_definition(&raw).is_err());
        let mut invalid = definition();
        invalid.keys[1].reference_column = "b".into();
        assert!(invalid.validate().is_err());
        assert!(validate_reference(&definition(), &[]).is_err());
    }

    #[test]
    fn lookup_pipeline_preserves_provenance_conflicts_and_typed_children() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, descriptor) = fixture(
            dir.path(),
            "case-a",
            b"{\"a\":1,\"b\":\"t\",\"value\":{\"payload\":\"eA==\",\"empty\":null}}\n",
        );
        let input = sources::CompiledDerived {
            name: "key".into(),
            source: "raw_key".into(),
            rules: Vec::new(),
            steps: vec![
                crate::field_transform::Step::Base64Decode,
                crate::field_transform::Step::ParseJson,
            ],
            lookup: None,
        };
        let decode = sources::CompiledDerived {
            name: "decoded".into(),
            source: "asset.payload".into(),
            rules: Vec::new(),
            steps: vec![crate::field_transform::Step::Base64Decode],
            lookup: None,
        };
        let fields = prepare_fields(
            dir.path(),
            &owner,
            &[descriptor],
            &[input, field("asset"), decode],
            &|| false,
        )
        .unwrap();
        let mut event = Event::empty();
        event.fields.insert("raw_key".into(), json!("MQ=="));
        event.fields.insert("tenant".into(), json!("t"));
        let original = event.clone();
        sources::apply_derived(&mut event, &fields);
        assert_eq!(event.fields["key"], json!(1));
        assert_eq!(event.fields["asset.empty"], Value::Null);
        assert_eq!(event.fields["decoded"], json!("x"));
        assert!(event.fields["asset"].is_object());
        assert!(event.derived_diagnostics.is_empty());
        let saved = serde_json::to_string(&event).unwrap();
        let mut evidence: Event = serde_json::from_str(&saved).unwrap();
        sources::apply_derived(&mut evidence, &fields);
        assert_eq!(serde_json::to_string(&evidence).unwrap(), saved);
        sources::apply_derived(&mut evidence, &[]);
        assert_eq!(evidence.fields, original.fields);
        let mut collision = original;
        collision
            .fields
            .insert("asset.payload".into(), json!("raw"));
        sources::apply_derived(&mut collision, &fields);
        assert_eq!(collision.fields["asset.payload"], json!("raw"));
        assert!(!collision.fields.contains_key("asset"));
        assert_eq!(collision.derived_diagnostics[0].code, "target_conflict");
        assert_eq!(serde_json::to_string(&event).unwrap(), saved);
    }

    #[test]
    fn budget_is_aggregate_but_identical_projections_share_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"a\":1,\"b\":\"t\",\"value\":\"one\"}\n";
        let (owner, first) = fixture(dir.path(), "case-a", data);
        let mut second = first.clone();
        second.id = "ref-2".into();
        reference_store::prepare_jsonl(
            dir.path(),
            &owner,
            &second,
            &data[..],
            Default::default(),
            &|| false,
        )
        .unwrap();
        let references = [first, second];
        let original = field("one");
        let one = prepare_fields(
            dir.path(),
            &owner,
            &references,
            &[original.clone()],
            &|| false,
        )
        .unwrap();
        let limit = retained_bytes(&one);
        let shared = prepare_with_limits(
            dir.path(),
            &owner,
            &references,
            &[original.clone(), field("shared")],
            &|| false,
            limit,
            1,
        )
        .unwrap();
        assert_eq!(retained_bytes(&shared), limit);
        let mut other = field("other");
        other.lookup.as_mut().unwrap().definition.reference_id = "ref-2".into();
        assert!(prepare_with_limits(
            dir.path(),
            &owner,
            &references,
            &[original.clone(), other.clone()],
            &|| false,
            limit,
            100
        )
        .is_err());
        assert!(prepare_with_limits(
            dir.path(),
            &owner,
            &references,
            &[original, other],
            &|| false,
            1000,
            1
        )
        .is_err());
    }
}
