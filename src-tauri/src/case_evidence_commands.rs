//! Native Case IPC. Returned Serialize wrappers retain their work credits
//! through the IPC boundary; preserved rows never pass through browser Events.
use crate::{
    case_evidence::{self as evidence, *},
    case_evidence_history::{self as history, MemberRequest, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemberDetailRequest {
    pub store: StoreIdentity,
    pub reference: EvidenceReference,
    pub member: MemberHandle,
    pub cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemberFieldRequest {
    pub store: StoreIdentity,
    pub reference: EvidenceReference,
    pub member: MemberHandle,
    pub column: String,
}

#[tauri::command]
pub(crate) async fn cases_load_view(
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<LoadedView, String> {
    crate::offload_operation(operation_id, move || {
        crate::operations::with_reporter(
            Arc::new(move |progress| {
                let _ = app.emit("operation-progress", progress);
            }),
            || load_view_after_verified_adoption(&crate::config_dir()),
        )
    })
    .await?
}
fn error_code(error: &str, code: &str) -> bool {
    error.split_once(':').map_or(error, |(code, _)| code) == code
}
pub(crate) fn load_view_after_verified_adoption(root: &Path) -> Result<LoadedView, String> {
    load_view_with_transition(root).map(|(view, _)| view)
}
pub(crate) fn load_view_with_transition(root: &Path) -> Result<(LoadedView, bool), String> {
    let token = crate::operations::current_token();
    crate::operations::run_with_token(token.clone(), || evidence::bootstrap(root))??;
    match evidence::load_view(root) {
        Ok(view) => return Ok((view, false)),
        Err(error) if error_code(&error, "CASE_EVIDENCE_MIGRATION_REQUIRED") => (),
        Err(error) => return Err(error),
    }
    // Coalesce the expensive recovery copy across processes, then recheck.
    // The final adoption CAS still protects against noncooperating writers.
    let _preparation = evidence::adoption_preparation(root)?;
    match evidence::load_view(root) {
        Ok(view) => return Ok((view, true)),
        Err(error) if error_code(&error, "CASE_EVIDENCE_MIGRATION_REQUIRED") => (),
        Err(error) => return Err(error),
    }
    let cancelled = || token.cancelled();
    let progress = |_phase: &str, completed: u64, total: Option<u64>| {
        crate::operations::report_progress(
            "recuperação",
            "verification",
            "Preservando a investigação antes da atualização",
            completed.min(usize::MAX as u64) as usize,
            total.unwrap_or(0).min(usize::MAX as u64) as usize,
            "bytes",
            0,
        );
    };
    let work = crate::case_recovery::Work {
        cancelled: &cancelled,
        progress: &progress,
    };
    let recovery = crate::case_recovery::prepare(root, &work)?;
    match evidence::adopt(root, &uuid::Uuid::new_v4().to_string(), &recovery) {
        Ok(_) => evidence::load_view(root).map(|view| (view, true)),
        Err(error) if error_code(&error, "CASE_EVIDENCE_ADOPTION_CHANGED") => {
            // A competing successful adoption may have installed authority.
            // Only this exact CAS failure can reconcile through a fresh read.
            match evidence::load_view(root) {
                Ok(view) => Ok((view, true)),
                Err(current) if error_code(&current, "CASE_EVIDENCE_MIGRATION_REQUIRED") => {
                    Err(error)
                }
                Err(current) => Err(current),
            }
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub(crate) async fn cases_save_view(
    request: SaveViewRequest,
    operation_id: Option<String>,
) -> Result<PublishedSave, String> {
    crate::offload_operation(operation_id, move || {
        evidence::save_view(&crate::config_dir(), &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_prepare_membership(
    request: MembershipEdit,
    operation_id: Option<String>,
) -> Result<MembershipReceipt, String> {
    crate::offload_operation(operation_id, move || {
        evidence::prepare_membership(&crate::config_dir(), &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_discard(
    request: DiscardRequest,
    operation_id: Option<String>,
) -> Result<DiscardReceipt, String> {
    crate::offload_operation(operation_id, move || {
        evidence::discard_prepared(&crate::config_dir(), &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_member_detail(
    request: MemberDetailRequest,
    operation_id: Option<String>,
) -> Result<Response<history::MemberDetail>, String> {
    crate::offload_operation(operation_id, move || {
        let member = MemberRequest {
            store: request.store,
            reference: request.reference,
            member: request.member,
        };
        let lease =
            evidence::open_reference(&crate::config_dir(), &member.store, &member.reference)?;
        history::detail(&lease, &member, request.cursor.as_deref())
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_member_field_text(
    request: MemberFieldRequest,
    operation_id: Option<String>,
) -> Result<Response<history::FieldText>, String> {
    crate::offload_operation(operation_id, move || {
        let member = MemberRequest {
            store: request.store,
            reference: request.reference,
            member: request.member,
        };
        let lease =
            evidence::open_reference(&crate::config_dir(), &member.store, &member.reference)?;
        history::field_text(&lease, &member, &request.column)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_member_java_trace(
    request: MemberRequest,
    operation_id: Option<String>,
) -> Result<Response<history::JavaDetail>, String> {
    crate::offload_operation(operation_id, move || {
        let lease =
            evidence::open_reference(&crate::config_dir(), &request.store, &request.reference)?;
        history::java(&lease, &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_timeline(
    request: history::TimelineRequest,
    operation_id: Option<String>,
) -> Result<Response<history::TimelinePage>, String> {
    crate::offload_operation(operation_id, move || {
        let lease = evidence::open_history(
            &crate::config_dir(),
            &request.store,
            &request.owner,
            &request.evidence_signature,
        )?;
        history::timeline(&lease, &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_display_timeline(
    request: crate::case_evidence_display::DisplayRequest,
    operation_id: Option<String>,
) -> Result<Response<crate::case_evidence_display::DisplayPage>, String> {
    crate::offload_operation(operation_id, move || {
        let root = crate::config_dir();
        evidence::validate_view_stamp(&root, &request.store)?;
        let lease = evidence::open_history(
            &root,
            &request.store.identity(),
            &request.owner,
            &request.evidence_signature,
        )?;
        let authority = crate::case_evidence_display::NativeAuthority::new(&root, &lease)?;
        crate::case_evidence_display::timeline(&authority, &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_display_members(
    request: crate::case_evidence_display::MembersRequest,
    operation_id: Option<String>,
) -> Result<Response<crate::case_evidence_display::MembersPage>, String> {
    crate::offload_operation(operation_id, move || {
        let root = crate::config_dir();
        evidence::validate_view_stamp(&root, &request.scope.store)?;
        let lease = evidence::open_history(
            &root,
            &request.scope.store.identity(),
            &request.scope.owner,
            &request.scope.evidence_signature,
        )?;
        let authority = crate::case_evidence_display::NativeAuthority::new(&root, &lease)?;
        crate::case_evidence_display::members(&authority, &request)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_preview(
    request: PreviewRequest,
    operation_id: Option<String>,
) -> Result<Response<EvidencePreviewPage>, String> {
    crate::offload_operation(operation_id, move || {
        let lease =
            evidence::open_reference(&crate::config_dir(), &request.store, &request.reference)?;
        history::preview_page(&lease, &request)
    })
    .await?
}

use crate::{
    analysis_context::Identity,
    analysis_runtime::{self, Admitted},
    case_cache,
    case_evidence_members::{
        self as members, FindMemberProof, FindMembersRequest, FindMembersResult,
    },
    page_projection::{self, Receipt, RowHandle},
    AppState,
};
use serde::Serialize;
use std::{cell::RefCell, collections::BTreeSet, io::Write, path::Path, sync::Arc};
use tauri::{AppHandle, Emitter, Manager};

const INVALID: &str = "CASE_EVIDENCE_SOURCE_CHANGED";
const LIMIT: &str = "CASE_EVIDENCE_CAPTURE_LIMIT";
const UNAVAILABLE: &str = "CASE_MEMBERS_SOURCE_UNAVAILABLE";

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceRequest {
    pub analysis_context: Identity,
    pub source_generation: Option<u64>,
    pub case_key: Option<String>,
    pub case_content_token: Option<String>,
}
impl From<&Receipt> for SourceRequest {
    fn from(receipt: &Receipt) -> Self {
        Self {
            analysis_context: receipt.analysis_context.clone(),
            source_generation: receipt.source_generation,
            case_key: receipt.case_key.clone(),
            case_content_token: receipt.case_content_token.clone(),
        }
    }
}
struct SourceAdmission {
    admitted: Arc<Admitted>,
    receipt: Receipt,
    // Operation-local authority. Its RootLease never persists in this value.
    native: Option<CaseAuthorityLease>,
}
fn capture_source(state: &AppState, request: &SourceRequest) -> Result<SourceAdmission, String> {
    if !matches!(
        (
            request.source_generation,
            request.case_key.as_deref(),
            request.case_content_token.as_deref()
        ),
        (Some(_), None, None) | (None, Some(_), Some(_))
    ) || request
        .case_key
        .as_ref()
        .is_some_and(|k| k.is_empty() || k.len() > 512)
        || request
            .case_content_token
            .as_ref()
            .is_some_and(|k| k.is_empty() || k.len() > 128)
    {
        return Err(INVALID.into());
    }
    let _publication = crate::catalog_read_guard()?;
    let (admitted, records) = analysis_runtime::capture_case_shared(
        state,
        Some(request.analysis_context.clone()),
        request.source_generation,
        None,
        request.case_key.clone(),
    )?;
    if admitted.case_content_token != request.case_content_token
        || (request.case_key.is_some() && records.as_ref().is_none_or(|r| r.native().is_none()))
    {
        return Err(INVALID.into());
    }
    let receipt =
        Receipt::from_admitted(&admitted, &admitted.interpretation.codes, &admitted.interpretation.system_codes)?;
    Ok(SourceAdmission {
        admitted,
        receipt,
        native: None,
    })
}
fn source_from_receipt(state: &AppState, receipt: &Receipt) -> Result<SourceAdmission, String> {
    let captured = capture_source(state, &SourceRequest::from(receipt))?;
    if captured.receipt != *receipt {
        return Err(INVALID.into());
    }
    Ok(captured)
}
impl SourceAdmission {
    fn prepare(&mut self, root: &Path) -> Result<(), String> {
        let admitted = Arc::clone(&self.admitted);
        analysis_runtime::with(Some(admitted.clone()), || {
            if let Some(records) = admitted.native_records() {
                let authority = &records.native().ok_or(INVALID)?.authority;
                // Reopen only the station scope of this source publication.
                // Unselected stations cannot expand the capture's read set.
                self.native = Some(evidence::open_case(
                    root,
                    &NativeCaseOpen {
                        store: authority.store.clone(),
                        analysis_context: self.receipt.analysis_context.clone(),
                        station_id: authority.station_id.clone(),
                        evidence_signature: authority.case_evidence_signature.clone(),
                    },
                )?);
                admitted.prepare_native_case_visibility()
            } else {
                admitted.prepare_visibility(None).map(|_| ())
            }
        })
    }
    fn validate(&self, state: &AppState) -> Result<(), String> {
        self.admitted.validate(state)?;
        self.admitted.validate_visibility()?;
        self.receipt.validate_admitted(
            &self.admitted,
            &self.admitted.interpretation.codes,
            &self.admitted.interpretation.system_codes,
        )?;
        if let Some(native) = &self.native {
            native.validate()?;
        }
        crate::operations::check()
    }
    fn visit_case_rows(
        &self,
        rows: &[RowHandle],
        sink: &mut evidence::RecordSink,
    ) -> Result<(), String> {
        let lease = self.native.as_ref().ok_or(INVALID)?;
        // Only selected positions are retained. Borrowed handles stay owned by
        // the admitted native Records; the request credit covers both vectors.
        let mut selected = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            crate::operations::check()?;
            selected.push((self.admitted.native_case_member(row)?, index));
        }
        selected.sort_unstable_by(|a, b| {
            (&a.0.container_id, &a.0.manifest_id, &a.0.occurrence_id).cmp(&(
                &b.0.container_id,
                &b.0.manifest_id,
                &b.0.occurrence_id,
            ))
        });
        let mut positions = vec![None; rows.len()];
        for (_, container) in lease.case().containers() {
            crate::operations::check()?;
            let reference = container.reference();
            let key = (
                reference.container_id.as_str(),
                reference.manifest_id.as_str(),
            );
            let start = selected.partition_point(|(member, _)| {
                (member.container_id.as_str(), member.manifest_id.as_str()) < key
            });
            let end = selected.partition_point(|(member, _)| {
                (member.container_id.as_str(), member.manifest_id.as_str()) <= key
            });
            let wanted = &selected[start..end];
            if wanted.is_empty() {
                continue;
            }
            // One metadata-only pass per required manifest. No unselected
            // envelope is opened and no full manifest hash map is retained.
            for (ordinal, member) in container.manifest().members.iter().enumerate() {
                if ordinal % 256 == 0 {
                    crate::operations::check()?;
                }
                if let Ok(found) = wanted
                    .binary_search_by(|(handle, _)| handle.occurrence_id.cmp(&member.occurrence_id))
                {
                    let requested = wanted[found].1;
                    if positions[requested]
                        .replace((container.as_ref(), ordinal))
                        .is_some()
                    {
                        return Err(INVALID.into());
                    }
                }
            }
        }
        if positions.iter().any(Option::is_none) {
            return Err(INVALID.into());
        }
        let _encoded =
            case_cache::reserve_work(crate::case_work_budget::global(), evidence::ENVELOPE_BYTES)?;
        for (index, position) in positions.into_iter().enumerate() {
            crate::operations::check()?;
            let (container, ordinal) = position.ok_or(INVALID)?;
            let expected = self.admitted.native_case_member(&rows[index])?;
            container.visit_envelopes(ordinal..ordinal + 1, |member, envelope| {
                if member != *expected {
                    return Err(INVALID.into());
                }
                sink.push_envelope(envelope)?;
                Ok(evidence::Visit::Continue)
            })?;
        }
        lease.validate()
    }
    fn proof(&self, state: &AppState, row: &RowHandle) -> Result<FindMemberProof, String> {
        if self.native.is_some() {
            return FindMemberProof::member(self.admitted.native_case_member(row)?);
        }
        let mut proof = None;
        page_projection::visit_native_dataset_rows(
            state,
            &self.admitted,
            &self.receipt,
            std::slice::from_ref(row),
            |event| {
                // Dataset memory without persisted native provenance has no
                // stable cross-session proof. Its id is never an authority.
                proof = Some(FindMemberProof::provenance(
                    &event.event_ref,
                    event.evidence_provenance.as_ref().ok_or(UNAVAILABLE)?,
                )?);
                Ok(())
            },
        )?;
        proof.ok_or_else(|| UNAVAILABLE.into())
    }
}
struct WireSize {
    bytes: usize,
    limit: usize,
}
impl Write for WireSize {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(data.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other(LIMIT))?;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn request_credit(
    value: &impl Serialize,
    rows: &[RowHandle],
    max_rows: usize,
) -> Result<crate::case_work_budget::Lease, String> {
    if rows.is_empty() || rows.len() > max_rows {
        return Err(LIMIT.into());
    }
    let mut wire = WireSize {
        bytes: 0,
        limit: evidence::REQUEST_BYTES,
    };
    serde_json::to_writer(&mut wire, value).map_err(|_| LIMIT)?;
    let bytes = wire
        .bytes
        .checked_mul(4)
        .and_then(|n| n.checked_add(rows.len().checked_mul(256)?))
        .ok_or(LIMIT)?;
    let credit = case_cache::reserve_work(crate::case_work_budget::global(), bytes)?;
    let mut seen = BTreeSet::new();
    for row in rows {
        if row.id as u128 > 9_007_199_254_740_991u128
            || row.event_ref.is_empty()
            || row.event_ref.len() > 2048
            || !seen.insert((row.id, row.event_ref.as_str()))
        {
            return Err(LIMIT.into());
        }
    }
    Ok(credit)
}

#[tauri::command]
pub(crate) async fn case_evidence_source_receipt(
    request: SourceRequest,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<Receipt, String> {
    crate::offload_operation(operation_id, move || {
        let state = app.state::<AppState>();
        let admission = capture_source(state.inner(), &request)?;
        admission.admitted.validate(state.inner())?;
        admission.receipt.validate_admitted(
            &admission.admitted,
            &admission.admitted.interpretation.codes,
            &admission.admitted.interpretation.system_codes,
        )?;
        Ok(admission.receipt)
    })
    .await?
}

fn prepare_capture_from_source(
    state: &AppState,
    root: &Path,
    request: &CaptureRequest,
) -> Result<PendingEvidenceRef, String> {
    let _credit = request_credit(&request, &request.rows, evidence::CAPTURE_RECORDS)?;
    let captured = RefCell::new(None::<SourceAdmission>);
    // Admission is lazy so durable replay/expired tombstones are checked
    // before any source is reopened or any original is recaptured.
    evidence::prepare_capture(
        root,
        request,
        |sink| {
            let mut source = source_from_receipt(state, &request.source)?;
            source.prepare(root)?;
            analysis_runtime::with(Some(Arc::clone(&source.admitted)), || {
                if source.native.is_some() {
                    source.visit_case_rows(&request.rows, sink)?;
                } else {
                    page_projection::visit_native_dataset_rows(
                        state,
                        &source.admitted,
                        &source.receipt,
                        &request.rows,
                        |event| sink.push_event(&event),
                    )?;
                }
                source.validate(state)
            })?;
            *captured.borrow_mut() = Some(source);
            Ok(())
        },
        || {
            let source = captured.borrow();
            let source = source.as_ref().ok_or(INVALID)?;
            analysis_runtime::with(Some(Arc::clone(&source.admitted)), || {
                source.validate(state)
            })
        },
    )
}
#[tauri::command]
pub(crate) async fn case_evidence_prepare(
    request: CaptureRequest,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<PendingEvidenceRef, String> {
    crate::offload_operation(operation_id, move || {
        prepare_capture_from_source(
            app.state::<AppState>().inner(),
            &crate::config_dir(),
            &request,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn case_evidence_find_members(
    request: FindMembersRequest,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<Response<FindMembersResult>, String> {
    crate::offload_operation(operation_id, move || {
        let _credit = request_credit(&request, &request.rows, 2000)?;
        let state = app.state::<AppState>();
        let root = crate::config_dir();
        let mut source = source_from_receipt(state.inner(), &request.source)?;
        source.prepare(&root)?;
        let lease = evidence::open_history(
            &root,
            &request.store,
            &request.owner,
            &request.case_evidence_signature,
        )?;
        analysis_runtime::with(Some(Arc::clone(&source.admitted)), || {
            let result = members::find_members(&lease, &request, &mut |row| {
                source.proof(state.inner(), row)
            })?;
            source.validate(state.inner())?;
            Ok(result)
        })
    })
    .await?
}

fn validate_open_context(root: &Path, request: &NativeCaseOpen) -> Result<(), String> {
    let conn = crate::case_store::context_connection(root)?;
    let (snapshot, _credit) =
        crate::analysis_context::read_snapshot_leased(&conn, &request.analysis_context.case_id)?;
    if snapshot.identity() != request.analysis_context {
        return Err(INVALID.into());
    }
    Ok(())
}
fn build_native_case(
    lease: Arc<CaseAuthorityLease>,
    request: &NativeCaseOpen,
) -> Result<case_cache::Records, String> {
    let (signature, count) = lease
        .case()
        .station_signature(request.station_id.as_deref())?;
    let authority = case_cache::NativeAuthority {
        store: request.store.clone(),
        owner: lease.case().owner().clone(),
        case_evidence_signature: request.evidence_signature.clone(),
        evidence_signature: signature,
        station_id: request.station_id.clone(),
        preserved_count: count,
    };
    let expected = authority.clone();
    let validate = Arc::clone(&lease);
    let guard = case_cache::NativeGuard {
        storage_bytes: lease.storage_bytes(),
        lease: lease.clone(),
        validate: Arc::new(move |authority| {
            if authority.store != expected.store
                || authority.owner != expected.owner
                || authority.case_evidence_signature != expected.case_evidence_signature
                || authority.evidence_signature != expected.evidence_signature
                || authority.station_id != expected.station_id
                || authority.preserved_count != expected.preserved_count
            {
                return Err(INVALID.into());
            }
            validate.validate()
        }),
    };
    let mut builder = case_cache::NativeBuilder::new(authority, guard)?;
    let admission = builder.record_admission();
    let items = lease
        .case()
        .metadata()
        .get("items")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for (location, container) in lease.case().containers() {
        let ContainerLocation::ItemRows { index } = location else {
            continue;
        };
        let item = items.get(*index as usize).ok_or(INVALID)?;
        let context = case_cache::AnalyticalItemContext {
            station_id: item.get("stationId").and_then(serde_json::Value::as_str),
            artifact_id: item.get("artifactId"),
            origin: item.get("origin"),
        };
        container.visit_events(
            0..container.reference().member_count as usize,
            |member, plan| {
                admission.reserve(member, plan.encoded_bytes, plan.materialization_credit)
            },
            |member, event, _, credit| {
                builder.push(&context, member, event, credit)?;
                Ok(evidence::Visit::Continue)
            },
        )?;
    }
    lease.validate()?;
    builder.finish()
}

fn open_native_case(root: &Path, request: &NativeCaseOpen) -> Result<NativeCaseOpenResult, String> {
    let lease = Arc::new(evidence::open_case(root, request)?);
    let preserved_count = lease
        .case()
        .station_signature(request.station_id.as_deref())?
        .1;
    let records = match build_native_case(Arc::clone(&lease), request) {
        Ok(records) => records,
        Err(code) if code == crate::case_work_budget::MATERIALIZATION_LIMIT => {
            lease.validate()?;
            validate_open_context(root, request)?;
            return Ok(NativeCaseOpenResult::Unavailable {
                code,
                preserved_count,
                materialization_limit: crate::case_work_budget::global().limits().materialized
                    as u64,
            });
        }
        Err(error) => return Err(error),
    };
    validate_open_context(root, request)?;
    lease.validate()?;
    let publication =
        case_cache::publish_native(format!("native-evidence:{}", uuid::Uuid::new_v4()), records)?;
    Ok(NativeCaseOpenResult::Ready { publication })
}
#[tauri::command]
pub(crate) async fn case_evidence_open(
    request: NativeCaseOpen,
    operation_id: Option<String>,
) -> Result<NativeCaseOpenResult, String> {
    crate::offload_operation(operation_id, move || {
        let snapshot = analysis_runtime::validate_identity(&request.analysis_context)?;
        let preferences = snapshot.interpretation.as_ref().map(|settings| settings.resources.clone()).unwrap_or_default();
        let policy = crate::case_resources::Policy::capture(Some(&request.analysis_context), &preferences)?;
        drop(snapshot);
        crate::case_resources::with(policy, || open_native_case(&crate::config_dir(), &request))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_request_admission_charges_metadata_and_rejects_duplicates_and_escaped_overflow() {
        let before = crate::case_work_budget::global().used();
        let rows = vec![RowHandle {
            id: 0,
            event_ref: "source:0".into(),
        }];
        let credit = request_credit(&rows, &rows, 10_000).unwrap();
        assert!(crate::case_work_budget::global().used() > before);
        drop(credit);
        assert_eq!(crate::case_work_budget::global().used(), before);
        let duplicate = vec![rows[0].clone(), rows[0].clone()];
        assert!(request_credit(&duplicate, &duplicate, 10_000).is_err());
        let escaped = (0..1000)
            .map(|id| RowHandle {
                id,
                event_ref: "\0".repeat(2048),
            })
            .collect::<Vec<_>>();
        assert!(request_credit(&escaped, &escaped, 10_000).is_err());
        assert_eq!(crate::case_work_budget::global().used(), before);
    }
    #[test]
    fn source_and_member_commands_refuse_browser_records_instead_of_adopting_them() {
        let request = serde_json::json!({
            "analysisContext": {"caseId":"case", "analysisId":"analysis", "configRevision":0, "visibilityRevision":0},
            "sourceGeneration": 1, "caseKey": null, "caseContentToken": null,
            "events": [{"id":0,"raw":"fabricated"}]
        });
        assert!(serde_json::from_value::<SourceRequest>(request).is_err());
        let detail = serde_json::json!({"store":{"storeId":"store","epoch":"epoch"}, "reference":{}, "member":{}, "event":{"id":0}, "cursor":null});
        assert!(serde_json::from_value::<MemberDetailRequest>(detail).is_err());
    }
}

#[cfg(test)]
#[path = "case_evidence_app_tests.rs"]
mod app_tests;
