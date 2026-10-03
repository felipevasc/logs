import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const contextSource = readFileSync(new URL('../../frontend/analysis-context.js', import.meta.url), 'utf8');
const tasksSource = readFileSync(new URL('../../frontend/tasks.js', import.meta.url), 'utf8');
const appSource = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const workspaceSource = readFileSync(new URL('../../frontend/workspace-context.js', import.meta.url), 'utf8');
const transportSource = appSource.slice(appSource.indexOf('const caseTransport ='), appSource.indexOf('// ------------------------------------------------------------------ helpers de espera'));
const plain = value => JSON.parse(JSON.stringify(value));
const settle = async () => { for (let i = 0; i < 50; i++) await Promise.resolve(); };
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function snapshot(caseId = 'a', configRevision = 1, visibilityRevision = 1, analysisId = `analysis-${caseId}`) {
  return { schemaVersion: 1, caseId, analysisId, configRevision, visibilityRevision,
    config: { derivedFields: [], references: [] }, migrationDiagnostics: [], legacyRaw: null };
}
const identity = value => ({ caseId: value.caseId, analysisId: value.analysisId,
  configRevision: value.configRevision, visibilityRevision: value.visibilityRevision });
const caseWith = (id, context = snapshot(id)) => ({ id, analysisContext: context, items: [], workspace: { filters: ['saved'] } });

function fixture({ cases = [caseWith('a'), caseWith('b')], active = cases[0]?.id || null, tasks = false, transport = false, native, save } = {}) {
  const state = { cases: { active, cases }, sourcePublication: { generation: 10 }, currentArtifact: { id: 'source', loadedAt: 100 },
    datasetRevision: 0, rows: [{ id: 'retained' }], derivedFields: [], activeDatasetTab: 'table' };
  const calls = [], invalidations = [], workspaceInvalidations = [], events = [], messages = [], nodes = new Map();
  let saveCount = 0, nativeHandler = native, saveHandler = save;
  const node = key => {
    if (!nodes.has(key)) nodes.set(key, { dataset: {}, children: [], hidden: false, innerHTML: '',
      classList: { add() {}, remove() {}, toggle() {} }, before() {}, appendChild() {}, append() {},
      setAttribute() {}, removeAttribute() {}, querySelector() { return this; }, querySelectorAll() { return []; } });
    return nodes.get(key);
  };
  const context = vm.createContext({
    state, structuredClone, performance,
    window: {
      WorkspaceContext: { invalidateAnalysis: caseId => workspaceInvalidations.push(caseId) },
      invalidateAnalysisComputedData: value => invalidations.push(value),
      PerformanceTools: { queue: () => ({ add: (run, active = () => true) => Promise.resolve().then(() => {
        if (!active()) throw Error('Operação cancelada.');
        return run();
      }) }) },
    },
    document: { body: { dataset: { page: 'explore' } }, documentElement: { dataset: { zone: 'analysis' } },
      dispatchEvent: event => events.push(event), querySelectorAll: () => [] },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    $: node, el: () => node(Symbol()), fmtNum: String, esc: String, toast: message => messages.push(String(message)),
    setTimeout() {}, clearTimeout() {}, setInterval() {}, clearInterval() {}, requestAnimationFrame() {},
    saveCases: async () => { saveCount++; return saveHandler ? saveHandler() : true; },
    api: async (cmd, args = {}, opts = {}) => {
      calls.push({ cmd, args, opts });
      if (opts.cancelled?.()) throw Error('Operação cancelada.');
      if (nativeHandler) return nativeHandler(cmd, args, opts);
      if (cmd === 'list_derived_fields') return [];
      if (cmd === 'analysis_context_snapshot') return cases.find(item => item.id === args.caseId)?.analysisContext || null;
      return { ok: true };
    },
  });
  if (transport) {
    Object.assign(context, { invoke: context.api, TextEncoder, explorerAnalytics: new Map(), activity: { count: 0, timer: null }, ACTIVITY_DELAY: 320, activityShow() {}, activityHide() {} });
    vm.runInContext(transportSource, context, { filename: 'app-transport.js' });
  }
  vm.runInContext(contextSource, context, { filename: 'analysis-context.js' });
  if (tasks) vm.runInContext(tasksSource, context, { filename: 'tasks.js' });
  const contexts = context.window.AnalysisContexts;
  return { context, contexts, state, calls, invalidations, workspaceInvalidations, events, messages,
    get saveCount() { return saveCount; }, setNative: handler => { nativeHandler = handler; }, setSave: handler => { saveHandler = handler; },
    prime: (fields = []) => contexts.definitionsLoaded(contexts.capture(), fields) };
}

test('configuration and visibility revisions both remain monotonic', async () => {
  const f = fixture({ cases: [caseWith('a', snapshot('a', 4, 7))] });
  for (const [config, visibility, reason] of [[3, 7, 'older'], [4, 6, 'older'], [3, 6, 'older'], [4, 7, 'unchanged']]) {
    const result = await f.contexts.adopt(snapshot('a', config, visibility));
    assert.equal(result.accepted, false);
    assert.equal(result.reason, reason);
    assert.deepEqual(plain(f.contexts.identity()), identity(snapshot('a', 4, 7)));
  }
  assert.equal((await f.contexts.adopt(snapshot('a', 5, 7))).accepted, true);
  assert.equal((await f.contexts.adopt(snapshot('a', 5, 8))).accepted, true);
  assert.equal((await f.contexts.adopt(snapshot('a', 6, 9))).accepted, true);
  assert.deepEqual(plain(f.contexts.identity()), identity(snapshot('a', 6, 9)));
  assert.equal(f.calls.length, 0, 'ordered receipts need no reconciliation request');
});

test('incomparable revisions refresh the authoritative direct Snapshot in both directions', async () => {
  for (const incoming of [snapshot('a', 6, 6), snapshot('a', 4, 8)]) {
    const authoritative = snapshot('a', 7, 9);
    const f = fixture({ cases: [caseWith('a', snapshot('a', 5, 7))], native: async cmd => {
      assert.equal(cmd, 'analysis_context_snapshot');
      return authoritative;
    } });
    const result = await f.contexts.adopt(incoming);
    assert.equal(result.accepted, true);
    assert.deepEqual(plain(f.contexts.context()), authoritative);
    assert.equal(f.calls.length, 1);
    assert.deepEqual(plain(f.calls[0].args), { caseId: 'a' });
    assert.equal(f.invalidations.length, 1, 'the incomparable receipt itself is never installed');
  }
});

test('a still-incomparable authoritative snapshot fails without regressing local revisions', async () => {
  const original = snapshot('a', 5, 7);
  const f = fixture({ cases: [caseWith('a', original)], native: async () => snapshot('a', 6, 6) });
  await assert.rejects(f.contexts.adopt(snapshot('a', 6, 6)), /revisões.*divergem/);
  assert.deepEqual(plain(f.contexts.context()), original);
  assert.equal(f.invalidations.length, 0);
});

test('an inactive receipt updates its Case without disturbing the active computed view', async () => {
  const f = fixture();
  f.prime([{ name: 'active_field' }]);
  const fields = f.state.derivedFields, rows = f.state.rows, owner = f.contexts.capture('b');
  const result = await f.contexts.receipt({ analysisContext: snapshot('b', 2, 3) }, owner);
  assert.equal(result.accepted, true);
  assert.deepEqual(plain(f.contexts.identity('b')), identity(snapshot('b', 2, 3)));
  assert.equal(f.state.derivedFields, fields);
  assert.equal(f.state.rows, rows);
  assert.equal(f.state.analysisDefinitionsPending, false);
  assert.equal(f.invalidations.length, 0);
  assert.deepEqual(f.workspaceInvalidations, ['b']);
  assert.equal(f.events.at(-1).detail.active, false);
});

test('deleted/recreated Case objects reject old receipts even after Case ID and analysis ID return', async () => {
  const f = fixture();
  const oldOwner = f.contexts.capture();
  f.state.cases.cases[0] = caseWith('a', snapshot('a', 1, 1, 'replacement'));
  f.state.cases.cases[0] = caseWith('a', snapshot('a'));
  assert.equal(f.contexts.owns(oldOwner), false);
  assert.throws(() => f.contexts.assertOwner(oldOwner), /ANALYSIS_CONTEXT_CHANGED/);
  const result = await f.contexts.adopt(snapshot('a', 9, 9), { owner: oldOwner });
  assert.equal(result.reason, 'owner');
  assert.deepEqual(plain(f.contexts.identity()), identity(snapshot('a')));
});

test('analysis identity replacement rejects both an old receipt and a receipt from the intervening identity', async () => {
  const f = fixture(), item = f.state.cases.cases[0], ownerA = f.contexts.capture();
  item.analysisContext = snapshot('a', 1, 1, 'analysis-replaced');
  assert.equal(f.contexts.owns(ownerA), false);
  assert.equal((await f.contexts.adopt(snapshot('a', 8, 8), { owner: ownerA })).reason, 'owner');
  const ownerB = f.contexts.capture();
  item.analysisContext = snapshot('a');
  assert.equal((await f.contexts.adopt(snapshot('a', 8, 8, 'analysis-replaced'), { owner: ownerB })).reason, 'owner');
  assert.equal(f.invalidations.length, 0);
});

test('switching Cases clears prior definitions and loads only the selected Case definitions', async () => {
  const f = fixture({ native: async (cmd, args) => {
    assert.equal(cmd, 'list_derived_fields');
    return [{ name: `field_${args.analysisContext.caseId}` }];
  } });
  await f.contexts.prepare(f.contexts.capture());
  assert.equal(f.state.derivedFields[0].name, 'field_a');
  f.state.cases.active = 'b';
  f.contexts.activate();
  assert.equal(f.state.derivedFields.length, 0);
  assert.equal(f.state.analysisDefinitionsPending, true);
  await f.contexts.prepare(f.contexts.capture());
  assert.deepEqual(plain(f.state.derivedFields), [{ name: 'field_b' }]);
  assert.deepEqual(f.calls.map(call => call.args.analysisContext.caseId), ['a', 'b']);
});

test('new configuration reloads runtime definitions instead of installing raw snapshot definitions', async () => {
  const f = fixture({ native: async () => [{ name: 'compiled', sample: 'runtime metadata' }] });
  f.prime([{ name: 'previous' }]);
  const updated = snapshot('a', 2, 1);
  updated.config.derivedFields = [{ name: 'raw-only', rules: [{ pattern: '(.*)' }] }];
  await f.contexts.adopt(updated);
  assert.equal(f.state.derivedFields.length, 0);
  await f.contexts.prepare(f.contexts.capture());
  assert.deepEqual(plain(f.state.derivedFields), [{ name: 'compiled', sample: 'runtime metadata' }]);
  assert.equal(f.calls[0].args.analysisContext.configRevision, 2);
  assert.deepEqual(plain(f.contexts.context().config.derivedFields), updated.config.derivedFields);
});

test('a pending definition read for an old source cannot block or overwrite the new source read', async () => {
  const old = deferred(), current = deferred();
  const f = fixture({ tasks: true, native: (cmd, args) => {
    assert.equal(cmd, 'list_derived_fields');
    return args.sourceGeneration === 10 ? old.promise : current.promise;
  } });
  const oldRead = f.contexts.prepare(f.contexts.capture()).then(() => null, error => error);
  await settle();
  f.state.sourcePublication = { generation: 11 };
  f.state.currentArtifact = { id: 'next-source', loadedAt: 200 };
  const newRead = f.contexts.prepare(f.contexts.capture());
  await settle();
  assert.deepEqual(f.calls.map(call => call.args.sourceGeneration), [10, 11]);
  current.resolve([{ name: 'new_source' }]);
  await newRead;
  old.resolve([{ name: 'old_source' }]);
  assert.match(String(await oldRead), /ANALYSIS_CONTEXT_CHANGED/);
  assert.deepEqual(plain(f.state.derivedFields), [{ name: 'new_source' }]);
  assert.equal(f.state.analysisDefinitionsPending, false);
});

test('first Case save is shared across source reset without poisoning the current definition load', async () => {
  for (const change of ['reset', 'replacement']) for (const receipt of [true, false]) {
    const saved = deferred(), f = fixture({ cases: [caseWith('new', null)], tasks: true });
    const firstOwner = f.contexts.capture();
    f.setNative(async command => {
      if (command === 'analysis_context_snapshot') return snapshot('new', 0, 0);
      assert.equal(command, 'list_derived_fields');
      return [];
    });
    f.setSave(async () => {
      await saved.promise;
      if (receipt) await f.contexts.adopt(snapshot('new', 0, 0), { owner: firstOwner });
      return true;
    });
    f.context.renderExploreTree = () => {};
    const start = appSource.indexOf('async function loadDerivedFields(');
    vm.runInContext(appSource.slice(start, appSource.indexOf('\nfunction renderExploreTree(', start)), f.context);
    // newCase renders its first reads before afterCaseCreation resets the old
    // source. Both callers must share persistence, but not source ownership.
    const staleRead = f.context.api('profile_fields', { filters: [] }).then(() => null, error => error);
    await settle();
    f.state.currentArtifact = change === 'reset' ? null : { id: 'replacement', loadedAt: 200 };
    if (change === 'replacement') f.state.sourcePublication = { generation: 11 };
    const currentRead = f.context.loadDerivedFields();
    await settle();
    assert.equal(f.saveCount, 1, 'simultaneous first reads keep one durable save');
    assert.equal(f.calls.length, 0, 'nothing is admitted before persistence');
    saved.resolve();
    assert.equal(await currentRead, true, `${change}/${receipt ? 'receipt' : 'snapshot'}: current definitions must finish after shared persistence`);
    assert.match(String(await staleRead), /ANALYSIS_CONTEXT_CHANGED/);
    assert.equal(f.state.analysisDefinitionsPending, false);
    assert.deepEqual(plain(f.state.derivedFields), []);
    assert.deepEqual(f.calls.map(call => call.cmd), [...(receipt ? [] : ['analysis_context_snapshot']), 'list_derived_fields'], 'the old source query never reaches native code');
    assert.equal(f.calls.at(-1).args.sourceGeneration, change === 'reset' ? 10 : 11);
    assert.equal(f.calls.at(-1).args.analysisContext.caseId, 'new');
  }
});

test('actual newCase and afterCaseCreation finish empty isolation while an initial render waits for persistence', async () => {
  for (const scope of ['dataset', 'case']) {
    const saved = deferred(), f = fixture({ tasks: true });
    f.state.loaded = true; f.state.total = 6000; f.state.columns = ['timestamp', 'previous_only'];
    f.state.cases.cases[0].artifacts = [{ id: 'source', path: '/previous-only.jsonl' }];
    f.setSave(async () => {
      const owner = f.contexts.capture();
      await saved.promise;
      await f.contexts.adopt(snapshot(owner.caseId, 0, 0), { owner });
      return true;
    });
    let completion, staleRead;
    const c = f.context;
    Object.assign(c, {
      CURRENT_FILTER_ID: '__current__', defaultCaseWorkspace: () => ({}),
      activeCase: () => f.state.cases.cases.find(item => item.id === f.state.cases.active),
      caseGeneration: 0, restoringCase: false, runtime: new Map(), states: new Map(),
      key: value => `${f.state.cases.active}:${value}`,
      setAnalysisView() {}, renderCaseBar() {}, updateAnalysisBadge() {}, renderExploreTree() {},
      renderAnalysis: () => { staleRead = c.api('profile_fields', { filters: [] }).then(() => null, error => error); },
      resetCaseSourceState: () => Object.assign(f.state, { loaded: false, currentArtifact: null, currentOrigin: '',
        columns: [], rows: [], total: 0, dataPeriod: null, queryError: null }),
      syncActiveCaseArtifacts: async () => { await c.api('clear_events'); },
      setScope: async target => { f.state.activeContext = target; },
      refresh: async () => { throw Error('an empty new Case must not refresh the previous source'); },
    });
    const install = (source, start, end) => {
      const from = source.indexOf(start); assert.ok(from >= 0);
      vm.runInContext(source.slice(from, source.indexOf(end, from)), c);
    };
    install(appSource, 'async function loadDerivedFields(', '\nfunction renderExploreTree(');
    install(appSource, 'function newCase(', '\nfunction caseItems(');
    install(workspaceSource, '  function defaults()', '  const record =');
    install(workspaceSource, '  function sourceRuntime(', '  function stored(');
    install(workspaceSource, '  async function afterCaseCreation(', '  async function deleteCase(');
    Object.assign(c.window.WorkspaceContext, {
      beforeCaseCreation: () => { c.caseGeneration++; c.restoringCase = true; return { scope, snapshot: { page: 'explore' } }; },
      afterCaseCreation: previous => { completion = c.afterCaseCreation(previous); return completion; },
    });
    const created = c.newCase('Isolated');
    await settle(); assert.equal(f.calls.length, 0);
    saved.resolve(); await completion;
    assert.match(String(await staleRead), /ANALYSIS_CONTEXT_CHANGED/);
    assert.equal(c.restoringCase, false); assert.equal(f.state.analysisDefinitionsPending, false);
    assert.equal(f.state.activeContext, scope); assert.equal(f.state.loaded, false); assert.equal(f.state.total, 0);
    assert.equal(f.state.currentArtifact, null); assert.deepEqual(plain(f.state.columns), []);
    assert.deepEqual(plain(created.artifacts), []); assert.equal(created.workspace.activeScope, scope);
    assert.equal(f.state.cases.cases[0].artifacts[0].path, '/previous-only.jsonl');
    assert.deepEqual(f.calls.map(call => call.cmd), ['list_derived_fields', 'clear_events']);
    assert.ok(f.calls.every(call => call.args.analysisContext.caseId === created.id));
  }
});

test('shared first-save completion cannot admit a switched or recreated Case caller', async () => {
  for (const change of ['switch', 'recreate']) {
    const saved = deferred(), f = fixture({ cases: [caseWith('a', null), caseWith('b')], tasks: true });
    f.setSave(() => saved.promise);
    const pending = f.context.api('clear_events').then(() => null, error => error);
    await settle();
    if (change === 'switch') f.state.cases.active = 'b';
    else f.state.cases.cases[0] = caseWith('a');
    f.contexts.activate(); f.prime([{ name: 'current_fields' }]);
    saved.resolve(true);
    assert.match(String(await pending), /ANALYSIS_CONTEXT_CHANGED/);
    assert.equal(f.calls.some(call => call.cmd === 'clear_events'), false);
    assert.deepEqual(plain(f.state.derivedFields), [{ name: 'current_fields' }]);
  }
});

test('pending definition requests are isolated across visibility-only revisions too', async () => {
  const old = deferred(), current = deferred();
  const f = fixture({ native: (cmd, args) => args.analysisContext.visibilityRevision === 1 ? old.promise : current.promise });
  const oldRead = f.contexts.prepare(f.contexts.capture()).then(() => null, error => error);
  await settle();
  await f.contexts.adopt(snapshot('a', 1, 2));
  const newRead = f.contexts.prepare(f.contexts.capture());
  await settle();
  assert.deepEqual(f.calls.map(call => call.args.analysisContext.visibilityRevision), [1, 2]);
  current.resolve([{ name: 'current_visibility' }]); await newRead;
  old.resolve([{ name: 'old_visibility' }]);
  assert.match(String(await oldRead), /ANALYSIS_CONTEXT_CHANGED/);
  assert.equal(f.state.derivedFields[0].name, 'current_visibility');
});

test('initial source and configuration commands wait for a durable save receipt before dispatch', async () => {
  for (const cmd of ['load_file', 'load_files', 'load_bundle', 'load_event_log', 'clear_events', 'save_derived_field', 'delete_derived_field']) {
    const saved = deferred();
    const f = fixture({ cases: [caseWith('a', null)], tasks: true });
    const owner = f.contexts.capture();
    f.setSave(async () => {
      await saved.promise;
      await f.contexts.adopt(snapshot('a', 3, 4), { owner });
      return true;
    });
    const result = f.context.api(cmd, { name: 'authored', path: 'source.log' });
    await settle();
    assert.equal(f.saveCount, 1, cmd);
    assert.equal(f.calls.length, 0, `${cmd} must not reach native code before the save receipt`);
    saved.resolve();
    await result;
    const dispatched = f.calls.find(call => call.cmd === cmd);
    assert.ok(dispatched, `${cmd} dispatches after persistence`);
    assert.deepEqual(plain(dispatched.args.analysisContext), identity(snapshot('a', 3, 4)));
    assert.equal(dispatched.args.sourceGeneration, 10);
    assert.equal(f.calls.some(call => call.cmd === 'analysis_context_snapshot'), false, 'save receipt avoids another snapshot fetch');
  }
});

test('simultaneous first reads share durable admission and do not dispatch early', async () => {
  const saved = deferred();
  const f = fixture({ cases: [caseWith('a', null)], tasks: true });
  const owner = f.contexts.capture();
  f.setSave(async () => { await saved.promise; await f.contexts.adopt(snapshot('a'), { owner }); return true; });
  const first = f.context.api('query_page', { offset: 0 }), second = f.context.api('count_filtered', {});
  await settle();
  assert.equal(f.saveCount, 1);
  assert.equal(f.calls.length, 0);
  saved.resolve();
  await Promise.all([first, second]);
  assert.equal(f.calls.filter(call => call.cmd === 'list_derived_fields').length, 1);
  assert.equal(f.calls.filter(call => ['query_page', 'count_filtered'].includes(call.cmd)).length, 2);
});

test('failed persistence prevents scoped source admission', async () => {
  const f = fixture({ cases: [caseWith('a', null)], tasks: true, save: async () => false });
  await assert.rejects(f.context.api('load_file', { path: 'unpersisted.log' }), /Salve o Caso/);
  assert.equal(f.calls.length, 0);
  assert.equal(f.contexts.identity(), null);
});

test('legacy null snapshots stay null and do not trigger repeated Case persistence', async () => {
  const f = fixture({ cases: [caseWith('a', null)], tasks: true });
  await f.context.api('query_page', { offset: 0 });
  await f.context.api('query_page', { offset: 1 });
  assert.equal(f.saveCount, 1);
  assert.equal(f.calls.filter(call => call.cmd === 'analysis_context_snapshot').length, 1);
  assert.equal(f.contexts.identity(), null);
  for (const call of f.calls.filter(call => call.cmd === 'query_page')) assert.equal(call.args.analysisContext, null);
});

test('a null active Case still permits a scoped source clear with legacy null identity', async () => {
  const f = fixture({ cases: [], tasks: true });
  await f.context.api('clear_events');
  assert.equal(f.saveCount, 0);
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].cmd, 'clear_events');
  assert.equal(f.calls[0].args.analysisContext, null);
  assert.equal(f.calls[0].args.sourceGeneration, 10);
});

test('Tasks forwards captured identity and generation, overriding stale supplied context arguments', async () => {
  const f = fixture({ tasks: true });
  f.prime([{ name: 'a_field' }]);
  const owner = f.contexts.capture();
  await f.context.api('query_page', { filters: [], analysisContext: identity(snapshot('b')), sourceGeneration: 999 }, { analysisOwner: owner });
  assert.equal(f.calls.length, 1);
  assert.deepEqual(plain(f.calls[0].args.analysisContext), identity(snapshot('a')));
  assert.equal(f.calls[0].args.sourceGeneration, 10);
  assert.equal(f.calls[0].opts.cancelled(), false);
});

test('visible source metadata uses captured ownership and rejects replies after visibility changes', async () => {
  for (const command of ['list_sources', 'source_summary', 'exclusion_visibility', 'reference_list', 'reference_inspect']) {
    const response = deferred(), f = fixture({ tasks: true, native: () => response.promise });
    const owner = f.contexts.capture();
    const read = f.context.api(command, {}, { analysisOwner: owner }).then(value => ({ value }), error => ({ error }));
    await settle();
    assert.equal(f.calls.length, 1, 'metadata admission does not trigger derived-field discovery');
    assert.deepEqual(plain(f.calls[0].args.analysisContext), plain(owner.identity));
    assert.equal(f.calls[0].args.sourceGeneration, 10);
    await f.contexts.adopt(snapshot('a', 1, 2)); response.resolve({ count: 99 });
    assert.match(String((await read).error), /ANALYSIS_CONTEXT_CHANGED/);
  }
});

test('Tasks rejects late read results when the Case, revisions, source, or Case instance changes', async () => {
  for (const change of ['active-case', 'config', 'visibility', 'source', 'recreated-case', 'analysis-id']) {
    const response = deferred();
    const f = fixture({ tasks: true, native: () => response.promise });
    f.prime();
    const read = f.context.api('query_page', {}).then(value => ({ value }), error => ({ error }));
    await settle();
    if (change === 'active-case') f.state.cases.active = 'b';
    if (change === 'config') await f.contexts.adopt(snapshot('a', 2, 1));
    if (change === 'visibility') await f.contexts.adopt(snapshot('a', 1, 2));
    if (change === 'source') f.state.sourcePublication.generation++;
    if (change === 'recreated-case') f.state.cases.cases[0] = caseWith('a');
    if (change === 'analysis-id') f.state.cases.cases[0].analysisContext = snapshot('a', 1, 1, 'replacement');
    assert.equal(f.calls[0].opts.cancelled(), true, change);
    response.resolve({ rows: [{ id: 'obsolete' }] });
    const outcome = await read;
    assert.match(String(outcome.error), /ANALYSIS_CONTEXT_CHANGED/, change);
    assert.equal(outcome.value, undefined, change);
  }
});

test('Tasks rejects stale queued ownership before any native dispatch', async () => {
  const f = fixture({ tasks: true });
  f.prime();
  const read = f.context.api('query_page', {}, { background: true }).then(value => ({ value }), error => ({ error }));
  f.state.cases.active = 'b';
  const outcome = await read;
  assert.match(String(outcome.error), /ANALYSIS_CONTEXT_CHANGED/);
  assert.equal(f.calls.length, 0);
});

test('a completed mutation receipt is adopted for its inactive owner without invalidating the active Case', async () => {
  const response = deferred();
  const f = fixture({ tasks: true, native: () => response.promise });
  f.prime();
  const mutation = f.context.api('save_derived_field', { name: 'field_a' });
  await settle();
  f.state.cases.active = 'b';
  f.contexts.activate(); f.prime([{ name: 'field_b' }]);
  const fields = f.state.derivedFields;
  response.resolve({ ok: true, analysisContext: snapshot('a', 2, 1) });
  await mutation;
  assert.equal(f.contexts.identity('a').configRevision, 2);
  assert.equal(f.state.derivedFields, fields);
  assert.equal(f.invalidations.length, 0);
});

test('committed source receipts remain available after a late cancellation', async () => {
  const response = deferred();
  const f = fixture({ tasks: true, native: cmd => cmd === 'cancel_task' ? true : response.promise });
  const load = f.context.api('load_file', { path: 'committed.log' }, { latest: 'source-load' });
  await settle();
  f.context.window.Tasks.cancelLatest('source-load');
  response.resolve({ publication: { generation: 11, analysisContext: identity(snapshot('a')) }, count: 3 });
  const result = await load;
  assert.equal(result.publication.generation, 11);
  assert.equal(result.count, 3);
  assert.equal(f.contexts.identity().configRevision, 1, 'an identity-only source receipt is not mistaken for a full Snapshot');
});


test('legacy null context never borrows another Case or global definition list', async () => {
  const f=fixture({cases:[caseWith('legacy',null)],tasks:true,native:async cmd=>cmd==='analysis_context_snapshot'?null:cmd==='list_derived_fields'?[{name:'foreign-global-field'}]:{ok:true}});
  f.state.derivedFields=[{name:'previous-case-field'}];
  await f.context.api('query_page',{});
  assert.deepEqual(plain(f.state.derivedFields),[]);
  assert.equal(f.calls.filter(call=>call.cmd==='list_derived_fields').length,0,'null context cannot prove ownership of a global metadata response');
  assert.equal(f.calls.find(call=>call.cmd==='query_page').args.analysisContext,null,'legacy source reads remain compatible');
  f.contexts.definitionsLoaded(f.contexts.capture(),[{name:'foreign'}]);
  assert.deepEqual(plain(f.state.derivedFields),[]);
});

test('late timestamp cancellation preserves the authoritative source publication receipt', async () => {
  const result=deferred();
  const f=fixture({tasks:true,native:async cmd=>cmd==='set_ts_config'?result.promise:true});
  const applying=f.context.api('set_ts_config',{path:'source.jsonl',config:{sources:['message'],format:'unix'}},{latest:'timestamp-config',silent:true});
  await settle();f.context.window.Tasks.cancelLatest('timestamp-config');
  const publication={generation:11,operationId:'published-timestamps',analysisContext:identity(snapshot('a'))};
  result.resolve({publication});
  assert.deepEqual(plain((await applying).publication),publication,'a completed native retimestamp cannot be represented as a rollback');
});

const interpretationReads = ['get_codes','system_codes_count','list_formats','get_ts_config','detection_rules','threat_catalog'];
const interpretationWrites = ['save_codes','harvest_codes','save_custom_format','detection_settings_save','sigma_import','sigma_clear','threat_catalog_update'];
test('Case interpretation and security catalog reads carry exact metadata admission and reject late A responses', async () => {
  for (const command of interpretationReads) {
    const reply = deferred(), f = fixture({ tasks: true, native: cmd => { assert.equal(cmd, command); return reply.promise; } });
    const owner = f.contexts.capture(), reading = f.context.api(command, { path: 'same-path.log' }, { silent: true, analysisOwner: owner }).catch(String);
    await settle(); assert.deepEqual(plain(f.calls[0].args.analysisContext), identity(snapshot('a')), command);
    assert.equal(f.calls[0].args.sourceGeneration, 10); f.state.cases.active = 'b'; f.contexts.activate(); reply.resolve({ from: 'a' });
    assert.match(await reading, /ANALYSIS_CONTEXT_CHANGED/, command);
    assert.equal(f.contexts.identity().caseId, 'b');
  }
});
test('late interpretation and security mutation receipts update only their captured Case and never B', async () => {
  for (const command of interpretationWrites) {
    const reply = deferred(), f = fixture({ tasks: true, native: cmd => { assert.equal(cmd, command); return reply.promise; } });
    const owner = f.contexts.capture(), writing = f.context.api(command, { text: 'A content' }, { silent: true, analysisOwner: owner });
    await settle(); assert.deepEqual(plain(f.calls[0].args.analysisContext), identity(snapshot('a')), command);
    f.state.cases.active = 'b'; f.contexts.activate(); const before = f.contexts.identity();
    reply.resolve({ analysisContext: snapshot('a', 2, 1) }); await writing;
    assert.deepEqual(plain(f.contexts.identity()), plain(before)); assert.equal(f.contexts.identity('a').configRevision, 2);
    assert.equal(f.invalidations.length, 0, 'inactive receipt cannot clear current Case computed data');
  }
});

test('timestamp batch carries both newly committed configuration revision and source generation into each next file', async () => {
  const f = fixture({ tasks: true, native: async (command, args) => {
    assert.equal(command, 'set_ts_config'); const revision = args.analysisContext.configRevision + 1;
    return { publication: { generation: args.sourceGeneration + 1, analysisContext: identity(snapshot('a', revision, 1)) }, analysisContext: snapshot('a', revision, 1) };
  } });
  f.state.tsAnalysisOwner = f.contexts.capture(); f.context.invalidateAnalysisComputedData = () => {};
  const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
  vm.runInContext(app.slice(app.indexOf('async function commitTsConfig('), app.indexOf('async function resetTsConfig(')), f.context);
  await f.context.commitTsConfig(['same-one.log','same-two.log'], { sources: ['message'], format: 'epoch_ms' });
  assert.deepEqual(f.calls.map(call => [call.args.analysisContext.caseId, call.args.analysisContext.configRevision, call.args.sourceGeneration]), [['a',1,10],['a',2,11]]);
  assert.equal(f.state.sourcePublication.generation, 12); assert.equal(f.state.tsAnalysisOwner.identity.configRevision, 3);
});
test('filter preflight and final command retain the same captured Case interpretation for identical rule IDs', async () => {
  const f = fixture({ tasks: true, transport: true, native: (cmd, args) => {
    if (cmd === 'list_derived_fields') return [];
    if (cmd === 'validate_filters') { if (args.analysisContext.caseId === 'b') throw Error('Rule same-id disabled in Case B'); return null; }
    return { owner: args.analysisContext.caseId };
  } });
  const filters = [{ column: '_all', op: 'detection', value: 'same-id' }];
  assert.equal((await f.context.api('aggregate_events', { filters }, { silent: true })).owner, 'a');
  const first = f.calls.filter(call => ['validate_filters', 'aggregate_events'].includes(call.cmd));
  assert.deepEqual(first.map(call => call.cmd), ['validate_filters', 'aggregate_events']);
  assert.deepEqual(plain(first[0].args.analysisContext), identity(snapshot('a'))); assert.deepEqual(plain(first[0].args.analysisContext), plain(first[1].args.analysisContext));
  assert.deepEqual(Object.keys(first[0].args).sort(), ['analysisContext', 'filters']);
  f.state.cases.active = 'b'; f.contexts.activate();
  await assert.rejects(f.context.api('aggregate_events', { filters }, { silent: true }), /disabled in Case B/);
  assert.equal(f.calls.filter(call => call.cmd === 'aggregate_events').length, 1, 'B invalid rule fails before dispatching aggregation');
  assert.equal(f.calls.at(-1).args.analysisContext.caseId, 'b');
});
test('explicit filter editor validation is one metadata-only invoke and never prepares a Case payload', async () => {
  const f = fixture({ tasks: true, transport: true, native: () => null });
  const owner = f.contexts.capture();
  await f.context.api('validate_filters', { filters: [{ column: '_all', op: 'threat_rule', value: 'same-id' }], caseEvents: [{ id: 'never-transfer' }] }, { silent: true, analysisOwner: owner });
  assert.deepEqual(f.calls.map(call => call.cmd), ['validate_filters']);
  assert.deepEqual(Object.keys(f.calls[0].args).sort(), ['analysisContext', 'filters']);
  assert.deepEqual(plain(f.calls[0].args.analysisContext), identity(snapshot('a')));
});
test('late metadata validation cannot be accepted as B and cannot dispatch the original filtered command after switching', async () => {
  for (const command of ['validate_filters', 'aggregate_events']) {
    const gate = deferred(), f = fixture({ tasks: true, transport: true, native: cmd => cmd === 'list_derived_fields' ? [] : gate.promise });
    const validating = f.context.api(command, { filters: [{ column: '_all', op: 'detection', value: 'same-id' }] }, { silent: true });
    const rejected = assert.rejects(validating, /ANALYSIS_CONTEXT_CHANGED|Operação cancelada/); await settle();
    assert.equal(f.calls.at(-1).cmd, 'validate_filters'); f.state.cases.active = 'b'; f.contexts.activate(); gate.resolve(null); await rejected;
    assert.equal(f.calls.filter(call => call.cmd === 'aggregate_events').length, 0);
  }
});
test('Case-aware Tasks preserves the four hotpath preflight exemptions', async () => {
  const f = fixture({ tasks: true, transport: true, native: cmd => cmd === 'list_derived_fields' ? [] : { okay: true } });
  for (const command of ['query_page', 'count_filtered', 'stats_events', 'tree_aggs']) await f.context.api(command, { filters: [{ column: '_all', op: 'detection', value: 'same-id' }] }, { silent: true });
  assert.equal(f.calls.filter(call => call.cmd === 'validate_filters').length, 0);
  assert.ok(f.calls.every(call => call.args.analysisContext?.caseId === 'a'));
});
