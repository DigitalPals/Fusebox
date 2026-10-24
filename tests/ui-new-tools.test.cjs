const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The dashboard's newer tools, run from its own sources: the config diff shown before a
// save, webhook destinations, client limit summaries and quota pace.
const config = fs.readFileSync(path.join(__dirname, '../ui/config.js'), 'utf8');
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
// A whole top-level function or one-line const from app.js; fails loudly if renamed.
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf('\n', at);
  return app.slice(at, end);
}

function load(extra = {}) {
  const context = {
    S: { private: false, quotaDisplay: 'used', config: { values: {}, busy: false, clientUsage: null } },
    ICON: { copy: '' }, document: { addEventListener() {} }, crypto: globalThis.crypto, URL, ...extra,
  };
  vm.createContext(context);
  const helpers = [piece('const esc = (v) =>'), piece('function fmt(n) {'), piece('const plural = (n, word'), piece('function quotaOf(w, now'),
    piece('function windowSeconds(w) {'), piece('function windowElapsed(w, now')].join('\n');
  vm.runInContext(`${helpers}\n${config}`, context);
  // Values cross back through JSON, so they compare like this realm's own.
  return (code) => {
    const value = vm.runInContext(code, context);
    return value !== null && typeof value === 'object' ? JSON.parse(JSON.stringify(value)) : value;
  };
}

test('the review shows added and removed lines with a little context and folds the rest', () => {
  const run = load();
  const before = Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n');
  const after = before.replace('line 10', 'line ten').replace('line 15\n', '');
  const ops = run(`lineDiff(${JSON.stringify(before)}, ${JSON.stringify(after)})`);
  assert.deepEqual(ops.filter(([t]) => t !== ' '), [['-', 'line 10'], ['+', 'line ten'], ['-', 'line 15']]);
  const html = run(`diffHTML(${JSON.stringify(ops)})`);
  assert.match(html, /⋯ 7 unchanged lines/);
  assert.match(html, /class="dl add"><b aria-hidden="true">\+<\/b>line ten/);
  assert.ok(!html.includes('line 2<'), 'far context is folded away');
  // Identical texts have nothing to show; whole-file rewrites still diff.
  assert.equal(run(`lineDiff('a\\nb', 'a\\nb')`).filter(([t]) => t !== ' ').length, 0);
  assert.deepEqual(run(`lineDiff('', 'x')`).filter(([t]) => t !== ' '), [['-', ''], ['+', 'x']]);
});

test('webhook URLs say where they deliver, by host only', () => {
  const run = load();
  const kind = (url) => run(`webhookKind(${JSON.stringify(url)})`);
  assert.equal(kind('https://discord.com/api/webhooks/1/secret'), 'Discord');
  assert.equal(kind('https://hooks.slack.com/services/T/B/X'), 'Slack');
  assert.equal(kind('https://prod-1.westus.logic.azure.com/workflows/x'), 'Teams');
  assert.equal(kind('https://api.telegram.org/bot1:x/sendMessage?chat_id=2'), 'Telegram');
  assert.equal(kind('https://ntfy.sh/alerts'), 'ntfy');
  assert.equal(kind('https://chat.example.com/hooks/x'), 'Webhook');
  assert.equal(kind('not a url'), null);
  assert.equal(run(`webhookHost('https://discord.com/api/webhooks/1/secret')`), 'discord.com');
});

test('client limits read as one line, with what was used when it is known', () => {
  const run = load();
  const client = { id: 'laptop', label: 'Laptop', key: 'k', 'requests-per-minute': 60, 'tokens-per-day': 5000000, 'usd-per-month': 100, models: ['claude-*'], 'user-agents': ['claude-cli/*'] };
  const text = run(`namedLimitsText(${JSON.stringify(client)})`);
  assert.match(text, /^60 a minute · 5M tokens today · \$100 this month · claude-\* · from claude-cli\/\*$/);
  run(`S.config.clientUsage = { clients: { laptop: { tokens_today: 1200000, usd_month: 3.1 } } }`);
  assert.match(run(`namedLimitsText(${JSON.stringify(client)})`), /1\.2M of 5M tokens today · \$3\.10 of \$100 this month/);
  assert.match(run(`namedLimitsText({ id: 'x', expires: '2001-01-31' })`), /^expired /);
  assert.equal(run(`namedLimitsText({ id: 'x' })`), '');
});

test('the pace mark follows elapsed time for known usage windows', () => {
  const run = load();
  const now = Date.parse('2026-10-13T12:00:00Z');
  const resetsIn = (h) => new Date(now + h * 3600e3).toISOString();
  const fast = { name: '5h', used: 80, resets_at: resetsIn(2.5) };
  assert.equal(run(`windowElapsed(${JSON.stringify(fast)}, ${now})`), 0.5);
  assert.equal(run(`windowElapsed(${JSON.stringify({ name: 'week', used: 10, resets_at: resetsIn(84) })}, ${now})`), 0.5);
  assert.equal(run(`windowElapsed(${JSON.stringify({ name: 'month', used: 10, resets_at: resetsIn(1) })}, ${now})`), null);
});
