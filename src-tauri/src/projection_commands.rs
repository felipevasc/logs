//! Opt-in bounded rows and an explicit canonical field action. Existing Event
//! APIs stay available until every exact frontend action has its own boundary.
use crate::{
    analysis_context::Identity,
    analysis_runtime,
    model::Event,
    page_projection::{
        ExactField, ProjectedPage, ProjectionPlan, ProjectionRequest, Receipt, RowHandle,
    },
    query, AppState, SourceData,
};
use tauri::{AppHandle, Manager};

struct FieldTextAdmission {
    admitted: std::sync::Arc<analysis_runtime::Admitted>,
    events: Option<Vec<Event>>,
    receipt: Receipt,
}

fn capture_field_text(
    state: &AppState,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    case_key: Option<String>,
    case_content_token: Option<String>,
) -> Result<FieldTextAdmission, String> {
    // Unlike a general query, an exact action must name the source shown by
    // its caller. Missing context must never silently bind the current source.
    if analysis_context.is_none() || (case_key.is_none() && source_generation.is_none()) {
        return Err("ANALYSIS_FIELD_ADMISSION: Informe a identidade da análise e a geração da fonte ou a chave sincronizada do Caso.".into());
    }
    if case_key
        .as_deref()
        .is_some_and(|key| key.is_empty() || key.len() > 512)
    {
        return Err("ANALYSIS_FIELD_ADMISSION: Chave sincronizada do Caso inválida.".into());
    }
    match (case_key.as_ref(), case_content_token.as_deref()) {
        (None, None) => (),
        (Some(_), Some(token)) if !token.is_empty() && token.len() <= 128 => (),
        _ => return Err("ANALYSIS_FIELD_ADMISSION: Informe o recibo caseContentToken retornado por case_sync apenas ao consultar evidências do Caso.".into()),
    }
    let _publication = crate::catalog_read_guard()?;
    // No client-supplied Event/value is accepted. Case content and its token
    // are captured together from case_sync, including same-key replacement.
    let (admitted, events) =
        analysis_runtime::capture_case(state, analysis_context, source_generation, None, case_key)?;
    // The caller's sync token predates command admission. Comparing only a
    // freshly constructed receipt would miss same-key replacement before it.
    if admitted.case_content_token.as_deref() != case_content_token.as_deref() {
        return Err(crate::case_cache::CHANGED.into());
    }
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let receipt = Receipt::from_admitted(&admitted, &codes, &system)?;
    Ok(FieldTextAdmission {
        admitted,
        events,
        receipt,
    })
}

/// Canonical comparator/copy text for an existing Event row, without enabling
/// projected pages or round-tripping numbers/objects through JavaScript.
/// Tauri arguments use camelCase, including eventRef and analysisContext.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn analysis_field_text(
    id: usize,
    event_ref: String,
    column: String,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    case_key: Option<String>,
    case_content_token: Option<String>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<ExactField, String> {
    let FieldTextAdmission {
        admitted,
        events,
        receipt,
    } = capture_field_text(
        app.state::<AppState>().inner(),
        analysis_context,
        source_generation,
        case_key,
        case_content_token,
    )?;
    let captured = admitted.clone();
    let row = RowHandle { id, event_ref };
    crate::offload_case(operation_id, app.clone(), admitted, events, move |events| {
        let _interactive = crate::operations::interactive();
        let _publication = crate::catalog_read_guard()?;
        // Keep the exact helper's all-or-error record/output bounds and its
        // final source/config/cache/catalog/visibility revalidation intact.
        crate::page_projection::hydrate_projected_field(
            app.state::<AppState>().inner(),
            &captured,
            &receipt,
            &row,
            &column,
            events.as_deref(),
        )
    })
    .await?
}

fn require_synchronized_case(events: &Option<Vec<Event>>) -> Result<(), String> {
    if events.is_some() {
        return Err("PROJECTED_CASE_SYNC_REQUIRED: Sincronize as evidências com case_sync e consulte pela chave do Caso antes de usar páginas projetadas.".into());
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn query_projected_page(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    cursor: Option<String>,
    projection: ProjectionRequest,
    operation_id: Option<String>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<ProjectedPage, String> {
    require_synchronized_case(&case_events)?;
    crate::workspace::validate(&filters)?;
    let (admitted, case_events, receipt, plan) = {
        // Publication read guard spans capture, but no catalog guard spans
        // source acquisition. Writers use this same boundary through enrich.
        let _publication = crate::catalog_read_guard()?;
        let state = app.state::<AppState>();
        let (admitted, events) = analysis_runtime::capture_case(
            state.inner(),
            analysis_context,
            source_generation,
            case_events,
            case_key,
        )?;
        let codes = state.codes.read();
        let system = state.system_codes.read();
        let receipt = Receipt::from_admitted(&admitted, &codes, &system)?;
        let plan = ProjectionPlan::new(projection, limit, receipt.clone())?;
        (admitted, events, receipt, plan)
    };
    let captured = admitted.clone();
    crate::offload_case(
        operation_id,
        app.clone(),
        admitted,
        case_events,
        move |events| {
            let _interactive = crate::operations::interactive();
            let _publication = crate::catalog_read_guard()?;
            let state = app.state::<AppState>();
            let source = analysis_runtime::source(state.inner());
            let codes = state.codes.read();
            let system = state.system_codes.read();
            receipt.validate_admitted(&captured, &codes, &system)?;
            let derived = analysis_runtime::derived(state.inner());
            let result = if let Some(events) = events.as_deref() {
                query::query_projected_memory(
                    events,
                    &filters,
                    &sort_column,
                    &sort_dir,
                    offset,
                    &plan,
                )
            } else {
                match &*source {
                    SourceData::Indexed(index) => query::query_projected_indexed(
                        index,
                        &filters,
                        &sort_column,
                        &sort_dir,
                        offset,
                        cursor.as_deref(),
                        &codes,
                        &system,
                        &derived,
                        &plan,
                    ),
                    SourceData::Memory(events) => query::query_projected_memory(
                        events,
                        &filters,
                        &sort_column,
                        &sort_dir,
                        offset,
                        &plan,
                    ),
                    SourceData::None => query::query_projected_memory(
                        &[],
                        &filters,
                        &sort_column,
                        &sort_dir,
                        offset,
                        &plan,
                    ),
                }
            }?;
            drop(codes);
            drop(system);
            drop(source);
            captured.validate(state.inner())?;
            captured.validate_visibility()?;
            Ok(result)
        },
    )
    .await?
}

#[tauri::command]
pub(crate) async fn hydrate_projected_rows(
    receipt: Receipt,
    rows: Vec<RowHandle>,
    case_events: Option<Vec<Event>>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<Vec<Event>, String> {
    require_synchronized_case(&case_events)?;
    let (admitted, events) = {
        let _publication = crate::catalog_read_guard()?;
        let state = app.state::<AppState>();
        let (admitted, events) = analysis_runtime::capture_case(
            state.inner(),
            Some(receipt.analysis_context.clone()),
            receipt.source_generation,
            case_events,
            receipt.case_key.clone(),
        )?;
        receipt.validate_admitted(&admitted, &state.codes.read(), &state.system_codes.read())?;
        (admitted, events)
    };
    let captured = admitted.clone();
    crate::offload_case(operation_id, app.clone(), admitted, events, move |events| {
        let _interactive = crate::operations::interactive();
        let _publication = crate::catalog_read_guard()?;
        crate::page_projection::hydrate_projected_rows(
            app.state::<AppState>().inner(),
            &captured,
            &receipt,
            &rows,
            events.as_deref(),
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn hydrate_projected_field(
    receipt: Receipt,
    row: RowHandle,
    column: String,
    case_events: Option<Vec<Event>>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<ExactField, String> {
    require_synchronized_case(&case_events)?;
    let (admitted, events) = {
        let _publication = crate::catalog_read_guard()?;
        let state = app.state::<AppState>();
        let (admitted, events) = analysis_runtime::capture_case(
            state.inner(),
            Some(receipt.analysis_context.clone()),
            receipt.source_generation,
            case_events,
            receipt.case_key.clone(),
        )?;
        receipt.validate_admitted(&admitted, &state.codes.read(), &state.system_codes.read())?;
        (admitted, events)
    };
    let captured = admitted.clone();
    crate::offload_case(operation_id, app.clone(), admitted, events, move |events| {
        let _interactive = crate::operations::interactive();
        let _publication = crate::catalog_read_guard()?;
        crate::page_projection::hydrate_projected_field(
            app.state::<AppState>().inner(),
            &captured,
            &receipt,
            &row,
            &column,
            events.as_deref(),
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::{Mutex, RwLock};
    use std::sync::Arc;

    #[test]
    fn projected_case_actions_require_a_synchronized_publication() {
        assert!(require_synchronized_case(&None).is_ok());
        for events in [Vec::new(), vec![Event::empty()]] {
            assert!(require_synchronized_case(&Some(events))
                .unwrap_err()
                .contains("PROJECTED_CASE_SYNC_REQUIRED"));
        }
    }

    #[test]
    fn canonical_field_admission_requires_explicit_owner_and_source() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let identity = Identity {
            case_id: "field-action".into(),
            analysis_id: "analysis".into(),
            config_revision: 0,
            visibility_revision: 0,
        };
        for (owner, generation, key, token) in [
            (None, Some(0), None, None),
            (Some(identity.clone()), None, None, None),
            (Some(identity.clone()), None, Some(String::new()), None),
            (Some(identity.clone()), None, Some("x".repeat(513)), None),
            (Some(identity.clone()), Some(0), None, Some("token".into())),
            (Some(identity.clone()), None, Some("case".into()), None),
            (
                Some(identity.clone()),
                None,
                Some("case".into()),
                Some(String::new()),
            ),
            (
                Some(identity),
                None,
                Some("case".into()),
                Some("x".repeat(129)),
            ),
        ] {
            let error = capture_field_text(&state, owner, generation, key, token)
                .err()
                .unwrap();
            assert!(error.starts_with("ANALYSIS_FIELD_ADMISSION:"), "{error}");
        }
    }

    // Exercise the same captured admission and prepared worker boundary as the
    // command, without a desktop window. Existing exact-helper tests cover the
    // scalar spellings, visibility exclusions and bounded serialization.
    fn read_field(state: &AppState, captured: FieldTextAdmission) -> Result<ExactField, String> {
        let FieldTextAdmission {
            admitted,
            events,
            receipt,
        } = captured;
        analysis_runtime::with(Some(admitted.clone()), || {
            admitted.validate(state)?;
            let events = admitted.prepare_visibility(events)?;
            let _publication = crate::catalog_read_guard()?;
            crate::page_projection::hydrate_projected_field(
                state,
                &admitted,
                &receipt,
                &RowHandle {
                    id: 0,
                    event_ref: "memory:0".into(),
                },
                "name",
                events.as_deref(),
            )
        })
    }

    #[test]
    fn canonical_field_capture_binds_dataset_and_synchronized_case_publications() {
        struct RestoreDataDir(Option<std::ffi::OsString>);
        impl Drop for RestoreDataDir {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        // Like the existing admission suite, run native tests with one thread
        // because the data directory is process-wide.
        let directory = tempfile::tempdir().unwrap();
        let _restore = RestoreDataDir(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
        crate::case_store::save_at(
            directory.path(),
            serde_json::json!({
                "cases": [{"id": "field-action"}], "active": null,
            }),
        )
        .unwrap();
        let identity = crate::analysis_context::snapshot("field-action")
            .unwrap()
            .identity();
        let state = state(directory.path());
        let event = {
            let mut source = state.source.write();
            let SourceData::Memory(events) = &mut *source else {
                panic!()
            };
            events[0].event_ref = "memory:0".into();
            events[0].clone()
        };
        let publisher = analysis_runtime::capture(
            &state,
            Some(identity.clone()),
            Some(0),
            analysis_runtime::Mode::Publish,
        )
        .unwrap();
        analysis_runtime::with(Some(publisher), || {
            let _source = crate::source_write_checked(&state).unwrap();
            let next = crate::source_publication::prepare_touch_locked(&state).unwrap();
            crate::source_publication::commit_touch_locked(&state, next);
        });
        let generation = crate::source_publication::snapshot(&state)
            .receipt
            .generation;
        let capture_dataset =
            || capture_field_text(&state, Some(identity.clone()), Some(generation), None, None);
        let field = read_field(&state, capture_dataset().unwrap()).unwrap();
        assert_eq!(field.canonical_text.as_deref(), Some("original"));
        assert_eq!(field.receipt.source_generation, Some(generation));
        assert!(field.receipt.case_content_token.is_none());

        let old_source = capture_dataset().unwrap();
        {
            let _source = state.source.write();
            let next = crate::source_publication::prepare_touch_locked(&state).unwrap();
            crate::source_publication::commit_touch_locked(&state, next);
        }
        assert_eq!(
            read_field(&state, old_source).unwrap_err(),
            analysis_runtime::STALE
        );
        assert_eq!(capture_dataset().err().unwrap(), analysis_runtime::STALE);

        let key = format!("field-action-{}", uuid::Uuid::new_v4());
        let capture_case = |token: &str| {
            capture_field_text(
                &state,
                Some(identity.clone()),
                None,
                Some(key.clone()),
                Some(token.into()),
            )
        };
        assert_eq!(
            capture_case("unpublished").err().unwrap(),
            crate::case_cache::MISS
        );
        let original_sync = tauri::async_runtime::block_on(crate::case_cache::case_sync(
            key.clone(),
            vec![event.clone()],
            Some(identity.clone()),
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&original_sync).unwrap(),
            serde_json::json!({"caseContentToken": original_sync.case_content_token}),
        );
        let field = read_field(
            &state,
            capture_case(&original_sync.case_content_token).unwrap(),
        )
        .unwrap();
        assert_eq!(field.canonical_text.as_deref(), Some("original"));
        assert_eq!(field.receipt.case_key.as_deref(), Some(key.as_str()));
        assert!(field.receipt.source_generation.is_none());

        let old_case = capture_case(&original_sync.case_content_token).unwrap();
        let original_token = old_case.receipt.case_content_token.clone();
        let mut replacement = event;
        replacement.name = "replacement".into();
        let replacement_sync = tauri::async_runtime::block_on(crate::case_cache::case_sync(
            key.clone(),
            vec![replacement],
            Some(identity.clone()),
        ))
        .unwrap();
        assert_eq!(
            read_field(&state, old_case).unwrap_err(),
            crate::case_cache::CHANGED
        );
        // Replacement happened before this new command's capture, with the
        // same owner/key/id/event_ref. Only the caller's sync token detects it.
        assert_eq!(
            capture_case(&original_sync.case_content_token)
                .err()
                .unwrap(),
            crate::case_cache::CHANGED,
        );
        let current = capture_case(&replacement_sync.case_content_token).unwrap();
        assert_ne!(current.receipt.case_content_token, original_token);
        assert_eq!(
            read_field(&state, current)
                .unwrap()
                .canonical_text
                .as_deref(),
            Some("replacement")
        );
    }

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        let mut event = Event::empty();
        event.source = "service".into();
        event.code = "200".into();
        event.name = "original".into();
        Arc::new(AppState {
            source: RwLock::new(SourceData::Memory(vec![event])),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(vec!["memory fixture".into()]),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: dir.join("codes.json"),
            system_codes_path: dir.join("system-codes.json"),
        })
    }

    #[test]
    fn catalog_receipts_cannot_capture_partial_memory_publication() {
        for system in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let state = state(directory.path());
            let source = state.source.write();
            let copied = state.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let text = r#"{"service":{"200":{"name":"updated","description":"description"}}}"#;
                let result = if system {
                    crate::publish_system_catalog(&copied, serde_json::from_str(text).unwrap())
                } else {
                    crate::save_codes_impl(&copied, text)
                };
                sender.send(result).unwrap();
            });
            // Wait until the new dictionary is published but Memory enrichment
            // is blocked by this test's source guard. No receipt may bind here.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let ready = if system {
                    !state.system_codes.read().sources.is_empty()
                } else {
                    !state.codes.read().sources.is_empty()
                };
                if ready {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "catalog publisher did not progress"
                );
                std::thread::yield_now();
            }
            assert!(crate::catalog_read_guard().is_err());
            assert!(receiver.try_recv().is_err());
            drop(source);
            receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
                .unwrap();
            worker.join().unwrap();
            let _catalog = crate::catalog_read_guard().unwrap();
            let source = state.source.read();
            let SourceData::Memory(events) = &*source else {
                panic!()
            };
            assert_eq!(events[0].name, "updated");
            assert_eq!(events[0].description, "description");
        }
    }
}
