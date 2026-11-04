'use strict';

// The Models page's "Try a model" card. Send a prompt: one prompt to one model,
// through routing like any request, streamed back. Inspect a request: paste what a
// client would send and see what Fusebox would send upstream for it (the model it
// becomes, the account, passthrough or translated, the request with credentials
// redacted, dropped controls or the refusal), without sending anything.
//
// Loaded before app.js; everything here runs only after app.js has defined S,
// esc, patch and the other helpers it uses.

const PLAY_FORMATS = [['', 'Detect'], ['responses', 'Responses'], ['chat', 'Chat Completions'], ['messages', 'Claude Messages'], ['gemini', 'Gemini']];
const PLAY_SAMPLE = '{\n  "model": "claude-sonnet-5-5",\n  "max_tokens": 256,\n  "messages": [{ "role": "user", "content": "Say hello in one sentence." }]\n}';

function openPlay() {
  if (S.play) return closePlay();
  S.play = { mode: 'send', model: S.models.find((m) => m.id.startsWith('claude-'))?.id || S.models[0]?.id || '', prompt: 'Say hello in one sentence.', out: '', error: '', meta: '', busy: false, abort: null,
    request: PLAY_SAMPLE, format: '', headers: '', client: '', inspected: null, inspectError: '' };
  patch('models-play', playHTML);
  for (const b of $$('[data-act="open-play"]')) b.setAttribute('aria-expanded', 'true');
  $('#play-prompt')?.focus();
}

function closePlay() {
  S.play?.abort?.abort();
  S.play = null;
  patch('models-play', playHTML);
  for (const b of $$('[data-act="open-play"]')) b.setAttribute('aria-expanded', 'false');
  $('[data-act="open-play"]')?.focus();
}

function stopPlay() {
  S.play?.abort?.abort();
}

function setPlayMode(mode) {
  const P = S.play;
  if (!P || P.mode === mode || P.busy) return;
  keepPlayDraft();
  P.mode = mode;
  patch('models-play', playHTML);
  $(mode === 'inspect' ? '#play-request' : '#play-prompt')?.focus();
}

// What was typed survives switching modes and re-renders.
function keepPlayDraft() {
  const P = S.play;
  const val = (sel) => $(sel)?.value;
  if (!P) return;
  for (const [key, sel] of [['model', '#play-model'], ['prompt', '#play-prompt'], ['request', '#play-request'], ['format', '#play-format'], ['headers', '#play-headers'], ['client', '#play-client']]) {
    const v = val(sel);
    if (v != null) P[key] = v;
  }
}

function playHTML() {
  const P = S.play;
  if (!P) return '';
  const modes = `<div class="seg" role="group" aria-label="Playground mode">${[['send', 'Send a prompt'], ['inspect', 'Inspect a request']].map(([m, label]) => `<button type="button" data-act="play-mode" data-id="${m}" aria-pressed="${P.mode === m}"${P.busy ? ' disabled' : ''}>${label}</button>`).join('')}</div>`;
  return `<section class="card pad play" aria-label="Try a model">
    <div class="card-head"><span class="label">Try a model</span><span class="grow"></span>${modes}<button class="icon-btn" type="button" data-act="close-play" aria-label="Close the playground">×</button></div>
    ${P.mode === 'inspect' ? inspectFormHTML(P) : sendFormHTML(P)}</section>`;
}

function sendFormHTML(P) {
  const ids = [...Object.keys(S.routes?.groups || {}), ...S.models.map((m) => m.id)];
  return `<form id="play-form" class="play-form">
      <div class="field"><label for="play-model">Model</label><input id="play-model" list="play-models" value="${esc(P.model)}" required autocomplete="off" spellcheck="false" placeholder="claude-sonnet-5-5">
        <datalist id="play-models">${ids.map((id) => `<option value="${esc(id)}">`).join('')}</datalist></div>
      <div class="field"><label for="play-prompt">Prompt</label><textarea id="play-prompt" rows="3" required>${esc(P.prompt)}</textarea></div>
      <div class="play-actions"><button class="btn primary" type="submit"${P.busy ? ' disabled' : ''}>${P.busy ? 'Sending…' : 'Send'}</button>
        ${P.busy ? '<button class="btn ghost" type="button" data-act="stop-play">Stop</button>' : ''}
        <span class="meta" id="play-meta" role="status">${esc(P.meta || 'Shows on the Usage page as the client dashboard.')}</span></div>
    </form>
    <pre class="play-out${P.error ? ' err' : ''}" id="play-out"${P.out || P.error ? '' : ' hidden'}>${esc(P.error || P.out)}</pre>`;
}

function inspectFormHTML(P) {
  return `<form id="inspect-form" class="play-form inspect-form">
      <div class="field"><label for="play-format">Format</label><select id="play-format">${PLAY_FORMATS.map(([v, label]) => `<option value="${v}"${P.format === v ? ' selected' : ''}>${label}</option>`).join('')}</select>
        <label for="play-client">Named client <span class="meta">optional</span></label><input id="play-client" value="${esc(P.client)}" autocomplete="off" spellcheck="false" placeholder="client id">
        <label for="play-headers">Headers <span class="meta">optional, one per line</span></label><textarea id="play-headers" rows="3" spellcheck="false" placeholder="user-agent: claude-cli/2.1.318 (external, cli)">${esc(P.headers)}</textarea></div>
      <div class="field"><label for="play-request">Request body</label><textarea id="play-request" class="mono" rows="9" spellcheck="false" required>${esc(P.request)}</textarea></div>
      <div class="play-actions"><button class="btn primary" type="submit"${P.busy ? ' disabled' : ''}>${P.busy ? 'Inspecting…' : 'Inspect, don’t send'}</button>
        <span class="meta" role="status">Nothing is sent: no quota, no usage, no log.</span></div>
    </form>
    ${P.inspectError ? `<pre class="play-out err">${esc(P.inspectError)}</pre>` : ''}${P.inspected ? inspectedHTML(P.inspected) : ''}`;
}

function inspectedHTML(r) {
  const row = (label, value) => (value ? `<div class="inspect-row"><span class="meta">${label}</span><span>${value}</span></div>` : '');
  const chip = (text) => `<span class="idchip static">${esc(text)}</span>`;
  const model = [r.requested_model && r.requested_model !== r.model ? `${chip(r.requested_model)} → ` : '', r.model ? chip(r.model) : '', r.reasoning_suffix ? ` <span class="meta">${esc(r.reasoning_suffix)}</span>` : ''].join('');
  const how = [r.mapped && `mapped by ${esc(r.mapped.client)}`, r.group && `model group ${esc(r.group.name)}: ${r.group.order.map(esc).join(' → ')}`, r.background_model && 'Claude Code background call'].filter(Boolean).join(' · ');
  const acct = r.account ? `${esc(who(r.account.label || r.account.id))} <span class="meta">${esc(PROVIDER[r.account.provider] || r.account.provider)}${r.account.subscription ? ' subscription' : ''} · ${esc(r.account.upstream_model)}</span>` : '';
  const shape = r.shape ? `${r.shape.path === 'passthrough' ? 'Passed through' : 'Translated'} to ${esc(r.shape.wire)}${r.shape.upstream_stream ? ', streamed' : ''}` : '';
  const route = (r.route || []).map((s) => `${chip(who(s.account))}<span class="meta"> ${esc(s.state)}${s.next ? ' · next' : ''}${s.fallback ? ' · fallback' : ''}</span>`).join(' ');
  const req = r.request;
  const headers = req ? req.headers.map(([k, v]) => `${k}: ${v}`).join('\n') : '';
  return `<div class="inspect-out" aria-label="Inspection">
      ${r.refusal ? `<p class="msg err">Refused with ${r.refusal.status}: ${esc(r.refusal.message)}</p>` : ''}
      ${row('Model', model)}${row('How', how)}${row('Account', acct)}${row('Path', shape)}
      ${row('Dropped', (r.dropped_controls || []).map((d) => chip(d)).join(' '))}${row('Route', route)}
      ${req ? `<div class="inspect-row"><span class="meta">Request</span><span class="mono">POST ${esc(req.url)} · ${fmt(req.bytes)} bytes</span></div>
      <pre class="play-out mono">${esc(headers)}\n\n${esc(JSON.stringify(req.body, null, 2))}</pre>` : ''}
      ${(r.notes || []).map((n) => `<p class="meta inspect-note">${esc(n)}</p>`).join('')}</div>`;
}

function parseHeaderLines(text) {
  const out = {};
  for (const line of String(text || '').split('\n')) {
    const at = line.indexOf(':');
    if (at <= 0) continue;
    out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  return out;
}

async function runInspect() {
  const P = S.play;
  if (!P || P.busy) return;
  keepPlayDraft();
  let body;
  try { body = JSON.parse(P.request); } catch (e) {
    P.inspectError = `The request body isn't JSON: ${e.message}`;
    P.inspected = null;
    return patch('models-play', playHTML);
  }
  const ask = { body, format: P.format || undefined, headers: parseHeaderLines(P.headers), client: P.client.trim() || undefined };
  if ((P.format === 'gemini' || (!P.format && body.contents)) && !body.model) ask.model = S.models.find((m) => m.id.startsWith('gemini'))?.id;
  Object.assign(P, { busy: true, inspectError: '' });
  patch('models-play', playHTML);
  try {
    const headers = { 'content-type': 'application/json' };
    if (S.key) headers.authorization = `Bearer ${S.key}`;
    const res = await fetch('/api/inspect', { method: 'POST', headers, body: JSON.stringify(ask) });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.error?.message || data.error || res.statusText);
    P.inspected = data;
  } catch (e) {
    P.inspected = null;
    P.inspectError = e.message;
  }
  P.busy = false;
  if (S.play === P) patch('models-play', playHTML);
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
  if (e.target.id === 'play-form') { e.preventDefault(); runPlay(e.target); }
  else if (e.target.id === 'inspect-form') { e.preventDefault(); runInspect(); }
});
document.addEventListener('keydown', (e) => {
  if (!(e.key === 'Enter' && (e.metaKey || e.ctrlKey))) return;
  if (e.target.id === 'play-prompt') { e.preventDefault(); runPlay(e.target.form); }
  else if (e.target.id === 'play-request') { e.preventDefault(); runInspect(); }
});
