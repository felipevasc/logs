/* Native historical display entries are complete-plan search results. They are
   presentation DTOs; exact preserved members are expanded in separate pages. */
window.CaseEvidenceDisplay = (() => {
  "use strict";
  const scopes = new WeakMap(), hosts = new WeakMap(), queries = new WeakMap();
  const clone = value => structuredClone(value), key = value => JSON.stringify(value), integer = value => Number.isSafeInteger(value) && value >= 0;
  const digest = value => typeof value === "string" && /^[0-9a-f]{64}$/.test(value), text = (value, maximum) => typeof value === "string" && window.CaseEvidence.validate.utf8Within(value, maximum);
  const same = (a, b) => a?.storeId === b?.storeId && a?.epoch === b?.epoch && a?.revision === b?.revision;
  const authored = c => ({ timeline: c.timeline || {}, manual: c.manual || [] });
  const fail = () => { throw Error("CASE_DISPLAY_INVALID: A página não corresponde à cronologia preservada capturada."); };
  const stale = () => Error("CASE_DISPLAY_CHANGED: A investigação ou a busca mudou. Reabra a cronologia.");
  const stamp = ref => key([ref.kind, ref.owner, ref.containerId, ref.manifestId, ref.manifestSha256, ref.memberCount, ref.token ?? null]);
  const keys = (value, allowed) => { if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some(name => !allowed.includes(name))) fail(); };
  const preview = value => { keys(value, ["text", "complete"]); if (!text(value.text, 4096) || typeof value.complete !== "boolean") fail(); };
  const time = value => value === null || typeof value === "number" && Number.isFinite(value) && Math.abs(value) <= 8640000000000000;
  const rangeTime = value => value === null || time(value) && Number.isSafeInteger(value);
  const cursor = value => value === null || text(value, 4096) && !!value;
  function create({ client, getStore, currentCase, invoke }) {
    function capture({ item = currentCase(), filters = [], stationId = null, fromMs = null, toMs = null, includeUntimed = true, displayQuery = "", isCurrent = () => true } = {}) {
      const store = getStore(), live = currentCase(), summary = store?.caseEvidence?.find(entry => entry.state === "ready" && entry.owner.caseId === item?.id);
      if (!summary || live?.id !== item.id || item.kind === "preserved_case_unavailable" || window.CaseEvidence.analysisShape(item) !== client.analysisShape(item.id)) throw stale();
      if (typeof displayQuery !== "string" || displayQuery.length > 200 || !Array.isArray(filters) || filters.length > 200 || !rangeTime(fromMs) || !rangeTime(toMs) || fromMs !== null && toMs !== null && fromMs > toMs || typeof includeUntimed !== "boolean" || stationId !== null && !text(stationId, 4096)) fail();
      for (const filter of filters) if (!filter || !text(filter.column, 1024) || !filter.column || !text(filter.op, 32) || !filter.op || filter.value != null && !text(filter.value, 1000000) || filter.value2 != null && !text(filter.value2, 1000000)) fail();
      const value = clone(authored(item)), authoredViewJson = JSON.stringify(value); if (!text(authoredViewJson, 1048576)) fail();
      const request = { store: clone(store.store), owner: clone(summary.owner), evidenceSignature: summary.evidenceSignature, authoredViewJson, stationId,
        filters: filters.map(filter => ({ column: filter.column, op: filter.op, value: filter.value ?? "", value2: filter.value2 ?? null })), fromMs, toMs, includeUntimed, displayQuery };
      if (!text(JSON.stringify({ ...request, cursor: null }), 2097152)) fail();
      const captured = { store, item, live, authored: value, request, preservedCount: summary.preservedCount, authoredViewSignature: null, displaySignature: null, entries: new Map(),
        locations: new Map((item.items || []).flatMap((entry, index) => entry.rows?.kind === "native_evidence_container" ? [[entry.rows.reference.containerId, { index, reference: clone(entry.rows.reference) }]] : [])) };
      captured.current = () => getStore() === store && currentCase() === live && same(store.store, request.store) && isCurrent()
        && key(authored(item)) === authoredViewJson && window.CaseEvidence.analysisShape(live) === client.analysisShape(item.id)
        && store.caseEvidence.some(entry => entry.state === "ready" && key(entry.owner) === key(request.owner) && entry.evidenceSignature === request.evidenceSignature);
      if (!captured.current()) throw stale(); const ticket = Object.freeze({ kind: "native_case_display_scope" }); scopes.set(ticket, captured); return ticket;
    }
    async function page(ticket, { cursor: next = null } = {}) {
      const captured = scopes.get(ticket); if (!captured?.current()) throw stale(); if (!cursor(next)) fail();
      const result = await invoke("case_evidence_display_timeline", { request: { ...clone(captured.request), cursor: next } });
      if (!captured.current()) throw stale();
      keys(result, ["kind", "owner", "evidenceSignature", "authoredViewSignature", "displaySignature", "preservedCount", "scopeOccurrenceCount", "unavailableCount", "displayEntryCount", "matchingDisplayEntryCount", "entries", "nextCursor"]);
      if (result.kind !== "native_case_display_page" || key(result.owner) !== key(captured.request.owner) || result.evidenceSignature !== captured.request.evidenceSignature
        || !digest(result.authoredViewSignature) || !digest(result.displaySignature) || captured.authoredViewSignature && result.authoredViewSignature !== captured.authoredViewSignature || captured.displaySignature && result.displaySignature !== captured.displaySignature
        || result.preservedCount !== captured.preservedCount || !integer(result.scopeOccurrenceCount) || result.scopeOccurrenceCount > result.preservedCount || !integer(result.unavailableCount) || result.unavailableCount > result.preservedCount || !integer(result.displayEntryCount) || !integer(result.matchingDisplayEntryCount)
        || result.matchingDisplayEntryCount > result.displayEntryCount || !Array.isArray(result.entries) || result.entries.length > 128 || !cursor(result.nextCursor) || result.nextCursor && (!result.entries.length || result.nextCursor === next)) fail();
      const counts = key([result.preservedCount, result.scopeOccurrenceCount, result.unavailableCount, result.displayEntryCount, result.matchingDisplayEntryCount]);
      if (captured.counts && captured.counts !== counts) fail();
      const ids = new Set();
      for (const entry of result.entries) {
        keys(entry, ["entryId", "type", "startMs", "endMs", "timing", "title", "detail", "source", "memberCount", "memberToken", "firstMemberEntryId", "firstMemberEditKey", "itemIndex", "groupIndex", "manualIndex", "noteIndices"]);
        if (!text(entry.entryId, 4096) || !entry.entryId || ids.has(entry.entryId) || !["event", "auto", "group", "manual"].includes(entry.type) || !time(entry.startMs) || !time(entry.endMs)
          || !["timed", "untimed", "unavailable"].includes(entry.timing) || (entry.timing === "timed" ? entry.startMs === null || entry.endMs === null || entry.endMs < entry.startMs : entry.startMs !== null || entry.endMs !== null)
          || (entry.type === "manual" ? entry.firstMemberEntryId !== null : !text(entry.firstMemberEntryId, 4096) || !entry.firstMemberEntryId.startsWith("n:"))
          || entry.firstMemberEditKey !== null && (!text(entry.firstMemberEditKey, 4096) || !Object.hasOwn(captured.authored.timeline.edits || {}, entry.firstMemberEditKey))
          || !integer(entry.memberCount) || (entry.type === "manual" ? entry.memberCount !== 0 || entry.memberToken !== null : !entry.memberCount || !text(entry.memberToken, 4096) || !entry.memberToken)
          || entry.itemIndex !== null && (!integer(entry.itemIndex) || !captured.item.items?.[entry.itemIndex]) || entry.groupIndex !== null && (!integer(entry.groupIndex) || !captured.authored.timeline.groups?.[entry.groupIndex])
          || entry.manualIndex !== null && (!integer(entry.manualIndex) || !captured.authored.manual[entry.manualIndex]) || entry.type === "manual" && entry.manualIndex === null
          || !Array.isArray(entry.noteIndices) || new Set(entry.noteIndices).size !== entry.noteIndices.length || entry.noteIndices.some(index => !integer(index) || !captured.authored.timeline.annotations?.[index])) fail();
        for (const field of ["title", "detail", "source"]) preview(entry[field]); ids.add(entry.entryId);
      }
      if (!text(JSON.stringify(result), 1048576)) fail(); captured.authoredViewSignature ||= result.authoredViewSignature; captured.displaySignature ||= result.displaySignature; captured.counts ||= counts;
      // Only the currently published display page can issue new expansions.
      captured.entries = new Map(result.entries.map(entry => [entry.entryId, clone(entry)])); return clone(result);
    }
    async function members(ticket, entry, { cursor: next = null } = {}) {
      const captured = scopes.get(ticket), admitted = captured?.entries.get(entry.entryId);
      if (!captured?.current() || !admitted || admitted.memberToken !== entry.memberToken || !admitted.memberToken) throw stale(); if (!cursor(next)) fail();
      const result = await invoke("case_evidence_display_members", { request: { scope: { ...clone(captured.request), cursor: null }, displaySignature: captured.displaySignature, memberToken: admitted.memberToken, cursor: next } });
      if (!captured.current() || captured.entries.get(entry.entryId)?.memberToken !== admitted.memberToken) throw stale();
      keys(result, ["kind", "displaySignature", "entryId", "memberCount", "members", "nextCursor"]);
      if (result.kind !== "native_case_display_members" || result.displaySignature !== captured.displaySignature || result.entryId !== admitted.entryId || result.memberCount !== admitted.memberCount
        || !Array.isArray(result.members) || result.members.length > 128 || !cursor(result.nextCursor) || result.nextCursor && (!result.members.length || result.nextCursor === next)) fail();
      for (const value of result.members) { keys(value, ["reference", "member", "itemIndex"]); window.CaseEvidence.validate.reference(value.reference, captured.request.owner); window.CaseEvidence.validate.member(value.member, value.reference);
        const location = captured.locations.get(value.reference.containerId); if (!location || value.itemIndex !== location.index || stamp(value.reference) !== stamp(location.reference)) fail(); }
      if (!text(JSON.stringify(result), 1048576)) fail(); return clone(result);
    }
    return { capture, page, members, current: ticket => scopes.get(ticket)?.current() === true, notes: (ticket, entry) => entry.noteIndices.map(index => scopes.get(ticket).authored.timeline.annotations[index]) };
  }
  function reader() { const services = nativeEvidenceServices(); return services.display ||= create({ client: services.client, getStore: () => state.cases, currentCase: activeCase, invoke: (command, args) => api(command, args, { silent: true, latest: `preserved-display:${command}` }) }); }
  const button = (label, action) => { const node = el("button", "btn ghost small", label); node.type = "button"; node.onclick = action; return node; };
  const shown = value => `${value.text}${value.complete ? "" : "… [prévia]"}`;
  async function expand(history, ticket, entry) {
    const focus = document.activeElement, overlay = el("div", "modal-overlay"), modal = el("section", "modal"), head = el("header", "modal-head"), body = el("div", "modal-body");
    modal.setAttribute("role", "dialog"); modal.setAttribute("aria-modal", "true"); modal.setAttribute("aria-label", "Ocorrências preservadas do item da cronologia");
    let closed = false, serial = 0; const cursors = [null], offsets = [0], close = () => { closed = true; ++serial; overlay.remove(); if (focus?.isConnected) focus.focus(); };
    head.append(el("h3", "", shown(entry.title)), button("Fechar", close)); modal.append(head, body); overlay.append(modal); document.body.append(overlay);
    overlay.onclick = event => { if (event.target === overlay) close(); }; overlay.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } };
    async function paint(index) {
      const request = ++serial; body.textContent = "Lendo referências das ocorrências preservadas…";
      try { const result = await history.members(ticket, entry, { cursor: cursors[index] }); if (closed || request !== serial || !history.current(ticket)) return;
        const end = offsets[index] + result.members.length; if (end > result.memberCount || !result.nextCursor && end !== result.memberCount || result.nextCursor && cursors.slice(0, index + 1).includes(result.nextCursor)) fail();
        cursors[index + 1] = result.nextCursor; offsets[index + 1] = end;
        if (entry.memberCount === 1 && index === 0 && result.members.length === 1 && !result.nextCursor) { const value = result.members[0]; close(); return window.CaseEvidenceDetail.open(value.reference, value.member, { guard: () => history.current(ticket) }); }
        body.replaceChildren();
        for (const [position, value] of result.members.entries()) body.append(button(`Ocorrência ${offsets[index] + position + 1} · item ${value.itemIndex + 1}`, () => { if (!history.current(ticket)) return; close(); return window.CaseEvidenceDetail.open(value.reference, value.member, { guard: () => history.current(ticket) }); }));
        const previous = button("Anteriores", () => paint(index - 1)), next = button("Próximas", () => paint(index + 1)); previous.disabled = index === 0; next.disabled = !result.nextCursor;
        const navigation = el("div", "case-trails-pager"); navigation.append(previous, el("span", "", `${offsets[index] + (result.members.length ? 1 : 0)}–${end} de ${result.memberCount}`), next); body.append(navigation);
      } catch (error) { if (!closed && request === serial) body.textContent = `Não foi possível ler as ocorrências: ${String(error.message || error)}`; }
    }
    await paint(0);
  }
  function showNotes(notes, edit) {
    const focus = document.activeElement, overlay = el("div", "modal-overlay"), modal = el("section", "modal"), body = el("div", "modal-body"), head = el("header", "modal-head");
    const close = () => { overlay.remove(); if (focus?.isConnected) focus.focus(); };
    modal.setAttribute("role", "dialog"); modal.setAttribute("aria-modal", "true"); modal.setAttribute("aria-label", "Notas preservadas da cronologia");
    head.append(el("h3", "", "Notas da cronologia"), button("Fechar", close)); modal.append(head, body); overlay.append(modal); document.body.append(overlay);
    overlay.onclick = event => { if (event.target === overlay) close(); }; overlay.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } };
    function paint(index) {
      body.replaceChildren();
      for (const note of notes.slice(index * 32, (index + 1) * 32)) { const row = el("section"); row.append(el("p", "", note.text || "Ícone"));
        if (note.anchor || note.links?.length) row.append(el("p", "muted small", "Os vínculos internos a grupos ou fora da página permanecem preservados; a posição deles não é desenhada aqui."));
        if (edit) row.append(button("Editar nota", () => { close(); edit(note); })); body.append(row); }
      const previous = button("Notas anteriores", () => paint(index - 1)), next = button("Próximas notas", () => paint(index + 1)); previous.disabled = !index; next.disabled = (index + 1) * 32 >= notes.length;
      body.append(previous, el("span", "", `${index * 32 + (notes.length ? 1 : 0)}–${Math.min((index + 1) * 32, notes.length)} de ${notes.length}`), next);
    }
    paint(0);
  }
  const COLORS = ["#76c9b5", "#7daef2", "#d3a9fa", "#f4bb73", "#f08e91", "#a7b9cf"];
  function noteResolver(notes, anchors) {
    const first = new Map(), memo = new Map();
    for (const note of notes) if (!first.has(note.id)) first.set(note.id, note);
    return target => {
      const chain = [], seen = new Set(); let value = null, id = target;
      while (id) {
        if (anchors.has(id)) { value = anchors.get(id); break; }
        if (memo.has(id)) { value = memo.get(id); break; }
        if (seen.has(id)) break;
        seen.add(id); chain.push(id); const note = first.get(id); if (!note) break; id = note.anchor;
      }
      for (const key of chain) memo.set(key, value);
      return value;
    };
  }
  function collectGraph(c, page) {
    const config = c.timeline, entries = page.entries.filter(value => value.timing === "timed").map(value => {
      const item = c.items?.[value.itemIndex], manual = c.manual?.[value.manualIndex], group = config.groups?.[value.groupIndex];
      const color = manual?.color || group?.color || config.edits?.[value.firstMemberEditKey]?.color || item?.timelineColor || COLORS[(value.itemIndex ?? 3) % COLORS.length];
      return { id: value.entryId, type: value.type, start: value.startMs, end: value.endMs, title: shown(value.title), detail: shown(value.detail), source: shown(value.source), color,
        editTitle: manual?.name ?? group?.name ?? config.edits?.[value.firstMemberEditKey]?.title ?? (value.title.complete ? value.title.text : ""),
        rows: [], recordCount: value.memberCount, firstMemberEntryId: value.firstMemberEntryId, nativeEntry: value, manual };
    });
    const visible = new Set(entries.map(entry => entry.id)), annotations = config.annotations || [];
    const resolves = noteResolver(annotations, new Map([...visible].map(id => [id, true])));
    const availableAnnotations = annotations.filter(note => resolves(note.anchor)), visibleAnnotations = availableAnnotations.slice(0, 128);
    const drawnTargets = new Set([...visible, ...visibleAnnotations.map(note => note.id)]);
    const omittedRelationships = annotations.reduce((count, note) => count + (!note.arrowHidden && note.anchor && (!drawnTargets.has(note.id) || !drawnTargets.has(note.anchor)) ? 1 : 0)
      + (note.links || []).filter(link => link.targetId && (!drawnTargets.has(note.id) || !drawnTargets.has(link.targetId))).length, 0);
    return { config, entries, visibleAnnotations, omittedRelationships, omittedAnnotations: annotations.length - visibleAnnotations.length, targetVisible: id => drawnTargets.has(id) };
  }
  async function actionMembers(history, ticket, entry, c) {
    const value = entry.nativeEntry;
    if (!value || value.memberCount > 10000) throw Error("Esta ação admite até 10.000 referências. Abra as ocorrências para trabalhar em partes.");
    const slots = [], cursors = new Set(); let next = null, bytes = 0;
    do { const page = await history.members(ticket, value, { cursor: next });
      bytes += new TextEncoder().encode(JSON.stringify(page.members)).length;
      if (bytes > 8388608 || slots.length + page.members.length > value.memberCount) throw Error("As referências excedem o limite desta ação; nenhuma ocorrência foi alterada.");
      slots.push(...page.members); next = page.nextCursor;
      if (next && cursors.has(next)) fail(); if (next) cursors.add(next);
    } while (next);
    if (!history.current(ticket) || slots.length !== value.memberCount) throw stale();
    const unique = new Map();
    for (const slot of slots) { const id = `n:${slot.member.containerId}:${slot.member.occurrenceId}`;
      if (!unique.has(id)) unique.set(id, { id, type: "event", title: entry.editTitle || "Sequência", color: entry.color, occurrence: { item: c.items[slot.itemIndex], itemIndex: slot.itemIndex, member: slot.member },
        rows: [{ kind: "preserved_member", reference: slot.reference, member: slot.member }] }); }
    return [...unique.values()];
  }
  async function renderGraph(box, c, mode, callbacks) {
    const version = {}, pageName = document.body.dataset.page, analysisView = state.analysisView, stationId = state.stationAnalyticsId || null, filters = clone(backendFilters()); hosts.set(box, version);
    const guard = () => hosts.get(box) === version && box.isConnected && activeCase() === c && document.body.dataset.page === pageName && state.analysisView === analysisView && (state.stationAnalyticsId || null) === stationId && key(backendFilters()) === key(filters);
    box.textContent = "Lendo os resumos completos da cronologia preservada…";
    try {
      const config = c.timeline ||= {};
      if (typeof config !== "object" || Array.isArray(config) || ["groups", "annotations"].some(name => config[name] != null && !Array.isArray(config[name])) || ["edits", "layout"].some(name => config[name] != null && (typeof config[name] !== "object" || Array.isArray(config[name])))) throw Error("A configuração histórica está indisponível e permanece preservada.");
      config.groups ||= []; config.annotations ||= []; config.edits ||= {}; config.layout ||= {};
      const services = nativeEvidenceServices(); await services.session.pending(); if (!guard()) return;
      if (services.session.status() !== "ready") throw Error("Confirme o salvamento pendente antes de consultar a cronologia.");
      const history = reader(), ticket = history.capture({ item: c, stationId, filters, includeUntimed: false, displayQuery: "", isCurrent: guard });
      const cursors = [null], offsets = [0]; let serial = 0;
      async function paint(index) {
        const request = ++serial, result = await history.page(ticket, { cursor: cursors[index] }); if (!guard() || request !== serial) return;
        const end = offsets[index] + result.entries.length;
        if (end > result.matchingDisplayEntryCount || !result.nextCursor && end !== result.matchingDisplayEntryCount || result.nextCursor && cursors.slice(0, index + 1).includes(result.nextCursor)) fail();
        cursors[index + 1] = result.nextCursor; offsets[index + 1] = end;
        const tools = el("div", "source-controls"), body = el("div"), previous = button("Anteriores", () => navigate(index - 1)), next = button("Próximas", () => navigate(index + 1));
        previous.disabled = index === 0; next.disabled = !result.nextCursor;
        const collected = collectGraph(c, result);
        tools.append(previous, el("span", "muted small", `${offsets[index] + (result.entries.length ? 1 : 0)}–${end} de ${fmtNum(result.matchingDisplayEntryCount)} entradas · intervalos e contagens completos de cada grupo`), next);
        tools.append(el("span", "muted small", "Abra um grupo para navegar pelas referências preservadas. Marcadores de membros internos não são desenhados no resumo."));
        if (collected.omittedAnnotations) tools.append(el("span", "muted small", `${collected.omittedAnnotations} notas acessíveis no painel Notas; até 128 notas posicionadas por página.`));
        if (collected.omittedRelationships) tools.append(el("span", "muted small", `${collected.omittedRelationships} vínculos internos ou fora desta página preservados sem seta desenhada. Consulte as notas.`));
        if (result.unavailableCount) tools.append(el("span", "muted small", `${fmtNum(result.unavailableCount)} ocorrências com horário indisponível; consulte a tabela.`));
        let editNote; if (config.annotations.length) tools.append(button(`Notas (${config.annotations.length})`, () => showNotes(config.annotations, note => history.current(ticket) && editNote?.(note))));
        box.replaceChildren(tools, body);
        window.CaseTimeline.render(body, c, mode, { ...callbacks, collected, visibleAnnotations: collected.visibleAnnotations, targetVisible: collected.targetVisible,
          pageLabel: `Entradas ${offsets[index] + (result.entries.length ? 1 : 0)}–${end} de ${fmtNum(result.matchingDisplayEntryCount)}; grupos com intervalos e contagens completos; ${collected.omittedRelationships} vínculos sem posição nesta página`,
          nativeCurrent: () => history.current(ticket), members: entry => actionMembers(history, ticket, entry, c),
          openEntry: entry => history.current(ticket) && expand(history, ticket, entry.nativeEntry), openEntries: (entries, x, y) => history.current(ticket) && callbacks.menu(x, y, entries.map(entry => ({ icon: "fa-eye", label: `${entry.title} · ${entry.recordCount} registros`, onClick: () => history.current(ticket) && expand(history, ticket, entry.nativeEntry) }))), installNoteEditor: edit => { editNote = edit; } });
        async function navigate(target) { previous.disabled = next.disabled = true; body.inert = true;
          try { await paint(target); } catch (error) { if (guard()) { callbacks.notify?.(String(error.message || error)); previous.disabled = !index; next.disabled = !result.nextCursor; body.inert = false; } } }
      }
      await paint(0);
    } catch (error) { if (guard()) { box.textContent = `A cronologia preservada está indisponível: ${String(error.message || error)}`; box.append(button("Tentar novamente", () => renderGraph(box, c, mode, callbacks))); } }
  }
  async function render(box, c) {
    const version = {}, pageName = document.body.dataset.page, analysisView = state.analysisView, stationId = state.stationAnalyticsId || null, filters = clone(backendFilters()); hosts.set(box, version);
    const guard = () => hosts.get(box) === version && box.isConnected && activeCase() === c && document.body.dataset.page === pageName && state.analysisView === analysisView && (state.stationAnalyticsId || null) === stationId && key(backendFilters()) === key(filters);
    box.classList.add("case-timeline-host"); box.replaceChildren();
    const shell = el("section", "ct-table-shell"), tools = el("div", "ct-table-tools"), label = el("label", "ct-table-search"), search = el("input"), report = button("Relatório PDF", () => window.CaseReport.open()), status = el("p", "muted small"), scroll = el("div", "ct-table-scroll"), table = el("table", "ct-data-table"), head = el("thead"), headings = el("tr"), body = el("tbody"), navigation = el("div", "ct-table-pager"), previous = button("Anterior", () => paint(pageIndex - 1)), next = button("Próxima", () => paint(pageIndex + 1));
    search.type = "search"; search.maxLength = 200; search.placeholder = "Buscar ocorrências ou notas no Caso"; search.setAttribute("aria-label", "Buscar na cronologia completa do Caso"); search.value = queries.get(c) || ""; label.append(search); tools.append(label, report);
    for (const title of ["Horário local", "Ocorrência", "Origem", "Registros", "Notas"]) headings.append(el("th", "", title)); head.append(headings); table.append(head, body); scroll.append(table); navigation.append(previous, status, next); shell.append(tools, scroll, navigation); box.append(shell);
    let ticket = null, pageIndex = 0, serial = 0, queryGeneration = 0, debounce; const cursors = [null], offsets = [0], history = reader();
    const moment = value => value === null ? "Sem horário" : new Date(value).toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit", fractionalSecondDigits: 3 });
    async function paint(index) {
      const request = ++serial; previous.disabled = next.disabled = true; body.inert = true; status.textContent = "Buscando na cronologia completa…";
      try {
        await nativeEvidenceServices().session.pending(); if (!guard() || request !== serial) return;
        if (nativeEvidenceServices().session.status() !== "ready") throw Error("Confirme o salvamento pendente antes de consultar a cronologia.");
        if (!ticket) { const query = search.value, queryVersion = queryGeneration; ticket = history.capture({ item: c, stationId, filters, displayQuery: query, includeUntimed: true, isCurrent: () => guard() && search.value === query && queryGeneration === queryVersion }); }
        const captured = ticket, result = await history.page(captured, { cursor: cursors[index] }); if (!guard() || request !== serial || ticket !== captured) return;
        const end = offsets[index] + result.entries.length; if (end > result.matchingDisplayEntryCount || !result.nextCursor && end !== result.matchingDisplayEntryCount || result.nextCursor && cursors.slice(0, index + 1).includes(result.nextCursor)) fail();
        pageIndex = index; cursors[index + 1] = result.nextCursor; offsets[index + 1] = end; body.replaceChildren(); body.inert = false;
        for (const entry of result.entries) { const row = el("tr"), main = el("td"), title = entry.memberCount ? button(shown(entry.title), () => expand(history, captured, entry)) : el("strong", "", shown(entry.title)); row.dataset.id = entry.entryId; title.className += " ct-table-title"; main.append(title);
          if (entry.detail.text) { const details = el("details"), summary = el("summary", "", "Detalhes"); details.append(summary, el("p", "", shown(entry.detail))); main.append(details); }
          const notes = el("td"); if (entry.noteIndices.length) notes.append(button(`${entry.noteIndices.length} notas`, () => showNotes(history.notes(captured, entry))));
          row.append(el("td", "", `${entry.timing === "unavailable" ? "Horário indisponível" : moment(entry.startMs)}${entry.endMs !== entry.startMs ? ` até ${moment(entry.endMs)}` : ""}`), main, el("td", "", shown(entry.source)), el("td", "", entry.memberCount ? fmtNum(entry.memberCount) : "Marco manual"), notes); body.append(row); }
        status.textContent = `${offsets[index] + (result.entries.length ? 1 : 0)}–${end} de ${fmtNum(result.matchingDisplayEntryCount)} entradas correspondentes · ${fmtNum(result.displayEntryCount)} entradas no recorte · ${fmtNum(result.scopeOccurrenceCount)} ocorrências preservadas no recorte`;
        previous.disabled = index === 0; next.disabled = !result.nextCursor;
      } catch (error) { if (guard() && request === serial) { body.replaceChildren(); status.textContent = `A busca completa está indisponível: ${String(error.message || error)}`; } }
    }
    search.oninput = () => { body.inert = true; status.textContent = "Buscando na cronologia completa…"; window.Tasks?.cancelLatest("preserved-display:case_evidence_display_timeline"); queries.set(c, search.value); ++serial; ++queryGeneration; clearTimeout(debounce); ticket = null; cursors.splice(0, cursors.length, null); offsets.splice(0, offsets.length, 0); pageIndex = 0; debounce = setTimeout(() => paint(0), 180); };
    await paint(0);
  }
  return { create, render, renderGraph, collectGraph, actionMembers, expand };
})();
