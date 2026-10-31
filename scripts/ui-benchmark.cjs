#!/usr/bin/env node
'use strict';

// Synthetic CPU/markup benchmark; no server, browser, network or credentials.
// Compare a checkout with its base, e.g.:
//   node scripts/ui-benchmark.cjs --baseline-ref f49466b --output /tmp/ui-results.json
// DOM parsing, style/layout and painting are deliberately outside this measurement.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const os = require('node:os');
const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { performance } = require('node:perf_hooks');

const root = path.resolve(__dirname, '..');
const options = { '--baseline-ref': 'HEAD', '--output': null };
for (let i = 2; i < process.argv.length; i += 2) {
  const key = process.argv[i], value = process.argv[i + 1];
  if (!Object.hasOwn(options, key) || !value) throw new Error('Usage: ui-benchmark.cjs [--baseline-ref REF] [--output FILE]');
  options[key] = value;
}
const baselineRef = options['--baseline-ref'];
const baseline = execFileSync('git', ['show', `${baselineRef}:ui/usage.js`], { cwd: root, encoding: 'utf8' });
const current = fs.readFileSync(path.join(root, 'ui/usage.js'), 'utf8');
const baselineApp = execFileSync('git', ['show', `${baselineRef}:ui/app.js`], { cwd: root, encoding: 'utf8' });
const currentApp = fs.readFileSync(path.join(root, 'ui/app.js'), 'utf8');
const baselineCommit = execFileSync('git', ['rev-parse', `${baselineRef}^{commit}`], { cwd: root, encoding: 'utf8' }).trim();
const events = 60, batches = 5;

function fixture(days, groups) {
  const end = new Date('2026-10-30T00:00:00Z');
  const dates = Array.from({ length: days }, (_, i) => new Date(end.getTime() - (days - i) * 86400000).toISOString().slice(0, 10));
  const trend = dates.flatMap((date, day) => Array.from({ length: groups }, (_, group) => ({ date, group: `model-${group}`, observations: 20 + day % 11 + group, estimated_cost_nanos: (groups - group) * 1234567, unpriced: group % 3 })));
  return { days: dates, groups, range: { start: dates[0], end: '2026-10-30', through: dates.at(-1) }, summary: { combined: { trend, totals: {}, proxy_first_event_at_ms: Date.parse('2026-10-15T12:00:00Z') } } };
}

function harness(source, data) {
  const counts = {};
  const reset = () => Object.assign(counts, { aggregations: 0, numberFormatters: 0, dateFormatters: 0, chartReplacements: 0, readoutReplacements: 0, assignedHTMLBytes: 0, classChanges: 0 });
  reset();
  const assign = (kind, value) => { counts[kind]++; counts.assignedHTMLBytes += Buffer.byteLength(value); };
  const readout = { set innerHTML(value) { assign('readoutReplacements', value); } };
  const bars = data.days.map((date) => {
    let className = 'u-bar';
    return { dataset: { usageDay: date }, get className() { return className; }, set className(value) { className = value; counts.classChanges++; } };
  });
  const chart = {
    set innerHTML(value) { assign('chartReplacements', value); },
    querySelector: () => readout,
    querySelectorAll: () => bars,
  };
  const cost = {};
  const context = {
    Intl: { DateTimeFormat: function (...args) { counts.dateFormatters++; return new Intl.DateTimeFormat(...args); }, NumberFormat: class extends Intl.NumberFormat {
      constructor(...args) { super(...args); counts.numberFormatters++; }
    } },
    Date, URLSearchParams, AbortController, setInterval() {}, addEventListener() {},
    document: { documentElement: { dataset: {} }, activeElement: null, addEventListener() {}, getElementById: (id) => ({ 'usage-chart': chart, 'usage-part-cost': cost })[id] || null },
    S: { private: false, route: 'usage', locked: false }, seed: data,
    aggregate: () => { counts.aggregations++; },
  };
  vm.createContext(context);
  vm.runInContext(source, context);
  vm.runInContext('Object.assign(Usage, seed); const originalChart = UsageTools.chart; UsageTools.chart = (...args) => { aggregate(); return originalChart(...args); }; usageRenderChart()', context);
  const hover = vm.runInContext('typeof usageHover === "function" ? usageHover : (day) => { Usage.hoverDay = day; usageRenderChart(); }', context);
  return { hover, counts, reset };
}

function measure(source, data) {
  const h = harness(source, data);
  for (let i = 0; i < 12; i++) h.hover(data.days[i % data.days.length]);
  const measurements = [];
  for (let batch = 0; batch < batches; batch++) {
    h.hover(null);
    h.reset();
    const start = performance.now();
    for (let i = 0; i < events; i++) h.hover(data.days[i % data.days.length]);
    measurements.push({ milliseconds: performance.now() - start, operations: { ...h.counts } });
  }
  measurements.sort((a, b) => a.milliseconds - b.milliseconds);
  const median = measurements[Math.floor(measurements.length / 2)];
  return { medianMilliseconds: Number(median.milliseconds.toFixed(3)), microsecondsPerHover: Number((median.milliseconds * 1000 / events).toFixed(2)), operationsPerBatch: median.operations };
}

function familyFixture(modelCount, accountCount) {
  const models = Array.from({ length: modelCount }, (_, i) => i < modelCount / 2 ? { id: `model-${i}` } : { id: `team/model-${i - modelCount / 2}`, prefix: 'team' });
  const accounts = Array.from({ length: accountCount }, (_, i) => ({ id: `account-${i}` }));
  const routes = Object.fromEntries(models.map((m, i) => [m.id, accounts.filter((_, j) => (i * 7 + j * 11) % 5 < 3).map((a, j) => ({ account: a.id, state: j % 2 ? 'ready' : 'cooling', next: j === 0 }))]));
  return { models, accounts, routes, family: { ids: models.map((m) => m.id) } };
}

function familyHarness(source, data, count = false) {
  const start = source.indexOf('const routeSteps ='), end = source.indexOf('function failuresByModel()', start);
  if (start < 0 || end < 0) throw new Error('Could not find model routing helpers');
  let reads = 0;
  const routes = count ? new Proxy(data.routes, { get(target, key) { reads++; return target[key]; } }) : data.routes;
  const context = { S: { models: data.models, routes: { models: routes } }, accountById: (id) => data.accounts.find((a) => a.id === id) };
  vm.createContext(context);
  vm.runInContext(source.slice(start, end), context);
  const render = vm.runInContext('famRoute', context);
  return { render: () => render(data.family), reads: () => reads };
}

function measureFamily(source, data) {
  const counted = familyHarness(source, data, true);
  const result = JSON.stringify(counted.render());
  const h = familyHarness(source, data);
  for (let i = 0; i < 5; i++) h.render();
  const times = [], renders = 10;
  for (let i = 0; i < batches; i++) {
    const start = performance.now();
    for (let j = 0; j < renders; j++) h.render();
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  return { result, timing: { medianMilliseconds: Number(median.toFixed(3)), microsecondsPerRender: Number((median * 1000 / renders).toFixed(2)), rendersPerBatch: renders, routeReadsPerRender: counted.reads() } };
}

function requestHarness(usageSource, appSource, rows, mobile, counted = false) {
  const counts = { dateFormatters: 0, nativeLocaleCalls: 0 };
  const BenchDate = counted ? class extends Date {
    toLocaleTimeString(...args) { counts.nativeLocaleCalls++; return super.toLocaleTimeString(...args); }
    toLocaleDateString(...args) { counts.nativeLocaleCalls++; return super.toLocaleDateString(...args); }
    toLocaleString(...args) { counts.nativeLocaleCalls++; return super.toLocaleString(...args); }
  } : Date;
  const context = {
    Date: BenchDate, Intl: counted ? { NumberFormat: Intl.NumberFormat, DateTimeFormat: function (...args) {
      counts.dateFormatters++; return new Intl.DateTimeFormat(...args);
    } } : Intl,
    URLSearchParams, AbortController,
    S: { openReq: null }, rows, minuteLabels: new Map(),
    errLineText: () => '', isFresh: () => false, accountOf: () => null,
    esc: (value) => String(value ?? ''), ms: (value) => `${value}ms`,
    routeHTML: () => '', acctCellHTML: () => '', statusCell: () => '<span>200</span>', tokensText: () => '100',
    reqAcctName: () => 'Test account', noteHTML: () => '', logo: () => '',
    CLIENT: { responses: 'Responses' }, ENDPOINT: { responses: '/v1/responses' }, PROVIDER: {},
  };
  const section = (start, end) => {
    const at = appSource.indexOf(start), to = appSource.indexOf(end, at);
    if (at < 0 || to < 0) throw new Error(`Could not find request helper ${start}`);
    return appSource.slice(at, to);
  };
  vm.createContext(context);
  vm.runInContext(usageSource, context);
  vm.runInContext(section('const clock =', '// Within a day') + '\n' + section('function reqItemHTML(r) {', '// A live arrival goes on top'), context);
  const render = vm.runInContext(`() => {
    if (typeof syncLocalTime === 'function') syncLocalTime();
    return rows.map(${mobile ? 'reqMobileHTML' : 'reqItemHTML'}).join('');
  }`, context);
  return { render, counts, reset() { counts.dateFormatters = counts.nativeLocaleCalls = 0; } };
}

function measureRequests(rows, mobile) {
  const input = Array.from({ length: rows }, (_, i) => ({ id: i + 1, ts: new Date(Date.parse('2026-10-31T12:00:00Z') - i * 17000).toISOString(),
    model: 'model-test', client: 'responses', status: 200, latency_ms: 100, ttft_ms: 20, attempts: 1 }));
  const before = requestHarness(baseline, baselineApp, input, mobile), after = requestHarness(current, currentApp, input, mobile);
  if (before.render() !== after.render()) throw new Error('Request row markup changed');
  for (let i = 0; i < 3; i++) { before.render(); after.render(); }
  const times = { baseline: [], optimized: [] };
  // Alternate run order to reduce fixed ordering bias; preserve all nine samples.
  for (let i = 0; i < 9; i++) for (const [name, h] of i % 2 ? [['optimized', after], ['baseline', before]] : [['baseline', before], ['optimized', after]]) {
    const start = performance.now(); h.render(); times[name].push(performance.now() - start);
  }
  const result = { rows, layout: mobile ? 'mobile' : 'desktop' };
  for (const [name, usageSource, appSource] of [['baseline', baseline, baselineApp], ['optimized', current, currentApp]]) {
    const counted = requestHarness(usageSource, appSource, input, mobile, true);
    counted.render(); counted.reset(); counted.render();
    result[name] = { medianMilliseconds: Number([...times[name]].sort((a, b) => a - b)[4].toFixed(3)),
      samplesMilliseconds: times[name], operationsPerRender: { ...counted.counts } };
  }
  return result;
}

async function measureRefresh(source) {
  let nextTimer = 0, outstanding = 0, maximum = 0;
  const timers = new Map(), pending = [], calls = [];
  const context = {
    S: { route: 'models', locked: null }, overviewAt: 0, AbortController,
    setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
    api(endpoint) {
      calls.push(endpoint); outstanding++; maximum = Math.max(maximum, outstanding);
      return new Promise((resolve) => pending.push(() => { outstanding--; resolve({}); }));
    },
    refreshSetups() {}, expireActivity() {}, refreshViews() {}, syncResetModal() {},
  };
  const start = source.indexOf('const refreshing ='), end = source.indexOf('// Activity outlives account changes', start);
  if (start < 0 || end < start) throw new Error('Could not find live refresh helpers');
  vm.createContext(context);
  vm.runInContext(source.slice(start, end), context);
  for (let burst = 0; burst < 3; burst++) {
    vm.runInContext('scheduleRefresh("accounts")', context);
    for (const [id, timer] of [...timers]) if (timer.ms === 250) { timers.delete(id); timer.fn(); }
  }
  const outstandingAfterBursts = outstanding;
  for (let round = 0; pending.length && round < 10; round++) {
    pending.splice(0).forEach((resolve) => resolve());
    await new Promise(setImmediate);
  }
  if (outstanding || timers.size) throw new Error('Refresh fixture did not drain');
  return { outstandingAfterBursts, maximumOutstanding: maximum, totalReads: calls.length, endpoints: calls };
}

const results = [];
for (const [days, groups] of [[7, 3], [30, 20], [366, 40]]) {
  const data = fixture(days, groups);
  const before = measure(baseline, data), after = measure(current, data);
  results.push({ days, groups, trendRows: data.summary.combined.trend.length, baseline: before, optimized: after, speedup: Number((before.medianMilliseconds / after.medianMilliseconds).toFixed(2)) });
}
const modelFamilies = [];
for (const [models, accounts] of [[20, 5], [120, 40], [300, 80]]) {
  const data = familyFixture(models, accounts);
  const before = measureFamily(baselineApp, data), after = measureFamily(currentApp, data);
  if (before.result !== after.result) throw new Error('Model route output changed');
  modelFamilies.push({ models, accounts, baseline: before.timing, optimized: after.timing, speedup: Number((before.timing.medianMilliseconds / after.timing.medianMilliseconds).toFixed(2)) });
}
const requestRows = [6, 300].flatMap((rows) => [false, true].map((mobile) => measureRequests(rows, mobile)));
const report = {
  measuredAt: new Date().toISOString(), node: process.version, baselineCommit,
  environment: { platform: process.platform, architecture: process.arch, cpu: os.cpus()[0]?.model, logicalCPUs: os.cpus().length },
  sourceSha256: Object.fromEntries(Object.entries({ baselineUsage: baseline, currentUsage: current, baselineApp, currentApp }).map(([name, value]) => [name, createHash('sha256').update(value).digest('hex')])),
  command: `node scripts/ui-benchmark.cjs --baseline-ref ${baselineCommit}`,
  method: 'Real dashboard source in a Node VM with minimal DOM stand-ins. Chart and model-family medians use five batches after warmup: 60 hovers or 10 coverage calculations per batch. Request rows include the local-timezone boundary check, use three warmup renders, then nine alternating before/after runs (order reversed every other run), and preserve raw samples; unchanged non-date helpers are stand-ins. Before/after model routes and request markup must match. Operation counts are collected separately, outside timed runs. Timing excludes browser DOM parsing, layout and painting.',
  timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  eventsPerBatch: events, batches, results, modelFamilies, requestRows,
};
Promise.all([measureRefresh(baselineApp), measureRefresh(currentApp)]).then(([before, after]) => {
  report.liveRefresh = {
    scenario: 'Three account-event bursts on Models; each 250ms debounce fires before any API read resolves, then all reads resolve in controlled rounds. Counts are deterministic, not latency measurements.',
    baseline: before, optimized: after,
  };
  const json = `${JSON.stringify(report, null, 2)}\n`;
  if (options['--output']) fs.writeFileSync(path.resolve(options['--output']), json);
  process.stdout.write(json);
}).catch((error) => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
