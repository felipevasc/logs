//! Separating a file that the disk cannot hold twice.
//!
//! 1. Decide: one read-only pass records, as runs of bytes, which output each
//!    part of the original goes to (`map.bin`, 8 bytes per run).
//! 2. Carve: from the end of the original backwards, one chunk at a time, the
//!    chunk's bytes are written to two part files and the original is then
//!    truncated before them, so used space stays the same plus one chunk.
//!    The leading run, already in a single output, is never copied.
//! 3. Assemble: the shortened original is renamed to the output of its
//!    leading run, and the parts are appended to the outputs in file order,
//!    each part deleted once appended.
//!
//! A journal written before every irreversible step lets an interrupted
//! separation finish or, while carving, be undone: the parts are put back
//! into the original in their original order.
use super::outputs::{free_space, mib};
use super::stream::{read_at, Meter, Sink, Stats};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Free space kept on the disk while working in parts.
pub(crate) const RESERVE: u64 = 64 << 20;
const MIN_CHUNK: u64 = 1 << 20;
const MAX_CHUNK: u64 = 256 << 20;
const COPY: usize = 4 << 20;
const BLOCK_RUNS: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Decide,
    Carve,
    Assemble,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    pub start: u64,
    pub end: u64,
    pub matched: u64,
    pub rest: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assembly {
    /// Parts appended, in file order.
    pub done: usize,
    /// Length of the output after them.
    pub len: u64,
    pub created: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Journal {
    pub version: u32,
    pub id: String,
    pub original: PathBuf,
    pub matched: PathBuf,
    pub rest: PathBuf,
    pub work: PathBuf,
    pub bom: Vec<u8>,
    pub original_len: u64,
    pub phase: Phase,
    /// Length the original has once the recorded chunks are carved.
    pub cut: u64,
    /// End of the leading run, which stays in the original.
    pub head_end: u64,
    pub head_matched: bool,
    pub chunks: Vec<Chunk>,
    pub renamed: bool,
    /// [matched, rest]
    pub assembled: [Assembly; 2],
    pub matched_bytes: u64,
    pub rest_bytes: u64,
    pub stats: Stats,
    pub framing: String,
    pub started_ms: i64,
}

fn io(context: &str, error: std::io::Error) -> String {
    if error.raw_os_error() == Some(32) || error.raw_os_error() == Some(1224) {
        return format!("{context}: o arquivo está aberto em outro programa ou carregado nesta análise. Feche-o (ou remova-o das fontes) e tente de novo.");
    }
    format!("{context}: {error}")
}

impl Journal {
    pub(crate) fn path(dir: &Path, id: &str) -> PathBuf {
        dir.join(format!("{id}.json"))
    }

    pub(crate) fn save(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| io("Não foi possível guardar o andamento da separação", e))?;
        let path = Self::path(dir, &self.id);
        let temp = path.with_extension("json.tmp");
        let mut file = File::create(&temp).map_err(|e| io("Não foi possível guardar o andamento da separação", e))?;
        file.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
            .and_then(|_| file.sync_all())
            .map_err(|e| io("Não foi possível guardar o andamento da separação", e))?;
        drop(file);
        fs::rename(&temp, &path).map_err(|e| io("Não foi possível guardar o andamento da separação", e))
    }

    pub(crate) fn load(path: &Path) -> Result<Journal, String> {
        let bytes = fs::read(path).map_err(|e| io("Não foi possível ler o andamento da separação", e))?;
        let journal: Journal = serde_json::from_slice(&bytes).map_err(|e| format!("Andamento da separação ilegível: {e}"))?;
        if journal.version != 1 {
            return Err("Andamento de separação de uma versão desconhecida.".into());
        }
        Ok(journal)
    }

    pub(crate) fn map_path(&self) -> PathBuf {
        self.work.join("map.bin")
    }

    fn part(&self, index: usize, matched: bool) -> PathBuf {
        self.work.join(format!("parte-{index:06}.{}", if matched { "filtrado" } else { "restante" }))
    }

    fn output(&self, matched: bool) -> &Path {
        if matched { &self.matched } else { &self.rest }
    }

    /// Removes the work folder and the journal.
    pub(crate) fn discard(&self, dir: &Path) {
        let _ = fs::remove_dir_all(&self.work);
        let _ = fs::remove_file(Self::path(dir, &self.id));
    }

    pub(crate) fn carved(&self) -> u64 {
        self.original_len.saturating_sub(self.cut)
    }
}

/// Runs of bytes of the original, each going entirely to one output.
pub(crate) struct MapSink {
    out: BufWriter<File>,
    run: Option<(bool, u64)>,
    pub matched: u64,
    pub rest: u64,
}

impl MapSink {
    pub(crate) fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| io("Não foi possível criar a pasta de trabalho", e))?;
        Ok(MapSink { out: BufWriter::with_capacity(1 << 20, file), run: None, matched: 0, rest: 0 })
    }

    fn push(&mut self, matched: bool, len: u64) -> Result<(), String> {
        if len == 0 {
            return Ok(());
        }
        if matched { self.matched += len } else { self.rest += len }
        match &mut self.run {
            Some((flag, total)) if *flag == matched => *total += len,
            _ => {
                self.flush_run()?;
                self.run = Some((matched, len));
            }
        }
        Ok(())
    }

    fn flush_run(&mut self) -> Result<(), String> {
        if let Some((flag, len)) = self.run.take() {
            self.out.write_all(&(len << 1 | u64::from(flag)).to_le_bytes()).map_err(|e| io("Não foi possível gravar o mapa da separação", e))?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.flush_run()?;
        let file = self.out.into_inner().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| io("Não foi possível gravar o mapa da separação", e))
    }
}

impl Sink for MapSink {
    fn write(&mut self, bytes: &[u8], matched: bool) -> Result<(), String> {
        self.push(matched, bytes.len() as u64)
    }
    fn region(&mut self, from: u64, to: u64, matched: bool, _: &mut Meter) -> Result<(), String> {
        self.push(matched, to - from)
    }
    fn keeps(&self, _: bool) -> bool {
        true
    }
}

/// Random access to the runs of `map.bin`, a block at a time.
struct Runs {
    file: File,
    count: u64,
    block: Vec<u64>,
    first: u64,
}

impl Runs {
    fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| io("Não foi possível ler o mapa da separação", e))?;
        let count = file.metadata().map_err(|e| io("Não foi possível ler o mapa da separação", e))?.len() / 8;
        Ok(Runs { file, count, block: Vec::new(), first: u64::MAX })
    }

    fn get(&mut self, index: u64) -> Result<(bool, u64), String> {
        if index < self.first || index >= self.first + self.block.len() as u64 {
            self.first = index / BLOCK_RUNS * BLOCK_RUNS;
            let n = (self.count - self.first).min(BLOCK_RUNS) as usize;
            let mut bytes = vec![0; n * 8];
            self.file.seek(SeekFrom::Start(self.first * 8)).and_then(|_| self.file.read_exact(&mut bytes)).map_err(|e| io("Não foi possível ler o mapa da separação", e))?;
            self.block = bytes.chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).collect();
        }
        let value = self.block[(index - self.first) as usize];
        Ok((value & 1 == 1, value >> 1))
    }
}

/// Position in the runs: run `index` covers `start..end` of the original.
struct Cursor {
    index: u64,
    start: u64,
    end: u64,
}

/// The run that contains the byte before `at` (scanning from the data start).
fn cursor_before(runs: &mut Runs, data_start: u64, at: u64) -> Result<Option<Cursor>, String> {
    let mut start = data_start;
    for index in 0..runs.count {
        let (_, len) = runs.get(index)?;
        if start + len >= at {
            return Ok(Some(Cursor { index, start, end: start + len }));
        }
        start += len;
    }
    Ok(None)
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CHUNK: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn chunk_size(dir: &Path) -> Result<u64, String> {
    #[cfg(test)]
    if let Some(size) = TEST_CHUNK.with(std::cell::Cell::get) {
        return Ok(size);
    }
    let free = free_space(dir).unwrap_or(0);
    let usable = free.saturating_sub(RESERVE);
    if usable < 2 * MIN_CHUNK {
        return Err(format!(
            "Espaço livre insuficiente em {} mesmo para separar em partes: são necessários ao menos {} livres.",
            dir.display(),
            mib(RESERVE + 2 * MIN_CHUNK)
        ));
    }
    Ok((usable / 2).clamp(MIN_CHUNK, MAX_CHUNK))
}

fn create_part(path: &Path) -> Result<BufWriter<File>, String> {
    Ok(BufWriter::with_capacity(1 << 20, File::create(path).map_err(|e| io("Não foi possível gravar uma parte da separação", e))?))
}

fn close_part(part: BufWriter<File>) -> Result<(), String> {
    let file = part.into_inner().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| io("Não foi possível gravar uma parte da separação", e))
}

/// Copies `len` bytes from `input` (at its position) to the end of `output`.
fn copy(input: &mut impl Read, output: &mut impl Write, mut len: u64, buf: &mut [u8]) -> Result<(), String> {
    while len > 0 {
        let want = (len as usize).min(buf.len());
        input.read_exact(&mut buf[..want]).map_err(|e| io("Não foi possível ler uma parte da separação", e))?;
        output.write_all(&buf[..want]).map_err(|e| io("Não foi possível gravar a separação", e))?;
        len -= want as u64;
    }
    Ok(())
}

pub(crate) enum Stop {
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(error: String) -> Self {
        Stop::Failed(error)
    }
}

/// Brings the original to the journal's length. An interruption between saving
/// a chunk and truncating leaves it one chunk longer: that chunk is in its parts.
fn settle_original(journal: &Journal, original: &File) -> Result<(), String> {
    let len = original.metadata().map_err(|e| io("Não foi possível ler o arquivo original", e))?.len();
    if len == journal.cut {
        return Ok(());
    }
    if journal.chunks.last().is_some_and(|c| c.start == journal.cut && c.end == len) {
        return original.set_len(journal.cut).and_then(|_| original.sync_all()).map_err(|e| io("Não foi possível encurtar o arquivo original", e));
    }
    Err(format!(
        "O arquivo original tem {len} bytes, mas a separação esperava {}: ele foi alterado fora do LogInsight. As partes estão em {}.",
        journal.cut,
        journal.work.display()
    ))
}

/// Phase 2: moves the original's bytes after its leading run into parts,
/// truncating it chunk by chunk. Stops between chunks when cancelled.
pub(crate) fn carve(journal: &mut Journal, journal_dir: &Path, original: &mut File) -> Result<(), Stop> {
    settle_original(journal, original)?;
    let mut runs = Runs::open(&journal.map_path())?;
    let data_start = journal.bom.len() as u64;
    let Some(mut at) = cursor_before(&mut runs, data_start, journal.cut)? else { return Ok(()) };
    let mut meter = Meter::new("split-carve", "Separando do fim para o início", 0, journal.original_len - journal.head_end);
    let mut buf = vec![0u8; COPY];
    while journal.cut > journal.head_end {
        if crate::operations::cancelled() {
            return Err(Stop::Cancelled);
        }
        let size = chunk_size(&journal.work)?;
        let end = journal.cut;
        let start = end.saturating_sub(size).max(journal.head_end);
        // Runs inside start..end, collected backwards.
        let mut pieces: Vec<(bool, u64)> = Vec::new();
        loop {
            let (matched, _) = runs.get(at.index)?;
            let piece_start = at.start.max(start);
            let piece_end = at.end.min(end);
            if piece_end > piece_start {
                pieces.push((matched, piece_end - piece_start));
            }
            if at.start <= start || at.index == 0 {
                break;
            }
            let (_, previous) = runs.get(at.index - 1)?;
            at = Cursor { index: at.index - 1, start: at.start - previous, end: at.start };
        }
        let index = journal.chunks.len();
        let (mut matched_part, mut rest_part) = (create_part(&journal.part(index, true))?, create_part(&journal.part(index, false))?);
        let (mut matched_bytes, mut rest_bytes) = (0, 0);
        let mut position = start;
        for &(matched, len) in pieces.iter().rev() {
            let mut left = len;
            while left > 0 {
                let want = (left as usize).min(COPY);
                let got = read_at(original, position, &mut buf[..want])?;
                if got != want {
                    return Err(Stop::Failed("O arquivo original ficou menor durante a separação.".into()));
                }
                let part = if matched { &mut matched_part } else { &mut rest_part };
                part.write_all(&buf[..want]).map_err(|e| io("Não foi possível gravar uma parte da separação", e))?;
                position += want as u64;
                left -= want as u64;
            }
            if matched { matched_bytes += len } else { rest_bytes += len }
        }
        close_part(matched_part)?;
        close_part(rest_part)?;
        journal.chunks.push(Chunk { start, end, matched: matched_bytes, rest: rest_bytes });
        journal.cut = start;
        journal.save(journal_dir)?;
        let len = original.metadata().map_err(|e| io("Não foi possível ler o arquivo original", e))?.len();
        if len != end {
            return Err(Stop::Failed("O arquivo original foi alterado por outro programa durante a separação.".into()));
        }
        original.set_len(start).and_then(|_| original.sync_all()).map_err(|e| io("Não foi possível encurtar o arquivo original", e))?;
        meter.at(journal.original_len - start);
    }
    Ok(())
}

/// Puts carved chunks back into the original, in file order.
pub(crate) fn undo(journal: &mut Journal, journal_dir: &Path) -> Result<(), String> {
    let mut original = OpenOptions::new().write(true).open(&journal.original).map_err(|e| io("Não foi possível reabrir o arquivo original", e))?;
    settle_original(journal, &original)?;
    let mut runs = Runs::open(&journal.map_path())?;
    let data_start = journal.bom.len() as u64;
    let mut meter = Meter::new("split-undo", "Devolvendo os trechos ao arquivo original", journal.cut, journal.original_len);
    let mut buf = vec![0u8; COPY];
    let mut at = cursor_before(&mut runs, data_start, journal.cut + 1)?;
    while let Some(chunk) = journal.chunks.last().cloned() {
        let index = journal.chunks.len() - 1;
        let Some(cursor) = at.as_mut().filter(|_| chunk.start == journal.cut) else {
            return Err("O andamento da separação não corresponde ao arquivo original.".into());
        };
        let mut matched_part = File::open(journal.part(index, true)).map_err(|e| io("Não foi possível ler uma parte da separação", e))?;
        let mut rest_part = File::open(journal.part(index, false)).map_err(|e| io("Não foi possível ler uma parte da separação", e))?;
        original.seek(SeekFrom::Start(chunk.start)).map_err(|e| io("Não foi possível restaurar o arquivo original", e))?;
        let mut writer = BufWriter::with_capacity(1 << 20, &mut original);
        let mut position = chunk.start;
        while position < chunk.end {
            let (matched, _) = runs.get(cursor.index)?;
            let len = cursor.end.min(chunk.end) - position;
            copy(if matched { &mut matched_part } else { &mut rest_part }, &mut writer, len, &mut buf)?;
            position += len;
            if position == cursor.end && cursor.index + 1 < runs.count {
                let (_, next) = runs.get(cursor.index + 1)?;
                *cursor = Cursor { index: cursor.index + 1, start: cursor.end, end: cursor.end + next };
            }
        }
        writer.flush().map_err(|e| io("Não foi possível restaurar o arquivo original", e))?;
        drop(writer);
        original.sync_all().map_err(|e| io("Não foi possível restaurar o arquivo original", e))?;
        journal.chunks.pop();
        journal.cut = chunk.end;
        journal.save(journal_dir)?;
        let _ = fs::remove_file(journal.part(index, true));
        let _ = fs::remove_file(journal.part(index, false));
        meter.at(journal.cut);
    }
    journal.discard(journal_dir);
    Ok(())
}

/// Phase 3: the shortened original becomes the output of its leading run and
/// the parts are appended to the outputs in file order.
pub(crate) fn assemble(journal: &mut Journal, journal_dir: &Path) -> Result<(), String> {
    if journal.phase != Phase::Assemble {
        journal.phase = Phase::Assemble;
        journal.save(journal_dir)?;
    }
    let head = journal.head_matched;
    if !journal.renamed {
        if journal.original.exists() || !journal.output(head).exists() {
            fs::rename(&journal.original, journal.output(head)).map_err(|e| io("Não foi possível renomear o arquivo original", e))?;
        }
        journal.renamed = true;
        journal.assembled[usize::from(!head)] = Assembly { done: 0, len: journal.cut, created: true };
        journal.save(journal_dir)?;
    }
    let total = journal.chunks.len();
    let mut meter = Meter::new("split-assemble", "Montando os arquivos separados", 0, journal.carved());
    let mut moved = 0u64;
    let mut buf = vec![0u8; COPY];
    for matched in [head, !head] {
        let slot = usize::from(!matched);
        if !journal.assembled[slot].created {
            let mut file = File::create(journal.output(matched)).map_err(|e| io("Não foi possível criar o arquivo separado", e))?;
            file.write_all(&journal.bom).and_then(|_| file.sync_all()).map_err(|e| io("Não foi possível criar o arquivo separado", e))?;
            journal.assembled[slot] = Assembly { done: 0, len: journal.bom.len() as u64, created: true };
            journal.save(journal_dir)?;
        }
        let mut output = OpenOptions::new().write(true).open(journal.output(matched)).map_err(|e| io("Não foi possível abrir o arquivo separado", e))?;
        while journal.assembled[slot].done < total {
            let index = total - 1 - journal.assembled[slot].done;
            let chunk = &journal.chunks[index];
            let len = if matched { chunk.matched } else { chunk.rest };
            let part_path = journal.part(index, matched);
            let mut part = File::open(&part_path).map_err(|e| io("Não foi possível ler uma parte da separação", e))?;
            output.set_len(journal.assembled[slot].len).map_err(|e| io("Não foi possível gravar o arquivo separado", e))?;
            output.seek(SeekFrom::Start(journal.assembled[slot].len)).map_err(|e| io("Não foi possível gravar o arquivo separado", e))?;
            let mut writer = BufWriter::with_capacity(1 << 20, &mut output);
            copy(&mut part, &mut writer, len, &mut buf)?;
            writer.flush().map_err(|e| io("Não foi possível gravar o arquivo separado", e))?;
            drop(writer);
            output.sync_all().map_err(|e| io("Não foi possível gravar o arquivo separado", e))?;
            drop(part);
            journal.assembled[slot].done += 1;
            journal.assembled[slot].len += len;
            journal.save(journal_dir)?;
            let _ = fs::remove_file(&part_path);
            moved += len;
            meter.at(moved);
        }
    }
    journal.discard(journal_dir);
    Ok(())
}

/// After the decision pass: where the leading run ends.
pub(crate) fn locate_head(journal: &mut Journal) -> Result<(), String> {
    let mut runs = Runs::open(&journal.map_path())?;
    let data_start = journal.bom.len() as u64;
    match runs.count {
        0 => {
            journal.head_end = journal.original_len;
            journal.head_matched = false;
        }
        _ => {
            let (matched, len) = runs.get(0)?;
            journal.head_end = data_start + len;
            journal.head_matched = matched;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal(dir: &Path, content: &[u8], runs: &[(bool, u64)], bom: usize) -> Journal {
        TEST_CHUNK.with(|c| c.set(Some(300_001)));
        let original = dir.join("app.log");
        fs::write(&original, content).unwrap();
        let work = dir.join(".work");
        fs::create_dir_all(&work).unwrap();
        let mut map = MapSink::create(&work.join("map.bin")).unwrap();
        for &(m, n) in runs {
            map.push(m, n).unwrap();
        }
        let (matched_bytes, rest_bytes) = (map.matched, map.rest);
        map.finish().unwrap();
        let mut journal = Journal {
            version: 1, id: "t".into(), original, matched: dir.join("m.log"), rest: dir.join("r.log"), work,
            bom: content[..bom].to_vec(), original_len: content.len() as u64, phase: Phase::Carve, cut: content.len() as u64,
            head_end: 0, head_matched: false, chunks: Vec::new(), renamed: false, assembled: Default::default(),
            matched_bytes, rest_bytes, stats: Stats::default(), framing: String::new(), started_ms: 0,
        };
        locate_head(&mut journal).unwrap();
        journal
    }

    fn expected(content: &[u8], runs: &[(bool, u64)], bom: usize) -> (Vec<u8>, Vec<u8>) {
        let (mut m, mut r) = (content[..bom].to_vec(), content[..bom].to_vec());
        let mut at = bom;
        for &(flag, n) in runs {
            let piece = &content[at..at + n as usize];
            if flag { m.extend_from_slice(piece) } else { r.extend_from_slice(piece) }
            at += n as usize;
        }
        (m, r)
    }

    fn content(len: usize) -> Vec<u8> {
        (0..len).map(|i| b"abcdefghij\n"[i % 11]).collect()
    }

    #[test]
    fn carving_and_assembling_reproduce_both_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let data = content(5 << 20);
        let mut runs = vec![(true, 1_000_003), (false, 7), (true, 2 << 20), (false, 1)];
        let used: u64 = runs.iter().map(|r| r.1).sum();
        runs.push((true, data.len() as u64 - 3 - used));
        let mut bom_data = vec![0xef, 0xbb, 0xbf];
        bom_data.extend_from_slice(&data[..data.len() - 3]);
        let mut j = journal(dir.path(), &bom_data, &runs, 3);
        let mut original = OpenOptions::new().read(true).write(true).open(&j.original).unwrap();
        carve(&mut j, dir.path(), &mut original).map_err(|e| match e { Stop::Failed(e) => e, Stop::Cancelled => "cancelled".into() }).unwrap();
        drop(original);
        assert_eq!(j.cut, 3 + 1_000_003);
        assert!(!j.chunks.is_empty());
        assemble(&mut j, dir.path()).unwrap();
        let (m, r) = expected(&bom_data, &runs, 3);
        assert_eq!(fs::read(&j.matched).unwrap(), m);
        assert_eq!(fs::read(&j.rest).unwrap(), r);
        assert!(!j.original.exists() && !j.work.exists());
    }

    #[test]
    fn undo_restores_the_original_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let data = content(3 << 20);
        let runs = [(false, 10), (true, 1 << 20), (false, (2 << 20) - 10), (true, data.len() as u64 - (3 << 20))];
        let runs: Vec<_> = runs.into_iter().filter(|r| r.1 > 0).collect();
        let mut j = journal(dir.path(), &data, &runs, 0);
        let mut original = OpenOptions::new().read(true).write(true).open(&j.original).unwrap();
        carve(&mut j, dir.path(), &mut original).ok().unwrap();
        drop(original);
        assert_eq!(fs::metadata(&j.original).unwrap().len(), 10);
        undo(&mut j, dir.path()).unwrap();
        assert_eq!(fs::read(&j.original).unwrap(), data);
        assert!(!j.work.exists());
    }

    #[test]
    fn an_untruncated_chunk_is_settled_but_foreign_changes_stop() {
        let dir = tempfile::tempdir().unwrap();
        let data = content(1000);
        let mut j = journal(dir.path(), &data, &[(false, 1000)], 0);
        j.cut = 600;
        j.chunks.push(Chunk { start: 600, end: 1000, matched: 0, rest: 400 });
        let file = OpenOptions::new().write(true).open(&j.original).unwrap();
        settle_original(&j, &file).unwrap();
        assert_eq!(fs::metadata(&j.original).unwrap().len(), 600);
        file.set_len(700).unwrap();
        assert!(settle_original(&j, &file).unwrap_err().contains("alterado fora"));
    }

    #[test]
    fn single_run_files_are_renamed_without_copying() {
        let dir = tempfile::tempdir().unwrap();
        let data = content(1000);
        let mut j = journal(dir.path(), &data, &[(false, 1000)], 0);
        let mut original = OpenOptions::new().read(true).write(true).open(&j.original).unwrap();
        carve(&mut j, dir.path(), &mut original).ok().unwrap();
        drop(original);
        assert!(j.chunks.is_empty());
        assemble(&mut j, dir.path()).unwrap();
        assert_eq!(fs::read(&j.rest).unwrap(), data);
        assert_eq!(fs::read(&j.matched).unwrap(), b"");
    }

    #[test]
    fn interrupted_assembly_resumes_without_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let data = content(3 << 20);
        let runs = vec![(true, 100), (false, data.len() as u64 - 100)];
        let mut j = journal(dir.path(), &data, &runs, 0);
        let mut original = OpenOptions::new().read(true).write(true).open(&j.original).unwrap();
        carve(&mut j, dir.path(), &mut original).ok().unwrap();
        drop(original);
        // Simulate a crash after the first part was appended but before the journal was saved.
        let mut first = j.clone();
        first.phase = Phase::Assemble;
        fs::rename(&first.original, &first.matched).unwrap();
        let mut rest = File::create(&first.rest).unwrap();
        let last = first.chunks.len() - 1;
        rest.write_all(&fs::read(first.part(last, false)).unwrap()).unwrap();
        drop(rest);
        first.renamed = true;
        first.assembled[0] = Assembly { done: 0, len: first.cut, created: true };
        first.assembled[1] = Assembly { done: 0, len: 0, created: true };
        assemble(&mut first, dir.path()).unwrap();
        let (m, r) = expected(&data, &runs, 0);
        assert_eq!(fs::read(&j.matched).unwrap(), m);
        assert_eq!(fs::read(&j.rest).unwrap(), r);
    }
}
