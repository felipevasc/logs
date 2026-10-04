// Explicit file discovery works in both Windows cmd.exe and POSIX shells.
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
const root = new URL("../", import.meta.url);
const names = process.argv.includes("--extended")
  ? readdirSync(new URL("scripts/tests/", root)).filter(name => name.endsWith(".test.mjs")).sort()
  : JSON.parse(readFileSync(new URL("scripts/ci/essential-tests.json", root), "utf8"));
if (new Set(names).size !== names.length || names.some(name => !/^[a-z0-9-]+\.test\.mjs$/.test(name))) throw Error("Invalid essential test manifest");
const tests = ["scripts/preview/test-detail-fields.cjs", ...names.map(name => `scripts/tests/${name}`)];
const result = spawnSync(process.execPath, ["--test", ...tests], { cwd: fileURLToPath(root), stdio: "inherit" });
if (result.error) console.error(result.error);
process.exitCode = result.status ?? 1;
