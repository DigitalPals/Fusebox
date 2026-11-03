# CLIProxyAPI since 17 October 2026, checked against Fusebox

Research date: 2026-11-03. This covers CLIProxyAPI (CPA) v8.0.33 (2026-10-18) to v8.0.52 (2026-11-02): 20 releases, about 390 commits. It also covers the Management Center from v1.26.5 to v1.27.2, the plugin store, and CPA issues opened since 2026-10-15. It follows [roadmap-research-2026-10-13.md](roadmap-research-2026-10-13.md) and [improvement-research-2026-10-17.md](improvement-research-2026-10-17.md), which covered CPA up to v8.0.32.

The Fusebox checkout was `main` at `4578d2f`. Nothing in Fusebox was changed, and the production journal was only read. Items marked **verified** I read in Fusebox's source myself. The rest come from three research agents, who cited file:line. **inferred** marks claims nobody tested.

**Superseded for planning by [cliproxyapi-action-list-2026-11-03.md](cliproxyapi-action-list-2026-11-03.md)**, which re-checks every item, adds regression-risk verdicts and corrects the points below where they differ. In particular, the 2026-10-31 refresh floods came from a stale duplicate credential file that 8180e0a fixed the same morning.

## Recommended, in order

1. **Stop retrying refresh tokens that can never renew.** **verified**, bug, S.
   - `oauth::refresher` runs every 60 s for every enabled OAuth account (`src/oauth.rs:748-765`). `ensure_fresh` has no backoff after a permanent failure. Requests that land on the account also try a refresh and then cool it (`src/proxy.rs:1604-1614`).
   - The production journal has 1,883 "token refresh failed" lines since 2026-10-20:
     - 1,349 in 6 h on 2026-10-31 for one Claude account, all `invalid_grant`;
     - 140 in 45 min for a second account the same morning;
     - 391 in 2 h on 2026-10-22 for a third account.
   - Each failure is logged twice (**verified**): once inside the refresh task (`src/oauth.rs:497`) and once by the refresher (`src/oauth.rs:759`). The due plan check also calls `ensure_fresh` (`src/quota.rs:773`). So 1,349 lines in 6 h is roughly 675 token-endpoint calls, about two a minute, until the user signs in again.
   - `token_revoked` is missing from `error_classification` (`src/oauth.rs:181-200`).
   - CPA 514d09f7 and a96e640d treat these token-endpoint answers as final for Codex: `refresh_token_{reused,expired,invalidated}`, `token_revoked`, `invalid_client`, a 401, and `invalid_grant` (the last as a Codex-specific case). CPA then stops refreshing and keeps a still-valid access token in use. Fusebox should do the same: stop renewing, keep serving while the access token lasts, then take the account out of rotation until a new sign-in.
2. **Treat Anthropic's credit-balance 400 as exhausted.** **verified**, bug, S.
   - `quota_exhausted` checks "credit balance is too low" only on 429 and 402 (`src/proxy.rs:625`). Anthropic sends a 400, which takes the `400 | 404 | 413 | 422 | 504` branch (`src/proxy.rs:1616`): no failover, no cooldown.
   - This was found on 2026-10-22 ([credits-research-2026-10-22.md](credits-research-2026-10-22.md)) and is still open.
   - CPA d00267c3 (#7120) fails over with a 30-minute payment-required cooldown.
   - From CPA #6762: Max's monthly API credits are not applied to requests that look like Claude Code. Those need purchased credit, so a Max-credits API key in the pool returns this 400 to every Claude Code request.
   - Fusebox never cloaks API keys (**verified**, `src/upstream.rs:363`, `uncloaked = !oauth || !claude_cloak`). But a real Claude Code client's own identity reaches the key unchanged. CPA's "uncloak" (084b9ca7) strips that identity; Fusebox has no equivalent, and turning `claude-cloak` off does not provide one.
3. **Renew Codex tokens before ChatGPT invalidates them.** S.
   - CPA #6742: Codex access tokens failed about 7 days after issue, although the JWT said 10. The 401 is "Encountered invalidated oauth token for user", code `auth_unavailable`. A 7-day lifetime is **not established**: the reporter could not see the refresh history in between. The 96 h lead is CPA's heuristic.
   - CPA feb63984 now refreshes 96 h before expiry and also refreshes after a 401 that arrives inside a stream.
   - Fusebox refreshes 10 minutes before expiry. An HTTP 401 triggers a refresh and a retry on the same account (`src/proxy.rs:1604`, `src/ws.rs:853-878`), so the client normally sees no failure.
   - The real gap is a 401 inside an SSE stream. Stream bootstrap fails over only on 429 and 5xx (`src/proxy.rs:1935-1938`), and `auth_unavailable` maps to 500 (`src/formats/responses.rs:418`).
4. **Codex `turn_rate_limit_exceeded`.** S.
   - CPA 9a35fa12 and #7017: HTTP 429 `{"code":"turn_rate_limit_exceeded","message":"Codex is at capacity…"}`. Retry-After ranges from 8 to 515 s (median 256). It affects one model, and requests succeed only after the deadline.
   - Fusebox matches "at capacity" (`src/proxy.rs:651`) and honours a Retry-After of up to 10 s. A longer one is discarded, and the same account is retried after 1 s and 3 s (**verified**, `src/proxy.rs:586-604`, `1568-1581`). Fusebox never passes Retry-After to the client.
   - The code is missing from `status_of_code` (`src/formats/responses.rs:418-424`), so an in-stream `response.failed` with it reads as a 500.
   - On WebSocket, a pre-output capacity error is not retried (`src/ws.rs:859-865`).
5. **Small request and response fixes**, each S, mostly hours:
   - Translated error bodies set `"code": <number>` and never `context_length_exceeded` (**verified**, `src/formats/mod.rs:508`). CPA 43f27e3a adds it for "prompt is too long". Responses and Chat clients key on it, Codex to compact (**inferred**).
   - Passthrough forwards every upstream frame (**verified**, `src/proxy.rs:2187`). That includes Codex's new private `response.metadata` event and `codex.*` events, which strict Responses clients reject (Grok Build: "unknown variant response.metadata"). CPA 86d951cd filters them for non-Codex clients.
   - When `input` replays `web_search_call` items without a `web_search` tool, ChatGPT answers 502 "response protection is unavailable". Codex compaction sends `tools: []`, so long sessions that searched fail to compact. Fusebox treats that 502 as a soft failure and retries it across accounts. CPA e7d4c731 (#6770) adds the tool (`src/upstream.rs:662-725`).
   - Vertex multi-region `us` and `eu` are built as `us-aiplatform.googleapis.com` (**verified**, `src/vertex.rs:16-21`). Google serves them from `aiplatform.{us,eu}.rep.googleapis.com` (CPA 88732ad5, #6867).
   - `CorsLayer::permissive()` sends `Access-Control-Allow-Headers: *` (**verified**, tower-http 0.6.11 `cors/mod.rs:134`). The Fetch spec does not let `*` cover `Authorization`, so browser apps that send `Authorization: Bearer` fail preflight. Use `AllowHeaders::mirror_request()` (CPA 058b803b).
   - The Gemini passthrough forwards `systemInstruction.parts: [{}]` (`src/proxy.rs:1264-1290`), which Google rejects with 400 "required oneof field 'data'" (CPA 4c46f0aa, #7136).
   - Translated Claude requests forward a trailing assistant message (**verified**, `src/formats/claude.rs:376-418`, no prefill handling). Haiku 5.5 and Opus 5 or newer reject prefill (CPA #6692).
   - Translated Claude requests that replay signed thinking send no `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` and no `thinking-binding-controls-2026-08-01` beta. Per CPA #6692 and #6697, accounts created on or after 2026-08-31 then get 400 "block is bound to a different conversation" when history or tools change. Not checked against Anthropic's docs.
   - Opus and Sonnet 5.5 on Antigravity (CPA's fix names only these two): Cloud Code turns `responseSchema` into a forced tool, and these models reject forced tools (CPA 1d45a5f3, #7000). Fusebox keeps `responseJsonSchema` for Claude models (`src/formats/gemini.rs:324-327`, `src/antigravity.rs:286-292`) (**inferred** failure).
   - The Devin `swe-2-{medium,high,max}-lightning` UIDs are not grouped (CPA 3dae075d; `src/devin.rs:68`).
   - Schema cleaner (CPA b2fda98e, ec06c3cb):
     - Local `$ref`s are inlined first (`src/schema.rs:111`). A placeholder added inside `allOf` can still override the real array items (**inferred**, `src/schema.rs:208-228`).
     - Numeric-string constraints are not converted when Gemini is translated out (`src/formats/gemini.rs:150-167`).
6. **Meta Muse Code `client_attestation_denied`.** S, but see the caution.
   - Meta now answers 403 "This API key is billed to a Muse Code subscription. Muse Code subscriptions cover requests from the Muse Code client only." It fires when `tools[].name` is one of a case-sensitive list: `TodoWrite`, `bash`, `shell`, `read_file`, `write_file`, `edit_file`, `apply_patch`, `update_plan`, `request_user_input` (CPA #7175).
   - CPA's fix appends a fake `muse` tool namespace and bumps to 1.4.14 headers (bb6e1aa, 5d8fddd). That works around a client check the provider added on purpose; **don't copy it**.
   - CPA's namespace is also a callable reminder tool the caller never declared, so copying it would be wrong on correctness grounds as well.
   - Fusebox's side (**verified**, `src/proxy.rs:680-685`, `1604-1614`): on an OAuth credential the 403 re-mints the key, retries, then cools the account for 10 minutes. A static key is cooled at once. Either way, one Claude Code or Codex request takes the Meta account out of rotation for everyone.
   - Match the exact `client_attestation_denied` code and treat it as a per-request rejection: no refresh, no account cooldown, keep Meta's message, and fail over only to a configured route that serves the same requested model.
   - Treat the tool-name list as observed behaviour, not a permanent rule.
7. **Server hardening.** S.
   - No listener sets a hyper timer, so hyper's 30 s header-read timeout is inactive (**verified**, `src/main.rs:388-410`, `460-475`; axum 0.8.9 and axum-server 0.8.0 build `Builder::new(TokioExecutor::new())` without `.timer()`). This is the CPA 477605dc class (slow unauthenticated connections). It matters only where the port is reachable by others.
   - `collect` turns an unreadable auth directory into zero file accounts (**verified**, `src/accounts.rs:1080`), and cooldowns and counters reset when it comes back. CPA 57a12996 keeps the last snapshot instead.
   - `file_safe` maps `+` to `_` (**verified**, `src/oauth.rs:438`), so `a+b@x` and `a_b@x` share one credential file. CPA d65840fb adds an identity hash (xAI only).
8. **Sign in with ChatGPT (SIWC).** Strategic, M. CPA shipped it between v8.0.35 and v8.0.40, and the Management Center added a binding UI.
   - **What it is:** a second token set bound to an existing Codex credential. It uses OpenAI's dynamic-agent OAuth: client `dynamic_agent_client`, scope including `resource.invoke chatgpt.tokens.use.direct`, resource `https://api.openai.com/v1`, the client_id issued in the callback, and a redirect to `127.0.0.1:1455`.
   - **How requests use it:** bound requests go to `api.openai.com/v1/responses` with `store:false` and streaming. The User-Agent is CPA's configured agent identifier, not the downstream client's (`codex_executor_request.go:374`). Requests from the Codex CLI keep using the Codex OAuth token (d946c4fb).
   - **Known limits:**
     - `agent_message` items are rejected with `subscription_sharing_unsupported_capability` (#6833).
     - Both tokens share one failure state (#6957); still plausible (**inferred**).
     - #6917 asks whether the Decisions API works with these tokens; it is a request for investigation, not proof that it doesn't.
     - Alpha search's 401 (#6830) is fixed: 0db19025 sends it with the original OAuth token.
   - The 2026-10-13 roadmap (item 2) called it the only ToS-clean ChatGPT path; that needs OpenAI's policy text to confirm. Fusebox has none (**verified** by grep).

## Product ideas from the ecosystem

- **Per-account personal reserve:** stop routing to an account at N% of its weekly window left, so the owner keeps direct ChatGPT and claude.ai use. CPA refused it (#6759, "script it"). It fits the single-owner premise. Fusebox only has the global `five-hour-reserve-percent` (`src/config.rs:49-51`).
- **Route preview:** type a model and see which account and tier would serve it now, and why. Management Center v1.27.0 (routing board) and EasyCLIProxyAPI both shipped this. Fusebox already explains past requests.
- **Count server-tool usage:** web search and fetch, code execution. Web search is billed per search and is unpriced in Fusebox today. CPA #7074 records these.
- **LAN discovery (opt-in):** DNS-SD per draft-su-ai-gateway-discovery-01 (`_ai-gateway._tcp`). CPA's own client app reads it. Low priority, because production is reached over Tailscale.
- **Regression tests:**
  - CPA #7197: a Fable-only 429 that rejects `7d_oi` while `7d` is allowed, on an Ultra account with no 5-hour headers. CPA cooled the whole credential for about 6 days.
  - CPA #6646: a dry pool answers with a retryable 429 and a multi-hour Retry-After, so agents loop on "reconnecting".
  - Fusebox's per-model cooldowns probably avoid #7197 (**inferred**). #6646 applies to Fusebox: a dry pool answers with a generic 429, "all accounts for X are rate limited; next available in Ns", and sets no Retry-After header (**verified**, `src/proxy.rs:1181-1186`, `src/server.rs:672-677`).

Demand is thin. 299 issues since 2026-10-15, three authors wrote 82 of them, and no issue has more than one reaction. The plugin store grew from 115 to 159 entries:
- six more soonest-reset schedulers (fourteen in all);
- two window warmers;
- ordered fallback chains;
- quota and usage dashboards;
- per-key quotas;
- providers: Cursor, Kiro, GLM Coding Plan and OpenCode Go.

No notification plugins were added. Fusebox already has every scheduler, dashboard and governance category built in.

## Already handled in Fusebox (no work)

- Streams that end early: an error event to the client and no account cooldown (CPA f013eb66, 4f2e5270, aa4130e6).
- Early WebSocket failover, but only for quota exhaustion before any frame is forwarded (`src/ws.rs:841-852`). A generic 429 or a capacity error does not fall back, and `response.created` already counts as forwarded.
- The client WebSocket is kept open when upstream drops.
- Explicit `parallel_tool_calls: false`.
- Empty text blocks dropped before cache_control, on translated Claude requests only. They survive native passthrough (`src/proxy.rs:1285`, `src/upstream.rs:490`; CPA df07b1e5).
- Refusal and `model_context_window_exceeded` stop reasons.
- `between_tools` and the adaptive fallbacks.
- `sdk-ts` treated as native Claude Code.
- `usage.speed` and service tier in usage, with fast and priority prices.
- Short Retry-After preferred over the unified reset.
- Partial-write protection on credential reload.
- Cooldowns kept across reloads, and a re-login restores the account.
- No refresh call without a refresh token.
- Grok Video 1.5 Lite.

## Review by GPT-6.1 Sol (max effort), 2026-11-03

A cross-model review checked each claim above against the source and the cited CPA commits and issues; its corrections are folded in above. It also reported these missed items. I have not re-read them; they are its file:line citations:

- **Claude dynamic tools:** `tool_addition.tool` declarations in system messages are lost in translation (`src/formats/claude.rs:78`; CPA 12d97cfe). This matters for Claude Code routed to Codex.
- **Codex replay metadata:** cached output is replayed without removing the output-only `internal_chat_message_metadata_passthrough.content_token_count` (`src/affinity.rs:912`, `src/upstream.rs:704`; CPA 9037d543). That upstream rejects it is **inferred**.
- **Gemini/Vertex translation:** streaming `partialArgs`/`willContinue` are unsupported, and an omitted role always becomes `user` (`src/formats/gemini.rs:387`, `61`; CPA 64316127, c888ff72).
- **Antigravity scope:** Fusebox's OAuth omits the `aicode` scope CPA added (`src/antigravity.rs:25`; CPA 7c627ffb). The impact is **inferred**, and a wider scope needs a fresh sign-in.
- **xAI historical tools:** native Responses passthrough does not promote `additional_tools` into top-level tools (`src/upstream.rs:1048`; CPA 0c61b12c). That upstream rejects it is **inferred**.
- **Meta telemetry:** `response.subscription_usage` is forwarded to clients unfiltered (`src/proxy.rs:2187`; CPA 3a072298).
- **Devin fast effort:** values like `high-fast` lack CPA's suffix-preserving mapping (`src/devin.rs:136`; CPA ecbaa6c7).
- **Kimi/Meta compaction:** no handler for ordinary Responses input that contains `compaction_trigger` (`src/upstream.rs:982`, `1044`; CPA 344e0ef9). The client impact is **inferred**.

Its order for this deployment:

1. Refresh failures (item 1).
2. `turn_rate_limit_exceeded` (item 4).
3. The web_search tool during compaction (item 5, third bullet).
4. In-stream 401 recovery (item 3).
5. Claude interoperability: thinking binding, `context_length_exceeded`, prefill, dynamic tools, and empty text on native passthrough.
6. The credit-balance 400 (item 2); move it up if Anthropic API keys carry real traffic.
7. The rest of item 5.
8. Server hardening (item 7); move it up if untrusted clients can reach the port.
9. The Meta 403 (item 6).
10. SIWC (item 8).

It also noted that "a few hours each" for item 5 is unverifiable until each fix is scoped.

## Not applicable

The plugin host, the Home cluster, per-API-key connection pools, request-log file fixes (Fusebox writes no request log files), the Claude web-search result cache, Kimi replay sharing, and Antigravity credits recovery.

## Sources

- CPA releases v8.0.33 to v8.0.52, and commits read with `git show` from a clone of history since 2026-10-10.
- CPA issues #6646, #6692, #6697, #6742, #6759, #6762, #6770, #6830, #6833, #6867, #6917, #6957, #7000, #7017, #7120, #7136, #7175, #7197.
- Management Center releases v1.26.5 to v1.27.2.
- CPA-Plugins-Store `registry.json` history.
- draft-su-ai-gateway-discovery-01.
- The production journal of `cli-proxy-api.service` since 2026-10-20, read only.
