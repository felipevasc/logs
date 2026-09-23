/* Visual case chronology. Its edits belong to the case, never to the source logs. */
window.CaseTimeline = (() => {
  const COLORS = ["#76c9b5", "#7daef2", "#d3a9fa", "#f4bb73", "#f08e91", "#a7b9cf"];
  const sharedSelection = new Set();
  let activeCaseId = null;
  let selectionAnchor = null;
  let resizeObserver = null;
  const scrollPositions = new Map();
  const tableViews = new Map();
  document.addEventListener("workspace-context-change", () => {
    // The case board is retained while Análise is shown. Do not measure a hidden board.
    resizeObserver?.disconnect(); resizeObserver = null;
  });
  const safe = value => String(value ?? "").replace(/[&<>"']/g, character => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]);
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const preview = (value, limit = 600) => {
    const text = String(value ?? "");
    if (text.length <= limit) return text;
    let prefix = text.slice(0, limit);
    if (/[\uD800-\uDBFF]$/.test(prefix)) prefix = prefix.slice(0, -1);
    return `${prefix}…`;
  };
  const dateTimeFormat = new Intl.DateTimeFormat("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const timeFormat = new Intl.DateTimeFormat("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const when = value => dateTimeFormat.format(new Date(value));
  const months = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];
  const dayLabel = value => { const date = new Date(value); return `${String(date.getDate()).padStart(2, "0")}/${months[date.getMonth()]}${date.getFullYear() === new Date().getFullYear() ? "" : `/${String(date.getFullYear()).slice(-2)}`}`; };
  const clockLabel = value => timeFormat.format(new Date(value));
  const shortWhen = value => `${dayLabel(value)} ${clockLabel(value)}`;
  const valid = value => Number.isFinite(value) && Math.abs(value) <= 8640000000000000;
  const uniqueId = prefix => `${prefix}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
  function noteHelp(origin) {
    const title = 'Notas e setas', text = 'Para mudar a seta, use o botão direito sobre ela. Com teclado, selecione a seta e pressione Enter.';
    if (window.Discovery?.showExplanation) {
      window.Discovery.showExplanation(title, text, origin);
      document.getElementById('analysis-help')?.classList.add('ct-note-help-overlay');
      return;
    }
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay ct-note-help-overlay';
    overlay.innerHTML = '<section class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="ct-note-help-title"><div class="modal-head"><h3 id="ct-note-help-title"></h3><button type="button" class="icon-btn" aria-label="Fechar explicação"><i class="fas fa-xmark" aria-hidden="true"></i></button></div><div class="modal-body"></div></section>';
    overlay.querySelector('h3').textContent = title; overlay.querySelector('.modal-body').textContent = text;
    const close = () => { overlay.remove(); origin.focus(); }, exit = overlay.querySelector('button');
    exit.onclick = close;
    overlay.onclick = event => { if (event.target === overlay) close(); };
    overlay.onkeydown = event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); } else if (event.key === 'Tab') { event.preventDefault(); exit.focus(); } };
    document.body.append(overlay); exit.focus();
  }

  function collect(c, passes = () => true, { signal } = {}) {
    const config = c.timeline ||= { groups: [], annotations: [], edits: {}, layout: {} };
    config.groups ||= []; config.annotations ||= []; config.edits ||= {}; config.layout ||= {};
    const events = []; let undated = 0;
    (c.items || []).forEach((item, itemIndex) => {
      (item.rows || []).forEach((event, index) => {
        if (!(index % 256) && signal?.aborted) throw new DOMException("Operação cancelada", "AbortError");
        if (!passes(event)) return;
        if (!valid(event.timestamp)) { undated++; return; }
        const id = `e:${item.id}:${event.event_ref || (event.id ?? index)}`;
        const edit = config.edits[id] || {};
        events.push({ id, itemId: item.id, type: "event", start: event.timestamp, end: event.timestamp,
          title: edit.title || event.name || event.message || event.code || "Evento",
          detail: event.message || event.description || "", source: item.label || item.name || "Item do caso",
          color: edit.color || item.timelineColor || COLORS[itemIndex % COLORS.length], rows: [event] });
      });
    });
    events.sort((a, b) => a.start - b.start);
    const byId = new Map(events.map(event => [event.id, event]));
    const consumed = new Set();
    const entries = [];
    for (const group of config.groups) {
      const members = (group.ids || []).map(id => byId.get(id)).filter(Boolean);
      if (members.length < 2) continue;
      members.forEach(member => consumed.add(member.id));
      entries.push({ id: group.id, type: "group", start: members.reduce((min, member) => Math.min(min, member.start), Infinity),
        end: members.reduce((max, member) => Math.max(max, member.end), -Infinity), title: group.name || "Sequência",
        detail: `${members.length} eventos`, source: members[0].source, color: group.color || members[0].color,
        rows: members.flatMap(member => member.rows), members });
    }
    const rest = events.filter(event => !consumed.has(event.id));
    for (let i = 0; i < rest.length;) {
      const first = rest[i];
      const signature = first.rows[0].code || first.rows[0].name || first.title;
      let j = i + 1;
      while (config.compact !== false && j < rest.length && j - i < 250 && rest[j].start - rest[j - 1].start <= 5 * 60_000 &&
        rest[j].itemId === first.itemId && (rest[j].rows[0].code || rest[j].rows[0].name || rest[j].title) === signature) j++;
      if (j - i >= 2) {
        const members = rest.slice(i, j);
        entries.push({ id: `a:${first.id}`, type: "auto", start: first.start, end: members.at(-1).end,
          title: first.title, detail: `${members.length} eventos na sequência`, source: first.source,
          color: first.color, rows: members.flatMap(member => member.rows), members });
      } else entries.push(first);
      i = j;
    }
    for (const manual of c.manual || []) {
      if (!valid(manual.start)) continue;
      entries.push({ id: `m:${manual.id}`, type: "manual", start: manual.start,
        end: valid(manual.end) && manual.end > manual.start ? manual.end : manual.start,
        title: manual.name || "Marco", detail: manual.description || "", source: "Marco do caso",
        color: manual.color || COLORS[3], rows: [], manual });
    }
    entries.sort((a, b) => a.start - b.start || a.end - b.end);
    return { config, entries, undated };
  }

  function noteMap(config, entries) {
    const anchors = new Map(), notes = new Map();
    for (const entry of entries) {
      anchors.set(entry.id, entry.id);
      for (const member of entry.members || [entry]) { anchors.set(member.id, entry.id); if (member.id.startsWith("e:")) anchors.set(`a:${member.id}`, entry.id); }
    }
    for (const group of config.groups) if (!anchors.has(group.id)) {
      const member = group.ids?.find(id => anchors.has(id));
      if (member) anchors.set(group.id, anchors.get(member));
    }
    for (const note of config.annotations) {
      const id = anchors.get(note.anchor); if (!id) continue;
      if (!notes.has(id)) notes.set(id, []); notes.get(id).push(note);
    }
    return notes;
  }
  const rowKind = type => ({ event: "Evento", auto: "Sequência", group: "Grupo", manual: "Marco" })[type] || "Evento";
  function exportRow(entry, notes) {
    return { id: entry.id, type: entry.type, start: entry.start, end: entry.end,
      title: String(entry.title || ""), detail: String(entry.detail || ""), source: String(entry.source || ""), count: entry.rows.length,
      itemIds: [...new Set((entry.members || [entry]).map(member => member.itemId).filter(id => id != null))],
      manualId: entry.manual?.id ?? null,
      notes: (notes.get(entry.id) || []).map(note => ({ id: note.id, anchor: note.anchor, text: String(note.text || ""), icon: note.icon || "", color: note.color || null })) };
  }
  function rows(c, passes = () => true, { limit = 10000, maxChars = 8000000, includeUndated = false, signal } = {}) {
    if (signal?.aborted) throw new DOMException("Operação cancelada", "AbortError");
    limit = clamp(Math.floor(Number(limit) || 10000), 1, 100000);
    maxChars = clamp(Math.floor(Number(maxChars) || 8000000), 1, 16000000);
    // Default normalization must not add properties to the original case during export.
    const copy = { ...c, timeline: { ...c.timeline } }, collected = collect(copy, passes, { signal });
    const total = collected.entries.length + (includeUndated ? collected.undated : 0);
    if (total > limit) throw new Error(`A timeline tem ${total.toLocaleString("pt-BR")} linhas; o relatório permite até ${limit.toLocaleString("pt-BR")}. Refine o recorte antes de exportar.`);
    if (includeUndated) for (const item of c.items || []) for (let index = 0; index < (item.rows || []).length; index++) {
      if (!(index % 256) && signal?.aborted) throw new DOMException("Operação cancelada", "AbortError");
      const event = item.rows[index]; if (valid(event.timestamp) || !passes(event)) continue;
      const id = `e:${item.id}:${event.event_ref || (event.id ?? index)}`, edit = collected.config.edits[id] || {};
      collected.entries.push({ id, itemId: item.id, type: "event", start: null, end: null,
        title: edit.title || event.name || event.message || event.code || "Evento",
        detail: event.message || event.description || "", source: item.label || item.name || "Item do caso", rows: [event] });
    }
    const notes = noteMap(collected.config, collected.entries), result = []; let chars = 0, eventCount = 0, end = null;
    for (const entry of collected.entries) {
      if (signal?.aborted) throw new DOMException("Operação cancelada", "AbortError");
      const row = exportRow(entry, notes);
      chars += row.title.length + row.detail.length + row.source.length + row.notes.reduce((sum, note) => sum + note.text.length, 0);
      if (chars > maxChars) throw new Error("O texto completo da timeline ultrapassa o limite do relatório. Refine o recorte antes de exportar; nenhum texto foi omitido.");
      result.push(row); eventCount += row.count; if (row.end != null) end = end == null ? row.end : Math.max(end, row.end);
    }
    return { start: result[0]?.start ?? null, end, eventCount, undated: collected.undated, complete: true, rows: result };
  }
  function renderTable(box, c, config, entries, callbacks) {
    box.classList.add("case-timeline-host");
    if (!tableViews.has(c.id)) { tableViews.set(c.id, { query: "", page: 0 }); if (tableViews.size > 24) tableViews.delete(tableViews.keys().next().value); }
    const view = tableViews.get(c.id), notes = noteMap(config, entries), pageSize = 100;
    box.innerHTML = `<section class="ct-table-shell"><div class="ct-table-tools"><label class="ct-table-search"><i class="fas fa-magnifying-glass" aria-hidden="true"></i><input type="search" aria-label="Buscar na tabela da timeline" placeholder="Buscar ocorrências ou notas" maxlength="200"></label><button type="button" class="btn ghost small" data-ct-report><i class="fas fa-file-pdf" aria-hidden="true"></i> Relatório PDF</button></div><div class="ct-table-scroll"><table class="ct-data-table"><caption class="sr-only">Timeline do caso em ordem cronológica</caption><thead><tr><th scope="col">Horário local</th><th scope="col">Ocorrência</th><th scope="col">Origem</th><th scope="col" class="ct-table-count">Registros</th><th scope="col">Notas</th></tr></thead><tbody></tbody></table></div><div class="ct-table-pager"><span role="status" aria-live="polite"></span><div><button type="button" class="btn ghost small" data-ct-page="previous" aria-label="Página anterior"><i class="fas fa-chevron-left" aria-hidden="true"></i></button><button type="button" class="btn ghost small" data-ct-page="next" aria-label="Próxima página"><i class="fas fa-chevron-right" aria-hidden="true"></i></button></div></div></section>`;
    const body = box.querySelector("tbody"), search = box.querySelector("input"), status = box.querySelector('[role="status"]'), previous = box.querySelector('[data-ct-page="previous"]'), next = box.querySelector('[data-ct-page="next"]');
    const report = box.querySelector("[data-ct-report]"); report.onclick = () => { if (window.CaseReport?.open) window.CaseReport.open(); else callbacks.notify?.("O relatório PDF ainda está sendo preparado. Tente novamente em instantes."); };
    search.value = view.query;
    let indices = entries.map((_, index) => index), generation = 0, debounce;
    const preciseTime = time => { const date = new Date(time); return `${when(time)}.${String(date.getMilliseconds()).padStart(3, "0")}`; };
    const disclosure = (summary, fill) => {
      const details = document.createElement("details"), heading = document.createElement("summary"); heading.textContent = summary; details.append(heading);
      details.addEventListener("toggle", () => { if (details.open && details.children.length === 1) fill(details); }); return details;
    };
    const paint = () => {
      view.page = clamp(view.page, 0, Math.max(0, Math.ceil(indices.length / pageSize) - 1));
      const from = view.page * pageSize, visible = indices.slice(from, from + pageSize); body.replaceChildren();
      for (const index of visible) {
        const entry = entries[index], rowNotes = notes.get(entry.id) || [], tr = document.createElement("tr"); tr.dataset.id = entry.id;
        const time = document.createElement("td"); time.className = "ct-table-time";
        const first = document.createElement("time"); first.dateTime = new Date(entry.start).toISOString(); first.textContent = preciseTime(entry.start); time.append(first);
        if (entry.end !== entry.start) { const last = document.createElement("time"); last.dateTime = new Date(entry.end).toISOString(); last.textContent = `até ${preciseTime(entry.end)}`; time.append(last); }
        const main = document.createElement("td"), open = document.createElement(entry.rows.length ? "button" : "strong"); open.className = "ct-table-title"; open.textContent = preview(entry.title, 180); open.title = preview(entry.title, 600);
        if (entry.rows.length) { open.type = "button"; open.onclick = () => { if (entry.rows.length === 1) callbacks.detail(entry.rows[0]); else { const rect = open.getBoundingClientRect(); callbacks.bucket(rect.left, rect.bottom, entry.rows); } }; }
        main.append(open); const kind = document.createElement("small"); kind.textContent = rowKind(entry.type); main.append(kind);
        if (entry.detail || entry.title.length > 180) main.append(disclosure("Detalhes", node => { for (const text of [entry.title, entry.detail].filter((value, i, all) => value && all.indexOf(value) === i)) { const paragraph = document.createElement("p"); paragraph.textContent = text; node.append(paragraph); } }));
        const source = document.createElement("td"); source.textContent = preview(entry.source, 120); source.title = preview(entry.source, 600);
        const count = document.createElement("td"); count.className = "ct-table-count"; count.textContent = entry.rows.length ? entry.rows.length.toLocaleString("pt-BR") : "—";
        const annotation = document.createElement("td"); annotation.className = "ct-table-notes";
        if (rowNotes.length) annotation.append(disclosure(`${rowNotes.length} ${rowNotes.length === 1 ? "nota" : "notas"} · ${preview(rowNotes[0].text || "Ícone", 80)}`, node => {
          for (const note of rowNotes) { const paragraph = document.createElement("p"), classes = window.NoteIconPicker?.classes(note.icon); if (classes) { const icon = document.createElement("i"); icon.className = classes; icon.setAttribute("aria-hidden", "true"); paragraph.append(icon); } paragraph.append(document.createTextNode(note.text || "Ícone")); node.append(paragraph); }
        })); else annotation.textContent = "—";
        tr.append(time, main, source, count, annotation); body.append(tr);
      }
      if (!visible.length) { const tr = document.createElement("tr"), td = document.createElement("td"); td.colSpan = 5; td.className = "ct-table-empty"; td.textContent = entries.length ? "Nenhuma ocorrência ou nota corresponde à busca." : "Nenhum evento com horário neste recorte do caso."; tr.append(td); body.append(tr); }
      previous.disabled = view.page === 0; next.disabled = from + pageSize >= indices.length;
      status.textContent = indices.length ? `${(from + 1).toLocaleString("pt-BR")}–${Math.min(from + pageSize, indices.length).toLocaleString("pt-BR")} de ${indices.length.toLocaleString("pt-BR")} ocorrências` : "0 ocorrências";
    };
    async function filter() {
      const current = ++generation, query = view.query.trim().toLocaleLowerCase("pt-BR");
      if (!query) { indices = entries.map((_, index) => index); paint(); return; }
      status.textContent = "Buscando…"; const matching = [];
      for (let index = 0; index < entries.length; index++) {
        if (!(index % 1000)) { await new Promise(resolve => setTimeout(resolve, 0)); if (current !== generation || !body.isConnected) return; }
        const entry = entries[index], texts = [entry.title, entry.detail, entry.source, ...(notes.get(entry.id) || []).map(note => note.text)];
        if (texts.some(text => String(text || "").toLocaleLowerCase("pt-BR").includes(query))) matching.push(index);
      }
      if (current === generation) { indices = matching; paint(); }
    }
    search.oninput = () => { view.query = search.value; view.page = 0; generation++; clearTimeout(debounce); debounce = setTimeout(filter, 180); };
    previous.onclick = () => { view.page--; paint(); box.querySelector(".ct-table-scroll").scrollTop = 0; };
    next.onclick = () => { view.page++; paint(); box.querySelector(".ct-table-scroll").scrollTop = 0; };
    if (view.query) filter(); else paint();
  }

  function render(box, c, mode, callbacks) {
    const isolated = !!callbacks.isolated, selection = isolated ? new Set() : sharedSelection;
    if (!isolated) {
      resizeObserver?.disconnect(); resizeObserver = null;
      if (activeCaseId !== c.id) { activeCaseId = c.id; selection.clear(); selectionAnchor = null; }
    }
    const previousScroll = box.scrollTop;
    const scrollKey = JSON.stringify([c.id, mode]);
    const { config, entries } = callbacks.collected || collect(c, callbacks.passes);
    if (mode === "table") { renderTable(box, c, config, entries, callbacks); return; }
    const presentIds = new Set(entries.map(entry => entry.id));
    for (const id of selection) if (!presentIds.has(id)) selection.delete(id);
    box.classList.add("case-timeline-host");
    if (!entries.length) {
      const now = Date.now(), localNow = new Date(now - new Date(now).getTimezoneOffset() * 60000).toISOString().slice(0, 23);
      box.innerHTML = `<div class="analysis-empty"><i class="fas fa-timeline"></i><br>Nenhum evento com horário neste recorte do caso.<br><span class="small">Adicione eventos ao caso, revise os filtros ou crie um marco.</span><br><button type="button" class="btn primary small" id="ct-create-empty">Criar marco</button><form class="ct-empty-form" hidden><label>Nome do marco<input name="name" maxlength="120" required></label><label>Horário local<input name="start" type="datetime-local" step="0.001" required value="${localNow}"></label><div><button type="button" class="btn ghost small" data-empty-cancel>Cancelar</button><button type="submit" class="btn primary small">Criar marco</button></div></form></div>`;
      const create = box.querySelector("#ct-create-empty"), form = box.querySelector(".ct-empty-form");
      create.onclick = () => { create.hidden = true; form.hidden = false; form.querySelector("input").focus(); };
      const cancel = () => { form.hidden = true; create.hidden = false; create.focus(); };
      form.querySelector("[data-empty-cancel]").onclick = cancel;
      form.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); cancel(); } };
      form.onsubmit = event => {
        event.preventDefault();
        const values = new FormData(form), name = String(values.get("name")).trim(), start = new Date(values.get("start")).getTime();
        if (!name || !valid(start)) { callbacks.notify("Preencha o nome e um horário válido."); return; }
        (c.manual ||= []).push({ id: uniqueId("m"), createdAt: Date.now(), name, start, end: null, description: "" });
        callbacks.save();
      };
      return;
    }
    const start = entries[0].start;
    const end = entries.reduce((max, entry) => Math.max(max, entry.end), start);
    const range = Math.max(end - start, 60_000);
    const horizontal = mode === "horizontal";
    const laneMap = new Map(), matrixLanes = [], entryLane = new Map();
    if (horizontal) entries.forEach((entry, index) => {
      const isolated = entry.type === "manual" || entry.type === "group";
      let sourceLanes = laneMap.get(entry.source);
      if (!sourceLanes && !isolated) { sourceLanes = new Map(); laneMap.set(entry.source, sourceLanes); }
      let lane = isolated ? null : sourceLanes.get(entry.title);
      if (!lane) { lane = { index: matrixLanes.length, title: entry.title, source: entry.source, color: entry.color, entries: [], ids: new Set(), count: 0, noteCount: 0 }; if (!isolated) sourceLanes.set(entry.title, lane); matrixLanes.push(lane); }
      lane.entries.push(index); lane.count += entry.rows.length; lane.ids.add(entry.id);
      for (const member of entry.members || []) lane.ids.add(member.id);
      entryLane.set(entry.id, lane);
    });
    const noteLanes = new Map();
    for (const lane of matrixLanes) for (const id of lane.ids) { noteLanes.set(id, lane); if (id.startsWith("e:")) noteLanes.set(`a:${id}`, lane); }
    for (const group of config.groups) if (!noteLanes.has(group.id)) { const first = group.ids?.find(id => noteLanes.has(id)); if (first) noteLanes.set(group.id, noteLanes.get(first)); }
    for (const note of config.annotations) { const lane = noteLanes.get(note.anchor); if (lane) lane.noteCount++; }
    let matrixHeight = 48;
    for (const lane of matrixLanes) {
      lane.y = matrixHeight; lane.height = 44 + lane.noteCount * 64; matrixHeight += lane.height;
    }
    const labelWidth = horizontal ? clamp(Math.round((box.clientWidth || 980) * .22), 160, 240) : 0;
    const matrixZoom = [1, 2, 4, 8].includes(config.matrixZoom) ? config.matrixZoom : 1;
    const plotWidth = horizontal ? Math.max(360, (box.clientWidth || 980) - labelWidth - 50) * matrixZoom : 0;
    const matrixRange = Math.max(1, end - start);
    const matrixPosition = time => labelWidth + 24 + (time - start) / matrixRange * plotWidth;
    const height = horizontal ? Math.max(160, matrixHeight + 22) : 90 + entries.length * 70;
    const actualWidth = horizontal ? labelWidth + plotWidth + 48 : 0;
    const centerY = 174;
    const points = entries.map((entry, index) => ({
      x: horizontal ? matrixPosition(entry.start) : 0,
      y: horizontal ? entryLane.get(entry.id).y + 22 : 54 + index * 70,
    }));
    const positionFor = time => {
      if (horizontal) return matrixPosition(time);
      const coord = point => horizontal ? point.x : point.y;
      if (time <= entries[0].start) return coord(points[0]);
      for (let i = 1; i < entries.length; i++) if (time <= entries[i].start) {
        const ratio = (time - entries[i - 1].start) / Math.max(1, entries[i].start - entries[i - 1].start);
        return coord(points[i - 1]) + ratio * (coord(points[i]) - coord(points[i - 1]));
      }
      return coord(points.at(-1)) + Math.min(90, (time - entries.at(-1).start) / range * 90);
    };
    const timeAt = coordinate => {
      if (horizontal) return Math.round(start + clamp((coordinate - labelWidth - 24) / plotWidth, 0, 1) * matrixRange);
      const coord = point => horizontal ? point.x : point.y;
      if (coordinate <= coord(points[0])) return entries[0].start;
      for (let i = 1; i < entries.length; i++) if (coordinate <= coord(points[i])) {
        const ratio = (coordinate - coord(points[i - 1])) / Math.max(1, coord(points[i]) - coord(points[i - 1]));
        return Math.round(entries[i - 1].start + ratio * (entries[i].start - entries[i - 1].start));
      }
      const last = coord(points.at(-1));
      const tail = positionFor(end);
      return Math.round(entries.at(-1).start + clamp((coordinate - last) / Math.max(1, tail - last), 0, 1) * (end - entries.at(-1).start));
    };
    const eventCount = entries.reduce((count, entry) => count + entry.rows.length, 0);
    box.innerHTML = `<div class="ct-shell ${horizontal ? "ct-horizontal ct-matrix" : "ct-vertical"}">
      <div class="ct-top"><div><strong>${(horizontal ? matrixLanes.length : entries.length).toLocaleString("pt-BR")} ${horizontal ? "linhas" : "ocorrências"}</strong><span>${shortWhen(start)} — ${shortWhen(end)}</span></div>
      <div class="ct-top-actions">${horizontal ? `<div class="ct-zoom" aria-label="Escala da linha do tempo"><button type="button" class="btn ghost small" data-ct-zoom="out" aria-label="Diminuir escala" ${matrixZoom === 1 ? "disabled" : ""}>−</button><button type="button" class="btn ghost small" data-ct-zoom="fit" title="Mostrar todo o período">${matrixZoom === 1 ? "Todo o período" : `${matrixZoom}× · Enquadrar`}</button><button type="button" class="btn ghost small" data-ct-zoom="in" aria-label="Ampliar escala" ${matrixZoom === 8 ? "disabled" : ""}>+</button></div>` : ""}<button type="button" class="btn ghost small" data-ct-action="compact" aria-pressed="${config.compact !== false}" title="Resume eventos consecutivos do mesmo tipo, separados por até 5 minutos"><i class="fas fa-compress"></i> Resumir repetições</button><button type="button" class="btn ghost small" data-ct-action="add"><i class="fas fa-plus"></i> Marco</button><button type="button" class="btn ghost small" data-ct-action="note"><i class="fas fa-comment-dots"></i> Nota</button><button type="button" class="btn ghost small ct-group-action" data-ct-action="group" hidden>Agrupar seleção</button><button type="button" class="btn ghost small ct-clear-action" data-ct-action="clear" hidden>Limpar seleção</button></div></div>
      <div class="ct-hint"><span>${eventCount.toLocaleString("pt-BR")} registros</span><button type="button" class="icon-btn ct-info" aria-label="Como usar a timeline" title="${horizontal ? "Posições proporcionais ao tempo; títulos iguais na mesma linha." : "Ordem cronológica, espaçamento adaptado para leitura."} Shift + clique: trecho · Ctrl + clique: adicionar · Enter: abrir · botão direito: editar"><i class="fas fa-circle-info"></i></button></div>
      ${horizontal ? '<div class="ct-minimap" tabindex="0" role="scrollbar" aria-label="Navegar pela cronologia" aria-controls="ct-viewport" aria-orientation="horizontal" aria-valuemin="0" aria-valuemax="100" aria-valuenow="0" title="Clique ou use as setas para navegar"><div class="ct-minimap-track"></div><div class="ct-minimap-window"></div></div>' : ""}
      <div class="ct-scroll" id="ct-viewport"><div class="ct-board" style="${horizontal ? `width:${actualWidth}px;height:${height}px` : `height:${height}px`}">
        <div class="ct-axis"></div><svg class="ct-links" aria-hidden="true"></svg><div class="ct-items"></div><div class="ct-notes"></div><svg class="ct-controls" aria-label="Setas das notas"></svg></div></div>
      <div class="ct-editor-backdrop" hidden><form class="ct-editor" role="dialog" aria-modal="true" aria-labelledby="ct-editor-title"><div class="ct-editor-head"><strong id="ct-editor-title"></strong><button type="button" class="ct-close" aria-label="Fechar">×</button></div><div class="ct-editor-fields"></div><div class="ct-editor-actions"><button type="button" class="btn ghost small ct-cancel">Cancelar</button><button type="submit" class="btn primary small">Salvar</button></div></form></div></div>`;
    const shell = box.querySelector(".ct-shell"), board = shell.querySelector(".ct-board");
    window.TimelineExport?.attach(shell.querySelector(".ct-top-actions"), { source: shell, type: horizontal ? "matrix" : "vertical", title: `${c.name || "Caso"} · Linha do tempo`, subtitle: `${when(start)} — ${when(end)} · ${eventCount.toLocaleString("pt-BR")} registros`, filename: `${c.name || "caso"}-timeline-${horizontal ? "horizontal" : "vertical"}` });
    const info = shell.querySelector(".ct-info");
    info.onclick = () => window.Discovery?.showExplanation("Como usar a timeline", info.title, info);
    const itemLayer = shell.querySelector(".ct-items"), noteLayer = shell.querySelector(".ct-notes");
    const axis = shell.querySelector(".ct-axis");
    if (horizontal) axis.style.top = `${centerY}px`;
    else axis.style.left = "50%";
    const callbacksSave = () => {
      const scroll = box.scrollTop, viewport = shell.querySelector(".ct-scroll");
      const top = viewport.scrollTop, left = viewport.scrollLeft;
      const focused = document.activeElement;
      const focusedEntry = focused?.closest(".ct-entry")?.dataset.id, focusedNote = focused?.closest(".ct-note")?.dataset.note, focusedArrow = focused?.dataset.arrow, focusedAction = focused?.dataset.ctAction, focusedZoom = focused?.dataset.ctZoom;
      callbacks.save();
      requestAnimationFrame(() => {
        box.scrollTop = scroll;
        const next = box.querySelector(".ct-scroll");
        if (next) { next.scrollTop = top; next.scrollLeft = left; next.dispatchEvent(new Event("scroll")); }
        const focusTarget = [...box.querySelectorAll(".ct-entry,.ct-note,.ct-link-hit,[data-ct-action],[data-ct-zoom]")].find(node => focusedEntry && node.dataset.id === focusedEntry || focusedNote && node.dataset.note === focusedNote || focusedArrow && node.dataset.arrow === focusedArrow || focusedAction && node.dataset.ctAction === focusedAction || focusedZoom && node.dataset.ctZoom === focusedZoom);
        focusTarget?.focus({ preventScroll: true });
      });
    };
    if (horizontal) shell.querySelectorAll("[data-ct-zoom]").forEach(button => {
      button.onclick = () => {
        const viewport = shell.querySelector(".ct-scroll"), center = viewport.scrollLeft + viewport.clientWidth / 2;
        config.matrixZoom = button.dataset.ctZoom === "fit" ? 1 : clamp(matrixZoom * (button.dataset.ctZoom === "in" ? 2 : .5), 1, 8);
        callbacksSave();
        requestAnimationFrame(() => { const next = box.querySelector(".ct-scroll"); if (next) next.scrollLeft = config.matrixZoom === 1 ? 0 : labelWidth + (center - labelWidth) / matrixZoom * config.matrixZoom - next.clientWidth / 2; });
      };
    });
    const selectedEntries = () => entries.filter(entry => selection.has(entry.id));
    const syncSelection = () => {
      shell.querySelectorAll(".ct-entry").forEach(node => { node.classList.toggle("selected", selection.has(node.dataset.id)); node.setAttribute("aria-label", `${selection.has(node.dataset.id) ? "Selecionado. " : ""}${node.title}`); });
      if (horizontal) shell.querySelectorAll(".ct-lane-label").forEach((label, index) => { const selected = matrixLanes[index].entries.every(i => selection.has(entries[i].id)); label.classList.toggle("is-selected", selected); label.setAttribute("aria-pressed", String(selected)); });
      const selected = selectedEntries();
      const canGroup = selected.flatMap(entry => entry.members || [entry]).filter(entry => entry.type === "event").length >= 2;
      shell.querySelector(".ct-group-action").hidden = !canGroup || selected.length < 2;
      shell.querySelector(".ct-group-action").textContent = `Agrupar ${selected.length} selecionados`;
      shell.querySelector(".ct-clear-action").hidden = !selected.length;
    };
    const colorMenu = (entry, x, y) => callbacks.menu(x, y, COLORS.map((color, index) => ({
      icon: "fa-circle", label: ["Verde", "Azul", "Lilás", "Âmbar", "Coral", "Cinza"][index],
      onClick: () => {
        if (entry.type === "manual") entry.manual.color = color;
        else if (entry.type === "group") config.groups.find(g => g.id === entry.id).color = color;
        else if (entry.type === "auto") entry.members.forEach(member => (config.edits[member.id] ||= {}).color = color);
        else (config.edits[entry.id] ||= {}).color = color;
        callbacksSave();
      },
    })));
    function openEditor(type, entry, time, noteAnchor = null) {
      const backdrop = shell.querySelector(".ct-editor-backdrop"), form = backdrop.querySelector("form");
      const heading = backdrop.querySelector(".ct-editor-head strong"), fields = backdrop.querySelector(".ct-editor-fields");
      const local = value => { const date = new Date(value); return new Date(value - date.getTimezoneOffset() * 60000).toISOString().slice(0, 23); };
      if (type === "manual") {
        heading.textContent = entry ? "Editar marco" : "Novo marco";
        fields.innerHTML = `<label>Nome<input name="name" maxlength="120" required value="${safe(entry?.title || "")}"></label><label>Início<input name="start" type="datetime-local" step="0.001" required value="${local(entry?.start ?? time ?? Date.now())}"></label><label>Fim <span>(opcional)</span><input name="end" type="datetime-local" step="0.001" value="${entry?.end > entry?.start ? local(entry.end) : ""}"></label><label>Descrição<textarea name="description" rows="3" maxlength="1200">${safe(entry?.detail || "")}</textarea></label>`;
      } else if (type === "annotation") {
        heading.textContent = entry ? "Editar nota" : "Nova nota";
        fields.innerHTML = `<label>Texto <span>(opcional com ícone)</span><textarea name="text" rows="3" maxlength="1500">${safe(entry?.text || "")}</textarea></label><div class="ct-note-icon-host"></div>`;
        window.NoteIconPicker.mount(fields.querySelector('.ct-note-icon-host'),entry?.icon??'fa-comment');
        const help = document.createElement('button'); help.type = 'button'; help.className = 'icon-btn ct-note-help'; help.title = 'Ajuda sobre notas e setas'; help.setAttribute('aria-label', help.title); help.innerHTML = '<i class="fas fa-circle-info" aria-hidden="true"></i>'; help.onclick = () => noteHelp(help);
        fields.querySelector('.ct-icon-heading').append(help);
      } else if (type === "arrow") {
        heading.textContent = "Editar seta";
        const modeKey = horizontal ? "matrix" : "vertical";
        const sideOptions = selected => [["auto", "Automático"], ["left", "Esquerda"], ["right", "Direita"], ["top", "Cima"], ["bottom", "Baixo"]].map(([value, label]) => `<option value="${value}" ${selected === value ? "selected" : ""}>${label}</option>`).join("");
        fields.innerHTML = `<label>Pontas<select name="arrow"><option value="forward" ${!entry.arrow || entry.arrow === "forward" ? "selected" : ""}>Para a nota</option><option value="back" ${entry.arrow === "back" ? "selected" : ""}>Para o título</option><option value="both" ${entry.arrow === "both" ? "selected" : ""}>Nas duas pontas</option><option value="none" ${entry.arrow === "none" ? "selected" : ""}>Sem pontas</option></select></label><label>Traço<select name="lineStyle"><option value="solid" ${!entry.lineStyle || entry.lineStyle === "solid" ? "selected" : ""}>Contínuo</option><option value="dashed" ${entry.lineStyle === "dashed" ? "selected" : ""}>Tracejado</option><option value="dotted" ${entry.lineStyle === "dotted" ? "selected" : ""}>Pontilhado</option></select></label><label>Saída do título<select name="startSide">${sideOptions(entry.endpoints?.[modeKey]?.start || "auto")}</select></label><label>Chegada à nota<select name="endSide">${sideOptions(entry.endpoints?.[modeKey]?.end || "auto")}</select></label>`;
      } else {
        heading.textContent = "Editar título";
        fields.innerHTML = `<label>Nome<input name="name" maxlength="120" required value="${safe(entry?.title || "")}"></label>`;
      }
      const returnFocus = type === "arrow" ? arrowTarget(entry.id) || document.activeElement : document.activeElement;
      backdrop.hidden = false;
      fields.querySelector("input,textarea,select")?.focus();
      const close = () => { backdrop.hidden = true; form.onsubmit = null; backdrop.onkeydown = null; returnFocus?.focus({ preventScroll: true }); };
      backdrop.onkeydown = event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
        if (event.key !== "Tab") return;
        const focusable = [...form.querySelectorAll("button,input,textarea,select")].filter(node => !node.disabled && node.type !== 'hidden' && node.tabIndex >= 0 && node.getClientRects().length);
        if (event.shiftKey && document.activeElement === focusable[0]) { event.preventDefault(); focusable.at(-1).focus(); }
        else if (!event.shiftKey && document.activeElement === focusable.at(-1)) { event.preventDefault(); focusable[0].focus(); }
      };
      backdrop.querySelector(".ct-close").onclick = close;
      backdrop.querySelector(".ct-cancel").onclick = close;
      backdrop.onclick = event => { if (event.target === backdrop) close(); };
      form.onsubmit = event => {
        event.preventDefault();
        const values = new FormData(form);
        if (!["annotation", "arrow"].includes(type) && !String(values.get("name")).trim()) { callbacks.notify("Dê um nome para identificar este item."); return; }
        if (type === "manual") {
          const from = new Date(values.get("start")).getTime(), to = values.get("end") ? new Date(values.get("end")).getTime() : null;
          if (!valid(from) || to != null && (!valid(to) || to < from)) { callbacks.notify("Revise o intervalo do marco."); return; }
          const manual = entry?.manual || { id: uniqueId("m"), createdAt: Date.now() };
          Object.assign(manual, { name: String(values.get("name")).trim(), description: String(values.get("description")).trim(), start: from, end: to });
          if (!entry) (c.manual ||= []).push(manual);
        } else if (type === "annotation") {
          if (!String(values.get("text")).trim() && !String(values.get("icon"))) { callbacks.notify("Escreva um texto ou escolha um ícone."); return; }
          const selected = selectedEntries()[0] || entries[0];
          const annotation = entry || { id: uniqueId("n"), anchor: noteAnchor || selected.members?.[0]?.id || selected.id };
          Object.assign(annotation, { text: String(values.get("text")).trim(), icon: String(values.get("icon")) });
          if (!entry) Object.assign(annotation, { arrow: "forward", lineStyle: "solid" });
          if (!entry) config.annotations.push(annotation);
        } else if (type === "arrow") {
          entry.arrow = String(values.get("arrow")); entry.lineStyle = String(values.get("lineStyle"));
          (entry.endpoints ||= {})[horizontal ? "matrix" : "vertical"] = { start: String(values.get("startSide")), end: String(values.get("endSide")) };
        } else if (entry) {
          const name = String(values.get("name")).trim();
          if (entry.type === "group") {
            const group = config.groups.find(group => group.id === entry.id);
            if (group) group.name = name;
          } else for (const member of entry.members || [entry]) (config.edits[member.id] ||= {}).title = name;
        }
        close(); callbacksSave();
      };
    }
    shell.querySelector('[data-ct-action="add"]').onclick = () => openEditor("manual", null, entries[Math.floor(entries.length / 2)].start);
    shell.querySelector('[data-ct-action="note"]').onclick = () => openEditor("annotation", null);
    shell.querySelector('[data-ct-action="compact"]').onclick = () => { config.compact = config.compact === false; selection.clear(); callbacksSave(); };
    shell.querySelector('[data-ct-action="clear"]').onclick = () => { selection.clear(); selectionAnchor = null; syncSelection(); };
    shell.querySelector('[data-ct-action="group"]').onclick = () => {
      const members = selectedEntries().flatMap(entry => entry.members || [entry]).filter(entry => entry.type === "event");
      const ids = [...new Set(members.map(entry => entry.id))];
      if (ids.length < 2) return;
      const idSet = new Set(ids);
      config.groups = config.groups.filter(group => {
        const original = group.ids || [];
        group.ids = original.filter(id => !idSet.has(id));
        if (group.ids.length >= 2) return true;
        config.annotations.filter(note => note.anchor === group.id).forEach(note => note.anchor = original[0]);
        return false;
      });
      config.groups.push({ id: uniqueId("g"), ids, name: members[0].title, color: members[0].color });
      selection.clear(); callbacksSave();
    };
    board.ondblclick = event => {
      if (!event.target.closest(".ct-entry,.ct-note,.ct-controls,.ct-lane-label,.ct-matrix-ruler")) {
        const rect = board.getBoundingClientRect();
        openEditor("manual", null, timeAt(horizontal ? event.clientX - rect.left : event.clientY - rect.top));
      }
    };

    const rangeLanes = new Map(), occupiedLanes = { left: [], right: [] };
    entries.forEach((entry, index) => {
      if (entry.end <= entry.start) return;
      const side = config.layout[entry.id]?.side || (index % 2 ? "right" : "left");
      const lanes = occupiedLanes[side];
      let lane = lanes.findIndex(lastEnd => lastEnd < entry.start);
      if (lane < 0) lane = lanes.length;
      lanes[lane] = entry.end;
      rangeLanes.set(entry.id, lane);
    });
    const laneStep = horizontal ? 8 : 9;
    const rangeGutter = { left: Math.max(0, occupiedLanes.left.length - 1) * laneStep, right: Math.max(0, occupiedLanes.right.length - 1) * laneStep };
    const entryNodes = new Map(), markerNodes = new Map(), markerPoints = new Map();
    if (horizontal) {
      board.style.setProperty("--ct-label-width", `${labelWidth}px`);
      const ruler = document.createElement("div"); ruler.className = "ct-matrix-ruler";
      const corner = document.createElement("span"); corner.className = "ct-matrix-corner"; corner.textContent = "Ocorrência / origem"; ruler.append(corner);
      const steps = Math.max(2, Math.floor(plotWidth / 135));
      for (let i = 0; i <= steps; i++) {
        const time = start + matrixRange * i / steps, x = matrixPosition(time);
        const tick = document.createElement("span"); tick.className = "ct-matrix-time"; tick.style.left = `${x}px`; tick.dataset.edge = i === 0 ? "first" : i === steps ? "last" : "middle";
        tick.innerHTML = `<small>${safe(dayLabel(time))}</small><strong>${safe(clockLabel(time))}</strong>`; tick.title = when(time); ruler.append(tick);
        const line = document.createElement("span"); line.className = "ct-matrix-gridline"; line.style.left = `${x}px`; board.append(line);
      }
      board.prepend(ruler);
      for (const lane of matrixLanes) {
        const row = document.createElement("div"); row.className = "ct-matrix-lane"; row.dataset.lane = String(lane.index); row.style.top = `${lane.y}px`; row.style.height = `${lane.height}px`;
        const label = document.createElement("button"); label.type = "button"; label.className = "ct-lane-label"; label.style.setProperty("--entry-color", lane.color);
        label.innerHTML = `<span class="ct-lane-title">${safe(preview(lane.title, 180))}</span><span class="ct-lane-source">${safe(preview(lane.source, 180))}</span><span class="ct-lane-count">${lane.count ? lane.count.toLocaleString("pt-BR") : "◆"}</span>`;
        label.title = `${preview(lane.title)}\n${preview(lane.source, 180)} · ${lane.count} registros\nClique para selecionar a linha; botão direito para opções.`;
        label.onclick = event => { if (!event.ctrlKey && !event.metaKey) selection.clear(); lane.entries.forEach(index => selection.add(entries[index].id)); selectionAnchor = entries[lane.entries[0]].id; syncSelection(); };
        label.oncontextmenu = event => entryNodes.get(entries[lane.entries[0]].id)?.oncontextmenu(event);
        label.onkeydown = event => { if (event.key === "ContextMenu" || event.shiftKey && event.key === "F10") { event.preventDefault(); const rect = label.getBoundingClientRect(); label.oncontextmenu({ preventDefault() {}, clientX: rect.right, clientY: rect.bottom }); } else if (event.key === "ArrowRight") { event.preventDefault(); entryNodes.get(entries[lane.entries[0]].id)?.focus(); } };
        label.ondblclick = event => { event.stopPropagation(); const rows = lane.entries.flatMap(index => entries[index].rows); const bounds = label.getBoundingClientRect(); if (rows.length === 1) callbacks.detail(rows[0]); else if (rows.length) callbacks.bucket(bounds.right, bounds.bottom, rows); else openEditor("manual", entries[lane.entries[0]]); };
        row.append(label); board.append(row);
      }
    }
    entries.forEach((entry, index) => {
      const point = points[index], saved = config.layout[entry.id] || {};
      const side = saved.side || (index % 2 ? "right" : "left");
      const offset = clamp(saved.offset || 0, 0, 100);
      const lane = rangeLanes.get(entry.id) || 0, gutter = rangeGutter[side];
      const node = document.createElement("article");
      node.className = `ct-entry ${entry.type === "manual" ? "is-manual" : ""} ${entry.type === "auto" ? "is-auto" : ""} ${entry.end > entry.start ? "has-range" : ""}`;
      node.tabIndex = index === 0 ? 0 : -1;
      node.setAttribute("role", "group");
      node.setAttribute("aria-roledescription", "ocorrência");
      node.dataset.id = entry.id; node.style.setProperty("--entry-color", entry.color);
      node.dataset.side = side;
      node.title = `${preview(entry.title)}\n${preview(entry.source, 180)} · ${when(entry.start)}${entry.end > entry.start ? ` → ${when(entry.end)}` : ""}${entry.detail ? `\n${preview(entry.detail, 800)}` : ""}`;
      node.innerHTML = `<strong>${safe(preview(entry.title, 180))}</strong>${entry.rows.length > 1 ? `<span class="ct-entry-count" aria-label="${entry.rows.length} registros" title="${entry.type === "auto" ? "Repetições resumidas automaticamente" : "Registros agrupados no caso"}">${entry.rows.length.toLocaleString("pt-BR")}</span>` : ""}<button class="ct-open" type="button" tabindex="-1" title="Abrir detalhes" aria-label="Abrir detalhes de ${safe(preview(entry.title, 180))}"><i class="fas fa-arrow-up-right-from-square"></i></button>`;
      node.style.setProperty("--entry-offset", `${offset}px`);
      node.style.setProperty("--range-gutter", `${gutter}px`);
      node.style.setProperty("--range-lane", `${lane * laneStep}px`);
      if (horizontal) { node.style.left = `${point.x - 7}px`; node.style.top = `${point.y - 10}px`; }
      else node.style.top = `${point.y - 11}px`;
      const open = () => { if (entry.type === "manual") openEditor("manual", entry); else if (entry.rows.length === 1) callbacks.detail(entry.rows[0]); else callbacks.bucket(node.getBoundingClientRect().left, node.getBoundingClientRect().bottom, entry.rows); };
      node.querySelector(".ct-open").onclick = event => { event.stopPropagation(); open(); };
      node.ondblclick = event => { event.stopPropagation(); open(); };
      node.onclick = event => {
        if (event.shiftKey && presentIds.has(selectionAnchor)) {
          const from = entries.findIndex(item => item.id === selectionAnchor);
          if (!event.ctrlKey && !event.metaKey) selection.clear();
          entries.slice(Math.min(from, index), Math.max(from, index) + 1).forEach(item => selection.add(item.id));
        } else {
          if (event.ctrlKey || event.metaKey) selection.has(entry.id) ? selection.delete(entry.id) : selection.add(entry.id);
          else { selection.clear(); selection.add(entry.id); }
          selectionAnchor = entry.id;
        }
        shell.querySelectorAll(".ct-entry").forEach(item => { item.tabIndex = item === node ? 0 : -1; });
        syncSelection();
      };
      node.onkeydown = event => {
        if (event.target !== node) return;
        if (event.key === "Enter") { event.preventDefault(); open(); }
        else if (event.key === " ") { event.preventDefault(); node.onclick(event); }
        else if (event.key === "Escape") { selection.clear(); selectionAnchor = null; syncSelection(); }
        else if (["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          let nextIndex = event.key === "Home" ? 0 : event.key === "End" ? entries.length - 1 : clamp(index + (["ArrowUp", "ArrowLeft"].includes(event.key) ? -1 : 1), 0, entries.length - 1);
          if (horizontal && ["ArrowLeft", "ArrowRight"].includes(event.key)) {
            const laneEntries = entryLane.get(entry.id).entries, current = laneEntries.indexOf(index);
            nextIndex = laneEntries[clamp(current + (event.key === "ArrowLeft" ? -1 : 1), 0, laneEntries.length - 1)];
          } else if (horizontal && ["ArrowUp", "ArrowDown"].includes(event.key)) {
            const lane = entryLane.get(entry.id), nextLane = matrixLanes[clamp(lane.index + (event.key === "ArrowUp" ? -1 : 1), 0, matrixLanes.length - 1)];
            nextIndex = nextLane.entries.reduce((best, candidate) => Math.abs(entries[candidate].start - entry.start) < Math.abs(entries[best].start - entry.start) ? candidate : best, nextLane.entries[0]);
          }
          const next = entryNodes.get(entries[nextIndex].id);
          if (event.shiftKey && !selectionAnchor) selectionAnchor = entry.id;
          next?.onclick(event);
          next?.focus({ preventScroll: true });
          next?.scrollIntoView({ block: "nearest", inline: "nearest" });
        } else if (event.key === "ContextMenu" || event.shiftKey && event.key === "F10") {
          event.preventDefault();
          const rect = node.getBoundingClientRect();
          node.oncontextmenu({ preventDefault() {}, clientX: rect.left, clientY: rect.bottom });
        }
      };
      node.oncontextmenu = event => {
        event.preventDefault(); selection.add(entry.id); syncSelection();
        const menu = [{ icon: "fa-eye", label: "Abrir detalhes", onClick: open },
          { icon: "fa-comment-dots", label: "Adicionar nota aqui", onClick: () => { selection.clear(); selection.add(entry.id); openEditor("annotation", null, null, event.target?.closest("[data-marker-anchor]")?.dataset.markerAnchor || null); } },
          ...(entry.type === "manual" ? [] : [{ icon: "fa-pen", label: "Editar título", onClick: () => openEditor("title", entry) }]),
          { icon: "fa-palette", label: "Alterar cor", onClick: () => colorMenu(entry, event.clientX, event.clientY) },
          ...(!horizontal ? [{ icon: "fa-arrows-left-right", label: "Mover para o outro lado", onClick: () => { config.layout[entry.id] = { side: side === "left" ? "right" : "left", offset }; callbacksSave(); } }] : [])];
        if (entry.type === "manual") {
          menu.push({ icon: "fa-pen", label: "Editar marco", onClick: () => openEditor("manual", entry) },
            { icon: "fa-trash-can", label: "Remover marco", danger: true, onClick: () => { c.manual = c.manual.filter(manual => manual.id !== entry.manual.id); callbacksSave(); } });
        } else {
          menu.push({
            icon: "fa-trash-can",
            label: "Remover do caso",
            danger: true,
            onClick: async () => {
              const idsToRemove = new Set(entry.members ? entry.members.map(m => m.id) : [entry.id]);
              c.items = (c.items || []).filter(it => !idsToRemove.has(it.id));
              config.groups = (config.groups || []).filter(g => !idsToRemove.has(g.id));
              config.annotations = (config.annotations || []).filter(a => !idsToRemove.has(a.anchor));
              await callbacksSave();
              toast("Item removido do Caso.", "ok");
            }
          });
        }
        if (entry.type === "group") menu.push({ icon: "fa-layer-group", label: "Desagrupar", onClick: () => { config.annotations.filter(note => note.anchor === entry.id).forEach(note => note.anchor = entry.members[0].id); config.groups = config.groups.filter(group => group.id !== entry.id); callbacksSave(); } });
        if (!shell.querySelector(".ct-group-action").hidden) menu.push({ icon: "fa-layer-group", label: "Agrupar seleção", onClick: () => shell.querySelector('[data-ct-action="group"]').click() });
        callbacks.menu(event.clientX, event.clientY, menu);
      };
      let drag = null;
      node.onpointerdown = event => { if (horizontal || event.button !== 0 || event.target.closest("button")) return; drag = { x: event.clientX, y: event.clientY, left: node.offsetLeft, top: node.offsetTop, originalLeft: node.style.left, originalTop: node.style.top, originalRight: node.style.right }; node.setPointerCapture(event.pointerId); };
      node.onpointermove = event => { if (!drag) return; const delta = horizontal ? event.clientY - drag.y : event.clientX - drag.x; if (Math.abs(delta) < 4 && !drag.moved) return; drag.moved = true; node.classList.add("dragging"); if (horizontal) node.style.top = `${drag.top + delta}px`; else { node.style.right = "auto"; node.style.left = `${drag.left + delta}px`; } redrawLinks(); };
      node.onpointerup = event => { if (!drag) return; if (drag.moved) {
        const rect = board.getBoundingClientRect();
        const side = horizontal ? (node.offsetTop < centerY ? "left" : "right") : (node.offsetLeft + node.offsetWidth / 2 < rect.width / 2 ? "left" : "right");
        const sideGutter = rangeGutter[side];
        const offset = horizontal ? clamp((side === "left" ? centerY - 48 - sideGutter - node.offsetTop : node.offsetTop - centerY - 25 - sideGutter) * 2, 0, 100) : clamp(side === "left" ? rect.width / 2 - 30 - sideGutter - node.offsetLeft - node.offsetWidth : node.offsetLeft - rect.width / 2 - 30 - sideGutter, 0, 100);
        config.layout[entry.id] = { side, offset }; callbacksSave();
        node.onclick = null;
      } drag = null; node.classList.remove("dragging"); };
      node.onpointercancel = () => { if (!drag) return; node.style.left = drag.originalLeft; node.style.top = drag.originalTop; node.style.right = drag.originalRight; drag = null; node.classList.remove("dragging"); redrawLinks(); };
      itemLayer.appendChild(node);
      entryNodes.set(entry.id, node);
      if (horizontal) {
        const members = entry.members || [entry];
        for (const member of members) {
          const marker = document.createElement("span"); marker.className = "ct-matrix-dot"; marker.dataset.markerAnchor = member.id;
          marker.style.left = `${matrixPosition(member.start) - point.x + 2}px`;
          marker.title = `${preview(entry.title)}\n${when(member.start)}${member.rows[0]?.message ? `\n${preview(member.rows[0].message, 800)}` : ""}`;
          marker.ondblclick = event => { event.stopPropagation(); if (member.rows.length === 1) callbacks.detail(member.rows[0]); else open(); };
          node.append(marker); markerNodes.set(member.id, marker); markerPoints.set(member.id, { x: matrixPosition(member.start), y: point.y });
          if (!markerNodes.has(entry.id)) { markerNodes.set(entry.id, marker); markerPoints.set(entry.id, { x: point.x, y: point.y }); }
        }
      }
      if (!horizontal) {
        const tick = document.createElement("span"); tick.className = "ct-axis-tick"; tick.textContent = clockLabel(entry.start);
        if (horizontal) { tick.style.left = `${point.x}px`; tick.style.top = `${centerY + 14}px`; }
        else { tick.style.top = `${point.y + 12}px`; tick.style.left = "50%"; }
        itemLayer.appendChild(tick);
      }
      const previousDay = index ? new Date(entries[index - 1].start).toDateString() : null;
      if (!horizontal && new Date(entry.start).toDateString() !== previousDay) {
        const day = document.createElement("span"); day.className = "ct-day"; day.textContent = dayLabel(entry.start);
        if (horizontal) { day.style.left = `${point.x - 38}px`; day.style.top = `${centerY - 15}px`; }
        else { day.style.left = "50%"; day.style.top = `${point.y - 32}px`; }
        itemLayer.appendChild(day);
      }
      if (entry.end > entry.start && (!horizontal || entry.type === "manual")) {
        const bar = document.createElement("div"); bar.className = "ct-range-bar"; bar.style.background = entry.color;
        if (horizontal) { bar.style.left = `${point.x}px`; bar.style.width = `${Math.max(2, positionFor(entry.end) - point.x)}px`; bar.style.top = `${point.y - 3}px`; bar.title = `${entry.title} · ${when(entry.start)} → ${when(entry.end)}`; }
        else { bar.style.top = `${point.y}px`; bar.style.height = `${Math.max(14, positionFor(entry.end) - point.y)}px`; bar.style.left = `calc(50% ${side === "left" ? "-" : "+"} ${side === "left" ? 17 + lane * laneStep : 14 + lane * laneStep}px)`; }
        itemLayer.appendChild(bar);
      }
    });
    const anchors = new Map();
    entries.forEach((entry, index) => { anchors.set(entry.id, index); if (entry.type === "event") anchors.set(`a:${entry.id}`, index); for (const member of entry.members || []) { anchors.set(member.id, index); anchors.set(`a:${member.id}`, index); } });
    // Preserve notes attached to a group when filters leave only one member.
    for (const group of config.groups) if (!anchors.has(group.id)) {
      const member = group.ids?.find(id => anchors.has(id));
      if (member) anchors.set(group.id, anchors.get(member));
    }
    const anchorIndex = annotation => anchors.get(annotation.anchor) ?? -1;
    const anchorPoint = annotation => horizontal ? markerPoints.get(annotation.anchor) || points[anchorIndex(annotation)] : points[anchorIndex(annotation)];
    const pointOnSide = (r, side) => {
      const middleX = r.left + r.width / 2, middleY = r.top + r.height / 2;
      if (side === "left") return { x: r.left - 7, y: middleY };
      if (side === "right") return { x: r.right + 7, y: middleY };
      if (side === "top") return { x: middleX, y: r.top - 7 };
      return { x: middleX, y: r.bottom + 7 };
    };
    const nearestSide = (r, x, y) => ["left", "right", "top", "bottom"].reduce((best, side) => {
      const point = pointOnSide(r, side);
      const distance = Math.hypot(x - point.x, y - point.y);
      return distance < best.distance ? { side, distance } : best;
    }, { side: "left", distance: Infinity }).side;
    let curveEditId = null, curveDrag = null;
    const arrowTarget = id => [...shell.querySelectorAll(".ct-link-hit")].find(node => node.dataset.arrow === id);
    const arrowMenu = (annotation, x, y) => {
      const mode = horizontal ? "matrix" : "vertical";
      const saveArrow = change => () => { arrowTarget(annotation.id)?.focus({ preventScroll: true }); change(); callbacksSave(); };
      const choices = (key, options) => callbacks.menu(x, y, options.map(([value, label]) => ({ icon: (annotation[key] || (key === "arrow" ? "forward" : "solid")) === value ? "fa-check" : "fa-minus", label, onClick: saveArrow(() => { annotation[key] = value; }) })));
      callbacks.menu(x, y, [
        { icon: "fa-pen", label: "Editar seta…", onClick: () => openEditor("arrow", annotation) },
        { icon: "fa-arrows-left-right", label: "Pontas da seta…", onClick: () => choices("arrow", [["forward", "Para a nota"], ["back", "Para o título"], ["both", "Nas duas pontas"], ["none", "Sem pontas"]]) },
        { icon: "fa-grip-lines", label: "Tipo de traço…", onClick: () => choices("lineStyle", [["solid", "Contínuo"], ["dashed", "Tracejado"], ["dotted", "Pontilhado"]]) },
        { icon: "fa-palette", label: "Cor…", onClick: () => callbacks.menu(x, y, COLORS.map((color, index) => ({ icon: "fa-circle", color, label: ["Verde", "Azul", "Lilás", "Âmbar", "Coral", "Cinza"][index], onClick: saveArrow(() => { annotation.color = color; }) }))) },
        { sep: true },
        { icon: "fa-bezier-curve", label: curveEditId === annotation.id ? "Concluir ajuste da curva" : "Ajustar curva", onClick: () => { curveEditId = curveEditId === annotation.id ? null : annotation.id; arrowTarget(annotation.id)?.focus({ preventScroll: true }); redrawLinks(); } },
        ...(annotation.curve?.[mode] || annotation.endpoints?.[mode] ? [{ icon: "fa-rotate-left", label: "Restaurar forma automática", onClick: saveArrow(() => { if (annotation.curve) delete annotation.curve[mode]; if (annotation.endpoints) delete annotation.endpoints[mode]; }) }] : []),
      ]);
    };
    const redrawLinks = () => {
      if (!board.isConnected) return;
      const svg = shell.querySelector(".ct-links"), controls = shell.querySelector(".ct-controls"), rect = board.getBoundingClientRect();
      const focusedArrow = isolated ? null : document.activeElement?.dataset.arrow;
      svg.setAttribute("viewBox", `0 0 ${rect.width} ${board.offsetHeight}`);
      controls.setAttribute("viewBox", `0 0 ${rect.width} ${board.offsetHeight}`);
      svg.replaceChildren(); controls.replaceChildren();
      for (const annotation of config.annotations) {
        const anchor = anchorIndex(annotation);
        if (anchor < 0) continue;
        const note = noteNodes.get(annotation.id);
        if (!note) continue;
        const nr = note.getBoundingClientRect();
        const icon = note.querySelector("i"), label = note.querySelector("span");
        const origin = anchorPoint(annotation);
        const axisX = horizontal ? origin.x : rect.width / 2;
        const axisY = origin.y;
        const noteX = nr.left - rect.left + nr.width / 2;
        const noteY = nr.top - rect.top + nr.height / 2;
        const modeKey = horizontal ? "matrix" : "vertical";
        const startSide = annotation.endpoints?.[modeKey]?.start || "auto";
        const endSide = annotation.endpoints?.[modeKey]?.end || "auto";
        const target = endSide === "top" && icon ? icon : endSide === "bottom" && label ? label : horizontal && noteY < axisY && label ? label : icon || label;
        const tr = target?.getBoundingClientRect() || nr;
        const noteSide = Math.sign(horizontal ? noteY - axisY : noteX - axisX) || 1;
        const title = horizontal ? markerNodes.get(annotation.anchor) || markerNodes.get(entries[anchor].id) : entryNodes.get(entries[anchor].id)?.querySelector("strong");
        if (!title) continue;
        const titleRect = title.getBoundingClientRect();
        const titleX = titleRect.left - rect.left + titleRect.width / 2;
        const titleY = titleRect.top - rect.top + titleRect.height / 2;
        let x1, y1, sourceAbove = false;
        if (startSide !== "auto") {
          const point = pointOnSide(titleRect, startSide);
          x1 = point.x - rect.left; y1 = point.y - rect.top;
          sourceAbove = startSide === "top";
        } else if (horizontal) {
          x1 = titleX;
          if (noteY < titleRect.top - rect.top - 8) { y1 = titleRect.top - rect.top - 7; sourceAbove = true; }
          else if (noteY > titleRect.bottom - rect.top + 8) y1 = titleRect.bottom - rect.top + 7;
          else { x1 = (noteX > titleX ? titleRect.right + 7 : titleRect.left - 7) - rect.left; y1 = titleY; }
        } else {
          const titleDirection = titleX < axisX ? -1 : 1;
          const outerEdge = titleDirection > 0 ? titleRect.right : titleRect.left;
          const noteBeyondTitle = (noteX - (outerEdge - rect.left)) * titleDirection > 8;
          if ((noteX - axisX) * titleDirection > 0 && noteBeyondTitle) {
            x1 = outerEdge - rect.left + titleDirection * 7; y1 = titleY;
          } else {
            x1 = titleX;
            sourceAbove = noteY < titleY - 18;
            y1 = (sourceAbove ? titleRect.top - 7 : titleRect.bottom + 7) - rect.top;
          }
        }
        const endPoint = endSide === "auto" ? null : pointOnSide(tr, endSide);
        const x2 = endPoint ? endPoint.x - rect.left : horizontal ? tr.left - rect.left + tr.width / 2 : (noteSide > 0 ? tr.left : tr.right) - rect.left - noteSide * 7;
        const y2 = endPoint ? endPoint.y - rect.top : horizontal ? (noteSide > 0 ? tr.top : tr.bottom) - rect.top - noteSide * 7 : tr.top - rect.top + tr.height / 2;
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        const dx = x2 - x1, dy = y2 - y1;
        let c1x, c1y, c2x, c2y;
        if (horizontal) {
          const bow = clamp(Math.abs(dy) * .22, 32, 65) * (dx >= 0 ? 1 : -1);
          c1x = x1 + bow; c1y = y1 + dy * .3;
          c2x = x2 + bow; c2y = y2 - dy * .3;
        } else {
          const bow = clamp(Math.abs(dx) * .12, 24, 42) * (sourceAbove ? -1 : 1);
          c1x = x1 + dx * .3; c1y = y1 + bow;
          c2x = x2 - dx * .3; c2y = y2 + bow;
        }
        const defaultMidX = (x1 + 3 * c1x + 3 * c2x + x2) / 8;
        const defaultMidY = (y1 + 3 * c1y + 3 * c2y + y2) / 8;
        const curve = annotation.curve?.[horizontal ? "matrix" : "vertical"] || { dx: 0, dy: 0 };
        c1x += curve.dx * 4 / 3; c2x += curve.dx * 4 / 3;
        c1y += curve.dy * 4 / 3; c2y += curve.dy * 4 / 3;
        path.setAttribute("d", `M${x1},${y1} C${c1x},${c1y} ${c2x},${c2y} ${x2},${y2}`);
        path.setAttribute("stroke-linecap", "round");
        if (annotation.lineStyle === "dashed") path.setAttribute("stroke-dasharray", "6 5");
        if (annotation.lineStyle === "dotted") path.setAttribute("stroke-dasharray", "1 5");
        const color = annotation.color || "#d3a9fa";
        path.classList.add("ct-link-line");
        path.setAttribute("stroke", color); svg.appendChild(path);
        // A generous transparent stroke makes thin connectors easy to target without visual clutter.
        const hit = document.createElementNS("http://www.w3.org/2000/svg", "path");
        hit.classList.add("ct-link-hit"); hit.dataset.arrow = annotation.id;
        hit.setAttribute("d", path.getAttribute("d")); hit.setAttribute("tabindex", "0"); hit.setAttribute("role", "button"); hit.setAttribute("aria-haspopup", "menu");
        hit.setAttribute("aria-label", `Editar seta da nota: ${annotation.text || "Ícone"}`);
        const hint = document.createElementNS("http://www.w3.org/2000/svg", "title"); hint.textContent = "Botão direito: editar seta · Enter: opções da seta"; hit.append(hint);
        hit.oncontextmenu = event => { event.preventDefault(); event.stopPropagation(); hit.focus({ preventScroll: true }); arrowMenu(annotation, event.clientX, event.clientY); };
        hit.ondblclick = event => { event.stopPropagation(); openEditor("arrow", annotation); };
        hit.onkeydown = event => {
          if (event.key === "Enter" || event.key === " ") { event.preventDefault(); event.stopPropagation(); openEditor("arrow", annotation); }
          else if (event.key === "ContextMenu" || event.shiftKey && event.key === "F10") { event.preventDefault(); event.stopPropagation(); const bounds = hit.getBoundingClientRect(); arrowMenu(annotation, bounds.left + bounds.width / 2, bounds.top + bounds.height / 2); }
          else if (event.key === "Escape" && curveEditId === annotation.id) { event.preventDefault(); curveEditId = null; redrawLinks(); }
        };
        const highlight = active => { path.classList.toggle("is-highlighted", active); note.classList.toggle("is-arrow-target", active); };
        hit.onpointerenter = () => highlight(true); hit.onpointerleave = () => highlight(document.activeElement === hit); hit.onfocus = () => highlight(true); hit.onblur = () => highlight(false);
        controls.append(hit);
        const addHead = (tipX, tipY, fromX, fromY) => {
          const length = Math.hypot(tipX - fromX, tipY - fromY) || 1;
          const ux = (tipX - fromX) / length, uy = (tipY - fromY) / length;
          const baseX = tipX - ux * 10, baseY = tipY - uy * 10;
          const head = document.createElementNS("http://www.w3.org/2000/svg", "polygon");
          head.setAttribute("points", `${tipX},${tipY} ${baseX - uy * 4.5},${baseY + ux * 4.5} ${baseX + uy * 4.5},${baseY - ux * 4.5}`);
          head.setAttribute("fill", color); svg.appendChild(head);
        };
        const arrow = annotation.arrow || "forward";
        if (arrow === "back" || arrow === "both") addHead(x1, y1, c1x, c1y);
        if (arrow === "forward" || arrow === "both") addHead(x2, y2, c2x, c2y);
        if (curveEditId === annotation.id) {
          const handle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
          handle.setAttribute("class", "ct-curve-handle");
          handle.setAttribute("cx", defaultMidX + curve.dx); handle.setAttribute("cy", defaultMidY + curve.dy);
          handle.setAttribute("r", "7"); handle.setAttribute("fill", color);
          handle.setAttribute("title", "Arraste para ajustar a curva");
          handle.onpointerdown = event => { event.preventDefault(); event.stopPropagation(); curveDrag = { kind: "midpoint", annotation, previous: annotation.curve?.[modeKey] ? { ...annotation.curve[modeKey] } : null, midX: defaultMidX, midY: defaultMidY, pointerId: event.pointerId }; board.setPointerCapture(event.pointerId); };
          controls.appendChild(handle);
          for (const [endpoint, x, y, targetRect] of [["start", x1, y1, titleRect], ["end", x2, y2, tr]]) {
            const grip = document.createElementNS("http://www.w3.org/2000/svg", "circle");
            grip.setAttribute("class", "ct-endpoint-handle"); grip.setAttribute("data-endpoint", endpoint);
            grip.setAttribute("cx", x); grip.setAttribute("cy", y); grip.setAttribute("r", "6");
            grip.setAttribute("stroke", color);
            grip.setAttribute("aria-label", endpoint === "start" ? "Mover saída do título" : "Mover chegada à nota");
            grip.onpointerdown = event => { event.preventDefault(); event.stopPropagation(); curveDrag = { kind: "endpoint", annotation, previous: annotation.endpoints?.[modeKey] ? { ...annotation.endpoints[modeKey] } : null, endpoint, targetRect, pointerId: event.pointerId }; board.setPointerCapture(event.pointerId); };
            controls.appendChild(grip);
          }
        }
      }
      if (focusedArrow) arrowTarget(focusedArrow)?.focus({ preventScroll: true });
    };
    board.onpointermove = event => {
      if (!curveDrag || event.pointerId !== curveDrag.pointerId) return;
      if (curveDrag.kind === "endpoint") {
        const modeKey = horizontal ? "matrix" : "vertical";
        const endpoints = (curveDrag.annotation.endpoints ||= {});
        (endpoints[modeKey] ||= { start: "auto", end: "auto" })[curveDrag.endpoint] = nearestSide(curveDrag.targetRect, event.clientX, event.clientY);
        redrawLinks();
        return;
      }
      const rect = board.getBoundingClientRect();
      (curveDrag.annotation.curve ||= {})[horizontal ? "matrix" : "vertical"] = {
        dx: event.clientX - rect.left - curveDrag.midX,
        dy: event.clientY - rect.top - curveDrag.midY,
      };
      redrawLinks();
    };
    board.onpointerup = event => { if (!curveDrag || event.pointerId !== curveDrag.pointerId) return; curveDrag = null; callbacksSave(); };
    board.onpointercancel = () => {
      if (!curveDrag) return;
      const modeKey = horizontal ? "matrix" : "vertical", key = curveDrag.kind === "endpoint" ? "endpoints" : "curve";
      if (curveDrag.previous) (curveDrag.annotation[key] ||= {})[modeKey] = curveDrag.previous;
      else if (curveDrag.annotation[key]) delete curveDrag.annotation[key][modeKey];
      if (board.hasPointerCapture(curveDrag.pointerId)) board.releasePointerCapture(curveDrag.pointerId);
      curveDrag = null; redrawLinks();
    };
    const noteNodes = new Map();
    const noteRows = new Map();
    const boardRect = board.getBoundingClientRect();
    const occupiedNotes = (config.annotations.length ? [...entryNodes.values()] : []).map(node => {
      const rect = node.getBoundingClientRect();
      return { left: rect.left - boardRect.left, top: rect.top - boardRect.top, right: rect.right - boardRect.left, bottom: rect.bottom - boardRect.top };
    });
    for (const annotation of config.annotations) {
      const anchor = anchorIndex(annotation);
      if (anchor < 0) continue;
      const note = document.createElement("article"); note.className = "ct-note"; note.dataset.note = annotation.id;
      note.tabIndex = 0; note.setAttribute("role", "button"); note.setAttribute("aria-label", `Editar nota: ${annotation.text || "Ícone"}`);
      note.title = annotation.text || "Editar nota";
      const origin = anchorPoint(annotation), lane = horizontal ? entryLane.get(entries[anchor].id) : null;
      const noteRow = horizontal ? noteRows.get(lane.index) || 0 : 0;
      if (horizontal) noteRows.set(lane.index, noteRow + 1);
      const hasPlacement = horizontal ? !!annotation.matrix : !!annotation.vertical || annotation.x != null || annotation.y != null;
      const placement = horizontal ? annotation.matrix || { x: 18, y: 20 + noteRow * 64 } : annotation.vertical || { x: annotation.x ?? 80, y: annotation.y ?? 20 };
      const minNoteX = horizontal ? labelWidth + 8 : 8;
      note.style.left = horizontal ? `${origin.x + placement.x}px` : `calc(50% + ${placement.x}px)`;
      note.style.top = `${origin.y + placement.y}px`;
      note.style.setProperty("--note-color", annotation.color || "#d3a9fa");
      const iconClasses=window.NoteIconPicker.classes(annotation.icon);
      note.innerHTML = `${iconClasses ? `<i class="${iconClasses}" aria-hidden="true"></i>` : ""}${annotation.text ? `<span>${safe(annotation.text)}</span>` : ""}`;
      note.ondblclick = () => openEditor("annotation", annotation);
      note.onkeydown = event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); openEditor("annotation", annotation); }
        else if (event.key === "ContextMenu" || event.shiftKey && event.key === "F10") { event.preventDefault(); const rect = note.getBoundingClientRect(); note.oncontextmenu({ preventDefault() {}, clientX: rect.left, clientY: rect.bottom }); }
      };
      note.oncontextmenu = event => { event.preventDefault(); callbacks.menu(event.clientX, event.clientY, [
        { icon: "fa-pen", label: "Editar nota", onClick: () => openEditor("annotation", annotation) },
        { icon: "fa-arrow-right-long", label: "Opções da seta…", onClick: () => arrowMenu(annotation, event.clientX, event.clientY) },
        ...COLORS.map((color, index) => ({ icon: "fa-circle", color, label: ["Verde", "Azul", "Lilás", "Âmbar", "Coral", "Cinza"][index], onClick: () => { annotation.color = color; callbacksSave(); } })),
        { icon: "fa-trash-can", label: "Remover nota", danger: true, onClick: () => { config.annotations = config.annotations.filter(item => item.id !== annotation.id); callbacksSave(); } },
      ]); };
      let drag = null;
      note.onpointerdown = event => { if (event.button !== 0) return; drag = { x: event.clientX, y: event.clientY, left: note.offsetLeft, top: note.offsetTop }; note.setPointerCapture(event.pointerId); };
      note.onpointermove = event => { if (!drag || !drag.moved && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 4) return; const left = drag.left + event.clientX - drag.x, top = drag.top + event.clientY - drag.y; note.style.left = `${clamp(left, minNoteX, board.clientWidth - note.offsetWidth - 8)}px`; note.style.top = `${clamp(top, horizontal ? 48 : 8, board.offsetHeight - note.offsetHeight - 8)}px`; drag.moved = true; redrawLinks(); };
      note.onpointerup = () => { if (drag?.moved) { annotation[horizontal ? "matrix" : "vertical"] = { x: note.offsetLeft - (horizontal ? origin.x : board.clientWidth / 2), y: note.offsetTop - origin.y }; callbacksSave(); } drag = null; };
      note.onpointercancel = () => { if (drag) { note.style.left = `${drag.left}px`; note.style.top = `${drag.top}px`; drag = null; redrawLinks(); } };
      noteLayer.appendChild(note);
      noteNodes.set(annotation.id, note);
      note.style.left = `${clamp((horizontal ? origin.x : board.clientWidth / 2) + placement.x, minNoteX, board.clientWidth - note.offsetWidth - 8)}px`;
      note.style.top = `${clamp(origin.y + placement.y, horizontal ? 48 : 8, board.offsetHeight - note.offsetHeight - 8)}px`;
      if (!hasPlacement) {
        const anchorNode = entryNodes.get(entries[anchor].id), ar = anchorNode.getBoundingClientRect();
        const width = note.offsetWidth, height = note.offsetHeight;
        const x = horizontal ? origin.x + 18 : anchorNode.dataset.side === "left" ? ar.left - boardRect.left - width - 24 : ar.right - boardRect.left + 24;
        const y = horizontal ? lane.y + 42 + noteRow * 64 : points[anchor].y - height / 2;
        const candidates = [[x, y], [x - width - 24, y], [x + width + 24, y], [x, y - height - 18], [x, y + height + 18]];
        for (const [cx, cy] of candidates) {
          const left = clamp(cx, minNoteX, board.clientWidth - width - 8), top = clamp(cy, horizontal ? 48 : 8, board.offsetHeight - height - 8);
          if (occupiedNotes.some(rect => left < rect.right + 8 && left + width > rect.left - 8 && top < rect.bottom + 8 && top + height > rect.top - 8)) continue;
          note.style.left = `${left}px`; note.style.top = `${top}px`; break;
        }
      }
      occupiedNotes.push({ left: note.offsetLeft, top: note.offsetTop, right: note.offsetLeft + note.offsetWidth, bottom: note.offsetTop + note.offsetHeight });
    }
    requestAnimationFrame(redrawLinks);
    syncSelection();
    let updateMinimap = () => {};
    if (horizontal) {
      const scroll = shell.querySelector(".ct-scroll"), minimap = shell.querySelector(".ct-minimap");
      const track = minimap.querySelector(".ct-minimap-track"), windowMark = minimap.querySelector(".ct-minimap-window");
      const bins = Array(100).fill(0);
      entries.forEach(entry => (entry.members || [entry]).forEach(member => bins[clamp(Math.floor(matrixPosition(member.start) / actualWidth * 100), 0, 99)]++));
      const max = Math.max(...bins);
      track.innerHTML = bins.map(count => `<i style="height:${count ? Math.max(2, Math.round(count / max * 22)) : 0}px"></i>`).join("");
      const updateWindow = () => { windowMark.style.left = `${scroll.scrollLeft / scroll.scrollWidth * 100}%`; windowMark.style.width = `${scroll.clientWidth / scroll.scrollWidth * 100}%`; minimap.setAttribute("aria-valuenow", String(Math.round(scroll.scrollLeft / Math.max(1, scroll.scrollWidth - scroll.clientWidth) * 100))); };
      updateMinimap = updateWindow;
      scroll.onscroll = updateWindow;
      minimap.onclick = event => {
        const rect = minimap.getBoundingClientRect();
        scroll.scrollLeft = clamp((event.clientX - rect.left) / rect.width, 0, 1) * scroll.scrollWidth - scroll.clientWidth / 2;
      };
      minimap.onkeydown = event => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        scroll.scrollLeft = event.key === "Home" ? 0 : event.key === "End" ? scroll.scrollWidth : scroll.scrollLeft + (event.key === "ArrowLeft" ? -1 : 1) * scroll.clientWidth * .7;
      };
      requestAnimationFrame(updateWindow);
    }
    const viewport = shell.querySelector(".ct-scroll");
    const position = isolated ? null : scrollPositions.get(scrollKey);
    if (position) { viewport.scrollLeft = position.left; viewport.scrollTop = position.top; }
    if (!isolated) viewport.addEventListener("scroll", () => {
      scrollPositions.set(scrollKey, { left: viewport.scrollLeft, top: viewport.scrollTop });
      if (scrollPositions.size > 24) scrollPositions.delete(scrollPositions.keys().next().value);
    });
    if (!isolated) {
      const observer = new ResizeObserver(() => { if (!board.isConnected) { observer.disconnect(); return; } redrawLinks(); updateMinimap(); });
      resizeObserver = observer; observer.observe(board); observer.observe(shell.querySelector(".ct-scroll"));
    }
    box.scrollTop = previousScroll;
  }
  function overviewImage(host, entries, eventCount, undated) {
    const start = entries[0].start, end = entries.reduce((max, entry) => Math.max(max, entry.end), start), span = Math.max(1, end - start), sources = new Map();
    for (const entry of entries) for (const member of entry.members || [entry]) if (member.rows.length) sources.set(member.source, (sources.get(member.source) || 0) + member.rows.length);
    const sorted = [...sources].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])), shown = sorted.slice(0, 15), names = new Map(shown.map(([name], index) => [name, index]));
    if (sorted.length > shown.length) shown.push(["Outras origens", sorted.slice(15).reduce((sum, [, count]) => sum + count, 0)]);
    const bins = shown.map(() => Array(80).fill(0));
    for (const entry of entries) for (const member of entry.members || [entry]) if (member.rows.length) {
      const index = names.has(member.source) ? names.get(member.source) : shown.length - 1;
      bins[index][clamp(Math.floor((member.start - start) / span * 80), 0, 79)] += member.rows.length;
    }
    const width = host.clientWidth, label = 220, plotWidth = width - label - 25, rowHeight = 38, height = 58 + Math.max(1, shown.length) * rowHeight;
    const summary = `Visão agregada de ${eventCount.toLocaleString("pt-BR")} registros em 80 intervalos${sorted.length > 15 ? "; as demais origens estão somadas em Outras origens" : ""}. Marcos, notas e ocorrências constam na tabela do relatório.${undated ? ` ${undated.toLocaleString("pt-BR")} registros sem horário não entram no eixo temporal.` : ""}`;
    const section = document.createElement("section"); section.className = "ct-report-overview";
    section.innerHTML = `<p>${safe(summary)}</p><svg width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-label="Volume de registros por origem e intervalo"><text x="${label}" y="18" font-size="10" fill="#526575">${safe(shortWhen(start))}</text><text x="${width - 25}" y="18" text-anchor="end" font-size="10" fill="#526575">${safe(shortWhen(end))}</text>${shown.map(([name, count], row) => {
      const y = 40 + row * rowHeight, max = Math.max(1, ...bins[row]);
      return `<text x="8" y="${y + 8}" font-size="11" fill="#192b37">${safe(preview(name, 27))}</text><text x="${label - 14}" y="${y + 21}" text-anchor="end" font-size="9" fill="#526575">${count.toLocaleString("pt-BR")}</text><line x1="${label}" x2="${width - 25}" y1="${y + 23}" y2="${y + 23}" stroke="#dce3e9"/>${bins[row].map((count, bin) => count ? `<rect x="${label + bin / 80 * plotWidth}" y="${y + 23 - Math.max(2, count / max * 23)}" width="${Math.max(1, plotWidth / 80 - 2)}" height="${Math.max(2, count / max * 23)}" fill="#7850a5"/>` : "").join("")}`;
    }).join("")}</svg>`;
    host.append(section); return { source: section, summary };
  }
  async function image(c, { mode = "horizontal", passes = () => true, width = 1100, maxHeight = 900, forceOverview = false, signal } = {}) {
    const check = () => { if (signal?.aborted) throw new DOMException("Operação cancelada", "AbortError"); };
    check(); if (!window.TimelineExport) throw new Error("O exportador de timeline não está disponível.");
    const copy = { ...c, timeline: structuredClone(c.timeline || {}) };
    copy.timeline.matrixZoom = 1;
    const collected = collect(copy, passes, { signal }), { entries, config, undated } = collected;
    const eventCount = entries.reduce((count, entry) => count + entry.rows.length, 0);
    if (!entries.length) return { blob: null, width: 0, height: 0, complete: true, overview: false, eventCount, undated, summary: "Nenhum evento com horário neste recorte do caso." };
    const lanes = new Map(); let laneCount = 0;
    for (const entry of entries) {
      if (entry.type === "manual" || entry.type === "group") { laneCount++; continue; }
      if (!lanes.has(entry.source)) lanes.set(entry.source, new Set());
      const titles = lanes.get(entry.source); if (!titles.has(entry.title)) { titles.add(entry.title); laneCount++; }
    }
    const noteChars = config.annotations.reduce((sum, note) => sum + String(note.text || "").length, 0);
    let overview = !!forceOverview || eventCount > 2000 || laneCount > 40 || config.annotations.length > 60 || noteChars > 20000 || mode === "vertical" && entries.length > 60;
    const host = document.createElement("div"); host.className = "ct-report-capture ct-report-light"; host.inert = true; host.setAttribute("aria-hidden", "true");
    host.style.width = `${clamp(Math.round(Number(width) || 1100), 800, 1400)}px`; document.body.append(host);
    let source, type, summary;
    const drawOverview = () => { host.replaceChildren(); const result = overviewImage(host, entries, eventCount, undated); source = result.source; type = "activity"; summary = result.summary; overview = true; };
    try {
      if (overview) drawOverview();
      else {
        render(host, copy, mode === "vertical" ? "vertical" : "horizontal", { isolated: true, collected, passes, detail() {}, bucket() {}, menu() {}, notify() {}, save() {} });
        await new Promise(requestAnimationFrame); check();
        source = host.querySelector(".ct-shell"); type = mode === "vertical" ? "vertical" : "matrix";
        const measured = TimelineExport.measure({ source, type }), limits = TimelineExport.limits;
        if (measured.height + 110 > Math.max(800, Number(maxHeight) || 900) || measured.width * (measured.height + 110) > limits.maxPixels || Math.max(measured.width, measured.height + 110) > limits.maxEdge) drawOverview();
        else summary = `Timeline · ${eventCount.toLocaleString("pt-BR")} registros${undated ? ` · ${undated.toLocaleString("pt-BR")} sem horário fora do eixo temporal` : ""}. Notas completas na tabela.`;
      }
      check();
      const rendered = await TimelineExport.render({ source, type, format: "png", theme: "light", noteAppendix: false, title: `${c.name || "Caso"} · Linha do tempo`, subtitle: summary }, { signal });
      return { ...rendered, complete: !overview, overview, summary, eventCount, undated };
    } finally { host.remove(); }
  }
  return { render, rows, image };
})();
