/* Ensures CI retains natural playback checks; this is not a rendered pass. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {fullPreview,planValidation} from '../ci/validation-plan.mjs';
const source=readFileSync(new URL('../preview/test-waiting-playback.mjs',import.meta.url),'utf8');
test('natural playback gate records production scenes and covers sparse/churn behavior without fake animation events',()=>{
  assert.match(source,/readFileSync\('frontend\/waiting-visuals\.css'/);assert.match(source,/readFileSync\('frontend\/waiting-visuals\.js'/);
  assert.match(source,/nativeEngineVerified:false,installedWebViewVerified:false/);
  assert.match(source,/\['quiet',0\],\['sparse',9000\],\['churn',9000\]/);
  assert.match(source,/e\.type==='animationiteration'&&e\.trusted/);
  assert.doesNotMatch(source,/new AnimationEvent|sampling\.seek|playbackRate\s*=(?!=)|\.currentTime\s*=(?!=)/);
});
test('completion playback checks actual identity/time preservation, available inputs and full reaction return',()=>{
  for(const fact of ['sameAnimations','beforeTimes','afterTimes','sameFocus','pointerEvents','tailClicked','parentIsBody'])assert.ok(source.includes(fact));
  assert.match(source,/adapter==='react'/);assert.match(source,/adapter==='resume'/);
  assert.match(source,/tail\.element\.isConnected/);assert.match(source,/workspace-context-change/);assert.match(source,/reduced-motion/);
  assert.match(source,/recordVideo:/);assert.match(source,/waiting-playback-real-preview\.webm/);assert.match(source,/video\.saveAs/);
  assert.ok(fullPreview.includes('test-waiting-playback.mjs'));
  assert.ok(planValidation(['frontend/waiting-visuals.js']).preview.includes('test-waiting-playback.mjs'));
});
test('legacy completion gate covers consumed loops, parked reaction tracks and strict original remainder',()=>{
  const fallback=readFileSync(new URL('../preview/test-waiting-completion-fallback.mjs',import.meta.url),'utf8');
  assert.match(fallback,/Object\.defineProperty\(document\.body,'moveBefore',\{value:undefined/);
  assert.match(fallback,/\['short-gesture','later-work-loop','coffee-react','bridge-prepare','bridge-resume'\]/);
  assert.match(fallback,/sameAnimations,false/);assert.match(fallback,/currentTime>8000/);
  assert.match(fallback,/before\.state==='running'/);assert.match(fallback,/e\.elapsedTime===14\.4/);
  assert.match(fallback,/removedAfterMs-handoff\.expectedRemainingMs-extra/);
  assert.doesNotMatch(fallback,/new AnimationEvent|sampling\.seek|playbackRate\s*=(?!=)|\.currentTime\s*=(?!=)/);
  assert.ok(fullPreview.includes('test-waiting-completion-fallback.mjs'));
  assert.ok(planValidation(['frontend/waiting-visuals.js']).preview.includes('test-waiting-completion-fallback.mjs'));
  assert.match(source,/report\.capabilities\.atomicMove,true/);
  assert.match(source,/tailBoundary\.afterCompletionMs-report\.tailHandoff\.expectedRemainingMs/);
});
test('completion cleanup awaits trusted media delivery and still rejects a missing production listener',()=>{
  const fallback=readFileSync(new URL('../preview/test-waiting-completion-fallback.mjs',import.meta.url),'utf8');
  for(const gate of [source,fallback]) {
    assert.match(gate,/completionMediaProbe\.addEventListener\('change'/);
    assert.match(gate,/trusted:event\.isTrusted,matches:event\.matches,connectedAtChange:tail\.element\.isConnected/);
    assert.match(gate,/waitForFunction\(\(\)=>window\.reducedCompletionEvent!==null,null,\{timeout:1000\}\)/);
    assert.match(gate,/report\.reducedCompletion\.trusted,true/);
    assert.match(gate,/report\.reducedCompletion\.matches,true/);
    assert.match(gate,/report\.reducedCompletion\.connectedAtChange,false/);
    const arm=gate.indexOf("completionMediaProbe.addEventListener('change'");
    const emulate=gate.indexOf("await page.emulateMedia({reducedMotion:'reduce'})",arm);
    const wait=gate.indexOf('await page.waitForFunction(()=>window.reducedCompletionEvent!==null',emulate);
    assert.ok(arm>=0&&emulate>arm&&wait>emulate,'probe is armed before requesting the real media change');
    assert.doesNotMatch(gate.slice(arm,gate.indexOf("assert.equal(await page.evaluate(()=>tail.element.isConnected),false,mode)",wait)),/waitForTimeout|dispatchEvent|new MediaQueryListEvent/);
  }
});
