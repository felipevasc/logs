/* Two workspaces share the engines, never their active selection or visual state. */
window.WorkspaceContext = (() => {
  "use strict";
  let scope = "dataset", changing = false, generation = 0, restoringCase = false, initialized = false, caseGeneration = 0;
  let sourceBusy = 0, sourceQueue = Promise.resolve(), caseReturnScope = "dataset";
  const states = new Map(), runtime = new Map();
  const key = (value = scope) => `${activeCase()?.id || "none"}:${value}`;
  const copy = value => structuredClone(value);
  const stateKeys = ["filters", "quick", "visibleCols", "colWidths", "sortCol", "sortDir", "page", "pageSize", "groupCol", "aggs", "activeDatasetTab", "analysisView", "dashboardCompact", "favoriteFields"];
  const runtimeKeys = ["loaded", "rows", "total", "columns", "dataPeriod", "facetData", "explorerCache", "queryError", "sourceIdentityUnconfirmed"];
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
    snapshot.queryDraft = typeof input.queryDraft?.value === "string" ? { value: input.queryDraft.value, start: input.queryDraft.start, end: input.queryDraft.end, direction: input.queryDraft.direction } : null;
    snapshot.page = ["summary", "compromises", "timeline", "case-timeline", "case-trails", "journeys", "explore", "compare", "evidence", "sources", "connections", "import"].includes(input.page) ? input.page : "summary";
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
    const snapshot = { page: document.body.dataset.page || "summary", queryDraft: window.QueryBar?.captureDraft?.() || { value: $("#quick-search").value }, values: Object.fromEntries(stateKeys.map(name => [name, copy(state[name])])), tree: [...state.treeCollapsed], cubeCollapsed: [...cubeState.collapsed], density: document.body.dataset.density, wrap: document.body.dataset.wrap, sideCollapsed: document.querySelector(".shell").classList.contains("side-collapsed"), scroll: {}, discovery: window.Discovery?.capture(), workbench: window.WorkspaceAnalysis?.capture(), workspace: window.Workspace?.capture(), journeys: window.Journeys?.capture() };
    for (const selector of scrollSelectors) { const node = document.querySelector(selector); if (node) snapshot.scroll[selector] = [node.scrollLeft, node.scrollTop]; }
    states.set(key(), snapshot); runtime.set(key(), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
    const c = activeCase(); if (c) { c.workspace ||= defaultCaseWorkspace(); c.workspace.contextStates = record(c.workspace.contextStates); c.workspace.contextStates[scope] = snapshot; c.workspace.activeScope = scope; }
    return snapshot;
  }
  // Loaded source shared by every Case; its filtered rows belong to one Case only.
  function sourceRuntime(source = state) {
    return { loaded: source.loaded, columns: source.columns, dataPeriod: source.dataPeriod, rows: [], total: 0, facetData: null, explorerCache: null, queryError: null };
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
    state.treeAgg[scope] = null; state.treeAggSig[scope] = null; state.treeAggError[scope] = null; treeAggVersion.dataset++; treeAggVersion.case++;
    state.treeCollapsed = new Set(snapshot.tree || []);
    cubeState.collapsed = new Set(snapshot.cubeCollapsed || []); cubeState.requestVersion++;
    document.body.dataset.density = snapshot.density || "comfortable"; document.body.dataset.wrap = snapshot.wrap || "false";
    document.querySelector(".shell").classList.toggle("side-collapsed", !!snapshot.sideCollapsed);
    const sideToggle = $("#btn-side-toggle"); sideToggle.innerHTML = `<i class="fas fa-chevron-${snapshot.sideCollapsed ? "left" : "right"}"></i>`; sideToggle.title = snapshot.sideCollapsed ? "Mostrar campos" : "Recolher campos"; sideToggle.setAttribute("aria-label", sideToggle.title); sideToggle.setAttribute("aria-expanded", String(!snapshot.sideCollapsed));
    if (window.QueryBar?.restoreDraft) window.QueryBar.restoreDraft(snapshot.queryDraft || { value: state.quick });
    else $("#quick-search").value = snapshot.queryDraft?.value ?? state.quick;
    window.Workspace?.restore(snapshot.workspace); window.Discovery?.restore(snapshot.discovery); window.WorkspaceAnalysis?.restore(snapshot.workbench);
    window.Journeys?.restore(snapshot.journeys);
    fillColumnControls(); renderChips(); renderExploreTree(); updateContextBar();
    restoreVisiblePreferences();
  }
  function updateToggle() {
    document.documentElement.dataset.workspace = scope; document.body.dataset.workspace = scope;
    $("#context-case-count").textContent = String(caseEvents().length || "");
    window.Workspace?.syncZone?.();
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
      finishOperation(scope === "case" ? "Caso" : "Análise", scope === "case" ? `${fmtNum(caseEvents().length)} registros preservados no Caso` : `${currentCountLabel("registros")} na Análise`);
      document.dispatchEvent(new CustomEvent("workspace-context-change", { detail: { scope, previousScope } }));
      render = Workspace.showPage(["sources", "connections", "import"].includes(page) && scope === "case" ? "summary" : page).then(async () => {
        if (request !== generation) return;
        for (const [selector, [left, top]] of Object.entries(snapshot.scroll || {})) { const node = document.querySelector(selector); if (node) { node.scrollLeft = left; node.scrollTop = top; } }
      });
    };
    const animate = options.animate !== false && !matchMedia("(prefers-reduced-motion: reduce)").matches;
    // Zones are stacked in the menu: moving down the list slides the new area up from below.
    const order = { analysis: 0, case: 1, structure: 2 }, from = document.documentElement.dataset.zone || "analysis";
    const to = next === "case" ? "case" : ["sources", "connections", "import"].includes(page) ? "structure" : "analysis";
    const down = order[to] >= order[from];
    document.documentElement.dataset.switchDirection = down ? "down" : "up";
    try {
      if (animate && document.startViewTransition) { const transition = document.startViewTransition(update); await transition.updateCallbackDone; transition.finished.catch(() => {}); }
      else { update(); if (animate) { const node = [...document.querySelectorAll("#workspace-home,.shell,#view-analysis")].find(node => !node.hidden); node?.animate([{ transform: `translateY(${down ? "32%" : "-32%"})`, opacity: .3 }, { transform: "translateY(0)", opacity: 1 }], { duration: 240, easing: "cubic-bezier(.2,.7,.2,1)" }); } }
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
      // Each Case keeps its own analysis: its filters and views come back and
      // its records are queried again, never carried over from the previous Case.
      runtime.set(key("dataset"), sourceRuntime());
      await setScope(previousScope, { force: true, animate: false, skipCapture: true });
      if (request === caseGeneration && state.loaded) await refresh();
    } finally { if (request === caseGeneration) restoringCase = false; }
  }
  function beforeCaseCreation() {
    if (!initialized) return null;
    capture(); restoringCase = true; caseGeneration++; generation++; state.refreshVersion++; detailRequest++;
    return { scope, snapshot: stored("dataset"), activeSnapshot: stored(scope), runtime: runtime.get(key("dataset")), artifacts: copy(activeCase()?.artifacts || []), activeArtifactId: activeCase()?.activeArtifactId || null };
  }
  async function afterCaseCreation(previous) {
    if (!previous) return;
    const c = activeCase();
    // Keep the user's area; a new Case has no evidence but shares the loaded source.
    // When creating from Case, state describes saved evidence, not that source.
    const fresh = { ...defaults(), page: previous.snapshot?.page || "summary" };
    const target = previous.scope === "case" ? "case" : "dataset";
    const freshCase = { ...defaults(), page: target === "case" ? previous.activeSnapshot?.page || "summary" : "summary" };
    const source = previous.runtime || (previous.scope === "dataset" ? state : { loaded: false, columns: [], dataPeriod: null });
    runtime.set(key("dataset"), sourceRuntime(source));
    states.set(key("dataset"), fresh); states.set(key("case"), freshCase);
    c.workspace.contextStates = { dataset: fresh, case: freshCase }; c.workspace.activeScope = target;
    try {
      await setScope(target, { force: true, animate: false, skipCapture: true });
      if (state.loaded) await refresh();
    } finally { restoringCase = false; }
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
  const originalSave = saveCases;
  saveCases = function(...args) { if (initialized && !changing && !restoringCase) capture(); return originalSave(...args); };
  updateToggle();
  Promise.resolve(window.workspaceBootstrap).then(() => { if (!initialized) return initialize(); }).catch(error => toast(`Não foi possível restaurar a área de trabalho: ${error}`, "err"));
  return { scope: () => scope, setScope, changeCase, initialize, capture, sourceChanged, replaceCases, waitForSource: () => sourceQueue.catch(() => {}), beforeCaseCreation, afterCaseCreation, deleteCase, isIncluded, refreshMembership, get sourceBusy() { return !!sourceBusy; }, get ready() { return initialized; }, get changing() { return changing || restoringCase; } };
})();
