#!/usr/bin/env node
// Plays a Fusebox's side of the push relay for a Debug build on a simulator, which registered
// with FUSEBOX_DEBUG_FORCE_RELAY=1 and wrote its handle and key to its own container
// (Library/Caches/relay-debug.json). The key is read from there and never printed.
//
//   node ios/scripts/relay-e2e.mjs <simulator> wait               until the app has registered
//   node ios/scripts/relay-e2e.mjs <simulator> send <account id> [title] [provider]
//   node ios/scripts/relay-e2e.mjs <simulator> cleanup            DELETE the handle at the relay
//   node ios/scripts/relay-e2e.mjs - health                       GET /healthz
//
// Sealed as src/push/relay.rs seals: standard base64 of nonce(12) ‖ ciphertext ‖ tag(16),
// AES-256-GCM with the AAD "fusebox-push-v1".
import { createCipheriv, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const RELAY = process.env.FUSEBOX_RELAY || 'https://push.whombat.com';
const [sim, command, ...rest] = process.argv.slice(2);

function device() {
  const container = execFileSync('xcrun', ['simctl', 'get_app_container', sim, 'com.digitalpals.fusebox', 'data'], { encoding: 'utf8' }).trim();
  const file = join(container, 'Library/Caches/relay-debug.json');
  return existsSync(file) ? { file, ...JSON.parse(readFileSync(file, 'utf8')) } : { file };
}

function seal(key, plaintext) {
  const nonce = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, nonce);
  cipher.setAAD(Buffer.from('fusebox-push-v1'));
  const body = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return Buffer.concat([nonce, body, cipher.getAuthTag()]).toString('base64');
}

async function health() {
  const res = await fetch(`${RELAY}/healthz`);
  console.log('healthz', res.status, await res.text());
}

switch (command) {
  case 'wait': {
    const deadline = Date.now() + 180_000;
    let d = device();
    while (!d.handle && Date.now() < deadline) { await new Promise((r) => setTimeout(r, 1000)); d = device(); }
    if (!d.handle) throw new Error(`no ${d.file} after 3 minutes`);
    console.log('registered: handle', `${d.handle.slice(0, 6)}…`, 'at', d.relay);
    break;
  }
  case 'send': {
    const [account, title = 'Relay check', provider = 'claude'] = rest;
    if (!account) throw new Error('send needs an account id');
    const d = device();
    if (!d.handle) throw new Error(`no ${d.file}: turn notifications on in the app first`);
    const key = Buffer.from(d.key, 'base64');
    const contents = {
      title, body: `${account} came through the push relay, sealed.`,
      path: `#/accounts/${encodeURIComponent(account)}`, kind: 'signin', level: 'err',
      provider, account, thread: provider,
    };
    const res = await fetch(`${d.relay}/v1/send`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        handle: d.handle, sealed: seal(key, JSON.stringify(contents)), badge: 1,
        collapse_id: randomBytes(16).toString('hex'), priority: 'high', ttl: 600,
      }),
    });
    console.log('send', res.status, await res.text());
    if (!res.ok) process.exit(1);
    break;
  }
  case 'cleanup': {
    const d = device();
    if (!d.handle) { console.log('no handle in the app; nothing to remove'); await health(); break; }
    const res = await fetch(`${d.relay}/v1/devices/${d.handle}`, { method: 'DELETE' });
    console.log('delete', res.status);
    await health();
    break;
  }
  case 'health': await health(); break;
  default:
    console.error('usage: relay-e2e.mjs <simulator> wait | send <account id> [title] | cleanup; relay-e2e.mjs - health');
    process.exit(2);
}
