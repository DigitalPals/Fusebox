'use strict';

const CONFIG_SECTIONS = [
  ['clients', 'Clients'], ['server', 'Server'], ['access', 'Access'], ['routing', 'Routing'], ['connections', 'Connections'],
  ['providers', 'Providers'], ['models', 'Model rules'], ['notifications', 'Notifications'], ['diagnostics', 'Diagnostics'], ['yaml', 'YAML file'],
];
const CONFIG_DESCRIPTIONS = {
  clients: 'The endpoint, a key for each machine or tool, and how to set each one up.',
  server: 'Where Fusebox listens and where it keeps sign-in files.',
  access: 'Who can use the proxy and the dashboard.',
  routing: 'How Fusebox picks an account for each request.',
  connections: 'How Fusebox reaches the providers.',
  providers: 'API keys used alongside your signed-in accounts.',
  models: 'Rules for signed-in accounts. API key model rules are set under Providers.',
  notifications: 'Push notifications on your phone or computer, and messages in chat, when something trips. The events apply to all of them.',
  diagnostics: 'Logging, and settings in the file that Fusebox ignores.',
  yaml: 'The whole config.yaml, including settings the other sections don\u2019t cover. Changes are checked before they are applied.',
};
// Which section shows each setting, for the unsaved-changes dots.
const CONFIG_KEY_SECTION = {
  host: 'server', port: 'server', 'auth-dir': 'server', tls: 'server',
  'api-keys': 'clients', 'management-key': 'access', 'management-allow-remote': 'access',
  routing: 'routing', 'five-hour-reserve-percent': 'routing', 'banked-reset-policy': 'routing', 'request-retry': 'routing', 'session-affinity': 'routing',
  'session-affinity-idle-seconds': 'routing', 'force-model-prefix': 'routing', 'first-token-timeout-seconds': 'routing',
  'model-groups': 'routing', 'background-model': 'routing', 'prompt-cache': 'routing', 'request-limits': 'connections', 'log-format': 'diagnostics',
  'proxy-url': 'connections', 'codex-websockets': 'connections', 'claude-cloak': 'connections', 'banked-resets': 'connections',
  'oauth-model-alias': 'models', 'oauth-excluded-models': 'models', 'extra-models': 'models', notifications: 'notifications', debug: 'diagnostics',
};
const CONFIG_OAUTH = [['claude', 'Claude'], ['codex', 'Codex'], ['antigravity', 'Antigravity'], ['kimi', 'Kimi'], ['xai', 'Grok'], ['meta', 'Meta'], ['devin', 'Devin'], ['vertex', 'Vertex AI']];
const CONFIG_PROVIDERS = [
  ['claude', 'Claude', 'claude-api-key'], ['codex', 'OpenAI / Codex', 'codex-api-key'],
  ['gemini', 'Gemini', 'gemini-api-key'], ['vertex', 'Vertex AI', 'vertex-api-key'],
  ['kimi', 'Kimi', 'kimi-api-key'], ['xai', 'Grok / xAI', 'xai-api-key'], ['meta', 'Meta', 'meta-api-key'],
  ['compat', 'OpenAI-compatible', 'openai-compatibility'],
];
const configClone = (value) => JSON.parse(JSON.stringify(value));
const configEqual = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const configPath = (path) => esc(JSON.stringify(path));
const configId = (path) => `setting-${path.map((part) => encodeURIComponent(part)).join('-')}`;
const configGet = (path, root = S.config.values) => path.reduce((v, part) => v?.[part], root);

function configSet(path, value) {
  let parent = S.config.values;
  for (let i = 0; i < path.length - 1; i++) {
    const key = path[i];
    if (!Object.hasOwn(parent, key) || parent[key] == null) {
      Object.defineProperty(parent, key, { value: typeof path[i + 1] === 'number' ? [] : {}, writable: true, enumerable: true, configurable: true });
    }
    parent = parent[key];
  }
  Object.defineProperty(parent, path[path.length - 1], { value, writable: true, enumerable: true, configurable: true });
}

function configChanges() {
  const c = S.config;
  if (!c.values || !c.saved) return {};
  return Object.fromEntries(Object.entries(c.values).filter(([key, value]) => !configEqual(value, c.saved[key])));
}

function configDirty() { return Object.keys(configChanges()).length > 0; }

function configHelp(path, text) {
  return text ? `<small id="${configId(path)}-help">${esc(text)}</small>` : '';
}

// A text, number or secret input. `bare` keeps the label for screen readers only (list rows).
function configField(path, label, opts = {}) {
  const value = configGet(path);
  const id = configId(path);
  const error = S.config.errors[id];
  const secret = opts.type === 'password';
  const editing = S.config.secrets[JSON.stringify(path)];
  const shown = secret && value && !editing ? '' : value ?? '';
  const placeholder = secret && value && !editing ? 'Saved key · leave unchanged' : opts.placeholder || '';
  const input = `<input id="${id}" data-cfg="${configPath(path)}" ${opts.nullable ? 'data-nullable="true"' : ''}
    type="${opts.type || 'text'}" value="${esc(shown)}" placeholder="${esc(placeholder)}" ${secret ? 'data-secret="true"' : ''}
    ${opts.mono === false ? 'style="font-family:var(--sans)"' : 'spellcheck="false"'} ${opts.disabled ? 'disabled' : ''}
    ${opts.required && !(secret && value) ? 'required' : ''} ${opts.min != null ? `min="${opts.min}"` : ''}
    ${opts.max != null ? `max="${opts.max}"` : ''} ${opts.type === 'number' ? 'step="1"' : ''}
    ${secret ? 'autocomplete="new-password"' : 'autocomplete="off"'}
    aria-describedby="${opts.help ? `${id}-help ` : ''}${id}-error" ${error ? 'aria-invalid="true"' : ''}>`;
  const badge = opts.restart ? '<span class="cfg-badge">Needs restart</span>' : '';
  return `<div class="field ${opts.wide ? 'wide' : ''} ${opts.cls || ''}"><label for="${id}"${opts.bare ? ' class="sr-only"' : ''}>${esc(label)}${badge}</label>
    ${secret ? `<div class="cfg-secret">${input}<button type="button" class="btn ghost" data-config-act="reveal" data-path="${configPath(path)}" aria-label="Show ${esc(label)}">Show</button>
      ${value && !opts.bare ? `<button type="button" class="btn ghost" data-config-act="clear-secret" data-path="${configPath(path)}" aria-label="Clear ${esc(label)}">Clear</button>` : ''}</div>` : input}
    ${configHelp(path, opts.help)}<small id="${id}-error" class="cfg-error" data-error-for="${id}">${esc(error || '')}</small></div>`;
}

// Segmented choice (a few short options) for a setting.
function configChoice(path, label, options, help = '') {
  const value = JSON.stringify(configGet(path) ?? null);
  const id = configId(path);
  return `<div class="cfg-choice wide"><span id="${id}-label">${esc(label)}</span>
    <div class="seg" role="group" aria-labelledby="${id}-label">${options.map(([v, text]) => `<button type="button" data-config-act="choice" data-path="${configPath(path)}" data-value="${esc(JSON.stringify(v))}" aria-pressed="${value === JSON.stringify(v)}">${esc(text)}</button>`).join('')}</div>
    ${configHelp(path, help)}</div>`;
}

// Option cards for a choice whose options need a sentence each (routing strategy).
function configCards(path, label, options) {
  const value = configGet(path);
  return `<div class="cfg-choice wide"><span id="${configId(path)}-label">${esc(label)}</span>
    <div class="opt-cards" role="radiogroup" aria-labelledby="${configId(path)}-label">${options.map(([v, title, desc]) => `<button type="button" class="opt-card" role="radio" aria-checked="${value === v}" data-config-act="choice" data-path="${configPath(path)}" data-value="${esc(JSON.stringify(v))}">
      <span class="ring" aria-hidden="true"></span><span class="cell2"><span>${esc(title)}</span><span>${esc(desc)}</span></span></button>`).join('')}</div></div>`;
}

function configSwitch(path, label, help = '', restart = false) {
  const id = configId(path);
  return `<div class="cfg-toggle wide"><div><label id="${id}-label" for="${id}">${esc(label)}${restart ? '<span class="cfg-badge">Needs restart</span>' : ''}</label>${configHelp(path, help)}</div>
    <button id="${id}" type="button" class="switch" role="switch" aria-checked="${!!configGet(path)}" aria-labelledby="${id}-label"
      ${help ? `aria-describedby="${id}-help"` : ''} data-config-act="switch" data-path="${configPath(path)}"></button></div>`;
}

function configRemove(path, label = 'Remove row') {
  return `<button type="button" class="btn danger" data-config-act="remove" data-path="${configPath(path)}" aria-label="${esc(label)}">Remove</button>`;
}

function configListHead(title, help, addLabel, addAttrs, primary = false) {
  return `<div class="cfg-list-head"><div class="cell2"><h3>${esc(title)}</h3>${help ? `<p class="cfg-description">${esc(help)}</p>` : ''}</div>
    ${addLabel ? `<button type="button" class="btn sm${primary ? ' primary' : ''}" ${addAttrs}>${esc(addLabel)}</button>` : ''}</div>`;
}

// `words` renames the rows for lists that aren't patterns: { add, item, one, placeholder }.
function configStrings(path, label, help = '', secret = false, empty = '', words = null) {
  const list = configGet(path) || [];
  const w = words || (secret ? { add: 'Add key', item: 'API key', one: 'API key', placeholder: 'Enter an API key' } : { add: 'Add pattern', item: 'Model pattern', one: 'pattern', placeholder: 'claude-*' });
  return `<div class="cfg-list wide">${configListHead(label, help, w.add, `data-config-act="add-string" data-path="${configPath(path)}"`)}
    ${list.length ? list.map((_, i) => `<div class="cfg-row">${configField([...path, i], `${w.item} ${i + 1}`, { type: secret ? 'password' : 'text', required: true, bare: true, placeholder: w.placeholder })}${configRemove([...path, i], `Remove ${w.one} ${i + 1}`)}</div>`).join('')
      : `<p class="cfg-empty">${esc(empty || (secret ? 'No keys configured.' : 'No models excluded.'))}</p>`}</div>`;
}

function configAliases(path, oauth = false) {
  const list = configGet(path) || [];
  return `<div class="cfg-list wide">${configListHead(oauth ? 'Model aliases' : 'Allowed models and aliases', oauth ? 'Expose an upstream model under a name your clients use.' : 'Leave empty to allow all available models. An alias changes the name clients use.', 'Add model', `data-config-act="add-alias" data-path="${configPath(path)}"${oauth ? ' data-oauth="true"' : ''}`)}
    ${list.length ? list.map((_, i) => {
      const fork = [...path, i, 'fork'];
      return `<div class="cfg-row">${configField([...path, i, 'name'], `Upstream model ${i + 1}`, { required: true, bare: true, placeholder: oauth ? 'claude-sonnet-5-5' : 'Upstream model' })}
      <span class="arrow" aria-hidden="true">→</span>${configField([...path, i, 'alias'], `Alias ${i + 1}`, { required: oauth, nullable: !oauth, bare: true, cls: 'alias', placeholder: oauth ? 'sonnet' : 'Same as upstream' })}
      ${oauth ? `<button type="button" class="fork" role="switch" aria-checked="${!!configGet(fork)}" data-config-act="switch" data-path="${configPath(fork)}" id="${configId(fork)}" title="Serve both the original name and the alias"><span class="switch sm" aria-hidden="true"${configGet(fork) ? ' data-on="true"' : ''}></span>Keep original</button>` : ''}
      ${configRemove([...path, i], `Remove model ${i + 1}`)}</div>`;
    }).join('') : `<p class="cfg-empty">${oauth ? 'No model aliases configured.' : 'All models from this provider are allowed.'}</p>`}</div>`;
}

function configHeaders(path) {
  const headers = configGet(path) || {};
  return `<div class="cfg-list wide">${configListHead('Extra HTTP headers', 'Sent with requests to this provider.', 'Add header', `data-config-act="add-header" data-path="${configPath(path)}"`)}
    ${Object.entries(headers).map(([key], i) => `<div class="cfg-row"><div class="field"><label class="sr-only" for="${configId([...path, i, 'key'])}">Header name ${i + 1}</label>
      <input id="${configId([...path, i, 'key'])}" type="text" value="${esc(key)}" data-header-path="${configPath(path)}" data-header-key="${esc(key)}" data-header-index="${i}" required autocomplete="off" spellcheck="false">
      <small class="cfg-error" data-header-error="true">${esc(S.config.errors[configId([...path, i, 'key'])] || '')}</small></div>
      <span class="arrow" aria-hidden="true">:</span>${configField([...path, key], `${key} value`, { bare: true, placeholder: 'Value' })}
      <button type="button" class="btn danger" data-config-act="remove-header" data-path="${configPath(path)}" data-key="${esc(key)}" aria-label="Remove ${esc(key)} header">Remove</button></div>`).join('') || '<p class="cfg-empty">No extra headers configured.</p>'}</div>`;
}

// Config, Clients: the endpoint and a key for each client, each with a name, so the Usage
// page shows who used what and limits can apply. Shared keys from before (api-keys) are
// listed until each is named; none are made here. Changes here are saved at once.
function configClientsHTML() {
  loadClientUsage();
  const open = !sharedKeys().length && !(configGet(['named-clients']) || []).length;
  const help = open
    ? 'Fusebox takes requests without a key until you add a client. Give each machine or tool its own, and the Usage page shows each by name.'
    : 'Each client sends its own key. Give each machine or tool its own, and the Usage page shows each by name.';
  return `<div class="cfg-clients">${endpointItemHTML()}<p class="cfg-description">${help}</p></div>
    ${open ? `<div class="setup" data-setup="">${setupHTML()}</div>` : ''}
    ${sharedKeys().length ? `<div class="cfg-divider"></div><div class="cfg-list" id="shared-keys">${sharedKeysHTML()}</div>` : ''}
    <div class="cfg-divider"></div>
    <div class="cfg-list" id="named-clients">${namedClientsHTML()}</div>
    <div class="cfg-divider"></div>
    <div class="cfg-list" id="connected-now">${connectedHTML()}</div>`;
}

const sharedKeys = () => configGet(['api-keys']) || [];

// Named clients: a key per machine or tool. Other tools (Whombat, for one) add their own
// through the same settings API. A row has one thing open at a time.
const NAMED_ROW = { open: null, setup: null, confirm: null, replace: null, rename: null, renameLabel: '', renameError: '', limits: null, limitsError: '', addresses: null };
const namedIdle = () => ({ adding: false, label: '', error: '', created: null, replaced: false, shown: {}, ...NAMED_ROW,
  shared: { naming: null, label: '', error: '', confirm: null } });

// "Whombat · Cybex BV" → "whombat-cybex-bv": lowercase letters, digits and dashes, at most 40, not taken.
function namedClientId(label, taken) {
  const base = String(label).normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40).replace(/-$/, '') || 'client';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base.slice(0, 39 - String(n).length).replace(/-$/, '')}-${n}`;
  return id;
}

const namedName = (client) => (S.private ? '••••••' : client.label || client.id);

// A client's name must be given, and differ from the others' so the Usage page can tell them apart.
function namedLabelProblem(label, list, except = null) {
  if (!label) return 'Enter a name.';
  const same = (x) => x.id !== except && (x.label || x.id).trim().toLowerCase() === label.toLowerCase();
  return list.some(same) ? 'Another client already has this name.' : '';
}

// A name field with its id preview; the add form, and naming a shared key, both use it.
function namedLabelField(id, value, error, help, placeholder, taken) {
  const preview = value.trim() ? namedClientId(value.trim(), taken) : '';
  return `<div class="field"><label for="${id}">Name</label>
      <input id="${id}" type="${S.private ? 'password' : 'text'}" value="${esc(value)}" placeholder="${S.private ? 'Name hidden' : placeholder}" maxlength="100" autocomplete="off" style="font-family:var(--sans)" aria-describedby="${id}-help ${id}-error"${error ? ' aria-invalid="true"' : ''}${S.config.busy ? ' disabled' : ''}>
      <small id="${id}-help">${help}${S.private || !taken ? '' : `<span id="${id}-id-line"${preview ? '' : ' hidden'}> Id: <span class="mono" id="${id}-id">${esc(preview)}</span></span>`}</small>
      <small id="${id}-error" class="cfg-error" data-error-for="${id}">${esc(error)}</small></div>`;
}

function namedClientsHTML() {
  const c = S.config;
  const n = c.named;
  const list = configGet(['named-clients']) || [];
  const shared = sharedKeys().length > 0;
  const off = c.busy ? ' disabled' : '';
  const used = c.clientUsage?.last_used || {};
  const seen = c.clientUsage?.addresses || {};
  const row = (client) => {
    const name = esc(namedName(client));
    const id = esc(client.id);
    const btn = (act, text, label, cls = '', extra = '') => `<button type="button" class="btn sm${cls}" data-config-act="${act}" data-id="${id}" aria-label="${label}"${extra}${off}>${text}</button>`;
    const shown = !S.private && n.shown[client.id];
    const key = S.private ? secret(client.key) : shown ? client.key : configMask(client.key);
    const from = seen[client.id]?.[0];
    const sub = [used[client.id] ? `Last used ${liveAgo(used[client.id])}${from ? ` from <span class="mono">${esc(addrText(from.ip))}</span>` : ''}` : '', S.private ? '' : `<span class="mono">${id}</span>`].filter(Boolean).join(' · ');
    const limits = namedLimitsText(client);
    const asking = [n.confirm, n.replace, n.rename, n.limits].includes(client.id);
    const last = list.length === 1 && !shared; // removing it turns client keys off
    const confirm = (text, keep, act, doing, done, cls = '') => `<div class="named-confirm${cls}" role="alert"><span>${text}</span>
        <div class="named-actions">${btn(keep, 'Keep', 'Keep', ' ghost')}${btn(act, c.busy ? doing : done, c.busy ? doing : done, ' danger')}</div></div>`;
    return `<div class="named-row"><span class="cell2"><span>${name}</span>${sub ? `<span class="meta named-sub">${sub}</span>` : ''}${limits ? `<span class="meta named-limits">${esc(limits)}</span>` : ''}</span>
      <span class="named-key"><span class="mono">${esc(key)}</span>${copyBtn(client.key, `Copy the key for ${name}`, 'Key copied')}</span>
      ${asking ? '<span></span>' : `<span class="named-btns">${btn('named-setup', 'Set up', `Set up a tool with the key for ${name}`, '', ` aria-expanded="${n.setup === client.id}"`)}${btn('limits-edit', 'Limits', `Limits for ${name}`)}${btn('named-more', 'More', `More for ${name}`, ' ghost', ` aria-expanded="${n.open === client.id}"`)}</span>`}</div>
      ${n.open === client.id && !asking ? `<div class="named-more" role="group" aria-label="More for ${name}">${S.private ? '' : btn('named-reveal', shown ? 'Hide key' : 'Show key', `${shown ? 'Hide' : 'Show'} the key for ${name}`, ' ghost')}${btn('named-addresses', 'Addresses', `Where the key for ${name} was used from`, ' ghost', ` aria-expanded="${n.addresses === client.id}"`)}${btn('named-rename', 'Rename', `Rename ${name}`, ' ghost')}${btn('named-replace', 'Replace key', `Replace the key for ${name}`, ' ghost')}${btn('named-remove', 'Remove', `Remove ${name}`, ' danger')}</div>` : ''}
      ${n.setup === client.id ? `<div class="setup named-setup" data-setup="${id}" data-setup-uid="-row">${setupHTML(client.key, '-row')}</div>` : ''}
      ${n.addresses === client.id ? namedAddressesHTML(client, seen[client.id] || []) : ''}
      ${n.rename === client.id ? namedRenameForm(client) : ''}
      ${n.limits === client.id ? namedLimitsForm(client) : ''}
      ${n.replace === client.id ? confirm(`Replace the key for ${name}? Tools using the current key stop working until you give them the new one.`, 'named-replace-keep', 'named-replace-confirm', 'Replacing…', 'Replace key') : ''}
      ${n.confirm === client.id ? confirm(`Remove ${name}? Tools using its key stop working.${last ? ' Fusebox will accept requests without a key again.' : ''}`, 'named-keep', 'named-confirm', 'Removing…', 'Remove', last ? ' keyless' : '') : ''}`;
  };
  const created = list.find((client) => client.id === n.created);
  const createdName = created && esc(namedName(created));
  const form = `<form class="named-add" id="named-add" novalidate>
      ${namedLabelField('named-label', n.label, n.error, `Shown on the Usage page.${list.length || shared ? '' : '<br>After this, Fusebox only accepts requests with a key.'}`, 'Build server', list.map((client) => client.id))}
      <div class="named-actions"><button type="submit" class="btn primary"${off}>${c.busy ? 'Adding…' : 'Add client'}</button><button type="button" class="btn ghost" data-config-act="named-cancel"${off}>Cancel</button></div></form>`;
  return `${configListHead('Clients', 'A key for each machine or tool, so the Usage page shows each by name and limits can apply. Some tools, such as Whombat, add their own.', 'Add client', `data-config-act="named-add" aria-expanded="${n.adding}"${off}`, true)}
    ${created ? `<div class="named-new" role="status"><span>${n.replaced ? `New key for ${createdName}` : `${createdName} is ready`}</span>
      <span class="meta">${n.replaced ? 'The old key no longer works. Give this one to the tools that used it.' : 'The key works right away. Copy it, or the setup for your tool.'}</span>
      <span class="named-key"><span class="mono">${esc(S.private ? secret(created.key) : created.key)}</span>${copyBtn(created.key, `Copy the key for ${createdName}`, 'Key copied')}</span>
      <div class="setup" data-setup="${esc(created.id)}" data-setup-uid="-new">${setupHTML(created.key, '-new')}</div>
      <div class="named-actions"><button type="button" class="btn sm ghost" data-config-act="named-done">Done</button></div></div>` : ''}
    ${n.adding ? form : ''}
    ${list.length ? `<div class="named-list">${list.map(row).join('')}</div>` : '<p class="cfg-empty">No clients yet.</p>'}`;
}

function namedRenameForm(client) {
  const n = S.config.named;
  const off = S.config.busy ? ' disabled' : '';
  const id = esc(client.id);
  return `<form class="named-add named-inline" id="named-rename-form" data-id="${id}" novalidate>
    ${namedLabelField('named-rename', n.renameLabel, n.renameError, `Shown on the Usage page. The id${S.private ? '' : ` <span class="mono">${id}</span>`} stays, so its history and limits stay with it.`, '', null)}
    <div class="named-actions"><button type="submit" class="btn primary"${off}>${S.config.busy ? 'Saving…' : 'Save name'}</button><button type="button" class="btn ghost" data-config-act="named-rename-cancel" data-id="${id}"${off}>Cancel</button></div></form>`;
}

// Shared keys: any tool may send them, so the Usage page can't tell who did. Naming one
// makes it a client with the same key, so whatever uses it keeps working.
function sharedKeysHTML() {
  const c = S.config;
  const s = c.named.shared;
  const keys = sharedKeys();
  const off = c.busy ? ' disabled' : '';
  const used = c.clientUsage?.shared_last_used || [];
  const last = keys.length === 1 && !(configGet(['named-clients']) || []).length; // removing it turns keys off
  const row = (key, i) => {
    const mask = esc(configMask(key));
    const label = `shared key ${i + 1}`;
    const btn = (act, text, aria, cls = '') => `<button type="button" class="btn sm${cls}" data-config-act="${act}" data-index="${i}"${aria ? ` aria-label="${aria}"` : ''}${off}>${text}</button>`;
    const asking = s.naming === key || s.confirm === key;
    return `<div class="named-row"><span class="mono shared-key">${mask}</span><span class="meta named-used">${used[i] ? `Last used ${liveAgo(used[i])}` : ''}</span>
      ${asking ? '<span></span>' : `<span class="named-btns">${btn('shared-name', 'Name it', `Name ${label}`)}${btn('shared-remove', 'Remove', `Remove ${label}`, ' danger')}</span>`}</div>
      ${s.naming === key ? `<form class="named-add named-inline" id="shared-name-form" novalidate>
        ${namedLabelField('shared-label', s.label, s.error, 'The machine or tool that uses this key. Shown on the Usage page.', 'MacBook Pro', (configGet(['named-clients']) || []).map((x) => x.id))}
        <div class="named-actions"><button type="submit" class="btn primary"${off}>${c.busy ? 'Saving…' : 'Save name'}</button>${btn('shared-cancel', 'Cancel', '', ' ghost')}</div></form>` : ''}
      ${s.confirm === key ? `<div class="named-confirm${last ? ' keyless' : ''}" role="alert"><span>Remove ${mask}? Tools using it stop working.${last ? ' Fusebox will accept requests without a key again.' : ''}</span>
        <div class="named-actions">${btn('shared-keep', 'Keep', '', ' ghost')}${btn('shared-confirm', c.busy ? 'Removing…' : 'Remove', '', ' danger')}</div></div>` : ''}`;
  };
  return `${configListHead('Shared keys', 'Keys without a name, from before each client had its own. The Usage page shows them as “Shared key …”, and limits don’t apply to them. Name each one to make it a client. It keeps its value, so whatever uses it keeps working.')}
    <div class="named-list">${keys.map(row).join('')}</div>`;
}

// Where a named client's key was used from: kept for half a year, most recent first.
function namedAddressesHTML(client, list) {
  const name = esc(namedName(client));
  const day = (iso) => new Date(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
  const rows = list.map((a) => `<div class="addr-row" role="row"><span class="mono" role="cell">${esc(addrText(a.ip))}</span>
    <span class="meta" role="cell">${esc(viaText(a))} · ${plural(a.requests, 'request')} · first ${esc(day(a.first))} · last ${liveAgo(a.last)}</span></div>`).join('');
  const listed = client.addresses?.length ? ` Its key only works from ${esc(client.addresses.map(addrText).join(', '))}.` : ' To refuse other addresses, set Addresses under Limits.';
  return `<div class="named-addresses">${rows ? `<div role="table" aria-label="Addresses the key for ${name} was used from">${rows}</div>` : '<p class="cfg-empty">No requests with this key yet.</p>'}
    <p class="meta">The ${fmt(20)} most recent of the last half year. A new one sends a notification when that is on.${listed}</p></div>`;
}

// Keys and addresses with requests in flight or sockets open, and those seen in the
// last five minutes, for every key (named, shared, or none).
function connectedHTML() {
  const list = S.config.clientUsage?.connected;
  const head = configListHead('Connected now', 'Which machine is using Fusebox: requests in flight, open sockets, and what was seen in the last five minutes.');
  if (!list) return `${head}<p class="cfg-empty">Loading…</p>`;
  if (!list.length) return `${head}<p class="cfg-empty">Nothing in the last five minutes.</p>`;
  const rows = list.map((c) => {
    const busy = [c.in_flight ? `${plural(c.in_flight, 'request')} in flight` : '', c.sockets ? plural(c.sockets, 'open socket') : ''].filter(Boolean).join(' · ');
    return `<div class="named-row conn-row"><span class="cell2"><span>${esc(keyText(c.label))}</span><span class="meta named-sub"><span class="mono">${esc(addrText(c.ip))}</span> · ${esc(viaText(c))}</span></span>
      <span class="meta named-used">${busy ? `<span class="dot s6 ok"></span> ${busy}` : `Last request ${liveAgo(c.last)}`}</span><span></span></div>`;
  }).join('');
  return `${head}<div class="named-list">${rows}</div>`;
}

// Limits a named client's key can have; each is optional and works on its own.
const LIMITS = [
  ['models', 'Models', 'claude-*, gpt-6-astra', 'list', 'Patterns with * wildcards. Empty allows every model.'],
  ['user-agents', 'Programs', 'claude-cli/*, codex*', 'list', 'User-Agent patterns its requests must match. Empty allows any.'],
  ['addresses', 'Addresses', '10.10.0.0/24, 100.101.7.12', 'list', 'Addresses or ranges its requests must come from. Empty allows any. Behind a proxy, Fusebox must be told the real address: see trusted-proxies and proxy-protocol-port.'],
  ['requests-per-minute', 'Requests a minute', '60', 'int', ''],
  ['tokens-per-day', 'Tokens a day', '5000000', 'int', 'Input and output; cache reads don’t count.'],
  ['usd-per-month', 'Dollars a month', '100', 'num', 'Estimated at API list prices.'],
  ['expires', 'Last day', '', 'date', 'The key stops working after this day.'],
];

// "60 a minute · 1.2M of 5M tokens today · $3.10 of $100 this month · until 31 Dec 2026".
function namedLimitsText(client) {
  const u = S.config.clientUsage?.clients?.[client.id];
  const parts = [];
  if (client['requests-per-minute']) parts.push(`${fmt(client['requests-per-minute'])} a minute`);
  if (client['tokens-per-day']) parts.push(`${u ? `${fmt(u.tokens_today)} of ` : ''}${fmt(client['tokens-per-day'])} tokens today`);
  if (client['usd-per-month']) parts.push(`${u ? `$${u.usd_month.toFixed(2)} of ` : ''}$${client['usd-per-month']} this month`);
  if (client.expires) {
    const day = new Date(`${client.expires}T00:00:00`);
    const text = day.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
    parts.push(day < new Date(new Date().toDateString()) ? `expired ${text}` : `until ${text}`);
  }
  if (client.models?.length) parts.push(client.models.join(', '));
  if (client['user-agents']?.length) parts.push(`from ${client['user-agents'].join(', ')}`);
  if (client.addresses?.length) parts.push(`only at ${client.addresses.map(addrText).join(', ')}`);
  return parts.join(' · ');
}

function namedLimitsForm(client) {
  const n = S.config.named;
  const off = S.config.busy ? ' disabled' : '';
  const value = (key, kind) => (kind === 'list' ? (client[key] || []).join(', ') : client[key] ?? '');
  return `<form class="named-limits-form" id="limits-form" data-id="${esc(client.id)}" novalidate>
    <div class="cfg-grid">${LIMITS.map(([key, label, placeholder, kind, help]) => `<div class="field${kind === 'list' ? ' wide' : ''}">
      <label for="limit-${key}">${esc(label)}</label>
      <input id="limit-${key}" name="${key}" type="${kind === 'date' ? 'date' : key === 'addresses' && S.private ? 'password' : kind === 'list' ? 'text' : 'number'}" ${kind === 'int' ? 'min="1" step="1"' : kind === 'num' ? 'min="0.01" step="0.01"' : ''}
        value="${esc(String(value(key, kind)))}" placeholder="${esc(placeholder)}" autocomplete="off" spellcheck="false"${off}>
      ${help ? `<small>${esc(help)}</small>` : ''}</div>`).join('')}</div>
    ${n.limitsError ? `<p class="cfg-error" role="alert">${esc(n.limitsError)}</p>` : ''}
    <div class="named-actions"><button type="submit" class="btn primary"${off}>${S.config.busy ? 'Saving…' : 'Save limits'}</button><button type="button" class="btn ghost" data-config-act="limits-cancel" data-id="${esc(client.id)}"${off}>Cancel</button></div></form>`;
}

function limitsAction(act, id) {
  const n = S.config.named;
  if (act === 'limits-edit') {
    if (namedBlocked()) return;
    Object.assign(n, NAMED_ROW, { limits: id, created: null });
    render();
    return $('#limit-models')?.focus();
  }
  if (act === 'limits-cancel') {
    Object.assign(n, NAMED_ROW);
    render();
    return $(`[data-config-act="limits-edit"][data-id="${CSS.escape(id)}"]`)?.focus();
  }
}

async function saveLimits(form) {
  const n = S.config.named;
  const list = configGet(['named-clients']) || [];
  const client = list.find((x) => x.id === form.dataset.id);
  if (!client || S.config.busy || namedBlocked()) return;
  const updated = { id: client.id, label: client.label, key: client.key };
  for (const [key, label, , kind] of LIMITS) {
    const raw = form.elements[key].value.trim();
    if (!raw) continue;
    if (kind === 'list') updated[key] = raw.split(',').map((p) => p.trim()).filter(Boolean);
    else if (kind === 'date') updated[key] = raw;
    else {
      const num = Number(raw);
      if (!(num > 0) || (kind === 'int' && !Number.isInteger(num))) {
        n.limitsError = `${label}: enter ${kind === 'int' ? 'a whole number' : 'an amount'} above 0, or leave it empty.`;
        render();
        return form.ownerDocument.getElementById(`limit-${key}`)?.focus();
      }
      updated[key] = num;
    }
  }
  await saveClients({ 'named-clients': list.map((x) => (x === client ? updated : configClone(x))) }, `Saved the limits for ${namedName(client)}.`,
    () => Object.assign(n, { limits: null, limitsError: '' }));
}

// What named clients used today and this month, for their limits, and when each key was
// last used; refreshed now and then while the page is open.
async function loadClientUsage(force = false) {
  const c = S.config;
  if (!force && c.clientUsageAt && Date.now() - c.clientUsageAt < 15000) return;
  c.clientUsageAt = Date.now();
  try {
    c.clientUsage = await api('/clients/usage');
    // Forms here keep what is typed in them: the lists wait for the next render.
    if (S.route !== 'config' || c.section !== 'clients' || c.named.limits || document.activeElement?.closest('#named-clients form, #shared-keys form')) return;
    patch('named-clients', namedClientsHTML);
    patch('shared-keys', sharedKeysHTML);
    patch('connected-now', connectedHTML);
  } catch { /* the lists still show without their usage */ }
}

// Other sections' unsaved edits would be lost when this saves, so one kind of edit at a time.
function namedBlocked() {
  if (!configDirty()) return false;
  S.config.msg = { kind: 'err', text: 'Save or discard the changes in the other sections first.' };
  configUpdateFoot();
  return true;
}

async function saveClients(changes, text, onSaved) {
  const c = S.config;
  c.busy = true; c.msg = null; render();
  try {
    const result = await configPatch(changes);
    onSaved();
    c.msg = { kind: 'ok', text: configSavedText(result, text) };
    refreshAccounts();
    loadClientUsage(true);
  } catch (e) { c.msg = { kind: 'err', text: e.message }; }
  c.busy = false; render();
}

const namedEdit = (list, client, change) => ({ 'named-clients': list.map((x) => (x === client ? { ...configClone(x), ...change } : configClone(x))) });

async function addNamedClient() {
  const n = S.config.named;
  const label = n.label.trim();
  if (S.config.busy || namedBlocked()) return;
  const list = configGet(['named-clients']) || [];
  const problem = namedLabelProblem(label, list);
  if (problem) {
    n.error = problem;
    render(); $('#named-label')?.focus(); return;
  }
  const client = { id: namedClientId(label, list.map((x) => x.id)), label, key: generateClientKey() };
  await saveClients({ 'named-clients': [...configClone(list), client] }, `Added ${namedName(client)}.`,
    () => Object.assign(n, NAMED_ROW, { adding: false, label: '', error: '', created: client.id, replaced: false }));
  if (n.created === client.id) $('.named-new .copy')?.focus();
}

async function renameNamedClient(form) {
  const n = S.config.named;
  const list = configGet(['named-clients']) || [];
  const client = list.find((x) => x.id === form.dataset.id);
  const label = n.renameLabel.trim();
  if (!client || S.config.busy || namedBlocked()) return;
  const problem = label === client.label ? '' : namedLabelProblem(label, list, client.id);
  if (problem) {
    n.renameError = problem;
    render(); $('#named-rename')?.focus(); return;
  }
  const done = () => Object.assign(n, NAMED_ROW);
  if (label === client.label) { done(); render(); } else await saveClients(namedEdit(list, client, { label }), `Renamed it ${namedName({ label })}.`, done);
  if (!n.rename) $(`[data-config-act="named-more"][data-id="${CSS.escape(client.id)}"]`)?.focus();
}

// A new key under the same id, so the client keeps its name, limits and history.
async function replaceNamedKey(id) {
  const n = S.config.named;
  const list = configGet(['named-clients']) || [];
  const client = list.find((x) => x.id === id);
  if (!client || namedBlocked()) return;
  await saveClients(namedEdit(list, client, { key: generateClientKey() }), `Replaced the key for ${namedName(client)}.`,
    () => Object.assign(n, NAMED_ROW, { adding: false, created: id, replaced: true }));
  if (n.created === id) $('.named-new .copy')?.focus();
}

async function removeNamedClient(id) {
  const list = configGet(['named-clients']) || [];
  const client = list.find((x) => x.id === id);
  if (!client || namedBlocked()) return;
  await saveClients({ 'named-clients': list.filter((x) => x !== client) }, `Removed ${namedName(client)}.`,
    () => Object.assign(S.config.named, NAMED_ROW, { created: null }));
  if (!S.config.named.confirm) $('[data-config-act="named-add"]')?.focus();
}

function namedAction(act, id) {
  const n = S.config.named;
  const focus = (selector) => $(selector)?.focus();
  const byId = (a) => `[data-config-act="${a}"][data-id="${CSS.escape(id)}"]`;
  // Opens one thing in a row and closes whatever else was open there.
  const row = (state, selector) => { Object.assign(n, NAMED_ROW, state); render(); return focus(selector); };
  if (act === 'named-add') {
    if (namedBlocked()) return;
    Object.assign(n, { adding: true, error: '', created: null }); render(); return focus('#named-label');
  }
  if (act === 'named-cancel') { Object.assign(n, { adding: false, label: '', error: '' }); render(); return focus('[data-config-act="named-add"]'); }
  if (act === 'named-done') { n.created = null; render(); return focus('[data-config-act="named-add"]'); }
  if (act === 'named-more') return row({ open: n.open === id ? null : id }, byId('named-more'));
  if (act === 'named-setup') { n.created = null; return row({ setup: n.setup === id ? null : id }, byId('named-setup')); }
  if (act === 'named-addresses') return row({ open: id, addresses: n.addresses === id ? null : id }, byId('named-addresses'));
  if (act === 'named-reveal') { n.shown[id] = !n.shown[id]; render(); return focus(byId('named-reveal')); }
  if (act === 'named-rename-cancel') return row({ open: id }, byId('named-rename'));
  if (act === 'named-replace-keep') return row({ open: id }, byId('named-replace'));
  if (act === 'named-keep') return row({ open: id }, byId('named-remove'));
  if (act === 'named-replace-confirm') return replaceNamedKey(id);
  if (act === 'named-confirm') return removeNamedClient(id);
  if (namedBlocked()) return;
  if (act === 'named-rename') {
    const client = (configGet(['named-clients']) || []).find((x) => x.id === id);
    row({ rename: id, renameLabel: client?.label || id });
    return $('#named-rename')?.select();
  }
  if (act === 'named-replace') return row({ replace: id }, byId('named-replace-keep'));
  if (act === 'named-remove') { n.created = null; return row({ confirm: id }, byId('named-keep')); }
}

async function nameSharedKey() {
  const s = S.config.named.shared;
  const label = s.label.trim();
  if (S.config.busy || namedBlocked()) return;
  const keys = sharedKeys();
  const list = configGet(['named-clients']) || [];
  const problem = namedLabelProblem(label, list) || (keys.includes(s.naming) ? '' : 'This key changed in the file. Reload, then try again.');
  if (problem) {
    s.error = problem;
    render(); $('#shared-label')?.focus(); return;
  }
  const client = { id: namedClientId(label, list.map((x) => x.id)), label, key: s.naming };
  // One save moves the key, so there's no moment it doesn't work.
  const named = list.some((x) => x.key === client.key) ? configClone(list) : [...configClone(list), client];
  await saveClients({ 'api-keys': keys.filter((k) => k !== client.key), 'named-clients': named },
    `Named it ${namedName(client)}. Whatever uses the key keeps working.`, () => Object.assign(s, { naming: null, label: '', error: '' }));
  if (!s.naming) $(`[data-config-act="named-more"][data-id="${CSS.escape(client.id)}"]`)?.focus();
}

async function removeSharedKey(key) {
  const s = S.config.named.shared;
  if (namedBlocked() || !sharedKeys().includes(key)) return;
  await saveClients({ 'api-keys': sharedKeys().filter((k) => k !== key) }, 'Removed the shared key.', () => { s.confirm = null; });
  if (!s.confirm) ($('[data-config-act="shared-name"]') || $('[data-config-act="named-add"]'))?.focus();
}

function sharedAction(act, index) {
  const s = S.config.named.shared;
  const key = sharedKeys()[index];
  const focus = (a) => $(`[data-config-act="${a}"][data-index="${index}"]`)?.focus();
  if (act === 'shared-cancel') { Object.assign(s, { naming: null, label: '', error: '' }); render(); return focus('shared-name'); }
  if (act === 'shared-keep') { s.confirm = null; render(); return focus('shared-remove'); }
  if (act === 'shared-confirm') return removeSharedKey(key);
  if (key == null || namedBlocked()) return;
  if (act === 'shared-name') { Object.assign(s, { naming: key, label: '', error: '', confirm: null }); render(); return $('#shared-label')?.focus(); }
  if (act === 'shared-remove') { Object.assign(s, { confirm: key, naming: null }); render(); return focus('shared-keep'); }
}

document.addEventListener('submit', (e) => {
  const form = e.target;
  const save = { 'named-add': addNamedClient, 'named-rename-form': renameNamedClient, 'shared-name-form': nameSharedKey, 'limits-form': saveLimits }[form.id];
  if (!save) return;
  e.preventDefault();
  save(form);
});

// Name fields keep their text in state, so a re-render keeps it; the id preview follows.
const NAME_FIELDS = { 'named-label': [(n) => n, 'label', 'error'], 'named-rename': [(n) => n, 'renameLabel', 'renameError'], 'shared-label': [(n) => n.shared, 'label', 'error'] };
document.addEventListener('input', (e) => {
  const field = NAME_FIELDS[e.target.id];
  if (!field) return;
  const [state, text, error] = field;
  const s = state(S.config.named);
  const id = e.target.id;
  s[text] = e.target.value;
  if (s[error]) { s[error] = ''; e.target.removeAttribute('aria-invalid'); $(`#${id}-error`).textContent = ''; }
  const label = s[text].trim();
  const out = $(`#${id}-id`);
  if (out) out.textContent = label ? namedClientId(label, (configGet(['named-clients']) || []).map((x) => x.id)) : '';
  $(`#${id}-id-line`)?.toggleAttribute('hidden', !label);
});

function configServerHTML() {
  const tls = !!configGet(['tls', 'enable']);
  return `<div class="cfg-grid">
    ${configField(['host'], 'Bind address', { restart: true, placeholder: '127.0.0.1' })}
    ${configField(['port'], 'Port', { type: 'number', min: 1, max: 65535, required: true, restart: true })}
    ${configField(['auth-dir'], 'Credentials directory', { required: true, wide: true, help: 'OAuth credential files are read from this directory.' })}
    <div class="cfg-divider wide"></div>
    ${configSwitch(['tls', 'enable'], 'HTTPS', 'Serve the endpoint and the dashboard over TLS.', true)}
    ${configField(['tls', 'cert'], 'Certificate path', { required: tls, disabled: !tls, placeholder: '/etc/fusebox/cert.pem' })}
    ${configField(['tls', 'key'], 'Private key path', { required: tls, disabled: !tls, placeholder: '/etc/fusebox/key.pem' })}
  </div>`;
}

function configAccessHTML() {
  const clients = '<button type="button" class="linkbtn" data-config-act="section" data-section="clients">Clients</button>';
  return `<div class="cfg-grid"><div class="cfg-list wide">${configListHead('Client keys', '')}
      <p class="cfg-description">Each machine or tool has its own key.${sharedKeys().length ? ' Shared keys from before stay listed until you name them.' : ''} Add and manage them under ${clients}.</p></div>
    <div class="cfg-divider wide"></div>
    ${configField(['management-key'], 'Dashboard key', { type: 'password', wide: true, help: 'Protects the dashboard and management API. With no key, access is limited to localhost.' })}
    ${configChoice(['management-allow-remote'], 'Remote dashboard access', [[null, 'Default'], [true, 'Allow'], [false, 'Localhost only']], 'Remote access requires a dashboard key, even when allowed.')}</div>`;
}

function configRoutingHTML() {
  const strategy = configGet(['routing']);
  const affinity = !!configGet(['session-affinity']);
  const what = affinity ? 'new sessions' : 'requests';
  return `<div class="cfg-grid">
    ${configCards(['routing'], 'Account selection', [
      ['least-used', 'Most quota remaining', `Send ${what} to the account with the most subscription quota remaining. Falls back to round robin when quota is unavailable.`],
      ['smart-quota', 'Smart quota balancing', `Prefer accounts with banked resets nearing expiry, then the earliest weekly renewal. Below the reserves, prefer other accounts while sessions are active; idle accounts can use their remaining quota. Close deadlines compete on quota and load.`],
      ['round-robin', 'Round robin', `Rotate ${what} across available accounts.`],
      ['fill-first', 'Fill first', `Send ${what} to the first available account until it cannot serve them, then the next.`],
    ])}
    ${strategy === 'smart-quota' ? configField(['five-hour-reserve-percent'], '5-hour reserve for existing sessions (%)', { type: 'number', min: 0, max: 100, required: true,
      help: `Below this level (or 5% weekly quota left), prefer other accounts for ${what} while this account has an in-flight request or a session used in the last five minutes. Idle accounts can admit a session to drain remaining quota. If every account needs its reserve, usable quota remains available. Default: 30%. Set 0 to turn off the 5-hour reserve.` }) : ''}
    ${strategy === 'smart-quota' ? configField(['banked-reset-policy', 'expiry-priority-hours'], 'Prefer resets expiring within (hours)', { type: 'number', min: 0, max: 8760, required: true,
      help: 'Requires Banked resets under Connections. Prefer new work on an account whose saved reset expires within this time, while preserving active-session reserves. Default: 168 hours (7 days). 0 disables the preference.' }) : ''}
    ${strategy === 'smart-quota' ? configSwitch(['banked-reset-policy', 'auto-redeem'], 'Automatically use expiring resets', 'Allows Fusebox to spend saved resets within the expiry preference window. It refreshes provider data first and normally waits for 95% weekly use. One hour before expiry it redeems regardless of usage, including 0%, even if quota will renew soon. Provider eligibility still applies. Uncertain outcomes stop automatic spending. Off by default.') : ''}
    ${configField(['request-retry'], 'Account attempts', { type: 'number', min: 0, max: 4294967295, required: true,
      help: 'Maximum accounts to try before a request fails, up to 10. Zero still tries one account.' })}
    ${configField(['first-token-timeout-seconds'], 'Wait for a stream to start (seconds)', { type: 'number', min: 0, max: 3600, required: true,
      help: 'A stream that sends nothing this long moves to the next account, as do errors that arrive before its first word. 0 turns this off.' })}
    <div class="cfg-divider wide"></div>
    ${configSwitch(['session-affinity'], 'Keep sessions on one account', 'A coding session stays on the account it started on, so its prompt cache keeps working. It moves when that subscription runs out or the account is disabled; while an account is busy, its requests briefly use another.')}
    ${configField(['session-affinity-idle-seconds'], 'Forget idle sessions after (seconds)', { type: 'number', min: 60, required: true, disabled: !affinity, help: '86400 is one day.' })}
    ${configChoice(['prompt-cache', 'scope'], 'Prompt cache key scope', [['session', 'Session'], ['client', 'Client'], ['workspace', 'Workspace']],
      'Session keeps the existing cache key. Client shares a key across that client’s sessions. Workspace uses the x-fusebox-cache-workspace header, falling back to session when absent. Account selection still follows the session; caller cache keys always win.')}
    ${configField(['prompt-cache', 'namespace'], 'Prompt version', { help: 'Optional cache namespace for a shared prompt or tool schema version. Changing it starts a new derived cache key.' })}
    ${configChoice(['prompt-cache', 'on-incompatible'], 'Incompatible cache hints', [['drop', 'Drop hints'], ['automatic', 'Use provider defaults'], ['reject', 'Reject request']],
      'Drop keeps the current behavior. Provider defaults enables automatic Claude caching after explicit boundaries are lost in translation. Explicit mode without boundaries remains uncached. Reject returns an error before contacting the provider.')}
    <div class="cfg-divider wide"></div>
    ${configSwitch(['force-model-prefix'], 'Require model prefixes', 'Unprefixed requests only use accounts without a prefix. Use prefix/model to select a prefixed account.')}
    <div class="cfg-divider wide"></div>
    ${configGroupsHTML()}
    ${configField(['background-model'], 'Background model', { nullable: true, wide: true, placeholder: 'What Claude Code asks for',
      help: 'Claude Code asks its Haiku model for titles, summaries and quick checks. Name another model or group to send those calls there.' })}
  </div>`;
}

// Model groups: one name for several models. A group is a list (tried in order) or,
// when its new sessions are spread across the models, { models, balance: true }.
const groupModels = (g) => (Array.isArray(g) ? g : g?.models || []);
const groupBalanced = (g) => !Array.isArray(g) && !!g?.balance;

function configGroupsHTML() {
  const groups = configGet(['model-groups']) || {};
  const rows = Object.entries(groups).map(([name, g], i) => {
    const on = groupBalanced(g);
    return `<div class="cfg-row group-row">
      <div class="field"><label class="sr-only" for="group-name-${i}">Group ${i + 1} name</label>
        <input id="group-name-${i}" type="text" value="${esc(name)}" data-group-name="${esc(name)}" placeholder="smart" required autocomplete="off" spellcheck="false"></div>
      <span class="arrow" aria-hidden="true">→</span>
      <div class="field"><label class="sr-only" for="group-models-${i}">Models in group ${i + 1}</label>
        <input id="group-models-${i}" type="text" value="${esc(groupModels(g).join(', '))}" data-group-models="${esc(name)}" placeholder="claude-opus-5-5, gpt-6-astra" required autocomplete="off" spellcheck="false"></div>
      <button type="button" class="fork" role="switch" aria-checked="${on}" data-config-act="group-balance" data-name="${esc(name)}" title="Spread new sessions across the models instead of trying them in order"><span class="switch sm" aria-hidden="true"${on ? ' data-on="true"' : ''}></span>Spread</button>
      <button type="button" class="btn danger" data-config-act="group-remove" data-name="${esc(name)}" aria-label="Remove group ${esc(name)}">Remove</button></div>`;
  });
  return `<div class="cfg-list wide">${configListHead('Model groups', 'One model name for several models. Each model takes over when the one before it can’t answer, or, with Spread, new sessions start on different models.', 'Add group', 'data-config-act="group-add"')}
    ${rows.join('') || '<p class="cfg-empty">No model groups.</p>'}</div>`;
}

function groupAction(act, name) {
  const groups = configGet(['model-groups']) || {};
  if (act === 'group-add') {
    let n = Object.keys(groups).length + 1;
    while (Object.hasOwn(groups, `group-${n}`)) n++;
    configSet(['model-groups'], { ...groups, [`group-${n}`]: [] });
  } else if (act === 'group-remove') {
    configSet(['model-groups'], Object.fromEntries(Object.entries(groups).filter(([k]) => k !== name)));
  } else if (act === 'group-balance') {
    const g = groups[name];
    configSet(['model-groups'], { ...groups, [name]: groupBalanced(g) ? groupModels(g) : { models: groupModels(g), balance: true } });
  }
  S.config.msg = null;
  render();
  if (act === 'group-add') $$('[data-group-name]').pop()?.select();
  else if (act === 'group-balance') $(`[data-config-act="group-balance"][data-name="${CSS.escape(name)}"]`)?.focus();
}

// Typing in a group's row edits it in place, keeping the order of groups.
document.addEventListener('input', (e) => {
  const groups = configGet(['model-groups']) || {};
  const models = e.target.closest('[data-group-models]');
  if (models) {
    const name = models.dataset.groupModels;
    const list = models.value.split(',').map((m) => m.trim()).filter(Boolean);
    configSet(['model-groups'], { ...groups, [name]: groupBalanced(groups[name]) ? { models: list, balance: true } : list });
    S.config.msg = null; configUpdateFoot();
    return;
  }
  const input = e.target.closest('[data-group-name]');
  if (!input) return;
  const old = input.dataset.groupName;
  const name = input.value.trim();
  if (!name || /\s/.test(name) || (name !== old && Object.hasOwn(groups, name))) {
    input.setCustomValidity('Use a unique name without spaces.');
    return;
  }
  input.setCustomValidity('');
  if (name === old) return;
  configSet(['model-groups'], Object.fromEntries(Object.entries(groups).map(([k, v]) => [k === old ? name : k, v])));
  const row = input.closest('.group-row');
  input.dataset.groupName = name;
  row.querySelector('[data-group-models]').dataset.groupModels = name;
  for (const b of row.querySelectorAll('[data-name]')) b.dataset.name = name;
  S.config.msg = null; configUpdateFoot();
});

function configConnectionsHTML() {
  return `<div class="cfg-grid">${configField(['proxy-url'], 'Upstream proxy', { wide: true,
    placeholder: 'socks5://127.0.0.1:1080', help: 'Default proxy for upstream requests. Supports HTTP, HTTPS and SOCKS5; individual providers can override it.' })}
    <div class="cfg-divider wide"></div>
    ${configSwitch(['codex-websockets'], 'Native Codex websockets', 'Keep a native upstream websocket connection for clients using Codex over websockets.')}
    ${configSwitch(['claude-cloak'], 'Claude Code compatibility', 'Make requests through Claude OAuth accounts resemble Claude Code requests for other clients.')}
    ${configSwitch(['banked-resets'], 'Banked resets', 'Show saved Claude and ChatGPT limit resets beside each subscription and let you spend them. Checks every 30 minutes through unofficial provider endpoints.')}
    <div class="cfg-divider wide"></div>
    ${configField(['request-limits', 'text-body-bytes'], 'Text request limit (bytes)', { type: 'number', min: 1, max: 268435456, required: true, help: 'Maximum encoded and decoded body size for text, embedding and token-count requests. Default: 256 MiB.' })}
    ${configField(['request-limits', 'media-body-bytes'], 'Media request limit (bytes)', { type: 'number', min: 1, max: 268435456, required: true, help: 'Maximum encoded and decoded body size for image and video requests, including multipart uploads. Default: 256 MiB.' })}
    ${configField(['request-limits', 'max-in-flight'], 'Concurrent HTTP requests', { type: 'number', min: 0, required: true, help: 'Includes reading the body, provider work and the response stream. Excess requests return 503. Zero is unlimited; WebSockets are separate.' })}
    ${configField(['request-limits', 'max-in-flight-body-bytes'], 'Combined request bodies (bytes)', { type: 'number', min: 0, required: true, help: 'Maximum decoded body bytes across active HTTP requests. Zero is unlimited. Parsed JSON and responses use additional memory.' })}</div>`;
}

// This device's subscription (app.js) acts at once; the events and webhooks are saved to the file.
function configNotificationsHTML() {
  return `<div class="cfg-grid">${pushDeviceHTML()}
    <div class="cfg-divider wide"></div>
    ${configWebhooksHTML()}
    <div class="cfg-divider wide"></div>
    ${configSwitch(['notifications', 'sign-in-expired'], 'Sign-in expired', 'A signed-in account needs signing in again before it can take requests.')}
    ${configSwitch(['notifications', 'provider-exhausted'], 'Provider out of capacity', 'Every account of a provider is used up or unavailable, and again when one is back.')}
    ${configSwitch(['notifications', 'account-used-up'], 'Account limit used up', 'One account used up its 5-hour or weekly limit while others carry on.')}
    ${configSwitch(['notifications', 'account-errors'], 'Account errors', 'An account reports an error, or three or more of its requests fail within an hour.')}
    ${configSwitch(['notifications', 'client-new-address'], 'New client address', 'A client’s key is used from an address it wasn’t used from before, or refused at one its address list doesn’t allow.')}</div>`;
}

// Where a webhook URL delivers, as the server decides it (src/push/webhooks.rs).
function webhookKind(url) {
  let u;
  try { u = new URL(url); } catch { return null; }
  const host = u.hostname.toLowerCase();
  const under = (d) => host === d || host.endsWith(`.${d}`);
  if ((under('discord.com') || under('discordapp.com')) && u.pathname.startsWith('/api/webhooks/')) return 'Discord';
  if (host === 'hooks.slack.com') return 'Slack';
  if (['webhook.office.com', 'logic.azure.com', 'powerplatform.com', 'powerautomate.com'].some(under)) return 'Teams';
  if (host === 'api.telegram.org') return 'Telegram';
  if (host.split('.').includes('ntfy')) return 'ntfy';
  return 'Webhook';
}
const webhookHost = (url) => { try { return new URL(url).host; } catch { return ''; } };

// Chat and webhook destinations: shown by kind and host only (the rest is the secret).
function configWebhooksHTML() {
  const hooks = configGet(['notifications', 'webhooks']) || [];
  const w = S.config.webhooks;
  const kind = webhookKind(w.draft.trim());
  const rows = hooks.map((url, i) => {
    const result = w.results[url];
    return `<div class="hook-row"><span class="cell2"><span>${esc(webhookKind(url) || 'Webhook')}</span><span class="mono meta">${esc(webhookHost(url))}</span></span>
      <span class="hook-result ${result?.ok ? 'ok' : result ? 'err' : ''}" role="status">${esc(result ? result.text : '')}</span>
      <span class="named-btns"><button type="button" class="btn sm" data-config-act="hook-test" data-index="${i}"${w.testing === url ? ' disabled' : ''}>${w.testing === url ? 'Sending…' : 'Send test'}</button>
      <button type="button" class="btn sm danger" data-config-act="remove" data-path="${configPath(['notifications', 'webhooks', i])}" aria-label="Remove the ${esc(webhookKind(url) || 'webhook')} destination">Remove</button></span></div>`;
  }).join('');
  return `<div class="cfg-list wide">${configListHead('Chat and webhooks', 'The same notifications in Discord, Slack, Teams, Telegram, ntfy, or anything that takes a JSON POST. Paste the URL the service gives you; for Telegram, the sendMessage URL with ?chat_id=.')}
    ${rows || '<p class="cfg-empty">No chat or webhook destinations.</p>'}
    <div class="hook-add"><div class="field"><label class="sr-only" for="hook-draft">Webhook URL</label>
      <input id="hook-draft" type="${S.private ? 'password' : 'url'}" value="${esc(w.draft)}" placeholder="https://discord.com/api/webhooks/…" autocomplete="off" spellcheck="false" aria-describedby="hook-draft-note">
      <small id="hook-draft-note" class="${w.error ? 'cfg-error' : ''}">${esc(w.error || (kind ? `Sends as ${kind}.` : ''))}</small></div>
      <button type="button" class="btn" data-config-act="hook-add">Add</button></div></div>`;
}

function webhookAction(act, index) {
  const c = S.config;
  const w = c.webhooks;
  if (act === 'hook-add') {
    const url = w.draft.trim();
    let ok = false;
    try { const u = new URL(url); ok = ['https:', 'http:'].includes(u.protocol) && !!u.hostname; } catch {}
    if (!ok) { w.error = 'Paste the full https:// URL the service gave you.'; render(); return $('#hook-draft')?.focus(); }
    const hooks = configGet(['notifications', 'webhooks']) || [];
    if (!hooks.includes(url)) configSet(['notifications', 'webhooks'], [...hooks, url]);
    Object.assign(w, { draft: '', error: '' });
    c.msg = null; render();
    return $('#hook-draft')?.focus();
  }
  if (act === 'hook-test') {
    const url = (configGet(['notifications', 'webhooks']) || [])[Number(index)];
    if (!url) return;
    w.testing = url; render();
    api('/push/webhooks/test', { method: 'POST', body: JSON.stringify({ url }) })
      .then(() => { w.results[url] = { ok: true, text: 'Sent' }; })
      .catch((e) => { w.results[url] = { ok: false, text: e.message }; })
      .finally(() => { w.testing = null; if (S.route === 'config' && c.section === 'notifications') render(); });
  }
}

document.addEventListener('input', (e) => {
  if (e.target.id !== 'hook-draft') return;
  const w = S.config.webhooks;
  w.draft = e.target.value; w.error = '';
  const note = $('#hook-draft-note');
  const kind = webhookKind(w.draft.trim());
  if (note) { note.className = ''; note.textContent = kind ? `Sends as ${kind}.` : ''; }
});
document.addEventListener('keydown', (e) => {
  if (e.target.id === 'hook-draft' && e.key === 'Enter') { e.preventDefault(); webhookAction('hook-add'); }
});

// Provider ids here are config groups; the logo sprite knows them as account providers.
const configLogo = (provider, name, size = 16) => logo(provider === 'compat' ? 'openai-compat' : provider, name, 'api-key', size);
// "sk-ant-…f3e2", or dots while emails and keys are hidden.
const configMask = (key) => (!key ? 'No key' : S.private ? '••••…••••' : key.length > 12 ? `${key.slice(0, 7)}…${key.slice(-4)}` : '••••');

function configProvidersHTML() {
  const c = S.config;
  const [provider, label, field] = CONFIG_PROVIDERS.find(([p]) => p === c.provider);
  const entries = configGet([field]) || [];
  const compatible = provider === 'compat';
  return `<div class="cfg-tabs" role="group" aria-label="Provider">${CONFIG_PROVIDERS.map(([p, name, f]) => `<button type="button" data-config-act="provider" data-provider="${p}" aria-pressed="${p === provider}">${configLogo(p, null, 14)}${esc(name)}<span class="n">${configGet([f])?.length || 0}</span></button>`).join('')}</div>
    ${entries.length ? entries.map((entry, i) => {
      const path = [field, i];
      const openKey = `${field}:${i}`;
      const title = compatible ? entry.name || `Provider ${i + 1}` : entry.label || `${label} key ${i + 1}`;
      const meta = compatible ? `${(entry['api-keys'] || []).length} key${(entry['api-keys'] || []).length === 1 ? '' : 's'} · ${(entry.models || []).length} models` : configMask(entry['api-key']);
      return `<details class="cfg-provider" data-cfg-open="${openKey}" ${c.opens[openKey] ? 'open' : ''}>
        <summary>${configLogo(provider, entry.name || entry['base-url'])}<span class="nm">${esc(title)}</span>
          ${compatible && entry.disabled ? '<span class="badge">Disabled</span>' : entry.prefix ? `<span class="badge">${esc(entry.prefix)}/</span>` : ''}
          <span class="meta-r">${esc(meta)}</span><span class="cfg-chevron" aria-hidden="true">›</span></summary>
        <div class="cfg-provider-body"><div class="cfg-grid">
          ${compatible ? configField([...path, 'name'], 'Provider name', { required: true, mono: false }) : configField([...path, 'label'], 'Label', { nullable: true, mono: false, placeholder: 'Optional' })}
          ${configField([...path, 'base-url'], 'Base URL', { type: 'url', required: compatible, nullable: !compatible, placeholder: compatible ? 'http://localhost:11434/v1' : 'Provider default' })}
          ${!compatible ? configField([...path, 'api-key'], 'API key', { type: 'password', required: true, wide: true }) : ''}
          ${compatible ? configStrings([...path, 'api-keys'], 'API keys', 'Leave empty for local servers that do not require authentication.', true) : ''}
          ${compatible ? configAliases([...path, 'models']) : ''}
          ${compatible ? configSwitch([...path, 'disabled'], 'Disable provider', 'Keep its configuration while excluding it from requests.') : ''}
          <details class="cfg-advanced wide" data-cfg-open="${openKey}:advanced" ${c.opens[`${openKey}:advanced`] ? 'open' : ''}><summary>Advanced options<span class="cfg-chevron" aria-hidden="true">›</span></summary>
            <div><div class="cfg-grid">${configField([...path, 'prefix'], 'Model prefix', { nullable: true, placeholder: 'team', help: 'Clients use prefix/model to send requests to this key.' })}
              ${configField([...path, 'proxy-url'], 'Proxy override', { nullable: true, placeholder: 'Use default proxy' })}</div>
            ${compatible ? '' : configAliases([...path, 'models'])}
            ${configStrings([...path, 'excluded-models'], 'Excluded models', 'Model names or patterns this provider must not serve. Use * as a wildcard.')}
            ${configHeaders([...path, 'headers'])}</div>
          </details></div>
          <div class="cfg-provider-foot"><button type="button" class="btn danger" data-config-act="remove" data-path="${configPath(path)}">Remove ${esc(title)}</button></div>
        </div></details>`;
    }).join('') : `<div class="cfg-empty-state"><h3>No ${compatible ? 'compatible providers' : `${esc(label)} keys`} configured</h3><p>${compatible ? 'Connect OpenRouter, Ollama, or another OpenAI-compatible endpoint.' : 'Add an API key to use alongside your signed-in accounts.'}</p></div>`}`;
}

function configModelsHTML() {
  const known = CONFIG_OAUTH.map(([p]) => p);
  const sections = ['oauth-model-alias', 'oauth-excluded-models', 'extra-models'];
  const names = [...new Set([...known, ...sections.flatMap((f) => Object.keys(configGet([f]) || {}))])];
  const provider = S.config.oauthProvider;
  const count = (p) => sections.reduce((n, f) => n + (configGet([f, p]) || []).length, 0);
  return `<div class="cfg-tabs" role="group" aria-label="Provider">${names.map((p) => `<button type="button" data-config-act="oauth-provider" data-provider="${esc(p)}" aria-pressed="${p === provider}">${logo(p, null, null, 14)}${esc((CONFIG_OAUTH.find(([id]) => id === p) || [, PROVIDER[p] || p])[1])}<span class="n">${count(p)}</span></button>`).join('')}</div>
    ${configAliases(['oauth-model-alias', provider], true)}
    <div class="cfg-divider"></div>
    ${configStrings(['oauth-excluded-models', provider], 'Excluded models', 'Model names or patterns these accounts must not serve. Use * as a wildcard.')}
    <div class="cfg-divider"></div>
    ${configStrings(['extra-models', provider], 'Added models', 'Models released after this version of Fusebox, served by every account of this provider. Each is removed once Fusebox or the provider’s model list knows it.', false, 'No models added.', { add: 'Add model', item: 'Model', one: 'model', placeholder: 'claude-opus-6' })}`;
}

function configDiagnosticsHTML() {
  return `${configSwitch(['debug'], 'Debug logging', 'Write detailed server logs for troubleshooting.', true)}
    <div class="cfg-divider"></div>
    ${configChoice(['log-format'], 'Log format', [['text', 'Text'], ['json', 'JSON']], 'JSON writes one object per line, for log collectors. Needs a restart.')}
    <div class="cfg-divider"></div>
    <div class="cfg-list">${configListHead('Compatibility notices', S.config.ignored.length ? 'These settings are kept in the file but have no effect in Fusebox.' : 'No ignored CLIProxyAPI features were detected.')}
    ${S.config.ignored.length ? `<ul class="cfg-notices">${S.config.ignored.map((s) => `<li>${esc(s)}</li>`).join('')}</ul>` : ''}</div>`;
}

// The whole file, for settings the sections don't cover. One kind of edit at a time:
// form drafts and YAML drafts never pile up on top of each other.
const rawDirty = () => S.config.raw.text != null && S.config.raw.text !== S.config.raw.saved;

function configYamlHTML() {
  const c = S.config;
  const r = c.raw;
  if (configDirty()) return '<p class="cfg-empty">Save or discard the changes in the other sections first.</p>';
  if (S.private && !c.reveal) {
    return `<div class="cfg-empty-state"><h3>Hidden while emails and keys are hidden</h3><p>config.yaml holds your API keys in plain text.</p>
      <button type="button" class="btn" data-act="reveal-config">Show file</button></div>`;
  }
  if (r.text == null) {
    loadRawConfig();
    return `<div class="skel-rows" aria-busy="true" aria-label="Loading">${'<div class="skel"></div>'.repeat(4)}</div>`;
  }
  return `<label class="sr-only" for="cfg-yaml">config.yaml</label>
    <textarea id="cfg-yaml" class="editor" spellcheck="false" autocapitalize="off" autocomplete="off">${esc(r.text)}</textarea>`;
}

async function loadRawConfig() {
  const r = S.config.raw;
  if (r.loading) return;
  r.loading = true;
  try {
    const res = await api('/config');
    Object.assign(r, { text: res.text, saved: res.text, revision: res.revision });
  } catch (e) { S.config.msg = { kind: 'err', text: e.message }; }
  r.loading = false;
  if (S.route === 'config' && S.config.section === 'yaml') render();
}

// ------------------------------------------------------------ review before saving

// Lines of `before` and `after` as unchanged (' '), removed ('-') and added ('+').
function lineDiff(before, after) {
  const x = before.split('\n');
  const y = after.split('\n');
  let start = 0;
  while (start < x.length && start < y.length && x[start] === y[start]) start++;
  let ex = x.length;
  let ey = y.length;
  while (ex > start && ey > start && x[ex - 1] === y[ey - 1]) { ex--; ey--; }
  const a = x.slice(start, ex);
  const b = y.slice(start, ey);
  const mid = [];
  if (a.length * b.length > 4e6) {
    a.forEach((t) => mid.push(['-', t]));
    b.forEach((t) => mid.push(['+', t]));
  } else {
    // Longest common subsequence, from the end, then walked from the start.
    const lcs = Array.from({ length: a.length + 1 }, () => new Uint32Array(b.length + 1));
    for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
    let i = 0;
    let j = 0;
    while (i < a.length || j < b.length) {
      if (i < a.length && j < b.length && a[i] === b[j]) { mid.push([' ', a[i]]); i++; j++; }
      else if (i < a.length && (j === b.length || lcs[i + 1][j] >= lcs[i][j + 1])) mid.push(['-', a[i++]]);
      else mid.push(['+', b[j++]]);
    }
  }
  return [...x.slice(0, start).map((t) => [' ', t]), ...mid, ...x.slice(ex).map((t) => [' ', t])];
}

// Changed lines with three lines around them; longer unchanged runs fold away.
function diffHTML(ops) {
  const near = ops.map((_, i) => ops.slice(Math.max(0, i - 3), i + 4).some(([t]) => t !== ' '));
  const out = [];
  for (let i = 0; i < ops.length; i++) {
    if (near[i]) { const [t, text] = ops[i]; out.push(`<span class="dl ${t === '+' ? 'add' : t === '-' ? 'del' : ''}"><b aria-hidden="true">${t}</b>${esc(text) || ' '}</span>`); continue; }
    let j = i;
    while (j < ops.length && !near[j]) j++;
    out.push(`<span class="dl fold">⋯ ${plural(j - i, 'unchanged line')}</span>`);
    i = j - 1;
  }
  return out.join('');
}

function openReview(before, after, run) {
  const ops = lineDiff(before, after);
  S.config.review = { ops, run, added: ops.filter(([t]) => t === '+').length, removed: ops.filter(([t]) => t === '-').length };
  render();
  $('[data-config-act="review-save"]')?.focus();
}

function closeReview() {
  S.config.review = null;
  render();
  $('[data-config-act="save"]')?.focus();
}

async function confirmReview() {
  const review = S.config.review;
  if (!review) return;
  S.config.review = null;
  await review.run();
}

function configReviewHTML() {
  const r = S.config.review;
  if (!r) return '';
  const summary = r.added || r.removed ? [r.added && plural(r.added, 'line') + ' added', r.removed && plural(r.removed, 'line') + ' removed'].filter(Boolean).join(', ') : 'No change to the file';
  return `<div class="scrim" data-config-act="review-cancel"></div>
    <div class="review" role="dialog" aria-modal="true" aria-labelledby="review-title">
      <div class="review-head"><h2 id="review-title">Save these changes to config.yaml?</h2><span class="meta">${esc(summary)}</span></div>
      <pre class="diff">${S.private ? '<span class="dl fold">Hidden while emails and keys are hidden.</span>' : diffHTML(r.ops)}</pre>
      <div class="review-foot"><button type="button" class="btn ghost" data-config-act="review-cancel">Keep editing</button><button type="button" class="btn primary" data-config-act="review-save">Save</button></div>
    </div>`;
}

document.addEventListener('keydown', (e) => {
  if (S.config.review && e.key === 'Escape') { e.preventDefault(); closeReview(); }
});

async function saveRawConfig() {
  const c = S.config;
  if (c.busy || !rawDirty()) return;
  openReview(c.raw.saved, c.raw.text, writeRawConfig);
}

async function writeRawConfig() {
  const c = S.config;
  const ta = $('#cfg-yaml');
  const pos = ta ? [ta.selectionStart, ta.scrollTop] : null;
  c.busy = true; c.msg = null; render();
  try {
    const res = await api('/config', { method: 'PUT', body: JSON.stringify({ text: c.raw.text, revision: c.raw.revision }) });
    c.raw.saved = c.raw.text;
    c.raw.revision = res.revision;
    acceptConfig(await api('/config/settings'));
    c.msg = { kind: 'ok', text: res.restart_required ? 'Saved. Some changes need a server restart.' : 'Saved and applied.' };
    refreshAccounts();
  } catch (e) { c.msg = { kind: 'err', text: e.message }; }
  c.busy = false; render();
  const ta2 = $('#cfg-yaml');
  if (ta2 && pos) { ta2.focus(); ta2.selectionStart = ta2.selectionEnd = pos[0]; ta2.scrollTop = pos[1]; }
}

// The footer's message, and whether there is anything to save.
function configFootState() {
  const c = S.config;
  if (c.section === 'yaml') {
    const dirty = rawDirty();
    return { dirty, save: 'Save file', discard: 'Revert',
      msg: c.msg || (dirty ? { kind: '', text: 'The file has unsaved changes.' } : { kind: 'dim', text: 'Saved changes apply immediately.' }) };
  }
  const count = Object.keys(configChanges()).length;
  const error = Object.values(c.errors).find(Boolean);
  const msg = error ? { kind: 'err', text: error }
    : c.msg || (count ? { kind: '', text: `${count} ${count === 1 ? 'setting has' : 'settings have'} unsaved changes.` } : { kind: 'dim', text: 'Changes are saved to your config file.' });
  return { dirty: count > 0, save: 'Save changes', discard: 'Discard changes', msg };
}

function configFootHTML() {
  const c = S.config;
  const f = configFootState();
  const dot = { err: 'err', ok: 'ok', dim: 'dim', '': 'pending' }[f.msg.kind] || 'pending';
  const can = f.dirty && !c.busy;
  return `<span class="dot s7 ${dot}"></span><span class="msg ${f.msg.kind}" role="status">${esc(f.msg.text)}</span><span class="grow"></span>
    <kbd class="kbd">${MOD}S</kbd>
    <button type="button" class="btn ghost" data-config-act="discard" ${can ? '' : 'disabled'}>${mob() ? 'Discard' : f.discard}</button>
    <button type="button" class="btn primary" data-config-act="save" ${can ? '' : 'disabled'}>${c.busy ? 'Saving…' : mob() ? 'Save' : f.save}</button>`;
}

// On phones, and in Clients (which has no settings of its own), the save bar only
// shows while there is something to save or report.
function configFootClass() {
  const f = configFootState();
  if (f.dirty || ['ok', 'err'].includes(f.msg.kind)) return 'cfg-foot';
  return `cfg-foot idle${S.config.section === 'clients' ? ' quiet' : ''}`;
}

function configDirtySections() {
  const out = new Set(Object.keys(configChanges()).map((k) => CONFIG_KEY_SECTION[k] || 'providers'));
  if (rawDirty()) out.add('yaml');
  return out;
}

function configHTML() {
  const c = S.config;
  if (!c.values) {
    if (!c.loading && !c.msg) loadConfig();
    if (c.msg) return `<div class="lock"><h1>Config</h1><p class="err" role="alert">${esc(c.msg.text)}</p><button class="btn" type="button" data-config-act="reload">Try again</button></div>`;
    return skeletonHTML();
  }
  const sections = { server: configServerHTML, access: configAccessHTML, routing: configRoutingHTML,
    connections: configConnectionsHTML, providers: configProvidersHTML, models: configModelsHTML, notifications: configNotificationsHTML,
    diagnostics: configDiagnosticsHTML, yaml: configYamlHTML };
  const dirty = configDirtySections();
  const name = CONFIG_SECTIONS.find(([id]) => id === c.section)[1];
  const restart = { host: 'bind address', port: 'port', tls: 'HTTPS', debug: 'debug logging' };
  const sectionAction = c.section === 'providers' ? `<button type="button" class="btn" data-config-act="add-provider">Add ${c.provider === 'compat' ? 'provider' : 'key'}</button>` : '';
  const reload = `<button type="button" class="btn" data-config-act="reload" ${c.busy ? 'disabled' : ''}>Reload from file</button>`;
  const head = `<div class="cfg-sec-head"><div class="cell2"><h2 class="h-sec">${esc(name)}</h2><p class="meta">${esc(CONFIG_DESCRIPTIONS[c.section])}</p></div>${sectionAction}</div>`;
  // Clients has nothing to save, so it isn't a form.
  const content = c.section === 'clients'
    ? `<section class="card cfg-content" aria-label="Clients">${head}${configClientsHTML()}</section>`
    : `<form id="config-form" class="card cfg-content" aria-label="${esc(name)} settings" novalidate>${head}
      <fieldset ${c.busy ? 'disabled' : ''}>${sections[c.section]()}</fieldset></form>`;
  return `<div class="page-head">${mob() ? `<span class="mono meta ellipsis" style="font-size:12px">${esc(home(c.path))}</span>` : `<div><h1 class="h-page">Config</h1><span class="mono meta" style="font-size:12.5px">${esc(home(c.path))}</span></div>`}<span class="grow"></span>${reload}</div>
    ${c.reloadConfirm ? '<div class="banner plain"><span class="grow">Reloading will discard your unsaved changes.</span><button class="btn sm" type="button" data-config-act="confirm-reload">Reload and discard</button><button class="btn sm ghost" type="button" data-config-act="cancel-reload">Keep editing</button></div>' : ''}
    ${c.restart_fields.length ? `<div class="banner warn" role="status"><span class="dot warn"></span><span class="grow">Restart Fusebox to apply changes to ${esc(c.restart_fields.map((f) => restart[f] || f).join(', '))}.</span></div>` : ''}
    <div class="cfg-chips" role="group" aria-label="Config sections">${CONFIG_SECTIONS.map(([id, label]) => `<button type="button" data-config-act="section" data-section="${id}" ${c.section === id ? 'aria-current="page"' : ''}><span class="cfg-label">${label}</span><span class="cfg-dot${dirty.has(id) ? ' on' : ''}" aria-label="${dirty.has(id) ? 'Unsaved changes' : ''}"></span></button>`).join('')}</div>
    <div class="cfg-layout"><nav class="cfg-nav" aria-label="Config sections">${CONFIG_SECTIONS.map(([id, label]) => `<button type="button" data-config-act="section" data-section="${id}" ${c.section === id ? 'aria-current="page"' : ''}><span class="cfg-label">${label}</span><span class="cfg-dot${dirty.has(id) ? ' on' : ''}"${dirty.has(id) ? ' title="Unsaved changes"' : ''}></span></button>`).join('')}</nav>
    ${content}</div>
    <div class="${configFootClass()}" id="config-foot" aria-live="polite">${configFootHTML()}</div>
    ${configReviewHTML()}`;
}

function acceptConfig(result) {
  const c = S.config;
  Object.assign(c, { values: result.values, saved: configClone(result.values), defaults: result.defaults, revision: result.revision,
    path: result.path, ignored: result.ignored || [], restart_fields: result.restart_fields || [], errors: {}, secrets: {}, reloadConfirm: false,
    raw: { text: null, saved: null, revision: '', loading: false } });
  // In place: a save's follow-up holds on to this object.
  Object.assign(c.named, { confirm: null, replace: null });
  c.named.shared.confirm = null;
}

async function loadConfig() {
  const c = S.config;
  if (c.loading || c.busy) return;
  c.loading = true;
  c.msg = null;
  try { acceptConfig(await api('/config/settings')); }
  catch (e) { c.msg = { kind: 'err', text: e.message }; }
  c.loading = false;
  if (S.route === 'config') render();
}

function configUpdateFoot() {
  const foot = $('#config-foot');
  if (foot) { foot.innerHTML = configFootHTML(); foot.className = configFootClass(); }
  const dirty = configDirtySections();
  for (const dot of $$('.cfg-nav [data-section] .cfg-dot, .cfg-chips [data-section] .cfg-dot')) {
    dot.classList.toggle('on', dirty.has(dot.closest('[data-section]').dataset.section));
  }
}

function configError(path, message) {
  const id = configId(path);
  if (message) S.config.errors[id] = message; else delete S.config.errors[id];
  const field = document.getElementById(id);
  if (field) { if (message) field.setAttribute('aria-invalid', 'true'); else field.removeAttribute('aria-invalid'); }
  const text = document.querySelector(`[data-error-for="${CSS.escape(id)}"]`);
  if (text) text.textContent = message || '';
}

function configValidate() {
  const c = S.config;
  c.errors = {};
  const invalidPaths = [];
  const invalid = (path, message) => { invalidPaths.push(path); configError(path, message); };
  const v = c.values;
  const changed = configChanges();
  for (const input of document.querySelectorAll('[data-header-path]')) {
    if (!input.checkValidity()) invalid([...JSON.parse(input.dataset.headerPath), Number(input.dataset.headerIndex), 'key'], input.validationMessage);
  }
  if ('port' in changed && (!Number.isInteger(v.port) || v.port < 1 || v.port > 65535)) invalid(['port'], 'Enter a port between 1 and 65535.');
  if ('host' in changed && v.host) {
    let valid = false;
    if (v.host.includes(':')) { try { valid = !!new URL(`http://[${v.host}]/`).hostname; } catch {} }
    else { const parts = v.host.split('.'); valid = parts.length === 4 && parts.every((p) => /^(0|[1-9]\d{0,2})$/.test(p) && Number(p) <= 255); }
    if (!valid) invalid(['host'], 'Enter an IPv4 or IPv6 address.');
  }
  if ('auth-dir' in changed && !v['auth-dir']?.trim()) invalid(['auth-dir'], 'Enter a credentials directory.');
  if ('request-retry' in changed && (!Number.isInteger(v['request-retry']) || v['request-retry'] < 0 || v['request-retry'] > 4294967295)) invalid(['request-retry'], 'Enter a nonnegative whole number.');
  if ('five-hour-reserve-percent' in changed && (!Number.isInteger(v['five-hour-reserve-percent']) || v['five-hour-reserve-percent'] < 0 || v['five-hour-reserve-percent'] > 100)) invalid(['five-hour-reserve-percent'], 'Enter a whole percentage between 0 and 100.');
  if ('session-affinity-idle-seconds' in changed && (!Number.isInteger(v['session-affinity-idle-seconds']) || v['session-affinity-idle-seconds'] < 60)) invalid(['session-affinity-idle-seconds'], 'Enter at least 60 seconds.');
  if ('first-token-timeout-seconds' in changed && (!Number.isInteger(v['first-token-timeout-seconds']) || v['first-token-timeout-seconds'] < 0 || v['first-token-timeout-seconds'] > 3600)) invalid(['first-token-timeout-seconds'], 'Enter 0 to 3600 seconds.');
  if ('model-groups' in changed) {
    for (const [name, g] of Object.entries(v['model-groups'] || {})) {
      if (!name || /\s/.test(name) || !groupModels(g).length) { invalid(['model-groups'], `Give the group ${name || 'without a name'} a name without spaces and at least one model.`); break; }
    }
  }
  const checkURL = (path, proxy = false, required = false) => {
    const value = configGet(path);
    if (!value) { if (required) invalid(path, 'Enter a base URL.'); return; }
    // CLIProxyAPI's spelling for "no proxy, not even the default one".
    if (proxy && ['direct', 'none'].includes(value.trim())) return;
    try { const u = new URL(value); if (!u.hostname || !(proxy ? ['http:', 'https:', 'socks5:', 'socks5h:'] : ['http:', 'https:']).includes(u.protocol)) throw new Error(); }
    catch { invalid(path, proxy ? 'Use a complete HTTP, HTTPS or SOCKS5 URL.' : 'Use a complete HTTP or HTTPS URL.'); }
  };
  if ('tls' in changed && v.tls.enable) {
    if (!v.tls.cert?.trim()) invalid(['tls', 'cert'], 'Enter a certificate path.');
    if (!v.tls.key?.trim()) invalid(['tls', 'key'], 'Enter a private key path.');
  }
  if ('proxy-url' in changed) checkURL(['proxy-url'], true);
  for (const [provider, , field] of CONFIG_PROVIDERS) {
    if (!(field in changed)) continue;
    (v[field] || []).forEach((entry, i) => {
      const path = [field, i];
      if (provider === 'compat') {
        if (!entry.name?.trim()) invalid([...path, 'name'], 'Enter a provider name.');
        (entry['api-keys'] || []).forEach((key, j) => { if (!key.trim()) invalid([...path, 'api-keys', j], 'Enter an API key or remove this row.'); });
      } else if (!entry['api-key']?.trim()) invalid([...path, 'api-key'], 'Enter an API key.');
      checkURL([...path, 'base-url'], false, provider === 'compat');
      checkURL([...path, 'proxy-url'], true);
      (entry.models || []).forEach((model, j) => { if (!model.name?.trim()) invalid([...path, 'models', j, 'name'], 'Enter an upstream model name.'); });
      (entry['excluded-models'] || []).forEach((pattern, j) => { if (!pattern.trim()) invalid([...path, 'excluded-models', j], 'Enter a model pattern or remove this row.'); });
      for (const [name, value] of Object.entries(entry.headers || {})) {
        if (!/^[!#$%&'*+.^_`|~\w-]+$/.test(name) || /[\r\n]/.test(value)) invalid([...path, 'headers', name], 'Use a valid HTTP header name and value.');
      }
    });
  }
  if ('oauth-model-alias' in changed) for (const [provider, aliases] of Object.entries(v['oauth-model-alias'] || {})) aliases.forEach((alias, i) => {
    for (const key of ['name', 'alias']) if (!alias[key]?.trim()) invalid(['oauth-model-alias', provider, i, key], `Enter ${key === 'name' ? 'an upstream model name' : 'an alias'}.`);
  });
  if ('oauth-excluded-models' in changed) for (const [provider, patterns] of Object.entries(v['oauth-excluded-models'] || {})) patterns.forEach((pattern, i) => {
    if (!pattern.trim()) invalid(['oauth-excluded-models', provider, i], 'Enter a model pattern or remove this row.');
  });
  if ('extra-models' in changed) for (const [provider, models] of Object.entries(v['extra-models'] || {})) models.forEach((model, i) => {
    if (!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(model.trim())) invalid(['extra-models', provider, i], 'Enter a model id like claude-opus-6, or remove this row.');
  });
  if (Object.keys(c.errors).length) {
    const id = Object.keys(c.errors)[0];
    const path = invalidPaths[0];
    if (path[0].startsWith('oauth-')) { c.section = 'models'; c.oauthProvider = path[1]; }
    else {
      const provider = CONFIG_PROVIDERS.find(([, , field]) => path[0] === field);
      if (provider) { c.section = 'providers'; c.provider = provider[0];
        for (let i = 0; i < v[provider[2]].length; i++) { c.opens[`${provider[2]}:${i}`] = true; c.opens[`${provider[2]}:${i}:advanced`] = true; }
      } else if (['request-retry', 'session-affinity-idle-seconds', 'five-hour-reserve-percent', 'first-token-timeout-seconds', 'model-groups', 'background-model'].includes(path[0])) c.section = 'routing';
      else if (path[0] === 'proxy-url') c.section = 'connections';
      else c.section = 'server';
    }
    render(); document.getElementById(id)?.focus(); return false;
  }
  return true;
}

// Sends changes with the revision they were made against; the server refuses (409) if the file moved on.
async function configPatch(changes) {
  const result = await api('/config/settings', { method: 'PATCH', body: JSON.stringify({ revision: S.config.revision, changes }) });
  acceptConfig(result);
  // The main line and setup snippets read the keys from the overview; keep them current until it refreshes.
  Object.assign(S.overview, { client_keys: result.values['api-keys'] || [], named_clients: (result.values['named-clients'] || []).length });
  return result;
}

function configSavedText(result, text = 'Saved and applied.') {
  return [result.restart_required ? 'Saved. Some changes need a server restart.' : text,
    result.rewritten ? 'This file\'s layout couldn\'t be kept, so its comments were removed; the original is in config.yaml.bak.' : ''].filter(Boolean).join(' ');
}

async function saveConfig() {
  const c = S.config;
  if (c.section === 'yaml') return saveRawConfig();
  if (c.busy || c.review || !configDirty() || !configValidate()) return;
  const changes = configChanges();
  c.busy = true; c.msg = null; render();
  try {
    const preview = await api('/config/settings', { method: 'PATCH', body: JSON.stringify({ revision: c.revision, changes, dry_run: true }) });
    c.busy = false;
    openReview(preview.before, preview.after, () => commitConfig(changes));
  } catch (e) {
    c.busy = false; c.msg = { kind: 'err', text: e.message }; render();
  }
}

async function commitConfig(changes) {
  const c = S.config;
  const oldKey = c.saved['management-key'];
  const newKey = c.values['management-key'];
  c.busy = true; c.msg = null; render();
  try {
    const result = await configPatch(changes);
    c.msg = { kind: 'ok', text: configSavedText(result) };
    // A newly configured dashboard key must also authenticate this browser's next request.
    if (oldKey !== newKey) {
      S.key = newKey;
      if (newKey) store.set('key', newKey); else store.del('key');
      ws?.close();
    }
    refreshAccounts();
  } catch (e) { c.msg = { kind: 'err', text: e.message }; }
  c.busy = false; render();
}

function discardConfig() {
  if (S.config.busy) return;
  if (S.config.section === 'yaml') {
    Object.assign(S.config.raw, { text: S.config.raw.saved });
    S.config.msg = null;
    return render();
  }
  Object.assign(S.config, { values: configClone(S.config.saved), errors: {}, msg: null, secrets: {}, reloadConfirm: false });
  render();
}

function bindConfig() {
  const form = $('#config-form');
  if (!form) return;
  const ta = $('#cfg-yaml');
  ta?.addEventListener('input', () => { S.config.raw.text = ta.value; S.config.msg = null; configUpdateFoot(); });
  ta?.addEventListener('keydown', (e) => {
    if (e.key === 'Tab' && !e.shiftKey) {
      e.preventDefault();
      ta.setRangeText('  ', ta.selectionStart, ta.selectionEnd, 'end');
      S.config.raw.text = ta.value; configUpdateFoot();
    }
  });
  form.addEventListener('submit', (e) => { e.preventDefault(); saveConfig(); });
  form.addEventListener('input', (e) => {
    const input = e.target.closest('[data-cfg]');
    if (!input) return;
    const path = JSON.parse(input.dataset.cfg);
    let value = input.type === 'number' ? (input.value === '' ? null : Number(input.value)) : input.value;
    if (input.dataset.nullable && value === '') value = null;
    if (input.dataset.secret) S.config.secrets[JSON.stringify(path)] = true;
    configSet(path, value);
    configError(path, ''); S.config.msg = null;
    configUpdateFoot();
  });
  const updateHeaderName = (e) => {
    const input = e.target.closest('[data-header-path]');
    if (input) {
      const path = JSON.parse(input.dataset.headerPath);
      const headers = configGet(path);
      const old = input.dataset.headerKey;
      const name = input.value.trim();
      if (!name || (name !== old && Object.hasOwn(headers, name))) {
        input.setCustomValidity('Use a unique, nonempty header name.');
        if (e.type === 'change') input.reportValidity();
        return;
      }
      input.setCustomValidity('');
      if (old !== name) {
        Object.defineProperty(headers, name, { value: headers[old], writable: true, enumerable: true, configurable: true });
        delete headers[old];
        const row = input.closest('.cfg-row');
        const valueInput = row.querySelector('[data-cfg]');
        const oldId = valueInput.id;
        valueInput.dataset.cfg = JSON.stringify([...path, name]);
        valueInput.id = configId([...path, name]);
        row.querySelector(`label[for="${CSS.escape(oldId)}"]`).htmlFor = valueInput.id;
        row.querySelector(`label[for="${CSS.escape(valueInput.id)}"]`).textContent = `${name} value`;
        row.querySelector('[data-error-for]').dataset.errorFor = valueInput.id;
        row.querySelector('[data-error-for]').id = `${valueInput.id}-error`;
        valueInput.setAttribute('aria-describedby', `${valueInput.id}-error`);
        row.querySelector('[data-config-act="remove-header"]').dataset.key = name;
        input.dataset.headerKey = name;
      }
      S.config.msg = null; configUpdateFoot();
    }
  };
  form.addEventListener('input', updateHeaderName);
  form.addEventListener('change', updateHeaderName);
  form.addEventListener('toggle', (e) => {
    if (e.target.dataset.cfgOpen) S.config.opens[e.target.dataset.cfgOpen] = e.target.open;
  }, true);
}

document.addEventListener('keydown', (e) => {
  if (S.route === 'config' && (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') { e.preventDefault(); saveConfig(); }
});

document.addEventListener('click', (e) => {
  const button = e.target.closest('[data-config-act]');
  if (!button || S.config.busy) return;
  const c = S.config;
  const act = button.dataset.configAct;
  const path = button.dataset.path ? JSON.parse(button.dataset.path) : null;
  if (act === 'save') return saveConfig();
  if (act === 'discard') return discardConfig();
  if (act.startsWith('named-')) return namedAction(act, button.dataset.id);
  if (act.startsWith('shared-')) return sharedAction(act, Number(button.dataset.index));
  if (act.startsWith('group-')) return groupAction(act, button.dataset.name);
  if (act.startsWith('hook-')) return webhookAction(act, button.dataset.index);
  if (act.startsWith('limits-')) return limitsAction(act, button.dataset.id);
  if (act === 'review-save') return confirmReview();
  if (act === 'review-cancel') return closeReview();
  if (act === 'reload') {
    if (configDirty() || rawDirty()) { c.reloadConfirm = true; render(); return; }
    return loadConfig();
  }
  if (act === 'confirm-reload') return loadConfig();
  if (act === 'cancel-reload') { c.reloadConfirm = false; render(); return; }
  if (act === 'section') {
    if (c.section === 'yaml' && button.dataset.section !== 'yaml' && rawDirty()) {
      c.msg = { kind: 'err', text: 'Save or revert the file first.' }; configUpdateFoot(); return;
    }
    c.section = button.dataset.section; c.msg = null;
    history.replaceState(null, '', `#/config/${c.section}`);
    render();
    $(`.cfg-nav [data-section="${c.section}"], .cfg-chips [data-section="${c.section}"]`)?.focus({ preventScroll: true });
    if (mob()) $(`.cfg-chips [data-section="${c.section}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    return;
  }
  if (act === 'provider') { c.provider = button.dataset.provider; render(); $(`[data-config-act="provider"][data-provider="${c.provider}"]`)?.focus(); return; }
  if (act === 'oauth-provider') { c.oauthProvider = button.dataset.provider; render(); $(`[data-config-act="oauth-provider"][data-provider="${CSS.escape(c.oauthProvider)}"]`)?.focus(); return; }
  if (act === 'choice') {
    configSet(path, JSON.parse(button.dataset.value)); c.msg = null; render();
    $(`[data-config-act="choice"][data-path="${CSS.escape(button.dataset.path)}"][data-value="${CSS.escape(button.dataset.value)}"]`)?.focus();
    return;
  }
  if (act === 'switch') {
    configSet(path, !configGet(path)); c.msg = null; render(); document.getElementById(configId(path))?.focus(); return;
  }
  if (act === 'reveal') {
    const input = document.getElementById(configId(path));
    const visible = input.type === 'password';
    input.type = visible ? 'text' : 'password'; input.value = configGet(path) || '';
    button.textContent = visible ? 'Hide' : 'Show'; button.setAttribute('aria-label', `${visible ? 'Hide' : 'Show'} key`);
    return;
  }
  if (act === 'clear-secret') { configSet(path, ''); c.secrets[JSON.stringify(path)] = true; }
  if (act === 'add-provider') {
    const [provider, , field] = CONFIG_PROVIDERS.find(([p]) => p === c.provider);
    const list = c.values[field];
    c.opens[`${field}:${list.length}`] = true;
    list.push(provider === 'compat' ? { name: '', 'base-url': '', 'api-keys': [], models: [], headers: {}, 'excluded-models': [], disabled: false }
      : { 'api-key': '', label: null, 'base-url': null, 'proxy-url': null, prefix: null, models: [], headers: {}, 'excluded-models': [] });
  }
  if (act === 'add-string' || act === 'add-alias') {
    const list = configGet(path) || [];
    list.push(act === 'add-string' ? '' : button.dataset.oauth ? { name: '', alias: '', fork: false } : { name: '', alias: null });
    configSet(path, list);
  }
  if (act === 'remove') {
    const parent = path.slice(0, -1); const index = path[path.length - 1];
    configGet(parent).splice(index, 1);
    c.secrets = {};
  }
  if (act === 'add-header') {
    const headers = configGet(path) || {};
    let key = 'X-Header'; let n = 2; while (Object.hasOwn(headers, key)) key = `X-Header-${n++}`;
    headers[key] = ''; configSet(path, headers);
  }
  if (act === 'remove-header') delete configGet(path)[button.dataset.key];
  c.msg = null; c.errors = {}; render();
  if (act.startsWith('add-')) {
    const inputs = formInputsForAddedRow(act, path);
    inputs?.focus();
  }
});

function generateClientKey() {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz0123456789';
  const bytes = crypto.getRandomValues(new Uint8Array(24));
  return `fbx_${[...bytes].map((b) => alphabet[b % alphabet.length]).join('')}`;
}

function formInputsForAddedRow(act, path) {
  if (act === 'add-provider') return $$('.cfg-provider[open]').at(-1)?.querySelector('input');
  if (path) {
    if (act === 'add-header') return document.querySelectorAll('[data-header-path]')[document.querySelectorAll('[data-header-path]').length - 1];
    const index = (configGet(path)?.length || 1) - 1;
    return document.getElementById(configId([...path, index, ...(act === 'add-alias' ? ['name'] : [])]));
  }
}
