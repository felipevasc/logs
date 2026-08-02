export default async (page) => {
  // rola até o nó de faixas de "tamanho"
  const head = page.locator("#explore-tree .xnode-head", { hasText: "tamanho" });
  await head.scrollIntoViewIfNeeded();
  await page.waitForTimeout(300);
  await page.screenshot({ path: "output/arvore-ranges.png" });

  // clica na terceira faixa e confere o filtro aplicado
  const node = page.locator("#explore-tree .xnode", { has: page.locator(".xnode-head", { hasText: "tamanho" }) });
  await node.locator(".facet-item").nth(2).click();
  await page.waitForTimeout(1200);
  const chips = await page.locator("#chips .chip").allTextContents();
  const total = await page.locator("#result-count").textContent();
  console.log("chips:", JSON.stringify(chips), "| resultado:", total);
  await page.screenshot({ path: "output/arvore-filtro-faixa.png" });

  // filtra também um valor categórico (usuario) e depois remove a faixa
  const userNode = page.locator("#explore-tree .xnode", { has: page.locator(".xnode-head", { hasText: "usuario" }) });
  await userNode.locator(".facet-item").first().click();
  await page.waitForTimeout(1200);
  console.log("chips 2:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  await node.locator(".facet-item").nth(2).click(); // desliga a faixa
  await page.waitForTimeout(1200);
  console.log("chips 3:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));

  // recolhe "Campos" e "Nível"
  await page.locator("#explore-tree .xnode-head", { hasText: "Campos" }).click();
  await page.locator("#explore-tree .xnode-head", { hasText: "Nível" }).first().click();
  await page.waitForTimeout(300);
};
