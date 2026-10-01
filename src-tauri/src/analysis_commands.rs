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

#[tauri::command]
pub(crate) async fn analysis_context_snapshot(case_id: String) -> Result<Snapshot, String> {
    crate::offload(move || {
        crate::analysis_runtime::with_diagnostics(analysis_context::snapshot(&case_id)?)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn analysis_context_update(
    analysis_context: Identity,
    config: Config,
) -> Result<MutationReceipt, String> {
    crate::offload(move || {
        current_for_edit(&analysis_context)?;
        validate_config(&config)?;
        crate::operations::check()?;
        let snapshot = analysis_context::update(&analysis_context, config)?;
        crate::operations::commit();
        Ok(MutationReceipt {
            analysis_context: snapshot,
        })
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
}
