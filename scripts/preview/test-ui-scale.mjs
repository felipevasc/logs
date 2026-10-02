import { fileURLToPath } from "node:url";
/* Requested scale, accessible settings, shortcuts, persistence and control reflow.
   Preview ui_zoom returns false: these checks do not verify installed native zoom. */
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { launchBrowser } from "./browser.mjs";
const url = process.argv[2] || "http://127.0.0.1:4173";
const browser = await launchBrowser();
const errors = [], results = {};
const consoleErrors = [], failedRequests = [];
const directory = new URL("../../output/playwright/", import.meta.url);
await mkdir(directory, { recursive: true });
let page, phase = "initial load";
try {
  // A 2000×1054 screen at 150% system scaling: the window is 1333×703 CSS pixels.
  const context = await browser.newContext({ viewport: { width: 1333, height: 703 }, deviceScaleFactor: 1.5 });
  page = await context.newPage();
  page.on("pageerror", error => errors.push(error.message));
  page.on("console", message => { if (message.type() === "error") consoleErrors.push(message.text()); });
  page.on("requestfailed", request => failedRequests.push({ url: request.url(), error: request.failure()?.errorText }));
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && !state.loadOverlay && window.UiScale);
  const settled = () => page.waitForFunction(() => UiScale.status().state !== "pending");
  await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 0.85, "automatic mode requests the existing small/high-DPI scale");
  assert.deepEqual(await page.evaluate(() => [UiScale.status().state, UiScale.status().applied]), ["failed", 1], "preview does not apply native zoom");
  results.transport = "synthetic-preview; ui_zoom returns false";
  results.nativeZoomAt200Percent = "not verified; requires the installed app";
  results.fullApplication320CssPixelReflow = "not verified; only settings controls are covered here";
  await page.keyboard.press("Control+Minus");
  await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 0.8);
  assert.equal(await page.evaluate(() => localStorage.getItem("li-ui-scale")), "0.8");
  assert.equal(await page.evaluate(() => {
    const saved = JSON.parse(localStorage.getItem("__mockStore"));
    return saved?.cases?.find(item => item.id === saved.active)?.activeArtifactId === state.currentArtifact?.id;
  }), true, "a completed initial load has persisted its source before restart");
  phase = "restore after reload";
  await page.reload();
  await page.waitForFunction(() => window.UiScale && state.loaded);
  await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 0.8, "a chosen size survives a restart");
  await page.keyboard.press("Control+0");
  await settled();
  assert.deepEqual(await page.evaluate(() => [UiScale.current(), UiScale.setting()]), [0.85, "auto"]);
  results.shortcuts = "Ctrl − / Ctrl 0 e persistência";

  phase = "settings and keyboard";
  await page.evaluate(() => openSettings());
  await page.waitForSelector("#settings-pane-interface [data-scale]");
  const scales = page.locator('#settings-pane-interface [data-scale]');
  const choice = value => page.locator(`#settings-pane-interface [data-scale="${value}"]`);
  assert.deepEqual(await scales.evaluateAll(nodes => nodes.map(node => node.dataset.scale)),
    ["auto", "0.7", "0.75", "0.8", "0.85", "0.9", "1", "1.1", "1.25", "1.4", "1.5", "1.75", "2"]);
  await choice(2).click(); await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 2);
  assert.equal(await choice(2).getAttribute("aria-checked"), "true");
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.scale), "2", "focus survives the asynchronous pane render");
  assert.match(await page.locator("#ui-scale-status").textContent(), /200% selecionado.*Não foi possível aplicar.*100%/);
  assert.equal(await page.locator('#settings-pane-interface [data-scale][tabindex="0"]').count(), 1);
  await page.keyboard.press("ArrowLeft"); await settled();
  assert.equal(await page.evaluate(() => UiScale.setting()), 1.75);
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.scale), "1.75");
  await page.keyboard.press("End"); await settled();
  assert.equal(await page.evaluate(() => UiScale.setting()), 2);
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.themeChoice), "dark", "Tab leaves the scale group once");
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.id), "settings-close", "the modal trap skips the inactive theme radio");
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.themeChoice), "dark");
  await page.keyboard.press("ArrowRight");
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "light");
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.themeChoice), "light");
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.dataset.scale), "2");
  await page.keyboard.press("Home"); await settled();
  assert.equal(await page.evaluate(() => UiScale.setting()), "auto");
  await choice(2).focus(); await page.keyboard.press("Space"); await settled();
  assert.equal(await choice(2).getAttribute("aria-checked"), "true", "Space uses the button's native activation");
  await choice(1.5).focus(); await page.keyboard.press("Enter"); await settled();
  assert.equal(await choice(1.5).getAttribute("aria-checked"), "true", "Enter uses the button's native activation");
  await page.keyboard.press("Control+Equal"); await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 1.75);
  await page.keyboard.press("Control+Equal"); await settled();
  await page.keyboard.press("Control+Equal"); await settled();
  assert.equal(await page.evaluate(() => UiScale.current()), 2, "Ctrl + reaches and stops at 200%");
  await page.evaluate(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "-", ctrlKey: true, isComposing: true, bubbles: true })));
  assert.equal(await page.evaluate(() => UiScale.current()), 2, "IME composition does not change scale");
  results.settings = "70–200% e Automático; setas/Home/End, Enter/Espaço, tabulação e foco";

  phase = "manual enlargement persistence";
  await page.reload();
  await page.waitForFunction(() => window.UiScale && state.loaded); await settled();
  assert.deepEqual(await page.evaluate(() => [UiScale.current(), UiScale.setting(), UiScale.status().applied]), [2, 2, 1],
    "200% remains selected across reload without claiming preview zoom");
  await page.evaluate(() => openSettings());
  results.controlReflow = [];
  for (const { width, height } of [{ width: 1024, height: 720 }, { width: 640, height: 420 }, { width: 320, height: 720 }]) {
    phase = `settings control reflow at ${width} CSS px`;
    await page.setViewportSize({ width, height });
    await page.locator(`#settings-pane-interface [data-theme-choice="${width === 1024 ? "dark" : "light"}"]`).click();
    await page.waitForTimeout(300); // Wait through the actual resize debounce.
    assert.deepEqual(await page.evaluate(() => [UiScale.current(), UiScale.setting()]), [2, 2], "resize preserves the manual choice");
    await choice(2).focus();
    await page.keyboard.press("ArrowLeft"); await settled();
    assert.equal(await page.evaluate(() => document.activeElement?.dataset.scale), "1.75", "roving focus follows wrapped choices");
    await page.keyboard.press("ArrowRight"); await settled();
    assert.deepEqual(await page.evaluate(() => [UiScale.setting(), document.activeElement?.dataset.scale]), [2, "2"], "reflow preserves keyboard selection and restored focus");
    const layout = await page.evaluate(() => {
      const pane = document.querySelector("#settings-pane-interface");
      const group = pane.querySelector(".ui-scale-choices");
      const box = group.getBoundingClientRect(), paneBox = pane.getBoundingClientRect();
      const groupStyle = getComputedStyle(group), preferenceStyle = getComputedStyle(group.parentElement);
      const controls = [...pane.querySelectorAll("[role=radio]")].map(node => {
        const rect = node.getBoundingClientRect(), style = getComputedStyle(node);
        return { label: node.textContent.trim(), left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom,
          width: rect.width, height: rect.height, fontSize: Number.parseFloat(style.fontSize) };
      });
      return { viewport: innerWidth, groupWidth: box.width, paneWidth: paneBox.width,
        gridColumns: preferenceStyle.gridTemplateColumns, groupDisplay: groupStyle.display, groupWrap: groupStyle.flexWrap,
        horizontalOverflow: pane.scrollWidth > pane.clientWidth + 1,
        rows: new Set([...group.children].map(node => Math.round(node.getBoundingClientRect().top))).size,
        controls, focused: document.activeElement?.dataset.scale,
        groupFits: box.left >= paneBox.left - 1 && box.right <= paneBox.right + 1,
        controlsFit: controls.every(control => control.left >= paneBox.left - 1 && control.right <= paneBox.right + 1
          && control.left >= 0 && control.right <= innerWidth),
      };
    });
    assert.equal(layout.viewport, width, "the preview has not faked native zoom");
    assert.equal(layout.horizontalOverflow, false, `settings pane has no horizontal overflow: ${JSON.stringify(layout)}`);
    assert.equal(layout.groupFits && layout.controlsFit, true, `scale and theme controls fit at ${width}px: ${JSON.stringify(layout)}`);
    assert.ok(layout.controls.every(control => control.width >= 24 && control.height >= 30 && control.fontSize >= 12), "options keep their existing readable text and target size");
    if (width <= 640) assert.ok(layout.rows > 1, "scale choices wrap instead of shrinking");
    assert.equal(layout.focused, "2");
    await page.keyboard.press("Tab");
    const themeInView = await page.evaluate(() => {
      const node = document.activeElement, box = node.getBoundingClientRect();
      return !!node.dataset.themeChoice && box.top >= 0 && box.bottom <= innerHeight;
    });
    assert.equal(themeInView, true, "theme remains reachable by keyboard in a short or narrow settings dialog");
    await choice(2).focus();
    await page.screenshot({ path: fileURLToPath(new URL(`ui-scale-settings-${width}.png`, directory)), fullPage: true });
    results.controlReflow.push({ width, height, rows: layout.rows, paneWidth: layout.paneWidth, groupWidth: layout.groupWidth });
  }
  await page.setViewportSize({ width: 1333, height: 703 });
  await page.evaluate(async () => { await UiScale.set(1, false); document.querySelector("#settings-modal").hidden = true; });

  // Layouts at 100% in the same small window.
  phase = "layouts";
  await page.evaluate(async () => { await Workspace.showPage("explore"); switchTab("group"); });
  await page.waitForFunction(() => document.querySelectorAll("#group-table tbody tr").length > 1);
  const widths = await page.evaluate(() => [document.querySelector("#group-table").getBoundingClientRect().width, document.querySelector("#group-table").parentElement.clientWidth]);
  assert.ok(widths[0] >= widths[1] - 2, `group table fills its panel: ${widths}`);
  await page.evaluate(() => Workspace.showPage("compare"));
  await page.waitForSelector(".compare-form input");
  const overflow = await page.evaluate(() => { const box = document.querySelector(".compare-form").getBoundingClientRect(); return [...document.querySelectorAll(".compare-form input, .compare-form button")].filter(e => e.getBoundingClientRect().right > box.right + 1).length; });
  assert.equal(overflow, 0, "period fields stay inside the card");
  results.layout = "Resumir usa a largura toda; Comparar sem estouro";

  // A regular screen without system scaling keeps text legible.
  phase = "automatic scale";
  const plain = await browser.newPage({ viewport: { width: 1024, height: 680 }, deviceScaleFactor: 1 });
  await plain.goto(url);
  await plain.waitForFunction(() => window.UiScale);
  assert.equal(await plain.evaluate(() => UiScale.current()), 0.9);
  const large = await browser.newPage({ viewport: { width: 1920, height: 1000 } });
  await large.goto(url);
  await large.waitForFunction(() => window.UiScale);
  assert.equal(await large.evaluate(() => UiScale.current()), 1);
  results.automatic = "85% (1333×703 @150%), 90% (1024×680 @100%), 100% (1920×1000)";

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ ...results, errors }, null, 2));
} catch (error) {
  const diagnostics = { phase, error: String(error.stack || error), errors, consoleErrors, failedRequests, results };
  try {
    diagnostics.page = await page?.evaluate(() => {
      const current = typeof state === "undefined" ? null : state;
      const stored = JSON.parse(localStorage.getItem("__mockStore") || "null");
      const caseSummary = item => item && ({ id: item.id, activeArtifactId: item.activeArtifactId, artifacts: item.artifacts?.map(({ id, path, source }) => ({ id, path, source })) });
      return {
        url: location.href, readyState: document.readyState,
        scale: window.UiScale?.status(), workspaceReady: window.WorkspaceContext?.ready,
        workspaceChanging: window.WorkspaceContext?.changing, scope: window.WorkspaceContext?.scope(),
        loaded: current?.loaded, loadOverlay: current?.loadOverlay, queryError: current?.queryError,
        active: current?.cases?.active, currentArtifact: current?.currentArtifact,
        cases: current?.cases?.cases?.map(caseSummary), savedActive: stored?.active, savedCases: stored?.cases?.map(caseSummary),
        loadStatus: document.querySelector("#load-status")?.textContent,
        toasts: [...document.querySelectorAll(".toast")].map(node => node.textContent),
        requests: window.__mockRequests,
      };
    });
  } catch (captureError) { diagnostics.captureError = String(captureError); }
  try { await page?.screenshot({ path: fileURLToPath(new URL("ui-scale-failure.png", directory)), fullPage: true }); }
  catch (captureError) { diagnostics.screenshotError = String(captureError); }
  await writeFile(new URL("ui-scale-failure.json", directory), JSON.stringify(diagnostics, null, 2));
  console.error(JSON.stringify(diagnostics, null, 2));
  throw error;
} finally {
  await browser.close();
}
