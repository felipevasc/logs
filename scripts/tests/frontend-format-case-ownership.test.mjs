import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const source = app.slice(app.indexOf('let formatOptionsRequest'), app.indexOf('// ------------------------------------------------------------------ data/hora'));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function fixture() {
  const a = { id: 'a', revision: 1 }, b = { id: 'b', revision: 1 }, cases = [a, b], state = { cases: { active: 'a', cases } }, nodes = new Map(), requests = [], notices = [];
  class Node {
    constructor() { this.value = ''; this.hidden = false; this.children = []; this.dataset = {}; this.classList = { toggle() {} }; }
    set innerHTML(value) { this.html = value; this.children = []; }
    get innerHTML() { return this.html; }
    appendChild(child) { this.children.push(child); return child; }
    get options() { return this.children; }
    focus() {}
  }
  const $ = id => { if (!nodes.has(id)) nodes.set(id, new Node()); return nodes.get(id); };
  const helper = { capture: () => { const item = cases.find(value => value.id === state.cases.active); return { item, caseId: item.id, revision: item.revision }; },
    assertOwner: (owner, { revisions = true } = {}) => { if (cases.find(value => value.id === state.cases.active) !== owner.item || revisions && owner.revision !== owner.item.revision) throw Error('ANALYSIS_CONTEXT_CHANGED'); },
    prepare: async owner => { helper.assertOwner(owner); return owner; } };
  const context = vm.createContext({ state, $, window: { AnalysisContexts: helper }, document: { querySelector: () => ({ dataset: { kind: 'regex' } }), querySelectorAll: () => [] },
    el: () => new Node(), esc: String, toast: (...values) => notices.push(values),
    api: (command, args, options) => { const request = { command, args, options, ...deferred() }; requests.push(request); return request.promise; } });
  vm.runInContext(source, context);
  return { context, state, a, b, cases, requests, notices, $, switchCase: id => { state.cases.active = id; },
    draft: name => { $('#fmt-name').value = name; $('#fmt-pattern').value = '(.*)'; $('#fmt-fields').value = 'message'; $('#fmt-sample').value = 'example'; } };
}
test('formats are read for the captured Case; late A list cannot replace the B list', async () => {
  const f = fixture(), first = f.context.loadFormatOptions('custom:A'); await tick(); assert.equal(f.requests[0].options.analysisOwner.caseId, 'a');
  f.switchCase('b'); const second = f.context.loadFormatOptions('custom:B'); await tick();
  f.requests[1].resolve([{ id: 'auto', name: 'Auto' }, { id: 'custom:B', name: 'B' }]); assert.equal(await second, true);
  f.requests[0].resolve([{ id: 'auto', name: 'Auto' }, { id: 'custom:A', name: 'A' }]); assert.equal(await first, false);
  assert.deepEqual(f.$('#file-format').options.map(value => value.value), ['auto','custom:B']); assert.equal(f.$('#file-format').value, 'custom:B');
});
test('format save and preview cannot dispatch from an old Case editor', async () => {
  const f = fixture(); f.context.openFormatModal(); f.draft('A'); f.switchCase('b'); await f.context.saveNewFormat(); await f.context.testNewFormat(); assert.equal(f.requests.length, 0);
});
test('late save receipt from A cannot close or erase a reopened B format draft', async () => {
  const f = fixture(); f.context.openFormatModal(); f.draft('A'); const saving = f.context.saveNewFormat();
  assert.equal(f.requests[0].options.analysisOwner.caseId, 'a'); f.context.closeFormatModal(); f.switchCase('b'); f.context.openFormatModal(); f.draft('B');
  f.a.revision++; f.requests[0].resolve({ analysisContext: { caseId: 'a' } }); await saving;
  assert.equal(f.$('#format-modal').hidden, false); assert.equal(f.$('#fmt-name').value, 'B'); assert.equal(f.requests.length, 1); assert.deepEqual(f.notices, []);
});
test('format save captures its submitted version and retains newer same-Case typing', async () => {
  const f = fixture(); f.context.openFormatModal(); f.draft('A'); const saving = f.context.saveNewFormat();
  f.$('#fmt-pattern').value = '(newer)'; f.a.revision++; f.requests[0].resolve({}); await tick();
  assert.equal(f.requests[0].args.pattern, '(.*)'); assert.equal(f.requests[1].command, 'list_formats'); assert.equal(f.requests[1].options.analysisOwner.revision, 2);
  f.requests[1].resolve([{ id: 'auto' }, { id: 'custom:A' }]); await saving;
  assert.equal(f.$('#format-modal').hidden, false); assert.equal(f.$('#fmt-pattern').value, '(newer)'); assert.match(f.notices[0][0], /posterior/);
});
test('late parser preview is fenced by Case, editor reopen and draft signature', async () => {
  for (const change of ['case','reopen','draft']) {
    const f = fixture(); f.context.openFormatModal(); f.draft('A'); const preview = f.context.testNewFormat();
    if (change === 'case') f.switchCase('b');
    if (change === 'reopen') { f.context.closeFormatModal(); f.context.openFormatModal(); f.draft('new'); }
    if (change === 'draft') f.$('#fmt-pattern').value = '(newer)';
    f.requests[0].resolve([{ message: 'old A only' }]); await preview; assert.equal(f.$('#fmt-test-result').children.length, 0, change);
  }
});
test('failed format writes keep the owned draft editable for retry', async () => {
  const f = fixture(); f.context.openFormatModal(); f.draft('A'); const failed = f.context.saveNewFormat(); f.requests[0].reject(Error('write failed')); await failed;
  assert.equal(f.$('#fmt-name').value, 'A'); assert.equal(f.$('#format-save').disabled, false); assert.match(f.notices[0][0], /write failed/);
});
test('Case transition makes old format ownership visible and disables actions until an explicit new editor', () => {
  const f = fixture(); f.context.openFormatModal(); f.draft('A'); f.switchCase('b');
  const start = app.indexOf('function refreshCaseEditorOwnership()'), end = app.indexOf('// ------------------------------------------------------------------ configurações / MCP', start);
  Object.assign(f.context, { codesEditorSession: null, tsEditor: null }); vm.runInContext(app.slice(start, end), f.context); f.context.refreshCaseEditorOwnership();
  assert.equal(f.$('#fmt-name').value, 'A'); assert.equal(f.$('#format-save').disabled, true); assert.equal(f.$('#fmt-test').disabled, true); assert.match(f.$('#fmt-test-result').textContent, /pertence ao Caso a/);
  f.context.closeFormatModal(); f.context.openFormatModal(); assert.equal(f.$('#fmt-name').value, ''); assert.equal(f.$('#fmt-test').disabled, false);
});
