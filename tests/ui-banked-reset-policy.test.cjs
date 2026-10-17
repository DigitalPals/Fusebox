const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function load() {
  const context = {
    S: { overview: { banked_resets: true }, resets: {} },
    esc: (s) => String(s ?? '').replaceAll('<', '&lt;'), acctLabel: (a) => a.label,
    provName: () => 'Claude', liveAgo: () => 'just now',
  };
  vm.createContext(context);
  vm.runInContext(source.slice(source.indexOf('function hasBankedResets'), source.indexOf('\nfunction syncResetModal')), context);
  return (s) => vm.runInContext(s, context);
}
const account = {
  id: 'demo', label: 'demo@test.invalid', provider: 'claude', kind: 'oauth', disabled: false,
  banked_reset_plan: { prioritized: true, auto_redeem: true, message: 'Automatic reset at 95% weekly use.' },
  banked_resets: {
    checked_at: new Date().toISOString(), quote: 'quote', error: null,
    inventory: { available: 1, applicable: 1, eligible: true, grants: [] },
  },
};

test('expiring reset status explains the automatic policy and priority', () => {
  const run = load();
  run(`var a=${JSON.stringify(account)}`);
  assert.match(run('bankedLineHTML(a)'), /prioritized/);
  const html = run('bankedResetsHTML(a)');
  assert.match(html, /Prioritized for new work/);
  assert.match(html, /Automatic reset at 95% weekly use/);
  assert.doesNotMatch(html, /Always asks/);
  run('a.banked_resets.operation={automatic:true,status:"applied",message:"One banked reset applied"}');
  assert.match(run('bankedResetsHTML(a)'), /requested automatically/);
});

test('manual use still asks for confirmation when automation is enabled', () => {
  const run = load();
  run(`var a=${JSON.stringify(account)}; S.resets.demo={dialog:{action:'redeem',grant:'demo'}}`);
  const html = run('bankedResetsHTML(a)');
  assert.match(html, /This uses one saved reset/);
  assert.match(html, /Apply reset/);
  assert.match(html, /data-act="banked-confirm"/);
});

test('missing plans describe manual behavior, and unresolved resets remain reviewable', () => {
  const run = load();
  run(`var a=${JSON.stringify(account)}; a.banked_reset_plan=null`);
  assert.match(run('bankedPolicyText(a)'), /Manual use asks for confirmation/);
  run('a.banked_resets.operation={status:"unknown",automatic:true}; a.banked_resets.inventory.available=0');
  assert.match(run('bankedLineHTML(a)'), /Review reset/);
  assert.match(run('bankedResetsHTML(a)'), /New resets are blocked until resolved/);
});
