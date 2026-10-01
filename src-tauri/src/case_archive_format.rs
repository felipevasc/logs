//! Bounded, path-free portable Case transport. Entry bodies are streamed in
//! manifest order; no archive-provided path is ever joined to local storage.
//! Authentication, Case ownership and SQLite validation belong to the caller.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 16] = b"LOGSCASE\r\n\x1a\n\x01\x00\x00\x00";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 8 << 20;
pub(crate) const MAX_DOCUMENT_BYTES: u64 = 64 << 20;
const MAX_IMAGE_BYTES: u64 = 16 << 20;
const MAX_PAYLOAD_BYTES: u64 = 4 << 30;
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 32 << 30;
const MAX_IMAGES: usize = 1000;
pub(crate) const MAX_PAYLOADS: usize = 4096;
const COPY_BUFFER_BYTES: usize = 64 << 10;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum EntryKind {
    Document,
    Image(String),
    Exclusion(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Entry {
    #[serde(rename = "asset")]
    pub kind: EntryKind,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope<T> {
    format_version: u32,
    metadata: T,
    entries: Vec<Entry>,
}
pub(crate) struct Source {
    pub entry: Entry,
    pub file: File,
}
#[derive(Debug)]
pub(crate) struct StagedArchive<T> {
    pub metadata: T,
    pub entries: Vec<Entry>,
    directory: tempfile::TempDir,
}
impl<T> StagedArchive<T> {
    pub fn path(&self, index: usize) -> Result<PathBuf, String> {
        if index >= self.entries.len() {
            return Err("Entrada do arquivo portátil inexistente.".into());
        }
        // Only a locally generated integer becomes a filename.
        Ok(self.directory.path().join(index.to_string()))
    }
}

fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_entries(entries: &[Entry]) -> Result<u64, String> {
    if entries.first().map(|e| &e.kind) != Some(&EntryKind::Document)
        || entries.len() > 1 + MAX_IMAGES + MAX_PAYLOADS
    {
        return Err("Estrutura do arquivo portátil inválida ou excessiva.".into());
    }
    let (mut images, mut payloads, mut bytes) = (0, 0, 0u64);
    let mut seen = HashSet::new();
    for entry in entries {
        if !seen.insert(&entry.kind) || !digest_valid(&entry.sha256) {
            return Err("Entrada portátil duplicada ou com hash inválido.".into());
        }
        let limit = match &entry.kind {
            EntryKind::Document => MAX_DOCUMENT_BYTES,
            EntryKind::Image(id) => {
                images += 1;
                if !digest_valid(id) || id != &entry.sha256 {
                    return Err("Identificador de imagem portátil inválido.".into());
                }
                MAX_IMAGE_BYTES
            }
            EntryKind::Exclusion(id) => {
                payloads += 1;
                let parsed = uuid::Uuid::parse_str(id)
                    .map_err(|_| "Identificador de exclusão portátil inválido.")?;
                if parsed.is_nil() || parsed.to_string() != *id {
                    return Err("Identificador de exclusão portátil inválido.".into());
                }
                MAX_PAYLOAD_BYTES
            }
        };
        if entry.bytes == 0 || entry.bytes > limit {
            return Err("Uma entrada excede o limite do arquivo portátil.".into());
        }
        bytes = bytes
            .checked_add(entry.bytes)
            .ok_or("Arquivo portátil excessivo.")?;
    }
    if images > MAX_IMAGES || payloads > MAX_PAYLOADS || bytes > MAX_ARCHIVE_BYTES {
        return Err("O arquivo portátil excede 32 GiB, 1.000 imagens ou 4.096 payloads.".into());
    }
    Ok(bytes)
}

struct BoundedManifest(Vec<u8>);
impl Write for BoundedManifest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES.saturating_sub(self.0.len() as u64) {
            return Err(std::io::Error::other("O manifesto portátil excede 8 MiB."));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn copy_checked(
    input: &mut impl Read,
    output: &mut impl Write,
    entry: &Entry,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    let mut remaining = entry.bytes;
    let mut digest = Sha256::new();
    while remaining != 0 {
        if cancelled() {
            return Err("Operação cancelada.".into());
        }
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let count = input
            .read(&mut buffer[..wanted])
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("Arquivo portátil incompleto; nenhum Caso foi publicado.".into());
        }
        digest.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
        remaining -= count as u64;
    }
    if format!("{:x}", digest.finalize()) != entry.sha256 {
        return Err(
            "A integridade de uma entrada portátil não confere; nenhum Caso foi publicado.".into(),
        );
    }
    Ok(())
}

/// Preserve the old destination until the entire stream is written, checked and
/// synced. Sources are already leased and validated by their storage owner.
pub(crate) fn write<T: Serialize>(
    target: &Path,
    metadata: &T,
    sources: &mut [Source],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let entries: Vec<_> = sources.iter().map(|s| s.entry.clone()).collect();
    let bodies = validate_entries(&entries)?;
    let mut encoded = BoundedManifest(Vec::new());
    serde_json::to_writer(
        &mut encoded,
        &Envelope {
            format_version: 1,
            metadata,
            entries,
        },
    )
    .map_err(|e| e.to_string())?;
    let total = bodies
        .checked_add(24 + encoded.0.len() as u64)
        .ok_or("Arquivo portátil excessivo.")?;
    if total > MAX_ARCHIVE_BYTES {
        return Err("O arquivo portátil excede 32 GiB.".into());
    }
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut output = std::io::BufWriter::new(temporary.as_file_mut());
        output
            .write_all(MAGIC)
            .and_then(|_| output.write_all(&(encoded.0.len() as u64).to_le_bytes()))
            .and_then(|_| output.write_all(&encoded.0))
            .map_err(|e| e.to_string())?;
        for source in sources {
            if source.file.metadata().map_err(|e| e.to_string())?.len() != source.entry.bytes {
                return Err("Uma entrada mudou antes da exportação; tente novamente.".into());
            }
            source
                .file
                .seek(SeekFrom::Start(0))
                .map_err(|e| e.to_string())?;
            copy_checked(&mut source.file, &mut output, &source.entry, cancelled)?;
            if source.file.metadata().map_err(|e| e.to_string())?.len() != source.entry.bytes {
                return Err("Uma entrada mudou durante a exportação; tente novamente.".into());
            }
        }
        output.flush().map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    temporary.persist(target).map_err(|e| e.error.to_string())?;
    Ok(())
}

pub(crate) fn is_archive(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0u8; MAGIC.len()];
    // Recognize the family even when a newer version must be rejected by read.
    Ok(file.read_exact(&mut magic).is_ok() && magic[..12] == MAGIC[..12])
}

/// Every body is verified before returning. Temporary files are automatically
/// removed on any error, including cancellation, hash mismatch and truncation.
pub(crate) fn read<T: DeserializeOwned>(
    path: &Path,
    staging_parent: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<StagedArchive<T>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_ARCHIVE_BYTES {
        return Err("O arquivo portátil excede 32 GiB.".into());
    }
    let mut input = std::io::BufReader::new(file);
    let mut magic = [0u8; MAGIC.len()];
    input
        .read_exact(&mut magic)
        .map_err(|_| "Cabeçalho portátil incompleto.")?;
    if magic != *MAGIC {
        return Err("Formato portátil não suportado.".into());
    }
    let mut length = [0u8; 8];
    input
        .read_exact(&mut length)
        .map_err(|_| "Manifesto portátil incompleto.")?;
    let length = u64::from_le_bytes(length);
    if length == 0 || length > MAX_MANIFEST_BYTES {
        return Err("Manifesto portátil inválido ou maior que 8 MiB.".into());
    }
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    let mut encoded = vec![0u8; length as usize];
    input
        .read_exact(&mut encoded)
        .map_err(|_| "Manifesto portátil incompleto.")?;
    let manifest: Envelope<T> = serde_json::from_slice(&encoded)
        .map_err(|e| format!("Manifesto portátil inválido: {e}"))?;
    drop(encoded);
    if manifest.format_version != 1 {
        return Err("O arquivo portátil exige uma versão mais recente.".into());
    }
    let bodies = validate_entries(&manifest.entries)?;
    if bodies.checked_add(24 + length) != Some(size) {
        return Err("O tamanho do arquivo portátil não confere com o manifesto.".into());
    }
    let directory = tempfile::Builder::new()
        .prefix("case-import-")
        .tempdir_in(staging_parent)
        .map_err(|e| e.to_string())?;
    for (index, entry) in manifest.entries.iter().enumerate() {
        let mut file =
            File::create(directory.path().join(index.to_string())).map_err(|e| e.to_string())?;
        copy_checked(&mut input, &mut file, entry, cancelled)?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let mut trailing = [0u8; 1];
    if input.read(&mut trailing).map_err(|e| e.to_string())? != 0 {
        return Err("O arquivo portátil contém dados após a última entrada.".into());
    }
    if cancelled() {
        return Err("Operação cancelada.".into());
    }
    Ok(StagedArchive {
        metadata: manifest.metadata,
        entries: manifest.entries,
        directory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn entry(kind: EntryKind, bytes: &[u8]) -> Entry {
        Entry {
            kind,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }
    fn source(root: &Path, name: &str, kind: EntryKind, bytes: &[u8]) -> Source {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        Source {
            entry: entry(kind, bytes),
            file: File::open(path).unwrap(),
        }
    }
    fn raw_archive(path: &Path, metadata: Value, entries: Vec<Entry>, bodies: &[u8]) {
        let manifest = serde_json::to_vec(&Envelope {
            format_version: 1,
            metadata,
            entries,
        })
        .unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&(manifest.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(&manifest).unwrap();
        file.write_all(bodies).unwrap();
    }

    #[test]
    fn portable_stream_roundtrips_without_foreign_paths() {
        let root = tempfile::tempdir().unwrap();
        let image = b"synthetic image body";
        let id = format!("{:x}", Sha256::digest(image));
        let exclusion = uuid::Uuid::new_v4().to_string();
        let mut sources = [
            source(
                root.path(),
                "document",
                EntryKind::Document,
                b"{\"cases\":[]}",
            ),
            source(root.path(), "image", EntryKind::Image(id), image),
            source(
                root.path(),
                "payload",
                EntryKind::Exclusion(exclusion),
                b"synthetic payload body",
            ),
        ];
        let path = root.path().join("case.licase");
        let metadata = json!({"foreignCaseId":"../never-a-local-path","batches":[]});
        write(&path, &metadata, &mut sources, &|| false).unwrap();
        let encoded = std::fs::read(&path).unwrap();
        let length = u64::from_le_bytes(encoded[16..24].try_into().unwrap()) as usize;
        let manifest: Value = serde_json::from_slice(&encoded[24..24 + length]).unwrap();
        assert_eq!(manifest["entries"][0]["asset"], json!({"kind":"document"}));
        assert_eq!(manifest["entries"][1]["asset"]["kind"], "image");
        assert!(is_archive(&path).unwrap());
        let staged: StagedArchive<Value> = read(&path, root.path(), &|| false).unwrap();
        assert_eq!(staged.metadata, metadata);
        assert_eq!(staged.entries.len(), 3);
        assert_eq!(std::fs::read(staged.path(1).unwrap()).unwrap(), image);
        assert_eq!(staged.path(1).unwrap().file_name().unwrap(), "1");
        assert!(staged.path(3).is_err());
    }

    #[test]
    fn portable_stream_rejects_corruption_truncation_and_trailing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("case.licase");
        for bytes in [b"wrong".as_slice(), b"ok".as_slice(), b"okay!".as_slice()] {
            raw_archive(
                &path,
                json!({}),
                vec![entry(EntryKind::Document, b"okay")],
                bytes,
            );
            assert!(read::<Value>(&path, root.path(), &|| false).is_err());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn portable_manifest_bounds_and_identifiers_precede_staging() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("case.licase");
        let document = entry(EntryKind::Document, b"{}");
        for entries in [
            vec![],
            vec![document.clone(), document.clone()],
            vec![
                document.clone(),
                entry(EntryKind::Exclusion("../../escape".into()), b"x"),
            ],
            vec![
                document.clone(),
                entry(EntryKind::Image("../escape".into()), b"x"),
            ],
            vec![Entry {
                bytes: MAX_DOCUMENT_BYTES + 1,
                ..document.clone()
            }],
        ] {
            raw_archive(&path, json!({}), entries, b"{}");
            assert!(read::<Value>(&path, root.path(), &|| false).is_err());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        }
        let mut file = File::create(&path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&(MAX_MANIFEST_BYTES + 1).to_le_bytes())
            .unwrap();
        assert!(read::<Value>(&path, root.path(), &|| false)
            .unwrap_err()
            .contains("8 MiB"));
        let mut budget = BoundedManifest(Vec::new());
        assert!(budget
            .write_all(&vec![0; MAX_MANIFEST_BYTES as usize + 1])
            .is_err());
        assert!(budget.0.is_empty());
    }

    #[test]
    fn portable_failed_export_preserves_destination_and_sources() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("case.licase");
        std::fs::write(&target, b"existing export").unwrap();
        let mut original = source(root.path(), "source", EntryKind::Document, b"evidence");
        original.entry.sha256 = "0".repeat(64);
        assert!(write(&target, &json!({}), &mut [original], &|| false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing export");
        assert_eq!(
            std::fs::read(root.path().join("source")).unwrap(),
            b"evidence"
        );
        let good = source(root.path(), "source", EntryKind::Document, b"evidence");
        assert!(write(&target, &json!({}), &mut [good], &|| true).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing export");
    }

    #[test]
    fn portable_cancelled_import_removes_partial_staging() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("case.licase");
        let content = vec![b'x'; COPY_BUFFER_BYTES * 4];
        raw_archive(
            &path,
            json!({}),
            vec![entry(EntryKind::Document, &content)],
            &content,
        );
        let polls = std::cell::Cell::new(0);
        assert!(read::<Value>(&path, root.path(), &|| {
            polls.set(polls.get() + 1);
            polls.get() > 2
        })
        .is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn portable_payload_copy_never_requests_more_than_its_fixed_buffer() {
        struct Repeated {
            remaining: u64,
            largest_read: usize,
        }
        impl Read for Repeated {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.largest_read = self.largest_read.max(buffer.len());
                let count = self.remaining.min(buffer.len() as u64) as usize;
                buffer[..count].fill(b'x');
                self.remaining -= count as u64;
                Ok(count)
            }
        }
        let chunk = [b'x'; COPY_BUFFER_BYTES];
        let mut digest = Sha256::new();
        for _ in 0..128 {
            digest.update(chunk);
        }
        let bytes = 128 * COPY_BUFFER_BYTES as u64;
        let entry = Entry {
            kind: EntryKind::Exclusion(uuid::Uuid::new_v4().to_string()),
            bytes,
            sha256: format!("{:x}", digest.finalize()),
        };
        let mut source = Repeated {
            remaining: bytes,
            largest_read: 0,
        };
        copy_checked(&mut source, &mut std::io::sink(), &entry, &|| false).unwrap();
        assert_eq!(source.remaining, 0);
        assert_eq!(source.largest_read, COPY_BUFFER_BYTES);
    }
}
