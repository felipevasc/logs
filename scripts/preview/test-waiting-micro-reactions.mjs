/* CI-only: node scripts/preview/run-smoke.mjs test-waiting-micro-reactions.mjs
   Four labelled synthetic cards reuse the real component/director/rig. A bounded
   natural recording closes before diagnostics; no product or other preview edits.
   Continuity diagnostics cover existing timelines across reduced-motion changes. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
import { installMotionSampling } from './motion-sampling.mjs';

const cards = [
  { key: 'reading-visor', family: 'reading', episode: 'visor', phaseId: 'metadata-scan', durationMs: 4800 },
  { key: 'reading-wave', family: 'reading', episode: 'wave', phaseId: 'metadata-scan', durationMs: 3600 },
  { key: 'composition-visor', family: 'composition', episode: 'visor', phaseId: 'command:case_report_render', durationMs: 4800 },
  { key: 'composition-wave', family: 'composition', episode: 'wave', phaseId: 'command:case_report_render', durationMs: 3600 },
];
const output = resolve('output/playwright'), started = Date.now();
const viewport = { width: 1120, height: 760 };
const filename = 'waiting-micro-reactions-real-preview.webm';
mkdirSync(output, { recursive: true });
const results = {
  evidence: {
    component: 'Production WaitingVisuals, SVG, CSS, director and existing preparation/return adapters',
    context: 'Real Chromium application, four explicitly labelled synthetic cards; two families and two reactions',
    timing: 'Natural work -> gesture -> work at 1x, with complete composition bridges; no diagnostic seeks in WebM',
    eligibility: 'Supplied elapsedMs=60000 is synthetic input, not native latency or progress',
    diagnostics: 'Separate non-recorded context after WebM finalization; sampled poses are not natural playback',
    reducedMotion: 'Existing CSSAnimation identities and clocks survive preference changes; static clocks freeze exactly and the same microepisodes resume',
    nativeEngineVerified: false, installedWebViewVerified: false, generatedImageAssets: false,
  },
  limits: { totalTimeoutMs: 90000, naturalBudgetMs: 22000, recordingMaxMs: 30000,
    videoMaxBytes: 6_000_000, seedSearchLimit: 4096, contactToleranceCssPx: .15,
    fallbackArtifact: 'preview-recordings-other', artifactTransferCapBytes: 32_000_000,
    knownCompanionBudgetBytes: 5_614_425 + 6_000_000 },
  sourceCommit: process.env.GITHUB_SHA || null, cards, markers: [], screenshots: [],
};
let browser, context, page, video, recordingTimer, failure = null, phase = 'startup';
const errors = [], mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
const deadline = setTimeout(() => {
  results.timedOut = true; results.ok = false; process.exitCode = 1;
  void browser?.close().catch(() => {});
}, results.limits.totalTimeoutMs);
deadline.unref();
const roots = () => page.locator('#waiting-micro-fixture .waiting-visual');
const states = () => roots().evaluateAll(nodes => nodes.map(root => ({
  key: root.parentElement.dataset.card, family: root.dataset.family, episode: root.dataset.episode,
  adapter: root.dataset.adapter, motion: root.dataset.motion, state: root.dataset.state,
  status: root.querySelector('.wv-status').textContent, metric: root.querySelector('.wv-metric').textContent,
  svgCount: root.querySelectorAll('svg').length,
  poses: [...root.querySelectorAll('.wv-art svg g')].map(node => ({ target: node.getAttribute('class'),
    transform: getComputedStyle(node).transform, opacity: Number(getComputedStyle(node).opacity), visibility: getComputedStyle(node).visibility })),
  animations: root.querySelector('.wv-art').getAnimations({ subtree: true }).map(a => {
    const fixture = __microPreview;
    if (!fixture.animationIds.has(a)) fixture.animationIds.set(a, ++fixture.nextAnimationId);
    return { id: fixture.animationIds.get(a), name: a.animationName, target: a.effect.target.getAttribute('class'), currentTime: a.currentTime,
      durationMs: a.effect.getComputedTiming().duration, iterations: String(a.effect.getTiming().iterations),
      playbackRate: a.playbackRate, playState: a.playState, pending: a.pending };
  }),
})));
const identity = sample => sample.animations.map(a => [a.id, a.name, a.target, a.durationMs, a.iterations, a.playbackRate]);
const settle = () => roots().evaluateAll(async nodes => {
  await Promise.all(nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true })).map(a => a.ready));
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
});
const snap = async (name, kind) => {
  await page.screenshot({ path: resolve(output, name), animations: 'allow' });
  results.screenshots.push({ filename: name, kind, phase, offsetMs: Date.now() - started });
};
async function openApplication(recording) {
  context = await browser.newContext({ viewport, reducedMotion: 'no-preference',
    ...(recording ? { recordVideo: { dir: resolve(output, 'video-raw'), size: viewport } } : {}) });
  page = await context.newPage();
  if (recording) {
    video = page.video();
    results.video = { filename, finalized: false, recordedSize: viewport, playbackRate: 1,
      startedOffsetMs: Date.now() - started, includesDiagnosticSeeks: false, naturalComplete: false };
    recordingTimer = setTimeout(() => {
      results.video.recordingLimitReached = true; results.ok = false; process.exitCode = 1;
      void context?.close().catch(() => {});
    }, results.limits.recordingMaxMs);
    recordingTimer.unref();
  }
  await page.addInitScript(installMotionSampling);
  page.setDefaultTimeout(12000);
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden && window.WaitingVisuals);
  await page.waitForFunction(() => Tasks.pending() === 0);
  await page.evaluate(async () => {
    await UiScale.set(1, false);
    await document.fonts.ready;
    if (document.documentElement.dataset.theme !== 'dark') toggleTheme();
    await Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {})));
  });
}
async function mountFixture(freezeAtReaction = false) {
  await page.evaluate(({ cards, freezeAtReaction }) => {
    if (window.__microPreview) {
      __microPreview.observers.forEach(observer => observer.disconnect());
      Object.values(__microPreview.views).forEach(view => view.destroy());
      document.querySelector('#waiting-micro-fixture').remove();
    }
    const panel = document.createElement('section'); panel.id = 'waiting-micro-fixture';
    panel.setAttribute('aria-label', 'Microreações: fixture com dados sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '66px auto auto 50%', transform: 'translateX(-50%)',
      width: '720px', maxWidth: 'calc(100vw - 24px)', boxSizing: 'border-box', padding: '18px', zIndex: '20000',
      border: '1px solid var(--border)', borderRadius: '12px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const heading = document.createElement('h2'); heading.textContent = 'Visor e pequeno aceno · fixture de teste';
    Object.assign(heading.style, { fontSize: '18px', margin: '0 0 6px' });
    const note = document.createElement('p');
    note.textContent = 'Componente real · recibos sintéticos · captura 1× · não mede a operação nativa';
    Object.assign(note.style, { fontSize: '12px', margin: '0 0 14px', color: 'var(--text-1)' });
    const grid = document.createElement('div');
    Object.assign(grid.style, { display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px' });
    panel.append(heading, note, grid); document.body.append(panel);
    const fixture = window.__microPreview = { views: {}, receipts: {}, seeds: {}, events: [], svg: {}, observers: [],
      animationIds: new WeakMap(), nextAnimationId: 0,
      application: { rowCount: state.rows.length, loaded: state.loaded }, freezeAtReaction };
    for (const card of cards) {
      const host = document.createElement('section'); host.dataset.card = card.key;
      Object.assign(host.style, { padding: '10px 6px', border: '1px solid var(--border)', borderRadius: '8px' });
      const caption = document.createElement('p');
      caption.textContent = `${card.episode === 'visor' ? 'Ajustar visor' : 'Pequeno aceno'} · ${card.family === 'reading' ? 'leitura' : 'composição'}`;
      Object.assign(caption.style, { fontSize: '12px', margin: '0 0 4px', textAlign: 'center' });
      host.append(caption); grid.append(host);
      const receipt = { operationId: `micro-${card.key}-fixture`, phaseId: card.phaseId, state: 'running',
        label: card.family === 'reading' ? 'Indexando registros' : 'Compondo relatório',
        completed: card.family === 'reading' ? 1200 : 7, total: card.family === 'reading' ? 6300 : 18,
        unit: card.family === 'reading' ? 'registros' : 'seções', elapsedMs: 60000 };
      const model = WaitingVisuals.derive(receipt);
      let reactionSeed;
      for (let seed = 0; seed < 4096; seed++) {
        if (WaitingVisuals.createDirector(receipt.operationId, seed).boundary(model)?.episode === card.episode) { reactionSeed = seed; break; }
      }
      if (reactionSeed === undefined) throw Error(`No first-boundary ${card.episode} seed for ${card.key}`);
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed });
      fixture.views[card.key] = view; fixture.receipts[card.key] = receipt; fixture.seeds[card.key] = reactionSeed;
      fixture.svg[card.key] = view.element.querySelector('svg');
      const record = () => {
        const root = view.element, art = root.querySelector('.wv-art');
        const station = root.querySelector(card.family === 'reading' ? '.wv-reader-station' : '.wv-compose-station');
        const matrix = station.getScreenCTM();
        const workActor = root.querySelector(card.family === 'reading' ? '.wv-work > .wv-reader' : '.wv-work > .wv-compose-actor');
        const parked = root.querySelector('.wv-proof-parked');
        return { key: card.key, episode: root.dataset.episode, adapter: root.dataset.adapter, timeMs: performance.now(),
          status: root.querySelector('.wv-status').textContent, metric: root.querySelector('.wv-metric').textContent,
          workVisible: getComputedStyle(workActor).visibility, reactionVisible: getComputedStyle(root.querySelector('.wv-reaction-actor')).visibility,
          stationMatrix: [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f],
          proofParkedOpacity: parked ? Number(getComputedStyle(parked).opacity) : null,
          tracks: art.getAnimations({ subtree: true }).filter(a => a.animationName === 'wv-episode-boundary'
            || a.animationName.startsWith(`wv-${card.episode}-`)).map(a => ({ name: a.animationName,
              durationMs: a.effect.getComputedTiming().duration, iterations: String(a.effect.getTiming().iterations), playbackRate: a.playbackRate })) };
      };
      fixture.events.push(record());
      let previous = `${view.element.dataset.episode}:${view.element.dataset.adapter}`, frozen = false;
      const observer = new MutationObserver(() => {
        const key = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
        if (key !== previous) { fixture.events.push(record()); previous = key; }
        // Diagnostics alone use the public control at each natural react boundary.
        // This does not alter or seek the separately finalized natural recording.
        if (freezeAtReaction && !frozen && view.element.dataset.adapter === 'react') {
          frozen = true; view.setMotionEnabled(false);
        }
      });
      observer.observe(view.element, { attributes: true, attributeFilter: ['data-episode', 'data-adapter'] });
      fixture.observers.push(observer);
    }
  }, { cards, freezeAtReaction });
}
async function finalizeNaturalVideo() {
  if (results.video?.finalized) return;
  if (!video) throw Error('Micro-reaction recording is unavailable');
  const captureWallMs = Date.now() - started - results.video.startedOffsetMs;
  mark('natural-recording-end'); clearTimeout(recordingTimer);
  await context.close(); context = null;
  await video.saveAs(resolve(output, filename));
  Object.assign(results.video, { finalized: true, bytes: statSync(resolve(output, filename)).size,
    captureWallMs, captureTiming: 'Wall-clock upper-bound target, not an encoded-duration probe', endedOffsetMs: Date.now() - started });
  await video.delete(); video = null;
  assert.ok(results.video.bytes > 0 && results.video.bytes <= results.limits.videoMaxBytes, 'keep the intact WebM below 6 MB');
  assert.ok(captureWallMs <= results.limits.recordingMaxMs && !results.video.recordingLimitReached, 'natural recording must close within 30s');
}
function sameReceipt(actual, baseline) {
  assert.deepEqual(actual.map(({ key, state, status, metric, svgCount }) => ({ key, state, status, metric, svgCount })),
    baseline.map(({ key, state, status, metric, svgCount }) => ({ key, state, status, metric, svgCount })));
}

try {
  browser = await launchBrowser();
  await openApplication(true);
  phase = 'natural micro-reactions and complete bridges';
  await mountFixture();
  results.seeds = await page.evaluate(() => __microPreview.seeds);
  results.initial = await states();
  assert.ok(results.initial.every(s => s.episode === 'work' && s.state === 'running' && s.svgCount === 1));
  mark('natural-work-start');
  // Reading and composition do not reach react together: observe histories, not
  // an impossible assertion that all four short reactions overlap in wall time.
  await page.waitForFunction(() => Object.entries(__microPreview.views).every(([key, view]) =>
    __microPreview.events.some(event => event.key === key && event.adapter === 'react')
      && view.element.dataset.episode === 'work' && view.element.dataset.adapter === 'work'),
  null, { timeout: results.limits.naturalBudgetMs });
  results.returned = await states(); sameReceipt(results.returned, results.initial);
  results.sequence = await page.evaluate(() => ({ events: __microPreview.events,
    stableSvg: Object.entries(__microPreview.views).every(([key, view]) => view.element.querySelector('svg') === __microPreview.svg[key]),
    inspections: Object.fromEntries(Object.entries(__microPreview.views).map(([key, view]) => [key, view.inspect()])),
    applicationBefore: __microPreview.application, applicationAfter: { rowCount: state.rows.length, loaded: state.loaded } }));
  assert.equal(results.sequence.stableSvg, true);
  assert.deepEqual(results.sequence.applicationAfter, results.sequence.applicationBefore);
  results.naturalDurations = {};
  for (const card of cards) {
    const events = results.sequence.events.filter(event => event.key === card.key);
    const expected = card.family === 'composition'
      ? [['work', 'work'], [card.episode, 'prepare'], [card.episode, 'react'], [card.episode, 'resume'], ['work', 'work']]
      : [['work', 'work'], [card.episode, 'react'], ['work', 'work']];
    assert.deepEqual(events.map(e => [e.episode, e.adapter]), expected);
    const react = events.findIndex(e => e.adapter === 'react'), durationMs = events[react + 1].timeMs - events[react].timeMs;
    assert.ok(durationMs >= card.durationMs - 100 && durationMs < card.durationMs + 1200, `${card.key}: full natural duration`);
    assert.equal(events[react].tracks.length, card.episode === 'visor' ? 7 : 10);
    assert.ok(events[react].tracks.every(a => a.durationMs === card.durationMs && a.iterations === '1' && a.playbackRate === 1));
    assert.equal(events[react].workVisible, 'hidden'); assert.equal(events[react].reactionVisible, 'visible');
    assert.ok(events.every(e => JSON.stringify(e.stationMatrix) === JSON.stringify(events[0].stationMatrix)), 'station stays on stage');
    assert.ok(events.every(e => e.status === events[0].status && e.metric === events[0].metric));
    results.naturalDurations[card.key] = { durationMs };
    if (card.family === 'composition') {
      assert.equal(events[react].proofParkedOpacity, 1, 'the work proof is parked before the free-hand gesture');
      const prepareMs = events[2].timeMs - events[1].timeMs, resumeMs = events[4].timeMs - events[3].timeMs;
      assert.ok(prepareMs >= 3100 && prepareMs < 4400 && resumeMs >= 3100 && resumeMs < 4400);
      Object.assign(results.naturalDurations[card.key], { prepareMs, resumeMs });
    }
    assert.deepEqual(results.sequence.inspections[card.key].history, [card.episode]);
  }
  await snap('waiting-micro-returned-natural-dark.png', 'four natural returns, complete bridges and unchanged receipts');
  results.video.naturalComplete = true;
  await finalizeNaturalVideo();

  phase = 'non-recorded pause and geometric diagnostics';
  assert.equal(results.video.finalized, true);
  await openApplication(false);
  await mountFixture(true);
  await page.waitForFunction(() => Object.values(__microPreview.views).every(view => view.element.dataset.adapter === 'react'
    && view.element.dataset.motion === 'static'), null, { timeout: 14000 });
  // Exercise the real buttons before any diagnostic seek, then keep all four
  // product-paused so sampling cannot outlive a 3.6s reaction.
  for (const button of await roots().locator('.wv-motion-toggle').all()) await button.click();
  await page.waitForTimeout(200);
  for (const button of await roots().locator('.wv-motion-toggle').all()) await button.click();
  await settle(); results.pauseSettled = await states();
  assert.ok(results.pauseSettled.every(s => s.motion === 'static' && s.animations.every(a => a.playState !== 'running' && !a.pending)));
  await page.waitForTimeout(220); results.pauseFrozen = await states();
  assert.deepEqual(results.pauseFrozen.map(s => s.animations.map(a => a.currentTime)), results.pauseSettled.map(s => s.animations.map(a => a.currentTime)));
  await page.evaluate(() => __microPreview.views['reading-visor'].update({ ...__microPreview.receipts['reading-visor'], completed: 1300 }));
  results.receiptDuringPause = await states();
  assert.equal(results.receiptDuringPause[0].metric, '1.300 / 6.300 registros');
  assert.equal(results.receiptDuringPause[1].metric, results.pauseFrozen[1].metric);
  results.rigs = await roots().evaluateAll(async nodes => Promise.all(nodes.map(async root => {
    const episode = root.dataset.episode, durationMs = episode === 'visor' ? 4800 : 3600;
    const art = root.querySelector('.wv-art'), actor = root.querySelector('.wv-reaction-actor');
    const hand = root.querySelector('.wv-react-hand'), head = root.querySelector('.wv-react-head');
    const palm = hand.querySelector('.wv-react-palm'), fingers = hand.querySelector('.wv-react-fingers');
    const feet = ['front', 'back'].map(side => root.querySelector(`.wv-react-foot-${side}`));
    const tracks = art.getAnimations({ subtree: true }).filter(a => a.animationName === 'wv-episode-boundary'
      || a.animationName.startsWith(`wv-${episode}-`));
    const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
    const local = (node, p) => p.matrixTransform(node.getScreenCTM().inverse());
    const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const opacity = selector => Number(getComputedStyle(root.querySelector(selector)).opacity);
    const result = { key: root.parentElement.dataset.card, episode, samples: [], contacts: [], wrists: [], endpoints: [],
      palmOwnedByHand: palm.parentElement === hand, fingersOwnedByHand: fingers.parentElement === hand };
    const sampling = await __waitingMotionSampling.begin(tracks);
    try {
      // Never seek the end boundary: 99.9% lies in the settled return hold and
      // cannot enqueue an artificial animationend in the real lifecycle.
      for (const percent of [0, 12, 20, 28, 30, 32, 36, 38, 40, 43, 48, 52, 55, 56, 62, 64, 66, 72, 84, 94, 99.9]) {
        await sampling.seek(percent / 100 * durationMs);
        const center = point(hand, 91, 55), inHead = local(head, center), inActor = local(actor, center);
        const vertices = [[90, 53], [93, 53], [93, 58], [90, 58], [88, 56]].map(([x, y]) => local(head, point(palm, x, y)));
        const sample = { percent, center: { x: inActor.x, y: inActor.y }, headLocal: { x: inHead.x, y: inHead.y },
          clearHead: vertices.every(p => p.x > 79.1 || p.y > 48.1),
          feet: feet.map((node, i) => { const p = point(node, i ? 61 : 68, 88); return { x: p.x, y: p.y }; }),
          parkedOpacity: root.dataset.family === 'composition' ? opacity('.wv-proof-parked') : null };
        result.samples.push(sample);
        if (percent === 0 || percent === 99.9) result.endpoints.push({ center: sample.center,
          head: getComputedStyle(head).transform, palm: getComputedStyle(palm).transform, gaze: getComputedStyle(root.querySelector('.wv-react-gaze')).transform });
        if (episode === 'visor' && [36, 43, 48, 55, 62].includes(percent)) {
          const x = percent === 43 || percent === 55 ? 69 : 71.5;
          result.contacts.push({ percent, distanceCssPx: distance(center, point(head, x, 36.5)) });
        }
        if (episode === 'wave' && [40, 48, 56, 64].includes(percent)) {
          const matrix = new DOMMatrix(getComputedStyle(palm).transform);
          result.wrists.push({ percent, angle: Math.atan2(matrix.b, matrix.a) * 180 / Math.PI,
            fingerMatrix: getComputedStyle(fingers).transform, palmMatrix: getComputedStyle(palm).transform });
        }
      }
      return result;
    } finally { await sampling.restore(); }
  })));
  for (const rig of results.rigs) {
    assert.equal(rig.palmOwnedByHand, true); assert.equal(rig.fingersOwnedByHand, true);
    assert.deepEqual(rig.endpoints[0], rig.endpoints[1], `${rig.key}: exact resting endpoint`);
    assert.ok(rig.samples.every(s => JSON.stringify(s.feet) === JSON.stringify(rig.samples[0].feet)), 'both feet stay planted');
    assert.ok(rig.samples.every(s => s.parkedOpacity === null || s.parkedOpacity === 1));
    if (rig.episode === 'visor') {
      assert.equal(rig.contacts.length, 5); assert.ok(rig.contacts.every(p => p.distanceCssPx < .15));
      assert.ok(rig.samples.filter(p => p.percent >= 36 && p.percent <= 62).every(p => p.headLocal.x >= 68.8 && p.headLocal.x <= 71.7
        && Math.abs(p.headLocal.y - 36.5) < .1), 'hand remains on the visor face during both shallow sweeps');
    } else {
      assert.ok(rig.samples.every(p => p.clearHead), 'wave palm never crosses the padded head silhouette');
      assert.ok(rig.samples.filter(p => p.percent >= 38 && p.percent <= 66).every(p => Math.hypot(p.center.x - 83, p.center.y - 37) < .001));
      for (const [i, expected] of [-18, 16, -14, 10].entries()) {
        assert.ok(Math.abs(rig.wrists[i].angle - expected) < .001); assert.equal(rig.wrists[i].fingerMatrix, rig.wrists[i].palmMatrix);
      }
    }
  }
  // Diagnostic stills have their own attribution and never enter the natural WebM.
  const poses = await page.evaluateHandle(async () => {
    const samplers = [];
    for (const view of Object.values(__microPreview.views)) {
      const root = view.element, episode = root.dataset.episode;
      const sampling = await __waitingMotionSampling.begin(root.querySelector('.wv-art').getAnimations({ subtree: true })
        .filter(a => a.animationName === 'wv-episode-boundary' || a.animationName.startsWith(`wv-${episode}-`)));
      samplers.push(sampling); await sampling.seek((episode === 'visor' ? 4800 : 3600) * .48);
    }
    return { restore: () => Promise.all(samplers.map(s => s.restore())) };
  });
  try {
    await snap('waiting-micro-contact-diagnostic-dark.png', 'sampled 48% contact/wrist, excluded from video');
    await page.evaluate(async () => {
      if (document.documentElement.dataset.theme !== 'light') toggleTheme();
      await Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {})));
    });
    await snap('waiting-micro-contact-diagnostic-light.png', 'same sampled poses in light theme, excluded from video');
  } finally { await poses.evaluate(sampling => sampling.restore()); await poses.dispose(); }
  phase = 'reduced-motion and immediate terminal safety';
  const beforeReduced = results.beforeReduced = await states();
  assert.ok(beforeReduced.every(s => s.motion === 'static' && s.adapter === 'react' && s.animations.length > 0
    && s.animations.every(a => a.playState !== 'running' && !a.pending)));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  // These four reactions were already product-paused. Re-enable the product
  // control under reduce so exiting reduce must resume the existing timelines.
  await page.evaluate(() => Object.values(__microPreview.views).forEach(view => view.setMotionEnabled(true)));
  await page.waitForFunction(() => Object.values(__microPreview.views).every(view => view.element.dataset.motion === 'static'));
  await settle(); results.reduced = await states(); sameReceipt(results.reduced, beforeReduced);
  assert.ok(results.reduced.every(s => s.motion === 'static' && s.animations.length > 0
    && s.animations.every(a => a.playState !== 'running' && !a.pending)));
  assert.deepEqual(results.reduced.map(identity), beforeReduced.map(identity), 'retain every CSSAnimation identity under reduce');
  assert.deepEqual(results.reduced.map(s => s.poses), beforeReduced.map(s => s.poses), 'already paused SVG poses survive preference change');
  assert.deepEqual(results.reduced.map(s => [s.episode, s.adapter]), beforeReduced.map(s => [s.episode, s.adapter]));
  assert.deepEqual(results.reduced.map(s => s.animations.map(a => a.currentTime)), beforeReduced.map(s => s.animations.map(a => a.currentTime)),
    'already product-paused clocks stay unchanged on entering reduce');
  await page.waitForTimeout(220); results.reducedFrozen = await states(); sameReceipt(results.reducedFrozen, beforeReduced);
  assert.deepEqual(results.reducedFrozen.map(identity), results.reduced.map(identity));
  assert.deepEqual(results.reducedFrozen.map(s => s.poses), results.reduced.map(s => s.poses), 'reduced SVG poses remain frozen');
  assert.deepEqual(results.reducedFrozen.map(s => s.animations.map(a => a.currentTime)), results.reduced.map(s => s.animations.map(a => a.currentTime)),
    'reduced-motion clocks remain exactly frozen during the wait');
  assert.ok(results.reducedFrozen.every(s => s.motion === 'static' && s.animations.every(a => a.playState !== 'running' && !a.pending)));
  assert.equal(await roots().locator('.wv-motion-toggle:visible').count(), 0);
  await snap('waiting-micro-reduced-motion-light.png', 'reduced-motion static safety with retained identities and exactly frozen clocks');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.waitForFunction(() => Object.values(__microPreview.views).every(view => view.element.dataset.motion === 'running'));
  await settle(); await page.waitForTimeout(100); results.reducedResumed = await states(); sameReceipt(results.reducedResumed, beforeReduced);
  assert.deepEqual(results.reducedResumed.map(identity), beforeReduced.map(identity), 'no replacement timelines on exiting reduce');
  for (const [i, sample] of results.reducedResumed.entries()) {
    const original = beforeReduced[i], frozen = results.reducedFrozen[i];
    assert.equal(sample.motion, 'running'); assert.equal(sample.episode, cards[i].episode); assert.equal(sample.episode, original.episode);
    assert.equal(sample.adapter, 'react'); assert.equal(sample.adapter, original.adapter);
    assert.ok(sample.animations.every((a, j) => a.currentTime >= frozen.animations[j].currentTime), 'no animation clock restarts');
    const reactionTracks = sample.animations.filter(a => a.name === 'wv-episode-boundary' || a.name.startsWith(`wv-${sample.episode}-`));
    assert.equal(reactionTracks.length, sample.episode === 'visor' ? 7 : 10);
    assert.ok(reactionTracks.every(a => a.playState === 'running' && !a.pending
      && a.currentTime > frozen.animations.find(previous => previous.id === a.id).currentTime), 'the same microepisode naturally advances again');
  }
  // Continuity is proved above; terminal checks use fresh naturally reached episodes.
  await mountFixture(true);
  await page.waitForFunction(() => Object.values(__microPreview.views).every(view => view.element.dataset.adapter === 'react'
    && view.element.dataset.motion === 'static'), null, { timeout: 14000 });
  results.terminal = await page.evaluate(() => {
    __microPreview.observers.forEach(observer => observer.disconnect());
    return Object.entries(__microPreview.views).map(([key, view], index) => {
      const targetState = ['cancelled', 'error', 'completed', 'cancelling'][index];
      view.setMotionEnabled(true);
      const before = { episode: view.element.dataset.episode, adapter: view.element.dataset.adapter, motion: view.element.dataset.motion };
      view.update({ ...__microPreview.receipts[key], state: targetState });
      return { key, before, targetState, state: view.element.dataset.state, family: view.element.dataset.family,
        episode: view.element.dataset.episode, motion: view.element.dataset.motion,
        actorPresent: !!view.element.querySelector('.wv-reaction-actor'),
        runningTracks: view.element.querySelector('.wv-art').getAnimations({ subtree: true }).filter(a => a.playState === 'running').length };
    });
  });
  for (const sample of results.terminal) {
    assert.equal(sample.before.episode, cards.find(card => card.key === sample.key).episode);
    assert.equal(sample.before.adapter, 'react'); assert.equal(sample.before.motion, 'running');
    assert.equal(sample.state, sample.targetState); assert.equal(sample.family, 'neutral');
    assert.equal(sample.motion, 'static'); assert.equal(sample.episode, 'work'); assert.equal(sample.actorPresent, false); assert.equal(sample.runningTracks, 0);
  }
  results.applicationUnchanged = await page.evaluate(() => JSON.stringify(__microPreview.application)
    === JSON.stringify({ rowCount: state.rows.length, loaded: state.loaded }));
  assert.equal(results.applicationUnchanged, true);
  await page.evaluate(() => {
    __microPreview.observers.forEach(observer => observer.disconnect());
    Object.values(__microPreview.views).forEach(view => view.destroy());
    document.querySelector('#waiting-micro-fixture').remove();
  });
  assert.equal(await roots().count(), 0); assert.deepEqual(errors, []); results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error); process.exitCode = 1;
  if (page) await captureFailure(page, 'waiting-micro-reactions', error, { phase, errors, results });
} finally {
  try { if (video) await finalizeNaturalVideo(); }
  catch (error) { results.videoError = String(error); results.ok = false; if (!failure) process.exitCode = 1; }
  clearTimeout(recordingTimer); clearTimeout(deadline);
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  if (results.elapsedMs >= results.limits.totalTimeoutMs) { results.ok = false; process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-micro-reactions-results.json'), JSON.stringify(results, null, 2));
  writeFileSync(resolve(output, 'waiting-micro-reactions-video.json'), JSON.stringify({ evidence: results.evidence,
    limits: results.limits, sourceCommit: results.sourceCommit, video: results.video, seeds: results.seeds,
    markers: results.markers, completed: results.ok === true }, null, 2));
}
