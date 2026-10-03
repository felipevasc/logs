//! Keep Tantivy's directory behavior, except Windows metadata replacement.
//! Capturing the operation token preserves isolated and parent cancellation
//! even if Tantivy later invokes this directory from another thread.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tantivy::directory::error::{DeleteError, LockError, OpenDirectoryError, OpenReadError, OpenWriteError};
use tantivy::directory::{Directory, DirectoryLock, FileHandle, FileSlice, Lock, MmapDirectory, WatchCallback, WatchHandle, WritePtr};

#[derive(Clone)]
pub(super) struct MetadataDirectory {
    inner: MmapDirectory,
    root: Arc<PathBuf>,
    token: crate::operations::Cancellation,
}

impl std::fmt::Debug for MetadataDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataDirectory").field("inner", &self.inner).finish_non_exhaustive()
    }
}

impl MetadataDirectory {
    pub(super) fn open(root: &Path) -> Result<Self, OpenDirectoryError> {
        Ok(Self {
            inner: MmapDirectory::open(root)?,
            root: Arc::new(root.to_owned()),
            token: crate::operations::current_token(),
        })
    }
}

impl Directory for MetadataDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        self.inner.get_file_handle(path)
    }
    fn open_read(&self, path: &Path) -> Result<FileSlice, OpenReadError> {
        self.inner.open_read(path)
    }
    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        self.inner.exists(path)
    }
    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        self.inner.open_write(path)
    }
    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.inner.atomic_read(path)
    }
    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        if path == Path::new(".managed.json") || path == Path::new("meta.json") {
            super::atomic_metadata::atomic_write_metadata(&self.root.join(path), data, || self.token.cancelled())
        } else {
            self.inner.atomic_write(path, data)
        }
    }
    fn sync_directory(&self) -> io::Result<()> {
        self.inner.sync_directory()
    }
    // The default Directory lock differs from MmapDirectory's native lock.
    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        self.inner.acquire_lock(lock)
    }
    fn watch(&self, callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.inner.watch(callback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn replacement_is_limited_to_metadata_and_keeps_the_captured_cancel_token() {
        let folder = tempfile::tempdir().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let token = crate::operations::current_token().with_stop(stop.clone());
        let directory = crate::operations::with_context(token, || MetadataDirectory::open(folder.path()).unwrap());
        for name in [".managed.json", "meta.json"] {
            directory.atomic_write(Path::new(name), b"old-complete").unwrap();
            let mut old = std::fs::File::open(folder.path().join(name)).unwrap();
            directory.atomic_write(Path::new(name), b"new-complete").unwrap();
            let mut bytes = Vec::new();
            old.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"old-complete");
            assert_eq!(directory.atomic_read(Path::new(name)).unwrap(), b"new-complete");
        }
        let ordinary = Path::new("other.json");
        directory.atomic_write(ordinary, b"ordinary-old").unwrap();
        let _old = std::fs::File::open(folder.path().join(ordinary)).unwrap();
        assert_eq!(directory.atomic_write(ordinary, b"ordinary-new").unwrap_err().raw_os_error(), Some(5));
        assert_eq!(directory.atomic_read(ordinary).unwrap(), b"ordinary-old");
        stop.store(true, Ordering::Relaxed);
        assert_eq!(directory.atomic_write(Path::new("meta.json"), b"cancelled").unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(directory.atomic_read(Path::new("meta.json")).unwrap(), b"new-complete");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 3);
        assert!(crate::operations::check().is_ok());
    }
}
