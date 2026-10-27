import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
const sandbox = { window: {}, structuredClone, console, performance };
vm.runInNewContext(readFileSync(new URL("../../frontend/columns-engine.js", import.meta.url), "utf8"), sandbox);
const E = sandbox.window.ColumnsEngine;
const JAVA = [
  "2026-10-23 17:05:09,120 ERROR [pool-3] br.app.Pedido - Falha ao salvar pedido 8841 (cliente 77) em 12.5ms\njava.lang.IllegalStateException: estoque zerado (sku 42) (lote A7) (filial 3)\n\tat br.app.Estoque.baixar(Estoque.java:88)\n\tat br.app.Pedido.salvar(Pedido.java:41)",
  "2026-10-23 17:05:44,410 INFO [pool-1] br.app.Login - usuario=bruno entrou de 192.168.1.20 em 3ms",
  "2026-10-23 17:06:12,001 ERROR [pool-2] br.app.Pagamento - Timeout no gateway 10.0.0.9 (tentativa 3) em 250ms\njava.net.SocketTimeoutException: Read timed out (host gw-1) (porta 443) (filial 9)\n\tat br.app.Gateway.chamar(Gateway.java:130)",
];
const ctx = (records) => ({ records, existing: [] });
/** O primeiro resultado do padrão tem de ser exatamente o trecho escolhido (partes: dentro dele). */
const reproduces = (c, rec, s, e) => {
  const m = new RegExp(c.re, "d").exec(rec);
  return c.part ? !!m && m.indices[1][0] >= s && m.indices[1][1] <= e : !!m && m.indices[1][0] === s && m.indices[1][1] === e;
};

test("o terceiro parênteses da segunda linha é localizado pela linha e pela posição, e cada sugestão o reproduz", () => {
  const rec = JAVA[0], s = rec.indexOf("filial 3"), e = s + "filial 3".length;
  const list = E.candidates(rec, s, e, ctx(JAVA));
  assert.ok(list.length >= 8);
  assert.ok(list.some((c) => /2ª linha/.test(c.label)) && list.some((c) => /3º|último/.test(c.label)));
  for (const c of list) assert.ok(reproduces(c, rec, s, e), c.label);
});

test("toda seleção tem ao menos uma sugestão, mesmo que seja o texto exato ou a posição", () => {
  let total = 0;
  for (const rec of JAVA) for (let s = 0; s < rec.length; s += 31) for (const len of [2, 9, 40]) {
    const e = Math.min(rec.length, s + len);
    if (e <= s) continue;
    total++;
    const list = E.candidates(rec, s, e, ctx(JAVA));
    assert.ok(list.length >= 1, JSON.stringify(rec.slice(s, e)));
    for (const c of list) assert.ok(reproduces(c, rec, s, e), `${c.label} :: ${JSON.stringify(rec.slice(s, e))}`);
  }
  assert.ok(total > 40);
});

test("cada sugestão oferece as formas Grok e Regex com o mesmo nome de grupo", () => {
  const rec = JAVA[1], s = rec.indexOf("192.168.1.20"), e = s + "192.168.1.20".length;
  const list = E.candidates(rec, s, e, ctx(JAVA));
  const typed = list.find((c) => c.kind === "tipo");
  assert.ok(typed, "reconhece o tipo IP");
  const forms = typed.forms("ip_origem");
  assert.match(forms.regex, /\(\?<ip_origem>/);
  assert.match(forms.grok, /%\{\w+:ip_origem\}/);
  // padrões próprios do Grok têm nome estável e a definição que a regex usa
  const own = list.find((c) => c.forms("x").custom);
  if (own) { const f = own.forms("x"); assert.match(f.custom.name, /^[A-Z0-9_]+_[0-9A-F]{1,4}$/); assert.equal(f.custom.name, own.forms("y").custom.name); assert.ok(f.custom.src.length > 0); }
});

test("os tipos do catálogo reconhecem os seus exemplos", () => {
  const known = E.TYPES.filter((t) => t.re && t.ex && !/…/.test(t.ex));
  assert.ok(known.length >= 25);
  for (const t of known) assert.ok(new RegExp(`^(?:${t.re})$`).test(t.ex), `${t.id}: ${t.ex}`);
});

test("sugestões de nome seguem a palavra anterior, o tipo e nomes comuns", () => {
  const rec = JAVA[1], s = rec.indexOf("bruno"), e = s + 5;
  const first = E.candidates(rec, s, e, ctx(JAVA))[0];
  const names = E.nameSuggestions(first);
  assert.ok(names.length >= 1 && names.every((n) => /^[\w.@]+$/.test(n)), names.join());
  assert.equal(E.clean("Endereço IP"), "endereco_ip");
});

test("registros longos (JSON de ~2 KB) continuam sugerindo em tempo de interface", () => {
  const mk = (i) => JSON.stringify({ "@timestamp": `2026-10-23T17:05:${String(i % 60).padStart(2, "0")}.123Z`, level: i % 3 ? "INFO" : "ERROR", host: { name: `srv-${i}`, ip: `10.0.${i % 255}.${(i * 7) % 255}` }, message: "x".repeat(300) + ` pedido ${8000 + i} falhou (sku ${i}) ` + "lorem ipsum dolor sit amet ".repeat(30), user: { email: `ana${i}@exemplo.com` }, payload: "y".repeat(400) });
  const records = Array.from({ length: 60 }, (_, i) => mk(i)), rec = records[5];
  const t0 = performance.now();
  for (const re of [/10\.0\.\d+\.\d+/, /pedido (\d+)/, /srv-\d+/, /[\w]+@exemplo\.com/]) {
    const m = re.exec(rec), text = m[1] ?? m[0], s = rec.indexOf(text), e = s + text.length;
    const list = E.candidates(rec, s, e, ctx(records));
    assert.ok(list.length >= 8, text);
    for (const c of list) assert.ok(reproduces(c, rec, s, e), `${c.label} :: ${text}`);
  }
  assert.ok(performance.now() - t0 < 6000, `4 seleções levaram ${Math.round(performance.now() - t0)} ms`);
});
