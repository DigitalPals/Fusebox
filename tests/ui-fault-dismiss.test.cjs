const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Dismissing faults from the faults menu, run from the dashboard's own sources: each of the
// server's faults has a dismiss button, dismissing hides it at once and tells the server,
// and the menu offers the dismissed ones back.
const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = source.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') || start.startsWith('async function') ? source.indexOf('\n}\n', at) + 2 : source.indexOf('\n', at);
  assert.ok(end > at, `app.js no longer ends ${start}`);
  return source.slice(at, end);
}

function dashboard() {
  const layer = { innerHTML: '' };
  const calls = [];
  const context = {
    S: { faultList: null, dismissedFaults: 0, faults: true, private: false },
    ICON: { x: '<svg></svg>' },
    esc: (s) => String(s ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;'),
    $: (sel) => (sel === '#lay-faults' ? layer : null),
    $$: () => [],
    mob: () => true,
    innerWidth: 400,
    accountById: () => null,
    provName: () => 'Claude', acctLabel: () => 'a', who: (s) => s,
    api: async (...args) => { calls.push(args); return { ok: true }; },
    toast: (s) => calls.push(['toast', s]),
  };
  context.faultsList = () => context.S.faultList || [];
  context.renderFaults = () => context.renderFaultsMenu();
  vm.createContext(context);
  vm.runInContext([
    piece('const FAULT_RANK ='), piece('function faultItems(list) {'), piece('function onFaults(list, dismissed) {'),
    piece('function renderFaultsMenu('), piece('async function dismissFault('), piece('async function restoreFaults('),
  ].join('\n'), context);
  return { context, calls, menu: () => layer.innerHTML };
}

const faults = [
  { key: 'signin:c', kind: 'signin', level: 'err', account_id: 'c', title: 'Sign-in expired', provider_name: 'Claude', label: 'c' },
  { key: 'rate:b', kind: 'rate_limit', level: 'warn', account_id: 'b', title: 'Rate limited', provider_name: 'Claude', label: 'b' },
];

test('every fault in the menu can be dismissed, and dismissed ones can be shown again', async () => {
  const d = dashboard();
  d.context.S.route = 'none';
  d.context.onFaults(faults, []);
  assert.equal((d.menu().match(/data-act="fault-dismiss"/g) || []).length, 2);
  assert.match(d.menu(), /data-id="rate:b" aria-label="Dismiss Rate limited"/);
  assert.doesNotMatch(d.menu(), /faults-restore/, 'nothing dismissed, nothing to show again');

  await d.context.dismissFault('rate:b');
  assert.deepEqual(JSON.parse(JSON.stringify(d.calls[0])), ['/faults/dismiss', { method: 'POST', body: '{"key":"rate:b"}' }]);
  assert.doesNotMatch(d.menu(), /Rate limited/, 'gone from the menu before the server answers');
  assert.match(d.menu(), /1 dismissed until it clears/);
  assert.match(d.menu(), /data-act="faults-restore"/);

  // The live list confirms it.
  d.context.onFaults([faults[0]], [faults[1]]);
  assert.match(d.menu(), /1 dismissed until it clears/);
  d.context.onFaults([], faults);
  assert.match(d.menu(), /Nothing else tripped\./);
  assert.match(d.menu(), /2 dismissed until they clear/);

  await d.context.restoreFaults();
  assert.deepEqual(JSON.parse(JSON.stringify(d.calls[1])), ['/faults/restore', { method: 'POST', body: '{}' }]);
  assert.doesNotMatch(d.menu(), /dismissed/);
});

test('faults worked out in the browser, before the server\'s list, have nothing to dismiss', () => {
  const d = dashboard();
  d.context.renderFaultsMenu([{ lvl: 'err', id: 'a', title: 'Account error' }]);
  assert.doesNotMatch(d.menu(), /fault-dismiss/);
  assert.match(d.menu(), /Account error/);
});
