const groupItems = (page, label, scope) => page.evaluate(([lbl, sc]) => {
  const roots = [...document.querySelectorAll(`.explore-tree-sync[data-tree-scope="${sc}"]`)];
  for (const root of roots) {
    const head = [...root.querySelectorAll(".xnode-head")]
      .find((h) => h.querySelector(".xnode-label")?.textContent === lbl);
    if (head) {
      const node = head.closest(".xnode");
      return [...node.querySelectorAll(".facet-item")].map((i) =>
        `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim() || "?"}`);
    }
  }
  return null;
}, [label, scope]);

export default async (page) => {
  await page.waitForTimeout(1500);
  // abre a tela do Caso (árvore do caso deve carregar contagens vivas)
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(2000);
  console.log("caso Nível:", JSON.stringify(await groupItems(page, "Nível", "case")));
  console.log("caso Fonte:", JSON.stringify(await groupItems(page, "Fonte", "case")));

  // filtra Fonte = Auth na árvore do caso e confere recorte nos demais grupos
  const fonteNode = page.locator("#view-analysis .explore-tree-sync .xnode", { has: page.locator(".xnode-head", { hasText: "Fonte" }) }).first();
  await fonteNode.locator(".facet-item", { hasText: "Auth" }).click();
  await page.waitForTimeout(1800);
  console.log("--- caso com Fonte=Auth ---");
  console.log("caso Fonte (mantém opções):", JSON.stringify(await groupItems(page, "Fonte", "case")));
  console.log("caso Nível (recorte Auth):", JSON.stringify(await groupItems(page, "Nível", "case")));
  console.log("caso Código (recorte Auth):", JSON.stringify(await groupItems(page, "Código", "case")));
  await page.screenshot({ path: "output/f4-caso-live.png" });
};
