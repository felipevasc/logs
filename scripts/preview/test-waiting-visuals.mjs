/* Browser regression for the production waiting pilots.
   Run remotely: node scripts/preview/run-smoke.mjs test-waiting-visuals.mjs
   The app/controllers/CSS are unmodified. Preview transport is gated below so real
   production promises remain pending; native-like metadata receipts and standalone
   component states are explicitly fixtures. This does NOT validate the native engine.
   No image assets are generated: screenshots capture the rendered application. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'no-preference' });
page.setDefaultTimeout(20_000);
const errors = [];
const results = {
  evidence: {
    app: 'Actual production DOM, CSS, WaitingVisuals, WaitingProgress, Tasks, loadData, runGroup and runCube',
    transport: 'Synthetic preview data; a test-only gate holds existing mock commands before their unchanged handlers return',
    loadPhases: 'Built-in preview parse/ready events plus explicitly synthetic metadata/checkpoint/error receipts through the real operation-progress listener',
    calculationPhases: 'Real command-level pending waits with measured elapsed time; unchanged preview calculation handlers produce the final data',
    componentStates: 'Explicit standalone fixture for paused, cancelling, error, unknown, reduced-motion and visibility states',
    nativeEngineVerified: false,
    generatedImageAssets: false,
  },
  screenshots: [],
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
      return new Promise(resolve => this.pending.push({ command, operationId, release: resolve }));
    },
    release(command) {
      this.held.delete(command);
      const ready = this.pending.filter(item => item.command === command);
      this.pending = this.pending.filter(item => item.command !== command);
      for (const item of ready) item.release();
    }
  };`);
  code = code.replace(result, `          await window.__waitingPreviewBridge.beforeResult(cmd, args.operationId);\n${result}`);
  await route.fulfill({ response, body: code });
});

const load = page.locator('#load-visual .waiting-visual');
const group = page.locator('#tab-group .area-loading-semantic .waiting-visual');
const pivot = page.locator('.cube-output .area-loading-semantic .waiting-visual');
const setTheme = theme => page.evaluate(theme => {
  if (document.documentElement.dataset.theme !== theme) toggleTheme();
}, theme);
const waitMotion = (selector, value) => page.waitForFunction(({ selector, value }) => document.querySelector(selector)?.dataset.motion === value, { selector, value });
const renderedState = locator => locator.evaluate(root => {
  const status = root.querySelector('.wv-status'), metric = root.querySelector('.wv-metric'), button = root.querySelector('.wv-motion-toggle');
  const rect = root.getBoundingClientRect();
  return {
    family: root.dataset.family, motion: root.dataset.motion, pace: root.dataset.pace, state: root.dataset.state, variant: root.dataset.variant,
    status: status.textContent, metric: metric.hidden ? null : metric.textContent,
    statusFont: Number.parseFloat(getComputedStyle(status).fontSize), metricFont: Number.parseFloat(getComputedStyle(metric).fontSize),
    buttonFont: Number.parseFloat(getComputedStyle(button).fontSize), buttonMinHeight: Number.parseFloat(getComputedStyle(button).minHeight),
    buttonPointerEvents: getComputedStyle(button).pointerEvents, buttonHidden: button.hidden,
    runningAnimations: root.getAnimations({ subtree: true }).filter(animation => animation.playState === 'running').length,
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
  await page.screenshot({ path: resolve(output, filename), animations: 'allow' });
  results.screenshots.push({ filename, ...attribution, viewport: page.viewportSize(), theme: await page.evaluate(() => document.documentElement.dataset.theme) });
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

  phase = 'fixture receipts through real foreground progress adapter';
  await page.waitForFunction(() => performance.now() - window.__waitingLoadStarted >= 4100);
  await emitLoadPhase();
  await waitMotion('#load-visual .waiting-visual', 'running');
  results.reading = await renderedState(load);
  assert.equal(results.reading.family, 'reading'); assert.equal(results.reading.pace, 'loop');
  assert.equal(results.reading.status, 'Indexando registros'); assert.equal(results.reading.metric, '1.200 / 6.300 registros');
  assert.ok(results.reading.runningAnimations > 0); assertTypography(results.reading);
  assert.equal(await page.locator('#load-progress-details').evaluate(node => node.open), false);
  await assertFits(load);
  await screenshot('waiting-fixture-reading-dark-1440.png', { fixture: true, content: 'Synthetic metadata receipt rendered by the production foreground adapter while its preview load command is pending' });

  const beforeForeign = await load.textContent();
  await page.evaluate(() => window.__waitingPreviewBridge.emit('operation-progress', {
    operationId: 'fixture-unowned-background', phaseId: 'analytics-sql', phase: 'Must not replace foreground', completed: 999, total: 999, unit: 'itens', fixture: true,
  }));
  assert.equal(await load.textContent(), beforeForeign, 'foreign operation cannot replace the foreground scene');
  const cancelBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
  await load.locator('.wv-motion-toggle').click();
  assert.equal((await renderedState(load)).motion, 'static');
  await emitLoadPhase({ completed: 1800 });
  assert.equal((await renderedState(load)).motion, 'static', 'new receipts preserve the user pause');
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), cancelBefore);
  assert.equal(await page.evaluate(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'load_file')), true);
  await load.locator('.wv-motion-toggle').click(); await waitMotion('#load-visual .waiting-visual', 'running');

  await emitLoadPhase({ total: 0, completed: 1800 });
  assert.equal((await renderedState(load)).metric, '1.800 registros');
  assert.equal(await page.locator('#load-bar-fill').evaluate(node => node.parentElement.hidden), true);
  await emitLoadPhase({ phaseId: 'metadata-checkpoint-sync', phase: 'Sincronizando checkpoint (fixture)', completed: 0, total: 0, unit: '' });
  assert.equal((await renderedState(load)).family, 'checkpoint'); assert.equal((await renderedState(load)).metric, null);
  await emitLoadPhase({ phaseId: 'metadata-checkpoint-committed', phase: 'Checkpoint de metadados preservado (fixture)', completed: 6300, total: 6300, unit: 'bytes', checkpointRows: 1200 });
  results.partialCheckpoint = await renderedState(load);
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
  results.loadSettlement = await page.evaluate(async () => { window.__waitingPreviewBridge.release('load_file'); return window.__waitingLoadWork; });
  assert.equal(results.loadSettlement.ok, true); assert.equal(results.loadSettlement.disposedAtSettlement, true); assert.equal(results.loadSettlement.overlayHiddenAtSettlement, true);
  assert.equal(await page.locator('#load-visual .waiting-visual').count(), 0, 'no cosmetic delay after owner settlement');

  phase = 'real grouping pilot, independent pause and prompt disposal';
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
  assert.equal(results.group.family, 'calculation'); assert.equal(results.group.status, 'Calculando resumo'); assert.equal(results.group.metric, null);
  assert.ok(results.group.runningAnimations > 0); assert.equal(results.group.buttonPointerEvents, 'auto'); assertTypography(results.group);
  const groupCancelBefore = await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0);
  await group.locator('.wv-motion-toggle').click();
  assert.equal((await renderedState(group)).motion, 'static');
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.cancel_task || 0), groupCancelBefore);
  assert.equal(await page.evaluate(() => window.__waitingPreviewBridge.pending.some(item => item.command === 'aggregate_events')), true);
  await group.locator('.wv-motion-toggle').focus(); await page.keyboard.press('Enter');
  await waitMotion('#tab-group .waiting-visual', 'running');
  await assertFits(group);
  await screenshot('waiting-group-dark-1024.png', { fixture: false, syntheticTransport: true, content: 'Production group wait with actual elapsed time, pending gated preview command, no invented metrics' });
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
    document.body.append(host);
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
  await browser.close();
}
