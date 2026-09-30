/* Areas (Análise, Caso, Estrutura), grouped menus and per-tab loading with selective cancel. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
const url = process.argv[2] || "http://127.0.0.1:4173";
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
const css = name => page.evaluate(n => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
const menu = () => page.evaluate(() => [...document.querySelectorAll(".nav-pages button")].filter(b => !b.hidden).map(b => b.dataset.page));
try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && !state.loadOverlay);

  // Same background in every area; only the accent changes.
  const analysis = { bg: await css("--bg-0"), accent: await css("--accent"), menu: await menu() };
  await page.click('.zone-switch [data-zone="case"]');
  await page.waitForFunction(() => WorkspaceContext.scope() === "case" && !WorkspaceContext.changing);
  const caseArea = { bg: await css("--bg-0"), accent: await css("--accent"), menu: await menu() };
  await page.click('.zone-switch [data-zone="structure"]');
  await page.waitForFunction(() => document.body.dataset.page === "sources" && !WorkspaceContext.changing);
  const structure = { bg: await css("--bg-0"), accent: await css("--accent"), menu: await menu(), scope: await page.evaluate(() => WorkspaceContext.scope()) };
  assert.equal(caseArea.bg, analysis.bg); assert.equal(structure.bg, analysis.bg);
  assert.equal(new Set([analysis.accent, caseArea.accent, structure.accent]).size, 3, "yellow, purple and orange accents");
  assert.deepEqual(analysis.menu, ["summary", "compromises", "case-timeline", "explore"]);
  assert.deepEqual(caseArea.menu, ["summary", "compromises", "case-timeline", "explore", "evidence"]);
  assert.deepEqual(structure.menu, ["sources", "connections", "import"]);
  assert.equal(structure.scope, "dataset");
  results.areas = { analysis: analysis.accent, case: caseArea.accent, structure: structure.accent };

  await page.click('.nav-pages [data-page="connections"]');
  await page.waitForSelector("#ws-content .remote-modal.docked");
  await page.click('.nav-pages [data-page="import"]');
  assert.equal(await page.locator("#ws-empty").isVisible(), true);
  await page.click('.nav-pages [data-page="sources"]');
  assert.equal(await page.locator("#ws-content .remote-modal").count(), 0, "connections panel returns to its overlay");
  results.structure = "Arquivos, Conexões e Abrir logs";

  // Linha do tempo groups its sections as tabs; Comparar lives in Descobrir; no Arquivos tab in Explorar.
  await page.click('.zone-switch [data-zone="analysis"]');
  await page.click('.nav-pages [data-page="case-timeline"]');
  assert.deepEqual(await page.locator("#page-tabs [role=tab]").allTextContents(), ["Linha do tempo", "Possíveis trilhas"]);
  await page.getByRole("tab", { name: "Possíveis trilhas" }).click();
  await page.waitForFunction(() => document.body.dataset.page === "journeys");
  assert.equal(await page.locator('.nav-pages [data-page="case-timeline"]').getAttribute("aria-current"), "page");
  await page.evaluate(() => Workspace.showPage("compare"));
  await page.waitForSelector(".discovery-compare #ws-compare-form");
  assert.equal(await page.evaluate(() => [document.body.dataset.page, state.activeDatasetTab, Discovery.mode()].join()), "explore,dashboard,compare");
  assert.equal(await page.locator("#btn-back-drive").isVisible(), false);
  results.menus = "Linha do tempo em abas; Comparar em Descobrir";

  // Topbar: export where the codes were; codes inside Settings.
  assert.equal(await page.locator(".topbar #ws-export").isVisible(), true);
  await page.click("#btn-settings");
  await page.click('[data-settings-tab="codes"]');
  await page.waitForSelector("#settings-pane-codes #codes-editor");
  await page.evaluate(() => { document.querySelector("#settings-modal").hidden = true; });
  results.topbar = "Exportar e Configurações → Códigos";

  // Loading follows the tab that asked for it, survives navigation and can be cancelled per item.
  await page.evaluate(() => { window.__mockLatency = { timeline_range: 3000, discover_patterns: 4000 }; Discovery.setMode("overview"); });
  await page.evaluate(() => Workspace.search("Erro"));
  await page.waitForFunction(() => document.body.dataset.page === "explore" && state.quick === "Erro");
  await page.click('.nav-pages [data-page="case-timeline"]');
  await page.getByRole("tab", { name: "Linha do tempo" }).click();
  await page.waitForTimeout(400);
  const before = await page.evaluate(() => window.__mockCommandCalls.timeline_range || 0);
  await page.click('.nav-pages [data-page="explore"]');
  await page.click("#tabbtn-dashboard");
  await page.click('.discovery-mode[data-mode="patterns"]');
  await page.waitForFunction(() => { const labels = Tasks.groups().map(g => g.label).join("|"); return /Linha do tempo/.test(labels) && /Padrões/.test(labels); });
  const status = await page.locator("#workbar-tasks span").textContent();
  assert.match(status, /^Carregando /);
  assert.equal(await page.locator('.nav-pages [data-page="case-timeline"] > .li-loader').count(), 1, "the loading menu item has a compact spinner");
  assert.equal(await page.locator('.nav-pages [data-page="case-timeline"]').getAttribute('aria-busy'),'true');
  assert.equal(await page.locator('.li-waves').count(),0);
  await page.screenshot({path:'output/playwright/loading-orbit.png'});
  await page.click('.nav-pages [data-page="case-timeline"]');
  await page.waitForTimeout(300);
  assert.equal(await page.evaluate(() => window.__mockCommandCalls.timeline_range) - before, 0, "returning joins the same request");
  await page.click("#workbar-tasks");
  await page.locator(".task-group", { hasText: "Padrões" }).locator("[data-cancel]").click();
  await page.waitForFunction(() => ![...document.querySelectorAll(".task-group")].some(g => g.textContent.includes("Padrões")));
  assert.equal(await page.locator(".task-group", { hasText: "Linha do tempo" }).count(), 1);
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("#workbar").classList.contains("has-tasks"), null, { timeout: 15000 });
  assert.equal(await page.evaluate(() => !!document.querySelector(".tl-main svg")), true, "the timeline finishes after the other task is cancelled");
  assert.equal(await page.locator('.nav-pages [data-page="case-timeline"]').getAttribute('aria-busy'),null);
  results.loading = status;

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ ...results, errors }, null, 2));
} finally {
  await browser.close();
}
