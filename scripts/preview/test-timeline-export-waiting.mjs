/* Actual TimelineExport modal/render pipeline and real local PNG/PDF bytes.
   Only htmlToImage.toCanvas and the preview save boundary receive test gates.
   Synthetic Case data and preview transport do not verify Tauri/native writes. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser(), page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference' });
const errors = [], results = { evidence: { app: 'Actual TimelineExport open/render/save and WaitingVisuals DOM/CSS', gates: 'Test-only waits at htmlToImage.toCanvas and preview export_timeline', data: 'Synthetic Case, real PNG/PDF output bytes and application pixels', nativeEngineVerified: false, generatedImageAssets: false }, screenshots: [], focus: [] };
const dialog = page.locator('.tx-dialog'), scene = dialog.locator('.waiting-visual');
let phase = 'startup';
page.setDefaultTimeout(20000); page.on('pageerror', error => errors.push(error.message));
const motion = state => page.waitForFunction(value => document.querySelector('.tx-dialog .waiting-visual')?.dataset.motion === value, state);
const snap = async name => { await page.screenshot({ path: resolve(output, name), animations: 'allow' }); results.screenshots.push({ name, viewport: page.viewportSize(), theme: await page.evaluate(() => document.documentElement.dataset.theme) }); };
const focusReceipt = async point => {
  const receipt = await page.evaluate(() => {
    const active = document.activeElement;
    return { tag: active?.tagName, className: active?.className, insideDialog: !!active?.closest('.tx-dialog'), cancel: active?.hasAttribute('data-tx-cancel'), panel: active?.matches('.tx-dialog'), origin: active?.matches('.tx-trigger') };
  });
  results.focus.push({ point, phase, ...receipt }); return receipt;
};
const begin = async (format = 'pdf', reopen = true) => {
  await page.evaluate(() => { __timelineWaitGate.holdCanvas = true; });
  if (reopen) await page.locator('.tx-trigger:visible').click();
  await dialog.locator(`input[value="${format}"]`).check(); await dialog.locator('[data-tx-save]').click();
  assert.equal((await focusReceipt(`submit-${format}`)).cancel, true, 'disabling Export must preserve keyboard focus on Cancel without a test focus call');
  await page.waitForFunction(() => __timelineWaitGate.pendingCanvas.length === 1);
  await scene.waitFor({ state: 'visible' }); await motion('running');
};
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => state.loaded && state.rows.length && window.TimelineExport && window.WaitingVisuals && window.WorkspaceContext?.ready && !WorkspaceContext.changing && document.querySelector('#load-overlay').hidden);
  await page.evaluate(async () => {
    const start = Date.UTC(2026, 8, 22, 12);
    const fixture = { id: 'timeline-wait-fixture', name: 'Timeline de composição', items: [{ id: 'svc', label: 'API de teste', rows: Array.from({ length: 6 }, (_, index) => ({ id: index, event_ref: `r${index}`, name: `Ocorrência ${index}`, timestamp: start + index * 60000, message: `Registro ${index}` })) }], manual: [], timeline: { compact: false, groups: [], edits: {}, layout: {}, annotations: [{ id: 'note', anchor: 'e:svc:r1', text: 'Nota completa: ' + 'Conferir os registros preservados e a relação temporal. '.repeat(15), icon: 'fa-comment', arrow: 'both', lineStyle: 'dashed' }] } };
    state.cases.cases.push(fixture); state.cases.active = fixture.id;
    await WorkspaceContext.setScope('case', { page: 'case-timeline', animate: false }); setAnalysisView('timeline'); renderAnalysis();
    const source = document.querySelector('.ct-shell');
    const warm = await TimelineExport.render({ source, type: 'matrix', format: 'png', title: fixture.name });
    if (warm.blob.size < 100) throw Error('Real PNG warmup failed');
    window.__timelineExportMock = { files: [], cancel: false, download: false };
    const originalCanvas = htmlToImage.toCanvas, originalInvoke = __TAURI__.core.invoke;
    const gate = window.__timelineWaitGate = { holdCanvas: false, holdSave: false, failCanvas: false, pendingCanvas: [], pendingSave: [], canvasEntries: [], saveEntries: [],
      releaseOne() { this.pendingCanvas.shift()?.(); },
      releaseCanvas() { this.holdCanvas = false; for (const resume of this.pendingCanvas.splice(0)) resume(); },
      releaseSave() { this.holdSave = false; for (const resume of this.pendingSave.splice(0)) resume(); } };
    htmlToImage.toCanvas = async (...args) => {
      gate.canvasEntries.push({ heading: args[0].querySelector('.tx-sheet-heading strong').textContent, footer: args[0].querySelector('.tx-sheet-footer').textContent });
      if (gate.holdCanvas) await new Promise(resolve => gate.pendingCanvas.push(resolve));
      if (gate.failCanvas) throw Error('Falha de desenho (fixture de teste).');
      return originalCanvas(...args);
    };
    __TAURI__.core.invoke = async (command, args, options) => {
      if (command === 'export_timeline') {
        gate.saveEntries.push({ sceneCount: document.querySelectorAll('.tx-dialog .waiting-visual').length, filename: args.filename });
        if (gate.holdSave) await new Promise(resolve => gate.pendingSave.push(resolve));
      }
      return originalInvoke(command, args, options);
    };
  });
  phase = 'pdf-drawing'; await begin();
  results.short = await scene.evaluate(root => {
    window.__timelineWaitSVG = root.querySelector('svg');
    return { family: root.dataset.family, pace: root.dataset.pace, actors: root.querySelectorAll('.wv-task').length, status: root.querySelector('.wv-status').textContent, metric: root.querySelector('.wv-metric').textContent, detailsHidden: root.querySelector('.wv-details').hidden };
  });
  assert.equal(results.short.family, 'composition'); assert.equal(results.short.pace, 'gesture'); assert.equal(results.short.actors, 1);
  assert.equal(results.short.status, 'Desenhando páginas…'); assert.match(results.short.metric, /^0 \/ \d+ páginas desenhadas$/); assert.equal(results.short.detailsHidden, true);
  results.pdfTaskTotal = Number(results.short.metric.match(/^0 \/ (\d+)/)[1]);
  const estimated = await page.evaluate(() => TimelineExport.planPages(TimelineExport.measure({ source: document.querySelector('.ct-shell'), type: 'matrix' })).length);
  assert.ok(results.pdfTaskTotal > estimated, 'notes appendix adds real tasks before the first drawing receipt');
  await snap('timeline-export-wait-short-dark.png');
  await page.waitForFunction(() => document.querySelector('.tx-dialog .waiting-visual')?.dataset.pace === 'loop');
  await snap('timeline-export-wait-long-dark.png');
  phase = 'continuity-motion'; await page.evaluate(() => __timelineWaitGate.releaseOne());
  await page.waitForFunction(() => __timelineWaitGate.pendingCanvas.length === 1 && document.querySelector('.tx-dialog .wv-metric')?.textContent.startsWith('1 / '));
  assert.equal(await scene.evaluate(root => root.querySelector('svg') === __timelineWaitSVG), true, 'page callbacks preserve scene identity');
  assert.equal(await scene.locator('.wv-status').textContent(), 'Desenhando páginas…');
  await scene.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter'); await motion('static');
  assert.equal(await page.evaluate(() => __timelineWaitGate.pendingCanvas.length), 1, 'animation pause does not cancel or release drawing');
  await scene.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter'); await motion('running');
  await page.setViewportSize({ width: 640, height: 800 });
  await page.evaluate(() => { if (document.documentElement.dataset.theme !== 'light') toggleTheme(); });
  const bounds = await dialog.boundingBox(); assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 640 && bounds.y >= 0 && bounds.y + bounds.height <= 800);
  await snap('timeline-export-wait-light-640.png');
  await page.emulateMedia({ forcedColors: 'active' }); await snap('timeline-export-wait-forced-colors.png');
  await page.emulateMedia({ forcedColors: 'none', reducedMotion: 'reduce' }); await motion('static');
  assert.equal(await scene.locator('.wv-motion-toggle').isVisible(), false);
  await dialog.locator('[data-tx-cancel]').focus(); await page.keyboard.press('Shift+Tab');
  assert.equal(await dialog.locator('[data-tx-cancel]').evaluate(node => node === document.activeElement), true, 'hidden motion control is not a focus-trap endpoint');
  await snap('timeline-export-wait-reduced-motion.png');
  await page.emulateMedia({ reducedMotion: 'no-preference' }); await motion('running');
  await scene.evaluate(root => { root.style.transform = 'translateX(200vw)'; }); await motion('static');
  await scene.evaluate(root => { root.style.transform = ''; }); await motion('running');
  phase = 'save-cancel'; await page.evaluate(() => { __timelineWaitGate.holdSave = true; __timelineExportMock.cancel = true; __timelineWaitGate.releaseCanvas(); });
  await page.waitForFunction(() => __timelineWaitGate.pendingSave.length === 1);
  assert.equal(await scene.count(), 0); assert.equal(await dialog.locator('progress').isVisible(), false);
  assert.equal(await dialog.locator('[data-tx-status]').textContent(), 'Escolha onde salvar o arquivo…');
  assert.equal(await dialog.locator('[data-tx-cancel]').isDisabled(), true);
  assert.equal((await focusReceipt('save-pending')).panel, true, 'disabling Cancel must keep keyboard focus within the guarded dialog');
  for (const key of ['Tab', 'Shift+Tab']) {
    await page.keyboard.press(key); assert.equal((await focusReceipt(`save-pending-${key}`)).panel, true, 'Tab remains in the dialog while all controls are disabled');
  }
  await page.keyboard.press('Escape'); assert.equal(await dialog.count(), 1, 'native-save guard stays intact');
  assert.equal(await page.evaluate(() => __timelineWaitGate.saveEntries.at(-1).sceneCount), 0);
  await page.evaluate(() => __timelineWaitGate.releaseSave());
  await page.waitForFunction(() => document.querySelector('[data-tx-status]')?.textContent.includes('Salvamento cancelado'));
  assert.equal(await scene.count(), 0); assert.equal(await page.evaluate(() => __timelineExportMock.files.length), 0);
  assert.equal((await focusReceipt('save-cancelled')).panel, true);
  await snap('timeline-export-wait-save-cancelled.png'); await page.keyboard.press('Escape'); await dialog.waitFor({ state: 'detached' });
  assert.equal((await focusReceipt('save-cancelled-close')).origin, true);
  phase = 'render-cancel-reopen'; await page.setViewportSize({ width: 1024, height: 800 });
  for (const action of ['Escape', 'Cancel']) {
    await begin('png'); assert.equal(await scene.locator('.wv-metric').textContent(), '0 / 1 imagem desenhada');
    const beforeCancel = await page.evaluate(() => __timelineWaitGate.saveEntries.length);
    assert.equal((await focusReceipt(`before-${action}`)).cancel, true);
    if (action === 'Escape') await page.keyboard.press('Escape'); else await dialog.locator('[data-tx-cancel]').click();
    await dialog.waitFor({ state: 'detached' }); assert.equal(await scene.count(), 0);
    assert.equal((await focusReceipt(`after-${action}`)).origin, true);
    await page.evaluate(() => __timelineWaitGate.releaseCanvas()); await page.waitForFunction(() => !document.querySelector('.tx-stage'));
    assert.equal(await page.evaluate(() => __timelineWaitGate.saveEntries.length), beforeCancel, 'late raster settlement after cancel cannot save');
  }
  phase = 'error-retry'; await begin('png'); await scene.locator('.wv-motion-toggle').focus();
  await page.evaluate(() => { __timelineWaitGate.failCanvas = true; __timelineWaitGate.releaseCanvas(); });
  await dialog.locator('[data-tx-status].error').waitFor({ state: 'visible' });
  assert.match(await dialog.locator('[data-tx-status]').textContent(), /Falha de desenho/); assert.equal(await scene.count(), 0); await snap('timeline-export-wait-error.png');
  assert.equal((await focusReceipt('error-toggle-removed')).panel, true, 'only the removed focused toggle receives a replacement focus');
  await page.keyboard.press('Shift+Tab'); assert.equal(await dialog.locator('[data-tx-save]').evaluate(node => node === document.activeElement), true, 'backward Tab from the fallback section remains inside the modal');
  phase = 'unrelated-focus'; await begin('png', false); await page.locator('.tx-trigger:visible').focus();
  await page.evaluate(() => __timelineWaitGate.releaseCanvas()); await dialog.locator('[data-tx-status].error').waitFor({ state: 'visible' });
  assert.equal((await focusReceipt('error-unrelated-focus')).origin, true, 'a render settling after focus moved elsewhere must not take it back');
  phase = 'error-retry';
  await page.evaluate(() => { __timelineWaitGate.failCanvas = false; __timelineExportMock.cancel = false; }); await begin('png', false);
  await page.evaluate(() => __timelineWaitGate.releaseCanvas()); await dialog.waitFor({ state: 'detached' });
  let file = await page.evaluate(() => __timelineExportMock.files.at(-1)), bytes = Buffer.from(file.base64, 'base64');
  assert.equal(bytes.subarray(1, 4).toString(), 'PNG'); assert.match(file.filename, /^timeline-de-composicao-timeline-horizontal-.*\.png$/);
  results.png = { filename: file.filename, bytes: bytes.length, width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
  writeFileSync(resolve(output, 'timeline-export-wait-output.png'), bytes);
  phase = 'pdf-settlement'; await begin('pdf'); const finalMetric = await scene.locator('.wv-metric').textContent(), finalPages = Number(finalMetric.match(/^0 \/ (\d+)/)[1]);
  await page.evaluate(() => __timelineWaitGate.releaseCanvas()); await dialog.waitFor({ state: 'detached' });
  file = await page.evaluate(() => __timelineExportMock.files.at(-1)); bytes = Buffer.from(file.base64, 'base64');
  assert.equal(bytes.subarray(0, 5).toString(), '%PDF-'); const pages = (bytes.toString('latin1').match(/\/Type \/Page\b/g) || []).length; assert.equal(pages, finalPages);
  results.pdf = { filename: file.filename, bytes: bytes.length, pages }; writeFileSync(resolve(output, 'timeline-export-wait-output.pdf'), bytes);
  assert.equal(await scene.count(), 0); assert.equal(await page.locator('.tx-stage').count(), 0); assert.deepEqual(errors, []); results.ok = true;
} catch (error) {
  results.ok = false; results.error = String(error.stack || error); await focusReceipt('failure').catch(() => {}); await captureFailure(page, 'timeline-export-waiting', error, { phase, errors, results }); throw error;
} finally {
  results.phase = phase; results.errors = errors; writeFileSync(resolve(output, 'timeline-export-waiting-results.json'), JSON.stringify(results, null, 2)); await browser.close();
}
