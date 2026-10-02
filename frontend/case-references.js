/* Immutable reference imports and exact lookup fields belong to one captured Case. */
window.CaseReferences = (() => {
  "use strict";
  const copy = value => structuredClone(value), count = value => Number.isSafeInteger(value) && value >= 0;
  const overlay = el("div", "modal-overlay"); overlay.id = "case-references-modal"; overlay.hidden = true;
  overlay.innerHTML = `<section class="modal case-references-modal" role="dialog" aria-modal="true" aria-labelledby="rf-title">
    <div class="modal-head"><h3 id="rf-title">Referências deste Caso</h3><button id="rf-close" class="icon-btn" type="button" aria-label="Fechar referências"><i class="fas fa-xmark"></i></button></div>
    <div class="modal-body"><p id="rf-owner" class="muted small"></p>
      <div id="rf-manager"><div class="rf-tools"><button id="rf-choose" class="btn primary" type="button">Importar JSONL</button><button id="rf-reload" class="btn ghost" type="button">Atualizar referências</button></div>
        <p class="muted small">JSONL: um objeto por linha, com as mesmas colunas. Até 8 MiB e 100.000 registros. A referência importada fica vinculada somente a este Caso.</p>
        <div id="rf-import-pane" class="rf-import-pane" hidden><p id="rf-file" class="small"></p><p id="rf-inspection" class="muted small"></p>
          <div class="fld"><label for="rf-import-name">Nome da referência</label><input id="rf-import-name" maxlength="4096" spellcheck="false"></div>
          <div class="fld"><label>Colunas da chave, na ordem</label><ol id="rf-keys"></ol><div class="rf-tools"><select id="rf-key-choice" aria-label="Coluna para adicionar à chave"></select><button id="rf-add-key" class="btn ghost small" type="button">Adicionar chave</button></div></div>
          <p class="muted small">Escolha de 1 a 16 colunas. Chaves repetidas são rejeitadas; nenhum registro é escolhido arbitrariamente.</p><button id="rf-import" class="btn primary" type="button" disabled>Importar referência</button>
        </div><div id="rf-list" class="rf-list"></div>
      </div>
      <div id="rf-lookup-pane" hidden><p class="muted small">O novo campo recebe um único valor da referência. Os tipos são preservados: o número 1 é diferente do texto "1". Os registros originais continuam intactos.</p>
        <div class="fld"><label for="rf-field-name">Nome do campo resultante</label><input id="rf-field-name" maxlength="4096" spellcheck="false"></div>
        <div class="fld"><label for="rf-reference">Referência deste Caso</label><select id="rf-reference"></select></div><p id="rf-availability" class="muted small"></p>
        <div id="rf-mappings" class="rf-mappings"></div><div class="fld"><label for="rf-value-column">Coluna da referência que fornece o valor</label><select id="rf-value-column"></select></div>
        <p class="muted small">Mapeie cada chave explicitamente com texto, número ou booleano não nulos. Campos nativos usam texto, incluindo id e timestamp (ISO 8601); campos JSON mantêm seu tipo original. Um valor JSON null encontrado continua sendo null, diferente de uma chave sem correspondência. Esta etapa não multiplica linhas.</p>
      </div>
    </div><div class="rf-footer">
      <p id="rf-status" class="small" role="status" aria-live="polite" tabindex="-1"></p>
      <p id="rf-close-note" class="muted small" hidden>Fechar não interrompe nem desfaz a operação em andamento.</p>
      <div class="modal-actions"><button id="rf-delete-lookup" class="btn ghost" type="button" hidden>Excluir campo</button><div class="rf-confirm-actions"><button id="rf-cancel" class="btn ghost" type="button">Cancelar</button><button id="rf-save-lookup" class="btn primary" type="button" hidden>Salvar campo</button></div></div>
    </div></section>`;
  document.body.append(overlay);
  const node = name => overlay.querySelector(`#rf-${name}`);
  let session = null, serial = 0;
  const current = (draft, request = serial) => session === draft && !overlay.hidden && request === serial;
  const ownerLabel = () => activeCase()?.name || "Caso atual";
  function assertOwner(draft, revisions = true) { window.AnalysisContexts.assertOwner(draft.owner, { revisions }); }
  function status(text, failed = false) { node("status").textContent = text; node("status").setAttribute("tabindex", text ? "0" : "-1"); node("status").classList.toggle("rf-error", failed); }
  function validDescriptor(value) {
    return value?.schemaVersion === 1 && typeof value.id === "string" && !!value.id && typeof value.name === "string"
      && value.format === "jsonl" && /^[a-f0-9]{64}$/i.test(value.contentSha256) && Array.isArray(value.columns) && value.columns.length > 0
      && value.columns.length <= 1024 && value.columns.every(column => typeof column === "string" && column.trim()) && new Set(value.columns).size === value.columns.length
      && Array.isArray(value.keyColumns) && value.keyColumns.length > 0 && value.keyColumns.length <= 16 && new Set(value.keyColumns).size === value.keyColumns.length
      && value.keyColumns.every(column => value.columns.includes(column)) && value.duplicatePolicy === "reject";
  }
  function validInspection(value) {
    return value?.format === "jsonl" && typeof value.name === "string" && /^[a-f0-9]{64}$/i.test(value.contentSha256)
      && count(value.rowCount) && value.rowCount > 0 && value.rowCount <= 100000 && count(value.sourceBytes) && value.sourceBytes <= 8 * 1024 * 1024
      && Array.isArray(value.columns) && value.columns.length > 0 && value.columns.length <= 1024
      && value.columns.every(column => typeof column === "string" && !!column.trim()) && new Set(value.columns).size === value.columns.length;
  }
  function options(select, values, selected, placeholder = "Escolha…") {
    select.replaceChildren(); const empty = el("option", "", placeholder); empty.value = ""; select.append(empty);
    for (const value of values) { const label = value.label ?? value, option = el("option", "", trunc(label, 160)); option.value = value.value ?? value; option.title = trunc(label, 512); select.append(option); }
    select.value = selected || "";
  }
  function busy(draft, value) {
    draft.busy = value; if (!current(draft)) return;
    for (const id of ["choose", "reload", "import-name", "key-choice", "add-key", "import", "reference", "value-column", "save-lookup", "delete-lookup"]) node(id).disabled = value;
    node("field-name").disabled = value || !!draft.editName;
    for (const input of node("mappings").querySelectorAll("select")) input.disabled = value;
    for (const button of node("keys").querySelectorAll("button")) if (value || draft.needsRefresh) button.disabled = true;
    if (!value) {
      if (!draft.needsRefresh) renderKeys(draft);
      node("choose").disabled = node("import-name").disabled = node("key-choice").disabled = node("add-key").disabled = !!draft.needsRefresh;
      node("import").disabled = !draft.inspection || !draft.keys?.length || !!draft.needsRefresh;
      node("save-lookup").disabled = !!draft.deleted || !draft.entries?.some(entry => entry.descriptor.id === node("reference").value && entry.available);
    }
    node("close-note").hidden = !(value && draft.mutating);
    node("cancel").textContent = value && draft.mutating ? "Fechar" : "Cancelar";
  }
  function close() {
    const anchor = session?.anchor; session = null; serial++; overlay.hidden = true;
    for (const key of ["reference-list", "reference-inspect"]) window.Tasks?.cancelLatest(key);
    const target = [anchor, $("#btn-case-menu"), $("#btn-colpicker")].find(item => item?.isConnected && !item.disabled && !item.hidden && item.matches?.("button,input,select,textarea,[tabindex],a[href]") && !item.closest?.("[hidden],[inert]"));
    target?.focus?.();
  }
  function start(mode, owner, anchor) {
    try { window.AnalysisContexts.assertOwner(owner); } catch { toast("O Caso mudou. Reabra esta ação.", "info"); return null; }
    for (const key of ["reference-list", "reference-inspect"]) window.Tasks?.cancelLatest(key);
    serial++; session = { mode, owner, anchor, busy: false, entries: [], keys: [], mappings: Object.create(null), inspection: null, imported: null, needsRefresh: false };
    node("manager").hidden = mode !== "manager"; node("lookup-pane").hidden = mode !== "lookup"; node("import-pane").hidden = true;
    node("save-lookup").hidden = mode !== "lookup"; node("delete-lookup").hidden = true;
    node("title").textContent = mode === "manager" ? "Referências deste Caso" : "Campo por referência"; node("owner").textContent = `Caso: ${ownerLabel()}`;
    node("list").replaceChildren(); status(""); overlay.hidden = false; busy(session, false); return session;
  }
  async function list(draft) {
    assertOwner(draft);
    const result = await api("reference_list", { analysisContext: draft.owner.identity }, { silent: true, latest: "reference-list", analysisOwner: draft.owner });
    assertOwner(draft);
    if (window.AnalysisContexts.signature(result?.analysisContext) !== window.AnalysisContexts.signature(draft.owner.identity) || !Array.isArray(result.references)
      || !result.references.every(entry => validDescriptor(entry.descriptor) && typeof entry.available === "boolean"
        && (entry.rowCount == null || count(entry.rowCount)) && (entry.sourceBytes == null || count(entry.sourceBytes)) && (entry.reason == null || typeof entry.reason === "string"))) throw Error("Lista de referências inválida.");
    draft.entries = result.references; return result.references;
  }
  function dependencies(id) { return (window.AnalysisContexts.context()?.config?.derivedFields || state.derivedFields || []).filter(field => field.lookup?.referenceId === id).map(field => field.name); }
  const interpretation = descriptor => descriptor.interpretationVersion == null || descriptor.interpretationVersion === 1 ? "legada v1"
    : descriptor.interpretationVersion === 2 ? "exata v2" : `não suportada (${String(descriptor.interpretationVersion)})`;
  function renderList(draft) {
    node("list").replaceChildren();
    const diagnostics = window.AnalysisContexts.context()?.migrationDiagnostics || [];
    if (diagnostics.length) {
      const notices = el("div", "rf-diagnostics"); notices.setAttribute("role", "status"); notices.append(el("strong", "", "Avisos da interpretação preservada"));
      for (const diagnostic of diagnostics.slice(0, 20)) notices.append(el("p", "muted small", trunc(diagnostic.message || diagnostic.code || "Aviso da investigação importada", 600)));
      if (diagnostics.length > 20) notices.append(el("p", "muted small", `${fmtNum(diagnostics.length - 20)} avisos adicionais na configuração preservada.`));
      node("list").append(notices);
    }
    for (const entry of draft.entries) {
      const descriptor = entry.descriptor, card = el("article", "rf-reference"), name = el("strong", "", trunc(descriptor.name, 160)); name.title = trunc(descriptor.name, 512);
      card.append(name, el("span", entry.available ? "muted small" : "rf-error small", entry.available
        ? `Disponível · ${entry.rowCount == null ? "contagem indisponível" : `${fmtNum(entry.rowCount)} registros`} · JSONL` : `Indisponível: ${entry.reason || "não foi possível abrir o conteúdo"}`));
      card.append(el("span", "muted small", `Chaves: ${descriptor.keyColumns.join(" → ")}`));
      const details = el("details"), detailTitle = el("summary", "", "Detalhes da referência");
      details.append(detailTitle, el("pre", "rf-details", `SHA-256: ${descriptor.contentSha256}\nColunas (${Math.min(32, descriptor.columns.length)} de ${descriptor.columns.length}): ${descriptor.columns.slice(0, 32).map(column => trunc(column, 80)).join(", ")}\nVersão do descritor: ${descriptor.schemaVersion}\nInterpretação numérica: ${interpretation(descriptor)}`)); card.append(details);
      const actions = el("div", "rf-tools"), create = el("button", "btn ghost small", "Criar campo por referência"), remove = el("button", "btn ghost small", "Remover referência do Caso");
      create.type = remove.type = "button"; create.disabled = !entry.available;
      create.onclick = () => { if (!draft.busy) void openLookup(null, { referenceId: descriptor.id, owner: draft.owner, anchor: create }); };
      remove.onclick = () => { if (!draft.busy) void removeReference(descriptor.id); };
      const used = dependencies(descriptor.id); remove.disabled = !!used.length; remove.title = used.length ? `Usada por: ${used.join(", ")}` : "Remove o vínculo deste Caso; o arquivo original é preservado.";
      actions.append(create, remove); card.append(actions);
      if (used.length) card.append(el("small", "muted", `Campos dependentes: ${used.join(", ")}`)); node("list").append(card);
    }
    if (!draft.entries.length) node("list").append(el("p", "muted small", "Nenhuma referência importada neste Caso."));
  }
  async function openManager({ owner = window.AnalysisContexts?.capture(), anchor = $("#btn-case-menu") } = {}) {
    const draft = start("manager", owner, anchor); if (!draft) return;
    await refreshList(); if (current(draft)) node("choose").focus();
  }
  async function refreshList({ reconcile = false } = {}) {
    const draft = session; if (!draft || draft.busy) return;
    const request = ++serial; busy(draft, true); status("Carregando referências do Caso…");
    try {
      if (reconcile) { assertOwner(draft, false); await window.AnalysisContexts.refresh(draft.owner.caseId, { owner: draft.owner }); draft.owner = window.AnalysisContexts.capture(draft.owner.caseId); }
      else draft.owner = await window.AnalysisContexts.prepare(draft.owner, { metadata: true });
      if (!current(draft, request)) return;
      if (!draft.owner.identity) throw Error("Salve o Caso antes de gerenciar referências.");
      await list(draft); if (!current(draft, request)) return;
      if (draft.needsRefresh) {
        const prior = draft.attempt;
        const found = prior && draft.entries.find(entry => !prior.ids.includes(entry.descriptor.id) && entry.descriptor.contentSha256 === prior.inspection.contentSha256
          && entry.descriptor.name === prior.name && JSON.stringify(entry.descriptor.keyColumns) === JSON.stringify(prior.keys));
        if (found) { draft.imported = found.descriptor; node("import-pane").hidden = true; }
        draft.needsRefresh = false;
      }
      if (draft.mode === "manager") renderList(draft); else renderLookup(draft);
      status(draft.imported ? "Importação confirmada neste Caso." : "");
    } catch (error) { if (current(draft, request)) status(String(error), true); }
    finally { if (current(draft, request)) busy(draft, false); }
  }
  async function chooseFile() {
    const draft = session; if (!draft || draft.busy || draft.mode !== "manager") return;
    try {
      assertOwner(draft); const path = await dialogApi.open({ multiple: false, directory: false, filters: [{ name: "Referência JSONL", extensions: ["jsonl"] }] });
      assertOwner(draft); if (!current(draft) || !path) return; await inspectFile(path);
    } catch (error) { if (current(draft)) status(String(error), true); }
  }
  async function inspectFile(path) {
    const draft = session; if (!draft || draft.busy || draft.mode !== "manager") return;
    const request = ++serial; busy(draft, true); status("Inspecionando JSONL…");
    try {
      assertOwner(draft); const inspection = await api("reference_inspect", { path, analysisContext: draft.owner.identity }, { silent: true, latest: "reference-inspect", analysisOwner: draft.owner });
      assertOwner(draft); if (!current(draft, request)) return; if (!validInspection(inspection)) throw Error("Inspeção de referência inválida.");
      Object.assign(draft, { path, inspection: copy(inspection), keys: [], imported: null });
      node("import-pane").hidden = false; node("file").textContent = path; node("import-name").value = inspection.name;
      node("inspection").textContent = `${fmtNum(inspection.rowCount)} registros · ${fmtNum(inspection.sourceBytes)} bytes · JSONL`;
      options(node("key-choice"), inspection.columns); renderKeys(draft); status("Escolha as colunas da chave antes de importar.");
    } catch (error) { if (current(draft, request)) status(String(error), true); }
    finally { if (current(draft, request)) busy(draft, false); }
  }
  function renderKeys(draft) {
    node("keys").replaceChildren();
    draft.keys.forEach((key, index) => {
      const row = el("li", "rf-key"), label = el("span", "", key); row.append(label);
      for (const [direction, caption] of [[-1, "Subir chave"], [1, "Descer chave"], [0, "Remover chave"]]) {
        const button = el("button", "btn ghost small", direction < 0 ? "↑" : direction > 0 ? "↓" : "Remover"); button.type = "button"; button.setAttribute("aria-label", `${caption}: ${key}`);
        button.disabled = draft.busy || direction < 0 && index === 0 || direction > 0 && index === draft.keys.length - 1;
        button.onclick = () => { if (draft.busy || !current(draft)) return; if (!direction) draft.keys.splice(index, 1); else [draft.keys[index], draft.keys[index + direction]] = [draft.keys[index + direction], draft.keys[index]]; renderKeys(draft); busy(draft, false); }; row.append(button);
      }
      node("keys").append(row);
    });
  }
  async function accepted(draft, result) {
    if (!window.AnalysisContexts.validSnapshot(result?.analysisContext) || result.analysisContext.caseId !== draft.owner.caseId || result.analysisContext.analysisId !== draft.owner.identity.analysisId) throw Error("Confirmação de configuração inválida.");
    assertOwner(draft, false); draft.owner = window.AnalysisContexts.capture(draft.owner.caseId);
    if (!await loadDerivedFields(draft.owner)) throw Error("Configuração salva. Atualize os campos para carregar o estado atual."); assertOwner(draft);
  }
  async function importReference() {
    const draft = session; if (!draft || draft.busy || !draft.inspection || !draft.keys.length || draft.needsRefresh) return;
    const name = node("import-name").value.trim(); if (!name) { status("Informe o nome da referência.", true); return; }
    const attempt = { path: draft.path, inspection: copy(draft.inspection), name, keys: [...draft.keys], ids: draft.entries.map(entry => entry.descriptor.id) };
    draft.mutating = true; busy(draft, true); status("Importando referência no Caso…"); let committed = false;
    try {
      assertOwner(draft); draft.attempt = attempt;
      const result = await api("reference_import", { path: attempt.path, inspection: attempt.inspection, name, keyColumns: attempt.keys, analysisContext: draft.owner.identity }, { silent: true, analysisOwner: draft.owner });
      committed = true; if (!current(draft)) return;
      if (!validDescriptor(result.reference)) throw Error("Confirmação de referência inválida."); draft.imported = result.reference;
      await accepted(draft, result); await refresh(); await list(draft); if (!current(draft)) return;
      node("import-pane").hidden = true; renderList(draft); status("Referência importada. Escolha Criar campo por referência para usá-la.");
    } catch (error) { if (current(draft)) { draft.needsRefresh = true; status(`${committed ? "A referência já foi importada. " : ""}${String(error)} Atualize as referências antes de repetir a importação.`, true); } }
    finally { draft.mutating = false; if (current(draft)) busy(draft, false); }
  }
  async function removeReference(referenceId) {
    const draft = session; if (!draft || draft.busy) return;
    const used = dependencies(referenceId); if (used.length) { status(`A referência é usada por: ${used.join(", ")}. Edite ou remova esses campos antes de remover o vínculo.`, true); return; }
    draft.mutating = true; busy(draft, true); status("Removendo vínculo do Caso…"); let committed = false;
    try {
      assertOwner(draft); const result = await api("reference_remove", { referenceId, analysisContext: draft.owner.identity }, { silent: true, analysisOwner: draft.owner }); committed = true;
      if (!current(draft)) return; await accepted(draft, result); await refresh(); await list(draft); if (!current(draft)) return; renderList(draft); status("Referência removida do Caso. O arquivo original foi preservado.");
    } catch (error) { if (current(draft)) status(`${committed ? "O vínculo já foi removido. " : ""}${String(error)}`, true); }
    finally { draft.mutating = false; if (current(draft)) busy(draft, false); }
  }
  function renderLookup(draft) {
    options(node("reference"), draft.entries.map(entry => ({ value: entry.descriptor.id, label: `${entry.descriptor.name}${entry.available ? "" : " · indisponível"}` })), draft.referenceId);
    const entry = draft.entries.find(entry => entry.descriptor.id === draft.referenceId), descriptor = entry?.descriptor;
    node("availability").textContent = !entry ? "Escolha uma referência importada neste Caso." : entry.available ? "Referência disponível neste Caso." : `Referência indisponível: ${entry.reason || "conteúdo ausente"}. O campo não será tratado como uma busca sem correspondência.`;
    if (descriptor) node("availability").textContent += ` Interpretação numérica: ${interpretation(descriptor)}.`;
    node("mappings").replaceChildren();
    for (const referenceColumn of descriptor?.keyColumns || []) {
      const row = el("label", "rf-mapping"), select = el("select"); row.append(el("span", "", `Chave da referência: ${referenceColumn}`));
      const fields = [...new Set([...state.columns, ...Object.values(draft.mappings)])].filter(Boolean);
      options(select, fields, draft.mappings[referenceColumn], "Escolha o campo do log…"); select.setAttribute("aria-label", `Campo do log para ${referenceColumn}`);
      select.onchange = () => { draft.mappings[referenceColumn] = select.value; draft.saved = false; }; row.append(select); node("mappings").append(row);
    }
    options(node("value-column"), descriptor?.columns || [], draft.valueColumn);
    node("save-lookup").disabled = draft.busy || !entry?.available;
  }
  async function openLookup(definition = null, { referenceId = definition?.lookup?.referenceId || "", sourceField = "", owner = window.AnalysisContexts?.capture(), anchor = $("#btn-colpicker") } = {}) {
    const draft = start("lookup", owner, anchor); if (!draft) return;
    Object.assign(draft, { referenceId, sourceField, editName: definition?.name || null, valueColumn: definition?.lookup?.valueColumn || "",
      mappings: Object.assign(Object.create(null), Object.fromEntries((definition?.lookup?.keys || []).map(key => [key.referenceColumn, key.sourceField]))), saved: false });
    node("field-name").value = definition?.name || ""; node("save-lookup").textContent = "Salvar campo";
    node("delete-lookup").hidden = !definition; node("delete-lookup").textContent = "Excluir campo";
    await refreshList(); if (current(draft)) (draft.editName ? node("reference") : node("field-name")).focus();
  }
  async function refreshFields(draft) {
    const scope = workspaceScope(), evidence = scope === "case" ? caseEvents("analysis") : null, signature = scope === "case" ? caseSig() : null;
    const profiles = await api("profile_fields", { filters: [], ...(evidence ? { caseEvents: evidence } : {}) }, { silent: true, analysisOwner: draft.owner });
    assertOwner(draft); if (scope !== workspaceScope() || signature !== (scope === "case" ? caseSig() : null)) throw Error("A área mudou depois de salvar. Atualize os campos nesta área.");
    const names = [...new Set([...(profiles || []).map(field => field.name), ...state.derivedFields.map(field => field.name)])];
    if (state.columns.includes("comentario")) names.push("comentario");
    if (names.length) state.columns = names;
    else if (draft.deleted) state.columns = state.columns.filter(name => name !== draft.editName && !name.startsWith(`${draft.editName}.`));
    state.visibleCols = (state.visibleCols || []).filter(name => state.columns.includes(name));
    includeDiscoveredFields(profiles, scope); fillColumnControls(); renderExploreTree(); if (await refresh() === false) throw Error("A configuração foi salva; atualize a visualização.");
  }
  async function saveLookup() {
    const draft = session; if (!draft || draft.busy || draft.mode !== "lookup" || draft.deleted) return;
    try {
      assertOwner(draft); const entry = draft.entries.find(entry => entry.descriptor.id === node("reference").value);
      if (!entry?.available) throw Error("Escolha uma referência disponível neste Caso.");
      const name = node("field-name").value.trim(), valueColumn = node("value-column").value;
      if (!name) throw Error("Informe o nome do campo resultante.");
      if (!draft.editName && (state.columns.includes(name) || state.derivedFields.some(field => field.name === name))) throw Error("Já existe um campo com esse nome.");
      const keys = entry.descriptor.keyColumns.map(referenceColumn => ({ referenceColumn, sourceField: draft.mappings[referenceColumn] || "" }));
      if (keys.some(key => !key.sourceField) || !entry.descriptor.columns.includes(valueColumn)) throw Error("Mapeie todas as chaves e escolha a coluna de valor.");
      draft.mutating = true; busy(draft, true); status(draft.saved ? "Atualizando campos…" : "Salvando campo no Caso…");
      if (!draft.saved) {
        const result = await api("reference_save_lookup", { name, lookup: { schemaVersion: 1, referenceId: entry.descriptor.id, keys, valueColumn }, analysisContext: draft.owner.identity }, { silent: true, analysisOwner: draft.owner });
        draft.saved = true; draft.editName = name; if (!current(draft)) return; await accepted(draft, result);
      } else { if (!await loadDerivedFields(draft.owner)) throw Error("Atualize a configuração do campo salvo."); }
      await refreshFields(draft); if (!current(draft)) return; close(); toast(`Campo "${name}" salvo no Caso.`, "ok");
    } catch (error) { if (current(draft)) { status(`${draft.saved ? "O campo já foi salvo. " : ""}${String(error)}`, true); if (draft.saved) node("save-lookup").textContent = "Atualizar campos"; } }
    finally { draft.mutating = false; if (current(draft)) busy(draft, false); }
  }
  async function deleteLookup() {
    const draft = session; if (!draft || draft.busy || draft.mode !== "lookup" || !draft.editName) return;
    draft.mutating = true; busy(draft, true); status(draft.deleted ? "Atualizando campos…" : "Excluindo campo do Caso…");
    try {
      assertOwner(draft);
      if (!draft.deleted) {
        const result = await api("delete_derived_field", { name: draft.editName }, { silent: true, analysisOwner: draft.owner });
        draft.deleted = true; if (!current(draft)) return; await accepted(draft, result);
      } else if (!await loadDerivedFields(draft.owner)) throw Error("Atualize os campos da configuração salva.");
      await refreshFields(draft); if (!current(draft)) return; close(); toast("Campo removido. A referência permanece no Caso.", "ok");
    } catch (error) { if (current(draft)) { status(`${draft.deleted ? "O campo já foi excluído. " : ""}${String(error)}`, true); if (draft.deleted) node("delete-lookup").textContent = "Atualizar campos"; } }
    finally { draft.mutating = false; if (current(draft)) busy(draft, false); }
  }
  function lookupMenuItem(sourceField, anchor) {
    const owner = window.AnalysisContexts?.capture();
    return { icon: "fa-table-list", label: "Criar campo por referência", onClick: () => openLookup(null, { sourceField, anchor, owner }) };
  }
  function describe(definition) { return window.AnalysisContexts?.context()?.config?.references?.find(reference => reference.id === definition.lookup?.referenceId)?.name || "Referência do Caso"; }
  node("choose").onclick = chooseFile; node("reload").onclick = () => refreshList({ reconcile: true }); node("import").onclick = importReference;
  node("add-key").onclick = () => { const draft = session, value = node("key-choice").value; if (!draft || draft.busy || !value || draft.keys.includes(value) || draft.keys.length >= 16) return; draft.keys.push(value); renderKeys(draft); busy(draft, false); };
  node("reference").onchange = () => { if (!session || session.busy) return; session.referenceId = node("reference").value; session.valueColumn = ""; session.mappings = Object.create(null); session.saved = false; const entry = session.entries.find(entry => entry.descriptor.id === session.referenceId); if (entry?.descriptor.keyColumns.length === 1 && session.sourceField) session.mappings[entry.descriptor.keyColumns[0]] = session.sourceField; renderLookup(session); };
  node("value-column").onchange = () => { if (session) { session.valueColumn = node("value-column").value; session.saved = false; } };
  node("field-name").oninput = () => { if (session) session.saved = false; }; node("save-lookup").onclick = saveLookup;
  node("delete-lookup").onclick = deleteLookup;
  node("close").onclick = node("cancel").onclick = close; overlay.onclick = event => { if (event.target === overlay) close(); };
  overlay.addEventListener("keydown", event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (event.key !== "Tab") return;
    const controls = [...overlay.querySelectorAll('button,input,select,[tabindex="0"]')].filter(item => !item.disabled && !item.hidden && !item.closest("[hidden]"));
    const first = controls[0], last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  });
  return { openManager, openLookup, chooseFile, inspectFile, importReference, removeReference, refreshList, saveLookup, deleteLookup, close, lookupMenuItem, describe, validDescriptor, validInspection };
})();
