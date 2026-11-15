# Routing, retries and sessions

How Fusebox picks an account, what it does when a request fails, and how it keeps coding sessions on one account for prompt caching. The settings themselves are in [configuration](configuration.md).

## When a request fails

A request tries up to `request-retry` account/model choices and at most ten attempts, including temporary retries on the same account. These limits and the two-minute retry budget cover every model in a model group together. A connection that never opened gets one more try on the same account; one that broke after the request went out moves to the next account, so the same account never runs it twice. An upstream HTTP 504 is answered, including in a model group, because the provider may still be working on it.

Streams are held back until their first content, at most 5 seconds after they start. A rate limit or overload that arrives inside a successful response before then moves the request to the next account, and the client never sees it. A stream that sends nothing at all for `first-token-timeout-seconds` (120 by default) moves too, and that account sits out the model for two minutes. When no other account could take the request, the account gets twice as long and stays in, so the client's retry can reach it again.

Rate limits and used-up limits are kept in `.cooldowns.state` in the auth directory, so a restart doesn't send requests straight back to an account that is sitting out.

When a provider refuses a refresh token for good (`invalid_grant`, `token_revoked`, an expired, reused or invalidated refresh token, or a bare 401 from OpenAI's token endpoint), Fusebox stops asking with that token. It first checks the credential file, in case another program already renewed the token there, and uses that one if so. Otherwise the account keeps serving while its access token is valid, then sits out of rotation, as a switched-off account does, until a sign-in or a different refresh token in its file. Its error says to sign in again, and that shows on the dashboard and in notifications rather than as a cooldown. A restart keeps that: `.cooldowns.state` remembers the refused token by its digest, never the token. `invalid_client` is not held: refreshing waits 5 minutes, doubling up to an hour. Vertex and Meta mint their own keys and are not held.

If the auth directory exists but can't be read, its accounts stay as they were, with their sessions and cooldowns, until it can be read again; an empty or deleted directory removes them.

Meta answers a Muse Code subscription's key used from another client with 403 `client_attestation_denied`. That rejects the request, not the account: Fusebox skips Meta's other accounts for that request, tries another provider that serves the same model, and otherwise returns Meta's message. The account gets no pause, error or failed-request count.

When every account for a model is sitting out, the 429 says when the next one is back. If a rate limit, a used-up limit or a quota check keeps it away for a minute or less, the answer carries a `Retry-After` header, which clients wait out before trying again. A longer wait gets no header, because clients would sleep through it; OpenAI clients get a `usage_limit_reached` error with `resets_at` instead, which Codex shows as a usage limit. A sign-in or permission problem is never offered as a wait.

A 429 that says the provider is at capacity ("Codex is at capacity, please try again later", or Codex's `turn_rate_limit_exceeded`) is an overload, not the account's rate limit. If it names a wait of 10 seconds or less, or none, it is retried on the same account after 1 and 3 seconds; a longer wait moves the request to the next account at once, since retries within it fail. If no account answers, only that request fails, with the provider's error and any wait it named as `Retry-After` (at most ten minutes), so Codex sits out just that turn. That holds for a capacity error inside a successful stream too. The model stays available to the account's other sessions.

On a Codex WebSocket, a rate limit or capacity error that arrives before anything else moves the turn to HTTP, with the retries and failover above. A plain rate limit does so only when another account can take the model; otherwise the client gets the provider's own error. WebSocket error events carry any wait in their `headers`, where Codex reads it.

An Anthropic API key whose credit has run out (a 400 "Your credit balance is too low…") sits out for 30 minutes and the request moves to the next account; the dashboard and notifications say "API credit used up". Meanwhile a request with no other account left gets that 400 again, without asking Anthropic, also after a restart. Subscriptions are not paused for it.

ChatGPT's 502 "response protection is unavailable" refuses the request as sent, so it is answered at once, also when it arrives inside a stream, without retries on this account or another, and the account shows no error. The request log counts it as `response_protection_unavailable`.

Codex asks for remote compaction (a `compaction_trigger` input item) by provider, not by model, and Kimi and Meta can't do it, so such a request leaves their accounts out. If only they serve the model, it gets a 400 without calling them: point Codex at those models through a model provider entry of their own with `capabilities.remote_compaction = "unsupported"`, and Codex compacts locally.

Temporary upstream failures (connection failures, capacity 429s and HTTP 500, 502, 503 and 529, including errors before a stream's first content) appear as a recent account error for 30 seconds. The error then clears automatically, even if the account is idle; the original failure remains in the request log. Ready means the account is eligible to serve requests, without sending an inference to probe it.

## Coding sessions and prompt caching

Requests from one coding session stay on the account the session started on, so the provider's prompt cache keeps working. `routing` picks the account for each new session. A session moves only when its subscription runs out of quota, or its account is disabled, removed or can't serve the model, and the replacement then keeps it. When its account is only busy (a rate limit, an overload, a failed attempt), that request is answered by another account and the session goes back afterwards. With no other account free, a session whose account is sitting out gets a 429 saying when it is back, which clients retry, as a new session would. Turn this off with `session-affinity: false` (`routing.session-affinity` in a CLIProxyAPI v8 file).

**Smart quota balancing** (`routing: smart-quota`, or **Config → Routing → Account selection**) first prefers valid banked resets nearing expiry (when enabled), then concentrates new sessions on the account whose weekly limit renews first, so allowance that would expire unused gets used first. Many sessions can share that account: load only affects selection between accounts renewing within an hour of the earliest reset. For example, accounts renewing in two, three and four days receive new sessions in that order, even if the three-day account has only 16% weekly quota left.

Reserves protect existing work. When an account has an in-flight request or a session used in the last five minutes, prefer other accounts for new sessions below the configured **5-hour reserve** or 5% applicable weekly quota remaining. An idle account may admit a new session below either reserve to drain its allowance; subsequent new sessions then protect that session's remaining quota. Existing sessions keep their account until actual quota exhaustion or account unavailability. Explicit session completion releases the reservation immediately, and older saved assignments preserve affinity without reserving quota indefinitely. When every account needs its reserve, or no eligible future weekly reset is known, quota and load decide rather than refusing available allowance.

```yaml
routing: smart-quota
five-hour-reserve-percent: 30 # 0 turns the reserve off; other strategies ignore it
```

In a v8 file both go under `routing` (`strategy: smart-quota`).

Sessions are recognised from what clients already send: Claude Code's session metadata, Codex's `session_id` and `thread-id` headers, a Responses `conversation` id or `prompt_cache_key`. Writing your own client? Send one stable `x-fusebox-session-id` per task (the older `x-cliproxy-session-id` still works). Each client API key has its own sessions. A WebSocket without any of these keeps its account for the connection.

An HTTP request without any of them is recognised by its conversation's first user message, which every turn repeats, so the conversation's turns still share one account and its prompt cache. Cache markers are ignored for this, since clients move them from turn to turn. Conversations that start with the same message share an account; send an explicit session id to distinguish them. These inferred assignments use the same configured idle timeout as explicit sessions (a day by default), allowing reuse across longer cache lifetimes. The Requests page marks them as **Inferred session**. A request with no user message at all is routed on its own.

Assignments are saved to `.routing-sessions.state` in the auth directory (hashed ids, owner-only permissions), so they survive restarts, and forgotten after `session-affinity-idle-seconds` without requests (a day by default). Send `x-fusebox-session-end: true` (or `x-cliproxy-session-end: true`) with a task's last request to release it early. Responses history for `previous_response_id` is kept in memory only, up to 64 MiB. Run one proxy per auth directory. To move an installation to another machine, see [moving Fusebox](moving.md).

Claude/Codex provider sessions have a separate lifetime. Each conversation gets a generated provider session for its current account; every account/subscription switch starts a fresh one, including a temporary A → B → A fallback. Inactivity expires it after `provider-session-idle-seconds` (default 1800; range 1–86400), and restart starts fresh provider sessions. The logical chat, available history and tool state continue across rotation. Routing assignments and prompt-cache keys remain separate; rotating the provider session does not itself replace a cache key. See [session and account identity](provider-identity.md#conversation-and-account-sessions).

The Requests page shows each request's session fingerprint (click it to see the whole session), why its account was chosen, and its cached tokens.

Cache hints carry across formats: compatible OpenAI cache settings and content-block breakpoints, including each breakpoint's lifetime override, between Chat Completions and Responses; and Claude's native cache markers, in their TTL order, when a request is made to look like Claude Code. OpenAI translation preserves message endings so appending another message does not erase an earlier implicit cache boundary. A hint that can't be carried over follows `on-incompatible` below. Public OpenAI documentation establishes the newer controls for Responses; accepting a hint in another format does not establish support on every upstream backend.

Requests translated to Anthropic use its automatic prompt caching when the caller has not supplied explicit cache controls. So do requests that are made to look like Claude Code on a subscription account, which also cache the shared tools and system prompt. This covers the growing conversation, including caller instructions moved by OAuth cloaking. Explicit controls take precedence; cache hits still depend on the provider's minimum prompt size and reuse within its cache lifetime.

On models before GPT-5.6, a stable `prompt_cache_key` helps OpenAI route requests to a reusable cache. GPT-5.6 and later handle routing automatically; the optional key separates cache accounting for customers, workspaces or sessions. Fusebox keeps existing keys stable: requests to ChatGPT accounts or to OpenAI's own API that arrive without one, including Claude-format requests translated for a GPT model, get a key derived from their session. Native Codex clients can deliberately omit that key; Fusebox preserves their choice on HTTP and WebSockets, independently of their session header and any shared cache scope. xAI receives a derived key as its conversation id. A key the client sends is passed on unchanged, and API keys with a custom `base-url` never get one added. Cache-key scope and account affinity are separate choices.

The Requests page and structured completion logs show the requested and applied cache controls, derived key scope, and any translation downgrade. These describe what Fusebox sent; provider usage determines how much cache was actually reused. When an upstream returns `prompt_cache_diagnostics`, request details also show its comparison result, miss reason and reported token counts. Request a comparison with `prompt_cache_options.comparison_response_id` on a supported Responses backend; this does not load previous conversation state or enable caching. Missing or unavailable diagnostics do not imply a hit. Defaults preserve the behavior above. Optional controls are available under Routing in the dashboard or in YAML:

```yaml
prompt-cache:
  scope: session          # session | client | workspace
  namespace: ""           # optional prompt/tool schema version, at most 256 bytes
  on-incompatible: drop   # drop | automatic | reject
```

`client` shares a derived key across sessions authenticated by the same client key. `workspace` also includes the `x-fusebox-cache-workspace` header (1–1024 bytes after trimming); absent or invalid headers fall back to session scope. Unauthenticated requests share one client scope. A nonempty namespace versions derived keys in any scope. Shared scopes do not alter account affinity, so cache reuse still depends on the selected account and provider. Caller-supplied keys always take precedence, and workspace names and raw cache keys are excluded from policy diagnostics. Native Codex WebSockets use the same derivation as HTTP.

Translated Responses usage reports cache writes as `input_tokens_details.cache_write_tokens`, with the earlier `cache_creation_tokens` name repeating the same count for existing clients. Reported OpenAI lifetime details remain under `cache_write_tokens_details` as `ttl_30m_tokens` and `ttl_12h_tokens`. These are parts of the total writes, not extra input tokens. Usage stores them separately from Anthropic's 5-minute and 1-hour writes. Do not add repeated aliases or lifetime subsets to the total.

Shared derived cache keys keep a separate upstream Codex session identifier for each session. Caller-supplied session headers and cache keys retain their existing behavior. These scopes affect derived OpenAI/xAI keys; Claude caching uses its native boundaries and TTLs.

`drop` discards incompatible hints while preserving compatible fields and the caller's explicit caching intent. `automatic` opts into automatic caching on the Claude provider when translation loses explicit boundaries. Explicit mode without boundaries remains uncached; OpenAI and Anthropic TTLs are not interchangeable. `reject` returns `400` before sending a request with incompatible hints. Unsupported prewarming remains an error in every mode. See [context efficiency](context-efficiency.md) for choosing breakpoints and lifetimes and measuring cache reuse and latency with stable prefixes.

Cache intent is checked outside tool schemas, tool-call arguments and structured-output schemas, so a data property named `cache_control` does not disable defaults or trigger a false downgrade. Final Claude OAuth preparation honors the same policy: `drop` and explicit mode without boundaries do not acquire automatic cache controls during cloaking.
