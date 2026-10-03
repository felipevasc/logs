//! Exact native source membership in a preserved Case. The command owns source
//! admission and final source revalidation; this module never accepts JS records.
use crate::{
    analysis_visibility::EvidenceProvenance,
    case_evidence::{self, EvidenceOwner, MemberHandle, RawJson, StoreIdentity},
    case_evidence_history::{self as history, HistoryAuthority, Response, TimelineRequest},
    case_work_budget,
    exclusion_store::Locator,
    page_projection::{Receipt, RowHandle},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Write};

const INVALID: &str = "CASE_MEMBERS_IDENTITY_CHANGED";
const UNAVAILABLE: &str = "CASE_MEMBERS_SOURCE_UNAVAILABLE";
const LIMIT: &str = "CASE_MEMBERS_LIMIT";
const AMBIGUITY_LIMIT: &str = "CASE_MEMBERS_AMBIGUITY_LIMIT";
const RESPONSE_BYTES: usize = 1 << 20;
const MAX_ROWS: usize = 2_000;
const MAX_JS_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FindMembersRequest {
    pub store: StoreIdentity,
    pub owner: EvidenceOwner,
    pub case_evidence_signature: String,
    pub source: Receipt,
    pub rows: Vec<RowHandle>,
    pub station_id: Option<String>,
}

/// Only native admitted source resolvers construct these proofs. A digest of a
/// preview, overlaid Event, or reconstructed browser body is never this proof.
#[derive(Clone, Debug)]
pub(crate) struct FindMemberProof {
    key: ProofKey,
    reference: Option<[u8; 32]>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ProofKey {
    Provenance([u8; 32]),
    Member([u8; 32]),
    OriginalEnvelope([u8; 32]),
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn digest(value: &impl Serialize) -> Result<[u8; 32], String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).map_err(|_| INVALID)?;
    Ok(writer.0.finalize().into())
}
fn uuid(value: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| INVALID.into())
}
impl FindMemberProof {
    /// The source resolver must already have validated provenance against the
    /// captured source receipt and the requested RowHandle.
    pub(crate) fn provenance(event_ref: &str, value: &EvidenceProvenance) -> Result<Self, String> {
        if event_ref.is_empty() || event_ref.len() > 2_048 {
            return Err(UNAVAILABLE.into());
        }
        let source = value.source.key()?;
        match &value.locator {
            Locator::ByteOffset(offset) => {
                if value
                    .source
                    .event_ref_prefix
                    .as_ref()
                    .is_some_and(|prefix| format!("{prefix}:{offset}") != event_ref)
                {
                    return Err(UNAVAILABLE.into());
                }
            }
            Locator::StableRecord(value) if value.is_empty() || value.len() > 512 => {
                return Err(UNAVAILABLE.into())
            }
            Locator::StableRecord(_) => {}
        }
        Ok(Self {
            key: ProofKey::Provenance(digest(&(
                "case-member-provenance-v1",
                source,
                &value.locator,
                event_ref,
            ))?),
            reference: Some(Sha256::digest(event_ref.as_bytes()).into()),
        })
    }
    /// A verified source Case publication supplies its immutable original
    /// occurrence. Membership changes may replace the manifest, not occurrence.
    pub(crate) fn member(member: &MemberHandle) -> Result<Self, String> {
        uuid(&member.container_id)?;
        uuid(&member.manifest_id)?;
        uuid(&member.occurrence_id)?;
        Ok(Self {
            key: ProofKey::Member(digest(&(
                "case-member-occurrence-v1",
                &member.container_id,
                &member.occurrence_id,
            ))?),
            reference: None,
        })
    }
    /// Requires the exact original envelope bytes retained by native capture.
    /// An admitted Memory Event without these bytes/provenance cannot infer old
    /// membership; its wrapper must return CASE_MEMBERS_SOURCE_UNAVAILABLE.
    pub(crate) fn original_envelope(envelope: &str) -> Result<Self, String> {
        RawJson::checked(envelope)?;
        Ok(Self {
            key: ProofKey::OriginalEnvelope(Sha256::digest(envelope.as_bytes()).into()),
            reference: None,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberMatch {
    pub container_id: String,
    pub item_id: Option<String>,
    pub item_index: u32,
    pub member: MemberHandle,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberRow {
    pub row: RowHandle,
    pub state: &'static str,
    pub matches: Vec<MemberMatch>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FindMembersResult {
    pub kind: &'static str,
    pub case_evidence_signature: String,
    pub rows: Vec<MemberRow>,
}
struct Count {
    bytes: usize,
    limit: usize,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other(LIMIT))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded_len(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    let mut writer = Count { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| LIMIT)?;
    Ok(writer.bytes)
}
fn timeline_request(request: &FindMembersRequest) -> TimelineRequest {
    TimelineRequest {
        store: request.store.clone(),
        owner: request.owner.clone(),
        evidence_signature: request.case_evidence_signature.clone(),
        authored_view_json: "{\"timeline\":{},\"manual\":[]}".into(),
        station_id: request.station_id.clone(),
        filters: vec![],
        from_ms: None,
        to_ms: None,
        include_untimed: true,
        cursor: None,
        aliases: vec![],
    }
}
fn validate_request(request: &FindMembersRequest) -> Result<(), String> {
    if request.rows.len() > MAX_ROWS
        || request.rows.is_empty()
        || request.owner.store_id != request.store.store_id
        || request.owner.case_id.is_empty()
        || request.owner.case_id.len() > 4_096
        || request
            .station_id
            .as_ref()
            .is_some_and(|id| id.len() > 4_096)
        || request.case_evidence_signature.len() != 64
        || !request
            .case_evidence_signature
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(LIMIT.into());
    }
    uuid(&request.store.store_id)?;
    uuid(&request.store.epoch)?;
    uuid(&request.owner.analysis_id)?;
    for row in &request.rows {
        if row.id as u128 > MAX_JS_INTEGER as u128
            || row.event_ref.is_empty()
            || row.event_ref.len() > 2_048
        {
            return Err(INVALID.into());
        }
    }
    encoded_len(request, 2 << 20)?;
    Ok(())
}

// Read only identity fields. Unsupported unrelated columns remain preserved and
// do not need a full Value/Event allocation to locate an exact source record.
fn preserved_provenance(
    envelope: &str,
    requested_refs: &BTreeMap<[u8; 32], ()>,
) -> Result<Option<ProofKey>, String> {
    if requested_refs.is_empty() {
        return Ok(None);
    }
    let raw = RawJson::checked(envelope)?;
    if raw.kind() != b'{' {
        return Ok(None);
    }
    let mut reference = None;
    let mut provenance = None;
    let mut members = raw.members()?;
    while let Some(member) = members.next()? {
        match member.key()?.as_str() {
            "event_ref" => {
                if reference.replace(member.value).is_some() {
                    return Err(UNAVAILABLE.into());
                }
            }
            "evidence_provenance" => {
                if provenance.replace(member.value).is_some() {
                    return Err(UNAVAILABLE.into());
                }
            }
            _ => {}
        }
    }
    let Some(reference) = reference.filter(|raw| raw.kind() == b'"') else {
        return Ok(None);
    };
    // Escaping can expand a bounded native reference by at most six bytes/char.
    if reference.get().len() > 6 * 2_048 + 2 {
        return Ok(None);
    }
    let reference: String = serde_json::from_str(reference.get()).map_err(|_| UNAVAILABLE)?;
    if !requested_refs.contains_key(&<[u8; 32]>::from(Sha256::digest(reference.as_bytes()))) {
        return Ok(None);
    }
    let Some(provenance) = provenance.filter(|raw| raw.kind() == b'{') else {
        return Err(UNAVAILABLE.into());
    };
    if provenance.get().len() > 64 << 10 {
        return Err(UNAVAILABLE.into());
    }
    let provenance: EvidenceProvenance =
        serde_json::from_str(provenance.get()).map_err(|_| UNAVAILABLE)?;
    Ok(Some(
        FindMemberProof::provenance(&reference, &provenance)?.key,
    ))
}

/// `resolve` runs once per requested handle under the command's admitted source
/// proof. The command must revalidate that receipt again after this call and
/// before serialization; this helper independently revalidates Case authority.
pub(crate) fn find_members(
    authority: &impl HistoryAuthority,
    request: &FindMembersRequest,
    resolve: &mut dyn FnMut(&RowHandle) -> Result<FindMemberProof, String>,
) -> Result<Response<FindMembersResult>, String> {
    crate::operations::check()?;
    validate_request(request)?;
    let binding = timeline_request(request);
    let count = authority.validate(&binding)?;
    let pool = case_work_budget::global();
    let response_credit = crate::case_cache::reserve_work(pool, 4 << 20)?;
    // Fixed digest keys, Vec indices and tree nodes; proofs never retain Events.
    let _index_credit =
        crate::case_cache::reserve_work(pool, request.rows.len().checked_mul(1_024).ok_or(LIMIT)?)?;
    let _encoded_credit = crate::case_cache::reserve_work(pool, case_evidence::ENVELOPE_BYTES)?;
    // A single top-level key can be as large as the envelope. Reserve its decode
    // plus serde's string scratch before storage reads; no record tree retained.
    let _scratch_credit = crate::case_cache::reserve_work(pool, 2 * case_evidence::ENVELOPE_BYTES)?;
    let mut index: BTreeMap<ProofKey, Vec<usize>> = BTreeMap::new();
    let mut references = BTreeMap::new();
    let mut response = FindMembersResult {
        kind: "native_case_members",
        case_evidence_signature: request.case_evidence_signature.clone(),
        rows: Vec::with_capacity(request.rows.len()),
    };
    let mut response_bytes = encoded_len(&response, RESPONSE_BYTES)?;
    for (i, row) in request.rows.iter().enumerate() {
        crate::operations::check()?;
        let proof = resolve(row)?;
        if let Some(reference) = proof.reference {
            if reference != <[u8; 32]>::from(Sha256::digest(row.event_ref.as_bytes())) {
                return Err(INVALID.into());
            }
            references.insert(reference, ());
        }
        index.entry(proof.key).or_default().push(i);
        let result = MemberRow {
            row: row.clone(),
            state: "missing",
            matches: vec![],
        };
        response_bytes = response_bytes
            .checked_add(encoded_len(&result, RESPONSE_BYTES)? + 1)
            .filter(|n| *n <= RESPONSE_BYTES)
            .ok_or(LIMIT)?;
        response.rows.push(result);
    }
    let has_envelope = index
        .keys()
        .any(|key| matches!(key, ProofKey::OriginalEnvelope(_)));
    let has_member = index.keys().any(|key| matches!(key, ProofKey::Member(_)));
    let mut visited = 0u32;
    let mut previous = None;
    authority.visit(&mut |item, member, envelope| {
        crate::operations::check()?;
        visited = visited
            .checked_add(1)
            .filter(|n| *n <= count)
            .ok_or(INVALID)?;
        let position = (item.item_index, item.original_row_index);
        if previous.is_some_and(|old| old >= position) {
            return Err(INVALID.into());
        }
        previous = Some(position);
        if request.station_id.as_deref().is_some_and(|station| {
            item.item
                .get("stationId")
                .and_then(serde_json::Value::as_str)
                != Some(station)
        }) {
            return Ok(());
        }
        let keys = [
            if has_member {
                Some(FindMemberProof::member(&member)?.key)
            } else {
                None
            },
            if has_envelope {
                Some(ProofKey::OriginalEnvelope(
                    Sha256::digest(envelope.as_bytes()).into(),
                ))
            } else {
                None
            },
            preserved_provenance(envelope, &references)?,
        ];
        for key in keys.into_iter().flatten() {
            let Some(rows) = index.get(&key) else {
                continue;
            };
            let item_id = item.item.get("id").and_then(serde_json::Value::as_str);
            if item_id.is_some_and(|id| id.len() > 4_096) {
                return Err(LIMIT.into());
            }
            uuid(&member.container_id)?;
            uuid(&member.manifest_id)?;
            uuid(&member.occurrence_id)?;
            // Count the complete match before any identity clone/retention. A
            // large ambiguous set is an explicit error, never a partial choice.
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct BorrowedMatch<'a> {
                container_id: &'a str,
                item_id: Option<&'a str>,
                item_index: u32,
                member: &'a MemberHandle,
            }
            let bytes = encoded_len(
                &BorrowedMatch {
                    container_id: &member.container_id,
                    item_id,
                    item_index: item.item_index,
                    member: &member,
                },
                RESPONSE_BYTES,
            )? + 4;
            for i in rows {
                response_bytes = response_bytes
                    .checked_add(bytes)
                    .filter(|n| *n <= RESPONSE_BYTES)
                    .ok_or(AMBIGUITY_LIMIT)?;
                response.rows[*i].matches.push(MemberMatch {
                    container_id: member.container_id.clone(),
                    item_id: item_id.map(str::to_owned),
                    item_index: item.item_index,
                    member: member.clone(),
                });
            }
        }
        Ok(())
    })?;
    if visited != count || authority.validate(&binding)? != count {
        return Err(INVALID.into());
    }
    for row in &mut response.rows {
        row.state = match row.matches.len() {
            0 => "missing",
            1 => "unique",
            _ => "ambiguous",
        };
    }
    crate::operations::check()?;
    history::finish(response, response_credit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        case_evidence_history::HistoricalItem, exclusion_store::SourceDescriptor, model::Event,
    };
    use serde_json::{json, Value};
    use std::cell::Cell;

    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn member() -> MemberHandle {
        MemberHandle {
            container_id: id(),
            manifest_id: id(),
            occurrence_id: id(),
        }
    }
    fn provenance(version: &str) -> EvidenceProvenance {
        EvidenceProvenance {
            source: SourceDescriptor {
                version: version.into(),
                record_space: "lines-v1".into(),
                label: "original label".into(),
                event_ref_prefix: Some("source".into()),
            },
            locator: Locator::ByteOffset(50),
        }
    }
    fn request() -> FindMembersRequest {
        let store = StoreIdentity {
            store_id: id(),
            epoch: id(),
        };
        let owner = EvidenceOwner {
            store_id: store.store_id.clone(),
            case_id: "case".into(),
            analysis_id: id(),
        };
        let source=serde_json::from_value(json!({"analysisContext":{"caseId":"source-case","analysisId":id(),"configRevision":0,"visibilityRevision":0},"sourceGeneration":1,"caseKey":null,"caseContentToken":null,"catalogSignature":"a".repeat(64),"catalogEpoch":0})).unwrap();
        FindMembersRequest {
            store,
            owner,
            case_evidence_signature: "b".repeat(64),
            source,
            rows: vec![RowHandle {
                id: 17,
                event_ref: "source:50".into(),
            }],
            station_id: None,
        }
    }
    struct Fixture {
        request: FindMembersRequest,
        rows: Vec<(Value, u32, u32, MemberHandle, String)>,
        validations: Cell<usize>,
        scans: Cell<usize>,
        visits: Cell<usize>,
        invalid_final: bool,
        cancel_after: Option<(usize, String)>,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                request: request(),
                rows: vec![],
                validations: Cell::new(0),
                scans: Cell::new(0),
                visits: Cell::new(0),
                invalid_final: false,
                cancel_after: None,
            }
        }
        fn push(&mut self, item: Value, item_index: u32, envelope: String) -> MemberHandle {
            let previous: Vec<_> = self
                .rows
                .iter()
                .filter(|(_, i, _, _, _)| *i == item_index)
                .collect();
            let mut handle = member();
            if let Some((_, _, _, first, _)) = previous.first() {
                handle.container_id = first.container_id.clone();
                handle.manifest_id = first.manifest_id.clone();
            }
            self.rows.push((
                item,
                item_index,
                previous.len() as u32,
                handle.clone(),
                envelope,
            ));
            handle
        }
        fn run(&self, proof: FindMemberProof) -> Result<Response<FindMembersResult>, String> {
            find_members(self, &self.request, &mut |_| Ok(proof.clone()))
        }
    }
    impl HistoryAuthority for Fixture {
        fn validate(&self, request: &TimelineRequest) -> Result<u32, String> {
            let n = self.validations.get();
            self.validations.set(n + 1);
            if request.store != self.request.store
                || request.owner != self.request.owner
                || request.evidence_signature != self.request.case_evidence_signature
                || (n > 0 && self.invalid_final)
            {
                return Err(INVALID.into());
            }
            Ok(self.rows.len() as u32)
        }
        fn visit(
            &self,
            visitor: &mut dyn FnMut(HistoricalItem<'_>, MemberHandle, &str) -> Result<(), String>,
        ) -> Result<(), String> {
            self.scans.set(self.scans.get() + 1);
            for (item, index, origin, member, envelope) in &self.rows {
                self.visits.set(self.visits.get() + 1);
                if let Some((after, id)) = &self.cancel_after {
                    if self.visits.get() == *after {
                        crate::operations::cancel_id(id);
                    }
                }
                visitor(
                    HistoricalItem {
                        item_index: *index,
                        original_row_index: *origin,
                        item,
                    },
                    member.clone(),
                    envelope,
                )?;
            }
            Ok(())
        }
    }
    fn envelope(
        proof: Option<EvidenceProvenance>,
        reference: &str,
        id: usize,
        message: &str,
    ) -> String {
        let mut event = Event::empty();
        event.id = id;
        event.event_ref = reference.into();
        event.evidence_provenance = proof;
        event.message = message.into();
        serde_json::to_string(&event).unwrap()
    }
    fn failure(result: Result<Response<FindMembersResult>, String>) -> String {
        match result {
            Err(e) => e,
            Ok(_) => panic!("expected refusal"),
        }
    }

    #[test]
    fn provenance_matches_all_occurrences_ignoring_catalog_body_and_labels() {
        let mut f = Fixture::new();
        let one = f.push(
            json!({"id":"duplicate","stationId":"one"}),
            0,
            envelope(
                Some(provenance("v1")),
                "source:50",
                900,
                "old catalog title",
            ),
        );
        let mut renamed = provenance("v1");
        renamed.source.label = "renamed file".into();
        let two = f.push(
            json!({"id":"duplicate","stationId":"two"}),
            1,
            envelope(Some(renamed), "source:50", 1, "another overlay"),
        );
        f.push(
            json!({"id":"different-source"}),
            2,
            envelope(Some(provenance("v2")), "source:50", 17, "same numeric id"),
        );
        let proof = FindMemberProof::provenance("source:50", &provenance("v1")).unwrap();
        let result = f.run(proof.clone()).unwrap();
        assert_eq!(result.rows[0].state, "ambiguous");
        assert_eq!(result.rows[0].matches.len(), 2);
        assert_eq!(result.rows[0].matches[0].member, one);
        assert_eq!(result.rows[0].matches[1].member, two);
        assert_ne!(
            result.rows[0].matches[0].container_id,
            result.rows[0].matches[1].container_id
        );
        assert_eq!(f.scans.get(), 1);
        assert_eq!(f.visits.get(), 3);
        drop(result);
        f.request.station_id = Some("two".into());
        let result = f.run(proof).unwrap();
        assert_eq!(result.rows[0].state, "unique");
        assert_eq!(result.rows[0].matches[0].item_index, 1);
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(
            wire.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["caseEvidenceSignature", "kind", "rows"]
                .into_iter()
                .collect()
        );
        assert_eq!(wire["rows"][0]["row"]["id"], 17);
        assert!(wire.get("events").is_none());
    }
    #[test]
    fn provenance_mismatch_is_missing_but_missing_proof_is_explicitly_unavailable() {
        let mut f = Fixture::new();
        f.push(
            json!({"id":"one"}),
            0,
            envelope(Some(provenance("v2")), "source:50", 17, "same"),
        );
        let proof = FindMemberProof::provenance("source:50", &provenance("v1")).unwrap();
        assert_eq!(f.run(proof.clone()).unwrap().rows[0].state, "missing");
        f.rows[0].4 = envelope(None, "source:50", 17, "same");
        assert_eq!(failure(f.run(proof)), UNAVAILABLE);
        assert!(FindMemberProof::provenance("source:51", &provenance("v1")).is_err());
    }
    #[test]
    fn exact_member_follows_only_immutable_occurrence_across_manifest_replacement() {
        let mut f = Fixture::new();
        let current = f.push(
            json!({"id":"one"}),
            0,
            envelope(None, "source:50", 17, "same"),
        );
        f.push(
            json!({"id":"one"}),
            1,
            envelope(None, "source:50", 17, "same"),
        );
        let mut original = current.clone();
        original.manifest_id = id();
        let result = f.run(FindMemberProof::member(&original).unwrap()).unwrap();
        assert_eq!(result.rows[0].state, "unique");
        assert_eq!(result.rows[0].matches[0].member, current);
        original.occurrence_id = id();
        assert_eq!(
            f.run(FindMemberProof::member(&original).unwrap())
                .unwrap()
                .rows[0]
                .state,
            "missing"
        );
    }
    #[test]
    fn original_envelope_is_exact_and_supports_uninterpretable_preserved_records() {
        let mut f = Fixture::new();
        let original =
            r#"{"future_schema":3,"raw":[18446744073709551615,1.0,-0.0],"unknown":{"x":true}}"#;
        let found = f.push(json!({"id":null}), 0, original.into());
        f.push(json!({"id":"different"}), 1, original.replace("1.0", "1"));
        let result = f
            .run(FindMemberProof::original_envelope(original).unwrap())
            .unwrap();
        assert_eq!(result.rows[0].state, "unique");
        assert_eq!(result.rows[0].matches[0].member, found);
        assert_eq!(result.rows[0].matches[0].item_id, None);
    }
    #[test]
    fn final_authority_and_source_handle_binding_refuse_partial_results() {
        let mut f = Fixture::new();
        let m = f.push(json!({}), 0, "{}".into());
        f.invalid_final = true;
        assert_eq!(
            failure(f.run(FindMemberProof::member(&m).unwrap())),
            INVALID
        );
        let f = Fixture::new();
        let mut wrong = provenance("v1");
        wrong.source.event_ref_prefix = None;
        assert_eq!(
            failure(f.run(FindMemberProof::provenance("other", &wrong).unwrap())),
            INVALID
        );
        assert_eq!(f.scans.get(), 0);
    }
    #[test]
    fn handles_and_empty_authoritative_case_preserve_order_under_one_scan() {
        let mut f = Fixture::new();
        f.request.rows = (0..MAX_ROWS)
            .map(|id| RowHandle {
                id,
                event_ref: format!("row:{id}"),
            })
            .collect();
        let proof = FindMemberProof::member(&member()).unwrap();
        let calls = Cell::new(0);
        let result = find_members(&f, &f.request, &mut |_| {
            calls.set(calls.get() + 1);
            Ok(proof.clone())
        })
        .unwrap();
        assert_eq!(calls.get(), MAX_ROWS);
        assert_eq!(result.rows.len(), MAX_ROWS);
        assert_eq!(f.scans.get(), 1);
        assert!(result
            .rows
            .iter()
            .enumerate()
            .all(|(id, row)| row.row.id == id && row.state == "missing" && row.matches.is_empty()));
        drop(result);
        f.request.rows.push(RowHandle {
            id: 2001,
            event_ref: "extra".into(),
        });
        assert_eq!(
            failure(find_members(&f, &f.request, &mut |_| panic!(
                "limit must precede source reads"
            ))),
            LIMIT
        );
    }
    #[test]
    fn excessive_ambiguity_fails_without_partial_choices_and_releases_credit() {
        let mut f = Fixture::new();
        let p = provenance("v1");
        for _ in 0..400 {
            f.push(
                json!({"id":"x".repeat(4_096)}),
                0,
                envelope(Some(p.clone()), "source:50", 1, "same"),
            );
        }
        let before = case_work_budget::global().used();
        let error = failure(f.run(FindMemberProof::provenance("source:50", &p).unwrap()));
        assert_eq!(error, AMBIGUITY_LIMIT);
        assert!(f.visits.get() < 400);
        assert_eq!(case_work_budget::global().used(), before);
    }
    #[test]
    fn cancellation_interrupts_one_scan_and_releases_request_work() {
        let mut f = Fixture::new();
        for _ in 0..20 {
            f.push(json!({}), 0, "{}".into());
        }
        let id = format!("members-{}", id());
        f.cancel_after = Some((3, id.clone()));
        let before = case_work_budget::global().used();
        let token = crate::operations::token(Some(id)).unwrap();
        let result = crate::operations::run_with_token(token, || {
            failure(f.run(FindMemberProof::original_envelope("{}").unwrap()))
        });
        assert!(result.unwrap_err().contains("cancel"));
        assert_eq!(f.visits.get(), 3);
        assert_eq!(case_work_budget::global().used(), before);
    }
}
