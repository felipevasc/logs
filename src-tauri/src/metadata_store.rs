//! Immutable line metadata views. Portable 27-byte records are decoded by value;
//! no references to unaligned integers or native-layout casts are ever created.
//! A mapped journal retains its shared writer/pruner lease for its whole lifetime.
use crate::model::LineMeta;
use std::{
    fs::File,
    io::{BufWriter, Write},
    ops::Range,
    sync::Arc,
};

pub(crate) const RECORD_BYTES: usize = 27;

#[inline]
pub(crate) fn decode_record(b: &[u8]) -> LineMeta {
    debug_assert_eq!(b.len(), RECORD_BYTES);
    LineMeta {
        offset: u64::from_le_bytes(b[0..8].try_into().unwrap()),
        len: u32::from_le_bytes(b[8..12].try_into().unwrap()),
        ts: i64::from_le_bytes(b[12..20].try_into().unwrap()),
        level: b[20],
        code_off: u32::from_le_bytes(b[21..25].try_into().unwrap()),
        code_len: u16::from_le_bytes(b[25..27].try_into().unwrap()),
    }
}

struct Mapping {
    // Drop the mapping before its lease / private temporary-file handle.
    bytes: memmap2::Mmap,
    _lease: Option<Arc<File>>,
    _private_file: Option<File>,
}

enum Records {
    Owned(Vec<LineMeta>),
    Packed {
        map: Arc<Mapping>,
        byte_start: usize,
    },
}
impl Records {
    #[inline]
    fn at(&self, row: usize) -> LineMeta {
        match self {
            Self::Owned(rows) => rows[row],
            Self::Packed { map, byte_start } => {
                let start = byte_start + row * RECORD_BYTES;
                decode_record(&map.bytes[start..start + RECORD_BYTES])
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Timestamps {
    map: Arc<Mapping>,
    byte_start: usize,
    len: usize,
}
impl Timestamps {
    /// The caller already checked the immutable overlay's key and checksum.
    pub(crate) fn from_validated_map(
        map: memmap2::Mmap,
        byte_start: usize,
        len: usize,
        lease: Option<Arc<File>>,
    ) -> Result<Self, String> {
        let end = len
            .checked_mul(8)
            .and_then(|n| n.checked_add(byte_start))
            .ok_or("Tamanho de timestamps inválido.")?;
        if end > map.len() {
            return Err("Timestamps truncados.".into());
        }
        Ok(Self {
            map: Arc::new(Mapping {
                bytes: map,
                _lease: lease,
                _private_file: None,
            }),
            byte_start,
            len,
        })
    }
    #[inline]
    fn at(&self, row: usize) -> i64 {
        debug_assert!(row < self.len);
        let start = self.byte_start + row * 8;
        i64::from_le_bytes(self.map.bytes[start..start + 8].try_into().unwrap())
    }
}

#[derive(Clone)]
struct Span {
    records: Arc<Records>,
    first: usize,
    len: usize,
    offset_delta: u64,
    timestamps: Option<(Timestamps, usize)>,
}
impl Span {
    #[inline]
    fn at(&self, row: usize) -> LineMeta {
        let mut m = self.records.at(self.first + row);
        m.offset += self.offset_delta;
        if let Some((timestamps, first)) = &self.timestamps {
            m.ts = timestamps.at(first + row);
        }
        m
    }
}

/// Cloning or slicing copies span descriptors, never per-row metadata. Reads
/// return values so native alignment and endianness do not constrain storage.
#[derive(Clone, Default)]
pub struct LineStore {
    spans: Vec<Span>,
    ends: Vec<usize>,
    len: usize,
}
impl From<Vec<LineMeta>> for LineStore {
    fn from(rows: Vec<LineMeta>) -> Self {
        let len = rows.len();
        if len == 0 {
            return Self::default();
        }
        Self {
            spans: vec![Span {
                records: Arc::new(Records::Owned(rows)),
                first: 0,
                len,
                offset_delta: 0,
                timestamps: None,
            }],
            ends: vec![len],
            len,
        }
    }
}
impl LineStore {
    /// The mapping has passed journal identity, full checksum and record-bound
    /// validation while holding `lease`. Writers and pruning use its exclusive
    /// counterpart; they must never unlink the stable lock file.
    pub(crate) fn from_validated_journal(
        map: memmap2::Mmap,
        lease: Arc<File>,
        byte_start: usize,
        len: usize,
    ) -> Result<Self, String> {
        let end = len
            .checked_mul(RECORD_BYTES)
            .and_then(|n| n.checked_add(byte_start))
            .ok_or("Tamanho de metadados inválido.")?;
        if end > map.len() {
            return Err("Metadados truncados.".into());
        }
        if len == 0 {
            return Ok(Self::default());
        }
        let records = Arc::new(Records::Packed {
            map: Arc::new(Mapping {
                bytes: map,
                _lease: Some(lease),
                _private_file: None,
            }),
            byte_start,
        });
        Ok(Self {
            spans: vec![Span {
                records,
                first: 0,
                len,
                offset_delta: 0,
                timestamps: None,
            }],
            ends: vec![len],
            len,
        })
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline]
    pub fn get(&self, row: usize) -> Option<LineMeta> {
        if row >= self.len {
            return None;
        }
        let span = if self.spans.len() == 1 {
            0
        } else {
            self.ends.partition_point(|&end| end <= row)
        };
        let base = if span == 0 { 0 } else { self.ends[span - 1] };
        Some(self.spans[span].at(row - base))
    }
    #[inline]
    pub fn at(&self, row: usize) -> LineMeta {
        self.get(row).expect("line metadata index out of bounds")
    }
    pub fn first(&self) -> Option<LineMeta> {
        self.get(0)
    }
    pub fn last(&self) -> Option<LineMeta> {
        self.len.checked_sub(1).and_then(|row| self.get(row))
    }
    pub fn iter(&self) -> LineIter<'_> {
        self.range(0..self.len).iter()
    }
    pub fn range(&self, range: Range<usize>) -> LineRange<'_> {
        assert!(range.start <= range.end && range.end <= self.len);
        LineRange {
            store: self,
            start: range.start,
            end: range.end,
        }
    }
    pub fn partition_point(&self, pred: impl FnMut(LineMeta) -> bool) -> usize {
        self.range(0..self.len).partition_point(pred)
    }
    pub fn chunks(&self, size: usize) -> LineChunks<'_> {
        assert!(size > 0);
        LineChunks {
            store: self,
            next: 0,
            size,
        }
    }
    pub fn slice(&self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len);
        let mut out = Self::default();
        let mut start = 0;
        for span in &self.spans {
            let end = start + span.len;
            let from = range.start.max(start);
            let to = range.end.min(end);
            if from < to {
                let mut view = span.clone();
                let skip = from - start;
                view.first += skip;
                view.len = to - from;
                if let Some((_, first)) = &mut view.timestamps {
                    *first += skip;
                }
                out.push_span(view);
            }
            start = end;
            if end >= range.end {
                break;
            }
        }
        out
    }
    fn push_span(&mut self, span: Span) {
        self.len += span.len;
        self.ends.push(self.len);
        self.spans.push(span);
    }
    /// Lazy relocation preserves offsets and saved event IDs without rewriting
    /// either source's metadata or timestamp overlay.
    pub fn append_relocated(&mut self, other: &Self, offset_delta: u64) -> Result<(), String> {
        self.len
            .checked_add(other.len)
            .ok_or("Quantidade de metadados excedida.")?;
        let mut spans = Vec::with_capacity(other.spans.len());
        for span in &other.spans {
            let mut next = span.clone();
            next.offset_delta = next
                .offset_delta
                .checked_add(offset_delta)
                .ok_or("Posição de fonte excedida.")?;
            let maximum = match &*span.records {
                Records::Owned(rows) => rows[span.first..span.first + span.len]
                    .iter()
                    .map(|m| m.offset)
                    .max(),
                // Journal validation proves physical offsets are ordered.
                Records::Packed { .. } => span
                    .len
                    .checked_sub(1)
                    .map(|last| span.records.at(span.first + last).offset),
            };
            if let Some(maximum) = maximum {
                maximum
                    .checked_add(next.offset_delta)
                    .ok_or("Posição de fonte excedida.")?;
            }
            spans.push(next);
        }
        // All checks happen before changing the destination, even outside the
        // caller's usual staged-import transaction.
        for span in spans {
            self.push_span(span);
        }
        Ok(())
    }
    pub(crate) fn with_timestamps(&self, timestamps: Timestamps) -> Result<Self, String> {
        if timestamps.len != self.len {
            return Err("Quantidade de timestamps incompatível.".into());
        }
        let mut out = self.clone();
        let mut first = 0;
        for span in &mut out.spans {
            span.timestamps = Some((timestamps.clone(), first));
            first += span.len;
        }
        Ok(out)
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn resident_rows(&self) -> usize {
        self.spans
            .iter()
            .filter(|s| matches!(&*s.records, Records::Owned(_)))
            .map(|s| s.len)
            .sum()
    }
}

#[derive(Clone, Copy)]
pub struct LineRange<'a> {
    store: &'a LineStore,
    start: usize,
    end: usize,
}
impl<'a> LineRange<'a> {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
    pub fn get(&self, row: usize) -> Option<LineMeta> {
        (row < self.len()).then(|| self.store.at(self.start + row))
    }
    pub fn at(&self, row: usize) -> LineMeta {
        self.get(row)
            .expect("line metadata range index out of bounds")
    }
    pub fn first(&self) -> Option<LineMeta> {
        self.get(0)
    }
    pub fn last(&self) -> Option<LineMeta> {
        self.len().checked_sub(1).and_then(|row| self.get(row))
    }
    pub fn iter(self) -> LineIter<'a> {
        LineIter {
            store: self.store,
            next: self.start,
            end: self.end,
        }
    }
    pub fn partition_point(&self, mut pred: impl FnMut(LineMeta) -> bool) -> usize {
        let (mut left, mut right) = (0, self.len());
        while left < right {
            let mid = left + (right - left) / 2;
            if pred(self.at(mid)) {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        left
    }
    pub fn to_store(self) -> LineStore {
        self.store.slice(self.start..self.end)
    }
}
pub struct LineIter<'a> {
    store: &'a LineStore,
    next: usize,
    end: usize,
}
impl Iterator for LineIter<'_> {
    type Item = LineMeta;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.end {
            return None;
        }
        let row = self.store.at(self.next);
        self.next += 1;
        Some(row)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.end - self.next;
        (len, Some(len))
    }
}
impl DoubleEndedIterator for LineIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.next == self.end {
            return None;
        }
        self.end -= 1;
        Some(self.store.at(self.end))
    }
}
impl ExactSizeIterator for LineIter<'_> {}
impl std::iter::FusedIterator for LineIter<'_> {}
impl<'a> IntoIterator for LineRange<'a> {
    type Item = LineMeta;
    type IntoIter = LineIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<'a> IntoIterator for &'a LineStore {
    type Item = LineMeta;
    type IntoIter = LineIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
pub struct LineChunks<'a> {
    store: &'a LineStore,
    next: usize,
    size: usize,
}
impl<'a> Iterator for LineChunks<'a> {
    type Item = LineRange<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.store.len {
            return None;
        }
        let end = self.next.saturating_add(self.size).min(self.store.len);
        let view = self.store.range(self.next..end);
        self.next = end;
        Some(view)
    }
}

/// Use the application data filesystem, not the OS temp directory which may
/// reside on tmpfs and turn a disk-backed overlay into unreclaimable RAM.
fn scratch_file() -> Result<File, String> {
    let dir = crate::config_dir()
        .join(crate::index_cache::INDEX_DIR)
        .join("metadata-scratch");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tempfile::tempfile_in(dir).map_err(|e| e.to_string())
}

pub(crate) fn overlay_lease(path: &std::path::Path, exclusive: bool) -> Result<Arc<File>, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open({
            // A pending overlay belongs to the same stable lock as its final
            // generation, so pruning cannot unlink an active temporary writer.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("timestamps-") {
                path.with_file_name(format!("{}.lock", name.split('.').next().unwrap_or(name)))
            } else {
                path.with_extension("lock")
            }
        })
        .map_err(|e| e.to_string())?;
    if exclusive {
        fs2::FileExt::try_lock_exclusive(&lock)
    } else {
        fs2::FileExt::try_lock_shared(&lock)
    }
    .map_err(|e| format!("Cache de timestamps em uso: {e}"))?;
    Ok(Arc::new(lock))
}
/// Append-only private spool used while parsing/restoring a checkpoint. Only
/// the latest logical record remains mutable; all earlier rows live on disk.
/// This is temporary storage, not the durable commit point (the journal is).
#[derive(Default)]
pub(crate) struct LineBuilder {
    file: Option<BufWriter<File>>,
    sealed: usize,
    tail: Option<LineMeta>,
}
impl LineBuilder {
    pub(crate) fn len(&self) -> usize {
        self.sealed + usize::from(self.tail.is_some())
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn last_mut(&mut self) -> Option<&mut LineMeta> {
        self.tail.as_mut()
    }
    fn write_row(&mut self, row: LineMeta) -> Result<(), String> {
        let file = match &mut self.file {
            Some(file) => file,
            slot @ None => slot.insert(BufWriter::with_capacity(256 << 10, scratch_file()?)),
        };
        let mut bytes = [0u8; RECORD_BYTES];
        bytes[..8].copy_from_slice(&row.offset.to_le_bytes());
        bytes[8..12].copy_from_slice(&row.len.to_le_bytes());
        bytes[12..20].copy_from_slice(&row.ts.to_le_bytes());
        bytes[20] = row.level;
        bytes[21..25].copy_from_slice(&row.code_off.to_le_bytes());
        bytes[25..].copy_from_slice(&row.code_len.to_le_bytes());
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.sealed = self
            .sealed
            .checked_add(1)
            .ok_or("Quantidade de metadados excedida.")?;
        Ok(())
    }
    pub(crate) fn push(&mut self, row: LineMeta) -> Result<(), String> {
        if let Some(previous) = self.tail.take() {
            self.write_row(previous)?;
        }
        self.tail = Some(row);
        Ok(())
    }
    pub(crate) fn extend(
        &mut self,
        rows: impl IntoIterator<Item = LineMeta>,
    ) -> Result<(), String> {
        for (n, row) in rows.into_iter().enumerate() {
            if n % 8192 == 0 {
                crate::operations::check()?;
            }
            self.push(row)?;
        }
        Ok(())
    }
    pub(crate) fn flush(&mut self) -> Result<(), String> {
        if let Some(file) = &mut self.file {
            file.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(crate) fn at(&self, row: usize) -> Result<LineMeta, String> {
        if row == self.sealed {
            return self.tail.ok_or_else(|| "Metadado fora da fonte.".into());
        }
        if row > self.sealed {
            return Err("Metadado fora da fonte.".into());
        }
        let file = self
            .file
            .as_ref()
            .ok_or("Metadados temporários ausentes.")?;
        if !file.buffer().is_empty() {
            return Err("Metadados temporários não foram sincronizados para leitura.".into());
        }
        let mut bytes = [0; RECORD_BYTES];
        read_at(file.get_ref(), &mut bytes, row as u64 * RECORD_BYTES as u64)?;
        Ok(decode_record(&bytes))
    }
    /// Checkpoint transfer stays bounded to 8192 rows regardless of source size.
    pub(crate) fn read_chunk(&self, range: Range<usize>) -> Result<Vec<LineMeta>, String> {
        if range.start > range.end || range.end > self.len() || range.len() > 8192 {
            return Err("Lote de metadados inválido.".into());
        }
        let disk_end = range.end.min(self.sealed);
        let disk_count = disk_end.saturating_sub(range.start);
        let mut bytes = vec![0; disk_count * RECORD_BYTES];
        if disk_count > 0 {
            let file = self
                .file
                .as_ref()
                .ok_or("Metadados temporários ausentes.")?;
            if !file.buffer().is_empty() {
                return Err("Metadados temporários não foram sincronizados para leitura.".into());
            }
            read_at(
                file.get_ref(),
                &mut bytes,
                range.start as u64 * RECORD_BYTES as u64,
            )?;
        }
        let mut rows: Vec<_> = bytes
            .chunks_exact(RECORD_BYTES)
            .map(decode_record)
            .collect();
        if range.end > self.sealed {
            rows.push(self.tail.ok_or("Cauda de metadados ausente.")?);
        }
        Ok(rows)
    }
    pub(crate) fn finish(mut self) -> Result<LineStore, String> {
        if let Some(tail) = self.tail.take() {
            self.write_row(tail)?;
        }
        self.flush()?;
        let Some(file) = self.file.take() else {
            return Ok(LineStore::default());
        };
        let file = file.into_inner().map_err(|e| e.to_string())?;
        // SAFETY: consumed private spool, no other write handle escapes. Its
        // only owner moves into Mapping, whose bytes are always read-only.
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?;
        let records = Arc::new(Records::Packed {
            map: Arc::new(Mapping {
                bytes,
                _lease: None,
                _private_file: Some(file),
            }),
            byte_start: 0,
        });
        Ok(LineStore {
            spans: vec![Span {
                records,
                first: 0,
                len: self.sealed,
                offset_delta: 0,
                timestamps: None,
            }],
            ends: vec![self.sealed],
            len: self.sealed,
        })
    }
}

fn read_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> Result<(), String> {
    while !bytes.is_empty() {
        #[cfg(unix)]
        let result = std::os::unix::fs::FileExt::read_at(file, bytes, offset);
        #[cfg(windows)]
        let result = std::os::windows::fs::FileExt::seek_read(file, bytes, offset);
        #[cfg(not(any(unix, windows)))]
        let result: std::io::Result<usize> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Leitura posicional indisponível.",
        ));
        match result {
            Ok(0) => return Err("Metadados temporários truncados.".into()),
            Ok(n) => {
                offset += n as u64;
                bytes = &mut bytes[n..];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Transaction-local timestamp spool. Bounded writes and immutable publication
/// replace the previous all-row Vec and copy-on-write LineMeta allocation.
pub(crate) struct TimestampWriter {
    file: BufWriter<File>,
    len: usize,
}
impl TimestampWriter {
    pub(crate) fn new() -> Result<Self, String> {
        Ok(Self {
            file: BufWriter::with_capacity(64 << 10, scratch_file()?),
            len: 0,
        })
    }
    pub(crate) fn push(&mut self, ts: i64) -> Result<(), String> {
        self.file
            .write_all(&ts.to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.len += 1;
        Ok(())
    }
    pub(crate) fn finish(mut self) -> Result<Timestamps, String> {
        self.file.flush().map_err(|e| e.to_string())?;
        let file = self.file.into_inner().map_err(|e| e.to_string())?;
        // Empty overlays need a nonempty mapping, but still expose zero rows.
        if self.len == 0 {
            file.set_len(8).map_err(|e| e.to_string())?;
        }
        // SAFETY: this private tempfile has no shared writers. Ownership of its
        // only file handle is moved into the immutable mapping and never exposed.
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?;
        Ok(Timestamps {
            map: Arc::new(Mapping {
                bytes,
                _lease: None,
                _private_file: Some(file),
            }),
            byte_start: 0,
            len: self.len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(i: u64) -> LineMeta {
        LineMeta {
            offset: i * 101,
            len: 100,
            ts: i as i64 - 10,
            level: 4,
            code_off: 2,
            code_len: 5,
        }
    }
    fn fields(m: LineMeta) -> (u64, u32, i64, u8, u32, u16) {
        (m.offset, m.len, m.ts, m.level, m.code_off, m.code_len)
    }
    #[test]
    fn unaligned_little_endian_codec_preserves_every_field() {
        let mut bytes = vec![255];
        for m in [
            row(0),
            row(1),
            LineMeta {
                offset: u64::MAX,
                ts: i64::MIN,
                len: u32::MAX,
                level: u8::MAX,
                code_off: u32::MAX,
                code_len: u16::MAX,
            },
        ] {
            crate::metadata_checkpoint::encode_record(&m, &mut bytes);
            assert_eq!(fields(decode_record(&bytes[bytes.len() - 27..])), fields(m));
        }
    }
    #[test]
    fn views_and_lazy_relocation_do_not_mutate_shared_source() {
        let a = LineStore::from((0..6).map(row).collect::<Vec<_>>());
        let view = a.slice(2..5);
        let mut joined = a.slice(0..2);
        joined.append_relocated(&view, 10_000).unwrap();
        assert_eq!(joined.len(), 5);
        assert_eq!(joined.at(2).offset, 10_202);
        assert_eq!(a.at(2).offset, 202);
        assert_eq!(joined.partition_point(|m| m.offset < 10_000), 2);
        assert_eq!(
            joined
                .range(1..4)
                .iter()
                .map(|m| m.offset)
                .collect::<Vec<_>>(),
            vec![101, 10202, 10303]
        );
        assert_eq!(
            joined.chunks(2).map(|r| r.len()).collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
        assert_eq!(joined.iter().rev().next().unwrap().offset, 10404);
        assert!(joined.get(5).is_none());
    }
    #[test]
    fn timestamp_spools_are_immutable_and_follow_slices_and_appends() {
        let original = LineStore::from((0..5).map(row).collect::<Vec<_>>());
        let mut writer = TimestampWriter::new().unwrap();
        for ts in 100..105 {
            writer.push(ts).unwrap();
        }
        let overlay = original.with_timestamps(writer.finish().unwrap()).unwrap();
        assert_eq!(original.at(0).ts, -10);
        assert_eq!(overlay.at(0).ts, 100);
        let mut joined = overlay.slice(2..4);
        joined
            .append_relocated(&original.slice(0..1), 1000)
            .unwrap();
        assert_eq!(
            joined.iter().map(|m| m.ts).collect::<Vec<_>>(),
            vec![102, 103, -10]
        );
    }
    #[test]
    fn packed_maps_keep_reader_lease_through_slices_and_clones() {
        use fs2::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.lines");
        let lock_path = dir.path().join("rows.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        FileExt::try_lock_shared(&lock).unwrap();
        let mut bytes = vec![0; 72];
        for i in 0..17 {
            crate::metadata_checkpoint::encode_record(&row(i), &mut bytes);
        }
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        // Test publication is complete and no handle mutates the mapped file.
        let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
        let store = LineStore::from_validated_journal(map, Arc::new(lock), 72, 17).unwrap();
        assert_eq!(store.resident_rows(), 0);
        for i in 0..17 {
            assert_eq!(fields(store.at(i)), fields(row(i as u64)));
        }
        let view = store.slice(3..12);
        drop(store);
        let prune = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(FileExt::try_lock_exclusive(&prune).is_err());
        assert_eq!(fields(view.at(0)), fields(row(3)));
        drop(view);
        FileExt::try_lock_exclusive(&prune).unwrap();
    }
    #[test]
    fn mapped_constructor_rejects_truncation_and_count_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows");
        std::fs::write(&path, [0; 72]).unwrap();
        for count in [1, usize::MAX] {
            let file = File::open(&path).unwrap();
            let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
            assert!(LineStore::from_validated_journal(map, Arc::new(file), 72, count).is_err());
        }
    }
    #[test]
    fn cold_builder_keeps_one_mutable_tail_and_publishes_only_packed_rows() {
        let mut rows = LineBuilder::default();
        rows.extend((0..20_003).map(row)).unwrap();
        rows.last_mut().unwrap().len = 333;
        rows.flush().unwrap();
        assert_eq!(rows.len(), 20_003);
        assert_eq!(rows.sealed, 20_002);
        assert_eq!(rows.at(8192).unwrap().offset, 8192 * 101);
        assert_eq!(rows.read_chunk(20_000..20_003).unwrap()[2].len, 333);
        assert!(
            rows.read_chunk(0..8193).is_err(),
            "checkpoint reads must remain bounded"
        );
        let published = rows.finish().unwrap();
        assert_eq!(published.resident_rows(), 0);
        assert_eq!(published.len(), 20_003);
        assert_eq!(published.last().unwrap().len, 333);
    }
    #[test]
    fn overflowing_append_leaves_existing_views_unchanged() {
        let mut destination = LineStore::from(vec![row(1)]);
        let invalid = LineStore::from(vec![
            LineMeta {
                offset: u64::MAX,
                ..row(0)
            },
            row(0),
        ]);
        assert!(destination.append_relocated(&invalid, 1).is_err());
        assert_eq!(destination.len(), 1);
        assert_eq!(destination.at(0).offset, 101);
    }
    #[test]
    fn failed_private_spool_never_returns_a_mapped_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly");
        std::fs::write(&path, b"seed").unwrap();
        let file = File::open(&path).unwrap();
        let mut rows = LineBuilder {
            file: Some(BufWriter::new(file)),
            sealed: 0,
            tail: None,
        };
        rows.push(row(0)).unwrap();
        assert!(rows.finish().is_err());
        let file = File::open(&path).unwrap();
        let mut timestamps = TimestampWriter {
            file: BufWriter::new(file),
            len: 0,
        };
        timestamps.push(17).unwrap();
        assert!(timestamps.finish().is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"seed");
    }
}
