export default async (page) => {
  await page.waitForTimeout(1500);
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(1200);
  await page.evaluate(() => document.querySelector(".case-view-tabs .seg-btn[data-view='vtimeline']").click());
  await page.waitForTimeout(800);

  // quantidade de linhas e grupos
  const stats = await page.evaluate(() => ({
    rows: document.querySelectorAll("#analysis-list .vtl-row").length,
    groups: [...document.querySelectorAll("#analysis-list .vtl-src")]
      .filter((n) => n.textContent.includes("eventos seguidos")).map((n) => n.textContent),
    bars: document.querySelectorAll("#analysis-list .vtl-bar").length,
    dotsInZone: document.querySelectorAll("#analysis-list .vtl-dot.in-zone").length,
  }));
  console.log(JSON.stringify(stats, null, 2));

  // rola até a janela do evento manual (barra) para o screenshot
  await page.evaluate(() => {
    const card = [...document.querySelectorAll("#analysis-list .vtl-card.manual")][0];
    card?.scrollIntoView({ block: "center" });
  });
  await page.waitForTimeout(400);
  await page.screenshot({ path: "output/f11-vtimeline.png" });

  // clica no grupo para abrir o popover com os eventos
  await page.evaluate(() => {
    const src = [...document.querySelectorAll("#analysis-list .vtl-src")]
      .find((n) => n.textContent.includes("eventos seguidos"));
    src?.closest(".vtl-row").scrollIntoView({ block: "center" });
    src?.closest(".vtl-row").click();
  });
  await page.waitForTimeout(500);
  console.log("pop items:", await page.locator(".tl-pop-item").count());
  await page.screenshot({ path: "output/f11-grupo-pop.png" });
};
