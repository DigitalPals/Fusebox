const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The overview's combined weekly limits, run from their own sources: each provider's
// subscriptions as one meter, with the average of where each account would be at even pace,
// each account counting by its plan's price when every plan is known.
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
    piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('const cap = (s)'),
    piece('const CODEX_PLAN_NAMES ='), piece('const planName = (a)'), piece('function quotaOf(w'),
    piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'), piece('const metered ='),
    piece('function windowSeconds(w) {'), piece('function windowElapsed(w'), piece('function meterHTML(q'),
    piece('const PLAN_PRICE ='), piece('const PLAN_PRICE_NOTE ='), piece('const planWeight ='), piece('function planCounts(list) {'),
    piece('function weeklyPools(accounts'), piece('function poolGap(p) {'), piece('const POOL_PACE ='), piece('function poolMeterHTML(p) {'),
    piece('function poolCount(p) {'), piece('function poolBasis(pools) {'), piece('function poolGapHTML(p) {'), piece('function ovPoolHTML() {'),
  ].join('\n'), context);
  return (code) => {
    const value = vm.runInContext(code, context);
    return value !== null && typeof value === 'object' ? JSON.parse(JSON.stringify(value)) : value;
  };
}

// An account with a weekly window `used` percent through, renewing `left` ms from NOW.
const sub = (id, used, left, extra = {}, plan) => ({
  id, provider: 'claude', kind: 'oauth', disabled: false,
  quota: { windows: [{ name: '5h', used: 90, resets_at: new Date(NOW + HOUR).toISOString() },
    ...(used == null ? [] : [{ name: 'week', used, resets_at: left == null ? null : new Date(NOW + left).toISOString() }])],
  ...(plan ? { plan } : {}) },
  ...extra,
});
const on = (plan, id, used, left) => sub(id, used, left, {}, plan);

test('Codex Pro labels use the active SKU and fall back to the broad plan for unknown tiers', () => {
  const run = load();
  const account = (sku, provider = 'codex') => ({ provider, quota: { plan: 'pro', subscription_plan: sku } });
  for (const [sku, label] of [['chatgptprolite', 'Pro 100'], ['chatgptpro', 'Pro 200'], ['chatgptpromax', 'Pro 500']]) {
    assert.equal(run(`planName(${JSON.stringify(account(sku))})`), label);
  }
  for (const sku of [undefined, '', 'future-pro-plan', 'chatgptpro-20x', 'constructor', '__proto__']) {
    assert.equal(run(`planName(${JSON.stringify(account(sku))})`), 'Pro', 'unknown details do not guess a price or multiplier');
  }
  assert.equal(run(`planName(${JSON.stringify(account('chatgptpro', 'claude'))})`), 'Pro', 'Codex SKUs do not relabel Claude');
  assert.equal(run('planName({ provider: "codex" })'), '');
  const accounts = ['chatgptprolite', 'chatgptpro', 'chatgptpromax'].map((sku, i) => ({
    ...sub(String(i), 30, 3.5 * DAY), ...account(sku), kind: 'oauth',
    quota: { ...sub(String(i), 30, 3.5 * DAY).quota, ...account(sku).quota },
  }));
  const [pool] = run(`weeklyPools(${JSON.stringify(accounts)})`);
  assert.equal(run(`poolCount(${JSON.stringify(pool)})`), '1 Pro 100, 1 Pro 200, 1 Pro 500');
  assert.equal(pool.priced, false, 'the display tier does not infer quota weighting');
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

test('each account counts by its plan’s price when every plan is known', () => {
  const run = load();
  // Max 20x half through its week at 30%, Max 5x seven eighths through at 60%: by price
  // (10 and 5) the pool is at 40% with even pace at 62.5%; counted equally it would be 45% and 68.75%.
  const [pool] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 30, 3.5 * DAY), on('max_5x', 'b', 60, 0.875 * DAY)])})`);
  assert.equal(pool.priced, true);
  assert.equal(pool.used, 40);
  assert.equal(pool.pace, 0.625);
  assert.deepEqual(pool.plans, [['Max 20x', 1], ['Max 5x', 1]]);
  assert.equal(run(`poolCount(${JSON.stringify(pool)})`), '1 Max 20x, 1 Max 5x', 'the plans count the accounts');
  // Pro against Max 20x is 1 to 10.
  const [pro] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 0, 7 * DAY), on('pro', 'b', 88, 3.5 * DAY)])})`);
  assert.equal(pro.used, 8);
  // The same plan everywhere: the same answer as counting equally, and one plan name.
  const [same] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 42, 3.5 * DAY), on('max_20x', 'b', 4, 5.25 * DAY), on('max_20x', 'c', 20, 1.75 * DAY)])})`);
  assert.equal(same.used, 22);
  assert.equal(same.pace, 0.5);
  assert.equal(run(`poolCount(${JSON.stringify(same)})`), '3 accounts · Max 20x');
});

test('an unknown plan makes the pool count accounts equally', () => {
  const run = load();
  const [pool] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 30, 3.5 * DAY), on('max_20x', 'b', 50, 3.5 * DAY), sub('c', 10, 3.5 * DAY)])})`);
  assert.equal(pool.priced, false);
  assert.equal(pool.used, 30);
  assert.equal(run(`poolCount(${JSON.stringify(pool)})`), '3 accounts · 2 Max 20x');
  // So does a plan without a price: Max with no multiple, or any ChatGPT plan.
  const [max] = run(`weeklyPools(${JSON.stringify([on('max', 'a', 30, 3.5 * DAY), on('max_20x', 'b', 50, 3.5 * DAY)])})`);
  assert.equal(max.priced, false);
  assert.equal(max.used, 40);
  const codex = (id, plan, used) => ({ id, provider: 'codex', kind: 'oauth', quota: { plan, windows: [{ name: 'week', used, resets_at: new Date(NOW + DAY).toISOString() }] } });
  const [gpt] = run(`weeklyPools(${JSON.stringify([codex('x', 'pro', 10), codex('y', 'plus', 30)])})`);
  assert.equal(gpt.priced, false);
  assert.equal(gpt.used, 20);
  assert.equal(run(`poolCount(${JSON.stringify(gpt)})`), '1 Plus, 1 Pro');
  // Only accounts that report a week are weighed: an unreporting one's plan doesn't matter.
  const [reporting] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 30, 3.5 * DAY), on('pro', 'b', 90, 3.5 * DAY), sub('c', null)])})`);
  assert.equal(reporting.priced, true);
  assert.equal(run(`poolCount(${JSON.stringify(reporting)})`), '2 of 3 accounts reporting · 1 Max 20x, 1 Pro');
  // A plan known everywhere but a week missing keeps the count first.
  const [unreported] = run(`weeklyPools(${JSON.stringify([on('max_20x', 'a', 30, 3.5 * DAY), on('pro', 'b', 90, 3.5 * DAY), on('pro', 'c', null)])})`);
  assert.equal(run(`poolCount(${JSON.stringify(unreported)})`), '2 of 3 accounts reporting · 2 Pro, 1 Max 20x');
  assert.equal(run('poolCount({ accounts: 2, reported: 2 })'), '2 accounts', 'no plans known');
});

test('the heading says how each provider’s accounts count', () => {
  const S = { quotaDisplay: 'used', accounts: [on('max_20x', 'a', 30, 3.5 * DAY), on('max_5x', 'b', 60, 0.875 * DAY)] };
  const run = load(S);
  let html = run('ovPoolHTML()');
  assert.match(html, /<span class="meta" title="Each account counts by its plan’s monthly price: Claude Pro 1, Max 5x 5, Max 20x 10">Each provider’s subscriptions together, weighted by plan price<\/span>/);
  assert.match(html, /<span class="sub">1 Max 20x, 1 Max 5x<\/span>/);
  assert.match(html, /<span class="pct ">40%<\/span>/);
  run('S.accounts[1].quota.plan = undefined');
  html = run('ovPoolHTML()');
  assert.match(html, /<span class="meta">Each provider’s subscriptions together, weighted equally<\/span>/);
  assert.match(html, /<span class="sub">2 accounts · 1 Max 20x<\/span>/);
  assert.equal(run('poolBasis([{ priced: true, account: { provider: "claude" } }, { priced: false, account: { provider: "codex" } }])'),
    'Claude weighted by plan price, Codex equally');
  // Plan names reach the page escaped.
  run('S.accounts = [{ ...S.accounts[0], quota: { ...S.accounts[0].quota, plan: "<b>" } }, S.accounts[1]]');
  assert.match(run('ovPoolHTML()'), /2 accounts · 1 &lt;b&gt;/);
});
