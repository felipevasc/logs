/* Visual case chronology. Its edits belong to the case, never to the source logs. */
window.CaseTimeline = (() => {
  const COLORS = ["#76c9b5", "#7daef2", "#d3a9fa", "#f4bb73", "#f08e91", "#a7b9cf"];
  const ICONS = ["fa-comment", "fa-triangle-exclamation", "fa-shield-halved", "fa-key", "fa-link", "fa-circle-info"];
  const selection = new Set();
  let activeCaseId = null;
  const safe = value => String(value ?? "").replace(/[&<>"']/g, character => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]);
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const when = value => new Intl.DateTimeFormat("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(value));
  const months = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];
  const dayLabel = value => { const date = new Date(value); return `${String(date.getDate()).padStart(2, "0")}/${months[date.getMonth()]}${date.getFullYear() === new Date().getFullYear() ? "" : `/${String(date.getFullYear()).slice(-2)}`}`; };
  const clockLabel = value => new Intl.DateTimeFormat("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(value));
  const shortWhen = value => `${dayLabel(value)} ${clockLabel(value)}`;
  const valid = value => Number.isFinite(value) && value > 0;
  const uniqueId = prefix => `${prefix}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;

  function collect(c, passes) {
    const config = c.timeline ||= { groups: [], annotations: [], edits: {}, layout: {} };
    config.groups ||= []; config.annotations ||= []; config.edits ||= {}; config.layout ||= {};
    const events = [];
    (c.items || []).forEach((item, itemIndex) => {
      (item.rows || []).forEach((event, index) => {
        if (!valid(event.timestamp) || !passes(event)) return;
        const id = `e:${item.id}:${event.event_ref || event.id || index}`;
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
      entries.push({ id: group.id, type: "group", start: Math.min(...members.map(m => m.start)),
        end: Math.max(...members.map(m => m.end)), title: group.name || "Sequência",
        detail: `${members.length} eventos`, source: members[0].source, color: group.color || members[0].color,
        rows: members.flatMap(member => member.rows), members });
    }
    const rest = events.filter(event => !consumed.has(event.id));
    for (let i = 0; i < rest.length;) {
      const first = rest[i];
      const signature = `${first.itemId}|${first.rows[0].code || first.rows[0].name || first.title}`;
      let j = i + 1;
      while (j < rest.length && j - i < 250 && rest[j].start - rest[j - 1].start <= 5 * 60_000 &&
        `${rest[j].itemId}|${rest[j].rows[0].code || rest[j].rows[0].name || rest[j].title}` === signature) j++;
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
    return { config, entries };
  }

  function render(box, c, mode, callbacks) {
    if (activeCaseId !== c.id) { activeCaseId = c.id; selection.clear(); }
    const previousScroll = box.scrollTop;
    const { config, entries } = collect(c, callbacks.passes);
    box.classList.add("case-timeline-host");
    if (!entries.length) {
      box.innerHTML = `<div class="analysis-empty"><i class="fas fa-timeline"></i><br>Nenhum evento com horário no caso.<br><span class="small">Adicione eventos ao caso ou crie um marco.</span><br><button class="btn primary small" id="ct-create-empty">Criar marco</button></div>`;
      box.querySelector("#ct-create-empty").onclick = () => callbacks.createAt(Date.now());
      return;
    }
    const start = Math.min(...entries.map(entry => entry.start));
    const end = Math.max(...entries.map(entry => entry.end));
    const range = Math.max(end - start, 60_000);
    const horizontal = mode === "horizontal";
    const width = horizontal ? Math.max(980, entries.length * 134 + 120) : 0;
    const height = horizontal ? 360 : 90 + entries.length * 70;
    const xs = entries.map((entry, index) => horizontal
      ? 90 + (width - 180) * (entry.start - start) / range
      : 0);
    if (horizontal) for (let i = 1; i < xs.length; i++) xs[i] = Math.max(xs[i], xs[i - 1] + 92);
    const actualWidth = horizontal ? Math.max(width, (xs.at(-1) || 0) + 200) : 0;
    const centerY = 174;
    const points = entries.map((entry, index) => ({
      x: horizontal ? xs[index] : 0,
      y: horizontal ? centerY : 54 + index * 70,
    }));
    const positionFor = time => {
      const coord = point => horizontal ? point.x : point.y;
      if (time <= entries[0].start) return coord(points[0]);
      for (let i = 1; i < entries.length; i++) if (time <= entries[i].start) {
        const ratio = (time - entries[i - 1].start) / Math.max(1, entries[i].start - entries[i - 1].start);
        return coord(points[i - 1]) + ratio * (coord(points[i]) - coord(points[i - 1]));
      }
      return coord(points.at(-1)) + Math.min(90, (time - entries.at(-1).start) / range * 90);
    };
    const timeAt = coordinate => {
      const coord = point => horizontal ? point.x : point.y;
      if (coordinate <= coord(points[0])) return entries[0].start;
      for (let i = 1; i < entries.length; i++) if (coordinate <= coord(points[i])) {
        const ratio = (coordinate - coord(points[i - 1])) / Math.max(1, coord(points[i]) - coord(points[i - 1]));
        return Math.round(entries[i - 1].start + ratio * (entries[i].start - entries[i - 1].start));
      }
      return entries.at(-1).start;
    };
    box.innerHTML = `<div class="ct-shell ${horizontal ? "ct-horizontal" : "ct-vertical"}">
      <div class="ct-top"><div><strong>${entries.length.toLocaleString("pt-BR")} ocorrências</strong><span>${shortWhen(start)} — ${shortWhen(end)}</span></div>
      <div class="ct-top-actions"><button type="button" class="btn ghost small" data-ct-action="add"><i class="fas fa-plus"></i> Marco</button><button type="button" class="btn ghost small" data-ct-action="note"><i class="fas fa-comment-dots"></i> Nota</button><button type="button" class="btn ghost small ct-group-action" data-ct-action="group" hidden>Agrupar seleção</button></div></div>
      <div class="ct-hint">Clique para selecionar · Shift + clique para agrupar · arraste uma barra para mudar o lado · botão direito para editar</div>
      ${horizontal ? '<div class="ct-minimap" title="Clique para navegar pela linha do tempo"><div class="ct-minimap-track"></div><div class="ct-minimap-window"></div></div>' : ""}
      <div class="ct-scroll"><div class="ct-board" style="${horizontal ? `width:${actualWidth}px;height:${height}px` : `height:${height}px`}">
        <div class="ct-axis"></div><svg class="ct-links" aria-hidden="true"></svg><div class="ct-items"></div><div class="ct-notes"></div></div></div>
      <div class="ct-editor-backdrop" hidden><form class="ct-editor"><div class="ct-editor-head"><strong></strong><button type="button" class="ct-close" aria-label="Fechar">×</button></div><div class="ct-editor-fields"></div><div class="ct-editor-actions"><button type="button" class="btn ghost small ct-cancel">Cancelar</button><button type="submit" class="btn primary small">Salvar</button></div></form></div></div>`;
    const shell = box.querySelector(".ct-shell"), board = shell.querySelector(".ct-board");
    const itemLayer = shell.querySelector(".ct-items"), noteLayer = shell.querySelector(".ct-notes");
    const axis = shell.querySelector(".ct-axis");
    if (horizontal) axis.style.top = `${centerY}px`;
    else axis.style.left = "50%";
    const callbacksSave = () => { const scroll = box.scrollTop; callbacks.save(); requestAnimationFrame(() => { box.scrollTop = scroll; }); };
    const selectedEntries = () => entries.filter(entry => selection.has(entry.id));
    const syncSelection = () => {
      shell.querySelectorAll(".ct-entry").forEach(node => node.classList.toggle("selected", selection.has(node.dataset.id)));
      shell.querySelector(".ct-group-action").hidden = selectedEntries().length < 2;
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
    function openEditor(type, entry, time) {
      const backdrop = shell.querySelector(".ct-editor-backdrop"), form = backdrop.querySelector("form");
      const heading = backdrop.querySelector(".ct-editor-head strong"), fields = backdrop.querySelector(".ct-editor-fields");
      const local = value => { const date = new Date(value); return new Date(value - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16); };
      if (type === "manual") {
        heading.textContent = entry ? "Editar marco" : "Novo marco";
        fields.innerHTML = `<label>Nome<input name="name" maxlength="120" required value="${safe(entry?.title || "")}"></label><label>Início<input name="start" type="datetime-local" step="1" required value="${local(entry?.start || time || Date.now())}"></label><label>Fim <span>(opcional)</span><input name="end" type="datetime-local" step="1" value="${entry?.end > entry?.start ? local(entry.end) : ""}"></label><label>Descrição<textarea name="description" rows="3" maxlength="1200">${safe(entry?.detail || "")}</textarea></label>`;
      } else if (type === "annotation") {
        heading.textContent = entry ? "Editar nota" : "Nova nota";
        fields.innerHTML = `<label>Texto <span>(opcional com ícone)</span><textarea name="text" rows="4" maxlength="1500">${safe(entry?.text || "")}</textarea></label><label>Ícone<select name="icon"><option value="" ${entry?.icon === "" ? "selected" : ""}>Sem ícone</option>${ICONS.map(icon => `<option value="${icon}" ${entry?.icon === icon || !entry && icon === ICONS[0] ? "selected" : ""}>${({"fa-comment":"Comentário","fa-triangle-exclamation":"Atenção","fa-shield-halved":"Proteção","fa-key":"Acesso","fa-link":"Relação","fa-circle-info":"Informação"})[icon]}</option>`).join("")}</select></label><label>Pontas<select name="arrow"><option value="forward" ${!entry || entry.arrow === "forward" ? "selected" : ""}>Para a nota</option><option value="back" ${entry?.arrow === "back" ? "selected" : ""}>Para o título</option><option value="both" ${entry?.arrow === "both" ? "selected" : ""}>Nas duas pontas</option><option value="none" ${entry?.arrow === "none" ? "selected" : ""}>Sem pontas</option></select></label><label>Traço<select name="lineStyle"><option value="solid" ${!entry || !entry.lineStyle || entry.lineStyle === "solid" ? "selected" : ""}>Contínuo</option><option value="dashed" ${entry?.lineStyle === "dashed" ? "selected" : ""}>Tracejado</option><option value="dotted" ${entry?.lineStyle === "dotted" ? "selected" : ""}>Pontilhado</option></select></label>`;
      } else {
        heading.textContent = "Editar título";
        fields.innerHTML = `<label>Nome<input name="name" maxlength="120" required value="${safe(entry?.title || "")}"></label>`;
      }
      backdrop.hidden = false;
      fields.querySelector("input,textarea")?.focus();
      const close = () => { backdrop.hidden = true; form.onsubmit = null; };
      backdrop.querySelector(".ct-close").onclick = close;
      backdrop.querySelector(".ct-cancel").onclick = close;
      backdrop.onclick = event => { if (event.target === backdrop) close(); };
      form.onsubmit = event => {
        event.preventDefault();
        const values = new FormData(form);
        if (type === "manual") {
          const from = new Date(values.get("start")).getTime(), to = values.get("end") ? new Date(values.get("end")).getTime() : null;
          if (!valid(from) || to != null && (!valid(to) || to < from)) { callbacks.notify("Revise o intervalo do marco."); return; }
          const manual = entry?.manual || { id: uniqueId("m"), createdAt: Date.now() };
          Object.assign(manual, { name: String(values.get("name")).trim(), description: String(values.get("description")).trim(), start: from, end: to });
          if (!entry) (c.manual ||= []).push(manual);
        } else if (type === "annotation") {
          if (!String(values.get("text")).trim() && !String(values.get("icon"))) { callbacks.notify("Escreva um texto ou escolha um ícone."); return; }
          const selected = selectedEntries()[0] || entries[0];
          const annotation = entry || { id: uniqueId("n"), anchor: selected.members?.[0]?.id || selected.id };
          Object.assign(annotation, { text: String(values.get("text")).trim(), icon: String(values.get("icon")), arrow: String(values.get("arrow")), lineStyle: String(values.get("lineStyle")) });
          if (!entry) config.annotations.push(annotation);
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
    shell.querySelector('[data-ct-action="group"]').onclick = () => {
      const members = selectedEntries().flatMap(entry => entry.members || [entry]).filter(entry => entry.type === "event");
      const ids = [...new Set(members.map(entry => entry.id))];
      if (ids.length < 2) return;
      config.groups = config.groups.filter(group => !group.ids?.some(id => ids.includes(id)));
      config.groups.push({ id: uniqueId("g"), ids, name: members[0].title, color: members[0].color });
      selection.clear(); callbacksSave();
    };
    board.ondblclick = event => {
      if (!event.target.closest(".ct-entry,.ct-note")) {
        const rect = board.getBoundingClientRect();
        openEditor("manual", null, timeAt(horizontal ? event.clientX - rect.left : event.clientY - rect.top));
      }
    };

    const entryNodes = new Map();
    entries.forEach((entry, index) => {
      const point = points[index], saved = config.layout[entry.id] || {};
      const side = saved.side || (index % 2 ? "right" : "left");
      const offset = clamp(saved.offset || 0, 0, 100);
      const node = document.createElement("article");
      node.className = `ct-entry ${entry.type === "manual" ? "is-manual" : ""} ${entry.end > entry.start ? "has-range" : ""}`;
      node.dataset.id = entry.id; node.style.setProperty("--entry-color", entry.color);
      node.dataset.side = side;
      node.title = `${entry.title}\n${entry.source} · ${when(entry.start)}${entry.end > entry.start ? ` → ${when(entry.end)}` : ""}${entry.detail ? `\n${entry.detail}` : ""}`;
      node.innerHTML = `<strong>${safe(entry.title)}</strong>${entry.rows.length > 1 ? `<span class="ct-entry-count">${entry.rows.length}</span>` : ""}<button class="ct-open" type="button" title="Abrir detalhes" aria-label="Abrir detalhes"><i class="fas fa-arrow-up-right-from-square"></i></button>`;
      node.style.setProperty("--entry-offset", `${offset}px`);
      if (horizontal) { node.style.left = `${point.x}px`; node.style.top = `${side === "left" ? centerY - 48 - offset * .5 : centerY + 25 + offset * .5}px`; }
      else node.style.top = `${point.y - 11}px`;
      const open = () => { if (entry.type === "manual") openEditor("manual", entry); else if (entry.rows.length === 1) callbacks.detail(entry.rows[0]); else callbacks.bucket(node.getBoundingClientRect().left, node.getBoundingClientRect().bottom, entry.rows); };
      node.querySelector(".ct-open").onclick = event => { event.stopPropagation(); open(); };
      node.ondblclick = event => { event.stopPropagation(); open(); };
      node.onclick = event => {
        if (event.shiftKey || event.ctrlKey || event.metaKey) selection.has(entry.id) ? selection.delete(entry.id) : selection.add(entry.id);
        else { selection.clear(); selection.add(entry.id); }
        syncSelection();
      };
      node.oncontextmenu = event => {
        event.preventDefault(); selection.add(entry.id); syncSelection();
        const menu = [{ icon: "fa-eye", label: "Abrir detalhes", onClick: open },
          { icon: "fa-comment-dots", label: "Adicionar nota aqui", onClick: () => { selection.clear(); selection.add(entry.id); openEditor("annotation", null); } },
          ...(entry.type === "manual" ? [] : [{ icon: "fa-pen", label: "Editar título", onClick: () => openEditor("title", entry) }]),
          { icon: "fa-palette", label: "Alterar cor", onClick: () => colorMenu(entry, event.clientX, event.clientY) },
          { icon: "fa-arrows-left-right", label: "Mover para o outro lado", onClick: () => { config.layout[entry.id] = { side: side === "left" ? "right" : "left", offset }; callbacksSave(); } }];
        if (entry.type === "manual") menu.push({ icon: "fa-pen", label: "Editar marco", onClick: () => openEditor("manual", entry) },
          { icon: "fa-trash-can", label: "Remover marco", danger: true, onClick: () => { c.manual = c.manual.filter(manual => manual.id !== entry.manual.id); callbacksSave(); } });
        if (entry.type === "group") menu.push({ icon: "fa-layer-group", label: "Desagrupar", onClick: () => { config.annotations.filter(note => note.anchor === entry.id).forEach(note => note.anchor = entry.members[0].id); config.groups = config.groups.filter(group => group.id !== entry.id); callbacksSave(); } });
        if (selectedEntries().length >= 2) menu.push({ icon: "fa-layer-group", label: "Agrupar seleção", onClick: () => shell.querySelector('[data-ct-action="group"]').click() });
        callbacks.menu(event.clientX, event.clientY, menu);
      };
      let drag = null;
      node.onpointerdown = event => { if (event.button !== 0 || event.target.closest("button")) return; drag = { x: event.clientX, y: event.clientY, left: node.offsetLeft, top: node.offsetTop }; if (!horizontal) node.style.right = "auto"; node.setPointerCapture(event.pointerId); };
      node.onpointermove = event => { if (!drag) return; const delta = horizontal ? event.clientY - drag.y : event.clientX - drag.x; if (Math.abs(delta) < 4 && !drag.moved) return; drag.moved = true; node.classList.add("dragging"); if (horizontal) node.style.top = `${drag.top + delta}px`; else node.style.left = `${drag.left + delta}px`; };
      node.onpointerup = event => { if (!drag) return; if (drag.moved) {
        const rect = board.getBoundingClientRect();
        const side = horizontal ? (node.offsetTop < centerY ? "left" : "right") : (node.offsetLeft + node.offsetWidth / 2 < rect.width / 2 ? "left" : "right");
        const offset = horizontal ? clamp(Math.abs(node.offsetTop - centerY) - 25, 0, 100) : clamp(side === "left" ? rect.width / 2 - 30 - node.offsetLeft - node.offsetWidth : node.offsetLeft - rect.width / 2 - 30, 0, 100);
        config.layout[entry.id] = { side, offset }; callbacksSave();
        node.onclick = null;
      } else if (!horizontal) node.style.right = ""; drag = null; node.classList.remove("dragging"); };
      itemLayer.appendChild(node);
      entryNodes.set(entry.id, node);
      {
        const tick = document.createElement("span"); tick.className = "ct-axis-tick"; tick.textContent = clockLabel(entry.start);
        if (horizontal) { tick.style.left = `${point.x}px`; tick.style.top = `${centerY + 14}px`; }
        else { tick.style.top = `${point.y + 12}px`; tick.style.left = "50%"; }
        itemLayer.appendChild(tick);
      }
      const previousDay = index ? new Date(entries[index - 1].start).toDateString() : null;
      if (new Date(entry.start).toDateString() !== previousDay) {
        const day = document.createElement("span"); day.className = "ct-day"; day.textContent = dayLabel(entry.start);
        if (horizontal) { day.style.left = `${point.x - 38}px`; day.style.top = `${centerY - 15}px`; }
        else { day.style.left = "50%"; day.style.top = `${point.y - 32}px`; }
        itemLayer.appendChild(day);
      }
      if (entry.end > entry.start) {
        const bar = document.createElement("div"); bar.className = "ct-range-bar"; bar.style.background = entry.color;
        if (horizontal) { bar.style.left = `${point.x}px`; bar.style.width = `${Math.max(14, positionFor(entry.end) - point.x)}px`; bar.style.top = `${side === "left" ? centerY - 14 : centerY + 11}px`; }
        else { bar.style.top = `${point.y}px`; bar.style.height = `${Math.max(14, positionFor(entry.end) - point.y)}px`; bar.style.left = `calc(50% ${side === "left" ? "-" : "+"} 10px)`; }
        itemLayer.appendChild(bar);
      }
    });
    const anchorIndex = annotation => entries.findIndex(entry => entry.id === annotation.anchor || entry.members?.some(member => member.id === annotation.anchor || `a:${member.id}` === annotation.anchor));
    let curveEditId = null, curveDrag = null;
    const redrawLinks = () => {
      const svg = shell.querySelector(".ct-links"), rect = board.getBoundingClientRect();
      svg.setAttribute("viewBox", `0 0 ${rect.width} ${board.offsetHeight}`);
      svg.replaceChildren();
      for (const annotation of config.annotations) {
        const anchor = anchorIndex(annotation);
        if (anchor < 0) continue;
        const note = noteLayer.querySelector(`[data-note="${annotation.id}"]`);
        if (!note) continue;
        const nr = note.getBoundingClientRect();
        const icon = note.querySelector("i"), label = note.querySelector("span");
        const axisX = horizontal ? points[anchor].x : rect.width / 2;
        const axisY = horizontal ? centerY : points[anchor].y;
        const noteX = nr.left - rect.left + nr.width / 2;
        const noteY = nr.top - rect.top + nr.height / 2;
        const target = horizontal && noteY < axisY && label ? label : icon || label;
        const tr = target?.getBoundingClientRect() || nr;
        const noteSide = Math.sign(horizontal ? noteY - axisY : noteX - axisX) || 1;
        const title = entryNodes.get(entries[anchor].id)?.querySelector("strong");
        if (!title) continue;
        const titleRect = title.getBoundingClientRect();
        const titleX = titleRect.left - rect.left + titleRect.width / 2;
        const titleY = titleRect.top - rect.top + titleRect.height / 2;
        let x1, y1, sourceAbove = false;
        if (horizontal) {
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
        const x2 = horizontal ? tr.left - rect.left + tr.width / 2 : (noteSide > 0 ? tr.left : tr.right) - rect.left - noteSide * 7;
        const y2 = horizontal ? (noteSide > 0 ? tr.top : tr.bottom) - rect.top - noteSide * 7 : tr.top - rect.top + tr.height / 2;
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
        const curve = annotation.curve?.[horizontal ? "horizontal" : "vertical"] || { dx: 0, dy: 0 };
        c1x += curve.dx * 4 / 3; c2x += curve.dx * 4 / 3;
        c1y += curve.dy * 4 / 3; c2y += curve.dy * 4 / 3;
        path.setAttribute("d", `M${x1},${y1} C${c1x},${c1y} ${c2x},${c2y} ${x2},${y2}`);
        path.setAttribute("stroke-linecap", "round");
        if (annotation.lineStyle === "dashed") path.setAttribute("stroke-dasharray", "6 5");
        if (annotation.lineStyle === "dotted") path.setAttribute("stroke-dasharray", "1 5");
        const color = annotation.color || "#d3a9fa";
        path.setAttribute("stroke", color); svg.appendChild(path);
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
          handle.onpointerdown = event => { event.preventDefault(); event.stopPropagation(); curveDrag = { annotation, midX: defaultMidX, midY: defaultMidY, pointerId: event.pointerId }; board.setPointerCapture(event.pointerId); };
          svg.appendChild(handle);
        }
      }
    };
    board.onpointermove = event => {
      if (!curveDrag || event.pointerId !== curveDrag.pointerId) return;
      const rect = board.getBoundingClientRect();
      (curveDrag.annotation.curve ||= {})[horizontal ? "horizontal" : "vertical"] = {
        dx: event.clientX - rect.left - curveDrag.midX,
        dy: event.clientY - rect.top - curveDrag.midY,
      };
      redrawLinks();
    };
    board.onpointerup = event => { if (!curveDrag || event.pointerId !== curveDrag.pointerId) return; curveDrag = null; callbacksSave(); };
    for (const annotation of config.annotations) {
      const anchor = anchorIndex(annotation);
      if (anchor < 0) continue;
      const note = document.createElement("article"); note.className = "ct-note"; note.dataset.note = annotation.id;
      const placement = horizontal ? annotation.horizontal || { x: 180, y: 40 } : annotation.vertical || { x: annotation.x ?? 80, y: annotation.y ?? 20 };
      note.style.left = horizontal ? `${points[anchor].x + placement.x}px` : `calc(50% + ${placement.x}px)`;
      note.style.top = horizontal ? `${placement.y}px` : `${points[anchor].y + placement.y}px`;
      note.style.setProperty("--note-color", annotation.color || "#d3a9fa");
      note.innerHTML = `${ICONS.includes(annotation.icon) ? `<i class="fas ${annotation.icon}"></i>` : ""}${annotation.text ? `<span>${safe(annotation.text)}</span>` : ""}`;
      note.ondblclick = () => openEditor("annotation", annotation);
      note.oncontextmenu = event => { event.preventDefault(); callbacks.menu(event.clientX, event.clientY, [
        { icon: "fa-pen", label: "Editar nota", onClick: () => openEditor("annotation", annotation) },
        { icon: "fa-bezier-curve", label: "Ajustar curva", onClick: () => { curveEditId = annotation.id; redrawLinks(); } },
        ...(annotation.curve?.[horizontal ? "horizontal" : "vertical"] ? [{ icon: "fa-rotate-left", label: "Restaurar curva", onClick: () => { delete annotation.curve[horizontal ? "horizontal" : "vertical"]; callbacksSave(); } }] : []),
        ...COLORS.map((color, index) => ({ icon: "fa-circle", color, label: ["Verde", "Azul", "Lilás", "Âmbar", "Coral", "Cinza"][index], onClick: () => { annotation.color = color; callbacksSave(); } })),
        { icon: "fa-trash-can", label: "Remover nota", danger: true, onClick: () => { config.annotations = config.annotations.filter(item => item.id !== annotation.id); callbacksSave(); } },
      ]); };
      let drag = null;
      note.onpointerdown = event => { if (event.button !== 0) return; drag = { x: event.clientX, y: event.clientY, left: note.offsetLeft, top: note.offsetTop }; note.setPointerCapture(event.pointerId); };
      note.onpointermove = event => { if (!drag) return; const left = drag.left + event.clientX - drag.x, top = drag.top + event.clientY - drag.y; note.style.left = `${horizontal ? left : clamp(left, 8, board.clientWidth - note.offsetWidth - 8)}px`; note.style.top = `${clamp(top, 8, board.offsetHeight - note.offsetHeight - 8)}px`; drag.moved = true; redrawLinks(); };
      note.onpointerup = () => { if (drag?.moved) { annotation[horizontal ? "horizontal" : "vertical"] = { x: note.offsetLeft - (horizontal ? points[anchor].x : board.clientWidth / 2), y: note.offsetTop - (horizontal ? 0 : points[anchor].y) }; callbacksSave(); } drag = null; };
      noteLayer.appendChild(note);
      if (!horizontal) note.style.left = `${clamp(board.clientWidth / 2 + placement.x, 8, board.clientWidth - note.offsetWidth - 8)}px`;
    }
    requestAnimationFrame(redrawLinks);
    syncSelection();
    if (horizontal) {
      const scroll = shell.querySelector(".ct-scroll"), minimap = shell.querySelector(".ct-minimap");
      const track = minimap.querySelector(".ct-minimap-track"), windowMark = minimap.querySelector(".ct-minimap-window");
      const bins = Array(100).fill(0);
      entries.forEach(entry => bins[clamp(Math.floor((entry.start - start) / range * 100), 0, 99)]++);
      const max = Math.max(...bins);
      track.innerHTML = bins.map(count => `<i style="height:${Math.max(2, Math.round(count / max * 22))}px"></i>`).join("");
      const updateWindow = () => { windowMark.style.left = `${scroll.scrollLeft / scroll.scrollWidth * 100}%`; windowMark.style.width = `${scroll.clientWidth / scroll.scrollWidth * 100}%`; };
      scroll.onscroll = updateWindow;
      minimap.onclick = event => {
        const rect = minimap.getBoundingClientRect(), time = start + clamp((event.clientX - rect.left) / rect.width, 0, 1) * range;
        const index = entries.findIndex(entry => entry.start >= time);
        scroll.scrollLeft = Math.max(0, points[Math.max(0, index < 0 ? entries.length - 1 : index)].x - scroll.clientWidth / 2);
      };
      requestAnimationFrame(updateWindow);
    }
    box.scrollTop = previousScroll;
  }
  return { render };
})();
