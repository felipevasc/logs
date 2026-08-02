export default async (page) => {
  await page.waitForTimeout(1200);

  // expande o campo "usuario" dentro de Campos e aplica filtro "contém ana"
  const usuarioHead = page.locator(".side .xnode-field > .xnode-head", { hasText: "usuario" });
  await usuarioHead.scrollIntoViewIfNeeded();
  await usuarioHead.click();
  await page.waitForTimeout(300);
  const form = page.locator(".side .xnode-field", { has: page.locator(".xnode-head", { hasText: "usuario" }) }).locator(".field-filter");
  await form.locator("input").first().fill("ana");
  await form.locator("input").first().press("Enter");
  await page.waitForTimeout(1500);
  console.log("chips:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado:", await page.locator("#result-count").textContent());

  // expande o campo "tamanho" e aplica "entre 100 MB e 200 MB" (valor com unidade)
  const tamHead = page.locator(".side .xnode-field > .xnode-head", { hasText: "tamanho" });
  await tamHead.scrollIntoViewIfNeeded();
  await tamHead.click();
  await page.waitForTimeout(300);
  const tamForm = page.locator(".side .xnode-field", { has: page.locator(".xnode-head", { hasText: "tamanho" }) }).locator(".field-filter");
  await tamForm.locator("input").first().fill("100 MB");
  await tamForm.locator("input").nth(1).fill("200 MB");
  await tamForm.locator("input").nth(1).press("Enter");
  await page.waitForTimeout(1500);
  console.log("chips 2:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado 2:", await page.locator("#result-count").textContent());
  await tamHead.scrollIntoViewIfNeeded();
  await page.screenshot({ path: "output/f5-campos.png" });
};
