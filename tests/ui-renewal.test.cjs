const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const usage = fs.readFileSync(path.join(__dirname, '../ui/usage.js'), 'utf8');
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start, end) {
  const at = app.indexOf(start), to = app.indexOf(end, at);
  assert.ok(at >= 0 && to > at, start);
  return app.slice(at, to);
}

function load(now) {
  const RealDate = Date;
  class FixedDate extends RealDate {
    constructor(...args) { super(...(args.length ? args : [now])); }
    static now() { return now; }
  }
  const context = { Intl, Date: FixedDate, URLSearchParams };
  vm.createContext(context);
  vm.runInContext(`${usage}\n${piece('function left(', '// Compact quota countdown')}\n${piece('const clock =', '// Within a day')}\n`
    + `${piece('function renewalRow(', 'function detSigninHTML(')}\nthis.renewalRow = renewalRow;`, context);
  return context.renewalRow;
}

test('the sign-in card says when a subscription charges next, or ends', () => {
  process.env.TZ = 'Europe/Amsterdam';
  const row = load(Date.parse('2026-11-02T12:00:00Z'));
  const quota = (renewal) => ({ quota: { renewal } });
  assert.deepEqual([...row(quota({ at: '2026-11-06T09:53:56Z', estimated: false, ends: false, period: 'monthly' }))],
    ['Renews', 'Fri 6 Nov, 10:53 · in 3d 21h · monthly']);
  assert.deepEqual([...row(quota({ at: '2026-11-04T05:42:21Z', estimated: true, ends: false, period: null }))],
    ['Renews', 'about Wed 4 Nov · in 1d 17h · estimated from the start date']);
  assert.deepEqual([...row(quota({ at: '2026-11-06T09:53:56Z', estimated: false, ends: true }))],
    ['Ends', 'Fri 6 Nov · in 3d 21h · cancelled']);
  for (const a of [{}, quota(null), quota({ at: '2026-11-01T00:00:00Z' }), quota({ at: 'soon' })]) assert.equal(row(a), null);
});
