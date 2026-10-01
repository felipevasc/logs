//! Full admitted details. Page responses intentionally omit raw text and must
//! not be used as the detail transport for synchronized Case evidence.
use crate::{analysis_context::Identity, analysis_runtime, model::Event, AppState};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

struct Admission {
    admitted: Arc<analysis_runtime::Admitted>,
    events: Option<Arc<Vec<Event>>>,
}

fn require_synchronized_evidence(events: &Option<serde_json::Value>) -> Result<(), String> {
    if events.is_some() {
        return Err("CASE_DETAIL_SYNC_REQUIRED: Sincronize as evidências antes de abrir o detalhe do Caso.".into());
    }
    Ok(())
}

fn capture(
    state: &AppState,
    identity: Option<Identity>,
    generation: Option<u64>,
    case_key: Option<String>,
    case_content_token: Option<String>,
) -> Result<Admission, String> {
    match (case_key.as_deref(), case_content_token.as_deref()) {
        (None, None) => (),
        (Some(key), Some(token)) if !key.is_empty() && key.len() <= 512
            && !token.is_empty() && token.len() <= 128 => (),
        _ => return Err("CASE_DETAIL_ADMISSION: Informe a chave e o recibo atuais de case_sync apenas ao consultar evidências do Caso.".into()),
    }
    // There is no inline Event argument: a detail refers to one synchronized
    // publication, including replacement under the same key/id/event_ref.
    let (admitted, events) = analysis_runtime::capture_case_shared(state, identity, generation, None, case_key)?;
    if admitted.case_content_token != case_content_token {
        return Err(crate::case_cache::CHANGED.into());
    }
    Ok(Admission { admitted, events })
}

pub(crate) fn detail_in_scope(
    state: &AppState,
    id: usize,
    event_ref: Option<&str>,
    visible_case: Option<Vec<Event>>,
) -> Result<Option<Event>, String> {
    let _interactive = crate::operations::interactive();
    crate::operations::check()?;
    let mut event = match visible_case {
        Some(events) => events.into_iter().find(|event| event.id == id),
        None => crate::event_detail_raw(state, id),
    };
    if let Some(event) = &mut event {
        if event_ref.is_some_and(|expected| expected.is_empty() || event.event_ref != expected) {
            return Err("O registro retornado não corresponde à referência solicitada.".into());
        }
        crate::entities::annotate(event);
    }
    crate::operations::check()?;
    Ok(event)
}

fn detail_in_admission(state: &AppState, admitted: &analysis_runtime::Admitted, id: usize, event_ref: Option<&str>, visible_case: Option<Vec<Event>>) -> Result<Option<Event>, String> {
    let result = detail_in_scope(state, id, event_ref, visible_case)?;
    // A detail is an action on the currently displayed context. Unlike a
    // background aggregate, it must not publish after config/source changes.
    admitted.validate(state)?;
    Ok(result)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn event_detail(
    id: usize,
    event_ref: Option<String>,
    case_events: Option<serde_json::Value>,
    case_key: Option<String>,
    case_content_token: Option<String>,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<Option<Event>, String> {
    require_synchronized_evidence(&case_events)?;
    let Admission { admitted, events } = capture(
        app.state::<AppState>().inner(), analysis_context, source_generation, case_key, case_content_token,
    )?;
    let captured = Arc::clone(&admitted);
    crate::offload_case_record(operation_id, app.clone(), admitted, events, id, event_ref.clone(), move |events| {
        detail_in_admission(app.state::<AppState>().inner(), &captured, id, event_ref.as_deref(), events)
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analysis_context, SourceData};
    use base64::Engine;
    use parking_lot::{Mutex, RwLock};
    use serde_json::json;

    struct Fixture {
        directory: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
        state: AppState,
        identity: Identity,
        event: Event,
        key: String,
        token: String,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("LOGINSIGHT_DATA_DIR");
            std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
            crate::case_store::save_at(directory.path(), json!({"cases":[{"id":"detail-case"}]})).unwrap();
            let initial = analysis_context::snapshot("detail-case").unwrap();
            let updated = analysis_context::update(&initial.identity(), analysis_context::Config {
                derived_fields: vec![json!({"name":"decoded","source":"payload","steps":["base64_decode","parse_json"]})],
                references: vec![],
            }).unwrap();
            let state = AppState {
                source: RwLock::new(SourceData::None), source_publication: RwLock::new(Default::default()),
                source_names: RwLock::new(vec![]), codes: RwLock::new(Default::default()),
                system_codes: RwLock::new(Default::default()), derived: RwLock::new(vec![]),
                case_store_lock: Mutex::new(()), codes_path: directory.path().join("codes.json"),
                system_codes_path: directory.path().join("system-codes.json"),
            };
            let mut event = Event::empty();
            event.id = 7; event.event_ref = "detail-evidence:7".into();
            event.raw = "original:".to_string() + &"x".repeat(72_000);
            event.fields.insert("payload".into(), json!(base64::engine::general_purpose::STANDARD.encode(br#"{"code":503,"ok":false,"number":1.0}"#)));
            let key = format!("detail-{}", uuid::Uuid::new_v4());
            let synced = tauri::async_runtime::block_on(crate::case_cache::case_sync(key.clone(), vec![event.clone()], Some(updated.identity()))).unwrap();
            Self { directory, previous, state, identity: updated.identity(), event, key, token: synced.case_content_token }
        }
        fn capture(&self) -> Result<Admission, String> {
            capture(&self.state, Some(self.identity.clone()), None, Some(self.key.clone()), Some(self.token.clone()))
        }
        fn read(&self, capture: Admission, id: usize, event_ref: &str) -> Result<Option<Event>, String> {
            let Admission { admitted, events } = capture;
            analysis_runtime::with(Some(admitted.clone()), || {
                admitted.validate(&self.state)?;
                let events = admitted.prepare_visibility_record(events.as_deref().map(Vec::as_slice), id, Some(event_ref))?;
                let result = detail_in_admission(&self.state, &admitted, id, Some(event_ref), events)?;
                admitted.validate_visibility()?;
                Ok(result)
            })
        }
    }

    #[test]
    fn case_details_keep_full_raw_and_current_typed_children_without_mutating_saved_rows() {
        let fixture = Fixture::new();
        let detail = fixture.read(fixture.capture().unwrap(), 7, &fixture.event.event_ref).unwrap().unwrap();
        assert_eq!(detail.raw, fixture.event.raw);
        assert_eq!(detail.fields["payload"], fixture.event.fields["payload"]);
        assert_eq!(detail.fields["decoded.code"], json!(503));
        assert_eq!(detail.fields["decoded.ok"], json!(false));
        assert_eq!(detail.fields["decoded.number"].to_string(), "1.0");
        assert!(!fixture.event.fields.contains_key("decoded"));
        assert!(fixture.read(fixture.capture().unwrap(), 8, "missing").unwrap().is_none());
        assert!(fixture.read(fixture.capture().unwrap(), 7, "replacement").is_err());
        assert!(fixture.directory.path().join("investigations.sqlite3").exists());
    }

    #[test]
    fn case_details_reject_missing_or_replaced_sync_tokens_before_and_after_capture() {
        let fixture = Fixture::new();
        assert!(require_synchronized_evidence(&None).is_ok());
        for value in [json!([]), json!([fixture.event.clone()]), json!({"caseKey":"inline"})] {
            assert!(require_synchronized_evidence(&Some(value)).unwrap_err().contains("CASE_DETAIL_SYNC_REQUIRED"));
        }
        assert!(capture(&fixture.state, Some(fixture.identity.clone()), None, Some(fixture.key.clone()), None).is_err());
        assert!(capture(&fixture.state, Some(fixture.identity.clone()), None, None, Some(fixture.token.clone())).is_err());
        let previous = fixture.capture().unwrap();
        let mut replacement = fixture.event.clone(); replacement.raw = "replacement raw".into();
        tauri::async_runtime::block_on(crate::case_cache::case_sync(fixture.key.clone(), vec![replacement], Some(fixture.identity.clone()))).unwrap();
        assert_eq!(fixture.capture().err().unwrap(), crate::case_cache::CHANGED);
        assert_eq!(fixture.read(previous, 7, &fixture.event.event_ref).unwrap_err(), crate::case_cache::CHANGED);
    }

    #[test]
    fn full_detail_revalidates_configuration_after_the_visible_overlay_is_prepared() {
        let fixture = Fixture::new();
        let Admission { admitted, events } = fixture.capture().unwrap();
        analysis_runtime::with(Some(admitted.clone()), || {
            admitted.validate(&fixture.state).unwrap();
            let prepared = admitted.prepare_visibility_record(events.as_deref().map(Vec::as_slice), 7, Some(&fixture.event.event_ref)).unwrap();
            let current = analysis_context::snapshot("detail-case").unwrap();
            analysis_context::update(&current.identity(), current.config).unwrap();
            assert!(detail_in_admission(&fixture.state, &admitted, 7, Some(&fixture.event.event_ref), prepared).is_err());
            assert!(!fixture.event.fields.contains_key("decoded"));
        });
    }
}
