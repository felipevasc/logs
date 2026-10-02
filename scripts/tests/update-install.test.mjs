import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

// Run the real installation controller without a browser/native installer.
const source = readFileSync(new URL('../../frontend/updates.js', import.meta.url), 'utf8');
const controller = source.slice(source.indexOf('  async function install('), source.indexOf('  async function installOnClose('));
function fixture(save, invoke) {
  const calls = [], opened = [];
  const context = vm.createContext({
    preparingInstall: false, status: { phase: 'ready' }, saveCases: save,
    call: async (command, args) => {
      calls.push([command, args]);
      if (command === 'update_status') return { phase: 'ready' };
      return invoke?.(command, args);
    },
    set: status => { context.status = status; }, open: () => opened.push(true), render: () => {},
  });
  vm.runInContext(controller, context);
  return { context, calls, opened };
}
test('install waits for a successful case save before invoking the updater', async () => {
  let saved;
  const f = fixture(() => new Promise(resolve => { saved = resolve; }));
  const pending = f.context.install();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.calls[0][0], 'update_install_on_close');
  assert.equal(f.calls[0][1].enabled, true);
  assert.equal(f.calls.some(([name]) => name === 'update_install'), false);
  saved(true);
  await pending;
  assert.equal(f.calls.at(-1)[0], 'update_install');
  assert.equal(f.calls.at(-1)[1].restart, true);
});
test('a failed save leaves the app open and never invokes installation', async () => {
  const f = fixture(async () => false);
  await assert.rejects(f.context.install(), /Não foi possível salvar/);
  assert.equal(f.calls.some(([name]) => name === 'update_install'), false);
  assert.equal(f.opened.length, 2);
  assert.match(f.context.status.error, /salvar os casos/);
});
test('a rejected save cancels install-on-close and reports an actionable failure', async () => {
  const f = fixture(async () => { throw Error('disk unavailable'); });
  await assert.rejects(f.context.install(false), /disk unavailable/);
  assert.equal(f.calls[0][0], 'update_install_on_close');
  assert.equal(f.calls[0][1].enabled, false);
  assert.equal(f.calls.some(([name]) => name === 'update_install'), false);
  assert.equal(f.context.preparingInstall, false);
});
test('launch failure is retryable without losing the verified ready state', async () => {
  let failed = true;
  const f = fixture(async () => true, command => {
    if (command === 'update_install' && failed) throw Error('installer unavailable');
  });
  await assert.rejects(f.context.install(), /installer unavailable/);
  assert.equal(f.context.status.phase, 'ready');
  failed = false;
  await f.context.install();
  assert.equal(f.calls.filter(([name]) => name === 'update_install').length, 2);
});
test('repeated clicks and close notifications cannot launch duplicate installers', async () => {
  let saved;
  const f = fixture(() => new Promise(resolve => { saved = resolve; }));
  const first = f.context.install(false);
  await f.context.install();
  saved(true);
  await first;
  assert.equal(f.calls.filter(([name]) => name === 'update_install').length, 1);
  assert.equal(f.calls[0][1].restart, false);
});
test('an already installing updater cannot trigger another save or install', async () => {
  const f = fixture(() => { throw Error('unexpected save'); });
  f.context.status.phase = 'installing';
  await f.context.install();
  assert.deepEqual(f.calls, []);
});
