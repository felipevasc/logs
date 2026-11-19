import assert from 'node:assert/strict';
import test from 'node:test';
import { isCargoTestBinary } from '../ci/windows-harness-path.mjs';

test('matches the executed Windows harness in both Cargo layouts', () => {
  const root = 'D:/a/logs/logs/src-tauri/target/release';
  const unit = 'loginsight_lib-3b73ed9de6c5840d.exe';
  const output = root + '/build/loginsight/3b73ed9de6c5840d/out/';
  assert.ok(isCargoTestBinary(root, root + '/deps/' + unit, true));
  assert.ok(isCargoTestBinary(root, output + unit, true));
  assert.ok(isCargoTestBinary(root, output + 'engine_parity-3b73ed9de6c5840d.exe'));
  for (const path of [output + 'engine_parity-3b73ed9de6c5840d.exe', output + 'loginsight_lib-deadbeef.exe',
    output.replace('/loginsight/', '/another/') + unit, root + '/deps/other/' + unit, root + '/deps/../../' + unit]) {
    assert.equal(isCargoTestBinary(root, path, true), false, path);
  }
});
