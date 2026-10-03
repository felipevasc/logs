/* Source protocol contract, not native execution or browser playback. */
import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
const native = read('src-tauri/src/lib.rs'), engine = read('src-tauri/src/engine/mod.rs'), builder = read('src-tauri/src/engine/build.rs');
const adapter = native.match(/fn engine_progress_phase_id\(phase: &str\) -> &'static str \{([\s\S]*?)\n\}/)?.[1];
assert.ok(adapter);
const phaseIds = new Map();
for (const [, labels, id] of adapter.matchAll(/((?:"[^"]+"\s*\|\s*)*"[^"]+")\s*=>\s*"([^"]+)"/g))
  for (const [, label] of labels.matchAll(/"([^"]+)"/g)) phaseIds.set(label, id);
const window = {};
vm.runInNewContext(read('frontend/waiting-progress.js'), { window });
vm.runInNewContext(read('frontend/waiting-visuals.js'), { window });
function firstArgument(source, start) {
  let depth = 0, quoted = false, escaped = false;
  for (let i = start; i < source.length; i++) {
    const character = source[i];
    if (quoted) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === '"') quoted = false;
    } else if (character === '"') quoted = true;
    else if ('({['.includes(character)) depth++;
    else if (')}]'.includes(character)) { if (depth === 0) return source.slice(start, i); depth--; }
    else if (character === ',' && depth === 0) return source.slice(start, i);
  }
  throw Error('Unterminated native progress call');
}
test('every current engine label maps to an explicit stable transport ID', () => {
  const prepare = engine.slice(engine.indexOf('fn prepare_variant('), engine.indexOf('/// Store size per 1000 bytes'));
  const emitted = new Set([...builder.matchAll(/\bphase\("([^"]+)"\)/g)].map(m => m[1]));
  for (const [, label] of prepare.matchAll(/phase:\s*"([^"]+)"/g)) emitted.add(label);
  for (const call of prepare.matchAll(/\bpublish\(/g))
    for (const [, label] of firstArgument(prepare, call.index + call[0].length).matchAll(/"([^"]+)"/g)) emitted.add(label);
  assert.equal(emitted.size, 17, 'review coverage if native phase repertoire changes');
  assert.deepEqual([...emitted].sort(), [...phaseIds.keys()].sort());
  assert.match(adapter, /_\s*=>\s*"engine-unknown"/);
  const transport = native.slice(native.indexOf('pub(crate) fn prepare_engine('), native.indexOf('async fn engine_status('));
  assert.match(transport, /"phaseId":\s*engine_progress_phase_id\(&p\.phase\)/);
  assert.match(transport, /"phase":\s*p\.phase/);
});
test('engine receipts retain human labels and truthful counts while choosing recognized scenes', () => {
  const families = { 'engine-validate':'verification', 'engine-validated':'verification', 'engine-restore':'restoration',
    'engine-prepare':'reading','engine-columns':'reading','engine-index':'reading','engine-checkpoint-write':'checkpoint',
    'engine-text-merge':'calculation','engine-checkpoint-sync':'checkpoint','engine-checkpoint-publish':'checkpoint',
    'engine-checkpoint-committed':'checkpoint','engine-open':'restoration','engine-ready':'restoration' };
  for (const [label, phaseId] of phaseIds) {
    const receipt = window.WaitingProgress.snapshot({operationId:'load-native',operation:'carregamento',phaseId,phase:label,
      completed:1200,total:6300,unit:'registros',elapsedMs:48000});
    const result=window.WaitingVisuals.derive(receipt);
    assert.equal(receipt.phaseId,phaseId); assert.equal(receipt.label,label); assert.equal(result.state,'running');
    assert.equal(result.family,families[phaseId]||'neutral');
    assert.equal(result.canAnimate,!!families[phaseId]&&phaseId!=='engine-checkpoint-committed');
    assert.equal(receipt.completed,1200); assert.equal(receipt.total,6300);
  }
  for(const phaseId of ['engine-unknown','engine-made-up','Publicando checkpoint validado'])
    assert.equal(window.WaitingVisuals.derive({operationId:'load-native',phaseId,state:'running',elapsedMs:48000}).canAnimate,false);
});
test('committed segment and full counters never complete the owning source operation', () => {
  for(const phaseId of ['engine-checkpoint-publish','engine-checkpoint-committed','engine-ready']) {
    const receipt=window.WaitingProgress.snapshot({operationId:'load-native',operation:'carregamento',phaseId,phase:'Etapa do motor',completed:100,total:100,unit:'registros',state:'ready'});
    assert.equal(receipt.state,'running'); assert.equal(window.WaitingVisuals.derive(receipt).partialCheckpoint,phaseId==='engine-checkpoint-committed');
  }
});
test('source transitions use exact stable IDs and settlement remains pending until its owner resolves', () => {
  const emit=native.slice(native.indexOf('fn emit_progress('),native.indexOf('pub(crate) fn config_dir('));
  assert.match(emit,/phase_id: match \(operation, phase\)/); assert.match(emit,/_ => phase/); assert.match(emit,/phase: phase\.into\(\)/);
  for(const [phaseId,family] of [['source-prepare','reading'],['source-indexed','reading'],['source-activate','restoration'],['source-settle','restoration']]) {
    assert.ok(emit.includes(`"${phaseId}"`));
    const receipt=window.WaitingProgress.snapshot({operationId:'source-load',operation:'carregamento',phaseId,phase:'Texto humano',elapsedMs:6000});
    assert.equal(receipt.state,'running'); assert.equal(window.WaitingVisuals.derive(receipt).family,family);
    assert.equal(receipt.label,phaseId==='source-settle'?'Confirmando abertura':'Texto humano');
  }
  for(const label of ['Preparando arquivo','Preparando entrada para indexação','Indexando linhas','Ativando fonte carregada','Concluído','Pronto']) assert.ok(emit.includes(`"${label}"`));
});
