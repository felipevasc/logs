export default async (page) => {
  // 1) auto-navegação: após o carregamento o app deve estar na listagem (tabela visível)
  await page.waitForTimeout(2500);
  const onViz = await page.evaluate(() => !document.querySelector(".shell").hidden && !document.querySelector("#tab-table").hidden);
  console.log("auto-navegou para a listagem:", onViz);

  // 2) modal de campo derivado: template $N + filtro condicional
  await page.evaluate(() => {
    const row = state.rows.find((r) => r.fields?.usuario);
    state.currentDetailEv = row;
    openDeriveModal("fabio.nunes", "message", cellValue(row, "message"));
  });
  await page.waitForTimeout(300);
  await page.fill("#dv-name", "resumo_io");
  await page.fill("#dv-pattern", "usuário ([\\w.]+), (\\d+) MB em (\\d+)");
  await page.fill("#dv-template", "$2 MB por $1");
  await page.waitForTimeout(300);
  console.log("preview com template:", await page.locator("#dv-preview").textContent());
  // filtro que não é atendido pelo evento atual
  await page.selectOption("#dv-filter-col", "usuario");
  await page.selectOption("#dv-filter-op", "equals");
  await page.fill("#dv-filter-val", "usuario.inexistente");
  await page.waitForTimeout(300);
  console.log("preview filtro não atendido:", await page.locator("#dv-preview").textContent());
  // filtro atendido → salva e confere o campo na árvore
  const usuarioReal = await page.evaluate(() => (state.rows.find((r) => r.fields?.usuario) || {}).fields?.usuario);
  await page.fill("#dv-filter-val", usuarioReal);
  await page.waitForTimeout(300);
  console.log("preview filtro atendido:", await page.locator("#dv-preview").textContent());
  await page.screenshot({ path: "output/f14-derive.png", clip: { x: 380, y: 60, width: 760, height: 760 } });
  await page.click("#derive-save");
  await page.waitForTimeout(1800);
  const campos = await page.evaluate(() =>
    [...document.querySelectorAll(".side .field-row .field-item span")].map((n) => n.textContent));
  console.log("campo derivado na árvore:", campos.includes("resumo_io"));

  // 3) select de Caso: opções com fundo/cor legíveis
  const optStyle = await page.evaluate(() => {
    const o = document.querySelector("#case-select option");
    const cs = getComputedStyle(o);
    return { background: cs.backgroundColor, color: cs.color };
  });
  console.log("case-select option:", JSON.stringify(optStyle));
  await page.screenshot({ path: "output/f14-topbar.png", clip: { x: 950, y: 0, width: 490, height: 56 } });

  // 4) ts modal: Testar usa a concatenação real
  await page.click("#events-table thead th .th-cfg");
  await page.waitForTimeout(500);
  await page.evaluate(() => {
    state.tsSources = ["linha", "code"];
    renderTsSources();
  });
  await page.fill("#ts-regex", "(\\w+\\.\\w+)");
  await page.click("#ts-test");
  await page.waitForTimeout(800);
  const tr = await page.locator("#ts-test-result .tr-row").first().textContent().catch(() => "?");
  console.log("test_ts_config linha 1:", tr?.slice(0, 110));
};
