const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Exercise the dashboard's actual live request handler without a browser/DOM.
const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const handlerAt = source.indexOf('function onRequest(log) {'), handlerTo = source.indexOf('// ---------------------------------------------------------------- render', handlerAt);
assert.ok(handlerAt >= 0 && handlerTo > handlerAt, 'app.js no longer has onRequest .. render');
const handler = source.slice(handlerAt, handlerTo);

function dashboard() {
  const S = {
    requests: [], fresh: new Map(), activity: {}, route: 'none',
    overview: { totals: { requests: 0, ok: 0, failed: 0 }, series: [] },
  };
  const context = { S, setTimeout() {}, renderFaults() {} };
  vm.createContext(context);
  vm.runInContext(handler, context);
  return {
    S, context,
    record(minute, input = 100) {
      context.onRequest({ id: S.requests.length, ts: new Date(minute * 60000).toISOString(), status: 200, usage_completeness: 'complete', input_tokens: input, output_tokens: 2, cache_tokens: 3 });
    },
  };
}

test('out-of-order stream completions update one bucket per start minute', () => {
  const now = Math.floor(Date.now() / 60000);
  const { S, record } = dashboard();
  [now - 2, now, now - 2, now - 1, now - 2].forEach((minute) => record(minute));
  assert.equal(S.overview.series.length, 3);
  const first = S.overview.series.find((bucket) => bucket.minute === now - 2);
  assert.equal(first.requests, 3);
  assert.equal(first.input_tokens, 300);
  assert.equal(first.tokens, 315);
  assert.equal(S.overview.series.reduce((sum, bucket) => sum + bucket.requests, 0), 5);
  assert.ok(S.overview.series.every((bucket, index, series) => !index || series[index - 1].minute < bucket.minute));
});

test('a very long stream finishing late preserves the current hour', () => {
  const now = Math.floor(Date.now() / 60000);
  const { S, record } = dashboard();
  for (let ago = 59; ago >= 0; ago--) record(now - ago);
  record(now - 65, 1000);
  assert.equal(S.overview.series.length, 60);
  assert.equal(S.overview.series.reduce((sum, bucket) => sum + bucket.input_tokens, 0), 6000);
  assert.equal(S.overview.totals.requests, 61);
});

test('timed requests add their output speed per model and tier, and the figure reads the hour', () => {
  const now = Math.floor(Date.now() / 60000);
  const d = dashboard();
  const log = (minute, model, output, stream_ms, speed) => d.context.onRequest({ id: 0, ts: new Date(minute * 60000).toISOString(), status: 200,
    usage_completeness: 'complete', model, speed, input_tokens: 10, output_tokens: output, cache_tokens: 0, stream_ms });
  log(now, 'gpt-6.1-sol', 1240, 10000, 'fast');
  log(now, 'gpt-6.1-sol', 720, 10000);
  log(now - 1, 'gpt-6.1-sol', 300, 2000, 'fast');
  log(now, 'claude-opus-5-5', 900);
  const c = vm.runInContext('seriesCounters(S.overview.series)', d.context);
  assert.equal(c.timed_output, 2260);
  assert.equal(c.stream_ms, 22000);
  assert.equal(c.output_tokens, 3160, 'untimed output still counts as output');
  assert.deepEqual(JSON.parse(JSON.stringify([...c.speeds.values()].sort((a, b) => a.speed.localeCompare(b.speed)))), [
    { model: 'gpt-6.1-sol', speed: 'fast', output: 1540, stream_ms: 12000 },
    { model: 'gpt-6.1-sol', speed: 'standard', output: 720, stream_ms: 10000 },
  ]);
  Object.assign(d.context, { esc: (v) => String(v).replace(/</g, '&lt;'), fmt: (n) => n.toLocaleString('en-US'), plural: (n, w) => `${n} ${w}${n === 1 ? '' : 's'}` });
  const html = vm.runInContext('speedFigureHTML(seriesCounters(S.overview.series))', d.context);
  assert.match(html, /^<a class="fig-speed" href="#\/usage"/);
  assert.ok(html.includes('103<small>tok/s</small>'), '2260 tokens in 22 s');
  assert.ok(html.indexOf('gpt-6.1-sol</span><span class="fig-tag">Fast</span><b>128 tok/s') > 0, 'busiest first, Fast tagged');
  assert.ok(html.includes('gpt-6.1-sol</span><b>72 tok/s'));
  assert.ok(vm.runInContext('speedFigureHTML(seriesCounters([]))', d.context).includes('—'), 'nothing timed');
});
