/* Dispara os demais kinds do evento MCP e verifica ausência de erros. */
export default async function run(page) {
  // modal de códigos aberto deve recarregar o editor ao receber "codes"
  await page.click("#btn-codes");
  await page.waitForTimeout(400);
  await page.evaluate(() => window.__mockMcpEmit("codes"));
  await page.waitForTimeout(800);
  console.log("codes modal aberto:", await page.locator("#codes-modal").isVisible());
  await page.keyboard.press("Escape");
  for (const kind of ["derived", "ts_config", "formats"]) {
    await page.evaluate((k) => window.__mockMcpEmit(k), kind);
    await page.waitForTimeout(900);
    console.log(kind, "ok");
  }
  // fonte limpa via MCP: mock devolve perfis vazios? (aqui só confere que não quebra)
  await page.screenshot({ path: "output/playwright/mcp-kinds.png" });
}
