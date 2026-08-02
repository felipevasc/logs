export default async (page) => {
  await page.waitForTimeout(4500);

  // ---- 1) pasta: recolhe e expande pelo ícone
  await page.evaluate(() => document.querySelector("#btn-back-drive").click());
  await page.waitForTimeout(700);
  const filhos0 = await page.evaluate(() => document.querySelectorAll("#drive-table tr.drive-child").length);
  await page.evaluate(() => document.querySelector(".drive-caret-btn")?.click());
  await page.waitForTimeout(300);
  const filhos1 = await page.evaluate(() => document.querySelectorAll("#drive-table tr.drive-child").length);
  await page.evaluate(() => document.querySelector(".drive-caret-btn")?.click());
  await page.waitForTimeout(300);
  const filhos2 = await page.evaluate(() => document.querySelectorAll("#drive-table tr.drive-child").length);
  console.log("pasta filhos (aberta→fecha→abre):", filhos0, filhos1, filhos2);
  await page.screenshot({ path: "output/f27-pasta.png", clip: { x: 240, y: 120, width: 900, height: 260 } });

  // volta pra listagem de eventos
  await page.evaluate(() => [...document.querySelectorAll("#drive-table .drive-actions .btn")].find((b) => b.textContent === "Detalhar")?.click());
  await page.waitForTimeout(2500);

  // ---- 2) abas de filtro (np-val) — revisita
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
  });
  await page.waitForTimeout(1500);
  await page.evaluate(() => document.querySelector(".filter-tab-add")?.click());
  await page.waitForTimeout(400);
  const popVisible = await page.evaluate(() => !document.querySelector("#name-pop").hidden);
  console.log("name-pop visível:", popVisible);
  await page.fill("#np-val", "Só erros");
  await page.evaluate(() => document.querySelector("#np-apply")?.click() || document.querySelector("#name-pop .btn.primary")?.click());
  await page.waitForTimeout(1500);
  const tabs = await page.evaluate(() => [...document.querySelectorAll(".filter-tab")].map((t) => ({
    nome: t.querySelector(".filter-tab-name")?.textContent,
    contagens: t.querySelector(".filter-tab-counts")?.textContent,
    ativa: t.classList.contains("active"),
  })));
  console.log("abas:", JSON.stringify(tabs));

  // ---- 3) comentário vira coluna
  await page.evaluate(() => {
    const td = document.querySelector("#events-table tbody tr:nth-child(3) td:nth-child(2)");
    td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
  });
  await page.waitForTimeout(300);
  const menu = await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((b) => b.textContent.trim()));
  console.log("menu:", JSON.stringify(menu));
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("Adicionar comentário"))?.click());
  await page.waitForTimeout(400);
  await page.fill("#cm-text", "Suspeito — revisar origem");
  await page.click("#cm-save");
  await page.waitForTimeout(1000);
  const commentState = await page.evaluate(() => ({
    coluna: state.columns.includes("comentario"),
    visivel: state.visibleCols.includes("comentario") || null,
    header: [...document.querySelectorAll("#events-table thead th")].map((t) => t.textContent).includes("Comentário"),
  }));
  console.log("comentário:", JSON.stringify(commentState));

  // ---- 4) enviar todos visíveis (sem duplicar)
  await page.evaluate(() => {
    const td = document.querySelector("#events-table tbody tr:nth-child(3) td:nth-child(2)");
    td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("Enviar todos visíveis"))?.click());
  await page.waitForTimeout(500);
  await page.click("#case-add-confirm");
  await page.waitForTimeout(1500);
  const items1 = await page.evaluate(() => state.cases.cases.find((x) => x.id === state.cases.active).items.map((i) => i.label));
  console.log("itens no caso:", JSON.stringify(items1));
  // segunda vez: deve ignorar duplicados
  await page.evaluate(() => {
    const td = document.querySelector("#events-table tbody tr:nth-child(3) td:nth-child(2)");
    td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("Enviar todos visíveis"))?.click());
  await page.waitForTimeout(500);
  const dupMsg = await page.evaluate(() => document.querySelector("#case-add-modal").hidden ? "sem modal (tudo duplicado ou fluxo direto)" : "modal aberto");
  console.log("segunda tentativa:", dupMsg);
};
