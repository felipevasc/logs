export default async (page) => {
  await page.waitForTimeout(2500);

  // cria um campo derivado com template + filtro
  await page.evaluate(() => {
    const row = state.rows.find((r) => r.fields?.usuario);
    state.currentDetailEv = row;
    openDeriveModal("fabio.nunes", "message", cellValue(row, "message"));
  });
  await page.fill("#dv-name", "resumo_io");
  await page.fill("#dv-pattern", "usuário ([\\w.]+), (\\d+) MB em (\\d+)");
  await page.fill("#dv-template", "$2 MB por $1");
  await page.click("#derive-save");
  await page.waitForTimeout(1800);

  // nó Campos customizados na árvore
  const derivedNode = await page.evaluate(() => {
    const head = [...document.querySelectorAll(".side .xnode-head")]
      .find((h) => h.querySelector(".xnode-label")?.textContent === "Campos customizados");
    if (!head) return null;
    const node = head.closest(".xnode");
    return {
      meta: head.querySelector(".xnode-meta")?.textContent,
      items: [...node.querySelectorAll(".field-item span")].map((n) => n.textContent),
    };
  });
  console.log("nó customizados:", JSON.stringify(derivedNode));

  // edita pelo botão: modal abre preenchido, nome travado, excluir visível
  await page.evaluate(() => {
    document.querySelector(".side .field-adv-btn[title^='Editar campo']")?.click();
  });
  await page.waitForTimeout(400);
  const editState = await page.evaluate(() => ({
    name: document.querySelector("#dv-name").value,
    nameDisabled: document.querySelector("#dv-name").disabled,
    pattern: document.querySelector("#dv-pattern").value,
    template: document.querySelector("#dv-template").value,
    preview: document.querySelector("#dv-preview").textContent,
    deleteVisible: !document.querySelector("#derive-delete").hidden,
  }));
  console.log("edição:", JSON.stringify(editState));
  await page.screenshot({ path: "output/f15-edit.png", clip: { x: 380, y: 60, width: 760, height: 500 } });

  // exclui e confere que o nó some
  await page.click("#derive-delete");
  await page.waitForTimeout(1500);
  const gone = await page.evaluate(() =>
    ![...document.querySelectorAll(".side .xnode-head .xnode-label")].some((n) => n.textContent === "Campos customizados"));
  console.log("nó removido após excluir:", gone);
  await page.screenshot({ path: "output/f15-tree.png", clip: { x: 0, y: 190, width: 300, height: 400 } });
};
