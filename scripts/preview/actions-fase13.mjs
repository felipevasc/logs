export default async (page) => {
  await page.waitForTimeout(2000);

  // 1) primeira coluna é Data/hora e tem o botão de configurar
  const firstTh = await page.locator("#events-table thead th").first().textContent();
  const hasCfg = await page.locator("#events-table thead th .th-cfg").count();
  console.log("primeira coluna:", JSON.stringify(firstTh), "| botão configurar:", hasCfg);

  // 2) o botão abre o modal de data/hora
  await page.click("#events-table thead th .th-cfg");
  await page.waitForTimeout(500);
  console.log("modal aberto:", await page.evaluate(() => !document.querySelector("#ts-modal").hidden));
  await page.screenshot({ path: "output/f13-tsmodal.png" });
  await page.evaluate(() => { document.querySelector("#ts-modal").hidden = true; });

  // 3) colpicker: data/hora marcada e desabilitada
  await page.click("#btn-colpicker");
  await page.waitForTimeout(300);
  const tsCb = await page.evaluate(() => {
    const item = [...document.querySelectorAll("#col-list .col-item")]
      .find((n) => n.textContent.trim() === "Data/hora");
    const cb = item?.querySelector("input");
    return cb ? { checked: cb.checked, disabled: cb.disabled } : null;
  });
  console.log("checkbox Data/hora:", JSON.stringify(tsCb));
};
