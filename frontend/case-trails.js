/* Authored trails reference saved evidence; they never duplicate or delete its records. */
window.CaseTrails = (() => {
  "use strict";
  const contexts = new Map(), PAGE = 40;
  let host = null, currentCase = null, view = null, generation = 0, undo = null, imageObserver = null;
  const pendingImages = new WeakMap();
  const button = (text, action, cls = "btn ghost small") => { const node = el("button", cls, text); node.type = "button"; node.onclick = action; return node; };
  const icon = (name, label, action) => { const node = button("", action, "icon-btn"); node.innerHTML = `<i class="fas ${name}" aria-hidden="true"></i>`; node.title = label; node.setAttribute("aria-label", label); return node; };
  const itemName = item => item?.label || item?.name || "Item sem título";
  const trails = c => Array.isArray(c?.caseTrails) ? c.caseTrails : [];
  const narrative = target => window.CaseContent?.narrative(target) || { summary: target.summary || target.note || "", details: target.details || "" };
  const active = () => host?.isConnected && document.body.dataset.page === "case-trails" && activeCase()?.id === currentCase?.id && workspaceScope() === "case";
  const exists = (c, trail) => state.cases.cases.some(item => item === c) && (!trail || trails(c).includes(trail));
  function notifyChanged(c) {
    if (activeCase()?.id === c.id) updateAnalysisBadge();
    return saveCases();
  }
  function showNarrative(parent, target) {
    const text = narrative(target);
    if (text.summary) parent.append(el("p", "case-trail-summary", text.summary));
    if (text.details) { const details = el("details", "case-trail-details"); details.append(el("summary", "", "Detalhes"), el("div", "case-trail-text", text.details)); parent.append(details); }
  }
  function images(parent, target, token) {
    if (!target.attachments?.length || !window.CaseContent?.mountAttachments) return;
    const gallery = el("div", "case-trail-images"); gallery.textContent = `${fmtNum(target.attachments.length)} imagens`; parent.append(gallery);
    const mount = () => { if (token !== generation || !gallery.isConnected) return; gallery.textContent = ""; Promise.resolve(window.CaseContent.mountAttachments(gallery, target, { editable: false })).catch(() => { if (token === generation && gallery.isConnected) gallery.textContent = "Não foi possível abrir as imagens deste item."; }); };
    if (imageObserver) { pendingImages.set(gallery, mount); imageObserver.observe(gallery); } else mount();
  }
  function pager(parent, page, total, change, label) {
    const pages = Math.max(1, Math.ceil(total / PAGE)), node = el("div", "case-trails-pager");
    const previous = button("Anterior", () => change(page - 1)), next = button("Próxima", () => change(page + 1)); previous.disabled = page === 0; next.disabled = page >= pages - 1;
    previous.setAttribute("aria-label", `${label} anteriores`); next.setAttribute("aria-label", `Próximos ${label.toLowerCase()}`);
    node.append(previous, el("span", "", `${page + 1} / ${pages} · ${fmtNum(total)}`), next); parent.append(node);
  }
  async function editTrail(trail = null) {
    const c = currentCase; if (!c || !window.CaseContent) return;
    const target = trail || { id: `ct-${nid()}`, title: "", summary: "", details: "", itemIds: [], attachments: [], createdAt: Date.now(), updatedAt: Date.now() };
    const changed = await CaseContent.edit(target, { title: trail ? "Editar trilha" : "Nova trilha", nameField: "title", onSave: async () => {
      if (!exists(c, trail)) return false;
      const added = !trail && !trails(c).includes(target); if (added) { c.caseTrails ||= []; c.caseTrails.push(target); }
      target.updatedAt = Date.now(); const saved = await notifyChanged(c);
      if (saved === false && added) c.caseTrails = c.caseTrails.filter(item => item !== target);
      return saved;
    } });
    if (!changed) return;
    if (!exists(c, trail)) { toast("O Caso mudou durante a edição. Reabra a trilha para continuar.", "info"); return; }
    if (currentCase === c) { view.selected = target.id; view.itemPage = 0; view.itemScroll = 0; }
    if (active()) draw();
  }
  function removeTrail(trail) {
    const c = currentCase, index = trails(c).indexOf(trail); if (index < 0) return;
    c.caseTrails.splice(index, 1); undo = { caseId: c.id, trail, index }; view.selected = null; notifyChanged(c); draw();
  }
  function restoreTrail() {
    const c = state.cases.cases.find(item => item.id === undo?.caseId); if (!c) { undo = null; return; }
    if (!trails(c).some(trail => trail.id === undo.trail.id)) { c.caseTrails ||= []; c.caseTrails.splice(Math.min(undo.index, c.caseTrails.length), 0, undo.trail); }
    if (c === currentCase) view.selected = undo.trail.id;
    undo = null; notifyChanged(c); if (active()) draw();
  }
  function selectItems(trail) {
    const c = currentCase, focus = document.activeElement;
    const selected = new Set(trail.itemIds || []), known = new Set((c.items || []).map(item => item.id));
    const choices = [...(c.items || []), ...[...selected].filter(id => !known.has(id)).map(id => ({ id, label: "Item indisponível", missing: true }))];
    let search = "", page = 0;
    const overlay = el("div", "modal-overlay case-trail-picker"), modal = el("section", "modal"); modal.setAttribute("role", "dialog"); modal.setAttribute("aria-modal", "true"); modal.setAttribute("aria-labelledby", "case-trail-picker-title");
    const head = el("header", "modal-head"), title = el("h3", "", "Associar itens"); title.id = "case-trail-picker-title";
    const close = () => { overlay.remove(); focus?.isConnected && focus.focus(); }; head.append(title, icon("fa-xmark", "Fechar seleção de itens", close));
    const tools = el("div", "case-trail-picker-tools"), input = el("input"); input.type = "search"; input.placeholder = "Buscar nos itens do Caso…"; input.setAttribute("aria-label", "Buscar item para associar");
    const count = el("span", "", ""), list = el("div", "case-trail-picker-list"), navigation = el("div"); count.setAttribute("aria-live", "polite"); tools.append(input, count);
    function drawChoices() {
      const rows = choices.filter(item => `${itemName(item)} ${narrative(item).summary}`.toLocaleLowerCase().includes(search)); page = Math.min(page, Math.max(0, Math.ceil(rows.length / PAGE) - 1));
      list.replaceChildren(); navigation.replaceChildren(); count.textContent = `${fmtNum(selected.size)} associados`;
      for (const item of rows.slice(page * PAGE, (page + 1) * PAGE)) {
        const label = el("label", "case-trail-choice"), checkbox = el("input"); checkbox.type = "checkbox"; checkbox.checked = selected.has(item.id); checkbox.setAttribute("aria-label", itemName(item));
        checkbox.onchange = () => { checkbox.checked ? selected.add(item.id) : selected.delete(item.id); count.textContent = `${fmtNum(selected.size)} associados`; };
        const text = el("span"); text.append(el("strong", "", itemName(item)), el("small", "", item.missing ? "Referência indisponível" : `${fmtNum(item.rows?.length || 0)} registros${narrative(item).summary ? ` · ${narrative(item).summary}` : ""}`)); label.append(checkbox, text); list.append(label);
      }
      if (!rows.length) list.append(el("p", "case-trails-empty", choices.length ? "Nenhum item corresponde à busca." : "Salve registros na Análise para associá-los a esta trilha."));
      pager(navigation, page, rows.length, next => { page = next; drawChoices(); list.scrollTop = 0; }, "Itens");
    }
    input.oninput = () => { search = input.value.toLocaleLowerCase(); page = 0; drawChoices(); };
    const footer = el("footer", "case-trail-picker-footer"), apply = button("Aplicar", async () => {
      if (!exists(c, trail)) { close(); toast("A trilha mudou. Abra a seleção novamente.", "info"); return; }
      trail.itemIds = [...selected]; trail.updatedAt = Date.now(); apply.disabled = true; await notifyChanged(c); close(); if (active()) { view.itemPage = 0; draw(); }
    }, "btn primary"); footer.append(button("Cancelar", close), apply);
    modal.append(head, tools, list, navigation, footer); overlay.append(modal); document.body.append(overlay); drawChoices(); input.focus();
    overlay.onclick = event => { if (event.target === overlay) close(); };
    overlay.onkeydown = event => {
      if (event.key === "Escape") { event.stopPropagation(); event.preventDefault(); close(); }
      if (event.key === "Tab") { const nodes = [...modal.querySelectorAll("button:not(:disabled),input:not(:disabled)")].filter(node => node.offsetParent !== null); const first = nodes[0], last = nodes.at(-1); if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }
    };
  }
  function drawDetail(parent, trail, token) {
    if (!trail) { const empty = el("div", "case-trails-empty"); empty.append(el("p", "", "Organize os itens do Caso em uma sequência com a sua interpretação."), button("Criar trilha", () => editTrail(), "btn primary")); parent.append(empty); return; }
    const header = el("header", "case-trail-head"), title = el("div"); title.append(el("h2", "", trail.title || "Trilha sem título"), el("small", "", `${fmtNum(trail.itemIds?.length || 0)} itens associados`));
    const actions = el("div", "case-trail-actions"); actions.append(button("Associar itens", () => selectItems(trail), "btn primary small"), icon("fa-pen", "Editar trilha", () => editTrail(trail)), icon("fa-trash-can", "Remover trilha", () => removeTrail(trail))); header.append(title, actions); parent.append(header);
    const body = el("div", "case-trail-body"); showNarrative(body, trail); images(body, trail, token);
    const ids = trail.itemIds || [], byId = new Map((currentCase.items || []).map(item => [item.id, item])); view.itemPage = Math.min(view.itemPage, Math.max(0, Math.ceil(ids.length / PAGE) - 1));
    for (let index = view.itemPage * PAGE; index < Math.min(ids.length, (view.itemPage + 1) * PAGE); index++) {
      const id = ids[index], item = byId.get(id), card = el("article", "case-trail-item"); card.dataset.itemId = id;
      const itemHead = el("div", "case-trail-item-head"), number = el("span", "case-trail-position", String(index + 1)), content = el("div", "case-trail-item-title"); content.append(el("h3", "", item ? itemName(item) : "Item indisponível"));
      if (item?.rows?.length) content.append(el("small", "", `${fmtNum(item.rows.length)} registros preservados`));
      const itemActions = el("div", "case-trail-item-actions");
      const move = direction => { const next = index + direction; if (next < 0 || next >= ids.length) return; [ids[index], ids[next]] = [ids[next], ids[index]]; trail.updatedAt = Date.now(); view.itemPage = Math.floor(next / PAGE); notifyChanged(currentCase); draw(); };
      const up = icon("fa-arrow-up", "Mover item acima", () => move(-1)), down = icon("fa-arrow-down", "Mover item abaixo", () => move(1)); up.disabled = index === 0; down.disabled = index === ids.length - 1;
      itemActions.append(up, down, icon("fa-link-slash", "Desassociar item", () => { trail.itemIds = ids.filter((_, position) => position !== index); trail.updatedAt = Date.now(); notifyChanged(currentCase); draw(); }));
      itemHead.append(number, content, itemActions); card.append(itemHead);
      if (item) {
        showNarrative(card, item); images(card, item, token);
        const actions = el("div", "case-trail-item-footer"); actions.append(button("Editar item", async () => { await window.CaseContent?.editItem(id); if (active()) draw(); }));
        if (item.rows?.length) actions.append(button(item.rows.length === 1 ? "Ver registro" : "Primeiro registro", () => { showDetail(item.rows[0], item.sourceSpec); $("#dr-prev").hidden = $("#dr-next").hidden = true; }));
        card.append(actions);
      } else card.append(el("p", "muted small", "O item foi removido do Caso. Você pode desassociar esta referência."));
      body.append(card);
    }
    if (!ids.length) body.append(el("p", "case-trails-empty", "Associe os itens que sustentam esta trilha. Eles continuam disponíveis no Caso."));
    body.onscroll = () => { view.itemScroll = body.scrollTop; }; parent.append(body); body.scrollTop = view.itemScroll;
    if (ids.length > PAGE) pager(parent, view.itemPage, ids.length, next => { view.itemPage = next; view.itemScroll = 0; draw(); }, "Itens");
  }
  function draw() {
    if (!active()) return; const token = ++generation; imageObserver?.disconnect(); host.replaceChildren();
    imageObserver = typeof IntersectionObserver === "function" ? new IntersectionObserver(entries => { for (const entry of entries) if (entry.isIntersecting) { imageObserver?.unobserve(entry.target); pendingImages.get(entry.target)?.(); pendingImages.delete(entry.target); } }, { root: host, rootMargin: "150px" }) : null;
    const toolbar = el("div", "case-trails-toolbar"), search = el("input"); search.type = "search"; search.placeholder = "Buscar trilha…"; search.setAttribute("aria-label", "Buscar trilha"); search.value = view.search;
    search.oninput = () => { view.search = search.value; view.listPage = 0; drawList(); };
    toolbar.append(search, button("Nova trilha", () => editTrail(), "btn primary small"), button("Possíveis trilhas", () => Workspace.showPage("journeys")));
    if (undo?.caseId === currentCase.id) toolbar.append(button("Desfazer remoção", restoreTrail)); host.append(toolbar);
    const grid = el("div", "case-trails-grid"), listPanel = el("section", "case-trails-list-panel"), detail = el("section", "case-trails-detail"); listPanel.setAttribute("aria-label", "Trilhas do Caso"); detail.setAttribute("aria-label", "Narrativa da trilha"); grid.append(listPanel, detail); host.append(grid);
    const available = trails(currentCase); if (!available.some(trail => trail.id === view.selected)) view.selected = available[0]?.id || null;
    function drawList() {
      listPanel.replaceChildren(); const rows = available.filter(trail => `${trail.title} ${narrative(trail).summary}`.toLocaleLowerCase().includes(view.search.toLocaleLowerCase())); view.listPage = Math.min(view.listPage, Math.max(0, Math.ceil(rows.length / PAGE) - 1));
      const list = el("div", "case-trails-list");
      for (const trail of rows.slice(view.listPage * PAGE, (view.listPage + 1) * PAGE)) { const entry = button("", () => { view.selected = trail.id; view.itemPage = 0; view.itemScroll = 0; draw(); }, "case-trail-list-item"); entry.setAttribute("aria-current", String(view.selected === trail.id)); entry.append(el("strong", "", trail.title || "Trilha sem título"), el("small", "", `${fmtNum(trail.itemIds?.length || 0)} itens`)); const summary = narrative(trail).summary; if (summary) entry.append(el("p", "", summary)); list.append(entry); }
      if (!rows.length) list.append(el("p", "case-trails-empty", available.length ? "Nenhuma trilha encontrada." : "Nenhuma trilha criada.")); listPanel.append(list);
      if (rows.length > PAGE) pager(listPanel, view.listPage, rows.length, next => { view.listPage = next; drawList(); }, "Trilhas");
    }
    drawList(); drawDetail(detail, available.find(trail => trail.id === view.selected), token);
  }
  function render(container, c = activeCase()) {
    generation++; currentCase = c; container.replaceChildren(); host = el("div", "case-trails-workspace"); container.append(host);
    if (!c) { host.append(el("p", "case-trails-empty", "Crie um Caso para organizar suas trilhas.")); return; }
    if (!contexts.has(c.id)) { contexts.set(c.id, { selected: null, search: "", listPage: 0, itemPage: 0, itemScroll: 0 }); if (contexts.size > 12) contexts.delete(contexts.keys().next().value); }
    view = contexts.get(c.id); draw();
  }
  async function fromJourney({ scope, field, value, filters, from, to }) {
    const c = activeCase(); if (!c) { toast("Crie um Caso antes de guardar uma trilha.", "info"); return; }
    const artifact = state.currentArtifact ? { ...state.currentArtifact } : null, origin = state.currentOrigin;
    const sourceSpec = artifact ? sourceSpecFromArtifact(artifact) : sourceSpecFromControls();
    const signature = scope === "case" ? caseSig() : `${artifact?.id}:${artifact?.loadedAt}`;
    const current = () => activeCase() === c && workspaceScope() === scope && signature === (scope === "case" ? caseSig() : `${state.currentArtifact?.id}:${state.currentArtifact?.loadedAt}`);
    const queryFilters = [...structuredClone(filters || []), { column: field, op: "equals_exact", value, value2: null }];
    if (from != null && to != null) queryFilters.push({ column: "timestamp", op: "between", value: String(from), value2: String(to) });
    const overlay = el("div", "modal-overlay case-trail-convert"), panel = el("section", "modal"); panel.setAttribute("role", "dialog"); panel.setAttribute("aria-modal", "true"); panel.setAttribute("aria-label", "Guardar em trilha");
    const heading = el("div", "modal-head"), body = el("div", "modal-body"), message = el("p", "", "Preparando todos os registros desta possível trilha…"), footer = el("div", "modal-actions");
    let closed = false; const close = () => { closed = true; overlay.remove(); document.removeEventListener("workspace-context-change", close); };
    heading.append(el("h3", "", "Guardar em trilha"), icon("fa-xmark", "Fechar", close)); body.append(message); footer.append(button("Cancelar", close)); panel.append(heading, body, footer); overlay.append(panel); document.body.append(overlay); document.addEventListener("workspace-context-change", close); footer.querySelector("button").focus();
    overlay.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } };
    try {
      const itemIds = new Set(), pending = [], byKey = new Map(), ownerByIndex = new Map(), seen = new Set(); let index = 0, bytes = 0, offset = 0, total = 0;
      for (const item of c.items || []) for (const row of item.rows || []) {
        if (!Number.isInteger(row.id)) continue;
        const key = caseRecordKey(row, item.artifactId, item.origin); if (!byKey.has(key)) byKey.set(key, item.id);
        if (scope === "case" && (!state.stationAnalyticsId || item.stationId === state.stationAnalyticsId) && !seen.has(key)) { seen.add(key); ownerByIndex.set(index++, item.id); }
      }
      const caseRows = scope === "case" ? caseEvents() : null;
      do {
        const result = await api("query_events", { filters: queryFilters, ...(caseRows ? { caseEvents: caseRows } : {}), sortColumn: "timestamp", sortDir: "asc", offset, limit: 2000 }, { silent: true });
        if (closed) return; if (!current()) throw Error("O contexto mudou. Abra a possível trilha novamente.");
        total = result.total;
        if (total > 10000) throw Error(`Esta possível trilha tem ${fmtNum(total)} registros. Refine o período ou os filtros para no máximo 10.000 antes de guardá-la.`);
        if (!result.rows?.length && offset < total) throw Error("A leitura terminou antes de obter todos os registros. Tente novamente.");
        for (const row of result.rows || []) {
          const owner = scope === "case" ? ownerByIndex.get(row.id) : byKey.get(caseRecordKey(row, artifact?.id, origin));
          if (owner) itemIds.add(owner);
          else if (scope === "case") throw Error("Não foi possível localizar o item original. Atualize o Caso e tente novamente.");
          else { bytes += new TextEncoder().encode(JSON.stringify(row)).length; if (bytes > 32 * 1024 * 1024) throw Error("Os registros excedem 32 MB. Refine o período ou os filtros antes de guardar a trilha."); pending.push(row); }
        }
        offset += result.rows.length; message.textContent = `Preparando ${fmtNum(offset)} de ${fmtNum(total)} registros…`;
      } while (offset < total);
      if (!total) throw Error("Nenhum registro encontrado nesta possível trilha.");
      message.textContent = `${fmtNum(total)} registros completos · ${fmtNum(itemIds.size)} itens existentes${pending.length ? ` · ${fmtNum(pending.length)} registros novos serão preservados no Caso` : ""}.`;
      if (itemIds.size) body.append(el("p", "muted small", "Itens existentes serão reutilizados. Um item agrupado pode conter outros registros além desta possível trilha."));
      const label = el("label", "fld", "Trilha de destino"), select = el("select"); select.setAttribute("aria-label", "Trilha de destino"); const create = el("option", "", "Criar nova trilha"); create.value = ""; select.append(create);
      for (const trail of trails(c)) { const option = el("option", "", trail.title || "Trilha sem título"); option.value = trail.id; select.append(option); } label.append(select); body.append(label);
      async function commit(target, isNew) {
        if (!current() || !exists(c, isNew ? null : target)) throw Error("O Caso mudou. Reabra a possível trilha.");
        const previousItems = c.items, previousTrails = c.caseTrails, previousIds = target.itemIds, previousTime = target.updatedAt;
        let added;
        if (pending.length) { added = { ...caseItemBase("grupo", artifact?.stationId || null, total, pending.length), label: `${field}: ${value}`, rows: pending, total: pending.length, sourceFilters: structuredClone(queryFilters), sourceSpec: structuredClone(sourceSpec), summary: "", details: "", attachments: [] }; c.items = [...c.items, added]; }
        target.itemIds = [...new Set([...(target.itemIds || []), ...itemIds, ...(added ? [added.id] : [])])]; target.updatedAt = Date.now();
        if (isNew) c.caseTrails = [...trails(c), target];
        try { if (await notifyChanged(c) === false) throw Error("Não foi possível salvar a trilha."); }
        catch (error) { c.items = previousItems; c.caseTrails = previousTrails; target.itemIds = previousIds; target.updatedAt = previousTime; updateAnalysisBadge(); throw error; }
        return true;
      }
      const save = button("Continuar", async () => {
        if (!current()) { message.textContent = "O contexto mudou. Abra a possível trilha novamente."; return; }
        save.disabled = true;
        try {
          const existing = select.value ? trails(c).find(trail => trail.id === select.value) : null;
          if (select.value && !existing) throw Error("A trilha escolhida não existe mais.");
          const target = existing || { id: `ct-${nid()}`, title: `${field}: ${value}`.slice(0, 200), summary: "", details: "", attachments: [], itemIds: [], createdAt: Date.now(), updatedAt: Date.now() };
          let saved;
          if (existing) saved = await commit(target, false);
          else { close(); saved = await CaseContent.edit(target, { title: "Nova trilha", nameField: "title", onSave: () => commit(target, true) }); }
          if (!saved) return;
          close(); contexts.set(c.id, { selected: target.id, search: "", listPage: 0, itemPage: 0, itemScroll: 0 });
          if (workspaceScope() !== "case") await WorkspaceContext.setScope("case", { page: "case-trails" }); else await Workspace.showPage("case-trails");
          toast("Possível trilha guardada no Caso.", "ok");
        } catch (error) { message.textContent = String(error.message || error); save.disabled = false; }
      }, "btn primary"); footer.append(save); select.focus();
    } catch (error) { if (!closed) message.textContent = String(error.message || error); }
  }
  const nav = button("", () => Workspace.showPage("case-trails"), ""); nav.dataset.page = "case-trails"; nav.innerHTML = '<i class="fas fa-route" aria-hidden="true"></i>Trilhas'; document.querySelector('.nav-pages [data-page="evidence"]').before(nav);
  const updateNav = () => { nav.hidden = workspaceScope() !== "case"; }; document.addEventListener("workspace-context-change", () => { generation++; imageObserver?.disconnect(); updateNav(); document.querySelector(".case-trail-picker")?.remove(); }); updateNav();
  return { render, fromJourney, refresh: () => { if (active()) render(host.parentElement, activeCase()); } };
})();
