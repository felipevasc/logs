import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const selected = process.argv.slice(2);
const tests = selected.length ? selected : ["test-navigation.mjs", "test-ui-scale.mjs", "test-workspace-context.mjs", "test-responsiveness.mjs", "test-journeys.mjs", "test-remote-sources.mjs", "test-analysis-workbench.mjs", "test-discovery.mjs", "test-updates.mjs", "test-explorer-timeline.mjs", "test-field-transform.mjs", "test-exclusion-archive.mjs", "test-case-references.mjs", "test-canonical-field-actions.mjs", "test-java-trace.mjs", "test-native-case-startup.mjs", "test-native-case-timeline.mjs", "test-context-menu.mjs", "test-waiting-visuals.mjs"];
for (const name of tests) {
  if (!/^test-[a-z0-9-]+\.mjs$/.test(name)) throw new Error(`Invalid preview test name: ${name}`);
}
await mkdir(new URL("../../output/playwright/", import.meta.url), { recursive: true });
const server = spawn(process.execPath, ["scripts/preview/serve.mjs", "0"], { cwd: root, stdio: ["ignore", "pipe", "inherit"] });
let active;
const stop = () => { active?.kill(); server.kill(); };
process.on("SIGINT", () => { stop(); process.exitCode = 130; });
process.on("SIGTERM", () => { stop(); process.exitCode = 143; });
try {
  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("Preview server startup timed out")), 15_000);
    let output = "";
    server.stdout.on("data", chunk => {
      output += chunk;
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timeout); resolve(match[0]); }
    });
    server.once("error", error => { clearTimeout(timeout); reject(error); });
    server.once("exit", code => { clearTimeout(timeout); reject(new Error(`Preview server exited (${code})`)); });
  });
  const failures = [];
  for (const test of tests) {
    console.log(`\nPreview regression: ${test}`);
    try { await new Promise((resolve, reject) => {
      active = spawn(process.execPath, [`scripts/preview/${test}`, url], { cwd: root, stdio: "inherit" });
      const timeout = setTimeout(() => { active.kill(); reject(new Error(`${test} timed out after 120s`)); }, 120_000);
      active.once("error", error => { clearTimeout(timeout); reject(error); });
      active.once("exit", code => { clearTimeout(timeout); code === 0 ? resolve() : reject(new Error(`${test} failed (${code})`)); });
    }); } catch (error) {
      failures.push({ test, message: String(error) });
      console.error(`Preview regression failed: ${test}: ${error.message}`);
    }
  }
  if (failures.length) {
    console.error(`\n${failures.length}/${tests.length} preview regressions failed: ${failures.map(f => f.test).join(", ")}`);
    process.exitCode = 1;
  }
} finally { stop(); }
