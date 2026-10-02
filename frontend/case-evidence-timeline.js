/* Historical chronology uses paged previews and member handles, never Events or
   the current Dataset. The existing Timeline owns authored notes and layout. */
window.CaseEvidenceTimeline = (() => {
  "use strict";
  const views = new WeakMap(), COLORS = ["#76c9b5", "#7daef2", "#d3a9fa", "#f4bb73", "#f08e91", "#a7b9cf"];
  const manualTime = value => typeof value === "number" && Number.isFinite(value) && Math.abs(value) <= 8640000000000000;
  const timed = value => Number.isSafeInteger(value) && Math.abs(value) <= 8640000000000000;
  function aliases(c) {
    const result = new Set(), config = c.timeline || {};
    const add = value => { if (typeof value === "string" && /^(?:a:)?e:/.test(value)) result.add(value); };
    Object.keys(config.edits || {}).forEach(add); Object.keys(config.layout || {}).forEach(add);
    for (const group of config.groups || []) (group.ids || []).forEach(add);
    for (const note of config.annotations || []) { add(note.anchor); for (const link of note.links || []) add(link.targetId); }
    const values = [...result]; if (values.length > 10000 || new TextEncoder().encode(JSON.stringify(values)).length > 1048576) throw Error("Os vínculos históricos excedem o limite de leitura desta visualização.");
    return values;
  }
  function collect(c, page, { includeUntimed = false, complete = false } = {}) {
    const config = c.timeline ||= { groups: [], annotations: [], edits: {}, layout: {} };
    config.groups ||= []; config.annotations ||= []; config.edits ||= {}; config.layout ||= {};
    const aliases = new Map((page.aliases || []).filter(value => value.state === "unique").map(value => [value.alias, value.entryId])), reverse = new Map();
    for (const [alias, id] of aliases) { if (!reverse.has(id)) reverse.set(id, []); reverse.get(id).push(alias); }
    const display = value => window.EvidenceUI?.redact(value) ?? value;
    const events = page.entries.filter(entry => includeUntimed || entry.timing === "timed").map(value => {
      const item = c.items[value.itemIndex], reference = item.rows.reference, names = reverse.get(value.entryId) || [];
      const edit = config.edits[value.entryId] || names.map(alias => config.edits[alias]).find(Boolean) || {};
      const handle = Object.freeze({ kind: "preserved_member", reference: structuredClone(reference), member: structuredClone(value.member), title: value.title.text });
      return { id: value.entryId, itemId: value.containerId, itemRef: { kind: "container", containerId: value.containerId }, aliases: names,
        occurrence: { item, itemIndex: value.itemIndex, member: handle.member }, type: "event", start: value.timestampMs, end: value.timestampMs,
        title: display(edit.title || value.title.text), detail: display(value.detail.text), source: display(value.source.text),
        previewComplete: value.title.complete && value.detail.complete && value.source.complete, timing: value.timing,
        groupingKey: value.groupingUsesTitle && edit.title ? `authored:${edit.title}` : `native:${value.groupingKey}`,
        color: edit.color || item.timelineColor || COLORS[value.itemIndex % COLORS.length], rows: [handle] };
    });
    const byId = new Map(events.map(entry => [entry.id, entry])); for (const [alias, id] of aliases) if (byId.has(id)) byId.set(alias, byId.get(id));
    const consumed = new Set(), entries = [];
    for (const group of config.groups) {
      const members = (group.ids || []).map(id => byId.get(id)).filter(Boolean);
      // A group crossing a page stays as individual occurrences until all its
      // members are visible, without rewriting the authored group.
      if (members.length < 2 || !complete && members.length !== (group.ids || []).length || members.some(entry => !timed(entry.start))) continue;
      members.forEach(entry => consumed.add(entry.id)); entries.push({ id: group.id, type: "group", start: Math.min(...members.map(entry => entry.start)), end: Math.max(...members.map(entry => entry.end)),
        title: group.name || "Sequência", detail: `${members.length} ocorrências`, source: members[0].source, color: group.color || members[0].color, rows: members.flatMap(entry => entry.rows), members });
    }
    const rest = events.filter(entry => !consumed.has(entry.id));
    for (let index = 0; index < rest.length;) {
      const first = rest[index]; let end = index + 1;
      while (complete && config.compact !== false && timed(first.start) && end < rest.length && end - index < 250 && timed(rest[end].start)
        && rest[end].start - rest[end - 1].start <= 300000 && rest[end].itemId === first.itemId && rest[end].groupingKey === first.groupingKey) end++;
      if (end - index > 1) { const members = rest.slice(index, end); entries.push({ id: `a:${first.id}`, type: "auto", start: first.start, end: members.at(-1).end, title: first.title, detail: `${members.length} ocorrências na sequência`, source: first.source, color: first.color, rows: members.flatMap(entry => entry.rows), members }); }
      else entries.push(first); index = end;
    }
    for (const manual of c.manual || []) if (manualTime(manual.start)) entries.push({ id: `m:${manual.id}`, type: "manual", start: manual.start, end: manualTime(manual.end) && manual.end > manual.start ? manual.end : manual.start,
      title: manual.name || "Marco", detail: manual.description || "", source: "Marco do caso", color: manual.color || COLORS[3], rows: [], manual });
    entries.sort((a, b) => (a.start ?? Infinity) - (b.start ?? Infinity) || (a.end ?? Infinity) - (b.end ?? Infinity));
    return { config, entries, undated: page.untimedCount + page.unavailableCount, aliases: page.aliases || [], incompletePreviews: events.filter(entry => !entry.previewComplete).length };
  }
  function reader() {
    const services = nativeEvidenceServices();
    return services.history ||= window.CaseEvidencePreserved.create({ client: services.client, getStore: () => state.cases, currentCase: activeCase, invoke: (command, args) => api(command, args, { silent: true }) });
  }
  async function open(c, { filters = [], stationId = null, guard = () => true } = {}) {
    const services = nativeEvidenceServices(); await services.session.pending();
    if (services.session.status() !== "ready" || activeCase()?.id !== c.id || window.CaseEvidence.analysisShape(activeCase()) !== window.CaseEvidence.analysisShape(c) || !guard()) throw Error("Confirme o salvamento pendente antes de abrir a cronologia preservada.");
    const authoredViewJson = JSON.stringify({ timeline: c.timeline || {}, manual: c.manual || [] }), previousGuard = guard;
    guard = () => previousGuard() && JSON.stringify({ timeline: c.timeline || {}, manual: c.manual || [] }) === authoredViewJson;
    const history = reader(), values = aliases(c), chunks = values.length ? Array.from({ length: Math.ceil(values.length / 256) }, (_, i) => values.slice(i * 256, (i + 1) * 256)) : [[]];
    const ticket = history.captureHistory({ stationId, filters, authoredViewJson, aliases: chunks[0], includeUntimed: true, isCurrent: guard });
    const first = await history.timeline(ticket), resolved = first.aliases.slice();
    for (const chunk of chunks.slice(1)) { const extra = history.captureHistory({ stationId, filters, authoredViewJson, aliases: chunk, includeUntimed: true, isCurrent: () => guard() && history.current(ticket) }); const page = await history.timeline(extra); if (page.authoredViewSignature !== first.authoredViewSignature) throw Error("A autoria da cronologia mudou entre páginas."); resolved.push(...page.aliases); }
    const summary = page => JSON.stringify([page.owner, page.evidenceSignature, page.authoredViewSignature, page.preservedCount, page.scopeCount, page.timedCount, page.untimedCount, page.unavailableCount, page.minTimestampMs, page.maxTimestampMs]);
    const read = async cursor => { if (!guard() || !history.current(ticket)) throw Error("A cronologia capturada mudou. Reabra a visualização."); const page = cursor === null ? first : await history.timeline(ticket, { cursor });
      if (!guard() || !history.current(ticket) || summary(page) !== summary(first) || page.nextCursor && (!page.entries.length || page.nextCursor === cursor)) throw Error("A página não corresponde à cronologia capturada.");
      return { ...page, aliases: resolved }; };
    return { read, current: () => guard() && history.current(ticket) };
  }
  async function render(box, c, mode, callbacks) {
    const config = c.timeline ||= {}; config.groups ||= []; config.annotations ||= []; config.edits ||= {}; config.layout ||= {};
    const version = {}, pageName = document.body.dataset.page, analysisView = state.analysisView, stationId = state.stationAnalyticsId || null, filters = structuredClone(backendFilters());
    views.set(box, version); const guard = () => views.get(box) === version && box.isConnected && activeCase() === c && document.body.dataset.page === pageName && state.analysisView === analysisView && (state.stationAnalyticsId || null) === stationId && JSON.stringify(backendFilters()) === JSON.stringify(filters);
    box.textContent = "Lendo a cronologia das evidências preservadas…";
    try {
      const source = await open(c, { filters, stationId, guard }); if (!source.current()) return;
      const cursors = [null], offsets = [0]; let pageIndex = 0, generation = 0;
      const paint = async index => {
        const request = ++generation, page = await source.read(cursors[index]); if (!source.current() || request !== generation) return;
        if (offsets[index] + page.entries.length > page.scopeCount || !page.nextCursor && offsets[index] + page.entries.length !== page.scopeCount || page.nextCursor && cursors.slice(0, index + 1).includes(page.nextCursor)) throw Error("A paginação da cronologia mudou. Reabra a visualização.");
        pageIndex = index; cursors[index + 1] = page.nextCursor; offsets[index + 1] = offsets[index] + page.entries.length;
        const tools = el("div", "source-controls"), previous = el("button", "btn ghost small", "Anteriores"), next = el("button", "btn ghost small", "Próximas"), body = el("div"), summary = el("span", "muted small", `${fmtNum(offsets[index] + (page.entries.length ? 1 : 0))}–${fmtNum(offsets[index + 1])} de ${fmtNum(page.scopeCount)} ocorrências preservadas`);
        previous.type = next.type = "button"; previous.disabled = index === 0; next.disabled = !page.nextCursor;
        const navigate = async target => { previous.disabled = next.disabled = true; try { await paint(target); } catch (error) { if (source.current()) { callbacks.notify?.(String(error.message || error)); previous.disabled = pageIndex === 0; next.disabled = !cursors[pageIndex + 1]; } } };
        previous.onclick = () => navigate(index - 1); next.onclick = () => navigate(index + 1); tools.append(previous, summary, next);
        const unresolved = page.aliases.filter(alias => alias.state !== "unique").length, untimed = page.untimedCount + page.unavailableCount;
        if (unresolved) tools.append(el("span", "muted small", `${unresolved} vínculos antigos indisponíveis ou ambíguos; a autoria permanece preservada`));
        if (untimed && mode !== "table") tools.append(el("span", "muted small", `${fmtNum(untimed)} ocorrências sem horário no recorte; consulte a tabela`));
        box.replaceChildren(tools, body); const collected = collect(c, page, { includeUntimed: mode === "table", complete: index === 0 && !page.nextCursor });
        if (collected.incompletePreviews) tools.append(el("span", "muted small", "Textos resumidos na visualização; abra a ocorrência para ler o valor preservado"));
        const detail = handle => source.current() && window.CaseEvidenceDetail.open(handle.reference, handle.member, { guard: source.current });
        const bucket = (x, y, handles) => callbacks.menu(x, y, handles.map((handle, index) => ({ icon: "fa-eye", label: `${index + 1}. ${handle.title || "Ocorrência preservada"}`, onClick: () => detail(handle) })));
        window.CaseTimeline.render(body, c, mode, { ...callbacks, collected, detail, bucket, nativePage: true });
      };
      await paint(0);
    } catch (error) { if (guard()) { box.textContent = `A cronologia preservada não está disponível: ${String(error.message || error)}`; const retry = el("button", "btn ghost small", "Tentar novamente"); retry.type = "button"; retry.onclick = () => render(box, c, mode, callbacks); box.append(retry); } }
  }
  async function all(c, { limit = 10000, maxChars = 8000000, signal } = {}) {
    limit = Math.max(1, Math.min(10000, Math.floor(Number(limit) || 10000))); maxChars = Math.max(1, Math.min(16000000, Math.floor(Number(maxChars) || 8000000)));
    const check = () => { if (signal?.aborted) throw new DOMException("Operação cancelada", "AbortError"); };
    const live = activeCase(), shape = window.CaseEvidence.analysisShape(c);
    const source = await open(c, { guard: () => !signal?.aborted && activeCase() === live && live?.id === c.id && window.CaseEvidence.analysisShape(live) === shape }), entries = [], seen = new Set(), cursors = new Set(); let cursor = null, bytes = 0, first;
    do { check(); const page = await source.read(cursor); first ||= page;
      if (page.scopeCount > limit) throw Error(`A cronologia tem ${page.scopeCount} ocorrências; este relatório admite até ${limit}. Exporte a investigação nativa para conservar todo o conteúdo.`);
      for (const entry of page.entries) { if (seen.has(entry.entryId)) throw Error("A cronologia repetiu uma ocorrência entre páginas."); seen.add(entry.entryId); if (!entry.title.complete || !entry.detail.complete || !entry.source.complete) throw Error("O relatório exige textos completos; uma prévia foi resumida. Exporte a investigação nativa ou abra os valores preservados."); bytes += entry.title.text.length + entry.detail.text.length + entry.source.text.length; if (bytes > maxChars) throw Error("A cronologia excede o limite de texto deste relatório. Nenhum texto foi omitido."); entries.push(entry); }
      cursor = page.nextCursor; if (cursor && cursors.has(cursor)) throw Error("A cronologia repetiu um cursor."); if (cursor) cursors.add(cursor);
    } while (cursor);
    if (entries.length !== first.scopeCount) throw Error("A leitura da cronologia está incompleta; nenhum relatório foi gerado.");
    check(); return { ...first, entries, nextCursor: null };
  }
  return { aliases, collect, render, all };
})();
