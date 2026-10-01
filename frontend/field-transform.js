/* A Case-owned local transformation editor. Preview is always an explicit action. */
window.FieldTransforms = (() => {
  "use strict";
  const STEPS = [
    ["base64_decode", "Base64 → texto"], ["base64_url_decode", "Base64 URL → texto"],
    ["base64_encode", "Texto → Base64"], ["base64_url_encode", "Texto → Base64 URL"],
    ["url_decode", "Decodificar URL (%)"], ["url_encode", "Codificar URL (%)"], ["form_decode", "Decodificar formulário (+ e %)"],
    ["hex_to_text", "Hexadecimal → texto"], ["text_to_hex", "Texto → hexadecimal"],
    ["parse_json", "Interpretar JSON"], ["parse_xml", "Interpretar XML"], ["parse_query", "Interpretar query string"],
    ["jwt_payload", "Ler conteúdo do JWT"],
  ];
  const INPUT_LIMIT = 256 * 1024, PREVIEW_LIMIT = 8192;
  const overlay = el("div", "modal-overlay field-transform-overlay"); overlay.id = "field-transform-modal"; overlay.hidden = true;
  overlay.innerHTML = `<section class="modal field-transform-modal" role="dialog" aria-modal="true" aria-labelledby="ft-title">
    <div class="modal-head"><h3 id="ft-title">Transformar campo</h3><button type="button" class="icon-btn" id="ft-close" aria-label="Fechar transformações"><i class="fas fa-xmark"></i></button></div>
    <div class="modal-body"><p id="ft-source" class="muted small"></p>
      <div class="fld"><label for="ft-name">Nome do campo resultante</label><input id="ft-name" type="text" spellcheck="false" maxlength="4096"></div>
      <p class="muted small">As transformações criam um campo no Caso. O valor de origem é preservado e todo o processamento acontece localmente no LogInsight.</p>
      <div class="ft-inputs"><div class="fld"><label>Valor de origem</label><pre id="ft-original" class="code-pane"></pre></div>
      <div class="fld"><label for="ft-sample">Valor de exemplo para a prévia</label><textarea id="ft-sample" rows="4" spellcheck="false" maxlength="262144"></textarea><small id="ft-sample-hint" class="muted"></small></div></div>
      <div class="fld"><label>Etapas, na ordem de execução</label><ol id="ft-steps" class="ft-steps"></ol>
      <div class="ft-add-row"><select id="ft-step-choice" aria-label="Transformação para adicionar"></select><button id="ft-add-step" type="button" class="btn ghost small">Adicionar etapa</button><small id="ft-step-count" class="muted"></small></div></div>
      <p id="ft-rules-note" class="notice" hidden></p><p id="ft-jwt-warning" class="notice" role="note" hidden>JWT: apenas o conteúdo é decodificado. A assinatura não foi verificada.</p>
      <div class="ft-preview-head"><button id="ft-preview" type="button" class="btn ghost">Prévia</button><span id="ft-result-type" class="muted small"></span></div>
      <pre id="ft-output" class="code-pane" aria-label="Resultado da prévia"></pre><p id="ft-notices" class="muted small"></p>
      <p id="ft-status" class="small" role="status" aria-live="polite"></p>
      <p class="muted small">Até 8 etapas. Valores incompatíveis são sinalizados por registro; os demais registros continuam disponíveis.</p>
      <div class="modal-actions"><button id="ft-cancel" type="button" class="btn ghost">Cancelar</button><button id="ft-save" type="button" class="btn primary">Salvar campo</button></div>
    </div></section>`;
  document.body.append(overlay);
  const node = id => overlay.querySelector(`#ft-${id}`), clone = value => structuredClone(value);
  for (const [value, label] of STEPS) { const option = el("option", "", label); option.value = value; node("step-choice").append(option); }
  let session = null, serial = 0;
  const prefix = (input, count) => {
    let end = Math.min(input.length, Math.max(0, count));
    if (end < input.length && end && /[\uD800-\uDBFF]/.test(input[end - 1])) end--;
    return input.slice(0, end);
  };
  const utf8Size = (value, ceiling = INPUT_LIMIT) => {
    let bytes = 0;
    for (const char of value) {
      const point = char.codePointAt(0);
      bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
      if (bytes > ceiling) break;
    }
    return bytes;
  };
  // Visit a bounded prefix for display or editable sample preparation.
  // These limits bound visited values/output, not the received Event or JS heap.
  // JS may enumerate every object key before the first loop iteration; strict
  // wide-object work limits also require bounded native property projection.
  function serializePrefix(value, { limit, byteLimit = Infinity, maxDepth = 16, maxNodes = 512, redact = false }) {
    const ancestors = new WeakSet();
    let output = "", reason = "", nodes = 0, bytes = 0;
    const append = input => {
      if (reason) return;
      const remaining = limit - output.length;
      const selected = prefix(input, remaining), size = Number.isFinite(byteLimit) ? utf8Size(selected, byteLimit - bytes) : 0;
      if (bytes + size > byteLimit) { reason = "tamanho"; return; }
      output += selected; bytes += size;
      if (input.length > remaining) reason = "caracteres";
    };
    function string(input, quoted, mask = false) {
      // Redaction also sees only this display prefix; original values stay intact.
      const selected = prefix(input, limit - output.length);
      // A clipped token/PEM may not match the full redactor. Omit that scalar
      // rather than reveal a partial secret that its complete value would mask.
      const clipped = selected.length < input.length;
      const safe = mask && clipped ? "[texto extenso omitido]" : mask && window.EvidenceUI ? window.EvidenceUI.redact(selected) : selected;
      if (quoted) append('"');
      for (const char of safe) {
        if (reason) break;
        append(quoted ? JSON.stringify(char).slice(1, -1) : char);
      }
      if (!reason && selected.length < input.length) reason = "caracteres";
      if (quoted) append('"');
    }
    function visit(input, depth, root = false) {
      if (reason) return;
      if (++nodes > maxNodes) { reason = "itens"; return; }
      if (typeof input === "string") { string(input, !root, redact); return; }
      if (input == null) { append("null"); return; }
      if (typeof input === "number" || typeof input === "boolean") { append(JSON.stringify(input)); return; }
      if (typeof input !== "object") { reason = "valor não JSON"; return; }
      if (depth >= maxDepth) { reason = "profundidade"; return; }
      if (ancestors.has(input)) { reason = "referência circular"; return; }
      ancestors.add(input);
      const array = Array.isArray(input), indent = "  ".repeat(depth + 1);
      append(array ? "[" : "{"); let first = true;
      const member = (key, descriptor) => {
        if (!first) append(","); append(`\n${indent}`); first = false;
        if (!array) { string(key, true); append(": "); }
        if (reason) return;
        if (descriptor && !Object.hasOwn(descriptor, "value")) { reason = "propriedade calculada"; return; }
        const hidden = redact && !array && window.EvidenceUI?.isSensitiveKey?.(key);
        visit(hidden ? "[oculto]" : descriptor?.value ?? null, depth + 1);
      };
      if (array) {
        for (let index = 0; index < input.length && !reason; index++) member(String(index), Object.getOwnPropertyDescriptor(input, index));
      } else {
        // Avoid an explicit full key/value copy. The engine's key enumeration
        // can still scale with the input object's width before this loop stops.
        for (const key in input) {
          if (reason) break;
          const descriptor = Object.getOwnPropertyDescriptor(input, key);
          if (descriptor?.enumerable) member(key, descriptor);
        }
      }
      if (!first) append(`\n${"  ".repeat(depth)}`);
      append(array ? "]" : "}"); ancestors.delete(input);
    }
    visit(value, 0, true);
    return { output, reason };
  }
  function bounded(value, limit = PREVIEW_LIMIT, { redact = false } = {}) {
    limit = Number.isFinite(limit) ? Math.max(64, Math.min(PREVIEW_LIMIT, Math.floor(limit))) : PREVIEW_LIMIT;
    const { output, reason } = serializePrefix(value, { limit, redact });
    if (!reason) return output;
    const marker = `\n… [prévia truncada: ${reason}]`;
    return prefix(output, limit - marker.length) + marker;
  }
  function rawValue(event, field, copy = true) {
    if (!event) return null;
    if (field === "timestamp") return event.timestamp == null ? null : new Date(event.timestamp).toISOString().replace(/\.000Z$/, "+00:00").replace(/Z$/, "+00:00");
    if (field === "id") return String(event.id);
    const canonical = ["event_ref", "source", "level", "code", "name", "description", "message", "raw"].includes(field);
    if (!canonical && Object.hasOwn(event.fields || {}, field)) return copy ? clone(event.fields[field]) : event.fields[field];
    if (Object.hasOwn(event, field)) return copy ? clone(event[field]) : event[field];
    return null;
  }
  const current = draft => session === draft && !overlay.hidden;
  function assertOwner(draft, revisions = true) { if (draft.owner) window.AnalysisContexts.assertOwner(draft.owner, { revisions }); }
  function status(message, failed = false) { node("status").textContent = message; node("status").classList.toggle("ft-error", failed); }
  function focusBack(anchor) {
    const target = [anchor, $("#btn-colpicker"), $("#quick-search")].find(item => item?.isConnected && !item.disabled && !item.hidden && item.matches?.("button,input,select,textarea,[tabindex],a[href]") && !item.closest?.("[hidden],[inert]"));
    target?.focus();
  }
  function close() {
    const anchor = session?.anchor; serial++; session = null; overlay.hidden = true;
    window.Tasks?.cancelLatest("field-transform-preview"); focusBack(anchor);
  }
  function dirty({ configuration = true } = {}) {
    if (!session) return;
    session.version++; serial++; if (configuration) session.saved = false;
    node("preview").disabled = !!session.busy;
    node("output").textContent = "Clique em Prévia para avaliar estas etapas."; node("result-type").textContent = ""; node("notices").textContent = "";
    node("jwt-warning").hidden = !session.steps.includes("jwt_payload"); status("");
    window.Tasks?.cancelLatest("field-transform-preview");
  }
  function renderSteps() {
    node("steps").replaceChildren();
    if (!session.steps.length) node("steps").append(el("li", "muted small", "Nenhuma transformação adicionada."));
    session.steps.forEach((step, index) => {
      const row = el("li", "ft-step"), label = el("span", "", STEPS.find(([key]) => key === step)?.[1] || step);
      row.append(label);
      for (const [action, caption, move] of [["up", "Mover etapa para cima", -1], ["down", "Mover etapa para baixo", 1], ["remove", "Remover etapa", 0]]) {
        const button = el("button", "btn ghost small", action === "up" ? "↑" : action === "down" ? "↓" : "Remover"); button.type = "button";
        button.setAttribute("aria-label", `${caption}: ${label.textContent}`);
        button.disabled = session.busy || action === "up" && index === 0 || action === "down" && index === session.steps.length - 1;
        button.onclick = () => {
          if (session?.busy) return;
          if (!move) session.steps.splice(index, 1);
          else [session.steps[index], session.steps[index + move]] = [session.steps[index + move], session.steps[index]];
          dirty(); renderSteps();
        };
        row.append(button);
      }
      node("steps").append(row);
    });
    node("step-count").textContent = `${session.steps.length} / 8`;
    node("add-step").disabled = session.busy || session.steps.length >= 8;
    node("jwt-warning").hidden = !session.steps.includes("jwt_payload");
  }
  function busy(value) {
    if (!session) return;
    session.busy = value;
    for (const id of ["sample", "step-choice", "preview", "save"]) node(id).disabled = value;
    node("name").disabled = value || !!session.editName; renderSteps();
  }
  function open(field, { event = null, anchor = null, owner = window.AnalysisContexts?.capture() } = {}) {
    if (owner) { try { window.AnalysisContexts.assertOwner(owner); } catch { toast("O contexto mudou. Abra o campo novamente.", "info"); return false; } }
    const definition = state.derivedFields?.find(item => item.name === field && !item.lookup), source = definition?.source || field;
    const sampleEvent = event || state.rows.find(row => rawValue(row, source, false) != null) || state.currentDetailEv;
    const originalValue = rawValue(sampleEvent, source, false), rules = clone(definition?.rules || []);
    const input = rules.length ? null : serializePrefix(originalValue, { limit: INPUT_LIMIT, byteLimit: INPUT_LIMIT, maxDepth: 64, maxNodes: 65536 });
    const originalText = input && !input.reason ? input.output : null;
    // Only a complete, bounded sample needs an immutable copy for native preview.
    const original = originalText == null ? originalValue : clone(originalValue);
    serial++; window.Tasks?.cancelLatest("field-transform-preview");
    session = { owner, source, original, originalText, rules, editName: definition?.name || null, steps: [...(definition?.steps || [])], anchor, version: 0, busy: false, saved: false };
    node("source").textContent = `Origem: ${colLabel(source)} · Caso: ${activeCase()?.name || "atual"}`;
    node("name").value = definition?.name || `${field.replace(/^@/, "")}_transformado`;
    node("original").textContent = bounded(original, 4096);
    const oversized = originalText == null;
    node("sample").value = oversized ? "" : originalText;
    node("sample-hint").textContent = rules.length ? "Informe um exemplo já extraído pelas regras existentes. A prévia avalia somente as transformações."
      : oversized ? "A origem excede 256 KiB ou o limite de estrutura do exemplo. Use um exemplo menor para a prévia." : "Edite o exemplo se necessário. Isso não altera o registro de origem.";
    node("rules-note").hidden = !rules.length;
    node("rules-note").textContent = `${rules.length} regra(s) de extração preservada(s). Elas são executadas antes das transformações.`;
    node("output").textContent = "Clique em Prévia para avaliar estas etapas."; node("result-type").textContent = ""; node("notices").textContent = "";
    node("save").textContent = "Salvar campo"; overlay.hidden = false; status(""); busy(false);
    (definition ? node("step-choice") : node("name")).focus(); return true;
  }
  function validateDraft(draft) {
    const name = node("name").value.trim();
    if (!name) throw Error("Informe o nome do campo resultante.");
    if (!draft.steps.length && !draft.rules.length) throw Error("Adicione ao menos uma transformação.");
    if (draft.steps.length > 8 || draft.steps.some(step => !STEPS.some(([key]) => key === step))) throw Error("Sequência de transformações inválida.");
    if (!draft.editName && (state.columns.includes(name) || state.derivedFields.some(field => field.name === name))) throw Error("Já existe um campo com esse nome. Escolha outro nome ou abra as transformações do campo existente.");
    return name;
  }
  async function preview() {
    const draft = session; if (!draft || draft.busy) return;
    const request = ++serial, version = draft.version;
    try {
      assertOwner(draft);
      const sample = node("sample").value;
      if (draft.rules.length && !sample) throw Error("Informe um exemplo já extraído pelas regras existentes.");
      if (utf8Size(sample) > INPUT_LIMIT) throw Error("O exemplo excede 256 KiB. Use um valor menor.");
      const value = !draft.rules.length && sample === draft.originalText ? clone(draft.original) : sample;
      const steps = [...draft.steps]; node("preview").disabled = true; status("Calculando prévia local…");
      const result = await api("preview_field_transform", { value, steps }, { silent: true, latest: "field-transform-preview" });
      if (!current(draft) || request !== serial || version !== draft.version) return;
      assertOwner(draft);
      if (!result || !Object.hasOwn(result, "value") || !Array.isArray(result.notices)) throw Error("Prévia inválida.");
      node("output").textContent = bounded(result.value);
      node("result-type").textContent = result.value == null ? "Nulo" : Array.isArray(result.value) ? "Lista" : ({ object: "Objeto", string: "Texto", number: "Número", boolean: "Booleano" })[typeof result.value] || "Valor";
      node("jwt-warning").hidden = !steps.includes("jwt_payload") && !result.notices.includes("jwt_signature_not_verified");
      node("notices").textContent = result.notices.filter(notice => notice !== "jwt_signature_not_verified").join(" · ");
      status("Prévia atualizada. O registro original foi preservado.");
    } catch (error) { if (current(draft) && request === serial) status(String(error), true); }
    finally { if (current(draft) && request === serial) node("preview").disabled = false; }
  }
  async function refreshFields(draft) {
    if (!await loadDerivedFields(draft.owner)) throw Error("Configuração salva. Não foi possível atualizar os campos; tente novamente.");
    assertOwner(draft);
    const scope = workspaceScope(), profiles = await api("profile_fields", { filters: [], ...(scope === "case" ? { caseEvents: caseEvents("analysis") } : {}) }, { silent: true, analysisOwner: draft.owner });
    assertOwner(draft);
    const names = [...new Set([...(profiles || []).map(field => field.name), ...state.derivedFields.map(field => field.name)])];
    if (state.columns.includes("comentario")) names.push("comentario");
    if (names.length) state.columns = names;
    state.visibleCols = state.visibleCols.filter(field => state.columns.includes(field));
    if (scope === "dataset") state.datasetProfiles = profiles;
    else { state.caseTreeProfiles[draft.owner.caseId] = { sig: JSON.stringify([caseSig(), draft.owner.instance, draft.owner.identity]), profiles }; state.caseProfilesLoading = false; }
    fillColumnControls(); renderExploreTree();
    if (await refresh() === false) throw Error("Configuração salva. A visualização ainda precisa ser atualizada.");
  }
  async function save() {
    const draft = session; if (!draft || draft.busy) return;
    let committed = draft.saved;
    try {
      assertOwner(draft); const name = validateDraft(draft); serial++; window.Tasks?.cancelLatest("field-transform-preview"); busy(true); status(draft.saved ? "Atualizando campos…" : "Salvando no Caso…");
      if (!draft.saved) {
        await api("save_derived_field", { name, source: draft.source, rules: clone(draft.rules), steps: [...draft.steps] }, { silent: true, analysisOwner: draft.owner });
        committed = true;
        if (!current(draft)) return;
        assertOwner(draft, false); draft.owner = window.AnalysisContexts?.capture(draft.owner?.caseId); draft.saved = true; draft.editName = name;
      }
      await refreshFields(draft);
      if (!current(draft)) return;
      assertOwner(draft); close(); toast(`Campo "${name}" salvo no Caso.`, "ok");
    } catch (error) {
      if (current(draft)) { status(`${committed ? "A configuração já foi salva. " : ""}${String(error)}`, true); if (draft.saved) node("save").textContent = "Atualizar campos"; }
    } finally { if (current(draft)) busy(false); }
  }
  node("name").oninput = () => dirty(); node("sample").oninput = () => dirty({ configuration: false });
  node("add-step").onclick = () => { if (!session || session.busy || session.steps.length >= 8) return; session.steps.push(node("step-choice").value); dirty(); renderSteps(); };
  node("preview").onclick = preview; node("save").onclick = save; node("close").onclick = node("cancel").onclick = close;
  overlay.onclick = event => { if (event.target === overlay) close(); };
  overlay.addEventListener("keydown", event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (event.key !== "Tab") return;
    const targets = [...overlay.querySelectorAll("button,input,select,textarea")].filter(item => !item.disabled && !item.hidden && !item.closest("[hidden]"));
    const first = targets[0], last = targets.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  });
  function menuItem(field, { event = null, anchor = null } = {}) {
    const owner = window.AnalysisContexts?.capture();
    return { icon: "fa-wand-magic-sparkles", label: "Transformar campo", onClick: () => open(field, { event, anchor, owner }) };
  }
  function renderDiagnostics(event) {
    const diagnostics = Array.isArray(event?.derived_diagnostics) ? event.derived_diagnostics : [];
    if (!diagnostics.length) return null;
    const block = el("section", "ft-diagnostics"), owner = window.AnalysisContexts?.capture();
    block.setAttribute("aria-label", "Diagnósticos de transformações deste registro");
    block.append(el("strong", "", "Transformações neste registro"));
    for (const diagnostic of diagnostics.slice(0, 32)) {
      const row = el("div", `ft-diagnostic ${diagnostic.warning ? "is-warning" : "is-error"}`);
      if (diagnostic.field) row.append(el("b", "", bounded(String(diagnostic.field), 128)));
      const message = diagnostic.code === "jwt_signature_not_verified"
        ? "Conteúdo JWT decodificado. A assinatura e as declarações não foram verificadas."
        : diagnostic.message || (diagnostic.warning ? "Transformação com aviso." : "A transformação não foi aplicada a este registro.");
      row.append(el("span", "", bounded(message, 512)));
      const definition = state.derivedFields?.find(field => field.name === diagnostic.field);
      if (definition) {
        const edit = el("button", "btn ghost small", definition.lookup ? "Revisar referência" : "Revisar transformação"); edit.type = "button";
        edit.onclick = () => definition.lookup ? window.CaseReferences?.openLookup(definition, { anchor: edit, owner }) : open(diagnostic.field, { event, anchor: edit, owner }); row.append(edit);
      }
      block.append(row);
    }
    if (diagnostics.length > 32) block.append(el("small", "muted", "Outros diagnósticos estão disponíveis na visualização JSON."));
    block.append(el("small", "muted", "O registro de origem permanece disponível.")); return block;
  }
  return { open, close, preview, save, menuItem, bounded, rawValue, renderDiagnostics };
})();
