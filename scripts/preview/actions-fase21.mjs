export default async (page) => {
  await page.waitForTimeout(4500);

  // cria campo com 2 regras (OU): a 1ª não pega nada, a 2ª pega
  await page.evaluate(() => {
    const row = state.rows.find((r) => r.fields?.usuario);
    state.currentDetailEv = row;
    openDeriveModal("fabio.nunes", "message", cellValue(row, "message"));
  });
  await page.waitForTimeout(300);
  await page.fill("#dv-name", "io_mb");
  // regra 1: formato que não existe nas linhas atuais
  await page.fill("#dv-rules .dv-rule:nth-child(1) .dv-rule-pattern", "payload=(\\d+)");
  // adiciona regra 2: formato real
  await page.click("#dv-add-rule");
  await page.fill("#dv-rules .dv-rule:nth-child(2) .dv-rule-pattern", "usuário ([\\w.]+), (\\d+) MB em (\\d+)");
  await page.fill("#dv-rules .dv-rule:nth-child(2) .dv-rule-template", "$2 MB ($1)");
  await page.waitForTimeout(400);
  const previews = await page.evaluate(() => ({
    r1: document.querySelector("#dv-rules .dv-rule:nth-child(1) .dv-rule-result").textContent,
    r2: document.querySelector("#dv-rules .dv-rule:nth-child(2) .dv-rule-result").textContent,
    final: document.querySelector("#dv-preview").textContent,
    rules: document.querySelectorAll("#dv-rules .dv-rule").length,
  }));
  console.log("previews:", JSON.stringify(previews));
  await page.screenshot({ path: "output/f21-rules.png", clip: { x: 380, y: 60, width: 760, height: 700 } });
  await page.click("#derive-save");
  await page.waitForTimeout(2000);

  // campo na árvore (Campos customizados)
  const inTree = await page.evaluate(() => {
    const head = [...document.querySelectorAll(".side .xnode-head")]
      .find((h) => h.querySelector(".xnode-label")?.textContent === "Campos customizados");
    return head ? [...head.closest(".xnode").querySelectorAll(".field-item span")].map((n) => n.textContent) : null;
  });
  console.log("Campos customizados:", JSON.stringify(inTree));

  // menu de contexto com seleção → opção de incrementar regra
  await page.evaluate(() => {
    const row = state.rows.find((r) => r.fields?.usuario);
    state.currentDetailEv = row;
    // simula seleção de texto no drawer
    const sel = "fabio.nunes";
    const items = [];
    // chama diretamente a lógica do menu: verifica que derivedFields alimenta o item
    items.push(...(state.derivedFields || []).map((f) => f.name));
    console.log("campos disponíveis p/ incrementar:", JSON.stringify(items));
  });
  console.log("derivedFields:", await page.evaluate(() => JSON.stringify((state.derivedFields || []).map((f) => ({ name: f.name, regras: f.rules.length })))));

  // abre edição com appendRule
  await page.evaluate(() => {
    const def = state.derivedFields.find((f) => f.name === "io_mb");
    openDeriveEdit(def, { appendRule: true });
  });
  await page.waitForTimeout(400);
  const editRules = await page.evaluate(() => document.querySelectorAll("#dv-rules .dv-rule").length);
  console.log("regras na edição (2 salvas + 1 nova):", editRules);
};
