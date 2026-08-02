export default async (page) => {
  await page.waitForTimeout(1500);

  // 1) Campos não devem mais ter formulário inline nem caret
  const formCount = await page.locator(".side .field-filter").count();
  const fieldCaret = await page.locator(".side .xnode-field").count();
  console.log("forms inline:", formCount, "| nós expansíveis de campo:", fieldCaret);

  // 2) clica no funil do campo "tamanho" → popover de filtro avançado ancorado no ícone
  await page.evaluate(() => {
    const row = [...document.querySelectorAll(".side .field-row")]
      .find((r) => r.querySelector(".field-item span")?.textContent === "tamanho");
    row.scrollIntoView({ block: "center" });
    row.querySelector(".field-adv-btn").click();
  });
  await page.waitForTimeout(400);
  const pop = await page.evaluate(() => {
    const p = document.querySelector("#filter-pop");
    const r = p.getBoundingClientRect();
    return { visible: !p.hidden, x: Math.round(r.x), y: Math.round(r.y), col: document.querySelector("#fp-col").value, op: document.querySelector("#fp-op").value };
  });
  console.log("popover:", JSON.stringify(pop));
  await page.screenshot({ path: "output/f8-popover.png", clip: { x: 0, y: 300, width: 700, height: 400 } });

  // 3) aplica "entre 100 MB e 200 MB" pelo popover
  await page.selectOption("#fp-op", "between");
  await page.fill("#fp-val", "100 MB");
  await page.fill("#fp-val2", "200 MB");
  await page.click("#fp-apply");
  await page.waitForTimeout(1500);
  console.log("chips:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado:", await page.locator("#result-count").textContent());
};
