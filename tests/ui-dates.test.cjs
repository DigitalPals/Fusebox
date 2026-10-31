const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');
const { execFileSync } = require('node:child_process');

const usage = fs.readFileSync(path.join(__dirname, '../ui/usage.js'), 'utf8');
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start, end) {
  const at = app.indexOf(start), to = app.indexOf(end, at);
  assert.ok(at >= 0 && to > at);
  return app.slice(at, to);
}
const code = usage + '\n' + piece('const clock =', '// Within a day') + '\n'
  + piece('const dayLabel =', '// One row per client') + '\n'
  + piece('const minuteLabels =', 'function barHTML(');

test('shared local date formats retain native output across locales, DST and invalid values', () => {
  const script = `
    const vm = require('node:vm'), assert = require('node:assert/strict');
    const context = { Intl, Date, URLSearchParams };
    vm.createContext(context);
    vm.runInContext(${JSON.stringify(code)}, context);
    const run = (source) => vm.runInContext(source, context);
    for (const zone of ['UTC', 'Europe/Amsterdam', 'America/Los_Angeles', 'Asia/Kathmandu']) {
      process.env.TZ = zone;
      run('syncLocalTime()');
      for (const literal of ['"2026-03-29T00:30:00Z"', '"2026-03-29T01:30:00Z"', '"2026-10-25T00:30:00Z"', '"2026-10-25T01:30:00Z"',
        '"2026-10-31T23:59:59Z"', '"2026-11-01T00:00:00Z"', '-1', '1', '0', 'null', 'undefined', '"bad date"', 'NaN', 'Infinity', '8640000000000001']) {
        const at = eval('new Date(' + literal + ')');
        assert.equal(run('clock(' + literal + ')'), at.toLocaleTimeString('en-GB', { hour12: false }));
        assert.equal(run('hm(' + literal + ')'), at.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false }));
        assert.equal(run('weekday(' + literal + ')'), at.toLocaleDateString('en-GB', { weekday: 'short' }));
        assert.equal(run('fullResetTime(' + literal + ')'), at.toLocaleString('en-GB', {
          weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false, timeZoneName: 'short',
        }));
        assert.equal(run('dayLabel(' + literal + ')'), at.toLocaleDateString('en-GB', { weekday: 'short' }) + ' ' + at.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }));
      }
    }
  `;
  for (const locale of ['en_US.UTF-8', 'en_GB.UTF-8', 'nl_NL.UTF-8', 'zh_CN.UTF-8']) {
    execFileSync(process.execPath, ['-e', script], { env: { ...process.env, LANG: locale, LC_ALL: locale, TZ: 'UTC' }, encoding: 'utf8' });
  }
});

test('warm request and quota dates reuse formatters between timezone boundary checks', () => {
  let constructed = 0;
  const context = { Date, URLSearchParams, Intl: { NumberFormat: Intl.NumberFormat, DateTimeFormat: function (...args) {
    constructed++; return new Intl.DateTimeFormat(...args);
  } } };
  vm.createContext(context);
  vm.runInContext(code, context);
  const run = (source) => vm.runInContext(source, context);
  run('syncLocalTime(); clock(1); hm(1); weekday(1); fullResetTime(1); dayLabel(1)');
  const warm = constructed;
  run('for (let i = 0; i < 600; i++) { clock(i); hm(i); weekday(i); fullResetTime(i); dayLabel(i); }');
  assert.equal(constructed, warm, 'rows and tooltips create no formatters after warmup');
  run('syncLocalTime()');
  assert.equal(constructed, warm + 1, 'one timezone lookup serves the whole render or timer sweep');
});

test('timezone boundary checks invalidate local formatter and minute-label caches', () => {
  const script = `
    const vm = require('node:vm'), assert = require('node:assert/strict');
    const context = { Intl, Date, URLSearchParams };
    vm.createContext(context);
    vm.runInContext(${JSON.stringify(code)}, context);
    const run = (source) => vm.runInContext(source, context);
    const minute = Math.floor(Date.parse('2026-10-31T12:34:00Z') / 60000);
    for (const zone of ['UTC', 'Europe/Amsterdam', 'America/Los_Angeles', 'UTC']) {
      process.env.TZ = zone;
      run('syncLocalTime()');
      assert.equal(run('minuteLabel(' + minute + ')'), new Date(minute * 60000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' }));
      assert.equal(run('clock(' + minute * 60000 + ')'), new Date(minute * 60000).toLocaleTimeString('en-GB', { hour12: false }));
    }
  `;
  execFileSync(process.execPath, ['-e', script], { env: { ...process.env, TZ: 'UTC' }, encoding: 'utf8' });
});
