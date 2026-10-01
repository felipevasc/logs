//! Reversible archive commands. Membership publication is a short revision-
//! checked transaction; expensive selection preparation happens beforehand.
use crate::{
    analysis_context::Identity,
    exclusion_store::{self, Admission, Budget, Member, Receipt, Work},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

// Ordinary readers enforce the admitted mandatory visibility scope; originals
// remain available only through the explicit, revision-bound archive reader.
const ENABLED: bool = true;
const PREVIEW_TTL: Duration = Duration::from_secs(600);
const MAX_PREVIEWS: usize = 2;
const MAX_RECEIPTS: usize = 8;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum PreviewScope {
    Selected { ids: Vec<usize> },
    Filtered { filters: Vec<crate::query::Filter> },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewReply {
    preview_token: String,
    selected_members: u64,
    analysis_context: Identity,
    source_generation: Option<u64>,
}
struct PendingPreview {
    staged: exclusion_store::Staged,
    admitted: std::sync::Arc<crate::analysis_runtime::Admitted>,
    scope: Value,
}
static PREVIEWS: std::sync::LazyLock<parking_lot::Mutex<PreviewRegistry<PendingPreview, Receipt>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(PreviewRegistry::new()));
static PENDING_SWEEP: std::sync::LazyLock<parking_lot::Mutex<exclusion_store::PendingSweep>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(exclusion_store::PendingSweep::new()));

async fn offload_archive<T: Send + 'static>(
    operation_id: Option<String>,
    app: tauri::AppHandle,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    crate::offload_operation(operation_id, move || {
        crate::operations::with_reporter(
            std::sync::Arc::new(move |progress| {
                let _ = app.emit("operation-progress", progress);
            }),
            f,
        )
    })
    .await?
}

fn require_enabled() -> Result<(), String> {
    if ENABLED {
        Ok(())
    } else {
        Err("O arquivo de exclusões está em preparação nesta versão.".into())
    }
}

fn selected_ids(scope: &PreviewScope) -> Result<Option<std::collections::BTreeSet<usize>>, String> {
    if let PreviewScope::Selected { ids } = scope {
        if ids.len() > 100_000 {
            return Err(
                "A seleção excede 100.000 IDs. Use o recorte filtrado para preparar um lote maior."
                    .into(),
            );
        }
        return Ok(Some(ids.iter().copied().collect()));
    }
    Ok(None)
}
fn filters(scope: &PreviewScope) -> &[crate::query::Filter] {
    match scope {
        PreviewScope::Filtered { filters } => filters,
        PreviewScope::Selected { .. } => &[],
    }
}

fn stage_evidence(
    dir: &std::path::Path,
    admission: Admission,
    events: &[crate::model::Event],
    scope: &PreviewScope,
) -> Result<exclusion_store::Staged, String> {
    let ids = selected_ids(scope)?;
    crate::workspace::validate(filters(scope))?;
    let prepared = crate::query::prepare(filters(scope));
    let mut descriptors = Vec::new();
    let mut dictionary = std::collections::BTreeMap::new();
    let mut members = Vec::new();
    let mut matched = std::collections::BTreeSet::new();
    let mut charged = 0usize;
    let budget = crate::analysis_visibility::MaskBudget::default();
    for event in events {
        crate::operations::check()?;
        if ids.as_ref().is_some_and(|ids| !ids.contains(&event.id))
            || !prepared
                .iter()
                .all(|filter| crate::query::matches(event, filter))
        {
            continue;
        }
        let proof = event.evidence_provenance.as_ref().ok_or("Este registro legado não tem proveniência persistida. Reabra a fonte original e capture o registro novamente antes de excluí-lo.")?;
        if event.event_ref.is_empty() {
            return Err("Registro sem referência estável para exclusão.".into());
        }
        if let (Some(prefix), exclusion_store::Locator::ByteOffset(offset)) =
            (&proof.source.event_ref_prefix, &proof.locator)
        {
            if event.event_ref != format!("{prefix}:{offset}") {
                return Err("A proveniência da evidência não corresponde ao registro.".into());
            }
        }
        let key = proof.source.key()?;
        let source_index = if let Some(index) = dictionary.get(&key) {
            *index
        } else {
            charged = charged
                .checked_add(
                    serde_json::to_vec(&proof.source)
                        .map_err(|e| e.to_string())?
                        .len()
                        + key.len()
                        + 64,
                )
                .ok_or("Proveniência excessiva.")?;
            let index = descriptors.len();
            descriptors.push(proof.source.clone());
            dictionary.insert(key, index);
            index
        };
        charged = charged
            .checked_add(
                event.event_ref.len()
                    + serde_json::to_vec(&proof.locator)
                        .map_err(|e| e.to_string())?
                        .len()
                    + 64,
            )
            .ok_or("Seleção de evidência excessiva.")?;
        if charged > budget.evidence_bytes {
            return Err("A seleção de evidência excede o orçamento de preparação.".into());
        }
        matched.insert(event.id);
        members.push(exclusion_store::InputMember {
            source_index,
            locator: proof.locator.clone(),
            event_ref: event.event_ref.clone(),
        });
    }
    if ids.as_ref().is_some_and(|ids| ids != &matched) {
        return Err(
            "A seleção contém registros que não estão mais visíveis. Reabra a seleção.".into(),
        );
    }
    exclusion_store::stage(
        dir,
        admission,
        exclusion_store::Purpose::Exclude,
        &descriptors,
        members.into_iter().map(Ok),
        &Budget::default(),
        &work(),
    )
}

fn prepare_preview(
    state: &crate::AppState,
    admitted: std::sync::Arc<crate::analysis_runtime::Admitted>,
    case_events: Option<Vec<crate::model::Event>>,
    scope: PreviewScope,
) -> Result<PreviewReply, String> {
    crate::workspace::validate(filters(&scope))?;
    let identity = admitted
        .identity
        .clone()
        .ok_or("Abra um Caso antes de excluir registros.")?;
    let scope_json = serde_json::to_value(&scope).map_err(|e| e.to_string())?;
    if serde_json::to_vec(&scope_json)
        .map_err(|e| e.to_string())?
        .len()
        > 64 << 10
    {
        return Err("A descrição do recorte excede o limite de preparação.".into());
    }
    let receipt = json!({"generation":admitted.source_generation,"analysisContext":identity,"caseKey":admitted.case_key});
    let admission = Admission {
        analysis: identity.clone(),
        source_receipt: receipt,
    };
    let dir = crate::config_dir();
    // Reclaim only recognized abandoned stages; a live owner lease always wins.
    // Cleanup is bounded and optional, while cancellation still stops staging.
    // Keep the directory cursor so stable published entries cannot starve later
    // orphan cleanup. Another preview may skip maintenance rather than queue.
    if let Some(mut sweep) = PENDING_SWEEP.try_lock() {
        let _ = sweep.pass(&dir, PREVIEW_TTL, 512, &work());
    }
    crate::operations::check()?;
    let staged = if let Some(events) = case_events {
        stage_evidence(&dir, admission, &events, &scope)?
    } else {
        let source = crate::analysis_runtime::source(state);
        match &*source {
            crate::SourceData::Indexed(index) => {
                let budget = crate::analysis_visibility::MaskBudget::default();
                if crate::analysis_visibility::required_mask_bytes(index.lines.len())?
                    > budget.mask_bytes
                {
                    return Err("A fonte excede o orçamento atual da máscara de exclusão. Nenhum lote foi publicado.".into());
                }
                let binding =
                    crate::analysis_visibility::SourceSet::prepare(index, &budget, &work())?;
                if let Some(ids) = selected_ids(&scope)? {
                    let members = ids.into_iter().map(|id| {
                        crate::operations::check()?;
                        if !crate::analysis_runtime::row_visible(index, id)? {
                            return Err(
                                "A seleção contém registros que não estão mais visíveis.".into()
                            );
                        }
                        binding.member_row(index, id, &budget)
                    });
                    exclusion_store::stage(
                        &dir,
                        admission,
                        exclusion_store::Purpose::Exclude,
                        binding.descriptors(),
                        members,
                        &Budget::default(),
                        &work(),
                    )?
                } else {
                    crate::workspace::with_selection(state, filters(&scope), |selection| {
                        exclusion_store::stage_with(
                            &dir,
                            admission,
                            exclusion_store::Purpose::Exclude,
                            binding.descriptors(),
                            |push| {
                                let exact = selection.visit_exact_ids(|id| {
                                    push(binding.member_row(index, id, &budget)?)?;
                                    Ok(true)
                                })?;
                                if exact.is_none() {
                                    for event in selection.iter() {
                                        push(binding.member(
                                            index,
                                            event.id,
                                            &event.event_ref,
                                            &budget,
                                        )?)?;
                                    }
                                }
                                Ok(())
                            },
                            &Budget::default(),
                            &work(),
                        )
                    })??
                }
            }
            crate::SourceData::Memory(events) => stage_evidence(&dir, admission, events, &scope)?,
            crate::SourceData::None => {
                return Err("Abra uma fonte antes de preparar uma exclusão.".into())
            }
        }
    };
    admitted.validate(state)?;
    crate::operations::check()?;
    let selected_members = staged.members();
    let generation = admitted.source_generation;
    let token = PREVIEWS.lock().insert(
        identity.clone(),
        generation,
        PendingPreview {
            staged,
            admitted,
            scope: scope_json,
        },
        Instant::now(),
    )?;
    // Deliver the prepared token even if cancellation arrives at this boundary;
    // the caller can then explicitly discard its owned temporary payload.
    crate::operations::commit();
    Ok(PreviewReply {
        preview_token: token,
        selected_members,
        analysis_context: identity,
        source_generation: generation,
    })
}

#[tauri::command]
pub(crate) async fn exclusion_preview(
    app: tauri::AppHandle,
    analysis_context: Identity,
    source_generation: Option<u64>,
    scope: PreviewScope,
    case_events: Option<Vec<crate::model::Event>>,
    case_key: Option<String>,
    operation_id: Option<String>,
) -> Result<PreviewReply, String> {
    require_enabled()?;
    let (admitted, events) = crate::analysis_runtime::capture_case(
        app.state::<crate::AppState>().inner(),
        Some(analysis_context),
        source_generation,
        case_events,
        case_key,
    )?;
    let captured = admitted.clone();
    crate::offload_case(operation_id, app.clone(), admitted, events, move |events| {
        prepare_preview(
            app.state::<crate::AppState>().inner(),
            captured,
            events,
            scope,
        )
    })
    .await?
}

struct PublicationGuard {
    token: String,
    finished: bool,
}
impl Drop for PublicationGuard {
    fn drop(&mut self) {
        if !self.finished {
            PREVIEWS.lock().finish(&self.token, None, Instant::now());
        }
    }
}
#[tauri::command]
pub(crate) async fn exclusion_commit(
    app: tauri::AppHandle,
    preview_token: String,
    analysis_context: Identity,
    source_generation: Option<u64>,
    label: Option<String>,
    reason: Option<String>,
    operation_id: Option<String>,
) -> Result<Receipt, String> {
    require_enabled()?;
    offload_archive(operation_id, app.clone(), move || {
        commit_preview(
            app.state::<crate::AppState>().inner(),
            &preview_token,
            &analysis_context,
            source_generation,
            label.as_deref().unwrap_or(""),
            reason.as_deref().unwrap_or(""),
        )
    })
    .await
}
fn commit_preview(
    state: &crate::AppState,
    preview_token: &str,
    analysis_context: &Identity,
    source_generation: Option<u64>,
    label: &str,
    reason: &str,
) -> Result<Receipt, String> {
    let pending = PREVIEWS.lock().begin(
        &preview_token,
        &analysis_context,
        source_generation,
        Instant::now(),
    )?;
    let Begin::Publish(pending) = pending else {
        let Begin::Replay(receipt) = pending else {
            unreachable!()
        };
        crate::operations::commit();
        return Ok(receipt);
    };
    let mut guard = PublicationGuard {
        token: preview_token.to_owned(),
        finished: false,
    };
    if pending.staged.members() == 0 {
        return Err("A prévia não contém registros para excluir.".into());
    }
    pending.admitted.validate(state)?;
    let _source = state.source.read();
    pending.admitted.validate_publication(state, false)?;
    crate::operations::check()?;
    let receipt = exclusion_store::publish(
        &crate::config_dir(),
        pending.staged,
        label,
        reason,
        pending.scope,
        &Budget::default(),
        &work(),
    )?;
    crate::operations::commit();
    PREVIEWS
        .lock()
        .finish(&preview_token, Some(receipt.clone()), Instant::now());
    guard.finished = true;
    Ok(receipt)
}

#[tauri::command]
pub(crate) async fn exclusion_discard(
    preview_token: String,
    analysis_context: Identity,
    source_generation: Option<u64>,
) -> Result<bool, String> {
    crate::offload(move || {
        PREVIEWS
            .lock()
            .discard(&preview_token, &analysis_context, source_generation)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveReply {
    analysis: Identity,
    batch: BatchSummary,
    rows: Vec<crate::analysis_runtime::ArchiveRecord>,
    active_members: Option<u64>,
    sources: std::collections::BTreeMap<String, exclusion_store::SourceDescriptor>,
    next_cursor: Option<exclusion_store::Key>,
}
#[tauri::command]
pub(crate) async fn exclusion_archive_page(
    app: tauri::AppHandle,
    analysis_context: Identity,
    source_generation: Option<u64>,
    batch_id: String,
    cursor: Option<exclusion_store::Key>,
    limit: Option<usize>,
    case_events: Option<Vec<crate::model::Event>>,
    case_key: Option<String>,
    operation_id: Option<String>,
) -> Result<ArchiveReply, String> {
    let source = crate::analysis_runtime::capture_archive_case(
        app.state::<crate::AppState>().inner(),
        analysis_context.clone(),
        source_generation,
        case_events,
        case_key,
    )?;
    offload_archive(operation_id, app.clone(), move || {
        let page = exclusion_store::archive_page(
            &crate::config_dir(),
            &analysis_context,
            &batch_id,
            cursor.as_ref(),
            limit.unwrap_or(50),
            &work(),
        )?;
        let rows = source.resolve(app.state::<crate::AppState>().inner(), &page)?;
        Ok(ArchiveReply {
            analysis: page.analysis,
            batch: page.batch.into(),
            rows,
            active_members: page.active_members,
            sources: page.sources,
            next_cursor: page.next_cursor,
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capabilities {
    available: bool,
    reason: Option<&'static str>,
}

#[tauri::command]
pub(crate) async fn exclusion_capabilities(
    analysis_context: Identity,
) -> Result<Capabilities, String> {
    crate::offload(move || {
        crate::analysis_runtime::validate_identity(&analysis_context)?;
        PREVIEWS.lock().prune(Instant::now());
        Ok(Capabilities {
            available: ENABLED,
            reason: (!ENABLED).then_some("O arquivo de exclusões está em preparação nesta versão."),
        })
    })
    .await?
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchSummary {
    id: String,
    created_at_ms: u64,
    label: String,
    reason: String,
    scope: Value,
    source_receipt: Value,
    members: u64,
    active: bool,
    restored_at_ms: Option<u64>,
}
impl From<exclusion_store::BatchInfo> for BatchSummary {
    fn from(batch: exclusion_store::BatchInfo) -> Self {
        Self {
            id: batch.id,
            created_at_ms: batch.created_at_ms,
            label: batch.label,
            reason: batch.reason,
            scope: batch.scope,
            source_receipt: batch.source_receipt,
            members: batch.members,
            active: batch.active,
            restored_at_ms: batch.restored_at_ms,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchPage {
    analysis: Identity,
    batches: Vec<BatchSummary>,
    next_cursor: Option<String>,
}

#[tauri::command]
pub(crate) async fn exclusion_list(
    analysis_context: Identity,
    cursor: Option<String>,
    limit: Option<usize>,
    operation_id: Option<String>,
) -> Result<BatchPage, String> {
    crate::offload_operation(operation_id, move || {
        crate::operations::check()?;
        let limit = limit.unwrap_or(50).clamp(1, 99);
        let mut batches = exclusion_store::list(
            &crate::config_dir(),
            &analysis_context,
            cursor.as_deref(),
            limit + 1,
        )?;
        let next_cursor = if batches.len() > limit {
            batches.pop();
            batches.last().map(|batch| batch.id.clone())
        } else {
            None
        };
        crate::operations::check()?;
        Ok(BatchPage {
            analysis: analysis_context,
            batches: batches.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    })
    .await?
}

fn report(phase: &str, completed: u64, total: Option<u64>) {
    let (id, message, unit) = match phase {
        "staging" => (
            "exclusion-stage",
            "Preparando registros selecionados",
            "registros",
        ),
        "staged" => ("exclusion-staged", "Seleção preparada", "registros"),
        "publishing" => (
            "exclusion-publish",
            "Publicando lote de exclusão",
            "registros",
        ),
        "published" => ("exclusion-published", "Lote publicado", "registros"),
        "verifying" => (
            "exclusion-verify",
            "Verificando arquivo de exclusões",
            "bytes",
        ),
        "validating_restore" => (
            "exclusion-restore",
            "Verificando registros para restaurar",
            "registros",
        ),
        _ => (
            "exclusion-identity",
            "Preparando identidades dos registros",
            "registros",
        ),
    };
    crate::operations::report_progress(
        "exclusões",
        id,
        message,
        completed.min(usize::MAX as u64) as usize,
        total.unwrap_or(0).min(usize::MAX as u64) as usize,
        unit,
        0,
    );
}
fn work() -> Work<'static> {
    Work {
        cancelled: &crate::operations::cancelled,
        progress: &report,
    }
}

#[tauri::command]
pub(crate) async fn exclusion_restore_batch(
    app: tauri::AppHandle,
    analysis_context: Identity,
    batch_id: String,
    operation_id: Option<String>,
) -> Result<Receipt, String> {
    offload_archive(operation_id, app.clone(), move || {
        let admission = Admission {
            analysis: analysis_context,
            source_receipt: json!({"kind":"archive","batchId":batch_id}),
        };
        let receipt =
            exclusion_store::restore_batch(&crate::config_dir(), &admission, &batch_id, &work())?;
        crate::operations::commit();
        Ok(receipt)
    })
    .await
}

#[tauri::command]
pub(crate) async fn exclusion_restore_selected(
    app: tauri::AppHandle,
    analysis_context: Identity,
    batch_id: String,
    members: Vec<Member>,
    operation_id: Option<String>,
) -> Result<Receipt, String> {
    offload_archive(operation_id, app, move || {
        restore_selected(analysis_context, batch_id, members)
    })
    .await
}
fn restore_selected(
    analysis_context: Identity,
    batch_id: String,
    members: Vec<Member>,
) -> Result<Receipt, String> {
    if members.is_empty() || members.len() > 500 {
        return Err("Selecione de 1 a 500 registros da página atual.".into());
    }
    let dir = crate::config_dir();
    let work = work();
    let budget = Budget::default();
    let view =
        exclusion_store::batch_visibility(&dir, &analysis_context, &batch_id, &budget, &work)?;
    let sources = view.sources();
    let descriptors: Vec<_> = sources.values().cloned().collect();
    let source_positions: std::collections::BTreeMap<_, _> = sources
        .keys()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index))
        .collect();
    let mut selected = Vec::new();
    for member in members {
        crate::operations::check()?;
        if !view.contains(&member)? {
            return Err(
                "Um registro não está mais ativo neste lote. Reabra a página antes de restaurar."
                    .into(),
            );
        }
        selected.push(exclusion_store::InputMember {
            source_index: *source_positions
                .get(member.key.source_key.as_str())
                .ok_or("Origem não pertence ao lote.")?,
            locator: member.key.locator,
            event_ref: member.event_ref,
        });
    }
    let admission = Admission {
        analysis: analysis_context,
        source_receipt: json!({"kind":"archive","batchId":batch_id}),
    };
    let staged = exclusion_store::stage(
        &dir,
        admission,
        exclusion_store::Purpose::RestoreSelection {
            batch_id: batch_id.clone(),
        },
        &descriptors,
        selected.into_iter().map(Ok),
        &budget,
        &work,
    )?;
    let receipt = exclusion_store::publish(
        &dir,
        staged,
        "",
        "",
        json!({"kind":"archive_selection","batchId":batch_id}),
        &budget,
        &work,
    )?;
    crate::operations::commit();
    Ok(receipt)
}

/// The registry owns only bounded pending payloads. A successful token remains
/// replayable briefly so a lost IPC reply cannot create a duplicate batch.
struct PreviewRegistry<P, R> {
    entries: VecDeque<PreviewEntry<P, R>>,
}
struct PreviewEntry<P, R> {
    token: String,
    owner: Identity,
    generation: Option<u64>,
    expires: Instant,
    value: PreviewValue<P, R>,
}
enum PreviewValue<P, R> {
    Ready(P),
    Publishing,
    Committed(R),
}
enum Begin<P, R> {
    Publish(P),
    Replay(R),
}
impl<P, R: Clone> PreviewRegistry<P, R> {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|entry| entry.expires > now || matches!(entry.value, PreviewValue::Publishing));
    }
    fn insert(
        &mut self,
        owner: Identity,
        generation: Option<u64>,
        payload: P,
        now: Instant,
    ) -> Result<String, String> {
        self.prune(now);
        if self
            .entries
            .iter()
            .filter(|entry| !matches!(entry.value, PreviewValue::Committed(_)))
            .count()
            >= MAX_PREVIEWS
        {
            return Err("Há duas seleções preparadas. Conclua ou descarte uma delas antes de preparar outra.".into());
        }
        let token = uuid::Uuid::new_v4().to_string();
        self.entries.push_back(PreviewEntry {
            token: token.clone(),
            owner,
            generation,
            expires: now + PREVIEW_TTL,
            value: PreviewValue::Ready(payload),
        });
        Ok(token)
    }
    fn begin(
        &mut self,
        token: &str,
        owner: &Identity,
        generation: Option<u64>,
        now: Instant,
    ) -> Result<Begin<P, R>, String> {
        self.prune(now);
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.token == token)
            .ok_or("A prévia expirou. Prepare a seleção novamente.")?;
        if entry.owner != *owner || entry.generation != generation {
            return Err("O Caso ou a fonte da prévia mudou. Prepare a seleção novamente.".into());
        }
        match &entry.value {
            PreviewValue::Committed(receipt) => return Ok(Begin::Replay(receipt.clone())),
            PreviewValue::Publishing => {
                return Err("Este lote já está sendo publicado. Aguarde o resultado.".into())
            }
            PreviewValue::Ready(_) => {}
        }
        let PreviewValue::Ready(payload) =
            std::mem::replace(&mut entry.value, PreviewValue::Publishing)
        else {
            unreachable!()
        };
        Ok(Begin::Publish(payload))
    }
    fn discard(&mut self, token: &str, owner: &Identity, generation: Option<u64>) -> bool {
        let Some(index) = self.entries.iter().position(|entry| {
            entry.token == token
                && entry.owner == *owner
                && entry.generation == generation
                && matches!(entry.value, PreviewValue::Ready(_))
        }) else {
            return false;
        };
        self.entries.remove(index);
        true
    }
    fn finish(&mut self, token: &str, receipt: Option<R>, now: Instant) {
        if let Some(receipt) = receipt {
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.token == token) {
                entry.value = PreviewValue::Committed(receipt);
                entry.expires = now + PREVIEW_TTL;
            }
            while self
                .entries
                .iter()
                .filter(|entry| matches!(entry.value, PreviewValue::Committed(_)))
                .count()
                > MAX_RECEIPTS
            {
                if let Some(index) = self
                    .entries
                    .iter()
                    .position(|entry| matches!(entry.value, PreviewValue::Committed(_)))
                {
                    self.entries.remove(index);
                }
            }
        } else {
            self.entries.retain(|entry| entry.token != token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(case: &str) -> Identity {
        Identity {
            case_id: case.into(),
            analysis_id: format!("analysis-{case}"),
            config_revision: 1,
            visibility_revision: 2,
        }
    }
    #[test]
    fn preview_tokens_bind_owner_source_and_expire_without_publishing() {
        let now = Instant::now();
        let mut registry = PreviewRegistry::<usize, usize>::new();
        let owner = identity("a");
        let token = registry.insert(owner.clone(), Some(3), 17, now).unwrap();
        assert!(registry
            .begin(&token, &identity("b"), Some(3), now)
            .is_err());
        assert!(registry.begin(&token, &owner, Some(4), now).is_err());
        assert!(registry
            .begin(&token, &owner, Some(3), now + PREVIEW_TTL)
            .is_err());
    }
    #[test]
    fn publication_is_single_consumer_and_success_is_replayable() {
        let now = Instant::now();
        let mut registry = PreviewRegistry::<usize, usize>::new();
        let owner = identity("a");
        let token = registry.insert(owner.clone(), None, 17, now).unwrap();
        assert!(matches!(
            registry.begin(&token, &owner, None, now),
            Ok(Begin::Publish(17))
        ));
        assert!(registry.begin(&token, &owner, None, now).is_err());
        registry.finish(&token, Some(23), now);
        assert!(matches!(
            registry.begin(&token, &owner, None, now),
            Ok(Begin::Replay(23))
        ));
    }
    #[test]
    fn failed_publications_and_expired_previews_release_pending_capacity() {
        let now = Instant::now();
        let mut registry = PreviewRegistry::<usize, usize>::new();
        let owner = identity("a");
        let first = registry.insert(owner.clone(), None, 1, now).unwrap();
        registry.insert(owner.clone(), None, 2, now).unwrap();
        assert!(registry.insert(owner.clone(), None, 3, now).is_err());
        registry.finish(&first, None, now);
        assert!(registry.insert(owner.clone(), None, 3, now).is_ok());
        assert!(registry.insert(owner, None, 4, now + PREVIEW_TTL).is_ok());
    }
    #[test]
    fn discard_releases_only_its_owned_unpublished_payload() {
        let now = Instant::now();
        let owner = identity("a");
        let mut registry = PreviewRegistry::<usize, usize>::new();
        let token = registry.insert(owner.clone(), Some(1), 4, now).unwrap();
        assert!(!registry.discard(&token, &identity("b"), Some(1)));
        assert!(!registry.discard(&token, &owner, Some(2)));
        assert!(registry.discard(&token, &owner, Some(1)));
        assert!(registry.begin(&token, &owner, Some(1), now).is_err());
        let token = registry.insert(owner.clone(), Some(1), 5, now).unwrap();
        assert!(matches!(
            registry.begin(&token, &owner, Some(1), now),
            Ok(Begin::Publish(5))
        ));
        assert!(
            !registry.discard(&token, &owner, Some(1)),
            "closing a preview cannot undo its in-flight publication"
        );
        registry.finish(&token, Some(6), now);
        assert!(
            !registry.discard(&token, &owner, Some(1)),
            "keep committed receipts replayable"
        );
    }
}

#[cfg(test)]
#[path = "exclusion_commands_tests.rs"]
mod transaction_tests;
