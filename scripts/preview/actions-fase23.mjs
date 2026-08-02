export default async (page) => {
  await page.waitForTimeout(4500);

  // botão direito numa célula da tabela (usuario)
  await page.evaluate(() => {
    const td = [...document.querySelectorAll("#events-table tbody td")].find((c) => c.textContent.includes("."));
    td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 500, clientY: 400 }));
  });
  await page.waitForTimeout(400);
  const menuItems = await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((b) => b.textContent.trim()));
  console.log("menu da célula:", JSON.stringify(menuItems));
  await page.screenshot({ path: "output/f23-menu.png", clip: { x: 380, y: 300, width: 460, height: 420 } });

  // clica na nova opção e confere o resumo no modal
  await page.evaluate(() => {
    const item = [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("preenchido"));
    item?.click();
  });
  await page.waitForTimeout(500);
  console.log("resumo do modal:", await page.locator("#case-add-summary").textContent());
  // confirma e verifica o item no Caso
  await page.click("#case-add-confirm");
  await page.waitForTimeout(1500);
  const label = await page.evaluate(() => {
    const c = state.cases.cases.find((x) => x.id === state.cases.active);
    return c.items.at(-1)?.label;
  });
  console.log("item criado:", label);

  // menu da árvore (botão direito num valor de faceta)
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "usuario");
    node.querySelector(".facet-item").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 500 }));
  });
  await page.waitForTimeout(400);
  const treeMenu = await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((b) => b.textContent.trim()));
  console.log("menu da árvore:", JSON.stringify(treeMenu));
};
