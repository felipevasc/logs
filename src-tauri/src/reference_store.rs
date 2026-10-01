//! Case-owned, immutable reference files. This deliberately does not hook a
//! per-record SQLite probe into the large-source evaluator: callers must choose
//! a bounded interactive probe or prepare a derived source variant in batches.
use crate::analysis_context::ReferenceDescriptor;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
};

pub(crate) const VERSION: u32 = 1;
const PAGE_BYTES: u64 = 4096;
const MANIFEST_BYTES: u64 = 4 << 20;
const KEY_BYTES: usize = 64 << 10;
const BATCH_KEYS: usize = 128;
const BATCH_BYTES: usize = 2 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Owner {
    pub case_id: String,
    pub analysis_id: String,
}

/// Config/visibility revisions belong to the admitted runtime request, not to
/// immutable bytes. Changing schema or lookup semantics changes this identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Version {
    pub store_version: u32,
    pub owner: Owner,
    pub reference_id: String,
    pub content_sha256: String,
    pub schema_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreparedReference {
    pub version: Version,
    pub row_count: u64,
    pub source_bytes: u64,
    pub database_bytes: u64,
    pub database_sha256: String,
    pub max_record_bytes: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    prepared: PreparedReference,
    sha256: String,
}

fn manifest_bytes(prepared: &PreparedReference) -> Result<Vec<u8>, Error> {
    json(&Manifest {
        prepared: prepared.clone(),
        sha256: digest(&json(prepared)?),
    })
}

/// Payload/storage ceilings, not a process RSS claim. SQLite has one 2 MiB
/// suggested page cache; parser/JSON/allocator/library overhead is additional.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub source_bytes: u64,
    pub database_bytes: u64,
    pub record_bytes: usize,
    pub rows: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: 256 << 20,
            database_bytes: 512 << 20,
            record_bytes: 512 << 10,
            rows: 2_000_000,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    InvalidDescriptor,
    UnsupportedFormat,
    Limit(&'static str),
    InvalidRecord(u64),
    InvalidKey,
    DuplicateKey(u64),
    HashMismatch,
    Unavailable,
    Busy,
    Corrupt,
    Cancelled,
    Io(io::Error),
    Database(rusqlite::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDescriptor => {
                f.write_str("Descritor ou proprietário da referência inválido.")
            }
            Self::UnsupportedFormat => {
                f.write_str("Esta preparação de referência aceita somente JSONL.")
            }
            Self::Limit(limit) => write!(f, "A referência excede o limite de {limit}."),
            Self::InvalidRecord(line) => {
                write!(f, "Registro de referência inválido na linha {line}.")
            }
            Self::InvalidKey => {
                f.write_str("A chave deve conter escalares JSON não nulos, na ordem declarada.")
            }
            Self::DuplicateKey(line) => write!(
                f,
                "Chave de referência duplicada na linha {line}; preparação descartada."
            ),
            Self::HashMismatch => {
                f.write_str("O conteúdo da referência não corresponde à versão declarada.")
            }
            Self::Unavailable => {
                f.write_str("Os bytes desta referência não estão disponíveis neste Caso.")
            }
            Self::Busy => f.write_str("A referência está em uso por uma operação exclusiva."),
            Self::Corrupt => f.write_str("A referência preparada está incompleta ou corrompida."),
            Self::Cancelled => f.write_str("Preparação ou consulta da referência cancelada."),
            Self::Io(error) => write!(f, "Falha de leitura ou gravação da referência: {error}"),
            Self::Database(error) => write!(f, "Falha no índice da referência: {error}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

fn check(cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(|_| Error::Corrupt)
}
fn version(owner: &Owner, descriptor: &ReferenceDescriptor) -> Result<Version, Error> {
    let valid_text = |text: &str, max| !text.trim().is_empty() && text.len() <= max;
    let columns: HashSet<_> = descriptor.columns.iter().collect();
    if !valid_text(&owner.case_id, 256)
        || !valid_text(&owner.analysis_id, 256)
        || descriptor.schema_version != VERSION
        || !valid_text(&descriptor.id, 256)
        || !valid_text(&descriptor.name, 4096)
        || descriptor.content_sha256.len() != 64
        || !descriptor
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || columns.is_empty()
        || columns.len() > 1024
        || columns.len() != descriptor.columns.len()
        || columns.iter().any(|column| !valid_text(column, 4096))
        || descriptor.key_columns.is_empty()
        || descriptor.key_columns.len() > 16
        || descriptor.key_columns.iter().collect::<HashSet<_>>().len()
            != descriptor.key_columns.len()
        || descriptor
            .key_columns
            .iter()
            .any(|key| !columns.contains(key))
        || descriptor.duplicate_policy != "reject"
    {
        return Err(Error::InvalidDescriptor);
    }
    if descriptor.format != "jsonl" {
        return Err(Error::UnsupportedFormat);
    }
    let schema = json(&(
        VERSION,
        descriptor.schema_version,
        &descriptor.format,
        &descriptor.columns,
        &descriptor.key_columns,
        &descriptor.duplicate_policy,
    ))?;
    if schema.len() as u64 > MANIFEST_BYTES {
        return Err(Error::InvalidDescriptor);
    }
    Ok(Version {
        store_version: VERSION,
        owner: owner.clone(),
        reference_id: descriptor.id.clone(),
        content_sha256: descriptor.content_sha256.to_ascii_lowercase(),
        schema_sha256: digest(&schema),
    })
}
fn directory(root: &Path, version: &Version) -> Result<PathBuf, Error> {
    Ok(root
        .join("references-v1")
        .join(digest(&json(&version.owner)?))
        .join(digest(&json(version)?)))
}
fn bounded_limits(limits: Limits) -> Result<Limits, Error> {
    let max = Limits::default();
    if limits.source_bytes > max.source_bytes
        || limits.database_bytes > max.database_bytes
        || limits.record_bytes > max.record_bytes
        || limits.rows > max.rows
        || limits.database_bytes < PAGE_BYTES * 3
        || limits.record_bytes == 0
    {
        return Err(Error::Limit("configuração de recursos"));
    }
    Ok(limits)
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("payload limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded_json<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<Vec<u8>, Error> {
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::Limit("bytes do valor"))?;
    Ok(writer.bytes)
}
fn key(values: &[&Value], expected: usize) -> Result<Vec<u8>, Error> {
    if values.len() != expected
        || values
            .iter()
            .any(|value| value.is_null() || value.is_array() || value.is_object())
    {
        return Err(Error::InvalidKey);
    }
    // JSON serialization preserves scalar types and key-component boundaries.
    // No coercion, trimming, case folding, or delimiter concatenation occurs.
    bounded_json(values, KEY_BYTES)
}

/// Reject duplicate top-level column names rather than serde_json's usual
/// last-key-wins interpretation. The descriptor defines literal column names.
struct Record(BTreeMap<String, Value>);
impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Record;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object with unique column names")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut input: A,
            ) -> Result<Record, A::Error> {
                let mut row = BTreeMap::new();
                while let Some((name, value)) = input.next_entry::<String, Value>()? {
                    if row.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate column"));
                    }
                }
                Ok(Record(row))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

fn hash_file(
    path: &Path,
    expected_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, Error> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(Error::Corrupt);
    }
    let mut file = File::open(path)?;
    if file.metadata()?.len() != expected_bytes {
        return Err(Error::Corrupt);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 << 10];
    let mut remaining = expected_bytes;
    while remaining > 0 {
        check(cancelled)?;
        let capacity = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..capacity])?;
        if read == 0 {
            return Err(Error::Corrupt);
        }
        remaining -= read as u64;
        hasher.update(&buffer[..read]);
    }
    if file.read(&mut buffer[..1])? != 0 {
        return Err(Error::Corrupt);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn sync_directory(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// No Case/config/source mutation occurs here. The caller separately admits
/// the prepared version under the Case's compare-and-swap config revision.
pub(crate) fn prepare_jsonl(
    root: &Path,
    owner: &Owner,
    descriptor: &ReferenceDescriptor,
    input: impl Read,
    limits: Limits,
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedReference, Error> {
    let limits = bounded_limits(limits)?;
    let version = version(owner, descriptor)?;
    check(cancelled)?;
    let target = directory(root, &version)?;
    let parent = target.parent().ok_or(Error::InvalidDescriptor)?;
    fs::create_dir_all(root)?;
    for path in [parent.parent().ok_or(Error::InvalidDescriptor)?, parent] {
        match fs::create_dir(path) {
            Ok(()) => sync_directory(path.parent().ok_or(Error::InvalidDescriptor)?)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(Error::Io(error)),
        }
        if !fs::symlink_metadata(path)?.file_type().is_dir() {
            return Err(Error::Corrupt);
        }
    }
    let staging = tempfile::Builder::new()
        .prefix(".prepare-")
        .tempdir_in(parent)?;
    let lease = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(staging.path().join("lease"))?;
    FileExt::try_lock_exclusive(&lease).map_err(|_| Error::Busy)?;
    let mut source = File::create(staging.path().join("source.jsonl"))?;
    let database_path = staging.path().join("rows.sqlite");
    let mut connection = Connection::open(&database_path)?;
    connection.execute_batch(
        "PRAGMA page_size=4096; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
        PRAGMA temp_store=FILE; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        CREATE TABLE rows(key BLOB NOT NULL PRIMARY KEY, body BLOB NOT NULL) WITHOUT ROWID;",
    )?;
    connection.pragma_update(None, "max_page_count", limits.database_bytes / PAGE_BYTES)?;
    let transaction = connection.transaction()?;
    let mut hasher = Sha256::new();
    let mut source_bytes = 0u64;
    let mut row_count = 0u64;
    let mut input = BufReader::with_capacity(64 << 10, input);
    let mut line = Vec::new();
    {
        let mut insert = transaction.prepare("INSERT INTO rows(key,body) VALUES(?1,?2)")?;
        loop {
            check(cancelled)?;
            line.clear();
            (&mut input)
                .take(limits.record_bytes as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if line.is_empty() {
                break;
            }
            if line.len() > limits.record_bytes {
                return Err(Error::Limit("bytes por registro"));
            }
            source_bytes = source_bytes
                .checked_add(line.len() as u64)
                .ok_or(Error::Limit("bytes da fonte"))?;
            if source_bytes > limits.source_bytes {
                return Err(Error::Limit("bytes da fonte"));
            }
            row_count += 1;
            if row_count > limits.rows {
                return Err(Error::Limit("registros"));
            }
            let Record(row) = serde_json::from_slice::<Record>(&line)
                .map_err(|_| Error::InvalidRecord(row_count))?;
            if row.len() != descriptor.columns.len()
                || descriptor
                    .columns
                    .iter()
                    .any(|name| !row.contains_key(name))
            {
                return Err(Error::InvalidRecord(row_count));
            }
            let keys: Vec<_> = descriptor
                .key_columns
                .iter()
                .map(|name| &row[name])
                .collect();
            let key = key(&keys, descriptor.key_columns.len())?;
            let body = bounded_json(&row, limits.record_bytes)?;
            if let Err(error) = insert.execute(params![key, body]) {
                if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY)
                {
                    return Err(Error::DuplicateKey(row_count));
                }
                return Err(Error::Database(error));
            }
            source.write_all(&line)?;
            hasher.update(&line);
        }
    }
    if format!("{:x}", hasher.finalize()) != version.content_sha256 {
        return Err(Error::HashMismatch);
    }
    check(cancelled)?;
    source.sync_all()?;
    if hash_file(
        &staging.path().join("source.jsonl"),
        source_bytes,
        cancelled,
    )? != version.content_sha256
    {
        return Err(Error::Corrupt);
    }
    transaction.commit()?;
    connection
        .close()
        .map_err(|(_, error)| Error::Database(error))?;
    File::open(&database_path)?.sync_all()?;
    let database_bytes = fs::metadata(&database_path)?.len();
    if database_bytes > limits.database_bytes {
        return Err(Error::Limit("disco do índice"));
    }
    let database_sha256 = hash_file(&database_path, database_bytes, cancelled)?;
    let prepared = PreparedReference {
        version,
        row_count,
        source_bytes,
        database_bytes,
        database_sha256,
        max_record_bytes: limits.record_bytes,
    };
    let mut ready = File::create(staging.path().join("ready.pending"))?;
    ready.write_all(&manifest_bytes(&prepared)?)?;
    ready.sync_all()?;
    drop(ready);
    fs::rename(
        staging.path().join("ready.pending"),
        staging.path().join("ready.json"),
    )?;
    sync_directory(staging.path())?;
    check(cancelled)?;
    // Unlock/close before directory rename for Windows; no reader knows staging.
    drop(source);
    drop(lease);
    match fs::rename(staging.path(), &target) {
        Ok(()) => {
            sync_directory(parent)?;
            Ok(prepared)
        }
        Err(error) if target.exists() => {
            // Another complete publisher may have won. Never replace its bytes.
            let existing = open(root, owner, descriptor, cancelled)?;
            if existing.prepared.row_count != row_count
                || existing.prepared.source_bytes != source_bytes
            {
                return Err(Error::Corrupt);
            }
            let _ = error;
            Ok(existing.prepared.clone())
        }
        Err(error) => Err(Error::Io(error)),
    }
}

/// Retain this reader for a prepared operation: every open fully verifies the
/// source and database hashes. Its shared lease must outlive all consumers.
/// Future cleanup must acquire the same lease exclusively before removal.
pub(crate) struct ReferenceReader {
    connection: Connection,
    prepared: PreparedReference,
    descriptor: ReferenceDescriptor,
    _lease: File,
}
pub(crate) fn open(
    root: &Path,
    owner: &Owner,
    descriptor: &ReferenceDescriptor,
    cancelled: &dyn Fn() -> bool,
) -> Result<ReferenceReader, Error> {
    let expected = version(owner, descriptor)?;
    check(cancelled)?;
    let directory = directory(root, &expected)?;
    // The root is application-selected. No descriptor or manifest supplies a
    // path, and version/owner components below that root may not be symlinks.
    for path in [
        directory.parent().and_then(Path::parent),
        directory.parent(),
        Some(directory.as_path()),
    ]
    .into_iter()
    .flatten()
    {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(Error::Corrupt),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Error::Unavailable)
            }
            Err(error) => return Err(Error::Io(error)),
        }
    }
    for name in ["lease", "ready.json"] {
        match fs::symlink_metadata(directory.join(name)) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err(Error::Corrupt),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Error::Unavailable)
            }
            Err(error) => return Err(Error::Io(error)),
        }
    }
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("lease"))
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::Unavailable
            } else {
                Error::Io(error)
            }
        })?;
    FileExt::try_lock_shared(&lease).map_err(|_| Error::Busy)?;
    let file = File::open(directory.join("ready.json")).map_err(|_| Error::Corrupt)?;
    if file.metadata()?.len() > MANIFEST_BYTES {
        return Err(Error::Corrupt);
    }
    let mut bytes = Vec::new();
    file.take(MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_BYTES {
        return Err(Error::Corrupt);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt)?;
    if digest(&json(&manifest.prepared)?) != manifest.sha256 {
        return Err(Error::Corrupt);
    }
    let prepared = manifest.prepared;
    let limits = Limits::default();
    if prepared.version != expected
        || prepared.row_count > limits.rows
        || prepared.source_bytes > limits.source_bytes
        || prepared.database_bytes > limits.database_bytes
        || prepared.max_record_bytes == 0
        || prepared.max_record_bytes > limits.record_bytes
    {
        return Err(Error::Corrupt);
    }
    let database = directory.join("rows.sqlite");
    if hash_file(
        &directory.join("source.jsonl"),
        prepared.source_bytes,
        cancelled,
    )? != expected.content_sha256
        || hash_file(&database, prepared.database_bytes, cancelled)? != prepared.database_sha256
    {
        return Err(Error::Corrupt);
    }
    check(cancelled)?;
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA trusted_schema=OFF;")?;
    Ok(ReferenceReader {
        connection,
        prepared,
        descriptor: descriptor.clone(),
        _lease: lease,
    })
}
impl ReferenceReader {
    pub(crate) fn prepared(&self) -> &PreparedReference {
        &self.prepared
    }

    pub(crate) fn lookup(&self, keys: &[Value], column: &str) -> Result<Option<Value>, Error> {
        let mut result = self.lookup_many(&[keys], column, &|| false)?;
        Ok(result.pop().flatten())
    }

    /// Bounded interactive batch. This reuses a statement, but is still B-tree
    /// probing, not a vectorized join or an acceptable implicit 50M-row scan.
    pub(crate) fn lookup_many(
        &self,
        keys: &[&[Value]],
        column: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Option<Value>>, Error> {
        check(cancelled)?;
        if keys.len() > BATCH_KEYS {
            return Err(Error::Limit("chaves por lote"));
        }
        if !self.descriptor.columns.iter().any(|name| name == column) {
            return Err(Error::InvalidDescriptor);
        }
        let mut query = self
            .connection
            .prepare_cached("SELECT body FROM rows WHERE key=?1")?;
        let mut output = Vec::with_capacity(keys.len());
        let mut key_bytes = 0usize;
        let mut output_bytes = 0usize;
        for values in keys {
            check(cancelled)?;
            if values.len() != self.descriptor.key_columns.len() {
                return Err(Error::InvalidKey);
            }
            let encoded = key(
                &values.iter().collect::<Vec<_>>(),
                self.descriptor.key_columns.len(),
            )?;
            key_bytes = key_bytes.saturating_add(encoded.len());
            if key_bytes > BATCH_BYTES {
                return Err(Error::Limit("bytes de chaves por lote"));
            }
            let value = query
                .query_row(params![encoded], |row| {
                    let blob = row.get_ref(0)?.as_blob()?;
                    if blob.len() > self.prepared.max_record_bytes {
                        return Ok(Err(Error::Corrupt));
                    }
                    let Record(mut body) = match serde_json::from_slice::<Record>(blob) {
                        Ok(body) => body,
                        Err(_) => return Ok(Err(Error::Corrupt)),
                    };
                    let Some(value) = body.remove(column) else {
                        return Ok(Err(Error::Corrupt));
                    };
                    let bytes = match bounded_json(&value, BATCH_BYTES.saturating_sub(output_bytes))
                    {
                        Ok(bytes) => bytes.len(),
                        Err(error) => return Ok(Err(error)),
                    };
                    output_bytes += bytes;
                    Ok(Ok(value))
                })
                .optional()?
                .transpose()?;
            output.push(value);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn owner(name: &str) -> Owner {
        Owner {
            case_id: name.into(),
            analysis_id: "analysis-a".into(),
        }
    }
    fn descriptor(data: &[u8]) -> ReferenceDescriptor {
        ReferenceDescriptor {
            schema_version: 1,
            id: "reference-a".into(),
            name: "Hosts".into(),
            content_sha256: digest(data),
            format: "jsonl".into(),
            columns: vec!["key".into(), "value".into()],
            key_columns: vec!["key".into()],
            duplicate_policy: "reject".into(),
        }
    }
    fn build(root: &Path, data: &[u8]) -> Result<PreparedReference, Error> {
        prepare_jsonl(
            root,
            &owner("case-a"),
            &descriptor(data),
            data,
            Limits::default(),
            &|| false,
        )
    }
    #[test]
    fn typed_keys_null_values_and_missing_results_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"key\":1,\"value\":\"integer\"}\n{\"key\":1.0,\"value\":\"float\"}\n{\"key\":\"1\",\"value\":null}\n{\"key\":true,\"value\":{\"name\":\"host\"}}";
        assert_eq!(build(dir.path(), data).unwrap().row_count, 4);
        let reader = open(dir.path(), &owner("case-a"), &descriptor(data), &|| false).unwrap();
        assert_eq!(
            reader.lookup(&[json!(1)], "value").unwrap(),
            Some(json!("integer"))
        );
        assert_eq!(
            reader.lookup(&[json!(1.0)], "value").unwrap(),
            Some(json!("float"))
        );
        assert_eq!(
            reader.lookup(&[json!("1")], "value").unwrap(),
            Some(Value::Null)
        );
        assert_eq!(
            reader.lookup(&[json!(true)], "value").unwrap(),
            Some(json!({"name":"host"}))
        );
        assert_eq!(reader.lookup(&[json!("missing")], "value").unwrap(), None);
        for key in [Value::Null, json!([]), json!({})] {
            assert!(matches!(
                reader.lookup(&[key], "value"),
                Err(Error::InvalidKey)
            ));
        }
    }
    #[test]
    fn rejects_duplicates_bad_rows_and_unbounded_inputs_without_publication() {
        for (data, duplicate) in [
            (
                b"{\"key\":1,\"value\":\"a\"}\n{\"key\":1,\"value\":\"b\"}\n".as_slice(),
                true,
            ),
            (b"{\"key\":null,\"value\":1}\n".as_slice(), false),
            (b"{\"key\":1,\"key\":2,\"value\":1}\n".as_slice(), false),
            (b"{\"key\":1}\n".as_slice(), false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let error = build(dir.path(), data).unwrap_err();
            if duplicate {
                assert!(matches!(error, Error::DuplicateKey(2)));
            }
            assert!(matches!(
                open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
                Err(Error::Unavailable)
            ));
            let parent = directory(
                dir.path(),
                &version(&owner("case-a"), &descriptor(data)).unwrap(),
            )
            .unwrap();
            assert_eq!(fs::read_dir(parent.parent().unwrap()).unwrap().count(), 0);
        }
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"key\":1,\"value\":\"hello\"}\n";
        for limits in [
            Limits {
                rows: 0,
                ..Default::default()
            },
            Limits {
                record_bytes: 8,
                ..Default::default()
            },
            Limits {
                source_bytes: 2,
                ..Default::default()
            },
        ] {
            assert!(matches!(
                prepare_jsonl(
                    dir.path(),
                    &owner("case-a"),
                    &descriptor(data),
                    &data[..],
                    limits,
                    &|| false
                ),
                Err(Error::Limit(_))
            ));
        }
    }
    #[test]
    fn verifies_hash_ownership_and_schema_on_each_open() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"key\":1,\"value\":\"one\"}\n";
        let prepared = build(dir.path(), data).unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-b"), &descriptor(data), &|| false),
            Err(Error::Unavailable)
        ));
        let mut changed = descriptor(data);
        changed.key_columns = vec!["value".into()];
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &changed, &|| false),
            Err(Error::Unavailable)
        ));
        changed = descriptor(data);
        changed.content_sha256 = "0".repeat(64);
        assert!(matches!(
            prepare_jsonl(
                dir.path(),
                &owner("case-a"),
                &changed,
                &data[..],
                Limits::default(),
                &|| false
            ),
            Err(Error::HashMismatch)
        ));
        let path = directory(dir.path(), &prepared.version).unwrap();
        fs::write(
            path.join("source.jsonl"),
            b"{\"key\":1,\"value\":\"two\"}\n",
        )
        .unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
            Err(Error::Corrupt)
        ));
    }
    #[test]
    fn read_lease_reuse_and_cancellation_preserve_existing_version() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"key\":1,\"value\":\"one\"}\n";
        let prepared = build(dir.path(), data).unwrap();
        let reader = open(dir.path(), &owner("case-a"), &descriptor(data), &|| false).unwrap();
        assert_eq!(build(dir.path(), data).unwrap(), prepared);
        let lease = File::open(
            directory(dir.path(), &prepared.version)
                .unwrap()
                .join("lease"),
        )
        .unwrap();
        assert!(FileExt::try_lock_exclusive(&lease).is_err());
        assert!(matches!(
            prepare_jsonl(
                dir.path(),
                &owner("case-a"),
                &descriptor(data),
                &data[..],
                Limits::default(),
                &|| true
            ),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            reader.lookup_many(&[&[json!(1)]], "value", &|| true),
            Err(Error::Cancelled)
        ));
        assert_eq!(
            reader.lookup(&[json!(1)], "value").unwrap(),
            Some(json!("one"))
        );
        drop(reader);
        FileExt::try_lock_exclusive(&lease).unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
            Err(Error::Busy)
        ));
    }
    #[test]
    fn composite_keys_do_not_collide_and_batches_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let data =
            b"{\"a\":\"x|y\",\"b\":\"z\",\"value\":1}\n{\"a\":\"x\",\"b\":\"y|z\",\"value\":2}\n";
        let mut desc = descriptor(data);
        desc.columns = vec!["a".into(), "b".into(), "value".into()];
        desc.key_columns = vec!["a".into(), "b".into()];
        prepare_jsonl(
            dir.path(),
            &owner("case-a"),
            &desc,
            &data[..],
            Limits::default(),
            &|| false,
        )
        .unwrap();
        let reader = open(dir.path(), &owner("case-a"), &desc, &|| false).unwrap();
        let first = [json!("x|y"), json!("z")];
        let second = [json!("x"), json!("y|z")];
        assert_eq!(
            reader
                .lookup_many(&[&first, &second], "value", &|| false)
                .unwrap(),
            vec![Some(json!(1)), Some(json!(2))]
        );
        assert!(matches!(
            reader.lookup_many(&vec![first.as_slice(); BATCH_KEYS + 1], "value", &|| false),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            reader.lookup(&[json!("x".repeat(KEY_BYTES)), json!("z")], "value"),
            Err(Error::Limit(_))
        ));
    }

    #[test]
    fn case_sensitive_columns_and_descriptor_changes_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"Key\":\"A\",\"key\":\"a\"}\n";
        let mut desc = descriptor(data);
        desc.columns = vec!["Key".into(), "key".into()];
        desc.key_columns = vec!["Key".into()];
        prepare_jsonl(
            dir.path(),
            &owner("case-a"),
            &desc,
            &data[..],
            Limits::default(),
            &|| false,
        )
        .unwrap();
        let reader = open(dir.path(), &owner("case-a"), &desc, &|| false).unwrap();
        assert_eq!(
            reader.lookup(&[json!("A")], "key").unwrap(),
            Some(json!("a"))
        );
        assert_eq!(reader.lookup(&[json!("a")], "key").unwrap(), None);
        let mut duplicate = desc.clone();
        duplicate.columns = vec!["Key".into(), "Key".into()];
        assert!(matches!(
            version(&owner("case-a"), &duplicate),
            Err(Error::InvalidDescriptor)
        ));
        let mut same = desc.clone();
        same.name = "Renamed".into();
        same.content_sha256.make_ascii_uppercase();
        assert_eq!(
            version(&owner("case-a"), &desc).unwrap(),
            version(&owner("case-a"), &same).unwrap()
        );
        same.id = "other".into();
        assert_ne!(
            version(&owner("case-a"), &desc).unwrap(),
            version(&owner("case-a"), &same).unwrap()
        );
    }

    #[test]
    fn cancellation_after_ready_manifest_discards_only_staging() {
        let dir = tempfile::tempdir().unwrap();
        let old = b"{\"key\":1,\"value\":\"old\"}\n";
        let new = b"{\"key\":1,\"value\":\"new\"}\n";
        let original = build(dir.path(), old).unwrap();
        let target = directory(
            dir.path(),
            &version(&owner("case-a"), &descriptor(new)).unwrap(),
        )
        .unwrap();
        let cancelled = || {
            fs::read_dir(target.parent().unwrap())
                .unwrap()
                .flatten()
                .any(|entry| {
                    entry.file_name().to_string_lossy().starts_with(".prepare-")
                        && entry.path().join("ready.json").exists()
                })
        };
        assert!(matches!(
            prepare_jsonl(
                dir.path(),
                &owner("case-a"),
                &descriptor(new),
                &new[..],
                Limits::default(),
                &cancelled
            ),
            Err(Error::Cancelled)
        ));
        assert!(!target.exists());
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
        let reader = open(dir.path(), &owner("case-a"), &descriptor(old), &|| false).unwrap();
        assert_eq!(reader.prepared(), &original);
        assert_eq!(
            reader.lookup(&[json!(1)], "value").unwrap(),
            Some(json!("old"))
        );
    }

    #[test]
    fn database_limit_and_batch_output_limit_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let data = serde_json::to_vec(&json!({"key":1, "value":"x".repeat(40 << 10)})).unwrap();
        assert!(matches!(
            prepare_jsonl(
                dir.path(),
                &owner("case-a"),
                &descriptor(&data),
                data.as_slice(),
                Limits {
                    database_bytes: PAGE_BYTES * 3,
                    ..Default::default()
                },
                &|| false
            ),
            Err(Error::Database(_))
        ));
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(&data), &|| false),
            Err(Error::Unavailable)
        ));
        build(dir.path(), &data).unwrap();
        let reader = open(dir.path(), &owner("case-a"), &descriptor(&data), &|| false).unwrap();
        let keys = [json!(1)];
        assert!(matches!(
            reader.lookup_many(&vec![keys.as_slice(); 64], "value", &|| false),
            Err(Error::Limit(_))
        ));
    }

    #[test]
    fn sqlite_payload_corruption_and_foreign_manifest_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"key\":1,\"value\":\"one\"}\n";
        let prepared = build(dir.path(), data).unwrap();
        let path = directory(dir.path(), &prepared.version).unwrap();
        let mut broken: Value =
            serde_json::from_slice(&manifest_bytes(&prepared).unwrap()).unwrap();
        broken["prepared"]["rowCount"] = json!(0);
        fs::write(path.join("ready.json"), json(&broken).unwrap()).unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
            Err(Error::Corrupt)
        ));
        let mut foreign = prepared.clone();
        foreign.version.owner = owner("case-b");
        fs::write(path.join("ready.json"), manifest_bytes(&foreign).unwrap()).unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
            Err(Error::Corrupt)
        ));
        fs::write(path.join("ready.json"), manifest_bytes(&prepared).unwrap()).unwrap();
        let mut bytes = fs::read(path.join("rows.sqlite")).unwrap();
        let index = bytes.windows(3).position(|bytes| bytes == b"one").unwrap();
        bytes[index] = b't';
        fs::write(path.join("rows.sqlite"), bytes).unwrap();
        assert!(matches!(
            open(dir.path(), &owner("case-a"), &descriptor(data), &|| false),
            Err(Error::Corrupt)
        ));
    }

    #[test]
    fn reopens_exact_version_in_a_fresh_process() {
        let data = b"{\"key\":1,\"value\":\"restart\"}\r\n";
        if let Some(path) = std::env::var_os("LOGINSIGHT_REFERENCE_TEST_ROOT") {
            let reader = open(
                Path::new(&path),
                &owner("case-a"),
                &descriptor(data),
                &|| false,
            )
            .unwrap();
            assert_eq!(
                reader.lookup(&[json!(1)], "value").unwrap(),
                Some(json!("restart"))
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        build(dir.path(), data).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "reference_store::tests::reopens_exact_version_in_a_fresh_process",
            ])
            .env("LOGINSIGHT_REFERENCE_TEST_ROOT", dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
