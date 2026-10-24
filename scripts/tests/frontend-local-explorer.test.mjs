import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
const eq = (actual, expected) => assert.deepStrictEqual(JSON.parse(JSON.stringify(actual)), expected);
const source = readFileSync(
  new URL("../../frontend/local-explorer.js", import.meta.url),
  "utf8",
);
function harness() {
  const listeners = new Map(),
    sandbox = {
      window: { LocalNative: { cancelAll() {}, cancel() {}, stamp: () => "k" } },
      document: { addEventListener: (name, fn) => listeners.set(name, fn) },
      structuredClone,
      esc: (value) =>
        String(value)
          .replaceAll("&", "&amp;")
          .replaceAll("<", "&lt;")
          .replaceAll(">", "&gt;")
          .replaceAll('"', "&quot;"),
      activeCase: () => ({ id: "a" }),
      workspaceScope: () => "dataset",
      state: { currentArtifact: null },
      Symbol,
      Map,
      Set,
      Promise,
      Number,
      JSON,
      BigInt,
    };
  vm.runInNewContext(source, sandbox);
  return { api: sandbox.window.LocalExplorer, listeners };
}
test("native UTF-16 highlight intervals preserve Unicode, escaping and active-match priority", () => {
  const { api } = harness();
  const html = api.paint(
    "😀 <ação>",
    [{ start: 4, end: 8 }],
    [{ start: 0, end: 9, label: "whole", color: "#ff9999" }],
    true,
  );
  assert.ok(html.includes("😀 &lt;"));
  assert.ok(html.includes("lx-active"));
  assert.ok(html.includes("ação"));
  assert.ok(html.includes("&gt;"));
  assert.ok(!html.includes("<ação>"));
});
test("saved SQL and visual rules have bounded Case-owned serializable preferences", () => {
  const { api } = harness(),
    value = api.sanitize({
      sql: "SELECT 1",
      history: Array(40).fill("SELECT 2"),
      saved: Array(80).fill({ name: "x", sql: "SELECT 3" }),
      rules: [
        {
          id: "a",
          label: "invalid color",
          color: "url(secret)",
          pattern: { engine: "pcre2", text: "." },
        },
        {
          id: "b",
          label: "ok",
          color: "#123abc",
          enabled: true,
          pattern: { engine: "pcre2", text: "(?<=id=)\\w+" },
        },
      ],
    });
  assert.equal(value.sql, "SELECT 1");
  assert.equal(value.history.length, 30);
  assert.equal(value.saved.length, 50);
  assert.equal(value.rules.length, 1);
  assert.equal(value.rules[0].id, "b");
  api.restore(value);
  const first = api.capture();
  first.saved.length = 0;
  assert.equal(api.capture().saved.length, 50);
});
test("Explorar keeps Eventos, SQL and Processos; text and regex pages are gone", () => {
  const { api } = harness();
  eq([...api.pages], ["sql", "processes"]);
  const workspace = readFileSync(new URL("../../frontend/workspace.js", import.meta.url), "utf8");
  assert.ok(workspace.includes('["explore","fa-table","Eventos"],["sql","fa-database","SQL"],["processes","fa-code-branch","Processos"]'));
  assert.ok(!workspace.includes('"Texto"]') && !workspace.includes('"Regex"]'));
  assert.ok(workspace.includes('"file-viewer": "Visualizar arquivos"'));
  assert.ok(workspace.includes('"log-split", "file-viewer", "extraction"])'));
  assert.ok(workspace.includes("LocalExplorer?.pages.has(next)"));
});
test("saved pages that no longer exist reopen where their features now live", () => {
  const workspace = readFileSync(new URL("../../frontend/workspace.js", import.meta.url), "utf8");
  const context = readFileSync(new URL("../../frontend/workspace-context.js", import.meta.url), "utf8");
  assert.ok(workspace.includes('({ text: "file-viewer", regex: "explore", maps: "extraction" })[next] ?? next'));
  assert.ok(context.includes('{ text: "file-viewer", regex: "explore", maps: "extraction" }[input.page]'));
  // A direct text session is runtime only and is never part of what is persisted.
  const { api } = harness();
  const saved = JSON.stringify(api.sanitize({ sql: "SELECT 1", expression: { pattern: "(?<=a)b", engine: "pcre2", field: "message", scope: "line" } }));
  assert.ok(!saved.includes("session"));
  eq(JSON.parse(saved).expression, { pattern: "(?<=a)b", engine: "pcre2", field: "message", caseSensitive: false, multiline: false, dotAll: false, scope: "line" });
});
test("expression drafts are bounded and only accept known engines", () => {
  const { api } = harness();
  assert.equal(api.sanitize({ expression: { pattern: "x".repeat(16385), engine: "pcre2", field: "message" } }).expression, null);
  assert.equal(api.sanitize({ expression: { pattern: "x", engine: "perl", field: "message" } }).expression, null);
  assert.equal(api.sanitize({ expression: { pattern: "x", engine: "rust", field: "message" } }).expression.scope, "event");
});
test("every new script is bundled and loaded by the page", () => {
  const html = readFileSync(new URL("../../frontend/index.html", import.meta.url), "utf8");
  const prepare = readFileSync(new URL("../prepare-frontend.mjs", import.meta.url), "utf8");
  for (const file of ["local-native.js", "local-explorer.js", "file-viewer.js", "regex-syntax.js", "regex-editor.js", "regex-assist.js", "regex-workbench.js", "event-context.js", "event-pattern-search.js", "field-extraction.js", "extraction-model.js", "extraction-easy.js", "extraction.js"]) {
    assert.ok(html.includes(`src="${file}"`), file);
    assert.ok(prepare.includes(`"${file}"`), file);
  }
  for (const file of ["regex-editor.css", "local-explorer.css"]) assert.ok(html.includes(`href="${file}"`) && prepare.includes(`"${file}"`), file);
});
