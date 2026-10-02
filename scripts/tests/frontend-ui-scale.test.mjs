import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/ui-scale.js', import.meta.url), 'utf8');
const flush = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const values = ['auto', '0.7', '0.75', '0.8', '0.85', '0.9', '1', '1.1', '1.25', '1.4', '1.5', '1.75', '2'];

function harness({ saved = null, width = 1920, height = 1000, density = 1, native = true, storageFails = false } = {}) {
  const store = new Map(saved === null ? [] : [['li-ui-scale', saved]]);
  const calls = [], events = [], toasts = [], timers = new Map();
  const windowListeners = new Map(), documentListeners = new Map();
  let nextTimer = 0, now = 0, document;
  const outside = { isConnected: true, focus() { document.activeElement = this; } };
  const pane = {
    hidden: false, modalHidden: false, isConnected: true, buttons: [],
    contains(node) { return this.buttons.includes(node); },
    closest() { return this.hidden || this.modalHidden ? this : null; },
    set innerHTML(html) {
      if (this.contains(document.activeElement)) document.activeElement = document.body;
      this.buttons.forEach(button => { button.isConnected = false; });
      this.html = html;
      this.buttons = [...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].map(([, markup, label]) => {
        const attrs = Object.fromEntries([...markup.matchAll(/([\w-]+)="([^"]*)"/g)].map(([, key, value]) => [key, value]));
        return {
          attrs, label, isConnected: true,
          dataset: { ...(attrs['data-scale'] === undefined ? {} : { scale: attrs['data-scale'] }), ...(attrs['data-theme-choice'] ? { themeChoice: attrs['data-theme-choice'] } : {}) },
          getAttribute(key) { return this.attrs[key] ?? null; },
          focus() { document.activeElement = this; },
          click() { return this.onclick?.(); },
        };
      });
    },
    querySelectorAll(selector) { return this.buttons.filter(button => selector.includes('data-scale') ? button.dataset.scale !== undefined : button.dataset.themeChoice !== undefined); },
    querySelector(selector) {
      const match = selector.match(/^\[data-(scale|theme-choice)="([^"]+)"\]$/);
      return match ? this.buttons.find(button => button.getAttribute(`data-${match[1]}`) === match[2]) : null;
    },
  };
  document = {
    documentElement: { dataset: {} }, body: outside, activeElement: outside,
    querySelector: () => pane,
    addEventListener: (name, listener) => documentListeners.set(name, listener),
    dispatchEvent: event => events.push(event),
  };
  const window = { devicePixelRatio: density, addEventListener: (name, listener) => windowListeners.set(name, listener) };
  const context = vm.createContext({ window, document, innerWidth: width, innerHeight: height,
    localStorage: { getItem(key) { if (storageFails) throw Error('unavailable'); return store.get(key) ?? null; }, setItem(key, value) { if (storageFails) throw Error('unavailable'); store.set(key, value); } },
    setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, { fn, at: now + delay }); return id; }, clearTimeout: id => timers.delete(id),
    invoke(command, args) {
      const call = { command, ...args }; calls.push(call);
      if (native === 'deferred') return new Promise((resolve, reject) => Object.assign(call, { resolve, reject }));
      if (native === 'throw') throw Error('unsupported');
      if (native === 'reject') return Promise.reject(Error('native failure'));
      return Promise.resolve(native);
    },
    toast: (message, type) => toasts.push({ message, type }),
    toggleTheme() { document.documentElement.dataset.theme = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light'; },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
  });
  vm.runInContext(source, context);
  const key = (key, options = {}, target = null) => {
    const event = { key, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.propagationStopped = true; }, ...options };
    if (target) target.onkeydown(event);
    if (!event.defaultPrevented) documentListeners.get('keydown')?.(event);
    return event;
  };
  const advance = async ms => {
    now += ms;
    for (const [id, timer] of [...timers]) if (timer.at <= now) { timers.delete(id); timer.fn(); }
    await flush();
  };
  return { ui: window.UiScale, context, document, window, pane, outside, calls, events, toasts, store, timers, key, advance,
    radio: value => pane.querySelector(`[data-scale="${value}"]`),
    theme: value => pane.querySelector(`[data-theme-choice="${value}"]`),
    resize(w, h, dpr = density) { context.innerWidth = w; context.innerHeight = h; window.devicePixelRatio = dpr; windowListeners.get('resize')(); },
  };
}

test('every supported step is visible, with one checked/tabbable radio per group through 200%', async () => {
  const h = harness(); await flush();
  assert.deepEqual(h.pane.querySelectorAll('[data-scale]').map(button => button.dataset.scale), values);
  for (const value of values) {
    await h.ui.set(value, false);
    const scales = h.pane.querySelectorAll('[data-scale]');
    assert.deepEqual(scales.filter(button => button.getAttribute('aria-checked') === 'true').map(button => button.dataset.scale), [value]);
    assert.deepEqual(scales.filter(button => button.getAttribute('tabindex') === '0').map(button => button.dataset.scale), [value]);
    assert.equal(h.pane.querySelectorAll('[data-theme-choice]').filter(button => button.getAttribute('tabindex') === '0').length, 1);
  }
  assert.equal(h.ui.current(), 2); assert.equal(h.ui.status().applied, 2);
});

test('explicit 200% persists across startup; invalid stored values use the existing automatic default', async () => {
  const first = harness(); await first.ui.set(2, false);
  const restored = harness({ saved: first.store.get('li-ui-scale'), width: 640, height: 480 }); await flush();
  assert.equal(restored.ui.setting(), 2); assert.equal(restored.ui.current(), 2); assert.equal(restored.ui.status().applied, 2);
  for (const saved of ['bogus', 'Infinity', 'NaN', '2.01', '0.5', '1.6', '', 'null']) {
    const h = harness({ saved, width: 1333, height: 703, density: 1.5 }); await flush();
    assert.equal(h.ui.setting(), 'auto', saved); assert.equal(h.ui.current(), 0.85, saved);
  }
});

test('invalid API values neither change the preference nor invoke native zoom', async () => {
  const h = harness(); await h.ui.set(1.5, false);
  const calls = h.calls.length;
  for (const value of [undefined, null, true, false, {}, [], [2], NaN, Infinity, -Infinity, 0, -1, 0.5, 2.1, 1.6, '', ' ', 'invalid']) {
    assert.equal(await h.ui.set(value), false);
    assert.equal(h.ui.setting(), 1.5); assert.equal(h.ui.current(), 1.5);
    assert.equal(h.store.get('li-ui-scale'), '1.5');
  }
  assert.equal(h.calls.length, calls); assert.equal(h.toasts.length, 0);
});

test('an unavailable storage backend still preserves the explicit choice for this session', async () => {
  const h = harness({ storageFails: true });
  await h.ui.set(2, false);
  h.resize(640, 400); await h.advance(500);
  assert.equal(h.ui.setting(), 2); assert.equal(h.ui.current(), 2);
  assert.deepEqual(h.calls.map(call => call.scale), [2]);
});

test('Ctrl/Meta shortcuts reach 200%, respect bounds, and Ctrl 0 preserves the automatic default', async () => {
  const h = harness(); await flush();
  for (let i = 0; i < 20; i++) assert.equal(h.key('+', { ctrlKey: true }).defaultPrevented, true);
  await flush(); assert.equal(h.ui.current(), 2); assert.equal(h.ui.setting(), 2);
  assert.equal(h.store.get('li-ui-scale'), '2');
  h.key('-', { metaKey: true }); await flush(); assert.equal(h.ui.current(), 1.75);
  for (let i = 0; i < 20; i++) h.key('_', { ctrlKey: true, shiftKey: true });
  await flush(); assert.equal(h.ui.current(), 0.7);
  h.key('=', { ctrlKey: true }); await flush(); assert.equal(h.ui.current(), 0.75);
  h.key('0', { ctrlKey: true }); await flush(); assert.equal(h.ui.setting(), 'auto');
  const plain = harness({ width: 1024, height: 680, native: false }); await flush(); assert.equal(plain.ui.current(), 0.9);
  const scaled = harness({ width: 1333, height: 703, density: 1.5, native: false }); await flush();
  await scaled.ui.set(2, false); scaled.key('0', { ctrlKey: true }); await flush();
  assert.equal(scaled.ui.current(), 0.85); assert.equal(scaled.ui.setting(), 'auto');
});

test('shortcuts leave composition, handled events, Alt and ordinary text entry alone', async () => {
  const h = harness(); await flush();
  for (const options of [{}, { ctrlKey: true, altKey: true }, { ctrlKey: true, isComposing: true }, { ctrlKey: true, keyCode: 229 }, { ctrlKey: true, defaultPrevented: true }]) {
    for (const key of ['+', '-', '0']) h.key(key, options);
  }
  for (const direction of [0, NaN, Infinity, '1', undefined]) h.ui.step(direction);
  await flush(); assert.equal(h.calls.length, 0); assert.equal(h.ui.current(), 1); assert.equal(h.ui.setting(), 'auto');
});

test('radiogroups support arrows/Home/End, native activation and focus restoration after every render', async () => {
  const h = harness(); await flush(); h.radio('auto').focus();
  assert.equal(h.key('End', {}, h.radio('auto')).propagationStopped, true, 'handled arrows cannot navigate a drawer behind settings'); await flush();
  assert.equal(h.ui.current(), 2); assert.equal(h.document.activeElement, h.radio('2'));
  h.key('ArrowLeft', {}, h.radio('2')); await flush();
  assert.equal(h.ui.current(), 1.75); assert.equal(h.document.activeElement, h.radio('1.75'));
  h.key('ArrowDown', {}, h.radio('1.75')); await flush(); assert.equal(h.ui.current(), 2);
  h.key('ArrowRight', {}, h.radio('2')); await flush(); assert.equal(h.ui.setting(), 'auto');
  h.key('ArrowUp', {}, h.radio('auto')); await flush(); assert.equal(h.ui.setting(), 2);
  h.key('Home', {}, h.radio('2')); await flush(); assert.equal(h.document.activeElement, h.radio('auto'));
  const auto = h.radio('auto');
  assert.equal(h.key(' ', {}, auto).defaultPrevented, undefined, 'Space remains native button activation');
  assert.equal(h.key('Enter', {}, auto).defaultPrevented, undefined, 'Enter remains native button activation');
  h.key('End', { isComposing: true }, auto); await flush(); assert.equal(h.ui.setting(), 'auto');
  h.theme('dark').focus(); h.key('ArrowRight', {}, h.theme('dark'));
  assert.equal(h.document.documentElement.dataset.theme, 'light'); assert.equal(h.document.activeElement, h.theme('light'));
  h.ui.renderPane(h.pane); assert.equal(h.document.activeElement, h.theme('light'));
});

test('a delayed receipt preserves the current focus, including moving out or closing settings', async () => {
  const h = harness({ native: 'deferred' }); await flush();
  h.radio('2').focus(); const selecting = h.radio('2').click(); await flush();
  assert.equal(h.document.activeElement, h.radio('2'), 'immediate replacement preserves focus');
  h.theme('dark').focus(); h.calls[0].resolve(true); await selecting;
  assert.equal(h.document.activeElement, h.theme('dark'), 'completion preserves the newer theme focus');
  const next = h.ui.set(1.5, false); await flush(); h.outside.focus(); h.calls[1].resolve(true); await next;
  assert.equal(h.document.activeElement, h.outside, 'completion never steals focus back from outside');
  const closing = h.ui.set(2, false); await flush(); h.pane.modalHidden = true; h.outside.focus();
  const before = h.pane.html; h.calls[2].resolve(true); await closing;
  assert.equal(h.document.activeElement, h.outside); assert.equal(h.pane.html, before, 'a closed dialog is not rerendered');
});

test('manual enlargement survives normal resize and an already scheduled automatic resize', async () => {
  const h = harness(); await flush(); h.resize(720, 480);
  assert.equal(h.timers.size, 1); await h.ui.set(2, false);
  h.resize(500, 300); await h.advance(500);
  assert.equal(h.ui.current(), 2); assert.equal(h.ui.setting(), 2); assert.equal(h.timers.size, 0);
  assert.deepEqual(h.calls.map(call => call.scale), [2]);
});

test('automatic resize waits for native settlement and uses confirmed zoom for geometry', async () => {
  const h = harness({ width: 1333, height: 703, density: 1.5, native: 'deferred' }); await flush();
  assert.equal(h.calls[0].scale, 0.85);
  h.resize(1333 / 0.85, 703 / 0.85, 1.5 * 0.85); await h.advance(250);
  assert.equal(h.calls.length, 1, 'no geometry feedback request while native zoom is pending');
  h.calls[0].resolve(true); await flush(); await h.advance(250);
  assert.equal(h.calls.length, 1); assert.equal(h.ui.current(), 0.85);
});

test('false and thrown receipts keep the request explicit without claiming native application', async () => {
  for (const native of [false, 'throw', 'reject']) {
    const h = harness({ native }); await flush();
    assert.equal(await h.ui.set(2), false);
    assert.equal(h.ui.current(), 2); assert.equal(h.ui.setting(), 2); assert.equal(h.ui.status().applied, 1); assert.equal(h.ui.status().state, 'failed');
    assert.match(h.pane.html, /200% selecionado.*Não foi possível aplicar.*100%/);
    assert.equal(h.toasts.at(-1).type, 'info'); assert.doesNotMatch(h.toasts.at(-1).message, /aplicado/);
    assert.equal(h.events.at(-1).detail.applied, false); assert.equal(h.events.at(-1).detail.appliedScale, 1);
    assert.equal(h.document.documentElement.dataset.uiScaleApplied, '1');
    await h.ui.set(2, false); assert.equal(h.calls.length, 2, 'explicitly selecting a failed size retries it');
  }
});

test('a failed change retains the last confirmed non-default scale', async () => {
  const h = harness({ native: 'deferred' }); await flush();
  const first = h.ui.set(1.5, false); await flush(); h.calls[0].resolve(true); await first;
  const second = h.ui.set(2, false); await flush(); h.calls[1].resolve(false); await second;
  assert.equal(h.ui.status().applied, 1.5); assert.match(h.pane.html, /Último tamanho confirmado: 150%/);
});

test('native writes are serialized; stale failures cannot replace or announce a newer intent', async () => {
  const h = harness({ native: 'deferred' }); await flush();
  const first = h.ui.set(1.5); await flush(); const second = h.ui.set(2); await flush();
  assert.equal(h.calls.length, 1, 'only one native write may run at a time');
  assert.equal(h.ui.current(), 2); assert.equal(h.ui.setting(), 2);
  h.calls[0].resolve(false); await flush();
  assert.equal(h.calls.length, 2); assert.equal(h.calls[1].scale, 2); assert.equal(h.events.length, 0); assert.equal(h.toasts.length, 0);
  h.calls[1].resolve(true); assert.equal(await first, false); assert.equal(await second, true);
  assert.equal(h.ui.status().applied, 2); assert.equal(h.ui.status().state, 'applied');
  assert.equal(h.events.length, 1); assert.equal(h.events[0].detail.scale, 2);
  assert.deepEqual(h.toasts, [{ message: 'Interface 200%', type: 'ok' }]);
});

test('queued intermediate sizes are coalesced, and stale success is not presented as the latest result', async () => {
  const h = harness({ native: 'deferred' }); await flush();
  const first = h.ui.set(1.5); await flush();
  const skipped = h.ui.set(1.75); const last = h.ui.set(2); await flush();
  h.calls[0].resolve(true); await flush();
  assert.deepEqual(h.calls.map(call => call.scale), [1.5, 2]);
  assert.equal(h.events.length, 0); assert.equal(h.toasts.length, 0);
  assert.equal(h.ui.status().state, 'pending'); assert.equal(h.ui.status().applied, 1.5);
  h.calls[1].resolve(false); await Promise.all([first, skipped, last]);
  assert.equal(h.ui.status().applied, 1.5); assert.equal(h.ui.current(), 2);
  assert.equal(h.events.length, 1); assert.equal(h.events[0].detail.applied, false);
  assert.equal(h.toasts.length, 1); assert.equal(h.toasts[0].type, 'info');
});

test('Auto requested during a failed manual zoom recalculates from unchanged geometry without requiring resize', async () => {
  const h = harness({ width: 1333, height: 703, density: 1.5, native: 'deferred' }); await flush();
  assert.equal(h.calls[0].scale, 0.85); h.calls[0].resolve(false); await flush();
  const manual = h.ui.set(2); await flush();
  const automatic = h.ui.set('auto'); await flush();
  assert.equal(h.calls.length, 2, 'Auto waits behind the in-flight manual native call');
  assert.equal(h.ui.setting(), 'auto'); assert.equal(h.ui.status().automaticPending, true);
  assert.match(h.pane.html, /Aguardando o ajuste em andamento para calcular o tamanho automático/);
  assert.doesNotMatch(h.pane.html, /Automático: 200%/);
  h.calls[1].resolve(false); await flush();
  assert.equal(h.calls[2].scale, 0.85, 'failed manual zoom did not change the viewport or its confirmed scale');
  h.calls[2].resolve(false); assert.equal(await manual, false); assert.equal(await automatic, false);
  assert.equal(h.ui.current(), 0.85); assert.equal(h.ui.status().applied, 1); assert.equal(h.ui.status().state, 'failed');
  assert.equal(h.ui.status().automaticPending, false);
  assert.equal(h.timers.size, 0, 'the correct result does not depend on a subsequent resize');
  assert.equal(h.events.length, 2, 'only startup and the latest Auto result are published');
  assert.equal(h.events.at(-1).detail.scale, 0.85); assert.equal(h.events.at(-1).detail.applied, false);
  assert.deepEqual(h.toasts, [{ message: 'Tamanho 85% selecionado, mas não foi possível aplicar nesta janela.', type: 'info' }]);
});

test('deferred Auto uses the viewport and confirmed scale after successful manual zoom', async () => {
  const h = harness({ width: 1333, height: 703, density: 1.5, native: 'deferred' }); await flush();
  h.calls[0].resolve(false); await flush();
  const manual = h.ui.set(2); await flush(); const automatic = h.ui.set('auto'); await flush();
  assert.equal(h.ui.status().automaticPending, true);
  // Change observed geometry without emitting resize: the receipt, not a timer,
  // must make the pending Auto intent read the now-zoomed viewport correctly.
  h.context.innerWidth = 1333 / 2; h.context.innerHeight = 703 / 2; h.window.devicePixelRatio = 1.5 * 2;
  h.calls[1].resolve(true); await flush();
  assert.deepEqual(h.calls.map(call => call.scale), [0.85, 2, 0.85]);
  assert.equal(h.ui.current(), 0.85); assert.equal(h.ui.status().applied, 2);
  h.context.innerWidth = 1333 / 0.85; h.context.innerHeight = 703 / 0.85; h.window.devicePixelRatio = 1.5 * 0.85;
  h.calls[2].resolve(true); assert.equal(await manual, false); assert.equal(await automatic, true);
  assert.equal(h.ui.status().applied, 0.85); assert.equal(h.ui.status().state, 'applied');
  assert.equal(h.ui.status().automaticPending, false); assert.equal(h.timers.size, 0);
  assert.deepEqual(h.toasts, [{ message: 'Interface automática (85%)', type: 'ok' }]);
});

test('a newer manual choice equal to the in-flight scale supersedes a deferred Auto intent', async () => {
  const h = harness({ native: 'deferred' }); await flush();
  const first = h.ui.set(2); await flush();
  const automatic = h.ui.set('auto'); const latest = h.ui.set(2); await flush();
  assert.equal(h.ui.setting(), 2); assert.equal(h.ui.current(), 2); assert.equal(h.ui.status().automaticPending, false);
  h.calls[0].resolve(false); await flush();
  assert.deepEqual(h.calls.map(call => call.scale), [2, 2], 'deferred Auto cannot replace the newest manual intent');
  h.calls[1].resolve(true); assert.equal(await first, false); assert.equal(await automatic, false); assert.equal(await latest, true);
  assert.equal(h.ui.setting(), 2); assert.equal(h.ui.status().applied, 2);
  assert.equal(h.events.length, 1); assert.deepEqual(h.toasts, [{ message: 'Interface 200%', type: 'ok' }]);
});

test('Auto supersedes a not-yet-started manual request without using its unconfirmed scale', async () => {
  const h = harness({ width: 1333, height: 703, density: 1.5, native: 'deferred' }); await flush();
  h.calls[0].resolve(false); await flush();
  const skipped = h.ui.set(2); const automatic = h.ui.set('auto'); await flush();
  assert.deepEqual(h.calls.map(call => call.scale), [0.85, 0.85], 'queued 200% was coalesced before any native write');
  h.calls[1].resolve(false); assert.equal(await skipped, false); assert.equal(await automatic, false);
  assert.equal(h.ui.setting(), 'auto'); assert.equal(h.ui.current(), 0.85); assert.equal(h.ui.status().applied, 1);
  assert.equal(h.ui.status().automaticPending, false); assert.equal(h.timers.size, 0);
  assert.deepEqual(h.toasts, [{ message: 'Tamanho 85% selecionado, mas não foi possível aplicar nesta janela.', type: 'info' }]);
});
