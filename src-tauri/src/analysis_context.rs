//! Case-owned configuration. The investigation body and this authoritative
//! context share a database, but have independent compare-and-swap revisions.
//! Query admission/visibility membership are deliberately separate consumers.
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, path::Path};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 4 << 20;
const MIGRATED: &str = "case-analysis-v1";

pub(crate) const MAX_DERIVED_RULES: usize = 1024;
pub(crate) const REGEX_SET_BYTES: usize = 16 << 20;
pub(crate) const DFA_SET_BYTES: usize = 4 << 20;
pub(crate) const REGEX_RULE_BYTES: usize = 2 << 20;
pub(crate) const DFA_RULE_BYTES: usize = 512 << 10;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegexLimits {
    pub size_limit: usize,
    pub dfa_size_limit: usize,
}

#[cfg(test)]
mod lookup_validation_tests {
    use super::*;
    use serde_json::json;
    fn reference() -> ReferenceDescriptor {
        ReferenceDescriptor {interpretation_version:1,schema_version:1,id:"ref".into(),name:"Ref".into(),content_sha256:"a".repeat(64),format:"jsonl".into(),columns:vec!["key".into(),"value".into()],key_columns:vec!["key".into()],duplicate_policy:"reject".into()}
    }
    fn lookup(source: &str) -> Value {
        json!({"name":"asset","lookup":{"schemaVersion":1,"referenceId":"ref","keys":[{"referenceColumn":"key","sourceField":source}],"valueColumn":"value"}})
    }
    #[test]
    fn lookup_inputs_participate_in_order_cycles_and_reference_validation() {
        let config = Config {derived_fields:vec![lookup("decoded.key"),json!({"name":"decoded","source":"message","steps":["parse_json"]})],references:vec![reference()]};
        validate(&config).unwrap();assert_eq!(definition_order(&config.derived_fields).unwrap(),vec![1,0]);
        let mut cycle = config.clone();cycle.derived_fields[1]["source"] = json!("asset");assert!(validate(&cycle).is_err());
        let mut missing = config.clone();missing.references.clear();assert!(validate(&missing).is_err());
        let mut mixed = config;mixed.derived_fields[0]["rules"] = json!([{"pattern":".*"}]);assert!(validate(&mixed).is_err());
    }

    #[test]
    fn typed_target_validation_matches_evaluation_and_preserves_legacy_regex() {
        for name in ["LEVEL".to_string(),"Event_Ref".to_string(),"x".repeat(513),"ç".repeat(257)] {
            let mut raw = lookup("message");raw["name"] = json!(name);
            assert!(validate(&Config {derived_fields:vec![raw.clone()],references:vec![reference()]}).is_err());
            let steps = vec![crate::field_transform::Step::UrlDecode];
            let transform = json!({"name":name,"source":"message","steps":steps});
            assert!(validate(&Config {derived_fields:vec![transform],references:vec![]}).is_err());
            let definition = crate::reference_lookup::parse_definition(&raw).unwrap().unwrap();
            for field in [
                crate::sources::CompiledDerived {name:name.clone(),source:"message".into(),rules:vec![],steps:steps.clone(),lookup:None},
                crate::sources::CompiledDerived {name:name.clone(),source:"message".into(),rules:vec![],steps:vec![],lookup:Some(crate::reference_lookup::Compiled::new(definition))},
            ] {
                let mut event = crate::model::Event::empty();event.message = "original".into();
                crate::sources::apply_derived(&mut event,&[field]);
                assert!(event.fields.is_empty());assert_eq!(event.message,"original");
                assert_eq!(event.derived_diagnostics[0].code,"reserved_target");
            }
            let legacy = json!({"name":name,"source":"message","pattern":"(.*)"});
            validate(&Config {derived_fields:vec![legacy],references:vec![]}).unwrap();
        }
        let name = "ç".repeat(256); // Exactly 512 UTF-8 bytes.
        let raw = json!({"name":name,"source":"message","steps":["url_decode"]});
        validate(&Config {derived_fields:vec![raw],references:vec![]}).unwrap();
        let mut event = crate::model::Event::empty();event.message = "%2Fok".into();
        let field = crate::sources::CompiledDerived {name:name.clone(),source:"message".into(),rules:vec![],steps:vec![crate::field_transform::Step::UrlDecode],lookup:None};
        crate::sources::apply_derived(&mut event,&[field]);
        assert_eq!(event.fields[&name],json!("/ok"));assert!(event.derived_diagnostics.is_empty());
    }
}
pub(crate) fn regex_limits(total_rules: usize) -> Result<RegexLimits, String> {
    if total_rules > MAX_DERIVED_RULES {
        return Err("O Caso excede 1.024 regras de campos derivados.".into());
    }
    let count = total_rules.max(1);
    Ok(RegexLimits {
        size_limit: REGEX_RULE_BYTES.min(REGEX_SET_BYTES / count),
        dfa_size_limit: DFA_RULE_BYTES.min(DFA_SET_BYTES / count),
    })
}
fn definition_rule_count(definitions: &[Value]) -> Result<usize, String> {
    definitions.iter().try_fold(0usize, |total, definition| {
        let count = definition
            .get("rules")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let count = if count == 0
            && definition
                .get("pattern")
                .and_then(Value::as_str)
                .is_some_and(|p| !p.is_empty())
        {
            1
        } else {
            count
        };
        total
            .checked_add(count)
            .ok_or_else(|| "Contagem de regras excessiva.".to_string())
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Preserve regex-compatible definitions, including invalid migrated entries.
    /// Consumers must honor diagnostics; updates/imports validate every entry.
    #[serde(default)]
    pub derived_fields: Vec<Value>,
    #[serde(default)]
    pub references: Vec<ReferenceDescriptor>,
}

/// Declarative, content-versioned metadata only. This does not implement joins,
/// trust a foreign filesystem path, or claim that imported bytes are available.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceDescriptor {
    pub schema_version: u32,
    /// The descriptor schema stays v1; existing declarations keep their old
    /// numeric interpretation unless a native input explicitly declares v2.
    #[serde(
        default = "legacy_reference_interpretation",
        skip_serializing_if = "is_legacy_reference_interpretation"
    )]
    pub interpretation_version: u32,
    pub id: String,
    pub name: String,
    pub content_sha256: String,
    pub format: String,
    pub columns: Vec<String>,
    pub key_columns: Vec<String>,
    pub duplicate_policy: String,
}

fn legacy_reference_interpretation() -> u32 {
    1
}
fn is_legacy_reference_interpretation(version: &u32) -> bool {
    *version == 1
}

#[cfg(test)]
mod reference_interpretation_tests {
    use super::*;
    #[test]
    fn shared_interpretation_disclosures_are_bounded_and_deduplicated() {
        assert!(reference_interpretation_diagnostics([], true).is_empty());
        assert!(reference_interpretation_diagnostics([1,1], false).is_empty());
        let diagnostics = reference_interpretation_diagnostics([1,2,1,2,1], true);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, "reference_interpretation_legacy");
        assert_eq!(diagnostics[1].code, "reference_interpretation_exact");
    }
    #[test]
    fn legacy_descriptor_wire_is_unchanged_and_explicit_interpretation_is_bounded() {
        let original = serde_json::json!({"schemaVersion":1,"id":"r","name":"Reference","contentSha256":"a".repeat(64),"format":"jsonl","columns":["key","value"],"keyColumns":["key"],"duplicatePolicy":"reject"});
        let legacy: ReferenceDescriptor = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(legacy.interpretation_version, 1);
        assert_eq!(serde_json::to_value(&legacy).unwrap(), original);
        let mut exact = legacy.clone();
        exact.interpretation_version = 2;
        assert_eq!(
            serde_json::to_value(&exact).unwrap()["interpretationVersion"],
            2
        );
        validate(&Config {
            references: vec![exact.clone()],
            derived_fields: Vec::new(),
        })
        .unwrap();
        exact.interpretation_version = 3;
        assert!(validate(&Config {
            references: vec![exact],
            derived_fields: Vec::new()
        })
        .is_err());
    }
}


/// Shared, bounded disclosure for native record preservation and declared
/// reference interpretations. This does not change either numeric codec.
pub(crate) fn reference_interpretation_diagnostics(
    versions: impl IntoIterator<Item = u32>,
    native_records: bool,
) -> Vec<Diagnostic> {
    let (mut legacy, mut exact) = (false, false);
    for version in versions {
        legacy |= version == 1;
        exact |= version == 2;
    }
    let mut diagnostics = Vec::with_capacity(2);
    if native_records && legacy {
        diagnostics.push(Diagnostic {
            definition_index: None,
            code: "reference_interpretation_legacy".into(),
            message: "A interpretação v1 das referências foi preservada. Decimais importados diretamente no Caso usam leitura exata; chaves que dependiam do arredondamento legado podem diferir. Revise a política numérica antes de interpretar resultados sem correspondência.".into(),
        });
    }
    if exact {
        diagnostics.push(Diagnostic {
            definition_index: None,
            code: "reference_interpretation_exact".into(),
            message: "Os bytes e a interpretação numérica exata v2 declarada pelas referências foram preservados. Números já capturados pelo leitor legado mantêm seus valores anteriores e podem não corresponder a chaves decimais exatas. Não há conversão implícita entre interpretações.".into(),
        });
    }
    diagnostics
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    pub definition_index: Option<usize>,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub case_id: String,
    pub analysis_id: String,
    pub config_revision: u64,
    pub visibility_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub case_id: String,
    pub analysis_id: String,
    pub config_revision: u64,
    pub visibility_revision: u64,
    pub config: Config,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpretation: Option<crate::case_interpretation::Settings>,
    #[serde(default)]
    pub migration_diagnostics: Vec<Diagnostic>,
    /// Malformed legacy documents are retained verbatim (bounded), as well as
    /// the untouched original file. They are never interpreted as definitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_raw: Option<Value>,
}

/// Browser management metadata omits potentially large interpretation catalogs.
/// Authoritative snapshots and portable codecs still serialize the full value.
pub(crate) struct ManagementSnapshot<'a>(pub &'a Snapshot);
impl Serialize for ManagementSnapshot<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct View<'a> {
            schema_version: u32, case_id: &'a str, analysis_id: &'a str,
            config_revision: u64, visibility_revision: u64, config: &'a Config,
            migration_diagnostics: &'a [Diagnostic],
            #[serde(skip_serializing_if = "Option::is_none")]
            legacy_raw: Option<&'a Value>,
        }
        let snapshot = self.0;
        View { schema_version: snapshot.schema_version, case_id: &snapshot.case_id, analysis_id: &snapshot.analysis_id,
            config_revision: snapshot.config_revision, visibility_revision: snapshot.visibility_revision, config: &snapshot.config,
            migration_diagnostics: &snapshot.migration_diagnostics, legacy_raw: snapshot.legacy_raw.as_ref() }.serialize(serializer)
    }
}
pub(crate) fn serialize_management_snapshot<S: serde::Serializer>(snapshot: &Snapshot, serializer: S) -> Result<S::Ok, S::Error> {
    ManagementSnapshot(snapshot).serialize(serializer)
}
pub(crate) fn serialize_management_snapshots<S: serde::Serializer>(snapshots: &[Snapshot], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut sequence = serializer.serialize_seq(Some(snapshots.len()))?;
    for snapshot in snapshots { sequence.serialize_element(&ManagementSnapshot(snapshot))?; }
    sequence.end()
}
pub(crate) fn serialize_management_snapshot_refs<S: serde::Serializer>(snapshots: &[&Snapshot], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut sequence = serializer.serialize_seq(Some(snapshots.len()))?;
    for snapshot in snapshots { sequence.serialize_element(&ManagementSnapshot(snapshot))?; }
    sequence.end()
}

impl Snapshot {
    pub fn identity(&self) -> Identity {
        Identity {
            case_id: self.case_id.clone(),
            analysis_id: self.analysis_id.clone(),
            config_revision: self.config_revision,
            visibility_revision: self.visibility_revision,
        }
    }
    fn empty(case_id: &str) -> Self {
        Self {
            schema_version: VERSION,
            case_id: case_id.into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
            config: Config::default(),
            interpretation: Some(Default::default()),
            migration_diagnostics: Vec::new(),
            legacy_raw: None,
        }
    }
}

pub(crate) fn schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS case_analysis(case_id TEXT PRIMARY KEY, body TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn validate_snapshot_size(snapshot: &Snapshot) -> Result<(), String> {
    bounded(snapshot).map(|_| ())
}

fn bounded(value: &impl Serialize) -> Result<String, String> {
    let body = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if body.len() > MAX_BYTES {
        return Err("A configuração do Caso excede o limite de 4 MiB.".into());
    }
    Ok(body)
}
fn required_text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .ok_or_else(|| format!("Campo derivado com {key} ausente ou inválido."))
}
fn validate_definition(value: &Value, limits: Option<RegexLimits>) -> Result<(), String> {
    let name = required_text(value, "name")?;
    if name.starts_with('@')
        || [
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
            "arquivo",
            "caminho",
        ]
        .contains(&name)
    {
        return Err("O nome do campo derivado é reservado para metadados do evento.".into());
    }
    if crate::reference_lookup::parse_definition(value)?.is_some() {
        if !crate::field_transform::valid_typed_target(name) {
            return Err(crate::field_transform::TARGET_NAME_ERROR.into());
        }
        return Ok(());
    }
    required_text(value, "source")?;
    let steps = value
        .get("steps")
        .map(|steps| serde_json::from_value::<Vec<crate::field_transform::Step>>(steps.clone()))
        .transpose()
        .map_err(|_| "Transformações do campo derivado inválidas.".to_string())?
        .unwrap_or_default();
    if steps.len() > 8 {
        return Err("O campo derivado excede oito transformações.".into());
    }
    if !steps.is_empty() && !crate::field_transform::valid_typed_target(name) {
        return Err(crate::field_transform::TARGET_NAME_ERROR.into());
    }
    let rules = match value.get("rules") {
        Some(Value::Array(rules)) if rules.len() <= 256 => rules.clone(),
        Some(_) => return Err("Regras do campo derivado inválidas.".into()),
        None => Vec::new(),
    };
    let rules = if rules.is_empty()
        && value
            .get("pattern")
            .and_then(Value::as_str)
            .is_some_and(|p| !p.is_empty())
    {
        vec![value.clone()]
    } else {
        rules
    };
    if rules.is_empty() && steps.is_empty() {
        return Err("O campo derivado precisa de regras ou transformações.".into());
    }
    for rule in rules {
        let pattern = required_text(&rule, "pattern")?;
        if let Some(limits) = limits {
            regex::RegexBuilder::new(pattern)
                .size_limit(limits.size_limit)
                .dfa_size_limit(limits.dfa_size_limit)
                .build()
                .map_err(|_| "Expressão regular inválida ou excessiva para o orçamento compartilhado do Caso.".to_string())?;
        }
        if rule
            .get("template")
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err("Modelo de extração inválido.".into());
        }
        if let Some(filter) = rule.get("filter").filter(|f| !f.is_null()) {
            required_text(filter, "column")?;
            required_text(filter, "op")?;
            if filter.get("value").is_some_and(|v| !v.is_string())
                || filter
                    .get("value2")
                    .is_some_and(|v| !v.is_string() && !v.is_null())
            {
                return Err("Condição do campo derivado inválida.".into());
            }
        }
    }
    Ok(())
}
/// Stable topological order. Dotted children conservatively depend on their
/// declared parent; exact declared field names take precedence. Only direct
/// filter-column references are dependencies, never interpreted query text.
pub(crate) fn definition_order(definitions: &[Value]) -> Result<Vec<usize>, String> {
    if definitions.len() > 256 {
        return Err("Configuração excede 256 campos derivados.".into());
    }
    let names: std::collections::HashMap<_, _> = definitions
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            value
                .get("name")
                .and_then(Value::as_str)
                .map(|name| (name, index))
        })
        .collect();
    let dependency = |mut field: &str| loop {
        if let Some(index) = names.get(field) {
            return Some(*index);
        }
        let Some((parent, _)) = field.rsplit_once('.') else {
            return None;
        };
        field = parent;
    };
    let mut edges = vec![Vec::new(); definitions.len()];
    for (index, definition) in definitions.iter().enumerate() {
        if let Some(found) = definition
            .get("source")
            .and_then(Value::as_str)
            .and_then(dependency)
        {
            edges[index].push(found);
        }
        for key in definition.get("lookup").and_then(|lookup| lookup.get("keys")).and_then(Value::as_array).into_iter().flatten() {
            if let Some(found) = key.get("sourceField").and_then(Value::as_str).and_then(dependency) {
                if !edges[index].contains(&found) { edges[index].push(found); }
            }
        }
        let rules = definition.get("rules").and_then(Value::as_array);
        for rule in rules
            .into_iter()
            .flatten()
            .chain(std::iter::once(definition))
        {
            if let Some(found) = rule
                .get("filter")
                .and_then(|filter| filter.get("column"))
                .and_then(Value::as_str)
                .and_then(dependency)
            {
                if !edges[index].contains(&found) {
                    edges[index].push(found);
                }
            }
        }
    }
    fn visit(
        index: usize,
        edges: &[Vec<usize>],
        state: &mut [u8],
        ordered: &mut Vec<usize>,
    ) -> Result<(), String> {
        match state[index] {
            1 => {
                return Err(
                    "Dependência circular entre campos derivados; revise as fontes e condições."
                        .into(),
                )
            }
            2 => return Ok(()),
            _ => {}
        }
        state[index] = 1;
        for &dependency in &edges[index] {
            visit(dependency, edges, state, ordered)?;
        }
        state[index] = 2;
        ordered.push(index);
        Ok(())
    }
    let mut ordered = Vec::with_capacity(definitions.len());
    let mut state = vec![0; definitions.len()];
    for index in 0..definitions.len() {
        visit(index, &edges, &mut state, &mut ordered)?;
    }
    Ok(ordered)
}
fn diagnostics(config: &Config) -> Vec<Diagnostic> {
    let budget = definition_rule_count(&config.derived_fields).and_then(regex_limits);
    let limits = budget.as_ref().ok().copied();
    let mut names = HashSet::new();
    let mut issues: Vec<_> = config
        .derived_fields
        .iter()
        .enumerate()
        .filter_map(|(index, definition)| {
            let error = validate_definition(definition, limits)
                .and_then(|_| {
                    if let Some(lookup) = crate::reference_lookup::parse_definition(definition)? {
                        crate::reference_lookup::validate_reference(&lookup, &config.references)?;
                    }
                    Ok(())
                }).err().or_else(|| {
                let name = definition.get("name").and_then(Value::as_str)?;
                (!names.insert(name)).then(|| "Nome de campo derivado duplicado no Caso.".into())
            });
            error.map(|message| Diagnostic {
                definition_index: Some(index),
                code: "invalid_legacy_definition".into(),
                message,
            })
        })
        .collect();
    if let Err(message) = budget {
        issues.push(Diagnostic {
            definition_index: None,
            code: "legacy_regex_budget".into(),
            message,
        });
    }
    if let Err(message) = definition_order(&config.derived_fields) {
        issues.push(Diagnostic {
            definition_index: None,
            code: "invalid_legacy_dependencies".into(),
            message,
        });
    }
    issues
}
pub(crate) fn validate(config: &Config) -> Result<(), String> {
    bounded(config)?;
    if config.derived_fields.len() > 256 || config.references.len() > 128 {
        return Err("A configuração excede o limite de campos ou referências do Caso.".into());
    }
    regex_limits(definition_rule_count(&config.derived_fields)?)?;
    if let Some(issue) = diagnostics(config).first() {
        return Err(match issue.definition_index {
            Some(index) => format!("Campo derivado {}: {}", index + 1, issue.message),
            None => issue.message.clone(),
        });
    }
    let mut ids = HashSet::new();
    for reference in &config.references {
        let columns: HashSet<_> = reference.columns.iter().collect();
        if reference.schema_version != VERSION
            || ![1, 2].contains(&reference.interpretation_version)
            || reference.id.trim().is_empty()
            || reference.id.len() > 256
            || reference.name.trim().is_empty()
            || reference.name.len() > 4096
            || !ids.insert(&reference.id)
            || reference.content_sha256.len() != 64
            || !reference
                .content_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || !["csv", "tsv", "json", "jsonl"].contains(&reference.format.as_str())
            || reference.columns.is_empty()
            || reference.columns.len() > 1024
            || columns.len() != reference.columns.len()
            || reference
                .columns
                .iter()
                .any(|c| c.trim().is_empty() || c.len() > 4096)
            || reference.key_columns.is_empty()
            || reference.key_columns.len() > 16
            || reference
                .key_columns
                .iter()
                .any(|key| !columns.contains(key))
            || reference.key_columns.iter().collect::<HashSet<_>>().len()
                != reference.key_columns.len()
            || reference.duplicate_policy != "reject"
        {
            return Err("Descritor de arquivo de referência inválido ou não suportado.".into());
        }
    }
    Ok(())
}
fn legacy(dir: &Path) -> Result<(Config, Vec<Diagnostic>, Option<Value>), String> {
    use std::io::Read;
    let file = match std::fs::File::open(dir.join("derived_fields.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Config::default(), Vec::new(), None))
        }
        Err(error) => {
            return Err(format!(
                "Não foi possível preservar os campos derivados legados: {error}"
            ))
        }
    };
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err(
            "Arquivo legado de campos derivados excede 4 MiB; o original foi preservado.".into(),
        );
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        "Arquivo legado de campos derivados não é UTF-8; o original foi preservado.".to_string()
    })?;
    legacy_value(serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
}
fn legacy_value(value:Value)->Result<(Config,Vec<Diagnostic>,Option<Value>),String>{
    match value {
        Value::Array(derived_fields) => {
            let config = Config {
                derived_fields,
                references: Vec::new(),
            };
            let mut issues = diagnostics(&config);
            if config.derived_fields.len() > 256 {
                issues.push(Diagnostic {
                    definition_index: None,
                    code: "legacy_definition_limit".into(),
                    message: "Configuração legada excede 256 campos; revise-a antes de ativar."
                        .into(),
                });
            }
            Ok((config, issues, None))
        }
        other => Ok((
            Config::default(),
            vec![Diagnostic {
                definition_index: None,
                code: "invalid_legacy_document".into(),
                message:
                    "Arquivo legado inválido; conteúdo e original preservados para recuperação."
                        .into(),
            }],
            Some(other),
        )),
    }
}
pub(crate) fn write(tx: &Transaction<'_>, snapshot: &Snapshot) -> Result<(), String> {
    let body = bounded(snapshot)?;
    tx.execute("INSERT INTO case_analysis(case_id,body) VALUES(?1,?2) ON CONFLICT(case_id) DO UPDATE SET body=excluded.body", params![snapshot.case_id, body]).map_err(|e| e.to_string())?;
    Ok(())
}
fn native_context_owner(conn: &Connection, case_id: &str) -> Result<Option<String>, String> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_evidence_cases')",
        [], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    if !exists { return Ok(None); }
    let owner: Option<String> = conn.query_row(
        "SELECT CASE WHEN octet_length(analysis_id)=36 THEN analysis_id END FROM native_evidence_cases WHERE case_id=?1",
        [case_id], |row| row.get(0),
    ).optional().map_err(|e| e.to_string())?;
    if owner.as_ref().is_some_and(|owner| uuid::Uuid::parse_str(owner).is_err()) {
        return Err("Identidade nativa da configuração inválida.".into());
    }
    Ok(owner)
}
/// A protected native owner uses exact numeric metadata interpretation. Legacy
/// contexts deliberately retain their existing effective parser until adoption
/// canonicalizes that Snapshot under the verified recovery transaction.
pub(crate) fn read(conn: &Connection, case_id: &str) -> Result<Snapshot, String> {
    read_snapshot_leased(conn,case_id).map(|(snapshot, _credit)|snapshot)
}
/// Retain the returned exact-decoder credit through native compilation or IPC.
/// A direct connection is wrapped in one read transaction so adoption cannot
/// change legacy/native interpretation between the owner and body reads.
pub(crate) fn read_snapshot_leased(conn: &Connection, case_id: &str)
    -> Result<(Snapshot, Option<crate::case_work_budget::Lease>), String>
{
    if conn.is_autocommit() {
        let tx = conn.unchecked_transaction().map_err(|e|e.to_string())?;
        let result = read_snapshot_credited(&tx,case_id)?;
        tx.commit().map_err(|e|e.to_string())?;
        Ok(result)
    } else { read_snapshot_credited(conn,case_id) }
}
fn read_snapshot_credited(conn: &Connection, case_id: &str) -> Result<(Snapshot, Option<crate::case_work_budget::Lease>), String> {
    let native_owner = native_context_owner(conn, case_id)?;
    let size: usize = conn.query_row(
        "SELECT octet_length(a.body) FROM case_analysis a JOIN cases c ON c.id=a.case_id WHERE a.case_id=?1",
        [case_id], |row| row.get(0),
    ).optional().map_err(|e| e.to_string())?
        .ok_or("Caso inexistente ou contexto ainda não inicializado.")?;
    if size > MAX_BYTES { return Err("A configuração do Caso excede o limite de 4 MiB.".into()); }
    // Includes SQLite TEXT + Rust input before the exact tree admission. The
    // returned Snapshot follows the caller's existing ownership budget.
    let mut credit = if native_owner.is_some() {
        Some(crate::case_cache::reserve_work(crate::case_work_budget::global(),
            size.checked_mul(3).and_then(|n| n.checked_add(64 << 10)).ok_or("Configuração excessiva.")?)?)
    } else { None };
    let body: String = conn.query_row(
        "SELECT CASE WHEN octet_length(body)=?2 THEN body END FROM case_analysis WHERE case_id=?1",
        params![case_id,size], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    let snapshot: Snapshot = if let Some(credit) = credit.as_mut() {
        let raw = crate::case_evidence::RawJson::checked(&body)?;
        let plan = crate::case_evidence::preflight_value(raw)?;
        credit.merge(crate::case_cache::reserve_work(crate::case_work_budget::global(), plan.materialization_credit)?)?;
        crate::case_evidence::native_snapshot_from_value(crate::case_evidence::materialize_value(raw,plan)?)
            .map_err(|e| e.to_string())?
    } else {
        serde_json::from_str(&body).map_err(|_| {
            "Configuração persistida do Caso está inválida; preserve o banco para recuperação.".to_string()
        })?
    };
    if snapshot.case_id != case_id
        || snapshot.schema_version != VERSION
        || uuid::Uuid::parse_str(&snapshot.analysis_id).is_err()
        || native_owner.as_ref().is_some_and(|owner| owner != &snapshot.analysis_id)
    {
        return Err("Identidade da configuração do Caso inválida ou não suportada.".into());
    }
    Ok((snapshot, credit))
}

/// Called under the case-store write transaction BEFORE adding ordinary cases,
/// or AFTER importing the pre-existing legacy cases.json for the first time.
pub(crate) fn initialize(tx: &Transaction<'_>, dir: &Path) -> Result<(), String> {
    let done: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata WHERE key=?1)",
            [MIGRATED],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if done {
        return crate::case_interpretation::initialize(tx, dir);
    }
    #[derive(Deserialize)]
    struct StoredContext {
        #[serde(rename = "analysisContext")]
        context: Option<Value>,
    }
    let mut legacy_values = None;
    // Decode only embedded configuration, ignoring evidence arrays. Process one
    // case body at a time instead of retaining another copy of all saved cases.
    let mut stmt = tx.prepare("SELECT id,body FROM cases WHERE NOT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=cases.id)").map_err(|e|e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let body: String = row.get(1).map_err(|e| e.to_string())?;
        let stored: StoredContext = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut snapshot = imported(&id, stored.context.as_ref())?;
        if stored.context.is_none() {
            if legacy_values.is_none() {
                legacy_values = Some(legacy(dir)?);
            }
            let (config, issues, raw) = legacy_values.as_ref().unwrap();
            snapshot.config = config.clone();
            snapshot.migration_diagnostics = issues.clone();
            snapshot.legacy_raw = raw.clone();
        }
        if stored.context.as_ref().and_then(|value| value.get("interpretation")).is_none() {
            snapshot.migration_diagnostics.retain(|diagnostic| diagnostic.code != "import_interpretation_defaults");
            let (settings, diagnostics) = crate::case_interpretation::local_legacy_for(tx, dir)?;
            snapshot.interpretation = Some(settings);
            snapshot.migration_diagnostics.extend(diagnostics);
        }
        write(tx, &snapshot)?;
    }
    drop(rows);
    drop(stmt);
    tx.execute("INSERT INTO metadata(key,value) VALUES(?1,'1')", [MIGRATED])
        .map_err(|e| e.to_string())?;
    crate::case_interpretation::initialize(tx, dir)
}
fn imported(case_id: &str, embedded: Option<&Value>) -> Result<Snapshot, String> {
    let mut local = Snapshot::empty(case_id);
    if let Some(value) = embedded {
        bounded(value)?;
        let foreign: Snapshot = serde_json::from_value(value.clone())
            .map_err(|_| "Configuração importada do Caso inválida.".to_string())?;
        if foreign.schema_version != VERSION {
            return Err("Configuração importada exige uma versão mais recente.".into());
        }
        if foreign.visibility_revision != 0 {
            return Err("Este JSON não preserva os lotes de exclusão/restauração do Caso. O original foi preservado; use o arquivo portátil da investigação quando disponível.".into());
        }
        validate(&foreign.config)?;
        // New local identity: foreign revisions and source visibility membership
        // cannot be applied to a different analysis without an explicit import.
        local.interpretation = foreign.interpretation;
        crate::case_interpretation::import_defaults(&mut local);
        // An explicit duplicate/import cannot turn unavailable legacy settings
        // into apparently valid defaults by dropping their repair diagnostics.
        for diagnostic in foreign.migration_diagnostics {
            if (diagnostic.code.starts_with("legacy_interpretation_unavailable") || diagnostic.code == "import_interpretation_defaults")
                && !local.migration_diagnostics.contains(&diagnostic) {
                local.migration_diagnostics.push(diagnostic);
            }
        }
        local.config = foreign.config;
        local.legacy_raw = foreign.legacy_raw;
        if local.legacy_raw.is_some() {
            local.migration_diagnostics.push(Diagnostic {
                definition_index: None,
                code: "imported_legacy_recovery".into(),
                message:
                    "Conteúdo legado foi preservado para recuperação; não é uma definição ativa."
                        .into(),
            });
        }
    }
    if let Some(settings) = &local.interpretation { settings.validate()?; }
    Ok(local)
}
/// Prepare an owner before evidence disk work, without creating a Case/context
/// row. Publication still requires the existing transactional insert hook.
pub(crate) fn prepare_new_case(case_id: &str) -> Result<Snapshot, String> {
    if case_id.is_empty() || case_id.len() > 4096 {
        return Err("Identificador de Caso inválido ou excessivo.".into());
    }
    Ok(Snapshot::empty(case_id))
}

/// The recovery/adoption caller chooses this only for the initial legacy
/// migration. Existing contexts are retained verbatim; new ordinary Cases use
/// prepare_new_case and never inherit global definitions implicitly.
pub(crate) fn prepare_legacy_case(
    case_id: &str,
    embedded: Option<&Value>,
    dir: &Path,
) -> Result<Snapshot, String> {
    if case_id.is_empty() || case_id.len() > 4096 {
        return Err("Identificador de Caso inválido ou excessivo.".into());
    }
    let mut snapshot = imported(case_id, embedded)?;
    if embedded.and_then(|value| value.get("interpretation")).is_none() {
        let (settings, diagnostics) = crate::case_interpretation::local_legacy(dir);
        snapshot.interpretation = Some(settings);
        snapshot.migration_diagnostics.retain(|diagnostic| diagnostic.code != "import_interpretation_defaults");
        snapshot.migration_diagnostics.extend(diagnostics);
    }
    if embedded.is_none() {
        let (config, diagnostics, raw) = legacy(dir)?;
        snapshot.config = config;
        snapshot.migration_diagnostics = diagnostics;
        snapshot.legacy_raw = raw;
        let (settings, diagnostics) = crate::case_interpretation::local_legacy(dir);
        snapshot.interpretation = Some(settings);
        snapshot.migration_diagnostics.extend(diagnostics);
    }
    Ok(snapshot)
}
/// Native adoption supplies a Value only after its bounded raw span and tree
/// admission. Malformed legacy text is supplied as Value::String verbatim.
pub(crate) fn prepare_legacy_value(case_id:&str,value:Value)->Result<Snapshot,String>{
    let mut snapshot=prepare_new_case(case_id)?;
    let (config,diagnostics,raw)=legacy_value(value)?;
    snapshot.config=config;snapshot.migration_diagnostics=diagnostics;snapshot.legacy_raw=raw;
    Ok(snapshot)
}
/// Called only inside verified native adoption after the complete copied
/// context identities were compared and the writer permit was installed.
/// Canonicalize the already-read legacy effective Snapshot, never reinterpret
/// its original numeric text through the new native codec.
pub(crate) fn canonicalize_native_adoption(tx:&Transaction<'_>, snapshot:&Snapshot)->Result<(),String>{
    let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",[&snapshot.case_id],|row|row.get(0)).map_err(|e|e.to_string())?;
    if !present {return Err("Caso inexistente durante adoção nativa.".into());}
    write(tx,snapshot)
}
/// Only the portable ledger importer may preserve a nonzero visibility baseline.
/// Its payloads are rebuilt/verified before this new identity can be published.
pub(crate) fn prepare_portable_snapshot(
    case_id: &str,
    foreign: &Snapshot,
) -> Result<Snapshot, String> {
    bounded(foreign)?;
    if case_id.is_empty()
        || case_id.len() > 4096
        || foreign.schema_version != VERSION
        || uuid::Uuid::parse_str(&foreign.analysis_id).is_err()
    {
        return Err("Identidade de Caso portátil inválida.".into());
    }
    validate(&foreign.config)?;
    let mut local = foreign.clone();
    crate::case_interpretation::import_defaults(&mut local);
    local.interpretation.as_ref().unwrap().validate()?;
    local.case_id = case_id.into();
    local.analysis_id = uuid::Uuid::new_v4().to_string();
    Ok(local)
}
pub(crate) fn insert_portable_snapshot(
    tx: &Transaction<'_>,
    snapshot: &Snapshot,
) -> Result<Snapshot, String> {
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cases WHERE id=?1)",
            [&snapshot.case_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Err("Caso inexistente durante a publicação portátil.".into());
    }
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1)",
        [&snapshot.case_id], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    if current {
        // The new context may have been inserted by a sidecar before native
        // ownership is installed. Compare sealed canonical bytes first so an
        // exact fractional Snapshot never passes through the legacy parser.
        let expected = bounded(snapshot)?;
        let same: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1 AND body=?2)",
            params![snapshot.case_id,expected], |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        if same { return Ok(snapshot.clone()); }
        let current = read(tx, &snapshot.case_id)?;
        if current.analysis_id != snapshot.analysis_id {
            return Err("O Caso já possui outra configuração; importação recusada.".into());
        }
        return Ok(current);
    }
    write(tx, snapshot)?;
    Ok(snapshot.clone())
}
pub(crate) fn ensure_case(tx: &Transaction<'_>, case: &Value) -> Result<Option<Snapshot>, String> {
    let id = case["id"].as_str().ok_or("Caso sem identificador.")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists {
        return Ok(None);
    }
    let snapshot = imported(id, case.get("analysisContext"))?;
    write(tx, &snapshot)?;
    Ok(Some(snapshot))
}
pub(crate) fn attach(conn: &Connection, case: &mut Value) -> Result<(), String> {
    let id = case
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Caso sem identificador.")?;
    let snapshot = read(conn, id)?;
    case.as_object_mut().ok_or("Caso inválido.")?.insert(
        "analysisContext".into(),
        serde_json::to_value(snapshot).map_err(|e| e.to_string())?,
    );
    Ok(())
}

pub fn snapshot(case_id: &str) -> Result<Snapshot, String> {
    snapshot_at(&crate::config_dir(), case_id)
}
fn snapshot_at(dir: &Path, case_id: &str) -> Result<Snapshot, String> {
    let conn = crate::case_store::context_connection(dir)?;
    read(&conn, case_id)
}
/// Last committed active Case only. This never guesses an in-progress UI switch
/// or adopts a context for an identity-less request.
pub fn active_snapshot() -> Result<Option<Snapshot>, String> {
    active_snapshot_at(&crate::config_dir())
}
pub(crate) fn active_snapshot_at(dir: &Path) -> Result<Option<Snapshot>, String> {
    let conn = crate::case_store::context_connection(dir)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let active: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key='active'", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let id: Option<String> = match active {
        Some(value) => serde_json::from_str(&value)
            .map_err(|_| "Identificador do Caso ativo persistido está inválido.".to_string())?,
        None => None,
    };
    let snapshot = id.map(|id| read(&conn, &id)).transpose()?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(snapshot)
}

pub fn update(expected: &Identity, config: Config) -> Result<Snapshot, String> {
    update_at(&crate::config_dir(), expected, config)
}
fn update_at(dir: &Path, expected: &Identity, config: Config) -> Result<Snapshot, String> {
    update_at_inner(dir, expected, config, false)
}
/// The bulk browser codec is legacy-only. Its early command admission is
/// repeated under the writer transaction because adoption preserves Identity.
pub(crate) fn update_legacy_browser_at(
    dir: &Path,
    expected: &Identity,
    config: Config,
) -> Result<Snapshot, String> {
    update_at_inner(dir, expected, config, true)
}
fn update_at_inner(dir: &Path, expected: &Identity, config: Config, legacy_browser: bool) -> Result<Snapshot, String> {
    validate(&config)?;
    let mut conn = crate::case_store::context_connection(dir)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if legacy_browser {
        for table in ["native_evidence_cases", "native_evidence_protected", "case_recovery_protected"] {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table], |row| row.get(0),
            ).map_err(|e| e.to_string())?;
            if exists && tx.query_row::<bool, _, _>(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE case_id=?1)"),
                [&expected.case_id], |row| row.get(0),
            ).map_err(|e| e.to_string())? {
                return Err("CASE_CONTEXT_TYPED_EDIT_REQUIRED: Use os editores de campos derivados e referências para alterar a configuração deste Caso preservado.".into());
            }
        }
    }
    let mut current = read(&tx, &expected.case_id)?; // JOIN prevents resurrection after deletion.
    if current.analysis_id != expected.analysis_id
        || current.config_revision != expected.config_revision
    {
        return Err(
            "A configuração ou identidade do Caso mudou. Recarregue antes de salvar.".into(),
        );
    }
    current.config_revision = current
        .config_revision
        .checked_add(1)
        .ok_or("Revisão do Caso excedeu o limite.")?;
    current.config = config;
    current.migration_diagnostics.retain(|issue| issue.code.starts_with("legacy_interpretation_unavailable"));
    if native_context_owner(&tx, &expected.case_id)?.is_some() {
        current.migration_diagnostics.extend(reference_interpretation_diagnostics(
            current.config.references.iter().map(|reference| reference.interpretation_version),
            true,
        ));
    }
    // Preserve malformed legacy material for recovery even after a repaired edit.
    write(&tx, &current)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(current)
}

/// Executes only short ledger SQL while checking the complete admitted context.
/// Payload building/verification must finish before entering this transaction.
pub(crate) fn visibility_update_at<T>(
    dir: &Path,
    expected: &Identity,
    cancelled: &dyn Fn() -> bool,
    change: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<(Snapshot, T), String> {
    let mut conn = crate::case_store::context_connection(dir)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut current = read(&tx, &expected.case_id)?;
    if current.identity() != *expected {
        return Err("O contexto do Caso mudou; refaça a operação com a revisão atual.".into());
    }
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    let result = change(&tx)?;
    current.visibility_revision = current
        .visibility_revision
        .checked_add(1)
        .ok_or("Revisão de visibilidade excedeu o limite.")?;
    write(&tx, &current)?;
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    tx.commit().map_err(|e| e.to_string())?;
    // No cancellation check after durable publication. Command owners call
    // operations::commit() on this success before leaving their admission guard.
    Ok((current, result))
}

pub(crate) fn visibility_read_at<T>(
    dir: &Path,
    expected: &Identity,
    read_ledger: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    let conn = crate::case_store::context_connection(dir)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let current = read(&conn, &expected.case_id)?;
    if current.identity() != *expected {
        return Err("O contexto do Caso mudou; recarregue a visibilidade.".into());
    }
    let result = read_ledger(&conn)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("loginsight-analysis-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn save(&self, value: Value) -> Value {
            crate::case_store::save_at(&self.0, value).unwrap()
        }
        fn load(&self) -> Value {
            crate::case_store::load_at(&self.0).unwrap()
        }
        fn snapshot(&self, id: &str) -> Snapshot {
            snapshot_at(&self.0, id).unwrap()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn update_revision_at(
        dir: &Path,
        id: &str,
        revision: u64,
        config: Config,
    ) -> Result<Snapshot, String> {
        let mut identity = snapshot_at(dir, id)?.identity();
        identity.config_revision = revision;
        update_at(dir, &identity, config)
    }
    fn config(pattern: &str) -> Config {
        Config {
            derived_fields: vec![json!({"name":"request","source":"message","pattern":pattern})],
            references: Vec::new(),
        }
    }
    #[test]
    fn two_cases_with_same_field_name_are_isolated_and_restart_preserves_identity() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"},{"id":"b"}]}));
        let a = update_revision_at(&dir.0, "a", 0, config("(first)")).unwrap();
        let b = update_revision_at(&dir.0, "b", 0, config("(second)")).unwrap();
        assert_ne!(a.analysis_id, b.analysis_id);
        assert_eq!(dir.snapshot("a"), a);
        assert_eq!(dir.snapshot("b"), b);
        assert_eq!(a.config_revision, 1);
        assert_eq!(a.visibility_revision, 0);
        assert_eq!(dir.load()["revision"], 1); // configuration does not bump case notes.
    }
    #[test]
    fn bulk_browser_update_rechecks_late_native_or_restored_protection() {
        for table in ["native_evidence_cases", "native_evidence_protected", "case_recovery_protected"] {
            let dir = Directory::new();
            dir.save(json!({"cases":[{"id":"a"}]}));
            let before = dir.snapshot("a");
            assert!(!crate::case_evidence::native_owner(&dir.0, &before.identity()).unwrap());
            // A separate committed adoption/protection marker leaves the
            // admitted analysis/config/visibility identity unchanged.
            let conn = crate::case_store::connect(&dir.0).unwrap();
            conn.execute_batch(&format!("CREATE TABLE {table}(case_id TEXT PRIMARY KEY,analysis_id TEXT);")).unwrap();
            conn.execute(&format!("INSERT INTO {table} VALUES(?1,?2)"), params![before.case_id,before.analysis_id]).unwrap();
            let raw: String = conn.query_row("SELECT body FROM case_analysis WHERE case_id='a'", [], |row|row.get(0)).unwrap();
            let error = update_legacy_browser_at(&dir.0, &before.identity(), config("(changed)")).unwrap_err();
            assert!(error.starts_with("CASE_CONTEXT_TYPED_EDIT_REQUIRED"), "{table}: {error}");
            assert_eq!(conn.query_row::<String,_,_>("SELECT body FROM case_analysis WHERE case_id='a'", [], |row|row.get(0)).unwrap(), raw);
            assert_eq!(dir.snapshot("a"), before);
            // Typed editor mutation retains its existing native-capable path.
            let edited = update_at(&dir.0, &before.identity(), config("(typed)")).unwrap();
            assert_eq!(edited.config_revision, before.config_revision + 1);
        }
    }
    #[test]
    fn bulk_browser_update_keeps_legacy_context_cas_behavior() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let before = dir.snapshot("a");
        let edited = update_legacy_browser_at(&dir.0, &before.identity(), config("(legacy)")).unwrap();
        assert_eq!(edited.config_revision, before.config_revision + 1);
        assert_eq!(edited.config, config("(legacy)"));
        assert!(update_legacy_browser_at(&dir.0, &before.identity(), Config::default()).is_err());
        assert_eq!(dir.snapshot("a"), edited);
    }
    #[test]
    fn native_typed_updates_refresh_interpretation_notices_and_drop_obsolete_errors() {
        for native in [false, true] {
            let dir = Directory::new();
            dir.save(json!({"cases":[{"id":"a"}]}));
            let mut initial = dir.snapshot("a");
            initial.config.references.push(ReferenceDescriptor {
                schema_version: 1, interpretation_version: 1, id: "r".into(), name: "Reference".into(),
                content_sha256: "a".repeat(64), format: "jsonl".into(), columns: vec!["key".into()],
                key_columns: vec!["key".into()], duplicate_policy: "reject".into(),
            });
            initial.migration_diagnostics.push(Diagnostic {
                definition_index: None, code: "obsolete_migration_error".into(), message: "old error".into(),
            });
            initial.migration_diagnostics.extend(reference_interpretation_diagnostics([1], true));
            let conn = crate::case_store::context_connection(&dir.0).unwrap();
            conn.execute("UPDATE case_analysis SET body=?1 WHERE case_id='a'", [serde_json::to_string(&initial).unwrap()]).unwrap();
            if native {
                conn.execute_batch("CREATE TABLE native_evidence_cases(case_id TEXT PRIMARY KEY,analysis_id TEXT);").unwrap();
                conn.execute("INSERT INTO native_evidence_cases VALUES('a',?1)", [&initial.analysis_id]).unwrap();
            }
            let unchanged = update_at(&dir.0, &initial.identity(), initial.config.clone()).unwrap();
            assert_eq!(unchanged.config, initial.config);
            assert_eq!(dir.snapshot("a"), unchanged);
            let codes: Vec<_> = unchanged.migration_diagnostics.iter().map(|d| d.code.as_str()).collect();
            assert_eq!(codes, if native { vec!["reference_interpretation_legacy"] } else { vec![] });
            let mut exact_config = unchanged.config.clone();
            exact_config.references[0].interpretation_version = 2;
            let exact = update_at(&dir.0, &unchanged.identity(), exact_config.clone()).unwrap();
            assert_eq!(exact.config, exact_config);
            assert_eq!(dir.snapshot("a"), exact);
            let codes: Vec<_> = exact.migration_diagnostics.iter().map(|d| d.code.as_str()).collect();
            assert_eq!(codes, if native { vec!["reference_interpretation_exact"] } else { vec![] });
            let mut empty_config = exact.config.clone();
            empty_config.references.clear();
            let empty = update_at(&dir.0, &exact.identity(), empty_config).unwrap();
            assert!(empty.migration_diagnostics.is_empty());
            assert_eq!(dir.snapshot("a"), empty);
        }
    }
    #[test]
    fn queued_body_save_cannot_overwrite_new_config_and_stale_config_cas_fails() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a","note":"old"}]}));
        let mut queued = dir.load();
        let changed = update_revision_at(&dir.0, "a", 0, config("(new)")).unwrap();
        queued["cases"][0]["note"] = json!("new note");
        queued["cases"][0]["analysisContext"]["config"] = json!({"malformed":"stale client"});
        dir.save(queued);
        assert_eq!(dir.snapshot("a"), changed);
        assert_eq!(dir.load()["cases"][0]["note"], "new note");
        assert!(update_revision_at(&dir.0, "a", 0, Config::default()).is_err());
        assert_eq!(dir.snapshot("a"), changed);
    }
    #[test]
    fn legacy_existing_cases_copy_once_while_new_cases_are_empty() {
        let dir = Directory::new();
        let defs = json!([{"name":"request","source":"message","pattern":"(legacy)"}, {"name":"bad","source":"message","pattern":"["}, "malformed"]);
        let original = defs.to_string();
        std::fs::write(dir.0.join("derived_fields.json"), &original).unwrap();
        std::fs::write(
            dir.0.join("cases.json"),
            json!({"cases":[{"id":"old"}]}).to_string(),
        )
        .unwrap();
        let mut cases = dir.load();
        let old = dir.snapshot("old");
        assert_eq!(old.config.derived_fields, defs.as_array().unwrap().clone());
        assert_eq!(old.migration_diagnostics.len(), 2);
        cases["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"new"}));
        dir.save(cases);
        assert_eq!(dir.snapshot("new").config, Config::default());
        std::fs::write(dir.0.join("derived_fields.json"), "changed afterwards").unwrap();
        assert_eq!(dir.snapshot("old"), old);
        assert!(dir.0.join("cases.json").exists());
        // Migration itself never wrote either legacy input.
        assert_eq!(
            old.config.derived_fields,
            serde_json::from_str::<Vec<Value>>(&original).unwrap()
        );
    }
    #[test]
    fn existing_sqlite_cases_migrate_but_first_new_case_does_not_inherit_globals() {
        let dir = Directory::new();
        let conn = crate::case_store::connect(&dir.0).unwrap();
        conn.execute("INSERT INTO cases VALUES('old','{\"id\":\"old\"}',0)", [])
            .unwrap();
        conn.execute("INSERT INTO metadata VALUES('revision','4')", [])
            .unwrap();
        drop(conn);
        std::fs::write(
            dir.0.join("derived_fields.json"),
            serde_json::to_string(&config("(old)").derived_fields).unwrap(),
        )
        .unwrap();
        assert_eq!(dir.snapshot("old").config, config("(old)"));
        let fresh = Directory::new();
        std::fs::write(
            fresh.0.join("derived_fields.json"),
            serde_json::to_string(&config("(foreign)").derived_fields).unwrap(),
        )
        .unwrap();
        fresh.save(json!({"cases":[{"id":"new"}]}));
        assert_eq!(fresh.snapshot("new").config, Config::default());
    }
    #[test]
    fn malformed_legacy_document_is_preserved_with_diagnostics() {
        let dir = Directory::new();
        let raw = "[{ broken";
        std::fs::write(dir.0.join("derived_fields.json"), raw).unwrap();
        std::fs::write(
            dir.0.join("cases.json"),
            json!({"cases":[{"id":"a"}]}).to_string(),
        )
        .unwrap();
        let snapshot = dir.snapshot("a");
        assert_eq!(snapshot.legacy_raw, Some(json!(raw)));
        assert_eq!(
            snapshot.migration_diagnostics[0].code,
            "invalid_legacy_document"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("derived_fields.json")).unwrap(),
            raw
        );
    }
    #[test]
    fn exported_config_imports_with_new_identity_without_foreign_globals() {
        let source = Directory::new();
        source.save(json!({"cases":[{"id":"a"}]}));
        let original = update_revision_at(&source.0, "a", 0, config("(portable)")).unwrap();
        let mut exported = source.load();
        exported.as_object_mut().unwrap().remove("revision");
        let target = Directory::new();
        std::fs::write(
            target.0.join("derived_fields.json"),
            serde_json::to_string(&config("(wrong global)").derived_fields).unwrap(),
        )
        .unwrap();
        target.save(exported);
        let imported = target.snapshot("a");
        assert_eq!(imported.config, original.config);
        assert_ne!(imported.analysis_id, original.analysis_id);
        assert_eq!(imported.config_revision, 0);
        assert_eq!(imported.visibility_revision, 0);
    }
    #[test]
    fn invalid_import_rolls_back_case_body_and_context_together() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"old"}]}));
        let before = dir.load();
        let mut invalid = before.clone();
        let mut foreign = Snapshot::empty("new");
        foreign.config = config("[");
        invalid["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"new","analysisContext":foreign}));
        assert!(crate::case_store::save_at(&dir.0, invalid).is_err());
        assert_eq!(dir.load(), before);
        assert!(snapshot_at(&dir.0, "new").is_err());
    }
    #[test]
    fn disk_write_failure_does_not_publish_partial_config_or_revision() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let before = dir.snapshot("a");
        let conn = crate::case_store::connect(&dir.0).unwrap();
        conn.execute_batch("CREATE TRIGGER simulated_write_failure BEFORE UPDATE ON case_analysis BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        let error = update_revision_at(&dir.0, "a", 0, config("(next)")).unwrap_err();
        assert!(error.contains("simulated storage failure"));
        assert_eq!(dir.snapshot("a"), before);
        assert_eq!(dir.load()["revision"], 1);
    }
    #[test]
    fn deleted_case_cannot_be_resurrected_by_late_context_update() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let before = dir.snapshot("a");
        dir.save(json!({"revision":1,"cases":[]}));
        assert!(update_revision_at(&dir.0, "a", before.config_revision, config("(late)")).is_err());
        assert!(snapshot_at(&dir.0, "a").is_err());
        let conn = crate::case_store::connect(&dir.0).unwrap();
        assert_eq!(
            conn.query_row::<usize, _, _>("SELECT count(*) FROM case_analysis", [], |r| r.get(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn typed_steps_and_reference_descriptors_are_validated_without_activation() {
        let mut value = Config {
            derived_fields: vec![
                json!({"name":"payload","source":"message","steps":["base64_decode","parse_json"]}),
            ],
            references: vec![ReferenceDescriptor {
                interpretation_version: 1,
                schema_version: 1,
                id: "ref".into(),
                name: "Inventory".into(),
                content_sha256: "a".repeat(64),
                format: "csv".into(),
                columns: vec!["host".into()],
                key_columns: vec!["host".into()],
                duplicate_policy: "reject".into(),
            }],
        };
        validate(&value).unwrap();
        value.derived_fields[0]["steps"] = json!(["decrypt"]);
        assert!(validate(&value).is_err());
        value.derived_fields.clear();
        value.references[0].key_columns = vec!["missing".into()];
        assert!(validate(&value).is_err());
    }
    #[test]
    fn same_case_id_recreated_cannot_accept_old_analysis_edit() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let old = dir.snapshot("a").identity();
        dir.save(json!({"revision":1,"cases":[]}));
        dir.save(json!({"revision":2,"cases":[{"id":"a"}]}));
        let new = dir.snapshot("a");
        assert_ne!(new.analysis_id, old.analysis_id);
        assert_eq!(new.config_revision, old.config_revision);
        assert!(update_at(&dir.0, &old, config("(stale)")).is_err());
        assert_eq!(dir.snapshot("a"), new);
    }
    #[test]
    fn concurrent_config_edits_have_exactly_one_cas_winner() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let identity = dir.snapshot("a").identity();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let tasks: Vec<_> = ["(one)", "(two)"]
                .into_iter()
                .map(|pattern| {
                    let identity = &identity;
                    let barrier = &barrier;
                    let path = &dir.0;
                    scope.spawn(move || {
                        barrier.wait();
                        update_at(path, identity, config(pattern))
                    })
                })
                .collect();
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(dir.snapshot("a").config_revision, 1);
    }
    #[test]
    fn snapshot_survives_a_genuinely_new_process() {
        const KEY: &str = "LOGINSIGHT_CONTEXT_RESTART_TEST";
        if let Some(path) = std::env::var_os(KEY) {
            let path = std::path::PathBuf::from(path);
            let expected: Snapshot =
                serde_json::from_slice(&std::fs::read(path.join("expected.json")).unwrap())
                    .unwrap();
            assert_eq!(snapshot_at(&path, "a").unwrap(), expected);
            return;
        }
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"a"}]}));
        let original = update_revision_at(&dir.0, "a", 0, config("(durable)")).unwrap();
        std::fs::write(
            dir.0.join("expected.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "analysis_context::tests::snapshot_survives_a_genuinely_new_process",
                "--nocapture",
            ])
            .env(KEY, &dir.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[test]
    fn new_case_receipts_return_committed_context_without_bumping_existing_config() {
        let dir = Directory::new();
        let first = dir.save(json!({"cases":[{"id":"a"}]}));
        let a = dir.snapshot("a");
        assert_eq!(first["analysisContexts"], json!([a]));
        let second = dir.save(json!({"revision":1,"cases":[{"id":"a"},{"id":"b"}]}));
        assert_eq!(second["analysisContexts"], json!([dir.snapshot("b")]));
        assert_eq!(dir.snapshot("a"), a);
        let third = dir.save(dir.load());
        assert_eq!(third["analysisContexts"], json!([]));
    }
    #[test]
    fn reserved_names_and_cycles_reject_new_config_but_legacy_values_survive() {
        for name in ["id", "timestamp", "event_ref", "arquivo", "caminho", "@ip"] {
            let mut value = config("(a)");
            value.derived_fields[0]["name"] = json!(name);
            assert!(validate(&value).is_err(), "{name}");
        }
        let fields = vec![
            json!({"name":"a","source":"b.child","steps":["parse_json"]}),
            json!({"name":"b","source":"a","steps":["parse_json"]}),
        ];
        let invalid = Config {
            derived_fields: fields.clone(),
            references: Vec::new(),
        };
        assert!(validate(&invalid).unwrap_err().contains("circular"));
        let dir = Directory::new();
        std::fs::write(
            dir.0.join("derived_fields.json"),
            serde_json::to_vec(&fields).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.0.join("cases.json"),
            json!({"cases":[{"id":"a"}]}).to_string(),
        )
        .unwrap();
        let migrated = dir.snapshot("a");
        assert_eq!(migrated.config.derived_fields, fields);
        assert!(migrated
            .migration_diagnostics
            .iter()
            .any(|d| d.code == "invalid_legacy_dependencies"));
    }
    #[test]
    fn definition_order_handles_forward_children_and_direct_filter_dependencies() {
        let fields = vec![
            json!({"name":"a","source":"b.child","steps":["parse_json"]}),
            json!({"name":"b","source":"message","steps":["parse_json"]}),
        ];
        assert_eq!(definition_order(&fields).unwrap(), vec![1, 0]);
        let fields = vec![
            json!({"name":"a","source":"message","rules":[{"pattern":"(a)","filter":{"column":"b","op":"equals","value":"yes"}}]}),
            json!({"name":"b","source":"message","pattern":"(b)"}),
        ];
        assert_eq!(definition_order(&fields).unwrap(), vec![1, 0]);
        let too_many = vec![json!({"name":"x","source":"y","pattern":"(a)"}); 257];
        assert!(definition_order(&too_many).is_err());
    }
    #[test]
    fn portable_import_keeps_malformed_legacy_recovery_material_inactive() {
        let dir = Directory::new();
        let mut portable = Snapshot::empty("old");
        portable.legacy_raw = Some(json!("broken legacy document"));
        let receipt = dir.save(json!({"cases":[{"id":"new","analysisContext":portable}]}));
        let local = dir.snapshot("new");
        assert_eq!(local.legacy_raw, portable.legacy_raw);
        assert_eq!(local.config, Config::default());
        assert_eq!(
            local.migration_diagnostics[0].code,
            "imported_legacy_recovery"
        );
        assert_eq!(receipt["analysisContexts"], json!([local]));
    }
    #[test]
    fn active_snapshot_reads_committed_context_without_deserializing_evidence() {
        let dir = Directory::new();
        dir.save(json!({"active":"a","cases":[{"id":"a"},{"id":"b"}]}));
        let original = dir.snapshot("a");
        let conn = crate::case_store::connect(&dir.0).unwrap();
        conn.execute("UPDATE cases SET body='not evidence JSON' WHERE id='a'", [])
            .unwrap();
        assert_eq!(active_snapshot_at(&dir.0).unwrap(), Some(original));
        conn.execute(
            "UPDATE metadata SET value='\"missing\"' WHERE key='active'",
            [],
        )
        .unwrap();
        assert!(active_snapshot_at(&dir.0).is_err());
        conn.execute("UPDATE metadata SET value='7' WHERE key='active'", [])
            .unwrap();
        assert!(active_snapshot_at(&dir.0).is_err());
        conn.execute("UPDATE metadata SET value='null' WHERE key='active'", [])
            .unwrap();
        assert_eq!(active_snapshot_at(&dir.0).unwrap(), None);
    }
    #[test]
    fn aggregate_regex_budget_matches_runtime_limits_and_rejects_new_imports() {
        assert_eq!(
            regex_limits(0).unwrap(),
            RegexLimits {
                size_limit: 2 << 20,
                dfa_size_limit: 512 << 10
            }
        );
        assert_eq!(
            regex_limits(64).unwrap(),
            RegexLimits {
                size_limit: 256 << 10,
                dfa_size_limit: 64 << 10
            }
        );
        assert!(regex_limits(1025).is_err());
        let pattern = "(?:a{128}){128}";
        assert!(regex::RegexBuilder::new(pattern)
            .size_limit(REGEX_RULE_BYTES)
            .build()
            .is_ok());
        let value = Config {
            derived_fields: vec![
                json!({"name":"expanded","source":"message","rules":vec![json!({"pattern":pattern});64]}),
            ],
            references: Vec::new(),
        };
        assert!(validate(&value).is_err());
        let dir = Directory::new();
        let mut foreign = Snapshot::empty("foreign");
        foreign.config = value;
        assert!(crate::case_store::save_at(
            &dir.0,
            json!({"cases":[{"id":"new","analysisContext":foreign}]})
        )
        .is_err());
        assert!(dir.load()["cases"].as_array().unwrap().is_empty());
    }
    #[test]
    fn legacy_over_budget_rules_are_preserved_with_diagnostics() {
        let fields:Vec<_>=(0..5).map(|i|json!({"name":format!("f{i}"),"source":"message","rules":vec![json!({"pattern":"(a)"});256]})).collect();
        let dir = Directory::new();
        std::fs::write(
            dir.0.join("derived_fields.json"),
            serde_json::to_vec(&fields).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.0.join("cases.json"),
            json!({"cases":[{"id":"a"}]}).to_string(),
        )
        .unwrap();
        let migrated = dir.snapshot("a");
        assert_eq!(migrated.config.derived_fields, fields);
        assert!(migrated
            .migration_diagnostics
            .iter()
            .any(|d| d.code == "legacy_regex_budget"));
    }
    #[test]
    fn direct_json_import_cannot_silently_reset_exclusion_visibility() {
        let dir = Directory::new();
        dir.save(json!({"cases":[{"id":"existing"}]}));
        let before = dir.load();
        let mut foreign = Snapshot::empty("foreign");
        foreign.visibility_revision = 7;
        let mut incoming = before.clone();
        incoming["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"imported","analysisContext":foreign}));
        let error = crate::case_store::save_at(&dir.0, incoming).unwrap_err();
        assert!(error.contains("JSON") && error.contains("portátil"));
        assert_eq!(dir.load(), before);
    }
}
