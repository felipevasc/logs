//! Versioned on-disk line indexes. Raw files are never copied into the cache.
use crate::sources::{self, CompiledTsConfig, CustomParse, FileIndex};
use sha2::{Digest, Sha256};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

/// Parser semantics are part of the cache version; older directories are removed.
pub const INDEX_DIR: &str = "indexes-v6";
/// Storage layout only; changing it must not invalidate identical engine stores.
pub(crate) const METADATA_DIR: &str = "metadata-v1";

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
            if stale {
                let path = entry.path();
                let _ = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
            }
        }
    }
    crate::metadata_checkpoint::prune(&base.join(INDEX_DIR).join(METADATA_DIR), cutoff);
    crate::workspace::prune_canonical_sources();
    // Timestamp overlays are immutable; only aged temporaries are orphaned.
    if let Ok(entries) = std::fs::read_dir(base.join(INDEX_DIR)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("timestamps-") || name.ends_with(".lock") { continue; }
            let stale = entry.metadata().ok().and_then(|m| m.accessed().or_else(|_| m.modified()).ok()).is_some_and(|t| t < cutoff);
            if stale {
                // Stable lock files are never removed. Active mapped readers
                // keep this generation alive on every supported platform.
                if let Ok(_lease) = crate::metadata_store::overlay_lease(&entry.path(), true) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
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

/// Compatibility adapter for callers that only need scanning byte progress.
pub fn open(path: &str, format: &str, custom: Option<CustomParse>, ts: Option<CompiledTsConfig>, progress: Option<&dyn Fn(usize, usize)>) -> Result<FileIndex, String> {
    open_with_progress(path, format, custom, ts, Some(&|p| {
        if p.phase_id == "metadata-scan" { if let Some(cb) = progress { cb(p.completed, p.total); } }
    }))
}

pub(crate) fn open_with_progress(path: &str, format: &str, custom: Option<CustomParse>, ts: Option<CompiledTsConfig>, progress: crate::metadata_checkpoint::Reporter<'_>) -> Result<FileIndex, String> {
    let prepared = sources::prepare_index(path, format, custom, ts)?;
    open_prepared_at(&prepared, &crate::config_dir().join(INDEX_DIR).join(METADATA_DIR), progress)
}

pub(crate) fn open_prepared_at(prepared: &sources::PreparedIndex, dir: &std::path::Path, progress: crate::metadata_checkpoint::Reporter<'_>) -> Result<FileIndex, String> {
    use crate::metadata_checkpoint::{report, Journal, OpenError, Progress, Resume};
    let key = prepared.key()?;
    prepared.validate()?;
    if let Some((lines, mut columns)) = crate::metadata_checkpoint::open_complete(dir, &key, prepared.part.mmap.len(), prepared.initial_cursor(), prepared.multiline(), progress)? {
        prepared.validate()?;
        // Completed journals bypass index_prepared. Refresh only the in-memory
        // supported-field catalog; record framing and persisted bytes stay put.
        crate::java_stacktrace::extend_columns(&prepared.part.format, &mut columns);
        let mut part = prepared.part.clone(); part.metadata_identity = key;
        return Ok(FileIndex { parts: vec![part], lines: std::sync::Arc::new(lines), columns, time_order: std::sync::Arc::new(std::sync::OnceLock::new()) });
    }
    let resumed = std::cell::Cell::new(0usize);
    let committed = std::cell::Cell::new(0usize);
    let parsed = std::cell::Cell::new(0usize);
    let forward = |p: &Progress| {
        resumed.set(resumed.get().max(p.resumed_rows));
        committed.set(committed.get().max(p.checkpoint_rows));
        parsed.set(parsed.get().max(p.parsed_rows));
        let mut p = p.clone(); p.resumed_rows = resumed.get(); p.checkpoint_rows = committed.get(); p.parsed_rows = parsed.get();
        report(progress, p);
    };
    let mut warning = None;
    let (mut journal, resume) = match Journal::open(dir, &key, prepared.part.mmap.len(), prepared.initial_cursor(), prepared.multiline(), Some(&forward)) {
        Ok((journal, resume)) => (Some(journal), resume),
        Err(error @ (OpenError::Cancelled(_) | OpenError::Busy)) => return Err(error.to_string()),
        Err(OpenError::Unavailable(error)) => {
            warning = Some(error);
            (None, Resume { cursor: prepared.initial_cursor(), ..Resume::default() })
        }
    };
    let mut sink = |lines: &mut crate::metadata_store::LineBuilder, cursor, scan_complete, columns: Option<&[String]>| -> Result<(), String> {
        prepared.validate()?;
        if let Some(active) = journal.as_mut() {
            if let Err(error) = active.checkpoint_rows(lines, cursor, scan_complete, columns, Some(&forward), &|| prepared.validate()) {
                crate::operations::check()?;
                // A changed source is fatal even if the persistence operation
                // failed at the same time. Never continue a mixed generation.
                prepared.validate()?;
                warning = Some(error.clone());
                journal = None;
                let mut p = Progress::new("metadata-unavailable", "Metadados sem checkpoint persistente", 0, 0, "");
                p.phase = format!("Metadados sem checkpoint persistente: {error}");
                forward(&p);
            }
        }
        Ok(())
    };
    let mut idx = sources::index_prepared(prepared, resume, &mut sink, Some(&forward))?;
    drop(sink);
    let persisted = journal.is_some();
    drop(journal);
    if persisted {
        // The post-write verification is visible, but these newly written rows
        // are not falsely reported as resumed from a prior load.
        let handoff = |p: &Progress| {
            let mut p = p.clone(); p.resumed_rows = resumed.get(); forward(&p);
        };
        if let Some((lines, _)) = crate::metadata_checkpoint::open_complete(dir, &key, prepared.part.mmap.len(), prepared.initial_cursor(), prepared.multiline(), Some(&handoff))? {
            prepared.validate()?;
            idx.lines = std::sync::Arc::new(lines);
        }
    }
    crate::operations::check()?;
    if let Some(error) = warning {
        eprintln!("[índice] checkpoint de metadados indisponível: {error}");
        let mut p = Progress::new("metadata-unavailable", "Metadados concluídos sem checkpoint persistente", 0, 0, "");
        p.phase = format!("Metadados concluídos sem checkpoint persistente: {error}");
        forward(&p);
    }
    Ok(idx)
}

/// Only timestamp rules reading newly enriched fields (or their query-parameter
/// descendants) depend on this parser revision. Raw/legacy-field rules do not.
fn timestamp_enrichment_revision(format: &str, sources: &[String]) -> Option<&'static str> {
    crate::java_stacktrace::enrichment_signature(format).filter(|_| {
        sources.iter().any(|source| source == "event_ref" || crate::java_stacktrace::COLUMNS.iter().any(|column| {
            source == column
                || source.strip_prefix(column).is_some_and(|tail| tail.starts_with('.'))
        }))
    })
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
    hash.update(b"|raw-metadata-identity:");
    hash.update(part.metadata_identity.as_bytes());
    hash.update(b"|timezone-configuration:");
    hash.update(part.calendar.timezone.as_bytes());
    if matches!(part.format.as_str(), "syslog3164" | "firewall") {
        hash.update(format!("|inferred-year:{}", part.calendar.year));
    }
    if let Some(revision) = timestamp_enrichment_revision(&part.format, &config.sources) {
        hash.update(b"|timestamp-parser-enrichment:");
        hash.update(revision.as_bytes());
    }
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
    let Ok(lease) = crate::metadata_store::overlay_lease(path, false) else { return Ok(false); };
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
    sources::validate_source(&idx.parts[0])?;
    if let Some(report) = progress { report("Reutilizando cache de data/hora", count, count); }
    crate::operations::check()?;
    let timestamps = crate::metadata_store::Timestamps::from_validated_map(mapped, 80, count, Some(lease))?;
    idx.lines = std::sync::Arc::new(idx.lines.with_timestamps(timestamps)?);
    idx.time_order = std::sync::Arc::new(std::sync::OnceLock::new());
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
    let _lease = crate::metadata_store::overlay_lease(path, true)?;
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
        idx.lines = std::sync::Arc::new(idx.lines.iter().map(|mut line| { line.ts = 0; line }).collect::<Vec<_>>().into());
        assert!(timestamps_at(&mut idx, &cache, None).unwrap());
        assert_eq!(idx.lines.iter().map(|m| m.ts).collect::<Vec<_>>(), expected);
        let mut bytes = std::fs::read(&cache).unwrap();
        bytes[80] ^= 1;
        let corrupt = dir.path().join("corrupt.bin");
        std::fs::write(&corrupt, bytes).unwrap();
        // Release this test reader before replacing its cache generation.
        idx.lines = std::sync::Arc::new(idx.lines.iter().collect::<Vec<_>>().into());
        std::fs::rename(corrupt, &cache).unwrap();
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
        idx.lines = std::sync::Arc::new(idx.lines.iter().map(|mut line| { line.ts = 0; line }).collect::<Vec<_>>().into());
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
        assert_eq!(published.at(0).ts, 0);
        assert_eq!(published.at(1).ts, 0);
    }
}

#[cfg(test)]
mod java_warm_catalog_tests {
    use super::*;

    #[test]
    fn old_completed_java_journal_gains_scalar_columns_without_reparse_or_rewrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("java.log");
        let cache = directory.path().join("metadata");
        std::fs::write(&path, "2026-09-30 12:00:00,000 ERROR [worker] a.Logger - failure\n\
a.FailureException: detail\n\tat a.Service.run(A.java:1)\n\
2026-09-30 12:00:01,000 INFO [worker] a.Logger - next\n").unwrap();
        let prepared = sources::prepare_index(path.to_str().unwrap(), "log4j", None, None).unwrap();
        let key = prepared.key().unwrap();
        let (mut journal, resume) = crate::metadata_checkpoint::Journal::open(
            &cache, &key, prepared.part.mmap.len(), prepared.initial_cursor(), prepared.multiline(), None,
        ).unwrap_or_else(|_| panic!("failed to create the tiny completed-journal fixture"));
        let cold = sources::index_prepared(&prepared, resume, &mut |lines, cursor, complete, columns| {
            // Publish a valid completed journal with the pre-enrichment catalog.
            let old = columns.map(|columns| columns.iter().filter(|name| !name.starts_with("java.")).cloned().collect::<Vec<_>>());
            journal.checkpoint_rows(lines, cursor, complete, old.as_deref(), None, &|| prepared.validate())
        }, None).unwrap();
        let expected: Vec<_> = cold.lines.iter().map(|row| (row.offset, row.len, row.ts, row.level, row.code_off, row.code_len)).collect();
        drop(cold);
        drop(journal);
        let (_, old_columns) = crate::metadata_checkpoint::open_complete(
            &cache, &key, prepared.part.mmap.len(), prepared.initial_cursor(), prepared.multiline(), None,
        ).unwrap().unwrap();
        assert!(!old_columns.iter().any(|name| name.starts_with("java.")));
        let immutable = [cache.join(format!("{key}.lines")), cache.join(format!("{key}.state"))];
        let before: Vec<_> = immutable.iter().map(|path| (std::fs::read(path).unwrap(), std::fs::metadata(path).unwrap().modified().unwrap())).collect();
        let warm_prepared = sources::prepare_index(path.to_str().unwrap(), "log4j", None, None).unwrap();
        assert_eq!(warm_prepared.key().unwrap(), key);
        let warm = open_prepared_at(&warm_prepared, &cache, None).unwrap();
        assert_eq!(warm_prepared.parsed_rows.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(warm.parts[0].metadata_identity, key);
        assert_eq!(warm.lines.iter().map(|row| (row.offset, row.len, row.ts, row.level, row.code_off, row.code_len)).collect::<Vec<_>>(), expected);
        for &column in crate::java_stacktrace::COLUMNS {
            assert!(warm.columns.iter().any(|name| name == column));
        }
        assert!(!warm.columns.iter().any(|name| name == "java.trace"));
        for (path, (bytes, modified)) in immutable.iter().zip(before) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
    }
}

#[cfg(test)]
mod java_timestamp_dependency_tests {
    use super::*;

    fn old_key(idx: &FileIndex) -> String {
        let part = &idx.parts[0];
        let config = part.ts_config.as_ref().unwrap();
        let mut hash = Sha256::new();
        hash.update(format!("timestamps-v1|{INDEX_DIR}|{}|{}|{}|{}|{}|{}|{:?}",
            part.identity, part.path, part.file_name, config.signature(),
            chrono::Local::now().offset(), idx.lines.len(), part.physical_file_id));
        hash.update(b"|raw-metadata-identity:"); hash.update(part.metadata_identity.as_bytes());
        hash.update(b"|timezone-configuration:"); hash.update(part.calendar.timezone.as_bytes());
        if matches!(part.format.as_str(), "syslog3164" | "firewall") {
            hash.update(format!("|inferred-year:{}", part.calendar.year));
        }
        format!("{:x}", hash.finalize())
    }

    fn old_overlay(path: &std::path::Path, idx: &FileIndex) {
        let mut bytes = b"LTSO0001".to_vec();
        bytes.extend_from_slice(&(idx.lines.len() as u64).to_le_bytes());
        bytes.extend_from_slice(old_key(idx).as_bytes());
        for row in idx.lines.iter() { bytes.extend_from_slice(&row.ts.to_le_bytes()); }
        let digest = Sha256::digest(&bytes); bytes.extend_from_slice(&digest);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn only_java_rules_reading_new_scalar_fields_change_overlay_keys() {
        for format in ["log4j", "wildfly"] {
            assert!(timestamp_enrichment_revision(format, &["event_ref".into()]).is_some());
            for &column in crate::java_stacktrace::COLUMNS {
                assert_eq!(timestamp_enrichment_revision(format, &[column.into()]), Some("java-trace-v1"));
            }
            assert!(timestamp_enrichment_revision(format, &["java.exception.message.ts".into()]).is_some());
            for column in ["linha", "arquivo", "caminho", "timestamp", "hora_linha", "exception", "stacktrace", "message", "@host", "JAVA.exception.message", "java.unrelated", "java.exception.message_suffix"] {
                assert!(timestamp_enrichment_revision(format, &[column.into()]).is_none(), "{column}");
            }
        }
        let directory = tempfile::tempdir().unwrap();
        for (format, raw) in [
            ("jsonl", "{\"timestamp\":\"2026-09-30T00:00:00Z\",\"message\":\"ordinary\"}\n"),
            ("apache", "127.0.0.1 - - [30/Sep/2026:12:00:00 +0000] \"GET /health HTTP/1.1\" 200 5 \"-\" \"client\"\n"),
        ] {
            let path = directory.path().join(format!("{format}.log")); std::fs::write(&path, raw).unwrap();
            let config = sources::TsConfig { sources: vec!["java.exception.message".into()], format: "epoch_ms".into(), ..Default::default() }.compile().unwrap();
            let idx = sources::index_file(path.to_str().unwrap(), format, None, Some(config), None).unwrap();
            assert_eq!(timestamp_key(&idx).unwrap(), old_key(&idx), "{format}");
        }
    }

    #[test]
    fn java_fields_change_synthesized_event_ref_timestamp_inputs() {
        let raw = b"2026-09-30 12:00:00,000 ERROR [worker] a.Logger - a.FailureException: detail\n\tat a.Service.run(A.java:1)\n";
        let mut current = sources::parse_line_at(raw, "log4j", None, &[], 2026);
        let mut legacy = current.clone();
        legacy.fields.retain(|name, _| !name.starts_with("java."));
        assert!(current.event_ref.is_empty() && legacy.event_ref.is_empty());
        let old_ref = legacy.col_str("event_ref").unwrap();
        assert_ne!(old_ref, current.col_str("event_ref").unwrap());
        let config = sources::TsConfig {
            sources: vec!["event_ref".into()], format: "epoch_ms".into(),
            rules: vec![
                sources::TsRule { regex: Some(format!("^{}$", regex::escape(&old_ref))), template: Some("111".into()) },
                sources::TsRule { regex: None, template: None },
            ], ..Default::default()
        }.compile().unwrap();
        sources::apply_ts_config_event(&mut legacy, &config);
        sources::apply_ts_config_event(&mut current, &config);
        assert_eq!(legacy.timestamp, Some(111));
        assert_ne!(current.timestamp, Some(111), "changed parsed fields can alter a rule over the synthesized reference");
        assert!(timestamp_enrichment_revision("log4j", &config.sources).is_some());
        assert!(timestamp_enrichment_revision("jsonl", &config.sources).is_none());
        assert!(timestamp_enrichment_revision("apache", &config.sources).is_none());
    }

    #[test]
    fn raw_journal_is_reused_but_an_old_java_rule_overlay_is_recomputed() {
        let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("java.log");
        std::fs::write(&path, "2026-09-30 12:00:00,000 ERROR [worker] a.Logger - a.FailureException: 123456\n\tat a.Service.run(A.java:1)\n2026-09-30 12:00:01,000 INFO [worker] a.Logger - next\n").unwrap();
        let config = sources::TsConfig { sources: vec!["java.exception.message".into()], format: "epoch_ms".into(), ..Default::default() }.compile().unwrap();
        let prepared = sources::prepare_index(path.to_str().unwrap(), "log4j", None, Some(config.clone())).unwrap();
        let plain = sources::prepare_index(path.to_str().unwrap(), "log4j", None, None).unwrap();
        assert_eq!(prepared.key().unwrap(), plain.key().unwrap(), "raw metadata is independent of TsConfig");
        let cache = directory.path().join("metadata");
        let idx = open_prepared_at(&prepared, &cache, None).unwrap();
        let raw_time = idx.lines.at(0).ts;
        assert!(raw_time > 1_000_000_000_000, "cold metadata stores the original header timestamp");
        let record_space = crate::analysis_visibility::SourceSet::new(&idx).unwrap().descriptors()[0].record_space.clone();
        let key = prepared.key().unwrap();
        let artifacts = [cache.join(format!("{key}.lines")), cache.join(format!("{key}.state"))];
        let before: Vec<_> = artifacts.iter().map(|p| (std::fs::read(p).unwrap(), std::fs::metadata(p).unwrap().modified().unwrap())).collect();
        let overlay = directory.path().join("old-timestamps.bin"); old_overlay(&overlay, &idx);
        assert_ne!(timestamp_key(&idx).unwrap(), old_key(&idx));
        drop(idx);
        let warmed = sources::prepare_index(path.to_str().unwrap(), "log4j", None, Some(config)).unwrap();
        let mut restored = open_prepared_at(&warmed, &cache, None).unwrap();
        assert_eq!(warmed.parsed_rows.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(restored.lines.at(0).ts, raw_time, "completed journals still contain raw parser time");
        let codes = crate::model::CodesConfig::default();
        let reference = sources::event_at(&restored, 0, &codes, &codes, &[]).event_ref;
        assert!(!timestamps_at(&mut restored, &overlay, None).unwrap(), "the old enrichment-dependent overlay is rejected");
        assert_eq!(restored.lines.at(0).ts, 123456);
        let event = sources::event_at(&restored, 0, &codes, &codes, &[]);
        assert_eq!(event.timestamp, Some(123456)); assert_eq!(event.event_ref, reference);
        assert_eq!(crate::analysis_visibility::SourceSet::new(&restored).unwrap().descriptors()[0].record_space, record_space);
        assert!(timestamps_at(&mut restored, &overlay, None).unwrap(), "the current overlay remains reusable");
        for (path, (bytes, modified)) in artifacts.iter().zip(before) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
    }
}

#[cfg(test)]
mod metadata_timezone_tests {
    use super::*;

    #[test]
    fn timestamp_overlay_key_includes_timezone_rules_not_only_current_offset() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("clock.log");
        std::fs::write(&path, "12:30:00\n").unwrap();
        let config = sources::TsConfig { sources: vec!["linha".into()], format: "%H:%M:%S".into(), ..Default::default() }.compile().unwrap();
        let mut idx = sources::index_file(path.to_str().unwrap(), "text", None, Some(config), None).unwrap();
        let first = timestamp_key(&idx).unwrap();
        idx.parts[0].calendar.timezone = "different-rule-set-at-the-same-current-offset".into();
        assert_ne!(first, timestamp_key(&idx).unwrap());
        assert!(sources::validate_source(&idx.parts[0]).is_err());
        let original = idx.lines.at(0).ts;
        assert!(sources::retimestamp_index(&mut idx, None).is_err());
        assert_eq!(idx.lines.at(0).ts, original, "invalid context must not publish timestamps");
    }

    #[test]
    fn nonmatching_timestamp_rule_cannot_restore_the_previous_inferred_year() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("syslog.log");
        std::fs::write(&path, "Sep 30 12:30:00 host app: message\n").unwrap();
        let config = sources::TsConfig { sources: vec!["missing".into()], format: "epoch_ms".into(), regex: Some("never-matches".into()), ..Default::default() }.compile().unwrap();
        let make = |year| {
            let mut prepared = sources::prepare_index(path.to_str().unwrap(), "syslog3164", None, Some(config.clone())).unwrap();
            prepared.part.calendar.year = year;
            let seed = crate::metadata_checkpoint::Resume { cursor: prepared.initial_cursor(), ..Default::default() };
            sources::index_prepared(&prepared, seed, &mut |_, _, _, _| Ok(()), None).unwrap()
        };
        let mut first = make(2031); let cache = dir.path().join("timestamps.bin");
        assert!(!timestamps_at(&mut first, &cache, None).unwrap());
        let mut next = make(2032); assert_ne!(timestamp_key(&first), timestamp_key(&next));
        assert!(!timestamps_at(&mut next, &cache, None).unwrap());
        assert_ne!(first.lines.at(0).ts, next.lines.at(0).ts);
        let codes = crate::model::CodesConfig::default();
        assert_eq!(Some(next.lines.at(0).ts), sources::event_at(&next, 0, &codes, &codes, &[]).timestamp);
        let mut warm = make(2032); assert!(timestamps_at(&mut warm, &cache, None).unwrap());
        assert_eq!(next.lines.at(0).ts, warm.lines.at(0).ts);
    }


    #[test]
    fn overlay_rejects_a_different_base_parser_with_the_same_row_count() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("parser.log");
        // Logfmt requires at least three key=value pairs; a two-pair fixture falls back to text.
        std::fs::write(&path, "ts=2026-09-30T12:00:00Z level=info msg=hello\n").unwrap();
        let config = sources::TsConfig { sources: vec!["missing".into()], format: "epoch_ms".into(), regex: Some("never-matches".into()), ..Default::default() }.compile().unwrap();
        let mut logfmt = sources::index_file(path.to_str().unwrap(), "logfmt", None, Some(config.clone()), None).unwrap();
        let cache = dir.path().join("timestamps.bin"); assert!(!timestamps_at(&mut logfmt, &cache, None).unwrap());
        let mut text = sources::index_file(path.to_str().unwrap(), "text", None, Some(config.clone()), None).unwrap();
        assert_eq!(text.lines.len(), logfmt.lines.len()); assert_ne!(timestamp_key(&logfmt), timestamp_key(&text));
        assert!(!timestamps_at(&mut text, &cache, None).unwrap()); assert_eq!(text.lines.at(0).ts, 0); assert_ne!(logfmt.lines.at(0).ts, 0);
        let with_time = sources::CustomParse::Regex(regex::Regex::new(r"ts=(?P<timestamp>\S+) level=\S+ msg=(?P<message>.*)").unwrap());
        let without_time = sources::CustomParse::Regex(regex::Regex::new(r"(?P<message>.*)").unwrap());
        let mut first = sources::index_file(path.to_str().unwrap(), "custom", Some(with_time), Some(config.clone()), None).unwrap();
        assert!(!timestamps_at(&mut first, &cache, None).unwrap());
        assert_ne!(first.lines.at(0).ts, 0, "the timestamp-bearing custom parser must exercise overlay fallback");
        let mut second = sources::index_file(path.to_str().unwrap(), "custom", Some(without_time), Some(config), None).unwrap();
        assert_ne!(timestamp_key(&first), timestamp_key(&second));
        assert!(!timestamps_at(&mut second, &cache, None).unwrap()); assert_eq!(second.lines.at(0).ts, 0);
    }

}
