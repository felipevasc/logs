/* Update dialog and Settings → Atualizações against the preview mock (no real download or installation). */
import assert from "node:assert/strict";
import { chromium } from "playwright";
const url = process.argv[2] || "http://127.0.0.1:4173";
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome" });
const errors = [], results = {};
const NOTES = "# LogInsight 0.6.0\n\n## Atualizações\n\n- Verifica versões **novas** ao abrir.\n- Instala com `um clique`.\n\nDetalhes em [Atualizações](https://example.org/docs).\n\n<img src=x onerror=alert(1)>";
async function open(update) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.on("pageerror", error => errors.push(error.message));
  if (update) await page.addInitScript(value => { window.__mockUpdate = value; }, update);
  await page.goto(url);
  await page.waitForFunction(() => window.Updates && window.WorkspaceContext?.ready);
  return page;
}
const dialog = ".update-overlay:not([hidden])";
try {
  const page = await open({ version: "0.6.0", notes: NOTES });
  await page.waitForSelector(dialog, { timeout: 8000 });
  const lead = await page.textContent(".update-lead");
  assert.match(lead, /0\.6\.0 está disponível\. Você usa a versão 0\.5\.1/);
  assert.equal(await page.textContent(".update-notes strong"), "LogInsight 0.6.0");
  assert.equal(await page.locator(".update-notes .update-bullet").count(), 2);
  assert.equal(await page.locator(".update-notes img").count(), 0, "release notes are text, never markup");
  assert.ok((await page.textContent(".update-notes")).includes("Detalhes em Atualizações."), "links show their text");
  assert.ok(await page.locator("#btn-settings.has-update").count(), "settings button marks the pending update");
  results.announce = "diálogo ao abrir, notas como texto, marca em Configurações";

  await page.getByRole("button", { name: "Atualizar agora" }).click();
  await page.waitForSelector(".update-progress span");
  await page.getByRole("button", { name: "Reiniciar e instalar" }).waitFor({ timeout: 8000 });
  await page.getByRole("button", { name: "Instalar ao fechar" }).click();
  await page.waitForSelector(".update-overlay[hidden]", { state: "attached" });
  assert.equal(await page.evaluate(() => window.__TAURI__.core.invoke("update_status").then(s => s.installOnClose)), true);
  await page.evaluate(() => Updates.open());
  await page.getByRole("button", { name: "Não instalar ao fechar" }).waitFor();
  await page.getByRole("button", { name: "Reiniciar e instalar" }).click();
  await page.waitForFunction(() => document.querySelector(".update-lead")?.textContent.startsWith("Instalando"));
  assert.equal(await page.locator(".update-overlay [data-close]").isHidden(), true, "the dialog stays open while installing");
  results.install = "download com progresso, instalar ao fechar, reiniciar e instalar";
  await page.close();

  const skip = await open({ version: "0.6.0", notes: NOTES });
  await skip.waitForSelector(dialog, { timeout: 8000 });
  await skip.getByRole("button", { name: "Pular esta versão" }).click();
  await skip.waitForSelector(".update-overlay[hidden]", { state: "attached" });
  await skip.evaluate(() => openSettings("updates"));
  await skip.waitForSelector("#settings-pane-updates .update-toggle input");
  assert.match(await skip.textContent("#settings-pane-updates"), /A versão 0\.6\.0 foi pulada/);
  await skip.getByText("Voltar a oferecer").click();
  await skip.waitForFunction(() => !document.querySelector("#settings-pane-updates").textContent.includes("foi pulada"));
  results.skip = "pular versão e voltar a oferecer";
  await skip.close();

  const current = await open(null);
  await current.waitForTimeout(3500);
  assert.equal(await current.locator(dialog).count(), 0, "no dialog without a new version");
  await current.evaluate(() => openSettings("updates"));
  await current.waitForSelector("#settings-pane-updates .update-toggle input");
  const pane = await current.textContent("#settings-pane-updates");
  assert.match(pane, /LogInsight 0\.5\.1/);
  assert.match(pane, /Você está usando a versão mais recente/);
  await current.locator("#settings-pane-updates .update-toggle input").uncheck();
  assert.equal(await current.evaluate(() => window.__TAURI__.core.invoke("update_status").then(s => s.checkOnStart)), false);
  await current.getByRole("button", { name: "Versões publicadas" }).click();
  assert.equal(await current.evaluate(() => window.__mockOpenedRelease), "latest");
  results.settings = "sem diálogo quando atualizado; aba Atualizações";

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ ...results, errors }, null, 2));
} finally {
  await browser.close();
}
