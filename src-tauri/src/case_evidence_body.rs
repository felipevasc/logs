//! Incremental compatibility-body I/O. This keeps the existing TEXT column
//! readable by older binaries; native immutable envelopes remain authoritative.
use crate::case_work_budget::Lease;
use rusqlite::{params, Connection, DatabaseName, Transaction};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
const LIMIT: usize = 64 << 20;
const BUFFER: usize = 256 << 10;
const INVALID: &str =
    "CASE_EVIDENCE_MIRROR: O espelho foi alterado ou excede o limite; nenhum Caso foi publicado.";

fn require_utf8(conn: &Connection) -> Result<(), String> {
    let encoding: String = conn
        .query_row("PRAGMA encoding", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if encoding != "UTF-8" {
        return Err("CASE_EVIDENCE_DB_ENCODING: O armazenamento usa uma codificação não suportada para evidência nativa; os originais foram preservados.".into());
    }
    Ok(())
}

pub(crate) struct OriginalBody {
    text: String,
    _credit: Lease,
}
impl OriginalBody {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}
pub(crate) fn read_case_body(conn: &Connection, id: &str) -> Result<OriginalBody, String> {
    crate::operations::check()?;
    require_utf8(conn)?;
    if id.is_empty() || id.len() > 4096 {
        return Err(INVALID.into());
    }
    let (rowid, bytes, kind): (i64, usize, String) = conn
        .query_row(
            "SELECT rowid,octet_length(body),typeof(body) FROM cases WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    if bytes > LIMIT || kind != "text" {
        return Err(INVALID.into());
    }
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes.checked_add(BUFFER).ok_or(INVALID)?,
    )?;
    let mut blob = conn
        .blob_open(DatabaseName::Main, "cases", "body", rowid, true)
        .map_err(|e| e.to_string())?;
    if blob.len() != bytes {
        return Err(INVALID.into());
    }
    let mut output = vec![0; bytes];
    for chunk in output.chunks_mut(BUFFER) {
        crate::operations::check()?;
        blob.read_exact(chunk).map_err(|e| e.to_string())?;
    }
    blob.close().map_err(|e| e.to_string())?;
    let text = String::from_utf8(output).map_err(|_| INVALID)?;
    crate::operations::check()?;
    Ok(OriginalBody {
        text,
        _credit: credit,
    })
}
pub(super) struct StagedMirror {
    file: tempfile::NamedTempFile,
    bytes: usize,
    sha256: String,
}
impl StagedMirror {
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(super) fn sha256(&self) -> &str {
        &self.sha256
    }
}
pub(super) fn stage_mirror(
    root: &Path,
    produce: impl FnOnce(&mut dyn Write) -> Result<(), String>,
) -> Result<StagedMirror, String> {
    crate::operations::check()?;
    let mut file = tempfile::Builder::new()
        .prefix("native-case-mirror-")
        .tempfile_in(root)
        .map_err(|e| e.to_string())?;
    struct Sink<'a> {
        file: &'a mut File,
        bytes: usize,
        hash: Sha256,
    }
    impl Write for Sink<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            crate::operations::check().map_err(std::io::Error::other)?;
            let total = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= LIMIT)
                .ok_or_else(|| std::io::Error::other(INVALID))?;
            self.file.write_all(bytes)?;
            self.hash.update(bytes);
            self.bytes = total;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let mut sink = Sink {
        file: file.as_file_mut(),
        bytes: 0,
        hash: Sha256::new(),
    };
    produce(&mut sink)?;
    sink.flush().map_err(|e| e.to_string())?;
    let bytes = sink.bytes;
    let sha256 = format!("{:x}", sink.hash.finalize());
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    // Syntax-only streaming validation never reconstructs an Event/Value tree
    // or interprets the preserved number lexemes.
    let _scratch = crate::case_cache::reserve_work(crate::case_work_budget::global(), BUFFER * 2)?;
    struct Checked<'a> {
        file: &'a File,
        left: usize,
    }
    impl Read for Checked<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            crate::operations::check().map_err(std::io::Error::other)?;
            if self.left == 0 {
                return Ok(0);
            }
            let n = bytes.len().min(self.left);
            let read = self.file.read(&mut bytes[..n])?;
            self.left -= read;
            Ok(read)
        }
    }
    let mut input = file.as_file();
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let input = std::io::BufReader::with_capacity(
        BUFFER,
        Checked {
            file: input,
            left: bytes,
        },
    );
    serde_json::from_reader::<_, serde::de::IgnoredAny>(input)
        .map_err(|e| format!("{INVALID} {e}"))?;
    crate::operations::check()?;
    Ok(StagedMirror {
        file,
        bytes,
        sha256,
    })
}
/// Must run inside the verified permit transaction. A caller rolls back on any
/// error; no Blob survives this function or COMMIT. SQLite's final TEXT cast
/// and record construction get explicit workspace credit in addition to I/O.
pub(super) fn write_mirror(
    tx: &Transaction<'_>,
    id: &str,
    position: i64,
    mirror: &StagedMirror,
) -> Result<(), String> {
    write_mirror_inner(tx, id, position, mirror, |_| {})
}
fn write_mirror_inner(
    tx: &Transaction<'_>,
    id: &str,
    position: i64,
    mirror: &StagedMirror,
    mut copied: impl FnMut(usize),
) -> Result<(), String> {
    crate::operations::check()?;
    require_utf8(tx)?;
    if id.is_empty() || id.len() > 4096 || mirror.bytes > LIMIT || position < 0 {
        return Err(INVALID.into());
    }
    let workspace = mirror
        .bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(BUFFER * 2))
        .ok_or(INVALID)?;
    let _credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), workspace)?;
    let file = mirror.file.as_file();
    let stamp = (
        file.metadata().map_err(|e| e.to_string())?.len(),
        file.metadata().map_err(|e| e.to_string())?.modified().ok(),
    );
    if stamp.0 != mirror.bytes as u64 {
        return Err(INVALID.into());
    }
    tx.execute("INSERT INTO cases(id,body,position) VALUES(?1,zeroblob(?2),?3) ON CONFLICT(id) DO UPDATE SET body=excluded.body,position=excluded.position",params![id,mirror.bytes as i64,position]).map_err(|e|e.to_string())?;
    let rowid: i64 = tx
        .query_row("SELECT rowid FROM cases WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    let mut blob = tx
        .blob_open(DatabaseName::Main, "cases", "body", rowid, false)
        .map_err(|e| e.to_string())?;
    let mut input = file;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut buffer = vec![0; BUFFER];
    let mut total = 0usize;
    let mut hash = Sha256::new();
    while total < mirror.bytes {
        crate::operations::check()?;
        let count = (mirror.bytes - total).min(buffer.len());
        input
            .read_exact(&mut buffer[..count])
            .map_err(|e| e.to_string())?;
        blob.write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
        hash.update(&buffer[..count]);
        total += count;
        copied(total);
    }
    let mut trailing = [0; 1];
    if input.read(&mut trailing).map_err(|e| e.to_string())? != 0 {
        return Err(INVALID.into());
    }
    blob.close().map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != stamp.0
        || file.metadata().map_err(|e| e.to_string())?.modified().ok() != stamp.1
        || format!("{:x}", hash.finalize()) != mirror.sha256
    {
        return Err(INVALID.into());
    }
    crate::operations::check()?;
    tx.execute("UPDATE cases SET body=CAST(body AS TEXT) WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    let (kind, bytes): (String, usize) = tx
        .query_row(
            "SELECT typeof(body),octet_length(body) FROM cases WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    if kind != "text" || bytes != mirror.bytes {
        return Err(INVALID.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;
    fn protected(root: &Path, request: &NativeCaseOpen) -> Connection {
        let mut conn = Connection::open(root.join("investigations.sqlite3")).unwrap();
        let owner = EvidenceOwner {
            store_id: request.store.store_id.clone(),
            case_id: "c".into(),
            analysis_id: request.analysis_context.analysis_id.clone(),
        };
        db::transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| db::protect(tx, &owner),
        )
        .unwrap();
        conn
    }
    #[test]
    fn utf16_database_is_rejected_before_body_mutation() {
        let root = tempfile::tempdir().unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA encoding='UTF-16le';CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);").unwrap();
        let original = "{\"id\":\"c\",\"label\":\"日本\"}";
        conn.execute("INSERT INTO cases VALUES('c',?1,0)", [original])
            .unwrap();
        assert!(read_case_body(&conn, "c")
            .err()
            .unwrap()
            .contains("DB_ENCODING"));
        let mirror = stage_mirror(root.path(), |writer| {
            writer
                .write_all(b"{\"id\":\"c\"}")
                .map_err(|e| e.to_string())
        })
        .unwrap();
        let tx = conn.transaction().unwrap();
        assert!(write_mirror(&tx, "c", 0, &mirror)
            .unwrap_err()
            .contains("DB_ENCODING"));
        tx.rollback().unwrap();
        assert_eq!(
            conn.query_row::<String, _, _>("SELECT body FROM cases", [], |r| r.get(0))
                .unwrap(),
            original
        );
    }
    #[test]
    fn incremental_body_keeps_exact_json_and_text_storage_with_legacy_barrier() {
        let (root, request, _) = authority::tests::fixture();
        let mut conn = protected(root.path(), &request);
        let raw="{\n \"id\":\"c\", \"unknown\":[18446744073709551615,1.0,-0.0,2.547114365375239e-8],\"text\":\"日本\"\n}";
        let mirror = stage_mirror(root.path(), |writer| {
            writer.write_all(raw.as_bytes()).map_err(|e| e.to_string())
        })
        .unwrap();
        db::transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| write_mirror(tx, "c", 0, &mirror),
        )
        .unwrap();
        let restored = read_case_body(&conn, "c").unwrap();
        assert_eq!(restored.text(), raw);
        let kind: String = conn
            .query_row("SELECT typeof(body) FROM cases WHERE id='c'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "text");
        conn.execute("UPDATE cases SET body=?1 WHERE id='c'", [raw])
            .unwrap();
        assert!(conn
            .execute("UPDATE cases SET body='{}' WHERE id='c'", [])
            .is_err());
    }
    #[test]
    fn checksum_failure_after_blob_write_rolls_back_the_exact_original_body() {
        let (root, request, _) = authority::tests::fixture();
        let mut conn = protected(root.path(), &request);
        let mirror = stage_mirror(root.path(), |w| {
            w.write_all(b"{\"id\":\"c\"}").map_err(|e| e.to_string())
        })
        .unwrap();
        let mut file = mirror.file.as_file();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"{\"id\":\"x\"}").unwrap();
        assert!(db::transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| write_mirror(tx, "c", 0, &mirror)
        )
        .is_err());
        assert_eq!(read_case_body(&conn, "c").unwrap().text(), "{}");
        assert_eq!(
            conn.query_row::<usize, _, _>(
                "SELECT count(*) FROM native_evidence_permits",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn named_cancel_between_blob_chunks_closes_handle_and_rolls_back() {
        let (root, request, _) = authority::tests::fixture();
        let mut conn = protected(root.path(), &request);
        let mirror = stage_mirror(root.path(), |w| {
            w.write_all(b"{\"payload\":\"").map_err(|e| e.to_string())?;
            for _ in 0..3 {
                w.write_all(&vec![b'a'; BUFFER])
                    .map_err(|e| e.to_string())?;
            }
            w.write_all(b"\"}").map_err(|e| e.to_string())
        })
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let mut copied = 0;
        let result = crate::operations::run_with_token(token, || {
            db::transaction(
                &mut conn,
                &["c".into()],
                |_| Ok(()),
                |tx| {
                    write_mirror_inner(tx, "c", 0, &mirror, |bytes| {
                        copied = bytes;
                        crate::operations::cancel_id(&id);
                    })
                },
            )
        });
        assert!(result.is_err());
        assert_eq!(copied, BUFFER);
        assert_eq!(read_case_body(&conn, "c").unwrap().text(), "{}");
        // A fresh write proves no aborted Blob handle still owns this row.
        db::transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| write_mirror(tx, "c", 0, &mirror),
        )
        .unwrap();
        assert_eq!(
            read_case_body(&conn, "c").unwrap().text().len(),
            mirror.bytes()
        );
    }
}
