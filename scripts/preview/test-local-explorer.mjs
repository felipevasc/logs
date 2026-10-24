/* Browser presentation/ownership contracts; native SQL and PCRE2 are tested in Rust. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync } from "node:fs";
const browser = await launchBrowser(),
  page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
mkdirSync("output/playwright", { recursive: true });
await page.addInitScript(() => {
  let searched = false;
  const row = {
    line: 420,
    offset: "10240",
    bytes: "52",
    text: "ERROR user=ana ação=login",
    windowStart: 0,
    truncatedBefore: false,
    truncatedAfter: false,
    lossy: false,
    matchesComplete: true,
    spans: [{ start: 11, end: 14 }],
    highlights: [],
  };
  const columns = [
    { name: "id", dataType: "BIGINT" },
    { name: "event_ref", dataType: "VARCHAR" },
    { name: "exact", dataType: "BIGINT" },
  ];
  const process = {
    key: "root",
    host: "host-a",
    boot: "boot-a",
    guid: "guid-a",
    pid: "123",
    started: 1700000000000,
    ended: null,
    image: "cmd.exe",
    command: "cmd /c script",
    user: "ana",
    parent: null,
    relation: "explicit_guid",
    candidates: 1,
    eventId: 0,
    eventRef: "fixture:0",
    children: 1,
    events: 1,
  };
  window.__mockLocalAnalysis = async ({ request: r }) => {
    if (r.action === "text") {
      const action = r.request.action;
      if (action === "search") searched = true;
      const done = action === "step";
      return {
        status: {
          session: "text-test",
          path: "C:\\fixture.log",
          bytes: "20000",
          indexedBytes: done ? "20000" : "12000",
          indexedLines: 500,
          searchedBytes: searched ? "20000" : "0",
          matches: searched ? 1 : 0,
          indexComplete: done,
          searchComplete: searched && done,
          skippedRecords: 0,
          limitedRecords: 0,
          buckets: Array.from({ length: 128 }, (_, i) =>
            i === 65 && searched ? 1 : 0,
          ),
        },
        lines: r.request.matchesOnly && !searched ? [] : [row],
      };
    }
    if (r.action === "schema") return columns;
    if (r.action === "query" || r.action === "query_page")
      return {
        result: "sql-test",
        columns,
        rows: [[String(r.offset || 0), "fixture:0", "9007199254740993"]],
        total: "161",
        offset: r.offset || 0,
        complete: true,
      };
    if (r.action === "regex")
      return {
        complete: true,
        spans: [
          {
            start: 5,
            end: 8,
            byteStart: 5,
            byteEnd: 8,
            captures: { 0: "ana", user: "ana" },
          },
        ],
      };
    if (r.action === "highlights") return r.texts.map(() => []);
    if (r.action === "map_template")
      return {
        id: "fixture",
        version: "1",
        title: "Local map fixture",
        rules: [
          {
            id: "generic",
            selector: {},
            priority: 0,
            fields: {
              user: {
                from: ["User"],
                format: "text",
                literal: null,
                description: null,
              },
            },
          },
        ],
      };
    if (r.action === "map_preview")
      return {
        inspected: 100,
        affected: 42,
        conflicts: 2,
        samples: [
          {
            id: 0,
            event_ref: "fixture:0",
            enrichment: {
              user: "ana",
              _provenance: { map: "fixture", version: "1" },
            },
          },
        ],
        countsComplete: true,
        sampleOnly: true,
      };
    if (r.action === "processes" || r.action === "process_page")
      return {
        graph: "graph-test",
        nodes: [
          r.parent
            ? {
                ...process,
                key: "child",
                image: "powershell.exe",
                parent: "root",
                children: 0,
                relation: "inferred_pid_lifetime",
              }
            : process,
        ],
        total: 1,
        offset: 0,
        skipped: 0,
        evidence: [
          { id: 0, event_ref: "fixture:0", timestamp: 1700000000000, kind: 1 },
        ],
        ancestors: r.selected ? [process] : [],
      };
    throw Error(`Unmocked presentation request ${r.action}`);
  };
});
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await page.waitForFunction(
    () =>
      window.WorkspaceContext?.ready &&
      !WorkspaceContext.changing &&
      state.loaded &&
      !state.loadOverlay,
  );
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  assert.deepEqual(
    (await page.locator("#page-tabs [role=tab]").allTextContents()).map((t) => t.trim()),
    ["Eventos", "SQL", "Processos"],
  );
  const sourceBefore = await page.evaluate(() => JSON.stringify(state.currentArtifact?.source));
  // Visualizar arquivos lives in Estrutura and never loads the file into the analysis.
  await page.getByRole("button", { name: "Estrutura", exact: true }).click();
  await page.locator('.nav-pages [data-page="file-viewer"]').click();
  assert.equal(await page.locator("#ws-title").textContent(), "Visualizar arquivos");
  await page.evaluate(() => FileViewer.open("C:\\fixture.log"));
  await page.waitForSelector("#fv-head .fv-title");
  const head = await page.locator("#fv-head").textContent();
  assert.match(head, /fixture\.log/);
  assert.match(head, /Tamanho/);
  assert.match(head, /Leitura/);
  assert.doesNotMatch(head, /6\.000|eventos/i, "the header describes the file, never another source's events");
  assert.equal(await page.evaluate(() => JSON.stringify(state.currentArtifact?.source)), sourceBefore, "opening the viewer leaves the analysis untouched");
  assert.ok(await page.getByRole("button", { name: "Abrir na análise" }).count());
  await page.locator("#lx-pattern").fill("ana");
  await page.locator("#lx-search").click();
  await page.waitForFunction(
    () => document.querySelectorAll("#lx-hits [data-hit]").length === 1,
  );
  await page.locator("#lx-hits [data-hit]").first().click();
  assert.match(await page.locator("#lx-source").textContent(), /ação=login/);
  assert.ok(await page.locator("#lx-source mark").count());
  await page.screenshot({
    path: "output/playwright/local-text.png",
    fullPage: true,
  });
  // Another source is already being investigated: moving this file to the analysis asks first.
  await page.getByRole("button", { name: "Abrir na análise" }).click();
  await page.waitForSelector("dialog.lx-dialog[open]");
  assert.match(await page.locator("dialog.lx-dialog").textContent(), /Unir à análise atual[\s\S]*Substituir a análise/);
  await page.locator('dialog.lx-dialog [data-v="cancel"]').click();
  assert.equal(await page.evaluate(() => JSON.stringify(state.currentArtifact?.source)), sourceBefore, "cancelling keeps the current source");
  await page.getByRole("button", { name: "Análise", exact: true }).click();
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  await page.getByRole("tab", { name: "SQL", exact: true }).click();
  await page.locator("#lx-schema-load").click();
  await page.waitForSelector("#lx-schema [data-column]");
  await page.locator("#lx-sql-run").click();
  await page.waitForSelector("#lx-sql-result tbody tr");
  assert.match(
    await page.locator("#lx-sql-result").textContent(),
    /9007199254740993/,
  );
  await page.locator("#lx-query-name").fill("Exatos");
  await page.locator("#lx-query-save").click();
  assert.equal(await page.locator("#lx-saved-select option").count(), 2);
  await page.locator("#lx-sql-result [data-lx-page=next]").click();
  await page.waitForFunction(
    () =>
      document.querySelector("#lx-sql-result tbody td:nth-child(2)")
        ?.textContent === "80",
  );
  await page.screenshot({
    path: "output/playwright/local-sql.png",
    fullPage: true,
  });
  await page.getByRole("tab", { name: "Processos", exact: true }).click();
  await page.locator("#lx-process-build").click();
  await page.waitForSelector("[data-process=root]");
  await page.locator("[data-expand=root]").click();
  await page.waitForSelector("[data-process=child]");
  await page.locator("[data-process=child]").click();
  await page.waitForFunction(() =>
    document
      .querySelector("#lx-process-detail")
      .textContent.includes("cmd /c script"),
  );
  await page.screenshot({
    path: "output/playwright/local-processes.png",
    fullPage: true,
  });
  await page.getByRole("tab", { name: "SQL", exact: true }).click();
  assert.equal(await page.locator("#lx-saved-select option").count(), 2);
  for (const width of [1024, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(
        (theme) => (document.documentElement.dataset.theme = theme),
        theme,
      );
      for (const tab of ["Texto", "SQL", "Processos"]) {
        if (tab === "Texto") await page.evaluate(() => Workspace.showPage("file-viewer"));
        else {
          if ((await page.locator("#page-tabs [role=tab]").count()) === 0) await page.evaluate(() => Workspace.showPage("explore"));
          await page.getByRole("tab", { name: tab, exact: true }).click();
        }
        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth > innerWidth,
        );
        assert.equal(overflow, false, `${tab} ${theme} ${width}px`);
        if (width === 1024 && tab === "Texto")
          await page.screenshot({
            path: `output/playwright/local-text-${theme}-1024.png`,
            fullPage: true,
          });
      }
    }
  }
  assert.deepEqual(errors, []);
  console.log(
    "Local exploration: file viewer in Estrutura (no analysis change), SQL exact cells/pagination/saved query and lazy process branches passed (mock presentation).",
  );
} finally {
  await browser.close();
}
