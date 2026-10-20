const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The overview's combined weekly limits, run from their own sources. The server works the
// pools out (src/pools.rs, with their tests) for this meter and the pace warning alike; the
// dashboard draws them: one meter per provider with its pace mark, how its accounts count,
// and whether at this pace they all run out before renewals bring capacity back.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  return app.slice(at, end);
}

const NOW = Date.parse('2026-10-17T20:00:00Z');
const DAY = 86400000;
function load(S = { quotaDisplay: 'used', accounts: [], overview: { pools: [] } }) {
  const context = {
    S, Date: class extends Date { static now() { return NOW; } },
    mob: () => false, acctLogo: () => '<svg></svg>', provName: (a) => ({ claude: 'Claude', codex: 'Codex' }[a.provider]),
  };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('const cap = (s)'),
    piece('const fullResetTime ='), piece('const accountById ='),
    piece('const CODEX_PLAN_NAMES ='), piece('const planName = (a)'), piece('function quotaOf(w'),
    piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'),
    piece('function meterHTML(q'), piece('const PLAN_PRICE_NOTE ='), piece('function planCounts(list) {'),
    piece('function weeklyPools() {'), piece('function poolGap(p) {'), piece('const POOL_PACE ='), piece('function poolMeterHTML(p) {'),
    piece('function poolCount(p) {'), piece('function poolBasis(pools) {'), piece('function poolGapHTML(p) {'),
    piece('function poolEarlyHTML(p) {'), piece('function ovPoolHTML() {'),
  ].join('\n'), context);
  return (code) => {
    const value = vm.runInContext(code, context);
    return value !== null && typeof value === 'object' ? JSON.parse(JSON.stringify(value)) : value;
  };
}

// A subscription as /api/accounts lists it.
const sub = (id, plan, provider = 'claude') => ({ id, provider, kind: 'oauth', disabled: false, quota: { windows: [], ...(plan ? { plan } : {}) } });
// A pool as /api/overview lists it.
const pool = (ids, extra = {}) => ({
  provider: 'claude', window: 'week', model: null, accounts: ids.length, account_ids: ids, reported: ids.length,
  used: 22, pace: 0.5, priced: false, runs_out_early: false, until: null, ...extra,
});
const state = (accounts, pools) => ({ quotaDisplay: 'used', accounts, overview: { pools } });

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
  const accounts = ['chatgptprolite', 'chatgptpro', 'chatgptpromax'].map((sku, i) => ({ ...sub(String(i), null, 'codex'), ...account(sku) }));
  const counted = load(state(accounts, [pool(['0', '1', '2'], { provider: 'codex' })]));
  const [p] = counted('weeklyPools()');
  assert.equal(counted(`poolCount(${JSON.stringify(p)})`), '1 Pro 100, 1 Pro 200, 1 Pro 500');
});

test('the meter draws the server’s weekly pools with their accounts', () => {
  const accounts = [sub('a', 'max_20x'), sub('b', 'max_20x'), sub('c', 'pro'), sub('x', null, 'codex'), sub('y', null, 'codex')];
  const run = load(state(accounts, [
    pool(['a', 'b', 'c'], { window: '5h' }),
    pool(['a', 'b', 'c'], { used: 40, pace: 0.625, priced: true }),
    pool(['a', 'b', 'c'], { model: 'opus' }),
    pool(['x', 'y', 'gone'], { provider: 'codex', accounts: 3 }),
    pool(['gone'], { provider: 'codex' }),
  ]));
  const pools = run('weeklyPools()');
  assert.equal(pools.length, 2, 'whole-account weeks only, and only with accounts the page knows');
  assert.equal(pools[0].account.id, 'a');
  assert.deepEqual(pools[0].plans, [['Max 20x', 2], ['Pro', 1]]);
  assert.deepEqual([pools[0].used, pools[0].pace, pools[0].priced], [40, 0.625, true]);
  assert.equal(pools[1].account.id, 'x');
  assert.equal(pools[1].accounts, 3, 'the server counts the members');
  assert.deepEqual(load(state(accounts, undefined))('weeklyPools()'), [], 'an overview without pools');
});

test('the gap reads over pace as a warning, and close to it as on pace', () => {
  const run = load();
  assert.deepEqual(run('poolGap({ used: 41, pace: 0.32 })'), { text: '9 points over pace', cls: 'warn' });
  assert.deepEqual(run('poolGap({ used: 31.6, pace: 0.32 })'), { text: 'On pace', cls: '' });
  assert.deepEqual(run('poolGap({ used: 31, pace: 0.32 })'), { text: '1 point under pace', cls: '' });
  assert.deepEqual(run('poolGap({ used: 28, pace: 0.5 })'), { text: '22 points under pace', cls: '' });
  assert.equal(run('poolGap({ used: 20, pace: null })'), null, 'a window in use with no reset time has no pace');
});

test('the row shows the combined meter and pace in the chosen display', () => {
  const S = state([sub('a'), sub('b'), sub('c')], [pool(['a', 'b', 'c'])]);
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
  assert.doesNotMatch(html, /Runs out early/);
  assert.equal((html.match(/<i class="on"><\/i>/g) || []).length, 4, '22% lights four segments');
  assert.doesNotMatch(html, /data-quota-reset/, 'the pool has no single reset');
  run('S.quotaDisplay = "remaining"');
  html = run('ovPoolHTML()');
  assert.match(html, /<span class="pct ">78%<\/span>/);
  html = run('poolMeterHTML({ account: { provider: "claude" }, used: 22, pace: 0.32 })');
  assert.match(html, /pace 68%/, 'pace in the same terms as the percentage');
  assert.match(html, /class="pace" style="left:68%"/);
  run('S.overview.pools = []');
  assert.equal(run('ovPoolHTML()'), '', 'no pools: no section');
});

test('the row says when the pace warning holds for the pool', () => {
  const until = new Date(NOW + 2 * DAY).toISOString();
  const S = state([sub('a'), sub('b')], [pool(['a', 'b'], { used: 75, pace: 0.45, runs_out_early: true, until })]);
  const run = load(S);
  const html = run('ovPoolHTML()');
  assert.match(html, /<div class="c-status"><span class="pool-gap warn">30 points over pace<\/span><span class="pool-gap warn" title="At this pace these accounts run out before renewals bring capacity back\. Capacity comes back [^"]+\.">Runs out early<\/span><\/div>/);
  // A phone's row has room for one: the warning.
  run('mob = () => true');
  const phone = run('ovPoolHTML()');
  assert.match(phone, /Runs out early/);
  assert.doesNotMatch(phone, /points over pace/);
  // Even under pace, when a renewal wastes what an account didn't use.
  assert.match(run(`poolEarlyHTML(${JSON.stringify(pool(['a', 'b'], { used: 50, pace: 0.525, runs_out_early: true, until }))})`), /Runs out early/);
  assert.equal(run(`poolEarlyHTML(${JSON.stringify(pool(['a', 'b']))})`), '');
});

test('the count names the plans of the members', () => {
  const run = load();
  const count = (p) => run(`poolCount(${JSON.stringify(p)})`);
  assert.equal(count({ accounts: 2, reported: 2, plans: [['Max 20x', 1], ['Max 5x', 1]] }), '1 Max 20x, 1 Max 5x', 'the plans count the accounts');
  assert.equal(count({ accounts: 3, reported: 3, plans: [['Max 20x', 3]] }), '3 accounts · Max 20x');
  assert.equal(count({ accounts: 3, reported: 3, plans: [['Max 20x', 2]] }), '3 accounts · 2 Max 20x', 'a plan unknown');
  assert.equal(count({ accounts: 2, reported: 2, plans: [['Plus', 1], ['Pro', 1]] }), '1 Plus, 1 Pro');
  assert.equal(count({ accounts: 4, reported: 3, plans: [] }), '3 of 4 accounts reporting');
  assert.equal(count({ accounts: 3, reported: 2, plans: [['Max 20x', 1], ['Pro', 1]] }), '2 of 3 accounts reporting · 1 Max 20x, 1 Pro');
  assert.equal(count({ accounts: 3, reported: 2, plans: [['Pro', 2], ['Max 20x', 1]] }), '2 of 3 accounts reporting · 2 Pro, 1 Max 20x', 'a week missing keeps the count first');
  assert.equal(count({ accounts: 2, reported: 2 }), '2 accounts', 'no plans known');
});

test('the heading says how each provider’s accounts count', () => {
  const S = state([sub('a', 'max_20x'), sub('b', 'max_5x')], [pool(['a', 'b'], { used: 40, pace: 0.625, priced: true })]);
  const run = load(S);
  let html = run('ovPoolHTML()');
  assert.match(html, /<span class="meta" title="Each account counts by its plan’s monthly price: Claude Pro 1, Max 5x 5, Max 20x 10">Each provider’s subscriptions together, weighted by plan price<\/span>/);
  assert.match(html, /<span class="sub">1 Max 20x, 1 Max 5x<\/span>/);
  assert.match(html, /<span class="pct ">40%<\/span>/);
  run('S.accounts[1].quota.plan = undefined; S.overview.pools[0].priced = false');
  html = run('ovPoolHTML()');
  assert.match(html, /<span class="meta">Each provider’s subscriptions together, weighted equally<\/span>/);
  assert.match(html, /<span class="sub">2 accounts · 1 Max 20x<\/span>/);
  assert.equal(run('poolBasis([{ priced: true, account: { provider: "claude" } }, { priced: false, account: { provider: "codex" } }])'),
    'Claude weighted by plan price, Codex equally');
  // Plan names reach the page escaped.
  run('S.accounts[0].quota.plan = "<b>"');
  assert.match(run('ovPoolHTML()'), /2 accounts · 1 &lt;b&gt;/);
});
