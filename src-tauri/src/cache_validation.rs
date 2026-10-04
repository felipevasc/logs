//! Durable receipts for fully verified immutable cache generations. A receipt
//! replaces payload scans only while EVERY file has the same native identity,
//! size, write time and change time. Unsupported filesystems fail closed.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    native: Vec<i128>,
}
impl Stamp {
    fn of(file: &File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            bytes: metadata.len(),
            modified: metadata.modified().ok()?,
            native: native_stamp(file)?,
        })
    }
}

#[cfg(unix)]
fn native_stamp(file: &File) -> Option<Vec<i128>> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().ok()?;
    Some(vec![
        m.dev().into(),
        m.ino().into(),
        m.ctime().into(),
        m.ctime_nsec().into(),
    ])
}
#[cfg(windows)]
fn native_stamp(file: &File) -> Option<Vec<i128>> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_ID_INFO,
    };
    let mut basic = FILE_BASIC_INFO::default();
    let mut id = FILE_ID_INFO::default();
    // SAFETY: both buffers have exactly the size/type required by their class,
    // and the borrowed handle remains open for both calls.
    unsafe {
        let handle = HANDLE(file.as_raw_handle());
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
        .ok()?;
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
        .ok()?;
    }
    Some(vec![
        id.VolumeSerialNumber.into(),
        i128::from_le_bytes(id.FileId.Identifier),
        basic.CreationTime.into(),
        basic.LastWriteTime.into(),
        basic.ChangeTime.into(),
    ])
}
#[cfg(not(any(unix, windows)))]
fn native_stamp(_: &File) -> Option<Vec<i128>> {
    None
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Generation {
    files: Vec<(PathBuf, Stamp)>,
    // Eligibility belongs to the instant BEFORE validation began. It must not
    // become true just because a lengthy payload scan crosses the clock window.
    #[serde(skip)]
    observed_at: Option<std::time::SystemTime>,
}
impl PartialEq for Generation {
    fn eq(&self, other: &Self) -> bool {
        self.files == other.files
    }
}
impl Eq for Generation {}
#[derive(Serialize, Deserialize)]
struct Receipt {
    version: u32,
    context: String,
    generation: Generation,
    checksum: String,
}
pub(crate) fn receipt_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".verified.json");
    PathBuf::from(name)
}
impl Generation {
    /// Unix inode times may advance only once per filesystem clock tick. Never
    /// promote a just-written generation to a reusable proof: a second write
    /// within that tick could preserve ctime even after restoring mtime.
    pub(crate) fn can_reuse(&self) -> bool {
        #[cfg(unix)]
        {
            let now = std::time::SystemTime::now();
            let Some(observed) = self.observed_at else {
                return false;
            };
            return self.files.iter().all(|(_, stamp)| {
                let seconds = stamp.native.get(2).and_then(|v| u64::try_from(*v).ok());
                let nanos = stamp.native.get(3).and_then(|v| u32::try_from(*v).ok());
                seconds
                    .zip(nanos)
                    .and_then(|(s, n)| {
                        (n < 1_000_000_000)
                            .then(|| {
                                std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(s, n))
                            })
                            .flatten()
                    })
                    .and_then(|changed| {
                        Some(
                            now.duration_since(changed)
                                .ok()?
                                .min(observed.duration_since(changed).ok()?),
                        )
                    })
                    .is_some_and(|age| age >= std::time::Duration::from_secs(2))
            });
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
    pub(crate) fn capture(paths: &[PathBuf]) -> Option<Self> {
        let observed_at = std::time::SystemTime::now();
        let files = paths
            .iter()
            .map(|path| {
                // Never block opening a pipe or follow a substituted symlink.
                if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
                    return None;
                }
                let file = File::open(path).ok()?;
                Some((path.clone(), Stamp::of(&file)?))
            })
            .collect::<Option<Vec<_>>>()?;
        (!files.is_empty()).then_some(Self {
            files,
            observed_at: Some(observed_at),
        })
    }
    pub(crate) fn is_current(&self) -> bool {
        Self::capture(
            &self
                .files
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
        )
        .as_ref()
            == Some(self)
    }
    pub(crate) fn covers(&self, path: &Path, file: &File) -> bool {
        self.files
            .iter()
            .find(|(name, _)| name == path)
            .is_some_and(|(_, stamp)| Stamp::of(file).as_ref() == Some(stamp))
    }
    fn checksum(&self, context: &str) -> Option<String> {
        let bytes = serde_json::to_vec(&(2u32, context, self)).ok()?;
        Some(format!("{:x}", Sha256::digest(bytes)))
    }
    pub(crate) fn verified(&self, path: &Path, context: &str) -> bool {
        if !self.can_reuse() {
            return false;
        }
        let Ok(file) = File::open(receipt_path(path)) else {
            return false;
        };
        if !file.metadata().is_ok_and(|m| m.len() <= 4_000_000) {
            return false;
        }
        // Receipts are small; damaged or oversized JSON cannot allocate freely.
        let Ok(receipt) = serde_json::from_reader::<_, Receipt>(file.take(4_000_001)) else {
            return false;
        };
        receipt.version == 2
            && receipt.context == context
            && receipt.generation == *self
            && self.checksum(context).as_deref() == Some(receipt.checksum.as_str())
    }
    /// Call only AFTER full validation and while publication leases are held.
    /// A persistence failure merely means the next opening verifies again.
    pub(crate) fn remember(&self, path: &Path, context: &str) {
        let result = (|| -> Option<()> {
            if !self.can_reuse() || !self.is_current() {
                return None;
            }
            let receipt = Receipt {
                version: 2,
                context: context.into(),
                generation: self.clone(),
                checksum: self.checksum(context)?,
            };
            let target = receipt_path(path);
            let mut temp = tempfile::NamedTempFile::new_in(target.parent()?).ok()?;
            temp.write_all(&serde_json::to_vec(&receipt).ok()?).ok()?;
            temp.as_file().sync_all().ok()?;
            if !self.is_current() {
                return None;
            }
            temp.persist(target).ok()?;
            Some(())
        })();
        let _ = result;
    }
}

#[cfg(test)]
pub(crate) fn settle_filesystem_clock() {
    #[cfg(unix)]
    std::thread::sleep(std::time::Duration::from_millis(2100));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn recent_inode_times_cannot_publish_reusable_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent");
        std::fs::write(&path, b"new").unwrap();
        let generation = Generation::capture(&[path.clone()]).unwrap();
        assert!(!generation.can_reuse());
        generation.remember(&path, "schema-1");
        assert!(!receipt_path(&path).exists());
        assert!(!generation.verified(&path, "schema-1"));
        settle_filesystem_clock();
        assert!(
            !generation.can_reuse(),
            "a lengthy scan cannot promote a snapshot captured inside the unsafe clock window"
        );
        assert!(Generation::capture(&[path]).unwrap().can_reuse());
    }
    #[test]
    fn durable_receipt_survives_fresh_capture_and_rejects_context_and_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        let marker = dir.path().join("complete");
        std::fs::write(&path, b"payload").unwrap();
        std::fs::write(&marker, b"complete").unwrap();
        settle_filesystem_clock();
        let paths = [path.clone(), marker.clone()];
        Generation::capture(&paths)
            .unwrap()
            .remember(&path, "schema-1");
        let fresh = Generation::capture(&paths).unwrap();
        assert!(fresh.verified(&path, "schema-1"));
        assert!(!fresh.verified(&path, "schema-2"));
        std::fs::write(receipt_path(&path), b"broken").unwrap();
        assert!(!fresh.verified(&path, "schema-1"));
        std::fs::remove_file(&marker).unwrap();
        assert!(Generation::capture(&paths).is_none());
    }
    #[test]
    fn same_size_edits_and_replacements_cannot_reuse_verification_even_with_restored_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        std::fs::write(&path, b"original").unwrap();
        settle_filesystem_clock();
        let paths = [path.clone()];
        let before = Generation::capture(&paths).unwrap();
        before.remember(&path, "schema-1");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, b"modified").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert!(!before.is_current());
        assert!(!Generation::capture(&paths)
            .unwrap()
            .verified(&path, "schema-1"));
        let replacement = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        std::fs::write(replacement.path(), b"original").unwrap();
        replacement.as_file().set_modified(modified).unwrap();
        replacement.persist(&path).unwrap();
        assert!(!Generation::capture(&paths)
            .unwrap()
            .verified(&path, "schema-1"));
    }
}
