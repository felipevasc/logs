import { chromium } from "playwright";

const b = await chromium.launch({ executablePath: process.env.LOCALAPPDATA + "/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe" });
const p = await b.newPage();
p.setDefaultTimeout(90000);
p.on("pageerror", (e) => console.log("[pageerror]", e.message));

await p.goto("http://127.0.0.1:4183/", { waitUntil: "commit" });
await p.waitForTimeout(15000);

// salva um filtro
await p.evaluate(() => {
  const node = [...document.querySelectorAll(".side .xnode")]
    .find((n) => n.querySelector(".xnode-head .xnode-label")?.textContent === "Nível");
  [...node.querySelectorAll(".facet-item")].find((i) => i.querySelector(".fv").textContent === "Erro")?.click();
});
await p.waitForTimeout(2000);
await p.evaluate(() => document.querySelector(".filter-tab-add")?.click());
await p.waitForTimeout(600);
await p.fill("#np-val", "Só erros");
await p.evaluate(() => document.querySelector("#np-ok").click());
await p.waitForTimeout(2000);
const store1 = await p.evaluate(() => {
  const raw = JSON.parse(localStorage.getItem("__mockStore"));
  return { saved: raw?.cases?.[0]?.savedFilters?.length ?? "ausente" };
});
console.log("store antes do reload:", JSON.stringify(store1));

// abre a trilha
await p.evaluate(() => {
  const td = document.querySelector("#events-table tbody tr:nth-child(10) td:nth-child(2)");
  td.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 600, clientY: 400 }));
});
await p.waitForTimeout(500);
await p.evaluate(() => [...document.querySelectorAll(".ctx-item")].find((x) => x.textContent.includes("Analisar Trilha"))?.click());
await p.waitForTimeout(1500);

// fecha e abre
await p.goto("http://127.0.0.1:4183/", { waitUntil: "commit" });
await p.waitForTimeout(10000);
await p.waitForTimeout(10000);
const dbg = await p.evaluate(() => {
  const c = state.cases.cases.find((x) => x.id === state.cases.active);
  return { savedFilters: c?.savedFilters?.length, activeArtifactId: c?.activeArtifactId, artifacts: (c?.artifacts || []).length, trailSaved: !!c?.trail };
});
console.log("caso após reload:", JSON.stringify(dbg));
const r = await p.evaluate(() => ({
  trail: !document.querySelector("#view-trail").hidden,
  loaded: state.loaded,
  total: state.total,
  rows: document.querySelectorAll("#trail-list .vtl-row").length,
  center: document.querySelectorAll("#trail-list .trail-center").length,
  scroll: (() => { const s = document.querySelector(".trail-scroll"); return s ? s.scrollHeight > s.clientHeight : null; })(),
  tabs: [...document.querySelectorAll(".filter-tab .filter-tab-name")].map((t) => t.textContent),
  counts: [...document.querySelectorAll(".filter-tab .filter-tab-counts")].map((t) => t.textContent),
}));
console.log("reaberto:", JSON.stringify(r));
await p.screenshot({ path: "output/f29-restore.png" });
await b.close();
