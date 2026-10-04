import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const code = name => readFileSync(new URL(`../../frontend/${name}.js`, import.meta.url), 'utf8');
function fixture() {
  const events = [], requests = [], node = { before() {} };
  let owner = 'a';
  const context = vm.createContext({ window: { PerformanceTools: { queue: () => ({ add: fn => fn() }), estimate: () => null }, AnalysisContexts: { capture: () => ({ caseId: owner }), prepare: async value => value, isCurrent: value => value.caseId === owner, assertOwner() {}, validSnapshot: () => false } },
    state: {}, performance, document: { body: { dataset: { page: 'compromises' } }, documentElement: { dataset: { zone: 'analysis' } }, dispatchEvent: event => events.push(event) },
    CustomEvent: class { constructor(type, { detail }) { this.type = type; this.detail = detail; } }, $: () => node, el: () => ({ ...node }), requestAnimationFrame() {}, setTimeout() {}, clearTimeout() {}, setInterval() {}, clearInterval() {},
    api: (cmd, args) => cmd === 'cancel_task' ? Promise.resolve({}) : new Promise(resolve => requests.push({ cmd, args, resolve })) });
  vm.runInContext(code('compromise-progress'), context); vm.runInContext(code('tasks'), context);
  return { context, events, requests, switchOwner: () => { owner = 'b'; } };
}
const item = (id, state = 'pending', findings = 0) => ({ id, name: id, state, findings, countsByLevel: [0, 0, findings, 0, 0] });
test('incremental receipts preserve verified items and reject older revisions', () => {
  const f = fixture(), merge = f.context.window.CompromiseProgress.merge;
  const first = merge(null, { reset: true, revision: 1, items: [item('a'), item('b')] });
  const next = merge(first, { revision: 3, completed: 1, items: [item('a', 'completed', 2)] });
  assert.equal(next.items.length, 2); assert.equal(next.items[0].findings, 2); assert.equal(next.items[1].state, 'pending');
  assert.equal(merge(next, { revision: 2, items: [item('a')] }), next);
  assert.deepEqual(Array.from(merge(next, { reset: true, revision: 4, items: [item('c')] }).items, value => value.id), ['c']);
});
test('progress belongs to the exact active task and retains the checklist across shared phases', async () => {
  const f = fixture(), request = f.context.api('triage', {}, { latest: 'security:a' });
  for (let i = 0; i < 12; i++) await Promise.resolve();
  const tasks = f.context.window.Tasks, operationId = tasks.operationFor('security:a');
  assert.equal(tasks.progress({ operationId: 'other', triage: { revision: 1, items: [item('private')] } }), null);
  tasks.progress({ operationId, phaseId: 'triage-scan', triage: { reset: true, revision: 1, total: 1, completed: 0, findings: 0, items: [item('a')] } });
  tasks.progress({ operationId, phaseId: 'triage-correlate', completed: 100, unit: 'registros' });
  assert.equal(tasks.entryFor('security:a').triage.items[0].id, 'a');
  tasks.progress({ operationId, triage: { revision: 2, total: 1, completed: 1, findings: 2, items: [item('a', 'completed', 2)] } });
  assert.equal(tasks.entryFor('security:a').triage.completed, 1);
  const changes = f.events.filter(event => event.type === 'task-progress').length;
  tasks.progress({ operationId, triage: { revision: 1, items: [] } });
  assert.equal(f.events.filter(event => event.type === 'task-progress').length, changes);
  f.switchOwner(); assert.equal(tasks.progress({ operationId, triage: { revision: 3, items: [item('private')] } }), null);
  tasks.cancelLatest('security:a'); f.requests[0].resolve({}); await assert.rejects(request, /cancelada/);
  assert.equal(tasks.progress({ operationId, triage: { revision: 4, items: [] } }), null);
});
