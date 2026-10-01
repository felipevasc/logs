/* Direct, bounded presentation of server-side grouping and cross-tabulation. */
(() => {
  "use strict";
  const measureLabels = { count: "Contagem", count_distinct: "Valores únicos", sum: "Soma", avg: "Média", min: "Mínimo", max: "Máximo", string_agg: "Textos reunidos" };
  const numericKinds = new Set(["number", "duration", "bytes", "bits", "percent"]);
  const groupView = { result: null, field: null, aggs: [], search: "", sort: null, direction: -1, page: 0, busy: false, queued: false, version: 0, computedKey: "" };
  const pivotView = { search: "", page: 0, columnPage: 0, heat: true, sort: "tree", tableKey: "" };
  const pivotTask = { busy: false, queued: false };
  let groupTimer;
  let groupKeys = new WeakMap();
  const number = value => typeof value === "number" ? fmtNum(value) : String(value ?? "—");
  const percent = value => `${(value * 100).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}%`;
  const button = (label, action, className = "btn ghost small") => {
    const b = el("button", className, label); b.type = "button"; b.onclick = action; return b;
  };
  const option = (select, value, label) => { const item = el("option", "", label); item.value = value; select.append(item); };
  function calculationError(target, error, retry) {
    target.replaceChildren(el("span", "", "Não foi possível calcular."));
    const details = button("Detalhes", () => window.Discovery?.showExplanation
      ? window.Discovery.showExplanation("Falha no cálculo", String(error), details) : toast(String(error), "err"));
    target.append(details, button("Tentar novamente", retry));
  }
  const emptyFilter = (field, value, exclude = false) => ({ column: field, op: value === "(vazio)" ? (exclude ? "not_empty" : "empty") : (exclude ? "not_equals_exact" : "equals_exact"), value: value === "(vazio)" ? "" : String(value), value2: null });
  const exactFilter = (field, value, exclude = false) => ({ column: field, op: value == null ? (exclude ? "not_empty" : "empty") : (exclude ? "not_equals_exact" : "equals_exact"), value: value == null ? "" : String(value), value2: null });
  const displayGroup = value => value == null ? "(vazio)" : value === "(vazio)" ? '“(vazio)”' : String(value);
  const groupValue = row => groupKeys.has(row) ? groupKeys.get(row) : row[groupView.field] === "(vazio)" ? null : row[groupView.field];
  const fieldNames = (scope = workspaceScope()) => [...new Set([...(window.AnalysisFields?.names(scope) || state.columns), ...(scope === "case" ? [...STANDARD, ...(scopeProfiles(scope) || []).map(p => p.name)] : [])])];
  const candidates = (scope = workspaceScope()) => {
    const columns = fieldNames(scope), profiles = scopeProfiles(scope) || [];
    const best = profiles.filter(p => p.cardinality > 1 && p.cardinality <= 80 && !["time", "text", "id"].includes(p.kind))
      .sort((a, b) => ((a.empty || 0) / Math.max(1, a.sampled_events || 3000) - (b.empty || 0) / Math.max(1, b.sampled_events || 3000)) || a.cardinality - b.cardinality).map(p => p.name);
    return [...new Set([...best, ...["level", "source", "code", "name"].filter(c => columns.includes(c)), ...columns.filter(c => c !== "timestamp")])];
  };
  function scheduleGroup() {
    groupView.version++;
    clearTimeout(groupTimer);
    groupTimer = setTimeout(() => runGroup(), 300);
  }
  function measureName(agg) { return agg.func === "count" && agg.column === "*" && agg.alias === "qtd" ? "Registros" : agg.alias || `${measureLabels[agg.func] || agg.func} · ${agg.column === "*" ? "Registros" : colLabel(agg.column)}`; }
  function checkMeasures(measures, dimension) {
    const names = new Set(dimension ? [dimension] : []);
    for (const item of measures) {
      if (item.column !== "*" && window.AnalysisFields && !window.AnalysisFields.available(item.column)) { toast("Uma medida usa um campo indisponível neste contexto. Escolha outro campo.", "info"); return false; }
      if (item.column === "*" && item.func !== "count") { toast("Escolha um campo para calcular essa medida.", "info"); return false; }
      const key = item.alias || `${item.func}(${item.column})`;
      if (names.has(key)) { toast("Cada medida precisa de um nome diferente dos demais campos.", "info"); return false; }
      names.add(key);
    }
    return true;
  }

  function groupShell() {
    const panel = $("#tab-group");
    if (!panel || $("#aw-group-tools")) return;
    panel.classList.add("aw-group");
    const select = $("#group-col"); select.setAttribute("aria-label", "Campo para resumir");
    select.parentElement.querySelector("label").textContent = "Resumir por";
    select.onchange = () => { state.groupCol = select.value; groupView.page = 0; renderGroupShortcuts(); scheduleGroup(); };
    const shortcuts = el("div", "aw-shortcuts"); shortcuts.id = "aw-group-shortcuts";
    panel.querySelector(".group-editor").prepend(shortcuts);
    const add = $("#btn-add-agg"); add.textContent = "+ Medida";
    add.onclick = () => { state.aggs.push({ func: "count_distinct", column: candidates().find(field => field !== state.groupCol) || state.groupCol, alias: "" }); renderAggs(); scheduleGroup(); };
    const run = $("#btn-run-group"); run.textContent = "Atualizar"; run.onclick = () => runGroup();
    const tools = el("div", "aw-result-tools"); tools.id = "aw-group-tools";
    const search = el("input", "aw-search"); search.type = "search"; search.placeholder = "Buscar nos grupos…"; search.setAttribute("aria-label", "Buscar nos grupos calculados");
    search.oninput = () => { groupView.search = search.value; groupView.page = 0; renderGroups(); };
    const sort = el("select"); sort.id = "aw-group-order"; sort.setAttribute("aria-label", "Ordenar grupos");
    option(sort, "largest", "Mais frequentes"); option(sort, "smallest", "Menos frequentes"); option(sort, "name", "Nome do grupo");
    sort.onchange = () => { groupView.sort = sort.value === "name" ? groupView.field : groupView.result?.columns[1]; groupView.direction = sort.value === "largest" ? -1 : 1; groupView.page = 0; renderGroups(); };
    tools.append(search, sort);
    const summary = el("span", "aw-summary"); summary.id = "aw-group-summary"; summary.setAttribute("aria-live", "polite"); tools.append(summary);
    panel.querySelector(".group-hint").replaceWith(tools);
    const footer = el("div", "aw-pager"); footer.id = "aw-group-pager"; panel.append(footer);
  }
  function renderGroupShortcuts() {
    const box = $("#aw-group-shortcuts"); if (!box) return;
    box.replaceChildren(el("span", "aw-caption", "Comece por"));
    for (const field of candidates().slice(0, 5)) {
      const b = button(colLabel(field), () => { state.groupCol = field; $("#group-col").value = field; groupView.page = 0; renderGroupShortcuts(); runGroup(); }, "aw-preset");
      b.classList.toggle("active", state.groupCol === field); b.setAttribute("aria-pressed", String(state.groupCol === field)); box.append(b);
    }
  }

  renderAggs = function () {
    groupShell(); renderGroupShortcuts();
    window.AnalysisFields?.control($("#group-col"), { value: state.groupCol,
      choose: field => { state.groupCol = field; groupView.page = 0; renderAggs(); scheduleGroup(); } });
    const box = $("#agg-list"); box.replaceChildren();
    if (!state.aggs.length) state.aggs.push({ func: "count", column: "*", alias: "Registros" });
    for (const [index, agg] of state.aggs.entries()) {
      const row = el("div", "agg-row aw-measure");
      const fn = el("select"); fn.setAttribute("aria-label", `Cálculo da medida ${index + 1}`);
      for (const [value, label] of Object.entries(measureLabels)) option(fn, value, label);
      fn.value = agg.func;
      const field = el("select"); field.setAttribute("aria-label", `Campo da medida ${index + 1}`);
      if (!window.AnalysisFields) {
        if (agg.func === "count") option(field, "*", "Todos os registros");
        for (const name of fieldNames()) option(field, name, colLabel(name));
        field.value = agg.column;
      }
      fn.onchange = () => {
        if (agg.func === "count" && agg.column === "*" && ["qtd", "Registros"].includes(agg.alias)) agg.alias = "";
        agg.func = fn.value;
        if (agg.column === "*" && agg.func !== "count") agg.column = (scopeProfiles(workspaceScope()) || []).find(p => numericKinds.has(p.kind))?.name || state.groupCol;
        renderAggs(); scheduleGroup();
      };
      field.onchange = () => { agg.column = field.value; scheduleGroup(); };
      window.AnalysisFields?.control(field, { value: agg.column, fixed: agg.func === "count" ? [["*", "Todos os registros"]] : [],
        choose: value => { agg.column = value; scheduleGroup(); } });
      const alias = el("input"); alias.type = "text"; alias.placeholder = "Nome opcional"; alias.value = agg.alias || ""; alias.setAttribute("aria-label", `Nome da medida ${index + 1}`);
      alias.onchange = () => { agg.alias = alias.value.trim(); scheduleGroup(); };
      const remove = button("×", () => { state.aggs.splice(index, 1); renderAggs(); scheduleGroup(); }, "icon-btn");
      remove.title = "Remover medida"; remove.setAttribute("aria-label", "Remover medida"); remove.disabled = state.aggs.length === 1;
      row.append(fn, field, alias, remove); box.append(row);
    }
  };

  runGroup = async function ({ force = false } = {}) {
    groupShell(); clearTimeout(groupTimer);
    if (!scopeHasEvents(workspaceScope())) { groupView.version++; groupView.result = null; groupView.computedKey = ""; $("#aw-group-summary").textContent = workspaceScope() === "case" ? "Adicione registros relevantes ao Caso para resumir." : "Abra um arquivo para resumir seus registros."; $("#group-table thead").replaceChildren(); $("#group-table tbody").replaceChildren(); $("#aw-group-pager").replaceChildren(); return; }
    if (!state.groupCol) state.groupCol = state.columns[0];
    if (window.AnalysisFields && !window.AnalysisFields.available(state.groupCol)) {
      groupView.result = null; groupView.computedKey = "";
      $("#group-table thead").replaceChildren(); $("#group-table tbody").replaceChildren(); $("#aw-group-pager").replaceChildren();
      $("#aw-group-summary").textContent = "O campo salvo está indisponível neste contexto. Escolha um campo para resumir."; return;
    }
    if (!state.aggs.length) { state.aggs = [{ func: "count", column: "*", alias: "Registros" }]; renderAggs(); }
    if (!checkMeasures(state.aggs, state.groupCol)) return;

    const field = state.groupCol, aggs = state.aggs.map(a => ({ ...a }));
    const scope = workspaceScope(), source = scope === "case" ? caseSig() : state.currentArtifact?.id, filters = backendFilters();
    const signature = JSON.stringify([scope, source, state.currentArtifact?.loadedAt, filters, field, aggs, state.derivedFields]);

    if (!force && groupView.result && groupView.computedKey === signature && $("#group-table tbody").children.length > 0) {
      renderGroupShortcuts();
      return;
    }

    if (groupView.busy) { groupView.version++; groupView.queued = true; window.Tasks?.cancelLatest("group"); return; }
    groupView.busy = true;
    const version = ++groupView.version;
    const run = $("#btn-run-group"); run.disabled = true;
    $("#group-table").setAttribute("aria-busy", "true"); $("#aw-group-summary").textContent = "Calculando todos os registros do recorte…";
    $("#group-table").inert = true;
    startOperation("group", "Calculando resumo", `Por ${colLabel(field)}`);
    try {
      const result = await api("aggregate_events", { ...analyticsRequest(scope), groupColumn: field, aggs, filters }, { latest: "group" });
      if (result.error) throw new Error(result.error);
      if (version !== groupView.version || source !== (scope === "case" ? caseSig() : state.currentArtifact?.id) || signature !== JSON.stringify([workspaceScope(), scope === "case" ? caseSig() : state.currentArtifact?.id, state.currentArtifact?.loadedAt, backendFilters(), field, aggs, state.derivedFields])) return;
      groupView.result = result; groupView.field = field; groupView.aggs = aggs; groupView.computedKey = signature;
      groupKeys = new WeakMap(); if (result.group_values?.length === result.rows.length) result.rows.forEach((row, index) => groupKeys.set(row, result.group_values[index]));
      const order = $("#aw-group-order");
      order.options[0].text = aggs[0]?.func === "count" ? "Mais frequentes" : "Maior medida";
      order.options[1].text = aggs[0]?.func === "count" ? "Menos frequentes" : "Menor medida";
      if (!result.columns.includes(groupView.sort)) { groupView.sort = result.columns[1]; groupView.direction = -1; }
      order.value = groupView.sort === field ? "name" : groupView.direction < 0 ? "largest" : "smallest";
      renderGroupShortcuts(); renderGroups(); finishOperation("Resumo atualizado", `${fmtNum(result.rows.length)} grupos`);
    } catch (error) {
      if (version !== groupView.version || source !== (scope === "case" ? caseSig() : state.currentArtifact?.id) || signature !== JSON.stringify([workspaceScope(), scope === "case" ? caseSig() : state.currentArtifact?.id, state.currentArtifact?.loadedAt, backendFilters(), field, aggs, state.derivedFields])) return;
      groupView.result = null; groupView.computedKey = ""; groupPresentationCache = null;
      $("#group-table thead").replaceChildren(); $("#group-table tbody").replaceChildren(); $("#aw-group-pager").replaceChildren();
      calculationError($("#aw-group-summary"), error, () => runGroup({ force: true })); finishOperation("Falha ao resumir", String(error));
    } finally {
      groupView.busy = false; run.disabled = false; $("#group-table").setAttribute("aria-busy", "false");
      $("#group-table").inert = false;
      if (groupView.queued) { groupView.queued = false; runGroup({ force: true }); }
    }
  };

  function filterGroup(value, exclude = false) {
    state.filters.push(exactFilter(groupView.field, value, exclude)); state.page = 0; switchTab("table"); filtersChanged();
  }
  let groupPresentationCache = null;
  function groupPresentation(result, view, countKey) {
    const field = view.field, sort = view.sort, direction = view.direction, query = view.search.toLocaleLowerCase();
    const omittedRecords = Number(result.omitted_records) || 0, cached = groupPresentationCache;
    // Reuse sorting/filtering, but do not assume IPC results remain immutable.
    // This linear identity/value check also catches edits to rows in place.
    if (cached && cached.result === result && cached.sourceRows === result.rows && cached.revision === view.computedKey
      && cached.field === field && cached.sort === sort && cached.direction === direction && cached.query === query
      && cached.countKey === countKey && cached.omittedRecords === omittedRecords && cached.inputs.length === result.rows.length
      && result.rows.every((row, i) => {
        const prior = cached.inputs[i];
        return prior[0] === row && Object.is(prior[1], row[field]) && Object.is(prior[2], row[sort]) && Object.is(prior[3], countKey ? row[countKey] : null);
      })) return cached.presentation;
    const rows = result.rows.filter(row => String(row[field] ?? "").toLocaleLowerCase().includes(query));
    rows.sort((a, b) => {
      const aa = a[sort], bb = b[sort];
      const comparison = typeof aa === "number" && typeof bb === "number" ? aa - bb : String(aa ?? "").localeCompare(String(bb ?? ""), "pt-BR", { numeric: true });
      return direction * comparison;
    });
    const total = countKey ? result.rows.reduce((sum, row) => sum + (Number(row[countKey]) || 0), 0) + omittedRecords : 0;
    const max = countKey ? result.rows.reduce((n, row) => Math.max(n, Number(row[countKey]) || 0), 1) : 1;
    const presentation = { rows, total, max };
    // One result only; snapshots retain scalar references, never clone log text.
    const scalar = value => value == null || ["string", "number", "boolean", "undefined"].includes(typeof value);
    const cacheable = result.rows.length <= 50000 && result.rows.every(row => scalar(row[field]) && scalar(row[sort]) && (!countKey || scalar(row[countKey])));
    groupPresentationCache = cacheable ? { result, sourceRows: result.rows, revision: view.computedKey, field, sort, direction, query, countKey, omittedRecords, presentation,
      inputs: result.rows.map(row => [row, row[field], row[sort], countKey ? row[countKey] : null]) } : null;
    return presentation;
  }
  function renderGroups() {
    const result = groupView.result; if (!result) return;
    const field = groupView.field, countIndex = groupView.aggs.findIndex(a => a.func === "count" && a.column === "*"), countKey = countIndex >= 0 ? result.columns[countIndex + 1] : null;
    // Beyond 50 000 groups only the largest come back; their records still count in the total.
    const omitted = Number(result.omitted_groups) || 0;
    const { rows, total, max } = groupPresentation(result, groupView, countKey);
    const pageSize = 100, pages = Math.max(1, Math.ceil(rows.length / pageSize)); groupView.page = Math.min(groupView.page, pages - 1);
    const head = $("#group-table thead"), body = $("#group-table tbody"); head.replaceChildren(); body.replaceChildren();
    const hr = el("tr");
    result.columns.forEach((key, index) => {
      const th = el("th"); th.setAttribute("aria-sort", groupView.sort === key ? (groupView.direction < 0 ? "descending" : "ascending") : "none");
      const label = index ? measureName(groupView.aggs[index - 1]) : colLabel(key);
      th.append(button(`${label}${groupView.sort === key ? (groupView.direction < 0 ? " ↓" : " ↑") : ""}`, () => { groupView.direction = groupView.sort === key ? -groupView.direction : index ? -1 : 1; groupView.sort = key; groupView.page = 0; renderGroups(); }, "aw-sort")); hr.append(th);
    });
    if (countKey) hr.append(el("th", "aw-number", "% do recorte")); hr.append(el("th", "", "")); head.append(hr);
    for (const row of rows.slice(groupView.page * pageSize, (groupView.page + 1) * pageSize)) {
      const tr = el("tr"); const value = groupValue(row); tr.title = "Abrir os registros deste grupo"; tr.onclick = () => filterGroup(value);
      tr.oncontextmenu = event => { event.preventDefault(); showCtxMenu(event.clientX, event.clientY, [
        valueFilterMenuItem(field, value, tr, { op: exactFilter(field, value).op }),
        { icon: "fa-filter", label: "Abrir registros do grupo", onClick: () => filterGroup(value) },
        { icon: "fa-filter-circle-xmark", label: "Excluir este grupo do recorte", onClick: () => filterGroup(value, true) },
        { icon: "fa-briefcase", label: "Adicionar grupo ao caso…", onClick: () => openNamePop(tr, name => { const filter = exactFilter(field, value); addGroupToAnalysis(field, filter.value, name, filter.op); }) },
        { icon: "fa-circle-info", label: "Inspecionar campo", onClick: () => showFieldInspector(field) },
      ].filter(action => workspaceScope() !== "case" || action.icon !== "fa-briefcase")); };
      for (const [index, key] of result.columns.entries()) {
        const td = el("td", index ? "aw-number" : "aw-group-key", index ? number(row[key]) : displayGroup(value)); td.title = String(row[key] ?? "(vazio)");
        if (key === countKey) { td.classList.add("aw-bar-cell"); td.style.setProperty("--aw-bar", `${Math.max(0, Number(row[key]) / max * 100)}%`); }
        tr.append(td);
      }
      if (countKey) tr.append(el("td", "aw-number aw-percent", total ? percent(Number(row[countKey]) / total) : "—"));
      const action = el("td", "aw-row-action"); const open = button("Ver registros →", event => { event.stopPropagation(); filterGroup(value); }); action.append(open); tr.append(action); body.append(tr);
    }
    if (!rows.length) { const tr = el("tr"), td = el("td", "aw-empty", groupView.search ? "Nenhum grupo corresponde à busca. Altere o termo acima." : "Nenhum registro neste recorte. Revise os filtros."); td.colSpan = result.columns.length + 2; tr.append(td); body.append(tr); }
    const groupsText = omitted ? `${fmtNum(result.rows.length)} maiores de ${fmtNum(result.rows.length + omitted)} grupos` : `${fmtNum(result.rows.length)} grupos`;
    $("#aw-group-summary").textContent = `${groupsText}${countKey ? ` · ${fmtNum(total)} registros` : ""}${groupView.search ? ` · ${fmtNum(rows.length)} encontrados` : ""}`;
    const incompatible = (result.incompatible_units || []).map((count, index) => count ? `${measureName(groupView.aggs[index])} (${fmtNum(count)} valores)` : null).filter(Boolean);
    if (incompatible.length) $("#aw-group-summary").append(el("span", "aw-partial", ` · Unidades incompatíveis em ${incompatible.join(", ")}. Separe os registros por unidade para calcular essas medidas.`));
    const footer = $("#aw-group-pager"); footer.replaceChildren(el("span", "muted small", omitted ? "Busca e ordenação usam os maiores grupos calculados; refine o recorte para ver os demais." : "Busca e ordenação usam todos os grupos calculados. Clique em um grupo para investigar."));
    const prev = button("←", () => { groupView.page--; renderGroups(); }); prev.disabled = !groupView.page; prev.setAttribute("aria-label", "Grupos anteriores");
    const next = button("→", () => { groupView.page++; renderGroups(); }); next.disabled = groupView.page >= pages - 1; next.setAttribute("aria-label", "Próximos grupos");
    footer.append(prev, el("span", "", `${groupView.page + 1} / ${pages}`), next);
  }

  function pivotShell() {
    if ($("#aw-pivot-tools")) return;
    const main = $(".cube-main"); if (!main) return;
    const top = el("div", "aw-pivot-presets"); top.id = "aw-pivot-tools";
    top.append(el("span", "aw-caption", "Comece com"), button("Contagem por campo", () => pivotPreset(false), "aw-preset"), button("Cruzar dois campos", () => pivotPreset(true), "aw-preset"), button("⇄ Trocar eixos", () => {
      const cube = activeCube(); [cube.rows, cube.cols] = [cube.cols, cube.rows]; cubeState.collapsed.clear(); markCubeTableChanged(); renderCubeZones(); runCube();
    })); main.prepend(top);
    top.append($("#btn-cube-clear"));
    const tools = el("div", "aw-result-tools aw-pivot-result-tools");
    const search = el("input", "aw-search"); search.type = "search"; search.placeholder = "Buscar nas linhas…"; search.setAttribute("aria-label", "Buscar linhas do cruzamento"); search.id = "aw-pivot-search";
    search.oninput = () => { pivotView.search = search.value; pivotView.page = 0; redrawPivot(); };
    const heat = button("Mapa de calor", () => { pivotView.heat = !pivotView.heat; heat.setAttribute("aria-pressed", String(pivotView.heat)); redrawPivot(); }); heat.id = "aw-pivot-heat"; heat.setAttribute("aria-pressed", "true");
    const order = el("select"); order.id = "aw-pivot-order"; order.setAttribute("aria-label", "Ordenação do cruzamento"); option(order, "tree", "Por grupos"); option(order, "desc", "Maior primeira medida"); option(order, "asc", "Menor primeira medida");
    order.onchange = () => { pivotView.sort = order.value; pivotView.page = 0; redrawPivot(); };
    const fold = button("Recolher", () => { cubeState.collapsed = new Set((cubeState.result?.row_values || cubeState.result?.row_paths || []).filter(p => p.length === 1)); pivotView.page = 0; redrawPivot(); });
    const expand = button("Expandir", () => { cubeState.collapsed.clear(); redrawPivot(); });
    tools.append(search, order, heat, fold, expand); $("#cube-table-view").before(tools);
    const summary = el("div", "aw-pivot-summary"); summary.id = "aw-pivot-summary"; summary.setAttribute("aria-live", "polite"); tools.after(summary);
    const pager = el("div", "aw-pager"); pager.id = "aw-pivot-pager"; $(".cube-output").append(pager);
    $(".cube-hint").textContent = "Busque um campo e escolha onde usá-lo. Arrastar também funciona.";
    const input = el("input", "aw-search"); input.type = "search"; input.placeholder = "Buscar campo…"; input.setAttribute("aria-label", "Buscar campo do cruzamento"); input.id = "aw-field-search";
    input.oninput = () => { for (const row of document.querySelectorAll("#cube-field-list .cube-field")) row.hidden = !row.dataset.field.toLocaleLowerCase().includes(input.value.toLocaleLowerCase()); };
    $("#cube-field-list").before(input);
  }
  function pivotPreset(cross) {
    const fields = candidates(state.analyticsScope), cube = activeCube();
    cube.rows = fields[0] ? [fields[0]] : []; cube.cols = cross && fields[1] ? [fields[1]] : [];
    cube.values = [{ func: "count", column: "*", alias: "Registros" }]; cubeState.collapsed.clear(); markCubeTableChanged(); renderCubeZones(); runCube();
  }
  function redrawPivot() { const result = cubeResultForTable(activeCube()); if (result) renderCubeTable(activeCube(), result); }
  const originalFields = renderCubeFields;
  renderCubeFields = function () {
    pivotShell(); originalFields();
    $("#cube-info").textContent = state.analyticsScope === "case" ? "Registros visíveis do Caso · filtros ativos aplicados" : `${backendFilters().length ? "Recorte filtrado" : "Todos os registros"} do arquivo aberto`;
    const available = new Set(fieldNames(state.analyticsScope));
    for (const row of [...$("#cube-field-list").children]) if (!available.has(row.dataset.field)) row.remove();
    const existing = new Set([...$("#cube-field-list").children].map(e => e.dataset.field));
    for (const name of fieldNames(state.analyticsScope)) if (!existing.has(name)) {
      const row = el("div", "cube-field"); row.dataset.field = name; row.append(el("span", "fn", colLabel(name))); row.onclick = event => showCubeFieldActions(event, name); $("#cube-field-list").append(row);
    }
    $("#aw-field-search").dispatchEvent(new Event("input"));
  };
  const originalZones = renderCubeZones;
  renderCubeZones = function () {
    pivotShell(); originalZones(); const cube = activeCube();
    for (const zone of ["rows", "cols", "values"]) {
      const box = $(`#cz-${zone}`); const items = cube[zone];
      [...box.querySelectorAll(".cube-chip")].forEach((chip, index) => {
        const item = items[index]; chip.querySelector("span").textContent = zone === "values" ? measureName(item) : colLabel(item);
        const field = zone === "values" ? item.column : item;
        if (field !== "*" && window.AnalysisFields && !window.AnalysisFields.available(field, state.analyticsScope)) chip.querySelector("span").textContent += " (indisponível)";
        chip.querySelector(".x")?.setAttribute("aria-label", `Remover ${zone === "values" ? measureName(item) : colLabel(item)}`);
        if (zone === "values") {
          const remove = chip.querySelector(".x");
          if (remove && items.length === 1) { remove.disabled = true; remove.title = "Mantenha ao menos uma medida"; }
          chip.tabIndex = 0; chip.setAttribute("role", "button"); chip.title = "Alterar cálculo da medida";
          chip.onclick = event => { event.stopPropagation(); showCtxMenu(event.clientX, event.clientY, Object.entries(measureLabels).filter(([fn]) => item.column !== "*" || fn === "count").map(([fn, label]) => ({
            icon: "fa-calculator", label, onClick: () => { item.func = fn; item.alias = ""; markCubeTableChanged(); renderCubeZones(); runCube(); },
          }))); };
          chip.onkeydown = event => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); const rect = chip.getBoundingClientRect(); chip.onclick({ stopPropagation() {}, clientX: rect.left, clientY: rect.bottom }); } };
        }
        if (index > 0) { const move = button("←", event => { event.stopPropagation(); [items[index - 1], items[index]] = [items[index], items[index - 1]]; cubeState.collapsed.clear(); markCubeTableChanged(); renderCubeZones(); runCube(); }, "aw-chip-move"); move.title = "Mover antes"; move.setAttribute("aria-label", "Mover campo antes"); chip.prepend(move); }
      });
      const empty = box.querySelector(".cube-zone-empty"); if (empty) empty.textContent = zone === "rows" ? "Um grupo por linha" : zone === "cols" ? "Opcional: comparar lado a lado" : "Escolha uma medida";
      const select = el("select", "aw-zone-add"); select.setAttribute("aria-label", `Adicionar campo em ${zone === "rows" ? "linhas" : zone === "cols" ? "colunas" : "valores"}`);
      if (!window.AnalysisFields) {
        option(select, "", "+ Campo");
        if (zone === "values" && !items.some(v => v.func === "count" && v.column === "*")) option(select, "*", "Contar registros");
        for (const name of fieldNames(state.analyticsScope)) if (zone === "values" || !items.includes(name)) option(select, name, colLabel(name));
      }
      select.onchange = () => { if (select.value) cubeAdd(zone, select.value); }; box.append(select);
      window.AnalysisFields?.control(select, { value: "", scope: state.analyticsScope,
        exclude: zone === "values" ? [] : items,
        fixed: [["", "+ Campo"], ...(zone === "values" && !items.some(v => v.func === "count" && v.column === "*") ? [["*", "Contar registros"]] : [])],
        choose: field => { if (field) cubeAdd(zone, field); } });
    }
  };
  cubeAdd = function (zone, field) {
    if (!["rows", "cols", "values"].includes(zone) || !field) return;
    if (field !== "*" && window.AnalysisFields && !window.AnalysisFields.available(field, state.analyticsScope)) { toast("Este campo não está disponível no contexto atual.", "info"); return; }
    const cube = activeCube();
    if (zone === "values") {
      const kind = (scopeProfiles() || []).find(p => p.name === field)?.kind;
      const func = field === "*" ? "count" : numericKinds.has(kind) ? "avg" : "count_distinct";
      if (cube.values.some(v => v.func === func && v.column === field)) { toast("Essa medida já está na tabela.", "info"); return; }
      cube.values.push({ func, column: field, alias: field === "*" ? "Registros" : "" });
    } else {
      if (cube[zone].includes(field)) return; cube[zone].push(field);
    }
    markCubeTableChanged(); renderCubeZones(); runCube();
  };
  const originalRunCube = runCube;
  runCube = async function ({ force = false } = {}) {
    pivotShell(); const cube = activeCube();
    if (window.AnalysisFields && [...cube.rows, ...cube.cols].some(field => !window.AnalysisFields.available(field, state.analyticsScope))) {
      cubeState.requestVersion++; window.Tasks?.cancelLatest("pivot"); cubeState.result = null;
      for (const selector of ["#cube-table thead", "#cube-table tbody", "#aw-pivot-pager"]) $(selector)?.replaceChildren();
      $("#aw-pivot-summary").textContent = "O cruzamento salvo usa um campo indisponível neste contexto. Escolha outro campo."; return;
    }
    if (!cube.values.length) { cube.values = [{ func: "count", column: "*", alias: "Registros" }]; renderCubeZones(); }
    if (!checkMeasures(cube.values)) return;
    if (pivotTask.busy) { pivotTask.queued = true; cubeState.requestVersion++; window.Tasks?.cancelLatest("pivot"); return; }
    pivotTask.busy = true;
    $("#aw-pivot-summary").textContent = "Calculando o recorte…";
    try {
      const outcome = await originalRunCube({ force });
      if (!scopeHasEvents()) $("#aw-pivot-summary").textContent = "Adicione registros a este contexto para cruzar os campos.";
      else if (outcome?.status === "error" && !pivotTask.queued) {
        $("#aw-pivot-pager").replaceChildren();
        calculationError($("#aw-pivot-summary"), outcome.error, () => runCube({ force: true }));
      }
      else if (!cubeResultForTable(activeCube()) && !pivotTask.queued) $("#aw-pivot-summary").textContent = "Não foi possível calcular. Altere os campos para tentar novamente.";
    } finally {
      pivotTask.busy = false;
      if (pivotTask.queued) { pivotTask.queued = false; runCube({ force: true }); }
    }
  };

  function pivotFilters(cube, path, columnKey = null, columnValues = null, exact = false) {
    const filters = path.slice(0, cube.rows.length).map((value, index) => (exact ? exactFilter : emptyFilter)(cube.rows[index], value));
    if (Array.isArray(columnValues)) columnValues.forEach((value, index) => { if (cube.cols[index]) filters.push(exactFilter(cube.cols[index], value)); });
    else if (cube.cols.length === 1 && columnKey !== null) filters.push(emptyFilter(cube.cols[0], columnKey));
    return filters;
  }
  function pivotMenu(event, cube, path, columnKey = null, columnValues = null, exact = false) {
    event.preventDefault(); const scope = state.analyticsScope, filters = pivotFilters(cube, path, columnKey, columnValues, exact);
    if (!filters.length) return;
    const apply = (open = false) => {
      for (const filter of filters) if (!state.filters.some(f => f.column === filter.column && f.op === filter.op && f.value === filter.value)) state.filters.push(filter);
      state.page = 0;
      if (open) { switchView("viz"); switchTab("table"); }
      filtersChanged();
    };
    const label = cube.cols.length > 1 && !columnValues ? "Filtrar esta linha" : "Filtrar esta combinação";
    const actions = filters.map(filter => valueFilterMenuItem(filter.column, filter.value, event.currentTarget || event.target, { scope, op: filter.op }));
    actions.push({ icon: "fa-filter", label, onClick: () => apply() });
    actions.push({ icon: "fa-table-list", label: "Abrir registros correspondentes", onClick: () => apply(true) });
    actions.push({ icon: "fa-circle-info", label: `Inspecionar ${colLabel(filters[filters.length - 1].column)}`, onClick: () => showFieldInspector(filters[filters.length - 1].column) });
    showCtxMenu(event.clientX, event.clientY, actions);
  }
  let pivotPresentationCache = null;
  function pivotPresentation(result, paths, depth, view, collapsed, revision) {
    const query = view.search.toLocaleLowerCase(), sort = view.sort, collapsedKey = JSON.stringify(collapsed), cached = pivotPresentationCache;
    const measure = ri => sort === "tree" ? null : result.cells[ri]?.[0]?.[0];
    if (cached && cached.result === result && cached.paths === paths && cached.depth === depth && cached.revision === revision
      && cached.query === query && cached.sort === sort && cached.collapsedKey === collapsedKey && cached.inputs.length === paths.length
      && paths.every((path, i) => {
        const prior = cached.inputs[i];
        return prior[0] === path && prior[1].length === path.length && path.every((value, j) => Object.is(value, prior[1][j])) && Object.is(prior[2], measure(i));
      })) return cached.presentation;
    const pathKey = path => JSON.stringify(path);
    const parents = new Set(paths.filter(path => path.length > 1).map(path => pathKey(path.slice(0, -1))));
    let rows = paths.map((path, ri) => ({ path, ri })).filter(({ path }) => {
      if (!depth) return true;
      if (collapsed.some(prefix => prefix.length < path.length && prefix.every((value, i) => path[i] === value))) return false;
      return path.length === depth || !parents.has(pathKey(path)) || collapsed.some(prefix => pathKey(prefix) === pathKey(path));
    });
    if (query) rows = rows.filter(row => row.path.some(value => displayGroup(value).toLocaleLowerCase().includes(query)));
    if (sort !== "tree") rows.sort((a, b) => ((Number(result.cells[a.ri]?.[0]?.[0]) || 0) - (Number(result.cells[b.ri]?.[0]?.[0]) || 0)) * (sort === "desc" ? -1 : 1));
    const presentation = { rows, parents };
    const scalar = value => value == null || ["string", "number", "boolean", "undefined"].includes(typeof value);
    const cacheable = paths.length <= 50000 && paths.reduce((n, path) => n + path.length, 0) <= 250000
      && paths.every((path, i) => path.every(scalar) && scalar(measure(i)));
    // Keep one current view. Validation copies array structure, not field text.
    pivotPresentationCache = cacheable ? { result, paths, depth, revision, query, sort, collapsedKey, presentation,
      inputs: paths.map((path, i) => [path, path.slice(), measure(i)]) } : null;
    return presentation;
  }
  renderCubeTable = function (cube, result) {
    pivotShell();
    const key = `${state.analyticsScope}:${cube.id}:${cubeSchemaSignature(cube)}`;
    if (pivotView.tableKey !== key) { if (!pivotView.restoring) { pivotView.page = 0; pivotView.columnPage = 0; } pivotView.tableKey = key; } pivotView.restoring = false;
    const head = $("#cube-table thead"), body = $("#cube-table tbody"); head.replaceChildren(); body.replaceChildren();
    const exact = Array.isArray(result.row_values) && result.row_values.length === result.row_paths?.length;
    const pathKey = path => JSON.stringify(path), paths = exact ? result.row_values : result.row_paths || [], depth = cube.rows.length;
    const collapsed = [...cubeState.collapsed];
    const { rows, parents } = pivotPresentation(result, paths, depth, pivotView, collapsed, `${key}:${exact}`);
    const query = pivotView.search.toLocaleLowerCase();
    const pageSize = 100, pages = Math.max(1, Math.ceil(rows.length / pageSize)), names = result.value_names?.length ? result.value_names : ["Registros"], columnKeys = result.col_keys?.length ? result.col_keys : ["(total)"];
    const columnSize = Math.max(1, Math.floor(24 / names.length)), columnPages = Math.max(1, Math.ceil(columnKeys.length / columnSize));
    pivotView.page = Math.min(pivotView.page, pages - 1); pivotView.columnPage = Math.min(pivotView.columnPage, columnPages - 1);
    const columns = columnKeys.map((key, ci) => ({ key, ci })).slice(pivotView.columnPage * columnSize, (pivotView.columnPage + 1) * columnSize);
    const multiple = cube.cols.length > 0, dimensionCount = Math.max(1, depth), hr = el("tr", "cube-header-top"), measureHeader = el("tr", "cube-header-bottom");
    for (const field of depth ? cube.rows : [null]) { const th = el("th", "rowh cube-dimension-head", field ? colLabel(field) : "Registros"); th.rowSpan = multiple ? 2 : 1; hr.append(th); }
    for (const { key } of columns) {
      if (multiple) { const th = el("th", "cube-column-group", key); th.colSpan = names.length; th.title = cube.cols.map(colLabel).join(" → "); hr.append(th); }
      for (let vi = 0; vi < names.length; vi++) (multiple ? measureHeader : hr).append(el("th", "cube-measure-head", cube.values[vi] ? measureName(cube.values[vi]) : names[vi]));
    }
    head.append(hr); if (multiple) head.append(measureHeader);
    const maxima = columns.map(({ ci }) => names.map((_, vi) => rows.reduce((max, { ri }) => { const v = result.cells[ri]?.[ci]?.[vi]; return typeof v === "number" ? Math.max(max, Math.abs(v)) : max; }, 0)));
    const visibleRows = rows.slice(pivotView.page * pageSize, (pivotView.page + 1) * pageSize);
    const sameGroup = (left, right, dimension) => left?.path.length === depth && right?.path.length === depth && left.path.slice(0, dimension + 1).every((value, index) => right.path[index] === value);
    for (const [rowIndex, { path, ri }] of visibleRows.entries()) {
      const tr = el("tr", path.length < depth ? "cube-collapsed-group" : "cube-leaf-row");
      for (let di = 0; di < dimensionCount; di++) {
        if (depth && sameGroup(visibleRows[rowIndex], visibleRows[rowIndex - 1], di)) continue;
        const td = el("td", "rowh cube-dimension"); const value = depth ? path[di] : "Todos os registros";
        if (depth && path.length === depth) {
          let span = 1;
          while (sameGroup(visibleRows[rowIndex], visibleRows[rowIndex + span], di)) span++;
          td.rowSpan = span;
        }
        if (depth && di < path.length && di < depth - 1) {
          const prefix = path.slice(0, di + 1), isCollapsed = collapsed.some(p => pathKey(p) === pathKey(prefix));
          const toggle = button(isCollapsed ? "▸" : "▾", event => {
            event.stopPropagation(); const saved = [...cubeState.collapsed].find(p => pathKey(p) === pathKey(prefix));
            if (saved) cubeState.collapsed.delete(saved); else cubeState.collapsed.add(prefix); renderCubeTable(cube, result);
          }, "cube-toggle"); toggle.title = isCollapsed ? "Expandir grupo" : "Recolher grupo"; toggle.setAttribute("aria-label", `${toggle.title}: ${value}`); toggle.setAttribute("aria-expanded", String(!isCollapsed)); td.append(toggle);
        }
        td.append(document.createTextNode(value === undefined ? "Subtotal" : exact ? displayGroup(value) : String(value))); td.title = value === undefined ? "Subtotal das dimensões anteriores" : exact ? displayGroup(value) : String(value);
        if (depth && value !== undefined) td.oncontextmenu = event => pivotMenu(event, cube, path.slice(0, di + 1), null, null, exact); tr.append(td);
      }
      columns.forEach(({ key, ci }, displayedColumn) => names.forEach((_, vi) => {
        const raw = result.cells[ri]?.[ci]?.[vi], td = el("td", "cube-value aw-number", number(raw));
        if (pivotView.heat && typeof raw === "number" && Number.isFinite(raw) && maxima[displayedColumn][vi] > 0) td.style.backgroundColor = `color-mix(in srgb, var(--accent) ${Math.round(Math.abs(raw) / maxima[displayedColumn][vi] * 24)}%, transparent)`;
        td.title = "Botão direito: investigar os registros desta combinação"; td.oncontextmenu = event => pivotMenu(event, cube, depth ? path : [], cube.cols.length ? key : null, result.col_values?.[ci], exact); tr.append(td);
      })); body.append(tr);
    }
    if (!rows.length) { const tr = el("tr"), td = el("td", "aw-empty", query ? "Nenhuma linha corresponde à busca." : "Nenhum registro neste recorte."); td.colSpan = dimensionCount + columns.length * names.length; tr.append(td); body.append(tr); }
    const total = el("tr", "total cube-grand-total"), title = el("td", "rowh", "Total do recorte"); title.colSpan = dimensionCount; total.append(title);
    for (const { ci } of columns) for (let vi = 0; vi < names.length; vi++) total.append(el("td", "cube-value aw-number", number(result.totals?.[ci]?.[vi]))); body.append(total);
    const partial = result.complete === false;
    $("#aw-pivot-summary").textContent = `${fmtNum(rows.length)} linhas · ${fmtNum(columnKeys.length)} ${columnKeys.length === 1 ? "coluna" : "colunas"}${result.processed_events !== undefined ? ` · ${fmtNum(result.processed_events)} registros analisados` : ""}${partial ? " · Resultado parcial: reduza dimensões ou período." : result.truncated ? " · Exibição limitada pelo motor; totais consideram o recorte completo." : ""}`;
    const incompatible = (result.incompatible_units || []).map((count, index) => count ? `${names[index]} (${fmtNum(count)} valores)` : null).filter(Boolean);
    if (incompatible.length) $("#aw-pivot-summary").append(el("span", "aw-partial", ` · Unidades incompatíveis em ${incompatible.join(", ")}. Separe os registros por unidade para calcular essas medidas.`));
    if (partial) title.textContent = "Total analisado (parcial)";
    $("#aw-pivot-summary").classList.toggle("aw-partial", partial);
    const pager = $("#aw-pivot-pager"); pager.replaceChildren(el("span", "muted small", "Totais independem da busca e da página. Botão direito para investigar."));
    const prev = button("←", () => { pivotView.page--; redrawPivot(); }); prev.disabled = !pivotView.page; prev.setAttribute("aria-label", "Linhas anteriores");
    const next = button("→", () => { pivotView.page++; redrawPivot(); }); next.disabled = pivotView.page >= pages - 1; next.setAttribute("aria-label", "Próximas linhas");
    pager.append(prev, el("span", "", `Linhas ${pivotView.page + 1}/${pages}`), next);
    if (columnPages > 1) {
      const before = button("‹", () => { pivotView.columnPage--; redrawPivot(); }); before.disabled = !pivotView.columnPage; before.setAttribute("aria-label", "Colunas anteriores");
      const after = button("›", () => { pivotView.columnPage++; redrawPivot(); }); after.disabled = pivotView.columnPage >= columnPages - 1; after.setAttribute("aria-label", "Próximas colunas");
      pager.append(before, el("span", "", `Colunas ${pivotView.columnPage + 1}/${columnPages}`), after);
    }
  };
  const originalViews = renderCubeViews;
  renderCubeViews = function () {
    originalViews(); const chart = cubeWorkspace().activeView.type === "chart";
    for (const element of document.querySelectorAll(".aw-pivot-result-tools, #aw-pivot-summary, #aw-pivot-pager")) element.hidden = chart;
  };
  window.WorkspaceAnalysis = {
    invalidateAnalysis: () => {
      groupPresentationCache = null; pivotPresentationCache = null; groupView.version++; clearTimeout(groupTimer);
      groupView.result = null; groupView.page = 0; pivotView.page = 0;
      for (const selector of ["#group-table thead", "#group-table tbody", "#cube-table thead", "#cube-table tbody", "#aw-group-pager", "#aw-pivot-pager"]) $(selector)?.replaceChildren();
    },
    capture: () => ({ group: { search: groupView.search, sort: groupView.sort, direction: groupView.direction, page: groupView.page }, pivot: { ...pivotView } }),
    restore: saved => {
      groupPresentationCache = null; pivotPresentationCache = null;
      groupView.version++; clearTimeout(groupTimer); Object.assign(groupView, { result: null, field: null, search: "", sort: null, direction: -1, page: 0 }, saved?.group);
      Object.assign(pivotView, { search: "", page: 0, columnPage: 0, heat: true, sort: "tree", tableKey: "" }, saved?.pivot, { restoring: true });
      if ($("#aw-group-tools input")) $("#aw-group-tools input").value = groupView.search;
      if ($("#aw-pivot-search")) $("#aw-pivot-search").value = pivotView.search;
      if ($("#aw-pivot-order")) $("#aw-pivot-order").value = pivotView.sort;
      $("#aw-pivot-heat")?.setAttribute("aria-pressed", String(pivotView.heat));
      if ($("#aw-field-search")) $("#aw-field-search").value = "";
      for (const selector of ["#group-table thead", "#group-table tbody", "#cube-table thead", "#cube-table tbody", "#aw-group-pager", "#aw-pivot-pager"]) $(selector)?.replaceChildren();
      if ($("#aw-group-summary")) $("#aw-group-summary").textContent = "";
      if ($("#aw-pivot-summary")) $("#aw-pivot-summary").textContent = "";
    }
  };
  document.addEventListener?.("analysis-fields-change", () => {
    renderAggs();
    if (!$("#view-cube")?.hidden) { renderCubeFields(); renderCubeZones(); }
  });
  groupShell(); renderAggs(); pivotShell();
})();
