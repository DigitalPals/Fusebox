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
    + `${piece('function renewalOf(', '// The renewal row, with a way')}\n${piece('function renewalLineHTML(', 'function detSigninHTML(')}\n`
    + 'this.renewalRow = renewalRow; this.renewalLineHTML = renewalLineHTML;', context);
  context.esc = (v) => String(v);
  return Object.assign(context.renewalRow, { line: context.renewalLineHTML });
}

test('the sign-in card says when a subscription charges next, or ends', () => {
  process.env.TZ = 'Europe/Amsterdam';
  const row = load(Date.parse('2026-11-02T12:00:00Z'));
  const quota = (renewal) => ({ quota: { renewal } });
  assert.deepEqual([...row(quota({ at: '2026-11-06T09:53:56Z', estimated: false, ends: false, period: 'monthly' }))],
    ['Renews', 'Fri 6 Nov, 10:53 · in 3d 21h · monthly']);
  assert.deepEqual([...row(quota({ at: '2026-11-04T05:42:21Z', estimated: true, ends: false, period: null }))],
    ['Renews', 'about Wed 4 Nov · in 1d 17h · estimated']);
  assert.deepEqual([...row(quota({ at: '2026-11-06T09:53:56Z', estimated: false, ends: true }))],
    ['Ends', 'Fri 6 Nov · in 3d 21h · cancelled']);
  for (const a of [{}, quota(null), quota({ at: '2026-11-01T00:00:00Z' }), quota({ at: 'soon' })]) assert.equal(row(a), null);
});

test('a billing day set by hand is a day: shown until it is over, without a time', () => {
  process.env.TZ = 'Europe/Amsterdam';
  // 4 November 2026, 15:00 in Amsterdam; the server keeps a day set by hand as its 09:00.
  const row = load(Date.parse('2026-11-04T14:00:00Z'));
  const manual = (at, extra = {}) => ({ quota: { renewal: { at, estimated: false, ends: false, period: 'monthly', manual: true, ...extra } } });
  assert.deepEqual([...row(manual('2026-11-23T08:00:00Z'))], ['Renews', 'Mon 23 Nov · in 19 days · monthly']);
  assert.deepEqual([...row(manual('2026-11-05T08:00:00Z'))], ['Renews', 'Thu 5 Nov · tomorrow · monthly']);
  assert.deepEqual([...row(manual('2026-11-04T08:00:00Z'))], ['Renews', 'Wed 4 Nov · today · monthly'], 'its morning passed, the day did not');
  assert.deepEqual([...row(manual('2027-01-15T08:00:00Z', { period: 'yearly' }))], ['Renews', 'Fri 15 Jan · in 72 days · yearly']);
  assert.deepEqual([...row(manual('2026-11-23T08:00:00Z', { estimated: true }))], ['Renews', 'Mon 23 Nov · in 19 days · monthly · plan changed, check the date']);
  assert.deepEqual([...row(manual('2026-11-23T08:00:00Z', { ends: true }))], ['Ends', 'Mon 23 Nov · in 19 days · cancelled']);
  assert.equal(row(manual('2026-11-03T08:00:00Z')), null, 'a day that is over');
  // Once a cancelled subscription has ended the server sends no renewal; the file's date says when.
  assert.deepEqual([...row({ quota: {}, billing: { date: '2026-11-01', ends: true } })], ['Ended', 'Sun 1 Nov · cancelled']);
  // The overview's line comes in the last days only.
  assert.match(row.line(manual('2026-11-05T08:00:00Z')), />Renews tomorrow</);
  assert.match(row.line(manual('2026-11-06T08:00:00Z', { ends: true })), />Ends in 2 days</);
  assert.equal(row.line(manual('2026-11-07T08:00:00Z')), '');
  assert.equal(row.line({}), '');
});
