export default async (page) => {
  // overlay no meio da carga com o novo tracker
  await page.waitForTimeout(2500);
  await page.evaluate(() => document.querySelector("#btn-drive").click());
  await page.waitForTimeout(600);
  await page.evaluate(() => document.querySelector("#btn-load").click());
  await page.waitForTimeout(2100); // pega passos já avançados
  await page.screenshot({ path: "output/f17-overlay.png", clip: { x: 520, y: 300, width: 420, height: 320 } });
  await page.waitForTimeout(4500);

  // workbar durante um filtro (spinner de anel)
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
  });
  await page.waitForTimeout(200);
  await page.screenshot({ path: "output/f17-workbar.png", clip: { x: 0, y: 856, width: 700, height: 44 } });
};
