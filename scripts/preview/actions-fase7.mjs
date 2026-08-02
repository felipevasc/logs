export default async (page) => {
  await page.waitForTimeout(1200);
  // aplica um filtro com texto longo via formulário do campo Mensagem
  await page.evaluate(() => {
    const head = [...document.querySelectorAll(".side .xnode-field > .xnode-head")]
      .find((h) => h.querySelector(".xnode-label")?.textContent === "Mensagem");
    head.click();
  });
  await page.waitForTimeout(300);
  await page.evaluate(() => {
    const node = [...document.querySelectorAll(".side .xnode-field")]
      .find((n) => n.querySelector(".xnode-label")?.textContent === "Mensagem");
    const input = node.querySelector(".field-filter input");
    input.value = "certificado próximo do vencimento";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });
  await page.waitForTimeout(1200);
  // mede se o ✕ está visível dentro do chip
  const m = await page.evaluate(() => {
    const chip = document.querySelector(".side-chips .chip");
    const x = chip.querySelector(".x").getBoundingClientRect();
    const c = chip.getBoundingClientRect();
    return { chipRight: Math.round(c.right), xRight: Math.round(x.right), xVisible: x.right <= c.right + 0.5 && x.width > 0 };
  });
  console.log("chip:", JSON.stringify(m));
  await page.screenshot({ path: "output/f7-chip.png", clip: { x: 0, y: 100, width: 300, height: 160 } });
};
