# Fusebox: improvements, new features and competitor research

Research date: 2026-10-13. Repo at `~/Code/Fusebox`, local `main` at `7b6211d` (11 commits behind `fork/main`, which carries the 0.4.0 release and named clients). Verification on this checkout: `cargo +1.96.1 clippy --all-targets` clean, `cargo +1.96.1 test` 396 passed. Everything marked **verified** was checked in the source; items marked **unverified** come from a competitor's docs or a research agent and were not confirmed.

This file is untracked. Keep it, move it, or delete it.

## Summary

Fusebox is in good shape: bounded in-memory state everywhere, credentials never logged, constant-time key comparison, usage store on its own thread, real router-plus-mock-upstream tests on three operating systems. Its differentiators against the 55k-star CLIProxyAPI are exactly the things that project's users ask for and get only from plugins: quota-aware routing, persisted session affinity, an embedded dashboard that works on a phone, durable usage and costs, push notifications, banked resets, named client keys, and live model discovery.

The twelve highest-value moves, in order:

1. **Decide the compliance posture before building more.** Anthropic's Claude Code legal page (fetched 2026-10-13) says developers may not "route requests through Free, Pro, or Max plan credentials on behalf of their users" nor "collect, store, or intermediate Claude.ai credentials or session tokens", and reserves enforcement "without prior notice"; Google bans accounts that reach Antigravity through proxies. Fusebox stores OAuth tokens, pools accounts and cloaks other clients as Claude Code. Minimum: `claude-cloak` off by default behind a per-account acknowledgement, a compliance mode that forwards only genuine Claude Code traffic to subscription accounts, and a README that states the single-owner use case plainly. Details in 3.5.
2. **Add a Sign in with ChatGPT account type.** OpenAI now sanctions plan usage for open-source and locally hosted apps through the Responses API. It would be the only ToS-clean ChatGPT path in this category.
3. Close four small security holes in the management API and upstream path (Host check, forwarded internal headers, proxy fallback, credential file permissions). All are a few lines each.
4. Fast-forward local `main` to `fork/main`, then port the webhook notification system that upstream IuCC123 shipped (Discord, Slack, Teams, Telegram, Mattermost, generic). Push covers phones; webhooks cover teams and servers.
5. Grow named clients into per-client policy: model allowlist, allowed clients, request and token limits, budgets, expiry. The most requested governance feature on every competitor tracker, and Fusebox already has both halves (named keys, per-client usage).
6. Add a `priority` field on credentials, persist cooldowns across restarts, fail over when a 200 stream opens with an overload error, and add a first-token deadline. These are the resilience features users hit most.
7. Ship a scoped read-only status token plus `fusebox configure <client>`, so status lines, menu-bar apps and coding agents are set up in one step. The CLIProxyAPI ecosystem of 30 tools exists because its API is readable.
8. Show quota pace and forecasts: a pace marker on each meter, burn rate, time to exhaustion, and a reset timeline across accounts. Every third-party monitor adds this on top of raw meters.
9. Auto-spend banked resets when the pool is dry (opt-in) and sync pricing from models.dev. Both are small.
10. Prometheus `/metrics`, JSON logs, a real `/readyz`, and `fusebox usage backup`.
11. User-defined fallback chains and alias pools (`model-groups`), the routing feature CLIProxyAPI users keep asking for and get only from plugins; later, task-shaped routing for Claude Code's background calls.
12. `/v1/embeddings` and dashboard polish competitors have: config diff before save, a playground, more client snippets, a light theme, localisation.

---

## 1. Improvements to what already exists

### 1.1 Security and robustness

| # | Finding | Evidence | Severity | Effort | Fix |
|---|---|---|---|---|---|
| 1 | Management API trusts any loopback peer with no `Host`, `Origin` or `Sec-Fetch-Site` check. A DNS-rebinding page, or a plain cross-site POST to the bodyless routes, reaches `/api/config` (full YAML with keys), `/api/accounts`, login, refresh and reset on every keyless local install. With empty `api-keys` the inference API is reachable the same way because the API router uses `CorsLayer::permissive()`. **verified** | `src/mgmt.rs:320-373`, bodyless POSTs at `src/mgmt.rs:299-314`, `src/server.rs:43` | high | S | When relying on loopback trust, require `Host` to be a loopback literal or the configured host; reject mutating `/api` requests with `Sec-Fetch-Site: cross-site`. Setting `management-key` already closes it, so document that too. |
| 2 | Plain-HTTP shutdown has no deadline (TLS path uses 10 s). A long stream can hold shutdown until systemd's `TimeoutStopSec=90s` kills the process, skipping session save and usage store shutdown. Upgraded WebSockets (dashboard live view, Codex relay) get no shutdown signal at all. **verified** | `src/main.rs:225` vs `src/main.rs:239`, flushes at `src/main.rs:241-257` | high | S | One shutdown path with a 10-20 s deadline; pass a `CancellationToken` into `mgmt::live` and `ws::handle` so they close cleanly. |
| 3 | An unparseable `proxy-url` (missing scheme, unsupported `socks5h://`) is logged once and the HTTP client is built without a proxy, so every provider request goes direct. Per-account `proxy_url` hits the same path. **verified** | `src/state.rs:253-257`, no validation in `src/config.rs:441-476` | high | S | Validate in `Config::parse` and when loading credential files; refuse to start or fail the request with 502 instead of falling back. |
| 4 | OAuth credential files are written as a temp file with umask permissions and only then `chmod 0600` (world-readable window), never fsynced, and `set_file_disabled` rewrites the live file in place. Windows gets no restriction although the README promises 0600. | `src/accounts.rs:699-761`; the correct helper already exists at `src/files.rs:184-211` | medium | S | Route both through `files::write_private`; add a Windows ACL branch like `src/banked_resets/ledger.rs:131-143`. |
| 5 | On the native Claude Code passthrough every client header not in the `HOP` denylist is forwarded, and `HOP` omits `x-fusebox-client-scope` (an unsalted SHA-256 of the client's proxy key), `x-fusebox-session-id`, `x-fusebox-session-end` and their `x-cliproxy-*` twins. These reach api.anthropic.com. The Codex path is safe because it uses an allowlist. **verified** | `src/upstream.rs:50-84`, `146-156`; header inserted at `src/server.rs:95`; derivation `src/affinity.rs:36-43` | medium | S | Add the names to `HOP`, or switch Claude to the same allowlist approach as `codex_headers`. |
| 6 | `mask()` byte-slices API keys, so a non-ASCII key longer than 10 bytes panics inside `Pool::reload`, which runs at startup and inside the hot-reload task (killing hot reload silently). | `src/accounts.rs:764-770`, called from `:882`, task at `src/main.rs:293-320` | medium | S | Mask by chars; validate keys in `Config::parse`. |
| 7 | The retry loop re-sends non-idempotent generation requests after any `send()` error (not just connect errors), up to two more times per account on 500/502/503/504/529, then across `request_retry` accounts, with no overall deadline and no upper bound on `request_retry`. | `src/proxy.rs:390-409`, `885-897`, `936-950`, loop at `671` | medium | M | Retry only on connect errors or before the body was sent; treat 504 as terminal after a full upload; cap `request_retry` and add a per-request wall-clock budget. |
| 8 | No throttling of management-key failures. Comparison is constant-time but attempts are unlimited, so a weak plain key behind a public proxy is brute-forceable. | `src/mgmt.rs:378-395` | medium | S | Small per-IP delay or lockout after repeated failures, with a log line. |
| 9 | Docker image runs as root with `HOME=/root` and no `USER`. | `Dockerfile` | medium | S | Offer a `nonroot` variant; keep the root layout as the documented CLIProxyAPI drop-in. |
| 10 | `PUT /api/config` (raw YAML editor) has no revision check, unlike `PATCH /api/config/settings`, so it silently clobbers external edits. | `src/mgmt.rs:864-876` vs `892-905` | low | S | Require the same revision token; 409 on mismatch. |

### 1.2 Performance

| # | Finding | Evidence | Effort | Fix |
|---|---|---|---|---|
| 11 | Every streamed SSE event is JSON-parsed twice: once in `observe_sse`/`passthrough_stream` for quota and usage, again inside `StreamParser::feed`. | `src/proxy.rs:1213`, `1302`; `src/formats/responses.rs:438`, `src/formats/claude.rs:515` | M | Give `StreamParser` a `feed_value(&Value, event)` entry point. |
| 12 | Per-attempt deep clones: the client body, the parsed IR and a re-serialisation on every loop iteration, plus the whole Responses `input` cloned for history capture on every request. | `src/proxy.rs:783`, `790`, `807`, `875`, `991`, `595` | M | Build the upstream body once per account; share `Request` via `Arc`; capture history lazily. |
| 13 | Each dashboard socket recomputes account load and faults every second independently. | `src/mgmt.rs:1003-1045` | S | Compute once in a central ticker and broadcast on change. |
| 14 | Every new session waits for two fsyncs (file and directory) before the upstream request starts. | `src/affinity.rs:665-677` | S | Drop the directory sync, or persist after the response starts when no migration is involved. |

These matter only at VPS scale with many concurrent long conversations; nothing blocks the tokio runtime.

### 1.3 Maintainability and tests

| # | Finding | Evidence | Effort | Fix |
|---|---|---|---|---|
| 15 | `execute_inner` is one ~420-line loop doing selection, refresh, translation, headers, send, error classification and stream dispatch. | `src/proxy.rs:640-1058` | M | Split into `select`, `build`, `send`, `classify_failure`, `dispatch`. |
| 16 | `accounts.rs` (1873 lines) mixes the provider catalog, credential file I/O, pool reload and three routing strategies; `upstream.rs` repeats header boilerplate per provider; `oauth::refresh_locked` is one giant provider match. | `src/accounts.rs`, `src/upstream.rs`, `src/oauth.rs:363-470` | M | `provider.rs`, `credentials.rs`, `pool.rs`, `scoring.rs`; an `UpstreamProfile` struct consumed by one header builder. |
| 17 | `ui/app.js` (3072 lines) keeps all state in one global, builds HTML from template strings, and the Node tests slice functions out by string markers, so a renamed marker silently tests the wrong code. | `ui/app.js:35-78`, `tests/ui-request-accounting.test.cjs:8-20` | M | Split into a few plain scripts exporting pure functions on `window.Fusebox` (plus `module.exports` as `usage.js` does); tests `require` them. No build step needed. |
| 18 | No tests for `device.rs` (device-code flows, 279 lines), config hot reload `watch()`, `oauth::exchange`, or `ui/config.js`; several WebSocket tests `recv().await.unwrap()` with no timeout, so a regression hangs CI instead of failing. | `src/device.rs`, `src/main.rs:293`, `src/ws_cancel_tests.rs:203-214` | M | Mocked device-code server, a `watch()` test with a temp config, `tokio::time::timeout` around the blocking receives. |

### 1.4 Operations and developer experience

| # | Finding | Effort | Fix |
|---|---|---|---|
| 19 | No Prometheus or OpenTelemetry metrics, no JSON log output, `/healthz` always says `ok` regardless of usage store errors or account state. | M | `/metrics` text exposition from `Stats` and account counters; `--log-format json`; `/readyz` that reports the usage DB and whether any account can serve. |
| 20 | No usage-DB backup command although rusqlite's `backup` feature is compiled in. | S | `fusebox usage backup <path>` (online, WAL-safe). |
| 21 | `--version` prints only the crate version; `Cargo.toml` on this checkout still says 0.3.2. | S | `build.rs` embedding git SHA and date into `--version` and `/api/overview`. |
| 22 | `docs/` mixes user docs with production verification notes that contain deployment-specific data (binary hashes, event counts, "deployed to production"). No `CONTRIBUTING`, `CHANGELOG` or `SECURITY.md`; CI has no `cargo audit`/`cargo deny` or dependabot. | S | Keep `banked-resets`, `notifications`, `usage-costs`, `usage-imports*`, `context-efficiency`, `usage-ui-contract`, `binary-size` in `docs/`; move the rest to `notes/`; add `SECURITY.md` and a weekly audit job. |
| 23 | Local checkout is stale: `main` is 11 commits behind `fork/main` (named clients, 0.4.0 release, client version bumps). | S | `git pull --ff-only fork main`. |
| 24 | Upstream IuCC123 has small fixes worth cherry-picking: label an unsupported `Content-Encoding` as a client error (`08954d6`), count quota readings past 100% as a full window (`23adc39`), send the recovery alert when usage stops reporting a window (`58d43c4`), keep usage checks at their usual pace when notifications are on (`96589cb`). | S | Review and cherry-pick; the notification commits only make sense with item 2.1 below. |

### 1.5 Already done well (leave alone)

Constant-time key comparison with cached bcrypt; loopback trust defeated by `Forwarded`, `X-Forwarded-For`, `X-Real-IP` and `tailscale-user-login` with tests; bounded state (request log 300, affinity 10k sessions / 64 MiB, WS queue 32 turns / 16 MiB, push subscriptions 32); compressed-body limits with 413/415 tests; coalesced per-account token refresh that survives client cancellation; usage store on its own thread with `try_send` backpressure, WAL and `synchronous=FULL`; atomic private session persistence; per-subscription HTTP/2 pools and TCP keepalive; WebSocket relay with write deadlines and scrubbed diagnostics; SSE keepalive comments every 15 s **verified**; a service worker that caches nothing authenticated; a config editor that preserves comments with revision-checked patches; `fusebox check` preflight.

---

## 2. New features

Effort: S = a day or two, M = about a week, L = several weeks. "Demand" cites CLIProxyAPI (CPA) issues and discussions, which is the largest tracker in this niche.

### 2.1 Notifications and integrations

| Feature | Why | Demand | Effort |
|---|---|---|---|
| **Webhook notifications**: generic JSON, Discord, Slack, Mattermost, Teams, Telegram, ntfy. | Push only reaches a browser or phone; servers and teams want chat alerts. Upstream IuCC123 shipped a complete implementation (14 commits, ~6.8k lines including tests, write-only credential store with 0600 files, HTTPS-origin checks, delivery log, test button) that can be ported rather than written. Reuse Fusebox's existing fault rules as the event source so push and webhooks share one definition. | CPA discussions #519, #2539, #825; plugins `lamplighter`, `cpa-bark-notifier`; LiteLLM and Bifrost ship Slack/Teams/webhook alerting | M (port) |
| **Scoped read-only status token and `GET /api/status`**: per-account windows, resets, cooldowns and faults in one compact document. `/api/accounts` already includes each account's quota **verified**, but only with the full management key. | The CPA ecosystem (cc-status-line, Quotio, Infinitus), teamclaude's `/teamclaude/quota` and CodexBar's `serve` mode exist because a readable, low-privilege quota endpoint is what widgets need. | CPA tools in 3.2; teamclaude; CodexBar | S |
| **Claude Code status line snippet** that reads the status API. | Shows remaining quota inside the terminal, which is where users are. | `cc-status-line`, ccusage statusline | S |

### 2.2 Client keys and governance

| Feature | Why | Demand | Effort |
|---|---|---|---|
| **Per-client policy on named clients**: model allowlist, requests per minute, tokens per window, daily or weekly budget in tokens or estimated USD, expiry date, optional account binding. Show usage per client on the Usage page (already recorded) and refuse with a clear 429/403. | Fusebox already has named keys and per-client usage in SQLite; the missing part is enforcement. This is the most repeated governance request on every tracker. | CPA discussions #1205 (8 upvotes), #2205, #2031, #2519, #4968, #5572, issue #6396 (open); nine CPA plugins; LiteLLM and Bifrost virtual keys | M |
| **Per-client "which account answered" view** and a read-only key-scoped usage page. | Shared VPS users want to see only their own traffic. | CPA Usage Keeper | S |
| **Allowed clients per key** (user-agent allowlist) and a **compliance mode** that sends only genuine Claude Code traffic to Claude OAuth accounts and everything else to API keys. | Stops unknown tools from consuming subscription accounts on a shared box, and is the concrete answer to Anthropic's terms (see 3.5). | claude-relay-service, claude-code-hub, sub2api's client restriction detector | S |

### 2.3 Routing and resilience

| Feature | Why | Demand | Effort |
|---|---|---|---|
| **`priority` on credentials**, read from CPA auth files and settable per account in the dashboard. Fusebox ignores the field today **verified**. | "Use Max before Pro", "API key last" without changing strategy. | CPA core field; discussions #692 (7), #1978 | S |
| **Cooldown persistence across restarts.** Cooldowns are in memory only **verified**. | A restart after an upgrade re-hits rate-limited accounts immediately. | CPA `save-cooldown-status`; stale-cooldown complaints are a top CPA pain point (#5586, 20 comments) | S |
| **Stream bootstrap buffering**: hold a 200 response until the first content event; if the first event is an error (`server_is_overloaded`, `rate_limit_exceeded`) and nothing was sent to the client, fail over to the next account. Today the error is recorded and rendered to the client **verified**. | ChatGPT returns overloads inside 200 streams; without this a retry budget is useless. | CPA `stream-bootstrap-buffering`; issues #5586, #5844 | M |
| **Per-request deadline and bounded retries** (see 1.1 #7), plus `max-retry-interval`: wait up to N seconds for a cooling account instead of failing when every account is briefly limited. | Predictable latency under failure. | CPA `routing.retry.*` | S |
| **Request-scoped error rules**: classify upstream errors by status and message pattern into stop, stop-and-cooldown, continue, continue-and-cooldown. A small built-in set (prompt too long = stop; invalid key = cooldown that account) covers most cases without a rules language. | Stops "prompt too long" from burning five accounts. | CPA `request-scoped-errors` | M |
| **Model groups / fallback chains / alias pools**: `model-groups: {fast-claude: [claude-sonnet-5-5@claude, claude-sonnet-4-6@antigravity, kimi-k3@openrouter]}` with ordered failover and round-robin inside the group. | The one routing feature CPA users keep asking for; today Fusebox only has vendor-first overflow. | CPA discussions #692, #1978, #3923, #4975; LiteLLM fallbacks; Bifrost `fallbacks` | M |
| **Payload defaults per model** (subset of CPA payload rules): force reasoning effort, `max_tokens`, `service_tier`/Claude `speed` for a model pattern. Fusebox has no `service_tier`↔`speed` mapping today **verified**. | Covers most real uses of CPA's rules engine without adopting the engine. | CPA `requests.payload.*`; plugins `prompt-rules`, `request-rewrite` | S-M |
| **Model capability declarations** for OpenAI-compatible and API-key entries: thinking levels, context length, image input, display name. | Lets custom models accept `(xhigh)` and advertise context to Codex. Not present today **verified**. | CPA `models[]` capability keys; issue #6099 | M |
| **Trusted-proxy CIDRs** instead of header heuristics for `X-Forwarded-For`. | Correct client IP behind Cloudflare or nginx; prerequisite for per-IP throttling. Not present **verified**. | CPA `server.trusted-proxies` | S |
| **Auto-spend a banked reset when the pool is dry** (opt-in). Fusebox already shows and spends them manually. | Removes a 3 a.m. decision. | underclass, sub2api `openai_quota_auto_reset` (#3248, 25 upvotes), codex-lb | S |
| **First-token deadline and per-error-class cooldowns**, with the reason and ETA shown per account. | Subscription upstreams accept and stall; re-routing before the agent's own timeout is the difference between a hiccup and a lost turn. | claude-relay-service per-class TTLs, LiteLLM `allowed_fails_policy`, underclass, new-api #6141, sub2api #2738 (143 comments) | M |
| **Task-shaped routing**: send Claude Code's background and title calls (small `max_tokens`, known system prompts) to a cheap or free pool, with header and body conditions. | Protects Opus hours from housekeeping calls. | claude-code-router `background`/`think`/`longContext`, Antigravity-Manager's silent downgrade, OpenRouter auto-router | M |
| **Keep-warm of the 5-hour window** on a schedule or when another account nears exhaustion. | A cold account's window only starts when rotation reaches it. Caveat: close to "circumventing usage limits"; decide under 3.5 first. | teamclaude `warmup`/`reset`, codex-lb limit warm-up, Codex-Manager, cockpit-tools | S-M |
| **Per-account egress proxy** for OAuth accounts (today only global and per API key **verified**). | Users with several accounts want distinct exits. Caveat: ban-evasion adjacent and reportedly ineffective (sub2api #1374). | claude-relay-service, sub2api, ccLoad, teamclaude; requested on Bifrost #2318 | S-M |

### 2.4 Providers and endpoints

| Feature | Why | Demand | Effort |
|---|---|---|---|
| **`/v1/embeddings`** (and `/v1/rerank`) through OpenAI-compatible, OpenAI, Gemini and Vertex keys. Not present **verified**. | Cheap; makes Fusebox the single base URL for RAG tools too. | CPA discussion #2810 (8 upvotes), #1828, #3924, issue #5961; LiteLLM, Bifrost, Portkey all have it | S-M |
| **GitHub Copilot** as a subscription provider (device-code OAuth). | Second most requested new subscription after Cursor; widely held through employers. | CPA discussions #5883 (6), #1490, #640; two CPA plugins; LiteLLM, Bifrost, `copilot-api` | L |
| **Sign in with ChatGPT account type** using OpenAI's sanctioned open-source token sharing: dynamic client registration, Responses API on api.openai.com with `store: false` and streaming. | The only ToS-clean ChatGPT path in this category; see 3.5 for what it gives up. | OpenAI SIWC docs (DevDay, 2026-09-29) | M |
| **Gemini CLI OAuth** (Google account, Code Assist free tier). Fusebox only has Gemini API keys and Antigravity. The Antigravity path already talks to `cloudcode-pa.googleapis.com` **verified**, so this is a second client id and endpoint on the same code. | Free Gemini quota without a key. Caveat: CPA removed it from core in mid-2026 (discussion #3918) and moved it to a plugin; check the reason before investing. | CPA plugin store | M |
| **Codex device-code login** (headless). Fusebox's Codex login is browser-only with paste-the-redirect **verified**. | Servers and Docker without a browser. | CPA `--codex-device-login` | S |
| **Kimi global (kimi.ai)** flow beside kimi.com **verified** (only `auth.kimi.com` today). | International Kimi Code users. | CPA `kimi-ai` | S |
| **Antigravity credits policy**: spend paid credits only after free quota, or never. Not present **verified**. | Cost control. | CPA `antigravity-credits` | S |
| Cursor, Kiro, ZCode/GLM, OpenCode Go/Zen, CommandCode, Trae as providers. | Real demand (Cursor 14 upvotes, Trae 23) but each is a reverse-engineered client protocol with ToS risk and churn. | CPA discussions #4613, #671, #4136 | L each, low priority |
| Grok TTS (`/v1/audio/speech`), OpenAI Videos through ChatGPT, Gemini Interactions API, Realtime. | Niche; OpenAI is retiring `gpt-realtime`. | CPA routes | M-L, skip for now |

### 2.5 Dashboard

| Feature | Why | Seen in | Effort |
|---|---|---|---|
| **Quota forecast**: burn rate per window, "runs out in 1h 40m at this pace", and a reset timeline across all accounts. | The raw meters are already the best in class; the forecast is what every monitor app adds. | CPA Management Center QuotaTimeline, Infinitus, Claude-Code-Usage-Monitor | M |
| **Config diff preview before save** in the YAML editor. | Prevents the clobber in 1.1 #10 and shows exactly what changes. | CPA Management Center | S |
| **Playground**: send a test prompt to any model and watch which account answers, with the routing explanation inline. | Verifies setup without leaving the dashboard. | Oh-My-CPA, LiteLLM, Bifrost, Portkey console | M |
| **Cooldown snapshot per account with a manual clear**, and inline edits of prefix, aliases, excluded models and priority. | The palette can clear cooldowns, but per-account policy edits require the YAML editor. | CPA Management Center | S-M |
| **Served-vs-requested model flag and tokens per second** in the request log. | Makes silent aliasing and slow accounts visible. | Oh-My-CPA, CPA-Manager-Plus | S |
| **More client snippets** under Config, Clients: Cursor, Cline, Roo Code, OpenCode, Zed, Continue, Aider, Gemini CLI, JetBrains. Today only Claude Code, Codex, the OpenAI SDK and curl **verified**. | One URL for every client is a product principle; the page should prove it. | ProxyPal agent auto-setup, Bifrost CLI | S |
| **Onboarding checklist** on first run (add account, send first request, set a key, install the app). | CPA's top usability complaints are first-run confusion (#1564, #2064); Fusebox already avoids the worst of it, a checklist finishes the job. | ProxyPal | S |
| **Light theme.** `style.css` defines 34 custom properties but also uses 34 hard-coded hex values outside `:root` **verified**. | Accessibility and daylight use; every competitor dashboard has one. | all | M |
| **Localisation** (zh-CN first, then others). | The user base for this category is heavily Chinese-speaking; CPA's panel ships six languages. | CPA Management Center | M |
| **Audit trail** of sensitive dashboard actions (reveal key, remove account, config save). | Shared VPS accountability. | Oh-My-CPA, LiteLLM, Bifrost (enterprise) | S |
| **Pace marker on every meter** (a tick at even use, red past it, signed delta) and the **countdown beside the absolute reset time**. | The most converged idea across quota monitors; the 20-segment meters can take it directly. | claude-powerline, CodexBar, ccstatusline, Claude-Code-Usage-Monitor #167 (20 upvotes) | S |
| **Reset-credit inventory with expiry warnings** ("2 resets banked, 1 expires in 47h"). | Extends the banked-resets chip. | CodexBar, ai-usagebar, token-monitor | S |
| **Provenance and staleness labels** on quota numbers (official, estimate, unknown; snapshot age). | Honest when one account's usage fetch fails. | Claude-Code-Usage-Monitor v4, CodexBar, quota-float | S |
| **Provider incident strip** from status.claude.com and OpenAI's status page beside the faults button. | Separates "we are throttled" from "they are down". | ccstatusline, CodexBar | S |

### 2.6 CLI, packaging and deployment

| Feature | Why | Seen in | Effort |
|---|---|---|---|
| **`fusebox configure claude\|codex\|opencode\|cursor`** writes the client's config to point at Fusebox (and a `--print` mode). | One command instead of copying snippets. | Bifrost CLI `configure`, ProxyPal | S-M |
| **Homebrew tap, AUR package, `curl \| sh` installer**, Windows service and launchd notes. | Binaries and Docker exist; package managers are how people actually install. | CPA has brew and AUR | S-M |
| **Notarised macOS binaries.** The release notes tell users to strip quarantine by hand. | First-run friction. | | M (needs an Apple developer account) |
| **Remote config and credential store** (S3 or Git) for stateless deploys. | Only for cloud-hosted users; the stated audience is laptops and small VPSs. | CPA git/pgsql/s3 stores | L, later |
| **CPA management-API compatibility subset** (`auth-files`, `usage-queue`, `api-call`) so ProxyPal, Quotio and similar front-ends work. | Instant ecosystem; but it ties Fusebox to CPA's API shape. The status API in 2.1 is the cleaner alternative. | ProxyPal source | M, optional |

### 2.7 Usage and costs

| Feature | Why | Seen in | Effort |
|---|---|---|---|
| **Pricing auto-sync** from models.dev (then LiteLLM) with a local override table. The bundled `src/usage/rates.json` (`2026-10-07.3`) is static **verified**. | Estimates go stale with every model launch. | CPA-Manager-Plus, cc-switch, claude-code-hub, ccusage | S |
| **Extra-usage budget**: parse Anthropic's `extra_usage` field (not parsed today **verified**; the per-model weekly windows already are) and show a daily budget: remaining credit divided by remaining days. | Max users now pay for overage. | ccstatusline, CodexBar | S |
| **5-hour block timeline** on the provider's real reset boundaries with per-block cost and a live $/hour. | Shows which block a session burned. | ccusage blocks, rz1989s statusline | M |
| **Opt-in capture of the last N failed requests' bodies**, off by default. | Translation bugs are undebuggable from metadata alone. | ccflare, claude-code-router, AIClient2API, LiteLLM, Bifrost | M |

Out of scope by principle (keep saying so in the README): Go plugins, a payload rules language, Redis queues, browser TLS fingerprinting, an MCP gateway, SSO.

---

## 3. Competitors

### 3.1 Landscape

| Project | What it is | State (2026-10) | What matters for Fusebox |
|---|---|---|---|
| **CLIProxyAPI** (router-for-me, Go) | The original subscription pool. | 55k stars, v8.0.27 on 2026-10-12, 150 releases in three months, 115 plugins in a signed store, ~30 ecosystem GUIs and widgets. | Direct competitor and compatibility target. Core lacks quota routing, usage storage, notifications and named keys; it wins on policy knobs, request shaping, client fidelity and ecosystem. |
| **CLIProxyAPI-Rust** (IuCC123) | Fusebox's upstream. | Diverged; 14 commits since the fork, mostly webhook quota notifications. | Port the notifications; cherry-pick four small fixes. |
| **cliproxy-rs** | Another Rust port of CPA. | Advertises a `soonest-reset` strategy as its differentiator. | Confirms demand for quota-aware routing, which Fusebox already has. |
| **LiteLLM proxy** (Python) | General gateway. | Subscription support is single-account per vendor (ChatGPT, Copilot, Claude token passthrough); its Rust gateway covers about 5% of its API. | Borrow: virtual keys with budgets, Prometheus/OTel, alerting, fallback chains, `/health/readiness`, playground. Not a pooling competitor. |
| **Bifrost** (maximhq, Go) | Single binary gateway with embedded UI. | Active. Open-source virtual keys with budgets, rate limits, expiry and model allowlists; failure-class-aware key rotation; session affinity adopted from Claude Code and Codex headers; `bifrost configure claude|codex`. Cooldown persistence and alerts are enterprise-gated. No OAuth pooling. | Closest in product shape (one binary, UI inside). Borrow its governance and CLI ideas. |
| **Portkey Gateway** (TypeScript) | Routing primitives. | Community reports it is no longer actively maintained (fork "Lightport"); hosted product rebranded under Palo Alto Networks. | Not a threat. |
| **Plano** (ex-archgw, Rust + Envoy) | Preference-based router. | Active. Has single-account ChatGPT OAuth, prompt-cache-aware affinity, cost and latency ordering. No keys, budgets, pooling or UI. | Validates affinity-for-cache as a feature; not a competitor. |
| **Helicone AI Gateway** (Rust) | Self-hosted gateway. | Dormant since 2025-07, relicensed GPL. | None. |
| **new-api** (QuantumNous, Go) | Billing-centric multi-channel gateway with an embedded console; successor of one-api. | Very active (daily commits, rc.44 on 2026-10-12). Has a Codex channel that runs ChatGPT OAuth tokens with refresh and usage-window readout, and a Sub2API channel; no Claude, Antigravity or Gemini OAuth. Auto-disables failing channels and re-tests them on a schedule, channel affinity rules, user alerts by email, webhook, Bark and Gotify, an audit log, seven languages. No Prometheus, no first-byte timeout. | Shows where subscription pooling is heading in the Chinese ecosystem; borrow the scheduled re-test of cooled accounts and the alert channels. |
| **TensorZero** (Rust) | Gateway plus experimentation. | Archived 2026-06, "no longer maintained". | Reference only, for per-request TTFT timeouts and JSON-pointer cost rules. |
| **one-api** (Go) | Ancestor of new-api. | Dormant since 2025-02. | None. |

### 3.2 CLIProxyAPI in detail

**Features CPA has that Fusebox lacks** (full table and config keys in the research notes; the items below survived source verification):

- Routing and policy: per-credential `priority`, weighted round-robin, retry rounds and caps, cooldown opt-out and persistence, transient-error cooldown length, request-scoped error rules, session-affinity TTL and subagent toggle, alias pools, `fork`/`display-name`/`force-mapping` on OAuth aliases, per-model capability declarations, custom model catalog sources, Antigravity credits policy.
- Request shaping: payload rules, dynamic client-header copy, stream bootstrap buffering for in-stream overloads, Codex client-fidelity knobs, Codex `tool_search` to Anthropic deferred tools, `service_tier` to Claude `speed`, image generation policy, upstream header passthrough to clients.
- Cloaking: per-key cloak modes (`auto`/`always`/`never`), strict mode, sensitive-word obfuscation, fingerprint profiles for API keys, pinned device profiles, uTLS.
- Providers and endpoints: AI Studio WebSocket relay, Kimi global, Codex device-code login, Claude multi-org credentials (unverified), Interactions API, Realtime, Grok TTS, Codex alpha search, OpenAI videos; plugins for Cursor, Copilot, Kiro, ZCode, OpenCode Go, CommandCode, CodeBuddy.
- Ops: git/PostgreSQL/S3 config and credential stores, `MANAGEMENT_PASSWORD` env, trusted-proxy CIDRs, body-size limit, mDNS discovery, file logging with rotation, pprof, usage event queue, TUI, Homebrew and AUR packages, the Home control plane for multi-node teams.
- A ~100-route management API that ProxyPal, the Management Center, Quotio, cc-status-line and others consume.

**Parity** (do not claim as strengths): effort and budget suffixes in model names, wildcard `excluded-models` **verified**, config hot reload with comment-preserving edits, Codex WebSocket relay, images and xAI video, compaction, Docker paths, `proxy-url` with per-credential `direct`, TLS, bcrypt management key, SSE keepalives **verified**.

**What Fusebox has that CPA core lacks** (verified against CPA's config and docs): quota-aware routing (CPA has round-robin, weighted and fill-first only; users install eight scheduler plugins); session affinity on by default and persisted (CPA defaults to off, 1 h TTL, and only added quota-only rebinding on 2026-10-11); the dashboard inside the binary (CPA downloads its panel from GitHub at first access and requires a key even on localhost, the root of its top usability complaints); usage and costs in SQLite with history imports (CPA removed built-in usage in v6.10.0); push notifications and an app badge; banked resets spent by the proxy; named client keys server-side; a request log with routing explanations; live model discovery per account (CPA ships a static catalog and bumps a release per new model; "add model X" issues are among its most-reacted); PWA install; vendor-first overflow out of the box; `fusebox check`; keyless localhost management.

**User demand on the CPA tracker, ranked** (reactions and comments are small in absolute terms; the plugin store is the better signal of sustained demand): first-run confusion (covered by Fusebox); degraded answers and bans for third-party clients, which drives CPA's fingerprint work (Fusebox: partial, no uTLS by design); day-one new models (covered); image and video generation (mostly covered, no Veo); streaming stalls, overloads and stale cooldowns (gap: bootstrap buffering, cooldown persistence); Trae, Cursor, Copilot, Zed, Perplexity, Bedrock as providers (gaps); persistent usage and cost (covered); per-key model allowlists and limits (gap); subscription-plan providers like OpenCode Go and GLM (only via compatible keys); priority and fallback chains (gap); quota-aware scheduling (covered); embeddings (gap); quota notifications to Telegram or webhooks (gap); weighted balancing (out of scope); per-account concurrency caps and window warmers (gap, low priority); banked-reset automation (covered).

### 3.3 General gateways: transferable ideas

- **Virtual keys** (LiteLLM, Bifrost, Portkey hosted): budget with reset period, requests and tokens per window, expiry, model allowlist, alias rewrite, tags. Bifrost does this in open source with hierarchical key, team and customer budgets. Fusebox's version should stay small: policy on named clients, no teams.
- **Failure classes** (Bifrost): credential, quota, model access, rate limit, 5xx each get a different rotation rule, and attempts are not charged to budgets. Fusebox's `soft_failure` already distinguishes some; naming the classes in the request log and the routing explanation would help users.
- **Health and readiness** (LiteLLM, Bifrost, Plano): `/health` returns 503 when the store or config check fails. Fusebox's `/healthz` is unconditional.
- **Prompt-cache auto-injection** (Bifrost `prompt_cache.auto_inject`): Fusebox already applies automatic caching on translated and cloaked requests; parity.
- **Client launchers** (Bifrost CLI, ProxyPal): `configure claude|codex` writing the client's settings; a temporary settings file for one-off runs.
- **Alerting** (LiteLLM, Bifrost enterprise): Slack, Teams, PagerDuty and webhooks on budget thresholds and cooldowns. Fusebox's fault rules are already the right event source.
- **Observability** (all): Prometheus and OTel are table stakes for anyone running on a VPS with Grafana.
- **Playground** (LiteLLM, Bifrost, Portkey console, Oh-My-CPA): a test request page with provider picker.

### 3.4 Relay projects and usage monitors

Subscription-pooling projects, with today's numbers (`gh repo view`, 2026-10-13):

| Project | Stars | State | Has, and Fusebox lacks | Lacks, and Fusebox has |
|---|---|---|---|---|
| **sub2api** (Wei-Shaw, Go+Vue, Postgres+Redis), successor of claude-relay-service (12.7k) | 43.9k | Releases every few days; built for operators who resell quota (payments, affiliates); "Simple Mode" is its only personal-use mode. | Per-key 5-hour, daily and 7-day spending windows; users with LDAP and TOTP; eight notification channels in claude-relay-service (Telegram, Bark, Discord, Slack, Feishu, DingTalk, SMTP, WeChat); per-account egress proxy; allowed clients per key; auto-reset of banked ChatGPT resets; fingerprint and version-sync services; body capture. | Single binary; quota-aware routing (sub2api is sticky plus weighted); PWA and push; usage imports; keyless localhost. |
| **9router** (Next.js, SQLite) | 30.7k | v0.5.35 (2026-07); core package private; trust complaints (obfuscated `mitm/server.js`, antivirus flags). | Three-tier model combos; GitHub Copilot; "RTK" tool-result compression on by default; Cloudflare Workers deploy. | Persisted affinity, per-model cooldowns, TLS, open code. |
| **claude-code-router** (TS, Electron) | 37.7k | v3.1.3 (2026-10-09). | Task-shaped routing (`default`, `background`, `think`, `longContext`, `webSearch`, header and body conditions); ordered fallbacks; client keys with expiry and request, token and image limits; body logging. | Any subscription pooling: its users ask for Claude (#482, 20 upvotes) and ChatGPT (#543, 28) OAuth. |
| **cc-switch** (Rust/Tauri) | 143.8k (API-confirmed, but only 263 watchers) | v4.0.11 (2026-10-13). | Desktop app; failover queue; Copilot accounts; models.dev pricing import; history import. | Cannot route official Claude subscriptions; no Antigravity (#2959, 44 upvotes); no server mode. |
| **codex-lb** (Python) | 3.3k | Active. | Eight routing strategies including "prefer earlier reset" and latency weighting; limit warm-up; per-key limits by token, cost, window and model; TOTP and SSO; banked-reset button. | Claude, Antigravity and the other providers; single binary. |
| **teamclaude** (JS) | 0.4k | Active; ships `docs/compliance.md`. | `GET /teamclaude/quota` for status lines; `quota.forecast` ("week reaches 98% in ~31h"); keep-warm scheduling; per-account SOCKS. | Everything else. |
| **underclass** (Rust) | 0.2k | Active. | Codex plus Copilot pooling; pre-first-byte failover only; auto-redeems a banked reset when every account is out. | Claude, Antigravity, a dashboard. |
| **copilot-api** (ericc-ch) and the caozhiyuan fork | 4.1k / 1.1k | Original abandoned 2025-10; fork active (v2.7.15, 2026-10-13). | Copilot subscription as OpenAI and Anthropic APIs; `/usage` endpoint. | Pooling and a dashboard. Its README carries an abuse-detection warning and #84 documents suspensions. |
| **claude-code-hub** (TS, Postgres+Redis) | 3.4k | v0.9.7. | Per-key RPM and spend per 5 hours, week and month; allowed clients with version checks; "FAKE_200" soft-error detection. | Cross-format translation (strict same-format by design). |
| **antigravity-claude-proxy** (JS) and **Antigravity-Manager** (Rust/Tauri) | 4.0k / 32.2k | Active; README: "Google has been issuing ToS violation bans"; the ban thread (#277) has 330 comments. | Tier-and-reset-aware Antigravity routing; silent downgrade of Claude Code's background calls to Flash; draggable per-model quota-protection thresholds. | Multi-provider pooling. |
| **ccflare** (TS/Bun) | 1.1k | Dormant since 2026-04. | Desktop app and TUI; body capture. | Translation (by design); per-model cooldowns. |

Convergence, features in two or more of these that Fusebox lacks: per-key budgets and limits; fallback chains; keep-warm of the 5-hour window (teamclaude, codex-lb, Codex-Manager, cockpit-tools); a quota endpoint for status lines; burn-rate forecast; chat-channel alerts; Prometheus; task-shaped routing; per-error-class cooldowns with a first-token deadline; auto-redeem of banked resets; allowed clients per key; per-account egress proxy; pricing auto-sync; opt-in body capture for failed requests; GitHub Copilot. All of these appear in section 2.

Usage monitors, and what they show that Fusebox's meters do not:

- **ccusage** (19.0k, Rust since v20): a 5-hour block timeline on the provider's real reset boundaries with per-block cost and a live $/hour; cost modes (`auto`, `calculate`, `display`).
- **Claude-Code-Usage-Monitor** (8.7k): a "tokens will run out at" prediction; a provenance layer (`official`, `local_estimate`, `unknown`) with staleness.
- **CodexBar** (22.5k, about 95 providers): pace markers with an even-use guide; burn-down widgets; reset-credit inventory with expiry; hooks with an event schema (`quota_low`, `quota_reached`, `quota_reset`, `provider_unavailable`, `provider_recovered`); a `serve` mode that feeds about 15 widgets.
- **ccstatusline** (13.3k): per-model weekly buckets (Opus, Sonnet); an extra-usage daily budget (remaining monthly credit divided by remaining days); a provider incident strip from the status pages.

Fusebox already parses the per-model weekly windows (`seven_day_opus`, `seven_day_sonnet` in `src/quota.rs`) **verified** but not Anthropic's `extra_usage` field.

### 3.5 Threats and positioning

**Anthropic's terms now name this category, and Anthropic enforces them.** The Claude Code legal page (https://code.claude.com/docs/en/legal-and-compliance, fetched 2026-10-13) says OAuth "is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications"; that "Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens"; that "Advertised usage limits for Pro and Max plans assume ordinary, individual usage"; and that "Anthropic reserves the right to take measures to enforce these restrictions and may do so without prior notice." It also says nothing prevents "an end user from signing in to the unmodified Claude Code binary with their own Claude subscription." Fusebox stores Claude.ai OAuth tokens, pools several accounts, and by default presents other clients as Claude Code (`claude-cloak: true`), which is the "spoofing the Claude Code harness" behaviour an Anthropic engineer described as the target of the January 2026 server-side block (The Register, 2026-02-20; OpenCode removed Claude OAuth after legal requests). Ban reports in the relay ecosystem are continuous (claude-relay-service #1048, sub2api #1374 even with per-account residential IPs, teamclaude #529 in October 2026). A secondary source quotes the same engineer saying several Max accounts per person are not a violation and that enforcement targets reselling (**unverified**).

What this means for Fusebox:

- Say plainly in the README who it is for: one person, their own subscriptions, on their own machines. Never add per-user billing, key sales or multi-tenant features.
- Turn `claude-cloak` off by default and require a per-account acknowledgement to turn it on; keep genuine Claude Code passthrough unchanged.
- Add a compliance mode: only requests genuinely from Claude Code (user agent and version) reach Claude OAuth accounts; everything else goes to API keys. The allowed-clients feature in 2.2 is the same mechanism.
- Keep the client-version workflow, since a stale identity is the most likely trigger, and keep the FAQ honest about blocks.
- teamclaude's `docs/compliance.md` is a model for a short, honest page.

**Google bans Antigravity proxy users.** Google's maintainer statement of 2026-02-27 (https://github.com/google-gemini/gemini-cli/discussions/20632) says bans targeted "the use of 3rd party tools or proxies to access Antigravity resources and quotas", that "bypassing system measures or circumventing usage limits is prohibited", that a second violation is permanent, and that bans also cut Gemini CLI and Code Assist. antigravity-claude-proxy's ban thread (#277) has 330 comments, and the "This version of Antigravity is no longer supported" gate hit every proxy; Fusebox's own Antigravity version issues (#14, #25) show it is on the same treadmill. Say so on the Connect account panel for Antigravity, and treat the Gemini CLI OAuth idea (2.4) as carrying the same risk; CLIProxyAPI dropped it from core.

**OpenAI is the one provider with a sanctioned path, and Fusebox qualifies.** "Sign in with ChatGPT" plan usage (https://developers.openai.com/siwc/token-sharing-open-source, fetched 2026-10-13) is documented "for open-source and locally hosted apps"; paid or remotely hosted apps go through an interest form. Requests go to the Responses API on api.openai.com with `store: false` and streaming, a client can have several hosts sharing one usage configuration, users cap each app in ChatGPT settings, and the preview-limitations page lists what is missing (no `previous_response_id` over HTTP, no image generation, no hosted tools, fewer parameters). A SIWC account type in Fusebox would be the only ToS-clean ChatGPT path in this category, at the cost of the WebSocket relay, server-side `previous_response_id` and `gpt-image-*` for those accounts. The current `chatgpt.com/backend-api/codex` path has no public OpenAI statement either way (**unverified**). Headwinds: Codex login now step-ups phone verification (openai/codex #25820) and a reported Pro allowance cut after 2026-10-29 (**secondary source**).

**Competitor momentum.** CLIProxyAPI releases every one or two days and has a 115-entry plugin store, so feature-by-feature chasing is a losing game; Fusebox's lane is "batteries included, one binary, no plugins": ship the plugin categories users actually install (quota scheduling, per-key policy, notifications, usage dashboards, reset automation) natively and say so. sub2api went from 0 to 43.9k stars in ten months but needs Postgres and Redis and targets resellers. cc-switch is the desktop default for many users but cannot route official Claude subscriptions. 9router has the most aggressive feature set and trust problems. claude-code-router owns "Claude Code to other models" and its users keep asking for pooling. codex-lb has the richest routing vocabulary. Demand validation from the general gateways: LiteLLM's second most requested feature is multiple ChatGPT OAuth accounts (#23777, 45 upvotes) and Bifrost's top three requests are all OAuth subscriptions. The general gateways are not converging on this niche: LiteLLM's Rust rewrite is years out, Portkey is unmaintained, Helicone's gateway is dormant, TensorZero is archived, and Bifrost and Plano avoid consumer OAuth.

**Hosted alternatives that remove the need to pool.** OpenCode Go ($10 or $40 a month, open models only, supports Claude Code and Codex as clients) and OpenRouter BYOK with fallback to credits are ToS-clean. Fusebox's answer is "your existing plans plus a legitimate spill-over", which is fallback chains (2.3) plus per-client budgets (2.2).

**Compatibility is an asset.** Auth files, config layouts, Docker paths and flags carry over from CLIProxyAPI, and TensorZero, Portkey, Helicone, one-api and the original copilot-api have all gone dormant or been archived within a year, so being the maintained drop-in matters. The audience is bilingual: CLIProxyAPI's panel ships six languages and most of its tracker is Chinese-language.

**Unverified in this section:** the Anthropic engineer's "multiple Max accounts" quote (secondary source); OpenAI's position on `backend-api/codex` proxies; the Pro allowance cut and the reported Antigravity model removal on 2026-11-02 (secondary sources); cc-switch's star-to-watcher ratio.

---

## 4. Suggested roadmap

**First (this week):** decide the compliance posture (3.5): README wording, the `claude-cloak` default, and whether to build the compliance mode. Everything below assumes the single-owner use case.

**Now (one to two weeks):** fast-forward `main`; security fixes 1.1 #1-#6, #8, #10; `priority` field; cooldown persistence; trusted-proxy CIDRs; scoped read-only status token and a status-line snippet (quota is already in `/api/accounts`); opt-in auto-spend of banked resets when the pool is dry; pricing auto-sync from models.dev with an override table; more client snippets; `fusebox usage backup`; `--version` with SHA; cherry-pick the four upstream fixes; `SECURITY.md` and `cargo audit` in CI.

**Next (one to two months):** port webhook notifications; per-client policy on named clients, including allowed clients; stream bootstrap buffering, bounded retries and a first-token deadline; model groups and alias pools; payload defaults per model; `/v1/embeddings`; pace markers, burn-rate forecast and a reset timeline; config diff preview; playground; `fusebox configure`; Prometheus, JSON logs and `/readyz`; Homebrew and AUR; a Sign in with ChatGPT account type.

**Later:** task-shaped routing; opt-in capture of the last failed requests; GitHub Copilot (abuse-detection risk) and Gemini CLI (ban risk) providers; keep-warm scheduling only if the compliance posture allows it; light theme and localisation; model capability declarations; the `proxy.rs`, `accounts.rs` and `app.js` splits with the test improvements; remote credential stores and a CPA management subset only if hosted or GUI users ask.

---

## Sources

- CLIProxyAPI: https://github.com/router-for-me/CLIProxyAPI (releases, `config.example.yaml`, `internal/api/server_routes.go`), docs https://help.router-for.me/, plugin registry https://github.com/router-for-me/CLIProxyAPI-Plugins-Store/blob/main/registry.json, Management Center https://github.com/router-for-me/Cli-Proxy-API-Management-Center, Home https://github.com/router-for-me/CLIProxyAPIHome. Discussions and issues cited by number above.
- Ecosystem: ProxyPal https://github.com/heyhuynhgiabuu/proxypal, Oh-My-CPA https://github.com/WizisCool/oh-my-cpa, CPA-Manager-Plus https://github.com/seakee/CPA-Manager-Plus, CPA Usage Keeper https://github.com/Willxup/cpa-usage-keeper, EasyCLIProxyAPI https://github.com/router-for-me/EasyCLIProxyAPI.
- Upstream Rust: https://github.com/IuCC123/CLIProxyAPI-Rust (`origin/main`, commits `a4a31a3`..`b07936b`, `docs/notifications.md`).
- LiteLLM: https://docs.litellm.ai/docs/proxy/virtual_keys, /proxy/reliability, /proxy/prometheus, /proxy/alerting, /proxy/health, /proxy/rust_gateway, /providers/chatgpt, /tutorials/claude_code_max_subscription.
- Bifrost: https://docs.getbifrost.ai/features/governance/virtual-keys, /features/retries-and-fallbacks, /providers/session-affinity, /quickstart/cli/coding-agents, /features/telemetry.
- Portkey: https://github.com/Portkey-AI/gateway (issues #1739, #1759), https://docs.portkey.ai/docs/product/product-feature-comparison.
- Plano: https://github.com/katanemo/plano (`cli/planoai/chatgpt_auth.py`, docs sources).
- Helicone: https://github.com/Helicone/ai-gateway (issue #298, commit history).
- Relay projects: https://github.com/Wei-Shaw/sub2api, https://github.com/Wei-Shaw/claude-relay-service, https://github.com/decolua/9router, https://github.com/musistudio/claude-code-router, https://github.com/farion1231/cc-switch, https://github.com/Soju06/codex-lb (docs https://soju06.github.io/codex-lb/routing/), https://github.com/KarpelesLab/teamclaude (docs/quota.md, docs/compliance.md), https://github.com/ghuntley/underclass, https://github.com/ericc-ch/copilot-api, https://github.com/ding113/claude-code-hub, https://github.com/badrisnarayanan/antigravity-claude-proxy, https://github.com/lbjlaq/Antigravity-Manager, https://github.com/snipeship/ccflare, https://github.com/QuantumNous/new-api.
- Usage monitors: https://github.com/ccusage/ccusage, https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor, https://github.com/steipete/CodexBar (docs/configuration.md, docs/ui.md), https://github.com/sirmalloc/ccstatusline (docs/USAGE.md), https://github.com/Owloops/claude-powerline.
- Policy: https://code.claude.com/docs/en/legal-and-compliance (fetched 2026-10-13), https://www.theregister.com/2026/02/20/anthropic_clarifies_ban_third_party_claude_access/, https://github.com/google-gemini/gemini-cli/discussions/20632, https://developers.openai.com/siwc/token-sharing-open-source and https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt (fetched 2026-10-13), https://opencode.ai/docs/console/go, https://openrouter.ai/docs/use-cases/byok.
