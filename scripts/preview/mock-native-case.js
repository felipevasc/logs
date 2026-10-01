/* Opt-in native Case transport for the browser startup acceptance test.
   It models the command response after verified native adoption; controllers,
   metadata persistence, context adoption and source installation stay real. */
window.createMockNativeCase = options => {
  "use strict";
  const { analysisContexts } = options;
  const copy = value => structuredClone(value), id = number => `00000000-0000-0000-0000-${String(number).padStart(12, "0")}`;
  const owner = { storeId: id(1), caseId: "native-startup-case", analysisId: id(3) };
  const context = { schemaVersion: 1, caseId: owner.caseId, analysisId: owner.analysisId, configRevision: 0, visibilityRevision: 0,
    config: { derivedFields: [], references: [] }, migrationDiagnostics: [], legacyRaw: null };
  const reference = { kind: "native_evidence", schemaVersion: 1, owner, containerId: id(4), manifestId: id(5), manifestSha256: "a".repeat(64), memberCount: 2 };
  let view = { evidenceViewVersion: 1, store: { storeId: owner.storeId, epoch: id(2), revision: "1" }, active: owner.caseId,
    cases: [{ id: owner.caseId, name: "Native startup after adoption", createdAt: 1000, analysisContext: copy(context), items: [{ id: "native-item", label: "Preserved original records", note: "", rows: { kind: "native_evidence_container", reference: copy(reference), preservedCount: 2, preview: null } }],
      manual: [], stations: [], artifacts: [], caseTrails: [], timeline: { groups: [], annotations: [], edits: {}, layout: {} },
      workspace: { view: "caso", analysisView: "overview", activeScope: "dataset", expandedItemIds: [], contextStates: { dataset: { page: "import", values: { analysisView: "overview", filters: [], quick: "" } } } } }],
    caseEvidence: [{ state: "ready", owner, evidenceSignature: "b".repeat(64), preservedCount: 2 }], diagnostics: [] };
  analysisContexts.set(owner.caseId, copy(context));
  const calls = [], saves = new Map(); let advanceContext = false;
  function request(command, args = {}) { calls.push({ command, args: copy(args), nativeActive: window.CaseEvidence?.active === true }); }
  const handlers = {
    cases_load_view: args => { request("cases_load_view", args); return copy(view); },
    cases_save_view: args => {
      request("cases_save_view", args); const value = args.request;
      if (!value || typeof value.requestId !== "string" || typeof value.documentJson !== "string" || Object.hasOwn(args, "data")) throw Error("MOCK_NATIVE_SAVE_REQUEST");
      if (saves.has(value.requestId)) return { ...copy(saves.get(value.requestId)), replayed: true };
      if (JSON.stringify(value.expectedStore) !== JSON.stringify(view.store)) throw Error("MOCK_NATIVE_STORE_CAS");
      const next = JSON.parse(value.documentJson); window.CaseEvidence.validate.document(next);
      if (next.cases.length !== 1 || next.cases[0].id !== owner.caseId || JSON.stringify(next.cases[0].items[0].rows.reference) !== JSON.stringify(reference)) throw Error("MOCK_NATIVE_REFERENCE_CHANGED");
      const snapshot = copy(analysisContexts.get(owner.caseId));
      if (advanceContext) { snapshot.configRevision++; advanceContext = false; analysisContexts.set(owner.caseId, copy(snapshot)); }
      next.cases[0].analysisContext = copy(snapshot); next.store = { ...view.store, revision: String(BigInt(view.store.revision) + 1n) }; view = next;
      const receipt = { requestId: value.requestId, committedStore: copy(view.store), currentStore: copy(view.store), evidence: [copy(reference)], analysisContexts: [snapshot], caseEvidence: copy(view.caseEvidence), replayed: false, reconcileRequired: false };
      saves.set(value.requestId, copy(receipt)); return receipt;
    },
    cases_load: () => { request("cases_load"); throw Error("MOCK_NATIVE_LEGACY_LOAD_FORBIDDEN"); },
    cases_save: () => { request("cases_save"); throw Error("MOCK_NATIVE_LEGACY_SAVE_FORBIDDEN"); },
    case_sync: () => { request("case_sync"); throw Error("MOCK_NATIVE_INLINE_SYNC_FORBIDDEN"); },
  };
  return { handlers, calls, document: () => copy(view),
    publishName: name => { view.cases[0].name = name; view.store.revision = String(BigInt(view.store.revision) + 1n); },
    advanceContextOnNextSave: () => { advanceContext = true; } };
};
