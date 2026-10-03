import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../../frontend/workspace.js', import.meta.url), 'utf8');
const start = source.indexOf('  async function showPage(');
const showPage = source.slice(start, source.indexOf('  function renderUnavailableCase(', start));

function fixture(scope = 'dataset') {
  const state = { loaded: false, rows: [], activeDatasetTab: 'table' }, calls = [];
  const context = vm.createContext({
    state, window: {}, navigation: 0, page: 'summary', lastExploredKey: null,
    home: { hidden: false }, empty: { hidden: false }, content: { hidden: true }, sourceList: [],
    STRUCTURE: new Set(['sources', 'connections', 'import']),
    activeCase: () => ({ id: 'fresh-case' }), workspaceScope: () => scope,
    markPage: value => { context.page = value; }, closeDrawer() {},
    switchView: value => calls.push(['view', value]), sourceKey: () => 'fresh-case-source', renderExploreTree() {},
    refresh: async () => { calls.push(['refresh', state.loaded]); return true; },
    switchTab: value => calls.push(['tab', value]), renderSummary: async () => calls.push(['summary']),
  });
  vm.runInContext(showPage, context);
  return { context, state, calls };
}

test('empty dataset Explore stays on Summary until its own source is explicitly loaded', async () => {
  const f = fixture();
  await f.context.showPage('explore');
  assert.equal(f.context.page, 'summary');
  assert.equal(f.context.home.hidden, false); assert.equal(f.context.empty.hidden, false);
  assert.deepEqual(f.calls, [['view', 'workspace']], 'no inherited rows, explorer or query is used');
  f.state.loaded = true;
  await f.context.showPage('explore');
  assert.equal(f.context.page, 'explore'); assert.equal(f.context.home.hidden, true);
  assert.deepEqual(f.calls.slice(1), [['view', 'viz'], ['refresh', true], ['tab', 'table']]);
});

test('empty Case evidence Explore remains available without an external source', async () => {
  const f = fixture('case');
  await f.context.showPage('explore');
  assert.equal(f.context.page, 'explore'); assert.equal(f.context.home.hidden, true);
  assert.deepEqual(f.calls, [['view', 'viz'], ['refresh', false], ['tab', 'table']]);
});
