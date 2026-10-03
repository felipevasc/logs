import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("../../frontend/resource-settings.js", import.meta.url), "utf8");
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const automatic = { schemaVersion: 1, mode: "automatic", memoryLimitMib: null, parallelismLimit: null };
const snapshot = (saved = automatic) => ({
  active: { memoryAvailableMib: 8192, memoryBudgetMib: 2730, duckdbPerInstanceMib: 682, textIndexMib: 341, selectionCacheMib: 341,
    globalParallelism: 7, maximumParallelism: 8, parserThreads: 4, queryThreadsPerSession: 3, textThreads: 1, environmentOverrideMib: null, invalidEnvironmentOverride: false, conservativeBuilder: false },
  activePreferences: automatic, saved, minimumMemoryMib: 128, maximumMemoryMib: 8192, maximumParallelism: 8,
  restartRequired: saved.mode !== "automatic" || saved.parallelismLimit != null, startupWarning: null, savedWarning: null,
});

function fixture() {
  class Node {
    constructor(tag) { this.tagName = tag; this.children = []; this.attrs = {}; this.value = ""; this.disabled = false; this._text = ""; }
    set textContent(value) { this._text = String(value); this.children = []; }
    get textContent() { return this._text + this.children.map(item => item.textContent ?? item).join(""); }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this._text = ""; this.children = children; }
    setAttribute(name, value) { this.attrs[name] = String(value); }
    focus() { document.activeElement = this; }
  }
  const pane = new Node("section"), requests = [];
  const all = (root = pane) => [root, ...root.children.filter(c => c instanceof Node).flatMap(all)];
  const document = { createElement: tag => new Node(tag), getElementById: id => all().find(item => item.id === id), activeElement: null };
  const state = { rows: [{ id: 7 }], datasetRevision: 17, explorerCache: { retained: true }, analysisContext: { id: "same" } };
  const context = vm.createContext({ document, state, window: { __TAURI__: { core: { invoke: (command, args) => { const pending = deferred(); requests.push({ command, args, ...pending }); return pending.promise; } } } } });
  vm.runInContext(source, context);
  const module = context.window.ResourceSettings;
  const control = id => document.getElementById(id);
  const form = () => all().find(item => item.tagName === "form");
  const button = text => all().find(item => item.tagName === "button" && item.textContent.includes(text));
  const edit = (mode, value) => {
    const select = control("resource-memory-mode"); select.value = mode; select.onchange();
    if (value != null) { const input = control("resource-memory-limit"); input.value = value; input.oninput(); }
  };
  const submit = () => form().onsubmit({ preventDefault() {} });
  const open = async (status = snapshot()) => { const ready = module.renderPane(pane); requests.at(-1).resolve(status); await ready; };
  return { module, pane, requests, control, button, state, edit, submit, open, document, all, window: context.window, bridge: context.window.__TAURI__.core };
}

test("resource input validates integers, bounds and automatic mode", () => {
  const f = fixture();
  assert.deepEqual(plain(f.module.preferences("automatic", "invalid", 4096)), automatic);
  assert.deepEqual(plain(f.module.preferences("custom", "128", 4096)), { schemaVersion: 1, mode: "custom", memoryLimitMib: 128, parallelismLimit: null });
  for (const value of ["", "-1", "127", "4097", "512.5", "1e3", " 512 ", "Infinity", "9007199254740992"]) {
    assert.throws(() => f.module.preferences("custom", value, 4096));
  }
  assert.throws(() => f.module.preferences("other", "512", 4096));
});
test("repeated tab opens deduplicate reads and preserve unsaved preference", async () => {
  const f = fixture();
  const a = f.module.renderPane(f.pane), b = f.module.renderPane(f.pane);
  assert.equal(f.requests.length, 1); f.requests[0].resolve(snapshot()); await Promise.all([a, b]);
  f.edit("custom", "512");
  await f.module.renderPane(f.pane);
  assert.equal(f.requests.length, 1); assert.equal(f.control("resource-memory-limit").value, "512");
  assert.equal(f.control("resource-memory-mode").value, "custom");
  assert.equal(f.button("Descartar").disabled, false);
  f.button("Descartar").onclick();
  assert.equal(f.control("resource-memory-mode").value, "automatic");
  assert.equal(f.requests.length, 1, "discard is local; it cannot change a saved setting");
});
test("save is explicit, deduplicated and leaves dataset and active resources unchanged", async () => {
  const f = fixture(); await f.open();
  const before = plain(f.state);
  f.edit("custom", "512"); assert.equal(f.requests.length, 1);
  f.submit(); f.submit();
  assert.equal(f.requests.length, 2); assert.equal(f.requests[1].command, "resource_settings_save");
  assert.deepEqual(plain(f.requests[1].args), { preferences: { schemaVersion: 1, mode: "custom", memoryLimitMib: 512, parallelismLimit: null } });
  assert.equal(f.control("resource-memory-mode").disabled, true);
  f.pane.hidden = true; f.requests[1].resolve(snapshot(f.requests[1].args.preferences)); await tick();
  assert.equal(f.pane.hidden, true, "a save receipt cannot reopen settings");
  assert.deepEqual(plain(f.state), before);
  assert.match(f.pane.textContent, /2\.730 MiB/); assert.match(f.pane.textContent, /aguardando o próximo início/);
  assert.equal(f.control("resource-memory-limit").value, "512");
  assert.match(f.control("resource-settings-feedback").textContent, /Preferência salva/);
});
test("failed save retains editable draft and can retry, invalid input sends nothing", async () => {
  const f = fixture(); await f.open(); f.edit("custom", "1"); f.submit();
  assert.equal(f.requests.length, 1); assert.match(f.pane.textContent, /entre 128 e 8192/);
  f.edit("custom", "256"); f.submit(); f.requests[1].reject(Error("disk full")); await tick();
  assert.match(f.pane.textContent, /disk full/); assert.equal(f.control("resource-memory-limit").value, "256");
  assert.equal(f.control("resource-memory-limit").disabled, false);
  f.submit(); assert.equal(f.requests.length, 3);
  f.requests[2].resolve(snapshot(f.requests[2].args.preferences)); await tick();
  assert.doesNotMatch(f.pane.textContent, /disk full/);
});
test("failed read can retry and warnings explain environment priority and memory semantics", async () => {
  const f = fixture(); const pending = f.module.renderPane(f.pane);
  f.requests[0].reject(Error("offline")); await pending;
  assert.match(f.pane.textContent, /offline/); f.button("Tentar novamente").onclick();
  const status = snapshot(); status.active.environmentOverrideMib = 512;
  status.startupWarning = "Preferência inválida; automático."; status.savedWarning = status.startupWarning;
  f.requests[1].resolve(status); await tick();
  assert.match(f.pane.textContent, /tem prioridade/);
  assert.match(f.pane.textContent, /não reserva RAM/);
  assert.match(f.pane.textContent, /não é um teto de memória total/);
  assert.equal(f.pane.textContent.split(status.startupWarning).length - 1, 1);
});
test("a synchronously unavailable native bridge does not trap future retries", async () => {
  const f = fixture(), invoke = f.bridge.invoke;
  f.bridge.invoke = () => { throw Error("bridge unavailable"); };
  await f.module.renderPane(f.pane);
  assert.match(f.pane.textContent, /bridge unavailable/);
  f.bridge.invoke = invoke; f.button("Tentar novamente").onclick();
  assert.equal(f.requests.length, 1); f.requests[0].resolve(snapshot()); await tick();
  assert.equal(f.control("resource-memory-mode").value, "automatic");
});
test("typing enables discard and automatic save cancels a pending custom preference", async () => {
  const saved = { schemaVersion: 1, mode: "custom", memoryLimitMib: 512, parallelismLimit: null };
  const f = fixture(); await f.open(snapshot(saved)); assert.equal(f.button("Descartar").disabled, true);
  const input = f.control("resource-memory-limit"); input.value = "768"; input.oninput();
  assert.equal(f.button("Descartar").disabled, false); f.button("Descartar").onclick();
  assert.equal(f.control("resource-memory-limit").value, "512");
  f.edit("automatic"); f.submit(); assert.deepEqual(plain(f.requests[1].args.preferences), automatic);
  f.requests[1].resolve(snapshot()); await tick(); assert.doesNotMatch(f.pane.textContent, /aguardando o próximo início/);
});
test("native wiring resolves profile before freezing preferences and includes bundled UI", () => {
  const lib = readFileSync(new URL("../../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const run = lib.slice(lib.indexOf("pub fn run()"));
  assert.ok(run.indexOf("profiles::initialize") < run.indexOf("resource_settings::initialize"));
  assert.ok(run.indexOf("updates::prepare") < run.indexOf("resource_settings::initialize"));
  assert.ok(run.indexOf("resource_settings::initialize") < run.indexOf("resources::init()"));
  assert.match(lib, /resource_settings::resource_settings_status/); assert.match(lib, /resource_settings::resource_settings_save/);
  const html = readFileSync(new URL("../../frontend/index.html", import.meta.url), "utf8");
  assert.match(html, /data-settings-tab="resources"/); assert.match(html, /src="resource-settings\.js"/);
  assert.match(html, /href="resource-settings\.css"/);
  const bundle = readFileSync(new URL("../prepare-frontend.mjs", import.meta.url), "utf8"); assert.match(bundle, /"resource-settings\.js"/);
  assert.doesNotMatch(source, /\bapi\(/);
});


const caseSnapshot = (caseId, revision = 0) => ({ schemaVersion: 1, caseId, analysisId: `analysis-${caseId}`, configRevision: revision, visibilityRevision: 0,
  config: { derivedFields: [], references: [] }, migrationDiagnostics: [] });
const caseStatus = (caseId, revision = 0, workLimitMib = null) => ({
  analysisContext: caseSnapshot(caseId, revision), preferences: { schemaVersion: 1, mode: workLimitMib == null ? "inherit" : "custom", workLimitMib },
  effective: { accountedLimitMib: workLimitMib ?? 1152, workLiveMib: Math.min(workLimitMib ?? 128, 128), selectionMib: workLimitMib ?? 1024,
    materializedMib: 64, analyticsMib: 32, collectedIdsMib: 32, selectionCacheMib: 128, applicationWorkMib: 128, applicationSelectionMib: 1024 },
  minimumWorkMib: 8, maximumWorkMib: 8192, maximumEffectiveWorkMib: 1152, clamped: false,
});
function attachCases(f) {
  const saved = new Map([['a', caseSnapshot('a')], ['b', caseSnapshot('b')]]); let active = 'a';
  const identity = id => { const s = saved.get(id); return { caseId: id, analysisId: s.analysisId, configRevision: s.configRevision, visibilityRevision: s.visibilityRevision }; };
  const capture = () => ({ caseId: active, instance: active, identity: identity(active) });
  const same = (a,b) => JSON.stringify(a) === JSON.stringify(b);
  f.window.AnalysisContexts = { capture, owns: owner => saved.get(owner.caseId)?.analysisId === owner.identity.analysisId,
    isCurrent: owner => same(owner, capture()), assertOwner: owner => { if (!same(owner,capture())) throw Error('stale'); },
    adopt: async (snapshot, { owner }) => { if (snapshot.caseId !== owner.caseId) throw Error('wrong owner'); saved.set(owner.caseId, snapshot); },
  };
  return { saved, switch: id => { active = id; return f.module.renderPane(f.pane); },
    edit: (mode, value) => { const select=f.control('case-resource-mode'); select.value=mode; select.onchange(); if(value != null) { const input=f.control('case-resource-work'); input.value=value; input.oninput(); } },
    submit: () => f.all().find(item => item.tagName === 'form' && item.children.some(child => child.children?.some(button => button.id === 'case-resource-save'))).onsubmit({preventDefault(){}}),
  };
}
test('Case quota validation uses logical volume and preserves inherit semantics', () => {
  const f=fixture();
  assert.deepEqual(plain(f.module.casePreferences('inherit','bad',1152)), {schemaVersion:1,mode:'inherit',workLimitMib:null});
  assert.deepEqual(plain(f.module.casePreferences('custom','8',1152)), {schemaVersion:1,mode:'custom',workLimitMib:8});
  for (const value of ['7','1153','1e3','8.5',' 16 ','']) assert.throws(()=>f.module.casePreferences('custom',value,1152));
});
test('Case A late read cannot overwrite Case B controls', async () => {
  const f=fixture(); await f.open(); const cases=attachCases(f);
  const a=f.module.renderPane(f.pane); assert.equal(f.requests[1].command,'case_resource_settings_status');
  const b=cases.switch('b'); assert.equal(f.requests[2].args.identity.caseId,'b');
  f.requests[2].resolve(caseStatus('b',0,32)); await b;
  f.requests[1].resolve(caseStatus('a',0,64)); await a;
  assert.equal(f.control('case-resource-work').value,'32');
  assert.match(f.pane.textContent,/volume lógico contabilizado/); assert.match(f.pane.textContent,/Não configura RAM do DuckDB/);
});
test('Case save is CAS-bound, deduplicated, and A receipt never overwrites B', async () => {
  const f=fixture(); await f.open(); const cases=attachCases(f);
  const a=f.module.renderPane(f.pane); f.requests[1].resolve(caseStatus('a')); await a;
  cases.edit('custom','16'); cases.submit(); cases.submit();
  assert.equal(f.requests.length,3); assert.equal(f.requests[2].command,'case_resource_settings_save');
  assert.equal(f.requests[2].args.expected.caseId,'a'); assert.equal(f.requests[2].args.expected.configRevision,0);
  const b=cases.switch('b'); f.requests[3].resolve(caseStatus('b',0,32)); await b;
  f.requests[2].resolve(caseStatus('a',1,16)); await tick();
  assert.equal(cases.saved.get('a').configRevision,1);
  assert.equal(f.control('case-resource-work').value,'32');
});
test('Case save failure retains draft and successful save adopts revision', async () => {
  const f=fixture(); await f.open(); const cases=attachCases(f);
  const ready=f.module.renderPane(f.pane); f.requests[1].resolve(caseStatus('a')); await ready;
  cases.edit('custom','7'); cases.submit(); assert.equal(f.requests.length,2);
  cases.edit('custom','16'); cases.submit(); f.requests[2].reject(Error('CAS changed')); await tick();
  assert.equal(f.control('case-resource-work').value,'16'); assert.match(f.pane.textContent,/CAS changed/);
  cases.submit(); f.requests[3].resolve(caseStatus('a',1,16)); await tick();
  assert.equal(cases.saved.get('a').configRevision,1); assert.match(f.pane.textContent,/Cota salva neste Caso/);
  assert.equal(f.control('resource-memory-mode').value,'automatic','Case save must not alter application defaults');
});


test("global parallelism validates independently from automatic memory", () => {
  const f = fixture();
  assert.deepEqual(plain(f.module.preferences("automatic", "invalid", 4096, "1", 8)), { ...automatic, parallelismLimit: 1 });
  for (const value of ["0", "9", "1.5", "1e0", " 1", "", "NaN"]) assert.throws(() => f.module.preferences("automatic", "", 4096, value, 8));
  assert.equal(f.module.parallelismPreference(null, 8), null);
});
test("parallelism-only save keeps active capacity until restart and discard restores saved value", async () => {
  const f = fixture(); await f.open();
  const mode = f.control("resource-parallelism-mode"); mode.value = "custom"; mode.onchange();
  const input = f.control("resource-parallelism-limit"); input.value = "2"; input.oninput();
  await f.module.renderPane(f.pane); assert.equal(f.control("resource-parallelism-limit").value, "2");
  f.submit(); f.submit(); assert.equal(f.requests.length, 2);
  assert.deepEqual(plain(f.requests[1].args.preferences), { ...automatic, parallelismLimit: 2 });
  assert.equal(f.control("resource-parallelism-limit").disabled, true);
  f.requests[1].resolve(snapshot({ ...automatic, parallelismLimit: 2 })); await tick();
  assert.match(f.pane.textContent, /Paralelismo global ativo: 7/);
  assert.match(f.pane.textContent, /aguardando o próximo início/);
  f.control("resource-parallelism-limit").value = "3"; f.control("resource-parallelism-limit").oninput();
  f.button("Descartar").onclick(); assert.equal(f.control("resource-parallelism-limit").value, "2");
  const automaticMode = f.control("resource-parallelism-mode"); automaticMode.value = "automatic"; automaticMode.onchange();
  f.submit(); assert.deepEqual(plain(f.requests[2].args.preferences), automatic);
  f.requests[2].resolve(snapshot()); await tick();
  assert.equal(f.control("resource-parallelism-limit").disabled, true);
});


test("large detected-memory limits are accepted with a non-blocking full-memory warning", async () => {
  for (const maximum of [65536, 131072]) {
    const f = fixture(), status = snapshot();
    status.maximumMemoryMib = maximum; status.active.memoryAvailableMib = maximum;
    await f.open(status);
    assert.equal(f.control("resource-memory-limit").max, String(maximum));
    f.edit("custom", String(maximum));
    assert.match(f.control("resource-memory-warning").textContent, /A escolha é permitida/);
    assert.match(f.pane.textContent, /não é a RAM livre/);
    f.submit();
    assert.equal(f.requests[1].args.preferences.memoryLimitMib, maximum);
    const saved = { ...status, saved: f.requests[1].args.preferences, restartRequired: true };
    f.requests[1].resolve(saved); await tick();
    assert.equal(f.control("resource-memory-limit").value, String(maximum));
    f.edit("custom", String(maximum + 1)); f.submit();
    assert.equal(f.requests.length, 2, "only above-machine capacity must be rejected");
    f.edit("custom", "512");
    assert.equal(f.control("resource-memory-warning").textContent, "");
    f.edit("automatic");
    assert.equal(f.control("resource-memory-warning").textContent, "");
  }
});
test("a moved 128 GiB preference is shown unchanged while active memory is clamped", async () => {
  const saved = { ...automatic, mode: "custom", memoryLimitMib: 131072 };
  const status = snapshot(saved); status.activePreferences = saved; status.restartRequired = false;
  status.active.memoryBudgetMib = status.maximumMemoryMib;
  const f = fixture(); await f.open(status);
  assert.equal(f.control("resource-memory-limit").value, "131072");
  assert.match(f.pane.textContent, /máquina com mais memória/);
  assert.match(f.pane.textContent, /orçamento ativo usa o total/);
  f.edit("custom", "8192"); f.submit();
  assert.equal(f.requests[1].args.preferences.memoryLimitMib, 8192);
});
test("Case accepts a detected-memory-sized preference but clearly reports its effective component limits", async () => {
  const f = fixture(); await f.open(); const cases = attachCases(f);
  const status = caseStatus('a'); status.maximumWorkMib = 131072;
  const opened = f.module.renderPane(f.pane); f.requests[1].resolve(status); await opened;
  assert.equal(f.control('case-resource-work').max, '131072');
  cases.edit('custom', '131072'); cases.submit();
  assert.equal(f.requests[2].args.preferences.workLimitMib, 131072);
  f.requests[2].resolve({ ...status, analysisContext: caseSnapshot('a', 1),
    preferences: f.requests[2].args.preferences, clamped: true }); await tick();
  assert.equal(f.control('case-resource-work').value, '131072');
  assert.match(f.pane.textContent, /Preferência salva do Caso: 131\.072 MiB/);
  assert.match(f.pane.textContent, /Cota lógica efetiva do Caso: 1\.152 MiB/);
  assert.match(f.pane.textContent, /não amplia os motores/);
  assert.match(f.pane.textContent, /orçamento de memória do aplicativo pode ser ajustado abaixo/);
  cases.edit('custom', '131073'); cases.submit(); assert.equal(f.requests.length, 3);
  assert.throws(() => f.module.casePreferences('custom', '65536'), 'a missing native maximum must fail closed');
});
