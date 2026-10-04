/* Run: node scripts/preview/test-workspace-context.mjs http://127.0.0.1:4175 */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { captureFailure } from "./diagnostics.mjs";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
const url = process.argv[2] || "http://127.0.0.1:4175", output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1024, height: 900 }, reducedMotion: "reduce" });
const errors = [], results = {};
const exactTotal = () => page.evaluate(async () => {
  if (!Number.isFinite(state.total)) await loadExplorerAnalytics(explorerKey(), workspaceScope(), backendFilters()).promise;
  return state.total;
});
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length && document.querySelector("#load-overlay").hidden);
  const initial = await page.evaluate(async () => {
    await WorkspaceContext.setScope("dataset", { page: "explore", animate: false });
    state.filters = []; state.quick = ""; await refresh();
    await loadExplorerAnalytics(explorerKey(), workspaceScope(), backendFilters()).promise;
    return { total: state.total, caseId: activeCase().id, artifactId: state.currentArtifact.id, rows: state.rows.slice(0, 3) };
  });
  assert.equal(initial.total, 6000);
  await page.evaluate(async () => { toggleTheme(); await UiScale.set(1, false); });
  const originalTheme = await page.evaluate(() => document.documentElement.dataset.theme);
  await page.evaluate(() => FieldIndexes.refresh());
  await page.evaluate(async () => { const action = FieldIndexes.menuItem("source"); await action.onClick(); });
  await page.waitForFunction(() => FieldIndexes.has("source"));
  await page.waitForFunction(() => document.querySelector('.field-row[data-column="source"].field-indexed'));
  await page.evaluate(rows => { activeCase().items = [{ id: "context-fixture", kind: "events", label: "Preservados", rows }]; caseEventsCache.sig = null; updateAnalysisBadge(); }, initial.rows);
  await page.evaluate(async () => {
    await WorkspaceContext.setScope("case", { page: "explore", animate: false });
    await refreshTreeAggs("case", { force: true });
    window.contextFacetTotals = Object.fromEntries(Object.entries(state.treeAgg.case).map(([key, values]) => [key, values.reduce((sum, value) => sum + value[1], 0)]));
    state.filters = [{ column: "source", op: "equals_exact", value: caseEvents()[0].source }]; await refresh();
    window.oldCaseFilter = structuredClone(state.filters);
    toast("Aviso pertencente ao Caso anterior", "info");
    window.createdId = newCase("Caso vazio isolado").id;
  });
  for (const total of Object.values(await page.evaluate(() => window.contextFacetTotals))) assert.ok(total <= 3, "case facets contain only saved records");
  await page.waitForFunction(() => !WorkspaceContext.changing && WorkspaceContext.scope() === "dataset" && !state.loaded);
  assert.equal(await page.locator("#explore-tree").getAttribute("data-tree-scope"), "dataset");
  assert.equal(await page.evaluate(() => caseEvents().length), 0);
  assert.doesNotMatch(await page.locator('body').innerText(), /Aviso pertencente ao Caso anterior/);
  assert.equal(await page.evaluate(() => FieldIndexes.has("source")), false, "an index from another Case is never highlighted");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "dark", "new Case starts with its own theme");
  assert.equal(await page.evaluate(() => UiScale.setting()), "auto", "new Case starts with its own scale");
  assert.equal(await page.locator('.zone-switch [data-zone="case"] > span:not(.li-loader)').textContent(), "Achados");
  assert.equal(await page.locator("#ws-empty").isVisible(), true);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.deepEqual(await page.evaluate(() => ({ loaded: state.loaded, total: state.total, artifact: state.currentArtifact, artifacts: activeCase().artifacts, columns: state.columns, path: document.querySelector("#file-path").value })),
    { loaded: false, total: 0, artifact: null, artifacts: [], columns: [], path: "" }, "new Case has no inherited source, columns or import draft");
  await page.evaluate(async original => { await WorkspaceContext.setScope("case", { animate: false }); await WorkspaceContext.changeCase(original); }, initial.caseId);
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), originalTheme);
  assert.equal(await page.evaluate(() => UiScale.setting()), 1);
  assert.equal(await page.evaluate(() => caseEvents().length), 3);
  assert.deepEqual(await page.evaluate(() => state.filters), await page.evaluate(() => window.oldCaseFilter));
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.equal(await exactTotal(), 6000);
  await page.evaluate(async () => {
    await WorkspaceContext.setScope("case", { page: "explore", animate: false });
    await mcpRefreshSource();
  });
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case", "MCP source refresh preserves active workspace");
  assert.deepEqual(await page.evaluate(() => state.filters), await page.evaluate(() => window.oldCaseFilter));
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.equal(await exactTotal(), 6000);
  results.lifecycle = "new case sources empty; original case filters/source restored; native dataset6000 reopens; MCP refresh scoped";
  // A rejected old summary must not paint over the new workspace.
  await page.evaluate(async () => {
    const original = api; window.restoreContextApi = () => { api = original; };
    let rejectOld;
    api = async (command, args, options) => command === "dataset_overview" && !args.caseEvents ? new Promise((_, reject) => { rejectOld = reject; }) : original(command, args, options);
    window.pendingOldSummary = Workspace.showPage("summary");
    await WorkspaceContext.setScope("case", { page: "summary", animate: false });
    rejectOld(new Error("Stale dataset response")); await window.pendingOldSummary; window.restoreContextApi();
  });
  assert.doesNotMatch(await page.locator("#ws-content").innerText(), /Stale dataset response/);
  // Persist the active case workspace, then verify a cold start after native source load.
  await page.evaluate(async () => { await Workspace.showPage("explore"); await refresh(); await saveCases(); });
  await exactTotal();
  const before = await page.evaluate(() => ({ caseId: activeCase().id, filters: state.filters, total: state.total }));
  await page.reload();
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && document.querySelector("#load-overlay").hidden);
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case");
  await exactTotal();
  assert.deepEqual(await page.evaluate(() => ({ caseId: activeCase().id, filters: state.filters, total: state.total })), before);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.equal(await exactTotal(), 6000);
  results.reload = "active workspace + filters restored after source rehydration";
  // Corrupt imported UI preferences must safely normalize without touching events.
  await page.evaluate(async () => {
    const store = structuredClone(state.cases), c = store.cases.find(item => item.id === store.active);
    c.workspace.activeScope = "case";
    c.workspace.contextStates = { case: { page: {}, values: { filters: [null, {}, { column: 3, op: "equals" }], visibleCols: {}, aggs: "bad", sortCol: [], sortDir: {}, page: -3, pageSize: "bad", favoriteFields: 7, colWidths: [2] }, scroll: { "#workspace-home": null }, tree: {}, cubeCollapsed: {}, workspace: { history: "bad", previousSelection: 3 }, workbench: { group: { search: {}, sort: [] }, pivot: { search: [] } } }, dataset: [] };
    localStorage.setItem("__mockStore", JSON.stringify(store)); await mcpReloadCases();
  });
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case");
  assert.equal(await page.evaluate(() => caseEvents().length), 3);
  assert.deepEqual(await page.evaluate(() => state.filters), []);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.equal(await exactTotal(), 6000);
  results.invalidImportedPreferences = "normalized; case3/dataset6000 retained";
  // Background MCP events are serialized. A context request cannot split their update.
  await page.evaluate(async () => {
    await WorkspaceContext.setScope("case", { page: "explore", animate: false });
    const original = api; window.contextSourceSnapshots = [];
    api = async (command, args, options) => {
      if (command === "source_snapshot") {
        window.contextSourceSnapshots.push(WorkspaceContext.scope());
        if (window.contextSourceSnapshots.length === 1) await new Promise(resolve => { window.releaseContextSource = resolve; });
      }
      return original(command, args, options);
    };
    window.restoreContextApi = () => { api = original; };
    window.firstSourceUpdate = mcpRefreshSource();
  });
  await page.waitForFunction(() => typeof window.releaseContextSource === "function");
  await page.evaluate(async () => { window.secondSourceUpdate = mcpRefreshSource(); await WorkspaceContext.setScope("case", { animate: false }); });
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "dataset", "MCP update holds its source context until reconciled");
  await page.evaluate(async () => { window.releaseContextSource(); await Promise.all([window.firstSourceUpdate, window.secondSourceUpdate]); window.restoreContextApi(); });
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case");
  assert.deepEqual(await page.evaluate(() => window.contextSourceSnapshots), ["dataset", "dataset"], "both source publication reconciliations run serially inside their Dataset scope");
  assert.equal(await exactTotal(), 3);
  await page.evaluate(async () => { await api("clear_events"); await mcpRefreshSource(); });
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case");
  assert.equal(await exactTotal(), 3, "native clear retains saved case records");
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "summary", animate: false }));
  assert.deepEqual(await page.evaluate(() => ({ loaded: state.loaded, total: state.total, rows: state.rows.length })), { loaded: false, total: 0, rows: 0 });
  assert.equal(await page.locator("#ws-empty").isVisible(), true);
  results.mcp = "queued source updates preserve Case; clear_events leaves Case3/Analysis0";
  assert.deepEqual(errors, []);
  writeFileSync(resolve(output, "workspace-context-results.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ ...results, pageErrors: errors }, null, 2));
} catch (error) { await captureFailure(page, 'workspace-context', error, { errors, results }); throw error; }
finally { await browser.close(); }
