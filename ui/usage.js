'use strict';

// Small shared helpers are exported for date, privacy and chart tests.
const UsageTools = (() => {
  const escape = (v) => String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const date = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const shift = (value, days) => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) throw new Error('Choose a valid date.');
    const [y, m, d] = value.split('-').map(Number);
    const at = new Date(Date.UTC(y, m - 1, d));
    if (at.toISOString().slice(0, 10) !== value) throw new Error('Choose a valid date.');
    at.setUTCDate(at.getUTCDate() + days);
    return at.toISOString().slice(0, 10);
  };
  const range = (days, now = new Date()) => {
    const today = date(now);
    return { start: shift(today, 1 - days), end: shift(today, 1), through: today };
  };
  // Every local calendar date from start (inclusive) to end (exclusive).
  const dates = (r) => {
    const out = [];
    for (let d = r.start; d < r.end && out.length < 3660; d = shift(d, 1)) out.push(d);
    return out;
  };
  const query = (r, filters, timezone) => {
    shift(r.start, 0); shift(r.end, 0);
    if (r.start >= r.end) throw new Error('The end date must be on or after the start date.');
    const q = new URLSearchParams({ start: r.start, end: r.end, timezone });
    for (const k of ['provider', 'model', 'account', 'client']) if (filters[k]) q.set(k, filters[k]);
    return q.toString();
  };
  const privateLabel = (value, hidden) => hidden && value != null && value !== '' ? '••••••' : String(value ?? 'Unknown');
  // Totals: cents above a dollar, four places below so small estimates stay visible.
  const dollars = (nanos) => {
    if (nanos == null || !Number.isFinite(Number(nanos))) return 'Unpriced';
    const v = Number(nanos) / 1e9, small = v !== 0 && Math.abs(v) < 1;
    return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: small ? 4 : 2, maximumFractionDigits: small ? 4 : 2 }).format(v);
  };
  const compact = (v) => v == null || !Number.isFinite(Number(v)) ? 'Unknown' : v >= 1e9 ? `${(v / 1e9).toFixed(2)}B` : v >= 1e6 ? `${(v / 1e6).toFixed(1)}M` : v >= 1e3 ? `${(v / 1e3).toFixed(1)}k` : String(Math.round(v));

  // BY DAY series: the top five groups keep their own colour, the rest merge into Other.
  const OTHER = '\u0000other';
  const HUES = [30, 90, 150, 210, 270, 330];
  const chart = (trend, days, metric) => {
    const value = (r) => metric === 'calls' ? Number(r.observations || 0) : Number(r.estimated_cost_nanos || 0) / 1e9;
    const totals = new Map(), volumes = new Map();
    for (const r of trend || []) {
      const k = r.group ?? '';
      totals.set(k, (totals.get(k) || 0) + value(r));
      volumes.set(k, (volumes.get(k) || 0) + Number(r.observations || 0));
    }
    const ranked = [...totals.keys()].filter((k) => totals.get(k) > 0 || volumes.get(k) > 0).sort((a, b) => totals.get(b) - totals.get(a) || volumes.get(b) - volumes.get(a) || a.localeCompare(b));
    const shown = ranked.length > 6 ? ranked.slice(0, 5) : ranked;
    const keys = shown.concat(ranked.length > shown.length ? [OTHER] : []);
    // A light page needs darker tones for the same contrast.
    const light = typeof document !== 'undefined' && document.documentElement.dataset.shade === 'light';
    const colors = keys.map((k, i) => (k === OTHER ? (light ? '#a8a397' : '#57534b') : `oklch(${light ? 0.6 : 0.74} 0.09 ${HUES[i]})`));
    const index = new Map(days.map((d, i) => [d, i]));
    const series = days.map((date) => ({ date, parts: keys.map(() => 0), groups: keys.map(() => ({ calls: 0, cost: 0, priced: false, unpriced: 0 })), total: 0, calls: 0, cost: 0, priced: false, unpriced: 0 }));
    for (const r of trend || []) {
      const day = series[index.get(r.date)];
      if (!day) continue;
      const v = value(r), k = keys.indexOf(shown.includes(r.group ?? '') ? r.group ?? '' : OTHER);
      if (k >= 0) {
        day.parts[k] += v;
        const group = day.groups[k];
        group.calls += Number(r.observations || 0);
        group.unpriced += Number(r.unpriced || 0);
        if (r.estimated_cost_nanos != null) { group.cost += Number(r.estimated_cost_nanos); group.priced = true; }
      }
      day.total += v;
      day.calls += Number(r.observations || 0);
      if (r.estimated_cost_nanos != null) { day.cost += Number(r.estimated_cost_nanos); day.priced = true; }
      day.unpriced += Number(r.unpriced || 0);
    }
    let peak = series.length - 1;
    series.forEach((d, i) => { if (d.total > series[peak].total || d.total === series[peak].total && d.calls > series[peak].calls) peak = i; });
    return { keys, colors, series, peak, max: Math.max(0, ...series.map((d) => d.total)), OTHER };
  };
  // Local calendar date of an instant in the page's timezone.
  const localDate = (ms, timezone) => new Intl.DateTimeFormat('en-CA', { timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(new Date(ms));
  const beforeProxy = (day, firstProxyMs, timezone) => firstProxyMs != null && Number.isFinite(Number(firstProxyMs)) && day < localDate(Number(firstProxyMs), timezone);
  // Sizes in decimal units, as the server's messages: "512 B", "3.4 MB", "128 MB", "1.2 GB".
  const bytes = (v) => {
    const n = Math.max(0, Number(v) || 0);
    const [div, unit] = n >= 1e9 ? [1e9, 'GB'] : n >= 1e6 ? [1e6, 'MB'] : n >= 1e3 ? [1e3, 'KB'] : [1, 'B'];
    const x = n / div;
    return `${unit === 'B' ? Math.round(x) : x < 10 ? x.toFixed(1).replace(/\.0$/, '') : Math.round(x)} ${unit}`;
  };
  const W = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'], M = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const parts = (iso) => { const at = new Date(`${iso}T00:00:00Z`); return { w: W[at.getUTCDay()], m: M[at.getUTCMonth()], d: at.getUTCDate(), y: at.getUTCFullYear(), monday: at.getUTCDay() === 1 }; };
  const dayLabel = (iso) => { const p = parts(iso); return `${p.w} ${p.m} ${p.d}`; };
  const shortDate = (iso) => { const p = parts(iso); return `${p.m} ${p.d}`; };
  return { OTHER, escape, date, shift, range, dates, query, privateLabel, dollars, compact, chart, localDate, beforeProxy, parts, dayLabel, shortDate, bytes };
})();
if (typeof module !== 'undefined' && module.exports) module.exports = UsageTools;

const Usage = {
  preset: '7', range: UsageTools.range(7), timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
  filters: { provider: '', model: '', account: '', client: '' },
  stack: 'provider', metric: 'cost', hoverDay: null, dim: 'model', showAll: false, clientsAll: false,
  drawerOpen: false, customError: '',
  summary: null, status: null,
  loading: false, loaded: false, error: '', actionError: '', busy: false,
  sequence: 0, credential: null, confirm: null, roots: {}, collectorDraft: '', rows: [],
};
const ux = UsageTools.escape;
const usageCount = (value) => value == null ? 'Unknown' : new Intl.NumberFormat('en-US').format(value);
const usageSensitive = (value) => ux(UsageTools.privateLabel(value, S.private));
const usageSource = (source) => ({ proxy: 'Proxy', claude_code: 'Claude Code history', codex: 'Codex history' }[source] || source || 'Unknown source');
const usageTime = (value) => value == null || value === 0 ? 'Not reported' : Number.isFinite(new Date(value).getTime()) ? new Date(value).toLocaleString(undefined, { timeZone: Usage.timezone }) : 'Not reported';
const usageClock = (value) => value == null || value === 0 ? 'Not reported' : new Date(value).toLocaleTimeString('en-GB', { timeZone: Usage.timezone, hour12: false });
const usageButton = (act, label, extra = '', disabled = false, cls = 'btn') => `<button class="${cls}" type="button" data-usage-act="${act}" ${extra}${disabled || Usage.busy ? ' disabled' : ''}>${label}</button>`;
const usagePhone = () => typeof mob === 'function' && mob();
const usageCombined = () => Usage.summary?.combined || {};
// Usage records name providers by API vendor; the dashboard names them by account kind.
const USAGE_PROVIDER = { anthropic: 'claude', openai: 'codex', google: 'gemini' };
const usageProvKey = (id) => USAGE_PROVIDER[id] || id;
const usageProvName = (id) => id == null || id === '' ? 'Unknown provider' : typeof provName === 'function' ? provName({ provider: usageProvKey(id) }) : String(id);
const usageLogo = (provider, size = 16) => typeof logo === 'function' ? logo(usageProvKey(provider), null, null, size) : '';
const usageAccount = (id) => id != null && typeof accountById === 'function' ? accountById(id) : null;
const usageAccountName = (id, label) => {
  const a = usageAccount(id);
  if (a && typeof acctLabel === 'function') return ux(acctLabel(a));
  return id == null && !label ? 'No account reported' : usageSensitive(label || id);
};
const usageClientLabel = (id) => (Usage.summary?.facets?.clients || []).find((c) => c.id === id)?.label || id;
const usageStack = (k) => k === UsageTools.OTHER ? 'Other' : Usage.stack === 'model' ? k || 'Unknown model' : usageProvName(k);
const usageDollarsShort = (v) => v >= 10 ? `$${Math.round(v).toLocaleString('en-US')}` : v > 0 ? `$${v.toFixed(2)}` : '$0';
const usageToday = () => UsageTools.date(new Date());
const usageRangeLabel = () => Usage.preset === '1' ? 'Today' : Usage.preset === '7' ? 'Last 7 days' : Usage.preset === '30' ? 'Last 30 days' : `${UsageTools.shortDate(Usage.range.start)} – ${UsageTools.shortDate(Usage.range.through)}`;
const usageDayCost = (day) => day.priced ? UsageTools.dollars(day.cost) : day.calls ? 'Unpriced' : '$0.00';

// The page is drawn once; after that each section is its own patch target (a
// display:contents wrapper, so the layout is unchanged) and repaints only when its
// markup changes. A read repaints the sections it feeds.
const USAGE_PARTS = {
  head: () => usageHeadHTML(),
  notices: () => usageNoticesHTML(),
  cost: () => (Usage.summary ? usageCostHTML() : !Usage.loading && !Usage.error ? '<div class="card pad"><h2 class="h-sec">No usage loaded</h2><p class="meta">Refresh to read persisted metadata.</p></div>' : ''),
  breakdown: () => (Usage.summary ? usageBreakdownHTML() : ''),
  clients: () => (Usage.summary ? usageClientsHTML() : ''),
  drawer: () => (Usage.drawerOpen ? usageDrawerHTML() : ''),
};
// What a full render drew in each section, handed to the patcher by bindUsage.
let usageShell = null;
const usagePart = (name) => {
  const html = USAGE_PARTS[name]();
  if (usageShell) usageShell[name] = html;
  return `<div id="usage-part-${name}" style="display:contents">${html}</div>`;
};
function usageHTML() {
  usageShell = {};
  return `<div class="usage-page">
    ${usagePart('head')}
    ${usagePart('notices')}
    <div id="usage-results" class="usage-results" aria-busy="${Usage.loading}">${usagePart('cost')}${usagePart('breakdown')}
    ${usagePart('clients')}</div>
    ${usagePart('drawer')}
  </div>`;
}
function usageNoticesHTML() {
  const u = Usage;
  return `${u.error ? `<div class="notice usage-message err" role="alert"><span class="grow">${S.private ? 'Usage could not be loaded. Try refreshing.' : ux(u.error)}</span>${usageButton('refresh', 'Retry', '', false, 'btn sm')}</div>` : ''}
    ${u.actionError && !u.drawerOpen ? `<p class="notice usage-message err" role="alert">${S.private ? 'The action failed. Retry or check server logs.' : ux(u.actionError)}</p>` : ''}
    ${u.loading && !u.summary ? '<p class="usage-message meta" role="status">Loading usage metadata…</p>' : ''}`;
}

// The header dot also turns amber for a stale collector; the drawer's health line does not.
function usageHealthDot(collectors = true) {
  const state = Usage.status?.health?.state;
  if (!state) return 'idle';
  if (['error', 'stopped'].includes(state)) return 'err';
  const stale = collectors && (Usage.status?.collectors || []).some((c) => !c.revoked && c.state === 'offline');
  return state === 'degraded' || stale ? 'warn' : state === 'disabled' ? 'idle' : 'ok';
}
function usageBoundary() {
  const r = Usage.range, tz = Usage.timezone;
  const text = Usage.preset === '1' ? `Today, ${UsageTools.dayLabel(r.through)}` : `${UsageTools.dayLabel(r.start)} – ${UsageTools.dayLabel(r.through)}`;
  return `<span class="u-boundary" title="${ux(`${r.start} 00:00 inclusive → ${r.end} 00:00 exclusive`)}">${ux(text)} · ${ux(tz)}</span>`;
}
function usageHeadHTML() {
  const u = Usage, phone = usagePhone();
  const presets = `<div class="seg u-range" role="group" aria-label="Date range">${[['1', 'Today'], ['7', '7 days'], ['30', '30 days'], ['custom', 'Custom']].map(([v, label]) => `<button type="button" data-usage-act="range" data-value="${v}" aria-pressed="${u.preset === v}">${label}</button>`).join('')}</div>`;
  const collection = `<button class="btn u-collection" type="button" data-usage-act="open-collection" aria-haspopup="dialog"><span class="dot s7 ${usageHealthDot()}"></span>Collection</button>`;
  const refresh = `<button class="icon-btn row" type="button" data-usage-act="refresh" title="Refresh · updates every minute" aria-label="Refresh usage"${u.loading ? ' disabled' : ''}><svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><path d="M11.5 7a4.5 4.5 0 1 1-1.3-3.2"/><path d="M11.5 1.8v2.6H8.9"/></svg></button>`;
  const today = usageToday(), min = UsageTools.shift(today, -365);
  const custom = u.preset === 'custom' ? `<form id="usage-range" class="u-custom"><label class="u-field"><span>From</span><input type="date" name="start" value="${ux(u.range.start)}" min="${min}" max="${today}" required></label><label class="u-field"><span>Through</span><input type="date" name="through" value="${ux(u.range.through)}" min="${min}" max="${today}" required></label><button class="btn" type="submit">Apply dates</button>${u.customError ? `<span class="u-custom-err" role="alert">${ux(u.customError)}</span>` : ''}</form>` : '';
  const labels = { model: 'Model', account: 'Account', provider: 'Provider', client: 'Client' };
  const pills = Object.keys(labels).filter((k) => u.filters[k]).map((k) => {
    const v = u.filters[k];
    const value = k === 'account' ? usageAccountName(v) : k === 'client' ? usageSensitive(usageClientLabel(v)) : k === 'provider' ? ux(usageProvName(v)) : ux(v);
    return phone
      ? `<button class="u-pill" type="button" data-usage-act="clear-filter" data-value="${k}" aria-label="Remove ${labels[k]} filter"><span class="dim">${labels[k]}</span><span class="mono">${value}</span><span class="dim" aria-hidden="true">×</span></button>`
      : `<span class="u-pill"><span class="dim">${labels[k]}</span><span class="mono">${value}</span><button type="button" data-usage-act="clear-filter" data-value="${k}" aria-label="Remove ${labels[k]} filter">×</button></span>`;
  }).join('');
  if (phone) {
    return `<div class="u-head">${presets}${custom}<div class="u-headline">${usageBoundary()}${collection}</div>${pills ? `<div class="u-pills">${pills}</div>` : ''}</div>`;
  }
  return `<div class="u-head">
    <div class="u-titlerow"><div class="u-title"><h1 class="h-page">Usage</h1><p class="meta">What your traffic would cost at published API prices, in USD. Proxy traffic and imported Claude Code and Codex history are combined, with each call counted once. An estimate, not a subscription bill.</p></div>${presets}${collection}</div>
    ${custom}
    <div class="u-filterrow">${pills ? `${pills}<button class="u-clear" type="button" data-usage-act="clear-filters">Clear filters</button>` : '<span class="meta">All models, accounts, providers and clients. Click a row in the breakdown to narrow it down.</span>'}<span class="grow"></span>${usageBoundary()}${refresh}</div>
  </div>`;
}

function usageCoverageLine(t) {
  const n = Number(t.observations || 0), unpriced = Number(t.unpriced || 0);
  const parts = [n ? `${(Math.floor((1 - unpriced / n) * 1000) / 10).toFixed(1)}% of calls priced` : 'No matching calls'];
  if (t.matched) parts.push(`${usageCount(t.matched)} matched across sources, counted once`);
  if (t.weak_identity) parts.push(`${usageCount(t.weak_identity)} history record${t.weak_identity === 1 ? '' : 's'} without a stable ID`);
  for (const [k, label] of [['unpriced', 'unpriced'], ['partial', 'partial'], ['missing_usage', 'missing usage'], ['pricing_partial', 'with assumptions']]) if (t[k]) parts.push(`${usageCount(t[k])} ${label}`);
  if (t.aggregation_overflow) parts.push('a total exceeds the supported range, amount unknown');
  return parts.join(' · ');
}
function usageStaleLine() {
  if (Usage.range.through !== usageToday()) return '';
  const stale = (Usage.status?.collectors || []).filter((c) => !c.revoked && c.state === 'offline');
  return stale.map((c) => {
    const sources = (c.covered_sources || []).map((s) => usageSource(s).replace(' history', '')).join(' and ') || 'imported';
    const since = c.last_sync_at_ms ? new Date(c.last_sync_at_ms).toLocaleString('en-GB', { timeZone: Usage.timezone, weekday: 'short', hour: '2-digit', minute: '2-digit', hour12: false }).replace(',', '') : null;
    return `${usageSensitive(c.label || c.id)} has not synced ${since ? `since ${ux(since)}` : 'yet'}, so its recent ${ux(sources)} history may be missing`;
  }).join('. ');
}
function usageCacheLifetimes(tokens = {}) {
  const fields = [['write_5m', '5 min'], ['write_1h', '1 hr'], ['write_30m', '30 min'], ['write_12h', '12 hr']];
  const parts = fields.filter(([key]) => Number(tokens[key]) > 0).map(([key, label]) => `${usageCount(tokens[key])} (${label})`);
  const known = fields.reduce((sum, [key]) => sum + Number(tokens[key] || 0), 0);
  if (tokens.cache_write != null && Number(tokens.cache_write) > known) parts.push(`${usageCount(Number(tokens.cache_write) - known)} (lifetime unknown)`);
  if (!parts.length) return '';
  if (tokens.cache_write == null) parts.push('total unknown');
  return `Cache write lifetimes: ${parts.join(', ')}`;
}
function usageCostHTML() {
  const u = Usage, s = u.summary, t = usageCombined().totals || {}, tokens = t.tokens || {}, phone = usagePhone();
  const days = UsageTools.dates(u.range);
  const word = usageRangeLabel().toUpperCase();
  const cost = t.estimated_cost_nanos;
  const sub = u.preset === '1' ? 'So far today · priced calls only' : `Avg ${ux(UsageTools.dollars(cost == null ? null : Number(cost) / Math.max(1, days.length)))} a day · priced calls only`;
  const missingWrite = Number(t.missing_token_counts?.cache_write || 0);
  const stats = [[usageCount(t.observations ?? 0), 'Calls'], [usageCount(t.history_only ?? 0), 'From history only'], [UsageTools.compact(tokens.input), 'Tokens in'], [UsageTools.compact(tokens.output), 'Tokens out'], [UsageTools.compact(tokens.cache_read), 'Cache read'],
    [tokens.cache_write == null ? 'Unknown' : UsageTools.compact(tokens.cache_write), tokens.cache_write != null && missingWrite > 0 ? 'Cache write, partial' : 'Cache write']];
  const stale = usageStaleLine(), lifetimes = usageCacheLifetimes(tokens);
  return `<section class="card u-cost" aria-labelledby="usage-cost-label">
    <div class="u-cost-top"><div class="u-total"><span class="label" id="usage-cost-label">ESTIMATED COST · ${ux(word)}</span><span class="u-total-v">${t.aggregation_overflow && cost == null ? 'Out of range' : ux(UsageTools.dollars(cost ?? (t.observations ? null : 0)))}</span><span class="u-total-sub">${sub}</span></div>
    <dl class="u-stats">${stats.map(([v, l]) => `<div><dd>${ux(v)}</dd><dt>${ux(l)}</dt></div>`).join('')}</dl></div>
    ${days.length > 1 ? `<div id="usage-chart" class="u-chart">${usageChartHTML()}</div>` : ''}
    ${lifetimes ? `<div class="u-foot"><span class="fg2">${ux(lifetimes)}</span></div>` : ''}
    <div class="u-foot"><span class="fg2">${ux(usageCoverageLine(t))}</span>${stale ? `<span class="warn">${stale}</span>` : ''}${phone ? '' : `<span class="grow"></span><span>Unknown usage and missing prices are left out, never counted as zero · catalogue ${ux(s.pricing?.version || u.status?.pricing?.version || 'unknown')}</span>`}</div>
  </section>`;
}

function usageChartHTML() {
  const u = Usage, phone = usagePhone(), days = UsageTools.dates(u.range), c = UsageTools.chart(usageCombined().trend || [], days, u.metric);
  const calls = u.metric === 'calls', showVals = days.length <= 10 && !phone, today = usageToday();
  const hover = days.indexOf(u.hoverDay);
  const seg = (name, opts, act, label) => `<div class="seg u-seg" role="group" aria-label="${label}">${opts.map(([v, l]) => `<button type="button" data-usage-act="${act}" data-value="${v}" aria-pressed="${u[name] === v}">${l}</button>`).join('')}</div>`;
  const legend = c.keys.map((k, i) => `<span><i style="background:${c.colors[i]}"></i>${ux(usageStack(k))}</span>`).join('');
  const bars = c.series.map((d, i) => {
    const h = c.max > 0 ? Math.max(d.total > 0 ? 1.5 : 0, d.total / c.max * (showVals ? 84 : 100)) : 0;
    const value = calls ? UsageTools.compact(d.total) : d.priced || !d.calls ? usageDollarsShort(d.total) : 'Unpriced';
    const segs = d.parts.map((v, k) => (v > 0 ? `<i style="flex:${Math.max(1, Math.round(v / d.total * 1000))} 1 0px;background:${c.colors[k]}"></i>` : '')).join('');
    const description = calls ? `${usageCount(d.calls)} calls` : `${usageDayCost(d)} · ${usageCount(d.calls - d.unpriced)} priced calls${d.unpriced ? ` · ${usageCount(d.unpriced)} unpriced` : ''}`;
    return `<button type="button" class="u-bar${hover >= 0 && hover !== i ? ' dim' : ''}${hover === i ? ' on' : ''}" data-usage-day="${d.date}" aria-label="${ux(UsageTools.dayLabel(d.date))}: ${ux(description)}">${showVals ? `<span class="u-bar-v">${ux(value)}</span>` : ''}<span class="u-bar-s" style="height:${h.toFixed(2)}%">${segs}</span></button>`;
  }).join('');
  const labels = c.series.map((d) => {
    const p = UsageTools.parts(d.date);
    const text = days.length <= 10 ? (phone ? p.w.slice(0, 2) : `${p.w} ${p.d}`) : p.monday ? `${p.m} ${p.d}` : '';
    return `<span${d.date === today ? ' class="today"' : ''}>${ux(text)}</span>`;
  }).join('');
  return `<div class="u-chart-head"><span class="label">BY DAY</span>${calls ? '' : '<span class="meta u-chart-scope">Priced calls only</span>'}<span class="grow"></span>${seg('stack', [['provider', 'Provider'], ['model', 'Model']], 'stack', 'Stack bars by')}${seg('metric', [['cost', 'Cost'], ['calls', 'Calls']], 'metric', 'Bar value')}</div>
    ${legend ? `<div class="u-legend">${legend}</div>` : ''}
    <div class="u-bars${days.length > 20 ? ' dense' : ''}" data-usage-bars>${bars}</div>
    <div class="u-days${days.length > 20 ? ' dense' : ''}" aria-hidden="true">${labels}</div>
    <div class="u-readout" role="status">${usageReadoutHTML(c, hover)}</div>`;
}
function usageReadoutHTML(c, hover) {
  if (!c.series.length) return '';
  const i = hover >= 0 ? hover : c.peak, d = c.series[i], calls = Usage.metric === 'calls';
  const title = `${hover >= 0 ? 'Day' : !calls && !c.max && d.calls ? 'Busiest day' : 'Peak day'} · ${UsageTools.dayLabel(d.date)}${d.date === usageToday() ? ', so far' : ''}`;
  const parts = d.groups.map((g, k) => (g.calls || g.priced ? `<span class="u-rp"><i style="background:${c.colors[k]}"></i><span class="u-rp-name">${ux(usageStack(c.keys[k]))}</span> <span class="mono">${ux(calls ? `${usageCount(g.calls)} calls` : usageDayCost(g))}</span>${g.unpriced ? `<span class="dim">(${calls ? '' : `${usageCount(g.calls - g.unpriced)} priced · `}${usageCount(g.unpriced)} unpriced)</span>` : ''}</span>` : '')).join('');
  const before = UsageTools.beforeProxy(d.date, usageCombined().proxy_first_event_at_ms, Usage.timezone);
  return `<span class="${hover >= 0 ? '' : 'dim'}">${ux(title)}</span><span class="mono">${ux(usageDayCost(d))}</span><span class="fg2">${usageCount(d.calls)} call${d.calls === 1 ? '' : 's'}</span>${parts}${before ? '<span class="dim">Before the proxy was set up · history only</span>' : ''}`;
}

const USAGE_DIMS = [['model', 'Model'], ['account', 'Account'], ['provider', 'Provider'], ['client', 'Client / origin']];
function usageBreakdownRow(dim, r) {
  if (dim === 'model') return { name: r.id || 'Unknown model', mono: true, sub: usageProvName(r.provider), logo: usageLogo(r.provider) };
  if (dim === 'provider') return { name: usageProvName(r.id), sub: r.accounts ? `${r.accounts} account${r.accounts === 1 ? '' : 's'}` : 'No account reported', logo: usageLogo(r.id) };
  if (dim === 'account') {
    const a = usageAccount(r.id);
    if (!a) return { name: r.id == null ? 'No account reported' : r.id, sensitive: r.id != null, sub: r.id == null ? 'Imported history does not record the account' : usageProvName(r.provider), logo: usageLogo(r.provider) };
    return { name: typeof acctLabel === 'function' ? acctLabel(a) : a.label, sub: `${typeof provName === 'function' ? provName(a) : a.provider} · ${(typeof planName === 'function' && planName(a)) || (typeof authName === 'function' ? authName(a) : '')}`, logo: typeof acctLogo === 'function' ? acctLogo(a, 16) : '' };
  }
  const collector = String(r.id || '').startsWith('collector:');
  return { name: r.id == null ? 'No client reported' : usageClientLabel(r.id), sensitive: r.id != null, sub: collector ? 'Collector' : '', square: true };
}
function usageCoverage(r) {
  const n = Number(r.observations || 0);
  if (n && Number(r.unpriced || 0) >= n) return ['Unpriced', 'fg2'];
  if (r.unpriced) return [`${usageCount(r.unpriced)} unpriced`, 'warn'];
  if (r.partial) return [`${usageCount(r.partial)} partial`, 'warn'];
  if (r.missing_usage) return [`${usageCount(r.missing_usage)} no usage`, 'warn'];
  return ['Full', 'dim'];
}
// A row's share of the range's estimated cost: the bar's fraction and its label.
function usageCostShare(r, total) {
  const share = total > 0 && r.estimated_cost_nanos != null ? Number(r.estimated_cost_nanos) / total : 0;
  return [share, total > 0 && r.estimated_cost_nanos ? (share * 100 < 1 ? '<1%' : `${Math.round(share * 100)}%`) : '—'];
}
function usageBreakdownHTML() {
  const u = Usage, b = usageCombined().breakdowns || {}, phone = usagePhone();
  const rows = b[u.dim] || [], total = Number(usageCombined().totals?.estimated_cost_nanos || 0);
  const rowLimit = u.dim === 'model' ? 20 : 8;
  const shown = u.showAll ? rows : rows.slice(0, rowLimit);
  u.rows = shown.map((r) => r.id);
  const tabs = `<div class="u-tabs" role="tablist" aria-label="Breakdown">${USAGE_DIMS.map(([k, l]) => `<button type="button" role="tab" data-usage-act="dim" data-value="${k}" aria-selected="${u.dim === k}">${l} <span class="mono dim">${(b[k] || []).length}</span></button>`).join('')}</div>`;
  const body = shown.map((r, i) => {
    const x = usageBreakdownRow(u.dim, r), selected = r.id != null && u.filters[u.dim] === r.id;
    const [share, pct] = usageCostShare(r, total);
    const [cov, covCls] = usageCoverage(r);
    const coverageTitle = ux(usageCoverageLine(r));
    const name = x.sensitive ? usageSensitive(x.name) : ux(x.name);
    const icon = x.square ? '<span class="u-sq" aria-hidden="true"><i></i></span>' : x.logo || '<span class="u-sq" aria-hidden="true"><i></i></span>';
    const cost = r.estimated_cost_nanos != null ? UsageTools.dollars(r.estimated_cost_nanos) : 'Unpriced';
    const attrs = r.id != null ? `data-usage-act="filter-row" data-index="${i}" aria-pressed="${selected}"` : 'disabled';
    const bar = `<span class="u-share-bar"><i style="width:${(share * 100).toFixed(1)}%"></i></span>`;
    if (phone) return `<button type="button" class="u-bd-item${selected ? ' on' : ''}" ${attrs}><span class="u-bd-l1">${icon}<span class="u-bd-name${x.mono ? ' mono' : ''}">${name}</span><span class="u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span></span><span class="u-bd-l2">${bar}<span class="dim">${ux(pct)} · ${usageCount(r.observations)} calls</span>${cov !== 'Full' ? `<span class="${covCls}" title="${coverageTitle}">${ux(cov)}</span>` : ''}</span></button>`;
    return `<button type="button" class="u-bd-row${selected ? ' on' : ''}" ${attrs}><span class="u-bd-who">${icon}<span class="cell2"><span class="${x.mono ? 'mono' : ''}">${name}</span>${x.sub ? `<span class="dim">${ux(x.sub)}</span>` : ''}</span></span><span class="u-share">${bar}<span class="fg2">${ux(pct)}</span></span><span class="r">${usageCount(r.observations)}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(r.tokens?.input))}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(r.tokens?.output))}</span><span class="r u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span><span class="r ${covCls}" title="${coverageTitle}">${ux(cov)}</span></button>`;
  }).join('');
  const more = rows.length > rowLimit ? `<button type="button" class="u-more" data-usage-act="show-all">${u.showAll ? `Show top ${rowLimit}` : `Show all ${rows.length}`}</button>` : '';
  const empty = rows.length ? '' : '<p class="u-empty">No records match these filters.</p>';
  if (phone) return `<section class="sec u-breakdown"><h2 class="h-sec">Breakdown · ${ux(usageRangeLabel())}</h2>${tabs}<div class="card u-bd-list">${body}${empty}${more}</div></section>`;
  const dimLabel = USAGE_DIMS.find(([k]) => k === u.dim)[1].toUpperCase();
  return `<section class="card u-breakdown"><div class="u-bd-head"><span class="label">BREAKDOWN · ${ux(usageRangeLabel().toUpperCase())}</span>${tabs}<span class="grow"></span><span class="meta">Priced calls only · Click a row to filter the page</span></div>
    <div class="u-bd-th" aria-hidden="true"><span>${ux(dimLabel)}</span><span>SHARE OF COST</span><span class="r">CALLS</span><span class="r u-wide">IN</span><span class="r u-wide">OUT</span><span class="r">EST. COST</span><span class="r">COVERAGE</span></div>
    ${body}${empty}${more}</section>`;
}

// Top clients: what each client used in the range, largest estimated cost first, and
// the models it used.
const USAGE_CLIENT_ROWS = 8;
// Every token a client sent or received: cache reads and writes are kept apart from input.
const usageTokenTotal = (t = {}) => ['input', 'cache_read', 'cache_write', 'output'].reduce((n, k) => n + Number(t[k] || 0), 0);
const usageTokenSplit = (t = {}) => `${usageCount(t.input || 0)} input, ${usageCount(t.output || 0)} output, ${usageCount(t.cache_read || 0)} read from the cache, ${usageCount(t.cache_write || 0)} written to the cache`;
function usageModelBadges(r) {
  const models = r.models || [], cost = Number(r.estimated_cost_nanos || 0);
  const badges = models.slice(0, 3).map((m) => {
    const title = [m.estimated_cost_nanos != null ? UsageTools.dollars(m.estimated_cost_nanos) : 'Unpriced', `${usageCount(m.observations)} ${m.observations === 1 ? 'call' : 'calls'}`];
    if (cost > 0 && m.estimated_cost_nanos) title.push(`${usageCostShare(m, cost)[1]} of this client`);
    return `<span class="idchip static" title="${ux(title.join(' · '))}">${ux(m.id || 'Unknown model')}</span>`;
  });
  if (models.length > 3) badges.push(`<span class="idchip static" title="${ux(models.slice(3).map((m) => m.id || 'Unknown model').join(', '))}">+${models.length - 3}</span>`);
  return badges.join('') || '<span class="dim">No model reported</span>';
}
function usageClientsHTML() {
  const u = Usage, rows = usageCombined().breakdowns?.client || [], phone = usagePhone();
  const total = Number(usageCombined().totals?.estimated_cost_nanos || 0);
  const shown = u.clientsAll ? rows : rows.slice(0, USAGE_CLIENT_ROWS);
  const body = shown.map((r) => {
    const name = r.id == null ? 'No client reported' : usageSensitive(usageClientLabel(r.id));
    const [share, pct] = usageCostShare(r, total);
    const cost = r.estimated_cost_nanos != null ? UsageTools.dollars(r.estimated_cost_nanos) : 'Unpriced';
    const tokens = UsageTools.compact(usageTokenTotal(r.tokens)), split = ux(usageTokenSplit(r.tokens));
    const bar = `<span class="u-share-bar"><i style="width:${(share * 100).toFixed(1)}%"></i></span>`;
    if (phone) return `<div class="u-tc-item"><span class="u-bd-l1"><span class="u-bd-name">${name}</span><span class="u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span></span><span class="u-bd-l2">${bar}<span class="dim" title="${split}">${ux(pct)} · ${ux(tokens)} tokens</span></span><span class="u-tc-models">${usageModelBadges(r)}</span></div>`;
    return `<div class="u-tc-row"><span class="u-tc-name">${name}</span><span class="u-share">${bar}<span class="fg2">${ux(pct)}</span></span><span class="r u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span><span class="r mono fg2" title="${split}">${ux(tokens)}</span><span class="u-tc-models">${usageModelBadges(r)}</span></div>`;
  }).join('');
  const more = rows.length > USAGE_CLIENT_ROWS ? `<button type="button" class="u-more" data-usage-act="clients-all">${u.clientsAll ? `Show top ${USAGE_CLIENT_ROWS}` : `Show all ${rows.length}`}</button>` : '';
  const empty = rows.length ? '' : '<p class="u-empty">No records match these filters.</p>';
  const th = phone || !rows.length ? '' : '<div class="u-tc-th" aria-hidden="true"><span>CLIENT</span><span>SHARE OF COST</span><span class="r">EST. COST</span><span class="r">TOKENS</span><span>MODELS</span></div>';
  return `<section class="sec u-clients" aria-labelledby="usage-clients-title"><div class="sec-head"><h2 class="h-sec" id="usage-clients-title">Top clients</h2>${phone ? '' : '<span class="meta">By estimated cost at API list prices</span>'}</div>
    <div class="card u-tc-table">${th}${body}${empty}${more}</div></section>`;
}

function usageDrawerHTML() {
  const status = Usage.status, health = status?.health || {};
  const word = { healthy: 'Healthy', degraded: 'Degraded', stopped: 'Stopped', disabled: 'Disabled', error: 'Unavailable' }[health.state] || 'Not loaded';
  const gap = health.historical_gap || {}, gaps = Object.values(gap).some((n) => n > 0);
  const gapText = gaps ? `Earlier writer sessions recorded ${[['dropped', 'dropped'], ['rejected', 'rejected'], ['writer_errors', 'writer errors']].filter(([k]) => gap[k]).map(([k, l]) => `${usageCount(gap[k])} ${l}`).join(', ')}. Totals for those periods may be low.` : '';
  return `<div class="scrim" data-usage-act="close-collection"></div>
  <aside class="drawer u-drawer" id="usage-drawer" role="dialog" aria-modal="true" aria-labelledby="usage-drawer-title">
    <div class="drawer-head"><div class="cell2 grow"><h2 id="usage-drawer-title">Collection</h2><span class="meta">Where usage records come from, and how fresh they are</span></div><button class="close-btn" type="button" data-usage-act="close-collection"><kbd class="kbd">Esc</kbd>Close</button></div>
    <div class="drawer-body">
      ${Usage.actionError ? `<p class="notice usage-message err" role="alert">${S.private ? 'The action failed. Retry or check server logs.' : ux(Usage.actionError)}</p>` : ''}
      <div class="block u-health"><span class="label">ANALYTICS HEALTH</span>
        <div class="statline"><span class="dot ${usageHealthDot(false)}"></span><span>${ux(word)}</span><span class="dim">· last commit ${ux(usageClock(health.last_commit_at_ms))}</span></div>
        ${health.message ? `<p class="meta">${S.private ? 'See server status for details' : ux(health.message)}</p>` : ''}
        <div class="figs u-figs">${[['queue_depth', 'Queued'], ['dropped', 'Dropped'], ['writer_errors', 'Writer errors'], ['rejected', 'Rejected']].map(([k, l]) => `<div><b>${usageCount(health[k])}</b><span>${l}</span></div>`).join('')}</div>
        ${usageDatabaseHTML(status?.database)}
        ${gaps || health.recovery_warning ? `<div class="notice warn"><span class="dot s7 warn"></span><span class="grow">${ux(gapText)}${health.recovery_warning ? `${gaps ? ' ' : ''}An earlier or concurrent writer may have left usage uncommitted.` : ''}</span></div>` : ''}
        <span class="meta">Proxy records count once they are committed to disk. Gaps and writer errors can make totals incomplete.</span></div>
      <div class="block"><span class="label">LOCAL HISTORY IMPORTS</span><span class="meta">Reads usage metadata from Claude Code or Codex history on this server and rescans about every 30 seconds. Each record keeps the time it happened, not the time it was imported. No transcript content is stored.</span>${['claude_code', 'codex'].map(usageImportHTML).join('')}</div>
      <div class="block"><span class="label">COLLECTORS</span><span class="meta">Send usage metadata from another machine. Each credential is shown once. Last contact is a health signal; last sync reports durable ingestion.</span>
        <form id="usage-enroll" class="u-inline"><input name="label" type="${S.private ? 'password' : 'text'}" value="${S.private ? '' : ux(Usage.collectorDraft)}" placeholder="${S.private ? 'Label hidden' : 'Label, e.g. Work laptop'}" aria-label="Collector label" maxlength="120" required autocomplete="off"${Usage.busy ? ' disabled' : ''}><button class="btn" type="submit"${Usage.busy ? ' disabled' : ''}>Enroll collector</button></form>
        ${usageCredentialHTML()}
        <div class="u-collectors">${(status?.collectors || []).map(usageCollectorHTML).join('') || '<p class="meta">No collectors enrolled.</p>'}</div></div>
    </div>
  </aside>`;
}
// The usage database on disk (null while it is off): its size, how fast it grows, the
// disk space left under it, and whether older records are being compressed.
function usageDatabaseHTML(db) {
  if (!db) return '';
  const figs = [[UsageTools.bytes(db.bytes), 'Database'], [UsageTools.bytes(db.growth_bytes_per_day), 'Growth a day'], [db.disk_free_bytes == null ? 'Unknown' : UsageTools.bytes(db.disk_free_bytes), 'Free disk']];
  return `<div class="figs u-figs u-db">${figs.map(([v, l]) => `<div><b>${ux(v)}</b><span>${l}</span></div>`).join('')}</div>${db.compressing ? '<p class="meta" role="status">Compressing older records…</p>' : ''}`;
}
function usageImportHTML(source) {
  const roots = (Usage.status?.imports || []).map((item, index) => ({ item, index })).filter(({ item }) => item.source === source);
  const enabled = roots.some(({ item }) => item.enabled);
  const word = (item) => (!item.enabled ? 'Disabled' : item.state === 'attention' ? 'Needs attention' : item.last_scan_at_ms ? 'Enabled · up to date' : 'Enabled · scanning');
  const list = roots.map(({ item, index }) => `<div class="u-import-root"><div class="statline"><span class="dot s7 ${item.enabled ? item.state === 'attention' ? 'warn' : 'ok' : 'idle'}"></span><span class="u-strong">${ux(usageSource(source))}</span><span class="grow"></span><span class="fg2">${ux(word(item))}</span></div>
    <span class="mono fg2 u-path">${usageSensitive(item.root)}</span>
    <span class="meta">${item.imported ? `${usageCount(item.imported)} imported · ${usageCount(item.duplicate)} duplicates · ${usageCount(item.skipped)} skipped · ${usageCount(item.unsupported)} unsupported · ${usageCount(item.failed)} failed` : 'Nothing imported yet'} · last scan ${ux(usageTime(item.last_scan_at_ms))}</span>
    ${item.last_error ? `<span class="err meta">${S.private ? 'Import error; details hidden' : ux(item.last_error)}</span>` : ''}
    <div class="u-actions">${usageButton('scan', 'Scan now', `data-source="${source}"`, !item.enabled, 'btn sm')}${usageButton('toggle-import', item.enabled ? 'Disable' : 'Enable', `data-index="${index}"`, false, 'btn sm')}</div></div>`).join('');
  const form = `<form data-usage-import="${source}" class="u-inline"><input name="root" type="${S.private ? 'password' : 'text'}" autocomplete="off" spellcheck="false" aria-label="${ux(usageSource(source))} root on the server" placeholder="${S.private ? 'Path hidden · enter a root' : source === 'claude_code' ? '/path/to/.claude/projects' : '/path/to/.codex/sessions'}" value="${S.private ? '' : ux(Usage.roots[source] || '')}"${Usage.busy ? ' disabled' : ''}><button class="btn" type="submit"${Usage.busy ? ' disabled' : ''}>Enable import</button></form>`;
  if (!roots.length) return `<div class="u-import"><div class="statline"><span class="dot s7 idle"></span><span class="u-strong">${ux(usageSource(source))}</span><span class="grow"></span><span class="fg2">Not set up</span></div>${form}</div>`;
  return `<div class="u-import">${list}${enabled ? '' : form}</div>`;
}
function usageCollectorHTML(c, i) {
  const confirming = Usage.confirm && Usage.confirm.id === c.id ? Usage.confirm : null;
  const state = c.revoked ? ['Revoked', 'idle', 'fg2'] : ({ synced: ['Connected', 'ok', 'fg2'], contacted: ['Contacted, not synced', 'ok', 'fg2'], offline: ['Stale', 'warn', 'warn'], enrolled: ['Not yet contacted', 'idle', 'fg2'] })[c.state] || [c.state || 'Unknown', 'idle', 'fg2'];
  const range = c.time_start_ms ? ` · ${ux(usageTime(c.time_start_ms))} → ${ux(usageTime(c.time_end_ms))}` : '';
  const name = usageSensitive(c.label || c.id);
  return `<article class="u-collector${c.revoked ? ' faded' : ''}"><div class="statline"><span class="dot s7 ${state[1]}"></span><span class="u-strong">${name}</span><span class="grow"></span><span class="${state[2]}">${ux(state[0])}</span></div>
    <span class="meta">Last contact ${ux(usageTime(c.last_contact_at_ms))} · last sync ${ux(usageTime(c.last_sync_at_ms))} · ${usageCount(c.pending)} pending</span>
    <span class="meta">${(c.covered_sources || []).map(usageSource).map(ux).join(', ') || 'No sources reported yet'}${range}</span>
    ${(c.progress || []).map((p) => `<span class="meta">${ux(usageSource(p.source))} · ${ux(p.state)} · imported ${usageCount(p.imported)} · duplicates ${usageCount(p.duplicate)} · skipped ${usageCount(p.skipped)} · unsupported ${usageCount(p.unsupported)} · failed ${usageCount(p.failed)} · last scan ${ux(usageTime(p.last_scan_at_ms))} · total historical coverage unknown</span>`).join('')}
    ${!c.revoked && !confirming ? `<div class="u-actions">${usageButton('rotate', 'Rotate credential', `data-index="${i}"`, false, 'btn sm')}${usageButton('revoke', 'Revoke', `data-index="${i}"`, false, 'btn sm danger')}</div>` : ''}
    ${confirming ? `<div class="u-confirm" role="alert"><span>${confirming.action === 'revoke' ? `Revoke ${name}? New uploads will be rejected. Stored records stay.` : `Rotate the credential for ${name}? The old one stops working right away. Update the collector with the replacement.`}</span><div class="u-actions">${usageButton('confirm', confirming.action === 'revoke' ? 'Confirm revoke' : 'Confirm rotation', '', false, `btn sm${confirming.action === 'revoke' ? ' u-danger' : ''}`)}${usageButton('cancel', 'Cancel', '', false, 'btn sm ghost')}</div></div>` : ''}
  </article>`;
}
function usageCredentialHTML() {
  if (!Usage.credential) return '';
  return `<div class="u-credential" role="status"><span class="u-strong">One-time credential${Usage.credentialFor ? ` for ${usageSensitive(Usage.credentialFor)}` : ''}</span><span class="meta">Copy it into the collector now. It disappears when you dismiss this.</span><span class="mono usage-secret">${S.private ? '••••••••••••' : ux(Usage.credential)}</span><div class="u-actions">${usageButton('copy-credential', 'Copy', '', false, 'btn sm')}${usageButton('dismiss-credential', 'I saved it', '', false, 'btn sm')}</div></div>`;
}

function usageFocusSelector(el) {
  if (!el || !el.closest('.usage-page')) return null;
  if (el.dataset.usageDay) return `[data-usage-day="${el.dataset.usageDay}"]`;
  if (el.dataset.usageAct) return `[data-usage-act="${el.dataset.usageAct}"]${el.dataset.value ? `[data-value="${el.dataset.value}"]` : ''}${el.dataset.source ? `[data-source="${el.dataset.source}"]` : ''}${el.dataset.index ? `[data-index="${el.dataset.index}"]` : ''}`;
  const form = el.closest('form');
  if (form && el.name) return `${form.id ? `#${form.id}` : `[data-usage-import="${form.dataset.usageImport}"]`} [name="${el.name}"]`;
  return null;
}
// The markup each section last received; a section whose markup is unchanged is left alone.
const usagePainted = new WeakMap();
// Repaints the page's sections (all of them unless named). The first call on a fresh
// route draws the whole page; later calls patch only the sections whose markup changed,
// keeping focus, the drawer's scroll and hover where they were.
function usageRender(focusSelector, parts = Object.keys(USAGE_PARTS)) {
  if (S.route !== 'usage' || S.locked) return;
  const active = document.activeElement;
  const selector = focusSelector || usageFocusSelector(active) || Usage.focus;
  Usage.focus = selector;
  const results = document.getElementById('usage-results');
  const scroll = document.querySelector('#usage-drawer .drawer-body')?.scrollTop;
  if (!results) {
    render();
    if (scroll) { const body = document.querySelector('#usage-drawer .drawer-body'); if (body) body.scrollTop = scroll; }
    if (selector) document.querySelector(selector)?.focus({ preventScroll: true });
    return;
  }
  let lost = !active || active === document.body;
  for (const name of parts) {
    const el = document.getElementById(`usage-part-${name}`);
    if (!el) continue;
    const html = USAGE_PARTS[name]();
    if (usagePainted.get(el) === html) continue;
    if (el.contains(active)) lost = true;
    el.innerHTML = html;
    usagePainted.set(el, html);
  }
  results.setAttribute('aria-busy', String(Usage.loading));
  if (scroll) { const body = document.querySelector('#usage-drawer .drawer-body'); if (body && body.scrollTop !== scroll) body.scrollTop = scroll; }
  // Focus goes back where it was when a repaint took it away, or where the caller asked.
  if (selector && (focusSelector || lost || !document.contains(active))) document.querySelector(selector)?.focus({ preventScroll: true });
}
// Hover and stacking only touch the chart; the rest of the page keeps its DOM.
function usageRenderChart() {
  const el = typeof document !== 'undefined' && document.getElementById('usage-chart');
  if (!el) return usageRender();
  const focus = usageFocusSelector(document.activeElement);
  el.innerHTML = usageChartHTML();
  // The cost section's markup now differs from what it last received as a whole.
  const cost = document.getElementById('usage-part-cost');
  if (cost) usagePainted.delete(cost);
  if (focus) el.querySelector(focus)?.focus({ preventScroll: true });
}
// Account changes reach account names in the filter and the breakdown.
function usageAccountsChanged() {
  usageRender(undefined, ['head', 'breakdown']);
}
// What each read feeds.
const USAGE_READ_PARTS = {
  summary: ['head', 'notices', 'cost', 'breakdown', 'clients'],
  status: ['head', 'notices', 'cost', 'drawer'],
};
const USAGE_LOAD_PARTS = ['head', 'notices', 'cost', 'breakdown', 'clients'];
const usageQuery = () => UsageTools.query(Usage.range, Usage.filters, Usage.timezone);
function usageSelectStack(summary) {
  const combined = summary?.combined;
  if (combined?.trends?.[Usage.stack]) {
    combined.stack = Usage.stack; combined.trend = combined.trends[Usage.stack];
  }
  return summary;
}
const usageReads = new Map();
function usageCancelRead(field) {
  const controller = usageReads.get(field);
  if (!controller) return;
  usageReads.delete(field);
  controller.abort();
}
function usageCancelReads() {
  for (const field of usageReads.keys()) usageCancelRead(field);
}
function usageSuspendReads() {
  ++Usage.sequence;
  usageCancelReads();
  Usage.loading = false; Usage.loaded = false;
}
async function usageRead(field, path, sequence) {
  usageCancelRead(field);
  const controller = new AbortController();
  usageReads.set(field, controller);
  // Keep both guards: a transport may finish even after its signal is aborted.
  const current = () => !controller.signal.aborted && usageReads.get(field) === controller && sequence === Usage.sequence;
  try {
    const value = await api(path, { signal: controller.signal });
    if (current()) Usage[field] = field === 'summary' ? usageSelectStack(value) : value;
  } catch (e) {
    if (current() && e.name !== 'AbortError') {
      if (field !== 'status') Usage[field] = null;
      Usage.error = e.message === 'locked' ? 'Management authentication required.' : e.message;
    }
  } finally {
    const active = current();
    if (usageReads.get(field) === controller) usageReads.delete(field);
    if (active) usageRender(undefined, USAGE_READ_PARTS[field]);
  }
}
async function usageLoad() {
  const u = Usage, sequence = ++u.sequence;
  usageCancelReads();
  u.loading = true; u.error = ''; u.loaded = true;
  usageRender(undefined, USAGE_LOAD_PARTS);
  try {
    const query = usageQuery();
    // Each section renders as soon as its own read completes.
    await Promise.allSettled([
      usageRead('summary', `/usage/dashboard?${query}&stack=${u.stack}`, sequence),
      usageRead('status', '/usage/status', sequence),
    ]);
  } catch (e) { if (sequence === u.sequence) u.error = e.message === 'locked' ? 'Management authentication required.' : e.message; }
  finally { if (sequence === u.sequence) { u.loading = false; usageRender(undefined, USAGE_LOAD_PARTS); } }
}
// Both chart groupings arrive in the same snapshot; toggling needs no round trip.
function usageLoadStack() {
  usageSelectStack(Usage.summary); usageRenderChart();
}
// Range and filter changes read everything again.
function usageReload() { Usage.hoverDay = null; Usage.summary = null; return usageLoad(); }
async function usageMutate(path, body, after) {
  Usage.busy = true; Usage.actionError = ''; usageRender();
  try {
    const r = await api(path, { method: 'POST', ...(body ? { body: JSON.stringify(body) } : {}) });
    if (after) after(r);
    await usageLoad();
  } catch (e) { Usage.actionError = e.message; }
  finally { Usage.busy = false; usageRender(); }
}
let usageDrawerOpener = null;
function usageOpenDrawer() {
  usageDrawerOpener = document.activeElement;
  Usage.drawerOpen = true; Usage.actionError = '';
  document.body.style.overflow = 'hidden';
  usageRender('#usage-drawer [data-usage-act="close-collection"]');
}
function usageCloseDrawer() {
  if (!Usage.drawerOpen) return;
  Usage.drawerOpen = false; Usage.confirm = null; Usage.focus = null;
  document.body.style.overflow = '';
  usageRender(usageDrawerOpener?.dataset?.usageAct ? `[data-usage-act="${usageDrawerOpener.dataset.usageAct}"]` : '[data-usage-act="open-collection"]');
  usageDrawerOpener = null;
}
function bindUsage() {
  if (usageShell && typeof document !== 'undefined' && document.getElementById) {
    for (const [name, html] of Object.entries(usageShell)) {
      const el = document.getElementById(`usage-part-${name}`);
      if (el) usagePainted.set(el, html);
    }
  }
  usageShell = null;
  if (!Usage.loaded && !Usage.loading) usageLoad();
}

if (typeof document !== 'undefined') {
  document.addEventListener('input', (event) => {
    const form = event.target.closest('[data-usage-import]');
    if (form && event.target.name === 'root') Usage.roots[form.dataset.usageImport] = event.target.value;
    if (event.target.closest('#usage-enroll')) Usage.collectorDraft = event.target.value;
  });
  document.addEventListener('submit', (event) => {
    const form = event.target;
    if (!form.matches('#usage-range, #usage-enroll, [data-usage-import]')) return;
    event.preventDefault();
    if (Usage.busy) return;
    const values = new FormData(form);
    if (form.id === 'usage-range') {
      try {
        const start = values.get('start'), through = values.get('through'), today = usageToday(), min = UsageTools.shift(today, -365);
        UsageTools.shift(start, 0); UsageTools.shift(through, 0);
        if (through < start) throw new Error('The end date must be on or after the start date.');
        if (start < min || through > today) throw new Error(`Choose dates between ${UsageTools.shortDate(min)}, ${min.slice(0, 4)} and ${UsageTools.shortDate(today)}, ${today.slice(0, 4)}.`);
        Usage.range = { start, through, end: UsageTools.shift(through, 1) }; Usage.customError = ''; usageReload();
      } catch (e) { Usage.customError = e.message; usageRender(); }
    } else if (form.id === 'usage-enroll') {
      const label = String(values.get('label') || Usage.collectorDraft).trim();
      if (!label) return;
      if (Usage.credential) { Usage.actionError = 'Save and dismiss the current credential before enrolling another collector.'; usageRender(); return; }
      usageMutate('/usage/collectors', { label }, (r) => { Usage.credential = r.credential || null; Usage.credentialFor = r.collector?.label || label; Usage.collectorDraft = ''; });
    } else {
      const source = form.dataset.usageImport;
      const root = String(values.get('root') || Usage.roots[source] || '').trim();
      if (!root) { Usage.actionError = 'Enter a history root on the server.'; usageRender(); return; }
      usageMutate('/usage/imports', { source, root, enabled: true }, () => { delete Usage.roots[source]; });
    }
  });
  const hoverDay = (day) => {
    if (S.route !== 'usage' || Usage.hoverDay === day) return;
    Usage.hoverDay = day; usageRenderChart();
  };
  document.addEventListener('mouseover', (event) => {
    if (usagePhone()) return;
    const bar = event.target.closest?.('[data-usage-day]');
    if (bar) hoverDay(bar.dataset.usageDay);
  });
  document.addEventListener('mouseout', (event) => {
    const bars = event.target.closest?.('[data-usage-bars]');
    if (bars && !bars.contains(event.relatedTarget) && !usagePhone()) hoverDay(null);
  });
  document.addEventListener('focusin', (event) => {
    const bar = event.target.closest?.('[data-usage-day]');
    if (bar && !usagePhone()) hoverDay(bar.dataset.usageDay);
  });
  document.addEventListener('click', (event) => {
    const bar = event.target.closest('[data-usage-day]');
    if (bar) return hoverDay(Usage.hoverDay === bar.dataset.usageDay && usagePhone() ? null : bar.dataset.usageDay);
    const el = event.target.closest('[data-usage-act]');
    if (!el || el.disabled) return;
    const act = el.dataset.usageAct, index = Number(el.dataset.index);
    if (act === 'refresh') { if (Usage.preset !== 'custom') Usage.range = UsageTools.range(Number(Usage.preset)); return usageLoad(); }
    if (act === 'range') {
      Usage.preset = el.dataset.value; Usage.customError = '';
      if (Usage.preset === 'custom') return usageRender('#usage-range [name="start"]');
      Usage.range = UsageTools.range(Number(Usage.preset)); return usageReload();
    }
    if (act === 'stack') { if (Usage.stack !== el.dataset.value) { Usage.stack = el.dataset.value; usageRenderChart(); usageLoadStack(); } return; }
    if (act === 'metric') { Usage.metric = el.dataset.value; return usageRenderChart(); }
    if (act === 'dim') { Usage.dim = el.dataset.value; Usage.showAll = false; return usageRender(); }
    if (act === 'show-all') { Usage.showAll = !Usage.showAll; return usageRender(); }
    if (act === 'clients-all') { Usage.clientsAll = !Usage.clientsAll; return usageRender(undefined, ['clients']); }
    if (act === 'filter-row') {
      const id = Usage.rows[index];
      if (id == null) return;
      Usage.filters[Usage.dim] = Usage.filters[Usage.dim] === id ? '' : id;
      return usageReload();
    }
    if (act === 'clear-filter') { Usage.filters[el.dataset.value] = ''; return usageReload(); }
    if (act === 'clear-filters') { Object.keys(Usage.filters).forEach((k) => { Usage.filters[k] = ''; }); return usageReload(); }
    if (act === 'open-collection') return usageOpenDrawer();
    if (act === 'close-collection') return usageCloseDrawer();
    if (act === 'scan') return usageMutate('/usage/imports/scan', { source: el.dataset.source });
    if (act === 'toggle-import') {
      const item = Usage.status?.imports?.[index];
      if (item) return usageMutate('/usage/imports', { source: item.source, root: item.root, enabled: !item.enabled });
    }
    if (act === 'revoke' || act === 'rotate') {
      if (Usage.credential && act === 'rotate') { Usage.actionError = 'Save and dismiss the current credential before rotating another.'; return usageRender(); }
      const collector = Usage.status?.collectors?.[index];
      if (collector) Usage.confirm = { action: act, id: collector.id, label: collector.label || collector.id };
      return usageRender('[data-usage-act="confirm"]');
    }
    if (act === 'cancel') { Usage.confirm = null; return usageRender('#usage-enroll input'); }
    if (act === 'confirm' && Usage.confirm) {
      const c = Usage.confirm; Usage.confirm = null;
      return usageMutate(`/usage/collectors/${encodeURIComponent(c.id)}/${c.action}`, null, (r) => { if (c.action === 'rotate') { Usage.credential = r.credential || null; Usage.credentialFor = c.label; } });
    }
    if (act === 'dismiss-credential') { Usage.credential = null; Usage.credentialFor = null; return usageRender('#usage-enroll input'); }
    if (act === 'copy-credential') navigator.clipboard.writeText(Usage.credential).then(() => toast('Enrollment credential copied')).catch(() => { Usage.actionError = 'Clipboard unavailable. Turn privacy off and select the credential to copy it.'; usageRender(); });
  });
  document.addEventListener('keydown', (event) => {
    if (!Usage.drawerOpen || S.route !== 'usage' || S.palette) return;
    if (event.key === 'Escape') { event.preventDefault(); usageCloseDrawer(); }
    else if (event.key === 'Tab' && typeof trapFocus === 'function') trapFocus(event, document.getElementById('usage-drawer'));
  });
  addEventListener('hashchange', () => {
    if (/^#\/?usage(?:[/?]|$)/.test(location.hash)) return;
    usageSuspendReads();
    if (Usage.drawerOpen) { Usage.drawerOpen = false; Usage.confirm = null; document.body.style.overflow = ''; }
  });
  addEventListener('pagehide', usageSuspendReads);
  addEventListener('pageshow', (event) => {
    if (event.persisted && S.route === 'usage' && !S.locked) bindUsage();
  });
}

// Refresh persistent analytics while the page is open; never interrupts a source action.
if (typeof document !== 'undefined') setInterval(() => {
  if (S.route !== 'usage' || S.locked || Usage.loading || Usage.busy || document.hidden) return;
  if (Usage.preset !== 'custom') Usage.range = UsageTools.range(Number(Usage.preset));
  usageLoad();
}, 60000);
