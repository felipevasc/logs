import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

let serializations = 0;
const context = { window: {}, TextEncoder, TextDecoder, Uint8Array, atob, btoa, JSON: { parse: JSON.parse, stringify(...args) { serializations++; return JSON.stringify(...args); } } };
vm.runInNewContext(readFileSync(new URL("../../frontend/value-inspector.js", import.meta.url), "utf8"), context);
const { analyze, copyText, limits } = context.window.ValueInspector;
const base64 = text => Buffer.from(text).toString("base64");
const url = value => Buffer.from(JSON.stringify(value)).toString("base64url");
const token = `${url({ alg: "HS256", typ: "JWT" })}.${url({ sub: "José 🔐", exp: 1700000000, nested: { roles: ["admin", "usuário"], html: '<img src=x onerror="alert(1)">' } })}.c2ln`;
const flat = node => [node, ...node.children.flatMap(flat)];
const find = (model, label) => flat(model.root).find(node => node.label === label);

let model = analyze(`Bearer ${token}`, '$["fields"]["authorization"]');
assert.equal(find(model, "sub").value, "José 🔐");
assert.equal(find(model, "exp").value, 1700000000);
assert.equal(find(model, "datas_declaradas_UTC").value.exp, "2023-11-14T22:13:20.000Z");
assert.equal(find(model, "signature_base64url").value, "c2ln");
assert.ok(find(model, "roles").path.includes("::jwt"));
assert.equal(find(model, "html").value, '<img src=x onerror="alert(1)">');
assert.equal(copyText(token), token);
assert.equal(copyText(find(model, "nested").value), JSON.stringify({ roles: ["admin", "usuário"], html: '<img src=x onerror="alert(1)">' }, null, 2));

const embedded = `Authorization: Bearer ${token}; url=https://local.invalid/?token=${token}&next=1`;
model = analyze(embedded);
assert.equal(find(model, "ocorrência_1").value.inicio_UTF16, embedded.indexOf(token));
assert.equal(find(model, "ocorrência_2").value.inicio_UTF16, embedded.lastIndexOf(token));
assert.equal(flat(analyze(Array(12).fill(token).join("; ")).root).filter(node => node.label.startsWith("ocorrência_")).length, 8);

const nested = { password: "fixture-secret", x: { list: [true, null, 3] }, unicode: "你好" };
model = analyze(base64(JSON.stringify(nested)));
assert.equal(find(model, "password").value, "fixture-secret");
assert.ok(find(model, "password").path.includes("::base64::json"));
assert.equal(nested.password, "fixture-secret");
assert.equal(analyze("[oculto]").root.preview, "[oculto]");
assert.match(analyze("[oculto]").notices.join(" "), /original indisponível/);
for (const input of ["not a token", "aaaa.bbbb.cccc", "@@@@@@==", "Zh======", "/w==", "AAAAAA==", "{bad json}", `${url({ alg: "none" })}.${url([1, 2])}.`]) {
  assert.equal(analyze(input).root.children.length, 0, input);
}
assert.equal(find(analyze(`${url({ alg: "none" })}.${url({ a: 1 })}.`), "a").value, 1);
assert.equal(find(analyze(`${url({})}.${url({ exp: 1e100, iat: "not numeric" })}.c2ln`), "datas_declaradas_UTC"), undefined);

const unsafeKeys = JSON.parse('{"__proto__":{"polluted":true},"constructor":{"prototype":{"polluted":true}}}');
assert.equal(find(analyze(unsafeKeys), "polluted").value, true);
assert.equal(JSON.parse(copyText(unsafeKeys)).__proto__.polluted, true);
assert.equal({}.polluted, undefined);
assert.equal(analyze("x".repeat(limits.bytes + 1)).root.limited, true);
assert.equal(analyze("🔐".repeat(limits.bytes / 2)).root.limited, true);
assert.equal(copyText("x".repeat(limits.bytes + 1)), null);
let deep = {}; let cursor = deep;
for (let i = 0; i < 30; i++) { cursor.child = {}; cursor = cursor.child; }
assert.match(analyze(deep).notices.join(" "), /profundidade/);
assert.equal(copyText(deep), null);
const many = Object.fromEntries(Array.from({ length: 2000 }, (_, i) => [String(i), i]));
assert.ok(flat(analyze(many).root).length <= limits.nodes + limits.depth + 1);
assert.match(analyze(many).notices.join(" "), /1.000 nós/);
assert.equal(copyText(many), null);
const cycle = { name: "cycle" }; cycle.self = cycle;
assert.match(analyze(cycle).notices.join(" "), /circular/);
assert.equal(copyText(cycle), null);
let executed = 0;
const getter = Object.defineProperty({}, "secret", { enumerable: true, get() { executed++; return "secret"; } });
assert.match(analyze(getter).notices.join(" "), /dinâmica/);
assert.equal(copyText(getter), null);
const ownHook = Object.defineProperty({ a: 1 }, "toJSON", { get() { executed++; return () => "wrong"; } });
const inheritedHook = Object.create(Object.defineProperty({}, "toJSON", { get() { executed++; return () => "wrong"; } })); inheritedHook.a = 1;
assert.equal(copyText(ownHook), '{\n  "a": 1\n}');
assert.equal(copyText(inheritedHook), '{\n  "a": 1\n}');
const arrayHook = [1]; Object.defineProperty(arrayHook, "toJSON", { value() { executed++; return "wrong"; } });
assert.equal(copyText(arrayHook), "[\n  1\n]");
assert.equal(copyText(new Array(1e7)), null);
const exactNumericJson = '{"id":9007199254740993,"infinite":1e400}';
const exactModel = analyze(exactNumericJson);
// Runtimes without reviver context.source must decline rather than round.
if (find(exactModel, "id")) {
  assert.equal(find(exactModel, "id").value, "9007199254740993");
  assert.equal(find(exactModel, "infinite").value, "1e400");
  assert.match(exactModel.notices.join(" "), /preservados como texto/);
  assert.equal(JSON.parse(copyText(exactModel.root.children[0].value)).id, "9007199254740993");
} else assert.match(exactModel.notices.join(" "), /não interpretada/);
assert.equal(copyText({ nonfinite: Infinity }), null);
const noSource = { ...context, window: {}, JSON: { ...context.JSON, parse: (text, reviver) => JSON.parse(text, (key, value) => reviver ? reviver(key, value) : value) } };
vm.runInNewContext(readFileSync(new URL("../../frontend/value-inspector.js", import.meta.url), "utf8"), noSource);
assert.equal(noSource.window.ValueInspector.analyze(exactNumericJson).root.children.length, 0);
assert.match(noSource.window.ValueInspector.analyze(exactNumericJson).notices.join(" "), /não interpretada/);
const rawUrl = text => Buffer.from(text).toString("base64url");
for (const lexeme of ["9007199254740993", "1e400", "1e-400", "1.0000000000000001", "0.123456789012345678901", "-0", "-0.000e+99", "-1e-400", "1e+0000000000000000400", "1e" + "9".repeat(10000)]) {
  const json = `{"n":${lexeme},"iat":1700000000}`;
  for (const input of [json, `${url({ alg: "none" })}.${rawUrl(json)}.`]) {
    const result = analyze(input);
    const numeric = find(result, "n");
    if (numeric) {
      assert.equal(numeric.value, lexeme, `exact spelling must survive ${lexeme.slice(0, 40)}`);
      assert.match(result.notices.join(" "), /preservados como texto/);
      assert.equal(find(result, "iat").value, 1700000000, "ordinary NumericDate stays numeric");
    } else assert.match(result.notices.join(" "), /não interpretada/);
    const oldRuntime = noSource.window.ValueInspector.analyze(input);
    assert.equal(find(oldRuntime, "n"), undefined, "old runtimes must decline unsafe interpretations");
    assert.match(oldRuntime.notices.join(" "), /não interpretada/);
  }
}
for (const lexeme of ["0", "0.0000e+999999999999999999", "1", "1.0", "1e+0000000000000000003", "0.1", "123.4500", "1e-7", "5e-324", "1700000000", "1.700000000e9"]) {
  for (const inspect of [analyze, noSource.window.ValueInspector.analyze]) {
    const result = inspect(`{"n":${lexeme}}`);
    assert.equal(find(result, "n").value, Number(lexeme), `representable spelling stays numeric: ${lexeme}`);
    assert.equal(result.notices.length, 0);
  }
}
const numbersInStrings = JSON.stringify({ text: '1e-400 and "n":1.0000000000000001 and \\"9007199254740993', "1e400": "-0", normal: 17 });
const textResult = noSource.window.ValueInspector.analyze(numbersInStrings);
assert.equal(find(textResult, "normal").value, 17, "quoted/escaped number spellings are not numeric tokens");
assert.equal(textResult.notices.length, 0);
assert.equal(find(analyze(`${url({ alg: "none" })}.${rawUrl('{"iat":1.700000000e9,"n":1e-400}')}.`), "datas_declaradas_UTC").value.iat, "2023-11-14T22:13:20.000Z");
assert.equal(executed, 0, "inspection/copy must not invoke getters or toJSON");
const getters = {};
for (let i = 0; i < 2000; i++) Object.defineProperty(getters, `getter${i}`, { enumerable: true, get() { executed++; } });
assert.ok(flat(analyze(getters).root).length <= limits.nodes, "accessor placeholders must also respect the node budget");
assert.equal(executed, 0);
const largeKeys = Object.fromEntries(Array.from({ length: 4 }, (_, i) => [String(i) + "x".repeat(130000), {}]));
serializations = 0;
assert.equal(copyText(largeKeys), null);
assert.equal(serializations, 0, "key byte budget must refuse before JSON serialization");
let encoded = JSON.stringify({ leaf: "stop" });
for (let i = 0; i < 10; i++) encoded = base64(encoded);
assert.equal(find(analyze(encoded), "leaf"), undefined, "nested transformations must stop at the configured limit");
console.log("Value inspector unit checks passed (JWT, embedded tokens, JSON, Base64, Unicode, limits, cycles, getters, copy).");
