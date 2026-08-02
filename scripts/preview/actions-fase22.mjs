export default async (page) => {
  await page.waitForTimeout(4500);
  // vai para a listagem de arquivos
  await page.evaluate(() => document.querySelector("#btn-back-drive").click());
  await page.waitForTimeout(700);
  const state1 = await page.evaluate(() => ({
    rows: [...document.querySelectorAll("#drive-table tbody tr")].map((tr) => ({
      cls: tr.className,
      nome: tr.querySelector(".drive-name span")?.textContent,
    })),
    caret: document.querySelector(".drive-caret-btn i")?.className,
  }));
  console.log("expandida:", JSON.stringify(state1, null, 1));
  await page.screenshot({ path: "output/f22-lista.png", clip: { x: 240, y: 120, width: 900, height: 300 } });

  // recolhe
  await page.evaluate(() => document.querySelector("#drive-table tr.drive-folder .drive-name")?.click());
  await page.waitForTimeout(300);
  const state2 = await page.evaluate(() => ({
    filhos: document.querySelectorAll("#drive-table tbody tr.drive-child").length,
    caret: document.querySelector(".drive-caret-btn i")?.className,
  }));
  console.log("recolhida:", JSON.stringify(state2));
};
