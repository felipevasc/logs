/* Close-up captures of the expression editor in both themes (synthetic transport). */
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  context = await browser.newContext({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2 }),
  page = await context.newPage();
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript({ path: "scripts/preview/mock-local-regex.js" });
await page.goto(process.argv[2] || "http://127.0.0.1:4173");
await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay);
await page.getByRole("button", { name: "Explorar", exact: true }).click();
await page.locator("#btn-add-expr").click();
const SAMPLE = "Falha user=ana origem=10.0.0.5\nuser=ana confirmado=ana\nuser=ana confirmado=joao\nuser=pedro confirmado=pedro";
for (const theme of ["dark", "light"]) {
  await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
  await page.evaluate(([pattern, sample]) => {
    const bench = EventPatternSearch.open();
    bench.sample.set(sample);
    bench.setDraft({ pattern, engine: "pcre2", caseSensitive: true });
    document.querySelector("#expr-search .rxw-sample").open = true;
  }, ["(?<=user=)(?<usuario>\\w+)\\s+confirmado=\\k<usuario>\\b|(?:x)\\1(", SAMPLE]);
  await page.waitForTimeout(900);
  await page.locator("#expr-search .rxw-edit-row .rx-input").evaluate((el) => { el.focus(); el.setSelectionRange(12, 12); el.dispatchEvent(new Event("keyup")); });
  await page.waitForTimeout(150);
  await page.locator("#expr-search").screenshot({ path: `output/playwright/regex-closeup-${theme}.png` });
  await page.evaluate(() => { const b = EventPatternSearch.open(); b.setDraft({ pattern: "user=(?<usuario>\\w+)\\s+confirmado=\\k<usuario>\\b" }); });
  await page.waitForTimeout(900);
  await page.locator("#expr-search").screenshot({ path: `output/playwright/regex-result-${theme}.png` });
}
await browser.close();
console.log("captured");
