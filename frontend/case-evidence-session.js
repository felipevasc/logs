/* Native metadata persistence coordinator. Not installed until all Case consumers migrate. */
window.CaseEvidenceSession = (() => {
  "use strict";
  const clone = value => structuredClone(value);
  const sameStore = (left, right) => left?.storeId === right?.storeId && left?.epoch === right?.epoch;
  const referenceKey = ref => JSON.stringify([ref.kind, ref.owner.storeId, ref.owner.caseId, ref.owner.analysisId,
    ref.containerId, ref.manifestId, ref.manifestSha256, ref.memberCount, ref.token ?? null]);
  function containers(document) {
    const result = new Map();
    for (const item of document.cases || []) {
      const add = (target, field) => {
        const value = target?.[field];
        if (value?.kind === "native_evidence_container") result.set(value.reference.containerId, { item, target, field, value });
      };
      for (const field of ["rows", "events"]) add(item, field);
      for (const child of item.items || []) for (const field of ["rows", "events"]) add(child, field);
    }
    return result;
  }
  function draftKey(value) {
    if (value?.evidenceViewVersion !== 1) return null;
    const metadata = window.CaseEvidence.metadataDocument(value);
    // Config snapshots are native facts. Workspace snapshots are installed by
    // the existing source-queue transaction; UI save intents still mark them dirty.
    for (const item of metadata.cases) { delete item.analysisContext; delete item.workspace; }
    return JSON.stringify(metadata);
  }
  const stateCaseId = entry => entry.owner?.caseId || entry.caseId;
  function create({ client, getStore, setStore, normalize = value => value, captureOwner = () => null,
    adoptContext = async () => {}, changed = () => {}, draftBusy = () => false }) {
    let serial = 0, dirty = 0, committedDirty = 0, queue = Promise.resolve(), failed = null, blocked = null, saving = 0, scheduled = 0;
    const deferredLoads = new WeakMap();
    function markDirty() { dirty++; serial++; }
    function assertLoadCurrent(value) {
      const captured = deferredLoads.get(value);
      if (!captured || captured.request !== serial || getStore() !== captured.before || draftBusy() || draftKey(getStore()) !== captured.fingerprint) {
        blocked = "reconcile_required"; throw stale();
      }
    }
    const stale = () => Error("EVIDENCE_VIEW_CHANGED: A investigação mudou; mantenha o rascunho e reabra a visualização.");
    function isClean() {
      const fingerprint = draftKey(getStore()), committed = client.document();
      return !(scheduled || dirty !== committedDirty || failed || blocked || draftBusy() || fingerprint !== null && committed && fingerprint !== draftKey(committed));
    }
    async function load({ install = true } = {}) {
      const before = getStore(), fingerprint = draftKey(before), committed = client.document();
      if (scheduled || dirty !== committedDirty || failed || blocked || draftBusy() || fingerprint !== null && committed && fingerprint !== draftKey(committed)) throw Error("EVIDENCE_DRAFT_PENDING: Há alterações pendentes. Reconcilie a investigação sem descartar o rascunho.");
      const request = ++serial;
      const loaded = await client.load();
      if (request !== serial || getStore() !== before || draftBusy() || draftKey(getStore()) !== fingerprint) { blocked = "reconcile_required"; throw stale(); }
      const next = normalize(loaded); window.CaseEvidence.validate.document(next);
      if (install) { setStore(next); changed({ kind: "loaded", store: next }); }
      else deferredLoads.set(next, { request, before, fingerprint });
      return next;
    }
    async function adopt(result, captured) {
      const live = getStore();
      if (live !== captured.store || !sameStore(live.store, captured.document.store)) { blocked = "reconcile_required"; throw stale(); }
      if (result.reconcileRequired || client.status() === "reconcile_required") {
        blocked = "reconcile_required";
        throw Error("EVIDENCE_RECONCILE_REQUIRED: O salvamento foi confirmado em outra revisão. O rascunho local foi mantido para reconciliação.");
      }
      const before = containers(captured.document), current = containers(live);
      for (const reference of result.evidence) {
        const prior = before.get(reference.containerId), entry = current.get(reference.containerId);
        // A user may edit/reorder/remove an item while its previous save runs.
        // Only replace the exact descriptor that this save actually published.
        if (!prior || !entry || captured.cases.get(entry.item.id) !== entry.item
          || referenceKey(prior.value.reference) !== referenceKey(entry.value.reference)) continue;
        entry.target[entry.field] = { kind: "native_evidence_container", reference: clone(reference), preservedCount: reference.memberCount, preview: null };
      }
      live.store = clone(result.currentStore);
      const currentStates = new Map(live.caseEvidence.map(entry => [stateCaseId(entry), entry]));
      const receivedStates = new Map(result.caseEvidence.map(entry => [stateCaseId(entry), entry]));
      live.caseEvidence = live.cases.flatMap(item => {
        const owned = captured.cases.get(item.id) === item;
        const entry = owned ? receivedStates.get(item.id) || currentStates.get(item.id) : currentStates.get(item.id);
        return entry ? [clone(entry)] : [];
      });
      for (const context of result.analysisContexts) {
        const item = live.cases.find(item => item.id === context.caseId);
        if (!item || item !== captured.cases.get(context.caseId) || item.kind === "preserved_case_unavailable") continue;
        await adoptContext(context, { owner: captured.owners.get(context.caseId) });
      }
      if (getStore() !== live) { blocked = "reconcile_required"; throw stale(); }
      changed({ kind: "saved", store: live, result });
      return result;
    }
    async function execute(captured) {
      saving++;
      try {
        const result = await client.save(captured.ticket);
        await adopt(result, captured); committedDirty = Math.max(committedDirty, captured.dirty); failed = null; return result;
      } catch (error) {
        failed = captured;
        if (client.status() === "reconcile_required") blocked = "reconcile_required";
        changed({ kind: "save_failed", error, store: captured.store }); throw error;
      } finally { saving--; }
    }
    function save() {
      const store = getStore(); markDirty(); scheduled++;
      const run = async () => {
        if (store !== getStore()) throw stale();
        if (blocked) throw Error("EVIDENCE_RECONCILE_REQUIRED: Reconcilie a investigação antes de salvar novamente; o rascunho foi mantido.");
        if (failed) throw Error("EVIDENCE_SAVE_RETRY_REQUIRED: Repita a solicitação pendente antes de enviar outro salvamento.");
        const document = window.CaseEvidence.metadataDocument(store);
        const captured = { store, document, dirty, cases: new Map(store.cases.map(item => [item.id, item])),
          owners: new Map(store.cases.map(item => [item.id, captureOwner(item)])), ticket: client.prepareSave(document) };
        return execute(captured);
      };
      const result = queue.catch(() => {}).then(run).finally(() => { scheduled--; }); queue = result; return result;
    }
    function retry() {
      const captured = failed;
      if (!captured) return Promise.reject(Error("EVIDENCE_SAVE_RETRY_REQUIRED: Não há salvamento pendente para repetir."));
      if (blocked) return Promise.reject(Error("EVIDENCE_RECONCILE_REQUIRED: Reabra a visão nativa antes de repetir o salvamento."));
      serial++; scheduled++;
      const result = queue.catch(() => {}).then(() => {
        if (failed !== captured || captured.store !== getStore()) throw stale();
        return execute(captured);
      }).finally(() => { scheduled--; });
      queue = result; return result;
    }
    async function reconcile() {
      await queue.catch(() => {});
      // Return native authority separately. The caller must preserve/review its
      // authored draft; this read never installs it over local unsaved notes.
      const authoritative = await client.load(); blocked = "reconcile_required";
      return { authoritative, draft: clone(getStore()), failedRequest: failed ? clone(failed.ticket.request) : null };
    }
    return { load, assertLoadCurrent, save, retry, reconcile, markDirty, isClean, hasUnconfirmedSave: () => !!failed || !!blocked, pending: () => queue.catch(() => {}),
      status: () => blocked || (failed ? "retry_required" : scheduled ? "saving" : "ready") };
  }
  return { create };
})();
