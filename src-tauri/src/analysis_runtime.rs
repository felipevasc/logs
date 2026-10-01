//! Immutable, Case-owned query admission. Capture happens before blocking work
//! is queued. Workers never resolve an active Case or a mutable derived registry.
use crate::{
    analysis_context::{self, Identity, Snapshot},
    model::Event,
    sources, AppState, SourceData,
};
use parking_lot::RwLockReadGuard;
use std::{
    cell::RefCell,
    ops::Deref,
    sync::{Arc, OnceLock},
};

/// Common desktop/MCP wire context. The schema mirror avoids coupling the
/// durable storage model to a particular protocol's schema implementation.
#[derive(Clone, Debug, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Params {
    #[schemars(with = "Option<IdentitySchema>")]
    #[serde(alias = "analysis_context")]
    pub analysis_context: Option<Identity>,
    #[serde(alias = "source_generation")]
    pub source_generation: Option<u64>,
}
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct IdentitySchema {
    case_id: String,
    analysis_id: String,
    config_revision: u64,
    visibility_revision: u64,
}

pub(crate) const STALE: &str = "O Caso, a configuração ou a fonte mudou; atualize a consulta.";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Dataset,
    Case,
    Publish,
}

pub(crate) struct Admitted {
    pub identity: Option<Identity>,
    pub source_generation: Option<u64>,
    pub case_key: Option<String>,
    mode: Mode,
    source: Option<Arc<SourceData>>,
    names: Vec<String>,
    derived: Arc<Vec<sources::CompiledDerived>>,
    prepared_fields: OnceLock<Arc<Vec<sources::CompiledDerived>>>,
    data_root: std::path::PathBuf,
    definition_bytes: usize,
    pub diagnostics: Arc<Vec<analysis_context::Diagnostic>>,
    pub references: Arc<Vec<analysis_context::ReferenceDescriptor>>,
    visibility: OnceLock<PreparedVisibility>,
    failure: parking_lot::Mutex<Option<String>>,
    source_set: OnceLock<Result<Arc<crate::analysis_visibility::SourceSet>, String>>,
}

/// A fixed admitted row domain. Query workers share this object; checks never
/// enter SQLite and retain the underlying immutable source/payload leases.
pub(crate) struct RowGate {
    rows: usize,
    visible: usize,
    unavailable_members: Option<u64>,
    keep: Arc<dyn Fn(usize) -> bool + Send + Sync>,
    verify: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}
impl RowGate {
    fn new(
        rows: usize,
        visible: usize,
        unavailable_members: Option<u64>,
        keep: impl Fn(usize) -> bool + Send + Sync + 'static,
        verify: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Result<Self, String> {
        if visible > rows {
            return Err("Contagem de visibilidade incompatível com a fonte.".into());
        }
        Ok(Self {
            rows,
            visible,
            unavailable_members,
            keep: Arc::new(keep),
            verify: Arc::new(verify),
        })
    }
    pub(crate) fn allows(&self, row: usize) -> Result<bool, String> {
        if row >= self.rows {
            return Err("Registro fora da geração de visibilidade admitida.".into());
        }
        Ok((self.keep)(row))
    }
    pub(crate) fn allows_known_row(&self, row: usize) -> bool {
        row < self.rows && (self.keep)(row)
    }
    pub(crate) fn visible_count(&self) -> usize {
        self.visible
    }
    pub(crate) fn is_unrestricted(&self) -> bool {
        self.visible == self.rows
    }
    fn validate(&self) -> Result<(), String> {
        (self.verify)()
    }
}
struct PreparedVisibility {
    gate: Option<Arc<RowGate>>,
    source: Option<Arc<SourceData>>,
}

thread_local! { static CURRENT: RefCell<Option<Arc<Admitted>>> = const { RefCell::new(None) }; }

pub(crate) fn current() -> Option<Arc<Admitted>> {
    CURRENT.with(|value| value.borrow().clone())
}
pub(crate) fn with<T>(admitted: Option<Arc<Admitted>>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<Admitted>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|value| *value.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|value| value.replace(admitted)));
    f()
}

pub(crate) fn validate_identity(identity: &Identity) -> Result<Snapshot, String> {
    let snapshot = analysis_context::snapshot(&identity.case_id)?;
    if snapshot.identity() != *identity {
        return Err(STALE.into());
    }
    Ok(snapshot)
}

const REGEX_BYTES: usize = analysis_context::REGEX_SET_BYTES;
const DFA_BYTES: usize = analysis_context::DFA_SET_BYTES;
const COMPILED_CACHE_BYTES: usize = 64 << 20;
struct CompiledEntry {
    identity: Identity,
    fields: Arc<Vec<sources::CompiledDerived>>,
    diagnostics: Arc<Vec<analysis_context::Diagnostic>>,
    budget: usize,
}
static COMPILED: parking_lot::Mutex<Vec<CompiledEntry>> = parking_lot::Mutex::new(Vec::new());

/// Identity validation is deliberately outside this content cache: a hit never
/// authorizes a deleted Case or a superseded config/visibility revision.
fn compile(
    snapshot: &Snapshot,
) -> Result<
    (
        Arc<Vec<sources::CompiledDerived>>,
        Arc<Vec<analysis_context::Diagnostic>>,
    ),
    String,
> {
    let identity = snapshot.identity();
    let mut cache = COMPILED.lock();
    if let Some(index) = cache.iter().position(|entry| entry.identity == identity) {
        let entry = cache.remove(index);
        let result = (Arc::clone(&entry.fields), Arc::clone(&entry.diagnostics));
        cache.push(entry);
        return Ok(result);
    }
    // Compile while holding this small cache lock to coalesce concurrent
    // count/stats/facet admissions for the same revision.
    let mut diagnostics = snapshot.migration_diagnostics.clone();
    let fields = Arc::new(match compile_uncached(snapshot) {
        Ok(fields) => fields,
        Err(message) => {
            diagnostics.push(analysis_context::Diagnostic {
                definition_index: None,
                code: "disabled_runtime_definitions".into(),
                message,
            });
            Vec::new()
        }
    });
    let diagnostics = Arc::new(diagnostics);
    let budget = REGEX_BYTES
        + DFA_BYTES
        + serde_json::to_vec(&snapshot.config)
            .map_err(|e| e.to_string())?
            .len();
    while !cache.is_empty()
        && (cache.len() >= 4
            || cache
                .iter()
                .map(|entry| entry.budget)
                .sum::<usize>()
                .saturating_add(budget)
                > COMPILED_CACHE_BYTES)
    {
        cache.remove(0);
    }
    cache.push(CompiledEntry {
        identity,
        fields: Arc::clone(&fields),
        diagnostics: Arc::clone(&diagnostics),
        budget,
    });
    Ok((fields, diagnostics))
}

/// Expose disabled legacy/over-budget definitions beside their retained source.
/// This augments only the response; it never edits durable configuration.
pub(crate) fn with_diagnostics(mut snapshot: Snapshot) -> Result<Snapshot, String> {
    let (_, diagnostics) = compile(&snapshot)?;
    snapshot.migration_diagnostics = (*diagnostics).clone();
    Ok(snapshot)
}

/// Mutation callers can reject an excessive compilation before committing CAS.
pub(crate) fn validate_compilation(config: &analysis_context::Config) -> Result<(), String> {
    let snapshot = Snapshot {
        schema_version: 1,
        case_id: String::new(),
        analysis_id: String::new(),
        config_revision: 0,
        visibility_revision: 0,
        config: config.clone(),
        migration_diagnostics: Vec::new(),
        legacy_raw: None,
    };
    compile_uncached(&snapshot).map(|_| ())
}

/// Explicit save preflight, called in the command's cancellable worker before
/// configuration CAS. Holding the returned fields pins verified projection bytes
/// and their file-only leases through publication, without changing any config.
pub(crate) fn prepare_candidate_config(
    root: &std::path::Path,
    owner: &crate::reference_store::Owner,
    config: &analysis_context::Config,
    cancelled: &dyn Fn() -> bool,
) -> Result<Arc<Vec<sources::CompiledDerived>>, String> {
    prepare_candidate_config_detailed(root, owner, config, cancelled)
        .map_err(|error| error.to_string())
}

/// Portable-import callers can preserve unavailable/over-budget definitions
/// with explicit diagnostics, without interpreting localized error messages.
pub(crate) fn prepare_candidate_config_detailed(
    root: &std::path::Path,
    owner: &crate::reference_store::Owner,
    config: &analysis_context::Config,
    cancelled: &dyn Fn() -> bool,
) -> Result<Arc<Vec<sources::CompiledDerived>>, crate::reference_lookup::PreparationError> {
    let snapshot = Snapshot {
        schema_version: 1,
        case_id: owner.case_id.clone(),
        analysis_id: owner.analysis_id.clone(),
        config_revision: 0,
        visibility_revision: 0,
        config: config.clone(),
        migration_diagnostics: Vec::new(),
        legacy_raw: None,
    };
    prepare_portable_snapshot(root, &snapshot, cancelled)
}

/// Import preflight preserves explicitly disabled original definitions while
/// strictly validating and preparing every remaining active dependency.
pub(crate) fn prepare_portable_snapshot(
    root: &std::path::Path,
    snapshot: &Snapshot,
    cancelled: &dyn Fn() -> bool,
) -> Result<Arc<Vec<sources::CompiledDerived>>, crate::reference_lookup::PreparationError> {
    use crate::reference_lookup::PreparationError;
    let check = || {
        if cancelled() {
            Err(PreparationError::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    let bytes = serde_json::to_vec(&snapshot.config)
        .map_err(|error| PreparationError::Invalid(error.to_string()))?;
    if snapshot.schema_version != 1
        || bytes.len() > 4 << 20
        || snapshot.config.derived_fields.len() > 256
        || snapshot.config.references.len() > 128
    {
        return Err(PreparationError::Invalid(
            "A configuração importada excede os limites do Caso ou usa uma versão incompatível."
                .into(),
        ));
    }
    let active = analysis_context::Config {
        derived_fields: enabled_definitions(snapshot),
        references: snapshot.config.references.clone(),
    };
    analysis_context::validate(&active).map_err(PreparationError::Invalid)?;
    let fields = compile_uncached(snapshot).map_err(PreparationError::Invalid)?;
    check()?;
    let owner = crate::reference_store::Owner {
        case_id: snapshot.case_id.clone(),
        analysis_id: snapshot.analysis_id.clone(),
    };
    let prepared = crate::reference_lookup::prepare_fields_detailed(
        root,
        &owner,
        &snapshot.config.references,
        &fields,
        cancelled,
    )?;
    check()?;
    Ok(Arc::new(prepared))
}

fn enabled_definitions(snapshot: &Snapshot) -> Vec<serde_json::Value> {
    // A malformed legacy definition is disabled, while original logs remain
    // queryable. A diagnosed global dependency error makes no ordering safe.
    if snapshot.migration_diagnostics.iter().any(|issue| {
        issue.definition_index.is_none() && issue.code == "invalid_legacy_dependencies"
    }) {
        return Vec::new();
    }
    let definitions = &snapshot.config.derived_fields;
    let mut disabled: std::collections::HashSet<usize> = snapshot
        .migration_diagnostics
        .iter()
        .filter_map(|issue| issue.definition_index)
        .collect();
    let reserved = |name: &str| {
        name.starts_with('@')
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
    };
    loop {
        let names: Vec<&str> = disabled
            .iter()
            .filter_map(|&index| definitions.get(index)?.get("name")?.as_str())
            .filter(|name| !reserved(name))
            .collect();
        let mut more = Vec::new();
        for (index, definition) in definitions
            .iter()
            .enumerate()
            .filter(|(index, _)| !disabled.contains(index))
        {
            let references =
                std::iter::once(definition.get("source").and_then(serde_json::Value::as_str))
                    .chain(
                        definition
                            .get("rules")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .chain(std::iter::once(definition))
                            .map(|rule| {
                                rule.get("filter")
                                    .and_then(|filter| filter.get("column"))
                                    .and_then(serde_json::Value::as_str)
                            }),
                    )
                    .chain(
                        definition
                            .get("lookup")
                            .and_then(|lookup| lookup.get("keys"))
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|key| key.get("sourceField").and_then(serde_json::Value::as_str)),
                    );
            if references.flatten().any(|field| {
                names.iter().any(|name| {
                    field == *name
                        || field
                            .strip_prefix(*name)
                            .is_some_and(|suffix| suffix.starts_with('.'))
                })
            }) {
                more.push(index);
            }
        }
        if more.is_empty() {
            break;
        }
        disabled.extend(more);
    }
    definitions
        .iter()
        .enumerate()
        .filter(|(index, _)| !disabled.contains(index))
        .map(|(_, value)| value.clone())
        .collect()
}

fn compile_uncached(snapshot: &Snapshot) -> Result<Vec<sources::CompiledDerived>, String> {
    let safe = enabled_definitions(snapshot);
    let rule_count = safe
        .iter()
        .map(|value| {
            value
                .get("rules")
                .and_then(serde_json::Value::as_array)
                .filter(|rules| !rules.is_empty())
                .map(Vec::len)
                .unwrap_or_else(|| {
                    usize::from(
                        value
                            .get("pattern")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|pattern| !pattern.is_empty()),
                    )
                })
        })
        .sum::<usize>();
    let limits = analysis_context::regex_limits(rule_count)?;
    let regex_limit = limits.size_limit;
    let dfa_limit = limits.dfa_size_limit;
    analysis_context::definition_order(&safe)?.into_iter().map(|index| {
        let definition: sources::DerivedFieldCompat = serde_json::from_value(safe[index].clone())
            .map_err(|_| "Campo derivado inválido no Caso.".to_string())?;
        let definition = definition.normalize();
        let rules = definition.rules.into_iter().map(|rule| {
            regex::RegexBuilder::new(&rule.pattern).size_limit(regex_limit).dfa_size_limit(dfa_limit).build()
                .map(|re| sources::CompiledRule { re, template: rule.template, filter: rule.filter })
                .map_err(|_| "Expressão regular inválida ou excede o orçamento total de compilação do Caso.".to_string())
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(sources::CompiledDerived { name: definition.name, source: definition.source, rules, steps: definition.steps,
            lookup: definition.lookup.map(crate::reference_lookup::Compiled::new) })
    }).collect()
}

pub(crate) fn same_owner(a: Option<&Identity>, b: Option<&Identity>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.case_id == b.case_id && a.analysis_id == b.analysis_id,
        _ => false,
    }
}

pub(crate) fn capture(
    state: &AppState,
    identity: Option<Identity>,
    expected_generation: Option<u64>,
    mode: Mode,
) -> Result<Arc<Admitted>, String> {
    let (derived, diagnostics, references, definition_bytes) = match &identity {
        Some(identity) => {
            let snapshot = validate_identity(identity)?;
            let (derived, diagnostics) = compile(&snapshot)?;
            let definition_bytes = serde_json::to_vec(&snapshot.config)
                .map_err(|e| e.to_string())?
                .len();
            (
                derived,
                diagnostics,
                Arc::new(snapshot.config.references),
                definition_bytes,
            )
        }
        None => {
            if analysis_context::active_snapshot()?.is_some() {
                return Err(
                    "Informe a identidade do Caso ativo para executar esta operação.".into(),
                );
            }
            (
                Arc::new(state.derived.read().clone()),
                Arc::new(Vec::new()),
                Arc::new(Vec::new()),
                0,
            )
        }
    };
    let mut names = Vec::new();
    let (generation, captured) = if mode == Mode::Case {
        (None, None)
    } else {
        let source = state.source.read();
        let receipt = crate::source_publication::receipt_locked(state);
        if expected_generation.is_some_and(|expected| expected != receipt.generation) {
            return Err(STALE.into());
        }
        if mode == Mode::Dataset
            && !same_owner(identity.as_ref(), receipt.analysis_context.as_ref())
        {
            return Err(STALE.into());
        }
        names = state.source_names.read().clone();
        let captured = if mode == Mode::Dataset {
            Some(Arc::new(match &*source {
                SourceData::None => SourceData::None,
                SourceData::Memory(events) => SourceData::Memory(events.clone()),
                SourceData::Indexed(index) => {
                    for part in &index.parts {
                        sources::validate_source(part)?;
                    }
                    SourceData::Indexed(sources::FileIndex {
                        parts: index.parts.clone(),
                        lines: Arc::clone(&index.lines),
                        columns: index.columns.clone(),
                        time_order: Arc::clone(&index.time_order),
                    })
                }
            }))
        } else {
            None
        };
        (Some(receipt.generation), captured)
    };
    let source = captured;
    Ok(Arc::new(Admitted {
        identity,
        source_generation: generation,
        case_key: None,
        mode,
        source,
        names,
        derived,
        prepared_fields: OnceLock::new(),
        data_root: crate::config_dir(),
        definition_bytes,
        diagnostics,
        references,
        visibility: OnceLock::new(),
        failure: parking_lot::Mutex::new(None),
        source_set: OnceLock::new(),
    }))
}

pub(crate) fn capture_case(
    state: &AppState,
    identity: Option<Identity>,
    generation: Option<u64>,
    events: Option<Vec<Event>>,
    key: Option<String>,
) -> Result<(Arc<Admitted>, Option<Vec<Event>>), String> {
    let case = events.is_some() || key.is_some();
    if case && identity.is_none() {
        return Err("Informe a identidade do Caso antes de consultar suas evidências.".into());
    }
    let mut admitted = capture(
        state,
        identity,
        generation,
        if case { Mode::Case } else { Mode::Dataset },
    )?;
    Arc::get_mut(&mut admitted).expect("new admission").case_key = key.clone();
    let events = crate::case_cache::take_for(events, key, admitted.identity.as_ref())?;
    Ok((admitted, events))
}

// Cached projections contain copied, verified bytes and file-only leases. No
// SQLite reader crosses threads. Cache hits preserve the exact snapshot
// even if its immutable disk artifact later becomes unavailable; a changed
// descriptor/config/owner cannot reuse it, and a miss verifies storage again.
const PREPARED_CACHE_BYTES: usize = 64 << 20;
const PREPARED_CACHE_ENTRIES: usize = 2;
struct PreparedFieldsEntry {
    root: std::path::PathBuf,
    identity: Identity,
    references: Arc<Vec<analysis_context::ReferenceDescriptor>>,
    fields: Arc<Vec<sources::CompiledDerived>>,
    budget: usize,
}
static PREPARED_FIELDS: parking_lot::Mutex<Vec<PreparedFieldsEntry>> =
    parking_lot::Mutex::new(Vec::new());
static PREPARING_FIELDS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

impl Admitted {
    fn cached_prepared_fields(
        &self,
        identity: &Identity,
    ) -> Option<Arc<Vec<sources::CompiledDerived>>> {
        let mut cache = PREPARED_FIELDS.lock();
        let index = cache.iter().position(|entry| {
            entry.root == self.data_root
                && &entry.identity == identity
                && entry.references == self.references
        })?;
        let entry = cache.remove(index);
        let fields = Arc::clone(&entry.fields);
        cache.push(entry);
        Some(fields)
    }

    /// File verification and projection loading run only inside the admitted
    /// worker, before overlays or a derived engine variant can consume fields.
    pub(crate) fn prepare_references(&self) -> Result<Arc<Vec<sources::CompiledDerived>>, String> {
        crate::operations::check()?;
        if let Some(fields) = self.prepared_fields.get() {
            return Ok(Arc::clone(fields));
        }
        let fields = if !self.derived.iter().any(|field| field.lookup.is_some()) {
            Arc::clone(&self.derived)
        } else {
            let identity = self
                .identity
                .as_ref()
                .ok_or("Consultas à referência exigem um Caso admitido.")?;
            if let Some(fields) = self.cached_prepared_fields(identity) {
                fields
            } else {
                let token = crate::operations::current_token();
                let cancelled = || token.cancelled();
                let _preparing = loop {
                    if cancelled() {
                        return Err("Preparação das referências cancelada.".into());
                    }
                    if let Some(guard) =
                        PREPARING_FIELDS.try_lock_for(std::time::Duration::from_millis(25))
                    {
                        break guard;
                    }
                };
                if let Some(fields) = self.cached_prepared_fields(identity) {
                    fields
                } else {
                    crate::operations::progress(
                        "reference-runtime",
                        "Preparando referências verificadas do Caso",
                        0,
                        0,
                        0,
                    );
                    let owner = crate::reference_store::Owner {
                        case_id: identity.case_id.clone(),
                        analysis_id: identity.analysis_id.clone(),
                    };
                    let fields = Arc::new(crate::reference_lookup::prepare_fields(
                        &self.data_root,
                        &owner,
                        &self.references,
                        &self.derived,
                        &cancelled,
                    )?);
                    if cancelled() {
                        return Err("Preparação das referências cancelada.".into());
                    }
                    for lookup in fields.iter().filter_map(|field| field.lookup.as_ref()) {
                        if lookup
                            .version()
                            .is_none_or(|version| version.owner != owner)
                        {
                            return Err(
                                "A referência preparada não pertence à captura do Caso.".into()
                            );
                        }
                    }
                    let projection_bytes = crate::reference_lookup::retained_bytes(&fields);
                    if projection_bytes > crate::reference_lookup::MAX_RETAINED_BYTES {
                        return Err("As referências preparadas excedem o orçamento do Caso.".into());
                    }
                    // Conservatively account for retained regex automata and
                    // serialized metadata as well as the exact projection bytes.
                    let budget = (REGEX_BYTES + DFA_BYTES)
                        .saturating_add(self.definition_bytes)
                        .saturating_add(projection_bytes);
                    if budget <= PREPARED_CACHE_BYTES {
                        let mut cache = PREPARED_FIELDS.lock();
                        while !cache.is_empty()
                            && (cache.len() >= PREPARED_CACHE_ENTRIES
                                || cache
                                    .iter()
                                    .map(|entry| entry.budget)
                                    .sum::<usize>()
                                    .saturating_add(budget)
                                    > PREPARED_CACHE_BYTES)
                        {
                            cache.remove(0);
                        }
                        cache.push(PreparedFieldsEntry {
                            root: self.data_root.clone(),
                            identity: identity.clone(),
                            references: Arc::clone(&self.references),
                            fields: Arc::clone(&fields),
                            budget,
                        });
                    }
                    fields
                }
            }
        };
        crate::operations::check()?;
        let _ = self.prepared_fields.set(Arc::clone(&fields));
        Ok(Arc::clone(
            self.prepared_fields
                .get()
                .expect("prepared fields installed"),
        ))
    }
}

static MASKS: std::sync::LazyLock<crate::analysis_visibility::Cache> =
    std::sync::LazyLock::new(crate::analysis_visibility::Cache::new);
static EVIDENCE_MASKS: std::sync::LazyLock<crate::analysis_visibility::EvidenceCache> =
    std::sync::LazyLock::new(crate::analysis_visibility::EvidenceCache::new);

fn indexed_row_gate(mask: Arc<crate::analysis_visibility::Mask>) -> Result<Arc<RowGate>, String> {
    let rows = mask.rows();
    let visible = rows
        .checked_sub(mask.cardinality())
        .ok_or("Máscara excede a fonte admitida.")?;
    let lease = Arc::clone(&mask);
    Ok(Arc::new(RowGate::new(
        rows,
        visible,
        Some(mask.ignored_members()),
        move |row| !mask.contains_row(row),
        move || lease.validate(),
    )?))
}
fn evidence_row_gate(
    mask: Arc<crate::analysis_visibility::EvidenceMask>,
) -> Result<Arc<RowGate>, String> {
    let rows = mask.rows();
    let visible = rows
        .checked_sub(mask.cardinality())
        .ok_or("Máscara excede as evidências admitidas.")?;
    let lease = Arc::clone(&mask);
    Ok(Arc::new(RowGate::new(
        rows,
        visible,
        None,
        move |row| !mask.contains_row(row),
        move || lease.validate(),
    )?))
}

impl Admitted {
    /// Called only inside a registered blocking operation. The lazy caches
    /// serialize preparation, while hits validate retained immutable leases.
    pub(crate) fn prepare_visibility(
        &self,
        case_events: Option<Vec<Event>>,
    ) -> Result<Option<Vec<Event>>, String> {
        if self.visibility.get().is_some() {
            return Err("Esta captura de consulta já foi executada.".into());
        }
        crate::operations::check()?;
        let derived = self.prepare_references()?;
        let token = crate::operations::current_token();
        let cancelled = || token.cancelled();
        let progress = |phase: &str, completed: u64, total: Option<u64>| {
            crate::operations::report_progress(
                "análise",
                "visibility",
                "Preparando visibilidade da análise",
                completed.min(usize::MAX as u64) as usize,
                total.unwrap_or(0).min(usize::MAX as u64) as usize,
                "registros",
                0,
            );
            let _ = phase;
        };
        let work = crate::exclusion_store::Work {
            cancelled: &cancelled,
            progress: &progress,
        };
        let budget = crate::analysis_visibility::MaskBudget::default();
        let scope = crate::exclusion_store::Scope::ActiveUnion;
        let load = || {
            crate::exclusion_store::visibility(
                &crate::config_dir(),
                self.identity
                    .as_ref()
                    .ok_or("Caso ausente na visibilidade.")?,
                &crate::exclusion_store::Budget::default(),
                &work,
            )
        };
        let mut gate = None;
        let mut visible_source = self.source.clone();
        let mut evidence = case_events;
        if self.mode != Mode::Publish {
            match (&self.mode, self.source.as_deref()) {
                (Mode::Case, _) => {
                    let events = evidence
                        .as_mut()
                        .ok_or("Evidências do Caso ausentes na captura.")?;
                    if let Some(identity) = &self.identity {
                        gate = Some(evidence_row_gate(
                            EVIDENCE_MASKS
                                .get_or_prepare(events, identity, &scope, &budget, &work, load)?,
                        )?);
                    }
                    let mut row = 0;
                    let mut stopped = false;
                    events.retain_mut(|event| {
                        if row % 256 == 0 && cancelled() {
                            stopped = true;
                        }
                        let keep =
                            !stopped && gate.as_ref().is_none_or(|gate| gate.allows_known_row(row));
                        row += 1;
                        if keep {
                            sources::apply_derived(event, &derived);
                        }
                        keep
                    });
                }
                (_, Some(SourceData::Indexed(index))) => {
                    if let Some(identity) = &self.identity {
                        gate = Some(indexed_row_gate(MASKS.get_or_prepare(
                            index,
                            self.source_generation.ok_or("Geração de fonte ausente.")?,
                            identity,
                            &scope,
                            &budget,
                            &work,
                            load,
                        )?)?);
                    }
                }
                (_, Some(SourceData::Memory(events))) => {
                    let mut events = events.clone();
                    if let Some(identity) = &self.identity {
                        gate = Some(evidence_row_gate(
                            EVIDENCE_MASKS
                                .get_or_prepare(&events, identity, &scope, &budget, &work, load)?,
                        )?);
                    }
                    let mut row = 0;
                    let mut stopped = false;
                    events.retain_mut(|event| {
                        if row % 256 == 0 && cancelled() {
                            stopped = true;
                        }
                        let keep =
                            !stopped && gate.as_ref().is_none_or(|gate| gate.allows_known_row(row));
                        row += 1;
                        if keep {
                            sources::apply_derived(event, &derived);
                        }
                        keep
                    });
                    visible_source = Some(Arc::new(SourceData::Memory(events)));
                }
                _ => {
                    if let Some(identity) = &self.identity {
                        gate = Some(evidence_row_gate(EVIDENCE_MASKS.get_or_prepare(
                            &[],
                            identity,
                            &scope,
                            &budget,
                            &work,
                            load,
                        )?)?);
                    }
                }
            }
        }
        crate::operations::check()?;
        if let Some(gate) = &gate {
            gate.validate()?;
        }
        self.visibility
            .set(PreparedVisibility {
                gate,
                source: visible_source,
            })
            .map_err(|_| "Captura de visibilidade já publicada.")?;
        Ok(evidence)
    }
    /// Only optional scheduling consults publication again, after page work.
    /// The query itself continues to use its immutable admitted snapshot.
    pub(crate) fn schedule_derived_variant(&self, state: &AppState) {
        if self.mode != Mode::Dataset || self.derived.is_empty() {
            return;
        }
        let Some(SourceData::Indexed(index)) = self.source.as_deref() else {
            return;
        };
        let _source = state.source.read();
        if self.validate_publication(state, false).is_err() {
            return;
        }
        if self
            .identity
            .as_ref()
            .is_some_and(|identity| validate_identity(identity).is_err())
        {
            return;
        }
        let Some(derived) = self.prepared_fields.get() else {
            return;
        };
        crate::engine::ensure_admitted_variant(
            index,
            &state.codes.read(),
            &state.system_codes.read(),
            derived,
        );
    }

    pub(crate) fn validate_visibility(&self) -> Result<(), String> {
        if let Some(error) = self.failure.lock().clone() {
            return Err(error);
        }
        let prepared = self
            .visibility
            .get()
            .ok_or("Visibilidade não preparada para esta consulta.")?;
        if let Some(gate) = &prepared.gate {
            gate.validate()?;
        }
        crate::operations::check()
    }

    /// Counts belong to this exact admitted scope, before user filters. Only
    /// indexed Dataset masks can classify unmatched archived source members;
    /// absence from a Case evidence subset says nothing about source availability.
    pub(crate) fn visibility_summary(&self) -> Result<VisibilitySummary, String> {
        self.validate_visibility()?;
        let analysis = self
            .identity
            .clone()
            .ok_or("Caso ausente na visibilidade.")?;
        let prepared = self.visibility.get().ok_or("Visibilidade não preparada.")?;
        let scope = match self.mode {
            Mode::Dataset => VisibilityScope::Dataset,
            Mode::Case => VisibilityScope::Case,
            Mode::Publish => {
                return Err("Publicação de fonte não é uma consulta de visibilidade.".into())
            }
        };
        let source_available = self.mode == Mode::Case
            || matches!(
                self.source.as_deref(),
                Some(SourceData::Indexed(_) | SourceData::Memory(_))
            );
        let (total_rows, excluded_rows, unavailable_members) = if source_available {
            let gate = prepared
                .gate
                .as_ref()
                .ok_or("Máscara da análise ausente.")?;
            (
                Some(gate.rows),
                Some(gate.rows - gate.visible),
                gate.unavailable_members,
            )
        } else {
            (None, None, None)
        };
        Ok(VisibilitySummary {
            analysis,
            source_generation: self.source_generation,
            scope,
            source_available,
            total_rows,
            excluded_rows,
            unavailable_members,
        })
    }

    /// Validate once before execution. The immutable snapshot remains valid if
    /// another request publishes a source/config while this query is running.
    pub(crate) fn validate(&self, state: &AppState) -> Result<(), String> {
        if let Some(identity) = &self.identity {
            validate_identity(identity)?;
        } else if analysis_context::active_snapshot()?.is_some() {
            return Err(STALE.into());
        }
        if self.mode != Mode::Case {
            let _source = state.source.read();
            self.validate_publication(state, false)?;
        }
        if let Some(source) = &self.source {
            if let SourceData::Indexed(index) = source.as_ref() {
                for part in &index.parts {
                    sources::validate_source(part)?;
                }
            }
        }
        Ok(())
    }
    /// Caller already owns the source read/write guard: never reacquire it.
    pub(crate) fn validate_publication(&self, state: &AppState, merge: bool) -> Result<(), String> {
        let receipt = crate::source_publication::receipt_locked(state);
        if self
            .source_generation
            .is_some_and(|generation| generation != receipt.generation)
        {
            return Err(STALE.into());
        }
        if (self.mode == Mode::Dataset || merge)
            && !same_owner(self.identity.as_ref(), receipt.analysis_context.as_ref())
        {
            return Err(STALE.into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum VisibilityScope {
    Dataset,
    Case,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VisibilitySummary {
    pub analysis: Identity,
    pub source_generation: Option<u64>,
    pub scope: VisibilityScope,
    pub source_available: bool,
    pub total_rows: Option<usize>,
    pub excluded_rows: Option<usize>,
    pub unavailable_members: Option<u64>,
}

#[tauri::command]
pub(crate) async fn exclusion_visibility(
    app: tauri::AppHandle,
    analysis_context: Identity,
    source_generation: Option<u64>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    operation_id: Option<String>,
) -> Result<VisibilitySummary, String> {
    use tauri::Manager;
    let (admitted, events) = capture_case(
        app.state::<AppState>().inner(),
        Some(analysis_context),
        source_generation,
        case_events,
        case_key,
    )?;
    let captured = Arc::clone(&admitted);
    crate::offload_case(operation_id, app, admitted, events, move |_| {
        captured.visibility_summary()
    })
    .await?
}

pub(crate) fn clone_index(index: &sources::FileIndex) -> sources::FileIndex {
    sources::FileIndex {
        parts: index.parts.clone(),
        lines: Arc::clone(&index.lines),
        columns: index.columns.clone(),
        time_order: Arc::clone(&index.time_order),
    }
}

/// Build source descriptors once for a returned page/detail, never for every
/// scanned row. SourceSet verifies framing and exact original event references.
pub(crate) fn source_set(
    index: &sources::FileIndex,
) -> Result<Arc<crate::analysis_visibility::SourceSet>, String> {
    if let Some(admitted) = current() {
        if let Some(SourceData::Indexed(original)) = admitted.source.as_deref() {
            if !Arc::ptr_eq(&index.lines, &original.lines) {
                return Err(STALE.into());
            }
            return admitted
                .source_set
                .get_or_init(|| crate::analysis_visibility::SourceSet::new(index).map(Arc::new))
                .clone();
        }
    }
    crate::analysis_visibility::SourceSet::new(index).map(Arc::new)
}
pub(crate) fn attach_provenance_with(
    index: &sources::FileIndex,
    binding: &crate::analysis_visibility::SourceSet,
    event: &mut Event,
) -> Result<(), String> {
    event.evidence_provenance = binding.provenance(
        index,
        event.id,
        &event.event_ref,
        &crate::analysis_visibility::MaskBudget::default(),
    )?;
    Ok(())
}

pub(crate) fn attach_provenance(
    index: &sources::FileIndex,
    event: &mut Event,
) -> Result<(), String> {
    let binding = source_set(index)?;
    attach_provenance_with(index, &binding, event)
}

pub(crate) fn indexed_gate(index: &sources::FileIndex) -> Result<Option<Arc<RowGate>>, String> {
    let result = indexed_gate_inner(index);
    if let Err(error) = &result {
        record_failure(error.clone());
    }
    result
}
fn indexed_gate_inner(index: &sources::FileIndex) -> Result<Option<Arc<RowGate>>, String> {
    let Some(admitted) = current() else {
        return Ok(None);
    };
    if admitted.mode == Mode::Publish || admitted.identity.is_none() {
        return Ok(None);
    }
    let prepared = admitted
        .visibility
        .get()
        .ok_or("A visibilidade ainda não foi admitida para esta consulta.")?;
    let Some(SourceData::Indexed(original)) = admitted.source.as_deref() else {
        return Err("Esta consulta não pertence à fonte indexada admitida.".into());
    };
    if !Arc::ptr_eq(&index.lines, &original.lines)
        || index.parts.len() != original.parts.len()
        || index.parts.iter().zip(&original.parts).any(|(a, b)| {
            a.base != b.base || !Arc::ptr_eq(&a.mmap, &b.mmap) || a.identity != b.identity
        })
    {
        return Err("A visibilidade pertence a outra geração da fonte.".into());
    }
    if let Some(gate) = &prepared.gate {
        gate.validate()?;
    }
    Ok(prepared.gate.clone())
}
pub(crate) fn visibility_restricted() -> bool {
    current()
        .and_then(|admitted| {
            admitted.visibility.get().map(|prepared| {
                prepared
                    .gate
                    .as_ref()
                    .is_some_and(|gate| !gate.is_unrestricted())
            })
        })
        .unwrap_or(false)
}
pub(crate) fn visibility_unrestricted(index: &sources::FileIndex) -> Result<bool, String> {
    Ok(indexed_gate(index)?.is_none_or(|gate| gate.is_unrestricted()))
}
pub(crate) fn visible_total(index: &sources::FileIndex) -> Result<usize, String> {
    Ok(indexed_gate(index)?.map_or(index.lines.len(), |gate| gate.visible_count()))
}
pub(crate) fn row_visible(index: &sources::FileIndex, row: usize) -> Result<bool, String> {
    match indexed_gate(index)? {
        Some(gate) => gate.allows(row),
        None => Ok(row < index.lines.len()),
    }
}
pub(crate) fn record_failure(error: String) {
    if let Some(admitted) = current() {
        *admitted.failure.lock() = Some(error);
    }
}

/// A capability for hydrating a bounded, already-verified archive page. It is
/// deliberately a different type from query admission and has no query bypass.
pub(crate) struct ArchiveSource {
    admitted: Arc<Admitted>,
    evidence: Option<Vec<Event>>,
    unavailable: Option<String>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveRecord {
    pub member: crate::exclusion_store::Member,
    pub event: Option<Event>,
    pub unavailable_reason: Option<String>,
    pub active_in_batch: bool,
}
pub(crate) fn capture_archive_case(
    state: &AppState,
    identity: Identity,
    generation: Option<u64>,
    events: Option<Vec<Event>>,
    key: Option<String>,
) -> Result<ArchiveSource, String> {
    if events.is_some() || key.is_some() {
        let (admitted, evidence) = capture_case(state, Some(identity), generation, events, key)?;
        return Ok(ArchiveSource {
            admitted,
            evidence,
            unavailable: None,
        });
    }
    match capture(state, Some(identity.clone()), generation, Mode::Dataset) {
        Ok(admitted) => Ok(ArchiveSource {
            admitted,
            evidence: None,
            unavailable: None,
        }),
        Err(error) => {
            // A valid ledger history remains accessible after closing a source.
            // Revalidating the explicit identity still rejects stale/other Cases.
            let admitted = capture(state, Some(identity), None, Mode::Case)?;
            Ok(ArchiveSource {
                admitted,
                evidence: None,
                unavailable: Some(error),
            })
        }
    }
}

impl ArchiveSource {
    pub(crate) fn resolve(
        &self,
        state: &AppState,
        page: &crate::exclusion_store::ArchivePage,
    ) -> Result<Vec<ArchiveRecord>, String> {
        if page.rows.len() > 500 || self.admitted.identity.as_ref() != Some(&page.analysis) {
            return Err("A página do arquivo não pertence à captura de análise.".into());
        }
        validate_identity(&page.analysis)?;
        let unavailable = |reason: String| {
            page.rows
                .iter()
                .map(|row| ArchiveRecord {
                    member: crate::exclusion_store::Member {
                        key: row.key.clone(),
                        event_ref: row.event_ref.clone(),
                    },
                    event: None,
                    unavailable_reason: Some(reason.clone()),
                    active_in_batch: !row.restored_from_batch,
                })
                .collect()
        };
        if let Some(reason) = &self.unavailable {
            return Ok(unavailable(reason.clone()));
        }
        if let Err(error) = self.admitted.validate(state) {
            return Ok(unavailable(error));
        }
        let codes = state.codes.read();
        let system = state.system_codes.read();
        let index = match self.admitted.source.as_deref() {
            Some(SourceData::Indexed(index)) => Some(index),
            _ => None,
        };
        let token = crate::operations::current_token();
        let cancelled = || token.cancelled();
        let progress = |_: &str, completed: u64, total: Option<u64>| {
            crate::operations::report_progress(
                "análise",
                "archive",
                "Localizando registros do arquivo",
                completed as usize,
                total.unwrap_or(0) as usize,
                "registros",
                0,
            )
        };
        let work = crate::exclusion_store::Work {
            cancelled: &cancelled,
            progress: &progress,
        };
        let binding = match index
            .map(|index| {
                crate::analysis_visibility::SourceSet::prepare(
                    index,
                    &crate::analysis_visibility::MaskBudget::default(),
                    &work,
                )
            })
            .transpose()
        {
            Ok(binding) => binding,
            Err(error) => {
                crate::operations::check()?;
                return Ok(unavailable(error));
            }
        };
        let memory = self
            .evidence
            .as_deref()
            .or_else(|| match self.admitted.source.as_deref() {
                Some(SourceData::Memory(events)) => Some(events.as_slice()),
                _ => None,
            });
        let derived = if !page.rows.is_empty() && (index.is_some() || memory.is_some()) {
            self.admitted.prepare_references()?
        } else {
            Arc::new(Vec::new())
        };
        let mut budget = crate::query::AnalyticsBudget::new();
        let mut result = Vec::with_capacity(page.rows.len());
        for row in &page.rows {
            crate::operations::check()?;
            let member = crate::exclusion_store::Member {
                key: row.key.clone(),
                event_ref: row.event_ref.clone(),
            };
            let source = page
                .sources
                .get(&member.key.source_key)
                .ok_or("Proveniência ausente na página do arquivo.")?;
            let mut event = if let (Some(index), Some(binding)) = (index, binding.as_ref()) {
                match binding.resolve_member(
                    index,
                    &member,
                    &crate::analysis_visibility::MaskBudget::default(),
                )? {
                    Some(id) => {
                        let mut event = sources::event_at(index, id, &codes, &system, &derived);
                        attach_provenance_with(index, binding, &mut event)?;
                        Some(event)
                    }
                    None => None,
                }
            } else if let Some(events) = memory {
                let mut matching = events
                    .iter()
                    .take_while(|_| !crate::operations::cancelled())
                    .filter(|event| event.event_ref == member.event_ref)
                    .filter(|event| {
                        if let Some(proof) = &event.evidence_provenance {
                            proof.source.key().ok().as_deref()
                                == Some(member.key.source_key.as_str())
                                && proof.locator == member.key.locator
                        } else if let (
                            Some(prefix),
                            crate::exclusion_store::Locator::ByteOffset(offset),
                        ) = (&source.event_ref_prefix, &member.key.locator)
                        {
                            event.event_ref == format!("{prefix}:{offset}")
                        } else {
                            false
                        }
                    });
                let event = matching.next().cloned();
                if matching.next().is_some() {
                    return Err("Referência ambígua nas evidências do Caso.".into());
                }
                event
            } else {
                None
            };
            if let Some(event) = &mut event {
                sources::apply_derived(event, &derived);
                crate::entities::annotate(event);
                budget.charge(crate::query::event_payload_bytes(event))?;
            }
            result.push(ArchiveRecord { member, unavailable_reason: event.is_none().then(|| "O registro original com esta proveniência não está disponível na captura atual.".into()), event, active_in_batch: !row.restored_from_batch });
        }
        crate::operations::check()?;
        Ok(result)
    }
}

pub(crate) enum SourceView<'a> {
    Captured(Arc<SourceData>),
    Legacy(RwLockReadGuard<'a, SourceData>),
}
impl Deref for SourceView<'_> {
    type Target = SourceData;
    fn deref(&self) -> &SourceData {
        match self {
            Self::Captured(value) => value,
            Self::Legacy(value) => value,
        }
    }
}
pub(crate) fn source(state: &AppState) -> SourceView<'_> {
    match current().and_then(|admitted| {
        admitted
            .visibility
            .get()
            .and_then(|prepared| prepared.source.clone())
            .or_else(|| admitted.source.clone())
    }) {
        Some(source) => SourceView::Captured(source),
        None if current().is_some_and(|admitted| admitted.mode == Mode::Case) => {
            SourceView::Captured(Arc::new(SourceData::None))
        }
        None => SourceView::Legacy(state.source.read()),
    }
}
pub(crate) fn derived(state: &AppState) -> Arc<Vec<sources::CompiledDerived>> {
    current()
        .map(|admitted| {
            if let Some(prepared) = admitted.prepared_fields.get() {
                return Arc::clone(prepared);
            }
            if admitted.derived.iter().any(|field| field.lookup.is_some()) {
                record_failure(
                    "As referências ainda não foram preparadas para esta consulta.".into(),
                );
                return Arc::new(Vec::new());
            }
            Arc::clone(&admitted.derived)
        })
        .unwrap_or_else(|| Arc::new(state.derived.read().clone()))
}
/// Immutable descriptor metadata only; readers and lookup materialization stay
/// local to worker preparation and must bind content/schema version hashes.
pub(crate) fn reference_descriptors() -> Arc<Vec<analysis_context::ReferenceDescriptor>> {
    current()
        .map(|admitted| Arc::clone(&admitted.references))
        .unwrap_or_else(|| Arc::new(Vec::new()))
}

pub(crate) fn source_names(state: &AppState) -> Vec<String> {
    current()
        .filter(|admitted| admitted.mode == Mode::Dataset)
        .map(|admitted| admitted.names.clone())
        .unwrap_or_else(|| state.source_names.read().clone())
}
pub(crate) fn cache_namespace() -> String {
    current()
        .map(|admitted| {
            format!(
                "{:?}|{:?}|{:?}",
                admitted.identity, admitted.source_generation, admitted.case_key
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::{Mutex, RwLock};
    use serde_json::json;
    fn state(source: SourceData) -> AppState {
        AppState {
            source: RwLock::new(source),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(Vec::new()),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        }
    }
    struct Directory {
        path: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl Directory {
        fn new() -> Self {
            let path = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("LOGINSIGHT_DATA_DIR");
            std::env::set_var("LOGINSIGHT_DATA_DIR", path.path());
            crate::case_store::save(json!({"cases":[{"id":"a"},{"id":"b"}],"active":null}))
                .unwrap();
            Self { path, previous }
        }
        fn snapshot(&self, id: &str) -> Snapshot {
            analysis_context::snapshot(id).unwrap()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    fn config(value: &str) -> analysis_context::Config {
        serde_json::from_value(json!({"derivedFields":[{"name":"tag","source":"message","rules":[{"pattern":"(.*)","template":value}]}]})).unwrap()
    }
    fn original() -> Event {
        let mut event = Event::empty();
        event.message = "raw message".into();
        event.event_ref = "original:0".into();
        event
    }
    #[test]
    fn definitions_are_case_owned_cached_and_applied_only_to_copies() {
        let directory = Directory::new();
        let a = analysis_context::update(&directory.snapshot("a").identity(), config("A")).unwrap();
        let b = analysis_context::update(&directory.snapshot("b").identity(), config("B")).unwrap();
        let state = state(SourceData::None);
        let evidence = vec![original()];
        let (first, a_events) = capture_case(
            &state,
            Some(a.identity()),
            None,
            Some(evidence.clone()),
            None,
        )
        .unwrap();
        let (again, _) = capture_case(
            &state,
            Some(a.identity()),
            None,
            Some(evidence.clone()),
            None,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&first.derived, &again.derived));
        let a_events = first.prepare_visibility(a_events).unwrap();
        assert_eq!(a_events.as_ref().unwrap()[0].fields["tag"], "A");
        let (b_admitted, b_events) =
            capture_case(&state, Some(b.identity()), None, a_events, None).unwrap();
        let b_events = b_admitted.prepare_visibility(b_events).unwrap();
        assert_eq!(b_events.as_ref().unwrap()[0].fields["tag"], "B");
        let empty = analysis_context::update(&b.identity(), Default::default()).unwrap();
        let (empty_admitted, empty_events) =
            capture_case(&state, Some(empty.identity()), None, b_events, None).unwrap();
        let empty_events = empty_admitted.prepare_visibility(empty_events).unwrap();
        assert!(empty_events.unwrap()[0].fields.get("tag").is_none());
        assert!(evidence[0].fields.is_empty());
        assert!(first.validate(&state).is_ok());
        analysis_context::update(&a.identity(), config("A2")).unwrap();
        assert_eq!(first.validate(&state).unwrap_err(), STALE);
        with(Some(first), || {
            assert_eq!(derived(&state)[0].rules[0].template.as_deref(), Some("A"))
        });
    }
    #[test]
    fn publication_owner_generation_and_producer_context_cannot_be_adopted() {
        let directory = Directory::new();
        let state = state(SourceData::Memory(vec![original()]));
        let a = directory.snapshot("a").identity();
        let b = directory.snapshot("b").identity();
        let publishing = capture(&state, Some(a.clone()), Some(0), Mode::Publish).unwrap();
        with(Some(publishing), || {
            crate::source_publication::clear(&state).unwrap();
        });
        assert!(capture(&state, Some(b), Some(1), Mode::Dataset).is_err());
        assert!(capture(&state, None, Some(1), Mode::Dataset).is_err());
        assert!(capture(&state, Some(a.clone()), Some(0), Mode::Dataset).is_err());
        let admitted = capture(&state, Some(a.clone()), Some(1), Mode::Dataset).unwrap();
        with(Some(admitted.clone()), || {
            let token = crate::operations::current_token();
            std::thread::spawn(move || {
                crate::operations::run_with_token(token, || {
                    assert_eq!(current().unwrap().identity.as_ref(), Some(&a))
                })
            })
            .join()
            .unwrap()
            .unwrap();
        });
        assert!(current().is_none());
        crate::source_publication::clear(&state).unwrap();
        assert_eq!(admitted.validate(&state).unwrap_err(), STALE);
    }
    #[test]
    fn immutable_index_shares_sort_cache_and_never_reacquires_source_lock() {
        let directory = Directory::new();
        let path = directory.path.path().join("events.jsonl");
        std::fs::write(&path, "{\"message\":\"two\"}\n{\"message\":\"one\"}\n").unwrap();
        let index = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        index.ordered();
        let ordered = Arc::clone(&index.time_order);
        let state = state(SourceData::Indexed(index));
        let admitted = capture(&state, None, Some(0), Mode::Dataset).unwrap();
        admitted.validate(&state).unwrap();
        with(Some(admitted), || {
            let write = state.source.write();
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!("captured index missing")
            };
            assert!(Arc::ptr_eq(&ordered, &index.time_order));
            drop(write);
        });
    }
    #[test]
    fn migrated_invalid_definitions_keep_raw_fields_and_disable_dependents() {
        let mut snapshot = Snapshot {
            schema_version: 1,
            case_id: "migration".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
            config: serde_json::from_value(json!({"derivedFields":[
                {"name":"message","source":"raw","rules":[{"pattern":"["}]},
                {"name":"valid","source":"message","rules":[{"pattern":"(.*)"}]},
                {"name":"bad","source":"message","rules":[{"pattern":"["}]},
                {"name":"dependent","source":"bad.child","rules":[{"pattern":"(.*)"}]}
            ]}))
            .unwrap(),
            migration_diagnostics: vec![],
            legacy_raw: Some(json!({"preserved":"earlier damaged input"})),
        };
        for index in [0, 2] {
            snapshot
                .migration_diagnostics
                .push(analysis_context::Diagnostic {
                    definition_index: Some(index),
                    code: "invalid_legacy_definition".into(),
                    message: "invalid".into(),
                });
        }
        let fields = compile_uncached(&snapshot).unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["valid"]
        );
        let mut event = original();
        sources::apply_derived(&mut event, &fields);
        assert_eq!(event.message, "raw message");
        assert_eq!(event.fields["valid"], "raw message");
        snapshot
            .migration_diagnostics
            .push(analysis_context::Diagnostic {
                definition_index: None,
                code: "invalid_legacy_dependencies".into(),
                message: "cycle".into(),
            });
        assert!(compile_uncached(&snapshot).unwrap().is_empty());
    }
    #[test]
    fn excessive_rules_are_diagnosed_without_blocking_raw_records() {
        let mut snapshot = Snapshot {
            schema_version: 1,
            case_id: "budget".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
            config: Default::default(),
            migration_diagnostics: vec![],
            legacy_raw: None,
        };
        snapshot.config.derived_fields = (0..5).map(|n| json!({"name":format!("field{n}"),"source":"message","rules":(0..256).map(|_|json!({"pattern":"(.*)"})).collect::<Vec<_>>()})).collect();
        assert!(validate_compilation(&snapshot.config).is_err());
        let (fields, diagnostics) = compile(&snapshot).unwrap();
        assert!(fields.is_empty());
        assert_eq!(
            diagnostics.last().unwrap().code,
            "disabled_runtime_definitions"
        );
    }
    #[test]
    fn committed_active_case_blocks_identityless_legacy_admission() {
        let directory = Directory::new();
        let state = state(SourceData::Memory(vec![original()]));
        let queued = capture(&state, None, Some(0), Mode::Dataset).unwrap();
        let mut cases = crate::case_store::load().unwrap();
        cases["active"] = json!("a");
        crate::case_store::save(cases).unwrap();
        assert!(capture(&state, None, Some(0), Mode::Dataset).is_err());
        assert_eq!(queued.validate(&state).unwrap_err(), STALE);
        let a = directory.snapshot("a");
        let (_, evidence) = capture_case(
            &state,
            Some(a.identity()),
            None,
            Some(vec![original()]),
            None,
        )
        .unwrap();
        assert!(evidence.unwrap()[0].fields.is_empty());
        assert!(capture_case(&state, None, None, Some(vec![original()]), None).is_err());
    }

    #[test]
    fn retimestamp_advances_generation_and_failed_save_restores_snapshot() {
        let directory = Directory::new();
        let path = directory.path.path().join("timestamp.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":\"event\"}\n",
        )
        .unwrap();
        let index = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let state = state(SourceData::Indexed(index));
        let queued = capture(&state, None, Some(0), Mode::Dataset).unwrap();
        let receipt =
            crate::set_ts_config_impl(&state, path.to_str().unwrap(), None, None).unwrap();
        assert_eq!(receipt.publication.generation, 1);
        assert_eq!(queued.validate(&state).unwrap_err(), STALE);
        let (lines, order) = {
            let source = state.source.read();
            let SourceData::Indexed(index) = &*source else {
                panic!()
            };
            index.ordered();
            (index.lines.clone(), index.time_order.clone())
        };
        std::fs::remove_file(crate::ts_configs_path()).unwrap();
        std::fs::create_dir(crate::ts_configs_path()).unwrap();
        assert!(crate::set_ts_config_impl(
            &state,
            path.to_str().unwrap(),
            Some(sources::TsConfig {
                clock_adjustment_ms: 1000,
                ..Default::default()
            }),
            None
        )
        .is_err());
        assert_eq!(
            crate::source_publication::receipt_locked(&state).generation,
            1
        );
        let source = state.source.read();
        let SourceData::Indexed(index) = &*source else {
            panic!()
        };
        assert!(Arc::ptr_eq(&lines, &index.lines));
        assert!(Arc::ptr_eq(&order, &index.time_order));
    }
    #[test]
    fn queued_visibility_and_deleted_case_identities_are_rejected() {
        let directory = Directory::new();
        let state = state(SourceData::None);
        let a = directory.snapshot("a");
        let (queued, _) =
            capture_case(&state, Some(a.identity()), None, Some(vec![]), None).unwrap();
        let (changed, _) = analysis_context::visibility_update_at(
            directory.path.path(),
            &a.identity(),
            &|| false,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(queued.validate(&state).unwrap_err(), STALE);
        let (restored, events) =
            capture_case(&state, Some(changed.identity()), None, Some(vec![]), None).unwrap();
        assert!(
            restored
                .prepare_visibility(events)
                .unwrap()
                .unwrap()
                .is_empty(),
            "actual empty membership is valid after any revision"
        );
        let mut cases = crate::case_store::load().unwrap();
        cases["cases"] = json!([]);
        cases["active"] = serde_json::Value::Null;
        let saved = crate::case_store::save(cases).unwrap();
        crate::case_store::save(
            json!({"revision":saved["revision"],"active":null,"cases":[{"id":"a"}]}),
        )
        .unwrap();
        let replacement = directory.snapshot("a");
        assert_ne!(replacement.analysis_id, a.analysis_id);
        assert!(capture_case(&state, Some(a.identity()), None, Some(vec![]), None).is_err());
        assert!(capture_case(
            &state,
            Some(replacement.identity()),
            None,
            Some(vec![]),
            None
        )
        .is_ok());
    }

    #[test]
    fn memory_snapshots_and_grouped_case_timeline_use_captured_definitions() {
        let directory = Directory::new();
        let a = analysis_context::update(&directory.snapshot("a").identity(), config("A")).unwrap();
        let state = state(SourceData::Memory(vec![original()]));
        *state.derived.write() = compile_uncached(&a).unwrap();
        let captured = capture(&state, None, Some(0), Mode::Dataset).unwrap();
        state.derived.write().clear();
        captured.prepare_visibility(None).unwrap();
        with(Some(captured), || {
            let view = source(&state);
            let SourceData::Memory(events) = &*view else {
                panic!()
            };
            assert_eq!(events[0].fields["tag"], "A");
        });
        assert!(
            matches!(&*state.source.read(), SourceData::Memory(events) if events[0].fields.is_empty())
        );
        let mut event = original();
        event.timestamp = Some(1000);
        let (admitted, events) = capture_case(
            &state,
            Some(a.identity()),
            None,
            Some(vec![event]),
            Some("evidence-v1".into()),
        )
        .unwrap();
        let context = crate::grouped_timeline::Context {
            analysis: Some(a.identity()),
            source_generation: None,
            case_key: Some("evidence-v1".into()),
        };
        let spec = crate::grouped_timeline::Spec::new(
            "tag".into(),
            crate::grouped_timeline::Grid {
                start: 1000,
                bucket_ms: 100,
                bucket_count: 2,
            },
            None,
            context.clone(),
        )
        .unwrap();
        let events = admitted.prepare_visibility(events).unwrap();
        let result = with(Some(admitted), || {
            crate::workspace::grouped_timeline_impl(&state, &[], events.as_deref(), &spec)
        })
        .unwrap();
        assert_eq!(result.context, context);
        assert_eq!(result.series[0].key, "A");
        assert_eq!(result.total.count, 1);
    }
    fn visibility_fixture(directory: &Directory) -> (AppState, Vec<Event>) {
        let path = directory.path.path().join("visible.jsonl");
        let text = (0..12).map(|n| json!({"timestamp":1700000000000i64+n*1000,"source":"fixture","level":if n%2==0 {"ERROR"} else {"INFO"},"message":format!("event {n}"),"group":if n<6 {"A"} else {"B"},"value":n,"trace_id":if n==0 {"0123456789abcdef0123456789abcdef"} else {"ordinary"}}).to_string()+"\n").collect::<String>();
        std::fs::write(&path, text).unwrap();
        let index = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = crate::model::CodesConfig::default();
        let binding = crate::analysis_visibility::SourceSet::new(&index).unwrap();
        let events = (0..12)
            .map(|id| {
                let mut event = sources::event_at(&index, id, &codes, &codes, &[]);
                attach_provenance_with(&index, &binding, &mut event).unwrap();
                event
            })
            .collect();
        let state = state(SourceData::None);
        let owner = capture(
            &state,
            Some(directory.snapshot("a").identity()),
            Some(0),
            Mode::Publish,
        )
        .unwrap();
        with(Some(owner), || {
            crate::source_publication::publish(&state, index, vec!["fixture".into()], vec![], false)
        })
        .unwrap();
        (state, events)
    }
    fn exclude(
        directory: &Directory,
        state: &AppState,
        rows: &[usize],
    ) -> crate::exclusion_store::Receipt {
        let work = crate::exclusion_store::Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        };
        let source = state.source.read();
        let SourceData::Indexed(index) = &*source else {
            panic!()
        };
        let binding =
            crate::analysis_visibility::SourceSet::prepare(index, &Default::default(), &work)
                .unwrap();
        let admission = crate::exclusion_store::Admission {
            analysis: directory.snapshot("a").identity(),
            source_receipt: serde_json::to_value(crate::source_publication::receipt_locked(state))
                .unwrap(),
        };
        let members = rows
            .iter()
            .map(|&row| binding.member_row(index, row, &Default::default()));
        let staged = crate::exclusion_store::stage(
            directory.path.path(),
            admission,
            crate::exclusion_store::Purpose::Exclude,
            binding.descriptors(),
            members,
            &Default::default(),
            &work,
        )
        .unwrap();
        crate::exclusion_store::publish(
            directory.path.path(),
            staged,
            "fixture batch",
            "test",
            json!({}),
            &Default::default(),
            &work,
        )
        .unwrap()
    }
    fn restore(
        directory: &Directory,
        state: &AppState,
        batch: &str,
    ) -> crate::exclusion_store::Receipt {
        crate::exclusion_store::restore_batch(
            directory.path.path(),
            &crate::exclusion_store::Admission {
                analysis: directory.snapshot("a").identity(),
                source_receipt: serde_json::to_value(crate::source_publication::receipt_locked(
                    state,
                ))
                .unwrap(),
            },
            batch,
            &crate::exclusion_store::Work {
                cancelled: &|| false,
                progress: &|_, _, _| {},
            },
        )
        .unwrap()
    }
    fn admitted_dataset(directory: &Directory, state: &AppState) -> Arc<Admitted> {
        let admitted = capture(
            state,
            Some(directory.snapshot("a").identity()),
            Some(crate::source_publication::receipt_locked(state).generation),
            Mode::Dataset,
        )
        .unwrap();
        admitted.validate(state).unwrap();
        admitted.prepare_visibility(None).unwrap();
        admitted
    }
    #[test]
    fn visible_sql_fallback_case_memory_counts_pages_and_analytics_agree() {
        let directory = Directory::new();
        let (state, originals) = visibility_fixture(&directory);
        exclude(&directory, &state, &[0, 5]);
        exclude(&directory, &state, &[5, 11]);
        let expected: Vec<_> = originals
            .iter()
            .filter(|event| ![0, 5, 11].contains(&event.id))
            .cloned()
            .collect();
        let admitted = admitted_dataset(&directory, &state);
        struct ResetEngine;
        impl Drop for ResetEngine {
            fn drop(&mut self) {
                crate::engine::set_enabled(false);
            }
        }
        let _reset = ResetEngine;
        for sql in [false, true] {
            crate::engine::set_enabled(sql);
            if sql {
                let source = state.source.read();
                let SourceData::Indexed(index) = &*source else {
                    panic!()
                };
                crate::engine::prepare(
                    index,
                    &Default::default(),
                    &Default::default(),
                    &[],
                    &|_, _| {},
                )
                .unwrap();
            }
            with(Some(admitted.clone()), || {
                assert_eq!(crate::count_filtered_impl(&state, vec![], None).unwrap(), 9);
                let union = crate::query::Filter {
                    column: "_all".into(),
                    op: "query".into(),
                    value: "group:A OR group:B".into(),
                    value2: None,
                };
                assert_eq!(
                    crate::count_filtered_impl(&state, vec![union], None).unwrap(),
                    9
                );
                let page = crate::query_events_impl(&state, vec![], "id", "asc", 0, 100).unwrap();
                assert_eq!(page.total, 9);
                assert_eq!(
                    page.rows.iter().map(|event| event.id).collect::<Vec<_>>(),
                    vec![1, 2, 3, 4, 6, 7, 8, 9, 10]
                );
                assert!(page
                    .rows
                    .iter()
                    .all(|event| event.evidence_provenance.is_some()));
                assert!(crate::event_detail_raw(&state, 0).is_none());
                assert_eq!(crate::event_detail_raw(&state, 1).unwrap().id, 1);
                assert_eq!(crate::source_summary_impl(&state).unwrap().count, 9);
                assert_eq!(
                    serde_json::to_value(crate::workspace::sources_impl(&state).unwrap()).unwrap()
                        [0]["count"],
                    9
                );
                let actual = crate::stats_events_impl(&state, vec![]).unwrap();
                let expected_stats = crate::query::stats(&expected, &[]);
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(expected_stats).unwrap()
                );
                assert_eq!(
                    crate::workspace::overview_impl(&state, vec![])
                        .unwrap()
                        .total,
                    9
                );
                let spec = crate::grouped_timeline::Spec::new(
                    "group".into(),
                    crate::grouped_timeline::Grid {
                        start: 1700000000000,
                        bucket_ms: 1000,
                        bucket_count: 12,
                    },
                    Some(1),
                    crate::grouped_timeline::Context::default(),
                )
                .unwrap();
                let grouped =
                    crate::workspace::grouped_timeline_impl(&state, &[], None, &spec).unwrap();
                assert_eq!(grouped.total.count, 9);
                assert_eq!(grouped.series[0].key, "B");
                assert_eq!(grouped.series[0].count, 5);
                assert_eq!(grouped.other.count, 4);
                let pivot:crate::analysis::PivotSpec=serde_json::from_value(json!({"rows":["group"],"cols":[],"values":[{"func":"count","column":"*","alias":"n"}]})).unwrap();
                assert_eq!(
                    serde_json::to_value(
                        crate::pivot_impl(&state, vec![], None, pivot.clone()).unwrap()
                    )
                    .unwrap(),
                    serde_json::to_value(crate::analysis::pivot(&expected, &pivot).unwrap())
                        .unwrap()
                );
                let journey =
                    crate::journeys::index_impl(&state, &[], None, "group", 0, 100, "recent", true)
                        .unwrap();
                let expected_journey = crate::journeys::index_impl(
                    &state,
                    &[],
                    Some(&expected),
                    "group",
                    0,
                    100,
                    "recent",
                    true,
                )
                .unwrap();
                assert_eq!(
                    serde_json::to_value(journey).unwrap(),
                    serde_json::to_value(expected_journey).unwrap()
                );
                let mut exported = Vec::new();
                let total = crate::workspace::with_selection(&state, &[], |selection| {
                    crate::workspace::write_events_export(&mut exported, "jsonl", false, || {
                        selection.iter()
                    })
                })
                .unwrap()
                .unwrap();
                assert_eq!(total, 9);
                assert_eq!(String::from_utf8(exported).unwrap().lines().count(), 9);
                let singleton = crate::query::Filter {
                    column: "trace_id".into(),
                    op: "equals_exact".into(),
                    value: "0123456789abcdef0123456789abcdef".into(),
                    value2: None,
                };
                let source = source(&state);
                let SourceData::Indexed(index) = &*source else {
                    panic!()
                };
                let page = crate::query::query_page_indexed(
                    index,
                    &[singleton],
                    "id",
                    "asc",
                    0,
                    5,
                    None,
                    &Default::default(),
                    &Default::default(),
                    &[],
                )
                .unwrap();
                assert!(page.rows.is_empty());
                assert_eq!(page.total, Some(0));
                if sql {
                    let mut ids = Vec::new();
                    let visited = crate::workspace::with_selection(&state, &[], |selection| {
                        selection.visit_exact_ids(|id| {
                            ids.push(id);
                            Ok(true)
                        })
                    })
                    .unwrap()
                    .unwrap();
                    assert!(visited.is_some());
                    assert_eq!(ids.len(), 9);
                }
            });
        }
        let (case_admitted, case_events) = capture_case(
            &state,
            Some(directory.snapshot("a").identity()),
            None,
            Some(originals.clone()),
            Some("evidence".into()),
        )
        .unwrap();
        let filtered = case_admitted
            .prepare_visibility(case_events)
            .unwrap()
            .unwrap();
        assert_eq!(
            filtered.iter().map(|event| event.id).collect::<Vec<_>>(),
            expected.iter().map(|event| event.id).collect::<Vec<_>>()
        );
        assert_eq!(originals.len(), 12);
        let memory = self::state(SourceData::Memory(originals));
        let owner = capture(
            &memory,
            Some(directory.snapshot("a").identity()),
            Some(0),
            Mode::Publish,
        )
        .unwrap();
        with(Some(owner), || {
            let _source = crate::source_write_checked(&memory).unwrap();
            let receipt = crate::source_publication::prepare_touch_locked(&memory).unwrap();
            crate::source_publication::commit_touch_locked(&memory, receipt);
        });
        let admitted = admitted_dataset(&directory, &memory);
        with(Some(admitted), || {
            assert_eq!(
                crate::count_filtered_impl(&memory, vec![], None).unwrap(),
                9
            );
            assert!(crate::event_detail_raw(&memory, 0).is_none());
            assert_eq!(crate::event_detail_raw(&memory, 6).unwrap().id, 6);
        });
    }
    #[test]
    fn restores_invalidate_cursors_and_real_empty_membership_reopens_fast_paths() {
        let directory = Directory::new();
        let (state, _) = visibility_fixture(&directory);
        let first = exclude(&directory, &state, &[0, 5]);
        let second = exclude(&directory, &state, &[5, 11]);
        crate::engine::set_enabled(true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::engine::set_enabled(false);
            }
        }
        let _reset = Reset;
        {
            let source = state.source.read();
            let SourceData::Indexed(index) = &*source else {
                panic!()
            };
            crate::engine::prepare(
                index,
                &Default::default(),
                &Default::default(),
                &[],
                &|_, _| {},
            )
            .unwrap();
        }
        let before = admitted_dataset(&directory, &state);
        let cursor = with(Some(before), || {
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            crate::query::query_page_indexed(
                index,
                &[],
                "id",
                "asc",
                0,
                3,
                None,
                &Default::default(),
                &Default::default(),
                &[],
            )
            .unwrap()
            .next_cursor
            .unwrap()
        });
        // Registry tokens are statement-local. A fresh admission of the same
        // immutable identity must continue an earlier cursor and reuse results.
        let unchanged = admitted_dataset(&directory, &state);
        with(Some(unchanged), || {
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            let next = crate::query::query_page_indexed(
                index,
                &[],
                "id",
                "asc",
                3,
                3,
                Some(&cursor),
                &Default::default(),
                &Default::default(),
                &[],
            )
            .unwrap();
            assert_eq!(
                next.rows.iter().map(|event| event.id).collect::<Vec<_>>(),
                vec![4, 6, 7]
            );
            assert_eq!(crate::count_filtered_impl(&state, vec![], None).unwrap(), 9);
            assert_eq!(crate::count_filtered_impl(&state, vec![], None).unwrap(), 9);
        });
        restore(&directory, &state, &first.batch_id);
        let restored = admitted_dataset(&directory, &state);
        with(Some(restored), || {
            assert_eq!(
                crate::count_filtered_impl(&state, vec![], None).unwrap(),
                10
            );
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            assert!(crate::query::query_page_indexed(
                index,
                &[],
                "id",
                "asc",
                3,
                3,
                Some(&cursor),
                &Default::default(),
                &Default::default(),
                &[]
            )
            .err()
            .expect("a cursor from the old visibility must be rejected")
            .contains("PAGINATION_RESET_REQUIRED"));
            assert!(!visibility_unrestricted(index).unwrap());
        });
        restore(&directory, &state, &second.batch_id);
        let empty = admitted_dataset(&directory, &state);
        assert!(empty.identity.as_ref().unwrap().visibility_revision > 0);
        with(Some(empty), || {
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            assert!(visibility_unrestricted(index).unwrap());
            assert_eq!(visible_total(index).unwrap(), 12);
            assert_eq!(
                crate::count_filtered_impl(&state, vec![], None).unwrap(),
                12
            );
        });
    }
    #[test]
    fn corrupt_membership_and_unknown_evidence_fail_without_mutating_originals() {
        let directory = Directory::new();
        let (state, originals) = visibility_fixture(&directory);
        let receipt = exclude(&directory, &state, &[0]);
        let good = admitted_dataset(&directory, &state);
        good.validate_visibility().unwrap();
        let mut unsupported = original();
        unsupported.event_ref.clear();
        let (mut capture, events) = capture_case(
            &state,
            Some(directory.snapshot("a").identity()),
            None,
            Some(vec![unsupported]),
            None,
        )
        .unwrap();
        assert!(capture.prepare_visibility(events).is_err());
        let payload = directory
            .path
            .path()
            .join("exclusions-v1")
            .join(format!("{}.sqlite3", receipt.batch_id));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(payload)
            .unwrap();
        file.set_len(10).unwrap();
        assert!(good.validate_visibility().is_err());
        capture = capture_case(
            &state,
            Some(directory.snapshot("a").identity()),
            None,
            Some(originals.clone()),
            None,
        )
        .unwrap()
        .0;
        assert!(capture.prepare_visibility(Some(originals.clone())).is_err());
        assert_eq!(originals.len(), 12);
        let view = state.source.read();
        let SourceData::Indexed(index) = &*view else {
            panic!()
        };
        assert_eq!(index.lines.len(), 12);
    }
    #[test]
    fn bounded_archive_resolves_original_members_after_restore_and_source_close() {
        let directory = Directory::new();
        let (state, _) = visibility_fixture(&directory);
        let batch = exclude(&directory, &state, &[0, 5]);
        restore(&directory, &state, &batch.batch_id);
        let identity = directory.snapshot("a").identity();
        let work = crate::exclusion_store::Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        };
        let page = crate::exclusion_store::archive_page(
            directory.path.path(),
            &identity,
            &batch.batch_id,
            None,
            10,
            &work,
        )
        .unwrap();
        assert_eq!(page.rows.len(), 2);
        assert!(page.rows.iter().all(|row| row.restored_from_batch));
        let archive = capture_archive_case(&state, identity.clone(), None, None, None).unwrap();
        let records = archive.resolve(&state, &page).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|row| row.event.as_ref().unwrap().id)
                .collect::<Vec<_>>(),
            vec![0, 5]
        );
        assert!(records
            .iter()
            .all(|row| !row.active_in_batch && row.unavailable_reason.is_none()));
        let mut wrong = page.clone();
        wrong.analysis = directory.snapshot("b").identity();
        assert!(archive.resolve(&state, &wrong).is_err());
        let mut too_large = page.clone();
        too_large.rows = vec![page.rows[0].clone(); 501];
        assert!(archive.resolve(&state, &too_large).is_err());
        *state.source.write() = SourceData::None;
        let archive = capture_archive_case(&state, identity, None, None, None).unwrap();
        let unavailable = archive.resolve(&state, &page).unwrap();
        assert_eq!(unavailable.len(), 2);
        assert!(unavailable.iter().all(|row| row.event.is_none()
            && row.unavailable_reason.is_some()
            && !row.active_in_batch));
    }
    #[test]
    fn visibility_summary_reports_changed_versions_and_missing_source_without_remapping() {
        use std::io::Write;
        let directory = Directory::new();
        let (state, _) = visibility_fixture(&directory);
        exclude(&directory, &state, &[0, 5]);
        exclude(&directory, &state, &[5, 11]);
        let before = admitted_dataset(&directory, &state);
        let summary = before.visibility_summary().unwrap();
        assert_eq!(summary.scope, VisibilityScope::Dataset);
        assert_eq!(
            (
                summary.total_rows,
                summary.excluded_rows,
                summary.unavailable_members
            ),
            (Some(12), Some(3), Some(0))
        );
        let path = directory.path.path().join("visible.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({"message":"appended record", "group":"C"})
        )
        .unwrap();
        drop(file);
        let index = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let publication = capture(
            &state,
            Some(directory.snapshot("a").identity()),
            Some(crate::source_publication::receipt_locked(&state).generation),
            Mode::Publish,
        )
        .unwrap();
        with(Some(publication), || {
            crate::source_publication::publish(&state, index, vec!["fixture".into()], vec![], false)
        })
        .unwrap();
        assert!(before.validate(&state).is_err());
        let admitted = admitted_dataset(&directory, &state);
        let summary = admitted.visibility_summary().unwrap();
        assert_eq!(summary.analysis, directory.snapshot("a").identity());
        assert_eq!(
            summary.source_generation,
            Some(crate::source_publication::receipt_locked(&state).generation)
        );
        assert!(summary.source_available);
        assert_eq!(
            (
                summary.total_rows,
                summary.excluded_rows,
                summary.unavailable_members
            ),
            (Some(13), Some(0), Some(3))
        );
        let owner = capture(
            &state,
            Some(summary.analysis),
            summary.source_generation,
            Mode::Publish,
        )
        .unwrap();
        with(Some(owner), || crate::source_publication::clear(&state)).unwrap();
        let summary = admitted_dataset(&directory, &state)
            .visibility_summary()
            .unwrap();
        assert!(!summary.source_available);
        assert_eq!(
            (
                summary.total_rows,
                summary.excluded_rows,
                summary.unavailable_members
            ),
            (None, None, None)
        );
    }

    #[test]
    fn visibility_summary_keeps_case_subset_and_unavailable_members_distinct() {
        let directory = Directory::new();
        let (state, originals) = visibility_fixture(&directory);
        exclude(&directory, &state, &[0, 5, 11]);
        let identity = directory.snapshot("a").identity();
        let subset = vec![originals[0].clone(), originals[1].clone()];
        let (admitted, events) =
            capture_case(&state, Some(identity.clone()), None, Some(subset), None).unwrap();
        assert!(admitted.visibility_summary().is_err());
        let filtered = admitted.prepare_visibility(events).unwrap().unwrap();
        assert_eq!(filtered.len(), 1);
        let value = serde_json::to_value(admitted.visibility_summary().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"analysis":identity, "sourceGeneration":null, "scope":"case",
            "sourceAvailable":true, "totalRows":2, "excludedRows":1, "unavailableMembers":null})
        );
        let (empty, events) = capture_case(
            &state,
            Some(directory.snapshot("a").identity()),
            None,
            Some(vec![]),
            None,
        )
        .unwrap();
        empty.prepare_visibility(events).unwrap();
        let summary = empty.visibility_summary().unwrap();
        assert!(summary.source_available);
        assert_eq!(
            (
                summary.total_rows,
                summary.excluded_rows,
                summary.unavailable_members
            ),
            (Some(0), Some(0), None)
        );
        assert_eq!(originals.len(), 12);
    }
    fn lookup_snapshot(directory: &Directory, case: &str, value: &str, install: bool) -> Snapshot {
        use sha2::Digest;
        let bytes = (json!({"key":1,"value":value}).to_string() + "\n").into_bytes();
        let descriptor = analysis_context::ReferenceDescriptor {
            schema_version: 1,
            id: "shared-reference-id".into(),
            name: "Fixture reference".into(),
            content_sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
            format: "jsonl".into(),
            columns: vec!["key".into(), "value".into()],
            key_columns: vec!["key".into()],
            duplicate_policy: "reject".into(),
        };
        let config = analysis_context::Config {
            derived_fields: vec![json!({"name":"lookup_tag","lookup":{"schemaVersion":1,
                "referenceId":"shared-reference-id","keys":[{"referenceColumn":"key","sourceField":"lookup_key"}],"valueColumn":"value"}})],
            references: vec![descriptor.clone()],
        };
        let snapshot =
            analysis_context::update(&directory.snapshot(case).identity(), config).unwrap();
        if install {
            let owner = crate::reference_store::Owner {
                case_id: case.into(),
                analysis_id: snapshot.analysis_id.clone(),
            };
            crate::reference_store::prepare_jsonl(
                directory.path.path(),
                &owner,
                &descriptor,
                &bytes[..],
                Default::default(),
                &|| false,
            )
            .unwrap();
        }
        snapshot
    }
    fn lookup_event() -> Event {
        let mut event = original();
        event.fields.insert("lookup_key".into(), json!(1));
        event
    }
    fn prepare_lookup_case(
        state: &AppState,
        identity: Identity,
        event: Event,
    ) -> (Arc<Admitted>, Vec<Event>) {
        let (admitted, events) =
            capture_case(state, Some(identity), None, Some(vec![event]), None).unwrap();
        let token = crate::operations::token(None).unwrap();
        let filtered =
            crate::operations::run_with_token(token, || admitted.prepare_visibility(events))
                .unwrap()
                .unwrap()
                .unwrap();
        (admitted, filtered)
    }

    #[test]
    fn references_are_prepared_in_workers_and_pinned_to_case_and_config() {
        let directory = Directory::new();
        let a = lookup_snapshot(&directory, "a", "owner-a", true);
        let b = lookup_snapshot(&directory, "b", "owner-b", true);
        let state = state(SourceData::None);
        let original = lookup_event();
        let (first, events) = prepare_lookup_case(&state, a.identity(), original.clone());
        assert_eq!(events[0].fields.get("lookup_tag"), Some(&json!("owner-a")));
        assert!(original.fields.get("lookup_tag").is_none());
        assert!(first.derived[0]
            .lookup
            .as_ref()
            .unwrap()
            .version()
            .is_none());
        let prepared = first.prepared_fields.get().unwrap();
        assert_eq!(
            prepared[0]
                .lookup
                .as_ref()
                .unwrap()
                .version()
                .unwrap()
                .owner
                .case_id,
            "a"
        );
        let (other, events) = prepare_lookup_case(&state, b.identity(), original.clone());
        assert_eq!(events[0].fields.get("lookup_tag"), Some(&json!("owner-b")));
        assert!(!Arc::ptr_eq(prepared, other.prepared_fields.get().unwrap()));
        let (again, _) = prepare_lookup_case(&state, a.identity(), original.clone());
        assert!(Arc::ptr_eq(prepared, again.prepared_fields.get().unwrap()));
        let fields = Arc::clone(prepared);
        let threaded = std::thread::spawn(move || {
            let mut event = lookup_event();
            sources::apply_derived(&mut event, &fields);
            event.fields.get("lookup_tag").cloned()
        })
        .join()
        .unwrap();
        assert_eq!(threaded, Some(json!("owner-a")));
        let changed = lookup_snapshot(&directory, "a", "owner-a-new", true);
        let (latest, events) = prepare_lookup_case(&state, changed.identity(), original);
        assert_eq!(
            events[0].fields.get("lookup_tag"),
            Some(&json!("owner-a-new"))
        );
        assert!(!Arc::ptr_eq(
            prepared,
            latest.prepared_fields.get().unwrap()
        ));
        assert!(first.validate(&state).is_err());
        assert_eq!(
            prepared[0]
                .lookup
                .as_ref()
                .unwrap()
                .evaluate(&lookup_event())
                .unwrap(),
            Some(json!("owner-a"))
        );
        let cache = PREPARED_FIELDS.lock();
        assert!(cache.len() <= PREPARED_CACHE_ENTRIES);
        assert!(cache.iter().map(|entry| entry.budget).sum::<usize>() <= PREPARED_CACHE_BYTES);
    }

    #[test]
    fn reference_capture_has_no_io_and_missing_foreign_or_cancelled_preparation_fails() {
        let directory = Directory::new();
        let missing = lookup_snapshot(&directory, "a", "not-installed", false);
        let state = state(SourceData::None);
        let original = lookup_event();
        let (admitted, events) = capture_case(
            &state,
            Some(missing.identity()),
            None,
            Some(vec![original.clone()]),
            None,
        )
        .unwrap();
        assert!(admitted.prepared_fields.get().is_none());
        assert!(admitted.derived[0]
            .lookup
            .as_ref()
            .unwrap()
            .version()
            .is_none());
        assert!(admitted.prepare_visibility(events).is_err());
        assert!(admitted.prepared_fields.get().is_none());
        assert!(admitted.visibility.get().is_none());
        assert!(original.fields.get("lookup_tag").is_none());
        let installed = lookup_snapshot(&directory, "a", "installed", true);
        let foreign = analysis_context::update(
            &directory.snapshot("b").identity(),
            installed.config.clone(),
        )
        .unwrap();
        let (foreign, events) = capture_case(
            &state,
            Some(foreign.identity()),
            None,
            Some(vec![original.clone()]),
            None,
        )
        .unwrap();
        assert!(foreign.prepare_visibility(events).is_err());
        let (cancelled, events) = capture_case(
            &state,
            Some(installed.identity()),
            None,
            Some(vec![original]),
            None,
        )
        .unwrap();
        let id = format!("reference-cancel-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            assert!(crate::operations::cancel_id(&id));
            cancelled.prepare_visibility(events)
        });
        assert!(result.is_err());
        assert!(cancelled.prepared_fields.get().is_none());
        assert!(cancelled.visibility.get().is_none());
    }

    #[test]
    fn verified_reference_cache_survives_backing_loss_but_new_descriptors_do_not_reuse_it() {
        let directory = Directory::new();
        let initial = lookup_snapshot(&directory, "a", "pinned", true);
        let state = state(SourceData::None);
        let (first, _) = prepare_lookup_case(&state, initial.identity(), lookup_event());
        std::fs::remove_dir_all(directory.path.path().join("references-v1")).unwrap();
        let (again, events) = prepare_lookup_case(&state, initial.identity(), lookup_event());
        assert_eq!(events[0].fields.get("lookup_tag"), Some(&json!("pinned")));
        assert!(Arc::ptr_eq(
            first.prepared_fields.get().unwrap(),
            again.prepared_fields.get().unwrap()
        ));
        let changed = lookup_snapshot(&directory, "a", "missing-new-version", false);
        let (admitted, events) = capture_case(
            &state,
            Some(changed.identity()),
            None,
            Some(vec![lookup_event()]),
            None,
        )
        .unwrap();
        assert!(admitted.prepare_visibility(events).is_err());
        assert!(admitted.prepared_fields.get().is_none());
        assert_eq!(
            first.prepared_fields.get().unwrap()[0]
                .lookup
                .as_ref()
                .unwrap()
                .evaluate(&lookup_event())
                .unwrap(),
            Some(json!("pinned"))
        );
    }
    #[test]
    fn candidate_reference_preflight_pins_projection_without_publishing_config() {
        let directory = Directory::new();
        let saved = lookup_snapshot(&directory, "a", "ready", true);
        let owner = crate::reference_store::Owner {
            case_id: saved.case_id.clone(),
            analysis_id: saved.analysis_id.clone(),
        };
        let mut candidate = saved.config.clone();
        candidate.derived_fields[0]["name"] = json!("new_lookup_tag");
        let prepared =
            prepare_candidate_config(directory.path.path(), &owner, &candidate, &|| false).unwrap();
        assert_eq!(prepared[0].name, "new_lookup_tag");
        assert_eq!(
            prepared[0]
                .lookup
                .as_ref()
                .unwrap()
                .version()
                .unwrap()
                .owner,
            owner
        );
        assert_eq!(directory.snapshot("a"), saved);
        let mut missing = candidate.clone();
        missing.references[0].content_sha256 = "f".repeat(64);
        assert!(
            matches!(prepare_candidate_config_detailed(directory.path.path(), &owner, &missing, &|| false),
            Err(crate::reference_lookup::PreparationError::Unavailable { reference_id }) if reference_id == "shared-reference-id")
        );
        assert!(
            prepare_candidate_config(directory.path.path(), &owner, &candidate, &|| true).is_err()
        );
        assert_eq!(directory.snapshot("a"), saved);
        let published = analysis_context::update(&saved.identity(), candidate).unwrap();
        assert_eq!(published.config_revision, saved.config_revision + 1);
        assert_eq!(
            prepared[0]
                .lookup
                .as_ref()
                .unwrap()
                .evaluate(&lookup_event())
                .unwrap(),
            Some(json!("ready"))
        );
    }
    #[test]
    fn portable_reference_preflight_disables_diagnosed_dependencies_and_preserves_healthy_fields() {
        let directory = Directory::new();
        let mut snapshot = lookup_snapshot(&directory, "a", "healthy", true);
        let healthy = snapshot.config.derived_fields[0].clone();
        let mut missing = snapshot.config.references[0].clone();
        missing.id = "unavailable-reference".into();
        missing.content_sha256 = "e".repeat(64);
        snapshot.config.references.push(missing);
        let lookup = |name: &str, reference: &str, source: &str| {
            json!({"name":name,"lookup":{"schemaVersion":1,
            "referenceId":reference,"keys":[{"referenceColumn":"key","sourceField":source}],"valueColumn":"value"}})
        };
        snapshot.config.derived_fields = vec![
            lookup("disabled_lookup", "unavailable-reference", "lookup_key"),
            lookup("dependent_lookup", "shared-reference-id", "disabled_lookup"),
            json!({"name":"dependent_extract","source":"dependent_lookup","pattern":"(.*)"}),
            healthy,
            json!({"name":"raw_copy","source":"message","pattern":"(.*)"}),
        ];
        assert!(
            matches!(prepare_portable_snapshot(directory.path.path(),&snapshot,&||false),
            Err(crate::reference_lookup::PreparationError::Unavailable { reference_id }) if reference_id == "unavailable-reference")
        );
        snapshot
            .migration_diagnostics
            .push(analysis_context::Diagnostic {
                definition_index: Some(0),
                code: "unavailable_reference".into(),
                message: "Reference bytes absent in imported evidence.".into(),
            });
        let original = snapshot.clone();
        let fields =
            prepare_portable_snapshot(directory.path.path(), &snapshot, &|| false).unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["lookup_tag", "raw_copy"]
        );
        let mut event = lookup_event();
        event.fields.insert("disabled_lookup".into(), json!(1));
        sources::apply_derived(&mut event, &fields);
        assert_eq!(event.fields.get("lookup_tag"), Some(&json!("healthy")));
        assert_eq!(event.fields.get("raw_copy"), Some(&json!("raw message")));
        assert_eq!(event.fields.get("disabled_lookup"), Some(&json!(1)));
        assert!(!event.fields.contains_key("dependent_lookup"));
        assert!(!event.fields.contains_key("dependent_extract"));
        assert_eq!(snapshot, original);
    }

    #[test]
    fn portable_reference_preflight_preserves_typed_projection_budget_failure() {
        use sha2::Digest;
        let directory = Directory::new();
        let mut snapshot = directory.snapshot("a");
        let value = "x".repeat(500 << 10);
        let data = (0..17)
            .map(|key| json!({"key":key,"value":value}).to_string() + "\n")
            .collect::<String>()
            .into_bytes();
        let descriptor = analysis_context::ReferenceDescriptor {
            schema_version: 1,
            id: "large".into(),
            name: "Large fixture".into(),
            content_sha256: format!("{:x}", sha2::Sha256::digest(&data)),
            format: "jsonl".into(),
            columns: vec!["key".into(), "value".into()],
            key_columns: vec!["key".into()],
            duplicate_policy: "reject".into(),
        };
        let owner = crate::reference_store::Owner {
            case_id: snapshot.case_id.clone(),
            analysis_id: snapshot.analysis_id.clone(),
        };
        crate::reference_store::prepare_jsonl(
            directory.path.path(),
            &owner,
            &descriptor,
            &data[..],
            Default::default(),
            &|| false,
        )
        .unwrap();
        snapshot.config.references = vec![descriptor];
        snapshot.config.derived_fields = vec![
            json!({"name":"large_lookup","lookup":{"schemaVersion":1,"referenceId":"large",
            "keys":[{"referenceColumn":"key","sourceField":"lookup_key"}],"valueColumn":"value"}}),
        ];
        assert!(matches!(
            prepare_portable_snapshot(directory.path.path(), &snapshot, &|| false),
            Err(crate::reference_lookup::PreparationError::RetainedBytes)
        ));
        assert_eq!(
            directory.snapshot("a").config,
            analysis_context::Config::default()
        );
    }
}
