//! Output files written while the original is read. Each output is a hidden
//! temporary file next to its destination until the end, so a cancelled or
//! failed separation leaves nothing behind.
use super::stream::{read_at, Meter, Sink};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Free space kept on the disk while writing outputs.
pub(crate) const RESERVE: u64 = 256 << 20;
const CHECK_EVERY: u64 = 64 << 20;
const COPY: usize = 4 << 20;

pub(crate) fn free_space(dir: &Path) -> Option<u64> {
    fs2::available_space(dir).ok()
}

pub(crate) fn parent(path: &Path) -> PathBuf {
    path.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn mib(bytes: u64) -> String {
    let value = bytes as f64;
    if value >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", value / (1024.0 * 1024.0 * 1024.0)).replace('.', ",")
    } else {
        format!("{:.0} MB", (value / (1024.0 * 1024.0)).ceil())
    }
}

struct Output {
    path: PathBuf,
    dir: PathBuf,
    file: BufWriter<tempfile::NamedTempFile>,
    bytes: u64,
    unchecked: u64,
}

impl Output {
    fn create(path: &Path, bom: &[u8]) -> Result<Self, String> {
        let dir = parent(path);
        let temp = tempfile::Builder::new()
            .prefix(".loginsight-separando-")
            .suffix(".parcial")
            .tempfile_in(&dir)
            .map_err(|e| format!("Não foi possível criar o arquivo em {}: {e}", dir.display()))?;
        let mut file = BufWriter::with_capacity(4 << 20, temp);
        file.write_all(bom).map_err(|e| format!("Não foi possível gravar {}: {e}", path.display()))?;
        Ok(Output { path: path.to_path_buf(), dir, file, bytes: bom.len() as u64, unchecked: 0 })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.file.write_all(bytes).map_err(|e| format!("Não foi possível gravar {}: {e}", self.path.display()))?;
        self.bytes += bytes.len() as u64;
        self.unchecked += bytes.len() as u64;
        if self.unchecked >= CHECK_EVERY {
            self.unchecked = 0;
            if free_space(&self.dir).is_some_and(|free| free < RESERVE) {
                return Err(format!(
                    "O disco de {} ficou com menos de {} livres; a separação foi interrompida e os arquivos parciais removidos. Libere espaço, escolha outra pasta ou use “remover o original”, que trabalha em partes.",
                    self.dir.display(),
                    mib(RESERVE)
                ));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(PathBuf, u64), String> {
        let temp = self.file.into_inner().map_err(|e| format!("Não foi possível gravar {}: {}", self.path.display(), e.error()))?;
        temp.as_file().sync_all().map_err(|e| format!("Não foi possível concluir {}: {e}", self.path.display()))?;
        temp.persist(&self.path).map_err(|e| format!("Não foi possível salvar {}: {}", self.path.display(), e.error))?;
        Ok((self.path, self.bytes))
    }
}

pub(crate) struct Outputs {
    matched: Output,
    rest: Option<Output>,
    source: File,
    buf: Vec<u8>,
}

pub(crate) struct Written {
    pub matched: (PathBuf, u64),
    pub rest: Option<(PathBuf, u64)>,
}

impl Outputs {
    pub(crate) fn create(matched: &Path, rest: Option<&Path>, bom: &[u8], source: File) -> Result<Self, String> {
        Ok(Outputs { matched: Output::create(matched, bom)?, rest: rest.map(|p| Output::create(p, bom)).transpose()?, source, buf: Vec::new() })
    }

    pub(crate) fn finish(self) -> Result<Written, String> {
        let matched = self.matched.finish()?;
        let rest = self.rest.map(Output::finish).transpose()?;
        Ok(Written { matched, rest })
    }
}

impl Sink for Outputs {
    fn write(&mut self, bytes: &[u8], matched: bool) -> Result<(), String> {
        match (matched, self.rest.as_mut()) {
            (true, _) => self.matched.write(bytes),
            (false, Some(rest)) => rest.write(bytes),
            (false, None) => Ok(()),
        }
    }

    fn region(&mut self, from: u64, to: u64, matched: bool, meter: &mut Meter) -> Result<(), String> {
        self.buf.resize(COPY, 0);
        let mut at = from;
        while at < to {
            crate::operations::check()?;
            let want = ((to - at) as usize).min(COPY);
            let mut buf = std::mem::take(&mut self.buf);
            let got = read_at(&mut self.source, at, &mut buf[..want]);
            let result = got.and_then(|got| {
                if got == 0 {
                    return Err("O arquivo original ficou menor durante a separação.".to_string());
                }
                at += got as u64;
                self.write(&buf[..got], matched)
            });
            self.buf = buf;
            result?;
            meter.at(at);
        }
        Ok(())
    }

    fn keeps(&self, matched: bool) -> bool {
        matched || self.rest.is_some()
    }
}
