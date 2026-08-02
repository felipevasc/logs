export default async (page) => {
  await page.waitForTimeout(1500);
  console.log("contagem inicial:", await page.locator("#context-summary").textContent());
  console.log("btn-merge desabilitado antes da carga?", await page.evaluate(() => document.querySelector("#btn-merge").disabled));

  // volta para a tela de abrir fonte e clica em Unir
  await page.evaluate(() => document.querySelector("#btn-open-artifact").click());
  await page.waitForTimeout(600);
  await page.evaluate(() => document.querySelector("#btn-merge").click());
  await page.waitForTimeout(2500);
  console.log("após unir:", await page.locator("#context-summary").textContent());
  console.log("artefato:", await page.locator("#artifact-select").evaluate((s) => s.options[s.selectedIndex]?.textContent));

  // abre a exploração: Fonte deve ter Firewall e o campo "regra" deve virar grupo
  await page.evaluate(() => document.querySelector("#btn-right-artifact").click());
  await page.waitForTimeout(2000);
  const groups = await page.evaluate(() =>
    [...document.querySelectorAll(".side .xnode-head .xnode-label")].map((n) => n.textContent));
  console.log("nós da árvore:", JSON.stringify(groups));
  const fonteItems = await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Fonte");
    return [...node.querySelectorAll(".facet-item")].map((i) =>
      `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim()}`);
  });
  console.log("Fonte:", JSON.stringify(fonteItems));
  await page.screenshot({ path: "output/f10-merge.png" });
};
