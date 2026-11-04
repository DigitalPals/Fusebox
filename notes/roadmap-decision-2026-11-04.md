# Roadmap decision, 2026-11-04 (Claude Opus 5.5, max effort)

Input: the 2026-11-04 research list (groups A–D). Owner constraints: the four
verified gaps (A1–A4) are mandatory; "implement everything" was allowed.
Verdict: **a named subset, not everything.** Checked against main `f27f183`.

## Implement, in waves

- **0.1** tokio 1.53.1 → 1.53.3, lockfile only (closes #47).
- **0.2** Record dropped client header *names* (bounded, values never stored)
  in `inbound_headers` for genuine Claude Code/Codex clients; show in
  `GET /api/client-identities`.
- **0.3 (A4)** iOS stale state: generation counter in `AppStore.stop()`,
  cancel `accountsRefresh` + untracked refresh task, `connect()` stops first,
  guard every post-await write incl. widget/badge/Live Activity. New Swift
  Testing file; validate on macOS, next TestFlight after the 1.0 review.
- **0.4** B5 as `docs/moving.md` instead of backup/restore code.
- **1.1 (D3)** Client reviews (#48): Claude Code 2.1.316 and Codex 0.166.0
  first (header allowlists affect production traffic), one header per commit.
- **1.2 (D1)** `proptest` dev-dependency only (no cargo-fuzz; CI is stable):
  (a) YAML round-trip/differential — required before 3.1; (b) SSE chunking;
  (c) format parse never panics + round-trip stability; (d) schema cleaner.
- **1.3** Dashboard `fn()`/`line()` test helpers must throw on missing markers.
- **2.1 (A2)** `model-map` on named clients: case-insensitive match on the
  suffix-stripped base model; exact beats wildcard, longest literal wildcard
  wins, ties are a config error; target may carry a suffix; no `*` targets,
  no chaining. Order: map → allowlist (on mapped) → background-model. One
  choke point `admit_model` replacing `policy::check_model` at all call sites
  (proxy, WebSocket — mapped takes `Native::Fallback`, media/compact,
  embeddings). Fix `ui/config.js` `saveLimits` to clone the client (it would
  silently drop the map). Guards: early return when no maps; old binaries
  refuse configs using it (`deny_unknown_fields`) — check before production.
- **2.2 (B2)** Capture Claude `server_tool_use.web_search_requests` /
  `web_fetch_requests` and Responses `web_search_call` items as allowlisted
  `numeric_metadata` keys; price per-call fees from a new bundled fee table
  (web fetch is tokens only); unknown fee → partial row, token cost kept.
  Never add fields to `Observation`/`Tokens`/`Rate`/`Catalogue`
  (`deny_unknown_fields`, persisted — rollback safety).
- **3.1 (A3b)** Replace `serde_yaml` (closes #42), after 1.2(a). `src/yaml.rs`
  facade over `serde_json::Value` trees; `yaml-edit` untouched (rowan only).
  Crate spike against the differential harness (saphyr/serde-saphyr or
  yaml-rust2; reject serde_yml/serde_norway). Match today: YAML 1.2 core
  schema, duplicate keys error, aliases resolve, `<<` plain key, non-string
  keys error, empty doc = empty mapping. Ship both parsers in one commit
  with a differential corpus, then remove `serde_yaml`; verify
  `cargo tree -i unsafe-libyaml` empty and `cargo +1.88 check --locked`.
- **4.1** Golden upstream requests (URL, headers, payload bytes; format ×
  wire × passthrough/cloaked/translated), tests only. Before 4.2.
- **4.2** Extract `plan_attempt(…) -> Result<Plan, Refusal>` from
  `execute_inner` (proxy.rs ~1529–1750), side-effect free. Goldens must stay
  byte-identical.
- **4.3 (A1)** `POST /api/inspect` (management auth): effective model, route
  steps + chosen account (`route_order` is read-only), passthrough/translated,
  redacted URL/headers/body, dropped controls, refusal if any. Must never
  send, refresh tokens, record usage/logs/metrics, touch affinity/cooldowns,
  call `clients::observe_trusted`, or create provider sessions. UI: "Paste a
  request / Inspect, don't send" in Try a model; playground moves to
  `ui/playground.js` (the targeted D2 piece, needs 1.3).
- **5.1 (B6 trimmed)** `GET /v1/fusebox/status` (+ `?format=line`) for any
  accepted client key (collectors refused; no new token type). Pool-level
  only via `push::current_pools`; no emails/labels/ids; no CORS; doesn't
  count as a request. Docs: Claude Code statusLine snippet.
- **5.2 (B1 part 1)** Fault for refused usage checks: ≥3 consecutive 401/403
  over ≥30 min after a successful refresh ⇒ existing `error` fault, display
  only, push only under `account-errors` (off by default). Today failures
  are debug-logged only (`quota.rs:880`).
- **5.3 (B1 part 2)** `POST /api/accounts/{id}/test` + drawer Test button:
  one minimal pinned request, no failover (`pinned` flag only this endpoint
  can set), cheapest listed model, recorded as `dashboard`, once per
  account per minute, UI warns it may start a 5-hour window.

Rules: at most one request-path change per production rollout; each rollout
via the existing updater with qualification + rollback.

## Cut / deferred, with reasons

- **B1 scheduled probes / `check --probe` / probe matrix: cut.** Final
  refresh failures already hold accounts out with a default-on fault; bot
  protection hits sign-in token exchange, which an inference probe with an
  existing token never exercises. Synthetic traffic spends quota and starts
  5-hour windows. A CLI probe would race the server's credential writes.
- **B3 slow-account fault: deferred until measured.** Latency samples exist
  only with `latency-routing` on (default off). First query production
  `request_ttft_ms` for sustained ≥3×-sibling episodes; build only if real.
- **B4 weekly digest: deferred.** Notifications are for action; the digest
  duplicates dashboard/widgets and adds a notification type in three UIs.
- **B5 in code: cut** (→ 0.4 docs). A whole-state archive is high-value
  secret material; restore invites two instances fighting over rotating
  refresh tokens.
- **C1 payload rules: cut.** Foot-gun on the native path; the common uses
  are covered by model suffixes and A2. Declared out of scope in the README.
- **C2 peer mode: cut.** Correct peer forwarding (Messages/Responses/WS/
  compaction/count_tokens/media) is L, double-counts usage/attribution,
  conflicts with one-proxy-per-auth-dir. Point clients at the VPS URL.
- **C3 audio: cut.** No demand; multipart + duration/character pricing.
- **C4 TLS fingerprint: cut.** BoringSSL toolchain conflicts with the
  one-binary story; evades provider bot protection. Optional S: detect a
  challenge at token exchange and suggest desktop sign-in + copying the
  credential file.
- **C5 SIWC: deferred** until OpenAI policy text explicitly covers a local
  proxy; production Codex traffic gains nothing.
- **D2 broad splits: deferred.** Only the two A1-driven extractions (4.2,
  `ui/playground.js`).

## Corrections found while verifying

- B2's mechanism: Claude server-tool rows are *not* flagged
  `unsupported_modality_or_tools` (that fires only on audio/image/tool-token
  metadata). Nothing reads `server_tool_use`, so the undercount is silent.
- A1 is M, not S: the build sequence is inline in `execute_inner`.
- A4 is worse than described: `connect()` never cancels the old socket loop,
  and late results rewrite widget snapshot/badge/Live Activities.
- D3: identity learning covers versions, not header shape; both native
  allowlists drop unknown headers with only a debug log.

Full verdict text: the delegated Opus 5.5 thread "Opus 5.5: decide Fusebox
implementation roadmap" (2026-11-04).

## Build status, 2026-11-04

Waves 0.1–0.4, 1.2, 1.3, 2.1, 2.2 and 5.1–5.3 were built first (Muse); the rest
followed (Claude Opus 5.5). Committed on main as one commit per item
(590fc4b…879e583), with the second review's fixes on top; not pushed or
deployed.

- **1.1** All six clients in #48 reviewed with no protocol change needed;
  identities bumped: Claude Code 2.1.318 (fingerprint `195`, runtime
  `v26.12.1`), Codex 0.166.0, Grok Build 1.0.70, Muse Code 1.4.14 (build
  `dec7d749…`), Devin 3000.11.234, Antigravity 2.29.0. Evidence in
  `docs/client-compatibility.md` (2026-11-04). Generated Claude requests still
  omit 2.1.318's first-party-only `x-claude-code-sandy-plover`, as they omit
  other first-party-only fields.
- **3.1** `serde_yaml`/`unsafe-libyaml` gone (closes #42). `src/yaml.rs` parses
  with `yaml-rust2` 0.13.0's event parser into `serde_json::Value` under
  serde_yaml 0.9's rules, and writes block YAML itself. A differential run
  against serde_yaml (corpus + 20,000 generated documents) agreed except three
  pinned cases where libyaml was lenient (`KNOWN_DIFFERENCES`); its results
  are kept as `src/yaml_corpus.json`. The harness with serde_yaml is in
  commit dcab7f5; serde_yaml leaves in 40c4308. The second review showed it
  counted "accepted, but no JSON form" as a refusal; 237edbb redoes it
  three ways and keeps loading every config serde_yaml loaded.
  `cargo +1.88 check --locked --all-targets` passes.
- **4.1** `src/golden_tests.rs`: 31 cases (4 client formats × 6 account kinds,
  plus streaming and native Claude Code / Codex CLI), raw upstream bytes in
  `tests/golden/upstream/`. `FUSEBOX_UPDATE_GOLDENS=1` rewrites them.
- **4.2** `plan_attempt`/`attempt_shape` in `proxy.rs`, side-effect free;
  `refuse` records what the inline code recorded. Goldens unchanged.
- **4.3** `POST /api/inspect` (`src/inspect.rs`) and the "Inspect a request"
  mode in `ui/playground.js` (the playground moved out of `app.js`). A test
  checks every golden case: the inspection equals the request then really
  sent, and inspecting sends, logs and assigns nothing.

### Review (GPT-6.1 Sol, max effort) and fixes

Ten findings, none critical or high; all ten checked and fixed:
inspection now redacts Kimi `X-Msh-Device-*` headers and the Vertex
`/projects/{id}/` path, previews a balanced group without taking its turn
(`group_order_preview`), and skips the accounts execution skips
(`ineligible_accounts`, shared with `execute_inner`); equally specific
overlapping `model-map` wildcards are a config error; the account Test calls
a prefixed account as `prefix/model`; `/v1/fusebox/status` counts an account
ready only if one of its models is unpaused; the usage-refusal fault counts
only refusals of a freshly refreshed token (no regression test: the usage
URLs have no test seam); Claude's `message_start` server-tool counts are kept;
aliases can't exceed the YAML nesting limit; and on iOS, outage Live
Activity passes are serialized and end on sign-out or a server change
(not compiled yet).

### Second review (Muse Wonder Full, high effort) and fixes

One high, twelve medium, sixteen low. Fixed in the commits after 879e583:
the YAML parser refused documents serde_yaml accepted (numeric keys, local
tags, .inf), now kept; inspection follows model-group failover, expands
continuations read-only, refuses an expired named client, takes 256 MB
bodies, logs nothing, and redacts configured headers and more body fields;
the account Test prices its model, sends once (plus one retry after a token
refresh) and leaves cooling or held-out accounts alone; status, usage-refusal,
fee-overflow, model-map, count_tokens and dropped-header edges; moving.md
says which files a move can't lose; goldens add Moonshot, xAI and Meta keys
and guard their update flag and orphans; property tests reach the parsers
and resolve defined refs; the iOS tests no longer sign a simulator out.

Left as they are, with reasons:
- Dropped-header names can be fed by any client key (UA shape is enough):
  they are evidence to check in a review, not something acted on.
- iOS lifecycle tests still lack a transport seam; nothing iOS is compiled
  yet. Build and test on the Mac before the next TestFlight.
- Only this parser reads what the writer emits in tests; no Go or libyaml
  reader runs in CI.
- `usage downgrade` copies rows with the new server-tool metadata keys as
  they are; an older binary reads them, and only a newer collector sending
  to an older server would trip its validation.
- Inspection picks the account a new session would get from `route_order`,
  not the live picker (latency tie-breaks may differ); it says so.
- Media and embeddings allowlists match the suffix-stripped model, like
  inference; `/v1/models` lists real models, not mapped ones.
- The writer refuses keys over 1024 characters (YAML's implicit-key limit),
  which only blocks a full rewrite of such a file.
- Goldens don't cover Vertex, Antigravity, Devin or the Kimi, Grok and Meta
  sign-ins yet.
- History: `Clients::admit_status` lands in the model-map commit, one before
  its caller; the tokio commit's lockfile also re-resolves `windows-sys` for
  errno and quinn-udp, as cargo did.
