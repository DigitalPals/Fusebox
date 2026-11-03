# What to address after the CLIProxyAPI review, and whether it is safe

Date: 2026-11-03. Source: [cliproxyapi-research-2026-11-03.md](cliproxyapi-research-2026-11-03.md) and its GPT-6.1 Sol review. The items were then re-checked, and four research agents assessed regression risk against main `4578d2f`. Nothing was built or changed; every verdict comes from reading code, upstream docs and client source. File:line references are to main `4578d2f`.

## What production actually exercises

`cli-proxy-api.service` journal, 2026-10-20 to 2026-11-03. All traffic was HTTP, with no WebSocket and no translation.

| Client → provider | Requests | Non-200 |
|---|---:|---:|
| Claude Code → Claude (native) | 154,701 | 159 |
| Codex app-server (`client_app` "unknown"; T3 Code by its user agent shape, inferred) → Codex (native) | 42,914 | 1,974, of which 1,932 are client disconnects |
| Codex CLI → Codex (native) | 1,179 | 8 |
| Anthropic SDK, OpenAI SDK, curl | 8 | 2 |

So only routing, refresh and native-passthrough changes can affect today's behaviour. Translation, WebSocket, CORS and non-Claude/Codex provider fixes matter to other users of the product, not to this deployment. The journal does not log upstream error text, so it can't show which specific errors occurred.

## Verdicts

**Safe** means no working request changes. **Guards** means safe only with the listed guards and tests. **Reproduce** means measure first. "Prod" says whether production traffic goes through the changed path.

### A. Build: real gaps, safe with guards

| # | Change | Prod | Verdict | Guards and tests | Effort |
|---|---|---|---|---|---|
| A1 | Stop renewing after a final token-endpoint answer | yes | Guards | See [A1](#a1-final-refresh-failures) | M |
| A2 | Codex `turn_rate_limit_exceeded`: no futile retries | yes | Guards | See [A2](#a2-codex-turn_rate_limit_exceeded) | S |
| A3 | Filter Codex/Meta private events for non-Codex clients | yes, must not change | Guards | Decide with `clients::is_codex(headers)` (`src/clients.rs:526`) or an `originator` header, never `proxy::client_app` (`src/proxy.rs:508`). Plain `client_app` would strip `response.metadata`, which Codex reads for routing and safety buffering, from the 42.9k app-server requests. Filter after the quota observers (`src/proxy.rs:2147-2150`, `src/ws.rs:208`). Match the SSE event name and the data `type`. Responses format only. Count dropped frames per client. Test T3-shaped and codex-tui headers (events kept) against an OpenAI SDK user agent (events dropped). | S |
| A4 | Retry-After when the pool is dry | yes (single Codex account) | Guards | Send the header only for rate-limit, quota or check pauses of 60 s or less. Don't advertise 401/403 cooldowns as a wait. Longer waits get no header and an OpenAI `usage_limit_reached` body. Cover the "last account error" path (`src/proxy.rs:1198-1201`). Today Codex ends a turn at once on a header-less 429. A short header lets it wait. A multi-hour header would make Codex and the Anthropic SDK sleep (CPA #6646). Claude Code may give up on waits over 60 s (unconfirmed). | M |
| A5 | WebSocket `error` event code | no | Safe, bug | `ws::error_event` copies `formats::error_body`'s numeric `code` (`src/ws.rs:268-275`, `src/formats/mod.rs:508`). Codex expects a string, fails to parse the event, ignores it, and waits for its idle timeout (codex-rs `responses_websocket.rs:685`, read by an agent). This affects translated WebSocket turns and named-client admission denials (`src/ws.rs:477`, `536`). Send a string code. | S |
| A6 | Anthropic credit-balance 400 | no (no API keys) | Guards | Claude API keys only. Match a 400 whose message starts "your credit balance is too low", case-insensitive. Pause the whole account (`*`), not one model, for 30 min, with its own reason text. While it is paused, answer with the stored 400 rather than the generic "all accounts rate limited" 429, so "add funds" isn't lost. Tests: failover with two accounts, and a single account. Not for OAuth: the same text was reported on subscriptions with an unclear cause, and the next usage reading would flip the pause. | S |
| A7 | Meta `client_attestation_denied` 403 | no | Guards | Match `error.code` exactly. Don't re-mint, don't pause, record no account error, and don't count it toward the failed-requests warning (`src/push/faults.rs:368-384`). Skip the other Meta accounts (they refuse the same tools). Fail over only to non-Meta routes for the same model. Keep Meta's message. | S |
| A8 | `compaction_trigger` on Kimi and Meta | no | Guards | Before routing, leave Kimi and Meta accounts out when the input contains `compaction_trigger`. If none remain, answer 400 suggesting a provider entry with `remote_compaction = "unsupported"`. No upstream call, no account error. Only requests that fail today change. Codex picks remote compaction per provider name, so a Fusebox catalog can't prevent it (codex-rs `model-provider/src/capabilities.rs`). | S |
| A9 | Unreadable auth directory | yes, on error only | Guards | On any `read_dir` error except NotFound, keep the previous file accounts and signature. Also fix `src/accounts.rs:1359`, where `!path.exists()` is true when the parent directory can't be read. An empty directory and NotFound still remove accounts. Today an unreadable directory drops every file account, moves sessions away (losing their cache), prunes pools, and the 2 s saves rewrite cooldown, quota and attribution state without those accounts. Tests: the auth path replaced by a file keeps accounts; `rm *.json` and `rm -rf` remove them. Existing deletion tests must stay green. | S |
| A10 | Credential filename collisions | yes, new sign-ins only | Guards | Change only `sign_in_path` (`src/mgmt.rs:147-166`): when a name is taken by a different identity (and it isn't the Claude same-email match), save as `<stem>-<8 hex of identity>.json`. Never rename existing files: the account ID is `file:<name>` (`src/accounts.rs:1123`) and keys cooldowns, quota, attribution, affinity, usage and faults. Today a collision overwrites the other account's tokens; this also happens with one email in two ChatGPT workspaces. Update `signing_in_again_reuses_the_accounts_file_whatever_it_is_called` (`src/mgmt.rs:1890`). | S |
| A11 | Gemini `systemInstruction.parts: [{}]` | no | Safe | Remove only parts that are exactly `{}`, in both spellings. Drop `systemInstruction` if nothing remains. On Antigravity, do it before wrapping. Google rejects `{}` today, so nothing that works changes. | S |
| A12 | Vertex `us`/`eu` host | no | Safe | `aiplatform.{us,eu}.rep.googleapis.com`. A configured base URL still wins (`src/upstream.rs:904`). Extend `src/vertex.rs:108`. | S |
| A13 | CORS allows `Authorization` | no | Safe | `AllowHeaders::mirror_request()`. The CORS layer covers only the proxy routes, with no cookies or credentials, so nothing widens. Add a preflight test and a test that `/api` gets no CORS headers. | S |
| A14 | Prefill on translated Claude requests | no | Guards | Drop a trailing assistant message only for models that reject prefill. Anthropic's errors page: Claude 4.6 and later and Mythos don't support it; Haiku 4.5 and older still do. Drop it before `manual_thinking_allowed` (`src/formats/claude.rs:224-241`). Never empty `messages`. Record a `control_downgrades` reason. Translated only (`src/proxy.rs:1338`). | S |
| A15 | Claude `tool_addition` (dynamic tools) | no | Guards | Merge `tool_definition` into `req.tools`, deduplicated by name. A `tool_reference` does nothing, because `defer_loading` tools are already sent up front (`src/formats/claude.rs:87-97`). Keep removed tools callable. Costs one cache miss when a tool appears. Whether Claude Code sends these betas for gpt-* models is unverified. | S |
| A16 | Antigravity Opus and Sonnet 5.5 structured output | no | Guards | Only for models containing `opus-5-5` or `sonnet-5-5`, with any `-max/-medium/-low` suffix (`src/accounts.rs:46-56`). Move the schema into `systemInstruction`, drop the MIME type and schema, and use AUTO instead of VALIDATED. The JSON is then not guaranteed. Keep the `sonnet-4-6` test (`src/antigravity.rs:346-371`) and add a 5.5 case. | S |
| A17 | Schema cleaner | no | Guards | (a) Add placeholder `items` only after merging `allOf`/`anyOf`; today a branch's placeholder overrides a later branch's real items (`src/schema.rs:206-230`, `248-255`, `341-344`). (b) Convert Gemini's numeric-string limits at schema positions only, never property names (`src/formats/gemini.rs:148-167`). The cleaner is used only by the Antigravity envelope. Adjacent: `lower_schema_types` misses a property named `type`. | S |
| A18 | Gemini contents with no `role` | no | Safe | An entry with `functionCall` parts becomes the assistant (`src/formats/gemini.rs:61`). Translated only. | S |
| A19 | Devin `model(high-fast)` | no | Safe, low value | Today it silently becomes `-medium` (`src/devin.rs` `clamp`). Append `-fast` only for models that have a fast variant. Full UIDs already pass through. `swe-2-lightning` aggregation is optional. Fast Devin UIDs are priced as standard (inferred). | S |
| A20 | Strip `content_token_count` from replayed output | no (Codex 0.164 HTTP sends no `previous_response_id`) | Guards, low value | Strip it only from output appended at `src/affinity.rs:920`, only that key, and drop the parent object if it ends up empty. Never touch client input. | S |
| A21 | `context_length_exceeded` code | no | Guards, low value | Only for a Claude upstream, status 400, and a message starting "prompt is too long". Codex reads the code only from `response.failed`, not from an HTTP 400, so this will **not** make Codex compact. That would need a 200 stream ending in `response.failed` (M, reproduce first). Helps Cline-style clients. | S |
| A22 | WebSocket fallback for 429 and capacity errors | no | Guards | Only when a 429 or capacity error is the first upstream frame, never a 5xx. Reuse A2's long-wait rule. If local history is missing, the fallback answers 400 `previous_response_not_found` (an existing risk on the quota path). Add mock WebSocket tests. | S–M |

#### A1. Final refresh failures

- **Evidence:** the 2026-10-31 floods (1,349 and 140 lines) came from a stale duplicate credential file that kept a revoked token. **8180e0a fixed that the same morning.**
- **What remains:** the missing backoff. The 2026-10-22 case logged 391 lines in 2 h ("Refresh token expired"). Every caller of `ensure_fresh` retries a final failure (`src/oauth.rs:759`, `src/proxy.rs:1228`, `1604`, `src/quota.rs:773`, banked resets, window starter, discovery, WebSocket, management).
- **Guards:**
  - Gate inside `ensure_fresh`, keyed on the rejected refresh token, not a flag. Credential identity excludes tokens, so a new sign-in wouldn't lift a flag.
  - Re-read the file before holding the account out. A local CLI or a second Fusebox may have rotated the token, giving a false `invalid_grant` or `refresh_token_reused`.
  - Keep serving while the access token is valid.
  - Hold the account out at selection, as `disabled` is (`src/accounts.rs:1652`, `1787`; `src/affinity.rs:646`; `src/proxy.rs:1140`), not with `cool()`.
    - A `cool()` writes `cooldowns["*"]`, which shows as "Cooling". It hides the sign-in fault (`src/push/faults.rs:264-265`), and the fault is forgotten after two checks (`src/push/mod.rs:253-262`). So today a "sign-in expired" push probably repeats every cooldown cycle (inferred).
  - Exclude Vertex (JWT mint; `invalid_grant` can be clock skew) and Meta.
  - Back off on `invalid_client` instead of holding the account out; it hits every account of a provider at once.
  - Treat a bare 401 from the token endpoint as final only for Codex.
  - Put "sign in again" into the message rather than changing the three fault regexes (dashboard, push, iOS; the iOS one is in the build under App Review).
  - Add `token_revoked` to `error_classification`.
- **Tests to update:** `oauth_classifications_are_allowlisted_and_transport_urls_are_discarded`, `refresh_errors_and_account_status_exclude_response_and_redirect_secrets`, `oauth_error_markers_do_not_escape_to_downstream_status_or_request_logs`, `a_new_sign_in_replaces_the_revoked_tokens_even_while_they_refresh`, `reads_sign_in_errors_like_the_dashboard`, `mirrors_the_faults_menu`.
- **Tests to add:**
  - one token call after a final answer, across all paths;
  - the account serves until expiry;
  - the hold lifts on a dashboard sign-in, on a new file, and on a newer token already on disk;
  - a 503 and a Vertex failure are not final.

#### A2. Codex `turn_rate_limit_exceeded`

- **Today:** a capacity Retry-After over 10 s is discarded, and the same account is retried after 1 s and 3 s (`src/proxy.rs:592-604`, `1569-1581`). CPA #7017's data says those retries fail.
- **Change:**
  - Skip the same-account retries when the capacity Retry-After is over 10 s.
  - Fail over for this request only, with no pause. fd0d465 removed the pause because, with one subscription, it "turned away every session for 30 to 120 seconds".
  - With no account left, pass Retry-After (capped at about 10 min) so Codex waits out only this turn.
  - Map the code to 429 in `status_of_code` (`src/formats/responses.rs:418-424`), and match the code in `at_capacity` too. Otherwise a 429 whose message lacks "at capacity" gets a 30 s pause at the start-of-stream check (`src/proxy.rs:1662`).
- **Tests to update:** `soft_failures_retry_and_quota_does_not` and `codex_capacity_errors_retry_the_subscription_without_pausing_its_sessions`.

### B. Measure or reproduce before building

| # | Question | What to do first | Why not now |
|---|---|---|---|
| B1 | Do Codex SSE streams start with an in-stream 401 (`auth_unavailable`)? | Log the raw code and type of any Codex error event before output. | CPA rewrites ordinary HTTP 401s into `auth_unavailable` (`codex_executor_terminal.go:392`), so #6742's body is probably CPA's own (inferred, strong). Fusebox already refreshes and retries HTTP 401s. Mapping to 401 alone would release the stream and show failures to the 42.9k app-server requests. |
| B2 | Do Codex tokens die before `expires_at`? | Keep the 10 min margin. Log the token age on each Codex 401. | At 96 h, an outage at the token endpoint means days of every-minute retries plus a false fault. Refreshing early also logs out a local CLI sharing the credential sooner. If it is ever needed: min(96 h, half the lifetime), with backoff and no fault before the last 10 min. |
| B3 | Is the turn-rate limit per turn, or per account and model? | Log the upstream Retry-After (today only the chosen wait is logged). | Pausing the model for 256–515 s turns away every session on a single account. |
| B4 | Does compaction hit "response protection is unavailable"? | Look for the 502 text in the request log or the dashboard. | Codex 0.164 compaction sends the full tool list. `tools: []` comes only from older Codex or Lite mode. CPA adds `web_search` to every Codex request, which could turn search back on after the user disabled it and changes the cache prefix. **Narrowest form:** retry once on the same account with the tool added, only after that exact 502. **Also:** that 502 is a soft failure, so one bad compaction gives every Codex account a 30 s error (`src/proxy.rs:598`, `1569-1594`). |
| B5 | Thinking `block_binding`: does Fusebox hit the 400? | Reproduce on a translated Codex → Claude session after a tool-list change. Check once on an OAuth account whether the beta is accepted. | Anthropic documents `block_binding` and the `thinking-binding-controls-2026-08-01` beta. Accounts created on or after 2026-08-31 get a 400 on 5.x when the prefix changes. Fusebox moves late system messages into the top-level system (`src/formats/mod.rs:311-340`), which probably triggers it. **Reactive form:** on a 400 containing "bound to a different conversation", retry once with the beta and `drop_block`, and remember that for the session. Translated only. Native beta strings stay byte-identical (tests at `src/upstream.rs:1319-1363`). Don't send it on every request, as CPA does: older accounts would lose blocks that work today. |
| B6 | Does xAI reject `additional_tools` input items? | Reproduce against xAI. | If xAI accepts them, promoting them breaks the cache prefix. Codex `namespace` tools would also need name flattening and reverse mapping. |
| B7 | Does Codex's backend accept `additional_tools` from a translated client (A15's richer form)? | Reproduce. | Codex sends it only in Lite mode with an internal header. |
| B8 | Empty text blocks on native Claude passthrough | Look for an occurrence. | Only Claude Code uses this path, and it would fail the same way talking to Anthropic directly. If done: only `""`, never move `cache_control`, never leave a message empty. **Probably don't.** |

### C. Don't do

| Item | Reason |
|---|---|
| hyper header-read timeout | hyper re-arms it on every idle keep-alive connection (`proto/h1/conn.rs:1156-1161`), so idle Claude Code and Codex connections, and the gate's, would close after 30 s. If ever needed: listeners reachable by untrusted peers only, at 120 s or more. |
| "Uncloak" Claude Code on API keys | Anthropic says subscriber API credits don't cover Claude Code. Stripping the identity dodges that billing rule and rewrites native requests. A6 (failover) is the answer. |
| CPA's Muse `muse` namespace | Evades Meta's client attestation and adds a callable tool the client never declared. A7 is the answer. |
| Antigravity `aicode` scope | Only needed for Gemini 3.5 transcription at `aicode.googleapis.com`, which Fusebox doesn't call. It would change the consent screen for every sign-in. |
| Gemini `partialArgs` | Not applicable. Upstream sends them only when `streamFunctionCallArguments` is set, and Fusebox builds `toolConfig` itself without it (`src/formats/gemini.rs:299`). |
| CPA-style compaction summaries for Kimi and Meta | Codex demands one `compaction` item with `encrypted_content`. Faking one means inventing and expanding capsules forever. A8 covers the failure. |

### D. Features: separate product decisions, default off

Not risk-researched in depth. Each can ship off by default, which leaves current behaviour unchanged.

- **Sign in with ChatGPT (SIWC)** (M): a second token bound to a Codex account. Non-Codex clients use it; Codex clients keep the normal token. Known limits: `agent_message` is rejected, both tokens share one failure state (inferred). Needs OpenAI's policy text to confirm it is the sanctioned route.
- **Per-account weekly reserve** (S–M): stop routing at N% of the weekly window left. It touches smart-quota routing for every request, so it needs routing tests at 0% (off).
- **Route preview** (S): read-only, no risk.
- **Count and price server-tool use** (web search and fetch, code execution) (S–M): adds usage columns. Coordinate with the uncommitted `src/usage` work.

## Status, 2026-11-03

A1–A22 and the B1–B4 logging are built on branch `fix/cpa-review-2026-11-03`, worktree `.claude/worktrees/cpa-fixes-20261103`, based on `4578d2f`. The changes are uncommitted and not deployed. Four implementers built them in parallel, and they were then integrated.

**Review.** GPT-6.1 Sol reviewed the integrated change at max effort and requested changes: one P1 and five P2 issues.
- **P1:** a 403 could hold out an account.
- **P2:**
  - the pause kind and the refusal were read from the shown error;
  - the dry-pool check disagreed with selection;
  - stream and WebSocket waits were lost;
  - a single-account WebSocket fallback could not reach HTTP;
  - model groups reset request-wide refusals.

All were fixed with tests. A second review approved. Its two follow-ups were also done: held accounts are not ready in `/readyz`, the metrics or the overview, and a decoder test covers rollback compatibility.

**Results:** 890 Rust tests pass (837 at base) and 169 UI tests pass. clippy, fmt and `git diff --check` are clean.

**Deferred on purpose:**
- B2 can misattribute a token's age when the token rotates concurrently.
- The WebSocket 401 path after output still cools for 60 s.
- B4 is HTTP only.
- The credit pause shows as kind `rate_limit`.
- Refresh holds are not persisted across restarts.
- Everything in sections B (except the logging), C and D.

**Deferred items done, and LIVE, 2026-11-03.** The five deferred items were then fixed:
- B2 takes the token the request carried;
- a WebSocket 401 after output puts a held account out;
- the response-protection refusal is handled inside streams and on WebSocket;
- a credit pause is its own `credit` kind ("API credit used up");
- refresh holds are saved by digest in `.cooldowns.state`.

That is commit fdff08c on main. A clippy 1.99 fix (a8c03ca) and a Windows deletion fix (8306284) followed. CI was green on Linux, macOS and Windows. It was force-deployed at 11:47 CET as 0.12.1+gui.821899217a91, and all checks passed.

Still open: everything in sections B (except the logging), C and D. The iOS app shows the new `credit` kind with its "Rate limited" fallback until an app update.
