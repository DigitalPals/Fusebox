const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// An account's meter rows and window blocks, run from their own sources. Rows always
// count down to the provider's reset, including when usage would run out sooner.
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `app.js no longer has ${start}`);
  const end = start.startsWith('function') ? app.indexOf('\n}\n', at) + 2 : app.indexOf(';\n', at) + 1;
  assert.ok(end > at, `app.js no longer ends ${start}`);
  return app.slice(at, end);
}

// Production on 2026-10-23: 5-hour window 99% used, renewing in 2h 58m.
const NOW = Date.parse('2026-10-23T13:42:00Z');
const RESET = '2026-10-23T16:40:00Z';
function load() {
  const context = { S: { quotaDisplay: 'remaining' }, Date: class extends Date { static now() { return NOW; } }, UsageTools: require('../ui/usage.js'), usedByHTML: () => '' };
  vm.createContext(context);
  vm.runInContext([
    piece('const esc = (v) =>'), piece('function left(iso) {'), piece('function compactLeft(iso) {'),
    piece('const hm ='), piece('const weekday ='), piece('const fullResetTime ='), piece('const when ='), piece('const liveLeft ='),
    piece('function quotaOf(w'), piece('function quotaPercent(value) {'), piece('const quotaWord ='), piece('function quotaView(q) {'),
    piece('const metered ='),
    piece('function windowOf(a, short) {'), piece('const NO_SHORT_LIMIT ='), piece('const noShortLimit ='), piece('const noShortNote ='),
    piece('const windowLabel ='), piece('const windowTitle ='), piece('const plainTitle ='), piece('function windowSeconds(w) {'),
    piece('const GROUP_WINDOWS ='), piece('const groupLabel ='), piece('function groupWindows(a) {'), piece('function covers(w, model) {'),
    piece('function windowElapsed(w'), piece('function windowSpent(a, w) {'),
    piece('const segsHTML ='), piece('function meterHTML(q'), piece('function pctHTML(w) {'), piece('function meterRowHTML(a, short) {'), piece('const meterRowsHTML ='),
    piece('const metersHTML ='), piece('const cap = (s)'), piece('const scopedWindows ='), piece('const scopedTitle ='),
    piece('function childRowHTML('), piece('function scopedRowsHTML(a, wide) {'),
    piece('function windowBlockHTML(a, w, title, size) {'), piece('function miniMetersHTML(a) {'), piece('function windowHTML(a, short, size) {'),
    piece('function noShortBlockHTML(size) {'), piece('function quotaWindowsHTML(a'),
  ].join('\n'), context);
  return (code) => vm.runInContext(code, context);
}

const account = (cooldowns = {}, kinds = {}) => ({
  id: 'a', provider: 'claude', kind: 'oauth', cooldowns, cooldown_kinds: kinds,
  quota: { windows: [{ name: '5h', used: 99, resets_at: RESET }, { name: 'week', used: 23, resets_at: '2026-10-28T16:00:00Z' }] },
});
const row = (run, a, short = true) => run(`meterRowHTML(${JSON.stringify(a)}, ${short})`);
const block = (run, a) => run(`(() => { const a = ${JSON.stringify(a)}; return windowBlockHTML(a, windowOf(a, true), '5-hour', 'lg'); })()`);

test('windows always show their reset countdown even when usage would run out sooner', () => {
  const run = load();
  for (const mode of ['remaining', 'used']) {
    run(`S.quotaDisplay = '${mode}'`);
    for (const used of [0, 50, 99, 100]) {
      const a = account();
      a.quota.windows[0].used = used;
      a.quota.windows[1].used = used;
      for (const short of [true, false]) {
        const reset = a.quota.windows[short ? 0 : 1].resets_at;
        const html = row(run, a, short);
        assert.match(html, /class="rst" title="(?:5-hour|Weekly) window resets [^"]+">↺ /);
        assert.ok(html.includes(`data-until="${reset}" data-countdown="compact"`));
        assert.doesNotMatch(html, /out|using .*% an hour/);
      }
      const note = block(run, a);
      assert.match(note, /(?:Resets|Back) .* · in <span[^>]*>2h 58m<\/span>/);
      assert.doesNotMatch(note, /at this pace/);
    }
  }
});

test('a usage-limit cooldown until the reset shows the reset, not the projection', () => {
  const run = load();
  const cooling = account({ 'claude-opus-5-5': '2026-10-23T16:40:00+00:00' }, { 'claude-opus-5-5': 'quota' });
  const html = row(run, cooling);
  assert.doesNotMatch(html, /out/);
  assert.match(html, /class="rst"[^>]*>↺ <span data-until="2026-10-23T16:40:00Z" data-countdown="compact">2h 58m<\/span>/);
  const note = block(run, cooling);
  assert.match(note, /Back \S+ · in <span[^>]*>2h 58m<\/span>/);
  assert.doesNotMatch(note, /at this pace/);
  assert.match(row(run, cooling, false), /↺ <span[^>]*>5d 2h<\/span>/, 'the weekly row is unchanged');
});

test('cooldown scope does not change the reset countdown', () => {
  const run = load();
  const early = account({ 'claude-opus-5-5': '2026-10-23T14:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.match(row(run, early), /class="rst"[^>]*>↺ <span[^>]*>2h 58m<\/span>/);
  const limited = account({ 'claude-opus-5-5': RESET }, { 'claude-opus-5-5': 'rate_limit' });
  assert.match(row(run, limited), /class="rst"[^>]*>↺ <span[^>]*>2h 58m<\/span>/);
  const weekly = account({ 'claude-opus-5-5': '2026-10-28T16:00:00Z' }, { 'claude-opus-5-5': 'quota' });
  assert.doesNotMatch(row(run, weekly), /out/, 'a weekly limit used up covers the 5-hour window too');
});

test('lists leave out the 5-hour meter of an account without that limit; its page says why', () => {
  const run = load();
  const ultra = { id: 'u', provider: 'claude', kind: 'oauth', quota: { no_five_hour_limit: true, windows: [{ name: 'week', used: 23, resets_at: '2026-10-28T16:00:00Z' }] } };
  const rows = run(`meterRowsHTML(${JSON.stringify(ultra)})`);
  assert.equal((rows.match(/class="mrow"/g) || []).length, 1);
  assert.doesNotMatch(rows, /5H|No limit|not reported/);
  assert.match(rows, /<span class="capsm">WK<\/span>/);
  assert.match(rows, /↺ /, 'the weekly row is unchanged');
  const mini = run(`miniMetersHTML(${JSON.stringify(ultra)})`);
  assert.equal((mini.match(/class="mrow"/g) || []).length, 1);
  assert.doesNotMatch(mini, /5H/);
  assert.equal(run(`noShortNote(${JSON.stringify(ultra)})`), ' · no 5h limit');
  const blocks = run(`quotaWindowsHTML(${JSON.stringify(ultra)}, 'lg')`);
  assert.match(blocks, /^<div class="window"><div class="window-top"><span>5-hour<\/span><span class="dim">No limit<\/span>/);
  assert.match(blocks, /This account has no five-hour limit; only the weekly limits apply\./);
  assert.match(blocks, /Weekly/);
  assert.match(run(`windowHTML(${JSON.stringify(ultra)}, true, 'xl')`), /5-hour window<\/span><span class="dim">No limit/);
  // A reported 5-hour window always wins, and plain missing data stays "not reported".
  const back = { ...ultra, quota: { ...ultra.quota, windows: [{ name: '5h', used: 10, resets_at: RESET }, ...ultra.quota.windows] } };
  assert.equal((run(`meterRowsHTML(${JSON.stringify(back)})`).match(/class="mrow"/g) || []).length, 2);
  assert.match(run(`meterRowsHTML(${JSON.stringify(back)})`), /<span class="capsm">5H<\/span>/);
  assert.equal(run(`noShortNote(${JSON.stringify(back)})`), '');
  const unknown = { ...ultra, quota: { windows: ultra.quota.windows } };
  assert.match(run(`meterRowsHTML(${JSON.stringify(unknown)})`), /5-hour: not reported/);
  assert.equal((run(`miniMetersHTML(${JSON.stringify(unknown)})`).match(/class="mrow"/g) || []).length, 2);
  assert.equal(run(`noShortNote(${JSON.stringify(unknown)})`), '');
  // ChatGPT Pro reports the same way and gets the same treatment.
  const pro = { id: 'p', provider: 'codex', kind: 'oauth', quota: { plan: 'pro', no_five_hour_limit: true, windows: [{ name: 'week', used: 17, resets_at: '2026-11-05T18:04:27Z' }] } };
  assert.equal((run(`meterRowsHTML(${JSON.stringify(pro)})`).match(/class="mrow"/g) || []).length, 1);
  assert.equal(run(`noShortNote(${JSON.stringify(pro)})`), ' · no 5h limit');
  assert.match(run(`quotaWindowsHTML(${JSON.stringify(pro)}, 'lg')`), /5-hour<\/span><span class="dim">No limit/);
  // A lapsed Meta subscription says so beside the plan.
  const lapsed = { id: 'm', provider: 'meta', kind: 'oauth', quota: { plan_name: 'Max Usage', subscription_inactive: true, windows: [] } };
  assert.equal(run(`noShortNote(${JSON.stringify(lapsed)})`), ' · subscription inactive');
  assert.equal(run(`noShortNote(${JSON.stringify({ ...lapsed, quota: { ...lapsed.quota, subscription_inactive: undefined } })})`), '');
});

test('Meta sign-ins are metered subscriptions with 5-hour and weekly rows', () => {
  const run = load();
  const meta = (quota, kind = 'oauth') => ({ id: 'm', provider: 'meta', kind, quota });
  assert.equal(run(`metered(${JSON.stringify(meta({ windows: [] }))})`), true, 'metered before the first reading, like Claude and Codex');
  assert.equal(run(`metered(${JSON.stringify(meta({ windows: [] }, 'api-key'))})`), false, 'a Meta API key has no subscription meters');
  const live = meta({
    plan_name: 'Muse Code Max Usage',
    windows: [{ name: '5h', used: 3, resets_at: RESET }, { name: 'week', used: 1, resets_at: '2026-10-28T16:00:00Z' }],
  });
  assert.equal(run(`metered(${JSON.stringify(live)})`), true);
  const rows = run(`meterRowsHTML(${JSON.stringify(live)})`);
  assert.equal((rows.match(/class="mrow"/g) || []).length, 2);
  assert.match(rows, /<span class="capsm">5H<\/span>/);
  assert.match(rows, /<span class="capsm">WK<\/span>/);
  const blocks = run(`quotaWindowsHTML(${JSON.stringify(live)}, 'lg')`);
  assert.match(blocks, /5-hour/);
  assert.match(blocks, /Weekly/);
});

test('the overview draws a model’s week, such as Claude’s Fable, as a child row of its account', () => {
  const run = load();
  run('S.quotaDisplay = "used"');
  // As production read an Ultra account on 2026-11-09, Fable further along than the week.
  const ultra = { id: 'u', provider: 'claude', kind: 'oauth', quota: { no_five_hour_limit: true, windows: [
    { name: 'week', used: 25, resets_at: '2026-10-28T16:00:00Z' },
    { name: 'week fable', used: 61, resets_at: '2026-10-28T16:00:00Z', model: 'fable' },
  ] } };
  const json = JSON.stringify(ultra);
  // The account's own meters keep 5H and WK, in every list.
  const plain = run(`metersHTML(${json})`);
  assert.equal((plain.match(/class="mrow"/g) || []).length, 1);
  assert.doesNotMatch(plain, /Fable|sub-/);
  // Wide: a line of its own under the account, its label in the name column, an empty label
  // slot so its meter lines up with WK's.
  const wide = run(`scopedRowsHTML(${json}, true)`);
  assert.match(wide, /^<div class="subline"><span class="c-idx"><\/span><span class="sub-label"><span aria-hidden="true">↳ <\/span>Fable<\/span><div class="meters"><div class="mrow"><span class="capsm"><\/span><span class="segs /);
  assert.match(wide, /aria-label="Fable weekly window quota used" aria-valuemin="0" aria-valuemax="100" aria-valuenow="61"/);
  assert.match(wide, /<span class="pct ">61%<\/span><span class="rst" title="Fable weekly window resets [^"]+">↺ <span data-until="2026-10-28T16:00:00Z"/);
  // Phone: a row in the meters after WK, its label in the label slot.
  const phone = run(`scopedRowsHTML(${json}, false)`);
  assert.match(phone, /^<div class="mrow sub-row"><span class="sub-label"><span aria-hidden="true">↳ <\/span>Fable<\/span><span class="segs /);
  assert.doesNotMatch(phone, /subline|capsm/);
  // The model's week is a week long, so it has an even-pace mark like WK's.
  const mark = (html) => (html.match(/<b class="pace" style="left:([\d.]+)%"/) || [])[1];
  assert.ok(mark(phone));
  assert.equal(mark(phone), mark(plain), 'both weeks renew together, so their pace is the same');
  run('S.quotaDisplay = "remaining"');
  assert.match(run(`scopedRowsHTML(${json}, true)`), /Fable weekly window quota remaining[^>]*aria-valuenow="39"/);
  // Running low, the whole child row takes the state colour: amber from 75% used, red from 95%.
  const at = (used) => JSON.stringify({ ...ultra, quota: { ...ultra.quota, windows: [ultra.quota.windows[0], { ...ultra.quota.windows[1], used }] } });
  assert.match(run(`scopedRowsHTML(${at(83)}, true)`), /^<div class="subline" data-level="warn">.*<span class="segs warn /s);
  assert.match(run(`scopedRowsHTML(${at(83)}, false)`), /^<div class="mrow sub-row" data-level="warn">.*<span class="pct warn">17%/s);
  assert.match(run(`scopedRowsHTML(${at(96)}, true)`), /^<div class="subline" data-level="err">/);
  assert.doesNotMatch(run(`scopedRowsHTML(${at(74)}, true)`), /data-level/);
  // A renewed model window waits for the next reading instead of showing a stale bar.
  const renewed = { ...ultra, quota: { ...ultra.quota, windows: [ultra.quota.windows[0], { ...ultra.quota.windows[1], resets_at: '2026-10-23T13:00:00Z' }] } };
  assert.equal(run(`scopedRowsHTML(${JSON.stringify(renewed)}, true)`), '');
  // Names reach the page escaped.
  const odd = { ...ultra, quota: { ...ultra.quota, windows: [{ ...ultra.quota.windows[1], name: 'week <b>', model: '<b>' }] } };
  assert.doesNotMatch(run(`scopedRowsHTML(${JSON.stringify(odd)}, true)`), /<b>/);
});

test('antigravity model groups become meters: Gemini as the account, others as a child row', () => {
  const run = load();
  run('S.quotaDisplay = "used"');
  // As production stored an Antigravity account on 2026-11-21 (usage invented).
  const limit = (group, window, used, resets_at, value) => ({ id: `antigravity:${group}:${window}`, name: `${group} · ${window}`,
    scope: { kind: 'model_families', value }, used_percent: used, resets_at });
  const a = { id: 'g', provider: 'antigravity', kind: 'oauth', cooldowns: {}, cooldown_kinds: {}, quota: { windows: [], limits: [
    limit('Gemini Models', 'weekly', 21, '2026-10-28T16:00:00Z', ['gemini-']),
    limit('Gemini Models', '5h', 38, RESET, ['gemini-']),
    limit('Claude and GPT models', 'weekly', 81, '2026-10-28T16:00:00Z', ['claude-', 'gpt-']),
    limit('Claude and GPT models', '5h', 64, '2026-10-23T17:40:00Z', ['claude-', 'gpt-']),
    { id: 'x', name: 'Agent credits', scope: { kind: 'feature', value: 'x' }, used_percent: 5 },
  ] } };
  const g = JSON.parse(run(`JSON.stringify(groupWindows(${JSON.stringify(a)}))`));
  assert.deepEqual(g.quota.windows.map((w) => [w.name, w.model || null, w.group, w.used]), [
    ['5h', null, 'Gemini', 38], ['week', null, 'Gemini', 21], ['5h', 'Claude & GPT', 'Claude & GPT', 64], ['week', 'Claude & GPT', 'Claude & GPT', 81],
  ]);
  assert.deepEqual(g.quota.limits.map((l) => l.id), ['x'], 'anything else stays a limit');
  const json = JSON.stringify(g);
  // Gemini's windows are the account's 5H and WK rows, titled by their group.
  const rows = run(`meterRowsHTML(${json})`);
  assert.match(rows, /<span class="capsm">5H<\/span>.*aria-valuenow="38".*title="Gemini 5-hour window resets/s);
  assert.match(rows, /<span class="capsm">WK<\/span>.*aria-valuenow="21"/s);
  // The other group: one child line per window, named once, each labelled by its window.
  const wide = run(`scopedRowsHTML(${json}, true)`);
  assert.equal((wide.match(/class="subline"/g) || []).length, 2);
  assert.equal((wide.match(/Claude &amp; GPT<\/span>/g) || []).length, 1);
  assert.match(wide, /<span class="capsm">5H<\/span>.*aria-valuenow="64".*<span class="capsm">WK<\/span>.*aria-valuenow="81"/s);
  assert.match(wide, /Claude &amp; GPT weekly window quota used/);
  const phone = run(`scopedRowsHTML(${json}, false)`);
  assert.match(phone, /^<div class="mrow sub-row sub-head"><span class="sub-label">.*Claude &amp; GPT<\/span><\/div><div class="mrow sub-row"><span class="capsm">5H<\/span>/s);
  // The account page names every window.
  const blocks = run(`quotaWindowsHTML(${json})`);
  for (const title of ['Gemini 5-hour', 'Gemini weekly', 'Claude &amp; GPT 5-hour', 'Claude &amp; GPT weekly']) assert.ok(blocks.includes(`${title} window`), title);
  // A Claude cooldown until the group's renewal marks only that group's window as spent.
  const cooled = { ...g, cooldowns: { 'claude-opus-5-5-high': '2026-10-28T16:00:00Z' }, cooldown_kinds: { 'claude-opus-5-5-high': 'quota' } };
  const c = JSON.stringify(cooled);
  assert.equal(run(`windowSpent(${c}, ${c}.quota.windows[3])`), true);
  assert.equal(run(`windowSpent(${c}, ${c}.quota.windows[1])`), false);
  // Accounts with ordinary windows are left alone.
  const claude = account();
  assert.equal(run(`groupWindows(${JSON.stringify(claude)}).quota.windows.length`), 2);
});
