/* CI-only: node scripts/preview/run-smoke.mjs test-waiting-manual.mjs
   Real WaitingVisuals in the application; labelled synthetic receipts and panel.
   Complete 1x episodes and bridges are finalized before separate pose diagnostics. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
import { installMotionSampling } from './motion-sampling.mjs';

const families = ['reading', 'composition'];
const phases = ['metadata-scan', 'command:case_report_render'];
const output = resolve('output/playwright'), started = Date.now();
const viewport = { width: 1440, height: 960 };
mkdirSync(output, { recursive: true });
const results = {
  evidence: {
    component: 'Production WaitingVisuals, SVG, CSS, adapters and shared director; no replacement artwork',
    context: 'Actual Chromium preview application; fixture panel and receipts are explicitly synthetic',
    timing: 'Complete 32s manual at 1x plus natural work/prepare/resume; no seeks during recording',
    eligibility: 'Supplied elapsedMs=60000 is synthetic input, never native latency or progress',
    poseSampling: 'Only after WebM finalization, in a separate context without recordVideo',
    scenery: 'Temporary open manual shelf; coffee kitchen, hatch, cup and steam remain invisible',
    generatedImageAssets: false, nativeEngineVerified: false, installedWebViewVerified: false,
  },
  limits: { totalTimeoutMs: 110000, targetRuntimeMs: [70000, 90000], seedSearchLimit: 4096,
    naturalBudgetMs: 46000, diagnosticBoundaryBudgetMs: 10800, contactToleranceCssPx: .15,
    durationMs: 32000, bridgeMs: 3200 },
  sourceCommit: process.env.GITHUB_SHA || null, families, phases, markers: [], screenshots: [],
};
let browser, context, page, video, failure = null, phase = 'startup';
const errors = [];
const mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
const deadline = setTimeout(() => {
  results.timedOut = true; results.ok = false; process.exitCode = 1;
  void browser?.close().catch(() => {});
}, results.limits.totalTimeoutMs);
deadline.unref();
const roots = () => page.locator('#waiting-manual-fixture .waiting-visual');
const scene = family => page.locator(`#manual-${family} .waiting-visual`);
const states = () => roots().evaluateAll(nodes => nodes.map(root => {
  const art = root.querySelector('.wv-art');
  const opacity = selector => Number(getComputedStyle(root.querySelector(selector)).opacity);
  const effectiveOpacity = node => {
    let value = 1;
    for (; node && node !== art; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.visibility === 'hidden' || style.display === 'none') return 0;
      value *= Number(style.opacity);
    }
    return value;
  };
  return { family: root.dataset.family, episode: root.dataset.episode, adapter: root.dataset.adapter,
    motion: root.dataset.motion, state: root.dataset.state, status: root.querySelector('.wv-status').textContent,
    metric: root.querySelector('.wv-metric').textContent, svgCount: art.querySelectorAll('svg').length,
    size: art.getBoundingClientRect().toJSON(), kitchenOpacity: opacity('.wv-kitchen'),
    manualKitOpacity: opacity('.wv-manual-kit'), hatchOpacity: effectiveOpacity(root.querySelector('.wv-kitchen-hatch')),
    cupOpacity: Math.max(...['.wv-cup-held', '.wv-cup-shelf'].map(selector => effectiveOpacity(root.querySelector(selector)))),
    steamOpacity: Math.max(...[...root.querySelectorAll('.wv-cup-steam')].map(effectiveOpacity)),
    heldOpacity: opacity('.wv-manual-held'), supportedOpacity: opacity('.wv-manual-supported'),
    shelfOpacity: opacity('.wv-manual-shelf'),
    animations: art.getAnimations({ subtree: true }).map(animation => ({ name: animation.animationName,
      currentTime: animation.currentTime, durationMs: animation.effect.getComputedTiming().duration,
      iterations: String(animation.effect.getTiming().iterations), playbackRate: animation.playbackRate,
      playState: animation.playState, pending: animation.pending })),
  };
}));
const snap = async (filename, kind) => {
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  results.screenshots.push({ filename, kind, phase, offsetMs: Date.now() - started });
};
const waitEpisode = (episode, timeout = 45000) => page.waitForFunction(episode => {
  const nodes = [...document.querySelectorAll('#waiting-manual-fixture .waiting-visual')];
  return nodes.length === 2 && nodes.every(root => root.dataset.episode === episode
    && root.dataset.adapter === (episode === 'manual' ? 'react' : 'work'));
}, episode, { timeout });
const settlePausedArtwork = () => roots().evaluateAll(async nodes => {
  const animations = nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true }));
  await Promise.all(animations.map(animation => animation.ready));
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
});
async function openApplication(recording) {
  context = await browser.newContext({ viewport, reducedMotion: 'no-preference',
    ...(recording ? { recordVideo: { dir: resolve(output, 'video-raw'), size: viewport } } : {}) });
  page = await context.newPage();
  if (recording) {
    video = page.video();
    results.video = { filename: 'waiting-manual-real-preview.webm', finalized: false, recordedSize: viewport,
      startedOffsetMs: Date.now() - started, playbackRate: 1, includesDiagnosticSeeks: false };
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
    if (document.documentElement.dataset.theme !== 'dark') toggleTheme();
    await Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {})));
  });
}
async function mountFixture() {
  await page.evaluate(({ families, phases }) => {
    const panel = document.createElement('section'); panel.id = 'waiting-manual-fixture';
    panel.setAttribute('aria-label', 'Consulta ao manual: fixture com dados sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '180px auto auto 50%', transform: 'translateX(-50%)',
      width: '740px', maxWidth: 'calc(100vw - 32px)', boxSizing: 'border-box', padding: '24px', zIndex: '20000',
      border: '1px solid var(--border)', borderRadius: '12px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const heading = document.createElement('h2'); heading.textContent = 'Uma consulta ao manual · fixture de teste';
    Object.assign(heading.style, { fontSize: '18px', margin: '0 0 8px' });
    const attribution = document.createElement('p');
    attribution.textContent = 'Componente real · recibos sintéticos · captura 1× · não mede a operação nativa';
    Object.assign(attribution.style, { fontSize: '12px', color: 'var(--text-1)', margin: '0 0 20px' });
    const grid = document.createElement('div');
    Object.assign(grid.style, { display: 'flex', gap: '16px', justifyContent: 'center' });
    panel.append(heading, attribution, grid); document.body.append(panel);
    const fixture = window.__manualPreview = { views: {}, receipts: {}, seeds: {}, events: [], svg: {}, observers: [],
      application: { rowCount: state.rows.length, loaded: state.loaded } };
    for (const [index, family] of families.entries()) {
      const host = document.createElement('section'); host.id = `manual-${family}`;
      Object.assign(host.style, { width: '310px', padding: '12px 8px', border: '1px solid var(--border)', borderRadius: '8px' });
      grid.append(host);
      const receipt = { operationId: `manual-${family}-fixture`, phaseId: phases[index], state: 'running',
        label: index === 0 ? 'Indexando registros' : 'Compondo relatório',
        completed: index === 0 ? 1200 : 7, total: index === 0 ? 6300 : 18,
        unit: index === 0 ? 'registros' : 'seções', elapsedMs: 60000 };
      const model = WaitingVisuals.derive(receipt);
      let reactionSeed;
      for (let seed = 0; seed < 4096; seed++) {
        if (WaitingVisuals.createDirector(receipt.operationId, seed).boundary(model)?.episode === 'manual') { reactionSeed = seed; break; }
      }
      if (reactionSeed === undefined) throw Error(`No first-boundary manual seed found for ${family}`);
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed });
      fixture.views[family] = view; fixture.receipts[family] = receipt; fixture.seeds[family] = reactionSeed;
      fixture.svg[family] = view.element.querySelector('svg');
      const record = () => ({ family, episode: view.element.dataset.episode, adapter: view.element.dataset.adapter, timeMs: performance.now() });
      fixture.events.push(record());
      let previous = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
      const observer = new MutationObserver(() => {
        const key = `${view.element.dataset.episode}:${view.element.dataset.adapter}`;
        if (key !== previous) { fixture.events.push(record()); previous = key; }
      });
      observer.observe(view.element, { attributes: true, attributeFilter: ['data-episode', 'data-adapter'] });
      fixture.observers.push(observer);
    }
  }, { families, phases });
  await page.waitForFunction(() => Object.values(__manualPreview.views).every(view => view.element.dataset.motion === 'running'));
}
async function finalizeNaturalVideo() {
  if (results.video?.finalized) return;
  if (!video) throw Error('Manual recording is unavailable');
  mark('natural-recording-end');
  await context.close(); context = null;
  const filename = 'waiting-manual-real-preview.webm';
  await video.saveAs(resolve(output, filename));
  Object.assign(results.video, { bytes: statSync(resolve(output, filename)).size, finalized: true, endedOffsetMs: Date.now() - started });
  assert.ok(results.video.bytes > 0);
  await video.delete(); video = null;
}
function assertReceiptStates(actual, baseline) {
  assert.deepEqual(actual.map(s => s.family), families);
  for (const [i, state] of actual.entries()) {
    assert.equal(state.state, 'running'); assert.equal(state.svgCount, 1);
    assert.equal(state.kitchenOpacity, 0); assert.equal(state.hatchOpacity, 0);
    assert.equal(state.cupOpacity, 0); assert.equal(state.steamOpacity, 0);
    assert.equal(state.status, baseline[i].status); assert.equal(state.metric, baseline[i].metric);
    assert.ok(Math.abs(state.size.width - 240) < .5 && Math.abs(state.size.height - 120) < .5);
  }
}
async function pauseManual(label, updateReceipt = false) {
  const before = await states();
  await scene('reading').locator('.wv-motion-toggle').click();
  const requested = (await states())[0];
  assert.equal(requested.motion, 'static'); assert.equal(requested.episode, 'manual');
  assert.ok(requested.animations.every(a => a.playState !== 'running'));
  await settlePausedArtwork();
  const settled = (await states())[0];
  assert.ok(settled.animations.every(a => !a.pending));
  await page.waitForTimeout(250);
  const frozen = (await states())[0];
  assert.deepEqual(frozen.animations.map(a => a.currentTime), settled.animations.map(a => a.currentTime),
    `${label}: product CSS freezes the complete episode without restarting its clocks`);
  assert.equal(frozen.episode, 'manual');
  if (updateReceipt) {
    await page.evaluate(() => __manualPreview.views.reading.update({ ...__manualPreview.receipts.reading, completed: 1300, elapsedMs: 61000 }));
    const updated = await states();
    assert.equal(updated[0].motion, 'static'); assert.equal(updated[0].metric, '1.300 / 6.300 registros');
    assert.equal(updated[0].status, before[0].status);
    assert.equal(updated[1].metric, before[1].metric, 'other instance keeps its independent receipt');
  }
  await scene('reading').locator('.wv-motion-toggle').click();
  await page.waitForTimeout(150);
  const resumed = (await states())[0];
  assert.equal(resumed.motion, 'running'); assert.equal(resumed.episode, 'manual');
  assert.ok(resumed.animations.some((a, i) => a.currentTime > settled.animations[i].currentTime));
  mark(label);
  return { before, requested, settled, frozen, resumed };
}

try {
  browser = await launchBrowser();
  await openApplication(true);
  phase = 'natural manual in reading and composition';
  await mountFixture();
  results.seeds = await page.evaluate(() => __manualPreview.seeds);
  results.initial = await states();
  assertReceiptStates(results.initial, results.initial);
  assert.ok(results.initial.every(s => s.episode === 'work' && s.adapter === 'work' && s.kitchenOpacity === 0 && s.manualKitOpacity === 0));
  assert.deepEqual(results.initial.map(s => s.metric), ['1.200 / 6.300 registros', '7 / 18 seções']);
  mark('natural-work-start');
  await waitEpisode('manual', 15000);
  mark('both-natural-manual-started');
  results.started = await states();
  assertReceiptStates(results.started, results.initial);
  for (const state of results.started) {
    const tracks = state.animations.filter(a => /^(?:wv-manual-|wv-coffee-|wv-episode-boundary)/.test(a.name));
    assert.ok(tracks.length > 15);
    assert.ok(tracks.every(a => a.durationMs === 32000 && a.iterations === '1' && a.playbackRate === 1));
  }
  // Observe both natural clocks; nothing rewinds, pauses or replaces the artwork.
  await page.waitForFunction(() => Object.values(__manualPreview.views).every(view =>
    view.element.querySelector('.wv-episode-boundary').getAnimations()[0]?.currentTime >= 16300), null, { timeout: 18000 });
  results.naturalSupported = await states();
  assertReceiptStates(results.naturalSupported, results.initial);
  assert.ok(results.naturalSupported.every(s => s.supportedOpacity === 1 && s.heldOpacity === 0 && s.manualKitOpacity === 1 && s.shelfOpacity === 0));
  await snap('waiting-manual-natural-two-hands-dark.png', 'natural consultation, both production families');
  await waitEpisode('work', 22000);
  mark('both-natural-returned-to-work');
  results.returned = await states();
  assertReceiptStates(results.returned, results.initial);
  assert.ok(results.returned.every(s => s.kitchenOpacity === 0 && s.manualKitOpacity === 0));
  results.sequence = await page.evaluate(() => ({ events: __manualPreview.events,
    stableSvg: Object.entries(__manualPreview.views).every(([family, view]) => view.element.querySelector('svg') === __manualPreview.svg[family]),
    inspections: Object.fromEntries(Object.entries(__manualPreview.views).map(([family, view]) => [family, view.inspect()])),
    applicationBefore: __manualPreview.application, applicationAfter: { rowCount: state.rows.length, loaded: state.loaded } }));
  assert.equal(results.sequence.stableSvg, true);
  assert.deepEqual(results.sequence.applicationAfter, results.sequence.applicationBefore);
  results.naturalDurations = {};
  for (const family of families) {
    const events = results.sequence.events.filter(event => event.family === family);
    const expected = family === 'composition'
      ? [['work', 'work'], ['manual', 'prepare'], ['manual', 'react'], ['manual', 'resume'], ['work', 'work']]
      : [['work', 'work'], ['manual', 'react'], ['work', 'work']];
    assert.deepEqual(events.map(({ episode, adapter }) => [episode, adapter]), expected);
    const reactIndex = events.findIndex(event => event.adapter === 'react');
    const durationMs = events[reactIndex + 1].timeMs - events[reactIndex].timeMs;
    assert.ok(durationMs >= 31900 && durationMs < 35000, `${family}: complete natural 32s episode`);
    results.naturalDurations[family] = { durationMs };
    if (family === 'composition') {
      const prepareMs = events[2].timeMs - events[1].timeMs, resumeMs = events[4].timeMs - events[3].timeMs;
      assert.ok(prepareMs >= 3100 && prepareMs < 4500 && resumeMs >= 3100 && resumeMs < 4500);
      Object.assign(results.naturalDurations[family], { prepareMs, resumeMs });
    }
    assert.deepEqual(results.sequence.inspections[family].history, ['manual']);
    assert.ok(results.sequence.inspections[family].cooldown >= 4 && results.sequence.inspections[family].cooldown <= 5,
      'five-cycle cooldown may consume the first resumed work cycle while the other family finishes its bridge');
  }
  await snap('waiting-manual-returned-dark.png', 'complete natural return, independent receipts unchanged');
  await finalizeNaturalVideo();

  phase = 'non-recorded diagnostics after natural video';
  assert.equal(results.video.finalized, true);
  await openApplication(false);
  await mountFixture();
  await waitEpisode('manual', 15000);
  results.preSamplingPause = await pauseManual('manual-product-pause-before-any-seek');
  // All pose diagnostics run concurrently, then restore exact clock positions.
  results.rigs = await roots().evaluateAll(async nodes => Promise.all(nodes.map(async root => {
    const art = root.querySelector('.wv-art'), front = root.querySelector('.wv-react-hand');
    const back = root.querySelector('.wv-react-arm-back');
    const held = root.querySelector('.wv-manual-held'), supported = root.querySelector('.wv-manual-supported');
    const frontBook = held.querySelector('.wv-manual-wrist > g');
    const backBook = supported.querySelector('.wv-manual-support-wrist > g');
    const shelf = root.querySelector('.wv-manual-shelf'), cover = root.querySelector('.wv-manual-cover-fold');
    const leaf = root.querySelector('.wv-manual-page'), kit = root.querySelector('.wv-manual-kit');
    const kitchen = root.querySelector('.wv-kitchen'), hatch = root.querySelector('.wv-kitchen-hatch');
    const tracks = art.getAnimations({ subtree: true }).filter(a => /^(?:wv-manual-|wv-coffee-|wv-episode-boundary)/.test(a.animationName));
    const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
    const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const opacity = node => Number(getComputedStyle(node).opacity);
    const effectiveOpacity = node => {
      let value = 1;
      for (; node && node !== art; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (style.visibility === 'hidden' || style.display === 'none') return 0;
        value *= Number(style.opacity);
      }
      return value;
    };
    const result = { family: root.dataset.family, heldByFront: held.parentElement === front,
      supportedByBack: supported.parentElement === back, leafOwnedByBook: leaf.parentElement === backBook,
      sharedTravel: tracks.some(a => a.animationName === `wv-coffee-travel-${root.dataset.family === 'reading' ? 'reading' : 'checkpoint'}`),
      shelfParentIsKit: shelf.parentElement === kit, shelfOutsideKitchen: !kitchen.contains(shelf),
      propStates: [], reachContacts: [], reachPaths: [], shelfContacts: [], handoffs: [], palms: [], edges: [], owners: [], reading: [], pageTurns: [] };
    const sampling = await window.__waitingMotionSampling.begin(tracks);
    try {
      const at = seconds => sampling.seek(seconds * 1000);
      for (const seconds of [0, 1.6, 2, 6.5, 8.75, 9, 26.5, 28.5, 31.04, 31.9]) {
        await at(seconds);
        result.propStates.push({ seconds, kit: opacity(kit), kitchen: opacity(kitchen), hatch: effectiveOpacity(hatch),
          cup: Math.max(...['.wv-cup-held', '.wv-cup-shelf'].map(selector => effectiveOpacity(root.querySelector(selector)))),
          steam: Math.max(...[...root.querySelectorAll('.wv-cup-steam')].map(effectiveOpacity)),
          shelf: opacity(shelf), actorX: new DOMMatrix(getComputedStyle(root.querySelector('.wv-reaction-actor')).transform).e });
      }
      for (const seconds of [8.75, 9, 26.5, 26.9]) {
        await at(seconds);
        result.reachContacts.push({ seconds, distance: distance(point(front, 91, 55), point(shelf, 64, 58.7)) });
      }
      for (const [start, end] of [[6.5, 8.75], [26.9, 28.5]]) {
        await at(start); const first = point(front, 91, 55);
        await at(end); const last = point(front, 91, 55);
        for (let step = 0; step <= 16; step++) {
          const fraction = step / 16, seconds = start + (end - start) * fraction;
          await at(seconds);
          result.reachPaths.push({ seconds, distance: distance(point(front, 91, 55),
            { x: first.x + (last.x - first.x) * fraction, y: first.y + (last.y - first.y) * fraction }),
            verticalSpan: Math.abs(last.y - first.y), frontOwner: opacity(held), backOwner: opacity(supported) });
        }
      }
      for (const seconds of [9, 26.5]) {
        await at(seconds);
        result.shelfContacts.push({ seconds, distances: [[64, 48.5], [74.5, 63], [64, 58.7]].map(([x, y]) => distance(point(frontBook, x, y), point(shelf, x, y))) });
      }
      for (const seconds of [15, 22]) {
        await at(seconds);
        result.handoffs.push({ seconds, distances: [[64, 48.5], [74.5, 63], [85, 48.5]].map(([x, y]) => distance(point(frontBook, x, y), point(backBook, x, y))) });
      }
      for (const seconds of [16.2, 16.6, 20.2, 20.6]) {
        await at(seconds);
        result.palms.push({ seconds, back: distance(point(back, 54, 65), point(backBook, 64, 62.66190379)),
          front: distance(point(front, 91, 55), point(backBook, 85, 62.66190379)),
          headAngle: Math.atan2(new DOMMatrix(getComputedStyle(root.querySelector('.wv-react-head')).transform).b,
            new DOMMatrix(getComputedStyle(root.querySelector('.wv-react-head')).transform).a) * 180 / Math.PI });
      }
      for (const [name, node, start, end] of [['cover-open', cover, 15, 16], ['page-turn', leaf, 18.2, 19.6], ['cover-close', cover, 21, 22]]) {
        for (let step = 0; step <= 16; step++) {
          const seconds = start + (end - start) * step / 16;
          await at(seconds);
          result.edges.push({ name, seconds, front: distance(point(front, 91, 55), point(node, 85, 58.7)),
            back: distance(point(back, 54, 65), point(backBook, 64, 62.66190379)) });
        }
      }
      for (const [seconds, x, y] of [[17, 77, 52], [17.4, 83, 52], [17.6, 77, 55], [18, 83, 55]]) {
        await at(seconds);
        result.reading.push({ seconds, distance: distance(point(front, 91, 55), point(backBook, x, y)) });
      }
      for (const seconds of [0, 8.999, 9, 14.999, 15, 20, 21.999, 22, 26.499, 26.5, 31.9]) {
        await at(seconds);
        result.owners.push({ seconds, shelf: opacity(shelf), front: opacity(held), back: opacity(supported) });
      }
      for (const seconds of [15, 16, 18.2, 18.55, 18.9, 19.25, 19.6, 20.6, 21, 22, 22.02]) {
        await at(seconds);
        result.pageTurns.push({ seconds, scaleX: new DOMMatrix(getComputedStyle(leaf).transform).a,
          opacity: opacity(leaf), supportedOpacity: opacity(supported), coverScaleX: new DOMMatrix(getComputedStyle(cover).transform).a });
      }
      return result;
    } finally { await sampling.restore(); }
  })));
  for (const rig of results.rigs) {
    assert.equal(rig.heldByFront, true); assert.equal(rig.supportedByBack, true); assert.equal(rig.leafOwnedByBook, true);
    assert.equal(rig.sharedTravel, true); assert.equal(rig.shelfParentIsKit, true); assert.equal(rig.shelfOutsideKitchen, true);
    assert.ok(rig.propStates.every(p => p.kitchen === 0 && p.hatch === 0 && p.cup === 0 && p.steam === 0), 'coffee props never enter the manual episode');
    assert.equal(rig.propStates.find(p => p.seconds === 0).kit, 0);
    assert.ok(rig.propStates.filter(p => p.seconds >= 1.6 && p.seconds <= 31.04).every(p => p.kit === 1), 'open shelf exists throughout approach, pickup and return');
    assert.ok(rig.propStates.find(p => p.seconds === 31.9).kit < 1, 'shelf fades only after returning home');
    for (const seconds of [1.6, 31.04]) assert.equal(rig.propStates.find(p => p.seconds === seconds).actorX, rig.family === 'reading' ? 0 : 20);
    for (const seconds of [1.6, 8.75, 26.5, 31.04]) assert.equal(rig.propStates.find(p => p.seconds === seconds).shelf, 1, 'book is visible at rest on its own shelf');
    assert.ok(rig.reachContacts.every(p => p.distance < .15), 'the empty hand meets the visible book directly');
    assert.ok(rig.reachPaths.every(p => p.distance < .15 && p.verticalSpan < 1 && p.frontOwner === 0 && p.backOwner === 0), 'reach and retract stay at shelf height without a phantom hatch gesture');
    for (const contact of [...rig.shelfContacts, ...rig.handoffs]) assert.ok(contact.distances.every(d => d < .15), `${rig.family}: complete book matrices coincide`);
    assert.ok([...rig.palms, ...rig.edges].every(p => p.front < .15 && p.back < .15), `${rig.family}: both hands keep measured contact`);
    assert.ok(rig.reading.every(p => p.distance < .15));
    assert.ok(rig.owners.every(p => Math.abs(p.shelf + p.front + p.back - 1) < .00001));
    const turn = rig.pageTurns.filter(p => p.seconds >= 18.2 && p.seconds <= 19.6);
    assert.ok(Math.abs(turn[0].scaleX - 1) < .00001 && Math.abs(turn.at(-1).scaleX + 1) < .00001);
    assert.ok(turn.every((p, i) => i === 0 || p.scaleX < turn[i - 1].scaleX), 'one monotonic page turn');
    const closed = rig.pageTurns.find(p => p.seconds === 22), reset = rig.pageTurns.at(-1);
    assert.ok(Math.abs(closed.coverScaleX + 1) < .00001 && closed.supportedOpacity === 0);
    assert.equal(reset.opacity, 0); assert.equal(reset.supportedOpacity, 0, 'page resets only after the support copy disappears');
    assert.ok(rig.palms.filter(p => p.seconds >= 20.2).every(p => Math.abs(p.headAngle + 5) < .01), 'looks toward the task before closing');
  }
  // A held pose is diagnostic only. Restore while CSS remains the sole owner of playback.
  const poses = await page.evaluateHandle(async () => {
    const nodes = [...document.querySelectorAll('#waiting-manual-fixture .waiting-visual')];
    const sampling = await __waitingMotionSampling.begin(nodes.flatMap(root => root.querySelector('.wv-art').getAnimations({ subtree: true })
      .filter(a => /^(?:wv-manual-|wv-coffee-|wv-episode-boundary)/.test(a.animationName))));
    await sampling.seek(16500);
    return sampling;
  });
  try {
    await snap('waiting-manual-supported-diagnostic-dark.png', 'measured pose at 16.5s, excluded from video');
    await page.evaluate(() => { if (document.documentElement.dataset.theme !== 'light') toggleTheme(); });
    await page.waitForTimeout(250);
    await snap('waiting-manual-supported-diagnostic-light.png', 'measured light-theme pose, excluded from video');
  } finally { await poses.evaluate(sampling => sampling.restore()); await poses.dispose(); }
  results.postSamplingPause = await pauseManual('manual-product-pause-after-contact-sampling', true);
  phase = 'hidden and reduced motion after sampling';
  await page.evaluate(() => Object.values(__manualPreview.views).forEach(view => view.setVisible(false)));
  results.hiddenRequested = await states();
  assert.ok(results.hiddenRequested.every(s => s.motion === 'static' && s.animations.every(a => a.playState !== 'running')));
  await settlePausedArtwork();
  const hidden = results.hiddenSettled = await states();
  await page.waitForTimeout(250);
  results.hiddenFrozen = await states();
  assert.deepEqual(results.hiddenFrozen.map(s => s.animations.map(a => a.currentTime)), hidden.map(s => s.animations.map(a => a.currentTime)));
  await page.evaluate(() => Object.values(__manualPreview.views).forEach(view => view.setVisible(true)));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.waitForFunction(() => Object.values(__manualPreview.views).every(view => view.element.dataset.motion === 'static'));
  results.reduced = await states();
  assert.ok(results.reduced.every(s => s.animations.every(a => a.playState !== 'running')));
  assert.equal(await roots().locator('.wv-motion-toggle:visible').count(), 0);
  await snap('waiting-manual-reduced-motion-light.png', 'real reduced-motion policy after diagnostic restoration');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.waitForFunction(() => Object.values(__manualPreview.views).every(view => view.element.dataset.motion === 'running'));
  phase = 'synchronous terminal interruption';
  results.terminal = await page.evaluate(() => Object.entries(__manualPreview.views).map(([family, view]) => {
    const read = () => ({ family: view.element.dataset.family, state: view.element.dataset.state, episode: view.element.dataset.episode,
      motion: view.element.dataset.motion, status: view.element.querySelector('.wv-status').textContent,
      manualPresent: !!view.element.querySelector('.wv-manual-held'), manualKitPresent: !!view.element.querySelector('.wv-manual-kit'), kitchenPresent: !!view.element.querySelector('.wv-kitchen'),
      moving: view.element.querySelector('.wv-art').getAnimations({ subtree: true }).filter(a => a.playState === 'running').length });
    const before = read();
    view.update({ ...__manualPreview.receipts[family], state: family === 'reading' ? 'error' : 'completed' });
    return { family, before, after: read(), synchronous: true };
  }));
  for (const sample of results.terminal) {
    assert.equal(sample.before.episode, 'manual'); assert.equal(sample.before.motion, 'running');
    assert.equal(sample.after.motion, 'static'); assert.equal(sample.after.episode, 'work');
    assert.equal(sample.after.manualPresent, false); assert.equal(sample.after.manualKitPresent, false); assert.equal(sample.after.kitchenPresent, false); assert.equal(sample.after.moving, 0);
    assert.equal(sample.after.state, sample.family === 'reading' ? 'error' : 'completed');
  }
  results.applicationUnchanged = await page.evaluate(() => JSON.stringify(__manualPreview.application)
    === JSON.stringify({ rowCount: state.rows.length, loaded: state.loaded }));
  assert.equal(results.applicationUnchanged, true);
  await page.evaluate(() => {
    __manualPreview.observers.forEach(observer => observer.disconnect());
    Object.values(__manualPreview.views).forEach(view => view.destroy());
    document.querySelector('#waiting-manual-fixture').remove();
  });
  assert.equal(await roots().count(), 0); assert.deepEqual(errors, []);
  results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error);
  if (page) {
    try { results.failureStates = await states(); } catch {}
    await captureFailure(page, 'waiting-manual', error, { phase, errors, results });
  }
  process.exitCode = 1;
} finally {
  try { if (video) await finalizeNaturalVideo(); }
  catch (error) { results.videoError = String(error); results.ok = false; if (!failure) process.exitCode = 1; }
  try { await context?.close(); await browser?.close(); }
  catch (error) { results.cleanupError = String(error); results.ok = false; process.exitCode = 1; }
  clearTimeout(deadline);
  results.phase = phase; results.errors = errors; results.elapsedMs = Date.now() - started;
  if (results.elapsedMs >= results.limits.totalTimeoutMs) { results.ok = false; process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-manual-results.json'), JSON.stringify(results, null, 2));
  writeFileSync(resolve(output, 'waiting-manual-video.json'), JSON.stringify({ evidence: results.evidence, limits: results.limits,
    sourceCommit: results.sourceCommit, video: results.video, seeds: results.seeds, markers: results.markers,
    completed: results.ok === true, elapsedMs: results.elapsedMs }, null, 2));
}
