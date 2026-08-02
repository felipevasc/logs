export default async (page) => {
  await page.waitForTimeout(4500); // boot + auto-load → deve estar na visão do artefato
  console.log("boot na visão do artefato:", await page.evaluate(() => !document.querySelector(".shell").hidden));

  // botão "Arquivos" (voltar) → listagem
  await page.evaluate(() => document.querySelector("#btn-back-drive").click());
  await page.waitForTimeout(600);
  const list = await page.evaluate(() => ({
    driveVisible: !document.querySelector("#drive-card").hidden,
    loadVisible: !document.querySelector("#source-load-card").hidden,
    rows: document.querySelectorAll("#drive-table tbody tr").length,
  }));
  console.log("listagem:", JSON.stringify(list));
  await page.screenshot({ path: "output/f20-lista.png" });

  // "Adicionar arquivos" → formulário de carga
  await page.evaluate(() => document.querySelector("#btn-drive-add").click());
  await page.waitForTimeout(400);
  console.log("modo carga:", await page.evaluate(() => !document.querySelector("#source-load-card").hidden));
  // voltar para a lista
  await page.evaluate(() => document.querySelector("#btn-source-back").click());
  await page.waitForTimeout(300);
  console.log("voltou para a lista:", await page.evaluate(() => !document.querySelector("#drive-card").hidden));

  // detalhar → visão do artefato; rail "Visualizar artefato" mantém a visão
  await page.evaluate(() => [...document.querySelectorAll("#drive-table .drive-actions .btn")].find((b) => b.textContent === "Detalhar")?.click());
  await page.waitForTimeout(2500);
  console.log("detalhar abriu:", await page.evaluate(() => !document.querySelector(".shell").hidden));
  await page.evaluate(() => document.querySelector("#btn-right-artifact").click());
  await page.waitForTimeout(300);
  console.log("rail mantém visão do artefato:", await page.evaluate(() => !document.querySelector(".shell").hidden));

  // ts: aplicar config → overlay com progresso
  await page.click("#events-table thead th .th-cfg");
  await page.waitForTimeout(400);
  await page.evaluate(() => { state.tsSources = ["linha"]; renderTsSources(); });
  await page.evaluate(() => document.querySelector("#ts-apply").click());
  await page.waitForTimeout(700);
  const tsOverlay = await page.evaluate(() => ({
    visible: !document.querySelector("#load-overlay").hidden,
    phase: document.querySelector("#load-phase").textContent,
    volume: document.querySelector("#load-volume").textContent,
    eta: document.querySelector("#load-eta").textContent,
  }));
  console.log("overlay ts:", JSON.stringify(tsOverlay));
  await page.screenshot({ path: "output/f20-ts.png", clip: { x: 520, y: 280, width: 420, height: 360 } });
  await page.waitForTimeout(3000);
};
