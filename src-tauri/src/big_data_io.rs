//! Preserve the first write failure before Tantivy replaces a worker's error
//! with its generic WorkerKilled message, and use std's Windows rename semantics
//! for atomic metadata replacement. No retry or additional commit occurs.
#[cfg(any(windows, test))]
#[path = "atomic_metadata.rs"]
mod atomic_metadata;

use std::io::{self, BufWriter, IoSlice, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
use tantivy::directory::{
    AntiCallToken, Directory, DirectoryLock, FileHandle, FileSlice, Lock, MmapDirectory,
    TerminatingWrite, WatchCallback, WatchHandle, WritePtr,
};
use tantivy::schema::Schema;
use tantivy::Index;

#[derive(Clone, Debug)]
pub(super) struct WriteFailure {
    root: Arc<PathBuf>,
    first: Arc<Mutex<Option<String>>>,
}

impl WriteFailure {
    fn capture<T, E: std::fmt::Debug>(
        &self,
        operation: &str,
        path: &Path,
        result: Result<T, E>,
    ) -> Result<T, E> {
        result.map_err(|error| {
            let mut first = self.first.lock().unwrap_or_else(|p| p.into_inner());
            if first.is_none() {
                *first = Some(format!("{operation} {:?}: {error:?}", self.root.join(path)));
            }
            error
        })
    }

    pub(super) fn message(&self, error: impl std::fmt::Display) -> String {
        let first = self.first.lock().unwrap_or_else(|p| p.into_inner());
        match first.as_ref() {
            Some(cause) => format!("{error}; falha de escrita do índice: {cause}"),
            None => error.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
struct DiagnosticDirectory {
    inner: MmapDirectory,
    failure: WriteFailure,
    #[cfg(windows)]
    generation: Option<u64>,
}

pub(super) fn create(root: &Path, schema: Schema) -> Result<(Index, WriteFailure), String> {
    let failure = WriteFailure {
        root: Arc::new(root.to_owned()),
        first: Arc::new(Mutex::new(None)),
    };
    let directory = DiagnosticDirectory {
        inner: MmapDirectory::open(root).map_err(|e| e.to_string())?,
        failure: failure.clone(),
        #[cfg(windows)]
        generation: crate::operations::current_generation(),
    };
    // Preserve create_in_dir's refusal to replace an existing index.
    if Index::exists(&directory).map_err(|e| failure.message(e))? {
        return Err(tantivy::TantivyError::IndexAlreadyExists.to_string());
    }
    let index =
        Index::create(directory, schema, Default::default()).map_err(|e| failure.message(e))?;
    Ok((index, failure))
}

impl Directory for DiagnosticDirectory {
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
        let writer = self
            .failure
            .capture("open_write", path, self.inner.open_write(path))?;
        let capacity = writer.capacity();
        // MmapDirectory returns a fresh, empty BufWriter. Replace its inner
        // writer, keeping the same buffer capacity and number of buffer layers.
        let inner = writer
            .into_inner()
            .map_err(|e| OpenWriteError::wrap_io_error(e.into_error(), path.to_owned()))?;
        Ok(BufWriter::with_capacity(
            capacity,
            Box::new(DiagnosticWriter {
                inner,
                path: path.to_owned(),
                failure: self.failure.clone(),
            }),
        ))
    }
    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.inner.atomic_read(path)
    }
    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        #[cfg(windows)]
        let result = if path == Path::new(".managed.json") || path == Path::new("meta.json") {
            atomic_metadata::atomic_write_metadata(
                &self.failure.root.join(path),
                data,
                self.generation,
            )
        } else {
            self.inner.atomic_write(path, data)
        };
        #[cfg(not(windows))]
        let result = self.inner.atomic_write(path, data);
        self.failure.capture("atomic_write", path, result)
    }
    fn sync_directory(&self) -> io::Result<()> {
        self.failure
            .capture("sync_directory", Path::new(""), self.inner.sync_directory())
    }
    // Missing initial metadata, busy locks and failed garbage collection are
    // expected in some lifecycle paths. Do not let them mask a worker's error.
    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        self.inner.acquire_lock(lock)
    }
    fn watch(&self, callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.inner.watch(callback)
    }
}

struct DiagnosticWriter {
    inner: Box<dyn TerminatingWrite + Send + Sync>,
    path: PathBuf,
    failure: WriteFailure,
}

impl Write for DiagnosticWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.failure
            .capture("write", &self.path, self.inner.write(bytes))
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.failure
            .capture("write_all", &self.path, self.inner.write_all(bytes))
    }
    fn write_vectored(&mut self, bytes: &[IoSlice<'_>]) -> io::Result<usize> {
        self.failure.capture(
            "write_vectored",
            &self.path,
            self.inner.write_vectored(bytes),
        )
    }
    fn flush(&mut self) -> io::Result<()> {
        self.failure
            .capture("flush", &self.path, self.inner.flush())
    }
}

impl TerminatingWrite for DiagnosticWriter {
    fn terminate_ref(&mut self, token: AntiCallToken) -> io::Result<()> {
        self.failure
            .capture("terminate", &self.path, self.inner.terminate_ref(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn worker_error_keeps_original_write_cause_without_committing() {
        let folder = tempfile::tempdir().unwrap();
        let (schema, fields) = super::super::schema();
        let (mut index, failure) = create(folder.path(), schema).unwrap();
        super::super::configure(&mut index).unwrap();
        let writer = index
            .writer_with_num_threads::<tantivy::TantivyDocument>(1, super::super::WRITER_BUDGET)
            .unwrap();
        // The initial missing .managed.json was normal, not a captured failure.
        assert_eq!(failure.message("sentinel"), "sentinel");
        // Inject a portable atomic replacement failure in this private index.
        // No production environment flags, permissions or security changes.
        let managed = folder.path().join(".managed.json");
        std::fs::remove_file(&managed).unwrap();
        std::fs::create_dir(&managed).unwrap();
        let doc = super::super::document(&crate::model::Event::empty(), 0, fields).unwrap();
        writer.add_document(doc.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while failure.first.lock().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "worker did not encounter the injected failure"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // A worker records IO before its status is killed. Feed until that
        // status is visible, without ever calling commit/prepare_commit.
        let error = loop {
            match writer.add_document(doc.clone()) {
                Err(error) => break error,
                Ok(_) => assert!(Instant::now() < deadline, "worker status did not stop"),
            }
        };
        let message = failure.message(error);
        assert!(message.contains("atomic_write"), "{message}");
        assert!(message.contains(".managed.json"), "{message}");
        assert!(message.contains("Os {"), "{message}");
        drop(writer); // Regular drop retains and joins all worker handles.
        drop(index);
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.path().join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["segments"].as_array().unwrap().len(), 0);
        // A later cleanup error must not replace the first original cause.
        let _: io::Result<()> = failure.capture(
            "later",
            Path::new("other"),
            Err(io::Error::other("later failure")),
        );
        assert_eq!(
            failure.message("sentinel").split_once("; ").unwrap().1,
            message.split_once("; ").unwrap().1
        );
    }

    #[test]
    fn directory_diagnostics_preserve_read_write_and_existing_index() {
        let folder = tempfile::tempdir().unwrap();
        let (schema, fields) = super::super::schema();
        let (mut index, failure) = create(folder.path(), schema.clone()).unwrap();
        super::super::configure(&mut index).unwrap();
        let mut writer = index
            .writer_with_num_threads::<tantivy::TantivyDocument>(1, super::super::WRITER_BUDGET)
            .unwrap();
        writer
            .add_document(super::super::document(&crate::model::Event::empty(), 0, fields).unwrap())
            .unwrap();
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        assert_eq!(index.reader().unwrap().searcher().num_docs(), 1);
        assert_eq!(failure.message("sentinel"), "sentinel");
        assert!(create(folder.path(), schema).is_err());
        assert_eq!(index.reader().unwrap().searcher().num_docs(), 1);
    }
}
