/* CI-only rendered regression for the five EXISTING WaitingVisuals families.
   Run: node scripts/preview/run-smoke.mjs test-waiting-boundaries.mjs
   Synthetic receipts select modes; all animation boundaries run at natural speed.
   Screenshots/JSON are evidence, not a native engine or installed WebView claim. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const definitions = [
  { family: 'reading', phaseId: 'metadata-scan', cycleMs: 7200, workCount: 17 },
  { family: 'checkpoint', phaseId: 'metadata-checkpoint-sync', cycleMs: 6800, workCount: 8 },
  { family: 'calculation', phaseId: 'command:aggregate_events', cycleMs: 6400, workCount: 11 },
  { family: 'composition', phaseId: 'command:case_report_render', cycleMs: 7600, workCount: 11 },
  { family: 'verification', phaseId: 'metadata-validate', cycleMs: 9600, workCount: 7 },
];
const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const started = Date.now(), errors = [];
const results = {
  evidence: { component: 'Production WaitingVisuals SVG/CSS/controller', receipts: 'Explicitly synthetic mode/eligibility fixtures',
    timing: 'Natural browser time, trusted CSS animationiteration, no seeks or speed overrides',
    nativeEngineVerified: false, installedWebViewVerified: false, videoRequired: false },
  timingBudget: { expectedNaturalMs: [30000, 45000], runnerTimeoutMs: 120000 },
  sourceCommit: process.env.GITHUB_SHA || null, screenshots: [],
};
let browser, context, page, phase = 'startup';
const states = () => page.evaluate(() => __boundaryPreview.entries.map(entry => entry.read()));
const snap = async filename => {
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' }); results.screenshots.push(filename);
};
async function mountFixture(batch) {
  await page.evaluate(({ definitions, batch }) => {
    window.__boundaryPreview?.destroy();
    const panel = document.createElement('section'); panel.id = 'waiting-boundary-fixture';
    panel.setAttribute('aria-label', 'Fronteiras de animação: recibos sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '100px 20px auto', padding: '16px', zIndex: '20000',
      background: 'var(--bg-1)', color: 'var(--text-0)', border: '1px solid var(--border)' });
    const label = document.createElement('p'); label.textContent = 'Cinco famílias reais · recibos sintéticos · tempo natural';
    const row = document.createElement('div'); Object.assign(row.style, { display: 'grid', gridTemplateColumns: 'repeat(5, 1fr)', gap: '8px' });
    panel.append(label, row); document.body.append(panel);
    const sample = animations => animations.map(animation => ({ name: animation.animationName,
      target: animation.effect.target.getAttribute('class'), currentTime: animation.currentTime, startTime: animation.startTime,
      durationMs: animation.effect.getComputedTiming().duration, iterations: String(animation.effect.getComputedTiming().iterations),
      pending: animation.pending, playState: animation.playState, playbackRate: animation.playbackRate }));
    const fixture = window.__boundaryPreview = { entries: [], raf: null,
      destroy() { cancelAnimationFrame(this.raf); this.entries.forEach(e => { e.observer.disconnect(); e.view.destroy(); }); panel.remove(); } };
    fixture.entries = definitions.map((definition, index) => {
      const host = document.createElement('section'); host.id = `boundary-${index}`; row.append(host);
      const receipt = { operationId: `boundary-${batch}-${definition.family}`, phaseId: definition.phaseId, state: 'running',
        label: `Fixture sintético: ${definition.family}`, elapsedMs: 0, completed: 12, total: 42, unit: 'registros' };
      const model = WaitingVisuals.derive({ ...receipt, elapsedMs: 60000 });
      let reactionSeed;
      for (let seed = 0; seed < 4096; seed++) {
        if (WaitingVisuals.createDirector(receipt.operationId, seed).boundary(model)?.episode === 'review') { reactionSeed = seed; break; }
      }
      if (reactionSeed === undefined) throw Error(`No review seed for ${definition.family}`);
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed }), root = view.element;
      const entry = { definition, initialFamily: definition.family, receipt, view, reactionSeed,
        initialSvg: root.querySelector('svg'), firstBoundary: null, afterBoundary: null, boundaryEvents: [], phaseChange: null };
      // Include every work joint/prop and the separate controller clock. Never
      // substitute just the front arm for the rest of the rig.
      entry.animations = () => [...root.querySelector('.wv-work').getAnimations({ subtree: true }),
        ...root.querySelector('.wv-work-boundary').getAnimations()];
      entry.read = () => ({ family: root.dataset.family, pace: root.dataset.pace, motion: root.dataset.motion,
        episode: root.dataset.episode, adapter: root.dataset.adapter, cycle: view.inspect().cycle,
        metric: root.querySelector('.wv-metric').textContent, tracks: sample(entry.animations()) });
      entry.beginLoop = () => {
        const before = entry.animations();
        view.update({ ...entry.receipt, elapsedMs: 60000 });
        entry.loopTracks = entry.animations(); entry.loopSvg = root.querySelector('svg');
        entry.loopStart = { ...entry.read(), stableSvg: entry.loopSvg === entry.initialSvg,
          freshIdentities: entry.loopTracks.every(animation => !before.includes(animation)) };
      };
      // Capture before the product's bubbling handler pauses work for a reaction.
      // isTrusted rejects manufactured events; no event is dispatched by this test.
      root.addEventListener('animationiteration', event => {
        if (event.animationName !== 'wv-work-boundary') return;
        const isCurrentClock = event.target === root.querySelector('.wv-work-boundary');
        entry.boundaryEvents.push({ isTrusted: event.isTrusted, isCurrentClock, family: root.dataset.family, elapsedTime: event.elapsedTime });
        if (entry.firstBoundary || !isCurrentClock) return;
        entry.firstBoundary = { ...entry.read(), elapsedTime: event.elapsedTime, isTrusted: event.isTrusted,
          stableSvg: root.querySelector('svg') === entry.loopSvg,
          stableTracks: entry.animations().length === entry.loopTracks.length && entry.animations().every(a => entry.loopTracks.includes(a)),
          endpointPose: entry.animations().filter(a => a.effect.target instanceof SVGElement).map(a => {
            const node = a.effect.target, matrix = node.getScreenCTM(), style = getComputedStyle(node);
            return { name: a.animationName, target: node.getAttribute('class'), opacity: Number(style.opacity), transform: style.transform,
              screenMatrix: matrix ? [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f] : null };
          }) };
      }, true);
      entry.observer = new MutationObserver(() => {
        if (entry.firstBoundary && !entry.afterBoundary && root.dataset.episode !== 'work') {
          entry.afterBoundary = { family: root.dataset.family, episode: root.dataset.episode, adapter: root.dataset.adapter, cycle: view.inspect().cycle };
        }
      });
      entry.observer.observe(root, { attributes: true, attributeFilter: ['data-episode', 'data-adapter'] });
      return entry;
    });
    fixture.changeCheckpointPhaseNearBoundary = () => {
      const entry = fixture.entries.find(e => e.definition.family === 'checkpoint');
      const tick = () => {
        const beforeTracks = entry.animations(), clock = beforeTracks.find(a => a.animationName === 'wv-work-boundary');
        const remainingMs = entry.definition.cycleMs - clock.currentTime;
        if (remainingMs > 600) { fixture.raf = requestAnimationFrame(tick); return; }
        const before = entry.read(), svg = entry.loopSvg, root = entry.view.element;
        const beforeStatus = root.querySelector('.wv-status').textContent;
        entry.receipt = { ...entry.receipt, phaseId: 'metadata-checkpoint-write', elapsedMs: 60000,
          label: 'Fixture sintético: guardando checkpoint' };
        entry.view.update(entry.receipt);
        entry.sameFamilyChange = { fromPhase: 'metadata-checkpoint-sync', toPhase: entry.receipt.phaseId,
          remainingMs, before, after: entry.read(), beforeStatus, afterStatus: root.querySelector('.wv-status').textContent,
          stableSvg: root.querySelector('svg') === svg,
          stableTracks: entry.animations().length === beforeTracks.length && entry.animations().every(a => beforeTracks.includes(a)),
          hadNoBoundary: entry.firstBoundary === null, history: entry.view.inspect().history };
      };
      fixture.raf = requestAnimationFrame(tick);
    };
    fixture.changePhasesNearBoundary = () => {
      const tick = () => {
        for (const [index, entry] of fixture.entries.entries()) {
          if (entry.phaseChange) continue;
          const clock = entry.animations().find(a => a.animationName === 'wv-work-boundary');
          const remainingMs = entry.definition.cycleMs - clock.currentTime;
          if (remainingMs > 600) continue;
          const next = definitions[(index + 1) % definitions.length], oldSvg = entry.loopSvg, oldClock = clock;
          const before = entry.read(), previousTracks = entry.animations();
          entry.receipt = { ...entry.receipt, phaseId: next.phaseId, elapsedMs: 60000, label: `Fixture sintético: ${next.family}` };
          entry.view.update(entry.receipt); entry.definition = next;
          entry.loopSvg = entry.view.element.querySelector('svg'); entry.loopTracks = entry.animations();
          entry.phaseChange = { initialFamily: entry.initialFamily, nextFamily: next.family, remainingMs, before,
            after: entry.read(), hadNoBoundary: entry.firstBoundary === null,
            replacedSvg: oldSvg !== entry.loopSvg, oldClockDetached: !oldClock.effect.target.isConnected,
            freshIdentities: entry.loopTracks.every(a => !previousTracks.includes(a)) };
        }
        if (fixture.entries.some(entry => !entry.phaseChange)) fixture.raf = requestAnimationFrame(tick);
      };
      fixture.raf = requestAnimationFrame(tick);
    };
  }, { definitions, batch });
  await page.waitForFunction(() => __boundaryPreview.entries.every(e => e.view.element.dataset.motion === 'running'));
}
async function settle() {
  await page.evaluate(async () => {
    await Promise.all(__boundaryPreview.entries.flatMap(e => e.animations()).map(animation => animation.ready));
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
}
async function pauseAndResume() {
  for (const control of await page.locator('#waiting-boundary-fixture .wv-motion-toggle').all()) await control.click();
  await settle(); const paused = await states();
  assert.ok(paused.every(s => s.motion === 'static' && s.episode === 'work' && s.cycle === 0));
  assert.ok(paused.every(s => s.tracks.every(a => a.playState === 'paused' && !a.pending)));
  await page.waitForTimeout(300); const frozen = await states();
  assert.deepEqual(frozen.map(s => s.tracks), paused.map(s => s.tracks), 'all joints/props AND clocks freeze together');
  for (const control of await page.locator('#waiting-boundary-fixture .wv-motion-toggle').all()) await control.click();
  await settle(); await page.waitForTimeout(200); const resumed = await states();
  for (const [index, state] of resumed.entries()) {
    assert.equal(state.motion, 'running'); assert.equal(state.episode, 'work');
    assert.deepEqual(state.tracks.map(t => t.name), paused[index].tracks.map(t => t.name));
    assert.ok(state.tracks.every((track, i) => track.currentTime > paused[index].tracks[i].currentTime), 'resume advances every existing track without restarting');
  }
  return { paused, frozen, resumed };
}
function assertRig(sample, definition) {
  assert.equal(sample.tracks.length, definition.workCount + 1, `${definition.family}: complete work rig plus controller clock`);
  const clock = sample.tracks.find(track => track.name === 'wv-work-boundary'); assert.ok(clock);
  for (const track of sample.tracks) {
    assert.equal(track.durationMs, definition.cycleMs); assert.equal(track.iterations, 'Infinity'); assert.equal(track.playbackRate, 1);
    assert.ok(Number.isFinite(track.currentTime));
    assert.ok(Math.abs(track.currentTime - clock.currentTime) < 1, `${definition.family}/${track.name}: same active time as controller clock`);
    if (sample.motion === 'running') {
      assert.ok(Number.isFinite(track.startTime));
      assert.ok(Math.abs(track.startTime - clock.startTime) < 1, `${definition.family}/${track.name}: same timeline origin`);
    }
  }
}
async function collectBoundaries() {
  await page.waitForFunction(() => __boundaryPreview.entries.every(e => e.afterBoundary), null, { timeout: 22000 });
  const entries = await page.evaluate(() => __boundaryPreview.entries.map(e => ({ initialFamily: e.initialFamily, definition: e.definition,
    loopStart: e.loopStart, firstBoundary: e.firstBoundary, afterBoundary: e.afterBoundary, phaseChange: e.phaseChange, sameFamilyChange: e.sameFamilyChange, boundaryEvents: e.boundaryEvents })));
  for (const entry of entries) {
    const sample = entry.firstBoundary;
    assertRig(sample, entry.definition); assert.equal(sample.isTrusted, true);
    assert.equal(sample.stableSvg, true); assert.equal(sample.stableTracks, true);
    assert.equal(sample.endpointPose.length, entry.definition.workCount);
    assert.ok(sample.endpointPose.every(p => p.screenMatrix?.length === 6 && p.screenMatrix.every(Number.isFinite)), 'rendered joint/prop matrices accompany the boundary clocks');
    assert.equal(sample.cycle, 0); assert.equal(sample.episode, 'work');
    assert.equal(sample.elapsedTime * 1000, entry.definition.cycleMs, 'the FIRST real iteration, never a later boundary');
    assert.ok(sample.tracks.every(t => Math.abs(t.currentTime - entry.definition.cycleMs) < 300), 'every work track reaches its first complete loop');
    assert.equal(entry.afterBoundary.cycle, 1); assert.equal(entry.afterBoundary.episode, 'review');
    assert.equal(entry.afterBoundary.adapter, ['calculation', 'composition'].includes(entry.definition.family) ? 'prepare' : 'react');
    assert.equal(entry.boundaryEvents.length, 1); assert.ok(entry.boundaryEvents.every(e => e.isTrusted && e.isCurrentClock));
  }
  return entries;
}

try {
  browser = await launchBrowser(); context = await browser.newContext({ viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference' });
  page = await context.newPage(); page.setDefaultTimeout(15000); page.on('pageerror', error => errors.push(error.message));
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length > 0 && !state.loadOverlay);
  await page.waitForFunction(() => Tasks.pending() === 0);
  results.applicationBefore = await page.evaluate(() => ({ rows: state.rows.length, loaded: state.loaded }));
  phase = 'finished gesture to loop, all five families'; await mountFixture('finished');
  await page.waitForFunction(() => __boundaryPreview.entries.every(e => e.animations().length > 0 && e.animations().every(a => a.playState === 'finished')));
  results.finishedGestures = await states();
  assert.ok(results.finishedGestures.every(s => s.pace === 'gesture' && s.cycle === 0 && !s.tracks.some(t => t.name === 'wv-work-boundary')));
  await snap('waiting-boundaries-finished-gestures.png');
  await page.evaluate(() => __boundaryPreview.entries.forEach(e => e.beginLoop())); await settle();
  results.loopStart = await page.evaluate(() => __boundaryPreview.entries.map(e => e.loopStart));
  assert.ok(results.loopStart.every(s => s.stableSvg && s.freshIdentities));
  phase = 'real pause and resume before the first natural boundary'; results.pause = await pauseAndResume();
  for (const [index, sample] of results.pause.resumed.entries()) assertRig(sample, definitions[index]);
  await page.evaluate(() => __boundaryPreview.changeCheckpointPhaseNearBoundary());
  phase = 'first natural boundary across every work track'; results.firstBoundaries = await collectBoundaries();
  results.sameFamilyChange = results.firstBoundaries.find(e => e.definition.family === 'checkpoint').sameFamilyChange;
  const same = results.sameFamilyChange;
  assert.ok(same.remainingMs > 0 && same.remainingMs <= 600);
  assert.equal(same.fromPhase, 'metadata-checkpoint-sync'); assert.equal(same.toPhase, 'metadata-checkpoint-write');
  assert.notEqual(same.afterStatus, same.beforeStatus); assert.equal(same.afterStatus, 'Fixture sintético: guardando checkpoint');
  assert.ok(same.stableSvg && same.stableTracks && same.hadNoBoundary);
  assert.equal(same.before.cycle, 0); assert.equal(same.after.cycle, 0); assert.equal(same.after.episode, 'work');
  assert.deepEqual(same.history, [], 'same-family phase receipt cannot enqueue a reaction');
  assert.deepEqual(same.after.tracks, same.before.tracks, 'same-family phase receipt cannot reset any work identity, origin or clock');
  await snap('waiting-boundaries-first-review.png');

  phase = 'active gesture to loop followed by phase change near the first boundary'; await mountFixture('interrupted');
  await page.waitForFunction(() => __boundaryPreview.entries.every(e => e.animations().every(a => a.currentTime >= 700 && a.playState === 'running')));
  results.activeGestures = await states();
  assert.ok(results.activeGestures.every(s => s.pace === 'gesture' && !s.tracks.some(t => t.name === 'wv-work-boundary')));
  await page.evaluate(() => { __boundaryPreview.entries.forEach(e => e.beginLoop()); __boundaryPreview.changePhasesNearBoundary(); });
  await settle();
  results.activeLoopStart = await page.evaluate(() => __boundaryPreview.entries.map(e => e.loopStart));
  assert.ok(results.activeLoopStart.every(s => s.stableSvg && s.freshIdentities));
  results.phaseBoundaries = await collectBoundaries();
  for (const { phaseChange } of results.phaseBoundaries) {
    assert.ok(phaseChange.remainingMs > 0 && phaseChange.remainingMs <= 600, 'phase changes within the final 600ms BEFORE the old boundary');
    assert.equal(phaseChange.before.cycle, 0); assert.equal(phaseChange.before.episode, 'work');
    assert.equal(phaseChange.after.cycle, 0); assert.equal(phaseChange.after.episode, 'work');
    assert.ok(phaseChange.hadNoBoundary && phaseChange.replacedSvg && phaseChange.oldClockDetached && phaseChange.freshIdentities);
  }
  await snap('waiting-boundaries-new-phase-review.png');
  results.applicationAfter = await page.evaluate(() => ({ rows: state.rows.length, loaded: state.loaded }));
  assert.deepEqual(results.applicationAfter, results.applicationBefore); assert.deepEqual(errors, []);
  await page.evaluate(() => __boundaryPreview.destroy()); results.ok = true;
} catch (error) {
  results.ok = false; results.error = String(error.stack || error); process.exitCode = 1;
  if (page) await captureFailure(page, 'waiting-boundaries', error, { phase, results, errors });
} finally {
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  writeFileSync(resolve(output, 'waiting-boundaries-results.json'), JSON.stringify(results, null, 2));
}
