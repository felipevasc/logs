// Run with an optional app.js path to compare an unchanged baseline with the
// current checkout. Adapted from the v0.11 frontend IPC/auto-columns probes.
// Counts only: native IPC, Rust, rendering and elapsed-time gains are not measured.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const path = resolve(process.argv[2] || fileURLToPath(new URL('../../frontend/app.js', import.meta.url)));
const app = readFileSync(path, 'utf8');
const section = (start, end) => {
  const from = app.indexOf(start), to = app.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `Source section exists: ${start}`);
  return app.slice(from, to);
};
let serializations = 0;
const fields = Object.fromEntries(Array.from({ length: 200 }, (_, index) => [`hidden_${index}`, {
  toJSON() { serializations++; return { text: 'x'.repeat(65536) }; },
}]));
const state = { columns: ['timestamp', 'level', 'source', 'message', ...Object.keys(fields)],
  rows: [{ timestamp: 1, level: 'Info', source: 'log', message: 'short', fields }], visibleCols: [] };
const columnsContext = vm.createContext({ state, fmtTs: String, eventComment: () => '', fillColumnControls() {}, renderTable() {} });
vm.runInContext(section('function cellValue(', '// ------------------------------------------------------------------ tema'), columnsContext);
columnsContext.autoVisibleCols();
assert.deepEqual(Array.from(state.visibleCols), ['timestamp', 'level', 'source', 'message']);

const calls = [];
const apiContext = vm.createContext({ window: {}, state: {}, TextEncoder,
  invoke: async (command, args) => { calls.push(command); return {}; }, caseArgs: async args => args });
vm.runInContext(section('async function api(', '// ------------------------------------------------------------------ helpers de espera'), apiContext);
const filters = [{ column: 'level', op: 'equals_exact', value: 'error' }];
const commands = ['query_page', 'count_filtered', 'stats_events', 'tree_aggs'];
for (const command of commands) await apiContext.api(command, { filters }, { silent: true });
assert.deepEqual(calls.filter(command => command !== 'validate_filters'), commands);
console.log(JSON.stringify({
  app_path: path,
  app_sha256: createHash('sha256').update(app).digest('hex'),
  node: process.version,
  scope: 'Actual app.js functions in Node with native invoke and DOM replaced by stubs; causal JS work counts only. No native or end-to-end timing measurement.',
  automatic_columns: { hidden_fields: 200, bytes_per_structured_field: 65536, structured_value_serializations: serializations, visible_columns: state.visibleCols },
  filtered_page_and_summaries: { commands, invokes: calls, invoke_count: calls.length, preflight_invokes: calls.filter(command => command === 'validate_filters').length },
}, null, 2));
