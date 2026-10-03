import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const window = {};
vm.runInNewContext(readFileSync(new URL('../../frontend/waiting-progress.js', import.meta.url), 'utf8'), { window });
const snapshot = window.WaitingProgress.snapshot;

test('received phase chooses concise text without using localized-label guesses', () => {
  const result = snapshot({ operationId: 'load-a', phaseId: 'metadata-scan', phase: 'Indexando metadados · arquivo.json', completed: 12, total: 40, unit: 'bytes', elapsedMs: 6000 });
  assert.equal(result.label, 'Indexando registros');
  assert.equal(result.operationId, 'load-a'); assert.equal(result.phaseId, 'metadata-scan');
  assert.equal(result.completed, 12); assert.equal(result.total, 40); assert.equal(result.unit, 'bytes');
  assert.equal(result.elapsedMs, 6000);
  assert.equal(snapshot({ phaseId: 'future', phase: 'Novo trabalho' }).label, 'Novo trabalho');
  assert.equal(snapshot({ phaseId: '__proto__', phase: 'Novo trabalho' }).label, 'Novo trabalho');
});

test('phase ready and a full checkpoint never claim the entire operation completed', () => {
  for (const phaseId of ['metadata-scan', 'metadata-checkpoint-committed']) {
    const result = snapshot({ phaseId, state: 'ready', completed: 40, total: 40, unit: 'registros' });
    assert.equal(result.state, 'running');
    assert.equal(Object.hasOwn(result, 'phaseCount'), false);
    assert.equal(Object.hasOwn(result, 'percent'), false);
  }
  for (const phaseId of ['ready', 'Pronto', 'Concluído']) {
    assert.equal(snapshot({ phaseId, operation: 'carregamento', phase: 'Concluído' }).label, 'Confirmando abertura');
  }
});

test('only supplied counts and measured times are presented', () => {
  const none = snapshot(null);
  assert.equal(none.label, 'Processando'); assert.equal(none.completed, undefined); assert.equal(none.elapsedMs, undefined);
  assert.equal(snapshot({ completed: 0, total: 0, unit: '' }).completed, undefined);
  const known = snapshot({ completed: 7, total: 0, unit: 'registros' }, { operationId: 'fallback', elapsedMs: 4300, estimateMs: 9000 });
  assert.equal(known.completed, 7); assert.equal(known.total, 0); assert.equal(known.operationId, 'fallback');
  assert.equal(known.elapsedMs, 4300); assert.equal(known.estimateMs, 9000);
  assert.equal(snapshot({ elapsedMs: -1 }, { elapsedMs: 3 }).elapsedMs, 3);
  assert.equal(snapshot({}, { estimateMs: Infinity }).estimateMs, undefined);
});

test('error remains directly visible instead of hidden behind an attractive phase label', () => {
  const result = snapshot({ phaseId: 'metadata-checkpoint-write', error: 'Disco sem espaço' });
  assert.equal(result.state, 'error'); assert.equal(result.label, 'Disco sem espaço');
});

test('entrypoint, production bundle and real pilots include the component and adapter', () => {
  const root = new URL('../../', import.meta.url);
  const html = readFileSync(new URL('frontend/index.html', root), 'utf8');
  const bundle = readFileSync(new URL('scripts/prepare-frontend.mjs', root), 'utf8');
  for (const file of ['waiting-visuals.js', 'waiting-progress.js', 'waiting-visuals.css']) {
    assert.ok(html.includes(file)); assert.ok(bundle.includes(file));
  }
  assert.ok(html.indexOf('waiting-progress.js') < html.indexOf('src="app.js"'));
  const app = readFileSync(new URL('frontend/app.js', root), 'utf8');
  assert.match(app, /WaitingProgress\.snapshot\(payload/);
  assert.match(app, /phaseId: "command:pivot"/);
  const workbench = readFileSync(new URL('frontend/analysis-workbench.js', root), 'utf8');
  assert.match(workbench, /phaseId: "command:aggregate_events"/);
  assert.match(workbench, /waiting\.done\(\)/);
});
