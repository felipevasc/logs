import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import vm from 'node:vm';

const plain = value => JSON.parse(JSON.stringify(value));
const identity = value => Object.fromEntries(['caseId', 'analysisId', 'configRevision', 'visibilityRevision'].map(key => [key, value[key]]));
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const materializationStart = app.indexOf('function caseRecordKey(');
const materializationEnd = app.indexOf('function analyticsRequest(', materializationStart);
assert.ok(materializationStart >= 0 && materializationEnd > materializationStart);

async function fixture() {
  const storage = new Map();
  const context = vm.createContext({
    window: { addEventListener() {} }, crypto: webcrypto, TextEncoder, TextDecoder,
    structuredClone, btoa, atob, setTimeout: callback => { callback(); return 0; },
    localStorage: { getItem: key => storage.get(key) || null, setItem: (key, value) => storage.set(key, value) },
    state: { stationAnalyticsId: null }, activeCase: () => context.window.preservedCase,
  });
  for (const file of ['mock-field-transforms.js', 'mock-references.js', 'mock-tauri.js']) {
    vm.runInContext(readFileSync(new URL(`../preview/${file}`, import.meta.url), 'utf8'), context);
  }
  vm.runInContext(app.slice(materializationStart, materializationEnd), context);
  const invoke = context.window.__TAURI__.core.invoke;
  const store = await invoke('cases_load');
  let owner = identity(store.cases[0].analysisContext);
  const mutation = async (command, args) => {
    const result = await invoke(command, { ...args, analysisContext: owner });
    if (result.analysisContext) owner = identity(result.analysisContext);
    return result;
  };
  return {
    context, invoke, mutation,
    save: definition => mutation('save_derived_field', { rules: [], steps: [], ...definition }),
    remove: name => mutation('delete_derived_field', { name }),
    query: async rows => plain((await invoke('query_events', { caseEvents: rows, analysisContext: owner, filters: [], limit: 100 })).rows),
    materialize: rows => {
      context.window.preservedCase = { items: [{ artifactId: 'saved-artifact', rows }] };
      return plain(vm.runInContext('caseEventsCompute()', context));
    },
    persist: async rows => {
      store.cases[0].items = [{ id: 'captured-evidence', artifactId: 'saved-artifact', rows: plain(rows) }];
      await invoke('cases_save', { data: store });
    },
    saved: async () => plain((await invoke('cases_load')).cases[0].items[0].rows),
  };
}

const proof = {
  source: { version: 'fixture-content-v1', recordSpace: 'jsonl-v1', label: 'evidence.jsonl', eventRefPrefix: 'fixture-source' },
  locator: { byte_offset: 128 },
};
const sourceEvent = fields => ({
  id: 37, event_ref: 'fixture-source:128', parse_status: 'parsed', timestamp: 1234,
  source: 'original source', level: 'Informação', code: '200', name: 'Original', description: '',
  message: 'Original message', raw: 'Original raw record', evidence_provenance: plain(proof), fields,
});
const rule = template => [{ pattern: '(.*)', template }];

test('saved Case materialization recomputes A → B → reset without losing originals or evidence proof', async () => {
  const f = await fixture();
  const collisions = { native_null: null, native_false: false, native_zero: 0, native_empty: '', native_object: { kept: [null, false, 0] }, native_array: [0, false, null] };
  const base = sourceEvent({
    input: '%2Fapi', payload: btoa(JSON.stringify({ child: { enabled: false }, list: [0, null] })),
    invalid: '%', jwt: 'eyJhbGciOiJub25lIn0.eyJzdWIiOiJsb2NhbCJ9.',
    historical_unmarked: 'keep', ...collisions,
  });
  const baseBefore = plain(base);
  await f.save({ name: 'decoded', source: 'payload', steps: ['base64_decode', 'parse_json'] });
  for (const name of Object.keys(collisions)) await f.save({ name, source: 'input', rules: rule('A') });
  await f.save({ name: 'scalar_added', source: 'input', rules: rule('A') });
  await f.save({ name: 'bad', source: 'invalid', steps: ['url_decode'] });
  await f.save({ name: 'claims', source: 'jwt', steps: ['jwt_payload'] });
  const a = (await f.query([base]))[0];
  assert.deepEqual(base, baseBefore, 'the source record is unchanged');
  assert.equal(a.fields['decoded.child.enabled'], false);
  assert.equal(a.fields['decoded.list.0'], 0);
  assert.equal(a.fields['decoded.list.1'], null);
  for (const name of Object.keys(a.fields).filter(name => name === 'decoded' || name.startsWith('decoded.') || name === 'claims' || name.startsWith('claims.') || name === 'scalar_added')) {
    assert.deepEqual(a.derived_originals[name], { state: 'missing' }, `record generated parent or child ${name}`);
  }
  for (const [name, value] of Object.entries(collisions)) assert.deepEqual(a.derived_originals[name], { state: 'present', value });
  assert.deepEqual(a.derived_diagnostics.map(diagnostic => diagnostic.code), ['transform_error', 'jwt_signature_not_verified']);
  assert.deepEqual(a.evidence_provenance, proof);
  await f.persist([a]);
  const capturedA = await f.saved();
  const materializedA = f.materialize(capturedA);
  assert.equal(materializedA[0].id, 0, 'Case materialization assigns its own row IDs');
  for (const key of ['derived_originals', 'derived_diagnostics', 'evidence_provenance']) assert.deepEqual(materializedA[0][key], a[key], `materialization retains ${key}`);
  const repeatA = (await f.query(materializedA))[0];
  assert.deepEqual(repeatA.fields, a.fields, 'same configuration is repeatable after capture');
  assert.deepEqual(repeatA.derived_diagnostics, a.derived_diagnostics, 'diagnostics do not accumulate');

  await f.save({ name: 'decoded', source: 'input', steps: ['url_decode'] });
  for (const name of Object.keys(collisions)) await f.save({ name, source: 'input', rules: rule('B') });
  for (const name of ['scalar_added', 'bad', 'claims']) await f.remove(name);
  const materializedBefore = plain(materializedA);
  const b = (await f.query(materializedA))[0];
  assert.deepEqual(materializedA, materializedBefore, 'request-local overlays do not mutate materialized evidence');
  assert.equal(b.fields.decoded, '/api');
  assert.equal(Object.keys(b.fields).some(name => name.startsWith('decoded.') || name.startsWith('claims')), false);
  assert.equal(Object.hasOwn(b.fields, 'scalar_added'), false);
  assert.equal(Object.hasOwn(b, 'derived_diagnostics'), false, 'old errors and notices are cleared');
  for (const [name, value] of Object.entries(collisions)) {
    assert.equal(b.fields[name], 'B');
    assert.deepEqual(b.derived_originals[name], { state: 'present', value }, 'B remembers the original value, not A');
  }
  assert.deepEqual(b.evidence_provenance, proof);
  assert.deepEqual(await f.saved(), capturedA, 'saved A rows remain exact while configuration changes');
  await f.persist([b]);
  const capturedB = await f.saved();
  for (const name of ['decoded', ...Object.keys(collisions)]) await f.remove(name);
  for (const captured of [capturedA, capturedB]) {
    const reset = (await f.query(f.materialize(captured)))[0];
    assert.deepEqual(reset.fields, base.fields, 'empty configuration restores every original field and removes all marked outputs');
    assert.equal(Object.hasOwn(reset, 'derived_originals'), false);
    assert.equal(Object.hasOwn(reset, 'derived_diagnostics'), false);
    assert.deepEqual(reset.evidence_provenance, proof);
    assert.equal(reset.event_ref, base.event_ref);
    assert.equal(reset.raw, base.raw);
  }
  assert.deepEqual(await f.saved(), capturedB, 'reset does not rewrite captured B evidence');
});

test('typed parent and child collisions reject the whole overlay and clear on reset', async () => {
  const f = await fixture();
  await f.save({ name: 'decoded', source: 'payload', steps: ['parse_json'] });
  for (const collision of [{ decoded: null }, { 'decoded.child': false }]) {
    const row = sourceEvent({ payload: '{"child":1,"extra":2}', ...collision });
    const before = plain(row), result = (await f.query([row]))[0];
    assert.deepEqual(result.fields, row.fields, 'typed collision never leaves a partial parent/child write');
    assert.equal(result.derived_diagnostics[0].code, 'target_conflict');
    assert.equal(Object.hasOwn(result, 'derived_originals'), false);
    assert.deepEqual(result.evidence_provenance, proof);
    assert.deepEqual(row, before);
  }
  const conflicting = sourceEvent({ payload: '{"child":1}', 'decoded.child': false });
  const captured = await f.query([conflicting]);
  await f.remove('decoded');
  const reset = (await f.query(f.materialize(captured)))[0];
  assert.deepEqual(reset.fields, conflicting.fields);
  assert.equal(Object.hasOwn(reset, 'derived_diagnostics'), false);
});

test('reference overlays record expanded outputs and retain proof through capture and reset', async () => {
  const f = await fixture(), path = '/fixtures/overlay-reference.jsonl';
  f.context.window.__mockReferenceFiles = { [path]: '{"key":"match","value":{"child":false,"list":[0,null]}}\n' };
  const inspection = await f.mutation('reference_inspect', { path });
  const imported = await f.mutation('reference_import', { path, inspection, name: 'Overlay reference', keyColumns: ['key'] });
  await f.mutation('reference_save_lookup', { name: 'looked_up', lookup: {
    schemaVersion: 1, referenceId: imported.reference.id, keys: [{ referenceColumn: 'key', sourceField: 'key' }], valueColumn: 'value',
  } });
  const original = sourceEvent({ key: 'match', historical_unmarked: 'keep' });
  const captured = (await f.query([original]))[0];
  for (const name of ['looked_up', 'looked_up.child', 'looked_up.list', 'looked_up.list.0', 'looked_up.list.1']) {
    assert.deepEqual(captured.derived_originals[name], { state: 'missing' });
  }
  const repeat = (await f.query(f.materialize([captured])))[0];
  assert.deepEqual(repeat.fields, captured.fields);
  assert.deepEqual(repeat.evidence_provenance, proof);
  const collision = sourceEvent({ key: 'match', 'looked_up.child': null });
  const rejected = (await f.query([collision]))[0];
  assert.deepEqual(rejected.fields, collision.fields);
  assert.equal(rejected.derived_diagnostics[0].code, 'target_conflict');
  await f.remove('looked_up');
  const restored = (await f.query(f.materialize([captured])))[0];
  assert.deepEqual(restored.fields, original.fields);
  assert.deepEqual(restored.evidence_provenance, proof);
  assert.equal(Object.hasOwn(restored, 'derived_originals'), false);
});
