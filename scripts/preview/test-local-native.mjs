/* Real Windows Tauri IPC acceptance for the integrated local exploration. */
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
const output = await mkdtemp(resolve(parent, "local-native-")),
  profile = resolve(output, "webview"),
  data = resolve(output, "data"),
  engine = resolve(output, "engine");
await mkdir(profile);
await mkdir(data);
const fixture = resolve(output, "events.jsonl"),
  textFile = resolve(output, "direct.log"),
  directLines = Math.max(
    200000,
    Math.min(
      5000000,
      Math.ceil((Number(process.env.LOCAL_NATIVE_LINES) || 200000) / 10000) *
        10000,
    ),
  );
const events = Array.from({ length: 401 }, (_, i) => ({
  timestamp: 1700000000000 + i * 1000,
  source: "Microsoft-Windows-Sysmon",
  code: i < 2 ? "1" : "3",
  level: i % 2 ? "Erro" : "Informação",
  message: `user=ana sequence=${i}`,
  Channel: "Microsoft-Windows-Sysmon/Operational",
  Computer: "host-a",
  ProcessGuid: i === 0 ? "root-guid" : "child-guid",
  ParentProcessGuid: i === 1 ? "root-guid" : "",
  ProcessId: i === 0 ? "100" : "200",
  ParentProcessId: i === 1 ? "100" : "0",
  Image: i === 0 ? "cmd.exe" : "powershell.exe",
  CommandLine: i === 0 ? "cmd /c script" : "powershell -File script.ps1",
  User: "ana",
  "host.boot.id": "boot-a",
}));
await writeFile(fixture, events.map(JSON.stringify).join("\n") + "\n");
const text = await open(textFile, "w");
for (let block = 0; block < directLines / 10000; block++) {
  let chunk = "";
  for (let n = 0; n < 10000; n++) {
    const i = block * 10000 + n;
    chunk += `${i} ${"padding ".repeat(8)} ${i === directLines - 5 ? "needle ação" : "nothing"}\n`;
  }
  await text.write(chunk);
}
await text.close();
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
  const sourceBefore = await page.evaluate(() => JSON.stringify(state.currentArtifact?.source ?? null));
  await page.evaluate((path) => FileViewer.open(path), textFile);
  assert.match(await page.locator("#lx-source").textContent(), /padding/);
  assert.equal(await page.locator("#ws-title").textContent(), "Visualizar arquivos");
  const viewerHead = await page.locator("#fv-head").textContent();
  assert.ok(viewerHead.includes("direct.log") && viewerHead.includes("Tamanho") && viewerHead.includes("Leitura"));
  assert.equal(await page.evaluate(() => JSON.stringify(state.currentArtifact?.source ?? null)), sourceBefore, "the viewer never loads the file into the analysis");
  await page.locator("#lx-pause").click();
  record("first-file-opens-in-the-real-file-viewer-without-loading-the-analysis");
  const openStarted = performance.now();
  const opened = await page.evaluate(
    (path) =>
      localNative({ action: "text", request: { action: "open", path } }),
    textFile,
  );
  assert.equal(opened.status.indexComplete, false);
  assert.ok(BigInt(opened.status.indexedBytes) < BigInt(opened.status.bytes));
  record("direct-open-before-full-index", {
    indexed: opened.status.indexedBytes,
    total: opened.status.bytes,
    elapsedMs: Math.round(performance.now() - openStarted),
    lines: directLines,
  });
  const searchStarted = performance.now();
  const direct = await page.evaluate(async (session) => {
    const pattern = {
      engine: "pcre2",
      text: "(?<=needle )ação",
      caseSensitive: true,
    };
    await localNative({
      action: "text",
      request: { action: "search", session, pattern },
    });
    for (let n = 0; n < 2000; n++) {
      const result = await localNative({
        action: "text",
        request: { action: "step", session, matchesOnly: true },
      });
      if (result.status.searchComplete) return { ...result, waves: n + 1 };
    }
    throw Error("Search never completed");
  }, opened.status.session);
  assert.equal(direct.status.matches, 1);
  assert.ok(direct.lines[0].text.includes("ação"));
  assert.ok(direct.lines[0].spans.length);
  assert.equal(direct.lines[0].line, directLines - 4);
  record("direct-pcre2-late-match", {
    matches: 1,
    lines: directLines,
    elapsedMs: Math.round(performance.now() - searchStarted),
    waves: direct.waves,
  });
  await page.evaluate(async (path) => {
    if (
      !(await loadData({ kind: "file", path, paths: [path], format: "jsonl" }))
    )
      throw Error("Load failed");
  }, fixture);
  await page.waitForFunction(
    () => state.loaded && !state.loadOverlay && state.total === 401,
  );
  const rebound = await page.evaluate(
    ({ session, line }) =>
      localNative({
        action: "text",
        request: { action: "rebind", session, line },
      }),
    { session: opened.status.session, line: directLines - 10 },
  );
  assert.equal(rebound.status.matches, 1);
  assert.equal(rebound.lines[0].line, directLines - 10);
  record("direct-index-and-position-survive-source-preparation");
  const sql = await page.evaluate(() =>
    localNative({
      action: "query",
      sql: "SELECT id,event_ref,9007199254740993::BIGINT AS exact_integer FROM events ORDER BY id",
    }),
  );
  assert.equal(sql.total, "401");
  assert.equal(sql.rows[0][2], "9007199254740993");
  assert.equal(sql.rows.length, 80);
  const last = await page.evaluate(
    (result) =>
      localNative({ action: "query_page", result, offset: 400, limit: 80 }),
    sql.result,
  );
  assert.equal(last.rows.length, 1);
  assert.equal(last.rows[0][0], "400");
  const exportPath = resolve(output, "complete.jsonl");
  assert.equal(
    await page.evaluate(
      ({ result, path }) =>
        localNative({ action: "query_export", result, path, format: "jsonl" }),
      { result: sql.result, path: exportPath },
    ),
    401,
  );
  assert.equal(
    (await readFile(exportPath, "utf8")).trim().split("\n").length,
    401,
  );
  record("indexed-sql-exact-pagination-and-native-complete-export");
  const rawRows = await page.evaluate(() =>
    localNative({
      action: "query",
      sql: "SELECT id,raw FROM events WHERE id IN(0,400) ORDER BY id",
    }),
  );
  assert.equal(rawRows.rows.length, 2);
  assert.equal(JSON.parse(rawRows.rows[0][1]).message, "user=ana sequence=0");
  assert.equal(JSON.parse(rawRows.rows[1][1]).message, "user=ana sequence=400");
  record("indexed-sql-reads-original-raw-lazily");
  const cte = await page.evaluate(() =>
    localNative({
      action: "query",
      sql: "WITH counts AS (SELECT level,count(*) AS n FROM events GROUP BY level) SELECT *,rank() OVER(ORDER BY n DESC) AS ranking FROM counts",
    }),
  );
  assert.equal(cte.rows.length, 2);
  for (const forbidden of [
    "SELECT * FROM read_csv('private')",
    "SELECT * FROM evn",
    "SELECT li_raw(id,1) FROM events",
    "WITH evn AS(SELECT * FROM evn) SELECT * FROM evn",
  ]) {
    const blocked = await page.evaluate(async (sql) => {
      try {
        await localNative({ action: "query", sql });
        return false;
      } catch {
        return true;
      }
    }, forbidden);
    assert.equal(blocked, true);
  }
  record("cte-window-and-external-sql-refusal");
  const regex = await page.evaluate(() =>
    localNative({
      action: "regex",
      pattern: {
        engine: "pcre2",
        text: "(?<=user=)(?<user>\\w+)\\s+\\k<user>",
        caseSensitive: true,
      },
      text: "😀 user=ana ana",
    }),
  );
  assert.equal(regex.spans[0].captures.user, "ana");
  await page.evaluate(async () => {
    await localNative({
      action: "save_field",
      name: "extracted_user",
      source: "message",
      definition: {
        kind: "captures",
        pattern: {
          engine: "pcre2",
          text: "(?<=user=)(?<user>\\w+)",
          caseSensitive: true,
        },
        capture: "user",
      },
    });
    await loadDerivedFields();
  });
  const capture = await page.evaluate(() =>
    localNative({
      action: "query",
      sql: "SELECT extracted_user,count(*) AS n FROM events GROUP BY extracted_user",
    }),
  );
  assert.equal(capture.rows[0][0], "ana");
  assert.equal(capture.rows[0][1], "401");
  record("pcre2-capture-persisted-and-queryable");
  const preview = await page.evaluate(async () => {
    const map = await localNative({ action: "map_template" });
    const result = await localNative({
      action: "map_preview",
      map_json: JSON.stringify(map),
      limit: 3,
    });
    await localNative({
      action: "save_map",
      name: "norm",
      map_json: JSON.stringify(map),
    });
    await loadDerivedFields();
    return result;
  });
  assert.equal(preview.affected, 401);
  const enriched = await page.evaluate(() =>
    localNative({
      action: "query",
      sql: 'SELECT id,event_ref,try_cast("norm.process.pid" AS BIGINT) AS pid,"norm._provenance.map" AS map FROM events WHERE id=1',
    }),
  );
  assert.equal(enriched.rows[0][2], "200");
  assert.equal(enriched.rows[0][3], "loginsight.local-normalization");
  record("declarative-map-preview-persistence-provenance-and-sql");
  // ---- Extração e normalização on the real engine: Grok, typed outputs, scan, indexes
  const lab = await page.evaluate(async () => {
    const grok = {
      id: "grok-real", version: "1", title: "Grok real", schema: 2,
      rules: [{ id: "r", extract: { kind: "grok", patterns: ["user=%{WORD:quem} sequence=%{INT:seq}"], anchor: "full", source: "message" },
        fields: { quem: { from: ["quem"], type: "text", steps: [{ op: "uppercase" }], index: "terms" }, sequencia: { from: ["seq"], type: "integer", index: "range" } } }],
    };
    const json = JSON.stringify(grok);
    const catalog = await localNative({ action: "map_catalog" });
    const checked = await localNative({ action: "map_check", map_json: json });
    const bad = await localNative({ action: "map_check", map_json: json.replace("%{WORD:quem}", "%{NOPE:quem}") });
    const sampled = await localNative({ action: "map_sample", count: 20, field: "message", seed: 3 });
    const tried = await localNative({ action: "map_try", map_json: json, texts: ["user=ana sequence=7", "sem formato"] });
    const evaluated = await localNative({ action: "map_evaluate", map_json: json, samples: sampled.samples.slice(0, 5).map((s) => ({ id: s.id, event_ref: s.eventRef })) });
    const scanned = await localNative({ action: "map_scan", map_json: json });
    await localNative({ action: "save_map", name: "grokmap", map_json: json });
    await loadDerivedFields();
    const typed = await localNative({ action: "query", sql: 'SELECT "grokmap.quem" AS q, try_cast("grokmap.sequencia" AS BIGINT) AS s FROM events WHERE id=5' });
    return { catalog: catalog.grok.patterns.length, templates: catalog.templates.length, checked, bad, population: sampled.population, sampled: sampled.samples.length, tried, evaluated, scanned, typed };
  });
  assert.ok(lab.catalog > 40 && lab.templates >= 8);
  assert.equal(lab.checked.valid, true);
  assert.equal(lab.bad.valid, false);
  assert.ok(lab.bad.diagnostics[0].end > lab.bad.diagnostics[0].start, "the diagnostic carries a position");
  assert.equal(lab.population, 401);
  assert.equal(lab.sampled, 20);
  assert.deepEqual(lab.tried.results[0].fields, { quem: "ANA", sequencia: 7 });
  assert.equal(lab.tried.results[1].trace.rules[0].extraction, "not_recognized");
  assert.deepEqual(lab.tried.results[0].trace.rules[0].extracted[0].slice(0, 1), ["quem"]);
  assert.equal(lab.evaluated.inspected, 5);
  assert.equal(lab.scanned.countsComplete, true);
  assert.equal(lab.scanned.inspected, 401);
  assert.equal(lab.scanned.rules[0].recognized, 401);
  assert.deepEqual(lab.typed.rows, [["ANA", "5"]]);
  record("grok-samples-typed-outputs-scan-and-sql-on-the-real-engine", { scan: lab.scanned.elapsedMs });
  const indexes = await page.evaluate(async () => {
    const owner = AnalysisContexts.capture();
    const count = (filters) => api("count_filtered", { filters }, { silent: true });
    const filters = [
      { column: "grokmap.sequencia", op: "gte", value: "100", value2: null },
      { column: "grokmap.sequencia", op: "between", value: "10", value2: "20" },
      { column: "grokmap.quem", op: "term", value: "ana", value2: null },
      { column: "grokmap.quem", op: "term", value: "ninguem", value2: null },
    ];
    const before = [];
    for (const f of filters) before.push(await count([f]));
    await api("field_index_create", { column: "grokmap.sequencia", kind: "range" }, { analysisOwner: owner });
    const status = await api("field_index_create", { column: "grokmap.quem", kind: "terms" }, { analysisOwner: owner });
    const after = [];
    for (const f of filters) after.push(await count([f]));
    const removed = await api("field_index_remove", { column: "grokmap.quem", kind: "terms" }, { analysisOwner: owner });
    return { before, after, kinds: status.indexes.map((i) => i.kind).sort(), bytes: status.indexes.every((i) => i.bytes > 0), left: removed.indexes.length };
  });
  assert.deepEqual(indexes.before, [301, 11, 401, 0]);
  assert.deepEqual(indexes.after, indexes.before, "indexes never change a result");
  assert.deepEqual(indexes.kinds, ["range", "terms"]);
  assert.equal(indexes.bytes, true);
  assert.equal(indexes.left, 1);
  record("range-and-terms-indexes-built-removed-and-equivalent-on-the-real-engine");
  await page.evaluate(() => Workspace.showPage("extraction"));
  await page.waitForSelector("#ex-left");
  await page.waitForFunction(() => document.querySelector("#ex-installed")?.options.length > 1);
  await page.getByRole("button", { name: /Sortear amostras/ }).click();
  await page.waitForFunction(() => document.querySelectorAll("#ex-sample-list .ex-sample").length > 5);
  await page.waitForFunction(() => /Prévia rápida/.test(document.querySelector("#ex-right-body")?.textContent || ""));
  await page.screenshot({ path: resolve(output, "extraction.png"), fullPage: true });
  record("extraction-page-samples-and-real-preview-in-the-app");
  const tree = await page.evaluate(() => localNative({ action: "processes" }));
  const root = tree.nodes.find((n) => n.pid === "100");
  assert.ok(root);
  const children = await page.evaluate(
    ({ graph, parent }) =>
      localNative({
        action: "process_page",
        graph,
        parent,
        search: "",
        offset: 0,
        selected: null,
      }),
    { graph: tree.graph, parent: root.key },
  );
  assert.equal(children.nodes.length, 1);
  assert.equal(children.nodes[0].pid, "200");
  assert.equal(children.nodes[0].relation, "explicit_guid");
  record("process-guid-tree-lazy-child");
  const context = await page.evaluate(
    (event_ref) =>
      localNative({
        action: "context",
        id: 1,
        event_ref,
        mode: "physical",
        radius: 2,
        seconds: 120,
        field: null,
        pattern: null,
      }),
    sql.rows[1][1],
  );
  assert.ok(context.rows.length >= 3);
  record("source-context-preserves-original-event-references");
  const scopes = await page.evaluate(async (event_ref) => {
    const pattern = {
      engine: "rust",
      text: "^user=ana sequence=1$",
      caseSensitive: true,
      multiline: false,
      dotAll: false,
    };
    const request = {
      action: "context",
      id: 1,
      event_ref,
      mode: "physical",
      radius: 1,
      pattern,
    };
    const message = await localNative({ ...request, subjectScope: "message" });
    const raw = await localNative({ ...request, subjectScope: "event" });
    return {
      message: message.rows.find((r) => r.id === 1),
      raw: raw.rows.find((r) => r.id === 1),
    };
  }, sql.rows[1][1]);
  assert.equal(scopes.message.spans.length, 1);
  assert.equal(scopes.raw.spans.length, 0);
  assert.equal(scopes.message.text, "user=ana sequence=1");
  record("context-matches-the-original-search-subject");
  await page.evaluate(() => Workspace.showPage("sql"));
  await page
    .locator("#lx-sql")
    .fill("SELECT id,event_ref,message FROM events ORDER BY id LIMIT 4");
  await page.locator("#lx-sql-run").click();
  await page.waitForSelector("#lx-sql-result tbody tr");
  await page.screenshot({ path: resolve(output, "sql.png"), fullPage: true });
  await page.getByRole("tab", { name: "Processos", exact: true }).click();
  await page.locator("#lx-process-build").click();
  await page.waitForSelector("#lx-process-tree [data-process]");
  await page.screenshot({
    path: resolve(output, "processes.png"),
    fullPage: true,
  });
  record("native-ui-sql-and-processes");
  // ---- expression search in Eventos: real PCRE2, real filter pipeline, real SQL scope
  await page.evaluate(() => Workspace.showPage("explore"));
  await page.locator("#btn-add-expr").click();
  await page.locator("#expr-search [data-f=engine]").selectOption("pcre2");
  await page.locator("#expr-search [data-f=caseSensitive]").check();
  await page.locator("#expr-search .rxw-edit-row .rx-input").click();
  await page.keyboard.insertText("(?<=sequence=)(?<n>1\\d)");
  await page.evaluate(() => EventPatternSearch.open().sample.set("😀 user=ana sequence=12\r\nuser=ana sequence=7", { fromUser: false }));
  await page.waitForFunction(() => /Válida · PCRE2/.test(document.querySelector("#expr-search .rxw-status")?.textContent || ""));
  await page.waitForFunction(() => document.querySelector("#expr-search .rxw-table")?.textContent.includes("[21–23]"));
  const workbench = await page.locator("#expr-search .rxw-table").textContent();
  assert.ok(workbench.includes("não participou") === false);
  assert.ok(/12/.test(workbench), "capture value comes from the engine ranges (emoji and CRLF safe)");
  const typed = await page.evaluate(() => document.querySelector("#expr-search .rxw-status").textContent);
  assert.match(typed, /PCRE2 10\./, "the embedded library version is reported");
  await page.locator("#expr-search [data-act=apply]").click();
  await page.waitForFunction(() => state.filters.some((f) => f.op === "search"));
  const applied = await page.evaluate(async () => {
    const filters = backendFilters();
    const count = await api("count_filtered", { filters }, { silent: true });
    const sql = await localNative({ action: "query", sql: "SELECT count(*) FROM events" }, filters);
    const page = await api("query_page", { filters, sortColumn: "timestamp", sortDir: "asc", offset: 0, limit: 25 }, { silent: true });
    const groups = await api("aggregate_events", { filters, groupColumn: "level", aggs: [{ func: "count", column: "*", alias: "n" }] }, { silent: true });
    return { count, sql: sql.rows[0][0], total: page.total, firstPage: page.rows.length, grouped: groups.rows.reduce((sum, row) => sum + Number(row.n), 0), filters };
  });
  const expressionExport = resolve(output, "expression-export.jsonl");
  await page.evaluate((path) => api("export_events", { path, format: "jsonl", filters: backendFilters(), mask: false }, { silent: true }), expressionExport);
  const exported = (await readFile(expressionExport, "utf8")).trim().split("\n").length;
  assert.equal(exported, 110, "the export uses the same selection");
  assert.equal(applied.grouped, 110, "grouping uses the same selection");
  // 10..19 and 100..199 are followed by two digits starting with 1 after "sequence=".
  assert.equal(applied.count, 110);
  assert.equal(applied.sql, "110");
  // Interactive pages may defer the exact total; when given it is the same set.
  assert.ok(applied.total === 110 || applied.total === null, `page total ${applied.total}`);
  assert.equal(applied.firstPage, 25);
  record("expression-filter-covers-the-complete-set-through-count-page-and-sql", applied);
  const refused = await page.evaluate(async () => {
    const filter = { column: "message", op: "search", value: "(?<=a+)b", value2: JSON.stringify({ engine: "pcre2" }) };
    try { await api("count_filtered", { filters: [filter] }, { silent: true }); return "accepted"; } catch (error) { return String(error); }
  });
  assert.match(refused, /Express/, "an invalid expression is refused, never counted as zero");
  const other = await page.evaluate(async () => {
    const filter = { column: "message", op: "search", value: "(?<=sequence=)1\\d", value2: JSON.stringify({ engine: "rust" }) };
    try { await api("count_filtered", { filters: [filter] }, { silent: true }); return "accepted"; } catch (error) { return String(error); }
  });
  assert.match(other, /Express/, "PCRE2 syntax is not silently reinterpreted by the Rust engine");
  record("invalid-or-engine-mismatched-expression-is-refused");
  // An engine limit reached while matching fails the query; it is never a count, and the
  // partial selection is not cached for the next identical request.
  const limited = await page.evaluate(async () => {
    const filter = { column: "CommandLine", op: "search", value: "(*NO_START_OPT)^(?:[\\s\\S]+[\\s\\S]+)+!", value2: JSON.stringify({ engine: "pcre2", caseSensitive: true }) };
    const attempt = async () => { try { return { count: await api("count_filtered", { filters: [filter] }, { silent: true }) }; } catch (error) { return { error: String(error) }; } };
    return [await attempt(), await attempt()];
  });
  for (const outcome of limited) assert.ok(outcome.error && !("count" in outcome), `a limit must be an error, got ${JSON.stringify(outcome)}`);
  record("engine-limit-is-an-error-and-is-never-cached", { message: limited[0].error.slice(0, 120) });
  await page.screenshot({ path: resolve(output, "expression.png"), fullPage: true });
  // ---- context of the event inside the detail drawer, from the real native source
  await page.evaluate(() => { state.filters = []; renderChips(); });
  const eventRef = await page.evaluate(() => state.rows[5]?.event_ref);
  await page.evaluate(async () => { const row = state.rows[5]; await openDetail(row.id, { eventRef: row.event_ref }); });
  await page.waitForSelector("#ws-detail-context:not([hidden])");
  await page.locator("#ws-detail-context").click();
  await page.waitForSelector("#pane-context .ec-row");
  assert.ok((await page.locator("#pane-context .ec-row").count()) >= 3);
  assert.ok(eventRef);
  record("event-context-in-the-detail-drawer");
  await page.evaluate(() => closeDrawer());
  // ---- derived field from a capture group, saved in the Case and used in SQL
  await page.evaluate((event_ref) => FieldExtraction.open({ source: "message", event: { id: 1, event_ref } }), sql.rows[1][1]);
  await page.locator("#field-extraction-modal .rxw-edit-row .rx-input").click();
  await page.keyboard.insertText("(?<=user=)(?<quem>\\w+)");
  await page.locator('#field-extraction-modal [data-f="name"]').fill("quem_extraido");
  await page.waitForFunction(() => /Válida/.test(document.querySelector("#field-extraction-modal .rxw-status")?.textContent || ""));
  await page.locator('#field-extraction-modal [data-act="save"]').click();
  await page.waitForFunction(() => document.querySelector("#field-extraction-modal").hidden);
  const extracted = await page.evaluate(async () => {
    const group = await localNative({ action: "query", sql: "SELECT quem_extraido, count(*) AS n FROM events GROUP BY quem_extraido" });
    const filter = { column: "quem_extraido", op: "equals_exact", value: "ana", value2: null };
    const filtered = await api("count_filtered", { filters: [filter] }, { silent: true });
    const original = await localNative({ action: "query", sql: "SELECT count(*) FROM events WHERE message LIKE 'user=ana%'" });
    return { rows: group.rows, filtered, original: original.rows[0][0] };
  });
  assert.deepEqual(extracted.rows, [["ana", "401"]]);
  assert.equal(extracted.filtered, 401);
  assert.equal(extracted.original, "401", "the original field keeps its content");
  record("capture-saved-as-field-and-used-in-filter-and-sql");
  const cancellation = await page.evaluate(async () => {
    const operationId = `cancel-sql-${Date.now()}`,
      owner = await AnalysisContexts.prepare(AnalysisContexts.capture());
    const pending = window.__TAURI__.core.invoke("local_analysis", {
      request: {
        action: "query",
        sql: "SELECT sum(a.id*b.id*c.id*d.id) FROM events a CROSS JOIN events b CROSS JOIN events c CROSS JOIN events d",
      },
      filters: [],
      analysisContext: owner.identity,
      sourceGeneration: owner.sourceGeneration,
      operationId,
    });
    await new Promise((done) => setTimeout(done, 100));
    await window.__TAURI__.core.invoke("cancel_task", { operationId });
    try {
      await pending;
      return false;
    } catch {
      return true;
    }
  });
  assert.equal(cancellation, true);
  record("native-sql-cancellation");
  const exclusion = await page.evaluate(async () => {
    const preview = await api(
      "exclusion_preview",
      { scope: { kind: "selected", ids: [1] } },
      { silent: true },
    );
    return api(
      "exclusion_commit",
      {
        previewToken: preview.previewToken,
        label: "native acceptance",
        reason: "fixture only",
      },
      { silent: true },
    );
  });
  assert.ok(exclusion.analysisContext);
  const visible = await page.evaluate(() =>
    localNative({ action: "query", sql: "SELECT count(*) AS n FROM events" }),
  );
  assert.equal(visible.rows[0][0], "400");
  const oldRef = sql.rows[1][1];
  const denied = await page.evaluate(async (event_ref) => {
    try {
      await localNative({
        action: "context",
        id: 1,
        event_ref,
        mode: "physical",
        radius: 2,
      });
      return false;
    } catch {
      return true;
    }
  }, oldRef);
  assert.equal(denied, true);
  const stale = await page.evaluate(async (result) => {
    try {
      await localNative({ action: "query_page", result, offset: 0, limit: 10 });
      return false;
    } catch {
      return true;
    }
  }, sql.result);
  assert.equal(stale, true);
  record("exclusion-enforced-and-old-snapshot-invalidated");
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
