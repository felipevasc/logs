//! Local indicators, with bounded evidence and one RegexSet pass per record.
//! A hit describes a logged string; it does not prove exploitation or compromise.
use crate::{
    model::Event,
    operations,
    query::{self, Filter},
    sources, AppState, SourceData,
};
use parking_lot::Mutex;
use regex::bytes::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::Manager;

const CATALOG_BYTES: usize = 4 * 1024 * 1024;
const CORPUS_BYTES: usize = 64 * 1024;
use crate::event_preview::preview;
#[cfg(test)]
use crate::event_preview::PREVIEW_BYTES;
const BUILTIN: &str = include_str!("../resources/threat-rules.json");

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub category: String,
    pub severity: String,
    pub kind: String,
    pub pattern: String,
    pub description: String,
    pub enabled: bool,
    #[serde(default)]
    pub references: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    version: u32,
    name: String,
    rules: Vec<Rule>,
}
pub(crate) struct CompiledCatalog {
    file: CatalogFile,
    regexes: Vec<Regex>,
    set: RegexSet,
    enabled: Vec<usize>,
}
struct Cached {
    path: PathBuf,
    hash: [u8; 32],
    result: Result<Arc<CompiledCatalog>, String>,
}
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
static UPDATE_LOCK: Mutex<()> = Mutex::new(());

fn compile(bytes: &[u8]) -> Result<Arc<CompiledCatalog>, String> {
    if bytes.len() > CATALOG_BYTES {
        return Err("O catálogo excede 4 MiB.".into());
    }
    let file: CatalogFile =
        serde_json::from_slice(bytes).map_err(|e| format!("Catálogo JSON inválido: {e}"))?;
    if file.version != 1
        || file.name.trim().is_empty()
        || file.name.len() > 160
        || file.rules.len() > 1000
    {
        return Err("O catálogo requer version: 1, nome e no máximo 1.000 regras.".into());
    }
    let mut ids = HashSet::new();
    let mut regexes = Vec::new();
    let mut enabled = Vec::new();
    for (index, rule) in file.rules.iter().enumerate() {
        operations::check()?;
        if rule.id.is_empty()
            || rule.id.len() > 100
            || !rule
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            || !ids.insert(rule.id.clone())
        {
            return Err(format!("ID de regra inválido ou duplicado: {}", rule.id));
        }
        if rule.name.trim().is_empty()
            || rule.name.len() > 240
            || rule.category.trim().is_empty()
            || rule.category.len() > 100
            || rule.description.len() > 4000
            || rule.pattern.is_empty()
            || rule.pattern.len() > 8192
            || !matches!(rule.severity.as_str(), "high" | "medium" | "low")
            || !matches!(rule.kind.as_str(), "attempt" | "response" | "indicator")
            || rule.references.len() > 10
        {
            return Err(format!(
                "Metadados ou limites inválidos na regra {}.",
                rule.id
            ));
        }
        for reference in &rule.references {
            let url = reqwest::Url::parse(reference)
                .map_err(|_| format!("Referência inválida na regra {}.", rule.id))?;
            if reference.len() > 2048
                || !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(format!("Referência HTTP(S) inválida na regra {}.", rule.id));
            }
        }
        let regex = RegexBuilder::new(&rule.pattern)
            // Security signatures use ASCII case folding and character classes
            // by default. Authors may opt in to Unicode with (?u) explicitly.
            .unicode(false)
            .size_limit(256 * 1024)
            .dfa_size_limit(64 * 1024)
            .build()
            .map_err(|e| format!("Regex inválida ou complexa na regra {}: {e}", rule.id))?;
        if regex.is_match(b"") {
            return Err(format!(
                "A regra {} também corresponde a texto vazio. Use um padrão específico.",
                rule.id
            ));
        }
        regexes.push(regex);
        if rule.enabled {
            enabled.push(index);
        }
    }
    let set = RegexSetBuilder::new(enabled.iter().map(|&i| file.rules[i].pattern.as_str()))
        .unicode(false)
        .size_limit(32 * 1024 * 1024)
        .dfa_size_limit(16 * 1024 * 1024)
        .build()
        .map_err(|e| format!("O conjunto de regras excedeu o orçamento de compilação: {e}"))?;
    Ok(Arc::new(CompiledCatalog {
        file,
        regexes,
        set,
        enabled,
    }))
}

fn path() -> PathBuf {
    crate::config_dir().join("threat-rules.json")
}
fn load_path(path: &Path) -> Result<Arc<CompiledCatalog>, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Não foi possível preparar o catálogo: {e}"))?;
    }
    if !path.exists() {
        let mut file =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("Pasta do catálogo inválida.")?)
                .map_err(|e| format!("Não foi possível preparar o catálogo: {e}"))?;
        file.write_all(BUILTIN.as_bytes())
            .and_then(|_| file.as_file().sync_all())
            .map_err(|e| format!("Não foi possível criar o catálogo: {e}"))?;
        if let Err(error) = file.persist_noclobber(path) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!(
                    "Não foi possível criar o catálogo: {}",
                    error.error
                ));
            }
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(CATALOG_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("Não foi possível ler o catálogo: {e}"))?;
    if bytes.len() > CATALOG_BYTES {
        return Err("O catálogo excede 4 MiB; o arquivo foi preservado.".into());
    }
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let mut cached = CACHE.lock();
    if let Some(entry) = cached
        .as_ref()
        .filter(|entry| entry.path == path && entry.hash == hash)
    {
        return entry.result.clone();
    }
    let result = compile(&bytes);
    operations::check()?;
    *cached = Some(Cached {
        path: path.to_owned(),
        hash,
        result: result.clone(),
    });
    result
}

pub(crate) struct RuleMatcher {
    catalog: Arc<CompiledCatalog>,
    rule: Option<usize>,
}
pub(crate) fn matcher(
    id: &str,
    catalog: Option<Arc<CompiledCatalog>>,
) -> Result<RuleMatcher, String> {
    let catalog = catalog.map(Ok).unwrap_or_else(|| load_path(&path()))?;
    let rule = if id == "*" {
        None
    } else {
        let i = catalog
            .file
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| {
                format!("Regra de ameaça não encontrada: {id}. Atualize ou remova o filtro.")
            })?;
        if !catalog.file.rules[i].enabled {
            return Err(format!(
                "A regra {id} está desativada. Ative-a ou remova o filtro."
            ));
        }
        Some(i)
    };
    Ok(RuleMatcher { catalog, rule })
}
impl RuleMatcher {
    pub(crate) fn matches(&self, event: &Event) -> bool {
        self.matches_corpus(&corpus(event))
    }
    fn matches_corpus(&self, corpus: &Corpus) -> bool {
        self.rule
            .map(|i| self.catalog.regexes[i].is_match(corpus.text.as_bytes()))
            .unwrap_or_else(|| self.catalog.set.is_match(corpus.text.as_bytes()))
    }
}

fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
struct Corpus {
    text: String,
    original_len: usize,
    clipped: bool,
}
fn append(corpus: &mut Corpus, text: &str) {
    if text.is_empty() {
        return;
    }
    let remaining = CORPUS_BYTES.saturating_sub(corpus.text.len());
    if remaining <= 1 {
        corpus.clipped = true;
        return;
    }
    if !corpus.text.is_empty() {
        corpus.text.push('\n');
    }
    let part = prefix(text, CORPUS_BYTES.saturating_sub(corpus.text.len()));
    corpus.clipped |= part.len() < text.len();
    corpus.text.push_str(part);
}
fn percent_decode(text: &str) -> String {
    let hex = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn unicode_escapes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text.as_bytes()[at] == b'\\'
            && text.as_bytes().get(at + 1) == Some(&b'u')
            && at + 6 <= text.len()
        {
            if let Some(hex) = text.get(at + 2..at + 6) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        result.push(ch);
                        at += 6;
                        continue;
                    }
                }
            }
        }
        let ch = text[at..].chars().next().unwrap();
        result.push(ch);
        at += ch.len_utf8();
    }
    result
}
fn corpus(event: &Event) -> Corpus {
    let mut result = Corpus {
        text: String::new(),
        original_len: 0,
        clipped: false,
    };
    append(&mut result, &event.message);
    if event.description != event.message {
        append(&mut result, &event.description);
    }
    if event.raw != event.message {
        append(&mut result, &event.raw);
    }
    // Parsed strings preserve backslashes/quotes that JSON serialization would
    // escape again. Include field names, but bound recursion and visited nodes.
    fn visit_value(body: &mut Corpus, value: &serde_json::Value, depth: usize, nodes: &mut usize) {
        if *nodes >= 1000 || depth > 12 || body.text.len() >= CORPUS_BYTES {
            body.clipped = true;
            return;
        }
        *nodes += 1;
        match value {
            serde_json::Value::String(text) => append(body, text),
            serde_json::Value::Array(values) => {
                for item in values {
                    if *nodes >= 1000 || body.text.len() >= CORPUS_BYTES {
                        body.clipped = true;
                        break;
                    }
                    visit_value(body, item, depth + 1, nodes);
                }
            }
            serde_json::Value::Object(fields) => {
                for (key, item) in fields {
                    if *nodes >= 1000 || body.text.len() >= CORPUS_BYTES {
                        body.clipped = true;
                        break;
                    }
                    append(body, key);
                    visit_value(body, item, depth + 1, nodes);
                }
            }
            other => append(body, &other.to_string()),
        }
    }
    let mut nodes = 0;
    for (key, item) in &event.fields {
        if result.text.len() >= CORPUS_BYTES || nodes >= 1000 {
            result.clipped = true;
            break;
        }
        if item
            .as_str()
            .is_some_and(|s| s == event.message || s == event.raw || s == event.description)
        {
            continue;
        }
        append(&mut result, key);
        visit_value(&mut result, item, 0, &mut nodes);
    }
    result.original_len = result.text.len();
    let original = result.text.clone();
    let mut previous = original;
    for _ in 0..2 {
        let decoded = unicode_escapes(&percent_decode(&previous));
        if decoded == previous {
            break;
        }
        append(&mut result, &decoded);
        previous = decoded;
    }
    result
}

#[derive(Serialize)]
pub struct CatalogInfo {
    path: String,
    name: String,
    version: u32,
    rules: Vec<Rule>,
    enabled: usize,
    categories: Vec<String>,
    updates_available: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}
fn info_for(path: &Path, catalog: &CompiledCatalog, bundled: &CatalogFile) -> CatalogInfo {
    let ids: HashSet<_> = catalog
        .file
        .rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect();
    CatalogInfo {
        path: path.to_string_lossy().into_owned(),
        name: catalog.file.name.clone(),
        version: catalog.file.version,
        rules: catalog.file.rules.clone(),
        enabled: catalog.enabled.len(),
        categories: catalog
            .file
            .rules
            .iter()
            .map(|rule| rule.category.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        updates_available: bundled
            .rules
            .iter()
            .filter(|rule| !ids.contains(rule.id.as_str()))
            .count(),
        error: None,
    }
}
fn catalog_info() -> CatalogInfo {
    let path = path();
    match load_path(&path) {
        Ok(catalog) => info_for(
            &path,
            &catalog,
            &serde_json::from_str(BUILTIN).expect("bundled catalog JSON"),
        ),
        Err(error) => CatalogInfo {
            path: path.to_string_lossy().into_owned(),
            name: "Catálogo de ameaças".into(),
            version: 1,
            rules: vec![],
            enabled: 0,
            categories: vec![],
            updates_available: 0,
            error: Some(error),
        },
    }
}

#[derive(Serialize)]
pub struct CatalogUpdate {
    added: usize,
    backup_path: Option<String>,
    catalog: CatalogInfo,
}
fn read_catalog_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(CATALOG_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Não foi possível ler o catálogo: {error}"))?;
    if bytes.len() > CATALOG_BYTES {
        return Err("O catálogo excede 4 MiB; o arquivo foi preservado.".into());
    }
    Ok(bytes)
}
fn update_catalog_path(path: &Path, bundled: &[u8]) -> Result<CatalogUpdate, String> {
    let _guard = UPDATE_LOCK.lock();
    let original = read_catalog_bytes(path)?;
    let existing = compile(&original)?;
    let bundled: CatalogFile = serde_json::from_slice(bundled)
        .map_err(|error| format!("Catálogo distribuído inválido: {error}"))?;
    let ids: HashSet<_> = existing
        .file
        .rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect();
    let additions: Vec<_> = bundled
        .rules
        .iter()
        .filter(|rule| !ids.contains(rule.id.as_str()))
        .cloned()
        .collect();
    let added = additions.len();
    if added == 0 {
        return Ok(CatalogUpdate {
            added,
            backup_path: None,
            catalog: info_for(path, &existing, &bundled),
        });
    }
    let mut merged = existing.file.clone();
    merged.rules.extend(additions);
    let bytes = serde_json::to_vec_pretty(&merged)
        .map_err(|error| format!("Não foi possível preparar a atualização: {error}"))?;
    let checked = compile(&bytes)?;
    let directory = path.parent().ok_or("Pasta do catálogo inválida.")?;
    let mut replacement = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("Não foi possível preparar a atualização: {error}"))?;
    replacement
        .write_all(&bytes)
        .and_then(|_| replacement.as_file().sync_all())
        .map_err(|error| format!("Não foi possível preparar a atualização: {error}"))?;
    operations::check()?;
    if read_catalog_bytes(path)? != original {
        return Err(
            "O catálogo mudou durante a atualização. Reabra Regras e tente novamente.".into(),
        );
    }
    let mut backup = tempfile::Builder::new()
        .prefix("threat-rules.backup-")
        .suffix(".json")
        .tempfile_in(directory)
        .map_err(|error| format!("Não foi possível criar backup: {error}"))?;
    backup
        .write_all(&original)
        .and_then(|_| backup.as_file().sync_all())
        .map_err(|error| format!("Não foi possível salvar backup: {error}"))?;
    let (_, backup_path) = backup
        .keep()
        .map_err(|error| format!("Não foi possível preservar backup: {error}"))?;
    operations::check()?;
    if read_catalog_bytes(path)? != original {
        return Err(
            "O catálogo mudou durante a atualização; o arquivo atual foi preservado.".into(),
        );
    }
    replacement.persist(path).map_err(|error| {
        format!(
            "Não foi possível atualizar o catálogo; backup preservado em {}: {}",
            backup_path.display(),
            error.error
        )
    })?;
    operations::commit();
    *CACHE.lock() = Some(Cached {
        path: path.to_owned(),
        hash: Sha256::digest(&bytes).into(),
        result: Ok(checked.clone()),
    });
    Ok(CatalogUpdate {
        added,
        backup_path: Some(backup_path.to_string_lossy().into_owned()),
        catalog: info_for(path, &checked, &bundled),
    })
}
#[derive(Serialize)]
pub struct Count {
    name: String,
    count: usize,
}
#[derive(Serialize)]
pub struct TimeCount {
    timestamp: i64,
    count: usize,
}
#[derive(Serialize)]
pub struct Example {
    event_id: usize,
    event_ref: String,
    timestamp: Option<i64>,
    snippet: String,
    normalized: bool,
}
#[derive(Serialize)]
pub struct RuleResult {
    #[serde(flatten)]
    rule: Rule,
    count: usize,
    examples: Vec<Example>,
    start: Option<i64>,
    end: Option<i64>,
}
#[derive(Serialize)]
pub struct ScanResult {
    total: usize,
    matched: usize,
    occurrences: usize,
    complete: bool,
    clipped_records: usize,
    enabled_rules: usize,
    catalog_path: String,
    categories: Vec<Count>,
    rules: Vec<RuleResult>,
    time: Vec<TimeCount>,
    time_bucket_ms: i64,
    sources: Vec<Count>,
    undated_matches: usize,
    corpus_limit: usize,
    start: Option<i64>,
    end: Option<i64>,
}
struct Timeline {
    width: i64,
    bins: BTreeMap<i64, usize>,
}
impl Timeline {
    fn new() -> Self {
        Self {
            width: 1000,
            bins: BTreeMap::new(),
        }
    }
    fn push(&mut self, timestamp: i64) {
        while self
            .bins
            .first_key_value()
            .zip(self.bins.last_key_value())
            .is_some_and(|((lo, _), (hi, _))| {
                let bucket = timestamp.div_euclid(self.width);
                hi.max(&bucket).saturating_sub(*lo.min(&bucket)) >= 120
            })
        {
            self.width = self.width.saturating_mul(2);
            let previous = std::mem::take(&mut self.bins);
            for (key, count) in previous {
                *self.bins.entry(key.div_euclid(2)).or_default() += count;
            }
        }
        *self
            .bins
            .entry(timestamp.div_euclid(self.width))
            .or_default() += 1;
    }
    fn finish(&self) -> Vec<TimeCount> {
        match self.bins.first_key_value().zip(self.bins.last_key_value()) {
            Some(((lo, _), (hi, _))) => (*lo..=*hi)
                .map(|key| TimeCount {
                    timestamp: key.saturating_mul(self.width),
                    count: *self.bins.get(&key).unwrap_or(&0),
                })
                .collect(),
            None => vec![],
        }
    }
}
fn validate_local(filters: &[Filter], catalog: &Arc<CompiledCatalog>) -> Result<(), String> {
    let ordinary: Vec<_> = filters
        .iter()
        .filter(|f| f.op != "threat_rule")
        .cloned()
        .collect();
    crate::workspace::validate(&ordinary)?;
    for filter in filters.iter().filter(|f| f.op == "threat_rule") {
        if filter.column != "_all" {
            return Err("O filtro de ameaça deve usar a coluna _all.".into());
        }
        matcher(&filter.value, Some(catalog.clone()))?;
    }
    Ok(())
}
fn split_filters(
    filters: &[Filter],
    catalog: &Arc<CompiledCatalog>,
) -> Result<(Vec<Filter>, Vec<RuleMatcher>), String> {
    let ordinary = filters
        .iter()
        .filter(|f| f.op != "threat_rule")
        .cloned()
        .collect();
    let predicates = filters
        .iter()
        .filter(|f| f.op == "threat_rule")
        .map(|f| matcher(&f.value, Some(catalog.clone())))
        .collect::<Result<_, _>>()?;
    Ok((ordinary, predicates))
}
fn visit(
    state: &AppState,
    filters: &[Filter],
    case: Option<&[Event]>,
    catalog: &Arc<CompiledCatalog>,
    mut visitor: impl FnMut(&Event),
) {
    let prepared = query::prepare_with_threat_catalog(filters, Some(catalog));
    let mut memory = |events: &[Event]| {
        for event in events {
            if operations::cancelled() {
                break;
            }
            if prepared.iter().all(|pf| query::matches(event, pf)) {
                visitor(event);
            }
        }
    };
    if let Some(events) = case {
        memory(events);
        return;
    }
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => memory(events),
        SourceData::Indexed(index) => {
            let codes = state.codes.read();
            let system = state.system_codes.read();
            let derived = state.derived.read();
            query::visit_indexed_prepared(index, &prepared, &codes, &system, &derived, |i| {
                visitor(&sources::event_at(index, i, &codes, &system, &derived))
            });
        }
        SourceData::None => {}
    }
}
fn snippet(text: &str, start: usize, end: usize) -> String {
    let mut from = start.saturating_sub(90);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let until = prefix(&text[from..], (end.saturating_sub(from) + 130).min(800)).len() + from;
    format!(
        "{}{}{}",
        if from > 0 { "…" } else { "" },
        &text[from..until],
        if until < text.len() { "…" } else { "" }
    )
}
fn scan_impl(
    state: &AppState,
    filters: Vec<Filter>,
    case: Option<Vec<Event>>,
    catalog: Arc<CompiledCatalog>,
    catalog_path: String,
    mut progress: impl FnMut(usize),
) -> Result<ScanResult, String> {
    validate_local(&filters, &catalog)?;
    let (ordinary, predicates) = split_filters(&filters, &catalog)?;
    let mut counts = vec![0usize; catalog.file.rules.len()];
    let mut examples: Vec<Vec<Example>> = (0..counts.len()).map(|_| vec![]).collect();
    let mut starts = vec![None; counts.len()];
    let mut ends = vec![None; counts.len()];
    let (mut total, mut matched, mut occurrences, mut clipped, mut undated) = (0, 0, 0, 0, 0);
    let mut categories = BTreeMap::<String, usize>::new();
    let mut sources = crate::distinct::Terms::default();
    let mut timeline = Timeline::new();
    let (mut start, mut end) = (None, None);
    visit(state, &ordinary, case.as_deref(), &catalog, |event| {
        if operations::cancelled() {
            return;
        }
        let body = corpus(event);
        clipped += usize::from(body.clipped);
        if !predicates
            .iter()
            .all(|predicate| predicate.matches_corpus(&body))
        {
            return;
        }
        total += 1;
        if total % 4096 == 0 {
            progress(total);
        }
        if catalog.enabled.is_empty() {
            return;
        }
        let ids: Vec<_> = catalog
            .set
            .matches(body.text.as_bytes())
            .iter()
            .map(|i| catalog.enabled[i])
            .collect();
        if ids.is_empty() {
            return;
        }
        matched += 1;
        occurrences += ids.len();
        let mut seen = HashSet::new();
        for i in ids {
            counts[i] += 1;
            seen.insert(catalog.file.rules[i].category.clone());
            if let Some(t) = event.timestamp {
                starts[i] = Some(starts[i].map_or(t, |v: i64| v.min(t)));
                ends[i] = Some(ends[i].map_or(t, |v: i64| v.max(t)));
            }
            if examples[i].len() < 3 {
                if let Some(found) = catalog.regexes[i].find(body.text.as_bytes()) {
                    examples[i].push(Example {
                        event_id: event.id,
                        event_ref: prefix(&event.event_ref, 512).into(),
                        timestamp: event.timestamp,
                        snippet: snippet(&body.text, found.start(), found.end()),
                        normalized: found.end() > body.original_len,
                    });
                }
            }
        }
        for category in seen {
            *categories.entry(category).or_default() += 1;
        }
        sources.insert(event.source.clone());
        if let Some(t) = event.timestamp {
            timeline.push(t);
            start = Some(start.map_or(t, |v: i64| v.min(t)));
            end = Some(end.map_or(t, |v: i64| v.max(t)));
        } else {
            undated += 1;
        }
    });
    operations::check()?;
    progress(total);
    let mut rules: Vec<_> = catalog
        .file
        .rules
        .iter()
        .enumerate()
        .filter(|(i, _)| counts[*i] > 0)
        .map(|(i, rule)| RuleResult {
            rule: rule.clone(),
            count: counts[i],
            examples: std::mem::take(&mut examples[i]),
            start: starts[i],
            end: ends[i],
        })
        .collect();
    rules.sort_by(|a, b| b.count.cmp(&a.count).then(a.rule.id.cmp(&b.rule.id)));
    Ok(ScanResult {
        total,
        matched,
        occurrences,
        complete: clipped == 0,
        clipped_records: clipped,
        enabled_rules: catalog.enabled.len(),
        catalog_path,
        categories: categories
            .into_iter()
            .map(|(name, count)| Count { name, count })
            .collect(),
        rules,
        time: timeline.finish(),
        time_bucket_ms: timeline.width,
        sources: sources
            .top(10)
            .into_iter()
            .map(|(name, count)| Count { name, count })
            .collect(),
        undated_matches: undated,
        corpus_limit: CORPUS_BYTES,
        start,
        end,
    })
}

#[derive(Serialize)]
pub struct EventResult {
    total: usize,
    rows: Vec<Event>,
    complete: bool,
    clipped_records: usize,
    rows_clipped: usize,
}
fn events_impl(
    state: &AppState,
    mut filters: Vec<Filter>,
    case: Option<Vec<Event>>,
    offset: usize,
    limit: usize,
    catalog: Arc<CompiledCatalog>,
) -> Result<EventResult, String> {
    if !filters.iter().any(|f| f.op == "threat_rule") {
        filters.push(Filter {
            column: "_all".into(),
            op: "threat_rule".into(),
            value: "*".into(),
            value2: None,
        });
    }
    validate_local(&filters, &catalog)?;
    let (ordinary, predicates) = split_filters(&filters, &catalog)?;
    let mut result = EventResult {
        total: 0,
        rows: vec![],
        complete: true,
        clipped_records: 0,
        rows_clipped: 0,
    };
    visit(state, &ordinary, case.as_deref(), &catalog, |event| {
        if operations::cancelled() {
            return;
        }
        let body = corpus(event);
        result.clipped_records += usize::from(body.clipped);
        if !predicates
            .iter()
            .all(|predicate| predicate.matches_corpus(&body))
        {
            return;
        }
        result.total += 1;
        if result.total > offset && result.rows.len() < limit.clamp(1, 200) {
            let (row, clipped) = preview(event);
            result.rows_clipped += usize::from(clipped);
            result.rows.push(row);
        }
    });
    operations::check()?;
    result.complete = result.clipped_records == 0;
    Ok(result)
}

#[tauri::command]
pub async fn threat_catalog() -> Result<CatalogInfo, String> {
    crate::offload(catalog_info).await
}
#[tauri::command]
pub async fn threat_catalog_update() -> Result<CatalogUpdate, String> {
    crate::offload(|| update_catalog_path(&path(), BUILTIN.as_bytes())).await?
}
#[tauri::command]
pub async fn threat_scan(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    app: tauri::AppHandle,
) -> Result<ScanResult, String> {
    crate::offload(move || {
        let path = path();
        let catalog = load_path(&path)?;
        scan_impl(
            app.state::<AppState>().inner(),
            filters,
            case_events,
            catalog,
            path.to_string_lossy().into_owned(),
            |count| {
                crate::emit_progress(
                    Some(&app),
                    "ameaças",
                    "Conferindo regras locais",
                    count,
                    0,
                    "registros",
                    true,
                )
            },
        )
    })
    .await?
}
#[tauri::command]
pub async fn threat_events(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    offset: Option<usize>,
    limit: Option<usize>,
    app: tauri::AppHandle,
) -> Result<EventResult, String> {
    crate::offload(move || {
        events_impl(
            app.state::<AppState>().inner(),
            filters,
            case_events,
            offset.unwrap_or(0),
            limit.unwrap_or(100),
            load_path(&path())?,
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Arc<CompiledCatalog> {
        compile(&serde_json::to_vec(&serde_json::json!({"version":1,"name":"Tests","rules":[
        {"id":"test.alpha","name":"Alpha","category":"Payload","severity":"medium","kind":"attempt","pattern":r"(?i)attack\s+alpha","description":"Test only","enabled":true,"references":[]},
        {"id":"test.path","name":"Path","category":"Payload","severity":"high","kind":"indicator","pattern":r"(?i)c:\\windows\\system32\\evil\.exe","description":"Test only","enabled":true,"references":[]},
        {"id":"test.disabled","name":"Disabled","category":"Other","severity":"low","kind":"response","pattern":"disabled match","description":"Test only","enabled":false,"references":[]}
    ]})).unwrap()).unwrap()
    }
    fn state(source: SourceData) -> AppState {
        AppState {
            source: parking_lot::RwLock::new(source),
            source_names: parking_lot::RwLock::new(vec![]),
            codes: parking_lot::RwLock::new(Default::default()),
            system_codes: parking_lot::RwLock::new(Default::default()),
            derived: parking_lot::RwLock::new(vec![]),
            case_store_lock: parking_lot::Mutex::new(()),
            codes_path: PathBuf::new(),
            system_codes_path: PathBuf::new(),
        }
    }
    fn event(id: usize, message: &str) -> Event {
        let mut event = Event::empty();
        event.id = id;
        event.event_ref = format!("fixture:{id}");
        event.timestamp = Some(id as i64 * 1000);
        event.message = message.into();
        event.source = "source".into();
        event
    }
    fn filter(id: &str) -> Filter {
        Filter {
            column: "_all".into(),
            op: "threat_rule".into(),
            value: id.into(),
            value2: None,
        }
    }
    #[test]
    fn evidence_crossing_into_decoded_text_is_marked_normalized() {
        let compiled = compile(&serde_json::to_vec(&serde_json::json!({
            "version":1,"name":"Boundary test","rules":[{
                "id":"test.boundary","name":"Boundary","category":"Test","severity":"low",
                "kind":"indicator","pattern":"alpha\\nattack alpha","description":"Test only","enabled":true,"references":[]
            }]
        })).unwrap()).unwrap();
        let event = event(1, "attack%20alpha");
        let body = corpus(&event);
        let found = compiled.regexes[0].find(body.text.as_bytes()).unwrap();
        assert!(found.start() < body.original_len && found.end() > body.original_len);
        let result = scan_impl(
            &state(SourceData::Memory(vec![event])),
            vec![],
            None,
            compiled,
            "test".into(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.matched, 1);
        assert!(result.rules[0].examples[0].normalized);
    }
    #[test]
    fn builtin_threat_catalog_compiles_and_invalid_rules_are_rejected() {
        let builtin = compile(BUILTIN.as_bytes()).unwrap();
        assert!(builtin.file.rules.len() >= 250);
        assert_eq!(
            builtin.enabled.len(),
            builtin
                .file
                .rules
                .iter()
                .filter(|rule| rule.enabled)
                .count()
        );
        let mut file: serde_json::Value = serde_json::from_str(BUILTIN).unwrap();
        file["rules"][0]["pattern"] = serde_json::json!("(?=not-supported)");
        assert!(compile(&serde_json::to_vec(&file).unwrap())
            .err()
            .unwrap()
            .contains("Regex inválida"));
        file["rules"][0]["pattern"] = serde_json::json!(".*");
        assert!(compile(&serde_json::to_vec(&file).unwrap())
            .err()
            .unwrap()
            .contains("texto vazio"));
        assert!(matcher("test.disabled", Some(catalog())).is_err());
        assert!(matcher("not.found", Some(catalog())).is_err());
    }
    #[test]
    fn builtin_threat_catalog_matches_curated_positive_and_benign_fixtures() {
        let catalog = compile(BUILTIN.as_bytes()).unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../resources/threat-examples.json")).unwrap();
        for key in ["positive", "normalizedPositive"] {
            for sample in fixture[key].as_array().unwrap() {
                let id = sample["id"].as_str().unwrap();
                let text = sample["text"].as_str().unwrap();
                assert!(
                    matcher(id, Some(catalog.clone()))
                        .unwrap()
                        .matches(&event(0, text)),
                    "{id}: {text}"
                );
            }
        }
        for text in fixture["benign"].as_array().unwrap() {
            let text = text.as_str().unwrap();
            let body = corpus(&event(0, text));
            let ids: Vec<_> = catalog
                .set
                .matches(body.text.as_bytes())
                .iter()
                .map(|i| catalog.file.rules[catalog.enabled[i]].id.clone())
                .collect();
            assert!(ids.is_empty(), "benign {text}: {ids:?}");
        }
    }
    #[test]
    fn threat_corpus_matches_original_decoded_and_parsed_fields_consistently() {
        let catalog = catalog();
        let direct = event(1, "attack alpha attack alpha");
        let encoded = event(2, "attack%2520alpha");
        let mut fields = event(3, "");
        fields.fields.insert(
            "CommandLine".into(),
            serde_json::json!(r"C:\Windows\System32\evil.exe"),
        );
        for e in [&direct, &encoded, &fields] {
            assert!(matcher("*", Some(catalog.clone())).unwrap().matches(e));
        }
        let events = vec![
            direct,
            encoded,
            fields,
            event(4, "disabled match"),
            event(5, "ordinary successful request"),
        ];
        let result = scan_impl(
            &state(SourceData::Memory(events.clone())),
            vec![],
            None,
            catalog.clone(),
            "fixture".into(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.total, 5);
        assert_eq!(result.matched, 3);
        assert_eq!(result.occurrences, 3);
        assert!(result.complete);
        assert_eq!(result.categories[0].count, 3);
        assert!(result
            .rules
            .iter()
            .find(|r| r.rule.id == "test.alpha")
            .unwrap()
            .examples
            .iter()
            .any(|e| e.normalized));
        let pfs = query::prepare_with_threat_catalog(&[filter("test.alpha")], Some(&catalog));
        assert_eq!(
            events.iter().filter(|e| query::matches(e, &pfs[0])).count(),
            2
        );
        let page = events_impl(
            &state(SourceData::None),
            vec![filter("test.alpha")],
            Some(events),
            1,
            1,
            catalog,
        )
        .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, 2);
    }
    #[test]
    fn threat_scan_matches_memory_index_case_and_reaches_file_tail() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for i in 0..10020 {
            writeln!(file,"{}",serde_json::json!({"timestamp":1700000000000i64+i*1000,"source":if i%2==0{"API"}else{"api"},"message":if i>=10000{"attack alpha"}else{"ordinary"}})).unwrap();
        }
        file.flush().unwrap();
        let index =
            sources::index_file(file.path().to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = crate::model::CodesConfig::default();
        let events: Vec<_> = (0..index.lines.len())
            .map(|i| sources::event_at(&index, i, &codes, &codes, &[]))
            .collect();
        let indexed = state(SourceData::Indexed(index));
        let memory = state(SourceData::Memory(events.clone()));
        let catalog = catalog();
        let filters = vec![Filter {
            column: "source".into(),
            op: "equals_exact".into(),
            value: "API".into(),
            value2: None,
        }];
        let scan = |state: &AppState, case| {
            serde_json::to_value(
                scan_impl(
                    state,
                    filters.clone(),
                    case,
                    catalog.clone(),
                    "fixture".into(),
                    |_| {},
                )
                .unwrap(),
            )
            .unwrap()
        };
        let expected = scan(&memory, None);
        assert_eq!(expected["total"], 5010);
        assert_eq!(expected["matched"], 10);
        assert_eq!(expected["rules"][0]["count"], 10);
        assert_eq!(scan(&indexed, None), expected);
        assert_eq!(
            scan(&state(SourceData::None), Some(events.clone())),
            expected
        );
        let pfs = query::prepare_with_threat_catalog(&[filter("test.alpha")], Some(&catalog));
        let mut matches = vec![];
        if let SourceData::Indexed(index) = &*indexed.source.read() {
            query::visit_indexed_prepared(index, &pfs, &codes, &codes, &[], |i| matches.push(i));
        }
        assert_eq!(matches, (10000..10020).collect::<Vec<_>>());
    }
    #[test]
    fn threat_limits_remain_explicit_even_when_clipped_record_does_not_match() {
        let mut clipped = event(1, &"界".repeat(CORPUS_BYTES));
        clipped.message.push_str("attack alpha");
        let mut preview = event(2, "attack alpha");
        preview.fields.insert(
            "padding".into(),
            serde_json::json!("z".repeat(CORPUS_BYTES * 2)),
        );
        let events = vec![clipped, preview];
        let state = state(SourceData::Memory(events));
        let catalog = catalog();
        let scan = scan_impl(
            &state,
            vec![],
            None,
            catalog.clone(),
            "fixture".into(),
            |_| {},
        )
        .unwrap();
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.clipped_records, 2);
        assert!(!scan.complete);
        let page = events_impl(&state, vec![], None, 0, 100, catalog).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.clipped_records, 2);
        assert_eq!(page.rows_clipped, 1);
        assert_eq!(page.rows[0].id, 2);
        assert!(!page.complete);
        assert!(serde_json::to_vec(&page.rows[0]).unwrap().len() <= PREVIEW_BYTES);
        let mut timeline = Timeline::new();
        for t in [-2_000_000_000_000i64, 0, 1_000_000, 2_000_000_000_000] {
            timeline.push(t);
        }
        assert!(timeline.finish().len() <= 120);
        assert_eq!(
            timeline.finish().iter().map(|bin| bin.count).sum::<usize>(),
            4
        );
    }
    #[test]
    fn threat_catalog_hash_reload_preserves_invalid_user_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rules.json");
        fs::write(
            &path,
            serde_json::json!({"version":1,"name":"One","rules":[]}).to_string(),
        )
        .unwrap();
        let first = load_path(&path).unwrap();
        assert!(first.enabled.is_empty());
        fs::write(&path, BUILTIN).unwrap();
        assert!(!load_path(&path).unwrap().enabled.is_empty());
        fs::write(&path, "invalid user file").unwrap();
        assert!(load_path(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid user file");
    }
    #[test]
    fn catalog_update_adds_only_new_ids_and_keeps_backup_and_customizations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rules.json");
        let existing = serde_json::json!({"version":1,"name":"Meu catálogo","rules":[{
            "id":"shared","name":"Meu nome","category":"Minha categoria","severity":"low","kind":"indicator",
            "pattern":"custom-value","description":"Minha descrição","enabled":false,"references":[]
        }]});
        let mut bundled = existing.clone();
        bundled["name"] = "Padrão".into();
        bundled["rules"][0]["pattern"] = "default-value".into();
        bundled["rules"][0]["enabled"] = true.into();
        let mut new_rule = bundled["rules"][0].clone();
        new_rule["id"] = "new-rule".into();
        bundled["rules"].as_array_mut().unwrap().push(new_rule);
        let original = serde_json::to_vec(&existing).unwrap();
        fs::write(&path, &original).unwrap();
        let bundled = serde_json::to_vec(&bundled).unwrap();
        let result = update_catalog_path(&path, &bundled).unwrap();
        assert_eq!(result.added, 1);
        assert_eq!(result.catalog.updates_available, 0);
        assert_eq!(result.catalog.name, "Meu catálogo");
        assert_eq!(fs::read(result.backup_path.unwrap()).unwrap(), original);
        let updated: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(updated["rules"][0], existing["rules"][0]);
        let unchanged = fs::read(&path).unwrap();
        assert_eq!(update_catalog_path(&path, &bundled).unwrap().added, 0);
        assert_eq!(fs::read(&path).unwrap(), unchanged);
        fs::write(&path, "invalid original").unwrap();
        assert!(update_catalog_path(&path, &bundled).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid original");
    }
    #[test]
    fn threat_preview_budget_counts_escaped_json_bytes_and_exact_node_limit() {
        let mut event = event(7, &"\0".repeat(CORPUS_BYTES));
        event.raw = "\u{1}".repeat(CORPUS_BYTES);
        event.description = "\u{2}".repeat(CORPUS_BYTES);
        let (row, clipped) = preview(&event);
        assert!(clipped);
        assert_eq!(row.id, 7);
        assert!(serde_json::to_vec(&row).unwrap().len() <= PREVIEW_BYTES);
        let mut event = event;
        event.message.clear();
        event.raw.clear();
        event.description.clear();
        event.fields.insert(
            "values".into(),
            serde_json::json!((0..999).map(|_| 1).collect::<Vec<_>>()),
        );
        assert!(!corpus(&event).clipped);
        event
            .fields
            .get_mut("values")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(1.into());
        assert!(corpus(&event).clipped);
    }
}
