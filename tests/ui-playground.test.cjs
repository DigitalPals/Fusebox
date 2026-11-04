const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// playground.js on its own, with the few app.js helpers it calls stubbed.
function load() {
  const listeners = {};
  const elements = {};
  const patched = [];
  const context = {
    S: { play: null, models: [{ id: 'claude-sonnet-5-5' }, { id: 'gemini-3.8-flash' }], routes: { groups: {} }, key: '' },
    PROVIDER: { claude: 'Claude', codex: 'Codex' },
    esc: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
    who: (s) => s,
    fmt: (n) => String(n),
    ms: (n) => `${n} ms`,
    patch: (id) => patched.push(id),
    $: (sel) => elements[sel] || null,
    $$: () => [],
    document: { addEventListener: (type, fn) => { listeners[type] = fn; } },
  };
  vm.createContext(context);
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../ui/playground.js'), 'utf8'), context);
  return { context, listeners, elements, patched };
}

test('the card opens in send mode with the old playground, and switches to inspect', () => {
  const { context, elements } = load();
  context.openPlay();
  assert.equal(context.S.play.mode, 'send');
  let html = context.playHTML();
  assert.match(html, /id="play-form"/);
  assert.match(html, /aria-pressed="true">Send a prompt/);
  elements['#play-prompt'] = { value: 'typed before switching' };
  context.setPlayMode('inspect');
  assert.equal(context.S.play.prompt, 'typed before switching', 'the draft survives the switch');
  html = context.playHTML();
  assert.match(html, /id="inspect-form"/);
  assert.match(html, /Inspect, don’t send/);
  assert.match(html, /claude-sonnet-5-5/, 'a sample request is filled in');
});

test('header lines become a map; lines without a name are ignored', () => {
  const { context } = load();
  assert.deepEqual(
    JSON.parse(JSON.stringify(context.parseHeaderLines('user-agent: claude-cli/2.1.318 (external, cli)\n\nnot a header\nx-app:cli'))),
    { 'user-agent': 'claude-cli/2.1.318 (external, cli)', 'x-app': 'cli' },
  );
});

test('an inspection shows the model, account, path, route and redacted request', () => {
  const { context } = load();
  const html = context.inspectedHTML({
    requested_model: 'big', model: 'claude-sonnet-5-5', mapped: { client: 'agent', from: 'big', to: 'claude-sonnet-5-5' },
    account: { id: 'file:a.json', label: 'Work', provider: 'claude', subscription: true, upstream_model: 'claude-sonnet-5-5' },
    shape: { path: 'translated', wire: 'messages', upstream_stream: true },
    dropped_controls: ['verbosity_unsupported'],
    route: [{ account: 'file:a.json', state: 'ready', next: true }],
    request: { url: 'https://api.anthropic.com/v1/messages', headers: [['authorization', '<redacted>']], body: { model: 'claude-sonnet-5-5' }, bytes: 27 },
    notes: ['Nothing was sent.'],
  });
  assert.match(html, /big<\/span> → /);
  assert.match(html, /mapped by agent/);
  assert.match(html, /Work <span class="meta">Claude subscription · claude-sonnet-5-5/);
  assert.match(html, /Translated to messages, streamed/);
  assert.match(html, /verbosity_unsupported/);
  assert.match(html, /ready · next/);
  assert.match(html, /authorization: &lt;redacted&gt;/);
  assert.match(html, /Nothing was sent\./);
});

test('a refusal is shown first, and nothing else is required', () => {
  const { context } = load();
  const html = context.inspectedHTML({ requested_model: 'nope', refusal: { status: 404, message: 'no available account serves model `nope`' } });
  assert.match(html, /^\s*<div class="inspect-out"[^>]*>\s*<p class="msg err">Refused with 404: no available account serves model `nope`<\/p>/);
});

test('a body that is not JSON is explained without calling the server', async () => {
  const { context } = load();
  let called = false;
  context.fetch = async () => { called = true; };
  context.openPlay();
  context.S.play.request = '{ not json';
  await context.runInspect();
  assert.equal(called, false);
  assert.match(context.S.play.inspectError, /isn't JSON/);
});
