const groupItems = (page, label) => page.evaluate((lbl) => {
  const heads = [...document.querySelectorAll(".side .xnode-head")];
  const head = heads.find((h) => h.querySelector(".xnode-label")?.textContent === lbl);
  if (!head) return null;
  const node = head.closest(".xnode");
  return [...node.querySelectorAll(".facet-item")].map((i) =>
    `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim() || "?"}`);
}, label);

export default async (page) => {
  await page.waitForTimeout(1500); // aguarda contagens vivas
  console.log("Nível (sem filtro):", JSON.stringify(await groupItems(page, "Nível")));
  console.log("usuario (sem filtro):", JSON.stringify(await groupItems(page, "usuario")));
  console.log("tamanho (sem filtro):", JSON.stringify(await groupItems(page, "tamanho")));

  // 1) filtra Nível = Erro → demais grupos devem refletir o recorte
  const nivelNode = page.locator(".side .xnode", { has: page.locator(".xnode-head", { hasText: "Nível" }) });
  await nivelNode.locator(".facet-item", { hasText: "Erro" }).first().click();
  await page.waitForTimeout(1800);
  console.log("--- com Nível=Erro ---");
  console.log("Nível (mantém todas opções):", JSON.stringify(await groupItems(page, "Nível")));
  console.log("Fonte (recorte Erro):", JSON.stringify(await groupItems(page, "Fonte")));
  console.log("usuario (recorte Erro):", JSON.stringify(await groupItems(page, "usuario")));
  console.log("tamanho (recorte Erro):", JSON.stringify(await groupItems(page, "tamanho")));
  await page.screenshot({ path: "output/f4-recorte.png" });

  // 2) clica dois usuários seguidos → o segundo SUBSTITUI o primeiro
  const userNode = page.locator(".side .xnode", { has: page.locator(".xnode-head", { hasText: "usuario" }) });
  await userNode.locator(".facet-item").nth(0).click();
  await page.waitForTimeout(1500);
  await userNode.locator(".facet-item").nth(1).click();
  await page.waitForTimeout(1500);
  const chips = await page.locator("#chips .chip").allTextContents();
  console.log("chips após 2 cliques em usuario:", JSON.stringify(chips));
  console.log("usuario filters:", await page.evaluate(() =>
    JSON.stringify((window.__f = null, [...document.querySelectorAll("#chips .chip")].map(c => c.textContent)))));
  const total = await page.locator("#result-count").textContent();
  console.log("resultado:", total);
  await page.screenshot({ path: "output/f4-substituicao.png" });
};
