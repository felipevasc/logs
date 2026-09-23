/* Exact correlation across the current source or saved case records. */
window.Journeys = (() => {
  "use strict";
  const contexts = new Map();
  const globalScope = () => window.WorkspaceContext?.scope() || state.analyticsScope || "dataset";
  const contextKey = (scope = globalScope()) => JSON.stringify([scope, state.cases?.active, scope === "dataset" ? state.currentArtifact?.id : null]);
  let view, host = null, drawerSource = null, generation = 0;
  function syncContext() {
    const scope = globalScope(), key = contextKey(scope);
    if (!contexts.has(key)) {
      contexts.set(key, { scope, field: "", fields: [], customFields: [], page: 0, sort: "recent", singles: false, selected: null, detailPage: 0, from: null, to: null, seed: null, seedPending: false, token: 0, detailToken: 0, indexToken: 0, signature: "", listScroll: 0, detailScroll: 0 });
      if (contexts.size > 12) contexts.delete(contexts.keys().next().value);
    }
    view = contexts.get(key);
  }
  syncContext();
  document.addEventListener("workspace-context-change", () => { generation++; syncContext(); });
  function capture() {
    syncContext();
    return { field: view.field, customFields: view.customFields.map(item => item.field), sort: view.sort, singles: view.singles, page: view.page, detailPage: view.detailPage, selected: view.selected?.value ?? null, from: view.from, to: view.to, listScroll: view.listScroll, detailScroll: view.detailScroll, seed: view.seed && { event_ref: view.seed.event_ref || "", id: view.seed.id, timestamp: view.seed.timestamp, source: view.seed.source || "" } };
  }
  function restore(snapshot) {
    generation++; syncContext();
    if (!snapshot || typeof snapshot !== "object") return;
    const field = value => typeof value === "string" && value.length <= 256;
    const position = value => Number.isFinite(value) && value >= 0 ? Math.min(1e7, Math.floor(value)) : 0;
    view.field = field(snapshot.field) ? snapshot.field : "";
    view.customFields = (Array.isArray(snapshot.customFields) ? snapshot.customFields : []).filter(field).slice(0, 50).map(field => ({ field, kind: inferredKind(field), label: field }));
    view.sort = ["recent", "duration", "count", "errors"].includes(snapshot.sort) ? snapshot.sort : "recent";
    view.singles = snapshot.singles === true;
    view.page = position(snapshot.page); view.detailPage = position(snapshot.detailPage);
    view.selected = typeof snapshot.selected === "string" && snapshot.selected.length <= 4096 ? { value: snapshot.selected } : null;
    view.from = Number.isFinite(snapshot.from) ? snapshot.from : null; view.to = Number.isFinite(snapshot.to) ? snapshot.to : null;
    view.listScroll = position(snapshot.listScroll); view.detailScroll = position(snapshot.detailScroll);
    const seed = snapshot.seed;
    view.seed = seed && Number.isInteger(seed.id) && (seed.timestamp == null || Number.isFinite(seed.timestamp)) ? { id: seed.id, timestamp: seed.timestamp ?? null, event_ref: typeof seed.event_ref === "string" ? seed.event_ref.slice(0, 4096) : "", source: typeof seed.source === "string" ? seed.source.slice(0, 4096) : "" } : null;
    view.seedPending = false; view.rangeDraft = null; view.restorePending = true;
  }
  const button = (label, action, cls = "btn ghost small") => { const node = el("button", cls, label); node.type = "button"; node.onclick = action; return node; };
  const isActive = token => token === generation && token === view.token && view.scope === globalScope() && host?.isConnected && document.body.dataset.page === "journeys";
  const base = () => analyticsRequest(view.scope);
  const valueOf = (event, field) => { const value = Object.hasOwn(event || {}, field) ? event[field] : event?.fields?.[field]; return value == null || typeof value === "object" ? null : String(value); };
  const currentField = () => view.fields.find(field => field.field === view.field);
  const heuristic = () => ["user", "ip"].includes(currentField()?.kind);
  const timeArgs = () => heuristic() ? { from: view.from, to: view.to } : {};
  const hasWindow = () => Number.isFinite(view.from) && Number.isFinite(view.to) && view.from <= view.to;
  const resetRange = () => { view.from = view.to = null; view.rangeDraft = null; };
  function inferredKind(field) {
    const name = field.toLowerCase().replace(/[^a-z0-9]/g, "");
    if (["username", "userid", "userprincipalname", "accountname", "principalname"].some(suffix => name.endsWith(suffix)) || ["user", "usuario", "nomeusuario", "idusuario"].includes(name)) return "user";
    if (["sourceip", "destinationip", "clientip", "remoteip", "srcip", "dstip", "remoteaddr", "sourceaddress", "destinationaddress"].some(suffix => name.endsWith(suffix)) || ["ip", "ipcliente", "iporigem", "ipdestino", "enderecoip"].includes(name)) return "ip";
    return "custom";
  }
  function preferred(fields) {
    const count = view.scope === "case" ? caseEvents().filter(rowPassesFilters).length : state.total;
    const repeated = field => field.distinct != null && field.distinct < Math.round(field.coverage * count);
    return fields.find(field => field.suggested && repeated(field)) || fields.find(field => field.suggested) || fields.find(field => ["user", "ip"].includes(field.kind)) || fields.find(repeated) || fields[0];
  }
  const duration = value => { if (value == null) return "Sem duração"; if (value < 1000) return `${value} ms`; if (value < 60000) return `${(value / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s`; if (value < 3600000) return `${(value / 60000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} min`; return `${(value / 3600000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} h`; };
  const fullTime = value => value == null ? "Sem horário" : new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "medium" }).format(value);
  const localInput = value => value == null ? "" : new Date(value - new Date(value).getTimezoneOffset() * 60000).toISOString().slice(0, 23);
  function help(origin, message = "Registros são ligados pelo mesmo valor exato no campo escolhido. Letras maiúsculas e espaços são preservados; nomes de campos diferentes não são fundidos. IDs de trace, requisição, correlação ou sessão costumam ser as melhores chaves. Uma ligação não demonstra causa. Para usuário ou IP, escolha uma janela de tempo: são relações temporais, não sessões confirmadas.") { window.Discovery?.showExplanation("Como interpretar Jornadas", message, origin); }
  function pager(target, page, total, size, change) {
    const node = el("div", "journey-pager"), pages = Math.max(1, Math.ceil(total / size));
    const previous = button("Anterior", () => change(page - 1)), next = button("Próxima", () => change(page + 1)); previous.disabled = page === 0; next.disabled = page >= pages - 1;
    node.append(previous, el("span", "", `${page + 1} / ${pages}`), next); target.append(node);
  }
  function status(message, warning = false) { const node = host?.querySelector(".journey-status"); if (node) { node.textContent = message; node.classList.toggle("journey-warning", warning); } }
  function busy(target, message) {
    target.innerHTML = ""; const block = el("div", "journey-empty", message);
    const cancel = button("Cancelar", async () => { cancel.disabled = true; try { await api("cancel_operation"); } catch {} }); block.append(document.createElement("br"), cancel); target.append(block);
  }
  function failure(target, error, retry) { target.innerHTML = ""; const node = el("div", "journey-empty", String(error)); node.append(document.createElement("br"), button("Tentar novamente", retry)); target.append(node); }
  function controls() {
    const tools = el("div", "journey-controls");
    function select(label, values, value, action, className = "") {
      const wrapper = el("label", "", label), input = el("select", className); input.setAttribute("aria-label", label);
      for (const [key, caption] of values) { const option = el("option", "", caption); option.value = key; input.append(option); }
      input.value = value; input.onchange = () => action(input.value); wrapper.append(input); tools.append(wrapper); return input;
    }
    const choices = view.fields.map(item => [item.field, `${item.field}${item.suggested ? " · sugerido" : ""}`]);
    if (!view.field) choices.unshift(["", "Escolha um campo"]);
    const field = select("Ligar pelo campo", choices, view.field, field => { view.field = field; view.selected = null; view.page = 0; view.detailPage = 0; view.seed = null; resetRange(); draw(); loadIndex(); }, "journey-key");
    const custom = el("option", "", "Outro campo…"); custom.value = "__custom__"; field.append(custom);
    field.onchange = () => {
      if (field.value !== "__custom__") { view.field = field.value; view.selected = null; view.page = 0; view.seed = null; resetRange(); draw(); loadIndex(); return; }
      const input = el("input"); input.placeholder = "Nome exato do campo"; input.setAttribute("aria-label", "Campo personalizado"); input.maxLength = 256;
      const apply = button("Usar campo", () => { if (!input.value) return; view.field = input.value; if (!view.fields.some(item => item.field === view.field)) { const custom = { field: view.field, kind: inferredKind(view.field), label: view.field }; view.fields.push(custom); view.customFields.push(custom); } view.page = 0; view.selected = null; view.seed = null; resetRange(); draw(); loadIndex(); });
      field.parentElement.append(input, apply); input.focus(); input.onkeydown = event => { if (event.key === "Enter") apply.click(); };
    };
    select("Ordenar", [["recent", "Mais recentes"], ["duration", "Maior duração"], ["count", "Mais registros"], ["errors", "Mais erros"]], view.sort, sort => { view.sort = sort; view.page = 0; loadIndex(); });
    const single = el("label", "journey-single"), checkbox = el("input"); checkbox.type = "checkbox"; checkbox.checked = view.singles; checkbox.onchange = () => { view.singles = checkbox.checked; view.page = 0; loadIndex(); }; single.append(checkbox, document.createTextNode("Incluir isolados")); tools.append(single);
    const jump = el("label", "journey-key-jump", "Abrir valor exato"), row = el("div"), input = el("input"); input.type = "search"; input.placeholder = "Cole um identificador…"; input.setAttribute("aria-label", "Identificador exato");
    const go = button("Abrir", () => { if (input.value !== "") selectValue(input.value); }); row.append(input, go); jump.append(row); tools.append(jump); input.onkeydown = event => { if (event.key === "Enter") go.click(); };
    const info = button("", () => help(info), "icon-btn"); info.innerHTML = '<i class="fas fa-circle-info"></i>'; info.title = "Como interpretar Jornadas"; info.setAttribute("aria-label", info.title);
    const reload = button("", () => render(host.parentElement), "icon-btn"); reload.innerHTML = '<i class="fas fa-rotate"></i>'; reload.title = "Atualizar jornadas"; reload.setAttribute("aria-label", reload.title); tools.append(info, reload);
    return tools;
  }
  function draw() {
    view.detailToken++; host.innerHTML = ""; host.append(controls());
    if (heuristic()) {
      const range = el("form", "journey-window");
      const tag = button("Proximidade temporal", () => help(tag), "journey-heuristic"); range.append(tag);
      for (const [key, label] of [["from", "De"], ["to", "Até"]]) { const wrapper = el("label", "", label), input = el("input"); input.name = key; input.type = "datetime-local"; input.step = "0.001"; input.required = true; input.setAttribute("aria-label", label); input.value = view.rangeDraft?.[key] ?? localInput(view[key]); input.oninput = () => { view.rangeDraft ||= { from: localInput(view.from), to: localInput(view.to) }; view.rangeDraft[key] = input.value; }; wrapper.append(input); range.append(wrapper); }
      const apply = button("Aplicar período", () => range.requestSubmit()); range.append(apply);
      range.onsubmit = event => { event.preventDefault(); const from = new Date(range.elements.from.value).getTime(), to = new Date(range.elements.to.value).getTime(); if (!Number.isFinite(from) || !Number.isFinite(to) || from > to) { toast("Revise o início e o fim do período.", "info"); return; } view.from = from; view.to = to; view.rangeDraft = null; view.seedPending = false; view.page = view.detailPage = 0; loadIndex(); if (view.selected) loadDetail(); }; host.append(range);
    }
    const statusNode = el("div", "journey-status"); statusNode.setAttribute("role", "status"); host.append(statusNode);
    const grid = el("div", "journey-grid"), list = el("section", "journey-list-panel"), detail = el("section", "journey-detail");
    list.setAttribute("aria-label", "Jornadas encontradas"); detail.setAttribute("aria-label", "Registros da jornada");
    detail.append(el("div", "journey-empty", "Escolha um identificador para acompanhar seus registros no tempo.")); grid.append(list, detail); host.append(grid);
  }
  async function loadIndex() {
    const token = view.token, version = ++view.indexToken, list = host.querySelector(".journey-list-panel");
    if (!view.field) { list.innerHTML = '<p class="journey-empty">Escolha um campo para ligar os registros.</p>'; return; }
    if (heuristic() && !hasWindow()) { list.innerHTML = '<p class="journey-empty">Escolha início e fim para investigar este usuário ou IP no tempo.</p>'; status("Informe um período para evitar ligar registros sem relação."); return; }
    busy(list, "Relacionando registros…");
    try {
      const data = await api("journey_index", { ...base(), ...timeArgs(), field: view.field, offset: view.page * 50, limit: 50, sort: view.sort, includeSingles: view.singles });
      if (!isActive(token) || version !== view.indexToken) return;
      const lastPage = Math.max(0, Math.ceil(data.total / 50) - 1);
      if (view.page > lastPage) { view.page = lastPage; view.listScroll = 0; return loadIndex(); }
      list.innerHTML = ""; const rows = el("div", "journey-list"); list.append(rows);
      const known = currentField(), coverage = known?.coverage == null ? "" : ` · ${(known.coverage * 100).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}% com a chave`;
      status(`${view.scope === "case" ? "Caso atual" : "Logs abertos"}${backendFilters().length ? " · com filtros" : ""} · ${fmtNum(data.total)} ${heuristic() ? "valores no período" : "jornadas"}${heuristic() ? "" : coverage}${data.missing_key ? ` · ${fmtNum(data.missing_key)} sem chave` : ""}${data.skipped_keys ? ` · ${fmtNum(data.skipped_keys)} chaves muito longas omitidas` : ""}${data.complete === false ? " · resultado parcial" : ""}`, data.complete === false || data.skipped_keys > 0);
      for (const group of data.groups || []) {
        const item = button("", () => selectValue(group.value, group), "journey-item"); item.dataset.journeyValue = group.value; item.setAttribute("aria-current", String(view.selected?.value === group.value)); item.title = group.value;
        item.append(el("strong", "", group.value)); const meta = el("div", "journey-item-meta");
        meta.append(el("span", "", `${fmtNum(group.count)} registros`), el("small", group.errors ? "journey-errors" : "", group.errors ? `${fmtNum(group.errors)} erros` : duration(group.duration_ms)));
        item.append(meta, el("span", "journey-origin", `${group.sources.join(" · ")}${group.sources_limited ? " · …" : ""}`));
        if (group.errors) item.append(el("small", "journey-origin", duration(group.duration_ms))); rows.append(item);
        if (view.selected?.value === group.value) view.selected.group = group;
      }
      if (!data.groups?.length) rows.append(el("div", "journey-empty", view.singles ? "Nenhum registro com esta chave no recorte." : "Nenhuma chave repetida. Inclua isolados ou escolha outro campo."));
      const current = view;
      rows.onscroll = () => { current.listScroll = rows.scrollTop; };
      pager(list, view.page, data.total, 50, page => { view.page = page; view.listScroll = 0; loadIndex(); });
      rows.scrollTop = current.listScroll;
    } catch (error) { if (isActive(token) && version === view.indexToken) failure(list, error, loadIndex); }
  }
  function selectValue(value, group = null) {
    view.selected = { value, group }; view.detailPage = 0; view.detailScroll = 0;
    host.querySelectorAll(".journey-item").forEach(item => item.setAttribute("aria-current", String(item.dataset.journeyValue === value))); loadDetail();
  }
  async function loadDetail() {
    if (!view.selected) return;
    const token = view.token, version = ++view.detailToken, detail = host.querySelector(".journey-detail"), selected = view.selected;
    detail.innerHTML = "";
    const head = el("header", "journey-detail-head"), title = el("div", "journey-detail-title"), name = el("strong", "", selected.value); name.title = selected.value;
    const copy = button("", () => navigator.clipboard.writeText(selected.value).then(() => toast("Identificador copiado.")).catch(() => toast("Não foi possível copiar.", "err")), "icon-btn"); copy.innerHTML = '<i class="fas fa-copy"></i>'; copy.title = "Copiar identificador"; copy.setAttribute("aria-label", copy.title); title.append(name, copy); head.append(title);
    const metadata = el("div", "journey-detail-meta", view.field); head.append(metadata);
    const actions = el("div", "journey-detail-actions");
    const explore = button("Ver registros", () => { const filters = [{ column: view.field, op: "equals_exact", value: selected.value, value2: null }]; if (heuristic()) filters.push({ column: "timestamp", op: "between", value: String(view.from), value2: String(view.to) }); window.Discovery.applySelection(filters, view.scope, true); }); actions.append(explore); head.append(actions); detail.append(head);
    const records = el("div", "journey-records"); detail.append(records);
    if (heuristic() && !hasWindow()) { records.append(el("div", "journey-empty", "Escolha um período para consultar relações por usuário ou IP.")); explore.disabled = true; return; }
    busy(records, "Lendo a sequência…");
    try {
      const data = await api("journey_events", { ...base(), ...timeArgs(), field: view.field, value: selected.value, offset: view.detailPage * 100, limit: 100 });
      if (!isActive(token) || version !== view.detailToken) return;
      const lastPage = Math.max(0, Math.ceil(data.total / 100) - 1);
      if (view.detailPage > lastPage) { view.detailPage = lastPage; view.detailScroll = 0; return loadDetail(); }
      records.innerHTML = ""; const group = selected.group;
      metadata.textContent = `${view.field} · ${fmtNum(data.total)} registros${group && !heuristic() ? ` · ${duration(group.duration_ms)}` : ""}${data.missing_time ? ` · ${fmtNum(data.missing_time)} sem horário` : ""}${data.complete === false ? " · parcial" : ""}`;
      for (const [index, event] of (data.rows || []).entries()) {
        const row = button("", async () => {
          const scope = view.scope;
          if (scope === "dataset") await openDetail(event.id);
          else { const events = caseEvents(), original = (event.event_ref && events.find(candidate => candidate.event_ref === event.event_ref)) || events.find(candidate => candidate.id === event.id && candidate.timestamp === event.timestamp); showDetail(original || event); }
          if (!isActive(token) || version !== view.detailToken) return;
          drawerSource = { event: state.currentDetailEv, scope }; $("#dr-prev").hidden = $("#dr-next").hidden = true;
        }, "journey-record"); row.style.setProperty("--event-color", levelColor(event.level)); row.title = `${fullTime(event.timestamp)} · ${event.source}\n${event.message || event.name || ""}`;
        if (view.seed && (event.event_ref && event.event_ref === view.seed.event_ref || event.id === view.seed.id && event.timestamp === view.seed.timestamp && event.source === view.seed.source)) row.classList.add("journey-focus");
        const time = el("span", "journey-time", event.timestamp == null ? "Sem horário" : new Intl.DateTimeFormat("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(event.timestamp));
        time.append(el("small", "", event.timestamp == null ? "" : new Intl.DateTimeFormat("pt-BR", { day: "2-digit", month: "2-digit" }).format(event.timestamp)));
        const previous = data.rows[index - 1]; if (previous?.timestamp != null && event.timestamp != null) time.append(el("small", "", `+${duration(event.timestamp - previous.timestamp)}`));
        const body = el("span", "journey-record-body"); body.append(el("strong", "", event.name || event.code || "Registro"), el("small", "", `${event.source || "Sem origem"} · ${event.level || "Sem nível"}`), el("p", "", event.message || event.description || "")); row.append(time, el("i", "journey-dot"), body); records.append(row);
      }
      if (!data.rows?.length) records.append(el("div", "journey-empty", "Nenhum registro com este valor e período no recorte atual."));
      if (data.rows_clipped > 0) detail.append(el("p", "journey-preview-note", "Prévia reduzida; abra o registro para ver o conteúdo completo."));
      const current = view;
      records.onscroll = () => { current.detailScroll = records.scrollTop; };
      pager(detail, view.detailPage, data.total, 100, page => { view.detailPage = page; view.detailScroll = 0; loadDetail(); });
      records.scrollTop = current.detailScroll;
    } catch (error) { if (isActive(token) && version === view.detailToken) failure(records, error, loadDetail); }
  }
  async function render(container) {
    syncContext();
    const token = view.token = ++generation; view.detailToken++; view.indexToken = 0;
    const signature = JSON.stringify([view.scope, view.scope === "case" ? caseSig() : [state.currentArtifact?.id, state.currentArtifact?.loadedAt], backendFilters()]);
    if (signature !== view.signature && !view.restorePending) { view.page = 0; view.selected = null; view.listScroll = view.detailScroll = 0; }
    view.signature = signature; view.restorePending = false;
    container.innerHTML = ""; host = el("div", "journey-workspace"); container.append(host);
    busy(host, "Identificando chaves de correlação…");
    try {
      const fields = await api("journey_fields", base());
      if (!isActive(token)) return;
      view.fields = [...fields, ...view.customFields.filter(custom => !fields.some(field => field.field === custom.field))];
      const seedFields = view.seedPending && view.seed ? view.fields.filter(field => valueOf(view.seed, field.field) != null) : [];
      const chosen = preferred(seedFields);
      if (chosen) view.field = chosen.field;
      else if (!view.fields.some(field => field.field === view.field)) view.field = preferred(view.fields)?.field || "";
      if (view.seed && chosen) { const value = valueOf(view.seed, view.field); if (heuristic() && view.seed.timestamp != null) { view.from = view.seed.timestamp - 300000; view.to = view.seed.timestamp + 300000; } view.selected = { value }; view.detailPage = 0; }
      view.seedPending = false;
      draw();
      await Promise.allSettled([loadIndex(), view.selected ? loadDetail() : Promise.resolve()]);
    } catch (error) { if (isActive(token)) failure(host, error, () => render(container)); }
  }
  async function open({ event = null, scope = null } = {}) {
    const nextScope = scope || globalScope();
    if (nextScope !== globalScope()) {
      if (window.WorkspaceContext) await window.WorkspaceContext.setScope(nextScope);
      else state.analyticsScope = nextScope;
    }
    syncContext();
    if (event) { view.seed = event; view.seedPending = true; view.selected = null; resetRange(); view.page = 0; view.field = ""; view.listScroll = view.detailScroll = 0; }
    await window.Workspace.showPage("journeys");
  }
  const nav = button("", () => open()); nav.dataset.page = "journeys"; nav.innerHTML = '<i class="fas fa-route"></i>Jornadas'; document.querySelector('.nav-pages [data-page="case-timeline"]').after(nav);
  const oldDetail = showDetail;
  showDetail = function(event, ...args) { drawerSource = { event, scope: globalScope() }; return oldDetail(event, ...args); };
  if (typeof caseTimelineCallbacks !== "undefined") caseTimelineCallbacks.detail = event => { showDetail(event); drawerSource = { event, scope: "case" }; };
  const investigate = document.querySelector("#ws-detail-follow"); investigate.innerHTML = '<i class="fas fa-route"></i> Investigar daqui'; investigate.title = "Seguir identificadores deste registro entre as origens";
  investigate.onclick = () => { const event = state.currentDetailEv; if (event) open({ event, scope: drawerSource?.event === event ? drawerSource.scope : globalScope() }); };
  new MutationObserver(() => { investigate.hidden = !state.currentDetailEv || !!document.querySelector(".drawer-loading") || document.querySelector("#drawer").hidden; }).observe(document.querySelector("#drawer-badges"), { childList: true });
  return { open, render, capture, restore, refresh: () => { if (host?.isConnected && document.body.dataset.page === "journeys") return render(host.parentElement); } };
})();
