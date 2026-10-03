// Explicit file discovery works in both Windows cmd.exe and POSIX shells.
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
const root = new URL("../", import.meta.url);
const tests = ["scripts/preview/test-detail-fields.cjs", ...readdirSync(new URL("scripts/tests/", root))
  .filter(name => name.endsWith(".test.mjs")).sort().map(name => `scripts/tests/${name}`)];
const result = spawnSync(process.execPath, ["--test", ...tests], { cwd: fileURLToPath(root), stdio: "inherit" });
if (result.error) console.error(result.error);
process.exitCode = result.status ?? 1;
