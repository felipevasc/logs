export default async (page) => {
  await page.waitForTimeout(4500);
  // abre o detalhe do primeiro evento com usuario
  await page.evaluate(() => {
    const row = state.rows.find((r) => r.fields?.usuario);
    openDetail(row.id);
  });
  await page.waitForTimeout(800);
  // simula seleção de texto e botão direito numa linha do drawer
  await page.evaluate(() => {
    window.getSelection = () => ({ toString: () => "fabio.nunes" });
    const row = document.querySelector("#drawer .kv-row[data-col='usuario']")
      || document.querySelector("#drawer .kv-row");
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 1150, clientY: 300 }));
  });
  await page.waitForTimeout(400);
  const menu = await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((b) => b.textContent.trim()));
  console.log("menu:", JSON.stringify(menu));
  await page.screenshot({ path: "output/f25-menu.png" });

  // clica em "contém" e confere o filtro
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("contém"))?.click());
  await page.waitForTimeout(1500);
  console.log("chips:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado:", await page.locator("#result-count").textContent());

  // limpa e testa "não contém"
  await page.evaluate(() => { state.filters = []; filtersChanged(); });
  await page.waitForTimeout(800);
  await page.evaluate(() => {
    const row = document.querySelector("#drawer .kv-row");
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 1150, clientY: 300 }));
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((b) => b.textContent.includes("não contém"))?.click());
  await page.waitForTimeout(1500);
  console.log("chips 2:", JSON.stringify(await page.locator("#chips .chip").allTextContents()));
  console.log("resultado 2:", await page.locator("#result-count").textContent());
};
