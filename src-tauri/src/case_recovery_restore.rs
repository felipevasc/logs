//! Restores only verified generations. Assets publish append-only; existing
//! conflicting paths require restoration into a separate empty profile.
use super::*;
use rusqlite::{OptionalExtension, TransactionBehavior};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreReceipt {
    pub schema_version: u32,
    pub request_id: String,
    pub recovery_id: String,
    pub manifest_sha256: String,
    pub previous_recovery_id: Option<String>,
    pub after: AuthoritySnapshot,
    pub committed: bool,
    pub reconcile_required: bool,
    pub observed_unavailable_references: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    receipt: RestoreReceipt,
    before: Option<AuthoritySnapshot>,
}
const MARKER: &str = "case-recovery-last-restore";
const PENDING: &str = "restore-pending.json";
const CONFLICT:&str="CASE_RECOVERY_ASSET_CONFLICT: Existe um ativo diferente no destino; restaure em um perfil separado para preservar ambas as versões.";

fn marker(conn: &Connection) -> Result<Option<RestoreReceipt>, String> {
    if !table_exists(conn, "metadata")? {
        return Ok(None);
    }
    let text: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key=?1", [MARKER], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    text.map(|text| {
        if text.len() > MANIFEST_LIMIT {
            return Err(INVALID.into());
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    })
    .transpose()
}
fn durable_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or(INVALID)?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut pending = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut writer = Limited {
            writer: pending.as_file_mut(),
            bytes: 0,
        };
        serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    pending.as_file().sync_all().map_err(|e| e.to_string())?;
    pending
        .persist_noclobber(path)
        .map_err(|e| e.error.to_string())?;
    sync_directory(parent)
}
fn finish_intent(root: &Path, intent: &Intent) -> Result<(), String> {
    let directory = root.join("case-recovery-v1");
    let receipts = directory.join("restore-receipts");
    std::fs::create_dir_all(&receipts).map_err(|e| e.to_string())?;
    let path = receipts.join(format!(
        "{}-{}.json",
        intent.receipt.request_id,
        uuid::Uuid::new_v4()
    ));
    durable_json(&path, intent)?;
    std::fs::remove_file(directory.join(PENDING)).map_err(|e| e.to_string())?;
    sync_directory(&directory)
}
fn recover_pending(
    root: &Path,
    conn: &Connection,
    work: &Work<'_>,
) -> Result<Option<RestoreReceipt>, String> {
    let path = root.join("case-recovery-v1").join(PENDING);
    if !path.exists() {
        return Ok(None);
    }
    let intent: Intent = serde_json::from_slice(&bounded_read(&path, MANIFEST_LIMIT, work)?)
        .map_err(|e| e.to_string())?;
    if uuid::Uuid::parse_str(&intent.receipt.request_id).is_err() {
        return Err(INVALID.into());
    }
    let current = marker(conn)?;
    let committed = current.as_ref().is_some_and(|value| {
        value.request_id == intent.receipt.request_id
            && value.manifest_sha256 == intent.receipt.manifest_sha256
    });
    if committed {
        let mut receipt = current.unwrap();
        receipt.committed = true;
        receipt.reconcile_required =
            crate::case_evidence::snapshot_authority(conn)? != receipt.after;
        receipt.reconcile_required |= finish_intent(root, &intent).is_err();
        return Ok(Some(receipt));
    }
    let authority = crate::case_evidence::snapshot_authority(conn)?;
    if intent.before.as_ref() != Some(&authority) {
        return Err("CASE_RECOVERY_RECONCILE: Há uma restauração interrompida e o destino mudou; preserve o perfil e abra uma recuperação separada.".into());
    }
    // No database commit occurred. New immutable assets may remain unreferenced;
    // retain them rather than deleting anything a later version might use.
    let mut aborted = intent;
    aborted.receipt.committed = false;
    finish_intent(root, &aborted)?;
    Ok(None)
}

fn legacy_guard(conn: &Connection) -> Result<(), String> {
    if !table_exists(conn, "cases")? {
        return Ok(());
    }
    conn.execute_batch(r#"
CREATE TABLE IF NOT EXISTS native_evidence_permits(case_id TEXT PRIMARY KEY,nonce TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS case_recovery_protected(case_id TEXT PRIMARY KEY);
INSERT OR IGNORE INTO case_recovery_protected SELECT id FROM cases;
CREATE TRIGGER IF NOT EXISTS case_recovery_cases_insert BEFORE INSERT ON cases
WHEN EXISTS(SELECT 1 FROM case_recovery_protected p WHERE p.case_id=NEW.id)
 AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.id)
 AND NOT EXISTS(SELECT 1 FROM cases c WHERE c.id=NEW.id AND c.body IS NEW.body AND c.position IS NEW.position)
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS case_recovery_cases_update BEFORE UPDATE ON cases
WHEN (NEW.id IS NOT OLD.id OR NEW.body IS NOT OLD.body OR NEW.position IS NOT OLD.position)
 AND ((EXISTS(SELECT 1 FROM case_recovery_protected p WHERE p.case_id=OLD.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.id))
 OR (EXISTS(SELECT 1 FROM case_recovery_protected p WHERE p.case_id=NEW.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.id)))
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS case_recovery_cases_delete BEFORE DELETE ON cases
WHEN EXISTS(SELECT 1 FROM case_recovery_protected p WHERE p.case_id=OLD.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
"#).map_err(|e|e.to_string())
}
fn writable_database(path: &Path, create: bool) -> Result<Connection, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => {}
        Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err(INVALID.into()),
        Err(e) => return Err(e.to_string()),
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(100))
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA synchronous=FULL; PRAGMA cache_size=-1024; PRAGMA locking_mode=EXCLUSIVE;",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}
fn restored_backup(
    source: &Connection,
    destination: &mut Connection,
    work: &Work<'_>,
) -> Result<(), String> {
    let backup = Backup::new(source, destination).map_err(|e| e.to_string())?;
    loop {
        work.check()?;
        match backup.step(64).map_err(|e| e.to_string())? {
            StepResult::Done => return Ok(()), // committed: never report late cancel as rollback
            StepResult::More => {
                let p = backup.progress();
                (work.progress)(
                    "restore",
                    (p.pagecount - p.remaining).max(0) as u64,
                    Some(p.pagecount.max(0) as u64),
                );
            }
            StepResult::Busy | StepResult::Locked => {
                return Err(
                    "CASE_RECOVERY_BUSY: Feche outras operações ou instâncias antes de restaurar."
                        .into(),
                )
            }
            _ => return Err(INVALID.into()),
        }
    }
}
fn publish_assets(root: &Path, recovery: &VerifiedRecovery, work: &Work<'_>) -> Result<(), String> {
    // An observed-absent version must remain absent after restore. Retaining a
    // later prepared triple at the same owner/version would silently change it.
    for reference in &recovery.manifest.unavailable_references {
        let path = crate::reference_store::recovery_directory(
            root,
            &reference.owner,
            &reference.descriptor,
        )
        .map_err(|e| e.to_string())?;
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(CONFLICT.into()),
            Err(error) => return Err(error.to_string()),
        }
    }
    // Check every conflict before publishing the first missing path.
    let mut missing = Vec::new();
    for (index, asset) in recovery.manifest.assets.iter().enumerate() {
        work.check()?;
        let target = root.join(asset_path(&asset.relative_path)?);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => verify_asset(root, asset, work).map_err(|_| CONFLICT.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(index),
            Err(e) => return Err(e.to_string()),
        }
    }
    let stage = tempfile::Builder::new()
        .prefix("restore-assets-")
        .tempdir_in(root.join("case-recovery-v1"))
        .map_err(|e| e.to_string())?;
    for index in missing {
        let asset = &recovery.manifest.assets[index];
        copy_asset(
            &recovery.directory.join("assets"),
            stage.path(),
            asset,
            work,
        )?;
        let source = stage.path().join(&asset.relative_path);
        let target = root.join(&asset.relative_path);
        std::fs::create_dir_all(target.parent().ok_or(INVALID)?).map_err(|e| e.to_string())?;
        // Same-volume create-without-replacement. A crash cannot publish a
        // truncated payload; an unexpected existing path is never overwritten.
        match std::fs::hard_link(&source, &target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_asset(root, asset, work).map_err(|_| CONFLICT.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        }
        sync_directory(target.parent().ok_or(INVALID)?)?;
    }
    Ok(())
}

/// The caller must invalidate in-memory publications before releasing this
/// callback: EXCLUSIVE SQLite mode remains held on the same connection.
pub(crate) fn restore_current(
    root: &Path,
    recovery: &VerifiedRecovery,
    expected: &AuthoritySnapshot,
    request_id: &str,
    work: &Work<'_>,
    after_commit: impl FnOnce(),
) -> Result<RestoreReceipt, String> {
    uuid::Uuid::parse_str(request_id).map_err(|_| INVALID)?;
    recovery.validate_with(work)?;
    {
        let lease = RootLease::shared(root, work)?;
        let conn = readonly(&lease.root.join("investigations.sqlite3"))?;
        if let Some(mut receipt) = marker(&conn)? {
            if receipt.request_id == request_id {
                if receipt.manifest_sha256 != recovery.manifest_sha256 {
                    return Err(INVALID.into());
                }
                receipt.reconcile_required = crate::case_evidence::snapshot_authority(&conn)?
                    != receipt.after
                    || lease.root.join("case-recovery-v1").join(PENDING).exists();
                crate::operations::commit();
                return Ok(receipt);
            }
        }
    }
    let prior = prepare(root, work)?;
    if prior.authority() != expected {
        return Err("CASE_RECOVERY_STALE: A investigação mudou antes da restauração.".into());
    }
    let root_lease = RootLease::exclusive(root, work)?;
    let root = &root_lease.root;
    let mut destination = writable_database(&root.join("investigations.sqlite3"), false)?;
    destination
        .execute_batch("BEGIN EXCLUSIVE")
        .map_err(|e| e.to_string())?;
    let current = crate::case_evidence::snapshot_authority(&destination)?;
    if &current != expected {
        return Err("CASE_RECOVERY_STALE: A investigação mudou antes da publicação.".into());
    }
    // In EXCLUSIVE locking mode COMMIT retains the native file lock. Backup
    // requires no active destination transaction, so keep this connection open.
    destination
        .execute_batch("COMMIT")
        .map_err(|e| e.to_string())?;
    if let Some(receipt) = recover_pending(root, &destination, work)? {
        if receipt.request_id == request_id {
            return Ok(receipt);
        }
    }
    if let Some(receipt) = marker(&destination)? {
        if receipt.request_id == request_id {
            return if receipt.manifest_sha256 == recovery.manifest_sha256 {
                Ok(receipt)
            } else {
                Err(INVALID.into())
            };
        }
    }
    let stage = tempfile::Builder::new()
        .prefix("restore-db-")
        .tempdir_in(root.join("case-recovery-v1"))
        .map_err(|e| e.to_string())?;
    copy_asset(
        &recovery.directory,
        stage.path(),
        &recovery.manifest.database,
        work,
    )?;
    let working = stage.path().join("investigations.sqlite3");
    let mut source = Connection::open(&working).map_err(|e| e.to_string())?;
    source
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    assert_no_permits(&source)?;
    legacy_guard(&source)?;
    let tx = source
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if !recovery.has_interpretation_assets() {
        // An old recovery did not capture the source profile's interpretation.
        // Preserve that fact in the restored database before publishing it, so
        // later adoption cannot inherit unrelated destination configuration.
        tx.execute("INSERT OR IGNORE INTO metadata(key,value) VALUES('case-interpretation-origin-unavailable','1')", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM metadata WHERE key='case-interpretation-v1'", [])
            .map_err(|e| e.to_string())?;
    }
    // A newer recovery of an already-unknown lineage keeps its copied marker.
    // Full embedded Case settings remain authoritative; only missing settings
    // need the diagnostic, and raw view/export remains available.
    let next = expected
        .body_revision
        .parse::<u64>()
        .map_err(|_| INVALID)?
        .max(
            recovery
                .authority()
                .body_revision
                .parse::<u64>()
                .map_err(|_| INVALID)?,
        )
        .checked_add(1)
        .ok_or(LIMIT)?
        .to_string();
    tx.execute("INSERT INTO metadata(key,value)VALUES('revision',?1)ON CONFLICT(key)DO UPDATE SET value=excluded.value",[next]).map_err(|e|e.to_string())?;
    crate::case_evidence::restore_epoch(&tx, &uuid::Uuid::new_v4().to_string())?;
    let receipt = RestoreReceipt {
        schema_version: FORMAT,
        request_id: request_id.into(),
        recovery_id: recovery.manifest.id.clone(),
        manifest_sha256: recovery.manifest_sha256.clone(),
        previous_recovery_id: Some(prior.manifest.id.clone()),
        after: crate::case_evidence::snapshot_authority(&tx)?,
        committed: true,
        reconcile_required: false,
        observed_unavailable_references: recovery.manifest.unavailable_references.len(),
    };
    tx.execute("INSERT INTO metadata(key,value)VALUES(?1,?2)ON CONFLICT(key)DO UPDATE SET value=excluded.value",rusqlite::params![MARKER,serde_json::to_string(&receipt).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    integrity_check(&source, work)?;
    publish_assets(root, recovery, work)?;
    root_lease.validate()?;
    work.check()?;
    let intent = Intent {
        receipt: receipt.clone(),
        before: Some(current),
    };
    durable_json(&root.join("case-recovery-v1").join(PENDING), &intent)?;
    fault_point("before_backup");
    restored_backup(&source, &mut destination, work)?;
    fault_point("after_done");
    // From this point every outcome acknowledges the committed database image.
    crate::operations::commit();
    let mut result = receipt;
    let callback_failed =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(after_commit)).is_err();
    result.reconcile_required = callback_failed
        || marker(&destination)
            .ok()
            .flatten()
            .is_none_or(|v| v.request_id != request_id)
        || finish_intent(root, &intent).is_err();
    Ok(result)
}

/// Recovery of a damaged/conflicting profile stays separate and preserves both
/// copies. The destination must be empty and remains app-owned after creation.
pub(crate) fn restore_fresh(
    destination: &Path,
    recovery: &VerifiedRecovery,
    request_id: &str,
    work: &Work<'_>,
) -> Result<RestoreReceipt, String> {
    recovery.validate_with(work)?;
    if !destination.exists() {
        std::fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    }
    let _directory = open_directory(destination)?;
    if std::fs::read_dir(destination)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("CASE_RECOVERY_DESTINATION: Escolha um diretório vazio; nenhum arquivo existente foi alterado.".into());
    }
    let conn = writable_database(&destination.join("investigations.sqlite3"), true)?;
    conn.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO metadata VALUES('revision','0'); CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);").map_err(|e|e.to_string())?;
    let before = crate::case_evidence::snapshot_authority(&conn)?;
    drop(conn);
    restore_current(destination, recovery, &before, request_id, work, || {})
}

/// Called before ordinary profile admission when a restore journal exists.
pub(crate) fn reconcile_restore(
    root: &Path,
    work: &Work<'_>,
    after_commit: impl FnOnce(),
) -> Result<Option<RestoreReceipt>, String> {
    if !root.join("case-recovery-v1").join(PENDING).exists() {
        return Ok(None);
    }
    let lease = RootLease::exclusive(root, work)?;
    let conn = writable_database(&lease.root.join("investigations.sqlite3"), false)?;
    conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;")
        .map_err(|e| e.to_string())?;
    let mut result = recover_pending(&lease.root, &conn, work)?;
    if let Some(receipt) = &mut result {
        crate::operations::commit();
        receipt.reconcile_required |=
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(after_commit)).is_err();
    }
    Ok(result)
}

#[cfg(test)]
fn fault_point(point: &str) {
    if std::env::var("CASE_RESTORE_EXIT_POINT").ok().as_deref() == Some(point) {
        std::process::exit(73);
    }
}
#[cfg(not(test))]
fn fault_point(_: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    fn work() -> Work<'static> {
        Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        }
    }
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO metadata VALUES('revision','1');CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE padding(body BLOB);INSERT INTO padding VALUES(zeroblob(1048576));").unwrap();
        conn.execute("INSERT INTO cases VALUES('c',?1,0)",[r#"{"id":"c","items":[{"id":"i","rows":[{"id":1,"fields":{"wide":18446744073709551615,"decimal":1.000,"negative":-0.0},"future":{"keep":true}}]}]}"#]).unwrap();
        drop(conn);
        root
    }
    fn change(root: &Path) -> AuthoritySnapshot {
        let conn = Connection::open(root.join("investigations.sqlite3")).unwrap();
        conn.execute_batch("UPDATE cases SET body='{\"id\":\"c\",\"notes\":\"new\"}'; UPDATE metadata SET value='2' WHERE key='revision';").unwrap();
        crate::case_evidence::snapshot_authority(&conn).unwrap()
    }
    fn body(root: &Path) -> String {
        readonly(&root.join("investigations.sqlite3"))
            .unwrap()
            .query_row("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn restore_keeps_exact_envelopes_and_blocks_known_legacy_writer_after_unlock() {
        let root = fixture();
        let original = body(root.path());
        let copy = prepare(root.path(), &work()).unwrap();
        let expected = change(root.path());
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = restore_current(root.path(), &copy, &expected, &id, &work(), || {}).unwrap();
        assert!(receipt.committed);
        assert!(!receipt.reconcile_required);
        assert_eq!(receipt.after.body_revision, "3");
        assert_eq!(body(root.path()), original);
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        assert!(conn.execute("UPDATE cases SET body='{}'", []).is_err());
        assert!(conn.execute("DELETE FROM cases", []).is_err());
        conn.execute("UPDATE cases SET body=body,position=position", [])
            .unwrap();
        drop(conn);
        let replay = restore_current(root.path(), &copy, &expected, &id, &work(), || {
            panic!("replay must not rerun the mutation")
        })
        .unwrap();
        assert_eq!(replay.request_id, id);
        assert_eq!(replay.after.body_revision, "3");
    }
    #[test]
    fn old_recovery_origin_marker_survives_restore_and_new_recovery_lineage() {
        let source = fixture();
        let original = body(source.path());
        let conn = Connection::open(source.path().join("investigations.sqlite3")).unwrap();
        conn.execute("INSERT INTO metadata VALUES('case-interpretation-v1','1')", []).unwrap();
        drop(conn);
        let mut old = prepare(source.path(), &work()).unwrap();
        // Recreate the wire shape of an existing pre-0.12 recovery generation.
        old.manifest.interpretation_assets_version = None;
        let bytes = serde_json::to_vec(&old.manifest).unwrap();
        std::fs::write(old.directory.join("manifest.json"), &bytes).unwrap();
        old.manifest_sha256 = digest(&bytes);
        old.validate().unwrap();
        let first = tempfile::tempdir().unwrap();
        restore_fresh(first.path(), &old, &uuid::Uuid::new_v4().to_string(), &work()).unwrap();
        assert_eq!(body(first.path()), original);
        let conn = readonly(&first.path().join("investigations.sqlite3")).unwrap();
        assert!(crate::case_interpretation::origin_unavailable(&conn).unwrap());
        assert!(!conn.query_row::<bool, _, _>("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='case-interpretation-v1')", [], |row| row.get(0)).unwrap());
        drop(conn);
        let refreshed = prepare(first.path(), &work()).unwrap();
        assert!(refreshed.has_interpretation_assets());
        let second = tempfile::tempdir().unwrap();
        restore_fresh(second.path(), &refreshed, &uuid::Uuid::new_v4().to_string(), &work()).unwrap();
        assert_eq!(body(second.path()), original);
        let conn = readonly(&second.path().join("investigations.sqlite3")).unwrap();
        assert!(crate::case_interpretation::origin_unavailable(&conn).unwrap(), "new proof must not erase copied unknown lineage");
        drop(conn);
        let known = prepare(source.path(), &work()).unwrap();
        let third = tempfile::tempdir().unwrap();
        restore_fresh(third.path(), &known, &uuid::Uuid::new_v4().to_string(), &work()).unwrap();
        let conn = readonly(&third.path().join("investigations.sqlite3")).unwrap();
        assert!(!crate::case_interpretation::origin_unavailable(&conn).unwrap());
        assert!(conn.query_row::<bool, _, _>("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='case-interpretation-v1')", [], |row| row.get(0)).unwrap());
    }
    #[test]
    fn cancellation_before_done_rolls_back_and_can_reconcile_then_retry() {
        let root = fixture();
        let copy = prepare(root.path(), &work()).unwrap();
        let expected = change(root.path());
        let prior = body(root.path());
        let cancelled = AtomicBool::new(false);
        let id = uuid::Uuid::new_v4().to_string();
        let work = Work {
            cancelled: &|| cancelled.load(Ordering::Relaxed),
            progress: &|phase, _, _| {
                if phase == "restore" {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        };
        assert!(
            restore_current(root.path(), &copy, &expected, &id, &work, || panic!(
                "not committed"
            ))
            .is_err()
        );
        assert_eq!(body(root.path()), prior);
        cancelled.store(false, Ordering::Relaxed);
        assert!(
            reconcile_restore(root.path(), &self::work(), || panic!("not committed"))
                .unwrap()
                .is_none()
        );
        let receipt =
            restore_current(root.path(), &copy, &expected, &id, &self::work(), || {}).unwrap();
        assert!(receipt.committed);
    }
    #[test]
    fn late_named_cancellation_acknowledges_commit_and_keeps_destination_exclusive() {
        let root = fixture();
        let copy = prepare(root.path(), &work()).unwrap();
        let expected = change(root.path());
        let request = uuid::Uuid::new_v4().to_string();
        let operation = uuid::Uuid::new_v4().to_string();
        let result = crate::operations::run_with_token(
            crate::operations::token(Some(operation.clone())).unwrap(),
            || {
                restore_current(
                    root.path(),
                    &copy,
                    &expected,
                    &request,
                    &Work {
                        cancelled: &crate::operations::cancelled,
                        progress: &|_, _, _| {},
                    },
                    || {
                        let output = std::process::Command::new(std::env::current_exe().unwrap())
                            .args([
                                "--exact",
                                "case_recovery::tests::sqlite_writer_lock_child",
                                "--test-threads=1",
                            ])
                            .env(
                                "CASE_RECOVERY_LOCK_CHILD",
                                root.path().join("investigations.sqlite3"),
                            )
                            .output()
                            .unwrap();
                        assert!(output.status.success());
                        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
                        crate::operations::cancel_id(&operation);
                    },
                )
            },
        )
        .unwrap()
        .unwrap();
        assert!(result.committed);
        assert!(!result.reconcile_required);
    }
    #[test]
    fn restored_legacy_profile_can_be_adopted_only_inside_shared_permit_transaction() {
        let source = fixture();
        let copy = prepare(source.path(), &work()).unwrap();
        let destination = tempfile::tempdir().unwrap();
        restore_fresh(
            destination.path(),
            &copy,
            &uuid::Uuid::new_v4().to_string(),
            &work(),
        )
        .unwrap();
        let mut conn = Connection::open(destination.path().join("investigations.sqlite3")).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "INSERT INTO native_evidence_permits VALUES('c','owned')",
            [],
        )
        .unwrap();
        tx.execute("DELETE FROM cases WHERE id='c'", []).unwrap();
        tx.execute("DELETE FROM native_evidence_permits WHERE case_id='c'", [])
            .unwrap();
        tx.commit().unwrap();
        assert!(
            conn.execute("INSERT INTO cases VALUES('c','{}',0)", [])
                .is_err(),
            "protected tombstone survives deletion"
        );
    }
    #[test]
    fn committed_reconciliation_reports_later_authority_changes() {
        let root = fixture();
        let copy = prepare(root.path(), &work()).unwrap();
        change(root.path());
        let request = uuid::Uuid::new_v4().to_string();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "case_recovery::restore::tests::restore_process_child",
                "--test-threads=1",
            ])
            .env("CASE_RESTORE_CHILD_ROOT", root.path())
            .env("CASE_RESTORE_CHILD_COPY", copy.manifest.id.clone())
            .env("CASE_RESTORE_CHILD_REQUEST", request)
            .env("CASE_RESTORE_EXIT_POINT", "after_done")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(73));
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute("UPDATE metadata SET value='4' WHERE key='revision'", [])
            .unwrap();
        drop(conn);
        let receipt = reconcile_restore(root.path(), &work(), || {})
            .unwrap()
            .unwrap();
        assert!(receipt.committed);
        assert!(receipt.reconcile_required);
        assert_eq!(receipt.after.body_revision, "3");
    }
    #[test]
    fn restore_process_child() {
        let Some(root) = std::env::var_os("CASE_RESTORE_CHILD_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let copy = load_verified(
            &root,
            &std::env::var("CASE_RESTORE_CHILD_COPY").unwrap(),
            &work(),
        )
        .unwrap();
        let expected = crate::case_evidence::snapshot_authority(
            &readonly(&root.join("investigations.sqlite3")).unwrap(),
        )
        .unwrap();
        let _ = restore_current(
            &root,
            &copy,
            &expected,
            &std::env::var("CASE_RESTORE_CHILD_REQUEST").unwrap(),
            &work(),
            || {},
        );
        panic!("expected process exit");
    }
    #[test]
    fn process_exit_before_and_after_done_reconciles_the_actual_commit() {
        for (point, committed) in [("before_backup", false), ("after_done", true)] {
            let root = fixture();
            let original = body(root.path());
            let copy = prepare(root.path(), &work()).unwrap();
            change(root.path());
            let changed = body(root.path());
            let request = uuid::Uuid::new_v4().to_string();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "case_recovery::restore::tests::restore_process_child",
                    "--test-threads=1",
                ])
                .env("CASE_RESTORE_CHILD_ROOT", root.path())
                .env("CASE_RESTORE_CHILD_COPY", copy.manifest.id.clone())
                .env("CASE_RESTORE_CHILD_REQUEST", request.clone())
                .env("CASE_RESTORE_EXIT_POINT", point)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(73),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let receipt = reconcile_restore(root.path(), &work(), || {}).unwrap();
            assert_eq!(receipt.is_some(), committed);
            assert_eq!(
                body(root.path()),
                if committed { original } else { changed }
            );
            assert!(!root.path().join("case-recovery-v1").join(PENDING).exists());
        }
    }
}
