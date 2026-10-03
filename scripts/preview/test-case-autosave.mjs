/* Exercise the real autosave/store replacement and lifecycle through the preview. */
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
// A private browser context contains only synthetic Cases. The mock's native
// close surface calls the same registered callback as Tauri, without closing
// the runner's browser. Root/native smoke also exercises actual WebView close.
await page.addInitScript(() => {
  if (!localStorage.getItem("__mockStore")) localStorage.setItem("__mockStore", JSON.stringify({
    schemaVersion: 2, revision: 1, active: "autosave-test", cases: [{ id: "autosave-test", name: "Autosave synthetic",
      createdAt: 1, items: [], manual: [], stations: [], artifacts: [], workspace: { view: "source", analysisView: "overview" } }],
  }));
  Object.defineProperty(window, "__TAURI__", { configurable: true, set(value) {
    value.window = { getCurrentWindow: () => ({ onCloseRequested(handler) { window.__testClose = handler; return Promise.resolve(() => {}); } }) };
    Object.defineProperty(window, "__TAURI__", { value, configurable: true, writable: true });
  } });
});
const ready = async () => {
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && !bigDataRuntime.busy && document.querySelector("#load-overlay").hidden);
  assert.equal(await page.evaluate(() => flushCaseSaves()), true);
};

try {
  await page.goto(url); await ready();
  const bounded = await page.evaluate(async () => {
    const before = window.__mockCommandCalls.cases_save || 0;
    window.__mockLatency = { cases_save: 1500 };
    activeCase().name = "first snapshot";
    const first = saveCases();
    while ((window.__mockCommandCalls.cases_save || 0) === before) await new Promise(resolve => setTimeout(resolve, 10));
    const promises = new Set();
    for (let i = 0; i < 5; i++) {
      activeCase().name = `latest edit ${i}`; promises.add(saveCases());
      await new Promise(resolve => setTimeout(resolve, 225));
    }
    const whileBusy = (window.__mockCommandCalls.cases_save || 0) - before;
    window.__mockLatency = {};
    const saved = await first, pending = await Promise.all(promises);
    return { saved, pending, pendingPromises: promises.size, whileBusy,
      writes: (window.__mockCommandCalls.cases_save || 0) - before,
      disk: JSON.parse(localStorage.getItem("__mockStore")).cases[0].name, name: activeCase().name,
      revision: state.cases.revision, diskRevision: JSON.parse(localStorage.getItem("__mockStore")).revision };
  });
  assert.equal(bounded.saved, true); assert.deepEqual(bounded.pending, [true]);
  assert.equal(bounded.pendingPromises, 1); assert.equal(bounded.whileBusy, 1); assert.equal(bounded.writes, 2);
  assert.equal(bounded.disk, "latest edit 4"); assert.equal(bounded.name, bounded.disk); assert.equal(bounded.revision, bounded.diskRevision);
  results.bounded = bounded;

  const conflict = await page.evaluate(async () => {
    const external = JSON.parse(localStorage.getItem("__mockStore"));
    external.revision++; external.cases[0].name = "external revision";
    localStorage.setItem("__mockStore", JSON.stringify(external));
    activeCase().name = "stale local edit";
    const failed = saveCases(); const flushed = await flushCaseSaves();
    const diskAfterFailure = JSON.parse(localStorage.getItem("__mockStore"));
    await WorkspaceContext.replaceCases(normalizeCaseStore(await api("cases_load", {}, { silent: true })));
    activeCase().name = "fresh edit";
    const fresh = saveCases(); await flushCaseSaves();
    return { failed: await failed, flushed, diskAfterFailure: diskAfterFailure.cases[0].name,
      fresh: await fresh, disk: JSON.parse(localStorage.getItem("__mockStore")).cases[0].name, dirty: caseSavesPending() };
  });
  assert.deepEqual(conflict, { failed: false, flushed: false, diskAfterFailure: "external revision", fresh: true, disk: "fresh edit", dirty: false });
  results.conflict = conflict;

  const close = await page.evaluate(async () => {
    const attempt = async () => { const event = { prevented: false, preventDefault() { this.prevented = true; } }; await window.__testClose(event); return event.prevented; };
    window.__mockErrors = { cases_save: "Disco cheio (teste)" };
    activeCase().name = "close failure"; const failedSave = saveCases();
    const blocked = await attempt(), failed = await failedSave, dirty = caseSavesPending();
    window.__mockErrors = {}; activeCase().name = "durable before close";
    const prevented = await attempt();
    const before = window.__mockCommandCalls.cases_save;
    await flushCaseSaves(); await flushCaseSaves();
    const event = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(event);
    return { blocked, failed, dirty, prevented, pending: caseSavesPending(), idleWrites: window.__mockCommandCalls.cases_save - before,
      unloadBlocked: event.defaultPrevented, disk: JSON.parse(localStorage.getItem("__mockStore")).cases[0].name };
  });
  assert.deepEqual(close, { blocked: true, failed: false, dirty: true, prevented: false, pending: false, idleWrites: 0, unloadBlocked: false, disk: "durable before close" });
  results.close = close;
  await page.reload(); await ready();
  assert.equal(await page.evaluate(() => activeCase().name), "durable before close");
  results.reload = "latest committed content restored";
  assert.deepEqual(errors, []);
  writeFileSync(resolve(output, "case-autosave.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} finally {
  await browser.close();
}
