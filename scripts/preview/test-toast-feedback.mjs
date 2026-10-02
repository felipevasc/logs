/* UX03: real copy/filter UI with synthetic preview transport and clipboard.
   Component stress fixtures are identified separately from natural placement. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Browser UI with synthetic preview transport/clipboard; not native WebView or screen-reader verification', placements: [] };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
await page.addInitScript(() => {
  window.__toastClipboardWrites = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
    writeText: async text => { window.__toastClipboardWrites.push(text); },
  } });
});
const clearFeedback = () => page.evaluate(() => { for (const node of [...toastFeedback.keys()]) removeToastFeedback(node); });
const dataCalls = () => page.evaluate(() => JSON.stringify(Object.entries(window.__mockCommandCalls)
  .filter(([cmd]) => ['query_page', 'count_filtered', 'stats_events', 'tree_aggs', 'cases_save'].includes(cmd))));
const geometry = () => page.evaluate(() => {
  const toastBounds = document.querySelector('#toast-feedback-area').getBoundingClientRect();
  const overlaps = node => {
    const rect = node.getBoundingClientRect();
    return toastBounds.left < rect.right && toastBounds.right > rect.left && toastBounds.top < rect.bottom && toastBounds.bottom > rect.top;
  };
  return { width: innerWidth, focus: document.activeElement.id, focusedCovered: overlaps(document.activeElement),
    cancelCovered: overlaps(document.querySelector('#fp-cancel')), applyCovered: overlaps(document.querySelector('#fp-apply')),
    toast: { x: toastBounds.x, y: toastBounds.y, width: toastBounds.width, height: toastBounds.height },
    count: document.querySelectorAll('.toast-passive').length };
});

try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
  await page.getByRole('button', { name: 'Explorar', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#events-table').getAttribute('aria-busy') === 'false' && state.rows.length && Tasks.pending() === 0);
  await clearFeedback();

  phase = 'repeated real JSON copy';
  await page.locator('#events-table tbody tr[data-event-id] td[data-column]').first().click();
  await page.waitForFunction(() => state.currentDetailEv && !document.querySelector('#drawer').hidden);
  const dataBefore = await dataCalls();
  const copiedEvent = await page.evaluate(() => JSON.stringify(state.currentDetailEv, null, 2));
  for (let i = 0; i < 5; i++) await page.locator('#dr-copy').click();
  await page.waitForFunction(() => document.querySelector('.toast-repeat-count')?.textContent === '×5');
  assert.equal(await page.locator('.toast-passive').count(), 1);
  assert.equal(await page.locator('.toast-message').innerText(), 'JSON copiado.');
  assert.equal(await page.locator('.toast-passive').getAttribute('role'), 'status');
  assert.equal(await page.locator('.toast-repeat-count').getAttribute('aria-hidden'), 'true');
  assert.equal(await page.locator('#dr-copy').evaluate(node => node === document.activeElement), true);
  assert.deepEqual(await page.evaluate(() => window.__toastClipboardWrites), Array(5).fill(copiedEvent));
  assert.equal(await dataCalls(), dataBefore, 'feedback and copies add no queries or saves');
  results.repeatedCopy = { writes: 5, notices: 1, literalPreserved: true, focusPreserved: true };
  await page.screenshot({ path: resolve(output, 'toast-copy-1440.png') });
  await page.locator('#dr-close').click(); await clearFeedback();

  phase = 'filter editor, natural and collision placement';
  const draftBefore = await page.evaluate(() => JSON.stringify(state.filters));
  for (const [width, height] of [[1440, 900], [1024, 768]]) {
    await page.setViewportSize({ width, height });
    await page.locator('#btn-add-filter').click();
    await page.locator('#fp-val').fill('rascunho literal preservado');
    const callsBefore = await dataCalls();
    await page.evaluate(() => { for (let i = 0; i < 5; i++) toast('Filtro atualizado.', 'ok'); });
    await page.waitForFunction(() => document.querySelector('.toast-message')?.textContent === 'Filtro atualizado.');
    let placement = await geometry();
    assert.equal(placement.count, 1); assert.equal(placement.focus, 'fp-val');
    assert.equal(placement.focusedCovered || placement.cancelCovered || placement.applyCovered, false);
    results.placements.push({ fixture: 'natural editor', ...placement });
    await page.screenshot({ path: resolve(output, `toast-filter-${width}.png`) });

    // Deliberately move the existing editor into the previous toast corner.
    // This proves collision handling rather than assuming every normal editor overlaps.
    await page.evaluate(() => {
      const pop = document.querySelector('#filter-pop'), rect = pop.getBoundingClientRect();
      pop.style.left = `${innerWidth - rect.width - 18}px`; pop.style.top = `${innerHeight - rect.height - 18}px`;
    });
    await page.locator('#fp-apply').focus();
    await page.evaluate(() => toast('Filtro atualizado.', 'ok'));
    await page.waitForFunction(() => document.querySelector('.toast-message')?.textContent === 'Filtro atualizado.');
    placement = await geometry();
    assert.equal(placement.count, 1); assert.equal(placement.focus, 'fp-apply');
    assert.equal(placement.focusedCovered || placement.cancelCovered || placement.applyCovered, false);
    assert.ok(placement.toast.x >= 0 && placement.toast.x + placement.toast.width <= width);
    results.placements.push({ fixture: 'editor moved to corner for collision stress', ...placement });
    assert.equal(await page.locator('#fp-val').inputValue(), 'rascunho literal preservado');
    assert.equal(await dataCalls(), callsBefore);
    await page.screenshot({ path: resolve(output, `toast-filter-collision-${width}.png`) });
    await page.locator('#fp-cancel').click();
    assert.equal(await page.evaluate(() => JSON.stringify(state.filters)), draftBefore);
    await clearFeedback();
  }

  phase = 'distinct errors retain pointer selection and copy';
  await page.evaluate(() => {
    toast('Não foi possível copiar: erro A.', 'err');
    toast('Não foi possível copiar: erro B.', 'err');
    toast('Não foi possível copiar: erro A.', 'err');
  });
  await page.waitForFunction(() => document.querySelectorAll('.toast.err .toast-message')[2]?.textContent.endsWith('erro A.'));
  assert.equal(await page.getByRole('alert').count(), 3, 'even identical errors keep distinct notices');
  const errorMessage = page.locator('.toast.err .toast-message').first();
  const errorInteraction = await errorMessage.evaluate(node => {
    const rect = node.getBoundingClientRect(), notice = node.parentElement;
    return { pointerEvents: getComputedStyle(notice).pointerEvents, userSelect: getComputedStyle(notice).userSelect,
      hit: notice.contains(document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2)) };
  });
  assert.deepEqual(errorInteraction, { pointerEvents: 'auto', userSelect: 'text', hit: true });
  await errorMessage.click({ clickCount: 3 });
  assert.match(await page.evaluate(() => String(getSelection())), /Não foi possível copiar: erro A\./);
  await page.evaluate(() => document.addEventListener('copy', event => {
    window.__toastSelectedCopy = String(getSelection()); event.preventDefault();
  }, { once: true }));
  await page.keyboard.press('ControlOrMeta+C');
  assert.match(await page.evaluate(() => window.__toastSelectedCopy), /Não foi possível copiar: erro A\./);
  await page.evaluate(() => getSelection().removeAllRanges());
  await clearFeedback();
  results.errorSelection = { distinctErrors: 3, selectable: true, keyboardCopy: true };

  phase = 'late Undo append, stable rectangle under stationary pointer and expiry';
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.evaluate(() => toast('Valor copiado.', 'ok'));
  await page.waitForFunction(() => document.querySelector('.toast-message')?.textContent === 'Valor copiado.');
  const managedBeforeUndo = await page.locator('#toast-feedback-area').boundingBox();
  await page.evaluate(() => {
    // The same real DOM append pattern and 15 s lifetime as legacy removal.
    // Crucially, append happens after passive feedback has already been positioned.
    const notice = el('div', 'toast ok', 'Recibo de teste: '), undo = el('button', 'btn ghost small', 'Desfazer');
    notice.id = 'toast-undo-fixture'; undo.id = 'toast-undo-action'; undo.type = 'button';
    window.__toastUndoCount = 0; undo.onclick = () => { window.__toastUndoCount++; };
    notice.appendChild(undo); document.querySelector('#toast-area').appendChild(notice);
    setTimeout(() => notice.remove(), 15000);
  });
  await page.locator('#toast-undo-fixture').evaluate(node => Promise.all(node.getAnimations().map(animation => animation.finished)));
  await page.waitForFunction(() => {
    const a = document.querySelector('#toast-feedback-area').getBoundingClientRect(), b = document.querySelector('#toast-area').getBoundingClientRect();
    return a.right <= b.left || a.left >= b.right || a.bottom <= b.top || a.top >= b.bottom;
  });
  const undoButton = page.locator('#toast-undo-action'), undoBefore = await undoButton.boundingBox();
  assert.ok(undoBefore.y >= 0 && undoBefore.y + undoBefore.height <= 768, 'late Undo stays inside the viewport');
  assert.equal(await page.locator('#toast-area').evaluate(node => node.style.top), '', 'managed placement never writes the Undo lane');
  assert.equal(await page.locator('#toast-area').evaluate(node => node.style.left), '');
  const pointer = { x: undoBefore.x + undoBefore.width / 2, y: undoBefore.y + undoBefore.height / 2 };
  await page.mouse.move(pointer.x, pointer.y);
  assert.equal(await undoButton.evaluate(node => node === document.activeElement), false, 'regression checks pointer stability without relying on focus');
  await page.evaluate(() => { toast('JSON copiado.', 'ok'); toast('Falha distinta durante o teste.', 'err'); });
  await page.waitForFunction(() => [...document.querySelectorAll('.toast-message')].some(node => node.textContent === 'Falha distinta durante o teste.'));
  assert.deepEqual(await undoButton.boundingBox(), undoBefore, 'managed growth cannot move Undo under the pointer');
  await page.waitForFunction(() => !document.querySelector('#toast-feedback-area'), null, { timeout: 7000 });
  assert.deepEqual(await undoButton.boundingBox(), undoBefore, 'expiry/removal of managed siblings cannot shift the actual button rectangle');
  assert.equal(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('button')?.id, pointer), 'toast-undo-action');
  await page.mouse.click(pointer.x, pointer.y);
  assert.equal(await page.evaluate(() => window.__toastUndoCount), 1, 'stationary-pointer click still activates the same Undo');
  assert.equal(await page.evaluate(() => toastFeedback.size), 0);

  phase = 'focused Undo stays fixed through managed expiry';
  await page.evaluate(() => toast('Valor copiado.', 'ok'));
  await undoButton.focus();
  await page.waitForFunction(() => document.querySelector('.toast-message')?.textContent === 'Valor copiado.');
  assert.deepEqual(await undoButton.boundingBox(), undoBefore);
  await page.waitForFunction(() => !document.querySelector('#toast-feedback-area'), null, { timeout: 7000 });
  assert.deepEqual(await undoButton.boundingBox(), undoBefore);
  assert.equal(await undoButton.evaluate(node => node === document.activeElement), true);
  await undoButton.press('Enter'); assert.equal(await page.evaluate(() => window.__toastUndoCount), 2);
  assert.equal(await undoButton.evaluate(node => getComputedStyle(node).pointerEvents), 'auto');
  await page.evaluate(() => {
    const notice = el('div', 'toast ok', 'Segundo recibo de teste: '), undo = el('button', 'btn ghost small', 'Desfazer');
    notice.id = 'toast-second-undo-fixture'; notice.appendChild(undo); document.querySelector('#toast-area').appendChild(notice);
  });
  const grownLegacyLane = await page.locator('#toast-area').boundingBox();
  assert.ok(grownLegacyLane.y >= 0 && grownLegacyLane.y + grownLegacyLane.height <= 768, 'legacy lane still grows upward from bottom');
  assert.equal(await page.locator('#toast-undo-fixture').count(), 1, 'only the independent Undo lifetime owns removal');
  results.undoIsolation = { managedBeforeUndo, undoBefore, grownLegacyLane, pointerStable: true, focusStable: true, activations: 2 };
  await page.locator('#toast-undo-fixture').evaluate(node => node.remove());
  await page.locator('#toast-second-undo-fixture').evaluate(node => node.remove());

  phase = 'reflow and reduced motion';
  await page.setViewportSize({ width: 320, height: 600 });
  await page.evaluate(() => toast('Confirmação com texto longo: ' + 'token-sem-espaço-'.repeat(12), 'info'));
  await page.waitForFunction(() => document.querySelector('.toast-message')?.textContent.startsWith('Confirmação'));
  const reflow = await page.locator('.toast-feedback').evaluate(node => {
    const rect = node.getBoundingClientRect(), style = getComputedStyle(node);
    return { left: rect.left, right: rect.right, fontSize: style.fontSize, animation: style.animationName, pointerEvents: style.pointerEvents };
  });
  assert.ok(reflow.left >= 0 && reflow.right <= 320); assert.equal(reflow.fontSize, '12.5px');
  assert.equal(reflow.animation, 'none'); assert.equal(reflow.pointerEvents, 'auto', 'non-aggregable info remains selectable');
  results.reflow = reflow;
  await clearFeedback();
  assert.deepEqual(errors, []); results.ok = true;
  writeFileSync(resolve(output, 'toast-feedback.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, 'toast-feedback', error, { phase, errors, results });
  throw error;
} finally { await browser.close(); }
