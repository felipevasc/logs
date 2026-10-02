import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
const workspace = read('frontend/workspace.css'), base = read('frontend/styles.css');
const previews = ['test-settings-codes.mjs', 'test-timestamp-editor.mjs'].map(name => ({ name, source: read(`scripts/preview/${name}`) }));
const palette = theme => workspace.match(new RegExp(`html\\[data-theme="${theme}"\\]\\s*\\{([^}]+)\\}`))[1];
const token = (theme, name) => palette(theme).match(new RegExp(`${name}:\\s*(#[\\da-f]{6})`))[1];
const rgb = hex => `rgb(${hex.slice(1).match(/../g).map(value => parseInt(value, 16)).join(', ')})`;

function fixture(source, theme, overrides = {}) {
  const style = { color: rgb(token(theme, '--text-0')), backgroundColor: rgb(token(theme, '--code-bg')), opacity: '1', ...overrides.style };
  const node = { tagName: 'TEXTAREA', value: '{"loaded":1}', readOnly: false, matches: () => false, ...overrides.node };
  const context = vm.createContext({ assert, getComputedStyle: () => style });
  const start = source.indexOf('function measureEditorReadability(node)');
  const end = source.indexOf('\nasync function ', start);
  assert.ok(start >= 0 && end > start, 'the browser measurement and assertions must be present');
  vm.runInContext(source.slice(start, end), context);
  return { measure: () => context.measureEditorReadability(node), validate: () => context.assertEditorReadability(context.measureEditorReadability(node), theme) };
}

test('plain editor fix targets exactly two IDs and changes only the existing foreground token', () => {
  const matches = [...workspace.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(match => /#codes-editor|#ts-example/.test(match[1]));
  assert.equal(matches.length, 1);
  const selector = matches[0][1].replace(/\/\*[\s\S]*?\*\//g, '').trim();
  assert.equal(selector, '#codes-editor, #ts-example');
  assert.equal(matches[0][2].trim(), 'color: var(--text-0);', 'no font, geometry, background or interaction changes');
  assert.match(base, /\.code-pane\s*\{[^}]*color:\s*#e6e6e6;/, 'the unrelated code-pane and syntax palette are not redesigned');
  assert.match(base, /\.ts-example mark\.tsg1\s*\{[^}]*color:\s*inherit;/, 'timestamp group text keeps inheriting its preview foreground');
});

for (const { name, source } of previews) {
  test(`${name}: both palettes are readable and the former light foreground reproduces the defect`, () => {
    for (const theme of ['dark', 'light']) {
      const f = fixture(source, theme), measured = f.measure();
      f.validate();
      const expected = theme === 'light' ? 12.912339648373214 : 16.433523738246777;
      assert.ok(Math.abs(measured.contrast - expected) < 1e-9);
    }
    const old = fixture(source, 'light', { style: { color: 'rgb(230, 230, 230)' } });
    assert.ok(Math.abs(old.measure().contrast - 1.106639563648) < 1e-9);
    assert.throws(old.validate, /foreground and actual editor surface/);
    const pre = fixture(source, 'light', { node: { tagName: 'PRE', value: undefined, textContent: 'Selecione as fontes acima.', readOnly: undefined } });
    pre.validate(); assert.equal(pre.measure().text, 'Selecione as fontes acima.');
  });

  test(`${name}: probes require enabled nonempty opaque content and validate luminance`, () => {
    const measured = fixture(source, 'light', { style: { color: 'rgb(0, 0, 0)', backgroundColor: 'rgb(255, 255, 255)' } }).measure();
    assert.equal(measured.contrast, 21);
    for (const node of [{ matches: () => true }, { readOnly: true }, { value: '' }]) assert.throws(fixture(source, 'light', { node }).validate);
    assert.throws(fixture(source, 'light', { style: { opacity: '.55' } }).validate);
    assert.throws(fixture(source, 'light', { style: { backgroundColor: 'rgba(237, 242, 245, 0.5)' } }).measure, /must be opaque/);
    assert.throws(fixture(source, 'light', { style: { color: 'transparent' } }).measure, /Unsupported editor color/);
    assert.match(source, /\.evaluate\(measureEditorReadability\)/, 'measure real computed styles in the existing browser journey');
    assert.match(source, /assertEditorReadability\(/);
  });
}

test('browser receipts capture enabled Settings at every layout and Timestamp in both themes', () => {
  assert.match(previews[0].source, /await usable\(\);\s*layout\.readability = await editor\.evaluate/);
  assert.match(previews[0].source, /editor\.getAttribute\('aria-busy'\), 'false'/);
  assert.match(previews[1].source, /if \(!snapshot\.applyDisabled\)\s*\{\s*assert\.equal\(await rule\.isDisabled\(\), false/);
  assert.match(previews[1].source, /results\.states\.filter\(value => value\.readability\)\.map\(value => value\.theme\)/);
});
