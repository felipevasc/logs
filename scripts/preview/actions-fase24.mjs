export default async (page) => {
  await page.waitForTimeout(4500);

  // ---- 1) ts modal com regras OU
  await page.click("#events-table thead th .th-cfg");
  await page.waitForTimeout(500);
  await page.evaluate(() => { state.tsSources = ["linha"]; renderTsSources(); });
  // regra 1: não casa; adiciona regra 2 que casa
  await page.fill("#ts-rules .dv-rule:nth-child(1) .dv-rule-pattern", "epoch=(\\d+)");
  await page.click("#ts-add-rule");
  await page.fill("#ts-rules .dv-rule:nth-child(2) .dv-rule-pattern", "(\\w+\\.\\w+)");
  await page.waitForTimeout(400);
  const tsPrev = await page.evaluate(() => ({
    r1: document.querySelector("#ts-rules .dv-rule:nth-child(1) .dv-rule-result").textContent,
    r2: document.querySelector("#ts-rules .dv-rule:nth-child(2) .dv-rule-result").textContent,
  }));
  console.log("ts regras:", JSON.stringify(tsPrev));
  await page.click("#ts-test");
  await page.waitForTimeout(800);
  console.log("ts test linha 1:", (await page.locator("#ts-test-result .tr-row").first().textContent()).slice(0, 100));
  await page.evaluate(() => { document.querySelector("#ts-modal").hidden = true; });

  // ---- 2) arrastar coluna (simulado): move Código para antes de Nível
  const before = await page.evaluate(() => [...state.visibleCols]);
  await page.evaluate(() => {
    const ths = [...document.querySelectorAll("#events-table thead th")];
    const thCode = ths.find((t) => t.textContent.includes("Código"));
    const thNivel = ths.find((t) => t.textContent.includes("Nível"));
    thCode.ondragstart({ dataTransfer: { setData() {} } });
    thNivel.ondrop({ preventDefault() {}, dataTransfer: { getData: () => "code" } });
  });
  await page.waitForTimeout(300);
  const after = await page.evaluate(() => [...state.visibleCols]);
  console.log("colunas antes:", JSON.stringify(before), "depois:", JSON.stringify(after));
  const savedCols = await page.evaluate(() => {
    const c = state.cases.cases.find((x) => x.id === state.cases.active);
    return c.artifacts.find((a) => a.visibleCols)?.visibleCols || null;
  });
  console.log("colunas persistidas no artefato:", JSON.stringify(savedCols));

  // ---- 3) persistência multi-arquivo: recarrega a página e confere a pasta + source.paths
  await page.reload({ waitUntil: "load" });
  await page.waitForTimeout(4500);
  const persisted = await page.evaluate(() => {
    const c = state.cases.cases.find((x) => x.id === state.cases.active);
    const a = (c.artifacts || [])[0];
    return { artifacts: (c.artifacts || []).length, paths: a?.source?.paths || a?.source?.path || a?.path, cols: a?.visibleCols || null };
  });
  console.log("após reload:", JSON.stringify(persisted));
  await page.evaluate(() => document.querySelector("#btn-back-drive").click());
  await page.waitForTimeout(700);
  console.log("listagem após reload:", await page.evaluate(() =>
    [...document.querySelectorAll("#drive-table tbody tr")].map((tr) => tr.querySelector(".drive-name span")?.textContent)));
  await page.screenshot({ path: "output/f24-lista.png" });

  // ---- 4) remover artefato
  await page.evaluate(() => document.querySelector(".drive-remove")?.click());
  await page.waitForTimeout(1200);
  console.log("após remover:", await page.evaluate(() => ({
    rows: document.querySelectorAll("#drive-table tbody tr").length,
    empty: !document.querySelector("#drive-empty").hidden,
  })));
};
