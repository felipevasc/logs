/* Preserved-member detail reuses the drawer. No display DTO becomes an Event. */
window.CaseEvidenceDetail = (() => {
  "use strict";
  const tasks = ["case_evidence_member_detail", "case_evidence_member_field_text", "case_evidence_member_java_trace"];
  const values = new WeakMap(); let active = null;
  const current = model => active === model && model.request === detailRequest && !$("#drawer").hidden && model.guard()
    && (!model.ticket || model.reader.current(model.ticket));
  const display = (column, text) => window.EvidenceUI ? window.EvidenceUI.redact({ [column]: text })[column] : text;
  const focus = anchor => filterFocusTarget(anchor)?.focus?.({ preventScroll: true });
  function clear() {
    if (!active) return;
    const previous = active; active = null;
    if (values.get(detailValueNode)?.model === previous) detailValueReturnFocus = null;
    if ($("#pane-raw").oncontextmenu === previous.rawMenu) $("#pane-raw").oncontextmenu = previous.previousRawMenu || null;
    for (const task of tasks) window.Tasks?.cancelLatest(`preserved:${task}`);
  }
  function reader() {
    const services = nativeEvidenceServices();
    return services.preserved ||= window.CaseEvidencePreserved.create({ client: services.client, getStore: () => state.cases, currentCase: () => activeCase(),
      invoke: (command, args) => api(command, args, { silent: true, latest: `preserved:${command}` }) });
  }
  async function open(reference, member = null, { guard = () => true } = {}) {
    const store = state.cases, item = activeCase(), epoch = store?.store?.epoch;
    if (!guard() || reference?.owner?.caseId !== item?.id || reference?.owner?.storeId !== store?.store?.storeId) {
      toast("O contexto mudou. Abra a evidência novamente.", "info"); return false;
    }
    const ownsCase = () => guard() && state.cases === store && activeCase() === item && state.cases.store?.epoch === epoch;
    clear(); const request = ++detailRequest; showDetailLoading();
    const model = { request, guard: ownsCase, reader: reader(), ticket: null, page: 0, pageRequest: 0, cursors: [null], offsets: [0], data: null, action: 0 };
    active = model;
    try {
      if (!current(model)) return false;
      if (!member) {
        const preview = await nativeEvidenceServices().client.preview(reference, { limit: 1, columns: [], isCurrent: () => current(model) });
        if (!current(model)) return false;
        member = preview.rows[0]?.member;
        if (!member) { $("#pane-overview").textContent = "Este contêiner preservado não contém ocorrências."; return false; }
      }
      model.ticket = model.reader.capture({ reference, member, isCurrent: () => active === model && model.request === detailRequest && !$("#drawer").hidden && ownsCase() });
      return await page(model, 0);
    } catch (error) { if (current(model)) $("#pane-overview").textContent = `Não foi possível abrir a evidência preservada: ${String(error)}`; return false; }
  }
  async function page(model, index) {
    if (!current(model)) return false;
    const request = ++model.pageRequest;
    try {
      const data = await model.reader.detail(model.ticket, { cursor: model.cursors[index] });
      if (!current(model) || request !== model.pageRequest) return false;
      const end = model.offsets[index] + data.fields.length;
      if (end > data.fieldsTotal || (data.nextCursor ? !data.fields.length || end >= data.fieldsTotal : end !== data.fieldsTotal))
        throw Error("A página de campos não corresponde à contagem preservada.");
      model.data = data; model.page = index; model.cursors[index + 1] = data.nextCursor; model.offsets[index + 1] = model.offsets[index] + data.fields.length; paint(model); return true;
    } catch (error) {
      if (current(model) && request === model.pageRequest) {
        const message = `Não foi possível ler os campos: ${String(error)}`;
        if (!model.data) $("#pane-overview").textContent = message; else toast(message, "err");
      }
      return false;
    }
  }
  async function action(model, column, anchor, perform, { selected = null, analysisOwner = null } = {}) {
    if (!current(model)) return false;
    if (analysisOwner && !window.AnalysisContexts.isCurrent(analysisOwner)) { toast("O contexto mudou. Abra a ação novamente.", "info"); focus(anchor); return false; }
    const request = ++model.action;
    const owns = () => current(model) && request === model.action && (!analysisOwner || window.AnalysisContexts.isCurrent(analysisOwner));
    try {
      if (!owns()) throw Error("O contexto mudou. Abra a ação novamente.");
      const result = selected !== null ? { present: true, type: "string", text: selected, complete: true } : await model.reader.fieldText(model.ticket, column);
      if (!owns()) return false;
      await perform(result, owns); return true;
    } catch (error) { if (owns()) { toast(String(error.message || error), "err"); focus(anchor); } return false; }
  }
  function copy(model, column, anchor, selected = null) {
    return action(model, column, anchor, async (result, owns) => {
      if (!result.present) throw Error("O campo está ausente nesta evidência.");
      const focusAtWrite = document.activeElement; await navigator.clipboard.writeText(result.text);
      if (owns()) { toast("Valor preservado copiado.", "ok"); if (document.activeElement === focusAtWrite) focus(anchor); }
    }, { selected });
  }
  function filter(model, column, anchor, { selected = null, apply = false, op = null, analysisOwner = window.AnalysisContexts?.capture() } = {}) {
    return action(model, column, anchor, result => {
      const value = result.present ? result.text : null, operator = op || (value === null ? "empty" : selected !== null ? "contains" : column === "timestamp" ? "between" : "equals_exact");
      if (apply) addFilter({ column, op: operator, value: value ?? "", value2: operator === "between" ? value ?? "" : null });
      else openValueFilter(column, value, anchor, operator);
    }, { selected, analysisOwner });
  }
  function openValue(model, column, anchor) {
    return action(model, column, anchor, result => {
      const node = Object.freeze({ kind: "preserved_field_value", column }); values.set(node, { model, column });
      detailValueReturnFocus = anchor; detailValueNode = node; detailValueText = result.text ?? "";
      $("#detail-value-title").textContent = column; $("#detail-value-type").textContent = `${result.type || "ausente"} · evidência preservada`;
      $("#detail-value-content").textContent = result.present ? display(column, result.text) : "(campo ausente)";
      $("#detail-value-modal").hidden = false; $("#detail-value-close").focus();
    });
  }
  function menu(model, column, event, selected = null) {
    event.preventDefault(); event.stopPropagation(); if (!current(model)) return;
    const anchor = event.currentTarget || event.target, analysisOwner = window.AnalysisContexts?.capture();
    showCtxMenu(event.clientX, event.clientY, [
      { icon: "fa-filter", label: `Criar filtro: ${colLabel(column)}`, onClick: () => filter(model, column, anchor, { selected, analysisOwner }) },
      { icon: "fa-filter", label: selected !== null ? "Filtrar contendo a seleção" : "Filtrar valor exato preservado", onClick: () => filter(model, column, anchor, { selected, apply: true, analysisOwner }) },
      { icon: "fa-expand", label: "Ver valor completo preservado", onClick: () => openValue(model, column, anchor) },
      { icon: "fa-copy", label: selected !== null ? "Copiar seleção" : "Copiar valor preservado", onClick: () => copy(model, column, anchor, selected) },
    ]);
  }
  function selection(node) {
    const selected = window.getSelection?.();
    return selected && node.contains(selected.anchorNode) && node.contains(selected.focusNode) && selected.toString() !== "" ? selected.toString() : null;
  }
  function paint(model) {
    if (!current(model)) return; const data = model.data;
    $("#drawer-badges").replaceChildren(el("span", "badge code", "Evidência preservada"), el("span", "badge", "Valores originais"));
    for (const id of ["dr-prev", "dr-next"]) $("#" + id).hidden = true;
    $("#dr-copy").hidden = !data.envelope.complete;
    document.querySelectorAll("#drawer .dtab").forEach(tab => { tab.hidden = false; });
    const overview = $("#pane-overview"); overview.replaceChildren(el("p", "muted small", "Registro original preservado. Filtros, exclusões e campos derivados da análise atual não alteram este conteúdo."));
    const java = window.JavaTrace?.renderPreserved(data, { isCurrent: () => current(model), load: async () => model.javaResult || (model.javaResult = await model.reader.java(model.ticket)),
      cancel: () => window.Tasks?.cancelLatest("preserved:case_evidence_member_java_trace"), raw: () => switchDetailTab("raw"),
      original: (column, anchor) => openValue(model, column, anchor), scalarMenu: (event, column) => menu(model, column, event),
      menu: (event, items) => showCtxMenu(event.clientX, event.clientY, items), copied: () => toast("Valor copiado.", "ok"), copyFailed: () => toast("Não foi possível copiar.", "err") });
    if (java) overview.append(java);
    const fields = el("div", "detail-tree"); fields.setAttribute("aria-label", "Campos preservados");
    for (const field of data.fields) {
      const row = el("div", "detail-tree-row"), name = el("span", "detail-tree-name", colLabel(field.column)), value = el("button", "detail-tree-value mono");
      value.type = "button"; value.textContent = field.type === "missing" ? "(ausente)" : `${display(field.column, field.text) || "(vazio)"}${field.complete ? "" : "… (prévia limitada)"}`;
      value.setAttribute("aria-label", `Ver valor completo preservado de ${field.column}`); value.onclick = () => openValue(model, field.column, value);
      name.oncontextmenu = event => menu(model, field.column, event); value.oncontextmenu = event => menu(model, field.column, event, selection(value));
      row.append(name, value); fields.append(row);
    }
    overview.append(fields);
    const pager = el("div", "modal-actions"), status = el("span", "muted small", `${data.fields.length ? model.offsets[model.page] + 1 : 0}–${model.offsets[model.page] + data.fields.length} de ${data.fieldsTotal} campos`);
    for (const [label, index, disabled] of [["Campos anteriores", model.page - 1, model.page === 0], ["Próximos campos", model.page + 1, !data.nextCursor]]) {
      const button = el("button", "btn ghost small", label); button.type = "button"; button.disabled = disabled; button.onclick = () => page(model, index); pager.append(button);
    }
    pager.prepend(status); overview.append(pager);
    $("#pane-json").textContent = `${display("envelope", data.envelope.text)}${data.envelope.complete ? "" : "\n\n[Prévia limitada do JSON preservado]"}`;
    $("#pane-raw").textContent = `${display("raw", data.raw.text) || "(sem conteúdo bruto)"}${data.raw.complete ? "" : "\n\n[Prévia limitada do texto preservado]"}`;
    if (!model.rawMenu) model.previousRawMenu = $("#pane-raw").oncontextmenu;
    model.rawMenu = event => menu(model, "raw", event, selection($("#pane-raw")));
    $("#pane-raw").oncontextmenu = model.rawMenu;
  }
  async function copyEnvelope() {
    const model = active; if (!model || !current(model) || !model.data?.envelope.complete) return false;
    try { await navigator.clipboard.writeText(model.data.envelope.text); if (current(model)) toast("JSON preservado copiado.", "ok"); return true; }
    catch { if (current(model)) toast("Não foi possível copiar.", "err"); return false; }
  }
  document.addEventListener("keydown", event => {
    if (event.key !== "Escape" || !active || !current(active)) return;
    active.action++; window.Tasks?.cancelLatest("preserved:case_evidence_member_field_text");
  }, true);
  document.addEventListener("case-evidence-state", () => { if (active && !current(active)) closeDrawer(); });
  return { open, clear, isOpen: () => !!active && current(active), copyEnvelope, ownsValue: node => values.has(node),
    copyValue: (node, anchor) => { const value = values.get(node); return value && copy(value.model, value.column, anchor); },
    valueMenu: (node, event, selected) => { const value = values.get(node); if (value) menu(value.model, value.column, event, selected === "" ? null : selected); } };
})();
