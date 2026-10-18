use log_learning::{DiagnosticKnowledge, MAX_DOCUMENT_BYTES};
use serde_json::{json, Value};
use std::io::Write;
fn read(path: &std::path::Path) -> Result<DiagnosticKnowledge, String> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err("Base diagnóstica excedeu 16 MiB".into());
    }
    let model: DiagnosticKnowledge =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    model.validate()?;
    Ok(model)
}
pub fn load() -> Result<DiagnosticKnowledge, String> {
    let path = crate::config_dir().join("diagnostic-knowledge.json");
    if path.exists() {
        read(&path)
    } else {
        let model: DiagnosticKnowledge =
            serde_json::from_str(include_str!("../../models/diagnostic-knowledge.json"))
                .map_err(|e| e.to_string())?;
        model.validate()?;
        Ok(model)
    }
}
pub fn store(document: Value) -> Result<Value, String> {
    if document.to_string().len() > MAX_DOCUMENT_BYTES {
        return Err("Base diagnóstica excedeu 16 MiB".into());
    }
    let model: DiagnosticKnowledge = serde_json::from_value(document).map_err(|e| e.to_string())?;
    model.validate()?;
    let directory = crate::config_dir();
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let path = directory.join("diagnostic-knowledge.json");
    // Preserve the last *valid* base. A corrupt current file must not prevent
    // recovery by importing a valid replacement, nor replace a valid backup.
    let previous_invalid = path.exists() && read(&path).is_err();
    if let Ok(old) = read(&path) {
        let mut backup = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut backup, &old).map_err(|e| e.to_string())?;
        backup.flush().map_err(|e| e.to_string())?;
        backup.as_file().sync_all().map_err(|e| e.to_string())?;
        backup
            .persist(directory.join("diagnostic-knowledge.previous.json"))
            .map_err(|e| e.error.to_string())?;
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, &model).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    Ok(
        json!({"status":"imported","revision":model.revision(),"episodes":model.episodes.len(),"evaluation":model.evaluation,"ranking_active":model.ranker.active,"previous_invalid":previous_invalid,"apply":"next_explicit_analysis"}),
    )
}
pub fn rollback() -> Result<Value, String> {
    let model = read(&crate::config_dir().join("diagnostic-knowledge.previous.json"))?;
    store(serde_json::to_value(model).map_err(|e| e.to_string())?)
}
