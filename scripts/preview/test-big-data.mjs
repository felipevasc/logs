/* Big Data changes the engine while preserving the investigation and selection. */
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:4181", output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const fallback = `${process.env.LOCALAPPDATA}/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe`;
const browser = await chromium.launch({ executablePath: existsSync(chromium.executablePath()) ? undefined : fallback });
const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
const ready = () => page.waitForFunction(() => WorkspaceContext?.ready && !WorkspaceContext.changing && !bigDataRuntime.busy && document.querySelector("#load-overlay").hidden);
const selection = () => page.evaluate(() => ({ total: state.total, page: state.page, pageSize: state.pageSize, filters: state.filters, quick: state.quick, sortCol: state.sortCol, sortDir: state.sortDir, rows: state.rows.map(row => [row.id, row.event_ref]) }));
const toggle = async enabled => {
  await page.locator("#btn-big-data").click();
  await page.waitForFunction(expected => !bigDataRuntime.busy && activeCase().bigData === expected && document.querySelector("#btn-big-data").getAttribute("aria-pressed") === String(expected), enabled);
  await page.waitForFunction(() => !state.activeOperation);
};

try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && state.rows.length && document.querySelector("#load-overlay").hidden);
  await ready();
  const firstId = await page.evaluate(() => activeCase().id);
  await page.evaluate(async () => {
    await WorkspaceContext.setScope("dataset", { page: "explore", animate: false });
    state.filters = [{ column: "source", op: "equals_exact", value: "Auth", value2: null }];
    state.quick = "Timeout"; document.querySelector("#quick-search").value = state.quick;
    state.pageSize = 25; state.page = 1; await refresh();
  });
  const baseline = await selection();
  assert.ok(baseline.total > 0);
  assert.equal(baseline.page, 1, "exercise a page after the first");
  await toggle(true);
  assert.deepEqual(await selection(), baseline, "activation preserves filters, search, sort, page and rows");
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem("__mockStore")).cases.find(c => c.id === state.cases.active).bigData), true);
  await page.screenshot({ path: resolve(output, "big-data-active.png"), fullPage: true });
  results.selection = { total: baseline.total, page: baseline.page, unchanged: true };

  const syncs = await page.evaluate(async () => {
    const records = state.rows.slice(0, 3).map((row, index) => ({ ...row, id: index }));
    const before = window.__mockCommandCalls.case_sync || 0;
    const counts = await Promise.all([api("count_filtered", { filters: [], caseEvents: records }, { silent: true }), api("count_filtered", { filters: [], caseEvents: records }, { silent: true })]);
    return { calls: (window.__mockCommandCalls.case_sync || 0) - before, counts };
  });
  assert.equal(syncs.calls, 1);
  assert.deepEqual(syncs.counts, [3, 3]);
  results.caseTransport = "one synchronization for parallel requests";

  const secondId = await page.evaluate(() => newCase("Caso padrão").id);
  await page.waitForFunction(() => !WorkspaceContext.changing && !bigDataRuntime.busy && bigDataRuntime.caseId === activeCase().id);
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.enabled), false);
  await page.evaluate(() => WorkspaceContext.setScope("case", { page: "explore", animate: false }));
  await toggle(true);
  assert.equal(await page.evaluate(() => state.total), 0, "empty Case must not read the open source");
  assert.equal(await page.evaluate(() => bigDataRuntime.status.ready), true, "open source has its own ready index");
  assert.equal(await page.evaluate(() => bigDataRuntime.status.eventCount), 6000);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "pending", "empty Case cannot inherit source readiness");
  await toggle(false);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  results.emptyCaseWithSource = "empty Case stays at zero and unready while the 6000-row source index is ready";
  await page.evaluate(id => WorkspaceContext.changeCase(id), firstId);
  await ready();
  assert.equal(await page.evaluate(() => activeCase().bigData && bigDataRuntime.status.ready), true);
  assert.deepEqual(await selection(), baseline);
  await page.evaluate(() => saveCases());
  await page.reload();
  await ready();
  assert.equal(await page.evaluate(() => activeCase().id), firstId);
  assert.equal(await page.evaluate(() => activeCase().bigData && bigDataRuntime.status.ready), true);
  assert.deepEqual(await selection(), baseline, "cold restore preserves selected page");
  await page.evaluate(id => WorkspaceContext.changeCase(id), secondId);
  await ready();
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.enabled), false);
  results.lifecycle = "preference survives switching and cold restore; other case uses standard engine";

  await page.evaluate(() => { window.__mockErrors = { set_big_data_mode: "Falha de índice para teste" }; });
  await page.locator("#btn-big-data").click();
  await page.waitForFunction(() => !bigDataRuntime.busy && !!bigDataRuntime.error);
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  assert.equal(await page.locator("#btn-big-data").getAttribute("aria-pressed"), "false");
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "error");
  assert.equal(await page.locator("#btn-big-data").isEnabled(), true);
  await page.evaluate(() => { window.__mockErrors = {}; });
  await toggle(true);
  await toggle(false);
  results.buildFailure = "setting preserved, no ready indicator, retry works";

  await page.evaluate(() => { window.__mockLatency = { set_big_data_mode: 1000 }; });
  await page.locator("#btn-big-data").click();
  await page.waitForFunction(() => bigDataRuntime.busy > 0);
  assert.equal(await page.locator("#btn-big-data").getAttribute("aria-busy"), "true");
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "building");
  await page.locator("#workbar-cancel").click();
  await page.waitForFunction(() => !bigDataRuntime.busy);
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  assert.notEqual(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  await page.evaluate(() => { window.__mockLatency = {}; });
  results.cancellation = "backend cancellation leaves saved preference unchanged";

  await page.evaluate(() => { window.__mockLatency = { set_big_data_mode: 250 }; window.racingMutationToggle = toggleBigData(); });
  await page.waitForFunction(() => bigDataRuntime.busy > 0);
  await page.evaluate(() => api("save_codes", { text: "{}" }, { silent: true }));
  await page.waitForFunction(() => !bigDataRuntime.busy);
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.enabled), false);
  await page.evaluate(() => { window.__mockLatency = {}; });
  results.concurrentMutation = "semantic mutation discards superseded toggle and keeps durable mode consistent";

  await toggle(true);
  const mutations = await page.evaluate(async () => {
    let before = window.__mockCommandCalls.set_big_data_mode || 0;
    await api("save_codes", { text: "{}" }, { silent: true });
    const success = (window.__mockCommandCalls.set_big_data_mode || 0) - before;
    before = window.__mockCommandCalls.set_big_data_mode || 0;
    window.__mockErrors = { save_codes: "Falha de catálogo para teste" };
    try { await api("save_codes", { text: "{}" }, { silent: true }); } catch {}
    window.__mockErrors = {};
    return { success, failed: (window.__mockCommandCalls.set_big_data_mode || 0) - before };
  });
  assert.deepEqual(mutations, { success: 1, failed: 0 });
  results.mutations = "one rebuild after success; none after rejected mutation";

  const ruleMutations = await page.evaluate(async () => {
    const { settings } = await api("detection_rules", {}, { silent: true });
    let before = window.__mockCommandCalls.set_big_data_mode || 0;
    await api("detection_settings_save", { settings: { ...settings, disabled: ["auth.bruteforce.source"] } }, { silent: true });
    const success = (window.__mockCommandCalls.set_big_data_mode || 0) - before;
    before = window.__mockCommandCalls.set_big_data_mode || 0;
    window.__mockErrors = { detection_settings_save: "Falha de regras para teste" };
    try { await api("detection_settings_save", { settings }, { silent: true }); } catch {}
    window.__mockErrors = {};
    return { success, failed: (window.__mockCommandCalls.set_big_data_mode || 0) - before };
  });
  assert.deepEqual(ruleMutations, { success: 1, failed: 0 });

  await toggle(false);
  const offRuleCaches = await page.evaluate(async () => {
    const { settings } = await api("detection_rules", {}, { silent: true });
    const verify = async (command, args) => {
      await refresh();
      const overview = await Workspace.overview(), revision = state.bigDataRevision, key = Workspace.sourceKey();
      const beforeMode = window.__mockCommandCalls.set_big_data_mode || 0;
      const beforeOverview = window.__mockCommandCalls.dataset_overview || 0;
      const beforeSnapshot = window.__mockCommandCalls.explore_snapshot || 0;
      await api(command, args, { silent: true });
      const snapshotInvalidated = state.explorerCache === null;
      const nextOverview = await Workspace.overview();
      await refresh();
      return {
        rebuilds: (window.__mockCommandCalls.set_big_data_mode || 0) - beforeMode,
        revisionChanged: state.bigDataRevision > revision, keyChanged: key !== Workspace.sourceKey(), snapshotInvalidated,
        overviewRecomputed: overview !== nextOverview && (window.__mockCommandCalls.dataset_overview || 0) - beforeOverview === 1,
        snapshotRecomputed: (window.__mockCommandCalls.explore_snapshot || 0) - beforeSnapshot === 1,
      };
    };
    const rules = await verify("detection_settings_save", { settings: { ...settings, disabled: [] } });
    const threats = await verify("threat_catalog_update", {});
    const revision = state.bigDataRevision, snapshot = state.explorerCache, overview = await Workspace.overview();
    window.__mockErrors = { detection_settings_save: "Falha de regras para teste" };
    try { await api("detection_settings_save", { settings }, { silent: true }); } catch {}
    window.__mockErrors = {};
    const failurePreservedCaches = state.bigDataRevision === revision && state.explorerCache === snapshot && await Workspace.overview() === overview;
    const beforeMode = window.__mockCommandCalls.set_big_data_mode || 0;
    const beforeOverview = window.__mockCommandCalls.dataset_overview || 0;
    const beforeSnapshot = window.__mockCommandCalls.explore_snapshot || 0;
    const events = caseEvents();
    await handleMcpStateChanged("threats");
    const nextOverview = await Workspace.overview();
    return { rules, threats, failurePreservedCaches, mcp: {
      rebuilds: (window.__mockCommandCalls.set_big_data_mode || 0) - beforeMode,
      revisionChanged: state.bigDataRevision > revision, rowsPreserved: events === caseEvents(),
      overviewRecomputed: overview !== nextOverview && (window.__mockCommandCalls.dataset_overview || 0) - beforeOverview === 1,
      snapshotRecomputed: (window.__mockCommandCalls.explore_snapshot || 0) - beforeSnapshot === 1,
    } };
  });
  const freshOffCaches = { rebuilds: 0, revisionChanged: true, keyChanged: true, snapshotInvalidated: true, overviewRecomputed: true, snapshotRecomputed: true };
  assert.deepEqual(offRuleCaches, { rules: freshOffCaches, threats: freshOffCaches, failurePreservedCaches: true, mcp: { rebuilds: 0, revisionChanged: true, rowsPreserved: true, overviewRecomputed: true, snapshotRecomputed: true } });
  assert.equal(await page.evaluate(() => activeCase().bigData), false);
  results.detectionMutations = "one rebuild after changed rules; none after rejected change or while mode is off";
  results.standardModeRuleCaches = "rule and threat updates recompute snapshots and overview without building an index; rejected changes preserve caches; MCP preserves Case records";
  await page.evaluate(() => { window.__mockLatency = { set_big_data_mode: 1000 }; window.staleToggle = toggleBigData(); });
  await page.waitForFunction(() => bigDataRuntime.busy > 0);
  await page.evaluate(id => WorkspaceContext.changeCase(id), firstId);
  await ready();
  await page.evaluate(() => { window.__mockLatency = {}; });
  assert.equal(await page.evaluate(() => activeCase().id), firstId);
  assert.equal(await page.evaluate(id => state.cases.cases.find(c => c.id === id).bigData, secondId), false);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.enabled && bigDataRuntime.status.ready), true);
  assert.deepEqual(await selection(), baseline);
  results.staleBuild = "switch discards old activation and restores destination mode";

  await page.evaluate(() => { window.__mockErrors = { cases_save: "Falha de persistência para teste" }; });
  await page.locator("#btn-big-data").click();
  await page.waitForFunction(() => caseSaveErrorShown && !bigDataRuntime.busy && activeCase().bigData === true && bigDataRuntime.status?.enabled === true && !state.activeOperation);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem("__mockStore")).cases.find(c => c.id === state.cases.active).bigData), true);
  await page.evaluate(() => { window.__mockErrors = {}; });
  results.saveFailure = "previous preference and engine restored after failed durable save";

  const mcpMutation = await page.evaluate(async () => {
    const before = window.__mockCommandCalls.set_big_data_mode || 0;
    await handleMcpStateChanged("codes");
    return (window.__mockCommandCalls.set_big_data_mode || 0) - before;
  });
  assert.equal(mcpMutation, 1);
  await page.evaluate(() => WorkspaceContext.setScope("case", { page: "explore", animate: false }));
  const mcpThreats = await page.evaluate(async () => {
    const events = caseEvents(), key = caseTransport.keys.get(events)?.get(true);
    const rows = JSON.stringify(events), preference = activeCase().bigData;
    const beforeMode = window.__mockCommandCalls.set_big_data_mode || 0;
    const beforeCase = window.__mockCommandCalls.case_sync || 0;
    await handleMcpStateChanged("threats");
    return {
      sourceRebuilds: (window.__mockCommandCalls.set_big_data_mode || 0) - beforeMode,
      caseRebuilds: (window.__mockCommandCalls.case_sync || 0) - beforeCase,
      sameArray: events === caseEvents(), sameKey: key === caseTransport.keys.get(caseEvents())?.get(true),
      rowsPreserved: rows === JSON.stringify(caseEvents()), preferencePreserved: preference === activeCase().bigData,
    };
  });
  assert.deepEqual(mcpThreats, { sourceRebuilds: 1, caseRebuilds: 0, sameArray: true, sameKey: true, rowsPreserved: true, preferencePreserved: true });
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  results.mcpThreatRules = "rule update rebuilds source once while preserving Case rows, transport key, index and preference";
  await page.evaluate(() => mcpRefreshSource());
  assert.equal(await page.evaluate(() => bigDataRuntime.status.enabled && bigDataRuntime.status.ready), true);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.eventCount), await page.evaluate(() => state.total));
  await page.evaluate(async () => { await api("clear_events"); await mcpRefreshSource(); });
  assert.equal(await page.evaluate(() => activeCase().bigData), true);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.ready), false);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "pending");
  results.mcp = "semantic changes and source replacement rebuild; empty source preserves preference without ready indicator";

  await page.evaluate(() => {
    const c = activeCase(), rows = caseEvents().slice(0, 3);
    c.items = [{ id: "saved-only", kind: "events", label: "Registros preservados", rows }];
    c.artifacts = []; c.activeArtifactId = null; caseEventsCache.sig = null; updateAnalysisBadge();
    window.__mockLatency = { case_sync: 250 };
    window.savedCaseOpen = WorkspaceContext.setScope("case", { page: "explore", animate: false });
  });
  await page.waitForFunction(() => document.querySelector("#btn-big-data").getAttribute("aria-busy") === "true");
  assert.equal(await page.locator("#btn-big-data").isDisabled(), true);
  await page.evaluate(async () => { await window.savedCaseOpen; window.__mockLatency = {}; });
  assert.equal(await page.evaluate(() => state.total), 3);
  assert.equal(await page.evaluate(() => bigDataRuntime.status.ready), false, "source stays unready when only the Case is indexed");
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  assert.match(await page.locator("#btn-big-data").getAttribute("title"), /3 eventos indexados/);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "summary", animate: false }));
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "pending");
  await page.evaluate(() => WorkspaceContext.setScope("case", { page: "explore", animate: false }));
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  await page.evaluate(async () => {
    activeCase().items[0].rows = activeCase().items[0].rows.map(row => ({ ...row }));
    caseEventsCache.sig = null; window.__mockErrors = { case_sync: "Falha de índice do Caso para teste" };
    await refresh();
  });
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "error");
  await page.evaluate(() => { window.__mockErrors = {}; });
  await toggle(true);
  assert.equal(await page.evaluate(() => state.total), 3);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "ready");
  results.savedOnlyCase = "Case readiness and count remain distinct from source readiness; failure retries work";

  await page.evaluate(async () => {
    activeCase().items = []; caseEventsCache.sig = null; updateAnalysisBadge(); await refresh();
  });
  assert.equal(await page.evaluate(() => state.total), 0);
  assert.equal(await page.locator("#btn-big-data").getAttribute("aria-pressed"), "true");
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "pending");
  const emptyStatus = await page.evaluate(() => invoke("case_sync", { key: "empty-case-contract", events: [], bigData: true }));
  assert.equal(emptyStatus.enabled, true);
  assert.equal(emptyStatus.ready, false);
  assert.equal(emptyStatus.eventCount, 0);
  assert.equal(emptyStatus.indexBytes, 0);
  await toggle(false);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "off");
  await toggle(true);
  assert.equal(await page.evaluate(() => state.total), 0);
  assert.equal(await page.locator("#btn-big-data").getAttribute("data-status"), "pending");
  results.emptyCase = "zero preserved rows never become ready or fall back to another source; mode remains reversible";

  assert.deepEqual(errors, []);
  writeFileSync(resolve(output, "big-data-results.json"), JSON.stringify({ ...results, pageErrors: errors }, null, 2));
  console.log(JSON.stringify({ ...results, pageErrors: errors }, null, 2));
} finally { await browser.close(); }
