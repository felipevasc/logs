import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = new URL('../../src-tauri/src/', import.meta.url);
const read = name => readFileSync(new URL(name, root), 'utf8');

// Ignore Rust strings/comments while retaining offsets. In particular raw
// regex/JSON literals must not pretend to open or close a function/worker.
function maskRust(source) {
  const out = source.split('');
  const blank = (start, end) => { for (let i = start; i < end; i++) if (out[i] !== '\n') out[i] = ' '; };
  for (let i = 0; i < source.length;) {
    if (source.startsWith('//', i)) {
      const end = source.indexOf('\n', i); const next = end < 0 ? source.length : end;
      blank(i, next); i = next; continue;
    }
    if (source.startsWith('/*', i)) {
      let end = i + 2, depth = 1;
      while (end < source.length && depth) {
        if (source.startsWith('/*', end)) { depth++; end += 2; }
        else if (source.startsWith('*/', end)) { depth--; end += 2; }
        else end++;
      }
      blank(i, end); i = end; continue;
    }
    const raw = (source[i] === 'r' || source[i] === 'b') && /^(?:br|r)(#*)"/.exec(source.slice(i));
    if (raw) {
      const suffix = `"${raw[1]}`, at = source.indexOf(suffix, i + raw[0].length);
      const end = at < 0 ? source.length : at + suffix.length;
      blank(i, end); i = end; continue;
    }
    const quote = source[i];
    const character = quote === "'" && /^'(?:[^'\\\n]|\\(?:.|u\{[^}]*\}))'/.test(source.slice(i));
    if (quote === '"' || character) {
      let end = i + 1;
      while (end < source.length) {
        if (source[end] === '\\') { end += 2; continue; }
        if (source[end++] === quote) break;
      }
      blank(i, end); i = end; continue;
    }
    i++;
  }
  return out.join('');
}
function closeAt(masked, open, left, right) {
  let depth = 1, at = open + 1;
  while (at < masked.length && depth) {
    if (masked[at] === left) depth++;
    else if (masked[at] === right) depth--;
    at++;
  }
  assert.equal(depth, 0, `Unclosed ${left} in inspected Rust source`);
  return at;
}
function asyncBodies(source) {
  const masked = maskRust(source), result = [];
  for (const match of masked.matchAll(/\basync\s+fn\s+(\w+)\s*(?:<[^\n]*>)?\s*\(/g)) {
    const open = masked.indexOf('{', match.index), end = closeAt(masked, open, '{', '}');
    result.push({ name: match[1], body: masked.slice(open + 1, end - 1), raw: source.slice(open + 1, end - 1) });
  }
  return result;
}
const bodyOf = (file, name) => {
  const result = asyncBodies(read(file)).find(body => body.name === name);
  assert.ok(result, `Missing async ${name} in ${file}`); return result.body;
};
function rustFiles(path) {
  return readdirSync(path, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? rustFiles(join(path, entry.name)) : entry.name.endsWith('.rs') ? [join(path, entry.name)] : []);
}
function admittedRanges(body) {
  // These wrappers all terminate at run_with_token. This checks closure
  // containment, rather than merely finding an offload later in the function.
  const wrappers = /\b(?:offload(?:_operation(?:_priority)?|_admitted|_case(?:_interactive|_record|_input)?|_archive)?|capture_prepared|run_context|run_interactive|run_source_result)\s*\(/g;
  const calls = regex => [...body.matchAll(regex)].map(match => {
    const open = body.indexOf('(', match.index); return [open, closeAt(body, open, '(', ')')];
  });
  const blocking = calls(/\bspawn_blocking\s*\(/g);
  const tokens = calls(/\brun_with_token\s*\(/g).filter(([from, to]) => blocking.some(([start, end]) => start < from && to < end));
  return [...calls(wrappers), ...tokens];
}

function unadmittedCalls(body) {
  const ranges = admittedRanges(body);
  return [...body.matchAll(/\b(?:capture|capture_case|capture_case_shared|capture_archive_case|case_editor_admission|validate_identity|validate)\s*\(/g)]
    .filter(call => !ranges.some(([from, to]) => from < call.index && call.index < to));
}

test('wiring guard rejects local capture and policy preambles even when offload follows', () => {
  for (const body of [
    'let result = capture_case(state); offload(move || result).await',
    'let policy = Policy::capture(owner); offload_operation(id, move || use_policy(policy)).await',
    'spawn_blocking(move || capture_case(state)).await',
    'run_with_token(token, || capture_case(state))',
  ]) assert.ok(unadmittedCalls(body).length, body);
  for (const body of [
    'capture_prepared(app, pin, id, priority, move |state, pin| pin.capture_case(state)).await',
    'spawn_blocking(move || run_with_token(token, || capture_case(state))).await',
  ]) assert.equal(unadmittedCalls(body).length, 0, body);
});

test('every async Case capture and heavy validation stays inside admitted worker closures', () => {
  const failures = [];
  for (const file of rustFiles(fileURLToPath(root))) {
    for (const { name, body } of asyncBodies(readFileSync(file, 'utf8'))) {
      for (const call of unadmittedCalls(body)) failures.push(`${file}: ${name}: ${call[0]}`);
    }
  }
  assert.deepEqual(failures, [], `Synchronous capture/validation escaped admission:\n${failures.join('\n')}`);
});

test('capture registration and heavy preparation run on a blocking admitted worker', () => {
  const body = bodyOf('analysis_runtime.rs', 'capture_prepared');
  assert.match(body, /operations::token\(operation_id\)/);
  assert.match(body, /spawn_blocking\(move\s*\|\|\s*crate::operations::run_with_token\(token,/);
  assert.match(body, /prepare_pinned\(state\.inner\(\),\s*&pin,\s*retained,\s*prepare\)/);
  assert.ok(body.indexOf('operations::token(') < body.indexOf('spawn_blocking('));
});

test('local visibility capture, legacy sync and normalization preview use the shared admission boundary', () => {
  assert.match(bodyOf('analysis_runtime.rs', 'exclusion_visibility'), /capture_case_async\(app\.clone\(\),/);
  assert.match(bodyOf('case_cache.rs', 'case_sync'), /crate::offload\(move\s*\|\|\s*case_sync_impl\(key,\s*events,\s*analysis_context\)\)\.await\?/);
  const preview = bodyOf('triage.rs', 'normalization_preview');
  const ranges = admittedRanges(preview);
  for (const call of preview.matchAll(/\b(?:validate_mappings|normalize|to_value|redact_value)\s*\(/g)) {
    assert.ok(ranges.some(([from, to]) => from < call.index && call.index < to), `${call[0]} escaped preview admission`);
  }
  assert.match(preview, /crate::offload\(/);
});
