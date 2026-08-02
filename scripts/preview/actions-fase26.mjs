export default async (page) => {
  await page.waitForTimeout(4500);

  // ---- 1) Analisar Trilha via menu de contexto (célula da tabela)
  await page.evaluate(() => {
    const tr = document.querySelector("#events-table tbody tr:nth-child(20)");
    tr.querySelector("td:nth-child(2)").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
  });
  await page.waitForTimeout(400);
  const menu = await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((b) => b.textContent.trim()));
  console.log("menu:", JSON.stringify(menu));
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("Analisar Trilha"))?.click());
  await page.waitForTimeout(1500);
  const trail = await page.evaluate(() => ({
    visible: !document.querySelector("#view-trail").hidden,
    rows: document.querySelectorAll("#trail-list .trail-row").length,
    center: document.querySelectorAll("#trail-list .trail-row.center").length,
    before: !document.querySelector("#trail-more-before").hidden,
    after: !document.querySelector("#trail-more-after").hidden,
  }));
  console.log("trilha:", JSON.stringify(trail));
  await page.screenshot({ path: "output/f26-trail.png" });

  // carregar mais antes
  await page.click("#trail-more-before");
  await page.waitForTimeout(800);
  console.log("linhas após carregar mais:", await page.locator("#trail-list .trail-row").count());

  // toggles
  await page.check("#trail-only-case");
  await page.waitForTimeout(800);
  console.log("só do caso:", await page.locator("#trail-list .trail-row").count(), "linhas");
  await page.check("#trail-unfiltered");
  await page.waitForTimeout(800);

  // volta para eventos
  await page.click("#btn-trail-back");
  await page.waitForTimeout(500);

  // ---- 2) salvar filtro e abas
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
  });
  await page.waitForTimeout(1500);
  // aba "+" salvar filtro
  await page.evaluate(() => document.querySelector(".filter-tab-add")?.click());
  await page.waitForTimeout(300);
  await page.fill("#np-val", "Só erros");
  await page.evaluate(() => document.querySelector("#np-apply, #name-pop .btn.primary")?.click());
  await page.waitForTimeout(1500);
  const tabs = await page.evaluate(() => [...document.querySelectorAll(".filter-tab")].map((t) => ({
    nome: t.querySelector(".filter-tab-name")?.textContent,
    contagens: t.querySelector(".filter-tab-counts")?.textContent,
    ativa: t.classList.contains("active"),
  })));
  console.log("abas:", JSON.stringify(tabs));
  await page.screenshot({ path: "output/f26-tabs.png", clip: { x: 0, y: 800, width: 1440, height: 100 } });

  // alterna: limpa filtros e reaplica pela aba
  await page.evaluate(() => { state.filters = []; filtersChanged(); });
  await page.waitForTimeout(1000);
  await page.evaluate(() => [...document.querySelectorAll(".filter-tab")].find((t) => t.textContent.includes("Só erros"))?.click());
  await page.waitForTimeout(1500);
  console.log("chips após aba:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));

  // ---- 3) persistência: reload volta para a trilha salva
  await page.evaluate(() => {
    const tr = document.querySelector("#events-table tbody tr:nth-child(10)");
    tr.querySelector("td:nth-child(2)").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("Analisar Trilha"))?.click());
  await page.waitForTimeout(1200);
  await page.reload({ waitUntil: "load" });
  await page.waitForTimeout(5000);
  const restored = await page.evaluate(() => ({
    view: document.querySelector("#view-trail").hidden ? "outra" : "trail",
    rows: document.querySelectorAll("#trail-list .trail-row").length,
    tabs: document.querySelectorAll(".filter-tab").length,
  }));
  console.log("após reload:", JSON.stringify(restored));
};
