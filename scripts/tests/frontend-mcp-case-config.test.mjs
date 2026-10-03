import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const code = source.slice(source.indexOf('async function handleMcpStateChanged('), source.indexOf('// a fonte de eventos mudou'));
const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };
function fixture() {
  const a = { id: 'a' }, b = { id: 'b' }, snapshot = deferred(), calls = []; let active = a;
  const context = vm.createContext({ state: { loaded: false }, activeCase: () => active, caseEventsCache: {},
    window: { AnalysisContexts: { capture: () => ({ item: active, caseId: active.id }), owns: owner => [a,b].includes(owner.item), refresh: (id, options) => { calls.push(['snapshot',id,options.owner.item]); return snapshot.promise; } },
      Discovery: { clearCache() {} } },
    codesChangedExternally: () => calls.push(['codes']), updateSysCount: () => calls.push(['count']),
    loadFormatOptions: async () => calls.push(['formats']), mcpRefreshSource: async () => calls.push(['source']), updateTsExample: () => calls.push(['example']),
    toast: value => calls.push(['toast',value]), loadDerivedFields: async () => true,
  }); vm.runInContext(code, context);
  return { context, a, b, calls, snapshot, switchCase: () => { active = b; } };
}
test('MCP edit of another Case cannot mark unchanged active Case catalog as externally changed', async () => {
  for (const kind of ['codes','formats','ts_config','threats']) {
    const f = fixture(), work = f.context.handleMcpStateChanged(kind); f.snapshot.resolve({ accepted: false, reason: 'unchanged' }); await work;
    assert.deepEqual(f.calls.map(call => call[0]), ['snapshot'], kind);
  }
});
test('Case switch while confirming an MCP configuration edit cannot update the new Case panels', async () => {
  for (const kind of ['codes','formats','ts_config','threats']) {
    const f = fixture(), work = f.context.handleMcpStateChanged(kind); f.switchCase(); f.snapshot.resolve({ accepted: true }); await work;
    assert.deepEqual(f.calls.map(call => call[0]), ['snapshot'], kind);
  }
});
test('confirmed current-Case MCP timestamps reconcile their new source receipt; catalogs retain their draft warning', async () => {
  const f = fixture(), work = f.context.handleMcpStateChanged('ts_config'); f.snapshot.resolve({ accepted: true }); await work;
  assert.deepEqual(f.calls.map(call => call[0]), ['snapshot','source','example','toast']);
  const codes = fixture(), update = codes.context.handleMcpStateChanged('codes'); codes.snapshot.resolve({ accepted: true }); await update;
  assert.deepEqual(codes.calls.map(call => call[0]), ['snapshot','codes','count','toast']);
});
