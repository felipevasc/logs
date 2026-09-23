// Reproduce the checked-in offline browser dependencies from package-lock.json.
import { cpSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = resolve(root, "frontend/vendor/timeline-export");
mkdirSync(target, { recursive: true });
for (const [name, bundle] of [["html-to-image", "dist/html-to-image.js"], ["jspdf", "dist/jspdf.umd.min.js"]]) {
  const source = resolve(root, "node_modules", name);
  cpSync(resolve(source, bundle), resolve(target, name === "jspdf" ? "jspdf.umd.min.js" : "html-to-image.js"));
  cpSync(resolve(source, "LICENSE"), resolve(target, `${name}.LICENSE`));
  const { version } = JSON.parse(readFileSync(resolve(source, "package.json"), "utf8"));
  writeFileSync(resolve(target, `${name}.version`), `${version}\n`);
}
