export default async (page) => {
  // confere quais grupos de categoria aparecem na árvore do artefato
  const groups = await page.evaluate(() =>
    [...document.querySelectorAll(".side .xnode-head .xnode-label")].map((n) => n.textContent));
  console.log("nós da árvore:", JSON.stringify(groups));

  // rola até "anotacao" (1 valor + vazios) e clica em "(vazio)"
  const head = page.locator(".side .xnode-head", { hasText: "anotacao" });
  await head.scrollIntoViewIfNeeded();
  await page.waitForTimeout(300);
  await page.screenshot({ path: "output/f3-anotacao.png" });
  const node = page.locator(".side .xnode", { has: page.locator(".xnode-head", { hasText: "anotacao" }) });
  await node.locator(".facet-item", { hasText: "(vazio)" }).click();
  await page.waitForTimeout(1200);
  console.log("chips:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado:", await page.locator("#result-count").textContent());
  console.log("border:", await page.evaluate(() => document.body.classList.contains("filters-active")));
};
