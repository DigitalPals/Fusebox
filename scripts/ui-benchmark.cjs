#!/usr/bin/env node
'use strict';

// Synthetic CPU/markup benchmark; no server, browser, network or credentials.
// Compare a checkout with its base, e.g.:
//   node scripts/ui-benchmark.cjs --baseline-ref f49466b --output /tmp/ui-results.json
// DOM parsing, style/layout and painting are deliberately outside this measurement.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
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
const baselineCommit = execFileSync('git', ['rev-parse', `${baselineRef}^{commit}`], { cwd: root, encoding: 'utf8' }).trim();
const events = 60, batches = 5;

function fixture(days, groups) {
  const end = new Date('2026-10-30T00:00:00Z');
  const dates = Array.from({ length: days }, (_, i) => new Date(end.getTime() - (days - i) * 86400000).toISOString().slice(0, 10));
  const trend = dates.flatMap((date, day) => Array.from({ length: groups }, (_, group) => ({ date, group: `model-${group}`, observations: 20 + day % 11 + group, estimated_cost_nanos: (groups - group) * 1234567, unpriced: group % 3 })));
  return { days: dates, groups, range: { start: dates[0], end: '2026-10-30', through: dates.at(-1) }, summary: { combined: { trend, totals: {} } } };
}

function harness(source, data) {
  const counts = {};
  const reset = () => Object.assign(counts, { aggregations: 0, numberFormatters: 0, chartReplacements: 0, readoutReplacements: 0, assignedHTMLBytes: 0, classChanges: 0 });
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
    Intl: { DateTimeFormat: Intl.DateTimeFormat, NumberFormat: class extends Intl.NumberFormat {
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

const results = [];
for (const [days, groups] of [[7, 3], [30, 20], [366, 40]]) {
  const data = fixture(days, groups);
  const before = measure(baseline, data), after = measure(current, data);
  results.push({ days, groups, trendRows: data.summary.combined.trend.length, baseline: before, optimized: after, speedup: Number((before.medianMilliseconds / after.medianMilliseconds).toFixed(2)) });
}
const report = {
  measuredAt: new Date().toISOString(), node: process.version, baselineCommit,
  command: `node scripts/ui-benchmark.cjs --baseline-ref ${baselineCommit}`,
  method: 'Median of five batches of 60 desktop chart hovers after warmup; real dashboard source in a Node VM with minimal DOM stand-ins. Timing covers JavaScript/HTML generation only, excluding browser DOM parsing, layout and painting. Synthetic snapshots contain 7, 30 or 366 days and 3, 20 or 40 groups.',
  eventsPerBatch: events, batches, results,
};
const json = `${JSON.stringify(report, null, 2)}\n`;
if (options['--output']) fs.writeFileSync(path.resolve(options['--output']), json);
process.stdout.write(json);
