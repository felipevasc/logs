/* Portable investigation transfer stays native. Import receipts never replace
   live state directly; a fresh management read owns post-commit selection. */
window.CaseEvidenceTransfer = (() => {
  "use strict";
  const clone = value => structuredClone(value), own = (value, name) => Object.hasOwn(value, name);
  const fail = () => { throw Error("EVIDENCE_TRANSFER_INVALID: O recibo de transferência não corresponde à investigação capturada."); };
  const same = (a, b) => a?.storeId === b?.storeId && a?.epoch === b?.epoch;
  const id = value => typeof value === "string" && !!value && value.length <= 4096;
  function keys(value, allowed) { if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some(key => !allowed.includes(key))) fail(); }
  function importReceipt(result, request) {
    keys(result, ["importedCaseId", "importedCaseIds", "caseViews", "receipt"]);
    const receipt = result.receipt;
    keys(receipt, ["requestId", "committedStore", "currentStore", "evidence", "analysisContexts", "caseEvidence", "replayed", "reconcileRequired"]);
    window.CaseEvidence.validate.stamp(receipt.committedStore); window.CaseEvidence.validate.stamp(receipt.currentStore);
    if (receipt.requestId !== request.requestId || !same(receipt.committedStore, request.store) || !same(receipt.currentStore, request.store)
      || window.CaseEvidence.compareRevisions(receipt.committedStore.revision, request.store.revision) <= 0
      || window.CaseEvidence.compareRevisions(receipt.currentStore.revision, receipt.committedStore.revision) < 0
      || window.CaseEvidence.compareRevisions(receipt.currentStore.revision, receipt.committedStore.revision) > 0 && receipt.reconcileRequired !== true
      || typeof receipt.replayed !== "boolean" || typeof receipt.reconcileRequired !== "boolean" || !Array.isArray(receipt.evidence) || !Array.isArray(receipt.analysisContexts) || !Array.isArray(receipt.caseEvidence)
      || !Array.isArray(result.importedCaseIds) || result.importedCaseIds.length > 1000 || new Set(result.importedCaseIds).size !== result.importedCaseIds.length
      || result.importedCaseIds.some(value => !id(value)) || !Array.isArray(result.caseViews) || result.caseViews.length !== result.importedCaseIds.length
      || result.caseViews.some((value, index) => value.id !== result.importedCaseIds[index]) || result.importedCaseId !== null && !result.importedCaseIds.includes(result.importedCaseId)) fail();
    for (const reference of receipt.evidence) window.CaseEvidence.validate.reference(reference);
    const selected = new Set(result.importedCaseIds), views = { evidenceViewVersion: 1, store: receipt.committedStore, active: result.importedCaseId, cases: result.caseViews,
      caseEvidence: receipt.caseEvidence.filter(entry => selected.has(entry.owner?.caseId || entry.caseId)),
      diagnostics: receipt.caseEvidence.filter(entry => entry.state === "unavailable" && selected.has(entry.caseId)).map(entry => { const { state: _state, ...diagnostic } = entry; return diagnostic; }) };
    window.CaseEvidence.validate.document(views);
    if (!window.CaseEvidence.validate.utf8Within(JSON.stringify(result), window.CaseEvidence.limits.documentBytes)) fail();
    return result;
  }
  function create({ client, session, getStore, invoke, replace, requestId = () => crypto.randomUUID(), commands = { export: "case_export_native", import: "case_import_native" } }) {
    let importing = null;
    async function settle() { await session.pending(); if (session.status() !== "ready" || client.status() !== "ready") throw Error("Confirme o salvamento pendente antes de transferir a investigação."); }
    async function exportFile(c, path, { mask = false } = {}) {
      await settle(); const live = getStore();
      if (!id(path) || typeof mask !== "boolean" || !live.cases.includes(c)) throw Error("A investigação selecionada mudou. Reabra a exportação.");
      const document = window.CaseEvidence.metadataDocument(live), selected = document.cases.find(value => value.id === c.id);
      document.active = c.id; document.cases = [selected]; document.caseEvidence = document.caseEvidence.filter(entry => (entry.owner?.caseId || entry.caseId) === c.id); document.diagnostics = document.diagnostics.filter(entry => entry.caseId === c.id);
      window.CaseEvidence.validate.document(document);
      const request = { store: clone(document.store), documentJson: JSON.stringify(document), path, mask, requestId: requestId() };
      const result = await invoke(commands.export, { request });
      keys(result, ["kind", "path", "format", "cases", "records", "masked"]);
      if (result.kind !== "native_case_export" || result.path !== path || !["json", "licase"].includes(result.format) || result.format !== (path.toLowerCase().endsWith('.licase') ? 'licase' : 'json')
        || result.cases !== 1 || result.records !== null && (!Number.isSafeInteger(result.records) || result.records < 0) || result.masked !== mask) fail();
      return clone(result);
    }
    async function execute(attempt) {
      if (attempt.running) return attempt.running;
      const run = (async () => {
        const result = importReceipt(await invoke(commands.import, { request: clone(attempt.request) }), attempt.request); attempt.result = clone(result);
        if (getStore() !== attempt.store || JSON.stringify(window.CaseEvidence.metadataDocument(getStore())) !== attempt.fingerprint) {
          attempt.phase = "reconcile_required"; throw Error("EVIDENCE_IMPORT_RECONCILE: A importação foi confirmada, mas o rascunho local mudou. Reabra a investigação depois de reconciliar o rascunho.");
        }
        const loaded = await session.load({ install: false });
        await replace(loaded, () => session.assertLoadCurrent(loaded));
        attempt.phase = "complete"; if (importing === attempt) importing = null;
        return { importedCaseIds: clone(result.importedCaseIds), importedCaseId: result.importedCaseId, replayed: result.receipt.replayed, reconciled: true };
      })().catch(error => { if (attempt.phase !== "reconcile_required") attempt.phase = attempt.result ? "reload_required" : "retry_required"; throw error; }).finally(() => { attempt.running = null; });
      attempt.running = run; return run;
    }
    async function importFile(path) {
      if (importing) throw Error("Confirme ou reconcilie a importação pendente antes de escolher outro arquivo.");
      await settle(); if (importing) throw Error("Uma importação já está em andamento."); if (session.isClean?.() === false) throw Error("Salve ou reconcilie o rascunho antes de importar outra investigação."); if (!id(path)) fail(); const store = getStore();
      const attempt = { request: { store: clone(store.store), path, requestId: requestId() }, store, fingerprint: JSON.stringify(window.CaseEvidence.metadataDocument(store)), phase: "importing", result: null, running: null };
      importing = attempt; return execute(attempt);
    }
    async function retryImport() {
      if (!importing) throw Error("Não há importação pendente para repetir.");
      return execute(importing);
    }
    return { exportFile, importFile, retryImport, status: () => importing?.phase || "ready" };
  }
  function service() {
    const services = nativeEvidenceServices();
    return services.transfer ||= create({ client: services.client, session: services.session, getStore: () => state.cases, invoke: (command, args) => api(command, args, { silent: true }),
      replace: (loaded, beforeReplace) => window.WorkspaceContext.replaceCases(loaded, { beforeReplace }) });
  }
  async function openExport(c = activeCase(), { mask = false } = {}) {
    const store = state.cases, path = await dialogApi.save({ defaultPath: "loginsight.licase", filters: [{ name: "Investigação portátil", extensions: ["licase"] }, { name: "Investigação JSON", extensions: ["json"] }] });
    if (!path) return null;
    if (state.cases !== store || !store.cases.includes(c)) throw Error("A investigação mudou. Reabra a exportação.");
    const result = await service().exportFile(c, path, { mask });
    toast(result.records === null ? "Investigação exportada; o conteúdo preservado não permite uma contagem segura de registros." : `Investigação exportada com ${fmtNum(result.records)} registros preservados.`, "ok"); return result;
  }
  async function openImport() {
    const transfer = service();
    if (transfer.status() !== "ready") return transfer.retryImport();
    const path = await dialogApi.open({ multiple: false, filters: [{ name: "Investigação", extensions: ["licase", "json"] }] }); if (!path) return null;
    const result = await transfer.importFile(path); toast("Investigação importada e relida da preservação nativa.", "ok"); return result;
  }
  return { create, service, openExport, openImport, validateImportReceipt: importReceipt };
})();
