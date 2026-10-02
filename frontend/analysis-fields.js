/* Explicit field choices supplement sampled suggestions without discovering data. */
window.AnalysisFields = (() => {
  "use strict";
  const EXACT = "\u0000exact-field-path", LIMIT = 64, MAX_BYTES = 4096;
  const choices = new Map(), controls = new WeakMap();
  let pendingEntry = null, serial = 0;
  const notify = () => { if (typeof CustomEvent !== "undefined") document.dispatchEvent?.(new CustomEvent("analysis-fields-change")); };
  const capture = (scope = workspaceScope()) => ({ owner: window.AnalysisContexts?.capture(), scope,
    signature: scope === "case" ? caseSig() : null });
  const isCurrent = value => !!value && value.scope === workspaceScope()
    && (!value.owner || window.AnalysisContexts.isCurrent(value.owner))
    && (value.signature === null || value.signature === caseSig());
  const key = context => JSON.stringify([context.scope, context.owner, context.signature]);
  function valid(field) {
    return typeof field === "string" && field.length <= MAX_BYTES && !!field.trim()
      && !/[\u0000-\u001f\u007f]/.test(field) && new TextEncoder().encode(field).length <= MAX_BYTES;
  }
  const parents = () => (state.derivedFields || []).map(field => field.name).filter(valid);
  const base = () => [...new Set([...(state.columns || []), ...parents()].filter(valid))];
  function local(scope) {
    if (scope !== workspaceScope()) return [];
    const current = key(capture(scope));
    return [...choices.values()].filter(value => value.context === current).map(value => value.field);
  }
  function available(field, scope = workspaceScope()) {
    if (scope !== workspaceScope() || state.analysisDefinitionsPending || !valid(field)) return false;
    return base().includes(field) || local(scope).includes(field) || parents().some(parent => field.startsWith(parent + "."));
  }
  function names(scope = workspaceScope(), authored = []) {
    return [...new Set([...(scope === workspaceScope() ? base() : []), ...local(scope), ...authored.filter(valid)])];
  }
  function register(field, context) {
    let contextKey = key(context);
    for (const [entryKey, entry] of choices) if (entry.context === contextKey) {
      contextKey = entry.context;
      if (entry.field === field) choices.delete(entryKey);
    }
    choices.set(++serial, { context: contextKey, field });
    while (choices.size > LIMIT) choices.delete(choices.keys().next().value);
    return field;
  }
  function remember(field, context = capture()) {
    if (!isCurrent(context)) throw Error("O contexto mudou. Escolha o campo novamente no Caso e na fonte atuais.");
    if (!valid(field)) throw Error("Informe um caminho não vazio, sem caracteres de controle e com até 4 KiB.");
    if (!available(field, context.scope)) throw Error("Use um campo do contexto atual ou o caminho de um filho de um campo derivado deste Caso.");
    if (base().includes(field)) return field;
    return register(field, context);
  }
  function admitted(node, admission = state.detailAdmission, event = state.currentDetailEv) {
    return !!node?.original && valid(node.path) && !!admission && admission === state.detailAdmission
      && !$("#drawer").hidden && event === state.currentDetailEv
      && detailAdmissionCurrent(admission) && Object.hasOwn(event?.fields || {}, node.path);
  }
  function observe(node, admission = state.detailAdmission, event = state.currentDetailEv) {
    if (!admitted(node, admission, event) || state.analysisDefinitionsPending) return false;
    if (base().includes(node.path)) return true;
    register(node.path, capture()); notify(); return true;
  }
  function request(anchor, choose, context = capture()) {
    const callback = field => {
      try { remember(field, context); choose(field); pendingEntry = null; notify(); return true; }
      catch (error) { toast(String(error.message || error), "info"); return false; }
    };
    pendingEntry = { context, callback };
    openNamePop(anchor, callback, { title: "Caminho exato do campo", placeholder: "ex.: payload.usuario.id", exact: true, confirmLabel: "Usar" });
  }
  function cancelStaleEntry() {
    if (!pendingEntry || isCurrent(pendingEntry.context)) return;
    if (typeof namePopCb !== "undefined" && namePopCb === pendingEntry.callback) closeNamePop(false);
    pendingEntry = null;
  }
  function refresh() {
    cancelStaleEntry();
    if (state.analysisDefinitionsPending || state.loadOverlay || window.WorkspaceContext?.changing || window.WorkspaceContext?.sourceBusy) return;
    notify();
  }
  function control(select, { value = "", fixed = [], exclude = [], scope = workspaceScope(), choose } = {}) {
    cancelStaleEntry();
    const context = capture(scope), fields = names(scope, [value]);
    const known = new Set([...base(), ...local(scope)]), declared = parents();
    const enabled = field => scope === workspaceScope() && !state.analysisDefinitionsPending
      && (known.has(field) || declared.some(parent => field.startsWith(parent + ".")));
    const fixedValues = new Set(fixed.map(([field]) => field));
    const excluded = new Set(exclude);
    const options = fixed.map(([field, label]) => ({ field, label, disabled: false }));
    for (const field of fields) if (!fixedValues.has(field) && (!excluded.has(field) || field === value)) options.push({ field,
      label: `${trunc(colLabel(field), 80)}${enabled(field) ? "" : " (campo indisponível)"}`, disabled: !enabled(field) });
    options.push({ field: EXACT, label: "Usar caminho exato…", disabled: false });
    const optionsKey = JSON.stringify(options);
    if (controls.get(select) !== optionsKey) {
      controls.set(select, optionsKey); select.replaceChildren();
      for (const item of options) { const option = el("option", "", item.label); option.value = item.field; option.title = item.field === EXACT ? "" : item.field; option.disabled = item.disabled; select.append(option); }
    }
    select.value = value; select.setAttribute("data-exact-field-picker", "");
    select.disabled = !!state.analysisDefinitionsPending || !!state.loadOverlay;
    const apply = field => {
      if (!isCurrent(context)) { toast("O contexto mudou. Escolha o campo novamente.", "info"); return false; }
      if (!fixedValues.has(field) && !available(field, scope)) { toast("Este campo não está disponível no contexto atual.", "info"); return false; }
      control(select, { value: field, fixed, exclude, scope, choose }); choose?.(field); return true;
    };
    select.onchange = () => {
      if (select.disabled) return;
      const field = select.value;
      if (field === EXACT) { select.value = value; request(select, apply, context); }
      else if (!apply(field)) select.value = value;
    };
  }
  document.addEventListener?.("analysis-context-change", event => { cancelStaleEntry(); if (event.detail?.active) refresh(); });
  document.addEventListener?.("workspace-context-change", cancelStaleEntry);
  return { EXACT, capture, isCurrent, valid, names, available, remember, admitted, observe, request, control, refresh };
})();
