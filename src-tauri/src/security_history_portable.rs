//! Frozen historical profiles travel as checked bytes, bound to an archive
//! owner and rebound to a new local owner. No archive path is used.
use crate::{
    analysis_context::Snapshot,
    case_archive_format::{Entry, EntryKind, Source},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Profile {
    pub case_id: String,
    pub analysis_id: String,
    pub name: String,
    pub revision: String,
    pub id: String,
    pub bytes: u64,
    pub sha256: String,
}
pub(crate) struct Captured {
    pub profile: Profile,
    pub source: Source,
}
fn selection(snapshot: &Snapshot) -> Result<Option<(String, String)>, String> {
    let Some(interpretation) = &snapshot.interpretation else {
        return Ok(None);
    };
    let settings = interpretation.security.detection_settings()?;
    settings.investigation.validate()?;
    Ok(settings
        .investigation
        .profile
        .zip(settings.investigation.profile_revision))
}
fn destination(root: &Path, snapshot: &Snapshot, name: &str, revision: &str) -> PathBuf {
    let key = crate::security_history::profile_key(
        &format!("{}|{}", snapshot.case_id, snapshot.analysis_id),
        name,
    );
    root.join("environment-history")
        .join(format!("{key}-{revision}.sqlite"))
}
fn digest(file: &mut File) -> Result<(u64, String), String> {
    let mut buffer = [0u8; 65536];
    let mut size = 0u64;
    let mut hash = Sha256::new();
    loop {
        crate::operations::check()?;
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > 1024 * 1024 * 1024 {
            return Err("Perfil histórico excede 1 GiB".into());
        }
        hash.update(&buffer[..n]);
    }
    Ok((size, format!("{:x}", hash.finalize())))
}
fn validate(path: &Path) -> Result<(), String> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    db.progress_handler(10000, Some(crate::operations::cancelled));
    db.execute_batch("PRAGMA query_only=ON;PRAGMA trusted_schema=OFF;PRAGMA cache_size=-2048")
        .map_err(|e| e.to_string())?;
    let integrity: String = db
        .query_row("PRAGMA integrity_check(1)", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err("Perfil histórico corrompido".into());
    }
    let manifest: String = db
        .query_row(
            "SELECT payload FROM manifest WHERE length(payload)<=32768",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let m: serde_json::Value = serde_json::from_str(&manifest).map_err(|e| e.to_string())?;
    if m["version"] != crate::security_history::VERSION
        || m["normalization_version"] != crate::evidence::NORMALIZATION_VERSION
        || m["frozen"] != true
    {
        return Err("Perfil histórico incompatível ou não congelado".into());
    }
    for table in ["pairs", "hours"] {
        let n: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if n > 5000000 {
            return Err("Perfil histórico excede cinco milhões de grupos".into());
        }
    }
    Ok(())
}
pub(crate) fn capture(root: &Path, snapshot: &Snapshot) -> Result<Option<Captured>, String> {
    let Some((name, revision)) = selection(snapshot)? else {
        return Ok(None);
    };
    let path = destination(root, snapshot, &name, &revision);
    validate(&path)?;
    let mut file = File::open(&path).map_err(|e| e.to_string())?;
    let (bytes, sha256) = digest(&mut file)?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let profile = Profile {
        case_id: snapshot.case_id.clone(),
        analysis_id: snapshot.analysis_id.clone(),
        name,
        revision,
        id: id.clone(),
        bytes,
        sha256: sha256.clone(),
    };
    Ok(Some(Captured {
        profile,
        source: Source {
            entry: Entry {
                kind: EntryKind::History(id),
                bytes,
                sha256,
            },
            file,
        },
    }))
}
pub(crate) fn prepare(
    root: &Path,
    foreign: &Snapshot,
    local: &Snapshot,
    profiles: &[Profile],
    entries: &[Entry],
    path: impl Fn(usize) -> Result<PathBuf, String>,
) -> Result<(), String> {
    let selected = selection(foreign)?;
    let matching = profiles
        .iter()
        .filter(|p| p.case_id == foreign.case_id)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        if selected.is_some() {
            return Err("O arquivo portátil omitiu o perfil histórico selecionado".into());
        }
        return Ok(());
    }
    if matching.len() != 1 {
        return Err("Mais de um perfil histórico para o mesmo Caso".into());
    }
    let p = matching[0];
    if selected != Some((p.name.clone(), p.revision.clone()))
        || p.analysis_id != foreign.analysis_id
    {
        return Err("Perfil histórico de outro proprietário ou seleção".into());
    }
    let index = entries
        .iter()
        .position(|e| e.kind == EntryKind::History(p.id.clone()))
        .ok_or("Bytes do perfil histórico ausentes")?;
    if entries[index].sha256 != p.sha256 || entries[index].bytes != p.bytes {
        return Err("Integridade do perfil histórico divergiu do manifesto".into());
    }
    let source = path(index)?;
    validate(&source)?;
    let target = destination(root, local, &p.name, &p.revision);
    let directory = target.parent().unwrap();
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::Builder::new()
        .prefix("profile-import-")
        .tempfile_in(directory)
        .map_err(|e| e.to_string())?;
    let mut input = File::open(&source).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 65536];
    use std::io::Write;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    loop {
        crate::operations::check()?;
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hash.update(&buffer[..n]);
        temporary
            .write_all(&buffer[..n])
            .map_err(|e| e.to_string())?;
    }
    if bytes != p.bytes || format!("{:x}", hash.finalize()) != p.sha256 {
        return Err("Os bytes do perfil histórico mudaram durante a preparação".into());
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    crate::operations::check()?;
    temporary
        .persist_noclobber(&target)
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn validate_owners(
    profiles: &[Profile],
    snapshots: &[Snapshot],
    entries: &[Entry],
) -> Result<(), String> {
    if profiles.len() > 1024 {
        return Err("Perfis portáteis excessivos".into());
    }
    let mut cases = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for p in profiles {
        if !cases.insert(&p.case_id)
            || !ids.insert(&p.id)
            || !snapshots
                .iter()
                .any(|s| s.case_id == p.case_id && s.analysis_id == p.analysis_id)
        {
            return Err("Proprietário de perfil portátil inválido".into());
        }
    }
    if entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::History(_)))
        .count()
        != profiles.len()
    {
        return Err("Perfil portátil sem declaração ou bytes".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn snapshot(case: &str) -> Snapshot {
        let mut interpretation = crate::case_interpretation::Settings::default();
        let mut settings = crate::detections::Settings::default();
        settings.investigation.profile = Some("reviewed".into());
        settings.investigation.profile_revision = Some("revision-a".into());
        interpretation.security.detection_settings_json = serde_json::to_string(&settings).unwrap();
        Snapshot {
            schema_version: 1,
            case_id: case.into(),
            analysis_id: format!("analysis-{case}"),
            config_revision: 0,
            visibility_revision: 0,
            config: Default::default(),
            interpretation: Some(interpretation),
            migration_diagnostics: vec![],
            legacy_raw: None,
        }
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_frozen_history_transports_exact_bytes_rebinds_owner_and_rejects_foreign_or_changed_files(
    ) {
        let root = tempfile::tempdir().unwrap();
        let source = snapshot("source");
        let path = destination(root.path(), &source, "reviewed", "revision-a");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE manifest(payload TEXT);CREATE TABLE pairs(ns TEXT,entity TEXT);CREATE TABLE hours(ns TEXT,hour INTEGER);").unwrap();
        db.execute("INSERT INTO manifest VALUES(?1)",[json!({"version":crate::security_history::VERSION,"normalization_version":crate::evidence::NORMALIZATION_VERSION,"frozen":true,"interpretation":"preserved"}).to_string()]).unwrap();
        db.execute("INSERT INTO pairs VALUES('tenant-a','alice')", [])
            .unwrap();
        drop(db);
        let captured = capture(root.path(), &source).unwrap().unwrap();
        let local = snapshot("local");
        let profiles = vec![captured.profile.clone()];
        let entries = vec![captured.source.entry.clone()];
        validate_owners(&profiles, std::slice::from_ref(&source), &entries).unwrap();
        prepare(root.path(), &source, &local, &profiles, &entries, |_| {
            Ok(path.clone())
        })
        .unwrap();
        let imported = destination(root.path(), &local, "reviewed", "revision-a");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&imported).unwrap()
        );
        let mut bad = profiles.clone();
        bad[0].analysis_id = "foreign".into();
        assert!(validate_owners(&bad, std::slice::from_ref(&source), &entries).is_err());
        assert!(prepare(
            root.path(),
            &source,
            &snapshot("another"),
            &[],
            &[],
            |_| Ok(path.clone())
        )
        .is_err());
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(prepare(
            root.path(),
            &source,
            &snapshot("changed"),
            &profiles,
            &entries,
            |_| Ok(path.clone())
        )
        .is_err());
    }
}
