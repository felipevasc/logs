/* Real production WaitingVisuals rendered in an explicitly synthetic comparison.
   CI only: node scripts/preview/run-smoke.mjs test-waiting-reactions.mjs
   Neither receipts nor these capture holds represent native work/performance. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
import { installMotionSampling } from './motion-sampling.mjs';

const output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const context = await browser.newContext({
  viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference',
  recordVideo: { dir: resolve(output, 'video-raw'), size: { width: 1440, height: 960 } },
});
const started = Date.now(), page = await context.newPage(), video = page.video();
await page.addInitScript(installMotionSampling);
page.setDefaultTimeout(15000);
const errors = [], results = {
  evidence: {
    component: 'Production WaitingVisuals, SVG, CSS and reaction controller; no replacement artwork',
    context: 'Actual application in Chromium preview; comparison panel and receipts are explicit test fixtures',
    timing: 'Complete episodes recorded at natural speed; elapsedMs is a supplied synthetic fixture, not measured native latency',
    generatedImageAssets: false, nativeEngineVerified: false, installedWebViewVerified: false,
  },
  markers: [], screenshots: [],
};
page.on('pageerror', error => errors.push(error.message));
const mark = label => results.markers.push({ label, offsetMs: Date.now() - started });
const roots = page.locator('#waiting-reaction-fixture .waiting-visual');
const scene = family => page.locator(`#reaction-${family} .waiting-visual`);
let phase = 'startup', failure = null;
const snap = async filename => {
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  results.screenshots.push(filename);
};
const states = () => roots.evaluateAll(nodes => nodes.map(root => ({
  family: root.dataset.family, episode: root.dataset.episode, motion: root.dataset.motion,
  variant: root.dataset.reactionVariant, status: root.querySelector('.wv-status').textContent,
  metric: root.querySelector('.wv-metric').textContent,
  art: root.querySelector('.wv-art').getBoundingClientRect().toJSON(),
  svgCount: root.querySelectorAll('svg').length,
  moving: root.querySelector('.wv-art').getAnimations({ subtree: true }).filter(a => a.playState === 'running').length,
  animations: root.querySelector('.wv-art').getAnimations({ subtree: true }).map(a => ({
    name: a.animationName, target: a.effect.target.getAttribute('class'),
    currentTime: a.currentTime, playState: a.playState,
  })),
})));
// Run this both before any diagnostic seek and after sampling, so a fixture
// cannot hide a native pause failure or silently disable future CSS governance.
const pauseAndResumeReading = async (label, updateReceipt = false) => {
  const before = await states();
  await scene('reading').locator('.wv-motion-toggle').click();
  const paused = (await states())[0];
  assert.equal(paused.motion, 'static', `${label}: the real control pauses the scene`);
  assert.equal(paused.moving, 0, `${label}: no artwork animation remains running`);
  await page.waitForTimeout(300);
  const stillPaused = (await states())[0];
  assert.equal(stillPaused.episode, 'coffee');
  assert.equal(stillPaused.moving, 0);
  assert.deepEqual(stillPaused.animations.map(animation => animation.currentTime), paused.animations.map(animation => animation.currentTime),
    `${label}: manual pause freezes every existing coffee timeline rather than restarting it`);
  if (updateReceipt) {
    await page.evaluate(() => {
      const receipt = { ...__reactionPreview.receipts.reading, completed: 1300, elapsedMs: 61000 };
      __reactionPreview.views.reading.update(receipt);
    });
    const updated = (await states())[0];
    assert.equal(updated.motion, 'static');
    assert.equal(updated.moving, 0);
    assert.equal(updated.metric, '1.300 / 6.300 registros');
    assert.equal(updated.status, before[0].status);
  }
  await scene('reading').locator('.wv-motion-toggle').click();
  await page.waitForTimeout(200);
  const resumed = await states();
  assert.equal(resumed[0].episode, 'coffee');
  assert.equal(resumed[0].motion, 'running');
  assert.ok(resumed[0].moving > 0, `${label}: original CSS resumes the coffee tracks`);
  assert.ok(resumed[0].animations.some((animation, index) => animation.currentTime > paused.animations[index].currentTime),
    `${label}: the resumed timelines advance`);
  mark(label);
  return { before, paused, stillPaused, resumed };
};
const bothEpisode = episode => page.waitForFunction(episode => [...document.querySelectorAll('#waiting-reaction-fixture .waiting-visual')]
  .length === 2 && [...document.querySelectorAll('#waiting-reaction-fixture .waiting-visual')].every(root => root.dataset.episode === episode), episode, { timeout: 45000 });

try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && state.rows.length > 0 && !state.loadOverlay && document.querySelector('#load-overlay').hidden);
  await page.waitForFunction(() => Tasks.pending() === 0);
  await page.evaluate(async () => {
    await UiScale.set(1, false);
    if (document.documentElement.dataset.theme !== 'dark') toggleTheme();
    const transitions = document.getAnimations().filter(animation => animation instanceof CSSTransition);
    await Promise.all(transitions.map(animation => animation.finished.catch(() => {})));
  });
  phase = 'two real component families';
  await page.evaluate(() => {
    const panel = document.createElement('section'); panel.id = 'waiting-reaction-fixture';
    panel.setAttribute('aria-label', 'Comparação de reações: dados sintéticos');
    Object.assign(panel.style, { position: 'fixed', inset: '160px auto auto 50%', transform: 'translateX(-50%)',
      width: '720px', maxWidth: 'calc(100vw - 32px)', padding: '24px', zIndex: '20000',
      border: '1px solid var(--border)', borderRadius: '12px', background: 'var(--bg-1)', color: 'var(--text-0)' });
    const heading = document.createElement('h2'); heading.textContent = 'Um assistente, tarefas diferentes';
    Object.assign(heading.style, { fontSize: '18px', margin: '0 0 8px' });
    const attribution = document.createElement('p');
    attribution.textContent = 'Componente real · recibos sintéticos · a operação continua durante a reação';
    Object.assign(attribution.style, { fontSize: '12px', color: 'var(--text-1)', margin: '0 0 24px' });
    const pair = document.createElement('div'); Object.assign(pair.style, { display: 'flex', gap: '20px', justifyContent: 'center', flexWrap: 'wrap' });
    panel.append(heading, attribution, pair); document.body.append(panel);
    window.__reactionPreview = { views: {}, receipts: {}, events: [], observers: [], svg: {} };
    for (const [family, phaseId, label] of [['reading', 'metadata-scan', 'Indexando registros'], ['checkpoint', 'metadata-checkpoint-sync', 'Confirmando gravação']]) {
      const host = document.createElement('section'); host.id = `reaction-${family}`;
      Object.assign(host.style, { width: '310px', padding: '12px 8px', border: '1px solid var(--border)', borderRadius: '8px' });
      pair.append(host);
      const receipt = { operationId: `reaction-${family}`, phaseId, state: 'running', label,
        completed: 1200, total: 6300, unit: 'registros', elapsedMs: 60000 };
      // Seeds select coffee at the first safe boundary, without changing eligibility,
      // choreography speed or the product's normal random-selection contract.
      const reactionSeed = family === 'reading' ? 32 : 31;
      const view = WaitingVisuals.mount(host, receipt, { reactionSeed });
      __reactionPreview.views[family] = view; __reactionPreview.receipts[family] = receipt;
      __reactionPreview.svg[family] = view.element.querySelector('svg');
      const observer = new MutationObserver(records => {
        for (const record of records) if (record.attributeName === 'data-episode') {
          __reactionPreview.events.push({ family, episode: view.element.dataset.episode, timeMs: performance.now() });
        }
      });
      observer.observe(view.element, { attributes: true, attributeFilter: ['data-episode'] });
      __reactionPreview.observers.push(observer);
    }
  });
  await page.waitForFunction(() => [...document.querySelectorAll('#waiting-reaction-fixture .waiting-visual')].every(root => root.dataset.motion === 'running'));
  results.initial = await states();
  assert.deepEqual(results.initial.map(s => s.family), ['reading', 'checkpoint']);
  for (const state of results.initial) {
    assert.equal(state.svgCount, 1); assert.equal(state.episode, 'work');
    assert.ok(Math.abs(state.art.width - 240) < .5 && Math.abs(state.art.height - 120) < .5);
  }
  mark('natural-work-start');
  await bothEpisode('coffee');
  mark('both-coffee-started');
  await snap('waiting-reactions-coffee-two-families-dark.png');
  // No seeking, playback-rate change or artificial DOM pose during this segment.
  await bothEpisode('work');
  mark('both-returned-to-work');
  results.sequence = await page.evaluate(() => ({
    events: __reactionPreview.events,
    stableSvg: Object.entries(__reactionPreview.views).every(([family, view]) => view.element.querySelector('svg') === __reactionPreview.svg[family]),
    inspection: Object.fromEntries(Object.entries(__reactionPreview.views).map(([family, view]) => [family, view.inspect()])),
  }));
  assert.equal(results.sequence.stableSvg, true, 'episodes retain each scene and its mounted SVG');
  for (const family of ['reading', 'checkpoint']) {
    const events = results.sequence.events.filter(event => event.family === family);
    const firstCoffee = events.findIndex(event => event.episode === 'coffee');
    assert.ok(firstCoffee >= 0); assert.equal(events[firstCoffee + 1]?.episode, 'work');
    assert.ok(events[firstCoffee + 1].timeMs - events[firstCoffee].timeMs >= 31000, 'coffee plays a complete natural-speed episode');
  }
  await snap('waiting-reactions-returned-dark.png');

  phase = 'pause during a second explicit reaction fixture';
  await page.evaluate(() => {
    __reactionPreview.observers.forEach(observer => observer.disconnect());
    for (const family of ['reading', 'checkpoint']) {
      __reactionPreview.views[family].destroy();
      __reactionPreview.views[family] = WaitingVisuals.mount(document.querySelector(`#reaction-${family}`),
        __reactionPreview.receipts[family], { reactionSeed: family === 'reading' ? 32 : 31 });
    }
  });
  await bothEpisode('coffee');
  results.preSamplingPause = await pauseAndResumeReading('coffee-pause-resume-before-any-seek');
  phase = 'rendered coffee contacts after the uninterrupted video';
  results.rigs = [];
  for (const family of ['reading', 'checkpoint']) {
    const rig = await scene(family).evaluate(async root => {
      const art = root.querySelector('.wv-art'), hand = root.querySelector('.wv-react-hand');
      const held = root.querySelector('.wv-cup-held'), shelf = root.querySelector('.wv-cup-shelf');
      const hatch = root.querySelector('.wv-kitchen-hatch');
      const tracks = art.getAnimations({ subtree: true }).filter(animation => /^(?:wv-coffee-|wv-episode-boundary)/.test(animation.animationName));
      const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
      const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
      const at = fraction => sampling.seek(fraction * 32000);
      const result = { family: root.dataset.family, carriedByHand: held.parentElement === hand,
        contacts: [], hatchGrip: [], renderedSize: art.getBoundingClientRect().toJSON(),
        episodeDurationMs: [...new Set(tracks.map(animation => animation.effect.getComputedTiming().duration))] };
      const sampling = await window.__waitingMotionSampling.begin(tracks);
      try {
        for (const fraction of [.28125, .828125]) {
          await at(fraction);
          result.contacts.push({ fraction, corners: [[92, 50], [99, 58]].map(([x, y]) => distance(point(held, x, y), point(shelf, x, y))) });
        }
        for (const [start, end] of [[.203125, .265625], [.859375, .90625]]) {
          for (let i = 0; i <= 16; i++) {
            const fraction = start + (end - start) * i / 16;
            await at(fraction);
            result.hatchGrip.push({ fraction, distance: distance(point(hand, 91, 55), point(hatch, 173, 55)) });
          }
        }
        return result;
      } finally {
        await sampling.restore();
      }
    });
    assert.equal(rig.carriedByHand, true, `${family}: the cup belongs to its articulated hand`);
    assert.deepEqual(rig.episodeDurationMs, [32000]);
    assert.ok(rig.contacts.every(contact => contact.corners.every(distance => distance < .2)), `${family}: both cup handovers meet exactly`);
    assert.ok(rig.hatchGrip.every(sample => sample.distance < .75), `${family}: hand keeps contact throughout hatch travel`);
    results.rigs.push(rig);
  }
  phase = 'pause, receipts and hidden context';
  results.midEpisodePause = await pauseAndResumeReading('coffee-pause-resume-after-contact-sampling', true);
  await page.evaluate(() => Object.values(__reactionPreview.views).forEach(view => view.setVisible(false)));
  const hidden = await states();
  assert.ok(hidden.every(state => state.motion === 'static' && state.moving === 0));
  await page.waitForTimeout(300);
  assert.deepEqual((await states()).map(state => state.animations.map(animation => animation.currentTime)),
    hidden.map(state => state.animations.map(animation => animation.currentTime)), 'explicit hiding preserves paused timeline positions');
  await page.evaluate(() => Object.values(__reactionPreview.views).forEach(view => view.setVisible(true)));
  await page.waitForFunction(() => [...document.querySelectorAll('#waiting-reaction-fixture .waiting-visual')].every(root => root.dataset.motion === 'running'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.waitForFunction(() => [...document.querySelectorAll('#waiting-reaction-fixture .waiting-visual')].every(root => root.dataset.motion === 'static'));
  assert.ok((await states()).every(state => state.moving === 0));
  assert.equal(await roots.locator('.wv-motion-toggle:visible').count(), 0);
  await snap('waiting-reactions-reduced-motion-dark.png');
  phase = 'immediate terminal and disposal';
  results.terminal = await page.evaluate(() => {
    __reactionPreview.views.checkpoint.update({ ...__reactionPreview.receipts.checkpoint, phaseId: 'metadata-checkpoint-committed' });
    __reactionPreview.views.reading.update({ ...__reactionPreview.receipts.reading, state: 'cancelling' });
    return Object.values(__reactionPreview.views).map(view => ({ state: view.element.dataset.state, motion: view.element.dataset.motion, inspection: view.inspect() }));
  });
  assert.ok(results.terminal.every(state => state.motion === 'static'));
  await page.evaluate(() => {
    __reactionPreview.observers.forEach(observer => observer.disconnect());
    Object.values(__reactionPreview.views).forEach(view => view.destroy());
    document.querySelector('#waiting-reaction-fixture').remove();
  });
  assert.equal(await roots.count(), 0); assert.deepEqual(errors, []);
  results.ok = true; mark('finished');
} catch (error) {
  failure = error; results.ok = false; results.error = String(error.stack || error);
  try { results.failureStates = await states(); } catch {}
  await captureFailure(page, 'waiting-reactions', error, { phase, errors, results });
  throw error;
} finally {
  results.phase = phase; results.errors = errors;
  try {
    await context.close();
    if (!video) throw Error('Reaction recording is unavailable');
    const filename = 'waiting-reactions-real-preview.webm';
    await video.saveAs(resolve(output, filename));
    results.video = { filename, bytes: statSync(resolve(output, filename)).size, finalized: true };
    await video.delete();
  } catch (error) { results.videoError = String(error); if (!failure) process.exitCode = 1; }
  writeFileSync(resolve(output, 'waiting-reactions-results.json'), JSON.stringify(results, null, 2));
  await browser.close();
}
