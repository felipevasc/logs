import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
const app = read('frontend/app.js');

// Execute the installed controller. This deliberately small DOM models focus,
// disabled/hidden controls and capture/bubble ordering; browser QA owns layout.
function harness({ appWrapper = false, nativeCallbacks = false, captureReturnFocus } = {}) {
  const listeners = new Map(), windowListeners = new Map(), microtasks = [], timers = [], changes = [], effects = [];
  let document;
  class Element {
    constructor(tag) { this.tagName = tag.toUpperCase(); this.children = []; this.style = {}; this.attrs = {}; this.dataset = {}; this.className = ''; this.tabIndex = tag === 'button' ? 0 : -1; this.rect = { left: 20, top: 40, bottom: 70, width: 230, height: 160 }; }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    append(...children) { children.forEach(child => this.appendChild(child)); }
    appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
    remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; if (this.contains(document.activeElement)) document.activeElement = document.body; }
    contains(node) { return this === node || this.children.some(child => child.contains(node)); }
    matches(selector) { return selector.split(',').some(part => {
      part = part.trim();
      if (part === '[hidden]') return !!this.hidden;
      if (part === '[inert]') return !!this.inert;
      if (part === '[tabindex]') return Object.hasOwn(this.attrs, 'tabindex');
      if (part.startsWith('.')) return this.className.split(' ').includes(part.slice(1));
      const attr = part.match(/^\[([^=]+)=["']([^"']+)["']\]$/);
      return attr ? this.getAttribute(attr[1]) === attr[2] : this.tagName.toLowerCase() === part;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    getClientRects() { return this.isConnected && !this.closest('[hidden]') ? [this.rect] : []; }
    getBoundingClientRect() { return this.rect; }
    focus() { document.activeElement = this; dispatch('focusin', { target: this }); }
    scrollIntoView() { this.scrolled = true; }
    click() { if (!this.disabled) dispatch('click', { target: this, detail: 0, clientX: 0, clientY: 0 }); }
    dispatchEvent(event) { dispatch(event.type, { ...event, target: this }); }
  }
  document = {
    createElement: tag => new Element(tag), documentElement: { clientWidth: 800, clientHeight: 600 },
    addEventListener(type, fn, capture) { const list = listeners.get(type) || []; list.push({ fn, capture }); listeners.set(type, list); },
    querySelectorAll: selector => document.body.querySelectorAll(selector),
  };
  document.body = new Element('body'); document.activeElement = document.body;
  function dispatch(type, options = {}) {
    const event = { type, target: document.activeElement, defaultPrevented: false, stopped: false, ...options,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; },
      stopImmediatePropagation() { this.stopped = true; this.immediate = true; } };
    const invoke = (fn, currentTarget, eventPhase) => {
      if (!fn) return;
      Object.assign(event, { currentTarget, eventPhase }); fn(event);
      // Native browser events may checkpoint after each JS callback. A normal
      // synchronous dispatchEvent call keeps its caller's JS stack instead.
      if (nativeCallbacks) while (microtasks.length) microtasks.shift()();
    };
    invoke(windowListeners.get(type), window, 1);
    for (const listener of listeners.get(type) || []) if (listener.capture && !event.immediate) invoke(listener.fn, document, 1);
    if (!event.stopped) {
      for (let node = event.target; node; node = node.parentElement) {
        invoke(node[`on${type}`]?.bind(node), node, node === event.target ? 2 : 3); if (event.stopped) break;
      }
    }
    for (const listener of listeners.get(type) || []) if (!listener.capture && !event.stopped) invoke(listener.fn, document, 3);
    Object.assign(event, { eventPhase: 0, currentTarget: null });
    return event;
  }
  const fallback = document.body.appendChild(new Element('button'));
  const drawer = document.body.appendChild(new Element('div')); drawer.hidden = true;
  const origin = document.body.appendChild(new Element('button'));
  const window = {
    addEventListener: (type, fn) => windowListeners.set(type, fn),
    AnalysisContexts: { capture: () => ({ id: 1 }), isCurrent: () => true },
    CanonicalFields: { cancel: () => effects.push('cancel') },
  };
  const context = vm.createContext({ window, document, queueMicrotask: fn => microtasks.push(fn), setTimeout: fn => timers.push(fn),
    getComputedStyle: node => ({ visibility: node.visibility || 'visible' }),
    MouseEvent: class { constructor(type, init) { this.type = type; Object.assign(this, init); } },
    $: selector => selector === '#drawer' ? drawer : fallback, toast: text => effects.push(text),
  });
  vm.runInContext(read('frontend/context-menu.js'), context);
  let controller;
  if (appWrapper) {
    vm.runInContext(app.slice(app.indexOf('let ctxEl = null;'), app.indexOf('\nconst trunc =')), context);
    controller = vm.runInContext('ctxMenu', context);
  } else controller = window.ContextMenu.create({ fallbackFocus: () => [fallback], captureReturnFocus, onChange: menu => changes.push(menu) });
  origin.focus();
  const open = (items, options) => appWrapper ? context.showCtxMenu(10, 20, items, options) : controller.open(10, 20, items, options);
  const key = (key, extras = {}) => dispatch('keydown', { key, ...extras });
  return { document, window, context, controller, origin, fallback, effects, changes, open, key, dispatch,
    items: () => controller.element.querySelectorAll('[role="menuitem"]'),
    flush: () => { while (microtasks.length) microtasks.shift()(); while (timers.length) timers.shift()(); },
    node: (tag = 'button', parent = document.body) => parent.appendChild(new Element(tag)),
    resize: () => windowListeners.get('resize')(),
  };
}
const choice = (label, onClick = () => {}) => ({ label, icon: 'fa-filter', onClick });

test('named menu, decorative icons, separator, safe labels and initial focus', () => {
  const h = harness();
  const menu = h.open([{ ...choice('Unavailable'), disabled: true }, { sep: true }, choice('<b>Literal</b>'), choice('Last')]);
  assert.equal(menu.getAttribute('role'), 'menu');
  assert.equal(menu.getAttribute('aria-label'), 'Ações do contexto');
  assert.equal(menu.children[1].getAttribute('role'), 'separator');
  const first = h.items()[1];
  assert.equal(h.document.activeElement, h.items()[0]);
  assert.equal(h.items()[0].getAttribute('aria-disabled'), 'true');
  assert.equal(first.type, 'button'); assert.equal(first.tabIndex, -1);
  assert.equal(first.children[0].getAttribute('aria-hidden'), 'true');
  assert.equal(first.children[1].textContent, '<b>Literal</b>');
});

test('arrows include disabled actions; Home/End focus the menu endpoints', () => {
  const h = harness(); h.open([choice('First'), { sep: true }, { ...choice('Disabled'), disabled: true }, choice('Last')]);
  const [first, disabled, last] = h.items();
  h.key('ArrowUp'); assert.equal(h.document.activeElement, last);
  h.key('ArrowDown'); assert.equal(h.document.activeElement, first);
  h.key('End'); assert.equal(h.document.activeElement, last);
  h.key('Home'); assert.equal(h.document.activeElement, first);
  h.key('ArrowDown'); assert.equal(h.document.activeElement, disabled);
  h.key('ArrowDown'); assert.equal(h.document.activeElement, last); assert.equal(last.scrolled, true);
});

test('disabled items expose their reason and never activate from keys or pointer', () => {
  const h = harness(); let calls = 0;
  h.open([{ ...choice('Unavailable', () => calls++), disabled: true, title: 'Requires a loaded source' }]);
  const button = h.items()[0];
  assert.equal(h.document.activeElement, button);
  assert.equal(button.getAttribute('aria-description'), 'Requires a loaded source');
  h.key('Enter'); h.key(' '); button.click();
  assert.equal(calls, 0); assert.ok(h.controller.element);
});

test('Enter and Space activate exactly once, reject repeats and return focus before the action', () => {
  for (const key of ['Enter', ' ']) {
    const h = harness(); let calls = 0;
    h.open([choice('Run', () => { calls++; assert.equal(h.document.activeElement, h.origin); })]);
    const button = h.items()[0];
    h.key(key, { repeat: true }); assert.equal(calls, 0);
    assert.equal(h.key(key).defaultPrevented, true); assert.equal(calls, 1);
    button.onclick(); assert.equal(calls, 1, 'detached menu cannot activate a second time');
    assert.equal(h.controller.element, null);
  }
});

test('Escape consumes only the menu and composition leaves it untouched', () => {
  const h = harness(); let dismissed = 0;
  h.document.addEventListener('keydown', () => dismissed++);
  h.open([choice('Action')]);
  h.key('Escape', { isComposing: true }); assert.ok(h.controller.element);
  dismissed = 0;
  const event = h.key('Escape');
  assert.equal(event.defaultPrevented, true); assert.equal(dismissed, 0);
  assert.equal(h.document.activeElement, h.origin);
  h.key('Escape'); assert.equal(dismissed, 1, 'the next Escape belongs to the underlying surface');
});

test('empty and all-disabled menus keep keyboard focus and can be dismissed', () => {
  for (const items of [[], [{ ...choice('Disabled'), disabled: true }]]) {
    const h = harness(); const menu = h.open(items);
    assert.equal(h.document.activeElement, h.items()[0] || menu);
    for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' ']) h.key(key);
    assert.equal(h.controller.element, menu); h.key('Escape'); assert.equal(h.document.activeElement, h.origin);
  }
});

test('Shift+F10 and ContextMenu reuse the focused caller without firing its primary click', () => {
  for (const key of ['F10', 'ContextMenu']) for (const [tag, className] of [['button', 'source-path-toggle'], ['button', 'source-menu-trigger'], ['input', 'source-check'], ['textarea', 'unmarked-editable']]) {
    const h = harness(); let calls = 0, primary = 0;
    const row = h.node('div'), button = h.node(tag, row); button.className = className;
    row.oncontextmenu = event => { calls++; event.preventDefault(); h.open([choice('Alternate')]); };
    button.onclick = () => primary++;
    button.focus();
    assert.equal(h.key(key, { shiftKey: key === 'F10' }).defaultPrevented, true);
    assert.equal(calls, 1); assert.equal(primary, 0);
    assert.equal(h.controller.element.style.left, '20px'); assert.equal(h.controller.element.style.top, '70px');
    h.key('Escape'); assert.equal(h.document.activeElement, button);
  }
});

test('explicit native context-menu opt-out preserves both keyboard commands without synthetic dispatch', () => {
  for (const key of ['F10', 'ContextMenu']) for (const marker of ['', 'true']) {
    const h = harness(), row = h.node('div'), textarea = h.node('textarea', row);
    let calls = 0, dispatches = 0;
    row.oncontextmenu = () => { calls++; };
    textarea.setAttribute('data-native-context-menu', marker);
    textarea.dispatchEvent = () => { dispatches++; };
    textarea.focus();
    const event = h.key(key, { shiftKey: key === 'F10' });
    assert.equal(event.defaultPrevented, false); assert.equal(event.stopped, false);
    assert.equal(dispatches, 0); assert.equal(calls, 0); assert.equal(h.controller.element, null);
    assert.equal(h.document.activeElement, textarea);
  }
});

test('native-menu marker never bypasses key handling or Escape inside an already open custom menu', () => {
  const h = harness(); h.open([choice('Action')]);
  h.items()[0].setAttribute('data-native-context-menu', '');
  for (const key of ['ContextMenu', 'F10']) {
    assert.equal(h.key(key, { shiftKey: key === 'F10' }).defaultPrevented, true);
    assert.ok(h.controller.element);
  }
  assert.equal(h.key('Escape').defaultPrevented, true);
  assert.equal(h.controller.element, null); assert.equal(h.document.activeElement, h.origin);
});

test('invocation does not hijack unrelated controls, IME or modified shortcuts', () => {
  const h = harness();
  assert.equal(h.key('ContextMenu').defaultPrevented, false);
  h.origin.oncontextmenu = () => h.open([choice('Action')]);
  for (const extras of [{ isComposing: true }, { keyCode: 229 }, { repeat: true }, { ctrlKey: true }, { altKey: true }, { metaKey: true }]) {
    h.key('F10', { shiftKey: true, ...extras }); assert.equal(h.controller.element, null);
  }
});

test('hidden, disabled, inert and disconnected return targets use a valid fallback', () => {
  for (const invalidate of [node => node.remove(), node => { node.hidden = true; }, node => { node.disabled = true; }, node => { node.inert = true; }, node => { node.visibility = 'hidden'; }]) {
    const h = harness(); h.open([choice('Action')]); invalidate(h.origin); h.key('Escape');
    assert.equal(h.document.activeElement, h.fallback);
  }
});

test('outside clicks, focus changes and scrolling dismiss without stealing focus', () => {
  const h = harness();
  h.open([choice('Action')]); const outside = h.node(); outside.focus();
  assert.equal(h.controller.element, null); assert.equal(h.document.activeElement, outside);
  h.origin.focus(); h.open([choice('Action')]);
  h.dispatch('click', { target: outside, detail: 1, clientX: 30, clientY: 40 });
  assert.equal(h.controller.element, null); assert.notEqual(h.document.activeElement, h.origin);
  h.flush(); h.origin.focus(); h.open([choice('Action')]);
  h.dispatch('scroll', { target: h.controller.element }); assert.ok(h.controller.element);
  h.document.documentElement.scrollTop = 50;
  h.dispatch('scroll', { target: h.document }); assert.equal(h.controller.element, null);
});

test('queued caller scroll notifications preserve a just-opened menu; new movement dismisses it', () => {
  const h = harness(), drawer = h.node('div'), caller = h.node('button', drawer);
  drawer.scrollTop = 420; caller.focus();
  h.open([choice('Action')]); const menu = h.controller.element;
  h.dispatch('scroll', { target: drawer });
  assert.equal(h.controller.element, menu, 'the scroll position already existed at open');
  h.dispatch('scroll', { target: h.document });
  assert.equal(h.controller.element, menu, 'unchanged viewport notification is harmless');
  const unrelated = h.node('div'); unrelated.scrollTop = 80;
  h.dispatch('scroll', { target: unrelated });
  assert.equal(h.controller.element, menu, 'an unrelated surface does not move the caller');
  drawer.scrollTop = 440; h.dispatch('scroll', { target: drawer });
  assert.equal(h.controller.element, null, 'actual originating-container movement dismisses');
});

test('Tab resumes from the caller and leaves enclosing modal tab handling available', () => {
  const h = harness(); let modalTabs = 0;
  h.document.addEventListener('keydown', event => { if (event.key === 'Tab') modalTabs++; });
  h.open([choice('Action')]); const event = h.key('Tab');
  assert.equal(h.controller.element, null); assert.equal(h.document.activeElement, h.origin);
  assert.equal(event.defaultPrevented, false); assert.equal(modalTabs, 1);
});

test('viewport coordinates clamp and a resized menu is repositioned', () => {
  const h = harness();
  const menu = h.controller.open(1000, 1000, [choice('Action')]);
  assert.equal(menu.style.left, '562px'); assert.equal(menu.style.top, '432px');
  assert.equal(menu.style.maxWidth, 'calc(100vw - 16px)');
  h.document.documentElement.clientWidth = 400; h.document.documentElement.clientHeight = 300; h.resize();
  assert.equal(menu.style.left, '162px'); assert.equal(menu.style.top, '132px');
});

test('replacement menus preserve original focus and ignore old detached actions', () => {
  const h = harness(); let oldCalls = 0;
  h.open([choice('Old', () => oldCalls++)]); const old = h.items()[0];
  h.open([choice('New')]); old.onclick(); assert.equal(oldCalls, 0);
  h.key('Escape'); assert.equal(h.document.activeElement, h.origin);
});

test('captured logical return resolves a replacement caller after rerender during the menu', () => {
  let target, captured;
  const h = harness({ captureReturnFocus: (source, origin) => { captured = [source, origin]; return () => target; } });
  target = h.origin; h.open([choice('Action')]);
  assert.deepEqual(captured, [h.origin, h.origin]);
  h.origin.remove(); target = h.node();
  h.key('Escape'); assert.equal(h.document.activeElement, target);
  assert.equal(h.controller.element, null);
});

test('a stale logical owner rejects even a connected origin and uses the valid fallback', () => {
  for (const disconnected of [false, true]) {
    let owner = 'source-a/case-a', target;
    const h = harness({ captureReturnFocus: () => { const captured = owner; return () => captured === owner ? target : null; } });
    target = h.origin; h.open([choice('Action')]);
    owner = 'source-b/case-b'; if (disconnected) h.origin.remove();
    h.key('Escape'); assert.equal(h.document.activeElement, h.fallback);
  }
});

test('missing, hidden or disabled logical replacements never fall back to the stale origin', () => {
  for (const invalidate of [() => null, node => { node.hidden = true; return node; }, node => { node.disabled = true; return node; }]) {
    let target;
    const h = harness({ captureReturnFocus: () => () => target });
    target = h.origin; h.open([choice('Action')]); target = invalidate(h.node());
    h.key('Escape'); assert.equal(h.document.activeElement, h.fallback);
  }
});

test('replacing an open menu preserves its original logical guard rather than recapturing a newer owner', () => {
  let owner = 1, target, captures = 0;
  const h = harness({ captureReturnFocus: () => { const captured = owner; captures++; return () => captured === owner ? target : null; } });
  target = h.origin; h.open([choice('Page one')]);
  h.origin.remove(); target = h.node();
  h.open([choice('Page two')]); assert.equal(captures, 1);
  owner = 2; h.open([choice('Page three')]); assert.equal(captures, 1);
  h.key('Escape'); assert.equal(h.document.activeElement, h.fallback);
});

test('paging activation can reopen a menu after its caller rerenders and Escape returns to the logical caller', () => {
  let target;
  const h = harness({ captureReturnFocus: () => () => target });
  target = h.origin;
  h.open([choice('Next page', () => h.open([choice('Page two')]))]);
  h.origin.remove(); target = h.node(); h.key('Enter');
  assert.ok(h.controller.element); assert.equal(h.document.activeElement, h.items()[0]);
  h.key('Escape'); assert.equal(h.document.activeElement, target);
});

test('an action rerender re-resolves its logical return but respects focus moved to an editor', () => {
  for (const moveToEditor of [false, true]) {
    let target;
    const h = harness({ captureReturnFocus: () => () => target }), editor = h.node('input');
    target = h.origin;
    h.open([choice('Edit', () => { target.remove(); target = h.node(); if (moveToEditor) editor.focus(); })]);
    h.key('Enter'); assert.equal(h.document.activeElement, moveToEditor ? editor : target);
  }
});

test('closing a logical menu does not leak its return resolver into a later unrelated menu', () => {
  const h = harness({ captureReturnFocus: source => { const target = source; return () => target; } });
  h.open([choice('First')]); h.key('Escape');
  const next = h.node(); next.focus(); h.open([choice('Later')]); h.key('Escape');
  assert.equal(h.document.activeElement, next);
});

test('app admission remains captured at open and a new menu cancels canonical work', () => {
  const h = harness({ appWrapper: true }); let calls = 0;
  h.open([choice('Action', () => calls++)]);
  assert.deepEqual(h.effects, ['cancel']);
  h.window.AnalysisContexts.isCurrent = () => false;
  h.key('Enter'); assert.equal(calls, 0); assert.match(h.effects.at(-1), /O contexto mudou/);
  h.window.AnalysisContexts.isCurrent = () => true;
  h.open([choice('Action', () => calls++)]); h.key(' '); assert.equal(calls, 1);
  assert.equal(h.effects.filter(effect => effect === 'cancel').length, 2);
});


test('menu Escape is consumed before document capture handlers on underlying surfaces', () => {
  const h = harness(); let underlyingCapture = 0;
  h.document.addEventListener('keydown', () => underlyingCapture++, true);
  h.open([choice('Action')]); h.key('Escape');
  assert.equal(underlyingCapture, 0);
});

test('an action that removes its caller restores fallback without stealing new editor focus', () => {
  const h = harness(); h.open([choice('Remove', () => h.origin.remove())]); h.key('Enter');
  assert.equal(h.document.activeElement, h.fallback);
  const origin = h.node(), editor = h.node('input'); origin.focus();
  h.open([choice('Edit', () => { origin.remove(); editor.focus(); })]); h.key('Enter');
  assert.equal(h.document.activeElement, editor);
});


test('native callback microtasks do not lose the pointer caller before its target handler', () => {
  const h = harness({ nativeCallbacks: true }), caller = h.node();
  let checkpoint = false;
  h.document.addEventListener('contextmenu', () => h.context.queueMicrotask(() => { checkpoint = true; }), true);
  caller.oncontextmenu = event => { assert.equal(checkpoint, true); event.preventDefault(); h.open([choice('Action')]); };
  h.dispatch('contextmenu', { target: caller, clientX: 120, clientY: 150, detail: 0 });
  h.key('Escape'); assert.equal(h.document.activeElement, caller, 'the pointer caller wins over unrelated previous focus');
});

test('native keyboard click keeps its anchor across callback microtasks', () => {
  const h = harness({ nativeCallbacks: true }), caller = h.node();
  caller.rect = { ...caller.rect, left: 300, bottom: 220 };
  caller.onclick = () => h.controller.open(0, 0, [choice('Action')]);
  h.dispatch('click', { target: caller, detail: 0, clientX: 0, clientY: 0 });
  assert.equal(h.controller.element.style.left, '300px'); assert.equal(h.controller.element.style.top, '220px');
  h.key('Escape'); assert.equal(h.document.activeElement, caller);
});

test('finished input dispatch is never reused by later async work before timer cleanup', () => {
  const h = harness({ nativeCallbacks: true }), caller = h.node(), newer = h.node();
  let later;
  caller.onclick = () => { later = () => h.open([choice('Async result')]); };
  const event = h.dispatch('click', { target: caller, detail: 0, clientX: 0, clientY: 0 });
  assert.equal(event.eventPhase, 0); newer.focus(); later();
  assert.equal(h.controller.element.style.left, '10px'); assert.equal(h.controller.element.style.top, '20px');
  h.key('Escape'); assert.equal(h.document.activeElement, newer);
});

test('controller loads before app and is copied into the frontend bundle', () => {
  const html = read('frontend/index.html'), bundle = read('scripts/prepare-frontend.mjs');
  assert.ok(html.indexOf('src="context-menu.js"') < html.indexOf('src="app.js"'));
  assert.ok(bundle.includes('"context-menu.js"'));
});

// App-owned logical targets use the field identity and originating tree, never
// an index or a same-named field in another surface.
function explorerField(h, box, column = 'message') {
  const row = h.node('div', box); row.className = 'field-row'; row.dataset.column = column;
  const field = h.node('button', row); field.className = 'field-item';
  const more = h.node('button', row); more.className = 'field-adv-btn';
  return { row, field, more };
}
function explorerBox(h, scope = 'dataset') {
  const box = h.node('div'); box.className = 'explore-tree-sync'; box.dataset.treeScope = scope; return box;
}

test('app hook resolves only the same field in the originating tree after a background rerender', () => {
  const h = harness({ appWrapper: true }), other = explorerBox(h), box = explorerBox(h);
  const decoy = explorerField(h, other), original = explorerField(h, box);
  const icon = h.node('i', original.field);
  h.open([choice('Action')], { trigger: icon });
  original.row.remove(); const replacement = explorerField(h, box);
  h.key('Escape'); assert.equal(h.document.activeElement, replacement.field);
  assert.notEqual(h.document.activeElement, decoy.field);
});

test('app hook rejects a changed AnalysisContext before considering a connected or replaced field', () => {
  for (const rerender of [false, true]) {
    const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
    h.open([choice('Action')], { trigger: original.field });
    if (rerender) { original.row.remove(); explorerField(h, box); }
    h.window.AnalysisContexts.isCurrent = () => false;
    h.key('Escape'); assert.equal(h.document.activeElement, h.fallback);
  }
});

test('app hook rejects removed fields, replaced containers and changes to tree scope', () => {
  for (const change of ['field', 'container', 'scope']) {
    const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
    h.open([choice('Action')], { trigger: original.field });
    if (change === 'field') { original.row.remove(); explorerField(h, box, 'different_column'); }
    if (change === 'container') { box.remove(); explorerField(h, explorerBox(h)); }
    if (change === 'scope') box.dataset.treeScope = 'case';
    h.key('Escape'); assert.equal(h.document.activeElement, h.fallback);
  }
});

test('app hook keeps the captured owner when a field menu replaces itself after rerender', () => {
  const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
  h.open([choice('Page one')], { trigger: original.field });
  original.row.remove(); const replacement = explorerField(h, box);
  h.open([choice('Page two')]); h.key('Escape');
  assert.equal(h.document.activeElement, replacement.field);
});


test('app hook restores the equivalent field ellipsis after rerender for Escape and Tab', () => {
  for (const key of ['Escape', 'Tab']) {
    const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
    const icon = h.node('i', original.more);
    h.open([choice('Action')], { trigger: icon });
    original.row.remove(); const replacement = explorerField(h, box);
    h.key(key); assert.equal(h.document.activeElement, replacement.more, key);
    assert.notEqual(h.document.activeElement, replacement.field, 'the ellipsis and field-name controls have distinct return identities');
  }
});

test('field ellipsis keeps its original tree and owner; missing controls never switch to the name button', () => {
  for (const change of ['button', 'column', 'container', 'scope', 'owner-connected', 'owner-replaced']) {
    const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
    h.open([choice('Action')], { trigger: original.more });
    if (change === 'button') original.more.remove();
    if (change === 'column') { original.row.remove(); explorerField(h, box, 'other'); }
    if (change === 'container') { box.remove(); explorerField(h, explorerBox(h)); }
    if (change === 'scope') box.dataset.treeScope = 'case';
    if (change === 'owner-replaced') { original.row.remove(); explorerField(h, box); }
    if (change.startsWith('owner')) h.window.AnalysisContexts.isCurrent = () => false;
    h.key('Escape'); assert.equal(h.document.activeElement, h.fallback, change);
  }
});

test('field ellipsis paging preserves the original logical control across replacement', () => {
  const h = harness({ appWrapper: true }), box = explorerBox(h), original = explorerField(h, box);
  h.open([choice('Page one')], { trigger: original.more });
  original.row.remove(); const replacement = explorerField(h, box);
  h.open([choice('Page two')]); h.key('Escape');
  assert.equal(h.document.activeElement, replacement.more);
});
