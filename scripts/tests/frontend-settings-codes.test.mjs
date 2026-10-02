import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const section = (start, end) => { const from = app.indexOf(start), to = app.indexOf(end, from); assert.ok(from >= 0 && to > from); return app.slice(from, to); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
function fixture({ ownership = false } = {}) {
  const nodes = new Map(), requests = [], pathRequests = [], messages = [], confirmations = [], listeners = new Map(); let approval = false, refreshes = 0, delayedPath = false;
  let document;
  class Node {
    constructor(id, tag = 'div') { this.id = id; this.tagName = tag.toUpperCase(); this.value = ''; this.textContent = ''; this.hidden = false; this.disabled = false; this.attrs = {}; this.dataset = {}; this.tabIndex = 0; this.isConnected = true; this.classes = new Set(); this.classList = { add: name => this.classes.add(name), toggle: (name, on) => on ? this.classes.add(name) : this.classes.delete(name) }; }
    setAttribute(name, value) { this.attrs[name] = String(value); }
    getAttribute(name) { return this.attrs[name]; }
    append(node) { node.parentElement = this; }
    focus() { document.activeElement = this; }
    getClientRects() { return this.hidden ? [] : [{}]; }
    addEventListener(type, fn) { this['on' + type] = fn; }
  }
  const node = (id, tag) => { const value = new Node(id, tag); nodes.set('#' + id, value); return value; };
  const tabs = ['interface', 'codes', 'mcp', 'detection', 'recovery', 'updates'].map(name => { const value = node('settings-tab-' + name, 'button'); value.dataset.settingsTab = name; return value; });
  const panes = tabs.map(tab => node('settings-pane-' + tab.dataset.settingsTab));
  for (const id of ['settings-modal','codes-modal','sys-count','codes-path','codes-status','codes-reload','codes-save','codes-cancel','codes-close','settings-close','btn-harvest','btn-settings','drawer','filter-pop','name-pop','detail-value-modal','col-pop']) node(id, /close|save|reload|cancel|btn-/.test(id) ? 'button' : 'div');
  const editor = node('codes-editor', 'textarea');
  for (const id of ['settings-modal','drawer','filter-pop','name-pop','detail-value-modal','col-pop']) nodes.get('#' + id).hidden = true;
  const body = new Node('codes-body'); body.parentElement = nodes.get('#codes-modal'); nodes.set('#codes-modal .modal-body', body);
  const $ = selector => nodes.get(selector);
  document = { activeElement: $('#btn-settings'), querySelector: selector => selector === '[data-settings-tab="recovery"]' ? tabs[4] : $(selector),
    querySelectorAll: selector => selector.endsWith('.settings-tab') ? tabs : selector.endsWith('.settings-pane') ? panes : [],
    addEventListener(type, fn) { listeners.set(type, [...listeners.get(type) || [], fn]); } };
  const api = (command, args = {}, options = {}) => {
    if (command === 'get_codes_path') { if (!delayedPath) return Promise.resolve('/catalog.json'); const pending = deferred(); pathRequests.push(pending); return pending.promise; }
    if (command === 'system_codes_count') return Promise.resolve(123);
    const pending = deferred(); requests.push({ command, args, options, ...pending }); return pending.promise;
  };
  const cases = [{ id: 'a', revision: 1 }, { id: 'b', revision: 1 }], state = { rows: [], cases: { active: 'a', cases } };
  const helper = { capture: () => { const item = cases.find(value => value.id === state.cases.active); return { item, caseId: item.id, revision: item.revision }; },
    assertOwner: (owner, { revisions = true } = {}) => { if (cases.find(value => value.id === state.cases.active) !== owner.item || revisions && owner.revision !== owner.item.revision) throw Error('ANALYSIS_CONTEXT_CHANGED'); },
    prepare: async owner => { helper.assertOwner(owner); return owner; } };
  const context = vm.createContext({ document, $, api, confirm: text => { confirmations.push(text); return approval; },
    toast: (text, kind) => messages.push({ text, kind }), refresh: () => { refreshes++; return Promise.resolve(); },
    updateSysCount() {}, nativeEvidenceEnabled: () => false,
    window: { ...(ownership ? { AnalysisContexts: helper } : {}), UiScale: { renderPane() {} }, Security: { renderRulesPane() {} }, Updates: { renderPane() {} } }, renderMcpPane() {},
    ctxEl: null, namePopExact: false, state, fmtNum: String, closeDrawer() { throw Error('Escape reached a lower surface'); },
  });
  vm.runInContext(section('// The codes catalog is a section of Settings.', 'async function updateSysCount('), context);
  if (ownership) vm.runInContext(section('async function updateSysCount(', 'async function saveCodes()'), context);
  vm.runInContext(section('async function saveCodes()', '// ------------------------------------------------------------------ configurações / MCP'), context);
  vm.runInContext(section('function switchSettingsTab(', 'async function renderMcpPane()'), context);
  vm.runInContext(section('  $("#btn-settings").onclick =', '  $("#btn-theme").onclick ='), context);
  vm.runInContext(section('function bindKeyboard()', '// ------------------------------------------------------------------ init'), context); context.bindKeyboard();
  const edit = text => { editor.value = text; context.codesEdited(); };
  const open = async (catalog = '{"saved":1}') => { const pending = context.openSettings('codes'); await tick(); requests.at(-1).resolve(catalog); await pending; };
  return { context, state, cases, helper, switchCase: id => { state.cases.active = id; }, $, tabs, panes, document, requests, pathRequests, messages, confirmations, editor, edit, open, tick, delayPath: () => { delayedPath = true; },
    allow: value => { approval = value; }, refreshes: () => refreshes,
    key(key, extras = {}) { const event = { key, ...extras, preventDefault() { this.prevented = true; }, stopImmediatePropagation() { this.stopped = true; }, stopPropagation() { this.stopped = true; } }; for (const fn of listeners.get('keydown') || []) { fn(event); if (event.stopped) break; } return event; },
  };
}

test('catalog draft survives repeated tab clicks and leaving/returning without another read', async () => {
  const f = fixture(); await f.open(); f.edit('{"draft":2}');
  await f.context.openSettings('codes'); await f.context.openSettings('interface'); await f.context.openSettings('codes');
  assert.equal(f.editor.value, '{"draft":2}'); assert.equal(f.requests.length, 1); assert.match(f.$('#codes-status').textContent, /não salvas/);
});
test('two rapid tab switches share the initial read and preserve the selected pane/focus', async () => {
  const f = fixture(), pending = f.context.openSettings('codes'); await f.tick();
  await f.context.openSettings('interface'); await f.context.openSettings('codes');
  assert.equal(f.requests.length, 1); f.requests[0].resolve('{"fresh":3}'); await pending;
  assert.equal(f.editor.value, '{"fresh":3}'); assert.equal(f.$('#settings-pane-codes').hidden, false);
});
test('late fetch from a closed session cannot replace a reopened draft or path', async () => {
  const f = fixture(), old = f.context.openSettings('codes'); await f.tick(); assert.equal(f.context.closeSettings(), true);
  const current = f.context.openSettings('codes'); await f.tick(); f.requests[1].resolve('{"new":1}'); await current; f.edit('{"newDraft":2}');
  f.requests[0].resolve('{"old":1}'); await old; assert.equal(f.editor.value, '{"newDraft":2}');
});
test('typing during deliberate reload defeats the older response; declining discard sends no read', async () => {
  const f = fixture(); await f.open(); f.edit('{"draft":2}');
  await f.context.reloadCodesPane({ deliberate: true }); assert.equal(f.requests.length, 1);
  f.allow(true); const loading = f.context.reloadCodesPane({ deliberate: true }); await f.tick(); f.edit('{"newest":3}');
  f.requests.at(-1).resolve('{"stored":4}'); await loading;
  assert.equal(f.editor.value, '{"newest":3}'); assert.match(f.$('#codes-status').textContent, /mais recente foi mantida/);
  assert.equal(f.$('#codes-save').disabled, false);
});
test('read failure is visible, retains a loaded draft and allows deliberate retry', async () => {
  const f = fixture(), initial = f.context.openSettings('codes'); await f.tick(); f.requests[0].reject(Error('offline')); await initial;
  assert.match(f.$('#codes-status').textContent, /offline/); assert.equal(f.editor.disabled, true); assert.equal(f.$('#codes-reload').disabled, false);
  const retry = f.context.reloadCodesPane({ deliberate: true }); f.requests[1].resolve('{"ok":1}'); await retry;
  f.edit('{"kept":2}'); f.allow(true); const reload = f.context.reloadCodesPane({ deliberate: true }); f.requests[2].reject(Error('again')); await reload;
  assert.equal(f.editor.value, '{"kept":2}'); assert.equal(f.editor.disabled, false); assert.match(f.$('#codes-status').textContent, /again/);
});
test('MCP updates are explicit, retain a draft off-tab and invalidate an older fetch', async () => {
  const f = fixture(); await f.open(); f.edit('{"local":2}'); await f.context.openSettings('interface');
  f.context.codesChangedExternally(); await f.context.openSettings('codes'); assert.equal(f.editor.value, '{"local":2}');
  assert.match(f.$('#codes-status').textContent, /mudou fora/); await f.context.saveCodes(); assert.equal(f.requests.length, 1);
  f.allow(true); const reload = f.context.reloadCodesPane({ deliberate: true }); f.context.codesChangedExternally(); f.requests[1].resolve('{"stale":3}'); await reload;
  assert.equal(f.editor.value, '{"local":2}'); assert.equal(f.$('#codes-reload').disabled, false);
  const fresh = f.context.reloadCodesPane({ deliberate: true }); f.requests[2].resolve('{"external":4}'); await fresh;
  assert.equal(f.editor.value, '{"external":4}'); assert.doesNotMatch(f.$('#codes-status').textContent, /mudou fora|não salvas/);
});
test('save captures text once, blocks duplicates and does not declare later typing saved', async () => {
  const f = fixture(); await f.open(); f.edit('{"submitted":2}'); const saving = f.context.saveCodes(); await f.context.saveCodes();
  assert.equal(f.requests.length, 2); assert.equal(f.requests[1].command, 'save_codes'); assert.deepEqual(JSON.parse(JSON.stringify(f.requests[1].args)), { text: '{"submitted":2}' });
  f.edit('{"later":3}'); f.requests[1].resolve(); await saving;
  assert.equal(f.editor.value, '{"later":3}'); assert.match(f.$('#codes-status').textContent, /edição posterior ainda não foi salva/); assert.equal(f.refreshes(), 1);
  assert.equal(f.context.codesDraftChanged(), true); assert.equal(f.$('#codes-save').disabled, false);
});
test('save failure preserves editable draft and successful retry reports only its own receipt', async () => {
  const f = fixture(); await f.open(); f.edit('{"sent":2}'); const saving = f.context.saveCodes(); f.requests[1].reject(Error('invalid JSON')); await saving;
  assert.equal(f.editor.value, '{"sent":2}'); assert.match(f.$('#codes-status').textContent, /invalid JSON/); assert.equal(f.refreshes(), 0);
  const retry = f.context.saveCodes(); f.requests[2].resolve(); await retry;
  assert.equal(f.context.codesDraftChanged(), false); assert.match(f.$('#codes-status').textContent, /Catálogo salvo/);
});
test('external update arriving during save is not cleared by its earlier receipt', async () => {
  const f = fixture(); await f.open(); f.edit('{"sent":2}'); const saving = f.context.saveCodes(); f.context.codesChangedExternally(); f.requests[1].resolve(); await saving;
  assert.match(f.$('#codes-status').textContent, /mudou fora/);
});
test('close warns once about both draft and pending save; reopen waits and ignores its old UI receipt', async () => {
  const f = fixture(); await f.open(); f.edit('{"sent":2}'); const saving = f.context.saveCodes(); f.edit('{"later":3}');
  assert.equal(f.context.closeSettings(), false); assert.equal(f.confirmations.length, 1); assert.match(f.confirmations[0], /não salvas.*gravação já enviada continuará/);
  f.allow(true); assert.equal(f.context.closeSettings(), true); const reopened = f.context.openSettings('codes'); await f.tick();
  assert.equal(f.requests.length, 2, 'new editor waits for existing write'); assert.equal(f.editor.disabled, true);
  f.requests[1].resolve(); await saving; await f.tick(); assert.equal(f.requests[2].command, 'get_codes');
  f.requests[2].resolve('{"sent":2}'); await reopened; assert.equal(f.editor.value, '{"sent":2}'); assert.doesNotMatch(f.$('#codes-status').textContent, /Catálogo salvo/);
});
test('Escape declining discard stops before lower surfaces, then accepted close returns focus', async () => {
  const f = fixture(); await f.open(); f.edit('{"draft":2}'); const event = f.key('Escape');
  assert.equal(event.prevented, true); assert.equal(event.stopped, true); assert.equal(f.$('#settings-modal').hidden, false);
  f.allow(true); f.key('Escape'); assert.equal(f.$('#settings-modal').hidden, true); assert.equal(f.document.activeElement, f.$('#btn-settings'));
});
test('wrapped settings tabs retain manual activation, selected state and one roving tab stop', async () => {
  const f = fixture(); await f.context.openSettings(); assert.equal(f.document.activeElement, f.tabs[0]);
  assert.equal(f.tabs[0].getAttribute('aria-selected'), 'true'); assert.equal(f.tabs[1].getAttribute('aria-controls'), 'settings-pane-codes');
  const e = key => ({ key, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } });
  const right = e('ArrowRight'); f.tabs[0].onkeydown(right); assert.equal(right.stopped, true); assert.equal(f.document.activeElement, f.tabs[1]);
  assert.equal(f.requests.length, 0, 'moving tab focus does not query'); assert.equal(f.$('#settings-pane-interface').hidden, false);
  f.tabs[1].onkeydown(e('End')); assert.equal(f.document.activeElement, f.tabs[5]);
  f.tabs[5].onkeydown(e('ArrowLeft')); assert.equal(f.document.activeElement, f.tabs[3], 'hidden recovery tab is skipped');
  assert.equal(f.tabs.filter(tab => !tab.hidden && tab.tabIndex === 0).length, 1);
  const css = readFileSync(new URL('../../frontend/styles.css', import.meta.url), 'utf8');
  assert.match(css, /\.settings-tabs\s*\{[^}]*flex-wrap:\s*wrap/); assert.match(css, /\.settings-tab\s*\{[^}]*min-height:\s*36px/);
});

test('disabling a focused save or reload keeps focus in the editable catalog without stealing later focus', async () => {
  const f = fixture(); await f.open(); f.edit('{"sent":2}'); f.$('#codes-save').focus();
  const saving = f.context.saveCodes(); assert.equal(f.document.activeElement, f.editor);
  f.tabs[0].focus(); f.requests[1].resolve(); await saving; assert.equal(f.document.activeElement, f.tabs[0]);
  f.$('#codes-reload').focus(); const reload = f.context.reloadCodesPane({ deliberate: true }); assert.equal(f.document.activeElement, f.editor);
  f.tabs[1].focus(); f.requests[2].resolve('{"new":3}'); await reload; assert.equal(f.document.activeElement, f.tabs[1]);
});

test('explorer shortcuts do not move focus or selection behind open Settings', async () => {
  const f = fixture(); await f.context.openSettings();
  for (const key of ['/', 'ArrowLeft', 'ArrowRight']) { f.key(key); assert.equal(f.document.activeElement, f.tabs[0]); }
});


test('independently delayed catalog path cannot repaint a newer session', async () => {
  const f = fixture(); f.delayPath(); const first = f.context.openSettings('codes'); await f.tick(); f.context.closeSettings();
  const second = f.context.openSettings('codes'); await f.tick(); f.requests[1].resolve('{}'); f.pathRequests[1].resolve('/new/catalog.json'); await second;
  f.requests[0].resolve('{"old":1}'); f.pathRequests[0].resolve('/old/catalog.json'); await first; await f.tick();
  assert.equal(f.$('#codes-path').textContent, '/new/catalog.json'); assert.equal(f.editor.value, '{}');
});

test('pending save rejection after reopening reports failure and reads the still-persisted catalog', async () => {
  const f = fixture(); await f.open(); f.edit('{"sent":2}'); const saving = f.context.saveCodes(); f.allow(true); f.context.closeSettings();
  const reopened = f.context.openSettings('codes'); await f.tick(); assert.equal(f.requests.length, 2);
  f.requests[1].reject(Error('write rejected')); await saving; await f.tick();
  assert.match(f.messages.at(-1).text, /write rejected/); assert.equal(f.requests[2].command, 'get_codes');
  f.requests[2].resolve('{"saved":1}'); await reopened;
  assert.equal(f.editor.value, '{"saved":1}'); assert.equal(f.editor.disabled, false); assert.equal(f.$('#codes-save').disabled, false);
  assert.doesNotMatch(f.$('#codes-status').textContent, /salvo|salva/);
});

test('catalog status writes once per state change rather than repeating unchanged dirty text on input', async () => {
  const f = fixture(); await f.open(); const status = f.$('#codes-status');
  let text = status.textContent, writes = 0;
  Object.defineProperty(status, 'textContent', { get: () => text, set: value => { writes++; text = value; } });
  f.edit('{"draft":2}'); assert.equal(writes, 1); assert.equal(text, 'Há alterações não salvas.');
  f.edit('{"draft":3}'); f.edit('{"draft":4}');
  assert.equal(writes, 1, 'two further dirty edits do not mutate the unchanged status text');
  f.edit('{"saved":1}'); assert.equal(writes, 2); assert.equal(text, ''); assert.equal(status.hidden, true);
});

test('Case-owned catalog reads for A cannot paint over the editor opened for B', async () => {
  const f = fixture({ ownership: true }), first = f.context.openSettings('codes'); await f.tick();
  assert.equal(f.requests[0].options.analysisOwner.caseId, 'a'); f.switchCase('b');
  const second = f.context.openSettings('codes'); await f.tick(); assert.equal(f.requests[1].options.analysisOwner.caseId, 'b');
  f.requests[1].resolve('{"B":1}'); await second; f.edit('{"B draft":2}');
  f.requests[0].resolve('{"A":1}'); await first;
  assert.equal(f.editor.value, '{"B draft":2}'); assert.equal(f.context.codesDraftChanged(), true);
});
test('late Case A save waits durably but cannot refresh or replace the reopened Case B editor', async () => {
  const f = fixture({ ownership: true }); await f.open('{"A":1}'); f.edit('{"saved A":2}');
  const saving = f.context.saveCodes(); assert.equal(f.requests[1].options.analysisOwner.caseId, 'a');
  f.switchCase('b'); const reopened = f.context.openSettings('codes'); await f.tick(); assert.equal(f.requests.length, 2);
  f.cases[0].revision++; f.requests[1].resolve({ analysisContext: { caseId: 'a' } }); await saving; await f.tick();
  assert.equal(f.refreshes(), 0); assert.equal(f.requests[2].options.analysisOwner.caseId, 'b');
  f.requests[2].resolve('{"B":1}'); await reopened; assert.equal(f.editor.value, '{"B":1}');
  assert.doesNotMatch(f.$('#codes-status').textContent, /Catálogo salvo/);
});
test('same-Case successful receipt advances editor CAS owner for a second save', async () => {
  const f = fixture({ ownership: true }); await f.open(); f.edit('{"one":1}');
  const first = f.context.saveCodes(); f.cases[0].revision++; f.requests[1].resolve({}); await first;
  f.edit('{"two":2}'); const second = f.context.saveCodes(); assert.equal(f.requests[2].options.analysisOwner.revision, 2);
  f.cases[0].revision++; f.requests[2].resolve({}); await second; assert.equal(f.context.codesDraftChanged(), false);
});
test('switching Cases fences old Save/Harvest actions and late harvest/count receipts', async () => {
  const f = fixture({ ownership: true }); await f.open(); f.edit('{"A draft":2}'); f.switchCase('b');
  await f.context.saveCodes(); await f.context.runHarvest(); assert.equal(f.requests.length, 1);
  f.switchCase('a'); const harvesting = f.context.runHarvest(); assert.equal(f.requests[1].options.analysisOwner.caseId, 'a');
  f.switchCase('b'); const opening = f.context.openSettings('codes'); await f.tick(); f.requests[2].resolve('{"B":1}'); await opening;
  f.cases[0].revision++; f.requests[1].resolve({ count: 987, sources: 2, analysisContext: { caseId: 'a' } }); await harvesting;
  assert.equal(f.$('#sys-count').textContent, '123'); assert.equal(f.refreshes(), 0); assert.equal(f.editor.value, '{"B":1}');
});
test('same-Case external revision changes preserve the draft and require a deliberate reload', async () => {
  const f = fixture({ ownership: true }); await f.open(); f.edit('{"keep draft":2}'); f.cases[0].revision++;
  await f.context.openSettings('interface'); await f.context.openSettings('codes');
  assert.equal(f.editor.value, '{"keep draft":2}'); assert.equal(f.requests.length, 1); assert.equal(f.$('#codes-save').disabled, true);
  f.allow(true); const reload = f.context.reloadCodesPane({ deliberate: true }); await f.tick();
  assert.equal(f.requests[1].options.analysisOwner.revision, 2); f.requests[1].resolve('{"new config":3}'); await reload;
  assert.equal(f.editor.value, '{"new config":3}'); assert.equal(f.$('#codes-save').disabled, false);
});
test('Case change immediately labels and disables the prior catalog draft without transferring it', async () => {
  const f = fixture({ ownership: true }); await f.open('{"A":1}'); f.edit('{"A draft":2}'); f.switchCase('b'); f.context.renderCodesStatus();
  assert.equal(f.editor.value, '{"A draft":2}'); assert.equal(f.editor.disabled, true); assert.equal(f.$('#codes-save').disabled, true); assert.equal(f.$('#codes-reload').disabled, true); assert.equal(f.$('#btn-harvest').disabled, true);
  assert.match(f.$('#codes-status').textContent, /pertence ao Caso a/);
});
