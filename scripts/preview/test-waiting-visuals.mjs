/* Browser regression for the loading room (ring loader + robot skits).
   Run remotely: node scripts/preview/run-smoke.mjs test-waiting-visuals.mjs
   Uses the real app overlay/area veils and the production component in natural
   real time. Receipts are explicit fixtures; this does NOT validate the native engine. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const context = await browser.newContext({ viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference',
  recordVideo: { dir: resolve(output, 'video-raw'), size: { width: 1440, height: 960 } } });
const page = await context.newPage(), video = page.video(), errors = [], started = Date.now();
const results = { evidence: { app: 'Real load overlay, area veil and WaitingVisuals SVG/CSS', receipts: 'Synthetic fixtures', timing: 'Natural 1x', nativeEngineVerified: false }, screenshots: [], markers: [] };
const mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
const snap = async name => { await page.screenshot({ path: resolve(output, name), animations: 'allow' }); results.screenshots.push(name); };
let phase = 'startup', failure = null;
page.setDefaultTimeout(20000); page.on('pageerror', error => errors.push(error.message));

// Geometry of the robot relative to the round room, in screen pixels.
const robotState = selector => page.evaluate(selector => {
  const root = document.querySelector(selector), room = root?.querySelector('.wv-room'), robot = root?.querySelector('.wv-actors .wv-robot');
  if (!room) return null;
  const r = room.getBoundingClientRect(), b = robot?.getBoundingClientRect();
  const animations = root.querySelector('.wv-actors').getAnimations({ subtree: true });
  return { skit: root.dataset.skit, motion: root.dataset.motion, mirror: root.dataset.mirror, hasRobot: !!robot,
    inside: !!b && b.width > 0 && b.right > r.left + 4 && b.left < r.right - 4,
    playing: animations.filter(a => a.playState === 'running').length, paused: animations.filter(a => a.playState === 'paused').length,
    toggles: root.querySelectorAll('button').length };
}, selector);

try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && document.querySelector('#load-overlay').hidden);

  phase = 'overlay'; mark('overlay');
  await page.evaluate(() => showLoadOverlay('Validando a fonte'));
  const overlay = '#load-visual .waiting-visual';
  await page.waitForFunction(selector => document.querySelector(selector)?.dataset.motion === 'running', overlay);
  results.overlay = await page.evaluate(() => ({
    ring: !!document.querySelector('#load-visual .wv-ring-arc'), scan: !!document.querySelector('#load-visual .wv-scan'),
    toggles: document.querySelectorAll('#load-visual button').length, status: document.querySelector('#load-visual .wv-status').textContent,
    barHidden: document.querySelector('#load-bar-fill').parentElement.hidden, size: document.querySelector('#load-visual .wv-art').getBoundingClientRect().width,
    ringSpins: document.querySelector('#load-visual .wv-ring-arc').getAnimations().some(a => a.playState === 'running') }));
  assert.equal(results.overlay.ring, true); assert.equal(results.overlay.scan, true); assert.equal(results.overlay.toggles, 0);
  assert.equal(results.overlay.status, 'Validando a fonte'); assert.equal(results.overlay.barHidden, true); assert.equal(results.overlay.ringSpins, true);
  assert.ok(Math.abs(results.overlay.size - 124) < 1);
  await page.evaluate(() => LoadProgress.render({ label: 'Transferindo · fonte.gz', progress: 50,
    metrics: { completed: 1024, total: 2048, unit: 'bytes', elapsed: 61, phaseSeconds: 10, rate: 512, eta: 2, error: 'Aguardando disco' } }));
  assert.equal(await page.locator('#load-amount').textContent(), '1 KB');
  assert.equal(await page.locator('#load-of').textContent(), 'de 2 KB');
  assert.equal(await page.locator('#load-note').textContent(), 'fonte.gz');
  assert.equal(await page.locator('#load-stats [data-stat="rate"] dd').textContent(), '512 B/s');
  await page.evaluate(() => mirrorLoadOverlay('Indexando registros', '1.200 / 6.300 registros', 19));
  assert.equal(await page.locator('#load-stats').evaluate(node => node.childElementCount), 0, 'plain phases clear previous metrics');
  assert.equal(await page.locator('#load-error').evaluate(node => node.hidden), true, 'plain phases clear previous errors');
  await page.evaluate(() => LoadProgress.tick());
  assert.equal(await page.locator('#load-stats').evaluate(node => node.childElementCount), 0, 'ticks cannot restore stale timing');
  results.progress = await page.evaluate(() => ({ barHidden: document.querySelector('#load-bar-fill').parentElement.hidden,
    title: document.querySelector('#load-title').textContent, amount: document.querySelector('#load-amount').textContent,
    amountVisible: document.querySelector('#load-amount').getClientRects().length > 0, percent: document.querySelector('#load-percent').textContent }));
  assert.deepEqual(results.progress, { barHidden: false, title: 'Indexando registros', amount: '1.200 / 6.300 registros', amountVisible: true, percent: '19%' });
  // The first robot shows up soon, inside the room, as part of a whole skit.
  await page.waitForFunction(selector => document.querySelector(selector)?.dataset.skit, overlay, { timeout: 4000 });
  await page.waitForTimeout(1800);
  results.overlaySkit = await robotState(overlay);
  assert.ok(results.overlaySkit.hasRobot && results.overlaySkit.playing > 5, 'a skit is playing');
  await snap('waiting-room-overlay-dark.png');
  await page.evaluate(() => { if (document.documentElement.dataset.theme !== 'light') toggleTheme(); });
  await page.waitForTimeout(400); await snap('waiting-room-overlay-light.png');
  results.lightRoom = await page.evaluate(() => getComputedStyle(document.querySelector('#load-visual .wv-room')).backgroundImage.includes('rgb(3, 4, 6)'));
  assert.equal(results.lightRoom, true, 'the room stays dark in the light theme');
  await page.evaluate(() => toggleTheme());
  await page.evaluate(() => hideLoadOverlay(true));
  assert.equal(await page.locator('.waiting-visual').count(), 0, 'results settle immediately with the overlay');

  phase = 'area-veil'; mark('area');
  await page.evaluate(() => {
    const host = document.createElement('div'); host.id = 'wait-area'; Object.assign(host.style, { position: 'fixed', left: '320px', top: '200px', width: '640px', height: '360px' });
    document.body.append(host); window.__area = areaLoading(host, 'Calculando resumo', { phaseId: 'command:aggregate_events' });
  });
  await page.waitForFunction(() => document.querySelector('#wait-area .waiting-visual')?.dataset.motion === 'running');
  results.area = await page.evaluate(() => { const v = document.querySelector('#wait-area .waiting-visual'); return { display: getComputedStyle(v).display, size: v.querySelector('.wv-art').getBoundingClientRect().width, family: v.dataset.family, toggles: v.querySelectorAll('button').length }; });
  assert.deepEqual(results.area, { display: 'grid', size: 72, family: 'calculation', toggles: 0 });
  await page.waitForTimeout(1600); await snap('waiting-room-area.png');
  await page.evaluate(() => { __area.done(); document.querySelector('#wait-area').remove(); });

  phase = 'whole-skit'; mark('whole-skit');
  await page.evaluate(() => {
    const host = document.createElement('section'); host.id = 'wait-solo'; Object.assign(host.style, { position: 'fixed', left: '520px', top: '160px', width: '360px', background: 'var(--bg-1)', padding: '24px', zIndex: 5000 });
    document.body.append(host);
    window.__solo = WaitingVisuals.mount(host, { operationId: 'solo', phaseId: 'engine-index', state: 'running', label: 'Indexando' }, { random: () => 0.42 });
    window.__soloEvents = [];
    const root = __solo.element;
    new MutationObserver(() => __soloEvents.push({ t: performance.now(), skit: root.dataset.skit || '', robots: root.querySelectorAll('.wv-actors .wv-robot').length })).observe(root, { attributes: true, attributeFilter: ['data-skit'] });
  });
  await page.waitForFunction(() => __solo.inspect().mode === 'skit');
  await page.evaluate(() => __solo.preview('skate'));
  const solo = '#wait-solo .waiting-visual';
  await page.waitForTimeout(1500);
  results.skateMid = await robotState(solo);
  assert.equal(results.skateMid.skit, 'skate'); assert.equal(results.skateMid.inside, true);
  await page.waitForFunction(() => __solo.inspect().mode === 'gap', null, { timeout: 8000 });
  results.afterSkit = await page.evaluate(() => ({ actors: document.querySelector('#wait-solo .wv-actors').childElementCount, skit: __solo.element.dataset.skit, history: __solo.inspect().history }));
  assert.equal(results.afterSkit.actors, 0, 'the room is empty between skits'); assert.equal(results.afterSkit.skit, '');
  await page.waitForFunction(() => __solo.inspect().mode === 'skit', null, { timeout: 7000 });
  results.nextSkit = await page.evaluate(() => __solo.inspect().skit);

  phase = 'pauses'; mark('pauses');
  await page.evaluate(() => __solo.setVisible(false));
  results.hidden = await robotState(solo);
  assert.equal(results.hidden.motion, 'static'); assert.equal(results.hidden.playing, 0);
  await page.evaluate(() => __solo.setVisible(true));
  await page.waitForFunction(() => __solo.element.dataset.motion === 'running');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.waitForFunction(() => __solo.element.dataset.motion === 'static');
  results.reduced = await robotState(solo);
  assert.equal(results.reduced.playing, 0);
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.waitForFunction(() => __solo.element.dataset.motion === 'running');
  await page.evaluate(() => { __solo.destroy(); document.querySelector('#wait-solo').remove(); });
  assert.deepEqual(errors, []); results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error);
  await captureFailure(page, 'waiting-visuals', error, { phase, errors, results }); throw error;
} finally {
  results.phase = phase; results.errors = errors;
  try {
    await context.close(); if (!video) throw Error('Recording is unavailable');
    const filename = 'waiting-visuals-real-preview.webm'; await video.saveAs(resolve(output, filename));
    results.video = { filename, bytes: statSync(resolve(output, filename)).size }; await video.delete();
  } catch (error) { results.videoError = String(error); if (!failure) process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-visuals-results.json'), JSON.stringify(results, null, 2));
  await browser.close();
}
