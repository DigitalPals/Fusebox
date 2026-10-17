# Fusebox: efficiency and new-feature research, 17 October 2026

Follow-up to [roadmap-research-2026-10-13.md](roadmap-research-2026-10-13.md). Checkout: `main` at `9acda24` (equal to `fork/main`), version 0.5.1. Verification today: `cargo +1.96.1 test` 471 passed. Production (`0.5.1+gui.b2c191be55a1`) was read, never changed: `/metrics`, the management API, `/proc`, and the usage database through a read-only SQLite connection at idle I/O priority. Items marked **measured** come from that; **estimate** means a projection; the two code audits were done by research agents and cite file:line, and I re-read the lines I quote.

**Implemented the same day** on branch `feat/efficiency-quota-attribution` (worktree `.claude/worktrees/efficiency`, uncommitted): everything in sections 2 and 3.1, measured in `docs/efficiency-2026-10-17.md` on that branch. Two corrections to this note: the `max_page_count` cap in 2.1 applies only to collector outboxes, not the server database; and the winner tables never held the JSON columns, so 2.1 item 2 became dropping the unread per-source table and three unused indexes. mimalloc (2.5) was measured and rejected; glibc arena limits were adopted instead.

This file is untracked, like the October 13 note. Keep it, move it, or delete it.

## Summary

1. The usage database is the one thing that will hurt on a small VPS: **2.0 GB after 35 days of history, with no file shrink at purge**, and the Usage page's summary query already takes 0.54 s. Compressing or dropping the two JSON blobs every row carries, turning on incremental vacuum, and keeping daily rollups would cut it by roughly three quarters and make the query O(days). (Section 2.1)
2. The management API sends the 300-row request log uncompressed on every dashboard load (194 KB, 9.6 KB gzipped) and the dashboard re-renders far more than it needs to. Two small server changes and a handful of client changes fix both. (2.2, 2.3)
3. The Rust hot path is in good shape after the October 15 pass; what remains is a double parse on the Responses history and native Codex WebSocket paths, one HTTP chunk and one timer per SSE frame, and two fsyncs on the critical path of every new session. (2.4)
4. Resident memory is 154 MB with four accounts and five streams in flight, almost all heap; worth one measured experiment before any change. (2.5)
5. The project's biggest recurring cost is the client-version treadmill: 20 of the 23 issues ever opened are version bumps, and each needs a patch release. Fusebox already learns the newest Claude Code and Codex version from the traffic passing through it and already fetches Antigravity's version from Google; the cloak just doesn't use what it learns. (2.6)
6. The feature nobody in this category has, and the one that fits Fusebox's premise best: **quota attribution**. Subscriptions are limited in percent of a window, not in dollars, yet nothing today says which client or session consumed the week. (3.1)

## 1. What shipped since October 13, and what is still open

Shipped in 0.4.0 to 0.5.1 (all verified in the source or README): webhooks; named-client limits (models, programs, rate, tokens, budget, expiry); account priority; cooldown persistence; stream bootstrap buffering and the first-token timeout; model groups and the background model; `/v1/embeddings`; pace marks, the "out in" forecast and the reset timeline; config diff before save; the playground; client snippets for ten tools; Prometheus, JSON logs, `/readyz`, `usage backup`; light theme; Simplified Chinese; banked-reset auto-redeem; the nonroot image; `SECURITY.md` and a weekly audit; the SSE decoder fix; gzip-embedded assets; prompt-cache scopes; admission limits; `--version` with the commit.

Still open from that note (checked by grep today): the compliance posture (`claude-cloak: true` is still the default and the README still says so); Sign in with ChatGPT; a scoped read-only status token and a status-line snippet; `fusebox configure <client>`; pricing auto-sync from models.dev; Anthropic's `extra_usage`; opt-in capture of failed request bodies; GitHub Copilot; Codex device-code login; Homebrew and AUR; the provider incident strip; model capability declarations for custom endpoints; per-account egress proxies; keep-warm; task-shaped routing beyond `background-model`; an audit trail; an onboarding checklist; the `proxy.rs` (1,892 lines), `accounts.rs` (2,002) and `app.js` (3,400) splits.

## 2. Efficiency

### 2.1 The usage database (highest impact, measured)

Production, 2026-10-17 16:13 CEST:

| Fact | Value |
| --- | ---: |
| `usage.sqlite3` | 2,004,955,136 bytes (489,544 pages of 4 KB), freelist 0, `auto_vacuum` off |
| Observations | 286,375 (192,716 from the keeper import covering 2026-09-12 to 10-07, 93,659 native since) |
| `usage_observations` | 1,121 MiB, about 4.1 KB per row |
| `usage_entries` + `usage_source_entries` | 160 MiB each (the same 26 columns again) |
| Eleven indexes | about 433 MiB |
| Last 1,000 rows, `payload` / `snapshot_json` | 1,005 B / 876 B on average |
| `GET /api/usage/dashboard` (default) | 541 ms, 144 KB |
| `GET /api/usage/observations?limit=50` | 82 ms, 112 KB (2.2 KB per row) |
| Box | 2 vCPU, 2 GB RAM |

Why it is this big: every observation stores two JSON documents (`payload`, `snapshot_json`, `src/usage/store.rs:555`) that together exceed what fits in a 4 KB page beside the other 24 columns, so most rows spill into an overflow page; the two "winner" tables duplicate the row again; and `ENTRY_COLUMNS` only ever reads one field out of `snapshot_json` (`json_extract(snapshot_json,'$.partial')`, `store.rs:28`). The hourly purge (`purge_conn`, `store.rs:815`) deletes rows but with `auto_vacuum` off the file never shrinks, and a `max_page_count` cap exists (`store.rs:208`) which, once hit, will refuse writes rather than trim.

**Estimate:** native capture alone has added 9,000 to 26,000 rows a day (93,659 in ten days; 179 requests in the 587 s sampled). At 90 days' retention the file settles between roughly 5 and 15 GB at today's layout, on a machine with 2 GB of RAM, and the summary query grows with it.

Fixes, in order of payoff:

1. **Shrink the row.** Store `payload` and `snapshot_json` as one zstd-compressed BLOB (the `zstd` crate is already linked), or better, lift the handful of fields the code reads into columns and keep only the compressed original for provenance. Rows then fit in-page; expect the observations table to drop by two thirds or more (estimate; the JSON is highly repetitive and compresses well). A schema migration with the existing `user_version` path.
2. **Stop duplicating the winner tables.** `usage_entries` and `usage_source_entries` could hold `(canonical_key, source, observation_id)` and join, or stay as they are but without the two JSON columns. Saves about 300 MiB today.
3. **`PRAGMA auto_vacuum=INCREMENTAL`** on new databases (requires `VACUUM` once on existing ones, which `fusebox usage backup` could offer as `usage compact`) and `PRAGMA incremental_vacuum` after each hourly purge, so the 90-day purge actually frees disk.
4. **Daily rollups.** Maintain a `usage_daily(day, provider, model, account_id, client_id, source, totals…)` table from the writer thread, and have `summary()` (`src/usage/store/dashboard.rs:75`) read it for whole days and scan only today's rows. The summary becomes O(days) instead of O(rows in range).
5. **Say so on the dashboard.** Put the file size and growth per day into `/api/usage/status` and raise a fault at a configurable size. The user runs this on a 2 GB box and would want to know before the disk does.
6. Trim the `observations` response: 2.2 KB per row suggests it returns the full payload; the table on the Usage page needs a dozen fields.

### 2.2 The management API (measured)

- `GET /api/requests` is 193,717 bytes and the dashboard fetches it before first paint on every load and after every resume, even on the Overview, which shows six rows (`ui/app.js:291-295`, `3366-3376`). It gzips to 9,614 bytes. No route under `/api` sends `Content-Encoding` (`tower-http` is built with `cors` only, `Cargo.toml:47`). Add `CompressionLayer` (feature `compression-gzip`) to the management router and a `?limit=` parameter to `/api/requests`. Effort S.
- "accounts" broadcasts carry no payload, so the client answers each with four fetches (`/accounts`, `/overview`, `/models`, `/routes`) and clears its activity cache (`ui/app.js:299-310`). The server fires that broadcast from more than fifteen sites including every 429. Send the changed account in the message; fetch `/models` and `/routes` only on their own change signal. Effort M.

### 2.3 The dashboard (agent audit, line-cited)

Ranked by what a phone user notices:

1. The Usage page is rendered four or five times per load (once up front, once per `usageRead` in `finally`, once at the end, `ui/usage.js:448-527`) and again on every "accounts" broadcast (`ui/app.js:528-529`), each time as a full `view.innerHTML` rebuild including the chart. Patch sections instead.
2. Every live request rebuilds whole Overview sections with no change check: figures, 60 load bars with `toLocaleTimeString` each, the six recent rows (`ui/app.js:433-448`); on the Requests page the filter chips make six passes over 300 rows per insert (`2176-2196`); on Models the entire list is rebuilt per request (`445`). Coalesce with one `requestAnimationFrame` flush and compute chip counts incrementally.
3. The 5 s `tick` rewrites the figures whether totals changed or not (`src/mgmt.rs:1169-1176`, `ui/app.js:377-381`); `load` is already deduplicated server-side, do the same.
4. A 1 s `setInterval` runs on every route, hidden or not, doing four `querySelectorAll` sweeps (`ui/app.js:3324-3341`); the 60 s `/api/overview` poll also ignores visibility. Pause both when `document.hidden`.
5. For Chinese readers, the i18n `MutationObserver` re-walks every patched subtree and tries about 115 regexes per unmatched text node (`ui/i18n.js:80-87`, `ui/i18n-zh.js:932-1048`). Cache lookups and translate inside `patch()` rather than observing.
6. The server already sends a `faults` message the client ignores (`src/mgmt.rs:1156-1160`); consuming it removes the per-request `alertsList()` recompute.
7. The four scripts are classic `<script>` tags at the end of `<body>` with `no-cache` revalidation on all six text assets, so cold loads pay six serial conditional requests before `boot()`. Add `defer`, preload the two other fonts the first screen uses, and consider hashed asset URLs with `immutable`.

Already good: lamp-only `load` patching, document-level event delegation, bounded client state, the Usage page's own 60 s poll respects visibility.

### 2.4 The Rust hot path (agent audit, line-cited)

What the October 13 list looks like now: item 11 (double JSON parse per SSE event) is fixed on the HTTP path (`observe_sse` feeds the parsed value, `src/proxy.rs:1594-1606`); item 12 (per-attempt clones) is fixed for translated requests; item 13 (per-socket recompute) is half done, change detection exists but the work is still per socket; item 14 (two fsyncs before the upstream request) is unchanged but runs on its own thread.

What remains, ranked:

1. **Responses history capture re-parses every frame** one layer up from the fix: `execute` parses each outbound frame again and runs a second `responses::Parser` plus an `Aggregate` for every Responses request without `previous_response_id`, which is normal Codex traffic (`src/proxy.rs:636-650`). Feed the history sink from the `Value` the inner stream already holds. Effort M.
2. **Native Codex WebSocket parses each upstream message twice and clones it** (`src/ws.rs:751-755`, then `StreamParser::feed` re-parses; `capture()` at `:517` parses the terminal event a third time). Use `feed_value`. Effort S.
3. **One HTTP chunk and one `tokio::time::timeout` per SSE frame** (`src/server.rs:556-566`): an upstream chunk carrying N events becomes N chunked writes and N timer registrations, and `to_sse()` copies the data once more. Batch the frames drained from one upstream chunk into one `BytesMut` and keep a single `Sleep` reset per frame. Effort S. Matters with many concurrent Claude or Codex streams, which emit many small events.
4. **The routing registry prunes on every `pick_session`** (`src/affinity.rs:476`, `633-643`: two `retain` passes and a sum over all responses under the global mutex), `account_load` builds a HashMap over all bindings per pick with smart-quota, and `Lease::drop` scans every binding per completed request (`:339`). Negligible with hundreds of sessions; tens of microseconds of lock hold at the 10,000 cap. Prune on a timer, cache the load map for a second, index bindings by session. Effort S.
5. **Per-socket dashboard recompute** (`src/mgmt.rs:1144-1162`): each socket runs `account_load` and `current_faults` every second; `current_faults` builds 60 buckets per account with a linear `find` each (`src/state.rs:405-414`). One central ticker that runs only while `receiver_count() > 0`. Effort S.
6. **Two fsyncs before the provider is contacted** for every new explicit session (`src/affinity.rs:670-682` waits on `files::write_private`, which syncs the file and the directory, `src/files.rs:34-38`). Start the flush, send the upstream request, and await the flush before the first byte reaches the client. Dropping the directory sync alone is the low-risk half. Effort S.
7. **Passthrough still clones and re-serialises the client body per attempt** (`src/proxy.rs:954`, `:1094`); Responses `input` is cloned twice per request (`:629` and `src/affinity.rs:716`); cache-policy bookkeeping walks the full JSON trees up to six times per attempt (`src/cache.rs:114-148`, `src/formats/claude.rs:273-280`). Effort S to M.
8. **A WARN line per request** for clients that send no session id (`src/proxy.rs:157-163`) and one per attempt when cache hints are dropped (`src/formats/mod.rs:84`), on top of the INFO completion event. Make them once per client scope or `debug!`. Effort S.
9. Per-event allocations in the decoder and frame path (`src/sse.rs:20-37`, `:68`) and `Pool::canonical` materialising every account's model list per request before the exact-match check (`src/accounts.rs:1157-1170`). Effort S to M, small.

Checked and clean: config behind `ArcSwap`, broadcast skips serialisation with no receivers, the usage store uses `try_send`, no regex compilation or blocking filesystem I/O on the request path, admission off by default.

### 2.5 Resident memory (measured, cause not yet established)

With four accounts, five streams in flight and no dashboard open, the process held 154 MB (144 MB anonymous, peak 166 MB) ten minutes after start. The sampled traffic averaged about 300,000 cached tokens per request, so each in-flight request body is on the order of a megabyte of JSON held as a `serde_json::Value` with `preserve_order` (an index map per object), plus the per-attempt clone above. That is a hypothesis, not a measurement. Two cheap experiments: build with `mimalloc` as the global allocator and compare RSS under the same traffic (glibc malloc keeps freed multi-megabyte chunks per arena); and count how often the passthrough path needs the body as a tree at all. For genuine Claude Code traffic to a Claude account, the body is forwarded with headers adjusted and only the model suffix stripped, so a raw-bytes fast path with a bounded partial parse for `model`, `stream` and `metadata` would avoid building the tree. The 64 MiB Responses history cap and the 300-entry request log are bounded and are not the explanation at this size.

### 2.6 The client-version treadmill (project efficiency)

Of the 23 issues ever opened on the repository, 20 are "client X is out", opened by the daily workflow and closed by six commits in ten days. Each one ships to users only through a patch release. What a bump touches:

| Client | Typical change | Example |
| --- | --- | --- |
| Claude Code | two constants, one test | `231c507`: `CC_VERSION`, `CC_USER_AGENT`, the billing-header fixture |
| Codex | two constants | `e1388fe` |
| Antigravity | one fallback constant | `e8afd33` |
| Codex, Devin, Grok on a shape change | 40 to 76 lines | `313f279`, `59c1c7d`, `cd92514` |

Fusebox already does most of the work to not need this: `src/clients.rs` records the newest Claude Code and Codex release seen in passing traffic (`observe` is called from `src/proxy.rs:573` and `src/ws.rs:275`, persisted in `.model-lists.state`), and `antigravity::version_updater` fetches Google's update manifest every three hours. But the learned versions are used only for Fusebox's own usage and model-list calls (`src/quota.rs:293`, `src/discovery.rs:338`), while the cloak uses the compile-time constants for the user agent (`src/upstream.rs:186`) and the billing header, whose three-character fingerprint hashes the version (`:242`, `:291`).

Proposal: cloak with the learned version, including the fingerprint; snapshot the `x-stainless-*` and `anthropic-beta` profile from genuine Claude Code traffic as well, since those drift per release too (`:188-197` hard-code Node 26.3.0 and stainless 0.128.0); add a `client-identities:` override in `config.yaml` for people who never run the real client through the proxy; and keep the workflow for shape changes only, which the existing identity tests already catch. Anyone running real Claude Code or Codex through Fusebox then stops needing a release for version-only bumps. This is also a product feature: "keeps up with Claude Code and Codex by itself". Effort S for the version, M with the header profile. Caveat: when a release changes the fingerprint salt or the identity text, only a code change helps; the test guards that.

## 3. New features not in the October 13 note

### 3.1 Quota attribution: which client or session used the week (headline)

The Usage page prices tokens at API list rates, which for subscription accounts is a number nobody pays (production shows one client at $866 this month at list price). The resource that actually runs out is the 5-hour and weekly window, and Fusebox reads it every 60 s per account (`src/quota.rs:360-370`) and from response headers. Attribute each window's change between two readings to the requests that completed on that account in between, weighted by uncached input plus output tokens, and store the share per observation. Then the Usage page can say "the-dragon-claude used 61% of cybex's week; interactive sessions 9%", a session can show "this task cost 4% of the week", and a fault can warn "one client is using the week three times faster than the rest". Label it an estimate with provenance, as the usage docs already do for prices. No competitor or monitor does this; all of them show per-account meters only. Effort M. Pairs with 3.3.

### 3.2 Self-updating client identity

Section 2.6, framed as a feature. Effort S to M.

### 3.3 Per-client model overrides

Named clients can allow models but not rewrite them. A `model-map` per client (`opus → sonnet`, `claude-* → gpt-6-astra`) lets batch agents run on cheaper models or another provider while interactive sessions keep Opus, without touching the agents' configuration. It is the per-client form of the task-shaped routing the October note deferred, and it reuses the named-client machinery that already exists. Effort S.

### 3.4 Latency-aware tie-breaking and a slow-account fault

Fusebox records first-token time and total latency per request per account and never uses them. Keep a rolling TTFT and tokens-per-second per account and model; prefer the faster account among candidates with equal quota and priority; raise a fault when one account's median TTFT is several times its siblings' over the last hour (providers do degrade individual accounts). codex-lb has latency weighting; nobody has the fault. Effort M.

### 3.5 Session receipts and a weekly digest

`x-fusebox-session-end` exists and the Requests page can follow a session, but there are no per-session totals. Show requests, tokens, cached share, estimated price and (with 3.1) quota share per session, and send an optional weekly digest through the existing webhooks: quota used per account, upcoming resets, estimated price per client, faults. Effort S.

### 3.6 A translation inspector

A "show the request as Fusebox would send it" toggle on the playground, or `POST /api/translate`, that returns the provider request with secrets redacted and without sending it. Translation bugs are the largest category on CLIProxyAPI's tracker (#6191, #6264, #5879 this month) and are undebuggable from metadata. Effort S.

### 3.7 Peer mode for the laptop and the VPS

The product says both matter equally, and the README says "run one proxy per auth directory", which forces a choice. A `fusebox` provider type that forwards the client's native format untouched, with the session and client headers, lets a laptop instance use the VPS pool when reachable and its own keys otherwise. Today an `openai-compatibility` entry can point at another Fusebox but loses the Anthropic and Responses formats on the way. Effort M.

### 3.8 Smaller ideas

- An exact-match cache for `/v1/embeddings` (RAG tools re-embed the same chunks). Effort S.
- `fusebox backup` for the whole state (config, auth files, routing state, usage database) as one encrypted archive, for moving between a laptop and a server. Effort S.
- Show database size and growth on the Usage page (2.1, item 5).

## 4. Suggested order

**Now:** 2.1 items 1, 3 and 5 (shrink rows, incremental vacuum, show size); 2.2 (gzip and `?limit=`); 2.6 (learned version in the cloak); dashboard items 1, 3, 4 and 6.
**Next:** daily rollups; the four small hot-path fixes (2.4 items 2, 3, 6, 8); quota attribution (3.1); per-client model map (3.3).
**Later:** the Responses capture restructure (2.4 item 1); latency-aware routing (3.4); peer mode (3.7); the memory experiment (2.5); and the compliance decision the October note put first, which is still open.

## Sources

Production readings 2026-10-17 16:05 to 16:20 CEST (`/metrics`, `/api/*`, `/proc/<pid>/status` and `smaps_rollup`, read-only SQLite with `dbstat`); `notes/roadmap-research-2026-10-13.md`; `docs/efficiency-2026-10-15.md`; `git log` and `gh issue list` for DigitalPals/Fusebox; CLIProxyAPI releases v8.0.28 to v8.0.32 (Claude 5.5 thinking-disabled handling, which Fusebox already has in `normalize_disabled_thinking`, `src/formats/claude.rs:301`); the two agent audits of 2026-10-17.
