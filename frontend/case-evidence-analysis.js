/* Captured native Case scopes. These descriptors are never Event arrays. */
window.CaseEvidenceAnalysis = (() => {
  "use strict";
  const scopes = new WeakMap();
  const storeKey = store => JSON.stringify([store?.storeId, store?.epoch]);
  const identityKey = value => JSON.stringify([value?.caseId, value?.analysisId, value?.configRevision, value?.visibilityRevision]);
  const fail = () => Error("EVIDENCE_VIEW_CHANGED: O Caso ou suas evidências mudaram. Reabra a análise no contexto atual.");
  const pending = () => Error("EVIDENCE_SAVE_PENDING: Aguarde a confirmação do salvamento das evidências antes de consultar o Caso.");
  const stateFor = (store, id) => store?.caseEvidence?.find(value => (value.owner?.caseId || value.caseId) === id);
  function capture({ client, session, store, item, stationId = null, currentStore, currentCase, currentStation = () => stationId }) {
    if (!item || store?.evidenceViewVersion !== 1) throw fail();
    const evidence = stateFor(store, item.id);
    if (item.kind === "preserved_case_unavailable" || evidence?.state === "unavailable") throw Error(`EVIDENCE_UNAVAILABLE: ${evidence?.message || item.code || "As evidências deste Caso estão preservadas, mas indisponíveis para análise."}`);
    const shape = window.CaseEvidence.analysisShape(item), committedShape = client.analysisShape(item.id);
    const initial = !evidence && !item.analysisContext?.analysisId && shape === "[]" && committedShape === null;
    if (!initial && (!evidence || evidence.state !== "ready")) throw fail();
    if (!initial && shape !== committedShape) throw pending();
    const descriptor = Object.freeze({ kind: "native_case_scope", caseId: item.id, stationId,
      evidenceSignature: evidence?.evidenceSignature ?? null, owner: evidence?.owner ? Object.freeze({ ...evidence.owner }) : null });
    scopes.set(descriptor, { client, session, store, item, currentStore, currentCase, currentStation, shape,
      storeKey: storeKey(store.store), initial, identity: initial ? null : identityKey(item.analysisContext), bound: null, token: null });
    return descriptor;
  }
  const isCapture = value => !!value && typeof value === "object" && scopes.has(value);
  function current(descriptor, captured) {
    const store = captured.currentStore(), item = captured.currentCase(descriptor.caseId), evidence = stateFor(store, descriptor.caseId);
    if (store !== captured.store || item !== captured.item || storeKey(store.store) !== captured.storeKey
      || captured.currentStation() !== descriptor.stationId || evidence?.state !== "ready") return false;
    if (window.CaseEvidence.analysisShape(item) !== captured.shape || captured.client.analysisShape(item.id) !== captured.shape) return false;
    if (captured.initial) {
      if (evidence.preservedCount !== 0 || evidence.owner.analysisId !== item.analysisContext?.analysisId) return false;
      const binding = JSON.stringify([evidence.owner, evidence.evidenceSignature, identityKey(item.analysisContext)]);
      if (captured.bound === null) captured.bound = binding;
      return captured.bound === binding;
    }
    return evidence.evidenceSignature === descriptor.evidenceSignature && evidence.owner.analysisId === descriptor.owner.analysisId
      && identityKey(item.analysisContext) === captured.identity;
  }
  async function prepare(args, resync = false, previousToken = null, { canonical = false, cancelled = () => false } = {}) {
    const descriptor = args.caseEvents, captured = scopes.get(descriptor);
    if (!captured || cancelled() || !current(descriptor, captured)) throw fail();
    if (["retry_required", "reconcile_required"].includes(captured.session.status())) throw Error("EVIDENCE_RECONCILE_REQUIRED: Confirme o salvamento pendente antes de abrir a análise do Caso.");
    if (identityKey(args.analysisContext) !== identityKey(captured.item.analysisContext)) throw fail();
    const available = () => !cancelled() && current(descriptor, captured);
    const result = await captured.client.openCase({ caseId: descriptor.caseId, analysisContext: args.analysisContext,
      stationId: descriptor.stationId, force: resync, previousToken, isCurrent: available });
    if (!available()) throw fail();
    if (result.state !== "ready") throw Error(`${result.code}: ${result.preservedCount} ocorrências permanecem preservadas; a análise deste recorte está indisponível neste orçamento.`);
    captured.token = result.publication.caseContentToken;
    const { caseEvents: _scope, caseContentToken: _old, ...rest } = args;
    return { ...rest, caseKey: result.publication.caseKey, sourceGeneration: null,
      ...(canonical ? { caseContentToken: captured.token } : {}) };
  }
  return { capture, isCapture, prepare, token: value => scopes.get(value)?.token ?? null };
})();
