const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const start = source.indexOf('const refreshing =');
const end = source.indexOf('// Activity outlives account changes', start);
assert.ok(start >= 0 && end > start);
const settle = () => new Promise(setImmediate);

function dashboard(route = 'models', ignoreAbort = false) {
  let nextTimer = 0;
  const timers = new Map(), calls = [], paints = [];
  let outstanding = 0, maximum = 0;
  const S = { route, locked: null, routesStale: false };
  const context = {
    S, AbortController,
    setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
    api(endpoint, { signal } = {}) {
      outstanding++; maximum = Math.max(maximum, outstanding);
      return new Promise((resolve, reject) => {
        let done = false;
        const call = { endpoint, signal,
          resolve(value) { if (!done) { done = true; outstanding--; resolve(value); } },
          reject() { if (!done) { done = true; outstanding--; reject(new Error('unavailable')); } },
        };
        calls.push(call);
        if (!ignoreAbort) signal?.addEventListener('abort', () => call.reject(), { once: true });
      });
    },
    refreshSetups() {}, expireActivity() {}, syncResetModal() {},
    refreshViews() { paints.push({ route: S.route, accounts: S.accounts, models: S.models }); },
  };
  vm.createContext(context);
  vm.runInContext(source.slice(start, end), context);
  return {
    S, calls, paints, timers,
    get outstanding() { return outstanding; }, get maximum() { return maximum; },
    event(kind = 'accounts') { vm.runInContext(`scheduleRefresh(${JSON.stringify(kind)})`, context); },
    fire() { for (const [id, timer] of [...timers]) if (timer.ms === 250) { timers.delete(id); timer.fn(); } },
    timeout() { for (const [id, timer] of [...timers]) if (timer.ms === 30000) { timers.delete(id); timer.fn(); } },
  };
}

test('live refreshes bound an active batch and coalesce midflight bursts', async (t) => {
  const d = dashboard();
  for (let i = 0; i < 3; i++) { d.event(); d.fire(); }
  t.diagnostic(`${d.outstanding} outstanding reads after three debounced account bursts`);
  assert.equal(d.outstanding, 3);
  assert.deepEqual(d.calls.map((c) => c.endpoint), ['/accounts', '/overview', '/routes']);
  d.calls.slice().forEach((call) => call.resolve({ version: 1 }));
  await settle();
  assert.equal(d.calls.length, 6, 'both pending bursts share one trailing batch');
  d.calls.slice(3).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.equal(d.S.accounts.version, 2);
  assert.equal(d.maximum, 3);
  assert.equal(d.outstanding, 0);
  assert.equal(d.timers.size, 0, 'the drain consumes the pending debounce');
});

test('account and model flags merge without losing changes received during a batch', async () => {
  const d = dashboard();
  d.event('models'); d.fire();
  assert.deepEqual(d.calls.map((c) => c.endpoint), ['/models', '/routes']);
  d.event('accounts'); d.event('models');
  d.calls.slice().forEach((call) => call.resolve({ version: 1 }));
  await settle();
  assert.deepEqual(d.calls.slice(2).map((c) => c.endpoint), ['/accounts', '/overview', '/models', '/routes']);
  // A third batch must also run if an event arrives after the trailing one starts.
  d.event('models'); d.fire();
  d.calls.slice(2).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.deepEqual(d.calls.slice(6).map((c) => c.endpoint), ['/models', '/routes']);
  d.calls.slice(6).forEach((call) => call.resolve({ version: 3 }));
  await settle();
  assert.equal(d.S.accounts.version, 2);
  assert.equal(d.S.models.version, 3);
  assert.equal(d.maximum, 4);
  assert.equal(d.outstanding, 0);
});

test('a failed read waits for its siblings and the pending refresh still recovers', async () => {
  const d = dashboard();
  d.event(); d.fire();
  d.event('models'); d.fire();
  d.calls[0].reject();
  await settle();
  assert.equal(d.calls.length, 3, 'failure does not overlap the next batch with pending siblings');
  d.calls[1].resolve({ version: 1 }); d.calls[2].resolve({ version: 1 });
  await settle();
  assert.equal(d.paints.length, 0, 'a partial failed snapshot is not applied');
  assert.deepEqual(d.calls.slice(3).map((c) => c.endpoint), ['/models', '/routes']);
  d.calls.slice(3).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.equal(d.S.models.version, 2);
  d.event(); d.fire();
  d.calls.slice(5).forEach((call) => call.resolve({ version: 3 }));
  await settle();
  assert.equal(d.S.accounts.version, 3, 'future events work after failure recovery');
  assert.equal(d.maximum, 3);
});

test('a trailing refresh uses the current page and only paints the current view', async () => {
  const d = dashboard('overview');
  d.event(); d.fire();
  assert.deepEqual(d.calls.map((c) => c.endpoint), ['/accounts', '/overview']);
  assert.equal(d.S.routesStale, true);
  d.S.route = 'models'; d.event('models'); d.fire();
  d.calls.slice().forEach((call) => call.resolve({ version: 1 }));
  await settle();
  assert.deepEqual(d.calls.slice(2).map((c) => c.endpoint), ['/models', '/routes']);
  d.S.route = 'config';
  d.calls.slice(2).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.deepEqual(d.paints.map((p) => p.route), ['models', 'config']);
  d.event(); d.fire();
  assert.deepEqual(d.calls.slice(4).map((c) => c.endpoint), ['/accounts', '/overview']);
  assert.equal(d.S.routesStale, true);
  d.calls.slice(4).forEach((call) => call.resolve({ version: 3 }));
  await settle();
});

test('management locking prevents applying pending data and resumes after a new event', async () => {
  const d = dashboard();
  d.event(); d.fire();
  d.S.locked = 'key';
  d.event('models'); d.fire();
  d.calls.slice().forEach((call) => call.resolve({ version: 1 }));
  await settle();
  assert.equal(d.paints.length, 0);
  assert.equal(d.calls.length, 3);
  d.S.locked = null;
  d.event(); d.fire();
  assert.deepEqual(d.calls.slice(3).map((c) => c.endpoint), ['/accounts', '/overview', '/models', '/routes']);
  d.calls.slice(3).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.equal(d.S.models.version, 2);
  assert.equal(d.S.accounts.version, 2);
});

test('the batch deadline aborts stalled siblings and pending and future events still run', async () => {
  const d = dashboard();
  d.event(); d.fire();
  assert.equal(d.calls.length, 3, 'firing the debounce does not fire the deadline');
  d.event('models'); d.fire();
  d.timeout();
  await settle();
  assert.ok(d.calls.slice(0, 3).every((call) => call.signal.aborted));
  assert.deepEqual(d.calls.slice(3).map((c) => c.endpoint), ['/models', '/routes']);
  assert.ok(d.calls.slice(3).every((call) => !call.signal.aborted));
  assert.equal(d.paints.length, 0, 'the timed-out snapshot is discarded');
  d.calls.slice(3).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.equal(d.S.models.version, 2);
  d.event(); d.fire();
  d.calls.slice(5).forEach((call) => call.resolve({ version: 3 }));
  await settle();
  assert.equal(d.S.accounts.version, 3);
  assert.equal(d.maximum, 3);
  assert.equal(d.outstanding, 0);
  assert.equal(d.timers.size, 0, 'successful reads clear their deadline');
});

test('a transport that completes after abort cannot apply its expired snapshot', async () => {
  const d = dashboard('models', true);
  d.event(); d.fire();
  d.event('models'); d.fire();
  d.timeout();
  d.calls.slice().forEach((call) => call.resolve({ version: 1 }));
  await settle();
  assert.equal(d.paints.length, 0);
  assert.equal(d.S.accounts, undefined);
  d.calls.slice(3).forEach((call) => call.resolve({ version: 2 }));
  await settle();
  assert.equal(d.S.models.version, 2);
  assert.equal(d.timers.size, 0);
});
