const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// An account's meter rows and window blocks, run from their own sources. Rows always
// count down to the provider's reset, including when usage would run out sooner.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  return app.slice(at, end);
}

// Production on 2026-10-23: 5-hour window 99% used, renewing in 2h 58m.
const NOW = Date.parse('2026-10-23T13:42:00Z');
const RESET = '2026-10-23T16:40:00Z';
function load() {
  const context = { S: { quotaDisplay: 'remaining' }, Date: class extends Date { static now() { return NOW; } }, usedByHTML: () => '' };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function left(iso) {'), piece('function compactLeft(iso) {'),
    piece('const hm ='), piece('const weekday ='), piece('const fullResetTime ='), piece('const when ='), piece('const liveLeft ='),
    piece('function quotaOf(w'), piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'),
    piece('function windowOf(a, short) {'), piece('const NO_SHORT_LIMIT ='), piece('const noShortLimit ='), piece('const noShortNote ='),
    piece('const windowLabel ='), piece('const windowTitle ='), piece('function windowSeconds(w) {'),
    piece('function windowElapsed(w'), piece('function windowSpent(a, w) {'),
    piece('const segsHTML ='), piece('function meterHTML(q'), piece('function pctHTML(w) {'), piece('function meterRowHTML(a, short) {'), piece('const meterRowsHTML ='),
    piece('function windowBlockHTML(a, w, title, size) {'), piece('function miniMetersHTML(a) {'), piece('function windowHTML(a, short, size) {'),
    piece('function noShortBlockHTML(size) {'), piece('function quotaWindowsHTML(a'),
  ].join('\n'), context);
  return (code) => vm.runInContext(code, context);
}

const account = (cooldowns = {}, kinds = {}) => ({
  id: 'a', provider: 'claude', kind: 'oauth', cooldowns, cooldown_kinds: kinds,
  quota: { windows: [{ name: '5h', used: 99, resets_at: RESET }, { name: 'week', used: 23, resets_at: '2026-10-28T16:00:00Z' }] },
});
const row = (run, a, short = true) => run(`meterRowHTML(${JSON.stringify(a)}, ${short})`);
const block = (run, a) => run(`(() => { const a = ${JSON.stringify(a)}; return windowBlockHTML(a, windowOf(a, true), '5-hour', 'lg'); })()`);

test('windows always show their reset countdown even when usage would run out sooner', () => {
  const run = load();
  for (const mode of ['remaining', 'used']) {
    run(`S.quotaDisplay = '${mode}'`);
    for (const used of [0, 50, 99, 100]) {
      const a = account();
      a.quota.windows[0].used = used;
      a.quota.windows[1].used = used;
      for (const short of [true, false]) {
        const reset = a.quota.windows[short ? 0 : 1].resets_at;
        const html = row(run, a, short);
        assert.match(html, /class="rst" title="(?:5-hour|Weekly) window resets [^"]+">↺ /);
        assert.ok(html.includes(`data-until="${reset}" data-countdown="compact"`));
        assert.doesNotMatch(html, /out|using .*% an hour/);
      }
      const note = block(run, a);
      assert.match(note, /(?:Resets|Back) .* · in <span[^>]*>2h 58m<\/span>/);
      assert.doesNotMatch(note, /at this pace/);
    }
  }
});

test('a usage-limit cooldown until the reset shows the reset, not the projection', () => {
  const run = load();
  const cooling = account({ 'claude-opus-5-5': '2026-10-23T16:40:00+00:00' }, { 'claude-opus-5-5': 'quota' });
  const html = row(run, cooling);
  assert.doesNotMatch(html, /out/);
  assert.match(html, /class="rst"[^>]*>↺ <span data-until="2026-10-23T16:40:00Z" data-countdown="compact">2h 58m<\/span>/);
  const note = block(run, cooling);
  assert.match(note, /Back \S+ · in <span[^>]*>2h 58m<\/span>/);
  assert.doesNotMatch(note, /at this pace/);
  assert.match(row(run, cooling, false), /↺ <span[^>]*>5d 2h<\/span>/, 'the weekly row is unchanged');
});

test('cooldown scope does not change the reset countdown', () => {
  const run = load();
  const early = account({ 'claude-opus-5-5': '2026-10-23T14:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.match(row(run, early), /class="rst"[^>]*>↺ <span[^>]*>2h 58m<\/span>/);
  const limited = account({ 'claude-opus-5-5': RESET }, { 'claude-opus-5-5': 'rate_limit' });
  assert.match(row(run, limited), /class="rst"[^>]*>↺ <span[^>]*>2h 58m<\/span>/);
  const weekly = account({ 'claude-opus-5-5': '2026-10-28T16:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.doesNotMatch(row(run, weekly), /out/, 'a weekly limit used up covers the 5-hour window too');
});

test('lists leave out the 5-hour meter of an account without that limit; its page says why', () => {
  const run = load();
  const ultra = { id: 'u', provider: 'claude', kind: 'oauth', quota: { no_five_hour_limit: true, windows: [{ name: 'week', used: 23, resets_at: '2026-10-28T16:00:00Z' }] } };
  const rows = run(`meterRowsHTML(${JSON.stringify(ultra)})`);
  assert.equal((rows.match(/class="mrow"/g) || []).length, 1);
  assert.doesNotMatch(rows, /5H|No limit|not reported/);
  assert.match(rows, /<span class="capsm">WK<\/span>/);
  assert.match(rows, /↺ /, 'the weekly row is unchanged');
  const mini = run(`miniMetersHTML(${JSON.stringify(ultra)})`);
  assert.equal((mini.match(/class="mrow"/g) || []).length, 1);
  assert.doesNotMatch(mini, /5H/);
  assert.equal(run(`noShortNote(${JSON.stringify(ultra)})`), ' · no 5h limit');
  const blocks = run(`quotaWindowsHTML(${JSON.stringify(ultra)}, 'lg')`);
  assert.match(blocks, /^<div class="window"><div class="window-top"><span>5-hour<\/span><span class="dim">No limit<\/span>/);
  assert.match(blocks, /This account has no five-hour limit; only the weekly limits apply\./);
  assert.match(blocks, /Weekly/);
  assert.match(run(`windowHTML(${JSON.stringify(ultra)}, true, 'xl')`), /5-hour window<\/span><span class="dim">No limit/);
  // A reported 5-hour window always wins, and plain missing data stays "not reported".
  const back = { ...ultra, quota: { ...ultra.quota, windows: [{ name: '5h', used: 10, resets_at: RESET }, ...ultra.quota.windows] } };
  assert.equal((run(`meterRowsHTML(${JSON.stringify(back)})`).match(/class="mrow"/g) || []).length, 2);
  assert.match(run(`meterRowsHTML(${JSON.stringify(back)})`), /<span class="capsm">5H<\/span>/);
  assert.equal(run(`noShortNote(${JSON.stringify(back)})`), '');
  const unknown = { ...ultra, quota: { windows: ultra.quota.windows } };
  assert.match(run(`meterRowsHTML(${JSON.stringify(unknown)})`), /5-hour: not reported/);
  assert.equal((run(`miniMetersHTML(${JSON.stringify(unknown)})`).match(/class="mrow"/g) || []).length, 2);
  assert.equal(run(`noShortNote(${JSON.stringify(unknown)})`), '');
});
