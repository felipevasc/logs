import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../preview/mock-tauri.js', import.meta.url), 'utf8');
const identity = value => Object.fromEntries(['caseId','analysisId','configRevision','visibilityRevision'].map(key => [key, value[key]]));
const plain = value => JSON.parse(JSON.stringify(value));
function fixture() {
  const storage = new Map();
  const context = vm.createContext({ window: { addEventListener() {} }, structuredClone, btoa, atob, TextEncoder,
    localStorage: { getItem: key => storage.get(key) || null, setItem: (key,value) => storage.set(key,value) },
    setTimeout: callback => setTimeout(callback, 0) });
  vm.runInContext(source, context);
  return context.window.__TAURI__.core.invoke;
}
test('preview Case interpretation preserves separate catalog, same-path timestamps and custom formats with CAS receipts', async () => {
  const invoke = fixture(), store = await invoke('cases_load');
  store.cases.push({ id: 'case-b', name: 'B', items: [], artifacts: [], workspace: {} });
  await invoke('cases_save', { data: store });
  let a = identity(await invoke('analysis_context_snapshot', { caseId: store.cases[0].id }));
  const b = identity(await invoke('analysis_context_snapshot', { caseId: 'case-b' }));
  const savedCodes = await invoke('save_codes', { analysisContext: a, text: '{"A_only":{"code":"name"}}' }); a = identity(savedCodes.analysisContext);
  assert.equal(JSON.parse(await invoke('get_codes', { analysisContext: a })).A_only.code, 'name');
  assert.deepEqual(JSON.parse(await invoke('get_codes', { analysisContext: b })), {});
  const savedFormat = await invoke('save_custom_format', { analysisContext: a, name: 'A format', kind: 'regex', pattern: '(.*)', separator: ',', fields: ['message'] }); a = identity(savedFormat.analysisContext);
  assert.ok((await invoke('list_formats', { analysisContext: a })).some(format => format.id === 'custom:A format'));
  assert.equal((await invoke('list_formats', { analysisContext: b })).some(format => format.id === 'custom:A format'), false);
  const savedTs = await invoke('set_ts_config', { analysisContext: a, path: 'same.log', config: { sources: ['message'], format: 'epoch_ms' } }); a = identity(savedTs.analysisContext);
  assert.equal(savedTs.publication.analysisContext.caseId, a.caseId); assert.equal(savedTs.publication.analysisContext.configRevision, a.configRevision);
  assert.equal((await invoke('get_ts_config', { analysisContext: a, path: 'same.log' })).format, 'epoch_ms');
  assert.equal(await invoke('get_ts_config', { analysisContext: b, path: 'same.log' }), null);
  assert.deepEqual(plain((await invoke('source_snapshot')).analysisContext), a);
  await assert.rejects(invoke('save_codes', { analysisContext: { ...a, configRevision: a.configRevision - 1 }, text: '{}' }), /ANALYSIS_CONTEXT_CHANGED/);
  await assert.rejects(invoke('get_codes', {}), /ANALYSIS_CONTEXT_CHANGED/);
  await assert.rejects(invoke('list_formats', {}), /ANALYSIS_CONTEXT_CHANGED/);
});
test('preview source clear and reopen retain the immutable original row references', async () => {
  const invoke = fixture(), store = await invoke('cases_load'), owner = identity(store.cases[0].analysisContext);
  const before = await invoke('source_snapshot'), rows = await invoke('query_page', { filters: [], limit: 5, analysisContext: owner, sourceGeneration: before.generation });
  await invoke('clear_events', { analysisContext: owner }); assert.equal((await invoke('source_snapshot')).count, 0);
  const reopened = await invoke('load_file', { analysisContext: owner, path: 'original.jsonl', format: 'auto' });
  const after = await invoke('query_page', { filters: [], limit: 5, analysisContext: owner, sourceGeneration: reopened.publication.generation });
  assert.equal(reopened.count, before.count); assert.deepEqual(plain(after.rows.map(row => row.event_ref)), plain(rows.rows.map(row => row.event_ref)));
});
test('preview filter editor validation requires a current Case identity and retains syntax errors', async () => {
  const invoke = fixture(), store = await invoke('cases_load'), owner = identity(store.cases[0].analysisContext);
  assert.equal(await invoke('validate_filters', { analysisContext: owner, filters: [{ column: 'message', op: 'regex', value: 'valid' }] }), null);
  await assert.rejects(invoke('validate_filters', { filters: [] }), /ANALYSIS_CONTEXT_CHANGED/);
  await assert.rejects(invoke('validate_filters', { analysisContext: { ...owner, configRevision: owner.configRevision + 1 }, filters: [] }), /ANALYSIS_CONTEXT_CHANGED/);
  await assert.rejects(invoke('validate_filters', { analysisContext: owner, filters: [{ column: 'message', op: 'regex', value: '[' }] }), /regular expression|unterminated/i);
});
