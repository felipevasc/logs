import test from 'node:test';
import assert from 'node:assert/strict';
import { inspectPe, importEvidence, confirmsAbsentManifest } from '../ci/windows-native-manifest-ab.mjs';

function image(manifest = false) {
  const b = Buffer.alloc(0x800), pe = 0x80, optional = pe + 24, table = optional + 0xf0;
  b.writeUInt16LE(0x5a4d, 0); b.writeUInt32LE(pe, 0x3c);
  b.writeUInt32LE(0x4550, pe); b.writeUInt16LE(0x8664, pe + 4);
  b.writeUInt16LE(2, pe + 6); b.writeUInt16LE(0xf0, pe + 20);
  b.writeUInt16LE(0x20b, optional); b.writeUInt32LE(16, optional + 108);
  b.writeUInt32LE(0x2000, optional + 128); b.writeUInt32LE(0x80, optional + 132);
  for (const [i, name, rva, raw] of [[0, '.text', 0x1000, 0x400], [1, '.rsrc', 0x2000, 0x600]]) {
    const at = table + i * 40;
    b.write(name, at); b.writeUInt32LE(64, at + 8); b.writeUInt32LE(rva, at + 12);
    b.writeUInt32LE(0x200, at + 16); b.writeUInt32LE(raw, at + 20);
  }
  b[0x400] = 0xc3;
  b.writeUInt16LE(1, 0x600 + 14);
  b.writeUInt32LE(manifest ? 24 : 16, 0x600 + 16);
  return b;
}

test('manifest-only change preserves exact .text, while code edits and invalid ranges are detected', () => {
  const before = inspectPe(image()), after = inspectPe(image(true));
  assert.equal(before.manifestType, false); assert.equal(after.manifestType, true);
  assert.deepEqual(after.text, before.text); assert.notEqual(after.sha256, before.sha256);
  const changed = image(); changed[0x400] ^= 1;
  assert.notEqual(inspectPe(changed).text.sha256, before.text.sha256);
  assert.throws(() => inspectPe(image().subarray(0, 0x500)), /range/);
});

test('mt absence evidence accepts the observed resource-section error but not generic failures', () => {
  assert.equal(confirmsAbsentManifest('The specified resource type cannot be found in the image file.'), true);
  assert.equal(confirmsAbsentManifest('The specified image file did not contain a resource section.'), true);
  assert.equal(confirmsAbsentManifest('Failed to read the manifest: access denied.'), false);
  assert.equal(confirmsAbsentManifest('The system cannot find the file specified.'), false);
});

test('TaskDialog evidence stays within comctl32 and filename/resource summary changes do not alter imports', () => {
  const imports = `Dump of file C:\\test.exe\nFile Type: EXECUTABLE IMAGE\n    COMCTL32.dll\n        159 TaskDialogIndirect\n    KERNEL32.dll\n        123 ExitProcess\n  Summary\n  200 .rsrc\n`;
  assert.equal(importEvidence(imports).named, true);
  assert.equal(importEvidence(imports.replace('COMCTL32.dll', 'other.dll')).named, false);
  assert.equal(importEvidence(imports.replace('TaskDialogIndirect', 'OtherSymbol')).named, false);
  assert.equal(importEvidence(imports.replace('159 TaskDialogIndirect', 'Ordinal 345')).ordinal, true);
  assert.equal(importEvidence(imports.replace('159 TaskDialogIndirect', '345 OtherSymbol')).ordinal, false);
  assert.equal(importEvidence(imports).normalized,
    importEvidence(imports.replace('C:\\test.exe', 'C:\\copy.exe').replace('200 .rsrc', '400 .rsrc')).normalized);
});
