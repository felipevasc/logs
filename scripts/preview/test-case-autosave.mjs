/* Production bounded scheduling over native metadata tickets, receipts and retry. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
await page.addInitScript(() => {
  window.__mockNativeCaseBootstrapEnabled = true;
  Object.defineProperty(window, "__TAURI__", { configurable: true, set(value) {
    value.window = { getCurrentWindow: () => ({ onCloseRequested(handler) { window.__testClose = handler; return Promise.resolve(() => {}); } }) };
    Object.defineProperty(window, "__TAURI__", { value, configurable: true, writable: true });
  } });
});
try {
  await page.goto(process.argv[2] || process.env.PREVIEW_URL || "http://127.0.0.1:4181");
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && CaseEvidence.active && nativeEvidenceServices().session.isClean());
  results.bounded = await page.evaluate(async () => {
    const original = api, requests = [];
    let release, started;
    const firstStarted = new Promise(resolve => { started = resolve; });
    api = async (command, args, options) => {
      if (command === "cases_save_view") {
        requests.push(args.request);
        if (requests.length === 1) { started(); await new Promise(resolve => { release = resolve; }); }
      }
      return original(command, args, options);
    };
    try {
      activeCase().name = "first snapshot"; const first = saveCases(); await firstStarted;
      const pending = new Set();
      for (let i = 0; i < 1000; i++) { activeCase().name = `latest edit ${i}`; pending.add(saveCases()); }
      await new Promise(resolve => setTimeout(resolve, 250));
      const activeWrites = requests.length, closing = flushCaseSaves(); release();
      const saved = await first, confirmations = await Promise.all(pending), flushed = await closing;
      const native = __mockNativeCaseBootstrap.document();
      return { saved, confirmations, flushed, pendingPromises: pending.size, activeWrites, writes: requests.length,
        names: requests.map(request => JSON.parse(request.documentJson).cases[0].name),
        revisions: requests.map(request => request.expectedStore.revision),
        name: activeCase().name, nativeName: native.cases[0].name, revisionMatches: native.store.revision === state.cases.store.revision,
        referenceMatches: JSON.stringify(native.cases[0].items[0].rows.reference) === JSON.stringify(activeCase().items[0].rows.reference) };
    } finally { api = original; }
  });
  assert.equal(results.bounded.saved, true); assert.deepEqual(results.bounded.confirmations, [true]);
  assert.equal(results.bounded.flushed, true); assert.equal(results.bounded.pendingPromises, 1);
  assert.equal(results.bounded.activeWrites, 1); assert.equal(results.bounded.writes, 2);
  assert.deepEqual(results.bounded.names, ["first snapshot", "latest edit 999"]);
  assert.equal(BigInt(results.bounded.revisions[1]), BigInt(results.bounded.revisions[0]) + 1n);
  assert.equal(results.bounded.name, results.bounded.nativeName); assert.equal(results.bounded.revisionMatches, true); assert.equal(results.bounded.referenceMatches, true);

  results.close = await page.evaluate(async () => {
    const original = api, requests = []; let fail = true;
    const attempt = async () => { const event = { prevented: false, preventDefault() { this.prevented = true; } }; await __testClose(event); return event.prevented; };
    api = async (command, args, options) => {
      if (command === "cases_save_view") { requests.push(args.request); if (fail) throw Error("Disco cheio (teste)"); }
      return original(command, args, options);
    };
    try {
      activeCase().name = "failed native draft"; const saving = saveCases();
      const blocked = await attempt(), confirmed = await saving, blockedAgain = await attempt(), attemptsBeforeRetry = requests.length;
      fail = false; activeCase().name = "newer draft after failure";
      await nativeEvidenceServices().session.retry();
      const sameTicket = JSON.stringify(requests[0]) === JSON.stringify(requests[1]);
      const prevented = await attempt(), before = requests.length;
      await flushCaseSaves(); await flushCaseSaves();
      return { blocked, confirmed, blockedAgain, attemptsBeforeRetry, sameTicket, prevented, idleWrites: requests.length - before,
        dirty: caseSavesPending(), name: __mockNativeCaseBootstrap.document().cases[0].name };
    } finally { api = original; }
  });
  assert.deepEqual(results.close, { blocked: true, confirmed: false, blockedAgain: true, attemptsBeforeRetry: 1, sameTicket: true,
    prevented: false, idleWrites: 0, dirty: false, name: "newer draft after failure" });

  results.conflict = await page.evaluate(async () => {
    __mockNativeCaseBootstrap.publishName("external native revision");
    activeCase().name = "local draft kept for reconciliation"; const store = state.cases;
    const saving = saveCases(), flushed = await flushCaseSaves(), confirmed = await saving;
    const result = await nativeEvidenceServices().session.reconcile();
    return { confirmed, flushed, sameStore: state.cases === store, live: activeCase().name, draft: result.draft.cases[0].name,
      authoritative: result.authoritative.cases[0].name, retryTicketRetained: !!result.failedRequest, dirty: caseSavesPending(),
      legacyCalls: __mockNativeCaseBootstrap.calls.filter(call => ["cases_load", "cases_save", "case_sync"].includes(call.command)).length };
  });
  assert.deepEqual(results.conflict, { confirmed: false, flushed: false, sameStore: true, live: "local draft kept for reconciliation",
    draft: "local draft kept for reconciliation", authoritative: "external native revision", retryTicketRetained: true, dirty: true, legacyCalls: 0 });
  assert.deepEqual(errors, []);
  mkdirSync("output/playwright", { recursive: true });
  writeFileSync(resolve("output/playwright/case-autosave.json"), JSON.stringify(results, null, 2));
  console.log("Native autosave: one active/pending intent, exact retry, close flush, and non-destructive reconciliation passed.");
} finally { await browser.close(); }
