const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Config, Clients: the clients and shared keys lists, run from the dashboard's own sources.
const source = fs.readFileSync(path.join(__dirname, '../ui/config.js'), 'utf8');
const app = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const line = (start) => app.slice(app.indexOf(start), app.indexOf('\n', app.indexOf(start)));
// A whole top-level function from app.js, up to its closing brace.
const fn = (name) => { const at = app.indexOf(`function ${name}(`); return app.slice(at, app.indexOf('\n}\n', at) + 2); };
const helpers = [line("const esc = (v) =>"), line("const HIDDEN = '"), line('const secret = (key) =>'), line('const copyBtn = (text, label, note) =>'),
  fn('ago'), line('const liveAgo = (iso) =>')].join('\n');

function clients(values, { private: hidden = false, usage = null } = {}) {
  const saves = [];
  const context = {
    S: {
      private: hidden, route: 'config',
      overview: { client_keys: values['api-keys'] || [], named_clients: (values['named-clients'] || []).length },
      config: { values, saved: JSON.parse(JSON.stringify(values)), busy: false, errors: {}, section: 'clients', clientUsage: usage },
    },
    ICON: { copy: '' }, document: { addEventListener() {}, activeElement: null }, crypto: globalThis.crypto, CSS: { escape: (v) => v },
    fmt: String, render() {}, patch() {}, refreshAccounts() {}, $: () => null, $$: () => [],
    endpointItemHTML: () => '',
    setupHTML: (key, uid) => `<div class="stub-setup" data-key="${key}" data-uid="${uid}">setup</div>`,
    // The settings API: a save answers with the file's new values.
    api: async (url, opts = {}) => {
      if (opts.method !== 'PATCH') return { clients: {}, last_used: {}, shared_last_used: [] };
      const { changes } = JSON.parse(opts.body);
      saves.push(changes);
      return { values: { ...context.S.config.values, ...JSON.parse(JSON.stringify(changes)) }, defaults: {}, revision: `r${saves.length}` };
    },
  };
  vm.createContext(context);
  vm.runInContext(`${helpers}\n${source}`, context);
  vm.runInContext('S.config.named = namedIdle()', context);
  const html = () => vm.runInContext('`${sharedKeysHTML()}${namedClientsHTML()}`', context);
  return {
    named: context.S.config.named,
    saves,
    values: () => context.S.config.values,
    html,
    // What a person sees, and what a screen reader reads out.
    visible: () => {
      const labels = [...html().matchAll(/aria-label="([^"]*)"/g)].map((m) => m[1]);
      return `${html().replace(/<[^>]*>/g, ' ')} ${labels.join(' ')}`.replace(/\s+/g, ' ');
    },
    run: (code) => vm.runInContext(code, context),
  };
}

const whombat = { id: 'whombat-cybex', label: 'Whombat · Cybex BV', key: 'wbgw_0123456789abcdefghij' };
const laptop = { id: 'laptop', label: 'Laptop', key: 'fbx_abcdefghijklmnopqrstuvwx' };

test('ids come from the label: lowercase, a-z0-9 and dashes, unique, at most 40', () => {
  const { run } = clients({});
  const id = (label, taken = []) => run(`namedClientId(${JSON.stringify(label)}, ${JSON.stringify(taken)})`);
  assert.equal(id('Whombat · Cybex BV'), 'whombat-cybex-bv');
  assert.equal(id('  Café Büro  '), 'cafe-buro');
  assert.equal(id('!!!'), 'client');
  assert.equal(id('Laptop', ['laptop']), 'laptop-2');
  assert.equal(id('Laptop', ['laptop', 'laptop-2']), 'laptop-3');
  const long = 'A very long name for a tool that goes on and on and on';
  for (const taken of [[], [id(long)]]) {
    const value = id(long, taken);
    assert.ok(value.length <= 40, value);
    assert.match(value, /^[a-z0-9]+(-[a-z0-9]+)*$/);
    assert.ok(!taken.includes(value));
  }
});

test('new keys are fbx_ client keys, never collector keys', () => {
  const { run } = clients({});
  for (let i = 0; i < 20; i++) assert.match(run('generateClientKey()'), /^fbx_[a-z0-9]{24}$/);
});

test('keys are masked until shown, and copy buttons carry the real key', () => {
  const page = clients({ 'api-keys': ['fbx_shared'], 'named-clients': [whombat, laptop] });
  page.named.open = 'whombat-cybex';
  assert.ok(page.visible().includes('Whombat · Cybex BV'));
  assert.ok(page.visible().includes('whombat-cybex'));
  assert.ok(page.visible().includes('wbgw_01…ghij'));
  assert.ok(!page.visible().includes(whombat.key));
  assert.ok(page.html().includes(`data-text="${whombat.key}"`));
  assert.ok(page.html().includes('Show key'));
  page.named.shown['whombat-cybex'] = true;
  assert.ok(page.visible().includes(whombat.key));
  assert.ok(page.html().includes('Hide key'));
  assert.ok(!page.visible().includes(laptop.key));
  assert.ok(!page.visible().includes('fbx_shared'), 'a shared key is masked too');
});

test('privacy hides labels, ids and keys, even a shown key', () => {
  const page = clients({ 'api-keys': ['fbx_sharedsharedshared'], 'named-clients': [whombat, laptop] }, { private: true });
  page.named.shown['whombat-cybex'] = true;
  page.named.open = 'whombat-cybex';
  const html = page.visible();
  for (const text of [whombat.label, whombat.id, whombat.key, laptop.key, 'wbgw_01', 'fbx_sha']) assert.ok(!html.includes(text), text);
  assert.ok(html.includes('fbx_••••••••'));
  assert.ok(!page.html().includes('named-reveal'));
  assert.ok(page.html().includes(`data-text="${laptop.key}"`));
});

test('removing the last named client without a shared key warns that keys turn off', () => {
  const alone = clients({ 'api-keys': [], 'named-clients': [whombat] });
  alone.named.confirm = 'whombat-cybex';
  assert.ok(alone.visible().includes('Fusebox will accept requests without a key again.'));
  const shared = clients({ 'api-keys': ['fbx_shared'], 'named-clients': [whombat] });
  shared.named.confirm = 'whombat-cybex';
  assert.ok(shared.visible().includes('Remove Whombat · Cybex BV?'));
  assert.ok(!shared.visible().includes('without a key again'));
  const two = clients({ 'api-keys': [], 'named-clients': [whombat, laptop] });
  two.named.confirm = 'laptop';
  assert.ok(!two.visible().includes('without a key again'));
});

test('adding the first key-giving client says Fusebox will ask for keys', () => {
  const open = clients({ 'api-keys': [], 'named-clients': [] });
  open.named.adding = true;
  open.named.label = 'Whombat · Cybex BV';
  assert.ok(open.visible().includes('After this, Fusebox only accepts requests with a key.'));
  assert.ok(open.visible().includes('Id: whombat-cybex-bv'));
  assert.equal(open.run(`namedLabelProblem('${laptop.label}', [${JSON.stringify(laptop)}])`), 'Another client already has this name.');
  assert.equal(open.run("namedLabelProblem('', [])"), 'Enter a name.');
  const keyed = clients({ 'api-keys': ['fbx_shared'], 'named-clients': [] });
  keyed.named.adding = true;
  assert.ok(!keyed.visible().includes('only accepts requests with a key'));
});

const shared = 'fbx_sharedkeysharedkey01';
const other = 'fbx_sharedkeysharedkey02';

test('shared keys are listed, masked, with when each was last used, until each is named', () => {
  const page = clients({ 'api-keys': [shared, other], 'named-clients': [laptop] },
    { usage: { last_used: { laptop: new Date(Date.now() - 120000).toISOString() }, shared_last_used: [null, new Date().toISOString()] } });
  const text = page.visible();
  assert.ok(text.includes('Shared keys'));
  assert.ok(text.includes('fbx_sha…ey01') && text.includes('fbx_sha…ey02'));
  assert.ok(!text.includes(shared) && !text.includes(other));
  assert.ok(text.includes('Name shared key 1') && text.includes('Remove shared key 2'));
  assert.ok(text.includes('Last used just now'), 'the second key was used');
  assert.ok(text.includes('Last used 2m ago'), 'and so was the laptop');
  assert.equal(clients({ 'api-keys': [], 'named-clients': [laptop] }).run('sharedKeysHTML()').includes('Name it'), false);
});

test('naming a shared key makes it a client with the same key, in one save', async () => {
  const page = clients({ 'api-keys': [shared, other], 'named-clients': [laptop] });
  page.run('sharedAction("shared-name", 1)');
  assert.equal(page.named.shared.naming, other);
  assert.ok(page.visible().includes('Save name'));
  page.named.shared.label = 'MacBook Pro';
  await page.run('nameSharedKey()');
  assert.equal(page.saves.length, 1, 'the key never stops working');
  assert.deepEqual(page.saves[0]['api-keys'], [shared]);
  assert.deepEqual(page.saves[0]['named-clients'], [laptop, { id: 'macbook-pro', label: 'MacBook Pro', key: other }]);
  assert.equal(page.named.shared.naming, null);
  assert.ok(!page.run('sharedKeysHTML()').includes('ey02'));
  assert.ok(page.visible().includes('MacBook Pro'));
});

test('a shared key needs a name no other client has', async () => {
  const page = clients({ 'api-keys': [shared], 'named-clients': [laptop] });
  page.run('sharedAction("shared-name", 0)');
  page.named.shared.label = ' laptop ';
  await page.run('nameSharedKey()');
  assert.equal(page.saves.length, 0);
  assert.ok(page.visible().includes('Another client already has this name.'));
});

test('removing the last shared key without clients warns that keys turn off', () => {
  const alone = clients({ 'api-keys': [shared], 'named-clients': [] });
  alone.run('sharedAction("shared-remove", 0)');
  assert.ok(alone.visible().includes('Fusebox will accept requests without a key again.'));
  const keyed = clients({ 'api-keys': [shared], 'named-clients': [laptop] });
  keyed.run('sharedAction("shared-remove", 0)');
  assert.ok(keyed.visible().includes('Tools using it stop working.'));
  assert.ok(!keyed.visible().includes('without a key again'));
});

test('a new client shows its key with the setup for it', async () => {
  const page = clients({ 'api-keys': [], 'named-clients': [laptop] });
  page.run('namedAction("named-add")');
  page.named.label = 'Build server';
  await page.run('addNamedClient()');
  const [added] = page.saves[0]['named-clients'].slice(-1);
  assert.equal(added.id, 'build-server');
  assert.match(added.key, /^fbx_[a-z0-9]{24}$/);
  const text = page.visible();
  assert.ok(text.includes('Build server is ready'));
  assert.ok(page.html().includes(`data-key="${added.key}" data-uid="-new"`), 'the setup carries the new key');
});

test('Set up shows the setup with that client’s key; one row panel at a time', () => {
  const page = clients({ 'named-clients': [whombat, laptop] });
  page.run('namedAction("named-more", "laptop")');
  assert.ok(page.visible().includes('Replace the key for Laptop'));
  page.run('namedAction("named-setup", "laptop")');
  assert.equal(page.named.open, null, 'opening the setup closes the other actions');
  assert.ok(page.html().includes(`data-key="${laptop.key}" data-uid="-row"`));
  assert.ok(!page.html().includes(`data-key="${whombat.key}"`));
  page.run('namedAction("named-setup", "laptop")');
  assert.ok(!page.html().includes('stub-setup'));
});

test('replacing a key keeps the client’s id, name and limits', async () => {
  const limited = { ...laptop, 'requests-per-minute': 60 };
  const page = clients({ 'named-clients': [whombat, limited] });
  page.run('namedAction("named-replace", "laptop")');
  assert.ok(page.visible().includes('Tools using the current key stop working until you give them the new one.'));
  await page.run('namedAction("named-replace-confirm", "laptop")');
  const [, saved] = page.saves[0]['named-clients'];
  assert.deepEqual({ ...saved, key: 'new' }, { ...limited, key: 'new' });
  assert.notEqual(saved.key, laptop.key);
  assert.deepEqual(page.saves[0]['named-clients'][0], whombat);
  assert.ok(page.visible().includes('New key for Laptop'));
  assert.ok(page.visible().includes('The old key no longer works.'));
});

test('renaming changes the label only', async () => {
  const page = clients({ 'named-clients': [whombat, laptop] });
  page.run('namedAction("named-rename", "laptop")');
  assert.equal(page.named.renameLabel, 'Laptop');
  page.named.renameLabel = 'Whombat · Cybex BV';
  await page.run('renameNamedClient({ dataset: { id: "laptop" } })');
  assert.equal(page.saves.length, 0, 'not a name another client has');
  page.named.renameLabel = 'Old laptop';
  await page.run('renameNamedClient({ dataset: { id: "laptop" } })');
  assert.deepEqual(page.saves[0]['named-clients'][1], { ...laptop, label: 'Old laptop' });
  assert.equal(page.named.rename, null);
});

// The setup snippets, from app.js: a client's key, or a placeholder while keys are on.
function snippets(overview) {
  const context = { S: { private: false, overview, models: [] }, location: { origin: 'https://fusebox.test' } };
  vm.createContext(context);
  vm.runInContext(`${helpers}\n${fn('setupModelsFor')}\n${fn('setupModel')}\n${fn('snippet')}`, context);
  return (kind, key) => vm.runInContext(`snippet(${JSON.stringify(kind)}, ${JSON.stringify(key ?? null)})`, context);
}

test('setup snippets carry the client’s key, never a shared one', () => {
  const keyed = snippets({ client_keys: [shared], named_clients: 1 });
  assert.ok(keyed('claude', laptop.key).text.includes(`ANTHROPIC_AUTH_TOKEN=${laptop.key}`));
  assert.ok(keyed('claude').text.includes('ANTHROPIC_AUTH_TOKEN=your-client-key'));
  assert.ok(!keyed('curl').text.includes(shared));
  assert.ok(keyed('codex', laptop.key).text.includes('env_key = "FUSEBOX_KEY"'));
  const open = snippets({ client_keys: [], named_clients: 0 });
  assert.ok(open('claude').text.includes('ANTHROPIC_AUTH_TOKEN=fbx_local'));
  assert.ok(open('sdk').note.includes('No key is required'));
  assert.ok(!open('codex').text.includes('env_key'));
});

test('identity learning requires an explicit named-client opt-in and can be revoked', async () => {
  const page = clients({ 'api-keys': [], 'named-clients': [laptop] });
  page.named.open = laptop.id;
  assert.ok(page.html().includes('Allow identity learning'));
  await page.run(`namedAction('named-trust-identity', '${laptop.id}')`);
  assert.equal(page.values()['named-clients'][0]['trust-client-identity'], true);
  assert.ok(page.html().includes('Stop identity learning'));
  await page.run(`namedAction('named-trust-identity', '${laptop.id}')`);
  assert.equal(page.values()['named-clients'][0]['trust-client-identity'], false);
  assert.equal(page.values()['named-clients'][0].key, laptop.key);
});

test('learned identity inspection escapes values and respects privacy mode', () => {
  const page = clients({});
  page.run(`S.config.identities = { source: '<script>private-client</script>' }`);
  const html = page.run('clientIdentitiesHTML()');
  assert.ok(html.includes('&lt;script&gt;'));
  assert.ok(!html.includes('<script>'));
  page.run('S.private = true');
  assert.ok(!page.run('clientIdentitiesHTML()').includes('private-client'));
});
