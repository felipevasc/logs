import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const native = readFileSync(new URL('../../src-tauri/src/lib.rs', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const workspace = readFileSync(new URL('../../src-tauri/src/workspace.rs', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const commands = ['query_page', 'count_filtered', 'stats_events', 'tree_aggs'];
const section = (source, start, end) => {
  const from = source.indexOf(start), to = source.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `Source section exists: ${start}`);
  return source.slice(from, to);
};
const transport = section(app, 'const caseTransport =', '// ------------------------------------------------------------------ helpers de espera');
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const reached = async predicate => { for (let i = 0; i < 50; i++) { if (predicate()) return; await Promise.resolve(); } assert.fail('Request did not reach the expected boundary'); };

function apiFixture(handler = () => ({})) {
  const calls = [], notices = [], timers = new Set(), activity = { count: 0, timer: null };
  let hides = 0;
  const context = vm.createContext({
    window: {}, state: {}, TextEncoder, explorerAnalytics: new Map(), activity, ACTIVITY_DELAY: 320,
    setTimeout: callback => { timers.add(callback); return callback; }, clearTimeout: timer => timers.delete(timer),
    activityShow() {}, activityHide() { hides++; }, toast: (...args) => notices.push(args),
    invoke: async (command, args) => { calls.push({ command, args }); return handler(command, args); },
  });
  vm.runInContext(transport, context);
  return { context, calls, notices, timers, activity, hides: () => hides };
}

// Source-level audit, not a substitute for executing native validation tests.
// The allowlist is safe only while each endpoint unconditionally invokes the
// same validator as validate_filters, first inside the admitted Case worker
// before any query/aggregation. Validation itself must obey global admission.
test('every preflight-exempt endpoint validates filters before native query work', () => {
  for (const command of commands) {
    const endpoint = section(native, `async fn ${command}(`, '\n}\n');
    assert.match(endpoint, /\n    offload_case(?:_interactive)?\([^\n]*move \|case_events\| \{\n        workspace::validate\(&filters\)\?;/, command);
    assert.ok(endpoint.indexOf('capture_case_async(') < endpoint.indexOf('offload_case'), `${command} pins and captures the Case before admitted execution`);
    assert.equal((endpoint.match(/workspace::validate\(&filters\)/g) || []).length, 1, `${command} validates exactly once`);
    assert.match(endpoint, /\.await\?;?$/, `${command} propagates the validation domain error`);
  }
  const offload = section(native, 'async fn offload_case_input', '\n}\n');
  assert.match(offload, /operations::run_with_token\(token, \|\|/);
  assert.match(offload, /analysis_runtime::with\(Some\(admitted.clone\(\)\)/);
  assert.ok(offload.indexOf('analysis_runtime::with(') < offload.indexOf('let result = f(case_events)'), 'execution retains the captured Case inside the admitted worker');
  const editorValidation = section(workspace, 'pub async fn validate_filters(', '\n}\n');
  assert.match(editorValidation, /case_editor_admission_async\([\s\S]*analysis_context\)\.await\?/);
  assert.match(editorValidation, /offload_admitted\(None, app, admitted, move \|\| validate\(&filters\)\)\.await\?/);
  const validator = section(workspace, 'pub fn validate(filters:', '\n#[tauri::command]');
  assert.match(validator, /crate::querylang::compile\(&f.value\)\?/);
  assert.match(validator, /crate::detections::ruleset\(\)\?\.find\(&f.value\)/);
  assert.match(validator, /crate::threats::matcher\(&f.value, None\)\?/);
  assert.match(validator, /crate::query_regex::compile\(&f.value/);
});

test('page and independent summaries send four unchanged requests without four redundant preflights', async () => {
  const response = { rows: [{ id: 1, event_ref: 'exact:1' }], total: null, hasMore: true, nextCursor: 'exact-cursor' };
  const f = apiFixture(() => response);
  const filters = [{ column: 'level', op: 'equals_exact', value: 'error' }];
  const args = { filters, analysisContext: { caseId: 'c', analysisId: 'a', configRevision: 2, visibilityRevision: 3 }, sourceGeneration: 7, operationId: 'page-owner', cursor: 'previous' };
  for (const command of commands) assert.equal(await f.context.api(command, args, { silent: true }), response);
  assert.deepEqual(f.calls.map(call => call.command), commands);
  assert.ok(f.calls.every(call => call.args === args && call.args.filters === filters));
});

test('unknown commands retain preflight and stop on its error; empty filters need no preflight', async () => {
  const filters = [{ column: 'message', op: 'regex', value: '[' }];
  const failure = new Error('Expressão inválida: invalid regex');
  const f = apiFixture(command => { if (command === 'validate_filters') throw failure; return {}; });
  await assert.rejects(f.context.api('future_filtered_command', { filters }, { silent: true }), error => error === failure);
  assert.deepEqual(f.calls.map(call => call.command), ['validate_filters']);
  assert.equal(f.calls[0].args.filters, filters);
  const g = apiFixture();
  await g.context.api('future_filtered_command', { filters: [] }, { silent: true });
  await g.context.api('future_filtered_command', { filters: [{ column: 'level', op: 'equals', value: 'info' }] }, { silent: true });
  assert.deepEqual(g.calls.map(call => call.command), ['future_filtered_command', 'validate_filters', 'future_filtered_command']);
});

test('native validation errors propagate unchanged once and always settle activity', async () => {
  for (const command of commands) for (const silent of [false, true]) {
    const failure = new Error('Regra de detecção não encontrada: changed-rule.');
    const f = apiFixture(() => { throw failure; });
    await assert.rejects(f.context.api(command, { filters: [{ column: '_all', op: 'detection', value: 'changed-rule' }] }, { silent }), error => error === failure);
    assert.deepEqual(f.calls.map(call => call.command), [command]);
    assert.deepEqual(f.notices, silent ? [] : [[String(failure), 'err']]);
    assert.equal(f.activity.count, 0); assert.equal(f.timers.size, 0); assert.equal(f.hides(), silent ? 0 : 1);
  }
});

test('identical filters never cache validation across mutable rule changes', async () => {
  for (const command of commands) for (const op of ['detection', 'threat_rule']) {
    let available = true, admissions = 0;
    const failure = new Error(`Rule removed: ${op}`);
    const f = apiFixture(() => { admissions++; if (!available) throw failure; return { ok: true }; });
    const args = { filters: [{ column: '_all', op, value: 'mutable-rule' }] };
    assert.equal((await f.context.api(command, args, { silent: true })).ok, true);
    available = false;
    await assert.rejects(f.context.api(command, args, { silent: true }), error => error === failure);
    available = true;
    assert.equal((await f.context.api(command, args, { silent: true })).ok, true);
    assert.equal(admissions, 3); assert.deepEqual(f.calls.map(call => call.command), [command, command, command]);
  }
});

test('cancellation prevents initial dispatch and dispatch after Case synchronization', async () => {
  for (const command of commands) {
    const args = { filters: [{ column: 'message', op: 'contains', value: 'needle' }], caseEvents: [{ id: 1 }] };
    const f = apiFixture();
    await assert.rejects(f.context.api(command, args, { silent: true, cancelled: () => true }), /Operação cancelada/);
    assert.equal(f.calls.length, 0);
    const gate = deferred(); let cancelled = false;
    const g = apiFixture(name => { assert.equal(name, 'case_sync'); return gate.promise; });
    const request = g.context.api(command, args, { silent: true, cancelled: () => cancelled });
    const rejected = assert.rejects(request, /Operação cancelada/);
    await reached(() => g.calls.length === 1); cancelled = true; gate.resolve({}); await rejected;
    assert.deepEqual(g.calls.map(call => call.command), ['case_sync']);
  }
});

test('Case cache-miss retry retains captured payload and receives fresh authoritative validation', async () => {
  for (const command of commands) {
    let attempts = 0;
    const failure = new Error('Rule changed before retry');
    const f = apiFixture(name => {
      if (name === 'case_sync') return {};
      if (++attempts === 1) throw Error('CASE_CACHE_MISS');
      throw failure;
    });
    const events = [{ id: 7, event_ref: 'captured:7', fields: { exact: '9007199254740993' } }];
    const filters = [{ column: '_all', op: 'threat_rule', value: 'changed-rule' }];
    const identity = { caseId: 'original', analysisId: 'analysis', configRevision: 2, visibilityRevision: 3 };
    const args = { caseEvents: events, analysisContext: identity, sourceGeneration: 7, operationId: 'original-owner', filters };
    await assert.rejects(f.context.api(command, args, { silent: true }), error => error === failure);
    assert.deepEqual(f.calls.map(call => call.command), ['case_sync', command, 'case_sync', command]);
    for (const call of f.calls) {
      assert.equal(call.args.analysisContext, identity); assert.equal(call.args.sourceGeneration, 7);
      if (call.command === 'case_sync') assert.equal(call.args.events, events);
      else { assert.equal(call.args.filters, filters); assert.equal(call.args.operationId, 'original-owner'); assert.equal(call.args.caseEvents, undefined); }
    }
    assert.equal(f.calls[1].args.caseKey, f.calls[3].args.caseKey);
  }
});

function columnsFixture(columns, rows, visibleCols = []) {
  const state = { columns, rows, visibleCols, total: 12 }, renders = []; let controls = 0;
  const context = vm.createContext({ state, fmtTs: value => value == null ? '' : String(value), eventComment: () => '',
    fillColumnControls() { controls++; }, renderTable: (...args) => renders.push(args) });
  vm.runInContext(section(app, 'function cellValue(', '// ------------------------------------------------------------------ tema'), context);
  return { context, state, renders, controls: () => controls };
}

test('automatic columns do not inspect or serialize hidden structured fields', () => {
  let serializations = 0, reads = 0;
  const fields = Object.fromEntries(Array.from({ length: 200 }, (_, index) => [`hidden_${index}`, { toJSON() { serializations++; return { text: 'x'.repeat(65536) }; } }]));
  const row = { timestamp: 1, level: 'Info', source: 'log', message: 'short', get fields() { reads++; return fields; } };
  const f = columnsFixture(['timestamp', 'level', 'source', 'message', ...Object.keys(fields)], [row]);
  f.context.autoVisibleCols();
  assert.equal(reads, 0); assert.equal(serializations, 0);
  assert.deepEqual(plain(f.state.visibleCols), ['timestamp', 'level', 'source', 'message']);
  assert.equal(f.controls(), 1); assert.equal(f.renders.length, 1);
  assert.equal(f.renders[0][0].rows, f.state.rows); assert.equal(f.renders[0][0].total, 12);
  assert.deepEqual(plain(f.renders[0][1]), { reuseRows: true });
});

test('automatic columns preserve preferred order, blank timestamp and existing fallbacks', () => {
  const scenarios = [
    { columns: ['message', 'source', 'level', 'timestamp'], rows: [{ timestamp: 1, level: 'warn', source: 's', message: 'm' }], expected: ['timestamp', 'level', 'source', 'message'] },
    { columns: ['timestamp', 'level', 'source', 'message'], rows: [{ timestamp: null, level: null, source: ' ', message: '' }], expected: ['timestamp'] },
    { columns: ['timestamp', 'message'], rows: [], expected: ['timestamp'] },
    { columns: ['hidden', 'message'], rows: [{ message: ' ', fields: { hidden: { value: 1 } } }], expected: ['message'] },
    { columns: ['hidden', 'other'], rows: [{ fields: { hidden: null, other: false } }], expected: ['hidden'] },
    { columns: ['other', 'hidden'], rows: [], expected: ['other'] },
    { columns: [], rows: [], expected: [] },
    { columns: ['source', 'level', 'message'], rows: [{ source: 0, level: false, message: {} }], expected: ['level', 'source', 'message'] },
    { columns: ['source', 'level', 'message'], rows: [{ source: '', level: '', message: '' }, { source: 's', level: 'info', message: 'm' }], expected: ['level', 'source', 'message'] },
  ];
  for (const { columns, rows, expected } of scenarios) {
    const f = columnsFixture(columns, rows); f.context.autoVisibleCols();
    assert.deepEqual(plain(f.state.visibleCols), expected);
  }
});

test('opening still respects saved artifact and restored column preferences', () => {
  const gate = app.match(/if \(!savedArtifact\?\.visibleCols\?\.length && !restoredVisiblePreferences\) autoVisibleCols\(\);/)?.[0];
  assert.ok(gate, 'Load only chooses automatic columns when neither preference exists');
  for (const saved of [undefined, { visibleCols: [] }, { visibleCols: ['hidden'] }]) for (const restored of [false, true]) {
    const f = columnsFixture(['timestamp', 'message', 'hidden'], [{ message: 'm' }], ['hidden']);
    Object.assign(f.context, { savedArtifact: saved, restoredVisiblePreferences: restored });
    vm.runInContext(gate, f.context);
    const keep = !!saved?.visibleCols?.length || restored;
    assert.deepEqual(plain(f.state.visibleCols), keep ? ['hidden'] : ['timestamp', 'message']);
    assert.equal(f.renders.length, keep ? 0 : 1);
  }
});
