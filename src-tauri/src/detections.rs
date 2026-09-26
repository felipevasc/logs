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
use std::path::PathBuf;
use std::sync::Arc;

const BUILTIN: &str = include_str!("../resources/detection-rules.json");
const HIT_BUDGET: usize = 3_000_000;
const ENTITY_KEYS: usize = 100_000;
const RARE_KEYS: usize = 100_000;
const EVENT_SAMPLES: usize = 50;
const MAX_DETECTIONS: usize = 500;

fn yes() -> bool {
    true
}

#[derive(Clone, Deserialize, Serialize)]
pub struct StepDef {
    #[serde(rename = "where")]
    pub condition: String,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
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
    conds: Vec<Expr>,
    literals: Option<Vec<usize>>,
    window: i64,
    counts: Vec<usize>,
    by: Vec<FieldRef>,
    distinct: Option<FieldRef>,
    distinct_fallback: Vec<FieldRef>,
    attack: Vec<AttackRef>,
}

impl Compiled {
    /// Any step of the rule (evidence filter for the detection).
    pub fn matches(&self, ev: &Event) -> bool {
        let ctx = Ctx::new(ev);
        self.conds.iter().any(|c| c.matches_ctx(&ctx))
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

fn severity_weight(s: &str) -> f64 {
    match s {
        "critical" => 60.0,
        "high" => 40.0,
        "medium" => 20.0,
        "low" => 8.0,
        _ => 2.0,
    }
}

pub fn compile_rule(def: RuleDef, origin: &'static str, conds: Option<Vec<Expr>>) -> Result<Compiled, String> {
    let kind = def.kind.as_str();
    if !matches!(kind, "single" | "threshold" | "distinct" | "sequence" | "beacon") {
        return Err(format!("Tipo de regra inválido em {}: {kind}", def.id));
    }
    if severity_rank(&def.severity) == 0 && def.severity != "info" {
        return Err(format!("Severidade inválida em {}.", def.id));
    }
    let conds = match conds {
        Some(c) => c,
        None if kind == "sequence" => {
            if def.steps.len() < 2 {
                return Err(format!("A sequência {} precisa de ao menos duas etapas.", def.id));
            }
            def.steps
                .iter()
                .map(|s| querylang::compile(&s.condition).map_err(|e| format!("{}: {e}", def.id)))
                .collect::<Result<_, _>>()?
        }
        None => vec![querylang::compile(&def.condition).map_err(|e| format!("{}: {e}", def.id))?],
    };
    let counts = if kind == "sequence" {
        def.steps.iter().map(|s| s.count.unwrap_or(1).max(1)).collect()
    } else {
        vec![def.count.unwrap_or(1).max(1)]
    };
    let window = def
        .window
        .as_deref()
        .map(|w| parse_duration(w).ok_or_else(|| format!("Janela inválida em {}: {w}", def.id)))
        .transpose()?
        .unwrap_or(if kind == "single" { 3_600_000 } else { 600_000 });
    if matches!(kind, "threshold" | "distinct" | "sequence" | "beacon") && def.by.is_empty() {
        return Err(format!("A regra {} precisa de campos de agrupamento (by).", def.id));
    }
    if kind == "distinct" && def.distinct.is_none() {
        return Err(format!("A regra {} precisa do campo distinct.", def.id));
    }
    let attack = def.attack.iter().map(|id| attack::reference(id, &def.tactics)).collect();
    Ok(Compiled {
        enabled: true,
        by: def.by.iter().map(|c| querylang::field_ref(c)).collect(),
        distinct: def.distinct.as_deref().map(querylang::field_ref),
        distinct_fallback: def.distinct_fallback.iter().map(|c| querylang::field_ref(c)).collect(),
        literals: None,
        conds,
        window,
        counts,
        attack,
        origin,
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
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub suppress: Vec<Suppression>,
    /// Include threat-catalog text signals in the triage.
    #[serde(default = "yes")]
    pub threats: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { disabled: Vec::new(), suppress: Vec::new(), threats: true }
    }
}

fn settings_path() -> PathBuf {
    crate::config_dir().join("detections.json")
}

pub fn sigma_dir() -> PathBuf {
    crate::config_dir().join("sigma")
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("json.pending");
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ rule set

pub struct RuleSet {
    pub rules: Vec<Compiled>,
    automaton: Option<AhoCorasick>,
    pub sigma_loaded: usize,
    pub sigma_errors: Vec<String>,
    pub builtin: usize,
}

impl RuleSet {
    pub fn find(&self, id: &str) -> Option<&Compiled> {
        self.rules.iter().find(|r| r.def.id == id)
    }
}

fn fingerprint() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let stamp = |path: &std::path::Path, h: &mut std::collections::hash_map::DefaultHasher| {
        if let Ok(meta) = std::fs::metadata(path) {
            meta.len().hash(h);
            meta.modified().ok().hash(h);
        }
    };
    stamp(&settings_path(), &mut hasher);
    stamp(&crate::config_dir().join("detection-rules.json"), &mut hasher);
    for file in crate::sigma::rule_files(&sigma_dir()) {
        file.hash(&mut hasher);
        stamp(&file, &mut hasher);
    }
    hasher.finish()
}

static RULESET: Mutex<Option<(u64, Arc<RuleSet>)>> = Mutex::new(None);

/// Built-in rules, optional local overrides and imported Sigma rules,
/// compiled once per change of any of their files.
pub fn ruleset() -> Result<Arc<RuleSet>, String> {
    let key = fingerprint();
    if let Some((cached, set)) = RULESET.lock().as_ref() {
        if *cached == key {
            return Ok(set.clone());
        }
    }
    let settings = load_settings();
    let disabled: HashSet<&str> = settings.disabled.iter().map(String::as_str).collect();
    let mut defs: Vec<RuleDef> = serde_json::from_str::<RuleFile>(BUILTIN)
        .map_err(|e| format!("Regras de detecção embutidas inválidas: {e}"))?
        .rules;
    let builtin = defs.len();
    if let Ok(text) = std::fs::read_to_string(crate::config_dir().join("detection-rules.json")) {
        let local: RuleFile = serde_json::from_str(&text)
            .map_err(|e| format!("detection-rules.json local inválido: {e}"))?;
        for rule in local.rules {
            match defs.iter_mut().find(|d| d.id == rule.id) {
                Some(existing) => *existing = rule,
                None => defs.push(rule),
            }
        }
    }
    let (sigma, sigma_errors) = crate::sigma::load_dir(&sigma_dir());
    let set = Arc::new(build(defs, sigma, sigma_errors, &disabled, builtin)?);
    *RULESET.lock() = Some((key, set.clone()));
    Ok(set)
}

/// Built-in rules only (tests and first run without configuration).
pub fn builtin_ruleset() -> Result<RuleSet, String> {
    let defs = serde_json::from_str::<RuleFile>(BUILTIN).map_err(|e| e.to_string())?.rules;
    let count = defs.len();
    build(defs, Vec::new(), Vec::new(), &HashSet::new(), count)
}

fn build(
    defs: Vec<RuleDef>,
    sigma: Vec<Compiled>,
    sigma_errors: Vec<String>,
    disabled: &HashSet<&str>,
    builtin: usize,
) -> Result<RuleSet, String> {
    let mut rules = Vec::new();
    for def in defs {
        let enabled = def.enabled && !disabled.contains(def.id.as_str());
        let mut rule = compile_rule(def, "builtin", None)?;
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
    Ok(RuleSet { rules, automaton, sigma_loaded, sigma_errors, builtin })
}

pub fn invalidate() {
    *RULESET.lock() = None;
}

// ------------------------------------------------------------------ output

#[derive(Clone, Serialize)]
pub struct EntityRef {
    pub column: String,
    pub label: String,
    pub value: String,
}

#[derive(Clone, Serialize)]
pub struct Detection {
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
}

#[derive(Serialize)]
pub struct Episode {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: String,
    pub score: u32,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub detections: Vec<usize>,
    pub tactics: Vec<String>,
    pub entities: Vec<EntityRef>,
}

#[derive(Serialize)]
pub struct EntityRisk {
    pub column: String,
    pub label: String,
    pub value: String,
    pub score: u32,
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
}

// ------------------------------------------------------------------ pass

struct Hit {
    ts: i64,
    id: usize,
    step: u8,
    extra: Option<Box<str>>,
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
    groups: HashMap<(u32, Box<str>), Vec<Hit>>,
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
}

impl Acc {
    fn new(patterns: usize) -> Self {
        Acc {
            groups: HashMap::new(),
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
        }
    }
    fn merge(mut self, other: Acc) -> Acc {
        for (key, mut hits) in other.groups {
            self.groups.entry(key).or_default().append(&mut hits);
        }
        self.hits += other.hits;
        self.limited |= other.limited;
        for (mine, theirs) in self.entities.iter_mut().zip(other.entities) {
            for (key, stat) in theirs {
                if mine.len() >= ENTITY_KEYS && !mine.contains_key(&key) {
                    self.limited = true;
                    continue;
                }
                let entry = mine.entry(key).or_default();
                entry.events += stat.events;
                entry.failures += stat.failures;
                entry.first = min_opt(entry.first, stat.first);
                entry.last = max_opt(entry.last, stat.last);
            }
        }
        for (mine, theirs) in self.rare.iter_mut().zip(other.rare) {
            for (key, stat) in theirs {
                if mine.len() >= RARE_KEYS && !mine.contains_key(&key) {
                    continue;
                }
                let entry = mine.entry(key).or_default();
                if entry.count == 0 || stat.first.is_some_and(|f| entry.first.is_none_or(|e| f < e)) {
                    entry.sample = stat.sample;
                }
                entry.count += stat.count;
                entry.first = min_opt(entry.first, stat.first);
            }
        }
        for (a, b) in self.rare_totals.iter_mut().zip(other.rare_totals) {
            *a += b;
        }
        for (a, b) in self.coverage.iter_mut().zip(other.coverage) {
            *a += b;
        }
        self.total += other.total;
        self.undated += other.undated;
        self.first = min_opt(self.first, other.first);
        self.last = max_opt(self.last, other.last);
        self
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
    for part in [&ev.message, &ev.source, &ev.code, &ev.name, &ev.description] {
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
    rules: &'a RuleSet,
    catalog: Option<&'a CompiledCatalog>,
    /// Category index per threat rule (synthetic rule ids follow the real ones).
    threat_category: Vec<u32>,
}

impl Pass<'_> {
    fn group_key(&self, ctx: &Ctx<'_>, by: &[FieldRef], required: bool) -> Option<Box<str>> {
        let mut key = String::new();
        for (i, field) in by.iter().enumerate() {
            match ctx.get(field) {
                Some(v) if !v.trim().is_empty() => {
                    if i > 0 {
                        key.push('\u{1f}');
                    }
                    key.push_str(v.trim());
                }
                _ if required => return None,
                _ => {
                    if i > 0 {
                        key.push('\u{1f}');
                    }
                }
            }
        }
        Some(key.into_boxed_str())
    }

    fn step(&self, acc: &mut Acc, ev: &Event) {
        let ctx = Ctx::new(ev);
        acc.total += 1;
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
            if let Some(literals) = &rule.literals {
                if !literals.iter().any(|&l| acc.stamps[l] == acc.stamp) {
                    continue;
                }
            }
            let kind = rule.def.kind.as_str();
            for (step, cond) in rule.conds.iter().enumerate() {
                if !cond.matches_ctx(&ctx) {
                    continue;
                }
                if kind != "single" && ev.timestamp.is_none() {
                    break;
                }
                let Some(key) = self.group_key(&ctx, &rule.by, kind != "single") else { break };
                let extra = match kind {
                    "distinct" => {
                        let value = rule
                            .distinct
                            .as_ref()
                            .and_then(|f| ctx.get(f))
                            .or_else(|| rule.distinct_fallback.iter().find_map(|f| ctx.get(f)));
                        match value {
                            Some(v) if !v.trim().is_empty() => Some(v.trim().chars().take(512).collect::<String>().into_boxed_str()),
                            _ => break,
                        }
                    }
                    _ => None,
                };
                if acc.hits >= HIT_BUDGET {
                    acc.limited = true;
                    break;
                }
                acc.hits += 1;
                acc.groups
                    .entry((index as u32, key))
                    .or_default()
                    .push(Hit { ts, id: ev.id, step: step as u8, extra });
            }
        }
        if let Some(catalog) = self.catalog {
            let found = catalog.event_hits(ev);
            if !found.is_empty() {
                let key: Box<str> = [(Role::SrcIp, "@src_ip"), (Role::Host, "@host"), (Role::User, "@user")]
                    .iter()
                    .find_map(|(r, column)| ctx.role(*r).map(|v| format!("{column}\u{1e}{v}")))
                    .unwrap_or_default()
                    .into();
                let base = self.rules.rules.len() as u32;
                let mut seen = HashSet::new();
                for rule in found {
                    let category = self.threat_category[rule];
                    if acc.hits >= HIT_BUDGET {
                        acc.limited = true;
                        break;
                    }
                    let extra: Box<str> = catalog.rule(rule).id.clone().into();
                    if !seen.insert((category, rule)) {
                        continue;
                    }
                    acc.hits += 1;
                    acc.groups
                        .entry((base + category, key.clone()))
                        .or_default()
                        .push(Hit { ts, id: ev.id, step: 0, extra: Some(extra) });
                }
            }
        }
        let failure = ctx.role(Role::Outcome) == Some("failure");
        for (slot, role) in ENTITY_ROLES.iter().enumerate() {
            let Some(value) = ctx.role(*role) else { continue };
            let map = &mut acc.entities[slot];
            if map.len() >= ENTITY_KEYS && !map.contains_key(value) {
                acc.limited = true;
                continue;
            }
            let entry = map.entry(value.into()).or_default();
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

fn sequence_matches(hits: &[Hit], window: i64, counts: &[usize]) -> Vec<Vec<usize>> {
    let steps = counts.len();
    let mut out = Vec::new();
    let mut first_step: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut stage = 0usize;
    let mut stage_count = 0usize;
    let mut members: Vec<usize> = Vec::new();
    let mut start = 0i64;
    for (k, hit) in hits.iter().enumerate() {
        let step = hit.step as usize;
        if stage > 0 && hit.ts - start > window {
            stage = 0;
            stage_count = 0;
            members.clear();
        }
        if step == 0 {
            first_step.push_back(k);
            while let Some(&front) = first_step.front() {
                if hit.ts - hits[front].ts > window {
                    first_step.pop_front();
                } else {
                    break;
                }
            }
            if stage > 0 {
                members.push(k);
                continue;
            }
            if first_step.len() >= counts[0] {
                stage = 1;
                stage_count = 0;
                start = hits[*first_step.front().unwrap()].ts;
                members = first_step.iter().copied().collect();
            }
            continue;
        }
        if stage == 0 {
            continue;
        }
        if step == stage {
            stage_count += 1;
            members.push(k);
            if stage_count >= counts[stage] {
                stage += 1;
                stage_count = 0;
                if stage == steps {
                    out.push(std::mem::take(&mut members));
                    stage = 0;
                    first_step.clear();
                }
            }
        } else if step < stage {
            members.push(k);
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
        if (m - m.round()).abs() < 0.05 { format!("{} min", m.round()) } else { format!("{m:.1} min") }
    } else {
        format!("{:.1} h", ms as f64 / 3_600_000.0)
    }
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// ------------------------------------------------------------------ assembly

struct Raw {
    rule: u32,
    key: Box<str>,
    members: Vec<usize>,
    distinct: usize,
    period: Option<i64>,
    extras: Vec<String>,
}

fn label_for(column: &str) -> String {
    entities::role_of_column(column)
        .map(|r| entities::info(r).label.to_string())
        .unwrap_or_else(|| column.to_string())
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
    fn fold(&self, pass: &Pass<'_>, patterns: usize) -> Acc {
        use rayon::prelude::*;
        match self {
            Source::Selection(selection) => selection.par_fold(|| Acc::new(patterns), |acc, ev| pass.step(acc, ev), Acc::merge),
            Source::Events(events) => {
                let generation = crate::operations::current_generation();
                events
                    .par_chunks(1024)
                    .fold(
                        || Acc::new(patterns),
                        |mut acc, chunk| {
                            for ev in chunk {
                                if crate::operations::cancelled_for(generation) {
                                    break;
                                }
                                pass.step(&mut acc, ev);
                            }
                            acc
                        },
                    )
                    .reduce(|| Acc::new(patterns), Acc::merge)
            }
        }
    }
    fn lookup(&self, id: usize) -> Option<Event> {
        match self {
            Source::Selection(selection) => selection.event(id),
            Source::Events(events) => events.iter().find(|e| e.id == id).map(|e| (*e).clone()),
        }
    }
}

pub fn run(inputs: &Inputs<'_>, source: &Source<'_>) -> Result<Triage, String> {
    let started = std::time::Instant::now();
    let catalog = inputs.catalog.filter(|c| c.has_enabled() && inputs.settings.threats);
    let rule_count_in_catalog = catalog.map(|c| c.rule_count()).unwrap_or(0);
    let (threat_category, category_names) = match catalog {
        Some(c) => threat_categories(c, rule_count_in_catalog),
        None => (Vec::new(), Vec::new()),
    };
    let pass = Pass { rules: inputs.rules, catalog, threat_category };
    let patterns = inputs.rules.automaton.as_ref().map(|a| a.patterns_len()).unwrap_or(0);
    let acc = source.fold(&pass, patterns);
    let lookup = |id: usize| source.lookup(id);
    crate::operations::check()?;
    let base = inputs.rules.rules.len() as u32;

    // Group hits into raw detections per rule kind.
    let mut raws: Vec<Raw> = Vec::new();
    let mut groups: Vec<((u32, Box<str>), Vec<Hit>)> = acc.groups.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    for ((rule_index, key), mut hits) in groups {
        crate::operations::check()?;
        hits.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.id.cmp(&b.id)));
        let split_at = |hits: &[Hit], k: usize, gap: i64| {
            k == hits.len()
                || (hits[k].ts != i64::MIN && hits[k - 1].ts != i64::MIN && hits[k].ts - hits[k - 1].ts > gap)
        };
        if rule_index >= base {
            // Threat signals behave as single detections merged within an hour.
            let mut start = 0;
            for k in 1..=hits.len() {
                if split_at(&hits, k, 3_600_000) {
                    let segment = &hits[start..k];
                    let mut extras: Vec<String> = segment.iter().filter_map(|h| h.extra.as_deref().map(str::to_string)).collect();
                    extras.sort();
                    extras.dedup();
                    raws.push(Raw { rule: rule_index, key: key.clone(), members: segment.iter().map(|h| h.id).collect(), distinct: 0, period: None, extras });
                    start = k;
                }
            }
            continue;
        }
        let rule = &inputs.rules.rules[rule_index as usize];
        let window = rule.window;
        let mut push = |members: Vec<usize>, distinct: usize, period: Option<i64>| {
            raws.push(Raw { rule: rule_index, key: key.clone(), members: members.iter().map(|&m| hits[m].id).collect(), distinct, period, extras: Vec::new() });
        };
        match rule.def.kind.as_str() {
            "single" => {
                let mut start = 0;
                for k in 1..=hits.len() {
                    if split_at(&hits, k, window) {
                        push((start..k).collect(), 0, None);
                        start = k;
                    }
                }
            }
            "threshold" => {
                for (i, j) in threshold_ranges(&hits, window, rule.counts[0]) {
                    push((i..=j).collect(), 0, None);
                }
            }
            "distinct" => {
                for (i, j, n) in distinct_ranges(&hits, window, rule.counts[0]) {
                    push((i..=j).collect(), n, None);
                }
            }
            "sequence" => {
                for members in sequence_matches(&hits, window, &rule.counts) {
                    push(members, 0, None);
                }
            }
            "beacon" => {
                if let Some((period, _)) = beacon_period(&hits, rule.counts[0]) {
                    push((0..hits.len()).collect(), 0, Some(period));
                }
            }
            _ => {}
        }
    }

    // Timestamps per event id for the detection bounds.
    let mut detections: Vec<Detection> = Vec::new();
    let mut suppressed = 0usize;
    let settings = inputs.settings;
    for raw in raws {
        let ids = raw.members;
        if ids.is_empty() {
            continue;
        }
        let (name, description, severity, attack_refs, kind, origin, by_columns, rule_id): (String, String, String, Vec<AttackRef>, String, String, Vec<String>, String);
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
            filters.push(crate::query::Filter { column: "_all".into(), op: "query".into(), value: query, value2: None });
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
            filters.push(crate::query::Filter { column: "_all".into(), op: "detection".into(), value: rule.def.id.clone(), value2: None });
        }
        // Entities from the grouping key.
        let mut entity_refs: Vec<EntityRef> = Vec::new();
        let key_parts: Vec<&str> = if raw.rule >= base { Vec::new() } else { raw.key.split('\u{1f}').collect() };
        for (column, value) in by_columns.iter().zip(&key_parts) {
            if value.is_empty() {
                continue;
            }
            entity_refs.push(EntityRef { column: column.clone(), label: label_for(column), value: value.to_string() });
            filters.push(crate::query::Filter { column: column.clone(), op: "equals_exact".into(), value: value.to_string(), value2: None });
        }
        if raw.rule >= base {
            if let Some((column, value)) = raw.key.split_once('\u{1e}') {
                entity_refs.push(EntityRef { column: column.into(), label: label_for(column), value: value.to_string() });
                filters.push(crate::query::Filter { column: column.into(), op: "equals_exact".into(), value: value.to_string(), value2: None });
            }
        }
        // Suppressions by rule and entity.
        let hidden = settings.suppress.iter().any(|s| {
            s.rule == rule_id
                && match (&s.column, &s.value) {
                    (Some(column), Some(value)) => entity_refs.iter().any(|e| &e.column == column && &e.value == value),
                    (None, Some(value)) => entity_refs.iter().any(|e| &e.value == value),
                    _ => true,
                }
        });
        if hidden {
            suppressed += 1;
            continue;
        }
        // The primary tactic of each technique keeps episode chains honest
        // (Valid Accounts, for example, is listed under four tactics).
        let mut tactics: Vec<String> = attack_refs.iter().filter_map(|a| a.tactics.first().cloned()).collect();
        if let Some(rule) = (raw.rule < base).then(|| &inputs.rules.rules[raw.rule as usize]) {
            tactics.extend(rule.def.tactics.iter().cloned());
        }
        tactics.sort_by_key(|t| attack::tactic_order(t));
        tactics.dedup();
        detections.push(Detection {
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
            count: ids.len(),
            entities: entity_refs,
            summary: String::new(),
            distinct: raw.distinct,
            period_ms: raw.period,
            event_ids: ids,
            filters,
        });
    }

    // Bounds, samples and summaries (materializes one event per detection).
    detections.sort_by(|a, b| severity_rank(&b.severity).cmp(&severity_rank(&a.severity)).then(b.count.cmp(&a.count)));
    let limited_detections = detections.len() > MAX_DETECTIONS;
    detections.truncate(MAX_DETECTIONS);
    for (n, detection) in detections.iter_mut().enumerate() {
        crate::operations::check()?;
        let first = lookup(detection.event_ids[0]);
        let last = lookup(*detection.event_ids.last().unwrap());
        detection.start = first.as_ref().and_then(|e| e.timestamp);
        detection.end = last.as_ref().and_then(|e| e.timestamp).or(detection.start);
        let template = if detection.origin == "threats" {
            None
        } else {
            inputs.rules.find(&detection.rule).and_then(|r| r.def.summary.clone())
        };
        detection.summary = render_summary(template.as_deref(), detection, first.as_ref());
        if detection.event_ids.len() > EVENT_SAMPLES {
            let head = EVENT_SAMPLES / 2;
            let tail: Vec<usize> = detection.event_ids[detection.event_ids.len() - head..].to_vec();
            detection.event_ids.truncate(head);
            detection.event_ids.extend(tail);
        }
        detection.id = format!("d{n}-{}", detection.rule);
    }
    detections.sort_by(|a, b| a.start.unwrap_or(i64::MAX).cmp(&b.start.unwrap_or(i64::MAX)).then(severity_rank(&b.severity).cmp(&severity_rank(&a.severity))));

    let episodes = cluster(&detections);
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
        total: acc.total,
        undated: acc.undated,
        start: acc.first,
        end: acc.last,
        complete: !crate::operations::cancelled(),
        limited: acc.limited || limited_detections,
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
            column => detection
                .entities
                .iter()
                .find(|e| e.column == column)
                .map(|e| e.value.clone())
                .or_else(|| ctx.as_ref().and_then(|c| c.get(&querylang::field_ref(column)).map(|v| v.chars().take(120).collect()))),
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
                    if a.tactics.first().is_some_and(|t| t == tactic.key) || (a.tactics.is_empty() && d.tactics.iter().any(|t| t == tactic.key)) {
                        hit = true;
                        let entry = techniques.entry(a.id.clone()).or_insert((a.name.clone(), 0));
                        entry.1 += 1;
                    }
                }
                count += usize::from(hit);
            }
            let mut techniques: Vec<TechniqueCount> = techniques
                .into_iter()
                .map(|(id, (name, count))| TechniqueCount { id, name, count })
                .collect();
            techniques.sort_by(|a, b| b.count.cmp(&a.count));
            TacticSummary { id: tactic.id.into(), key: tactic.key.into(), label: tactic.label.into(), count, techniques }
        })
        .collect()
}

fn cluster(detections: &[Detection]) -> Vec<Episode> {
    let n = detections.len().min(2000);
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
    const GAP: i64 = 3_600_000;
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&detections[i], &detections[j]);
            let close = match (a.start, a.end, b.start, b.end) {
                (Some(a0), Some(a1), Some(b0), Some(b1)) => b0 <= a1 + GAP && a0 <= b1 + GAP,
                _ => false,
            };
            if !close {
                continue;
            }
            let shared = a.entities.iter().any(|x| b.entities.iter().any(|y| x.value == y.value))
                || a.event_ids.iter().any(|id| b.event_ids.contains(id));
            if shared {
                let (ra, rb) = (find(&mut parent, i), find(&mut parent, j));
                if ra != rb {
                    parent[rb] = ra;
                }
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
            let score: f64 = members.iter().map(|&i| severity_weight(&detections[i].severity)).sum();
            let mut tactics: Vec<String> = members.iter().flat_map(|&i| detections[i].tactics.clone()).collect();
            tactics.sort_by_key(|t| attack::tactic_order(t));
            tactics.dedup();
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
            let title = if members.len() > 1 && tactics.len() >= 3 {
                let first = attack::tactic(&tactics[0]).map(|t| t.label).unwrap_or("");
                let last = attack::tactic(tactics.last().unwrap()).map(|t| t.label).unwrap_or("");
                format!("Possível cadeia de ataque: {first} → {last}")
            } else {
                lead.name.clone()
            };
            let summary = if distinct_names.len() > 1 {
                let shown: Vec<&str> = distinct_names.iter().take(3).copied().collect();
                let more = distinct_names.len().saturating_sub(3);
                if more > 0 {
                    format!("{} e mais {more}", shown.join(" · "))
                } else {
                    shown.join(" · ")
                }
            } else {
                lead.summary.clone()
            };
            let mut detections_in_time = members.clone();
            detections_in_time.sort_by_key(|&i| detections[i].start);
            Episode {
                id: format!("e{}", ordered[0]),
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
    episodes.sort_by(|a, b| severity_rank(&b.severity).cmp(&severity_rank(&a.severity)).then(b.score.cmp(&a.score)));
    episodes.truncate(40);
    episodes
}

fn risk(detections: &[Detection], stats: &[HashMap<Box<str>, EntityStat>; 4]) -> Vec<EntityRisk> {
    let columns = ["@user", "@src_ip", "@host", "@dst_ip"];
    let mut scores: HashMap<(usize, String), (f64, usize, Vec<String>)> = HashMap::new();
    for d in detections {
        let weight = severity_weight(&d.severity) * if d.origin == "threats" { 0.3 } else { 1.0 };
        let bonus = 1.0 + (d.count.max(1) as f64).log2() / 8.0;
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
            let entry = scores.entry((slot, e.value.clone())).or_insert((0.0, 0, Vec::new()));
            entry.0 += weight * bonus;
            entry.1 += 1;
            entry.2.extend(d.tactics.iter().cloned());
        }
    }
    let mut out: Vec<EntityRisk> = scores
        .into_iter()
        .map(|((slot, value), (score, count, mut tactics))| {
            tactics.sort_by_key(|t| attack::tactic_order(t));
            tactics.dedup();
            let stat = stats[slot].get(value.as_str()).cloned().unwrap_or_default();
            let score = score.min(100.0).round() as u32;
            let scope = (slot == 1 || slot == 3)
                .then(|| entities::parse_ip(&value).map(|ip| entities::ip_scope(ip).to_string()))
                .flatten();
            EntityRisk {
                column: columns[slot].into(),
                label: label_for(columns[slot]),
                value,
                level: if score >= 70 { "alto" } else if score >= 40 { "médio" } else { "baixo" }.into(),
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
    out.sort_by(|a, b| b.score.cmp(&a.score).then(b.detections.cmp(&a.detections)).then(b.events.cmp(&a.events)));
    out.truncate(15);
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
        let mut candidates: Vec<(&Box<str>, &RareStat)> = maps[slot]
            .iter()
            .filter(|(_, s)| s.count <= limit.min(2))
            .collect();
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
                _ => crate::query::Filter { column: column.to_string(), op: "equals".into(), value: value.to_string(), value2: None },
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
    pub key: String,
    pub result: Arc<serde_json::Value>,
}

static TRIAGE_CACHE: Mutex<Vec<CachedTriage>> = Mutex::new(Vec::new());

pub fn cached(key: &str) -> Option<Arc<serde_json::Value>> {
    TRIAGE_CACHE.lock().iter().find(|c| c.key == key).map(|c| c.result.clone())
}

pub fn remember(key: String, result: Arc<serde_json::Value>) {
    let mut cache = TRIAGE_CACHE.lock();
    cache.retain(|c| c.key != key);
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push(CachedTriage { key, result });
}

pub fn clear_cache() {
    TRIAGE_CACHE.lock().clear();
}

pub(crate) fn quote_value(value: &str) -> String {
    quote(value)
}
