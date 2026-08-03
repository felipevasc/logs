/* Abre o modal de Configurações (aba MCP) e valida o live-refresh via evento MCP. */
export default async function run(page) {
  // 1) modal de configurações com a aba MCP
  await page.click("#btn-settings");
  await page.waitForTimeout(900);
  await page.screenshot({ path: "output/playwright/mcp-modal.png" });

  // status carregado?
  const statusText = await page.locator("#settings-pane-mcp .mcp-status").first().innerText();
  console.log("status:", statusText.replace(/\n/g, " | "));
  const toolCount = await page.locator("#settings-pane-mcp .mcp-tool").count();
  const badgeCount = await page.locator("#settings-pane-mcp .mcp-tool-badge").count();
  console.log("tools:", toolCount, "mutantes:", badgeCount);

  // 2) live-refresh: fonte mudou via MCP
  await page.click("#settings-close");
  await page.waitForTimeout(300);
  const before = await page.locator("#result-count").innerText();
  await page.evaluate(() => window.__mockMcpEmit("source"));
  await page.waitForTimeout(2500);
  const after = await page.locator("#result-count").innerText();
  console.log("result-count antes:", before, "→ depois:", after);
  await page.screenshot({ path: "output/playwright/mcp-refresh.png" });

  // 3) evento de casos: estado recarregado sem erro
  await page.evaluate(() => window.__mockMcpEmit("cases"));
  await page.waitForTimeout(1200);
  console.log("cases ok:", await page.locator("#case-select").inputValue());
}
