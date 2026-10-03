/* Windows-only acceptance of an actual packaged Tauri/WebView2 executable.
 * Usage: node scripts/preview/test-native-desktop.mjs path/to/loginsight.exe
 * Requires the Playwright dependency installed in scripts/preview. No mock IPC.
 * All logs, configuration, exports and WebView state remain under a unique
 * output/native-desktop-* directory; evidence is retained after the run.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, open, readFile, readdir, realpath, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

assert.equal(process.platform, "win32", "this test exercises Windows and WM_CLOSE");
assert.ok(process.argv[2], "pass the real Tauri executable as the first argument");
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const executable = await realpath(resolve(process.argv[2]));
const outputParent = resolve(repo, "output");
await mkdir(outputParent, { recursive: true });
const output = await mkdtemp(resolve(outputParent, "native-desktop-"));
const dataDir = resolve(output, "data"), profileDir = resolve(output, "webview"), engineDir = resolve(dataDir, "engine-v1");
await mkdir(dataDir);
await writeFile(resolve(dataDir, "system_codes.json"), JSON.stringify({ validation: {} }));
// A new installation must keep MCP disabled; do not inherit the user's config.
const fixture = resolve(output, "fixture.jsonl"), eventCount = 5000, epoch = 1_700_000_000_000;
const events = Array.from({ length: eventCount }, (_, i) => ({
  timestamp: epoch + i * 1000, source: `svc-${i % 6}`, code: String(100 + i % 11),
  level: i % 4 === 0 ? "error" : "info",
  message: `native row ${i}${i % 97 === 0 ? " rare-needle" : ""} token=fixture-secret-${i}`,
  token: `fixture-secret-${i}`, nested: { label: 'ação 東京, "quoted"', ordinal: i },
}));
// Keep the large integer as a JSON lexeme before encoding, never a JS Number.
const jwtPayload = '{"sub":"Pessoa Ω 東京","roles":["analista","auditor"],"id":9007199254740993}';
const jwt = [JSON.stringify({ alg: "HS256", typ: "JWT" }), jwtPayload, "synthetic-signature"]
  .map(value => Buffer.from(value, "utf8").toString("base64url")).join(".");
events[0].authorization = `Bearer ${jwt}`;
await writeFile(fixture, events.map(event => JSON.stringify(event)).join("\n") + "\n");
const syntheticEvidenceValues = [
  [`Bearer ${jwt}`, "[authorization sintética]"], [jwt, "[JWT sintético]"],
  ...jwt.split(".").map(part => [part, "[segmento JWT sintético]"]),
  [jwtPayload, "[payload JWT sintético]"], [JSON.stringify(jwtPayload).slice(1, -1), "[payload JWT sintético]"],
  ["Pessoa Ω 東京", "[sujeito JWT sintético]"],
  ["9007199254740993", "[ID sintético exato]"], ["9007199254740992", "[ID sintético arredondado]"],
].sort(([a], [b]) => b.length - a.length);
const evidenceText = value => syntheticEvidenceValues.reduce((text, [original, replacement]) =>
  text.replaceAll(original, replacement), String(value));
const evidenceJson = (_key, value) => typeof value === "string" ? evidenceText(value) : value;
function evidenceError(error) {
  // Do not retain AssertionError.actual/expected, which may contain complete
  // query events even after its displayed message has been sanitized.
  const safe = new Error(evidenceText(error?.message ?? error));
  safe.name = evidenceText(error?.name || "Error");
  if (error?.stack) safe.stack = evidenceText(error.stack);
  return safe;
}
const pageErrors = [], sessions = [], results = { executable, output, eventCount, checkpoints: [] };
let current = null, failure = null;
const delay = ms => new Promise(done => setTimeout(done, ms));
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const quotePS = value => `'${String(value).replaceAll("'", "''")}'`;
const checkpoint = (name, detail) => {
  results.checkpoints.push({ name, at: new Date().toISOString(), ...detail });
  console.log(JSON.stringify({ checkpoint: name, ...detail }, evidenceJson));
};

async function powershell(script) {
  const encoded = Buffer.from(`$ErrorActionPreference='Stop';\n${script}`, "utf16le").toString("base64");
  const helper = spawn("powershell.exe", ["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
    { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "", stderr = "";
  helper.stdout.on("data", chunk => { stdout += chunk; });
  helper.stderr.on("data", chunk => { stderr += chunk; });
  await new Promise((done, reject) => {
    const timer = setTimeout(() => { helper.kill(); reject(new Error("PowerShell helper timed out")); }, 15_000);
    helper.once("error", error => { clearTimeout(timer); reject(error); });
    helper.once("close", code => {
      clearTimeout(timer);
      if (code === 0) done(); else reject(new Error(`PowerShell failed (${code}): ${stderr.trim()}`));
    });
  });
  return stdout.trim();
}

async function freePort() {
  const server = createServer();
  await new Promise((done, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", done); });
  const { port } = server.address();
  await new Promise((done, reject) => server.close(error => error ? reject(error) : done()));
  return port;
}

async function ready(page) {
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing
    && !WorkspaceContext.sourceBusy && !state.loadOverlay && !state.activeOperation, null, { timeout: 120_000 });
}

async function launch(engineEnabled = true) {
  const port = await freePort();
  const number = sessions.length + 1;
  const stdoutPath = resolve(output, `launch-${number}-stdout.log`);
  const stderrPath = resolve(output, `launch-${number}-stderr.log`);
  const stdout = await open(stdoutPath, "wx"), stderr = await open(stderrPath, "wx");
  let child, session;
  try {
    child = spawn(executable, [], { cwd: repo, windowsHide: true, stdio: ["ignore", stdout.fd, stderr.fd], env: {
      ...process.env, LOGINSIGHT_DATA_DIR: dataDir, WEBVIEW2_USER_DATA_FOLDER: profileDir,
      RUST_BACKTRACE: "1", LOGINSIGHT_ENGINE: engineEnabled ? "1" : "0", LOGINSIGHT_ENGINE_DIR: engineDir,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-address=127.0.0.1 --remote-debugging-port=${port}`,
    } });
    session = { child, port, stdoutPath, stderrPath, browser: null, page: null, exited: false, error: null };
    sessions.push(session); current = session;
    child.once("error", error => { session.error = error; });
    session.exit = new Promise(done => child.once("exit", (code, signal) => {
      session.exited = true; session.exitCode = code; session.signal = signal; done();
    }));
  } finally {
    // The child owns inherited handles. Closing our copies does not discard its
    // output, and direct file descriptors avoid pipe backpressure during builds.
    await stdout.close(); await stderr.close();
  }
  checkpoint("launch", { number, engineEnabled, pid: child.pid, port, stdoutPath, stderrPath });
  const endpoint = `http://127.0.0.1:${port}`;
  let connected = false;
  for (let attempt = 0; attempt < 180; attempt++) {
    if (session.error) throw session.error;
    if (session.exited) throw new Error(`Application exited before CDP (${session.exitCode})`);
    try {
      const response = await fetch(`${endpoint}/json/version`, { signal: AbortSignal.timeout(800) });
      if (response.ok) { connected = true; break; }
    } catch {}
    await delay(250);
  }
  assert.ok(connected, "WebView2 did not expose the isolated loopback CDP endpoint");
  session.browser = await chromium.connectOverCDP(endpoint);
  for (let attempt = 0; attempt < 100 && !session.page; attempt++) {
    session.page = session.browser.contexts().flatMap(context => context.pages())[0];
    if (!session.page) await delay(100);
  }
  assert.ok(session.page, "native WebView page exists");
  const page = session.page;
  page.setDefaultTimeout(30_000);
  page.on("pageerror", error => pageErrors.push({ launch: sessions.length,
    message: evidenceText(error.message), stack: error.stack && evidenceText(error.stack) }));
  await page.waitForFunction(() => !!window.__TAURI__?.core?.invoke);
  await ready(page);
  assert.equal(await page.evaluate(() => flushCaseSaves()), true);
  // CDP can attach after the earliest startup scripts. Reload once with the
  // listener already attached so a complete frontend startup is observed.
  await page.reload({ waitUntil: "domcontentloaded" });
  await ready(page);
  const environment = await page.evaluate(async () => {
    const invoke = window.__TAURI__.core.invoke;
    const mcp = await invoke("mcp_status");
    for (let i = 0; i < 100; i++) {
      try {
        const resources = await invoke("resource_snapshot");
        if (resources.storage) return { mcp, resources };
      } catch (error) {
        if (!String(error).includes("Primeira coleta")) throw error;
      }
      await new Promise(done => setTimeout(done, 100));
    }
    throw new Error("Resource collector did not initialize");
  });
  assert.equal(environment.resources.app.pid, child.pid, "IPC belongs to the spawned app");
  assert.equal(resolve(environment.resources.storage.root).toLowerCase(), dataDir.toLowerCase(), "configuration must be isolated");
  assert.equal(environment.mcp.enabled, false, "MCP is opt-in on a clean installation");
  assert.equal(environment.mcp.running, false);
  checkpoint("native-ready", { launch: number, pid: child.pid, dataDir: environment.resources.storage.root });
  return session;
}

async function diagnostics(page) {
  const frontend = await page.evaluate(() => ({
    case: { id: activeCase()?.id, native: window.CaseEvidence?.active, context: window.AnalysisContexts?.identity() },
    loaded: state.loaded, total: state.total, loadOverlay: state.loadOverlay,
    operation: state.activeOperation && { kind: state.activeOperation.kind, cancelled: state.activeOperation.cancelled },
    sourcePublication: state.sourcePublication,
    footer: document.querySelector("#workbar")?.textContent,
  }));
  const backend = await page.evaluate(async () => {
    const bounded = async command => {
      let timer;
      try {
        return await Promise.race([api(command, {}, { silent: true }), new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("diagnostic IPC timed out")), 5000);
        })]);
      } catch (error) { return { error: String(error) }; }
      finally { clearTimeout(timer); }
    };
    const [mode, source, resources] = await Promise.all([bounded("engine_status"), bounded("source_summary"), bounded("resource_snapshot")]);
    return { mode, source, resources: resources.error ? resources : { app: resources.app, actions: resources.actions } };
  });
  return { frontend, backend };
}

async function nativeClose(session) {
  assert.ok(!session.exited && Number.isInteger(session.child.pid));
  const result = await powershell(`
    $targetProcess = Get-Process -Id ${session.child.pid};
    if (-not [String]::Equals($targetProcess.Path, ${quotePS(executable)}, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected process identity' }
    $targetProcess.Refresh();
    if (-not $targetProcess.CloseMainWindow()) { throw 'The owned application has no closeable main window' }
    'WM_CLOSE sent'
  `);
  assert.equal(result, "WM_CLOSE sent");
}

async function waitForExit(session) {
  let timer;
  try { await Promise.race([session.exit, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("Native close did not finish within 30 seconds")), 30_000);
  })]); } finally { clearTimeout(timer); }
  assert.equal(session.exitCode, 0, "normal native close must exit successfully");
  await session.browser.close().catch(() => {});
}

// Never kill by executable name. Discover only the live owned roots and their
// descendants, or WebView2 processes carrying this unique user-data directory.
// PID+creation time is rechecked immediately before stopping each process.
async function cleanupOwnedProcesses() {
  const roots = sessions.filter(session => !session.exited && session.child.pid).map(session => session.child.pid);
  const script = `
    $allProcesses = @(Get-CimInstance Win32_Process);
    $ownedIds = [System.Collections.Generic.HashSet[uint32]]::new();
    $rootIds = @(${roots.join(",")});
    foreach ($process in $allProcesses) {
      $ownedRoot = $rootIds -contains $process.ProcessId -and [String]::Equals($process.ExecutablePath, ${quotePS(executable)}, [StringComparison]::OrdinalIgnoreCase);
      $ownedWebview = $process.Name -eq 'msedgewebview2.exe' -and $process.CommandLine -and $process.CommandLine.IndexOf(${quotePS(profileDir)}, [StringComparison]::OrdinalIgnoreCase) -ge 0;
      if ($ownedRoot -or $ownedWebview) { [void]$ownedIds.Add($process.ProcessId) }
    }
    do {
      $changed = $false;
      foreach ($process in $allProcesses) {
        if ($ownedIds.Contains($process.ParentProcessId) -and $ownedIds.Add($process.ProcessId)) { $changed = $true }
      }
    } while ($changed);
    $stopped = @();
    foreach ($owned in $allProcesses | Where-Object { $ownedIds.Contains($_.ProcessId) }) {
      $live = Get-CimInstance Win32_Process -Filter ('ProcessId=' + $owned.ProcessId);
      if ($live -and $live.CreationDate -eq $owned.CreationDate) {
        Stop-Process -Id $owned.ProcessId -Force -ErrorAction SilentlyContinue;
        $stopped += [int]$owned.ProcessId;
      }
    }
    ConvertTo-Json -InputObject @($stopped) -Compress
  `;
  return JSON.parse(await powershell(script) || "[]");
}

const filters = (column, op, value, value2 = null) => [{ column, op, value, value2 }];
const scenarios = [
  { name: "all-first", filters: [], sortColumn: "timestamp", sortDir: "asc", offset: 0, limit: 25 },
  { name: "source-page", filters: filters("source", "equals_exact", "svc-2"), sortColumn: "timestamp", sortDir: "asc", offset: 30, limit: 25 },
  { name: "rare", filters: filters("message", "contains", "rare-needle"), sortColumn: "timestamp", sortDir: "desc", offset: 3, limit: 17 },
  { name: "code", filters: filters("code", "equals_exact", "104"), sortColumn: "source", sortDir: "desc", offset: 40, limit: 25 },
  { name: "range", filters: filters("timestamp", "between", String(epoch + 2500_000), String(epoch + 2502_000)), sortColumn: "timestamp", sortDir: "asc", offset: 0, limit: 100 },
  { name: "regex", filters: filters("message", "regex", "native row (123|456)"), sortColumn: "code", sortDir: "asc", offset: 0, limit: 100 },
  { name: "negative", filters: filters("source", "not_equals", "svc-2"), sortColumn: "source", sortDir: "asc", offset: 3000, limit: 25 },
  { name: "level", filters: filters("level", "equals", "Erro"), sortColumn: "level", sortDir: "desc", offset: 500, limit: 25 },
  { name: "empty", filters: filters("source", "equals_exact", "missing"), sortColumn: "timestamp", sortDir: "asc", offset: 0, limit: 25 },
];

async function queries(page) {
  return page.evaluate(async scenarios => {
    const output = {};
    for (const { name, ...args } of scenarios) output[name] = await api("query_events", { ...args, caseEvents: null, caseKey: null }, { silent: true });
    return output;
  }, scenarios);
}

async function detailInspection(page) {
  const show = id => page.evaluate(async id => {
    const event = await api("event_detail", { id, caseEvents: null, caseKey: null }, { silent: true });
    if (!event || event.id !== id) throw new Error("Native detail IPC returned the wrong event");
    showDetail(event);
  }, id);
  const authorization = page.locator('.detail-tree-row[data-col="authorization"]');
  const dialog = page.locator("dialog.value-inspector");
  try {
    await show(0);
    assert.equal(await page.locator("#dr-reveal").getAttribute("aria-pressed"), "false");
    assert.ok((await authorization.locator(".detail-tree-value").innerText()) === "[oculto]", "authorization is masked by default");
    assert.ok(!(await page.locator("#drawer").innerHTML()).includes(jwt), "JWT is absent from the masked drawer DOM");
    await page.locator("#dr-reveal").click();
    assert.equal(await page.locator("#dr-reveal").getAttribute("aria-pressed"), "true");
    assert.ok((await authorization.locator(".detail-tree-value").innerText()) === `Bearer ${jwt}`, "revealed value equals the original IPC value");
    await authorization.locator(".kv-inspect").click();
    await dialog.waitFor({ state: "visible" });
    await dialog.locator("details.value-node").evaluateAll(nodes => nodes.forEach(node => { node.open = true; }));
    const claims = await dialog.locator(".value-tree").innerText();
    for (const value of ["Pessoa Ω 東京", "analista", "auditor", "9007199254740993"]) {
      assert.ok(claims.includes(value), "decoded JWT claims preserve Unicode, roles and the exact integer");
    }
    assert.ok(!claims.includes("9007199254740992"), "the unsafe integer must never be rounded");
    const idLine = dialog.locator(".value-node-line").filter({ has: page.locator(".value-node-key", { hasText: /^id$/ }) });
    assert.ok((await idLine.locator(".value-node-preview").innerText()) === "9007199254740993", "decoded ID retains its exact original lexeme");
    await dialog.getByRole("button", { name: "Ocultar valor e subcampos", exact: true }).click();
    assert.equal(await dialog.locator(".value-node").count(), 0, "hiding removes decoded nodes from the DOM");
    const hiddenTree = await dialog.locator(".value-tree").innerHTML();
    for (const value of [jwt, "Pessoa Ω 東京", "9007199254740993"]) {
      assert.ok(!hiddenTree.includes(value), "hidden tree retains no token or decoded claims");
    }
    await dialog.getByRole("button", { name: "Fechar", exact: true }).click();
    await page.locator("#dr-reveal").click();
    assert.ok((await authorization.locator(".detail-tree-value").innerText()) === "[oculto]", "hiding masks authorization again");
    await page.locator("#dr-reveal").click();
    await page.locator("#dr-close").click();
    assert.equal(await page.evaluate(() => !detailRevealed && document.querySelector("#drawer").hidden), true, "closing resets revealed presentation while preserving canonical event identity");
    assert.ok(!(await page.locator("#drawer").innerHTML()).includes(jwt), "closing removes revealed values");
    await show(0);
    assert.equal(await page.locator("#dr-reveal").getAttribute("aria-pressed"), "false");
    assert.ok((await authorization.locator(".detail-tree-value").innerText()) === "[oculto]", "reopening starts with masked authorization");
    await page.locator("#dr-reveal").click();
    await authorization.locator(".kv-inspect").click();
    await dialog.waitFor({ state: "visible" });
    await show(1);
    assert.equal(await dialog.count(), 0, "changing the real event closes and clears the old inspector");
    assert.equal(await page.locator("#dr-reveal").getAttribute("aria-pressed"), "false");
    assert.equal(await authorization.count(), 0);
    assert.ok((await page.locator('.detail-tree-row[data-col="token"] .detail-tree-value').innerText()) === "[oculto]", "switching events masks token fields");
    assert.ok(!(await page.locator("#drawer").innerHTML()).includes(jwt));
    checkpoint("native-value-inspector", { realIpc: true, maskedByDefault: true, exactInteger: true,
      unicodeAndRoles: true, hideClearsTree: true, closeAndSwitchReset: true });
  } finally {
    // Keep revealed synthetic credentials out of screenshots, including failure
    // evidence. No copy buttons or clipboard APIs are invoked by this test.
    await page.evaluate(() => closeDrawer()).catch(() => {});
  }
}

async function exportsFor(page, mode) {
  const outputs = {};
  for (const format of ["jsonl", "csv"]) for (const mask of [false, true]) {
    const path = resolve(output, `${mode}-${mask ? "masked" : "plain"}.${format}`);
    assert.ok(relative(output, path) && !relative(output, path).startsWith(".."));
    const count = await page.evaluate(args => api("export_events", args, { silent: true }),
      { path, format, mask, filters: filters("source", "equals_exact", "svc-2"), caseEvents: null, caseKey: null });
    assert.equal(count, events.filter(event => event.source === "svc-2").length);
    const bytes = await readFile(path);
    assert.equal(bytes.toString("utf8").includes("fixture-secret-"), !mask, "export masking remains effective");
    outputs[`${format}-${mask}`] = bytes;
  }
  return outputs;
}

async function engineState(page, enabled) {
  const status = await page.evaluate(() => api("engine_status", {}, { silent: true }));
  assert.ok(status, "indexed source reports engine status");
  assert.equal(status.error, null, `native engine failed: ${status.error}`);
  assert.equal(status.state, enabled ? "ready" : "disabled");
  if (enabled) {
    assert.equal(status.baseReady, true);
    assert.equal(status.derivedReady, true);
    assert.equal(status.completedRows, eventCount);
    assert.equal(status.completedSegments, status.totalSegments);
  }
  return status;
}

async function engineManifests() {
  let entries;
  try { entries = await readdir(engineDir, { withFileTypes: true }); }
  catch (error) { if (error.code === "ENOENT") return []; throw error; }
  const manifests = [];
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith(".complete.json")) continue;
    const path = resolve(engineDir, entry.name), bytes = await readFile(path), metadata = await stat(path);
    manifests.push({ name: entry.name, bytes: bytes.length, modified: metadata.mtimeMs, sha256: hash(bytes) });
  }
  return manifests.sort((a, b) => a.name.localeCompare(b.name));
}

try {
  current = await launch(false);
  let page = current.page;
  await page.evaluate(() => { newCase("Native desktop acceptance"); });
  await ready(page);
  await page.evaluate(path => loadData({ kind: "file", path, paths: [path], format: "jsonl" }), fixture);
  await ready(page);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  assert.equal(await page.evaluate(() => CaseEvidence.active), true, "native Case session is active");
  await engineState(page, false);
  assert.deepEqual(await engineManifests(), [], "disabled baseline must not construct advanced stores");
  checkpoint("source-loaded", { eventCount, fixture });
  await detailInspection(page);
  const baseline = await queries(page);
  assert.equal(baseline["all-first"].total, eventCount);
  assert.deepEqual(baseline["all-first"].rows.map(row => row.id), Array.from({ length: 25 }, (_, i) => i));
  assert.equal(baseline["source-page"].total, 833);
  assert.deepEqual(baseline["source-page"].rows.map(row => row.id), Array.from({ length: 25 }, (_, i) => (30 + i) * 6 + 2));
  assert.equal(baseline.rare.total, 52);
  assert.equal(baseline.level.total, 1250);
  assert.deepEqual(baseline.range.rows.map(row => row.id), [2500, 2501, 2502]);
  assert.equal(baseline.empty.total, 0);
  checkpoint("baseline-queries", { scenarios: scenarios.length });
  const exportedOff = await exportsFor(page, "off");
  checkpoint("baseline-exports", { formats: Object.keys(exportedOff) });
  assert.equal(await page.evaluate(() => flushCaseSaves()), true);
  await nativeClose(current); await waitForExit(current);
  current = await launch(true); page = current.page;
  await page.evaluate(path => loadData({ kind: "file", path, paths: [path], format: "jsonl" }), fixture);
  await ready(page);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  const cold = await engineState(page, true);
  assert.deepEqual(await queries(page), baseline, "DuckDB/Tantivy must preserve canonical query results");
  const exportedOn = await exportsFor(page, "engine");
  const exportResults = {};
  for (const key of Object.keys(exportedOff)) {
    assert.deepEqual(exportedOn[key], exportedOff[key], `byte parity: ${key}`);
    exportResults[key] = { bytes: exportedOn[key].length, sha256: hash(exportedOn[key]) };
  }
  const manifests = await engineManifests();
  assert.ok(manifests.length > 0, "advanced engine publishes completed, checksummed stores");
  checkpoint("engine-parity", { scenarios: scenarios.length, cold, manifests, exports: exportResults });
  await page.screenshot({ path: resolve(output, "engine.png"), fullPage: true });

  await page.locator("#btn-resources").click();
  await page.waitForFunction(() => !!window.Resources?.snapshot() && Resources.snapshot().history.length >= 2);
  const first = await page.evaluate(() => Resources.snapshot());
  assert.notEqual(first.preview, true);
  assert.equal(first.app.pid, current.child.pid);
  assert.ok(first.app.residentBytes > 0 && first.host.totalMemoryBytes > 0);
  assert.ok(first.processes.some(process => process.pid === first.app.pid));
  assert.ok(first.processes.some(process => process.pid !== first.app.pid && /webview/i.test(process.name)), "native WebView descendants are included");
  assert.doesNotMatch(await page.locator("[data-resource-status]").innerText(), /simulad/);
  await page.waitForFunction(timestamp => Resources.snapshot().sampledAtMs > timestamp, first.sampledAtMs);
  await page.screenshot({ path: resolve(output, "resources.png"), fullPage: true });
  await page.locator("[data-resource-close]").click();
  // Closing the modal stops UI polling; the real backend continues its history.
  await delay(2200);
  const after = await page.evaluate(() => window.__TAURI__.core.invoke("resource_snapshot"));
  assert.ok(after.history.length > first.history.length);
  checkpoint("native-resources", { pid: first.app.pid, processCount: first.processes.length,
    hostMemoryBytes: first.host.totalMemoryBytes, historyBefore: first.history.length, historyAfter: after.history.length });

  assert.equal(await page.evaluate(() => flushCaseSaves()), true);
  await page.evaluate(() => {
    window.__nativeOriginalApi = api; window.__nativeFailedSaveAttempts = 0;
    api = async (command, ...args) => {
      if (command === "cases_save_view") { window.__nativeFailedSaveAttempts++; throw new Error("Isolated native test: rejected save"); }
      return window.__nativeOriginalApi(command, ...args);
    };
    activeCase().name = "rejected close edit";
    void saveCases();
  });
  await page.waitForFunction(() => window.__nativeFailedSaveAttempts > 0 && !caseSaveActive);
  const failuresBeforeClose = await page.evaluate(() => window.__nativeFailedSaveAttempts);
  await nativeClose(current);
  await page.waitForFunction(() => !caseSaveActive && caseSavesPending()
    && nativeEvidenceServices().session.hasUnconfirmedSave());
  assert.equal(await page.evaluate(() => window.__nativeFailedSaveAttempts), failuresBeforeClose,
    "close must preserve native retry semantics rather than silently retry a rejected receipt");
  await delay(500);
  assert.equal(current.exited, false, "a failed flush must leave the native window open");
  assert.equal(page.isClosed(), false);
  await page.screenshot({ path: resolve(output, "close-rejected.png"), fullPage: true });
  await page.evaluate(async () => {
    api = window.__nativeOriginalApi;
    await nativeEvidenceServices().session.retry();
    if (!await flushCaseSaves()) throw Error("Native retry did not restore a clean session");
  });
  const durable = await page.evaluate(() => {
    const c = activeCase(); c.name = "Persisted by real WM_CLOSE";
    c.manual.push({ id: "native-manual", createdAt: 1, name: "Native evidence", description: "ação 東京 preserved",
      start: 1_700_000_000_000, end: 1_700_000_001_000 });
    // Delay only this isolated page's transport so WM_CLOSE necessarily meets
    // an in-flight real disk write, then drain the existing production queue.
    api = async (command, ...args) => {
      if (command === "cases_save_view") await new Promise(done => setTimeout(done, 1500));
      return window.__nativeOriginalApi(command, ...args);
    };
    void saveCases();
    return { id: c.id, name: c.name, manual: c.manual.at(-1), priorRevision: state.cases.store.revision };
  });
  await page.waitForFunction(() => !!caseSaveActive);
  await nativeClose(current);
  await waitForExit(current);
  checkpoint("native-close", { rejectedSaveKeptWindowOpen: true, drainedRealWrite: true });

  current = await launch(); page = current.page;
  const restored = await page.evaluate(() => ({ id: activeCase()?.id, name: activeCase()?.name,
    manual: activeCase()?.manual.find(item => item.id === "native-manual"), native: CaseEvidence.active,
    revision: state.cases.store.revision }));
  assert.equal(restored.id, durable.id); assert.equal(restored.name, durable.name);
  assert.deepEqual(restored.manual, durable.manual); assert.equal(restored.native, true);
  assert.ok(BigInt(restored.revision) > BigInt(durable.priorRevision));
  const disk = await page.evaluate(() => window.__TAURI__.core.invoke("cases_load_view"));
  assert.equal(disk.cases.find(item => item.id === durable.id).name, durable.name);
  // An application reopen may restore a Case page instead of the dataset page.
  await page.evaluate(path => loadData({ kind: "file", path, paths: [path], format: "jsonl" }), fixture);
  await ready(page);
  await page.evaluate(() => WorkspaceContext.setScope("dataset", { page: "explore", animate: false }));
  const reopened = await engineState(page, true);
  assert.deepEqual(await engineManifests(), manifests, "restart reuses immutable completed stores without replacing manifests");
  assert.deepEqual(await queries(page), baseline, "process restart preserves cached index results");
  await page.screenshot({ path: resolve(output, "reopened.png"), fullPage: true });
  checkpoint("persistence-and-reopen", { restored, engine: reopened, immutableManifestsReused: true });
  assert.deepEqual(pageErrors, [], "native frontend pageerrors must be zero");
  await nativeClose(current); await waitForExit(current);
} catch (error) {
  failure = evidenceError(error);
  if (current?.page && !current.page.isClosed()) {
    // Also cover these surfaces if a UI bug prevented closeDrawer cleanup.
    await current.page.screenshot({ path: resolve(output, "failure.png"), fullPage: true,
      mask: [current.page.locator("#drawer"), current.page.locator("dialog.value-inspector")] }).catch(() => {});
    try { results.diagnostics = await diagnostics(current.page); }
    catch (diagnosticError) { results.diagnosticError = String(diagnosticError); }
  }
} finally {
  try { results.cleanupStoppedPids = await cleanupOwnedProcesses(); }
  catch (error) { results.cleanupError = evidenceText(error); failure ||= evidenceError(error); }
  for (const session of sessions) await session.browser?.close().catch(() => {});
  results.pageErrors = pageErrors;
  results.passed = !failure && pageErrors.length === 0;
  if (failure) results.error = { message: failure.message, stack: failure.stack };
  await writeFile(resolve(output, "results.json"), JSON.stringify(results, evidenceJson, 2));
  console.log(JSON.stringify({ passed: results.passed, output, pageErrors: pageErrors.length }));
}
if (failure) throw failure;
assert.deepEqual(pageErrors, []);
