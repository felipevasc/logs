//! Case-owned reference preparation. Source bytes are copied into an immutable
//! verified store before a short configuration CAS makes them discoverable.
use crate::{
    analysis_context::{self, Identity, ReferenceDescriptor, Snapshot},
    reference_store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};
use tauri::Emitter;

const SOURCE_BYTES: u64 = 8 << 20;
const ROWS: u64 = 100_000;
const RECORD_BYTES: usize = 512 << 10;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Inspection {
    name: String,
    format: String,
    content_sha256: String,
    columns: Vec<String>,
    row_count: u64,
    source_bytes: u64,
}

fn current(expected: &Identity) -> Result<Snapshot, String> {
    let snapshot = analysis_context::snapshot(&expected.case_id)?;
    if snapshot.analysis_id != expected.analysis_id
        || snapshot.config_revision != expected.config_revision
    {
        return Err(
            "A configuração do Caso mudou. Reabra as referências antes de continuar.".into(),
        );
    }
    Ok(snapshot)
}
fn owner(identity: &Identity) -> reference_store::Owner {
    reference_store::Owner {
        case_id: identity.case_id.clone(),
        analysis_id: identity.analysis_id.clone(),
    }
}
fn open_regular(path: &Path) -> Result<File, String> {
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Não foi possível abrir a referência: {e}"))?;
    if !metadata.is_file() {
        return Err("Selecione um arquivo JSONL regular.".into());
    }
    if metadata.len() > SOURCE_BYTES {
        return Err("Esta etapa aceita referências JSONL de até 8 MiB.".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A replaced path must not turn this worker into a blocking FIFO reader.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("Não foi possível abrir a referência: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Selecione um arquivo JSONL regular.".into());
    }
    Ok(file)
}
fn inspect(path: &Path) -> Result<Inspection, String> {
    crate::operations::check()?;
    let file = open_regular(path)?;
    let mut input = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut hash = Sha256::new();
    let mut source_bytes = 0u64;
    let mut row_count = 0u64;
    let mut columns: Option<Vec<String>> = None;
    loop {
        crate::operations::check()?;
        bytes.clear();
        // take bounds allocation even when a malformed record has no newline.
        let read = std::io::Read::take(&mut input, (RECORD_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        if read > RECORD_BYTES {
            return Err("Um registro de referência excede 512 KiB.".into());
        }
        source_bytes = source_bytes
            .checked_add(read as u64)
            .ok_or("Tamanho inválido.")?;
        if source_bytes > SOURCE_BYTES {
            return Err("Esta etapa aceita referências JSONL de até 8 MiB.".into());
        }
        hash.update(&bytes);
        row_count += 1;
        if row_count > ROWS {
            return Err("Esta etapa aceita até 100.000 registros de referência.".into());
        }
        let object =
            reference_store::parse_record(&bytes, row_count).map_err(|error| error.to_string())?;
        let names: Vec<_> = object.keys().cloned().collect();
        if names.is_empty()
            || names.len() > 1024
            || names.iter().any(|v| v.trim().is_empty() || v.len() > 4096)
        {
            return Err(format!("Colunas inválidas na linha {row_count}."));
        }
        if let Some(expected) = &columns {
            if &names != expected {
                return Err(format!(
                    "A linha {row_count} deve ter as mesmas colunas das demais linhas."
                ));
            }
        } else {
            columns = Some(names);
        }
    }
    let columns = columns.ok_or("A referência está vazia.")?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("Referência JSONL")
        .to_string();
    Ok(Inspection {
        name,
        format: "jsonl".into(),
        content_sha256: format!("{:x}", hash.finalize()),
        columns,
        row_count,
        source_bytes,
    })
}

async fn work<T: Send + 'static>(
    operation_id: Option<String>,
    app: tauri::AppHandle,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    crate::offload_operation(operation_id, move || {
        crate::operations::with_reporter(
            std::sync::Arc::new(move |progress| {
                let _ = app.emit("operation-progress", progress);
            }),
            f,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn reference_inspect(
    path: String,
    analysis_context: Identity,
    operation_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<Inspection, String> {
    work(operation_id, app, move || {
        current(&analysis_context)?;
        let result = inspect(Path::new(&path))?;
        current(&analysis_context)?;
        Ok(result)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportReceipt {
    #[serde(serialize_with = "crate::analysis_context::serialize_management_snapshot")]
    analysis_context: Snapshot,
    reference: ReferenceDescriptor,
    prepared: reference_store::PreparedReference,
}

fn import(
    expected: &Identity,
    path: &Path,
    inspection: Inspection,
    name: String,
    key_columns: Vec<String>,
) -> Result<ImportReceipt, String> {
    let current = current(expected)?;
    if inspection.format != "jsonl"
        || inspection.source_bytes > SOURCE_BYTES
        || inspection.row_count > ROWS
        || inspection.row_count == 0
    {
        return Err("Inspecione novamente a referência antes de importar.".into());
    }
    let name = name.trim();
    if name.is_empty() || name.len() > 4096 {
        return Err("Informe um nome de referência com até 4.096 bytes.".into());
    }
    let keys: BTreeSet<_> = key_columns.iter().collect();
    if keys.is_empty()
        || keys.len() != key_columns.len()
        || keys.iter().any(|v| !inspection.columns.contains(v))
    {
        return Err("Escolha colunas de chave distintas presentes na referência.".into());
    }
    let descriptor = ReferenceDescriptor {
        interpretation_version: 1, schema_version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        content_sha256: inspection.content_sha256,
        format: inspection.format,
        columns: inspection.columns,
        key_columns,
        duplicate_policy: "reject".into(),
    };
    let mut config = current.config;
    config.references.push(descriptor.clone());
    crate::analysis_commands::validate_config_for(expected, &config)?;
    let input = open_regular(path)?;
    let limits = reference_store::Limits {
        source_bytes: SOURCE_BYTES,
        rows: ROWS,
        record_bytes: RECORD_BYTES,
        ..Default::default()
    };
    let prepared = reference_store::prepare_jsonl(
        &crate::config_dir(),
        &owner(expected),
        &descriptor,
        input,
        limits,
        &|| crate::operations::check().is_err(),
    )
    .map_err(|e| e.to_string())?;
    if prepared.row_count != inspection.row_count
        || prepared.source_bytes != inspection.source_bytes
    {
        return Err("A referência mudou depois da inspeção. Inspecione novamente.".into());
    }
    crate::operations::check()?;
    let snapshot = analysis_context::update(expected, config)?;
    crate::operations::commit();
    Ok(ImportReceipt {
        analysis_context: snapshot,
        reference: descriptor,
        prepared,
    })
}

#[tauri::command]
pub(crate) async fn reference_import(
    path: String,
    inspection: Inspection,
    name: String,
    key_columns: Vec<String>,
    analysis_context: Identity,
    operation_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<ImportReceipt, String> {
    work(operation_id, app, move || {
        import(
            &analysis_context,
            Path::new(&path),
            inspection,
            name,
            key_columns,
        )
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    descriptor: ReferenceDescriptor,
    available: bool,
    row_count: Option<u64>,
    source_bytes: Option<u64>,
    reason: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Listing {
    analysis_context: Identity,
    references: Vec<Entry>,
}

#[tauri::command]
pub(crate) async fn reference_list(
    analysis_context: Identity,
    operation_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<Listing, String> {
    work(operation_id, app, move || {
        let snapshot = current(&analysis_context)?;
        let mut entries = Vec::with_capacity(snapshot.config.references.len());
        for descriptor in snapshot.config.references {
            crate::operations::check()?;
            let result = reference_store::open(
                &crate::config_dir(),
                &owner(&analysis_context),
                &descriptor,
                &|| crate::operations::check().is_err(),
            );
            let entry = match result {
                Ok(reader) => Entry {
                    descriptor,
                    available: true,
                    row_count: Some(reader.prepared().row_count),
                    source_bytes: Some(reader.prepared().source_bytes),
                    reason: None,
                },
                Err(error) => {
                    crate::operations::check()?;
                    Entry {
                        descriptor,
                        available: false,
                        row_count: None,
                        source_bytes: None,
                        reason: Some(error.to_string()),
                    }
                }
            };
            entries.push(entry);
        }
        current(&analysis_context)?;
        Ok(Listing {
            analysis_context,
            references: entries,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn reference_remove(
    reference_id: String,
    analysis_context: Identity,
    operation_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<crate::analysis_commands::MutationReceipt, String> {
    work(operation_id, app, move || {
        let snapshot = current(&analysis_context)?;
        let mut config = snapshot.config;
        config
            .references
            .retain(|reference| reference.id != reference_id);
        crate::analysis_commands::validate_config_for(&analysis_context, &config)?;
        let _prepared = crate::analysis_commands::prepare_config(&analysis_context, &config)?;
        crate::operations::check()?;
        let result = analysis_context::update(&analysis_context, config)?;
        crate::operations::commit();
        Ok(crate::analysis_commands::MutationReceipt {
            analysis_context: result,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn reference_save_lookup(
    name: String,
    lookup: crate::reference_lookup::Definition,
    analysis_context: Identity,
    operation_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<crate::analysis_commands::MutationReceipt, String> {
    work(operation_id, app, move || {
        let snapshot = current(&analysis_context)?;
        let mut config = snapshot.config;
        crate::reference_lookup::validate_reference(&lookup, &config.references)?;
        let at = config
            .derived_fields
            .iter()
            .position(|raw| raw.get("name").and_then(Value::as_str) == Some(name.as_str()));
        let id = at
            .and_then(|index| {
                config.derived_fields[index]
                    .get("id")
                    .and_then(Value::as_str)
            })
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let value = json!({"id":id,"name":name,"lookup":lookup});
        if let Some(at) = at {
            config.derived_fields[at] = value;
        } else {
            config.derived_fields.push(value);
        }
        crate::analysis_commands::validate_config_for(&analysis_context, &config)?;
        let _prepared = crate::analysis_commands::prepare_config(&analysis_context, &config)?;
        crate::operations::check()?;
        let result = analysis_context::update(&analysis_context, config)?;
        crate::operations::commit();
        Ok(crate::analysis_commands::MutationReceipt {
            analysis_context: result,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Environment(Option<std::ffi::OsString>);
    impl Drop for Environment {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    fn fixture() -> (tempfile::TempDir, Environment, Identity) {
        let dir = tempfile::tempdir().unwrap();
        let environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        crate::case_store::save_at(
            dir.path(),
            json!({"cases":[{"id":"references-case","name":"Reference case"}]}),
        )
        .unwrap();
        let identity = analysis_context::snapshot("references-case")
            .unwrap()
            .identity();
        (dir, environment, identity)
    }
    #[test]
    fn import_publishes_verified_typed_keys_and_rejects_stale_retry() {
        let (dir, _environment, identity) = fixture();
        let path = dir.path().join("reference.jsonl");
        let bytes = b"{\"key\":1,\"value\":null}\n{\"key\":\"1\",\"value\":\"string key\"}\n";
        std::fs::write(&path, bytes).unwrap();
        let inspection = inspect(&path).unwrap();
        let result = import(
            &identity,
            &path,
            inspection.clone(),
            "Labels".into(),
            vec!["key".into()],
        )
        .unwrap();
        assert_eq!(result.analysis_context.config.references.len(), 1);
        assert_eq!(
            result.analysis_context.config_revision,
            identity.config_revision + 1
        );
        let reader =
            reference_store::open(dir.path(), &owner(&identity), &result.reference, &|| false)
                .unwrap();
        assert_eq!(
            reader.lookup(&[json!(1)], "value").unwrap(),
            Some(Value::Null)
        );
        assert_eq!(
            reader.lookup(&[json!("1")], "value").unwrap(),
            Some(json!("string key"))
        );
        assert!(import(
            &identity,
            &path,
            inspection,
            "Retry".into(),
            vec!["key".into()]
        )
        .is_err());
        assert_eq!(
            analysis_context::snapshot(&identity.case_id)
                .unwrap()
                .config
                .references
                .len(),
            1
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn changed_bytes_and_duplicate_keys_leave_case_configuration_untouched() {
        let (dir, _environment, identity) = fixture();
        let path = dir.path().join("reference.jsonl");
        std::fs::write(&path, b"{\"key\":1,\"value\":\"first\"}\n").unwrap();
        let inspection = inspect(&path).unwrap();
        std::fs::write(&path, b"{\"key\":1,\"value\":\"other\"}\n").unwrap();
        assert!(import(
            &identity,
            &path,
            inspection,
            "Changed".into(),
            vec!["key".into()]
        )
        .is_err());
        std::fs::write(
            &path,
            b"{\"key\":1,\"value\":null}\n{\"key\":1,\"value\":false}\n",
        )
        .unwrap();
        assert!(import(
            &identity,
            &path,
            inspect(&path).unwrap(),
            "Duplicate".into(),
            vec!["key".into()]
        )
        .is_err());
        let snapshot = analysis_context::snapshot(&identity.case_id).unwrap();
        assert!(snapshot.config.references.is_empty());
        assert_eq!(snapshot.identity(), identity);
    }
    #[test]
    fn inspection_preserves_typed_columns_and_hashes_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reference.jsonl");
        let bytes = b"{\"key\":1,\"value\":null}\n{\"key\":\"1\",\"value\":{\"nested\":true}}\n";
        std::fs::write(&path, bytes).unwrap();
        let found = inspect(&path).unwrap();
        assert_eq!(found.row_count, 2);
        assert_eq!(found.source_bytes, bytes.len() as u64);
        assert_eq!(found.columns, vec!["key", "value"]);
        assert_eq!(found.content_sha256, format!("{:x}", Sha256::digest(bytes)));
    }
    #[test]
    fn inspection_rejects_schema_drift_and_oversized_unterminated_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reference.jsonl");
        for bytes in [
            b"{\"a\":1}\n{\"b\":2}\n".to_vec(),
            b"{\"key\":1,\"key\":2,\"value\":\"x\"}\n".to_vec(),
            vec![b'x'; RECORD_BYTES + 1],
            b"\n".to_vec(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(inspect(&path).is_err());
        }
    }
    #[test]
    fn inspection_accepts_reordered_columns_with_a_stable_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reference.jsonl");
        std::fs::write(&path, b"{\"a\":1,\"b\":2}\n{\"b\":3,\"a\":4}\n").unwrap();
        let result = inspect(&path).unwrap();
        assert_eq!(result.columns, vec!["a", "b"]);
        assert_eq!(result.row_count, 2);
    }
    #[cfg(unix)]
    #[test]
    fn reference_open_rejects_named_pipes_before_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reference.pipe");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is terminated and belongs to this test's private directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(open_regular(&path).is_err());
        assert!(inspect(&path).is_err());
    }
}
