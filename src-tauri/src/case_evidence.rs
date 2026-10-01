//! Native Case evidence contract. Immutable record envelopes and ordered
//! occurrence manifests are authoritative; management previews never are.
//! Storage/admission activation is gated on the complete recovery/save/open/
//! portable path. Merely declaring these types performs no schema migration.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
#[path = "case_evidence_history_tests.rs"]
mod history_tests;
#[path = "case_evidence_typed.rs"]
mod typed;
pub(crate) use typed::{
    deserialize_value as native_deserialize_value,
    deserialize_values as native_deserialize_values,
    deserialize_snapshot as native_deserialize_snapshot,
    deserialize_snapshots as native_deserialize_snapshots,
    snapshot_from_value as native_snapshot_from_value,
};
#[path = "case_evidence_json.rs"]
mod raw_json;
pub(crate) use raw_json::RawJson;
/// Recovery inspects bounded borrowed metadata spans without interpreting
/// unrelated record or authored numeric values.
pub(crate) fn checked_document(text: &str) -> Result<RawJson<'_>, String> {
    crate::operations::check()?;
    let value = RawJson::checked_with_limit(text, 64 << 20)?;
    crate::operations::check()?;
    Ok(value)
}
#[path = "case_evidence_db.rs"]
mod db;
pub(crate) use db::{
    native_dependencies, restore_epoch, snapshot_authority, AuthoritySnapshot, NativeDependency,
};
#[path = "case_evidence_authority.rs"]
mod authority;
pub(crate) use authority::{
    native_owner, open_case, open_history, open_reference, require_legacy_store,
    validate_case_authority, validate_view_stamp, CaseAuthorityLease, ReferenceAuthorityLease,
};
#[path = "case_evidence_adoption_gate.rs"]
mod adoption_gate;
pub(crate) use adoption_gate::{adoption_preparation, AdoptionPreparation};
#[path = "case_evidence_bootstrap.rs"]
mod bootstrap;
pub(crate) use bootstrap::{bootstrap, BootstrapReceipt};
#[path = "case_evidence_adopt.rs"]
mod adopt;
pub(crate) use adopt::{adopt, PublishedAdoption};
#[path = "case_evidence_anchor_store.rs"]
mod anchor_store;
#[path = "case_evidence_assemble.rs"]
mod assemble;
#[path = "case_evidence_body.rs"]
mod body;
#[path = "case_evidence_case.rs"]
mod prepared_case;
pub(crate) use prepared_case::{
    prepare_import_case, prepare_import_case_with_transfer, snapshot_transport_safe,
    CommittedCaseView, PortableCaseMap, PortableContainerMap, PortableMemberMap, PreparedCase,
};
#[path = "case_evidence_receipts.rs"]
mod receipts;
pub(crate) use body::{read_case_body, OriginalBody};
#[path = "case_evidence_view.rs"]
mod view;
pub(crate) use view::{load_view, LoadedView};
#[path = "case_evidence_commit.rs"]
mod commit;
pub(crate) use commit::{
    commit_import, replay_import, save_view, CommittedImport, ImportFingerprint, ImportFormat,
    PublishedSave,
};
#[path = "case_evidence_export.rs"]
mod export;
pub(crate) use export::{capture_export, capture_transfer_map, ExportCapture, ExportCase};
#[path = "case_evidence_prepare.rs"]
mod prepare;
pub(crate) use prepare::{discard_prepared, prepare_capture, prepare_membership};

#[path = "case_evidence_storage.rs"]
mod storage;
pub(crate) use storage::{
    clear_cached_readers, stage_records, PreparedContainer, RecordSink, VerifiedBatch,
    VerifiedContainer, Visit,
};
#[path = "case_evidence_decode.rs"]
mod decode;
pub(crate) use decode::{
    materialize_event, materialize_value, metadata_transport_safe, parse_exact_value,
    parse_view_document, preflight_envelope, preflight_value, ExactValue, RecordPlan,
};
#[path = "case_evidence_document.rs"]
mod document;
pub(crate) use document::{
    extract_case, ContainerLocation, ExtractedCase, RawContainer, VerifiedCase,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const CAPTURE_SOURCE_BYTES: usize = 2 << 20;
pub(crate) const EVENT_OWNED_BYTES: usize = 4 << 20;
pub(crate) const ENVELOPE_BYTES: usize = 8 << 20;
pub(crate) const CAPTURE_RECORDS: usize = 10_000;
pub(crate) const CAPTURE_BYTES: u64 = 32 << 20;
pub(crate) const REQUEST_BYTES: usize = 2 << 20;
pub(crate) const VIEW_DOCUMENT_BYTES: usize = 16 << 20;
pub(crate) const MANIFEST_MEMBERS: usize = 100_000;
pub(crate) const MANIFEST_BYTES: u64 = 8 << 20;
pub(crate) const PREVIEW_ROWS: u16 = 16;
pub(crate) const PREVIEW_COLUMNS: usize = 8;
pub(crate) const PREVIEW_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoreIdentity {
    pub store_id: String,
    pub epoch: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoreStamp {
    pub store_id: String,
    pub epoch: String,
    /// Canonical decimal u64, never an imprecise JavaScript number.
    pub revision: String,
}
impl StoreStamp {
    pub(crate) fn identity(&self) -> StoreIdentity {
        StoreIdentity {
            store_id: self.store_id.clone(),
            epoch: self.epoch.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceOwner {
    pub store_id: String,
    pub case_id: String,
    pub analysis_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BatchRef {
    pub schema_version: u32,
    pub owner: EvidenceOwner,
    pub batch_id: String,
    pub sha256: String,
    pub records: u32,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordLocator {
    pub batch_id: String,
    pub ordinal: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Member {
    pub occurrence_id: String,
    /// Original position in this container, never renumbered after removal.
    pub origin_position: u32,
    pub record: RecordLocator,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContainerManifest {
    pub schema_version: u32,
    pub owner: EvidenceOwner,
    pub container_id: String,
    pub manifest_id: String,
    /// Native/storage only. Apply BOTH byte and count budgets while building.
    pub members: Vec<Member>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum CommittedKind {
    #[serde(rename = "native_evidence")]
    NativeEvidence,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceRef {
    pub kind: CommittedKind,
    pub schema_version: u32,
    pub owner: EvidenceOwner,
    pub container_id: String,
    pub manifest_id: String,
    pub manifest_sha256: String,
    pub member_count: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum PendingKind {
    #[serde(rename = "pending_native_evidence")]
    PendingNativeEvidence,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PreparedPurpose {
    NewContainer {},
    ReplaceContainer {
        #[serde(rename = "baseManifestId")]
        base_manifest_id: String,
        #[serde(rename = "baseManifestSha256")]
        base_manifest_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PendingEvidenceRef {
    pub kind: PendingKind,
    pub purpose: PreparedPurpose,
    pub schema_version: u32,
    pub token: String,
    pub request_id: String,
    pub owner: EvidenceOwner,
    pub container_id: String,
    pub manifest_id: String,
    pub manifest_sha256: String,
    pub member_count: u32,
    pub bytes: u64,
    /// Informational UTC wall clock; backend admission owns actual expiry.
    pub expires_at: String,
}

/// The inner structs have disjoint mandatory kind tags and reject extra fields.
/// This is never an Event/preview union, even though serde tries two shapes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum EvidenceReference {
    Committed(EvidenceRef),
    Prepared(PendingEvidenceRef),
}
impl EvidenceReference {
    pub(crate) fn owner(&self) -> &EvidenceOwner {
        match self {
            Self::Committed(value) => &value.owner,
            Self::Prepared(value) => &value.owner,
        }
    }
    pub(crate) fn container_id(&self) -> &str {
        match self {
            Self::Committed(value) => &value.container_id,
            Self::Prepared(value) => &value.container_id,
        }
    }
    pub(crate) fn manifest_id(&self) -> &str {
        match self {
            Self::Committed(value) => &value.manifest_id,
            Self::Prepared(value) => &value.manifest_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemberHandle {
    pub container_id: String,
    pub manifest_id: String,
    pub occurrence_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidencePreviewRow {
    pub kind: &'static str, // "evidence_preview"
    pub member: MemberHandle,
    pub cells: Vec<crate::page_projection::Cell>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidencePreviewPage {
    pub kind: &'static str, // "evidence_preview_page"
    pub columns: Vec<String>,
    pub rows: Vec<EvidencePreviewRow>,
    pub total: u32,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerView {
    pub kind: &'static str, // "native_evidence_container"
    pub reference: EvidenceReference,
    pub preserved_count: u32,
    pub preview: Option<EvidencePreviewPage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaseEvidenceSummary {
    pub owner: EvidenceOwner,
    /// Complete ordered items[].rows configuration, including station and
    /// artifact/origin fallback inputs; unrelated notes do not affect it.
    pub evidence_signature: String,
    pub preserved_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_item_aliases: Vec<crate::case_evidence_anchors::BlockedAlias>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreservedCaseDiagnostic {
    pub case_id: String,
    pub owner: Option<EvidenceOwner>,
    pub code: String,
    pub message: String,
    pub preserved_count: Option<u32>,
    pub readiness: CaseReadiness,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaseReadiness {
    PreservedOnly,
    MigrationRequired,
    RecoveryRequired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum CaseEvidenceState {
    Ready(CaseEvidenceSummary),
    Unavailable(PreservedCaseDiagnostic),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum UnavailableCaseKind {
    #[serde(rename = "preserved_case_unavailable")]
    PreservedCaseUnavailable,
}

/// The only admitted representation when even authored Case metadata cannot
/// be exposed safely. A save preserves its existing body; it never stores this
/// stub as a replacement body or accepts it for a ready Case.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnavailableCaseStub {
    pub kind: UnavailableCaseKind,
    pub id: String,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaseViewDocument {
    pub evidence_view_version: u32,
    pub store: StoreStamp,
    pub active: Option<String>,
    /// Authored JSON with recognized record arrays replaced by ContainerView.
    #[serde(deserialize_with = "native_deserialize_values")]
    pub cases: Vec<Value>,
    pub case_evidence: Vec<CaseEvidenceState>,
    pub diagnostics: Vec<PreservedCaseDiagnostic>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaptureRequest {
    pub request_id: String,
    pub expected_store: StoreStamp,
    pub target: EvidenceOwner,
    pub source: crate::page_projection::Receipt,
    pub rows: Vec<crate::page_projection::RowHandle>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MembershipAction {
    Remove { members: Vec<MemberHandle> },
    Restore { target: EvidenceReference },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MembershipEdit {
    pub request_id: String,
    pub expected_store: StoreStamp,
    pub reference: EvidenceReference,
    pub action: MembershipAction,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MembershipReceipt {
    pub reference: PendingEvidenceRef,
    /// The exact preceding reference, for unsaved Undo or an explicit later
    /// Restore against the current committed manifest. Never replay old notes.
    pub undo: EvidenceReference,
    pub changed_count: u32,
    pub remaining_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveViewRequest {
    pub request_id: String,
    pub expected_store: StoreStamp,
    /// Exact inner JSON, never parsed through Tauri Value before admission.
    pub document_json: String,
}
impl SaveViewRequest {
    pub(crate) fn parse_document(&self) -> Result<CaseViewDocument, String> {
        let document = parse_view_document(&self.document_json)?;
        if document.store != self.expected_store {
            return Err("CASE_EVIDENCE_VIEW_STALE: O documento e a revisão esperada não pertencem ao mesmo recibo.".into());
        }
        Ok(document)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveViewReceipt {
    pub request_id: String,
    pub committed_store: StoreStamp,
    pub current_store: StoreStamp,
    pub evidence: Vec<EvidenceRef>,
    #[serde(deserialize_with = "native_deserialize_snapshots")]
    pub analysis_contexts: Vec<crate::analysis_context::Snapshot>,
    /// Signatures at committed_store, never relabeled as current on replay.
    pub case_evidence: Vec<CaseEvidenceState>,
    pub replayed: bool,
    pub reconcile_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeCaseOpen {
    pub store: StoreIdentity,
    pub analysis_context: crate::analysis_context::Identity,
    pub station_id: Option<String>,
    /// From CaseViewDocument.caseEvidence, never recomputed by JavaScript.
    pub evidence_signature: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeCaseReceipt {
    pub case_key: String,
    pub case_content_token: String,
    pub case_evidence_signature: String,
    /// Actual station-subset publication signature.
    pub evidence_signature: String,
    pub preserved_count: u32,
    pub analytical_count: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum NativeCaseOpenResult {
    Ready {
        publication: NativeCaseReceipt,
    },
    Unavailable {
        code: String,
        #[serde(rename = "preservedCount")]
        preserved_count: u32,
        #[serde(rename = "materializationLimit")]
        materialization_limit: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreviewRequest {
    pub store: StoreIdentity,
    pub reference: EvidenceReference,
    pub cursor: Option<String>,
    pub limit: u16,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryReceipt {
    pub schema_version: u32,
    pub recovery_id: String,
    pub source: RecoverySource,
    pub manifest_sha256: String,
    /// Returned verified fact, never trusted from an incoming wire value.
    pub verified: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RecoverySource {
    Legacy {
        #[serde(rename = "bodyRevision")]
        body_revision: String,
    },
    Native {
        store: StoreStamp,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdoptionReceipt {
    pub request_id: String,
    pub recovery: RecoveryReceipt,
    pub before: RecoverySource,
    pub after: StoreStamp,
    pub current_store: StoreStamp,
    pub replayed: bool,
    pub reconcile_required: bool,
    pub adopted: Vec<EvidenceOwner>,
    pub unavailable: Vec<PreservedCaseDiagnostic>,
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    fn owner() -> EvidenceOwner {
        EvidenceOwner {
            store_id: "store".into(),
            case_id: "case".into(),
            analysis_id: "analysis".into(),
        }
    }
    fn reference() -> EvidenceRef {
        EvidenceRef {
            kind: CommittedKind::NativeEvidence,
            schema_version: 1,
            owner: owner(),
            container_id: "container".into(),
            manifest_id: "before".into(),
            manifest_sha256: "a".repeat(64),
            member_count: 2,
        }
    }
    #[test]
    fn reference_tags_and_replacement_bases_are_not_interchangeable() {
        let committed = EvidenceReference::Committed(reference());
        let encoded = serde_json::to_value(&committed).unwrap();
        assert_eq!(encoded["kind"], "native_evidence");
        assert_eq!(
            serde_json::from_value::<EvidenceReference>(encoded.clone()).unwrap(),
            committed
        );
        let pending = EvidenceReference::Prepared(PendingEvidenceRef {
            kind: PendingKind::PendingNativeEvidence,
            purpose: PreparedPurpose::ReplaceContainer {
                base_manifest_id: "before".into(),
                base_manifest_sha256: "a".repeat(64),
            },
            schema_version: 1,
            token: "token".into(),
            request_id: "request".into(),
            owner: owner(),
            container_id: "container".into(),
            manifest_id: "after".into(),
            manifest_sha256: "b".repeat(64),
            member_count: 1,
            bytes: 123,
            expires_at: "2026-10-01T14:00:00Z".into(),
        });
        let value = serde_json::to_value(&pending).unwrap();
        assert_eq!(value["kind"], "pending_native_evidence");
        assert_eq!(value["purpose"]["kind"], "replace_container");
        assert_eq!(value["purpose"]["baseManifestId"], "before");
        assert_eq!(
            serde_json::from_value::<EvidenceReference>(value.clone()).unwrap(),
            pending
        );
        let mut forged = encoded;
        forged["kind"] = Value::from("evidence_preview");
        assert!(serde_json::from_value::<EvidenceReference>(forged).is_err());
        let mut mixed = value;
        mixed["purpose"]["kind"] = Value::from("new_container");
        assert!(serde_json::from_value::<EvidenceReference>(mixed).is_err());
    }
    #[test]
    fn empty_case_publication_and_unavailable_are_distinct_wire_states() {
        let ready = NativeCaseOpenResult::Ready {
            publication: NativeCaseReceipt {
                case_key: "empty-case".into(),
                case_content_token: "token".into(),
                case_evidence_signature: "full".into(),
                evidence_signature: "station".into(),
                preserved_count: 0,
                analytical_count: 0,
            },
        };
        let ready = serde_json::to_value(ready).unwrap();
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["publication"]["analyticalCount"], 0);
        let unavailable = serde_json::to_value(NativeCaseOpenResult::Unavailable {
            code: "materialization_limit".into(),
            preserved_count: 20,
            materialization_limit: 16 << 20,
        })
        .unwrap();
        assert_eq!(unavailable["state"], "unavailable");
        assert_eq!(unavailable["preservedCount"], 20);
        assert!(unavailable.get("publication").is_none());
    }
    #[test]
    fn store_revision_remains_text_across_the_javascript_integer_boundary() {
        let stamp = StoreStamp {
            store_id: "store".into(),
            epoch: "epoch".into(),
            revision: u64::MAX.to_string(),
        };
        let json = serde_json::to_value(&stamp).unwrap();
        assert_eq!(json["revision"].as_str(), Some("18446744073709551615"));
        assert_eq!(serde_json::from_value::<StoreStamp>(json).unwrap(), stamp);
    }

    #[test]
    fn unavailable_case_preserves_identity_and_known_count_without_an_empty_signature() {
        let state = CaseEvidenceState::Unavailable(PreservedCaseDiagnostic {
            case_id: "case".into(),
            owner: Some(owner()),
            code: "unsupported_record".into(),
            message: "Registros preservados; interpretação analítica indisponível.".into(),
            preserved_count: Some(42),
            readiness: CaseReadiness::PreservedOnly,
        });
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["state"], "unavailable");
        assert_eq!(value["caseId"], "case");
        assert_eq!(value["preservedCount"], 42);
        assert_eq!(value["readiness"], "preserved_only");
        assert!(value.get("evidenceSignature").is_none());
        assert_eq!(
            serde_json::from_value::<CaseEvidenceState>(value).unwrap(),
            state
        );
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscardRequest {
    pub store: StoreIdentity,
    pub reference: PendingEvidenceRef,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscardReceipt {
    pub discarded: bool,
}
