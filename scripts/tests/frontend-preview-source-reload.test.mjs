import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const plain = value => JSON.parse(JSON.stringify(value));
const java = JSON.parse(readFileSync(new URL('../preview/fixtures/java-traces.json', import.meta.url), 'utf8'));
const paths = ['C:\\mock\\mock.jsonl', 'C:\\mock\\firewall.log'];

async function fixture(options = {}) {
  const storage = new Map();
  const context = vm.createContext({
    window: { addEventListener() {}, ...options }, structuredClone, btoa, atob, TextEncoder,
    localStorage: { getItem: key => storage.get(key) || null, setItem: (key, value) => storage.set(key, value) },
    // Exercise the public transport and publication path without visual delays.
    setTimeout: callback => { queueMicrotask(callback); return 0; },
  });
  for (const file of ['mock-journeys.js', 'mock-tauri.js']) {
    vm.runInContext(readFileSync(new URL(`../preview/${file}`, import.meta.url), 'utf8'), context, { filename: file });
  }
  const invoke = context.window.__TAURI__.core.invoke;
  const store = await invoke('cases_load');
  const analysisContext = Object.fromEntries(['caseId', 'analysisId', 'configRevision', 'visibilityRevision']
    .map(key => [key, store.cases[0].analysisContext[key]]));
  const request = (command, args = {}) => invoke(command, { ...args, analysisContext });
  const rows = async () => {
    const result = [];
    const snapshot = await request('source_snapshot');
    for (let offset = 0; offset < snapshot.count; offset += 2000) {
      const page = await request('query_page', { filters: [], offset, limit: 2000, sourceGeneration: snapshot.generation });
      result.push(...plain(page.rows));
    }
    return result;
  };
  return { window: context.window, request, rows };
}

async function verifyCombinedSource(f, baseRows, baseColumns) {
  const snapshot = await f.request('source_snapshot');
  assert.equal(snapshot.count, 6000, 'reload restores the 4,000 application rows before appending the 2,000 firewall rows');
  assert.deepEqual(plain(snapshot.columns), [...baseColumns, 'regra']);
  const rows = await f.rows();
  assert.equal(rows.length, 6000);
  assert.equal(new Set(rows.map(row => row.id)).size, 6000);
  assert.equal(new Set(rows.map(row => row.event_ref)).size, 6000);
  assert.equal(rows.filter(row => row.source === 'Firewall').length, 2000);
  const byId = new Map(rows.map(row => [row.id, row]));
  for (const row of baseRows) assert.deepEqual(byId.get(row.id), row, `reload preserves application fixture ${row.id}`);
  assert.equal(await f.request('count_filtered', { filters: [], sourceGeneration: snapshot.generation }), 6000);
  return snapshot;
}

test('isolated Case startup clears then loads all preview sources, including repeated reopen and bundle paths', async () => {
  const f = await fixture();
  const baseRows = await f.rows(), baseColumns = plain((await f.request('source_snapshot')).columns);
  assert.equal(baseRows.length, 4000);
  for (const command of ['load_files', 'load_files', 'load_bundle']) {
    const previous = await f.request('source_snapshot');
    await f.request('clear_events', { operationId: 'case-switch-clear' });
    const empty = await f.request('source_snapshot');
    assert.equal(empty.count, 0);
    assert.deepEqual(plain(empty.columns), []);
    assert.equal(empty.generation, previous.generation + 1);
    const args = command === 'load_bundle' ? { members: paths.map(path => ({ kind: 'file', path })) } : { paths };
    const loaded = await f.request(command, { ...args, operationId: 'case-source-reload' });
    assert.equal(loaded.count, 6000);
    assert.equal(loaded.source_desc, 'mock.jsonl (preview) + firewall.log (preview)');
    const snapshot = await verifyCombinedSource(f, baseRows, baseColumns);
    assert.equal(snapshot.generation, empty.generation + 1, 'internal restoration creates one source publication');
    assert.equal(loaded.publication.generation, snapshot.generation);
    await f.request('load_files', { paths });
    await verifyCombinedSource(f, baseRows, baseColumns);
  }
  await f.request('clear_events');
  const single = await f.request('load_files', { paths: paths.slice(0, 1) });
  assert.equal(single.count, 4000);
  assert.deepEqual(plain(single.columns), baseColumns);
  await f.request('load_files', { paths: paths.slice(1), merge: true });
  await verifyCombinedSource(f, baseRows, baseColumns);
});

test('cleared multi-source reload retains canonical, Java, rare-field and journey fixtures', async () => {
  const f = await fixture({ __mockCanonicalFieldsEnabled: true, __mockRareDerivedFieldsEnabled: true,
    __mockJavaFixturesEnabled: true, __mockJavaFixtures: structuredClone(java.cases) });
  const baseRows = await f.rows(), baseColumns = plain((await f.request('source_snapshot')).columns);
  await f.request('clear_events');
  await f.request('load_files', { paths });
  const snapshot = await verifyCombinedSource(f, baseRows, baseColumns);
  const canonical = f.window.__mockCanonicalFixture;
  for (const [column, text] of Object.entries(canonical.values)) {
    const result = await f.request('analysis_field_text', { id: canonical.id, eventRef: canonical.eventRef,
      column, sourceGeneration: snapshot.generation });
    assert.equal(result.canonicalText, text);
  }
  for (const [index, row] of f.window.__mockJavaFixtureRows.entries()) {
    const result = await f.request('java_trace_detail', { id: row.id, eventRef: row.eventRef, sourceGeneration: snapshot.generation });
    assert.equal(result.state, 'available');
    assert.deepEqual(plain(result.trace), java.cases[index].detail.trace);
  }
  const rare = f.window.__mockRareDerivedFixture;
  const detail = await f.request('event_detail', { id: rare.id, eventRef: rare.eventRef, sourceGeneration: snapshot.generation });
  assert.deepEqual(JSON.parse(atob(detail.fields.mock_payload_b64)), { rare: { flag: true, latency: 42 } });
  const journeys = await f.request('journey_index', { field: 'correlation_id', filters: [], limit: 100,
    sourceGeneration: snapshot.generation });
  assert.equal(journeys.total, 30);
  assert.ok(journeys.groups.every(group => group.count === 4));
});
