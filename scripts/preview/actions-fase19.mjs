export default async (page) => {
  await page.waitForTimeout(4000);
  const calls0 = await page.evaluate(() => ({ ...window.__mockCalls }));

  // árvore com contagens vivas funcionando
  const nivel = await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    return [...node.querySelectorAll(".facet-item")].map((i) =>
      `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim()}`);
  });
  console.log("Nível:", JSON.stringify(nivel));

  // aplica filtro → deve recalcular com UMA chamada tree_aggs
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
    [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
  });
  await page.waitForTimeout(1500);
  const calls1 = await page.evaluate(() => ({ ...window.__mockCalls }));
  console.log("chamadas antes:", JSON.stringify(calls0), "| depois do filtro:", JSON.stringify(calls1));

  const fonte = await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode")]
      .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Fonte");
    return [...node.querySelectorAll(".facet-item")].map((i) =>
      `${i.querySelector(".fv").textContent.trim()}=${i.querySelector(".fc")?.textContent.trim()}`);
  });
  console.log("Fonte (recorte Erro):", JSON.stringify(fonte));

  // memoização: ir ao Caso e voltar não deve disparar novas tree_aggs do dataset
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(1500);
  await page.evaluate(() => document.querySelector(".case-view-tabs .seg-btn[data-view='items']").click());
  await page.waitForTimeout(800);
  await page.evaluate(() => document.querySelector(".case-view-tabs .seg-btn[data-view='overview']").click());
  await page.waitForTimeout(800);
  const calls2 = await page.evaluate(() => ({ ...window.__mockCalls }));
  console.log("após navegar no Caso (memo):", JSON.stringify(calls2));

  // colpicker: toggle coluna sem refresh no backend
  await page.evaluate(() => document.querySelector("#btn-right-artifact").click());
  await page.waitForTimeout(800);
  const calls3a = await page.evaluate(() => ({ ...window.__mockCalls }));
  await page.click("#btn-colpicker");
  await page.waitForTimeout(300);
  await page.evaluate(() => {
    const item = [...document.querySelectorAll("#col-list .col-item")].find((n) => n.textContent.trim() === "Código");
    item?.querySelector("input")?.click();
  });
  await page.waitForTimeout(600);
  const calls3b = await page.evaluate(() => ({ ...window.__mockCalls }));
  console.log("colpicker: antes", JSON.stringify(calls3a), "depois", JSON.stringify(calls3b));
};
