import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const contextSource = readFileSync(new URL('../../frontend/workspace-context.js', import.meta.url), 'utf8');
const section = (start, end) => contextSource.slice(contextSource.indexOf(start), contextSource.indexOf(end, contextSource.indexOf(start)));
for (const scope of ['case', 'dataset']) for (const hasSource of [true, false]) {
  const c = { id: 'new', workspace: {} }, runtime = new Map(), states = new Map(), switches = [];
  const native = { loaded: true, columns: ['timestamp', 'native_only'], rows: [{ id: 100 }], total: 50_000_000, dataPeriod: { min: 1, max: 2 } };
  const context = vm.createContext({
    window: {}, state: { loaded: true, columns: scope === 'case' ? ['timestamp', 'saved_only'] : native.columns, total: 3, dataPeriod: null }, runtime, states,
    activeCase: () => c, key: target => `new:${target}`, restoringCase: true, caseGeneration: 1,
    resetCaseSourceState: () => Object.assign(context.state, { loaded: false, columns: [], rows: [], dataPeriod: null }), syncActiveCaseArtifacts: async () => {},
    setScope: async (target, options) => { switches.push({ target, options }); }, refresh: async () => {},
  });
  vm.runInContext(section('  function defaults()', '  const record ='), context);
  vm.runInContext(section('  function sourceRuntime(', '  function stored('), context);
  vm.runInContext(section('  async function afterCaseCreation(', '  async function deleteCase('), context);
  await context.afterCaseCreation({ scope, runtime: hasSource ? native : undefined, snapshot: { page: 'explore', values: { filters: [{ column: 'source' }] } }, activeSnapshot: { page: 'explore' } });
  assert.equal(switches[0].target, scope, 'creating a case preserves the active area');
  assert.equal(c.workspace.activeScope, scope);
  assert.deepEqual(Array.from(runtime.get('new:dataset').columns), [], 'a new Case never inherits the prior source columns');
  assert.equal(runtime.get('new:dataset').loaded, false, 'a new Case starts without any source, including creation from the dataset');
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
console.log('Case creation preserves scope and starts with an empty source; decorated table preserves row reuse');

// The actual constructor never clones source descriptors, even when a prior snapshot contains them.
const appSource = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const appSection = (start, end) => appSource.slice(appSource.indexOf(start), appSource.indexOf(end, appSource.indexOf(start)));
{
  const prior = { artifacts: [{ id: 'A-file', path: '/only-A.jsonl' }], activeArtifactId: 'A-file' }, handed = [];
  const context = vm.createContext({ state: { cases: { active: 'a', cases: [{ id: 'a', artifacts: prior.artifacts }] } },
    window: { WorkspaceContext: { beforeCaseCreation: () => prior, afterCaseCreation: value => handed.push(value) }, AnalysisContexts: { activate() {} } },
    CURRENT_FILTER_ID: '__current__', defaultCaseWorkspace: () => ({}), setAnalysisView() {}, saveCases() {}, renderCaseBar() {}, updateAnalysisBadge() {}, renderAnalysis() {}, toast() {},
  });
  vm.runInContext(appSection('function newCase(', 'function caseItems('), context);
  const created = context.newCase('B'); assert.equal(created.artifacts.length, 0); assert.equal(created.activeArtifactId, null);
  assert.equal(context.state.cases.cases[0].artifacts[0].path, '/only-A.jsonl'); assert.equal(handed[0], prior);
}

const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };
{
  const a = { id: 'a', workspace: { saved: 'A' } }, b = { id: 'b', workspace: { saved: 'B' } }, c = { id: 'c', workspace: { saved: 'C' } }, gate = deferred();
  const captures = [], sources = [], switches = [], state = { cases: { active: 'a', cases: [a, b, c] }, loaded: true };
  const context = vm.createContext({ state, window: { AnalysisContexts: { activate() {} } }, activeCase: () => state.cases.cases.find(item => item.id === state.cases.active),
    sourceBusy: 0, restoringCase: false, caseGeneration: 0, scope: 'dataset', caseReturnScope: 'dataset',
    capture: () => captures.push(state.cases.active), resetCaseSourceState: () => { state.loaded = false; }, renderCaseBar() {}, updateAnalysisBadge() {},
    loadDerivedFields: () => state.cases.active === 'b' ? gate.promise : Promise.resolve(true), syncActiveCaseArtifacts: async () => sources.push(state.cases.active),
    runtime: new Map(), key: value => `${state.cases.active}:${value}`, sourceRuntime: () => ({ loaded: false, columns: [] }),
    setScope: async next => switches.push([state.cases.active, next]), refresh: async () => {},
  });
  vm.runInContext(section('  async function changeCase(', '  function beforeCaseCreation('), context);
  const openingB = context.changeCase('b'); const openingC = context.changeCase('c'); await openingC; gate.resolve(true); await openingB;
  assert.deepEqual(captures, ['a'], 'an unfinished B transition must not overwrite its saved workspace with temporary empty state');
  assert.deepEqual(sources, ['c']); assert.deepEqual(switches, [['c', 'dataset']]); assert.equal(state.cases.active, 'c'); assert.equal(b.workspace.saved, 'B');
}
{
  const created = { id: 'created', workspace: {} }, next = { id: 'next', workspace: {} }, gate = deferred(); let active = created;
  const calls = [], state = { loaded: true, columns: ['A_field'], rows: [{ id: 1 }] };
  const context = vm.createContext({ state, window: { AnalysisContexts: {} }, activeCase: () => active, caseGeneration: 1, restoringCase: true,
    runtime: new Map(), states: new Map(), key: value => `${active.id}:${value}`,
    resetCaseSourceState: () => Object.assign(state, { loaded: false, columns: [], rows: [], dataPeriod: null }),
    loadDerivedFields: () => gate.promise, syncActiveCaseArtifacts: async () => calls.push('source'), setScope: async () => calls.push('scope'), refresh: async () => calls.push('refresh'),
  });
  vm.runInContext(section('  function defaults()', '  const record ='), context);
  vm.runInContext(section('  function sourceRuntime(', '  function stored('), context);
  vm.runInContext(section('  async function afterCaseCreation(', '  async function deleteCase('), context);
  const pending = context.afterCaseCreation({ scope: 'dataset' }); active = next; context.caseGeneration++; gate.resolve(true); await pending;
  assert.deepEqual(calls, [], 'obsolete Case creation cannot clear or query the newly selected Case');
  assert.equal(context.restoringCase, true, 'old finalizer cannot finish a newer transition');
}
console.log('New Case constructor, interrupted creation and rapid A→B→C workspace ownership passed');

// A native file picker may settle after a Case transition or a newer source intent.
for (const replacement of ['case', 'intent', 'same-id']) {
  const original = { id: 'a' }, gate = deferred(), input = { value: 'B draft' }; let active = original;
  const context = vm.createContext({ state: { artifactSwitchVersion: 1 }, activeCase: () => active, dialogApi: { open: () => gate.promise }, $: () => input });
  vm.runInContext(appSection('async function browseFile()', 'async function refreshChannels()'), context);
  const choosing = context.browseFile();
  if (replacement === 'intent') context.state.artifactSwitchVersion++; else active = { id: replacement === 'same-id' ? 'a' : 'b' };
  gate.resolve('/only-A.jsonl'); await choosing; assert.equal(input.value, 'B draft');
}
// Format discovery during restoration must not supersede a newer explicit source intent.
{
  const c = { id: 'a' }, gate = deferred(), calls = [], state = { artifactSwitchVersion: 4 };
  const context = vm.createContext({ state, activeCase: () => c, loadFormatOptions: () => gate.promise,
    artifactSessionFor: () => ({ activeId: 'old-source', artifacts: new Map([['old-source', {}]]) }), renderArtifactBar() {}, updateContextBar() {},
    activateArtifact: async id => calls.push(id), clearData: async () => calls.push('clear'),
  });
  vm.runInContext(appSection('async function syncActiveCaseArtifacts()', '\nfunction updateContextBar()'), context);
  const restoring = context.syncActiveCaseArtifacts(); state.artifactSwitchVersion++; gate.resolve(true); await restoring;
  assert.deepEqual(calls, []);
}
