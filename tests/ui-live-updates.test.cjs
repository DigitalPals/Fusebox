const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Live requests: their data lands at once, the page catches up once a frame.
const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const section = (start, end) => {
  const at = source.indexOf(start);
  const to = source.indexOf(end, at);
  assert.ok(at >= 0 && to > at, `${start} … ${end}`);
  return source.slice(at, to);
};
const code = [
  section('function onRequest(log) {', '// ---------------------------------------------------------------- render'),
  section('const requestFilter =', 'function pauseHTML() {'),
  section('function failuresByModel() {', 'function routeStepText('),
  section('let modelFails = new Map();', 'function patchModelFailures() {'),
].join('\n');

function dashboard(route = 'overview') {
  const calls = [];
  const frames = [];
  const S = {
    route, requests: [], fresh: new Map(), activity: {}, overview: { totals: { requests: 0 }, series: [] },
    filter: '', reqAcc: null, reqSess: null, reqChip: 'all', paused: false, pending: 0, faultList: [],
  };
  const note = (name) => (...args) => calls.push([name, ...args]);
  const context = {
    S, setTimeout() {}, requestAnimationFrame: (fn) => { frames.push(fn); return frames.length; },
    patchMainline: note('mainline'), patchFigures: note('figures'), patchBars: note('bars'), patch: note('patch'), patchChanged: note('patchChanged'),
    renderFaults: note('faults'), insertRequests: (logs) => calls.push(['insert', Array.from(logs, (l) => l.id)]), refreshRequests: note('refresh'),
    recentHTML() {}, pauseHTML() {}, detRequestsHTML() {}, drawerRequestsHTML() {},
    patchModelFailures: note('models'), loadActivity: note('activity'), $: () => null,
    isErr: (r) => r.status >= 400 && r.status !== 499, isRerouted: (r) => /^moved/.test(r.routing_reason || ''),
    accountOf: () => null, textMatches: () => true,
  };
  vm.createContext(context);
  vm.runInContext(code, context);
  let id = 0;
  return {
    S, calls, frames, context,
    request(extra = {}) { context.onRequest({ id: ++id, ts: new Date().toISOString(), status: 200, usage_completeness: 'complete', input_tokens: 1, output_tokens: 1, cache_tokens: 0, ...extra }); },
    run(expr) { return vm.runInContext(expr, context); },
  };
}

test('a burst of requests updates the data at once and the page once', () => {
  const d = dashboard();
  for (let i = 0; i < 5; i++) d.request();
  assert.equal(d.S.requests.length, 5, 'the log is current straight away');
  assert.equal(d.S.overview.totals.requests, 5);
  assert.equal(d.frames.length, 1, 'one frame is asked for');
  assert.equal(d.calls.length, 0, 'nothing is drawn before it');
  d.frames[0]();
  assert.deepEqual(d.calls.map(([name]) => name), ['mainline', 'figures', 'bars', 'patch']);
  assert.equal(d.calls[3][1], 'recent');
  assert.deepEqual(Array.from(d.S.fresh.keys()), [1, 2, 3, 4, 5], 'new rows glow once they are drawn');
  d.request();
  assert.equal(d.frames.length, 2, 'the next request asks for the next frame');
});

test('the Requests page inserts a frame of arrivals together, or redraws after a flood', () => {
  const d = dashboard('requests');
  d.request(); d.request();
  d.frames[0]();
  assert.deepEqual(d.calls, [['insert', [1, 2]]]);
  d.calls.length = 0;
  for (let i = 0; i < 60; i++) d.request();
  d.frames[1]();
  assert.deepEqual(d.calls.map(([name]) => name), ['refresh']);
  d.calls.length = 0;
  d.S.paused = true;
  d.request();
  d.frames[2]();
  assert.deepEqual(d.calls.map(([name, id]) => [name, id]), [['patchChanged', 'req-pause']]);
});

test('faults come from the server once its list arrives; until then from the log', () => {
  const d = dashboard('config');
  d.S.faultList = null;
  d.request();
  d.frames[0]();
  assert.deepEqual(d.calls.map(([name]) => name), ['faults']);
  d.calls.length = 0;
  d.S.faultList = [];
  d.request();
  d.frames[1]();
  assert.deepEqual(d.calls, []);
});

test('only the account page or drawer a request went to is touched', () => {
  const d = dashboard('accounts');
  d.S.sub = 'a';
  d.request({ account_id: 'b' });
  d.frames[0]();
  assert.deepEqual(d.calls, []);
  d.request({ account_id: 'a' });
  d.frames[1]();
  assert.deepEqual(d.calls.map(([name, id]) => [name, id]), [['patch', 'det-reqs'], ['activity', 'a']]);
});

test('chip counts come from one pass and match the filters', () => {
  const d = dashboard('requests');
  for (const [status, routing_reason] of [[200], [502], [499], [200, 'moved'], [429, 'moved']]) d.request({ status, routing_reason });
  const counts = d.run('chipCounts(visibleRequests())');
  assert.deepEqual({ ...counts }, { all: 5, errors: 2, cancelled: 1, rerouted: 2 });
  for (const chip of ['all', 'errors', 'cancelled', 'rerouted']) {
    d.S.reqChip = chip;
    assert.equal(d.run('shownRequests().length'), counts[chip], chip);
  }
});

test('the model list hears of a request only when a model starts or stops failing', () => {
  const d = dashboard('models');
  const marks = () => d.run('failureMarks(failuresByModel())');
  assert.equal(marks().size, 0);
  for (let i = 0; i < 2; i++) d.request({ model: 'm', status: 502, error: 'bad' });
  assert.equal(marks().size, 0, 'two failures are not yet a fault');
  d.request({ model: 'm', status: 502, error: 'bad' });
  assert.equal(marks().get('m'), '3\u0000502\u0000bad');
  d.request({ model: 'm', status: 200 });
  assert.equal(marks().get('m'), '3\u0000502\u0000bad', 'a success changes nothing');
});

test('an idle account refreshes when its temporary error expires', () => {
  let refreshes = 0;
  const now = Date.now();
  const account = { last_error: '529: Overloaded', last_error_until: new Date(now + 30_000).toISOString() };
  const context = {
    S: { accounts: [account] },
    Date: class extends Date { static now() { return now; } },
    document: { querySelectorAll: () => [] },
    refreshAccounts: () => { refreshes++; },
    refreshViews() {}, syncResetModal() {},
  };
  vm.createContext(context);
  vm.runInContext(section('const TIMED =', 'setInterval(() => { if (!document.hidden) tickTimers();'), context);
  context.tickTimers();
  assert.equal(refreshes, 0, 'a recent error stays visible');
  account.last_error_until = new Date(now).toISOString();
  context.tickTimers();
  assert.equal(refreshes, 1, 'expiry is checked without any countdown element or request');
  account.last_error_until = null;
  context.tickTimers();
  assert.equal(refreshes, 1, 'a permanent error does not cause polling');
});
