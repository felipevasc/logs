//! Versioned on-disk line indexes. Raw files are never copied into the cache.
use crate::model::LineMeta;
use crate::sources::{self, CompiledTsConfig, CustomParse, FileIndex, FilePart};
use sha2::{Digest, Sha256};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

/// Parser semantics are part of the cache version; older directories are removed.
pub const INDEX_DIR: &str = "indexes-v5";

/// Removes caches from previous parser versions and indexes unused for 30 days.
pub fn prune() {
    let base = crate::config_dir();
    for old in ["indexes", "indexes-v1", "indexes-v2", "indexes-v3", "indexes-v4"] {
        let _ = std::fs::remove_dir_all(base.join(old));
    }
    let Ok(entries) = std::fs::read_dir(base.join(INDEX_DIR)) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .and_then(|m| m.accessed().or_else(|_| m.modified()).ok())
            .is_some_and(|t| t < cutoff);
        let pending = entry.file_name().to_string_lossy().contains(".pending");
        if stale || pending {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn identity(path: &str, bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    h.update(canonical.to_string_lossy().as_bytes());
    h.update((bytes.len() as u64).to_le_bytes());
    if let Ok(m) = std::fs::metadata(path).and_then(|m| m.modified()) {
        h.update(
            m.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_le_bytes(),
        );
    }
    // Content checks complement size and timestamp without a second full scan.
    for start in [0, bytes.len() / 2, bytes.len().saturating_sub(65536)] {
        h.update(&bytes[start..(start + 65536).min(bytes.len())]);
    }
    format!("{:x}", h.finalize())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    format: String,
    columns: Vec<String>,
    header: Vec<String>,
    count: usize,
}

pub fn open(
    path: &str,
    format: &str,
    custom: Option<CustomParse>,
    ts: Option<CompiledTsConfig>,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FileIndex, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() == 0 {
        return Err("Arquivo vazio.".into());
    }
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?;
    let id = identity(path, &mmap);
    let mut hash = Sha256::new();
    hash.update(id.as_bytes());
    hash.update(format.as_bytes());
    if let Some(c) = &custom {
        match c {
            CustomParse::Regex(r) => hash.update(r.as_str()),
            CustomParse::Delimited { sep, fields } => hash.update(format!("{sep}{fields:?}")),
        }
    }
    // Local timezone affects timestamps without a zone.
    hash.update(chrono::Local::now().offset().to_string());
    // Parser semantics are part of the cache version (nested JSON/epoch/arrays).
    let dir = crate::config_dir().join(INDEX_DIR);
    let cache = dir.join(format!("{:x}.idx", hash.finalize()));
    if let Some((header, lines)) = read(&cache, mmap.len()) {
        if let Some(cb) = progress {
            cb(lines.len(), lines.len());
        }
        return Ok(FileIndex {
            parts: vec![FilePart {
                path: path.into(),
                file_name: PathBuf::from(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                format: header.format,
                custom,
                ts_config: ts,
                header: header.header,
                mmap,
                base: 0,
                identity: id,
            }],
            lines,
            columns: header.columns,
            time_order: std::sync::OnceLock::new(),
        });
    }
    drop(mmap);
    let idx = sources::index_file(path, format, custom, ts, progress)?;
    crate::operations::check()?;
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = write(&cache, &idx);
    }
    Ok(idx)
}

fn read(path: &PathBuf, bytes: usize) -> Option<(Header, Vec<LineMeta>)> {
    let mut file = BufReader::new(std::fs::File::open(path).ok()?);
    let mut prefix = [0u8; 12];
    file.read_exact(&mut prefix).ok()?;
    if &prefix[..8] != b"LIDX0003" {
        return None;
    }
    let len = u32::from_le_bytes(prefix[8..12].try_into().ok()?) as usize;
    if len > 4_000_000 {
        return None;
    }
    let mut data = vec![0; len];
    file.read_exact(&mut data).ok()?;
    let head: Header = serde_json::from_slice(&data).ok()?;
    if head.count > bytes || head.count > 500_000_000 {
        return None;
    }
    let expected = 12u64 + len as u64 + head.count as u64 * 27;
    if file.get_ref().metadata().ok()?.len() != expected {
        return None;
    }
    let mut lines = Vec::with_capacity(head.count.min(1_000_000));
    for i in 0..head.count {
        if i % 4096 == 0 && crate::operations::cancelled() {
            return None;
        }
        let mut b = [0u8; 27];
        file.read_exact(&mut b).ok()?;
        let m = LineMeta {
            offset: u64::from_le_bytes(b[0..8].try_into().ok()?),
            len: u32::from_le_bytes(b[8..12].try_into().ok()?),
            ts: i64::from_le_bytes(b[12..20].try_into().ok()?),
            level: b[20],
            code_off: u32::from_le_bytes(b[21..25].try_into().ok()?),
            code_len: u16::from_le_bytes(b[25..27].try_into().ok()?),
        };
        if m.offset.checked_add(m.len as u64)? > bytes as u64 {
            return None;
        }
        lines.push(m);
    }
    Some((head, lines))
}

fn write(path: &PathBuf, idx: &FileIndex) -> std::io::Result<()> {
    let temp = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let mut out = BufWriter::new(std::fs::File::create(&temp)?);
    let data = serde_json::to_vec(&Header {
        format: idx.format.clone(),
        columns: idx.columns.clone(),
        header: idx.header.clone(),
        count: idx.lines.len(),
    })?;
    out.write_all(b"LIDX0003")?;
    out.write_all(&(data.len() as u32).to_le_bytes())?;
    out.write_all(&data)?;
    for (i, m) in idx.lines.iter().enumerate() {
        if i % 4096 == 0 && crate::operations::cancelled() {
            drop(out);
            let _ = std::fs::remove_file(&temp);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Cancelado",
            ));
        }
        out.write_all(&m.offset.to_le_bytes())?;
        out.write_all(&m.len.to_le_bytes())?;
        out.write_all(&m.ts.to_le_bytes())?;
        out.write_all(&[m.level])?;
        out.write_all(&m.code_off.to_le_bytes())?;
        out.write_all(&m.code_len.to_le_bytes())?;
    }
    out.flush()?;
    drop(out);
    std::fs::rename(temp, path)
}
