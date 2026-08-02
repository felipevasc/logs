export default async (page) => {
  await page.waitForTimeout(1200);

  // aplica um filtro no artefato
  const nivelNode = page.locator(".side .xnode", { has: page.locator(".xnode-head", { hasText: "Nível" }) });
  await nivelNode.locator(".facet-item", { hasText: "Erro" }).first().click();
  await page.waitForTimeout(1500);
  console.log("contexto (artefato):", await page.locator("#context-summary").textContent());

  // vai para a tela do Caso — barra deve mostrar o conjunto do Caso
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(1500);
  console.log("contexto (caso):", await page.locator("#context-summary").textContent());

  // inspetor de campo no contexto do Caso deve usar os eventos do Caso
  await page.evaluate(() => {
    const head = [...document.querySelectorAll("#view-analysis .xnode-field > .xnode-head")]
      .find((h) => h.querySelector(".xnode-label")?.textContent === "usuario");
    head.click(); // expande o formulário
  });
  await page.waitForTimeout(400);
  await page.evaluate(() => {
    const node = [...document.querySelectorAll("#view-analysis .xnode-field")]
      .find((n) => n.querySelector(".xnode-label")?.textContent === "usuario");
    node.querySelector(".field-filter .icon-btn[title='Inspecionar campo']").click();
  });
  await page.waitForTimeout(600);
  console.log("inspetor:", await page.locator("#drawer .kv .muted").first().textContent().catch(() => "?"));
  await page.screenshot({ path: "output/f6-caso.png" });
};
