const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The overview's combined weekly limits, run from their own sources: each provider's
// subscriptions as one meter, with the average of where each account would be at even pace.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  return app.slice(at, end);
}

const NOW = Date.parse('2026-10-17T20:00:00Z');
const DAY = 86400000;
const HOUR = 3600000;
function load(S = { quotaDisplay: 'used', accounts: [] }) {
  const context = {
    S, Date: class extends Date { static now() { return NOW; } },
    mob: () => false, acctLogo: () => '<svg></svg>', provName: (a) => ({ claude: 'Claude', codex: 'Codex' }[a.provider]),
  };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('function quotaOf(w'),
    piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'), piece('const metered ='),
    piece('function windowSeconds(w) {'), piece('function windowElapsed(w'), piece('function meterHTML(q'),
    piece('function weeklyPools(accounts'), piece('function poolGap(p) {'), piece('const POOL_PACE ='), piece('function poolMeterHTML(p) {'),
    piece('const poolCount ='), piece('function poolGapHTML(p) {'), piece('function ovPoolHTML() {'),
  ].join('\n'), context);
  return (code) => {
    const value = vm.runInContext(code, context);
    return value !== null && typeof value === 'object' ? JSON.parse(JSON.stringify(value)) : value;
  };
}

// An account with a weekly window `used` percent through, renewing `left` ms from NOW.
const sub = (id, used, left, extra = {}) => ({
  id, provider: 'claude', kind: 'oauth', disabled: false,
  quota: { windows: [{ name: '5h', used: 90, resets_at: new Date(NOW + HOUR).toISOString() },
    ...(used == null ? [] : [{ name: 'week', used, resets_at: left == null ? null : new Date(NOW + left).toISOString() }])] },
  ...extra,
});

test('each account’s pace comes from its own reset, and use and pace are averaged', () => {
  const run = load();
  // Weeks half, a quarter and three quarters gone: even pace 50% between them.
  const accounts = [sub('a', 42, 3.5 * DAY), sub('b', 4, 5.25 * DAY), sub('c', 20, 1.75 * DAY)];
  const [pool] = run(`weeklyPools(${JSON.stringify(accounts)})`);
  assert.equal(pool.accounts, 3);
  assert.equal(pool.reported, 3);
  assert.equal(pool.used, 22);
  assert.equal(pool.pace, 0.5);
  assert.deepEqual(run(`poolGap(${JSON.stringify(pool)})`), { text: '28 points under pace', cls: '' });
});

test('renewed, idle and unreported windows, disabled accounts and single providers', () => {
  const run = load();
  const renewed = sub('a', 80, -HOUR); // reset an hour ago, not refreshed yet: a fresh week
  const idle = sub('b', 0, null); // not started: nothing used, at its start
  const unreported = sub('c', null);
  const disabled = sub('d', 100, DAY, { disabled: true });
  const single = { id: 'x', provider: 'codex', kind: 'oauth', quota: { windows: [{ name: 'week', used: 36, resets_at: new Date(NOW + DAY).toISOString() }] } };
  const pools = run(`weeklyPools(${JSON.stringify([renewed, idle, unreported, disabled, sub('e', 60, 3.5 * DAY), single])})`);
  assert.equal(pools.length, 1, 'one Codex account has no combined bar');
  assert.equal(pools[0].accounts, 4, 'the disabled account is left out');
  assert.equal(pools[0].reported, 3);
  assert.equal(pools[0].used, 20);
  assert.equal(pools[0].pace, 0.5 / 3);
  assert.equal(run(`poolCount(${JSON.stringify(pools[0])})`), '3 of 4 accounts reporting');
  assert.deepEqual(run(`weeklyPools(${JSON.stringify([sub('a', 10, DAY), disabled])})`), [], 'one enabled account left');
  assert.deepEqual(run(`weeklyPools(${JSON.stringify([sub('a', null), sub('b', null)])})`), [], 'nothing reported yet');
  // A window in use with no reset time has no pace, so neither has the pool.
  const [unpaced] = run(`weeklyPools(${JSON.stringify([sub('a', 30, null), sub('b', 10, DAY)])})`);
  assert.equal(unpaced.used, 20);
  assert.equal(unpaced.pace, null);
  assert.equal(run(`poolGap(${JSON.stringify(unpaced)})`), null);
});

test('the gap reads over pace as a warning, and close to it as on pace', () => {
  const run = load();
  assert.deepEqual(run('poolGap({ used: 41, pace: 0.32 })'), { text: '9 points over pace', cls: 'warn' });
  assert.deepEqual(run('poolGap({ used: 31.6, pace: 0.32 })'), { text: 'On pace', cls: '' });
  assert.deepEqual(run('poolGap({ used: 31, pace: 0.32 })'), { text: '1 point under pace', cls: '' });
});

test('the row shows the combined meter and pace in the chosen display', () => {
  const S = { quotaDisplay: 'used', accounts: [sub('a', 42, 3.5 * DAY), sub('b', 4, 5.25 * DAY), sub('c', 20, 1.75 * DAY)] };
  const run = load(S);
  let html = run('ovPoolHTML()');
  assert.match(html, /Combined weekly limits/);
  assert.match(html, /3 accounts/);
  assert.match(html, /<span class="pct ">22%<\/span>/);
  assert.match(html, /pace 50%/);
  assert.match(html, /class="pace" style="left:50%"/);
  assert.match(html, /aria-label="Claude combined weekly quota used"/);
  assert.match(html, /aria-valuetext="22% used · Healthy · even pace 50% used by now"/);
  assert.match(html, /28 points under pace/);
  assert.equal((html.match(/<i class="on"><\/i>/g) || []).length, 4, '22% lights four segments');
  assert.doesNotMatch(html, /data-quota-reset/, 'the pool has no single reset');
  run('S.quotaDisplay = "remaining"');
  html = run('ovPoolHTML()');
  assert.match(html, /<span class="pct ">78%<\/span>/);
  html = run('poolMeterHTML({ account: { provider: "claude" }, used: 22, pace: 0.32 })');
  assert.match(html, /pace 68%/, 'pace in the same terms as the percentage');
  assert.match(html, /class="pace" style="left:68%"/);
  run('S.accounts = S.accounts.slice(0, 1)');
  assert.equal(run('ovPoolHTML()'), '', 'one account: no section');
});
