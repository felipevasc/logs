//! Native transfer IPC. Import returns its retained receipt directly so the
//! acknowledged Case views stay charged until Tauri serialization completes.
use crate::{
    case_evidence::CommittedImport,
    case_portable_native::{self as portable, ExportReceipt, ExportRequest, ImportRequest},
};

#[tauri::command]
pub(crate) async fn case_import_native(
    request: ImportRequest,
    operation_id: Option<String>,
) -> Result<CommittedImport, String> {
    crate::offload_operation(operation_id, move || {
        portable::import_at(&crate::config_dir(), request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_export_native(
    request: ExportRequest,
    operation_id: Option<String>,
) -> Result<ExportReceipt, String> {
    crate::offload_operation(operation_id, move || {
        portable::export_at(&crate::config_dir(), request)
    })
    .await?
}
