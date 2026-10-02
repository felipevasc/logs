import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { installMotionSampling } from '../preview/motion-sampling.mjs';

// CSS/Animation protocol fixture only. Real browser pause/frozen-time/contact
// assertions stay in the three preview scripts and must still pass in CI.
function fixture() {
  const events = [], targets = [];
  const makeTarget = (name, value = '', priority = '', rule = 'running') => {
    const properties = new Map(value ? [['animation-play-state', { value, priority }]] : []);
    const target = { name, rule, computed: value || rule, style: {
      getPropertyValue: property => properties.get(property)?.value || '',
      getPropertyPriority: property => properties.get(property)?.priority || '',
      setProperty(property, value, priority) {
        events.push(['set', name, property, value, priority]);
        properties.set(property, { value, priority });
      },
      removeProperty(property) { events.push(['remove', name, property]); properties.delete(property); },
    } };
    targets.push(target);
    return target;
  };
  const makeAnimation = (target, time) => ({ effect: { target },
    get currentTime() { return time; },
    set currentTime(value) {
      assert.equal(target.computed, 'paused', 'CSS is flushed before a timeline is sought');
      events.push(['seek', target.name, value]); time = value;
    },
    get playState() { return target.computed; },
    pause() { assert.fail('WAAPI pause must never override CSS governance'); },
    play() { assert.fail('WAAPI play must never override CSS governance'); },
  });
  const sandbox = { window: {}, requestAnimationFrame: callback => queueMicrotask(callback),
    getComputedStyle(target) {
      target.computed = target.style.getPropertyValue('animation-play-state') || target.rule;
      events.push(['flush', target.name, target.computed]);
      return { getPropertyValue: () => target.computed };
    },
  };
  vm.runInNewContext(`(${installMotionSampling.toString()})()`, sandbox);
  return { sampling: sandbox.window.__waitingMotionSampling, makeTarget, makeAnimation, events, sandbox, targets };
}

test('sampling uses unique CSS targets, preserves all times and removes absent inline declarations', async () => {
  const f = fixture(), shared = f.makeTarget('shared'), work = f.makeTarget('work', '', '', 'paused');
  const a = f.makeAnimation(shared, 123), b = f.makeAnimation(shared, 456), c = f.makeAnimation(work, 0);
  const sample = await f.sampling.begin([a, b, a, c]);
  assert.equal(f.events.filter(([type, target]) => type === 'set' && target === 'shared').length, 1);
  assert.equal(a.playState, 'paused'); assert.equal(c.playState, 'paused');
  await sample.seek(9000);
  assert.deepEqual([a.currentTime, b.currentTime, c.currentTime], [9000, 9000, 9000]);
  await sample.restore();
  assert.deepEqual([a.currentTime, b.currentTime, c.currentTime], [123, 456, 0]);
  assert.equal(shared.style.getPropertyValue('animation-play-state'), '');
  assert.equal(work.style.getPropertyValue('animation-play-state'), '');
  assert.equal(a.playState, 'running'); assert.equal(c.playState, 'paused');
  assert.equal(f.events.filter(([type]) => type === 'remove').length, 2);
  const count = f.events.length;
  await sample.restore(); assert.equal(f.events.length, count, 'cleanup is idempotent');
  shared.rule = 'paused'; f.sandbox.getComputedStyle(shared);
  assert.equal(a.playState, 'paused', 'a subsequent product CSS pause retains control');
});

test('sampling restores exact inline values and priorities, including CSS lists', async () => {
  const f = fixture(), target = f.makeTarget('inline', 'running, paused', 'important');
  const animation = f.makeAnimation(target, 812);
  const sample = await f.sampling.begin([animation]);
  assert.equal(target.style.getPropertyPriority('animation-play-state'), 'important');
  await sample.seek(42); await sample.restore();
  assert.equal(animation.currentTime, 812);
  assert.equal(target.style.getPropertyValue('animation-play-state'), 'running, paused');
  assert.equal(target.style.getPropertyPriority('animation-play-state'), 'important');
});

test('finally cleanup restores a failed sample without forcing paused work tracks to run', async () => {
  const f = fixture(), target = f.makeTarget('work', 'paused', '');
  const animation = f.makeAnimation(target, 0);
  await assert.rejects(async () => {
    const sample = await f.sampling.begin([animation]);
    try { await sample.seek(2800); throw Error('measurement failed'); }
    finally { await sample.restore(); }
  }, /measurement failed/);
  assert.equal(animation.currentTime, 0); assert.equal(animation.playState, 'paused');
  assert.equal(target.style.getPropertyValue('animation-play-state'), 'paused');
  assert.equal(target.style.getPropertyPriority('animation-play-state'), '');
});

test('natural capture release restores CSS policy, then final cleanup restores original times', async () => {
  const f = fixture(), moving = f.makeTarget('coffee'), still = f.makeTarget('work', '', '', 'paused');
  const a = f.makeAnimation(moving, 320), b = f.makeAnimation(still, 84);
  const sample = await f.sampling.begin([a, b]);
  try {
    await sample.seek(0); await sample.release();
    assert.equal(a.playState, 'running'); assert.equal(b.playState, 'paused');
    moving.rule = 'paused'; f.sandbox.getComputedStyle(moving);
    assert.equal(a.playState, 'paused', 'product CSS can pause immediately after capture release');
  } finally { await sample.restore(); }
  assert.deepEqual([a.currentTime, b.currentTime], [320, 84]);
  assert.equal(a.playState, 'paused'); assert.equal(b.playState, 'paused');
});

test('preview samplers never use WAAPI playback overrides and test pause before and after seeks', () => {
  for (const filename of ['test-waiting-visuals.mjs', 'test-case-report-waiting.mjs', 'test-waiting-reactions.mjs']) {
    const source = readFileSync(new URL(`../preview/${filename}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /\.(?:pause|play)\s*\(/, filename);
    assert.match(source, /addInitScript\(installMotionSampling\)/, filename);
  }
  const source = readFileSync(new URL('../preview/test-waiting-reactions.mjs', import.meta.url), 'utf8');
  const naturalEnd = source.indexOf("mark('both-returned-to-work')");
  const before = source.indexOf("results.preSamplingPause =");
  const sampling = source.indexOf('const sampling = await window.__waitingMotionSampling.begin(tracks)');
  const after = source.indexOf('results.midEpisodePause =');
  assert.ok(naturalEnd >= 0 && naturalEnd < before && before < sampling && sampling < after);
  assert.match(source, /timeMs - events\[firstCoffee\]\.timeMs >= 31000/);
  assert.match(source, /family === 'reading' \? 32 : 31/);
  assert.match(source, /elapsedMs: 60000/);
});
