/* CI-only continuity regression against the complete application CSS cascade.
   Run: node scripts/preview/run-smoke.mjs test-waiting-reduced-motion.mjs
   Receipts are synthetic; all work/reaction/bridge clocks advance naturally. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const definitions = [
  { family: 'reading', phaseId: 'metadata-scan' },
  { family: 'checkpoint', phaseId: 'metadata-checkpoint-sync' },
  { family: 'calculation', phaseId: 'command:aggregate_events' },
  { family: 'composition', phaseId: 'command:case_report_render' },
  { family: 'verification', phaseId: 'metadata-validate' },
];
const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const started = Date.now(), errors = [];
const results = { evidence: { component: 'Production WaitingVisuals and full application styles',
  receipts: 'Explicitly synthetic; natural animation time, no seeks, manufactured events or speed overrides',
  nativeEngineVerified: false, installedWebViewVerified: false, videoRequired: false },
  sourceCommit: process.env.GITHUB_SHA || null, rounds: [], screenshots: [] };
let browser, context, page, phase = 'startup';
const states = () => page.evaluate(() => __reducedPreview.entries.filter(e => !e.closed).map(e => e.read()));
const snap = async name => { await page.screenshot({ path: resolve(output, name), animations: 'allow' }); results.screenshots.push(name); };
async function settle() {
  await page.evaluate(async () => {
    await Promise.all(__reducedPreview.entries.flatMap(e => e.animations()).map(a => a.ready));
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
}
const identity = sample => sample.animations.map(a => [a.id, a.name, a.target, a.durationMs, a.playbackRate]);
const times = sample => sample.animations.map(a => a.currentTime);
async function probe() {
  return page.evaluate(() => {
    const node = __reducedPreview.probe, style = getComputedStyle(node);
    return { animationName: style.animationName, transitionProperty: style.transitionProperty,
      scrollBehavior: style.scrollBehavior, animations: node.getAnimations().length };
  });
}
async function reducedRound(label, { dispose = false } = {}) {
  const before = await states();
  assert.ok(before.every(s => s.animations.length > 0));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.waitForFunction(() => __reducedPreview.entries.filter(e => !e.closed).every(e => e.view.element.dataset.motion === 'static'));
  await settle(); const paused = await states();
  for (const [i, sample] of paused.entries()) {
    assert.deepEqual(identity(sample), identity(before[i]), `${label}: retain every CSSAnimation identity`);
    assert.equal(sample.episode, before[i].episode); assert.equal(sample.adapter, before[i].adapter);
    assert.equal(sample.controlVisible, false); assert.equal(sample.controlPressed, before[i].controlPressed);
    assert.equal(sample.status, before[i].status); assert.equal(sample.metric, before[i].metric);
    assert.ok(sample.animations.every(a => a.playState === 'paused' && !a.pending));
    assert.ok(sample.animations.every((a, j) => a.currentTime >= before[i].animations[j].currentTime));
  }
  const outside = await probe();
  assert.deepEqual(outside, { animationName: 'none', transitionProperty: 'none', scrollBehavior: 'auto', animations: 0 });
  await page.waitForTimeout(200); const frozen = await states();
  assert.deepEqual(frozen.map(identity), paused.map(identity));
  assert.deepEqual(frozen.map(times), paused.map(times), `${label}: exact frozen clocks`);
  assert.deepEqual(frozen.map(s => s.poses), paused.map(s => s.poses), `${label}: exact frozen SVG matrices`);
  const round = { label, before, paused, frozen, outside };
  if (label === 'react') await snap('waiting-reduced-frozen-reaction.png');
  if (dispose) {
    round.disposal = await page.evaluate(() => {
      const terminal = __reducedPreview.entries.find(e => e.family === 'reading');
      const removed = __reducedPreview.entries.find(e => e.family === 'checkpoint');
      const beforeTerminal = terminal.read(), beforeDestroy = removed.read();
      terminal.view.update({ ...terminal.receipt, state: 'error' }); terminal.closed = true;
      removed.view.destroy(); removed.closed = true;
      return { beforeTerminal, beforeDestroy, terminal: terminal.read(), destroyed: removed.read() };
    });
    assert.equal(round.disposal.beforeTerminal.episode, 'review');
    assert.equal(round.disposal.beforeDestroy.episode, 'review');
    assert.equal(round.disposal.terminal.family, 'neutral'); assert.equal(round.disposal.terminal.state, 'error');
    assert.equal(round.disposal.terminal.episode, 'work'); assert.equal(round.disposal.terminal.animations.length, 0);
    assert.equal(round.disposal.destroyed.connected, false); assert.equal(round.disposal.destroyed.animations.length, 0);
  }
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.waitForFunction(() => __reducedPreview.entries.filter(e => !e.closed).every(e => e.view.element.dataset.motion === (e.manualPaused ? 'static' : 'running')));
  await settle(); await page.waitForTimeout(100); const resumed = await states();
  for (const sample of resumed) {
    const original = before.find(s => s.id === sample.id), last = frozen.find(s => s.id === sample.id);
    assert.deepEqual(identity(sample), identity(original), `${label}: no replacement timelines on resume`);
    assert.equal(sample.episode, original.episode); assert.equal(sample.adapter, original.adapter);
    assert.equal(sample.controlVisible, true); assert.equal(sample.controlPressed, original.controlPressed);
    assert.equal(sample.controlLabel, original.controlLabel); assert.equal(sample.status, original.status); assert.equal(sample.metric, original.metric);
    if (sample.controlPressed === 'true') {
      assert.equal(sample.motion, 'static'); assert.deepEqual(times(sample), times(last));
      assert.deepEqual(sample.poses, last.poses, 'pre-existing user pause survives both preference changes');
      assert.ok(sample.animations.every(a => a.playState === 'paused'));
    } else {
      assert.equal(sample.motion, 'running');
      assert.ok(sample.animations.every((a, j) => a.currentTime >= last.animations[j].currentTime), `${label}: no clock restarts`);
      assert.ok(sample.animations.some((a, j) => a.currentTime > last.animations[j].currentTime), `${label}: natural advancement resumes`);
    }
  }
  assert.equal((await probe()).animationName, 'li-spin', 'unrelated app animation returns to its normal policy');
  if (dispose) {
    round.disposalAfterResume = await page.evaluate(() => __reducedPreview.entries.filter(e => e.closed).map(e => e.read()));
    assert.ok(round.disposalAfterResume.every(s => s.animations.length === 0 && s.motion === 'static'));
  }
  round.resumed = resumed; results.rounds.push(round); return round;
}

try {
  browser = await launchBrowser(); context = await browser.newContext({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
  page = await context.newPage(); page.setDefaultTimeout(15000); page.on('pageerror', error => errors.push(error.message));
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length > 0 && !state.loadOverlay);
  await page.waitForFunction(() => Tasks.pending() === 0);
  results.applicationBefore = await page.evaluate(() => ({ rows: state.rows.length, loaded: state.loaded }));
  results.selectorSupported = await page.evaluate(() => CSS.supports('selector(*:not(.waiting-visual[data-animated="true"] .wv-art *))'));
  assert.equal(results.selectorSupported, true);
  await page.evaluate(definitions => {
    const panel = document.createElement('section'); panel.id = 'waiting-reduced-fixture';
    panel.setAttribute('aria-label', 'Continuidade reduced motion: recibos sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '100px 20px auto', zIndex: '20000', padding: '16px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const label = document.createElement('p'); label.textContent = 'Cinco famílias reais · recibos sintéticos · tempo natural';
    const row = document.createElement('div'); Object.assign(row.style, { display: 'grid', gridTemplateColumns: 'repeat(5, 1fr)', gap: '8px' });
    // Existing app animation, deliberately outside every WaitingVisuals instance.
    const probe = document.createElement('span'); probe.className = 'spin'; probe.textContent = '·';
    Object.assign(probe.style, { display: 'inline-block', transition: 'opacity 1s', scrollBehavior: 'smooth' });
    panel.append(label, row, probe); document.body.append(panel);
    const ids = new WeakMap(); let nextId = 0;
    const fixture = window.__reducedPreview = { entries: [], probe, destroy() { this.entries.forEach(e => e.view.destroy()); panel.remove(); } };
    fixture.entries = definitions.map(({ family, phaseId }, index) => {
      const host = document.createElement('section'); host.id = `reduced-${family}`; row.append(host);
      const receipt = { operationId: `reduced-${family}`, phaseId, state: 'running', elapsedMs: 60000,
        label: `Fixture sintético: ${family}`, completed: index + 1, total: 42, unit: 'registros' };
      const model = WaitingVisuals.derive(receipt); let reactionSeed;
      for (let seed = 0; seed < 4096; seed++) if (WaitingVisuals.createDirector(receipt.operationId, seed).boundary(model)?.episode === 'review') { reactionSeed = seed; break; }
      if (reactionSeed === undefined) throw Error(`No review seed for ${family}`);
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed }), root = view.element, art = root.querySelector('.wv-art');
      const entry = { id: family, family, receipt, view, reactionSeed, closed: false, manualPaused: false,
        animations: () => art.getAnimations({ subtree: true }) };
      entry.read = () => {
        const control = root.querySelector('.wv-motion-toggle');
        return { id: entry.id, family: root.dataset.family, state: root.dataset.state, episode: root.dataset.episode, adapter: root.dataset.adapter,
          motion: root.dataset.motion, animated: root.dataset.animated, connected: root.isConnected,
          status: root.querySelector('.wv-status').textContent, metric: root.querySelector('.wv-metric').textContent,
          controlVisible: !control.hidden && getComputedStyle(control).display !== 'none', controlPressed: control.getAttribute('aria-pressed'), controlLabel: control.getAttribute('aria-label'),
          animations: entry.animations().map(a => {
            if (!ids.has(a)) ids.set(a, ++nextId);
            return { id: ids.get(a), name: a.animationName, target: a.effect.target.getAttribute('class'), currentTime: a.currentTime,
              durationMs: a.effect.getComputedTiming().duration, playbackRate: a.playbackRate, playState: a.playState, pending: a.pending };
          }),
          poses: [...art.querySelectorAll('svg g')].map(node => {
            const m = node.getScreenCTM(), style = getComputedStyle(node);
            return { target: node.getAttribute('class'), matrix: m ? [m.a, m.b, m.c, m.d, m.e, m.f] : null,
              opacity: style.opacity, visibility: style.visibility };
          }) };
      };
      return entry;
    });
  }, definitions);
  await settle(); results.initialReduced = await states();
  assert.ok(results.initialReduced.every(s => s.animated === 'false' && s.motion === 'static' && s.animations.length === 0 && !s.controlVisible));
  assert.deepEqual(await probe(), { animationName: 'none', transitionProperty: 'none', scrollBehavior: 'auto', animations: 0 });
  await snap('waiting-reduced-initial-static.png');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.waitForFunction(() => __reducedPreview.entries.every(e => e.view.element.dataset.motion === 'running'));
  await settle(); await page.waitForTimeout(700);
  phase = 'work timelines'; assert.ok((await states()).every(s => s.episode === 'work'));
  await reducedRound('work');
  phase = 'existing manual pause';
  await page.locator('#reduced-reading .wv-motion-toggle').click();
  await page.evaluate(() => { __reducedPreview.entries.find(e => e.family === 'reading').manualPaused = true; });
  await settle(); await reducedRound('manual-pause');
  await page.locator('#reduced-reading .wv-motion-toggle').click();
  await page.evaluate(() => { __reducedPreview.entries.find(e => e.family === 'reading').manualPaused = false; });
  await settle();
  phase = 'prepare bridge';
  await page.waitForFunction(() => {
    const root = __reducedPreview.entries.find(e => e.family === 'composition').view.element;
    return root.dataset.adapter === 'prepare' && root.querySelector('.wv-adapter-boundary').getAnimations()[0]?.currentTime >= 200;
  });
  const prepare = await reducedRound('prepare');
  assert.equal(prepare.before.find(s => s.id === 'composition').adapter, 'prepare');
  phase = 'reaction, terminal and destroy while reduced';
  await page.waitForFunction(() => __reducedPreview.entries.every(e => e.view.element.dataset.adapter === 'react'));
  await page.waitForTimeout(200);
  const react = await reducedRound('react', { dispose: true });
  assert.ok(react.before.every(s => s.episode === 'review' && s.adapter === 'react'));
  await snap('waiting-reduced-resumed-reaction.png');
  phase = 'resume bridge';
  await page.waitForFunction(() => {
    const root = __reducedPreview.entries.find(e => e.family === 'composition').view.element;
    return root.dataset.adapter === 'resume' && root.querySelector('.wv-adapter-boundary').getAnimations()[0]?.currentTime >= 200;
  });
  const resume = await reducedRound('resume');
  assert.equal(resume.before.find(s => s.id === 'composition').adapter, 'resume');
  await page.waitForFunction(() => __reducedPreview.entries.filter(e => !e.closed).every(e => e.view.element.dataset.episode === 'work'));
  results.returned = await states();
  results.applicationAfter = await page.evaluate(() => ({ rows: state.rows.length, loaded: state.loaded }));
  assert.deepEqual(results.applicationAfter, results.applicationBefore); assert.deepEqual(errors, []);
  await page.evaluate(() => __reducedPreview.destroy()); results.ok = true;
} catch (error) {
  results.ok = false; results.error = String(error.stack || error); process.exitCode = 1;
  if (page) await captureFailure(page, 'waiting-reduced-motion', error, { phase, results, errors });
} finally {
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  writeFileSync(resolve(output, 'waiting-reduced-motion-results.json'), JSON.stringify(results, null, 2));
}
