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
    S, Date: class extends Date { static now() { return NOW; } }, UsageTools: require('../ui/usage.js'),
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
    piece('function weeklyPools() {'), piece('function poolStatus(p) {'), piece('const POOL_OUTLOOK_NOTE ='),
    piece('function poolQuota(p) {'), piece('function poolRateText(o) {'), piece('function poolMeterHTML(p) {'),
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

test('Meta plans show the mint’s tier name when Meta sends one', () => {
  const run = load();
  const meta = (quota) => run(`planName(${JSON.stringify({ provider: 'meta', quota })})`);
  assert.equal(meta({ plan_name: 'Muse Code Max Usage' }), 'Muse Code Max Usage');
  assert.equal(meta({ windows: [] }), '', 'no tier name, no plan');
  assert.equal(meta({}), '');
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

const lit = (html) => (html.match(/<i class="on"><\/i>/g) || []).length;

test('the combined meter is an account’s weekly row: current quota, even pace and the next reset', () => {
  const S = state([sub('a'), sub('b'), sub('c')], [pool(['a', 'b', 'c'])]);
  const run = load(S);
  let html = run('ovPoolHTML()');
  assert.match(html, /<h2 class="h-sec">Combined weekly quota<\/h2><span class="meta">Each provider’s subscriptions together, weighted equally<\/span>/);
  assert.match(html, /3 accounts/);
  assert.match(html, /<div class="mrow"><span class="capsm">WK<\/span><span class="segs[^"]*" role="meter" aria-label="Claude combined weekly quota used"/);
  assert.match(html, /aria-valuetext="22% used · Healthy · even pace 50% used by now"/);
  assert.match(html, /<b class="pace" style="left:50%"/);
  assert.match(html, /<span class="pct ">22%<\/span><span class="rst" title="Next account resets [^"]+">↺ <span data-until=/);
  assert.equal(lit(html), 4);
  assert.match(html, /Expected to last/);
  // The forecast gives the status only; the bar shows no projected reserve.
  assert.doesNotMatch(html, /Forecast (low|high)|29%|71%|class="on due"|lowest reserve|MIN<\/span>|NOW<\/span>/);
  run('S.quotaDisplay = "remaining"');
  html = run('ovPoolHTML()');
  assert.match(html, /aria-label="Claude combined weekly quota remaining"/);
  assert.match(html, /<b class="pace" style="left:50%"/);
  assert.match(html, /<span class="pct ">78%<\/span>/);
  assert.equal(lit(html), 16);
  run('S.overview.pools = []');
  assert.equal(run('ovPoolHTML()'), '');
});

test('the status’s hover says what the forecast assumes', () => {
  const outlook = { state: 'on_track', min_remaining: 54.45, min_at: new Date(NOW + 41 * 3600000).toISOString(),
    next_reset_at: new Date(NOW + 41 * 3600000).toISOString(), rate_per_day: 11.5, observed_hours: 12.14 };
  const run = load(state([sub('a'), sub('b')], [pool(['a', 'b'], { used: 25.75, pace: 0.42, outlook })]));
  run('S.quotaDisplay = "remaining"');
  const html = run('ovPoolHTML()');
  assert.match(html, /<span class="pool-gap good" title="Forecast of the next 7 days from recent consumption[^"]* · Recent use is 12% of the combined quota a day, the average of the last day and the last week of readings\. The shortest account history is 12h\.">Expected to last<\/span>/);
  assert.match(html, /<span class="pct ">74%<\/span>/);
  assert.doesNotMatch(html, /54%/);
  run('S.overview.pools[0].outlook.observed_hours = 52.5');
  assert.match(run('ovPoolHTML()'), /The shortest account history is 2d 4h\./);
  run('S.overview.pools[0].outlook.observed_hours = 168');
  assert.doesNotMatch(run('ovPoolHTML()'), /shortest account history/);
  // Without a forecast there is nothing to explain.
  run('S.overview.pools[0].outlook = { state: "learning" }');
  assert.doesNotMatch(run('ovPoolHTML()'), /pool-gap[^>]*title=/);
});

test('the forecast shows the depletion and recovery times on desktop and mobile', () => {
  const o = { state: 'run_out', min_remaining: 0, min_at: new Date(NOW + DAY).toISOString(), exhausts_at: new Date(NOW + DAY).toISOString(), recovers_at: new Date(NOW + 2 * DAY).toISOString() };
  const run = load(state([sub('a'), sub('b')], [pool(['a', 'b'], { outlook: o })]));
  for (const mobile of [false, true]) {
    run(`mob = () => ${mobile}`);
    const html = run('ovPoolHTML()');
    assert.match(html, /pool-gap err" title="[^"]+">Quota gap expected/);
    assert.match(html, /Runs out 2026-10-18T20:00:00.000Z · Back 2026-10-19T20:00:00.000Z/);
    assert.match(html, /aria-valuenow="22"/);
    assert.equal(lit(html), 4);
    assert.doesNotMatch(html, /Expected to last/);
  }
});

test('learning and unavailable forecasts still show the current quota', () => {
  const run = load(state([sub('a'), sub('b')], []));
  for (const outlook of [{ state: 'learning', learning_until: new Date(NOW + 6 * 3600000).toISOString() }, null,
    { state: 'unavailable', reason: 'stale_readings' }, { state: 'unavailable', reason: 'awaiting_reset' },
    { state: 'unavailable', reason: 'missing_quota' }]) {
    run(`S.overview.pools = [${JSON.stringify(pool(['a', 'b'], { used: 64.25, outlook }))}]`);
    for (const mobile of [false, true]) {
      run(`mob = () => ${mobile}; S.quotaDisplay = 'used'`);
      let html = run('ovPoolHTML()');
      assert.match(html, /WK<\/span><span class="segs[^"]*" role="meter" aria-label="Claude combined weekly quota used"/);
      assert.match(html, /64%<\/span><span class="rst"/);
      assert.match(html, /class="pace"/);
      assert.doesNotMatch(html, /Expected to last/);
      if (outlook?.state === 'learning') assert.match(html, /Learning usage.*Estimate after/s);
      else assert.match(html, /Forecast unavailable/);
      if (outlook?.reason === 'stale_readings') assert.match(html, /Quota readings are stale/);
      if (outlook?.reason === 'awaiting_reset') assert.match(html, /Waiting for reset confirmation/);
      run('S.quotaDisplay = "remaining"');
      html = run('ovPoolHTML()');
      assert.match(html, /36%<\/span><span class="rst"/);
    }
  }
});

test('incomplete current quota does not invent a balance', () => {
  const run = load(state([sub('a'), sub('b')], []));
  for (const extra of [{ used: null }, { reported: 1 }, { accounts: 0, reported: 0 }]) {
    run(`S.overview.pools = [${JSON.stringify(pool(['a', 'b'], { outlook: { state: 'learning' }, ...extra }))}]`);
    const html = run('ovPoolHTML()');
    assert.match(html, /Learning usage/);
    assert.match(html, /role="img"/);
    assert.match(html, /<span class="pct dim" title="Quota not reported">–<\/span>/);
    assert.doesNotMatch(html, /role="meter"|<i class="on/);
  }
});

test('low reserve remains distinct from an expected quota gap', () => {
  const run = load(state([sub('a'), sub('b')], [pool(['a', 'b'], { outlook: { state: 'tight', min_remaining: 0.3 } })]));
  const html = run('ovPoolHTML()');
  assert.match(html, /pool-gap warn" title="[^"]+">Low reserve/);
  assert.doesNotMatch(html, /Quota gap expected|Expected to last|&lt;1%/);
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
  assert.match(html, /<span class="meta" title="Each account counts by its plan’s weekly allowance: Claude Pro 1, Max 5, Ultra 10">Each provider’s subscriptions together, weighted by plan allowance<\/span>/);
  assert.match(html, /<span class="sub">1 Max, 1 Ultra<\/span>/);
  assert.match(html, /<span class="pct ">40%<\/span>/);
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
