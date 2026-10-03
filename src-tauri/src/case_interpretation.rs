//! Immutable, portable Case-effective interpretation. Profile files are read
//! only by the explicit one-time local migration, never by an admitted query.
use crate::analysis_context::{Diagnostic, Identity, Snapshot};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path};

pub(crate) const MIGRATED: &str = "case-interpretation-v1";
const MAX_BYTES: usize = 3 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    #[serde(default)]
    pub codes: crate::model::CodesConfig,
    #[serde(default)]
    pub system_codes: crate::model::CodesConfig,
    #[serde(default)]
    pub timestamps: BTreeMap<String, crate::sources::TsConfig>,
    #[serde(default)]
    pub formats: Vec<crate::CustomFormat>,
    #[serde(default)]
    pub resources: crate::case_resources::Preferences,
    #[serde(default)]
    pub security: crate::case_security::Settings,
}
impl Default for Settings {
    fn default() -> Self {
        Self { schema_version: 1, codes: serde_json::from_str(include_str!("default_codes.json")).unwrap_or_default(),
            system_codes: Default::default(), timestamps: Default::default(), formats: Vec::new(), resources: Default::default(), security: Default::default() }
    }
}
impl Settings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 { return Err("Versão de interpretação do Caso não suportada.".into()); }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_BYTES {
            return Err("A interpretação do Caso excede o limite de 3 MiB.".into());
        }
        if self.timestamps.len() > 4096 || self.formats.len() > 256 { return Err("Muitas definições de interpretação no Caso.".into()); }
        self.resources.validate()?;
        self.security.validate()?;
        for (path, config) in &self.timestamps {
            if path.is_empty() || path.len() > 16_384 { return Err("Caminho de timestamp inválido.".into()); }
            config.compile()?;
        }
        let mut names = std::collections::HashSet::new();
        for format in &self.formats {
            if format.name.trim().is_empty() || !names.insert(&format.name) { return Err("Nome de formato vazio ou duplicado.".into()); }
            format.to_parse()?;
        }
        Ok(())
    }
}
fn legacy_file<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<Option<T>, String> {
    let file = match std::fs::File::open(dir.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES { return Err(format!("{name} excede 3 MiB")); }
    serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string())
}
pub(crate) fn local_legacy(dir: &Path) -> (Settings, Vec<Diagnostic>) {
    let mut settings = Settings::default();
    let mut diagnostics = Vec::new();
    macro_rules! load { ($field:ident, $name:literal) => {
        match legacy_file(dir, $name) {
            Ok(Some(value)) => { let mut candidate = settings.clone(); candidate.$field = value;
                match candidate.validate() { Ok(()) => settings = candidate, Err(error) => diagnostics.push(legacy_diagnostic($name, &error)) }
            },
            Ok(None) => (), Err(error) => diagnostics.push(legacy_diagnostic($name, &error)),
        }
    }; }
    load!(codes, "codes.json"); load!(system_codes, "system_codes.json");
    load!(timestamps, "ts_configs.json"); load!(formats, "formats.json");
    let (security, issues) = crate::case_security::local_legacy(dir);
    let mut candidate = settings.clone(); candidate.security = security;
    match candidate.validate() { Ok(()) => settings = candidate, Err(error) => { for domain in ["security_settings", "security_rules", "security_sigma", "security_threats"] { let mut issue = legacy_diagnostic("security", &error); issue.code = format!("legacy_interpretation_unavailable_{domain}"); diagnostics.push(issue); } } }
    diagnostics.extend(issues);
    (settings, diagnostics)
}
fn legacy_diagnostic(file: &str, error: &str) -> Diagnostic {
    Diagnostic { definition_index: None, code: format!("legacy_interpretation_unavailable_{}", match file { "codes.json" => "codes", "system_codes.json" => "systemCodes", "ts_configs.json" => "timestamps", "formats.json" => "formats", _ => "security" }),
        message: format!("A configuração legada {file} não pôde ser ativada ({error}). O arquivo original foi preservado; revise o Caso.") }
}
pub(crate) fn origin_unavailable_diagnostic() -> Diagnostic {
    Diagnostic { definition_index: None, code: "legacy_interpretation_unavailable_origin".into(),
        message: "Esta recuperação antiga não registrou a interpretação do perfil original. Os registros preservados podem ser vistos e exportados; a reanálise depende de recuperar a configuração original. Não foram herdados catálogos ou regras do perfil de destino.".into() }
}
pub(crate) fn origin_unavailable(conn: &rusqlite::Connection) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='case-interpretation-origin-unavailable')", [], |row| row.get(0)).map_err(|e| e.to_string())
}
pub(crate) fn local_legacy_for(conn: &rusqlite::Connection, dir: &Path) -> Result<(Settings, Vec<Diagnostic>), String> {
    if origin_unavailable(conn)? { Ok((Settings::default(), vec![origin_unavailable_diagnostic()])) }
    else { Ok(local_legacy(dir)) }
}
pub(crate) fn mark_origin_unavailable(snapshot: &mut Snapshot) {
    snapshot.interpretation = Some(Settings::default());
    let diagnostic = origin_unavailable_diagnostic();
    if !snapshot.migration_diagnostics.contains(&diagnostic) { snapshot.migration_diagnostics.push(diagnostic); }
}

pub(crate) fn import_defaults(snapshot: &mut Snapshot) {
    if snapshot.interpretation.is_none() {
        snapshot.interpretation = Some(Settings::default());
        snapshot.migration_diagnostics.push(Diagnostic { definition_index: None, code: "import_interpretation_defaults".into(),
            message: "O arquivo anterior à versão 0.12 não contém catálogos, formatos ou ajustes de data/hora. Aplicados somente padrões internos; configurações do perfil de destino não foram herdadas.".into() });
    }
}
pub(crate) fn initialize(tx: &rusqlite::Transaction<'_>, dir: &Path) -> Result<(), String> {
    let done: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key=?1)", [MIGRATED], |row| row.get(0)).map_err(|e| e.to_string())?;
    if done { return Ok(()); }
    let ids = {
        let mut stmt = tx.prepare("SELECT case_id FROM case_analysis").map_err(|e| e.to_string())?;
        let ids = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        ids
    };
    let mut legacy = None;
    for id in ids {
        let mut snapshot = crate::analysis_context::read(tx, &id)?;
        if snapshot.interpretation.is_none() {
            if legacy.is_none() { legacy = Some(local_legacy_for(tx, dir)?); }
            let (settings, diagnostics) = legacy.as_ref().unwrap();
            snapshot.interpretation = Some(settings.clone());
            snapshot.migration_diagnostics.extend(diagnostics.clone());
            crate::analysis_context::write(tx, &snapshot)?;
        }
    }
    tx.execute("INSERT INTO metadata(key,value) VALUES(?1,'1')", [MIGRATED]).map_err(|e| e.to_string())?;
    Ok(())
}
/// Native management/export paths bypass ordinary analytical admission. Freeze
/// old local settings before either can expose/duplicate/export a Case. The
/// caller holds the recovery root lease; legacy/mixed stores must finish their
/// verified adoption first, so this never changes inputs before that backup.
pub(crate) fn initialize_native(conn: &mut rusqlite::Connection, dir: &Path) -> Result<(), String> {
    let native: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_evidence_store')", [], |row| row.get(0)).map_err(|e| e.to_string())?;
    if !native { return Ok(()); }
    let incomplete: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM cases c LEFT JOIN native_evidence_cases n ON n.case_id=c.id WHERE n.case_id IS NULL)", [], |row| row.get(0)).map_err(|e| e.to_string())?;
    if incomplete { return Err("CASE_EVIDENCE_MIGRATION_REQUIRED: Abra a investigação para concluir a adoção verificada antes de exportar seus Casos.".into()); }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    initialize(&tx, dir)?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn update(expected: &Identity, change: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<Snapshot, String> {
    update_at(&crate::config_dir(), expected, change)
}
pub(crate) fn update_domain(expected: &Identity, domain: &str, change: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<Snapshot, String> {
    update_at_inner(&crate::config_dir(), expected, Some(domain), change)
}
pub(crate) fn update_at(dir: &Path, expected: &Identity, change: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<Snapshot, String> {
    update_at_inner(dir, expected, None, change)
}
fn update_at_inner(dir: &Path, expected: &Identity, domain: Option<&str>, change: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<Snapshot, String> {
    let mut conn = crate::case_store::context_connection(dir)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let mut snapshot = crate::analysis_context::read(&tx, &expected.case_id)?;
    if snapshot.identity() != *expected { return Err(crate::analysis_runtime::STALE.into()); }
    let settings = snapshot.interpretation.as_mut().ok_or("Interpretação do Caso não inicializada.")?;
    change(settings)?;
    settings.validate()?;
    if let Some(domain) = domain {
        let repaired = format!("legacy_interpretation_unavailable_{domain}");
        snapshot.migration_diagnostics.retain(|diagnostic| diagnostic.code != repaired);
    }
    snapshot.config_revision = snapshot.config_revision.checked_add(1).ok_or("Revisão do Caso excedeu o limite.")?;
    crate::operations::check()?;
    crate::analysis_context::write(&tx, &snapshot)?;
    tx.commit().map_err(|e| e.to_string())?;
    crate::operations::commit();
    Ok(snapshot)
}
pub(crate) fn current() -> Result<std::sync::Arc<crate::analysis_runtime::Admitted>, String> {
    crate::analysis_runtime::current().filter(|admitted| admitted.identity.is_some()).ok_or_else(|| "Informe o contexto do Caso antes de consultar ou editar sua interpretação.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn snapshot(dir: &Path, case: &str) -> Snapshot {
        crate::analysis_context::read(&crate::case_store::context_connection(dir).unwrap(), case).unwrap()
    }
    fn cases(dir: &Path) {
        crate::case_store::save_at(dir, json!({"cases":[{"id":"a"},{"id":"b"}],"active":null})).unwrap();
    }
    fn catalog(name: &str) -> crate::model::CodesConfig {
        serde_json::from_value(json!({"app":{"200":{"name":name,"description":format!("{name} description")}}})).unwrap()
    }
    #[test]
    fn case_settings_cas_reopen_duplicate_and_old_import_do_not_share_profile() {
        let dir = tempfile::tempdir().unwrap();
        cases(dir.path());
        let a = snapshot(dir.path(), "a");
        let b = snapshot(dir.path(), "b");
        let changed = update_at(dir.path(), &a.identity(), |settings| {
            settings.codes = catalog("A");
            settings.resources = crate::case_resources::Preferences { schema_version: 1, mode: crate::case_resources::Mode::Custom, work_limit_mib: Some(32) };
            settings.security.detection_settings_json = r#"{"disabled":["auth.bruteforce.source"],"threats":false}"#.into();
            settings.timestamps.insert("same.log".into(), crate::sources::TsConfig { sources: vec!["linha".into()], format: "epoch_s".into(), ..Default::default() });
            Ok(())
        }).unwrap();
        assert_eq!(snapshot(dir.path(), "b"), b);
        assert_eq!(snapshot(dir.path(), "a"), changed);
        assert!(update_at(dir.path(), &a.identity(), |settings| { settings.codes = catalog("stale"); Ok(()) }).is_err());
        assert_eq!(snapshot(dir.path(), "a"), changed);
        let duplicate = crate::analysis_context::prepare_portable_snapshot("copy", &changed).unwrap();
        assert_ne!(duplicate.analysis_id, changed.analysis_id);
        assert_eq!(duplicate.interpretation, changed.interpretation);
        let raw = serde_json::to_string(&changed).unwrap();
        let decoded = crate::case_evidence::native_snapshot_from_value(serde_json::from_str(&raw).unwrap()).unwrap();
        assert_eq!(decoded, changed, "native portable typed adapter keeps all settings");
        let mut old = changed.clone(); old.interpretation = None;
        std::fs::write(dir.path().join("codes.json"), serde_json::to_vec(&catalog("destination-only")).unwrap()).unwrap();
        let imported = crate::analysis_context::prepare_portable_snapshot("old-import", &old).unwrap();
        assert_eq!(imported.interpretation.unwrap(), Settings::default());
        assert!(imported.migration_diagnostics.iter().any(|issue| issue.code == "import_interpretation_defaults"));
    }
    #[test]
    fn startup_readers_preserve_malformed_catalogs_until_explicit_case_migration() {
        for version in [None, Some("0")] {
            let dir = tempfile::tempdir().unwrap(); cases(dir.path());
            let conn = crate::case_store::connect(dir.path()).unwrap();
            for id in ["a", "b"] {
                let mut old = crate::analysis_context::read(&conn, id).unwrap(); old.interpretation = None;
                conn.execute("UPDATE case_analysis SET body=?2 WHERE case_id=?1", rusqlite::params![id,serde_json::to_string(&old).unwrap()]).unwrap();
            }
            conn.execute("DELETE FROM metadata WHERE key=?1", [MIGRATED]).unwrap();
            drop(conn);
            let codes = dir.path().join("codes.json");
            let system = dir.path().join("system_codes.json");
            let marker = dir.path().join("codes.version");
            std::fs::write(&codes, b"invalid-user-original").unwrap();
            std::fs::write(&system, b"invalid-system-original").unwrap();
            if let Some(version) = version { std::fs::write(&marker, version).unwrap(); }
            assert!(!crate::load_codes(&codes).sources.is_empty(), "startup may use an in-memory builtin template");
            assert!(crate::load_system_codes(&system).sources.is_empty());
            assert_eq!(std::fs::read(&codes).unwrap(), b"invalid-user-original");
            assert_eq!(std::fs::read(&system).unwrap(), b"invalid-system-original");
            assert_eq!(std::fs::read_to_string(&marker).ok().as_deref(), version);
            let migrated = snapshot(dir.path(), "a");
            for domain in ["codes", "systemCodes"] {
                assert!(migrated.migration_diagnostics.iter().any(|d| d.code == format!("legacy_interpretation_unavailable_{domain}")));
            }
            assert_eq!(std::fs::read(&codes).unwrap(), b"invalid-user-original");
            assert_eq!(std::fs::read(&system).unwrap(), b"invalid-system-original");
            // A second startup/read cannot erase repair diagnostics or make
            // unavailable legacy interpretation appear silently migrated.
            crate::load_codes(&codes); crate::load_system_codes(&system);
            assert_eq!(snapshot(dir.path(), "a"), migrated);
        }
        let empty = tempfile::tempdir().unwrap();
        crate::load_codes(&empty.path().join("codes.json"));
        crate::load_system_codes(&empty.path().join("system_codes.json"));
        assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_v011_migration_freezes_each_case_once_and_preserves_invalid_originals() {
        let dir = tempfile::tempdir().unwrap(); cases(dir.path());
        let conn = crate::case_store::connect(dir.path()).unwrap();
        for id in ["a", "b"] {
            let mut old = crate::analysis_context::read(&conn, id).unwrap(); old.interpretation = None;
            conn.execute("UPDATE case_analysis SET body=?2 WHERE case_id=?1", rusqlite::params![id,serde_json::to_string(&old).unwrap()]).unwrap();
        }
        conn.execute("DELETE FROM metadata WHERE key=?1", [MIGRATED]).unwrap();
        let global = serde_json::to_vec(&catalog("legacy")).unwrap();
        std::fs::write(dir.path().join("codes.json"), &global).unwrap();
        std::fs::write(dir.path().join("formats.json"), b"invalid-json").unwrap();
        drop(conn);
        let a = snapshot(dir.path(), "a"); let b = snapshot(dir.path(), "b");
        assert_eq!(a.interpretation.as_ref().unwrap().codes, catalog("legacy"));
        assert_eq!(a.interpretation, b.interpretation);
        assert!(a.migration_diagnostics.iter().any(|d| d.code == "legacy_interpretation_unavailable_formats"));
        assert_eq!(std::fs::read(dir.path().join("codes.json")).unwrap(), global);
        assert_eq!(std::fs::read(dir.path().join("formats.json")).unwrap(), b"invalid-json");
        std::fs::write(dir.path().join("codes.json"), serde_json::to_vec(&catalog("later-global")).unwrap()).unwrap();
        assert_eq!(snapshot(dir.path(), "a"), a);
        let changed = update_at(dir.path(), &a.identity(), |s| { s.codes = catalog("case-edit"); Ok(()) }).unwrap();
        assert_eq!(snapshot(dir.path(), "b"), b);
        assert!(changed.migration_diagnostics.iter().any(|d| d.code == "legacy_interpretation_unavailable_formats"), "unrelated edit cannot erase repair blockers");
    }
}
