import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser(), page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Production Timestamp and Sources controls in Chromium; synthetic get/set/test/list transport. No native engine, real-data migration or installed WebView validation.', states: [] };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
const modal = page.locator('#ts-modal'), apply = page.locator('#ts-apply'), close = page.locator('#ts-close');
const rule = page.locator('#ts-rules .dv-rule-pattern'), custom = page.locator('#ts-format-custom');
const A = 'C:\\Caso A\\servidor\\same.log', B = 'D:\\Evidências\\pacote.zip!/área <literal>/same.log';
const configured = { timezone_offset_minutes: -180, clock_adjustment_ms: 1234, sources: ['message', 'arquivo'], rules: [{ regex: 'SOURCE_A_(.*)', template: '$1' }], format: '%Y CUSTOM %m', complement: '2001-01-01' };
const known = { timezone_offset_minutes: 0, clock_adjustment_ms: 2000, sources: ['linha'], rules: [], format: 'epoch_ms', complement: null };
const empty = { timezone_offset_minutes: null, clock_adjustment_ms: 0, sources: [], rules: [], format: '%Y-%m-%d %H:%M:%S%.f', complement: null };
const config = () => page.evaluate(() => structuredClone(buildTsConfig()));
const ready = () => page.waitForFunction(() => !document.querySelector('#ts-modal').hidden && !document.querySelector('#ts-apply').disabled);
const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
function measureEditorReadability(node) {
  const style = getComputedStyle(node);
  const luminance = color => {
    if (!/^rgba?\([\d.,\s]+\)$/.test(color)) throw Error(`Unsupported editor color: ${color}`);
    const channels = color.match(/[\d.]+/g).map(Number);
    if (channels.length !== 3 && (channels.length !== 4 || channels[3] !== 1)) throw Error(`Editor color must be opaque: ${color}`);
    return channels.slice(0, 3).map(channel => {
      const value = channel / 255;
      return value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4;
    }).reduce((total, value, index) => total + value * [.2126, .7152, .0722][index], 0);
  };
  const a = luminance(style.color), b = luminance(style.backgroundColor);
  return { tag: node.tagName, color: style.color, background: style.backgroundColor, opacity: style.opacity,
    contrast: (Math.max(a, b) + .05) / (Math.min(a, b) + .05), disabled: node.matches(':disabled'),
    readOnly: Boolean(node.readOnly), text: node.value ?? node.textContent };
}
function assertEditorReadability(value, theme) {
  const expected = theme === 'light' ? ['rgb(25, 43, 55)', 'rgb(237, 242, 245)'] : ['rgb(237, 242, 247)', 'rgb(14, 20, 27)'];
  assert.deepEqual([value.color, value.background], expected, `${theme}: foreground and actual editor surface`);
  assert.equal(value.opacity, '1'); assert.equal(value.disabled, false); assert.equal(value.readOnly, false);
  assert.ok(value.text.length > 0, 'measure loaded content rather than an empty control');
  assert.ok(value.contrast >= 4.5, `${theme}: editor contrast ${value.contrast.toFixed(3)}:1`);
}
async function sourceOpen(index) {
  await page.evaluate(() => Workspace.showPage('sources'));
  await page.locator('.sources-table tbody tr').nth(index).locator('.source-menu-trigger').click();
  await page.locator('.ctx-menu').getByRole('menuitem', { name: 'Data/hora', exact: true }).click();
}
async function snap(name) {
  results.states.push(await page.evaluate(label => ({ label, theme: document.documentElement.dataset.theme, config: buildTsConfig(), path: state.tsEditingPath,
    status: document.querySelector('#ts-status').textContent, activeElement: document.activeElement?.id || document.activeElement?.className,
    applyDisabled: document.querySelector('#ts-apply').disabled, customHidden: document.querySelector('#ts-format-custom').hidden,
    customValue: document.querySelector('#ts-format-custom').value, overflow: document.documentElement.scrollWidth > innerWidth,
  }), name));
  const snapshot = results.states.at(-1);
  if (!snapshot.applyDisabled) {
    assert.equal(await rule.isDisabled(), false, 'timestamp controls are enabled before measuring the preview');
    snapshot.readability = await page.locator('#ts-example').evaluate(measureEditorReadability);
    assert.equal(snapshot.readability.tag, 'PRE');
    assertEditorReadability(snapshot.readability, snapshot.theme);
  }
  await page.screenshot({ path: resolve(output, `timestamp-editor-${name}.png`) });
}
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && document.querySelector('#load-overlay').hidden);
  await page.evaluate(async ({ A, B, configured }) => {
    await WorkspaceContext.setScope('dataset', { page: 'explore', animate: false });
    const originalApi = api;
    const sources = [A, B].map(path => ({ name: 'same.log', path, format: 'jsonl', bytes: 1024, count: 12, start: 1700000000000, end: 1700000001000, unparsed: 0, sampled: 12, undated: 0 }));
    state.currentArtifact = { ...state.currentArtifact, kind: 'file', path: A, source: { kind: 'file', path: A, paths: [A, B], format: 'auto' } };
    document.querySelector('#file-path').value = `${A};${B}`;
    window.__timestampTest = { saved: { [A]: configured }, sources, writes: [], reads: [], tests: [], held: [], testHeld: [], holdTests: false, holdReads: false, failRead: false, failWrite: false };
    api = async (command, args = {}, options = {}) => {
      const f = __timestampTest;
      if (command === 'list_sources') return structuredClone(f.sources);
      if (command === 'get_ts_config') {
        f.reads.push(structuredClone(args));
        if (f.holdReads) return new Promise((resolve, reject) => f.held.push({ path: args.path, resolve, reject }));
        if (f.failRead) throw Error('Synthetic timestamp read refused');
        return structuredClone(f.saved[args.path] ?? null);
      }
      if (command === 'set_ts_config') {
        f.writes.push(structuredClone(args));
        if (f.failWrite) throw Error('Synthetic timestamp write refused');
        f.saved[args.path] = structuredClone(args.config);
        return { publication: { ...state.sourcePublication, generation: (state.sourcePublication?.generation || 0) + 1 } };
      }
      if (command === 'test_ts_config') { f.tests.push(structuredClone(args)); if (f.holdTests) return new Promise(resolve => f.testHeld.push(resolve)); return [['synthetic sample', '2026-01-01T00:00:00Z']]; }
      return originalApi(command, args, options);
    };
  }, { A, B, configured });

  phase = 'configured A, close, disabled clean B while loading';
  await sourceOpen(0); await ready(); assert.deepEqual(await config(), configured);
  await page.locator('#ts-test').click(); await page.locator('#ts-test-result').getByText('synthetic sample', { exact: false }).waitFor();
  await snap('configured-a-dark');
  await page.evaluate(() => { __timestampTest.holdTests = true; }); await page.locator('#ts-test').click();
  await page.waitForFunction(() => __timestampTest.testHeld.length === 1); await page.locator('#ts-complement').fill('edited during Test');
  await page.evaluate(() => { __timestampTest.holdTests = false; __timestampTest.testHeld[0]([['old sample', 'old result']]); }); await settle();
  assert.equal(await page.locator('#ts-test-result').textContent(), ''); await page.locator('#ts-complement').fill(configured.complement);
  await close.click();
  assert.equal(await page.locator('.sources-table tbody tr').nth(0).locator('.source-menu-trigger').evaluate(node => node === document.activeElement), true);
  await page.evaluate(() => { __timestampTest.holdReads = true; }); await sourceOpen(1);
  await page.waitForFunction(() => __timestampTest.held.length === 1);
  assert.deepEqual(await config(), empty); assert.equal(await custom.inputValue(), ''); assert.equal(await custom.isVisible(), false);
  assert.equal(await rule.count(), 1); assert.equal(await rule.inputValue(), ''); assert.equal(await page.locator('#ts-test-result').textContent(), '');
  for (const id of ['ts-apply', 'ts-test', 'ts-reset', 'ts-zone', 'ts-format']) assert.equal(await page.locator(`#${id}`).isDisabled(), true);
  await snap('empty-b-loading');
  await page.evaluate(() => { __timestampTest.holdReads = false; __timestampTest.held[0].resolve(null); }); await ready();
  assert.deepEqual(await config(), empty); await page.setViewportSize({ width: 640, height: 960 });
  await page.evaluate(() => { document.documentElement.dataset.theme = 'light'; }); await snap('empty-b-light-640');
  assert.equal(results.states.at(-1).overflow, false);
  await page.locator('#ts-sources').getByRole('button', { name: 'Mensagem', exact: true }).click();
  await apply.click(); await page.waitForFunction(() => __timestampTest.writes.length === 1 && document.querySelector('#load-overlay').hidden); await ready();
  assert.deepEqual(await page.evaluate(() => __timestampTest.writes[0]), { path: B, config: { ...empty, sources: ['message'] } });

  phase = 'empty to configured, custom to known, and cancel/reopen';
  await close.click(); await page.setViewportSize({ width: 1024, height: 960 });
  await sourceOpen(0); await ready(); assert.deepEqual(await config(), configured);
  await page.locator('#ts-complement').fill('unsaved draft'); await page.keyboard.press('Escape'); await modal.waitFor({ state: 'hidden' });
  await sourceOpen(0); await ready(); assert.deepEqual(await config(), configured); await close.click();
  await page.evaluate(({ B, known }) => { __timestampTest.saved[B] = known; }, { B, known });
  await sourceOpen(1); await ready(); assert.deepEqual(await config(), known); assert.equal(await custom.inputValue(), ''); assert.equal(await custom.isVisible(), false);
  await snap('known-b-light-1024'); await close.click();

  phase = 'read failure and same-source retry';
  await page.evaluate(() => { __timestampTest.failRead = true; }); await sourceOpen(0);
  await page.locator('#ts-retry').waitFor({ state: 'visible' }); assert.equal(await apply.isDisabled(), true); assert.deepEqual(await config(), empty);
  assert.equal(await page.locator('#ts-retry').evaluate(node => node === document.activeElement), true);
  await snap('read-error'); await page.evaluate(() => { __timestampTest.failRead = false; }); await page.locator('#ts-retry').click(); await ready(); assert.deepEqual(await config(), configured);
  await close.click();

  phase = 'overlapping A/B and cancelled late replies';
  await page.evaluate(({ A, B }) => { __timestampTest.holdReads = true; void openTsModal(A); void openTsModal(B); }, { A, B });
  await page.waitForFunction(() => __timestampTest.held.length === 3);
  await page.evaluate(known => __timestampTest.held[2].resolve(known), known); await ready();
  await page.locator('#ts-zone').focus(); await page.evaluate(configured => __timestampTest.held[1].resolve(configured), configured); await settle();
  assert.deepEqual(await config(), known); assert.equal(await page.locator('#ts-zone').evaluate(node => node === document.activeElement), true);
  await close.click();
  await page.evaluate(A => { void openTsModal(A); }, A); await page.waitForFunction(() => __timestampTest.held.length === 4); await close.click();
  await page.evaluate(configured => { __timestampTest.holdReads = false; __timestampTest.held[3].resolve(configured); }, configured); await settle(); assert.equal(await modal.isVisible(), false);

  phase = 'general versus individual exact Apply scope';
  const before = await page.evaluate(() => __timestampTest.writes.length);
  await page.locator('#ws-source-config').click(); await page.locator('#ts-open').click();
  await ready(); assert.equal(await page.evaluate(() => state.tsEditingPath), null);
  await apply.click(); await page.waitForFunction(expected => __timestampTest.writes.length === expected && document.querySelector('#load-overlay').hidden, before + 2); await ready();
  assert.deepEqual(await page.evaluate(before => __timestampTest.writes.slice(before), before), [{ path: A, config: configured }, { path: B, config: configured }]);
  await close.click(); await sourceOpen(1); await ready(); await apply.click();
  await page.waitForFunction(expected => __timestampTest.writes.length === expected && document.querySelector('#load-overlay').hidden, before + 3); await ready();
  assert.equal(await page.evaluate(() => __timestampTest.writes.at(-1).path), B);

  phase = 'failed Apply retains editable draft';
  await page.evaluate(() => { __timestampTest.failWrite = true; }); await page.locator('#ts-complement').fill('retry-me'); await apply.click();
  await page.waitForFunction(() => document.querySelector('#load-overlay').hidden && !document.querySelector('#ts-apply').disabled);
  assert.equal(await page.locator('#ts-complement').inputValue(), 'retry-me'); await snap('write-error-draft');
  results.writes = await page.evaluate(() => __timestampTest.writes); results.errors = errors;
  assert.deepEqual([...new Set(results.states.filter(value => value.readability).map(value => value.theme))].sort(), ['dark', 'light']);
  assert.deepEqual(errors, []);
  writeFileSync(resolve(output, 'timestamp-editor.json'), JSON.stringify(results, null, 2) + '\n');
} catch (error) {
  await captureFailure(page, 'timestamp-editor', error, { phase, errors, results }); throw error;
} finally { await browser.close(); }
