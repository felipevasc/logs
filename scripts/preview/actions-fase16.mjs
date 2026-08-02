export default async (page) => {
  // 1) espera o boot e dispara uma nova carga para fotografar o overlay em voo
  await page.waitForTimeout(2500);
  await page.evaluate(() => document.querySelector("#btn-drive").click());
  await page.waitForTimeout(600);
  await page.evaluate(() => document.querySelector("#btn-load").click());
  await page.waitForTimeout(700);
  const overlayVisible = await page.evaluate(() => !document.querySelector("#load-overlay").hidden);
  const steps = await page.evaluate(() => [...document.querySelectorAll("#load-steps li")].map((li) => li.textContent));
  console.log("overlay visível:", overlayVisible, "| passos:", JSON.stringify(steps));
  await page.screenshot({ path: "output/f16-overlay.png" });
  await page.waitForTimeout(4000); // termina a carga

  // 2) drive view via botão Arquivos
  await page.evaluate(() => document.querySelector("#btn-drive").click());
  await page.waitForTimeout(800);
  const rows = await page.evaluate(() =>
    [...document.querySelectorAll("#drive-table tbody tr")].map((tr) => ({
      cls: tr.className, nome: tr.querySelector(".drive-name span")?.textContent,
      tipo: tr.querySelector(".drive-tipo")?.textContent,
      eventos: tr.querySelector(".drive-num")?.textContent,
    })));
  console.log("linhas da drive:", JSON.stringify(rows, null, 1));
  await page.screenshot({ path: "output/f16-drive.png" });

  // 3) recolhe/expande a pasta e abre via Detalhar
  await page.evaluate(() => document.querySelector(".drive-caret-btn")?.click());
  await page.waitForTimeout(300);
  const afterCollapse = await page.evaluate(() => document.querySelectorAll("#drive-table tbody tr.drive-child").length);
  console.log("filhos após recolher:", afterCollapse);
  await page.evaluate(() => [...document.querySelectorAll("#drive-table .drive-actions .btn")].find((b) => b.textContent === "Detalhar")?.click());
  await page.waitForTimeout(2500);
  const onViz = await page.evaluate(() => !document.querySelector(".shell").hidden);
  console.log("detalhar abriu a listagem:", onViz);

  // 4) workbar lúdica durante um filtro
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
  });
  await page.waitForTimeout(200);
  const bees = await page.evaluate(() => !document.querySelector("#workbar-bees").hidden);
  console.log("bees visíveis durante filtro:", bees);
  await page.screenshot({ path: "output/f16-bees.png", clip: { x: 0, y: 856, width: 700, height: 44 } });
};
