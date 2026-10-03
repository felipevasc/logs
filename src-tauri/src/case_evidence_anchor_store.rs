//! Frozen authored legacy-reference decisions. These rows belong to native
//! authority and are never replaced from an ordinary authored Case document.
use super::*;
use crate::{case_evidence_anchors::AnchorMap, case_work_budget::Lease};
use rusqlite::{params, Connection, Transaction};
const LIMIT: usize = 1 << 20;
const INVALID:&str="CASE_EVIDENCE_ANCHOR_AUTHORITY: A origem das referências não está disponível; os dados foram preservados.";

pub(super) fn insert(
    tx: &Transaction<'_>,
    owner: &EvidenceOwner,
    map: &AnchorMap,
) -> Result<(), String> {
    map.validate()?;
    if db::stamp(tx)?.store_id != owner.store_id {
        return Err(INVALID.into());
    }
    let bytes = view::json_size(map, LIMIT)?;
    let _credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes.checked_mul(3).ok_or(INVALID)?,
    )?;
    let body = serde_json::to_string(map).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO native_evidence_anchors(case_id,analysis_id,body) VALUES(?1,?2,?3) ON CONFLICT(case_id,analysis_id) DO NOTHING",params![owner.case_id,owner.analysis_id,body]).map_err(|e|e.to_string())?;
    let same:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_evidence_anchors WHERE case_id=?1 AND analysis_id=?2 AND body=?3)",params![owner.case_id,owner.analysis_id,body],|row|row.get(0)).map_err(|e|e.to_string())?;
    if !same {
        return Err(INVALID.into());
    }
    Ok(())
}
pub(super) fn read(conn: &Connection, owner: &EvidenceOwner) -> Result<(AnchorMap, Lease), String> {
    crate::operations::check()?;
    let bytes:usize=conn.query_row("SELECT octet_length(body) FROM native_evidence_anchors WHERE case_id=?1 AND analysis_id=?2",params![owner.case_id,owner.analysis_id],|row|row.get(0)).map_err(|_|INVALID)?;
    if bytes > LIMIT {
        return Err(INVALID.into());
    }
    let mut credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes
            .checked_mul(3)
            .and_then(|n| n.checked_add(16 << 10))
            .ok_or(INVALID)?,
    )?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(body)=?3 THEN body END FROM native_evidence_anchors WHERE case_id=?1 AND analysis_id=?2",params![owner.case_id,owner.analysis_id,bytes],|row|row.get(0)).map_err(|e|e.to_string())?;
    let raw = RawJson::checked_with_limit(&text, LIMIT)?;
    let plan = preflight_value(raw)?;
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        plan.materialization_credit,
    )?)?;
    let map: AnchorMap =
        serde_json::from_value(materialize_value(raw, plan)?).map_err(|e| e.to_string())?;
    map.validate()?;
    Ok((map, credit))
}
/// Moves the admitted item-only summary into a receipt/view; the caller keeps
/// the returned credit until IPC completes. No authored JSON can clear blocks.
pub(super) fn decorate(
    conn: &Connection,
    owner: &EvidenceOwner,
    state: &mut CaseEvidenceState,
) -> Result<Lease, String> {
    let (map, credit) = read(conn, owner)?;
    if let CaseEvidenceState::Ready(state) = state {
        state.legacy_item_aliases = map.item_aliases;
    }
    Ok(credit)
}
