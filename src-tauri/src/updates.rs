//! Updates published as GitHub Releases. The interface asks before anything is
//! downloaded; checking, downloading, verifying the signature and installing
//! happen here, so the webview needs no updater permission. Release flow and
//! policy: docs/atualizacoes.md.

use crate::config_dir;
use parking_lot::Mutex;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Error as UpdaterError, Update, UpdaterExt};

pub const RELEASES_URL: &str = "https://github.com/felipevasc/logs/releases";
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const PREFS_FILE: &str = "updates.json";
const BACKUPS_DIR: &str = "backups";
const BACKUPS_KEPT: usize = 3;
/// Larger files are caches or imported data that no version migrates.
const BACKUP_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const NOTES_LIMIT: usize = 6000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Prefs {
    check_on_start: bool,
    skipped_version: Option<String>,
    last_run_version: Option<String>,
    /// Written right before an installer runs; the next start compares it with its own version.
    pending: Option<Pending>,
    last_check: Option<LastCheck>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            check_on_start: true,
            skipped_version: None,
            last_run_version: None,
            pending: None,
            last_check: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    from: String,
    to: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCheck {
    at: String,
    /// `available`, `current`, `unavailable` or `error`.
    outcome: String,
    message: String,
}

/// Result of the last installation, shown once after the app opens.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// `updated` or `failed`.
    kind: &'static str,
    from: Option<String>,
    to: String,
    backup: Option<String>,
}

/// How this copy was installed, recorded in the executable by the bundler.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Nsis,
    Msi,
    Appimage,
    Deb,
    Rpm,
    Other,
}

impl Kind {
    fn current() -> Self {
        use tauri::utils::{config::BundleType, platform::bundle_type};
        match bundle_type() {
            Some(BundleType::Nsis) => Self::Nsis,
            Some(BundleType::Msi) => Self::Msi,
            Some(BundleType::AppImage) => Self::Appimage,
            Some(BundleType::Deb) => Self::Deb,
            Some(BundleType::Rpm) => Self::Rpm,
            _ => Self::Other,
        }
    }

    /// The system asks for an administrator password while these packages install.
    fn needs_admin(self) -> bool {
        matches!(self, Self::Msi | Self::Deb | Self::Rpm)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
    Installing,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    current_version: String,
    install_kind: Kind,
    /// Why this copy cannot update itself; updates are then manual.
    unavailable: Option<&'static str>,
    /// Why the announced update cannot be installed right now.
    blocker: Option<String>,
    needs_admin: bool,
    check_on_start: bool,
    skipped_version: Option<String>,
    last_check: Option<LastCheck>,
    phase: Phase,
    available: Option<Available>,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
    install_on_close: bool,
    notice: Option<Notice>,
    releases_url: &'static str,
}

struct Inner {
    prefs: Prefs,
    phase: Phase,
    update: Option<Update>,
    /// Downloaded package, already verified against the signature and version.
    package: Option<Arc<Vec<u8>>>,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
    install_on_close: bool,
    close_requested: Option<Instant>,
    notice: Option<Notice>,
    download: Option<tauri::async_runtime::JoinHandle<()>>,
}

pub struct UpdateState {
    inner: Mutex<Inner>,
    // A successful installer handoff must keep new work paused until exit,
    // including when its awaiting IPC future is dropped before restart.
    shutdown_pause: Mutex<Option<crate::operations::UpdatePause>>,
}

/// Runs before anything reads the data folder: on the first start of a version
/// it copies the data files, and it turns the outcome of the last installation
/// into a notice for the interface.
pub fn prepare(current: &Version) -> UpdateState {
    let dir = config_dir();
    let mut prefs = load_prefs(&dir);
    let version = current.to_string();
    let previous = prefs.last_run_version.clone();
    let backup = if previous.as_deref() == Some(version.as_str()) {
        None
    } else {
        backup_data(&dir, previous.as_deref(), &version).unwrap_or(None)
    };
    let notice = notice_for(prefs.pending.take(), previous.as_deref(), current, backup);
    prefs.last_run_version = Some(version);
    let _ = save_prefs(&dir, &prefs);
    UpdateState {
        shutdown_pause: Mutex::new(None),
        inner: Mutex::new(Inner {
            prefs,
            phase: Phase::Idle,
            update: None,
            package: None,
            downloaded: 0,
            total: None,
            error: None,
            install_on_close: false,
            close_requested: None,
            notice,
            download: None,
        }),
    }
}

fn notice_for(
    pending: Option<Pending>,
    previous: Option<&str>,
    current: &Version,
    backup: Option<PathBuf>,
) -> Option<Notice> {
    let backup = backup.map(|path| path.display().to_string());
    if let Some(pending) = pending {
        let installed = Version::parse(&pending.to).is_ok_and(|to| *current >= to);
        return Some(Notice {
            kind: if installed { "updated" } else { "failed" },
            from: Some(pending.from),
            to: if installed { current.to_string() } else { pending.to },
            backup,
        });
    }
    let previous = Version::parse(previous?).ok()?;
    (previous < *current).then(|| Notice {
        kind: "updated",
        from: Some(previous.to_string()),
        to: current.to_string(),
        backup,
    })
}

fn load_prefs(dir: &Path) -> Prefs {
    std::fs::read_to_string(dir.join(PREFS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_prefs(dir: &Path, prefs: &Prefs) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut temp, prefs).map_err(std::io::Error::other)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join(PREFS_FILE)).map_err(|error| error.error)?;
    Ok(())
}

/// Copies the data files (cases, settings and SQLite stores) and the imported
/// Sigma rules before another version reads them. Caches, indexes, snapshots and
/// case images stay out: they are large and no version migrates them.
fn backup_data(dir: &Path, from: Option<&str>, to: &str) -> std::io::Result<Option<PathBuf>> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(None) };
    let files: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry.metadata().is_ok_and(|meta| meta.len() <= BACKUP_FILE_LIMIT)
                && !entry.file_name().to_string_lossy().starts_with(PREFS_FILE)
        })
        .map(|entry| entry.path())
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    let root = dir.join(BACKUPS_DIR);
    let name = format!(
        "{}_{}_para_{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        from.map(safe_name).unwrap_or_else(|| "anterior".into()),
        safe_name(to)
    );
    let target = root.join(name);
    std::fs::create_dir_all(&target)?;
    for file in &files {
        if let Some(name) = file.file_name() {
            std::fs::copy(file, target.join(name))?;
        }
    }
    let sigma = dir.join("sigma");
    if sigma.is_dir() {
        copy_tree(&sigma, &target.join("sigma"), 4)?;
    }
    prune_backups(&root);
    Ok(Some(target))
}

fn safe_name(version: &str) -> String {
    version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .take(40)
        .collect()
}

fn copy_tree(from: &Path, to: &Path, depth: usize) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        } else if kind.is_dir() && depth > 0 {
            copy_tree(&entry.path(), &target, depth - 1)?;
        }
    }
    Ok(())
}

/// Keeps the newest backups; only folders named by `backup_data` are removed.
fn prune_backups(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.len() > 16 && name.as_bytes()[..8].iter().all(u8::is_ascii_digit))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(BACKUPS_KEPT);
    for name in &names[..excess] {
        let _ = std::fs::remove_dir_all(root.join(name));
    }
}

fn disabled_by_policy() -> bool {
    std::env::var("LOGINSIGHT_UPDATES").is_ok_and(|value| {
        matches!(
            value.trim().to_lowercase().as_str(),
            "0" | "off" | "false" | "no" | "nao" | "não" | "disabled" | "desativado"
        )
    })
}

fn unavailable(kind: Kind) -> Option<&'static str> {
    if cfg!(debug_assertions) {
        Some("As atualizações automáticas ficam desligadas no modo de desenvolvimento.")
    } else if disabled_by_policy() {
        Some("As atualizações automáticas foram desativadas pela variável de ambiente LOGINSIGHT_UPDATES.")
    } else if kind == Kind::Other {
        Some("Esta cópia não foi instalada por um instalador do LogInsight. Para atualizar, baixe a nova versão.")
    } else {
        None
    }
}

fn blocker(kind: Kind) -> Option<String> {
    #[cfg(windows)]
    if kind == Kind::Nsis {
        let exe = std::env::current_exe().ok()?;
        let profile = std::env::var_os("USERPROFILE")?;
        if in_other_profile(&exe, Path::new(&profile)) {
            return Some("O LogInsight está aberto com outra conta (como administrador). Feche-o e abra-o normalmente para instalar a atualização.".into());
        }
    }
    #[cfg(target_os = "linux")]
    if kind == Kind::Appimage {
        let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
        let folder = path.parent()?;
        if tempfile::Builder::new().prefix(".loginsight-update-").tempfile_in(folder).is_err() {
            return Some(format!(
                "Não há permissão para substituir o AppImage em {}. Mova o arquivo para uma pasta sua ou baixe a nova versão.",
                folder.display()
            ));
        }
    }
    let _ = kind;
    None
}

/// A per-user installation lives in its user's profile. Running elevated with
/// another account, the installer would update that other account instead.
#[cfg_attr(not(windows), allow(dead_code))]
fn in_other_profile(exe: &Path, profile: &Path) -> bool {
    let text = |path: &Path| path.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (exe, profile) = (text(exe), text(profile));
    let Some((users, _)) = profile.rsplit_once('\\') else { return false };
    exe.starts_with(&format!("{users}\\")) && !exe.starts_with(&format!("{profile}\\"))
}

fn describe(error: &UpdaterError) -> String {
    match error {
        UpdaterError::Reqwest(error) if error.is_timeout() => "o GitHub não respondeu a tempo".into(),
        UpdaterError::Reqwest(error) if error.is_connect() => "sem conexão com o GitHub".into(),
        UpdaterError::Minisign(_)
        | UpdaterError::SignedVersionMismatch { .. }
        | UpdaterError::MissingSignedVersion => {
            "a assinatura do pacote não confere com a versão anunciada, então ele foi recusado".into()
        }
        UpdaterError::AuthenticationFailed => "a permissão de administrador não foi concedida".into(),
        other => other.to_string(),
    }
}

fn status(app: &AppHandle, inner: &Inner) -> Status {
    let kind = Kind::current();
    let available = inner.update.as_ref().map(|update| Available {
        version: update.version.clone(),
        notes: update.body.as_ref().map(|notes| notes.chars().take(NOTES_LIMIT).collect()),
        date: update.raw_json.get("pub_date").and_then(|value| value.as_str()).map(str::to_owned),
    });
    Status {
        current_version: app.package_info().version.to_string(),
        install_kind: kind,
        unavailable: unavailable(kind),
        blocker: matches!(inner.phase, Phase::Available | Phase::Ready)
            .then(|| blocker(kind))
            .flatten(),
        needs_admin: kind.needs_admin(),
        check_on_start: inner.prefs.check_on_start,
        skipped_version: inner.prefs.skipped_version.clone(),
        last_check: inner.prefs.last_check.clone(),
        phase: inner.phase,
        available,
        downloaded: inner.downloaded,
        total: inner.total,
        error: inner.error.clone(),
        install_on_close: inner.install_on_close,
        notice: None,
        releases_url: RELEASES_URL,
    }
}

fn publish(app: &AppHandle, inner: &Inner) -> Status {
    let status = status(app, inner);
    let _ = app.emit("update-state", &status);
    status
}

async fn check(app: &AppHandle) -> Status {
    let kind = Kind::current();
    {
        let state = app.state::<UpdateState>();
        let mut inner = state.inner.lock();
        let busy = matches!(inner.phase, Phase::Checking | Phase::Downloading | Phase::Ready | Phase::Installing);
        if busy || unavailable(kind).is_some() {
            return status(app, &inner);
        }
        inner.phase = Phase::Checking;
        inner.error = None;
        publish(app, &inner);
    }
    let result = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let state = app.state::<UpdateState>();
    let mut inner = state.inner.lock();
    inner.phase = Phase::Idle;
    inner.update = None;
    let (outcome, message) = match result {
        Ok(Some(update)) => {
            let message = format!("A versão {} está disponível.", update.version);
            inner.update = Some(update);
            inner.phase = Phase::Available;
            ("available", message)
        }
        Ok(None) => ("current", "Você está usando a versão mais recente.".to_owned()),
        Err(UpdaterError::ReleaseNotFound) => ("current", "Nenhuma atualização publicada foi encontrada.".to_owned()),
        Err(UpdaterError::TargetsNotFound(_)) => (
            "unavailable",
            "A versão publicada não tem pacote para este tipo de instalação. Baixe-a manualmente.".to_owned(),
        ),
        Err(error) => ("error", format!("Não foi possível verificar: {}.", describe(&error))),
    };
    inner.prefs.last_check = Some(LastCheck {
        at: chrono::Local::now().to_rfc3339(),
        outcome: outcome.into(),
        message,
    });
    let _ = save_prefs(&config_dir(), &inner.prefs);
    publish(app, &inner)
}

#[tauri::command]
pub fn update_status(app: AppHandle, state: State<'_, UpdateState>) -> Status {
    status(&app, &state.inner.lock())
}

/// Called once by the interface when it loads: hands over the notice about the
/// last installation and checks for a new version when the user allows it.
#[tauri::command]
pub async fn update_startup(app: AppHandle) -> Status {
    let (notice, check_now) = {
        let state = app.state::<UpdateState>();
        let mut inner = state.inner.lock();
        (inner.notice.take(), inner.prefs.check_on_start)
    };
    let mut status = if check_now {
        check(&app).await
    } else {
        status(&app, &app.state::<UpdateState>().inner.lock())
    };
    status.notice = notice;
    status
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Status {
    check(&app).await
}

#[tauri::command]
pub fn update_download(app: AppHandle) -> Result<Status, String> {
    let state = app.state::<UpdateState>();
    let mut inner = state.inner.lock();
    if inner.phase != Phase::Available {
        return Err("Não há atualização aguardando download.".into());
    }
    if let Some(reason) = blocker(Kind::current()) {
        return Err(reason);
    }
    let Some(update) = inner.update.clone() else {
        return Err("Não há atualização aguardando download.".into());
    };
    inner.phase = Phase::Downloading;
    inner.downloaded = 0;
    inner.total = None;
    inner.error = None;
    let handle = app.clone();
    inner.download = Some(tauri::async_runtime::spawn(async move {
        let progress = handle.clone();
        let mut shown = Instant::now();
        let result = update
            .download(
                move |chunk, total| {
                    let state = progress.state::<UpdateState>();
                    let mut inner = state.inner.lock();
                    inner.downloaded += chunk as u64;
                    inner.total = total;
                    if shown.elapsed() >= Duration::from_millis(200) {
                        shown = Instant::now();
                        publish(&progress, &inner);
                    }
                },
                || {},
            )
            .await;
        let state = handle.state::<UpdateState>();
        let mut inner = state.inner.lock();
        inner.download = None;
        match result {
            Ok(package) => {
                inner.package = Some(Arc::new(package));
                inner.phase = Phase::Ready;
            }
            Err(error) => {
                inner.phase = Phase::Available;
                inner.error = Some(format!("O download não foi concluído: {}.", describe(&error)));
            }
        }
        publish(&handle, &inner);
    }));
    Ok(publish(&app, &inner))
}

#[tauri::command]
pub fn update_cancel(app: AppHandle, state: State<'_, UpdateState>) -> Status {
    let mut inner = state.inner.lock();
    if inner.phase == Phase::Downloading {
        if let Some(task) = inner.download.take() {
            task.abort();
        }
        inner.phase = Phase::Available;
        inner.downloaded = 0;
        inner.total = None;
    }
    publish(&app, &inner)
}

/// Takes the verified package and records the attempt, which the next start
/// reports as installed or not.
fn begin_install(app: &AppHandle, inner: &mut Inner) -> Result<(Update, Arc<Vec<u8>>), String> {
    let not_ready = || "Nenhuma atualização está pronta para instalar.".to_owned();
    if inner.phase != Phase::Ready {
        return Err(not_ready());
    }
    if let Some(reason) = blocker(Kind::current()) {
        return Err(reason);
    }
    let update = inner.update.clone().ok_or_else(not_ready)?;
    // Retain the verified bytes until process exit. A launch or worker failure
    // must permit retry without a second download (and without losing the UI).
    let package = inner.package.clone().ok_or_else(not_ready)?;
    inner.prefs.pending = Some(Pending {
        from: update.current_version.clone(),
        to: update.version.clone(),
    });
    if let Err(error) = save_prefs(&config_dir(), &inner.prefs) {
        inner.prefs.pending = None;
        return Err(format!("Não foi possível registrar a atualização: {error}. O aplicativo continua aberto."));
    }
    inner.phase = Phase::Installing;
    inner.install_on_close = false;
    inner.close_requested = None;
    inner.error = None;
    publish(app, inner);
    Ok((update, package))
}

fn restore_failed_install(inner: &mut Inner, reason: &str) -> String {
    inner.prefs.pending = None;
    inner.phase = Phase::Ready;
    inner.install_on_close = false;
    inner.close_requested = None;
    let message = format!("A instalação não foi concluída: {reason}. Tente novamente ou baixe a versão manualmente. O aplicativo continua aberto.");
    inner.error = Some(message.clone());
    message
}

fn show_install_failure(app: &AppHandle, inner: &Inner) {
    let _ = save_prefs(&config_dir(), &inner.prefs);
    publish(app, &inner);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn install_failed(app: &AppHandle, reason: &str) -> String {
    let state = app.state::<UpdateState>();
    let mut inner = state.inner.lock();
    let message = restore_failed_install(&mut inner, reason);
    show_install_failure(app, &inner);
    message
}

fn install_and_retain_pause(
    pause: crate::operations::UpdatePause,
    held: &Mutex<Option<crate::operations::UpdatePause>>,
    install: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    // Errors and unwinding release the reversible guard. Success transfers it
    // before the blocking task completes, leaving no admission gap at await.
    install()?;
    *held.lock() = Some(pause);
    Ok(())
}

#[tauri::command]
pub async fn update_install(app: AppHandle, restart: Option<bool>) -> Result<(), String> {
    let restart = restart.unwrap_or(true);
    let (update, package) = {
        let state = app.state::<UpdateState>();
        let mut inner = state.inner.lock();
        if !restart && inner.close_requested.is_none() {
            return Err("O pedido de instalar ao fechar foi cancelado. Tente novamente.".into());
        }
        begin_install(&app, &mut inner)?
    };
    // On Windows the plugin starts the installer and ends this process; the installer reopens the app.
    let handoff_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // The guard pauses admission, cancels and joins app-owned work. On a
        // failed launch its Drop makes the still-open source usable again.
        let pause = crate::engine::prepare_for_update()?;
        let state = handoff_app.state::<UpdateState>();
        install_and_retain_pause(pause, &state.shutdown_pause, || {
            update.restart_after_install(restart).install(package.as_slice()).map_err(|error| describe(&error))
        })
    }).await;
    match result {
        Ok(Ok(())) if restart => app.restart(),
        Ok(Ok(())) => { app.exit(0); Ok(()) }
        Ok(Err(error)) => Err(install_failed(&app, &error)),
        Err(error) => Err(install_failed(&app, &error.to_string())),
    }
}

#[tauri::command]
pub fn update_install_on_close(app: AppHandle, state: State<'_, UpdateState>, enabled: bool) -> Result<Status, String> {
    let mut inner = state.inner.lock();
    if inner.phase != Phase::Ready {
        return Err("Nenhuma atualização está pronta para instalar.".into());
    }
    inner.install_on_close = enabled;
    if !enabled { inner.close_requested = None; }
    Ok(publish(&app, &inner))
}

/// Installs the downloaded update when the window closes, if the user chose so.
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() != "main" { return; }
    let tauri::WindowEvent::CloseRequested { api, .. } = event else { return };
    let app = window.app_handle().clone();
    let request = Instant::now();
    {
        let state = app.state::<UpdateState>();
        let mut inner = state.inner.lock();
        if inner.phase == Phase::Installing {
            api.prevent_close();
            return;
        }
        if !inner.install_on_close || inner.phase != Phase::Ready {
            return;
        }
        api.prevent_close();
        if inner.close_requested.is_some() { return; }
        inner.close_requested = Some(request);
    }
    // The webview owns pending case edits. It must acknowledge a durable save
    // before invoking update_install(restart=false). Keep the window visible.
    if app.emit("update-close-requested", ()).is_err() {
        install_failed(&app, "não foi possível solicitar o salvamento dos casos");
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let state = app.state::<UpdateState>();
        let mut inner = state.inner.lock();
        if inner.close_requested == Some(request) {
            restore_failed_install(&mut inner, "o salvamento dos casos não confirmou o encerramento a tempo");
            show_install_failure(&app, &inner);
        }
    });
}

#[tauri::command]
pub fn update_set_check_on_start(app: AppHandle, state: State<'_, UpdateState>, enabled: bool) -> Status {
    let mut inner = state.inner.lock();
    inner.prefs.check_on_start = enabled;
    let _ = save_prefs(&config_dir(), &inner.prefs);
    publish(&app, &inner)
}

/// Stops asking about `version` on startup; `None` asks again.
#[tauri::command]
pub fn update_skip(app: AppHandle, state: State<'_, UpdateState>, version: Option<String>) -> Status {
    let mut inner = state.inner.lock();
    inner.prefs.skipped_version = version.filter(|version| Version::parse(version).is_ok());
    let _ = save_prefs(&config_dir(), &inner.prefs);
    publish(&app, &inner)
}

/// Opens the release page of `version`, or of the latest release.
#[tauri::command]
pub fn update_open_page(version: Option<String>) -> Result<(), String> {
    let url = match version.as_deref().and_then(|version| Version::parse(version).ok()) {
        Some(version) => format!("{RELEASES_URL}/tag/v{version}"),
        None => format!("{RELEASES_URL}/latest"),
    };
    open_in_browser(&url)
}

#[cfg(windows)]
fn open_in_browser(url: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let result = unsafe {
        ShellExecuteW(HWND::default(), w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err("Não foi possível abrir o navegador.".into())
    }
}

/// Set by the AppImage runtime for its bundled libraries; a browser started with them may fail.
#[cfg(not(windows))]
const APPIMAGE_VARIABLES: [&str; 16] = [
    "LD_LIBRARY_PATH",
    "GTK_PATH",
    "GTK_DATA_PREFIX",
    "GTK_EXE_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GTK_THEME",
    "GDK_BACKEND",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_EXTRA_MODULES",
    "GIO_MODULE_DIR",
    "GSETTINGS_SCHEMA_DIR",
    "GI_TYPELIB_PATH",
    "GST_PLUGIN_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_SCANNER",
];

#[cfg(not(windows))]
fn open_in_browser(url: &str) -> Result<(), String> {
    let mut command = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" });
    command.arg(url);
    if let Some(appdir) = std::env::var_os("APPDIR") {
        for variable in APPIMAGE_VARIABLES {
            command.env_remove(variable);
        }
        if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
            let kept: Vec<PathBuf> = std::env::split_paths(&dirs).filter(|dir| !dir.starts_with(&appdir)).collect();
            if let Ok(joined) = std::env::join_paths(kept) {
                command.env("XDG_DATA_DIRS", joined);
            }
        }
    }
    let mut child = command.spawn().map_err(|error| format!("Não foi possível abrir o navegador: {error}"))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Test builds only: the Windows installer reopens the app through the desktop
/// shell, which keeps the arguments but not the environment, so the test passes
/// its report file and data folder as `--e2e-report=` and `--e2e-data=`.
#[cfg(feature = "update-e2e")]
pub fn e2e_arguments() {
    for argument in std::env::args().skip(1) {
        if let Some(path) = argument.strip_prefix("--e2e-report=") {
            std::env::set_var("LOGINSIGHT_E2E_REPORT", path);
        } else if let Some(path) = argument.strip_prefix("--e2e-data=") {
            std::env::set_var("LOGINSIGHT_DATA_DIR", path);
        }
    }
}

/// Test builds only (`--features update-e2e`): runs the whole update flow without
/// asking and appends each step to LOGINSIGHT_E2E_REPORT. Driven by
/// scripts/release/update-e2e.mjs.
#[cfg(feature = "update-e2e")]
pub fn run_e2e(app: AppHandle) {
    let Some(report) = std::env::var_os("LOGINSIGHT_E2E_REPORT").map(PathBuf::from) else { return };
    std::thread::spawn(move || {
        let log = |line: String| {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&report) {
                let _ = writeln!(file, "{line}");
            }
        };
        let started = tauri::async_runtime::block_on(update_startup(app.clone()));
        log(format!(
            "started {} kind={:?} notice={}",
            started.current_version,
            started.install_kind,
            started.notice.as_ref().map_or("none", |notice| notice.kind)
        ));
        let code = (|| {
            if let Some(reason) = started.unavailable {
                log(format!("unavailable {reason}"));
                return 3;
            }
            let Some(available) = &started.available else {
                log(format!("current {}", started.last_check.as_ref().map_or("", |check| check.message.as_str())));
                return 0;
            };
            log(format!("available {}", available.version));
            if let Err(error) = update_download(app.clone()) {
                log(format!("error {error}"));
                return 4;
            }
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let current = update_status(app.clone(), app.state::<UpdateState>());
                match current.phase {
                    Phase::Downloading => continue,
                    Phase::Ready => break,
                    _ => {
                        log(format!("error {}", current.error.unwrap_or_default()));
                        return 5;
                    }
                }
            }
            log("installing".into());
            match tauri::async_runtime::block_on(update_install(app.clone(), None)) {
                Ok(()) => 0,
                Err(error) => {
                    log(format!("error {error}"));
                    6
                }
            }
        })();
        app.exit(code);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_install_retains_pause_but_failure_and_unwind_release_it() {
        let held = Mutex::new(None);
        let pause = crate::operations::pause_for_update().unwrap();
        assert!(install_and_retain_pause(pause, &held, || Err("installer unavailable".into())).is_err());
        assert!(held.lock().is_none());
        assert!(!crate::operations::update_paused());
        let pause = crate::operations::pause_for_update().unwrap();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = install_and_retain_pause(pause, &held, || panic!("controlled installer unwind"));
        }));
        assert!(unwind.is_err());
        assert!(!crate::operations::update_paused());
        let pause = crate::operations::pause_for_update().unwrap();
        install_and_retain_pause(pause, &held, || Ok(())).unwrap();
        assert!(held.lock().is_some());
        assert!(crate::operations::update_paused());
        assert!(crate::operations::token(None).is_err());
        drop(held);
        assert!(!crate::operations::update_paused());
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn notice_reports_the_last_installation() {
        let pending = |to: &str| Some(Pending { from: "0.6.0".into(), to: to.into() });
        let done = notice_for(pending("0.6.1"), Some("0.6.0"), &version("0.6.1"), None).unwrap();
        assert_eq!((done.kind, done.to.as_str()), ("updated", "0.6.1"));
        let failed = notice_for(pending("0.6.1"), Some("0.6.0"), &version("0.6.0"), None).unwrap();
        assert_eq!((failed.kind, failed.to.as_str()), ("failed", "0.6.1"));
        let manual = notice_for(None, Some("0.5.1"), &version("0.6.0"), None).unwrap();
        assert_eq!((manual.kind, manual.from.as_deref()), ("updated", Some("0.5.1")));
        assert!(notice_for(None, None, &version("0.6.0"), None).is_none());
        assert!(notice_for(None, Some("0.6.0"), &version("0.6.0"), None).is_none());
        assert!(notice_for(None, Some("0.7.0"), &version("0.6.0"), None).is_none());
    }

    #[test]
    fn prefs_default_to_checking_on_start() {
        assert!(Prefs::default().check_on_start);
        let prefs: Prefs = serde_json::from_str(r#"{"skippedVersion":"0.7.0"}"#).unwrap();
        assert!(prefs.check_on_start);
        assert_eq!(prefs.skipped_version.as_deref(), Some("0.7.0"));
    }

    #[test]
    fn failed_install_retains_verified_bytes_and_allows_retry() {
        let package = Arc::new(vec![1, 2, 3]);
        let mut inner = Inner {
            prefs: Prefs { pending: Some(Pending { from: "0.9.0".into(), to: "0.10.0".into() }), ..Prefs::default() },
            phase: Phase::Installing, update: None, package: Some(package.clone()),
            downloaded: 3, total: Some(3), error: None, install_on_close: true,
            close_requested: Some(Instant::now()), notice: None, download: None,
        };
        let message = restore_failed_install(&mut inner, "o instalador não iniciou");
        assert_eq!(inner.phase, Phase::Ready);
        assert!(Arc::ptr_eq(inner.package.as_ref().unwrap(), &package));
        assert!(inner.prefs.pending.is_none());
        assert!(!inner.install_on_close);
        assert!(inner.close_requested.is_none());
        assert!(message.contains("Tente novamente"));
        assert!(message.contains("continua aberto"));
    }

    #[test]
    fn pending_install_record_is_atomically_replaced_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        save_prefs(dir.path(), &Prefs::default()).unwrap();
        let mut prefs = Prefs::default();
        prefs.pending = Some(Pending { from: "0.9.0".into(), to: "0.10.0".into() });
        save_prefs(dir.path(), &prefs).unwrap();
        assert_eq!(load_prefs(dir.path()).pending.unwrap().to, "0.10.0");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(save_prefs(&dir.path().join(PREFS_FILE), &prefs).is_err());
        assert_eq!(load_prefs(dir.path()).pending.unwrap().to, "0.10.0");
    }

    #[test]
    fn detects_installations_in_another_profile() {
        let profile = Path::new(r"C:\Users\ana");
        assert!(!in_other_profile(Path::new(r"C:\Users\ana\AppData\Local\LogInsight\loginsight.exe"), profile));
        assert!(!in_other_profile(Path::new(r"c:/users/ANA/AppData/Local/LogInsight/loginsight.exe"), profile));
        assert!(in_other_profile(Path::new(r"C:\Users\admin\AppData\Local\LogInsight\loginsight.exe"), profile));
        assert!(in_other_profile(Path::new(r"C:\Users\anabela\AppData\Local\LogInsight\loginsight.exe"), profile));
        assert!(!in_other_profile(Path::new(r"D:\Apps\LogInsight\loginsight.exe"), profile));
    }

    #[test]
    fn backup_copies_data_files_and_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("cases.json"), b"{}").unwrap();
        std::fs::write(root.join(PREFS_FILE), b"{}").unwrap();
        std::fs::create_dir_all(root.join("sigma/rules")).unwrap();
        std::fs::write(root.join("sigma/rules/a.yml"), b"title: a").unwrap();
        std::fs::create_dir_all(root.join("indexes-v5")).unwrap();
        std::fs::write(root.join("indexes-v5/cache.idx"), b"x").unwrap();
        let target = backup_data(root, Some("0.5.1"), "0.6.0").unwrap().unwrap();
        assert!(target.file_name().unwrap().to_string_lossy().ends_with("_0.5.1_para_0.6.0"));
        assert!(target.join("cases.json").is_file());
        assert!(target.join("sigma/rules/a.yml").is_file());
        assert!(!target.join(PREFS_FILE).exists());
        assert!(!target.join("indexes-v5").exists());

        let backups = root.join(BACKUPS_DIR);
        for day in 1..=4 {
            std::fs::create_dir_all(backups.join(format!("2026010{day}-000000_x_para_y"))).unwrap();
        }
        std::fs::create_dir_all(backups.join("pessoal")).unwrap();
        prune_backups(&backups);
        let mut left: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), BACKUPS_KEPT + 1);
        assert!(left.contains(&"pessoal".to_owned()));
        assert!(!left.contains(&"20260101-000000_x_para_y".to_owned()));
    }

    #[test]
    fn backup_names_only_use_safe_characters() {
        assert_eq!(safe_name("0.6.0-beta.1"), "0.6.0-beta.1");
        assert_eq!(safe_name("../../x"), "....x");
    }
}
