import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const base = new URL('../../', import.meta.url);
const source = name => readFileSync(new URL(`frontend/${name}`, base), 'utf8');
const app = source('app.js'), workspace = source('workspace.js'), contextSource = source('workspace-context.js');
const part = (text, start, end) => { const from = text.indexOf(start), to = text.indexOf(end, from); assert.ok(from >= 0 && to > from, start); return text.slice(from, to); };
const plain = value => JSON.parse(JSON.stringify(value));
function fixture({ stored = null, legacy = true } = {}) {
  const a = { id: 'a', workspace: {} }, b = { id: 'b', workspace: stored ? { contextStates: { dataset: stored } } : {} }, nodes = new Map(), writes = [], renders = [];
  const state = { cases: { active: 'b', cases: [a, b] }, favoriteFields: legacy ? ['A.private-field'] : [], dashboardCompact: legacy, treeCollapsed: new Set(), columns: [], datasetRevision: 1 };
  const storage = new Map([['workspace.fields', '["A.private-field"]'], ['workspace.density', 'compact'], ['investigation.dashboardCompact', '1']]);
  const localStorage = { getItem: key => storage.get(key) || null, setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) };
  const document = { body: { dataset: { page: 'summary', density: legacy ? 'compact' : 'comfortable', wrap: 'false' } }, querySelector: selector => selector === '.shell' ? { classList: { contains: () => false } } : null };
  const $ = key => { if (!nodes.has(key)) nodes.set(key, { value: '' }); return nodes.get(key); };
  const activeCase = () => state.cases.cases.find(item => item.id === state.cases.active) || null;
  let context;
  const ownership = { capture: () => ({ item: activeCase(), scope: vm.runInContext('scope', context), revision: state.datasetRevision }), isCurrent: owner => owner.item === activeCase() && owner.scope === vm.runInContext('scope', context) && owner.revision === state.datasetRevision };
  context = vm.createContext({ state, document, $, window: { AnalysisContexts: ownership }, localStorage, structuredClone, activeCase,
    defaultCaseWorkspace: () => ({}), AGG_FUNCS: [['count', 'Contagem']], cubeState: { collapsed: new Set() },
    workspaceScope: () => vm.runInContext('scope', context),
    saveCases: async () => { writes.push(plain(state.cases)); return true; },
    setScope: async next => { vm.runInContext(`scope = ${JSON.stringify(next)}`, context); const snapshot = context.stored(next); Object.assign(state, structuredClone(snapshot.values)); document.body.dataset.density = snapshot.density; },
    showUnavailable() {}, renderExploreTree: () => renders.push('fields'), renderDashboard: () => renders.push('dashboard') });
  vm.runInContext(part(contextSource, '  let scope =', '  function apply('), context);
  vm.runInContext(part(contextSource, '  async function initialize()', '  function sourceChanged('), context);
  vm.runInContext(part(contextSource, '  const originalSave =', '  updateToggle();'), context);
  const ownerCapture = part(app, '  const favorites = state.favoriteFields || [];', '  const createFieldRow =');
  const callback = part(app, '    favorite.onclick = () => {', '    row.append(favorite);');
  vm.runInContext(`function favoriteFor(column) { const scope = workspaceScope(); ${ownerCapture} const favorite = {}; ${callback} return favorite.onclick; }`, context);
  vm.runInContext(part(workspace, '  $("#ws-density").onclick =', '  // Legacy profile-wide'), context);
  vm.runInContext(part(app, '  $("#btn-dash-compact").onclick =', '  $("#cp-type").onchange'), context);
  return { context, state, a, b, storage, writes, renders, document, $, setScope: scope => vm.runInContext(`scope = ${JSON.stringify(scope)}`, context) };
}
test('initial or replacement Case without dataset context starts at defaults instead of adopting previous live/profile values', async () => {
  const f = fixture(); await f.context.initialize();
  assert.deepEqual(plain(f.b.workspace.contextStates.dataset.values.favoriteFields), []);
  assert.equal(f.b.workspace.contextStates.dataset.values.dashboardCompact, false); assert.equal(f.b.workspace.contextStates.dataset.density, 'comfortable');
  assert.deepEqual(plain(f.state.favoriteFields), []); assert.equal(f.state.dashboardCompact, false); assert.equal(f.document.body.dataset.density, 'comfortable');
});
test('reopening an existing Case preserves its saved per-scope visual choices', async () => {
  const f = fixture({ stored: { values: { favoriteFields: ['B.only'], dashboardCompact: true }, density: 'compact' } }); await f.context.initialize();
  assert.deepEqual(plain(f.state.favoriteFields), ['B.only']); assert.equal(f.state.dashboardCompact, true); assert.equal(f.document.body.dataset.density, 'compact');
});
test('each favorite/density/compact click captures and requests Case persistence without requiring navigation', async () => {
  for (const control of ['favorite', 'density', 'compact']) {
    const f = fixture({ legacy: false }); await f.context.initialize();
    if (control === 'favorite') f.context.favoriteFor('B.field')(); else f.$(control === 'density' ? '#ws-density' : '#btn-dash-compact').onclick();
    assert.equal(f.writes.length, 1, control);
    const saved = f.writes[0].cases.find(item => item.id === 'b').workspace.contextStates.dataset;
    if (control === 'favorite') assert.deepEqual(saved.values.favoriteFields, ['B.field']);
    if (control === 'density') assert.equal(saved.density, 'compact');
    if (control === 'compact') assert.equal(saved.values.dashboardCompact, true);
    const reopened = fixture({ stored: saved }); await reopened.context.initialize();
    assert.deepEqual(plain(reopened.state.favoriteFields), saved.values.favoriteFields); assert.equal(reopened.state.dashboardCompact, saved.values.dashboardCompact); assert.equal(reopened.document.body.dataset.density, saved.density);
  }
});
test('visual edits are saved under the current Case scope without replacing the other scope', async () => {
  const f = fixture({ legacy: false }); await f.context.initialize(); f.context.favoriteFor('dataset-only')(); const dataset = plain(f.b.workspace.contextStates.dataset);
  f.setScope('case'); f.state.favoriteFields = []; f.context.favoriteFor('case-only')(); f.$('#ws-density').onclick();
  assert.deepEqual(plain(f.b.workspace.contextStates.dataset), dataset); assert.deepEqual(plain(f.b.workspace.contextStates.case.values.favoriteFields), ['case-only']); assert.equal(f.b.workspace.contextStates.case.density, 'compact');
});
test('a favorite callback from A cannot mutate B, a different scope, a replacement Case instance, or a replaced source', async () => {
  for (const change of ['case', 'scope', 'instance', 'source']) {
    const f = fixture({ legacy: false }); await f.context.initialize(); const old = f.context.favoriteFor('B.old-field');
    if (change === 'case') f.state.cases.active = 'a';
    if (change === 'scope') f.setScope('case');
    if (change === 'instance') f.state.cases.cases[1] = { id: 'b', workspace: {} };
    if (change === 'source') f.state.datasetRevision++;
    f.state.favoriteFields = ['new-owned']; old();
    assert.deepEqual(plain(f.state.favoriteFields), ['new-owned'], change); assert.equal(f.writes.length, 0, change); assert.equal(f.renders.length, 0, change);
  }
});
test('legacy 0.11 global preference bytes remain intact and are neither read nor rewritten as new Case defaults', async () => {
  const f = fixture(), original = [...f.storage]; await f.context.initialize(); f.context.favoriteFor('B.field')(); f.$('#ws-density').onclick(); f.$('#btn-dash-compact').onclick();
  assert.deepEqual([...f.storage], original);
  for (const key of ['workspace.fields', 'workspace.density', 'investigation.dashboardCompact']) {
    assert.equal(app.includes(`localStorage.getItem("${key}")`), false); assert.equal(app.includes(`localStorage.setItem("${key}"`), false);
    assert.equal(workspace.includes(`localStorage.getItem("${key}")`), false); assert.equal(workspace.includes(`localStorage.setItem("${key}"`), false);
  }
});
