//! Immutable, Case-owned query admission. A lightweight publication pin precedes
//! the queue; heavy capture runs under global admission on a blocking worker.
//! Workers never resolve an active Case or a mutable derived registry.
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
    Metadata,
    EditSource,
}

pub(crate) struct Admitted {
    pending_operation: parking_lot::Mutex<Option<crate::operations::Cancellation>>,
    pub identity: Option<Identity>,
    pub interpretation: Arc<crate::case_interpretation::Settings>,
    pub resource_policy: Arc<crate::case_resources::Policy>,
    timestamps: OnceLock<Result<std::collections::BTreeMap<String, sources::CompiledTsConfig>, String>>,
    pub source_generation: Option<u64>,
    pub case_key: Option<String>,
    pub case_content_token: Option<String>,
    case_cache_bound: bool,
    case_records: Option<Arc<crate::case_cache::Records>>,
    case_budget: Arc<crate::case_work_budget::Pool>,
    case_work: parking_lot::Mutex<Option<crate::case_work_budget::Lease>>,
    resource_work: parking_lot::Mutex<Vec<crate::case_work_budget::Lease>>,
    preparing_visibility: std::sync::atomic::AtomicBool,
    mode: Mode,
    source: Option<Arc<SourceData>>,
    names: Vec<String>,
    derived: Arc<Vec<sources::CompiledDerived>>,
    prepared_fields: OnceLock<Arc<Vec<sources::CompiledDerived>>>,
    data_root: std::path::PathBuf,
    definition_bytes: usize,
    pub diagnostics: Arc<Vec<analysis_context::Diagnostic>>,
    pub references: Arc<Vec<analysis_context::ReferenceDescriptor>>,
    _reference_credit: Option<Arc<crate::case_work_budget::Lease>>,
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
    /// Only Dataset admission exposes an exact positional active-union mask.
    indexed_mask: Option<Arc<crate::analysis_visibility::Mask>>,
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
            indexed_mask: None,
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
    pub(crate) fn indexed_mask(&self) -> Option<&crate::analysis_visibility::Mask> {
        self.indexed_mask.as_deref()
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

pub(crate) enum Catalog<'a> {
    Captured(Arc<Admitted>, bool),
    Legacy(RwLockReadGuard<'a, crate::model::CodesConfig>),
}
impl Deref for Catalog<'_> {
    type Target = crate::model::CodesConfig;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Captured(admitted, false) => &admitted.interpretation.codes,
            Self::Captured(admitted, true) => &admitted.interpretation.system_codes,
            Self::Legacy(value) => value,
        }
    }
}
pub(crate) fn codes(state: &AppState) -> Catalog<'_> {
    current().map(|a| Catalog::Captured(a, false)).unwrap_or_else(|| Catalog::Legacy(state.codes.read()))
}
pub(crate) fn system_codes(state: &AppState) -> Catalog<'_> {
    current().map(|a| Catalog::Captured(a, true)).unwrap_or_else(|| Catalog::Legacy(state.system_codes.read()))
}

/// Own the exact decoded context and its read credit together. The tree drops
/// before its credit, including comparisons that discard the return value.
pub(crate) struct LeasedSnapshot {
    snapshot: Snapshot,
    credit: Option<crate::case_work_budget::Lease>,
}
impl Deref for LeasedSnapshot {
    type Target = Snapshot;
    fn deref(&self) -> &Snapshot { &self.snapshot }
}
impl LeasedSnapshot {
    fn into_admission(self) -> (Arc<crate::case_interpretation::Settings>, Arc<Vec<analysis_context::ReferenceDescriptor>>, Option<Arc<crate::case_work_budget::Lease>>) {
        let Self { mut snapshot, credit } = self;
        let settings = snapshot.interpretation.take().unwrap_or_default();
        let references = std::mem::take(&mut snapshot.config.references);
        (Arc::new(settings), Arc::new(references), credit.map(Arc::new))
    }
    #[cfg(test)]
    fn into_references(self) -> (Arc<Vec<analysis_context::ReferenceDescriptor>>, Option<Arc<crate::case_work_budget::Lease>>) {
        let Self { mut snapshot, credit } = self;
        let references = std::mem::take(&mut snapshot.config.references);
        drop(snapshot);
        // The references are moved from the charged tree. Conservatively keep
        // that same credit until every admission/cache owner releases them.
        if references.is_empty() { drop(credit); return (Arc::new(references), None); }
        (Arc::new(references), credit.map(Arc::new))
    }
}
pub(crate) fn validate_identity(identity: &Identity) -> Result<LeasedSnapshot, String> {
    let conn = crate::case_store::context_connection(&crate::config_dir())?;
    let (snapshot, credit) = analysis_context::read_snapshot_leased(&conn, &identity.case_id)?;
    let leased = LeasedSnapshot { snapshot, credit };
    if leased.identity() != *identity { return Err(STALE.into()); }
    Ok(leased)
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
// Serialize cold construction separately from ready-cache access. This keeps
// count/stats/facet misses from duplicating the same bounded regex programs.
static COMPILING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
#[cfg(test)]
thread_local! {
    static BEFORE_COMPILE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

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
    crate::operations::check()?;
    let identity = snapshot.identity();
    {
        let mut cache = COMPILED.lock();
        if let Some(index) = cache.iter().position(|entry| entry.identity == identity) {
            let entry = cache.remove(index);
            let result = (Arc::clone(&entry.fields), Arc::clone(&entry.diagnostics));
            cache.push(entry);
            return Ok(result);
        }
    }
    // A cold Case must not block an unrelated ready cache hit. Only misses
    // wait for the bounded constructor; recheck after a preceding build.
    let compiling = loop {
        crate::operations::check()?;
        if let Some(guard) = COMPILING.try_lock() { break guard; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    {
        let mut cache = COMPILED.lock();
        if let Some(index) = cache.iter().position(|entry| entry.identity == identity) {
            let entry = cache.remove(index);
            let result = (Arc::clone(&entry.fields), Arc::clone(&entry.diagnostics));
            cache.push(entry);
            return Ok(result);
        }
    }
    let mut diagnostics = snapshot.migration_diagnostics.clone();
    let compiled = compile_uncached(snapshot);
    // Cancellation is not an invalid legacy definition and must not publish
    // a successful empty configuration. Ordinary capture still precedes worker
    // registration; this checkpoint serves callers with an existing token.
    crate::operations::check()?;
    let fields = Arc::new(match compiled {
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
    let mut retired = Vec::new();
    let winner = {
        let mut cache = COMPILED.lock();
        crate::operations::check()?;
        if let Some(index) = cache.iter().position(|entry| entry.identity == identity) {
            let entry = cache.remove(index);
            let result = (Arc::clone(&entry.fields), Arc::clone(&entry.diagnostics));
            cache.push(entry);
            Some(result)
        } else {
            while !cache.is_empty()
                && (cache.len() >= 4
                    || cache.iter().map(|entry| entry.budget).sum::<usize>()
                        .saturating_add(budget) > COMPILED_CACHE_BYTES)
            {
                retired.push(cache.remove(0));
            }
            cache.push(CompiledEntry {
                identity,
                fields: Arc::clone(&fields),
                diagnostics: Arc::clone(&diagnostics),
                budget,
            });
            None
        }
    };
    // Final regex/metadata destruction happens after both locks are released.
    drop(compiling);
    drop(retired);
    if let Some(winner) = winner { return Ok(winner); }
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
        interpretation: None,
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
        interpretation: None,
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
    let fields = match &snapshot.interpretation {
        Some(settings) => crate::case_security::compiling(&settings.security, || compile_uncached(snapshot)),
        None => compile_uncached(snapshot),
    }.map_err(PreparationError::Invalid)?;
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
    #[cfg(test)]
    if let Some(hook) = BEFORE_COMPILE.with(|hook| hook.borrow_mut().take()) { hook(); }
    crate::operations::check()?;
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
        crate::operations::check()?;
        let definition: sources::DerivedFieldCompat = serde_json::from_value(safe[index].clone())
            .map_err(|_| "Campo derivado inválido no Caso.".to_string())?;
        let definition = definition.normalize();
        let rules = definition.rules.into_iter().map(|rule| {
            crate::operations::check()?;
            let compiled = regex::RegexBuilder::new(&rule.pattern).size_limit(regex_limit).dfa_size_limit(dfa_limit).build();
            crate::operations::check()?;
            compiled
                .map_err(|_| "Expressão regular inválida ou excede o orçamento total de compilação do Caso.".to_string())
                .and_then(|re| sources::CompiledRule::new(re, rule.template, rule.filter))
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(sources::CompiledDerived { name: definition.name, source: definition.source, rules, steps: definition.steps,
            lookup: definition.lookup.map(crate::reference_lookup::Compiled::new) })
    }).collect()
}

#[cfg(test)]
mod compiled_cache_lifetime_tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            schema_version: 1,
            case_id: uuid::Uuid::new_v4().to_string(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
            config: analysis_context::Config::default(),
            interpretation: None,
            migration_diagnostics: Vec::new(),
            legacy_raw: None,
        }
    }

    #[test]
    fn decoded_snapshot_credit_follows_compilation_and_retained_reference_owners() {
        use crate::case_work_budget::{Limits, Pool};
        let pool = Pool::new(Limits { materialized: 1 << 20, retained: 1 << 20, live: 1 << 20 });
        let mut source = snapshot();
        source.config.references.push(analysis_context::ReferenceDescriptor {
            interpretation_version: 1,
            schema_version: 1, id: "reference".into(), name: "Reference".into(),
            content_sha256: "a".repeat(64), format: "jsonl".into(),
            columns: vec!["key".into()], key_columns: vec!["key".into()], duplicate_policy: "reject".into(),
        });
        let leased = LeasedSnapshot { snapshot: source, credit: Some(pool.reserve(8192).unwrap()) };
        compile(&leased).unwrap();
        assert_eq!(pool.used(), 8192);
        let (references, credit) = leased.into_references();
        assert_eq!(references.len(), 1); assert_eq!(pool.used(), 8192);
        let retained = credit.as_ref().map(Arc::clone);
        drop(references); drop(credit); assert_eq!(pool.used(), 8192);
        drop(retained); assert_eq!(pool.used(), 0);
        let empty = LeasedSnapshot { snapshot: snapshot(), credit: Some(pool.reserve(8192).unwrap()) };
        let (references, credit) = empty.into_references();
        assert!(references.is_empty()); assert!(credit.is_none()); assert_eq!(pool.used(), 0);
    }

    #[test]
    fn cold_compile_releases_global_lock_for_an_unrelated_ready_hit() {
        let warm = snapshot();
        let first = compile(&warm).unwrap().0;
        let cold = snapshot();
        let (entered, observed) = std::sync::mpsc::sync_channel(1);
        let (release, waiting) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            BEFORE_COMPILE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                entered.send(()).unwrap();
                waiting.recv().unwrap();
            })));
            compile(&cold).unwrap()
        });
        observed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        // Do not let a regression deadlock the test: release the cold worker
        // before asserting when the old lock scope is observed.
        let unlocked = COMPILED.try_lock().is_some();
        let hit = unlocked.then(|| compile(&warm).unwrap().0);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(unlocked, "regex construction must not hold the shared cache lock");
        assert!(Arc::ptr_eq(&first, &hit.unwrap()));
    }

    #[test]
    fn concurrent_misses_reuse_one_cold_build_and_owned_result() {
        let snapshot = snapshot();
        let identity = snapshot.identity();
        let (entered, observed) = std::sync::mpsc::channel();
        let builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let workers: Vec<_> = (0..2).map(|_| {
            let snapshot = snapshot.clone();
            let entered = entered.clone();
            let builds = Arc::clone(&builds);
            let (release, waiting) = std::sync::mpsc::sync_channel(1);
            let worker = std::thread::spawn(move || {
                BEFORE_COMPILE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                    builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    entered.send(()).unwrap();
                    waiting.recv().unwrap();
                })));
                compile(&snapshot).unwrap()
            });
            (worker, release)
        }).collect();
        drop(entered);
        let started = observed.recv_timeout(std::time::Duration::from_secs(5)).is_ok();
        for (_, release) in &workers { let _ = release.send(()); }
        let results: Vec<_> = workers.into_iter().map(|(worker, _)| worker.join().unwrap()).collect();
        assert!(started);
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&results[0].0, &results[1].0));
        assert!(Arc::ptr_eq(&results[0].1, &results[1].1));
        assert_eq!(COMPILED.lock().iter().filter(|entry| entry.identity == identity).count(), 1);
    }

    #[test]
    fn cancelled_compile_is_not_a_cached_disabled_configuration() {
        let snapshot = snapshot();
        let identity = snapshot.identity();
        let id = format!("compile-case-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            BEFORE_COMPILE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
                assert!(crate::operations::cancel_id(&id));
            })));
            assert!(compile(&snapshot).is_err(), "cancellation must not become disabled definitions");
        });
        assert!(result.is_err());
        assert!(!COMPILED.lock().iter().any(|entry| entry.identity == identity));
        assert!(compile(&snapshot).unwrap().1.is_empty(), "an uncancelled retry remains valid");
    }
}

pub(crate) fn same_owner(a: Option<&Identity>, b: Option<&Identity>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.case_id == b.case_id && a.analysis_id == b.analysis_id,
        _ => false,
    }
}

/// Only bounded identities are retained while a request waits. In particular,
/// this pin never clones memory events, decodes a Case config, or compiles rules.
#[derive(Clone)]
pub(crate) struct CapturePin {
    identity: Option<Identity>,
    receipt: Option<crate::source_publication::Receipt>,
    mode: Mode,
    case_publication: Option<(String, String)>,
}
impl CapturePin {
    pub(crate) fn new(state: &AppState, identity: Option<Identity>, expected_generation: Option<u64>, mode: Mode, cache_key: Option<&str>) -> Result<Self, String> {
        if matches!(mode, Mode::Metadata | Mode::EditSource) && identity.is_none() {
            return Err("Informe a identidade do Caso para consultar ou editar suas configurações.".into());
        }
        let receipt = if matches!(mode, Mode::Case | Mode::Metadata) { None } else {
            let receipt = crate::source_publication::pin_receipt(state);
            if expected_generation.is_some_and(|expected| expected != receipt.generation)
                || (mode == Mode::Dataset && !same_owner(identity.as_ref(), receipt.analysis_context.as_ref())) {
                return Err(STALE.into());
            }
            Some(receipt)
        };
        let case_publication = cache_key.map(|key| {
            crate::case_cache::pin_publication(key, identity.as_ref()).map(|token| (key.to_owned(), token))
        }).transpose()?;
        Ok(Self { identity, receipt, mode, case_publication })
    }
    pub(crate) fn for_case(state: &AppState, identity: Option<Identity>, generation: Option<u64>, inline: bool, key: Option<&str>) -> Result<Self, String> {
        let case = inline || key.is_some();
        if case && identity.is_none() { return Err("Informe a identidade do Caso antes de consultar suas evidências.".into()); }
        Self::new(state, identity, generation, if case { Mode::Case } else { Mode::Dataset }, if inline { None } else { key })
    }
    pub(crate) fn identity(&self) -> Option<Identity> { self.identity.clone() }
    pub(crate) fn generation(&self) -> Option<u64> { self.receipt.as_ref().map(|receipt| receipt.generation) }
    pub(crate) fn validate(&self, state: &AppState) -> Result<(), String> {
        crate::operations::check()?;
        if self.receipt.as_ref().is_some_and(|receipt| *receipt != crate::source_publication::pin_receipt(state)) {
            return Err(STALE.into());
        }
        if let Some((key, expected)) = &self.case_publication {
            if crate::case_cache::pin_publication(key, self.identity.as_ref())? != *expected {
                return Err(crate::case_cache::CHANGED.into());
            }
        }
        Ok(())
    }
    pub(crate) fn capture(&self, state: &AppState) -> Result<Arc<Admitted>, String> {
        self.validate(state)?;
        capture(state, self.identity.clone(), self.generation(), self.mode)
    }
    pub(crate) fn capture_case(&self, state: &AppState, events: Option<Vec<Event>>, key: Option<String>) -> Result<(Arc<Admitted>, Option<Vec<Event>>), String> {
        self.validate(state)?;
        capture_case(state, self.identity.clone(), self.generation(), events, key)
    }
    #[cfg(test)]
    fn capture_case_shared(&self, state: &AppState, events: Option<Vec<Event>>, key: Option<String>) -> Result<(Arc<Admitted>, Option<Arc<crate::case_cache::Records>>), String> {
        self.validate(state)?;
        capture_case_shared(state, self.identity.clone(), self.generation(), events, key)
    }
}

/// Preserve the original cancellation registration across capture and execution.
/// The stored token is the pre-admission token, without a retained scheduler
/// execution or analysis Arc, so it cannot retain a slot or form an Arc cycle.
pub(crate) async fn capture_prepared<T, F>(app: tauri::AppHandle, pin: CapturePin, operation_id: Option<String>, priority: crate::global_scheduler::Priority, prepare: F) -> Result<(Arc<Admitted>, T), String>
where T: Send + 'static, F: FnOnce(&AppState, &CapturePin) -> Result<(Arc<Admitted>, T), String> + Send + 'static {
    use tauri::Manager;
    let token = crate::operations::token(operation_id)?.with_priority(priority);
    let retained = token.clone();
    tauri::async_runtime::spawn_blocking(move || crate::operations::run_with_token(token, || {
        let state = app.state::<AppState>();
        prepare_pinned(state.inner(), &pin, retained, prepare)
    })).await.map_err(|error| error.to_string())??
}

fn prepare_pinned<T>(state: &AppState, pin: &CapturePin, token: crate::operations::Cancellation, prepare: impl FnOnce(&AppState, &CapturePin) -> Result<(Arc<Admitted>, T), String>) -> Result<(Arc<Admitted>, T), String> {
    pin.validate(state)?;
    let (admitted, result) = prepare(state, pin)?;
    pin.validate(state)?;
    admitted.validate(state)?;
    *admitted.pending_operation.lock() = Some(token);
    Ok((admitted, result))
}

pub(crate) async fn capture_async(app: tauri::AppHandle, identity: Option<Identity>, generation: Option<u64>, mode: Mode, operation_id: Option<String>, priority: crate::global_scheduler::Priority) -> Result<Arc<Admitted>, String> {
    use tauri::Manager;
    let pin = CapturePin::new(app.state::<AppState>().inner(), identity, generation, mode, None)?;
    capture_prepared(app, pin, operation_id, priority, |state, pin| Ok((pin.capture(state)?, ()))).await.map(|(admitted, ())| admitted)
}

pub(crate) async fn capture_case_async(app: tauri::AppHandle, identity: Option<Identity>, generation: Option<u64>, events: Option<Vec<Event>>, key: Option<String>, operation_id: Option<String>, priority: crate::global_scheduler::Priority) -> Result<(Arc<Admitted>, Option<Vec<Event>>), String> {
    use tauri::Manager;
    let pin = CapturePin::for_case(app.state::<AppState>().inner(), identity, generation, events.is_some(), key.as_deref())?;
    capture_prepared(app, pin, operation_id, priority, move |state, pin| pin.capture_case(state, events, key)).await
}

pub(crate) fn capture(
    state: &AppState,
    identity: Option<Identity>,
    expected_generation: Option<u64>,
    mode: Mode,
) -> Result<Arc<Admitted>, String> {
    if matches!(mode, Mode::Metadata | Mode::EditSource) && identity.is_none() { return Err("Informe a identidade do Caso para consultar ou editar suas configurações.".into()); }
    // Reverse local-drop order keeps moved descriptor values charged even if
    // a later source admission fails before constructing Admitted.
    let (interpretation, derived, diagnostics, reference_credit, references, definition_bytes) = match &identity {
        Some(identity) => {
            let snapshot = validate_identity(identity)?;
            if !matches!(mode, Mode::Metadata | Mode::EditSource) {
                if let Some(issue) = snapshot.migration_diagnostics.iter().find(|issue| issue.code.starts_with("legacy_interpretation_unavailable")) {
                    return Err(format!("CASE_INTERPRETATION_REPAIR_REQUIRED: {} A reanálise está bloqueada; os registros originais continuam disponíveis pela visualização preservada e pela exportação.", issue.message));
                }
            }
            let (derived, diagnostics) = crate::case_security::compiling(&snapshot.interpretation.as_ref().map(|s| s.security.clone()).unwrap_or_default(), || compile(&snapshot))?;
            let definition_bytes = serde_json::to_vec(&snapshot.config)
                .map_err(|e| e.to_string())?
                .len();
            let (settings, references, credit) = snapshot.into_admission();
            (settings, derived, diagnostics, credit, references, definition_bytes)
        }
        None => {
            if analysis_context::active_snapshot()?.is_some() {
                return Err(
                    "Informe a identidade do Caso ativo para executar esta operação.".into(),
                );
            }
            (
                Arc::new(crate::case_interpretation::Settings { codes: state.codes.read().clone(), system_codes: state.system_codes.read().clone(), ..Default::default() }),
                Arc::new(state.derived.read().clone()),
                Arc::new(Vec::new()),
                None,
                Arc::new(Vec::new()),
                0,
            )
        }
    };
    let mut names = Vec::new();
    let (generation, captured) = if matches!(mode, Mode::Case | Mode::Metadata) {
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
    let resource_policy = crate::case_resources::Policy::capture(identity.as_ref(), &interpretation.resources)?;
    Ok(Arc::new(Admitted {
        pending_operation: parking_lot::Mutex::new(None),
        interpretation,
        resource_policy,
        timestamps: OnceLock::new(),
        identity,
        source_generation: generation,
        case_key: None,
        case_content_token: None,
        case_cache_bound: false,
        case_records: None,
        case_budget: Arc::clone(crate::case_work_budget::global()),
        case_work: parking_lot::Mutex::new(None),
        resource_work: parking_lot::Mutex::new(Vec::new()),
        preparing_visibility: std::sync::atomic::AtomicBool::new(false),
        mode,
        source,
        names,
        derived,
        prepared_fields: OnceLock::new(),
        data_root: crate::config_dir(),
        definition_bytes,
        diagnostics,
        references,
        _reference_credit: reference_credit,
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
    let (admitted, events) = capture_case_shared(state, identity, generation, events, key)?;
    let events = with(Some(Arc::clone(&admitted)), || events.map(|records| {
        let (events, credit) = if !Arc::ptr_eq(records.pool(), admitted.resource_policy.pool()) {
            // Existing retained evidence keeps its original debit, but a new
            // request must allocate its working copy under its pinned policy.
            records.clone_for_work()?
        } else {
            match Arc::try_unwrap(records) {
                Ok(records) => records.into_work(),
                Err(records) => records.clone_for_work()?,
            }
        };
        admitted.retain_case_credit(credit)?;
        Ok::<_, String>(events)
    }).transpose())?;
    Ok((admitted, events))
}

/// Read-only actions retain the synchronized publication without cloning every
/// evidence payload on the command-admission thread.
pub(crate) fn capture_case_shared(
    state: &AppState,
    identity: Option<Identity>,
    generation: Option<u64>,
    events: Option<Vec<Event>>,
    key: Option<String>,
) -> Result<(Arc<Admitted>, Option<Arc<crate::case_cache::Records>>), String> {
    let case = events.is_some() || key.is_some();
    let cache_bound = events.is_none() && key.is_some();
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
    let (events, content_token) = with(Some(Arc::clone(&admitted)), || crate::case_cache::resolve_for_with_token(events, key, admitted.identity.as_ref()))?;
    let captured = Arc::get_mut(&mut admitted).expect("new admission");
    captured.case_content_token = content_token;
    captured.case_cache_bound = cache_bound;
    // Native origin authority and its live payload credit survive request-local
    // overlay clones and cache eviction. Legacy arrays cannot acquire it.
    captured.case_records = events.as_ref().filter(|records| records.native().is_some()).map(Arc::clone);
    if let Some(records) = &events { captured.case_budget = Arc::clone(records.pool()); }
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
    _reference_credit: Option<Arc<crate::case_work_budget::Lease>>,
    fields: Arc<Vec<sources::CompiledDerived>>,
    budget: usize,
}
static PREPARED_FIELDS: parking_lot::Mutex<Vec<PreparedFieldsEntry>> =
    parking_lot::Mutex::new(Vec::new());
static PREPARING_FIELDS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

impl Admitted {
    pub(crate) fn take_operation(&self, operation_id: Option<String>, priority: crate::global_scheduler::Priority) -> Result<crate::operations::Cancellation, String> {
        match self.pending_operation.lock().take() {
            Some(token) => Ok(token.with_priority(priority)),
            None => crate::operations::token(operation_id).map(|token| token.with_priority(priority)),
        }
    }
    pub(crate) fn resource_pool(&self) -> &Arc<crate::case_work_budget::Pool> { self.resource_policy.pool() }
    pub(crate) fn retain_resource_bytes(&self, bytes: usize) -> Result<(), String> {
        let credit = crate::case_cache::reserve_work(self.resource_policy.pool(), bytes)?;
        let mut work = self.resource_work.lock();
        if let Some(existing) = work.last_mut() { existing.merge(credit) }
        else { work.push(credit); Ok(()) }
    }
    pub(crate) fn native_records(&self) -> Option<&Arc<crate::case_cache::Records>> { self.case_records.as_ref() }
    fn case_pool(&self) -> &Arc<crate::case_work_budget::Pool> {
        &self.case_budget
    }
    fn retain_case_credit(&self, credit: crate::case_work_budget::Lease) -> Result<(), String> {
        let mut work = self.case_work.lock();
        match work.as_mut() {
            Some(existing) => existing.merge(credit),
            None => { *work = Some(credit); Ok(()) }
        }
    }
    fn begin_visibility(&self) -> Result<(), String> {
        if self.preparing_visibility.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Err("Esta captura de consulta já foi executada.".into());
        }
        Ok(())
    }
    fn ensure_case_payload_credit(&self, events: &Vec<Event>) -> Result<(), String> {
        let bytes = crate::case_cache::payload_bytes(events)?;
        let mut work = self.case_work.lock();
        match work.as_mut() {
            Some(existing) if existing.bytes() < bytes => {
                let additional = crate::case_cache::reserve_work(existing.pool(), bytes - existing.bytes())?;
                existing.merge(additional)
            },
            Some(_) => Ok(()),
            None => { *work = Some(crate::case_cache::reserve_work(self.case_pool(), bytes)?); Ok(()) }
        }
    }
    fn apply_interpretation(&self, event: &mut Event) -> Result<(), String> {
        let timestamps = self.timestamps.get_or_init(|| self.interpretation.timestamps.iter()
            .map(|(path, config)| config.compile().map(|compiled| (path.clone(), compiled)))
            .collect::<Result<std::collections::BTreeMap<_, _>, String>>()).as_ref().map_err(Clone::clone)?;
        let path = event.col_str("caminho").unwrap_or_default();
        if let Some(config) = timestamps.get(&path) { sources::apply_ts_config_event(event, config); }
        if let Some(info) = self.interpretation.codes.lookup(&event.source, &event.code)
            .or_else(|| self.interpretation.system_codes.lookup(&event.source, &event.code)) {
            let growth = info.name.len().saturating_add(info.description.len()).saturating_sub(event.name.len().saturating_add(event.description.len()));
            if growth > 0 { self.retain_resource_bytes(growth)?; }
            event.name.clone_from(&info.name);
            event.description.clone_from(&info.description);
        }
        Ok(())
    }
    fn apply_case_derived(&self, event: &mut Event, derived: &[sources::CompiledDerived]) -> Result<(), String> {
        if derived.is_empty() && event.derived_originals.is_empty() {
            self.apply_interpretation(event)?;
            sources::apply_derived(event, derived);
            return Ok(());
        }
        let before = crate::query::event_payload_bytes(event);
        // Existing derived evaluation retains at most 2 MiB of outputs per
        // record. Include bounded transform intermediates and worst-case JSON
        // string escaping of an input field; reserve BEFORE that expansion.
        // This explicit one-record allowance is conservative, not measured RSS.
        let scratch_bytes = before.checked_mul(6).and_then(|n| n.checked_add(8 << 20))
            .ok_or(crate::case_work_budget::WORK_BUSY)?;
        let mut scratch = crate::case_cache::reserve_work(self.case_pool(), scratch_bytes)?;
        self.apply_interpretation(event)?;
        sources::apply_derived(event, derived);
        crate::operations::check()?;
        let added = crate::query::event_payload_bytes(event).saturating_sub(before);
        // Transfer the retained growth out of the already admitted scratch;
        // there is no interval in which enlarged rows are uncharged.
        if added > 0 { self.retain_case_credit(scratch.split(added)?)?; }
        Ok(())
    }
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
                        let mut retired = Vec::new();
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
                            retired.push(cache.remove(0));
                        }
                        cache.push(PreparedFieldsEntry {
                            root: self.data_root.clone(),
                            identity: identity.clone(),
                            references: Arc::clone(&self.references),
                            _reference_credit: self._reference_credit.as_ref().map(Arc::clone),
                            fields: Arc::clone(&fields),
                            budget,
                        });
                        drop(cache);
                        drop(retired);
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
    let indexed_mask = Arc::clone(&mask);
    let mut gate = RowGate::new(
        rows,
        visible,
        Some(mask.ignored_members()),
        move |row| !mask.contains_row(row),
        move || lease.validate(),
    )?;
    gate.indexed_mask = Some(indexed_mask);
    Ok(Arc::new(gate))
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
        self.begin_visibility()?;
        self.prepare_visibility_inner(case_events, None)
    }

    /// A single-record action reuses the full publication's visibility mask,
    /// but clones and transforms only its requested record. Mask positions are
    /// original vector positions, never the selected record's numeric ID.
    pub(crate) fn prepare_visibility_record(
        &self, case_events: Option<&[Event]>, id: usize, event_ref: Option<&str>,
    ) -> Result<Option<Vec<Event>>, String> {
        let Some(events) = case_events else { return self.prepare_visibility(None); };
        self.begin_visibility()?;
        if self.mode != Mode::Case { return Err("Registro de evidência fora de uma captura de Caso.".into()); }
        let position = crate::case_cache::record_position(events, id, event_ref)?;
        let bytes = position.map_or(0, |position| crate::query::event_payload_bytes(&events[position]));
        let credit = crate::case_cache::reserve_work(self.case_pool(), bytes)?;
        let selected = position.map(|position| events[position].clone()).into_iter().collect();
        self.retain_case_credit(credit)?;
        self.prepare_visibility_inner(Some(selected), Some((events, position)))
    }

    /// Prepare the full immutable Case mask without cloning or transforming
    /// Event payloads. Native capture reads each original stored envelope using
    /// the member handle admitted by this publication.
    pub(crate) fn prepare_native_case_visibility(&self) -> Result<(), String> {
        if self.mode != Mode::Case || self.case_records.is_none() {
            return Err("CASE_NATIVE_AUTHORITY_REQUIRED".into());
        }
        self.begin_visibility()?;
        let records = self.case_records.as_ref().ok_or(STALE)?;
        let identity = self.identity.as_ref().ok_or(STALE)?;
        let token = crate::operations::current_token();
        let cancelled = || token.cancelled();
        let progress = |_: &str, completed: u64, total: Option<u64>| {
            crate::operations::report_progress(
                "análise", "visibility", "Preparando visibilidade da seleção",
                completed.min(usize::MAX as u64) as usize,
                total.unwrap_or(0).min(usize::MAX as u64) as usize,
                "registros", 0,
            );
        };
        let work = crate::exclusion_store::Work { cancelled: &cancelled, progress: &progress };
        let scope = crate::exclusion_store::Scope::ActiveUnion;
        let gate = evidence_row_gate(EVIDENCE_MASKS.get_or_prepare(
            records.as_slice(), identity, &scope,
            &crate::analysis_visibility::MaskBudget::default(), &work,
            || crate::exclusion_store::visibility(
                &self.data_root, identity, &crate::exclusion_store::Budget::default(), &work,
            ),
        )?)?;
        gate.validate()?;
        crate::operations::check()?;
        self.visibility.set(PreparedVisibility { gate: Some(gate), source: None })
            .map_err(|_| "Captura de visibilidade já publicada.")?;
        self.validate_case_publication()?;
        self.validate_visibility()
    }

    /// Exact native identity from an admitted full-publication mask. No dense
    /// analytical id is ever treated as an original manifest ordinal.
    pub(crate) fn native_case_member(
        &self, row: &crate::page_projection::RowHandle,
    ) -> Result<&crate::case_evidence::MemberHandle, String> {
        let records = self.case_records.as_ref().ok_or("CASE_NATIVE_AUTHORITY_REQUIRED")?;
        let gate = self.visibility.get().and_then(|v| v.gate.as_ref()).ok_or(STALE)?;
        let member = records.original_member(row.id, Some(&row.event_ref))?;
        // original_member proves this is a NativeBuilder-sealed dense position.
        if !gate.allows(row.id)? { return Err(STALE.into()); }
        Ok(member)
    }

    fn prepare_visibility_inner(
        &self, case_events: Option<Vec<Event>>, full_case: Option<(&[Event], Option<usize>)>,
    ) -> Result<Option<Vec<Event>>, String> {
        if self.visibility.get().is_some() {
            return Err("Esta captura de consulta já foi executada.".into());
        }
        crate::operations::check()?;
        if matches!(self.mode, Mode::Metadata | Mode::EditSource) {
            self.visibility.set(PreparedVisibility { gate: None, source: None }).map_err(|_| "Captura já preparada.")?;
            return Ok(None);
        }
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
        if !matches!(self.mode, Mode::Publish | Mode::Metadata) {
            match (&self.mode, self.source.as_deref()) {
                (Mode::Case, _) => {
                    let events = evidence
                        .as_mut()
                        .ok_or("Evidências do Caso ausentes na captura.")?;
                    self.ensure_case_payload_credit(events)?;
                    if let Some(identity) = &self.identity {
                        gate = Some(evidence_row_gate(
                            EVIDENCE_MASKS.get_or_prepare(
                                full_case.map_or(events.as_slice(), |(events, _)| events),
                                identity, &scope, &budget, &work, load,
                            )?,
                        )?);
                    }
                    let mut row = 0;
                    let mut stopped = false;
                    let mut work_error = None;
                    events.retain_mut(|event| {
                        if row % 256 == 0 && cancelled() {
                            stopped = true;
                        }
                        let mask_row = full_case.map_or(row, |(_, position)| position.unwrap_or(usize::MAX));
                        let keep = !stopped && work_error.is_none() && gate.as_ref().is_none_or(|gate| gate.allows_known_row(mask_row));
                        row += 1;
                        if keep {
                            if let Err(error) = self.apply_case_derived(event, &derived) {
                                work_error = Some(error);
                                return false;
                            }
                        }
                        keep
                    });
                    if let Some(error) = work_error { return Err(error); }
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
                    for event in &mut events { self.apply_interpretation(event)?; }
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
            &self.interpretation.codes,
            &self.interpretation.system_codes,
            derived,
        );
    }

    fn validate_case_publication(&self) -> Result<(), String> {
        if self.case_cache_bound {
            crate::case_cache::validate_token(
                self.case_key.as_deref().ok_or(STALE)?,
                self.case_content_token.as_deref().ok_or(STALE)?,
                self.identity.as_ref(),
            )?;
        }
        Ok(())
    }

    pub(crate) fn validate_visibility(&self) -> Result<(), String> {
        self.validate_case_publication()?;
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
            Mode::Publish | Mode::Metadata | Mode::EditSource => {
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

    /// Adoption preserves the context Identity, so a legacy Case request must
    /// also recheck this authority boundary after its work completes.
    pub(crate) fn validate_native_case_authority(&self) -> Result<(), String> {
        if self.mode == Mode::Case && self.case_records.is_none() {
            crate::case_cache::require_legacy_owner(self.identity.as_ref())?;
        }
        Ok(())
    }

    /// Validate once before execution. The immutable snapshot remains valid if
    /// another request publishes a source/config while this query is running.
    pub(crate) fn validate(&self, state: &AppState) -> Result<(), String> {
        self.validate_native_case_authority()?;
        self.validate_case_publication()?;
        if let Some(identity) = &self.identity {
            validate_identity(identity)?;
        } else if analysis_context::active_snapshot()?.is_some() {
            return Err(STALE.into());
        }
        if !matches!(self.mode, Mode::Case | Mode::Metadata) {
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
    let (admitted, events) = capture_case_async(app.clone(),
        Some(analysis_context),
        source_generation,
        case_events,
        case_key,
        operation_id.clone(),
        crate::global_scheduler::Priority::Normal,
    ).await?;
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
        let codes = &self.admitted.interpretation.codes;
        let system = &self.admitted.interpretation.system_codes;
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
                self.admitted.apply_interpretation(event)?;
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
                "{:?}|{:?}|{:?}|{:?}",
                admitted.identity, admitted.source_generation, admitted.case_key, admitted.case_content_token
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
    struct HeldCaptureSlot {
        scheduler: Arc<crate::global_scheduler::Scheduler>,
        release: Option<std::sync::mpsc::Sender<()>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl HeldCaptureSlot {
        fn new() -> Self {
            let scheduler = crate::global_scheduler::Scheduler::new(1);
            let worker_scheduler = Arc::clone(&scheduler);
            let (ready, started) = std::sync::mpsc::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                crate::global_scheduler::with_scheduler(worker_scheduler, || {
                    crate::operations::run_with_token(crate::operations::token(None).unwrap(), || {
                        ready.send(()).unwrap();
                        let _ = wait.recv_timeout(std::time::Duration::from_secs(10));
                    }).unwrap();
                });
            });
            started.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Self { scheduler, release: Some(release), worker: Some(worker) }
        }
        fn wait_queued(&self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !crate::global_scheduler::with_scheduler(Arc::clone(&self.scheduler), crate::global_scheduler::interactive_waiting) {
                assert!(std::time::Instant::now() < deadline, "capture never reached the global queue");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    impl Drop for HeldCaptureSlot {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() { let _ = release.send(()); }
            if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
        }
    }
    fn enqueue_capture<T: Send + 'static>(slot: &HeldCaptureSlot, state: Arc<AppState>, pin: CapturePin, id: String, prepare: impl FnOnce(&AppState, &CapturePin) -> Result<(Arc<Admitted>, T), String> + Send + 'static) -> std::thread::JoinHandle<Result<(Arc<Admitted>, T), String>> {
        let scheduler = Arc::clone(&slot.scheduler);
        let token = crate::operations::token(Some(id)).unwrap().with_priority(crate::global_scheduler::Priority::Interactive);
        let retained = token.clone();
        std::thread::spawn(move || crate::global_scheduler::with_scheduler(scheduler, || {
            crate::operations::run_with_token(token, || prepare_pinned(&state, &pin, retained, prepare)).and_then(|result| result)
        }))
    }
    fn no_compile_hook(compiled: Arc<std::sync::atomic::AtomicBool>) {
        BEFORE_COMPILE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
            compiled.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(crate::global_scheduler::current().is_some(), "capture compiled outside admission");
        })));
    }

    #[test]
    fn queued_memory_capture_waits_for_one_global_slot_without_holding_app_locks() {
        let _directory = Directory::new();
        let mut event = original(); event.raw = "heavy payload ".repeat(100_000);
        let state = Arc::new(state(SourceData::Memory(vec![event])));
        let pin = CapturePin::new(&state, None, None, Mode::Dataset, None).unwrap();
        let slot = HeldCaptureSlot::new();
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&entered);
        let worker = enqueue_capture(&slot, Arc::clone(&state), pin, uuid::Uuid::new_v4().to_string(), move |state, pin| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(crate::global_scheduler::current().is_some());
            Ok((pin.capture(state)?, ()))
        });
        slot.wait_queued();
        assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
        assert!(state.source.try_write().is_some());
        assert!(state.source_names.try_write().is_some());
        assert!(crate::CATALOG_PUBLICATION.try_write().is_some());
        drop(slot);
        let (admitted, ()) = worker.join().unwrap().unwrap();
        let Some(SourceData::Memory(events)) = admitted.source.as_deref() else { panic!("memory capture missing") };
        assert!(events[0].raw.len() > 1_000_000);
    }

    #[test]
    fn cold_case_capture_compiles_only_after_global_admission() {
        let directory = Directory::new();
        let state = Arc::new(state(SourceData::None));
        let identity = directory.snapshot("a").identity();
        COMPILED.lock().retain(|entry| entry.identity != identity);
        let pin = CapturePin::new(&state, Some(identity.clone()), None, Mode::Case, None).unwrap();
        let slot = HeldCaptureSlot::new();
        let compiled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&compiled);
        let worker = enqueue_capture(&slot, state, pin, uuid::Uuid::new_v4().to_string(), move |state, pin| {
            no_compile_hook(seen);
            Ok((pin.capture(state)?, ()))
        });
        slot.wait_queued();
        assert!(!compiled.load(std::sync::atomic::Ordering::SeqCst));
        drop(slot);
        let (admitted, ()) = worker.join().unwrap().unwrap();
        assert_eq!(admitted.identity, Some(identity));
        assert!(compiled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn cancelled_queued_capture_never_compiles_the_case() {
        let directory = Directory::new();
        let state = Arc::new(state(SourceData::None));
        let identity = directory.snapshot("a").identity();
        let pin = CapturePin::new(&state, Some(identity), None, Mode::Case, None).unwrap();
        let slot = HeldCaptureSlot::new();
        let id = uuid::Uuid::new_v4().to_string();
        let compiled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&compiled);
        let worker = enqueue_capture(&slot, state, pin, id.clone(), move |state, pin| {
            no_compile_hook(seen);
            Ok((pin.capture(state)?, ()))
        });
        slot.wait_queued();
        assert!(crate::operations::cancel_id(&id));
        let result = worker.join().unwrap();
        assert!(matches!(result, Err(error) if error == "Operação cancelada."));
        assert!(!compiled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn queued_source_replacement_rejects_before_capturing_the_new_memory_source() {
        let _directory = Directory::new();
        let state = Arc::new(state(SourceData::Memory(vec![original()])));
        let pin = CapturePin::new(&state, None, None, Mode::Dataset, None).unwrap();
        let slot = HeldCaptureSlot::new();
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&entered);
        let worker = enqueue_capture(&slot, Arc::clone(&state), pin, uuid::Uuid::new_v4().to_string(), move |state, pin| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok((pin.capture(state)?, ()))
        });
        slot.wait_queued();
        {
            let mut source = state.source.write();
            let receipt = crate::source_publication::prepare_touch_locked(&state).unwrap();
            let mut replacement = original(); replacement.message = "B".into();
            *source = SourceData::Memory(vec![replacement]);
            crate::source_publication::commit_touch_locked(&state, receipt);
        }
        drop(slot);
        assert!(matches!(worker.join().unwrap(), Err(error) if error == STALE));
        assert!(!entered.load(std::sync::atomic::Ordering::SeqCst), "stale capture must not inspect B");
    }

    #[test]
    fn queued_case_revision_change_rejects_before_compilation() {
        let directory = Directory::new();
        let state = Arc::new(state(SourceData::None));
        let identity = directory.snapshot("a").identity();
        let pin = CapturePin::new(&state, Some(identity.clone()), None, Mode::Case, None).unwrap();
        let slot = HeldCaptureSlot::new();
        let compiled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&compiled);
        let worker = enqueue_capture(&slot, state, pin, uuid::Uuid::new_v4().to_string(), move |state, pin| {
            no_compile_hook(seen);
            Ok((pin.capture(state)?, ()))
        });
        slot.wait_queued();
        analysis_context::update(&identity, config("replacement")).unwrap();
        drop(slot);
        assert!(matches!(worker.join().unwrap(), Err(error) if error == STALE));
        assert!(!compiled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn queued_same_key_case_replacement_never_adopts_the_new_publication() {
        let directory = Directory::new();
        let state = Arc::new(state(SourceData::None));
        let identity = directory.snapshot("a").identity();
        let key = uuid::Uuid::new_v4().to_string();
        tauri::async_runtime::block_on(crate::case_cache::case_sync(key.clone(), vec![original()], Some(identity.clone()))).unwrap();
        let pin = CapturePin::for_case(&state, Some(identity.clone()), None, false, Some(&key)).unwrap();
        let slot = HeldCaptureSlot::new();
        let requested_key = key.clone();
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&entered);
        let worker = enqueue_capture(&slot, Arc::clone(&state), pin, uuid::Uuid::new_v4().to_string(), move |state, pin| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            pin.capture_case_shared(state, None, Some(requested_key))
        });
        slot.wait_queued();
        let mut replacement = original(); replacement.message = "new evidence".into();
        tauri::async_runtime::block_on(crate::case_cache::case_sync(key, vec![replacement], Some(identity))).unwrap();
        drop(slot);
        assert!(matches!(worker.join().unwrap(), Err(error) if error == crate::case_cache::CHANGED));
        assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn capture_and_execution_share_one_cancel_registration_without_retaining_a_slot() {
        let _directory = Directory::new();
        let state = state(SourceData::Memory(vec![original()]));
        let pin = CapturePin::new(&state, None, None, Mode::Dataset, None).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let retained = token.clone();
        let scheduler = crate::global_scheduler::Scheduler::new(1);
        let (admitted, ()) = crate::global_scheduler::with_scheduler(Arc::clone(&scheduler), || {
            crate::operations::run_with_token(token, || prepare_pinned(&state, &pin, retained, |state, pin| Ok((pin.capture(state)?, ())))).unwrap().unwrap()
        });
        let (finished, result) = std::sync::mpsc::channel();
        let other = std::thread::spawn(move || crate::global_scheduler::with_scheduler(scheduler, || {
            crate::operations::run_with_token(crate::operations::token(None).unwrap(), || finished.send(()).unwrap()).unwrap();
        }));
        result.recv_timeout(std::time::Duration::from_secs(5)).expect("captured token retained the global slot");
        other.join().unwrap();
        assert!(crate::operations::token(Some(id.clone())).is_err(), "capture must retain the live registration");
        assert!(crate::operations::cancel_id(&id));
        let continuation = admitted.take_operation(Some(id.clone()), crate::global_scheduler::Priority::Interactive).unwrap();
        assert!(continuation.cancelled(), "cancel between the two blocking stages must survive");
        assert!(crate::operations::run_with_token(continuation, || panic!("cancelled continuation ran")).is_err());
        assert!(crate::operations::token(Some(id)).is_ok(), "completed continuation must release registration");
    }

    #[test]
    fn operation_context_preserves_dynamic_case_policy_fallback_and_explicit_overrides() {
        let directory = Directory::new();
        let state = state(SourceData::None);
        let a = capture(&state, Some(directory.snapshot("a").identity()), None, Mode::Case).unwrap();
        let b = capture(&state, Some(directory.snapshot("b").identity()), None, Mode::Case).unwrap();
        assert!(!Arc::ptr_eq(&a.resource_policy, &b.resource_policy));
        let assert_policy = |expected: &Arc<Admitted>| {
            assert!(Arc::ptr_eq(&crate::case_resources::current().unwrap(), &expected.resource_policy));
        };
        with(Some(Arc::clone(&a)), || {
            // Both registry-created roots and inherited worker tokens must
            // capture only an explicit override, not promote fallback A.
            for token in [crate::operations::token(None).unwrap(), crate::operations::current_token()] {
                crate::operations::with_context(token, || {
                    assert_policy(&a);
                    with(Some(Arc::clone(&b)), || assert_policy(&b));
                    assert_policy(&a);
                });
                assert_policy(&a);
            }
            crate::case_resources::with(Arc::clone(&a.resource_policy), || {
                let token = crate::operations::current_token();
                crate::operations::with_context(token, || {
                    with(Some(Arc::clone(&b)), || assert_policy(&a));
                });
            });
            assert_policy(&a);
        });
        assert!(current().is_none());
        assert!(crate::case_resources::current().is_none());
    }

    #[test]
    fn parallel_lanes_keep_captured_case_configuration_after_active_case_switch() {
        let directory = Directory::new();
        let configured = analysis_context::update(&directory.snapshot("a").identity(), config("owned-by-A")).unwrap();
        let a = crate::case_interpretation::update(&configured.identity(), |settings| {
            settings.codes = serde_json::from_value(json!({"app":{"200":{"name":"Catalog A","description":"A"}}})).unwrap();
            settings.timestamps.insert("same.log".into(), sources::TsConfig { clock_adjustment_ms: 1000, ..Default::default() });
            Ok(())
        }).unwrap();
        crate::case_interpretation::update(&directory.snapshot("b").identity(), |settings| {
            settings.codes = serde_json::from_value(json!({"app":{"200":{"name":"Catalog B","description":"B"}}})).unwrap();
            settings.timestamps.insert("same.log".into(), sources::TsConfig { clock_adjustment_ms: 2000, ..Default::default() });
            Ok(())
        }).unwrap();
        let state = state(SourceData::None);
        let admitted = capture(&state, Some(a.identity()), None, Mode::Case).unwrap();
        let mut cases = crate::case_store::load().unwrap(); cases["active"] = json!("b");
        crate::case_store::save(cases).unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let started = std::sync::atomic::AtomicUsize::new(0);
        let values = pool.install(|| crate::global_scheduler::with_limit(2, || with(Some(admitted), || {
            crate::operations::run_with_token(crate::operations::token(None).unwrap(), || {
                crate::global_scheduler::map(0..2, |_| {
                    started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while started.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                        assert!(std::time::Instant::now() < deadline, "the two admitted Rayon lanes did not run concurrently");
                        std::thread::yield_now();
                    }
                    let captured = current().expect("parallel lane lost its Case owner");
                    assert_eq!(captured.identity.as_ref(), Some(&a.identity()));
                    assert_eq!(captured.interpretation.timestamps["same.log"].clock_adjustment_ms, 1000);
                    assert_eq!(codes(&state).lookup("app", "200").unwrap().name, "Catalog A");
                    let mut event = original(); sources::apply_derived(&mut event, &derived(&state));
                    event.fields["tag"].clone()
                })
            }).unwrap()
        })));
        assert_eq!(values, vec![json!("owned-by-A"), json!("owned-by-A")]);
    }

    #[test]
    fn same_file_case_catalogs_timestamps_formats_and_queued_settings_are_independent() {
        let directory = Directory::new();
        let file = directory.path.path().join("same.log");
        std::fs::write(&file, "1700000000 app 200 alpha beta\n").unwrap();
        let path = file.to_string_lossy().into_owned();
        let configure = |case: &str, name: &str, adjustment: i64, tail: bool| {
            crate::case_interpretation::update(&directory.snapshot(case).identity(), |settings| {
                settings.codes = serde_json::from_value(json!({"app":{"200":{"name":name,"description":name}}})).unwrap();
                settings.timestamps.insert(path.clone(), sources::TsConfig { sources: vec!["linha".into()], regex: Some("^(\\d+)".into()), format: "epoch_s".into(), clock_adjustment_ms: adjustment, ..Default::default() });
                settings.formats.push(crate::CustomFormat { name: "same-format".into(), kind: "regex".into(), pattern: if tail { r"^\d+ (?P<source>\w+) (?P<code>\w+) \w+ (?P<message>.*)$".into() } else { r"^\d+ (?P<source>\w+) (?P<code>\w+) (?P<message>.*)$".into() }, separator: String::new(), fields: vec![] });
                Ok(())
            }).unwrap()
        };
        let a = configure("a", "Catalog A", 1000, false);
        let b = configure("b", "Catalog B", 2000, true);
        let state = state(SourceData::None);
        let first = capture(&state, Some(a.identity()), Some(0), Mode::Publish).unwrap();
        let second = capture(&state, Some(b.identity()), Some(0), Mode::Publish).unwrap();
        let read = |admitted: Arc<Admitted>| with(Some(admitted), || {
            let index = crate::index_source_file(&path, "custom:same-format", None).unwrap();
            sources::event_at(&index, 0, &codes(&state), &system_codes(&state), &[])
        });
        let ea = read(first.clone()); let eb = read(second.clone());
        assert_eq!(ea.event_ref, eb.event_ref, "parser interpretation never changes forensic event identity");
        assert_eq!(ea.name, "Catalog A"); assert_eq!(eb.name, "Catalog B");
        assert_eq!(ea.message, "alpha beta"); assert_eq!(eb.message, "beta");
        assert_eq!(ea.timestamp, Some(1_700_000_001_000)); assert_eq!(eb.timestamp, Some(1_700_000_002_000));
        let changed = crate::case_interpretation::update(&a.identity(), |settings| {
            settings.codes.sources.get_mut("app").unwrap().get_mut("200").unwrap().name = "Catalog A new".into();
            settings.security.detection_settings_json = r#"{"disabled":[],"threats":false}"#.into();
            Ok(())
        }).unwrap();
        assert!(first.validate(&state).is_err(), "queued old edit/query fails admission revalidation");
        assert!(second.validate(&state).is_ok());
        assert_eq!(read(first).name, "Catalog A", "already captured worker cannot observe later settings");
        assert_ne!(changed.interpretation.unwrap().security, second.interpretation.security);
        assert_eq!(read(second).name, "Catalog B");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "1700000000 app 200 alpha beta\n");
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
            interpretation: None,
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
            interpretation: None,
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
        std::fs::write(&path, "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":\"event\"}\n").unwrap();
        let index = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let state = state(SourceData::None);
        let identity = directory.snapshot("a").identity();
        let publishing = capture(&state, Some(identity.clone()), Some(0), Mode::Publish).unwrap();
        with(Some(publishing), || crate::source_publication::publish(&state, index, vec![path.to_string_lossy().into_owned()], vec![], false)).unwrap();
        let queued = capture(&state, Some(identity.clone()), Some(1), Mode::Dataset).unwrap();
        let editor = capture(&state, Some(identity), Some(1), Mode::EditSource).unwrap();
        let receipt = with(Some(editor), || crate::set_ts_config_impl(&state, path.to_str().unwrap(), None, None)).unwrap();
        assert_eq!(receipt.publication.generation, 2);
        assert_eq!(queued.validate(&state).unwrap_err(), STALE);
        let saved = directory.snapshot("a");
        let (lines, order) = {
            let source = state.source.read();
            let SourceData::Indexed(index) = &*source else { panic!() };
            index.ordered(); (index.lines.clone(), index.time_order.clone())
        };
        let conn = crate::case_store::connect(directory.path.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_case_settings BEFORE UPDATE ON case_analysis BEGIN SELECT RAISE(ABORT,'simulated settings failure'); END;").unwrap();
        drop(conn);
        let editor = capture(&state, Some(saved.identity()), Some(2), Mode::EditSource).unwrap();
        assert!(with(Some(editor), || crate::set_ts_config_impl(&state, path.to_str().unwrap(), Some(sources::TsConfig { clock_adjustment_ms: 1000, ..Default::default() }), None)).is_err());
        assert_eq!(crate::source_publication::receipt_locked(&state).generation, 2);
        assert_eq!(directory.snapshot("a"), saved);
        let source = state.source.read();
        let SourceData::Indexed(index) = &*source else { panic!() };
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
        // Full Case details consume only the already admitted visible overlay,
        // never a raw saved-evidence fallback when a member has been excluded.
        with(Some(case_admitted), || {
            assert!(crate::detail_commands::detail_in_scope(
                &state, 0, Some(&originals[0].event_ref), Some(filtered.clone()),
            ).unwrap().is_none());
            let detail = crate::detail_commands::detail_in_scope(
                &state, 6, Some(&originals[6].event_ref), Some(filtered.clone()),
            ).unwrap().unwrap();
            assert_eq!(detail.raw, originals[6].raw);
            assert_eq!(detail.event_ref, originals[6].event_ref);
        });
        let mut reordered = originals.clone();
        reordered.rotate_left(2);
        reordered.iter_mut().find(|event| event.id == 6).unwrap().id = 500;
        for (id, original_position) in [(0, 0), (500, 6)] {
            let (single, shared) = capture_case_shared(&state, Some(directory.snapshot("a").identity()), None,
                Some(reordered.clone()), Some("single-record-mask".into())).unwrap();
            with(Some(single.clone()), || {
                let selected = single.prepare_visibility_record(shared.as_deref().map(crate::case_cache::Records::as_slice), id, Some(&originals[original_position].event_ref)).unwrap().unwrap();
                assert_eq!(selected.len(), usize::from(id == 500), "visibility uses vector positions even when IDs are sparse/reordered");
                let summary = single.visibility_summary().unwrap();
                assert_eq!(summary.total_rows, Some(12), "record actions reuse the complete Case mask domain");
                assert_eq!(summary.excluded_rows, Some(3));
                if id == 500 { assert_eq!(selected[0].raw, originals[6].raw); }
            });
        }
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
                &codes(&state),
                &system_codes(&state),
                &[],
                &|_, _| {},
            )
            .unwrap();
        }
        let filters = vec![crate::query::Filter {
            column: "raw".into(), op: "contains".into(), value: "event ".into(), value2: None,
        }];
        let pfs = crate::query::prepare(&filters);
        let completed_page_hit = |index: &sources::FileIndex| {
            let codes = codes(&state); let system = system_codes(&state);
            let source = crate::engine::Source { idx: index, codes: &codes, system: &system, derived: &[] };
            crate::engine::explain_page(&source, &pfs, "id", "asc", 3, false)
                .unwrap().unwrap()["exactPredicate"].as_bool().unwrap()
        };
        let before = admitted_dataset(&directory, &state);
        let cursor = with(Some(before), || {
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            assert!(!completed_page_hit(index));
            assert_eq!(crate::count_filtered_impl(&state, filters.clone(), None).unwrap(), 9);
            assert!(completed_page_hit(index));
            crate::query::query_page_indexed(
                index,
                &filters,
                "id",
                "asc",
                0,
                3,
                None,
                &codes(&state),
                &system_codes(&state),
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
            assert!(completed_page_hit(index), "fresh admission of the same identity reuses the completed selection");
            let next = crate::query::query_page_indexed(
                index,
                &filters,
                "id",
                "asc",
                3,
                3,
                Some(&cursor),
                &codes(&state),
                &system_codes(&state),
                &[],
            )
            .unwrap();
            assert_eq!(
                next.rows.iter().map(|event| event.id).collect::<Vec<_>>(),
                vec![4, 6, 7]
            );
            assert_eq!(crate::count_filtered_impl(&state, filters.clone(), None).unwrap(), 9);
            assert_eq!(crate::count_filtered_impl(&state, vec![], None).unwrap(), 9);
        });
        restore(&directory, &state, &first.batch_id);
        let restored = admitted_dataset(&directory, &state);
        with(Some(restored), || {
            let view = source(&state);
            let SourceData::Indexed(index) = &*view else {
                panic!()
            };
            assert!(!completed_page_hit(index), "a restored visibility namespace must miss the old nine-row selection");
            let page = crate::query::query_page_indexed(index, &filters, "id", "asc", 0, 100, None, &codes(&state), &system_codes(&state), &[]).unwrap();
            assert_eq!(page.total, Some(10));
            assert_eq!(page.rows.iter().map(|event| event.id).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 6, 7, 8, 9, 10]);
            assert_eq!(crate::count_filtered_impl(&state, filters.clone(), None).unwrap(), 10);
            assert!(completed_page_hit(index));
            assert!(crate::query::query_page_indexed(
                index,
                &filters,
                "id",
                "asc",
                3,
                3,
                Some(&cursor),
                &codes(&state),
                &system_codes(&state),
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
            assert!(!completed_page_hit(index), "the final restore must not reuse either restricted selection");
            let page = crate::query::query_page_indexed(index, &filters, "id", "asc", 0, 100, None, &codes(&state), &system_codes(&state), &[]).unwrap();
            assert_eq!(page.rows.len(), 12);
            assert_eq!(crate::count_filtered_impl(&state, filters.clone(), None).unwrap(), 12);
            assert!(completed_page_hit(index));
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
        let read_page = || {
            crate::exclusion_store::archive_page(
                directory.path.path(),
                &identity,
                &batch.batch_id,
                None,
                10,
                &work,
            )
            .unwrap()
        };
        let page = read_page();
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
        let mut wrong = read_page();
        wrong.analysis = directory.snapshot("b").identity();
        assert!(archive.resolve(&state, &wrong).is_err());
        let mut too_large = read_page();
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
            interpretation_version: 1, schema_version: 1,
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
            interpretation_version: 1, schema_version: 1,
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

#[cfg(test)]
mod case_work_tests {
    use super::*;
    use crate::case_work_budget::{Limits, Pool};

    fn admission(pool: Arc<Pool>) -> Admitted {
        Admitted { pending_operation: parking_lot::Mutex::new(None), timestamps: OnceLock::new(), interpretation: Arc::new(Default::default()), resource_policy: crate::case_resources::Policy::capture(None, &Default::default()).unwrap(), identity: None, source_generation: None, case_key: None, case_content_token: None,
            case_cache_bound: false, case_records: None, case_budget: pool, case_work: parking_lot::Mutex::new(None), resource_work: parking_lot::Mutex::new(Vec::new()),
            preparing_visibility: std::sync::atomic::AtomicBool::new(false), mode: Mode::Case, source: None,
            names: Vec::new(), derived: Arc::new(Vec::new()), prepared_fields: OnceLock::new(), data_root: Default::default(),
            definition_bytes: 0, diagnostics: Arc::new(Vec::new()), references: Arc::new(Vec::new()), _reference_credit: None, visibility: OnceLock::new(),
            failure: parking_lot::Mutex::new(None), source_set: OnceLock::new() }
    }
    fn transform(bytes: usize) -> Vec<sources::CompiledDerived> {
        vec![sources::CompiledDerived { name: "expanded".into(), source: "message".into(), steps: Vec::new(), lookup: None,
            rules: vec![sources::CompiledRule::new(regex::Regex::new("^(.*)$").unwrap(), Some("x".repeat(bytes)), None).unwrap()] }]
    }

    #[test]
    fn native_member_resolution_uses_dense_publication_identity_and_full_mask() {
        use crate::case_evidence::{EvidenceOwner, MemberHandle, StoreIdentity};
        let store = StoreIdentity { store_id: uuid::Uuid::new_v4().to_string(), epoch: uuid::Uuid::new_v4().to_string() };
        let owner = EvidenceOwner { store_id: store.store_id.clone(), case_id: "test-case".into(), analysis_id: uuid::Uuid::new_v4().to_string() };
        let authority = crate::case_cache::NativeAuthority { store, owner, case_evidence_signature: "a".repeat(64), evidence_signature: "b".repeat(64), station_id: None, preserved_count: 2 };
        let guard = crate::case_cache::NativeGuard { validate: Arc::new(|_| Ok(())), lease: Arc::new(()), storage_bytes: 0 };
        let mut builder = crate::case_cache::NativeBuilder::new(authority, guard).unwrap();
        let mut origins = Vec::new();
        for id in 0..2 {
            let mut event = Event::empty(); event.id = 9000 + id; event.event_ref = format!("native:{id}");
            let member = MemberHandle { container_id: uuid::Uuid::new_v4().to_string(), manifest_id: uuid::Uuid::new_v4().to_string(), occurrence_id: uuid::Uuid::new_v4().to_string() };
            let credit = builder.record_admission().reserve(&member, 1024, crate::query::event_payload_bytes(&event) + 256).unwrap();
            origins.push(member.clone());
            builder.push(&crate::case_cache::AnalyticalItemContext { station_id: None, artifact_id: None, origin: None }, member, event, credit).unwrap();
        }
        let records = Arc::new(builder.finish().unwrap());
        let mut admitted = admission(Arc::clone(records.pool()));
        admitted.case_records = Some(Arc::clone(&records));
        assert!(admitted.native_case_member(&crate::page_projection::RowHandle { id: 0, event_ref: "native:0".into() }).is_err());
        let gate = Arc::new(RowGate::new(2, 1, None, |position| position == 0, || Ok(())).unwrap());
        assert!(admitted.visibility.set(PreparedVisibility { gate: Some(gate), source: None }).is_ok());
        let before = records.pool().used();
        assert_eq!(admitted.native_case_member(&crate::page_projection::RowHandle { id: 0, event_ref: "native:0".into() }).unwrap(), &origins[0]);
        for (id, reference) in [(0, "changed"), (1, "native:1"), (9000, "native:0")] {
            assert!(admitted.native_case_member(&crate::page_projection::RowHandle { id, event_ref: reference.into() }).is_err());
        }
        assert_eq!(records.pool().used(), before);
    }

    #[test]
    fn native_overlay_reserves_transient_expansion_before_mutating_a_record() {
        let pool = Pool::new(Limits { materialized: 1 << 20, retained: 1 << 20, live: 1 << 20 });
        let admitted = admission(Arc::clone(&pool));
        let mut events = vec![Event::empty()]; events[0].message = "original".into();
        admitted.ensure_case_payload_credit(&events).unwrap();
        let before = serde_json::to_value(&events[0]).unwrap(); let used = pool.used();
        assert!(admitted.apply_case_derived(&mut events[0], &transform(1024)).unwrap_err().contains("CASE_WORK_BUSY"));
        assert_eq!(serde_json::to_value(&events[0]).unwrap(), before, "failed scratch admission precedes derived mutation");
        assert_eq!(pool.used(), used);
        drop(events); drop(admitted); assert_eq!(pool.used(), 0);
    }

    #[test]
    fn post_overlay_growth_stays_charged_while_one_record_scratch_is_released() {
        let pool = Pool::new(Limits { materialized: 4 << 20, retained: 4 << 20, live: 32 << 20 });
        let admitted = admission(Arc::clone(&pool));
        let mut events = vec![Event::empty()]; events[0].message = "input".into();
        admitted.ensure_case_payload_credit(&events).unwrap();
        let base = pool.used(); let old = crate::query::event_payload_bytes(&events[0]);
        admitted.apply_case_derived(&mut events[0], &transform(32 << 10)).unwrap();
        let growth = crate::query::event_payload_bytes(&events[0]) - old;
        assert!(growth >= 32 << 10);
        assert_eq!(pool.used(), base + growth, "retained output replaces part of the pre-admitted scratch lease");
        assert_eq!(events[0].fields["expanded"].as_str().unwrap().len(), 32 << 10);
        drop(events); drop(admitted); assert_eq!(pool.used(), 0);
    }

    #[test]
    fn aggregate_overlay_growth_cannot_hide_behind_a_per_record_limit() {
        let pool = Pool::new(Limits { materialized: 4 << 20, retained: 4 << 20, live: 9 << 20 });
        let admitted = admission(Arc::clone(&pool));
        let mut events: Vec<_> = (0..10).map(|id| { let mut event = Event::empty(); event.id = id; event.message = "input".into(); event }).collect();
        admitted.ensure_case_payload_credit(&events).unwrap();
        let transforms = transform(400 << 10); let mut expanded = 0; let mut blocked = false;
        for event in &mut events {
            match admitted.apply_case_derived(event, &transforms) {
                Ok(()) => expanded += 1,
                Err(error) => { assert!(error.contains("CASE_WORK_BUSY")); assert!(!event.fields.contains_key("expanded")); blocked = true; break; }
            }
            assert!(pool.used() <= pool.limits().live);
        }
        assert!(expanded > 0 && blocked);
        drop(events); drop(admitted); assert_eq!(pool.used(), 0);
    }
}
