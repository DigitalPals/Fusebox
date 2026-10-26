const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = source.indexOf(start);
  assert.ok(at >= 0, start);
  return source.slice(at, source.indexOf('\n}\n', at) + 2);
}
function dashboard() {
  const a = { id: 'file:claude-user@example.test.json', label: 'Test account', provider: 'claude',
    last_error: '400: Third-party apps now draw from your extra usage, not your plan limits.',
    last_error_id: 'occurrence-1', last_error_until: null, cooldowns: {}, counters: { failures: 2 } };
  const calls = [];
  let resolve, reject;
  const pending = new Promise((yes, no) => { resolve = yes; reject = no; });
  const context = {
    S: { accounts: [a], errorClears: {} },
    esc: (s) => String(s ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;'),
    hideEmails: (s) => s, acctLabel: (a) => a.label,
    accountById: (id) => context.S.accounts.find((a) => a.id === id),
    api: (...args) => { calls.push(args); return pending; },
    refreshViews: () => calls.push(['render']), refreshAccounts: () => calls.push(['refresh']),
    toast: (s) => calls.push(['toast', s]),
  };
  vm.createContext(context);
  vm.runInContext(piece('function accountErrorHTML(') + '\n' + piece('async function clearAccountError('), context);
  return { a, calls, context, resolve, reject, render: () => context.accountErrorHTML(context.S.accounts[0]), clear: () => context.clearAccountError(a.id) };
}

test('an error without cooldowns has an accessible clear button and a fixed billing link', () => {
  const d = dashboard();
  assert.match(d.render(), /data-act="clear-error"/);
  assert.match(d.render(), /aria-label="Clear error for Test account"/);
  assert.match(d.render(), /https:\/\/claude.ai\/settings\/usage/);
  assert.doesNotMatch(d.render(), /disabled/);
  d.a.last_error = '<img src=x onerror=alert(1)>';
  assert.match(d.render(), /&lt;img/);
  assert.doesNotMatch(d.render(), /Manage extra usage/);
  d.a.last_error = null;
  assert.equal(d.render(), '');
});

test('clear waits for success, prevents double submission, and keeps counters and cooldowns', async () => {
  const d = dashboard();
  d.a.cooldowns['*'] = '2099-01-01T00:00:00Z';
  const operation = d.clear();
  assert.match(d.render(), /aria-busy="true"/);
  assert.match(d.render(), /disabled>Clearing…/);
  assert.equal(d.a.last_error_id, 'occurrence-1');
  assert.equal(d.calls.filter(([name]) => name === 'toast').length, 0);
  await d.clear();
  const posts = d.calls.filter(([name]) => name.startsWith('/accounts/'));
  assert.equal(posts.length, 1);
  assert.equal(posts[0][0], '/accounts/file%3Aclaude-user%40example.test.json/error/clear');
  assert.deepEqual(JSON.parse(posts[0][1].body), { error_id: 'occurrence-1' });
  d.resolve({ last_error: null, last_error_id: null, last_error_until: null });
  await operation;
  assert.equal(d.render(), '');
  assert.deepEqual(d.a.cooldowns, { '*': '2099-01-01T00:00:00Z' });
  assert.deepEqual(d.a.counters, { failures: 2 });
  assert.ok(d.calls.some(([name, value]) => name === 'toast' && value === 'Error cleared'));
});

test('a failed save keeps the provider error and shows the action failure separately', async () => {
  const d = dashboard();
  const error = d.a.last_error;
  const operation = d.clear();
  d.reject(new Error('network unavailable'));
  await operation;
  assert.equal(d.a.last_error, error);
  assert.equal(d.a.last_error_id, 'occurrence-1');
  assert.match(d.render(), /role="alert">Could not clear error: network unavailable/);
  assert.doesNotMatch(d.render(), /disabled>Clearing…/);
  assert.ok(!d.calls.some(([name]) => name === 'toast'));
});

test('a stale clear explains the conflict and never retries the newer error', async () => {
  const d = dashboard();
  const operation = d.clear();
  d.a.last_error_id = 'occurrence-2';
  d.reject(Object.assign(new Error('A newer error arrived. Review it before clearing.'), { status: 409 }));
  await operation;
  assert.equal(d.a.last_error_id, 'occurrence-2');
  assert.match(d.render(), /A newer error arrived/);
  assert.equal(d.calls.filter(([name]) => name.startsWith('/accounts/')).length, 1);
  assert.ok(d.calls.some(([name]) => name === 'refresh'));
});

test('a successful response cannot overwrite a newer account snapshot received while waiting', async () => {
  const d = dashboard();
  const operation = d.clear();
  d.context.S.accounts = [{ ...d.a, last_error_id: 'occurrence-2', last_error: '500: newer failure' }];
  d.resolve({ last_error: null, last_error_id: null, last_error_until: null });
  await operation;
  assert.equal(d.context.S.accounts[0].last_error_id, 'occurrence-2');
  assert.match(d.render(), /500: newer failure/);
});
