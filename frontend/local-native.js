/* Client for the native `local_analysis` command, shared by every local-exploration
 * component. It admits each call against the Case, revision and source captured when
 * the call starts and refuses to deliver a result to a different one. */
window.LocalNative = (() => {
  "use strict";
  const activeCalls = new Map();
  const scope = () => workspaceScope();
  const stamp = () => JSON.stringify([activeCase()?.id, scope(), window.AnalysisContexts?.capture()?.identity, state.currentArtifact?.id, state.currentArtifact?.loadedAt]);
  async function ready() {
    const previous = activeCase();
    await window.workspaceBootstrap;
    await window.WorkspaceContext?.waitForSource();
    if (previous && previous !== activeCase()) throw Error("O Caso mudou enquanto a área era preparada.");
    const item = ensureCase(), started = performance.now();
    while (window.WorkspaceContext && (!WorkspaceContext.ready || WorkspaceContext.changing)) {
      if (activeCase() !== item) throw Error("O Caso mudou enquanto a área era preparada.");
      if (performance.now() - started > 30000) throw Error("A área do Caso ainda não ficou disponível. Aguarde sua preparação e repita a ação.");
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    if (activeCase() !== item) throw Error("O Caso mudou antes da consulta.");
    return item;
  }
  /** `filters`: true = the current Explorar filters; an array = exactly those filters. */
  async function call(request, { latest = "local-action", filters = false } = {}) {
    await ready();
    const capturedScope = scope(), owner = await window.AnalysisContexts.prepare(window.AnalysisContexts.capture());
    const marker = Symbol();
    activeCalls.set(latest, marker);
    let result;
    try {
      result = await api("local_analysis", { ...analyticsRequest(capturedScope), filters: Array.isArray(filters) ? filters : filters ? backendFilters() : [], request }, { latest, silent: true, analysisOwner: owner });
    } finally {
      if (activeCalls.get(latest) === marker) activeCalls.delete(latest);
    }
    window.AnalysisContexts.assertOwner(owner, { revisions: !result?.analysisContext });
    if (scope() !== capturedScope) throw new Error("A área de trabalho mudou. Reabra a ação no contexto atual.");
    return result;
  }
  const cancel = (latest) => window.Tasks?.cancelLatest(latest);
  function cancelAll() { for (const key of activeCalls.keys()) cancel(key); }
  /** A superseded or cancelled call is not an error the user needs to read. */
  const superseded = (error) => /cancelad|substitu|superseded|aborted/i.test(String(error?.message || error));
  return { ready, call, cancel, cancelAll, scope, stamp, superseded };
})();
