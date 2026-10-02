/* Columns interaction checkpoint: real browser input and DOM with synthetic
   preview transport. This does not validate Rust or the installed WebView. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Synthetic preview transport with real keyboard, drag and focus; not installed WebView verification' };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
const trigger = page.locator('#btn-colpicker'), pop = page.locator('#col-pop');
const checkbox = column => page.locator(`#col-list input[data-column="${column}"]`);
const assertFocus = async locator => assert.equal(await locator.evaluate(node => node === document.activeElement), true);
const columns = () => page.evaluate(() => [...state.visibleCols]);
const queryCounts = () => page.evaluate(() => Object.fromEntries(Object.entries(window.__mockCommandCalls || {}).filter(([command]) => /^(query|count|stats|tree_aggs|aggregate|cases_save)/.test(command))));
async function settled() {
  await page.waitForFunction(() => Number.isFinite(state.total) && explorerAnalytics.get(explorerKey())?.status === 'done'
    && document.querySelector('#events-table').getAttribute('aria-busy') === 'false');
  await page.evaluate(() => settleFilterTabCounts()); await page.waitForFunction(() => Tasks.pending() === 0);
}
async function screenshot(name) {
  await page.evaluate(() => {
    const label = document.createElement('div'); label.id = 'columns-fixture-label';
    label.textContent = 'Colunas · Preview sintético · Teclado, arraste e foco · Sem validação WebView';
    label.style.cssText = 'position:fixed;bottom:8px;left:12px;z-index:99999;padding:6px 10px;background:#142139;color:#fff;border:1px solid #9ac7ff;font:12px system-ui;pointer-events:none';
    document.body.append(label);
  });
  try { await page.screenshot({ path: resolve(output, name) }); }
  finally { await page.locator('#columns-fixture-label').evaluate(node => node.remove()); }
}
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden);
  await page.getByRole('button', { name: 'Explorar', exact: true }).click(); await settled();
  const fields = await page.evaluate(() => {
    const available = ['message', 'level', 'code'].filter(column => state.columns.includes(column));
    if (!state.columns.includes('timestamp') || available.length !== 3) throw Error('Columns preview requires canonical timestamp, message, level and code fields');
    state.visibleCols = ['timestamp', ...available.slice(0, 2)]; state.colWidths = { ...state.colWidths, message: 300, level: 130 };
    saveVisibleCols(); renderTable({ total: state.total, rows: state.rows }, { reuseRows: true });
    state.selectedEventRows = new Map([[state.rows[0].id, state.rows[0]]]); updateRowSelectionStyles();
    window.__columnsRowNodes = [...document.querySelectorAll('#events-table tbody tr')];
    window.__columnsActionNodes = window.__columnsRowNodes.map(row => row.querySelector('.record-actions-trigger'));
    window.__columnsSelected = [...state.selectedEventRows.keys()];
    window.__columnsWidths = JSON.stringify(state.colWidths); window.__columnsPage = state.page;
    return available;
  });
  const [first, second, added] = fields, before = await queryCounts();

  phase = 'native header drag then checkbox add/remove';
  await page.locator(`#events-table th[data-column="${second}"]`).dragTo(page.locator(`#events-table th[data-column="${first}"]`));
  const dragged = ['timestamp', second, first]; assert.deepEqual(await columns(), dragged);
  await trigger.focus(); await page.keyboard.press('Enter'); await pop.waitFor({ state: 'visible' });
  assert.equal(await trigger.getAttribute('aria-expanded'), 'true'); await assertFocus(page.locator('#col-list input:not(:disabled)').first());
  await checkbox(added).focus(); await page.keyboard.press('Space'); assert.deepEqual(await columns(), [...dragged, added]); await assertFocus(checkbox(added));
  await checkbox(first).focus(); await page.keyboard.press('Space'); assert.deepEqual(await columns(), ['timestamp', second, added]); await assertFocus(checkbox(first));
  const integrity = await page.evaluate(() => ({
    rows: [...document.querySelectorAll('#events-table tbody tr')].every((row, index) => row === window.__columnsRowNodes[index]),
    actions: [...document.querySelectorAll('#events-table .record-actions-trigger')].every((button, index) => button === window.__columnsActionNodes[index]),
    selected: [...state.selectedEventRows.keys()], selectedStyle: document.querySelector('#events-table tbody tr').classList.contains('row-multi-selected'),
    widths: JSON.stringify(state.colWidths) === window.__columnsWidths, page: state.page === window.__columnsPage,
  }));
  assert.equal(integrity.rows, true); assert.equal(integrity.actions, true); assert.equal(integrity.widths, true); assert.equal(integrity.page, true); assert.equal(integrity.selectedStyle, true);
  assert.deepEqual(integrity.selected, await page.evaluate(() => window.__columnsSelected)); assert.deepEqual(await queryCounts(), before);
  await page.keyboard.press('Escape'); assert.equal(await pop.isVisible(), false); await assertFocus(trigger);
  const saved = await page.evaluate(() => {
    const key = visiblePreferenceKey(), value = JSON.parse(localStorage.getItem(key));
    state.visibleCols = ['timestamp']; state.colWidths = {}; restoreVisiblePreferences();
    renderTable({ total: state.total, rows: state.rows }, { reuseRows: true });
    return { saved: value.columns, restored: [...state.visibleCols], widthsRestored: JSON.stringify(state.colWidths) === window.__columnsWidths };
  });
  assert.deepEqual(saved.saved, ['timestamp', second, added]); assert.deepEqual(saved.restored, saved.saved); assert.equal(saved.widthsRestored, true); assert.deepEqual(await queryCounts(), before);
  results.visibility = { dragged, integrity, saved, queriesUnchanged: true };

  phase = 'real detail menu add/remove retains existing order';
  await page.locator('#events-table .record-actions-trigger').first().click();
  await page.getByRole('menuitem', { name: 'Ver detalhes', exact: true }).click();
  const detailName = page.locator(`#drawer .detail-tree-name[title="${first}"]`); await detailName.waitFor({ state: 'visible' });
  const detailBefore = await queryCounts();
  await detailName.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Adicionar coluna à tabela', exact: true }).click();
  assert.deepEqual(await columns(), ['timestamp', second, added, first]);
  await detailName.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Remover coluna da tabela', exact: true }).click();
  assert.deepEqual(await columns(), ['timestamp', second, added]); assert.deepEqual(await queryCounts(), detailBefore);
  await page.locator('#dr-close').click(); results.detail = { sharedOrdering: true, queriesUnchanged: true };

  phase = 'Escape closes only Columns and outside click retains target focus';
  // Deliberately retain another visible surface to exercise the global Escape
  // branch; normal pointer navigation may dismiss it before Columns opens.
  await page.evaluate(() => { document.querySelector('#drawer').hidden = false; document.querySelector('#btn-colpicker').focus(); openColPop(); });
  await page.keyboard.press('Escape'); assert.equal(await pop.isVisible(), false); await assertFocus(trigger);
  assert.equal(await page.locator('#drawer').isVisible(), true, 'Columns Escape must not close the underlying drawer');
  await page.evaluate(() => closeDrawer());
  for (const key of ['Enter', 'Space']) {
    await trigger.focus(); await page.keyboard.press(key); await assertFocus(page.locator('#col-list input:not(:disabled)').first());
    await page.locator('#quick-search').click(); assert.equal(await pop.isVisible(), false); await assertFocus(page.locator('#quick-search'));
    assert.equal(await trigger.getAttribute('aria-expanded'), 'false');
  }
  await trigger.click(); await trigger.click(); assert.equal(await pop.isVisible(), false); await assertFocus(trigger);
  results.focus = { firstEnabledCheckbox: true, escapeReturnsToTrigger: true, oneLayer: true, outsideClickPreserved: true, repeatedOpenClose: true };

  phase = 'compact visual comparison with unchanged text and targets';
  results.visuals = [];
  for (const width of [1440, 1024]) for (const theme of ['dark', 'light']) {
    await page.setViewportSize({ width, height: 960 }); await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
    await trigger.focus(); await page.keyboard.press('Enter');
    const geometry = await pop.evaluate(panel => {
      const rect = panel.getBoundingClientRect(), item = panel.querySelector('.col-item'), style = getComputedStyle(item);
      return { width: rect.width, height: rect.height, inViewport: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight,
        fontSize: parseFloat(style.fontSize), rowHeight: item.getBoundingClientRect().height, nativeCheckboxes: [...panel.querySelectorAll('input')].every(input => input.type === 'checkbox') };
    });
    assert.equal(geometry.inViewport, true); assert.equal(geometry.nativeCheckboxes, true); assert.ok(geometry.fontSize >= 12.5); assert.ok(geometry.rowHeight >= 24);
    await screenshot(`columns-${theme}-${width}.png`); results.visuals.push({ width, theme, ...geometry });
    await page.keyboard.press('Escape'); await assertFocus(trigger);
  }
  assert.deepEqual(errors, []); results.ok = true;
  writeFileSync(resolve(output, 'columns.json'), JSON.stringify(results, null, 2)); console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, 'columns', error, { phase, errors, results }); throw error;
} finally { await browser.close(); }
