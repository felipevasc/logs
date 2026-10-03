// Deterministic regression of the real autosave and store-replacement code.
// No browser, Tauri, local investigations or filesystem writes are involved.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const current = readFileSync(new URL("../frontend/app.js", import.meta.url), "utf8");
const workspace = readFileSync(new URL("../frontend/workspace-context.js", import.meta.url), "utf8");
const replacement = workspace.slice(workspace.indexOf("async function replaceCases(store,"), workspace.indexOf("  let nativeMembership ="));
assert.ok(replacement.startsWith("async function replaceCases(store,"));
const copy = value => JSON.parse(JSON.stringify(value));
const turn = () => new Promise(resolve => setImmediate(resolve));
const store = (revision = 1, name = "local") => ({ revision, active: "c1", cases: [{ id: "c1", name }], schemaVersion: 2 });
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

function harness(source, responder) {
  const calls = [], toasts = [], timers = new Map(), listeners = new Map();
  let serial = 0, snapshots = 0, inFlight = 0, maxInFlight = 0, close;
  const context = vm.createContext({
    state: { cases: store(), refreshVersion: 0 },
    sourceQueue: Promise.resolve(), restoringCase: false, initialized: true,
    caseGeneration: 0, generation: 0, detailRequest: 0, scope: "case", states: new Map(), runtime: new Map(),
    renderCaseBar() {}, updateAnalysisBadge() {}, activeCase() { return context.state.cases.cases[0]; }, async loadDerivedFields() {}, async syncActiveCaseArtifacts() {}, async initialize() {},
    setTimeout(fn) { const id = ++serial; timers.set(id, fn); return id; },
    clearTimeout(id) { timers.delete(id); },
    toast(message, kind) { toasts.push({ message, kind }); },
    JSON: { parse: JSON.parse, stringify(value, ...args) { snapshots++; return JSON.stringify(value, ...args); } },
    window: {
      addEventListener(name, handler) { listeners.set(name, handler); },
      __TAURI__: { window: { getCurrentWindow: () => ({ onCloseRequested(handler) { close = handler; return Promise.resolve(() => {}); } }) } },
    },
    api(command, args) {
      assert.equal(command, "cases_save");
      const data = copy(args.data); calls.push(data);
      inFlight++; maxInFlight = Math.max(maxInFlight, inFlight);
      return Promise.resolve().then(() => responder ? responder(data, calls.length) : { revision: data.revision + 1 })
        .finally(() => { inFlight--; });
    },
  });
  const start = source.indexOf("let casesSaveQueue =");
  assert.ok(start >= 0);
  const block = source.slice(start, source.indexOf("function defaultCaseWorkspace()"));
  vm.runInContext(block + "\n" + replacement, context);
  return {
    context, calls, toasts,
    save: () => context.saveCases(),
    flush: () => context.flushCaseSaves(),
    dirty: () => context.caseSavesPending(),
    replace: next => context.replaceCases(next),
    get snapshots() { return snapshots; },
    get maxInFlight() { return maxInFlight; },
    get timers() { return timers.size; },
    async close() {
      assert.equal(typeof close, "function", "native close hook installed");
      const event = { prevented: false, preventDefault() { this.prevented = true; } };
      await close(event); return event;
    },
    unload() {
      const event = { prevented: false, preventDefault() { this.prevented = true; } };
      listeners.get("beforeunload")?.(event); return event;
    },
    runTimers() { for (const [id, fn] of [...timers]) { timers.delete(id); fn(); } },
    async flushTimers() { this.runTimers(); await turn(); },
  };
}

const tests = {
  async queued_snapshot_cannot_borrow_replacement_revision(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(); await h.flushTimers();
    h.context.state.cases.cases[0].name = "old-pending";
    const pending = h.save(); await h.flushTimers();
    const external = store(7, "external-new"); await h.replace(external);
    request.resolve({ revision: 2 }); assert.equal(await first, false); assert.equal(await pending, false);
    assert.equal(h.calls.length, 1); assert.equal(external.revision, 7);
    assert.ok(h.toasts.length > 0);
  },
  async late_response_cannot_update_replacement_store(source) {
    const request = deferred(), h = harness(source, () => request.promise);
    const saved = h.save(); await h.flushTimers(); assert.equal(h.calls.length, 1);
    const external = store(7, "external-new"); await h.replace(external);
    request.resolve({ revision: 2 }); assert.equal(await saved, false);
    assert.equal(h.context.state.cases, external); assert.equal(external.revision, 7);
  },
  async replacement_before_debounce_abandons_old_waiter(source) {
    const h = harness(source), saved = h.save();
    const external = store(7, "external-new"); await h.replace(external);
    await h.flushTimers(); assert.equal(await saved, false);
    assert.equal(h.calls.length, 0); assert.equal(external.revision, 7);
  },
  async debounce_does_not_confirm_foreign_waiters(source) {
    const h = harness(source), old = h.save();
    const external = store(7, "external-new"); await h.replace(external);
    const fresh = h.save(); await h.flushTimers();
    assert.equal(await old, false); assert.equal(await fresh, true);
    assert.equal(h.calls.length, 1); assert.equal(h.calls[0].cases[0].name, "external-new");
    assert.equal(h.calls[0].revision, 7); assert.equal(external.revision, 8);
  },
  async consecutive_same_store_saves_follow_committed_revision(source) {
    const requests = [deferred(), deferred()], h = harness(source, (_, index) => requests[index - 1].promise);
    h.context.state.cases.cases[0].name = "first";
    const first = h.save(); await h.flushTimers();
    h.context.state.cases.cases[0].name = "second";
    const second = h.save(); await h.flushTimers(); assert.equal(h.calls.length, 1);
    let secondDone = false; second.then(() => { secondDone = true; });
    requests[0].resolve({ revision: 2 }); assert.equal(await first, true); await turn();
    assert.equal(secondDone, false, "a later edit is not confirmed by an earlier snapshot");
    assert.equal(h.calls.length, 2); assert.equal(h.calls[1].revision, 2);
    assert.equal(h.calls[0].cases[0].name, "first"); assert.equal(h.calls[1].cases[0].name, "second");
    requests[1].resolve({ revision: 3 }); assert.equal(await second, true);
    assert.equal(h.context.state.cases.revision, 3); assert.equal(h.maxInFlight, 1);
  },
  async same_store_debounce_coalesces_and_confirms_all(source) {
    const h = harness(source), first = h.save();
    h.context.state.cases.cases[0].name = "latest";
    const second = h.save(); await h.flushTimers();
    assert.equal(await first, true); assert.equal(await second, true);
    assert.equal(h.calls.length, 1); assert.equal(h.calls[0].cases[0].name, "latest");
  },
  async failed_and_cancelled_saves_retry_without_advancing_revision(source) {
    for (const failure of ["Disco cheio", "Operação cancelada."]) {
      const h = harness(source, (data, index) => index === 1 ? Promise.reject(new Error(failure)) : Promise.resolve({ revision: data.revision + 1 }));
      const failed = h.save(); await h.flushTimers(); assert.equal(await failed, false);
      assert.equal(h.context.state.cases.revision, 1); assert.equal(h.toasts.length, 1);
      const retry = h.save(); await h.flushTimers(); assert.equal(await retry, true);
      assert.equal(h.calls[1].revision, 1); assert.equal(h.context.state.cases.revision, 2);
    }
  },
  async abandoned_batch_does_not_poison_fresh_store_queue(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const old = h.save(); await h.flushTimers(); const abandoned = h.save(); await h.flushTimers();
    await h.replace(store(9, "external")); const fresh = h.save(); await h.flushTimers();
    request.resolve({ revision: 2 }); assert.equal(await old, false); assert.equal(await abandoned, false); assert.equal(await fresh, true);
    assert.equal(h.calls.length, 2); assert.equal(h.calls[1].revision, 9);
  },
  async serialization_failure_settles_waiter_and_allows_retry(source) {
    const h = harness(source); h.context.state.cases.cycle = h.context.state.cases;
    const failed = h.save(); await h.flushTimers(); assert.equal(await failed, false);
    assert.equal(h.calls.length, 0); assert.equal(h.toasts.length, 1);
    delete h.context.state.cases.cycle;
    const retried = h.save(); await h.flushTimers(); assert.equal(await retried, true);
  },
  async slow_storage_retains_one_pending_batch_without_serializing_intermediate_edits(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(); await h.flushTimers();
    const waiting = new Set();
    for (let i = 0; i < 1000; i++) {
      h.context.state.cases.cases[0].name = `edit-${i}`;
      waiting.add(h.save()); h.runTimers();
    }
    assert.equal(h.snapshots, 1, "busy storage must not allocate queued snapshots");
    assert.equal(waiting.size, 1, "pending callers share one result without an internal waiter list");
    assert.equal(h.calls.length, 1);
    request.resolve({ revision: 2 }); assert.equal(await first, true);
    for (const saved of waiting) assert.equal(await saved, true);
    assert.equal(h.calls.length, 2); assert.equal(h.snapshots, 2);
    assert.equal(h.calls[1].cases[0].name, "edit-999"); assert.equal(h.calls[1].revision, 2);
    assert.equal(h.maxInFlight, 1); assert.equal(h.timers, 0);
  },
  async active_completion_respects_new_debounce(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(); await h.flushTimers();
    const second = h.save(); h.runTimers();
    h.context.state.cases.cases[0].name = "last edit resets debounce"; h.save();
    request.resolve({ revision: 2 }); assert.equal(await first, true); await turn();
    assert.equal(h.calls.length, 1, "new debounce must not be bypassed by I/O completion");
    await h.flushTimers(); assert.equal(await second, true);
    assert.equal(h.calls[1].cases[0].name, "last edit resets debounce");
  },
  async pending_batch_retries_latest_data_after_active_failure(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(); await h.flushTimers();
    h.context.state.cases.cases[0].name = "retry latest";
    const next = h.save(); await h.flushTimers();
    request.reject(new Error("Disco cheio")); assert.equal(await first, false); assert.equal(await next, true);
    assert.equal(h.calls[1].revision, 1); assert.equal(h.calls[1].cases[0].name, "retry latest");
    assert.equal(h.maxInFlight, 1); assert.equal(h.dirty(), false);
  },
  async invalid_backend_acknowledgements_never_confirm_persistence(source) {
    for (const response of [false, null, undefined, {}, { revision: false }, { revision: 0 }, { revision: 1 }, { revision: 1.5 }, { revision: Number.MAX_SAFE_INTEGER + 1 }]) {
      const h = harness(source, () => response), saved = h.save(); await h.flushTimers();
      assert.equal(await saved, false, `invalid response ${JSON.stringify(response)}`);
      assert.equal(h.context.state.cases.revision, 1); assert.equal(h.dirty(), true);
    }
  },
  async flush_idle_avoids_serialization_and_writes(source) {
    const h = harness(source); assert.equal(await h.flush(), true); assert.equal(await h.flush(), true);
    assert.equal(h.snapshots, 0); assert.equal(h.calls.length, 0); assert.equal(h.unload().prevented, false);
  },
  async flush_bypasses_debounce_and_leaves_no_timer(source) {
    const h = harness(source), saved = h.save();
    assert.equal(await h.flush(), true); assert.equal(await saved, true);
    assert.equal(h.timers, 0); assert.equal(h.calls.length, 1); assert.equal(h.dirty(), false);
    assert.equal(await h.flush(), true); assert.equal(h.snapshots, 1, "idle flush must not reserialize persisted data");
  },
  async flush_captures_edits_created_during_active_write(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(), flushed = h.flush(); await turn();
    h.context.state.cases.cases[0].name = "edited while closing";
    const second = h.save();
    request.resolve({ revision: 2 }); assert.equal(await first, true); assert.equal(await second, true);
    assert.equal(await flushed, true); assert.equal(h.calls.length, 2);
    assert.equal(h.calls[1].cases[0].name, "edited while closing"); assert.equal(h.timers, 0);
  },
  async flush_includes_followup_queued_by_a_completed_save(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    const first = h.save(), flushed = h.flush();
    let followup;
    first.then(() => { h.context.state.cases.cases[0].name = "save completion followup"; followup = h.save(); });
    request.resolve({ revision: 2 });
    assert.equal(await flushed, true);
    assert.equal(h.calls.length, 2, "flush must include edits queued by save completion callbacks");
    assert.equal(await followup, true); assert.equal(h.dirty(), false);
  },
  async flush_failure_does_not_spin_and_close_retries(source) {
    const h = harness(source, (data, index) => index === 1 ? Promise.reject(new Error("Disco cheio")) : { revision: data.revision + 1 });
    const saved = h.save(), closed = await h.close();
    assert.equal(closed.prevented, true); assert.equal(await saved, false); assert.equal(h.calls.length, 1); assert.equal(h.dirty(), true);
    h.context.state.cases.cases[0].name = "still editable";
    assert.equal((await h.close()).prevented, false);
    assert.equal(h.calls[1].cases[0].name, "still editable"); assert.equal(h.dirty(), false);
  },
  async close_waits_for_active_and_pending_saves(source) {
    const request = deferred(), h = harness(source, (data, index) => index === 1 ? request.promise : { revision: data.revision + 1 });
    assert.equal(typeof h.context.flushCaseSaves, "function", "flush API available");
    const saved = h.save(); await h.flushTimers();
    h.context.state.cases.cases[0].name = "pending on close"; const pending = h.save();
    let closed = false; const closing = h.close().then(event => { closed = true; return event; });
    await turn(); assert.equal(closed, false);
    assert.equal((await h.close()).prevented, true, "a second close request cannot bypass the first flush");
    request.resolve({ revision: 2 }); assert.equal(await saved, true); assert.equal(await pending, true);
    assert.equal((await closing).prevented, false); assert.equal(h.calls.length, 2); assert.equal(h.dirty(), false);
  },
  async beforeunload_warns_only_while_unsaved_and_flushes(source) {
    const request = deferred(), h = harness(source, () => request.promise);
    assert.equal(h.unload().prevented, false);
    const saved = h.save(); assert.equal(h.unload().prevented, true); await turn();
    assert.equal(h.calls.length, 1); request.resolve({ revision: 2 }); assert.equal(await saved, true);
    assert.equal(h.unload().prevented, false); assert.equal(h.calls.length, 1);
  },
  async replaced_failed_store_does_not_retain_dirty_state(source) {
    const h = harness(source, () => Promise.reject(new Error("Revisão conflitante")));
    const saved = h.save(); await h.flushTimers(); assert.equal(await saved, false);
    await h.replace(store(8, "reloaded")); assert.equal(h.dirty(), false);
    assert.equal(await h.flush(), true); assert.equal(h.calls.length, 1); assert.equal(h.unload().prevented, false);
  },
};

async function run(source) {
  const passed = [], failed = [];
  for (const [name, test] of Object.entries(tests)) {
    let timeout;
    try {
      await Promise.race([test(source), new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("unsettled save/flush after 2s")), 2000); })]);
      passed.push(name);
    }
    catch (error) { failed.push({ name, error: error.message }); }
    finally { clearTimeout(timeout); }
  }
  return { passed, failed };
}
const baselineArg = process.argv.indexOf("--baseline");
const baseline = baselineArg >= 0 ? await run(readFileSync(process.argv[baselineArg + 1], "utf8")) : undefined;
const result = await run(current);
console.log(JSON.stringify({ baseline, current: result }, null, 2));
assert.equal(result.failed.length, 0, "current autosave regressions");
if (baseline) assert.ok(baseline.failed.some(test => test.name === "slow_storage_retains_one_pending_batch_without_serializing_intermediate_edits"), "baseline must reproduce unbounded snapshot queue");
