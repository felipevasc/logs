import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const code = app.slice(app.indexOf('let casesSaveQueue = Promise.resolve();'), app.indexOf('function defaultCaseWorkspace()'));
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function fixture() {
  const state = { cases: { revision: 1, active: 'a', cases: [{ id: 'a' }] } }, timers = new Map(), requests = [];
  let serial = 0;
  const context = vm.createContext({ state, window: {}, toast() {},
    setTimeout: fn => { timers.set(++serial, fn); return serial; }, clearTimeout: id => timers.delete(id),
    api: (cmd, args) => new Promise(resolve => requests.push({ cmd, args, resolve })),
  });
  vm.runInContext(code, context);
  return { state, context, requests, fire() { const pending = [...timers.values()]; timers.clear(); pending.forEach(fn => fn()); } };
}
{
  const f = fixture(), old = f.context.saveCases();
  f.state.cases = { revision: 8, active: 'b', cases: [{ id: 'b' }] };
  f.fire(); assert.equal(await old, false); assert.equal(f.requests.length, 0, 'debounce from replaced store cannot write its replacement');
}
{
  const f = fixture(), old = f.context.saveCases(); f.fire(); await flush();
  f.state.cases = { revision: 8, active: 'b', cases: [{ id: 'b' }] };
  f.requests[0].resolve({ revision: 2 });
  assert.equal(await old, false); assert.equal(f.state.cases.revision, 8, 'old acknowledgement cannot regress replacement store revision');
}
{
  const f = fixture(), first = f.context.saveCases(); f.fire(); await flush();
  const queued = f.context.saveCases(); f.fire(); await flush();
  f.state.cases = { revision: 8, active: 'b', cases: [{ id: 'b' }] };
  const next = f.context.saveCases(); f.fire();
  f.requests[0].resolve({ revision: 2 }); await flush();
  assert.equal(await first, false); assert.equal(await queued, false);
  assert.equal(f.requests.length, 2, 'obsolete queued save is skipped before native dispatch');
  assert.equal(f.requests[1].args.data.active, 'b'); assert.equal(f.requests[1].args.data.revision, 8);
  f.requests[1].resolve({ revision: 9 }); assert.equal(await next, true); assert.equal(f.state.cases.revision, 9);
}
console.log('Case persistence rejects replaced-store timers, queued writes and late receipts');
