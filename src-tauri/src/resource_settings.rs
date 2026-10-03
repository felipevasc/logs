//! Restart-only, profile-scoped preferences. The running process retains one
//! immutable startup value; saving never changes a pool or a live engine.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const FILE: &str = "resource-settings.json";
const FILE_LIMIT: u64 = 16 * 1024;
static STARTUP: OnceLock<Startup> = OnceLock::new();
static SAVING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Mode {
    Automatic,
    Custom,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Preferences {
    schema_version: u32,
    mode: Mode,
    memory_limit_mib: Option<u64>,
    #[serde(default)]
    parallelism_limit: Option<usize>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            mode: Mode::Automatic,
            memory_limit_mib: None,
            parallelism_limit: None,
        }
    }
}
impl Preferences {
    fn validate(&self, maximum: u64) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("Versão de configuração de recursos não suportada.".into());
        }
        if self.parallelism_limit.is_some_and(|value| !(1..=64).contains(&value)) {
            return Err("Informe um limite de paralelismo inteiro entre 1 e 64.".into());
        }
        match (&self.mode, self.memory_limit_mib) {
            (Mode::Automatic, None) => Ok(()),
            (Mode::Custom, Some(value)) if (128..=maximum).contains(&value) => Ok(()),
            (Mode::Automatic, Some(_)) => {
                Err("O modo Automático não aceita orçamento personalizado.".into())
            }
            _ => Err(format!(
                "Informe um orçamento inteiro entre 128 e {maximum} MiB."
            )),
        }
    }
}

struct Startup {
    directory: PathBuf,
    preferences: Preferences,
    warning: Option<String>,
}
impl Startup {
    fn load(directory: &Path) -> Self {
        let (preferences, warning) = read_or_default(directory);
        Self {
            directory: directory.to_path_buf(),
            preferences,
            warning,
        }
    }
}

pub(crate) fn initialize(directory: &Path) {
    STARTUP.get_or_init(|| Startup::load(directory));
}
pub(crate) fn startup_memory_mib() -> Option<u64> {
    STARTUP.get().and_then(|s| s.preferences.memory_limit_mib)
}

pub(crate) fn startup_parallelism_limit() -> Option<usize> {
    STARTUP.get().and_then(|s| s.preferences.parallelism_limit)
}

fn open_settings_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            // FILE_ATTRIBUTE_REPARSE_POINT
            return Err(std::io::Error::other(
                "Configuração de recursos não pode ser um link.",
            ));
        }
    }
    if !metadata.is_file() || metadata.len() > FILE_LIMIT {
        return Err(std::io::Error::other(
            "Configuração de recursos precisa ser um arquivo regular de até 16 KiB.",
        ));
    }
    Ok(file)
}
fn read(directory: &Path) -> Result<Preferences, String> {
    // Inspect the opened handle, not a pathname checked before opening. A
    // concurrent replacement with a symlink/FIFO must not be followed or block.
    let file = match open_settings_file(&directory.join(FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default())
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err("Configuração de recursos excede o tamanho permitido.".into());
    }
    let value: Preferences = serde_json::from_slice(&bytes)
        .map_err(|_| "Configuração de recursos inválida.".to_string())?;
    // A profile may have moved to a smaller machine. Retain a valid preference;
    // Budget::for_machine clamps the active value to this machine's safe range.
    value.validate(8192)?;
    Ok(value)
}
fn read_or_default(directory: &Path) -> (Preferences, Option<String>) {
    match read(directory) {
        Ok(value) => (value, None),
        Err(error) => (
            Preferences::default(),
            Some(format!(
                "{error} Usando Automático; o arquivo original foi preservado."
            )),
        ),
    }
}
fn persist(directory: &Path, preferences: &Preferences, maximum: u64) -> Result<(), String> {
    preferences.validate(maximum)?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    // Same-volume replacement: a failed write leaves the previous preference
    // intact, including on Windows. No delete-before-rename fallback.
    let mut pending = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut pending, preferences).map_err(|e| e.to_string())?;
    pending.as_file().sync_all().map_err(|e| e.to_string())?;
    pending
        .persist(directory.join(FILE))
        .map_err(|e| e.error.to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    active: crate::resources::Snapshot,
    active_preferences: Preferences,
    saved: Preferences,
    minimum_memory_mib: u64,
    maximum_memory_mib: u64,
    maximum_parallelism: usize,
    restart_required: bool,
    startup_warning: Option<String>,
    saved_warning: Option<String>,
}
fn status(startup: &Startup) -> Status {
    let (saved, saved_warning) = read_or_default(&startup.directory);
    Status {
        active: crate::resources::snapshot(),
        active_preferences: startup.preferences.clone(),
        restart_required: saved != startup.preferences,
        saved,
        minimum_memory_mib: 128,
        maximum_memory_mib: crate::resources::maximum_memory_mib(crate::resources::total_memory()),
        maximum_parallelism: crate::resources::maximum_parallelism(),
        startup_warning: startup.warning.clone(),
        saved_warning,
    }
}
fn startup() -> Result<&'static Startup, String> {
    STARTUP
        .get()
        .ok_or_else(|| "Configuração de recursos não inicializada.".into())
}
#[tauri::command]
pub(crate) fn resource_settings_status() -> Result<Status, String> {
    let _guard = SAVING.lock();
    Ok(status(startup()?))
}
#[tauri::command]
pub(crate) fn resource_settings_save(preferences: Preferences) -> Result<Status, String> {
    let _guard = SAVING.lock();
    let startup = startup()?;
    let maximum = crate::resources::maximum_parallelism();
    if preferences.parallelism_limit.is_some_and(|value| value > maximum) {
        return Err(format!("Informe um limite de paralelismo entre 1 e {maximum} nesta máquina."));
    }
    persist(
        &startup.directory,
        &preferences,
        crate::resources::maximum_memory_mib(crate::resources::total_memory()),
    )?;
    Ok(status(startup))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn custom(value: u64) -> Preferences {
        Preferences {
            schema_version: 1,
            mode: Mode::Custom,
            memory_limit_mib: Some(value),
            parallelism_limit: None,
        }
    }
    #[test]
    fn validates_modes_versions_and_machine_ceiling() {
        assert!(Preferences::default().validate(2048).is_ok());
        assert!(custom(128).validate(2048).is_ok());
        for value in [0, 127, 2049, u64::MAX] {
            assert!(custom(value).validate(2048).is_err());
        }
        let mut invalid = custom(512);
        invalid.schema_version = 2;
        assert!(invalid.validate(2048).is_err());
        invalid = custom(512);
        invalid.mode = Mode::Automatic;
        assert!(invalid.validate(2048).is_err());
        invalid = custom(512);
        invalid.memory_limit_mib = None;
        assert!(invalid.validate(2048).is_err());
        assert!(serde_json::from_str::<Preferences>(
            r#"{"schemaVersion":1,"mode":"custom","memoryLimitMib":1.5}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Preferences>(
            r#"{"schemaVersion":1,"mode":"automatic","extra":true}"#
        )
        .is_err());
    }
    #[test]
    fn parallelism_is_optional_independent_and_restart_only() {
        let legacy: Preferences = serde_json::from_str(r#"{"schemaVersion":1,"mode":"automatic","memoryLimitMib":null}"#).unwrap();
        assert_eq!(legacy.parallelism_limit, None);
        let mut limited = legacy.clone();
        limited.parallelism_limit = Some(1);
        assert!(limited.validate(2048).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let running = Startup::load(dir.path());
        persist(dir.path(), &limited, 2048).unwrap();
        assert_eq!(running.preferences.parallelism_limit, None);
        assert_eq!(Startup::load(dir.path()).preferences.parallelism_limit, Some(1));
        for value in [0, 65, usize::MAX] {
            limited.parallelism_limit = Some(value);
            assert!(limited.validate(2048).is_err());
        }
        limited.parallelism_limit = Some(64);
        persist(dir.path(), &limited, 2048).unwrap();
        assert_eq!(read(dir.path()).unwrap().parallelism_limit, Some(64), "a moved profile retains its original value for active clamping");
        assert!(serde_json::from_str::<Preferences>(r#"{"schemaVersion":1,"mode":"automatic","memoryLimitMib":null,"parallelismLimit":1.5}"#).is_err());
    }
    #[test]
    fn save_is_restart_only_and_uses_selected_directory() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected-profile");
        let running = Startup::load(&selected);
        persist(&selected, &custom(512), 4096).unwrap();
        assert_eq!(running.preferences, Preferences::default());
        assert_eq!(read(&selected).unwrap(), custom(512));
        assert_eq!(Startup::load(&selected).preferences, custom(512));
        assert!(!root.path().join(FILE).exists());
        assert!(persist(&selected, &custom(4097), 4096).is_err());
        assert_eq!(read(&selected).unwrap(), custom(512));
        persist(&selected, &Preferences::default(), 4096).unwrap();
        assert_eq!(read(&selected).unwrap(), Preferences::default());
    }
    #[test]
    fn corrupt_or_oversized_file_falls_back_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        for bytes in [b"broken".to_vec(), vec![b' '; FILE_LIMIT as usize + 1]] {
            std::fs::write(dir.path().join(FILE), &bytes).unwrap();
            let state = Startup::load(dir.path());
            assert_eq!(state.preferences, Preferences::default());
            assert!(state.warning.is_some());
            assert_eq!(std::fs::read(dir.path().join(FILE)).unwrap(), bytes);
        }
    }
    #[test]
    fn moved_profile_retains_preference_for_resource_clamping() {
        let dir = tempfile::tempdir().unwrap();
        persist(dir.path(), &custom(8192), 8192).unwrap();
        assert_eq!(read(dir.path()).unwrap(), custom(8192));
    }
    #[test]
    fn failed_replace_preserves_existing_destination_and_cleans_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join(FILE);
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("keep"), b"unchanged").unwrap();
        assert!(persist(dir.path(), &custom(512), 4096).is_err());
        assert_eq!(
            std::fs::read(destination.join("keep")).unwrap(),
            b"unchanged"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn linked_file_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join(FILE)).unwrap();
        assert!(read(dir.path()).is_err());
        persist(dir.path(), &custom(512), 4096).unwrap();
        assert_eq!(read(dir.path()).unwrap(), custom(512));
        assert_eq!(std::fs::read(outside).unwrap(), b"{}");
    }
    #[cfg(unix)]
    #[test]
    fn fifo_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(read(dir.path()).is_err());
    }
}

/// The application panel remains restart-only. This separate Case command
/// changes only its persisted accounted-result quota with identity CAS.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseStatus {
    #[serde(serialize_with = "crate::analysis_context::serialize_management_snapshot")]
    analysis_context: crate::analysis_context::Snapshot,
    preferences: crate::case_resources::Preferences,
    effective: serde_json::Value,
    minimum_work_mib: u64,
    maximum_work_mib: u64,
    clamped: bool,
}
fn case_status(snapshot: crate::analysis_context::Snapshot) -> Result<CaseStatus, String> {
    let preferences = snapshot.interpretation.as_ref().map(|settings| settings.resources.clone()).unwrap_or_default();
    let policy = crate::case_resources::Policy::capture(Some(&snapshot.identity()), &preferences)?;
    let effective = policy.snapshot();
    let application = effective["applicationWorkMib"].as_u64().unwrap_or(0)
        .saturating_add(effective["applicationSelectionMib"].as_u64().unwrap_or(0));
    Ok(CaseStatus {
        clamped: preferences.work_limit_mib.is_some_and(|requested| requested > application),
        analysis_context: snapshot, preferences, effective,
        minimum_work_mib: 8, maximum_work_mib: application.min(8192),
    })
}
#[tauri::command]
pub(crate) fn case_resource_settings_status(identity: crate::analysis_context::Identity) -> Result<CaseStatus, String> {
    let snapshot = crate::analysis_runtime::validate_identity(&identity)?;
    case_status((*snapshot).clone())
}
#[tauri::command]
pub(crate) fn case_resource_settings_save(expected: crate::analysis_context::Identity, preferences: crate::case_resources::Preferences) -> Result<CaseStatus, String> {
    preferences.validate()?;
    let snapshot = crate::case_interpretation::update(&expected, |settings| {
        settings.resources = preferences;
        Ok(())
    })?;
    case_status(snapshot)
}
