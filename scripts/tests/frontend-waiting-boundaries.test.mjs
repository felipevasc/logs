/* Static CSS/identity regression only; no browser, computed layout or native claim.
   WAITING_BOUNDARY_CSS can point to an extracted base CSS for before/after proof. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const css = readFileSync(process.env.WAITING_BOUNDARY_CSS || new URL('../../frontend/waiting-visuals.css', import.meta.url), 'utf8');
const source = readFileSync(new URL('../../frontend/waiting-visuals.js', import.meta.url), 'utf8');
const window = {};
vm.runInNewContext(source.replace('  function mount(', '  window.boundaryScenes = scenes;\n  function mount('), { window, Intl });
const families = ['reading', 'checkpoint', 'calculation', 'composition', 'verification'];
const aliases = new Map([
  ['wv-peek-head', 'wv-archive-head'], ['wv-peek-gaze', 'wv-archive-gaze'],
  ['wv-peek-arm-back', 'wv-archive-arm-back'], ['wv-sort-arm-back', 'wv-group-arm-back'],
]);
const expected = {
  reading: 'reader-travel reader-body reader-head reader-arm reader-hand reader-gaze reader-arm-back reader-leg-back reader-knee-back reader-foot-back reader-leg-front reader-knee-front reader-foot-front reader-visor reader-smile reader-filed reader-paper',
  checkpoint: 'archive-body archive-arm archive-hand archive-drawer archive-head archive-gaze archive-arm-back archive-label',
  calculation: 'group-body group-arm group-hand group-held group-source group-head group-gaze group-arm-back group-visor group-filed group-well',
  composition: 'compose-body compose-arm compose-hand compose-head compose-arm-back compose-gaze compose-visor compose-held compose-loaded compose-press compose-bed',
  verification: 'verify-body verify-head verify-arm verify-hand verify-gaze verify-held verify-docked',
};

// This bounded model resolves name/shorthand declarations only, not rendered CSS.
// Work declarations are flat descendant/class/attribute rules. Fail on unknown
// syntax instead of claiming a complete CSS engine. Reaction rules cannot affect
// work animation identities and are outside this contract's explicit scope.
const workCss = css.slice(0, css.indexOf('@keyframes wv-work-boundary'));
const rules = [...workCss.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^{}]*)\}/g)]
  .filter(([, , body]) => /(?:^|;)\s*animation(?:-name)?\s*:/.test(body))
  .map(([, selector, body]) => {
    selector = selector.trim();
    assert.match(selector, /^\.waiting-visual(?:\[data-[\w-]+="[\w-]+"\])* (?:\.[\w-]+ )*\.[\w-]+$/);
    return { selector, body, specificity: (selector.match(/\.|\[/g) || []).length };
  });
function matchesPart(node, part) {
  const classes = [...part.matchAll(/\.([\w-]+)/g)].map(m => m[1]);
  const attrs = [...part.matchAll(/\[([^=]+)="([^"]+)"\]/g)];
  return classes.every(name => node.classes.includes(name)) && attrs.every(([, name, value]) => node.attrs?.[name] === value);
}
function matches(node, selector) {
  const parts = selector.split(' ');
  if (!matchesPart(node, parts.pop())) return false;
  while (parts.length) {
    const part = parts.pop(); node = node.parent;
    while (node && !matchesPart(node, part)) node = node.parent;
    if (!node) return false;
  }
  return true;
}
function tracks(family, pace) {
  const root = { classes: ['waiting-visual'], attrs: { 'data-family': family, 'data-pace': pace, 'data-animated': 'true' } };
  const work = { classes: ['wv-work'], parent: root }, stack = [work], nodes = [];
  for (const [, closing, tag, attrs] of window.boundaryScenes[family].matchAll(/<(\/?)([a-z]+)\b([^>]*)>/g)) {
    if (closing) { stack.pop(); continue; }
    const node = { classes: /class="([^"]+)"/.exec(attrs)?.[1].split(' ') || [], parent: stack.at(-1) };
    nodes.push(node);
    if (!attrs.endsWith('/')) stack.push(node);
  }
  nodes.push({ classes: ['wv-work-boundary'], parent: root });
  return nodes.flatMap((node, index) => {
    let name = null, priority = -1;
    for (const rule of rules) if (matches(node, rule.selector) && rule.specificity >= priority) {
      for (const [, property, value] of rule.body.matchAll(/(?:^|;)\s*(animation(?:-name)?)\s*:\s*([^;]+)/g)) {
        name = property === 'animation-name' ? value.trim() : value.trim().split(/\s+/)[0];
        priority = rule.specificity;
      }
    }
    return name ? [{ target: node.classes.join(' '), index, name }] : [];
  });
}
function keyframes() {
  const result = new Map(), headers = /@keyframes\s+([\w-]+)\s*\{/g;
  for (let match; (match = headers.exec(css));) {
    let end = headers.lastIndex, depth = 1;
    while (depth && end < css.length) { if (css[end] === '{') depth++; if (css[end] === '}') depth--; end++; }
    assert.equal(depth, 0);
    assert.ok(!result.has(match[1]), `unique keyframe ${match[1]}`);
    result.set(match[1], css.slice(headers.lastIndex, end - 1)); headers.lastIndex = end;
  }
  return result;
}
for (const family of families) {
  test(`${family}: boundary clock is absent throughout the finite gesture`, () => {
    assert.equal(tracks(family, 'gesture').filter(t => t.name === 'wv-work-boundary').length, 0);
    assert.equal(tracks(family, 'loop').filter(t => t.name === 'wv-work-boundary').length, 1);
  });
  test(`${family}: every work track starts a new identity on entry to the loop`, () => {
    const short = tracks(family, 'gesture').filter(t => t.target !== 'wv-work-boundary');
    const long = tracks(family, 'loop').filter(t => t.target !== 'wv-work-boundary');
    assert.deepEqual(long.map(t => t.name).sort(), expected[family].split(' ').map(name => `wv-${name}`).sort());
    assert.ok(short.length > 0);
    for (const track of long) {
      const previous = short.find(t => t.index === track.index);
      if (previous) assert.notEqual(track.name, previous.name, `${family}/${track.target}: retained CSS identity would retain gesture age`);
    }
  });
}
test('the four added short aliases preserve the original curves byte for byte', () => {
  const frames = keyframes();
  for (const [alias, original] of aliases) {
    assert.ok(frames.has(alias), `${alias} exists`);
    assert.equal(frames.get(alias), frames.get(original));
  }
});
test('approved five-family choreography remains byte-identical to db021aa', () => {
  const frames = [...keyframes()].filter(([name]) => /^wv-(?:inspect|reader|peek|archive|sort|group|align|compose|verify)-/.test(name) && !aliases.has(name));
  const digest = createHash('sha256').update(JSON.stringify(frames)).digest('hex');
  assert.equal(frames.length, 81);
  assert.equal(digest, '56baed59262d7bce2f9468ae363cacd0cb392888447b7f5ffdad320bc7a6e72d');
});
