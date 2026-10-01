//! Case configuration mutations have their own CAS boundary. They do not
//! mutate loaded records or prepare another Case's source as a side effect.
use crate::{
    analysis_context::{self, Config, Identity, Snapshot},
    field_transform, sources,
};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutationReceipt {
    pub analysis_context: Snapshot,
}

pub(crate) fn validate_config(config: &Config) -> Result<(), String> {
    analysis_context::validate(config)?;
    for raw in &config.derived_fields {
        let definition: sources::DerivedFieldCompat = serde_json::from_value(raw.clone())
            .map_err(|_| "Definição de campo inválida.".to_string())?;
        for rule in definition.normalize().rules {
            if let Some(filter) = rule.filter {
                crate::workspace::validate(&[filter])?;
            }
        }
    }
    crate::analysis_runtime::validate_compilation(config)
}

/// Keep verified reference projections and their leases alive through CAS.
/// This runs only in an explicit mutation worker, never during request capture.
pub(crate) fn prepare_config(
    expected: &Identity,
    config: &Config,
) -> Result<std::sync::Arc<Vec<sources::CompiledDerived>>, String> {
    crate::analysis_runtime::prepare_candidate_config(
        &crate::config_dir(),
        &crate::reference_store::Owner {
            case_id: expected.case_id.clone(),
            analysis_id: expected.analysis_id.clone(),
        },
        config,
        &|| crate::operations::check().is_err(),
    )
}

pub(crate) fn expected_identity(explicit: Option<Identity>) -> Result<Identity, String> {
    if let Some(identity) = explicit {
        return Ok(identity);
    }
    if let Some(identity) =
        crate::analysis_runtime::current().and_then(|current| current.identity.clone())
    {
        return Ok(identity);
    }
    Err("Informe o contexto do Caso antes de consultar ou configurar campos derivados.".into())
}

fn current_for_edit(expected: &Identity) -> Result<Snapshot, String> {
    let current = analysis_context::snapshot(&expected.case_id)?;
    // Visibility changes do not overwrite a concurrent configuration edit.
    if current.analysis_id != expected.analysis_id
        || current.config_revision != expected.config_revision
    {
        return Err("A configuração do Caso mudou. Reabra o editor antes de salvar.".into());
    }
    Ok(current)
}

pub(crate) fn replace_definition(
    config: &Config,
    name: &str,
    source: &str,
    rules: Vec<sources::DerivedRule>,
    steps: Option<Vec<field_transform::Step>>,
) -> Result<Config, String> {
    let mut config = config.clone();
    let position = config
        .derived_fields
        .iter()
        .position(|value| value.get("name").and_then(Value::as_str) == Some(name));
    let mut definition = position
        .map(|at| config.derived_fields[at].clone())
        .unwrap_or_else(|| json!({"id": uuid::Uuid::new_v4().to_string()}));
    let object = definition
        .as_object_mut()
        .ok_or("Definição existente inválida; revise sua configuração.")?;
    object.insert("name".into(), json!(name));
    object.insert("source".into(), json!(source));
    object.insert(
        "rules".into(),
        serde_json::to_value(rules).map_err(|e| e.to_string())?,
    );
    for legacy in ["pattern", "template", "filter"] {
        object.remove(legacy);
    }
    if let Some(steps) = steps {
        object.insert(
            "steps".into(),
            serde_json::to_value(steps).map_err(|e| e.to_string())?,
        );
    }
    if let Some(at) = position {
        config.derived_fields[at] = definition;
    } else {
        config.derived_fields.push(definition);
    }
    validate_config(&config)?;
    Ok(config)
}

pub(crate) fn save_definition(
    expected: &Identity,
    name: &str,
    source: &str,
    rules: Vec<sources::DerivedRule>,
    steps: Option<Vec<field_transform::Step>>,
) -> Result<MutationReceipt, String> {
    let current = current_for_edit(expected)?;
    let config = replace_definition(&current.config, name, source, rules, steps)?;
    if config == current.config {
        return Ok(MutationReceipt {
            analysis_context: current,
        });
    }
    let _prepared = prepare_config(expected, &config)?;
    crate::operations::check()?;
    let snapshot = analysis_context::update(expected, config)?;
    crate::operations::commit();
    Ok(MutationReceipt {
        analysis_context: snapshot,
    })
}

pub(crate) fn delete_definition(
    expected: &Identity,
    name: &str,
) -> Result<MutationReceipt, String> {
    let current = current_for_edit(expected)?;
    let mut config = current.config;
    let before = config.derived_fields.len();
    config
        .derived_fields
        .retain(|value| value.get("name").and_then(Value::as_str) != Some(name));
    if before == config.derived_fields.len() {
        return Ok(MutationReceipt {
            analysis_context: current_for_edit(expected)?,
        });
    }
    validate_config(&config)?;
    let _prepared = prepare_config(expected, &config)?;
    crate::operations::check()?;
    let snapshot = analysis_context::update(expected, config)?;
    crate::operations::commit();
    Ok(MutationReceipt {
        analysis_context: snapshot,
    })
}

pub(crate) fn list_definitions(expected: &Identity) -> Result<Vec<sources::DerivedField>, String> {
    let snapshot = current_for_edit(expected)?;
    Ok(snapshot
        .config
        .derived_fields
        .into_iter()
        .filter_map(|value| {
            serde_json::from_value::<sources::DerivedFieldCompat>(value)
                .ok()
                .map(sources::DerivedFieldCompat::normalize)
        })
        .collect())
}

/// The browser consumes native-safe context metadata. Keep its exact decoded
/// tree charged through IPC, including the separately cloned diagnostics.
pub(crate) struct SnapshotResponse {
    snapshot: Snapshot,
    _read_credit: Option<crate::case_work_budget::Lease>,
    _diagnostic_credit: Option<crate::case_work_budget::Lease>,
}
impl serde::Serialize for SnapshotResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.snapshot.serialize(serializer)
    }
}
const SNAPSHOT_NOT_READY: &str = "CASE_CONTEXT_NOT_READY: A configuração preservada não pode atravessar o transporte numérico do navegador sem perda. Os valores originais permanecem preservados.";
fn snapshot_response(root: &std::path::Path, case_id: &str) -> Result<SnapshotResponse, String> {
    let conn = crate::case_store::context_connection(root)?;
    let (snapshot, credit) = analysis_context::read_snapshot_leased(&conn, case_id)?;
    let mut response = SnapshotResponse { snapshot, _read_credit: credit, _diagnostic_credit: None };
    crate::case_evidence::snapshot_transport_safe(&response.snapshot).map_err(|_| SNAPSHOT_NOT_READY)?;
    // The bounded 4 MiB context can carry diagnostic strings and entries that
    // the runtime cache clones. Reserve before that allocation is made.
    response._diagnostic_credit = Some(crate::case_cache::reserve_work(crate::case_work_budget::global(), 16 << 20)?);
    response.snapshot = crate::analysis_runtime::with_diagnostics(response.snapshot)?;
    crate::case_evidence::snapshot_transport_safe(&response.snapshot).map_err(|_| SNAPSHOT_NOT_READY)?;
    crate::operations::check()?;
    Ok(response)
}
#[tauri::command]
pub(crate) async fn analysis_context_snapshot(case_id: String) -> Result<SnapshotResponse, String> {
    crate::offload(move || snapshot_response(&crate::config_dir(), &case_id)).await?
}

const BULK_NATIVE_UNSUPPORTED: &str = "CASE_CONTEXT_TYPED_EDIT_REQUIRED: Use os editores de campos derivados e referências para alterar a configuração deste Caso preservado.";
fn update_context_from_browser_at(
    root: &std::path::Path,
    expected: &Identity,
    config: Config,
) -> Result<MutationReceipt, String> {
    // Config's legacy Value decoder has already run at the IPC boundary. None
    // of those caller values may replace a protected native configuration.
    if crate::case_evidence::native_owner(root, expected)? {
        return Err(BULK_NATIVE_UNSUPPORTED.into());
    }
    current_for_edit(expected)?;
    validate_config(&config)?;
    let _prepared = prepare_config(expected, &config)?;
    crate::operations::check()?;
    let snapshot = analysis_context::update_legacy_browser_at(root, expected, config)?;
    crate::operations::commit();
    Ok(MutationReceipt {
        analysis_context: snapshot,
    })
}
#[tauri::command]
pub(crate) async fn analysis_context_update(
    analysis_context: Identity,
    config: Config,
    operation_id: Option<String>,
) -> Result<MutationReceipt, String> {
    crate::offload_operation(operation_id, move || {
        update_context_from_browser_at(&crate::config_dir(), &analysis_context, config)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn preview_field_transform(
    value: Value,
    steps: Vec<field_transform::Step>,
) -> Result<field_transform::Output, String> {
    crate::offload(move || {
        field_transform::transform(&value, &steps, field_transform::Limits::default())
            .map_err(|error| error.to_string())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_regex_preserves_pipeline_identity_and_other_case_configuration() {
        let original = Config {
            derived_fields: vec![
                json!({"id":"stable","name":"decoded","source":"message","rules":[{"pattern":"(.*)"}],"steps":["url_decode"],"note":"kept"}),
            ],
            references: Vec::new(),
        };
        let rules = vec![sources::DerivedRule {
            pattern: "value=(.*)".into(),
            template: None,
            filter: None,
        }];
        let changed = replace_definition(&original, "decoded", "message", rules, None).unwrap();
        assert_eq!(changed.derived_fields[0]["id"], "stable");
        assert_eq!(changed.derived_fields[0]["note"], "kept");
        assert_eq!(changed.derived_fields[0]["steps"], json!(["url_decode"]));
        assert_eq!(original.derived_fields[0]["rules"][0]["pattern"], "(.*)");
        let cleared = replace_definition(
            &changed,
            "decoded",
            "message",
            vec![sources::DerivedRule {
                pattern: "(.*)".into(),
                template: None,
                filter: None,
            }],
            Some(Vec::new()),
        )
        .unwrap();
        assert_eq!(cleared.derived_fields[0]["steps"], json!([]));
    }
    #[test]
    fn creation_keeps_typed_steps_and_rejects_invalid_conditions() {
        let config = replace_definition(
            &Config::default(),
            "decoded",
            "payload",
            Vec::new(),
            Some(vec![
                field_transform::Step::Base64Decode,
                field_transform::Step::ParseJson,
            ]),
        )
        .unwrap();
        assert!(config.derived_fields[0]["id"].as_str().is_some());
        let bad: sources::DerivedRule = serde_json::from_value(
            json!({"pattern":"(.*)","filter":{"column":"code","op":"unsupported","value":"1"}}),
        )
        .unwrap();
        assert!(replace_definition(&config, "bad", "message", vec![bad], None).is_err());
    }
    fn native_snapshot_fixture(raw: Option<Value>) -> (tempfile::TempDir, Snapshot) {
        let root = tempfile::tempdir().unwrap();
        let conn = crate::case_store::connect(root.path()).unwrap();
        conn.execute_batch("INSERT INTO metadata VALUES('revision','1');INSERT INTO metadata VALUES('case-analysis-v1','1');INSERT INTO cases VALUES('snapshot-case','{\"id\":\"snapshot-case\"}',0);CREATE TABLE native_evidence_cases(case_id TEXT PRIMARY KEY,analysis_id TEXT NOT NULL);").unwrap();
        let mut snapshot = analysis_context::prepare_new_case("snapshot-case").unwrap();
        snapshot.legacy_raw = raw;
        conn.execute("INSERT INTO case_analysis VALUES(?1,?2)",rusqlite::params![snapshot.case_id,serde_json::to_string(&snapshot).unwrap()]).unwrap();
        conn.execute("INSERT INTO native_evidence_cases VALUES(?1,?2)",rusqlite::params![snapshot.case_id,snapshot.analysis_id]).unwrap();
        drop(conn);
        (root,snapshot)
    }
    #[test]
    fn browser_snapshot_keeps_exact_decode_credit_through_serialization() {
        let (root, snapshot) = native_snapshot_fixture(Some(json!({"fraction":1.25})));
        let before = crate::case_work_budget::global().used();
        let response = snapshot_response(root.path(), &snapshot.case_id).unwrap();
        assert!(response._read_credit.as_ref().is_some_and(|credit| credit.bytes() > 0));
        assert!(crate::case_work_budget::global().used() > before);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["legacyRaw"]["fraction"].as_f64(),Some(1.25));
        assert!(json.get("_readCredit").is_none());
        assert!(crate::case_work_budget::global().used() > before);
        drop(response); assert_eq!(crate::case_work_budget::global().used(),before);
    }
    #[test]
    fn browser_snapshot_refuses_unsafe_number_variants_without_rewriting_storage() {
        for raw in [json!(u64::MAX),json!(1.0),json!(-0.0)] {
            let (root, snapshot) = native_snapshot_fixture(Some(raw));
            let before = crate::case_work_budget::global().used();
            let error = snapshot_response(root.path(), &snapshot.case_id).err().unwrap();
            assert!(error.starts_with("CASE_CONTEXT_NOT_READY"));
            assert_eq!(crate::case_work_budget::global().used(),before);
            let conn=rusqlite::Connection::open(root.path().join("investigations.sqlite3")).unwrap();
            let stored:String=conn.query_row("SELECT body FROM case_analysis WHERE case_id=?1",[&snapshot.case_id],|row|row.get(0)).unwrap();
            assert_eq!(stored,serde_json::to_string(&snapshot).unwrap());
        }
    }
    #[test]
    fn bulk_browser_configuration_refuses_native_owner_before_compilation_or_mutation() {
        let (root, snapshot) = native_snapshot_fixture(Some(json!({"wide":u64::MAX,"float":1.0})));
        let conn = rusqlite::Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let before: String = conn
            .query_row(
                "SELECT body FROM case_analysis WHERE case_id=?1",
                [&snapshot.case_id],
                |row| row.get(0),
            )
            .unwrap();
        let revision: String = conn
            .query_row(
                "SELECT value FROM metadata WHERE key='revision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // Invalid regex would fail during compilation if the protected-owner
        // refusal were accidentally moved below the legacy Value path.
        let replacement = Config {
            derived_fields: vec![
                json!({"name":"bad","source":"message","rules":[{"pattern":"("}]}),
            ],
            references: vec![],
        };
        let error = update_context_from_browser_at(root.path(), &snapshot.identity(), replacement)
            .err()
            .unwrap();
        assert_eq!(error, BULK_NATIVE_UNSUPPORTED);
        let after: String = conn
            .query_row(
                "SELECT body FROM case_analysis WHERE case_id=?1",
                [&snapshot.case_id],
                |row| row.get(0),
            )
            .unwrap();
        let after_revision: String = conn
            .query_row(
                "SELECT value FROM metadata WHERE key='revision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(revision, after_revision);
    }

}
