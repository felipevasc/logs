//! Native portable boundary. Record JSON is syntax-checked borrowed text until
//! immutable evidence staging; management views and Event values are never a
//! source for a portable record. This module has no command/UI activation.
use crate::case_evidence::{self as evidence, RawJson, StoreStamp};
use crate::{
    analysis_context::Snapshot,
    case_archive::{payload_metadata, work, Metadata, MASKED_HISTORY, MASKED_REFERENCES},
    case_archive_format::{self as format, Entry, EntryKind, Source},
    case_archive_references as references,
    exclusion_store::{self as ledger, Budget, PortableLedger, PreparedPortable},
};
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const DOCUMENT_BYTES: usize = 64 << 20;
const MAX_CASES: usize = 1000;
const INVALID: &str =
    "CASE_PORTABLE_DOCUMENT: O arquivo de investigação é inválido; nenhum Caso foi publicado.";
const LIMIT: &str =
    "CASE_PORTABLE_LIMIT: O arquivo excede os limites de preservação; os originais foram mantidos.";

fn sensitive_key(key: &str) -> bool {
    [
        "password",
        "passwd",
        "token",
        "secret",
        "authorization",
        "apikey",
        "accesstoken",
        "refreshtoken",
    ]
    .iter()
    .any(|expected| {
        key.bytes()
            .filter(|byte| ![b'_', b'-'].contains(byte))
            .map(|byte| byte.to_ascii_lowercase())
            .eq(expected.bytes())
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExportRequest {
    pub store: StoreStamp,
    pub document_json: String,
    pub path: String,
    pub mask: bool,
    pub request_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportRequest {
    pub store: StoreStamp,
    pub path: String,
    pub request_id: String,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Json,
    Licase,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportReceipt {
    pub kind: &'static str,
    pub path: String,
    pub format: Format,
    pub cases: usize,
    /// Null means an opaque preserved Case has no safely interpretable count.
    pub records: Option<u64>,
    pub masked: bool,
}
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

/// The native path retains the admitted manifest tree and exact document file;
/// it never parses record arrays as serde_json::Value.
pub(crate) struct NativeArchive {
    pub archive: format::StagedArchive<Metadata>,
    credits: Vec<crate::case_work_budget::Lease>,
}
impl NativeArchive {
    pub(crate) fn credit_bytes(&self) -> usize {
        self.credits.iter().map(|credit| credit.bytes()).sum()
    }
    pub(crate) fn images(&self) -> Result<BTreeMap<String, PathBuf>, String> {
        self.archive
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if let EntryKind::Image(id) = &entry.kind {
                    Some(self.archive.path(index).map(|path| (id.clone(), path)))
                } else {
                    None
                }
            })
            .collect()
    }
}
// Kept separate from the legacy archive Metadata deserializer: only native
// manifests use literal dynamic objects inside exclusion history.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeMetadata {
    schema_version: u32,
    #[serde(deserialize_with = "ledger::native_deserialize_ledgers")]
    ledgers: Vec<PortableLedger>,
    #[serde(default)]
    reference_sets: Vec<references::ReferenceSet>,
    #[serde(default)]
    history_sets: Vec<crate::security_history_portable::Profile>,
}
pub(crate) fn read_native(root: &Path, path: &Path) -> Result<NativeArchive, String> {
    let credits = std::cell::RefCell::new(Vec::new());
    let archive: format::StagedArchive<Metadata> = format::read_native_decoded(
        path,
        root,
        &crate::operations::cancelled,
        &mut |bytes| {
            credits.borrow_mut().push(crate::case_cache::reserve_work(
                crate::case_work_budget::global(),
                bytes,
            )?);
            Ok(())
        },
        |encoded| {
            let text = std::str::from_utf8(encoded).map_err(|_| "Manifesto portátil inválido.")?;
            let raw = crate::case_evidence::checked_document(text)?;
            let plan = crate::case_evidence::preflight_value(raw)?;
            credits.borrow_mut().push(crate::case_cache::reserve_work(
                crate::case_work_budget::global(),
                plan.materialization_credit,
            )?);
            let value = crate::case_evidence::materialize_value(raw, plan)?;
            let envelope: format::Envelope<NativeMetadata> = serde_json::from_value(value)
                .map_err(|e| format!("Manifesto portátil inválido: {e}"))?;
            Ok(format::Envelope {
                format_version: envelope.format_version,
                entries: envelope.entries,
                metadata: Metadata {
                    schema_version: envelope.metadata.schema_version,
                    ledgers: envelope.metadata.ledgers,
                    reference_sets: envelope.metadata.reference_sets,
                    history_sets:envelope.metadata.history_sets,
                },
            })
        },
    )?;
    if ![1, 2, 3].contains(&archive.metadata.schema_version)
        || (archive.format_version == 3) != (archive.metadata.schema_version == 3)
    {
        return Err("O manifesto portátil exige uma versão mais recente.".into());
    }
    Ok(NativeArchive {
        archive,
        credits: credits.into_inner(),
    })
}
pub(crate) struct NativePreparedSidecars {
    pub prepared: PreparedPortable,
    reference_sources: Vec<crate::reference_store::PortableSource>,
}
impl NativePreparedSidecars {
    pub(crate) fn snapshot(&self) -> &Snapshot {
        self.prepared.snapshot()
    }
    pub(crate) fn insert(&self, tx: &Transaction<'_>) -> Result<(), String> {
        for source in &self.reference_sources {
            source.validate_unchanged().map_err(|e| e.to_string())?;
        }
        let current = ledger::insert_prepared_after_context(tx, &self.prepared)?;
        if current != *self.snapshot() {
            return Err(
                "CASE_PORTABLE_CONTEXT: A configuração preparada mudou antes da publicação.".into(),
            );
        }
        Ok(())
    }
}
pub(crate) fn prepare_native_sidecars(
    root: &Path,
    snapshots: &mut [Snapshot],
    archive: Option<&NativeArchive>,
) -> Result<Vec<NativePreparedSidecars>, String> {
    let plain;
    let (schema_version, ledgers, reference_sets, entries) = if let Some(archive) = archive {
        (
            &archive.archive.metadata.schema_version,
            &archive.archive.metadata.ledgers,
            archive.archive.metadata.reference_sets.as_slice(),
            archive.archive.entries.as_slice(),
        )
    } else {
        if snapshots
            .iter()
            .any(|snapshot| snapshot.visibility_revision != 0)
        {
            return Err("O JSON não contém o histórico de exclusões deste Caso. Importe um arquivo .licase completo; nenhum Caso foi publicado.".into());
        }
        plain = snapshots
            .iter()
            .map(|snapshot| PortableLedger {
                schema_version: 1,
                identity: snapshot.identity(),
                batches: Vec::new(),
            })
            .collect::<Vec<_>>();
        (&1, &plain, &[][..], &[][..])
    };
    if snapshots.len() != ledgers.len() {
        return Err("Documento e manifesto de visibilidade não correspondem.".into());
    }
    let mut owners = BTreeMap::new();
    let mut analyses = HashSet::new();
    let mut expected = BTreeMap::new();
    for ledger in ledgers {
        if owners
            .insert(ledger.identity.case_id.as_str(), ledger)
            .is_some()
            || !analyses.insert(&ledger.identity.analysis_id)
        {
            return Err("Proprietário portátil duplicado.".into());
        }
        for payload in payload_metadata(ledger) {
            if expected.insert(payload.id().to_owned(), payload).is_some() {
                return Err("Payload portátil compartilhado entre proprietários.".into());
            }
        }
    }
    let mut payloads = BTreeMap::new();
    let mut reference_paths = BTreeMap::new();
    if let Some(archive) = archive {
        for (index, entry) in entries.iter().enumerate() {
            match &entry.kind {
                EntryKind::Exclusion(id) => {
                    let payload = expected
                        .get(id)
                        .ok_or("Payload portátil sem proprietário.")?;
                    if payload.bytes() != entry.bytes || payload.sha256() != entry.sha256 {
                        return Err("Metadados do payload portátil não conferem.".into());
                    }
                    payloads.insert(id.clone(), archive.archive.path(index)?);
                }
                EntryKind::Reference(id) => {
                    reference_paths.insert(id.clone(), archive.archive.path(index)?);
                }
                _ => (),
            }
        }
    }
    if payloads.len() != expected.len() {
        return Err("O arquivo portátil omitiu payloads de exclusão.".into());
    }
    for snapshot in snapshots.iter() {
        if owners
            .get(snapshot.case_id.as_str())
            .map(|ledger| &ledger.identity)
            != Some(&snapshot.identity())
        {
            return Err("Configuração e visibilidade portáteis não correspondem.".into());
        }
    }
    let references = references::validate(*schema_version, reference_sets, snapshots, entries)?;
    if let Some(archive)=archive {crate::security_history_portable::validate_owners(&archive.archive.metadata.history_sets,snapshots,entries)?;}
    let mut prepared = Vec::new();
    for snapshot in snapshots {
        crate::operations::check()?;
        let ledger = owners[snapshot.case_id.as_str()];
        let files = payload_metadata(ledger)
            .map(|payload| {
                Ok((
                    payload.id().to_owned(),
                    payloads
                        .get(payload.id())
                        .ok_or("Payload portátil ausente.")?
                        .clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let mut local = ledger::prepare_portable(
            root,
            snapshot,
            ledger,
            &format!("c-import-{}", uuid::Uuid::new_v4()),
            &files,
            &Budget::default(),
            &work(),
        )?;
        if let Some(archive)=archive {crate::security_history_portable::prepare(root,snapshot,local.snapshot(),&archive.archive.metadata.history_sets,entries,|index|archive.archive.path(index))?;}
        let descriptors = &references[&snapshot.case_id];
        let reference_sources =
            references::prepare(root, local.snapshot(), descriptors, &reference_paths)?;
        references::interpretation_diagnostics(descriptors, &mut local, true)?;
        references::preflight(root, &mut local)?;
        prepared.push(NativePreparedSidecars {
            prepared: local,
            reference_sources,
        });
    }
    Ok(prepared)
}
pub(crate) struct NativeCapturedSidecars {
    histories:Vec<crate::security_history_portable::Captured>,
    ledgers: Vec<ledger::CapturedPortable>,
    references: Vec<Snapshot>,
    credits: Vec<crate::case_work_budget::Lease>,
}
pub(crate) struct NativeExportAssets {
    pub metadata: Metadata,
    pub sources: Vec<Source>,
    _references: Vec<references::Captured>,
    _credits: Vec<crate::case_work_budget::Lease>,
}
pub(crate) fn capture_native_sidecars(
    conn: &Connection,
    root: &Path,
    snapshots: &[Snapshot],
    mask: bool,
    plain_json: bool,
) -> Result<NativeCapturedSidecars, String> {
    // SQL guards cap one context at4MiB and one batch at192KiB. Charge input
    // scratch before the reader obtains Strings; decoded trees are separate.
    let _scratch = crate::case_cache::reserve_work(crate::case_work_budget::global(), 8 << 20)?;
    let mut captured = NativeCapturedSidecars {
        histories:Vec::new(),
        ledgers: Vec::new(),
        references: Vec::new(),
        credits: Vec::new(),
    };
    let mut metadata_budget = ByteCounter {
        bytes: 0,
        limit: format::MAX_MANIFEST_BYTES as usize,
    };
    for snapshot in snapshots {
        let selected_history=snapshot.interpretation.as_ref().map(|i|i.security.detection_settings()).transpose()?.is_some_and(|s|s.investigation.profile_revision.is_some());
        if selected_history && (plain_json||mask){return Err("O perfil histórico congelado exige exportação completa em .licase com os bytes originais. O destino foi preservado.".into());}
        if let Some(profile)=crate::security_history_portable::capture(root,snapshot)?{captured.histories.push(profile);}
        if plain_json && !snapshot.config.references.is_empty() {
            return Err("O JSON não transporta os bytes das referências. Exporte o Caso completo em .licase; o destino foi preservado.".into());
        }
        if plain_json && snapshot.visibility_revision != 0 {
            return Err("O JSON não transporta o histórico de exclusões. Exporte o Caso completo em .licase; o destino foi preservado.".into());
        }
        if mask && snapshot.visibility_revision != 0 {
            return Err(MASKED_HISTORY.into());
        }
        if mask && !snapshot.config.references.is_empty() {
            return Err(MASKED_REFERENCES.into());
        }
        let ledger = ledger::capture_portable_admitted(
            conn,
            root,
            snapshot,
            &Budget::default(),
            &mut |text| {
                let raw = crate::case_evidence::checked_document(text)?;
                let plan = crate::case_evidence::preflight_value(raw)?;
                captured.credits.push(crate::case_cache::reserve_work(
                    crate::case_work_budget::global(),
                    plan.materialization_credit,
                )?);
                Ok(())
            },
        )?;
        if plain_json && !ledger.manifest.batches.is_empty() {
            return Err("O JSON não pode omitir o histórico de exclusões.".into());
        }
        serde_json::to_writer(&mut metadata_budget, &ledger.manifest).map_err(|e| e.to_string())?;
        // Only reference declarations are needed after releasing SQLite. The
        // full authoritative context is already retained by ExportCapture.
        let declarations = (
            &snapshot.case_id,
            &snapshot.analysis_id,
            &snapshot.config.references,
        );
        let mut size = ByteCounter {
            bytes: 0,
            limit: format::MAX_MANIFEST_BYTES as usize,
        };
        serde_json::to_writer(&mut size, &declarations).map_err(|e| e.to_string())?;
        let scratch = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            size.bytes.checked_mul(2).ok_or("Metadados excessivos.")?,
        )?;
        let encoded = serde_json::to_string(&declarations).map_err(|e| e.to_string())?;
        let plan = crate::case_evidence::preflight_value(crate::case_evidence::checked_document(
            &encoded,
        )?)?;
        captured.credits.push(crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            plan.materialization_credit,
        )?);
        captured.references.push(Snapshot {
            schema_version: 1,
            case_id: snapshot.case_id.clone(),
            analysis_id: snapshot.analysis_id.clone(),
            config_revision: snapshot.config_revision,
            visibility_revision: snapshot.visibility_revision,
            config: crate::analysis_context::Config {
                derived_fields: Vec::new(),
                references: snapshot.config.references.clone(),
            },
            interpretation: None,
            migration_diagnostics: Vec::new(),
            legacy_raw: None,
        });
        drop(encoded);
        drop(scratch);
        captured.ledgers.push(ledger);
    }
    Ok(captured)
}
pub(crate) fn native_export_assets(
    root: &Path,
    captured: &NativeCapturedSidecars,
) -> Result<NativeExportAssets, String> {
    let clone_bytes = captured
        .credits
        .iter()
        .try_fold(0usize, |total, credit| total.checked_add(credit.bytes()))
        .ok_or("Metadados portáteis excessivos.")?;
    let mut credits = vec![crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        clone_bytes,
    )?];
    let _inspection = crate::case_cache::reserve_work(crate::case_work_budget::global(), 16 << 20)?;
    let mut references = Vec::new();
    {
        let mut count = 0usize;
        for snapshot in &captured.references {
            count = count
                .checked_add(snapshot.config.references.len())
                .ok_or("Referências excessivas.")?;
            if count > format::MAX_REFERENCES {
                return Err("O arquivo portátil excede4.096 referências.".into());
            }
            references.push(references::capture(root, snapshot)?);
        }
    }
    let count = captured
        .ledgers
        .iter()
        .map(|ledger| ledger.files.len())
        .sum::<usize>()
        .checked_add(references.iter().map(|set| set.files.len()).sum::<usize>())
        .ok_or("Conteúdo portátil excessivo.")?;
    if count > format::MAX_PAYLOADS + format::MAX_REFERENCES {
        return Err("Conteúdo portátil excessivo.".into());
    }
    credits.push(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        count
            .checked_mul(1024)
            .ok_or("Conteúdo portátil excessivo.")?,
    )?);
    let mut sources = Vec::new();
    for captured in &captured.ledgers {
        for file in &captured.files {
            sources.push(Source {
                entry: Entry {
                    kind: EntryKind::Exclusion(file.meta().id().into()),
                    bytes: file.meta().bytes(),
                    sha256: file.meta().sha256().into(),
                },
                file: file.open_reader()?,
            });
        }
    }
    for captured in &references {
        for (id, file) in &captured.files {
            sources.push(Source {
                entry: Entry {
                    kind: EntryKind::Reference(id.clone()),
                    bytes: file.prepared().source_bytes,
                    sha256: file.prepared().version.content_sha256.clone(),
                },
                file: file.source_reader().map_err(|e| e.to_string())?,
            });
        }
    }
    let mut history_sets=Vec::new();
    for history in &captured.histories {history_sets.push(history.profile.clone());let mut file=history.source.file.try_clone().map_err(|e|e.to_string())?;use std::io::{Seek,SeekFrom};file.seek(SeekFrom::Start(0)).map_err(|e|e.to_string())?;sources.push(Source{entry:history.source.entry.clone(),file});}
    let metadata = Metadata {
        history_sets,
        schema_version: 3,
        ledgers: captured
            .ledgers
            .iter()
            .map(|captured| captured.manifest.clone())
            .collect(),
        reference_sets: references
            .iter()
            .map(|captured| captured.manifest.clone())
            .collect(),
    };
    Ok(NativeExportAssets {
        metadata,
        sources,
        _references: references,
        _credits: credits,
    })
}

struct DocumentWriter<W> {
    inner: W,
    bytes: u64,
    sha256: Sha256,
}
impl<W: Write> Write for DocumentWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if crate::operations::cancelled() {
            return Err(std::io::Error::other("Operação cancelada."));
        }
        if bytes.len() as u64 > (DOCUMENT_BYTES as u64).saturating_sub(self.bytes) {
            return Err(std::io::Error::other(LIMIT));
        }
        let count = self.inner.write(bytes)?;
        self.sha256.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
impl<W: Write> DocumentWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            bytes: 0,
            sha256: Sha256::new(),
        }
    }
    fn finish(mut self) -> Result<(u64, String), String> {
        self.flush().map_err(|e| e.to_string())?;
        Ok((self.bytes, format!("{:x}", self.sha256.finalize())))
    }
}
fn write_image_assets(
    output: &mut dyn Write,
    references: &std::collections::BTreeMap<String, String>,
    sources: &mut [crate::case_archive_format::Source],
) -> Result<(), String> {
    output
        .write_all(b",\"imageAssets\":[")
        .map_err(|e| e.to_string())?;
    let _credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 128 << 10)?;
    let mut buffer = [0; 64 << 10];
    for (index, source) in sources.iter_mut().enumerate() {
        let crate::case_archive_format::EntryKind::Image(id) = &source.entry.kind else {
            return Err(INVALID.into());
        };
        if index != 0 {
            output.write_all(b",").map_err(|e| e.to_string())?;
        }
        output.write_all(b"{\"id\":").map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut *output, id).map_err(|e| e.to_string())?;
        output.write_all(b",\"name\":").map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut *output, references.get(id).ok_or(INVALID)?)
            .map_err(|e| e.to_string())?;
        output
            .write_all(b",\"base64\":\"")
            .map_err(|e| e.to_string())?;
        source
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let mut encoded = base64::write::EncoderWriter::new(
            &mut *output,
            &base64::engine::general_purpose::STANDARD,
        );
        let mut remaining = source.entry.bytes;
        let mut digest = Sha256::new();
        while remaining != 0 {
            crate::operations::check()?;
            let length = remaining.min(buffer.len() as u64) as usize;
            let count = source
                .file
                .read(&mut buffer[..length])
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("A imagem mudou durante a exportação.".into());
            }
            digest.update(&buffer[..count]);
            encoded
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
            remaining -= count as u64;
        }
        encoded.finish().map_err(|e| e.to_string())?;
        drop(encoded);
        if source
            .file
            .read(&mut buffer[..1])
            .map_err(|e| e.to_string())?
            != 0
            || format!("{:x}", digest.finalize()) != source.entry.sha256
        {
            return Err("A integridade da imagem mudou durante a exportação.".into());
        }
        output.write_all(b"\"}").map_err(|e| e.to_string())?;
    }
    output.write_all(b"]").map_err(|e| e.to_string())
}
fn write_export_document<T>(
    output: &mut dyn Write,
    captured: &evidence::ExportCapture<T>,
    active: Option<&str>,
    mask: bool,
    transfer: &TransferMap,
    images: Option<(&BTreeMap<String, String>, &mut [Source])>,
) -> Result<(), String> {
    output
        .write_all(
            b"{\"format\":\"loginsight.native-case\",\"schemaVersion\":3,\"document\":{\"active\":",
        )
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut *output, &active).map_err(|e| e.to_string())?;
    output
        .write_all(b",\"cases\":[")
        .map_err(|e| e.to_string())?;
    for (ordinal, (case, snapshot)) in captured
        .cases()
        .iter()
        .zip(captured.snapshots())
        .enumerate()
    {
        if ordinal != 0 {
            output.write_all(b",").map_err(|e| e.to_string())?;
        }
        if mask {
            case.write_to_with_parts(
                output,
                snapshot,
                masked_envelope,
                masked_metadata,
                masked_original_field,
            )?;
        } else {
            case.write_to_with_parts(
                output,
                snapshot,
                |text, writer| writer.write_all(text.as_bytes()).map_err(|e| e.to_string()),
                |_, value, writer| serde_json::to_writer(writer, value).map_err(|e| e.to_string()),
                |_, text, writer| writer.write_all(text.as_bytes()).map_err(|e| e.to_string()),
            )?;
        }
    }
    output.write_all(b"]").map_err(|e| e.to_string())?;
    if let Some((references, sources)) = images {
        write_image_assets(output, references, sources)?;
    }
    output
        .write_all(b"},\"nativeEvidenceMap\":")
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut *output, transfer).map_err(|e| e.to_string())?;
    output.write_all(b"}").map_err(|e| e.to_string())
}

pub(crate) fn export_at(root: &Path, request: ExportRequest) -> Result<ExportReceipt, String> {
    if request.document_json.len() > evidence::VIEW_DOCUMENT_BYTES
        || request.path.is_empty()
        || request.path.len() > 32 << 10
    {
        return Err(LIMIT.into());
    }
    let path = PathBuf::from(&request.path);
    crate::case_archive::safe_export_destination(root, &path)?;
    let format = if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("licase"))
    {
        Format::Licase
    } else {
        Format::Json
    };
    let active = object_field(
        evidence::checked_document(&request.document_json)?,
        "active",
    )?
    .filter(|raw| raw.kind() != b'n')
    .map(|raw| {
        if raw.get().len() > 16 << 10 {
            return Err(LIMIT.to_string());
        }
        serde_json::from_str::<String>(raw.get()).map_err(|e| e.to_string())
    })
    .transpose()?;
    let save = evidence::SaveViewRequest {
        request_id: request.request_id,
        expected_store: request.store,
        document_json: request.document_json,
    };
    let captured = evidence::capture_export(root, &save, |conn, snapshots| {
        capture_native_sidecars(conn, root, snapshots, request.mask, format == Format::Json)
    })?;
    let active = active.filter(|id| {
        captured
            .cases()
            .iter()
            .any(|lease| lease.owner().case_id == *id)
    });
    let mut maps = Vec::new();
    let mut map_credits = Vec::new();
    let mut records = 0u64;
    let mut records_known = true;
    let mut refs = BTreeMap::new();
    let mut map_size = ByteCounter {
        bytes: 0,
        limit: 8 << 20,
    };
    for (ordinal, case) in captured.cases().iter().enumerate() {
        let map = if let Some(native) = case.native() {
            records = native
                .containers()
                .try_fold(records, |n, (_, c)| {
                    n.checked_add(c.reference().member_count as u64)
                })
                .ok_or(LIMIT)?;
            let image_refs =
                crate::case_images::references_from_cases(std::iter::once(native.metadata()))?;
            refs.extend(image_refs);
            let (map, credit) =
                evidence::capture_transfer_map(ordinal as u32, native, case.anchors())?;
            map_credits.push(credit);
            map
        } else {
            records_known = false;
            let original = evidence::checked_document(case.original().ok_or(INVALID)?)?;
            let assets = asset_document(&[original])?;
            refs.extend(crate::case_images::references(&assets.value)?);
            // Opaque records remain exact. A previously frozen unresolved map
            // is transportable even when that document cannot be interpreted.
            evidence::PortableCaseMap {
                case_ordinal: ordinal as u32,
                anchors: case.anchors().clone(),
                containers: Vec::new(),
                members: Vec::new(),
            }
        };
        serde_json::to_writer(&mut map_size, &map).map_err(|e| e.to_string())?;
        maps.push(map);
    }
    let transfer = TransferMap {
        schema_version: 1,
        cases: maps,
    };
    let (mut images, _image_credit) =
        crate::case_images::native_image_sources_from_refs(root, &refs)?;
    if format == Format::Licase {
        let mut document = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
        let mut writer = DocumentWriter::new(std::io::BufWriter::new(document.as_file_mut()));
        write_export_document(
            &mut writer,
            &captured,
            active.as_deref(),
            request.mask,
            &transfer,
            None,
        )?;
        let (bytes, sha256) = writer.finish()?;
        document.as_file().sync_all().map_err(|e| e.to_string())?;
        let mut assets = native_export_assets(root, captured.sidecars())?;
        let mut sources = vec![crate::case_archive_format::Source {
            entry: crate::case_archive_format::Entry {
                kind: crate::case_archive_format::EntryKind::Document,
                bytes,
                sha256,
            },
            file: document.as_file().try_clone().map_err(|e| e.to_string())?,
        }];
        sources.append(&mut images);
        sources.append(&mut assets.sources);
        let mut credits = Vec::new();
        crate::case_archive_format::write_native_guarded(
            &path,
            &assets.metadata,
            &mut sources,
            &crate::operations::cancelled,
            &mut |bytes| {
                credits.push(crate::case_cache::reserve_work(
                    crate::case_work_budget::global(),
                    bytes,
                )?);
                Ok(())
            },
            &|| captured.validate(),
        )?;
    } else {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut document = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let mut writer = DocumentWriter::new(std::io::BufWriter::new(document.as_file_mut()));
        write_export_document(
            &mut writer,
            &captured,
            active.as_deref(),
            request.mask,
            &transfer,
            Some((&refs, &mut images)),
        )?;
        writer.finish()?;
        document.as_file().sync_all().map_err(|e| e.to_string())?;
        captured.validate()?;
        crate::operations::check()?;
        document.persist(&path).map_err(|e| e.error.to_string())?;
    }
    crate::operations::commit();
    Ok(ExportReceipt {
        kind: "native_case_export",
        path: request.path,
        format,
        cases: captured.cases().len(),
        records: records_known.then_some(records),
        masked: request.mask,
    })
}

/// Reserve before the input allocation and retain its credit while any raw
/// record/config/image span borrows it. Only application-owned staging paths
/// or a separately checked external source may reach this reader.
struct Document {
    text: String,
    _credit: crate::case_work_budget::Lease,
}
impl Document {
    fn read(path: &Path) -> Result<Self, String> {
        crate::operations::check()?;
        let mut file = crate::case_archive_format::open_regular(path)?;
        let before = file.metadata().map_err(|e| e.to_string())?;
        let bytes = usize::try_from(before.len()).map_err(|_| LIMIT)?;
        if !before.is_file() || bytes == 0 || bytes > DOCUMENT_BYTES {
            return Err(LIMIT.into());
        }
        let credit = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            bytes.checked_add(64 << 10).ok_or(LIMIT)?,
        )?;
        let mut buffer = vec![0; bytes];
        for chunk in buffer.chunks_mut(64 << 10) {
            crate::operations::check()?;
            file.read_exact(chunk).map_err(|e| e.to_string())?;
        }
        let mut extra = [0; 1];
        let after = file.metadata().map_err(|e| e.to_string())?;
        if file.read(&mut extra).map_err(|e| e.to_string())? != 0
            || after.len() != before.len()
            || after.modified().map_err(|e| e.to_string())?
                != before.modified().map_err(|e| e.to_string())?
        {
            return Err(
                "CASE_PORTABLE_CHANGED: O arquivo mudou durante a leitura; importe-o novamente."
                    .into(),
            );
        }
        let text = String::from_utf8(buffer).map_err(|_| INVALID)?;
        evidence::checked_document(&text)?;
        Ok(Self {
            text,
            _credit: credit,
        })
    }
}

/// Do not open a live SQLite database with an ordinary File, including an
/// external hard link to the same inode: closing it can release POSIX locks.
fn external_input(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if path.starts_with(&root) {
        return Err("CASE_PORTABLE_INTERNAL_PATH: Escolha um arquivo fora do armazenamento interno da aplicação.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let input = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if let Ok(database) = std::fs::metadata(root.join("investigations.sqlite3")) {
            if input.dev() == database.dev() && input.ino() == database.ino() {
                return Err(
                    "CASE_PORTABLE_INTERNAL_PATH: O arquivo pertence ao banco ativo da aplicação."
                        .into(),
                );
            }
        }
    }
    Ok(path)
}

struct RawDocument<'a> {
    cases: Vec<RawJson<'a>>,
    image_assets: Option<RawJson<'a>>,
    active: Option<String>,
    transfer: Option<TransferMap>,
    _credit: crate::case_work_budget::Lease,
}
fn charge(credit: &mut crate::case_work_budget::Lease, bytes: usize) -> Result<(), String> {
    credit.merge(crate::case_cache::reserve_work(credit.pool(), bytes)?)
}
fn object_field<'a>(value: RawJson<'a>, name: &str) -> Result<Option<RawJson<'a>>, String> {
    let mut members = value.members()?;
    let mut result = None;
    while let Some(member) = members.next()? {
        if member.key.len() > 16 << 10 {
            return Err(LIMIT.into());
        }
        if member.key()? == name {
            if result.is_some() {
                return Err(INVALID.into());
            }
            result = Some(member.value);
        }
    }
    Ok(result)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferMap {
    schema_version: u32,
    cases: Vec<evidence::PortableCaseMap>,
}
fn document(text: &str) -> Result<RawDocument<'_>, String> {
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 64 << 10)?;
    let outer = evidence::checked_document(text)?;
    let native = object_field(outer, "format")?.is_some();
    let (raw, transfer) = if native {
        if object_field(outer, "format")?.map(|v| v.get()) != Some(r#""loginsight.native-case""#)
            || object_field(outer, "schemaVersion")?.map(|v| v.get()) != Some("3")
            || object_field(outer, "cases")?.is_some()
        {
            return Err(INVALID.into());
        }
        let raw = object_field(outer, "document")?.ok_or(INVALID)?;
        let map = object_field(outer, "nativeEvidenceMap")?.ok_or(INVALID)?;
        if map.get().len() > 8 << 20 {
            return Err(LIMIT.into());
        }
        let plan = evidence::preflight_value(map)?;
        charge(
            &mut credit,
            plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
        )?;
        let transfer: TransferMap =
            serde_json::from_value(evidence::materialize_value(map, plan)?).map_err(|_| INVALID)?;
        if transfer.schema_version != 1 {
            return Err(INVALID.into());
        }
        (raw, Some(transfer))
    } else {
        if object_field(outer, "nativeEvidenceMap")?.is_some()
            || object_field(outer, "document")?.is_some()
        {
            return Err(INVALID.into());
        }
        (outer, None)
    };
    let mut fields = raw.members()?;
    let mut names = HashSet::new();
    let mut cases = None;
    let mut image_assets = None;
    let mut active = None;
    while let Some(field) = fields.next()? {
        if field.key.len() > 16 << 10 || names.len() >= 1024 {
            return Err(LIMIT.into());
        }
        charge(&mut credit, field.key.len() * 2 + 256)?;
        let name = field.key()?;
        if !names.insert(name.clone()) {
            return Err(INVALID.into());
        }
        match name.as_str() {
            "cases" => {
                let mut values = Vec::new();
                let mut ids = HashSet::new();
                let mut elements = field.value.elements()?;
                while let Some(case) = elements.next()? {
                    if values.len() >= MAX_CASES || case.kind() != b'{' {
                        return Err(LIMIT.into());
                    }
                    let id = object_field(case, "id")?.ok_or(INVALID)?;
                    if id.get().len() > 16 << 10 {
                        return Err(LIMIT.into());
                    }
                    charge(&mut credit, id.get().len() * 2 + 256)?;
                    let id: String = serde_json::from_str(id.get()).map_err(|_| INVALID)?;
                    if id.is_empty() || id.len() > 4096 || !ids.insert(id) {
                        return Err(INVALID.into());
                    }
                    values.push(case);
                }
                if values.is_empty() {
                    return Err(INVALID.into());
                }
                cases = Some(values);
            }
            "imageAssets" => {
                image_assets = Some(field.value);
            }
            "active" | "activeCaseId" => {
                if field.value.kind() != b'n' {
                    if field.value.get().len() > 16 << 10 {
                        return Err(LIMIT.into());
                    }
                    let id: String =
                        serde_json::from_str(field.value.get()).map_err(|_| INVALID)?;
                    if id.len() > 4096 || active.as_ref().is_some_and(|old| old != &id) {
                        return Err(INVALID.into());
                    }
                    active = Some(id);
                }
            }
            "schemaVersion" => {
                if !["1", "2"].contains(&field.value.get()) {
                    return Err("O arquivo exige uma versão de investigação mais recente.".into());
                }
            }
            _ => (),
        }
    }
    let cases = cases.ok_or(INVALID)?;
    if let Some(transfer) = &transfer {
        if transfer.cases.len() != cases.len() {
            return Err(INVALID.into());
        }
        let mut owners = HashSet::new();
        for (ordinal, map) in transfer.cases.iter().enumerate() {
            map.validate()?;
            if map.case_ordinal as usize != ordinal {
                return Err(INVALID.into());
            }
            for container in &map.containers {
                if !owners.insert(&container.container_id) {
                    return Err(INVALID.into());
                }
            }
        }
    }
    Ok(RawDocument {
        cases,
        image_assets,
        active,
        transfer,
        _credit: credit,
    })
}

struct AssetDocument {
    value: serde_json::Value,
    _credits: Vec<crate::case_work_budget::Lease>,
}
fn asset_document(cases: &[RawJson<'_>]) -> Result<AssetDocument, String> {
    let mut metadata_credit =
        crate::case_cache::reserve_work(crate::case_work_budget::global(), 64 << 10)?;
    let mut values: Vec<serde_json::Value> = Vec::new();
    let mut credits = Vec::new();
    let mut count = 0usize;
    for case in cases {
        charge(&mut metadata_credit, 16 << 10)?;
        let mut value = serde_json::Map::new();
        let id = object_field(*case, "id")?.ok_or(INVALID)?;
        value.insert(
            "id".into(),
            serde_json::from_str(id.get()).map_err(|_| INVALID)?,
        );
        for key in ["items", "caseTrails"] {
            let Some(items) = object_field(*case, key)? else {
                continue;
            };
            if items.kind() != b'[' {
                continue;
            }
            let mut source = items.elements()?;
            let mut target = Vec::new();
            while let Some(item) = source.next()? {
                if target.len() >= 10_000 {
                    return Err(LIMIT.into());
                }
                charge(&mut metadata_credit, 512)?;
                let mut retained = serde_json::Map::new();
                if item.kind() == b'{' {
                    if let Some(attachments) = object_field(item, "attachments")? {
                        let mut elements = attachments.elements()?;
                        let mut values = Vec::new();
                        while let Some(attachment) = elements.next()? {
                            count += 1;
                            if count > 1000 {
                                return Err(LIMIT.into());
                            }
                            let plan = evidence::preflight_value(attachment)?;
                            let credit = crate::case_cache::reserve_work(
                                crate::case_work_budget::global(),
                                plan.materialization_credit,
                            )?;
                            values.push(evidence::materialize_value(attachment, plan)?);
                            credits.push(credit);
                        }
                        retained.insert("attachments".into(), values.into());
                    }
                }
                target.push(serde_json::Value::Object(retained));
            }
            value.insert(key.into(), target.into());
        }
        values.push(value.into());
    }
    credits.push(metadata_credit);
    Ok(AssetDocument {
        value: serde_json::json!({"cases":values}),
        _credits: credits,
    })
}
fn context(
    case: RawJson<'_>,
) -> Result<
    (
        crate::analysis_context::Snapshot,
        Option<crate::case_work_budget::Lease>,
    ),
    String,
> {
    let id: String = serde_json::from_str(object_field(case, "id")?.ok_or(INVALID)?.get())
        .map_err(|_| INVALID)?;
    let Some(raw) = object_field(case, "analysisContext")?.filter(|raw| raw.kind() != b'n') else {
        return Ok((
            crate::analysis_context::Snapshot {
                schema_version: 1,
                case_id: id,
                analysis_id: uuid::Uuid::new_v4().to_string(),
                config_revision: 0,
                visibility_revision: 0,
                config: Default::default(),
                interpretation: None,
                migration_diagnostics: Vec::new(),
                legacy_raw: None,
            },
            None,
        ));
    };
    if raw.get().len() > 4 << 20 {
        return Err(LIMIT.into());
    }
    let plan = evidence::preflight_value(raw)?;
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
    )?;
    let value = evidence::materialize_value(raw, plan)?;
    let snapshot: crate::analysis_context::Snapshot =
        evidence::native_snapshot_from_value(value).map_err(|e| e.to_string())?;
    if snapshot.schema_version != 1
        || snapshot.case_id != id
        || uuid::Uuid::parse_str(&snapshot.analysis_id).is_err()
    {
        return Err(INVALID.into());
    }
    crate::analysis_context::validate_snapshot_size(&snapshot)?;
    Ok((snapshot, Some(credit)))
}
fn embedded_images(
    root: &Path,
    raw: Option<RawJson<'_>>,
) -> Result<
    (
        tempfile::TempDir,
        std::collections::BTreeMap<String, PathBuf>,
    ),
    String,
> {
    let staging = tempfile::Builder::new()
        .prefix("case-native-images-")
        .tempdir_in(root)
        .map_err(|e| e.to_string())?;
    let mut images = std::collections::BTreeMap::new();
    if let Some(raw) = raw {
        let mut assets = raw.elements()?;
        while let Some(asset) = assets.next()? {
            if images.len() >= 1000 {
                return Err(LIMIT.into());
            }
            let (id, path) = crate::case_images::stage_native_embedded_image(
                staging.path(),
                images.len(),
                asset,
            )?;
            if images.insert(id, path).is_some() {
                return Err("CASE_PORTABLE_IMAGE: Imagem incorporada duplicada.".into());
            }
        }
    }
    Ok((staging, images))
}

/// Read/hash before creating any local identity. A repeated request returns the
/// original committed IDs and original acknowledged views, even after edits.
pub(crate) fn import_at(
    root: &Path,
    request: ImportRequest,
) -> Result<evidence::CommittedImport, String> {
    if request.path.is_empty() || request.path.len() > 32 << 10 {
        return Err(INVALID.into());
    }
    let root_guard = crate::case_recovery::RootLease::shared(
        root,
        &crate::case_recovery::Work {
            cancelled: &crate::operations::cancelled,
            progress: &|_, _, _| {},
        },
    )?;
    let path = external_input(root, Path::new(&request.path))?;
    let archive = if format::is_archive(&path)? {
        Some(read_native(root, &path)?)
    } else {
        None
    };
    let input = Document::read(&match &archive {
        Some(archive) => archive.archive.path(0)?,
        None => path,
    })?;
    let fingerprint = if let Some(archive) = &archive {
        evidence::ImportFingerprint {
            sha256: archive.archive.source_sha256.clone(),
            bytes: archive.archive.source_bytes,
            format: evidence::ImportFormat::Licase,
        }
    } else {
        evidence::ImportFingerprint {
            sha256: format!("{:x}", Sha256::digest(input.text.as_bytes())),
            bytes: input.text.len() as u64,
            format: evidence::ImportFormat::Json,
        }
    };
    if let Some(receipt) =
        evidence::replay_import(root, &request.store, &request.request_id, &fingerprint)?
    {
        return Ok(receipt);
    }
    let parsed = document(&input.text)?;
    if archive
        .as_ref()
        .is_some_and(|a| (a.archive.format_version == 3) != parsed.transfer.is_some())
    {
        return Err(INVALID.into());
    }
    let loaded = evidence::load_view(root)?;
    if loaded.document().store != request.store {
        return Err("CASE_EVIDENCE_STALE_STORE".into());
    }
    let mut counter = ByteCounter {
        bytes: 0,
        limit: evidence::VIEW_DOCUMENT_BYTES,
    };
    serde_json::to_writer(&mut counter, loaded.document()).map_err(|e| e.to_string())?;
    let _view_credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        counter.bytes.checked_mul(2).ok_or(LIMIT)?,
    )?;
    let save = evidence::SaveViewRequest {
        request_id: request.request_id,
        expected_store: request.store,
        document_json: serde_json::to_string(loaded.document()).map_err(|e| e.to_string())?,
    };
    drop(loaded);
    let mut snapshots = Vec::new();
    let mut context_credits = Vec::new();
    for raw in &parsed.cases {
        let (snapshot, credit) = context(*raw)?;
        snapshots.push(snapshot);
        context_credits.extend(credit);
    }
    let selected = parsed
        .active
        .as_deref()
        .map(|id| {
            snapshots
                .iter()
                .position(|s| s.case_id == id)
                .ok_or(INVALID)
        })
        .transpose()?;
    let sidecars = prepare_native_sidecars(root, &mut snapshots, archive.as_ref())?;
    let mut assets = asset_document(&parsed.cases)?;
    let mut embedded = None;
    let images = if let Some(archive) = &archive {
        if parsed.image_assets.is_some() {
            return Err(INVALID.into());
        }
        archive.images()?
    } else {
        let (staging, images) = embedded_images(root, parsed.image_assets)?;
        embedded = Some(staging);
        images
    };
    crate::case_images::validate_native_images(&mut assets.value, &images)?;
    drop(assets);
    let mut prepared = Vec::new();
    for (ordinal, (raw, sidecar)) in parsed.cases.iter().zip(&sidecars).enumerate() {
        crate::operations::check()?;
        prepared.push(if let Some(transfer) = &parsed.transfer {
            evidence::prepare_import_case_with_transfer(
                root,
                &save.expected_store,
                sidecar.snapshot().clone(),
                raw.get(),
                &transfer.cases[ordinal],
            )?
        } else {
            evidence::prepare_import_case(
                root,
                &save.expected_store,
                sidecar.snapshot().clone(),
                raw.get(),
            )?
        });
    }
    crate::case_images::publish_native_images(root, &images)?;
    root_guard.validate()?;
    let selected = selected.map(|ordinal| prepared[ordinal].owner().case_id.as_str());
    let receipt = evidence::commit_import(root, &save, &prepared, &fingerprint, selected, |tx| {
        for sidecar in &sidecars {
            sidecar.insert(tx)?;
        }
        Ok(())
    })?;
    drop(embedded);
    Ok(receipt)
}

/// Only targeted sensitive values change. All non-target leaves, including
/// integers, floats, exponent spellings and unknown record members, are copied
/// directly from their checked span instead of passing through serde Value.
fn write_masked(raw: RawJson<'_>, output: &mut dyn Write, depth: usize) -> Result<(), String> {
    if depth > 128 {
        return Err(LIMIT.into());
    }
    crate::operations::check()?;
    match raw.kind() {
        b'{' => {
            output.write_all(b"{").map_err(|e| e.to_string())?;
            let mut members = raw.members()?;
            let mut first = true;
            while let Some(member) = members.next()? {
                if !first {
                    output.write_all(b",").map_err(|e| e.to_string())?;
                }
                first = false;
                output
                    .write_all(member.key.as_bytes())
                    .and_then(|_| output.write_all(b":"))
                    .map_err(|e| e.to_string())?;
                if sensitive_key(&member.key()?) {
                    output
                        .write_all(b"\"[oculto]\"")
                        .map_err(|e| e.to_string())?;
                } else {
                    write_masked(member.value, output, depth + 1)?;
                }
            }
            output.write_all(b"}").map_err(|e| e.to_string())?;
        }
        b'[' => {
            output.write_all(b"[").map_err(|e| e.to_string())?;
            let mut rows = raw.elements()?;
            let mut first = true;
            while let Some(row) = rows.next()? {
                if !first {
                    output.write_all(b",").map_err(|e| e.to_string())?;
                }
                first = false;
                write_masked(row, output, depth + 1)?;
            }
            output.write_all(b"]").map_err(|e| e.to_string())?;
        }
        b'"' => {
            let value: String = serde_json::from_str(raw.get()).map_err(|e| e.to_string())?;
            let masked = crate::workspace::redact(&value);
            if masked == value {
                output
                    .write_all(raw.get().as_bytes())
                    .map_err(|e| e.to_string())?;
            } else {
                serde_json::to_writer(output, &masked).map_err(|e| e.to_string())?;
            }
        }
        _ => {
            output
                .write_all(raw.get().as_bytes())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn masked_original_field(
    encoded_key: &str,
    text: &str,
    output: &mut dyn Write,
) -> Result<(), String> {
    let _credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        encoded_key
            .len()
            .checked_mul(4)
            .and_then(|n| n.checked_add(64 << 10))
            .ok_or(LIMIT)?,
    )?;
    let key: String = serde_json::from_str(encoded_key).map_err(|e| e.to_string())?;
    if sensitive_key(&key) {
        return output.write_all(b"\"[oculto]\"").map_err(|e| e.to_string());
    }
    masked_envelope(text, output)
}
fn masked_envelope(envelope: &str, output: &mut dyn Write) -> Result<(), String> {
    // Bound simultaneous decoded-key/string + redaction working strings before
    // allocation; the original envelope remains charged by its native reader.
    let scratch = envelope
        .len()
        .checked_mul(8)
        .and_then(|n| n.checked_add(64 << 10))
        .ok_or(LIMIT)?;
    let _credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), scratch)?;
    write_masked(RawJson::checked(envelope)?, output, 0)
}

fn masked_metadata(
    key: &str,
    value: &serde_json::Value,
    output: &mut dyn Write,
) -> Result<(), String> {
    crate::operations::check()?;
    if sensitive_key(key) {
        return output.write_all(b"\"[oculto]\"").map_err(|e| e.to_string());
    }
    match value {
        serde_json::Value::Object(values) => {
            output.write_all(b"{").map_err(|e| e.to_string())?;
            for (index, (key, value)) in values.iter().enumerate() {
                if index != 0 {
                    output.write_all(b",").map_err(|e| e.to_string())?;
                }
                serde_json::to_writer(&mut *output, key).map_err(|e| e.to_string())?;
                output.write_all(b":").map_err(|e| e.to_string())?;
                masked_metadata(key, value, output)?;
            }
            output.write_all(b"}").map_err(|e| e.to_string())
        }
        serde_json::Value::Array(values) => {
            output.write_all(b"[").map_err(|e| e.to_string())?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.write_all(b",").map_err(|e| e.to_string())?;
                }
                masked_metadata("", value, output)?;
            }
            output.write_all(b"]").map_err(|e| e.to_string())
        }
        serde_json::Value::String(value) => {
            let _credit = crate::case_cache::reserve_work(
                crate::case_work_budget::global(),
                value
                    .len()
                    .checked_mul(8)
                    .and_then(|n| n.checked_add(64 << 10))
                    .ok_or(LIMIT)?,
            )?;
            serde_json::to_writer(output, &crate::workspace::redact(value))
                .map_err(|e| e.to_string())
        }
        _ => serde_json::to_writer(output, value).map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_wrapper_requires_complete_ordinal_maps_and_legacy_readers_reject_it() {
        let valid = r#"{"format":"loginsight.native-case","schemaVersion":3,"document":{"active":"case","cases":[{"id":"case","items":[]}]},"nativeEvidenceMap":{"schemaVersion":1,"cases":[{"caseOrdinal":0,"anchors":{"schemaVersion":1,"eventAliases":[],"itemAliases":[]},"containers":[],"members":[]}]}}"#;
        let parsed = document(valid).unwrap();
        assert_eq!(parsed.cases.len(), 1);
        assert!(parsed.transfer.is_some());
        assert!(
            crate::case_images::validate_document(&serde_json::from_str(valid).unwrap()).is_err()
        );
        for invalid in [valid.replace("\"caseOrdinal\":0","\"caseOrdinal\":1"),valid.replace("\"schemaVersion\":3","\"schemaVersion\":2"),valid.replace("\"nativeEvidenceMap\":","\"lostMap\":"),valid.replace("\"cases\":[{\"caseOrdinal\":0,\"anchors\":{\"schemaVersion\":1,\"eventAliases\":[],\"itemAliases\":[]},\"containers\":[],\"members\":[]}]","\"cases\":[]")] {
            assert!(document(&invalid).is_err(),"{invalid}");
        }
        assert!(
            document(r#"{"schemaVersion":2,"cases":[{"id":"case"}],"nativeEvidenceMap":{}}"#)
                .is_err()
        );
    }
    #[test]
    fn raw_document_spans_keep_unknown_members_and_numeric_lexemes() {
        let text = r#"{"schemaVersion":1,"cases":[{"id":"c","items":[{"rows":[{"near":18446744073709551614,"max":18446744073709551615,"i":1,"f":1.0,"z":-0.0,"tiny":2.547114365375239e-8,"future":{"x":1.000}}]}]}]}"#;
        let parsed = document(text).unwrap();
        let extracted = evidence::extract_case(parsed.cases[0].get()).unwrap();
        assert_eq!(
            extracted.containers[0].records[0].get(),
            r#"{"near":18446744073709551614,"max":18446744073709551615,"i":1,"f":1.0,"z":-0.0,"tiny":2.547114365375239e-8,"future":{"x":1.000}}"#
        );
    }
    #[test]
    fn string_masking_keeps_non_target_lexemes_and_unknown_structure() {
        let raw = r#"{"max":18446744073709551615,"one":1,"float":1.0,"minus":-0.0,"fraction":2.547114365375239e-8,"future":{"x":1.000},"password":"private","note":"token=secret","escaped":"\u0061"}"#;
        let mut output = Vec::new();
        masked_envelope(raw, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        for fragment in [
            "18446744073709551615",
            "\"one\":1,",
            "\"float\":1.0",
            "-0.0",
            "2.547114365375239e-8",
            "1.000",
            "\\u0061",
        ] {
            assert!(text.contains(fragment), "{fragment}");
        }
        assert!(text.contains("[oculto]"));
        assert!(!text.contains("private"));
        assert!(!text.contains("token=secret"));
        evidence::checked_document(&text).unwrap();
    }
    #[test]
    fn duplicate_case_identity_and_top_level_fields_are_rejected() {
        for text in [
            r#"{"cases":[{"id":"a"}],"cases":[{"id":"b"}]}"#,
            r#"{"cases":[{"id":"a"},{"id":"a"}]}"#,
            r#"{"cases":[{"id":"a","id":"b"}]}"#,
        ] {
            assert!(document(text).is_err());
        }
    }
}
