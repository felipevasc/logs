import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const contextSource = readFileSync(new URL('../../frontend/workspace-context.js', import.meta.url), 'utf8');
const section = (start, end) => contextSource.slice(contextSource.indexOf(start), contextSource.indexOf(end, contextSource.indexOf(start)));
for (const scope of ['case', 'dataset']) for (const hasSource of [true, false]) {
  const c = { id: 'new', workspace: {} }, runtime = new Map(), states = new Map(), switches = [];
  const native = { loaded: true, columns: ['timestamp', 'native_only'], rows: [{ id: 100 }], total: 50_000_000, dataPeriod: { min: 1, max: 2 } };
  const context = vm.createContext({
    state: { loaded: true, columns: scope === 'case' ? ['timestamp', 'saved_only'] : native.columns, total: 3, dataPeriod: null }, runtime, states,
    activeCase: () => c, key: target => `new:${target}`, restoringCase: true,
    setScope: async (target, options) => { switches.push({ target, options }); }, refresh: async () => {},
  });
  vm.runInContext(section('  function defaults()', '  const record ='), context);
  vm.runInContext(section('  function sourceRuntime(', '  function stored('), context);
  vm.runInContext(section('  async function afterCaseCreation(', '  async function deleteCase('), context);
  await context.afterCaseCreation({ scope, runtime: hasSource ? native : undefined, snapshot: { page: 'explore', values: { filters: [{ column: 'source' }] } }, activeSnapshot: { page: 'explore' } });
  assert.equal(switches[0].target, scope, 'creating a case preserves the active area');
  assert.equal(c.workspace.activeScope, scope);
  const sourceAvailable = hasSource || scope === 'dataset';
  assert.deepEqual(Array.from(runtime.get('new:dataset').columns), sourceAvailable ? native.columns : [], 'native source columns are never replaced by saved-evidence columns');
  assert.equal(runtime.get('new:dataset').loaded, sourceAvailable, 'Case evidence cannot imply an open native source when its runtime is missing');
  assert.equal(runtime.get('new:dataset').rows.length, 0, 'filtered rows are not carried across cases');
  assert.equal(states.get('new:dataset').values.filters.length, 0, 'new analysis starts with a clean filter');
  assert.equal(states.get('new:case').values.filters.length, 0, 'new case starts with a clean filter');
  assert.equal(states.get('new:case').page, scope === 'case' ? 'explore' : 'summary');
  assert.equal(context.restoringCase, false);
}

// Exercise the installed table decorator, not just the core renderTable implementation.
const discoverySource = readFileSync(new URL('../../frontend/discovery.js', import.meta.url), 'utf8');
const renderCalls = [];
const tableContext = vm.createContext({ renderTable: (...args) => renderCalls.push(args), document: { querySelectorAll: () => [] } });
vm.runInContext(discoverySource.slice(discoverySource.indexOf('  const oldTable=renderTable;'), discoverySource.indexOf('  const oldDetail=showDetail;')), tableContext);
const page = { rows: [{ id: 1 }] }, options = { reuseRows: true };
tableContext.renderTable(page, options);
assert.equal(renderCalls[0][0], page);
assert.equal(renderCalls[0][1], options, 'Discovery forwards local row-reuse options to the real table renderer');
console.log('Case creation preserves scope/source isolation; decorated table preserves row reuse');
