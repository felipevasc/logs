/* Reversible Case exclusions. Capability admission gates every action. */
window.ExclusionArchive = (() => {
  "use strict";
  const PAGE_SIZE = 100, capabilities = new Map(), pendingCapabilities = new Map();
  const clone = value => structuredClone(value), count = value => Number.isSafeInteger(value) && value >= 0;
  const identityEqual = (a, b) => window.AnalysisContexts.signature(a) === window.AnalysisContexts.signature(b);
  const capKey = owner => JSON.stringify([owner?.instance, owner?.identity]);
  const archiveButton = el("button", "icon-btn"), filteredButton = el("button", "icon-btn");
  archiveButton.id = "btn-exclusion-archive"; archiveButton.type = filteredButton.type = "button";
  archiveButton.innerHTML = '<i class="fas fa-box-archive" aria-hidden="true"></i>'; archiveButton.setAttribute("aria-label", "Arquivo de exclusões");
  filteredButton.id = "btn-exclusion-filtered"; filteredButton.innerHTML = '<i class="fas fa-box-archive" aria-hidden="true"></i>'; filteredButton.setAttribute("aria-label", "Arquivar registros do recorte");
  $("#btn-case-menu").before(archiveButton); $("#btn-colpicker").before(filteredButton);
  const overlay = el("div", "modal-overlay exclusion-overlay"); overlay.id = "exclusion-modal"; overlay.hidden = true;
  overlay.innerHTML = `<section class="modal exclusion-modal" role="dialog" aria-modal="true" aria-labelledby="ex-title">
    <div class="modal-head"><h3 id="ex-title">Arquivo de exclusões</h3><button id="ex-close" type="button" class="icon-btn" aria-label="Fechar arquivo de exclusões"><i class="fas fa-xmark"></i></button></div>
    <div class="modal-body"><p id="ex-context" class="muted small"></p><p id="ex-status" class="small" role="status" aria-live="polite"></p>
    <div id="ex-preview-pane" hidden><p>Os registros serão ocultados nas consultas deste Caso. Os arquivos originais e as evidências guardadas são preservados. Você pode restaurar a exclusão depois.</p>
      <p id="ex-selection" class="notice"></p><p id="ex-preview-count"></p>
      <div class="fld"><label for="ex-label">Nome do lote (opcional)</label><input id="ex-label" maxlength="256" type="text"></div>
      <div class="fld"><label for="ex-reason">Motivo (opcional)</label><textarea id="ex-reason" maxlength="2000" rows="3"></textarea></div>
      <div class="modal-actions"><button id="ex-repreview" type="button" class="btn ghost">Recalcular prévia</button><button id="ex-cancel" type="button" class="btn ghost">Cancelar</button><button id="ex-commit" type="button" class="btn primary" disabled>Arquivar</button></div>
    </div>
    <div id="ex-archive-pane" hidden><p class="muted small">Restaurar remove a exclusão deste lote. Registros presentes em outros lotes podem continuar ocultos.</p>
      <div class="ex-archive-tools"><button id="ex-reload" type="button" class="btn ghost small">Atualizar</button><span id="ex-batch-page" class="muted small"></span><button id="ex-batches-prev" type="button" class="btn ghost small">Lotes anteriores</button><button id="ex-batches-next" type="button" class="btn ghost small">Mais lotes</button></div>
      <div class="ex-archive-layout"><div id="ex-batches" class="ex-batches" aria-label="Lotes arquivados"></div><section id="ex-members" hidden>
        <h4 id="ex-batch-title"></h4><p id="ex-batch-meta" class="muted small"></p><p id="ex-batch-reason" class="small"></p>
        <div class="ex-member-tools"><button id="ex-restore-batch" type="button" class="btn ghost small">Restaurar lote inteiro</button><button id="ex-restore-selected" type="button" class="btn ghost small" disabled>Restaurar selecionados desta página</button></div>
        <div id="ex-records" class="ex-records"></div><div class="ex-member-tools"><button id="ex-members-prev" type="button" class="btn ghost small">Registros anteriores</button><button id="ex-members-next" type="button" class="btn ghost small">Mais registros</button><span id="ex-member-page" class="muted small"></span></div>
      </section></div>
    </div></div></section>`;
  document.body.append(overlay);
  const node = id => overlay.querySelector(`#ex-${id}`);
  let session = null, serial = 0;
  const current = (draft, request = serial) => session === draft && !overlay.hidden && request === serial;
  function fail(message) { node("status").textContent = String(message); node("status").classList.add("ex-error"); }
  function status(message) { node("status").textContent = message; node("status").classList.remove("ex-error"); }
  function assertOwner(draft, revisions = true) {
    window.AnalysisContexts.assertOwner(draft.owner, { revisions });
    if (draft.scope !== workspaceScope()) throw Error("A área de trabalho mudou. Reabra esta ação no contexto atual.");
    if (draft.mode === "preview" && draft.scope === "dataset" && state.sourceIdentityUnconfirmed) throw Error("Confirme a fonte carregada antes de arquivar registros.");
    if (draft.mode === "preview" && draft.scope === "case" && draft.caseSignature !== caseSig()) throw Error("Os registros do Caso mudaram. Reabra a seleção antes de arquivar.");
  }
  function updateButtons() {
    const owner = window.AnalysisContexts?.capture(), cap = capabilities.get(capKey(owner));
    const reason = cap?.reason || "Arquivo de exclusões ainda indisponível nesta versão.";
    archiveButton.disabled = !owner?.caseId || !cap?.available; filteredButton.disabled = archiveButton.disabled || !state.loaded || !!state.sourceIdentityUnconfirmed;
    archiveButton.title = archiveButton.disabled ? reason : "Arquivo de exclusões deste Caso";
    filteredButton.title = state.sourceIdentityUnconfirmed ? "Confirme a fonte carregada antes de arquivar registros." : !state.loaded ? "Abra uma fonte para preparar uma exclusão." : filteredButton.disabled ? reason : "Arquivar todos os registros do recorte atual";
  }
  async function capability(captured = window.AnalysisContexts?.capture(), force = false) {
    if (!captured?.caseId || !window.AnalysisContexts) return { available: false, reason: "Abra um Caso para usar o arquivo de exclusões." };
    const owner = await window.AnalysisContexts.prepare(captured, { metadata: true });
    const key = capKey(owner), requestKey = JSON.stringify(owner);
    if (!force && capabilities.has(key)) return capabilities.get(key);
    if (pendingCapabilities.has(requestKey)) return pendingCapabilities.get(requestKey);
    const promise = (async () => {
      let result;
      try {
        result = await api("exclusion_capabilities", { analysisContext: owner.identity }, { silent: true, analysisOwner: owner });
        window.AnalysisContexts.assertOwner(owner);
        if (typeof result?.available !== "boolean") throw Error("Capacidade de exclusão inválida.");
      } catch (error) {
        if (!window.AnalysisContexts.isCurrent(owner)) throw error;
        result = { available: false, reason: "Arquivo de exclusões indisponível nesta versão." };
      }
      capabilities.set(key, result); updateButtons(); return result;
    })();
    pendingCapabilities.set(requestKey, promise);
    try { return await promise; } finally { if (pendingCapabilities.get(requestKey) === promise) pendingCapabilities.delete(requestKey); }
  }
  async function requireCapability(draft) {
    assertOwner(draft); const cap = await capability(draft.owner); assertOwner(draft);
    if (!cap.available) throw Error(cap.reason || "Arquivo de exclusões indisponível nesta versão.");
  }
  async function refreshCapability(force = false) { try { await capability(window.AnalysisContexts?.capture(), force); } catch { /* stale background availability never replaces active state */ } updateButtons(); }
  function discardPreview(draft) {
    const token = draft?.previewToken, owner = draft?.previewOwner || draft?.owner;
    if (!token || draft.commitInFlight || !owner?.identity) return Promise.resolve();
    draft.previewToken = null;
    return Promise.resolve().then(() => api("exclusion_discard", { previewToken: token, analysisContext: clone(owner.identity), sourceGeneration: owner.sourceGeneration },
      { silent: true, analysisOwner: owner })).catch(() => {});
  }
  function close() {
    void discardPreview(session);
    const anchor = session?.anchor; serial++; session = null; overlay.hidden = true;
    for (const key of ["exclusion-preview", "exclusion-list", "exclusion-page"]) window.Tasks?.cancelLatest(key);
    const target = [anchor, archiveButton, $("#btn-colpicker"), $("#case-select")].find(item => item?.isConnected && !item.disabled && !item.hidden && item.matches?.("button,input,select,textarea,[tabindex],a[href]") && !item.closest?.("[hidden],[inert]"));
    target?.focus();
  }
  function start(mode, owner, anchor) {
    window.ExclusionVisibility?.render(); void window.ExclusionVisibility?.refresh();
    void discardPreview(session);
    for (const key of ["exclusion-preview", "exclusion-list", "exclusion-page"]) window.Tasks?.cancelLatest(key);
    serial++; session = { mode, owner, anchor, scope: workspaceScope(), busy: false, selected: new Map(), batchCursors: [null], batchPage: 0, memberCursors: [null], memberPage: 0 };
    overlay.hidden = false; node("preview-pane").hidden = mode !== "preview"; node("archive-pane").hidden = mode !== "archive";
    node("title").textContent = mode === "preview" ? "Arquivar registros" : "Arquivo de exclusões";
    node("context").textContent = `Caso: ${activeCase()?.name || owner.caseId}`; status("");
    return session;
  }
  function setBusy(draft, value) {
    draft.busy = value;
    if (!current(draft)) return;
    node("label").disabled = node("reason").disabled = !!draft.committing && value || !!draft.commitUncertain;
    node("cancel").textContent = draft.operationStarted && value ? "Fechar" : "Cancelar";
    node("close").title = draft.operationStarted && value ? "Fechar; a alteração em andamento continuará" : "Fechar arquivo de exclusões";
    for (const id of ["commit", "repreview", "reload", "restore-batch", "restore-selected", "batches-prev", "batches-next", "members-prev", "members-next"]) node(id).disabled = value;
    for (const checkbox of node("records").querySelectorAll("input")) checkbox.disabled = value || !checkbox.__activeInBatch;
    if (!value) { node("commit").disabled = !draft.previewToken || !draft.selectedMembers; updateArchiveControls(draft); }
  }
  function validKey(key) {
    if (!key || typeof key.sourceKey !== "string" || !key.sourceKey || !key.locator || Object.keys(key.locator).length !== 1) return false;
    return Object.hasOwn(key.locator, "byte_offset") ? count(key.locator.byte_offset) : typeof key.locator.stable_record === "string" && !!key.locator.stable_record;
  }
  function validateMember(member) { return validKey(member?.key) && typeof member.eventRef === "string"; }
  function validateBatch(batch) { return !!batch && typeof batch.id === "string" && !!batch.id && count(batch.createdAtMs) && typeof batch.label === "string" && typeof batch.reason === "string" && count(batch.members) && typeof batch.active === "boolean"; }
  function openPreview(scope, { owner = window.AnalysisContexts?.capture(), anchor = filteredButton } = {}) {
    if (!owner?.caseId) return;
    try { window.AnalysisContexts.assertOwner(owner); } catch { toast("O contexto mudou. Abra a seleção novamente.", "info"); return; }
    if (!scope || scope.kind === "selected" && (!Array.isArray(scope.ids) || !scope.ids.every(count)) || scope.kind === "filtered" && !Array.isArray(scope.filters) || !["selected", "filtered"].includes(scope.kind)) { toast("Seleção inválida para arquivamento.", "err"); return; }
    const draft = start("preview", owner, anchor); draft.selection = clone(scope); draft.previewToken = null; draft.selectedMembers = null;
    if (draft.scope === "case") { draft.caseEvents = caseEvents(); draft.caseSignature = caseSig(); }
    node("label").value = ""; node("reason").value = ""; node("commit").disabled = true;
    node("preview-count").textContent = "Preparando a quantidade exata…";
    node("selection").textContent = scope.kind === "selected" ? `Seleção capturada: ${fmtNum(scope.ids.length)} registro(s).`
      : scope.filters.length ? `Recorte capturado: ${scope.filters.map(filter => chipLabel(filter)).join(" · ")}` : "Recorte capturado: todos os registros visíveis neste Caso e nesta área.";
    node("label").focus(); void preview(draft);
  }
  async function preview(draft = session) {
    if (!draft || draft.mode !== "preview" || draft.busy) return;
    const disposal = discardPreview(draft); draft.commitPayload = null; draft.commitUncertain = false;
    const request = ++serial; draft.selectedMembers = null; setBusy(draft, true); status("Preparando a lista exata de registros…");
    try {
      await disposal; if (!current(draft, request)) return;
      await requireCapability(draft);
      if (!current(draft, request)) return;
      const args = await caseArgs({ analysisContext: draft.owner.identity, sourceGeneration: draft.owner.sourceGeneration, scope: clone(draft.selection), ...(draft.scope === "case" ? { caseEvents: draft.caseEvents } : {}) });
      assertOwner(draft); if (!current(draft, request)) return;
      const result = await api("exclusion_preview", args, { silent: true, latest: "exclusion-preview", analysisOwner: draft.owner, caseEvents: draft.caseEvents });
      assertOwner(draft); if (!current(draft, request)) return;
      if (typeof result?.previewToken !== "string" || !result.previewToken || !count(result.selectedMembers) || !identityEqual(result.analysisContext, draft.owner.identity)
        || draft.scope === "dataset" && draft.owner.sourceGeneration != null && result.sourceGeneration !== draft.owner.sourceGeneration) throw Error("A prévia de exclusão não pertence a este contexto.");
      draft.previewToken = result.previewToken; draft.previewOwner = { ...draft.owner, sourceGeneration: result.sourceGeneration }; draft.selectedMembers = result.selectedMembers;
      node("preview-count").textContent = `${fmtNum(result.selectedMembers)} registro(s) na lista exata preparada.`;
      node("commit").textContent = `Arquivar ${fmtNum(result.selectedMembers)} registro(s)`; status(result.selectedMembers ? "Confira o lote e confirme para arquivar." : "Nenhum registro corresponde à seleção.");
    } catch (error) { if (current(draft, request)) { node("preview-count").textContent = "A prévia não foi concluída."; fail(error); } }
    finally { if (current(draft, request)) setBusy(draft, false); }
  }
  async function refreshAfterReceipt(draft, result) {
    if (!window.AnalysisContexts.validSnapshot(result?.analysisContext) || result.analysisContext.caseId !== draft.owner.caseId || result.analysisContext.analysisId !== draft.owner.identity?.analysisId || typeof result.batchId !== "string" || !count(result.selectedMembers) || result.newlyVisible != null && !count(result.newlyVisible)
      || draft.mode === "archive" && result.batchId !== draft.batch?.id || draft.mode === "preview" && result.selectedMembers !== draft.selectedMembers) throw Error("Confirmação de exclusão inválida.");
    window.AnalysisContexts.assertOwner(draft.owner, { revisions: false });
    draft.owner = window.AnalysisContexts.capture(draft.owner.caseId);
    if (!await loadDerivedFields(draft.owner)) throw Error("Alteração salva. Atualize a visualização para carregar a configuração atual.");
    window.AnalysisContexts.assertOwner(draft.owner);
    if (draft.scope === workspaceScope()) await refresh();
    await refreshCapability(true);
  }
  async function commit() {
    const draft = session; if (!draft || draft.mode !== "preview" || draft.busy || !draft.previewToken || !draft.selectedMembers) return;
    const payload = draft.commitPayload || { previewToken: draft.previewToken, label: node("label").value.trim() || null, reason: node("reason").value.trim() || null };
    draft.committing = true; setBusy(draft, true); status("Arquivando o lote…"); let committed = false;
    try {
      await requireCapability(draft); if (!current(draft)) return;
      draft.operationStarted = true; setBusy(draft, true); status("Arquivando o lote… A operação continuará se você fechar esta janela.");
      draft.commitPayload = payload; draft.commitInFlight = true;
      let result;
      try { result = await api("exclusion_commit", payload, { silent: true, analysisOwner: draft.previewOwner || draft.owner }); }
      finally { draft.commitInFlight = false; }
      committed = true; draft.previewToken = null; draft.commitUncertain = false;
      await refreshAfterReceipt(draft, result);
      if (session !== draft) return;
      close(); toast(`Lote arquivado com ${fmtNum(result.selectedMembers)} registro(s). Você pode restaurá-lo no arquivo de exclusões.`, "ok");
    } catch (error) {
      if (!committed && (!draft.commitPayload || /expir|stale|context(?:o)?(?:_|\s)+(?:changed|mudou)|configura.*mudou|(?:caso|fonte).*mudou/i.test(String(error)))) void discardPreview(draft);
      else if (!committed) draft.commitUncertain = !!draft.commitPayload;
      if (session === draft) {
        if (draft.commitUncertain) node("commit").textContent = "Confirmar resultado do arquivamento";
        fail(`${committed ? "O lote já foi arquivado. " : draft.commitUncertain ? "Não foi possível confirmar o resultado. Tente confirmar novamente com o mesmo lote. " : ""}${error}`);
      } else if (!committed) void discardPreview(draft);
    }
    finally { draft.committing = false; draft.operationStarted = false; if (session === draft) setBusy(draft, false); }
  }
  async function openArchive({ owner = window.AnalysisContexts?.capture(), anchor = archiveButton } = {}) {
    if (!owner?.caseId) return;
    try { window.AnalysisContexts.assertOwner(owner); } catch { toast("O contexto mudou. Abra o arquivo novamente.", "info"); return; }
    const draft = start("archive", owner, anchor); node("batches").replaceChildren(); node("members").hidden = true; node("reload").focus();
    await loadBatches(draft);
  }
  async function loadBatches(draft = session, page = 0, keepBatch = null) {
    if (!draft || draft.mode !== "archive" || draft.busy) return;
    const request = ++serial; setBusy(draft, true); status("Carregando lotes de exclusão…");
    try {
      await requireCapability(draft); if (!current(draft, request)) return;
      const result = await api("exclusion_list", { analysisContext: draft.owner.identity, cursor: draft.batchCursors[page] || null, limit: PAGE_SIZE }, { silent: true, latest: "exclusion-list", analysisOwner: draft.owner });
      assertOwner(draft); if (!current(draft, request)) return;
      if (!identityEqual(result?.analysis, draft.owner.identity) || !Array.isArray(result.batches) || result.batches.length > PAGE_SIZE || !result.batches.every(validateBatch) || result.nextCursor != null && typeof result.nextCursor !== "string") throw Error("Página de lotes inválida.");
      draft.batches = result.batches; draft.batchPage = page; draft.batchCursors[page + 1] = result.nextCursor || null;
      node("batches").replaceChildren();
      for (const batch of result.batches) {
        const button = el("button", "ex-batch"), label = batch.label || `Lote de ${new Date(batch.createdAtMs).toLocaleString("pt-BR")}`;
        button.type = "button"; button.append(el("strong", "", trunc(label, 100)), el("small", "muted", `${new Date(batch.createdAtMs).toLocaleString("pt-BR")} · ${fmtNum(batch.members)} no lote original · ${batch.active ? "Ativo" : "Restaurado"}`));
        button.title = label; button.onclick = () => { if (session === draft && !draft.busy) void loadMembers(draft, batch.id); }; node("batches").append(button);
      }
      if (!result.batches.length) node("batches").append(el("p", "muted small", "Nenhum lote nesta página."));
      node("batch-page").textContent = `${result.batches.length} lote(s) nesta página`; status("");
      setBusy(draft, false);
      const id = keepBatch || result.batches[0]?.id; if (id) await loadMembers(draft, id);
      else { draft.batch = null; node("members").hidden = true; }
    } catch (error) { if (current(draft, request)) fail(error); }
    finally { if (current(draft, request)) setBusy(draft, false); }
  }
  function updateArchiveControls(draft) {
    if (draft.mode !== "archive") return;
    node("batches-prev").disabled = draft.busy || draft.batchPage === 0;
    node("batches-next").disabled = draft.busy || !draft.batchCursors[draft.batchPage + 1];
    node("members-prev").disabled = draft.busy || draft.memberPage === 0;
    node("members-next").disabled = draft.busy || !draft.memberCursors[draft.memberPage + 1];
    node("restore-batch").disabled = draft.busy || !draft.batch?.active;
    node("restore-selected").disabled = draft.busy || !draft.batch?.active || !draft.selected.size;
    node("restore-selected").textContent = draft.selected.size ? `Restaurar ${fmtNum(draft.selected.size)} selecionado(s) desta página` : "Restaurar selecionados desta página";
  }
  async function loadMembers(draft, batchId, page = 0) {
    if (!draft || draft.busy) return;
    if (draft.batch?.id !== batchId) { draft.memberCursors = [null]; page = 0; node("members").hidden = true; }
    const evidence = draft.scope === "case" ? { signature: caseSig(), rows: caseEvents() } : null;
    const checkEvidence = () => { if (evidence && evidence.signature !== caseSig()) throw Error("Os registros disponíveis do Caso mudaram. Atualize esta página do arquivo."); };
    const request = ++serial; draft.selected.clear(); setBusy(draft, true); status("Carregando registros arquivados…");
    try {
      await requireCapability(draft); if (!current(draft, request)) return;
      checkEvidence();
      const args = await caseArgs({ analysisContext: draft.owner.identity, sourceGeneration: draft.owner.sourceGeneration, batchId, cursor: draft.memberCursors[page] || null, limit: PAGE_SIZE, ...(evidence ? { caseEvents: evidence.rows } : {}) });
      assertOwner(draft); checkEvidence(); if (!current(draft, request)) return;
      const result = await api("exclusion_archive_page", args, { silent: true, latest: "exclusion-page", analysisOwner: draft.owner, caseEvents: evidence?.rows });
      assertOwner(draft); checkEvidence(); if (!current(draft, request)) return;
      if (!identityEqual(result?.analysis, draft.owner.identity) || !validateBatch(result.batch) || result.batch.id !== batchId || !Array.isArray(result.rows) || result.rows.length > PAGE_SIZE
        || !result.rows.every(row => validateMember(row.member) && typeof row.activeInBatch === "boolean" && Object.hasOwn(result.sources || {}, row.member.key.sourceKey)
          && ["version", "recordSpace", "label"].every(key => typeof result.sources[row.member.key.sourceKey][key] === "string")) || result.nextCursor != null && !validKey(result.nextCursor)
        || result.activeMembers != null && !count(result.activeMembers)) throw Error("Página do arquivo inválida.");
      draft.batch = result.batch; draft.pageRows = result.rows; draft.memberPage = page; draft.memberCursors[page + 1] = clone(result.nextCursor || null);
      node("members").hidden = false; node("batch-title").textContent = result.batch.label || "Lote sem nome";
      node("batch-meta").textContent = `${new Date(result.batch.createdAtMs).toLocaleString("pt-BR")} · ${fmtNum(result.batch.members)} registro(s) no lote original · ${result.activeMembers == null ? "quantidade restante não calculada" : `${fmtNum(result.activeMembers)} exclusão(ões) restante(s) neste lote`}`;
      node("batch-reason").textContent = trunc(result.batch.reason || "Sem motivo informado.", 2000); node("records").replaceChildren();
      for (const row of result.rows) {
        const item = el("div", "ex-record"), select = el("input"), body = el("div", "ex-record-body"), source = result.sources?.[row.member.key.sourceKey];
        select.type = "checkbox"; select.__activeInBatch = result.batch.active && row.activeInBatch; select.disabled = !select.__activeInBatch; select.setAttribute("aria-label", `Selecionar ${row.member.eventRef || "registro arquivado"}`);
        const memberKey = JSON.stringify(row.member.key);
        select.onchange = () => { if (!current(draft) || draft.busy) return; if (select.checked) draft.selected.set(memberKey, clone(row.member)); else draft.selected.delete(memberKey); updateArchiveControls(draft); };
        const title = source?.label || "Origem registrada";
        body.append(el("strong", "", trunc(title, 100)), el("span", "muted small", result.batch.active && row.activeInBatch ? "Excluído por este lote" : "Restaurado neste lote"));
        const provenance = el("details", "ex-provenance"), summary = el("summary", "", "Procedência do registro");
        const location = row.member.key.locator.byte_offset != null ? `Posição: ${row.member.key.locator.byte_offset}` : `Registro: ${row.member.key.locator.stable_record}`;
        provenance.append(summary, el("pre", "", [source?.label, source?.version && `Versão: ${source.version}`, source?.recordSpace && `Espaço de registros: ${source.recordSpace}`, location, row.member.eventRef].filter(Boolean).join("\n"))); body.append(provenance);
        if (row.event) {
          const content = el("details", "ex-event-preview"), event = window.EvidenceUI?.redact(row.event) || row.event;
          content.append(el("summary", "", "Conteúdo disponível"), el("pre", "code-pane", window.FieldTransforms ? window.FieldTransforms.bounded(event, 4096) : trunc(JSON.stringify(event, null, 2), 4096))); body.append(content);
        } else body.append(el("span", "ex-unavailable small", trunc(row.unavailableReason || "Conteúdo indisponível: a origem não está aberta ou sua versão mudou.", 512)));
        item.append(select, body); node("records").append(item);
      }
      if (!result.rows.length) node("records").append(el("p", "muted small", "Nenhum registro nesta página."));
      node("member-page").textContent = `${result.rows.length} registro(s) nesta página`; status("");
    } catch (error) { if (current(draft, request)) fail(error); }
    finally { if (current(draft, request)) setBusy(draft, false); }
  }
  async function restore(selectedOnly) {
    const draft = session; if (!draft || draft.mode !== "archive" || draft.busy || !draft.batch?.active) return;
    const members = [...draft.selected.values()].map(clone), batchId = draft.batch.id;
    if (selectedOnly && (!members.length || members.length > 500)) return;
    setBusy(draft, true); status("Restaurando a exclusão…"); let committed = false;
    try {
      await requireCapability(draft); if (!current(draft)) return;
      draft.operationStarted = true; setBusy(draft, true); status("Restaurando a exclusão… A operação continuará se você fechar esta janela.");
      const result = await api(selectedOnly ? "exclusion_restore_selected" : "exclusion_restore_batch", { analysisContext: draft.owner.identity, batchId, ...(selectedOnly ? { members } : {}) }, { silent: true, analysisOwner: draft.owner });
      committed = true; await refreshAfterReceipt(draft, result);
      if (session !== draft) return;
      const message = selectedOnly ? "Seleção restaurada neste lote." : "Lote restaurado.";
      toast(count(result.newlyVisible) ? `${message} Registros que voltaram a ficar visíveis: ${fmtNum(result.newlyVisible)}.` : `${message} Outros lotes ainda podem manter registros ocultos.`, "ok");
      draft.selected.clear(); draft.batchCursors = [null]; draft.memberCursors = [null]; setBusy(draft, false); await loadBatches(draft, 0, batchId);
    } catch (error) { if (session === draft) fail(`${committed ? "A restauração já foi salva. " : ""}${error}`); }
    finally { draft.operationStarted = false; if (session === draft) setBusy(draft, false); }
  }
  function selectedMenuItem(event, anchor) {
    const owner = window.AnalysisContexts?.capture(), scope = workspaceScope(); ensureSelectionOwner();
    const selected = state.selectedEventRows?.has(event.id) ? [...state.selectedEventRows.values()] : [event];
    const ids = [...new Set(selected.map(row => row.id))], cap = capabilities.get(capKey(owner));
    return { icon: "fa-box-archive", label: `Arquivar ${fmtNum(ids.length)} registro(s) selecionado(s)…`, disabled: !cap?.available || scope === "dataset" && !!state.sourceIdentityUnconfirmed, title: scope === "dataset" && state.sourceIdentityUnconfirmed ? "Confirme a fonte carregada antes de arquivar registros." : cap?.reason || "Arquivo de exclusões ainda indisponível.",
      onClick: () => { if (scope !== workspaceScope()) { toast("A área mudou. Abra a seleção novamente.", "info"); return; } openPreview({ kind: "selected", ids }, { owner, anchor }); } };
  }
  archiveButton.onclick = () => { void openArchive(); };
  filteredButton.onclick = () => openPreview({ kind: "filtered", filters: clone(backendFilters()) });
  node("close").onclick = node("cancel").onclick = close; node("repreview").onclick = () => { void preview(); }; node("commit").onclick = commit;
  node("reload").onclick = () => { if (session) { session.batchCursors = [null]; void loadBatches(); } };
  node("batches-prev").onclick = () => { if (session) void loadBatches(session, session.batchPage - 1); };
  node("batches-next").onclick = () => { if (session) void loadBatches(session, session.batchPage + 1); };
  node("members-prev").onclick = () => { if (session?.batch) void loadMembers(session, session.batch.id, session.memberPage - 1); };
  node("members-next").onclick = () => { if (session?.batch) void loadMembers(session, session.batch.id, session.memberPage + 1); };
  node("restore-batch").onclick = () => { void restore(false); }; node("restore-selected").onclick = () => { void restore(true); };
  overlay.onclick = event => { if (event.target === overlay) close(); };
  overlay.addEventListener("keydown", event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (event.key !== "Tab") return;
    const controls = [...overlay.querySelectorAll("button,input,textarea,select")].filter(item => !item.disabled && !item.hidden && !item.closest("[hidden]"));
    const first = controls[0], last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  });
  function contextChanged() {
    updateButtons();
    if (session && !session.busy) { try { assertOwner(session); } catch { void discardPreview(session); node("commit").disabled = true; fail("O contexto mudou. Reabra o arquivo ou a seleção no Caso atual."); } }
    void refreshCapability();
  }
  document.addEventListener("analysis-context-change", contextChanged); document.addEventListener("workspace-context-change", contextChanged);
  Promise.resolve(window.workspaceBootstrap).then(() => refreshCapability()).catch(() => {}); updateButtons();
  return { openPreview, openArchive, preview, commit, restore, close, refreshCapability, updateButtons, selectedMenuItem, validateMember };
})();
