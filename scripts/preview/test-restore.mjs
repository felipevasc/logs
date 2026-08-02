import { chromium } from "playwright";

const b = await chromium.launch({ executablePath: process.env.LOCALAPPDATA + "/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe" });
const p = await b.newPage();
p.setDefaultTimeout(90000);
p.on("pageerror", (e) => console.log("[pageerror]", e.message));

await p.goto("http://127.0.0.1:4183/", { waitUntil: "domcontentloaded" });
await p.waitForFunction(() => typeof state !== "undefined" && state.loaded && document.querySelectorAll("#events-table tbody tr").length > 15, null, { timeout: 90000 });

await p.evaluate(() => {
  const td = document.querySelector("#events-table tbody tr:nth-child(10) td:nth-child(2)");
  td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
});
await p.waitForTimeout(600);
const menu = await p.evaluate(() => [...document.querySelectorAll(".ctx-item")].map((x) => x.textContent.trim()));
console.log("menu:", JSON.stringify(menu));
await p.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((x) => x.textContent.includes("Analisar Trilha"))?.click());
await p.waitForTimeout(2000);
console.log("trilha aberta:", await p.evaluate(() => !document.querySelector("#view-trail").hidden));

// salva filtro
await p.evaluate(() => {
  const node = [...document.querySelectorAll(".side .xnode")]
    .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
  [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
});
await p.waitForTimeout(2500);
await p.evaluate(() => document.querySelector(".filter-tab-add")?.click());
await p.waitForTimeout(800);
await p.fill("#np-val", "Só erros");
await p.evaluate(() => document.querySelector("#np-ok").click());
await p.waitForTimeout(2000);

// fecha e abre
await p.goto("http://127.0.0.1:4183/", { waitUntil: "domcontentloaded" });
await p.waitForFunction(() => typeof state !== "undefined", null, { timeout: 90000 });
await p.waitForTimeout(3000);
const r = await p.evaluate(() => ({
  trail: !document.querySelector("#view-trail").hidden,
  rows: document.querySelectorAll("#trail-list .trail-row").length,
  center: document.querySelectorAll("#trail-list .trail-row.center").length,
  tabs: [...document.querySelectorAll(".filter-tab .filter-tab-name")].map((t) => t.textContent),
  counts: [...document.querySelectorAll(".filter-tab .filter-tab-counts")].map((t) => t.textContent),
}));
console.log("reaberto:", JSON.stringify(r));
await b.close();
