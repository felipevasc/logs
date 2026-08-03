/* Valida a aba MCP revisada: 33 tools, 3 snippets, nota de clientes, refresh via source_summary. */
export default async function run(page) {
  await page.click("#btn-settings");
  await page.waitForTimeout(900);
  await page.screenshot({ path: "output/playwright/mcp-v2-modal.png" });

  const body = page.locator("#settings-modal .modal-body");
  await body.evaluate((el) => { el.scrollTop = 520; });
  await page.waitForTimeout(250);
  await page.screenshot({ path: "output/playwright/mcp-v2-snippets.png" });

  const toolCount = await page.locator("#settings-pane-mcp .mcp-tool").count();
  const badgeCount = await page.locator("#settings-pane-mcp .mcp-tool-badge").count();
  const fontes = await page.locator("#settings-pane-mcp .mcp-tool-group").first().innerText();
  console.log("tools:", toolCount, "mutantes:", badgeCount);
  console.log("grupo Fontes contém source_summary:", fontes.includes("source_summary"));

  // refresh da fonte via source_summary (label/contexto devem refletir o mock)
  await page.click("#settings-close");
  await page.waitForTimeout(300);
  await page.evaluate(() => window.__mockMcpEmit("source"));
  await page.waitForTimeout(2200);
  console.log("load-status:", await page.locator("#load-status").innerText());
  console.log("context-summary:", await page.locator("#context-summary").innerText());
  await page.screenshot({ path: "output/playwright/mcp-v2-refresh.png" });
}
