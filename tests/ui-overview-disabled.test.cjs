const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The overview leaves disabled accounts out of its subscriptions, their "ready" count and
// its other circuits; the Accounts page still lists them.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  assert.ok(end > at, `app.js no longer ends ${start}`);
  return app.slice(at, end);
}

function load(accounts) {
  const context = {
    S: { accounts, load: false },
    mob: () => false, fmt: String, logo: () => '', acctLogo: () => '', provName: (a) => a.provider, acctLabel: (a) => a.label,
    planName: () => 'Max', authName: () => '', noShortNote: () => '', scopedWindows: () => [], metersHTML: () => '',
    statusHTML: () => '', bankedLineHTML: () => '', renewalLineHTML: () => '', nowHTML: () => '', scopedRowsHTML: () => '',
    quotaControlsHTML: () => '', quotaWord: () => 'left', routingSentence: () => 'smart routing', inFlight: () => 0,
    acctState: (a) => (a.disabled ? { cls: 'disabled', dot: 'idle' } : { cls: 'ready', dot: 'ok' }), statusWord: () => 'Ready',
  };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('const metered ='), piece('const ovAccounts ='),
    piece('function subsMeta(short'), piece('function ovSubsHTML() {'), piece('function ovOtherHTML() {'),
  ].join('\n'), context);
  return (code) => vm.runInContext(code, context);
}

const acct = (id, provider, kind, disabled = false) => ({ id, label: id, provider, kind, disabled, counters: { requests: 1 } });

test('disabled accounts are left off the overview', () => {
  const run = load([
    acct('sub-on', 'claude', 'oauth'), acct('sub-off', 'claude', 'oauth', true),
    acct('key-on', 'openai', 'api_key'), acct('key-off', 'openai', 'api_key', true),
  ]);
  const subs = run('ovSubsHTML()');
  assert.match(subs, /data-id="sub-on"/);
  assert.doesNotMatch(subs, /sub-off/);
  assert.equal(run('subsMeta()'), '1 of 1 ready · smart routing');
  const other = run('ovOtherHTML()');
  assert.match(other, /data-id="key-on"/);
  assert.doesNotMatch(other, /key-off/);
});

test('sections disappear when every account in them is disabled', () => {
  const run = load([acct('sub-off', 'claude', 'oauth', true), acct('key-off', 'openai', 'api_key', true)]);
  assert.equal(run('ovSubsHTML()'), '');
  assert.equal(run('ovOtherHTML()'), '');
});

test('the sign-in prompt shows only when no account exists at all', () => {
  assert.match(load([])('ovSubsHTML()'), /No accounts connected/);
});
