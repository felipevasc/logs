import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
const source = readFileSync(new URL("../../frontend/regex-syntax.js", import.meta.url), "utf8");
const sandbox = { window: {} };
vm.runInNewContext(source, sandbox);
const R = sandbox.window.RegexSyntax;
const caps = (pattern) => R.analyze(pattern).captures.map((g) => [g.index, g.name]);
const eq = (actual, expected, message) => assert.deepStrictEqual(JSON.parse(JSON.stringify(actual)), expected, message);
const apply = (text, built) => R.applyEdits(text, built.edits);

test("parentheses that are not captures never become groups", () => {
  eq(caps("\\("), []);
  eq(caps("[()]"), []);
  eq(caps("(?:abc)"), []);
  eq(caps("(?<=user=)"), []);
  eq(caps("\\Q(a+b)\\E"), []);
  eq(caps("(?#note (not a group))x"), []);
  eq(caps("(?=a)(?!b)(?<!c)(?>d)"), []);
  eq(caps("(*pla:x)(*nlb:y)"), []);
  eq(caps("(?<usuario>\\w+)"), [[1, "usuario"]]);
  eq(caps("(?P<a>x)(?'b'y)(z)"), [[1, "a"], [2, "b"], [3, null]]);
});

test("nested groups, numbering and classification", () => {
  const a = R.analyze("((a)(?:b(c))(?<n>d))");
  eq(a.captures.map((g) => [g.index, g.name, g.depth]), [[1, null, 0], [2, null, 1], [3, null, 2], [4, "n", 1]]);
  assert.equal(a.complete, true);
  assert.equal(a.groups.find((g) => g.kind === "noncapture").index, null);
});

test("captures inside lookaround keep their identity and are flagged", () => {
  const a = R.analyze("(?<=(?<u>user)=)\\w+(?=(\\d))");
  eq(a.captures.map((g) => [g.index, g.name, g.insideLookaround]), [[1, "u", true], [2, null, true]]);
  const kinds = a.groups.map((g) => g.kind);
  eq(kinds, ["lookbehind", "capture", "lookahead", "capture"]);
  assert.match(R.describe(a, { group: a.groups[0] }), /participa do teste, mas não é consumida/);
  assert.doesNotMatch(R.describe(a, { group: a.groups[0] }), /não é consultad/);
});

test("numbered and named references resolve to the right capture", () => {
  const a = R.analyze("user=(\\w+)\\s+confirmado=\\1\\b");
  const ref = a.references[0];
  eq([ref.valid, ref.targets, ref.resolvedNumber], [true, [0], 1]);
  const b = R.analyze("user=(?<usuario>\\w+)\\s+confirmado=\\k<usuario>\\b");
  eq([b.references[0].valid, b.references[0].targets], [true, [0]]);
  for (const syntax of ["\\k'usuario'", "\\k{usuario}", "\\g{usuario}", "(?P=usuario)"]) {
    const c = R.analyze(`(?<usuario>a)${syntax}`);
    eq([c.references[0].valid, c.references[0].targets, c.references[0].type], [true, [0], "backref"], syntax);
  }
});

test("subroutine calls are not backreferences", () => {
  for (const syntax of ["(?1)", "(?&n)", "\\g<n>", "(?P>n)", "(?-1)"]) {
    const a = R.analyze(`(?<n>a)${syntax}`);
    assert.equal(a.references[0].type, "subroutine", syntax);
    assert.match(R.describe(a, { ref: a.references[0] }), /subrotina/);
    assert.doesNotMatch(R.describe(a, { ref: a.references[0] }), /mesmo texto/);
  }
  assert.match(R.describe(R.analyze("(a)\\1"), { ref: R.analyze("(a)\\1").references[0] }), /mesmo texto/);
});

test("relative references count captures opened before them", () => {
  const a = R.analyze("(a)(b)\\g{-1}\\g{-2}(c)\\g{+1}(d)");
  eq(a.references.map((r) => r.resolvedNumber), [2, 1, 4]);
  assert.ok(a.references.every((r) => r.valid));
});

test("invalid references are reported, never hidden", () => {
  const a = R.analyze("(a)\\2 \\k<nada>");
  eq(a.references.map((r) => r.valid), [false, false]);
  assert.equal(a.problems.filter((p) => p.code === "reference").length, 2);
  assert.match(a.references[0].message, /Não existe o grupo 2/);
});

test("\\10 is a reference only when that many groups exist", () => {
  const few = R.analyze("(a)\\10");
  assert.equal(few.references[0].octal, true);
  assert.equal(few.problems.length, 0);
  const many = R.analyze("(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\\10");
  assert.equal(many.references[0].valid, true);
  eq(many.references[0].targets, [9]);
});

test("escapes, classes and quote blocks hide their parentheses", () => {
  const a = R.analyze("\\(a\\)[(]\\Q(x\\E(y)");
  eq(a.captures.map((g) => g.index), [1]);
  assert.equal(a.tokens.filter((t) => t.type === "quote").length, 1);
  assert.equal(R.analyze("[]()]").captures.length, 0);
  assert.equal(R.analyze("[^]()]").captures.length, 0);
  assert.equal(R.analyze("[[:alpha:]()]").captures.length, 0);
  assert.equal(R.analyze("\\Q(unterminated").captures.length, 0);
});

test("incomplete expressions stay analyzable and say what is open", () => {
  const a = R.analyze("user=(\\w+");
  assert.equal(a.complete, false);
  assert.equal(a.captures.length, 1);
  assert.equal(a.problems[0].code, "unclosed");
  const b = R.analyze("a)b");
  assert.equal(b.problems[0].code, "unbalanced");
  const c = R.analyze("[abc");
  assert.equal(c.complete, false);
  assert.equal(c.problems[0].code, "unclosed-class");
  assert.equal(R.analyze("").captures.length, 0);
  assert.equal(R.analyze("\\").problems[0].code, "escape");
});

test("inline options: (?n) stops plain groups capturing, (?x) allows comments", () => {
  eq(caps("(?n)(a)(?<k>b)"), [[1, "k"]]);
  const x = R.analyze("(?x) (a) # not (a group)\n (b)");
  assert.equal(x.captures.length, 2);
  assert.equal(x.features.extended, true);
  const scoped = R.analyze("(?x: (a) # (c)\n )(b)");
  assert.equal(scoped.captures.length, 2);
  const after = R.analyze("(?x: a )(b)#(c)");
  assert.equal(after.captures.length, 2, "# is literal again after the x group");
});

test("branch reset and duplicate names are marked unreliable, not guessed", () => {
  const a = R.analyze("(?|(a)|(b))(c)");
  eq(a.captures.map((g) => g.index), [1, 1, 2]);
  assert.equal(a.features.branchReset, true);
  assert.equal(a.captures[0].uncertain, true);
  assert.equal(a.captures[2].uncertain, false);
  const d = R.analyze("(?J)(?<n>a)|(?<n>b)\\k<n>");
  assert.equal(d.features.duplicateNames, true);
  assert.equal(d.references[0].ambiguous, true);
  const neutral = R.segments(d, { slotOf: () => 3 }).flatMap((s) => s.classes);
  assert.ok(neutral.includes("rx-ref-neutral"));
});

test("conditionals map their condition to a reference and keep numbering", () => {
  const a = R.analyze("(a)?(?(1)b|c)(d)");
  assert.equal(a.features.conditionals, true);
  eq(a.captures.map((g) => g.index), [1, 2]);
  assert.equal(a.references[0].type, "condition");
  assert.equal(a.references[0].valid, true);
  const named = R.analyze("(?<x>a)?(?(<x>)b|c)");
  eq(named.references[0].targets, [0]);
  const assertion = R.analyze("(?(?=a)ab|cd)(e)");
  eq(assertion.captures.map((g) => g.index), [1]);
});

test("unknown constructs stay neutral and block transformations", () => {
  const a = R.analyze("(?Cx)(a)");
  assert.ok(a.uncertain.length > 0);
  assert.equal(R.wrappable(a, 0, 4).ok, false);
});

test("verbs and options are single tokens without groups", () => {
  const a = R.analyze("(*UTF)(*SKIP)(?i)(a)(*MARK:x)");
  eq(a.captures.map((g) => g.index), [1]);
  assert.equal(a.tokens.filter((t) => t.type === "verb").length, 3);
});

test("wrappable selections respect token, group and quantifier boundaries", () => {
  const text = "a\\d+(?:bc)*[xy]d";
  const a = R.analyze(text);
  assert.equal(R.wrappable(a, 1, 4).ok, true, "escape with quantifier");
  assert.equal(R.wrappable(a, 1, 3).ok, false, "cuts the quantifier off");
  assert.equal(R.wrappable(a, 2, 4).ok, false, "starts inside an escape");
  assert.equal(R.wrappable(a, 4, 11).ok, true, "group with quantifier");
  assert.equal(R.wrappable(a, 7, 9).ok, true, "inside the group");
  assert.equal(R.wrappable(a, 5, 9).ok, false, "starts inside the open syntax");
  assert.equal(R.wrappable(a, 11, 15).ok, true, "class");
  assert.equal(R.wrappable(R.analyze("(a"), 1, 2).ok, false, "incomplete expressions are not rewritten");
  assert.equal(R.wrappable(R.analyze("a|b|c"), 2, 5).ok, true, "alternatives at the same level");
});

test("literal escaping treats every metacharacter as text", () => {
  const out = R.escapeLiteral("a+b(1).log");
  assert.equal(out, "a\\+b\\(1\\)\\.log");
  assert.equal(R.escapeLiteral("x\ty\r\nz"), "x\\ty\\r\\nz");
  assert.equal(R.escapeLiteral("a b#c", { extended: true }), "a\\ b\\#c");
  assert.equal(R.escapeLiteral("a b#c"), "a b#c");
  assert.equal(R.escapeLiteral("[]{}^$|?*\\"), "\\[\\]\\{\\}\\^\\$\\|\\?\\*\\\\");
  const a = R.analyze(out);
  assert.equal(a.captures.length, 0);
  assert.equal(a.problems.length, 0);
});

test("assisted 'what comes after' builds a bounded lookbehind", () => {
  const value = R.valueExpression("word");
  const built = R.relative("after", { before: "user=", value: { ...value, kind: "word" } });
  assert.equal(built.text, "(?<=user=)\\w+");
  assert.match(built.explanation, /user= antes do valor, sem incluir esse trecho/);
  const named = R.relative("after", { before: "user=", value: { ...value, kind: "word" }, name: "usuario" });
  assert.equal(named.text, "(?<=user=)(?<usuario>\\w+)");
  const hazard = R.relative("after", { before: "a+b(1)=", value: { ...value, kind: "word" } });
  assert.equal(hazard.text, "(?<=a\\+b\\(1\\)=)\\w+");
});

test("'before' and 'between' combine the conditions", () => {
  const value = { ...R.valueExpression("number"), kind: "number" };
  assert.equal(R.relative("before", { after: " ms", value }).text, "-?\\d+(?:[.,]\\d+)?(?= ms)");
  assert.equal(R.relative("between", { before: "[", after: "]", value }).text, "(?<=\\[)-?\\d+(?:[.,]\\d+)?(?=\\])");
});

test("delimiters: one character is a class, several are a lookahead loop", () => {
  assert.equal(R.valueExpression("delimiter", { delimiter: ";" }).text, "[^;\\r\\n]+");
  assert.equal(R.valueExpression("delimiter", { delimiter: "]" }).text, "[^\\]\\r\\n]+");
  assert.equal(R.valueExpression("delimiter", { delimiter: "-" }).text, "[^\\-\\r\\n]+");
  const multi = R.valueExpression("delimiter", { delimiter: "&&" });
  assert.equal(multi.text, "(?:(?!&&)[^\\r\\n])+");
  assert.equal(R.valueExpression("delimiter", { delimiter: "" }).ok, false);
  assert.equal(R.valueExpression("custom", { custom: "a|b" }).text, "(?:a|b)");
  assert.equal(R.valueExpression("custom", { custom: "\\d{3}" }).text, "\\d{3}");
  assert.equal(R.valueExpression("custom", { custom: "" }).ok, false);
});

test("capture names are sanitized and unique", () => {
  assert.equal(R.captureName("user="), "user");
  assert.equal(R.captureName("usuário"), "usuario");
  assert.equal(R.captureName("123"), "v_123");
  assert.equal(R.captureName("==="), "valor");
  assert.equal(R.captureName("user", new Set(["user", "user_2"])), "user_3");
});

test("back-reference text prefers names and avoids digit ambiguity", () => {
  const a = R.analyze("(?<u>a)(b)");
  assert.equal(R.referenceText(a.captures[0]), "\\k<u>");
  assert.equal(R.referenceText(a.captures[1], "x"), "\\2");
  assert.equal(R.referenceText(a.captures[1], "5"), "\\g{2}");
});

test("rename touches only syntactic names, never literals or comments", () => {
  const text = "(?<u>a)\\k<u> u (?#\\k<u>) \\Q\\k<u>\\E (?P=u)(?<ux>b)";
  const a = R.analyze(text);
  const out = R.rename(a, "u", "usuario");
  assert.equal(out.ok, true);
  assert.equal(apply(text, out), "(?<usuario>a)\\k<usuario> u (?#\\k<u>) \\Q\\k<u>\\E (?P=usuario)(?<ux>b)");
  assert.equal(R.rename(a, "u", "ux").ok, false);
  assert.equal(R.rename(a, "u", "1x").ok, false);
  assert.equal(R.rename(a, "nada", "x").ok, false);
});

test("diff yields the smallest single replacement and never splits an emoji", () => {
  eq(R.diff("abc", "aXbc"), { start: 1, end: 1, text: "X" });
  eq(R.diff("abc", "abc"), { start: 3, end: 3, text: "" });
  const d = R.diff("😀a", "😀b😀a");
  assert.equal(R.applyEdits("😀a", [{ start: d.start, end: d.end, text: d.text }]), "😀b😀a");
  const e = R.diff("x😀", "x😁");
  assert.equal(R.applyEdits("x😀", [{ start: e.start, end: e.end, text: e.text }]), "x😁");
});

test("segments layer lookaround and captures without losing the capture identity", () => {
  const a = R.analyze("(?<=(?<u>user)=)\\w+");
  const segs = R.segments(a, { slotOf: (id) => id + 1 });
  const inside = segs.find((s) => a.text.slice(s.start, s.end).includes("user"));
  assert.ok(inside.classes.includes("rx-look"));
  assert.ok(inside.classes.includes("rx-cap-2"), "capture slot survives inside a lookbehind");
  const backref = R.segments(R.analyze("user=(\\w+)\\s+confirmado=\\1\\b"), { slotOf: () => 4 }).find((s) => s.classes.includes("rx-ref"));
  assert.ok(backref.classes.includes("rx-cap-4"));
});

test("long expressions fall back to plain text with a predictable cost", () => {
  const a = R.analyze("(".repeat(20000));
  assert.equal(a.tooLong, true);
  assert.equal(R.segments(a).length, 1);
  // Just under the limit: thousands of groups are still analysed and painted in bounded time.
  const started = performance.now();
  const big = R.analyze("(a)|".repeat(2000));
  assert.equal(big.captures.length, 2000);
  const segments = R.segments(big, { slotOf: (id) => id % 8 });
  assert.ok(segments.length > 2000);
  assert.ok(performance.now() - started < 1500, "analysis plus painting stays interactive");
  const nested = R.analyze("(".repeat(1500) + "a" + ")".repeat(1500));
  assert.equal(nested.captures.length, 1500);
  assert.ok(R.segments(nested, { slotOf: () => 0 }).length >= 1500);
});
