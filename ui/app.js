'use strict';

// ---------------------------------------------------------------- storage

// Preferences live under `fusebox.*`. Values saved before the rename are moved over once.
const STORE = 'fusebox.';
const LEGACY_STORE = 'cliproxyapi-rust.'; // the dashboard's prefix before the rename to Fusebox
const store = {
  get(k) { try { return localStorage.getItem(STORE + k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(STORE + k, v); } catch {} },
  del(k) { try { localStorage.removeItem(STORE + k); } catch {} },
};
(function migrateStorage() {
  try {
    for (const k of ['key', 'snippet', 'private', 'quota-display']) {
      const old = localStorage.getItem(LEGACY_STORE + k);
      if (old == null) continue;
      if (localStorage.getItem(STORE + k) == null) localStorage.setItem(STORE + k, old);
      localStorage.removeItem(LEGACY_STORE + k);
    }
  } catch {}
})();

// ---------------------------------------------------------------- state

const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
const view = $('#view');
const PHONE = matchMedia('(max-width: 759px)');
const mob = () => PHONE.matches;
const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const STANDALONE = matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;
const MOD = MAC ? '⌘' : 'Ctrl ';
const ROUTES = ['overview', 'accounts', 'requests', 'usage', 'models', 'config'];

const S = {
  key: store.get('key') || '',
  locked: null, // null | 'key' | 'remote'
  route: 'overview',
  sub: null, // account id (accounts) or section (config)
  overview: null,
  accounts: null,
  requests: [],
  requestsFull: false, // S.requests holds the whole log, not only the newest few
  models: [],
  routes: null,
  routesStale: true, // account or model changes since S.routes was fetched
  faultList: null, // the server's faults (live "faults"); null until the first arrives
  activity: {}, // account id -> { series, sessions, at, fetched }
  load: null, // account id -> { in_flight, sessions } for busy accounts; null while disconnected
  live: 'connecting',
  paused: false,
  pending: 0, // requests that arrived while paused
  fresh: new Map(), // request id -> highlight expiry
  filter: '',
  reqChip: 'all',
  reqAcc: null,
  reqSess: null,
  openReq: null,
  acctFilter: 'all',
  modelFilter: '',
  addModel: null, // { provider, picked, model, msg, err } while the Add model form is open
  drawer: null,
  palette: null, // { q, i, opener }
  faults: false,
  panel: null, // 'connect' | 'key' | 'vertex' | provider being signed in to
  login: null, // { state, provider, url, callback, status, message }
  keyProvider: 'claude',
  snippet: store.get('snippet') || 'claude',
  setupModels: Object.create(null), setupContexts: Object.create(null),
  served: store.get('served') === '1', // this browser has seen Fusebox serve a request
  private: store.get('private') === '1', // hide emails and keys
  quotaDisplay: store.get('quota-display') === 'remaining' ? 'remaining' : 'used',
  confirm: null,
  resets: {}, // account-specific confirmations and errors
  errorClears: {}, // pending error dismissals and action failures, separate from provider errors
  resetModal: null,
  push: { loaded: false, loading: false, status: null, endpoint: null, busy: false, msg: null }, // Config, Notifications
  config: { values: null, saved: null, defaults: {}, revision: '', path: '', ignored: [], restart_fields: [],
    msg: null, busy: false, loading: false, section: 'clients', provider: 'claude', oauthProvider: 'claude',
    errors: {}, opens: {}, secrets: {}, reloadConfirm: false, reveal: false, raw: { text: null, saved: null, revision: '', loading: false }, named: namedIdle(),
    webhooks: { draft: '', error: '', testing: null, results: {} }, review: null, clientUsage: null, clientUsageAt: 0 },
  play: null, // { model, prompt, out, busy, meta, error, abort } while the playground is open
  theme: store.get('theme') || 'system', // 'system' | 'light' | 'dark'
};

// ---------------------------------------------------------------- theme

// Light or dark follows the system unless the palette picks one, per browser.
// `data-shade` says which one shows, for code that draws its own colours.
const LIGHT = matchMedia('(prefers-color-scheme: light)');
const THEMES = { system: 'Match system', light: 'Light', dark: 'Dark' };
function applyTheme() {
  const root = document.documentElement;
  if (S.theme === 'light' || S.theme === 'dark') root.dataset.theme = S.theme; else delete root.dataset.theme;
  const shade = root.dataset.theme || (LIGHT.matches ? 'light' : 'dark');
  root.dataset.shade = shade;
  $('meta[name="theme-color"]')?.setAttribute('content', shade === 'light' ? '#f6f4ef' : '#0b0b0a');
}
function cycleTheme() {
  S.theme = { system: 'light', light: 'dark' }[S.theme] || 'system';
  if (S.theme === 'system') store.del('theme'); else store.set('theme', S.theme);
  applyTheme();
  if (S.route === 'usage') usageRenderChart();
  toast(`Theme: ${THEMES[S.theme]}`);
}
LIGHT.addEventListener('change', () => { applyTheme(); if (S.route === 'usage') usageRenderChart(); });
applyTheme();

const PROVIDER = {
  claude: 'Claude', codex: 'Codex', gemini: 'Gemini', vertex: 'Vertex AI', antigravity: 'Antigravity',
  kimi: 'Kimi', xai: 'Grok', meta: 'Meta', devin: 'Devin', 'openai-compat': 'Compatible',
};
// Accounts you can sign in to: [id, name, what it connects].
const SIGNIN = [
  ['claude', 'Claude', 'Pro, Max or Ultra subscription'],
  ['codex', 'ChatGPT', 'Plus, Pro or Team, for Codex models'],
  ['antigravity', 'Antigravity', 'Google account, Gemini and Claude models'],
  ['xai', 'Grok', 'SuperGrok or X Premium'],
  ['kimi', 'Kimi', 'Kimi Code membership'],
  ['meta', 'Meta', 'Muse Spark'],
  ['devin', 'Devin', 'Devin or Windsurf account'],
  ['vertex', 'Vertex AI', 'Google Cloud service account key'],
];
const LOGIN = {
  claude: { name: 'Claude', intro: 'Connect a Claude Pro, Max or Ultra subscription.', port: 54545, example: 'http://localhost:54545/callback?code=…&state=…' },
  codex: { name: 'ChatGPT', intro: 'Connect a ChatGPT Plus, Pro or Team subscription for Codex models.', port: 1455, example: 'http://localhost:1455/auth/callback?code=…&state=…' },
  antigravity: { name: 'Antigravity', intro: 'Connect a Google account with Antigravity access for Gemini and Claude models.', port: 51121, example: 'http://localhost:51121/oauth-callback?code=…&state=…' },
  devin: { name: 'Devin', intro: 'Connect a Devin or Windsurf account.', example: 'http://127.0.0.1:…/callback?code=…, or a session token' },
  kimi: { name: 'Kimi', intro: 'Connect a Kimi Code membership.' },
  xai: { name: 'Grok', intro: 'Connect a SuperGrok or X Premium subscription.' },
  meta: { name: 'Meta', intro: 'Connect a Meta account for Muse models.' },
};
const DEVICE_CODE = new Set(['kimi', 'xai', 'meta']);
const CLIENT = { openai: 'OpenAI', responses: 'Responses', claude: 'Anthropic', gemini: 'Gemini' };
const ENDPOINT = { openai: '/v1/chat/completions', responses: '/v1/responses', claude: '/v1/messages', gemini: '/v1beta/models' };

// Real provider logos live in the inline sprite (ui/logos.svg). OpenAI-compatible
// groups get their vendor's logo when the name gives it away.
const LOGOS = new Set(['claude', 'codex', 'gemini', 'vertex', 'antigravity', 'xai', 'kimi', 'meta', 'devin']);
const COMPAT_LOGOS = [
  ['openrouter', 'openrouter'], ['ollama', 'ollama'], ['lmstudio', 'lmstudio'], ['deepseek', 'deepseek'], ['groq', 'groq'],
  ['mistral', 'mistral'], ['qwen', 'qwen'], ['dashscope', 'qwen'], ['moonshot', 'kimi'], ['kimi', 'kimi'], ['grok', 'xai'],
  ['xai', 'xai'], ['gemini', 'gemini'], ['anthropic', 'claude'], ['claude', 'claude'], ['openai', 'codex'],
];

function logoId(provider, group, kind) {
  let id = LOGOS.has(provider) ? provider : 'compat';
  if (provider === 'xai' && kind === 'api-key') id = 'xai-api'; // xAI console keys; Grok is the subscription
  if (provider === 'openai-compat' || provider === 'compat') {
    const g = String(group || '').toLowerCase().replace(/[^a-z]/g, '');
    id = (COMPAT_LOGOS.find(([k]) => g.includes(k)) || [, 'compat'])[1];
  }
  return id;
}
function logo(provider, group, kind, size = '') {
  const id = logoId(provider, group, kind);
  return `<svg class="logo logo-${id}${size ? ` s${size}` : ''}" aria-hidden="true" focusable="false"><use href="#logo-${id}"/></svg>`;
}
const acctLogo = (a, size) => logo(a.provider, a.group, a.kind, size);

const svg = (body, view = 16, sw = 1.4) => `<svg viewBox="0 0 ${view} ${view}" fill="none" stroke="currentColor" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${body}</svg>`;
const ICON = {
  copy: svg('<rect x="4.5" y="4.5" width="7.5" height="7.5" rx="1.5"/><path d="M2.5 9.5v-6a1 1 0 0 1 1-1h6"/>', 14, 1.3),
  check: svg('<path d="m3.5 8.5 3 3 6-7"/>', 16, 1.7),
  refresh: svg('<path d="M11.5 7a4.5 4.5 0 1 1-1.3-3.2"/><path d="M11.2 1.8v2.4H8.8"/>', 14, 1.3),
  trash: svg('<path d="M2.5 4h9M5.5 4V2.5h3V4M3.6 4l.6 7.5h5.6l.6-7.5"/>', 14, 1.3),
  back: svg('<path d="M10 3L5 8l5 5"/>', 16, 1.5),
  search: svg('<circle cx="6" cy="6" r="4.25"/><path d="M9.2 9.2l3 3"/>', 14, 1.3),
  eye: svg('<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/>', 16, 1.3),
  eyeOff: svg('<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/><path d="M2.5 13.5l11-11"/>', 16, 1.3),
  external: svg('<path d="M9.5 2.5h4v4M13.5 2.5 7 9M11.5 9.5v3a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3"/>', 16, 1.5),
};
const WORDMARK = $('.brand .wordmark').outerHTML;
const MARK = $('.brand .mark').outerHTML;

// ---------------------------------------------------------------- helpers

const esc = (v) => String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

// Privacy: with the toggle on, emails and the visible ends of API keys are
// replaced before anything reaches the page. Copy buttons still copy the real value.
const EMAIL = /[^\s@<>()"',;:]+@[^\s@<>()"',;:]+\.[a-z]{2,}/gi;
const KEY_ENDS = /\S*…\S*/g;
const HIDDEN = '••••••••';
const hideEmails = (text) => (S.private ? String(text ?? '').replace(EMAIL, '••••••@••••••') : text);
// An account label: an email, a masked key ("sk-ant…f3e2") or a group and key.
const who = (text) => (S.private ? String(hideEmails(text) ?? '').replace(KEY_ENDS, '••••…••••') : text);
// Client keys keep their fbx_ prefix when hidden.
const secret = (key) => (S.private ? (String(key).startsWith('fbx_') ? `fbx_${HIDDEN}` : HIDDEN) : key);
// Signed-in accounts without an email (a Devin username, a file name) are hidden whole.
const acctLabel = (a) => (S.private && a.kind !== 'api-key' && !a.label.includes('@') ? HIDDEN : who(a.label));
// Home directories name the person: /Users/maya/... reads ~/...
const home = (path) => String(path ?? '').replace(/^(\/Users|\/home)\/[^/]+/, '~').replace(/^\/root(?=\/|$)/, '~').replace(/^[A-Za-z]:\\Users\\[^\\]+/, '~');

function fmt(n) {
  n = Number(n) || 0;
  if (n < 1000) return n.toLocaleString('en-US');
  if (n < 1e6) return (n / 1e3).toFixed(n < 1e5 ? 1 : 0).replace(/\.0$/, '') + 'k';
  if (n < 1e9) return (n / 1e6).toFixed(n < 1e7 ? 2 : n < 1e8 ? 1 : 0).replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '') + 'M';
  return (n / 1e9).toFixed(2).replace(/\.?0+$/, '') + 'B';
}

function ms(v) {
  if (v == null) return '—';
  if (v < 1000) return `${v}ms`;
  if (v < 60000) return `${(v / 1000).toFixed(v < 10000 ? 1 : 0)}s`;
  return `${Math.floor(v / 60000)}m ${Math.round((v % 60000) / 1000)}s`;
}

function ago(iso) {
  if (!iso) return 'never';
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  if (s < 10) return 'just now';
  if (s < 60) return `${Math.floor(s)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

// Time left until `iso`: "2d 22h", "1h 41m", "41m", "45s".
function left(iso) {
  const s = Math.max(0, Math.round((Date.parse(iso) - Date.now()) / 1000));
  if (s >= 86400) return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`;
  if (s >= 60) return `${Math.floor(s / 60)}m`;
  return `${s}s`;
}

// Compact quota countdown: at most two units, with no zero suffix or seconds.
function compactLeft(iso) {
  const remaining = Date.parse(iso) - Date.now();
  if (remaining <= 0) return 'now';
  const minutes = Math.floor(remaining / 60000);
  if (minutes >= 1440) {
    const hours = Math.floor((minutes % 1440) / 60);
    return `${Math.floor(minutes / 1440)}d${hours ? ` ${hours}h` : ''}`;
  }
  if (minutes >= 60) {
    const rest = minutes % 60;
    return `${Math.floor(minutes / 60)}h${rest ? ` ${rest}m` : ''}`;
  }
  return minutes ? `${minutes}m` : '<1m';
}

function span(secs) {
  if (secs < 60) return `${Math.floor(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

const clock = (iso) => new Date(iso).toLocaleTimeString('en-GB', { hour12: false });
const hm = (iso) => new Date(iso).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false });
const weekday = (iso) => new Date(iso).toLocaleDateString('en-GB', { weekday: 'short' });
const fullResetTime = (iso) => new Date(iso).toLocaleString('en-GB', {
  weekday: 'long', day: 'numeric', month: 'long', year: 'numeric',
  hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false, timeZoneName: 'short',
});
// Within a day "20:09", otherwise "Fri 14:58".
const when = (iso) => (Date.parse(iso) - Date.now() < 20 * 3600e3 ? hm(iso) : `${weekday(iso)} ${hm(iso)}`);
const liveLeft = (iso, compact = false) => `<span data-until="${esc(iso)}"${compact ? ' data-countdown="compact"' : ''}>${esc(compact ? compactLeft(iso) : left(iso))}</span>`;
const liveAgo = (iso) => `<span data-ago="${esc(iso || '')}">${ago(iso)}</span>`;
const plural = (n, word, many = `${word}s`) => `${fmt(n)} ${n === 1 ? word : many}`;

// Where a request came from (src/source.rs): the address, and how Fusebox knows it.
const addrText = (ip) => (S.private ? HIDDEN : String(ip ?? ''));
function viaText(s) {
  if (!s) return '';
  if (s.via === 'forwarded') return s.login ? `via Tailscale · ${S.private ? HIDDEN : s.login}` : 'via reverse proxy';
  if (s.via === 'proxy_protocol') return 'via TCP forwarder';
  if (s.via === 'unknown') return 'proxy gave no readable address';
  return /^(127\.|::1$)/.test(s.ip || '') ? 'this machine' : 'direct';
}
const sourceText = (s) => (s ? `${addrText(s.ip)} · ${viaText(s)}` : '');
const keyText = (label) => (S.private ? HIDDEN : label || '');
const cap = (s) => String(s || '').charAt(0).toUpperCase() + String(s || '').slice(1);

class ApiError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

async function api(path, opts = {}) {
  const headers = { 'content-type': 'application/json' };
  if (S.key) headers.authorization = `Bearer ${S.key}`;
  const res = await fetch(`/api${path}`, { ...opts, headers });
  if (res.status === 401 || res.status === 403) {
    const lock = res.status === 401 ? 'key' : 'remote';
    if (S.locked !== lock) { S.locked = lock; render(); }
    throw new ApiError(res.status, 'locked');
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(res.status, data.error || res.statusText);
  return data;
}

let toastTimer = 0;
function toast(text) {
  const el = $('#toast');
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 1600);
}

// ---------------------------------------------------------------- data

let overviewAt = 0; // when S.overview was last fetched whole

// The first paint needs only the newest requests (the overview shows six). Pages that
// read the whole log (up to 300) fetch it when they open; see loadRequests.
const FIRST_REQUESTS = 6;
const needsLog = () => ['requests', 'models'].includes(S.route) || (S.route === 'accounts' && !!S.sub) || !!S.drawer;
const requestsPath = (full) => (full ? '/requests' : `/requests?limit=${FIRST_REQUESTS}`);
// A short answer is the whole log; a long one comes from a server that sends it all.
const wholeLog = (list, full) => full || list.length !== FIRST_REQUESTS;

async function loadAll() {
  const full = needsLog();
  const routes = S.route === 'models';
  const [overview, accounts, requests, models, routeData] = await Promise.all([
    api('/overview'), api('/accounts'), api(requestsPath(full)), api('/models'), routes ? api('/routes') : null,
  ]);
  Object.assign(S, { overview, accounts, requests, models, requestsFull: wholeLog(requests, full), locked: null });
  if (routeData) Object.assign(S, { routes: routeData, routesStale: false });
  overviewAt = Date.now();
}

// The whole request log, merged with what arrived live while it was on its way.
let requestsLoading = null;
function loadRequests() {
  if (S.requestsFull || requestsLoading || S.locked) return;
  requestsLoading = api('/requests').then((list) => {
    const seen = new Set(list.map((r) => r.id));
    S.requests = [...S.requests.filter((r) => !seen.has(r.id)), ...list].slice(0, 300);
    S.requestsFull = true;
    requestsArrived();
  }).catch(() => {}).finally(() => { requestsLoading = null; });
}

// After time away: the log again, as much of it as this page has had.
function reloadRequests() {
  const full = S.requestsFull || needsLog();
  api(requestsPath(full)).then((list) => {
    S.requests = list;
    S.requestsFull = wholeLog(list, full);
    requestsArrived();
  }).catch(() => {});
}

// Repaints what reads the request log once more of it is here.
function requestsArrived() {
  if (S.locked || !S.overview) return;
  if (S.route === 'overview') patch('recent', recentHTML);
  else if (S.route === 'requests') refreshRequests();
  else if (S.route === 'accounts' && S.sub) patch('det-reqs', detRequestsHTML);
  else if (S.route === 'models') patch('models-root', modelsBodyHTML);
  if (S.drawer) patch('drawer-reqs', drawerRequestsHTML);
  if (!S.faultList) renderFaults();
}

// Route order follows account state and the model list, so it is fetched while the
// Models page shows and marked stale otherwise.
let routesLoading = null;
function loadRoutes() {
  if (routesLoading || S.locked) return;
  routesLoading = api('/routes').then((routes) => {
    Object.assign(S, { routes, routesStale: false });
    if (S.route === 'models') patchModels();
  }).catch(() => {}).finally(() => { routesLoading = null; });
}

// Account changes ("accounts": state, cooldowns, quota, priority) reload accounts and the
// overview; model changes ("models") reload the model list. Both are debounced together.
const refreshing = { accounts: false, models: false };
let refreshTimer = 0;
function scheduleRefresh(kind) {
  refreshing[kind] = true;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(runRefresh, 250);
}
const refreshAccounts = () => scheduleRefresh('accounts');
const refreshModels = () => scheduleRefresh('models');
async function runRefresh() {
  const { accounts: acc, models: mod } = refreshing;
  refreshing.accounts = refreshing.models = false;
  const routes = S.route === 'models';
  if (!routes) S.routesStale = true;
  try {
    const [accounts, overview, models, routeData] = await Promise.all([
      acc ? api('/accounts') : null, acc ? api('/overview') : null, mod ? api('/models') : null, routes ? api('/routes') : null,
    ]);
    if (accounts) S.accounts = accounts;
    if (overview) { S.overview = overview; overviewAt = Date.now(); }
    if (models) { S.models = models; refreshSetups(); }
    if (routeData) Object.assign(S, { routes: routeData, routesStale: false });
    if (acc) expireActivity();
    refreshViews();
    syncResetModal();
  } catch {}
}

// Activity outlives account changes, but not by more than a minute.
function expireActivity() {
  const old = Date.now() - 60000;
  for (const [id, a] of Object.entries(S.activity)) if (a.fetched < old) delete S.activity[id];
}

// One account's last hour and pinned sessions, fetched when its page or drawer opens.
const activityLoads = {};
function loadActivity(id, force = false) {
  const have = S.activity[id];
  if (!force && have && Date.now() - have.at < 15000) return;
  if (activityLoads[id]) return;
  activityLoads[id] = api(`/accounts/${encodeURIComponent(id)}/activity`).then((r) => {
    S.activity[id] = { ...r, at: Date.now(), fetched: Date.now() };
    if (S.route === 'accounts' && S.sub === id) { patch('det-load', detLoadHTML); patch('det-sess', detSessionsHTML); }
    if (S.drawer === id) patch('drawer-since', drawerSinceHTML);
  }).catch(() => {}).finally(() => { delete activityLoads[id]; });
}

// ---------------------------------------------------------------- live

let ws = null;
let wsDelay = 1000;
let wsRetry = 0;

function connectLive() {
  clearTimeout(wsRetry);
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const q = S.key ? `?key=${encodeURIComponent(S.key)}` : '';
  const sock = new WebSocket(`${proto}://${location.host}/api/live${q}`);
  ws = sock;
  sock.onopen = () => { wsDelay = 1000; setLive('live'); };
  sock.onclose = () => {
    if (sock !== ws) return; // replaced after the app came back from the background
    onLoad(null);
    setLive('offline');
    if (!S.locked) wsRetry = setTimeout(connectLive, wsDelay);
    wsDelay = Math.min(wsDelay * 2, 15000);
  };
  sock.onmessage = (e) => {
    if (sock !== ws) return;
    try { onLive(JSON.parse(e.data)); } catch {}
  };
}

// Phones suspend a backgrounded app, and its socket can come back dead while still
// looking open. After a while away, reconnect and catch up straight away.
let hiddenAt = 0;
function resume() {
  if (document.visibilityState === 'hidden') { hiddenAt = Date.now(); return; }
  const away = hiddenAt ? Date.now() - hiddenAt : 0;
  hiddenAt = 0;
  // Timers pause while hidden; catch up at once.
  tickTimers();
  if (S.locked || !S.overview) return;
  if (ws && ws.readyState <= 1 && away < 30000) {
    if (Date.now() - overviewAt >= 60000) pollOverview();
    return;
  }
  const old = ws;
  wsDelay = 1000;
  connectLive();
  old?.close();
  refreshAccounts();
  refreshModels();
  reloadRequests();
}

function setLive(state) {
  S.live = state;
  renderLive();
}

function onLive(msg) {
  if (msg.type === 'request') return onRequest(msg.data);
  if (msg.type === 'load') return onLoad(msg.data);
  if (msg.type === 'faults') return onFaults(msg.data);
  if (msg.type === 'accounts') return refreshAccounts();
  if (msg.type === 'models') return refreshModels();
  if (msg.type === 'login') return pollLogin();
  if (msg.type === 'tick' && S.overview) {
    const o = S.overview;
    const { totals, active } = msg.data;
    const same = o.active === active && JSON.stringify(o.totals) === JSON.stringify(totals);
    o.totals = totals;
    o.active = active;
    if (!same && S.route === 'overview') patchFigures();
  }
}

// The server's faults, sent whenever they change (see faultItems).
function onFaults(list) {
  S.faultList = faultItems(list);
  renderFaults();
  if (S.route === 'accounts' && !S.sub && !S.locked && S.overview) {
    patch('acct-filters', acctFiltersHTML);
    if (S.acctFilter === 'attention') patch('acct-list', accountListHTML);
  }
}

// The menu's items from the server's list: errors first, then warnings, and last the
// pace warnings (a window that would run out early: worth a look, nothing to do).
// Account faults, the usage database's ("storage", linking to the Usage page) and the
// pace warnings for all of a provider's subscriptions; other provider-wide faults are left
// out, as the menu has always listed accounts.
const FAULT_RANK = { err: 0, warn: 1, pace: 2 };
function faultItems(list) {
  const items = (Array.isArray(list) ? list : []).filter((f) => f.account_id || f.kind === 'storage' || f.kind === 'pace').map((f) => ({
    lvl: f.level === 'err' ? 'err' : 'warn', id: f.account_id || null, title: f.title, signin: f.kind === 'signin',
    provider: f.provider_name || '', label: f.label || '', kind: f.kind || '', detail: f.detail || '', badge: f.badge !== false,
    path: f.path || '',
  }));
  const rank = (f) => FAULT_RANK[f.kind === 'pace' ? 'pace' : f.lvl];
  return items.map((f, i) => [f, i]).sort(([x, i], [y, j]) => rank(x) - rank(y) || i - j).map(([f]) => f);
}

// The app badge counts faults that ask for something to be done; a pace warning
// (badge: false) only asks for attention.
const badgeCount = (list) => list.filter((f) => f.badge !== false).length;

// Puts the lamps and activity cells of accounts whose load changed right, without
// re-rendering whole sections (that would restart every pulse).
function onLoad(load) {
  const prev = S.load || {};
  S.load = load;
  const next = load || {};
  for (const id of new Set([...Object.keys(prev), ...Object.keys(next)])) {
    const a = accountById(id);
    if (!a) continue;
    const was = prev[id] || {};
    const now = next[id] || {};
    const sel = CSS.escape(id);
    if (was.in_flight !== now.in_flight) {
      for (const el of $$(`[data-status="${sel}"]`)) el.outerHTML = statusHTML(a, el.dataset.size || '');
    }
    if (was.in_flight !== now.in_flight || was.sessions !== now.sessions) {
      for (const el of $$(`[data-now="${sel}"]`)) el.innerHTML = nowHTML(a, 'inline' in el.dataset);
    }
  }
  for (const el of $$('[data-subs-meta]')) el.textContent = subsMeta(el.dataset.subsMeta === 'short');
}

function onRequest(log) {
  S.requests.unshift(log);
  if (S.requests.length > 300) S.requests.length = 300;
  if (S.paused) S.pending = Math.min(S.pending + 1, 300);
  const o = S.overview;
  if (o) {
    addRequestCounters(o.totals, log, true);
    const minute = Math.floor(Date.parse(log.ts) / 60000);
    const cutoff = Math.floor(Date.now() / 60000) - 59;
    o.series = o.series.filter((bucket) => bucket.minute >= cutoff);
    let b = o.series.find((bucket) => bucket.minute === minute);
    if (!b && minute >= cutoff) {
      o.series.push((b = { minute, requests: 0, failed: 0, cancelled: 0, usage_missing: 0, usage_partial: 0, tokens: 0, input_tokens: 0, output_tokens: 0, cache_tokens: 0 }));
      o.series.sort((a, b) => a.minute - b.minute);
    }
    if (b) {
      addRequestCounters(b, log);
      b.tokens += log.input_tokens + log.output_tokens + log.cache_tokens;
    }
  }
  if (log.account_id && S.activity[log.account_id]) S.activity[log.account_id].at = 0;
  queueRequest(log);
}

// Coding agents send several requests a second. Each one's data is kept as it arrives;
// the page catches up once a frame however many came (and not at all while hidden).
const requestQueue = [];
let requestFrame = 0;
function queueRequest(log) {
  if (requestQueue.length < 300) requestQueue.push(log); else requestQueue.overflow = true;
  if (!requestFrame) requestFrame = (typeof requestAnimationFrame === 'function' ? requestAnimationFrame : setTimeout)(flushRequests) || 1;
}

function flushRequests() {
  requestFrame = 0;
  const logs = requestQueue.splice(0);
  const many = !!requestQueue.overflow || logs.length > 50;
  requestQueue.overflow = false;
  if (!logs.length) return;
  // New rows glow for a moment once they are on screen.
  const until = Date.now() + 1500;
  for (const log of logs) S.fresh.set(log.id, until);
  setTimeout(() => {
    const body = $('#req-body');
    for (const log of logs) {
      S.fresh.delete(log.id);
      body?.querySelector(`[data-req="${log.id}"]`)?.classList.remove('fresh');
    }
  }, 1500);
  if (S.locked || !S.overview) return;
  // Without the server's list yet, faults still come from the request log.
  if (!S.faultList) renderFaults();
  const on = (id) => id && logs.some((log) => log.account_id === id);
  if (S.route === 'overview') {
    patchMainline();
    patchFigures();
    patchBars();
    patch('recent', recentHTML);
  } else if (S.route === 'requests') {
    if (S.paused) patchChanged('req-pause', pauseHTML);
    else if (many) refreshRequests();
    else insertRequests(logs);
  } else if (S.route === 'accounts' && S.sub && on(S.sub)) {
    patch('det-reqs', detRequestsHTML);
    loadActivity(S.sub);
  } else if (S.route === 'models') {
    patchModelFailures();
  }
  if (on(S.drawer)) { patch('drawer-reqs', drawerRequestsHTML); loadActivity(S.drawer); }
}

function usageState(r) {
  if (['complete', 'partial', 'missing'].includes(r.usage_completeness)) return r.usage_completeness;
  // Older request records do not establish that their observed usage is final.
  return r.input_tokens || r.output_tokens || r.cache_tokens ? 'partial' : 'missing';
}

function addRequestCounters(c, log, withOk = false) {
  c.requests = (c.requests || 0) + 1;
  const outcome = log.status === 499 ? 'cancelled' : log.status >= 400 ? 'failed' : withOk ? 'ok' : null;
  if (outcome) c[outcome] = (c[outcome] || 0) + 1;
  const completeness = usageState(log);
  if (completeness !== 'complete') {
    const field = `usage_${completeness}`;
    c[field] = (c[field] || 0) + 1;
  }
  for (const field of ['input_tokens', 'output_tokens', 'cache_tokens']) c[field] = (c[field] || 0) + (log[field] || 0);
}

function usageCoverageText(c) {
  return [c.usage_missing ? `${fmt(c.usage_missing)} missing` : '', c.usage_partial ? `${fmt(c.usage_partial)} partial` : ''].filter(Boolean).join(' · ');
}

function aggregateTokenText(c, field) {
  const value = c[field] || 0;
  if (!c.usage_missing && !c.usage_partial) return fmt(value);
  return value ? `≥${fmt(value)}` : 'Unknown';
}

function aggregateTokensHTML(c, field) {
  const coverage = usageCoverageText(c);
  const title = coverage ? `Observed tokens only; request usage: ${coverage}. Total consumption is unknown.` : 'Reported provider tokens';
  return `<span title="${esc(title)}">${aggregateTokenText(c, field)}</span>`;
}

function seriesCounters(series) {
  return series.reduce((sum, b) => {
    for (const field of ['requests', 'failed', 'cancelled', 'usage_missing', 'usage_partial', 'input_tokens', 'output_tokens', 'cache_tokens']) sum[field] = (sum[field] || 0) + (b[field] || 0);
    return sum;
  }, {});
}

function usageStats(c) {
  return [['usage_missing', 'Usage missing'], ['usage_partial', 'Usage partial']]
    .filter(([field]) => c[field]).map(([field, label]) => [fmt(c[field]), label]);
}

// ---------------------------------------------------------------- render

// Remembers which control had focus so a re-render can put it back.
function focusSelector(el) {
  if (!el || el === document.body) return null;
  if (el.id) return `#${CSS.escape(el.id)}`;
  const attrs = ['data-act', 'data-id', 'data-config-act', 'data-path', 'data-provider', 'data-section', 'data-resolution', 'data-reset-grant'];
  const parts = attrs.filter((a) => el.hasAttribute(a)).map((a) => `[${a}="${CSS.escape(el.getAttribute(a))}"]`);
  return parts.length ? parts.join('') : null;
}

// The markup each patched element last received, so live updates can skip a repaint
// that would change nothing. A full render makes new elements, which start empty.
const painted = new WeakMap();

function writeHTML(el, html) {
  const active = document.activeElement;
  const sel = el.contains(active) ? focusSelector(active) : null;
  el.innerHTML = html;
  painted.set(el, html);
  if (sel) el.querySelector(sel)?.focus({ preventScroll: true });
}

function patch(id, fn, ...args) {
  const el = document.getElementById(id);
  if (el) writeHTML(el, fn(...args));
}

// Like patch, but leaves the element alone when its markup is unchanged.
function patchChanged(id, fn, ...args) {
  const el = document.getElementById(id);
  if (!el) return;
  const html = fn(...args);
  if (painted.get(el) !== html) writeHTML(el, html);
}

// Swaps one element for new markup, keeping focus on the same control.
function replaceHTML(el, html) {
  const active = document.activeElement;
  const sel = el.contains(active) ? focusSelector(active) : null;
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const next = tpl.content.firstElementChild;
  if (!next) return;
  el.replaceWith(next);
  if (sel) next.querySelector(sel)?.focus({ preventScroll: true });
}

// Re-renders the parts of the current page that show account state.
function refreshViews() {
  renderChrome();
  if (S.locked || !S.overview) return;
  if (S.route === 'overview') {
    patchMainline();
    patchFigures();
    patchBars();
    patch('ov-subs', ovSubsHTML);
    patchChanged('ov-pool', ovPoolHTML);
    patch('ov-other', ovOtherHTML);
    patch('recent', recentHTML);
  } else if (S.route === 'accounts') {
    if (S.sub) patch('det-root', detailBodyHTML);
    else { patch('acct-head', accountHeadHTML); patch('acct-filters', acctFiltersHTML); patch('acct-timeline', resetTimelineHTML); patch('acct-list', accountListHTML); }
  } else if (S.route === 'usage') {
    usageAccountsChanged();
  } else if (S.route === 'models') {
    patchModels();
  }
  renderDrawer();
}

function patchModels() {
  patch('models-head', modelsHeadHTML);
  if (!S.addModel) patch('models-panel', modelsPanelHTML);
  patch('models-root', modelsBodyHTML);
  patch('models-groups', groupsCardHTML);
}

function renderLive() {
  const down = S.live === 'offline';
  for (const el of $$('[data-live]')) {
    el.className = `live${down ? ' down' : ''}`;
    el.innerHTML = down ? '<span class="dot s6 err"></span>Reconnecting' : S.live === 'live' ? '<span class="dot s6 ok"></span>Live' : '';
  }
  const dot = $('#mbar [data-live-dot]');
  if (dot) dot.className = `dot s6 ${down ? 'err' : 'ok'}`;
}

function renderPrivacy() {
  const label = S.private ? 'Show account and client labels, paths and keys' : 'Hide account and client labels, paths and keys';
  for (const btn of $$('[data-privacy]')) {
    btn.innerHTML = S.private ? ICON.eyeOff : ICON.eye;
    btn.setAttribute('aria-pressed', String(S.private));
    btn.setAttribute('aria-label', label);
    btn.title = `${label} ( . )`;
  }
}

function faultsButtonHTML(phone, list = faultsList()) {
  const n = list.length;
  const lvl = list.some((a) => a.lvl === 'err') ? 'err' : n ? 'warn' : 'ok';
  const label = n ? `${n} fault${n === 1 ? '' : 's'}` : 'No faults';
  return `<button class="faults" type="button" data-act="faults" aria-haspopup="true" aria-expanded="${!!S.faults}" aria-controls="faults-menu"${phone ? ` aria-label="${label}" title="Faults"` : ''}><span class="dot s7 ${lvl}"></span><span>${phone ? n : label}</span></button>`;
}

let badge = -1;
function renderFaults() {
  const ready = S.overview && !S.locked;
  const list = ready ? faultsList() : [];
  for (const slot of $$('[data-faults-slot]')) {
    const html = ready ? faultsButtonHTML(slot.closest('.mbar'), list) : '';
    if (painted.get(slot) !== html) writeHTML(slot, html);
  }
  if (S.faults) renderFaultsMenu(list);
  // An installed app shows the fault count on its icon.
  const n = badgeCount(list);
  if (n !== badge && 'setAppBadge' in navigator) {
    badge = n;
    (n ? navigator.setAppBadge(n) : navigator.clearAppBadge()).catch(() => {});
  }
}

const TITLES = { overview: 'Overview', accounts: 'Accounts', requests: 'Requests', usage: 'Usage', models: 'Models', config: 'Config' };

function renderChrome() {
  for (const a of $$('.tabs a, .mtabs a')) {
    if (a.dataset.tab === S.route && !S.locked) a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  }
  for (const k of $$('[data-mod-key]')) k.textContent = `${MOD}${k.dataset.modKey}`;
  const ready = S.overview && !S.locked;
  const searchBtn = `<button class="icon-btn" type="button" data-act="palette" aria-label="Search or run a command" aria-haspopup="dialog">${ICON.search}</button>`;
  const privacyBtn = '<button class="icon-btn" type="button" data-act="privacy" data-privacy></button>';
  const refreshBtn = `<button class="icon-btn" type="button" data-act="refresh-page" aria-label="Refresh page" title="Refresh page" ${pageRefreshBusy ? 'disabled' : ''}>${ICON.refresh}</button>`;
  const down = S.live === 'offline' ? '<span class="live down">Reconnecting</span>' : '';
  let lead;
  if (S.route === 'overview' || S.locked) lead = `${WORDMARK}<span class="dot s6 ${S.live === 'offline' ? 'err' : 'ok'}" data-live-dot aria-hidden="true"></span>`;
  else if (S.route === 'accounts' && S.sub) lead = `<button class="back" type="button" data-act="to-list">${ICON.back}Accounts</button>`;
  else lead = `${MARK}<span class="title">${TITLES[S.route]}</span>`;
  $('#mbar').innerHTML = `${lead}${S.route !== 'overview' ? down : ''}<span class="grow"></span>${ready ? '<span class="faults-slot" data-faults-slot></span>' : ''}${ready ? searchBtn : ''}${refreshBtn}${privacyBtn}`;
  $('.bar .search').hidden = !ready;
  renderPrivacy();
  renderLive();
  renderFaults();
}

function viewClass() {
  if (S.locked || !S.overview) return 'view';
  if (S.route === 'accounts') return S.sub ? 'view detail' : 'view tight list';
  if (S.route === 'requests') return 'view tight list';
  if (S.route === 'models') return 'view tight list';
  if (S.route === 'config') return 'view tight config';
  return 'view';
}

function render() {
  renderChrome();
  view.className = viewClass();
  if (S.locked) { closeResetModal(); closeLayers(); view.innerHTML = lockHTML(); bindLock(); return; }
  if (!S.overview) { view.innerHTML = skeletonHTML(); return; }
  const pages = { overview: overviewHTML, accounts: accountsHTML, requests: requestsHTML, usage: usageHTML, models: modelsHTML, config: configHTML };
  view.innerHTML = (pages[S.route] || overviewHTML)();
  if (S.route === 'config') bindConfig();
  if (S.route === 'usage') bindUsage();
  if (S.route === 'accounts' && S.sub) loadActivity(S.sub);
  renderDrawer();
  syncResetModal();
}

function skeletonHTML() {
  return `<div class="skel-rows" aria-busy="true" aria-label="Loading">${'<div class="skel"></div>'.repeat(6)}</div>`;
}

// ---------------------------------------------------------------- accounts: shared

function provName(a) {
  if (a.provider === 'openai-compat') return a.group || 'Compatible';
  if (a.kind === 'api-key' && a.provider === 'xai') return 'xAI';
  if (a.kind === 'api-key' && a.provider === 'codex') return 'OpenAI';
  return PROVIDER[a.provider] || a.provider;
}

function authName(a) {
  if (a.kind === 'service-account') return 'Service account';
  if (a.kind === 'api-key') return 'API key';
  return DEVICE_CODE.has(a.provider) ? 'Device code' : 'OAuth';
}

// ChatGPT's active subscription SKU distinguishes Pro tiers; usage's `pro` does not.
const CODEX_PLAN_NAMES = new Map([['chatgptprolite', 'Pro 100'], ['chatgptpro', 'Pro 200'], ['chatgptpromax', 'Pro 500']]);
// Claude renamed Max 5x and Max 20x on 30 October 2026; their tier identifiers stayed.
const CLAUDE_PLAN_NAMES = new Map([['max_5x', 'Max'], ['max_20x', 'Ultra']]);
const planName = (a) => (a.provider === 'codex' && CODEX_PLAN_NAMES.get(a.quota?.subscription_plan))
  || (a.provider === 'claude' && (a.quota?.plan_name || CLAUDE_PLAN_NAMES.get(a.quota?.plan)))
  || (a.quota && a.quota.plan ? cap(String(a.quota.plan).replace(/[_-]+/g, ' ')) : '');

// "Claude · OAuth · token valid 5h 11m"
function acctSub(a) {
  const parts = [provName(a), authName(a)];
  if (a.kind === 'oauth' && a.expires_at) {
    const secs = (Date.parse(a.expires_at) - Date.now()) / 1000;
    parts.push(secs > 0 ? `token valid ${span(secs)}` : 'token expired');
  }
  return parts.join(' · ');
}

const accountById = (id) => (S.accounts || []).find((a) => a.id === id);
// Request logs name their account by id; older entries only by provider and label.
const accountOf = (r) => (r.account_id && accountById(r.account_id)) || (S.accounts || []).find((a) => a.provider === r.provider && a.label === r.account);

const SIGNIN_ERR = /invalid_grant|refresh token|sign in again|re-?authenticat|unauthori[sz]ed|\b401\b|token (?:has )?expired|expired token|revoked/i;

// What state an account is in, and the word the dashboard uses for it.
function acctState(a) {
  if (a.disabled) return { cls: 'disabled', dot: 'idle', word: 'Disabled' };
  const cds = Object.entries(a.cooldowns || {}).sort((x, y) => Date.parse(y[1]) - Date.parse(x[1]));
  if (cds.length) {
    const [model, until] = cds[0];
    const kind = (a.cooldown_kinds || {})[model] || 'rate_limit';
    if (kind === 'quota' && model === '*' && a.quota_reset_unknown) return { cls: 'cooling', dot: 'warn', word: 'Quota used up', model, until, kind, resetUnknown: true };
    return { cls: 'cooling', dot: 'warn', word: 'Cooling', model, until, kind, html: `Cooling ${liveLeft(until)}` };
  }
  if (a.last_error) return { cls: 'error', dot: 'err', word: 'Error', signin: a.kind !== 'api-key' && SIGNIN_ERR.test(a.last_error) };
  return { cls: 'ready', dot: 'ok', word: 'Ready' };
}

// Requests in flight on an account right now (0 while the live connection is down).
const inFlight = (a) => (S.load && S.load[a.id] ? S.load[a.id].in_flight : 0);
const liveSessions = (a) => (S.load && S.load[a.id] ? S.load[a.id].sessions : 0);

// A ready account carrying requests lights its lamp and reads Serving.
function statusHTML(a, size = '') {
  const st = acctState(a);
  const n = st.cls === 'ready' ? inFlight(a) : 0;
  const attrs = `data-status="${esc(a.id)}"${size ? ` data-size="${size}"` : ''}`;
  const dot = (cls) => `<span class="dot ${cls}${size ? ` s${size}` : ''}"></span>`;
  if (n) return `<span class="status serving" ${attrs} title="${plural(n, 'request')} in flight">${dot('ok lamp')}<span>Serving ${fmt(n)}</span></span>`;
  return `<span class="status ${st.cls}" ${attrs}>${dot(st.dot)}<span>${st.html || st.word}</span></span>`;
}
const statusWord = (a) => {
  const st = acctState(a);
  if (st.cls === 'cooling') return st.resetUnknown ? st.word : `Cooling ${left(st.until)}`;
  return st.cls === 'ready' && inFlight(a) ? `Serving ${fmt(inFlight(a))}` : st.word;
};

// When an account was last busy, and how many coding sessions are on it.
function nowHTML(a, inline = false) {
  const n = liveSessions(a);
  const last = inFlight(a) ? `<span class="now">${inline ? 'in use now' : 'Now'}</span>` : liveAgo(a.last_used);
  const sessions = n ? `<span class="subs-line" title="Coding sessions active in the last 5 minutes">${plural(n, 'session')}</span>` : '';
  return inline ? `${last}${n ? ` · ${plural(n, 'session')}` : ''}` : `${last}${sessions}`;
}
const modelScope = (m) => (m === '*' ? 'All models' : m);

// Quota colours always describe how much is used, whichever percentage is displayed.
function quotaOf(w, now = Date.now()) {
  if (!w || typeof w.used !== 'number' || !Number.isFinite(w.used)) return null;
  if (w.resets_at && !(Date.parse(w.resets_at) > now)) return null;
  const used = Math.max(0, Math.min(100, w.used));
  return { used, remaining: 100 - used, cls: used >= 95 ? 'err' : used >= 75 ? 'warn' : '', exhausted: used === 100 };
}

// Whole percentages; the ends never round onto 0% or 100% unless they are.
function quotaPercent(value) {
  const rounded = Math.round(value);
  if (value > 0 && rounded === 0) return '<1%';
  if (value < 100 && rounded === 100) return '>99%';
  return `${rounded}%`;
}

const quotaWord = () => (S.quotaDisplay === 'used' ? 'used' : 'left');

function quotaView(q) {
  const mode = S.quotaDisplay;
  const value = q[mode];
  const text = `${quotaPercent(value)} ${quotaWord()}`;
  const status = q.exhausted ? 'Used up' : q.cls === 'err' ? 'Almost used up' : q.cls === 'warn' ? 'Running low' : 'Healthy';
  return { mode, value, text, status };
}

// Subscriptions that can report limits get meters (empty until they do); API keys have none.
const metered = (a) => (a.kind === 'oauth' && ['claude', 'codex'].includes(a.provider))
  || (a.quota?.windows || []).length > 0 || (a.quota?.limits || []).length > 0;

// The tightest live usage window of a kind: short (5-hour) or long (weekly).
function windowOf(a, short) {
  const now = Date.now();
  return ((a.quota && a.quota.windows) || [])
    .filter((w) => !w.model && quotaOf(w, now))
    .filter((w) => /^\d+h$/.test(w.name) === short)
    .sort((x, y) => y.used - x.used)[0];
}

// Verified Claude Ultra and ChatGPT Pro accounts have no 5-hour limit; their usage says so.
const NO_SHORT_LIMIT = 'This account has no five-hour limit; only the weekly limits apply.';
const noShortLimit = (a) => !!a.quota?.no_five_hour_limit && !windowOf(a, true);
// The lists leave such an account's 5-hour meter out; the line under its name says why.
const noShortNote = (a) => (noShortLimit(a) ? ' · no 5h limit' : '');

const windowLabel = (w, short) => (w && !['week', 'day'].includes(w.name) ? w.name.toUpperCase() : short ? '5H' : w && w.name === 'day' ? 'DAY' : 'WK');
const windowTitle = (w) => (/^\d+h$/.test(w.name) ? `${w.name.replace('h', '')}-hour` : w.name === 'day' ? 'Daily' : w.name.startsWith('week') ? (w.name === 'week' ? 'Weekly' : `Weekly · ${w.name.slice(5)}`) : w.name);

// A window's length from its name ("5h", "day", "week"), in seconds.
function windowSeconds(w) {
  const h = /^(\d+)h$/.exec(w?.name || '');
  if (h) return Number(h[1]) * 3600;
  const other = /^(\d+)([dms])$/.exec(w?.name || '');
  if (other) return Number(other[1]) * ({ d: 86400, m: 60, s: 1 }[other[2]]);
  return { day: 86400, week: 7 * 86400 }[w?.name] || null;
}

// How far through its window we are, 0 to 1, when the window says when it resets.
function windowElapsed(w, now = Date.now()) {
  const len = windowSeconds(w);
  if (!len || !w?.resets_at) return null;
  const left = (Date.parse(w.resets_at) - now) / 1000;
  return left >= 0 && left <= len ? 1 - left / len : null;
}

// A usage-limit cooldown lasting until the window renews means the window is used up,
// even while its meter still shows a sliver left.
function windowSpent(a, w) {
  if (!w || !w.resets_at) return false;
  const renews = Date.parse(w.resets_at) - 60000;
  const kinds = a.cooldown_kinds || {};
  return Object.entries(a.cooldowns || {}).some(([model, until]) => kinds[model] === 'quota' && Date.parse(until) >= renews);
}

// 20 segments of 5%, lit for the displayed share, coloured by how much is used. A thin
// mark shows even pace: where usage would be now if spread evenly over the window.
const segsHTML = (w, label, cls = '') => meterHTML(quotaOf(w), windowElapsed(w), label, cls, w && w.resets_at);

// The meter for any quota: `t` places the even-pace mark (0 to 1, or null for none), and
// a reset time makes the page refresh once the window renews.
function meterHTML(q, t, label, cls = '', resetsAt = '') {
  if (!q) return `<span class="segs ${cls}" role="img" aria-label="${esc(label)}: not reported">${'<i></i>'.repeat(20)}</span>`;
  const v = quotaView(q);
  const lit = Math.round(v.value / 5);
  const pace = t == null ? '' : `<b class="pace" style="left:${(v.mode === 'used' ? t : 1 - t) * 100}%" aria-hidden="true"></b>`;
  const paceText = t == null ? '' : ` · even pace ${Math.round(t * 100)}% used by now`;
  return `<span class="segs ${q.cls} ${cls}" role="meter" aria-label="${esc(label)} quota ${v.mode}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(v.value)}" aria-valuetext="${esc(v.text)} · ${v.status}${paceText}"${resetsAt ? ` data-quota-reset="${esc(resetsAt)}"` : ''}>${Array.from({ length: 20 }, (_, i) => (i < lit ? '<i class="on"></i>' : '<i></i>')).join('')}${pace}</span>`;
}

function pctHTML(w) {
  const q = quotaOf(w);
  if (!q) return '<span class="pct dim" title="Quota not reported">–</span>';
  return `<span class="pct ${q.cls}">${quotaPercent(q[S.quotaDisplay])}</span>`;
}

// One meter row: "5H [segments] 47% ↺ 2h 14m"; full reset time on hover.
function meterRowHTML(a, short) {
  const w = windowOf(a, short);
  const label = windowLabel(w, short);
  const reset = w && w.resets_at ? `↺ ${liveLeft(w.resets_at, true)}` : '';
  const title = w && w.resets_at ? ` title="${esc(windowTitle(w))} window resets ${esc(fullResetTime(w.resets_at))}"` : '';
  return `<div class="mrow"><span class="capsm">${esc(label)}</span>${segsHTML(w, `${windowTitle(w || { name: short ? '5h' : 'week' })}`)}${pctHTML(w)}<span class="rst"${title}>${reset}</span></div>`;
}
const meterRowsHTML = (a) => `${noShortLimit(a) ? '' : meterRowHTML(a, true)}${meterRowHTML(a, false)}`;
const metersHTML = (a) => `<div class="meters">${!a.quota?.windows?.length && a.quota?.limits?.length ? quotaLimitsHTML(a, true) : meterRowsHTML(a)}</div>`;

function miniMetersHTML(a) {
  if (!a.quota?.windows?.length && a.quota?.limits?.length) return quotaLimitsHTML(a, true);
  const row = (short) => {
    if (short && noShortLimit(a)) return '';
    const w = windowOf(a, short);
    return `<div class="mrow"><span class="capsm">${esc(windowLabel(w, short))}</span>${segsHTML(w, windowTitle(w || { name: short ? '5h' : 'week' }))}${pctHTML(w)}</div>`;
  };
  return `<div class="mini-meters">${row(true)}${row(false)}</div>`;
}

// A window as a big bar with its reset line (drawer and account page).
function windowHTML(a, short, size) {
  if (short && noShortLimit(a)) return noShortBlockHTML(size);
  const w = windowOf(a, short);
  return windowBlockHTML(a, w, w ? windowTitle(w) : short ? '5-hour' : 'Weekly', size);
}

function noShortBlockHTML(size) {
  return `<div class="window"><div class="window-top"><span>5-hour${size === 'xl' ? ' window' : ''}</span><span class="dim">No limit</span></div><span class="note">${esc(NO_SHORT_LIMIT)}</span></div>`;
}

function quotaWindowsHTML(a, size = 'xl') {
  const windows = (a.quota?.windows || []).filter((w) => quotaOf(w));
  if (!windows.length) return a.quota?.limits?.length ? '' : `${windowHTML(a, true, size)}${windowHTML(a, false, size)}`;
  return (noShortLimit(a) ? noShortBlockHTML(size) : '') + windows.map((w) => windowBlockHTML(a, w, esc(w.model ? scopedTitle(w) : windowTitle(w)), size)).join('');
}

function quotaLimitsHTML(a, compact = false) {
  const limits = (a.quota?.limits || []).filter((l) => !(l.id?.endsWith(':gate') && l.used_percent === 0));
  const primary = limits.filter((l) => l.scope?.kind === 'account' || l.scope?.kind === 'credits');
  const rows = compact ? (primary.length ? primary : limits).slice(0, 2) : limits;
  const amount = (n) => typeof n === 'number' && Number.isFinite(n) ? n.toLocaleString(undefined, { maximumFractionDigits: 4 }) : null;
  return rows.map((l) => {
    const q = l.enabled === false ? null : quotaOf({ used: l.used_percent, resets_at: l.resets_at });
    const divisor = l.unit === 'USD cents' ? 100 : 1;
    const unit = divisor === 100 ? 'USD' : l.unit;
    const values = l.used != null && l.limit != null ? `${amount(l.used / divisor)} / ${amount(l.limit / divisor)}` : l.remaining != null ? `${amount(l.remaining / divisor)} left` : '';
    const detail = l.unlimited === true ? 'Unlimited' : values ? `${values}${unit ? ` ${unit}` : ''}` : q ? '' : 'Not reported';
    const scope = l.scope?.kind === 'account' ? 'Applies to all models' : l.scope?.kind === 'model' ? `Model: ${l.scope.value}` : l.scope?.kind === 'models' ? `Models: ${l.scope.value.join(', ')}` : l.scope?.kind === 'model_families' ? `Model families: ${l.scope.value.join(', ')}` : l.scope?.kind === 'credits' ? 'Extra credits; separate from subscription allowance' : l.scope?.kind === 'overage' ? 'Optional paid usage; separate from subscription allowance' : 'Separate feature allowance';
    const reset = l.resets_at ? Date.parse(l.resets_at) > Date.now() ? `Resets ${when(l.resets_at)}` : 'Renewal passed; awaiting update' : 'No reset time reported';
    return `<div class="quota-limit${compact ? ' compact' : ''}"><div class="window-top"><span>${esc(l.name)}</span>${q ? `<span class="pct ${q.cls}">${quotaPercent(q[S.quotaDisplay])}</span>` : ''}</div>
      ${detail ? `<span class="note">${esc(detail)}</span>` : ''}${l.enabled === false ? '<span class="note">Disabled</span>' : ''}
      ${compact ? '' : `${q ? meterHTML(q, null, l.name, 'lg', l.resets_at) : ''}<span class="note">${esc(scope)}</span><span class="note">${esc(reset)}</span>`}</div>`;
  }).join('') || (compact ? '<span class="dim">Quota not reported</span>' : '');
}

// Windows limited to one model ("week opus" on some Claude plans), shown on the account page.
const scopedWindows = (a) => ((a.quota && a.quota.windows) || []).filter((w) => w.model && quotaOf(w))
  .sort((x, y) => String(x.name).localeCompare(String(y.name)));
const scopedTitle = (w) => `${cap(w.model)} ${windowTitle({ name: w.name.split(' ')[0] }).toLowerCase()}`;

function windowBlockHTML(a, w, title, size) {
  const q = quotaOf(w);
  const spent = windowSpent(a, w);
  let note = 'Not reported yet';
  if (w && w.resets_at) note = `${q && (q.exhausted || spent) ? 'Back' : 'Resets'} ${esc(when(w.resets_at))} · in ${liveLeft(w.resets_at)}`;
  else if (q) note = 'No reset time reported';
  const big = size === 'xl';
  return `<div class="window"><div class="window-top"><span>${title}${big ? ' window' : ''}</span>${q ? `<span class="pct ${q.cls}${big ? ' big' : ''}" style="width:auto">${quotaPercent(q[S.quotaDisplay])}</span>` : '<span class="dim">–</span>'}</div>
    ${segsHTML(w, `${title} window`, size === 'xl' ? 'xl' : 'lg')}<span class="note">${note}</span>${usedByHTML(a, w)}</div>`;
}

// ---------------------------------------------------------------- quota attribution

// Which clients used each subscription window, from `quota_attribution` on /api/accounts:
// estimates in percentage points of the window, so 18 points read "18%".
const sharePercent = (points) => quotaPercent(Math.min(100, Math.max(0, Number(points) || 0)));
// A window's key as the server writes it: "5h", "week", or "<name> · <model>".
const shareKey = (w) => (w.model ? `${w.name} · ${w.model}` : w.name);
// A window as people read it: "5h", "week", "week opus".
const windowName = (name, model) => (model && !String(name).includes(model) ? `${name} ${model}` : String(name || ''));
const currentShare = (a, key) => (a.quota_attribution?.current || []).find((s) => s.key === key) || null;
// The window of this kind that renewed last (the server lists them newest first).
const previousShare = (attribution, key) => (attribution?.previous || []).find((s) => s.key === key) || null;
// Named clients are hidden with the rest of the labels; programs ("Claude Code") are not.
function quotaClientName(c) {
  const id = String(c.id ?? '');
  if (!id) return c.label || 'Other requests';
  if (id.startsWith('app:')) return c.label || id.slice(4);
  return S.private ? HIDDEN : c.label || id;
}
const QUOTA_METHOD = 'Each rise in the window’s reported use is split over the requests that finished since the previous rise, by their API list price.';
const dayLabel = (iso) => `${weekday(iso)} ${new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}`;

// One row per client (the five largest, then the rest together), then use from outside
// Fusebox and from before it looked. Bars span the whole window, in the meter's colour.
function shareRowsHTML(s, cls = '') {
  const clients = (s.clients || []).filter((c) => c.points > 0);
  const rows = clients.slice(0, 5).map((c) => [quotaClientName(c), c.points, '']);
  const rest = clients.slice(5);
  if (rest.length) rows.push([plural(rest.length, 'more client'), rest.reduce((n, c) => n + c.points, 0), 'other']);
  if (s.outside > 0) rows.push(['Outside Fusebox', s.outside, 'other']);
  if (s.before > 0) rows.push(['Before Fusebox looked', s.before, 'other']);
  return rows.map(([name, points, kind]) => `<div class="ub-row${kind ? ` ${kind}` : ''}"><span class="ub-name" title="${esc(name)}">${esc(name)}</span><span class="ub-bar" aria-hidden="true"><i${cls ? ` class="${cls}"` : ''} style="width:${Math.min(100, points).toFixed(1)}%"></i></span><span class="ub-pct">${esc(sharePercent(points))}</span></div>`).join('');
}

// "Used by" under a meter: the current window, and the last one of its kind folded away.
function usedByHTML(a, w) {
  const attribution = a.quota_attribution;
  if (!w || !attribution) return '';
  const key = shareKey(w);
  const s = currentShare(a, key);
  const prev = previousShare(attribution, key);
  const rows = s ? shareRowsHTML(s, quotaOf(w)?.cls || '') : '';
  if (!rows && !prev) return '';
  const tokens = s && s.by_tokens > 0;
  const tip = `${QUOTA_METHOD}${tokens ? ` About ${sharePercent(s.by_tokens)} of it was split by tokens, as a model has no list price.` : ''}`;
  const head = rows ? `<div class="ub-head"><span class="capsm">Used by</span><span class="ub-basis" title="${esc(tip)}">Estimated from quota readings${tokens ? ' · partly by tokens' : ''}</span></div>${rows}` : '';
  return `<div class="used-by">${head}${prev ? lastShareHTML(a, prev) : ''}</div>`;
}

// Open "Last week" sections, so a refresh leaves them open.
const lastOpen = new Set();
document.addEventListener('toggle', (e) => {
  const id = e.target.dataset?.last;
  if (id) { if (e.target.open) lastOpen.add(id); else lastOpen.delete(id); }
}, true);

function lastShareHTML(a, s) {
  const id = `${a.id}|${s.key}`;
  const label = /^week/.test(s.name) ? 'Last week' : 'Last window';
  const renewed = s.resets_at ? ` · renewed ${dayLabel(s.resets_at)}` : '';
  const rows = shareRowsHTML(s);
  return `<details class="ub-last" data-last="${esc(id)}"${lastOpen.has(id) ? ' open' : ''}><summary><span class="capsm">${label}</span><span class="ub-basis">${esc(`${sharePercent(s.used)} used${renewed}`)}</span></summary><div class="ub-rows">${rows || '<span class="ub-basis">Nothing was used.</span>'}</div></details>`;
}

function quotaControlsHTML(short = false, label = true) {
  return `<div class="quota-ctl">${label ? '<span class="meta" style="font-size:12px">Quota</span>' : ''}
    <div class="seg" role="group" aria-label="Quota display" title="Display preference saved in this browser">
      ${['used', 'remaining'].map((mode) => `<button type="button" data-act="quota-display" data-id="${mode}" aria-pressed="${S.quotaDisplay === mode}">${mode === 'used' ? 'Used' : short ? 'Left' : 'Remaining'}</button>`).join('')}
    </div></div>`;
}

function setQuotaDisplay(mode, persist = true) {
  S.quotaDisplay = mode === 'remaining' ? 'remaining' : 'used';
  if (persist) store.set('quota-display', S.quotaDisplay);
  refreshViews();
}

// ---------------------------------------------------------------- alerts

// The faults menu, the badge and the Needs attention filter follow the server's list
// (live "faults"); until it arrives they are worked out here.
const faultsList = () => S.faultList || alertsList();

// Faults come from account state and the request log: expired sign-ins, quota used
// up, rate limits, account errors and a run of failed requests in the last hour.
function alertsList() {
  const out = [];
  const hourAgo = Date.now() - 3600e3;
  const fails = {};
  for (const r of S.requests) {
    if (Date.parse(r.ts) < hourAgo) break;
    if (r.status >= 400 && r.status !== 499 && r.account_id) (fails[r.account_id] ||= []).push(r);
  }
  const accounts = S.accounts || [];
  for (const a of accounts) {
    if (a.disabled) continue;
    const st = acctState(a);
    if (st.cls === 'cooling' && st.kind === 'quota') {
      const spent = [windowOf(a, true), windowOf(a, false)].find((w) => w && w.used >= 100);
      const back = spent?.resets_at || st.until;
      const others = S.overview?.session_affinity && accounts.some((x) => x.id !== a.id && x.provider === a.provider && acctState(x).cls === 'ready');
      out.push({ lvl: 'warn', id: a.id, title: `${spent ? windowTitle(spent) : 'Usage'} limit used up`,
        detail: `${st.resetUnknown ? 'Provider did not report a reset time. Refresh quota to check availability.' : `Back at ${when(back)}, in ${left(back)}.`}${others ? ` Its sessions moved to another ${provName(a)} account.` : ''}`, act: 'View' });
    } else if (st.cls === 'cooling' && st.kind === 'rate_limit') {
      out.push({ lvl: 'warn', id: a.id, title: 'Rate limited',
        detail: `${st.model === '*' ? 'Every model' : st.model} is paused until ${when(st.until)}, in ${left(st.until)}.`, act: 'View' });
    }
    if (st.cls === 'error') {
      out.push(st.signin
        ? { lvl: 'err', id: a.id, title: 'Sign-in expired', detail: 'Token refresh failed. Sign in again to put it back in rotation.', act: 'Sign in', signin: true }
        : { lvl: 'err', id: a.id, title: 'Account error', detail: String(hideEmails(a.last_error)).slice(0, 160), act: 'View' });
    }
    const f = fails[a.id];
    if (f && f.length >= 3 && st.cls !== 'error') {
      const e = f[0].error ? `: ${String(hideEmails(f[0].error)).slice(0, 120)}` : '';
      out.push({ lvl: 'err', id: a.id, title: `${f.length} failed requests`, detail: `${f[0].status} from ${provName(a)}${e}`, act: 'View', requests: true });
    }
  }
  return out.sort((x, y) => (x.lvl === y.lvl ? 0 : x.lvl === 'err' ? -1 : 1));
}

function runAlert(i) {
  const al = faultsList()[Number(i)];
  if (!al) return;
  closeFaults();
  if (al.kind === 'storage') { location.hash = al.path || '#/usage'; return; }
  if (!al.id) return showPools();
  const a = accountById(al.id);
  if (al.signin && a) return signInAgain(a);
  openAccount(al.id);
}

// A provider's pace warning opens on the overview's combined limits, after the route
// change has scrolled to the top.
function showPools() {
  const show = () => $('#ov-pool')?.scrollIntoView({ block: 'start' });
  if (S.route === 'overview') return show();
  addEventListener('hashchange', show, { once: true });
  location.hash = '#/overview';
}

// ---------------------------------------------------------------- overview

function overviewHTML() {
  const main = firstRun() ? `<section class="mainline-wrap" id="ov-main" aria-label="Main line">${mainlineHTML()}</section>` : '';
  const load = `<section class="card pad load" aria-labelledby="load-title">
      <div class="load-head"><h2 class="label" id="load-title">Load · last 60 min</h2><dl class="stats" id="figures">${figuresHTML()}</dl></div>
      <div class="bars" id="bars">${barsHTML()}</div>
      <div class="axis" aria-hidden="true"><span>60 min ago</span><span>now</span></div></section>`;
  const subs = `<section class="sec" id="ov-subs" aria-label="Subscriptions">${ovSubsHTML()}</section>`;
  const pool = `<section class="sec" id="ov-pool" aria-label="Weekly quota outlook">${ovPoolHTML()}</section>`;
  const other = `<section class="sec" id="ov-other" aria-label="Other circuits">${ovOtherHTML()}</section>`;
  const latest = `<section class="sec" aria-labelledby="latest-title"><div class="sec-head"><h2 class="h-sec" id="latest-title">Latest requests</h2><span class="grow"></span><a class="link" href="#/requests">All requests →</a></div>
      <div id="recent">${recentHTML()}</div></section>`;
  if (mob()) {
    const loadSec = `<section class="sec" aria-labelledby="load-title-m"><h2 class="h-sec" id="load-title-m">Load · last 60 min</h2>${load.replace(' aria-labelledby="load-title"', '').replace('<h2 class="label" id="load-title">Load · last 60 min</h2>', '')}</section>`;
    return main + subs + pool + loadSec + other + latest;
  }
  return `${main}${load}${subs}${pool}${other}${latest}`;
}

function figuresHTML() {
  const o = S.overview;
  const c = seriesCounters(o.series);
  const req = c.requests || 0;
  const completed = req - (c.cancelled || 0);
  const ok = completed - (c.failed || 0);
  const rate = completed ? `${((ok / completed) * 100).toFixed(ok === completed ? 0 : 1)}%` : '—';
  const items = [
    ['Requests', fmt(req)],
    ['Success', `<span title="Successful requests divided by non-cancelled requests">${rate}</span>`],
    ['Cancelled', fmt(c.cancelled || 0)],
    ['Tokens in', aggregateTokensHTML(c, 'input_tokens')],
    ['Tokens out', aggregateTokensHTML(c, 'output_tokens')],
    ['Cached', aggregateTokensHTML(c, 'cache_tokens')],
    ['In flight', fmt(o.active)],
    ...usageStats(c).map(([v, k]) => [k, v]),
  ];
  return items.map(([k, v]) => `<div><dt>${k}</dt><dd>${v}</dd></div>`).join('');
}

// "14:05" for a minute, formatted once: the hour's bars are redrawn as requests arrive.
const minuteLabels = new Map();
function minuteLabel(minute) {
  let label = minuteLabels.get(minute);
  if (label == null) {
    if (minuteLabels.size > 600) minuteLabels.clear();
    label = new Date(minute * 60000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
    minuteLabels.set(minute, label);
  }
  return label;
}

function barHTML(b, max, now) {
  const coverage = usageCoverageText(b);
  const label = `${minuteLabel(b.minute)} · ${plural(b.requests, 'request')}${b.failed ? `, ${b.failed} failed` : ''}${b.cancelled ? `, ${b.cancelled} cancelled` : ''} · ${aggregateTokenText(b, 'tokens')} tokens${coverage ? ` · usage: ${coverage}` : ''}`;
  const cls = [b.minute === now ? 'now' : '', b.failed ? 'fail' : '', b.requests ? '' : 'zero'].filter(Boolean).join(' ');
  return `<i${cls ? ` class="${cls}"` : ''} style="height:${b.requests ? Math.max(3, (b.requests / max) * 100) : 0}%" title="${esc(label)}"></i>`;
}
const barsMax = (series) => Math.max(4, ...series.map((b) => b.requests));
const barsTotalHTML = (series) => `<span class="sr-only">${plural(series.reduce((a, b) => a + b.requests, 0), 'request')} in the last hour</span>`;

function barsHTML(series = S.overview.series) {
  const max = barsMax(series);
  const now = Math.floor(Date.now() / 60000);
  return `${barsTotalHTML(series)}${series.map((b) => barHTML(b, max, now)).join('')}`;
}

// The overview's figures, repainted only when they read differently.
const patchFigures = () => patchChanged('figures', figuresHTML);

// A request usually changes one minute's bar. While the hour's minutes and the scale stay
// the same only the bars that changed are redrawn; a new minute or a new peak redraws all.
const barsPainted = new WeakMap();
function patchBars() {
  const el = document.getElementById('bars');
  if (!el) return;
  const series = S.overview.series;
  const max = barsMax(series);
  const now = Math.floor(Date.now() / 60000);
  const shape = `${max}:${now}:${series.map((b) => b.minute).join(',')}`;
  const bars = series.map((b) => barHTML(b, max, now));
  const prev = barsPainted.get(el);
  if (prev && prev.shape === shape && el.children.length === bars.length + 1) {
    bars.forEach((html, i) => { if (html !== prev.bars[i]) replaceHTML(el.children[i + 1], html); });
    const total = barsTotalHTML(series);
    if (total !== prev.total) replaceHTML(el.firstElementChild, total);
    barsPainted.set(el, { shape, bars, total });
    return;
  }
  const total = barsTotalHTML(series);
  el.innerHTML = total + bars.join('');
  barsPainted.set(el, { shape, bars, total });
}

const ROUTING_TEXT = {
  'least-used': ['go to the account with the most quota left', 'go to the most quota left'],
  'smart-quota': ['use expiring banked resets first, then the earliest weekly renewal, reserving quota for active sessions', 'prefer expiring resets, then weekly renewals'],
  'round-robin': ['take turns across accounts', 'take turns across accounts'],
  'fill-first': ['go to the first available account', 'go to the first available account'],
};

function routingSentence(short = false) {
  const o = S.overview;
  const text = (ROUTING_TEXT[o.routing] || ['', ''])[short ? 1 : 0];
  return text ? `${o.session_affinity === false ? 'requests' : 'new sessions'} ${text}` : '';
}

// "2 serving · 5 of 6 ready · smart routing…"; serving is left out while disconnected.
function subsMeta(short = false) {
  const subs = (S.accounts || []).filter(metered);
  const ready = subs.filter((a) => acctState(a).cls === 'ready');
  const serving = S.load ? `${fmt(ready.filter((a) => inFlight(a)).length)} serving · ` : '';
  return `${serving}${fmt(ready.length)} of ${fmt(subs.length)} ready · ${routingSentence(short)}`;
}

function ovSubsHTML() {
  const list = S.accounts || [];
  if (!list.length) {
    return `<div class="sec-head"><h2 class="h-sec">Subscriptions</h2></div><div class="card table"><div class="empty" style="border-top:0">
      <h3>No accounts connected</h3>
      <p>Sign in with a subscription or add an API key. From a terminal you can also run <code>fusebox login claude</code>.</p>
      <div class="actions">
        <button class="btn" type="button" data-act="start-login" data-provider="claude">${logo('claude', null, null, 14)}Sign in with Claude</button>
        <button class="btn" type="button" data-act="start-login" data-provider="codex">${logo('codex', null, null, 14)}Sign in with ChatGPT</button>
        <button class="btn" type="button" data-act="open-panel" data-panel="connect">Other accounts</button>
        <button class="btn" type="button" data-act="open-panel" data-panel="key">Add API key</button>
      </div></div></div>`;
  }
  const subs = list.filter(metered);
  if (!subs.length) return '';
  if (mob()) {
    const items = subs.map((a) => {
      const st = acctState(a);
      return `<div class="item click" data-open="${esc(a.id)}">
        <div class="item-top">${acctLogo(a, 18)}<div class="cell2"><button class="name-btn" type="button" data-act="open-acc" data-id="${esc(a.id)}">${esc(acctLabel(a))}</button><span>${esc(provName(a))} · ${esc(planName(a) || authName(a))}${esc(noShortNote(a))}</span></div>${statusHTML(a, 7)}</div>
        <div class="meters indent">${meterRowsHTML(a)}</div>
        <div class="metaline indent">${st.cls === 'cooling' ? `<span class="fg2">${esc(modelScope(st.model))} · ${st.resetUnknown ? 'Reset time unknown' : `back ${esc(when(st.until))}`}</span>` : ''}${bankedLineHTML(a)}<span>${fmt(a.counters.requests)} requests · <span data-now="${esc(a.id)}" data-inline>${nowHTML(a, true)}</span></span></div>
      </div>`;
    }).join('');
    return `<div class="sec-head"><div class="stack"><h2 class="h-sec">Subscriptions</h2><span class="meta" data-subs-meta="short">${esc(subsMeta(true))}</span></div>${quotaControlsHTML(true, false)}</div>
      <div class="card stack-list">${items}</div>`;
  }
  const rows = subs.map((a, i) => {
    const st = acctState(a);
    return `<div class="tr click" data-open="${esc(a.id)}">
      <span class="c-idx idx">${String(i + 1).padStart(2, '0')}</span>
      <div class="who">${acctLogo(a)}<div class="cell2"><button class="name-btn" type="button" data-act="open-acc" data-id="${esc(a.id)}">${esc(acctLabel(a))}</button><span class="sub">${esc(provName(a))} · ${esc(planName(a) || authName(a))}${esc(noShortNote(a))}</span></div></div>
      ${metersHTML(a)}
      <div class="c-status">${statusHTML(a)}${st.cls === 'cooling' ? `<span class="subs-line">${esc(modelScope(st.model))} · ${st.resetUnknown ? 'Reset time unknown' : `back ${esc(when(st.until))}`}</span>` : ''}${bankedLineHTML(a)}</div>
      <span class="c-req r">${fmt(a.counters.requests)}</span>
      <div class="c-last c-now" data-now="${esc(a.id)}">${nowHTML(a)}</div>
    </div>`;
  }).join('');
  return `<div class="sec-head"><h2 class="h-sec">Subscriptions</h2><span class="meta" data-subs-meta>${esc(subsMeta())}</span><span class="grow"></span>${quotaControlsHTML()}</div>
    <div class="card rivets table subs" role="table" aria-label="Subscriptions">
      <div class="th" role="row"><span class="c-idx">#</span><span>Account</span><span>Limits · ${quotaWord()}</span><span>Status</span><span class="c-req r">Requests</span><span class="r">Activity</span></div>
      ${rows}</div>`;
}

// Plans count by Claude's published weekly allowance; the weights themselves are in src/pools.rs.
const PLAN_WEIGHT_NOTE = 'Each account counts by its plan’s weekly allowance: Claude Pro 1, Max 5, Ultra 10';

// How many accounts are on each known plan, most first: [["Ultra", 2], ["Pro", 1]].
function planCounts(list) {
  const counts = new Map();
  for (const a of list) {
    const name = planName(a);
    if (name) counts.set(name, (counts.get(name) || 0) + 1);
  }
  return [...counts].sort((x, y) => y[1] - x[1] || x[0].localeCompare(y[0]));
}

// Server forecasts for each provider with several subscriptions; quota and resets
// are weighted consistently in src/pools.rs and src/outlook.rs.
function weeklyPools() {
  return ((S.overview && S.overview.pools) || []).filter((p) => p.window === 'week' && !p.model).map((p) => {
    const list = (p.account_ids || []).map(accountById).filter(Boolean);
    return list.length ? { ...p, account: list[0], plans: planCounts(list) } : null;
  }).filter(Boolean);
}

// Forecast reserves always mean remaining quota, independently of the subscription toggle.
const POOL_OUTLOOK_NOTE = 'Lowest combined weekly quota expected over the next 7 days, using recent consumption and each account’s scheduled resets. Assumes usage can move between accounts. Five-hour and model limits are separate; banked resets are not predicted.';
const POOL_CURRENT_NOTE = 'Current combined weekly quota while the forecast learns from recent usage. The forecast appears automatically when enough readings are available.';

function poolLearningQuota(p) {
  return p.outlook?.state === 'learning' && p.accounts > 0 && p.reported === p.accounts ? quotaOf({ used: p.used }) : null;
}

function poolStatus(p) {
  const o = p.outlook || {};
  if (o.state === 'run_out') return { text: 'Quota gap expected', cls: 'err' };
  if (o.state === 'tight') return { text: 'Low reserve', cls: 'warn' };
  if (o.state === 'on_track') return { text: 'Expected to last', cls: 'good' };
  if (o.state === 'learning') return { text: 'Learning usage', cls: '' };
  return { text: 'Forecast unavailable', cls: '' };
}

function poolMeterHTML(p) {
  const o = p.outlook || {};
  const reset = o.next_reset_at ? `<span title="${esc(fullResetTime(o.next_reset_at))}">Next reset in ${liveLeft(o.next_reset_at, true)}</span>` : '<span>No reset time reported</span>';
  const current = poolLearningQuota(p);
  if (current) {
    const label = S.quotaDisplay === 'used' ? 'used now' : 'remaining now';
    return `<div class="meters"><div class="mrow" title="${esc(POOL_CURRENT_NOTE)}"><span class="capsm">NOW</span>${meterHTML(current, null, `${provName(p.account)} current combined weekly`)}<span class="pct ${current.cls}">${esc(quotaPercent(current[S.quotaDisplay]))}</span><span class="rst">${label}</span></div><div class="pool-note">${reset}</div></div>`;
  }
  const status = poolStatus(p);
  const known = ['on_track', 'tight', 'run_out'].includes(o.state) && Number.isFinite(o.min_remaining);
  const reserve = known ? Math.max(0, Math.min(100, o.min_remaining)) : null;
  const label = `${provName(p.account)} lowest projected weekly reserve`;
  const meter = reserve == null
    ? `<span class="segs" role="img" aria-label="${esc(label)}: forecast unavailable">${'<i></i>'.repeat(20)}</span>`
    : `<span class="segs ${status.cls}" role="meter" aria-label="${esc(label)}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${reserve}" aria-valuetext="${esc(`${quotaPercent(reserve)} lowest projected reserve over the next 7 days`)}">${Array.from({ length: 20 }, (_, i) => i < Math.round(reserve / 5) ? '<i class="on"></i>' : '<i></i>').join('')}</span>`;
  const low = known && o.min_at ? ` · <span title="${esc(fullResetTime(o.min_at))}">Lowest ${esc(when(o.min_at))}</span>` : '';
  const basis = known && Number.isFinite(o.observed_hours) ? ` · Based on ${Math.floor(o.observed_hours)}h of recent quota readings.` : '';
  return `<div class="meters"><div class="mrow" title="${esc(POOL_OUTLOOK_NOTE + basis)}"><span class="capsm">MIN</span>${meter}<span class="pct ${status.cls}">${reserve == null ? '–' : esc(quotaPercent(reserve))}</span><span class="rst">lowest reserve</span></div><div class="pool-note">${reset}${low}</div></div>`;
}

function poolStatusHTML(p) {
  const o = p.outlook || {};
  const status = poolStatus(p);
  let detail = 'Next 7 days at recent usage';
  if (o.state === 'run_out') {
    detail = o.exhausts_at && o.recovers_at ? `Runs out ${when(o.exhausts_at)} · Back ${when(o.recovers_at)}` : 'Expected to exhaust weekly quota';
  } else if (o.state === 'learning') {
    detail = o.learning_until ? `Estimate after ${when(o.learning_until)}` : 'Needs 6h of recent quota readings';
  } else if (!['on_track', 'tight'].includes(o.state)) {
    detail = ({ stale_readings: 'Quota readings are stale', missing_quota: 'Waiting for every account’s weekly quota',
      missing_reset: 'Waiting for every account’s reset time', awaiting_reset: 'Waiting for reset confirmation',
      unavailable_account: 'An account needs to sign in again' })[o.reason] || 'Waiting for quota history';
  }
  return `<span class="pool-gap ${status.cls}">${status.text}</span><span class="pool-note">${esc(detail)}</span>`;
}

// "3 accounts · Ultra" when they share a plan, "2 Ultra, 1 Pro" when every plan is known
// and they differ, and the count first whenever some plan or weekly window is missing.
function poolCount(p) {
  const count = p.reported < p.accounts ? `${fmt(p.reported)} of ${plural(p.accounts, 'account')} reporting` : plural(p.accounts, 'account');
  const plans = p.plans || [];
  const known = plans.reduce((sum, [, n]) => sum + n, 0);
  if (plans.length === 1 && known === p.accounts) return `${count} · ${plans[0][0]}`;
  const mix = plans.map(([name, n]) => `${fmt(n)} ${name}`).join(', ');
  if (mix && known === p.accounts && p.reported === p.accounts) return mix;
  return [count, mix].filter(Boolean).join(' · ');
}

// How the accounts of each pool count: by plan allowance, or equally when a plan is unknown.
function poolBasis(pools) {
  const names = (priced) => pools.filter((p) => !!p.priced === priced).map((p) => provName(p.account)).join(', ');
  const [priced, equal] = [names(true), names(false)];
  if (!equal) return 'Each provider’s subscriptions together, weighted by plan allowance';
  if (!priced) return 'Each provider’s subscriptions together, weighted equally';
  return `${priced} weighted by plan allowance, ${equal} equally`;
}
// A forecast per provider, with the lowest reserve before any of the coming resets.
function ovPoolHTML() {
  const pools = weeklyPools();
  if (!pools.length) return '';
  const note = `${POOL_OUTLOOK_NOTE} · ${poolBasis(pools)}${pools.some((p) => p.priced) ? ` · ${PLAN_WEIGHT_NOTE}` : ''}`;
  const learning = pools.every((p) => poolLearningQuota(p));
  const heading = `<div class="sec-head"><h2 class="h-sec">Weekly quota outlook</h2><span class="meta" title="${esc(learning ? POOL_CURRENT_NOTE : note)}">${learning ? 'Current quota while learning' : 'Next 7 days · lowest projected reserve'}</span></div>`;
  if (mob()) {
    const items = pools.map((p) => `<div class="item">
        <div class="item-top">${acctLogo(p.account, 18)}<div class="cell2"><span>${esc(provName(p.account))}</span><span>${esc(poolCount(p))}</span></div></div>
        <div class="indent">${poolMeterHTML(p)}<div class="c-status">${poolStatusHTML(p)}</div></div>
      </div>`).join('');
    return `${heading}<div class="card stack-list pool">${items}</div>`;
  }
  const rows = pools.map((p) => `<div class="tr">
      <span class="c-idx"></span>
      <div class="who">${acctLogo(p.account)}<div class="cell2"><span class="pool-name">${esc(provName(p.account))}</span><span class="sub">${esc(poolCount(p))}</span></div></div>
      ${poolMeterHTML(p)}
      <div class="c-status">${poolStatusHTML(p)}</div>
    </div>`).join('');
  return `${heading}<div class="card table subs pool">${rows}</div>`;
}

function ovOtherHTML() {
  const others = (S.accounts || []).filter((a) => !metered(a));
  if (!others.length) return '';
  const cards = others.map((a) => {
    const st = acctState(a);
    return `<button class="circuit${a.disabled ? ' off' : ''}" type="button" data-act="open-acc" data-id="${esc(a.id)}">
      <span class="dot s7 ${st.dot}"></span>${acctLogo(a, 16)}
      <span class="cell2"><span class="prov">${esc(provName(a))}</span><span class="acc">${esc(acctLabel(a))}</span></span>
      <span class="right"><span class="${st.cls === 'ready' ? 'fg2' : `status ${st.cls}`}">${esc(statusWord(a))}</span><span class="dim">${fmt(a.counters.requests)} req</span></span>
    </button>`;
  }).join('');
  return `<div class="sec-head"><h2 class="h-sec">Other circuits</h2>${mob() ? '' : '<span class="meta">API keys and sign-ins without usage limits</span>'}<span class="grow"></span><button class="link textbtn" type="button" data-act="open-panel" data-panel="connect">Connect account</button></div>
    <div class="circuits">${cards}</div>`;
}

// Named clients see only models their key permits, including prefixed aliases.
function setupModelsFor(key = null) {
  const client = (S.config?.values?.['named-clients'] || []).find((c) => c.key === key);
  const allows = (id) => !client?.models?.length || client.models.some((pattern) => {
    const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*');
    return new RegExp(`^${escaped}$`, 'i').test(id);
  });
  return S.models.filter((m) => allows(m.id));
}

function setupModel(kind, key = null, chosen = null) {
  const models = setupModelsFor(key);
  const family = kind === 'codex' ? 'gpt-' : kind === 'gemini' ? 'gemini-' : 'claude-';
  return models.find((m) => m.id === chosen) || models.find((m) => m.id.split('/').pop().startsWith(family)) || models[0] || null;
}

function modelCapabilitiesText(model) {
  if (!model) return 'No models available for this client';
  const c = model.capabilities || {};
  return [c.context_window ? `${c.context_window.toLocaleString()} context tokens` : 'Context limit unknown',
    c.max_output_tokens ? `${c.max_output_tokens.toLocaleString()} output tokens` : 'Output limit unknown',
    c.input_modalities ? c.input_modalities.includes('image') ? 'Image input supported' : 'Image input not supported' : 'Image support unknown',
    c.tool_calling == null ? '' : c.tool_calling ? 'Tool calls supported' : 'Tool calls not supported'].filter(Boolean).join(' · ');
}

// A copyable/downloadable configuration for the selected public model ID. Known
// provider metadata is kept; unknown limits are omitted rather than guessed.
function snippet(kind, key = null, modelId = null) {
  const origin = location.origin;
  const keyed = !!key || S.overview.client_keys.length > 0 || S.overview.named_clients > 0;
  const token = key || (keyed ? 'your-client-key' : 'fbx_local');
  const model = setupModel(kind, key, modelId);
  const id = model?.id || (kind === 'codex' ? 'gpt-6-astra' : 'claude-sonnet-5-5');
  const caps = { ...(model?.capabilities || {}) };
  const entered = S.setupContexts?.[id];
  if (!caps.context_window && Number.isSafeInteger(entered) && entered > 0) caps.context_window = entered;
  const q = JSON.stringify;
  const sh = (s) => /^[A-Za-z0-9_:/.,@+=-]+$/.test(s) ? s : `'${s.replace(/'/g, `'"'"'`)}'`;
  const result = (text, filename, note) => ({ text, filename, note, html: esc(text).split(esc(token)).join(esc(key ? secret(key) : token)) });
  const json = (value, filename, note) => result(`${JSON.stringify(value, null, 2)}\n`, filename, note);
  switch (kind) {
    case 'opencode': {
      const entry = {};
      if (caps.context_window || caps.max_output_tokens) entry.limit = { ...(caps.context_window ? { context: caps.context_window } : {}), ...(caps.max_output_tokens ? { output: caps.max_output_tokens } : {}) };
      if (caps.input_modalities || caps.output_modalities) entry.modalities = { ...(caps.input_modalities ? { input: caps.input_modalities } : {}), ...(caps.output_modalities ? { output: caps.output_modalities } : {}) };
      if (caps.tool_calling != null) entry.tool_call = caps.tool_calling;
      return json({ $schema: 'https://opencode.ai/config.json', provider: { fusebox: { npm: '@ai-sdk/openai-compatible', name: 'Fusebox', options: { baseURL: `${origin}/v1`, apiKey: token }, models: { [id]: entry } } }, model: `fusebox/${id}` }, 'opencode.json', 'Merge this into opencode.json. Reported context, output and image capabilities are included.');
    }
    case 'continue': {
      const features = [...(caps.tool_calling ? ['tool_use'] : []), ...(caps.input_modalities?.includes('image') ? ['image_input'] : [])];
      const options = [caps.context_window ? `      contextLength: ${caps.context_window}` : '', caps.max_output_tokens ? `      maxTokens: ${caps.max_output_tokens}` : ''].filter(Boolean);
      return result(`name: Fusebox\nversion: 1.0.0\nschema: v1\nmodels:\n  - name: ${q(id)}\n    provider: openai\n    model: ${q(id)}\n    apiBase: ${q(`${origin}/v1`)}\n    apiKey: ${q(token)}${features.length ? `\n    capabilities: ${q(features)}` : ''}${options.length ? `\n    defaultCompletionOptions:\n${options.join('\n')}` : ''}\n`, 'config.yaml', 'Merge into ~/.continue/config.yaml. Unknown model capabilities use the client defaults.');
    }
    case 'zed':
      if (!caps.context_window) return result('Enter the model’s context limit above to generate Zed settings.', null, 'Zed requires a context limit. Use the provider’s documented value.');
      return json({ language_models: { openai_compatible: { Fusebox: { api_url: `${origin}/v1`, available_models: [{ name: id, max_tokens: caps.context_window, ...(caps.max_output_tokens ? { max_output_tokens: caps.max_output_tokens } : {}), ...(caps.input_modalities || caps.tool_calling != null ? { capabilities: { tools: caps.tool_calling ?? true, images: caps.input_modalities?.includes('image') ?? false, parallel_tool_calls: false, prompt_cache_key: false } } : {}) }] } } } }, 'settings.json', `Merge into Zed settings.json, then enter this client’s key in the Agent panel.${keyed ? '' : ' No key is required, so any value works.'}`);
    case 'codex': {
      const catalog = model?.codex_catalog ? `\nmodel_catalog_url = ${q(`${origin}/v1/codex/models.json?model=${encodeURIComponent(id)}`)}` : '';
      return result(`# ~/.codex/config.toml\nmodel = ${q(id)}\nmodel_provider = "fusebox"${caps.context_window ? `\nmodel_context_window = ${caps.context_window}` : ''}\n\n[model_providers.fusebox]\nname = "Fusebox"\nbase_url = ${q(`${origin}/v1`)}${catalog}\nwire_api = "responses"${keyed ? '\nenv_key = "FUSEBOX_KEY"' : ''}\n`, 'config.toml', `${keyed ? 'Export FUSEBOX_KEY with this client’s key. ' : ''}Merge top-level settings before provider tables, then restart Codex. ${model?.codex_catalog ? 'The catalog preserves the provider’s tools, reasoning and image metadata for this alias.' : 'Complete Codex metadata is not available for this model; Codex uses its built-in defaults.'}`);
    }
    case 'gemini':
      return result(`export GOOGLE_GEMINI_BASE_URL=${sh(origin)}\nexport GEMINI_API_KEY=${sh(token)}\ngemini -m ${sh(id)}\n`, 'fusebox-gemini.sh', 'Gemini CLI speaks the Gemini API; Fusebox translates the selected model.');
    case 'aider':
      return result(`export OPENAI_API_BASE=${sh(`${origin}/v1`)}\nexport OPENAI_API_KEY=${sh(token)}\naider --model ${sh(`openai/${id}`)}\n`, 'fusebox-aider.sh', 'The openai/ prefix selects the OpenAI API; the rest is the exact public model ID.');
    case 'cline':
      return result(`API Provider   OpenAI Compatible\nBase URL       ${origin}/v1\nAPI Key        ${token}\nModel ID       ${id}\n${caps.context_window ? `Context Window ${caps.context_window}\n` : ''}${caps.max_output_tokens ? `Max Output     ${caps.max_output_tokens}\n` : ''}${caps.input_modalities ? `Supports Images ${caps.input_modalities.includes('image')}\n` : ''}`, 'fusebox-cline.txt', 'Use these values in Cline, Roo Code or Kilo Code. Leave unreported capabilities at the client defaults.');
    case 'cursor':
      return result(`Settings → Models → API Keys\nOpenAI API Key            ${token}\nOverride OpenAI Base URL  ${origin}/v1\nModel ID                 ${id}`, 'fusebox-cursor.txt', 'Cursor sends requests from its own servers. Use a public HTTPS Fusebox address.');
    case 'sdk':
      return result(`from openai import OpenAI\n\nclient = OpenAI(base_url=${q(`${origin}/v1`)}, api_key=${q(token)})\nreply = client.chat.completions.create(\n    model=${q(id)},\n    messages=[{"role": "user", "content": "Hello"}],\n)\n`, 'fusebox-example.py', `Fusebox translates the selected model into the client’s API format.${keyed ? '' : ' No key is required, so any value works.'}`);
    case 'curl':
      return result(`curl ${sh(`${origin}/v1/chat/completions`)} \\\n  -H ${sh(`Authorization: Bearer ${token}`)} \\\n  -H 'Content-Type: application/json' \\\n  -d ${sh(q({ model: id, messages: [{ role: 'user', content: 'Hello' }] }))}\n`, 'fusebox-curl.sh', 'Also available: /v1/messages, /v1/responses (HTTP and websocket) and /v1beta/models.');
    default:
      return result(`export ANTHROPIC_BASE_URL=${sh(origin)}\nexport ANTHROPIC_AUTH_TOKEN=${sh(token)}\nexport ANTHROPIC_MODEL=${sh(id)}\nclaude\n`, 'fusebox-claude.sh', 'The selected model ID is sent to Fusebox. Fusebox translates when the model uses another provider’s API.');
  }
}

// Until this browser has seen Fusebox serve a request, the overview opens with the main
// line and how to connect a client. After that they live in Config, Clients.
function firstRun() {
  if (!S.served && S.overview.totals.requests) {
    S.served = true;
    store.set('served', '1');
  }
  return !S.served;
}

// The main line leaves the overview when the first request arrives.
function patchMainline() {
  if (firstRun()) patch('ov-main', mainlineHTML);
  else $('#ov-main')?.remove();
}

const copyBtn = (text, label, note) => `<button class="icon-btn copy" type="button" data-act="copy" data-text="${esc(text)}" data-toast="${note}" aria-label="${label}" title="${label}">${ICON.copy}</button>`;

// The endpoint: the overview's main line and Config, Clients.
const endpointItemHTML = () => `<div class="ml-item"><span class="k">Endpoint</span><span class="v" title="${esc(location.origin)}">${esc(location.origin)}</span>${copyBtn(location.origin, 'Copy endpoint', 'Endpoint copied')}</div>`;

// The main line's endpoint and key: each client has its own, made in Config, Clients.
function connectItemsHTML() {
  const keyed = S.overview.client_keys.length > 0 || S.overview.named_clients > 0;
  return `${endpointItemHTML()}
      <div class="ml-item"><span class="k">Key</span><span class="v" style="font-family:var(--sans);font-size:13px;color:var(--fg-2)">${keyed
        ? 'Each client\u2019s own, from <a class="linkbtn" href="#/config/clients">Config, Clients</a>' : 'None required'}</span></div>`;
}

function mainlineHTML() {
  const setup = (cls) => `<div class="${cls}" data-setup="">${setupHTML()}</div>`;
  return `<div class="card mainline-card"><div class="mainline">
      <span class="label">Main line</span>
      ${connectItemsHTML()}
      <div class="ml-item"><span class="k">Models</span><span class="v">${S.overview.models}</span></div>
    </div>${mob() ? setup('setup') : ''}</div>
    ${mob() ? '' : setup('card setup')}
    <p class="ml-later">After the first request this moves to <a class="linkbtn" href="#/config/clients">Config, Clients</a>.</p>`;
}

const SNIPPET_TABS = [['claude', 'Claude Code'], ['codex', 'Codex'], ['opencode', 'OpenCode'], ['sdk', 'OpenAI SDK'], ['curl', 'curl']];
const SNIPPET_MORE = [['gemini', 'Gemini CLI'], ['aider', 'Aider'], ['continue', 'Continue'], ['zed', 'Zed'], ['cline', 'Cline & Roo Code'], ['cursor', 'Cursor']];

// `uid` keeps ids apart when a page shows more than one setup.
function setupHTML(key = null, uid = '') {
  const client = (S.config?.values?.['named-clients'] || []).find((c) => c.key === key);
  const scope = `${client?.id || uid || 'main'}:${S.snippet}`;
  const models = setupModelsFor(key);
  const selected = setupModel(S.snippet, key, S.setupModels[scope]);
  const sn = snippet(S.snippet, key, selected?.id);
  const more = SNIPPET_MORE.some(([id]) => id === S.snippet);
  return `<div class="snip-head">
      <div class="snip-tabs" role="group" aria-label="Client">${SNIPPET_TABS.map(([id, label]) => `<button type="button" data-act="snippet" data-id="${id}" aria-pressed="${S.snippet === id}">${label}</button>`).join('')}
        <label class="sr-only" for="snip-more${uid}">More tools</label>
        <select id="snip-more${uid}" class="snip-more${more ? ' on' : ''}"><option value="">More tools</option>${SNIPPET_MORE.map(([id, label]) => `<option value="${id}"${S.snippet === id ? ' selected' : ''}>${label}</option>`).join('')}</select></div>
      <span class="grow"></span>
      ${selected ? `<button class="icon-btn copy" type="button" data-act="copy" data-text="${esc(sn.text)}" data-toast="Snippet copied" aria-label="Copy snippet" title="Copy snippet">${ICON.copy}</button>` : ''}
    </div>
    ${selected ? `<div class="setup-model"><label for="setup-model${uid}">Model</label><select id="setup-model${uid}" class="setup-model-select" data-setup-scope="${esc(scope)}">${models.map((m) => `<option value="${esc(m.id)}"${m.id === selected.id ? ' selected' : ''}>${esc(m.id)}</option>`).join('')}</select>
      ${S.snippet === 'zed' && !selected.capabilities?.context_window ? `<label for="setup-context${uid}">Context tokens</label><input id="setup-context${uid}" class="setup-context" type="number" min="1" step="1" data-model="${esc(selected.id)}" value="${esc(S.setupContexts[selected.id] || '')}" placeholder="Provider’s documented limit">` : ''}</div>
      <p class="note setup-capabilities">${esc(modelCapabilitiesText(selected))}</p>
      <pre class="code">${sn.html}</pre>
      <div class="setup-footer"><p class="note">${esc(sn.note)}</p>${sn.filename ? `<button class="btn sm" type="button" data-act="download-setup" data-filename="${esc(sn.filename)}" data-text="${esc(sn.text)}">Download file</button>` : ''}</div>`
      : '<p class="note">No models available for this client. Connect an account or update the client’s allowed models.</p>'}`;
}

function downloadSetup(text, filename) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// Every setup on the page follows the chosen tool; a client's keeps its key.
function refreshSetups() {
  for (const el of $$('[data-setup]')) {
    const client = (S.config.values?.['named-clients'] || []).find((c) => c.id === el.dataset.setup);
    writeHTML(el, setupHTML(client?.key, el.dataset.setupUid || ''));
  }
}

// ---------------------------------------------------------------- request rows

const ROUTING_LABEL = { 'least-used': 'Most quota remaining', 'smart-quota': 'Smart quota balancing', 'round-robin': 'Round robin', 'fill-first': 'Fill first' };
const ROUTING_REASON = {
  expiring_reset: 'Expiring reset',
  new_session: 'New session',
  session_reused: 'Same session',
  quota_exhausted: 'Moved: quota used up',
  account_disabled: 'Moved: account disabled',
  account_removed: 'Moved: account removed',
  model_unavailable: 'Moved: model not served',
  temporary_detour: 'Detour: account busy',
  missing_session: 'No session id',
  affinity_disabled: 'Affinity off',
  retry_same: 'Retried same account',
};
const MOVED = /^(quota_exhausted|account_|model_unavailable|temporary_detour)/;
const ROUTING_WARNING = {
  missing_session_id: ['No session ID', 'The client supplied no stable session identifier. Later requests may use another account and lose cache reuse.'],
  connection_only: ['Connection only', 'This assignment lasts for the WebSocket connection. A reconnect without a stable session identifier may use another account.'],
  response_id_only: ['Response ID only', 'This assignment relies on a previous response ID held in memory. A stable session identifier is needed to preserve it across server restarts.'],
  affinity_disabled: ['Affinity off', 'Session affinity is disabled. Requests from this session may use different accounts.'],
  inferred_session: ['Inferred session', 'The client sent no session ID, so this conversation was recognised by its first message. Conversations that start the same way share an account; send x-fusebox-session-id to keep them apart.'],
};
const SESSION_SOURCE = {
  previous_response_id: 'a previous response ID',
  websocket_connection: 'this WebSocket connection',
  generated_response: 'a generated response ID',
  prompt_cache_key: 'the prompt cache key',
  conversation_start: 'the conversation’s first message, as the client sent no session ID',
};
const STRATEGY_PICK = {
  'least-used': 'the account with the most quota left',
  'smart-quota': 'the account with an expiring banked reset, or the earliest weekly renewal',
  'round-robin': 'the next account in turn',
  'fill-first': 'the first available account',
};
const COUNT = ['zero', 'one', 'two', 'three', 'four', 'five'];

const routingReason = (reason) => ROUTING_REASON[reason] || (reason || '').replaceAll('_', ' ');
const isErr = (r) => r.status >= 400 && r.status !== 499;
const isRerouted = (r) => MOVED.test(r.routing_reason || '');

// One sentence on why a request went where it did, from its routing record.
function whySentence(r) {
  const pick = STRATEGY_PICK[r.routing_strategy] || 'the routing strategy’s pick';
  const base = {
    expiring_reset: 'This new session went to an account with a banked reset nearing expiry, to use its allowance before restoring it. The session stays on this account.',
    new_session: `First request of this session. It went to ${pick}, and the session stays there from now on.`,
    session_reused: 'Stayed on the account this session is pinned to, so its prompt cache keeps working.',
    temporary_detour: 'The session’s account was busy, so this one request used another. The session stays pinned to its account.',
    quota_exhausted: 'The session’s account ran out of quota, so the session moved here and stays here. Its prompt cache starts cold.',
    account_disabled: 'The session’s account was disabled, so the session moved here and stays here. Its prompt cache starts cold.',
    account_removed: 'The session’s account was removed, so the session moved here and stays here. Its prompt cache starts cold.',
    model_unavailable: 'The session’s account doesn’t serve this model, so the session moved here and stays here.',
    missing_session: `The client sent no session id, so this request went to ${pick} on its own.`,
    affinity_disabled: `Session affinity is off, so each request goes to ${pick}.`,
    retry_same: 'Retried on the same account.',
  }[r.routing_reason] || (r.account ? `It went to ${pick}.` : 'No account could take this request.');
  const tries = r.attempts > 1 ? ` It took ${COUNT[r.attempts] || r.attempts} tries.` : '';
  let end = '';
  if (r.status === 499) end = ` ${cancellationText(r)}`;
  else if (isErr(r)) end = r.attempts > 1 ? ' It still failed upstream.' : ' The request failed upstream; nothing was retried.';
  const chunks = r.chunks ? ` It used ${r.chunks} embedding chunks.` : '';
  return base + tries + chunks + end;
}

function routeHTML(r, tags = true, logoSize = 14) {
  const acct = accountOf(r);
  const provider = acct ? provName(acct) : PROVIDER[r.provider] || r.provider || '—';
  const kind = { ws: 'ws', images: 'image', video: 'video' }[r.transport];
  const extra = tags ? [kind, r.attempts > 1 ? `${r.attempts} tries` : null, r.chunks ? `${r.chunks} chunks` : null].filter(Boolean) : [];
  return `<span class="route">${esc(CLIENT[r.client] || r.client)} <span class="arrow">→</span> ${r.provider ? logo(r.provider, acct ? acct.group : r.account, acct && acct.kind, logoSize) : ''} ${esc(provider)}${extra.map((t) => `<span class="tag">${t}</span>`).join('')}</span>`;
}

const reqAcctName = (r) => (accountOf(r) ? acctLabel(accountOf(r)) : who(r.account)) || '—';
const noteHTML = (r) => (r.routing_reason ? `<span class="${isRerouted(r) ? 'note-warn' : ''}">${esc(routingReason(r.routing_reason))}</span>` : '');

function sessHTML(r, button = true) {
  if (!r.session_id) {
    const w = ROUTING_WARNING[r.routing_warning];
    return w ? `<span class="note-warn" title="${esc(w[1])}">${esc(w[0])}</span>` : '';
  }
  const source = SESSION_SOURCE[r.session_source] || (r.session_source ? `the client’s ${r.session_source}` : 'the client');
  const title = `Session fingerprint: ${r.session_id}\nIdentified by ${source}. Click to show only this session.`;
  const short = esc(r.session_id.slice(0, 8));
  return button
    ? `<button class="sess-btn" type="button" data-act="filter-session" data-id="${esc(r.session_id)}" title="${esc(title)}" aria-label="Show requests for session ${short}">${short}</button>`
    : `<span class="mono">${short}</span>`;
}

function acctCellHTML(r, button = true) {
  const parts = [noteHTML(r), sessHTML(r, button)].filter(Boolean).join('<span class="dim"> · </span>');
  return `<span class="cell2"><span title="${esc(reqAcctName(r))}">${esc(reqAcctName(r))}</span>${parts ? `<span class="acct-note">${parts}</span>` : ''}</span>`;
}

const statusCls = (r) => (isErr(r) ? 'code-err' : r.status === 499 ? 'code-closed' : 'code-ok');
const statusCell = (r) => `<span class="mono ${statusCls(r)}"${r.status === 499 ? ` title="${esc(cancellationText(r))}"` : ''}>${r.status || '—'}</span>`;

function cancellationText(r) {
  return {
    downstream_disconnect: 'Connection closed before completion.',
    downstream_write_timeout: 'Timed out sending the response to the client.',
    downstream_write_error: 'Connection error while sending the response to the client.',
    downstream_read_error: 'Connection error while reading from the client.',
    downstream_queue_limit: 'Connection ended because the pending request limit was reached.',
  }[r.failure_kind] || 'Connection ended before completion.';
}

function tokensText(r, field) {
  const completeness = usageState(r);
  if (completeness === 'missing') return '<span class="dim" title="No provider token usage was reported; consumption is unknown.">Unknown</span>';
  const partial = completeness === 'partial';
  return `<span title="${Number(r[field] || 0).toLocaleString('en-US')} reported tokens${partial ? '; partial usage, total consumption is unknown' : ''}">${partial ? '≥' : ''}${fmt(r[field] || 0)}</span>`;
}

function requestTokensHTML(r) {
  if (usageState(r) === 'missing') return '<span class="dim">Unknown · provider usage not reported</span>';
  return `${tokensText(r, 'input_tokens')} in · ${tokensText(r, 'output_tokens')} out · ${tokensText(r, 'cache_tokens')} cached${usageState(r) === 'partial' ? ' · partial usage' : ''}`;
}

function recentRowHTML(r) {
  const acct = accountOf(r);
  return `<div class="tr click"${acct ? ` data-open="${esc(acct.id)}"` : ''}>
    <span class="t" title="${esc(r.ts)}">${clock(r.ts)}</span>${routeHTML(r)}<span class="m">${esc(r.model)}</span>${acctCellHTML(r, false)}${statusCell(r)}
    <span class="n c-ft">${ms(r.ttft_ms)}</span><span class="n">${ms(r.latency_ms)}</span></div>`;
}

function reqLineHTML(r) {
  const acct = accountOf(r);
  return `<div class="item click"${acct ? ` data-open="${esc(acct.id)}"` : ''}>
    <div class="l1"><span class="t">${clock(r.ts)}</span>${r.provider ? logo(r.provider, acct?.group || r.account, acct?.kind, 13) : ''}<span class="rt">${esc(CLIENT[r.client] || r.client)} → ${esc(acct ? provName(acct) : PROVIDER[r.provider] || r.provider || '—')}</span>${statusCell(r)}<span class="n">${ms(r.latency_ms)}</span></div>
    <span class="l2">${esc(r.model)}</span>
    <span class="l3">${esc(reqAcctName(r))}${r.routing_reason ? ` · ${noteHTML(r)}` : ''}</span></div>`;
}

function recentHTML() {
  const rows = S.requests.slice(0, 6);
  if (!rows.length) {
    const how = firstRun() ? 'Point a client at the endpoint above' : 'Point a client at Fusebox (<a class="linkbtn" href="#/config/clients">set up a client</a>)';
    return `<div class="card table"><div class="empty" style="border-top:0"><h3>No requests yet</h3><p>${how} and requests will show up here as they happen.</p></div></div>`;
  }
  if (mob()) return `<div class="card lines">${rows.map(reqLineHTML).join('')}</div>`;
  return `<div class="card table flat recent" role="table" aria-label="Latest requests">
    <div class="th" role="row"><span>Time</span><span>Route</span><span>Model</span><span>Account</span><span>Status</span><span class="c-ft r">First token</span><span class="r">Total</span></div>
    ${rows.map(recentRowHTML).join('')}</div>`;
}

// ---------------------------------------------------------------- drawer (overview)

function openAccount(id) {
  if (!accountById(id)) return;
  closeFaults();
  if (S.route === 'overview') return openDrawer(id);
  location.hash = `#/accounts/${encodeURIComponent(id)}`;
}

let drawerOpener = null;
function openDrawer(id) {
  drawerOpener = document.activeElement;
  S.drawer = id;
  S.confirm = null;
  loadActivity(id, true);
  loadRequests();
  renderDrawer();
  document.body.style.overflow = 'hidden';
  $('#drawer [data-act="close-drawer"]')?.focus();
}

function closeDrawer() {
  if (!S.drawer) return;
  S.drawer = null;
  S.confirm = null;
  renderDrawer();
  document.body.style.overflow = '';
  if (drawerOpener && document.contains(drawerOpener)) drawerOpener.focus({ preventScroll: true });
  drawerOpener = null;
}

function renderDrawer() {
  const slot = $('#lay-drawer');
  const a = S.drawer && accountById(S.drawer);
  if (!a) {
    if (S.drawer) { S.drawer = null; document.body.style.overflow = ''; }
    slot.innerHTML = '';
    return;
  }
  const active = document.activeElement;
  const sel = slot.contains(active) ? focusSelector(active) : null;
  slot.innerHTML = drawerHTML(a);
  if (sel) slot.querySelector(sel)?.focus({ preventScroll: true });
}

function accountErrorHTML(a) {
  const local = S.errorClears[a.id];
  if (!a.last_error && !local?.error) return '';
  const billing = a.provider === 'claude' && /third-party apps now draw from your extra usage/i.test(a.last_error || '');
  return `<section class="account-error block" aria-label="Last error" aria-busy="${!!local?.busy}">
    ${a.last_error ? `<div class="account-error-head"><span class="label">Last error</span><button class="btn sm" type="button" data-act="clear-error" data-id="${esc(a.id)}" aria-label="Clear error for ${esc(acctLabel(a))}" ${local?.busy || !a.last_error_id ? 'disabled' : ''}>${local?.busy ? 'Clearing…' : 'Clear error'}</button></div>
    <div class="errbox">${esc(hideEmails(a.last_error))}</div>
    <p class="meta">Clears the saved message. A new failure will appear again.</p>
    ${billing ? '<a class="link" href="https://claude.ai/settings/usage" target="_blank" rel="noopener noreferrer">Manage extra usage</a>' : ''}` : ''}
    ${local?.error ? `<p class="msg err" role="alert">${esc(local.error)}</p>` : ''}
  </section>`;
}

function drawerHTML(a) {
  const st = acctState(a);
  const hasQuota = metered(a) && ((a.quota?.windows || []).some((w) => quotaOf(w)) || a.quota?.limits?.length > 0);
  const banked = hasBankedResets(a) ? a.banked_resets : null;
  const count = banked && !banked.error ? banked.inventory?.available : null;
  return `<div class="scrim" data-act="close-drawer"></div>
  <aside class="drawer" id="drawer" role="dialog" aria-modal="true" aria-labelledby="drawer-title">
    <div class="drawer-head">${acctLogo(a, 24)}<div class="cell2" style="flex:1;gap:2px"><h2 id="drawer-title">${esc(acctLabel(a))}</h2><span class="sub">${esc(acctSub(a))}</span></div>
      <button class="close-btn" type="button" data-act="close-drawer"><kbd class="kbd">Esc</kbd>Close</button></div>
    <div class="drawer-body">
      <div class="statline">${statusHTML(a)}<span class="dim">· last used ${liveAgo(a.last_used)}</span></div>
      ${st.cls === 'cooling' ? `<div class="notice warn"><div class="cell2"><span class="mono">${esc(modelScope(st.model))}</span><span class="small warn">${st.kind === 'quota' ? 'Usage limit used up' : 'Cooling down'} · ${st.resetUnknown ? 'Reset time unknown' : `back at ${esc(when(st.until))}, in ${liveLeft(st.until)}`}</span></div><button class="btn sm" type="button" data-act="reset" data-id="${esc(a.id)}">Clear cooldowns</button></div>` : ''}
      ${hasQuota ? `<div class="block" style="gap:14px"><span class="label">Limits · ${quotaWord()}</span>${quotaWindowsHTML(a, 'lg')}${quotaLimitsHTML(a)}</div>` : ''}
      ${windowStarterHTML(a)}
      ${banked && (count || ['pending', 'unknown'].includes(banked.operation?.status)) ? `<div class="notice"><div class="cell2"><span style="font-size:13px">${esc(bankedLabel(a))}</span><span class="small dim">${esc(bankedPolicyText(a))}</span></div><button class="btn sm" type="button" data-act="banked-details" data-id="${esc(a.id)}" aria-haspopup="dialog">Reset details</button></div>` : ''}
      ${accountErrorHTML(a)}
      <div class="block"><span class="label">Since start</span><div class="figs" id="drawer-since">${drawerSinceHTML()}</div></div>
      <div class="block" style="gap:6px"><span class="label">Sign-in</span><span class="fg2" style="font-size:13px">${esc(signinLine(a))}</span></div>
      <div class="block" style="gap:0" id="drawer-reqs">${drawerRequestsHTML()}</div>
    </div>
    <div class="drawer-foot">${drawerActionsHTML(a)}</div>
  </aside>`;
}

function signinLine(a) {
  if (a.kind === 'oauth') {
    const secs = a.expires_at ? (Date.parse(a.expires_at) - Date.now()) / 1000 : null;
    return [authName(a), secs == null ? null : secs > 0 ? `token valid ${span(secs)}` : 'token expired', 'refreshes on its own'].filter(Boolean).join(' · ');
  }
  return `${authName(a)} · ${acctLabel(a)}`;
}

function drawerSinceHTML() {
  const a = S.drawer && accountById(S.drawer);
  if (!a) return '';
  const c = a.counters;
  const sessions = S.activity[a.id] ? fmt(S.activity[a.id].sessions.length) : '—';
  return [[fmt(c.requests), 'Requests'], [fmt(c.failures), 'Failed'], [fmt(c.cancelled || 0), 'Cancelled'], [sessions, 'Sessions'], [aggregateTokensHTML(c, 'input_tokens'), 'Tokens in'], [aggregateTokensHTML(c, 'output_tokens'), 'Tokens out'], [aggregateTokensHTML(c, 'cache_tokens'), 'Cached'], ...usageStats(c)]
    .map(([v, k]) => `<div><b>${v}</b><span>${k}</span></div>`).join('');
}

function drawerRequestsHTML() {
  const a = S.drawer && accountById(S.drawer);
  if (!a) return '';
  const rows = S.requests.filter((r) => accountOf(r) === a).slice(0, 4);
  if (!rows.length) return '';
  return `<span class="label" style="margin-bottom:6px">Recent requests</span><div class="mini-reqs">${rows.map((r) => `<div class="tr"><span class="dim">${clock(r.ts)}</span><span class="ellipsis">${esc(r.model)}</span><span class="${statusCls(r)}">${r.status}</span><span class="fg2" style="text-align:right">${ms(r.latency_ms)}</span></div>`).join('')}</div>`;
}

function drawerActionsHTML(a) {
  if (S.confirm === a.id) {
    return `<span class="meta grow">Remove ${esc(acctLabel(a))}?</span><button class="btn ghost" type="button" data-act="cancel-delete">Keep</button><button class="btn danger" type="button" data-act="delete" data-id="${esc(a.id)}" style="border-color:var(--err-btn-line)">Remove</button>`;
  }
  return `${a.kind === 'oauth' ? `<button class="btn" type="button" data-act="refresh" data-id="${esc(a.id)}">Refresh</button>` : ''}
    <button class="btn" type="button" data-act="toggle" data-id="${esc(a.id)}">${a.disabled ? 'Enable' : 'Disable'}</button>
    <span class="grow"></span><button class="btn danger" type="button" data-act="confirm-delete" data-id="${esc(a.id)}">Remove</button>`;
}

// ---------------------------------------------------------------- palette

function paletteItems(q) {
  const items = [];
  const accounts = S.accounts || [];
  const act = (ref, label, hint, kbd = '') => items.push({ group: 'Actions', kind: 'act', ref, label, hint, kbd });
  act('connect', 'Connect account', 'Sign in with a subscription', 'C');
  act('refresh-page', 'Refresh page', 'Reload the dashboard and check for updates');
  act('apikey', 'Add API key', 'Anthropic, OpenAI, Gemini, OpenRouter…');
  act('quota', S.quotaDisplay === 'remaining' ? 'Show quota used' : 'Show quota remaining', 'Flip every meter', 'U');
  act('privacy', S.private ? 'Show emails and keys' : 'Hide emails and keys', 'For screenshots and screen sharing', '.');
  act('client', 'Set up a client', 'Claude Code, Codex, OpenCode, SDKs, curl and more');
  act('play', 'Try a model', 'Send a prompt and see which account answers');
  act('theme', `Theme: ${THEMES[S.theme]}`, 'Switch between light, dark and the system’s');
  const lang = window.fuseboxLanguage;
  if (lang) act('language', lang.current === 'zh' ? 'Language: 中文' : 'Language: English', lang.current === 'zh' ? 'Switch to English' : '切换到中文');
  act('copyurl', 'Copy endpoint URL', location.origin);
  for (const a of accounts.filter((x) => !x.disabled && Object.keys(x.cooldowns || {}).length).slice(0, 3)) {
    items.push({ group: 'Actions', kind: 'act', ref: 'clear', id: a.id, label: 'Clear cooldowns', hint: `${acctLabel(a)} · ${provName(a)}` });
  }
  // Accounts that need a look come first.
  const rank = (a) => ({ error: 0, cooling: 1, ready: 2, disabled: 3 })[acctState(a).cls];
  const accs = [...accounts].sort((x, y) => rank(x) - rank(y) || (metered(y) - metered(x)));
  for (const a of accs) items.push({ group: 'Accounts', kind: 'acc', ref: a.id, label: acctLabel(a), hint: `${provName(a)} · ${statusWord(a)}`, logo: acctLogo(a, 16) });
  for (const m of S.models) items.push({ group: 'Models', kind: 'model', ref: m.id, label: m.id, hint: 'Copy model id' });
  let out;
  if (q) {
    const needle = q.toLowerCase();
    out = items.filter((x) => `${x.label} ${x.hint} ${x.group}`.toLowerCase().includes(needle));
  } else {
    let accN = 0;
    let modN = 0;
    out = items.filter((x) => (x.group === 'Accounts' ? accN++ < 4 : x.group === 'Models' ? modN++ < 3 : true));
  }
  return out.slice(0, 11);
}

function openPalette() {
  if (!S.overview || S.locked) return;
  closeFaults();
  if (!S.palette) S.palette = { q: '', i: 0, opener: document.activeElement };
  renderPalette(true);
  $('#pal-q')?.focus();
}

function closePalette() {
  if (!S.palette) return;
  const opener = S.palette.opener;
  S.palette = null;
  $('#lay-pal').innerHTML = '';
  if (opener && document.contains(opener)) opener.focus({ preventScroll: true });
}

function paletteListHTML() {
  const P = S.palette;
  const items = paletteItems(P.q.trim());
  P.items = items;
  P.i = Math.min(P.i, Math.max(0, items.length - 1));
  if (!items.length) return '<div class="pal-none">No matches</div>';
  return items.map((x, i) => `${i === 0 || items[i - 1].group !== x.group ? `<div class="pal-group label" role="presentation">${x.group}</div>` : ''}
    <div class="pal-item" role="option" id="pal-i-${i}" data-pal="${i}" aria-selected="${i === P.i}">${x.logo || '<span class="sq" aria-hidden="true"></span>'}<span class="lbl">${esc(x.label)}</span><span class="hint">${esc(x.hint)}</span>${x.kbd ? `<kbd class="kbd">${esc(x.kbd)}</kbd>` : ''}</div>`).join('');
}

function renderPalette(full = false) {
  const P = S.palette;
  if (!P) return;
  if (full || !$('#pal-list')) {
    $('#lay-pal').innerHTML = `<div class="scrim light" data-act="close-palette"></div>
      <div class="palette" role="dialog" aria-modal="true" aria-label="Search or run a command">
        <div class="pal-input">${ICON.search}<input id="pal-q" type="text" role="combobox" aria-expanded="true" aria-controls="pal-list" aria-autocomplete="list" placeholder="Search accounts, models, actions" autocomplete="off" spellcheck="false" value="${esc(P.q)}"><kbd class="kbd">Esc</kbd></div>
        <div class="pal-list" id="pal-list" role="listbox" aria-label="Results"></div>
        <div class="pal-foot"><span>↑↓ move</span><span>↵ open</span><span>U quota</span><span>. privacy</span></div>
      </div>`;
  }
  $('#pal-list').innerHTML = paletteListHTML();
  syncPaletteActive();
}

function syncPaletteActive() {
  const P = S.palette;
  for (const el of $$('#pal-list [data-pal]')) el.setAttribute('aria-selected', String(Number(el.dataset.pal) === P.i));
  $('#pal-q')?.setAttribute('aria-activedescendant', P.items?.length ? `pal-i-${P.i}` : '');
  $(`#pal-i-${P.i}`)?.scrollIntoView({ block: 'nearest' });
}

function runPalette(i) {
  const it = S.palette?.items?.[i];
  if (!it) return;
  closePalette();
  if (it.kind === 'acc') return openAccount(it.ref);
  if (it.kind === 'model') return copyText(it.ref).then(() => toast(`Copied ${it.ref}`));
  switch (it.ref) {
    case 'refresh-page': return refreshPage();
    case 'connect': return openPanel('connect');
    case 'apikey': return openPanel('key');
    case 'quota': return setQuotaDisplay(S.quotaDisplay === 'used' ? 'remaining' : 'used');
    case 'privacy': return togglePrivacy();
    case 'client':
      location.hash = '#/config/clients';
      return;
    case 'copyurl': return copyText(location.origin).then(() => toast('Endpoint copied'));
    case 'theme': return cycleTheme();
    case 'language': return window.fuseboxLanguage?.set(window.fuseboxLanguage.current === 'zh' ? 'en' : 'zh');
    case 'play':
      if (S.route !== 'models') location.hash = '#/models';
      return setTimeout(() => { if (!S.play) $('[data-act="open-play"]')?.click(); else $('#play-prompt')?.focus(); }, 50);
    case 'clear': return accountAction('reset', it.id).then(() => toast('Cooldowns cleared'));
  }
}

// ---------------------------------------------------------------- faults menu

function toggleFaults(btn) {
  if (S.faults) return closeFaults();
  closePalette();
  S.faults = { opener: btn };
  renderFaultsMenu();
  for (const b of $$('[data-act="faults"][aria-expanded]')) b.setAttribute('aria-expanded', 'true');
  $('#faults-menu .fault-item')?.focus();
}

function closeFaults() {
  if (!S.faults) return;
  const opener = S.faults.opener;
  S.faults = false;
  $('#lay-faults').innerHTML = '';
  for (const b of $$('[data-act="faults"][aria-expanded]')) b.setAttribute('aria-expanded', 'false');
  if (opener && document.contains(opener) && !S.drawer) opener.focus({ preventScroll: true });
}

function renderFaultsMenu(list = faultsList()) {
  const btn = $$('[data-faults-slot] .faults').find((b) => b.offsetParent);
  let style = '';
  if (btn && !mob()) {
    const r = btn.getBoundingClientRect();
    style = ` style="top:${Math.round(r.bottom + 4)}px;right:${Math.round(innerWidth - r.right)}px"`;
  }
  const items = list.map((al, i) => {
    const a = accountById(al.id);
    // A pace warning's detail names the client that used most of the window.
    const tip = (al.kind === 'pace' || al.kind === 'storage') && al.detail ? ` title="${esc(S.private && al.kind === 'pace' ? al.detail.split(/(?<=\.) /)[0] : al.detail)}"` : '';
    // The usage database's faults belong to no account, nor does a provider's pace warning.
    const where = al.kind === 'storage' ? esc(al.provider || 'Usage')
      : !al.id ? `${esc(al.provider)} · all subscriptions`
        : `${esc(a ? provName(a) : al.provider || '')} · ${esc(a ? acctLabel(a) : al.label ? who(al.label) : '')}`;
    return `<button class="fault-item" type="button" data-act="alert-open" data-id="${i}"${tip}><span class="dot ${al.lvl}"></span><span class="cell2"><b>${esc(al.title)}</b><span class="mono">${where}</span></span><span class="chev" aria-hidden="true">›</span></button>`;
  }).join('');
  $('#lay-faults').innerHTML = `<div class="scrim menu" data-act="close-faults"></div><div class="faults-menu" id="faults-menu" role="dialog" aria-label="Faults"${style}>${items || '<p class="pal-none">Nothing tripped. Every account is in rotation.</p>'}</div>`;
}

function closeLayers() {
  closePalette();
  closeFaults();
  closeDrawer();
}

// ---------------------------------------------------------------- accounts

const ACCT_FILTERS = [['all', 'All', 'All'], ['subs', 'Subscriptions', 'Subs'], ['signins', 'Other sign-ins', 'Sign-ins'], ['keys', 'API keys', 'Keys'], ['attention', 'Needs attention', 'Attention']];
function acctGroups() {
  const attention = new Set(faultsList().map((al) => al.id));
  return {
    all: () => true,
    subs: (a) => metered(a),
    signins: (a) => !metered(a) && a.kind === 'oauth',
    keys: (a) => a.kind === 'api-key' || a.kind === 'service-account',
    attention: (a) => attention.has(a.id),
  };
}

function accountsHTML() {
  if (S.sub) return detailHTML();
  return `<div id="acct-head">${accountHeadHTML()}</div>
    <div id="acct-panel">${panelHTML()}</div>
    <div id="acct-filters">${acctFiltersHTML()}</div>
    <div id="acct-timeline">${resetTimelineHTML()}</div>
    <div id="acct-list">${accountListHTML()}</div>`;
}

// Every subscription's next weekly reset on one seven-day line, so it is clear whose
// allowance renews next. Shown with two or more subscriptions that report one.
function resetTimelineHTML() {
  if (!['all', 'subs'].includes(S.acctFilter)) return '';
  const now = Date.now();
  const span = 7 * 864e5;
  const items = (S.accounts || []).filter((a) => !a.disabled && metered(a)).map((a) => {
    const w = windowOf(a, false);
    const at = w?.resets_at ? Date.parse(w.resets_at) : NaN;
    return at > now && at <= now + span ? { a, w, at, q: quotaOf(w) } : null;
  }).filter(Boolean).sort((x, y) => x.at - y.at);
  if (items.length < 2) return '';
  const pos = (t) => ((t - now) / span) * 100;
  // Resets close together take the next lane, so their marks don't overlap.
  const lanes = [];
  for (const it of items) {
    let lane = lanes.findIndex((last) => pos(it.at) - last > (mob() ? 9 : 4));
    if (lane < 0) { lane = lanes.length; lanes.push(-Infinity); }
    if (lane > 2) lane = 2;
    lanes[lane] = pos(it.at);
    it.lane = lane;
  }
  const days = [];
  const day = new Date(now);
  day.setHours(24, 0, 0, 0);
  for (let t = day.getTime(); t < now + span; t += 864e5) days.push(t);
  const used = (it) => (it.q ? `${quotaPercent(it.q.used)} used` : 'usage not reported');
  return `<section class="card timeline" aria-label="Weekly resets in the next seven days">
    <div class="card-head"><span class="label">Weekly resets · next 7 days</span><span class="meta" style="font-size:12px">Next: ${esc(acctLabel(items[0].a))} ${esc(when(items[0].w.resets_at))}</span></div>
    <div class="tl-track" style="height:${22 + Math.min(lanes.length, 3) * 22}px">
      ${days.map((t) => `<span class="tl-day" style="left:${pos(t)}%">${esc(weekday(new Date(t).toISOString()))}</span>`).join('')}
      ${items.map((it) => `<button type="button" class="tl-mark ${it.q?.cls || ''}" style="left:${pos(it.at)}%;top:${it.lane * 22}px" data-act="open-acc" data-id="${esc(it.a.id)}"
        aria-label="${esc(`${acctLabel(it.a)}, ${provName(it.a)}: ${used(it)}, resets ${fullResetTime(it.w.resets_at)}`)}" title="${esc(`${acctLabel(it.a)} · ${used(it)} · resets ${fullResetTime(it.w.resets_at)}`)}">${acctLogo(it.a, 14)}</button>`).join('')}
    </div></section>`;
}

function accountHeadHTML() {
  const n = (S.accounts || []).length;
  const connecting = S.panel === 'connect' || !!LOGIN[S.panel] || S.panel === 'vertex';
  const connect = `<button class="btn primary" type="button" data-act="open-panel" data-panel="connect" aria-expanded="${connecting}">Connect account</button>`;
  const key = `<button class="btn" type="button" data-act="open-panel" data-panel="key" aria-expanded="${S.panel === 'key'}">Add API key</button>`;
  if (mob()) return `<div class="acct-actions">${connect}${key}</div>`;
  return `<div class="page-head">
    <div><h1 class="h-page">Accounts</h1><p class="meta">${n ? `${n} connected · sign-ins in <span class="mono">${esc(home(S.overview.auth_dir))}</span>, keys in <span class="mono">config.yaml</span>` : 'Nothing connected yet'}</p></div>
    <span class="grow"></span>${key}${connect}</div>`;
}

function acctFiltersHTML() {
  const list = S.accounts || [];
  if (!list.length) return '';
  const groups = acctGroups();
  const chips = ACCT_FILTERS.map(([id, label, short]) => `<button class="chip" type="button" data-act="acct-filter" data-id="${id}" aria-pressed="${S.acctFilter === id}">${mob() ? short : label} <span class="n">${list.filter(groups[id]).length}</span></button>`).join('');
  if (mob()) return `<div class="m-filters" role="group" aria-label="Show">${chips}</div><div class="m-row" style="margin-top:14px">${quotaControlsHTML(true, true)}</div>`;
  return `<div class="acct-filters"><div class="chips" role="group" aria-label="Show">${chips}</div><span class="grow"></span>${quotaControlsHTML()}</div>`;
}

function breakerHTML(a) {
  if (S.confirm === a.id) {
    return `<div class="breaker"><span class="confirm"><button class="btn sm ghost" type="button" data-act="cancel-delete">Keep</button><button class="btn sm danger" type="button" data-act="delete" data-id="${esc(a.id)}" aria-label="Confirm removing ${esc(acctLabel(a))}">Remove</button></span></div>`;
  }
  return `<div class="breaker">
    ${a.kind === 'oauth' ? `<button class="icon-btn row" type="button" data-act="refresh" data-id="${esc(a.id)}" aria-label="Refresh ${esc(acctLabel(a))}" title="Refresh">${ICON.refresh}</button>` : '<span style="width:28px"></span>'}
    <button class="switch" type="button" role="switch" aria-checked="${!a.disabled}" data-act="toggle" data-id="${esc(a.id)}" aria-label="${esc(acctLabel(a))} in rotation" title="${a.disabled ? 'Turn on' : 'Turn off'}"></button>
    <button class="icon-btn row del" type="button" data-act="confirm-delete" data-id="${esc(a.id)}" aria-label="Remove ${esc(acctLabel(a))}" title="Remove">${ICON.trash}</button></div>`;
}

function statusExtrasHTML(a) {
  const st = acctState(a);
  let out = '';
  if (st.cls === 'cooling') out += `<span class="subs-line">${esc(modelScope(st.model))} · <button class="linkbtn" type="button" data-act="reset" data-id="${esc(a.id)}" title="Clear local cooldowns; refresh to check provider limits">Clear</button></span>`;
  if (st.cls === 'error' && canSignIn(a)) out += `<span class="subs-line"><button class="linkbtn" type="button" data-act="signin-again" data-id="${esc(a.id)}">Sign in again</button></span>`;
  return out + bankedLineHTML(a);
}

function accountListHTML() {
  const list = S.accounts || [];
  if (!list.length) {
    return `<div class="card table"><div class="empty" style="border-top:0"><h3>No accounts yet</h3>
      <p>Connect a subscription or add an API key above, or run <code>fusebox login &lt;provider&gt;</code> on the server. Existing CLIProxyAPI credentials in the auth directory are picked up automatically.</p></div></div>`;
  }
  const rows = list.filter(acctGroups()[S.acctFilter] || (() => true));
  const none = S.acctFilter === 'attention' ? 'Nothing needs attention.' : 'No accounts in this group.';
  if (mob()) {
    const items = rows.map((a) => {
      const st = acctState(a);
      const fails = a.counters.failures;
      return `<div class="item click${a.disabled ? ' off' : ''}" data-open="${esc(a.id)}" style="${a.disabled ? 'opacity:.55' : ''}">
        <div class="item-top">${acctLogo(a, 18)}<div class="cell2"><button class="name-btn" type="button" data-act="open-acc" data-id="${esc(a.id)}">${esc(acctLabel(a))}</button><span>${esc(provName(a))} · ${esc(authName(a))}${esc(noShortNote(a))}</span></div>${statusHTML(a, 7)}</div>
        ${metered(a) ? `<div class="indent">${miniMetersHTML(a)}</div>` : ''}
        <div class="metaline indent">${st.cls === 'cooling' ? `<span>${esc(modelScope(st.model))}</span>` : ''}${bankedLineHTML(a)}<span>${fmt(a.counters.requests)} req · ${liveAgo(a.last_used)}</span>${fails ? `<span class="err">${fmt(fails)} failed</span>` : ''}${a.counters.cancelled ? `<span>${fmt(a.counters.cancelled)} cancelled</span>` : ''}${usageCoverageText(a.counters) ? `<span>Usage: ${usageCoverageText(a.counters)}</span>` : ''}</div>
        ${a.last_error && !a.disabled ? `<span class="errline indent">${esc(hideEmails(a.last_error))}</span>` : ''}
      </div>`;
    }).join('');
    return `<div class="card stack-list">${items || `<p class="empty-line" style="padding:20px 12px">${none}</p>`}</div>`;
  }
  const items = rows.map((a, i) => {
    const c = a.counters;
    return `<div class="acct-item${a.disabled ? ' off' : ''}">
      <div class="tr click" data-open="${esc(a.id)}">
        <span class="c-idx idx">${String(list.indexOf(a) + 1).padStart(2, '0')}</span>
        <div class="who">${acctLogo(a)}<div class="cell2"><button class="name-btn" type="button" data-act="open-acc" data-id="${esc(a.id)}">${esc(acctLabel(a))}</button><span class="sub">${esc(acctSub(a) + noShortNote(a))}</span></div></div>
        ${metered(a) ? metersHTML(a) : '<span class="no-limits">No usage limits</span>'}
        <div class="c-status">${statusHTML(a)}${statusExtrasHTML(a)}</div>
        <div class="c-traffic"><span>${fmt(c.requests)} <span class="dim">requests</span></span><span class="subs-line">${aggregateTokensHTML(c, 'input_tokens')} in · ${aggregateTokensHTML(c, 'output_tokens')} out${c.failures ? ` · <span class="err">${fmt(c.failures)} failed</span>` : ''}${c.cancelled ? ` · ${fmt(c.cancelled)} cancelled` : ''}</span>${usageCoverageText(c) ? `<span class="subs-line">Usage: ${usageCoverageText(c)}</span>` : ''}</div>
        <span class="c-last fg2" style="text-align:left">${liveAgo(a.last_used)}</span>
        ${breakerHTML(a)}
      </div>
      ${a.last_error && !a.disabled ? `<div class="errline">${esc(hideEmails(a.last_error))}</div>` : ''}
    </div>`;
  }).join('');
  return `<div class="card rivets table acct-table" role="table" aria-label="Accounts">
    <div class="th" role="row"><span class="c-idx">#</span><span>Account</span><span>Limits · ${quotaWord()}</span><span>Status</span><span class="c-traffic">Traffic</span><span class="c-last">Last used</span><span class="r">Breaker</span></div>
    ${items || `<div class="empty">${none}</div>`}</div>`;
}

const canSignIn = (a) => a.kind !== 'api-key' && (!!LOGIN[a.provider] || a.provider === 'vertex');
function signInAgain(a) {
  if (a.provider === 'vertex') return openPanel('vertex');
  if (LOGIN[a.provider]) return startLogin(a.provider);
}

// account detail -------------------------------------------------------

function detailHTML() {
  const a = accountById(S.sub);
  if (!a) {
    return `<nav class="crumbs" aria-label="Breadcrumb"><a href="#/accounts">Accounts</a></nav>
      <div class="card table"><div class="empty" style="border-top:0"><h3>This account is gone</h3><p>It was removed or its credentials changed.</p><a class="btn" href="#/accounts">All accounts</a></div></div>`;
  }
  return `<div id="det-root" style="display:contents">${detailBodyHTML()}</div>`;
}

function detailBodyHTML() {
  const a = accountById(S.sub);
  if (!a) return '';
  const idx = String((S.accounts || []).indexOf(a) + 1).padStart(2, '0');
  const st = acctState(a);
  const toggle = `<button class="btn toggle-btn" type="button" data-act="toggle" data-id="${esc(a.id)}" role="switch" aria-checked="${!a.disabled}"><span class="switch sm" aria-hidden="true" ${a.disabled ? '' : 'data-on="true"'}></span>${a.disabled ? 'Disabled' : 'Enabled'}</button>`;
  const remove = S.confirm === a.id
    ? `<button class="btn ghost" type="button" data-act="cancel-delete">Keep</button><button class="btn danger" type="button" data-act="delete" data-id="${esc(a.id)}" style="border-color:var(--err-btn-line)">Remove ${esc(acctLabel(a))}</button>`
    : `<button class="btn danger" type="button" data-act="confirm-delete" data-id="${esc(a.id)}">Remove</button>`;
  const head = mob()
    ? `<div class="det-head">${acctLogo(a, 28)}<div class="cell2"><h1>${esc(acctLabel(a))}</h1><span class="sub">${esc([provName(a), planName(a)].filter(Boolean).join(' '))} · ${esc(authName(a))}</span></div></div>
       <div class="statline" style="font-size:13px">${statusHTML(a)}<span class="dim">· last used ${liveAgo(a.last_used)}</span></div>
       ${accountErrorHTML(a)}`
    : `<nav class="crumbs" aria-label="Breadcrumb"><a href="#/accounts">Accounts</a><span aria-hidden="true">/</span><span>${esc(provName(a))}</span><span aria-hidden="true">/</span><span aria-current="page">${esc(acctLabel(a))}</span></nav>
       <div class="det-head">${acctLogo(a, 34)}<div class="cell2"><h1>${esc(acctLabel(a))}</h1><span class="sub"><span class="mono">${idx}</span><span>${esc(acctSub(a))}</span></span></div>
         <div class="det-actions"><span class="status-pill">${statusHTML(a)}</span>${a.kind === 'oauth' ? `<button class="btn" type="button" data-act="refresh" data-id="${esc(a.id)}">Refresh</button>` : ''}${toggle}${remove}</div></div>
       ${accountErrorHTML(a)}`;
  const limits = metered(a) ? `<section class="card pad block" style="gap:20px" aria-label="Limits">
      <div class="card-head"><span class="label">Limits · ${quotaWord()}</span>${quotaControlsHTML(mob(), false)}</div>
      <div class="windows">${quotaWindowsHTML(a)}</div>
      ${a.quota?.limits?.length ? `<div class="quota-limits">${quotaLimitsHTML(a)}</div>` : ''}
      ${detBankedHTML(a)}${windowStarterHTML(a)}</section>` : '';
  const cds = Object.entries(a.cooldowns || {}).sort((x, y) => Date.parse(x[1]) - Date.parse(y[1]));
  const reasonOf = (m) => ({ quota: 'Usage limit used up', rate_limit: 'Rate limited', checking: 'Checking quota' })[(a.cooldown_kinds || {})[m]] || 'Rate limited';
  const cools = (!mob() || cds.length) ? `<section class="card cools" style="padding:${mob() ? '12px 14px 4px' : '18px 20px 6px'}" aria-label="Cooldowns">
      <div class="card-head" style="margin-bottom:8px"><span class="label">Cooldowns</span>${cds.length ? `<button class="btn sm" type="button" data-act="reset" data-id="${esc(a.id)}">Clear all</button>` : ''}</div>
      ${cds.length ? cds.map(([m, t]) => mob()
        ? `<div class="tr"><span class="mono">${esc(modelScope(m))}</span><span class="warn" style="font-size:12px">${reasonOf(m)} · ${m === '*' && a.quota_reset_unknown ? 'Reset time unknown' : `back ${esc(when(t))}`}</span></div>`
        : `<div class="tr"><span class="mono">${esc(modelScope(m))}</span><span class="fg2">${reasonOf(m)}</span><span class="warn r">${m === '*' && a.quota_reset_unknown ? 'Reset time unknown' : `back ${esc(when(t))} · ${liveLeft(t)}`}</span></div>`).join('')
        : '<div class="tr" style="display:block;padding:12px 0 14px;color:var(--fg-3)">No models cooling down. A rate limit pauses only that model on this account.</div>'}</section>` : '';
  const load = `<section class="card pad load" style="flex:none" aria-label="Load in the last 60 minutes" id="det-load">${detLoadHTML()}</section>`;
  const reqs = `<div id="det-reqs" style="display:contents">${detRequestsHTML()}</div>`;
  const signin = `${detPriorityHTML(a)}${detSigninHTML(a)}`;
  const sessions = `<section class="card" style="padding:${mob() ? '12px 14px 4px' : '18px 20px 8px'}" aria-label="Pinned sessions" id="det-sess">${detSessionsHTML()}</section>`;
  if (mob()) {
    return `${head}${limits}${cools}${load}${reqs}${sessions}${signin}
      <div class="acct-actions">${a.kind === 'oauth' ? `<button class="btn" type="button" data-act="refresh" data-id="${esc(a.id)}">Refresh</button>` : ''}<button class="btn" type="button" data-act="toggle" data-id="${esc(a.id)}">${a.disabled ? 'Enable' : 'Disable'}</button>${S.confirm === a.id
        ? `<button class="btn danger" type="button" data-act="delete" data-id="${esc(a.id)}" style="border-color:var(--err-btn-line)">Confirm</button>`
        : `<button class="btn danger" type="button" data-act="confirm-delete" data-id="${esc(a.id)}" style="border-color:var(--err-btn-line)">Remove</button>`}</div>`;
  }
  return `${head}<div class="cols"><div class="col-main">${limits}${cools}${load}${reqs}</div><div class="col-side">${signin}${sessions}</div></div>`;
}

function detBankedHTML(a) {
  if (!hasBankedResets(a)) return '';
  const r = a.banked_resets;
  const count = !r?.error ? r?.inventory?.available : null;
  const review = ['pending', 'unknown'].includes(r?.operation?.status);
  if (!count && !review) return `<div class="banked"><span>↻ ${esc(bankedLabel(a))}</span><span class="grow"></span><button class="btn sm" type="button" data-act="banked-details" data-id="${esc(a.id)}" aria-haspopup="dialog">Details</button></div>`;
  return `<div class="banked"><span${review ? ' class="warn"' : ''}>↻ ${review ? 'Reset needs review' : `${count} reset${count === 1 ? '' : 's'} banked`}</span><span class="grow">${esc(bankedPolicyText(a))}</span><button class="btn sm" type="button" data-act="banked-details" data-id="${esc(a.id)}" aria-haspopup="dialog">${review ? 'Review' : 'Reset details'}</button></div>`;
}

function windowStarterHTML(a) {
  if (!a.window_starter_supported) return '';
  const v = a.window_starter;
  // Without a 5-hour limit there is no window to start, so there is no starter either.
  // Fresh quota wins: a 5-hour window reported since the last check means it is back.
  if (noShortLimit(a) || (v?.status === 'not_needed' && !windowOf(a, true))) return '';
  const busy = !!S.windowStarterBusy?.[a.id];
  const reset = windowOf(a, true)?.resets_at || v?.resets_at;
  const active = reset && Date.parse(reset) > Date.now();
  const held = v?.retry_after && Date.parse(v.retry_after) > Date.now() && ['unknown', 'refused'].includes(v.status);
  const message = v?.status === 'not_needed' ? '' : v?.message;
  const status = active ? `Window resets ${when(reset)}` : message || (a.window_starter_selected ? 'Automatic starting is enabled for this account.' : 'Automatic starting is off for this account.');
  const check = active || held;
  return `<div class="window-starter block" aria-label="Window starter"><div class="cell2"><span class="label">Window starter</span>
    <span class="fg2">${esc(status)}</span>
    ${held ? `<span class="small dim">${esc(`Next starter allowed ${when(v.retry_after)}`)}</span>` : ''}
    ${v?.attempted_at ? `<span class="small dim">${esc(`Last requested ${when(v.attempted_at)}${v.automatic ? ' · automatic' : ' · manual'}`)}</span>` : ''}
    <span class="small dim">Starting sends a small Claude request and uses subscription quota.</span></div>
    <div><button type="button" class="btn sm" data-act="${check ? 'window-check' : 'window-start'}" data-id="${esc(a.id)}" ${busy || a.disabled ? 'disabled' : ''}>${busy ? 'Checking…' : check ? 'Check status' : 'Start now'}</button></div></div>`;
}

async function windowStarterAction(id, checkOnly) {
  const a = accountById(id);
  if (!a || S.windowStarterBusy?.[id]) return;
  S.windowStarterBusy ||= {};
  S.windowStarterBusy[id] = true;
  refreshViews();
  try {
    const view = await api(`/accounts/${encodeURIComponent(id)}/window-starter`, {
      method: 'POST', body: JSON.stringify({ request_id: crypto.randomUUID(), check_only: checkOnly }),
    });
    a.window_starter = view;
    toast(view.message);
  } catch (e) {
    a.window_starter = { status: 'unavailable', message: e.message };
    toast(e.message);
  } finally {
    delete S.windowStarterBusy[id];
    await refreshAccounts();
    refreshViews();
  }
}

function detLoadHTML() {
  const a = accountById(S.sub);
  if (!a) return '';
  const act = S.activity[a.id];
  const series = act?.series || Array.from({ length: 60 }, (_, i) => ({ minute: Math.floor(Date.now() / 60000) - 59 + i, requests: 0, failed: 0, tokens: 0 }));
  const c = seriesCounters(series);
  const v = (k) => (act ? fmt(c[k] || 0) : '—');
  const tokens = (k) => act ? aggregateTokensHTML(c, k) : '—';
  const stats = [[v('requests'), 'Requests'], [v('failed'), 'Failed'], [v('cancelled'), 'Cancelled']];
  if (mob()) stats.push([act ? fmt(act.sessions.length) : '—', 'Sessions']);
  stats.push([tokens('input_tokens'), 'Tokens in'], [tokens('output_tokens'), 'Tokens out'], [tokens('cache_tokens'), 'Cached'], ...usageStats(c));
  return `<div class="load-head"><span class="label">Load · last 60 min</span><dl class="stats">${stats.map(([n, k]) => `<div><dt>${k}</dt><dd>${n}</dd></div>`).join('')}</dl></div>
    <div class="bars h72">${barsHTML(series)}</div><div class="axis" aria-hidden="true"><span>60 min ago</span><span>now</span></div>`;
}

function detRequestsHTML() {
  const a = accountById(S.sub);
  if (!a) return '';
  const rows = S.requests.filter((r) => accountOf(r) === a).slice(0, 6);
  if (!rows.length) return '';
  const all = `<a class="link" href="#/requests?acc=${encodeURIComponent(a.id)}">${mob() ? 'All →' : 'All for this account →'}</a>`;
  if (mob()) {
    return `<section class="card" style="padding:6px 14px 4px" aria-label="Recent requests"><div class="card-head" style="min-height:40px"><span class="label">Recent requests</span>${all}</div>
      ${rows.map((r) => `<div class="sess"><div class="sess-top" style="font-size:12px"><span class="mono fg2">${clock(r.ts)}</span><span class="mono ellipsis grow" style="font-size:13px">${esc(r.model)}</span>${statusCell(r)}<span class="mono dim" style="width:44px;text-align:right">${ms(r.latency_ms)}</span></div><span style="font-size:12px">${noteHTML(r) || '<span class="dim">—</span>'}</span></div>`).join('')}</section>`;
  }
  return `<section class="card table flat det-reqs" style="padding:14px 20px 6px" aria-label="Recent requests"><div class="card-head" style="margin-bottom:8px"><span class="label">Recent requests</span>${all}</div>
    ${rows.map((r) => `<div class="tr"><span class="t">${clock(r.ts)}</span><span class="c-route">${routeHTML(r, true)}</span><span class="m">${esc(r.model)}</span><span class="acct-note">${noteHTML(r) || '—'}</span>${statusCell(r)}<span class="n c-ft">${ms(r.ttft_ms)}</span><span class="n">${ms(r.latency_ms)}</span></div>`).join('')}</section>`;
}

function storedIn(a) {
  if (a.file) return `${home(S.overview.auth_dir)}/${S.private ? who(a.file) : a.file}`;
  if (a.provider === 'openai-compat') return `config.yaml › openai-compatibility › ${a.group || ''}`;
  return `config.yaml › ${a.provider === 'codex' ? 'codex' : a.provider}-api-key`;
}

// Which accounts take new sessions first. Higher priorities go first; the rest only take
// over when those can't (the file's or key's `priority`).
function detPriorityHTML(a) {
  const p = a.priority || 0;
  const pick = p > 0 ? 'first' : p < 0 ? 'last' : 'normal';
  const options = [['first', 'First', 10], ['normal', 'Normal', 0], ['last', 'Last', -10]];
  const help = { first: 'Takes new sessions before accounts set to Normal or Last.', normal: 'Takes new sessions by the routing strategy, with the other Normal accounts.', last: 'Takes new sessions only when no other account can.' }[pick];
  return `<section class="card" style="padding:${mob() ? '12px 14px' : '18px 20px'}" aria-label="Priority">
    <div class="card-head"><span class="label">New sessions</span>
      <div class="seg" role="group" aria-label="Priority for new sessions">${options.map(([k, label, value]) => `<button type="button" data-act="priority" data-id="${esc(a.id)}" data-value="${value}" aria-pressed="${pick === k}">${label}</button>`).join('')}</div></div>
    <span class="meta" style="display:block;font-size:12.5px;margin-top:8px">${esc(help)}</span></section>`;
}

async function setPriority(id, priority) {
  const a = accountById(id);
  if (!a || (a.priority || 0) === priority) return;
  try {
    await api(`/accounts/${encodeURIComponent(id)}/priority`, { method: 'POST', body: JSON.stringify({ priority }) });
    a.priority = priority;
    refreshViews();
    toast(priority > 0 ? 'Takes new sessions first' : priority < 0 ? 'Takes new sessions last' : 'Back to normal priority');
    refreshAccounts();
  } catch (e) { toast(e.message); }
}

function detSigninHTML(a) {
  const pad = mob() ? '12px 14px 4px' : '18px 20px 8px';
  const rows = [['Method', authName(a)]];
  if (planName(a)) rows.push(['Plan', `${provName(a)} ${planName(a)}`]);
  if (a.kind === 'oauth' && a.expires_at) {
    const secs = (Date.parse(a.expires_at) - Date.now()) / 1000;
    rows.push(['Token', secs > 0 ? `valid ${span(secs)} · refreshes on its own` : 'expired · refreshes on its own']);
  }
  const models = (a.models || []).slice(0, 24);
  const more = (a.models || []).length - models.length;
  return `<section class="card" style="padding:${pad}" aria-label="Sign-in"><span class="label">Sign-in</span>
    <div style="margin-top:10px">${rows.map(([k, v]) => `<div class="kv"><span>${k}</span><span>${esc(v)}</span></div>`).join('')}
      <div class="kv col"><span>Stored in</span><span class="mono fg2" style="font-size:12px;word-break:break-all">${esc(storedIn(a))}</span></div>
      <div class="kv col" style="gap:6px"><span>Serves</span><span class="chipset">${models.map((m) => `<span class="idchip static">${esc(m)}</span>`).join('')}${more > 0 ? `<span class="idchip static">+${more} more</span>` : ''}${models.length ? '' : '<span class="dim">No models</span>'}</span></div></div>
    ${canSignIn(a) ? `<button class="btn" type="button" data-act="signin-again" data-id="${esc(a.id)}" style="margin:8px 0;${mob() ? 'width:100%;height:44px' : ''}">Sign in again</button>` : ''}</section>`;
}

function detSessionsHTML() {
  const a = accountById(S.sub);
  if (!a) return '';
  const act = S.activity[a.id];
  const list = act ? act.sessions : null;
  const head = `<div class="card-head"><span class="label">Pinned sessions</span><span class="mono dim" style="font-size:12px">${list ? list.length : '—'}</span></div>
    <span class="meta" style="display:block;font-size:12.5px;margin:4px 0 10px">Coding sessions stay on this account so their prompt cache keeps working.</span>`;
  if (!list) return `${head}<div class="sess"><span class="dim" style="font-size:13px">Loading…</span></div>`;
  if (!list.length) {
    const note = S.overview.session_affinity === false
      ? 'Session affinity is off, so requests aren’t pinned to accounts. Turn it on under Config, Routing.'
      : a.disabled ? 'None. A disabled account takes no new sessions.' : 'No coding sessions on this account right now.';
    return `${head}<div class="sess"><span class="fg2" style="font-size:13px;text-wrap:pretty">${note}</span></div>`;
  }
  return head + list.map((s) => {
    const fp = s.session ? `<button class="sess-btn" type="button" data-act="filter-session" data-id="${esc(s.session)}" title="Show this session’s requests">${esc(s.session.slice(0, 8))}</button>` : '<span class="dim mono">unknown</span>';
    const client = s.client_app || (s.client ? `${CLIENT[s.client] || s.client} client` : '');
    const since = s.since ? `since ${hm(s.since)}` : `seen ${ago(s.last_seen)}`;
    const coverage = usageCoverageText(s);
    const meta = s.requests ? `<span class="mono">${esc(s.model || '')}</span><span>· ${plural(s.requests, 'recent req', 'recent req')} · ${aggregateTokensHTML(s, 'cache_tokens')} cached${coverage ? ` · usage: ${coverage}` : ''}</span>` : '<span>No requests in the last 300</span>';
    const quota = sessionQuotaText(a, s);
    const from = s.source ? `<div class="sess-meta"><span>from <span class="mono">${esc(sourceText(s.source))}</span>${s.key_label ? ` · key ${esc(keyText(s.key_label))}` : ''}</span></div>` : '';
    return `<div class="sess"><div class="sess-top">${fp}<span class="fg2">${esc(client)}</span>${s.active ? '<span class="dot s6 ok" title="A request is in flight"></span>' : ''}<span class="grow"></span><span class="dim" style="font-size:12px">${since}</span></div><div class="sess-meta">${meta}</div>${from}${quota ? `<div class="sess-quota" title="Estimated from quota readings">${esc(quota)}</div>` : ''}</div>`;
  }).join('');
}

// What a session used of the account's current windows: "4% of week · 11% of 5h".
function sessionQuotaText(a, s) {
  const quota = s.quota || {};
  const keys = Object.keys(quota).filter((k) => quota[k] > 0);
  if (!keys.length) return '';
  const order = (a.quota_attribution?.current || []).map((x) => x.key);
  const rank = (k) => (order.includes(k) ? order.indexOf(k) : order.length);
  return keys.sort((x, y) => rank(x) - rank(y)).map((k) => {
    const share = currentShare(a, k);
    return `${sharePercent(quota[k])} of ${share ? windowName(share.name, share.model) : k.replace(' · ', ' ')}`;
  }).join(' · ');
}

// connect, sign-in and API key panels ------------------------------------

function panelHTML() {
  if (S.panel === 'key') return keyPanelHTML();
  if (S.panel === 'connect') return connectPanelHTML();
  if (S.panel === 'vertex') return vertexPanelHTML();
  if (S.panel === 'vertex-done') return doneHTML('Vertex AI', S.login);
  if (LOGIN[S.panel]) return S.login && S.login.kind === 'device' ? devicePanelHTML() : loginPanelHTML();
  return '';
}

function connectPanelHTML() {
  return `<div class="panel" role="region" aria-label="Connect an account">
    <h3>Connect an account</h3>
    <p>Sign in with a subscription. Credentials are stored in the auth directory on this machine.</p>
    <div class="choices">${SIGNIN.map(([id, name, sub]) => `
      <button class="choice" type="button" data-act="start-login" data-provider="${id}">
        ${logo(id)}<span class="cell2"><span>${name}</span><span>${sub}</span></span>
      </button>`).join('')}
    </div>
    <div class="actions" style="margin-top:16px"><button class="btn ghost" type="button" data-act="close-panel">Cancel</button></div>
  </div>`;
}

function loginStatus(L, port) {
  if (!L || L.status === 'starting') return '<span class="wait"><span class="pulse"></span>Opening the sign-in page…</span>';
  if (L.status === 'done') return `<span class="ok">Connected ${esc(who(L.message) || '')}</span>`;
  if (L.status === 'error') return `<span class="err">${esc(hideEmails(L.message) || 'Sign-in failed')}</span>`;
  if (L.kind === 'device') return '<span class="wait"><span class="pulse"></span>Waiting for you to approve…</span>';
  if (L.callback) return '<span class="wait"><span class="pulse"></span>Waiting for you to approve in the browser…</span>';
  return `<span class="warn">This server can't receive the redirect${port ? ` (port ${port} is busy)` : ''}. Paste the URL below.</span>`;
}

function doneHTML(name, L) {
  return `<div class="panel" role="region" aria-label="Sign in with ${name}">
    <h3>Signed in</h3><p>${esc(who(L.message) || '')} is ready to serve requests.</p>
    <div class="actions"><button class="btn" type="button" data-act="close-panel">Done</button></div></div>`;
}

const reopen = (L, text) => (L && L.url ? ` <a class="link" href="${esc(L.url)}" target="_blank" rel="noopener">${text} ${ICON.external.replace('<svg', '<svg style="width:12px;height:12px;vertical-align:-1px"')}</a>` : '');

function loginPanelHTML() {
  const L = S.login;
  const info = LOGIN[S.panel];
  if (L && L.status === 'done') return doneHTML(info.name, L);
  return `<div class="panel" role="region" aria-label="Sign in with ${info.name}">
    <h3>Sign in with ${info.name}</h3>
    <p>${info.intro} Credentials stay on this machine.</p>
    <ol class="steps">
      <li><span class="n">1</span><div class="t"><b>Approve access</b> in the tab that opened.${reopen(L, 'Open sign-in page again')}</div></li>
      <li><span class="n">2</span><div class="t" aria-live="polite">${loginStatus(L, info.port)}</div></li>
    </ol>
    <div class="divider"></div>
    <form class="field" data-form="paste">
      <label for="paste-url">Signed in from another device? Paste the address the browser was sent to</label>
      <div class="inline">
        <input id="paste-url" type="text" name="input" placeholder="${esc(info.example)}" autocomplete="off" spellcheck="false" ${L && L.state ? '' : 'disabled'}>
        <button class="btn" type="submit" ${L && L.state ? '' : 'disabled'}>Connect</button>
      </div>
      <small>After you approve, that localhost page won't load when the browser runs elsewhere. Copy its full address from the address bar.</small>
      ${L && L.error ? `<p class="msg err" role="alert">${esc(L.error)}</p>` : ''}
    </form>
    <div class="actions" style="margin-top:18px"><button class="btn ghost" type="button" data-act="close-panel">Cancel</button></div>
  </div>`;
}

function devicePanelHTML() {
  const L = S.login;
  const info = LOGIN[S.panel];
  if (L.status === 'done') return doneHTML(info.name, L);
  let host = '';
  try { host = new URL(L.url).host; } catch {}
  return `<div class="panel" role="region" aria-label="Sign in with ${info.name}">
    <h3>Sign in with ${info.name}</h3>
    <p>${info.intro} Credentials stay on this machine.</p>
    <ol class="steps">
      <li><span class="n">1</span><div class="t"><b>Open ${esc(host || 'the sign-in page')}</b> in the tab that opened.${reopen(L, 'Open it again')}</div></li>
      <li><span class="n">2</span><div class="t"><b>Check the code matches</b> <span class="code-chip">${esc(L.user_code || '')}</span> <button class="linkbtn" type="button" data-act="copy" data-text="${esc(L.user_code || '')}" data-toast="Code copied">Copy</button></div></li>
      <li><span class="n">3</span><div class="t" aria-live="polite">${loginStatus(L)}</div></li>
    </ol>
    <div class="actions" style="margin-top:18px"><button class="btn ghost" type="button" data-act="close-panel">Cancel</button></div>
  </div>`;
}

function vertexPanelHTML() {
  return `<form class="panel" data-form="vertex" aria-label="Add a Vertex AI service account">
    <h3>Add a Vertex AI service account</h3>
    <p>Paste a Google Cloud service account key (JSON) with the Vertex AI User role. It is saved to the auth directory.</p>
    <div class="grid">
      <label class="field wide"><span>Service account key</span><textarea name="json" rows="6" spellcheck="false" placeholder='{ "type": "service_account", "project_id": "…", "private_key": "…", "client_email": "…" }' required></textarea></label>
      <label class="field"><span>Region</span><input type="text" name="location" placeholder="us-central1" autocomplete="off" spellcheck="false"><small>Use global for the newest models.</small></label>
    </div>
    <div class="actions"><button class="btn primary" type="submit">Add service account</button><button class="btn ghost" type="button" data-act="close-panel">Cancel</button></div>
    <p class="msg" id="vertex-msg" aria-live="polite"></p>
  </form>`;
}

function keyPanelHTML() {
  const p = S.keyProvider;
  const opts = [['claude', 'Claude'], ['codex', 'OpenAI'], ['gemini', 'Gemini'], ['vertex', 'Vertex AI'], ['kimi', 'Kimi'], ['xai', 'xAI'], ['meta', 'Meta'], ['compat', 'OpenAI-compatible']];
  const base = {
    claude: 'https://api.anthropic.com', codex: 'https://api.openai.com/v1', gemini: 'https://generativelanguage.googleapis.com',
    vertex: 'https://aiplatform.googleapis.com', kimi: 'https://api.kimi.com/coding', xai: 'https://api.x.ai/v1',
    meta: 'https://api.meta.ai/v1', compat: 'https://openrouter.ai/api/v1',
  }[p];
  const compat = p === 'compat';
  return `<form class="panel" data-form="key" aria-label="Add an API key">
    <h3>Add an API key</h3>
    <p>Keys are saved to <span class="mono">config.yaml</span> and used alongside your signed-in accounts.</p>
    <div class="seg" role="group" aria-label="Provider">${opts.map(([id, label]) => `<button type="button" data-act="key-provider" data-id="${id}" aria-pressed="${p === id}">${logo(id, null, 'api-key', 14)}${label}</button>`).join('')}</div>
    <div class="grid">
      <label class="field wide"><span>API key${compat ? ' (optional for local servers)' : ''}</span><input type="password" name="api_key" autocomplete="off" spellcheck="false" ${compat ? '' : 'required'}></label>
      <label class="field ${compat ? '' : 'wide'}"><span>Base URL${compat ? '' : ' (optional)'}</span><input type="url" name="base_url" placeholder="${esc(base)}" ${compat ? 'required' : ''}></label>
      ${compat ? `<label class="field"><span>Name</span><input type="text" name="name" placeholder="openrouter"></label>
      <label class="field wide"><span>Models</span><input type="text" name="models" placeholder="moonshotai/kimi-k3, kimi=moonshotai/kimi-k3" required><small>Comma separated. Write alias=upstream-name to expose a model under a shorter name.</small></label>` : ''}
    </div>
    <div class="actions"><button class="btn primary" type="submit">Add key</button><button class="btn ghost" type="button" data-act="close-panel">Cancel</button></div>
    <p class="msg" id="key-msg" aria-live="polite"></p>
  </form>`;
}

function openPanel(panel) {
  S.panel = panel;
  if (panel !== 'vertex-done' && !LOGIN[panel]) S.login = null;
  if (S.route !== 'accounts' || S.sub) { location.hash = '#/accounts'; return; }
  patch('acct-panel', panelHTML);
  patch('acct-head', accountHeadHTML);
  $('#acct-panel input, #acct-panel textarea, #acct-panel button')?.focus();
  $('#acct-panel')?.scrollIntoView({ block: 'nearest' });
}

// banked resets ----------------------------------------------------------

// Opt-in (banked-resets in config.yaml): it relies on unofficial provider endpoints.
function hasBankedResets(a) { return !!S.overview?.banked_resets && a.kind === 'oauth' && ['claude', 'codex'].includes(a.provider); }
function bankedLabel(a) {
  const r = a.banked_resets;
  if (['pending', 'unknown'].includes(r?.operation?.status)) return 'Reset needs review';
  const count = !r?.error ? r?.inventory?.available : null;
  return count == null ? 'Resets unavailable' : `${count} reset${count === 1 ? '' : 's'} banked`;
}
function bankedPolicyText(a) {
  const plan = a.banked_reset_plan;
  if (plan?.auto_redeem) return `${plan.prioritized ? 'Prioritized for new work. ' : ''}${plan.message}${plan.expiry_redeem_at ? ` Expiry redemption starts ${when(plan.expiry_redeem_at)}.` : ''}`;
  return 'Manual use asks for confirmation. Automatic redemption is off for this account.';
}
// "↻ 2 resets banked" under an account's status: only resets you have, or one that needs a decision.
function bankedLineHTML(a) {
  if (!hasBankedResets(a)) return '';
  const r = a.banked_resets;
  const review = ['pending', 'unknown'].includes(r?.operation?.status);
  const count = !r?.error ? r?.inventory?.available : null;
  if (!review && !count) return '';
  const now = Date.now();
  const expiring = !review && !r?.error && (r?.inventory?.grants || []).some((g) =>
    g.remaining > 0 && Date.parse(g.expires_at) > now && Date.parse(g.expires_at) <= now + 24 * 60 * 60 * 1000);
  const prioritized = !!a.banked_reset_plan?.prioritized;
  const hint = prioritized ? ' Prioritized: a banked reset is nearing expiry.' : expiring ? ' A reset expires within 24 hours.' : '';
  return `<button type="button" class="resets-line${review || expiring || prioritized ? ' warn' : ''}" data-act="banked-details" data-id="${esc(a.id)}" aria-haspopup="dialog" aria-controls="banked-reset-modal" aria-label="${esc(bankedLabel(a))} for ${esc(acctLabel(a))}.${hint}" title="View saved resets and expiry dates.${hint}">↻ ${esc(review ? 'Review reset' : bankedLabel(a))}${prioritized ? ' · prioritized' : ''}</button>`;
}
function resetButton(a, act, label, disabled = false, extra = '') {
  return `<button type="button" class="btn sm ${['banked-open', 'banked-confirm'].includes(act) ? 'primary' : ''}" data-act="${act}" data-id="${esc(a.id)}" ${disabled ? 'disabled' : ''} ${extra}>${label}</button>`;
}
// The browser's locale orders the date; the clock is always 24-hour, as elsewhere.
function resetDate(value) {
  return value ? new Date(value).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short', hourCycle: 'h23' }) : 'No expiry reported';
}
function grantDetail(g) {
  return `${g.expires_at ? `Expires ${resetDate(g.expires_at)}` : 'No expiry reported'} · Clears ${g.clears.join(', ') || 'provider limits'}`;
}
function bankedResetsHTML(a) {
  const local = S.resets[a.id] || {}, r = a.banked_resets, inv = r?.inventory, op = r?.operation, d = local.dialog;
  const uncertain = ['pending', 'unknown'].includes(op?.status);
  const stale = !r || !(Date.parse(r.checked_at) > Date.now() - 5 * 60 * 1000);
  const expiryChanged = inv?.grants?.some((g) => g.usable && ((g.expires_at && Date.parse(g.expires_at) <= Date.now()) || (g.starts_at && Date.parse(g.starts_at) > Date.now())));
  const usable = !local.busy && !a.disabled && !r?.error && !stale && !expiryChanged && inv?.eligible && r?.quote && !uncertain;
  let title = 'Banked resets', content, controls;
  if (d) {
    const confirmedInventory = d.inventory || inv;
    const grant = confirmedInventory?.grants?.find((g) => g.id === d.grant);
    if (d.action === 'redeem') {
      title = 'Use 1 reset?';
      const choices = (confirmedInventory?.grants || []).filter((g) => g.usable);
      const selection = a.provider === 'claude' && choices.length > 1
        ? `<label class="reset-selection">Grant<select data-reset-grant="${esc(a.id)}" ${local.busy ? 'disabled' : ''}>${choices.map((g) => `<option value="${esc(g.id)}" ${g.id === d.grant ? 'selected' : ''}>${esc(g.label)} · ${g.remaining} left</option>`).join('')}</select></label>`
        : grant ? `<p class="reset-grant-name">${esc(grant.label)}</p>` : '';
      const codexNote = a.provider !== 'codex' ? '' : grant ? '<p class="sub">Uses the reset that expires first.</p>' : '<p class="sub">Codex chooses the reset and restores its subscription limits.</p>';
      content = `<p>This uses one saved reset.</p>${selection}${grant ? `<p class="sub">${esc(grantDetail(grant))}</p>` : ''}${codexNote}`;
      controls = `${resetButton(a, 'banked-cancel', 'Back', local.busy)}${resetButton(a, 'banked-confirm', local.busy ? 'Applying…' : 'Apply reset', local.busy || a.disabled)}`;
    } else if (d.action === 'retry') {
      title = 'Retry reset request?';
      content = '<p>A reset may already have been used. This retries the saved request.</p>';
      controls = `${resetButton(a, 'banked-cancel', 'Back', local.busy)}${resetButton(a, 'banked-confirm', local.busy ? 'Retrying…' : 'Confirm retry', local.busy || a.disabled)}`;
    } else {
      title = 'Resolve reset outcome';
      content = '<p>Check your provider account first, then record the result.</p>';
      controls = `${resetButton(a, 'banked-cancel', 'Back', local.busy)}${resetButton(a, 'banked-confirm', 'Reset was used', local.busy, 'data-resolution="resolve-used"')}${resetButton(a, 'banked-confirm', 'No reset was used', local.busy, 'data-resolution="resolve-unused"')}`;
    }
  } else {
    const grants = (inv?.grants || []).map((g) => `<li><div class="reset-grant-head"><b>${esc(g.label || 'Subscription reset')}</b><span>${g.remaining} left</span></div><p class="sub">${esc(grantDetail(g))}</p>${g.reason ? `<p class="sub">${esc(g.reason)}</p>` : ''}</li>`).join('');
    const reason = a.disabled ? 'Enable this account to apply a reset.' : stale || expiryChanged ? 'Refresh to check availability.' : inv?.reason;
    const message = uncertain ? '<p class="warn">A reset may have been used. New resets are blocked until resolved.</p>' : op ? `<p class="${['applied', 'reconciled_used'].includes(op.status) ? 'ok' : 'sub'}" role="status">${esc(op.message)}</p>` : '';
    const plan = a.banked_reset_plan;
    const policy = plan ? `<p class="sub">${esc(bankedPolicyText(a))}</p>` : '';
    const automatic = op?.automatic ? '<p class="sub">This reset was requested automatically under your routing policy.</p>' : '';
    content = `<p class="reset-count">${esc(bankedLabel(a))}${a.provider === 'claude' && inv?.applicable != null && inv.applicable !== inv.available && !r?.error ? `<span class="sub"> · ${inv.applicable} usable now</span>` : ''}</p>${grants ? `<ul class="reset-grants">${grants}</ul>` : ''}${policy}${reason && !r?.error ? `<p class="sub">${esc(reason)}</p>` : ''}${message}${automatic}`;
    controls = `${resetButton(a, 'banked-refresh', local.busy ? 'Checking…' : 'Refresh', local.busy)}${resetButton(a, 'banked-open', 'Use 1 reset', !usable, r?.checked_at ? `data-reset-deadline="${esc(new Date(Date.parse(r.checked_at) + 5 * 60 * 1000).toISOString())}"` : '')}`;
    if (uncertain) {
      const retryable = a.provider === 'claude' && r.retryable && Date.parse(op.retry_until) > Date.now();
      controls = `${resetButton(a, 'banked-refresh', local.busy ? 'Checking…' : 'Refresh', local.busy)}${retryable ? resetButton(a, 'banked-retry', 'Retry request', local.busy || a.disabled, `data-reset-deadline="${esc(op.retry_until)}"`) : ''}${resetButton(a, 'banked-resolve', 'Check outcome', local.busy)}`;
    }
  }
  return `<header class="reset-modal-head"><div><h2 id="reset-modal-title">${title}</h2><p id="reset-modal-account">${esc(acctLabel(a))} · ${esc(provName(a))}</p></div><button type="button" class="icon-btn reset-close" data-act="banked-close" data-id="${esc(a.id)}" aria-label="Close reset details">×</button></header>
    <div class="reset-modal-body" aria-busy="${!!local.busy}">${content}${local.error || r?.error ? `<p class="err" role="alert">${esc(local.error || r.error)}</p>` : ''}</div>
    <footer class="reset-modal-footer">${!d && r ? `<span class="sub">Checked ${liveAgo(r.checked_at)}</span>` : ''}<div class="actions">${controls}</div></footer>`;
}
function syncResetModal() {
  if (!S.resetModal) return;
  const a = accountById(S.resetModal);
  if (!a || S.locked) { closeResetModal(); return; }
  patch('banked-reset-modal', () => bankedResetsHTML(a));
  const modal = $('#banked-reset-modal');
  if (!modal.open) modal.showModal();
}
function closeResetModal() {
  const id = S.resetModal;
  if (!id) return;
  S.resetModal = null;
  if (S.resets[id]) S.resets[id].dialog = null;
  $('#banked-reset-modal').close();
  document.querySelector(`[data-act="banked-details"][data-id="${CSS.escape(id)}"]`)?.focus({ preventScroll: true });
}
function patchResets() {
  refreshViews();
  syncResetModal();
}
async function bankedAction(act, id, resolution) {
  let a = accountById(id);
  if (!a) return;
  const local = S.resets[id] ||= {};
  if (act === 'banked-details') {
    S.resetModal = id;
    local.dialog = null;
    syncResetModal();
    if (local.busy) return;
  }
  if (local.busy) return;
  if (act === 'banked-cancel') { local.dialog = null; syncResetModal(); $('#banked-reset-modal [data-act="banked-close"]')?.focus(); return; }
  if (act === 'banked-resolve' || act === 'banked-retry') {
    local.dialog = { action: act === 'banked-retry' ? 'retry' : 'resolve', request: a.banked_resets?.operation?.request_id };
    syncResetModal(); $('#banked-reset-modal [data-act="banked-cancel"]')?.focus(); return;
  }
  local.busy = true; local.error = null;
  patchResets();
  try {
    const path = `/accounts/${encodeURIComponent(id)}/banked-resets`;
    if (['banked-details', 'banked-refresh', 'banked-open'].includes(act)) {
      const r = await api(act === 'banked-refresh' ? `/accounts/${encodeURIComponent(id)}/quota/refresh` : path, act === 'banked-refresh' ? { method: 'POST' } : {});
      a = accountById(id) || a;
      a.banked_resets = r;
      if (act === 'banked-open' && S.resetModal === id && r.quote && r.inventory?.eligible && !r.error) {
        local.dialog = { action: 'redeem', request: r.quote, grant: r.inventory.selected_grant || '', inventory: r.inventory };
      }
    } else if (act === 'banked-confirm' && local.dialog) {
      const d = local.dialog;
      const result = await api(path, { method: 'POST', body: JSON.stringify({ action: d.action === 'resolve' ? resolution : d.action, request_id: d.request, grant_id: d.grant || '', confirmed: true }) });
      a = accountById(id) || a;
      a.banked_resets = result;
      local.dialog = null;
    }
  } catch (e) {
    local.error = e.message;
    if (act === 'banked-confirm') {
      local.dialog = null;
      a = accountById(id) || a;
      if (a.banked_resets) a.banked_resets.quote = null;
      local.error += ' Refresh status before continuing.';
    }
  } finally {
    local.busy = false;
    patchResets();
    refreshAccounts();
    if (S.resetModal === id) $('#banked-reset-modal [data-act="banked-cancel"], #banked-reset-modal [data-act="banked-close"]')?.focus();
  }
}
document.addEventListener('change', (e) => {
  if (e.target.matches('[data-reset-grant]')) {
    const local = S.resets[e.target.dataset.resetGrant];
    if (local?.dialog) { local.dialog.grant = e.target.value; syncResetModal(); }
  }
});
const resetModal = $('#banked-reset-modal');
resetModal.addEventListener('cancel', (e) => { e.preventDefault(); closeResetModal(); });
resetModal.addEventListener('keydown', (e) => {
  if (e.key !== 'Tab') return;
  const targets = [...resetModal.querySelectorAll('button:not([disabled]), select:not([disabled])')];
  const first = targets[0], last = targets.at(-1);
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
});
resetModal.addEventListener('click', (e) => {
  const box = resetModal.getBoundingClientRect();
  if (e.target === resetModal && (e.clientX < box.left || e.clientX > box.right || e.clientY < box.top || e.clientY > box.bottom)) closeResetModal();
});

// ---------------------------------------------------------------- requests

function textMatches(r) {
  const q = S.filter.trim().toLowerCase();
  if (!q) return true;
  const acct = accountOf(r);
  const attempts = (r.routing_attempts || []).flatMap((a) => [a.account, a.previous_account, a.reason, routingReason(a.reason)]);
  return [r.model, r.account, acct && acctLabel(acct), acct && provName(acct), r.provider, r.client, CLIENT[r.client], r.client_app, String(r.status), r.error,
    r.source?.ip, r.source?.login, r.key_label,
    r.session_id, r.session_source, r.routing_strategy, r.routing_reason, routingReason(r.routing_reason),
    r.routing_warning, ...(ROUTING_WARNING[r.routing_warning] || []), ...attempts,
  ].some((s) => s != null && String(s).toLowerCase().includes(q));
}

// The account, session and text filters (the chips come on top).
const requestFilter = (r) => (!S.reqAcc || r.account_id === S.reqAcc || accountOf(r)?.id === S.reqAcc)
  && (!S.reqSess || r.session_id === S.reqSess)
  && textMatches(r);
// Requests on screen: newest first, minus any that arrived while paused.
const visibleRequests = () => S.requests.slice(S.paused ? S.pending : 0).filter(requestFilter);
const CHIPS = { all: () => true, errors: isErr, cancelled: (r) => r.status === 499, rerouted: isRerouted };
const chipFilter = () => CHIPS[S.reqChip] || CHIPS.all;
const shownRequests = (base = visibleRequests()) => base.filter(chipFilter());

// How many requests each chip would show, in one pass.
function chipCounts(base) {
  const n = { all: 0, errors: 0, cancelled: 0, rerouted: 0 };
  for (const r of base) {
    n.all++;
    if (isErr(r)) n.errors++;
    if (r.status === 499) n.cancelled++;
    if (isRerouted(r)) n.rerouted++;
  }
  return n;
}

function pauseHTML() {
  const label = S.paused ? (S.pending ? (mob() ? `${S.pending} new` : `Resume · ${S.pending} new`) : 'Resume') : 'Pause';
  return `<button class="btn pause" type="button" data-act="pause" aria-pressed="${S.paused}"${mob() ? ' style="height:44px;font-size:14px;padding:0 14px"' : ''}><span class="dot s7 ${S.paused ? 'warn' : 'ok'}"></span>${label}</button>`;
}

function reqToolsHTML(base = visibleRequests()) {
  const counts = chipCounts(base);
  const chips = [['all', 'All'], ['errors', 'Errors'], ['cancelled', 'Cancelled'], ['rerouted', 'Rerouted']].map(([id, label]) => `<button class="chip" type="button" data-act="req-chip" data-id="${id}" aria-pressed="${S.reqChip === id}">${label} <span class="n">${counts[id]}</span></button>`).join('');
  const acct = S.reqAcc && accountById(S.reqAcc);
  const pills = [];
  if (S.reqAcc) pills.push(['acc', 'Account', acct ? acctLabel(acct) : S.reqAcc.slice(0, 12)]);
  if (S.reqSess) pills.push(['sess', 'Session', S.reqSess.slice(0, 8)]);
  const shown = counts[S.reqChip] ?? counts.all;
  if (mob()) {
    return `<div class="m-filters">${chips}${pills.map(([k, , v]) => `<button class="pill" type="button" data-act="clear-pill" data-id="${k}" aria-label="Remove filter ${esc(v)}"><span class="v">${esc(v)}</span><span class="dim">×</span></button>`).join('')}</div>
      <span class="meta" style="font-size:12px;text-wrap:pretty;display:block;margin-top:12px">${fmt(shown)} shown, newest first. Tap a row to see why it went where it did.</span>`;
  }
  return `<div class="req-tools"><div class="chips" role="group" aria-label="Show">${chips}</div>
    ${pills.map(([k, label, v]) => `<span class="pill"><span class="k">${label}</span><span class="v">${esc(v)}</span><button type="button" data-act="clear-pill" data-id="${k}" aria-label="Remove ${label.toLowerCase()} filter">×</button></span>`).join('')}
    <span class="grow"></span><span class="meta" style="font-size:12.5px">${fmt(shown)} shown</span></div>`;
}

function requestsHTML() {
  const search = `<div class="searchbox"${mob() ? ' style="flex:1;height:44px;padding:0 12px"' : ''}>${ICON.search}<label class="sr-only" for="req-filter">Filter requests</label><input id="req-filter" type="text" placeholder="${mob() ? 'Session, account, address…' : 'Filter by session, account, model, address…'}" value="${esc(S.filter)}" autocomplete="off" spellcheck="false"${mob() ? ' style="font-size:14px"' : ''}>${S.filter && !mob() ? '<button class="clear" type="button" data-act="clear-filter" aria-label="Clear filter">×</button>' : ''}</div>`;
  const head = mob()
    ? `<div style="display:flex;gap:8px">${search}<span id="req-pause" style="display:contents">${pauseHTML()}</span></div>`
    : `<div class="page-head"><div><h1 class="h-page">Requests</h1><p class="meta">The last 300 since the server started, newest first. Click a row to see why it went where it did.</p></div>
        <span class="grow"></span>${search}<span id="req-pause" style="display:contents">${pauseHTML()}</span></div>`;
  const base = visibleRequests();
  return `${head}<div id="req-tools">${reqToolsHTML(base)}</div><div id="req-list">${reqListHTML(base)}</div>`;
}

function reqListHTML(base = visibleRequests()) {
  // The whole log is on its way (the first paint fetched only the newest few).
  if (!S.requestsFull) return skeletonHTML();
  const rows = shownRequests(base);
  const none = S.requests.length ? 'No requests match.' : 'No requests yet. They appear here the moment a client sends one.';
  if (mob()) return `<div class="card lines" id="req-body">${rows.map(reqMobileHTML).join('') || `<p class="empty-line" style="padding:20px 12px">${none}</p>`}</div>`;
  return `<div class="card table req-table" role="table" aria-label="Requests">
    <div class="th" role="row"><span>Time</span><span>Route</span><span>Model</span><span>Account</span><span>Status</span><span class="c-ft r">First token</span><span class="r">Total</span><span class="c-tok r">In</span><span class="c-tok r">Out</span><span class="c-tok r">Cached</span></div>
    <div id="req-body">${rows.map(reqItemHTML).join('') || `<div class="empty" id="req-empty">${none}</div>`}</div></div>`;
}

const isFresh = (r) => (S.fresh.get(r.id) || 0) > Date.now();

function errLineText(r) {
  if (r.status === 499) return cancellationText(r);
  if (!r.error) return '';
  return r.status >= 400 ? String(hideEmails(r.error)) : '';
}

function cachePolicyHTML(r) {
  const p = r.cache_policy;
  const d = r.cache_diagnostics;
  if (!p && !d) return '';
  const labels = { provider_default: 'Provider default', prewarm: 'Prewarm', automatic: 'Automatic', explicit: 'Explicit boundaries',
    explicit_no_breakpoints: 'Explicit without boundaries', none: 'No cache controls', rejected: 'Rejected',
    caller: 'Caller key', session: 'Session', client: 'Client', workspace: 'Workspace',
    session_fallback: 'Session fallback', not_sent: 'No cache key sent' };
  const results = { cache_hit: 'Cache hit', cache_miss: 'Cache miss', comparison_response_not_found: 'Comparison response not found', unavailable: 'Unavailable' };
  const reasons = { model_changed: 'Model changed', prompt_cache_key_changed: 'Prompt cache key changed',
    service_tier_changed: 'Service tier changed', tools_changed: 'Tools changed', text_format_changed: 'Text format changed',
    reasoning_effort_changed: 'Reasoning effort changed', verbosity_changed: 'Verbosity changed',
    context_compacted: 'Context compacted', input_changed: 'Input changed' };
  const counts = [];
  if (d?.type === 'cache_miss') {
    for (const [key, label] of [['comparison_reusable_tokens', 'Comparison reusable tokens'], ['cache_missed_tokens', 'Cache missed tokens']]) {
      if (Number.isSafeInteger(d[key]) && d[key] >= 0) counts.push(`<span>${label}</span>: ${esc(fmt(d[key]))}`);
    }
  }
  const reported = d
    ? `<p><span>Provider-reported result</span>: <span>${Object.hasOwn(results, d.type) ? results[d.type] : 'Unavailable'}</span>${d.type === 'cache_miss' ? ` · <span>${Object.hasOwn(reasons, d.reason) ? reasons[d.reason] : 'Unknown cache miss reason'}</span>` : ''}</p>${counts.length ? `<p>${counts.join(' · ')}</p>` : ''}`
    : '<p>No provider cache diagnostics reported; cache reuse is unconfirmed.</p>';
  const policy = p ? `<p><span>Fusebox policy</span> · <span>Requested</span>: ${esc(labels[p.requested] || p.requested)} · <span>Applied</span>: ${esc(labels[p.applied] || p.applied)} · ${esc(labels[p.key_scope] || p.key_scope)}</p>${p.downgrade_reason ? `<p>${esc(p.downgrade_reason)}</p>` : ''}` : '';
  return `<div class="why-text"><span class="label">Prompt cache</span>${reported}${policy}</div>`;
}

function reqItemHTML(r) {
  const open = S.openReq === r.id;
  const err = errLineText(r);
  const endpoint = (r.transport === 'images' ? '/v1/images' : r.transport === 'video' ? '/v1/videos' : ENDPOINT[r.client] || '') + (r.transport === 'ws' ? ' (websocket)' : '');
  const client = r.client_app || `${CLIENT[r.client] || r.client} format`;
  return `<div class="req-item${isFresh(r) ? ' fresh' : ''}${open ? ' open' : ''}" data-req="${r.id}">
    <div class="tr" data-act="toggle-req" data-id="${r.id}">
      <button class="textbtn t req-toggle" type="button" aria-expanded="${open}" aria-label="Details for the request at ${clock(r.ts)}" title="${esc(r.ts)}">${clock(r.ts)}</button>${routeHTML(r)}<span class="m">${esc(r.model)}</span>${acctCellHTML(r)}${statusCell(r)}
      <span class="n c-ft">${ms(r.ttft_ms)}</span><span class="n">${ms(r.latency_ms)}</span>
      <span class="n c-tok">${tokensText(r, 'input_tokens')}</span><span class="n c-tok">${tokensText(r, 'output_tokens')}</span><span class="n c-tok">${tokensText(r, 'cache_tokens')}</span>
    </div>
    ${err ? `<div class="errline${r.status === 499 ? ' closed' : ''}" title="${esc(err)}">${esc(err)}</div>` : ''}
    ${open ? `<div class="why">
      <div class="why-kv">
        <div><span>Client</span><span>${esc(client)}</span></div>
        <div><span>Endpoint</span><span class="mono">${esc(endpoint)}</span></div>
        <div><span>Session</span><span class="mono" title="${esc(r.session_id || '')}">${esc(r.session_id ? r.session_id.slice(0, 8) : 'none')}</span></div>
        ${r.source ? `<div><span>Source</span><span class="mono">${esc(sourceText(r.source))}</span></div>` : ''}
        ${r.key_label ? `<div><span>Key</span><span>${esc(keyText(r.key_label))}</span></div>` : ''}
        <div class="why-ft"><span>Latency</span><span class="mono">${ms(r.ttft_ms)} to first token</span></div>
        <div class="why-tok"><span>Tokens</span><span class="mono">${requestTokensHTML(r)}</span></div>
      </div>
      <div class="why-text"><span class="label">Why this account</span><p>${esc(whySentence(r))}</p></div>
      ${cachePolicyHTML(r)}
      <div class="why-acts">${accountOf(r) ? `<button class="btn sm" type="button" data-act="open-acc" data-id="${esc(accountOf(r).id)}">Open account</button>` : ''}${r.session_id ? `<button class="btn sm" type="button" data-act="filter-session" data-id="${esc(r.session_id)}">Only this session</button>` : ''}</div>
    </div>` : ''}
  </div>`;
}

function reqMobileHTML(r) {
  const open = S.openReq === r.id;
  const acct = accountOf(r);
  const kind = { ws: 'ws', images: 'image', video: 'video' }[r.transport] || (r.attempts > 1 ? `${r.attempts} tries` : '');
  const err = errLineText(r);
  const client = r.client_app || `${CLIENT[r.client] || r.client} format`;
  const endpoint = (r.transport === 'images' ? '/v1/images' : r.transport === 'video' ? '/v1/videos' : ENDPOINT[r.client] || '') + (r.transport === 'ws' ? ' (websocket)' : '');
  return `<div class="item m-req${isFresh(r) ? ' fresh' : ''}${open ? ' open' : ''}" data-req="${r.id}" data-act="toggle-req" data-id="${r.id}">
    <div class="l1"><button class="textbtn t req-toggle" type="button" aria-expanded="${open}" aria-label="Details for the request at ${clock(r.ts)}">${clock(r.ts)}</button>${r.provider ? logo(r.provider, acct?.group || r.account, acct?.kind, 13) : ''}<span class="rt" style="flex:0 1 auto">${esc(CLIENT[r.client] || r.client)} → ${esc(acct ? provName(acct) : PROVIDER[r.provider] || r.provider || '—')}</span>${kind ? `<span class="tag">${kind}</span>` : ''}<span class="grow"></span>${statusCell(r)}<span class="n">${ms(r.latency_ms)}</span></div>
    <span class="l2">${esc(r.model)}</span>
    <span class="l3">${esc(reqAcctName(r))}${r.routing_reason ? ` · ${noteHTML(r)}` : ''}${r.session_id ? ` · <span class="mono">${esc(r.session_id.slice(0, 8))}</span>` : ''}</span>
    ${err ? `<span class="errline${r.status === 499 ? ' dim' : ''}" style="font-size:11.5px${r.status === 499 ? ';color:var(--fg-3)' : ''}">${esc(err)}</span>` : ''}
    ${open ? `<div class="m-why"><p>${esc(whySentence(r))}</p><span class="meta" style="font-size:12px">${esc(client)} · <span class="mono">${esc(endpoint)}</span></span>
      ${r.source || r.key_label ? `<span class="meta" style="font-size:12px">${r.source ? `From <span class="mono">${esc(sourceText(r.source))}</span>` : ''}${r.source && r.key_label ? ' · ' : ''}${r.key_label ? `key ${esc(keyText(r.key_label))}` : ''}</span>` : ''}
      <span class="meta" style="font-size:12px">First token ${ms(r.ttft_ms)} · ${requestTokensHTML(r)}</span>
      ${cachePolicyHTML(r)}
      <div class="acts">${acct ? `<button class="btn" type="button" data-act="open-acc" data-id="${esc(acct.id)}">Open account</button>` : ''}${r.session_id ? `<button class="btn" type="button" data-act="filter-session" data-id="${esc(r.session_id)}">Session ${esc(r.session_id.slice(0, 8))}</button>` : ''}</div></div>` : ''}
  </div>`;
}

// A live arrival goes on top without re-rendering the list (a frame's worth at a
// time, oldest first).
function insertRequests(logs) {
  patchChanged('req-tools', reqToolsHTML);
  const body = $('#req-body');
  if (!body) return;
  const chip = chipFilter();
  const kept = new Set(S.requests);
  const rows = logs.filter((log) => kept.has(log) && requestFilter(log) && chip(log) && !body.querySelector(`[data-req="${log.id}"]`));
  if (!rows.length) return;
  $('#req-empty')?.remove();
  body.querySelector('.empty-line')?.remove();
  body.insertAdjacentHTML('afterbegin', rows.reverse().map(mob() ? reqMobileHTML : reqItemHTML).join(''));
  while (body.children.length > 300) body.lastElementChild.remove();
}

function refreshRequests() {
  const base = visibleRequests();
  patch('req-tools', reqToolsHTML, base);
  patch('req-list', reqListHTML, base);
  patch('req-pause', pauseHTML);
}

// Keeps the address bar in step with the account and session filters.
function syncRequestsHash() {
  const q = new URLSearchParams();
  if (S.reqAcc) q.set('acc', S.reqAcc);
  if (S.reqSess) q.set('sess', S.reqSess);
  const hash = `#/requests${q.toString() ? `?${q}` : ''}`;
  if (location.hash !== hash) history.replaceState(null, '', hash);
}

// ---------------------------------------------------------------- models

const FAMILIES = [
  ['claude', 'Claude', 'claude', /^claude-/],
  ['gpt', 'GPT & Codex', 'codex', /^(gpt-|codex-|o\d|chatgpt-)/],
  ['gemini', 'Gemini', 'gemini', /^(gemini-|gemma-|imagen-|veo-|text-embedding)/],
  ['grok', 'Grok', 'xai', /^grok-/],
  ['kimi', 'Kimi', 'kimi', /^(kimi-|moonshot-)/],
  ['muse', 'Meta', 'meta', /^muse-/],
  ['swe', 'Devin', 'devin', /^swe-/],
];

const routeSteps = (id) => (S.routes?.models || {})[id] || [];
const modelInfo = (id) => S.models.find((m) => m.id === id) || {};
// `team/model` reaches only the accounts with the team/ prefix; the model is `model`.
const modelPrefix = (id) => modelInfo(id).prefix;
const unprefixed = (id) => { const p = modelPrefix(id); return p ? id.slice(p.length + 1) : id; };

function modelFamilies() {
  const fams = new Map();
  for (const m of S.models) {
    const steps = routeSteps(m.id);
    let key, name, logoKey, group;
    if (m.provider === 'openai-compat') {
      const acct = steps.map((s) => accountById(s.account)).find(Boolean);
      group = acct?.group || 'Compatible';
      key = `compat:${group}`; name = group; logoKey = ['openai-compat', group];
    } else {
      const base = m.id.includes('/') ? m.id.slice(m.id.indexOf('/') + 1) : m.id;
      const f = FAMILIES.find(([, , , re]) => re.test(base.toLowerCase()));
      if (f) { key = f[0]; name = f[1]; logoKey = [f[2], null]; } else { key = 'other'; name = 'Other'; logoKey = ['compat', null]; }
    }
    if (!fams.has(key)) fams.set(key, { key, name, logoKey, ids: [], compat: m.provider === 'openai-compat' });
    fams.get(key).ids.push(m.id);
  }
  const order = (f) => { const i = FAMILIES.findIndex(([k]) => k === f.key); return i >= 0 ? i : f.key === 'other' ? 999 : 100; };
  return [...fams.values()].sort((a, b) => order(a) - order(b) || a.name.localeCompare(b.name));
}

function famPattern(f) {
  if (f.compat) return 'openai-compatibility';
  const pats = [...new Set(f.ids.map((id) => {
    const base = id.includes('/') ? id.slice(id.indexOf('/') + 1) : id;
    const m = base.match(/^[a-z]+\d*-/i);
    return m ? `${m[0].toLowerCase()}*` : base;
  }))];
  return pats.slice(0, 3).join(', ') + (pats.length > 3 ? ', …' : '');
}

// The family's route: its most widely served id's order, then accounts that only serve other ids.
function famRoute(f) {
  const rep = f.ids.reduce((best, id) => (routeSteps(id).length > routeSteps(best).length ? id : best), f.ids[0]);
  const rows = routeSteps(rep).map((s) => ({ ...s }));
  const seen = new Set(rows.map((r) => r.account));
  const models = new Set(), coverage = new Map();
  for (const id of f.ids) {
    const model = unprefixed(id);
    models.add(model);
    for (const s of routeSteps(id)) {
      if (!seen.has(s.account)) { seen.add(s.account); rows.push({ ...s, next: false }); }
      if (!coverage.has(s.account)) coverage.set(s.account, new Set());
      coverage.get(s.account).add(model);
    }
  }
  // Coverage counts models, so an account's prefixed copies don't make the others look partial.
  // Accumulate it while visiting routes, instead of rescanning all routes for every account.
  for (const r of rows) r.serves = coverage.get(r.account).size;
  return { rep, models: models.size, rows: rows.filter((r) => accountById(r.account)) };
}

function failuresByModel() {
  const hourAgo = Date.now() - 3600e3;
  const out = {};
  for (const r of S.requests) {
    if (Date.parse(r.ts) < hourAgo) break;
    if (isErr(r)) (out[r.model] ||= []).push(r);
  }
  return out;
}

function routeStepText(step, a) {
  const st = acctState(a);
  if (step.state === 'disabled' || a.disabled) return { txt: 'Disabled · skipped', short: 'Disabled', cls: 'off' };
  if (step.state === 'cooling' || st.cls === 'cooling') {
    if (st.resetUnknown) return { txt: 'Quota used up · Reset time unknown', short: 'Quota used up', cls: 'warn' };
    const until = step.until || st.until;
    return { txt: `Cooling · back ${when(until)}`, short: `Back ${hm(until)}`, cls: 'warn' };
  }
  if (st.cls === 'error') return st.signin ? { txt: 'Sign-in expired', short: 'Sign-in expired', cls: 'err' } : { txt: 'Error', short: 'Error', cls: 'err' };
  if (step.banked_reset_expires_at) return { txt: `Reset expires ${when(step.banked_reset_expires_at)}`, short: 'Expiring reset', cls: 'warn' };
  const w = windowOf(a, true) || windowOf(a, false);
  const q = quotaOf(w);
  if (q) {
    const pct = `${quotaPercent(q[S.quotaDisplay])} ${quotaWord()}`;
    return { txt: `${pct} · ${windowTitle(w).toLowerCase()}`, short: pct, cls: 'q' };
  }
  if (step.fallback) return { txt: 'Fallback', short: 'Fallback', cls: '' };
  return { txt: 'Ready', short: 'Ready', cls: '' };
}

function modelsHTML() {
  const search = `<div class="searchbox"${mob() ? ' style="flex:none;height:44px;padding:0 12px"' : ' style="flex:0 1 280px"'}>${ICON.search}<label class="sr-only" for="model-filter">Find a model</label><input id="model-filter" type="text" placeholder="Find a model" value="${esc(S.modelFilter)}" autocomplete="off" spellcheck="false"${mob() ? ' style="font-size:14px"' : ''}>${S.modelFilter && !mob() ? '<button class="clear" type="button" data-act="clear-model-filter" aria-label="Clear">×</button>' : ''}</div>`;
  const play = `<button class="btn" type="button" data-act="open-play" aria-expanded="${!!S.play}">Try a model</button>`;
  const panels = `<div id="models-panel" style="display:contents">${modelsPanelHTML()}</div><div id="models-play" style="display:contents">${playHTML()}</div>`;
  if (mob()) return `${search}<div id="models-head">${modelsHeadHTML()}</div>${panels}<div id="models-root" style="display:contents">${modelsBodyHTML()}</div><div id="models-groups" style="display:contents">${groupsCardHTML()}</div><div class="acct-actions">${play}</div>`;
  const add = `<button class="btn" type="button" id="add-model-btn" data-act="add-model" aria-expanded="${!!S.addModel}">Add model</button>`;
  return `<div class="page-head"><div><h1 class="h-page">Models</h1><p class="meta" id="models-count">${modelsCountText()}</p></div><span class="grow"></span>${play}${add}${search}</div>
    <div id="models-head">${modelsHeadHTML()}</div>${panels}<div id="models-root">${modelsBodyHTML()}</div><div id="models-groups">${groupsCardHTML()}</div>`;
}

// Model groups from config: each name, how it uses its models, and where Claude Code's
// background calls go.
function groupsCardHTML() {
  const groups = Object.entries(S.routes?.groups || {});
  const background = S.routes?.background_model;
  if (!groups.length && !background) return '';
  const line = (name, how, models, sep) => `<div class="group-line">${name}<span class="meta">${how}</span><span class="chipset">${models.map((m) => `<span class="idchip static">${esc(m)}</span>`).join(`<span class="sep-mark" aria-hidden="true">${sep}</span>`)}</span></div>`;
  return `<section class="card groups" aria-label="Model groups">
    <div class="card-head"><span class="label">Model groups</span><a class="link" href="#/config/routing">Change in Config →</a></div>
    ${groups.map(([name, g]) => line(`<button class="idchip" type="button" data-act="copy-model" data-id="${esc(name)}" title="Copy ${esc(name)}">${esc(name)}</button>`, g.balance ? 'spreads sessions across' : 'tries in order', g.models, g.balance ? '·' : '→')).join('')}
    ${background ? line('<span class="idchip static">Claude Code background calls</span>', 'go to', [background], '') : ''}</section>`;
}

// The playground: one prompt to one model, through routing like any request, streamed.
function playHTML() {
  const P = S.play;
  if (!P) return '';
  const ids = [...Object.keys(S.routes?.groups || {}), ...S.models.map((m) => m.id)];
  return `<section class="card pad play" aria-label="Try a model">
    <div class="card-head"><span class="label">Try a model</span><button class="icon-btn" type="button" data-act="close-play" aria-label="Close the playground">×</button></div>
    <form id="play-form" class="play-form">
      <div class="field"><label for="play-model">Model</label><input id="play-model" list="play-models" value="${esc(P.model)}" required autocomplete="off" spellcheck="false" placeholder="claude-sonnet-5-5">
        <datalist id="play-models">${ids.map((id) => `<option value="${esc(id)}">`).join('')}</datalist></div>
      <div class="field"><label for="play-prompt">Prompt</label><textarea id="play-prompt" rows="3" required>${esc(P.prompt)}</textarea></div>
      <div class="play-actions"><button class="btn primary" type="submit"${P.busy ? ' disabled' : ''}>${P.busy ? 'Sending…' : 'Send'}</button>
        ${P.busy ? '<button class="btn ghost" type="button" data-act="stop-play">Stop</button>' : ''}
        <span class="meta" id="play-meta" role="status">${esc(P.meta || 'Shows on the Usage page as the client dashboard.')}</span></div>
    </form>
    <pre class="play-out${P.error ? ' err' : ''}" id="play-out"${P.out || P.error ? '' : ' hidden'}>${esc(P.error || P.out)}</pre></section>`;
}

async function runPlay(form) {
  const P = S.play;
  if (!P || P.busy) return;
  P.model = form.querySelector('#play-model').value.trim();
  P.prompt = form.querySelector('#play-prompt').value;
  if (!P.model || !P.prompt.trim()) return;
  Object.assign(P, { busy: true, out: '', error: '', meta: '', abort: new AbortController() });
  patch('models-play', playHTML);
  const started = performance.now();
  let first = null;
  let usage = null;
  let account = null;
  try {
    const headers = { 'content-type': 'application/json' };
    if (S.key) headers.authorization = `Bearer ${S.key}`;
    const res = await fetch('/api/playground', { method: 'POST', headers, body: JSON.stringify({ model: P.model, prompt: P.prompt }), signal: P.abort.signal });
    account = res.headers.get('x-fusebox-account');
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new Error(data.error?.message || data.error || res.statusText);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      let cut;
      while ((cut = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, cut);
        buf = buf.slice(cut + 2);
        for (const line of block.split('\n')) {
          if (!line.startsWith('data:')) continue;
          const data = line.slice(5).trim();
          if (!data || data === '[DONE]') continue;
          let v;
          try { v = JSON.parse(data); } catch { continue; }
          if (v.error) throw new Error(v.error.message || 'The request failed');
          const delta = v.choices?.[0]?.delta || {};
          if ((delta.content || delta.reasoning_content) && first == null) first = performance.now() - started;
          if (delta.content) {
            P.out += delta.content;
            const out = $('#play-out');
            if (out) { out.hidden = false; out.textContent = P.out; }
          }
          if (v.usage) usage = v.usage;
        }
      }
    }
    P.meta = [account && `Answered by ${who(account)}`, first != null && `first token ${ms(Math.round(first))}`, `total ${ms(Math.round(performance.now() - started))}`,
      usage && `${fmt(usage.prompt_tokens || 0)} in · ${fmt(usage.completion_tokens || 0)} out`].filter(Boolean).join(' · ');
  } catch (e) {
    if (e.name === 'AbortError') P.meta = 'Stopped.';
    else P.error = e.message;
  }
  Object.assign(P, { busy: false, abort: null });
  if (S.play === P) patch('models-play', playHTML);
}

document.addEventListener('submit', (e) => {
  if (e.target.id !== 'play-form') return;
  e.preventDefault();
  runPlay(e.target);
});
document.addEventListener('keydown', (e) => {
  if (e.target.id === 'play-prompt' && e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); runPlay(e.target.form); }
});

const MODEL_EXAMPLE = {
  claude: 'claude-opus-6', codex: 'gpt-7', gemini: 'gemini-4-pro', vertex: 'gemini-4-pro', antigravity: 'gemini-4-pro',
  kimi: 'kimi-k4', xai: 'grok-5', meta: 'muse-spark-2', devin: 'swe-2',
};

// Providers a model can be added for: the connected ones (compatible providers list their own).
function addModelProviders() {
  const have = new Set((S.accounts || []).map((a) => a.provider));
  return Object.keys(MODEL_EXAMPLE).filter((p) => have.has(p));
}

function guessProvider(id) {
  const f = FAMILIES.find(([, , , re]) => re.test(id.trim().toLowerCase()));
  return f && addModelProviders().includes(f[2]) ? f[2] : null;
}

function modelsPanelHTML() {
  return `${removedNoteHTML()}${addModelHTML()}`;
}

// Models someone added that Fusebox has since learned, and so took off the list.
function removedNoteHTML() {
  const seen = Date.parse(store.get('removedSeen') || '') || 0;
  const list = (S.routes?.removed_models || []).filter((r) => Date.parse(r.at) > seen && Date.now() - Date.parse(r.at) < 7 * 864e5);
  if (!list.length) return '';
  const names = list.map((r) => r.model).join(', ');
  return `<div class="card fam-note models-note"><span>Fusebox now knows ${esc(names)}, so ${list.length === 1 ? 'it was' : 'they were'} removed from the models you added.</span><span class="grow"></span><button class="btn sm" type="button" data-act="dismiss-removed">Dismiss</button></div>`;
}

function addModelHTML() {
  const m = S.addModel;
  if (!m) return mob() ? '<button class="btn" type="button" data-act="add-model">Add model</button>' : '';
  const provs = addModelProviders();
  const p = provs.includes(m.provider) ? m.provider : provs[0];
  const lists = (S.accounts || []).some((a) => ['claude', 'codex'].includes(a.provider) && a.kind !== 'api-key');
  return `<form class="panel models-add" data-form="add-model" aria-label="Add a model">
    <h3>Add a model</h3>
    <p>For a model released after this version of Fusebox. It is listed and served right away, and taken off this list once Fusebox or the provider's own model list knows it.</p>
    ${provs.length ? `<div class="seg" role="group" aria-label="Provider">${provs.map((id) => `<button type="button" data-act="add-model-provider" data-id="${id}" aria-pressed="${p === id}">${logo(id, null, null, 14)}${esc(PROVIDER[id])}</button>`).join('')}</div>` : '<p class="msg err">Connect an account first.</p>'}
    <div class="grid"><label class="field wide"><span>Model id</span><input type="text" name="model" value="${esc(m.model || '')}" placeholder="${esc(MODEL_EXAMPLE[p] || '')}" autocomplete="off" spellcheck="false" required><small>Exactly as the provider writes it; clients ask for this name.</small></label></div>
    <div class="actions"><button class="btn primary" type="submit"${provs.length ? '' : ' disabled'}>Add model</button><button class="btn ghost" type="button" data-act="close-add-model">Cancel</button>
      ${lists ? '<span class="grow"></span><button class="btn" type="button" data-act="check-models">Check Claude and Codex for new models</button>' : ''}</div>
    <p class="msg${m.err ? ' err' : ''}" id="add-model-msg" aria-live="polite">${esc(m.msg || '')}</p>
  </form>`;
}

function patchModelsPanel() {
  patch('models-panel', modelsPanelHTML);
  $('#add-model-btn')?.setAttribute('aria-expanded', String(!!S.addModel));
}

function openAddModel(provider) {
  S.addModel = { provider: provider || null, picked: !!provider, model: S.addModel?.model || '' };
  patchModelsPanel();
  const input = $('#models-panel input[name="model"]');
  input?.focus();
  input?.scrollIntoView({ block: 'nearest' });
}

async function submitAddModel(form) {
  const m = S.addModel;
  const provider = addModelProviders().includes(m.provider) ? m.provider : addModelProviders()[0];
  const model = form.elements.model.value.trim();
  const btn = form.querySelector('button[type="submit"]');
  btn.disabled = true;
  try {
    await api('/models/added', { method: 'POST', body: JSON.stringify({ provider, model }) });
    S.addModel = null;
    S.modelFilter = '';
    toast(`Added ${model}`);
    patchModelsPanel();
    refreshAccounts();
  } catch (e) {
    Object.assign(m, { model, msg: e.message, err: true });
    patchModelsPanel();
  }
}

async function checkModels(btn) {
  btn.disabled = true;
  btn.textContent = 'Checking…';
  const m = S.addModel;
  try {
    const r = await api('/models/check', { method: 'POST' });
    let msg = r.new.length ? `New: ${r.new.join(', ')}.`
      : r.asked ? `Asked ${plural(r.asked, 'account')}; nothing new.` : 'Every account was checked in the last 10 minutes.';
    if (r.in_progress) {
      const pending = 'Some accounts are still being checked; results will appear automatically.';
      msg = r.asked || r.new.length ? `${msg} ${pending}` : pending;
    }
    if (m) Object.assign(m, { msg, err: false, model: $('#models-panel input[name="model"]')?.value || m.model });
    refreshAccounts();
  } catch (e) {
    if (m) Object.assign(m, { msg: e.message, err: true });
  }
  patchModelsPanel();
}

async function removeModel(id, provider) {
  try {
    await api('/models/added', { method: 'DELETE', body: JSON.stringify({ provider, model: id }) });
    toast(`Removed ${id}`);
    refreshAccounts();
  } catch (e) {
    toast(e.message);
  }
}

function modelsCountText() {
  if (!S.routes) return '';
  const accounts = new Set(Object.values(S.routes?.models || {}).flatMap((steps) => steps.map((s) => s.account)));
  return `${plural(S.models.length, 'model id')} from ${plural(accounts.size, 'account')}. Clients ask for a model by name; Fusebox picks the account. Click an id to copy it.`;
}

function modelsHeadHTML() {
  const r = S.routes || S.overview;
  const n = r.request_retry || 1;
  if (mob()) {
    const short = { 'least-used': 'Most quota left', 'smart-quota': 'Smart balancing', 'round-robin': 'Round robin', 'fill-first': 'Fill first' }[r.routing] || '';
    return `<a class="card routing-link" href="#/config/routing"><span class="label">Routing</span><span class="grow">${esc(short)}${r.session_affinity ? ' · sessions stay put' : ''}</span><span class="dim" aria-hidden="true">›</span></a>`;
  }
  return `<div class="card routing-strip"><span class="label">Routing</span><span>${esc(ROUTING_LABEL[r.routing] || r.routing)}</span>
    ${r.session_affinity ? '<span class="sep" aria-hidden="true"></span><span class="fg2">Sessions stay on one account</span>' : ''}
    <span class="sep" aria-hidden="true"></span><span class="fg2">${n === 1 ? '1 account per request' : `Up to ${n} accounts per request`}</span>
    <span class="grow"></span><a class="link" href="#/config/routing">Change in Config →</a></div>`;
}

function modelsBodyHTML() {
  const count = $('#models-count');
  if (count) count.textContent = modelsCountText();
  // Route order arrives on its own the first time the page opens (loadRoutes).
  if (!S.routes) return skeletonHTML();
  const q = S.modelFilter.trim().toLowerCase();
  const fails = failuresByModel();
  modelFails = failureMarks(fails);
  const fams = modelFamilies().map((f) => famView(f, fails, q)).filter((f) => f.chips.length);
  if (!fams.length) {
    const empty = q ? `No model matches “${esc(S.modelFilter)}”.` : 'No models yet. Connect an account to see what it serves.';
    return mob() ? `<p class="empty-line">${empty}</p>` : `<div class="card rivets table"><div class="empty" style="border-top:0">${empty}</div></div>`;
  }
  if (mob()) return fams.map((f) => famHTML(f, q)).join('');
  return `<div class="card rivets table fams" role="table" aria-label="Model families">
    <div class="th" role="row"><span>Family</span><span>Model ids</span><span class="c-route-h">Route order · ${quotaWord()}</span></div>
    ${fams.map((f) => famHTML(f, q)).join('')}</div>`;
}

// One family with its route, the ids that match the filter and what is wrong with it.
function famView(f, fails, q) {
  const route = famRoute(f);
  // A failed request doesn't take an account out of rotation; an expired sign-in does.
  const usable = route.rows.some((r) => r.state === 'ready' && !acctState(accountById(r.account)).signin);
  const chips = f.ids.map((id) => {
    const { prefix, added, provider } = modelInfo(id);
    const base = unprefixed(id);
    const target = routeSteps(id).map((s) => s.upstream).find((u) => u && u.toLowerCase() !== base.toLowerCase() && !(f.key === 'kimi' && !f.compat));
    const only = prefix ? routeSteps(id).map((s) => accountById(s.account)).filter(Boolean) : [];
    const bad = (fails[id] || []).length >= 3;
    return { id, prefix, added, provider, base, only, target, bad, match: !q || `${id} ${target || ''}`.toLowerCase().includes(q) };
  }).filter((c) => c.match);
  let note = null;
  const worst = Object.entries(fails).filter(([m, l]) => f.ids.includes(m) && l.length >= 3).sort((x, y) => y[1].length - x[1].length)[0];
  if (worst) {
    const [m, l] = worst;
    const e = l[0].error ? `, ${String(hideEmails(l[0].error)).replace(/^\d{3}[^:]*:\s*/, '').slice(0, 90).replace(/[.\s]+$/, '')}` : '';
    note = { cls: 'err', text: `${m} failed ${l.length} times in the last hour: ${l[0].status}${e}.`, act: 'View requests', data: `data-act="view-model" data-id="${esc(m)}"` };
  } else if (!usable && route.rows.length) {
    const signin = route.rows.map((r) => accountById(r.account)).find((a) => acctState(a).signin);
    const cooling = route.rows.filter((r) => r.state === 'cooling' && r.until).sort((x, y) => Date.parse(x.until) - Date.parse(y.until))[0];
    if (signin) note = { cls: 'err', text: 'Unavailable until the account signs in again.', act: 'Sign in', data: `data-act="signin-again" data-id="${esc(signin.id)}"` };
    else if (cooling) note = { cls: 'warn', text: `Every account for these models is cooling down. Back at ${when(cooling.until)}.` };
    else note = { cls: '', text: 'Every account that serves these models is turned off.' };
  }
  return { ...f, route, chips, note, off: !usable };
}

function famHTML(f, q) {
  const onlyText = (c) => (c.only.length === 1 ? acctLabel(c.only[0]) : plural(c.only.length, 'account'));
  const chipTitle = (c) => [
    c.prefix ? `${c.id} only goes to ${c.only.length ? c.only.map(acctLabel).join(', ') : `accounts with the ${c.prefix}/ prefix`}` : '',
    c.target ? `${c.prefix ? c.base : c.id} is an alias for ${c.target}` : '',
    modelCapabilitiesText(modelInfo(c.id)),
    c.bad ? `${c.id} is failing` : '',
    c.added ? 'You added this model; it goes once Fusebox knows it' : '',
  ].filter(Boolean).join('. ') || `Copy ${c.id}`;
  const idChipHTML = (c) => `<button class="idchip${c.target ? ' alias' : ''}${c.added ? ' added' : ''}${c.bad ? ' bad' : ''}${f.off ? ' off' : ''}${q ? ' hit' : ''}" type="button" data-act="copy-model" data-id="${esc(c.id)}" title="${esc(chipTitle(c))}">${c.prefix ? `<span><span class="pfx">${esc(c.prefix)}/</span>${esc(c.base)}</span>` : esc(c.id)}${c.target ? `<span class="to">→ ${esc(c.target)}</span>` : ''}${c.prefix && c.only.length ? `<span class="to">only ${esc(onlyText(c))}</span>` : ''}${c.added ? '<span class="tag">added</span>' : ''}${c.bad ? '<span class="dot s6 err"></span>' : ''}</button>`;
  const chipHTML = (c) => (c.added ? `<span class="idchip-pair">${idChipHTML(c)}<button class="idchip-x" type="button" data-act="remove-model" data-id="${esc(c.id)}" data-provider="${esc(c.provider)}" aria-label="Remove ${esc(c.id)}" title="Remove ${esc(c.id)}">×</button></span>` : idChipHTML(c));
  // Families of a provider that takes added models end with a "+ Add" chip.
  const addChip = !q && !f.compat && addModelProviders().includes(f.logoKey[0]) ? `<button class="idchip add" type="button" data-act="add-model" data-provider="${f.logoKey[0]}" title="Add a ${esc(f.name)} model Fusebox doesn't list yet">+ Add</button>` : '';
  const stepHTML = (r, i, short) => {
    const a = accountById(r.account);
    const t = routeStepText(r, a);
    const sub = `${provName(a)}${planName(a) ? ` ${planName(a)}` : ''}${r.serves < f.route.models ? ` · ${r.serves} of ${f.route.models} models` : ''}`;
    return `<button class="rstep" type="button" data-act="open-acc" data-id="${esc(a.id)}"><span class="idx">${i + 1}</span><span class="dot s7 ${acctState(a).dot}"></span>${acctLogo(a, 15)}
      <span class="nm"><span${a.disabled ? ' class="off"' : ''}>${esc(acctLabel(a))}</span>${short ? '' : `<span>${esc(sub)}</span>`}</span>
      <span class="txt ${t.cls}">${r.next ? '<span class="next-tag">Next</span>' : ''}${esc(short ? t.short : t.txt)}</span></button>`;
  };
  const countLabel = q ? `${f.chips.length} of ${f.ids.length} ids` : plural(f.ids.length, 'id');
  const note = f.note ? `<div class="fam-note ${f.note.cls}"><span>${esc(f.note.text)}</span>${f.note.act ? `<button class="btn sm" type="button" ${f.note.data}>${esc(f.note.act)}</button>` : ''}</div>` : '';
  const chips = `<div class="chipset" style="gap:6px">${f.chips.map(chipHTML).join('')}${addChip}</div>${note}`;
  if (mob()) {
    return `<section class="card m-fam" data-fam="${esc(f.key)}" aria-label="${esc(f.name)}">
      <div class="m-fam-head">${logo(f.logoKey[0], f.logoKey[1], null, 18)}<span class="nm">${esc(f.name)}</span><span class="pat">${esc(famPattern(f))}</span><span class="grow"></span><span class="meta" style="font-size:12px">${countLabel}</span></div>
      ${f.route.rows.length ? `<div class="m-route">${f.route.rows.map((r, i) => stepHTML(r, i, true)).join('')}</div>` : ''}
      ${chips}</section>`;
  }
  return `<div class="fam" role="row" data-fam="${esc(f.key)}">
      <div class="fam-name">${logo(f.logoKey[0], f.logoKey[1])}<div class="cell2"><span>${esc(f.name)}</span><span class="pat">${esc(famPattern(f))}</span><span class="meta" style="font-size:12px">${countLabel}</span></div></div>
      <div class="fam-ids">${chips}</div>
      <div class="route-col"><span class="capsm">Route order · ${quotaWord()}</span>${f.route.rows.map((r, i) => stepHTML(r, i, false)).join('') || '<span class="dim" style="font-size:12px">No account serves these right now.</span>'}</div>
    </div>`;
}

// What the model list shows of the last hour's failures: each model failing three or
// more times, with how often and its latest error.
let modelFails = new Map();
function failureMarks(fails) {
  const marks = new Map();
  for (const [m, l] of Object.entries(fails)) if (l.length >= 3) marks.set(m, `${l.length}\u0000${l[0].status}\u0000${l[0].error || ''}`);
  return marks;
}

// A live request reaches the model list only through those failures: when they change,
// only the families of the models concerned are redrawn.
function patchModelFailures() {
  const root = $('#models-root');
  if (!root || !S.routes) return;
  const fails = failuresByModel();
  const marks = failureMarks(fails);
  const changed = new Set([...marks.keys(), ...modelFails.keys()].filter((m) => marks.get(m) !== modelFails.get(m)));
  if (!changed.size) return;
  const q = S.modelFilter.trim().toLowerCase();
  for (const f of modelFamilies()) {
    if (!f.ids.some((id) => changed.has(id))) continue;
    const v = famView(f, fails, q);
    if (!v.chips.length) continue;
    const el = root.querySelector(`[data-fam="${CSS.escape(f.key)}"]`);
    if (!el) return patch('models-root', modelsBodyHTML);
    replaceHTML(el, famHTML(v, q));
  }
  modelFails = marks;
}

// ---------------------------------------------------------------- notifications

// Push notifications for this device (Config, Notifications). The server decides what
// to send; the dashboard only subscribes the browser and lists the devices.
const IOS = /iPhone|iPad|iPod/.test(navigator.userAgent) || (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1);
const pushReady = () => window.isSecureContext && 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;

// "Chrome on Android", "Fusebox app on iPhone": how the device is listed.
function deviceLabel() {
  const ua = navigator.userAgent;
  const os = /iPhone|iPod/.test(ua) ? 'iPhone' : IOS ? 'iPad' : /Android/.test(ua) ? 'Android' : /CrOS/.test(ua) ? 'ChromeOS'
    : /Mac OS X/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  const browser = /Edg(A|iOS)?\//.test(ua) ? 'Edge' : /Firefox\/|FxiOS/.test(ua) ? 'Firefox' : /Chrome\/|CriOS/.test(ua) ? 'Chrome'
    : IOS || /Safari\//.test(ua) ? 'Safari' : 'Browser';
  const name = STANDALONE ? 'Fusebox app' : browser;
  return os ? `${name} on ${os}` : name;
}

function pushKey(text) {
  const b64 = text.replace(/-/g, '+').replace(/_/g, '/');
  return Uint8Array.from(atob(b64 + '='.repeat((4 - (b64.length % 4)) % 4)), (c) => c.charCodeAt(0));
}

const sameBytes = (a, b) => !!a && a.byteLength === b.byteLength && new Uint8Array(a).every((x, i) => x === b[i]);
const pushMine = () => (S.push.status?.subscriptions || []).find((d) => d.endpoint === S.push.endpoint);

async function loadPush() {
  const p = S.push;
  p.loading = true;
  try {
    p.status = await api('/push');
    if (pushReady()) {
      const reg = await navigator.serviceWorker.getRegistration('/');
      p.endpoint = (await reg?.pushManager.getSubscription())?.endpoint || null;
    }
  } catch (e) {
    p.msg = e.message;
  }
  p.loading = false;
  p.loaded = true;
  patch('push-device', pushDeviceInner);
}

function pushDeviceHTML() {
  if (!S.push.loaded && !S.push.loading) loadPush();
  return `<div class="cfg-list wide" id="push-device">${pushDeviceInner()}</div>`;
}

function pushDeviceInner() {
  const p = S.push;
  const mine = pushMine();
  const button = (act, label) => `<button type="button" class="btn" data-act="${act}" ${p.busy ? 'disabled' : ''}>${label}</button>`;
  let text;
  let actions = '';
  if (!window.isSecureContext) {
    text = 'Notifications need HTTPS. Open the dashboard over HTTPS, for example through tailscale serve, or on this machine at localhost.';
  } else if (!pushReady()) {
    text = IOS && !STANDALONE
      ? 'On iPhone and iPad, add Fusebox to your Home Screen first: tap Share, then Add to Home Screen, and open it from there.'
      : 'This browser can\u2019t receive push notifications.';
  } else if (Notification.permission === 'denied') {
    text = 'Notifications are blocked for this site. Allow them in the browser\u2019s site settings, then reload.';
  } else if (!p.loaded) {
    text = 'Checking\u2026';
  } else if (mine) {
    text = `On. ${mine.label} gets the events below, even when Fusebox isn\u2019t open.`;
    actions = button('push-test', 'Send test') + button('push-off', 'Turn off');
  } else {
    text = 'Off. Turn it on to get the events below on this device, even when Fusebox isn\u2019t open.';
    actions = button('push-on', 'Turn on');
  }
  const day = (iso) => new Date(iso).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
  const others = (p.status?.subscriptions || []).filter((d) => d !== mine);
  return `<div class="cfg-list-head"><div class="cell2"><h3>This device</h3><p class="cfg-description">${esc(text)}</p></div>${actions ? `<div class="push-actions">${actions}</div>` : ''}</div>
    ${p.msg ? `<p class="msg err" role="alert">${esc(p.msg)}</p>` : ''}
    ${others.length ? `<div class="push-devices"><span class="label">Other devices</span>${others.map((d) => `<div class="push-device">
      <span class="dot s6 ok" aria-hidden="true"></span><span class="grow"><span>${esc(d.label)}</span><span class="meta">${esc(d.origin.replace(/^https?:\/\//, ''))} \u00b7 added ${esc(day(d.created_at))}</span></span>
      <button type="button" class="btn sm" data-act="push-remove" data-id="${esc(d.id)}" ${p.busy ? 'disabled' : ''} aria-label="Remove ${esc(d.label)}">Remove</button></div>`).join('')}</div>` : ''}`;
}

async function pushOn() {
  // Ask first, while the click still counts as the user's gesture.
  const permission = await Notification.requestPermission();
  if (permission !== 'granted') throw new Error(permission === 'denied' ? 'Notifications are blocked for this site.' : 'Notifications were not allowed.');
  await navigator.serviceWorker.register('/sw.js');
  const reg = await navigator.serviceWorker.ready;
  const key = pushKey(S.push.status.public_key);
  let sub = await reg.pushManager.getSubscription();
  // A subscription made for another server key can't be reused.
  if (sub && !sameBytes(sub.options.applicationServerKey, key)) { await sub.unsubscribe(); sub = null; }
  sub ||= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  const { endpoint, keys } = sub.toJSON();
  await api('/push/subscriptions', { method: 'POST', body: JSON.stringify({ endpoint, keys, origin: location.origin, label: deviceLabel() }) });
  S.push.endpoint = endpoint;
  toast('Notifications are on');
}

async function pushOff() {
  const mine = pushMine();
  if (mine) await api(`/push/subscriptions/${encodeURIComponent(mine.id)}`, { method: 'DELETE' });
  const reg = await navigator.serviceWorker.getRegistration('/');
  await (await reg?.pushManager.getSubscription())?.unsubscribe();
  S.push.endpoint = null;
  toast('Notifications are off');
}

async function pushAction(act, id) {
  const p = S.push;
  if (p.busy) return;
  p.busy = true;
  p.msg = null;
  patch('push-device', pushDeviceInner);
  try {
    if (act === 'push-on') await pushOn();
    else if (act === 'push-off') await pushOff();
    else if (act === 'push-test') {
      await api('/push/test', { method: 'POST', body: JSON.stringify({ id: pushMine()?.id }) });
      toast('Test notification sent');
    } else if (act === 'push-remove') await api(`/push/subscriptions/${encodeURIComponent(id)}`, { method: 'DELETE' });
  } catch (e) {
    p.msg = e.message;
  }
  p.busy = false;
  await loadPush();
}

// ---------------------------------------------------------------- lock

function lockHTML() {
  if (S.locked === 'remote') {
    return `<div class="lock"><h1>Dashboard is local-only</h1>
      <p>Without a management key the dashboard only answers on localhost, and not through a proxy such as tailscale serve, nginx or Caddy. Set <span class="mono">management-key</span> in config.yaml on the server, then reload this page.</p></div>`;
  }
  return `<div class="lock"><h1>Dashboard locked</h1>
    <p>Enter the <span class="mono">management-key</span> from config.yaml.</p>
    <form data-form="unlock"><label class="sr-only" for="mk">Management key</label>
      <input id="mk" type="password" name="key" autocomplete="current-password" required autofocus>
      <button class="btn primary" type="submit">Unlock</button>
      <p class="msg err" id="lock-msg" aria-live="polite"></p></form></div>`;
}

function bindLock() {
  $('#mk')?.focus();
}

// ---------------------------------------------------------------- actions

async function startLogin(provider) {
  S.panel = provider;
  if (provider === 'vertex') {
    S.login = null;
    if (S.route !== 'accounts' || S.sub) location.hash = '#/accounts';
    else { patch('acct-panel', panelHTML); patch('acct-head', accountHeadHTML); }
    $('textarea[name="json"]')?.focus();
    return;
  }
  S.login = { provider, status: 'starting' };
  if (S.route !== 'accounts' || S.sub) location.hash = '#/accounts';
  else { patch('acct-panel', panelHTML); patch('acct-head', accountHeadHTML); }
  // Open the tab synchronously so popup blockers allow it.
  const tab = window.open('about:blank', '_blank');
  try {
    const r = await api(`/login/${provider}`, { method: 'POST' });
    S.login = { provider, state: r.state, url: r.url, callback: r.callback, kind: r.kind, user_code: r.user_code, status: 'pending' };
    if (tab) { tab.opener = null; tab.location.href = r.url; }
  } catch (e) {
    tab?.close();
    S.login = { provider, status: 'error', message: e.message };
  }
  patch('acct-panel', panelHTML);
  schedulePoll();
}

let pollTimer = 0;
function schedulePoll() {
  clearTimeout(pollTimer);
  if (S.login && S.login.status === 'pending') pollTimer = setTimeout(pollLogin, 1500);
}

async function pollLogin() {
  const L = S.login;
  if (!L || !L.state || L.status !== 'pending') return;
  try {
    const r = await api(`/login/${encodeURIComponent(L.state)}`);
    if (r.status !== L.status) {
      Object.assign(L, { status: r.status, message: r.message });
      const input = $('#paste-url');
      const keep = input ? input.value : '';
      patch('acct-panel', panelHTML);
      if ($('#paste-url') && keep) $('#paste-url').value = keep;
      if (r.status === 'done') refreshAccounts();
    }
  } catch {}
  schedulePoll();
}

async function submitPaste(form) {
  const L = S.login;
  const input = form.elements.input.value.trim();
  if (!input || !L) return;
  const btn = form.querySelector('button[type="submit"]');
  btn.disabled = true;
  btn.textContent = 'Connecting…';
  try {
    const r = await api(`/login/${encodeURIComponent(L.state)}/code`, { method: 'POST', body: JSON.stringify({ input }) });
    Object.assign(L, { status: 'done', message: r.label });
    refreshAccounts();
  } catch (e) {
    L.error = e.message;
  }
  patch('acct-panel', panelHTML);
  schedulePoll();
}

async function submitVertex(form) {
  const data = Object.fromEntries(new FormData(form).entries());
  const msg = $('#vertex-msg');
  const btn = form.querySelector('button[type="submit"]');
  btn.disabled = true;
  btn.textContent = 'Checking…';
  try {
    const r = await api('/vertex', { method: 'POST', body: JSON.stringify(data) });
    S.panel = 'vertex-done';
    S.login = { provider: 'vertex', status: 'done', message: r.label };
    patch('acct-panel', panelHTML);
    patch('acct-head', accountHeadHTML);
    refreshAccounts();
  } catch (e) {
    msg.className = 'msg err';
    msg.textContent = e.message;
    btn.disabled = false;
    btn.textContent = 'Add service account';
  }
}

async function submitKey(form) {
  const data = Object.fromEntries(new FormData(form).entries());
  data.provider = S.keyProvider;
  const msg = $('#key-msg');
  const btn = form.querySelector('button[type="submit"]');
  btn.disabled = true;
  try {
    await api('/keys', { method: 'POST', body: JSON.stringify(data) });
    S.panel = null;
    patch('acct-panel', panelHTML);
    patch('acct-head', accountHeadHTML);
    refreshAccounts();
  } catch (e) {
    msg.className = 'msg err';
    msg.textContent = e.message;
    btn.disabled = false;
  }
}

async function clearAccountError(id) {
  const a = accountById(id);
  if (!a?.last_error_id || S.errorClears[id]?.busy) return;
  const errorId = a.last_error_id;
  const local = S.errorClears[id] = { busy: true, error: null };
  refreshViews();
  try {
    const updated = await api(`/accounts/${encodeURIComponent(id)}/error/clear`, { method: 'POST', body: JSON.stringify({ error_id: errorId }) });
    const current = accountById(id);
    if (current?.last_error_id === errorId) {
      for (const field of ['last_error', 'last_error_id', 'last_error_until']) current[field] = updated[field];
    }
    delete S.errorClears[id];
    toast('Error cleared');
  } catch (e) {
    local.error = e.status === 409 ? e.message : `Could not clear error: ${e.message}`;
  } finally {
    local.busy = false;
    refreshViews();
    refreshAccounts();
  }
}

async function accountAction(act, id) {
  const a = accountById(id);
  try {
    if (act === 'toggle') {
      a.disabled = !a.disabled;
      refreshViews();
      await api(`/accounts/${encodeURIComponent(id)}/toggle`, { method: 'POST', body: JSON.stringify({ disabled: a.disabled }) });
    } else if (act === 'refresh') {
      for (const btn of $$(`[data-act="refresh"][data-id="${CSS.escape(id)}"]`)) { btn.disabled = true; btn.style.opacity = 1; btn.firstElementChild && (btn.firstElementChild.style.animation = 'pulse 1s infinite'); }
      await api(`/accounts/${encodeURIComponent(id)}/refresh`, { method: 'POST' });
      toast(`Refreshed ${a ? acctLabel(a) : 'account'}`);
    } else if (act === 'reset') {
      await api(`/accounts/${encodeURIComponent(id)}/reset`, { method: 'POST' });
    } else if (act === 'delete') {
      S.confirm = null;
      await api(`/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' });
      if (S.drawer === id) closeDrawer();
      if (S.route === 'accounts' && S.sub === id) location.hash = '#/accounts';
    }
  } catch (e) {
    if (a) a.last_error = e.message;
  }
  refreshAccounts();
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = text;
    document.body.append(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
}

async function copy(btn) {
  await copyText(btn.dataset.text);
  if (btn.dataset.toast) toast(btn.dataset.toast);
  if (btn.classList.contains('linkbtn')) {
    const prev = btn.textContent;
    btn.textContent = 'Copied';
    setTimeout(() => { btn.textContent = prev; }, 1400);
    return;
  }
  const prev = btn.innerHTML;
  btn.innerHTML = ICON.check;
  btn.classList.add('copied');
  setTimeout(() => { btn.innerHTML = prev; btn.classList.remove('copied'); }, 1400);
}

function togglePrivacy() {
  S.private = !S.private;
  store.set('private', S.private ? '1' : '0');
  const paste = $('#paste-url')?.value;
  const focused = document.activeElement?.hasAttribute('data-privacy');
  render();
  if (S.palette) renderPalette();
  if (paste && $('#paste-url')) $('#paste-url').value = paste;
  if (focused) $$('[data-privacy]').find((b) => b.offsetParent)?.focus();
}

// Clicking anywhere on a row opens its account, unless the click was on a control.
document.addEventListener('click', (e) => {
  if (e.target.closest('[data-act], a, button, input, select, textarea, label, summary')) return;
  const row = e.target.closest('[data-open]');
  if (row) openAccount(row.dataset.open);
});

document.addEventListener('click', (e) => {
  const el = e.target.closest('[data-act]');
  if (!el) return;
  const { act, id } = el.dataset;
  switch (act) {
    case 'refresh-page': return refreshPage();
    case 'confirm-page-refresh': $('#page-refresh-modal').close(); return refreshPage(true);
    case 'cancel-page-refresh': return $('#page-refresh-modal').close();
    case 'copy': return copy(el);
    case 'copy-model': return copyText(id).then(() => toast(`Copied ${id}`));
    case 'add-model': return openAddModel(el.dataset.provider);
    case 'close-add-model': S.addModel = null; return patchModelsPanel();
    case 'add-model-provider':
      Object.assign(S.addModel, { provider: id, picked: true, model: $('#models-panel input[name="model"]')?.value || '' });
      patchModelsPanel();
      return $('#models-panel input[name="model"]')?.focus();
    case 'check-models': return checkModels(el);
    case 'open-play':
      if (S.play) { S.play.abort?.abort(); S.play = null; patch('models-play', playHTML); return; }
      S.play = { model: S.models.find((m) => m.id.startsWith('claude-'))?.id || S.models[0]?.id || '', prompt: 'Say hello in one sentence.', out: '', error: '', meta: '', busy: false, abort: null };
      patch('models-play', playHTML);
      for (const b of $$('[data-act="open-play"]')) b.setAttribute('aria-expanded', 'true');
      return $('#play-prompt')?.focus();
    case 'close-play':
      S.play?.abort?.abort();
      S.play = null;
      patch('models-play', playHTML);
      for (const b of $$('[data-act="open-play"]')) b.setAttribute('aria-expanded', 'false');
      return $('[data-act="open-play"]')?.focus();
    case 'stop-play': S.play?.abort?.abort(); return;
    case 'remove-model': return removeModel(id, el.dataset.provider);
    case 'dismiss-removed':
      store.set('removedSeen', (S.routes?.removed_models || []).map((r) => r.at).sort().pop() || new Date().toISOString());
      return patchModelsPanel();
    case 'privacy': return togglePrivacy();
    case 'push-on': case 'push-off': case 'push-test': case 'push-remove': return pushAction(act, id);
    case 'palette': return openPalette();
    case 'close-palette': return closePalette();
    case 'faults': return toggleFaults(el);
    case 'close-faults': return closeFaults();
    case 'alert-open': return runAlert(id);
    case 'open-acc': return openAccount(id);
    case 'close-drawer': return closeDrawer();
    case 'to-list': location.hash = '#/accounts'; return;
    case 'reveal-config':
      S.config.reveal = true;
      render();
      return $('#cfg-yaml')?.focus();
    case 'quota-display':
      setQuotaDisplay(id);
      return;
    case 'acct-filter':
      S.acctFilter = id;
      patch('acct-filters', acctFiltersHTML);
      patch('acct-timeline', resetTimelineHTML);
      return patch('acct-list', accountListHTML);
    case 'priority': return setPriority(id, Number(el.dataset.value));
    case 'filter-session':
      e.stopPropagation();
      S.reqSess = id;
      S.reqChip = 'all';
      S.openReq = null;
      closeDrawer();
      if (S.route !== 'requests') { location.hash = `#/requests?sess=${encodeURIComponent(id)}`; return; }
      syncRequestsHash();
      return refreshRequests();
    case 'clear-pill':
      if (id === 'acc') S.reqAcc = null; else S.reqSess = null;
      syncRequestsHash();
      refreshRequests();
      return $('#req-filter')?.focus();
    case 'req-chip':
      S.reqChip = id;
      S.openReq = null;
      return refreshRequests();
    case 'toggle-req': {
      if (e.target.closest('.why, .m-why') && !e.target.closest('.tr, .l1')) return;
      const rid = Number(id);
      S.openReq = S.openReq === rid ? null : rid;
      const r = S.requests.find((x) => x.id === rid);
      const item = $(`#req-body [data-req="${rid}"]`);
      for (const other of $$('#req-body .open')) {
        const oid = Number(other.dataset.req);
        const or = S.requests.find((x) => x.id === oid);
        if (or && oid !== rid) other.outerHTML = mob() ? reqMobileHTML(or) : reqItemHTML(or);
      }
      if (item && r) item.outerHTML = mob() ? reqMobileHTML(r) : reqItemHTML(r);
      return $(`#req-body [data-req="${rid}"] .req-toggle`)?.focus({ preventScroll: true });
    }
    case 'clear-filter':
      S.filter = '';
      return refreshRequests() || $('#req-filter')?.focus();
    case 'clear-model-filter':
      S.modelFilter = '';
      $('#model-filter').value = '';
      patch('models-root', modelsBodyHTML);
      return $('#model-filter')?.focus();
    case 'view-model':
      S.filter = id;
      S.reqAcc = null;
      S.reqSess = null;
      S.reqChip = 'errors';
      location.hash = '#/requests';
      return;
    case 'download-setup':
      return downloadSetup(el.dataset.text, el.dataset.filename);
    case 'snippet':
      S.snippet = id;
      store.set('snippet', id);
      return refreshSetups();
    case 'start-login':
      if (S.panel === el.dataset.provider && S.login && S.login.status === 'pending') return;
      return startLogin(el.dataset.provider);
    case 'signin-again': {
      const a = accountById(id);
      closeDrawer();
      return a && signInAgain(a);
    }
    case 'open-panel':
      return openPanel(el.dataset.panel);
    case 'close-panel':
      S.panel = null;
      S.login = null;
      clearTimeout(pollTimer);
      patch('acct-panel', panelHTML);
      return patch('acct-head', accountHeadHTML);
    case 'key-provider':
      S.keyProvider = id;
      return patch('acct-panel', panelHTML);
    case 'confirm-delete':
      S.confirm = id;
      refreshViews();
      return $('[data-act="cancel-delete"]')?.focus();
    case 'cancel-delete':
      S.confirm = null;
      return refreshViews();
    case 'banked-close': return closeResetModal();
    case 'window-start': case 'window-check':
      e.stopPropagation();
      return windowStarterAction(id, act === 'window-check');
    case 'banked-details': case 'banked-refresh': case 'banked-open': case 'banked-confirm': case 'banked-cancel': case 'banked-retry': case 'banked-resolve':
      e.stopPropagation();
      return bankedAction(act, id, el.dataset.resolution);
    case 'toggle': case 'refresh': case 'reset': case 'delete':
      e.stopPropagation();
      if (act === 'reset') toast('Cooldowns cleared');
      return accountAction(act, id);
    case 'clear-error':
      e.stopPropagation();
      return clearAccountError(id);
    case 'pause':
      S.paused = !S.paused;
      S.pending = 0;
      return refreshRequests();
    case 'save-config': return saveConfig();
    case 'revert-config': return discardConfig();
  }
});

document.addEventListener('input', (e) => {
  if (e.target.id === 'req-filter') {
    S.filter = e.target.value;
    S.openReq = null;
    const base = visibleRequests();
    patch('req-tools', reqToolsHTML, base);
    patch('req-list', reqListHTML, base);
  } else if (e.target.closest('form[data-form="add-model"]') && S.addModel) {
    // Pick the provider from the name until one is chosen by hand.
    S.addModel.model = e.target.value;
    const guess = !S.addModel.picked && guessProvider(e.target.value);
    if (guess && guess !== S.addModel.provider) {
      S.addModel.provider = guess;
      for (const b of $$('[data-act="add-model-provider"]')) b.setAttribute('aria-pressed', String(b.dataset.id === guess));
      e.target.placeholder = MODEL_EXAMPLE[guess] || '';
    }
  } else if (e.target.classList.contains('setup-model-select')) {
    S.setupModels[e.target.dataset.setupScope] = e.target.value;
    const id = e.target.id;
    refreshSetups();
    document.getElementById(id)?.focus();
  } else if (e.target.classList.contains('setup-context')) {
    const value = e.target.value === '' ? null : Number(e.target.value);
    if (value != null && (!Number.isSafeInteger(value) || value <= 0)) { e.target.setCustomValidity('Enter a positive whole number.'); e.target.reportValidity(); return; }
    if (value == null) delete S.setupContexts[e.target.dataset.model];
    else S.setupContexts[e.target.dataset.model] = value;
    const id = e.target.id;
    refreshSetups();
    document.getElementById(id)?.focus();
  } else if (e.target.classList.contains('snip-more')) {
    if (!e.target.value) return;
    const select = e.target.id;
    S.snippet = e.target.value;
    store.set('snippet', S.snippet);
    refreshSetups();
    document.getElementById(select)?.focus();
  } else if (e.target.id === 'model-filter') {
    S.modelFilter = e.target.value;
    patch('models-root', modelsBodyHTML);
  } else if (e.target.id === 'pal-q' && S.palette) {
    S.palette.q = e.target.value;
    S.palette.i = 0;
    $('#pal-list').innerHTML = paletteListHTML();
    syncPaletteActive();
  }
});

document.addEventListener('mousemove', (e) => {
  const item = e.target.closest('#pal-list [data-pal]');
  if (item && S.palette && S.palette.i !== Number(item.dataset.pal)) { S.palette.i = Number(item.dataset.pal); syncPaletteActive(); }
});
document.addEventListener('click', (e) => {
  const item = e.target.closest('#pal-list [data-pal]');
  if (item) runPalette(Number(item.dataset.pal));
});

// Keeps Tab inside an open drawer or palette.
function trapFocus(e, root) {
  const targets = $$('button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])', root).filter((el) => el.offsetParent);
  if (!targets.length) return;
  const first = targets[0], last = targets.at(-1);
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  else if (!root.contains(document.activeElement)) { e.preventDefault(); first.focus(); }
}

document.addEventListener('keydown', (e) => {
  const key = e.key;
  const modKey = e.metaKey || e.ctrlKey;
  if (modKey && key.toLowerCase() === 'k') { e.preventDefault(); if (S.palette) closePalette(); else openPalette(); return; }
  if (resetModal.open) return;
  if (key === 'Escape') {
    if (S.palette) { e.preventDefault(); closePalette(); } else if (S.faults) closeFaults(); else if (S.drawer) closeDrawer();
    return;
  }
  if (key === 'Tab') {
    if (S.palette) return trapFocus(e, $('.palette'));
    if (S.drawer) return trapFocus(e, $('#drawer'));
  }
  if (S.palette) {
    const P = S.palette;
    const n = (P.items || []).length;
    if (key === 'ArrowDown') { e.preventDefault(); P.i = Math.min(n - 1, P.i + 1); syncPaletteActive(); }
    else if (key === 'ArrowUp') { e.preventDefault(); P.i = Math.max(0, P.i - 1); syncPaletteActive(); }
    else if (key === 'Enter') { e.preventDefault(); runPalette(P.i); }
    return;
  }
  if (modKey || e.altKey) return;
  if (e.target.closest('input, textarea, select, [contenteditable="true"]')) return;
  if (S.locked || !S.overview) return;
  if (key === 'u' || key === 'U') { setQuotaDisplay(S.quotaDisplay === 'used' ? 'remaining' : 'used'); toast(S.quotaDisplay === 'used' ? 'Showing quota used' : 'Showing quota remaining'); }
  else if (key === '.') togglePrivacy();
  else if (key === '/') { e.preventDefault(); openPalette(); }
  else if (key === 'c' || key === 'C') openPanel('connect');
});

document.addEventListener('submit', async (e) => {
  const form = e.target.closest('form[data-form]');
  if (!form) return;
  e.preventDefault();
  const kind = form.dataset.form;
  if (kind === 'paste') return submitPaste(form);
  if (kind === 'key') return submitKey(form);
  if (kind === 'add-model') return submitAddModel(form);
  if (kind === 'vertex') return submitVertex(form);
  if (kind === 'unlock') {
    S.key = form.elements.key.value.trim();
    try {
      await api('/overview');
      store.set('key', S.key);
      S.locked = null;
      await boot();
    } catch {
      const m = $('#lock-msg');
      if (m) m.textContent = 'That key was not accepted.';
    }
  }
});

// ---------------------------------------------------------------- page refresh

let pageRefreshBusy = false;
let pageRefreshDiscard = false;
const refreshIndicator = $('#pull-refresh');
let refreshFrame = 0;
let refreshFeedback = null;
let refreshLabel = '';

// Keep gesture painting outside the live dashboard's render cycle.
function showRefreshFeedback(text = '', pull = 0) {
  refreshFeedback = { text, pull };
  if (refreshFrame) return;
  refreshFrame = requestAnimationFrame(() => {
    refreshFrame = 0;
    const { text, pull } = refreshFeedback;
    refreshIndicator.hidden = !text;
    refreshIndicator.classList.toggle('busy', pageRefreshBusy);
    refreshIndicator.style.setProperty('--refresh-pull', `${pull}px`);
    const label = $('[data-refresh-label]', refreshIndicator);
    if (refreshLabel !== text) { refreshLabel = text; label.textContent = text; }
  });
}

function positionRefreshFeedback() {
  const header = mob() ? $('#mbar') : $('#bar');
  refreshIndicator.style.setProperty('--refresh-top', `${header.getBoundingClientRect().bottom}px`);
}

function confirmPageRefresh() {
  showRefreshFeedback();
  $('#page-refresh-modal').showModal();
  $('[data-act="cancel-page-refresh"]', $('#page-refresh-modal')).focus();
}

async function refreshPage(discard = false) {
  if (pageRefreshBusy || $('#page-refresh-modal').open) return;
  if (S.config.busy || S.play?.busy) {
    showRefreshFeedback();
    toast('Wait for the current operation to finish before refreshing.');
    return;
  }
  if (!discard && (configDirty() || rawDirty())) return confirmPageRefresh();
  pageRefreshBusy = true;
  for (const button of $$('[data-act="refresh-page"]')) button.disabled = true;
  positionRefreshFeedback();
  showRefreshFeedback('Refreshing…', 48);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
  try {
    // Keep the current dashboard if the server is offline, rather than losing it
    // to a failed navigation. This does not fetch or change any provider data.
    const response = await fetch('/healthz', { cache: 'no-store', signal: controller.signal });
    if (!response.ok) throw new Error('unavailable');
    // Edits or a save may have started while the health check was in flight.
    if (S.config.busy || S.play?.busy) {
      toast('Wait for the current operation to finish before refreshing.');
    } else if (!discard && (configDirty() || rawDirty())) {
      confirmPageRefresh();
    } else {
      pageRefreshDiscard = discard;
      location.reload();
      return;
    }
  } catch {
    pageRefreshDiscard = false;
    toast('Can’t reach Fusebox. Try refreshing again when you’re connected.');
  } finally {
    clearTimeout(timeout);
    pageRefreshBusy = false;
    for (const button of $$('[data-act="refresh-page"]')) button.disabled = false;
    showRefreshFeedback();
  }
}

function installPullToRefresh() {
  // iPadOS may identify itself as a Mac. Standalone alone would also match Android.
  const ios = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  if (!STANDALONE || !ios) return;
  document.documentElement.classList.add('ios-pull-refresh');
  const threshold = 80;
  let gesture = null;
  const blocked = () => pageRefreshBusy || S.drawer || S.palette || S.faults || S.panel || S.addModel || S.config.review || document.querySelector('dialog[open]');
  const reset = () => { gesture = null; if (!pageRefreshBusy) showRefreshFeedback(); };
  const ignored = 'a, button, input, select, textarea, label, summary, [contenteditable]:not([contenteditable="false"]), [role="slider"], [data-no-pull-refresh], .mtabs, #layer, dialog, pre, code';
  document.addEventListener('touchstart', (event) => {
    reset();
    if (event.touches.length !== 1 || window.scrollY > 1 || blocked() || event.target.closest(ignored)) return;
    if (document.activeElement?.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])')) return;
    // Nested scrolling belongs to that control, even when it is at its own top.
    for (let el = event.target; el && el !== document.body; el = el.parentElement) {
      const style = getComputedStyle(el);
      if ((/(auto|scroll)/.test(style.overflowY) && el.scrollHeight > el.clientHeight + 1) ||
          (/(auto|scroll)/.test(style.overflowX) && el.scrollWidth > el.clientWidth + 1)) return;
    }
    const touch = event.touches[0];
    gesture = { id: touch.identifier, x: touch.clientX, y: touch.clientY, distance: 0, captured: false };
    positionRefreshFeedback();
  }, { passive: true });
  document.addEventListener('touchmove', (event) => {
    if (!gesture) return;
    if (event.touches.length !== 1 || event.touches[0].identifier !== gesture.id || blocked() || window.scrollY > 1) return reset();
    const touch = event.touches[0];
    const dx = Math.abs(touch.clientX - gesture.x);
    const dy = touch.clientY - gesture.y;
    if (!gesture.captured) {
      if (Math.max(dx, Math.abs(dy)) < 6) return;
      if (dy <= 0 || dy < dx * 1.25) return reset();
      gesture.captured = true;
    }
    if (!event.cancelable) return reset();
    event.preventDefault();
    gesture.distance = Math.max(0, dy);
    showRefreshFeedback(dy >= threshold ? 'Release to refresh' : 'Pull to refresh', Math.min(64, gesture.distance * 0.55));
  }, { passive: false });
  document.addEventListener('touchend', (event) => {
    if (!gesture) return;
    const ready = !event.touches.length && Array.from(event.changedTouches).some((touch) => touch.identifier === gesture.id) && gesture.captured && gesture.distance >= threshold && !blocked();
    reset();
    if (ready) refreshPage();
  }, { passive: true });
  document.addEventListener('touchcancel', reset, { passive: true });
  document.addEventListener('visibilitychange', reset);
  window.addEventListener('hashchange', reset);
  window.addEventListener('pagehide', reset);
  window.addEventListener('resize', reset);
}

// ---------------------------------------------------------------- routing + timers

// #/overview, #/accounts, #/accounts/{id}, #/requests?acc=…&sess=…, #/models, #/config/{section}
function parseHash() {
  const raw = location.hash.replace(/^#\/?/, '');
  const [path, query = ''] = raw.split('?');
  const [route, ...rest] = path.split('/');
  return { route: ROUTES.includes(route) ? route : 'overview', sub: rest.length ? decodeURIComponent(rest.join('/')) : null, query: new URLSearchParams(query) };
}

function onRoute() {
  closeResetModal();
  closeLayers();
  const { route, sub, query } = parseHash();
  const from = S.route;
  const prevSub = S.sub;
  S.route = route;
  S.sub = route === 'accounts' || route === 'config' ? sub : null;
  S.confirm = null;
  if (route === 'requests') {
    S.reqAcc = query.get('acc');
    S.reqSess = query.get('sess');
    if (from !== 'requests') S.openReq = null;
  }
  if (route === 'config') {
    if (S.sub && CONFIG_SECTIONS.some(([id]) => id === S.sub) && !(S.config.section === 'yaml' && S.sub !== 'yaml' && rawDirty())) S.config.section = S.sub;
  } else Object.assign(S.config, { msg: null, reveal: false, named: namedIdle() });
  // Reopening an account's page shows its activity afresh.
  if (route === 'accounts' && S.sub && (from !== 'accounts' || prevSub !== S.sub) && S.overview) loadActivity(S.sub, true);
  if (route === 'models' && S.overview && (S.routesStale || !S.routes)) loadRoutes();
  render();
  if (S.overview && needsLog()) loadRequests();
  if (route === 'accounts' && !S.sub && S.panel === 'key') $('#acct-panel input')?.focus();
  view.focus({ preventScroll: true });
  window.scrollTo(0, 0);
}

// Countdowns, "ago" times, reset deadlines and expiring quota windows, once a second in
// one sweep. Nothing runs while the page is hidden; resume() catches up.
const TIMED = '[data-until], [data-reset-deadline], [data-ago], [data-quota-reset]';
const timerText = new WeakMap();
function setTimerText(el, text) {
  if ((timerText.get(el) ?? el.textContent) === text) return;
  timerText.set(el, text);
  el.textContent = text;
}
function tickTimers() {
  const now = Date.now();
  let expired = S.accounts.some((a) => a.last_error_until && Date.parse(a.last_error_until) <= now);
  let quota = false;
  for (const el of document.querySelectorAll(TIMED)) {
    const d = el.dataset;
    if (d.until != null) {
      if (Date.parse(d.until) <= now) expired = true;
      setTimerText(el, d.countdown === 'compact' ? compactLeft(d.until) : left(d.until));
    }
    if (d.resetDeadline != null && Date.parse(d.resetDeadline) <= now) el.disabled = true;
    if (d.ago != null) setTimerText(el, ago(d.ago));
    if (d.quotaReset != null && Date.parse(d.quotaReset) <= now) quota = true;
  }
  if (quota) {
    refreshViews();
    syncResetModal();
    expired = true;
  }
  if (expired) refreshAccounts();
}
setInterval(() => { if (!document.hidden) tickTimers(); }, 1000);

// Resync the hour of load once a minute (rolls the window forward), while the page shows.
async function pollOverview() {
  if (S.locked || !S.overview || refreshing.accounts) return;
  overviewAt = Date.now();
  try {
    S.overview = await api('/overview');
    if (S.route === 'overview') { patchFigures(); patchBars(); patchChanged('ov-pool', ovPoolHTML); }
    if (S.route === 'accounts' && S.sub) loadActivity(S.sub, true);
  } catch {}
}
setInterval(() => { if (!document.hidden && Date.now() - overviewAt >= 30000) pollOverview(); }, 60000);

window.addEventListener('beforeunload', (e) => {
  if (!pageRefreshDiscard && (configDirty() || rawDirty())) e.preventDefault();
});

window.addEventListener('storage', (e) => {
  if (e.key === `${STORE}quota-display` || e.key === null) setQuotaDisplay(e.newValue, false);
  if (e.key === `${STORE}private`) { S.private = e.newValue === '1'; render(); }
});

// Crossing the phone breakpoint switches layouts.
PHONE.addEventListener('change', () => { closeFaults(); render(); });
addEventListener('resize', () => { if (S.faults && !mob()) renderFaultsMenu(); });

async function boot() {
  render();
  try {
    await loadAll();
  } catch (e) {
    if (!(e instanceof ApiError && (e.status === 401 || e.status === 403))) {
      view.innerHTML = `<div class="lock"><h1>Can't reach Fusebox</h1><p>${esc(e.message)}. Check that the server is running, then reload.</p></div>`;
    }
    return;
  }
  render();
  if (!ws || ws.readyState > 1) connectLive();
}

$('#layer').innerHTML = '<div id="lay-faults"></div><div id="lay-drawer"></div><div id="lay-pal"></div>';
window.addEventListener('hashchange', onRoute);
document.addEventListener('visibilitychange', resume);
addEventListener('pageshow', (e) => { if (e.persisted) resume(); });
// The service worker makes the dashboard installable and shows push notifications;
// a clicked notification asks an open dashboard to go to its page.
if (window.isSecureContext && 'serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(() => {});
  navigator.serviceWorker.addEventListener('message', (e) => {
    if (e.data?.type !== 'open') return;
    const url = new URL(e.data.url, location.href);
    if (url.origin === location.origin && url.hash) location.hash = url.hash;
  });
}
{
  const { route, sub, query } = parseHash();
  S.route = route;
  S.sub = route === 'accounts' || route === 'config' ? sub : null;
  if (route === 'requests') { S.reqAcc = query.get('acc'); S.reqSess = query.get('sess'); }
  if (route === 'config' && sub && CONFIG_SECTIONS.some(([id]) => id === sub)) S.config.section = sub;
}
installPullToRefresh();
boot();
