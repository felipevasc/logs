//! Restore selection is a restart boundary. Running processes retain their
//! original data root; no request can redirect live caches to a restored DB.
use super::*;
use std::sync::OnceLock;

const DIRECTORY: &str = "case-profiles-v1";
const ACTIVE: &str = "active.json";
const PENDING: &str = "pending.json";
const RETURN_ORIGINAL: &str = "return-original.json";
const PROFILE_LIMIT: usize = 1 << 20;
const PROFILE_ERROR: &str = "CASE_PROFILE_INVALID: O perfil restaurado não pôde ser verificado; a seleção anterior foi preservada.";
static SELECTED: OnceLock<StartupProfile> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Active {
    version: u32,
    profile: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pending {
    version: u32,
    request_id: String,
    from: Active,
    profile: String,
    recovery_id: String,
    restore: RestoreReceipt,
    proof: RestoredProfileProof,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OriginalIntent {
    version: u32,
    request_id: String,
    from: Active,
    proof: RestoredProfileProof,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedOriginal {
    destination: PathBuf,
    restart_required: bool,
    original_preserved: bool,
    warning: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedProfile {
    pub profile_id: String,
    pub destination: PathBuf,
    pub restart_required: bool,
    pub original_preserved: bool,
    pub warning: Option<String>,
}
#[derive(Clone)]
pub(crate) struct StartupProfile {
    pub selected: PathBuf,
    pub warning: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileStatus {
    current_profile: ProfileLocation,
    restart_only: bool,
    startup_warning: Option<String>,
    pending: Option<PendingProfile>,
    recoveries: Vec<RecoverySummary>,
    scan_limited: bool,
    return_original_pending: bool,
}
#[derive(Serialize)]
struct ProfileLocation {
    id: Option<String>,
    path: PathBuf,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingProfile {
    profile_id: String,
    recovery_id: String,
    request_id: String,
    destination: PathBuf,
    restart_required: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverySummary {
    recovery_id: String,
    created_at_ms: Option<u64>,
    state: &'static str,
}

/// This is a bounded inventory, not proof that a recovery still verifies.
/// Preparing a restart always revalidates the complete immutable generation.
pub(crate) fn status(
    base: &Path,
    current: &Path,
    work: &Work<'_>,
) -> Result<ProfileStatus, String> {
    work.check()?;
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let current = current.canonicalize().map_err(|e| e.to_string())?;
    let _current = open_directory(&current)?;
    let id = if current == base {
        None
    } else {
        let relative = current
            .strip_prefix(base.join(DIRECTORY))
            .map_err(|_| PROFILE_ERROR)?;
        let id = relative.to_str().ok_or(PROFILE_ERROR)?;
        profile_path(&base, Some(id))?;
        Some(id.to_string())
    };
    let directory = base.join(DIRECTORY);
    let mut warning = SELECTED.get().and_then(|state| state.warning.clone());
    let mut return_original_pending = false;
    let pending = match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => {
            let _directory = open_directory(&directory)?;
            return_original_pending =
                read::<OriginalIntent>(&directory.join(RETURN_ORIGINAL), work)?.is_some()
                    && id.is_some();
            match read::<Pending>(&directory.join(PENDING), work).and_then(|value| {
                if let Some(value) = &value {
                    validate_pending(value)?;
                }
                Ok(value)
            }) {
                Ok(Some(value)) if Some(value.profile.as_str()) != id.as_deref() => {
                    Some(PendingProfile {
                        destination: profile_path(&base, Some(&value.profile))?,
                        profile_id: value.profile,
                        recovery_id: value.recovery_id,
                        request_id: value.request_id,
                        restart_required: true,
                    })
                }
                Ok(_) => None,
                Err(error) => {
                    work.check()?;
                    warning = Some(error);
                    None
                }
            }
        }
    };
    let mut recoveries = Vec::new();
    let mut scan_limited = false;
    let recovery_root = current.join("case-recovery-v1");
    match std::fs::symlink_metadata(&recovery_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        _ => {
            let _directory = open_directory(&recovery_root)?;
            for (examined, entry) in std::fs::read_dir(&recovery_root)
                .map_err(|e| e.to_string())?
                .enumerate()
            {
                work.check()?;
                if examined >= 4096 {
                    scan_limited = true;
                    break;
                }
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if uuid::Uuid::parse_str(name)
                    .ok()
                    .is_none_or(|id| id.to_string() != name)
                {
                    continue;
                }
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if !kind.is_dir() || kind.is_symlink() {
                    continue;
                }
                let metadata = match std::fs::symlink_metadata(entry.path().join("manifest.json")) {
                    Ok(metadata)
                        if metadata.is_file()
                            && !metadata.file_type().is_symlink()
                            && metadata.len() <= MANIFEST_LIMIT as u64 =>
                    {
                        metadata
                    }
                    _ => continue,
                };
                let created_at_ms = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                    .filter(|ms| *ms <= 9_007_199_254_740_991);
                recoveries.push(RecoverySummary {
                    recovery_id: name.into(),
                    created_at_ms,
                    state: "requires_verification",
                });
                recoveries.sort_by(|a, b| {
                    b.created_at_ms
                        .cmp(&a.created_at_ms)
                        .then_with(|| a.recovery_id.cmp(&b.recovery_id))
                });
                if recoveries.len() > 128 {
                    recoveries.pop();
                    scan_limited = true;
                }
            }
        }
    }
    work.check()?;
    Ok(ProfileStatus {
        current_profile: ProfileLocation { id, path: current },
        restart_only: true,
        startup_warning: warning,
        pending,
        recoveries,
        scan_limited,
        return_original_pending,
    })
}

fn profile_path(base: &Path, id: Option<&str>) -> Result<PathBuf, String> {
    match id {
        None => Ok(base.to_path_buf()),
        Some(id) => {
            let parsed = uuid::Uuid::parse_str(id).map_err(|_| PROFILE_ERROR)?;
            if parsed.to_string() != id {
                return Err(PROFILE_ERROR.into());
            }
            Ok(base.join(DIRECTORY).join(id))
        }
    }
}
fn controller(base: &Path) -> Result<(PathBuf, File), String> {
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let _base = open_directory(base)?;
    let directory = base.join(DIRECTORY);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let _directory = open_directory(&directory)?;
    let lock = lock_file(&directory.join("selection.lock"), true)?;
    FileExt::try_lock_exclusive(&lock).map_err(|_| {
        "CASE_PROFILE_BUSY: Outra seleção de recuperação está em andamento.".to_string()
    })?;
    Ok((directory, lock))
}
fn read<T: serde::de::DeserializeOwned>(path: &Path, work: &Work<'_>) -> Result<Option<T>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => (),
        _ => return Err(PROFILE_ERROR.into()),
    }
    let bytes = bounded_read(path, PROFILE_LIMIT, work)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| PROFILE_ERROR.into())
}
fn active(directory: &Path, work: &Work<'_>) -> Result<Active, String> {
    let value = read(directory.join(ACTIVE).as_path(), work)?.unwrap_or(Active {
        version: 1,
        profile: None,
    });
    if value.version != 1 {
        return Err(PROFILE_ERROR.into());
    }
    if let Some(id) = &value.profile {
        profile_path(Path::new(""), Some(id))?;
    }
    Ok(value)
}
fn write(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or(PROFILE_ERROR)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    struct Output<'a> {
        file: &'a mut File,
        bytes: usize,
    }
    impl Write for Output<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > PROFILE_LIMIT.saturating_sub(self.bytes) {
                return Err(std::io::Error::other(PROFILE_ERROR));
            }
            let written = self.file.write(bytes)?;
            self.bytes += written;
            Ok(written)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    serde_json::to_writer(
        Output {
            file: temporary.as_file_mut(),
            bytes: 0,
        },
        value,
    )
    .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.error.to_string())?;
    #[cfg(test)]
    if path.file_name().is_some_and(|name| name == ACTIVE)
        && FAIL_AFTER_ACTIVE.with(|v| v.replace(false))
    {
        return Err("injected post-publication sync failure".into());
    }
    #[cfg(test)]
    if path.file_name().is_some_and(|name| name == PENDING)
        && FAIL_AFTER_PENDING.with(|v| v.replace(false))
    {
        return Err("injected post-publication sync failure".into());
    }
    sync_directory(parent)
}
fn verify_pending(base: &Path, pending: &Pending, work: &Work<'_>) -> Result<PathBuf, String> {
    validate_pending(pending)?;
    if pending.version != 1
        || pending.restore.request_id != pending.request_id
        || pending.restore.recovery_id != pending.recovery_id
    {
        return Err(PROFILE_ERROR.into());
    }
    let source = profile_path(base, pending.from.profile.as_deref())?;
    let target = profile_path(base, Some(&pending.profile))?;
    if source == target {
        return Err(PROFILE_ERROR.into());
    }
    let _target = open_directory(&target)?;
    let recovery = load_verified(&source, &pending.recovery_id, work)?;
    verify_restored_profile(
        &target,
        &recovery,
        &pending.restore,
        Some(&pending.proof),
        work,
    )?;
    Ok(target)
}
fn validate_pending(pending: &Pending) -> Result<(), String> {
    for id in [&pending.request_id, &pending.profile, &pending.recovery_id] {
        if uuid::Uuid::parse_str(id)
            .map_err(|_| PROFILE_ERROR)?
            .to_string()
            != *id
        {
            return Err(PROFILE_ERROR.into());
        }
    }
    if pending.version != 1
        || pending.from.version != 1
        || pending.proof.authority.analyses.len() > 1000
        || pending.proof.authority != pending.restore.after
        || pending.proof.database.relative_path != "investigations.sqlite3"
        || pending.proof.database.bytes > TOTAL_LIMIT
        || pending.proof.database.sha256.len() != 64
        || !pending
            .proof
            .database
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(PROFILE_ERROR.into());
    }
    if let Some(id) = &pending.from.profile {
        profile_path(Path::new(""), Some(id))?;
    }
    Ok(())
}

fn original_proof(root: &Path, work: &Work<'_>) -> Result<RestoredProfileProof, String> {
    let lease = RootLease::shared(root, work)?;
    let conn = readonly(&lease.root.join("investigations.sqlite3"))?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    assert_no_permits(&conn)?;
    let authority = crate::case_evidence::snapshot_authority(&conn)?;
    let stage = tempfile::Builder::new()
        .prefix("profile-return-")
        .tempdir_in(lease.root.join("case-recovery-v1"))
        .map_err(|e| e.to_string())?;
    let path = stage.path().join("investigations.sqlite3");
    let mut destination = Connection::open(&path).map_err(|e| e.to_string())?;
    destination
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    backup_database(&conn, &mut destination, work)?;
    integrity_check(&destination, work)?;
    drop(destination);
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let mut file = regular(stage.path(), "investigations.sqlite3")?;
    let bytes = file.metadata().map_err(|e| e.to_string())?.len();
    if bytes > TOTAL_LIMIT {
        return Err(LIMIT.into());
    }
    let database = Asset {
        relative_path: "investigations.sqlite3".into(),
        bytes,
        sha256: hash_reader(&mut file, bytes, work)?,
    };
    lease.validate()?;
    work.check()?;
    Ok(RestoredProfileProof {
        database,
        authority,
    })
}

pub(crate) fn prepare_original_restart(
    base: &Path,
    current: &Path,
    request_id: &str,
    work: &Work<'_>,
) -> Result<PreparedOriginal, String> {
    work.check()?;
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let current = current.canonicalize().map_err(|e| e.to_string())?;
    if uuid::Uuid::parse_str(request_id)
        .map_err(|_| PROFILE_ERROR)?
        .to_string()
        != request_id
    {
        return Err(PROFILE_ERROR.into());
    }
    let (directory, _lock) = controller(&base)?;
    let from = active(&directory, work)?;
    let current_path = profile_path(&base, from.profile.as_deref())?;
    let _current = open_directory(&current_path)?;
    if current_path.canonicalize().map_err(|e| e.to_string())? != current {
        return Err(PROFILE_ERROR.into());
    }
    if read::<Pending>(&directory.join(PENDING), work)?.is_some() {
        return Err("CASE_PROFILE_PENDING: Uma recuperação já está pronta para reiniciar.".into());
    }
    if from.profile.is_none() {
        return Ok(PreparedOriginal {
            destination: base,
            restart_required: false,
            original_preserved: true,
            warning: None,
        });
    }
    let intent = OriginalIntent {
        version: 1,
        request_id: request_id.into(),
        from,
        proof: original_proof(&base, work)?,
    };
    if let Some(prior) = read::<OriginalIntent>(&directory.join(RETURN_ORIGINAL), work)? {
        if prior != intent {
            return Err(
                "CASE_PROFILE_PENDING: Outra seleção já está preparada para reiniciar.".into(),
            );
        }
        return Ok(PreparedOriginal {
            destination: base,
            restart_required: true,
            original_preserved: true,
            warning: None,
        });
    }
    work.check()?;
    let warning = match write(&directory.join(RETURN_ORIGINAL), &intent) {
        Ok(()) => None,
        Err(error) => {
            let final_check = Work {
                cancelled: &|| false,
                progress: work.progress,
            };
            match read::<OriginalIntent>(&directory.join(RETURN_ORIGINAL), &final_check) {
                Ok(Some(actual)) if actual == intent => Some("O retorno ao perfil original está preparado; a confirmação de persistência da seleção encontrou uma falha.".into()),
                Ok(None) => return Err(error),
                _ => return Err("CASE_PROFILE_SELECTION_UNCERTAIN: Não foi possível confirmar a seleção do perfil original; verifique antes de reiniciar.".into()),
            }
        }
    };
    crate::operations::commit();
    Ok(PreparedOriginal {
        destination: base,
        restart_required: true,
        original_preserved: true,
        warning,
    })
}

/// This only stages the restart selection. It never changes this process's
/// root, starts a new executable or exposes live restore_current through IPC.
pub(crate) fn prepare_restart(
    base: &Path,
    current: &Path,
    recovery_id: &str,
    request_id: &str,
    work: &Work<'_>,
) -> Result<PreparedProfile, String> {
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let current = current.canonicalize().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::parse_str(request_id)
        .map_err(|_| PROFILE_ERROR)?
        .to_string();
    if id != request_id {
        return Err(PROFILE_ERROR.into());
    }
    let (directory, _lock) = controller(&base)?;
    if read::<OriginalIntent>(&directory.join(RETURN_ORIGINAL), work)?.is_some() {
        return Err(
            "CASE_PROFILE_PENDING: O retorno ao perfil original já está preparado para reiniciar."
                .into(),
        );
    }
    let from = active(&directory, work)?;
    if profile_path(&base, from.profile.as_deref())?
        .canonicalize()
        .map_err(|e| e.to_string())?
        != current
    {
        return Err("CASE_PROFILE_CHANGED: Reinicie para usar a seleção atual antes de preparar outra recuperação.".into());
    }
    if let Some(pending) = read::<Pending>(&directory.join(PENDING), work)? {
        if pending.request_id != request_id
            || pending.recovery_id != recovery_id
            || pending.from != from
        {
            return Err(
                "CASE_PROFILE_PENDING: Uma recuperação já está pronta para reiniciar.".into(),
            );
        }
        let target = verify_pending(&base, &pending, work)?;
        return Ok(PreparedProfile {
            profile_id: pending.profile,
            destination: target,
            restart_required: true,
            original_preserved: true,
            warning: None,
        });
    }
    let recovery = load_verified(&current, recovery_id, work)?;
    let target = profile_path(&base, Some(&id))?;
    // Existing directories are never reused as fresh destinations. An
    // interrupted unselected copy remains preserved for explicit recovery.
    std::fs::create_dir(&target).map_err(|_| {
        "CASE_PROFILE_DESTINATION: O destino já existe; a seleção anterior está intacta."
            .to_string()
    })?;
    // This DB belongs to an unselected staging profile. Keep the outer
    // cancellation scope alive until pending selection is durably published.
    let restore = crate::operations::run_with_token(crate::operations::current_token(), || {
        restore_fresh(&target, &recovery, request_id, work)
    })??;
    #[cfg(test)]
    if CANCEL_AFTER_COPY.with(|v| v.replace(false)) {
        if let Some(id) = crate::operations::current_id() {
            crate::operations::cancel_id(&id);
        }
    }
    work.check()?;
    let proof = verify_restored_profile(&target, &recovery, &restore, None, work)?;
    let pending = Pending {
        version: 1,
        request_id: request_id.into(),
        from,
        profile: id.clone(),
        recovery_id: recovery_id.into(),
        restore,
        proof,
    };
    work.check()?;
    let warning = match write(&directory.join(PENDING), &pending) {
        Ok(()) => None,
        Err(error) => {
            // An atomic rename may already have published the intent before
            // directory sync fails. Reconcile under the same selection lock.
            let final_check = Work {
                cancelled: &|| false,
                progress: work.progress,
            };
            match read::<Pending>(&directory.join(PENDING), &final_check) {
                Ok(Some(actual)) if actual == pending => Some("A recuperação está preparada para o reinício; a confirmação de persistência da seleção encontrou uma falha.".into()),
                Ok(None) => return Err(error),
                _ => {
                    crate::operations::commit();
                    return Err("CASE_PROFILE_SELECTION_UNCERTAIN: Não foi possível confirmar a seleção publicada. Preserve este perfil e verifique a recuperação antes de reiniciar.".into());
                }
            }
        }
    };
    crate::operations::commit();
    Ok(PreparedProfile {
        profile_id: id,
        destination: target,
        restart_required: true,
        original_preserved: true,
        warning,
    })
}

fn resolve_startup(base: &Path, work: &Work<'_>) -> Result<StartupProfile, String> {
    let canonical = base.canonicalize().map_err(|e| e.to_string())?;
    let base = canonical.as_path();
    let (directory, _lock) = controller(base)?;
    let old = active(&directory, work)?;
    let previous = profile_path(base, old.profile.as_deref())?;
    // Every fallback below must use a verified directory, including malformed
    // intent and failed target verification branches.
    let _previous = open_directory(&previous)?;
    if let Some(intent) = read::<OriginalIntent>(&directory.join(RETURN_ORIGINAL), work)? {
        if intent.version != 1
            || intent.from.version != 1
            || uuid::Uuid::parse_str(&intent.request_id)
                .map_err(|_| PROFILE_ERROR)?
                .to_string()
                != intent.request_id
        {
            return Err(PROFILE_ERROR.into());
        }
        let completed = directory.join(format!("returned-{}.json", intent.request_id));
        if old.profile.is_none() {
            let _ = std::fs::rename(directory.join(RETURN_ORIGINAL), completed);
            return Ok(StartupProfile {
                selected: previous,
                warning: None,
            });
        }
        if old != intent.from || original_proof(base, work).as_ref() != Ok(&intent.proof) {
            let _ = std::fs::rename(
                directory.join(RETURN_ORIGINAL),
                directory.join(format!("failed-return-{}.json", intent.request_id)),
            );
            let _ = sync_directory(&directory);
            return Ok(StartupProfile {
                selected: previous,
                warning: Some(
                    "O perfil original mudou desde a preparação; a seleção atual foi preservada."
                        .into(),
                ),
            });
        }
        let choice = Active {
            version: 1,
            profile: None,
        };
        let warning = match write(&directory.join(ACTIVE), &choice) {
            Ok(()) => None,
            Err(error) => {
                let final_check = Work {
                    cancelled: &|| false,
                    progress: work.progress,
                };
                if active(&directory, &final_check)? != choice {
                    return Err(error);
                }
                Some("O perfil original foi selecionado; houve uma falha ao confirmar a persistência do marcador.".into())
            }
        };
        let cleanup_warning = std::fs::rename(directory.join(RETURN_ORIGINAL), completed)
            .and_then(|_| sync_directory(&directory).map_err(std::io::Error::other))
            .err()
            .map(|_| {
                "O retorno foi selecionado; sua confirmação será reconciliada na próxima abertura."
                    .into()
            });
        return Ok(StartupProfile {
            selected: base.to_path_buf(),
            warning: warning.or(cleanup_warning),
        });
    }
    let pending = match read::<Pending>(&directory.join(PENDING), work) {
        Ok(value) => value,
        Err(error) => {
            return Ok(StartupProfile {
                selected: previous,
                warning: Some(error),
            })
        }
    };
    let Some(pending) = pending else {
        let _previous = open_directory(&previous)?;
        return Ok(StartupProfile {
            selected: previous,
            warning: None,
        });
    };
    if let Err(error) = validate_pending(&pending) {
        return Ok(StartupProfile {
            selected: previous,
            warning: Some(error),
        });
    }
    if old.profile.as_deref() == Some(pending.profile.as_str()) {
        // ACTIVE is the commit marker. A crash after it may leave an intent;
        // later legitimate edits in that active profile must not be undone by
        // comparing them to the original pre-activation database fingerprint.
        let _previous = open_directory(&previous)?;
        let _ = std::fs::rename(
            directory.join(PENDING),
            directory.join(format!("completed-{}.json", pending.request_id)),
        );
        let _ = sync_directory(&directory);
        return Ok(StartupProfile {
            selected: previous,
            warning: None,
        });
    }
    let target = if pending.from == old {
        verify_pending(base, &pending, work)
    } else {
        Err(PROFILE_ERROR.into())
    };
    match target {
        Ok(target) => {
            // Commit only after the entire target verifies. On restart, all
            // later data/catalog/MCP/cache initialization uses this fixed root.
            let choice = Active {
                version: 1,
                profile: Some(pending.profile),
            };
            let publication_warning = match write(&directory.join(ACTIVE), &choice) {
                Ok(()) => None,
                Err(error) => {
                    let final_check = Work {
                        cancelled: &|| false,
                        progress: work.progress,
                    };
                    if active(&directory, &final_check)? == choice {
                        Some("O perfil verificado foi selecionado; a confirmação de persistência do marcador encontrou uma falha.".into())
                    } else {
                        return Err(error);
                    }
                }
            };
            let completed = directory.join(format!("completed-{}.json", pending.request_id));
            let warning=std::fs::rename(directory.join(PENDING), completed)
                .and_then(|_|sync_directory(&directory).map_err(std::io::Error::other))
                .err().map(|_|"O perfil foi selecionado; a confirmação de reinício será reconciliada na próxima abertura.".into());
            Ok(StartupProfile {
                selected: target,
                warning: publication_warning.or(warning),
            })
        }
        Err(error) => {
            let failed = directory.join(format!("failed-{}.json", pending.request_id));
            // Keep failed intent as a diagnostic; original selection is never
            // overwritten just to report a verification failure.
            let _ = std::fs::rename(directory.join(PENDING), failed);
            let _ = sync_directory(&directory);
            Ok(StartupProfile {
                selected: previous,
                warning: Some(error),
            })
        }
    }
}
fn initialize_in(cell: &OnceLock<StartupProfile>, base: &Path, work: &Work<'_>) -> StartupProfile {
    cell.get_or_init(|| {
        std::fs::create_dir_all(base)
            .map_err(|e| e.to_string())
            .and_then(|_| base.canonicalize().map_err(|e| e.to_string()))
            .and_then(|root| resolve_startup(&root, work))
            .unwrap_or_else(|error| {
                // Reading an atomic committed marker does not require the writer
                // lock. A busy controller must keep selected B, never reopen A.
                let final_check = Work {
                    cancelled: &|| false,
                    progress: work.progress,
                };
                let previous = base
                    .canonicalize()
                    .map_err(|e| e.to_string())
                    .and_then(|root| {
                        let _controller = open_directory(&root.join(DIRECTORY))?;
                        let value = active(&root.join(DIRECTORY), &final_check)?;
                        let path = profile_path(&root, value.profile.as_deref())?;
                        // Check managed paths before canonicalization can
                        // erase evidence that they were replaced by a link.
                        let _selected = open_directory(&path)?;
                        path.canonicalize().map_err(|e| e.to_string())
                    })
                    .unwrap_or_else(|_| base.to_path_buf());
                StartupProfile {
                    selected: previous,
                    warning: Some(error),
                }
            })
    })
    .clone()
}
pub(crate) fn initialize(base: &Path, work: &Work<'_>) -> StartupProfile {
    initialize_in(&SELECTED, base, work)
}
#[cfg(test)]
thread_local! {
    static FAIL_AFTER_ACTIVE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};
    static FAIL_AFTER_PENDING: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};
    static CANCEL_AFTER_COPY: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};
}

#[cfg(test)]
mod tests {
    use super::*;
    fn work() -> Work<'static> {
        Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        }
    }
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO metadata VALUES('revision','1');").unwrap();
        conn.execute("INSERT INTO cases VALUES('c',?1,0)",[r#"{"id":"c","items":[{"rows":[{"id":1,"fields":{"exact":18446744073709551615,"decimal":1.00}}]}]}"#]).unwrap();
        root
    }
    #[test]
    fn prepared_restore_is_unselected_until_verified_restart_and_preserves_original() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let before = crate::case_evidence::snapshot_authority(
            &readonly(&root.path().join("investigations.sqlite3")).unwrap(),
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let result = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &id,
            &task,
        )
        .unwrap();
        assert!(result.restart_required && result.original_preserved);
        assert_eq!(selected(root.path()), root.path());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
        let replay = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &id,
            &task,
        )
        .unwrap();
        assert_eq!(replay.destination, result.destination);
        let chosen = resolve_startup(root.path(), &task).unwrap();
        assert!(chosen.warning.is_none(), "{:?}", chosen.warning);
        assert_eq!(chosen.selected, result.destination);
        assert_eq!(
            crate::case_evidence::snapshot_authority(
                &readonly(&root.path().join("investigations.sqlite3")).unwrap()
            )
            .unwrap(),
            before
        );
        let body: String = readonly(&chosen.selected.join("investigations.sqlite3"))
            .unwrap()
            .query_row("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
            .unwrap();
        assert!(body.contains("18446744073709551615") && body.contains("1.00"));
    }
    #[test]
    fn changed_target_or_cancelled_preparation_never_selects_it() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let result = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &id,
            &task,
        )
        .unwrap();
        let conn = Connection::open(result.destination.join("investigations.sqlite3")).unwrap();
        conn.execute("UPDATE metadata SET value='99' WHERE key='revision'", [])
            .unwrap();
        drop(conn);
        let chosen = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(chosen.selected, root.path().canonicalize().unwrap());
        assert!(chosen.warning.is_some());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
        let cancelled = Work {
            cancelled: &|| true,
            progress: &|_, _, _| {},
        };
        assert!(prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &cancelled
        )
        .is_err());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
    }
    #[test]
    fn activation_commit_reconciles_leftover_intent_without_reverting_later_state() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let result = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        write(
            &root.path().join(DIRECTORY).join(ACTIVE),
            &Active {
                version: 1,
                profile: Some(result.profile_id),
            },
        )
        .unwrap();
        let conn = Connection::open(result.destination.join("investigations.sqlite3")).unwrap();
        conn.execute("UPDATE metadata SET value='99' WHERE key='revision'", [])
            .unwrap();
        drop(conn);
        let chosen = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(chosen.selected, result.destination);
        assert!(chosen.warning.is_none());
        assert!(!root.path().join(DIRECTORY).join(PENDING).exists());
    }
    #[test]
    fn valid_sqlite_edit_without_revision_and_invalid_selector_paths_fail_closed() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let result = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        let conn = Connection::open(result.destination.join("investigations.sqlite3")).unwrap();
        conn.execute_batch(
            "CREATE TABLE unacknowledged(body TEXT);INSERT INTO unacknowledged VALUES('changed');",
        )
        .unwrap();
        drop(conn);
        let chosen = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(chosen.selected, root.path().canonicalize().unwrap());
        assert!(chosen.warning.is_some());
        let result = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        let path = root.path().join(DIRECTORY).join(PENDING);
        let mut pending = read::<Pending>(&path, &task).unwrap().unwrap();
        pending.request_id = "../../escape".into();
        write(&path, &pending).unwrap();
        let chosen = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(chosen.selected, root.path().canonicalize().unwrap());
        assert!(chosen.warning.is_some());
        assert!(path.exists());
        assert!(result.destination.exists());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
    }
    #[test]
    fn published_active_marker_wins_over_a_late_sync_failure() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let prepared = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        FAIL_AFTER_ACTIVE.with(|v| v.set(true));
        let startup = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(startup.selected, prepared.destination);
        assert!(startup.warning.is_some());
        assert_eq!(
            active(&root.path().join(DIRECTORY), &task)
                .unwrap()
                .profile
                .as_deref(),
            Some(prepared.profile_id.as_str())
        );
    }
    #[test]
    fn published_pending_marker_is_acknowledged_after_late_sync_failure() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        FAIL_AFTER_PENDING.with(|v| v.set(true));
        let prepared = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &id,
            &task,
        )
        .unwrap();
        assert!(prepared.warning.is_some());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
        let replay = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &id,
            &task,
        )
        .unwrap();
        assert_eq!(replay.destination, prepared.destination);
        let selected = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(selected.selected, prepared.destination);
    }
    #[test]
    fn original_profile_return_is_verified_and_only_selected_on_restart() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let prepared = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        resolve_startup(root.path(), &task).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let result =
            prepare_original_restart(root.path(), &prepared.destination, &id, &task).unwrap();
        assert!(result.restart_required && result.original_preserved);
        assert_eq!(
            active(&root.path().join(DIRECTORY), &task)
                .unwrap()
                .profile
                .as_deref(),
            Some(prepared.profile_id.as_str())
        );
        assert!(
            status(root.path(), &prepared.destination, &task)
                .unwrap()
                .return_original_pending
        );
        prepare_original_restart(root.path(), &prepared.destination, &id, &task).unwrap();
        let selected = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(selected.selected, root.path().canonicalize().unwrap());
        assert!(prepared
            .destination
            .join("investigations.sqlite3")
            .is_file());
        assert!(!root.path().join(DIRECTORY).join(RETURN_ORIGINAL).exists());
    }
    #[test]
    fn changed_original_does_not_replace_the_current_restored_profile() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let prepared = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        resolve_startup(root.path(), &task).unwrap();
        prepare_original_restart(
            root.path(),
            &prepared.destination,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        Connection::open(root.path().join("investigations.sqlite3"))
            .unwrap()
            .execute_batch("CREATE TABLE external_change(body TEXT);")
            .unwrap();
        let selected = resolve_startup(root.path(), &task).unwrap();
        assert_eq!(selected.selected, prepared.destination);
        assert!(selected.warning.is_some());
        assert!(!root.path().join(DIRECTORY).join(RETURN_ORIGINAL).exists());
        prepare_original_restart(
            root.path(),
            &prepared.destination,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
    }
    #[test]
    fn inventory_is_bounded_and_does_not_claim_recovery_integrity() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let directory = root.path().join("case-recovery-v1");
        for _ in 0..130 {
            let path = directory.join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("manifest.json"), b"unverified").unwrap();
        }
        let inventory = status(root.path(), root.path(), &task).unwrap();
        assert_eq!(inventory.recoveries.len(), 128);
        assert!(inventory.scan_limited);
        assert!(inventory
            .recoveries
            .iter()
            .all(|item| item.state == "requires_verification"));
        assert!(inventory.current_profile.id.is_none());
        assert!(load_verified(root.path(), &recovery.receipt().recovery_id, &task).is_ok());
    }
    #[test]
    fn busy_selection_preserves_committed_profile_and_initialization_has_one_winner() {
        let root = fixture();
        let task = work();
        let recovery = prepare(root.path(), &task).unwrap();
        let prepared = prepare_restart(
            root.path(),
            root.path(),
            &recovery.receipt().recovery_id,
            &uuid::Uuid::new_v4().to_string(),
            &task,
        )
        .unwrap();
        resolve_startup(root.path(), &task).unwrap();
        let (_directory, _lock) = controller(root.path()).unwrap();
        let cell = std::sync::Arc::new(OnceLock::new());
        let result = initialize_in(&cell, root.path(), &task);
        assert_eq!(result.selected, prepared.destination);
        assert!(result.warning.is_some());
        let another = fixture();
        let one = std::sync::Arc::clone(&cell);
        let path = another.path().to_path_buf();
        let other = std::thread::spawn(move || initialize_in(&one, &path, &work()))
            .join()
            .unwrap();
        assert_eq!(
            other.selected, result.selected,
            "a process never follows later roots or selectors"
        );
    }
    #[test]
    fn named_cancel_after_destination_commit_cannot_publish_pending_selection() {
        let root = fixture();
        let recovery = prepare(root.path(), &work()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(format!("profile-cancel-{id}"))).unwrap();
        CANCEL_AFTER_COPY.with(|v| v.set(true));
        let task = Work {
            cancelled: &crate::operations::cancelled,
            progress: &|_, _, _| {},
        };
        let result = crate::operations::run_with_token(token, || {
            prepare_restart(
                root.path(),
                root.path(),
                &recovery.receipt().recovery_id,
                &id,
                &task,
            )
        });
        assert!(result.is_err() || result.unwrap().is_err());
        assert!(!root.path().join(DIRECTORY).join(PENDING).exists());
        assert!(!root.path().join(DIRECTORY).join(ACTIVE).exists());
        assert!(
            root.path()
                .join(DIRECTORY)
                .join(id)
                .join("investigations.sqlite3")
                .exists(),
            "cancelled copied profile remains unselected; original is untouched"
        );
    }
    #[cfg(unix)]
    #[test]
    fn fallback_never_follows_a_managed_profile_or_controller_symlink() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let outside = fixture();
        let task = work();
        let (directory, lock) = controller(root.path()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        write(
            &directory.join(ACTIVE),
            &Active {
                version: 1,
                profile: Some(id.clone()),
            },
        )
        .unwrap();
        drop(lock);
        symlink(outside.path(), directory.join(id)).unwrap();
        std::fs::write(directory.join(PENDING), b"{malformed").unwrap();
        assert!(
            resolve_startup(root.path(), &task).is_err(),
            "a malformed pending marker cannot bypass the active directory check"
        );
        let result = initialize_in(&OnceLock::new(), root.path(), &task);
        assert_eq!(result.selected, root.path().canonicalize().unwrap());
        assert!(result.warning.is_some());
        let other = fixture();
        symlink(&directory, other.path().join(DIRECTORY)).unwrap();
        let result = initialize_in(&OnceLock::new(), other.path(), &task);
        assert_eq!(result.selected, other.path().canonicalize().unwrap());
        assert!(result.warning.is_some());
    }
}
pub(crate) fn selected(base: &Path) -> PathBuf {
    SELECTED
        .get()
        .map(|state| state.selected.clone())
        .unwrap_or_else(|| base.to_path_buf())
}
