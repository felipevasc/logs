/* CI-only: node scripts/preview/run-smoke.mjs test-waiting-restore-access.mjs
   Production components, explicitly synthetic receipts. Natural 1x WebM is
   finalized before a separate diagnostic context; no native lock timing claim. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
import { installMotionSampling } from './motion-sampling.mjs';

const families = ['access', 'restoration'];
const phases = ['metadata-lock', 'metadata-restore'];
const labels = ['Aguardando acesso', 'Retomando metadados'];
// CI70 observed the previous owner at the exact 17% and 81% steps, while
// both copies had coincident matrices and exactly one was visible. The saved
// exact-time samples did not expose the expected right side. Keep their geometry
// gate, plus strict probes one integer millisecond before/after each transfer.
const restorationSampling = Object.freeze({ cycleMs: 7200, sideMarginMs: 1, maxTimeErrorMs: .01,
  transferFractions: [.17, .81],
  fractions: [0, .14, .16999, .17, .20, .30, .42, .48, .58, .64, .78, .80999, .81, .84, .94, .99999] });
function restorationProbePlan(contract = restorationSampling) {
  const probes = contract.fractions.map(fraction => ({ fraction, requestedTimeMs: contract.cycleMs * fraction,
    kind: contract.transferFractions.includes(fraction) ? 'exact-transfer' : 'ordinary',
    expectedHeld: contract.transferFractions.includes(fraction) ? null : fraction >= .17 && fraction < .81 ? 1 : 0 }));
  for (const [index, transferFraction] of contract.transferFractions.entries()) for (const side of [-1, 1]) {
    const requestedTimeMs = contract.cycleMs * transferFraction + side * contract.sideMarginMs;
    probes.push({ fraction: requestedTimeMs / contract.cycleMs, requestedTimeMs, transferFraction,
      kind: side < 0 ? 'before-transfer' : 'after-transfer', expectedHeld: index === 0 ? Number(side > 0) : Number(side < 0) });
  }
  return probes.sort((a, b) => a.requestedTimeMs - b.requestedTimeMs);
}
function assertRestorationOwnership(sample, contract = restorationSampling) {
  assert.ok([0, 1].includes(sample.heldOpacity) && [0, 1].includes(sample.seatedOpacity), 'ownership is binary at every sample');
  assert.equal(sample.heldOpacity + sample.seatedOpacity, 1, 'exactly one record, including at exact transfers');
  if (sample.kind === 'exact-transfer') {
    assert.ok(contract.transferFractions.includes(sample.fraction));
    assert.equal(sample.expectedHeld, null, 'only the two exact discontinuities have no one-sided expectation');
  } else assert.equal(sample.heldOpacity, sample.expectedHeld, `${sample.kind} at ${sample.requestedTimeMs} ms`);
  assert.equal(sample.timeline.length, 7, 'observe all six restoration tracks and the work clock');
  for (const name of ['wv-restore-held', 'wv-restore-seated']) assert.equal(sample.timeline.filter(t => t.name === name).length, 1);
  for (const track of sample.timeline) {
    assert.equal(track.durationMs, contract.cycleMs);
    assert.ok(Number.isFinite(track.currentTime) && Number.isFinite(track.progress));
    assert.ok(Math.abs(track.currentTime - sample.requestedTimeMs) < contract.maxTimeErrorMs,
      `${track.name}: requested side must be observable within ${contract.maxTimeErrorMs} ms`);
    assert.equal(track.playState, 'paused'); assert.equal(track.pending, false);
  }
}

const output = resolve('output/playwright'), started = Date.now();
const viewport = { width: 1440, height: 960 };
mkdirSync(output, { recursive: true });
const results = {
  evidence: { component: 'Production WaitingVisuals, WaitingProgress, SVG, CSS and shared reaction director',
    context: 'Actual Chromium preview app; explicitly labelled synthetic receipt fixture',
    timing: 'Natural 1x work and contextual visor with full return, no seek during video',
    eligibility: 'Synthetic supplied age belongs to the whole operation, never measured phase latency',
    poseSampling: 'Only in a separate non-recorded context after natural WebM finalization',
    nativeEngineVerified: false, installedWebViewVerified: false, generatedImageAssets: false },
  limits: { totalTimeoutMs: 110000, targetRuntimeMs: [55000, 80000], naturalBudgetMs: 30000,
    diagnosticBoundaryBudgetMs: 14400, accessGestureMs: 2400, restorationCycleMs: 7200,
    visorMs: 4800, manualMs: 32000, contactToleranceCssPx: .15, staticMatrixTolerance: .00001,
    evidenceFileLimitBytes: 32 * 1024 * 1024,
    estimatedNaturalBytes: 6000000, estimatedOtherArchiveBytes: 16000000,
    estimateBasis: 'CI66 verification: 5,769,239 bytes / 58.061s; CI67 manual: 5,614,425 bytes. Use 200kB/s allowance plus 10MB manual reserve; estimate, not a measured output guarantee' },
  sourceCommit: process.env.GITHUB_SHA || null, families, phases, restorationSampling, markers: [], screenshots: [],
};
let browser, context, page, video, failure = null, phase = 'startup';
const errors = [];
const mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
const deadline = setTimeout(() => {
  results.timedOut = true; results.ok = false; process.exitCode = 1;
  void browser?.close().catch(() => {});
}, results.limits.totalTimeoutMs);
deadline.unref();
const roots = () => page.locator('#waiting-restore-access-fixture .waiting-visual');
const scene = family => page.locator(`#restore-access-${family} .waiting-visual`);
const states = () => roots().evaluateAll(nodes => nodes.map(root => {
  const opacity = selector => { const node = root.querySelector(selector); return node ? Number(getComputedStyle(node).opacity) : null; };
  const animationId = animation => {
    const fixture = __restoreAccessPreview;
    if (!fixture.animationIds.has(animation)) fixture.animationIds.set(animation, fixture.nextAnimationId++);
    return fixture.animationIds.get(animation);
  };
  return { family: root.dataset.family, state: root.dataset.state, pace: root.dataset.pace,
    episode: root.dataset.episode, adapter: root.dataset.adapter, motion: root.dataset.motion,
    metric: root.querySelector('.wv-metric').textContent, status: root.querySelector('.wv-status').textContent,
    heldOpacity: opacity('.wv-restore-held'), seatedOpacity: opacity('.wv-restore-seated'),
    kitchenOpacity: opacity('.wv-kitchen'), svgCount: root.querySelectorAll('svg').length,
    poses: [...root.querySelectorAll('.wv-art svg g')].map(node => ({ target: node.getAttribute('class'),
      transform: getComputedStyle(node).transform, opacity: Number(getComputedStyle(node).opacity), visibility: getComputedStyle(node).visibility })),
    animations: root.querySelector('.wv-art').getAnimations({ subtree: true }).map(a => ({
      id: animationId(a), name: a.animationName, currentTime: a.currentTime, playState: a.playState, pending: a.pending,
      durationMs: a.effect.getComputedTiming().duration, iterations: String(a.effect.getTiming().iterations), playbackRate: a.playbackRate })) };
}));
const settle = () => roots().evaluateAll(async nodes => {
  await Promise.all(nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true })).map(a => a.ready));
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
});
const snap = async (filename, kind) => {
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  assert.ok(statSync(resolve(output, filename)).size < results.limits.evidenceFileLimitBytes);
  results.screenshots.push({ filename, kind, phase, offsetMs: Date.now() - started });
};
const waitEpisode = (episode, timeout = 12000) => page.waitForFunction(episode => {
  const root = __restoreAccessPreview.views.restoration.element;
  return root.dataset.episode === episode && root.dataset.adapter === (episode === 'work' ? 'work' : 'react');
}, episode, { timeout });
const waitMotion = motion => page.waitForFunction(motion => Object.values(__restoreAccessPreview.views)
  .every(view => view.element.dataset.motion === motion), motion);
const setTheme = theme => page.evaluate(async theme => {
  if (document.documentElement.dataset.theme !== theme) toggleTheme();
  await Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {})));
}, theme);
function assertReceipts(samples) {
  assert.deepEqual(samples.map(s => s.family), families);
  assert.deepEqual(samples.map(s => s.status), labels);
  for (const sample of samples) {
    assert.equal(sample.state, 'running'); assert.equal(sample.metric, '8 / 8 registros'); assert.equal(sample.svgCount, 1);
    assert.doesNotMatch(sample.status, /sucesso|concluído|aprovado|100%/i);
  }
  assert.equal(samples[0].pace, 'gesture'); assert.equal(samples[0].episode, 'work');
  assert.equal(samples[0].kitchenOpacity, null, 'access has no reaction props');
}
async function openApplication(recording) {
  context = await browser.newContext({ viewport, reducedMotion: 'no-preference',
    ...(recording ? { recordVideo: { dir: resolve(output, 'video-raw'), size: viewport } } : {}) });
  page = await context.newPage();
  if (recording) {
    video = page.video(); results.video = { filename: 'waiting-restore-access-real-preview.webm',
      finalized: false, recordedSize: viewport, startedOffsetMs: Date.now() - started,
      playbackRate: 1, includesDiagnosticSeeks: false, artifact: 'preview-recordings-other' };
  }
  await page.addInitScript(installMotionSampling);
  page.setDefaultTimeout(12000); page.on('pageerror', error => errors.push(error.message));
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden && window.WaitingVisuals && window.WaitingProgress);
  await page.waitForFunction(() => Tasks.pending() === 0);
  await page.evaluate(() => UiScale.set(1, false)); await setTheme('dark');
}
async function mountFixture(mode, expectedMotion = 'running') {
  await page.evaluate(({ families, phases, mode }) => {
    const panel = document.createElement('section'); panel.id = 'waiting-restore-access-fixture';
    panel.setAttribute('aria-label', 'Acesso e retomada: fixture com dados sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '150px auto auto 50%', transform: 'translateX(-50%)',
      width: '740px', maxWidth: 'calc(100vw - 32px)', padding: '24px', boxSizing: 'border-box',
      zIndex: '20000', border: '1px solid var(--border)', borderRadius: '12px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const heading = document.createElement('h2'); heading.textContent = 'Acesso e retomada · fixture de teste';
    Object.assign(heading.style, { fontSize: '18px', margin: '0 0 8px' });
    const attribution = document.createElement('p');
    attribution.textContent = 'Componente real · recibos sintéticos · captura 1× · não mede a operação nativa';
    Object.assign(attribution.style, { fontSize: '12px', color: 'var(--text-1)', margin: '0 0 20px' });
    const grid = document.createElement('div'); Object.assign(grid.style, { display: 'flex', gap: '16px', justifyContent: 'center' });
    panel.append(heading, attribution, grid); document.body.append(panel);
    const fixture = window.__restoreAccessPreview = { views: {}, receipts: {}, svg: {}, observers: [], events: [],
      application: { loaded: state.loaded, rowCount: state.rows.length }, mode, seed: null, animationIds: new WeakMap(), nextAnimationId: 1 };
    for (const [index, family] of families.entries()) {
      const host = document.createElement('section'); host.id = `restore-access-${family}`;
      Object.assign(host.style, { width: '310px', padding: '10px 8px', border: '1px solid var(--border)', borderRadius: '8px' });
      const label = document.createElement('div'); label.textContent = phases[index];
      Object.assign(label.style, { textAlign: 'center', fontSize: '11px', color: 'var(--text-1)', marginBottom: '6px' });
      host.append(label); grid.append(host);
      const operationId = `restore-access-${family}-fixture`;
      const receipt = WaitingProgress.snapshot({ operationId, phaseId: phases[index], completed: 8, total: 8,
        unit: 'registros', elapsedMs: mode === 'natural' ? 0 : family === 'access' ? 120000 : 60000 });
      let reactionSeed;
      if (family === 'restoration') {
        const model = WaitingVisuals.derive({ ...receipt, elapsedMs: 60000 });
        const episode = mode === 'natural' ? 'visor' : 'manual';
        for (let seed = 0; seed < 4096; seed++) {
          if (WaitingVisuals.createDirector(operationId, seed).boundary(model)?.episode === episode) { reactionSeed = seed; break; }
        }
        if (reactionSeed === undefined) throw Error(`No first-boundary ${episode} seed`);
        fixture.seed = reactionSeed;
      }
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed });
      fixture.views[family] = view; fixture.receipts[family] = receipt; fixture.svg[family] = view.element.querySelector('svg');
      const record = () => ({ family, episode: view.element.dataset.episode, adapter: view.element.dataset.adapter, timeMs: performance.now() });
      let previous = `${view.element.dataset.episode}:${view.element.dataset.adapter}`; fixture.events.push(record());
      const observer = new MutationObserver(() => {
        const key = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
        if (key !== previous) { fixture.events.push(record()); previous = key; }
      });
      observer.observe(view.element, { attributes: true, attributeFilter: ['data-episode', 'data-adapter'] }); fixture.observers.push(observer);
    }
  }, { families, phases, mode });
  await waitMotion(expectedMotion);
}
async function finalizeNaturalVideo() {
  if (results.video?.finalized) return;
  if (!video) throw Error('Access/restoration recording unavailable');
  mark('natural-recording-end'); await context.close(); context = null;
  const filename = 'waiting-restore-access-real-preview.webm'; await video.saveAs(resolve(output, filename));
  Object.assign(results.video, { finalized: true, bytes: statSync(resolve(output, filename)).size, endedOffsetMs: Date.now() - started });
  assert.ok(results.video.bytes > 0 && results.video.bytes < results.limits.evidenceFileLimitBytes,
    'retain the original video and fail if it exceeds the transfer budget');
  await video.delete(); video = null;
}
async function pauseArtwork(label) {
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('static'); await settle();
  const settled = await states(); await page.waitForTimeout(250); const frozen = await states();
  assert.deepEqual(frozen.map(s => s.animations.map(a => a.currentTime)), settled.map(s => s.animations.map(a => a.currentTime)));
  assert.ok(frozen.every(s => s.animations.every(a => !a.pending && a.playState !== 'running')));
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('running'); await page.waitForTimeout(100); const resumed = await states();
  assert.ok(resumed[1].animations.some((a, i) => a.currentTime > settled[1].animations[i].currentTime));
  assertReceipts(resumed); mark(label); return { settled, frozen, resumed };
}



try {
  browser = await launchBrowser({ timeout: 15000 });
  await openApplication(true); await mountFixture('natural');
  phase = 'short gestures without pickup'; mark('natural-short-start');
  await page.waitForFunction(() => Object.values(__restoreAccessPreview.views).every(view => {
    const tracks = view.element.querySelector('.wv-art').getAnimations({ subtree: true });
    return tracks.length > 0 && tracks.every(a => a.currentTime >= 2400 && a.playState === 'finished');
  }));
  results.short = await states(); assertReceipts(results.short);
  assert.ok(results.short.every(s => s.pace === 'gesture' && s.animations.every(a => a.durationMs === 2400 && a.iterations === '1' && a.playbackRate === 1)));
  assert.equal(results.short[1].heldOpacity, 0); assert.equal(results.short[1].seatedOpacity, 1);

  phase = 'same-owner short to long, access remains finite at old global age';
  await page.evaluate(() => {
    const fixture = __restoreAccessPreview;
    for (const family of ['access', 'restoration']) {
      fixture.receipts[family] = { ...fixture.receipts[family], elapsedMs: family === 'access' ? 120000 : 4000 };
      fixture.views[family].update(fixture.receipts[family]);
    }
  });
  mark('natural-long-start');
  await page.waitForFunction(() => __restoreAccessPreview.views.restoration.inspect().cycle >= 1);
  results.long = await states(); assertReceipts(results.long);
  assert.equal(results.long[1].pace, 'loop'); assert.equal(results.long[1].episode, 'work');
  assert.ok(results.long[1].animations.every(a => a.durationMs === 7200 && a.iterations === 'Infinity' && a.playbackRate === 1));
  assert.deepEqual(results.long[0].animations, results.short[0].animations, 'global age never restarts access or gives it a loop');
  results.noReactionYet = await page.evaluate(() => __restoreAccessPreview.views.restoration.inspect());
  assert.deepEqual(results.noReactionYet.history, []);

  phase = 'one natural contextual visor and complete return';
  await page.evaluate(() => {
    const fixture = __restoreAccessPreview; fixture.receipts.restoration = { ...fixture.receipts.restoration, elapsedMs: 60000 };
    fixture.views.restoration.update(fixture.receipts.restoration);
  });
  await waitEpisode('visor'); mark('natural-visor-start');
  results.visorStarted = await states(); assertReceipts(results.visorStarted);
  assert.equal(results.visorStarted[1].heldOpacity, 0); assert.equal(results.visorStarted[1].seatedOpacity, 1);
  assert.equal(results.visorStarted[1].adapter, 'react');
  const visorTracks = results.visorStarted[1].animations.filter(a => /^(?:wv-visor-|wv-episode-boundary)/.test(a.name));
  assert.ok(visorTracks.length > 4 && visorTracks.every(a => a.durationMs === 4800 && a.iterations === '1' && a.playbackRate === 1));
  await snap('waiting-restore-access-natural-visor-dark.png', 'uninterrupted real contextual visor, N/N still running');
  await waitEpisode('work', 7800); mark('natural-visor-return');
  results.returned = await states(); assertReceipts(results.returned);
  assert.equal(results.returned[1].kitchenOpacity, 0);
  results.sequence = await page.evaluate(() => ({ events: __restoreAccessPreview.events, seed: __restoreAccessPreview.seed,
    stableSvg: Object.entries(__restoreAccessPreview.views).every(([family, view]) => view.element.querySelector('svg') === __restoreAccessPreview.svg[family]),
    inspections: Object.fromEntries(Object.entries(__restoreAccessPreview.views).map(([family, view]) => [family, view.inspect()])),
    applicationBefore: __restoreAccessPreview.application, applicationAfter: { loaded: state.loaded, rowCount: state.rows.length } }));
  assert.equal(results.sequence.stableSvg, true); assert.deepEqual(results.sequence.applicationAfter, results.sequence.applicationBefore);
  assert.equal(results.sequence.inspections.access.cycle, 0); assert.deepEqual(results.sequence.inspections.access.history, []);
  const events = results.sequence.events.filter(e => e.family === 'restoration');
  assert.deepEqual(events.map(({ episode, adapter }) => [episode, adapter]), [['work', 'work'], ['visor', 'react'], ['work', 'work']]);
  results.visorDurationMs = events[2].timeMs - events[1].timeMs;
  assert.ok(results.visorDurationMs >= 4700 && results.visorDurationMs < 7800);
  await snap('waiting-restore-access-natural-return-dark.png', 'same original SVG and receipts after complete natural return');
  await finalizeNaturalVideo();

  phase = 'separate non-recorded geometry and accessibility diagnostics';
  assert.equal(results.video.finalized, true); await openApplication(false); await mountFixture('diagnostic');
  results.preSamplingPause = await pauseArtwork('product-pause-before-any-seek');
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('static'); await settle();
  results.rig = await scene('restoration').evaluate(async (root, { contract, probes }) => {
    const held = root.querySelector('.wv-restore-held'), seated = root.querySelector('.wv-restore-seated');
    const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
    const matrix = node => { const m = node.getScreenCTM(); return [m.a, m.b, m.c, m.d, m.e, m.f]; };
    const fixedSelectors = ['.wv-restore-station', '.wv-restore-support', '.wv-restore-seated'];
    const rig = { carriedByHand: held.parentElement === root.querySelector('.wv-task-hand'),
      parkedAtStation: seated.parentElement === root.querySelector('.wv-restore-station'),
      actorTransform: root.querySelector('.wv-restore-actor').getAttribute('transform'),
      fixed: Object.fromEntries(fixedSelectors.map(selector => [selector, matrix(root.querySelector(selector))])), contacts: [], samples: [] };
    const animations = root.querySelector('.wv-art').getAnimations({ subtree: true });
    const sampling = await window.__waitingMotionSampling.begin(animations);
    try {
      for (const probe of probes) {
        const { fraction } = probe;
        await sampling.seek(probe.requestedTimeMs);
        const heldOpacity = Number(getComputedStyle(held).opacity), seatedOpacity = Number(getComputedStyle(seated).opacity);
        const inverse = root.querySelector('svg').getScreenCTM().inverse();
        const points = [[91, 55], [94, 50], [111, 50], [111, 62], [94, 62], [99, 55]];
        rig.samples.push({ ...probe, heldOpacity, seatedOpacity,
          timeline: animations.map(animation => ({ name: animation.animationName, target: animation.effect.target.getAttribute('class'),
            currentTime: animation.currentTime, progress: animation.effect.getComputedTiming().progress,
            durationMs: animation.effect.getComputedTiming().duration, easing: animation.effect.getTiming().easing,
            playState: animation.playState, pending: animation.pending,
            ...(animation.animationName === 'wv-restore-held' || animation.animationName === 'wv-restore-seated'
              ? { keyframes: animation.effect.getKeyframes().map(frame => ({ offset: frame.computedOffset, easing: frame.easing, opacity: frame.opacity })) } : {}) })),
          fixed: Object.fromEntries(fixedSelectors.map(selector => [selector, matrix(root.querySelector(selector))])),
          points: heldOpacity === 1 ? points.map(([x, y]) => { const p = point(held, x, y).matrixTransform(inverse); return { x: p.x, y: p.y }; }) : [] });
        if ([.14, .17, .20, .78, .81, .84].includes(fraction) || probe.transferFraction !== undefined) rig.contacts.push({ fraction,
          kind: probe.kind, requestedTimeMs: probe.requestedTimeMs,
          distances: points.map(([x, y]) => { const a = point(held, x, y), b = point(seated, x, y); return Math.hypot(a.x - b.x, a.y - b.y); }) });
      }
      return rig;
    } finally { await sampling.restore(); }
  }, { contract: restorationSampling, probes: restorationProbePlan() });
  assert.equal(results.rig.carriedByHand, true); assert.equal(results.rig.parkedAtStation, true);
  assert.equal(results.rig.actorTransform, 'translate(20 0)');
  assert.equal(results.rig.samples.length, 20); assert.equal(results.rig.contacts.length, 10);
  assert.ok(results.rig.contacts.every(contact => contact.distances.every(d => d < results.limits.contactToleranceCssPx)));
  for (const sample of results.rig.samples) {
    assertRestorationOwnership(sample);
    assert.ok(sample.points.every(p => p.x > 0 && p.x < 164 && p.y > 0 && p.y < 96));
    for (const [selector, baseline] of Object.entries(results.rig.fixed)) assert.ok(sample.fixed[selector].every((v, i) => Math.abs(v - baseline[i]) < results.limits.staticMatrixTolerance));
  }
  const pose = await page.evaluateHandle(async () => {
    const root = __restoreAccessPreview.views.restoration.element;
    const sampling = await __waitingMotionSampling.begin(root.querySelector('.wv-art').getAnimations({ subtree: true }));
    await sampling.seek(7200 * .48); return sampling;
  });
  try {
    await snap('waiting-restore-access-contact-dark.png', 'diagnostic held record, excluded from WebM');
    await setTheme('light'); await snap('waiting-restore-access-contact-light.png', 'same diagnostic pose in light theme');
  } finally { await pose.evaluate(sampling => sampling.restore()); await pose.dispose(); }
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('running'); results.postSamplingPause = await pauseArtwork('product-pause-after-restored-seek');

  phase = 'actual offscreen intersection and reduced motion';
  await page.evaluate(() => { document.querySelector('#waiting-restore-access-fixture').style.top = '200vh'; });
  await waitMotion('static'); await settle(); results.offscreen = await states();
  await page.waitForTimeout(250); results.offscreenFrozen = await states();
  assert.deepEqual(results.offscreenFrozen.map(s => s.animations.map(a => a.currentTime)), results.offscreen.map(s => s.animations.map(a => a.currentTime)));
  await page.evaluate(() => { document.querySelector('#waiting-restore-access-fixture').style.top = '150px'; });
  await waitMotion('running');
  const beforeReduced = await states();
  const identities = samples => samples.map(s => s.animations.map(a => [a.id, a.name]));
  await page.emulateMedia({ reducedMotion: 'reduce' }); await waitMotion('static'); await settle();
  results.reduced = await states();
  assert.deepEqual(identities(results.reduced), identities(beforeReduced), 'reduce retains every CSSAnimation identity');
  assert.ok(results.reduced.every(s => s.animations.length > 0 && s.animations.every(a => a.playState !== 'running' && !a.pending)));
  await page.waitForTimeout(250); results.reducedFrozen = await states();
  assert.deepEqual(results.reducedFrozen.map(s => s.animations.map(a => a.currentTime)), results.reduced.map(s => s.animations.map(a => a.currentTime)));
  assert.deepEqual(identities(results.reducedFrozen), identities(results.reduced));
  assert.deepEqual(results.reducedFrozen.map(s => s.poses), results.reduced.map(s => s.poses), 'SVG poses remain exactly frozen');
  assertReceipts(results.reduced); await snap('waiting-restore-access-reduced-motion-light.png', 'real reduced-motion media policy');
  await page.emulateMedia({ reducedMotion: 'no-preference' }); await waitMotion('running'); await settle();
  await page.waitForTimeout(100); results.reducedResumed = await states();
  assert.deepEqual(identities(results.reducedResumed), identities(beforeReduced), 'resume preserves original identities');
  for (const [index, sample] of results.reducedResumed.entries()) {
    assert.ok(sample.animations.every((a, i) => a.currentTime >= results.reducedFrozen[index].animations[i].currentTime), 'no clock restarts');
    if (sample.family === 'restoration') assert.ok(sample.animations.some((a, i) => a.currentTime > results.reducedFrozen[index].animations[i].currentTime));
  }
  if (beforeReduced[0].animations.every(a => a.playState === 'finished')) {
    assert.deepEqual(results.reducedResumed[0].animations.map(a => a.currentTime), beforeReduced[0].animations.map(a => a.currentTime));
    assert.deepEqual(results.reducedResumed[0].poses, beforeReduced[0].poses, 'completed access gesture must not restart after reduce');
  }
  assertReceipts(results.reducedResumed);

  phase = 'restoration manual uses shared travel, then owner interrupts synchronously';
  await waitEpisode('manual'); results.manualStarted = await states();
  assert.equal(results.manualStarted[1].heldOpacity, 0); assert.equal(results.manualStarted[1].seatedOpacity, 1);
  const manualTracks = results.manualStarted[1].animations.filter(a => /^(?:wv-manual-|wv-coffee-|wv-episode-boundary)/.test(a.name));
  assert.ok(manualTracks.some(a => a.name === 'wv-coffee-travel-checkpoint'));
  assert.ok(manualTracks.length > 15 && manualTracks.every(a => a.durationMs === 32000 && a.iterations === '1' && a.playbackRate === 1));
  results.ownerInterruption = await page.evaluate(() => {
    const fixture = __restoreAccessPreview, view = fixture.views.restoration, root = view.element;
    const before = { episode: root.dataset.episode, svg: root.querySelector('svg') };
    fixture.receipts.restoration = { ...fixture.receipts.restoration, operationId: 'restore-access-new-owner', elapsedMs: 60000 };
    // The mount seed remains fixed; find the outcome for this new owner without forcing CSS events.
    const nextEpisode = WaitingVisuals.createDirector('restore-access-new-owner', fixture.seed)
      .boundary(WaitingVisuals.derive(fixture.receipts.restoration))?.episode;
    view.update(fixture.receipts.restoration);
    return { beforeEpisode: before.episode, afterEpisode: root.dataset.episode, replacedSvg: root.querySelector('svg') !== before.svg,
      history: view.inspect().history, nextEpisode };
  });
  assert.equal(results.ownerInterruption.beforeEpisode, 'manual'); assert.equal(results.ownerInterruption.afterEpisode, 'work');
  assert.equal(results.ownerInterruption.replacedSvg, true); assert.deepEqual(results.ownerInterruption.history, []);

  // Mount two independent production views for immediate error/completed interruption
  // at their first real manual boundary; the old operation no longer owns either view.
  await page.evaluate(() => {
    const fixture = __restoreAccessPreview; fixture.views.restoration.destroy();
    fixture.observers.forEach(observer => observer.disconnect());
    fixture.terminalViews = ['error', 'completed'].map(state => {
      const host = document.createElement('section'); host.style.width = '310px';
      document.querySelector('#restore-access-restoration').append(host);
      const receipt = { ...fixture.receipts.restoration, operationId: 'restore-access-restoration-fixture' };
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed: fixture.seed }); return { state, view, receipt };
    });
  });
  await page.waitForFunction(() => __restoreAccessPreview.terminalViews.every(({ view }) => view.element.dataset.episode === 'manual'));
  results.terminal = await page.evaluate(() => __restoreAccessPreview.terminalViews.map(({ state, view, receipt }) => {
    const root = view.element, before = { episode: root.dataset.episode, state: root.dataset.state };
    view.update({ ...receipt, state });
    return { before, after: { state: root.dataset.state, family: root.dataset.family, motion: root.dataset.motion,
      propsPresent: !!root.querySelector('.wv-restore-held, .wv-manual-held, .wv-kitchen'),
      moving: root.querySelector('.wv-art').getAnimations({ subtree: true }).filter(a => a.playState === 'running').length } };
  }));
  for (const sample of results.terminal) {
    assert.equal(sample.before.episode, 'manual'); assert.equal(sample.before.state, 'running');
    assert.equal(sample.after.family, 'neutral'); assert.equal(sample.after.motion, 'static');
    assert.equal(sample.after.propsPresent, false); assert.equal(sample.after.moving, 0);
  }
  assert.deepEqual(results.terminal.map(s => s.after.state), ['error', 'completed']);
  results.applicationUnchanged = await page.evaluate(() => JSON.stringify(__restoreAccessPreview.application)
    === JSON.stringify({ loaded: state.loaded, rowCount: state.rows.length }));
  assert.equal(results.applicationUnchanged, true);
  await page.evaluate(() => {
    __restoreAccessPreview.observers.forEach(observer => observer.disconnect());
    Object.values(__restoreAccessPreview.views).forEach(view => view.destroy());
    __restoreAccessPreview.terminalViews.forEach(({ view }) => view.destroy());
    document.querySelector('#waiting-restore-access-fixture').remove();
  });
  assert.equal(await roots().count(), 0);
  phase = 'initially reduced static fallback for both new families';
  await page.emulateMedia({ reducedMotion: 'reduce' }); await mountFixture('diagnostic', 'static');
  await settle(); results.initiallyReduced = await states(); assertReceipts(results.initiallyReduced);
  assert.ok(results.initiallyReduced.every(s => s.animations.length === 0 && s.motion === 'static'), 'initial reduce never assigns animation tracks');
  await page.evaluate(() => {
    __restoreAccessPreview.observers.forEach(observer => observer.disconnect());
    Object.values(__restoreAccessPreview.views).forEach(view => view.destroy());
    document.querySelector('#waiting-restore-access-fixture').remove();
  });
  assert.deepEqual(errors, []); results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error); process.exitCode = 1;
  if (page) {
    try { results.failureStates = await states(); } catch {}
    await captureFailure(page, 'waiting-restore-access', error, { phase, errors, results });
  }
} finally {
  try { if (video) await finalizeNaturalVideo(); }
  catch (error) { results.videoError = String(error); results.ok = false; if (!failure) process.exitCode = 1; }
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  clearTimeout(deadline); results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  if (results.elapsedMs >= results.limits.totalTimeoutMs) { results.ok = false; process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-restore-access-results.json'), JSON.stringify(results, null, 2));
  writeFileSync(resolve(output, 'waiting-restore-access-video.json'), JSON.stringify({ evidence: results.evidence, limits: results.limits,
    sourceCommit: results.sourceCommit, video: results.video, markers: results.markers, completed: results.ok === true,
    elapsedMs: results.elapsedMs }, null, 2));
}
