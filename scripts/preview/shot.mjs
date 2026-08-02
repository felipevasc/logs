/* Captura de tela do preview. Uso: node shot.mjs <saida.png> [scriptDeAcoes.mjs]
   O script de ações opcional exporta default async (page) => {...}. */
import { chromium } from "playwright";
import { existsSync } from "node:fs";

const out = process.argv[2] || "shot.png";
const actionsPath = process.argv[3];

// usa o chromium já presente no cache, se a revisão esperada não existir
let executablePath;
const expected = chromium.executablePath();
if (!existsSync(expected)) {
  const base = expected.replace(/chromium-\d+.*$/, "");
  const alt = `${base}chromium_headless_shell-1217\\chrome-headless-shell-win64\\chrome-headless-shell.exe`;
  if (existsSync(alt)) executablePath = alt;
}
const browser = await chromium.launch(executablePath ? { executablePath } : {});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on("console", (m) => { if (m.type() === "error") console.error("[console]", m.text()); });
page.on("pageerror", (e) => console.error("[pageerror]", e.message));

await page.goto("http://127.0.0.1:4183/", { waitUntil: "load" });
await page.waitForTimeout(2500); // boot + auto-load do mock

if (actionsPath) {
  const { default: run } = await import(new URL(actionsPath, import.meta.url));
  await run(page);
}

await page.screenshot({ path: out, fullPage: false });
await browser.close();
console.log("ok:", out);
