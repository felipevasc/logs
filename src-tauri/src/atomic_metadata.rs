//! Atomic metadata replacement shared by the application and focused I/O tests.
use std::io::{self, Write};
use std::path::Path;

/// tempfile 3.27's Windows persist uses MoveFileExW alone, which rejects an
/// existing target held open even with FILE_SHARE_DELETE. std::fs::rename has
/// a FileRenameInfoEx/POSIX fallback since Rust 1.85 (our MSRV is 1.88).
/// https://github.com/rust-lang/rust/pull/131072
/// Keep Tantivy's same-directory write/flush/sync sequence and cleanup guard;
/// only change metadata replacement. Never delete/truncate the old target.
pub(super) fn atomic_write_metadata(
    path: &Path,
    data: &[u8],
    generation: Option<u64>,
) -> io::Result<()> {
    let check_cancel = || {
        if crate::operations::cancelled_for(generation) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Operação cancelada.",
            ))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Metadata path has no parent directory",
        )
    })?;
    // make_in keeps RAII cleanup, but lets us create a normal file rather than
    // one with FILE_ATTRIBUTE_TEMPORARY that requires changing its attributes
    // before publication. No attributes or permissions are changed here.
    let mut pending = tempfile::Builder::new()
        .prefix(".metadata-")
        .suffix(".pending")
        .make_in(parent, |path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        })?;
    pending.write_all(data)?;
    pending.flush()?;
    pending.as_file().sync_data()?;
    let mut pending = pending.into_temp_path(); // Close the file before rename.
    check_cancel()?;
    std::fs::rename(&pending, path)?;
    // The old temporary pathname no longer belongs to us after a successful
    // rename. On any preceding error, the live guard removes only that path.
    pending.disable_cleanup(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[cfg(windows)]
    use tantivy::directory::{Directory, MmapDirectory};

    #[test]
    fn atomic_metadata_replacement_keeps_open_reader_and_new_bytes_complete() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(".managed.json");
        std::fs::write(&path, b"old-complete").unwrap();
        let mut old_reader = std::fs::File::open(&path).unwrap();
        #[cfg(windows)]
        {
            // Reproduce the original bug: File::open permits shared deletion,
            // but tempfile::persist's MoveFileExW path still returns access denied.
            let directory = MmapDirectory::open(folder.path()).unwrap();
            let error = directory
                .atomic_write(Path::new(".managed.json"), b"discarded")
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(5));
            assert_eq!(std::fs::read(&path).unwrap(), b"old-complete");
        }
        atomic_write_metadata(&path, b"new-complete", None).unwrap();
        let mut old = Vec::new();
        old_reader.read_to_end(&mut old).unwrap();
        assert_eq!(old, b"old-complete");
        assert_eq!(std::fs::read(&path).unwrap(), b"new-complete");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
            assert_eq!(
                std::fs::metadata(&path).unwrap().file_attributes() & FILE_ATTRIBUTE_TEMPORARY,
                0
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn atomic_metadata_replacement_respects_non_shared_delete_and_cleans_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("meta.json");
        std::fs::write(&path, b"old-complete").unwrap();
        // Keep a legitimate handle that expressly forbids replacement.
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&path)
            .unwrap();
        let error = atomic_write_metadata(&path, b"new-complete", None).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32)), "{error:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"old-complete");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
        drop(blocker);
        atomic_write_metadata(&path, b"new-complete", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new-complete");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_metadata_cancellation_preserves_previous_bytes_and_no_pending_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("meta.json");
        std::fs::write(&path, b"old-complete").unwrap();
        let stale = Some(crate::operations::generation().wrapping_sub(1));
        let error = atomic_write_metadata(&path, b"new-complete", stale).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&path).unwrap(), b"old-complete");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }
}
