export default async (page) => {
  await page.waitForTimeout(1500);
  console.log("inicial:", await page.locator("#context-summary").textContent());

  // abre a tela de fonte, escolhe arquivos (mock retorna 2), carrega
  await page.evaluate(() => document.querySelector("#btn-open-artifact").click());
  await page.waitForTimeout(500);
  await page.evaluate(() => document.querySelector("#btn-browse, .path-row .icon-btn").click());
  await page.waitForTimeout(300);
  console.log("input paths:", await page.locator("#file-path").inputValue());
  await page.evaluate(() => document.querySelector("#btn-load").click());
  await page.waitForTimeout(2500);
  console.log("após carregar 2 arquivos:", await page.locator("#context-summary").textContent());

  // exploração deve mostrar as duas fontes e o campo "regra"
  await page.evaluate(() => document.querySelector("#btn-right-artifact").click());
  await page.waitForTimeout(2000);
  const fonteItems = await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Fonte");
    return [...node.querySelectorAll(".facet-item")].map((i) =>
      `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim()}`);
  });
  console.log("Fonte:", JSON.stringify(fonteItems));
  await page.screenshot({ path: "output/f12-multifile.png" });
};
