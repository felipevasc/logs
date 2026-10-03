import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../../frontend/security.js', import.meta.url), 'utf8');
const tick = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function fixture() {
  const calls = [], cancelled = [], listeners = new Map();
  const state = { active: 'a', revision: 1, derivedFields: [], currentArtifact: { id: 'source', loadedAt: 1 } };
  const capture = () => ({ caseId: state.active, instance: state.active, identity: { revision: state.revision }, source: state.currentArtifact.id });
  const helper = { capture, prepare: async owner => { helper.assertOwner(owner); return owner; }, assertOwner: owner => { if (JSON.stringify(owner) !== JSON.stringify(capture())) throw Error('ANALYSIS_CONTEXT_CHANGED'); } };
  const document = { body: { dataset: {} }, addEventListener(name, fn) { if (!listeners.has(name)) listeners.set(name, []); listeners.get(name).push(fn); }, dispatchEvent(event) { for (const fn of listeners.get(event.type) || []) fn(event); } };
  const context = vm.createContext({ state, window: { EvidenceUI: { label: String, control: () => '<div class="evidence-control"></div>' }, AnalysisContexts: helper, Tasks: { operationFor: () => null, cancelLatest: key => cancelled.push(key) } }, document,
    CustomEvent: class { constructor(type, options = {}) { this.type = type; this.detail = options.detail; } }, workspaceScope: () => 'dataset', caseSig: () => '',
    esc: String, fmtNum: String, updateContextBar() {}, api: (command, args, opts) => new Promise((resolve, reject) => calls.push({ command, args, opts, resolve, reject })) });
  vm.runInContext(source, context);
  const security = context.window.Security, data = id => ({ analysis_id: id, detections: [], episodes: [], total: 3 });
  const calculate = async id => { const request = security.get(); await tick(); calls.at(-1).resolve(data(id)); await request; };
  return { state, security, calls, cancelled, calculate, data, document };
}
test('Comprometimentos starts only on request and coalesces repeated/forced clicks', async () => {
  const f = fixture(); assert.equal(f.security.status().state, 'idle'); assert.equal(f.calls.length, 0);
  const a = f.security.get(), b = f.security.get({ force: true }); await tick();
  assert.equal(a, b); assert.equal(f.calls.length, 1); assert.equal(f.security.status().state, 'calculating');
  f.calls[0].resolve(f.data('first')); await a;
  assert.equal(f.security.status().state, 'ready'); await f.security.get(); assert.equal(f.calls.length, 1);
});
test('Case caches are isolated and returning to an unchanged Case preserves valid results', async () => {
  const f = fixture(); await f.calculate('a-result'); f.state.active = 'b';
  assert.equal(f.security.last(), null); assert.equal(f.security.status().state, 'idle');
  await f.calculate('b-result'); f.security.invalidate(); assert.equal(f.security.status().state, 'stale');
  f.state.active = 'a'; assert.equal(f.security.last().analysis_id, 'a-result'); assert.equal(f.security.status().state, 'ready');
  assert.equal(f.calls.length, 2);
});
test('data, rules and derived field changes are stale without launching scans', async () => {
  const f = fixture(); await f.calculate('old'); f.state.currentArtifact.id = 'new';
  assert.equal(f.security.status().state, 'stale'); assert.equal(f.security.cached(), null);
  f.state.revision++; f.state.derivedFields.push({ name: 'updated' }); f.security.invalidate();
  assert.equal(f.security.status().state, 'stale'); assert.equal(f.calls.length, 1);
});
test('cancel rejects late results, preserves the last valid result, and needs a new action to retry', async () => {
  const f = fixture(); await f.calculate('saved');
  const request = f.security.get({ force: true }); await tick(); f.security.cancel();
  assert.equal(f.security.status().state, 'cancelling'); assert.equal(f.cancelled.length, 1);
  f.calls[1].resolve(f.data('late')); await assert.rejects(request, /cancelada/);
  assert.equal(f.security.status().state, 'cancelled'); assert.equal(f.security.cached().analysis_id, 'saved');
  assert.equal(f.calls.length, 2);
});
test('switching Case rejects late results and never applies an old failure to the active Case', async () => {
  const f = fixture(), request = f.security.get(); await tick(); f.state.active = 'b';
  f.document.dispatchEvent({ type: 'workspace-context-change' });
  f.calls[0].resolve(f.data('a-late')); await assert.rejects(request);
  assert.equal(f.security.cached(), null); assert.equal(f.security.status().state, 'idle'); assert.equal(f.calls.length, 1);
});
test('error, empty computed result and not calculated are distinct states', async () => {
  const f = fixture(), request = f.security.get(); await tick(); f.calls[0].reject(Error('disk unavailable')); await assert.rejects(request);
  assert.equal(f.security.status().state, 'failed'); assert.equal(f.security.cached(), null);
  await f.calculate('empty'); assert.equal(f.security.status().state, 'ready'); assert.equal(f.security.cached().detections.length, 0);
});
test('summary, page and Timeline rendering never invoke triage', async () => {
  const f = fixture();
  const node = () => ({ isConnected: true, dataset: {}, innerHTML: '', children: new Map(), setAttribute() {}, removeAttribute() {},
    querySelector(selector) { if (!this.children.has(selector)) this.children.set(selector, node()); return this.children.get(selector); },
    querySelectorAll: () => [], addEventListener() {}, remove() {},
  });
  for (let i = 0; i < 3; i++) {
    const summary = node(), page = node(), timeline = node();
    await f.security.fillSummary({ attention: summary }); await f.security.renderPage(page);
    f.security.markers(timeline, 1, 10, () => {});
    assert.equal(summary.dataset.analysisState, 'idle');
    assert.equal(page.querySelector('[data-compromises-results]').dataset.analysisState, 'idle');
    assert.match(summary.innerHTML, /ainda não calculados/);
    assert.doesNotMatch(summary.innerHTML, /Nenhum indício/);
  }
  assert.equal(f.calls.length, 0);
});
test('cancelled recalculation keeps the previous results accessible in the summary', async () => {
  const f = fixture(); await f.calculate('previous');
  const pending = f.security.get({ force: true }); await tick(); f.security.cancel(); f.calls.at(-1).resolve(f.data('late'));
  await assert.rejects(pending, /cancelada/);
  const node = () => ({ isConnected: true, dataset: {}, children: new Map(), setAttribute() {}, removeAttribute() {},
    querySelector(selector) { if (!this.children.has(selector)) this.children.set(selector, node()); return this.children.get(selector); }, querySelectorAll: () => [], addEventListener() {},
  });
  const summary = node(); await f.security.fillSummary({ attention: summary });
  assert.equal(summary.dataset.analysisState, 'ready'); assert.match(summary.innerHTML, /Exibindo o resultado válido anterior/);
  assert.match(summary.innerHTML, /sec-count-chart/); assert.equal(f.calls.length, 2);
});
test('cancelling an unrelated unnamed task cannot cancel a preparing calculation', async () => {
  const f = fixture(), request = f.security.get();
  f.document.dispatchEvent({ type: 'task-state-change', detail: { operationId: null, state: 'cancelling' } });
  await tick(); assert.equal(f.calls.length, 1); assert.equal(f.security.status().state, 'calculating');
  f.calls[0].resolve(f.data('requested')); await request; assert.equal(f.security.status().state, 'ready');
});

test('the results DOM marker survives idle, busy, ready, stale, failed and cancelled views', async () => {
  const f = fixture();
  const node = () => ({ isConnected: true, dataset: {}, innerHTML: '', children: new Map(), setAttribute() {}, removeAttribute() {},
    querySelector(selector) { if (!this.children.has(selector)) this.children.set(selector, node()); return this.children.get(selector); },
    querySelectorAll: () => [], addEventListener() {}, remove() {},
  });
  const host = node(); await f.security.renderPage(host);
  assert.match(host.innerHTML, /class="sec-page-results" data-compromises-results/);
  const slot = host.querySelector('[data-compromises-results]');
  assert.equal(slot.dataset.analysisState, 'idle');
  const first = f.security.get(); await tick(); assert.equal(slot.dataset.analysisState, 'calculating');
  f.calls.at(-1).resolve(f.data('empty')); await first; assert.equal(slot.dataset.analysisState, 'ready');
  assert.equal(slot.className, 'sec-clear', 'state styling can change without replacing the stable DOM marker');
  f.security.invalidate(); assert.equal(slot.dataset.analysisState, 'stale');
  const failure = f.security.get(); await tick(); f.calls.at(-1).reject(Error('read failed')); await assert.rejects(failure);
  assert.equal(slot.dataset.analysisState, 'failed');
  const cancelled = f.security.get(); await tick(); f.security.cancel();
  assert.equal(slot.dataset.analysisState, 'cancelling');
  f.calls.at(-1).resolve(f.data('late')); await assert.rejects(cancelled);
  assert.equal(slot.dataset.analysisState, 'cancelled');
  assert.equal(host.querySelector('[data-compromises-results]'), slot);
});

test('submitting a search creates a persistent filter that clearing the legacy draft does not remove', () => {
  const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
  const commit = app.slice(app.indexOf('function commitQuickSearch()'), app.indexOf('function applyFilterPop()'));
  const expression = '@action:logon @outcome:failure', input = { value: expression }, state = { quick: '', filters: [] };
  const context = vm.createContext({ state, $: selector => selector === '#quick-search' ? input : {},
    window: { QueryLang: { validate: () => null }, QueryBar: { status() {}, clearDraft() {} } },
    addFilter: filter => state.filters.push(filter), renderChips() {}, filtersChanged() {},
  });
  vm.runInContext(commit + '\nthis.commit = commitQuickSearch;', context);
  assert.equal(context.commit(), true); input.value = ''; state.quick = '';
  assert.equal(state.filters.length, 1); assert.equal(state.filters[0].value, expression);
  assert.equal(state.filters[0].op, 'query', 'Timeline still receives the committed search chip');
});
