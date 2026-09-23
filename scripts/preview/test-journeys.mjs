/* Run: node scripts/preview/test-journeys.mjs http://127.0.0.1:4175 */
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
const url = process.argv[2] || "http://127.0.0.1:4175", output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const fallback = `${process.env.LOCALAPPDATA}/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe`;
const browser = await chromium.launch({ executablePath: existsSync(chromium.executablePath()) ? undefined : fallback });
const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, timezoneId: "America/Sao_Paulo" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => state.loaded && state.rows.length > 0 && window.Journeys && window.WorkspaceContext?.ready && !WorkspaceContext.changing && (window.__mockCommandCalls.load_file || window.__mockCommandCalls.load_files) && document.querySelector("#load-overlay").hidden);
  await page.getByRole("button", { name: "Jornadas", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 30);
  assert.equal(await page.getByRole("combobox", { name: "Conjunto", exact: true }).count(), 0, "Jornadas uses the global context toggle");
  assert.equal(await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), "correlation_id", "prefer useful repeated correlation over unique request IDs");
  await page.locator(".journey-item").first().click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 4);
  results.default = await page.locator(".journey-status").innerText();
  results.defaultDetail = await page.locator(".journey-detail-meta").innerText();
  assert.match(results.defaultDetail, /4 registros/);
  const expected = await page.evaluate(async () => {
    const value = document.querySelector(".journey-detail-title strong").textContent;
    const response = await api("journey_events", { filters: [], field: "correlation_id", value });
    return { value, timestamps: response.rows.map(row => row.timestamp), sources: [...new Set(response.rows.map(row => row.source))] };
  });
  assert.ok(expected.sources.length > 1);
  assert.deepEqual([...expected.timestamps].sort((a, b) => a - b), expected.timestamps);
  await page.evaluate(async () => {
    const original = api; window.journeyRestorePreview = () => { api = original; }; window.journeyDetailFetches = 0;
    api = async (name, args, opts) => { if (name === "event_detail") window.journeyDetailFetches++; const result = await original(name, args, opts); return name === "journey_events" ? { ...result, rows_clipped: 1, rows: result.rows.map(row => ({ ...row, message: "Prévia reduzida de teste", raw: "clipped" })) } : result; };
    await Workspace.showPage("journeys");
  });
  assert.match(await page.locator(".journey-preview-note").innerText(), /abra o registro/);
  await page.locator(".journey-record").first().click();
  await page.waitForFunction(() => state.currentDetailEv?.fields?.correlation_id);
  assert.equal(await page.evaluate(() => window.journeyDetailFetches), 1);
  assert.notEqual(await page.evaluate(() => state.currentDetailEv.message), "Prévia reduzida de teste");
  await page.evaluate(() => window.journeyRestorePreview());
  assert.ok(await page.getByRole("button", { name: "Investigar daqui", exact: true }).isVisible());
  await page.getByRole("button", { name: "Investigar daqui", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 4);
  assert.equal(await page.locator(".journey-focus").count(), 1);
  assert.equal(await page.locator(".journey-detail-title strong").textContent(), expected.value);
  const pagerBounds = () => page.locator(".journey-pager").evaluateAll(nodes => nodes.map(node => ({ bottom: node.getBoundingClientRect().bottom, visibleBottom: innerHeight - 30, height: node.getBoundingClientRect().height })));
  results.darkPagerBounds = await pagerBounds();
  for (const bounds of results.darkPagerBounds) assert.ok(bounds.bottom <= bounds.visibleBottom && bounds.height >= 40, "pagination stays fully above the status bar");
  assert.equal(await page.locator("#workspace-home").evaluate(node => node.scrollHeight > node.clientHeight), false);
  await page.screenshot({ path: resolve(output, "journeys-1200-dark.png") });
  await page.getByRole("button", { name: "Ver registros", exact: true }).click();
  await page.waitForFunction(() => state.activeDatasetTab === "table" && document.body.dataset.page === "explore");
  assert.deepEqual(await page.evaluate(() => state.filters.find(filter => filter.column === "correlation_id")), { column: "correlation_id", op: "equals_exact", value: expected.value, value2: null });

  await page.evaluate(async () => {
    state.filters = []; state.quick = "";
    const start = Date.UTC(2026, 8, 22, 12);
    window.journeyFixtureStart = start;
    const row = (id, value, timestamp) => ({ id, event_ref: `journey-test:${id}`, timestamp, source: id % 2 ? "Auth" : "API", level: id % 7 ? "Informação" : "Erro", code: "TEST", name: "Fluxo de teste", description: "", message: `Registro ${id} da jornada`, raw: "original:" + "x".repeat(id === 100 ? 70000 : 100), fields: { "trace.id": value, ip_cliente: "10.0.0.1", usuario: "ana", custom_key: value } });
    const rows = Array.from({ length: 150 }, (_, index) => row(index, "flow-long", index < 145 ? start + index * 1000 : null));
    for (let index = 0; index < 60; index++) rows.push(row(rows.length, `short-${index}`, start + index * 2000), row(rows.length + 1, `short-${index}`, start + index * 2000 + 500));
    rows.push(row(270, "API", start), row(271, "API", start + 1), row(272, "api", start), row(273, "api", start + 1));
    const current = ensureCase(); current.items = [{ id: "journey-fixture", kind: "grupo", label: "Fixture Jornadas", rows, origin: "teste", artifactId: "test-artifact" }]; caseEventsCache.sig = null;
    await Journeys.open({ scope: "case" });
  });
  await page.getByRole("combobox", { name: "Ordenar" }).selectOption("count");
  await page.waitForFunction(() => document.querySelector(".journey-item strong")?.textContent === "flow-long");
  assert.equal(await page.locator(".journey-item").count(), 50);
  results.caseIndex = await page.locator(".journey-status").innerText();
  assert.match(results.caseIndex, /63 jornadas/);
  await page.locator(".journey-list-panel").getByRole("button", { name: "Próxima", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 13);
  await page.locator(".journey-list-panel").getByRole("button", { name: "Anterior", exact: true }).click();
  await page.waitForFunction(() => document.querySelector(".journey-item strong")?.textContent === "flow-long");
  await page.locator(".journey-item").first().click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 100);
  results.caseDetail = await page.locator(".journey-detail-meta").innerText();
  assert.match(results.caseDetail, /150 registros.*5 sem horário/);
  await page.locator(".journey-detail").getByRole("button", { name: "Próxima", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 50);
  assert.match(await page.locator(".journey-record").last().innerText(), /Sem horário/);
  await page.locator(".journey-record").first().click();
  await page.waitForFunction(() => state.currentDetailEv?.raw?.length > 70000);
  assert.equal(await page.locator("#dr-prev").isVisible(), false);
  await page.evaluate(() => closeDrawer());
  await page.getByRole("searchbox", { name: "Identificador exato" }).fill("API");
  await page.getByRole("button", { name: "Abrir", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 2);
  assert.equal(await page.locator(".journey-detail-title strong").innerText(), "API");
  await page.getByRole("button", { name: "Ver registros", exact: true }).click();
  await page.waitForFunction(() => document.body.dataset.page === "explore" && state.rows.length === 2);
  assert.deepEqual(await page.evaluate(() => ({ scope: WorkspaceContext.scope(), context: state.activeContext, filter: state.filters.find(filter => filter.column === "trace.id") })), { scope: "case", context: "case", filter: { column: "trace.id", op: "equals_exact", value: "API", value2: null } });

  await page.evaluate(async () => { state.filters = []; state.quick = ""; await Journeys.open({ scope: "case" }); });
  await page.getByRole("combobox", { name: "Ligar pelo campo" }).selectOption("ip_cliente");
  assert.match(await page.locator(".journey-list-panel").innerText(), /Escolha início e fim/);
  assert.equal(await page.locator(".journey-item").count(), 0);
  await page.getByRole("textbox", { name: "De", exact: true }).fill("2026-09-22T09:00");
  await page.getByRole("textbox", { name: "Até", exact: true }).fill("2026-09-22T09:00:10");
  await page.getByRole("button", { name: "Aplicar período", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 1);
  await page.locator(".journey-item").click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 26);
  results.window = await page.locator(".journey-detail-meta").innerText();
  assert.ok(await page.getByRole("button", { name: "Proximidade temporal" }).isVisible());
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
  await page.waitForTimeout(350);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  results.lightPagerBounds = await pagerBounds();
  for (const bounds of results.lightPagerBounds) assert.ok(bounds.bottom <= bounds.visibleBottom && bounds.height >= 40);
  assert.equal(await page.locator("#workspace-home").evaluate(node => node.scrollHeight > node.clientHeight), false);
  await page.screenshot({ path: resolve(output, "journeys-1024-light.png") });
  await page.evaluate(async () => {
    await Journeys.open({ scope: "case", event: { ...caseEvents()[0], fields: { ip_cliente: "10.0.0.1" } } });
  });
  assert.equal(await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), "ip_cliente");
  assert.equal(await page.getByRole("textbox", { name: "De", exact: true }).inputValue(), "2026-09-22T08:55");
  assert.equal(await page.getByRole("textbox", { name: "Até", exact: true }).inputValue(), "2026-09-22T09:05");
  assert.equal(await page.locator(".journey-focus").count(), 1);
  // A superseded slow response may not restore its field's old rows.
  await page.getByRole("textbox", { name: "De", exact: true }).fill("2026-09-22T09:00");
  await page.getByRole("textbox", { name: "Até", exact: true }).fill("2026-09-22T09:00:10");
  await page.getByRole("button", { name: "Aplicar período", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 26);
  await page.evaluate(async () => { await Workspace.showPage("journeys"); await Workspace.showPage("summary"); await Journeys.open(); });
  assert.equal(await page.getByRole("textbox", { name: "De", exact: true }).inputValue(), "2026-09-22T09:00");
  assert.equal(await page.getByRole("textbox", { name: "Até", exact: true }).inputValue(), "2026-09-22T09:00:10");
  assert.equal(await page.locator(".journey-record").count(), 26);
  await page.evaluate(() => {
    const original = api; window.journeyRestoreApi = () => { api = original; };
    api = async (name, args, opts) => { if (name === "journey_index" && args.field === "custom_key") await new Promise(resolve => setTimeout(resolve, 250)); return original(name, args, opts); };
  });
  await page.getByRole("combobox", { name: "Ligar pelo campo" }).selectOption("custom_key");
  await page.getByRole("combobox", { name: "Ligar pelo campo" }).selectOption("usuario");
  await page.waitForTimeout(350);
  assert.match(await page.locator(".journey-list-panel").innerText(), /Escolha início e fim/);
  assert.equal(await page.locator(".journey-item").count(), 0);
  await page.evaluate(() => window.journeyRestoreApi());
  // Errors never leave stale rows; cancellation is explicit and can be retried.
  await page.evaluate(() => {
    const original = api; window.journeyRestoreFailure = () => { api = original; }; window.journeyFailOnce = true;
    api = async (name, args, opts) => { if (name === "journey_index" && window.journeyFailOnce) { window.journeyFailOnce = false; throw Error("Falha de teste na consulta"); } return original(name, args, opts); };
  });
  await page.getByRole("combobox", { name: "Ligar pelo campo" }).selectOption("custom_key");
  await page.waitForFunction(() => document.querySelector(".journey-list-panel").textContent.includes("Falha de teste"));
  assert.equal(await page.locator(".journey-item").count(), 0);
  await page.locator(".journey-list-panel").getByRole("button", { name: "Tentar novamente" }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 50);
  await page.evaluate(() => window.journeyRestoreFailure());
  await page.evaluate(() => {
    const original = api; window.journeyRestoreCancel = () => { api = original; };
    api = async (name, args, opts) => {
      if (name === "journey_index") return new Promise((_, reject) => { window.journeyReject = () => reject(Error("Operação cancelada")); });
      if (name === "cancel_operation") { window.journeyReject(); return null; }
      return original(name, args, opts);
    };
  });
  await page.getByRole("combobox", { name: "Ordenar" }).selectOption("duration");
  await page.locator(".journey-list-panel").getByRole("button", { name: "Cancelar" }).click();
  await page.waitForFunction(() => document.querySelector(".journey-list-panel").textContent.includes("Operação cancelada"));
  await page.evaluate(() => window.journeyRestoreCancel());
  await page.locator(".journey-list-panel").getByRole("button", { name: "Tentar novamente" }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 50);
  results.cancelAndRetry = true;

  // Each global context retains its own choices and exact filters.
  await page.evaluate(async () => { state.filters = [{ column: "source", op: "equals_exact", value: "Auth", value2: null }]; await Workspace.showPage("journeys"); });
  await page.getByRole("searchbox", { name: "Identificador exato" }).fill("API");
  await page.getByRole("button", { name: "Abrir", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 1);
  const caseConfig = { field: await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), sort: await page.getByRole("combobox", { name: "Ordenar" }).inputValue(), selected: await page.locator(".journey-detail-title strong").innerText() };
  await page.evaluate(() => Journeys.open({ scope: "dataset" }));
  await page.waitForFunction(() => document.querySelectorAll(".journey-item").length === 30);
  assert.equal(await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), "correlation_id");
  assert.equal(await page.getByRole("combobox", { name: "Ordenar" }).inputValue(), "recent");
  assert.deepEqual(await page.evaluate(() => state.filters), [], "case filters do not leak to source journeys");
  await page.getByRole("combobox", { name: "Ordenar" }).selectOption("errors");
  await page.locator(".journey-item").first().click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 4);
  await page.locator(".journey-record").first().click();
  await page.getByRole("button", { name: "Investigar daqui", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 4);
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "dataset", "Investigar daqui retains source scope");
  await page.evaluate(() => Journeys.open({ scope: "case" }));
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 1);
  assert.deepEqual({ field: await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), sort: await page.getByRole("combobox", { name: "Ordenar" }).inputValue(), selected: await page.locator(".journey-detail-title strong").innerText() }, caseConfig);
  assert.equal(await page.evaluate(() => state.filters[0]?.value), "Auth");
  await page.locator(".journey-record").first().click();
  await page.getByRole("button", { name: "Investigar daqui", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".journey-record").length === 1);
  assert.equal(await page.evaluate(() => WorkspaceContext.scope()), "case", "Investigar daqui retains case scope");
  results.isolatedContexts = caseConfig;
  await page.screenshot({ path: resolve(output, "journeys-context-case-light.png") });

  // A pending case query cannot repaint the source after a context change.
  await page.evaluate(() => {
    const original = api; window.journeyRestoreScope = () => { api = original; };
    api = async (name, args, opts) => { if (name === "journey_fields" && Array.isArray(args.caseEvents)) await new Promise(resolve => { window.releaseCaseJourneys = resolve; }); return original(name, args, opts); };
    window.pendingCaseJourneys = Workspace.showPage("journeys");
  });
  await page.waitForFunction(() => typeof window.releaseCaseJourneys === "function");
  await page.evaluate(async () => { await Journeys.open({ scope: "dataset" }); window.releaseCaseJourneys(); await window.pendingCaseJourneys; window.journeyRestoreScope(); });
  assert.equal(await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), "correlation_id");
  assert.equal(await page.getByRole("combobox", { name: "Ordenar" }).inputValue(), "errors");
  assert.match(await page.locator(".journey-status").innerText(), /Logs abertos/);
  results.staleContextRejected = true;
  const savedConfig = await page.evaluate(async () => { await saveCases(); return Journeys.capture(); });
  assert.equal(Object.hasOwn(savedConfig, "fields"), false);
  assert.equal(Object.hasOwn(savedConfig, "rows"), false);
  assert.equal(await page.evaluate(() => { const store = JSON.parse(localStorage.getItem("__mockStore")); return store.cases.find(item => item.id === store.active).workspace.contextStates.dataset.journeys.sort; }), savedConfig.sort, "saved case contains current source journey configuration");
  await page.reload();
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && document.querySelector("#load-overlay").hidden);
  await page.evaluate(() => Journeys.open({ scope: "dataset" }));
  assert.equal(await page.getByRole("combobox", { name: "Ligar pelo campo" }).inputValue(), savedConfig.field);
  assert.equal(await page.getByRole("combobox", { name: "Ordenar" }).inputValue(), savedConfig.sort);
  assert.equal(await page.locator(".journey-detail-title strong").innerText(), savedConfig.selected);
  results.configRestoredAfterReload = true;
  assert.deepEqual(errors, []);
  writeFileSync(resolve(output, "journeys-validation.json"), JSON.stringify({ results, errors }, null, 2));
  console.log(JSON.stringify({ results, errors }, null, 2));
} catch (error) {
  console.error(await page.evaluate(() => ({ status: document.querySelector(".journey-status")?.innerText, detail: document.querySelector(".journey-detail-meta")?.innerText, range: [...document.querySelectorAll(".journey-window input")].map(input => ({ value: input.value, validity: input.validity.valid, message: input.validationMessage })), filters: state.filters })));
  await page.screenshot({ path: resolve(output, "journeys-failure.png") }); throw error;
} finally { await browser.close(); }
