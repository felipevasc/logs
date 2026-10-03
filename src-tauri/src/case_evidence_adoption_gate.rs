//! Serialize only first-load recovery preparation for one profile. Callers
//! recheck native readiness after acquisition; the final adoption CAS remains.
use super::*;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    time::Duration,
};
const BUSY: &str = "CASE_ADOPTION_PREPARATION_BUSY: Outra sessão está preparando a investigação.";
const INVALID: &str =
    "CASE_ADOPTION_PREPARATION_CHANGED: A investigação mudou durante a preparação.";
pub(crate) struct AdoptionPreparation {
    _file: File,
    _root: crate::case_recovery::RootLease,
}
/// Hold across readiness recheck -> verified recovery -> adoption only. This
/// guard is never stored in a cache or returned over IPC.
pub(crate) fn adoption_preparation(root: &Path) -> Result<AdoptionPreparation, String> {
    acquire(root, || {})
}
fn acquire(root: &Path, mut waiting: impl FnMut()) -> Result<AdoptionPreparation, String> {
    crate::operations::check()?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let path = root.join("case-adoption-preparation-v1.lock");
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
    loop {
        crate::operations::check()?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => break,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                waiting();
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("{BUSY} {error}")),
        }
    }
    root_guard.validate()?;
    // The persistent lock file is never renamed or cleaned. Detect redirection
    // without reading its byte range through another Windows handle.
    let current = crate::case_archive_format::open_regular(&path)?;
    if crate::sources::file_identity(&current) != crate::sources::file_identity(&file) {
        return Err(INVALID.into());
    }
    crate::operations::check()?;
    Ok(AdoptionPreparation {
        _file: file,
        _root: root_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_specific_preparation_waits_cancellably_and_releases_on_exit() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let held = adoption_preparation(first.path()).unwrap();
        let other = adoption_preparation(second.path()).unwrap();
        drop(other);
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let root = first.path().to_path_buf();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let waiting = std::thread::spawn(move || {
            crate::operations::run_with_token(token, || {
                let mut announced = false;
                acquire(&root, || {
                    if !announced {
                        send.send(()).unwrap();
                        announced = true;
                    }
                })
            })
        });
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(crate::operations::cancel_id(&id));
        assert!(waiting.join().unwrap().is_err());
        drop(held);
        adoption_preparation(first.path()).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn preparation_lock_cannot_be_redirected_to_a_foreign_file() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(
            target.path(),
            root.path().join("case-adoption-preparation-v1.lock"),
        )
        .unwrap();
        assert!(adoption_preparation(root.path()).is_err());
    }
}
