const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');
const { execFileSync } = require('node:child_process');
const helpers = require('../ui/usage.js');
const source = fs.readFileSync(path.join(__dirname, '../ui/usage.js'), 'utf8');

function dashboard(extra = {}) {
  const context = {
    Intl, Date, URLSearchParams, AbortController, setTimeout() {},
    S: { private: false, key: 'management-test-secret', route: 'none', accounts: [] },
    metered: () => false, quotaControlsHTML: () => '', statusHTML: () => '', planName: () => '', authName: () => 'OAuth',
    acctLogo: () => '', metersHTML: () => '', provName: (a) => ({ claude: 'Claude', codex: 'Codex' })[a.provider] || a.provider,
    logo: () => '', accountById: () => null, acctLabel: (a) => a.label,
    ...extra,
  };
  vm.createContext(context);
  vm.runInContext(source, context);
  return {
    run(code) { return vm.runInContext(code, context); }, context,
    seed(data) { context.seed = data; vm.runInContext('Object.assign(Usage, seed)', context); },
  };
}

function pendingApi(ignoreAbort = false) {
  const calls = [];
  return { calls, api: (endpoint, { signal }) => new Promise((resolve, reject) => {
    calls.push({ endpoint, signal, resolve, reject });
    if (!ignoreAbort) signal.addEventListener('abort', () => reject(Object.assign(new Error('Read cancelled'), { name: 'AbortError' })), { once: true });
  }) };
}

test('presets use local calendar dates and inclusive today across a year boundary', () => {
  const r = helpers.range(7, new Date(2027, 0, 2, 12));
  assert.deepEqual(r, { start: '2026-12-27', end: '2027-01-03', through: '2027-01-02' });
  assert.deepEqual(helpers.range(1, new Date(2026, 9, 7, 23, 59)), { start: '2026-10-07', end: '2026-10-08', through: '2026-10-07' });
});

test('multiple roots, collector progress and labels stay clear and private in the Collection drawer', () => {
  const d = dashboard();
  d.seed({ loaded: true, drawerOpen: true, summary: { combined: { totals: {}, trend: [], breakdowns: { client: [{ id: 'collector:private-id', observations: 1 }] } }, facets: { clients: [{ id: 'collector:private-id', label: 'Private <laptop>' }] } }, status: { health: { historical_gap: { dropped: 0 } }, imports: [{ source: 'codex', root: '/private/root1', enabled: true }, { source: 'codex', root: '/private/root2', enabled: false }], collectors: [{ id: 'private-id', label: 'Private <laptop>', progress: [{ source: 'codex', state: 'attention', imported: 2, failed: 1 }] }] } });
  let html = d.run('usageHTML()');
  assert.ok(html.includes('/private/root1') && html.includes('/private/root2'));
  assert.ok(html.includes('Private &lt;laptop&gt;') && !html.includes('Private <laptop>'));
  assert.ok(html.includes('total historical coverage unknown'));
  assert.ok(html.includes('keeps the time it happened, not the time it was imported'));
  assert.ok(!html.includes('Earlier writer sessions recorded'));
  d.run('Usage.dim = "client"');
  assert.ok(d.run('usageHTML()').includes('Private &lt;laptop&gt;'));
  d.run('S.private = true'); html = d.run('usageHTML()');
  for (const secret of ['private-id', 'Private &lt;laptop&gt;', '/private/root1', '/private/root2']) assert.ok(!html.includes(secret), secret);
  assert.equal(d.run('usageTime(0)'), 'Not reported');
});

test('DST calendar shifts work in Amsterdam and Los Angeles', () => {
  const script = `const h=require(${JSON.stringify(path.join(__dirname, '../ui/usage.js'))});process.stdout.write(JSON.stringify([h.range(7,new Date(2026,2,29,12)),h.shift('2026-10-25',1)]))`;
  for (const timezone of ['Europe/Amsterdam', 'America/Los_Angeles']) {
    const result = JSON.parse(execFileSync(process.execPath, ['-e', script], { env: { ...process.env, TZ: timezone }, encoding: 'utf8' }));
    assert.deepEqual(result, [{ start: '2026-03-23', end: '2026-03-30', through: '2026-03-29' }, '2026-10-26']);
  }
});

test('custom dates reject invalid days and reversed boundaries', () => {
  assert.equal(helpers.shift('2028-02-29', 1), '2028-03-01');
  assert.throws(() => helpers.shift('2026-02-29', 0), /valid date/);
  assert.throws(() => helpers.shift('2026-04-31', 0), /valid date/);
  assert.throws(() => helpers.query({ start: '2026-10-08', end: '2026-10-08' }, {}, 'UTC'), /end date/);
});

test('filters are allowlisted, encoded, and use exclusive date endpoints', () => {
  const result = new URLSearchParams(helpers.query({ start: '2026-10-01', end: '2026-10-08' }, { provider: 'anthropic', model: 'model/a & b', account: 'account-1', client: 'collector-2', source: 'codex', key: 'should-not-be-sent' }, 'Europe/Amsterdam'));
  assert.equal(result.get('end'), '2026-10-08');
  assert.equal(result.get('timezone'), 'Europe/Amsterdam');
  assert.equal(result.get('model'), 'model/a & b');
  assert.equal(result.get('client'), 'collector-2');
  assert.equal(result.has('key'), false);
  assert.equal(result.has('source'), false);
});

test('unknown pricing remains unpriced and small backend estimates retain precision', () => {
  assert.equal(helpers.dollars(null), 'Unpriced');
  assert.equal(helpers.dollars(undefined), 'Unpriced');
  assert.equal(helpers.dollars(0), '$0.00');
  assert.equal(helpers.dollars(12345000), '$0.0123');
});

test('coverage distinguishes unavailable usage from missing rates without inventing free calls', () => {
  const d = dashboard();
  assert.equal(d.run('usageCoverage({ observations: 100, unpriced: 8, unpriced_reasons: { missing_usage: 8 } })[0]'), '8 missing usage');
  assert.equal(d.run('usageCoverage({ observations: 5, unpriced: 5, unpriced_reasons: { unknown_model: 5 } })[0]'), '5 no model rate');
  assert.equal(d.run('usageCoverage({ observations: 10, unpriced: 3, unpriced_reasons: { missing_usage: 2, unknown_model: 1 } })[0]'), '3 unpriced');
  d.seed({ summary: { combined: { totals: { observations: 100, unpriced: 8, unpriced_reasons: { missing_usage: 7, unknown_model: 1 } } } } });
  const line = d.run('usageCoverageLine(Usage.summary.combined.totals)');
  assert.match(line, /92\.0% of calls priced/);
  assert.match(line, /7 missing usage · 1 without a model rate/);
  assert.doesNotMatch(line, /8 unpriced/);
  const notice = d.run('usageNoticesHTML()');
  assert.match(notice, /7 calls have missing or incomplete token usage/);
  assert.match(notice, /costs remain unknown and are excluded/);
  assert.match(notice, /Adding model prices cannot recover missing usage/);
  d.seed({ summary: { combined: { totals: { observations: 3, unpriced: 0, missing_usage: 3, unpriced_reasons: {} } } } });
  assert.doesNotMatch(d.run('usageNoticesHTML()'), /costs remain unknown/);
});

test('usage formatting reuses formatters while retaining unknown, zero and currency precision', () => {
  let constructed = 0;
  const d = dashboard({ Intl: { DateTimeFormat: Intl.DateTimeFormat, NumberFormat: class extends Intl.NumberFormat {
    constructor(...args) { super(...args); constructed++; }
  } } });
  const initial = constructed;
  for (let i = 0; i < 100; i++) {
    assert.equal(d.run('usageCount(1234.5)'), '1,234.5');
    assert.equal(d.run('UsageTools.dollars(12345000)'), '$0.0123');
    assert.equal(d.run('UsageTools.dollars(1200000000)'), '$1.20');
  }
  assert.equal(constructed, initial, 'live renders do not allocate more number formatters');
  assert.equal(d.run('usageCount(null)'), 'Unknown');
  assert.equal(d.run('usageCount(0)'), '0');
  assert.equal(d.run('UsageTools.dollars(null)'), 'Unpriced');
  assert.equal(d.run('UsageTools.dollars(0)'), '$0.00');
});

test('usage dates retain native locale, timezone, DST and invalid-date behavior', () => {
  const script = `
    const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
    const context = { Intl, Date, URLSearchParams, S: {} };
    vm.createContext(context);
    vm.runInContext(fs.readFileSync(${JSON.stringify(path.join(__dirname, '../ui/usage.js'))}, 'utf8'), context);
    const run = (source) => vm.runInContext(source, context);
    for (const zone of ['UTC', 'Europe/Amsterdam', 'America/Los_Angeles', 'Asia/Kathmandu']) {
      run('Usage.timezone = ' + JSON.stringify(zone));
      for (const value of ['2026-03-29T00:30:00Z', '2026-03-29T01:30:00Z', '2026-10-25T00:30:00Z', '2026-10-25T01:30:00Z', '2026-10-29T23:59:59Z', -1, 1]) {
        const at = new Date(value), literal = JSON.stringify(value);
        assert.equal(run('usageTime(' + literal + ')'), at.toLocaleString(undefined, { timeZone: zone, hourCycle: 'h23' }));
        assert.equal(run('usageClock(' + literal + ')'), at.toLocaleTimeString('en-GB', { timeZone: zone, hour12: false }));
        assert.equal(run('UsageTools.localDate(' + literal + ', Usage.timezone)'), new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(at));
        assert.equal(run('UsageTools.timeText(' + literal + ', Usage.timezone, "weekday")'), at.toLocaleString('en-GB', { timeZone: zone, weekday: 'short', hour: '2-digit', minute: '2-digit', hour12: false }));
      }
    }
    for (const literal of ['null', 'undefined', '0']) {
      assert.equal(run('usageTime(' + literal + ')'), 'Not reported');
      assert.equal(run('usageClock(' + literal + ')'), 'Not reported');
    }
    for (const literal of ['"bad date"', 'NaN', 'Infinity', '8640000000000001']) {
      assert.equal(run('usageTime(' + literal + ')'), 'Not reported');
      assert.equal(run('usageClock(' + literal + ')'), 'Invalid Date');
    }
    run('Usage.timezone = "not/a-timezone"');
    assert.equal(run('usageTime("bad date")'), 'Not reported');
    assert.throws(() => run('usageTime(1)'), /time zone/i);
  `;
  for (const locale of ['en_US.UTF-8', 'en_GB.UTF-8', 'nl_NL.UTF-8', 'zh_CN.UTF-8']) {
    execFileSync(process.execPath, ['-e', script], { env: { ...process.env, LANG: locale, LC_ALL: locale, TZ: 'UTC' }, encoding: 'utf8' });
  }
});

test('usage timestamps and chart history labels reuse formatters and update after a timezone change', () => {
  let constructed = 0;
  const d = dashboard({ Intl: { NumberFormat: Intl.NumberFormat, DateTimeFormat: function (...args) {
    constructed++;
    return new Intl.DateTimeFormat(...args);
  } } });
  const exercise = () => d.run(`[
    usageTime(1793314800000), usageClock(1793314800000),
    UsageTools.localDate(1793314800000, Usage.timezone),
    UsageTools.beforeProxy('2026-10-28', 1793314800000, Usage.timezone),
    UsageTools.timeText(1793314800000, Usage.timezone, 'weekday')
  ]`);
  d.seed({ timezone: 'UTC' });
  const utc = exercise(), warm = constructed;
  for (let i = 0; i < 100; i++) exercise();
  assert.equal(constructed, warm, 'no new formatters during warm renders');
  d.seed({ timezone: 'Asia/Tokyo' });
  const tokyo = exercise();
  assert.notEqual(tokyo[2], utc[2], 'the calendar date moves with the timezone');
  assert.equal(constructed, warm + 4, 'each style replaces its cached timezone');
  d.seed({ timezone: 'UTC' });
  assert.deepEqual(exercise(), utc);
  assert.equal(constructed, warm + 8, 'the cache retains only one timezone per style');
});

test('privacy hides all labels, IDs, paths, credentials and draft values in rendered markup', () => {
  const d = dashboard();
  d.context.S.private = true;
  d.seed({
    loaded: true, drawerOpen: true, dim: 'account', collectorDraft: 'private draft laptop', roots: { codex: '/private/draft/root' }, credential: 'fbxc_one-time-private-secret', credentialFor: 'private-collector-label',
    summary: { combined: { totals: {}, trend: [], breakdowns: { account: [{ id: 'private-account-id', observations: 1 }], client: [{ id: 'private-client-id', observations: 1, tokens: {}, models: [{ id: 'model-a', observations: 1 }] }, { id: 'collector:private-collector-id', observations: 1, tokens: {}, models: [] }] } }, facets: { accounts: [{ id: 'private-account-id', label: 'private-account-label' }], clients: [{ id: 'private-client-id', label: 'private-client-label' }, { id: 'collector:private-collector-id', label: 'private-collector-label' }] } },
    filters: { provider: '', model: '', account: 'private-account-id', client: 'private-client-id' },
    status: { health: { message: '/private/health/error' }, imports: [{ source: 'codex', root: '/private/saved/root', last_error: '/private/import/error' }], collectors: [{ id: 'private-collector-id', label: 'private-collector-label' }] },
  });
  const html = d.run('usageHTML()');
  for (const sensitive of ['private-account', 'private-client', 'private-collector', '/private/', 'private draft laptop', 'fbxc_one-time-private-secret']) assert.equal(html.includes(sensitive), false, sensitive);
  assert.match(html, /••••••/);
  assert.match(html, /data-usage-act="filter-row" data-index="0"/);
  d.context.S.private = false;
  const visible = d.run('usageHTML()');
  assert.match(visible, /Top clients[\s\S]*private-client-label/);
  assert.match(visible, /private-collector-label/);
  assert.match(visible, /fbxc_one-time-private-secret/);
});

test('top clients name each client with its share, cost, tokens and models, escaped', () => {
  const d = dashboard();
  const models = ['<script>alert("x")</script>', 'gpt-6.1-sol', 'claude-opus-5-5', 'claude-haiku-5-5'].map((id, i) => ({ id, observations: 4 - i, estimated_cost_nanos: (4 - i) * 3e9 }));
  const client = (id, cost, extra = {}) => ({ id, observations: 10, estimated_cost_nanos: cost, tokens: { input: 1000, output: 200, cache_read: 9000, cache_write: 300, reasoning: 50 }, models: [], ...extra });
  d.seed({ summary: {
    combined: { totals: { estimated_cost_nanos: 40e9 }, breakdowns: { client: [client('laptop', 30e9, { models }), client('agent', 10e9), client(null, null)] } },
    facets: { clients: [{ id: 'laptop', label: '<img src=x onerror=evil()>' }, { id: 'agent', label: 'The agent' }] },
  } });
  const html = d.run('usageClientsHTML()');
  assert.match(html, /Top clients/);
  assert.equal(html.includes('<script>'), false);
  assert.equal(html.includes('<img src=x'), false);
  assert.match(html, /&lt;img src=x onerror=evil\(\)&gt;/);
  assert.match(html, /CLIENT<\/span><span>SHARE OF COST<\/span><span class="r">EST. COST<\/span><span class="r">TOKENS<\/span><span>MODELS/);
  // 75% of the cost, $30.00, and 10.5k tokens: reasoning is part of output.
  assert.match(html, /The agent[\s\S]*25%/);
  assert.match(html, /75%<\/span><\/span><span class="r u-bd-cost">\$30\.00<\/span><span class="r mono fg2" title="1,000 input, 200 output, 9,000 read from the cache, 300 written to the cache">10\.5k</);
  assert.match(html, /title="\$12\.00 · 4 calls · 40% of this client">&lt;script&gt;/);
  assert.match(html, /claude-opus-5-5<\/span><span class="idchip static" title="claude-haiku-5-5">\+1</);
  assert.match(html, /No client reported[\s\S]*u-bd-cost dim">Unpriced/);
  assert.match(html, /No model reported/);
  for (const gone of ['account', 'window', 'Renew']) assert.equal(html.includes(gone), false, gone);
});

test('top clients show eight at first and can expand', () => {
  const d = dashboard();
  const rows = Array.from({ length: 9 }, (_, i) => ({ id: `client-${i}`, observations: 1, estimated_cost_nanos: 1e9, tokens: {}, models: [] }));
  d.seed({ summary: { combined: { totals: { estimated_cost_nanos: 9e9 }, breakdowns: { client: rows } } } });
  let html = d.run('usageClientsHTML()');
  assert.match(html, /Show all 9/);
  assert.equal(html.includes('client-8'), false);
  d.run('Usage.clientsAll = true');
  html = d.run('usageClientsHTML()');
  assert.match(html, /Show top 8/);
  assert.match(html, /client-8/);
  d.seed({ summary: { combined: { totals: {}, breakdowns: { client: [] } } } });
  assert.match(d.run('usageClientsHTML()'), /No records match these filters\./);
});

test('the combined total is the headline and coverage leaves out zero parts', () => {
  const d = dashboard();
  d.seed({ preset: '7', range: { start: '2026-10-01', end: '2026-10-08', through: '2026-10-07' }, summary: { pricing: { version: 'cat-1' }, combined: { totals: { observations: 1000, estimated_cost_nanos: 7000000000, unpriced: 8, matched: 870, weak_identity: 12, history_only: 300, partial: 0, tokens: { input: 1500, output: 8, cache_read: null, cache_write: null }, missing_token_counts: {} }, trend: [] } } });
  const html = d.run('usageCostHTML()');
  assert.match(html, /\$7\.00/);
  assert.match(html, /LAST 7 DAYS/);
  assert.match(html, /Avg \$1\.00 a day · priced calls only/);
  assert.match(html, /99\.2% of calls priced · 870 matched across sources, counted once · 12 history records without a stable ID · 8 unpriced/);
  assert.equal(html.includes('partial ·'), false);
  assert.match(html, /From history only/);
  assert.match(html, /Unknown<\/dd><dt>Cache write</);
  assert.match(html, /catalogue cat-1/);
  assert.equal(html.includes('cross-source'), false);
});

test('chart keeps the top five groups and merges the rest into Other', () => {
  const days = ['2026-10-06', '2026-10-07'];
  const trend = ['a', 'b', 'c', 'd', 'e', 'f', 'g'].flatMap((g, i) => days.map((date) => ({ date, group: g, observations: 10 - i, estimated_cost_nanos: (10 - i) * 1e9 })));
  const c = helpers.chart(trend, days, 'cost');
  assert.deepEqual(c.keys.slice(0, 5), ['a', 'b', 'c', 'd', 'e']);
  assert.equal(c.keys[5], helpers.OTHER);
  assert.equal(c.colors[0], 'oklch(0.74 0.09 30)');
  assert.equal(c.colors[5], '#57534b');
  assert.equal(c.series[0].parts[5], 9); // f (5) + g (4)
  const six = helpers.chart(trend.filter((r) => r.group !== 'g'), days, 'calls');
  assert.deepEqual(six.keys, ['a', 'b', 'c', 'd', 'e', 'f']);
  assert.equal(six.colors[5], 'oklch(0.74 0.09 330)');
});

test('readout shows the hovered day with each group, and history-only days before the proxy', () => {
  const d = dashboard();
  const trend = [{ date: '2026-10-05', group: 'anthropic', observations: 4, estimated_cost_nanos: 3e9, unpriced: 1 }, { date: '2026-10-06', group: 'openai', observations: 9, estimated_cost_nanos: 9e9 }];
  d.seed({ range: { start: '2026-10-05', end: '2026-10-07', through: '2026-10-06' }, timezone: 'Europe/Amsterdam', summary: { combined: { trend, proxy_first_event_at_ms: Date.UTC(2026, 9, 5, 23, 30), totals: {} } } });
  let html = d.run('usageChartHTML()');
  assert.match(html, /Peak day · Tue Oct 6/);
  assert.equal(html.includes('Before the proxy was set up'), false);
  d.run('Usage.hoverDay = "2026-10-05"');
  html = d.run('usageChartHTML()');
  assert.match(html, /Mon Oct 5/);
  assert.match(html, /4 calls/);
  assert.match(html, /Claude<\/span> <span class="mono">\$3\.00/);
  assert.match(html, /3 priced · 1 unpriced/);
  assert.match(html, /1 unpriced/);
  assert.match(html, /Before the proxy was set up · history only/);
  assert.equal(helpers.beforeProxy('2026-10-06', Date.UTC(2026, 9, 5, 23, 30), 'Europe/Amsterdam'), false);
  assert.equal(helpers.beforeProxy('2026-10-05', null, 'UTC'), false);
});

test('chart aggregation is reused on hover and invalidates for data, range, metric, stack and theme', () => {
  const d = dashboard();
  const trend = [{ date: '2026-10-05', group: 'anthropic', observations: 4, estimated_cost_nanos: 3e9 }];
  d.seed({ range: { start: '2026-10-05', end: '2026-10-07', through: '2026-10-06' }, summary: { combined: { trend } } });
  d.run('let aggregations = 0; const originalChart = UsageTools.chart; UsageTools.chart = (...args) => { aggregations++; return originalChart(...args); }; usageChartHTML()');
  d.run('Usage.hoverDay = "2026-10-05"; usageChartHTML(); usageChartHTML()');
  assert.equal(d.run('aggregations'), 1);
  d.run('Usage.metric = "calls"');
  assert.match(d.run('usageChartHTML()'), /4 calls/);
  assert.equal(d.run('usageChartData().max'), 4);
  d.run('Usage.range.end = "2026-10-06"; usageChartHTML()');
  assert.equal(d.run('usageChartData().series.length'), 1);
  d.run('Usage.summary.combined.trend = [{date: "2026-10-05", group: "anthropic", observations: 9}]; usageChartHTML()');
  assert.equal(d.run('usageChartData().max'), 9);
  d.run('Usage.summary.combined.trends = {model: [{date: "2026-10-05", group: "model-a", observations: 2}]}; Usage.stack = "model"; usageSelectStack(Usage.summary); usageChartHTML()');
  assert.equal(d.run('usageChartData().keys[0]'), 'model-a');
  // Introduce only the document properties chart generation needs, after setup.
  d.context.document = { documentElement: { dataset: { shade: 'light' } } };
  d.run('usageChartHTML()');
  assert.equal(d.run('usageChartData().colors[0]'), 'oklch(0.6 0.09 30)');
  assert.equal(d.run('aggregations'), 6);
});

test('empty charts reuse their aggregate and labels remain fresh without re-aggregation', () => {
  const d = dashboard();
  d.seed({ range: { start: '2026-10-05', end: '2026-10-07', through: '2026-10-06' }, summary: { combined: {} } });
  d.run('let aggregations = 0; const originalChart = UsageTools.chart; UsageTools.chart = (...args) => { aggregations++; return originalChart(...args); }; usageChartHTML(); usageChartHTML()');
  assert.equal(d.run('aggregations'), 1);
  assert.equal(d.run('usageChartData().max'), 0);
  assert.equal(d.run('usageChartData().series.length'), 2);
  d.run('Usage.summary.combined.trend = [{date: "2026-10-05", group: "anthropic", observations: 1}]');
  assert.match(d.run('usageChartHTML()'), /Claude/);
  d.context.provName = () => 'Updated provider';
  assert.match(d.run('usageChartHTML()'), /Updated provider/);
  assert.equal(d.run('aggregations'), 2, 'labels are generated afresh without changing cached numeric data');
});

test('hover retains chart controls and focused bars, changing only selected classes and readout', () => {
  const writes = [], classWrites = [];
  const bars = ['2026-10-05', '2026-10-06', '2026-10-07'].map((date) => {
    let cls = 'u-bar';
    return { dataset: { usageDay: date }, get className() { return cls; }, set className(value) { cls = value; classWrites.push(date); } };
  });
  let readoutHTML = '';
  const readout = { get innerHTML() { return readoutHTML; }, set innerHTML(value) { readoutHTML = value; writes.push('readout'); } };
  const chart = { querySelector: () => readout, querySelectorAll: () => bars, set innerHTML(_) { assert.fail('hover must preserve the chart DOM'); } };
  const cost = {};
  const document = { documentElement: { dataset: {} }, activeElement: bars[1], addEventListener() {}, getElementById: (id) => ({ 'usage-chart': chart, 'usage-part-cost': cost })[id] || null };
  const d = dashboard({ document, addEventListener() {}, setInterval() {} });
  d.seed({ range: { start: '2026-10-05', end: '2026-10-08', through: '2026-10-07' }, summary: { combined: { trend: [{ date: '2026-10-05', group: 'anthropic', observations: 4, estimated_cost_nanos: 3e9 }] } } });
  d.context.cost = cost;
  d.run('S.route = "usage"; usagePainted.set(cost, "initial"); usageHover("2026-10-05")');
  assert.deepEqual(bars.map((b) => b.className), ['u-bar on', 'u-bar dim', 'u-bar dim']);
  assert.match(readout.innerHTML, /Day · Mon Oct 5/);
  assert.match(readout.innerHTML, /4 calls/);
  assert.equal(d.run('usagePainted.has(cost)'), false);
  d.run('usageHover("2026-10-06")');
  assert.deepEqual(bars.map((b) => b.className), ['u-bar dim', 'u-bar on', 'u-bar dim']);
  assert.match(readout.innerHTML, /Day · Tue Oct 6/);
  assert.equal(classWrites.length, 5, 'moving between bars changes only the previous and next selection');
  d.run('usageHover("2026-10-06")');
  assert.equal(writes.length, 2, 'repeated events on one day do no work');
  d.run('usageHover(null)');
  assert.deepEqual(bars.map((b) => b.className), ['u-bar', 'u-bar', 'u-bar']);
  assert.match(readout.innerHTML, /Peak day · Mon Oct 5/);
  assert.equal(document.activeElement, bars[1], 'the focused bar is the same element throughout');
  d.run('S.route = "overview"; usageHover("2026-10-05")');
  assert.equal(writes.length, 3, 'events outside the Usage page are ignored');
});

test('daily priced subset is distinct from the range breakdown and Fable is visible initially', () => {
  const d = dashboard();
  const trend = [
    { date: '2026-10-01', group: 'anthropic', observations: 2446, estimated_cost_nanos: 280000, unpriced: 2444 },
    { date: '2026-10-01', group: 'openai', observations: 13323, estimated_cost_nanos: 2111131084260, unpriced: 5 },
  ];
  const models = Array.from({ length: 15 }, (_, i) => ({ id: i === 10 ? 'claude-fable-5-1' : `model-${i}`, provider: 'anthropic', observations: 10, estimated_cost_nanos: 1e9 }));
  models[10] = { ...models[10], observations: 1896, estimated_cost_nanos: 6900000, unpriced: 1895, partial: 1880, missing_usage: 16 };
  d.seed({ preset: '30', range: { start: '2026-09-08', end: '2026-10-08', through: '2026-10-07' }, summary: { combined: { trend, totals: { estimated_cost_nanos: 14568080000000 }, breakdowns: { model: models } } } });
  const chart = d.run('usageChartHTML()');
  assert.match(chart, /Peak day · Thu Oct 1/);
  assert.match(chart, /Priced calls only/);
  assert.match(chart, /Claude<\/span> <span class="mono">\$0\.0003<\/span><span class="dim">\(2 priced · 2,444 unpriced\)/);
  const breakdown = d.run('usageBreakdownHTML()');
  assert.match(breakdown, /BREAKDOWN · LAST 30 DAYS/);
  assert.match(breakdown, /claude-fable-5-1/);
  assert.match(breakdown, /\$0\.0069/);
  assert.match(breakdown, /1,895 unpriced/);
  assert.match(breakdown, /title="[^"]*1,880 partial · 16 missing usage/);
  assert.equal(d.run('Usage.rows.length'), 15);
  assert.equal(d.run('Usage.rows[10]'), 'claude-fable-5-1');
  assert.equal(breakdown.includes('Show all 15'), false);
  d.run('Usage.hoverDay = "2026-10-01"');
  assert.match(d.run('usageChartHTML()'), /Day · Thu Oct 1/);
  assert.match(d.run('usageBreakdownHTML()'), /BREAKDOWN · LAST 30 DAYS/);
});

test('cost charts preserve wholly unpriced groups, known zero and empty days', () => {
  const d = dashboard();
  const trend = [
    { date: '2026-10-05', group: 'anthropic', observations: 4, estimated_cost_nanos: null, unpriced: 4 },
    { date: '2026-10-06', group: 'openai', observations: 1, estimated_cost_nanos: 0, unpriced: 0 },
  ];
  const chart = helpers.chart(trend, ['2026-10-05', '2026-10-06', '2026-10-07'], 'cost');
  assert.deepEqual(chart.keys, ['anthropic', 'openai']);
  assert.equal(chart.max, 0);
  assert.equal(chart.peak, 0);
  assert.equal(chart.series[0].groups[0].priced, false);
  assert.equal(chart.series[1].groups[1].priced, true);
  d.seed({ range: { start: '2026-10-05', end: '2026-10-08', through: '2026-10-07' }, hoverDay: '2026-10-05', summary: { combined: { trend } } });
  let html = d.run('usageChartHTML()');
  assert.match(html, /Claude<\/span> <span class="mono">Unpriced/);
  assert.match(html, /0 priced · 4 unpriced/);
  assert.match(html, /aria-label="Mon Oct 5: Unpriced · 0 priced calls · 4 unpriced"/);
  assert.match(html, /aria-label="Tue Oct 6: \$0\.00 · 1 priced calls"/);
  assert.match(html, /aria-label="Wed Oct 7: \$0\.00 · 0 priced calls"/);
  d.run('Usage.hoverDay = null');
  html = d.run('usageChartHTML()');
  assert.match(html, /Busiest day · Mon Oct 5/);
  assert.match(html, /Claude<\/span> <span class="mono">Unpriced/);
  d.run('Usage.metric = "calls"');
  html = d.run('usageChartHTML()');
  assert.match(html, /Claude<\/span> <span class="mono">4 calls/);
  assert.match(html, /\(4 unpriced\)/);
});

test('a range with only unknown costs has an unpriced headline while an empty range is zero', () => {
  const d = dashboard();
  d.seed({ summary: { combined: { totals: { observations: 4, estimated_cost_nanos: null, unpriced: 4 } } } });
  assert.match(d.run('usageCostHTML()'), /u-total-v">Unpriced/);
  d.seed({ summary: { combined: { totals: { observations: 0, estimated_cost_nanos: null, unpriced: 0 } } } });
  assert.match(d.run('usageCostHTML()'), /u-total-v">\$0\.00/);
});

test('Other retains the priced and unpriced coverage of every merged group', () => {
  const trend = Array.from({ length: 8 }, (_, i) => ({ date: '2026-10-05', group: `model-${i}`, observations: 10, estimated_cost_nanos: i < 6 ? (8 - i) * 1e9 : null, unpriced: i < 6 ? 1 : 10 }));
  const c = helpers.chart(trend, ['2026-10-05'], 'cost');
  assert.equal(c.keys[5], helpers.OTHER);
  assert.deepEqual(c.series[0].groups[5], { calls: 30, cost: 3e9, priced: true, unpriced: 21 });
  assert.equal(c.series[0].calls, 80);
  assert.equal(c.series[0].unpriced, 26);
  assert.equal(c.series[0].cost, 33e9);
});

test('model rows beyond the initial twenty can expand and coverage explains unknown costs', () => {
  const d = dashboard();
  const rows = Array.from({ length: 21 }, (_, i) => ({ id: `model-${i}`, observations: 2, estimated_cost_nanos: i ? 1e9 : null, unpriced: i ? 0 : 2 }));
  d.seed({ summary: { combined: { totals: { estimated_cost_nanos: 20e9 }, breakdowns: { model: rows } } } });
  let html = d.run('usageBreakdownHTML()');
  assert.equal(d.run('Usage.rows.length'), 20);
  assert.match(html, /Show all 21/);
  assert.match(html, /u-bd-cost dim">Unpriced/);
  assert.equal(html.includes('model-20'), false);
  d.run('Usage.showAll = true');
  html = d.run('usageBreakdownHTML()');
  assert.equal(d.run('Usage.rows.length'), 21);
  assert.match(html, /Show top 20/);
  assert.match(html, /model-20/);
});

test('late responses from a superseded filter do not replace current results', async () => {
  let resolveFirst;
  let call = 0;
  const first = new Promise((resolve) => { resolveFirst = resolve; });
  const d = dashboard({ api: () => { call++; return call <= 2 ? first : Promise.resolve({ marker: 'new-result' }); } });
  const oldLoad = d.run('usageLoad()');
  const newLoad = d.run('usageLoad()');
  await newLoad;
  resolveFirst({ marker: 'old-result' });
  await oldLoad;
  assert.equal(d.run('Usage.summary.marker'), 'new-result');
  assert.equal(d.run('Usage.loading'), false);
});

test('full refresh and filter changes abort superseded reads without exposing cancellation errors', async () => {
  const pending = pendingApi();
  const d = dashboard({ api: pending.api });
  const first = d.run('usageLoad()');
  assert.deepEqual(pending.calls.map((call) => call.endpoint.split('?')[0]), ['/usage/dashboard', '/usage/status'], 'no records are read');
  const refresh = d.run('usageLoad()');
  assert.ok(pending.calls.slice(0, 2).every((call) => call.signal.aborted));
  assert.ok(pending.calls.slice(2).every((call) => !call.signal.aborted));
  await first;
  assert.equal(d.run('Usage.loading'), true);
  assert.equal(d.run('Usage.error'), '');
  d.run('Usage.filters.model = "new-model"');
  const filtered = d.run('usageReload()');
  assert.ok(pending.calls.slice(0, 4).every((call) => call.signal.aborted));
  await refresh;
  assert.equal(d.run('Usage.loading'), true);
  assert.equal(d.run('Usage.error'), '');
  const active = pending.calls.slice(4);
  assert.equal(active.length, 2);
  assert.match(active[0].endpoint, /model=new-model/);
  active.forEach((call) => call.resolve({ marker: 'filtered' }));
  await filtered;
  for (const field of ['summary', 'status']) assert.equal(d.run(`Usage.${field}.marker`), 'filtered');
  assert.equal(d.run('Usage.loading'), false);
  assert.equal(d.run('Usage.error'), '');
  assert.equal(d.run('usageReads.size'), 0);
});

test('same-section replacement ignores late transport completion even within one load sequence', async () => {
  const pending = pendingApi(true);
  const d = dashboard({ api: pending.api });
  const first = d.run('usageRead("summary", "/usage/dashboard?old", Usage.sequence)');
  const next = d.run('usageRead("summary", "/usage/dashboard?new", Usage.sequence)');
  assert.equal(pending.calls[0].signal.aborted, true);
  assert.equal(pending.calls[1].signal.aborted, false);
  pending.calls[0].resolve({ marker: 'stale' });
  await first;
  assert.equal(d.run('Usage.summary'), null);
  assert.equal(d.run('usageReads.size'), 1);
  pending.calls[1].resolve({ marker: 'latest' });
  await next;
  assert.equal(d.run('Usage.summary.marker'), 'latest');
  assert.equal(d.run('usageReads.size'), 0);
});

test('route exit and pagehide cancel reads, and re-entry and restored pages resume cleanly', async () => {
  const pending = pendingApi(true), listeners = new Map();
  const d = dashboard({ api: pending.api,
    document: { addEventListener() {}, body: { style: {} } }, setInterval() {},
    addEventListener: (name, listener) => listeners.set(name, listener), location: { hash: '#/usage' },
  });
  d.run('usageRender = () => {}; S.route = "usage"');
  const first = d.run('usageLoad()');
  listeners.get('hashchange')();
  assert.ok(pending.calls.every((call) => !call.signal.aborted));
  d.context.location.hash = '#/accounts';
  listeners.get('hashchange')();
  assert.ok(pending.calls.every((call) => call.signal.aborted));
  assert.equal(d.run('Usage.loading'), false);
  assert.equal(d.run('Usage.loaded'), false);
  d.context.location.hash = '#/usage';
  d.run('bindUsage()');
  assert.equal(pending.calls.length, 4);
  pending.calls.slice(0, 2).forEach((call) => call.resolve({ marker: 'stale-route' }));
  await first;
  assert.equal(d.run('Usage.summary'), null);
  assert.equal(d.run('Usage.loading'), true);
  listeners.get('pagehide')();
  assert.ok(pending.calls.every((call) => call.signal.aborted));
  listeners.get('pageshow')({ persisted: true });
  assert.equal(pending.calls.length, 6);
  pending.calls.slice(2, 4).forEach((call) => call.reject(new Error('late failure')));
  await new Promise(setImmediate);
  assert.equal(d.run('Usage.error'), '');
  assert.equal(d.run('Usage.loading'), true);
  pending.calls.slice(4).forEach((call) => call.resolve({ marker: 'restored' }));
  await new Promise(setImmediate);
  assert.equal(d.run('Usage.summary.marker'), 'restored');
  assert.equal(d.run('Usage.status.marker'), 'restored');
  assert.equal(d.run('Usage.loading'), false);
  assert.equal(d.run('Usage.error'), '');
  assert.equal(d.run('usageReads.size'), 0);
});

test('a status failure preserves a successful summary read with visible error', async () => {
  const d = dashboard({ api: (endpoint) => endpoint === '/usage/status' ? Promise.reject(new Error('database unavailable')) : Promise.resolve({ marker: 'loaded' }) });
  await d.run('usageLoad()');
  assert.equal(d.run('Usage.summary.marker'), 'loaded');
  assert.match(d.run('Usage.error'), /database unavailable/);
  assert.equal(d.run('Usage.loading'), false);
});

test('counts render immediately while status is still pending', async () => {
  let finishStatus;
  const status = new Promise((resolve) => { finishStatus = resolve; });
  let renders = 0;
  const d = dashboard({ api: (endpoint) => endpoint === '/usage/status' ? status : Promise.resolve({ marker: 'counts-ready' }) });
  d.context.markRender = () => { renders++; };
  d.run('usageRender = markRender');
  const loading = d.run('usageLoad()');
  await new Promise(setImmediate);
  assert.equal(d.run('Usage.summary.marker'), 'counts-ready');
  assert.equal(d.run('Usage.loading'), true);
  assert.ok(renders >= 2);
  finishStatus({ health: {} });
  await loading;
  assert.equal(d.run('Usage.loading'), false);
});

test('chart stacking uses the loaded snapshot without another request, including during initial load', async () => {
  let finishSummary;
  let calls = 0;
  const response = { combined: { stack: 'provider', trend: [{ marker: 'provider' }], trends: { provider: [{ marker: 'provider' }], model: [{ marker: 'model' }] } } };
  const d = dashboard({ api: (endpoint) => {
    calls++;
    return endpoint.includes('/dashboard?') ? new Promise((resolve) => { finishSummary = resolve; }) : Promise.resolve({});
  } });
  const loading = d.run('usageLoad()');
  d.run('Usage.stack = "model"; usageLoadStack()');
  finishSummary(response);
  await loading;
  assert.equal(d.run('Usage.summary.combined.trend[0].marker'), 'model');
  assert.equal(d.run('Usage.summary.combined.stack'), 'model');
  d.run('Usage.stack = "provider"; usageLoadStack()');
  assert.equal(d.run('Usage.summary.combined.trend[0].marker'), 'provider');
  assert.equal(calls, 2);
});

test('the page is drawn once; a read or an account change repaints only the sections that changed', () => {
  const writes = [];
  const elements = new Map();
  const element = (id) => {
    let html = '';
    const el = { id, contains: () => false, setAttribute() {}, get innerHTML() { return html; }, set innerHTML(value) { html = value; writes.push(id); } };
    elements.set(id, el);
    return el;
  };
  const body = { closest: () => null };
  const document = { body, documentElement: { dataset: {} }, activeElement: body, getElementById: (id) => elements.get(id) || null, querySelector: () => null, contains: () => true, addEventListener() {} };
  const d = dashboard({ document, addEventListener() {}, setInterval() {}, render: () => writes.push('render'), accountById: (id) => d.context.S.accounts.find((a) => a.id === id) });
  const clients = Array.from({ length: 9 }, (_, i) => ({ id: `client-${i}`, observations: 1, tokens: {}, models: [] }));
  d.seed({ loaded: true, dim: 'account', summary: { combined: { totals: { observations: 1 }, trend: [], breakdowns: { account: [{ id: 'a', observations: 1 }], client: clients } } }, status: { health: {} } });
  d.run('S.route = "usage"; usageHTML()');
  element('usage-results');
  for (const name of ['head', 'notices', 'cost', 'breakdown', 'clients', 'drawer']) element(`usage-part-${name}`);
  d.run('bindUsage()');
  d.run('usageRender()');
  assert.deepEqual(writes, [], 'nothing changed, nothing is written');
  d.run('Usage.clientsAll = true; usageRender(undefined, ["clients"])');
  assert.deepEqual(writes, ['usage-part-clients']);
  assert.match(elements.get('usage-part-clients').innerHTML, /client-8/);
  writes.length = 0;
  d.run('S.accounts = [{ id: "a", label: "work@example.com", provider: "claude", kind: "oauth" }]; usageAccountsChanged()');
  assert.deepEqual(writes, ['usage-part-breakdown'], 'an account name reaches the breakdown');
  assert.match(elements.get('usage-part-breakdown').innerHTML, /work@example\.com/);
  writes.length = 0;
  d.run('Usage.dim = "provider"; usageRender()');
  assert.deepEqual(writes, ['usage-part-breakdown'], 'a breakdown tab repaints the breakdown only');
  elements.delete('usage-results');
  writes.length = 0;
  d.run('usageRender()');
  assert.deepEqual(writes, ['render'], 'without the page, it is drawn whole');
});


test('cache write lifetimes are independent subsets and unknown history stays explicit', () => {
  const d = dashboard();
  assert.equal(d.run('usageCacheLifetimes({ cache_write: 350, write_30m: 100, write_12h: 200 })'), 'Cache write lifetimes: 100 (30 min), 200 (12 hr), 50 (lifetime unknown)');
  assert.equal(d.run('usageCacheLifetimes({ cache_write: 300 })'), 'Cache write lifetimes: 300 (lifetime unknown)');
  assert.equal(d.run('usageCacheLifetimes({ write_12h: 200 })'), 'Cache write lifetimes: 200 (12 hr), total unknown');
  assert.equal(d.run('usageTokenTotal({ input: 10, cache_read: 20, cache_write: 300, write_30m: 100, write_12h: 200, output: 40 })'), 370);
  d.seed({ summary: { combined: { totals: { tokens: { cache_write: 300, write_30m: 100, write_12h: 200 } } } } });
  const html = d.run('usageCostHTML()');
  assert.match(html, /Cache write lifetimes: 100 \(30 min\), 200 \(12 hr\)/);
  assert.doesNotMatch(html, /5 min|1 hr/);
});
