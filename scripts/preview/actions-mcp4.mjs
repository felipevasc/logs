/* Simula clear_events via MCP: a UI deve voltar para a tela de fonte. */
export default async function run(page) {
  await page.evaluate(async () => {
    await window.__TAURI__.core.invoke("clear_events");
    window.__mockMcpEmit("source");
  });
  await page.waitForTimeout(1500);
  const sourceVisible = await page.locator("#view-artifact-source").isVisible();
  const workbar = await page.locator("#workbar-label").innerText();
  console.log("tela de fonte visível:", sourceVisible, "| workbar:", workbar);
  await page.screenshot({ path: "output/playwright/mcp-cleared.png" });
}
