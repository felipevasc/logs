//! Lossless first-run bridge. Legacy JSON bytes are copied and verified before
//! raw Case spans seed SQLite; no record enters Value or Event during seeding.
use super::*;
use crate::case_work_budget::Lease;
use fs2::FileExt;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
};
const INPUT_BYTES: u64 = 64 << 20;
const BUFFER: usize = 256 << 10;
const DIRECTORY: &str = "case-bootstrap-v1";
const KEY: &str = "native-bootstrap-inputs-v1";
const INVALID: &str =
    "CASE_BOOTSTRAP_INVALID: Os arquivos legados foram preservados; a inicialização foi recusada.";
static SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    name: String,
    bytes: u64,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    files: Vec<Input>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    generation: String,
    manifest: Manifest,
    selected: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapReceipt {
    pub initialized: bool,
    pub cases: u32,
    pub selected_legacy_source: Option<String>,
    pub input_generation: Option<String>,
}
struct Generation {
    proof: Proof,
    path: PathBuf,
    sources: Vec<(
        Input,
        File,
        Option<(u64, u64)>,
        Option<std::time::SystemTime>,
    )>,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    Ok(())
}
fn lock(root: &Path) -> Result<File, String> {
    let path = root.join("case-bootstrap-v1.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(&path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file()
        || fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err(INVALID.into());
    }
    FileExt::try_lock_exclusive(&file).map_err(|_| {
        "CASE_BOOTSTRAP_BUSY: Outra sessão está inicializando a investigação.".to_string()
    })?;
    Ok(file)
}
fn reader_hash(
    file: &mut File,
    expected: u64,
    mut write: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<String, String> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0; BUFFER];
    loop {
        crate::operations::check()?;
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .filter(|n| *n <= expected && *n <= INPUT_BYTES)
            .ok_or(INVALID)?;
        write(&buffer[..n])?;
        hash.update(&buffer[..n]);
    }
    if total != expected {
        return Err(INVALID.into());
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify_file(path: &Path, input: &Input) -> Result<(), String> {
    let mut file = crate::case_archive_format::open_regular(path)?;
    if file.metadata().map_err(|e| e.to_string())?.len() != input.bytes
        || reader_hash(&mut file, input.bytes, |_| Ok(()))? != input.sha256
    {
        return Err(INVALID.into());
    }
    Ok(())
}
fn verify_generation(path: &Path, proof: &Proof) -> Result<(), String> {
    validate_proof(proof)?;
    let encoded = serde_json::to_vec(&proof.manifest).map_err(|e| e.to_string())?;
    verify_file(
        &path.join("manifest.json"),
        &Input {
            name: "manifest.json".into(),
            bytes: encoded.len() as u64,
            sha256: proof.generation.clone(),
        },
    )?;
    for file in &proof.manifest.files {
        verify_file(&path.join(&file.name), file)?;
    }
    Ok(())
}
fn validate_proof(proof: &Proof) -> Result<(), String> {
    if proof.manifest.schema_version != 1
        || proof.manifest.files.len() > 2
        || !valid_hash(&proof.generation)
    {
        return Err(INVALID.into());
    }
    let mut names = HashSet::new();
    for file in &proof.manifest.files {
        if !matches!(file.name.as_str(), "cases.json" | "cases.backup.json")
            || !names.insert(file.name.as_str())
            || file.bytes > INPUT_BYTES
            || !valid_hash(&file.sha256)
        {
            return Err(INVALID.into());
        }
    }
    if proof
        .selected
        .as_ref()
        .is_some_and(|name| !names.contains(name.as_str()))
    {
        return Err(INVALID.into());
    }
    if hash(&serde_json::to_vec(&proof.manifest).map_err(|e| e.to_string())?) != proof.generation {
        return Err(INVALID.into());
    }
    Ok(())
}
fn copies(root: &Path) -> Result<Generation, String> {
    let _scratch =
        crate::case_cache::reserve_work(crate::case_work_budget::global(), BUFFER * 3 + 64 * 1024)?;
    let base = root.join(DIRECTORY);
    directory(&base)?;
    #[cfg(unix)]
    {
        File::open(root)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let pending = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(&base)
        .map_err(|e| e.to_string())?;
    let mut sources = Vec::new();
    let mut inputs = Vec::new();
    for name in ["cases.json", "cases.backup.json"] {
        let source = root.join(name);
        match fs::symlink_metadata(&source) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
            Ok(_) => (),
        }
        let mut file = crate::case_archive_format::open_regular(&source)?;
        FileExt::try_lock_shared(&file).map_err(|_| "CASE_BOOTSTRAP_BUSY")?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if meta.len() > INPUT_BYTES {
            return Err("CASE_BOOTSTRAP_INPUT_LIMIT".into());
        }
        let identity = crate::sources::file_identity(&file);
        let modified = meta.modified().ok();
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(pending.path().join(name))
            .map_err(|e| e.to_string())?;
        let sha256 = reader_hash(&mut file, meta.len(), |bytes| {
            output.write_all(bytes).map_err(|e| e.to_string())
        })?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        let input = Input {
            name: name.into(),
            bytes: meta.len(),
            sha256,
        };
        verify_file(&pending.path().join(name), &input)?;
        inputs.push(input.clone());
        sources.push((input, file, identity, modified));
    }
    let manifest = Manifest {
        schema_version: 1,
        files: inputs,
    };
    let encoded = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let generation = hash(&encoded);
    let proof = Proof {
        generation: generation.clone(),
        manifest,
        selected: None,
    };
    let mut ready = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pending.path().join("manifest.json"))
        .map_err(|e| e.to_string())?;
    ready
        .write_all(&encoded)
        .and_then(|_| ready.sync_all())
        .map_err(|e| e.to_string())?;
    drop(ready);
    #[cfg(unix)]
    {
        File::open(pending.path())
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let path = base.join(&generation);
    if fs::symlink_metadata(&path).is_ok() {
        verify_generation(&path, &proof)?;
    } else {
        fs::rename(pending.path(), &path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            File::open(&base)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
    }
    verify_generation(&path, &proof)?;
    let value = Generation {
        proof,
        path,
        sources,
    };
    value.validate_sources(root)?;
    Ok(value)
}
impl Generation {
    fn validate_sources(&self, root: &Path) -> Result<(), String> {
        for (input, file, identity, modified) in &self.sources {
            crate::operations::check()?;
            let current = crate::case_archive_format::open_regular(&root.join(&input.name))?;
            let meta = current.metadata().map_err(|e| e.to_string())?;
            if meta.len() != input.bytes
                || meta.modified().ok() != *modified
                || crate::sources::file_identity(&current) != *identity
                || crate::sources::file_identity(file) != *identity
            {
                return Err(INVALID.into());
            }
        }
        for name in ["cases.json", "cases.backup.json"] {
            if !self
                .sources
                .iter()
                .any(|(input, _, _, _)| input.name == name)
                && fs::symlink_metadata(root.join(name)).is_ok()
            {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
}
struct Text {
    text: String,
    _credit: Lease,
}
fn read_input(path: &Path, input: &Input) -> Result<Text, String> {
    let size = usize::try_from(input.bytes).map_err(|_| INVALID)?;
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        size.checked_add(BUFFER).ok_or(INVALID)?,
    )?;
    let mut file = crate::case_archive_format::open_regular(path)?;
    if file.metadata().map_err(|e| e.to_string())?.len() != input.bytes {
        return Err(INVALID.into());
    }
    let mut bytes = vec![0; size];
    for chunk in bytes.chunks_mut(BUFFER) {
        crate::operations::check()?;
        file.read_exact(chunk).map_err(|e| e.to_string())?;
    }
    let mut extra = [0];
    if file.read(&mut extra).map_err(|e| e.to_string())? != 0 || hash(&bytes) != input.sha256 {
        return Err(INVALID.into());
    }
    Ok(Text {
        text: String::from_utf8(bytes).map_err(|_| "CASE_BOOTSTRAP_UTF8")?,
        _credit: credit,
    })
}
fn member<'a>(raw: RawJson<'a>, wanted: &str) -> Result<Option<RawJson<'a>>, String> {
    let mut values = raw.members()?;
    let mut found = None;
    while let Some(field) = values.next()? {
        crate::operations::check()?;
        if field.key.len() <= 128 && field.key()? == wanted {
            if found.is_some() {
                return Err(INVALID.into());
            }
            found = Some(field.value);
        }
    }
    Ok(found)
}
struct Parsed<'a> {
    active: Option<String>,
    cases: Vec<(String, RawJson<'a>)>,
    _credit: Lease,
}
fn parse(raw: RawJson<'_>) -> Result<Parsed<'_>, String> {
    if raw.kind() != b'{' {
        return Err(INVALID.into());
    }
    if let Some(version) = member(raw, "schemaVersion")? {
        let version = version.get().parse::<u64>().map_err(|_| INVALID)?;
        if version > 2 {
            return Err("CASE_BOOTSTRAP_NEWER_FORMAT".into());
        }
    }
    let cases = member(raw, "cases")?
        .filter(|value| value.kind() == b'[')
        .ok_or(INVALID)?;
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 64 << 10)?;
    let active = match member(raw, "active")? {
        None => None,
        Some(value) if value.kind() == b'n' => None,
        Some(value) => {
            if value.get().len() > 32 << 10 {
                return Err(INVALID.into());
            }
            Some(serde_json::from_str::<String>(value.get()).map_err(|_| INVALID)?)
        }
    };
    if active.as_ref().is_some_and(|id| id.len() > 4096) {
        return Err(INVALID.into());
    }
    let mut elements = cases.elements()?;
    let mut output = Vec::new();
    let mut ids = HashSet::new();
    while let Some(case) = elements.next()? {
        crate::operations::check()?;
        if output.len() >= 1000 {
            return Err("CASE_BOOTSTRAP_CASE_LIMIT".into());
        }
        let id = member(case, "id")?.ok_or(INVALID)?;
        if id.get().len() > 32 << 10 {
            return Err(INVALID.into());
        }
        credit.merge(crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            id.get()
                .len()
                .checked_mul(3)
                .and_then(|n| n.checked_add(1024))
                .ok_or(INVALID)?,
        )?)?;
        let id: String = serde_json::from_str(id.get()).map_err(|_| INVALID)?;
        if id.is_empty() || id.len() > 4096 || !ids.insert(id.clone()) {
            return Err("CASE_BOOTSTRAP_CASE_ID".into());
        }
        output.push((id, case));
    }
    if active.as_ref().is_some_and(|id| !ids.contains(id)) {
        return Err("CASE_BOOTSTRAP_ACTIVE_ID".into());
    }
    Ok(Parsed {
        active,
        cases: output,
        _credit: credit,
    })
}
fn initialized(conn: &Connection) -> Result<Option<u32>, String> {
    if db::exists(conn, "metadata")? {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='revision')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            let cases = if db::exists(conn, "cases")? {
                conn.query_row::<u32, _, _>("SELECT count(*) FROM cases", [], |row| row.get(0))
                    .map_err(|e| e.to_string())?
            } else {
                0
            };
            return Ok(Some(cases));
        }
    }
    for table in ["cases", "case_analysis"] {
        if db::exists(conn, table)?
            && conn
                .query_row::<bool, _, _>(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?
        {
            return Err("CASE_BOOTSTRAP_EXISTING_DATA: O banco existente foi preservado; recupere-o antes de inicializar.".into());
        }
    }
    Ok(None)
}
fn existing(root: &Path) -> Result<Option<u32>, String> {
    match fs::symlink_metadata(root.join("investigations.sqlite3")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(_) => initialized(&authority::connect_readonly(root)?),
    }
}
/// Called before either the native view or the old context initialization path.
/// The known old load_at Value migration is never used for legacy row bodies.
pub(crate) fn bootstrap(root: &Path) -> Result<BootstrapReceipt, String> {
    // The seed's durable commit must not clear cancellation for a surrounding
    // load -> recovery -> adoption chain (or an early source admission).
    crate::operations::run_with_token(crate::operations::current_token(), || bootstrap_inner(root))?
}
fn bootstrap_inner(root: &Path) -> Result<BootstrapReceipt, String> {
    crate::operations::check()?;
    directory(root)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if let Some(cases) = existing(&root)? {
        return Ok(BootstrapReceipt {
            initialized: false,
            cases,
            selected_legacy_source: None,
            input_generation: None,
        });
    }
    let _serial = loop {
        crate::operations::check()?;
        if let Some(guard) = SERIAL.try_lock() {
            break guard;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let _lock = lock(&root)?;
    if let Some(cases) = existing(&root)? {
        return Ok(BootstrapReceipt {
            initialized: false,
            cases,
            selected_legacy_source: None,
            input_generation: None,
        });
    }
    let mut generation = copies(&root)?;
    let mut selected = None;
    for input in &generation.proof.manifest.files {
        let text = match read_input(&generation.path.join(&input.name), input) {
            Ok(value) => value,
            Err(error) if error == "CASE_BOOTSTRAP_UTF8" => continue,
            Err(error) => return Err(error),
        };
        if let Ok(raw) = checked_document(&text.text) {
            if raw.kind() == b'{' && member(raw, "cases")?.is_some_and(|value| value.kind() == b'[')
            {
                selected = Some((input.name.clone(), text));
                break;
            }
        } else {
            crate::operations::check()?;
        }
    }
    if selected.is_none() && !generation.proof.manifest.files.is_empty() {
        return Err(INVALID.into());
    }
    let empty = "{\"active\":null,\"cases\":[]}";
    let text = selected
        .as_ref()
        .map(|(_, text)| text.text.as_str())
        .unwrap_or(empty);
    let parsed = parse(checked_document(text)?)?;
    generation.proof.selected = selected.as_ref().map(|(name, _)| name.clone());
    let db_path = root.join("investigations.sqlite3");
    if let Ok(meta) = fs::symlink_metadata(&db_path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(INVALID.into());
        }
    }
    let mut conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if let Some(cases) = initialized(&tx)? {
        return Ok(BootstrapReceipt {
            initialized: false,
            cases,
            selected_legacy_source: None,
            input_generation: None,
        });
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE IF NOT EXISTS case_analysis(case_id TEXT PRIMARY KEY,body TEXT NOT NULL);").map_err(|e|e.to_string())?;
    for (position, (id, case)) in parsed.cases.iter().enumerate() {
        crate::operations::check()?;
        let mirror = body::stage_mirror(&root, |writer| {
            writer
                .write_all(case.get().as_bytes())
                .map_err(|e| e.to_string())
        })?;
        body::write_mirror(&tx, id, position as i64, &mirror)?;
    }
    tx.execute("INSERT INTO metadata(key,value) VALUES('revision','1')", [])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO metadata(key,value) VALUES('active',?1)",
        [serde_json::to_string(&parsed.active).map_err(|e| e.to_string())?],
    )
    .map_err(|e| e.to_string())?;
    let proof = serde_json::to_string(&generation.proof).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO metadata(key,value) VALUES(?1,?2)",
        params![KEY, proof],
    )
    .map_err(|e| e.to_string())?;
    generation.validate_sources(&root)?;
    root_guard.validate()?;
    crate::operations::check()?;
    tx.commit().map_err(|e| e.to_string())?;
    crate::operations::commit();
    Ok(BootstrapReceipt {
        initialized: true,
        cases: parsed.cases.len() as u32,
        selected_legacy_source: generation.proof.selected,
        input_generation: Some(generation.proof.generation),
    })
}
/// Required immutable input dependencies are part of later full recovery copies,
/// including the unused/invalid primary when a valid backup was selected.
pub(super) fn dependencies(conn: &Connection) -> Result<Vec<NativeDependency>, String> {
    if !db::exists(conn, "metadata")? {
        return Ok(Vec::new());
    }
    let text: Option<Option<String>> = conn
        .query_row(
            "SELECT CASE WHEN octet_length(value)<=8192 THEN value END FROM metadata WHERE key=?1",
            [KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let text = match text {
        None => return Ok(Vec::new()),
        Some(Some(value)) => value,
        Some(None) => return Err(INVALID.into()),
    };
    let proof: Proof = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    validate_proof(&proof)?;
    let mut output = Vec::new();
    for input in &proof.manifest.files {
        output.push(NativeDependency {
            kind: "legacy_input".into(),
            id: input.sha256.clone(),
            relative_path: format!("{DIRECTORY}/{}/{}", proof.generation, input.name),
            bytes: input.bytes,
            sha256: input.sha256.clone(),
        });
    }
    let bytes = serde_json::to_vec(&proof.manifest)
        .map_err(|e| e.to_string())?
        .len() as u64;
    output.push(NativeDependency {
        kind: "legacy_input_manifest".into(),
        id: proof.generation.clone(),
        relative_path: format!("{DIRECTORY}/{}/manifest.json", proof.generation),
        bytes,
        sha256: proof.generation,
    });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    const RAW: &str = r#"{ "id":"c", "name":"original", "items":[{"id":"i","rows":[{ "id":7,"future":{"wide":18446744073709551615,"decimal":1.000,"negative":-0.0,"tiny":2.547114365375239e-8},"$serde_json::private::RawValue":"1" }]}],"unrecognized":{"kept":true} }"#;
    fn input(root: &Path) {
        fs::write(
            root.join("cases.json"),
            format!("{{\"schemaVersion\":2,\"active\":\"c\",\"cases\":[{RAW}]}}"),
        )
        .unwrap();
    }
    fn body(root: &Path) -> String {
        let conn = authority::connect_readonly(root).unwrap();
        conn.query_row("SELECT body FROM cases WHERE id='c'", [], |row| row.get(0))
            .unwrap()
    }
    #[test]
    fn named_cancel_after_durable_seed_stops_outer_recovery_without_losing_seed() {
        let root = tempfile::tempdir().unwrap();
        input(root.path());
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            let seeded = bootstrap(root.path()).unwrap();
            assert!(seeded.initialized);
            assert!(crate::operations::cancel_id(&id));
            assert!(crate::operations::check().is_err());
            assert!(crate::case_recovery::prepare(root.path(), &prepare::root_work()).is_err());
        });
        assert!(result.is_err());
        assert_eq!(body(root.path()), RAW);
        assert!(!bootstrap(root.path()).unwrap().initialized);
        let conn = authority::connect_readonly(root.path()).unwrap();
        assert!(!db::exists(&conn, "native_evidence_store").unwrap());
    }
    #[test]
    fn legacy_entry_points_refuse_native_authority_before_body_or_new_case_writes() {
        let (root, request, _) = authority::tests::fixture();
        assert!(native_owner(root.path(), &request.analysis_context).unwrap());
        let mut stale = request.analysis_context.clone();
        stale.config_revision += 1;
        assert!(native_owner(root.path(), &stale).is_err());
        assert!(crate::case_store::load_at(root.path())
            .unwrap_err()
            .starts_with("CASE_NATIVE_EVIDENCE_REQUIRED"));
        let data =
            serde_json::json!({"schemaVersion":2,"revision":1,"cases":[{"id":"unprotected-new"}]});
        assert!(crate::case_store::save_at(root.path(), data)
            .unwrap_err()
            .starts_with("CASE_NATIVE_EVIDENCE_REQUIRED"));
        let conn = authority::connect_readonly(root.path()).unwrap();
        assert_eq!(
            conn.query_row::<u32, _, _>("SELECT count(*) FROM cases", [], |row| row.get(0))
                .unwrap(),
            1
        );
        assert_eq!(db::stamp(&conn).unwrap().revision, "1");
    }
    #[test]
    fn json_only_seed_keeps_exact_case_spans_and_both_verified_input_files() {
        let root = tempfile::tempdir().unwrap();
        input(root.path());
        let backup = b"opaque unused backup\xff";
        fs::write(root.path().join("cases.backup.json"), backup).unwrap();
        let primary = fs::read(root.path().join("cases.json")).unwrap();
        let result = bootstrap(root.path()).unwrap();
        assert!(result.initialized);
        assert_eq!(result.cases, 1);
        assert_eq!(result.selected_legacy_source.as_deref(), Some("cases.json"));
        assert_eq!(body(root.path()), RAW);
        let saved = root
            .path()
            .join(DIRECTORY)
            .join(result.input_generation.unwrap());
        assert_eq!(fs::read(saved.join("cases.json")).unwrap(), primary);
        assert_eq!(fs::read(saved.join("cases.backup.json")).unwrap(), backup);
        assert_eq!(fs::read(root.path().join("cases.json")).unwrap(), primary);
        let recovery = crate::case_recovery::prepare(root.path(), &prepare::root_work()).unwrap();
        recovery.validate().unwrap();
        let copied =
            authority::connect_readonly(recovery.snapshot_database_path().parent().unwrap())
                .unwrap();
        let mut deps = Vec::new();
        native_dependencies(&copied, |value| {
            deps.push(value);
            Ok(())
        })
        .unwrap();
        assert_eq!(deps.len(), 3);
        for dep in deps {
            assert_eq!(
                hash(&fs::read(recovery.snapshot_assets_root().join(dep.relative_path)).unwrap()),
                dep.sha256
            );
        }
        let adopted = adopt(root.path(), "bootstrap-adopt", &recovery).unwrap();
        assert_eq!(adopted.receipt.adopted.len(), 1);
    }
    #[test]
    fn empty_profile_and_existing_empty_database_initialize_without_path_replacement() {
        let root = tempfile::tempdir().unwrap();
        let first = bootstrap(root.path()).unwrap();
        assert!(first.initialized);
        assert_eq!(first.cases, 0);
        assert!(!bootstrap(root.path()).unwrap().initialized);
        let other = tempfile::tempdir().unwrap();
        input(other.path());
        let conn = crate::case_store::connect(other.path()).unwrap();
        bootstrap(other.path()).unwrap();
        assert_eq!(
            conn.query_row::<u32, _, _>("SELECT count(*) FROM cases", [], |row| row.get(0))
                .unwrap(),
            1
        );
        assert_eq!(body(other.path()), RAW);
    }
    #[test]
    fn duplicate_invalid_and_future_case_documents_never_seed_or_replace_originals() {
        for text in [
            r#"{"cases":[{"id":"x"},{"id":"x"}]}"#,
            r#"{"cases":[{"id":17}]}"#,
            r#"{"cases":[{"id":""}]}"#,
            r#"{"schemaVersion":3,"cases":[{"id":"x"}]}"#,
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("cases.json"), text).unwrap();
            assert!(bootstrap(root.path()).is_err());
            assert!(!root.path().join("investigations.sqlite3").exists());
            assert_eq!(
                fs::read_to_string(root.path().join("cases.json")).unwrap(),
                text
            );
        }
    }
    #[test]
    fn failed_sql_initialization_rolls_back_and_reuses_verified_inputs_on_retry() {
        let root = tempfile::tempdir().unwrap();
        input(root.path());
        let conn = crate::case_store::connect(root.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER fixture_seed_abort BEFORE INSERT ON cases BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(bootstrap(root.path()).is_err());
        assert_eq!(
            conn.query_row::<u32, _, _>("SELECT count(*) FROM cases", [], |row| row.get(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row::<u32, _, _>(
                "SELECT count(*) FROM metadata WHERE key='revision'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER fixture_seed_abort")
            .unwrap();
        assert!(bootstrap(root.path()).unwrap().initialized);
        assert_eq!(body(root.path()), RAW);
        let generations = fs::read_dir(root.path().join(DIRECTORY))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| valid_hash(&entry.file_name().to_string_lossy()))
            .count();
        assert_eq!(generations, 1);
    }
    #[test]
    fn concurrent_initializers_commit_one_exact_copy_and_populated_sqlite_wins() {
        let root = tempfile::tempdir().unwrap();
        input(root.path());
        let path = root.path().to_path_buf();
        let second = path.clone();
        let first = std::thread::spawn(move || bootstrap(&path).unwrap());
        let next = std::thread::spawn(move || bootstrap(&second).unwrap());
        let a = first.join().unwrap();
        let b = next.join().unwrap();
        assert_ne!(a.initialized, b.initialized);
        assert_eq!(body(root.path()), RAW);
        fs::write(
            root.path().join("cases.json"),
            "malformed newer external file",
        )
        .unwrap();
        assert!(!bootstrap(root.path()).unwrap().initialized);
        assert_eq!(body(root.path()), RAW);
    }
    #[test]
    fn invalid_primary_can_select_backup_while_preserving_both_and_early_context_keeps_rows() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("cases.json"), "broken").unwrap();
        fs::write(
            root.path().join("cases.backup.json"),
            format!("{{\"active\":\"c\",\"cases\":[{RAW}]}}"),
        )
        .unwrap();
        let result = bootstrap(root.path()).unwrap();
        assert_eq!(
            result.selected_legacy_source.as_deref(),
            Some("cases.backup.json")
        );
        assert_eq!(body(root.path()), RAW);
        let _context = crate::case_store::context_connection(root.path()).unwrap();
        assert_eq!(body(root.path()), RAW);
        let loaded = crate::case_store::load_at(root.path()).unwrap();
        assert_eq!(loaded["cases"][0]["id"], "c");
        assert_eq!(body(root.path()), RAW);
    }
    #[cfg(unix)]
    #[test]
    fn legacy_symlink_is_refused_before_copy_or_database_creation() {
        let root = tempfile::tempdir().unwrap();
        let foreign = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(foreign.path(), root.path().join("cases.json")).unwrap();
        assert!(bootstrap(root.path()).is_err());
        assert!(!root.path().join("investigations.sqlite3").exists());
    }
}
