//! Opt-in bounded rows. Existing Event APIs stay available until every exact
//! frontend action has an explicit hydration boundary.
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
