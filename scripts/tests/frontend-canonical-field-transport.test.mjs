import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const mockSource = readFileSync(new URL('../preview/mock-tauri.js', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const timestampText = value => new Date(value).toISOString().replace(/\.000Z$/, '+00:00').replace(/Z$/, '+00:00');

function ordinaryTransport() {
  const storage = new Map();
  const context = vm.createContext({
    // No canonical, exclusion or security fixture may supply the row identity.
    window: { addEventListener() {} },
    localStorage: { getItem: key => storage.get(key) || null, setItem: (key, value) => storage.set(key, value) },
    structuredClone, btoa, atob, TextEncoder,
    // Keep the real load/publication path; skip only visual progress delays.
    setTimeout: callback => setTimeout(callback, 0),
  });
  vm.runInContext(mockSource, context, { filename: 'mock-tauri.js' });
  return context.window.__TAURI__.core.invoke;
}

test('ordinary preview rows preserve canonical timestamp identity before and after appending a source', async () => {
  const invoke = ordinaryTransport();
  const store = await invoke('cases_load'), snapshot = store.cases[0].analysisContext;
  const authenticationRows = store.cases[0].items[0].rows;
  const burst = authenticationRows.filter(row => row.id >= 90000);
  assert.equal(burst.length, 4);
  for (const row of burst) assert.equal(row.event_ref, `preview:${row.id}`, 'seeded authentication rows must override the copied source reference');
  assert.equal(new Set(authenticationRows.map(row => row.event_ref)).size, authenticationRows.length, 'seeded rows have independent references');
  const analysisContext = Object.fromEntries(['caseId', 'analysisId', 'configRevision', 'visibilityRevision'].map(key => [key, snapshot[key]]));
  const pageFor = sourceGeneration => invoke('query_page', { filters: [], limit: 100, analysisContext, sourceGeneration });
  const exactTimestamp = (row, sourceGeneration) => invoke('analysis_field_text', {
    id: row.id, eventRef: row.event_ref, column: 'timestamp', analysisContext, sourceGeneration,
  });
  const verifyReportedRow = async sourceGeneration => {
    const page = await invoke('query_page', { filters: [{ column: 'id', op: 'equals_exact', value: '1331', value2: null }],
      limit: 100, analysisContext, sourceGeneration });
    assert.equal(page.rows.length, 1);
    const row = page.rows[0];
    assert.equal(row.id, 1331); assert.equal(row.event_ref, 'preview:1331', 'the actual responsiveness failure row has native identity');
    assert.equal((await exactTimestamp(row, sourceGeneration)).canonicalText, timestampText(row.timestamp));
  };
  const verifyPage = async (page, sourceGeneration) => {
    assert.equal(page.rows.length, 100);
    for (const row of page.rows) {
      assert.equal(typeof row.event_ref, 'string', `ordinary returned row ${row.id} needs a stable reference before any field action`);
      assert.equal(row.event_ref, `preview:${row.id}`);
    }
    assert.equal(new Set(page.rows.map(row => row.event_ref)).size, page.rows.length);
    const row = page.rows[0], exact = await exactTimestamp(row, sourceGeneration);
    assert.equal(exact.canonicalText, timestampText(row.timestamp));
    assert.equal(exact.presence, 'present'); assert.equal(exact.valueType, 'string');
    assert.deepEqual(plain(exact.row), { id: row.id, eventRef: row.event_ref });
    assert.deepEqual(plain(exact.receipt.analysisContext), analysisContext);
    assert.equal(exact.receipt.sourceGeneration, sourceGeneration);
    assert.equal(exact.receipt.caseKey, null); assert.equal(exact.receipt.caseContentToken, null);
    const detail = await invoke('event_detail', { id: row.id, analysisContext, sourceGeneration });
    assert.equal(detail.event_ref, row.event_ref, 'table and detail share the same native row identity');
    const repeated = await pageFor(sourceGeneration);
    assert.deepEqual(plain(repeated.rows.map(item => [item.id, item.event_ref])), plain(page.rows.map(item => [item.id, item.event_ref])));
    return exact;
  };

  const before = await invoke('source_snapshot');
  assert.equal(before.count, 4000);
  assert.equal(before.columns.includes('native_float'), false, 'the canonical test-only fixture stays disabled');
  const initialPage = await pageFor(before.generation);
  const initialExact = await verifyPage(initialPage, before.generation);
  await verifyReportedRow(before.generation);

  await invoke('load_files', { paths: ['mock.jsonl', 'firewall.log'], operationId: 'ordinary-source-append', analysisContext });
  const after = await invoke('source_snapshot');
  assert.equal(after.count, 6000); assert.equal(after.generation, before.generation + 1);
  const appendedPage = await pageFor(after.generation);
  assert.ok(appendedPage.rows.some(row => row.source === 'Firewall'), 'appended ordinary rows are exercised');
  await verifyPage(appendedPage, after.generation);
  await verifyReportedRow(after.generation);
  const firewall = appendedPage.rows.find(row => row.source === 'Firewall');
  assert.equal((await exactTimestamp(firewall, after.generation)).canonicalText, timestampText(firewall.timestamp));
  await assert.rejects(exactTimestamp(initialPage.rows[0], before.generation), /SOURCE_GENERATION_CHANGED/);
  const initialInNewPublication = await exactTimestamp(initialPage.rows[0], after.generation);
  assert.deepEqual(plain(initialInNewPublication.row), plain(initialExact.row), 'appending preserves the existing source row reference');
  assert.equal(initialInNewPublication.canonicalText, initialExact.canonicalText);
});
