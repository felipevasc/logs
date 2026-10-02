//! Full admitted details. Page responses intentionally omit raw text and must
//! not be used as the detail transport for synchronized Case evidence.
use crate::{analysis_context::Identity, analysis_runtime, model::Event, AppState};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

struct Admission {
    admitted: Arc<analysis_runtime::Admitted>,
    events: Option<Arc<crate::case_cache::Records>>,
}

fn require_synchronized_evidence(events: &Option<serde_json::Value>) -> Result<(), String> {
    if events.is_some() {
        return Err("CASE_DETAIL_SYNC_REQUIRED: Sincronize as evidências antes de abrir o detalhe do Caso.".into());
    }
    Ok(())
}

fn validate_capture_request(case_key: Option<&str>, case_content_token: Option<&str>) -> Result<(), String> {
    match (case_key, case_content_token) {
        (None, None) => (),
        (Some(key), Some(token)) if !key.is_empty() && key.len() <= 512
            && !token.is_empty() && token.len() <= 128 => (),
        _ => return Err("CASE_DETAIL_ADMISSION: Informe a chave e o recibo atuais de case_sync apenas ao consultar evidências do Caso.".into()),
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
    validate_capture_request(case_key.as_deref(), case_content_token.as_deref())?;
    // There is no inline Event argument: a detail refers to one synchronized
    // publication, including replacement under the same key/id/event_ref.
    let (admitted, events) = analysis_runtime::capture_case_shared(state, identity, generation, None, case_key)?;
    if admitted.case_content_token != case_content_token {
        return Err(crate::case_cache::CHANGED.into());
    }
    Ok(Admission { admitted, events })
}

fn record_in_scope(
    state: &AppState,
    id: usize,
    event_ref: Option<&str>,
    visible_case: Option<Vec<Event>>,
) -> Result<Option<Event>, String> {
    crate::operations::check()?;
    let event = match visible_case {
        Some(events) => events.into_iter().find(|event| event.id == id),
        None => crate::event_detail_raw(state, id),
    };
    if let Some(event) = &event {
        if event_ref.is_some_and(|expected| expected.is_empty() || event.event_ref != expected) {
            return Err("O registro retornado não corresponde à referência solicitada.".into());
        }
    }
    crate::operations::check()?;
    Ok(event)
}

pub(crate) fn detail_in_scope(
    state: &AppState,
    id: usize,
    event_ref: Option<&str>,
    visible_case: Option<Vec<Event>>,
) -> Result<Option<Event>, String> {
    let _interactive = crate::operations::interactive();
    let mut event = record_in_scope(state, id, event_ref, visible_case)?;
    if let Some(event) = &mut event { crate::entities::annotate(event); }
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceRow {
    id: usize,
    event_ref: String,
}

/// Rich frames belong to an explicit detail request, never ordinary Event
/// fields, page payloads, columnar values or automatic Case serialization.
#[derive(serde::Serialize)]
pub(crate) struct JavaTraceDetail {
    state: &'static str,
    trace: Option<crate::java_stacktrace::Trace>,
    reason: Option<&'static str>,
    row: Option<TraceRow>,
}

fn java_trace_in_admission(
    state: &AppState,
    admitted: &analysis_runtime::Admitted,
    id: usize,
    event_ref: &str,
    visible_case: Option<Vec<Event>>,
) -> Result<JavaTraceDetail, String> {
    let _interactive = crate::operations::interactive();
    if event_ref.is_empty() { return Err("Informe a referência exata do registro para interpretar a stack trace.".into()); }
    let event = record_in_scope(state, id, Some(event_ref), visible_case)?;
    let result = if let Some(event) = event {
        let row = Some(TraceRow { id: event.id, event_ref: event.event_ref.clone() });
        if event.raw.is_empty() {
            JavaTraceDetail { state: "unavailable", trace: None, reason: Some("raw_unavailable"), row }
        } else if let Some(trace) = crate::sources::java_trace_for_event(&event)? {
            JavaTraceDetail { state: "available", trace: Some(trace), reason: None, row }
        } else {
            JavaTraceDetail { state: "unavailable", trace: None, reason: Some("not_java"), row }
        }
    } else {
        JavaTraceDetail { state: "unavailable", trace: None, reason: Some("record_unavailable"), row: None }
    };
    crate::operations::check()?;
    admitted.validate(state)?;
    admitted.validate_visibility()?;
    Ok(result)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn java_trace_detail(
    id: usize,
    event_ref: String,
    case_events: Option<serde_json::Value>,
    case_key: Option<String>,
    case_content_token: Option<String>,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<JavaTraceDetail, String> {
    require_synchronized_evidence(&case_events)?;
    if event_ref.is_empty() { return Err("Informe a referência exata do registro para interpretar a stack trace.".into()); }
    validate_capture_request(case_key.as_deref(), case_content_token.as_deref())?;
    let pin = analysis_runtime::CapturePin::for_case(app.state::<AppState>().inner(), analysis_context, source_generation, false, case_key.as_deref())?;
    let (admitted, events) = analysis_runtime::capture_prepared(app.clone(), pin, operation_id.clone(), crate::global_scheduler::Priority::Interactive, move |state, pin| {
        let Admission { admitted, events } = capture(state, pin.identity(), pin.generation(), case_key, case_content_token)?;
        Ok((admitted, events))
    }).await?;
    let captured = Arc::clone(&admitted);
    crate::offload_case_record(operation_id, app.clone(), admitted, events, id, Some(event_ref.clone()), move |events| {
        java_trace_in_admission(app.state::<AppState>().inner(), &captured, id, &event_ref, events)
    }).await?
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
    validate_capture_request(case_key.as_deref(), case_content_token.as_deref())?;
    let pin = analysis_runtime::CapturePin::for_case(app.state::<AppState>().inner(), analysis_context, source_generation, false, case_key.as_deref())?;
    let (admitted, events) = analysis_runtime::capture_prepared(app.clone(), pin, operation_id.clone(), crate::global_scheduler::Priority::Interactive, move |state, pin| {
        let Admission { admitted, events } = capture(state, pin.identity(), pin.generation(), case_key, case_content_token)?;
        Ok((admitted, events))
    }).await?;
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
                let events = admitted.prepare_visibility_record(events.as_deref().map(crate::case_cache::Records::as_slice), id, Some(event_ref))?;
                let result = detail_in_admission(&self.state, &admitted, id, Some(event_ref), events)?;
                admitted.validate_visibility()?;
                Ok(result)
            })
        }
        fn replace_raw(&mut self, raw: &str) {
            self.event.raw = raw.to_owned();
            self.token = tauri::async_runtime::block_on(crate::case_cache::case_sync(
                self.key.clone(), vec![self.event.clone()], Some(self.identity.clone()),
            )).unwrap().case_content_token;
        }
        fn trace(&self, capture: Admission, id: usize, event_ref: &str) -> Result<JavaTraceDetail, String> {
            let Admission { admitted, events } = capture;
            analysis_runtime::with(Some(admitted.clone()), || {
                admitted.validate(&self.state)?;
                let events = admitted.prepare_visibility_record(events.as_deref().map(crate::case_cache::Records::as_slice), id, Some(event_ref))?;
                java_trace_in_admission(&self.state, &admitted, id, event_ref, events)
            })
        }
    }

    const JAVA: &str = "2026-09-30 12:00:00,000 ERROR [main] app.Service - java.lang.IllegalStateException: broken\n\tat app.Service.run(Service.java:17)\n";

    #[test]
    fn java_trace_metadata_is_exact_and_separate_from_saved_event_fields() {
        let mut fixture = Fixture::new();
        fixture.replace_raw(JAVA);
        let before = serde_json::to_vec(&fixture.event).unwrap();
        let result = fixture.trace(fixture.capture().unwrap(), 7, &fixture.event.event_ref).unwrap();
        assert_eq!(result.state, "available");
        assert!(result.reason.is_none());
        assert_eq!(result.trace.as_ref().unwrap().exception_class(), Some("java.lang.IllegalStateException"));
        let wire = serde_json::to_value(result).unwrap();
        assert_eq!(wire["row"], json!({"id":7,"eventRef":fixture.event.event_ref}));
        assert!(wire.get("fields").is_none());
        assert!(wire.get("raw").is_none());
        assert_eq!(serde_json::to_vec(&fixture.event).unwrap(), before);
        assert!(!fixture.event.fields.contains_key("java.trace"));
        assert!(fixture.trace(fixture.capture().unwrap(), 7, "different-reference").is_err());
        assert!(fixture.trace(fixture.capture().unwrap(), 7, "").is_err());
    }

    #[test]
    fn java_trace_unavailable_states_do_not_invent_raw_or_fall_back_to_dataset() {
        let mut fixture = Fixture::new();
        fixture.replace_raw("");
        let unavailable = fixture.trace(fixture.capture().unwrap(), 7, &fixture.event.event_ref).unwrap();
        assert_eq!(unavailable.reason, Some("raw_unavailable"));
        assert!(unavailable.trace.is_none());
        fixture.replace_raw("An ordinary event without a Java trace");
        assert_eq!(fixture.trace(fixture.capture().unwrap(), 7, &fixture.event.event_ref).unwrap().reason, Some("not_java"));
        fixture.replace_raw(JAVA);
        *fixture.state.source.write() = SourceData::Memory(vec![fixture.event.clone()]);
        let Admission { admitted, events: _ } = fixture.capture().unwrap();
        analysis_runtime::with(Some(admitted.clone()), || {
            admitted.prepare_visibility_record(Some(&[]), 7, Some(&fixture.event.event_ref)).unwrap();
            let hidden = java_trace_in_admission(&fixture.state, &admitted, 7, &fixture.event.event_ref, Some(vec![])).unwrap();
            assert_eq!(hidden.reason, Some("record_unavailable"));
            assert!(hidden.row.is_none());
            assert!(hidden.trace.is_none());
        });
    }

    #[test]
    fn java_trace_rejects_replaced_publications_config_and_cancellation() {
        let mut fixture = Fixture::new();
        fixture.replace_raw(JAVA);
        let old = fixture.capture().unwrap();
        fixture.replace_raw("replacement raw");
        assert_eq!(fixture.trace(old, 7, &fixture.event.event_ref).err().unwrap(), crate::case_cache::CHANGED);
        fixture.replace_raw(JAVA);
        let Admission { admitted, events } = fixture.capture().unwrap();
        analysis_runtime::with(Some(admitted.clone()), || {
            let prepared = admitted.prepare_visibility_record(events.as_deref().map(crate::case_cache::Records::as_slice), 7, Some(&fixture.event.event_ref)).unwrap();
            let current = analysis_context::snapshot("detail-case").unwrap();
            analysis_context::update(&current.identity(), current.config).unwrap();
            assert!(java_trace_in_admission(&fixture.state, &admitted, 7, &fixture.event.event_ref, prepared).is_err());
        });
        fixture.identity = analysis_context::snapshot("detail-case").unwrap().identity();
        fixture.replace_raw(JAVA);
        let id = format!("java-trace-cancel-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let admission = fixture.capture().unwrap();
        let result = crate::operations::run_with_token(token, || {
            assert!(crate::operations::cancel_id(&id));
            assert!(fixture.trace(admission, 7, &fixture.event.event_ref).is_err());
        });
        assert!(result.is_err());
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
            let prepared = admitted.prepare_visibility_record(events.as_deref().map(crate::case_cache::Records::as_slice), 7, Some(&fixture.event.event_ref)).unwrap();
            let current = analysis_context::snapshot("detail-case").unwrap();
            analysis_context::update(&current.identity(), current.config).unwrap();
            assert!(detail_in_admission(&fixture.state, &admitted, 7, Some(&fixture.event.event_ref), prepared).is_err());
            assert!(!fixture.event.fields.contains_key("decoded"));
        });
    }
}
