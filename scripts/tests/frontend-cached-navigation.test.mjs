import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/workspace.js', import.meta.url), 'utf8');
let queried = 0, renderedFields = 0, openedTab = 0, restoredSummaries = 0;
const shell = { hidden: true };
const filters = [{ column: 'source', op: 'equals_exact', value: 'Auth' }];
const cachedSummary = { status: 'done', total: 17, stats: { buckets: [[1000, 17]], bucketMs: 1000 } };
const context = vm.createContext({
  state: { loaded: true, rows: [{ id: 1 }], activeDatasetTab: 'table', queryError: null },
  navigation: 0, window: {}, workspaceScope: () => 'dataset', activeCase: () => null,
  STRUCTURE: new Set(['sources', 'connections', 'import']), sourceKey: () => 'loaded-source', lastExploredKey: 'loaded-source',
  home: { hidden: false }, markPage() {}, closeDrawer() {},
  switchView: () => { shell.hidden = false; }, switchTab: () => { openedTab++; },
  renderExploreTree: () => { assert.equal(shell.hidden, false, 'field rendering runs only after the workspace is visible'); renderedFields++; },
  explorerKey: () => 'matching-analysis-and-filter', backendFilters: () => filters,
  loadExplorerAnalytics: (key, scope, capturedFilters) => {
    assert.equal(key, 'matching-analysis-and-filter'); assert.equal(scope, 'dataset'); assert.equal(capturedFilters, filters);
    return cachedSummary;
  },
  showExplorerAnalytics: summary => { assert.equal(summary, cachedSummary, 'the matching cached summary is restored with its records'); restoredSummaries++; },
  refresh: async () => { queried++; return true; },
});
vm.runInContext(source.slice(source.indexOf('  async function showPage(next)'), source.indexOf('  async function getOverview(')), context);
await context.showPage('explore');
assert.equal(renderedFields, 1, 'cached navigation must populate fields skipped while Summary hid the workspace');
assert.equal(openedTab, 1);
assert.equal(queried, 0, 'opening cached records does not repeat the query');
assert.equal(restoredSummaries, 1);
console.log('Cached Explore navigation restores its matching summary and field catalog without querying again');
