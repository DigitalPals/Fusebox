#!/usr/bin/env node
// Captures the management API's responses from a running Fusebox and writes them,
// anonymised, to FuseboxTests/Fixtures for the JSON decoding tests.
//
//   node ios/scripts/capture-fixtures.mjs [--url https://fusebox.example] [--key-file path]
//
// The server is --url or FUSEBOX_URL (required); the key defaults to the one in
// ~/.config/fusebox-ios/management-key. The key is only sent in the Authorization
// header and never printed. Emails, account and client labels, addresses, keys,
// endpoints and session ids are replaced consistently before anything is written,
// and the script refuses to write a file in which a known secret still appears.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const base = (flag('url', process.env.FUSEBOX_URL) || '').replace(/\/$/, '');
if (!base) throw new Error('Pass --url or set FUSEBOX_URL');
const key = readFileSync(flag('key-file', join(homedir(), '.config/fusebox-ios/management-key')), 'utf8').trim();
const out = resolve(dirname(fileURLToPath(import.meta.url)), '../FuseboxTests/Fixtures');
mkdirSync(out, { recursive: true });

async function get(path) {
  const res = await fetch(`${base}/api${path}`, { headers: { authorization: `Bearer ${key}` } });
  if (!res.ok) throw new Error(`GET ${path}: ${res.status}`);
  return res.json();
}

// Consistent replacements, so ids still match across files.
const maps = new Map();
const alias = (kind, value, make) => {
  if (!maps.has(kind)) maps.set(kind, new Map());
  const m = maps.get(kind);
  if (!m.has(value)) m.set(value, make(m.size + 1));
  return m.get(value);
};
const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g;
const email = (e) => alias('email', e.toLowerCase(), (n) => `user${n}@example.com`);
const hex = (s, len) => createHash('sha256').update(`fixture:${s}`).digest('hex').slice(0, len);
const scrubText = (s) => s.replace(EMAIL, email).replace(/\b100\.\d+\.\d+\.\d+\b/g, '100.64.0.1');

const CLIENT_LISTS = new Set(['clients', 'client']); // facets/attribution lists, usage breakdown rows
function scrub(value, keyName = '', parent = '') {
  if (Array.isArray(value)) return value.map((v) => scrub(v, keyName, keyName));
  if (value && typeof value === 'object') {
    const o = {};
    for (const [k, v] of Object.entries(value)) {
      if (CLIENT_LISTS.has(parent) && (k === 'id' || k === 'label') && typeof v === 'string') {
        o[k] = k === 'id' ? alias('client-id', v, (n) => `client-${n}`) : alias('client-label', v, (n) => `Client ${n}`);
        continue;
      }
      o[k] = scrub(v, k, parent);
    }
    return o;
  }
  if (typeof value !== 'string') return value;
  switch (keyName) {
    case 'client_keys': return 'fbx_fixture_client_key';
    case 'public_key': return 'BFixturePublicKey';
    case 'endpoint': return `https://push.example.com/${hex(value, 16)}`;
    case 'origin': return 'https://fusebox.example.com';
    case 'ip': return '100.64.0.1';
    case 'login': return 'tailnet-user@example.com';
    case 'key': return value.startsWith('quota:') || value.includes(':') ? scrubText(value) : alias('key', value, (n) => `client-key-${n}`);
    case 'key_label': return alias('keylabel', value, (n) => `Device ${n}`);
    case 'session': case 'session_id': return hex(value, 64);
    case 'request_id': case 'quote': return `00000000-0000-4000-8000-${hex(value, 12)}`;
    case 'path': return value.startsWith('#/accounts/') ? `#/accounts/${encodeURIComponent(scrubText(decodeURIComponent(value.slice(11))))}` : value;
    case 'config_path': return '/etc/fusebox/config.yaml';
    case 'auth_dir': return '/var/lib/fusebox/auth';
    default: return scrubText(value);
  }
}

const today = new Date();
const day = (d) => d.toISOString().slice(0, 10);
const start = new Date(today.getTime() - 6 * 86400e3);
const end = new Date(today.getTime() + 86400e3);
const files = {
  'overview.json': '/overview',
  'accounts.json': '/accounts',
  'faults.json': '/faults',
  'requests.json': '/requests?limit=40',
  'push.json': '/push',
  'usage-dashboard.json': `/usage/dashboard?start=${day(start)}&end=${day(end)}&timezone=Europe%2FAmsterdam`,
  'usage-speed.json': `/usage/speed?start=${day(start)}&end=${day(end)}&timezone=Europe%2FAmsterdam`,
};
const raw = {};
for (const [name, path] of Object.entries(files)) raw[name] = await get(path);
const firstAccount = raw['accounts.json'][0]?.id;
if (firstAccount) raw['activity.json'] = await get(`/accounts/${encodeURIComponent(firstAccount)}/activity`);

const secrets = [key, ...raw['overview.json'].client_keys || []];
for (const [name, data] of Object.entries(raw)) {
  const text = `${JSON.stringify(scrub(data), null, 1)}\n`;
  for (const s of secrets) if (s && text.includes(s)) throw new Error(`${name} still contains a secret`);
  const leftover = text.match(EMAIL)?.find((e) => !e.endsWith('@example.com'));
  if (leftover) throw new Error(`${name} still contains an email address`);
  writeFileSync(join(out, name), text);
  console.log(`wrote ${name}`);
}
