import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(root, "frontend");
const output = resolve(source, "dist");

rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });

for (const item of ["index.html", "app.js", "workspace.js", "styles.css", "workspace.css", "vendor"]) {
  cpSync(resolve(source, item), resolve(output, item), { recursive: true });
}
