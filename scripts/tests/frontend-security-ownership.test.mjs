import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const text = readFileSync(new URL('../../frontend/security.js', import.meta.url), 'utf8');
const section = text.slice(text.indexOf('  let rulesCache = null;'), text.indexOf('  // ---------------------------------------------------------------- hunting recipes'));
const deferred = () => { let resolve; const promise = new Promise(done => resolve = done); return { promise, resolve }; };
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
function fixture() {
  class Node {
    constructor(kind) { this.kind = kind; this.children = []; this.nodes = new Map(); this.isConnected = true; this.value = ''; this.dataset = {}; }
    querySelector(selector) { if (!this.nodes.has(selector)) this.nodes.set(selector, new Node(selector)); return this.nodes.get(selector); }
    querySelectorAll() { return []; }
    prepend(node) { this.children.unshift(node); }
    append(...nodes) { this.children.push(...nodes); }
    insertBefore(node) { this.children.unshift(node); }
    addEventListener(name, fn) { this['on' + name] = fn; }
  }
  const a = { caseId: 'a', revision: 1 }, b = { caseId: 'b', revision: 1 };
  let active = a; const requests = [], mutations = [], selections = [], nodes = [], listeners = new Map();
  const helper = {
    capture: () => ({ caseId: active.caseId, revision: active.revision }),
    assertOwner: (owner, { revisions = true } = {}) => { if (owner.caseId !== active.caseId || revisions && owner.revision !== active.revision) throw Error('STALE_CASE'); },
    prepare: async owner => { helper.assertOwner(owner); return owner; },
  };
  const context = vm.createContext({ window: { AnalysisContexts: helper }, document: { createElement: kind => new Node(kind), addEventListener(name, fn) { listeners.set(name, fn); } },
    api: (cmd, args, options) => {
      if (cmd === 'detection_rules') { const pending = deferred(); requests.push({ ...pending, options }); return pending.promise; }
      helper.assertOwner(options.analysisOwner); mutations.push({ cmd, args: structuredClone(args), owner: options.analysisOwner }); active.revision++;
      return Promise.resolve({ analysisContext: { caseId: active.caseId }, imported: 1, rules: 1, failed: [] });
    }, results: new Map(), invalidate() {}, names: new Map(), structuredClone, esc: String, fmtNum: String,
    el: kind => { const node = new Node(kind); nodes.push(node); return node; }, toast() {}, cached: () => null,
    evidence: () => ({ label: String }), dialogApi: { open: () => { const pending = deferred(); selections.push(pending); return pending.promise; } }, confirm: () => true,
  });
  vm.runInContext(section + '\nthis.readRules = rules; this.render = renderRulesPane;', context);
  const overview = name => ({ rules: [{ id: name, name, origin: 'none' }], settings: { disabled: [], suppress: [], threats: true, mappings: [], coverage: [] }, sigma_errors: [], custom_rules_json: null });
  const open = async () => { const pane = new Node('pane'), done = context.render(pane); await tick(); requests.at(-1).resolve(overview(active.caseId)); await done; return pane; };
  return { context, a, b, requests, mutations, selections, nodes, overview, open, switch: value => { active = value; }, notify: name => listeners.get(name)?.() };
}
test('late Case A rules never enter the active Case B cache', async () => {
  const f = fixture(), a = f.context.readRules(); await tick(); f.switch(f.b);
  const b = f.context.readRules(); await tick(); f.requests[1].resolve(f.overview('b')); assert.equal((await b).rules[0].id, 'b');
  f.requests[0].resolve(f.overview('a')); await assert.rejects(a, /STALE_CASE/);
  assert.equal((await f.context.readRules()).rules[0].id, 'b'); assert.equal(f.requests.length, 2);
});
test('an old security editor cannot save settings into another Case', async () => {
  const f = fixture(), pane = await f.open(); f.switch(f.b);
  await assert.rejects(pane.querySelector('#rules-threats').onchange({ target: { checked: false } }), /STALE_CASE/);
  assert.equal(f.mutations.length, 0);
});
test('subsequent security edits use the new receipt revision', async () => {
  const f = fixture(), pane = await f.open(), toggle = pane.querySelector('#rules-threats');
  await toggle.onchange({ target: { checked: false } }); await toggle.onchange({ target: { checked: true } });
  assert.deepEqual(f.mutations.map(value => value.owner.revision), [1, 2]);
  assert.deepEqual(f.mutations.map(value => value.args.settings.threats), [false, true]);
});
test('switching Case during a Sigma picker cannot import into either Case', async () => {
  const f = fixture(), pane = await f.open(), pending = pane.querySelector('#rules-import').onclick(); await tick(); f.switch(f.b);
  f.selections[0].resolve(['/selected.yml']); await assert.rejects(pending, /STALE_CASE/); assert.equal(f.mutations.length, 0);
});

test('an open security pane labels its owner and marks itself stale on Case change', async () => {
  const f = fixture(), pane = await f.open();
  assert.match(pane.children[0].innerHTML, /Segurança do Caso a/);
  f.switch(f.b); f.notify('workspace-context-change');
  assert.match(pane.children[0].innerHTML, /formulário anterior está bloqueado/);
  assert.match(pane.children[0].innerHTML, /Segurança do Caso a/);
});
