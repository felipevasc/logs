import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const taskSource = readFileSync(new URL('../../frontend/tasks.js', import.meta.url), 'utf8');
const core = readFileSync(new URL('../../frontend/performance-core.js', import.meta.url), 'utf8');
const workspace = readFileSync(new URL('../../frontend/workspace.js', import.meta.url), 'utf8');
const section = (start, end) => app.slice(app.indexOf(start), app.indexOf(end, app.indexOf(start)));
const settle = async () => { for (let i = 0; i < 50; i++) await Promise.resolve(); };
const file = path => ({ kind: 'file', path });

function fixture() {
  const state = { cases: { active: 'case', cases: [{ id: 'case', artifacts: [] }] }, artifactSwitchVersion: 0, refreshVersion: 0, datasetRevision: 0, loaded: false, columns: [], visibleCols: [], rows: [], filters: [], quick: '', total: null, treeAgg: {}, treeAggSig: {}, treeAggError: {}, activeDatasetTab: 'table' };
  const pending = [], calls = [], messages = [], refreshReads = [], refreshTotals = [], nodes = new Map(), cancelled = new Set();
  let physicalCount = 1, visibleCount = null;
  let createdNodes = 0;
  let native = null, nativeOwner = null, generation = 0, nativeInputs = [], publicationId = null, snapshotFailure = false, prepare = async args => args, sourceWait = async () => {};
  const node = key => {
    if (!nodes.has(key)) nodes.set(key, { value: '', hidden: false, children: [], classList: { add() {}, toggle() {} }, appendChild() {}, before() {}, append(...children) { this.children.push(...children); }, replaceChildren(...children) { this.children = children; }, setAttribute() {}, querySelector() { return this; } });
    return nodes.get(key);
  };
  const context = vm.createContext({
    state, window: { WorkspaceContext: { waitForSource: () => sourceWait(), scope: () => 'dataset' } },
    document: { body: { dataset: { page: 'explore' } }, documentElement: { dataset: { zone: 'analysis' } }, createTextNode: String }, performance,
    $: node, el: () => node(`new-${++createdNodes}`), fmtNum: String, esc: String, toast: message => messages.push(String(message)),
    setTimeout() {}, clearTimeout() {}, setInterval() {}, clearInterval() {}, requestAnimationFrame() {},
    explorerAnalytics: new Map(), treeAggVersion: { dataset: 0 }, cubeState: { requestVersion: 0 }, workspaceScope: () => 'dataset', caseArgs: (...args) => prepare(...args),
    activeCase: () => state.cases.cases.find(c => c.id === state.cases.active), ensureCase: () => state.cases.cases[0], artifactIdFromSource: value => value.path || value.channel,
    applySourceSpec() {}, setSource() {}, startOperation() {}, updateOperation() {}, skeletonRows() {},
    showLoadOverlay: () => { state.loadOverlay = true; }, hideLoadOverlay: () => { state.loadOverlay = false; },
    finishOperation: label => messages.push(label), fillColumnControls() {}, renderChips() {}, syncCurrentSavedFilter() {}, restoreCurrentSavedFilter() {},
    restoreVisiblePreferences: () => false, storeCurrentArtifactInSession: () => Promise.resolve(true), currentCaseArtifact: () => null,
    loadTsConfig: async () => {}, loadFormatOptions: async () => true, caseStations: () => [], autoVisibleCols() {}, updateTsExample() {}, updateContextBar() {}, renderExploreTree() {},
    renderTable() {}, renderChart() {}, updateAnalysisBadge() {}, closeDrawer() {}, renderArtifactBar() {},
    refresh: async () => { refreshReads.push(native); refreshTotals.push(state.total); state.rows = [{ source: native }]; state.total = visibleCount; },
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === 'profile_fields') return [];
      if (cmd === 'source_snapshot') {
        if (snapshotFailure) throw Error('Receipt temporarily unavailable');
        return { generation, operationId:publicationId, analysisContext:nativeOwner, count:native?physicalCount:0, columns:native?['timestamp','message']:[], sourceDesc:native||'', sourceNames:native?[native]:[], sources:nativeInputs };
      }
      if (cmd === 'cancel_task') {
        const target = pending.find(item => item.args.operationId === args.operationId);
        if (!target || target.committed) return false;
        cancelled.add(args.operationId); return true;
      }
      return new Promise((resolve, reject) => {
        const record = {
          cmd, args, committed: false, owner: { caseId: state.cases.active, analysisId: `analysis-${state.cases.active}` },
          publish() {
            if (cancelled.has(args.operationId)) return false;
            if (['load_file', 'load_files', 'load_bundle', 'load_event_log', 'clear_events'].includes(cmd)) {
              native = cmd === 'clear_events' ? null : args.path || args.paths?.join(';') || args.members?.map(item => item.path).join(';') || args.channel;
              generation++; publicationId=args.operationId; nativeOwner=record.owner;
              nativeInputs = cmd === 'clear_events' ? [] : cmd === 'load_bundle' ? args.members : cmd === 'load_event_log' ? [{kind:'eventlog',channel:args.channel,maxEvents:args.maxEvents}] : [{kind:'file',paths:args.paths||[args.path],format:args.format||'auto'}];
            }
            record.committed = true; return true;
          },
          respond() {
            if (!record.committed && cancelled.has(args.operationId)) { reject(Error('Operação cancelada.')); return; }
            resolve({ count: physicalCount, columns: ['timestamp', 'message'], source_desc: native, publication:{generation,operationId:publicationId,analysisContext:nativeOwner} });
          },
          complete() { record.publish(); record.respond(); }, reject,
        };
        pending.push(record);
      });
    },
  });
  vm.runInContext(core, context);
  vm.runInContext(section('function setQuickSearchDraft(', 'function commitQuickSearch('), context);
  vm.runInContext(section('async function api(', '// ------------------------------------------------------------------ helpers de espera'), context);
  vm.runInContext(taskSource, context);
  vm.runInContext(section('function clearSourceRecovery()', '// ------------------------------------------------------------------ filtros / chips'), context);
  return { context, state, pending, calls, messages, refreshReads, refreshTotals, node, get native() { return native; }, counts: (physical, visible) => { physicalCount = physical; visibleCount = visible; }, prepare: fn => { prepare = fn; }, sourceWait: fn => { sourceWait = fn; }, snapshotFailure: value => { snapshotFailure = value; } };
}

// Import/publication counts describe physical records; exclusions change visible totals.
const visibility = fixture(); visibility.counts(50, 7);
visibility.state.total = 999; visibility.state.pageResult = { total: 999 };
const visibleLoad = visibility.context.loadData(file('masked-source')); await settle();
visibility.pending[0].complete(); await visibleLoad;
assert.deepEqual(visibility.refreshTotals, [null], 'a committed load clears the previous visible total before querying');
assert.equal(visibility.state.total, 7); assert.equal(visibility.state.currentArtifact.count, 50);
assert.match(visibility.node('#load-status').textContent, /50 registros importados/);
visibility.state.total = 999; visibility.state.pageResult = { total: 999 };
await visibility.context.reconcilePublishedSource();
assert.deepEqual(visibility.refreshTotals, [null, null], 'source reconciliation never seeds visible totals with physical counts');
assert.equal(visibility.state.total, 7); assert.equal(visibility.state.currentArtifact.count, 50);

// Slow A may complete after B; native cancellation prevents its late publication.
const reverse = fixture();
const a = reverse.context.loadData(file('A')); await settle();
const b = reverse.context.loadData(file('B')); await settle();
reverse.pending[1].complete(); assert.equal(await b, true);
reverse.pending[0].complete(); assert.equal(await a, false);
assert.equal(reverse.native, 'B'); assert.equal(reverse.state.currentArtifact.path, 'B');
assert.equal(reverse.messages.filter(message => message === 'Artefato pronto').length, 1);

// Clear invalidates an older load immediately, and a newer load invalidates an older Clear.
for (const firstKind of ['load', 'clear']) {
  const f = fixture();
  const first = firstKind === 'load' ? f.context.loadData(file('A')) : f.context.clearData(); await settle();
  const second = firstKind === 'load' ? f.context.clearData() : f.context.loadData(file('B')); await settle();
  f.pending[1].complete(); assert.equal(await second, true);
  f.pending[0].complete(); assert.equal(await first, false);
  assert.equal(f.native, firstKind === 'load' ? null : 'B');
  assert.equal(f.state.loaded, firstKind !== 'load');
  assert.equal(f.state.currentArtifact?.path || null, firstKind === 'load' ? null : 'B');
}

// An old error/finalizer must not hide the overlay or enable controls for B.
const waiting = fixture();
const old = waiting.context.loadData(file('A')); await settle();
const next = waiting.context.loadData(file('B')); await settle();
waiting.pending[0].complete(); assert.equal(await old, false);
assert.equal(waiting.state.loadOverlay, true); assert.equal(waiting.node('#btn-load').disabled, true);
assert.ok(!waiting.messages.some(message => message.includes('Falha')));
waiting.pending[1].complete(); assert.equal(await next, true);

// User intent is claimed before awaiting source reconciliation.
const blocked = fixture(); let releaseSource;
blocked.sourceWait(() => new Promise(resolve => { releaseSource = resolve; }));
const blockedA = blocked.context.loadData(file('A')); await settle();
blocked.sourceWait(async () => {});
const blockedB = blocked.context.loadData(file('B')); await settle();
releaseSource(); await settle(); assert.equal(await blockedA, false);
assert.equal(blocked.pending.length, 1); blocked.pending[0].complete(); await blockedB;
assert.equal(blocked.native, 'B');

// Reusing a Case ID after delete/import does not revive an already captured source intent.
const replacedCase = fixture(); let releaseCase;
replacedCase.sourceWait(() => new Promise(resolve => { releaseCase = resolve; }));
const oldCaseLoad = replacedCase.context.loadData(file('old-case')); await settle();
replacedCase.state.cases.cases = [{ id: 'case', artifacts: [] }];
releaseCase(); assert.equal(await oldCaseLoad, false); assert.equal(replacedCase.pending.length, 0);

const emptyCase = fixture();
const abandoned = emptyCase.context.loadData(file('A')); await settle();
emptyCase.state.cases.active = 'empty'; emptyCase.state.cases.cases.push({ id: 'empty', artifacts: [] });
emptyCase.context.artifactSessionFor = () => ({ activeId: null, artifacts: new Map() });
vm.runInContext(section('async function syncActiveCaseArtifacts()', '\nfunction updateContextBar()'), emptyCase.context);
const switchToEmpty = emptyCase.context.syncActiveCaseArtifacts(); await settle();
assert.equal(emptyCase.pending[1].cmd, 'clear_events', 'an empty Case also clears an unpublished source load');
emptyCase.pending[1].complete(); await switchToEmpty;
emptyCase.pending[0].complete(); assert.equal(await abandoned, false);
assert.equal(emptyCase.native, null); assert.equal(emptyCase.state.loaded, false); assert.equal(emptyCase.state.loadOverlay, false);

// Cancellation during async case preparation must never dispatch the obsolete load.
const preparing = fixture(); let releaseArgs;
preparing.prepare(args => args.path === 'A' ? new Promise(resolve => { releaseArgs = () => resolve(args); }) : Promise.resolve(args));
const preparingA = preparing.context.loadData(file('A')); await settle();
const preparingB = preparing.context.loadData(file('B')); await settle();
releaseArgs(); await settle(); assert.equal(await preparingA, false);
assert.deepEqual(preparing.pending.map(item => item.args.path), ['B']);
preparing.pending[0].complete(); await preparingB;

const retrying = fixture(); let releaseRetry;
retrying.prepare((args, retry) => retry ? new Promise(resolve => { releaseRetry = () => resolve(args); }) : Promise.resolve(args));
const retryRead = retrying.context.api('query_page', {}, { latest: 'records', silent: true }).catch(String); await settle();
retrying.pending[0].reject(Error('CASE_CACHE_MISS')); await settle();
retrying.context.window.Tasks.cancelLatest('records'); await settle();
releaseRetry(); assert.match(await retryRead, /cancelada/);
assert.equal(retrying.calls.filter(call => call.cmd === 'query_page').length, 1, 'cancelled case-cache retries do not dispatch');

// Success after the native commit boundary is authoritative, even after a late Cancel.
for (const kind of ['load', 'clear']) {
  const committed = fixture();
  const result = kind === 'load' ? committed.context.loadData(file('A')) : committed.context.clearData(); await settle();
  committed.pending[0].publish();
  committed.context.window.Tasks.cancelLatest('source-load'); await settle();
  committed.pending[0].respond(); assert.equal(await result, true);
  assert.equal(committed.state.loaded, kind === 'load');
  assert.equal(committed.state.sourcePublication.generation, 1, 'load and clear both advance the admitted source generation');
  assert.equal(committed.state.currentArtifact?.path || null, kind === 'load' ? 'A' : null);
}

const independent = fixture();
const reading = independent.context.api('count_filtered', {}, { latest: 'independent-count' }); await settle();
const firstLoad = independent.context.loadData(file('A')); await settle();
const lastLoad = independent.context.loadData(file('B')); await settle();
assert.ok(!independent.calls.some(call => call.cmd === 'cancel_task' && call.args.operationId === independent.pending[0].args.operationId), 'source replacement preserves unrelated reads');
independent.pending[2].complete(); await lastLoad;
independent.pending[1].complete(); await firstLoad;
independent.pending[0].complete(); await reading;

const all = fixture();
const allLoad = all.context.loadData(file('A')); await settle();
const allRead = all.context.api('query_page', {}, { latest: 'records', silent: true }).catch(String); await settle();
all.context.window.Tasks.cancelAll(); await settle();
assert.equal(all.calls.filter(call => call.cmd === 'cancel_task').length, 2, 'Cancel All addresses each native ID even before async registration');
all.pending[0].complete(); all.pending[1].complete(); await allLoad; assert.match(await allRead, /cancelada/);

// All source command shapes share the same latest intent and an isolated native ID.
for (const [source, command] of [[file('one'), 'load_file'], [{ kind: 'file', path: 'one', paths: ['one', 'two'] }, 'load_files'], [{ kind: 'bundle', path: 'one', members: [file('one'), file('two')] }, 'load_bundle'], [{ kind: 'eventlog', channel: 'System' }, 'load_event_log']]) {
  const f = fixture(), result = f.context.loadData(source); await settle();
  assert.equal(f.pending[0].cmd, command);
  assert.equal(f.context.window.Tasks.operationFor('source-load'), f.pending[0].args.operationId);
  assert.ok(f.pending[0].args.operationId); f.pending[0].complete(); assert.equal(await result, true);
}

// Removing the final source must clear the native engine before view cleanup.
for (const superseded of [false, true]) {
  const f = fixture(); let navigated = 0;
  Object.assign(f.context, { sourceList: ['A'], sessionStorage: { removeItem() {} }, showPage: async () => { navigated++; } });
  vm.runInContext(workspace.slice(workspace.indexOf('  async function clearAnalysis()'), workspace.indexOf('  function escapeRegex(')), f.context);
  const clearing = f.context.clearAnalysis(); await settle(); assert.equal(f.pending[0].cmd, 'clear_events');
  let newest;
  if (superseded) { newest = f.context.loadData(file('B')); await settle(); f.pending[1].complete(); await newest; }
  f.pending[0].complete(); await clearing;
  assert.equal(f.native, superseded ? 'B' : null); assert.equal(navigated, superseded ? 0 : 1);
}

assert.ok(!reverse.calls.some(call => call.cmd === 'cancel_operation'), 'latest source replacement never cancels unrelated queries');

// A committed receipt recovers the actual active source without reimporting.
const uncertain = fixture();
uncertain.state.cases.cases[0].items = [{ id: 'evidence', rows: [{ id: 7 }] }];
uncertain.state.cases.cases[0].workspace = { preference: 'preserve' };
const evidenceBefore = JSON.stringify(uncertain.state.cases);
const c = uncertain.context.loadData(file('C')); await settle(); uncertain.pending[0].complete(); await c;
const committedA = uncertain.context.loadData(file('A')); await settle(); uncertain.pending[1].publish();
const failedB = uncertain.context.loadData(file('B')); await settle(); uncertain.pending[2].reject(Error('B unreadable')); assert.equal(await failedB, false);
uncertain.pending[1].respond(); assert.equal(await committedA, false);
assert.equal(uncertain.native, 'A');
assert.equal(uncertain.state.sourceIdentityUnconfirmed, false);
assert.equal(uncertain.state.loaded, true); assert.equal(uncertain.state.currentArtifact.path, 'A');
assert.deepEqual(uncertain.refreshReads, ['C','A'], 'receipt identifies A before any query is made');
assert.equal(JSON.stringify(uncertain.state.cases), evidenceBefore, 'Case evidence and preferences are preserved');
assert.equal(uncertain.pending.filter(p => p.cmd === 'load_file').length, 3, 'reconciliation does not reingest valid sources');

// Even a failed receipt read keeps the old file descriptors and visible rows.
const disconnected = fixture();
const previousKnown = disconnected.context.loadData(file('C')); await settle(); disconnected.pending[0].complete(); await previousKnown;
const rowsBefore = disconnected.state.rows, artifactBefore = disconnected.state.currentArtifact;
disconnected.snapshotFailure(true);
const missing = disconnected.context.loadData(file('B')); await settle(); disconnected.pending[1].reject(Error('IPC response lost')); await missing;
assert.equal(disconnected.state.sourceIdentityUnconfirmed, true);
assert.equal(disconnected.state.currentArtifact, artifactBefore); assert.equal(disconnected.state.rows, rowsBefore);
assert.equal(disconnected.state.loaded, false, 'unconfirmed old view cannot issue new queries');
assert.equal(disconnected.node('#source-recovery').hidden, false);
disconnected.snapshotFailure(false);
await disconnected.node('#source-recovery').children[1].onclick();
assert.equal(disconnected.state.loaded, true); assert.equal(disconnected.state.currentArtifact.path, 'C');
assert.equal(disconnected.state.sourceIdentityUnconfirmed, false);

const ordinary = fixture();
const previous = ordinary.context.loadData(file('C')); await settle(); ordinary.pending[0].complete(); await previous;
const failure = ordinary.context.loadData(file('B')); await settle(); ordinary.pending[1].reject(Error('B unreadable')); await failure;
assert.equal(ordinary.state.sourceIdentityUnconfirmed, false, 'ordinary failures retain a confirmed prior source');
assert.equal(ordinary.state.currentArtifact.path, 'C'); assert.equal(ordinary.native, 'C');
console.log('Latest source intents, clear ordering, deferred preparation, stale finalizers and committed cancellation passed');

// Source-list discovery errors and late answers never erase newer known files.
let visibilityRefreshes = 0;
const listContext = vm.createContext({ window:{ExclusionVisibility:{refresh(){visibilityRefreshes++;}}}, state:{filters:[],quick:''}, structuredClone, history:[], sourceList:[{path:'C'}], cacheKey:'', overview:null, previousSelection:null, lastFilters:'', key:'C', sourceKey:()=>listContext.key, updateCounts(){}, api:async()=>{throw Error('temporary list failure');} });
vm.runInContext(workspace.slice(workspace.indexOf('  async function loaded()'),workspace.indexOf('  async function saveFinding(')),listContext);
await listContext.loaded();assert.equal(listContext.sourceList[0].path,'C');
let releaseList;listContext.api=()=>new Promise(resolve=>{releaseList=resolve;});
const slowList=listContext.loaded();listContext.key='D';listContext.sourceList=[{path:'D'}];releaseList([{path:'C'}]);await slowList;
assert.equal(listContext.sourceList[0].path,'D');
assert.equal(visibilityRefreshes,2,'source metadata refresh requests a new admitted exclusion status');
console.log('Receipt reconciliation, failed imports, and retained source-list generations passed');

// A failed/cancelled B replacement cannot adopt A's still-published source.
for (const cancelled of [false, true]) {
  const f = fixture(), first = f.context.loadData(file('only-A')); await settle(); f.pending[0].complete(); await first;
  const original = f.state.cases.cases[0], b = { id: 'b', artifacts: [] };
  f.state.cases.cases.push(b); f.state.cases.active = b.id;
  f.node('#file-path').value = 'only-A'; f.context.resetCaseSourceState();
  const adopted = []; f.context.storeCurrentArtifactInSession = () => { adopted.push([f.state.cases.active, f.state.currentArtifact.path]); return Promise.resolve(true); };
  assert.equal(f.node('#file-path').value, ''); assert.equal(f.state.columns.length, 0); assert.equal(f.state.rows.length, 0);
  const replacement = f.context.loadData(file('missing-B')); await settle();
  if (cancelled) { f.context.window.Tasks.cancelLatest('source-load'); await settle(); f.pending[1].complete(); }
  else f.pending[1].reject(Error('missing-B unreadable'));
  assert.equal(await replacement, false);
  assert.equal(f.native, 'only-A', 'a failed replacement leaves the native publication intact for its owner');
  assert.equal(f.state.currentArtifact, null); assert.equal(f.state.columns.length, 0); assert.equal(f.state.rows.length, 0);
  assert.deepEqual(adopted, [], 'foreign source receipt cannot register or save an artifact into B');
  assert.deepEqual(b.artifacts, []); assert.equal(f.state.sourceIdentityUnconfirmed, true);
  f.state.cases.active = original.id; f.context.resetCaseSourceState(); await f.context.reconcilePublishedSource();
  assert.deepEqual(adopted, [[original.id, 'only-A']], 'returning to A may reconcile its own publication');
}

// The receipt read itself owns the Case instance, including ID reuse after import.
{
  const f = fixture(), original = f.state.cases.cases[0]; let resolveSnapshot;
  const nativeApi = f.context.api;
  f.context.api = (command, ...args) => command === 'source_snapshot' ? new Promise(resolve => { resolveSnapshot = resolve; }) : nativeApi(command, ...args);
  const reading = f.context.reconcilePublishedSource(); await settle();
  f.state.cases.cases[0] = { id: original.id, artifacts: [] };
  resolveSnapshot({ generation: 1, count: 1, columns: ['A_only'], sourceDesc: 'only-A', sources: [{ kind: 'file', paths: ['only-A'] }], analysisContext: { caseId: original.id, analysisId: 'analysis-case' } });
  assert.equal(await reading, false); assert.equal(f.state.currentArtifact, undefined);
}
console.log('A→B failure/cancel, foreign receipt rejection, return-to-owner and replaced-Case receipt tests passed');
