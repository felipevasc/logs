import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
const sandbox = { window: {}, structuredClone };
vm.runInNewContext(readFileSync(new URL("../../frontend/extraction-model.js", import.meta.url), "utf8"), sandbox);
const M = sandbox.window.ExtractionModel;
const plain = (value) => JSON.parse(JSON.stringify(value));
const sample = "falha 10.0.0.5 usuario=ana acao=login duracao=12.5ms";
const at = (text, part) => ({ start: text.indexOf(part), end: text.indexOf(part) + part.length });

test("chips split a Grok expression into editable tokens and escaped literals", () => {
  const list = plain(M.chips("%{IP:ip_origem} usuario=%{WORD:usuario:int} \\[x\\] %{DATA}"));
  assert.deepEqual(list.map((c) => c.type), ["token", "literal", "token", "literal", "token"]);
  assert.equal(list[0].pattern, "IP");
  assert.equal(list[0].field, "ip_origem");
  assert.equal(list[2].conv, "int");
  assert.equal(list[3].text, " \\[x\\] ");
  assert.equal(M.fromChips(M.chips("a %{IP:x} b\\.")), "a %{IP:x} b\\.");
  assert.deepEqual(plain(M.fieldsOf("%{IP:a} %{INT} %{WORD:b}")), ["a", "b"]);
  assert.equal(M.chips("ok %{IP:x").at(-1).broken, true);
});

test("the same field name always gets the same colour unless it was chosen", () => {
  assert.equal(M.fieldColor("usuario"), M.fieldColor("usuario"));
  assert.notEqual(M.fieldColor("usuario"), M.fieldColor("ip_origem"));
  assert.equal(M.fieldColor("usuario", { usuario: "#112233" }), "#112233");
  assert.equal(M.fieldColor("usuario", { usuario: "red" }), M.fieldColor("usuario"));
});

test("selected text is classified by its characters", () => {
  for (const [text, expected] of [["12", "INT"], ["12.5", "NUMBER"], ["ana", "WORD"], ["ana_1", "WORD"], ["10.0.0.5", "NOTSPACE"], ["a b", "DATA"], ["", "DATA"]]) assert.equal(M.classify(text), expected, text);
});

test("literal text is escaped for Grok and can allow variation", () => {
  assert.equal(M.literal("a.b (x)"), "a\\.b \\(x\\)");
  assert.equal(M.literal("a  b"), "a\\s+b");
  assert.equal(M.literal("id 42 ok", true), "id\\s+%{INT}\\s+ok");
  assert.equal(M.literal("[x]"), "\\[x\\]");
});

test("extracting selections builds a pattern; the name comes from the text before the value", () => {
  let segments = [];
  for (const part of ["10.0.0.5", "ana", "login", "12.5"]) {
    const r = M.apply(sample, segments, "extract", at(sample, part));
    assert.ok(!r.error, r.error);
    segments = r.segments;
  }
  assert.deepEqual(plain(segments.map((s) => s.name)), ["ip", "usuario", "acao", "duracao"].map((n, i) => (i === 0 ? segments[0].name : n)));
  assert.equal(segments[1].name, "usuario");
  const pattern = M.compile(sample, segments);
  assert.ok(pattern.startsWith("%{NOTSPACE:"), pattern);
  assert.ok(pattern.includes("\\sacao=") === false);
  assert.ok(pattern.includes(" usuario=%{WORD:usuario} acao=%{WORD:acao} duracao=%{NUMBER:duracao}"), pattern);
});

test("overlapping marks are refused and removing one frees the space", () => {
  const first = M.apply(sample, [], "extract", at(sample, "usuario=ana"));
  const clash = M.apply(sample, first.segments, "extract", at(sample, "ana"));
  assert.match(clash.error, /sobrep/);
  assert.equal(M.removeSegment(first.segments, 0).length, 0);
});

test("after, before and between keep the marker as fixed text", () => {
  const after = M.apply(sample, [], "after", at(sample, "usuario="), { name: "quem" });
  assert.ok(!after.error);
  assert.equal(M.compile(sample, after.segments), "usuario=%{WORD:quem}");
  const before = M.apply(sample, [], "before", at(sample, "ms"));
  assert.equal(M.compile(sample, before.segments), "%{NOTSPACE:" + before.segments[0].name + "}ms");
  const between = M.apply("<a>valor</a>", [], "between", { start: 0, end: 12 }, { open: "<a>", close: "</a>", name: "v" });
  assert.equal(M.compile("<a>valor</a>", between.segments.map((s) => (s.kind === "field" ? { ...s, name: "v" } : s))), "<a>%{DATA:v}</a>");
  assert.match(M.apply(sample, [], "between", at(sample, "ana"), { open: "[", close: "]" }).error, /não foram encontrados/);
});

test("ignore, fixed, variation and optional change how a piece is written", () => {
  let r = M.apply(sample, [], "extract", at(sample, "ana"));
  r = M.apply(sample, r.segments, "ignore", at(sample, "falha"));
  assert.ok(M.compile(sample, r.segments).startsWith("%{WORD}"));
  const opt = M.apply(sample, M.apply(sample, [], "extract", at(sample, "ana")).segments, "optional", at(sample, "ana"));
  assert.equal(M.compile(sample, opt.segments), "(?:usuario=%{WORD:usuario})?");
  const loose = M.apply("n 42", M.apply("n 42", [], "extract", { start: 0, end: 1 }).segments, "fixed", { start: 1, end: 4 });
  assert.equal(M.compile("n 42", loose.segments), "%{WORD:" + loose.segments[0].name + "} 42");
  assert.match(M.apply(sample, [], "optional", at(sample, "ana")).error, /antes/);
});

test("positions stay in UTF-16 units so they match the native engine", () => {
  const text = "😀 usuario=ana";
  const r = M.apply(text, [], "after", at(text, "usuario="));
  assert.equal(r.segments[1].start, text.indexOf("ana"));
  assert.equal(text.slice(r.segments[1].start, r.segments[1].end), "ana");
});

test("history supports undo, redo and drops the redo branch on a new edit", () => {
  const h = new M.History();
  h.reset("a");
  assert.equal(h.push("b"), true);
  assert.equal(h.push("b"), false);
  h.push("c");
  assert.equal(h.undo(), "b");
  assert.equal(h.undo(), "a");
  assert.equal(h.undo(), null);
  assert.equal(h.redo(), "b");
  h.push("d");
  assert.equal(h.canRedo, false);
});

test("semantic text ignores titles, notes, colours and index preferences", () => {
  const a = { id: "x", version: "1", title: "A", schema: 2, ui: { colors: {} }, rules: [{ id: "r", description: "n", fields: { f: { from: ["x"], description: "d", index: "exact" } } }] };
  const b = structuredClone(a);
  b.title = "B"; b.ui = { colors: { f: "#000000" } }; b.rules[0].description = "m"; b.rules[0].fields.f.description = "e"; b.rules[0].fields.f.index = "none";
  assert.equal(M.semantic(a), M.semantic(b));
  b.rules[0].fields.f.type = "integer";
  assert.notEqual(M.semantic(a), M.semantic(b));
});

test("type suggestions follow the pattern but stay suggestions", () => {
  assert.equal(M.typeForToken("IP", ""), "ip");
  assert.equal(M.typeForToken("WORD", "int"), "integer");
  assert.equal(M.typeForToken("NUMBER", ""), "decimal");
  assert.equal(M.typeForToken("DATA", ""), "text");
});

test("painting never overlaps and keeps every character", () => {
  const parts = M.paintPlan("abcdef", [{ start: 1, end: 3, field: "a" }, { start: 2, end: 5, field: "b" }, { start: 4, end: 6, field: "c" }]);
  assert.equal(parts.map((p) => p.text).join(""), "abcdef");
  assert.deepEqual(plain(parts.filter((p) => p.field).map((p) => p.field)), ["a", "c"]);
});

test("the key that names the first value stays as an anchor, other leading text does not", () => {
  const first = M.apply(sample, [], "extract", at(sample, "ana"));
  assert.ok(M.compile(sample, first.segments).startsWith("usuario=%{WORD:usuario}"));
  const ip = M.apply(sample, [], "extract", at(sample, "10.0.0.5"));
  assert.ok(M.compile(sample, ip.segments).startsWith("%{NOTSPACE:"));
  const spaced = M.apply("nome: ana", [], "extract", { start: 6, end: 9 });
  assert.equal(M.compile("nome: ana", spaced.segments), "nome: %{WORD:nome}");
});
