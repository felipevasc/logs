//! Declarative, Case-owned enrichments. No code execution or remote lookups.
//!
//! Schema 1 (selectors, aliases, literals, legacy `format`) is read and evaluated exactly as
//! before. Schema 2 adds, per rule, an application condition (`when`) and one extraction
//! (Grok, dissect, JSON, XML, delimited, key=value or regex) and, per output, a type,
//! ordered normalization steps, missing/empty/invalid policies, a conflict policy and an
//! index preference. Fields that schema 2 does not use are not written back, so a map that
//! never used them serializes byte for byte as it did.
use crate::map_extract::{self, Extraction};
use crate::map_values::{self, Converted, DatetimeSpec, Step, ValueType};
use crate::model::Event;
use crate::query::{Filter, PreparedFilter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub(crate) const SCHEMA: u32 = 2;
fn is_zero(n: &u32) -> bool {
    *n == 0
}
fn is_true(b: &bool) -> bool {
    *b
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Selector {
    pub provider: Option<String>,
    pub channel: Option<String>,
    pub event_id: Option<String>,
    pub version: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Mapping {
    #[serde(default)]
    pub from: Vec<String>,
    pub literal: Option<Value>,
    #[serde(default)]
    pub format: String,
    pub description: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<ValueType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datetime: Option<DatetimeSpec>,
    /// `skip` (default) or `default`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_missing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// `skip` (default) or `keep` (emit the empty text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_empty: String,
    /// `skip` (default) or `keep_text` (emit the unconverted text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_invalid: String,
    /// `precedence` (default) or `reject` (no value when rules disagree).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conflict: String,
    /// `none`, `exact`, `range` or `terms`: a preference, built only when asked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub index: String,
}
impl Mapping {
    /// Uses the schema-2 conversion pipeline (otherwise the legacy `format` path runs).
    fn typed(&self) -> bool {
        self.ty.is_some() || !self.steps.is_empty() || self.datetime.is_some()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Rule {
    pub id: String,
    #[serde(default)]
    pub selector: Selector,
    #[serde(default)]
    pub priority: i32,
    pub fields: BTreeMap<String, Mapping>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Extra application conditions, all of which must hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Filter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Extraction>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Definition {
    pub id: String,
    pub version: String,
    pub title: String,
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Reusable Grok patterns defined for this map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grok_patterns: BTreeMap<String, String>,
    /// Presentation only (colours, notes): never changes what the map computes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
}

pub(crate) fn field<'a>(event: &'a Event, names: &[&str]) -> Option<&'a Value> {
    for name in names {
        if let Some(value) = event
            .fields
            .get(*name)
            .filter(|v| !v.is_null() && v.as_str() != Some(""))
        {
            return Some(value);
        }
        if let Some((_, value)) = event.fields.iter().find(|(key, value)| {
            key.eq_ignore_ascii_case(name) && !value.is_null() && value.as_str() != Some("")
        }) {
            return Some(value);
        }
    }
    None
}
pub(crate) fn text(event: &Event, names: &[&str]) -> String {
    field(event, names)
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default()
}
fn selector_values(event: &Event) -> [String; 4] {
    let mut provider = text(
        event,
        &[
            "Provider",
            "Provider.Name",
            "Event.System.Provider.Name",
            "winlog.provider_name",
            "provider",
        ],
    );
    if provider.is_empty() {
        provider = event.source.clone();
    }
    let channel = text(
        event,
        &[
            "Channel",
            "Event.System.Channel",
            "winlog.channel",
            "channel",
        ],
    );
    let id = if event.code.is_empty() {
        text(
            event,
            &["EventID", "Event.System.EventID", "event.code", "type"],
        )
    } else {
        event.code.clone()
    };
    [
        provider,
        channel,
        id,
        text(
            event,
            &[
                "Version",
                "Event.System.Version",
                "winlog.version",
                "event.version",
            ],
        ),
    ]
}
impl Selector {
    pub(crate) fn specificity(&self) -> usize {
        [&self.provider, &self.channel, &self.event_id, &self.version]
            .into_iter()
            .filter(|s| s.is_some())
            .count()
    }
    fn matches(&self, values: &[String; 4]) -> bool {
        [&self.provider, &self.channel, &self.event_id, &self.version]
            .into_iter()
            .zip(values)
            .all(|(expected, actual)| {
                expected
                    .as_ref()
                    .is_none_or(|s| s.eq_ignore_ascii_case(actual))
            })
    }
}

const ON_MISSING: &[&str] = &["", "skip", "default"];
const ON_EMPTY: &[&str] = &["", "skip", "keep"];
const ON_INVALID: &[&str] = &["", "skip", "keep_text"];
const CONFLICT: &[&str] = &["", "precedence", "reject"];
const INDEX: &[&str] = &["", "none", "exact", "range", "terms"];

pub(crate) fn validate(map: &Definition) -> Result<(), String> {
    if map.id.trim().is_empty()
        || map.version.trim().is_empty()
        || map.title.trim().is_empty()
        || map.id.len() > 128
        || map.version.len() > 128
        || map.title.len() > 256
        || map.rules.len() > 128
    {
        return Err("Informe id, versão, título e até 128 regras por mapa.".into());
    }
    if map.schema > SCHEMA {
        return Err(format!("Este mapa usa o esquema {}, mais novo que o suportado ({SCHEMA}).", map.schema));
    }
    if map.description.as_ref().is_some_and(|d| d.len() > 2048) {
        return Err("A descrição do mapa excede 2 KiB.".into());
    }
    let mut ids = HashSet::new();
    for rule in &map.rules {
        if rule.id.is_empty()
            || rule.id.len() > 128
            || !ids.insert(&rule.id)
            || rule.fields.len() > 64
        {
            return Err("As regras precisam de identificadores únicos e até 64 saídas.".into());
        }
        for selector in [
            &rule.selector.provider,
            &rule.selector.channel,
            &rule.selector.event_id,
            &rule.selector.version,
        ]
        .into_iter()
        .flatten()
        {
            if selector.is_empty() || selector.len() > 512 {
                return Err("Seletor vazio ou excessivo.".into());
            }
        }
        if rule.when.len() > 16 {
            return Err(format!("Regra {}: use até 16 condições de aplicação.", rule.id));
        }
        if !rule.when.is_empty() {
            crate::workspace::validate(&rule.when).map_err(|e| format!("Regra {}: condição inválida. {e}", rule.id))?;
            if rule.when.iter().any(|f| matches!(f.op.as_str(), "detection" | "finding" | "episode" | "threat_rule")) {
                return Err(format!("Regra {}: condições de detecção não podem ser usadas em mapas.", rule.id));
            }
        }
        for (target, mapping) in &rule.fields {
            if let Some(literal) = &mapping.literal {
                crate::case_evidence::metadata_transport_safe(literal).map_err(|_|"Literal numérico não pode atravessar o navegador sem perda. Escreva o número como string e use format pid ou TRY_CAST no SQL.".to_string())?;
            }
            if target.starts_with('_')
                || target.is_empty()
                || target.len() > 256
                || mapping.from.len() > 16
                || mapping.from.iter().any(|s| {
                    s.is_empty() || s.len() > 512 || matches!(s.as_str(), "id" | "event_ref")
                })
                || mapping.description.as_ref().is_some_and(|s| s.len() > 1024)
            {
                return Err("Campo do mapa inválido, reservado ou excessivo.".into());
            }
            if mapping.literal.is_some() == !mapping.from.is_empty() {
                return Err("Cada saída precisa de literal ou lista de campos de origem.".into());
            }
            if !["", "identity", "pid", "lowercase", "basename", "text"]
                .contains(&mapping.format.as_str())
            {
                return Err(format!("Normalização desconhecida: {}", mapping.format));
            }
            for (value, allowed, what) in [
                (&mapping.on_missing, ON_MISSING, "ausência"),
                (&mapping.on_empty, ON_EMPTY, "valor vazio"),
                (&mapping.on_invalid, ON_INVALID, "valor inválido"),
                (&mapping.conflict, CONFLICT, "conflito"),
                (&mapping.index, INDEX, "índice"),
            ] {
                if !allowed.contains(&value.as_str()) {
                    return Err(format!("Saída {target}: política de {what} desconhecida: {value}."));
                }
            }
            if mapping.typed() && !matches!(mapping.format.as_str(), "" | "identity") {
                return Err(format!("Saída {target}: use a normalização antiga (format) ou o tipo e as conversões, não ambos."));
            }
            map_values::validate_spec(mapping.ty, &mapping.steps, mapping.datetime.as_ref()).map_err(|e| format!("Saída {target}: {e}"))?;
            if mapping.on_missing == "default" {
                let Some(default) = &mapping.default else {
                    return Err(format!("Saída {target}: informe o valor padrão para ausência."));
                };
                crate::case_evidence::metadata_transport_safe(default).map_err(|_| format!("Saída {target}: o valor padrão numérico perderia precisão; escreva-o como texto."))?;
            }
            match (mapping.index.as_str(), mapping.ty) {
                ("range", Some(ValueType::Integer | ValueType::Decimal | ValueType::Datetime)) => {}
                ("range", _) => return Err(format!("Saída {target}: o índice de intervalos exige o tipo inteiro, decimal ou data/hora.")),
                ("terms", None | Some(ValueType::Text)) => {}
                ("terms", _) => return Err(format!("Saída {target}: o índice de termos só se aplica a texto.")),
                _ => {}
            }
        }
    }
    // Extractions must compile; every problem is reported together.
    compile_extractions(map).map(|_| ())?;
    if serde_json::to_vec(map).map_err(|e| e.to_string())?.len() > 256 << 10 {
        return Err("O mapa excede 256 KiB.".into());
    }
    Ok(())
}
fn compile_extractions(map: &Definition) -> Result<Vec<Option<map_extract::Compiled>>, String> {
    let mut problems = Vec::new();
    let mut out = Vec::new();
    for rule in &map.rules {
        match &rule.extract {
            None => out.push(None),
            Some(spec) => match map_extract::Compiled::new(spec, &map.grok_patterns) {
                Ok(c) => out.push(Some(c)),
                Err(diagnostics) => {
                    out.push(None);
                    for d in diagnostics.into_iter().take(4) {
                        problems.push(format!("Regra {}: {}", rule.id, d.message));
                    }
                }
            },
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems.join(" "))
    }
}
/// Every problem of every extraction with its position, for the editor.
pub(crate) fn extraction_diagnostics(map: &Definition) -> Vec<Value> {
    let mut out = Vec::new();
    for rule in &map.rules {
        if let Some(spec) = &rule.extract {
            if let Err(diagnostics) = map_extract::Compiled::new(spec, &map.grok_patterns) {
                for d in diagnostics {
                    out.push(json!({"rule": rule.id, "code": d.code, "message": d.message, "start": d.start, "end": d.end, "alternative": d.alternative}));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ evaluation trace
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputTrace {
    pub field: String,
    pub rule: String,
    /// `ok`, `missing`, `empty`, `invalid`, `default`, `kept_text`, `overridden`, `rejected`.
    pub status: &'static str,
    pub source: String,
    pub original: Option<String>,
    pub normalized: Option<Value>,
    pub reason: Option<String>,
    /// UTF-16 range of the original value inside the subject, when the parser knows it.
    pub range: Option<(usize, usize)>,
    pub steps: usize,
    pub ty: Option<ValueType>,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuleTrace {
    pub rule: String,
    pub eligible: bool,
    /// `none`, `recognized`, `not_recognized` or `error`.
    pub extraction: &'static str,
    pub alternative: Option<usize>,
    pub error: Option<String>,
    pub subject: Option<String>,
    pub extracted: Vec<(String, String, Option<(usize, usize)>)>,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Trace {
    /// Keep the texts and ranges (interactive previews); bulk scans only count states.
    #[serde(skip)]
    pub detail: bool,
    pub rules: Vec<RuleTrace>,
    pub outputs: Vec<OutputTrace>,
    pub conflicts: Vec<Value>,
}
fn utf16(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or(0, |s| s.encode_utf16().count())
}

// ------------------------------------------------------------------ compiled maps
struct RuleRuntime {
    when: Vec<PreparedFilter>,
    extraction: Option<map_extract::Compiled>,
}
pub(crate) struct Compiled {
    pub def: Definition,
    runtime: Vec<RuleRuntime>,
}
fn subject_of<'a>(event: &'a Event, source: &str) -> Option<std::borrow::Cow<'a, str>> {
    match source {
        "message" => Some(std::borrow::Cow::Borrowed(event.message.as_str())),
        "raw" => Some(std::borrow::Cow::Borrowed(if event.raw.is_empty() { event.message.as_str() } else { event.raw.as_str() })),
        other => event.col_ref(other),
    }
}
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}
/// The legacy normalizations, unchanged.
fn legacy(mapping: &Mapping, input: &Value) -> Option<Value> {
    let string = || input.as_str().map(str::to_owned).unwrap_or_else(|| input.to_string());
    Some(match mapping.format.as_str() {
        "pid" => {
            let s = string();
            let s = s.trim();
            let n = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16).ok()
            } else {
                s.parse().ok()
            };
            json!(n?)
        }
        "lowercase" => json!(string().to_lowercase()),
        "basename" => json!(string().rsplit(['\\', '/']).next().unwrap_or_default()),
        "text" => json!(string()),
        _ => input.clone(),
    })
}
fn truncated(text: &str) -> String {
    text.chars().take(512).collect()
}

impl Compiled {
    pub(crate) fn new(def: Definition) -> Result<Arc<Self>, String> {
        validate(&def)?;
        let extractions = compile_extractions(&def)?;
        let runtime = def
            .rules
            .iter()
            .zip(extractions)
            .map(|(rule, extraction)| RuleRuntime { when: crate::query::prepare(&rule.when), extraction })
            .collect();
        Ok(Arc::new(Self { def, runtime }))
    }

    pub(crate) fn evaluate(&self, event: &Event, mut trace: Option<&mut Trace>) -> Result<Option<Value>, String> {
        let map = &self.def;
        if !map.enabled {
            return Ok(None);
        }
        let mut values = selector_values(event);
        if values[3].is_empty()
            && map.rules.iter().any(|rule| rule.selector.version.is_some())
            && event.raw.len() <= crate::search_regex::SUBJECT_LIMIT
            && event.raw.trim_start().starts_with('<')
        {
            // Consult the original Windows System header only when requested.
            if let Ok(document) = roxmltree::Document::parse(&event.raw) {
                if let Some(version) = document
                    .root_element()
                    .children()
                    .find(|n| n.has_tag_name("System"))
                    .and_then(|system| system.children().find(|n| n.has_tag_name("Version")))
                    .and_then(|n| n.text())
                {
                    values[3] = version.trim().to_owned();
                }
            }
        }
        let mut rules: Vec<(usize, &Rule)> = Vec::new();
        for (index, rule) in map.rules.iter().enumerate() {
            let eligible = rule.enabled
                && rule.selector.matches(&values)
                && self.runtime[index].when.iter().all(|f| crate::query::matches(event, f));
            if let Some(t) = trace.as_deref_mut() {
                t.rules.push(RuleTrace { rule: rule.id.clone(), eligible, extraction: "none", ..Default::default() });
            }
            if eligible {
                rules.push((index, rule));
            }
        }
        rules.sort_by(|a, b| {
            b.1.selector
                .specificity()
                .cmp(&a.1.selector.specificity())
                .then_with(|| b.1.priority.cmp(&a.1.priority))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let mut fields = Map::new();
        let mut provenance = Map::new();
        let mut conflicts = Vec::new();
        let mut policies: HashMap<String, (String, String)> = HashMap::new();
        let mut rejected: HashMap<String, (String, String)> = HashMap::new();
        let mut bytes = 0usize;
        for (index, rule) in rules {
            crate::operations::check()?;
            // One extraction per rule and record, shared by all of its outputs.
            let mut extracted = None;
            let mut subject_text: Option<String> = None;
            if let Some(extraction) = &self.runtime[index].extraction {
                let subject = subject_of(event, extraction.spec.source());
                let outcome = match &subject {
                    Some(s) => extraction.run(s),
                    None => Ok(None),
                };
                if let Some(t) = trace.as_deref_mut() {
                    let detail = t.detail;
                    let slot = t.rules.iter_mut().find(|r| r.rule == rule.id).unwrap();
                    if detail {
                        slot.subject = subject.as_deref().map(|s| s.chars().take(16384).collect());
                    }
                    match &outcome {
                        Ok(Some(e)) => {
                            slot.extraction = "recognized";
                            slot.alternative = e.alternative;
                            if let (true, Some(s)) = (detail, &subject) {
                                slot.extracted = e.items.iter().map(|i| (i.name.clone(), truncated(&i.value), i.range.map(|(a, b)| (utf16(s, a), utf16(s, b))))).collect();
                            }
                        }
                        Ok(None) => slot.extraction = "not_recognized",
                        Err(e) => {
                            slot.extraction = "error";
                            slot.error = Some(e.clone());
                        }
                    }
                }
                subject_text = subject.map(|s| s.into_owned());
                extracted = outcome?;
            }
            for (target, mapping) in &rule.fields {
                let Some(candidate) = self.candidate(event, mapping, extracted.as_ref(), subject_text.as_deref(), &mut trace, rule, target)? else {
                    continue;
                };
                let (value, source, details) = candidate;
                if let Some(previous) = fields.get(target) {
                    if previous != &value && conflicts.len() < 128 {
                        conflicts.push(json!({"field":target,"ignoredRule":rule.id,"reason":"specificity_then_priority_then_id"}));
                    }
                    if previous != &value {
                        if let Some((winner, policy)) = policies.get(target) {
                            if policy == "reject" {
                                rejected.entry(target.clone()).or_insert((winner.clone(), rule.id.clone()));
                            }
                        }
                    }
                    if let Some(t) = trace.as_deref_mut() {
                        if let Some(o) = t.outputs.iter_mut().rev().find(|o| o.field == *target && o.rule == rule.id) {
                            o.status = "overridden";
                        }
                    }
                    continue;
                }
                bytes += serde_json::to_vec(&value).map_err(|e| e.to_string())?.len()
                    + target.len()
                    + mapping.description.as_ref().map_or(0, String::len)
                    + 512;
                if bytes > 512 << 10 || fields.len() >= 128 {
                    return Err("O mapa excede 128 saídas ou 512 KiB por registro.".into());
                }
                fields.insert(target.clone(), value);
                policies.insert(target.clone(), (rule.id.clone(), mapping.conflict.clone()));
                let mut entry = json!({"rule":rule.id,"source":source,"format":mapping.format,"description":mapping.description,"specificity":rule.selector.specificity(),"priority":rule.priority});
                if let Some(object) = entry.as_object_mut() {
                    // From schema 2 the description lives in the definition only, so editing it
                    // never makes stored provenance stale.
                    if map.schema >= 2 {
                        object.remove("description");
                    }
                    if let Some(extra) = details {
                        object.extend(extra);
                    }
                }
                provenance.insert(target.clone(), entry);
            }
        }
        for (field, (winner, ignored)) in &rejected {
            fields.remove(field);
            provenance.remove(field);
            conflicts.push(json!({"field":field,"rules":[winner,ignored],"reason":"rejected"}));
            if let Some(t) = trace.as_deref_mut() {
                for o in t.outputs.iter_mut().filter(|o| o.field == *field && matches!(o.status, "ok" | "default" | "kept_text")) {
                    o.status = "rejected";
                }
            }
        }
        if let Some(t) = trace.as_deref_mut() {
            t.conflicts = conflicts.clone();
        }
        if fields.is_empty() {
            return Ok(None);
        }
        fields.insert(
            "_provenance".into(),
            json!({"map":map.id,"version":map.version,"fields":provenance,"conflicts":conflicts}),
        );
        Ok(Some(Value::Object(fields)))
    }

    /// One output of one rule: its value, where it came from and extra provenance for
    /// outputs that use schema-2 features. `Ok(None)`: nothing is emitted.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn candidate(
        &self,
        event: &Event,
        mapping: &Mapping,
        extracted: Option<&map_extract::Extracted>,
        subject: Option<&str>,
        trace: &mut Option<&mut Trace>,
        rule: &Rule,
        target: &str,
    ) -> Result<Option<(Value, String, Option<Map<String, Value>>)>, String> {
        let mut out = OutputTrace { field: target.into(), rule: rule.id.clone(), status: "missing", ty: mapping.ty, steps: mapping.steps.len(), ..Default::default() };
        // 1. The input: a literal, an extracted value or an event field, in preference order.
        let detail = trace.as_ref().is_some_and(|t| t.detail);
        let mut range = None;
        let mut grok_conv = None;
        let input: Option<(Value, String)> = if let Some(literal) = &mapping.literal {
            Some((literal.clone(), "literal".to_owned()))
        } else {
            let mut found = None;
            for name in &mapping.from {
                if let Some(item) = extracted.and_then(|e| e.items.iter().find(|i| i.name == *name)) {
                    range = item.range.map(|(a, b)| (subject.map_or(a, |s| utf16(s, a)), subject.map_or(b, |s| utf16(s, b))));
                    grok_conv = item.conv;
                    found = Some((Value::String(item.value.clone()), format!("extract:{name}")));
                    break;
                }
                if let Some(value) = field(event, &[name]) {
                    found = Some((value.clone(), name.clone()));
                    break;
                }
            }
            found
        };
        let finish = |trace: &mut Option<&mut Trace>, out: OutputTrace| {
            if let Some(t) = trace.as_deref_mut() {
                t.outputs.push(out);
            }
        };
        let Some((input, source)) = input else {
            if mapping.on_missing == "default" {
                if let Some(default) = &mapping.default {
                    out.status = "default";
                    out.normalized = Some(default.clone());
                    out.source = "default".into();
                    finish(trace, out);
                    return Ok(Some((default.clone(), "default".into(), Some(Map::from_iter([("missing".to_owned(), json!(true))])))));
                }
            }
            finish(trace, out);
            return Ok(None);
        };
        out.source = source.clone();
        crate::field_transform::payload_bytes(&input, 64 << 10).map_err(|_| "Valor de enriquecimento excede 64 KiB ou os limites de estrutura.".to_string())?;
        let original_text = scalar_text(&input);
        if detail {
            out.original = original_text.as_deref().map(truncated);
            out.range = range;
        }
        // 2. Legacy normalization: exactly the pre-schema-2 behaviour.
        if !mapping.typed() && grok_conv.is_none() {
            return Ok(match legacy(mapping, &input) {
                Some(value) => {
                    out.status = "ok";
                    out.normalized = Some(value.clone());
                    finish(trace, out);
                    Some((value, source, None))
                }
                None => {
                    out.status = "invalid";
                    out.reason = Some("A normalização antiga não reconheceu o valor.".into());
                    finish(trace, out);
                    None
                }
            });
        }
        // 3. Schema-2 pipeline: steps, then the type. A Grok `:int`/`:float` suffix is an
        // explicit request for that type when the output does not choose one.
        let ty = mapping.ty.or(match grok_conv {
            Some(crate::grok::Conv::Int) => Some(ValueType::Integer),
            Some(crate::grok::Conv::Float) => Some(ValueType::Decimal),
            None => None,
        });
        let text = match (&input, ty) {
            (Value::Object(_) | Value::Array(_), Some(ValueType::Text) | None) => Some(input.to_string()),
            (Value::Object(_) | Value::Array(_), _) => None,
            _ => original_text.clone(),
        };
        let converted = match &text {
            Some(t) => map_values::convert(t, &mapping.steps, ty, mapping.datetime.as_ref()),
            None => Converted::Invalid("O valor é uma estrutura ou lista; só o tipo texto a aceita (como JSON compacto).".into()),
        };
        let mut extra = Map::new();
        extra.insert("original".into(), json!(truncated(&original_text.clone().unwrap_or_default())));
        if let Some(ty) = ty {
            extra.insert("type".into(), json!(ty));
        }
        if !mapping.steps.is_empty() {
            extra.insert("steps".into(), json!(mapping.steps));
        }
        if let Some(a) = extracted.and_then(|e| e.alternative) {
            extra.insert("alternative".into(), json!(a));
        }
        match converted {
            Converted::Value(value) => {
                out.status = "ok";
                out.normalized = Some(value.clone());
                finish(trace, out);
                Ok(Some((value, source, Some(extra))))
            }
            Converted::Empty if mapping.on_empty == "keep" => {
                let value = json!("");
                out.status = "empty";
                out.normalized = Some(value.clone());
                finish(trace, out);
                Ok(Some((value, source, Some(extra))))
            }
            Converted::Empty => {
                out.status = "empty";
                finish(trace, out);
                Ok(None)
            }
            Converted::Invalid(reason) if mapping.on_invalid == "keep_text" => {
                let value = json!(original_text.unwrap_or_default());
                out.status = "kept_text";
                out.reason = Some(reason);
                out.normalized = Some(value.clone());
                extra.insert("conversion".into(), json!("invalid_kept_text"));
                finish(trace, out);
                Ok(Some((value, source, Some(extra))))
            }
            Converted::Invalid(reason) => {
                out.status = "invalid";
                out.reason = Some(reason);
                finish(trace, out);
                Ok(None)
            }
        }
    }
}

/// Convenience for tests and one-off callers; production code keeps a [`Compiled`].
#[cfg(test)]
pub(crate) fn evaluate(event: &Event, map: &Definition) -> Result<Option<Value>, String> {
    Compiled::new(map.clone())?.evaluate(event, None)
}

/// What a map can produce, without running it (for the output list, diffs and indexes).
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputInfo {
    pub field: String,
    pub rule: String,
    pub ty: Option<ValueType>,
    pub index: String,
    pub description: Option<String>,
    /// A stable text of everything that changes the computed value.
    pub signature: String,
}
pub(crate) fn outputs(map: &Definition) -> Vec<OutputInfo> {
    let mut out = Vec::new();
    for rule in &map.rules {
        for (field, m) in &rule.fields {
            let computed = json!({"from":m.from,"literal":m.literal,"format":m.format,"type":m.ty,"steps":m.steps,"datetime":m.datetime,"onMissing":m.on_missing,"default":m.default,"onEmpty":m.on_empty,"onInvalid":m.on_invalid,"conflict":m.conflict,"selector":rule.selector,"priority":rule.priority,"when":rule.when,"extract":rule.extract,"enabled":rule.enabled && map.enabled,"grok":map.grok_patterns});
            out.push(OutputInfo { field: field.clone(), rule: rule.id.clone(), ty: m.ty, index: m.index.clone(), description: m.description.clone(), signature: computed.to_string() });
        }
    }
    out
}

/// Everything that changes a computed value, and nothing that does not: titles, notes and
/// colours (`ui`) are excluded, as are mapping descriptions once the map is schema 2 (the
/// record's provenance no longer embeds them). Caches and indexes key on this, so editing a
/// description or a colour never invalidates them.
pub(crate) fn semantic_signature(map: &Definition) -> String {
    let rules: Vec<Value> = map
        .rules
        .iter()
        .map(|rule| {
            let fields: BTreeMap<&String, Value> = rule
                .fields
                .iter()
                .map(|(name, m)| {
                    let mut m = m.clone();
                    if map.schema >= 2 {
                        m.description = None;
                        m.index.clear();
                    }
                    (name, serde_json::to_value(&m).unwrap_or_default())
                })
                .collect();
            json!({"id":rule.id,"selector":rule.selector,"priority":rule.priority,"enabled":rule.enabled,"when":rule.when,"extract":rule.extract,"fields":fields})
        })
        .collect();
    json!({"id":map.id,"version":map.version,"schema":map.schema.min(1),"enabled":map.enabled,"grok":map.grok_patterns,"rules":rules}).to_string()
}

pub(crate) fn built_in() -> Definition {
    fn rule(
        id: &str,
        provider: Option<&str>,
        channel: Option<&str>,
        code: Option<&str>,
        pairs: &[(&str, &[&str], &str)],
    ) -> Rule {
        Rule {
            id: id.into(),
            selector: Selector {
                provider: provider.map(str::to_owned),
                channel: channel.map(str::to_owned),
                event_id: code.map(str::to_owned),
                version: None,
            },
            priority: 0,
            enabled: true,
            description: None,
            when: Vec::new(),
            extract: None,
            fields: pairs
                .iter()
                .map(|(name, from, format)| {
                    (
                        name.to_string(),
                        Mapping {
                            from: from.iter().map(|s| s.to_string()).collect(),
                            format: format.to_string(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        }
    }
    let mut sysmon = rule(
        "sysmon-create",
        Some("Microsoft-Windows-Sysmon"),
        Some("Microsoft-Windows-Sysmon/Operational"),
        Some("1"),
        &[
            ("process.guid", &["ProcessGuid"], "lowercase"),
            ("process.pid", &["ProcessId"], "pid"),
            ("process.parent_guid", &["ParentProcessGuid"], "lowercase"),
            ("process.parent_pid", &["ParentProcessId"], "pid"),
            ("process.image", &["Image"], ""),
            ("process.command_line", &["CommandLine"], ""),
            ("process.user", &["User"], ""),
            ("host", &["Computer", "host.name", "Hostname"], "lowercase"),
        ],
    );
    sysmon.fields.insert(
        "event.kind".into(),
        Mapping {
            literal: Some(json!("process_start")),
            description: Some("Criação de processo Sysmon".into()),
            ..Default::default()
        },
    );
    let mut security = rule(
        "security-create",
        Some("Microsoft-Windows-Security-Auditing"),
        Some("Security"),
        Some("4688"),
        &[
            ("process.pid", &["NewProcessId"], "pid"),
            (
                "process.parent_pid",
                &["ProcessId", "CreatorProcessId"],
                "pid",
            ),
            ("process.image", &["NewProcessName"], ""),
            (
                "process.command_line",
                &["CommandLine", "ProcessCommandLine"],
                "",
            ),
            ("process.user", &["TargetUserName", "SubjectUserName"], ""),
            ("host", &["Computer", "host.name"], "lowercase"),
        ],
    );
    security.fields.insert(
        "event.kind".into(),
        Mapping {
            literal: Some(json!("process_start")),
            description: Some("Criação de processo Security 4688".into()),
            ..Default::default()
        },
    );
    let generic = rule(
        "generic-aliases",
        None,
        None,
        None,
        &[
            (
                "host",
                &["host.name", "hostname", "Computer", "host"],
                "lowercase",
            ),
            ("user", &["user.name", "User", "SubjectUserName", "uid"], ""),
            ("process.guid", &["process.entity_id"], "lowercase"),
            ("process.pid", &["process.pid"], "pid"),
            ("process.parent_pid", &["process.parent.pid"], "pid"),
            ("process.image", &["process.executable"], ""),
            ("process.command_line", &["process.command_line"], ""),
        ],
    );
    let auditd = rule(
        "auditd-exec",
        None,
        None,
        Some("EXECVE"),
        &[
            ("process.command_line", &["a0", "proctitle"], ""),
            ("process.pid", &["pid"], "pid"),
            ("process.parent_pid", &["ppid"], "pid"),
            ("process.image", &["exe", "comm"], ""),
            ("process.user", &["auid", "uid"], ""),
            ("host", &["node", "hostname"], "lowercase"),
        ],
    );
    Definition {
        id: "loginsight.local-normalization".into(),
        version: "1.0.0".into(),
        title: "Windows, Sysmon, auditd e campos genéricos".into(),
        rules: vec![sysmon, security, auditd, generic],
        schema: 0,
        description: None,
        enabled: true,
        grok_patterns: BTreeMap::new(),
        ui: None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specificity_provenance_and_raw_preservation() {
        let mut event = Event::empty();
        event.source = "Microsoft-Windows-Sysmon".into();
        event.code = "1".into();
        event.fields=serde_json::from_value(json!({"Channel":"Microsoft-Windows-Sysmon/Operational","ProcessId":"0x2a","process.pid":7,"Image":"C:\\a.exe"})).unwrap();
        let raw = event.fields.clone();
        let value = evaluate(&event, &built_in()).unwrap().unwrap();
        assert_eq!(value["process.pid"], 42);
        assert_eq!(
            value["_provenance"]["fields"]["process.pid"]["rule"],
            "sysmon-create"
        );
        assert!(!value["_provenance"]["conflicts"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(event.fields, raw);
    }
    #[test]
    fn channel_and_provider_prevent_event_id_collision() {
        let mut event = Event::empty();
        event.code = "4688".into();
        event.source = "other".into();
        event.fields.insert("NewProcessId".into(), json!("0x10"));
        assert!(evaluate(&event, &built_in()).unwrap().is_none());
        validate(&built_in()).unwrap();
    }
    #[test]
    fn system_version_specificity_and_large_output_are_verified() {
        let mut map = built_in();
        let mut specific = map.rules[0].clone();
        specific.id = "sysmon-v5".into();
        specific.selector.version = Some("5".into());
        specific.fields.insert(
            "schema".into(),
            Mapping { literal: Some(json!("v5")), ..Default::default() },
        );
        map.rules.push(specific);
        let mut event = Event::empty();
        event.source = "Microsoft-Windows-Sysmon".into();
        event.code = "1".into();
        event.fields.insert(
            "channel".into(),
            json!("Microsoft-Windows-Sysmon/Operational"),
        );
        event.raw = "<Event><System><Version>5</Version></System><EventData/></Event>".into();
        assert_eq!(evaluate(&event, &map).unwrap().unwrap()["schema"], "v5");
        event
            .fields
            .insert("Image".into(), json!("x".repeat(70 << 10)));
        assert!(evaluate(&event, &map).is_err());
    }
    #[test]
    fn schema_one_maps_round_trip_unchanged_and_new_fields_are_not_written_back() {
        let original = serde_json::to_value(built_in()).unwrap();
        let text = serde_json::to_string(&original).unwrap();
        for key in ["schema", "grokPatterns", "enabled", "when", "extract", "steps", "onMissing", "index", "type"] {
            assert!(!text.contains(&format!("\"{key}\"")), "{key} leaked into a schema-1 map");
        }
        let reread: Definition = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(reread, built_in());
        assert_eq!(serde_json::to_value(&reread).unwrap(), original);
    }

    fn grok_map(patterns: &[&str], fields: &[(&str, Mapping)]) -> Definition {
        Definition {
            id: "t".into(),
            version: "1".into(),
            title: "t".into(),
            schema: 2,
            description: None,
            enabled: true,
            grok_patterns: BTreeMap::new(),
            ui: None,
            rules: vec![Rule {
                id: "r".into(),
                selector: Selector::default(),
                priority: 0,
                enabled: true,
                description: None,
                when: vec![],
                extract: Some(Extraction::Grok { patterns: patterns.iter().map(|s| s.to_string()).collect(), anchor: "search".into(), case_insensitive: false, dot_all: false, source: "message".into() }),
                fields: fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            }],
        }
    }
    fn from(name: &str) -> Mapping {
        Mapping { from: vec![name.into()], ..Default::default() }
    }
    fn message(text: &str) -> Event {
        let mut e = Event::empty();
        e.message = text.into();
        e.raw = text.into();
        e
    }
    #[test]
    fn grok_extraction_feeds_typed_outputs_with_provenance_and_the_original_intact() {
        let map = grok_map(
            &["%{IP:ip_origem} usuario=%{WORD:usuario} duracao=%{NUMBER:duracao} pid=%{BASE16NUM:pid} id=%{NONNEGINT:codigo}"],
            &[
                ("src.ip", Mapping { ty: Some(ValueType::Ip), ..from("ip_origem") }),
                ("user", from("usuario")),
                ("duration", Mapping { ty: Some(ValueType::Decimal), ..from("duracao") }),
                ("process.pid", Mapping { ty: Some(ValueType::Integer), steps: vec![Step::HexToInt], ..from("pid") }),
                ("code_text", from("codigo")),
                ("code_number", Mapping { ty: Some(ValueType::Integer), ..from("codigo") }),
            ],
        );
        let event = message("falha 10.0.0.5 usuario=ana duracao=12.5 pid=0x2a id=00123");
        let before = event.clone();
        let value = evaluate(&event, &map).unwrap().unwrap();
        assert_eq!(value["src.ip"], "10.0.0.5");
        assert_eq!(value["duration"], 12.5);
        assert_eq!(value["process.pid"], 42);
        assert_eq!(value["code_text"], "00123", "recognising a number is not converting it");
        assert_eq!(value["code_number"], 123);
        assert_eq!(value["_provenance"]["fields"]["process.pid"]["original"], "0x2a");
        assert_eq!(value["_provenance"]["fields"]["user"]["source"], "extract:usuario");
        assert_eq!(event.message, before.message);
        assert_eq!(event.fields, before.fields);
    }
    #[test]
    fn invalid_missing_empty_and_conflict_policies_are_explicit_and_traced() {
        let mut keep = Mapping { ty: Some(ValueType::Integer), on_invalid: "keep_text".into(), ..from("n") };
        let skip = Mapping { ty: Some(ValueType::Integer), ..from("n") };
        let default = Mapping { on_missing: "default".into(), default: Some(json!("sem usuário")), ..from("quem") };
        let mut map = grok_map(&["n=%{WORD:n}"], &[("kept", keep.clone()), ("skipped", skip), ("who", default)]);
        let mut trace = Trace::default();
        let compiled = Compiled::new(map.clone()).unwrap();
        let value = compiled.evaluate(&message("n=abc"), Some(&mut trace)).unwrap().unwrap();
        assert_eq!(value["kept"], "abc");
        assert!(value.get("skipped").is_none());
        assert_eq!(value["who"], "sem usuário");
        let status = |f: &str| trace.outputs.iter().find(|o| o.field == f).unwrap().status;
        assert_eq!((status("kept"), status("skipped"), status("who")), ("kept_text", "invalid", "default"));
        // Reject: two rules with different values leave the field out and say why.
        keep.on_invalid.clear();
        let reject = Mapping { conflict: "reject".into(), literal: Some(json!("a")), ..Default::default() };
        let other = Mapping { literal: Some(json!("b")), ..Default::default() };
        let mut second = map.rules[0].clone();
        second.id = "s".into();
        second.priority = -1;
        second.extract = None;
        second.fields = [("z".to_owned(), other)].into();
        map.rules[0].fields = [("z".to_owned(), reject)].into();
        map.rules.push(second);
        let value = evaluate(&message("n=1"), &map).unwrap();
        assert!(value.is_none() || value.as_ref().unwrap().get("z").is_none());
    }
    #[test]
    fn conditions_and_disabled_rules_decide_eligibility() {
        let mut map = grok_map(&["x=%{INT:x}"], &[("x", from("x"))]);
        map.rules[0].when = vec![Filter { column: "message".into(), op: "contains".into(), value: "ATIVO".into(), value2: None }];
        assert!(evaluate(&message("x=1"), &map).unwrap().is_none());
        assert_eq!(evaluate(&message("ativo x=1"), &map).unwrap().unwrap()["x"], "1");
        map.rules[0].enabled = false;
        assert!(evaluate(&message("ativo x=1"), &map).unwrap().is_none());
        map.rules[0].enabled = true;
        map.enabled = false;
        assert!(evaluate(&message("ativo x=1"), &map).unwrap().is_none());
    }
    #[test]
    fn validation_rejects_unsupported_or_contradictory_definitions() {
        let bad = |mapping: Mapping| validate(&grok_map(&["x=%{INT:x}"], &[("o", mapping)])).is_err();
        assert!(bad(Mapping { ty: Some(ValueType::Datetime), ..from("x") }));
        assert!(bad(Mapping { format: "pid".into(), ty: Some(ValueType::Integer), ..from("x") }));
        assert!(bad(Mapping { index: "range".into(), ty: Some(ValueType::Text), ..from("x") }));
        assert!(bad(Mapping { index: "terms".into(), ty: Some(ValueType::Integer), ..from("x") }));
        assert!(bad(Mapping { on_missing: "default".into(), ..from("x") }));
        assert!(bad(Mapping { on_invalid: "explode".into(), ..from("x") }));
        validate(&grok_map(&["x=%{INT:x}"], &[("o", Mapping { index: "range".into(), ty: Some(ValueType::Integer), ..from("x") })])).unwrap();
        let unknown = validate(&grok_map(&["x=%{NOPE:x}"], &[("o", from("x"))])).unwrap_err();
        assert!(unknown.contains("NOPE"), "{unknown}");
        let diagnostics = extraction_diagnostics(&grok_map(&["x=%{NOPE:x}"], &[("o", from("x"))]));
        assert_eq!(diagnostics[0]["code"], "unknown_pattern");
    }
    #[test]
    fn output_signatures_change_only_when_the_computed_value_changes() {
        let mut map = grok_map(&["x=%{INT:x}"], &[("o", Mapping { description: Some("a".into()), ..from("x") })]);
        let before = outputs(&map);
        map.rules[0].fields.get_mut("o").unwrap().description = Some("b".into());
        map.ui = Some(json!({"colors":{"o":"#ff0000"}}));
        assert_eq!(outputs(&map)[0].signature, before[0].signature);
        map.rules[0].fields.get_mut("o").unwrap().steps = vec![Step::Trim];
        assert_ne!(outputs(&map)[0].signature, before[0].signature);
    }
}
