/* Two workspaces share the engines, never their active selection or visual state. */
window.WorkspaceContext = (() => {
  "use strict";
  let scope = "dataset", changing = false, generation = 0, restoringCase = false, initialized = false, caseGeneration = 0;
  let sourceBusy = 0, sourceQueue = Promise.resolve();
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
    const temporal = record(input.explorerTimeline);
    snapshot.explorerTimeline = { field: typeof temporal.field === "string" ? temporal.field : null, limit: [6, 12, 24].includes(temporal.limit) ? temporal.limit : 12, hidden: strings(temporal.hidden).slice(0, 27) };
    snapshot.queryDraft = typeof input.queryDraft?.value === "string" ? { value: input.queryDraft.value, start: input.queryDraft.start, end: input.queryDraft.end, direction: input.queryDraft.direction } : null;
    const security = record(input.security);
    snapshot.security = { minimumEvidence: integer(security.minimumEvidence, 5, 1, 5), tacticFilter: typeof security.tacticFilter === "string" && security.tacticFilter.length <= 100 ? security.tacticFilter : null, shownEpisodes: integer(security.shownEpisodes, 3, 3, 500) };
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
    snapshot.tree = strings(input.tree); snapshot.drive = strings(input.drive); snapshot.cubeCollapsed = Array.isArray(input.cubeCollapsed) ? input.cubeCollapsed.filter(path => Array.isArray(path) && path.every(part => part == null || ["string", "number", "boolean"].includes(typeof part))).slice(0, 1000) : [];
    snapshot.density = ["compact", "comfortable"].includes(input.density) ? input.density : "comfortable"; snapshot.wrap = input.wrap === "true" ? "true" : "false";
    for (const [selector, point] of Object.entries(record(input.scroll))) if (scrollSelectors.includes(selector) && Array.isArray(point) && point.length === 2 && point.every(n => Number.isFinite(n) && n >= 0)) snapshot.scroll[selector] = point;
    const w = record(input.workspace), selection = value => ({ filters: validFilters(value?.filters), quick: typeof value?.quick === "string" ? value.quick : "" });
    snapshot.workspace = { history: Array.isArray(w.history) ? w.history.slice(-30).map(selection) : [], previousSelection: selection(w.previousSelection), lastFilters: typeof w.lastFilters === "string" ? w.lastFilters : "", timeline: record(w.timeline) };
    const workbench = record(input.workbench), group = record(workbench.group), pivot = record(workbench.pivot);
    snapshot.workbench = { group: { search: typeof group.search === "string" ? group.search : "", sort: typeof group.sort === "string" ? group.sort : null, direction: group.direction === 1 ? 1 : -1, page: integer(group.page, 0) }, pivot: { search: typeof pivot.search === "string" ? pivot.search : "", page: integer(pivot.page, 0), columnPage: integer(pivot.columnPage, 0), heat: pivot.heat !== false, sort: typeof pivot.sort === "string" ? pivot.sort : "tree", configurationCollapsed: pivot.configurationCollapsed === true, tableKey: "" } };
    return snapshot;
  }
  function capture() {
    if (activeCase()?.kind === "preserved_case_unavailable") return defaults();
    const snapshot = { page: document.body.dataset.page || "summary", queryDraft: window.QueryBar?.captureDraft?.() || { value: $("#quick-search").value }, values: Object.fromEntries(stateKeys.map(name => [name, copy(state[name])])), tree: [...state.treeCollapsed], cubeCollapsed: [...cubeState.collapsed], density: document.body.dataset.density, wrap: document.body.dataset.wrap, sideCollapsed: document.querySelector(".shell").classList.contains("side-collapsed"), scroll: {}, discovery: window.Discovery?.capture(), workbench: window.WorkspaceAnalysis?.capture(), explorerTimeline: window.ExplorerTimeline?.capture(), workspace: window.Workspace?.capture(), journeys: window.Journeys?.capture(), security: window.Security?.capture() };
    for (const selector of scrollSelectors) { const node = document.querySelector(selector); if (node) snapshot.scroll[selector] = [node.scrollLeft, node.scrollTop]; }
    states.set(key(), snapshot); runtime.set(key(), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
    snapshot.drive = [...state.driveCollapsed];
    const c = activeCase(); if (c) { c.workspace ||= defaultCaseWorkspace(); c.workspace.contextStates = record(c.workspace.contextStates); c.workspace.contextStates[scope] = snapshot; c.workspace.activeScope = scope; }
    return snapshot;
  }
  // Loaded source metadata is retained only under its owning Case key.
  function sourceRuntime(source = state) {
    return { loaded: source.loaded, columns: source.columns, dataPeriod: source.dataPeriod, rows: [], total: 0, facetData: null, explorerCache: null, queryError: null };
  }
  function stored(value) { return sanitize(states.get(key(value)) || activeCase()?.workspace?.contextStates?.[value]); }
  function apply(snapshot) {
    initTheme(); repaintChartTheme(); window.UiScale?.activateCase?.();
    const base = defaults();
    for (const name of stateKeys) state[name] = copy(snapshot.values?.[name] ?? base.values[name]);
    for (const name of ["filters", "visibleCols", "aggs", "favoriteFields"]) if (!Array.isArray(state[name])) state[name] = copy(base.values[name]);
    state.quick = String(state.quick || "");
    Object.assign(state, runtime.get(key()) || {});
    if (scope === "case") {
      if (window.CaseEvidence?.active === true) {
        const summary = caseAnalysisSummary(true); state.loaded = summary.ready; state.columns = summary.columns;
      } else { const rows = caseEvents(); state.loaded = rows.length > 0; state.columns = [...caseEventsCache.summary.columns]; }
      // Saved evidence stays intact when records are excluded. Only an exact
      // count for this analysis, evidence signature and filter is a visible total.
      state.total = explorerAnalytics.get(explorerKey())?.total ?? null;
      if (!runtime.has(key())) { state.rows = []; state.dataPeriod = null; state.facetData = null; state.explorerCache = null; state.queryError = null; }
      if (window.CaseEvidence?.active === true && !caseAnalysisSummary(true).ready) state.queryError = caseAnalysisSummary(true).message;
    }
    state.visibleCols = state.visibleCols.filter(column => state.columns.includes(column)); if (!state.visibleCols.includes("timestamp")) state.visibleCols.unshift("timestamp");
    if (!state.groupCol) state.groupCol = "level";
    state.stationAnalyticsId = null; state.activeContext = scope === "case" ? "case" : "artifact"; state.analyticsScope = scope;
    $("#explore-tree").dataset.treeScope = scope;
    state.treeAgg[scope] = null; state.treeAggSig[scope] = null; state.treeAggError[scope] = null; treeAggVersion.dataset++; treeAggVersion.case++;
    state.treeCollapsed = new Set(snapshot.tree || []);
    state.driveCollapsed = new Set(snapshot.drive || []);
    cubeState.collapsed = new Set(snapshot.cubeCollapsed || []); cubeState.requestVersion++;
    document.body.dataset.density = snapshot.density || "comfortable"; document.body.dataset.wrap = snapshot.wrap || "false";
    document.querySelector(".shell").classList.toggle("side-collapsed", !!snapshot.sideCollapsed);
    const sideToggle = $("#btn-side-toggle"); sideToggle.innerHTML = `<i class="fas fa-chevron-${snapshot.sideCollapsed ? "left" : "right"}"></i>`; sideToggle.title = snapshot.sideCollapsed ? "Mostrar campos" : "Recolher campos"; sideToggle.setAttribute("aria-label", sideToggle.title); sideToggle.setAttribute("aria-expanded", String(!snapshot.sideCollapsed));
    if (window.QueryBar?.restoreDraft) window.QueryBar.restoreDraft(snapshot.queryDraft || { value: state.quick });
    else $("#quick-search").value = snapshot.queryDraft?.value ?? state.quick;
    window.Workspace?.restore(snapshot.workspace); window.Discovery?.restore(snapshot.discovery); window.WorkspaceAnalysis?.restore(snapshot.workbench);
    window.Journeys?.restore(snapshot.journeys); window.ExplorerTimeline?.restore(snapshot.explorerTimeline);
    window.Security?.restoreView?.(snapshot.security);
    fillColumnControls(); renderChips(); renderExploreTree(); updateContextBar();
    restoreVisiblePreferences();
  }
  function invalidateAnalysis(caseId) {
    for (const area of ["dataset", "case"]) {
      const cacheKey = `${caseId}:${area}`, saved = runtime.get(cacheKey);
      if (saved) runtime.set(cacheKey, { ...saved, rows: [], total: null, dataPeriod: null, facetData: null, explorerCache: null, queryError: null });
    }
  }
  function updateToggle() {
    document.documentElement.dataset.workspace = scope; document.body.dataset.workspace = scope;
    $("#context-case-count").textContent = window.CaseEvidence?.active === true ? String(caseAnalysisSummary(true).preservedCount ?? "—") : String(caseEvents().length || "");
    window.Workspace?.syncZone?.();
  }
  async function showUnavailable() {
    scope = "case"; state.analyticsScope = "case"; state.activeContext = "case";
    state.loaded = false; state.rows = []; state.total = null; state.columns = []; state.dataPeriod = null; state.facetData = null; state.explorerCache = null;
    state.derivedFields = []; state.analysisDefinitionsPending = false; state.activeStationId = null; state.stationAnalyticsId = null;
    detailRequest++; state.refreshVersion++; closeDrawer(); window.Tasks?.cancelStaleAnalysis?.();
    initialized = true; updateToggle(); await window.Workspace?.showPage("evidence");
  }
  async function setScope(next, options = {}) {
    if (activeCase()?.kind === "preserved_case_unavailable") return showUnavailable();
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
      const nativeCase = scope === "case" && window.CaseEvidence?.active === true ? caseAnalysisSummary(true) : null;
      finishOperation(scope === "case" ? "Achados" : "Análise", nativeCase ? nativeCase.ready ? `${fmtNum(nativeCase.preservedCount)} ocorrências preservadas no Caso` : nativeCase.message : scope === "case" ? `${fmtNum(caseEvents().length)} registros preservados no Caso` : `${currentCountLabel("registros")} na Análise`);
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
      if (request === generation && activeCase() && activeCase().kind !== "preserved_case_unavailable") { activeCase().workspace.activeScope = scope; saveCases(); }
    } finally { if (request === generation) changing = false; }
  }
  async function changeCase(id) {
    if (!state.cases.cases.some(item => item.id === id)) throw Error("O Caso selecionado não existe mais.");
    if (sourceBusy) { $("#case-select").value = state.cases.active || ""; toast("Aguarde a atualização das fontes para trocar de Caso.", "info"); return; }
    const request = ++caseGeneration; if (!restoringCase) capture(); restoringCase = true;
    const outgoing = activeCase(), outgoingScope = scope;
    try {
      if (scope === "case") await setScope("dataset", { animate: false });
      if (outgoing?.workspace) outgoing.workspace.activeScope = outgoingScope;
      if (request !== caseGeneration) return;
      state.cases.active = id; window.AnalysisContexts?.activate(); resetCaseSourceState(); state.activeStationId = null; state.stationAnalyticsId = null;
      renderCaseBar(); updateAnalysisBadge();
      if (activeCase()?.kind === "preserved_case_unavailable") { await showUnavailable(); return; }
      await loadDerivedFields(); if (request !== caseGeneration) return;
      await syncActiveCaseArtifacts();
      if (request !== caseGeneration) return;
      // Each Case keeps its own analysis: its filters and views come back and
      // its records are queried again, never carried over from the previous Case.
      runtime.set(key("dataset"), sourceRuntime());
      const target = activeCase()?.workspace?.activeScope === "case" ? "case" : "dataset";
      await setScope(target, { force: true, animate: false, skipCapture: true });
      if (request === caseGeneration && state.loaded) await refresh();
    } finally { if (request === caseGeneration) restoringCase = false; }
  }
  function beforeCaseCreation() {
    if (!initialized) return null;
    if (!restoringCase) capture(); restoringCase = true; caseGeneration++; generation++; state.refreshVersion++; detailRequest++;
    return { scope, snapshot: stored("dataset"), activeSnapshot: stored(scope) };
  }
  async function afterCaseCreation(previous) {
    if (!previous) return;
    const c = activeCase(), request = caseGeneration;
    const current = () => request === caseGeneration && activeCase() === c;
    // Every new Case starts from its own empty source and overview.
    resetCaseSourceState();
    const fresh = defaults();
    const target = "dataset";
    const freshCase = defaults();
    runtime.set(key("dataset"), sourceRuntime());
    states.set(key("dataset"), fresh); states.set(key("case"), freshCase);
    c.workspace.contextStates = { dataset: fresh, case: freshCase }; c.workspace.activeScope = target;
    try {
      if (window.AnalysisContexts && !await loadDerivedFields()) return;
      if (!current()) return;
      await syncActiveCaseArtifacts();
      if (!current()) return;
      runtime.set(key("dataset"), sourceRuntime());
      await setScope(target, { force: true, animate: false, skipCapture: true });
      if (current() && state.loaded) await refresh();
    } finally { if (current()) restoringCase = false; }
  }
  async function deleteCase(c) {
    if (c?.kind === "preserved_case_unavailable" || !state.cases.cases.includes(c)) { toast("Este Caso preservado não está disponível para excluir por esta ação.", "info"); return false; }
    if (sourceBusy) { toast("Aguarde a atualização das fontes para excluir o Caso.", "info"); return; }
    const target = state.cases.cases.find(item => item.id !== c.id)?.id;
    if (target) await changeCase(target);
    else { const prior = beforeCaseCreation(); state.cases.cases = []; state.cases.active = null; newCase("Caso 1", { keepArtifact: true, contextSnapshot: prior }); }
    state.cases.cases = state.cases.cases.filter(item => item.id !== c.id); state.artifactSessions.delete(c.id); states.delete(`${c.id}:dataset`); states.delete(`${c.id}:case`); runtime.delete(`${c.id}:dataset`); runtime.delete(`${c.id}:case`);
    renderCaseBar(); updateAnalysisBadge(); await saveCases();
  }
  async function initialize() {
    if (activeCase()?.kind === "preserved_case_unavailable") { await showUnavailable(); return; }
    const target = activeCase()?.workspace?.activeScope === "case" ? "case" : "dataset";
    runtime.set(key("dataset"), Object.fromEntries(runtimeKeys.map(name => [name, state[name]])));
    if (!activeCase()?.workspace?.contextStates?.dataset) {
      // No saved owner means defaults, including after replacing the active Case store.
      // Do not attach profile-wide 0.11 keys or the previous Case's live controls here.
      state.favoriteFields = []; state.dashboardCompact = false; document.body.dataset.density = "comfortable";
      capture();
    }
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
  async function replaceCases(store, { beforeReplace } = {}) {
    await sourceQueue.catch(() => {});
    beforeReplace?.();
    restoringCase = true; initialized = false; caseGeneration++; generation++; state.refreshVersion++; detailRequest++;
    try {
      scope = "dataset"; state.analyticsScope = "dataset"; state.activeContext = "artifact";
      state.cases = store; window.AnalysisContexts?.activate(); state.artifactSessions = new Map(); states.clear(); runtime.clear();
      renderCaseBar(); updateAnalysisBadge();
      if (activeCase()?.kind === "preserved_case_unavailable") { await showUnavailable(); return; }
      await loadDerivedFields(); await syncActiveCaseArtifacts(); await initialize();
    } finally { restoringCase = false; }
  }
  let nativeMembership = { key: null, state: "idle", rows: new Map() }, nativeMembershipScheduled = false;
  const nativeRowKey = row => JSON.stringify([row?.id, row?.event_ref ?? row?.eventRef]);
  let nativeMembershipKeyMemo = null;
  function nativeMembershipKey() {
    const c = activeCase(), store = state.cases, summary = store.caseEvidence?.find(entry => entry.owner?.caseId === c?.id), owner = window.AnalysisContexts?.capture();
    const inputs = [store, c, store.store?.epoch, store.store?.revision, summary?.evidenceSignature, scope, JSON.stringify(owner), state.rows, state.currentDetailEv];
    if (!nativeMembershipKeyMemo || inputs.some((value, index) => value !== nativeMembershipKeyMemo.inputs[index]))
      nativeMembershipKeyMemo = { inputs, key: JSON.stringify([inputs.slice(2, 7), state.rows.map(nativeRowKey), nativeRowKey(state.currentDetailEv)]) };
    return nativeMembershipKeyMemo.key;
  }
  function nativeMembershipRows() { const rows = new Map(); for (const row of [...state.rows, state.currentDetailEv].filter(Boolean)) if (row.event_ref || row.eventRef) rows.set(nativeRowKey(row), { id: row.id, eventRef: row.event_ref ?? row.eventRef }); return [...rows.values()]; }
  function paintMembership() {
    if (scope !== "dataset") return;
    const apply = (node, event, detail = false) => { const result = nativeMembership.rows.get(nativeRowKey(event)), included = result && result.state !== "missing";
      node.classList.toggle(detail ? "event-in-case-action" : "event-in-case", !!included);
      node.title = nativeMembership.state === "loading" ? "Confirmando se o registro está preservado no Caso" : nativeMembership.state === "unavailable" ? "Pertencimento indisponível; atualize a seleção para confirmar" : included ? result.state === "ambiguous" ? `${result.matches.length} ocorrências deste registro estão preservadas no Caso` : "Este registro já está preservado no Caso" : detail ? "Salvar em Achados" : "";
    };
    for (const node of document.querySelectorAll("#events-table tbody tr[data-event-id]")) apply(node, state.rows.find(row => row.id === Number(node.dataset.eventId)));
    if (state.currentDetailEv && !$("#drawer").hidden) apply($("#ws-detail-save"), state.currentDetailEv, true);
  }
  function scheduleNativeMembership() {
    if (nativeMembershipScheduled || scope !== "dataset") return; nativeMembershipScheduled = true;
    Promise.resolve().then(async () => {
      nativeMembershipScheduled = false; const key = nativeMembershipKey(); if (nativeMembership.key === key) return;
      const rows = nativeMembershipRows(); nativeMembership = { key, state: rows.length ? "loading" : "ready", rows: new Map() }; paintMembership(); if (!rows.length) return;
      const guard = () => window.CaseEvidence?.active === true && scope === "dataset" && nativeMembership.key === key && nativeMembershipKey() === key;
      try { const result = await nativeEvidenceServices().actions.membership(rows, { guard }); if (!guard()) return;
        nativeMembership = { key, state: "ready", rows: new Map(result.rows.map(row => [nativeRowKey(row.row), row])) }; paintMembership();
      } catch { if (guard()) { nativeMembership = { key, state: "unavailable", rows: new Map() }; paintMembership(); } }
    });
  }
  let membershipSignature = "", refs = new Set(), identities = new Set();
  const identity = (event, artifact = state.currentArtifact?.id) => JSON.stringify([event.fields?.caminho || artifact || "", event.id, event.timestamp, event.source, event.code, event.message]);
  function indexMembership() {
    if (activeCase()?.kind === "preserved_case_unavailable") { refs = new Set(); identities = new Set(); return; }
    const signature = caseSig(); if (signature === membershipSignature) return;
    membershipSignature = signature; refs = new Set(); identities = new Set();
    for (const item of activeCase()?.items || []) for (const row of item.rows || []) { if (row.event_ref) refs.add(row.event_ref); identities.add(identity(row, item.artifactId)); }
  }
  function isIncluded(event) { if (window.CaseEvidence?.active === true) { const key = nativeMembershipKey(); if (nativeMembership.key !== key) { scheduleNativeMembership(); return false; } const result = nativeMembership.rows.get(nativeRowKey(event)); return !!result && result.state !== "missing"; } indexMembership(); return !!event.event_ref && refs.has(event.event_ref) || identities.has(identity(event)); }
  function refreshMembership() {
    if (window.CaseEvidence?.active === true) { nativeMembership = { key: null, state: "idle", rows: new Map() }; updateToggle(); scheduleNativeMembership(); return; }
    membershipSignature = ""; updateToggle();
    if (scope === "dataset") for (const node of document.querySelectorAll("#events-table tbody tr[data-event-id]")) { const event = state.rows.find(row => row.id === Number(node.dataset.eventId)), included = event && isIncluded(event); node.classList.toggle("event-in-case", !!included); if (included) node.title = "Este registro já está no Caso"; else node.removeAttribute("title"); }
  }
  document.addEventListener("case-evidence-state", () => { if (window.CaseEvidence?.active === true) refreshMembership(); });
  const oldDetail = showDetail;
  showDetail = function(...args) { const result = oldDetail(...args); $("#ws-detail-save").hidden = scope === "case"; if (scope === "dataset") { const included = isIncluded(args[0]); $("#ws-detail-save").classList.toggle("event-in-case-action", included); $("#ws-detail-save").title = included ? "Este registro já está no Caso" : "Salvar em Achados"; } return result; };
  const originalSave = saveCases;
  saveCases = function(...args) { if (initialized && !changing && !restoringCase) capture(); return originalSave(...args); };
  updateToggle();
  Promise.resolve(window.workspaceBootstrap).then(() => { if (!initialized) return initialize(); }).catch(error => toast(`Não foi possível restaurar a área de trabalho: ${error}`, "err"));
  return { invalidateAnalysis, scope: () => scope, setScope, changeCase, initialize, capture, sourceChanged, replaceCases, waitForSource: () => sourceQueue.catch(() => {}), beforeCaseCreation, afterCaseCreation, deleteCase, isIncluded, refreshMembership, get sourceBusy() { return !!sourceBusy; }, get ready() { return initialized; }, get changing() { return changing || restoringCase; } };
})();
