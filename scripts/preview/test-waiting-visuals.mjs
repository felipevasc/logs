/* Browser regression for the production waiting pilots.
   Run remotely: node scripts/preview/run-smoke.mjs test-waiting-visuals.mjs
   The app/controllers/CSS are unmodified. Preview transport is gated below so real
   production promises remain pending; native-like metadata receipts and standalone
   component states are explicitly fixtures. This does NOT validate the native engine.
   No image assets are generated: screenshots capture the rendered application. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const context = await browser.newContext({
  viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference',
  // Explicit size avoids Playwright's default downscaling to an 800px box.
  recordVideo: { dir: resolve(output, 'video-raw'), size: { width: 1440, height: 960 } },
});
const videoStartedAt = Date.now();
const page = await context.newPage();
const video = page.video();
const videoMetadata = {
  filename: 'waiting-visuals-real-preview.webm', finalized: false,
  capture: 'Real Playwright browser recording of this production UI regression; no generated image assets',
  transport: 'Synthetic preview command transport with test-only gates; metadata/checkpoint receipts are explicit fixtures',
  nativeEngineVerified: false,
  timingReference: 'Approximate wall-clock offsets since context.newPage() was requested; inspect the actual video before trimming',
  recordedSize: { width: 1440, height: 960 },
  captureHoldBudgetMs: 28600,
  captureHoldBudgetNote: '9.2s reader/pause evidence plus full 6.8s checkpoint, 6.4s calculation, 2.8s/2.6s short gestures and frame overhead; test-only pending transport, never product latency',
  markers: [],
};
const markVideo = (label, details = {}) => videoMetadata.markers.push({ label, offsetMs: Date.now() - videoStartedAt, ...details });
let originalTestError = null;
page.setDefaultTimeout(20_000);
const errors = [];
const results = {
  evidence: {
    app: 'Actual production DOM, CSS, WaitingVisuals, WaitingProgress, Tasks, loadData, runGroup and runCube',
    transport: 'Synthetic preview data; a test-only gate holds existing mock commands before their unchanged handlers return',
    loadPhases: 'Built-in preview parse/ready events plus explicitly synthetic metadata/checkpoint/error receipts through the real operation-progress listener',
    calculationPhases: 'Real command-level pending waits with measured elapsed time; unchanged preview calculation handlers produce the final data',
    componentStates: 'Explicit standalone fixture for short gestures, paused, cancelling, error, unknown, reduced-motion, forced-colors and visibility states',
    decorativeTiming: 'Rendered contact sampling and screenshot poses seek only CSS animation timelines; full-cycle video markers identify uninterrupted real-time playback',
    nativeEngineVerified: false,
    generatedImageAssets: false,
  },
  screenshots: [],
  videoMetadata: 'waiting-visuals-video.json',
};
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));

// Instrument only the preview transport, before app.js captures invoke. Existing
// handlers, event routing, Tasks ownership and production adapters are not replaced.
await page.route('**/__mock__.js', async route => {
  const response = await route.fetch();
  let code = await response.text();
  const emitter = '  const emitMock = (name, payload) => (listeners[name] || []).forEach((cb) => cb({ payload }));';
  const result = '          let result = await h(args);';
  assert.equal(code.split(emitter).length, 2, 'preview emitter hook must be unique');
  assert.equal(code.split(result).length, 2, 'preview result hook must be unique');
  code = code.replace(emitter, `${emitter}
  // Test-only transport boundary; never included in the shipped app or screenshots as an image asset.
  window.__waitingPreviewBridge = {
    held: new Set(), pending: [], emit: emitMock,
    beforeResult(command, operationId) {
      if (!this.held.has(command)) return;
      return new Promise((resolve, reject) => this.pending.push({ command, operationId, release: resolve, reject }));
    },
    release(command) {
      this.held.delete(command);
      const ready = this.pending.filter(item => item.command === command);
      this.pending = this.pending.filter(item => item.command !== command);
      for (const item of ready) item.release();
    },
    fail(command, message) {
      this.held.delete(command);
      const ready = this.pending.filter(item => item.command === command);
      this.pending = this.pending.filter(item => item.command !== command);
      for (const item of ready) item.reject(new Error(message));
    }
  };`);
  code = code.replace(result, `          await window.__waitingPreviewBridge.beforeResult(cmd, args.operationId);\n${result}`);
  await route.fulfill({ response, body: code });
});

const load = page.locator('#load-visual .waiting-visual');
const group = page.locator('#tab-group .area-loading-semantic .waiting-visual');
const pivot = page.locator('.cube-output .area-loading-semantic .waiting-visual');
// Theme changes transition many real app surfaces. Wait for those finite CSS
// transitions to finish naturally; never fast-forward the robot's animations.
const settleTransitions = () => page.evaluate(async () => {
  const frames = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const deadline = performance.now() + 5000;
  await frames();
  for (;;) {
    const transitions = document.getAnimations().filter(animation => animation instanceof CSSTransition
      && (animation.playState === 'running' || animation.pending));
    if (!transitions.length) return;
    if (performance.now() > deadline) throw new Error('App CSS transitions did not settle');
    await Promise.all(transitions.map(animation => animation.finished.catch(() => {})));
    await frames();
  }
});
const setTheme = async theme => {
  await page.evaluate(theme => {
    if (document.documentElement.dataset.theme !== theme) toggleTheme();
  }, theme);
  await settleTransitions();
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), theme);
};
const waitMotion = (selector, value) => page.waitForFunction(({ selector, value }) => document.querySelector(selector)?.dataset.motion === value, { selector, value });
const renderedState = locator => locator.evaluate(root => {
  const status = root.querySelector('.wv-status'), metric = root.querySelector('.wv-metric'), button = root.querySelector('.wv-motion-toggle');
  const art = root.querySelector('.wv-art');
  const running = root.getAnimations({ subtree: true }).filter(animation => animation.playState === 'running');
  const rect = root.getBoundingClientRect();
  return {
    family: root.dataset.family, motion: root.dataset.motion, pace: root.dataset.pace, state: root.dataset.state, variant: root.dataset.variant,
    status: status.textContent, metric: metric.hidden ? null : metric.textContent,
    statusFont: Number.parseFloat(getComputedStyle(status).fontSize), metricFont: Number.parseFloat(getComputedStyle(metric).fontSize),
    buttonFont: Number.parseFloat(getComputedStyle(button).fontSize), buttonMinHeight: Number.parseFloat(getComputedStyle(button).minHeight),
    buttonPointerEvents: getComputedStyle(button).pointerEvents, buttonHidden: button.hidden,
    // Pausing the scene must be immediate. A finite button hover transition is
    // not a running robot loop (CI56 captured one 150 ms color transition).
    runningAnimations: art.getAnimations({ subtree: true }).filter(animation => animation.playState === 'running').length,
    controlAnimations: running.filter(animation => !art.contains(animation.effect.target)).map(animation => ({
      type: animation.constructor.name, target: animation.effect.target.className,
      property: animation.transitionProperty || null, duration: animation.effect.getComputedTiming().duration,
      iterations: animation.effect.getTiming().iterations,
    })),
    surface: { background: getComputedStyle(root).backgroundColor, opacity: getComputedStyle(root).opacity },
    bounds: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
  };
});
const assertTypography = state => {
  assert.ok(state.statusFont >= 13, 'main status stays legible');
  assert.ok(state.metricFont >= 12, 'metrics stay legible');
  assert.ok(state.buttonFont >= 12, 'pause control stays legible');
  assert.ok(state.buttonMinHeight >= 24, 'pause target keeps its minimum height');
};
const assertFits = async locator => {
  const box = await locator.boundingBox(), viewport = page.viewportSize();
  assert.ok(box && box.width > 0 && box.height > 0, 'scene is actually rendered');
  assert.ok(box.x >= -1 && box.y >= -1 && box.x + box.width <= viewport.width + 1 && box.y + box.height <= viewport.height + 1,
    `scene fits the ${viewport.width}px viewport: ${JSON.stringify(box)}`);
};
async function screenshot(filename, attribution) {
  await settleTransitions();
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  results.screenshots.push({ filename, ...attribution, viewport: page.viewportSize(), theme: await page.evaluate(() => document.documentElement.dataset.theme) });
}
// Contact evidence uses actual rendered SVG matrices at the shipping 240x120
// CSS size. Only decorative CSS timelines are sought; receipts never advance.
async function inspectTaskRig(locator, family, pace = 'loop') {
  const rig = await locator.evaluate(async (root, { family, pace }) => {
    const art = root.querySelector('.wv-art'), actor = root.querySelector('.wv-task');
    const hand = actor.querySelector('.wv-task-hand');
    const animations = art.getAnimations({ subtree: true });
    const duration = family === 'checkpoint' ? (pace === 'loop' ? 6800 : 2800) : (pace === 'loop' ? 6400 : 2600);
    const frames = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const at = async fraction => {
      for (const animation of animations) { animation.pause(); animation.currentTime = fraction * duration; }
      await frames();
    };
    const matrix = node => { const m = node.getScreenCTM(); return [m.a, m.b, m.c, m.d, m.e, m.f]; };
    const point = (node, x, y) => { const p = new DOMPoint(x, y).matrixTransform(node.getScreenCTM()); return { x: p.x, y: p.y }; };
    const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const transform = node => {
      const m = new DOMMatrix(getComputedStyle(node).transform);
      return { a: m.a, b: m.b, c: m.c, d: m.d, x: m.e, y: m.f };
    };
    const opacity = node => Number(getComputedStyle(node).opacity);
    const result = {
      family: root.dataset.family, pace: root.dataset.pace, actorCount: root.querySelectorAll('.wv-task').length,
      art: { width: art.getBoundingClientRect().width, height: art.getBoundingClientRect().height },
      durations: [...new Set(animations.map(animation => animation.effect.getComputedTiming().duration))],
      iterations: [...new Set(animations.map(animation => String(animation.effect.getTiming().iterations)))],
      animatedParts: animations.map(animation => animation.effect.target.getAttribute('class')),
    };
    try {
      await at(0);
      result.actorHeight = actor.getBoundingClientRect().height;
      if (family === 'checkpoint') {
        const drawer = root.querySelector('.wv-archive-drawer'), folder = root.querySelector('.wv-archive-folder');
        const label = root.querySelector('.wv-archive-label');
        result.folderBelongsToDrawer = folder.parentElement === drawer;
        result.folderTransform = getComputedStyle(folder).transform;
        const from = pace === 'loop' ? 18 : 24, to = pace === 'loop' ? 64 : 76;
        result.grip = [];
        // One-percent samples include interpolated poses, not just authored keys.
        for (let percent = from; percent <= to; percent++) {
          await at(percent / 100);
          const fingers = point(hand, 91, 55), handle = point(drawer, 110, 59);
          result.grip.push({ fraction: percent / 100, distance: distance(fingers, handle),
            ...([from, pace === 'loop' ? 39 : 49, to].includes(percent)
              ? { handMatrix: matrix(hand), drawerMatrix: matrix(drawer), fingers, handle } : {}) });
        }
        result.reactions = [];
        for (const fraction of (pace === 'loop' ? [0, .17, .32, .46, .64, .67, .82, .9] : [0, .23, .45, .52, .76, .9])) {
          await at(fraction);
          result.reactions.push({ fraction, drawer: transform(drawer), label: transform(label) });
        }
      } else {
        const held = root.querySelector('.wv-group-held'), source = root.querySelector('.wv-group-source');
        const filed = root.querySelector('.wv-group-filed'), well = root.querySelector('.wv-group-well');
        result.carriedByHand = held.parentElement === hand;
        result.contacts = [];
        for (const [fraction, station] of (pace === 'loop' ? [[.22, source], [.66, filed]] : [[.35, source]])) {
          await at(fraction);
          const corners = [[93, 47], [103, 57]].map(([x, y]) => {
            const carried = point(held, x, y), target = point(station, x, y);
            return { carried, target, distance: distance(carried, target) };
          });
          result.contacts.push({ fraction, station: station.getAttribute('class'), corners,
            distance: Math.max(...corners.map(corner => corner.distance)),
            heldMatrix: matrix(held), stationMatrix: matrix(station) });
        }
        result.reactions = [];
        for (const fraction of (pace === 'loop' ? [.21, .22, .4, .65, .66, .69, .74, .97] : [.34, .35, .68, .78, .88, 1])) {
          await at(fraction);
          result.reactions.push({ fraction, heldOpacity: opacity(held), sourceOpacity: opacity(source),
            filedOpacity: opacity(filed), heldTransform: getComputedStyle(held).transform,
            well: transform(well), visor: transform(root.querySelector('.wv-task-visor')) });
        }
      }
      return result;
    } finally {
      for (const animation of animations) { animation.currentTime = 0; animation.play(); }
      await frames();
    }
  }, { family, pace });
  assert.equal(rig.family, family); assert.equal(rig.pace, pace);
  assert.equal(rig.actorCount, 1, `${family}: one protagonist`);
  assert.ok(rig.actorHeight >= 60, `${family}: robot is legible at actual rendered size`);
  assert.ok(Math.abs(rig.art.width - 240) < .5 && Math.abs(rig.art.height - 120) < .5,
    `${family}: production art is 240x120, not a scaled-up test fixture`);
  assert.deepEqual(rig.durations, [family === 'checkpoint' ? (pace === 'loop' ? 6800 : 2800) : (pace === 'loop' ? 6400 : 2600)]);
  assert.deepEqual(rig.iterations, [pace === 'loop' ? 'Infinity' : '1']);
  for (const part of ['wv-task-body', 'wv-task-head', 'wv-task-arm', 'wv-task-hand', 'wv-task-gaze']) {
    assert.ok(rig.animatedParts.includes(part), `${family}: articulated ${part} drives the story`);
  }
  const reaction = fraction => rig.reactions.find(sample => sample.fraction === fraction);
  const near = (actual, expected, message) => assert.ok(Math.abs(actual - expected) < .02, `${message}: ${actual}`);
  if (family === 'checkpoint') {
    assert.equal(rig.folderBelongsToDrawer, true); assert.equal(rig.folderTransform, 'none');
    assert.ok(rig.grip.every(sample => sample.distance < .75), 'fingers maintain the drawer grip throughout its travel');
    near(reaction(pace === 'loop' ? .17 : .23).drawer.x, 0, 'drawer waits for hand contact');
    near(reaction(pace === 'loop' ? .32 : .45).drawer.x, pace === 'loop' ? -10 : -3, 'drawer opens under the hand');
    near(reaction(pace === 'loop' ? .64 : .76).drawer.x, 0, 'drawer closes before release');
    for (const sample of rig.reactions.filter(sample => pace !== 'loop' || sample.fraction <= .64)) {
      near(sample.label.b, 0, 'archive label does not react before the drawer closes');
    }
    if (pace === 'loop') {
      assert.ok(Math.abs(reaction(.67).label.b) > .1, 'archive label reacts causally after closure');
      near(reaction(.82).label.b, 0, 'archive reaction settles');
    }
  } else {
    assert.equal(rig.carriedByHand, true, 'calculation tile is a direct child of the articulated hand');
    assert.ok(rig.contacts.every(sample => sample.distance < .75), 'carried corners meet the source/destination exactly');
    assert.ok(rig.reactions.every(sample => sample.heldTransform === 'none'), 'carried tile has no independent transform');
    near(reaction(pace === 'loop' ? .21 : .34).heldOpacity, 0, 'tile is not carried before pickup');
    near(reaction(pace === 'loop' ? .22 : .35).heldOpacity, 1, 'tile transfers into hand at pickup');
    near(reaction(pace === 'loop' ? .22 : .35).sourceOpacity, .28, 'source reacts only once picked up');
    if (pace === 'loop') {
      near(reaction(.65).filedOpacity, 0, 'destination stays empty before drop');
      near(reaction(.66).heldOpacity, 0, 'hand releases at drop'); near(reaction(.66).filedOpacity, 1, 'destination receives at drop');
      for (const sample of rig.reactions.filter(sample => sample.fraction <= .66)) near(sample.well.y, 0, 'well waits for deposit');
      near(reaction(.69).well.y, 1.2, 'well responds after deposit'); near(reaction(.74).well.y, 0, 'well settles');
    } else {
      near(reaction(1).heldOpacity, 1, 'short gesture ends still holding its card');
      assert.ok(rig.reactions.every(sample => sample.filedOpacity === 0 && sample.well.y === 0), 'short inspection never invents a deposit');
      near(reaction(.68).visor.a, 1, 'short inspection reaches the visor before squinting');
      near(reaction(.78).visor.a, .65, 'short inspection squints at its own causal beat');
      near(reaction(.88).visor.a, 1, 'short inspection reopens the visor');
    }
  }
  return rig;
}

async function recordSceneCycle(locator, label, durationMs, attribution) {
  markVideo(`${label}-start`, { ...attribution, durationMs, animationRestartedForCapture: true, viewport: page.viewportSize() });
  const capture = await locator.evaluate(async (root, durationMs) => {
    const animations = root.querySelector('.wv-art').getAnimations({ subtree: true });
    if (!animations.length) throw new Error('Cannot record an absent decorative timeline');
    for (const animation of animations) { animation.currentTime = 0; animation.play(); }
    await Promise.all(animations.map(animation => animation.ready));
    const started = performance.now();
    return new Promise((resolve, reject) => {
      const frame = () => {
        const times = animations.map(animation => animation.currentTime);
        if (!root.isConnected || root.dataset.motion !== 'running') return reject(new Error('Scene stopped before its capture cycle ended'));
        if (times.every(time => typeof time === 'number' && time >= durationMs)) {
          return resolve({ requestedDurationMs: durationMs, elapsedMs: performance.now() - started,
            minimumTimelineMs: Math.min(...times), playStates: [...new Set(animations.map(animation => animation.playState))] });
        }
        if (performance.now() - started > durationMs + 10000) return reject(new Error('Decorative capture cycle did not advance'));
        requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    });
  }, durationMs);
  assert.ok(capture.minimumTimelineMs >= durationMs, 'video contains the complete real-time decorative cycle');
  markVideo(`${label}-end`, { ...attribution, ...capture });
  return capture;
}

async function screenshotPose(locator, fraction, durationMs, filename, attribution) {
  await locator.evaluate(async (root, { fraction, durationMs }) => {
    const animations = root.querySelector('.wv-art').getAnimations({ subtree: true });
    for (const animation of animations) { animation.pause(); animation.currentTime = fraction * durationMs; }
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  }, { fraction, durationMs });
  try { await screenshot(filename, { ...attribution, decorativePoseFraction: fraction, animationSeekForScreenshot: true }); }
  finally {
    await locator.evaluate(root => {
      for (const animation of root.querySelector('.wv-art').getAnimations({ subtree: true })) { animation.currentTime = 0; animation.play(); }
    });
  }
}

async function emitLoadPhase(overrides = {}) {
  return page.evaluate(overrides => {
    const pending = window.__waitingPreviewBridge.pending.find(item => item.command === 'load_file');
    if (!pending) throw new Error('The production load must still be pending before emitting a fixture receipt');
    const payload = {
      operationId: pending.operationId, operation: 'carregamento', phaseId: 'metadata-scan', phase: 'Indexando metadados (fixture)',
      completed: 1200, total: 6300, unit: 'registros', elapsedMs: performance.now() - window.__waitingLoadStarted,
      cancellable: true, fixture: true, ...overrides,
    };
    window.__waitingPreviewBridge.emit('operation-progress', payload);
    return payload;
  }, overrides);
}

try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && state.rows.length > 0 && !state.loadOverlay && document.querySelector('#load-overlay').hidden);
  await page.waitForFunction(() => Tasks.pending() === 0);
  await page.evaluate(async () => {
    await UiScale.set(1, false);
    window.__waitingObservedProgress = [];
    await window.__TAURI__.event.listen('operation-progress', ({ payload }) => window.__waitingObservedProgress.push(payload));
  });
  await setTheme('dark');

  phase = 'real load controller and built-in preview progress';
  await page.evaluate(() => {
    window.__waitingPreviewBridge.held.add('load_file');
    window.__waitingLoadStarted = performance.now();
    // A preview path, intentionally never sent to a real native filesystem.
    window.__waitingLoadWork = loadData({ kind: 'file', path: 'C:\\mock\\waiting-visuals.jsonl', paths: ['C:\\mock\\waiting-visuals.jsonl'], format: 'auto' })
      .then(ok => ({ ok, disposedAtSettlement: !document.querySelector('#load-visual .waiting-visual'), overlayHiddenAtSettlement: document.querySelector('#load-overlay').hidden }));
  });
  await load.waitFor({ state: 'visible' });
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'load_file'));
  results.builtInPreview = await page.evaluate(() => {
    const operationId = window.__waitingPreviewBridge.pending.find(item => item.command === 'load_file').operationId;
    const events = window.__waitingObservedProgress.filter(item => item.operationId === operationId);
    const root = document.querySelector('#load-visual .waiting-visual');
    return { operationId, owner: Tasks.operationFor('source-load'), phases: events.map(item => item.phaseId),
      lastReceipt: events.at(-1), family: root.dataset.family, state: root.dataset.state, metric: root.querySelector('.wv-metric').textContent,
      overlayVisible: !document.querySelector('#load-overlay').hidden };
  });
  assert.equal(results.builtInPreview.operationId, results.builtInPreview.owner);
  assert.ok(results.builtInPreview.phases.includes('parse')); assert.equal(results.builtInPreview.lastReceipt.phaseId, 'ready');
  assert.equal(results.builtInPreview.metric, '6.300 / 6.300 linhas');
  assert.equal(results.builtInPreview.family, 'neutral', 'unrecognized preview phase IDs do not fabricate a semantic scene');
  assert.equal(results.builtInPreview.state, 'running', 'a phase ready event does not settle its pending owner');
  assert.equal(await load.locator('.wv-status').textContent(), 'Confirmando abertura', 'the ready phase cannot announce a confirmed load before owner settlement');
  assert.equal(results.builtInPreview.overlayVisible, true);
  assert.equal(await page.locator('#load-cancel').isDisabled(), true, 'the real preview ready phase is noncancellable');

  phase = 'fixture receipts through real foreground progress adapter';
  await page.waitForFunction(() => performance.now() - window.__waitingLoadStarted >= 4100);
  await emitLoadPhase();
  await waitMotion('#load-visual .waiting-visual', 'running');
  results.reading = await renderedState(load);
  assert.equal(results.reading.family, 'reading'); assert.equal(results.reading.pace, 'loop');
  assert.equal(results.reading.status, 'Indexando registros'); assert.equal(results.reading.metric, '1.200 / 6.300 registros');
  assert.ok(results.reading.runningAnimations > 0); assertTypography(results.reading);
  assert.equal(await page.locator('#load-progress-details').evaluate(node => node.open), false);
  assert.equal(await page.locator('#load-cancel').isEnabled(), true, 'the exact owner receipt makes its card Cancel reachable');
  await assertFits(page.locator('#load-cancel'));
  await page.locator('#load-cancel').focus();
  await emitLoadPhase({ cancellable: false });
  assert.equal(await page.locator('#load-cancel').isDisabled(), true);
  assert.equal(await page.locator('#load-progress-details > summary').evaluate(node => node === document.activeElement), true,
    'noncancellable transition transfers focus from the now-disabled button');
  await emitLoadPhase();
  await assertFits(load);
  // Inspect real SVG/CSS contact poses, then restart only the decorative timeline
  // to record a complete story. This does not advance or alter operation progress.
  results.readingRig = await load.evaluate(async root => {
    const art = root.querySelector('.wv-art'), actor = root.querySelector('.wv-reader');
    const hand = root.querySelector('.wv-reader-hand'), paper = root.querySelector('.wv-read-card');
    const animations = art.getAnimations({ subtree: true });
    const durations = [...new Set(animations.map(animation => animation.effect.getComputedTiming().duration))];
    const frames = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const point = (node, x, y) => new DOMPoint(x, y).matrixTransform(node.getScreenCTM());
    const contact = async (fraction, target) => {
      for (const animation of animations) { animation.pause(); animation.currentTime = fraction * 7200; }
      await frames();
      const distances = [[90, 34], [108, 62]].map(([x, y]) => {
        const carried = point(paper, x, y), station = point(root.querySelector(target), x, y);
        return Math.hypot(carried.x - station.x, carried.y - station.y);
      });
      return { fraction, distance: Math.max(...distances), distances };
    };
    try {
      const pickup = await contact(.22, '.wv-reader-source');
      const deposit = await contact(.67, '.wv-reader-filed');
      return { actorCount: root.querySelectorAll('.wv-reader').length,
        height: actor.getBoundingClientRect().height, carriedByHand: paper.parentElement === hand,
        paperTransform: getComputedStyle(paper).transform, durations, pickup, deposit };
    } finally {
      for (const animation of animations) { animation.currentTime = 0; animation.play(); }
      await frames();
    }
  });
  assert.equal(results.readingRig.actorCount, 1, 'one legible protagonist');
  assert.ok(results.readingRig.height >= 60, 'the character is not a tiny decorative bystander');
  assert.equal(results.readingRig.carriedByHand, true, 'the payload belongs to the articulated hand');
  assert.equal(results.readingRig.paperTransform, 'none', 'the carried paper has no independent movement');
  assert.deepEqual(results.readingRig.durations, [7200]);
  assert.ok(results.readingRig.pickup.distance < .75, 'hand and source sheet meet at pickup');
  assert.ok(results.readingRig.deposit.distance < .75, 'hand and destination sheet meet at release');
  markVideo('reading-loop-start', { fixtureReceipt: true, animationRestartedForCapture: true, viewport: page.viewportSize() });
  await screenshot('waiting-fixture-reading-dark-1440.png', { fixture: true, content: 'Synthetic metadata receipt rendered by the production foreground adapter while its preview load command is pending' });

  // Capture-only dwell in the already pending command: one full 7.2s character
  // story before pause/resume. Reader-only dwell is 9.2s, never product latency.
  await page.waitForTimeout(7600);
  const beforeForeign = await load.textContent();
  await page.evaluate(() => window.__waitingPreviewBridge.emit('operation-progress', {
    operationId: 'fixture-unowned-background', phaseId: 'analytics-sql', phase: 'Must not replace foreground', completed: 999, total: 999, unit: 'itens', fixture: true,
  }));
  assert.equal(await load.textContent(), beforeForeign, 'foreign operation cannot replace the foreground scene');
  const cancelBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
  await load.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter');
  assert.equal((await renderedState(load)).motion, 'static');
  markVideo('reading-paused-by-keyboard', { fixtureReceipt: true });
  await page.waitForTimeout(500);
  await emitLoadPhase({ completed: 1800 });
  assert.equal((await renderedState(load)).motion, 'static', 'new receipts preserve the user pause');
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), cancelBefore);
  assert.equal(await page.evaluate(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'load_file')), true);
  await load.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter');
  await waitMotion('#load-visual .waiting-visual', 'running');
  markVideo('reading-resumed-by-keyboard', { fixtureReceipt: true });
  await page.waitForTimeout(1100);
  markVideo('reading-segment-end', { fixtureReceipt: true });
  await setTheme('light');
  await assertFits(load);
  await screenshot('waiting-fixture-reading-light-1440.png', { fixture: true, content: 'Articulated reader in the light theme; real component with a controlled metadata receipt' });

  await emitLoadPhase({ total: 0, completed: 1800 });
  assert.equal((await renderedState(load)).metric, '1.800 registros');
  assert.equal(await page.locator('#load-bar-fill').evaluate(node => node.parentElement.hidden), true);
  await emitLoadPhase({ phaseId: 'metadata-checkpoint-sync', phase: 'Sincronizando checkpoint (fixture)', completed: 0, total: 0, unit: '' });
  assert.equal((await renderedState(load)).family, 'checkpoint'); assert.equal((await renderedState(load)).metric, null);
  phase = 'real checkpoint pilot: rendered grip and full-cycle evidence';
  await waitMotion('#load-visual .waiting-visual', 'running');
  await setTheme('dark');
  results.checkpointRig = await inspectTaskRig(load, 'checkpoint');
  results.checkpointCapture = await recordSceneCycle(load, 'checkpoint-loop', 6800, { fixtureReceipt: true, syntheticTransport: true });
  await screenshotPose(load, .4, 6800, 'waiting-fixture-checkpoint-dark-1440.png', {
    fixture: true, syntheticTransport: true, content: 'Production foreground adapter, synthetic checkpoint-sync receipt; robot maintains grip on its open drawer' });
  await setTheme('light');
  await screenshotPose(load, .4, 6800, 'waiting-fixture-checkpoint-light-1440.png', {
    fixture: true, syntheticTransport: true, content: 'Same checkpoint grip in the light theme after real app transitions settle' });
  await emitLoadPhase({ phaseId: 'metadata-checkpoint-committed', phase: 'Checkpoint de metadados preservado (fixture)', completed: 6300, total: 6300, unit: 'bytes', checkpointRows: 1200 });
  results.partialCheckpoint = await renderedState(load);
  markVideo('partial-checkpoint', { fixtureReceipt: true });
  assert.equal(results.partialCheckpoint.family, 'checkpoint'); assert.equal(results.partialCheckpoint.motion, 'static');
  assert.equal(results.partialCheckpoint.state, 'running'); assert.equal(results.partialCheckpoint.status, 'Checkpoint preservado');
  assert.equal(results.partialCheckpoint.metric, '6.300 / 6.300 bytes');
  assert.equal(await page.locator('#load-overlay').isVisible(), true, 'a preserved partial checkpoint never closes the operation');
  await page.locator('#load-progress-details > summary').click();
  assert.equal(await page.locator('#load-progress-details').evaluate(node => node.open), true);
  assert.match(await page.locator('#load-phase').textContent(), /Checkpoint de metadados preservado/);
  assert.match(await page.locator('#load-volume').textContent(), /6\.300 \/ 6\.300 bytes/);
  await page.setViewportSize({ width: 1024, height: 768 }); await setTheme('light');
  await assertFits(load); await assertFits(page.locator('.semantic-load-stage'));
  await screenshot('waiting-fixture-checkpoint-light-1024.png', { fixture: true, content: 'Partial checkpoint with expanded real diagnostics; full receipt counter does not imply operation success' });

  await emitLoadPhase({ phaseId: 'future-native-phase', phase: 'Fase desconhecida (fixture)', completed: undefined, total: undefined, unit: '' });
  assert.equal((await renderedState(load)).family, 'neutral'); assert.equal((await renderedState(load)).metric, null);
  await emitLoadPhase({ error: 'Falha simulada de progresso', completed: undefined, total: undefined, unit: '' });
  assert.equal((await renderedState(load)).state, 'error'); assert.equal((await renderedState(load)).motion, 'static');
  await emitLoadPhase();
  await page.locator('#load-cancel').focus();
  // Hold an existing post-publication read to observe real frontend finalization,
  // with the source task settled and the foreground controller still pending.
  await page.evaluate(() => { window.__waitingPreviewBridge.held.add('get_ts_config'); window.__waitingPreviewBridge.release('load_file'); });
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'get_ts_config'));
  assert.equal(await page.locator('#load-cancel').isDisabled(), true);
  assert.equal(await page.locator('#load-overlay').isVisible(), true);
  assert.equal(await page.evaluate(() => Tasks.operationFor('source-load')), null);
  assert.equal(await page.locator('#load-progress-details > summary').evaluate(node => node === document.activeElement), true);
  await page.locator('#btn-theme').focus();
  results.loadSettlement = await page.evaluate(async () => { window.__waitingPreviewBridge.release('get_ts_config'); return window.__waitingLoadWork; });
  assert.equal(results.loadSettlement.ok, true); assert.equal(results.loadSettlement.disposedAtSettlement, true); assert.equal(results.loadSettlement.overlayHiddenAtSettlement, true);
  assert.equal(await page.locator('#btn-theme').evaluate(node => node === document.activeElement), true, 'settlement does not steal focus outside the overlay');
  assert.equal(await page.locator('#load-visual .waiting-visual').count(), 0, 'no cosmetic delay after owner settlement');

  phase = 'foreground operation Cancel reached by actual mouse and keyboard';
  results.foregroundCancel = [];
  for (const input of ['mouse', 'keyboard']) {
    await page.locator('#btn-theme').focus();
    await page.evaluate(input => {
      window.__waitingPreviewBridge.held.add('load_file');
      window.__waitingLoadStarted = performance.now();
      const path = `C:\\mock\\waiting-cancel-${input}.jsonl`;
      window.__waitingLoadWork = loadData({ kind: 'file', path, paths: [path], format: 'auto' })
        .then(ok => ({ ok, disposedAtSettlement: !document.querySelector('#load-visual .waiting-visual'), overlayHiddenAtSettlement: document.querySelector('#load-overlay').hidden }));
    }, input);
    await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'load_file'));
    await emitLoadPhase();
    const operationId = await page.evaluate(() => window.__waitingPreviewBridge.pending.find(item => item.command === 'load_file').operationId);
    const countBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
    await assertFits(page.locator('#load-cancel'));
    if (input === 'mouse') await page.locator('#load-cancel').click();
    else {
      await load.locator('.wv-motion-toggle').focus();
      await page.keyboard.press('Tab');
      assert.equal(await page.locator('#load-cancel').evaluate(node => node === document.activeElement), true, 'native tab order reaches operation Cancel after decorative Pause');
      await page.keyboard.press('Enter');
    }
    await page.waitForFunction(() => document.querySelector('#load-visual .waiting-visual')?.dataset.state === 'cancelling');
    const pending = await renderedState(load);
    assert.equal(pending.motion, 'static'); assert.equal(pending.runningAnimations, 0);
    assert.match(pending.status, /Cancelando/);
    assert.equal(await page.locator('#load-cancel').isDisabled(), true);
    assert.equal(await page.locator('#load-progress-details > summary').evaluate(node => node === document.activeElement), true);
    assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), countBefore + 1);
    assert.equal(await page.evaluate(() => window.__mockRequests.filter(item => item.cmd === 'cancel_task').at(-1).operationId), operationId,
      'the actual button dispatches only its visible operation ID');
    await page.keyboard.press('Enter'); // Focus is now on details, never a second Cancel.
    assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), countBefore + 1);
    assert.equal(await page.evaluate(id => window.__waitingPreviewBridge.pending.some(item => item.operationId === id), operationId), true,
      'requesting cancellation never settles or hides the owner');
    await emitLoadPhase();
    assert.equal((await renderedState(load)).state, 'cancelling', 'a late receipt cannot restart the cancelled owner');
    markVideo(`foreground-${input}-cancel-pending`, { fixtureReceipt: true, syntheticTransport: true });
    await screenshot(`waiting-foreground-${input}-cancel-pending.png`, { fixture: true, syntheticTransport: true,
      content: `Production Cancel activated by ${input}; exact source task remains pending, with a static waiting scene` });
    // The first gate explicitly rejects as a cancellation fixture. The second
    // releases the unchanged preview handler, modelling cancellation too late:
    // a committed source result must remain accepted rather than claim rollback.
    const settlement = await page.evaluate(async input => {
      if (input === 'mouse') window.__waitingPreviewBridge.fail('load_file', 'Operação cancelada. (fixture)');
      else window.__waitingPreviewBridge.release('load_file');
      return window.__waitingLoadWork;
    }, input);
    assert.equal(settlement.ok, input === 'keyboard');
    assert.equal(settlement.disposedAtSettlement, true); assert.equal(settlement.overlayHiddenAtSettlement, true);
    assert.equal(await page.locator('#btn-theme').evaluate(node => node === document.activeElement), true, 'closing the owned overlay returns focus to the still-available origin');
    results.foregroundCancel.push({ input, operationId, pending, settlement,
      terminalTransport: input === 'mouse' ? 'explicit rejection fixture' : 'unchanged preview handler succeeds after cancellation request' });
  }

  phase = 'real load trigger keyboard activation and held source recovery';
  await page.locator('.zone-switch [data-zone="structure"]').click();
  await page.locator('.nav-pages [data-page="import"]').click();
  await page.locator('#ws-windows').click();
  await page.locator('#src-btn-file').click();
  await page.locator('#file-path').fill('C:\\mock\\waiting-trigger-recovery.jsonl');
  await page.evaluate(() => {
    window.__waitingPreviewBridge.held.add('load_file');
    window.__waitingPreviewBridge.held.add('source_snapshot');
    window.__waitingLoadStarted = performance.now();
  });
  // Activate the actual bound UI trigger, so its synchronous disabled/blur
  // behavior is covered rather than starting loadData from a theme-button focus.
  await page.locator('#btn-load').focus(); await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'load_file'));
  assert.equal(await page.evaluate(() => loadReturnFocus === document.querySelector('#btn-load')), true,
    'the logical trigger is saved before the load button is disabled');
  await emitLoadPhase();
  await page.locator('#load-cancel').focus();
  const recoveryCancelBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
  await page.evaluate(() => window.__waitingPreviewBridge.fail('load_file', 'Falha de leitura (fixture de recuperação)'));
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'source_snapshot'));
  results.heldSourceRecovery = await page.evaluate(() => ({
    owner: Tasks.operationFor('source-load'), overlayVisible: !document.querySelector('#load-overlay').hidden,
    cancelDisabled: document.querySelector('#load-cancel').disabled,
    help: document.querySelector('#load-cancel-help').textContent,
    focusOnDetails: document.activeElement === document.querySelector('#load-progress-details > summary'),
    returnFocusId: loadReturnFocus?.id,
  }));
  assert.equal(results.heldSourceRecovery.owner, null); assert.equal(results.heldSourceRecovery.overlayVisible, true);
  assert.equal(results.heldSourceRecovery.cancelDisabled, true); assert.match(results.heldSourceRecovery.help, /indisponível/);
  assert.equal(results.heldSourceRecovery.focusOnDetails, true); assert.equal(results.heldSourceRecovery.returnFocusId, 'btn-load');
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), recoveryCancelBefore);
  await screenshot('waiting-foreground-load-recovery.png', { fixture: true, syntheticTransport: true,
    content: 'Actual keyboard-activated load trigger; settled native task has no active Cancel while source reconciliation is held' });
  await page.evaluate(() => window.__waitingPreviewBridge.release('source_snapshot'));
  await page.waitForFunction(() => !state.loadOverlay && !document.querySelector('#btn-load').disabled);
  assert.equal(await page.evaluate(() => document.activeElement !== document.body && document.activeElement !== document.documentElement
    && !document.querySelector('#load-overlay').contains(document.activeElement) && document.activeElement.getClientRects().length > 0), true,
    'settlement lands on a usable origin or navigation fallback, never non-focusable body');

  phase = 'real grouping pilot, independent pause and prompt disposal';
  await page.locator('.zone-switch [data-zone="analysis"]').click();
  await setTheme('dark');
  await page.getByRole('button', { name: 'Explorar', exact: true }).click();
  await page.getByRole('button', { name: 'Resumir', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#group-table').getAttribute('aria-busy') === 'false' && document.querySelector('#group-table tbody').rows.length > 0);
  await page.evaluate(() => {
    window.__waitingPreviewBridge.held.add('aggregate_events');
    window.__waitingGroupWork = runGroup({ force: true }).then(() => ({
      disposedAtSettlement: !document.querySelector('#tab-group .area-loading-semantic'),
      rows: document.querySelector('#group-table tbody').rows.length,
    }));
  });
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'aggregate_events'));
  await group.waitFor({ state: 'visible' });
  await page.waitForFunction(() => document.querySelector('#tab-group .waiting-visual')?.dataset.pace === 'loop');
  await waitMotion('#tab-group .waiting-visual', 'running');
  assert.equal(await page.locator('.waiting-visual:visible').count(), 1, 'one focused scene, not one per chart');
  results.group = await renderedState(group);
  markVideo('group-command-loop', { syntheticTransport: true, viewport: page.viewportSize() });
  assert.equal(results.group.family, 'calculation'); assert.equal(results.group.status, 'Calculando resumo'); assert.equal(results.group.metric, null);
  assert.ok(results.group.runningAnimations > 0); assert.equal(results.group.buttonPointerEvents, 'auto'); assertTypography(results.group);
  assert.equal(results.group.surface.opacity, '1');
  assert.match(results.group.surface.background, /^rgb\(\d+, \d+, \d+\)$/, 'the local scene surface is opaque so table values do not show through its status');
  results.calculationRig = await inspectTaskRig(group, 'calculation');
  results.calculationCapture = await recordSceneCycle(group, 'calculation-loop', 6400, { syntheticTransport: true, fixtureReceipt: false });
  const groupCancelBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
  await group.locator('.wv-motion-toggle').click();
  assert.equal((await renderedState(group)).motion, 'static');
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), groupCancelBefore);
  assert.equal(await page.evaluate(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'aggregate_events')), true);
  await group.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter');
  await waitMotion('#tab-group .waiting-visual', 'running');
  await assertFits(group);
  await screenshotPose(group, .4, 6400, 'waiting-group-dark-1024.png', { fixture: false, syntheticTransport: true, content: 'Production group wait with actual elapsed time, pending gated preview command, no invented metrics' });
  await setTheme('light');
  await screenshotPose(group, .4, 6400, 'waiting-group-light-1024.png', { fixture: false, syntheticTransport: true, content: 'Production calculation actor inspecting its held card; light theme transitions fully settled' });
  await setTheme('dark');
  results.groupSettlement = await page.evaluate(async () => { window.__waitingPreviewBridge.release('aggregate_events'); return window.__waitingGroupWork; });
  assert.equal(results.groupSettlement.disposedAtSettlement, true); assert.ok(results.groupSettlement.rows > 0);

  phase = 'real cancellation while the group command remains pending';
  await page.evaluate(() => {
    window.__waitingPreviewBridge.held.add('aggregate_events');
    window.__waitingCancelledGroupWork = runGroup({ force: true }).then(() => ({
      disposedAtSettlement: !document.querySelector('#tab-group .area-loading-semantic'),
    }));
  });
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'aggregate_events'));
  await page.waitForFunction(() => document.querySelector('#tab-group .waiting-visual')?.dataset.pace === 'loop');
  await waitMotion('#tab-group .waiting-visual', 'running');
  const cancelledOperation = await page.evaluate(() => {
    const pending = window.__waitingPreviewBridge.pending.find(item => item.command === 'aggregate_events');
    Tasks.cancelOperation(pending.operationId);
    return pending.operationId;
  });
  await page.waitForFunction(() => document.querySelector('#tab-group .waiting-visual')?.dataset.state === 'cancelling');
  results.cancelPendingGroup = await renderedState(group);
  markVideo('group-cancelling-before-settlement', { syntheticTransport: true });
  assert.equal(results.cancelPendingGroup.motion, 'static'); assert.equal(results.cancelPendingGroup.runningAnimations, 0);
  assert.match(results.cancelPendingGroup.status, /Cancelando/);
  assert.equal(await page.evaluate(id => window.__waitingPreviewBridge.pending.some(item => item.operationId === id), cancelledOperation), true,
    'Cancelando is shown before the held transport promise settles');
  assert.equal(await group.isVisible(), true, 'cancellation waits honestly for the owner to settle');
  results.cancelPendingGroupSettlement = await page.evaluate(async () => {
    window.__waitingPreviewBridge.release('aggregate_events'); return window.__waitingCancelledGroupWork;
  });
  assert.equal(results.cancelPendingGroupSettlement.disposedAtSettlement, true);

  phase = 'real pivot pilot and system reduced-motion';
  await page.setViewportSize({ width: 1440, height: 960 }); await setTheme('light');
  await page.getByRole('button', { name: 'Cruzar dados', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#cube-table tbody').rows.length > 0 && !document.querySelector('.cube-output .area-loading-semantic'));
  await page.evaluate(() => {
    window.__waitingPreviewBridge.held.add('pivot');
    window.__waitingPivotWork = runCube({ force: true }).then(() => ({
      disposedAtSettlement: !document.querySelector('.cube-output .area-loading-semantic'),
      rows: document.querySelector('#cube-table tbody').rows.length,
    }));
  });
  await page.waitForFunction(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'pivot'));
  await pivot.waitFor({ state: 'visible' });
  await waitMotion('.cube-output .waiting-visual', 'running');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await waitMotion('.cube-output .waiting-visual', 'static');
  results.pivotReducedMotion = await renderedState(pivot);
  markVideo('pivot-reduced-motion', { syntheticTransport: true, viewport: page.viewportSize() });
  assert.equal(results.pivotReducedMotion.family, 'calculation'); assert.equal(results.pivotReducedMotion.status, 'Cruzando dados');
  assert.equal(results.pivotReducedMotion.metric, null); assert.equal(results.pivotReducedMotion.runningAnimations, 0); assert.equal(results.pivotReducedMotion.buttonHidden, true);
  assert.equal(await pivot.locator('svg').count(), 1, 'reduced motion retains the complete static scene');
  await assertFits(pivot);
  await screenshot('waiting-pivot-reduced-light-1440.png', { fixture: false, syntheticTransport: true, reducedMotion: true, content: 'Production pivot wait, system motion reduction, real CSS/SVG static equivalent' });
  results.pivotSettlement = await page.evaluate(async () => { window.__waitingPreviewBridge.release('pivot'); return window.__waitingPivotWork; });
  assert.equal(results.pivotSettlement.disposedAtSettlement, true); assert.ok(results.pivotSettlement.rows > 0);

  phase = 'explicit component fixture: states, safe text, live region and visibility';
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.evaluate(() => {
    const host = document.createElement('section'); host.id = 'waiting-component-fixture';
    host.setAttribute('aria-label', 'Fixture controlado do componente');
    host.style.cssText = 'position:fixed;top:80px;right:24px;width:290px;z-index:4000;background:var(--bg-1);';
    const caption = document.createElement('div');
    caption.textContent = 'Fixture sintético · componente isolado';
    caption.style.cssText = 'padding:8px;color:var(--text-1);font-size:12px;text-align:center;';
    host.append(caption); document.body.append(host);
    window.__waitingFixtureReceipt = { operationId: 'explicit-component-fixture', phaseId: 'metadata-scan', state: 'running', label: 'Fixture controlado', elapsedMs: 8000, completed: 7, total: 20, unit: 'registros', phaseIndex: 2, phaseCount: 4 };
    window.__waitingFixtureView = WaitingVisuals.mount(host, window.__waitingFixtureReceipt, { showDetails: true });
  });
  const fixture = page.locator('#waiting-component-fixture .waiting-visual');
  await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
  results.componentFixture = { synthetic: true, states: {} };
  assert.match(await fixture.locator('.wv-details').textContent(), /Etapa 2 de 4 · 8 s decorridos/);
  assert.equal(await fixture.locator('.wv-status').getAttribute('role'), 'status');
  assert.equal(await fixture.locator('.wv-status').getAttribute('aria-live'), 'polite');
  assert.equal(await fixture.locator('svg').getAttribute('aria-hidden'), 'true');
  assert.equal(await fixture.locator('svg').getAttribute('focusable'), 'false');
  assert.equal(await fixture.locator('.wv-metric').evaluate(node => !!node.closest('[aria-live]')), false, 'counters are outside the live announcement');
  const updates = await page.evaluate(async () => {
    const root = window.__waitingFixtureView.element, status = root.querySelector('.wv-status'), svg = root.querySelector('svg');
    const variant = root.dataset.variant, mutations = [];
    const observer = new MutationObserver(records => mutations.push(...records)); observer.observe(status, { childList: true, subtree: true, characterData: true });
    for (const completed of [8, 9, 10]) window.__waitingFixtureView.update({ ...window.__waitingFixtureReceipt, completed });
    await Promise.resolve(); observer.disconnect();
    return { sameSvg: root.querySelector('svg') === svg, sameVariant: root.dataset.variant === variant, statusMutations: mutations.length };
  });
  assert.deepEqual(updates, { sameSvg: true, sameVariant: true, statusMutations: 0 });
  for (const state of ['paused', 'cancelling', 'error', 'cancelled', 'completed', 'unknown']) {
    await page.evaluate(state => window.__waitingFixtureView.update({ ...window.__waitingFixtureReceipt, state }), state);
    const current = await renderedState(fixture);
    assert.equal(current.family, 'neutral'); assert.equal(current.motion, 'static'); assert.equal(current.runningAnimations, 0);
    results.componentFixture.states[state] = { family: current.family, motion: current.motion, status: current.status };
  }
  await page.evaluate(() => window.__waitingFixtureView.update({ ...window.__waitingFixtureReceipt, label: '<img src=x onerror=alert(1)>', unit: '<script>fixture</script>', completed: 20, total: 20 }));
  assert.equal(await fixture.locator('img, script, foreignObject').count(), 0);
  assert.equal(await fixture.locator('.wv-status').textContent(), '<img src=x onerror=alert(1)>');
  assert.equal((await renderedState(fixture)).state, 'running', 'equal counters never infer overall completion');
  await page.evaluate(() => window.__waitingFixtureView.update({ ...window.__waitingFixtureReceipt, phaseId: 'future-fixture-phase', completed: undefined, total: undefined }));
  assert.equal((await renderedState(fixture)).family, 'neutral'); assert.equal((await renderedState(fixture)).metric, null);
  await page.evaluate(() => window.__waitingFixtureView.update(window.__waitingFixtureReceipt));
  await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
  await page.evaluate(() => { document.querySelector('#waiting-component-fixture').style.right = 'auto'; document.querySelector('#waiting-component-fixture').style.left = '-10000px'; });
  await waitMotion('#waiting-component-fixture .waiting-visual', 'static');
  assert.equal((await renderedState(fixture)).runningAnimations, 0, 'offscreen scenes have no running CSS animations');
  await page.evaluate(() => { document.querySelector('#waiting-component-fixture').style.left = 'auto'; document.querySelector('#waiting-component-fixture').style.right = '24px'; });
  await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
  await page.evaluate(() => { document.querySelector('#waiting-component-fixture').hidden = true; });
  await waitMotion('#waiting-component-fixture .waiting-visual', 'static');
  await page.evaluate(() => { document.querySelector('#waiting-component-fixture').hidden = false; });
  await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
  await page.evaluate(() => window.__waitingFixtureView.setVisible(false));
  assert.equal(await fixture.isVisible(), false); assert.equal((await renderedState(fixture)).runningAnimations, 0);
  await page.evaluate(() => window.__waitingFixtureView.setVisible(true));
  await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await waitMotion('#waiting-component-fixture .waiting-visual', 'static');
  assert.equal((await renderedState(fixture)).runningAnimations, 0);

  phase = 'explicit robot fixtures: short, paused, reduced-motion and forced-color evidence';
  results.componentFixture.robotFamilies = {};
  for (const [family, phaseId, durationMs] of [
    ['checkpoint', 'metadata-checkpoint-sync', 2800], ['calculation', 'command:aggregate_events', 2600],
  ]) {
    await page.emulateMedia({ reducedMotion: 'no-preference', forcedColors: 'none' });
    await setTheme('dark');
    await page.evaluate(({ family, phaseId }) => {
      window.__waitingRobotReceipt = { operationId: `explicit-${family}-short-fixture`, phaseId, state: 'running',
        label: `Fixture sintético: ${family}`, elapsedMs: 1000 };
      window.__waitingFixtureView.update(window.__waitingRobotReceipt);
      window.__waitingFixtureView.setMotionEnabled(true);
    }, { family, phaseId });
    await waitMotion('#waiting-component-fixture .waiting-visual', 'running');
    const evidence = results.componentFixture.robotFamilies[family] = { synthetic: true };
    evidence.shortRig = await inspectTaskRig(fixture, family, 'gesture');
    evidence.shortCapture = await recordSceneCycle(fixture, `${family}-short-gesture`, durationMs, { fixture: true, syntheticTransport: true });
    assert.deepEqual(evidence.shortCapture.playStates, ['finished'], 'short scene is a single real-duration gesture');
    assert.equal((await renderedState(fixture)).runningAnimations, 0, 'short scene does not loop without a measured long-wait receipt');
    await screenshotPose(fixture, family === 'checkpoint' ? .48 : .78, durationMs, `waiting-fixture-${family}-short-dark-1440.png`, {
      fixture: true, content: 'Explicit standalone short gesture at actual 240x120 CSS size; fixture elapsed time is not native progress' });

    await fixture.locator('.wv-motion-toggle').click();
    evidence.paused = await renderedState(fixture);
    assert.equal(evidence.paused.family, family); assert.equal(evidence.paused.motion, 'static');
    assert.equal(evidence.paused.runningAnimations, 0); assert.equal(evidence.paused.metric, null);
    assert.ok(evidence.paused.controlAnimations.every(animation => animation.type === 'CSSTransition'
      && animation.duration <= 200 && animation.iterations === 1), 'only finite interaction feedback may continue outside the paused scene');
    assert.equal(await fixture.locator('.wv-task').count(), 1, 'pause retains the robot');
    await assertFits(fixture);
    await screenshot(`waiting-fixture-${family}-static-dark-1440.png`, { fixture: true, userPaused: true,
      content: 'User-paused standalone component preserves its complete static robot scene' });

    await setTheme('light');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.evaluate(() => window.__waitingFixtureView.setMotionEnabled(true));
    await waitMotion('#waiting-component-fixture .waiting-visual', 'static');
    evidence.reduced = await renderedState(fixture);
    assert.equal(evidence.reduced.family, family); assert.equal(evidence.reduced.runningAnimations, 0);
    assert.equal(evidence.reduced.buttonHidden, true); assert.equal(await fixture.locator('svg').count(), 1);
    await screenshot(`waiting-fixture-${family}-reduced-light-1440.png`, { fixture: true, reducedMotion: true,
      content: 'Explicit standalone robot fixture under system reduced-motion with no running animation' });

    await page.emulateMedia({ forcedColors: 'active' });
    evidence.forcedColors = await fixture.evaluate(root => {
      const art = root.querySelector('.wv-art'), shell = root.querySelector('.wv-task-shell'), paper = root.querySelector('.wv-task-visor-bed');
      const canvas = document.createElement('span'); canvas.style.cssText = 'color:CanvasText;background-color:Canvas;'; root.append(canvas);
      const result = { active: matchMedia('(forced-colors: active)').matches, color: getComputedStyle(art).color,
        stroke: getComputedStyle(shell).stroke, fill: getComputedStyle(paper).fill,
        canvasText: getComputedStyle(canvas).color, canvas: getComputedStyle(canvas).backgroundColor };
      canvas.remove(); return result;
    });
    assert.equal(evidence.forcedColors.active, true);
    assert.equal(evidence.forcedColors.color, evidence.forcedColors.canvasText);
    assert.equal(evidence.forcedColors.stroke, evidence.forcedColors.canvasText);
    assert.equal(evidence.forcedColors.fill, evidence.forcedColors.canvas);
    assert.notEqual(evidence.forcedColors.canvasText, evidence.forcedColors.canvas, 'forced colors retain visible figure/background contrast');
    await screenshot(`waiting-fixture-${family}-forced-colors-1440.png`, { fixture: true, reducedMotion: true, forcedColors: true,
      content: 'Explicit standalone robot uses system Canvas/CanvasText in forced colors' });
  }
  await page.emulateMedia({ forcedColors: 'none' });
  results.componentFixture.disposal = await page.evaluate(() => {
    const root = window.__waitingFixtureView.element;
    window.__waitingFixtureView.destroy(); window.__waitingFixtureView.destroy();
    const result = { detached: !root.isConnected, animations: root.getAnimations({ subtree: true }).length, ignoredLateUpdate: window.__waitingFixtureView.update(window.__waitingFixtureReceipt) === false };
    document.querySelector('#waiting-component-fixture').remove();
    return result;
  });
  assert.deepEqual(results.componentFixture.disposal, { detached: true, animations: 0, ignoredLateUpdate: true });
  assert.equal(await page.locator('.waiting-visual').count(), 0);
  assert.deepEqual(errors, []);
  results.ok = true;
  results.errors = errors;
  writeFileSync(resolve(output, 'waiting-visuals-results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  originalTestError = error;
  // Capture actual rendered state at the failing boundary, especially a threshold
  // wait: screenshots alone cannot distinguish finished gestures from live loops.
  try {
    results.failureWaitingState = await page.evaluate(() => ({
      documentHidden: document.hidden,
      reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
      pendingTransport: window.__waitingPreviewBridge?.pending.map(({ command, operationId }) => ({ command, operationId })),
      tasks: window.Tasks?.groups().flatMap(group => group.tasks.map(task => ({
        operationId: task.operationId, command: task.cmd, status: task.status, elapsedMs: performance.now() - task.started,
      }))),
      scenes: [...document.querySelectorAll('.waiting-visual')].map(root => ({
        dataset: { ...root.dataset }, text: root.textContent,
        connected: root.isConnected, hiddenAncestor: !!root.closest('[hidden]'),
        bounds: root.getBoundingClientRect().toJSON(),
        animations: root.getAnimations({ subtree: true }).map(animation => ({
          type: animation.constructor.name, target: animation.effect.target.getAttribute('class'),
          belongsToArt: !!animation.effect.target.closest('.wv-art'),
          transitionProperty: animation.transitionProperty || null, animationName: animation.animationName || null,
          playState: animation.playState, currentTime: animation.currentTime, timing: animation.effect.getComputedTiming(),
        })),
      })),
    }));
  } catch (captureError) { results.failureWaitingCaptureError = String(captureError); }
  results.ok = false;
  writeFileSync(resolve(output, 'waiting-visuals-results.json'), JSON.stringify({ ...results, phase, errors, error: String(error.stack || error) }, null, 2));
  await captureFailure(page, 'waiting-visuals', error, { phase, errors, results });
  throw error;
} finally {
  // Closing the context finalizes the recording even when a UI assertion failed.
  // Preserve the original test error if video finalization or metadata writing fails.
  videoMetadata.testPassed = !originalTestError;
  videoMetadata.lastPhase = phase;
  videoMetadata.observedRunMs = Date.now() - videoStartedAt;
  try {
    await context.close();
    if (!video) throw new Error('Playwright did not provide the requested recording');
    const destination = resolve(output, videoMetadata.filename);
    await video.saveAs(destination);
    videoMetadata.bytes = statSync(destination).size;
    if (!videoMetadata.bytes) throw new Error('The finalized recording is empty');
    videoMetadata.finalized = true;
    // Keep one verified copy in the artifact instead of uploading the raw duplicate.
    try { await video.delete(); }
    catch (cleanupError) { videoMetadata.rawCleanupError = String(cleanupError); }
  } catch (videoError) {
    videoMetadata.error = String(videoError.stack || videoError);
    console.error('Waiting visuals video finalization failed:', videoMetadata.error);
    if (!originalTestError) process.exitCode = 1;
  }
  try {
    writeFileSync(resolve(output, 'waiting-visuals-video.json'), JSON.stringify(videoMetadata, null, 2));
  } catch (metadataError) {
    console.error('Waiting visuals video metadata could not be written:', String(metadataError));
    if (!originalTestError) process.exitCode = 1;
  }
  try { await browser.close(); }
  catch (closeError) {
    console.error('Waiting visuals browser cleanup failed:', String(closeError));
    if (!originalTestError) process.exitCode = 1;
  }
}
