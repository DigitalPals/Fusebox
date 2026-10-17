const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Quota attribution in the dashboard, run from its own sources: points read as a share
// of the window, the last window of a kind, session lines, and the faults menu and badge.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const usageSource = fs.readFileSync(path.join(__dirname, '../ui/usage.js'), 'utf8');
const UsageTools = require('../ui/usage.js');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf('\n', at);
  return app.slice(at, end);
}

function load(S = { private: false }) {
  const context = { S, HIDDEN: '••••••••' };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('function quotaPercent(value) {'),
    piece('const sharePercent ='), piece('const shareKey ='), piece('const windowName ='), piece('const currentShare ='),
    piece('const previousShare ='), piece('function quotaClientName(c) {'), piece('function shareRowsHTML(s'),
    piece('function sessionQuotaText(a, s) {'), piece('const FAULT_RANK ='), piece('function faultItems(list) {'), piece('const badgeCount ='),
  ].join('\n'), context);
  return (code) => {
    const value = vm.runInContext(code, context);
    return value !== null && typeof value === 'object' ? JSON.parse(JSON.stringify(value)) : value;
  };
}

test('points read as a share of the window, never rounding onto 0% or 100%', () => {
  const run = load();
  assert.deepEqual(run('[18, 18.4, 0.3, 0, -2, 99.6, 100, 140, null, "7"].map(sharePercent)'),
    ['18%', '18%', '<1%', '0%', '0%', '>99%', '100%', '100%', '0%', '7%']);
});

test('windows are named and keyed as the server writes them', () => {
  const run = load();
  assert.equal(run('shareKey({ name: "week" })'), 'week');
  assert.equal(run('shareKey({ name: "week opus", model: "opus" })'), 'week opus · opus');
  assert.equal(run('windowName("week opus", "opus")'), 'week opus');
  assert.equal(run('windowName("week", "opus")'), 'week opus');
  assert.equal(run('windowName("5h", null)'), '5h');
});

test('the last window of a kind is the newest previous share with its key', () => {
  const run = load();
  const attribution = { current: [{ key: 'week' }], previous: [
    { key: '5h', resets_at: '2026-10-17T10:00:00Z' },
    { key: 'week', resets_at: '2026-10-14T10:00:00Z', used: 64 },
    { key: 'week', resets_at: '2026-10-07T10:00:00Z', used: 40 },
  ] };
  assert.equal(run(`previousShare(${JSON.stringify(attribution)}, 'week').used`), 64);
  assert.equal(run(`previousShare(${JSON.stringify(attribution)}, '5h').resets_at`), '2026-10-17T10:00:00Z');
  assert.equal(run(`previousShare(${JSON.stringify(attribution)}, 'week opus · opus')`), null);
  assert.equal(run('previousShare(undefined, "week")'), null);
  assert.equal(run('previousShare({ current: [] }, "week")'), null);
});

test('a window lists its clients, then use from outside Fusebox and from before it looked', () => {
  const share = { clients: [{ id: 'laptop', label: 'The laptop', points: 18.2 }, { id: 'app:Claude Code', label: 'Claude Code', points: 4 }, { id: '', label: 'Other requests', points: 0.4 }, { id: 'gone', label: 'Nobody', points: 0 }],
    outside: 6, before: 0 };
  const run = load();
  const html = run(`shareRowsHTML(${JSON.stringify(share)}, 'warn')`);
  assert.deepEqual([...html.matchAll(/ub-name" title="[^"]*">([^<]*)</g)].map((m) => m[1]), ['The laptop', 'Claude Code', 'Other requests', 'Outside Fusebox']);
  assert.match(html, /18%/);
  assert.match(html, /&lt;1%/);
  assert.match(html, /class="warn" style="width:18\.2%"/);
  assert.doesNotMatch(html, /Before Fusebox looked/, 'nothing before, no row');
  const many = { clients: Array.from({ length: 7 }, (_, i) => ({ id: `c${i}`, label: `Client ${i}`, points: 7 - i })), outside: 0, before: 3 };
  const rows = run(`shareRowsHTML(${JSON.stringify(many)})`);
  assert.match(rows, /2 more clients/);
  assert.match(rows, /Before Fusebox looked/);
  const hidden = load({ private: true })(`shareRowsHTML(${JSON.stringify(share)})`);
  assert.doesNotMatch(hidden, /The laptop/, 'named clients are hidden');
  assert.match(hidden, /Claude Code/, 'programs are not');
});

test('a session says what it used of each current window, in the account\'s order', () => {
  const run = load();
  const account = { quota_attribution: { current: [{ key: '5h', name: '5h' }, { key: 'week', name: 'week' }, { key: 'week opus · opus', name: 'week opus', model: 'opus' }] } };
  const text = (quota) => run(`sessionQuotaText(${JSON.stringify(account)}, { quota: ${JSON.stringify(quota)} })`);
  assert.equal(text({ week: 4.2, '5h': 11, 'week opus · opus': 0.2 }), '11% of 5h · 4% of week · <1% of week opus');
  assert.equal(text({ week: 0 }), '');
  assert.equal(text({}), '');
  assert.equal(run(`sessionQuotaText(${JSON.stringify(account)}, {})`), '');
});

test('pace warnings go last in the menu and never count on the app badge', () => {
  const run = load();
  const faults = [
    { kind: 'pace', level: 'warn', account_id: 'a', title: 'Weekly limit runs out early at this pace', detail: 'It would run out before it renews. The laptop used 61% of it.', badge: false },
    { kind: 'rate_limit', level: 'warn', account_id: 'b', title: 'Rate limited', badge: true },
    { kind: 'signin', level: 'err', account_id: 'c', title: 'Sign-in expired' },
    { kind: 'provider', level: 'err', title: 'Every Claude account is used up', badge: true },
    { kind: 'storage', level: 'warn', title: 'Usage database is large', detail: 'It is 900 MB and grows about 12 MB a day.', path: '#/usage', provider: '', provider_name: 'Usage', badge: true },
  ];
  const items = run(`faultItems(${JSON.stringify(faults)})`);
  assert.deepEqual(items.map((f) => f.id), ['c', 'b', null, 'a'], 'provider-wide faults stay out; the usage database\'s come in');
  assert.deepEqual(items.map((f) => f.badge), [true, true, true, false]);
  assert.equal(items[0].signin, true);
  assert.equal(items[2].path, '#/usage');
  assert.equal(items[2].provider, 'Usage');
  assert.equal(items[3].detail, faults[0].detail);
  assert.equal(run(`badgeCount(faultItems(${JSON.stringify(faults)}))`), 3);
  assert.equal(run('badgeCount([{ lvl: "err" }, { lvl: "warn" }])'), 2, 'faults worked out here have no badge flag and count');
  assert.equal(run('badgeCount([])'), 0);
});

function usage(extra = {}) {
  const context = { Intl, Date, URLSearchParams, AbortController, setTimeout() {}, S: { private: false, route: 'none', accounts: [] }, accountById: () => null, ...extra };
  vm.createContext(context);
  vm.runInContext(usageSource, context);
  return (code) => vm.runInContext(code, context);
}

test('the Collection drawer shows the usage database\'s size, growth and free disk', () => {
  assert.deepEqual([0, 512, 3400, 3.4e6, 128.4e6, 1.25e9, 12e9, null].map(UsageTools.bytes), ['0 B', '512 B', '3.4 KB', '3.4 MB', '128 MB', '1.3 GB', '12 GB', '0 B']);
  const run = usage();
  assert.equal(run('usageDatabaseHTML(null)'), '', 'nothing while the database is off');
  const db = { bytes: 845e6, live_bytes: 800e6, free_bytes: 45e6, observations: 1e6, bytes_per_observation: 845, growth_bytes_per_day: 12.5e6, disk_free_bytes: 38e9, compressing: true };
  let html = run(`usageDatabaseHTML(${JSON.stringify(db)})`);
  assert.match(html, /845 MB<\/b><span>Database/);
  assert.match(html, /13 MB<\/b><span>Growth a day/);
  assert.match(html, /38 GB<\/b><span>Free disk/);
  assert.match(html, /Compressing older records…/);
  html = run(`usageDatabaseHTML(${JSON.stringify({ ...db, compressing: false, disk_free_bytes: null })})`);
  assert.doesNotMatch(html, /Compressing/);
  assert.match(html, /Unknown<\/b><span>Free disk/);
  run(`Usage.drawerOpen = true; Usage.status = { health: {}, database: ${JSON.stringify(db)} }`);
  assert.match(run('usageDrawerHTML()'), /Growth a day/);
});
