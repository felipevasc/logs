/* Authoritative Case analysis ownership. Layouts and evidence are never configuration receipts. */
window.AnalysisContexts = (() => {
  "use strict";
  const instances = new WeakMap(), snapshots = new Map(), definitions = new Map(), preparations = new Map(), preparedLegacy = new WeakSet();
  let nextInstance = 0, definitionKey = null, activeDefinitionOwner = null;
  const findCase = id => state.cases?.cases?.find(item => item.id === id) || null;
  const instance = value => { if (!value) return null; if (!instances.has(value)) instances.set(value, ++nextInstance); return instances.get(value); };
  const validIdentity = value => value && typeof value.caseId === "string" && value.caseId.length > 0
    && typeof value.analysisId === "string" && value.analysisId.length > 0
    && Number.isSafeInteger(value.configRevision) && value.configRevision >= 0
    && Number.isSafeInteger(value.visibilityRevision) && value.visibilityRevision >= 0;
  function identity(value) {
    return validIdentity(value) ? Object.freeze({ caseId: value.caseId, analysisId: value.analysisId,
      configRevision: value.configRevision, visibilityRevision: value.visibilityRevision }) : null;
  }
  function context(caseId = state.cases?.active) {
    const value = findCase(caseId)?.analysisContext;
    return value?.caseId === caseId && validIdentity(value) ? value : null;
  }
  const currentIdentity = caseId => identity(context(caseId));
  const signature = value => JSON.stringify(identity(value));
  const sourceKey = () => JSON.stringify([state.sourcePublication?.generation ?? null, state.currentArtifact?.id,
    state.currentArtifact?.loadedAt, !!state.sourceIdentityUnconfirmed]);
  function capture(caseId = state.cases?.active) {
    const item = findCase(caseId);
    return Object.freeze({ caseId: item?.id || null, instance: instance(item), identity: currentIdentity(caseId),
      sourceGeneration: state.sourcePublication?.generation ?? null, sourceKey: sourceKey() });
  }
  function owns(owner) {
    const item = findCase(owner?.caseId);
    return !!owner && instance(item) === owner.instance && (!owner.identity || context(owner.caseId)?.analysisId === owner.identity.analysisId);
  }
  function isCurrent(owner) {
    return owns(owner) && (state.cases?.active || null) === owner.caseId && sourceKey() === owner.sourceKey
      && signature(currentIdentity(owner.caseId)) === signature(owner.identity);
  }
  const stale = () => new Error("ANALYSIS_CONTEXT_CHANGED: O Caso, a configuração ou a fonte mudou. Reabra a ação no contexto atual.");
  function assertOwner(owner, { revisions = true, active = true } = {}) {
    if (!owns(owner) || active && ((state.cases?.active || null) !== owner.caseId || sourceKey() !== owner.sourceKey)
      || revisions && signature(currentIdentity(owner.caseId)) !== signature(owner.identity)) throw stale();
  }
  function validSnapshot(value) {
    return validIdentity(value) && value.schemaVersion === 1 && value.config && Array.isArray(value.config.derivedFields)
      && Array.isArray(value.config.references) && Array.isArray(value.migrationDiagnostics);
  }
  const definitionOwnerKey = owner => JSON.stringify([owner.instance, owner.identity?.analysisId ?? null, owner.identity?.configRevision ?? null]);
  function activate() {
    const key = definitionOwnerKey(capture());
    if (key === activeDefinitionOwner) return;
    activeDefinitionOwner = key; definitionKey = null;
    state.derivedFields = []; state.analysisDefinitionsPending = true; state.caseProfilesLoading = false;
  }
  async function adopt(value, { owner = capture(value?.caseId), reconcile = true } = {}) {
    if (!validSnapshot(value)) throw new Error("Resposta de configuração do Caso inválida.");
    if (owner.caseId !== value.caseId || !owns(owner)) return { accepted: false, reason: "owner" };
    const item = findCase(value.caseId), previous = context(value.caseId);
    if (item?.kind === "preserved_case_unavailable") return { accepted: false, reason: "metadata_unavailable" };
    if (previous && previous.analysisId !== value.analysisId || owner.identity && owner.identity.analysisId !== value.analysisId)
      return { accepted: false, reason: "owner" };
    if (previous) {
      const config = Math.sign(value.configRevision - previous.configRevision), visibility = Math.sign(value.visibilityRevision - previous.visibilityRevision);
      if (config * visibility < 0) {
        if (!reconcile) throw new Error("As revisões do Caso divergem. Atualize sua configuração antes de continuar.");
        return refresh(value.caseId, { owner: capture(value.caseId) });
      }
      if (config < 0 || visibility < 0) return { accepted: false, reason: "older" };
      if (!config && !visibility) return { accepted: false, reason: "unchanged" };
    }
    const snapshot = structuredClone(value);
    item.analysisContext = snapshot;
    const active = state.cases?.active === value.caseId;
    const configChanged = !previous || previous.configRevision !== snapshot.configRevision;
    const visibilityChanged = !previous || previous.visibilityRevision !== snapshot.visibilityRevision;
    // Stored workspace results are disposable, but authored chart/cube layouts stay intact.
    window.WorkspaceContext?.invalidateAnalysis?.(value.caseId);
    if (active) {
      if (configChanged) activate();
      window.invalidateAnalysisComputedData?.(snapshot);
    }
    document.dispatchEvent(new CustomEvent("analysis-context-change", { detail: { snapshot, previous: identity(previous), active, configChanged, visibilityChanged } }));
    return { accepted: true, snapshot };
  }
  async function refresh(caseId = state.cases?.active, { owner = capture(caseId) } = {}) {
    if (findCase(caseId)?.kind === "preserved_case_unavailable") throw Error("CASE_METADATA_UNAVAILABLE: Os metadados deste Caso estão indisponíveis; os originais permanecem preservados.");
    if (!owner.caseId || owner.caseId !== caseId || !owns(owner)) throw stale();
    const key = `${owner.instance}:${signature(owner.identity)}`;
    if (snapshots.has(key)) return snapshots.get(key);
    const request = (async () => {
      const result = await api("analysis_context_snapshot", { caseId }, { silent: true });
      return result == null ? { accepted: false, reason: "absent" } : adopt(result, { owner, reconcile: false });
    })();
    snapshots.set(key, request);
    try { return await request; } finally { if (snapshots.get(key) === request) snapshots.delete(key); }
  }
  async function prepare(owner, { metadata = false } = {}) {
    if (findCase(owner?.caseId)?.kind === "preserved_case_unavailable") throw Error("CASE_METADATA_UNAVAILABLE: Escolha outro Caso para analisar ou editar; os originais deste Caso permanecem preservados.");
    assertOwner(owner, { revisions: !!owner.identity });
    activate();
    const item = findCase(owner.caseId);
    if (item && !context(owner.caseId) && !preparedLegacy.has(item)) {
      // One durable save/receipt per Case instance, shared by simultaneous first reads.
      let request = preparations.get(owner.instance);
      if (!request) {
        request = (async () => {
          if (!await saveCases()) throw new Error("Salve o Caso antes de consultar sua análise.");
          assertOwner(owner, { revisions: false });
          if (!context(owner.caseId)) await refresh(owner.caseId, { owner });
          assertOwner(owner, { revisions: false });
          if (!context(owner.caseId)) preparedLegacy.add(item);
        })();
        preparations.set(owner.instance, request);
      }
      try { await request; } finally { if (preparations.get(owner.instance) === request) preparations.delete(owner.instance); }
      assertOwner(owner, { revisions: false });
    }
    // A first receipt may establish an identity; never change a captured existing one.
    if (!owner.identity) owner = Object.freeze({ ...owner, identity: currentIdentity(owner.caseId) });
    assertOwner(owner);
    if (!metadata && owner.caseId) await ensureDefinitions(owner);
    assertOwner(owner);
    return owner;
  }
  async function ensureDefinitions(owner) {
    const key = definitionOwnerKey(owner);
    // Legacy null admission has no proved Case configuration. Never borrow a global list.
    if (!owner.identity) { definitionsLoaded(owner, []); return; }
    if (definitionKey === key && !state.analysisDefinitionsPending) return;
    const requestKey = JSON.stringify([key, owner.identity, owner.sourceKey]);
    let request = definitions.get(requestKey);
    if (!request) {
      request = (async () => {
        const fields = await api("list_derived_fields", { analysisContext: owner.identity, sourceGeneration: owner.sourceGeneration },
          { silent: true, analysisOwner: owner });
        assertOwner(owner);
        if (!Array.isArray(fields)) throw new Error("Lista de campos do Caso inválida.");
        state.derivedFields = fields; state.analysisDefinitionsPending = false; definitionKey = key;
        window.AnalysisFields?.refresh();
      })();
      definitions.set(requestKey, request);
    }
    try { await request; } finally { if (definitions.get(requestKey) === request) definitions.delete(requestKey); }
  }
  function definitionsLoaded(owner, fields) {
    assertOwner(owner);
    if (!Array.isArray(fields)) throw new Error("Lista de campos do Caso inválida.");
    state.derivedFields = owner.identity ? fields : []; state.analysisDefinitionsPending = false;
    definitionKey = definitionOwnerKey(owner); activeDefinitionOwner = definitionKey;
    window.AnalysisFields?.refresh();
  }
  async function receipt(result, owner) {
    if (result?.analysisContext) return adopt(result.analysisContext, { owner });
    return { accepted: false, reason: "absent" };
  }
  return { context, identity: currentIdentity, signature, capture, owns, isCurrent, assertOwner, prepare,
    activate, adopt, receipt, refresh, definitionsLoaded, validSnapshot };
})();
function currentAnalysisContext(caseId) { return window.AnalysisContexts.context(caseId); }
function currentAnalysisIdentity(caseId) { return window.AnalysisContexts.identity(caseId); }
