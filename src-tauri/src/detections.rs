//! Correlated detections and triage in one parallel pass over the selection:
//! single-event signals, thresholds, distinct counts, ordered sequences,
//! periodic communication, threat-catalog signals, entity risk and rarity.
//! A detection points to verifiable records; it is not proof of compromise.
use crate::{
    attack::{self, AttackRef},
    entities::{self, Role},
    model::Event,
    querylang::{self, Ctx, Expr, FieldRef},
    threats::CompiledCatalog,
};
use aho_corasick::AhoCorasick;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

const BUILTIN: &str = include_str!("../resources/detection-rules.json");
const EXPANSION: &str = include_str!("../resources/detection-expansion.json");
const ADVANCED: &str = include_str!("../resources/detection-advanced.json");
const CONTENT: &str = include_str!("../resources/detection-content.json");
fn builtin_defs() -> Result<Vec<RuleDef>, String> {
    let mut rules = Vec::new();
    for text in [BUILTIN, EXPANSION, ADVANCED, CONTENT] {
        rules.extend(serde_json::from_str::<RuleFile>(text).map_err(|e| e.to_string())?.rules);
    }
    Ok(rules)
}
const ENTITY_KEYS: usize = 100_000;
const RARE_KEYS: usize = 100_000;

fn yes() -> bool {
    true
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StepDef {
    #[serde(default, rename = "where")]
    pub condition: String,
    #[serde(default)]
    pub by: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArrayCondition {
    pub field: String,
    #[serde(rename = "where")]
    pub condition: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateDef {
    pub field: String,
    pub operation: String,
    pub gte: Option<f64>,
    pub lte: Option<f64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatioDef {
    pub numerator: String,
    pub gte: f64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindingDef {
    pub field: String,
    pub steps: Vec<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageWindow {
    pub dataset_fingerprint: String,
    pub source: String,
    pub namespace: String,
    pub category: String,
    pub start: i64,
    pub end: i64,
    pub complete: bool,
    pub justification: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub severity: String,
    #[serde(default)]
    pub attack: Vec<String>,
    pub kind: String,
    #[serde(default, rename = "where")]
    pub condition: String,
    #[serde(default)]
    pub by: Vec<String>,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub distinct: Option<String>,
    #[serde(default)]
    pub distinct_fallback: Vec<String>,
    #[serde(default)]
    pub steps: Vec<StepDef>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Extra tactics declared by the rule (Sigma tags).
    #[serde(default)]
    pub tactics: Vec<String>,
    #[serde(default)]
    pub evidence: crate::evidence::Policy,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub any: Vec<ArrayCondition>,
    #[serde(default)]
    pub unless: String,
    #[serde(default)]
    pub provenance: serde_json::Value,
    #[serde(default)]
    pub ratio: Option<RatioDef>,
    #[serde(default)]
    pub aggregate: Option<AggregateDef>,
    #[serde(default)]
    pub coverage: Option<String>,
    #[serde(default)]
    pub bindings: Vec<BindingDef>,
}

#[derive(Deserialize)]
struct RuleFile {
    #[allow(dead_code)]
    version: u32,
    rules: Vec<RuleDef>,
}

pub struct Compiled {
    pub def: RuleDef,
    pub origin: &'static str,
    pub enabled: bool,
    pub(crate) conds: Vec<Expr>,
    literals: Option<Vec<usize>>,
    pub(crate) window: i64,
    pub(crate) counts: Vec<usize>,
    by: Vec<FieldRef>,
    step_by: Vec<Vec<FieldRef>>,
    distinct: Option<FieldRef>,
    distinct_fallback: Vec<FieldRef>,
    attack: Vec<AttackRef>,
    arrays: Vec<(String, Expr)>,
    contrary: Option<Expr>,
    numerator: Option<Expr>,
    pub(crate) chain_references:Vec<String>,
}

impl Compiled {
    pub(crate) fn gated_condition(self) -> Result<Expr, String> {
        if self.def.kind != "single" || !self.arrays.is_empty() || self.contrary.is_some() {
            return Err(format!("Referência a agregado/condição contextual não suportada: {}", self.def.id));
        }
        let mut terms = vec![querylang::or(self.conds)];
        for field in &self.def.evidence.required {
            terms.push(querylang::term(Some(field), None, querylang::Spec::Exists)?);
        }
        if !self.def.evidence.products.is_empty() {
            terms.push(querylang::or(
                self.def
                    .evidence
                    .products
                    .iter()
                    .map(|product| {
                        querylang::term(Some("_sec.product"), None, querylang::Spec::Equals(product.clone()))
                    })
                    .collect::<Result<_, _>>()?,
            ));
        }
        Ok(querylang::and(terms))
    }
    fn contextual(&self, ev: &Event) -> bool {
        if ev.fields.get("_sec.literal_output").and_then(|v| v.as_str()) == Some("true") {
            return false;
        }
        if self.contrary.as_ref().is_some_and(|e| e.matches(ev)) {
            return false;
        }
        self.arrays.iter().all(|(field, expr)| {
            crate::security_normalize::field_value(ev, field).and_then(|v| v.as_array()).is_some_and(|array| {
                array.iter().any(|value| {
                    let Some(fields) = value.as_object() else {
                        return false;
                    };
                    expr.matches_object(fields)
                })
            })
        })
    }
    /// Any step of the rule (evidence filter for the detection).
    pub fn matches(&self, ev: &Event) -> bool {
        if !self.chain_references.is_empty(){return false;}
        let (view, _) = crate::security_normalize::normalize(ev, &[]);
        let ctx = Ctx::new(&view);
        self.contextual(&view) && self.conds.iter().any(|c| c.matches_ctx(&ctx))
    }
}

pub fn parse_duration(text: &str) -> Option<i64> {
    let t = text.trim().to_lowercase();
    let split = t.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(t.len());
    let (n, unit) = t.split_at(split);
    let n: f64 = n.parse().ok()?;
    let ms = match unit.trim() {
        "ms" => 1.0,
        "" | "s" | "sec" | "seg" => 1000.0,
        "m" | "min" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        _ => return None,
    };
    let v = (n * ms) as i64;
    (v > 0).then_some(v)
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

pub fn compile_rule(def: RuleDef, origin: &'static str, conds: Option<Vec<Expr>>) -> Result<Compiled, String> {
    def.evidence.validate()?;
    let kind = def.kind.as_str();
    if conds.is_none() && def.steps.iter().any(|s| !s.rules.is_empty()) {
        return Err("Referências devem ser resolvidas antes da compilação".into());
    }
    let arrays = def
        .any
        .iter()
        .map(|a| querylang::compile_rule(&a.condition).map(|e| (a.field.clone(), e)))
        .collect::<Result<Vec<_>, _>>()?;
    let contrary = if def.unless.trim().is_empty() { None } else { Some(querylang::compile_rule(&def.unless)?) };
    if !matches!(
        kind,
        "single" | "threshold" | "distinct" | "sequence" | "temporal" | "beacon" | "ratio" | "aggregate" | "absence"
    ) {
        return Err(format!("Tipo de regra inválido em {}: {kind}", def.id));
    }
    if severity_rank(&def.severity) == 0 && def.severity != "info" {
        return Err(format!("Severidade inválida em {}.", def.id));
    }
    let conds = match conds {
        Some(c) => c,
        None if matches!(kind, "sequence" | "temporal" | "absence") => {
            if def.steps.len() < 2 {
                return Err(format!("A sequência {} precisa de ao menos duas etapas.", def.id));
            }
            def.steps
                .iter()
                .map(|s| querylang::compile_rule(&s.condition).map_err(|e| format!("{}: {e}", def.id)))
                .collect::<Result<_, _>>()?
        }
        None => vec![querylang::compile_rule(&def.condition).map_err(|e| format!("{}: {e}", def.id))?],
    };
    let conds=if let Some(namespace)=def.provenance["proposal_namespace"].as_str(){conds.into_iter().map(|expr|Ok(querylang::and(vec![expr,querylang::term(Some("_sec.namespace"),None,querylang::Spec::Exact(namespace.into()))?]))).collect::<Result<Vec<_>,String>>()?}else{conds};
    let counts: Vec<usize> = if matches!(kind, "sequence" | "temporal" | "absence") {
        def.steps.iter().map(|s| s.count.unwrap_or(1).max(1)).collect()
    } else {
        vec![def.count.unwrap_or(1).max(1)]
    };
    if matches!(kind, "sequence" | "temporal")
        && (conds.len() != counts.len()
            || counts.len() > 16
            || counts.iter().any(|&n| n > 128)
            || counts.iter().sum::<usize>() > 512)
    {
        return Err("Correlação suporta até 16 etapas e 512 fatos obrigatórios; construção não simplificada".into());
    }
    let window = def
        .window
        .as_deref()
        .map(|w| parse_duration(w).ok_or_else(|| format!("Janela inválida em {}: {w}", def.id)))
        .transpose()?
        .unwrap_or(if kind == "single" { 3_600_000 } else { 600_000 });
    if kind != "single" && def.by.is_empty() {
        return Err(format!("A regra {} precisa de campos de agrupamento (by).", def.id));
    }
    if kind == "distinct" && def.distinct.is_none() {
        return Err(format!("A regra {} precisa do campo distinct.", def.id));
    }
    if kind == "absence" && (def.steps.len() != 2 || def.coverage.as_ref().is_none_or(|v| v.is_empty())) {
        return Err("absence exige âncora, evento esperado e categoria de cobertura explícita".into());
    }
    if !def.bindings.is_empty()
        && (kind != "sequence"
            || def.bindings.len() > 1
            || def.bindings.iter().any(|b| {
                b.field.is_empty()
                    || b.steps.len() < 2
                    || b.steps.iter().any(|&s| s >= def.steps.len())
                    || b.steps.iter().collect::<HashSet<_>>().len() != b.steps.len()
            }))
    {
        return Err(
            "bindings suporta um vínculo adicional por sequência, com ao menos duas etapas distintas válidas".into()
        );
    }
    let numerator = if kind == "ratio" {
        let r = def.ratio.as_ref().ok_or("ratio exige numerator e gte")?;
        if !r.gte.is_finite() || !(0.0..=1.0).contains(&r.gte) || def.count.unwrap_or(0) < 2 {
            return Err("ratio exige gte entre 0 e 1 e count mínimo >= 2".into());
        }
        Some(querylang::compile_rule(&r.numerator)?)
    } else {
        None
    };
    if kind == "aggregate" {
        let a = def.aggregate.as_ref().ok_or("aggregate exige field, operation e limite")?;
        if !["sum", "avg", "min", "max"].contains(&a.operation.as_str())
            || a.field.is_empty()
            || (a.gte.is_none() && a.lte.is_none())
            || a.gte.is_some_and(|n| !n.is_finite())
            || a.lte.is_some_and(|n| !n.is_finite())
            || a.gte.zip(a.lte).is_some_and(|(a, b)| a > b)
        {
            return Err("Agregação inválida; operações suportadas: sum, avg, min, max".into());
        }
    }
    let attack = def.attack.iter().map(|id| attack::reference(id, &def.tactics)).collect();
    Ok(Compiled {
        enabled: true,
        by: def.by.iter().map(|c| querylang::field_ref(c)).collect(),
        step_by: def
            .steps
            .iter()
            .map(|s| if s.by.is_empty() { &def.by } else { &s.by })
            .map(|by| by.iter().map(|c| querylang::field_ref(c)).collect())
            .collect(),
        distinct: def.distinct.as_deref().map(querylang::field_ref),
        distinct_fallback: def.distinct_fallback.iter().map(|c| querylang::field_ref(c)).collect(),
        literals: None,
        conds,
        window,
        counts,
        attack,
        origin,
        arrays,
        contrary,
        numerator,
        chain_references:def.provenance["chain_references"].as_array().map(|values|values.iter().filter_map(serde_json::Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        def,
    })
}

// ------------------------------------------------------------------ settings

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Suppression {
    pub rule: String,
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub expires: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub investigation: crate::investigation::Settings,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub suppress: Vec<Suppression>,
    /// Include threat-catalog text signals in the triage.
    #[serde(default = "yes")]
    pub threats: bool,
    #[serde(default)]
    pub mappings: Vec<crate::security_normalize::SourceMapping>,
    #[serde(default)]
    pub coverage: Vec<CoverageWindow>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            investigation: Default::default(),
            disabled: Vec::new(),
            suppress: Vec::new(),
            threats: true,
            mappings: Vec::new(),
            coverage: Vec::new(),
        }
    }
}

pub fn load_settings() -> Settings {
    crate::case_security::with(|snapshot| {
        snapshot
            .detection_settings()
            .expect("validated Case security")
    })
}

pub(crate) fn validate_settings(settings: &Settings, previous: &Settings) -> Result<(), String> {
    settings.investigation.validate()?;
    if settings.disabled.len() > 8192
        || settings.suppress.len() > 4096
        || settings.coverage.len() > 4096
    {
        return Err("Ajustes de segurança excedem os limites do Caso.".into());
    }
    crate::security_normalize::validate_mappings(&settings.mappings)?;
    // These timestamps are editable in the web view. Never round an i64 in
    // a settings receipt/editor; arbitrary rule provenance remains raw JSON.
    const EXACT_UI_INTEGER: i64 = 9_007_199_254_740_991;
    let exact = |value: i64| (-EXACT_UI_INTEGER..=EXACT_UI_INTEGER).contains(&value);
    if settings.suppress.iter().any(|s| !exact(s.created) || s.expires.is_some_and(|v| !exact(v)))
        || settings.coverage.iter().any(|c| !exact(c.start) || !exact(c.end)) {
        return Err("Datas de segurança excedem a faixa inteira exata do editor.".into());
    }
    for c in &settings.coverage {
        if c.dataset_fingerprint.is_empty()
            || c.source.is_empty()
            || c.category.is_empty()
            || c.namespace.is_empty()
            || c.start >= c.end
            || c.justification.trim().is_empty()
        {
            return Err(
                "Cobertura exige fonte, namespace, categoria, intervalo válido e justificativa"
                    .into(),
            );
        }
    }
    for exception in &settings.suppress {
        let unchanged = previous
            .suppress
            .iter()
            .any(|old| serde_json::to_value(old).ok() == serde_json::to_value(exception).ok());
        if !unchanged
            && (exception.note.trim().is_empty()
                || exception.scope.is_none()
                || exception.expires.is_none_or(|t| t <= exception.created))
        {
            return Err(
                "Exceções exigem justificativa, escopo e validade posterior à criação".into(),
            );
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ rule set

pub struct RuleSet {
    pub rules: Vec<Compiled>,
    plan: crate::security_plan::Plan,
    condition_nodes: Vec<Vec<usize>>,
    predicate_signature:Option<String>,
    automaton: Option<AhoCorasick>,
    pub sigma_loaded: usize,
    pub sigma_errors: Vec<String>,
}

impl RuleSet {
    pub fn find(&self, id: &str) -> Option<&Compiled> {
        self.rules.iter().find(|r| r.def.id == id)
    }
}

pub(crate) fn fingerprint() -> String {
    crate::case_security::with(|snapshot| snapshot.signature())
}

static RULESET: Mutex<Option<(String, Arc<RuleSet>)>> = Mutex::new(None);

/// Content-keyed immutable compilation from the admitted Case, never profile files.
pub fn ruleset() -> Result<Arc<RuleSet>, String> {
    crate::case_security::with(ruleset_for)
}
pub(crate) fn ruleset_for(
    snapshot: &crate::case_security::Settings,
) -> Result<Arc<RuleSet>, String> {
    let key = snapshot.signature();
    if let Some((cached, set)) = RULESET.lock().as_ref() {
        if *cached == key {
            return Ok(set.clone());
        }
    }
    crate::case_security::compiling(snapshot, || {
        let settings = snapshot.detection_settings()?;
        let disabled: HashSet<&str> = settings.disabled.iter().map(String::as_str).collect();
        let mut defs = builtin_defs()?;
        let mut custom_ids = HashSet::new();
        if let Some(text) = &snapshot.custom_rules_json {
            let local: RuleFile =
                serde_json::from_str(text).map_err(|e| format!("Regras do Caso inválidas: {e}"))?;
            if local.version != 1 || local.rules.len() > 1024 {
                return Err("Regras do Caso exigem versão 1 e até 1.024 definições.".into());
            }
            for rule in local.rules {
                crate::operations::check()?;
                if rule.id.is_empty() || rule.id.len() > 160 || !custom_ids.insert(rule.id.clone())
                {
                    return Err("ID de regra vazio, extenso ou duplicado.".into());
                }
                match defs.iter_mut().find(|d| d.id == rule.id) {
                    Some(existing) => *existing = rule,
                    None => defs.push(rule),
                }
            }
        }
        let sigma = if snapshot.sigma_sources.is_empty() {
            Vec::new()
        } else {
            crate::sigma::convert_texts(snapshot.sigma_sources.iter().map(|s| s.text.as_str()))?
        };
        let mut set = build(defs, sigma, Vec::new(), &disabled)?;
        for rule in &mut set.rules {
            if rule.origin == "builtin" && custom_ids.contains(&rule.def.id) {
                rule.origin = "case";
            }
        }
        let set = Arc::new(set);
        crate::operations::check()?;
        *RULESET.lock() = Some((key, set.clone()));
        Ok(set)
    })
}

/// Built-in rules only.
#[cfg(test)]
pub fn builtin_ruleset() -> Result<RuleSet, String> {
    let defs = builtin_defs()?;
    build(defs, Vec::new(), Vec::new(), &HashSet::new())
}

#[cfg(test)]
pub(crate) fn test_ruleset(defs: Vec<RuleDef>, sigma: Vec<Compiled>) -> Result<RuleSet, String> {
    build(defs, sigma, vec![], &HashSet::new())
}

fn build(
    defs: Vec<RuleDef>,
    sigma: Vec<Compiled>,
    sigma_errors: Vec<String>,
    disabled: &HashSet<&str>,
) -> Result<RuleSet, String> {
    let mut rules = Vec::new();
    for def in &defs {
        crate::operations::check()?;
        let enabled = def.enabled && !disabled.contains(def.id.as_str());
        let mut rule = resolve_rule(def, &defs, &mut Vec::new())?;
        rule.enabled = enabled;
        rules.push(rule);
    }
    let sigma_loaded = sigma.len();
    for mut rule in sigma {
        rule.enabled = !disabled.contains(rule.def.id.as_str());
        rules.push(rule);
    }
    // Literal prefilter: one automaton over the literals required by each rule.
    let mut patterns: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for rule in &mut rules {
        let mut required: Option<Vec<String>> = None;
        for cond in &rule.conds {
            match cond.required_literals() {
                Some(list) => required.get_or_insert_with(Vec::new).extend(list),
                None => {
                    required = None;
                    break;
                }
            }
        }
        rule.literals = required.map(|list| {
            list.into_iter()
                .map(|lit| {
                    *index.entry(lit.clone()).or_insert_with(|| {
                        patterns.push(lit);
                        patterns.len() - 1
                    })
                })
                .collect()
        });
    }
    let automaton = if patterns.is_empty() {
        None
    } else {
        Some(AhoCorasick::new(&patterns).map_err(|e| format!("Pré-filtro de regras: {e}"))?)
    };
    let mut plan = crate::security_plan::Plan::default();
    let condition_nodes = rules.iter().map(|rule| rule.conds.iter().map(|expr| plan.add(expr)).collect()).collect();
    let predicate_signature=plan.signature();
    Ok(RuleSet { rules, plan, condition_nodes, predicate_signature,automaton, sigma_loaded, sigma_errors })
}

fn resolve_rule(def: &RuleDef, all: &[RuleDef], stack: &mut Vec<String>) -> Result<Compiled, String> {
    if stack.contains(&def.id) {
        return Err(format!("Dependência circular: {}", def.id));
    }
    stack.push(def.id.clone());
    let conditions = if def.steps.iter().any(|s| !s.rules.is_empty()) {
        let mut conditions = Vec::new();
        for step in &def.steps {
            let mut referenced = Vec::new();
            for id in &step.rules {
                let source =
                    all.iter().find(|r| &r.id == id).ok_or_else(|| format!("Regra referenciada ausente: {id}"))?;
                let compiled = resolve_rule(source, all, stack)?;
                referenced.push(compiled.gated_condition()?);
            }
            let mut expr = vec![querylang::compile_rule(&step.condition)?];
            if !referenced.is_empty() {
                expr.push(querylang::or(referenced));
            }
            conditions.push(querylang::and(expr));
        }
        Some(conditions)
    } else {
        None
    };
    stack.pop();
    compile_rule(def.clone(), "builtin", conditions)
}

pub fn invalidate() {
    *RULESET.lock() = None;
}

// ------------------------------------------------------------------ output

#[derive(Clone, Serialize, Deserialize)]
pub struct EntityRef {
    pub column: String,
    pub label: String,
    pub value: String,
}

#[derive(Clone, Serialize)]
pub struct Detection {
    pub participants: crate::security_participants::Participants,
    /// Internal source boundary for presentation grouping; original evidence is unchanged.
    #[serde(skip)]
    pub pattern_source: Option<String>,
    pub id: String,
    pub rule: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub origin: String,
    pub kind: String,
    pub attack: Vec<AttackRef>,
    pub tactics: Vec<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub count: usize,
    pub entities: Vec<EntityRef>,
    pub summary: String,
    pub distinct: usize,
    pub period_ms: Option<i64>,
    pub event_ids: Vec<usize>,
    /// Filters reproducing the supporting records (the time range is separate).
    pub filters: Vec<crate::query::Filter>,
    #[serde(flatten)]
    pub evidence: crate::evidence::Evidence,
    pub namespace: String,
    pub signal_rules: Vec<String>,
    pub measurements: serde_json::Value,
}

#[derive(Serialize)]
pub struct Episode {
    pub participants: crate::security_participants::Participants,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grouping: Option<crate::security_grouping::Grouping>,
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: String,
    pub score: u32,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub detections: Vec<usize>,
    pub evidence_level: u8,
    pub event_refs: Vec<String>,
    pub tactics: Vec<String>,
    pub entities: Vec<EntityRef>,
}

#[derive(Serialize)]
pub struct EntityRisk {
    pub namespace: String,
    pub column: String,
    pub label: String,
    pub value: String,
    pub score: u32,
    pub evidence_level: u8,
    pub level: String,
    pub detections: usize,
    pub events: usize,
    pub failures: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub scope: Option<String>,
    pub tactics: Vec<String>,
}

#[derive(Serialize)]
pub struct RareValue {
    pub column: String,
    pub label: String,
    pub value: String,
    pub count: usize,
    pub first: Option<i64>,
    pub event_id: usize,
    pub role_events: usize,
    pub role_distinct: usize,
    pub filter: crate::query::Filter,
}

#[derive(Serialize)]
pub struct TechniqueCount {
    pub id: String,
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct TacticSummary {
    pub id: String,
    pub key: String,
    pub label: String,
    pub count: usize,
    pub techniques: Vec<TechniqueCount>,
}

#[derive(Serialize)]
pub struct RoleCoverage {
    pub column: String,
    pub label: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct Triage {
    pub investigation: serde_json::Value,
    pub execution: serde_json::Value,
    pub dataset_fingerprint: String,
    pub total: usize,
    pub undated: usize,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub complete: bool,
    pub limited: bool,
    pub detections: Vec<Detection>,
    pub episodes: Vec<Episode>,
    pub entities: Vec<EntityRisk>,
    pub rare: Vec<RareValue>,
    pub tactics: Vec<TacticSummary>,
    pub coverage: Vec<RoleCoverage>,
    pub suppressed: usize,
    pub rules: usize,
    pub sigma_rules: usize,
    pub sigma_errors: Vec<String>,
    pub threat_rules: usize,
    pub elapsed_ms: u64,
    pub analysis_id: String,
    pub attack_version: String,
    pub policy_version: String,
    pub normalization_version: String,
    pub counts_by_level: [usize; 5],
    pub rule_coverage: Vec<RuleCoverage>,
    pub limitations: Vec<String>,
    pub duplicates: usize,
}

#[derive(Serialize)]
pub struct RuleCoverage {
    pub rule: String,
    pub unit: String,
    pub status: String,
    pub applicable: usize,
    pub eligible: usize,
    pub missing: Vec<String>,
}

// ------------------------------------------------------------------ pass

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Hit {
    pub(crate) ts: i64,
    pub(crate) id: usize,
    pub(crate) step: u8,
    pub(crate) extra: Option<Box<str>>,
    #[serde(default)]
    pub(crate) reference: String,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) uncertain_time: bool,
    #[serde(default)]
    pub(crate) binding: Option<String>,
}

#[derive(Default, Clone)]
struct EntityStat {
    events: usize,
    failures: usize,
    first: Option<i64>,
    last: Option<i64>,
}

#[derive(Default, Clone)]
struct RareStat {
    count: usize,
    first: Option<i64>,
    sample: usize,
}

const ENTITY_ROLES: [Role; 4] = [Role::User, Role::SrcIp, Role::Host, Role::DstIp];
const RARE_ROLES: [(Role, &str); 5] = [
    (Role::Process, "@process"),
    (Role::ParentProcess, "@parent_process"),
    (Role::UserAgent, "@user_agent"),
    (Role::DstPort, "@dst_port"),
    (Role::Domain, "@domain"),
];
const COVERAGE_ROLES: [Role; 9] = [
    Role::User,
    Role::SrcIp,
    Role::DstIp,
    Role::Host,
    Role::Process,
    Role::CommandLine,
    Role::Url,
    Role::Action,
    Role::Outcome,
];

struct Acc {
    population: crate::investigation::Population,
    facts: crate::security_facts::Facts,
    predicate_memo: Vec<Option<bool>>,
    columnar_batches:usize,
    columnar_rows:usize,
    columnar_reused_batches:usize,
    columnar_leaves:usize,
    fingerprint: sha2::Sha256,
    groups: crate::security_store::Groups<Hit>,
    hits: usize,
    limited: bool,
    entities: [HashMap<Box<str>, EntityStat>; 4],
    rare: [HashMap<Box<str>, RareStat>; 5],
    rare_totals: [usize; 5],
    coverage: [usize; 9],
    total: usize,
    undated: usize,
    first: Option<i64>,
    last: Option<i64>,
    stamps: Vec<u32>,
    stamp: u32,
    eligible: Vec<usize>,
    applicable: Vec<usize>,
    missing: Vec<HashSet<String>>,
    uncertain_absence: HashSet<usize>,
    reconstructed: crate::security_reconstruct::Members,
    duplicates: usize,
    clipped: usize,
    stats_bytes: usize,
}

impl Acc {
    fn new(patterns: usize, mappings: &[crate::security_normalize::SourceMapping], predicates: usize, behavior: &crate::investigation::Settings) -> Result<Self, String> {
        Ok(Acc {
            population: crate::investigation::Population::new(behavior)?,
            facts: crate::security_facts::Facts::new(mappings)?,
            predicate_memo: vec![None; predicates],
            columnar_batches:0,columnar_rows:0,columnar_reused_batches:0,columnar_leaves:0,
            fingerprint: Default::default(),
            groups: Default::default(),
            hits: 0,
            limited: false,
            entities: Default::default(),
            rare: Default::default(),
            rare_totals: [0; 5],
            coverage: [0; 9],
            total: 0,
            undated: 0,
            first: None,
            last: None,
            stamps: vec![0; patterns],
            stamp: 0,
            eligible: vec![],
            applicable: vec![],
            missing: vec![],
            uncertain_absence: HashSet::new(),
            reconstructed: crate::security_reconstruct::Members::new()?,
            duplicates: 0,
            clipped: 0,
            stats_bytes: 0,
        })
    }
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}
fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn basename(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

fn event_text(ev: &Event) -> String {
    let mut text = String::with_capacity(ev.message.len() + ev.raw.len().min(4096) + 64);
    for part in [&ev.message, &ev.source, &ev.code, &ev.raw] {
        text.push_str(part);
        text.push('\n');
    }
    for (key, value) in &ev.fields {
        if key == "arquivo" || key == "caminho" {
            continue;
        }
        match value {
            serde_json::Value::String(s) => text.push_str(s),
            serde_json::Value::Null => {}
            other => text.push_str(&other.to_string()),
        }
        text.push('\n');
    }
    text.to_lowercase()
}

struct Pass<'a> {
    full_entity_stats:bool,
    behavior: &'a crate::investigation::Settings,
    rules: &'a RuleSet,
    catalog: Option<&'a CompiledCatalog>,
    mappings: &'a [crate::security_normalize::SourceMapping],
    /// Category index per threat rule (synthetic rule ids follow the real ones).
    threat_category: Vec<u32>,
}

impl Pass<'_> {
    fn flush_predicates(&self, acc:&mut Acc, columnar:&mut crate::security_plan::Columnar,
        events:&mut Vec<Event>, normal:&mut Vec<crate::security_normalize::Normalized>,
        keys:&mut Vec<String>, memos:&mut Vec<Vec<Option<bool>>>) -> Result<(),String> {
        if events.is_empty(){return Ok(());}
        crate::security_budget::phase("predicate vectors");
        columnar.evaluate(&acc.population.db,events,memos)?;
        for (((event,normalized),key),memo) in events.drain(..).zip(normal.drain(..)).zip(keys.drain(..)).zip(memos.drain(..)) {
            acc.facts.select_key(key);acc.predicate_memo=memo;
            crate::security_budget::phase("normal facts and rule hits");
            acc.population.observe(&event,&normalized)?;self.step(acc,&event,&normalized)?;
        }
        acc.columnar_batches=columnar.batches;acc.columnar_rows=columnar.rows;acc.columnar_reused_batches=columnar.reused_batches;acc.columnar_leaves=columnar.leaves();Ok(())
    }
    fn group_key(&self, ctx: &Ctx<'_>, by: &[FieldRef], required: bool) -> Option<Box<str>> {
        let mut values = Vec::new();
        for field in by {
            match ctx.get(field) {
                Some(v) if !v.trim().is_empty() => values.push(v),
                _ if required => return None,
                _ => values.push("".into()),
            }
        }
        Some(serde_json::to_string(&values).ok()?.into_boxed_str())
    }

    fn step(
        &self,
        acc: &mut Acc,
        ev: &Event,
        normalized: &crate::security_normalize::Normalized,
    ) -> Result<(), String> {
        use sha2::Digest;
        crate::security_budget::check()?;
        acc.fingerprint.update((ev.event_ref.len() as u64).to_le_bytes());
        acc.fingerprint.update(ev.event_ref.as_bytes());
        acc.fingerprint.update(acc.facts.content_key().as_bytes());
        let ctx = Ctx::new(ev);
        acc.total += 1;
        acc.clipped += usize::from(normalized.content_clipped);
        match ev.timestamp {
            Some(t) => {
                acc.first = min_opt(acc.first, Some(t));
                acc.last = max_opt(acc.last, Some(t));
            }
            None => acc.undated += 1,
        }
        let ts = ev.timestamp.unwrap_or(i64::MIN);
        // Literal prefilter shared by every rule that declares required text.
        if let Some(automaton) = &self.rules.automaton {
            acc.stamp = acc.stamp.wrapping_add(1);
            if acc.stamp == 0 {
                acc.stamps.iter_mut().for_each(|s| *s = 0);
                acc.stamp = 1;
            }
            let text = event_text(ev);
            for found in automaton.find_overlapping_iter(&text) {
                acc.stamps[found.pattern().as_usize()] = acc.stamp;
            }
        }
        for (index, rule) in self.rules.rules.iter().enumerate() {
            if !rule.enabled {
                continue;
            }
            if !rule.chain_references.is_empty(){continue;}
            let policy = &rule.def.evidence;
            if !policy.products.is_empty() && !policy.products.contains(&normalized.product) {
                continue;
            }
            acc.applicable[index] += 1;
            let absent: Vec<_> = policy
                .required
                .iter()
                .filter(|f| ctx.get(&querylang::field_ref(f)).is_none_or(|v| v.is_empty()))
                .cloned()
                .collect();
            // An observed expected event that cannot be joined is not evidence
            // of absence. Fail closed for this rule, including missing namespaces.
            if rule.def.kind == "absence"
                && rule.contextual(ev)
                && rule.conds[1].matches_ctx(&ctx)
                && (!absent.is_empty() || self.group_key(&ctx, rule.step_by.get(1).unwrap_or(&rule.by), true).is_none())
            {
                acc.uncertain_absence.insert(index);
            }
            if !absent.is_empty() {
                acc.missing[index].extend(absent);
                continue;
            }
            if rule.def.kind != "single" {
                if ev.timestamp.is_none() && rule.def.kind != "absence" {
                    acc.missing[index].insert("timestamp".into());
                    continue;
                }
                let keys: Vec<_> = if rule.step_by.is_empty() { vec![&rule.by] } else { rule.step_by.iter().collect() };
                if !keys.iter().any(|key| self.group_key(&ctx, key, true).is_some()) {
                    acc.missing[index].extend(rule.def.by.iter().cloned());
                    continue;
                }
            }
            acc.eligible[index] += 1;
            if !rule.contextual(ev) {
                continue;
            }
            if let Some(literals) = &rule.literals {
                if !literals.iter().any(|&l| acc.stamps[l] == acc.stamp) {
                    continue;
                }
            }
            let kind = rule.def.kind.as_str();
            for (step, _cond) in rule.conds.iter().enumerate() {
                if !self.rules.plan.matches(self.rules.condition_nodes[index][step], &ctx, &mut acc.predicate_memo) {
                    continue;
                }
                if kind != "single" && kind != "absence" && ev.timestamp.is_none() {
                    break;
                }
                let Some(key) = self.group_key(&ctx, rule.step_by.get(step).unwrap_or(&rule.by), kind != "single")
                else {
                    break;
                };
                let key = serde_json::to_string(&(normalized.get("namespace").unwrap_or(""), key.as_ref()))
                    .map_err(|e| e.to_string())?;
                let extra = match kind {
                    "ratio" => Some(if rule.numerator.as_ref().unwrap().matches_ctx(&ctx) { "1" } else { "0" }.into()),
                    "aggregate" => {
                        let field = querylang::field_ref(&rule.def.aggregate.as_ref().unwrap().field);
                        let value = ctx.get(&field).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
                        match value {
                            Some(v) => Some(v.to_string().into()),
                            None => {
                                acc.missing[index].insert(rule.def.aggregate.as_ref().unwrap().field.clone());
                                continue;
                            }
                        }
                    }
                    "distinct" => {
                        let value = rule
                            .distinct
                            .as_ref()
                            .and_then(|f| ctx.get(f))
                            .or_else(|| rule.distinct_fallback.iter().find_map(|f| ctx.get(f)));
                        match value {
                            Some(v) if !v.trim().is_empty() => {
                                Some(v.trim().chars().take(512).collect::<String>().into_boxed_str())
                            }
                            _ => break,
                        }
                    }
                    _ => None,
                };
                acc.hits += 1;
                let binding = if let Some(b) = rule.def.bindings.first().filter(|b| b.steps.contains(&step)) {
                    let field = querylang::field_ref(&b.field);
                    match ctx.get(&field).filter(|v| !v.is_empty()) {
                        Some(v) => Some(v.to_string()),
                        None => {
                            acc.missing[index].insert(b.field.clone());
                            continue;
                        }
                    }
                } else {
                    None
                };
                let hit = Hit {
                    ts,
                    id: ev.id,
                    step: step as u8,
                    extra,
                    reference: ev.event_ref.clone(),
                    source: ev.source.clone(),
                    uncertain_time: normalized.time.ambiguity.is_some() || ev.timestamp.is_none(),
                    binding,
                };
                if kind == "single" {
                    acc.groups.push(index as u32, &serde_json::to_string(&(&key, &ev.event_ref)).unwrap(), hit)?;
                } else if kind == "absence" {
                    acc.groups.push(
                        index as u32,
                        &serde_json::to_string(&(&key, Option::<i64>::None)).unwrap(),
                        hit,
                    )?;
                } else {
                    // A bucket plus its successor covers every window starting
                    // in that bucket. Findings belong to their first member's bucket.
                    let bucket = ts.div_euclid(rule.window);
                    acc.groups.push(
                        index as u32,
                        &serde_json::to_string(&(&key, Some(bucket))).unwrap(),
                        hit.clone(),
                    )?;
                    acc.groups.push(
                        index as u32,
                        &serde_json::to_string(&(&key, Some(bucket.saturating_sub(1)))).unwrap(),
                        hit,
                    )?;
                }
            }
        }
        if let Some(catalog) =
            self.catalog.filter(|_| ev.fields.get("_sec.literal_output").and_then(|v| v.as_str()) != Some("true"))
        {
            let (found, clipped) = catalog.event_hits_checked(ev);
            acc.clipped += usize::from(clipped && !normalized.content_clipped);
            if !found.is_empty() {
                let explained = found.iter().any(|&r| catalog.rule(r).id.starts_with("web.")).then(||catalog.explain(ev));
                let key: Box<str> = [(Role::SrcIp, "@src_ip"), (Role::Host, "@host"), (Role::User, "@user")]
                    .iter()
                    .find_map(|(r, column)| ctx.role(*r).map(|v| serde_json::to_string(&[column, &v]).unwrap()))
                    .unwrap_or_default()
                    .into();
                let key = serde_json::to_string(&(normalized.get("namespace").unwrap_or(""), key.as_ref())).unwrap();
                let base = self.rules.rules.len() as u32;
                let mut seen = HashSet::new();
                for rule in found {
                    if !crate::threats::reviewed_indication(catalog.rule(rule)) {
                        continue;
                    }
                    let id = catalog.rule(rule).id.as_str();
                    if let Some(hit) = explained.as_ref().and_then(|v|v.iter().find(|h|h.rule==rule)) {
                        let original = match hit.provenance.field.as_str() {
                            "message"=>Some(ev.message.as_str()), "raw"=>Some(ev.raw.as_str()),
                            field=>crate::security_normalize::field_value(ev,field).and_then(|v|v.as_str()),
                        };
                        if original.is_some_and(|v| crate::security_normalize::literal_output(v)
                            || ["documentation:","example:","sample payload:","exemplo:","documentação:"].iter().any(|p|v.trim_start().to_lowercase().starts_with(p))) {continue;}
                        let request = hit.provenance.direction == "request" || normalized.values.get("url").is_some_and(|p| p.field == hit.provenance.field && p.method.starts_with("URI extraída"));
                        // A SQL query or a script returned by an application is not, by itself, an attack.
                        if id.starts_with("web.") && hit.provenance.direction == "response" {continue;}
                        if id.starts_with("web.sql.") && !request {continue;}
                        if id == "web.xss.remote-script" && !request {continue;}
                    }
                    // A contextual finding supersedes the equivalent legacy textual signal.
                    if normalized.signals.iter().any(|s| {
                        (id.starts_with("web.xss.") && ["_sec.content.xss_request", "_sec.content.xss_payload"].contains(&s.key.as_str()))
                        || (id.starts_with("web.sql.") && ["_sec.content.sqli_request", "_sec.content.sqli_payload"].contains(&s.key.as_str()))
                        || (id.starts_with("net.reverse.") && s.key == "_sec.content.reverse_payload")
                        || (id.starts_with("web.path.") && ["_sec.content.sensitive_request", "_sec.content.traversal_request"].contains(&s.key.as_str()))
                    }) { continue; }
                    let category = self.threat_category[rule];
                    let extra: Box<str> = catalog.rule(rule).id.clone().into();
                    if !seen.insert((category, rule)) {
                        continue;
                    }
                    acc.hits += 1;
                    acc.groups.push(
                        base + category,
                        &serde_json::to_string(&(&key, &ev.event_ref)).unwrap(),
                        Hit {
                            ts,
                            id: ev.id,
                            step: 0,
                            extra: Some(extra),
                            reference: ev.event_ref.clone(),
                            source: ev.source.clone(),
                            uncertain_time: normalized.time.ambiguity.is_some(),
                            binding: None,
                        },
                    )?;
                }
            }
        }
        let failure = ctx.role(Role::Outcome) == Some("failure");
        for (slot, role) in ENTITY_ROLES.iter().enumerate().filter(|_|!self.full_entity_stats) {
            let Some(value) = ctx.role(*role) else { continue };
            let value = serde_json::to_string(&(normalized.get("namespace").unwrap_or(""), value)).unwrap();
            let map = &mut acc.entities[slot];
            if !map.contains_key(value.as_str()) {
                if value.len() > 4096 || acc.stats_bytes.saturating_add(value.len() + 192) > 32 * 1024 * 1024 {
                    acc.limited = true;
                    continue;
                }
                acc.stats_bytes += value.len() + 192;
            }
            if map.len() >= ENTITY_KEYS && !map.contains_key(value.as_str()) {
                acc.limited = true;
                continue;
            }
            let entry = map.entry(value.into_boxed_str()).or_default();
            entry.events += 1;
            entry.failures += usize::from(failure);
            if ev.timestamp.is_some() {
                entry.first = min_opt(entry.first, ev.timestamp);
                entry.last = max_opt(entry.last, ev.timestamp);
            }
        }
        for (slot, (role, _)) in RARE_ROLES.iter().enumerate() {
            let Some(value) = ctx.role(*role) else { continue };
            let value: String = match role {
                Role::Process | Role::ParentProcess => basename(value),
                Role::Domain => value.to_lowercase(),
                _ => value.chars().take(300).collect(),
            };
            acc.rare_totals[slot] += 1;
            let map = &mut acc.rare[slot];
            if !map.contains_key(value.as_str()) {
                if acc.stats_bytes.saturating_add(value.len() + 192) > 32 * 1024 * 1024 {
                    acc.limited = true;
                    continue;
                }
                acc.stats_bytes += value.len() + 192;
            }
            if map.len() >= RARE_KEYS && !map.contains_key(value.as_str()) {
                continue;
            }
            let entry = map.entry(value.into_boxed_str()).or_default();
            if entry.count == 0 || ev.timestamp.is_some_and(|t| entry.first.is_none_or(|f| t < f)) {
                entry.sample = ev.id;
            }
            entry.count += 1;
            entry.first = min_opt(entry.first, ev.timestamp);
        }
        for (slot, role) in COVERAGE_ROLES.iter().enumerate() {
            if ctx.role(*role).is_some() {
                acc.coverage[slot] += 1;
            }
        }
        if let Some(signature)=&self.rules.predicate_signature {acc.facts.save_predicates(signature,&self.rules.plan.cache_values(&acc.predicate_memo))?;}
        Ok(())
    }
}

// ------------------------------------------------------------------ windows

/// Maximal index ranges where `ok(i, j)` holds for a trailing window.
fn threshold_ranges(hits: &[Hit], window: i64, count: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    for j in 0..hits.len() {
        while hits[j].ts - hits[i].ts > window {
            i += 1;
        }
        if j + 1 - i >= count {
            match out.last_mut() {
                Some(last) if i <= last.1 + 1 => last.1 = j,
                _ => out.push((i, j)),
            }
        }
    }
    out
}

fn distinct_ranges(hits: &[Hit], window: i64, count: usize) -> Vec<(usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let mut values: HashMap<&str, usize> = HashMap::new();
    let mut i = 0;
    for j in 0..hits.len() {
        *values.entry(hits[j].extra.as_deref().unwrap_or("")).or_default() += 1;
        while hits[j].ts - hits[i].ts > window {
            let key = hits[i].extra.as_deref().unwrap_or("");
            if let Some(n) = values.get_mut(key) {
                *n -= 1;
                if *n == 0 {
                    values.remove(key);
                }
            }
            i += 1;
        }
        if values.len() >= count {
            match out.last_mut() {
                Some(last) if i <= last.1 + 1 => {
                    last.1 = j;
                    last.2 = last.2.max(values.len());
                }
                _ => out.push((i, j, values.len())),
            }
        }
    }
    out
}

// Each member is a distinct recorded fact. Equal timestamps cannot prove order.
fn sequence_matches(hits: &[Hit], window: i64, counts: &[usize]) -> Vec<Vec<usize>> {
    ordered_matches(hits, window, counts, true)
}
fn temporal_matches(hits: &[Hit], window: i64, counts: &[usize]) -> Vec<Vec<usize>> {
    ordered_matches(hits, window, counts, false)
}
fn ordered_matches(hits: &[Hit], window: i64, counts: &[usize], ordered: bool) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut queues: Vec<std::collections::VecDeque<usize>> = vec![Default::default(); counts.len()];
    for (i, hit) in hits.iter().enumerate() {
        for q in &mut queues {
            while q.front().is_some_and(|&j| hit.ts.saturating_sub(hits[j].ts) > window) {
                q.pop_front();
            }
        }
        queues[hit.step as usize].push_back(i);
        if queues.iter().zip(counts).any(|(q, c)| q.len() < *c) {
            continue;
        }
        if !ordered {
            // A fact can match several selectors. Bipartite assignment avoids
            // consuming the only fact available to another stage.
            fn assign<'a>(
                slot: usize,
                stages: &[usize],
                queues: &[std::collections::VecDeque<usize>],
                hits: &'a [Hit],
                owners: &mut HashMap<&'a str, (usize, usize)>,
                visited: &mut HashSet<&'a str>,
            ) -> bool {
                for &j in &queues[stages[slot]] {
                    let reference = hits[j].reference.as_str();
                    if !visited.insert(reference) {
                        continue;
                    }
                    let previous = owners.get(reference).copied();
                    if previous.is_none() || assign(previous.unwrap().0, stages, queues, hits, owners, visited) {
                        owners.insert(reference, (slot, j));
                        return true;
                    }
                }
                false
            }
            let stages: Vec<_> =
                counts.iter().enumerate().flat_map(|(stage, count)| std::iter::repeat_n(stage, *count)).collect();
            let mut owners = HashMap::new();
            if (0..stages.len()).all(|slot| assign(slot, &stages, &queues, hits, &mut owners, &mut HashSet::new())) {
                let mut members: Vec<_> = owners.into_values().map(|(_, j)| j).collect();
                members.sort_unstable();
                out.push(members);
                for q in &mut queues {
                    q.clear();
                }
            }
            continue;
        }
        let mut members = Vec::new();
        let mut used = HashSet::new();
        let mut previous = i64::MIN;
        let mut complete = true;
        for (q, count) in queues.iter().zip(counts) {
            let mut stage = Vec::new();
            for &j in q {
                if (!ordered || hits[j].ts > previous) && used.insert(&hits[j].reference) {
                    stage.push(j);
                    if stage.len() == *count {
                        break;
                    }
                }
            }
            if stage.len() != *count {
                complete = false;
                break;
            }
            previous = hits[*stage.last().unwrap()].ts;
            members.extend(stage);
        }
        if complete {
            members.sort_unstable();
            out.push(members);
            for q in &mut queues {
                q.clear();
            }
        }
    }
    out
}

fn beacon_period(hits: &[Hit], count: usize) -> Option<(i64, f64)> {
    if hits.len() < count.max(6) {
        return None;
    }
    let mut gaps: Vec<i64> = hits.windows(2).map(|w| w[1].ts - w[0].ts).filter(|g| *g > 0).collect();
    if gaps.len() + 1 < count.max(6) {
        return None;
    }
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2];
    if median < 10_000 {
        return None;
    }
    let tolerance = (median as f64 * 0.2).max(2_000.0);
    let regular = gaps.iter().filter(|g| ((**g - median) as f64).abs() <= tolerance).count();
    let ratio = regular as f64 / gaps.len() as f64;
    (ratio >= 0.8).then_some((median, ratio))
}

fn format_period(ms: i64) -> String {
    if ms < 60_000 {
        format!("{} s", ms / 1000)
    } else if ms < 3_600_000 {
        let m = ms as f64 / 60_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{} min", m.round())
        } else {
            format!("{m:.1} min")
        }
    } else {
        format!("{:.1} h", ms as f64 / 3_600_000.0)
    }
}

// ------------------------------------------------------------------ assembly

#[derive(Serialize, Deserialize)]
struct Raw {
    rule: u32,
    key: Box<str>,
    members: Vec<usize>,
    #[serde(default)]
    external: Option<crate::security_correlate::Selection>,
    distinct: usize,
    period: Option<i64>,
    extras: Vec<String>,
    metrics: serde_json::Value,
}
fn raw_members(raw:&Raw,external:&crate::security_correlate::Store,mut visit:impl FnMut(usize,usize)->Result<(),String>)->Result<(),String>{if let Some(selection)=&raw.external{external.members(selection,visit)}else{for &id in &raw.members{visit(id,0)?;}Ok(())}}
fn external_raws(store:&crate::security_correlate::Store,cache:&mut crate::security_checkpoints::Cache,inputs:&Inputs<'_>,acc:&Acc,dataset:&str,base:u32)->Result<crate::security_store::Spool<Raw>,String>{
    let mut raws=crate::security_store::Spool::new()?;
    store.keys(|index,key|{
        let group=store.group(index,key)?;
        let (key,partition)=if index>=base||inputs.rules.rules[index as usize].def.kind=="single"{(serde_json::from_str::<(String,String)>(key).map_err(|e|e.to_string())?.0,None)}else{serde_json::from_str::<(String,Option<i64>)>(key).map_err(|e|e.to_string())?};
        if index>=base {
            let mut left=0;while left<group.len{let first=group.get(left)?;let mut right=left;let mut extras=Vec::new();while right<group.len && group.get(right)?.reference==first.reference{if let Some(extra)=group.get(right)?.extra{extras.push(extra.to_string());}right+=1;}extras.sort();extras.dedup();raws.push(Raw{rule:index,key:key.clone().into_boxed_str(),members:vec![first.id],external:None,distinct:0,period:None,extras,metrics:serde_json::Value::Null})?;left=right;}return Ok(());
        }
        let rule=&inputs.rules.rules[index as usize];
        if rule.def.kind=="single" {for n in 0..group.len{raws.push(Raw{rule:index,key:key.clone().into_boxed_str(),members:vec![group.get(n)?.id],external:None,distinct:0,period:None,extras:vec![],metrics:serde_json::Value::Null})?;}return Ok(());}
        let context=if rule.def.kind=="absence"{serde_json::to_string(&(dataset,&inputs.settings.coverage)).map_err(|e|e.to_string())?}else{crate::analysis_runtime::fact_interpretation_signature()};
        let checkpoint_key=group.signature(&serde_json::to_string(&rule.def).map_err(|e|e.to_string())?,&format!("{key}:{partition:?}"),&context)?;
        if let Some(saved)=cache.load(&checkpoint_key)?{for value in saved.into_iter()?{let value=value?;let selection=group.restore(&value["checkpoint_ranges"])?;let mut raw:Raw=serde_json::from_value(value).map_err(|e|e.to_string())?;raw.rule=index;raw.external=Some(selection);raws.push(raw)?;}return Ok(());}
        cache.begin(&checkpoint_key)?;
        let mut emit=|selection:crate::security_correlate::Selection,distinct:usize,period:Option<i64>,metrics:serde_json::Value|->Result<(),String>{let ranges=group.ranges(&selection)?;let raw=Raw{rule:index,key:key.clone().into_boxed_str(),members:vec![],external:Some(selection),distinct,period,extras:vec![],metrics};cache.push(&checkpoint_key,&serde_json::to_value(&raw).map_err(|e|e.to_string())?,ranges)?;raws.push(raw)};
        let mut emit_range=|left:usize,right:usize,distinct:usize,period:Option<i64>,metrics:serde_json::Value|->Result<(),String>{if partition.is_some_and(|b|group.get(left).is_ok_and(|h|h.ts.div_euclid(rule.window)!=b)){return Ok(());}emit(group.range(left,right)?,distinct,period,metrics)};
        match rule.def.kind.as_str(){
            "threshold"|"distinct"=>group.threshold(rule.window,rule.counts[0],rule.def.kind=="distinct",|left,right,n|emit_range(left,right,n,None,serde_json::Value::Null))?,
            "ratio"|"aggregate"=>group.measured(rule,partition,|left,right,metrics|emit(group.range(left,right)?,0,None,metrics))?,
            "beacon"=>{if let Some(period)=group.beacon(rule.counts[0])?{emit_range(0,group.len-1,0,Some(period),serde_json::Value::Null)?;}},
            "sequence"|"temporal"=>{
                let mut match_indices=|indices:Vec<usize>|->Result<(),String>{if indices.is_empty(){return Ok(());}if partition.is_some_and(|b|group.get(indices[0]).is_ok_and(|h|h.ts.div_euclid(rule.window)!=b)){return Ok(());}let selection=store.selection()?;for n in indices{let hit=group.get(n)?;store.add(&selection,hit.id,hit.step as usize)?;}emit(selection,0,None,serde_json::Value::Null)};
                if rule.def.bindings.is_empty(){group.sequence(rule.window,&rule.counts,rule.def.kind=="sequence",None,&mut match_indices)?;}else{group.bindings(|binding|group.sequence(rule.window,&rule.counts,true,Some(binding),&mut match_indices))?;}
            },
            "absence"=>{if !acc.uncertain_absence.contains(&(index as usize)){let ns=serde_json::from_str::<(String,String)>(&key).map_err(|e|e.to_string())?.0;for n in 0..group.len{let anchor=group.get(n)?;if anchor.step!=0||anchor.uncertain_time{continue;}let Some(end)=anchor.ts.checked_add(rule.window)else{continue;};let coverage=inputs.settings.coverage.iter().find(|c|c.complete&&c.dataset_fingerprint==dataset&&c.source==anchor.source&&c.namespace==ns&&Some(&c.category)==rule.def.coverage.as_ref()&&c.start<=anchor.ts&&c.end>=end);let Some(coverage)=coverage else{continue;};let mut prevented=false;for expected in 0..group.len{let h=group.get(expected)?;if h.step==1&&(h.uncertain_time||(h.ts>=anchor.ts&&h.ts<=end)){prevented=true;break;}}if !prevented{emit(group.one(n)?,0,None,serde_json::json!({"expected_events":0,"window_start":anchor.ts,"window_end":end,"coverage":coverage}))?;}}}},
            _=>{}
        }cache.finish(&checkpoint_key)?;Ok(())
    })?;Ok(raws)
}
pub(crate) fn proposal_ruleset(mut rule:RuleDef)->Result<RuleSet,String>{rule.enabled=true;build(vec![rule],vec![],vec![],&HashSet::new())}

/// Correlation results are first-class inputs to a chained Sigma correlation.
/// The anchor is completion time, while evidence retains every original member.
/// A projection is valid only when all members agree on the requested key.
fn expand_chain_raws(raws:crate::security_store::Spool<Raw>,external:&crate::security_correlate::Store,inputs:&Inputs<'_>,source:&Source<'_>,facts:&mut crate::security_facts::Facts,missing:&mut [HashSet<String>],applicable:&mut [usize],eligible:&mut [usize])->Result<crate::security_store::Spool<Raw>,String>{
    if !inputs.rules.rules.iter().any(|rule|!rule.chain_references.is_empty()){return Ok(raws);}
    use rusqlite::{Connection,params};
    let db=Connection::open("").map_err(|e|e.to_string())?;
    db.execute_batch("PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;CREATE TABLE stages(n INTEGER PRIMARY KEY,rule INTEGER,payload TEXT);CREATE INDEX stage_rule ON stages(rule,n);BEGIN;").map_err(|e|e.to_string())?;
    let mut next=0usize;
    for raw in raws.into_iter()? {let raw=raw?;db.execute("INSERT INTO stages VALUES(?1,?2,?3)",params![next as i64,raw.rule,serde_json::to_string(&raw).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;next+=1;}
    let mut done:HashSet<usize>=inputs.rules.rules.iter().enumerate().filter(|(_,rule)|rule.chain_references.is_empty()).map(|(i,_)|i).collect();
    while done.len()<inputs.rules.rules.len(){
        let mut progress=false;
        for (index,rule) in inputs.rules.rules.iter().enumerate(){
            if done.contains(&index){continue;}
            let refs=rule.chain_references.iter().map(|id|inputs.rules.rules.iter().position(|r|r.def.id==*id).ok_or_else(||format!("Referência ausente na cadeia: {id}"))).collect::<Result<Vec<_>,_>>()?;
            if refs.iter().any(|r|!done.contains(r)){continue;}
            done.insert(index);progress=true;
            if !rule.enabled{continue;}
            let mut groups=crate::security_store::Groups::<Hit>::default();
            for (step,&referenced) in refs.iter().enumerate(){
                let mut stmt=db.prepare("SELECT n,payload FROM stages WHERE rule=?1 ORDER BY n").map_err(|e|e.to_string())?;
                let mut rows=stmt.query([referenced as i64]).map_err(|e|e.to_string())?;
                while let Some(row)=rows.next().map_err(|e|e.to_string())?{
                    applicable[index]+=1;
                    crate::operations::check()?;let stage_id:usize=row.get(0).map_err(|e|e.to_string())?;
                    let raw:Raw=serde_json::from_str(&row.get::<_,String>(1).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
                    let mut projection:Option<(String,Vec<String>)>=None;let mut end=None;let mut valid=true;external.clear_identity()?;
                    raw_members(&raw,external,|id,_|{if !valid{return Ok(());}
                        let Some(event)=source.lookup(id) else{valid=false;missing[index].insert("original_event".into());return Ok(())};
                        let (event,n)=facts.normalize(&event,&inputs.settings.mappings)?;let ctx=Ctx::new(&event);
                        if event.timestamp.is_none() || n.time.ambiguity.is_some(){valid=false;missing[index].insert("timestamp".into());return Ok(());}
                        let mut values=Vec::new();for (field,name) in rule.step_by[step].iter().zip(if rule.def.steps[step].by.is_empty(){&rule.def.by}else{&rule.def.steps[step].by}){
                            match ctx.get(field).filter(|v|!v.is_empty()){Some(value)=>values.push(value.to_string()),None=>{valid=false;missing[index].insert(name.clone());break}}
                        }
                        if !valid{return Ok(());}
                        let candidate=(n.get("namespace").map(str::to_string).unwrap_or_else(||format!("source:{}",event.source)),values);
                        if projection.as_ref().is_some_and(|old|old!=&candidate){valid=false;missing[index].insert("ambiguous_aggregate_projection".into());return Ok(());}
                        projection=Some(candidate);end=max_opt(end,event.timestamp);external.reference(&event.event_ref)?;
                        Ok(())
                    })?;
                    if !valid{continue;}let Some((namespace,values))=projection else{continue};let Some(ts)=end else{continue};
                    eligible[index]+=1;

                    let key=serde_json::to_string(&(namespace,serde_json::to_string(&values).map_err(|e|e.to_string())?)).map_err(|e|e.to_string())?;
                    groups.push(0,&key,Hit{ts,id:stage_id,step:step as u8,extra:None,reference:external.identity(&inputs.rules.rules[referenced].def.id)?,source:"correlation".into(),uncertain_time:false,binding:None})?;
                }
            }
            let chain_store=crate::security_correlate::Store::new()?;chain_store.import(groups)?;
            chain_store.keys(|_,key|{let hits=chain_store.group(0,key)?;
                hits.sequence(rule.window,&rule.counts,rule.def.kind!="temporal",None,|matched|{
                    let selection=external.selection()?;let mut stage_ids=Vec::new();let mut member_steps=BTreeMap::new();
                    for n in matched{let stage=hits.get(n)?;let payload:String=db.query_row("SELECT payload FROM stages WHERE n=?1",[stage.id as i64],|r|r.get(0)).map_err(|e|e.to_string())?;
                        let raw:Raw=serde_json::from_str(&payload).map_err(|e|e.to_string())?;raw_members(&raw,external,|member,_|{external.add(&selection,member,stage.step as usize)?;if member_steps.len()<128{member_steps.entry(member.to_string()).or_insert(stage.step);}Ok(())})?;stage_ids.push(stage.reference.clone());}
                    let raw=Raw{rule:index as u32,key:key.to_string().into_boxed_str(),members:vec![],external:Some(selection),distinct:0,period:None,extras:vec![],metrics:serde_json::json!({"chain":true,"completion_time_semantics":true,"referenced_rules":rule.chain_references,"referenced_matches":stage_ids,"member_steps":member_steps})};
                    db.execute("INSERT INTO stages VALUES(?1,?2,?3)",params![next as i64,index as i64,serde_json::to_string(&raw).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;next+=1;Ok(())
                })
            })?;

        }
        if !progress{return Err("Cadeia circular ou dependência sem resolução; análise não publicada".into());}
    }
    let mut result=crate::security_store::Spool::new()?;let mut stmt=db.prepare("SELECT payload FROM stages ORDER BY n").map_err(|e|e.to_string())?;
    for payload in stmt.query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?{crate::operations::check()?;result.push(serde_json::from_str::<Raw>(&payload.map_err(|e|e.to_string())?).map_err(|e|e.to_string())?)?;}
    Ok(result)
}

/// Trailing windows; emit when the condition becomes true, retaining the complete
/// denominator even after an earlier finding. Partition overlap supplies history.
/// The denominator always includes every matching fact, not just suspicious facts.
fn measured_windows(hits: &[Hit], rule: &Compiled, partition: Option<i64>) -> Vec<(Vec<usize>, serde_json::Value)> {
    use std::collections::VecDeque;
    let mut out = Vec::new();
    let mut left = 0;
    let mut sum = 0.0;
    let mut was_matched = false;
    let mut minimum: VecDeque<usize> = VecDeque::new();
    let mut maximum: VecDeque<usize> = VecDeque::new();
    let value = |i: usize| hits[i].extra.as_deref().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
    for right in 0..hits.len() {
        while left < right && hits[right].ts.saturating_sub(hits[left].ts) > rule.window {
            sum -= value(left);
            left += 1;
        }
        while minimum.front().is_some_and(|&i| i < left) {
            minimum.pop_front();
        }
        while maximum.front().is_some_and(|&i| i < left) {
            maximum.pop_front();
        }
        let v = value(right);
        sum += v;
        while minimum.back().is_some_and(|&i| value(i) >= v) {
            minimum.pop_back();
        }
        while maximum.back().is_some_and(|&i| value(i) <= v) {
            maximum.pop_back();
        }
        minimum.push_back(right);
        maximum.push_back(right);
        // The first bucket is history. Evaluating its incomplete lookback could
        // turn benign/successful events outside the partition into false absence.
        if partition.is_some_and(|b| hits[right].ts.div_euclid(rule.window) != b.saturating_add(1))
            || hits.get(right + 1).is_some_and(|h| h.ts == hits[right].ts)
        {
            continue;
        }
        let count = right + 1 - left;
        if count < rule.counts[0] {
            was_matched = false;
            continue;
        }
        let (matched, metrics) = if let Some(r) = &rule.def.ratio {
            let ratio = sum / count as f64;
            (
                ratio >= r.gte,
                serde_json::json!({"numerator":sum as usize,"denominator":count,"ratio":ratio,"gte":r.gte}),
            )
        } else {
            let a = rule.def.aggregate.as_ref().unwrap();
            let result = match a.operation.as_str() {
                "sum" => sum,
                "avg" => sum / count as f64,
                "min" => value(*minimum.front().unwrap()),
                _ => value(*maximum.front().unwrap()),
            };
            (
                result.is_finite() && a.gte.is_none_or(|n| result >= n) && a.lte.is_none_or(|n| result <= n),
                serde_json::json!({"operation":a.operation,"field":a.field,"value":result,"samples":count,"gte":a.gte,"lte":a.lte}),
            )
        };
        if matched && !was_matched {
            out.push(((left..=right).collect(), metrics));
        }
        was_matched = matched;
    }
    out
}

fn label_for(column: &str) -> String {
    entities::role_of_column(column).map(|r| entities::info(r).label.to_string()).unwrap_or_else(|| column.to_string())
}

pub struct Inputs<'a> {
    pub rules: &'a RuleSet,
    pub catalog: Option<&'a CompiledCatalog>,
    pub settings: &'a Settings,
}

/// Categories of the threat catalog, stable order.
fn threat_categories(catalog: &CompiledCatalog, count: usize) -> (Vec<u32>, Vec<String>) {
    let mut names: Vec<String> = Vec::new();
    let mut per_rule = Vec::with_capacity(count);
    for i in 0..count {
        let category = &catalog.rule(i).category;
        let index = match names.iter().position(|c| c == category) {
            Some(p) => p,
            None => {
                names.push(category.clone());
                names.len() - 1
            }
        };
        per_rule.push(index as u32);
    }
    (per_rule, names)
}

/// Events analyzed by the triage: the current selection or explicit records (case).
pub enum Source<'a> {
    Selection(&'a crate::workspace::Selection<'a>),
    Events(Vec<&'a Event>),
}

impl Source<'_> {
    fn fold(&self, pass: &Pass<'_>, patterns: usize) -> Result<Acc, String> {
        let mut acc = Acc::new(patterns, pass.mappings, pass.rules.plan.len(), pass.behavior)?;
        acc.eligible.resize(pass.rules.rules.len(), 0);
        acc.applicable.resize(pass.rules.rules.len(), 0);
        acc.missing.resize_with(pass.rules.rules.len(), HashSet::new);
        let mut columnar=pass.rules.plan.columnar(&acc.population.db)?;
        let mut pending_events=Vec::new();let mut pending_normal=Vec::new();let mut pending_keys=Vec::new();let mut pending_memos=Vec::new();let mut pending_bytes=0usize;
        let mut seen = crate::security_store::Seen::default();
        let mut fragments = crate::security_reconstruct::Fragments::new()?;
        let reconstructed_members=crate::security_reconstruct::Members::new()?;
        let mut consume =
            |ev: &Event| -> Result<(), String> {
                let (derived, normalized) = acc.facts.normalize(ev, pass.mappings)?;
                if let Some(key) = &normalized.dedup_key {
                    // Exact producer identity only; identical-looking messages may be distinct events.
                    if !seen.insert(key.clone())? {
                        acc.duplicates += 1;
                        return Ok(());
                    }
                }
                let mut memo=vec![None;pass.rules.plan.len()];
                if let Some(signature)=&pass.rules.predicate_signature{acc.facts.restore_predicates(signature,&mut memo)?;}
                pending_keys.push(acc.facts.content_key().to_string());pending_memos.push(memo);
                pending_bytes+=derived.raw.len()+derived.message.len()+serde_json::to_vec(&derived.fields).map_err(|e|e.to_string())?.len()+serde_json::to_vec(&normalized).map_err(|e|e.to_string())?.len();
                pending_events.push(derived);pending_normal.push(normalized);
                if pending_events.len()>=1024 || pending_bytes>=8*1024*1024 {
                    pass.flush_predicates(&mut acc,&mut columnar,&mut pending_events,&mut pending_normal,&mut pending_keys,&mut pending_memos)?;pending_bytes=0;
                }
                Ok(())
            };
        match self {
            Source::Selection(s) => {
                for e in s.iter() {
                    crate::operations::check()?;
                    if let Some(key) = crate::security_reconstruct::key(&e) {
                        fragments.push(&key, &e)?;
                    } else {
                        consume(&e)?;
                    }
                }
            }
            Source::Events(events) => {
                for e in events {
                    crate::operations::check()?;
                    if let Some(key) = crate::security_reconstruct::key(e) {
                        fragments.push(&key, e)?;
                    } else {
                        consume(e)?;
                    }
                }
            }
        }
        while let Some((event,_))=fragments.next(&reconstructed_members)? {consume(&event)?;}
        drop(consume);
        acc.reconstructed=reconstructed_members;
        pass.flush_predicates(&mut acc,&mut columnar,&mut pending_events,&mut pending_normal,&mut pending_keys,&mut pending_memos)?;
        acc.facts.checkpoint()?;
        Ok(acc)
    }
    fn lookup(&self, id: usize) -> Option<Event> {
        match self {
            Source::Selection(selection) => selection.event(id),
            Source::Events(events) => events
                .get(id)
                .filter(|e| e.id == id)
                .or_else(|| events.iter().find(|e| e.id == id))
                .map(|e| (*e).clone()),
        }
    }
}

pub fn run(inputs: &Inputs<'_>, source: &Source<'_>) -> Result<Triage, String> {
    run_inner(inputs, source, None)
}
pub fn run_stored(inputs: &Inputs<'_>, source: &Source<'_>) -> Result<Arc<crate::security_results::Results>, String> {
    run_stored_mode(inputs,source,true)
}
pub fn run_stored_evaluation(inputs:&Inputs<'_>,source:&Source<'_>)->Result<Arc<crate::security_results::Results>,String>{run_stored_mode(inputs,source,false)}
fn run_stored_mode(inputs:&Inputs<'_>,source:&Source<'_>,publish:bool)->Result<Arc<crate::security_results::Results>,String>{
    let _working = crate::security_store::working_lane()?;
    let started = std::time::Instant::now();
    let mut writer = crate::security_results::Writer::new()?;
    let metadata = run_inner(inputs, source, Some(&mut writer))?;
    let mut result = if publish{writer.finish(metadata)?}else{writer.finish_evaluation(metadata)?};
    Arc::get_mut(&mut result).unwrap().metadata["elapsed_ms"] = serde_json::json!(started.elapsed().as_millis());
    Ok(result)
}
fn run_inner(
    inputs: &Inputs<'_>,
    source: &Source<'_>,
    mut sink: Option<&mut crate::security_results::Writer>,
) -> Result<Triage, String> {
    use sha2::Digest;
    let started = std::time::Instant::now();
    let catalog = inputs.catalog.filter(|c| c.has_enabled() && inputs.settings.threats);
    let rule_count_in_catalog = catalog.map(|c| c.rule_count()).unwrap_or(0);
    let (threat_category, category_names) = match catalog {
        Some(c) => threat_categories(c, rule_count_in_catalog),
        None => (Vec::new(), Vec::new()),
    };
    let pass = Pass { full_entity_stats:sink.is_some()&&inputs.settings.investigation.enabled,rules: inputs.rules, catalog, threat_category, mappings: &inputs.settings.mappings, behavior: &inputs.settings.investigation };
    let patterns = inputs.rules.automaton.as_ref().map(|a| a.patterns_len()).unwrap_or(0);
    let mut acc = source.fold(&pass, patterns)?;
    let investigation = acc.population.finish()?;
    if let Some(writer) = sink.as_mut() { writer.import_population(&acc.population)?; }
    let dataset_fingerprint = format!("{:x}", acc.fingerprint.clone().finalize());
    let lookup = |id: usize| source.lookup(id);
    crate::operations::check()?;
    let base = inputs.rules.rules.len() as u32;

    // Group hits into raw detections per rule kind.
    let external = crate::security_correlate::Store::new()?;
    let mut checkpoints=crate::security_checkpoints::Cache::new()?;
    let groups = std::mem::take(&mut acc.groups);
    let raws = if sink.is_some() {
        external.import(groups)?;
        external_raws(&external,&mut checkpoints,inputs,&acc,&dataset_fingerprint,base)?
    } else {
    let mut raws = crate::security_store::Spool::<Raw>::new()?;
    for group in groups.into_iter()? {
        let ((rule_index, key), mut hits) = group?;
        let (key, partition) = if rule_index >= base || inputs.rules.rules[rule_index as usize].def.kind == "single" {
            (serde_json::from_str::<(String, String)>(&key).map_err(|e| e.to_string())?.0, None)
        } else {
            serde_json::from_str::<(String, Option<i64>)>(&key).map_err(|e| e.to_string())?
        };
        let key = key.into_boxed_str();
        crate::operations::check()?;
        hits.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.reference.cmp(&b.reference)).then(a.step.cmp(&b.step)));
        hits.dedup_by(|a, b| a.reference == b.reference && a.step == b.step && a.extra == b.extra);
        if rule_index >= base {
            // Threat signals behave as single detections merged within an hour.
            let mut start = 0;
            for k in 1..=hits.len() {
                if k == hits.len() || hits[k].reference != hits[k - 1].reference {
                    let segment = &hits[start..k];
                    let mut extras: Vec<String> =
                        segment.iter().filter_map(|h| h.extra.as_deref().map(str::to_string)).collect();
                    extras.sort();
                    extras.dedup();
                    raws.push(Raw {
                        rule: rule_index,
                        key: key.clone(),
                        members: segment.iter().map(|h| h.id).collect(),
                        external: None,
                        distinct: 0,
                        period: None,
                        extras,
                        metrics: serde_json::Value::Null,
                    })?;
                    start = k;
                }
            }
            continue;
        }
        let rule = &inputs.rules.rules[rule_index as usize];
        let window = rule.window;
        let mut push = |members: Vec<usize>, distinct: usize, period: Option<i64>| -> Result<(), String> {
            if partition.is_some_and(|bucket| {
                members.iter().map(|&m| hits[m].ts).min().is_none_or(|ts| ts.div_euclid(window) != bucket)
            }) {
                return Ok(());
            }
            raws.push(Raw {
                rule: rule_index,
                key: key.clone(),
                members: members.iter().map(|&m| hits[m].id).collect(),
                external: None,
                distinct,
                period,
                extras: Vec::new(),
                metrics: serde_json::Value::Null,
            })
        };
        match rule.def.kind.as_str() {
            "ratio" | "aggregate" => {
                for (members, metrics) in measured_windows(&hits, rule, partition) {
                    raws.push(Raw {
                        rule: rule_index,
                        key: key.clone(),
                        members: members.iter().map(|&m| hits[m].id).collect(),
                external: None,
                        distinct: 0,
                        period: None,
                        extras: Vec::new(),
                        metrics,
                    })?;
                }
            }
            "absence" => {
                if acc.uncertain_absence.contains(&(rule_index as usize)) {
                    continue;
                }
                let namespace = serde_json::from_str::<(String, String)>(&key).map_err(|e| e.to_string())?.0;
                for anchor in hits.iter().filter(|h| h.step == 0 && !h.uncertain_time) {
                    let Some(end) = anchor.ts.checked_add(window) else {
                        continue;
                    };
                    let coverage = inputs.settings.coverage.iter().find(|c| {
                        c.complete
                            && c.dataset_fingerprint == dataset_fingerprint
                            && c.source == anchor.source
                            && c.namespace == namespace
                            && Some(&c.category) == rule.def.coverage.as_ref()
                            && c.start <= anchor.ts
                            && c.end >= end
                    });
                    let Some(coverage) = coverage else {
                        continue;
                    };
                    // Any expected event, even with ambiguous chronology, prevents an absence claim.
                    if hits.iter().any(|h| h.step == 1 && (h.uncertain_time || (h.ts >= anchor.ts && h.ts <= end))) {
                        continue;
                    }
                    raws.push(Raw {rule:rule_index,key:key.clone(),members:vec![anchor.id],external:None,distinct:0,period:None,extras:Vec::new(),metrics:serde_json::json!({"expected_events":0,"window_start":anchor.ts,"window_end":end,"coverage":coverage})})?;
                }
            }
            "single" => {
                for k in 0..hits.len() {
                    push(vec![k], 0, None)?;
                }
            }
            "threshold" => {
                for (i, j) in threshold_ranges(&hits, window, rule.counts[0]) {
                    push((i..=j).collect(), 0, None)?;
                }
            }
            "distinct" => {
                for (i, j, n) in distinct_ranges(&hits, window, rule.counts[0]) {
                    push((i..=j).collect(), n, None)?;
                }
            }
            "sequence" | "temporal" => {
                for members in if !rule.def.bindings.is_empty() {
                    let values: std::collections::BTreeSet<_> =
                        hits.iter().filter_map(|h| h.binding.as_ref()).collect();
                    let mut matches = Vec::new();
                    for value in values {
                        let indices: Vec<_> = hits
                            .iter()
                            .enumerate()
                            .filter(|(_, h)| h.binding.as_ref().is_none_or(|v| v == value))
                            .map(|(i, _)| i)
                            .collect();
                        let subset: Vec<_> = indices.iter().map(|&i| hits[i].clone()).collect();
                        matches.extend(
                            sequence_matches(&subset, window, &rule.counts)
                                .into_iter()
                                .map(|m| m.into_iter().map(|i| indices[i]).collect()),
                        );
                    }
                    matches
                } else if rule.def.kind == "temporal" {
                    temporal_matches(&hits, window, &rule.counts)
                } else {
                    sequence_matches(&hits, window, &rule.counts)
                } {
                    push(members, 0, None)?;
                }
            }
            "beacon" => {
                if let Some((period, _)) = beacon_period(&hits, rule.counts[0]) {
                    push((0..hits.len()).collect(), 0, Some(period))?;
                }
            }
            _ => {}
        }
    }

        raws
    };

    // Timestamps per event id for the detection bounds.
    let raws=expand_chain_raws(raws,&external,inputs,source,&mut acc.facts,&mut acc.missing,&mut acc.applicable,&mut acc.eligible)?;
    let mut detections: Vec<Detection> = Vec::new();
    let mut suppressed = 0usize;
    let settings = inputs.settings;
    for raw in raws.into_iter()? {
        let raw = raw?;
        let mut original_ids=crate::security_store::Spool::<(usize,Option<usize>)>::new()?;
        let mut reconstructed_ids=crate::security_store::Spool::<(usize,Option<usize>)>::new()?;
        let mut ids=Vec::new();let mut unique=crate::security_store::Seen::default();let mut full_count=0;
        let mut add=|id:usize,step:Option<usize>|->Result<(),String>{if unique.insert(id.to_string())?{original_ids.push((id,step))?;reconstructed_ids.push((id,step))?;full_count+=1;if sink.is_none()||ids.len()<128{ids.push(id);}}Ok(())};
        if let Some(selection)=&raw.external{external.members(selection,|id,step|add(id,Some(step)))?;}else{for id in raw.members{add(id,None)?;}}
        if ids.is_empty(){continue;}
        let (name, description, severity, attack_refs, kind, origin, by_columns, rule_id): (
            String,
            String,
            String,
            Vec<AttackRef>,
            String,
            String,
            Vec<String>,
            String,
        );
        let mut filters = Vec::new();
        if raw.rule >= base {
            let category = &category_names[(raw.rule - base) as usize];
            let catalog = catalog.expect("threat hits imply a catalog");
            let rules: Vec<&crate::threats::Rule> = raw
                .extras
                .iter()
                .filter_map(|id| (0..rule_count_in_catalog).map(|i| catalog.rule(i)).find(|r| &r.id == id))
                .collect();
            let top = rules.iter().max_by_key(|r| severity_rank(&r.severity));
            severity = top.map(|r| r.severity.clone()).unwrap_or_else(|| "low".into());
            let mut techniques: Vec<String> = rules.iter().flat_map(|r| crate::threats::attack_for(r)).collect();
            techniques.sort();
            techniques.dedup();
            attack_refs = techniques.iter().map(|t| attack::reference(t, &[])).collect();
            name = category.clone();
            description = format!(
                "Conteúdo compatível com {} {} do catálogo de ameaças. Confirme a direção e o resultado antes de concluir.",
                rules.len(),
                if rules.len() == 1 { "regra" } else { "regras" }
            );
            kind = "signal".into();
            origin = "threats".into();
            by_columns = vec![];
            rule_id = format!("threat:{category}");
            let query = raw.extras.iter().map(|id| format!("regra:{id}")).collect::<Vec<_>>().join(" OR ");
            filters.push(crate::query::Filter {
                column: "_all".into(),
                op: "query".into(),
                value: query,
                value2: None,
            });
        } else {
            let rule = &inputs.rules.rules[raw.rule as usize];
            name = rule.def.name.clone();
            description = rule.def.description.clone();
            severity = rule.def.severity.clone();
            attack_refs = rule.attack.clone();
            kind = rule.def.kind.clone();
            origin = rule.origin.to_string();
            by_columns = rule.def.by.clone();
            rule_id = rule.def.id.clone();
            filters.push(crate::query::Filter {
                column: "_all".into(),
                op: "detection".into(),
                value: rule.def.id.clone(),
                value2: None,
            });
        }
        // Entities from the grouping key.
        let mut entity_refs: Vec<EntityRef> = Vec::new();
        let (namespace, raw_key) = serde_json::from_str::<(String, String)>(&raw.key).map_err(|e| e.to_string())?;
        let key_parts: Vec<String> =
            if raw.rule >= base { Vec::new() } else { serde_json::from_str(&raw_key).map_err(|e| e.to_string())? };
        for (column, value) in by_columns.iter().zip(&key_parts) {
            if value.is_empty() {
                continue;
            }
            entity_refs.push(EntityRef { column: column.clone(), label: label_for(column), value: value.to_string() });
            filters.push(crate::query::Filter {
                column: column.clone(),
                op: "equals_exact".into(),
                value: value.to_string(),
                value2: None,
            });
        }
        if raw.rule >= base {
            if let Ok((column, value)) = serde_json::from_str::<(String, String)>(&raw_key) {
                entity_refs.push(EntityRef {
                    column: column.clone(),
                    label: label_for(&column),
                    value: value.to_string(),
                });
                filters.push(crate::query::Filter {
                    column: column.into(),
                    op: "equals_exact".into(),
                    value: value.to_string(),
                    value2: None,
                });
            }
        }
        // The primary tactic of each technique keeps episode chains honest
        // (Valid Accounts, for example, is listed under four tactics).
        let mut tactics: Vec<String> = attack_refs.iter().filter_map(|a| a.tactics.first().cloned()).collect();
        if let Some(rule) = (raw.rule < base).then(|| &inputs.rules.rules[raw.rule as usize]) {
            tactics.extend(rule.def.tactics.iter().cloned());
        }
        tactics.sort_by_key(|t| attack::tactic_order(t));
        tactics.dedup();
        let mut detection = Detection {
            participants: Default::default(),
            pattern_source: None,
            id: String::new(),
            rule: rule_id,
            name,
            description,
            severity,
            origin,
            kind,
            attack: attack_refs,
            tactics,
            start: None,
            end: None,
            count: full_count,
            entities: entity_refs,
            summary: String::new(),
            distinct: raw.distinct,
            period_ms: raw.period,
            event_ids: ids,
            filters,
            evidence: crate::evidence::Evidence::default(),
            namespace: namespace.into(),
            signal_rules: raw.extras.clone(),
            measurements: raw.metrics,
        };
        let detection = &mut detection;
        if let Some(writer)=sink.as_ref(){for entity in &detection.entities{writer.stage_entity(&detection.namespace,&entity.column,&entity.value,&entity.label)?;}}
        crate::operations::check()?;
        let first = lookup(detection.event_ids[0]);
        detection.start = None;
        detection.end = None;
        let rule = inputs.rules.find(&detection.rule);
        let mut policy = rule.map(|r| r.def.evidence.clone()).unwrap_or_else(|| crate::evidence::Policy {
            maturity: "experimental".into(),
            level: 1,
            rationale: "Conteúdo original com estrutura suspeita; a correspondência não demonstra execução.".into(),
            missing: vec!["Direção, resultado e corroboração comportamental".into()],
            benign: vec!["Teste autorizado, documentação ou conteúdo citado".into()],
            ..Default::default()
        });
        if rule.is_none() && detection.signal_rules.iter().any(|id| id.starts_with("web.xss.") || id.starts_with("web.sql.") || id.starts_with("web.path.")) {
            let request = if let Some(e) = first.as_ref() {
                let n=acc.facts.normalize(e,&inputs.settings.mappings)?.1;
                inputs.catalog.is_some_and(|catalog| catalog.explain(e).iter().any(|h| detection.signal_rules.contains(&catalog.rule(h.rule).id)
                    && (h.provenance.direction=="request" || ["url","request_body","request_command"].iter().filter_map(|k|n.values.get(*k)).any(|p|p.field==h.provenance.field))))
            } else { false };
            policy.level = if request { 3 } else { 2 };
            policy.claim = if request { "attempt" } else { "activity" }.into();
            policy.version = "2".into();
            policy.rationale = if request { "Estrutura específica de exploração presente em requisição HTTP; não comprova execução nem retorno sensível." } else { "Estrutura específica de payload presente no conteúdo registrado; direção e uso ainda não demonstrados." }.into();
        }
        detection.evidence = crate::evidence::Evidence::from_policy(&policy);
        let mut outcomes = HashSet::new();
        let mut refs = Vec::new();
        let mut suppressed_by_original=false;
        for entry in original_ids.into_iter()? {
            let (id,recorded_step)=entry?;
            crate::security_budget::check()?;
            if let Some(event) = lookup(id) {
                let reference = crate::security_normalize::event_ref(&event);
                if sink.is_none()||refs.len()<128{refs.push(reference.clone());}
                let (view, normalized) = acc.facts.normalize(&event, &inputs.settings.mappings)?;
                detection.participants.merge(&crate::security_participants::extract(&event, &normalized, &detection.rule, &detection.tactics));
                detection.start = min_opt(detection.start, normalized.time.epoch_ms);
                detection.end = max_opt(detection.end, normalized.time.epoch_ms);
                if normalized.time.ambiguity.is_some()
                    && detection.kind != "single"
                    && detection.evidence.evidence_level >= 4
                {
                    detection.evidence.limit(3, "Cronologia com fuso ou resolução não demonstrados");
                }
                for role in ENTITY_ROLES {
                    if let Some(value) = entities::value(&event, role) {
                        let column = entities::info(role).column;
                        if let Some(writer)=sink.as_ref(){writer.stage_entity(&detection.namespace,column,&value,&label_for(column))?;}
                        suppressed_by_original |= settings.suppress.iter().any(|s|s.rule==detection.rule&&s.expires.is_none_or(|t|t>chrono::Utc::now().timestamp_millis())&&s.scope.as_deref().is_none_or(|v|v==detection.namespace)&&s.column.as_deref().is_none_or(|v|v==column)&&s.value.as_deref().is_some_and(|v|v==value));
                        if !detection.entities.iter().any(|e| e.column == column && e.value == value) && (sink.is_none()||detection.entities.len()<256) {
                            detection.entities.push(EntityRef {
                                column: column.into(),
                                label: label_for(column),
                                value: value.into_owned(),
                            });
                        }
                    }
                }
                outcomes.insert(normalized.get("outcome").unwrap_or("unknown").to_string());
                if normalized.content_clipped && detection.rule.starts_with("content.") {
                    detection.evidence.limit(3,"Inspeção parcial (64 KiB/campo; até 256 KiB e 128 segmentos na varredura genérica); contexto restante não avaliado");
                }
                if !normalized.conflicts.is_empty() {
                    detection.evidence.limit(2, "Campos de identidade ou resultado conflitantes");
                }
                if normalized.product == "generic" && normalized.get("action").is_none()
                    && !(detection.rule.starts_with("content.") && (policy.claim == "attempt" || detection.rule.ends_with("_payload"))) {
                    detection.evidence.limit(2, "Sem ação observada em fonte reconhecida ou mapeada");
                }
                let step = recorded_step.or_else(|| detection.measurements["member_steps"].get(id.to_string()).and_then(serde_json::Value::as_u64).map(|step|step as usize))
                    .or_else(||rule.and_then(|r| r.conds.iter().position(|c| c.matches(&view)))).unwrap_or(0);
                if detection.evidence.excerpts.len() < 8 {
                    if let Some(condition) = rule.and_then(|r| r.conds.get(step)) {
                        detection
                            .evidence
                            .excerpts
                            .extend(condition.evidence_excerpts(&view, &normalized, &reference));
                    } else if let Some(catalog) = inputs.catalog {
                        for hit in catalog.explain(&event) {
                            if detection.signal_rules.contains(&catalog.rule(hit.rule).id) {
                                detection.evidence.excerpts.extend(hit.excerpt);
                            }
                        }
                    }
                    detection.evidence.excerpts.truncate(8);
                }
                let condition = rule
                    .map(|r| {
                        if r.def.steps.is_empty() {
                            r.def.condition.clone()
                        } else {
                            r.def.steps.get(step).map(|s| s.condition.clone()).unwrap_or_default()
                        }
                    })
                    .unwrap_or_else(|| detection.signal_rules.join(", "));
                let satisfied = format!(
                    "Etapa {}: {}",
                    step + 1,
                    if condition.is_empty() { "seletor compilado da regra versionada" } else { &condition }
                );
                if !detection.evidence.conditions_satisfied.contains(&satisfied) {
                    detection.evidence.conditions_satisfied.push(satisfied);
                }
                let mut excerpts: Vec<_> = normalized.signals.iter().filter(|s| condition.contains(&s.key)).map(|s| &s.excerpt).collect();
                if excerpts.is_empty() {excerpts.extend(detection.evidence.excerpts.iter().filter(|x|x.event_ref==reference));}
                let fields: Vec<_> = excerpts.iter().map(|x|x.field.clone()).collect();
                let observed = excerpts.iter().map(|x| format!("{}: {}", x.field, x.matched)).collect::<Vec<_>>().join(" · ");
                if let Some(check) = detection.evidence.checks.iter_mut().find(|c| c.status == "passed" && c.expected == condition) {
                    if check.event_refs.len() < 8 {check.event_refs.push(reference.clone());}
                } else if detection.evidence.checks.len() < 32 {
                    detection.evidence.checks.push(crate::evidence::Check {
                        status: "passed".into(), label: format!("Condição observada · etapa {}", step+1), expected: condition,
                        observed: if observed.is_empty() { "Seletor satisfeito no evento original".into() } else { observed.chars().take(1200).collect() },
                        event_refs: vec![reference.clone()], fields,
                    });
                }
                for field in &policy.required {
                    let expected = format!("{field}: presente");
                    if detection.evidence.checks.iter().any(|c| c.expected == expected) || detection.evidence.checks.len() >= 32 {continue;}
                    let provenance = field.strip_prefix("_sec.").and_then(|f|normalized.values.get(f));
                    let value = provenance.map(|p|p.value.clone()).or_else(||crate::security_normalize::field_text(&view,field)).unwrap_or_default();
                    detection.evidence.checks.push(crate::evidence::Check {
                        status: "passed".into(),label:"Campo obrigatório presente".into(),expected,
                        observed:value.chars().take(400).collect(),event_refs:vec![reference.clone()],
                        fields:vec![provenance.map(|p|p.field.clone()).unwrap_or_else(||field.clone())],
                    });
                }
                let member=crate::evidence::Member {
                    event_ref: reference,
                    event_id: id,
                    step,
                    fields: policy.required.clone(),
                    provenance: normalized.values.iter().filter(|(key,_)| !matches!(key.as_str(), "request_body" | "response_body")).map(|(k,v)|(k.clone(),v.clone())).collect(),
                    time: normalized.time.clone(),
                };
                if let Some(writer)=sink.as_ref(){writer.stage_member(&detection.namespace,&member)?;}
                if sink.is_none()||detection.evidence.evidence_members.len()<128{detection.evidence.evidence_members.push(member);}
            }
        }
        let mut expanded = Vec::new();
        for entry in reconstructed_ids.into_iter()? {let (id,recorded_step)=entry?;
            if let Some((member_count, limitations)) = acc.reconstructed.describe(id)? {
                let step =
                    detection.evidence.evidence_members.iter().find(|m| m.event_id == id).map(|m| m.step).or(recorded_step).unwrap_or(0);
                acc.reconstructed.for_each(id, |member_id,reference| {
                    if sink.is_none()||expanded.len()<128{expanded.push(member_id);}
                    if sink.is_none()||refs.len()<128{refs.push(reference.clone());}
                    if !detection.evidence.evidence_members.iter().any(|m| m.event_id == member_id) {
                        let original = if let Some(e) = lookup(member_id) {
                                let n = acc.facts.normalize(&e, &inputs.settings.mappings)?.1;
                                detection.participants.merge(&crate::security_participants::extract(&e, &n, &detection.rule, &detection.tactics));
                                if let Some(writer)=sink.as_ref(){for role in ENTITY_ROLES{if let Some(value)=entities::value(&e,role){let column=entities::info(role).column;writer.stage_entity(&detection.namespace,column,&value,&label_for(column))?;}}}
                                n
                            } else { Default::default() };
                        let member=crate::evidence::Member {
                            event_ref: reference.clone(),
                            event_id: member_id,
                            step,
                            fields: vec!["producer.operation_id".into()],
                            provenance: original.values,
                            time: original.time,
                        };
                        if let Some(writer)=sink.as_ref(){writer.stage_member(&detection.namespace,&member)?;}
                        if sink.is_none()||detection.evidence.evidence_members.len()<128{detection.evidence.evidence_members.push(member);}
                    }
                    Ok(())
                })?;
                for limitation in &limitations {
                    detection.evidence.limit(2, limitation);
                }
                if detection.evidence.relationships.len()<32{detection.evidence.relationships.push(crate::evidence::Relationship {
                    kind: "reconstruction".into(),
                    fields: vec!["producer.operation_id".into()],
                    description: format!(
                        "{} fragmentos ou estagios da mesma operacao; contam como um fato",
                        member_count
                    ),
                });}
            } else {
                if sink.is_none()||expanded.len()<128{expanded.push(id);}
            }
        }
        expanded.sort_unstable();
        expanded.dedup();
        detection.event_ids = expanded;
        if sink.is_none(){detection.count=detection.event_ids.len();}
        refs.sort();
        refs.dedup();
        detection.evidence.event_refs = refs.clone();
        if let Some(writer)=sink.as_ref(){let (id,count)=writer.staged_identity(&detection.rule)?;detection.id=id;detection.count=count;if count>detection.evidence.evidence_members.len(){
            if !detection.measurements.is_object(){detection.measurements=serde_json::json!({});}
            detection.measurements["evidence_paging"]=serde_json::json!({"complete_members_in_store":true,"total":count,"preview":detection.evidence.evidence_members.len(),"section":"finding_members"});
            detection.measurements["entity_paging"]=serde_json::json!({"complete_entities_in_store":true,"total":writer.staged_entity_count()?,"preview":detection.entities.len(),"section":"finding_entities"});
        }}else{detection.id=crate::evidence::stable_id("d",std::iter::once(detection.rule.clone()).chain(refs));}
        if outcomes.contains("blocked") && outcomes.len() == 1 {
            detection.evidence.outcome = "blocked".into();
            if matches!(policy.claim.as_str(), "execution" | "effect") {
                detection.evidence.limit(3, "Operação bloqueada: execução/efeito não demonstrado");
            }
        } else if outcomes.contains("failure") && outcomes.len() == 1 {
            detection.evidence.outcome = "failure".into();
            if matches!(policy.claim.as_str(), "execution" | "effect") {
                detection.evidence.limit(3, "Operação falhou: execução/efeito não demonstrado");
            }
        } else if outcomes.len() == 1 {
            detection.evidence.outcome = outcomes.into_iter().next().unwrap_or_else(|| "unknown".into());
        } else {
            detection.evidence.outcome = "mixed".into();
        }
        detection.evidence.checks.push(crate::evidence::Check {
            status: if detection.evidence.outcome == "unknown" { "unknown" } else if matches!(detection.evidence.outcome.as_str(), "blocked"|"failure") { "failed" } else { "observed" }.into(),
            label: "Resultado da operação".into(), expected: "Resultado registrado; sucesso HTTP não comprova exploração".into(),
            observed: detection.evidence.outcome.clone(), event_refs: detection.evidence.event_refs.iter().take(8).cloned().collect(), fields: vec!["outcome".into()],
        });
        if let Some(rule) = rule {
            for (label, expected, observed) in [
                ("Agrupamento da regra", rule.def.by.join(", "), format!("{} eventos · {} valores distintos · janela {}", detection.count, detection.distinct, rule.def.window.as_deref().unwrap_or("evento individual"))),
                ("Condição de exclusão", rule.def.unless.clone(), "Condição avaliada e não encontrada nos membros".into()),
            ] {
                if !expected.is_empty() { detection.evidence.checks.push(crate::evidence::Check { status:"passed".into(),label:label.into(),expected,observed,event_refs:vec![],fields:vec![] }); }
            }
        }
        if detection.evidence.outcome == "unknown"
            && matches!(policy.claim.as_str(), "execution" | "effect")
            && detection.evidence.evidence_level >= 4
        {
            detection.evidence.limit(3, "Resultado da operação desconhecido");
        }
        if matches!(detection.kind.as_str(), "sequence" | "temporal") {
            let level = detection.evidence.evidence_level;
            if detection.kind == "sequence" && detection.start == detection.end {
                detection.evidence.limit(3, "Resolução temporal insuficiente para demonstrar ordem");
            }
            if let Some(rule) = rule {
                detection.evidence.relationships.push(crate::evidence::Relationship {
                    kind: detection.kind.clone(),
                    fields: rule.def.by.clone(),
                    description: format!(
                        "Vínculos explícitos entre etapas; janela {}",
                        rule.def.window.as_deref().unwrap_or("10m")
                    ),
                });
            }
            if detection.evidence.evidence_members.iter().map(|m| m.event_ref.as_str()).collect::<HashSet<_>>().len()
                < 2
                && level >= 4
            {
                detection.evidence.limit(2, "Corroboração requer fatos independentes");
            }
        }
        // Apply exceptions after collecting the entities actually present in the evidence.
        // Single-event content rules need not group by an IP to support an IP-scoped exception.
        let hidden = suppressed_by_original || settings.suppress.iter().any(|s| {
            s.rule == detection.rule
                && s.expires.is_none_or(|t| t > chrono::Utc::now().timestamp_millis())
                && s.scope.as_deref().is_none_or(|v| v == detection.namespace)
                && match (&s.column, &s.value) {
                    (Some(column), Some(value)) => detection.entities.iter().any(|e| &e.column == column && &e.value == value),
                    (None, Some(value)) => detection.entities.iter().any(|e| &e.value == value),
                    _ => true,
                }
        });
        if hidden { suppressed += 1; if let Some(writer)=sink.as_ref(){writer.discard_staged()?;}continue; }
        let template = rule.and_then(|r| r.def.summary.as_deref());
        detection.summary = render_summary(template, detection, first.as_ref());
        detection.pattern_source = first.as_ref().map(crate::security_grouping::source);
        // Exact evidence selection, independent of mutable rule definitions and time filters.
        detection.filters = vec![crate::query::Filter {
            column: "event_ref".into(),
            op: if sink.is_some(){"finding"}else{"in_exact"}.into(),
            value: if sink.is_some(){detection.id.clone()}else{detection.evidence.event_refs.join("\n")},
            value2: None,
        }];
        if let Some(writer) = sink.as_mut() {
            writer.push(detection)?;
        } else {
            detections.push(detection.clone());
        }
    }
    detections.sort_by(|a, b| {
        b.evidence
            .evidence_level
            .cmp(&a.evidence.evidence_level)
            .then(severity_rank(&b.severity).cmp(&severity_rank(&a.severity)))
            .then(a.id.cmp(&b.id))
    });
    let counts_by_level =
        std::array::from_fn(|i| detections.iter().filter(|d| d.evidence.evidence_level as usize == i + 1).count());
    let fingerprint = format!("{:x}", acc.fingerprint.finalize());
    let versions = serde_json::to_string(&(
        crate::evidence::POLICY_VERSION,
        crate::evidence::NORMALIZATION_VERSION,
        crate::security_grouping::VERSION,
        crate::security_participants::VERSION,
        crate::attack::VERSION,
        inputs.rules.rules.iter().map(|r| &r.def).collect::<Vec<_>>(),
        inputs.settings,
        inputs.catalog.map(|catalog| catalog.signature()),
    ))
    .map_err(|e| e.to_string())?;
    let analysis_id = crate::evidence::stable_id("analysis", [fingerprint, versions]);
    let rule_coverage = inputs
        .rules
        .rules
        .iter()
        .enumerate()
        .map(|(i, r)| RuleCoverage {
            rule: r.def.id.clone(),
            unit: if r.chain_references.is_empty(){"events"}else{"referenced_findings"}.into(),
            status: if !r.enabled {
                "disabled"
            } else if r.def.kind == "absence"
                && !inputs.settings.coverage.iter().any(|c| {
                    c.complete
                        && c.dataset_fingerprint == dataset_fingerprint
                        && Some(&c.category) == r.def.coverage.as_ref()
                })
            {
                "missing_coverage"
            } else if acc.applicable[i] == 0 {
                "not_applicable"
            } else if acc.eligible[i] == 0 {
                "missing_fields"
            } else if acc.eligible[i] < acc.applicable[i] {
                "partial"
            } else {
                "complete"
            }
            .into(),
            applicable: acc.applicable[i],
            eligible: acc.eligible[i],
            missing: {
                let mut v: Vec<_> = acc.missing[i].iter().cloned().collect();
                v.sort();
                v
            },
        })
        .collect();
    let mut limitations = Vec::new();
    if acc.clipped > 0 {
        limitations.push(format!("{} registros com conteúdo textual parcial", acc.clipped));
    }
    if acc.limited {
        limitations.push("Estatísticas de entidades limitadas pelo orçamento; achados não foram descartados".into());
    }
    if acc.undated > 0 {
        limitations.push(format!("{} registros sem horário; correlação temporal indisponível para eles", acc.undated));
    }
    let episodes = cluster(&detections);
    if let Some(writer) = sink.as_mut() {
        for (role, stats) in ENTITY_ROLES.iter().zip(&acc.entities) {
            for (key, stat) in stats {
                if let Ok((namespace, value)) = serde_json::from_str::<(String, String)>(key) {
                    writer.entity_stat(&namespace, entities::info(*role).column, &value, stat.events, stat.failures)?;
                }
            }
        }
    }
    let entities_risk = risk(&detections, &acc.entities);
    let rare = rarity(&acc.rare, &acc.rare_totals);
    let tactics = tactics_summary(&detections);
    let coverage = COVERAGE_ROLES
        .iter()
        .zip(acc.coverage)
        .filter(|(_, n)| *n > 0)
        .map(|(role, count)| {
            let info = entities::info(*role);
            RoleCoverage { column: info.column.into(), label: info.label.into(), count }
        })
        .collect();
    Ok(Triage {
        investigation,
        execution: serde_json::json!({"facts_version":crate::security_facts::VERSION,"normalizations":acc.facts.computed,"fact_reuses":acc.facts.hits,"predicate_reuses":acc.facts.predicate_reuses,"shared_predicate_nodes":inputs.rules.plan.len(),"columnar":{"backend":"DuckDB shared Rust semantic UDFs","batches":acc.columnar_batches,"cached_batches":acc.columnar_reused_batches,"rows":acc.columnar_rows,"shared_scalar_leaves":acc.columnar_leaves,"event_confirmation":true},"rule_predicates":inputs.rules.rules.iter().map(|r|r.conds.len()).sum::<usize>(),"incremental_mode":"content_addressed_facts_predicates_and_group_results","correlations":"unchanged_groups_reused_changed_groups_exactly_reconciled","late_events":"full_temporal_reconciliation","checkpoints":checkpoints.describe(),"fact_cache_evictions":acc.facts.evicted}),
        dataset_fingerprint,
        total: acc.total,
        undated: acc.undated,
        start: acc.first,
        end: acc.last,
        complete: !crate::operations::cancelled(),
        limited: acc.limited || acc.clipped > 0,
        detections,
        episodes,
        entities: entities_risk,
        rare,
        tactics,
        coverage,
        suppressed,
        rules: inputs.rules.rules.iter().filter(|r| r.enabled).count(),
        sigma_rules: inputs.rules.sigma_loaded,
        sigma_errors: inputs.rules.sigma_errors.iter().take(20).cloned().collect(),
        threat_rules: if catalog.is_some() { rule_count_in_catalog } else { 0 },
        elapsed_ms: started.elapsed().as_millis() as u64,
        analysis_id,
        attack_version: crate::attack::VERSION.into(),
        policy_version: crate::evidence::POLICY_VERSION.into(),
        normalization_version: crate::evidence::NORMALIZATION_VERSION.into(),
        counts_by_level,
        rule_coverage,
        limitations,
        duplicates: acc.duplicates,
    })
}

fn render_summary(template: Option<&str>, detection: &Detection, first: Option<&Event>) -> String {
    let fallback = || {
        let who = detection.entities.first().map(|e| e.value.as_str()).unwrap_or("");
        if who.is_empty() {
            format!("{} {}", detection.count, if detection.count == 1 { "registro" } else { "registros" })
        } else {
            format!("{} {} · {who}", detection.count, if detection.count == 1 { "registro" } else { "registros" })
        }
    };
    let Some(template) = template else { return fallback() };
    let ctx = first.map(Ctx::new);
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let name = &rest[open + 1..open + close];
        let value = match name {
            "count" => Some(detection.count.to_string()),
            "distinct" => Some(detection.distinct.to_string()),
            "period" => detection.period_ms.map(format_period),
            column => detection.entities.iter().find(|e| e.column == column).map(|e| e.value.clone()).or_else(|| {
                ctx.as_ref().and_then(|c| c.get(&querylang::field_ref(column)).map(|v| v.chars().take(120).collect()))
            }),
        };
        match value {
            Some(v) if !v.is_empty() => out.push_str(&v),
            _ => out.push('—'),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    if out.contains('—') && detection.entities.is_empty() {
        return fallback();
    }
    out
}

fn tactics_summary(detections: &[Detection]) -> Vec<TacticSummary> {
    attack::TACTICS
        .iter()
        .map(|tactic| {
            let mut techniques: BTreeMap<String, (String, usize)> = BTreeMap::new();
            let mut count = 0;
            for d in detections {
                let mut hit = false;
                for a in &d.attack {
                    if a.tactics.first().is_some_and(|t| t == tactic.key)
                        || (a.tactics.is_empty() && d.tactics.iter().any(|t| t == tactic.key))
                    {
                        hit = true;
                        let entry = techniques.entry(a.id.clone()).or_insert((a.name.clone(), 0));
                        entry.1 += 1;
                    }
                }
                count += usize::from(hit);
            }
            let mut techniques: Vec<TechniqueCount> =
                techniques.into_iter().map(|(id, (name, count))| TechniqueCount { id, name, count }).collect();
            techniques.sort_by(|a, b| b.count.cmp(&a.count));
            TacticSummary {
                id: tactic.id.into(),
                key: tactic.key.into(),
                label: tactic.label.into(),
                count,
                techniques,
            }
        })
        .collect()
}

fn cluster(detections: &[Detection]) -> Vec<Episode> {
    let n = detections.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    let mut owners = HashMap::new();
    for (i, d) in detections.iter().enumerate() {
        if d.evidence.evidence_level == 0 {
            continue;
        }
        for reference in &d.evidence.event_refs {
            if let Some(&j) = owners.get(&(d.namespace.as_str(), reference.as_str())) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[b] = a;
                }
            } else {
                owners.insert((d.namespace.as_str(), reference.as_str()), i);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    let mut episodes: Vec<Episode> = groups
        .into_values()
        .map(|members| {
            let severity = members
                .iter()
                .map(|&i| detections[i].severity.as_str())
                .max_by_key(|s| severity_rank(s))
                .unwrap_or("low")
                .to_string();
            let evidence_level = members.iter().map(|&i| detections[i].evidence.evidence_level).max().unwrap_or(1);
            let score = (evidence_level as f64) * 20.0;
            let mut event_refs: Vec<_> =
                members.iter().flat_map(|&i| detections[i].evidence.event_refs.clone()).collect();
            event_refs.sort();
            event_refs.dedup();
            // Tactics in the order they were first observed (kill-chain order breaks ties).
            let mut first_seen: Vec<(String, i64)> = Vec::new();
            for &i in &members {
                let at = detections[i].start.unwrap_or(i64::MAX);
                for t in &detections[i].tactics {
                    match first_seen.iter_mut().find(|(k, _)| k == t) {
                        Some(entry) => entry.1 = entry.1.min(at),
                        None => first_seen.push((t.clone(), at)),
                    }
                }
            }
            first_seen.sort_by_key(|(t, at)| (*at, attack::tactic_order(t)));
            let tactics: Vec<String> = first_seen.into_iter().map(|(t, _)| t).collect();
            let mut entity_counts: Vec<(EntityRef, usize)> = Vec::new();
            for &i in &members {
                for e in &detections[i].entities {
                    match entity_counts.iter_mut().find(|(x, _)| x.column == e.column && x.value == e.value) {
                        Some((_, c)) => *c += 1,
                        None => entity_counts.push((e.clone(), 1)),
                    }
                }
            }
            entity_counts.sort_by(|a, b| b.1.cmp(&a.1));
            let start = members.iter().filter_map(|&i| detections[i].start).min();
            let end = members.iter().filter_map(|&i| detections[i].end).max();
            let mut ordered = members.clone();
            ordered.sort_by_key(|&i| (std::cmp::Reverse(severity_rank(&detections[i].severity)), detections[i].start));
            let lead = &detections[ordered[0]];
            let distinct_names: Vec<&str> = {
                let mut names: Vec<&str> = Vec::new();
                for &i in &ordered {
                    if !names.contains(&detections[i].name.as_str()) {
                        names.push(detections[i].name.as_str());
                    }
                }
                names
            };
            let title = if members.iter().any(|&i| {
                matches!(detections[i].kind.as_str(), "sequence" | "temporal")
                    && detections[i].evidence.evidence_level >= 4
            }) && tactics.len() >= 3
            {
                let first = attack::tactic(&tactics[0]).map(|t| t.label).unwrap_or("");
                let last = attack::tactic(tactics.last().unwrap()).map(|t| t.label).unwrap_or("");
                format!("Possível cadeia de ataque: {first} → {last}")
            } else {
                lead.name.clone()
            };
            let others: Vec<&str> = distinct_names.iter().copied().filter(|n| *n != lead.name).collect();
            let summary = match others.len() {
                0 => lead.summary.clone(),
                1 => format!("{} · também {}", lead.summary, others[0]),
                n => format!("{} · também {} e mais {}", lead.summary, others[0], n - 1),
            };
            let mut detections_in_time = members.clone();
            detections_in_time.sort_by_key(|&i| detections[i].start);
            Episode {
                participants: members.iter().fold(crate::security_participants::Participants::default(), |mut p,&i| {p.merge(&detections[i].participants);p}),
                grouping: None,
                id: if evidence_level == 0 {
                    crate::evidence::stable_id("episode-unassessed", std::iter::once(&lead.id).chain(event_refs.iter()))
                } else {
                    crate::evidence::stable_id("episode", &event_refs)
                },
                evidence_level,
                event_refs,
                title,
                summary,
                severity,
                score: score.min(999.0) as u32,
                start,
                end,
                detections: detections_in_time,
                tactics,
                entities: entity_counts.into_iter().take(6).map(|(e, _)| e).collect(),
            }
        })
        .collect();
    episodes = crate::security_grouping::group(episodes, detections);
    episodes.sort_by(|a, b| {
        b.evidence_level
            .cmp(&a.evidence_level)
            .then(severity_rank(&b.severity).cmp(&severity_rank(&a.severity)))
            .then(a.id.cmp(&b.id))
    });
    episodes
}

fn risk(detections: &[Detection], stats: &[HashMap<Box<str>, EntityStat>; 4]) -> Vec<EntityRisk> {
    let columns = ["@user", "@src_ip", "@host", "@dst_ip"];
    let mut scores: HashMap<(usize, String, String), (f64, usize, Vec<String>)> = HashMap::new();
    for d in detections {
        let weight = f64::from(d.evidence.evidence_level) * 20.0;
        let bonus = 1.0;
        let mut credited = HashSet::new();
        for e in &d.entities {
            let slot = match e.column.as_str() {
                "@user" => 0,
                "@src_ip" => 1,
                "@host" => 2,
                "@dst_ip" => 3,
                _ => continue,
            };
            if !credited.insert((slot, e.value.clone())) {
                continue;
            }
            let entry = scores.entry((slot, d.namespace.clone(), e.value.clone())).or_insert((0.0, 0, Vec::new()));
            entry.0 = entry.0.max(weight * bonus);
            entry.1 += 1;
            entry.2.extend(d.tactics.iter().cloned());
        }
    }
    let mut out: Vec<EntityRisk> = scores
        .into_iter()
        .map(|((slot, namespace, value), (score, count, mut tactics))| {
            tactics.sort_by_key(|t| attack::tactic_order(t));
            tactics.dedup();
            let stat_key = serde_json::to_string(&(namespace.as_str(), value.as_str())).unwrap();
            let stat = stats[slot].get(stat_key.as_str()).cloned().unwrap_or_default();
            let score = score.min(100.0).round() as u32;
            let scope = (slot == 1 || slot == 3)
                .then(|| entities::parse_ip(&value).map(|ip| entities::ip_scope(ip).to_string()))
                .flatten();
            EntityRisk {
                namespace,
                column: columns[slot].into(),
                label: label_for(columns[slot]),
                value,
                evidence_level: (score / 20) as u8,
                level: if score >= 70 {
                    "alto"
                } else if score >= 40 {
                    "médio"
                } else {
                    "baixo"
                }
                .into(),
                score,
                detections: count,
                events: stat.events,
                failures: stat.failures,
                first: stat.first,
                last: stat.last,
                scope,
                tactics,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.namespace.cmp(&b.namespace))
            .then(a.column.cmp(&b.column))
            .then(a.value.cmp(&b.value))
    });
    out
}

fn rarity(maps: &[HashMap<Box<str>, RareStat>; 5], totals: &[usize; 5]) -> Vec<RareValue> {
    let mut out = Vec::new();
    for (slot, (role, column)) in RARE_ROLES.iter().enumerate() {
        let total = totals[slot];
        let distinct = maps[slot].len();
        if total < 50 || distinct < 5 || distinct as f64 / total as f64 > 0.5 {
            continue;
        }
        let limit = ((total as f64) * 0.002).max(1.0) as usize;
        let mut candidates: Vec<(&Box<str>, &RareStat)> =
            maps[slot].iter().filter(|(_, s)| s.count <= limit.min(2)).collect();
        candidates.sort_by(|a, b| a.1.count.cmp(&b.1.count).then(a.1.first.cmp(&b.1.first)));
        for (value, stat) in candidates.into_iter().take(5) {
            let info = entities::info(*role);
            let filter = match role {
                Role::Process | Role::ParentProcess => crate::query::Filter {
                    column: "_all".into(),
                    op: "query".into(),
                    value: format!("{column}:/(^|[\\\\/]){}$/", regex::escape(value).replace('/', "\\/")),
                    value2: None,
                },
                _ => crate::query::Filter {
                    column: column.to_string(),
                    op: "equals".into(),
                    value: value.to_string(),
                    value2: None,
                },
            };
            out.push(RareValue {
                column: column.to_string(),
                label: info.label.into(),
                value: value.to_string(),
                count: stat.count,
                first: stat.first,
                event_id: stat.sample,
                role_events: total,
                role_distinct: distinct,
                filter,
            });
        }
    }
    out.truncate(20);
    out
}

// ------------------------------------------------------------------ cache

pub struct CachedTriage {
    namespace: String,
    pub key: String,
    pub result: Arc<crate::security_results::Results>,
}

static TRIAGE_CACHE: Mutex<Vec<CachedTriage>> = Mutex::new(Vec::new());

/// Results are SQLite-backed; count readers without querying busy connections
/// or presenting a configured page-cache limit as allocated RAM.
pub(crate) fn resource_cache_metrics() -> Option<(usize, u64)> {
    let cache = TRIAGE_CACHE.try_lock()?;
    let mut bytes = (cache.capacity() as u64).saturating_mul(std::mem::size_of::<CachedTriage>() as u64);
    let mut seen = std::collections::HashSet::new();
    for entry in cache.iter() {
        bytes = bytes.saturating_add(entry.key.capacity() as u64);
        if seen.insert(Arc::as_ptr(&entry.result) as usize) {
            bytes = bytes.saturating_add(std::mem::size_of::<crate::security_results::Results>() as u64)
                .saturating_add(2 * std::mem::size_of::<usize>() as u64);
        }
    }
    Some((cache.len(), bytes))
}

pub fn cached(key: &str) -> Option<Arc<crate::security_results::Results>> {
    TRIAGE_CACHE.lock().iter().find(|c| c.key == key).map(|c| c.result.clone())
}

pub fn remember(key: String, result: Arc<crate::security_results::Results>) {
    let mut cache = TRIAGE_CACHE.lock();
    cache.retain(|c| c.key != key);
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push(CachedTriage { key, result, namespace: crate::analysis_runtime::cache_namespace() });
}

pub fn cached_analysis(id: &str) -> Option<Arc<crate::security_results::Results>> {
    TRIAGE_CACHE.lock().iter().find(|c| c.namespace == crate::analysis_runtime::cache_namespace() && c.result.metadata["analysis_id"].as_str() == Some(id)).map(|c| c.result.clone())
}
pub(crate) fn cached_for_chain(rule:&str)->Option<Arc<crate::security_results::Results>>{
    TRIAGE_CACHE.lock().iter().rev().find(|c|c.namespace==crate::analysis_runtime::cache_namespace()&&c.result.metadata["complete"]==true
        && c.result.metadata["rule_coverage"].as_array().is_some_and(|coverage|coverage.iter().any(|item|item["rule"]==rule))).map(|c|c.result.clone())
}
pub(crate) fn cached_for_finding(id:&str)->Result<Arc<crate::security_results::Results>,String>{
    for cached in TRIAGE_CACHE.lock().iter().rev(){if cached.namespace==crate::analysis_runtime::cache_namespace()&&cached.result.metadata["complete"]==true&&cached.result.has_finding(id)?{return Ok(cached.result.clone());}}
    Err("Achado ausente da análise atual; recalcule os comprometimentos".into())
}

#[cfg(test)]
mod external_tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_incremental_groups_remap_ids_and_reconcile_late_removed_and_reordered_events(){
        let rule:RuleDef=serde_json::from_value(serde_json::json!({"id":"checkpoint-test","name":"Checkpoint","severity":"low","kind":"threshold","where":"stage:observed","by":["@src_ip"],"window":"1h","count":3})).unwrap();let rules=test_ruleset(vec![rule],vec![]).unwrap();let settings=Settings{threats:false,investigation:crate::investigation::Settings{enabled:false,..Default::default()},..Default::default()};let inputs=Inputs{rules:&rules,catalog:None,settings:&settings};
        let fixture=(0..4).map(|id|{let mut e=Event::empty();e.id=id;e.event_ref=format!("fixture:{id}");e.source="checkpoint".into();e.timestamp=Some(1700000000000+id as i64*1000);e.fields=serde_json::json!({"source.ip":"192.0.2.1","stage":"observed"}).as_object().unwrap().clone();e}).collect::<Vec<_>>();
        let directory=tempfile::tempdir().unwrap();let path=directory.path().join("groups.sqlite");
        let evaluate=|events:&[Event],cache:&mut crate::security_checkpoints::Cache|{let source=Source::Events(events.iter().collect());let pass=Pass{full_entity_stats:false,rules:&rules,catalog:None,mappings:&[],threat_category:vec![],behavior:&settings.investigation};let mut acc=source.fold(&pass,rules.automaton.as_ref().map(|a|a.patterns_len()).unwrap_or(0)).unwrap();let store=crate::security_correlate::Store::new().unwrap();store.import(std::mem::take(&mut acc.groups)).unwrap();let raws=external_raws(&store,cache,&inputs,&acc,"dataset",rules.rules.len() as u32).unwrap();let mut actual=Vec::new();for raw in raws.into_iter().unwrap(){let raw=raw.unwrap();let mut refs=Vec::new();raw_members(&raw,&store,|id,_|{refs.push(source.lookup(id).unwrap().event_ref);Ok(())}).unwrap();refs.sort();actual.push(refs);}actual.sort();actual};
        let mut cache=crate::security_checkpoints::Cache::open(&path).unwrap();let initial=evaluate(&fixture,&mut cache);assert_eq!(initial.len(),1);assert_eq!(initial[0].len(),4);drop(cache);
        let mut cache=crate::security_checkpoints::Cache::open(&path).unwrap();let mut remapped=fixture.clone();for e in &mut remapped{e.id+=1000;}remapped.reverse();assert_eq!(evaluate(&remapped,&mut cache),initial);assert!(cache.reused>=2,"both overlapping canonical groups were committed");
        let mut late=fixture[0].clone();late.id=2000;late.event_ref="late-original".into();late.timestamp=Some(1700000001500);remapped.push(late);let actual=evaluate(&remapped,&mut cache);let mut fresh=crate::security_checkpoints::Cache::new().unwrap();assert_eq!(actual,evaluate(&remapped,&mut fresh));assert_eq!(actual[0].len(),5);
        remapped.truncate(2);assert!(evaluate(&remapped,&mut cache).is_empty());
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_external_correlations_preserve_the_slice_reference_semantics(){
        for seed in 0..12usize{
            let mut hits=(0..240).map(|i|Hit{ts:1_000_000+(i/3) as i64*997,id:i,step:((i+seed)%3) as u8,extra:Some(format!("{}",(i*17+seed)%19).into_boxed_str()),reference:format!("ref:{}",i/2),source:"test".into(),uncertain_time:false,binding:None}).collect::<Vec<_>>();
            hits.sort_by(|a,b|a.ts.cmp(&b.ts).then(a.reference.cmp(&b.reference)).then(a.step.cmp(&b.step)));
            let mut groups=crate::security_store::Groups::default();for hit in &hits{groups.push(0,"key",hit.clone()).unwrap();}
            let store=crate::security_correlate::Store::new().unwrap();store.import(groups).unwrap();let group=store.group(0,"key").unwrap();assert_eq!(group.len,hits.len());
            for window in [0,3000,12000,100000]{
                let mut actual=vec![];group.threshold(window,5,false,|i,j,_|{actual.push((i,j));Ok(())}).unwrap();assert_eq!(actual,threshold_ranges(&hits,window,5));
                let mut actual=vec![];group.threshold(window,5,true,|i,j,n|{actual.push((i,j,n));Ok(())}).unwrap();assert_eq!(actual,distinct_ranges(&hits,window,5));
                for ordered in [true,false]{let mut actual=vec![];group.sequence(window,&[2,2,1],ordered,None,|m|{actual.push(m);Ok(())}).unwrap();assert_eq!(actual,ordered_matches(&hits,window,&[2,2,1],ordered),"seed {seed}, window {window}, ordered {ordered}");}
            }
            assert_eq!(group.beacon(6).unwrap(),beacon_period(&hits,6).map(|v|v.0));
        }
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_external_hot_key_has_no_group_size_cutoff_or_member_loss(){
        let mut groups=crate::security_store::Groups::default();for i in 0..36000{groups.push(0,"hot",Hit{ts:i as i64,id:i,step:0,extra:Some("x".repeat(1024).into_boxed_str()),reference:format!("hot:{i:08}"),source:"test".into(),uncertain_time:false,binding:None}).unwrap();}
        let store=crate::security_correlate::Store::new().unwrap();store.import(groups).unwrap();let group=store.group(0,"hot").unwrap();assert_eq!(group.len,36000);
        let mut selected=None;group.threshold(100000,100,false,|i,j,_|{selected=Some(group.range(i,j)?);Ok(())}).unwrap();let mut ids=vec![];store.members(&selected.unwrap(),|id,_|{ids.push(id);Ok(())}).unwrap();assert_eq!(ids,(0..36000).collect::<Vec<_>>());
    }
}

pub fn clear_cache() {
    TRIAGE_CACHE.lock().clear();
}
