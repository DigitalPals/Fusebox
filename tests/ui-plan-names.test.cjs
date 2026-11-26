const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The plan names the account lists show.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  assert.ok(end > at, `app.js no longer ends ${start}`);
  return app.slice(at, end);
}

function load() {
  const context = {};
  vm.createContext(context);
  vm.runInContext([
    piece('const cap = (s)'), piece('const CODEX_PLAN_NAMES ='), piece('const CLAUDE_PLAN_NAMES ='), piece('const planName = (a)'),
  ].join('\n'), context);
  return (code) => vm.runInContext(code, context);
}

test('Codex Pro labels use the active SKU and fall back to the broad plan for unknown tiers', () => {
  const run = load();
  const account = (sku, provider = 'codex') => ({ provider, quota: { plan: 'pro', subscription_plan: sku } });
  for (const [sku, label] of [['chatgptprolite', 'Pro 100'], ['chatgptpro', 'Pro 200'], ['chatgptpromax', 'Pro 500']]) {
    assert.equal(run(`planName(${JSON.stringify(account(sku))})`), label);
  }
  for (const sku of [undefined, '', 'future-pro-plan', 'chatgptpro-20x', 'constructor', '__proto__']) {
    assert.equal(run(`planName(${JSON.stringify(account(sku))})`), 'Pro', 'unknown details do not guess a price or multiplier');
  }
  assert.equal(run(`planName(${JSON.stringify(account('chatgptpro', 'claude'))})`), 'Pro', 'Codex SKUs do not relabel Claude');
  assert.equal(run('planName({ provider: "codex" })'), '');
});

test('Claude plans show the provider’s name, and the renamed Max tiers their new names', () => {
  const run = load();
  const claude = (quota) => run(`planName(${JSON.stringify({ provider: 'claude', quota })})`);
  assert.equal(claude({ plan: 'max_20x' }), 'Ultra');
  assert.equal(claude({ plan: 'max_5x' }), 'Max');
  assert.equal(claude({ plan: 'max_20x', plan_name: 'Ultra Plus' }), 'Ultra Plus', 'the profile’s own name wins');
  assert.equal(claude({ plan: 'pro' }), 'Pro');
  assert.equal(claude({ plan: 'max' }), 'Max', 'Max without a multiple stays as written');
  assert.equal(run(`planName(${JSON.stringify({ provider: 'codex', quota: { plan: 'max_20x' } })})`), 'Max 20x', 'only Claude is renamed');
});

test('Meta plans show the mint’s tier name when Meta sends one', () => {
  const run = load();
  const meta = (quota) => run(`planName(${JSON.stringify({ provider: 'meta', quota })})`);
  assert.equal(meta({ plan_name: 'Muse Code Max Usage' }), 'Muse Code Max Usage');
  assert.equal(meta({ windows: [] }), '', 'no tier name, no plan');
  assert.equal(meta({}), '');
});
