import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = name => readFileSync(new URL(`../../frontend/${name}`, import.meta.url), 'utf8');
const app = read('app.js');
const transport = app.slice(app.indexOf('const caseTransport ='), app.indexOf('\nasync function api('));
const nativeApi = app.slice(app.indexOf('async function api('), app.indexOf('// ------------------------------------------------------------------ helpers de espera'));
assert.ok(transport.includes('async function caseArgs('), 'use the installed Case transport');

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

// Execute the complete controller and real ownership/Case preparation code. Only
// native calls, DOM endpoints and clipboard I/O are replaced with controllable IO.
function harness({ scope = 'dataset', legacy = false, realApi = false } = {}) {
  const snapshot = {
    schemaVersion: 1, caseId: 'case-a', analysisId: 'analysis-a',
    configRevision: 2, visibilityRevision: 3,
    config: { derivedFields: [], references: [] }, migrationDiagnostics: [],
  };
  const state = {
    cases: { active: 'case-a', cases: [{ id: 'case-a', analysisContext: legacy ? null : structuredClone(snapshot) }] },
    sourcePublication: { generation: 7 }, currentArtifact: { id: 'source-a', loadedAt: 11 },
    sourceIdentityUnconfirmed: false, derivedFields: [],
  };
  const event = {
    id: 41, event_ref: 'source-a:41', timestamp: 1700000000123,
    source: 'local source', name: 'local catalog name', description: 'local catalog description',
    fields: { value: 'local value', float: 1, unsafe: 18446744073709551615, object: { 2: 'second', 10: 'tenth' } },
  };
  const calls = { prepare: [], sync: [], exact: [], definitions: [], saves: [], drafts: [], filters: [], copies: [], notices: [], focus: [], cancels: [] };
  const live = { scope, evidence: [event], evidenceRevision: 1, detailCurrent: true };
  const hooks = {};
  const anchor = { isConnected: true, focus: options => calls.focus.push({ anchor, options }) };
  const listeners = new Map();
  const exactResult = (call, overrides = {}) => ({
    kind: 'exact_field', version: 1,
    receipt: {
      analysisContext: { ...call.args.analysisContext },
      sourceGeneration: call.args.caseKey ? null : call.args.sourceGeneration,
      caseKey: call.args.caseKey ?? null,
      caseContentToken: call.args.caseKey ? call.args.caseContentToken : null,
      catalogSignature: 'a'.repeat(64), catalogEpoch: 4,
    },
    row: { id: call.args.id, eventRef: call.args.eventRef }, column: call.args.column,
    presence: 'present', valueType: 'string', canonicalText: 'native value', ...overrides,
  });
  const context = vm.createContext({
    state, window: { Tasks: { cancelLatest: key => calls.cancels.push(key) } }, structuredClone, TextEncoder,
    document: {
      addEventListener: (type, fn, capture) => listeners.set(type, { fn, capture }),
      dispatchEvent() {},
    },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    workspaceScope: () => live.scope,
    caseSig: () => `${state.cases.active}:${live.evidenceRevision}`,
    caseEvents: () => live.evidence,
    filterFocusTarget: target => target,
    openValueFilter: (...args) => calls.drafts.push(args),
    addFilter: filter => calls.filters.push(filter),
    toast: (message, level) => calls.notices.push({ message, level }),
    navigator: { clipboard: { writeText: async text => { calls.copies.push(text); await hooks.clipboard?.(text); } } },
    saveCases: async () => {
      calls.saves.push(true);
      if (hooks.save) return hooks.save();
      state.cases.cases[0].analysisContext = structuredClone(snapshot);
      return true;
    },
    invoke: async (command, args) => {
      assert.equal(command, 'case_sync');
      calls.sync.push(args);
      const response = await hooks.sync?.(args);
      return response === undefined ? { caseContentToken: 'captured-evidence-token' } : response;
    },
    api: async (command, args, options) => {
      if (command === 'list_derived_fields') {
        calls.definitions.push({ args, options });
        return hooks.definitions ? hooks.definitions(args) : [];
      }
      assert.equal(command, 'analysis_field_text', 'exact field actions do not fetch/rebuild full events');
      const call = { command, args, options };
      calls.exact.push(call);
      return hooks.exact ? hooks.exact(call) : exactResult(call);
    },
  });
  vm.runInContext(read('analysis-context.js'), context, { filename: 'analysis-context.js' });
  const prepare = context.window.AnalysisContexts.prepare;
  context.window.AnalysisContexts.prepare = async owner => { calls.prepare.push(owner); return prepare(owner); };
  vm.runInContext(transport, context, { filename: 'app.js:caseArgs' });
  if (realApi) {
    const ioApi = context.api, ioInvoke = context.invoke;
    context.invoke = (command, args) => command === 'analysis_field_text' ? ioApi(command, args) : ioInvoke(command, args);
    vm.runInContext(nativeApi, context, { filename: 'app.js:api' });
    const api = context.api;
    context.api = (command, args, options) => command === 'analysis_field_text' ? api(command, args, options) : ioApi(command, args, options);
  }
  vm.runInContext(read('canonical-fields.js'), context, { filename: 'canonical-fields.js' });
  const fields = context.window.CanonicalFields;
  const capture = (column = 'value', options = {}) => fields.capture(event, column, { anchor, ...options });
  const escape = (key = 'Escape') => {
    const effects = { prevented: false, stopped: false };
    listeners.get('keydown').fn({ key, preventDefault: () => { effects.prevented = true; }, stopPropagation: () => { effects.stopped = true; } });
    return effects;
  };
  return { state, event, live, calls, hooks, anchor, context, fields, capture, exactResult, escape, listeners };
}

async function reached(predicate, message) {
  for (let attempt = 0; attempt < 40; attempt++) {
    if (predicate()) return;
    await Promise.resolve();
  }
  assert.fail(message);
}

function noValueEffects(h, message) {
  assert.equal(h.calls.drafts.length, 0, `${message}: no composer`);
  assert.equal(h.calls.filters.length, 0, `${message}: no applied filter`);
  assert.equal(h.calls.copies.length, 0, `${message}: no clipboard write`);
}

test('async preparation adopts the first identity before resolving exact text', async () => {
  const h = harness({ legacy: true }), gate = deferred();
  h.hooks.definitions = () => gate.promise;
  const action = h.capture(), promise = h.fields.filter(action);
  assert.equal(action.owner.identity, null, 'legacy capture has no invented receipt');
  await reached(() => h.calls.definitions.length === 1, 'definitions preparation started');
  assert.equal(h.calls.exact.length, 0, 'the native read waits for analysis preparation');
  noValueEffects(h, 'pending preparation');
  gate.resolve([]);
  assert.equal(await promise, true);
  assert.equal(h.calls.saves.length, 1);
  const call = h.calls.exact[0];
  assert.equal(call.args.analysisContext.analysisId, 'analysis-a');
  assert.equal(call.args.sourceGeneration, 7);
  assert.equal(call.options.analysisOwner, action.owner);
  assert.equal(call.options.latest, 'canonical-field-action');
  assert.equal(call.options.silent, true);
  assert.equal(h.calls.drafts[0][1], 'native value');
  assert.equal(h.calls.filters.length, 0, 'opening the editable composer does not apply a filter');
});

test('Case reads carry the captured evidence and immutable transport key through async sync', async () => {
  const h = harness({ scope: 'case' }), gate = deferred();
  h.hooks.sync = () => gate.promise;
  const action = h.capture(), captured = action.evidence, promise = h.fields.filter(action);
  await reached(() => h.calls.sync.length === 1, 'Case synchronization started');
  assert.equal(h.calls.sync[0].events, captured);
  assert.equal(h.calls.sync[0].analysisContext, action.owner.identity);
  assert.equal(h.calls.exact.length, 0);
  // Even if a cache returns another array, this request keeps the exact captured
  // snapshot. Actual evidence changes are guarded separately by caseSig.
  h.live.evidence = [{ ...h.event, event_ref: 'replacement:41' }];
  gate.resolve();
  assert.equal(await promise, true);
  const call = h.calls.exact[0];
  assert.equal(call.args.caseKey, h.calls.sync[0].key);
  assert.equal(call.args.caseContentToken, 'captured-evidence-token', 'native reads carry the evidence token returned by Case sync');
  assert.equal(call.args.caseEvents, undefined, 'wire request contains the prepared key');
  assert.equal(call.options.caseEvents, captured, 'cache-miss resync retains original evidence');
  assert.equal(call.args.id, 41);
  assert.equal(call.args.eventRef, 'source-a:41');
});

test('a real Case cache-miss retry validates the refreshed dispatch token and keeps captured evidence', async () => {
  const h = harness({ scope: 'case', realApi: true }), captured = h.live.evidence;
  h.hooks.sync = () => ({ caseContentToken: `evidence-publication-${h.calls.sync.length}` });
  h.hooks.exact = call => {
    if (h.calls.exact.length === 1) {
      h.live.evidence = [{ ...h.event, event_ref: 'replacement:41' }];
      throw Error('CASE_CACHE_MISS');
    }
    return h.exactResult(call, { canonicalText: 'captured native value' });
  };
  assert.equal(await h.fields.filter(h.capture()), true);
  assert.equal(h.calls.sync.length, 2);
  assert.equal(h.calls.exact.length, 2);
  for (const sync of h.calls.sync) assert.equal(sync.events, captured);
  assert.equal(h.calls.exact[0].args.caseKey, h.calls.exact[1].args.caseKey);
  assert.equal(h.calls.exact[0].args.caseContentToken, 'evidence-publication-1');
  assert.equal(h.calls.exact[1].args.caseContentToken, 'evidence-publication-2');
  assert.equal(h.calls.drafts[0][1], 'captured native value');
  assert.equal(h.calls.notices.length, 0);
});

test('changed Case caches and missing synchronization tokens fail without substituting current evidence', async () => {
  const changed = harness({ scope: 'case', realApi: true });
  changed.hooks.exact = () => { throw Error('CASE_CACHE_CHANGED'); };
  assert.equal(await changed.fields.filter(changed.capture()), false);
  assert.equal(changed.calls.sync.length, 1, 'changed immutable evidence is not retried as a missing cache');
  assert.equal(changed.calls.exact.length, 1);
  noValueEffects(changed, 'changed cache');
  const missing = harness({ scope: 'case', realApi: true });
  missing.hooks.sync = () => null;
  assert.equal(await missing.fields.copy(missing.capture()), false);
  assert.equal(missing.calls.exact.length, 0, 'a sync without an immutable token never reaches the native field read');
  assert.match(missing.calls.notices.at(-1).message, /confirmar a versão exata das evidências/);
  noValueEffects(missing, 'missing content token');
});

const canonicalFixtures = [
  { column: 'float', valueType: 'number', text: '1.0' },
  { column: 'unsafe', valueType: 'number', text: '18446744073709551615' },
  { column: 'object', valueType: 'object', text: '{"10":"tenth","2":"second","nested":[18446744073709551615,1.0]}' },
  { column: 'array', valueType: 'array', text: '[1.0,18446744073709551615,{"10":true,"2":false}]' },
  { column: 'name', valueType: 'string', text: 'Catalog name from current native admission' },
  { column: 'description', valueType: 'string', text: 'Catalog description from current native admission' },
  { column: '@user_agent', valueType: 'string', text: '  native derived value\n' },
];

test('native comparator spellings reach composer, direct filters and clipboard verbatim', async t => {
  for (const fixture of canonicalFixtures) await t.test(fixture.column, async () => {
    const h = harness();
    // The IPC envelope crosses JSON, but canonicalText itself must stay a string.
    h.hooks.exact = call => JSON.parse(JSON.stringify(h.exactResult(call, { valueType: fixture.valueType, canonicalText: fixture.text })));
    assert.equal(await h.fields.filter(h.capture(fixture.column)), true);
    assert.deepEqual([...h.calls.drafts[0]], [fixture.column, fixture.text, h.anchor, 'equals_exact']);
    assert.equal(await h.fields.filter(h.capture(fixture.column), { apply: true }), true);
    const applied = h.calls.filters[0];
    assert.equal(applied.column, fixture.column);
    assert.equal(applied.op, 'equals_exact');
    assert.equal(applied.value, fixture.text);
    assert.equal(applied.value2, null);
    assert.equal(await h.fields.copy(h.capture(fixture.column)), true);
    assert.deepEqual(h.calls.copies, [fixture.text]);
    assert.equal(h.calls.focus.at(-1).anchor, h.anchor);
    assert.equal(h.calls.focus.at(-1).options.preventScroll, true);
  });
});

test('empty, missing, explicit null and timestamp-null keep distinct filter/copy semantics', async t => {
  const cases = [
    { name: 'empty string', column: 'value', presence: 'present', valueType: 'string', text: '', op: 'equals_exact', copy: '' },
    { name: 'false boolean', column: 'value', presence: 'present', valueType: 'boolean', text: 'false', op: 'equals_exact', copy: 'false' },
    { name: 'explicit dynamic null', column: 'value', presence: 'null', valueType: 'null', text: 'null', op: 'equals_exact', copy: 'null' },
    { name: 'missing field', column: 'value', presence: 'missing', valueType: null, text: null, op: 'empty' },
    { name: 'timestamp null', column: 'timestamp', presence: 'null', valueType: 'null', text: null, op: 'empty' },
    { name: 'timestamp value', column: 'timestamp', presence: 'present', valueType: 'number', text: '1700000000123', op: 'between', copy: '1700000000123' },
  ];
  for (const value of cases) await t.test(value.name, async () => {
    const h = harness();
    h.hooks.exact = call => h.exactResult(call, { presence: value.presence, valueType: value.valueType, canonicalText: value.text });
    assert.equal(await h.fields.filter(h.capture(value.column)), true);
    assert.equal(h.calls.drafts[0][1], value.text);
    assert.equal(h.calls.drafts[0][3], value.op);
    assert.equal(await h.fields.filter(h.capture(value.column), { apply: true }), true);
    assert.equal(h.calls.filters[0].value, value.text ?? '');
    assert.equal(h.calls.filters[0].value2, value.op === 'between' ? value.text : null);
    assert.equal(await h.fields.copy(h.capture(value.column)), Object.hasOwn(value, 'copy'));
    assert.deepEqual(h.calls.copies, Object.hasOwn(value, 'copy') ? [value.copy] : []);
    if (!Object.hasOwn(value, 'copy')) assert.match(h.calls.notices.at(-1).message, /não tem um valor para copiar/);
  });
});

test('explicit filter operators preserve native text without applying the editable draft', async () => {
  const h = harness();
  h.hooks.exact = call => h.exactResult(call, { valueType: 'number', canonicalText: '1.0' });
  assert.equal(await h.fields.filter(h.capture('float'), { op: 'contains' }), true);
  assert.equal(h.calls.drafts[0][1], '1.0');
  assert.equal(h.calls.drafts[0][3], 'contains');
  assert.equal(h.calls.filters.length, 0);
  assert.equal(await h.fields.filter(h.capture('float'), { op: 'not_equals', apply: true }), true);
  assert.equal(h.calls.filters[0].op, 'not_equals');
  assert.equal(h.calls.filters[0].value, '1.0');
});

const ownershipChanges = [
  ['active Case', h => { h.state.cases.active = 'case-b'; }],
  ['Case instance', h => { h.state.cases.cases[0] = structuredClone(h.state.cases.cases[0]); }],
  ['analysis identity', h => { h.state.cases.cases[0].analysisContext.analysisId = 'replacement-analysis'; }],
  ['configuration revision', h => { h.state.cases.cases[0].analysisContext.configRevision++; }],
  ['visibility revision', h => { h.state.cases.cases[0].analysisContext.visibilityRevision++; }],
  ['source generation', h => { h.state.sourcePublication.generation++; }],
  ['source identity', h => { h.state.currentArtifact.id = 'replacement-source'; }],
  ['source reload', h => { h.state.currentArtifact.loadedAt++; }],
  ['unconfirmed source', h => { h.state.sourceIdentityUnconfirmed = true; }],
  ['scope', h => { h.live.scope = 'dataset'; }],
  ['Case evidence', h => { h.live.evidenceRevision++; }],
  ['current detail', h => { h.live.detailCurrent = false; }],
];

test('source, Case, revisions, evidence and detail guards reject stale asynchronous work at each boundary', async t => {
  for (const stage of ['before click', 'prepare', 'Case sync', 'native reply']) await t.test(stage, async () => {
    for (const [name, change] of ownershipChanges) {
      const h = harness({ scope: 'case' }), gate = deferred();
      const action = h.capture('value', { guard: () => h.live.detailCurrent });
      if (stage === 'before click') change(h);
      if (stage === 'prepare') h.hooks.definitions = () => gate.promise;
      if (stage === 'Case sync') h.hooks.sync = () => gate.promise;
      if (stage === 'native reply') h.hooks.exact = () => gate.promise;
      const promise = h.fields.filter(action, { apply: true });
      if (stage !== 'before click') {
        const target = stage === 'prepare' ? h.calls.definitions : stage === 'Case sync' ? h.calls.sync : h.calls.exact;
        await reached(() => target.length === 1, `${name}: reached ${stage}`);
        change(h);
        gate.resolve(stage === 'prepare' ? [] : stage === 'native reply' ? h.exactResult(h.calls.exact[0]) : undefined);
      }
      assert.equal(await promise, false, `${name} during ${stage}`);
      noValueEffects(h, `${name} during ${stage}`);
      assert.equal(h.calls.focus.length, 0, 'stale anchors never regain focus');
      if (stage === 'before click') assert.equal(h.calls.prepare.length, 0);
      if (stage !== 'native reply') assert.equal(h.calls.exact.length, 0, 'invalidated preparation never sends a native read');
    }
  });
});

test('malformed and mismatched native envelopes are refused, including catalog receipt fields', async t => {
  const cases = [
    ['wrong kind', result => { result.kind = 'projected_row'; }],
    ['wrong version', result => { result.version = 2; }],
    ['wrong row id', result => { result.row.id++; }],
    ['wrong row reference', result => { result.row.eventRef = 'different:41'; }],
    ['missing row', result => { delete result.row; }],
    ['wrong column', result => { result.column = 'name'; }],
    ['missing receipt', result => { delete result.receipt; }],
    ['wrong Case receipt', result => { result.receipt.analysisContext.caseId = 'other-case'; }],
    ['wrong analysis receipt', result => { result.receipt.analysisContext.analysisId = 'other-analysis'; }],
    ['wrong config receipt', result => { result.receipt.analysisContext.configRevision++; }],
    ['wrong visibility receipt', result => { result.receipt.analysisContext.visibilityRevision++; }],
    ['wrong source generation', result => { result.receipt.sourceGeneration++; }],
    ['missing source generation', result => { delete result.receipt.sourceGeneration; }],
    ['unexpected Case key', result => { result.receipt.caseKey = 'other-evidence'; }],
    ['unexpected Case content token', result => { result.receipt.caseContentToken = 'other-evidence'; }],
    ['missing dataset content token', result => { delete result.receipt.caseContentToken; }],
    ['missing catalog signature', result => { delete result.receipt.catalogSignature; }],
    ['short catalog signature', result => { result.receipt.catalogSignature = 'a'.repeat(63); }],
    ['non-hex catalog signature', result => { result.receipt.catalogSignature = 'g'.repeat(64); }],
    ['array catalog signature', result => { result.receipt.catalogSignature = ['a'.repeat(64)]; }],
    ['missing catalog epoch', result => { delete result.receipt.catalogEpoch; }],
    ['negative catalog epoch', result => { result.receipt.catalogEpoch = -1; }],
    ['fractional catalog epoch', result => { result.receipt.catalogEpoch = 0.5; }],
    ['unsafe catalog epoch', result => { result.receipt.catalogEpoch = Number.MAX_SAFE_INTEGER + 1; }],
    ['string catalog epoch', result => { result.receipt.catalogEpoch = '4'; }],
    ['missing canonical text', result => { delete result.canonicalText; }],
    ['typed value instead of text', result => { result.canonicalText = 1; }],
    ['unsupported value type', result => { result.valueType = 'date'; }],
    ['present null type', result => { result.valueType = 'null'; }],
    ['unknown presence', result => { result.presence = 'preview'; }],
    ['missing with value type', result => { result.presence = 'missing'; result.canonicalText = null; }],
    ['missing with text', result => { result.presence = 'missing'; result.valueType = null; }],
    ['null with wrong type', result => { result.presence = 'null'; result.canonicalText = 'null'; }],
    ['dynamic null without null text', result => { result.presence = 'null'; result.valueType = 'null'; result.canonicalText = null; }],
  ];
  for (const [name, mutate] of cases) await t.test(name, async () => {
    const h = harness();
    h.hooks.exact = call => { const result = h.exactResult(call); mutate(result); return result; };
    assert.equal(await h.fields.filter(h.capture()), false);
    noValueEffects(h, name);
    assert.equal(h.calls.notices.at(-1).level, 'err');
    assert.equal(h.calls.focus.length, 1, 'an invalid response restores the still-current anchor');
  });
});

test('Case receipts reject another snapshot, source-backed receipts and absent immutable evidence tokens', async t => {
  for (const [name, mutate] of [
    ['another key', result => { result.receipt.caseKey = 'replacement-key'; }],
    ['absent key', result => { result.receipt.caseKey = null; }],
    ['source generation in Case receipt', result => { result.receipt.sourceGeneration = 7; }],
    ['different immutable content token', result => { result.receipt.caseContentToken = 'another-evidence-token'; }],
    ['absent content token', result => { delete result.receipt.caseContentToken; }],
    ['null content token', result => { result.receipt.caseContentToken = null; }],
    ['empty content token', result => { result.receipt.caseContentToken = ''; }],
    ['non-string content token', result => { result.receipt.caseContentToken = ['captured-evidence-token']; }],
    ['oversized content token', result => { result.receipt.caseContentToken = 'x'.repeat(129); }],
    ['oversized UTF-8 content token', result => { result.receipt.caseContentToken = 'é'.repeat(65); }],
  ]) await t.test(name, async () => {
    const h = harness({ scope: 'case' });
    h.hooks.exact = call => { const result = h.exactResult(call); mutate(result); return result; };
    assert.equal(await h.fields.copy(h.capture()), false);
    noValueEffects(h, name);
    assert.match(h.calls.notices.at(-1).message, /não corresponde ao registro e contexto/);
  });
});

test('timestamp-null cannot accidentally become an exact filter for the string null', async () => {
  const h = harness();
  h.hooks.exact = call => h.exactResult(call, { presence: 'null', valueType: 'null', canonicalText: 'null' });
  assert.equal(await h.fields.filter(h.capture('timestamp')), false);
  noValueEffects(h, 'incorrect timestamp null');
});

test('valid zero and safe-integer receipt boundaries remain usable', async () => {
  for (const catalogEpoch of [0, Number.MAX_SAFE_INTEGER]) {
    const h = harness();
    h.state.sourcePublication.generation = 0;
    h.state.cases.cases[0].analysisContext.configRevision = 0;
    h.state.cases.cases[0].analysisContext.visibilityRevision = 0;
    h.event.id = 0;
    h.hooks.exact = call => {
      const result = h.exactResult(call);
      result.receipt.catalogEpoch = catalogEpoch;
      result.receipt.catalogSignature = 'ABCDEF01'.repeat(8);
      return result;
    };
    assert.equal(await h.fields.filter(h.capture()), true);
    assert.equal(h.calls.drafts.length, 1);
    assert.equal(h.calls.notices.length, 0);
  }
});

test('unsafe or missing input handles never reach native preparation', async () => {
  for (const mutate of [
    event => { event.id = -1; }, event => { event.id = 0.5; },
    event => { event.id = Number.MAX_SAFE_INTEGER + 1; }, event => { event.id = '41'; },
    event => { delete event.id; }, event => { event.event_ref = ''; },
    event => { delete event.event_ref; }, event => { event.event_ref = 41; },
  ]) {
    const h = harness(); mutate(h.event);
    assert.equal(await h.fields.filter(h.capture()), false);
    assert.equal(h.calls.prepare.length, 0);
    assert.equal(h.calls.exact.length, 0);
    noValueEffects(h, 'invalid input handle');
    assert.match(h.calls.notices.at(-1).message, /referência estável/);
  }
});

test('captured row identity and column do not follow later mutations of the displayed event', async () => {
  const h = harness(), action = h.capture('float');
  h.event.id = 90; h.event.event_ref = 'new:90'; h.event.fields.float = 100;
  h.hooks.exact = call => h.exactResult(call, { valueType: 'number', canonicalText: '1.0' });
  assert.equal(await h.fields.filter(action), true);
  assert.equal(h.calls.exact[0].args.id, 41);
  assert.equal(h.calls.exact[0].args.eventRef, 'source-a:41');
  assert.equal(h.calls.exact[0].args.column, 'float');
  assert.equal(h.calls.drafts[0][1], '1.0');
});

test('native and preparation failures never substitute a JavaScript value', async t => {
  for (const stage of ['save', 'definitions', 'Case sync', 'native', 'null reply']) await t.test(stage, async () => {
    const h = harness({ scope: 'case', legacy: stage === 'save' });
    const fail = () => { throw Error('PROJECTED_FIELD_LIMIT: exact native text is unavailable'); };
    if (stage === 'save') h.hooks.save = async () => false;
    if (stage === 'definitions') h.hooks.definitions = fail;
    if (stage === 'Case sync') h.hooks.sync = fail;
    if (stage === 'native') h.hooks.exact = fail;
    if (stage === 'null reply') h.hooks.exact = () => null;
    assert.equal(await h.fields.filter(h.capture('float'), { apply: true }), false);
    noValueEffects(h, stage);
    assert.equal(h.calls.notices.at(-1).level, 'err');
    if (stage !== 'native' && stage !== 'null reply') assert.equal(h.calls.exact.length, 0);
  });
});

test('historical strings, booleans, null and missing use only preserved evidence', async () => {
  for (const [value, text, op] of [
    ['  historic\n', '  historic\n', 'equals_exact'], ['', '', 'equals_exact'],
    [true, 'true', 'equals_exact'], [false, 'false', 'equals_exact'],
    [null, 'null', 'equals_exact'], [undefined, null, 'empty'],
  ]) {
    const h = harness(); h.event.fields.value = value;
    assert.equal(await h.fields.filter(h.capture('value', { historical: true })), true);
    assert.equal(h.calls.drafts[0][1], text);
    assert.equal(h.calls.drafts[0][3], op);
    assert.equal(await h.fields.copy(h.capture('value', { historical: true })), text !== null);
    assert.deepEqual(h.calls.copies, text === null ? [] : [text]);
    assert.equal(h.calls.prepare.length, 0);
    assert.equal(h.calls.exact.length, 0, 'historical evidence never borrows the live row at the same id');
  }
  const h = harness(); h.event.timestamp = null;
  assert.equal(await h.fields.filter(h.capture('timestamp', { historical: true })), true);
  assert.equal(h.calls.drafts[0][1], null);
  assert.equal(h.calls.drafts[0][3], 'empty');
});

test('historical numeric and structured values are refused without a live fallback', async () => {
  for (const value of [0, 1, 1.5, 18446744073709551615, [], [1], {}, { 10: 'tenth', 2: 'second' }]) {
    const h = harness(); h.event.fields.value = value;
    for (const invoke of [action => h.fields.filter(action), action => h.fields.copy(action)]) {
      assert.equal(await invoke(h.capture('value', { historical: true })), false);
      assert.match(h.calls.notices.at(-1).message, /evidência histórica/);
    }
    noValueEffects(h, 'unsafe historical value');
    assert.equal(h.calls.exact.length, 0);
    assert.equal(h.calls.prepare.length, 0);
  }
});

test('selected snippets, IPs, URLs and role members stay literal under owner and detail guards', async () => {
  for (const snippet of ['  "10":"tenth", "2":1.0\n', '192.0.2.17', 'https://example.test/path?a=1&b=2', 'administrator']) {
    const h = harness(), original = h.capture('object', { guard: () => h.live.detailCurrent });
    const selected = h.fields.selection(original, snippet);
    assert.equal(original.historical, false, 'selection does not alter the full-field action');
    assert.equal(selected.owner, original.owner);
    assert.equal(await h.fields.filter(selected, { op: 'contains' }), true);
    assert.equal(h.calls.drafts[0][1], snippet);
    assert.equal(h.calls.drafts[0][3], 'contains');
    assert.equal(await h.fields.copy(selected), true);
    assert.deepEqual(h.calls.copies, [snippet]);
    assert.equal(h.calls.exact.length, 0);
    assert.equal(h.calls.prepare.length, 0);
    h.live.detailCurrent = false;
    assert.equal(await h.fields.filter(selected), false);
    assert.equal(h.calls.drafts.length, 1, 'selection cannot reopen a superseded detail');
    h.live.detailCurrent = true;
    h.state.sourcePublication.generation++;
    assert.equal(await h.fields.copy(selected), false);
    assert.deepEqual(h.calls.copies, [snippet], 'a literal selection still belongs to its captured source');
  }
});

test('Escape cancels pending work, restores current focus and consumes only a handled Escape', async t => {
  for (const stage of ['prepare', 'Case sync', 'native reply']) await t.test(stage, async () => {
    const h = harness({ scope: 'case' }), gate = deferred();
    assert.equal(h.listeners.get('keydown').capture, true);
    assert.deepEqual(h.escape(), { prevented: false, stopped: false });
    if (stage === 'prepare') h.hooks.definitions = () => gate.promise;
    if (stage === 'Case sync') h.hooks.sync = () => gate.promise;
    if (stage === 'native reply') h.hooks.exact = () => gate.promise;
    const promise = h.fields.filter(h.capture());
    const target = stage === 'prepare' ? h.calls.definitions : stage === 'Case sync' ? h.calls.sync : h.calls.exact;
    await reached(() => target.length === 1, `reached ${stage}`);
    assert.deepEqual(h.escape('Enter'), { prevented: false, stopped: false });
    assert.deepEqual(h.escape(), { prevented: true, stopped: true });
    assert.deepEqual(h.calls.cancels, ['canonical-field-action']);
    assert.equal(h.calls.focus.length, 1);
    assert.equal(h.calls.focus[0].anchor, h.anchor);
    assert.equal(h.calls.focus[0].options.preventScroll, true);
    gate.resolve(stage === 'prepare' ? [] : stage === 'native reply' ? h.exactResult(h.calls.exact[0]) : undefined);
    assert.equal(await promise, false);
    noValueEffects(h, `Escape during ${stage}`);
    assert.equal(h.calls.notices.length, 0, 'canceled replies do not create error toasts');
    assert.deepEqual(h.escape(), { prevented: false, stopped: false });
  });
});

test('cancel after detail replacement never restores an obsolete anchor', async () => {
  const h = harness(), gate = deferred();
  h.hooks.exact = () => gate.promise;
  const promise = h.fields.copy(h.capture('value', { guard: () => h.live.detailCurrent }));
  await reached(() => h.calls.exact.length === 1, 'copy request started');
  h.live.detailCurrent = false;
  h.fields.cancel({ focus: true });
  gate.reject(Error('late canceled request failed'));
  assert.equal(await promise, false);
  assert.equal(h.calls.focus.length, 0);
  assert.equal(h.calls.notices.length, 0);
  noValueEffects(h, 'obsolete detail cancellation');
});

test('replacement requests discard both late success and late failure without changing the newest action', async () => {
  for (const oldFirst of [false, true]) for (const failure of [false, true]) {
    const h = harness(), first = deferred(), second = deferred();
    h.hooks.exact = () => h.calls.exact.length === 1 ? first.promise : second.promise;
    const old = h.fields.copy(h.capture('float'));
    await reached(() => h.calls.exact.length === 1, 'old request started');
    const latest = h.fields.filter(h.capture('unsafe'));
    await reached(() => h.calls.exact.length === 2, 'replacement request started');
    const settleOld = async () => {
      if (failure) first.reject(Error('late old failure'));
      else first.resolve(h.exactResult(h.calls.exact[0], { valueType: 'number', canonicalText: '1.0' }));
      assert.equal(await old, false);
    };
    if (oldFirst) {
      await settleOld();
      noValueEffects(h, 'old response while replacement is still pending');
    }
    second.resolve(h.exactResult(h.calls.exact[1], { valueType: 'number', canonicalText: '18446744073709551615' }));
    assert.equal(await latest, true);
    if (!oldFirst) await settleOld();
    assert.deepEqual(h.calls.cancels, ['canonical-field-action']);
    assert.equal(h.calls.drafts.length, 1);
    assert.equal(h.calls.drafts[0][0], 'unsafe');
    assert.equal(h.calls.drafts[0][1], '18446744073709551615');
    assert.equal(h.calls.copies.length, 0);
    assert.equal(h.calls.notices.length, 0);
    assert.equal(h.calls.focus.length, 0);
  }
});

test('clipboard failure remains a failure, and an old clipboard completion cannot steal focus', async () => {
  const failed = harness(); failed.hooks.clipboard = () => { throw Error('Clipboard permission unavailable'); };
  assert.equal(await failed.fields.copy(failed.capture()), false);
  assert.match(failed.calls.notices.at(-1).message, /Clipboard permission/);
  assert.equal(failed.calls.notices.some(notice => notice.level === 'ok'), false);
  const h = harness(), gate = deferred(); h.hooks.clipboard = () => gate.promise;
  const promise = h.fields.copy(h.capture());
  await reached(() => h.calls.copies.length === 1, 'clipboard write started');
  h.state.sourcePublication.generation++;
  gate.resolve();
  assert.equal(await promise, true, 'the already-issued clipboard write completed');
  assert.equal(h.calls.notices.length, 0, 'completion in a replaced context is quiet');
  assert.equal(h.calls.focus.length, 0);
});
