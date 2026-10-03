//! Portable Case orchestration. Imports prepare immutable files first and expose
//! a local token; only the ordinary Case-save transaction publishes ownership.
use crate::{
    analysis_context::Snapshot,
    case_archive_format::{self as format, Entry, EntryKind, Source},
    case_archive_references::{self as references, ReferenceSet},
    exclusion_store::{self as ledger, Budget, PortableLedger, PreparedPortable, Work},
};
use parking_lot::Mutex;
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

pub(crate) const TOKEN_FIELD: &str = "portableImportToken";
const MAX_CASES: usize = 1024;
const MAX_PENDING_BYTES: usize = 128 << 20;
pub(crate) const MASKED_HISTORY: &str = "O arquivo portátil completo inclui a proveniência original das exclusões, que não pode ser mascarada nesta versão. Para transferi-lo, desmarque ‘Ocultar senhas e tokens nos textos’. Para compartilhar textos ocultando esses dados, exporte um relatório. O destino foi preservado.";
pub(crate) const MASKED_REFERENCES: &str = "O arquivo portátil inclui referências com seus bytes e esquema originais, que não podem ser mascarados sem alterar os resultados das consultas. Para transferi-lo, desmarque ‘Ocultar senhas e tokens nos textos’. Para compartilhar textos ocultando esses dados, exporte um relatório. O destino foi preservado.";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Metadata {
    pub schema_version: u32,
    pub ledgers: Vec<PortableLedger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_sets: Vec<ReferenceSet>,
}

struct Pending {
    root: PathBuf,
    prepared: PreparedPortable,
    reference_sources: Vec<crate::reference_store::PortableSource>,
    metadata_bytes: usize,
}
static PENDING: LazyLock<Mutex<HashMap<String, Arc<Pending>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ByteCounter {
    bytes: usize,
    limit: usize,
}
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(
                "Metadados portáteis excedem o orçamento permitido.",
            ));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn work() -> Work<'static> {
    Work {
        cancelled: &crate::operations::cancelled,
        progress: &report_ledger_progress,
    }
}
fn report_ledger_progress(phase: &str, completed: u64, total: Option<u64>) {
    let (id, label, unit) = match phase {
        "verifying" => (
            "portable_verify",
            "Conferindo exclusões do arquivo portátil",
            "bytes",
        ),
        "staging" | "staged" => (
            "portable_prepare",
            "Preparando exclusões do Caso",
            "registros",
        ),
        "validating_restore" => (
            "portable_restore",
            "Conferindo restaurações parciais",
            "registros",
        ),
        _ => return,
    };
    crate::operations::report_progress(
        "import_investigation",
        id,
        label,
        completed.min(usize::MAX as u64) as usize,
        total.unwrap_or(0).min(usize::MAX as u64) as usize,
        unit,
        0,
    );
}
fn cases(data: &Value) -> Result<&Vec<Value>, String> {
    let cases = data
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("Arquivo sem investigações.")?;
    if cases.is_empty() || cases.len() > MAX_CASES {
        return Err("O arquivo portátil deve conter de 1 a 1.024 Casos.".into());
    }
    let mut ids = HashSet::new();
    for case in cases {
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Caso sem identificador.")?;
        if id.is_empty() || id.len() > 4096 || !ids.insert(id) {
            return Err("Identificador de Caso portátil inválido ou duplicado.".into());
        }
    }
    Ok(cases)
}
fn snapshot(case: &Value) -> Result<Snapshot, String> {
    let value = case
        .get("analysisContext")
        .ok_or("Caso portátil sem configuração autoritativa.")?;
    let snapshot: Snapshot =
        serde_json::from_value(value.clone()).map_err(|_| "Configuração portátil inválida.")?;
    if snapshot.schema_version != 1
        || Some(snapshot.case_id.as_str()) != case.get("id").and_then(Value::as_str)
        || uuid::Uuid::parse_str(&snapshot.analysis_id).is_err()
    {
        return Err("Identidade da configuração portátil inválida.".into());
    }
    crate::analysis_context::validate(&snapshot.config)?;
    Ok(snapshot)
}

struct DocumentWriter<W> {
    inner: W,
    bytes: u64,
    digest: Sha256,
}
impl<W: Write> Write for DocumentWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if crate::operations::cancelled() {
            return Err(std::io::Error::other("Operação cancelada."));
        }
        if bytes.len() as u64 > format::MAX_DOCUMENT_BYTES.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("O documento do Caso excede 64 MiB."));
        }
        let count = self.inner.write(bytes)?;
        self.bytes += count as u64;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn document_source(file: &mut File, data: &Value) -> Result<Source, String> {
    let mut writer = DocumentWriter {
        inner: std::io::BufWriter::new(&mut *file),
        bytes: 0,
        digest: Sha256::new(),
    };
    serde_json::to_writer(&mut writer, data).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    let bytes = writer.bytes;
    let digest = format!("{:x}", writer.digest.clone().finalize());
    drop(writer);
    Ok(Source {
        entry: Entry {
            kind: EntryKind::Document,
            bytes,
            sha256: digest,
        },
        file: file.try_clone().map_err(|e| e.to_string())?,
    })
}
pub(crate) fn safe_export_destination(root: &Path, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    // A selected restored profile remains inside the application's original
    // storage namespace. Export must preserve its base and sibling profiles.
    let managed_profile = root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            uuid::Uuid::parse_str(name)
                .ok()
                .filter(|id| id.to_string() == name)
        })
        .is_some()
        && root.parent().and_then(Path::file_name)
            == Some(std::ffi::OsStr::new("case-profiles-v1"));
    let protected = if managed_profile {
        root.parent()
            .and_then(Path::parent)
            .ok_or("Perfil gerenciado inválido.")?
    } else {
        root.as_path()
    };
    if parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(protected)
    {
        return Err("Escolha um destino fora do armazenamento interno da aplicação para preservar os originais.".into());
    }
    Ok(())
}

/// Legacy exporters accept caller-owned record Values. Once an owner has
/// native authority or restore protection, those Values cannot supply its
/// portable evidence again.
/// Call only inside the same read transaction that captures Case contexts.
pub(crate) fn require_legacy_export(
    conn: &Connection,
    data: &Value,
) -> Result<Vec<String>, String> {
    let ids = data
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("Arquivo sem investigações.")?
        .iter()
        .map(|case| {
            case.get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "Caso sem identificador.".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    require_legacy_ids(conn, &ids)?;
    Ok(ids)
}
fn require_legacy_ids(conn: &Connection, ids: &[String]) -> Result<(), String> {
    let mut tables = Vec::new();
    for table in [
        "native_evidence_protected",
        "native_evidence_cases",
        "case_recovery_protected",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            tables.push(table);
        }
    }
    if tables.is_empty() {
        return Ok(());
    }
    for id in ids {
        crate::operations::check()?;
        for table in &tables {
            // Table names above are fixed application constants, never input.
            let native: bool = conn
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE case_id=?1)"),
                    [id],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if native {
                return Err("CASE_NATIVE_EXPORT_REQUIRED: Este Caso preserva evidências nativas. Reabra a investigação e use a exportação nativa para manter os registros originais; o destino foi preservado.".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {pub(crate) static BEFORE_LEGACY_EXPORT_RECHECK:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=std::cell::RefCell::new(None);}
/// Recheck after slow serialization/file copying, when adoption may have
/// committed since the old export released its initial read transaction.
pub(crate) fn recheck_legacy_export(root: &Path, ids: &[String]) -> Result<(), String> {
    #[cfg(test)]
    BEFORE_LEGACY_EXPORT_RECHECK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    crate::operations::check()?;
    let conn = Connection::open_with_flags(
        root.join("investigations.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    require_legacy_ids(&conn, ids)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())
}

pub(crate) fn export_at(
    root: &Path,
    path: &Path,
    mut data: Value,
    mask: bool,
) -> Result<(), String> {
    crate::case_images::validate_document(&data)?;
    cases(&data)?;
    let conn = crate::case_store::context_connection(root)?;
    safe_export_destination(root, path)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let legacy_ids = require_legacy_export(&conn, &data)?;
    let mut captured = Vec::new();
    let mut snapshots = Vec::new();
    let mut metadata_budget = ByteCounter {
        bytes: 0,
        limit: format::MAX_MANIFEST_BYTES as usize,
    };
    let mut captured_payloads = 0usize;
    for case in data["cases"].as_array_mut().unwrap() {
        crate::operations::check()?;
        crate::analysis_context::attach(&conn, case)?;
        case.as_object_mut().unwrap().remove(TOKEN_FIELD);
        let snapshot = snapshot(case)?;
        if mask && snapshot.visibility_revision != 0 {
            return Err(MASKED_HISTORY.into());
        }
        if mask && !snapshot.config.references.is_empty() {
            return Err(MASKED_REFERENCES.into());
        }
        let captured_case = ledger::capture_portable(&conn, root, &snapshot, &Budget::default())?;
        serde_json::to_writer(&mut metadata_budget, &captured_case.manifest)
            .map_err(|e| e.to_string())?;
        captured_payloads = captured_payloads
            .checked_add(captured_case.files.len())
            .ok_or("Quantidade de payloads portáteis excessiva.")?;
        if captured_payloads > format::MAX_PAYLOADS {
            return Err("O arquivo portátil excede 4.096 payloads.".into());
        }
        captured.push(captured_case);
        snapshots.push(snapshot);
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    // File leases remain; large exports never retain a WAL read lock. Reference
    // hashing uses immutable versions bound to the captured config and owner.
    drop(conn);
    let mut captured_references = Vec::new();
    if snapshots
        .iter()
        .any(|context| !context.config.references.is_empty())
    {
        let mut count = 0usize;
        for context in &snapshots {
            count = count
                .checked_add(context.config.references.len())
                .ok_or("Quantidade de referências excessiva.")?;
            if count > format::MAX_REFERENCES {
                return Err("O arquivo portátil excede 4.096 referências.".into());
            }
            let captured = references::capture(root, context)?;
            serde_json::to_writer(&mut metadata_budget, &captured.manifest)
                .map_err(|e| e.to_string())?;
            captured_references.push(captured);
        }
    }
    if mask
        && captured
            .iter()
            .any(|c| c.manifest.identity.visibility_revision != 0 || !c.manifest.batches.is_empty())
    {
        return Err(MASKED_HISTORY.into());
    }
    if mask {
        crate::workspace::redact_value(&mut data);
        for (case, captured_case) in cases(&data)?.iter().zip(&captured) {
            if snapshot(case)?.identity() != captured_case.manifest.identity {
                return Err("O mascaramento alteraria um identificador do Caso. Exporte um relatório mascarado ou desmarque a opção para transferir o Caso completo.".into());
            }
        }
    }
    data.as_object_mut().unwrap().remove("imageAssets");
    data.as_object_mut().unwrap().remove("revision");
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    let mut sources = vec![document_source(temporary.as_file_mut(), &data)?];
    sources.extend(crate::case_images::portable_image_sources(root, &data)?);
    for captured_case in &captured {
        for payload in &captured_case.files {
            sources.push(Source {
                entry: Entry {
                    kind: EntryKind::Exclusion(payload.meta().id().into()),
                    bytes: payload.meta().bytes(),
                    sha256: payload.meta().sha256().into(),
                },
                file: payload.open_reader()?,
            });
        }
    }
    for captured in &captured_references {
        for (id, reference) in &captured.files {
            sources.push(Source {
                entry: Entry {
                    kind: EntryKind::Reference(id.clone()),
                    bytes: reference.prepared().source_bytes,
                    sha256: reference.prepared().version.content_sha256.clone(),
                },
                file: reference.source_reader().map_err(|e| e.to_string())?,
            });
        }
    }
    let metadata = Metadata {
        schema_version: if captured_references.is_empty() { 1 } else { 2 },
        ledgers: captured.iter().map(|c| c.manifest.clone()).collect(),
        reference_sets: captured_references
            .iter()
            .map(|c| c.manifest.clone())
            .collect(),
    };
    format::write_guarded(
        path,
        &metadata,
        &mut sources,
        &crate::operations::cancelled,
        &mut |_| Ok(()),
        &|| recheck_legacy_export(root, &legacy_ids),
    )?;
    crate::operations::commit();
    Ok(())
}

pub(crate) fn payload_metadata(
    ledger: &PortableLedger,
) -> impl Iterator<Item = &ledger::PayloadMeta> {
    ledger
        .batches
        .iter()
        .flat_map(|batch| std::iter::once(batch.batch.payload()).chain(batch.masks.iter()))
}

pub(crate) fn import_at(root: &Path, path: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let archive = format::read::<Metadata>(path, &root, &crate::operations::cancelled)?;
    if ![1, 2].contains(&archive.metadata.schema_version) {
        return Err("O manifesto do Caso exige uma versão mais recente.".into());
    }
    let mut data: Value = serde_json::from_reader(std::io::BufReader::new(
        File::open(archive.path(0)?).map_err(|e| e.to_string())?,
    ))
    .map_err(|e| format!("Documento portátil inválido: {e}"))?;
    crate::case_images::validate_document(&data)?;
    let cases = cases(&data)?;
    if data.get("imageAssets").is_some() || cases.len() != archive.metadata.ledgers.len() {
        return Err("Documento e manifesto portátil não correspondem.".into());
    }
    let mut foreign_ledgers = BTreeMap::new();
    let mut analyses = HashSet::new();
    let mut expected_payloads = BTreeMap::new();
    for ledger in &archive.metadata.ledgers {
        if foreign_ledgers
            .insert(ledger.identity.case_id.clone(), ledger)
            .is_some()
            || !analyses.insert(&ledger.identity.analysis_id)
        {
            return Err("Identidade de Caso duplicada no manifesto portátil.".into());
        }
        for payload in payload_metadata(ledger) {
            if expected_payloads
                .insert(payload.id().to_owned(), payload)
                .is_some()
            {
                return Err("Um payload portátil pertence a mais de um lote ou Caso.".into());
            }
        }
    }
    let mut images = BTreeMap::new();
    let mut payloads = BTreeMap::new();
    let mut reference_paths = BTreeMap::new();
    for (index, entry) in archive.entries.iter().enumerate().skip(1) {
        match &entry.kind {
            EntryKind::Image(id) => {
                images.insert(id.clone(), archive.path(index)?);
            }
            EntryKind::Exclusion(id) => {
                let expected = expected_payloads
                    .get(id)
                    .ok_or("Payload sem proprietário no manifesto.")?;
                if entry.bytes != expected.bytes() || entry.sha256 != expected.sha256() {
                    return Err("Metadados do payload portátil não conferem.".into());
                }
                payloads.insert(id.clone(), archive.path(index)?);
            }
            EntryKind::Reference(id) => {
                reference_paths.insert(id.clone(), archive.path(index)?);
            }
            EntryKind::Document => return Err("Documento portátil duplicado.".into()),
        }
    }
    if payloads.len() != expected_payloads.len() {
        return Err("O arquivo portátil não contém todos os payloads de exclusão.".into());
    }
    let mut foreign = Vec::new();
    for case in cases {
        let context = snapshot(case)?;
        let ledger = foreign_ledgers
            .get(&context.case_id)
            .ok_or("Caso sem manifesto de visibilidade.")?;
        if ledger.identity != context.identity() {
            return Err("A configuração e a visibilidade do Caso não correspondem.".into());
        }
        foreign.push(context);
    }
    let foreign_references = references::validate(
        archive.metadata.schema_version,
        &archive.metadata.reference_sets,
        &mut foreign,
        &archive.entries,
    )?;
    crate::case_images::validate_portable_images(&mut data, &images)?;
    {
        let pending = PENDING.lock();
        if pending.len().saturating_add(foreign.len()) > MAX_CASES {
            return Err(
                "Há muitas importações pendentes. Salve os Casos antes de importar outro arquivo."
                    .into(),
            );
        }
    }
    let mut prepared = Vec::new();
    let mut mapping = BTreeMap::new();
    for context in foreign {
        crate::operations::check()?;
        let ledger = foreign_ledgers[&context.case_id];
        let own_payloads = payload_metadata(ledger)
            .map(|meta| {
                Ok((
                    meta.id().to_owned(),
                    payloads
                        .get(meta.id())
                        .ok_or("Payload portátil ausente.")?
                        .clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let local_id = format!("c-import-{}", uuid::Uuid::new_v4());
        let mut metadata_budget = ByteCounter {
            bytes: 0,
            limit: MAX_PENDING_BYTES,
        };
        let reference_set = &foreign_references[&context.case_id];
        serde_json::to_writer(&mut metadata_budget, &(&context, ledger, reference_set))
            .map_err(|e| e.to_string())?;
        let mut staged = ledger::prepare_portable(
            &root,
            &context,
            ledger,
            &local_id,
            &own_payloads,
            &Budget::default(),
            &work(),
        )?;
        let reference_sources =
            references::prepare(&root, staged.snapshot(), reference_set, &reference_paths)?;
        references::interpretation_diagnostics(reference_set, &mut staged, false)?;
        references::preflight(&root, &mut staged)?;
        // Include readiness diagnostics added by preflight in the pending cap.
        serde_json::to_writer(
            &mut metadata_budget,
            &staged.snapshot().migration_diagnostics,
        )
        .map_err(|e| e.to_string())?;
        let metadata_bytes = metadata_budget.bytes;
        mapping.insert(
            context.case_id,
            (local_id, staged.snapshot().analysis_id.clone()),
        );
        prepared.push(Arc::new(Pending {
            root: root.clone(),
            prepared: staged,
            reference_sources,
            metadata_bytes,
        }));
    }
    crate::case_images::publish_portable_images(&root, &images)?;
    for case in data["cases"].as_array_mut().unwrap() {
        let (local_id, token) = &mapping[case["id"].as_str().unwrap()];
        let object = case.as_object_mut().unwrap();
        object.insert("id".into(), Value::String(local_id.clone()));
        // Frontend ownership admission must adopt the committed save receipt,
        // not a foreign identity or a context that has not been published yet.
        object.remove("analysisContext");
        object.insert(TOKEN_FIELD.into(), Value::String(token.clone()));
    }
    for field in ["active", "activeCaseId"] {
        if let Some((id, _)) = data
            .get(field)
            .and_then(Value::as_str)
            .and_then(|id| mapping.get(id))
        {
            data[field] = Value::String(id.clone());
        }
    }
    data.as_object_mut().unwrap().remove("revision");
    let mut pending = PENDING.lock();
    let bytes = pending
        .values()
        .map(|p| p.metadata_bytes)
        .chain(prepared.iter().map(|p| p.metadata_bytes))
        .try_fold(0usize, |sum, size| sum.checked_add(size))
        .ok_or("Metadados de importação excessivos.")?;
    if bytes > MAX_PENDING_BYTES || pending.len().saturating_add(prepared.len()) > MAX_CASES {
        return Err("As importações pendentes excedem o orçamento local. Salve os Casos antes de importar outro arquivo.".into());
    }
    crate::operations::check()?;
    for entry in prepared {
        pending.insert(entry.prepared.snapshot().analysis_id.clone(), entry);
    }
    crate::operations::commit();
    Ok(data)
}

fn stored_snapshot(conn: &Connection, case_id: &str) -> Result<Option<Snapshot>, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1)",
            [case_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(None);
    }
    let mut case = serde_json::json!({"id":case_id});
    crate::analysis_context::attach(conn, &mut case)?;
    Ok(Some(
        serde_json::from_value(case["analysisContext"].clone())
            .map_err(|_| "Configuração persistida inválida.")?,
    ))
}

/// Called after the Case body is inserted, within the same transaction. A token
/// binds one local Case and analysis, never a caller-supplied path or snapshot.
pub(crate) fn consume_prepared(
    root: &Path,
    tx: &Transaction<'_>,
    case: &Value,
) -> Result<Option<Snapshot>, String> {
    let Some(value) = case.get(TOKEN_FIELD) else {
        return Ok(None);
    };
    let token = value
        .as_str()
        .ok_or("Token de importação portátil inválido.")?;
    let case_id = case["id"].as_str().ok_or("Caso sem identificador.")?;
    if uuid::Uuid::parse_str(token).is_err() {
        return Err("Token de importação portátil inválido.".into());
    }
    if let Some(current) = stored_snapshot(tx, case_id)? {
        if current.analysis_id != token {
            return Err("O Caso já possui outra identidade; a importação foi interrompida sem sobrescrevê-lo.".into());
        }
        return Ok(Some(current)); // Retry after COMMIT: retain every newer revision.
    }
    let pending = PENDING.lock().get(token).cloned().ok_or("A preparação desta importação não está mais disponível. Importe o arquivo portátil novamente; nenhum Caso foi publicado.")?;
    if pending.root != root.canonicalize().map_err(|e| e.to_string())?
        || pending.prepared.snapshot().case_id != case_id
    {
        return Err("O token de importação pertence a outro Caso ou armazenamento.".into());
    }
    for reference in &pending.reference_sources {
        reference.validate_unchanged().map_err(|e| e.to_string())?;
    }
    ledger::insert_prepared(tx, &pending.prepared).map(Some)
}

/// Only the outer Case transaction's successful COMMIT may retire preparations.
pub(crate) fn retire_committed(root: &Path, data: &Value) {
    let Ok(root) = root.canonicalize() else {
        return;
    };
    let mut pending = PENDING.lock();
    for case in data
        .get("cases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(token) = case.get(TOKEN_FIELD).and_then(Value::as_str) else {
            continue;
        };
        if pending.get(token).is_some_and(|p| {
            p.root == root
                && Some(p.prepared.snapshot().case_id.as_str())
                    == case.get("id").and_then(Value::as_str)
        }) {
            pending.remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis_context::Config;
    use serde_json::json;

    #[test]
    fn managed_profile_export_destination_preserves_base_and_sibling_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().join("application");
        let profiles = base.join("case-profiles-v1");
        let selected = profiles.join(uuid::Uuid::new_v4().to_string());
        let nil_profile = profiles.join(uuid::Uuid::nil().to_string());
        let sibling = profiles.join(uuid::Uuid::new_v4().to_string());
        let outside = directory.path().join("exports");
        for path in [&selected, &nil_profile, &sibling, &outside] {
            std::fs::create_dir_all(path).unwrap();
        }
        for selected in [&selected, &nil_profile] {
            for path in [
                base.join("investigations.sqlite3"),
                sibling.join("investigations.sqlite3"),
                selected.join("case.json"),
            ] {
                assert!(
                    safe_export_destination(selected, &path).is_err(),
                    "{selected:?}: {path:?}"
                );
            }
            let exported_path = outside
                .join(selected.file_name().unwrap())
                .with_extension("licase");
            safe_export_destination(selected, &exported_path).unwrap();
            create(selected, "managed-case");
            exported(selected, &exported_path);
            assert!(format::is_archive(&exported_path).unwrap());
        }
        let ordinary = profiles.join("not-a-canonical-profile");
        std::fs::create_dir_all(&ordinary).unwrap();
        safe_export_destination(&ordinary, &base.join("outside-this-test-root.licase")).unwrap();
        assert!(safe_export_destination(&ordinary, &ordinary.join("inside.licase")).is_err());
    }

    fn create(root: &Path, id: &str) {
        let context = Snapshot {
            schema_version: 1,
            case_id: id.into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            config_revision: 0,
            visibility_revision: 0,
            config: Config {
                derived_fields: vec![
                    json!({"name":"portable","source":"message","pattern":"(portable)"}),
                ],
                references: vec![],
            },
            interpretation: None,
            migration_diagnostics: vec![],
            legacy_raw: None,
        };
        crate::case_store::save_at(root, json!({"active":id,"cases":[{"id":id,"name":"Portable fixture","analysisContext":context,"items":[{"id":"evidence","note":"Original evidence"}]}]})).unwrap();
    }
    fn current(root: &Path, id: &str) -> Snapshot {
        let data = crate::case_store::load_at(root).unwrap();
        let case = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap();
        snapshot(case).unwrap()
    }
    fn admission(root: &Path, id: &str) -> ledger::Admission {
        ledger::Admission {
            analysis: current(root, id).identity(),
            source_receipt: json!({"fixture":"portable"}),
        }
    }
    fn exclude(
        root: &Path,
        case_id: &str,
        purpose: ledger::Purpose,
        offsets: &[u64],
    ) -> ledger::Receipt {
        let source: ledger::SourceDescriptor = serde_json::from_value(json!({"version":"portable-source","recordSpace":"portable-records","label":"fixture.log","eventRefPrefix":"portable-source"})).unwrap();
        let staged = ledger::stage(
            root,
            admission(root, case_id),
            purpose,
            &[source],
            offsets.iter().map(|offset| {
                Ok(ledger::InputMember {
                    source_index: 0,
                    locator: ledger::Locator::ByteOffset(*offset),
                    event_ref: format!("portable-source:{offset}"),
                })
            }),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        ledger::publish(
            root,
            staged,
            "Fixture batch",
            "Test exclusion",
            json!({"kind":"selection"}),
            &Budget::default(),
            &work(),
        )
        .unwrap()
    }
    fn offsets(root: &Path, id: &str) -> Vec<u64> {
        let view = ledger::visibility(
            root,
            &current(root, id).identity(),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        let mut found = Vec::new();
        view.visit(&work(), |member| {
            if let ledger::Locator::ByteOffset(offset) = member.key.locator {
                found.push(offset);
            }
            Ok(true)
        })
        .unwrap();
        found
    }
    fn exported(root: &Path, target: &Path) {
        let mut data = crate::case_store::load_at(root).unwrap();
        // Every fixture export also exercises a stale/untrusted UI context.
        data["cases"][0]["analysisContext"] = Value::Null;
        export_at(root, target, data, false).unwrap();
    }
    fn merge(root: &Path, imported: &Value) -> Value {
        let mut data = crate::case_store::load_at(root).unwrap();
        data["cases"]
            .as_array_mut()
            .unwrap()
            .extend(imported["cases"].as_array().unwrap().iter().cloned());
        data
    }

    #[test]
    fn portable_cases_preserve_overlap_masks_history_and_local_isolation() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "case-a");
        create(target.path(), "case-a");
        let a = exclude(
            source.path(),
            "case-a",
            ledger::Purpose::Exclude,
            &[1, 2, 3],
        );
        exclude(source.path(), "case-a", ledger::Purpose::Exclude, &[3, 4]);
        exclude(
            source.path(),
            "case-a",
            ledger::Purpose::RestoreSelection {
                batch_id: a.batch_id,
            },
            &[2, 3],
        );
        let inactive = exclude(source.path(), "case-a", ledger::Purpose::Exclude, &[8, 9]);
        ledger::restore_batch(
            source.path(),
            &admission(source.path(), "case-a"),
            &inactive.batch_id,
            &work(),
        )
        .unwrap();
        assert_eq!(offsets(source.path(), "case-a"), vec![1, 3, 4]);
        let original = crate::case_store::load_at(source.path()).unwrap();
        let existing = current(target.path(), "case-a");
        let path = exports.path().join("fixture.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        assert_ne!(id, "case-a");
        assert!(imported["cases"][0].get("analysisContext").is_none());
        assert_eq!(
            crate::case_store::load_at(target.path()).unwrap()["cases"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let mut data = merge(target.path(), &imported);
        data["cases"][0]["name"] = json!("Unsaved local edit survives merge");
        let receipt = crate::case_store::save_at(target.path(), data).unwrap();
        let local = current(target.path(), id);
        let foreign = current(source.path(), "case-a");
        assert_ne!(local.analysis_id, foreign.analysis_id);
        assert_eq!(local.config, foreign.config);
        assert_eq!(current(target.path(), "case-a"), existing);
        assert_eq!(offsets(target.path(), id), vec![1, 3, 4]);
        let foreign_batches = ledger::list(source.path(), &foreign.identity(), None, 100).unwrap();
        let local_batches = ledger::list(target.path(), &local.identity(), None, 100).unwrap();
        assert_eq!(local_batches.len(), 3);
        assert_eq!(local_batches.iter().filter(|b| !b.active).count(), 1);
        assert!(local_batches
            .iter()
            .all(|local| foreign_batches.iter().all(|old| old.id != local.id)));
        assert!(ledger::restore_batch(
            target.path(),
            &admission(target.path(), "case-a"),
            &local_batches[0].id,
            &work()
        )
        .is_err());
        assert!(receipt["analysisContexts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["analysisId"] == local.analysis_id));
        let saved = crate::case_store::load_at(target.path()).unwrap();
        assert_eq!(
            saved["cases"][0]["name"],
            "Unsaved local edit survives merge"
        );
        assert!(saved["cases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get(TOKEN_FIELD).is_none()));
        assert_eq!(crate::case_store::load_at(source.path()).unwrap(), original);
    }

    #[test]
    fn portable_masked_exclusion_export_preserves_existing_target() {
        let source = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "case-a");
        exclude(source.path(), "case-a", ledger::Purpose::Exclude, &[1]);
        let path = exports.path().join("fixture.licase");
        std::fs::write(&path, b"Previous export").unwrap();
        let error = export_at(
            source.path(),
            &path,
            crate::case_store::load_at(source.path()).unwrap(),
            true,
        )
        .unwrap_err();
        assert!(error.contains("desmarque"));
        assert_eq!(std::fs::read(path).unwrap(), b"Previous export");
    }

    #[test]
    fn portable_masked_case_without_exclusions_keeps_masking_available() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        let mut data = crate::case_store::load_at(source.path()).unwrap();
        data["cases"][0]["items"][0]["note"] = json!("password=synthetic-secret");
        let path = exports.path().join("fixture.licase");
        export_at(source.path(), &path, data, true).unwrap();
        let imported = import_at(target.path(), &path).unwrap();
        let note = imported["cases"][0]["items"][0]["note"].as_str().unwrap();
        assert!(note.contains("[oculto]"));
        assert!(!note.contains("synthetic-secret"));
        crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
    }

    #[test]
    fn portable_save_rolls_back_and_retry_preserves_newer_visibility() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        create(target.path(), "existing");
        let path = exports.path().join("fixture.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        let token = imported["cases"][0][TOKEN_FIELD].as_str().unwrap();
        let before = crate::case_store::load_at(target.path()).unwrap();
        let mut data = merge(target.path(), &imported);
        let conn = crate::case_store::connect(target.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_portable BEFORE INSERT ON case_analysis BEGIN SELECT RAISE(ABORT,'simulated transaction failure'); END;").unwrap();
        assert!(crate::case_store::save_at(target.path(), data.clone()).is_err());
        assert_eq!(crate::case_store::load_at(target.path()).unwrap(), before);
        assert!(PENDING.lock().contains_key(token));
        conn.execute_batch("DROP TRIGGER reject_portable").unwrap();
        let receipt = crate::case_store::save_at(target.path(), data.clone()).unwrap();
        assert!(!PENDING.lock().contains_key(token));
        let updated = exclude(target.path(), id, ledger::Purpose::Exclude, &[7]);
        data["revision"] = receipt["revision"].clone();
        // Simulate lost acknowledgment: stale UI retries with its original token.
        crate::case_store::save_at(target.path(), data).unwrap();
        assert_eq!(current(target.path(), id), updated.analysis_context);
        assert_eq!(offsets(target.path(), id), vec![7]);
        let normal = crate::case_store::load_at(target.path()).unwrap();
        crate::case_store::save_at(target.path(), normal).unwrap();
        assert_eq!(current(target.path(), id), updated.analysis_context);
    }

    #[test]
    fn portable_missing_or_foreign_tokens_never_publish_case_bodies() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        create(target.path(), "existing");
        create(other.path(), "existing");
        let path = exports.path().join("fixture.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let before = crate::case_store::load_at(target.path()).unwrap();
        let other_before = crate::case_store::load_at(other.path()).unwrap();
        assert!(crate::case_store::save_at(other.path(), merge(other.path(), &imported)).is_err());
        assert_eq!(
            crate::case_store::load_at(other.path()).unwrap(),
            other_before
        );
        let mut wrong = imported.clone();
        wrong["cases"][0]["id"] = json!("existing");
        let mut data = before.clone();
        data["cases"] = wrong["cases"].clone();
        assert!(crate::case_store::save_at(target.path(), data).is_err());
        assert_eq!(crate::case_store::load_at(target.path()).unwrap(), before);
        PENDING
            .lock()
            .remove(imported["cases"][0][TOKEN_FIELD].as_str().unwrap());
        assert!(
            crate::case_store::save_at(target.path(), merge(target.path(), &imported)).is_err()
        );
        assert_eq!(crate::case_store::load_at(target.path()).unwrap(), before);
    }

    #[test]
    fn portable_binary_image_roundtrip_preserves_evidence_bytes() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let image = image.into_inner();
        let id = format!("{:x}", Sha256::digest(&image));
        std::fs::create_dir(source.path().join("case-images")).unwrap();
        let original_path = source.path().join("case-images").join(&id);
        std::fs::write(&original_path, &image).unwrap();
        let mut document = crate::case_store::load_at(source.path()).unwrap();
        document["cases"][0]["items"][0]["attachments"] =
            json!([{ "id":id,"name":"Proof.png","width":0,"mime":"image/unknown" }]);
        crate::case_store::save_at(source.path(), document).unwrap();
        let path = exports.path().join("image.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        assert_eq!(
            imported["cases"][0]["items"][0]["attachments"][0]["width"],
            2
        );
        assert_eq!(
            imported["cases"][0]["items"][0]["attachments"][0]["mime"],
            "image/png"
        );
        assert_eq!(
            std::fs::read(target.path().join("case-images").join(id)).unwrap(),
            image
        );
        crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
        assert_eq!(std::fs::read(original_path).unwrap(), image);
    }

    #[test]
    fn portable_foreign_revision_exhaustion_cannot_disable_local_restore() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        exclude(source.path(), "source", ledger::Purpose::Exclude, &[1]);
        let path = exports.path().join("original.licase");
        exported(source.path(), &path);
        let mut staged: format::StagedArchive<Metadata> =
            format::read(&path, exports.path(), &|| false).unwrap();
        let mut document: Value = serde_json::from_reader(std::io::BufReader::new(
            File::open(staged.path(0).unwrap()).unwrap(),
        ))
        .unwrap();
        document["cases"][0]["analysisContext"]["configRevision"] = json!(u64::MAX);
        document["cases"][0]["analysisContext"]["visibilityRevision"] = json!(u64::MAX);
        staged.metadata.ledgers[0].identity.config_revision = u64::MAX;
        staged.metadata.ledgers[0].identity.visibility_revision = u64::MAX;
        let mut temporary = tempfile::NamedTempFile::new_in(exports.path()).unwrap();
        let mut sources = vec![document_source(temporary.as_file_mut(), &document).unwrap()];
        for (index, entry) in staged.entries.iter().enumerate().skip(1) {
            sources.push(Source {
                entry: entry.clone(),
                file: File::open(staged.path(index).unwrap()).unwrap(),
            });
        }
        let exhausted = exports.path().join("foreign-max.licase");
        format::write(&exhausted, &staged.metadata, &mut sources, &|| false).unwrap();
        let imported = import_at(target.path(), &exhausted).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
        let local = current(target.path(), id);
        assert_eq!(local.config_revision, 0);
        assert_eq!(local.visibility_revision, 1);
        let batches = ledger::list(target.path(), &local.identity(), None, 100).unwrap();
        ledger::restore_batch(
            target.path(),
            &admission(target.path(), id),
            &batches[0].id,
            &work(),
        )
        .unwrap();
        assert!(offsets(target.path(), id).is_empty());
    }

    fn add_reference(root: &Path, id: &str, content: &[u8], available: bool) {
        let mut context = current(root, id);
        let descriptor = crate::analysis_context::ReferenceDescriptor {
            interpretation_version: 1,
            schema_version: 1,
            id: "table".into(),
            name: "Portable table".into(),
            content_sha256: format!("{:x}", Sha256::digest(content)),
            format: "jsonl".into(),
            columns: vec!["code".into(), "label".into()],
            key_columns: vec!["code".into()],
            duplicate_policy: "reject".into(),
        };
        context.config.references = vec![descriptor.clone()];
        crate::analysis_context::validate(&context.config).unwrap();
        crate::case_store::connect(root)
            .unwrap()
            .execute(
                "UPDATE case_analysis SET body=?1 WHERE case_id=?2",
                rusqlite::params![serde_json::to_string(&context).unwrap(), id],
            )
            .unwrap();
        if available {
            crate::reference_store::prepare_jsonl(
                root,
                &crate::reference_store::Owner {
                    case_id: id.into(),
                    analysis_id: context.analysis_id,
                },
                &descriptor,
                content,
                crate::reference_store::Limits::default(),
                &|| false,
            )
            .unwrap();
        }
    }
    fn reference_reader(
        root: &Path,
        id: &str,
    ) -> Result<crate::reference_store::ReferenceReader, crate::reference_store::Error> {
        let context = current(root, id);
        crate::reference_store::open(
            root,
            &crate::reference_store::Owner {
                case_id: id.into(),
                analysis_id: context.analysis_id,
            },
            &context.config.references[0],
            &|| false,
        )
    }
    fn rewrite_archive(
        path: &Path,
        output: &Path,
        mutate: impl FnOnce(&mut Metadata, &mut Vec<Source>),
    ) {
        let mut staged: format::StagedArchive<Metadata> =
            format::read(path, output.parent().unwrap(), &|| false).unwrap();
        let mut sources = staged
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| Source {
                entry: entry.clone(),
                file: File::open(staged.path(index).unwrap()).unwrap(),
            })
            .collect::<Vec<_>>();
        mutate(&mut staged.metadata, &mut sources);
        format::write(output, &staged.metadata, &mut sources, &|| false).unwrap();
    }

    #[test]
    fn portable_reference_bytes_and_exclusions_roundtrip_under_new_owner() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "same-case");
        create(target.path(), "same-case");
        add_reference(
            source.path(),
            "same-case",
            b"{\"code\":1,\"label\":\"foreign\"}\n",
            true,
        );
        add_reference(
            target.path(),
            "same-case",
            b"{\"code\":1,\"label\":\"local\"}\n",
            true,
        );
        let batch = exclude(
            source.path(),
            "same-case",
            ledger::Purpose::Exclude,
            &[1, 2],
        );
        exclude(
            source.path(),
            "same-case",
            ledger::Purpose::RestoreSelection {
                batch_id: batch.batch_id,
            },
            &[2],
        );
        let path = exports.path().join("references.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        assert!(imported["cases"][0].get("analysisContext").is_none());
        assert_eq!(
            crate::case_store::load_at(target.path()).unwrap()["cases"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let before = crate::case_store::load_at(target.path()).unwrap();
        let conn = crate::case_store::connect(target.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_reference BEFORE INSERT ON case_analysis BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;").unwrap();
        let mut merged = merge(target.path(), &imported);
        assert!(crate::case_store::save_at(target.path(), merged.clone()).is_err());
        assert_eq!(crate::case_store::load_at(target.path()).unwrap(), before);
        conn.execute_batch("DROP TRIGGER reject_reference").unwrap();
        let receipt = crate::case_store::save_at(target.path(), merged.clone()).unwrap();
        let local = current(target.path(), id);
        assert_ne!(
            local.analysis_id,
            current(source.path(), "same-case").analysis_id
        );
        assert_eq!(local.config, current(source.path(), "same-case").config);
        assert_eq!(
            reference_reader(target.path(), id)
                .unwrap()
                .lookup(&[json!(1)], "label")
                .unwrap(),
            Some(json!("foreign"))
        );
        assert_eq!(
            reference_reader(target.path(), "same-case")
                .unwrap()
                .lookup(&[json!(1)], "label")
                .unwrap(),
            Some(json!("local"))
        );
        assert_eq!(offsets(target.path(), id), vec![1]);
        let changed = exclude(target.path(), id, ledger::Purpose::Exclude, &[3]);
        merged["revision"] = receipt["revision"].clone();
        crate::case_store::save_at(target.path(), merged).unwrap();
        assert_eq!(current(target.path(), id), changed.analysis_context);
        assert_eq!(
            reference_reader(target.path(), id)
                .unwrap()
                .lookup(&[json!(1)], "label")
                .unwrap(),
            Some(json!("foreign"))
        );
    }

    #[test]
    fn portable_unavailable_and_legacy_references_remain_explicit() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(source.path(), "source", b"", false);
        let path = exports.path().join("missing.licase");
        exported(source.path(), &path);
        let legacy = exports.path().join("legacy.licase");
        rewrite_archive(&path, &legacy, |metadata, _| {
            metadata.schema_version = 1;
            metadata.reference_sets.clear();
        });
        for archive in [path, legacy] {
            let imported = import_at(target.path(), &archive).unwrap();
            let id = imported["cases"][0]["id"].as_str().unwrap();
            crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
            let context = current(target.path(), id);
            assert_eq!(context.config.references.len(), 1);
            assert!(context
                .migration_diagnostics
                .iter()
                .any(|d| d.code == "portable_references_unavailable"));
            assert!(matches!(
                reference_reader(target.path(), id),
                Err(crate::reference_store::Error::Unavailable)
            ));
        }
    }

    #[test]
    fn portable_empty_reference_is_available_after_verified_preparation() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(source.path(), "source", b"", true);
        let path = exports.path().join("empty.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
        assert_eq!(
            reference_reader(target.path(), id)
                .unwrap()
                .prepared()
                .row_count,
            0
        );
        assert!(!current(target.path(), id)
            .migration_diagnostics
            .iter()
            .any(|d| d.code == "portable_references_unavailable"));
    }

    #[test]
    fn portable_reference_masking_preserves_destination() {
        let source = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(
            source.path(),
            "source",
            b"{\"code\":1,\"label\":\"password=private\"}\n",
            true,
        );
        let path = exports.path().join("masked.licase");
        std::fs::write(&path, b"previous").unwrap();
        let error = export_at(
            source.path(),
            &path,
            crate::case_store::load_at(source.path()).unwrap(),
            true,
        )
        .unwrap_err();
        assert!(error.contains("desmarque"));
        assert_eq!(std::fs::read(path).unwrap(), b"previous");
    }

    #[test]
    fn portable_reference_ownership_schema_and_completeness_are_checked() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(
            source.path(),
            "source",
            b"{\"code\":1,\"label\":\"value\"}\n",
            true,
        );
        let path = exports.path().join("valid.licase");
        exported(source.path(), &path);
        for attack in 0..5 {
            let bad = exports.path().join(format!("bad-{attack}.licase"));
            rewrite_archive(&path, &bad, |metadata, sources| match attack {
                0 => {
                    metadata.reference_sets[0].owner.analysis_id = uuid::Uuid::new_v4().to_string()
                }
                1 => {
                    metadata.reference_sets[0].references[0]
                        .descriptor
                        .key_columns = vec!["label".into()]
                }
                2 => {
                    if let references::ReferenceState::Available { schema_sha256, .. } =
                        &mut metadata.reference_sets[0].references[0].state
                    {
                        *schema_sha256 = "0".repeat(64)
                    }
                }
                3 => sources.retain(|source| !matches!(source.entry.kind, EntryKind::Reference(_))),
                _ => {
                    metadata.reference_sets[0].references[0].state =
                        references::ReferenceState::Unavailable
                }
            });
            assert!(import_at(target.path(), &bad).is_err(), "attack {attack}");
            assert!(crate::case_store::load_at(target.path()).unwrap()["cases"]
                .as_array()
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn portable_prepared_reference_changes_roll_back_case_publication() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(
            source.path(),
            "source",
            b"{\"code\":1,\"label\":\"value\"}\n",
            true,
        );
        let path = exports.path().join("valid.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let owner = std::fs::read_dir(target.path().join("references-v1"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let version = std::fs::read_dir(owner)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::OpenOptions::new()
            .append(true)
            .open(version.join("source.jsonl"))
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        assert!(
            crate::case_store::save_at(target.path(), merge(target.path(), &imported)).is_err()
        );
        assert!(crate::case_store::load_at(target.path()).unwrap()["cases"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(PENDING
            .lock()
            .contains_key(imported["cases"][0][TOKEN_FIELD].as_str().unwrap()));
    }

    fn lookup_definition(name: &str, reference: &str) -> Value {
        json!({"name":name,"lookup":{"schemaVersion":1,"referenceId":reference,"keys":[{"referenceColumn":"code","sourceField":"raw_key"}],"valueColumn":"label"}})
    }
    fn set_context(root: &Path, context: &Snapshot) {
        crate::analysis_context::validate(&context.config).unwrap();
        crate::case_store::connect(root)
            .unwrap()
            .execute(
                "UPDATE case_analysis SET body=?1 WHERE case_id=?2",
                rusqlite::params![serde_json::to_string(context).unwrap(), context.case_id],
            )
            .unwrap();
    }

    #[test]
    fn portable_missing_lookup_exposes_reason_while_healthy_and_raw_fields_work() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        add_reference(
            source.path(),
            "source",
            b"{\"code\":1,\"label\":\"ready\"}\n",
            true,
        );
        let mut context = current(source.path(), "source");
        let mut missing = context.config.references[0].clone();
        missing.id = "missing-table".into();
        context.config.references.push(missing);
        context.config.derived_fields.extend([
            lookup_definition("healthy_lookup", "table"),
            lookup_definition("missing_lookup", "missing-table"),
            json!({"name":"dependent","source":"missing_lookup","pattern":"(.*)"}),
        ]);
        set_context(source.path(), &context);
        let path = exports.path().join("mixed.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        let receipt =
            crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
        let local = current(target.path(), id);
        assert_eq!(local.config, context.config);
        assert!(receipt["analysisContexts"][0]["migrationDiagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |diagnostic| diagnostic["code"] == "portable_reference_unavailable"
                    && diagnostic["definitionIndex"] == 2
            ));
        let fields =
            crate::analysis_runtime::prepare_portable_snapshot(target.path(), &local, &|| false)
                .unwrap();
        assert!(fields.iter().any(|field| field.name == "healthy_lookup"));
        assert!(fields
            .iter()
            .all(|field| !["missing_lookup", "dependent"].contains(&field.name.as_str())));
        let mut event = crate::model::Event::empty();
        event.message = "portable".into();
        event.fields.insert("raw_key".into(), json!(1));
        crate::sources::apply_derived(&mut event, &fields);
        assert_eq!(event.message, "portable");
        assert_eq!(event.fields["raw_key"], json!(1));
        assert_eq!(event.fields["healthy_lookup"], json!("ready"));
        assert_eq!(event.fields["portable"], json!("portable"));
    }

    #[test]
    fn portable_oversized_projection_retains_bytes_with_indexed_runtime_reason() {
        use std::io::{Read, Seek, SeekFrom};
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        create(source.path(), "source");
        // Eighteen rows cross the real projection budget; no large log fixture
        // or whole-reference heap buffer is needed.
        let mut input = tempfile::NamedTempFile::new_in(exports.path()).unwrap();
        let value = "x".repeat(500_000);
        for index in 0..18 {
            writeln!(input, "{{\"code\":{index},\"label\":\"{value}\"}}").unwrap();
        }
        input.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 << 10];
        loop {
            let count = input.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        input.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let mut context = current(source.path(), "source");
        let descriptor = crate::analysis_context::ReferenceDescriptor {
            interpretation_version: 1,
            schema_version: 1,
            id: "table".into(),
            name: "Large values".into(),
            content_sha256: format!("{:x}", digest.finalize()),
            format: "jsonl".into(),
            columns: vec!["code".into(), "label".into()],
            key_columns: vec!["code".into()],
            duplicate_policy: "reject".into(),
        };
        context.config.references.push(descriptor.clone());
        context.config.derived_fields.extend([
            lookup_definition("large_lookup", "table"),
            json!({"name":"dependent","source":"large_lookup","pattern":"(.*)"}),
        ]);
        set_context(source.path(), &context);
        crate::reference_store::prepare_jsonl(
            source.path(),
            &crate::reference_store::Owner {
                case_id: context.case_id.clone(),
                analysis_id: context.analysis_id.clone(),
            },
            &descriptor,
            input.as_file_mut(),
            Default::default(),
            &|| false,
        )
        .unwrap();
        let path = exports.path().join("over-budget.licase");
        exported(source.path(), &path);
        let imported = import_at(target.path(), &path).unwrap();
        let id = imported["cases"][0]["id"].as_str().unwrap();
        let receipt =
            crate::case_store::save_at(target.path(), merge(target.path(), &imported)).unwrap();
        let local = current(target.path(), id);
        assert_eq!(local.config, context.config);
        let diagnostic = receipt["analysisContexts"][0]["migrationDiagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "portable_reference_runtime_budget")
            .unwrap();
        assert_eq!(diagnostic["definitionIndex"], 1);
        assert!(diagnostic["message"].as_str().unwrap().contains("8 MiB"));
        let reader = reference_reader(target.path(), id).unwrap();
        assert_eq!(reader.prepared().row_count, 18);
        assert_eq!(
            reader.prepared().version.content_sha256,
            descriptor.content_sha256
        );
        let fields =
            crate::analysis_runtime::prepare_portable_snapshot(target.path(), &local, &|| false)
                .unwrap();
        assert!(fields
            .iter()
            .all(|field| !["large_lookup", "dependent"].contains(&field.name.as_str())));
        let mut event = crate::model::Event::empty();
        event.message = "portable".into();
        crate::sources::apply_derived(&mut event, &fields);
        assert_eq!(event.message, "portable");
        assert_eq!(event.fields["portable"], json!("portable"));
    }
}
