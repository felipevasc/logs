// Parse every maintained script without executing browser or release code.
import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { spawnSync } from "node:child_process";

const root = new URL("../", import.meta.url);
let checked = 0;
async function check(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (["node_modules", "dist", "vendor", "output", ".git"].includes(entry.name)) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) { await check(path); continue; }
    if (!/\.(?:js|mjs|cjs)$/.test(path)) continue;
    const source = await readFile(path);
    const module = path.endsWith(".mjs") || ["mock-case-images.js", "mock-threats.js"].includes(entry.name);
    const result = spawnSync(process.execPath, ["--check", `--input-type=${module ? "module" : "commonjs"}`], {
      input: source, encoding: "utf8",
    });
    if (result.status !== 0) {
      console.error(relative(process.cwd(), path), result.stderr || result.error);
      process.exitCode = 1;
    }
    checked++;
  }
}
await check(new URL("frontend", root).pathname);
await check(new URL("scripts", root).pathname);
console.log(`Parsed ${checked} JavaScript files`);
