import test from 'node:test';
import assert from 'node:assert/strict';
import { checkpointRows, benchmarkRecords, main } from '../bench/check-recovery.mjs';
test('recovery probe validates checkpoint counters and structured benchmark evidence', () => {
  assert.equal(checkpointRows([{ rows: 965081 }, { rows: 34919 }]), 1000000);
  assert.throws(() => checkpointRows([{ rows: -1 }]));
  assert.throws(() => checkpointRows([{ rows: '100' }]));
  assert.deepEqual(benchmarkRecords('noise\nBENCH {"operation":"prepare","elapsedMs":3}\n'), [{ operation: 'prepare', elapsedMs: 3 }]);
});
test('recovery probe requires explicit isolated inputs before starting a process', async () => {
  await assert.rejects(main([]), /Required/);
  await assert.rejects(main(['--unknown', 'value']), /Unknown option/);
});
