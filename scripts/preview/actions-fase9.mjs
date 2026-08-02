export default async (page) => {
  await page.waitForTimeout(1500);

  // abre a tela do Caso na visão Linha vertical
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(1200);
  await page.evaluate(() => document.querySelector(".case-view-tabs .seg-btn[data-view='vtimeline']").click());
  await page.waitForTimeout(800);
  const before = await page.locator("#analysis-list .vtl-row").count();
  console.log("vtimeline sem filtro:", before, "linhas");

  // aplica Fonte=Auth pela árvore do Caso
  await page.evaluate(() => {
    const node = [...document.querySelectorAll("#view-analysis .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Fonte");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Auth").click();
  });
  await page.waitForTimeout(1800);
  const after = await page.locator("#analysis-list .vtl-row").count();
  console.log("vtimeline com Fonte=Auth:", after, "linhas");

  // linha do tempo horizontal
  await page.evaluate(() => document.querySelector(".case-view-tabs .seg-btn[data-view='timeline']").click());
  await page.waitForTimeout(800);
  const dots = await page.locator("#analysis-list .tl-dot, #analysis-list .tl-bar").count();
  console.log("timeline horizontal: pontos/barras =", dots);
  await page.screenshot({ path: "output/f9-timeline.png" });
};
