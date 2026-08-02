export default async (page) => {
  // 1) aplica filtro de nível "Erro" na árvore do artefato
  const nivelNode = page.locator(".side .xnode", { has: page.locator(".xnode-head", { hasText: "Nível" }) });
  await nivelNode.locator(".facet-item", { hasText: "Erro" }).first().click();
  await page.waitForTimeout(1200);
  console.log("border (explore):", await page.evaluate(() => document.body.classList.contains("filters-active")));
  await page.screenshot({ path: "output/f2-explore.png" });

  // 2) aba Painéis do artefato — gráficos devem refletir o filtro
  await page.locator("#tabbtn-dashboard").click();
  await page.waitForTimeout(2200);
  await page.screenshot({ path: "output/f2-paineis.png" });

  // 3) tela do Caso — sidebar com a árvore do caso
  await page.locator("#btn-right-case").click();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: "output/f2-caso.png" });

  // 4) Painéis do Caso — devem respeitar o filtro ativo
  await page.evaluate(() => document.querySelector("#btn-case-dashboard").click());
  await page.waitForTimeout(2500);
  console.log("dash-info:", await page.locator("#dash-info").textContent());
  await page.screenshot({ path: "output/f2-caso-paineis.png" });
};
