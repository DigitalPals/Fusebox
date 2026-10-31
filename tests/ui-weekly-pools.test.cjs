const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Render the server forecast without recomputing or inventing capacity in the browser.
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
    when: (t) => new Date(t).toISOString(), liveLeft: (t) => `<span data-until="${t}">3h 18m</span>`,
    mob: () => false, acctLogo: () => '<svg></svg>', provName: (a) => ({ claude: 'Claude', codex: 'Codex' }[a.provider]),
  };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('const cap = (s)'),
    piece('const fullResetTime ='), piece('const accountById ='),
    piece('const CODEX_PLAN_NAMES ='), piece('const CLAUDE_PLAN_NAMES ='), piece('const planName = (a)'), piece('function quotaOf(w'),
    piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'),
    piece('function meterHTML(q'), piece('const PLAN_WEIGHT_NOTE ='), piece('function planCounts(list) {'),
    piece('function weeklyPools() {'), piece('function poolStatus(p) {'), piece('const POOL_OUTLOOK_NOTE ='), piece('const POOL_CURRENT_NOTE ='),
    piece('function poolLearningQuota(p) {'), piece('function poolMeterHTML(p) {'),
    piece('function poolCount(p) {'), piece('function poolBasis(pools) {'), piece('function poolStatusHTML(p) {'), piece('function ovPoolHTML() {'),
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
  used: 22, pace: 0.5, priced: false, runs_out_early: false, until: null,
  outlook: { state: 'on_track', min_remaining: 29, min_at: new Date(NOW + 2 * DAY).toISOString(),
    next_reset_at: new Date(NOW + 3.3 * 3600000).toISOString(), observed_hours: 24 }, ...extra,
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

test('Claude plans show the provider’s name, and the renamed Max tiers their new names', () => {
  const run = load();
  const claude = (quota) => run(`planName(${JSON.stringify({ provider: 'claude', quota })})`);
  assert.equal(claude({ plan: 'max_20x' }), 'Ultra');
  assert.equal(claude({ plan: 'max_5x' }), 'Max');
  assert.equal(claude({ plan: 'max_20x', plan_name: 'Ultra Plus' }), 'Ultra Plus', 'the profile’s own name wins');
  assert.equal(claude({ plan: 'pro' }), 'Pro');
  assert.equal(claude({ plan: 'max' }), 'Max', 'Max without a multiple stays as written');
  assert.equal(run(`planName(${JSON.stringify({ provider: 'codex', quota: { plan: 'max_20x' } })})`), 'Max 20x', 'only Claude is renamed');
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
  assert.deepEqual(pools[0].plans, [['Ultra', 2], ['Pro', 1]]);
  assert.deepEqual([pools[0].used, pools[0].pace, pools[0].priced], [40, 0.625, true]);
  assert.equal(pools[1].account.id, 'x');
  assert.equal(pools[1].accounts, 3, 'the server counts the members');
  assert.deepEqual(load(state(accounts, undefined))('weeklyPools()'), [], 'an overview without pools');
});

test('the forecast meter always shows the minimum remaining reserve, independently of the quota toggle', () => {
  const S = state([sub('a'), sub('b'), sub('c')], [pool(['a', 'b', 'c'])]);
  const run = load(S);
  const html = run('ovPoolHTML()');
  assert.match(html, /Weekly quota outlook/);
  assert.match(html, /Next 7 days · lowest projected reserve/);
  assert.match(html, /3 accounts/);
  assert.match(html, /29%/);
  assert.match(html, /role="meter"[^>]+aria-label="Claude lowest projected weekly reserve"/);
  assert.match(html, /aria-valuetext="29% lowest projected reserve over the next 7 days"/);
  assert.match(html, /Expected to last/);
  assert.match(html, /Next reset in <span data-until=/);
  assert.doesNotMatch(html, /class="pace"|points under pace|quota used|78%/);
  assert.equal((html.match(/<i class="on"><\/i>/g) || []).length, 6);
  run('S.quotaDisplay = "remaining"');
  assert.equal(run('ovPoolHTML()'), html);
  run('S.overview.pools = []');
  assert.equal(run('ovPoolHTML()'), '');
});

test('the forecast shows the depletion and recovery times on desktop and mobile', () => {
  const o = { state: 'run_out', min_remaining: 0, exhausts_at: new Date(NOW + DAY).toISOString(), recovers_at: new Date(NOW + 2 * DAY).toISOString() };
  const run = load(state([sub('a'), sub('b')], [pool(['a', 'b'], { outlook: o })]));
  for (const mobile of [false, true]) {
    run(`mob = () => ${mobile}`);
    const html = run('ovPoolHTML()');
    assert.match(html, /pool-gap err">Quota gap expected/);
    assert.match(html, /Runs out 2026-10-18T20:00:00.000Z · Back 2026-10-19T20:00:00.000Z/);
    assert.match(html, /aria-valuenow="0"/);
    assert.doesNotMatch(html, /Expected to last|Healthy/);
  }
});

test('learning shows actual quota in the selected mode and automatically transitions to a forecast', () => {
  const S = state([sub('a'), sub('b')], [pool(['a', 'b'], { used: 64.25,
    outlook: { state: 'learning', learning_until: new Date(NOW + 6 * 3600000).toISOString() } })]);
  const run = load(S);
  for (const mobile of [false, true]) {
    run(`mob = () => ${mobile}; S.quotaDisplay = 'used'`);
    let html = run('ovPoolHTML()');
    assert.match(html, /Current quota while learning/);
    assert.match(html, /NOW<\/span>/);
    assert.match(html, /aria-label="Claude current combined weekly quota used"/);
    assert.match(html, /64%<\/span><span class="rst">used now/);
    assert.match(html, /Learning usage.*Estimate after/s);
    assert.doesNotMatch(html, /lowest reserve|Expected to last|class="pace"/);
    run('S.quotaDisplay = "remaining"');
    html = run('ovPoolHTML()');
    assert.match(html, /36%<\/span><span class="rst">remaining now/);
    assert.match(html, /aria-label="Claude current combined weekly quota remaining"/);
  }
  run(`S.overview.pools[0].outlook = ${JSON.stringify(pool(['a', 'b']).outlook)}`);
  const ready = run('ovPoolHTML()');
  assert.match(ready, /29%<\/span><span class="rst">lowest reserve/);
  assert.match(ready, /Expected to last/);
  assert.doesNotMatch(ready, /NOW<\/span>|remaining now|Learning usage/);
});

test('learning with incomplete current quota does not invent a balance', () => {
  const run = load(state([sub('a'), sub('b')], []));
  for (const extra of [{ used: null }, { reported: 1 }, { accounts: 0, reported: 0 }]) {
    run(`S.overview.pools = [${JSON.stringify(pool(['a', 'b'], { outlook: { state: 'learning' }, ...extra }))}]`);
    const html = run('ovPoolHTML()');
    assert.match(html, /Learning usage/);
    assert.doesNotMatch(html, /role="meter"|used now|remaining now/);
  }
});

test('missing and stale data never look like full or healthy reserves', () => {
  const run = load(state([sub('a'), sub('b')], []));
  for (const outlook of [null,
    { state: 'unavailable', reason: 'stale_readings' }, { state: 'unavailable', reason: 'awaiting_reset' },
    { state: 'unavailable', reason: 'missing_quota' }]) {
    run(`S.overview.pools = [${JSON.stringify(pool(['a', 'b'], { outlook }))}]`);
    const html = run('ovPoolHTML()');
    assert.match(html, /role="img"/);
    assert.doesNotMatch(html, /role="meter"|aria-valuenow|Expected to last|Healthy|<i class="on">/);
    if (outlook?.state === 'learning') assert.match(html, /Learning usage.*Estimate after/s);
    if (outlook?.reason === 'stale_readings') assert.match(html, /Quota readings are stale/);
    if (outlook?.reason === 'awaiting_reset') assert.match(html, /Waiting for reset confirmation/);
  }
});

test('low reserve and fractional percentages remain distinct from an expected quota gap', () => {
  const run = load(state([sub('a'), sub('b')], [pool(['a', 'b'], { outlook: { state: 'tight', min_remaining: 0.3 } })]));
  const html = run('ovPoolHTML()');
  assert.match(html, /pool-gap warn">Low reserve/);
  assert.match(html, /&lt;1%/);
  assert.match(html, /aria-valuenow="0.3"/);
  assert.doesNotMatch(html, /Quota gap expected|Expected to last/);
});

test('the count names the plans of the members', () => {
  const run = load();
  const count = (p) => run(`poolCount(${JSON.stringify(p)})`);
  assert.equal(count({ accounts: 2, reported: 2, plans: [['Ultra', 1], ['Max', 1]] }), '1 Ultra, 1 Max', 'the plans count the accounts');
  assert.equal(count({ accounts: 3, reported: 3, plans: [['Ultra', 3]] }), '3 accounts · Ultra');
  assert.equal(count({ accounts: 3, reported: 3, plans: [['Ultra', 2]] }), '3 accounts · 2 Ultra', 'a plan unknown');
  assert.equal(count({ accounts: 2, reported: 2, plans: [['Plus', 1], ['Pro', 1]] }), '1 Plus, 1 Pro');
  assert.equal(count({ accounts: 4, reported: 3, plans: [] }), '3 of 4 accounts reporting');
  assert.equal(count({ accounts: 3, reported: 2, plans: [['Ultra', 1], ['Pro', 1]] }), '2 of 3 accounts reporting · 1 Ultra, 1 Pro');
  assert.equal(count({ accounts: 3, reported: 2, plans: [['Pro', 2], ['Ultra', 1]] }), '2 of 3 accounts reporting · 2 Pro, 1 Ultra', 'a week missing keeps the count first');
  assert.equal(count({ accounts: 2, reported: 2 }), '2 accounts', 'no plans known');
});

test('the heading says how each provider’s accounts count', () => {
  const S = state([sub('a', 'max_20x'), sub('b', 'max_5x')], [pool(['a', 'b'], { used: 40, pace: 0.625, priced: true })]);
  const run = load(S);
  let html = run('ovPoolHTML()');
  assert.match(html, /title="[^"]*Each provider’s subscriptions together, weighted by plan allowance[^"]*Each account counts by its plan’s weekly allowance: Claude Pro 1, Max 5, Ultra 10/);
  assert.match(html, /<span class="sub">1 Max, 1 Ultra<\/span>/);
  assert.match(html, /<span class="pct good">29%<\/span>/);
  run('S.accounts[1].quota.plan = undefined; S.overview.pools[0].priced = false');
  html = run('ovPoolHTML()');
  assert.match(html, /Each provider’s subscriptions together, weighted equally/);
  assert.match(html, /<span class="sub">2 accounts · 1 Ultra<\/span>/);
  assert.equal(run('poolBasis([{ priced: true, account: { provider: "claude" } }, { priced: false, account: { provider: "codex" } }])'),
    'Claude weighted by plan allowance, Codex equally');
  // Plan names reach the page escaped.
  run('S.accounts[0].quota.plan = "<b>"');
  assert.match(run('ovPoolHTML()'), /2 accounts · 1 &lt;b&gt;/);
});
