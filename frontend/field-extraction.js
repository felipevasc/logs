/* Create a derived field from a capture group of an expression. It reuses the
 * assisted editor and the Case's own derived-field store: the field is a native
 * `captures` definition, usable afterwards in filters, SQL and every analysis, and
 * the original field is never changed. */
window.FieldExtraction = (() => {
  "use strict";
  const e = (t) => String(t ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay field-extraction-overlay"; overlay.id = "field-extraction-modal"; overlay.hidden = true;
  overlay.innerHTML = `<section class="modal fe-modal" role="dialog" aria-modal="true" aria-labelledby="fe-title"><div class="modal-head"><h3 id="fe-title">Extrair campo por expressão</h3><button type="button" class="icon-btn" data-act="close" aria-label="Fechar"><i class="fas fa-xmark"></i></button></div><div class="modal-body"><p class="muted small">Cria um campo no Caso com o valor de um grupo de captura. O campo de origem não é alterado.</p><div class="fe-row"><label>Extrair de<select data-f="source" aria-label="Campo de origem"></select></label><label>Nome do campo novo<input type="text" data-f="name" maxlength="128" spellcheck="false" placeholder="ex.: usuario" aria-label="Nome do campo novo"></label><label>Captura<select data-f="capture" aria-label="Captura a extrair"></select></label></div><div data-slot="bench"></div><p class="muted small">Usa a primeira correspondência de cada registro. Registros sem correspondência ficam sem o campo; falhas de limite do motor aparecem no diagnóstico do próprio registro, nunca como valor vazio.</p><p class="fe-status" role="status" aria-live="polite"></p><div class="modal-actions"><button type="button" class="btn ghost" data-act="cancel">Cancelar</button><button type="button" class="btn primary" data-act="save">Salvar campo no Caso</button></div></div></section>`;
  document.body.append(overlay);
  const q = (s) => overlay.querySelector(s), f = (n) => overlay.querySelector(`[data-f="${n}"]`);
  let bench = null, session = null;
  const status = (text, error = false) => { const n = q(".fe-status"); n.textContent = text; n.dataset.error = String(error); };
  const sources = () => [...new Set(["message", "raw", ...(state.columns || [])])];
  function captureOptions(selected) {
    const a = bench.editor.analysis, list = a.captures.filter((g) => !g.uncertain);
    f("capture").innerHTML = list.length ? list.map((g) => `<option value="${e(g.name ?? g.index)}">${g.index}${g.name ? ` · ${e(g.name)}` : ""}</option>`).join("") : '<option value="">Nenhum grupo de captura</option>';
    if (selected && list.some((g) => String(g.name ?? g.index) === String(selected))) f("capture").value = String(selected);
    else if (list.length) f("capture").value = String(list.find((g) => g.name)?.name ?? list[0].index);
  }
  function close() {
    if (!session) return;
    const anchor = session.anchor; session = null;
    bench?.destroy(); bench = null; overlay.hidden = true;
    window.LocalNative?.cancel("fe-sample");
    (anchor?.isConnected ? anchor : document.querySelector("#quick-search"))?.focus?.();
  }
  async function open({ pattern = null, capture = null, source = "message", event = null, edit = null, anchor = null, selection = "" } = {}) {
    const owner = window.AnalysisContexts?.capture();
    if (owner && !window.AnalysisContexts.isCurrent(owner)) { toast("O contexto mudou. Abra a ação novamente.", "info"); return false; }
    close();
    const def = edit?.advanced?.kind === "captures" ? edit.advanced : null;
    const base = def ? { ...def.pattern, field: edit.source } : pattern || {};
    const origin = edit?.source || source || base.field || "message";
    session = { owner, anchor, editName: edit?.name || null };
    f("source").innerHTML = sources().map((c) => `<option value="${e(c)}">${e(colLabel(c))}</option>`).join("");
    f("source").value = origin;
    f("name").value = edit?.name || ""; f("name").disabled = !!edit;
    status("");
    bench = window.RegexWorkbench.create(q('[data-slot="bench"]'), {
      mode: "extract", hideApply: true, applyNote: "O teste cobre só a amostra. A extração é gravada em “Salvar campo no Caso”.", sampleOpen: true,
      pattern: base.pattern || base.text || "", engine: base.engine || "pcre2", caseSensitive: base.caseSensitive, multiline: base.multiline, dotAll: base.dotAll, scope: "event",
      placeholder: "Exemplo: user=(?<usuario>\\w+)",
      sampleProvider: () => (state.currentDetailEv?.event_ref ? { id: state.currentDetailEv.id, event_ref: state.currentDetailEv.event_ref } : null),
      onChange: () => captureOptions(f("capture").value),
    });
    captureOptions(def?.capture ?? capture);
    overlay.hidden = false;
    // Seed the sample with the full text of the source field of the open event.
    const target = event || state.currentDetailEv;
    if (target?.event_ref) {
      try {
        const got = await window.LocalNative.call({ action: "sample_text", id: target.id, event_ref: target.event_ref, field: f("source").value === "_all" ? "raw" : f("source").value }, { latest: "fe-sample" });
        if (session) bench.setSample(got.text, { label: `Evento #${target.id} · ${f("source").value}`, truncated: got.truncated });
      } catch (error) { if (!window.LocalNative.superseded(error)) status(`A amostra do evento não pôde ser lida: ${String(error.message || error)}`, true); }
    }
    if (selection) bench.useSelection(selection, null, "capture");
    else (edit ? bench : f("name").value ? bench : { focus: () => f("name").focus() }).focus();
    return true;
  }
  async function save() {
    if (!session || !bench) return;
    const draft = bench.getDraft(), name = f("name").value.trim(), capture = f("capture").value, source = f("source").value, owner = session.owner;
    try {
      if (!name) throw Error("Informe o nome do campo novo.");
      if (!draft.pattern) throw Error("Escreva a expressão que extrai o valor.");
      if (!capture) throw Error("A expressão precisa de um grupo de captura para extrair o valor.");
      const existing = state.derivedFields?.find((d) => d.name === name);
      if (!session.editName && (state.columns.includes(name) && !existing || existing && existing.advanced?.kind !== "captures")) throw Error("Já existe um campo com esse nome. Escolha outro nome.");
      status("Validando a expressão…");
      const checked = await bench.runPreview();
      if (!checked?.ok) throw Error(checked?.error?.message || "A expressão não é válida. Corrija o erro indicado.");
      if (!bench.editor.analysis.captures.length && !checked.groups?.some((g) => g.names?.includes(capture))) throw Error("A captura escolhida não existe na expressão.");
      status("Salvando no Caso…");
      await window.LocalNative.call({ action: "save_field", name, source, definition: { kind: "captures", pattern: { engine: draft.engine, text: draft.pattern, caseSensitive: draft.caseSensitive, multiline: draft.multiline, dotAll: draft.dotAll }, capture } }, { latest: "local-save" });
      if (owner) window.AnalysisContexts.assertOwner(owner, { revisions: false });
      if (!await loadDerivedFields(window.AnalysisContexts?.capture(owner?.caseId))) throw Error("O campo foi salvo, mas a lista de campos não pôde ser atualizada. Reabra a análise.");
      if (!state.columns.includes(name)) { state.columns.push(name); fillColumnControls(); }
      state.datasetProfiles = null;
      close();
      toast(`Campo "${name}" salvo no Caso. Filtros e SQL já podem usá-lo.`, "ok");
      void refresh();
    } catch (error) { status(String(error.message || error), true); }
  }
  q('[data-act="close"]').onclick = q('[data-act="cancel"]').onclick = close;
  q('[data-act="save"]').onclick = () => void save();
  f("capture").onchange = () => {};
  overlay.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && !document.querySelector(".rxw-pop")) { ev.preventDefault(); ev.stopPropagation(); close(); }
  });
  document.addEventListener("analysis-context-change", close);
  return { open, close };
})();
