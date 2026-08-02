export default async (page) => {
  await page.waitForTimeout(2500);
  await page.evaluate(() => document.querySelector("#btn-drive").click());
  await page.waitForTimeout(600);
  await page.evaluate(() => document.querySelector("#btn-load").click());
  // captura no meio da indexação simulada
  await page.waitForTimeout(1100);
  const mid = await page.evaluate(() => ({
    phase: document.querySelector("#load-phase").textContent,
    volume: document.querySelector("#load-volume").textContent,
    eta: document.querySelector("#load-eta").textContent,
  }));
  console.log("meio:", JSON.stringify(mid));
  await page.screenshot({ path: "output/f18-granular.png", clip: { x: 520, y: 280, width: 420, height: 360 } });
  await page.waitForTimeout(1400);
  const late = await page.evaluate(() => ({
    volume: document.querySelector("#load-volume").textContent,
    eta: document.querySelector("#load-eta").textContent,
  }));
  console.log("fim:", JSON.stringify(late));
  await page.waitForTimeout(4000);
};
