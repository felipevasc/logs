/* Run against the preview server: node scripts/preview/test-analysis-workbench.mjs http://127.0.0.1:4174 */
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:4174";
const output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
let executablePath = chromium.executablePath();
if (!existsSync(executablePath)) executablePath = [
  `${process.env.LOCALAPPDATA}/ms-playwright/chromium-1217/chrome-win64/chrome.exe`,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
].find(existsSync);
const browser = await chromium.launch(executablePath ? { executablePath } : {});
const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => state.loaded && state.rows.length > 0, { timeout: 30000 });
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  await page.getByRole("button", { name: "Resumir", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#aw-group-summary").textContent.includes("6.000 registros"));
  results.groupSummary = await page.locator("#aw-group-summary").textContent();
  await page.screenshot({ path: resolve(output, "workbench-group-1440.png") });

  results.groupBounded = await page.evaluate(async () => {
    const originalApi = api, oldField = state.groupCol, oldAggs = state.aggs;
    let calls = 0;
    api = async (name, args) => name === "aggregate_events" ? (calls++, {
      columns: ["level", "Registros"], rows: Array.from({ length: 250 }, (_, index) => ({ level: `Grupo ${index}`, Registros: index + 1 })),
    }) : originalApi(name, args);
    state.groupCol = "level"; state.aggs = [{ func: "count", column: "*", alias: "Registros" }];
    try {
      await runGroup();
      const rowsPerPage = document.querySelector("#group-table tbody").rows.length;
      const first = document.querySelector("#group-table tbody tr").cells[0].textContent;
      const input = document.querySelector("#aw-group-tools input"); input.value = "Grupo 249"; input.dispatchEvent(new Event("input"));
      return { rowsPerPage, first, found: document.querySelector("#group-table tbody").rows.length, backendCalls: calls, summary: document.querySelector("#aw-group-summary").textContent };
    } finally {
      api = originalApi; state.groupCol = oldField; state.aggs = oldAggs;
      const input = document.querySelector("#aw-group-tools input"); input.value = ""; input.dispatchEvent(new Event("input")); await runGroup();
    }
  });
  assert.equal(results.groupBounded.rowsPerPage, 100);
  assert.equal(results.groupBounded.first, "Grupo 249");
  assert.equal(results.groupBounded.found, 1);
  assert.equal(results.groupBounded.backendCalls, 1);
  assert.match(results.groupBounded.summary, /31\.375 registros/);

  // Reproduce the user's two measures through their real controls.
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
    try { await runGroup(); } finally { api = original; }
  });
  assert.equal(await page.locator("#group-table tbody tr").count(), 0, "failed recalculation must not present stale totals");
  await page.locator("#aw-group-summary").getByRole("button", { name: "Detalhes", exact: true }).click();
  assert.match(await page.locator("#analysis-help .modal-body").textContent(), /Falha de conexão de teste/);
  await page.getByRole("button", { name: "Fechar explicação", exact: true }).click();
  await page.locator("#aw-group-summary").getByRole("button", { name: "Tentar novamente", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll("#group-table tbody tr").length === 5);
  await page.evaluate(async () => { state.aggs = [{ func: "count", column: "*", alias: "Registros" }]; renderAggs(); await runGroup(); });

  await page.locator("#group-table tbody tr").first().getByRole("button", { name: "Ver registros →" }).click();
  await page.waitForFunction(() => state.activeDatasetTab === "table" && state.filters.length > 0);
  results.savedGroupFilter = await page.evaluate(() => ({ active: state.filters, saved: savedFilters().find(filter => filter.id === CURRENT_FILTER_ID)?.filters }));
  assert.deepEqual(results.savedGroupFilter.active, results.savedGroupFilter.saved);
  await page.evaluate(() => { state.filters = []; filtersChanged(); });
  await page.waitForFunction(() => state.total === 6000);

  await page.getByRole("button", { name: "Cruzar dados", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 1);
  assert.equal(await page.evaluate(() => activeCube() === activeCube()), true, "normalization must preserve table references");
  assert.ok(await page.evaluate(() => activeCube().lastDataSignature));
  await page.evaluate(async () => {
    const original = api;
    api = async (name, args, opts) => { if (name === "pivot") throw new Error("Falha do cruzamento de teste"); return original(name, args, opts); };
    try { await runCube(); } finally { api = original; }
  });
  assert.equal(await page.locator("#cube-table tbody tr").count(), 0);
  assert.equal(await page.evaluate(() => cubeResultForTable(activeCube())), null);
  await page.locator("#aw-pivot-summary").getByRole("button", { name: "Detalhes", exact: true }).click();
  assert.match(await page.locator("#analysis-help .modal-body").textContent(), /Falha do cruzamento de teste/);
  await page.getByRole("button", { name: "Fechar explicação", exact: true }).click();
  await page.locator("#aw-pivot-summary").getByRole("button", { name: "Tentar novamente", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#cube-table tbody").rows.length > 1);
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

  results.pivotFilters = await page.evaluate(() => {
    let menu;
    const originalMenu = showCtxMenu, originalFiltersChanged = filtersChanged, originalFilters = state.filters;
    showCtxMenu = (x, y, items) => { menu = items; }; filtersChanged = () => {}; state.filters = [];
    try {
      document.querySelector("#cube-table .cube-leaf-row .cube-value").oncontextmenu({ preventDefault() {}, clientX: 0, clientY: 0 });
      menu[0].onClick(); return state.filters;
    } finally { showCtxMenu = originalMenu; filtersChanged = originalFiltersChanged; state.filters = originalFilters; }
  });
  assert.deepEqual(results.pivotFilters.map(filter => filter.column), ["level", "code", "source"]);

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
  assert.deepEqual(errors, []);
  results.pageErrors = errors;
  writeFileSync(resolve(output, "workbench-validation.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} finally {
  await browser.close();
}
