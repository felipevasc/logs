import { readdir, mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { spawnManaged, waitManaged, previewEnvironment } from "./managed-process.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const selected = process.argv.slice(2);
// Historical restore probes have no assertions; the native test requires a
// Windows executable and runs separately against real IPC, without this mock.
const separateTests = new Set(["test-restore.mjs", "test-restore2.mjs", "test-native-desktop.mjs"]);
const available = (await readdir(new URL("./", import.meta.url)))
  .filter(name => /^test-[a-z0-9-]+\.mjs$/.test(name) && !separateTests.has(name)).sort();
const tests = selected.length ? selected : available;
for (const name of tests) if (!available.includes(name)) throw new Error(`Unknown regression test: ${name}`);
await mkdir(new URL("../../output/playwright/", import.meta.url), { recursive: true });
const environment = previewEnvironment();
const server = spawnManaged(["scripts/preview/serve.mjs", "0"], {
  cwd: root, stdout: "pipe", env: environment,
});
let active, interrupted = false, cleanupFailure = null, runError = null;
const stop = async () => {
  const outcomes = await Promise.allSettled([active?.stop(), server.stop()]);
  for (const outcome of outcomes) if (outcome.status === "rejected") {
    cleanupFailure = String(outcome.reason);
    console.error(cleanupFailure);
  }
};
for (const [signal, code] of [["SIGINT", 130], ["SIGTERM", 143]]) {
  process.on(signal, () => { interrupted = true; process.exitCode = code; void stop(); });
}
const results = [];
try {
  const url = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Preview startup timed out")), 15_000);
    let output = "";
    const done = (error, value) => { clearTimeout(timer); error ? reject(error) : resolve(value); };
    server.closed.then(({ code }) => done(new Error(`Preview server exited (${code})`)), error => done(error));
    server.stdout.on("data", chunk => {
      output += chunk;
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) done(null, match[0]);
    });
  });
  for (const test of tests) {
    if (interrupted || cleanupFailure) break;
    console.log(`Preview regression: ${test}`);
    const started = performance.now();
    let error = null;
    try {
      active = spawnManaged([`scripts/preview/${test}`, url], {
        cwd: root, env: { ...environment, PREVIEW_URL: url },
      });
      await waitManaged(active, 150_000);
    } catch (failure) {
      error = String(failure);
      if (failure.fatalCleanup) cleanupFailure = error;
      console.error(`${test}: ${error}`);
    } finally { active = null; }
    results.push({ test, passed: error === null, elapsedMs: Math.round(performance.now() - started), error });
  }
} catch (failure) {
  runError = String(failure);
  throw failure;
} finally {
  await stop();
  await writeFile(new URL("../../output/playwright/smoke-summary.json", import.meta.url), JSON.stringify({
    transport: "synthetic-preview", interrupted, cleanupFailure, runError,
    passed: !interrupted && !cleanupFailure && !runError && results.length === tests.length && results.every(result => result.passed),
    tests: results,
  }, null, 2) + "\n");
}
if (!interrupted && (cleanupFailure || results.some(result => !result.passed))) process.exitCode = 1;
