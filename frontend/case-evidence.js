/* Native evidence view boundary. No legacy Event adapter. */
window.CaseEvidence = (() => {
  "use strict";
  const LIMITS = Object.freeze({ cases: 1000, members: 100000, previewRows: 16, previewColumns: 8, documentBytes: 16777216, pending: 8, legacyAliases: 10000, legacyAliasBytes: 4096, legacyMapBytes: 1048576 });
  const U64_MAX = "18446744073709551615";
  const own = (value, key) => Object.hasOwn(value, key);
  const fail = message => { throw Error(`EVIDENCE_VIEW_INVALID: ${message}`); };
  const object = value => value && typeof value === "object" && !Array.isArray(value);
  const integer = (value, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(value) && value >= 0 && value <= max;
  const id = value => typeof value === "string" && value.length > 0 && value.length <= 4096;
  const digest = value => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);
  const sameStore = (a, b) => a?.storeId === b?.storeId && a?.epoch === b?.epoch;
  const sameOwner = (a, b) => a?.storeId === b?.storeId && a?.caseId === b?.caseId && a?.analysisId === b?.analysisId;
  function keys(value, allowed, name) {
    if (!object(value) || Object.keys(value).some(key => !allowed.includes(key))) fail(name);
  }
  function utf8Within(value, maximum) {
    let bytes = 0;
    for (let index = 0; index < value.length; index++) {
      const code = value.charCodeAt(index);
      if (code <= 0x7f) bytes++;
      else if (code <= 0x7ff) bytes += 2;
      else if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length && value.charCodeAt(index + 1) >= 0xdc00 && value.charCodeAt(index + 1) <= 0xdfff) { bytes += 4; index++; }
      else bytes += 3;
      if (bytes > maximum) return false;
    }
    return true;
  }
  function revision(value) {
    if (typeof value !== "string" || !/^(0|[1-9]\d{0,19})$/.test(value) || value.length === 20 && value > U64_MAX) fail("Revisão decimal inválida.");
    return value;
  }
  const compare = (a, b) => { revision(a); revision(b); return Math.sign(a.length - b.length || (a === b ? 0 : a > b ? 1 : -1)); };
  function stamp(value) {
    keys(value, ["storeId", "epoch", "revision"], "Identidade da investigação inválida.");
    if (!id(value.storeId) || !id(value.epoch)) fail("Identidade da investigação inválida.");
    revision(value.revision); return value;
  }
  function owner(value) {
    keys(value, ["storeId", "caseId", "analysisId"], "Proprietário da evidência inválido.");
    if (![value.storeId, value.caseId, value.analysisId].every(id)) fail("Proprietário da evidência inválido.");
    return value;
  }
  function reference(value, expected = null) {
    if (value?.kind === "pending_native_evidence") return pendingReference(value, expected);
    keys(value, ["kind", "schemaVersion", "owner", "containerId", "manifestId", "manifestSha256", "memberCount"], "Referência de evidência inválida.");
    if (value.kind !== "native_evidence" || value.schemaVersion !== 1 || !id(value.containerId) || !id(value.manifestId)
      || !digest(value.manifestSha256) || !integer(value.memberCount, LIMITS.members)) fail("Referência de evidência inválida.");
    owner(value.owner);
    if (expected && !sameOwner(expected, value.owner)) fail("A evidência pertence a outro Caso ou análise.");
    return value;
  }
  function pendingReference(value, expected = null) {
    keys(value, ["kind", "schemaVersion", "token", "requestId", "owner", "containerId", "manifestId", "manifestSha256", "memberCount", "bytes", "expiresAt", "purpose"], "Preparação de evidência inválida.");
    if (value.kind !== "pending_native_evidence" || value.schemaVersion !== 1 || !id(value.token) || !id(value.requestId)
      || !id(value.containerId) || !id(value.manifestId) || !digest(value.manifestSha256) || !integer(value.memberCount, LIMITS.members)
      || !integer(value.bytes) || typeof value.expiresAt !== "string" || value.expiresAt.length > 128) fail("Preparação de evidência inválida.");
    owner(value.owner);
    if (expected && !sameOwner(expected, value.owner)) fail("A preparação pertence a outro Caso ou análise.");
    if (value.purpose?.kind === "new_container") {
      keys(value.purpose, ["kind"], "Finalidade da preparação inválida.");
      if (value.bytes > 32 * 1024 * 1024 || value.memberCount > 10000) fail("A captura excede o limite de novos registros.");
    } else if (value.purpose?.kind === "replace_container") {
      keys(value.purpose, ["kind", "baseManifestId", "baseManifestSha256"], "Finalidade da preparação inválida.");
      if (!id(value.purpose.baseManifestId) || !digest(value.purpose.baseManifestSha256)) fail("Lista anterior da preparação inválida.");
    } else fail("Finalidade da preparação inválida.");
    return value;
  }
  const referenceKey = value => JSON.stringify([value.kind, value.schemaVersion, value.owner.storeId, value.owner.caseId, value.owner.analysisId,
    value.containerId, value.manifestId, value.manifestSha256, value.memberCount, value.token, value.requestId, value.bytes, value.expiresAt,
    value.purpose?.kind, value.purpose?.baseManifestId, value.purpose?.baseManifestSha256]);
  function member(value, ref = null) {
    keys(value, ["containerId", "manifestId", "occurrenceId"], "Ocorrência de evidência inválida.");
    if (![value.containerId, value.manifestId, value.occurrenceId].every(id)
      || ref && (value.containerId !== ref.containerId || value.manifestId !== ref.manifestId)) fail("Ocorrência de outra lista de evidências.");
    return value;
  }
  function preview(value, ref) {
    if (value === null) return value;
    keys(value, ["kind", "columns", "rows", "total", "nextCursor"], "Prévia de evidência inválida.");
    if (value.kind !== "evidence_preview_page" || !Array.isArray(value.columns) || value.columns.length > LIMITS.previewColumns
      || value.columns.some(column => !id(column)) || new Set(value.columns).size !== value.columns.length
      || !Array.isArray(value.rows) || value.rows.length > LIMITS.previewRows || !integer(value.total, LIMITS.members)
      || value.total !== ref.memberCount || value.nextCursor != null && !id(value.nextCursor)) fail("Prévia de evidência inválida.");
    for (const row of value.rows) {
      keys(row, ["kind", "member", "cells"], "Uma prévia não é um registro de evidência.");
      if (row.kind !== "evidence_preview" || !Array.isArray(row.cells) || row.cells.length !== value.columns.length) fail("Uma prévia não é um registro de evidência.");
      member(row.member, ref);
      for (const cell of row.cells) {
        if (!object(cell) || !["missing", "null", "complete", "preview", "unavailable"].includes(cell.state)) fail("Célula de prévia inválida.");
        if (cell.state === "preview" && (typeof cell.text !== "string" || cell.text.length > 4096 || typeof cell.incomplete !== "boolean")) fail("Célula de prévia inválida.");
      }
    }
    return value;
  }
  function container(value, expected = null) {
    keys(value, ["kind", "reference", "preservedCount", "preview"], "Contêiner de evidências inválido.");
    if (value.kind !== "native_evidence_container") fail("Uma lista local não substitui a autoridade das evidências.");
    reference(value.reference, expected);
    if (!integer(value.preservedCount, LIMITS.members) || value.preservedCount !== value.reference.memberCount) fail("Contagem preservada inválida.");
    if (value.preview !== undefined) preview(value.preview, value.reference);
    return value;
  }
  function entries(document, caseId = null) {
    const found = [];
    for (const item of document.cases) {
      if (caseId !== null && item.id !== caseId) continue;
      const add = (value, itemIndex, field) => {
        if (value === undefined || value === null) return;
        // Only these original container positions are evidence. Nested authored
        // chart/filter JSON is retained verbatim and is not promoted to records.
        if (Array.isArray(value)) fail("Registros locais não são um contêiner nativo de evidências.");
        if (!object(value) || !own(value, "kind")) return;
        const expected = { storeId: document.store.storeId, caseId: item.id,
          analysisId: item.analysisContext?.analysisId ?? value.reference?.owner?.analysisId };
        container(value, expected);
        found.push({ caseId: item.id, itemIndex, field, view: value });
      };
      for (const field of ["rows", "events"]) add(item[field], null, field);
      if (item.items !== undefined && !Array.isArray(item.items)) fail("Lista de itens de Achados inválida.");
      for (const [index, child] of (item.items || []).entries()) {
        if (!object(child)) fail("Item de Achados inválido.");
        for (const field of ["rows", "events"]) add(child[field], index, field);
      }
    }
    return found;
  }
  function diagnostic(value, { tagged = false, storeId = null, cases = null } = {}) {
    keys(value, [...(tagged ? ["state"] : []), "caseId", "owner", "code", "message", "preservedCount", "readiness"], "Diagnóstico de preservação inválido.");
    if (tagged && value.state !== "unavailable" || !id(value.caseId) || !id(value.code) || typeof value.message !== "string" || value.message.length > 16384
      || !["preserved_only", "migration_required", "recovery_required"].includes(value.readiness)
      || value.preservedCount != null && !integer(value.preservedCount, 4294967295)) fail("Diagnóstico de preservação inválido.");
    if (value.owner != null) {
      owner(value.owner);
      const item = cases?.find(item => item.id === value.caseId);
      if (value.owner.caseId !== value.caseId || storeId && value.owner.storeId !== storeId
        || item?.analysisContext?.analysisId && item.analysisContext.analysisId !== value.owner.analysisId) fail("Diagnóstico de outro Caso ou análise.");
    }
    return value;
  }
  function legacyItemAliases(values) {
    if (!Array.isArray(values) || values.length > LIMITS.legacyAliases) fail("Mapa de associações legadas inválido.");
    const encoder = new TextEncoder(), seen = new Set();
    // The native cap covers the complete map. This readonly subset must fit
    // even with the required empty event-alias portion and version envelope.
    let bytes = encoder.encode(JSON.stringify({ schemaVersion: 1, eventAliases: [], itemAliases: [] })).length;
    for (const [index, entry] of values.entries()) {
      keys(entry, ["alias", "state"], "Associação legada inválida.");
      if (typeof entry.alias !== "string" || !utf8Within(entry.alias, LIMITS.legacyAliasBytes)
        || !["missing", "ambiguous"].includes(entry.state) || seen.has(entry.alias)) fail("Associação legada inválida.");
      seen.add(entry.alias); bytes += encoder.encode(JSON.stringify(entry)).length + (index ? 1 : 0);
      if (bytes > LIMITS.legacyMapBytes) fail("O mapa de associações legadas excede 1 MiB.");
    }
    return values;
  }
  function evidenceStates(values, { storeId, cases }) {
    if (!Array.isArray(values) || values.length > LIMITS.cases) fail("Assinaturas nativas ausentes.");
    const seen = new Set();
    for (const entry of values) {
      if (entry?.state === "unavailable") {
        diagnostic(entry, { tagged: true, storeId, cases });
        if (seen.has(entry.caseId)) fail("Estado nativo repetido."); seen.add(entry.caseId); continue;
      }
      keys(entry, ["state", "owner", "evidenceSignature", "preservedCount", "legacyItemAliases"], "Assinatura nativa inválida."); owner(entry.owner);
      const item = cases.find(item => item.id === entry.owner.caseId);
      if (entry.state !== "ready" || !item || entry.owner.storeId !== storeId || seen.has(entry.owner.caseId) || !id(entry.evidenceSignature)
        || !integer(entry.preservedCount, 4294967295) || item.analysisContext?.analysisId && item.analysisContext.analysisId !== entry.owner.analysisId) fail("Assinatura nativa de outro Caso ou análise.");
      if (own(entry, "legacyItemAliases")) legacyItemAliases(entry.legacyItemAliases);
      seen.add(entry.owner.caseId);
    }
    return values;
  }
  function document(value) {
    keys(value, ["evidenceViewVersion", "store", "active", "cases", "caseEvidence", "diagnostics"], "Documento de evidências inválido.");
    if (value.evidenceViewVersion !== 1 || !Array.isArray(value.cases) || value.cases.length > LIMITS.cases
      || value.active !== null && !id(value.active)) fail("Documento de evidências inválido.");
    stamp(value.store);
    const ids = new Set();
    for (const item of value.cases) {
      if (!object(item) || !id(item.id) || ids.has(item.id)) fail("Identidade do Caso inválida.");
      ids.add(item.id);
    }
    if (value.active !== null && !ids.has(value.active)) fail("Caso ativo ausente.");
    evidenceStates(value.caseEvidence, { storeId: value.store.storeId, cases: value.cases });
    if (!Array.isArray(value.diagnostics) || value.diagnostics.length > LIMITS.cases) fail("Diagnósticos nativos ausentes.");
    for (const entry of value.diagnostics) diagnostic(entry, { storeId: value.store.storeId, cases: value.cases });
    for (const item of value.cases) if (item.kind === "preserved_case_unavailable") {
      keys(item, ["kind", "id", "code"], "Caso preservado indisponível inválido.");
      if (!id(item.code) || !value.caseEvidence.some(entry => entry.state === "unavailable" && entry.caseId === item.id && entry.code === item.code)
        || !value.diagnostics.some(entry => entry.caseId === item.id && entry.code === item.code)) fail("O Caso preservado exige seu diagnóstico nativo.");
    }
    const all = entries(value), containers = new Set();
    for (const entry of all) {
      if (containers.has(entry.view.reference.containerId)) fail("O contêiner foi vinculado a mais de um local.");
      containers.add(entry.view.reference.containerId);
    }
    return value;
  }
  const clone = value => structuredClone(value);
  function forSave(value) {
    document(value);
    // Do not clone preview cells into a save payload. The narrow replacements
    // happen before cloning the authored metadata; previews never become Events.
    const cases = value.cases.map(item => ({ ...item, ...(item.items ? { items: item.items.map(child => ({ ...child })) } : {}) }));
    const result = { ...value, cases };
    for (const entry of entries(result)) {
      const item = cases.find(item => item.id === entry.caseId), target = entry.itemIndex === null ? item : item.items[entry.itemIndex];
      target[entry.field] = { kind: "native_evidence_container", reference: entry.view.reference, preservedCount: entry.view.preservedCount, preview: null };
    }
    return clone(result);
  }
  function receipt(value, request, preparedDocument) {
    keys(value, ["requestId", "committedStore", "currentStore", "evidence", "analysisContexts", "caseEvidence", "replayed", "reconcileRequired"], "Confirmação de salvamento inválida.");
    if (value.requestId !== request.requestId || typeof value.replayed !== "boolean" || typeof value.reconcileRequired !== "boolean"
      || !Array.isArray(value.evidence) || !Array.isArray(value.analysisContexts)) fail("Confirmação de salvamento inválida.");
    stamp(value.committedStore); stamp(value.currentStore);
    if (!sameStore(value.committedStore, request.expectedStore) || !sameStore(value.currentStore, request.expectedStore)
      || compare(value.committedStore.revision, request.expectedStore.revision) < 0 || compare(value.currentStore.revision, value.committedStore.revision) < 0) fail("Confirmação de outra autoridade ou revisão.");
    for (const ref of value.evidence) {
      reference(ref);
      if (ref.kind !== "native_evidence") fail("Uma preparação ainda não é um salvamento durável.");
      if (ref.owner.storeId !== request.expectedStore.storeId) fail("Referência de outra autoridade.");
    }
    const contexts = new Set();
    for (const context of value.analysisContexts) {
      const item = preparedDocument.cases.find(item => item.id === context?.caseId);
      if (!item || context.schemaVersion !== 1 || !id(context.analysisId) || !integer(context.configRevision) || !integer(context.visibilityRevision)
        || !object(context.config) || !Array.isArray(context.config.derivedFields) || !Array.isArray(context.config.references)
        || !Array.isArray(context.migrationDiagnostics) || contexts.has(context.caseId)
        || item.analysisContext?.analysisId && item.analysisContext.analysisId !== context.analysisId
        || integer(item.analysisContext?.configRevision) && context.configRevision < item.analysisContext.configRevision
        || integer(item.analysisContext?.visibilityRevision) && context.visibilityRevision < item.analysisContext.visibilityRevision) fail("Contexto de análise inválido no recibo.");
      contexts.add(context.caseId);
    }
    evidenceStates(value.caseEvidence, { storeId: value.committedStore.storeId, cases: preparedDocument.cases });
    return value;
  }
  function sourceReceipt(value) {
    keys(value, ["analysisContext", "sourceGeneration", "caseKey", "caseContentToken", "catalogSignature", "catalogEpoch"], "Recibo da seleção inválido.");
    const context = value.analysisContext;
    keys(context, ["caseId", "analysisId", "configRevision", "visibilityRevision"], "Contexto da seleção inválido.");
    if (!id(context.caseId) || !id(context.analysisId) || !integer(context.configRevision) || !integer(context.visibilityRevision)
      || !digest(value.catalogSignature) || !integer(value.catalogEpoch)) fail("Recibo da seleção inválido.");
    const dataset = integer(value.sourceGeneration) && value.caseKey == null && value.caseContentToken == null;
    const evidence = value.sourceGeneration == null && id(value.caseKey) && value.caseKey.length <= 512
      && id(value.caseContentToken) && new TextEncoder().encode(value.caseContentToken).length <= 128;
    if (!dataset && !evidence) fail("A seleção exige uma publicação nativa exata.");
    return value;
  }
  function rowHandle(value) {
    keys(value, ["id", "eventRef"], "Identificador do registro inválido.");
    if (!integer(value.id) || !id(value.eventRef)) fail("Identificador do registro inválido.");
    return value;
  }
  function requestBudget(value) {
    // Inputs here contain only validated bounded handles and metadata, never
    // record bodies. Stop per member before constructing a large request copy.
    const { rows, action, ...header } = value;
    const encoder = new TextEncoder(); let bytes = encoder.encode(JSON.stringify(header)).length;
    const values = rows || action?.members || [];
    if (action) bytes += encoder.encode(JSON.stringify({ ...action, ...(action.members ? { members: [] } : {}) })).length;
    for (const entry of values) {
      bytes += encoder.encode(JSON.stringify(entry)).length + 1;
      if (bytes > 2 * 1024 * 1024) fail("Os identificadores da seleção excedem o limite de 2 MiB.");
    }
    if (bytes > 2 * 1024 * 1024) fail("Os identificadores da seleção excedem o limite de 2 MiB.");
  }
  function create({ invoke, enabled = false, requestId = () => window.crypto.randomUUID() } = {}) {
    let view = null, phase = enabled ? "ready" : "inactive", generation = 0;
    const tickets = new WeakMap(), preparations = new WeakMap(), prepared = new WeakMap(), pending = new WeakMap(), publications = new Map(), opening = new Map(), sealed = new Map();
    const active = () => { if (!enabled) throw Error("NATIVE_EVIDENCE_INACTIVE: O fluxo de evidências nativas ainda não foi ativado."); };
    async function load() {
      active(); const mine = ++generation;
      const result = document(await invoke("cases_load_view", {}));
      if (mine !== generation) throw Error("EVIDENCE_VIEW_CHANGED: A leitura da investigação foi substituída.");
      // Only a fresh authoritative read can adopt a replacement store epoch.
      if (view && sameStore(view.store, result.store) && compare(result.store.revision, view.store.revision) < 0) throw Error("EVIDENCE_VIEW_CHANGED: A resposta da investigação é anterior à visão atual.");
      view = clone(result); phase = "ready"; return clone(view);
    }
    function prepareSave(value = view) {
      active(); if (!view) throw Error("EVIDENCE_VIEW_REQUIRED: Abra a investigação antes de salvar.");
      document(value);
      for (const item of value.cases) if (item.kind === "preserved_case_unavailable") {
        const issued = view.cases.find(prior => prior.id === item.id);
        if (issued?.kind !== "preserved_case_unavailable" || issued.code !== item.code) fail("O Caso indisponível deve manter o descritor emitido pela aplicação.");
      }
      for (const issued of view.cases) if (issued.kind === "preserved_case_unavailable") {
        const item = value.cases.find(item => item.id === issued.id);
        if (item && item.kind !== issued.kind) fail("O Caso preservado está indisponível para edição.");
      }
      if (!sameStore(value.store, view.store) || compare(value.store.revision, view.store.revision) !== 0) throw Error("EVIDENCE_VIEW_CHANGED: Releia a investigação antes de preparar outro salvamento.");
      const preparedDocument = forSave(value);
      // Tauri's generic Value decoding must not touch fractional metadata before
      // native exact admission. Keep the object private; never parse this back.
      const request = { requestId: requestId(), expectedStore: clone(view.store), documentJson: JSON.stringify(preparedDocument) };
      if (!utf8Within(request.documentJson, LIMITS.documentBytes)) fail("Os metadados da investigação excedem o limite de 16 MiB; as evidências originais permanecem preservadas.");
      if (!id(request.requestId)) fail("Identificador da operação inválido.");
      const preparedAuthorities = entries(preparedDocument).map(entry => entry.view.reference)
        .filter(ref => ref.kind === "pending_native_evidence").map(ref => ({ ref, captured: sealed.get(ref.token) }));
      if (preparedAuthorities.some(({ ref, captured }) => !captured || captured.abandoned || referenceKey(ref) !== referenceKey(captured.reference)))
        throw Error("EVIDENCE_PREPARATION_REQUIRED: A preparação expirou, foi cancelada ou pertence a outra operação. Preserve novamente a seleção.");
      const ticket = { request: clone(request) }; tickets.set(ticket, { request, preparedDocument, preparedAuthorities }); return ticket;
    }
    async function captureSource(request, { isCurrent = () => true } = {}) {
      active();
      keys(request, ["analysisContext", "sourceGeneration", "caseKey", "caseContentToken"], "Admissão da seleção inválida.");
      const context = request.analysisContext;
      keys(context, ["caseId", "analysisId", "configRevision", "visibilityRevision"], "Contexto da seleção inválido.");
      if (!id(context.caseId) || !id(context.analysisId) || !integer(context.configRevision) || !integer(context.visibilityRevision)) fail("Contexto da seleção inválido.");
      const dataset = integer(request.sourceGeneration) && request.caseKey == null && request.caseContentToken == null;
      const evidence = request.sourceGeneration == null && id(request.caseKey) && request.caseKey.length <= 512
        && id(request.caseContentToken) && new TextEncoder().encode(request.caseContentToken).length <= 128;
      if (!dataset && !evidence) fail("A seleção exige uma publicação nativa exata.");
      const captured = clone(request);
      if (!isCurrent()) throw Error("EVIDENCE_VIEW_CHANGED: A seleção foi substituída.");
      const result = sourceReceipt(await invoke("case_evidence_source_receipt", { request: clone(captured) }));
      const sameIdentity = (left, right) => left.caseId === right.caseId && left.analysisId === right.analysisId
        && left.configRevision === right.configRevision && left.visibilityRevision === right.visibilityRevision;
      if (!isCurrent() || !sameIdentity(result.analysisContext, captured.analysisContext)
        || (result.sourceGeneration ?? null) !== (captured.sourceGeneration ?? null) || (result.caseKey ?? null) !== (captured.caseKey ?? null)
        || (result.caseContentToken ?? null) !== (captured.caseContentToken ?? null)) throw Error("EVIDENCE_VIEW_CHANGED: O recibo não corresponde à seleção capturada.");
      return clone(result);
    }
    async function findMembers({ owner: target, source, rows, stationId = null }, { isCurrent = () => true } = {}) {
      mutationOwner(target); sourceReceipt(source);
      if (!Array.isArray(rows) || rows.length > 2000 || stationId !== null && !id(stationId)) fail("A consulta de pertencimento excede o recorte permitido.");
      const seen = new Set();
      for (const row of rows) { rowHandle(row); const key = JSON.stringify([row.id, row.eventRef]); if (seen.has(key)) fail("A consulta repete o mesmo registro."); seen.add(key); }
      const state = view.caseEvidence.find(entry => entry.state === "ready" && sameOwner(entry.owner, target));
      if (!state || !isCurrent()) throw Error("EVIDENCE_VIEW_CHANGED: A autoridade ou a seleção do Caso mudou.");
      const request = { store: { storeId: view.store.storeId, epoch: view.store.epoch }, owner: clone(target), caseEvidenceSignature: state.evidenceSignature,
        source, rows, stationId };
      requestBudget(request); const captured = clone(request);
      const targetCase = view.cases.find(item => item.id === target.caseId);
      const locations = new Map(entries(view, target.caseId).filter(entry => entry.field === "rows" && entry.itemIndex !== null)
        .map(entry => [entry.view.reference.containerId, { reference: clone(entry.view.reference), itemIndex: entry.itemIndex,
          itemId: typeof targetCase.items[entry.itemIndex].id === "string" ? targetCase.items[entry.itemIndex].id : null }]));
      const result = await invoke("case_evidence_find_members", { request: clone(captured) });
      mutationOwner(captured.owner);
      if (!isCurrent() || !sameStore(view.store, captured.store) || !view.caseEvidence.some(entry => entry.state === "ready" && sameOwner(entry.owner, captured.owner) && entry.evidenceSignature === captured.caseEvidenceSignature))
        throw Error("EVIDENCE_VIEW_CHANGED: A composição ou a seleção do Caso mudou durante a consulta.");
      keys(result, ["kind", "caseEvidenceSignature", "rows"], "Pertencimento nativo inválido.");
      if (result.kind !== "native_case_members" || result.caseEvidenceSignature !== captured.caseEvidenceSignature || !Array.isArray(result.rows) || result.rows.length !== captured.rows.length) fail("O pertencimento não corresponde à seleção capturada.");
      const encoder = new TextEncoder(); let bytes = encoder.encode(JSON.stringify({ kind: result.kind, caseEvidenceSignature: result.caseEvidenceSignature, rows: [] })).length;
      for (let index = 0; index < result.rows.length; index++) {
        const row = result.rows[index], expected = captured.rows[index]; keys(row, ["row", "state", "matches"], "Pertencimento inválido."); rowHandle(row.row);
        if (row.row.id !== expected.id || row.row.eventRef !== expected.eventRef || !Array.isArray(row.matches)
          || (row.state === "missing" ? row.matches.length !== 0 : row.state === "unique" ? row.matches.length !== 1 : row.state !== "ambiguous" || row.matches.length < 2)) fail("O pertencimento não corresponde ao registro selecionado.");
        bytes += encoder.encode(JSON.stringify({ ...row, matches: [] })).length + (index ? 1 : 0); const matched = new Set();
        for (const value of row.matches) {
          keys(value, ["containerId", "itemId", "itemIndex", "member"], "Ocorrência correspondente inválida."); const location = locations.get(value.containerId);
          if (!location || value.itemIndex !== location.itemIndex || value.itemId !== location.itemId) fail("A ocorrência corresponde a outro item de Achados."); member(value.member, location.reference);
          const memberKey = JSON.stringify([value.member.containerId, value.member.manifestId, value.member.occurrenceId]);
          if (value.member.containerId !== value.containerId || matched.has(memberKey)) fail("A ocorrência correspondente está repetida ou pertence a outro contêiner.");
          bytes += encoder.encode(JSON.stringify(value)).length + (matched.size ? 1 : 0); matched.add(memberKey); if (bytes > 1048576) fail("A resposta de pertencimento excede 1 MiB.");
        }
        if (bytes > 1048576) fail("A resposta de pertencimento excede 1 MiB.");
      }
      return clone(result);
    }
    function mutationOwner(target) {
      active(); owner(target);
      if (!view || phase === "reconcile_required") throw Error("EVIDENCE_VIEW_REQUIRED: Releia a investigação antes de alterar evidências.");
      const item = view.cases.find(item => item.id === target.caseId), issued = view.caseEvidence.find(entry => entry.owner?.caseId === target.caseId);
      if (!item || item.kind === "preserved_case_unavailable" || target.storeId !== view.store.storeId
        || target.analysisId !== (item.analysisContext?.analysisId || issued?.owner?.analysisId)) throw Error("EVIDENCE_OWNER_CHANGED: O Caso de destino está indisponível ou foi substituído.");
      return target;
    }
    function preparation(command, request) {
      if (!id(request.requestId)) fail("Identificador da operação inválido.");
      requestBudget(request);
      const saved = clone(request), ticket = { request: clone(saved) }; preparations.set(ticket, { command, request: saved }); return ticket;
    }
    function prepareCapture({ target, source, rows }) {
      mutationOwner(target); sourceReceipt(source);
      if (!Array.isArray(rows) || !rows.length || rows.length > 10000) fail("Selecione de 1 a 10.000 registros para preservar.");
      const seen = new Set(), encoder = new TextEncoder(); let bytes = 0;
      for (const row of rows) {
        rowHandle(row); const key = JSON.stringify([row.id, row.eventRef]); bytes += encoder.encode(JSON.stringify(row)).length + 1;
        if (bytes > 2 * 1024 * 1024) fail("Os identificadores da seleção excedem o limite de 2 MiB.");
        if (seen.has(key)) fail("A seleção repete o mesmo registro."); seen.add(key);
      }
      return preparation("case_evidence_prepare", { requestId: requestId(), expectedStore: clone(view.store), target, source, rows });
    }
    function prepareMembership({ reference: ref, action }) {
      reference(ref); mutationOwner(ref.owner);
      if (action?.kind === "remove") {
        keys(action, ["kind", "members"], "Remoção de ocorrências inválida.");
        if (!Array.isArray(action.members) || !action.members.length || action.members.length > LIMITS.members) fail("Nenhuma ocorrência válida foi selecionada.");
        const seen = new Set(), encoder = new TextEncoder(); let bytes = 0;
        for (const entry of action.members) {
          member(entry, ref); bytes += encoder.encode(JSON.stringify(entry)).length + 1;
          if (bytes > 2 * 1024 * 1024) fail("Os identificadores da seleção excedem o limite de 2 MiB.");
          if (seen.has(entry.occurrenceId)) fail("A ocorrência está repetida na remoção."); seen.add(entry.occurrenceId);
        }
      } else if (action?.kind === "restore") {
        keys(action, ["kind", "target"], "Restauração de ocorrências inválida."); reference(action.target, ref.owner);
        if (action.target.containerId !== ref.containerId) fail("A restauração pertence a outro contêiner.");
      } else fail("Operação de ocorrências inválida.");
      return preparation("case_evidence_prepare_membership", { requestId: requestId(), expectedStore: clone(view.store), reference: ref, action });
    }
    async function discard(ticket) {
      const captured = preparations.get(ticket);
      if (!captured || captured.state === "saving" || captured.state === "committed") return false;
      captured.abandoned = true;
      if (!captured.reference) return false; // A late preparation releases itself.
      sealed.delete(captured.reference.token);
      if (captured.discarding) return captured.discarding;
      const request = { store: { storeId: captured.request.expectedStore.storeId, epoch: captured.request.expectedStore.epoch }, reference: clone(captured.reference) };
      captured.discarding = (async () => {
        try {
          const result = await invoke("case_evidence_discard", { request });
          if (result?.discarded === true) { captured.state = "discarded"; sealed.delete(captured.reference.token); return true; }
          return false;
        } catch { return false; }
        finally { captured.discarding = null; }
      })();
      return captured.discarding;
    }
    async function prepare(ticket, { isCurrent = () => true } = {}) {
      active(); const captured = preparations.get(ticket);
      if (!captured) throw Error("EVIDENCE_PREPARATION_REQUIRED: Preserve a solicitação original de preparação.");
      if (captured.abandoned || !isCurrent()) { await discard(ticket); throw Error("EVIDENCE_VIEW_CHANGED: A preparação foi cancelada ou substituída."); }
      const { command, request } = captured, target = request.target || request.reference.owner;
      mutationOwner(target);
      if (!sameStore(view.store, request.expectedStore) || compare(view.store.revision, request.expectedStore.revision) !== 0) throw Error("EVIDENCE_VIEW_CHANGED: Releia a investigação antes de preparar evidências.");
      if (prepared.has(ticket)) return clone(prepared.get(ticket));
      if (pending.has(ticket)) {
        const result = await pending.get(ticket);
        if (captured.abandoned || !isCurrent()) { await discard(ticket); throw Error("EVIDENCE_VIEW_CHANGED: A preparação foi cancelada ou substituída."); }
        return clone(result);
      }
      const job = (async () => {
        const result = await invoke(command, { request: clone(request) });
        const ref = command === "case_evidence_prepare" ? result : result?.reference;
        pendingReference(ref, target);
        if (ref.requestId !== request.requestId) fail("Preparação de outra solicitação.");
        if (command === "case_evidence_prepare") {
          if (ref.purpose.kind !== "new_container" || ref.memberCount !== request.rows.length) fail("A preparação não corresponde à seleção exata.");
        } else {
          keys(result, ["reference", "undo", "changedCount", "remainingCount"], "Recibo de ocorrências inválido.");
          reference(result.undo, target);
          const expectedPurpose = request.reference.kind === "pending_native_evidence" ? request.reference.purpose
            : { kind: "replace_container", baseManifestId: request.reference.manifestId, baseManifestSha256: request.reference.manifestSha256 };
          if (referenceKey(result.undo) !== referenceKey(request.reference) || ref.containerId !== request.reference.containerId
            || ref.purpose.kind !== expectedPurpose.kind || ref.purpose.baseManifestId !== expectedPurpose.baseManifestId
            || ref.purpose.baseManifestSha256 !== expectedPurpose.baseManifestSha256 || !integer(result.changedCount, LIMITS.members)
            || result.remainingCount !== ref.memberCount) fail("O recibo não corresponde às ocorrências selecionadas.");
          if (request.action.kind === "remove" && (result.changedCount !== request.action.members.length
            || result.remainingCount !== request.reference.memberCount - result.changedCount)) fail("A remoção não confirmou todas as ocorrências selecionadas.");
          if (request.action.kind === "restore" && result.remainingCount !== request.action.target.memberCount) fail("A restauração não confirmou a lista de ocorrências solicitada.");
        }
        captured.reference = clone(ref); sealed.set(ref.token, captured);
        // Native preparation has eight live slots under an aggregate byte cap. Expired/abandoned tokens
        // must not retain their handle arrays for the lifetime of this client.
        // Save tickets retain private authority for an uncertain commit/retry.
        while (sealed.size > LIMITS.pending) sealed.delete(sealed.keys().next().value);
        try {
          mutationOwner(target);
          if (captured.abandoned || !isCurrent() || !sameStore(view.store, request.expectedStore) || compare(view.store.revision, request.expectedStore.revision) !== 0) throw Error("EVIDENCE_VIEW_CHANGED: A investigação mudou durante a preparação.");
        } catch (error) { await discard(ticket); throw error; }
        prepared.set(ticket, clone(result)); return result;
      })().finally(() => pending.delete(ticket));
      pending.set(ticket, job); return clone(await job);
    }
    async function previewPage(ref, { cursor = null, limit = LIMITS.previewRows, columns = [], isCurrent = () => true } = {}) {
      active(); reference(ref); mutationOwner(ref.owner);
      if (!integer(limit, LIMITS.previewRows) || limit < 1 || cursor !== null && !id(cursor) || !Array.isArray(columns)
        || columns.length > LIMITS.previewColumns || columns.some(column => !id(column)) || new Set(columns).size !== columns.length) fail("Recorte de prévia inválido.");
      if (!isCurrent()) throw Error("EVIDENCE_VIEW_CHANGED: A prévia foi substituída.");
      const request = { store: { storeId: view.store.storeId, epoch: view.store.epoch }, reference: clone(ref), cursor, limit, columns: columns.slice() };
      const capturedReference = referenceKey(request.reference);
      const result = await invoke("case_evidence_preview", { request: clone(request) });
      mutationOwner(request.reference.owner);
      reference(ref);
      if (!isCurrent() || !sameStore(view.store, request.store) || referenceKey(ref) !== capturedReference
        || columns.length !== request.columns.length || columns.some((column, index) => column !== request.columns[index])) throw Error("EVIDENCE_VIEW_CHANGED: A investigação ou a prévia foi substituída.");
      preview(result, request.reference);
      if (result.rows.length > request.limit || result.columns.length !== request.columns.length
        || result.columns.some((column, index) => column !== request.columns[index])) fail("A prévia não corresponde ao recorte solicitado.");
      return clone(result);
    }
    async function openCase({ caseId, analysisContext, stationId = null, isCurrent = () => true, force = false, previousToken = null }) {
      active();
      if (!view || phase === "reconcile_required") throw Error("EVIDENCE_VIEW_REQUIRED: Releia a investigação antes de abrir sua análise.");
      if (!object(analysisContext) || analysisContext.caseId !== caseId || !id(analysisContext.analysisId)
        || !integer(analysisContext.configRevision) || !integer(analysisContext.visibilityRevision) || stationId !== null && !id(stationId)) fail("Admissão da análise inválida.");
      const captured = view.caseEvidence.find(entry => entry.state === "ready" && entry.owner.caseId === caseId && entry.owner.analysisId === analysisContext.analysisId);
      if (!captured) throw Error("EVIDENCE_UNAVAILABLE: A autoridade deste Caso está indisponível; seus registros não foram substituídos por uma lista vazia.");
      const request = { store: { storeId: view.store.storeId, epoch: view.store.epoch }, analysisContext: clone(analysisContext),
        stationId, evidenceSignature: captured.evidenceSignature };
      const current = () => isCurrent() && view && sameStore(view.store, request.store)
        && view.caseEvidence.some(entry => entry.state === "ready" && sameOwner(entry.owner, captured.owner) && entry.evidenceSignature === request.evidenceSignature);
      if (!current()) throw Error("EVIDENCE_VIEW_CHANGED: O Caso ou suas evidências mudaram.");
      const key = JSON.stringify([request.store, captured.owner, request.evidenceSignature, stationId]);
      const cached = publications.get(key);
      if (cached && (!force || previousToken && cached.publication.caseContentToken !== previousToken)) return clone(cached);
      if (opening.has(key)) {
        const result = await opening.get(key);
        if (!current()) throw Error("EVIDENCE_VIEW_CHANGED: O Caso ou suas evidências mudaram.");
        return clone(result);
      }
      const job = (async () => {
        const result = await invoke("case_evidence_open", { request });
        if (!current()) throw Error("EVIDENCE_VIEW_CHANGED: O Caso ou suas evidências mudaram.");
        if (result?.state === "unavailable") {
          keys(result, ["state", "code", "preservedCount", "materializationLimit"], "Indisponibilidade da análise inválida.");
          if (!id(result.code) || !integer(result.preservedCount, 4294967295) || !integer(result.materializationLimit)) fail("Indisponibilidade da análise inválida.");
          // Preserved records remain manageable/exportable. This is never an
          // empty publication and is never routed to a Dataset request.
          return clone(result);
        }
        keys(result, ["state", "publication"], "Publicação da análise inválida.");
        const publication = result.publication;
        keys(publication, ["caseKey", "caseContentToken", "caseEvidenceSignature", "evidenceSignature", "preservedCount", "analyticalCount"], "Publicação da análise inválida.");
        if (result.state !== "ready" || !id(publication.caseKey) || publication.caseKey.length > 512 || !id(publication.caseContentToken)
          || publication.caseContentToken.length > 128 || new TextEncoder().encode(publication.caseContentToken).length > 128
          || publication.caseEvidenceSignature !== request.evidenceSignature || !id(publication.evidenceSignature) || !integer(publication.preservedCount, 4294967295)
          || !integer(publication.analyticalCount, publication.preservedCount)) fail("Publicação da análise inválida.");
        publications.set(key, clone(result));
        if (publications.size > 3) publications.delete(publications.keys().next().value);
        return clone(result);
      })().finally(() => opening.delete(key));
      opening.set(key, job); return job;
    }
    async function save(ticket) {
      active(); const saved = tickets.get(ticket);
      if (!saved) throw Error("EVIDENCE_SAVE_REQUIRED: Preserve a solicitação original de salvamento.");
      const { request, preparedDocument, preparedAuthorities } = saved;
      if (!view || !sameStore(view.store, request.expectedStore)) throw Error("EVIDENCE_VIEW_CHANGED: A autoridade da investigação foi substituída.");
      for (const prior of preparedDocument.cases) {
        const current = view.cases.find(item => item.id === prior.id);
        if (!current) continue; // A retired owner may still have a lost-ack receipt.
        const priorAnalysis = prior.analysisContext?.analysisId || entries(preparedDocument, prior.id)[0]?.view.reference.owner.analysisId;
        const currentAnalysis = current.analysisContext?.analysisId || entries(view, current.id)[0]?.view.reference.owner.analysisId;
        if (priorAnalysis && currentAnalysis && priorAnalysis !== currentAnalysis) throw Error("EVIDENCE_OWNER_CHANGED: O Caso foi substituído por outra análise.");
      }
      if (pending.has(ticket)) return pending.get(ticket);
      if (preparedAuthorities.some(({ captured }) => captured?.abandoned)) throw Error("EVIDENCE_PREPARATION_REQUIRED: A preparação foi cancelada; preserve novamente a seleção.");
      for (const { captured } of preparedAuthorities) if (captured) captured.state = "saving";
      const job = (async () => {
        // Retries retain the original id, CAS and document, even after a reload
        // has observed a later removal. Never reinterpret them as a new append.
        const received = await invoke("cases_save_view", { request: clone(request) });
        try {
          const result = receipt(received, request, preparedDocument);
          for (const { ref, captured } of preparedAuthorities) {
            if (captured) { captured.state = "committed"; sealed.delete(ref.token); }
          }
          if (!view || !sameStore(view.store, request.expectedStore)) { phase = "reconcile_required"; return result; }
          const advanced = compare(view.store.revision, request.expectedStore.revision) > 0;
          if (result.reconcileRequired || compare(result.currentStore.revision, result.committedStore.revision) > 0 || advanced) {
            phase = "reconcile_required"; return result;
          }
          const next = clone(preparedDocument); next.store = clone(result.currentStore); next.caseEvidence = clone(result.caseEvidence);
          for (const context of result.analysisContexts) {
            const item = next.cases.find(item => item.id === context.caseId);
            if (item.kind !== "preserved_case_unavailable") item.analysisContext = clone(context);
          }
          const refs = new Map(result.evidence.map(ref => [ref.containerId, ref]));
          for (const entry of entries(next)) {
            const ref = refs.get(entry.view.reference.containerId);
            if (!ref) { if (entry.view.reference.kind === "pending_native_evidence") fail("Preparação não confirmada no salvamento."); continue; }
            reference(ref, entry.view.reference.owner);
            entry.view.reference = clone(ref); entry.view.preservedCount = ref.memberCount;
          }
          document(next); view = next; phase = "ready"; generation++; return result;
        } catch (error) { phase = "reconcile_required"; throw error; }
      })().finally(() => pending.delete(ticket));
      pending.set(ticket, job); return job;
    }
    return { load, prepareSave, save, captureSource, findMembers, prepareCapture, prepareMembership, prepare, discard, preview: previewPage, openCase, status: () => phase, document: () => view ? clone(view) : null,
      analysisShape: caseId => view ? analysisShape(view.cases.find(item => item.id === caseId)) : null,
      stamp: () => view ? clone(view.store) : null,
      containers: caseId => view ? clone(entries(view, caseId)) : [],
      caseState: caseId => view ? clone(view.caseEvidence.find(entry => (entry.owner?.caseId || entry.caseId) === caseId) || null) : null,
      preservedCount: caseId => {
        if (!view) return null;
        if (view.cases.find(item => item.id === caseId)?.kind === "preserved_case_unavailable") return view.diagnostics.find(entry => entry.caseId === caseId)?.preservedCount ?? null;
        return entries(view, caseId).reduce((total, entry) => total + entry.view.preservedCount, 0);
      } };
  }
  // A local dirty/ownership comparison, never a native evidence signature.
  // Only the ordered native containers and the metadata used by admission enter it.
  function analysisShape(item) {
    if (!item || item.kind === "preserved_case_unavailable") return null;
    return JSON.stringify((item.items || []).flatMap(entry => {
      if (entry.rows == null) return [];
      container(entry.rows);
      return [[referenceKey(entry.rows.reference), entry.stationId ?? null, entry.artifactId ?? null, entry.origin ?? null]];
    }));
  }
  return { active: true, create, metadataDocument: forSave, analysisShape, limits: LIMITS, validate: { utf8Within, revision, stamp, owner, reference, pendingReference, member, preview, container, document, receipt, diagnostic, evidenceStates, legacyItemAliases, sourceReceipt, rowHandle }, compareRevisions: compare };
})();
