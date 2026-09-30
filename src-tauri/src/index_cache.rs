//! Versioned on-disk line indexes. Raw files are never copied into the cache.
use crate::model::LineMeta;
use crate::sources::{self, CompiledTsConfig, CustomParse, FileIndex, FilePart};
use sha2::{Digest, Sha256};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

/// Parser semantics are part of the cache version; older directories are removed.
pub const INDEX_DIR: &str = "indexes-v6";

/// Removes caches from previous parser versions and indexes unused for 30 days.
pub fn prune() {
    let base = crate::config_dir();
    for old in [
        "indexes",
        "indexes-v1",
        "indexes-v2",
        "indexes-v3",
        "indexes-v4",
        "indexes-v5",
    ] {
        let _ = std::fs::remove_dir_all(base.join(old));
    }
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
    // Expanded packages and event-log snapshots are recreated on demand.
    for folder in ["expanded", "snapshots"] {
        let Ok(entries) = std::fs::read_dir(base.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|t| t < cutoff);
            let pending = entry.file_name().to_string_lossy().contains("pending");
            if stale || pending {
                let path = entry.path();
                let _ = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(base.join(INDEX_DIR)) else {
        return;
    };
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
    hash.update(format!("{:?}", sources::file_identity(&file)));
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
            cb(mmap.len(), mmap.len());
        }
        return Ok(FileIndex {
            parts: vec![FilePart {
                path: path.into(),
                physical_path: path.into(),
                physical_file_id: sources::file_identity(&file),
                file_name: PathBuf::from(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                format: header.format,
                custom,
                ts_config: ts,
                header: header.header,
                mmap: std::sync::Arc::new(mmap),
                base: 0,
                identity: id,
            }],
            lines: std::sync::Arc::new(lines),
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

/// Corrected timestamps are cached only after the display path/file name have
/// been restored: timestamp rules may explicitly read `arquivo` or `caminho`.
fn timestamp_key(idx: &FileIndex) -> Option<String> {
    let part = idx.parts.first()?;
    let config = part.ts_config.as_ref()?;
    let mut hash = Sha256::new();
    hash.update(format!(
        "timestamps-v1|{INDEX_DIR}|{}|{}|{}|{}|{}|{}|{:?}",
        part.identity,
        part.path,
        part.file_name,
        config.signature(),
        chrono::Local::now().offset(),
        idx.lines.len(),
        part.physical_file_id
    ));
    Some(format!("{:x}", hash.finalize()))
}

/// Only for a newly constructed, unpublished index. A cancelled streaming
/// restore drops this index; the currently loaded AppState is never mutated.
/// The interactive retimestamp_index path keeps its existing atomic semantics.
pub(crate) fn timestamps_on_load(
    mut idx: FileIndex,
    progress: Option<&(dyn Fn(&str, usize, usize) + Sync)>,
) -> Result<FileIndex, String> {
    let Some(key) = timestamp_key(&idx) else {
        return Ok(idx);
    };
    if idx.parts.len() != 1 {
        sources::retimestamp_index(
            &mut idx,
            Some(&|done, total| {
                if let Some(report) = progress {
                    report("Recalculando data/hora", done, total);
                }
            }),
        )?;
        return Ok(idx);
    }
    let path = crate::config_dir()
        .join(INDEX_DIR)
        .join(format!("timestamps-{key}.bin"));
    timestamps_at(&mut idx, &path, progress)?;
    Ok(idx)
}

fn timestamps_at(
    idx: &mut FileIndex,
    path: &std::path::Path,
    progress: Option<&(dyn Fn(&str, usize, usize) + Sync)>,
) -> Result<bool, String> {
    sources::validate_source(&idx.parts[0])?;
    if let Some(report) = progress {
        report("Verificando cache de data/hora", 0, 0);
    }
    if restore_timestamps(idx, path, progress)? {
        return Ok(true);
    }
    sources::retimestamp_index(
        idx,
        Some(&|done, total| {
            if let Some(report) = progress {
                report("Recalculando data/hora", done, total);
            }
        }),
    )?;
    sources::validate_source(&idx.parts[0])?;
    crate::operations::check()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A cache write failure does not invalidate correctly computed timestamps.
    // Cancellation still aborts the load rather than publishing partial work.
    if let Err(error) = write_timestamps(idx, path, progress) {
        eprintln!("[índice] cache de horários não gravado: {error}");
    }
    crate::operations::check()?;
    sources::validate_source(&idx.parts[0])?;
    Ok(false)
}

fn restore_timestamps(
    idx: &mut FileIndex,
    path: &std::path::Path,
    progress: Option<&(dyn Fn(&str, usize, usize) + Sync)>,
) -> Result<bool, String> {
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(false);
    };
    let count = idx.lines.len();
    let Some(payload_len) = count.checked_mul(8).and_then(|n| n.checked_add(80)) else {
        return Ok(false);
    };
    let Some(expected) = payload_len.checked_add(32) else {
        return Ok(false);
    };
    if !file.metadata().is_ok_and(|m| m.len() == expected as u64) {
        return Ok(false);
    }
    let Ok(mapped) = (unsafe { memmap2::MmapOptions::new().map(&file) }) else {
        return Ok(false);
    };
    if &mapped[..8] != b"LTSO0001"
        || u64::from_le_bytes(mapped[8..16].try_into().unwrap()) != count as u64
    {
        return Ok(false);
    }
    let Some(key) = timestamp_key(idx) else {
        return Ok(false);
    };
    if &mapped[16..80] != key.as_bytes() {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    for bytes in mapped[..payload_len].chunks(1 << 20) {
        crate::operations::check()?;
        hash.update(bytes);
    }
    if hash.finalize().as_slice() != &mapped[payload_len..] {
        return Ok(false);
    }
    let mut last_report = std::time::Instant::now();
    // No additional full-length timestamp vector on a warm open. The mutable
    // index belongs exclusively to this still-unpublished load transaction.
    for (chunk_index, chunk) in std::sync::Arc::make_mut(&mut idx.lines)
        .chunks_mut(8192)
        .enumerate()
    {
        crate::operations::check()?;
        let first = chunk_index * 8192;
        for (i, line) in chunk.iter_mut().enumerate() {
            let offset = 80 + (first + i) * 8;
            line.ts = i64::from_le_bytes(mapped[offset..offset + 8].try_into().unwrap());
        }
        if last_report.elapsed() >= std::time::Duration::from_millis(150) {
            if let Some(report) = progress {
                report(
                    "Reutilizando cache de data/hora",
                    first + chunk.len(),
                    count,
                );
            }
            last_report = std::time::Instant::now();
        }
    }
    sources::validate_source(&idx.parts[0])?;
    crate::operations::check()?;
    idx.time_order.take();
    if let Some(report) = progress {
        report("Reutilizando cache de data/hora", count, count);
    }
    Ok(true)
}

fn write_timestamps(
    idx: &FileIndex,
    path: &std::path::Path,
    progress: Option<&(dyn Fn(&str, usize, usize) + Sync)>,
) -> Result<(), String> {
    if let Some(report) = progress {
        report("Gravando cache de data/hora", 0, idx.lines.len());
    }
    let temporary = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut out = BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(&temporary).map_err(|e| e.to_string())?,
        );
        let mut hash = Sha256::new();
        let mut header = Vec::from(&b"LTSO0001"[..]);
        header.extend_from_slice(&(idx.lines.len() as u64).to_le_bytes());
        header.extend_from_slice(
            timestamp_key(idx)
                .ok_or("Configuração de data/hora ausente.")?
                .as_bytes(),
        );
        out.write_all(&header).map_err(|e| e.to_string())?;
        hash.update(&header);
        let mut bytes = Vec::with_capacity(8192 * 8);
        let mut last_report = std::time::Instant::now();
        for (index, chunk) in idx.lines.chunks(8192).enumerate() {
            crate::operations::check()?;
            bytes.clear();
            for line in chunk {
                bytes.extend_from_slice(&line.ts.to_le_bytes());
            }
            out.write_all(&bytes).map_err(|e| e.to_string())?;
            hash.update(&bytes);
            if last_report.elapsed() >= std::time::Duration::from_millis(150) {
                if let Some(report) = progress {
                    report(
                        "Gravando cache de data/hora",
                        index * 8192 + chunk.len(),
                        idx.lines.len(),
                    );
                }
                last_report = std::time::Instant::now();
            }
        }
        out.write_all(&hash.finalize()).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        out.get_ref().sync_all().map_err(|e| e.to_string())?;
        sources::validate_source(&idx.parts[0])?;
        crate::operations::check()?;
        drop(out);
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod timestamp_overlay_tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, FileIndex) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(
            &path,
            "{\"message\":\"1700000001234\"}\n{\"message\":\"1700000005678\"}\n",
        )
        .unwrap();
        // Use the production compiler: even a rule-free user definition gets
        // the implicit (None, None) extraction rule.
        let config = sources::TsConfig {
            timezone_offset_minutes: Some(0),
            sources: vec!["message".into()],
            format: "epoch_ms".into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let idx =
            sources::index_file(path.to_str().unwrap(), "jsonl", None, Some(config), None).unwrap();
        (dir, idx)
    }
    #[test]
    fn warm_overlay_reuses_corrected_timestamps_and_rejects_corruption() {
        let (dir, mut idx) = fixture();
        let cache = dir.path().join("timestamps.bin");
        assert!(!timestamps_at(&mut idx, &cache, None).unwrap());
        let expected: Vec<_> = idx.lines.iter().map(|m| m.ts).collect();
        assert_eq!(expected, vec![1700000001234, 1700000005678]);
        for line in std::sync::Arc::make_mut(&mut idx.lines) {
            line.ts = 0;
        }
        assert!(timestamps_at(&mut idx, &cache, None).unwrap());
        assert_eq!(idx.lines.iter().map(|m| m.ts).collect::<Vec<_>>(), expected);
        let mut bytes = std::fs::read(&cache).unwrap();
        bytes[80] ^= 1;
        std::fs::write(&cache, bytes).unwrap();
        assert!(!timestamps_at(&mut idx, &cache, None).unwrap());
        assert_eq!(idx.lines.iter().map(|m| m.ts).collect::<Vec<_>>(), expected);
    }
    #[test]
    fn overlay_key_includes_display_path_name_and_configuration() {
        let (_dir, mut idx) = fixture();
        let original = timestamp_key(&idx).unwrap();
        idx.parts[0].path = "virtual.zip!/other.log".into();
        assert_ne!(timestamp_key(&idx).unwrap(), original);
        let path_key = timestamp_key(&idx).unwrap();
        idx.parts[0].file_name = "other-name".into();
        assert_ne!(timestamp_key(&idx).unwrap(), path_key);
        let name_key = timestamp_key(&idx).unwrap();
        idx.parts[0].ts_config.as_mut().unwrap().clock_adjustment_ms = 1000;
        assert_ne!(timestamp_key(&idx).unwrap(), name_key);
    }
    #[test]
    fn cancelled_restore_does_not_modify_published_metadata() {
        let (dir, mut idx) = fixture();
        let cache = dir.path().join("timestamps.bin");
        timestamps_at(&mut idx, &cache, None).unwrap();
        for line in std::sync::Arc::make_mut(&mut idx.lines) {
            line.ts = 0;
        }
        let published = std::sync::Arc::clone(&idx.lines);
        let token = crate::operations::token(Some("timestamp-restore-cancel".into())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            restore_timestamps(
                &mut idx,
                &cache,
                Some(&|_, _, _| {
                    crate::operations::cancel_id("timestamp-restore-cancel");
                }),
            )
        });
        assert!(
            result.is_err(),
            "cancellation prevents publishing the new load"
        );
        assert_eq!(published[0].ts, 0);
        assert_eq!(published[1].ts, 0);
    }
}
