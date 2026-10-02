/* Shared menu browser regression. Uses preview UI/transport, not native Rust. */
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';

const output = resolve('output/playwright');
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: 'reduce' });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: 'Preview browser UI, not native engine verification' };
let phase = 'startup';
page.on('pageerror', error => errors.push(error.message));
const focusedText = () => page.evaluate(() => document.activeElement.textContent);
const assertFocus = async locator => assert.equal(await locator.evaluate(node => node === document.activeElement), true);
const menu = page.getByRole('menu', { name: 'Ações do contexto', exact: true });

try {
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector('#load-overlay').hidden);
  await page.getByRole('button', { name: 'Explorar', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#events-table').getAttribute('aria-busy') === 'false' && state.rows.length);

  phase = 'existing focusable callers';
  const field = page.locator('#explore-tree .field-item:visible').first();
  await field.focus(); await field.press('Shift+F10');
  await menu.waitFor({ state: 'visible' });
  await assertFocus(menu.getByRole('menuitem').first());
  await page.keyboard.press('Escape'); await assertFocus(field);
  assert.equal(await menu.count(), 0);
  await page.locator('#btn-case-menu').focus(); await page.keyboard.press('Enter');
  await menu.waitFor({ state: 'visible' });
  results.keyboardPlacement = await page.evaluate(() => {
    const anchor = document.querySelector('#btn-case-menu').getBoundingClientRect(), bounds = document.querySelector('.ctx-menu').getBoundingClientRect();
    return { anchorBottom: anchor.bottom, menuTop: bounds.top };
  });
  assert.ok(Math.abs(results.keyboardPlacement.anchorBottom - results.keyboardPlacement.menuTop) < 2, 'keyboard click anchors to its control');
  await page.keyboard.press('Escape'); await assertFocus(page.locator('#btn-case-menu'));

  phase = 'navigation and single activation';
  await page.evaluate(() => {
    window.__contextMenuActions = [];
    const trigger = document.createElement('button'); trigger.id = 'context-menu-test-trigger'; trigger.textContent = 'Menu regression fixture';
    trigger.style.cssText = 'position:fixed;top:12px;right:12px;z-index:9999';
    trigger.oncontextmenu = event => {
      event.preventDefault();
      showCtxMenu(event.clientX, event.clientY, [
        { label: 'Unavailable', disabled: true },
        { label: 'First', onClick: () => window.__contextMenuActions.push('first') },
        { sep: true },
        { label: 'Unavailable in between', disabled: true },
        { label: 'Last', onClick: () => window.__contextMenuActions.push('last') },
      ]);
    };
    document.body.appendChild(trigger);
  });
  const trigger = page.locator('#context-menu-test-trigger');
  await trigger.focus(); await trigger.press('Shift+F10');
  assert.equal(await focusedText(), 'Unavailable');
  await page.keyboard.press('Enter'); assert.deepEqual(await page.evaluate(() => window.__contextMenuActions), []);
  await page.keyboard.press('ArrowUp'); assert.equal(await focusedText(), 'Last');
  await page.keyboard.press('ArrowDown'); assert.equal(await focusedText(), 'Unavailable');
  await page.keyboard.press('ArrowDown'); assert.equal(await focusedText(), 'First');
  await page.keyboard.press('End'); assert.equal(await focusedText(), 'Last');
  await page.keyboard.press('Home'); assert.equal(await focusedText(), 'Unavailable');
  await page.keyboard.press('ArrowDown'); assert.equal(await focusedText(), 'First');
  await page.keyboard.press('Space'); await assertFocus(trigger);
  assert.deepEqual(await page.evaluate(() => window.__contextMenuActions), ['first']);
  await trigger.press('Shift+F10'); await page.keyboard.press('End'); await page.keyboard.press('Enter');
  assert.deepEqual(await page.evaluate(() => window.__contextMenuActions), ['first', 'last']);
  await trigger.press('Shift+F10'); await page.keyboard.press('Tab'); assert.equal(await menu.count(), 0);

  phase = 'narrow viewport and long menu';
  await page.setViewportSize({ width: 320, height: 600 });
  await page.evaluate(() => showCtxMenu(innerWidth + 100, innerHeight + 100,
    Array.from({ length: 60 }, (_, index) => ({ label: `${index} ${'Long label '.repeat(25)}`, onClick() {} }))));
  const bounds = await menu.boundingBox();
  assert.ok(bounds.x >= 0 && bounds.y >= 0 && bounds.x + bounds.width <= 320 && bounds.y + bounds.height <= 600);
  await page.keyboard.press('End');
  assert.equal(await page.evaluate(() => document.activeElement === [...document.querySelectorAll('.ctx-item')].at(-1)), true);
  assert.ok(await menu.evaluate(node => node.scrollTop > 0), 'last item is scrolled into view');
  results.narrowMenuBounds = bounds;
  await page.keyboard.press('Escape'); await page.setViewportSize({ width: 1440, height: 960 });
  await trigger.evaluate(node => node.remove());

  phase = 'drawer keyboard menu';
  await page.locator('#events-table tbody tr[data-event-id] td[data-column]').first().click();
  await page.waitForFunction(() => state.currentDetailEv && !document.querySelector('#drawer').hidden);
  const detail = page.locator('#pane-overview .detail-tree-value:visible').first();
  await detail.focus(); await detail.press('Shift+F10');
  await menu.waitFor({ state: 'visible' });
  const detailId = await page.evaluate(() => state.currentDetailEv.id);
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.evaluate(() => state.currentDetailEv.id), detailId, 'menu arrows cannot step the drawer');
  await page.screenshot({ path: resolve(output, 'context-menu-drawer-1440.png') });
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('#drawer').isVisible(), true); await assertFocus(detail);
  assert.equal(await menu.count(), 0);

  phase = 'modal Escape and pointer dismissal';
  await detail.click(); await page.locator('#detail-value-modal').waitFor({ state: 'visible' });
  await page.locator('#detail-value-content').click({ button: 'right' }); await menu.waitFor({ state: 'visible' });
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('#detail-value-modal').isVisible(), true);
  assert.equal(await page.locator('#drawer').isVisible(), true);
  assert.equal(await page.evaluate(() => document.querySelector('#detail-value-modal').contains(document.activeElement)), true, 'fallback stays inside its modal');
  await page.locator('#detail-value-content').click({ button: 'right' });
  await page.locator('#detail-value-close').click();
  assert.equal(await menu.count(), 0); assert.equal(await page.locator('#detail-value-modal').isVisible(), false);
  assert.equal(await page.locator('#drawer').isVisible(), true);
  results.drawerAndModalIsolation = true;
  assert.deepEqual(errors, []);
  results.ok = true;
  writeFileSync(resolve(output, 'context-menu.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, 'context-menu', error, { phase, errors, results });
  throw error;
} finally {
  await browser.close();
}
