/* Exact field actions resolve comparator text without formatting it in JavaScript. */
window.CanonicalFields = (() => {
  "use strict";
  const latest = "canonical-field-action";
  let pending = null, serial = 0;
  const identityEqual = (a, b) => !!a && !!b && ["caseId", "analysisId", "configRevision", "visibilityRevision"].every(key => a[key] === b[key]);
  function rawValue(event, column) {
    if (["id", "event_ref", "timestamp", "source", "level", "code", "name", "description", "message", "raw"].includes(column)) return event?.[column];
    return event?.fields && Object.hasOwn(event.fields, column) ? event.fields[column] : undefined;
  }
  function capture(event, column, options = {}) {
    const scope = workspaceScope();
    return { row: { id: event?.id, eventRef: event?.event_ref }, column, owner: window.AnalysisContexts?.capture(), scope,
      signature: scope === "case" ? caseSig() : null, evidence: scope === "case" ? caseEvents("analysis") : null,
      anchor: options.anchor || null, historical: !!options.historical, guard: options.guard || (() => true),
      literal: Object.hasOwn(options, "literal") ? options.literal : rawValue(event, column) };
  }
  function current(action) {
    return action.scope === workspaceScope() && (!action.owner || window.AnalysisContexts.isCurrent(action.owner))
      && (action.signature === null || action.signature === caseSig()) && action.guard();
  }
  function assertCurrent(action) { if (!current(action)) throw Error("O contexto mudou. Abra o menu novamente no registro atual."); }
  function focusBack(action) { if (current(action)) filterFocusTarget(action.anchor)?.focus?.({ preventScroll: true }); }
  function cancel({ focus = false } = {}) {
    if (!pending) return;
    const action = pending.action; pending = null; serial++; window.Tasks?.cancelLatest(latest);
    if (focus) focusBack(action);
  }
  function historical(action) {
    const value = action.literal;
    if (value === undefined) return { presence: "missing", canonicalText: null };
    if (value === null) return { presence: "null", canonicalText: action.column === "timestamp" ? null : "null" };
    if (typeof value === "string" || typeof value === "boolean") return { presence: "present", canonicalText: String(value) };
    throw Error("O texto canônico exato deste valor numérico ou estruturado não está disponível na evidência histórica. O valor preservado não foi substituído pela análise atual.");
  }
  function validate(result, action, caseKey, caseContentToken = null) {
    const receipt = result?.receipt, row = result?.row;
    if (result?.kind !== "exact_field" || result.version !== 1 || result.column !== action.column
      || row?.id !== action.row.id || row?.eventRef !== action.row.eventRef
      || !identityEqual(receipt?.analysisContext, action.owner?.identity)
      || (action.scope === "case" ? receipt?.caseKey !== caseKey || receipt?.sourceGeneration !== null : receipt?.caseKey !== null || receipt?.sourceGeneration !== action.owner?.sourceGeneration)
      || (action.scope === "case" ? typeof receipt?.caseContentToken !== "string" || !receipt.caseContentToken || receipt.caseContentToken.length > 128 || new TextEncoder().encode(receipt.caseContentToken).length > 128 : receipt?.caseContentToken !== null)
      || action.scope === "case" && receipt.caseContentToken !== caseContentToken
      || typeof receipt?.catalogSignature !== "string" || !/^[a-f0-9]{64}$/i.test(receipt.catalogSignature) || !Number.isSafeInteger(receipt?.catalogEpoch) || receipt.catalogEpoch < 0) throw Error("A resposta do valor não corresponde ao registro e contexto solicitados.");
    if (result.presence === "missing") {
      if (result.valueType !== null || result.canonicalText !== null) throw Error("Resposta de campo ausente inválida.");
    } else if (result.presence === "null") {
      if (result.valueType !== "null" || result.canonicalText !== (action.column === "timestamp" ? null : "null")) throw Error("Resposta de valor nulo inválida.");
    } else if (result.presence !== "present" || !["string", "number", "boolean", "array", "object"].includes(result.valueType) || typeof result.canonicalText !== "string") throw Error("Resposta de valor canônico inválida.");
    return result;
  }
  async function resolve(action, request) {
    assertCurrent(action);
    if (action.historical) return historical(action);
    if (!Number.isSafeInteger(action.row.id) || action.row.id < 0 || typeof action.row.eventRef !== "string" || !action.row.eventRef) throw Error("Este registro não tem uma referência estável para recuperar o valor exato.");
    if (action.owner) action.owner = await window.AnalysisContexts.prepare(action.owner);
    assertCurrent(action); if (pending !== request) throw Error("Operação cancelada.");
    const args = await caseArgs({ id: action.row.id, eventRef: action.row.eventRef, column: action.column,
      analysisContext: action.owner?.identity ?? null, sourceGeneration: action.owner?.sourceGeneration ?? null,
      ...(action.scope === "case" ? { caseEvents: action.evidence } : {}) }, false, null, { canonical: true });
    assertCurrent(action); if (pending !== request) throw Error("Operação cancelada.");
    let admission = { caseKey: args.caseKey ?? null, caseContentToken: args.caseContentToken ?? null };
    const result = await api("analysis_field_text", args, { silent: true, latest, analysisOwner: action.owner, caseEvents: action.evidence,
      onCasePrepared: next => { assertCurrent(action); if (pending !== request) throw Error("Operação cancelada."); admission = next; } });
    assertCurrent(action); if (pending !== request) throw Error("Operação cancelada.");
    return validate(result, action, admission.caseKey, admission.caseContentToken);
  }
  async function run(action, perform, { retainPending = false } = {}) {
    cancel(); const request = { id: ++serial, action }; pending = request;
    try {
      const result = await resolve(action, request);
      if (pending !== request || request.id !== serial) return false;
      assertCurrent(action); if (!retainPending) pending = null;
      await perform(result, () => pending === request && request.id === serial && current(action));
      if (pending === request) pending = null;
      return true;
    } catch (error) {
      if (request.id !== serial || pending && pending !== request) return false;
      pending = null; toast(String(error?.message || error), /contexto mudou|cancelad/i.test(String(error)) ? "info" : "err"); focusBack(action); return false;
    }
  }
  function filter(action, { op = null, apply = false } = {}) {
    return run(action, result => {
      const text = result.canonicalText, operator = op || (text === null ? "empty" : action.column === "timestamp" ? "between" : "equals_exact");
      if (apply) addFilter({ column: action.column, op: operator, value: text ?? "", value2: operator === "between" ? text ?? "" : null });
      else openValueFilter(action.column, text, action.anchor, operator);
    });
  }
  function copy(action) {
    return run(action, async (result, ownsCompletion) => {
      if (result.canonicalText === null) throw Error("O campo não tem um valor para copiar neste registro.");
      const focusAtWrite = document.activeElement;
      await navigator.clipboard.writeText(result.canonicalText);
      if (ownsCompletion()) {
        toast("Valor copiado.", "ok");
        if (document.activeElement === focusAtWrite) focusBack(action);
      }
    }, { retainPending: true });
  }
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && pending) { cancel({ focus: true }); event.preventDefault(); event.stopPropagation(); }
  }, true);
  const selection = (action, text) => ({ ...action, historical: true, literal: text });
  return { capture, filter, copy, selection, cancel, validate };
})();
