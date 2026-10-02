/* UX02 restricted checkpoint: real browser keyboard/focus with preview transport.
   No virtual list, grid model, native engine or installed WebView claim. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright'); mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Synthetic preview transport and real keyboard/focus; not native engine or installed WebView verification' };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
const triggers = page.locator('#events-table .record-actions-trigger');
const menu = page.locator('.ctx-menu');
const assertFocus = async locator => assert.equal(await locator.evaluate(node => node === document.activeElement), true);
const rowButton = id => page.locator(`#events-table tr[data-event-id="${id}"] .record-actions-trigger`);
const rowCell = id => page.locator(`#events-table tr[data-event-id="${id}"] td[data-column]`).first();
async function openOrdinaryDetail(entry, id) {
  await rowCell(id).click(entry === 'cell-menu' ? { button: 'right' } : {});
  if (entry === 'cell-menu') await menu.getByRole('menuitem', { name: 'Ver detalhes', exact: true }).click();
}
async function screenshot(name) {
  await page.evaluate(() => {
    const label = document.createElement('div'); label.id = 'record-fixture-label';
    label.textContent = 'UX02 · Preview sintético · Tabela nativa + ações por registro · Sem validação WebView';
    label.style.cssText = 'position:fixed;bottom:8px;left:12px;z-index:99999;padding:6px 10px;background:#142139;color:#fff;border:1px solid #9ac7ff;font:12px system-ui;pointer-events:none';
    document.body.append(label);
  });
  try { await page.screenshot({ path: resolve(output, name) }); }
  finally { await page.locator('#record-fixture-label').evaluate(node => node.remove()); }
}
try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden);
  await page.getByRole('button', { name: 'Explorar', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#events-table').getAttribute('aria-busy') === 'false' && state.rows.length);
  await page.locator('#page-size').selectOption('500');
  await page.waitForFunction(() => state.rows.length === 500 && document.querySelector('#events-table').getAttribute('aria-busy') === 'false');
  assert.equal(await triggers.count(), 500);
  assert.equal(await page.locator('#events-table [role="grid"],#events-table tr[tabindex],#events-table td[tabindex]').count(), 0);
  assert.equal(await page.locator('#skip-records').count(), 1);
  const baseline = await page.evaluate(() => ({ columns: [...state.visibleCols], widths: { ...state.colWidths }, ids: state.rows.map(row => row.id), values: JSON.stringify(state.rows) }));
  const id = baseline.ids[3];

  phase = '500 keyboard stops and skip route';
  await triggers.first().focus();
  for (let index = 1; index < 500; index++) await page.keyboard.press('Tab');
  await assertFocus(triggers.last());
  await page.keyboard.press('Enter');
  await menu.waitFor({ state: 'visible' }); await assertFocus(menu.getByRole('menuitem').first());
  assert.equal(await menu.getByRole('menuitem').first().textContent(), 'Ver detalhes');
  await page.keyboard.press('Escape'); await assertFocus(triggers.last());
  await page.locator('#skip-records').focus(); await page.keyboard.press('Enter'); await assertFocus(page.locator('#page-size'));
  results.rowsAndSkip = { nativeRows: 500, sequentialTabStops: 500, skipTarget: 'page-size' };

  phase = 'record menu, selection and layers';
  await rowButton(id).focus(); await page.keyboard.press('Control+a');
  assert.equal(await page.evaluate(() => state.selectedEventRows.size), 500);
  await page.keyboard.press('Space'); await menu.waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => state.selectedEventRows.size), 500, 'opening actions does not replace Ctrl+A selection');
  assert.equal(await menu.getByRole('menuitem', { name: 'Copiar valor', exact: true }).count(), 0, 'record menu cannot invent a cell value');
  assert.equal(await menu.getByRole('menuitem', { name: /^Criar filtro:/ }).count(), 0);
  await page.keyboard.press('End'); await assertFocus(menu.getByRole('menuitem').last());
  await page.keyboard.press('Home'); await assertFocus(menu.getByRole('menuitem').first());
  await page.keyboard.press('Escape'); await assertFocus(rowButton(id));
  await page.keyboard.press('Shift+F10'); await menu.waitFor({ state: 'visible' });
  await page.keyboard.press('Escape'); await assertFocus(rowButton(id));

  phase = 'logical menu return after rerender';
  const nextId = baseline.ids[4];
  for (const key of ['Escape', 'Tab']) {
    await rowButton(id).focus(); await page.keyboard.press('Enter');
    await page.evaluate(target => {
      const old = document.querySelector(`#events-table tr[data-event-id="${target}"] .record-actions-trigger`);
      state.rows = state.rows.map(row => ({ ...row })); renderTable({ total: state.total, rows: state.rows });
      if (old.isConnected || !document.activeElement.closest('.ctx-menu')) throw Error('Rerender must detach the caller while preserving menu focus');
    }, id);
    await page.keyboard.press(key);
    await assertFocus(rowButton(key === 'Tab' ? nextId : id));
    assert.equal(await menu.count(), 0);
  }
  await rowButton(id).focus(); await page.keyboard.press('Enter');
  await page.evaluate(() => {
    showCtxMenu(20, 40, [{ label: 'Página de ações para regressão', onClick() {} }]);
    state.rows = state.rows.map(row => ({ ...row })); renderTable({ total: state.total, rows: state.rows });
  });
  await page.keyboard.press('Escape'); await assertFocus(rowButton(id));
  for (const change of ['removed', 'new-ref', 'source-connected']) {
    await rowButton(id).focus(); await page.keyboard.press('Enter');
    await page.evaluate(({ id, change }) => {
      window.__recordMenuRows = state.rows; window.__recordMenuLoadedAt = state.currentArtifact.loadedAt;
      if (change === 'source-connected') state.currentArtifact.loadedAt = `${state.currentArtifact.loadedAt}-menu-owner`;
      else {
        state.rows = change === 'removed' ? state.rows.filter(row => row.id !== id)
          : state.rows.map(row => row.id === id ? { ...row, event_ref: `replacement:${id}` } : row);
        renderTable({ total: state.total, rows: state.rows });
      }
    }, { id, change });
    await page.keyboard.press('Escape'); await assertFocus(page.locator('#page-size'));
    await page.evaluate(() => { state.rows = window.__recordMenuRows; state.currentArtifact.loadedAt = window.__recordMenuLoadedAt;
      renderTable({ total: state.total, rows: state.rows }); });
  }
  const more = page.locator('#explore-tree .field-adv-btn:visible').first();
  const fieldColumn = await more.evaluate(node => node.closest('.field-row').dataset.column);
  await more.focus(); await page.keyboard.press('Enter'); await menu.waitFor({ state: 'visible' });
  await page.evaluate(() => { const tree = document.querySelector('#explore-tree'); renderExploreTreeInto(tree, tree.dataset.treeScope || 'dataset'); });
  await page.keyboard.press('Escape');
  assert.equal(await page.evaluate(column => document.activeElement.matches('.field-adv-btn')
    && document.activeElement.closest('#explore-tree .field-row')?.dataset.column === column, fieldColumn), true);
  results.logicalMenus = { replacementEscape: true, replacementTabResumesAfterCaller: true, paging: true,
    removedAndReusedIds: true, connectedStaleSource: true, fieldEllipsisReturn: true };

  phase = 'loading drawer, exact reference and return after reorder';
  await page.evaluate(() => {
    window.__recordBaseApi = api; window.__recordDetailRequests = [];
    api = async (command, args, options) => {
      if (command !== 'event_detail') return window.__recordBaseApi(command, args, options);
      window.__recordDetailRequests.push(structuredClone(args));
      const value = await window.__recordBaseApi(command, args, options);
      await new Promise(resolve => { window.__recordDetailRelease = resolve; });
      return value;
    };
  });
  await rowButton(id).press('Enter'); await page.keyboard.press('Enter');
  await page.waitForFunction(() => typeof window.__recordDetailRelease === 'function');
  await assertFocus(page.locator('#dr-close'));
  assert.equal(await page.locator('#drawer').isVisible(), true);
  assert.ok(await page.evaluate(() => window.__recordDetailRequests.at(-1).eventRef), 'detail request carries native reference');
  await page.evaluate(() => {
    state.rows = [...state.rows].reverse().map(row => ({ ...row }));
    renderTable({ total: state.total, rows: state.rows });
    window.__recordDetailRelease();
  });
  await page.waitForFunction(expected => state.currentDetailEv?.id === expected, id);
  await assertFocus(page.locator('#dr-close'));
  const detailValue = page.locator('#pane-overview .detail-tree-value:visible').first();
  await detailValue.focus(); await page.keyboard.press('Shift+F10');
  await menu.waitFor({ state: 'visible' }); await page.keyboard.press('Escape');
  assert.equal(await page.locator('#drawer').isVisible(), true); await assertFocus(detailValue);
  await page.keyboard.press('Escape'); assert.equal(await page.locator('#drawer').isVisible(), false); await assertFocus(rowButton(id));
  await page.evaluate(() => { api = window.__recordBaseApi; });
  results.drawer = { loadingFocus: 'dr-close', eventRef: true, oneEscapePerLayer: true, returnAfterFreshRows: true };


  phase = 'ordinary row and cell-menu exact detail return';
  await page.evaluate(() => {
    window.__ordinaryBaseApi = api; window.__ordinaryDetailRequests = [];
    api = async (command, args, options) => {
      if (command !== 'event_detail') return window.__ordinaryBaseApi(command, args, options);
      window.__ordinaryDetailRequests.push(structuredClone(args));
      const value = await window.__ordinaryBaseApi(command, args, options);
      await new Promise(resolve => { window.__ordinaryDetailRelease = resolve; });
      return value;
    };
  });
  for (const entry of ['row', 'cell-menu']) {
    const before = await page.evaluate(target => ({ requests: window.__ordinaryDetailRequests.length,
      eventRef: state.rows.find(row => row.id === target).event_ref }), id);
    await page.evaluate(() => { window.__ordinaryDetailRelease = null; });
    if (entry === 'cell-menu') {
      await rowCell(id).click({ button: 'right' });
      await page.evaluate(() => {
        state.rows = state.rows.map(row => ({ ...row })); renderTable({ total: state.total, rows: state.rows });
        if (!document.activeElement.closest('.ctx-menu')) throw Error('Requery must preserve cell-menu focus');
      });
      await menu.getByRole('menuitem', { name: 'Ver detalhes', exact: true }).click();
    } else await openOrdinaryDetail(entry, id);
    await page.waitForFunction(() => typeof window.__ordinaryDetailRelease === 'function');
    await assertFocus(page.locator('#dr-close'));
    assert.deepEqual(await page.evaluate(() => [...state.selectedEventRows.keys()]), [id]);
    assert.equal(await page.evaluate(() => window.__ordinaryDetailRequests.length), before.requests + 1);
    assert.equal(await page.evaluate(() => window.__ordinaryDetailRequests.at(-1).eventRef), before.eventRef);
    await page.evaluate(target => {
      const old = document.querySelector(`#events-table tr[data-event-id="${target}"] .record-actions-trigger`);
      state.rows = [...state.rows].reverse().map(row => ({ ...row })); renderTable({ total: state.total, rows: state.rows });
      if (old.isConnected || document.activeElement.id !== 'dr-close') throw Error('Fresh reorder must preserve loading Close focus');
      window.__ordinaryDetailRelease();
    }, id);
    await page.waitForFunction(target => state.currentDetailEv?.id === target, id);
    await assertFocus(page.locator('#dr-close'));
    await page.keyboard.press('Escape'); await assertFocus(rowButton(id));
    assert.equal(await page.locator('#drawer').isVisible(), false);
  }
  for (const entry of ['row', 'cell-menu']) for (const change of ['removed', 'new-ref', 'source-connected', 'outside', 'closed']) {
    phase = `ordinary ${entry} pending return: ${change}`;
    await page.evaluate(() => {
      window.__ordinaryRows = state.rows; window.__ordinaryLoadedAt = state.currentArtifact.loadedAt;
      window.__ordinaryDetailRelease = null;
    });
    await openOrdinaryDetail(entry, id);
    await page.waitForFunction(() => typeof window.__ordinaryDetailRelease === 'function');
    await assertFocus(page.locator('#dr-close'));
    await page.evaluate(({ id, change }) => {
      if (change === 'source-connected') state.currentArtifact.loadedAt = `${state.currentArtifact.loadedAt}-ordinary-owner`;
      if (change === 'removed' || change === 'new-ref') {
        state.rows = change === 'removed' ? state.rows.filter(row => row.id !== id)
          : state.rows.map(row => row.id === id ? { ...row, event_ref: `replacement:${id}` } : row);
        renderTable({ total: state.total, rows: state.rows });
      }
      if (change === 'outside') document.querySelector('#quick-search').focus();
      closeDrawer();
    }, { id, change });
    const expected = change === 'outside' ? page.locator('#quick-search') : change === 'closed' ? rowButton(id) : page.locator('#page-size');
    await assertFocus(expected);
    await page.evaluate(async () => { window.__ordinaryDetailRelease(); await new Promise(resolve => setTimeout(resolve, 0)); });
    await assertFocus(expected); assert.equal(await page.locator('#drawer').isVisible(), false);
    assert.equal(await page.evaluate(() => state.currentDetailEv), null, 'closed entry cannot accept a late reply');
    await page.evaluate(() => {
      state.rows = window.__ordinaryRows; state.currentArtifact.loadedAt = window.__ordinaryLoadedAt;
      renderTable({ total: state.total, rows: state.rows });
    });
  }

  // Use a near-top caller at scrollTop=0. Removing the old near-bottom
  // caller clamps scroll after the prior reverse and intentionally dismisses
  // context menus, which tests scrolling rather than stale detail admission.
  const staleCellId = await page.evaluate(() => state.rows[3].id);
  results.staleCellMenus = [];
  for (const change of ['removed', 'new-ref', 'source-connected']) {
    phase = `ordinary cell menu rejects stale captured identities: ${change}`;
    await page.evaluate(async () => {
      document.querySelector('#events-table').closest('.table-scroll').scrollTop = 0;
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    });
    await rowCell(staleCellId).click({ button: 'right' }); await menu.waitFor({ state: 'visible' });
    const before = await page.evaluate(() => ({ requests: window.__ordinaryDetailRequests.length,
      scrollTop: document.querySelector('#events-table').closest('.table-scroll').scrollTop }));
    assert.equal(before.scrollTop, 0, 'stale-menu fixture must start clear of the scroll clamp boundary');
    await page.evaluate(({ id, change }) => {
      window.__ordinaryRows = state.rows; window.__ordinaryLoadedAt = state.currentArtifact.loadedAt;
      if (change === 'source-connected') state.currentArtifact.loadedAt = `${state.currentArtifact.loadedAt}-ordinary-menu`;
      else {
        state.rows = change === 'removed' ? state.rows.filter(row => row.id !== id)
          : state.rows.map(row => row.id === id ? { ...row, event_ref: `replacement:${id}` } : row);
        renderTable({ total: state.total, rows: state.rows });
      }
    }, { id: staleCellId, change });
    const after = await page.evaluate(async () => {
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const scroller = document.querySelector('#events-table').closest('.table-scroll');
      const current = document.querySelector('.ctx-menu');
      return { scrollTop: scroller.scrollTop, scrollHeight: scroller.scrollHeight, clientHeight: scroller.clientHeight,
        menuVisible: !!current?.getClientRects().length, menuHasFocus: !!current?.contains(document.activeElement),
        focusedTag: document.activeElement?.tagName, focusedId: document.activeElement?.id, rows: state.rows.length };
    });
    results.staleCellMenus.push({ change, before, after });
    assert.equal(after.scrollTop, before.scrollTop, 'stale admission must be exercised without moving its surface');
    assert.equal(after.menuVisible, true, 'stale menu stays available until its guarded action is invoked');
    assert.equal(after.menuHasFocus, true, 'replacing the caller cannot steal menu focus');
    await menu.getByRole('menuitem', { name: 'Ver detalhes', exact: true }).click();
    assert.equal(await page.evaluate(() => window.__ordinaryDetailRequests.length), before.requests);
    assert.equal(await page.locator('#drawer').isVisible(), false);
    await page.evaluate(() => {
      state.rows = window.__ordinaryRows; state.currentArtifact.loadedAt = window.__ordinaryLoadedAt;
      renderTable({ total: state.total, rows: state.rows });
    });
  }
  phase = 'ordinary row modifier selection remains one-step';
  const selectionIds = await page.evaluate(() => { state.selectedEventRows = new Map(); state.lastSelectedRowId = null; updateRowSelectionStyles(); return state.rows.slice(0, 5).map(row => row.id); });
  const beforeModifiers = await page.evaluate(() => window.__ordinaryDetailRequests.length);
  await rowCell(selectionIds[0]).click({ modifiers: ['Control'] });
  await rowCell(selectionIds[2]).click({ modifiers: ['Meta'] });
  assert.deepEqual(await page.evaluate(() => [...state.selectedEventRows.keys()]), [selectionIds[0], selectionIds[2]]);
  await rowCell(selectionIds[4]).click({ modifiers: ['Shift'] });
  assert.deepEqual(await page.evaluate(() => [...state.selectedEventRows.keys()]), selectionIds.slice(2, 5));
  assert.equal(await page.evaluate(() => window.__ordinaryDetailRequests.length), beforeModifiers);
  assert.equal(await page.locator('#drawer').isVisible(), false);
  await rowButton(id).focus();
  await page.evaluate(() => { api = window.__ordinaryBaseApi; });
  results.ordinaryDetails = { oneClick: true, rowAndCellMenuExactRef: true, freshReorderedReturn: true,
    removedAndReusedIdFallback: true, sourceSwapFallback: true, noOutsideFocusSteal: true, lateRepliesIgnored: true,
    staleCellMenuBlocked: true, ctrlCmdShiftPreserved: true };

  phase = 'full requery and reused-row focus';
  await page.evaluate(() => { state.visibleCols = [...state.visibleCols].reverse(); renderTable({ total: state.total, rows: state.rows }, { reuseRows: true }); });
  await assertFocus(rowButton(id));
  await page.evaluate(() => { state.rows = state.rows.map(row => ({ ...row })); renderTable({ total: state.total, rows: state.rows }); });
  await assertFocus(rowButton(id));
  await page.evaluate(() => {
    window.__recordQueryBase = api;
    api = async (command, args, options) => {
      const result = await window.__recordQueryBase(command, args, options);
      if (command === 'query_page') await new Promise(resolve => { window.__recordQueryRelease = resolve; });
      return result;
    };
    void refresh({ analytics: false });
  });
  await page.waitForFunction(() => typeof window.__recordQueryRelease === 'function');
  await page.locator('#quick-search').focus();
  await page.evaluate(() => { window.__recordQueryRelease(); });
  await page.waitForFunction(() => document.querySelector('#events-table').getAttribute('aria-busy') === 'false');
  await assertFocus(page.locator('#quick-search'));
  await page.evaluate(() => { api = window.__recordQueryBase; });
  results.queryFocus = { reuseRows: true, freshRows: true, userMovedToSearch: true };

  phase = 'removed identity and stale-context menu';
  const removed = await triggers.first().evaluate(node => Number(node.closest('tr').dataset.eventId));
  await rowButton(removed).focus();
  await page.evaluate(target => { state.rows = state.rows.filter(row => row.id !== target); renderTable({ total: state.total, rows: state.rows }); }, removed);
  await assertFocus(page.locator('#page-size'));
  const staleId = await triggers.first().evaluate(node => Number(node.closest('tr').dataset.eventId));
  await rowButton(staleId).focus(); await page.keyboard.press('Enter');
  await page.evaluate(() => {
    window.__recordOldLoadedAt = state.currentArtifact.loadedAt;
    state.currentArtifact.loadedAt = `${state.currentArtifact.loadedAt}-focus-regression`;
  });
  await page.keyboard.press('Enter'); assert.equal(await page.locator('#drawer').isVisible(), false);
  await page.evaluate(() => { state.currentArtifact.loadedAt = window.__recordOldLoadedAt; });
  await page.evaluate(() => refresh({ analytics: false }));
  results.invalidTargets = { removalUsesPagination: true, sourceSwapBlocksStaleAction: true };

  phase = 'field headers and data contract';
  await page.evaluate(columns => { state.visibleCols = columns; renderTable({ total: state.total, rows: state.rows }, { reuseRows: true }); }, baseline.columns);
  assert.equal(await page.locator('#events-table .record-actions-heading').evaluate(node => typeof node.oncontextmenu), 'object');
  const dataHeader = page.locator('#events-table th[data-column]').first();
  const column = await dataHeader.getAttribute('data-column');
  await dataHeader.click({ button: 'right' });
  assert.ok(await menu.getByRole('menuitem', { name: /^Top 10 de / }).count());
  const fieldTarget = await page.evaluate(() => document.querySelector('#events-table th[data-column]').dataset.column);
  assert.equal(column, fieldTarget); await page.keyboard.press('Escape');
  assert.deepEqual(await page.evaluate(() => state.visibleCols), baseline.columns);
  assert.deepEqual(await page.evaluate(() => state.colWidths), baseline.widths);
  assert.equal(await page.evaluate(() => JSON.stringify(state.rows)), baseline.values, 'data values and native query order round-trip untouched');

  phase = 'visual comparison';
  results.visuals = [];
  for (const width of [1440, 1024]) for (const theme of ['dark', 'light']) {
    await page.setViewportSize({ width, height: 960 });
    await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
    await triggers.first().focus();
    const geometry = await triggers.first().evaluate(node => ({ width: node.getBoundingClientRect().width, height: node.getBoundingClientRect().height,
      cellWidth: node.closest('td').getBoundingClientRect().width, font: getComputedStyle(document.querySelector('#events-table')).fontSize }));
    assert.ok(geometry.width >= 32 && geometry.height >= 32); assert.ok(geometry.cellWidth <= 50, 'action column remains narrow');
    await screenshot(`record-actions-${theme}-${width}.png`);
    results.visuals.push({ width, theme, ...geometry });
  }
  await page.locator('#skip-records').focus(); await screenshot('record-actions-skip-1024.png');
  assert.deepEqual(errors, []); results.ok = true;
  writeFileSync(resolve(output, 'record-actions.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, 'record-actions', error, { phase, errors, results }); throw error;
} finally { await browser.close(); }
