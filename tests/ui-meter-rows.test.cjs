const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// An account's meter rows and window blocks, run from their own sources. A row normally
// counts down to its reset and says when the window runs out instead if the pace so far
// would use it up first; once a usage-limit cooldown lasts until the reset, the window is
// used up and the reset is when the account is back.
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
    piece('function windowOf(a, short) {'), piece('const windowLabel ='), piece('const windowTitle ='), piece('function windowSeconds(w) {'),
    piece('function windowElapsed(w'), piece('function burnRate(w'), piece('function runsOutAt(w'), piece('function windowSpent(a, w) {'),
    piece('const segsHTML ='), piece('function meterHTML(q'), piece('function pctHTML(w) {'), piece('function meterRowHTML(a, short) {'),
    piece('function windowBlockHTML(a, w, title, size) {'),
  ].join('\n'), context);
  return (code) => vm.runInContext(code, context);
}

const account = (cooldowns = {}, kinds = {}) => ({
  id: 'a', provider: 'claude', kind: 'oauth', cooldowns, cooldown_kinds: kinds,
  quota: { windows: [{ name: '5h', used: 99, resets_at: RESET }, { name: 'week', used: 23, resets_at: '2026-10-28T16:00:00Z' }] },
});
const row = (run, a, short = true) => run(`meterRowHTML(${JSON.stringify(a)}, ${short})`);
const block = (run, a) => run(`(() => { const a = ${JSON.stringify(a)}; return windowBlockHTML(a, windowOf(a, true), '5-hour', 'lg'); })()`);

test('a window the pace would use up says when it runs out', () => {
  const run = load();
  const html = row(run, account());
  assert.match(html, /class="rst out"/);
  assert.match(html, />out <span[^>]*>1m<\/span>/);
  assert.match(block(run, account()), /^(?:(?!at this pace).)*Resets .*at this pace, used up/s);
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

test('a cooldown that ends before the reset, or is not about usage limits, keeps the projection', () => {
  const run = load();
  const early = account({ 'claude-opus-5-5': '2026-10-23T14:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.match(row(run, early), /class="rst out"/, 'a quota cooldown that lifts before the window renews');
  const limited = account({ 'claude-opus-5-5': RESET }, { 'claude-opus-5-5': 'rate_limit' });
  assert.match(row(run, limited), /class="rst out"/, 'a rate limit says nothing about the window');
  const weekly = account({ 'claude-opus-5-5': '2026-10-28T16:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.doesNotMatch(row(run, weekly), /out/, 'a weekly limit used up covers the 5-hour window too');
});
