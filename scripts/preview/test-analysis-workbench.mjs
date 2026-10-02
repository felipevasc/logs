/* Run against the preview server: node scripts/preview/test-analysis-workbench.mjs http://127.0.0.1:4174 */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { captureFailure } from "./diagnostics.mjs";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:4174";
const output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: "reduce" });
const errors = [], results = {};
let phase = "startup";
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length > 0 && !state.loadOverlay && document.querySelector("#load-overlay").hidden);
  phase = "grouping and bounded rendering";
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  await page.getByRole("button", { name: "Resumir", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#aw-group-summary").textContent.includes("6.000 registros"));
  results.groupSummary = await page.locator("#aw-group-summary").textContent();
  await page.screenshot({ path: resolve(output, "workbench-group-1440.png") });

  results.groupBounded = await page.evaluate(async () => {
    const originalApi = api, oldField = state.groupCol, oldAggs = state.aggs;
    let calls = 0;
    api = async (name, args, opts) => name === "aggregate_events" ? (calls++, {
      columns: ["level", "Registros"], rows: Array.from({ length: 250 }, (_, index) => ({ level: `Grupo ${index}`, Registros: index + 1 })),
    }) : originalApi(name, args, opts);
    state.groupCol = "level"; state.aggs = [{ func: "count", column: "*", alias: "Registros" }];
    try {
      await runGroup({ force: true });
      const rowsPerPage = document.querySelector("#group-table tbody").rows.length;
      const first = document.querySelector("#group-table tbody tr").cells[0].textContent;
      const input = document.querySelector("#aw-group-tools input"); input.value = "Grupo 249"; input.dispatchEvent(new Event("input"));
      return { rowsPerPage, first, found: document.querySelector("#group-table tbody").rows.length, backendCalls: calls, summary: document.querySelector("#aw-group-summary").textContent };
    } finally {
      api = originalApi; state.groupCol = oldField; state.aggs = oldAggs;
      const input = document.querySelector("#aw-group-tools input"); input.value = ""; input.dispatchEvent(new Event("input")); await runGroup({ force: true });
    }
  });
  assert.equal(results.groupBounded.rowsPerPage, 100);
  assert.equal(results.groupBounded.first, "Grupo 249");
  assert.equal(results.groupBounded.found, 1);
  assert.equal(results.groupBounded.backendCalls, 1);
  assert.match(results.groupBounded.summary, /31\.375 registros/);

  // Reproduce the user's two measures through their real controls.
  phase = "multiple measures";
  await page.getByRole("combobox", { name: "Cálculo da medida 1", exact: true }).selectOption("sum");
  await page.getByRole("combobox", { name: "Campo da medida 1", exact: true }).selectOption("code");
  await page.getByRole("button", { name: "+ Medida", exact: true }).click();
  await page.getByRole("combobox", { name: "Campo da medida 2", exact: true }).selectOption("name");
  await page.getByRole("textbox", { name: "Nome da medida 2", exact: true }).fill("Nomes únicos");
  await page.getByRole("textbox", { name: "Nome da medida 2", exact: true }).press("Tab");
  await page.waitForFunction(() => document.querySelectorAll("#group-table thead th").length === 4 && document.querySelector("#group-table thead").textContent.includes("Nomes únicos") && document.querySelector("#group-table").getAttribute("aria-busy") === "false");
  results.multipleMeasures = await page.evaluate(async () => {
    const rows = [];
    for (let offset = 0; ; offset += 2000) {
      const batch = await api("query_events", { filters: [], offset, limit: 2000 });
      rows.push(...batch.rows);
      if (offset + batch.rows.length >= batch.total || !batch.rows.length) break;
    }
    const expected = {};
    for (const row of rows) { const item = expected[row.level] ||= { sum: 0, names: new Set() }; if (Number.isFinite(Number(row.code))) item.sum += Number(row.code); item.names.add(row.name); }
    return [...document.querySelectorAll("#group-table tbody tr")].map(row => ({ level: row.cells[0].textContent, sum: Number(row.cells[1].textContent.replaceAll(".", "")), unique: Number(row.cells[2].textContent), expectedSum: expected[row.cells[0].textContent].sum, expectedUnique: expected[row.cells[0].textContent].names.size }));
  });
  for (const row of results.multipleMeasures) { assert.equal(row.sum, row.expectedSum); assert.equal(row.unique, row.expectedUnique); }
  await page.screenshot({ path: resolve(output, "workbench-multiple-measures.png") });

  await page.evaluate(async () => {
    const original = api;
    api = async (name, args, opts) => { if (name === "aggregate_events") throw new Error("Falha de conexão de teste"); return original(name, args, opts); };
    try { await runGroup({ force: true }); } finally { api = original; }
  });
  assert.equal(await page.locator("#group-table tbody tr").count(), 0, "failed recalculation must not present stale totals");
  await page.locator("#aw-group-summary").getByRole("button", { name: "Detalhes", exact: true }).click();
  assert.match(await page.locator("#analysis-help .modal-body").textContent(), /Falha de conexão de teste/);
  await page.getByRole("button", { name: "Fechar explicação", exact: true }).click();
  await page.locator("#aw-group-summary").getByRole("button", { name: "Tentar novamente", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll("#group-table tbody tr").length === 5 && document.querySelector("#group-table").getAttribute("aria-busy") === "false");
  await page.evaluate(async () => { state.aggs = [{ func: "count", column: "*", alias: "Registros" }]; renderAggs(); await runGroup(); });

  await page.locator("#group-table tbody tr").first().getByRole("button", { name: "Ver registros →" }).click();
  await page.waitForFunction(() => state.activeDatasetTab === "table" && state.filters.length > 0);
  results.savedGroupFilter = await page.evaluate(() => ({ active: state.filters, saved: savedFilters().find(filter => filter.id === CURRENT_FILTER_ID)?.filters }));
  assert.deepEqual(results.savedGroupFilter.active, results.savedGroupFilter.saved);
  await page.evaluate(() => { state.filters = []; filtersChanged(); });
  await page.waitForFunction(() => state.total === 6000);

  phase = "pivot calculations and recovery";
  await page.getByRole("button", { name: "Cruzar dados", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 1);
  assert.equal(await page.evaluate(() => activeCube() === activeCube()), true, "normalization must preserve table references");
  assert.ok(await page.evaluate(() => activeCube().lastDataSignature));
  assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "true", "the builder starts expanded");
  await page.getByRole("button", { name: "Recolher configuração", exact: true }).click();
  await page.evaluate(async () => {
    const original = api;
    api = async (name, args, opts) => { if (name === "pivot") throw new Error("Falha do cruzamento de teste"); return original(name, args, opts); };
    try { await runCube({ force: true }); } finally { api = original; }
  });
  assert.equal(await page.locator("#cube-table tbody tr").count(), 0);
  assert.equal(await page.evaluate(() => cubeResultForTable(activeCube())), null);
  await page.locator("#aw-pivot-summary").getByRole("button", { name: "Detalhes", exact: true }).click();
  assert.match(await page.locator("#analysis-help .modal-body").textContent(), /Falha do cruzamento de teste/);
  await page.getByRole("button", { name: "Fechar explicação", exact: true }).click();
  await page.locator("#aw-pivot-summary").getByRole("button", { name: "Tentar novamente", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 1);
  assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "false", "failure and retry do not reopen the builder");

  phase = "pivot cancellation with folded configuration";
  await page.evaluate(() => {
    window.__mockLatency = { ...window.__mockLatency, pivot: 10_000 };
    window.__configurationPending = runCube({ force: true });
  });
  await page.waitForFunction(() => Tasks.groups().some(group => group.tasks.some(task => task.cmd === "pivot")) && document.querySelector(".cube-output .area-loading-semantic"));
  const pendingBefore = await page.evaluate(() => ({ pivot: window.__mockCommandCalls.pivot, cancels: window.__mockCommandCalls.cancel_task || 0, operation: Tasks.operationFor("pivot") }));
  await page.getByRole("button", { name: "Editar configuração", exact: true }).click();
  await page.getByRole("button", { name: "Recolher configuração", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => ({ pivot: window.__mockCommandCalls.pivot, cancels: window.__mockCommandCalls.cancel_task || 0, operation: Tasks.operationFor("pivot") })), pendingBefore,
    "folding is available during calculation and never reruns or cancels it");
  const taskId = await page.evaluate(() => Tasks.groups().flatMap(group => group.tasks).find(task => task.cmd === "pivot").id);
  await page.locator("#workbar-tasks").click();
  await page.locator(`.tasks-modal [data-cancel="${taskId}"]`).click();
  await page.evaluate(async () => { await window.__configurationPending; delete window.__mockLatency.pivot; });
  await page.locator(".tasks-modal [data-close]").click();
  assert.equal(await page.locator(".cube-output .area-loading-semantic").count(), 0);
  assert.equal(await page.locator("#cube-table tbody tr").count(), 0, "a cancelled calculation cannot leave stale totals");
  assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "false");
  await page.locator("#aw-pivot-summary").getByRole("button", { name: "Tentar novamente", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 1);
  results.foldedRecovery = { errorDetailsAndRetry: true, realTaskCancellationAndRetry: true, toggleLeavesPendingOperationUntouched: true };
  await page.getByRole("button", { name: "Editar configuração", exact: true }).click();
  await page.getByRole("combobox", { name: "Adicionar campo em colunas", exact: true }).selectOption("source");
  await page.waitForFunction(() => document.querySelector("#cube-table thead").rows.length === 2);
  await page.getByRole("combobox", { name: "Adicionar campo em linhas", exact: true }).selectOption("code");
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 6);
  results.pivot = await page.evaluate(() => {
    const before = document.querySelector("#cube-table tbody").rows.length;
    const total = [...document.querySelectorAll("#cube-table .total .cube-value")].reduce((sum, cell) => sum + Number(cell.textContent.replaceAll(".", "")), 0);
    [...document.querySelectorAll(".aw-pivot-result-tools button")].find(button => button.textContent === "Recolher").click();
    const collapsed = document.querySelector("#cube-table tbody").rows.length;
    [...document.querySelectorAll(".aw-pivot-result-tools button")].find(button => button.textContent === "Expandir").click();
    return { before, collapsed, expanded: document.querySelector("#cube-table tbody").rows.length, headers: document.querySelector("#cube-table thead").rows.length, total, mergedDimensionCells: document.querySelectorAll("#cube-table tbody td[rowspan]:not([rowspan='1'])").length };
  });
  assert.equal(results.pivot.before, results.pivot.expanded);
  assert.equal(results.pivot.collapsed, 6);
  assert.equal(results.pivot.headers, 2);
  assert.equal(results.pivot.total, 6000);
  assert.ok(results.pivot.mergedDimensionCells > 0);

  // Each dimension opens the editable composer without applying the whole cell.
  await page.locator("#cube-table .cube-leaf-row .cube-value").first().click({ button: "right" });
  await page.getByRole("menuitem", { name: "Criar filtro: Nível", exact: true }).click();
  await page.locator("#filter-pop").waitFor({ state: "visible" });
  assert.equal(await page.locator("#fp-col").inputValue(), "level");
  assert.equal(await page.locator("#fp-op").inputValue(), "equals_exact");
  assert.ok((await page.locator("#fp-val").inputValue()).length > 0);
  assert.deepEqual(await page.evaluate(() => state.filters), [], "opening the dimension composer leaves the current recorte intact");
  await page.locator("#fp-cancel").click();
  await page.locator("#filter-pop").waitFor({ state: "hidden" });
  assert.deepEqual(await page.evaluate(() => state.filters), [], "cancel does not apply any pivot dimension");

  results.pivotFilters = await page.evaluate(() => {
    let menu;
    const originalMenu = showCtxMenu, originalFiltersChanged = filtersChanged, originalFilters = state.filters;
    showCtxMenu = (x, y, items) => { menu = items; }; filtersChanged = () => {}; state.filters = [];
    try {
      document.querySelector("#cube-table .cube-leaf-row .cube-value").oncontextmenu({ preventDefault() {}, clientX: 0, clientY: 0 });
      const combination = menu.find(item => item.label === "Filtrar esta combinação");
      if (!combination) throw Error("The pivot combination action is missing");
      combination.onClick(); return state.filters;
    } finally { showCtxMenu = originalMenu; filtersChanged = originalFiltersChanged; state.filters = originalFilters; }
  });
  assert.deepEqual(results.pivotFilters.map(filter => filter.column), ["level", "code", "source"]);
  assert.ok(results.pivotFilters.every(filter => filter.op === "equals_exact" && typeof filter.value === "string"), "the named combination action retains exact native dimension keys");

  phase = "bounded pivot rendering";
  results.pivotBounded = await page.evaluate(() => {
    const cube = activeCube(), actual = cubeState.result, fixture = structuredClone(actual);
    fixture.row_paths = Array.from({ length: 250 }, (_, index) => [`Grupo ${index}`, `Valor ${index}`]);
    fixture.cells = Array.from({ length: 250 }, (_, index) => Array.from({ length: 30 }, (_, column) => [index + column]));
    fixture.col_keys = Array.from({ length: 30 }, (_, index) => `Origem ${index}`); fixture.totals = Array.from({ length: 30 }, () => [999]);
    fixture.complete = false; fixture.processed_events = 250;
    renderCubeTable(cube, fixture);
    const report = { rows: document.querySelector("#cube-table tbody").rows.length, columns: document.querySelector("#cube-table tbody").rows[0].cells.length, partial: document.querySelector("#cube-table .total td").textContent };
    renderCubeTable(cube, actual); return report;
  });
  assert.equal(results.pivotBounded.rows, 101);
  assert.equal(results.pivotBounded.columns, 26);
  assert.match(results.pivotBounded.partial, /parcial/);

  phase = "pivot search and compact layout";
  await page.getByRole("searchbox", { name: "Buscar linhas do cruzamento" }).fill("IMPOSSIBLE-NONEXISTENT");
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").textContent.includes("Nenhuma linha"));
  assert.match(await page.locator("#cube-table .total").textContent(), /Total do recorte/);
  await page.getByRole("searchbox", { name: "Buscar linhas do cruzamento" }).fill("");
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.screenshot({ path: resolve(output, "workbench-pivot-1024.png") });
  results.compactViewport = await page.evaluate(() => {
    const panel = document.querySelector("#view-cube").getBoundingClientRect(), tools = document.querySelector(".aw-pivot-result-tools").getBoundingClientRect(), table = document.querySelector("#cube-table-view").getBoundingClientRect();
    return { horizontalOverflow: document.documentElement.scrollWidth > innerWidth, panelWidth: Math.round(panel.width), tableHeight: Math.round(table.height), tableWithinPanel: table.top >= panel.top && table.bottom <= panel.bottom + 1, toolsOverlapTable: tools.bottom > table.top + 1 };
  });
  assert.equal(results.compactViewport.horizontalOverflow, false);
  assert.equal(results.compactViewport.toolsOverlapTable, false);
  assert.equal(results.compactViewport.tableWithinPanel, true);
  assert.ok(results.compactViewport.tableHeight >= 100);

  phase = "explicit configuration folding, readable rows and keyboard";
  results.configurationLayout = [];
  const measureLayout = () => page.evaluate(() => {
    const panel = document.querySelector("#view-cube").getBoundingClientRect(), table = document.querySelector("#cube-table-view").getBoundingClientRect();
    const headerBottom = Math.max(...[...document.querySelectorAll("#cube-table thead th")].map(cell => cell.getBoundingClientRect().bottom));
    const totalTop = Math.min(...[...document.querySelectorAll("#cube-table .total td")].map(cell => cell.getBoundingClientRect().top));
    const tools = document.querySelector(".aw-pivot-result-tools").getBoundingClientRect(), toggle = document.querySelector("#aw-pivot-config-toggle");
    const rows = [...document.querySelectorAll("#cube-table tbody tr:not(.total)")].map(row => row.getBoundingClientRect());
    return {
      tableHeight: Math.round(table.height), tableWidth: Math.round(table.width),
      readableRows: rows.filter(row => row.top >= Math.max(table.top, headerBottom) - 1 && row.bottom <= Math.min(table.bottom, totalTop) + 1).length,
      horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
      tableWithinPanel: table.top >= panel.top && table.bottom <= panel.bottom + 1,
      toolsOverlapTable: tools.bottom > table.top + 1,
      rowFont: Number.parseFloat(getComputedStyle(document.querySelector("#cube-table tbody td")).fontSize),
      summaryFont: Number.parseFloat(getComputedStyle(document.querySelector("#aw-pivot-config-summary")).fontSize),
      toggleHeight: toggle.getBoundingClientRect().height, toggleFocused: document.activeElement === toggle,
      focusOutline: Number.parseFloat(getComputedStyle(toggle).outlineWidth),
    };
  });
  for (const width of [1440, 1024]) for (const theme of ["dark", "light"]) {
    await page.setViewportSize({ width, height: width === 1440 ? 960 : 768 });
    await page.evaluate(theme => { if (document.documentElement.dataset.theme !== theme) toggleTheme(); }, theme);
    await page.evaluate(() => {
      const table = document.querySelector("#cube-table-view"); table.scrollTop = 120; table.scrollLeft = 20;
      const cube = activeCube();
      window.__configurationProbe = {
        firstRow: document.querySelector("#cube-table tbody tr"), result: cubeResultForTable(cube), schema: cubeSchemaSignature(cube),
        requestVersion: cubeState.requestVersion, calls: window.__mockCommandCalls.pivot,
        scroll: [table.scrollLeft, table.scrollTop], state: WorkspaceAnalysis.capture().pivot,
      };
    });
    const expanded = await measureLayout();
    await page.screenshot({ path: resolve(output, `workbench-pivot-expanded-${theme}-${width}.png`) });
    await page.locator("#aw-pivot-config-toggle").focus(); await page.keyboard.press("Enter");
    assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "false");
    assert.equal(await page.locator("#aw-pivot-config-zones").isVisible(), false);
    assert.equal(await page.locator("#aw-pivot-fields").isVisible(), false);
    const collapsed = await measureLayout();
    await page.screenshot({ path: resolve(output, `workbench-pivot-collapsed-${theme}-${width}.png`) });
    const preserved = await page.evaluate(() => {
      const before = window.__configurationProbe, table = document.querySelector("#cube-table-view"), current = WorkspaceAnalysis.capture().pivot;
      const { configurationCollapsed: _a, ...previous } = before.state, { configurationCollapsed: _b, ...after } = current;
      return {
        sameRow: before.firstRow === document.querySelector("#cube-table tbody tr"), sameResult: before.result === cubeResultForTable(activeCube()),
        sameSchema: before.schema === cubeSchemaSignature(activeCube()), sameView: JSON.stringify(previous) === JSON.stringify(after),
        noRequest: before.calls === window.__mockCommandCalls.pivot && before.requestVersion === cubeState.requestVersion,
        sameScroll: Math.min(before.scroll[0], table.scrollWidth - table.clientWidth) === table.scrollLeft
          && Math.min(before.scroll[1], table.scrollHeight - table.clientHeight) === table.scrollTop,
      };
    });
    assert.ok(Object.values(preserved).every(Boolean), JSON.stringify(preserved));
    assert.ok(collapsed.tableHeight >= expanded.tableHeight + 60, `${width}/${theme} must recover real data height`);
    assert.ok(collapsed.readableRows >= expanded.readableRows + 2, `${width}/${theme} must expose more fully readable rows`);
    assert.equal(collapsed.rowFont, expanded.rowFont, "more rows do not come from shrinking the text");
    assert.ok(collapsed.summaryFont >= 12 && collapsed.toggleHeight >= 30);
    assert.ok(collapsed.toggleFocused && collapsed.focusOutline >= 2);
    assert.equal(collapsed.horizontalOverflow, false); assert.equal(collapsed.toolsOverlapTable, false); assert.equal(collapsed.tableWithinPanel, true);
    await page.keyboard.press("Space");
    assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "true");
    assert.equal(await page.getByRole("combobox", { name: "Adicionar campo em linhas", exact: true }).isVisible(), true);
    assert.equal(await page.evaluate(() => document.activeElement === document.querySelector("#aw-pivot-config-toggle")), true);
    assert.equal(await page.evaluate(() => {
      const table = document.querySelector("#cube-table-view"), previous = window.__configurationProbe.scroll;
      return table.scrollLeft === previous[0] && table.scrollTop === previous[1];
    }), true, "reopening restores even positions clamped by the larger viewport");
    results.configurationLayout.push({ width, theme, expanded, collapsed, preserved });
  }

  phase = "folding preferences survive real workspace rotation";
  await page.getByRole("button", { name: "Recolher configuração", exact: true }).click();
  await page.evaluate(() => WorkspaceContext.setScope("case", { page: "explore", tab: "cube", animate: false }));
  await page.waitForFunction(() => !WorkspaceContext.changing && WorkspaceContext.scope() === "case");
  assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "true", "a new context does not inherit the previous folding choice");
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", tab: "cube", animate: false }));
  await page.waitForFunction(() => !WorkspaceContext.changing && WorkspaceContext.scope() === "dataset" && document.querySelector("#cube-table tbody").rows.length > 1);
  assert.equal(await page.locator("#aw-pivot-config-toggle").getAttribute("aria-expanded"), "false", "returning to the original context restores its explicit choice");
  assert.match(await page.locator("#aw-pivot-config-summary").textContent(), /Nível.*Código.*Origem.*Registros/);
  await page.getByRole("button", { name: "Editar configuração", exact: true }).click();
  assert.equal(await page.getByRole("combobox", { name: "Adicionar campo em colunas", exact: true }).isVisible(), true);
  results.configurationContexts = "Case starts expanded; dataset restores its closed builder and authored axes; edit remains available";
  assert.deepEqual(errors, []);
  results.pageErrors = errors;
  writeFileSync(resolve(output, "workbench-validation.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, "analysis-workbench", error, { phase, errors, results });
  throw error;
} finally {
  await browser.close();
}
