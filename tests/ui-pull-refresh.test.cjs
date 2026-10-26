const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const code = source.slice(source.indexOf('// ---------------------------------------------------------------- page refresh\n'), source.indexOf('// ---------------------------------------------------------------- routing + timers'));
const unload = source.slice(source.indexOf("window.addEventListener('beforeunload'"), source.indexOf("window.addEventListener('storage'"));

function dashboard(options = {}) {
  const listeners = new Map();
  const frames = [];
  const timers = new Map();
  const classes = new Set();
  const calls = { fetch: 0, reload: 0, toasts: [], focused: 0 };
  const label = { textContent: '' };
  const indicator = { hidden: true, style: { setProperty() {} }, classList: { toggle() {} } };
  const modal = { open: false, showModal() { this.open = true; }, close() { this.open = false; } };
  const buttons = [{ disabled: false }, { disabled: false }];
  const state = { dirty: false, rawDirty: false };
  const body = {};
  const element = (extra = {}) => ({ closest: () => false, parentElement: body, scrollHeight: 0, clientHeight: 0, scrollWidth: 0, clientWidth: 0, ...extra });
  const target = element();
  const listen = (type, fn, settings) => {
    if (!listeners.has(type)) listeners.set(type, []);
    listeners.get(type).push({ fn, settings });
  };
  const context = {
    STANDALONE: options.standalone ?? true,
    navigator: { userAgent: 'iPhone', platform: 'iPhone', maxTouchPoints: 5, ...options.navigator },
    S: { config: { busy: false }, play: null },
    document: { body, activeElement: null, documentElement: { classList: { add: (name) => classes.add(name) } }, addEventListener: listen,
      querySelector: () => modal.open ? modal : null },
    scrollY: 0,
    addEventListener: listen,
    getComputedStyle: (el) => ({ overflowY: el.overflowY || 'visible', overflowX: el.overflowX || 'visible' }),
    requestAnimationFrame: (fn) => frames.push(fn),
    setTimeout: (fn, ms) => { const id = timers.size + 1; timers.set(id, { fn, ms }); return id; },
    clearTimeout: (id) => timers.delete(id),
    AbortController,
    mob: () => true,
    configDirty: () => state.dirty,
    rawDirty: () => state.rawDirty,
    location: { reload: () => { calls.reload++; } },
    toast: (text) => calls.toasts.push(text),
    fetch: (...args) => { calls.fetch++; calls.fetchArgs = args; return options.fetch ? options.fetch(...args) : Promise.resolve({ ok: true }); },
    $: (selector) => {
      if (selector === '#pull-refresh') return indicator;
      if (selector === '[data-refresh-label]') return label;
      if (selector === '#page-refresh-modal') return modal;
      if (selector === '[data-act="cancel-page-refresh"]') return { focus: () => { calls.focused++; } };
      if (selector === '#mbar' || selector === '#bar') return { getBoundingClientRect: () => ({ bottom: 96 }) };
      throw new Error(selector);
    },
    $$: () => buttons,
  };
  context.window = context;
  vm.createContext(context);
  vm.runInContext(code + '\n' + unload + '\ninstallPullToRefresh();', context);
  const touch = (y, x = 100, id = 1) => ({ identifier: id, clientX: x, clientY: y });
  const dispatch = (type, extra = {}) => {
    const event = { target, touches: [], changedTouches: [], cancelable: true, prevented: false, preventDefault() { this.prevented = true; }, ...extra };
    for (const { fn } of listeners.get(type) || []) fn(event);
    return event;
  };
  return {
    context, calls, listeners, frames, timers, classes, label, indicator, modal, buttons, state, element, target, touch, dispatch,
    start(y = 100, extra = {}) { return dispatch('touchstart', { touches: [touch(y)], ...extra }); },
    move(y, x = 100, extra = {}) { return dispatch('touchmove', { touches: [touch(y, x)], ...extra }); },
    end() { return dispatch('touchend', { changedTouches: [touch(200)] }); },
    paint() { while (frames.length) frames.shift()(); },
    run(expr) { return vm.runInContext(expr, context); },
  };
}

const settled = () => new Promise(setImmediate);

test('only installed iOS and iPadOS own the gesture and disable native overscroll', () => {
  for (const options of [{}, { navigator: { userAgent: 'Macintosh', platform: 'MacIntel', maxTouchPoints: 5 } }]) {
    const d = dashboard(options);
    assert.ok(d.classes.has('ios-pull-refresh'));
    assert.equal(d.listeners.get('touchmove')[0].settings.passive, false);
    assert.equal(d.listeners.get('touchstart')[0].settings.passive, true);
  }
  for (const options of [{ standalone: false }, { navigator: { userAgent: 'Android', platform: 'Linux' } }, { navigator: { userAgent: 'Macintosh', platform: 'MacIntel', maxTouchPoints: 0 } }]) {
    const d = dashboard(options);
    assert.equal(d.listeners.has('touchmove'), false);
    assert.equal(d.classes.size, 0);
  }
});

test('a deliberate pull shows progress and reloads once, only after release', async () => {
  const d = dashboard();
  d.start();
  assert.equal(d.move(140).prevented, true);
  d.paint();
  assert.equal(d.label.textContent, 'Pull to refresh');
  assert.equal(d.indicator.hidden, false);
  d.move(190);
  d.paint();
  assert.equal(d.label.textContent, 'Release to refresh');
  assert.equal(d.calls.fetch, 0);
  d.end();
  assert.equal(d.calls.fetch, 1);
  await settled();
  assert.equal(d.calls.reload, 1);
  assert.equal(d.calls.fetchArgs[0], '/healthz');
  assert.equal(d.calls.fetchArgs[1].cache, 'no-store');
  assert.equal(d.timers.size, 0);
});

test('a short pull or retreat below the threshold cancels', async () => {
  for (const movements of [[140], [200, 150], [200, 80]]) {
    const d = dashboard();
    d.start();
    movements.forEach((y) => d.move(y));
    d.end();
    d.paint();
    await settled();
    assert.equal(d.calls.reload, 0);
    assert.equal(d.calls.fetch, 0);
    assert.equal(d.indicator.hidden, true);
  }
});

test('scrolling to the top during a gesture never arms it', () => {
  const d = dashboard();
  d.context.scrollY = 50;
  d.start();
  d.context.scrollY = 0;
  assert.equal(d.move(220).prevented, false);
  d.end();
  assert.equal(d.calls.fetch, 0);
});

test('controls, focused editors, nested scrollers and overlays keep their gestures', () => {
  const scenarios = [
    (d) => { d.target.closest = () => true; },
    (d) => { d.context.document.activeElement = { closest: () => true }; },
    (d) => { Object.assign(d.target, { overflowY: 'auto', scrollHeight: 500, clientHeight: 100 }); },
    (d) => { d.target.parentElement = d.element({ overflowX: 'scroll', scrollWidth: 500, clientWidth: 100 }); },
    ...['drawer', 'palette', 'faults', 'panel', 'addModel'].map((key) => (d) => { d.context.S[key] = true; }),
    (d) => { d.context.S.config.review = {}; },
    (d) => { d.modal.open = true; },
  ];
  for (const scenario of scenarios) {
    const d = dashboard();
    scenario(d);
    d.start();
    assert.equal(d.move(220).prevented, false);
    d.end();
    assert.equal(d.calls.fetch, 0);
  }
});

test('horizontal, upward, multi-touch and browser-owned gestures never reload', () => {
  const scenarios = [
    (d) => d.move(110, 200),
    (d) => d.move(50),
    (d) => d.move(200, 100, { touches: [d.touch(200), d.touch(200, 150, 2)] }),
    (d) => d.move(200, 100, { touches: [d.touch(200, 100, 2)] }),
    (d) => d.move(200, 100, { cancelable: false }),
    (d) => { d.context.scrollY = 20; d.move(200); d.context.scrollY = 0; },
  ];
  for (const scenario of scenarios) {
    const d = dashboard();
    d.start();
    scenario(d);
    d.move(240);
    d.end();
    assert.equal(d.calls.fetch, 0);
  }
});

test('cancel, navigation, rotation, backgrounding and an overlay appearing cancel an armed pull', () => {
  for (const event of ['touchcancel', 'hashchange', 'resize', 'visibilitychange', 'pagehide', 'overlay']) {
    const d = dashboard();
    d.start(); d.move(200);
    if (event === 'overlay') d.context.S.drawer = true;
    else d.dispatch(event);
    d.end(); d.paint();
    assert.equal(d.calls.fetch, 0, event);
    assert.equal(d.indicator.hidden, true, event);
  }
});

test('refresh is single-flight and disables both header actions while checking health', async () => {
  let resolve;
  const d = dashboard({ fetch: () => new Promise((done) => { resolve = done; }) });
  const pending = d.context.refreshPage();
  d.context.refreshPage();
  d.start(); d.move(200); d.end(); d.paint();
  assert.equal(d.calls.fetch, 1);
  assert.equal(d.label.textContent, 'Refreshing…');
  assert.ok(d.buttons.every((b) => b.disabled));
  resolve({ ok: true });
  await pending;
  assert.equal(d.calls.reload, 1);
});

test('offline, HTTP failure and timeout keep the dashboard and allow retry', async () => {
  for (const fetch of [() => Promise.reject(new Error('offline')), () => Promise.resolve({ ok: false }),
    (url, { signal }) => new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(new Error('timeout'))))]) {
    const d = dashboard({ fetch });
    const pending = d.context.refreshPage();
    for (const { fn, ms } of d.timers.values()) if (ms === 5000) fn();
    await pending; d.paint();
    assert.equal(d.calls.reload, 0);
    assert.match(d.calls.toasts[0], /Can’t reach Fusebox/);
    assert.equal(d.indicator.hidden, true);
    assert.ok(d.buttons.every((b) => !b.disabled));
    assert.equal(d.run('pageRefreshBusy'), false);
  }
});

test('unsaved Config and YAML require explicit discard; cancel leaves the unload guard intact', async () => {
  for (const key of ['dirty', 'rawDirty']) {
    const d = dashboard();
    d.state[key] = true;
    await d.context.refreshPage();
    assert.equal(d.modal.open, true);
    assert.equal(d.calls.focused, 1, 'Keep editing is the default');
    assert.equal(d.calls.fetch, 0);
    d.modal.close();
    assert.equal(d.calls.reload, 0);
    assert.equal(d.dispatch('beforeunload').prevented, true);
    await d.context.refreshPage();
    d.modal.close();
    await d.context.refreshPage(true);
    assert.equal(d.calls.reload, 1);
    assert.equal(d.dispatch('beforeunload').prevented, false, 'explicit consent does not produce a second prompt');
  }
});

test('edits and in-progress operations are checked again after the asynchronous probe', async () => {
  for (const editing of [true, false]) {
    let resolve;
    const d = dashboard({ fetch: () => new Promise((done) => { resolve = done; }) });
    const pending = d.context.refreshPage();
    if (editing) d.state.dirty = true;
    else d.context.S.config.busy = true;
    resolve({ ok: true });
    await pending;
    assert.equal(d.calls.reload, 0);
    if (editing) assert.equal(d.modal.open, true);
    else assert.match(d.calls.toasts[0], /current operation/);
  }
  for (const operation of ['save', 'play']) {
    const d = dashboard();
    if (operation === 'save') d.context.S.config.busy = true;
    else d.context.S.play = { busy: true };
    await d.context.refreshPage();
    assert.equal(d.calls.fetch, 0);
    assert.equal(d.calls.reload, 0);
  }
});
