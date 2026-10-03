/* CI-only evidence: node scripts/preview/run-smoke.mjs test-waiting-verification.mjs
   Production WaitingVisuals/WaitingProgress/SVG/CSS inside the actual app; every
   receipt and panel is explicitly synthetic. No native-engine or latency claim.
   The 1x recording is finalized BEFORE pose diagnostics in a separate context. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
import { installMotionSampling } from './motion-sampling.mjs';

const phases = ['metadata-validate', 'metadata-map-validate', 'canonical-verify', 'analytics-verify'];
const expectedLabels = ['Conferindo checkpoint', 'Conferindo índice', 'Conferindo conversão', 'Conferindo seleção'];
const output = resolve('output/playwright'), started = Date.now();
mkdirSync(output, { recursive: true });
const viewport = { width: 1440, height: 960 };
const results = {
  evidence: {
    component: 'Unmodified production WaitingVisuals, WaitingProgress, SVG, CSS and shared reaction director',
    context: 'Actual Chromium preview application with an explicitly labelled synthetic fixture panel',
    receipts: 'Only the four existing verification protocol IDs; labels come from WaitingProgress.snapshot',
    timing: 'Real 1x playback; supplied elapsedMs is synthetic eligibility input, never measured native latency',
    poseSampling: 'CSS-governed diagnostics in a separate non-recorded browser context, after natural video finalization',
    nativeEngineVerified: false, installedWebViewVerified: false, generatedImageAssets: false,
  },
  limits: {
    totalTimeoutMs: 110000, targetRuntimeMs: [65000, 80000],
    naturalHoldBudgetMs: 53800, diagnosticBoundaryBudgetMs: 9600,
    durationsMs: { short: 2600, loop: 9600, coffee: 32000 },
    suppliedElapsedMs: { short: 0, loop: 4000, coffee: 60000 },
    contactToleranceCssPx: .15, staticMatrixTolerance: .00001,
    timingNote: '54s natural footage plus a second 9.6s boundary, diagnostics, navigation and encoding overhead',
  },
  sourceCommit: process.env.GITHUB_SHA || null, phases, markers: [], screenshots: [],
};
const errors = [];
let browser, context, page, video, failure = null, phase = 'startup';
const mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
// Leave ten seconds before run-smoke's hard 120s limit for failed-run diagnostics.
const deadline = setTimeout(() => {
  results.timedOut = true;
  results.ok = false;
  process.exitCode = 1;
  void browser?.close().catch(() => {});
}, results.limits.totalTimeoutMs);
deadline.unref();
const roots = () => page.locator('#waiting-verification-fixture .waiting-visual');
const first = () => roots().first();
const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const states = () => roots().evaluateAll(nodes => nodes.map(root => {
  const art = root.querySelector('.wv-art');
  const opacity = selector => { const node = root.querySelector(selector); return node ? Number(getComputedStyle(node).opacity) : null; };
  return {
    phaseId: root.parentElement.dataset.phaseId, family: root.dataset.family, pace: root.dataset.pace,
    state: root.dataset.state, episode: root.dataset.episode, adapter: root.dataset.adapter, motion: root.dataset.motion,
    status: root.querySelector('.wv-status').textContent, metric: root.querySelector('.wv-metric').textContent,
    svgCount: art.querySelectorAll('svg').length, kitchenOpacity: opacity('.wv-kitchen'),
    heldOpacity: opacity('.wv-verify-held'), dockedOpacity: opacity('.wv-verify-docked'),
    cupOpacity: opacity('.wv-cup-held'), steamOpacity: opacity('.wv-cup-held .wv-cup-steam'),
    animations: art.getAnimations({ subtree: true }).map(animation => ({
      name: animation.animationName, target: animation.effect.target.getAttribute('class'),
      durationMs: animation.effect.getComputedTiming().duration, iterations: String(animation.effect.getTiming().iterations),
      currentTime: animation.currentTime, playbackRate: animation.playbackRate, playState: animation.playState, pending: animation.pending,
    })),
  };
}));
const snap = async (filename, kind) => {
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  results.screenshots.push({ filename, kind, phase, offsetMs: Date.now() - started,
    viewport: page.viewportSize(), theme: await page.evaluate(() => document.documentElement.dataset.theme) });
};
const settlePausedArtwork = () => roots().evaluateAll(async nodes => {
  const animations = nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true }));
  await Promise.all(animations.map(animation => animation.ready));
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
});
const waitMotion = motion => page.waitForFunction(motion => {
  const nodes = [...document.querySelectorAll('#waiting-verification-fixture .waiting-visual')];
  return nodes.length > 0 && nodes.every(root => root.dataset.motion === motion);
}, motion);
const waitEpisode = (episode, timeout) => page.waitForFunction(episode => {
  const root = document.querySelector('#waiting-verification-fixture .waiting-visual');
  return root?.dataset.episode === episode && root.dataset.adapter === (episode === 'coffee' ? 'react' : 'work');
}, episode, { timeout });
const setTheme = async theme => {
  await page.evaluate(async theme => {
    if (document.documentElement.dataset.theme !== theme) toggleTheme();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const transitions = document.getAnimations().filter(animation => animation instanceof CSSTransition);
    await Promise.all(transitions.map(animation => animation.finished.catch(() => {})));
  }, theme);
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), theme);
};
async function openApplication(recording) {
  context = await browser.newContext({ viewport, reducedMotion: 'no-preference',
    ...(recording ? { recordVideo: { dir: resolve(output, 'video-raw'), size: viewport } } : {}) });
  page = await context.newPage();
  if (recording) {
    video = page.video();
    results.video = { filename: 'waiting-verification-real-preview.webm', finalized: false, recordedSize: viewport,
      startedOffsetMs: Date.now() - started, playbackRate: 1,
      timingReference: 'Approximate wall-clock markers since script startup; inspect the video before trimming' };
  }
  await page.addInitScript(installMotionSampling);
  page.setDefaultTimeout(12000);
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden && window.WaitingVisuals && window.WaitingProgress);
  await page.waitForFunction(() => Tasks.pending() === 0);
  await page.evaluate(() => UiScale.set(1, false));
  await setTheme('dark');
}
async function mountFixture(mode) {
  await page.evaluate(({ phases, mode }) => {
    if (window.__verificationPreview) {
      __verificationPreview.observers.forEach(observer => observer.disconnect());
      __verificationPreview.views.forEach(view => view.destroy());
      document.querySelector('#waiting-verification-fixture').remove();
    }
    const panel = document.createElement('section'); panel.id = 'waiting-verification-fixture';
    panel.setAttribute('aria-label', 'Fixture de conferência: dados sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '90px auto auto 50%', transform: 'translateX(-50%)',
      width: '740px', maxWidth: 'calc(100vw - 32px)', boxSizing: 'border-box', padding: '24px', zIndex: '20000',
      border: '1px solid var(--border)', borderRadius: '12px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const heading = document.createElement('h2'); heading.textContent = 'Conferência · fixture de teste';
    Object.assign(heading.style, { fontSize: '18px', margin: '0 0 8px' });
    const attribution = document.createElement('p');
    attribution.textContent = 'Componente real · dados e recibos sintéticos · captura 1× · não mede a operação nativa';
    Object.assign(attribution.style, { fontSize: '12px', color: 'var(--text-1)', margin: '0 0 20px' });
    const grid = document.createElement('div');
    Object.assign(grid.style, { display: 'flex', gap: '16px', justifyContent: 'center', flexWrap: 'wrap' });
    panel.append(heading, attribution, grid); document.body.append(panel);
    const fixture = window.__verificationPreview = { views: [], receipts: [], observers: [], events: [], svg: [], naturalEnds: [], mode, seed: null };
    for (const phaseId of mode === 'coffee' ? [phases[0]] : phases) {
      const host = document.createElement('section'); host.dataset.phaseId = phaseId;
      Object.assign(host.style, { width: '310px', padding: '10px 8px', border: '1px solid var(--border)', borderRadius: '8px' });
      const idLabel = document.createElement('div'); idLabel.textContent = phaseId;
      Object.assign(idLabel.style, { textAlign: 'center', fontSize: '11px', color: 'var(--text-1)', marginBottom: '6px' });
      host.append(idLabel); grid.append(host);
      const operationId = mode === 'coffee' ? 'verification-coffee-fixture' : `verification-${mode}-${phaseId}`;
      const elapsedMs = mode === 'coffee' ? 60000 : mode === 'loop' ? 4000 : 0;
      const receipt = WaitingProgress.snapshot({ operationId, phaseId, completed: 4, total: 4, unit: 'registros', elapsedMs });
      let reactionSeed;
      if (mode === 'coffee') {
        const model = WaitingVisuals.derive(receipt);
        // Real public pure director chooses the seed; no dispatch of fake animation events.
        for (let seed = 0; seed < 4096; seed++) {
          if (WaitingVisuals.createDirector(operationId, seed).boundary(model)?.episode === 'coffee') { reactionSeed = seed; break; }
        }
        if (reactionSeed === undefined) throw Error('No first-boundary coffee seed found');
        fixture.seed = reactionSeed;
      }
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed });
      fixture.views.push(view); fixture.receipts.push(receipt); fixture.svg.push(view.element.querySelector('svg'));
      view.element.addEventListener('animationend', event => {
        if (event.animationName !== 'wv-gesture-boundary' || event.target !== view.element.querySelector('.wv-work-boundary')) return;
        const animations = view.element.querySelector('.wv-art').getAnimations({ subtree: true });
        fixture.naturalEnds.push({ phaseId, isTrusted: event.isTrusted, elapsedTime: event.elapsedTime,
          pace: view.element.dataset.pace, stableSvg: view.element.querySelector('svg') === fixture.svg[fixture.views.indexOf(view)],
          heldOpacity: Number(getComputedStyle(view.element.querySelector('.wv-verify-held')).opacity),
          dockedOpacity: Number(getComputedStyle(view.element.querySelector('.wv-verify-docked')).opacity),
          times: animations.map(a => a.currentTime), durationsMs: [...new Set(animations.map(a => a.effect.getComputedTiming().duration))],
          iterations: [...new Set(animations.map(a => String(a.effect.getTiming().iterations)))],
          playbackRates: [...new Set(animations.map(a => a.playbackRate))] });
      }, true);
      const record = () => ({ phaseId, episode: view.element.dataset.episode, adapter: view.element.dataset.adapter, timeMs: performance.now() });
      fixture.events.push(record());
      let previous = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
      const observer = new MutationObserver(() => {
        const key = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
        if (key !== previous) { fixture.events.push(record()); previous = key; }
      });
      observer.observe(view.element, { attributes: true, attributeFilter: ['data-episode', 'data-adapter'] });
      fixture.observers.push(observer);
    }
  }, { phases, mode });
  await waitMotion('running');
}
async function recordNaturalCycle(durationMs) {
  return roots().evaluateAll(async (nodes, durationMs) => {
    const animations = nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true }));
    if (!animations.length) throw Error('Verification artwork has no CSS tracks');
    const started = performance.now();
    return await new Promise((resolve, reject) => {
      const frame = () => {
        if (nodes.some(root => !root.isConnected || root.dataset.motion !== 'running' || root.dataset.episode !== 'work'))
          return reject(Error('Verification changed or reacted during its natural work capture'));
        if (animations.some(animation => animation.playbackRate !== 1)) return reject(Error('Capture playback rate is not 1x'));
        const finite = durationMs === 2600;
        const ends = __verificationPreview.naturalEnds;
        const times = finite ? ends.flatMap(end => end.times) : animations.map(animation => animation.currentTime);
        if ((finite ? ends.length === nodes.length : times.every(time => typeof time === 'number' && time >= durationMs))) return resolve({
          elapsedMs: performance.now() - started, minimumTimelineMs: Math.min(...times), maximumTimelineMs: Math.max(...times),
          durationsMs: finite ? [...new Set(ends.flatMap(end => end.durationsMs))] : [...new Set(animations.map(animation => animation.effect.getComputedTiming().duration))],
          iterations: finite ? [...new Set(ends.flatMap(end => end.iterations))] : [...new Set(animations.map(animation => String(animation.effect.getTiming().iterations)))],
          playbackRates: finite ? [...new Set(ends.flatMap(end => end.playbackRates))] : [...new Set(animations.map(animation => animation.playbackRate))],
          tracks: animations.map(animation => animation.animationName), endpoints: finite ? ends : [],
          inspections: __verificationPreview.views.map(view => view.inspect()),
        });
        if (performance.now() - started > durationMs + 3000) return reject(Error('Natural work cycle did not finish within its budget'));
        requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    });
  }, durationMs);
}
function assertWorkStates(states, pace) {
  assert.deepEqual(states.map(state => state.phaseId), phases);
  assert.deepEqual(states.map(state => state.status), expectedLabels);
  for (const state of states) {
    assert.equal(state.family, 'verification'); assert.equal(state.state, 'running'); assert.equal(state.pace, pace);
    assert.equal(state.episode, 'work'); assert.equal(state.adapter, 'work'); assert.equal(state.svgCount, 1);
    assert.equal(state.kitchenOpacity, 0, 'the coffee nook is invisible outside coffee');
    assert.equal(state.metric, '4 / 4 registros', 'full counters do not fabricate completed verification');
    assert.doesNotMatch(state.status, /sucesso|aprovad|falhou|100%/i);
  }
}
async function finalizeNaturalVideo() {
  if (results.video?.finalized) return;
  if (!video) throw Error('Verification recording is unavailable');
  mark('natural-recording-end');
  await context.close(); context = null;
  const filename = 'waiting-verification-real-preview.webm';
  await video.saveAs(resolve(output, filename));
  Object.assign(results.video, { filename, bytes: statSync(resolve(output, filename)).size, finalized: true,
    endedOffsetMs: Date.now() - started });
  assert.ok(results.video.bytes > 0);
  await video.delete(); video = null;
}
async function pauseCoffee(label) {
  const before = (await states())[0];
  await first().locator('.wv-motion-toggle').click();
  const requested = (await states())[0];
  assert.equal(requested.motion, 'static');
  assert.ok(requested.animations.every(animation => animation.playState !== 'running'));
  await settlePausedArtwork();
  const settled = (await states())[0];
  assert.ok(settled.animations.every(animation => !animation.pending));
  await page.waitForTimeout(250);
  const frozen = (await states())[0];
  assert.deepEqual(frozen.animations.map(animation => animation.currentTime), settled.animations.map(animation => animation.currentTime),
    `${label}: product CSS freezes all tracks, including steam and boundary signals`);
  assert.equal(frozen.episode, 'coffee');
  await first().locator('.wv-motion-toggle').click();
  await page.waitForTimeout(150);
  const resumed = (await states())[0];
  assert.equal(resumed.motion, 'running'); assert.equal(resumed.episode, 'coffee');
  assert.ok(resumed.animations.some(animation => animation.playState === 'running'));
  assert.ok(resumed.animations.some((animation, index) => animation.currentTime > settled.animations[index].currentTime));
  mark(label);
  return { before, requested, settled, frozen, resumed };
}

try {
  browser = await launchBrowser({ timeout: 15000 });
  await openApplication(true);
  phase = 'natural short gesture';
  await mountFixture('short');
  mark('short-natural-start');
  results.short = await recordNaturalCycle(2600);
  mark('short-natural-end');
  results.short.states = await states();
  assertWorkStates(results.short.states, 'loop');
  assert.deepEqual(results.short.durationsMs, [2600]); assert.deepEqual(results.short.iterations, ['1']);
  assert.ok(results.short.minimumTimelineMs >= 2600 && results.short.elapsedMs >= 2500);
  assert.ok(results.short.endpoints.every(end => end.isTrusted && end.elapsedTime === 2.6 && end.pace === 'gesture' && end.stableSvg));
  assert.ok(results.short.endpoints.every(end => end.heldOpacity === 0 && end.dockedOpacity === 1));
  assert.ok(results.short.states.every(state => state.animations.every(a => a.durationMs === 9600 && a.iterations === 'Infinity')));
  await snap('waiting-verification-short-dark.png', 'natural 2.6s gesture endpoint followed automatically by continuous work');

  phase = 'natural full verification loop';
  await mountFixture('loop');
  mark('loop-natural-start');
  results.loop = await recordNaturalCycle(9600);
  mark('loop-natural-end');
  results.loop.states = await states();
  assertWorkStates(results.loop.states, 'loop');
  assert.deepEqual(results.loop.durationsMs, [9600]); assert.deepEqual(results.loop.iterations, ['Infinity']);
  assert.ok(results.loop.minimumTimelineMs >= 9600 && results.loop.elapsedMs >= 9500);
  assert.ok(results.loop.inspections.every(inspection => inspection.history.length === 0), 'first work cycle has no early reactions');
  await snap('waiting-verification-loop-natural-dark.png', 'natural completed 9.6s loop');

  phase = 'natural shared coffee and return';
  await mountFixture('coffee');
  results.coffee = { initial: (await states())[0], seed: await page.evaluate(() => __verificationPreview.seed) };
  assert.equal(results.coffee.initial.kitchenOpacity, 0);
  mark('coffee-work-boundary-start');
  await waitEpisode('coffee', 13000);
  mark('coffee-natural-start');
  results.coffee.start = (await states())[0];
  assert.equal(results.coffee.start.adapter, 'react', 'verification needs no prepare bridge');
  const coffeeTracks = results.coffee.start.animations.filter(animation => /^(?:wv-coffee-|wv-episode-boundary)/.test(animation.name));
  assert.ok(coffeeTracks.length > 10); assert.ok(coffeeTracks.every(animation => animation.durationMs === 32000 && animation.playbackRate === 1));
  assert.ok(coffeeTracks.some(animation => animation.name === 'wv-coffee-travel-checkpoint'));
  // Watch the natural frame; no seek, pause, clock override or pose sampling.
  await page.waitForFunction(() => {
    const root = __verificationPreview.views[0].element;
    const clock = root.querySelector('.wv-episode-boundary').getAnimations()[0];
    return root.dataset.episode === 'coffee' && clock?.currentTime >= 11000;
  }, null, { timeout: 14000 });
  results.coffee.carried = (await states())[0];
  assert.equal(results.coffee.carried.cupOpacity, 1); assert.equal(results.coffee.carried.kitchenOpacity, 1);
  assert.ok(results.coffee.carried.steamOpacity > 0);
  assert.equal(results.coffee.carried.status, expectedLabels[0]); assert.equal(results.coffee.carried.metric, '4 / 4 registros');
  await snap('waiting-verification-coffee-natural-dark.png', 'natural 32s shared coffee episode');
  await waitEpisode('work', 24000);
  mark('coffee-natural-return');
  results.coffee.sequence = await page.evaluate(() => ({
    events: __verificationPreview.events,
    stableSvg: __verificationPreview.views[0].element.querySelector('svg') === __verificationPreview.svg[0],
    inspection: __verificationPreview.views[0].inspect(),
  }));
  const events = results.coffee.sequence.events;
  assert.deepEqual(events.map(({ episode, adapter }) => [episode, adapter]), [['work', 'work'], ['coffee', 'react'], ['work', 'work']]);
  assert.equal(results.coffee.sequence.stableSvg, true);
  assert.equal(results.coffee.sequence.inspection.cycle, 1, 'seed selects coffee at the first actual work boundary');
  results.coffee.naturalDurationMs = events[2].timeMs - events[1].timeMs;
  assert.ok(results.coffee.naturalDurationMs >= 31900 && results.coffee.naturalDurationMs < 35000);
  results.coffee.returned = (await states())[0];
  assert.equal(results.coffee.returned.kitchenOpacity, 0);
  assert.equal(results.coffee.returned.family, 'verification'); assert.equal(results.coffee.returned.motion, 'running');
  assert.equal(results.coffee.returned.heldOpacity, 0); assert.equal(results.coffee.returned.dockedOpacity, 1);
  await snap('waiting-verification-returned-dark.png', 'natural return to the same verification scene');
  await finalizeNaturalVideo();

  // Everything below is excluded from the finished 1x WebM, including seeks,
  // contacts, CSS pauses and theme/accessibility fixtures.
  phase = 'diagnostic context after finalized natural video';
  assert.equal(results.video.finalized, true);
  await openApplication(false);
  mark('diagnostics-start-after-video-finalized');
  await mountFixture('loop');
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('static'); await settlePausedArtwork();
  results.rig = await roots().evaluateAll(async nodes => {
    const animations = nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true }));
    const point = (node, x, y) => { const p = new DOMPoint(x, y).matrixTransform(node.getScreenCTM()); return { x: p.x, y: p.y }; };
    const matrix = node => { const m = node.getScreenCTM(); return [m.a, m.b, m.c, m.d, m.e, m.f]; };
    const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const fixedSelectors = ['.wv-verify-station', '.wv-verify-cradle', '.wv-verify-document', '.wv-verify-reference', '.wv-verify-docked'];
    const rigs = nodes.map(root => ({
      phaseId: root.parentElement.dataset.phaseId,
      carriedByHand: root.querySelector('.wv-verify-held').parentElement === root.querySelector('.wv-task-hand'),
      parkedAtStation: root.querySelector('.wv-verify-docked').parentElement === root.querySelector('.wv-verify-station'),
      actorTransform: root.querySelector('.wv-verify-actor').getAttribute('transform'),
      actorCount: root.querySelectorAll('.wv-verify-actor .wv-task').length,
      art: root.querySelector('.wv-art').getBoundingClientRect().toJSON(),
      fixed: Object.fromEntries(fixedSelectors.map(selector => [selector, matrix(root.querySelector(selector))])),
      contacts: [], samples: [],
    }));
    const sampling = await window.__waitingMotionSampling.begin(animations);
    try {
      for (const fraction of [0, .16, .17999, .18, .20, .34, .40, .48, .54, .62, .68, .78, .81999, .82, .84, .94, .99999]) {
        await sampling.seek(9600 * fraction);
        for (const [index, root] of nodes.entries()) {
          const held = root.querySelector('.wv-verify-held'), docked = root.querySelector('.wv-verify-docked');
          const heldOpacity = Number(getComputedStyle(held).opacity), dockedOpacity = Number(getComputedStyle(docked).opacity);
          const lensPoints = [];
          if (heldOpacity === 1) {
            const inverse = root.querySelector('svg').getScreenCTM().inverse();
            for (let angle = 0; angle < 360; angle += 30) {
              const radians = angle * Math.PI / 180;
              const screen = point(held, 102 + 7.2 * Math.cos(radians), 46 + 7.2 * Math.sin(radians));
              const local = new DOMPoint(screen.x, screen.y).matrixTransform(inverse);
              lensPoints.push({ x: local.x, y: local.y });
            }
          }
          rigs[index].samples.push({ fraction, heldOpacity, dockedOpacity, lensPoints,
            fixed: Object.fromEntries(fixedSelectors.map(selector => [selector, matrix(root.querySelector(selector))])) });
          if ([.16, .18, .20, .78, .82, .84].includes(fraction)) rigs[index].contacts.push({ fraction,
            distances: [[91, 55], [102, 46], [95.7, 46], [108.3, 46], [102, 39.7]]
              .map(([x, y]) => distance(point(held, x, y), point(docked, x, y))) });
        }
      }
      return rigs;
    } finally {
      await sampling.restore();
    }
  });
  for (const rig of results.rig) {
    assert.equal(rig.carriedByHand, true); assert.equal(rig.parkedAtStation, true); assert.equal(rig.actorCount, 1);
    assert.equal(rig.actorTransform, 'translate(20 0)');
    assert.ok(Math.abs(rig.art.width - 240) < .5 && Math.abs(rig.art.height - 120) < .5);
    assert.ok(rig.contacts.every(contact => contact.distances.every(distance => distance < results.limits.contactToleranceCssPx)),
      `${rig.phaseId}: the complete lens coincides throughout both 18%/82% contact holds`);
    for (const sample of rig.samples) {
      assert.equal(sample.heldOpacity + sample.dockedOpacity, 1, 'exactly one lens is visible');
      assert.equal(sample.heldOpacity, sample.fraction >= .18 && sample.fraction < .82 ? 1 : 0);
      for (const [selector, baseline] of Object.entries(rig.fixed)) assert.ok(sample.fixed[selector].every((value, i) =>
        Math.abs(value - baseline[i]) < results.limits.staticMatrixTolerance), `${selector} never moves`);
      assert.ok(sample.lensPoints.every(point => point.x > 0 && point.x < 164 && point.y > 0 && point.y < 96),
        'the held tool stays inside the scene and clear of the coffee hatch');
    }
  }
  results.afterLensSampling = await states();
  assert.ok(results.afterLensSampling.every(state => state.motion === 'static' && state.animations.every(animation => animation.playState !== 'running')));

  phase = 'theme and reduced-motion screenshots';
  const pose = await page.evaluateHandle(() => window.__waitingMotionSampling.begin(
    [...document.querySelectorAll('#waiting-verification-fixture .waiting-visual')]
      .flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true }))));
  try {
    await pose.evaluate(sample => sample.seek(9600 * .48));
    await snap('waiting-verification-inspection-dark.png', 'diagnostic 48% pose, excluded from video');
    await setTheme('light');
    await snap('waiting-verification-inspection-light.png', 'diagnostic 48% pose, excluded from video');
  } finally {
    try { await pose.evaluate(sample => sample.restore()); }
    finally { await pose.dispose(); }
  }
  for (const control of await roots().locator('.wv-motion-toggle').all()) await control.click();
  await waitMotion('running');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await waitMotion('static'); await settlePausedArtwork();
  results.reducedMotion = await states();
  assert.ok(results.reducedMotion.every(state => state.animations.every(animation => animation.playState !== 'running')));
  assert.equal(await roots().locator('.wv-motion-toggle:visible').count(), 0);
  await snap('waiting-verification-reduced-motion-light.png', 'real prefers-reduced-motion policy');
  await setTheme('dark');
  await snap('waiting-verification-reduced-motion-dark.png', 'real prefers-reduced-motion policy');
  await page.emulateMedia({ reducedMotion: 'no-preference' }); await waitMotion('running');

  phase = 'second coffee fixture pause and rig';
  await mountFixture('coffee');
  await waitEpisode('coffee', 13000);
  results.coffee.preSamplingPause = await pauseCoffee('coffee-product-pause-before-coffee-poses');
  results.coffee.rig = await first().evaluate(async root => {
    const art = root.querySelector('.wv-art'), held = root.querySelector('.wv-cup-held'), hand = root.querySelector('.wv-react-hand');
    const shelf = root.querySelector('.wv-cup-shelf'), wrist = root.querySelector('.wv-cup-wrist');
    const steam = held.querySelector('.wv-cup-steam');
    const tracks = art.getAnimations({ subtree: true }).filter(animation => /^(?:wv-coffee-|wv-episode-boundary)/.test(animation.animationName));
    const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
    const result = { carriedByHand: held.parentElement === hand, steamOwnedByWrist: steam.parentElement === wrist && wrist.parentElement === held,
      trackNames: tracks.map(animation => animation.animationName), durationMs: [...new Set(tracks.map(animation => animation.effect.getComputedTiming().duration))], contacts: [] };
    const sampling = await window.__waitingMotionSampling.begin(tracks);
    try {
      for (const fraction of [.28125, .828125]) {
        await sampling.seek(32000 * fraction);
        result.contacts.push({ fraction, distances: [[92, 50], [99, 58]].map(([x, y]) => {
          const a = point(held, x, y), b = point(shelf, x, y); return Math.hypot(a.x - b.x, a.y - b.y);
        }) });
      }
      await sampling.seek(32000 * .5);
      result.carried = { heldOpacity: Number(getComputedStyle(held).opacity), steamOpacity: Number(getComputedStyle(steam).opacity),
        kitchenOpacity: Number(getComputedStyle(root.querySelector('.wv-kitchen')).opacity),
        stationVisible: getComputedStyle(root.querySelector('.wv-verify-station')).visibility,
        workActorHidden: getComputedStyle(root.querySelector('.wv-verify-actor')).visibility === 'hidden' };
      return result;
    } finally {
      await sampling.restore();
    }
  });
  assert.equal(results.coffee.rig.carriedByHand, true); assert.equal(results.coffee.rig.steamOwnedByWrist, true);
  assert.deepEqual(results.coffee.rig.durationMs, [32000]);
  assert.ok(results.coffee.rig.trackNames.includes('wv-coffee-travel-checkpoint'));
  assert.ok(results.coffee.rig.trackNames.includes('wv-coffee-steam'));
  assert.ok(results.coffee.rig.contacts.every(contact => contact.distances.every(distance => distance < .2)));
  assert.equal(results.coffee.rig.carried.heldOpacity, 1); assert.ok(results.coffee.rig.carried.steamOpacity > 0);
  assert.equal(results.coffee.rig.carried.kitchenOpacity, 1); assert.equal(results.coffee.rig.carried.stationVisible, 'visible');
  assert.equal(results.coffee.rig.carried.workActorHidden, true);
  // Exercise CSS pause while steam and the held cup are actually visible. This
  // explicitly diagnostic pose restores its original clock in full afterwards.
  const carriedPose = await first().evaluateHandle(root => window.__waitingMotionSampling.begin(
    root.querySelector('.wv-art').getAnimations({ subtree: true })
      .filter(animation => /^(?:wv-coffee-|wv-episode-boundary)/.test(animation.animationName))));
  try {
    await carriedPose.evaluate(sample => sample.seek(32000 * .5));
    await carriedPose.evaluate(sample => sample.release());
    results.coffee.postSamplingPause = await pauseCoffee('coffee-product-pause-after-coffee-poses');
    assert.equal(results.coffee.postSamplingPause.settled.cupOpacity, 1);
    assert.equal(results.coffee.postSamplingPause.settled.kitchenOpacity, 1);
    assert.ok(results.coffee.postSamplingPause.settled.steamOpacity > 0);
  } finally {
    try { await carriedPose.evaluate(sample => sample.restore()); }
    finally { await carriedPose.dispose(); }
  }

  phase = 'immediate error interrupts coffee';
  results.errorInterruption = await page.evaluate(() => {
    const view = __verificationPreview.views[0], before = { episode: view.element.dataset.episode, timeMs: performance.now() };
    view.update(WaitingProgress.snapshot({ ...__verificationPreview.receipts[0], error: 'Falha sintética de conferência' }));
    // Same synchronous task: no frame, timeout, animation event or coffee finish is awaited.
    return { before, after: { timeMs: performance.now(), state: view.element.dataset.state, family: view.element.dataset.family,
      episode: view.element.dataset.episode, adapter: view.element.dataset.adapter, motion: view.element.dataset.motion,
      status: view.element.querySelector('.wv-status').textContent, kitchenPresent: !!view.element.querySelector('.wv-kitchen'),
      moving: view.element.querySelector('.wv-art').getAnimations({ subtree: true }).filter(animation => animation.playState === 'running').length,
      inspection: view.inspect() }, synchronous: true };
  });
  assert.equal(results.errorInterruption.before.episode, 'coffee');
  const after = results.errorInterruption.after;
  assert.equal(after.state, 'error'); assert.equal(after.family, 'neutral'); assert.equal(after.episode, 'work');
  assert.equal(after.adapter, 'work'); assert.equal(after.motion, 'static'); assert.equal(after.moving, 0); assert.equal(after.kitchenPresent, false);
  assert.match(after.status, /Não foi possível continuar.*Falha sintética de conferência/);
  await snap('waiting-verification-error-dark.png', 'immediate interruption in the real component');
  await page.evaluate(() => {
    __verificationPreview.observers.forEach(observer => observer.disconnect());
    __verificationPreview.views.forEach(view => view.destroy());
    document.querySelector('#waiting-verification-fixture').remove();
  });
  await frames();
  assert.equal(await roots().count(), 0); assert.deepEqual(errors, []);
  results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error);
  if (page) {
    try { results.failureStates = await states(); } catch {}
    await captureFailure(page, 'waiting-verification', error, { phase, errors, results });
  }
  process.exitCode = 1;
} finally {
  try {
    if (video) await finalizeNaturalVideo();
  } catch (error) { results.videoError = String(error); results.ok = false; if (!failure) process.exitCode = 1; }
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  clearTimeout(deadline);
  results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  if (results.elapsedMs >= results.limits.totalTimeoutMs) { results.ok = false; process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-verification-results.json'), JSON.stringify(results, null, 2));
  writeFileSync(resolve(output, 'waiting-verification-video.json'), JSON.stringify({ evidence: results.evidence,
    limits: results.limits, sourceCommit: results.sourceCommit, video: results.video, markers: results.markers,
    completed: results.ok === true, elapsedMs: results.elapsedMs }, null, 2));
}
