import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';

const css = readFileSync(new URL('../../frontend/workspace.css', import.meta.url), 'utf8');

test('source reading badges wrap inside one bounded box without shrinking their inherited type', () => {
  const badge = css.match(/\.sources-table \.tag\{([^}]*)\}/)?.[1];
  assert.ok(badge, 'the override is scoped to Sources');
  assert.match(badge, /(?:^|;)display:inline-block(?:;|$)/);
  assert.match(badge, /(?:^|;)box-sizing:border-box(?:;|$)/);
  assert.match(badge, /(?:^|;)max-width:100%(?:;|$)/);
  assert.match(badge, /(?:^|;)white-space:normal(?:;|$)/);
  assert.match(badge, /(?:^|;)overflow-wrap:anywhere(?:;|$)/);
  assert.doesNotMatch(badge, /(?:^|;)(?:font(?:-size)?|zoom|transform):/, 'wrapping cannot reduce typography or scale');
});
