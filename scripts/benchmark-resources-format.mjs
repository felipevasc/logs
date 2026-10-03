// Formatting microbenchmark, not an end-to-end modal/WebView benchmark.
// Run from the repository root: node scripts/benchmark-resources-format.mjs
import assert from "node:assert/strict";
const number = value => typeof value === "number" && Number.isFinite(value);
const formatter = new Intl.NumberFormat("pt-BR", { maximumFractionDigits: 1 });
const previous = value => number(value) ? value.toLocaleString("pt-BR", { maximumFractionDigits: 1 }) : "—";
const cached = value => number(value) ? formatter.format(value) : "—";
const values = Array.from({ length: 20_000 }, (_, index) => index * 1.2345);
for (const value of [...values, null, undefined, NaN, Infinity, -Infinity, -0, -1234.56, 1e20]) {
  assert.equal(cached(value), previous(value));
}
const measure = fn => {
  const started = performance.now();
  let characters = 0;
  for (const value of values) characters += fn(value).length;
  return { ms: performance.now() - started, characters };
};
// Exercise both functions before measured rounds.
for (const value of values.slice(0, 1000)) { previous(value); cached(value); }
const legacyMs = [], cachedMs = [];
for (let round = 0; round < 5; round++) {
  const first = measure(round % 2 ? cached : previous);
  const second = measure(round % 2 ? previous : cached);
  assert.equal(first.characters, second.characters);
  legacyMs.push(round % 2 ? second.ms : first.ms);
  cachedMs.push(round % 2 ? first.ms : second.ms);
}
const median = samples => [...samples].sort((a, b) => a - b)[Math.floor(samples.length / 2)];
console.log(JSON.stringify({
  benchmark: "resource_decimal_formatting", runtime: process.version, platform: process.platform,
  values: values.length, rounds: legacyMs.length, allOutputsMatch: true,
  legacyMs, cachedMs, legacyMedianMs: median(legacyMs), cachedMedianMs: median(cachedMs),
  medianSpeedup: median(legacyMs) / median(cachedMs),
  notes: "Microbenchmark Node/V8 de formatação de 20 mil números; não representa o tempo do modal, IPC, layout, WebView ou consultas de logs."
}, null, 2));
