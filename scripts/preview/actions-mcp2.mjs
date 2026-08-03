/* Rola a aba MCP até a lista de ferramentas e alterna para o tema claro. */
export default async function run(page) {
  await page.click("#btn-settings");
  await page.waitForTimeout(900);
  await page.locator("#settings-modal .modal-body").evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await page.waitForTimeout(300);
  await page.screenshot({ path: "output/playwright/mcp-tools.png" });
  // tema claro
  await page.click("#settings-close");
  await page.click("#btn-theme");
  await page.click("#btn-settings");
  await page.waitForTimeout(600);
  await page.locator("#settings-modal .modal-body").evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await page.waitForTimeout(300);
  await page.screenshot({ path: "output/playwright/mcp-modal-light.png" });
}
