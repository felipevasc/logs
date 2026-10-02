//! Bounded durable idempotency. Retired requests never become a fresh action
//! under the same store CAS; capture tombstones outlive their in-memory token.
use super::*;
use crate::case_work_budget::Lease;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
const ROW_BYTES: usize = 16 << 20;
const TOTAL_BYTES: usize = 32 << 20;
const RECEIPTS: usize = 256;
const LIMIT:&str="CASE_EVIDENCE_RECEIPT_LIMIT: O histórico de recibos atingiu o limite; salve ou atualize a investigação antes de continuar.";
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Capture,
    Membership,
    Save,
    Import,
    Adoption,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Entry {
    pub version: u32,
    pub kind: Kind,
    pub expected_store: Option<StoreStamp>,
    pub publication_store: StoreIdentity,
    #[serde(deserialize_with = "native_deserialize_value")]
    pub value: Value,
}
pub(super) struct Replay {
    pub entry: Entry,
    _credit: Lease,
}
impl Replay {
    pub(super) fn into_parts(self) -> (Entry, Lease) {
        (self.entry, self._credit)
    }
}
pub(super) fn lookup(
    conn: &Connection,
    id: &str,
    fingerprint: &str,
    kind: Kind,
) -> Result<Option<Replay>, String> {
    prepare::request_id(id)?;
    if !db::exists(conn, "native_evidence_receipts")? {
        return Ok(None);
    }
    let size:Option<(usize,String)>=conn.query_row("SELECT octet_length(receipt),CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END FROM native_evidence_receipts WHERE request_id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?;
    let Some((bytes, actual)) = size else {
        return Ok(None);
    };
    if actual != fingerprint {
        return Err(
            "CASE_EVIDENCE_REQUEST_REUSED: O identificador já pertence a outra ação.".into(),
        );
    }
    if bytes > ROW_BYTES {
        return Err(LIMIT.into());
    }
    let mut credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes
            .checked_mul(3)
            .and_then(|n| n.checked_add(64 << 10))
            .ok_or(LIMIT)?,
    )?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(receipt)=?2 THEN receipt END FROM native_evidence_receipts WHERE request_id=?1",params![id,bytes],|row|row.get(0)).map_err(|e|e.to_string())?;
    let raw = RawJson::checked_with_limit(&text, ROW_BYTES)?;
    let plan = preflight_value(raw)?;
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        plan.materialization_credit,
    )?)?;
    let entry: Entry =
        serde_json::from_value(materialize_value(raw, plan)?).map_err(|e| e.to_string())?;
    if entry.version != 1 || entry.kind != kind {
        return Err("CASE_EVIDENCE_REQUEST_REUSED".into());
    }
    if db::stamp(conn)?.identity() != entry.publication_store {
        return Err("CASE_EVIDENCE_RECEIPT_EPOCH_CHANGED: A investigação foi restaurada; reabra a visualização.".into());
    }
    Ok(Some(Replay {
        entry,
        _credit: credit,
    }))
}
/// Call inside the publication transaction, after all response allocation and
/// serialization checks. Durable capture IDs for the current CAS are never
/// evicted merely because their pending token expired.
pub(super) fn insert(
    tx: &Transaction<'_>,
    id: &str,
    fingerprint: &str,
    entry: &Entry,
) -> Result<(), String> {
    prepare::request_id(id)?;
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("CASE_EVIDENCE_REQUEST_INVALID".into());
    }
    let bytes = view::json_size(entry, ROW_BYTES)?;
    let _credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes.checked_mul(3).ok_or(LIMIT)?,
    )?;
    let text = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    // Reject before persistence if this receipt could not be decoded under the
    // same metadata policy used by replay (64 MiB owned / bounded nodes).
    preflight_value(RawJson::checked_with_limit(&text, ROW_BYTES)?)?;
    let current = db::stamp(tx)?;
    // An old capture retry then fails its original CAS even after this row is
    // retired. Saves/imports retain their recent publication receipts below.
    tx.execute("DELETE FROM native_evidence_receipts WHERE json_valid(receipt) AND json_extract(receipt,'$.kind') IN ('capture','membership') AND (json_extract(receipt,'$.expectedStore.storeId') IS NOT ?1 OR json_extract(receipt,'$.expectedStore.epoch') IS NOT ?2 OR json_extract(receipt,'$.expectedStore.revision') IS NOT ?3)",params![current.store_id,current.epoch,current.revision]).map_err(|e|e.to_string())?;
    let (count, total): (usize, usize) = tx
        .query_row(
            "SELECT count(*),coalesce(sum(octet_length(receipt)),0) FROM native_evidence_receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    if count > RECEIPTS || total > TOTAL_BYTES {
        return Err(LIMIT.into());
    }
    if count >= RECEIPTS || total.checked_add(bytes).is_none_or(|n| n > TOTAL_BYTES) {
        let mut eligible = Vec::new();
        let mut stmt=tx.prepare("SELECT request_id,octet_length(receipt) FROM native_evidence_receipts WHERE json_valid(receipt) AND json_extract(receipt,'$.kind') NOT IN ('capture','membership') ORDER BY created_ms,rowid").map_err(|e|e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut remaining_count = count;
        let mut remaining_bytes = total;
        while remaining_count >= RECEIPTS
            || remaining_bytes
                .checked_add(bytes)
                .is_none_or(|n| n > TOTAL_BYTES)
        {
            crate::operations::check()?;
            let row = rows.next().map_err(|e| e.to_string())?.ok_or(LIMIT)?;
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            if id.len() > 160 || eligible.len() >= RECEIPTS {
                return Err(LIMIT.into());
            }
            let size: usize = row.get(1).map_err(|e| e.to_string())?;
            remaining_count = remaining_count.checked_sub(1).ok_or(LIMIT)?;
            remaining_bytes = remaining_bytes.checked_sub(size).ok_or(LIMIT)?;
            eligible.push(id);
        }
        drop(rows);
        drop(stmt);
        for id in eligible {
            crate::operations::check()?;
            tx.execute(
                "DELETE FROM native_evidence_receipts WHERE request_id=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.execute("INSERT INTO native_evidence_receipts(request_id,request_sha256,receipt,created_ms) VALUES(?1,?2,?3,?4)",params![id,fingerprint,text,chrono::Utc::now().timestamp_millis()]).map_err(|e|e.to_string())?;
    Ok(())
}
