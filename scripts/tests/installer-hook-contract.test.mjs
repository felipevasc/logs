import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';

const hook = readFileSync(new URL('../../src-tauri/windows/installer-hooks.nsh', import.meta.url), 'utf8');
// Static safety contract, not a Windows runtime substitute. The pinned Tauri
// template invokes PREINSTALL after SetOutPath, and PREUNINSTALL immediately
// before CheckIfAppIsRunning without creating its former install directory:
// https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.0/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi#L598-L605
// https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.0/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi#L726-L731
test('missing parent is accepted only by the explicit uninstall path after an independent absence probe', () => {
  assert.match(hook, /!macro NSIS_HOOK_PREINSTALL\s+StrCpy \$LogInsightUninstall 0\s+!macroend/);
  assert.match(hook, /!macro NSIS_HOOK_PREUNINSTALL\s+StrCpy \$LogInsightUninstall 1\s+!macroend/);
  const missing = hook.slice(hook.indexOf('${If} $1 = 3'), hook.indexOf('IntOp $2'));
  assert.match(missing, /\$\{If\} \$1 = 3\s+\$\{AndIf\} \$LogInsightUninstall = 1/);
  assert.match(missing, /GetFileAttributesW\(w "\$INSTDIR"\) i \.r3 \?e'\s+Pop \$4/);
  assert.match(missing, /\$\{If\} \$3 = -1\s+\$\{If\} \$4 = 2\s+\$\{OrIf\} \$4 = 3\s+Goto li_ready_/);
  assert.match(missing, /StrCpy \$1 \$4/, 'the second probe failure remains visible');
  assert.equal((missing.match(/Goto li_ready_/g) || []).length, 1);
  assert.doesNotMatch(hook, /\$\{OrIf\} \$1 = 3/, 'an installer cannot accept PATH_NOT_FOUND unconditionally');
});
test('file checks remain non-destructive and restore caller registers on every completion path', () => {
  assert.match(hook, /CreateFileW\(w "\$\{executablePath\}", i 0x40000000, i 7, p 0, i 3, i 0, p 0\)/);
  assert.doesNotMatch(hook, /System::Call '[^']*(?:TerminateProcess|RmShutdown|WriteFile|SetFilePointer|SetEndOfFile)/);
  for (const label of ['li_abort_', 'li_ready_']) {
    const position = hook.indexOf(`    ${label}`);
    const branch = hook.slice(position, position + 180);
    assert.match(branch, /Pop \$4\s+Pop \$3\s+Pop \$2\s+Pop \$1\s+Pop \$0/);
  }
  assert.match(hook, /SetErrorLevel 2[\s\S]*?\bQuit\b/);
});
