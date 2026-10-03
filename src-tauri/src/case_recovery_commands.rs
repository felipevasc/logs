//! Recovery IPC only stages a verified profile choice for the next process.
//! The running process keeps its fixed config_dir and active cache identities.
use crate::case_recovery::{profiles, Work};
use serde::Deserialize;
use std::{path::Path, sync::Arc};
use tauri::{AppHandle, Emitter};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PrepareRestartRequest {
    pub recovery_id: String,
    pub request_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReturnOriginalRequest {
    pub request_id: String,
}
async fn recovery_work<T, F>(
    operation_id: Option<String>,
    app: AppHandle,
    run: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Path, &Path, &Work<'_>) -> Result<T, String> + Send + 'static,
{
    crate::offload_operation(operation_id, move || {
        crate::operations::with_reporter(
            Arc::new(move |progress| {
                let _ = app.emit("operation-progress", progress);
            }),
            || {
                let token = crate::operations::current_token();
                let cancelled = || token.cancelled();
                let progress = |_phase: &str, completed: u64, total: Option<u64>| {
                    crate::operations::report_progress(
                        "recuperação",
                        "verification",
                        "Verificando o perfil de recuperação",
                        completed.min(usize::MAX as u64) as usize,
                        total.unwrap_or(0).min(usize::MAX as u64) as usize,
                        "bytes",
                        0,
                    );
                };
                let work = Work {
                    cancelled: &cancelled,
                    progress: &progress,
                };
                run(&crate::base_config_dir(), &crate::config_dir(), &work)
            },
        )
    })
    .await?
}
#[tauri::command]
pub(crate) async fn case_recovery_status(
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<profiles::ProfileStatus, String> {
    recovery_work(operation_id, app, profiles::status).await
}
#[tauri::command]
pub(crate) async fn case_recovery_prepare_restart(
    request: PrepareRestartRequest,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<profiles::PreparedProfile, String> {
    recovery_work(operation_id, app, move |base, current, work| {
        profiles::prepare_restart(
            base,
            current,
            &request.recovery_id,
            &request.request_id,
            work,
        )
    })
    .await
}
#[tauri::command]
pub(crate) async fn case_recovery_return_original(
    request: ReturnOriginalRequest,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<profiles::PreparedOriginal, String> {
    recovery_work(operation_id, app, move |base, current, work| {
        profiles::prepare_original_restart(base, current, &request.request_id, work)
    })
    .await
}
