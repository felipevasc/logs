/* Two workspaces share the engines, never their active selection or visual state. */
window.WorkspaceContext = (() => {
  "use strict";
  let scope = "dataset", changing = false, generation = 0, restoringCase = false, initialized = false, caseGeneration = 0;
  let sourceBusy = 0, sourceQueue = Promise.resolve(), caseReturnScope = "dataset";
  const states = new Map(), runtime = new Map();
  const key = (value = scope) => `${activeCase()?.id || "none"}:${value}`;
  const copy = value => structuredClone(value);
  const stateKeys = ["filters", "quick", "visibleCols", "colWidths", "sortCol", "sortDir", "page", "pageSize", "groupCol", "aggs", "activeDatasetTab", "analysisView", "dashboardCompact", "favoriteFields"];
  const runtimeKeys = ["loaded", "rows", "total", "columns", "dataPeriod", "facetData", "explorerCache", "queryError"];
  const scrollSelectors = ["#workspace-home", "#table-scroll", ".table-wrap", "#workspace-side", ".ct-scroll", ".journey-list", ".journey-records", "#view-dashboard", "#view-cube"];
  function defaults() {
    return { page: "summary", values: { filters: [], quick: "", visibleCols: ["timestamp", "level", "code", "name", "message"], colWidths: {}, sortCol: "timestamp", sortDir: "desc", page: 0, pageSize: 100, groupCol: "level", aggs: [{ func: "count", column: "*", alias: "Registros" }], activeDatasetTab: "table", analysisView: "vtimeline", dashboardCompact: false, favoriteFields: [] }, tree: [], density: "comfortable", wrap: "false", sideCollapsed: false, scroll: {}, discovery: { mode: "overview", showVolume: true } };
  }
  const record = value => value && typeof value === "object" && !Array.isArray(value) ? value : {};
  const strings = value => Array.isArray(value) ? value.filter(item => typeof item === "string").slice(0, 500) : [];
  const integer = (value, fallback, min = 0, max = 1000000) => Number.isInteger(value) && value >= min && value <= max ? value : fallback;
  const validFilters = value => Array.isArray(value) ? value.filter(item => item && typeof item.column === "string" && typeof item.op === "string").slice(0, 200).map(item => ({ ...item, ...(item.value != null ? { value: String(item.value) } : {}), ...(item.value2 != null ? { value2: String(item.value2) } : {}) })) : [];
  function sanitize(raw) {
    const base = defaults(), input = record(raw), values = record(input.values), snapshot = { ...base, ...input, values: { ...base.values, ...values }, scroll: {} };
    snapshot.page = ["summary", "timeline", "case-timeline", "case-trails", "journeys", "explore", "compare", "evidence", "sources"].includes(input.page) ? input.page : "summary";
    const v = snapshot.values;
    v.filters = validFilters(values.filters); v.quick = typeof values.quick === "string" ? values.quick : "";
    v.visibleCols = Array.isArray(values.visibleCols) ? strings(values.visibleCols) : base.values.visibleCols; v.favoriteFields = strings(values.favoriteFields);
    v.colWidths = Object.fromEntries(Object.entries(record(values.colWidths)).filter(([, width]) => Number.isFinite(width) && width >= 30 && width <= 3000));
    for (const name of ["sortCol", "groupCol"]) if (typeof v[name] !== "string") v[name] = base.values[name];
    v.sortDir = v.sortDir === "asc" ? "asc" : "desc"; v.page = integer(v.page, 0); v.pageSize = integer(v.pageSize, 100, 1, 2000);
    v.activeDatasetTab = ["table", "group", "dashboard", "cube"].includes(v.activeDatasetTab) ? v.activeDatasetTab : "table";
    v.analysisView = ["overview", "items", "timeline", "vtimeline", "timeline-table", "data"].includes(v.analysisView) ? v.analysisView : "vtimeline";
    v.dashboardCompact = !!v.dashboardCompact;
    v.aggs = Array.isArray(v.aggs) ? v.aggs.filter(item => item && AGG_FUNCS.some(([func]) => func === item.func) && typeof item.column === "string").slice(0, 20).map(item => ({ func: item.func, column: item.column, alias: typeof item.alias === "string" ? item.alias : item.func })) : base.values.aggs;
    if (!v.aggs.length) v.aggs = base.values.aggs;
    snapshot.tree = strings(input.tree); snapshot.cubeCollapsed = Array.isArray(input.cubeCollapsed) ? input.cubeCollapsed.filter(path => Array.isArray(path) && path.every(part => part == null || ["string", "number", "boolean"].includes(typeof part))).slice(0, 1000) : [];
    snapshot.density = ["compact", "comfortable"].includes(input.density) ? input.density : "comfortable"; snapshot.wrap = input.wrap === "true" ? "true" : "false";
    for (const [selector, point] of Object.entries(record(input.scroll))) if (scrollSelectors.includes(selector) && Array.isArray(point) && point.length === 2 && point.every(n => Number.isFinite(n) && n >= 0)) snapshot.scroll[selector] = point;
    const w = record(input.workspace), selection = value => ({ filters: validFilters(value?.filters), quick: typeof value?.quick === "string" ? value.quick : "" });
    snapshot.workspace = { history: Array.isArray(w.history) ? w.history.slice(-30).map(selection) : [], previousSelection: selection(w.previousSelection), lastFilters: typeof w.lastFilters === "string" ? w.lastFilters : "", timeline: record(w.timeline) };
    const workbench = record(input.workbench), group = record(workbench.group), pivot = record(workbench.pivot);
    snapshot.workbench = { group: { search: typeof group.search === "string" ? group.search : "", sort: typeof group.sort === "string" ? group.sort : null, direction: group.direction === 1 ? 1 : -1, page: integer(group.page, 0) }, pivot: { search: typeof pivot.search === "string" ? pivot.search : "", page: integer(pivot.page, 0), columnPage: integer(pivot.columnPage, 0), heat: pivot.heat !== false, sort: typeof pivot.sort === "string" ? pivot.sort : "tree", tableKey: "" } };
    return snapshot;
  }
  function capture() {
    const snapshot = { page: document.body.dataset.page || "summary", values: Object.fromEntries(stateKeys.map(name => [name, copy(state[name])])), tree: [...state.treeCollapsed], cubeCollapsed: [...cubeState.collapsed], density: document.body.dataset.density, wrap: document.body.dataset.wrap, sideCollapsed: document.querySelector(".shell").classList.contains("side-collapsed"), scroll: {}, discovery: window.Discovery?.capture(), workbench: window.WorkspaceAnalysis?.capture(), workspace: window.Workspace?.capture(), journeys: window.Journeys?.capture() };
    for (const selector of scrollSelectors) { const node = document.querySelector(selector); if (node) snapshot.scroll[selector] = [node.scrollLeft, node.scrollTop]; }
    states.set(key(), snapshot); runtime.set(key(), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
    const c = activeCase(); if (c) { c.workspace ||= defaultCaseWorkspace(); c.workspace.contextStates = record(c.workspace.contextStates); c.workspace.contextStates[scope] = snapshot; c.workspace.activeScope = scope; }
    return snapshot;
  }
  function stored(value) { return sanitize(states.get(key(value)) || activeCase()?.workspace?.contextStates?.[value]); }
  function apply(snapshot) {
    const base = defaults();
    for (const name of stateKeys) state[name] = copy(snapshot.values?.[name] ?? base.values[name]);
    for (const name of ["filters", "visibleCols", "aggs", "favoriteFields"]) if (!Array.isArray(state[name])) state[name] = copy(base.values[name]);
    state.quick = String(state.quick || "");
    Object.assign(state, runtime.get(key()) || {});
    if (scope === "case") {
      const rows = caseEvents(); state.loaded = rows.length > 0; state.columns = [...caseEventsCache.summary.columns]; state.total = rows.length;
      if (!runtime.has(key())) { state.rows = []; state.dataPeriod = null; state.facetData = null; state.explorerCache = null; state.queryError = null; }
    }
    state.visibleCols = state.visibleCols.filter(column => state.columns.includes(column)); if (!state.visibleCols.includes("timestamp")) state.visibleCols.unshift("timestamp");
    if (!state.columns.includes(state.groupCol)) state.groupCol = "level";
    state.stationAnalyticsId = null; state.activeContext = scope === "case" ? "case" : "artifact"; state.analyticsScope = scope;
    $("#explore-tree").dataset.treeScope = scope;
    state.treeAgg[scope] = null; state.treeAggSig[scope] = null; treeAggVersion.dataset++; treeAggVersion.case++;
    state.treeCollapsed = new Set(snapshot.tree || []);
    cubeState.collapsed = new Set(snapshot.cubeCollapsed || []); cubeState.requestVersion++;
    document.body.dataset.density = snapshot.density || "comfortable"; document.body.dataset.wrap = snapshot.wrap || "false";
    document.querySelector(".shell").classList.toggle("side-collapsed", !!snapshot.sideCollapsed);
    const sideToggle = $("#btn-side-toggle"); sideToggle.innerHTML = `<i class="fas fa-chevron-${snapshot.sideCollapsed ? "left" : "right"}"></i>`; sideToggle.title = snapshot.sideCollapsed ? "Mostrar campos" : "Recolher campos"; sideToggle.setAttribute("aria-label", sideToggle.title); sideToggle.setAttribute("aria-expanded", String(!snapshot.sideCollapsed));
    $("#quick-search").value = state.quick;
    window.Workspace?.restore(snapshot.workspace); window.Discovery?.restore(snapshot.discovery); window.WorkspaceAnalysis?.restore(snapshot.workbench);
    window.Journeys?.restore(snapshot.journeys);
    fillColumnControls(); renderChips(); renderExploreTree(); updateContextBar();
  }
  function updateToggle() {
    document.documentElement.dataset.workspace = scope; document.body.dataset.workspace = scope;
    for (const button of document.querySelectorAll("[data-workspace-scope]")) { const selected = button.dataset.workspaceScope === scope; button.setAttribute("aria-pressed", String(selected)); button.classList.toggle("selected", selected); }
    $("#context-toggle").setAttribute("aria-label", `Área de trabalho: ${scope === "case" ? "Caso" : "Análise"}`);
    $("#context-case-count").textContent = String(caseEvents().length || "");
    for (const node of document.querySelectorAll(".nav-import,.nav-bottom [data-page='sources'],#ws-remote,#ws-remote-page")) node.hidden = scope === "case";
    $(".nav-pages [data-page='evidence']").hidden = scope !== "case";
  }
  async function setScope(next, options = {}) {
    if (!["dataset", "case"].includes(next)) return;
    if (sourceBusy && !options.internal) { toast("Atualizando as fontes externas. A troca de área estará disponível em instantes.", "info"); return; }
    if (state.loadOverlay && !options.force) { toast("Aguarde a abertura dos logs para trocar de área.", "info"); return; }
    if (next === scope && !options.force && !changing) { if (options.tab) state.activeDatasetTab = options.tab; if (options.page) await Workspace.showPage(options.page); return; }
    const request = ++generation, previousScope = scope;
    if (!options.skipCapture) capture();
    const snapshot = stored(next), page = options.page || snapshot.page || "summary";
    detailRequest++; state.refreshVersion++; clearTimeout(debounceTimer); closeDrawer(); closeCtxMenu();
    state.currentDetailEv = null; state.detailSourceSpec = null;
    document.querySelectorAll(".ctx-menu,.filter-pop").forEach(node => { node.hidden = true; });
    changing = true;
    let render = Promise.resolve();
    const update = () => {
      if (request !== generation) return;
      scope = next; apply(snapshot); if (options.tab) state.activeDatasetTab = options.tab; updateToggle();
      finishOperation(scope === "case" ? "Caso" : "Análise", scope === "case" ? `${fmtNum(caseEvents().length)} registros preservados no Caso` : `${fmtNum(state.total)} registros na Análise`);
      document.dispatchEvent(new CustomEvent("workspace-context-change", { detail: { scope, previousScope } }));
      render = Workspace.showPage(page === "sources" && scope === "case" ? "summary" : page).then(async () => {
        if (request !== generation) return;
        for (const [selector, [left, top]] of Object.entries(snapshot.scroll || {})) { const node = document.querySelector(selector); if (node) { node.scrollLeft = left; node.scrollTop = top; } }
      });
    };
    const animate = options.animate !== false && !matchMedia("(prefers-reduced-motion: reduce)").matches;
    document.documentElement.dataset.switchDirection = next === "case" ? "to-case" : "to-analysis";
    try {
      if (animate && document.startViewTransition) { const transition = document.startViewTransition(update); await transition.updateCallbackDone; transition.finished.catch(() => {}); }
      else { update(); if (animate) { const node = [...document.querySelectorAll("#workspace-home,.shell,#view-analysis")].find(node => !node.hidden); node?.animate([{ transform: `translateX(${next === "case" ? "100%" : "-12%"})`, opacity: .3 }, { transform: "translateX(0)", opacity: 1 }], { duration: 220, easing: "ease-out" }); } }
      await render;
      if (request === generation && activeCase()) { activeCase().workspace.activeScope = scope; saveCases(); }
    } finally { if (request === generation) changing = false; }
  }
  async function changeCase(id) {
    if (sourceBusy) { $("#case-select").value = state.cases.active || ""; toast("Aguarde a atualização das fontes para trocar de Caso.", "info"); return; }
    if (!restoringCase) caseReturnScope = scope;
    const request = ++caseGeneration, previousScope = caseReturnScope; capture(); restoringCase = true;
    try {
      if (scope === "case") await setScope("dataset", { animate: false });
      if (request !== caseGeneration) return;
      state.cases.active = id; state.activeStationId = null; state.stationAnalyticsId = null;
      renderCaseBar(); updateAnalysisBadge(); await syncActiveCaseArtifacts();
      if (request !== caseGeneration) return;
      runtime.set(key("dataset"), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
      await setScope(previousScope, { force: true, animate: false, skipCapture: true });
    } finally { if (request === caseGeneration) restoringCase = false; }
  }
  function beforeCaseCreation() {
    if (!initialized) return null;
    capture(); restoringCase = true; caseGeneration++; generation++; state.refreshVersion++; detailRequest++;
    return { scope, snapshot: stored("dataset"), runtime: runtime.get(key("dataset")), artifacts: copy(activeCase()?.artifacts || []), activeArtifactId: activeCase()?.activeArtifactId || null };
  }
  async function afterCaseCreation(previous) {
    if (!previous) return;
    const c = activeCase();
    if (previous.runtime) runtime.set(key("dataset"), previous.runtime);
    states.set(key("dataset"), previous.snapshot); c.workspace.contextStates = { dataset: previous.snapshot }; c.workspace.activeScope = previous.scope;
    try { await setScope(previous.scope, { force: true, animate: false, skipCapture: true }); }
    finally { restoringCase = false; }
  }
  async function deleteCase(c) {
    if (sourceBusy) { toast("Aguarde a atualização das fontes para excluir o Caso.", "info"); return; }
    const target = state.cases.cases.find(item => item.id !== c.id)?.id;
    if (target) await changeCase(target);
    else { const prior = beforeCaseCreation(); state.cases.cases = []; state.cases.active = null; newCase("Caso 1", { keepArtifact: true, contextSnapshot: prior }); }
    state.cases.cases = state.cases.cases.filter(item => item.id !== c.id); state.artifactSessions.delete(c.id); states.delete(`${c.id}:dataset`); states.delete(`${c.id}:case`); runtime.delete(`${c.id}:dataset`); runtime.delete(`${c.id}:case`);
    renderCaseBar(); updateAnalysisBadge(); await saveCases();
  }
  async function initialize() {
    const target = activeCase()?.workspace?.activeScope === "case" ? "case" : "dataset";
    runtime.set(key("dataset"), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
    if (!activeCase()?.workspace?.contextStates?.dataset) capture();
    initialized = true;
    await setScope(target, { force: true, skipCapture: true, animate: false });
  }
  function sourceChanged(update) {
    sourceBusy++;
    const job = sourceQueue.catch(() => {}).then(async () => {
      const previous = scope;
      if (scope === "case") await setScope("dataset", { force: true, animate: false, internal: true });
      try { await update(); }
      finally { capture(); if (previous === "case") await setScope("case", { force: true, skipCapture: true, animate: false, internal: true }); }
    });
    sourceQueue = job.finally(() => { sourceBusy--; });
    return sourceQueue;
  }
  async function replaceCases(store) {
    await sourceQueue.catch(() => {});
    restoringCase = true; initialized = false; caseGeneration++; generation++; state.refreshVersion++; detailRequest++;
    try {
      scope = "dataset"; state.analyticsScope = "dataset"; state.activeContext = "artifact";
      state.cases = store; state.artifactSessions = new Map(); states.clear(); runtime.clear();
      renderCaseBar(); updateAnalysisBadge(); await syncActiveCaseArtifacts(); await initialize();
    } finally { restoringCase = false; }
  }
  let membershipSignature = "", refs = new Set(), identities = new Set();
  const identity = (event, artifact = state.currentArtifact?.id) => JSON.stringify([event.fields?.caminho || artifact || "", event.id, event.timestamp, event.source, event.code, event.message]);
  function indexMembership() {
    const signature = caseSig(); if (signature === membershipSignature) return;
    membershipSignature = signature; refs = new Set(); identities = new Set();
    for (const item of activeCase()?.items || []) for (const row of item.rows || []) { if (row.event_ref) refs.add(row.event_ref); identities.add(identity(row, item.artifactId)); }
  }
  function isIncluded(event) { indexMembership(); return !!event.event_ref && refs.has(event.event_ref) || identities.has(identity(event)); }
  function refreshMembership() {
    membershipSignature = ""; updateToggle();
    if (scope === "dataset") for (const node of document.querySelectorAll("#events-table tbody tr[data-event-id]")) { const event = state.rows.find(row => row.id === Number(node.dataset.eventId)), included = event && isIncluded(event); node.classList.toggle("event-in-case", !!included); if (included) node.title = "Este registro já está no Caso"; else node.removeAttribute("title"); }
  }
  const oldDetail = showDetail;
  showDetail = function(...args) { const result = oldDetail(...args); $("#ws-detail-save").hidden = scope === "case"; if (scope === "dataset") { const included = isIncluded(args[0]); $("#ws-detail-save").classList.toggle("event-in-case-action", included); $("#ws-detail-save").title = included ? "Este registro já está no Caso" : "Salvar no Caso"; } return result; };
  for (const button of document.querySelectorAll("[data-workspace-scope]")) button.onclick = () => setScope(button.dataset.workspaceScope);
  const originalSave = saveCases;
  saveCases = function(...args) { if (initialized && !changing && !restoringCase) capture(); return originalSave(...args); };
  updateToggle();
  Promise.resolve(window.workspaceBootstrap).then(() => { if (!initialized) return initialize(); }).catch(error => toast(`Não foi possível restaurar a área de trabalho: ${error}`, "err"));
  return { scope: () => scope, setScope, changeCase, initialize, capture, sourceChanged, replaceCases, waitForSource: () => sourceQueue.catch(() => {}), beforeCaseCreation, afterCaseCreation, deleteCase, isIncluded, refreshMembership, get sourceBusy() { return !!sourceBusy; }, get ready() { return initialized; }, get changing() { return changing || restoringCase; } };
})();
