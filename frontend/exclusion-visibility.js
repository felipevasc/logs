/* Source provenance warnings use admitted facts, never batch sizes or revisions. */
window.ExclusionVisibility = (() => {
  "use strict";
  const notices = [];
  for (const [id, anchor] of [["exclusion-source-notice", "#ws-content"], ["exclusion-explore-notice", ".hist-panel"], ["exclusion-archive-notice", "#ex-status"]]) {
    const notice = el("p", "notice ex-visibility-notice"); notice.id = id; notice.hidden = true; notice.setAttribute("role", "status");
    $(anchor)?.before(notice); notices.push(notice);
  }
  let cached = null, pending = null, serial = 0;
  const count = value => value == null || Number.isSafeInteger(value) && value >= 0;
  const capture = () => ({ owner: window.AnalysisContexts?.capture(), scope: workspaceScope(), evidence: workspaceScope() === "case" ? caseSig() : null });
  const key = value => JSON.stringify(value);
  function render() {
    const value = cached?.key === key(capture()) ? cached.value : null;
    const missing = value?.scope === "dataset" ? value.unavailableMembers : null;
    const message = missing > 0 ? `Exclusões de ${fmtNum(missing)} registro(s) não puderam ser aplicadas: a fonte original está ausente ou mudou. Confira a procedência no arquivo de exclusões.` : "";
    for (const notice of notices) { notice.textContent = message; notice.hidden = !message; }
  }
  function validate(value, captured) {
    if (!value || window.AnalysisContexts.signature(value.analysis) !== window.AnalysisContexts.signature(captured.owner.identity)
      || value.scope !== captured.scope || value.sourceGeneration !== (captured.scope === "case" ? null : captured.owner.sourceGeneration)
      || typeof value.sourceAvailable !== "boolean" || ![value.totalRows, value.excludedRows, value.unavailableMembers].every(count)
      || !["totalRows", "excludedRows", "unavailableMembers"].every(name => Object.hasOwn(value, name))
      || !value.sourceAvailable && (value.totalRows !== null || value.excludedRows !== null)
      || value.excludedRows != null && value.totalRows != null && value.excludedRows > value.totalRows
      || value.scope === "case" && value.unavailableMembers != null) throw Error("Resumo de exclusões inválido ou de outro contexto.");
    return value;
  }
  async function refresh({ force = false } = {}) {
    const captured = capture(), signature = key(captured);
    render();
    if (!captured.owner?.identity) return;
    if (!force && cached?.key === signature) return cached.value;
    if (pending?.key === signature) return pending.promise;
    const request = ++serial, evidence = captured.scope === "case" ? caseEvents("analysis") : null;
    const current = () => request === serial && signature === key(capture());
    const promise = (async () => {
      try {
        const args = await caseArgs({ analysisContext: captured.owner.identity, sourceGeneration: captured.owner.sourceGeneration,
          ...(evidence ? { caseEvents: evidence } : {}) });
        if (!current()) return;
        const value = await api("exclusion_visibility", args, { silent: true, background: true, latest: "exclusion-visibility", analysisOwner: captured.owner, caseEvents: evidence });
        if (!current()) return;
        window.AnalysisContexts.assertOwner(captured.owner);
        cached = { key: signature, value: validate(value, captured) }; render(); return value;
      } catch { /* An unavailable or stale summary cannot mean zero unapplied exclusions. */ }
      finally { if (pending?.request === request) pending = null; }
    })();
    pending = { key: signature, request, promise }; return promise;
  }
  for (const name of ["analysis-context-change", "workspace-context-change"]) document.addEventListener(name, () => { void refresh(); });
  Promise.resolve(window.workspaceBootstrap).then(() => refresh()).catch(() => {});
  return { refresh, render, validate };
})();
