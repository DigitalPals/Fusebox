const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');
const config = fs.readFileSync(path.join(__dirname, '../ui/config.js'), 'utf8');
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const esc = (s) => String(s ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
const policy = { mode: 'off', accounts: [], model: 'claude-haiku-4-5-20251001', timezone: 'Europe/Amsterdam', weekdays: [1, 2, 3, 4, 5], start: '08:00', end: '18:00' };
function piece(name) {
  const at = app.indexOf(name);
  assert.ok(at >= 0);
  return app.slice(at, app.indexOf('\n}\n', at) + 2);
}
function load(extra = {}) {
  const ctx = { S: { accounts: [], config: { values: { 'window-starter': structuredClone(policy) }, errors: {}, secrets: {} } },
    esc, acctLabel: (a) => a.label, when: (s) => s,
    windowOf: (a) => a.quota?.windows?.find((w) => w.name === '5h'),
    noShortLimit: (a) => !!a.quota?.no_five_hour_limit && !a.quota?.windows?.find((w) => w.name === '5h'),
    noShortNote: (a) => (ctx.noShortLimit(a) ? ' · no 5h limit' : ''),
    document: { addEventListener() {} }, ...extra };
  vm.createContext(ctx);
  vm.runInContext(config, ctx);
  vm.runInContext(piece('function windowStarterHTML'), ctx);
  vm.runInContext(piece('async function windowStarterAction'), ctx);
  return { ctx, run: (s) => vm.runInContext(s, ctx) };
}

test('settings default to off and explain quota use without silently selecting accounts', () => {
  const { run } = load();
  const html = run('configWindowStarterHTML()');
  assert.match(html, /aria-pressed="true">Off/);
  assert.match(html, /uses? a little subscription quota/i);
  assert.match(html, /does not increase weekly allowance/);
  assert.doesNotMatch(html, /data-config-act="window-account"/);
});

test('working hours expose timezone, weekdays, times and explicit safe account selection', () => {
  const { run } = load();
  run(`S.accounts = [{id:'a',label:'<name>',window_starter_supported:true},{id:'b',label:'API key',window_starter_supported:false}]; S.config.values['window-starter'].mode='working-hours'; S.config.values['window-starter'].accounts=['a','gone'];`);
  const html = run('configWindowStarterHTML()');
  assert.match(html, /data-id="a" checked/);
  assert.match(html, /&lt;name>/);
  assert.doesNotMatch(html, /data-id="b"/);
  assert.match(html, /Unavailable account · gone/);
  assert.match(html, /Working hours timezone/);
  assert.doesNotMatch(html, /type="time"/, "browser time pickers can show AM/PM");
  assert.match(html, /placeholder="09:00"/);
  assert.match(html, /24-hour clock/);
  assert.equal((html.match(/data-config-act="window-day"/g) || []).length, 7);
  assert.match(html, /data-day="6" aria-pressed="false"/);
  assert.match(html, /No selection means no automatic requests/);
});

test('manual action remains available with automation off and holds only offer status reads', () => {
  const { run } = load();
  run(`var a={id:'a',window_starter_supported:true,window_starter_selected:false}`);
  assert.match(run('windowStarterHTML(a)'), /data-act="window-start"/);
  assert.match(run('windowStarterHTML(a)'), /uses subscription quota/);
  run(`a.window_starter={status:'unknown',retry_after:'2099-01-01T00:00:00Z',message:'Waiting for confirmation'}`);
  assert.match(run('windowStarterHTML(a)'), /data-act="window-check"/);
  assert.doesNotMatch(run('windowStarterHTML(a)'), /data-act="window-start"/);
  run(`a.disabled=true`);
  assert.match(run('windowStarterHTML(a)'), /disabled/);
  run('a.window_starter_supported=false');
  assert.equal(run('windowStarterHTML(a)'), '');
});

test('active windows use the provider reset time and unavailable messages are escaped', () => {
  const { run } = load();
  run(`var a={id:'a',window_starter_supported:true,quota:{windows:[{name:'5h',resets_at:'2099-01-01T00:00:00Z'}]},window_starter:{status:'unavailable',message:'<unsafe>'}}`);
  assert.match(run('windowStarterHTML(a)'), /Window resets 2099-01-01T00:00:00Z/);
  assert.match(run('windowStarterHTML(a)'), /data-act="window-check"/);
  run('a.quota=null');
  assert.match(run('windowStarterHTML(a)'), /&lt;unsafe>/);
});

test('accounts without a 5-hour limit have no starter; settings say why', () => {
  const { run } = load();
  run(`var a={id:'a',window_starter_supported:true,window_starter_selected:true,quota:{no_five_hour_limit:true,windows:[]}}`);
  assert.equal(run('windowStarterHTML(a)'), '');
  run(`a.quota=null; a.window_starter={status:'not_needed',message:'No limit here',retry_after:'2099-01-01T00:00:00Z'}`);
  assert.equal(run('windowStarterHTML(a)'), '');
  // Still selectable: the flag can clear again, and the selection should survive that.
  run(`S.accounts = [{id:'a',label:'ultra',window_starter_supported:true,quota:{no_five_hour_limit:true,windows:[]}},{id:'b',label:'max',window_starter_supported:true}];
    S.config.values['window-starter'].mode='always'; S.config.values['window-starter'].accounts=['a'];`);
  const html = run('configWindowStarterHTML()');
  assert.match(html, /data-id="a" checked><span>ultra · no 5h limit</);
  assert.match(html, /data-id="b" ><span>max</);
  assert.doesNotMatch(html, /Unavailable account/);
});

test('a 5-hour limit reported since the last starter check overrides its not_needed result', () => {
  const { run } = load();
  run(`var a={id:'a',window_starter_supported:true,window_starter_selected:true,
    quota:{no_five_hour_limit:false,windows:[{name:'5h',used:0,resets_at:null}]},
    window_starter:{status:'not_needed',message:'This account has no five-hour limit, so there is no window to start'}}`);
  const html = run('windowStarterHTML(a)');
  assert.doesNotMatch(html, /This account has no five-hour limit/);
  assert.match(html, /data-act="window-start"/);
});

test('manual requests carry a unique ID and status checks never ask for generation', async () => {
  const calls = [];
  const a = { id: 'account/a', window_starter_supported: true };
  const { ctx, run } = load({ accountById: () => a, crypto: require('node:crypto').webcrypto,
    refreshViews() {}, refreshAccounts: async () => {}, toast() {},
    api: async (url, opts) => { calls.push({ url, body: JSON.parse(opts.body) }); return { status: 'inactive', message: 'Inactive' }; },
  });
  await run(`windowStarterAction('account/a', true)`);
  await run(`windowStarterAction('account/a', false)`);
  assert.equal(calls[0].url, '/accounts/account%2Fa/window-starter');
  assert.equal(calls[0].body.check_only, true);
  assert.equal(calls[1].body.check_only, false);
  assert.notEqual(calls[0].body.request_id, calls[1].body.request_id);
  assert.equal(ctx.S.windowStarterBusy['account/a'], undefined);
  assert.equal(a.last_error, undefined);
});
