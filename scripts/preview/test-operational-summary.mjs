import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { launchBrowser } from "./browser.mjs";

export async function verifyOperationalSummary(browser) {
  const page = await browser.newPage({ viewport: { width: 1170, height: 850 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await page.waitForFunction(() => window.WorkspaceContext?.ready && state.loaded && !WorkspaceContext.changing);
  await page.evaluate(() => Workspace.showPage("summary"));
  const host = page.locator("#ws-signals");
  await host
    .locator(".op-status")
    .filter({ hasText: /indícios/ })
    .waitFor();
  const first = host.locator(".op-row").first();
  await first.focus();
  await first.press("Enter");
  const panel = host.locator(".op-detail:visible");
  await panel.getByRole("button", { name: "Registros" }).click();
  await panel.locator(".op-extra").waitFor();
  await panel.getByRole("button", { name: "Hipótese" }).click();
  await panel.getByText("Escrever a hipótese").click();
  await panel.getByRole("textbox", { name: "Hipótese em português" }).fill("Qual é a causa?");
  await panel.getByRole("button", { name: "Comparar", exact: true }).click();
  assert.match(await panel.locator(".op-hyp-result").textContent(), /campo=valor/);
  await panel.getByRole("textbox", { name: "Hipótese em português" }).fill("Compare source=web quando level=Erro");
  await panel.getByRole("button", { name: "Comparar", exact: true }).click();
  await panel.getByRole("button", { name: "Contraexemplos" }).waitFor();
  await panel.getByRole("button", { name: "Contraexemplos" }).click();
  await panel.getByRole("button", { name: "Registros" }).click();
  await first.press("Escape");
  assert.equal(await first.getAttribute("aria-expanded"), "false");
  assert.ok(await first.evaluate((e) => document.activeElement === e));
  const duration = await page.evaluate(() => {
    const cur = state.currentArtifact?.source;
    if (!cur) return null;
    return Operational.withChoice({ kind: "file", paths: ["a", "b"], format: "auto" }, "b", "/events");
  });
  assert.equal(duration.members[0].format, "auto");
  assert.equal(duration.members[1].format, "json-list:/events");
  await first.click();
  await panel.getByRole("button", { name: "Registros" }).click();
  mkdirSync("output/playwright", { recursive: true });
  await host.screenshot({ path: "output/playwright/operational-summary.png" });
  await page.setViewportSize({ width: 390, height: 800 });
  assert.ok(await host.evaluate((e) => e.scrollWidth <= e.clientWidth + 1));
  // Feed the real component a second native-shaped report to exercise stage
  // alignment, raw reference navigation, and the page after the first twelve operations.
  await page.evaluate(() => {
    const root = document.getElementById("ws-signals");
    const at = Date.UTC(2026, 2, 8, 13);
    const record = (id, role, stage) => ({
      event_ref: "backup:" + id,
      file: "backup.jsonl",
      role,
      raw: JSON.stringify({ id, etapa: stage }),
      timestamp: at + id * 1000,
    });
    const stages = ["Início", "Verificação", "Conclusão"];
    const cases = Array.from({ length: 14 }, (_, i) => [
      "cópia " + (i + 1),
      [record(i * 3, "Operação", stages[0]), record(i * 3 + 2, "Operação", stages[2])],
    ]);
    const signal = {
      selector: "example",
      kind: "steps",
      title: "Etapa sem registro · Verificação",
      source: "Backup",
      count: 14,
      population: 240,
      priority: 65,
      detail: "A sequência habitual inclui verificação antes da conclusão.",
      measures: [],
      observed: [stages[0], stages[2]],
      reference: stages,
      examples: stages.map((s, i) => record(50 + i, "Referência", s)),
      bins: Array.from({ length: 32 }, (_, i) => [4 + ((i * 13) % 11), i % 5 === 0 ? 2 : 0]),
    };
    Operational.mount(
      root,
      { findings: [], patterns: [], buckets: [], total: 720, start: at, end: at + 320_000 },
      {
        current: () => true,
        scope: "dataset",
        captured: { filters: [] },
        request: async (_, args) => ({
          total: 720,
          signals: [{ ...signal, cases: cases.slice(args.offset || 0, (args.offset || 0) + 12) }],
        }),
      },
    );
  });
  const variant = host.locator(".op-row").first();
  await variant.click();
  await panel.getByText("Ausente", { exact: true }).waitFor();
  await panel.getByRole("button", { name: "Verificação", exact: true }).click();
  assert.match(await panel.locator(".op-raw").innerText(), /"id":51/);
  for (let i = 0; i < 12; i++) await panel.getByRole("button", { name: "Próxima operação" }).click();
  assert.match(await panel.locator(".op-case-pager").innerText(), /cópia 13/);
  assert.match(await panel.locator(".op-raw").innerText(), /"id":36/);
  await variant.press("Shift+F10");
  assert.ok(await panel.getByRole("button", { name: "Explorar operação" }).isVisible());
  assert.ok(await host.evaluate((e) => e.scrollWidth <= e.clientWidth + 1));
  await variant.press("Escape");
  assert.deepEqual(errors, []);
  console.log("Resumo, acordeão, registros, hipóteses, escolhas por fonte e teclado aprovados.");
  await page.close();
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const browser = await launchBrowser();
  try {
    await verifyOperationalSummary(browser);
  } finally {
    await browser.close();
  }
}
