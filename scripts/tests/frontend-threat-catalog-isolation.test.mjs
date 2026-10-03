import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../preview/mock-threats.js', import.meta.url), 'utf8');
const rule = (id, pattern, enabled = true) => ({ id, pattern, enabled, name: id, category: 'test', severity: 'medium', kind: 'indicator', description: 'fixture', references: [] });
test('preview threat compilation and additive updates retain the explicitly captured Case catalog', async () => {
  const beforeFetch = globalThis.fetch, beforeWindow = globalThis.window;
  const bundled = { version: 1, name: 'Builtin', rules: [rule('builtin', 'BUILTIN')] };
  globalThis.fetch = async () => ({ ok: true, headers: { get: () => '' }, json: async () => structuredClone(bundled) });
  globalThis.window = { __mockThreatCatalog: { ...bundled, name: 'Unrelated global override' } };
  try {
    const module = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
    const a = { version: 1, name: 'Case A', rules: [rule('shared', 'ALPHA'), rule('off', 'OFF', false)] };
    const b = { version: 1, name: 'Case B', rules: [rule('shared', 'BRAVO')] };
    const event = { id: 1, event_ref: 'fixture:1', message: 'ALPHA', source: 'fixture', fields: {}, timestamp: 1 };
    const [left, right] = await Promise.all([module.threatScan([event], [], a), module.threatScan([event], [], b)]);
    assert.equal(left.matched, 1); assert.equal(right.matched, 0);
    const update = await module.threatCatalogUpdate(a);
    assert.equal(update.catalog.name, 'Case A'); assert.equal(update.added, 1);
    assert.deepEqual(update.catalog.rules.slice(0, 2), a.rules);
    assert.equal(a.rules.length, 2); assert.equal(b.rules.length, 1);
    assert.equal((await module.threatCatalog(b)).name, 'Case B');
    assert.equal((await module.threatCatalog()).name, 'Builtin');
  } finally { globalThis.fetch = beforeFetch; globalThis.window = beforeWindow; }
});
