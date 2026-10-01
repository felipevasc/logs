import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const section = (start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
const settle = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };

function harness(store = { active: 'case', cases: [{ id: 'case', artifacts: [] }] }) {
  let timer, persisted, finishSave, failSave, rowsPainted = false, completed = false;
  const events = [], nodes = new Map();
  const state = { cases: structuredClone(store), artifactSessions: new Map(), artifactSwitchVersion: 0, loaded: false, columns: [], visibleCols: [], filters: [], quick: '', total: null };
  const node = selector => {
    if (!nodes.has(selector)) nodes.set(selector, { classList: { add() {} }, value: '', hidden: false, appendChild() {} });
    return nodes.get(selector);
  };
  const context = vm.createContext({
    state, window: {}, document: { body: { dataset: { page: 'explore' } } }, console,
    setTimeout: callback => (timer = callback, 1), clearTimeout: () => { timer = null; },
    $: node, fmtNum: String, toast: message => events.push(message),
    activeCase: () => state.cases.cases.find(item => item.id === state.cases.active),
    ensureCase: () => state.cases.cases[0], caseArtifacts: c => c.artifacts,
    artifactIdFromSource: value => `file:${value.path}`,
    applySourceSpec() {}, renderArtifactBar() {}, renderCaseBar() {}, updateContextBar() {},
    startOperation() {}, updateOperation() {}, skeletonRows() {},
    showLoadOverlay: () => { state.loadOverlay = true; },
    hideLoadOverlay: ok => { state.loadOverlay = false; completed = ok; },
    finishOperation: (label, detail) => events.push(`${label}: ${detail}`),
    fillColumnControls() {}, renderChips() {}, syncCurrentSavedFilter() {}, restoreCurrentSavedFilter() {},
    restoreVisiblePreferences: () => false, loadTsConfig: async () => {}, caseStations: () => [],
    autoVisibleCols() {}, updateTsExample() {}, renderExploreTree() {},
    refresh: async () => { rowsPainted = true; state.rows = [{ id: 1 }]; return true; },
    api: async (cmd, args) => {
      events.push(cmd);
      if (cmd === 'load_file') return { count: 50_000_000, columns: ['timestamp', 'message'], source_desc: args.path };
      if (cmd === 'profile_fields') return [];
      if (cmd === 'cases_save') {
        const snapshot = structuredClone(args.data);
        await new Promise((resolve, reject) => { finishSave = resolve; failSave = reject; });
        persisted = snapshot;
      }
    },
  });
  for (const code of [
    section('function setQuickSearchDraft(', 'function commitQuickSearch('),
    section('function sourceSpecFromArtifact(', '\nfunction applySourceSpec('),
    section('function artifactSessionFor(', '\nconst baseName'),
    section('function storeCurrentArtifactInSession()', '\nfunction updateContextBar()'),
    section('function registerCurrentArtifact(', '\nfunction activeStation()'),
    section('let casesSaveQueue =', '\nfunction defaultCaseWorkspace()'),
    section('function clearSourceRecovery()', '\nasync function clearData('),
  ]) vm.runInContext(code, context);
  return {
    context, state, events, get persisted() { return persisted; }, get rowsPainted() { return rowsPainted; }, get completed() { return completed; },
    flush: async () => { const save = timer; timer = null; assert.ok(save, 'artifact persistence was enqueued'); save(); await settle(); },
    commit: async () => { finishSave(); await settle(); }, fail: async () => { failSave(Error('disk full')); await settle(); },
  };
}

const first = harness();
let ready = false;
const opening = first.context.loadData({ kind: 'file', path: '/logs/large.jsonl' }).then(result => { ready = result; });
await settle();
assert.equal(first.rowsPainted, true, 'first-page paint precedes metadata persistence');
assert.equal(first.state.loaded, true, 'the source is available while saving');
assert.equal(first.state.loadOverlay, true, 'initial load remains in progress until it can survive restart');
assert.equal(ready, false);
await first.flush();
assert.equal(ready, false, 'issuing cases_save is insufficient: wait for durable acknowledgment');
await first.commit(); await opening;
assert.equal(ready, true); assert.equal(first.completed, true);
assert.equal(first.persisted.cases[0].activeArtifactId, 'file:/logs/large.jsonl');

// An immediate restart uses the actual saved artifact restoration path, without a preview auto-load.
const reopened = harness(first.persisted);
const reopening = reopened.context.syncActiveCaseArtifacts();
await settle();
assert.equal(reopened.state.loaded, true);
assert.equal(reopened.state.currentArtifact.source.path, '/logs/large.jsonl');
assert.equal(reopened.rowsPainted, true);
await reopened.flush(); await reopened.commit(); await reopening;

const failed = harness();
const failedOpening = failed.context.loadData({ kind: 'file', path: '/logs/large.jsonl' });
await settle(); await failed.flush(); await failed.fail(); await failedOpening;
assert.equal(failed.rowsPainted, true, 'save failure preserves usable rows');
assert.equal(failed.completed, false, 'save failure never shows the successful load animation');
assert.ok(failed.events.some(message => message.includes('sessão não salva')));
assert.ok(failed.events.some(message => message.includes('Reabra a fonte manualmente')));
assert.ok(!failed.events.some(message => message.startsWith('Artefato pronto:')), 'failed persistence never claims a restart-safe load');
console.log('Initial source persistence, rows-before-save, immediate restoration and save failure passed');
