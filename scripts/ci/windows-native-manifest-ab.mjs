// Diagnostic only: a same-environment control must reproduce the loader failure.
// Never modifies the original test executable or changes the Cargo step's result.
import { readFileSync, writeFileSync, copyFileSync, existsSync, openSync, closeSync } from 'node:fs';
import { resolve, dirname, basename, join } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
export function inspectPe(bytes) {
  const need = (offset, size) => { if (!Number.isSafeInteger(offset) || offset < 0 || offset + size > bytes.length) throw Error('Invalid PE range'); };
  const u16 = offset => { need(offset, 2); return bytes.readUInt16LE(offset); };
  const u32 = offset => { need(offset, 4); return bytes.readUInt32LE(offset); };
  if (u16(0) !== 0x5a4d) throw Error('Missing MZ signature');
  const pe = u32(0x3c);
  if (u32(pe) !== 0x4550 || u16(pe + 4) !== 0x8664) throw Error('Expected AMD64 PE');
  const optional = pe + 24, optionalSize = u16(pe + 20);
  need(optional, optionalSize);
  if (optionalSize < 136 || u16(optional) !== 0x20b) throw Error('Expected PE32+ directories');
  const count = u16(pe + 6), table = optional + optionalSize;
  if (!count || count > 96) throw Error('Invalid section count');
  need(table, count * 40);
  const sections = Array.from({ length: count }, (_, i) => {
    const at = table + i * 40;
    const rawSize = u32(at + 16), rawOffset = u32(at + 20);
    need(rawOffset, rawSize);
    return { name: bytes.toString('ascii', at, at + 8).replace(/\0.*$/, ''),
      virtualSize: u32(at + 8), rva: u32(at + 12), rawSize, rawOffset, characteristics: u32(at + 36) };
  });
  const texts = sections.filter(s => s.name === '.text');
  if (texts.length !== 1) throw Error('Expected exactly one .text section');
  const { rawOffset, ...text } = texts[0];
  text.sha256 = hash(bytes.subarray(rawOffset, rawOffset + text.rawSize));
  let manifestType = false;
  if (u32(optional + 108) >= 3) {
    const resourceRva = u32(optional + 128), resourceSize = u32(optional + 132);
    if (resourceRva) {
      const section = sections.find(s => resourceRva >= s.rva && resourceRva - s.rva + resourceSize <= s.rawSize);
      if (!section || resourceSize < 16) throw Error('Invalid resource directory');
      const root = section.rawOffset + resourceRva - section.rva;
      const entries = u16(root + 12) + u16(root + 14);
      if (16 + entries * 8 > resourceSize) throw Error('Invalid resource entries');
      for (let i = 0; i < entries; i++) if (u32(root + 16 + i * 8) === 24) manifestType = true;
    } else if (resourceSize) throw Error('Inconsistent resource directory');
  }
  return { sha256: hash(bytes), text, manifestType };
}

export function importEvidence(text) {
  let module = '', named = false, ordinal = false;
  const normalized = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === 'Summary') break;
    if (line.startsWith('Dump of file ')) continue;
    if (line) normalized.push(line);
    if (/^[\w.-]+\.dll$/i.test(line)) module = line.toLowerCase();
    if (module === 'comctl32.dll') {
      if (/\bTaskDialogIndirect\b/.test(line)) named = true;
      if (/\bOrdinal\s+345\b/i.test(line)) ordinal = true;
    }
  }
  return { named, ordinal, normalized: normalized.join('\n') };
}

function run(command, args, cwd, output, timeout) {
  const fd = openSync(output, 'w');
  try {
    const result = spawnSync(command, args, { cwd, env: process.env, stdio: ['ignore', fd, fd], timeout, windowsHide: true });
    return { status: result.status, unsignedStatus: result.status === null ? null : result.status >>> 0,
      signal: result.signal, error: result.error?.message ?? null };
  } finally { closeSync(fd); }
}

function diagnose(destination) {
  const report = { kind: 'diagnostic-unit-harness-copy', status: 'skipped',
    scope: 'Only the copied failed unit-test executable; not cargo --tests',
    environment: 'Inherited diagnostic environment; an unpatched control must reproduce the same loader status',
    args: ['--test-threads=1'], listArgs: ['--list'], cwd: resolve('src-tauri') };
  let original, originalHash, copy;
  const stop = reason => { report.reason = reason; };
  try {
    const raw = JSON.parse(readFileSync(join(destination, 'executables.json'), 'utf8').replace(/^\uFEFF/, ''));
    const entries = (Array.isArray(raw) ? raw : [raw]).filter(e => e.selection === 'cargo-failed-process' && /^loginsight_lib-[\w-]+\.exe$/i.test(basename(e.path)));
    if (entries.length !== 1) return stop('No unique exact failed unit-test executable');
    original = resolve(entries[0].path);
    if (dirname(original).toLowerCase() !== resolve('src-tauri/target/release/deps').toLowerCase()) return stop('Unexpected executable directory');
    const before = inspectPe(readFileSync(original));
    originalHash = before.sha256;
    report.original = { path: original, ...before };
    if (before.sha256 !== entries[0].sha256) return stop('Original differs from the preserved failure');
    if (before.manifestType) return stop('A manifest resource already exists; no automatic merge');
    const name = basename(original);
    const mtStatus = JSON.parse(readFileSync(join(destination, `${name}.manifest-status.json`), 'utf8').replace(/^\uFEFF/, ''));
    const mtLog = readFileSync(join(destination, `${name}.manifest-extraction.txt`), 'utf8');
    if (mtStatus.extracted || mtStatus.exitCode === 0 || !/specified resource type cannot be found/i.test(mtLog)) return stop('mt did not explicitly confirm an absent manifest resource type');
    const imports = importEvidence(readFileSync(join(destination, `${name}.imports.txt`), 'utf8'));
    const librariesRaw = JSON.parse(readFileSync(join(destination, 'common-controls-libraries.json'), 'utf8').replace(/^\uFEFF/, ''));
    const libraries = Array.isArray(librariesRaw) ? librariesRaw : [librariesRaw];
    const ordinalMapped = libraries.some(library => library.dumpbinExitCode === 0 &&
      /^\s*345\s+\S+\s+\S+\s+TaskDialogIndirect\b/m.test(readFileSync(join(destination, library.exports), 'utf8')));
    if (!imports.named && !(imports.ordinal && ordinalMapped)) return stop('Imports do not prove comctl32!TaskDialogIndirect');
    const mt = JSON.parse(readFileSync(join(destination, 'mt.json'), 'utf8').replace(/^\uFEFF/, '')).path;
    const dumpbin = JSON.parse(readFileSync(join(destination, 'dumpbin.json'), 'utf8').replace(/^\uFEFF/, '')).path;
    copy = join(dirname(original), `${name.slice(0, -4)}-manifest-ab.exe`);
    if (existsSync(copy)) { copy = null; return stop('Adjacent diagnostic copy already exists'); }
    copyFileSync(original, copy);
    if (hash(readFileSync(copy)) !== originalHash) return stop('Unpatched copy differs from the original');
    report.copy = copy;
    report.control = run(copy, report.listArgs, report.cwd, join(destination, 'ab-control-list.log'), 30_000);
    if (report.control.unsignedStatus !== 0xc0000139) return stop('Unpatched control did not reproduce STATUS_ENTRYPOINT_NOT_FOUND');
    const manifest = join(destination, 'ab-common-controls.manifest');
    writeFileSync(manifest, '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="amd64" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>\n');
    report.patch = run(mt, ['-nologo', '-manifest', manifest, `-outputresource:${copy};#1`], report.cwd, join(destination, 'ab-manifest-injection.log'), 30_000);
    if (report.patch.status !== 0) return stop('Manifest injection failed');
    const after = inspectPe(readFileSync(copy));
    report.patched = after;
    if (!after.manifestType || JSON.stringify(before.text) !== JSON.stringify(after.text)) return stop('Patched .text differs or manifest is absent');
    report.importCheck = run(dumpbin, ['/imports', copy], report.cwd, join(destination, 'ab-patched.imports.txt'), 30_000);
    if (report.importCheck.status !== 0 || imports.normalized !== importEvidence(readFileSync(join(destination, 'ab-patched.imports.txt'), 'utf8')).normalized) return stop('Patched imports differ');
    report.manifestCheck = run(mt, ['-nologo', `-inputresource:${copy};#1`, `-out:${join(destination, 'ab-patched.manifest')}`], report.cwd, join(destination, 'ab-manifest-extraction.log'), 30_000);
    if (report.manifestCheck.status !== 0) return stop('Cannot extract patched manifest');
    if (hash(readFileSync(original)) !== originalHash) return stop('Original changed before A/B execution');
    report.patchedList = run(copy, report.listArgs, report.cwd, join(destination, 'ab-patched-list.log'), 30_000);
    if (report.patchedList.status !== 0) return stop('Patched loader/list did not pass');
    report.harness = run(copy, report.args, report.cwd, join(destination, 'ab-unit-harness.log'), 600_000);
    report.status = report.harness.status === 0 ? 'copied-unit-harness-passed' : 'copied-unit-harness-failed';
  } catch (error) {
    report.status = 'diagnostic-error'; report.error = error.stack;
  } finally {
    if (originalHash) {
      report.originalHashAfter = hash(readFileSync(original));
      report.originalUnchanged = report.originalHashAfter === originalHash;
      if (!report.originalUnchanged) { report.status = 'diagnostic-error'; report.reason = 'Original executable changed'; }
    }
    if (copy && existsSync(copy)) copyFileSync(copy, join(destination, basename(copy)));
    writeFileSync(join(destination, 'manifest-ab.json'), JSON.stringify(report, null, 2) + '\n');
    console.log(`Windows manifest A/B: ${report.status}${report.reason ? ` (${report.reason})` : ''}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) diagnose(resolve(process.argv[2]));
