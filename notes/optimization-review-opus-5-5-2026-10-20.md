<!-- Independent review retained verbatim below. Requested and verified selection: claudeAgent / claude-opus-5-5 / effort=xhigh. Task status: completed. Baseline: 96626c93f03c9f94792685be1eb7f2591b974534. -->

# Independent review: Fusebox optimization research draft (20 Oct 2026)

**Scope and method.** I read the source only, in `/home/john/Code/Fusebox` at HEAD `96626c9`. The working tree still holds only the untracked draft. I built nothing, ran no tests, made no inference calls and launched no agents. I fetched the two official docs pages with `curl` into a scratch folder in `/tmp`, which I have since deleted, and checked the key provider claims against them. Every "confirmed" below comes from reading the control flow, not from a failing test. Writing the regression tests is the cheapest way to prove the two cache bugs before fixing anything.

**Overall verdict.** The draft is accurate and well hedged. Its `file:line` references hold. I tried to disprove both cache bugs and the control-loss claim and could not. Each needs a refinement, though. One concern in the draft is wrong (the embedding "overwrite"), and it misses several items, the most important being how translation moves system/developer messages.

## 1. Verdicts on each finding

| # | Finding | Verdict |
|---|---|---|
| 1 | Recursive marker scan treats schemas and arguments as cache markers | **Confirmed.** It affects more than the draft says; the fix design needs changing |
| 2 | Claude OAuth cloaking undoes explicit no-cache; drop vs automatic | **Confirmed** for explicit-no-breakpoints; **narrowed** for `drop` |
| 3 | Translation loses verbosity and service tier | **Confirmed, narrowed** to the Chat→Responses path |
| 4 | Translated usage uses the `cache_creation_tokens` name | **Confirmed.** The Chat renderer has the same problem |
| 5 | Telemetry and benchmark gaps | **Confirmed**, with one benchmark-design issue added |
| 6 | Body copies, history copies, SSE buffers, worker count | **Confirmed**, with two more history copies and a no-code worker knob |
| 7a | WebSocket fallback | **Confirmed** |
| 7b | Latency tie-break for new sessions | **Confirmed** (smart-quota only); low priority |
| 7c | Embedding counters overwritten per chunk | **Rejected** (the concern); the chunking facts are right |
| 7d | Absolute pre-output deadline | **Confirmed but demoted** |
| 8 | Provider documentation claims | **Confirmed** against the fetched docs, with one ChatGPT-specific caveat |

### 1. Recursive marker scan: confirmed, larger than described

**How it happens.**
- `has_fields` (`src/cache.rs:175-184`) returns true for any object key named `cache_control` or `prompt_cache_breakpoint` whose value is not null. A schema property such as `{"cache_control": {"type": "string"}}` therefore counts as a marker.
- `Requested::of` stores that result once (`src/cache.rs:123-130`, called at `src/proxy.rs:914`). It then feeds:
  - `apply_translated_cache_policy_with` (`src/proxy.rs:1167-1173` → `src/formats/claude.rs:280-283`);
  - the automatic-downgrade branch (`src/proxy.rs:1150-1158`).
- `claude.rs:281` also rescans the translated body (`has_cache_control`, `claude.rs:257-267`).
- The cloak scans it again (`src/upstream.rs:277`, `upstream.rs:415-423`).

**Why my attempts to disprove it failed.**
- An exact, format-aware breakpoint counter does exist (`src/formats/mod.rs:240-260`). It is only used in `carry_cache_hints`, for OpenAI→OpenAI carry-over, and never gates Claude's default caching.
- `params_or_empty` copies schemas unchanged (`src/formats/chat.rs:122-124`).
- Nothing strips schema property names.

**Precision about tool arguments.** In OpenAI formats, `arguments` is a JSON string, so the scan of the original body does not match it. The match happens after translation: `Part::ToolCall` arguments are parsed into `tool_use.input` objects (`claude.rs:391-393`), and the translated-body checks (`claude.rs:281`, `upstream.rs:277`) then find them. Claude-format `tool_use.input` objects match directly.

**The draft misses three consequences.**
- **False 400s.** For Claude-format callers translated to a non-Claude provider, `formats/mod.rs:115-119` uses the same recursive `has_claude_controls`. A schema collision produces a false "Anthropic cache controls … cannot be preserved" downgrade. With `on-incompatible: reject`, `proxy.rs:1135-1149` then returns a false 400.
- **It is sticky.** Once a tool call whose input contains such a key enters the history, every later turn of that conversation loses default caching on both the translated and cloak paths. Collisions are probably rare (nobody has measured prevalence), but each one can disable caching for a whole long agent session.
- **Native requests are affected too.** Native, non-Claude-Code Claude-format clients on OAuth with cloaking also go through `cloak_body`, so this is not limited to translated traffic.

**Fix design: invert the draft's proposal.** The broad scan exists for safety: it prevents a fifth breakpoint (`upstream.rs:413-414`; test `src/formats/efficiency_tests.rs:93`). Claude's docs say that four explicit breakpoints plus automatic caching return a 400. The draft proposes a parser that "traverses only supported … locations". That allowlist fails badly: missing a future or rare block type (document, search result, server-tool results) causes 400s. A denylist of opaque payloads fails safe instead. It would skip:
- `tools[].input_schema` and `tools[].parameters`;
- `tool_use.input` and `server_tool_use.input`;
- structured-output schemas (`output_config.format.schema`, `response_format.json_schema.schema`, `text.format.schema`).

Everything else keeps the conservative behaviour. Then test schema collisions, argument collisions, and legitimate nested `tool_result` markers.

### 2. Cloak re-adds caching: confirmed for explicit-no-breakpoints, narrowed for drop

**Path traced on the default configuration** (`claude-cloak: true`, `src/config.rs:398`):
1. `carry_cache_hints` returns an error for explicit mode on a Claude target (`formats/mod.rs:142-147`).
2. Fusebox strips the breakpoints and removes `prompt_cache_options` (`formats/mod.rs:102-105`).
3. The automatic branch adds nothing, because `markers` is false (`proxy.rs:1150-1158`).
4. The translated default adds nothing, because `explicit_mode` is true (`proxy.rs:1167-1173`).
5. `Target` (`upstream.rs:35-47`) has no intent field.
6. In `claude()`, `oauth && claude_cloak` (`upstream.rs:168`) leads to `cloak_body`: `configured_cache` is false (`upstream.rs:277`), so it adds `system[1]` and a top-level `cache_control` (`upstream.rs:328-333`).

This breaks a documented promise: the README says "Explicit mode without boundaries remains uncached."

**It already shows up in the logs.** `Policy::for_request` reads the prepared body (`proxy.rs:1191-1196`), so the structured log reports `cache_requested=explicit_no_breakpoints` with `cache_applied=automatic`. Prevalence can be counted from existing logs before anything changes.

**Drop vs automatic is more subtle than the draft says.** On the cloak path:
- `drop` → two breakpoints (`system[1]` plus top-level);
- `automatic` → one, because the proxy's own top-level marker makes `configured_cache` true and the cloak then skips `system[1]`.

So `drop` gives *more* caching than `automatic`. That contradicts the README's description of `drop`, but "fixing" it may cost money for growing conversations: the caller did want caching, just at boundaries Fusebox cannot translate. This is a maintainer decision. Either honour the README, or document that cloaked requests always get Claude Code-like caching except under explicit no-cache. Do not present it as a savings fix.

**Test gap confirmed.**
- `src/routing_tests.rs:1568-1604` covers API keys only (`claude_api_key`).
- The cloak unit tests (`upstream.rs:1024-1060`, `1148-1159`) cover default, automatic and nested-marker cases, but not explicit-no-breakpoints or drop combined with translation.

**Efficiency impact is unknown.** On the public API the caller avoided 1.25× write costs; how cache writes count against subscription quota is undocumented.

### 3. Lost verbosity and service tier: confirmed and narrowed

- `Request` has no such fields (`src/ir.rs:31-50`).
- `verbosity` appears nowhere in `src/`. `service_tier` appears only in usage/pricing code and in `CODEX_FIELD_ORDER` (`upstream.rs:498`).
- The parsers don't read either field (`chat.rs:13-42`, `responses.rs:40-65`).

**Where it actually matters.** Codex, xAI and Meta speak only Responses (`src/accounts.rs:126-136`). Chat Completions clients on those providers are therefore *always* translated and lose `verbosity` and `service_tier`. Claude-format callers have no verbosity concept, and Claude's `service_tier` semantics differ, so do not map it across providers.

- **Pass-through confirmed:** `CODEX_STRIP` (`upstream.rs:428-440`) does not remove `text` or `service_tier`.
- **Deliberate stripping confirmed:** `upstream.rs:428-449` and `responses.rs:365-375`.

**Tradeoffs.**
- Preserving `service_tier` is about honouring the caller's choice, not saving money. `priority` can cost more quota, and nobody has verified that subscription backends accept it. Make it opt-in, provider-aware, and report a diagnostic when it is dropped.
- Map Chat's `verbosity` to Responses `text.verbosity` only for verified model families. Support on xAI and Meta is unknown.

### 4. Cache-write field name: confirmed, plus the Chat renderer

- The Responses renderer emits `cache_creation_tokens` at `responses.rs:803-805`. The Chat renderer does the same at `chat.rs:506-508`, which the draft omits.
- The parsers accept both names (`responses.rs:397-401`, `chat.rs:327-329`, `src/usage/capture.rs:245-249`), so round-trip tests cannot catch the problem.
- There is a concrete affected consumer: `docs/usage-provenance.md:24` records that Codex maps `cache_write_tokens`. Translated Codex sessions therefore see writes as uncached input.
- The OpenAI docs confirm the Responses name (doc line 557). The Chat Completions name was not on the fetched page; verify it before changing `chat.rs`.
- This affects accounting and display only, not tokens.

### 5. Telemetry and benchmark: confirmed

- Prometheus help text says "input (with cache writes)", and there are no histograms (`src/metrics.rs:97-106`).
- Request logs are held in memory only (`src/state.rs:457`). The structured completion log is at `src/diagnostics.rs:185-207`.
- `Observation` has no latency field (`src/usage/types.rs:51-81`). It does have a generic `numeric_metadata` map (`types.rs:79-80`), which may be a cheaper extension point than a schema change.
- The benchmark only allows `input` and `instructions` to differ (`scripts/context-benchmark.py:394-396`), rejects stateful requests (`:306-307`), and reports `quality_evaluated: False` (`:404`).

**Added design problem.** Both variants run in one session with alternating order (`:420-427`). Cache state is shared between them, and the first pair includes cold writes. With the default three trials, the results mix cold and warm requests. Experiment modes must separate cold from warm and isolate cache cohorts.

### 6. Memory and runtime: confirmed, with additions

**Confirmed as written.**
- Per-attempt body clone and serialization: `proxy.rs:1051-1056`, `1206`. The Oct 17 report says this was deliberately left unchanged (`docs/efficiency-2026-10-17.md:83-86`).
- Capture copy: `proxy.rs:654`.
- `previous()` deep-copies outside the lock: `src/affinity.rs:763-776`.
- History caps: `affinity.rs:23-25` (64 MiB total, 8 MiB per entry, 1,000 entries), measured in serialized bytes.

**Missed: WebSocket turns.** Native WebSockets are on by default (`config.rs:397`). Each native turn that has a `previous_response_id`:
- deep-copies the stored history (`src/ws.rs:440-460` → `affinity.rs:775`);
- copies it again (`full.to_vec()`, `ws.rs:550`);
- runs a full serialization pass to measure its size (`affinity.rs:786`, `815+`).

That is work proportional to the whole history on every incremental turn, spent only to keep fallback state.

**Missed: capture scope (a hypothesis).** HTTP capture runs for *every* Responses request that has a session (`proxy.rs:647-654`). That includes full-history `store:false` HTTP turns that can never legitimately use `previous_response_id`. Those entries share one global least-recently-used store with clients that do need continuations, and can push those clients' entries out. The result is a `previous_response_not_found` 400 and a full resend. Skipping capture for non-WebSocket `store:false` requests might help, but some clients may rely on today's behaviour; WebSocket fallback turns arrive with `transport: "ws"` (`ws.rs:514`) and would have to be exempt.

**SSE buffers.**
- Neither the decoder's line buffer nor its data string has a size limit (`src/sse.rs:20-37`).
- Bootstrap holds upstream chunks (`proxy.rs:1569-1589`), but only for a limited *time*: 5 seconds after the first event (`BOOT_HOLD`, `proxy.rs:1542-1543`), and up to the first-token deadline before it.
- Providers are trusted, and legitimate events can be large (image partials). Label this defence-in-depth, not efficiency.

**Workers: no code needed.** `Builder::new_multi_thread()` (`src/main.rs:107`) honours the `TOKIO_WORKER_THREADS` environment variable (tokio `loom/std/mod.rs:88`), so the experiment is configuration-only. Compare against the *current* baseline (4 glibc arenas, 64 workers: 106–125 MiB) rather than the default-glibc row (206–227 MiB) in `efficiency-2026-10-17.md:96-100`. That load was synthetic and CPU-bound.

### 7. Smaller ideas

- **WebSocket fallback — confirmed.** Turns fall back to HTTP for configured proxies, model groups, policy failures, routes to other providers, and Codex accounts that are not OAuth or have their own proxy (`ws.rs:592-602`, `606-608`, `623-625`).
- **Latency tie-break — confirmed.** Scoring uses quota, reset timing and load (`accounts.rs:1703-1722`), and only under smart-quota routing. Low priority.
- **Embedding "overwrite" — rejected.**
  - The chunk sizes are right: 100 for Gemini, 1 or 5 for Vertex (`src/embeddings.rs:257`, `302`).
  - But every chunk calls `request()`, which calls `tracker.attempt()` (`embeddings.rs:222`). That finishes the previous capture and starts a new observation with its own attempt id (`capture.rs:112-125`).
  - Embedding observations have no response id, so each keys on its own event id (`src/usage/store.rs:722-739`) and is stored as a separate entry. Nothing is overwritten.
  - The real obstacles to parallel chunks: there is one current capture per request (`capture.rs:76`), and `log.attempts` is incremented per chunk (`proxy.rs:127`). The Requests page already overstates attempts for multi-chunk embeddings.
- **Absolute deadline — confirmed but demoted.** The worst case is about 2 × 120 s before failover (`proxy.rs:1210-1213`, `1565-1580`). The retry budget is only checked before starting new attempts (`proxy.rs:811-813`, `1539`). That worst case needs late headers *and* no first event, which is rare. A tighter deadline risks running the same paid inference twice when an account is slow but still working.

### 8. Provider docs: confirmed, with additions

**OpenAI** (all confirmed):
- 1,024-token minimum (doc line 69);
- 1.25× write, 0.1× read, 0.05× for GPT-6.1 Sol (lines 83, 237-238), and the doc itself states the 2.15× ten-request example;
- explicit mode with no breakpoints means no caching (line 90);
- 30-minute TTL (line 152);
- on GPT-5.6+ the key is optional and only separates accounting (lines 218-220, 475);
- about 15 requests/minute per key for earlier models (lines 196, 216).

**Claude** (all confirmed):
- 512-token minimum for 5.x models (line 608);
- TTL counted from request start (lines 214, 3249);
- 0.05× reads for Opus/Sonnet 5.5 (line 260), 2× for 1-hour writes (line 268);
- automatic caching uses one of the four slots, and four explicit breakpoints plus automatic returns a 400 (lines 500, 528);
- 20-position lookback, with runs of `tool_use` or `tool_result` blocks counting as one position (line 557).

**Additions.**
- **ChatGPT OAuth caveat.** On Codex OAuth a derived key is *also* sent as the `session_id` header (`upstream.rs:593-597`). Shared `client` or `workspace` scopes therefore make separate sessions present one session id to the ChatGPT backend. Don't recommend shared scopes there without verifying.
- **Scope does nothing for Claude.** `claude()` never reads `t.cache_key`; only the Codex path (`upstream.rs:584-588`) and xAI (`upstream.rs:794`) use it.
- **Fingerprint fragmentation is a hypothesis.** Within a session the billing fingerprint is stable. Whether it splits cross-session reuse depends on undocumented provider handling of that header. Verify it with provider usage (cache reads on the first turn of a new session that has the same tools), not with prepared-prefix hashes alone.

## 2. Missed issues worth adding

- **M1 — Translation moves system/developer messages (hypothesis about how often; mechanism proven).**
  - Every system or developer message is lifted out of its position: `chat.rs:47-49` and `responses.rs:123-124` move it into `req.system`.
  - Builders then emit it at the front: Claude's top-level `system` (`claude.rs:373-375`), or the initial developer block in Responses (`responses.rs:227-232`).
  - Adding or changing an instruction mid-conversation therefore changes the earliest part of the prompt and invalidates the cache from that point on, for that turn. It also moves the instruction, which can change model behaviour.
  - Responses accepts developer messages inline, so Chat→Responses could keep them in place for every model.
  - For Claude, in-place `role: "system"` messages are documented for 5.x models except Sonnet 5 (doc line 677). How they interact with the cloak is unknown.
  - Count how often this happens before doing anything. If it is common, it is likely the biggest cache item for translated traffic.
- **M2 — Wasted writes and cache-gap analysis from existing data (measurable now, no code).**
  - Native observations have `event_at_ms` at attempt start (`capture.rs:121`), a session id, and separate read and write counts.
  - That is enough to measure: writes never followed by a read within the TTL (does default automatic caching cost more than it saves for one-shot traffic?), and the distribution of gaps between requests.
  - Illustrative arithmetic at Claude API list prices (0.05× read, 1.25× 5-minute write, 2× 1-hour write): per turn, a 1-hour TTL is cheaper when the fraction of turns arriving more than 5 minutes after the previous request *started* exceeds about 0.625 × (new tokens per turn ÷ cached prefix), plus a one-time 0.75× premium on the first write.
  - This applies to API keys only. Whether 1-hour TTL is allowed on OAuth, and what it costs in quota, is unverified.
- **M3** — False Claude→OpenAI downgrade or `reject` 400 (§1.1).
- **M4** — History copies on every WebSocket turn (§1.6).
- **M5** — Capture pushing out continuation entries other clients need (§1.6, hypothesis).
- **M6** — Chat renderer field name (§1.4).
- **M7** — ChatGPT `session_id` behaviour under shared key scopes (§1.8).
- **M8 (low)** — Changing thinking or effort invalidates Claude's message cache (doc line 670). `claude.rs:366-370` can turn manual thinking off on some turns for older budget-thinking models whose history came from another provider. Edge case.

## 3. Revised priority order

0. **Measure with what exists (no code).** Count, from structured logs and native observations:
   - requested-vs-applied cache mismatches;
   - share of traffic translated to Claude;
   - wasted writes and gap distributions (M2);
   - cache hit rate by provider, model and client;
   - WebSocket fallback frequency.

   This tells you which of the items below matter.
1. **One cache-intent change** (§1.1, §1.2, M3): denylist-based detection, intent carried into `Target`, the default policy applied once after all rewriting, and the full test matrix. Small and low-risk. The maintainer must decide between the README promise and Claude Code-like caching on the cloak path.
2. **Canonical `cache_write_tokens`** in the Responses renderer, and the Chat renderer once the name is verified. Trivial; improves interoperability with Codex.
3. **Chat→Responses `verbosity`** for verified model families. `service_tier` only as an explicit, provider-aware opt-in, with downgrade diagnostics.
4. **Keep instruction positions (M1),** only if step 0 shows it happens often. Chat→Responses first; Claude limited to documented models, behind a flag, with quality checks.
5. **TTL and boundary experiments** using the extended benchmark: cold and warm separated, quality evaluated, API keys first.
6. **Prometheus:** a separate cache-write counter, plus bounded latency histograms with no session or prompt labels.
7. **Memory:** WebSocket and HTTP history copies and capture scope; profile body clones; trial `TOKIO_WORKER_THREADS`; SSE limits as defence-in-depth.
8. **Low:** embedding concurrency (fix per-subrequest capture first), latency tie-breaks, absolute deadline.

## 4. Claims to remove or soften

- **Remove** the embedding-overwrite concern; replace it with the per-attempt explanation in §1.7.
- **Cache-intent priority:** keep it first for correctness, but say "prevalence unknown, probably rare, sticky per conversation when triggered". State that `drop` may lose caching when fixed.
- **Fragmentation (§3):** change "can fragment shared prefixes" to "may fragment cross-session reuse; verify with provider usage".
- **Pass-through retains settings:** say which ones — `text.verbosity` and `service_tier` are not in `CODEX_STRIP`.
- **Shared key scopes:** add the ChatGPT `session_id` caveat and that scope does nothing for Claude.
- **Worker table:** compare against the current 4-arena baseline and mention `TOKIO_WORKER_THREADS`.
- **SSE limits:** label defence-in-depth. **Absolute deadline:** demote it and add the duplicate-inference risk.
- **Never** imply that turning cloak caching off saves money, or that public API prices predict subscription quota.

## 5. How solid each claim is

| Category | Items |
|---|---|
| **Proven from source (static reading, no tests run)** | §1.1–§1.7 control flow; M1, M3–M8 mechanisms; caps; defaults (cloak and native WebSockets on) |
| **Confirmed in the provider docs** | §1.8 figures; mid-conversation system messages (Claude doc line 677); effort invalidation (line 670) |
| **Hypotheses (need measurement)** | Prevalence of schema collisions and moved instructions; savings from 1-hour TTL or explicit boundaries; fingerprint fragmentation; whether capture pushes out other clients' entries; gains from workers, clones and history sharing; ChatGPT handling of `service_tier`, verbosity and shared `session_id` |
| **Old measurements (verified as quoted)** | Oct 17: summary 457 ms → 9.6 ms, database 1,913 → 653 MiB (`efficiency-2026-10-17.md:40-41`), 0.74–0.77 ms per request (`:52-53`), glibc/worker table (`:96-100`). Historical, synthetic or production-copy figures, not today's production |
| **New measured outcomes** | None |
