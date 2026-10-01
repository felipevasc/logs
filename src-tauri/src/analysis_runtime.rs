//! Immutable, Case-owned query admission. Capture happens before blocking work
//! is queued. Workers never resolve an active Case or a mutable derived registry.
use crate::{
    analysis_context::{self, Identity, Snapshot},
    model::Event,
    sources, AppState, SourceData,
};
use parking_lot::RwLockReadGuard;
use std::{cell::RefCell, ops::Deref, sync::Arc};

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
    pub diagnostics: Arc<Vec<analysis_context::Diagnostic>>,
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

const REGEX_BYTES: usize = 16 << 20;
const DFA_BYTES: usize = 4 << 20;
const MAX_RULES: usize = 1024;
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

fn compile_uncached(snapshot: &Snapshot) -> Result<Vec<sources::CompiledDerived>, String> {
    // A malformed legacy definition is disabled, while original logs remain
    // queryable. A diagnosed global dependency error makes no ordering safe.
    if snapshot.migration_diagnostics.iter().any(|issue| {
        issue.definition_index.is_none() && issue.code == "invalid_legacy_dependencies"
    }) {
        return Ok(Vec::new());
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
    let safe: Vec<_> = definitions
        .iter()
        .enumerate()
        .filter(|(index, _)| !disabled.contains(index))
        .map(|(_, value)| value.clone())
        .collect();
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
    if rule_count > MAX_RULES {
        return Err(format!(
            "A configuração excede o limite de {MAX_RULES} regras derivadas por Caso."
        ));
    }
    let regex_limit = (REGEX_BYTES / rule_count.max(1)).min(2 << 20);
    let dfa_limit = (DFA_BYTES / rule_count.max(1)).min(512 << 10);
    analysis_context::definition_order(&safe)?.into_iter().map(|index| {
        let definition: sources::DerivedFieldCompat = serde_json::from_value(safe[index].clone())
            .map_err(|_| "Campo derivado inválido no Caso.".to_string())?;
        let definition = definition.normalize();
        let rules = definition.rules.into_iter().map(|rule| {
            regex::RegexBuilder::new(&rule.pattern).size_limit(regex_limit).dfa_size_limit(dfa_limit).build()
                .map(|re| sources::CompiledRule { re, template: rule.template, filter: rule.filter })
                .map_err(|_| "Expressão regular inválida ou excede o orçamento total de compilação do Caso.".to_string())
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(sources::CompiledDerived { name: definition.name, source: definition.source, rules, steps: definition.steps })
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
    let (derived, diagnostics) = match &identity {
        Some(identity) => {
            let snapshot = validate_identity(identity)?;
            // Membership is not inferred from a revision. Until a visibility
            // payload is installed by the visibility consumer, revisions that
            // might contain exclusions are refused, including restored batches.
            if snapshot.visibility_revision != 0 {
                return Err(
                    "A visibilidade deste Caso precisa ser validada antes da consulta.".into(),
                );
            }
            compile(&snapshot)?
        }
        None => {
            if analysis_context::active_snapshot()?.is_some() {
                return Err(
                    "Informe a identidade do Caso ativo para executar esta operação.".into(),
                );
            }
            (Arc::new(state.derived.read().clone()), Arc::new(Vec::new()))
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
    let source = captured.map(|mut source| {
        if let SourceData::Memory(events) = Arc::get_mut(&mut source).expect("new snapshot") {
            for event in events {
                sources::apply_derived(event, &derived);
            }
        }
        source
    });
    Ok(Arc::new(Admitted {
        identity,
        source_generation: generation,
        case_key: None,
        mode,
        source,
        names,
        derived,
        diagnostics,
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
    let mut admitted = capture(
        state,
        identity,
        generation,
        if case { Mode::Case } else { Mode::Dataset },
    )?;
    Arc::get_mut(&mut admitted).expect("new admission").case_key = key.clone();
    let events =
        crate::case_cache::take_for(events, key, admitted.identity.as_ref())?.map(|mut events| {
            for event in &mut events {
                sources::apply_derived(event, &admitted.derived);
            }
            events
        });
    Ok((admitted, events))
}

impl Admitted {
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
    match current().and_then(|admitted| admitted.source.clone()) {
        Some(source) => SourceView::Captured(source),
        None if current().is_some_and(|admitted| admitted.mode == Mode::Case) => {
            SourceView::Captured(Arc::new(SourceData::None))
        }
        None => SourceView::Legacy(state.source.read()),
    }
}
pub(crate) fn derived(state: &AppState) -> Arc<Vec<sources::CompiledDerived>> {
    current()
        .map(|admitted| admitted.derived.clone())
        .unwrap_or_else(|| Arc::new(state.derived.read().clone()))
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
        assert_eq!(a_events.as_ref().unwrap()[0].fields["tag"], "A");
        let (_, b_events) = capture_case(&state, Some(b.identity()), None, a_events, None).unwrap();
        assert_eq!(b_events.as_ref().unwrap()[0].fields["tag"], "B");
        let empty = analysis_context::update(&b.identity(), Default::default()).unwrap();
        let (_, empty_events) =
            capture_case(&state, Some(empty.identity()), None, b_events, None).unwrap();
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
        assert!(
            capture_case(&state, Some(changed.identity()), None, Some(vec![]), None).is_err(),
            "unknown visibility must never silently fall back to all records"
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
        let result = with(Some(admitted), || {
            crate::workspace::grouped_timeline_impl(&state, &[], events.as_deref(), &spec)
        })
        .unwrap();
        assert_eq!(result.context, context);
        assert_eq!(result.series[0].key, "A");
        assert_eq!(result.total.count, 1);
    }
}
