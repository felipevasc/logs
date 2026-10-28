/* Real Windows Tauri IPC acceptance for the labeled/nmap reader, the list-to-rows SQL and regex 'every occurrence'. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  writeFile,
  readFile,
  open,
  realpath,
} from "node:fs/promises";
import { createServer } from "node:net";
import { resolve } from "node:path";
import { chromium } from "playwright";
assert.equal(process.platform, "win32");
assert.ok(process.argv[2], "Pass the native executable");
const executable = await realpath(resolve(process.argv[2])),
  parent = resolve("output");
await mkdir(parent, { recursive: true });
const output = await mkdtemp(resolve(parent, "nmap-native-")),
  profile = resolve(output, "webview"),
  data = resolve(output, "data"),
  engine = resolve(output, "engine");
await mkdir(profile);
await mkdir(data);
const server = createServer();
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const port = server.address().port;
await new Promise((done) => server.close(done));
const stdout = await open(resolve(output, "stdout.log"), "w"),
  stderr = await open(resolve(output, "stderr.log"), "w");
const child = spawn(executable, [], {
  cwd: process.cwd(),
  windowsHide: true,
  stdio: ["ignore", stdout.fd, stderr.fd],
  env: {
    ...process.env,
    LOGINSIGHT_DATA_DIR: data,
    LOGINSIGHT_ENGINE_DIR: engine,
    WEBVIEW2_USER_DATA_FOLDER: profile,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-address=127.0.0.1 --remote-debugging-port=${port}`,
    RUST_BACKTRACE: "1",
  },
});
await stdout.close();
await stderr.close();
let browser, page, failure;
const results = { executable, output, checks: [] },
  pageErrors = [];
let exited = false;
child.on("exit", () => (exited = true));
const delay = (ms) => new Promise((done) => setTimeout(done, ms)),
  record = (name, detail = {}) => {
    results.checks.push({ name, ...detail });
    console.log(JSON.stringify({ name, ...detail }));
  };
async function powershell(script) {
  const source = Buffer.from(
    `$ErrorActionPreference='Stop';\n${script}`,
    "utf16le",
  ).toString("base64");
  const p = spawn(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", source],
    { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  let out = "",
    error = "";
  p.stdout.on("data", (s) => (out += s));
  p.stderr.on("data", (s) => (error += s));
  await new Promise((done, reject) => {
    const timer = setTimeout(() => {
      p.kill();
      reject(Error("Owned process helper timeout"));
    }, 15000);
    p.on("error", reject);
    p.on("exit", (code) => {
      clearTimeout(timer);
      code ? reject(Error(error)) : done();
    });
  });
  return out;
}
const ps = (s) => `'${s.replaceAll("'", "''")}'`;
async function cleanup() {
  // Roots are checked by executable; descendants and this unique WebView
  // profile are checked again by PID + creation time before being stopped.
  await powershell(
    `$all=@(Get-CimInstance Win32_Process);$ids=[Collections.Generic.HashSet[uint32]]::new();foreach($p in $all){if(($p.ProcessId -eq ${child.pid} -and [String]::Equals($p.ExecutablePath,${ps(executable)},[StringComparison]::OrdinalIgnoreCase)) -or ($p.Name -eq 'msedgewebview2.exe' -and $p.CommandLine -and $p.CommandLine.Contains(${ps(profile)}))){[void]$ids.Add($p.ProcessId)}};do{$changed=$false;foreach($p in $all){if($ids.Contains($p.ParentProcessId) -and $ids.Add($p.ProcessId)){$changed=$true}}}while($changed);foreach($p in $all){if($ids.Contains($p.ProcessId)){$live=Get-CimInstance Win32_Process -Filter ('ProcessId='+$p.ProcessId);if($live -and $live.CreationDate -eq $p.CreationDate){Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue}}}`,
  );
}
try {
  let connected = false;
  for (let i = 0; i < 180; i++) {
    if (exited) throw Error("Native application exited before CDP");
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`, {
        signal: AbortSignal.timeout(800),
      });
      if (response.ok) {
        connected = true;
        break;
      }
    } catch {}
    await delay(250);
  }
  assert.ok(connected, "WebView CDP ready");
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  for (let i = 0; i < 60 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages())[0];
    if (!page) await delay(100);
  }
  assert.ok(page);
  page.setDefaultTimeout(60000);
  page.on("pageerror", (e) => pageErrors.push(e.message));
  await page.waitForFunction(
    () =>
      window.WorkspaceContext?.ready &&
      !WorkspaceContext.changing &&
      !state.loadOverlay,
  );
  await page.evaluate(() => {
    window.localNative = async (request, filters = []) =>
      api(
        "local_analysis",
        { ...analyticsRequest(workspaceScope()), filters, request },
        { silent: true, latest: "native-local" },
      );
  });
  await page.waitForFunction(
    () =>
      WorkspaceContext.ready &&
      !WorkspaceContext.changing &&
      !WorkspaceContext.sourceBusy &&
      !state.loadOverlay &&
      !state.activeOperation,
  );

  const nmap = resolve(output, "nmap-grepable.gnmap");
  await writeFile(nmap, await readFile(resolve("src-tauri/testdata/corpus/nmap-grepable.gnmap")));
  // The first load can lose a race with the startup restore; the app reports false and a retry settles.
  let loaded = false;
  for (let attempt = 0; attempt < 4 && !loaded; attempt++) {
    await delay(1500);
    loaded = await page.evaluate((path) => loadData({ kind: "file", path, paths: [path], format: "auto" }), nmap);
  }
  assert.ok(loaded, "the source loads");
  await page.waitForFunction(() => state.loaded && !state.loadOverlay && state.total === 25);
  const sql = (text) => page.evaluate((s) => localNative({ action: "query", sql: s }), text);
  const hosts = await sql('SELECT "destination.ip" AS ip, "host.name" AS nome, "nmap.open_count" AS abertas, "nmap.ports_open" AS portas, "label.host" AS rotulo FROM events WHERE "destination.ip" IS NOT NULL ORDER BY id');
  assert.equal(hosts.total, "24");
  assert.ok(hosts.rows.every((r) => /^10.20./.test(r[0]) && r[4]), "address and the labeled field on every host line");
  record("nmap-hosts-read-as-labeled-records", { rows: hosts.total, first: hosts.rows[0].slice(0, 4) });
  const dated = await sql("SELECT count(*) FROM events WHERE timestamp IS NOT NULL");
  assert.equal(dated.rows[0][0], "0", "versions such as 2.4.62 are no longer read as dates");
  record("no-false-dates-from-versions");
  const ports = await sql(`WITH portas AS (
  SELECT "destination.ip" AS ip, unnest(json_extract("nmap.ports", '$[*]')) AS p
  FROM events WHERE "nmap.ports" IS NOT NULL
)
SELECT ip, try_cast(json_extract_string(p, '$.port') AS INTEGER) AS porta, json_extract_string(p, '$.state') AS estado, json_extract_string(p, '$.service') AS servico
FROM portas WHERE json_extract_string(p, '$.state') = 'open' ORDER BY ip, porta`);
  assert.ok(Number(ports.total) >= 20, "one row per open port: " + ports.total);
  assert.ok(ports.rows.every((r) => Number(r[1]) > 0 && r[2] === "open" && r[3] !== null));
  const items = await sql(`SELECT id, unnest(string_split("nmap.ports_open", ',')) AS item FROM events WHERE "nmap.ports_open" IS NOT NULL`);
  assert.ok(Number(items.total) >= Number(ports.total) - 5);
  record("sql-expands-lists-to-one-row-per-port", { ports: ports.total, items: items.total });
  const raw = await sql("SELECT raw FROM events WHERE \"nmap.ports_open\" LIKE '%,%' LIMIT 1");
  const map = { schema: 2, id: "todas", title: "todas", version: "1", rules: [{ id: "col-portas", description: "portas", selector: {}, priority: 0, fields: { portas: { from: ["porta"], type: "text" } }, extract: { kind: "regex", pattern: "(?<porta>[0-9]+)/open/", engine: "pcre2", case_sensitive: true, source: "raw", all: true } }] };
  const tried = await page.evaluate(({ map, text }) => localNative({ action: "map_try", map_json: JSON.stringify(map), texts: [text] }), { map, text: raw.rows[0][0] });
  const out = tried.results[0].trace.outputs.find((o) => o.field === "portas");
  assert.match(out.original, /^[0-9]+(, [0-9]+)+$/, "every open port of the host, joined: " + JSON.stringify(out));
  record("regex-all-occurrences-on-the-real-engine", { portas: out.original });
  assert.deepEqual(pageErrors, []);
  results.passed = true;

} catch (error) {
  failure = error;
  results.passed = false;
  results.error = String(error.stack || error);
  if (page)
    await page
      .screenshot({ path: resolve(output, "failure.png"), fullPage: true })
      .catch(() => {});
} finally {
  await browser?.close().catch(() => {});
  await cleanup();
  await writeFile(
    resolve(output, "results.json"),
    JSON.stringify(results, null, 2),
  );
  console.log(`Native local results: ${output}`);
}
if (failure) throw failure;
