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
    for (const k of ['provider', 'model', 'account', 'client', 'entrypoint']) if (filters[k]) q.set(k, filters[k]);
    return q.toString();
  };
  const privateLabel = (value, hidden) => hidden && value != null && value !== '' ? '••••••' : String(value ?? 'Unknown');
  // Totals: cents above a dollar, four places below so small estimates stay visible.
  const currency = [2, 4].map((digits) => new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits }));
  const numbers = new Intl.NumberFormat('en-US');
  const count = (value) => value == null ? 'Unknown' : numbers.format(value);
  const dollars = (nanos) => {
    if (nanos == null || !Number.isFinite(Number(nanos))) return 'Unpriced';
    const v = Number(nanos) / 1e9, small = v !== 0 && Math.abs(v) < 1;
    return currency[small ? 1 : 0].format(v);
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
  // Reuse each format in the page's current timezone. Keeping only the latest
  // timezone per style bounds the cache even when a caller changes timezones.
  const timeStyles = {
    date: ['en-CA', { year: 'numeric', month: '2-digit', day: '2-digit' }],
    timestamp: [undefined, { year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: 'numeric', second: 'numeric', hourCycle: 'h23' }],
    clock: ['en-GB', { hour: 'numeric', minute: 'numeric', second: 'numeric', hour12: false }],
    weekday: ['en-GB', { weekday: 'short', hour: '2-digit', minute: '2-digit', hour12: false }],
    hourMinute: ['en-GB', { hour: '2-digit', minute: '2-digit', hour12: false }],
    weekdayName: ['en-GB', { weekday: 'short' }],
    monthDay: ['en-US', { month: 'short', day: 'numeric' }],
    dayDate: ['en-GB', { weekday: 'short', day: 'numeric', month: 'short' }],
    reset: ['en-GB', { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false, timeZoneName: 'short' }],
  };
  const timeFormats = new Map();
  const timeFormat = (style, timezone) => {
    let cached = timeFormats.get(style);
    if (!cached || cached.timezone !== timezone) {
      const [locale, options] = timeStyles[style];
      cached = { timezone, formatter: new Intl.DateTimeFormat(locale, { ...options, timeZone: timezone }) };
      timeFormats.set(style, cached);
    }
    return cached.formatter;
  };
  const timeText = (value, timezone, style, invalid = 'Invalid Date') => {
    const at = new Date(value);
    return Number.isFinite(at.getTime()) ? timeFormat(style, timezone).format(at) : invalid;
  };
  // Read the browser's current zone at render/timer boundaries, rather than
  // constructing a default formatter for every request row or meter tooltip.
  let localTimezone = null;
  const refreshLocalTimezone = () => {
    const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const changed = timezone !== localTimezone;
    localTimezone = timezone;
    return changed;
  };
  const localTimeText = (value, style) => {
    if (localTimezone === null) refreshLocalTimezone();
    return timeText(value, localTimezone, style);
  };
  // Local calendar date of an instant in the page's timezone.
  const localDate = (ms, timezone) => timeFormat('date', timezone).format(new Date(ms));
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
  // BY MONTH: the calendar months a range touches ("2026-10"), and the trend summed into them.
  const months = (days) => [...new Set(days.map((d) => d.slice(0, 7)))];
  const byMonth = (trend) => (trend || []).map((r) => ({ ...r, date: String(r.date).slice(0, 7) }));
  const monthLabel = (key) => { const p = parts(`${key}-01`); return `${p.m} ${p.y}`; };
  return { OTHER, escape, date, shift, range, dates, query, privateLabel, count, dollars, compact, chart, localDate, beforeProxy, timeText, localTimeText, refreshLocalTimezone, parts, dayLabel, shortDate, months, byMonth, monthLabel, bytes };
})();
if (typeof module !== 'undefined' && module.exports) module.exports = UsageTools;

const Usage = {
  preset: '7', range: UsageTools.range(7), timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
  filters: { provider: '', model: '', account: '', client: '', entrypoint: '' },
  stack: 'provider', metric: 'cost', period: '', hoverDay: null, dim: 'model', showAll: false, clientsAll: false,
  drawerOpen: false, customError: '',
  summary: null, status: null, entrypoints: null, speed: null,
  // Output speed: the provider chip, the sorted column, the charted models (key → colour slot) and the chart's metric.
  speedProvider: '', speedSort: 'rate', speedPicked: null, speedMetric: 'rate', speedAll: false,
  loading: false, loaded: false, error: '', actionError: '', busy: false,
  sequence: 0, credential: null, confirm: null, roots: {}, collectorDraft: '', rows: [],
};
const ux = UsageTools.escape;
const usageCount = UsageTools.count;
const usageSensitive = (value) => ux(UsageTools.privateLabel(value, S.private));
const usageSource = (source) => ({ proxy: 'Proxy', claude_code: 'Claude Code history', codex: 'Codex history' }[source] || source || 'Unknown source');
const usageTime = (value) => value == null || value === 0 ? 'Not reported' : UsageTools.timeText(value, Usage.timezone, 'timestamp', 'Not reported');
const usageClock = (value) => value == null || value === 0 ? 'Not reported' : UsageTools.timeText(value, Usage.timezone, 'clock');
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
const usageRangeLabel = () => Usage.preset === '1' ? 'Today' : Usage.preset === '7' ? 'Last 7 days' : Usage.preset === '30' ? 'Last 30 days' : Usage.preset === '365' ? 'Last 12 months' : `${UsageTools.shortDate(Usage.range.start)} – ${UsageTools.shortDate(Usage.range.through)}`;
// Bars by month once a range spans more than a month: chosen, or by month past three.
const usagePeriod = (days = UsageTools.dates(Usage.range).length) => days <= 31 ? 'day' : Usage.period || (days > 92 ? 'month' : 'day');
// How far back the custom range reaches: the server reads ranges of up to 3660 days.
const USAGE_HISTORY_DAYS = 3650;
const usageBarLabel = (key) => key.length === 7 ? UsageTools.monthLabel(key) : UsageTools.dayLabel(key);
// The first local day whose individual records are still stored, when retention
// has deleted older ones and kept their totals; null otherwise.
const usageRecordsFrom = () => {
  const ms = usageCombined().summarized_before_ms;
  return ms == null || !Number.isFinite(Number(ms)) ? null : UsageTools.localDate(Number(ms), Usage.timezone);
};
const usageDayCost = (day) => day.priced ? UsageTools.dollars(day.cost) : day.calls ? 'Unpriced' : '$0.00';

// The page is drawn once; after that each section is its own patch target (a
// display:contents wrapper, so the layout is unchanged) and repaints only when its
// markup changes. A read repaints the sections it feeds.
const USAGE_PARTS = {
  head: () => usageHeadHTML(),
  notices: () => usageNoticesHTML(),
  cost: () => (Usage.summary ? usageCostHTML() : !Usage.loading && !Usage.error ? '<div class="card pad"><h2 class="h-sec">No usage loaded</h2><p class="meta">Refresh to read persisted metadata.</p></div>' : ''),
  entrypoints: () => (Usage.summary && Usage.entrypoints ? usageEntrypointsHTML() : ''),
  speed: () => (Usage.summary && Usage.speed ? usageSpeedHTML() : ''),
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
    <div id="usage-results" class="usage-results" aria-busy="${Usage.loading}">${usagePart('cost')}${usagePart('entrypoints')}${usagePart('speed')}${usagePart('breakdown')}
    ${usagePart('clients')}</div>
    ${usagePart('drawer')}
  </div>`;
}
function usageNoticesHTML() {
  const u = Usage;
  const reasons = u.summary?.combined?.totals?.unpriced_reasons || {};
  const missing = Number(reasons.missing_usage || 0) + Number(reasons.incomplete_usage || 0);
  return `${u.error ? `<div class="notice usage-message err" role="alert"><span class="grow">${S.private ? 'Usage could not be loaded. Try refreshing.' : ux(u.error)}</span>${usageButton('refresh', 'Retry', '', false, 'btn sm')}</div>` : ''}
    ${u.actionError && !u.drawerOpen ? `<p class="notice usage-message err" role="alert">${S.private ? 'The action failed. Retry or check server logs.' : ux(u.actionError)}</p>` : ''}
    ${missing ? `<p class="usage-message meta">${usageCount(missing)} calls have missing or incomplete token usage, often after cancellation or upstream errors. Their costs remain unknown and are excluded from estimates. Adding model prices cannot recover missing usage.</p>` : ''}
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
  const presets = `<div class="seg u-range" role="group" aria-label="Date range">${[['1', 'Today'], ['7', '7 days'], ['30', '30 days'], ['365', '12 months'], ['custom', 'Custom']].map(([v, label]) => `<button type="button" data-usage-act="range" data-value="${v}" aria-pressed="${u.preset === v}">${label}</button>`).join('')}</div>`;
  const collection = `<button class="btn u-collection" type="button" data-usage-act="open-collection" aria-haspopup="dialog"><span class="dot s7 ${usageHealthDot()}"></span>Collection</button>`;
  const refresh = `<button class="icon-btn row" type="button" data-usage-act="refresh" title="Refresh · updates every minute" aria-label="Refresh usage"${u.loading ? ' disabled' : ''}><svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><path d="M11.5 7a4.5 4.5 0 1 1-1.3-3.2"/><path d="M11.5 1.8v2.6H8.9"/></svg></button>`;
  const today = usageToday(), min = UsageTools.shift(today, -USAGE_HISTORY_DAYS);
  const custom = u.preset === 'custom' ? `<form id="usage-range" class="u-custom"><label class="u-field"><span>From</span><input type="date" name="start" value="${ux(u.range.start)}" min="${min}" max="${today}" required></label><label class="u-field"><span>Through</span><input type="date" name="through" value="${ux(u.range.through)}" min="${min}" max="${today}" required></label><button class="btn" type="submit">Apply dates</button>${u.customError ? `<span class="u-custom-err" role="alert">${ux(u.customError)}</span>` : ''}</form>` : '';
  const labels = { model: 'Model', account: 'Account', provider: 'Provider', client: 'Client', entrypoint: 'Entrypoint' };
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
  if (t.unpriced_reasons) {
    const labels = { missing_usage: 'missing usage', incomplete_usage: 'incomplete usage', invalid_usage: 'invalid usage', unknown_model: 'without a model rate', unsupported_pricing: 'with unsupported pricing' };
    let classified = 0;
    for (const [key, label] of Object.entries(labels)) {
      const count = Number(t.unpriced_reasons[key] || 0);
      classified += count;
      if (count) parts.push(`${usageCount(count)} ${label}`);
    }
    if (unpriced > classified) parts.push(`${usageCount(unpriced - classified)} unpriced`);
  } else {
    if (t.unpriced) parts.push(`${usageCount(t.unpriced)} unpriced`);
  }
  if (t.partial) parts.push(`${usageCount(t.partial)} partial`);
  if (!t.unpriced_reasons && t.missing_usage) parts.push(`${usageCount(t.missing_usage)} missing usage`);
  if (t.pricing_partial) parts.push(`${usageCount(t.pricing_partial)} with assumptions`);
  if (t.aggregation_overflow) parts.push('a total exceeds the supported range, amount unknown');
  return parts.join(' · ');
}
function usageStaleLine() {
  if (Usage.range.through !== usageToday()) return '';
  const stale = (Usage.status?.collectors || []).filter((c) => !c.revoked && c.state === 'offline');
  return stale.map((c) => {
    const sources = (c.covered_sources || []).map((s) => usageSource(s).replace(' history', '')).join(' and ') || 'imported';
    const since = c.last_sync_at_ms ? UsageTools.timeText(c.last_sync_at_ms, Usage.timezone, 'weekday').replace(',', '') : null;
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
  const stale = usageStaleLine(), lifetimes = usageCacheLifetimes(tokens), from = usageRecordsFrom();
  const kept = from && u.range.start < from ? `Before ${UsageTools.dayLabel(from)}, calls are kept as totals: retention deleted their individual records and kept their cost and tokens` : '';
  return `<section class="card u-cost" aria-labelledby="usage-cost-label">
    <div class="u-cost-top"><div class="u-total"><span class="label" id="usage-cost-label">ESTIMATED COST · ${ux(word)}</span><span class="u-total-v">${t.aggregation_overflow && cost == null ? 'Out of range' : ux(UsageTools.dollars(cost ?? (t.observations ? null : 0)))}</span><span class="u-total-sub">${sub}</span></div>
    <dl class="u-stats">${stats.map(([v, l]) => `<div><dd>${ux(v)}</dd><dt>${ux(l)}</dt></div>`).join('')}</dl></div>
    ${days.length > 1 ? `<div id="usage-chart" class="u-chart">${usageChartHTML()}</div>` : ''}
    ${lifetimes ? `<div class="u-foot"><span class="fg2">${ux(lifetimes)}</span></div>` : ''}
    ${kept ? `<div class="u-foot"><span class="fg2">${ux(kept)}</span></div>` : ''}
    <div class="u-foot"><span class="fg2">${ux(usageCoverageLine(t))}</span>${stale ? `<span class="warn">${stale}</span>` : ''}${phone ? '' : `<span class="grow"></span><span>Unknown usage and missing prices are left out, never counted as zero · catalogue ${ux(s.pricing?.version || u.status?.pricing?.version || 'unknown')}</span>`}</div>
  </section>`;
}

// A snapshot's trend is immutable; hover, focus and section repaints reuse its
// aggregation. Keep only the current chart so old snapshots can be reclaimed.
let usageChartCache = null;
const usageEmptyTrend = [];
function usageChartData() {
  const trend = usageCombined().trend || usageEmptyTrend;
  const { start, end } = Usage.range, metric = Usage.metric, days = UsageTools.dates(Usage.range), period = usagePeriod(days.length);
  const shade = typeof document !== 'undefined' && document.documentElement.dataset.shade;
  const prev = usageChartCache;
  if (prev && prev.trend === trend && prev.start === start && prev.end === end && prev.metric === metric && prev.period === period && prev.shade === shade) return prev.chart;
  const chart = period === 'month' ? UsageTools.chart(UsageTools.byMonth(trend), UsageTools.months(days), metric) : UsageTools.chart(trend, days, metric);
  chart.period = period;
  usageChartCache = { trend, start, end, metric, period, shade, chart };
  return chart;
}
function usageChartHTML() {
  const u = Usage, phone = usagePhone(), c = usageChartData(), days = c.series.map((d) => d.date);
  const calls = u.metric === 'calls', showVals = days.length <= 10 && !phone, today = usageToday();
  const hover = days.indexOf(u.hoverDay);
  const seg = (name, opts, act, label) => `<div class="seg u-seg" role="group" aria-label="${label}">${opts.map(([v, l]) => `<button type="button" data-usage-act="${act}" data-value="${v}" aria-pressed="${u[name] === v}">${l}</button>`).join('')}</div>`;
  const legend = c.keys.map((k, i) => `<span><i style="background:${c.colors[i]}"></i>${ux(usageStack(k))}</span>`).join('');
  const bars = c.series.map((d, i) => {
    const h = c.max > 0 ? Math.max(d.total > 0 ? 1.5 : 0, d.total / c.max * (showVals ? 84 : 100)) : 0;
    const value = calls ? UsageTools.compact(d.total) : d.priced || !d.calls ? usageDollarsShort(d.total) : 'Unpriced';
    const segs = d.parts.map((v, k) => (v > 0 ? `<i style="flex:${Math.max(1, Math.round(v / d.total * 1000))} 1 0px;background:${c.colors[k]}"></i>` : '')).join('');
    const description = calls ? `${usageCount(d.calls)} calls` : `${usageDayCost(d)} · ${usageCount(d.calls - d.unpriced)} priced calls${d.unpriced ? ` · ${usageCount(d.unpriced)} unpriced` : ''}`;
    return `<button type="button" class="u-bar${hover >= 0 && hover !== i ? ' dim' : ''}${hover === i ? ' on' : ''}" data-usage-day="${d.date}" aria-label="${ux(usageBarLabel(d.date))}: ${ux(description)}">${showVals ? `<span class="u-bar-v">${ux(value)}</span>` : ''}<span class="u-bar-s" style="height:${h.toFixed(2)}%">${segs}</span></button>`;
  }).join('');
  const months = c.period === 'month', current = months ? today.slice(0, 7) : today;
  const labels = c.series.map((d, i) => {
    const p = UsageTools.parts(months ? `${d.date}-01` : d.date);
    // A month names its year at the first bar and every January.
    const text = months ? (i === 0 || p.m === 'Jan' ? `${p.m} '${String(p.y).slice(2)}` : p.m) : days.length <= 10 ? (phone ? p.w.slice(0, 2) : `${p.w} ${p.d}`) : p.monday ? `${p.m} ${p.d}` : '';
    return `<span${d.date === current ? ' class="today"' : ''}>${ux(text)}</span>`;
  }).join('');
  const period = UsageTools.dates(u.range).length > 31 ? `<div class="seg u-seg" role="group" aria-label="Bar per">${[['day', 'Day'], ['month', 'Month']].map(([v, l]) => `<button type="button" data-usage-act="period" data-value="${v}" aria-pressed="${c.period === v}">${l}</button>`).join('')}</div>` : '';
  return `<div class="u-chart-head"><span class="label">BY ${months ? 'MONTH' : 'DAY'}</span>${calls ? '' : '<span class="meta u-chart-scope">Priced calls only</span>'}<span class="grow"></span>${period}${seg('stack', [['provider', 'Provider'], ['model', 'Model']], 'stack', 'Stack bars by')}${seg('metric', [['cost', 'Cost'], ['calls', 'Calls']], 'metric', 'Bar value')}</div>
    ${legend ? `<div class="u-legend">${legend}</div>` : ''}
    <div class="u-bars${days.length > 20 ? ' dense' : ''}" data-usage-bars>${bars}</div>
    <div class="u-days${days.length > 20 ? ' dense' : ''}" aria-hidden="true">${labels}</div>
    <div class="u-readout" role="status">${usageReadoutHTML(c, hover)}</div>`;
}
function usageReadoutHTML(c, hover) {
  if (!c.series.length) return '';
  const i = hover >= 0 ? hover : c.peak, d = c.series[i], calls = Usage.metric === 'calls', unit = c.period === 'month' ? 'month' : 'day';
  const now = c.period === 'month' ? usageToday().slice(0, 7) : usageToday();
  const title = `${hover >= 0 ? (unit === 'month' ? 'Month' : 'Day') : !calls && !c.max && d.calls ? `Busiest ${unit}` : `Peak ${unit}`} · ${usageBarLabel(d.date)}${d.date === now ? ', so far' : ''}`;
  const parts = d.groups.map((g, k) => (g.calls || g.priced ? `<span class="u-rp"><i style="background:${c.colors[k]}"></i><span class="u-rp-name">${ux(usageStack(c.keys[k]))}</span> <span class="mono">${ux(calls ? `${usageCount(g.calls)} calls` : usageDayCost(g))}</span>${g.unpriced ? `<span class="dim">(${calls ? '' : `${usageCount(g.calls - g.unpriced)} priced · `}${usageCount(g.unpriced)} unpriced)</span>` : ''}</span>` : '')).join('');
  // A month is before the proxy only when its last day is.
  const last = c.period === 'month' ? UsageTools.shift(`${UsageTools.shift(`${d.date}-28`, 4).slice(0, 7)}-01`, -1) : d.date;
  const before = UsageTools.beforeProxy(last, usageCombined().proxy_first_event_at_ms, Usage.timezone);
  return `<span class="${hover >= 0 ? '' : 'dim'}">${ux(title)}</span><span class="mono">${ux(usageDayCost(d))}</span><span class="fg2">${usageCount(d.calls)} call${d.calls === 1 ? '' : 's'}</span>${parts}${before ? '<span class="dim">Before the proxy was set up · history only</span>' : ''}`;
}

// Claude Code names its entrypoint in the User-Agent. The Agent SDK labels (sdk-ts,
// sdk-py, sdk-cli for `claude -p`) are reported to use the 5-hour window faster.
const USAGE_ENTRYPOINTS = {
  cli: 'Claude Code in a terminal', 'sdk-ts': 'Agent SDK · TypeScript', 'sdk-py': 'Agent SDK · Python', 'sdk-cli': 'claude -p',
  'claude-vscode': 'VS Code extension', 't3-code': 'T3 Code', 'claude-desktop': 'Claude desktop app', mcp: 'MCP server',
  'claude-code-github-action': 'GitHub Action', remote: 'Claude Code on the web',
};
const usageSdkEntrypoint = (id) => typeof id === 'string' && id.startsWith('sdk-');
const USAGE_SDK_NOTE = 'Reported to use the 5-hour window about 1.8× faster than the terminal';
function usageEntrypointData() {
  const trend = Usage.entrypoints?.trend || [];
  if (!trend.length) return null;
  const days = UsageTools.dates(Usage.range), index = new Map(days.map((d, i) => [d, i]));
  const series = days.map((date) => ({ date, sdk: 0, other: 0 }));
  let sdk = 0, other = 0;
  for (const r of trend) {
    const n = Number(r.observations || 0), day = series[index.get(r.date)];
    if (usageSdkEntrypoint(r.group)) { sdk += n; if (day) day.sdk += n; } else { other += n; if (day) day.other += n; }
  }
  const latest = [...series].reverse().find((d) => d.sdk + d.other > 0);
  return { series, sdk, other, latest, max: Math.max(0, ...series.map((d) => d.sdk + d.other)) };
}
const usageShare = (part, whole) => (whole > 0 ? `${Math.round(part / whole * 100)}%` : '—');
// Which entrypoints Claude Code calls named, by day: the SDK labels against the rest.
function usageEntrypointsHTML() {
  const c = usageEntrypointData();
  if (!c) return '';
  const phone = usagePhone(), days = c.series.map((d) => d.date), today = usageToday();
  const all = c.sdk + c.other;
  const legend = `<div class="u-legend"><span><i class="u-ep-sdk"></i>SDK labels</span><span><i class="u-ep-other"></i>Other labels</span></div>`;
  const latest = c.latest && days.length > 1
    ? `<div class="u-ep-fig"><b>${ux(usageShare(c.latest.sdk, c.latest.sdk + c.latest.other))}</b><span>${c.latest.date === today ? 'today so far' : ux(`on ${UsageTools.dayLabel(c.latest.date)}`)}</span></div>` : '';
  const bars = days.length > 1 ? c.series.map((d, i) => {
    const total = d.sdk + d.other, h = c.max > 0 ? Math.max(total > 0 ? 1.5 : 0, total / c.max * 100) : 0;
    const text = `${UsageTools.dayLabel(d.date)}: ${usageCount(d.sdk)} with an SDK label, ${usageCount(d.other)} with another${total ? `, ${usageShare(d.sdk, total)} SDK` : ''}`;
    const edge = i < 2 ? ' lft' : i > days.length - 3 ? ' rgt' : '';
    const tip = total ? `<span class="u-ep-tip${edge}" aria-hidden="true"><b>${ux(UsageTools.dayLabel(d.date))}</b><span><i class="u-ep-sdk"></i>SDK labels <span class="mono">${usageCount(d.sdk)}</span></span><span><i class="u-ep-other"></i>Other labels <span class="mono">${usageCount(d.other)}</span></span><span class="dim">SDK share ${ux(usageShare(d.sdk, total))}</span></span>` : '';
    const segs = `${d.sdk ? `<i class="u-ep-sdk" style="flex:${d.sdk} 1 0px"></i>` : ''}${d.other ? `<i class="u-ep-other" style="flex:${d.other} 1 0px"></i>` : ''}`;
    return `<span class="u-bar u-ep-bar" tabindex="0" role="img" aria-label="${ux(text)}"><span class="u-bar-s" style="height:${h.toFixed(2)}%">${segs}</span>${tip}</span>`;
  }).join('') : '';
  const labels = days.length > 1 ? c.series.map((d) => {
    const p = UsageTools.parts(d.date);
    const text = days.length <= 10 ? (phone ? p.w.slice(0, 2) : `${p.w} ${p.d}`) : p.monday ? `${p.m} ${p.d}` : '';
    return `<span${d.date === today ? ' class="today"' : ''}>${ux(text)}</span>`;
  }).join('') : '';
  return `<section class="card u-ep" aria-labelledby="usage-ep-label">
    <div class="u-chart-head"><span class="label" id="usage-ep-label">CLAUDE CODE ENTRYPOINTS · ${ux(usageRangeLabel().toUpperCase())}</span><span class="grow"></span>${legend}</div>
    <div class="u-ep-figs"><div class="u-ep-fig"><b>${ux(usageShare(c.sdk, all))}</b><span>of ${ux(usageCount(all))} calls had an SDK label</span></div>${latest}</div>
    ${bars ? `<div class="u-bars u-ep-bars${days.length > 20 ? ' dense' : ''}">${bars}</div><div class="u-days${days.length > 20 ? ' dense' : ''}" aria-hidden="true">${labels}</div>` : ''}
    <p class="u-ep-note">Claude Code names its entrypoint in the User-Agent; Fusebox records it and passes it on unchanged. The Agent SDK labels (sdk-ts, sdk-cli) are reported to use the 5-hour window about 1.8× faster than cli (anthropics/claude-code#97074). Only calls that named one are counted.</p>
  </section>`;
}

// Output speed per model: tokens a second once the first content arrived, from the
// timings of calls Fusebox served. Codex models split by the tier the call asked
// for, which is also what pricing uses (Fast calls can come back saying "default").
const USAGE_SPEED_TIERS = { fast: 'Fast', ultrafast: 'Ultrafast', standard: 'Standard' };
const USAGE_SPEED_ORDER = { fast: 0, ultrafast: 1, standard: 2 };
const USAGE_SPEED_ROWS = 12;
const USAGE_SPEED_PICKS = 6;
const usageSpeedFields = ['calls', 'timed', 'output', 'stream_ms', 'ttft_ms'];
const usageSpeedSum = (rows) => rows.reduce((t, r) => { for (const f of usageSpeedFields) t[f] += Number(r[f] || 0); return t; }, { calls: 0, timed: 0, output: 0, stream_ms: 0, ttft_ms: 0 });
const usageRate = (t) => (t.stream_ms > 0 ? t.output * 1000 / t.stream_ms : null);
const usageFirst = (t) => (t.timed > 0 ? t.ttft_ms / t.timed / 1000 : null);
const usageRateText = (v) => (v == null ? '—' : v < 10 ? v.toFixed(1) : numbersText(Math.round(v)));
const usageSecondsText = (v) => (v == null ? '—' : `${v < 10 ? v.toFixed(1) : Math.round(v)} s`);
const numbersText = (n) => n.toLocaleString('en-US');
const usageSpeedKey = (r) => `${r.provider ?? ''}\u0000${r.model ?? ''}`;
const usageSpeedChange = (now, before) => {
  if (now == null || before == null || before <= 0) return ['—', 'dim'];
  const d = Math.round((now / before - 1) * 100);
  return [d === 0 ? '±0%' : `${d > 0 ? '+' : '−'}${Math.abs(d)}%`, d >= 3 ? 'ok' : d <= -3 ? 'warn' : 'dim'];
};
function usageSpeedData() {
  const s = Usage.speed;
  if (!s) return null;
  const models = new Map(), before = new Map();
  for (const r of s.models || []) {
    const k = usageSpeedKey(r);
    if (!models.has(k)) models.set(k, { key: k, provider: r.provider, model: r.model, tiers: [] });
    models.get(k).tiers.push(r);
  }
  for (const r of s.previous || []) {
    const k = usageSpeedKey(r);
    if (!before.has(k)) before.set(k, []);
    before.get(k).push(r);
  }
  const list = [...models.values()].map((m) => {
    m.tiers.sort((a, b) => USAGE_SPEED_ORDER[a.speed] - USAGE_SPEED_ORDER[b.speed]);
    const total = usageSpeedSum(m.tiers), prev = before.get(m.key) || [];
    const tiers = m.tiers.map((r) => ({ ...r, rate: usageRate(r), first: usageFirst(r), before: usageRate(usageSpeedSum(prev.filter((p) => p.speed === r.speed))) }));
    return { ...m, ...total, tiers, rate: usageRate(total), first: usageFirst(total), before: usageRate(usageSpeedSum(prev)),
      fast: m.tiers.filter((r) => r.speed !== 'standard').reduce((n, r) => n + Number(r.calls || 0), 0) };
  });
  // Charted models keep their colour while they stay charted; the busiest four start.
  const keys = new Set(list.map((m) => m.key));
  let picked = Usage.speedPicked && Object.fromEntries(Object.entries(Usage.speedPicked).filter(([k]) => keys.has(k)));
  if (!picked || !Object.keys(picked).length) {
    picked = {};
    [...list].filter((m) => m.timed > 0).sort((a, b) => b.timed - a.timed).slice(0, 4).forEach((m, i) => { picked[m.key] = i; });
  }
  Usage.speedPicked = picked;
  const providers = new Set(list.filter((m) => m.fast > 0).map((m) => m.provider));
  const withFast = list.filter((m) => providers.has(m.provider));
  const fastCalls = withFast.reduce((n, m) => n + m.fast, 0), fastOf = withFast.reduce((n, m) => n + m.calls, 0);
  // The busiest model timed at both speeds, for the Fast-against-Standard figure.
  const both = [...list].sort((a, b) => b.calls - a.calls).find((m) => m.tiers.some((t) => t.speed === 'fast' && t.rate) && m.tiers.some((t) => t.speed === 'standard' && t.rate));
  const gain = both && { model: both.model, ratio: both.tiers.find((t) => t.speed === 'fast').rate / both.tiers.find((t) => t.speed === 'standard').rate };
  const calls = list.reduce((n, m) => n + m.calls, 0), timed = list.reduce((n, m) => n + m.timed, 0);
  return { list, picked, calls, timed, providers: [...providers], fastShare: fastOf ? fastCalls / fastOf : null, gain, backfilling: !!s.backfilling };
}
const usageSpeedColor = (slot) => {
  const light = typeof document !== 'undefined' && document.documentElement.dataset.shade === 'light';
  return `oklch(${light ? 0.6 : 0.74} 0.09 ${[30, 90, 150, 210, 270, 330][slot]})`;
};
// Nice steps for the value axis: 1, 2 or 5 times a power of ten, at most five lines.
function usageSpeedAxis(max) {
  if (!(max > 0)) return { top: 1, step: 1 };
  const raw = max / 4, power = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 5, 10].map((m) => m * power).find((v) => v >= raw);
  return { top: Math.ceil(max / step) * step, step };
}
// The charted models by day (or month), one line per speed tier; Fast is dashed.
function usageSpeedChart(c) {
  const days = UsageTools.dates(Usage.range), period = usagePeriod(days.length);
  const keys = period === 'month' ? UsageTools.months(days) : days;
  if (keys.length < 2) return null;
  const index = new Map(keys.map((k, i) => [k, i])), metric = Usage.speedMetric;
  const lines = new Map();
  for (const r of Usage.speed?.trend || []) {
    const k = usageSpeedKey(r);
    if (!(k in c.picked) || !Number(r.timed)) continue;
    const id = `${k}\u0000${r.speed}`;
    if (!lines.has(id)) lines.set(id, { key: k, model: r.model, speed: r.speed, slot: c.picked[k], sums: keys.map(() => usageSpeedSum([])) });
    const i = index.get(period === 'month' ? String(r.date).slice(0, 7) : r.date);
    if (i == null) continue;
    const t = lines.get(id).sums[i];
    for (const f of usageSpeedFields) t[f] += Number(r[f] || 0);
  }
  const series = [...lines.values()].sort((a, b) => a.slot - b.slot || USAGE_SPEED_ORDER[a.speed] - USAGE_SPEED_ORDER[b.speed])
    .map((l) => ({ ...l, values: l.sums.map((t) => (metric === 'rate' ? usageRate(t) : usageFirst(t))), color: usageSpeedColor(l.slot) }));
  const max = Math.max(0, ...series.flatMap((l) => l.values.filter((v) => v != null)));
  return { keys, period, series, axis: usageSpeedAxis(max), metric };
}
const USAGE_SPEED_PLOT = 170;
function usageSpeedPlotHTML(c) {
  const chart = usageSpeedChart(c);
  if (!chart) return '<p class="u-sp-none">Pick a longer range to see speed by day.</p>';
  const { keys, series, axis, metric } = chart, phone = usagePhone(), today = usageToday();
  if (!series.length) return '<p class="u-sp-none">Click a model above to chart it.</p>';
  const x = (i) => (keys.length === 1 ? 500 : (i / (keys.length - 1)) * 1000);
  const y = (v) => (1 - v / axis.top) * 100;
  const fmt = (v) => (metric === 'rate' ? usageRateText(v) : usageSecondsText(v));
  let grid = '', ylabels = '';
  for (let v = 0; v <= axis.top + 1e-9; v += axis.step) {
    grid += `<line x1="0" x2="1000" y1="${y(v)}" y2="${y(v)}"/>`;
    ylabels += `<span style="top:${(y(v) / 100 * USAGE_SPEED_PLOT).toFixed(1)}px">${ux(metric === 'rate' ? numbersText(Math.round(v)) : `${+v.toFixed(2)}s`)}</span>`;
  }
  let paths = '', dots = '';
  const ends = [];
  for (const l of series) {
    const dash = l.speed === 'fast' ? ' stroke-dasharray="6 4"' : l.speed === 'ultrafast' ? ' stroke-dasharray="2 3"' : '';
    let run = [];
    const flush = () => {
      if (run.length > 1) paths += `<polyline points="${run.map(([i, v]) => `${x(i).toFixed(1)},${y(v).toFixed(2)}`).join(' ')}" stroke="${l.color}"${dash}/>`;
      run = [];
    };
    l.values.forEach((v, i) => { if (v == null) flush(); else run.push([i, v]); });
    flush();
    l.values.forEach((v, i) => {
      const lone = v != null && l.values[i - 1] == null && l.values[i + 1] == null;
      if (lone) dots += `<i class="u-sp-dot${l.speed === 'standard' ? '' : ' hollow'}" style="left:${(x(i) / 10).toFixed(2)}%;top:${(y(v) / 100 * USAGE_SPEED_PLOT).toFixed(1)}px;--c:${l.color}"></i>`;
    });
    const last = l.values.map((v, i) => [v, i]).filter(([v]) => v != null).pop();
    if (last) {
      dots += `<i class="u-sp-dot${l.speed === 'standard' ? '' : ' hollow'}" style="left:${(x(last[1]) / 10).toFixed(2)}%;top:${(y(last[0]) / 100 * USAGE_SPEED_PLOT).toFixed(1)}px;--c:${l.color}"></i>`;
      ends.push({ l, v: last[0], top: y(last[0]) / 100 * USAGE_SPEED_PLOT });
    }
  }
  ends.sort((a, b) => a.top - b.top);
  for (let i = 1; i < ends.length; i++) if (ends[i].top - ends[i - 1].top < 13) ends[i].top = ends[i - 1].top + 13;
  const short = (m) => String(m || 'Unknown model').replace(/^claude-/, '').replace(/^.*\//, '');
  const labels = phone ? '' : `<div class="u-sp-ends" aria-hidden="true">${ends.map((e) => `<span style="top:${e.top.toFixed(1)}px"><b>${ux(fmt(e.v))}</b> ${ux(short(e.l.model))}${e.l.speed === 'standard' ? '' : ` ${ux(USAGE_SPEED_TIERS[e.l.speed].toLowerCase())}`}</span>`).join('')}</div>`;
  const every = keys.length <= 10 ? 1 : Math.ceil(keys.length / (phone ? 4 : 7));
  const xlabels = keys.map((k, i) => {
    if (i % every && i !== keys.length - 1) return '';
    const text = chart.period === 'month' ? UsageTools.monthLabel(k) : (() => { const p = UsageTools.parts(k); return keys.length <= 10 ? (phone ? p.w.slice(0, 2) : `${p.w} ${p.d}`) : `${p.m} ${p.d}`; })();
    return `<span style="left:${(x(i) / 10).toFixed(2)}%"${k === today || k === today.slice(0, 7) && chart.period === 'month' ? ' class="today"' : ''}>${ux(text)}</span>`;
  }).join('');
  const legend = `<div class="u-legend u-sp-legend">${series.map((l) => `<span>${l.speed === 'standard' ? `<i style="background:${l.color}"></i>` : `<svg viewBox="0 0 16 8" aria-hidden="true"><line x1="0" x2="16" y1="4" y2="4" stroke="${l.color}" stroke-width="2" stroke-dasharray="${l.speed === 'fast' ? '4 3' : '1.5 2.5'}"/></svg>`}<span class="mono">${ux(l.model || 'Unknown model')}</span>${l.speed === 'standard' ? '' : ` ${ux(USAGE_SPEED_TIERS[l.speed])}`}</span>`).join('')}</div>`;
  const what = metric === 'rate' ? 'Output tokens a second' : 'Seconds to first token';
  const summary = series.map((l) => `${l.model}${l.speed === 'standard' ? '' : ` ${USAGE_SPEED_TIERS[l.speed]}`}: ${l.values.filter((v) => v != null).length ? `latest ${fmt(l.values.filter((v) => v != null).pop())}` : 'no timed calls'}`).join('; ');
  return `${legend}<div class="u-sp-plot" data-usage-speed-plot="${keys.length}" role="img" aria-label="${ux(`${what} by ${chart.period} for the charted models. ${summary}`)}">
    <div class="u-sp-y" aria-hidden="true">${ylabels}</div>
    <div class="u-sp-area"><svg viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true"><g class="u-sp-grid">${grid}</g><g class="u-sp-lines">${paths}</g></svg>${dots}<span class="u-sp-xh" hidden></span><div class="u-sp-tip" hidden></div></div>
    ${labels}<div class="u-sp-x" aria-hidden="true">${xlabels}</div></div>`;
}
// The hover readout for one day of the speed chart, drawn into the plot without a repaint.
function usageSpeedHover(plot, clientX) {
  const area = plot.querySelector('.u-sp-area'), tip = plot.querySelector('.u-sp-tip'), xh = plot.querySelector('.u-sp-xh');
  const c = usageSpeedData(), chart = c && usageSpeedChart(c);
  if (!area || !chart || !chart.series.length) return;
  const box = area.getBoundingClientRect(), n = chart.keys.length;
  if (clientX == null || clientX < box.left - 8 || clientX > box.right + 8) { tip.hidden = true; xh.hidden = true; return; }
  const i = Math.max(0, Math.min(n - 1, Math.round(((clientX - box.left) / box.width) * (n - 1))));
  const fmt = (v) => (chart.metric === 'rate' ? `${usageRateText(v)} tok/s` : usageSecondsText(v));
  const rows = chart.series.map((l) => [l, l.values[i]]).sort((a, b) => (b[1] ?? -1) - (a[1] ?? -1));
  const label = chart.period === 'month' ? UsageTools.monthLabel(chart.keys[i]) : UsageTools.dayLabel(chart.keys[i]);
  tip.innerHTML = `<b>${ux(label)}</b>${rows.map(([l, v]) => `<span><i style="background:${l.color}"></i><span class="mono">${ux(l.model || 'Unknown model')}</span>${l.speed === 'standard' ? '' : ` ${ux(USAGE_SPEED_TIERS[l.speed])}`}<span class="mono v">${ux(v == null ? '—' : fmt(v))}</span></span>`).join('')}`;
  const left = n === 1 ? 0.5 : i / (n - 1);
  xh.style.left = `${(left * 100).toFixed(2)}%`; xh.hidden = false;
  tip.hidden = false;
  const width = tip.offsetWidth, px = left * box.width;
  tip.style.left = `${px + 14 + width > box.width ? Math.max(0, px - width - 14) : px + 14}px`;
}
function usageSpeedRowHTML(m, max, phone) {
  const on = m.key in Usage.speedPicked, color = on ? usageSpeedColor(Usage.speedPicked[m.key]) : '';
  const pick = `<span class="u-sp-pick${on ? ' on' : ''}" aria-hidden="true"${on ? ` style="--c:${color}"` : ''}></span>`;
  const tiers = m.tiers.length > 1 || m.tiers[0]?.speed !== 'standard';
  const sub = `${usageProvName(m.provider)}${m.tiers.length > 1 ? ` · ${m.tiers.map((t) => USAGE_SPEED_TIERS[t.speed]).join(' and ')}` : m.tiers[0]?.speed !== 'standard' ? ` · ${USAGE_SPEED_TIERS[m.tiers[0].speed]} only` : ''}`;
  const bar = (rate, kid) => `<span class="u-sp-rate${kid ? ' kid' : ''}"><b>${ux(usageRateText(rate))}</b><span class="dim">tok/s</span><span class="u-sp-bar"><i style="width:${rate == null || !max ? 0 : (rate / max * 100).toFixed(1)}%"></i></span></span>`;
  const share = m.fast ? (() => { const p = Math.round(m.fast / m.calls * 100); return `<span class="u-sp-share">${p}%<span><i style="width:${p}%"></i></span></span>`; })() : '<span class="dim">—</span>';
  const [change, changeCls] = usageSpeedChange(m.rate, m.before);
  const label = `${m.model || 'Unknown model'}, ${usageRateText(m.rate)} tokens a second${on ? ', charted' : ''}`;
  const kids = tiers && m.tiers.length > 1 ? m.tiers.map((t) => {
    const [c, cls] = usageSpeedChange(t.rate, t.before);
    if (phone) return `<span class="u-sp-kid"><span class="dim">↳ ${ux(USAGE_SPEED_TIERS[t.speed])}</span><span class="mono">${ux(usageRateText(t.rate))} tok/s</span><span class="dim">${ux(usageSecondsText(t.first))} · ${usageCount(t.calls)} calls</span></span>`;
    return `<div class="u-sp-row kid"><span class="u-sp-who">↳ ${ux(USAGE_SPEED_TIERS[t.speed])}</span>${bar(t.rate, true)}<span class="r mono fg2">${ux(usageSecondsText(t.first))}</span><span class="r">${t.speed === 'standard' ? '' : `<span class="u-ep-tag">${ux(USAGE_SPEED_TIERS[t.speed])}</span>`}</span><span class="r ${cls} u-wide">${ux(c)}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(t.output))}</span><span class="r">${usageCount(t.calls)}</span></div>`;
  }).join('') : '';
  if (phone) return `<button type="button" class="u-sp-item" data-usage-act="speed-pick" data-value="${ux(m.key)}" aria-pressed="${on}" aria-label="${ux(label)}"><span class="u-bd-l1">${pick}${usageLogo(m.provider)}<span class="u-bd-name mono">${ux(m.model || 'Unknown model')}</span><span class="u-bd-cost mono">${ux(usageRateText(m.rate))}<span class="dim"> tok/s</span></span></span><span class="u-bd-l2"><span class="dim">${ux(usageSecondsText(m.first))} to first token · ${usageCount(m.calls)} calls${m.fast ? ` · ${Math.round(m.fast / m.calls * 100)}% Fast` : ''}</span></span>${kids}</button>`;
  return `<button type="button" class="u-sp-row${on ? ' on' : ''}" data-usage-act="speed-pick" data-value="${ux(m.key)}" aria-pressed="${on}" aria-label="${ux(label)}"><span class="u-sp-who">${pick}${usageLogo(m.provider)}<span class="cell2"><span class="mono">${ux(m.model || 'Unknown model')}</span><span class="dim">${ux(sub)}</span></span></span>${bar(m.rate)}<span class="r mono fg2">${ux(usageSecondsText(m.first))}</span><span class="r">${share}</span><span class="r ${changeCls} u-wide">${ux(change)}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(m.output))}</span><span class="r">${usageCount(m.calls)}</span></button>${kids}`;
}
function usageSpeedHTML() {
  const c = usageSpeedData();
  if (!c) return '';
  const phone = usagePhone(), range = ux(usageRangeLabel().toUpperCase());
  if (!c.list.length) {
    return `<section class="card u-sp" aria-labelledby="usage-sp-label"><div class="u-chart-head"><span class="label" id="usage-sp-label">OUTPUT SPEED BY MODEL · ${range}</span></div>
      <p class="u-sp-none">${c.backfilling ? 'Adding timings to earlier calls; this fills in shortly.' : 'No timed calls in this range. Fusebox times the calls it serves; imported history has no timings.'}</p></section>`;
  }
  const providers = [...new Set(c.list.map((m) => m.provider))];
  if (Usage.speedProvider && !providers.includes(Usage.speedProvider)) Usage.speedProvider = '';
  const chips = providers.length > 1 ? `<div class="u-tabs u-sp-chips" role="group" aria-label="Provider">${[['', 'All models', c.list.length], ...providers.map((p) => [p, usageProvName(p), c.list.filter((m) => m.provider === p).length])]
    .map(([p, name, n]) => `<button type="button" data-usage-act="speed-provider" data-value="${ux(p ?? '')}" aria-pressed="${(Usage.speedProvider || '') === (p ?? '')}">${p ? usageLogo(p, 14) : ''}${ux(name)} <span class="mono dim">${n}</span></button>`).join('')}</div>` : '';
  const sort = Usage.speedSort;
  const rows = c.list.filter((m) => !Usage.speedProvider || m.provider === Usage.speedProvider).sort((a, b) => {
    if (sort === 'calls') return b.calls - a.calls;
    // Untimed models go last either way, so check before the direction swaps them.
    const [p, q] = sort === 'first' ? [a.first, b.first] : [a.rate, b.rate];
    if (p == null || q == null) return (p == null) - (q == null) || b.calls - a.calls;
    return (sort === 'first' ? p - q : q - p) || b.calls - a.calls;
  });
  const shown = Usage.speedAll ? rows : rows.slice(0, USAGE_SPEED_ROWS);
  const max = Math.max(0, ...c.list.flatMap((m) => [m.rate, ...m.tiers.map((t) => t.rate)]).filter((v) => v != null));
  const body = shown.map((m) => usageSpeedRowHTML(m, max, phone)).join('');
  const more = rows.length > USAGE_SPEED_ROWS ? `<button type="button" class="u-more" data-usage-act="speed-all">${Usage.speedAll ? `Show top ${USAGE_SPEED_ROWS}` : `Show all ${rows.length}`}</button>` : '';
  const head = (key, text, cls = '') => `<button type="button" class="u-sp-sort${cls}" data-usage-act="speed-sort" data-value="${key}" aria-pressed="${sort === key}">${text}${sort === key ? (key === 'first' ? ' ▴' : ' ▾') : ''}</button>`;
  const days = UsageTools.dates(Usage.range).length;
  const period = days === 1 ? 'day before' : `previous ${days} days`;
  const th = phone ? '' : `<div class="u-sp-th"><span>MODEL</span>${head('rate', 'OUTPUT SPEED', ' l')}${head('first', 'FIRST TOKEN', ' r')}<span class="r">FAST</span><span class="r u-wide" title="${ux(`Output speed against the ${period}`)}">CHANGE</span><span class="r u-wide">TOKENS OUT</span>${head('calls', 'CALLS', ' r')}</div>`;
  const stats = [[numbersText(c.list.length), 'Models'], [usageCount(c.timed), 'Timed calls'], [usageCount(c.calls - c.timed), 'Not timed']];
  if (c.fastShare != null) stats.push([`${Math.round(c.fastShare * 100)}%`, `${c.providers.length === 1 ? `${usageProvName(c.providers[0])} calls` : 'Calls'} on Fast`]);
  if (c.gain) stats.push([`×${c.gain.ratio.toFixed(1)}`, `Fast vs Standard · ${c.gain.model}`]);
  const metric = `<div class="seg u-seg" role="group" aria-label="Chart"><button type="button" data-usage-act="speed-metric" data-value="rate" aria-pressed="${Usage.speedMetric === 'rate'}">Speed</button><button type="button" data-usage-act="speed-metric" data-value="first" aria-pressed="${Usage.speedMetric === 'first'}">First token</button></div>`;
  const list = phone ? `<div class="u-sp-list">${body}${more}</div>` : `<div class="u-sp-table">${th}${body}${more}</div>`;
  return `<section class="card u-sp" aria-labelledby="usage-sp-label">
    <div class="u-sp-top"><span class="label" id="usage-sp-label">OUTPUT SPEED BY MODEL · ${range}</span><dl class="u-stats">${stats.map(([v, k]) => `<div><dd>${ux(v)}</dd><dt>${ux(k)}</dt></div>`).join('')}</dl></div>
    <div class="u-sp-ctl">${chips}<span class="grow"></span><span class="meta">Click a model to chart it</span></div>
    ${list}
    <div class="u-chart-head u-sp-chart-head"><span class="label">BY ${usagePeriod(days) === 'month' ? 'MONTH' : 'DAY'}</span><span class="meta">${Usage.speedMetric === 'rate' ? 'Output tokens a second' : 'Seconds to first token'} · charted models</span><span class="grow"></span>${metric}</div>
    <div id="usage-speed-chart">${usageSpeedPlotHTML(c)}</div>
    <div class="u-foot u-sp-foot"><span>Speed is output tokens over the time after the first token, summed over calls, so long answers count more. Reasoning tokens count as output.</span><span>Fast is the tier the call asked for. Not timed: calls without streamed content or reported output, and answers that arrived in one piece.${c.backfilling ? ' Earlier calls are still getting their timings.' : ''}</span></div>
  </section>`;
}

const USAGE_DIMS = [['model', 'Model'], ['account', 'Account'], ['provider', 'Provider'], ['client', 'Client / origin'], ['entrypoint', 'Entrypoint']];
// The Entrypoint tab appears once a Claude Code call has named one.
// The entrypoints read lists calls that named one; the rest of the summary's calls
// make the "No entrypoint" row. Both reads use the same range and filters.
function usageEntrypointRows() {
  const rows = Usage.entrypoints?.breakdown;
  if (!rows) return [];
  const t = usageCombined().totals || {}, sum = (f) => rows.reduce((n, r) => n + Number(f(r) || 0), 0);
  const rest = (total, f) => (total == null ? null : Math.max(0, Number(total) - sum(f)));
  const observations = rest(t.observations ?? 0, (r) => r.observations);
  if (!observations) return rows;
  return rows.concat([{ id: null, observations, estimated_cost_nanos: rest(t.estimated_cost_nanos, (r) => r.estimated_cost_nanos),
    unpriced: rest(t.unpriced ?? 0, (r) => r.unpriced), partial: rest(t.partial ?? 0, (r) => r.partial), missing_usage: rest(t.missing_usage ?? 0, (r) => r.missing_usage),
    tokens: { input: rest(t.tokens?.input, (r) => r.tokens?.input), output: rest(t.tokens?.output, (r) => r.tokens?.output) } }]);
}
const usageBreakdowns = () => ({ ...(usageCombined().breakdowns || {}), entrypoint: usageEntrypointRows() });
const usageDims = (b) => USAGE_DIMS.filter(([k]) => k !== 'entrypoint' || Usage.filters.entrypoint || (b.entrypoint || []).some((r) => r.id != null));
function usageBreakdownRow(dim, r) {
  if (dim === 'entrypoint') {
    const sdk = usageSdkEntrypoint(r.id);
    return { name: r.id ?? 'No entrypoint', mono: r.id != null, sub: r.id == null ? 'Other clients, imported history and earlier calls' : USAGE_ENTRYPOINTS[r.id] || 'Claude Code',
      icon: `<span class="u-sq" aria-hidden="true"><i class="${r.id == null ? '' : sdk ? 'u-ep-sdk' : 'u-ep-other'}"></i></span>`,
      tag: sdk ? `<span class="u-ep-tag" title="${ux(USAGE_SDK_NOTE)}">SDK</span>` : '' };
  }
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
  if (r.unpriced_reasons) {
    const reasons = r.unpriced_reasons;
    const missing = Number(reasons.missing_usage || 0) + Number(reasons.incomplete_usage || 0);
    const noRate = Number(reasons.unknown_model || 0);
    if (missing && missing === Number(r.unpriced)) return [`${usageCount(missing)} missing usage`, 'warn'];
    if (noRate && noRate === Number(r.unpriced)) return [`${usageCount(noRate)} no model rate`, 'warn'];
  }
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
  const u = Usage, b = usageBreakdowns(), phone = usagePhone();
  // A range without entrypoints has no Entrypoint tab to stay on.
  if (!usageDims(b).some(([k]) => k === u.dim)) u.dim = 'model';
  const rows = b[u.dim] || [], total = Number(usageCombined().totals?.estimated_cost_nanos || 0);
  const rowLimit = u.dim === 'model' ? 20 : 8;
  const shown = u.showAll ? rows : rows.slice(0, rowLimit);
  u.rows = shown.map((r) => r.id);
  const tabs = `<div class="u-tabs" role="tablist" aria-label="Breakdown">${usageDims(b).map(([k, l]) => `<button type="button" role="tab" data-usage-act="dim" data-value="${k}" aria-selected="${u.dim === k}">${l} <span class="mono dim">${(b[k] || []).length}</span></button>`).join('')}</div>`;
  const body = shown.map((r, i) => {
    const x = usageBreakdownRow(u.dim, r), selected = r.id != null && u.filters[u.dim] === r.id;
    const [share, pct] = usageCostShare(r, total);
    const [cov, covCls] = usageCoverage(r);
    const coverageTitle = ux(usageCoverageLine(r));
    const name = x.sensitive ? usageSensitive(x.name) : ux(x.name);
    const icon = x.icon || (x.square ? '<span class="u-sq" aria-hidden="true"><i></i></span>' : x.logo || '<span class="u-sq" aria-hidden="true"><i></i></span>');
    const cost = r.estimated_cost_nanos != null ? UsageTools.dollars(r.estimated_cost_nanos) : 'Unpriced';
    const attrs = r.id != null ? `data-usage-act="filter-row" data-index="${i}" aria-pressed="${selected}"` : 'disabled';
    const bar = `<span class="u-share-bar"><i style="width:${(share * 100).toFixed(1)}%"></i></span>`;
    if (phone) return `<button type="button" class="u-bd-item${selected ? ' on' : ''}" ${attrs}><span class="u-bd-l1">${icon}<span class="u-bd-name${x.mono ? ' mono' : ''}">${name}</span>${x.tag || ''}<span class="u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span></span><span class="u-bd-l2">${bar}<span class="dim">${ux(pct)} · ${usageCount(r.observations)} calls</span>${cov !== 'Full' ? `<span class="${covCls}" title="${coverageTitle}">${ux(cov)}</span>` : ''}</span></button>`;
    return `<button type="button" class="u-bd-row${selected ? ' on' : ''}" ${attrs}><span class="u-bd-who">${icon}<span class="cell2">${x.tag ? `<span class="u-ep-name"><span class="${x.mono ? 'mono' : ''}">${name}</span>${x.tag}</span>` : `<span class="${x.mono ? 'mono' : ''}">${name}</span>`}${x.sub ? `<span class="dim">${ux(x.sub)}</span>` : ''}</span></span><span class="u-share">${bar}<span class="fg2">${ux(pct)}</span></span><span class="r">${usageCount(r.observations)}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(r.tokens?.input))}</span><span class="r mono fg2 u-wide">${ux(UsageTools.compact(r.tokens?.output))}</span><span class="r u-bd-cost${cost === 'Unpriced' ? ' dim' : ''}">${ux(cost)}</span><span class="r ${covCls}" title="${coverageTitle}">${ux(cov)}</span></button>`;
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
  // Retention deletes as much as arrives: where the file levels off with this many days of records.
  if (db.settles_near_bytes && db.retention_days) figs.splice(2, 0, [UsageTools.bytes(db.settles_near_bytes), `Levels off, ${db.retention_days} days`]);
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
// A pointer/focus move changes only selection and its readout. Retaining the bar
// elements also keeps the browser's hovered/focused target stable.
function usageHover(day) {
  if (S.route !== 'usage' || Usage.hoverDay === day) return;
  Usage.hoverDay = day;
  const el = document.getElementById('usage-chart');
  const readout = el?.querySelector('.u-readout');
  if (!readout) return usageRenderChart();
  const chart = usageChartData(), hover = chart.series.findIndex((d) => d.date === day);
  for (const bar of el.querySelectorAll('[data-usage-day]')) {
    const selected = hover >= 0 && bar.dataset.usageDay === day;
    const cls = `u-bar${hover >= 0 && !selected ? ' dim' : ''}${selected ? ' on' : ''}`;
    if (bar.className !== cls) bar.className = cls;
  }
  readout.innerHTML = usageReadoutHTML(chart, hover);
  const cost = document.getElementById('usage-part-cost');
  if (cost) usagePainted.delete(cost);
}
// Account changes reach account names in the filter and the breakdown.
function usageAccountsChanged() {
  usageRender(undefined, ['head', 'breakdown']);
}
// What each read feeds.
const USAGE_READ_PARTS = {
  summary: ['head', 'notices', 'cost', 'entrypoints', 'speed', 'breakdown', 'clients'],
  status: ['head', 'notices', 'cost', 'drawer'],
  entrypoints: ['entrypoints', 'breakdown'],
  speed: ['speed'],
};
const USAGE_LOAD_PARTS = ['head', 'notices', 'cost', 'entrypoints', 'speed', 'breakdown', 'clients'];
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
      // Without entrypoints or speed the page only lacks their cards and tab.
      if (field !== 'entrypoints' && field !== 'speed') Usage.error = e.message === 'locked' ? 'Management authentication required.' : e.message;
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
      usageRead('entrypoints', `/usage/entrypoints?${query}`, sequence),
      usageRead('speed', `/usage/speed?${query}`, sequence),
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
function usageReload() { Usage.hoverDay = null; Usage.summary = null; Usage.entrypoints = null; Usage.speed = null; return usageLoad(); }
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
        const start = values.get('start'), through = values.get('through'), today = usageToday(), min = UsageTools.shift(today, -USAGE_HISTORY_DAYS);
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
  document.addEventListener('mouseover', (event) => {
    if (usagePhone()) return;
    const bar = event.target.closest?.('[data-usage-day]');
    if (bar) usageHover(bar.dataset.usageDay);
  });
  document.addEventListener('mouseout', (event) => {
    const bars = event.target.closest?.('[data-usage-bars]');
    if (bars && !bars.contains(event.relatedTarget) && !usagePhone()) usageHover(null);
  });
  document.addEventListener('focusin', (event) => {
    const bar = event.target.closest?.('[data-usage-day]');
    if (bar && !usagePhone()) usageHover(bar.dataset.usageDay);
  });
  // The speed chart's readout follows the pointer, once a frame, without a repaint.
  let speedFrame = 0, speedAt = null;
  const speedHover = (plot, x) => {
    speedAt = [plot, x];
    if (!speedFrame) speedFrame = requestAnimationFrame(() => { speedFrame = 0; if (speedAt && document.contains(speedAt[0])) usageSpeedHover(...speedAt); });
  };
  document.addEventListener('mousemove', (event) => {
    const plot = event.target.closest?.('[data-usage-speed-plot]');
    if (plot && !usagePhone()) speedHover(plot, event.clientX);
  });
  document.addEventListener('mouseout', (event) => {
    const plot = event.target.closest?.('[data-usage-speed-plot]');
    if (plot && !plot.contains(event.relatedTarget)) speedHover(plot, null);
  });
  document.addEventListener('click', (event) => {
    const plot = event.target.closest('[data-usage-speed-plot]');
    if (plot) return speedHover(plot, event.clientX);
    const bar = event.target.closest('[data-usage-day]');
    if (bar) return usageHover(Usage.hoverDay === bar.dataset.usageDay && usagePhone() ? null : bar.dataset.usageDay);
    const el = event.target.closest('[data-usage-act]');
    if (!el || el.disabled) return;
    const act = el.dataset.usageAct, index = Number(el.dataset.index);
    if (act === 'refresh') { if (Usage.preset !== 'custom') Usage.range = UsageTools.range(Number(Usage.preset)); return usageLoad(); }
    if (act === 'range') {
      Usage.preset = el.dataset.value; Usage.customError = '';
      if (Usage.preset === 'custom') return usageRender('#usage-range [name="start"]');
      Usage.range = UsageTools.range(Number(Usage.preset)); return usageReload();
    }
    if (act === 'stack') { if (Usage.stack !== el.dataset.value) { Usage.stack = el.dataset.value; usageLoadStack(); } return; }
    if (act === 'metric') { Usage.metric = el.dataset.value; return usageRenderChart(); }
    if (act === 'period') { Usage.period = el.dataset.value; Usage.hoverDay = null; return usageRenderChart(); }
    if (act === 'dim') { Usage.dim = el.dataset.value; Usage.showAll = false; return usageRender(); }
    if (act === 'show-all') { Usage.showAll = !Usage.showAll; return usageRender(); }
    if (act === 'speed-pick') {
      const picked = Usage.speedPicked || {}, key = el.dataset.value;
      if (key in picked) delete picked[key];
      else {
        const used = new Set(Object.values(picked)), slot = [...Array(USAGE_SPEED_PICKS).keys()].find((i) => !used.has(i));
        if (slot == null) { if (typeof toast === 'function') toast(`Up to ${USAGE_SPEED_PICKS} models at a time`); return; }
        picked[key] = slot;
      }
      Usage.speedPicked = picked;
      return usageRender(`[data-usage-act="speed-pick"][data-value="${CSS.escape(key)}"]`, ['speed']);
    }
    if (act === 'speed-provider') { Usage.speedProvider = el.dataset.value; Usage.speedAll = false; return usageRender(undefined, ['speed']); }
    if (act === 'speed-sort') { Usage.speedSort = el.dataset.value; return usageRender(undefined, ['speed']); }
    if (act === 'speed-all') { Usage.speedAll = !Usage.speedAll; return usageRender(undefined, ['speed']); }
    if (act === 'speed-metric') { Usage.speedMetric = el.dataset.value; return usageRender(undefined, ['speed']); }
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
