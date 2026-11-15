# Configuration

`config.yaml` reloads automatically when it changes, and the dashboard edits the same file.

The dashboard's Config page starts with **Clients**: the endpoint, a key for each machine or tool, and copyable setup with that key for Claude Code, Codex, OpenCode, the OpenAI SDK and curl, and under **More tools** for Gemini CLI, Aider, Continue, Zed, Cline and Roo Code, and Cursor. Then come forms for server and access settings, routing, connections, provider keys, model rules, notifications and diagnostics, plus a **YAML file** section for everything else. Every save first shows the lines it changes in `config.yaml`. Keys stay masked until revealed, and leaving an existing key blank keeps it unchanged. If the file changes elsewhere while you edit, the page asks you to reload before saving. Bind address, port, HTTPS, debug logging and log format changes need a restart; the page shows which are pending.

**Clients** (`named-clients`) each have their own key and a name, so the Usage page shows which machine or tool sent what, and limits can apply. Add one under Config, Clients by giving it a name: Fusebox makes its key and shows the setup with it. A client can be renamed, given a new key (keeping its name, limits and history) or removed, and the list shows when each key was last used. Other tools, such as Whombat, can add their own with the management key. Shared keys in `api-keys`, from before, still work, but the dashboard no longer makes them: Config, Clients lists them until you name each one, which makes it a client with the same key, so nothing using it needs changing, and its earlier use shows under the new name. **Limits** on a client, each optional, refuse what it may not do with a clear error:

```yaml
named-clients:
  - id: laptop
    label: Work laptop
    key: fbx_...
    trust-client-identity: false     # default; opt in only for clients allowed to teach shared identities
    models: [claude-*, gpt-6-astra]   # model patterns it may ask for
    model-map: {claude-opus-*: claude-sonnet-5-5}  # serve another model instead (see below)
    user-agents: [claude-cli/*]       # programs it may use (User-Agent patterns)
    requests-per-minute: 60           # over it: 429 with Retry-After
    tokens-per-day: 5000000           # input, cache writes and output; cache reads don't count
    usd-per-month: 100                # estimated at API list prices (needs the usage database)
    expires: 2026-12-31               # the last day the key works, server time
    addresses: [10.10.0.0/24]         # addresses or ranges it may come from (see below)
```

Under **Config → Clients → More**, **Allow identity learning** / **Stop identity learning** toggles `trust-client-identity`. Only an authenticated named client with this setting enabled can teach Fusebox Claude Code/Codex releases and Claude runtime profiles. Shared keys and open access cannot learn; recognizing a native client alone does not grant trust. Learned profiles are shared, so enable this only for clients you control. Inspect or reset them under **Config → Connections → Client identities**. See [provider identity](provider-identity.md#trusted-identity-learning).

A client's **model map** (`model-map`, also in its Limits form) rewrites the model it asks for before routing — batch agents can keep asking for Opus and be served Sonnet, without changing their own configuration. Patterns match case-insensitively on the model with any reasoning suffix removed; an exact pattern beats a wildcard, and among wildcards the longest literal wins. The target may be a model group and may carry its own suffix (`gpt-6-astra: "gpt-6-astra-mini(low)"`), which replaces the request's; without one, the request's suffix is kept. The `models` allowlist is checked against the mapped model, and a mapped model is never mapped again. Mapping changes which model answers, not who the request claims to be: client identity and account selection follow the same rules as any other request for that model. Older Fusebox versions refuse a config that uses `model-map`.

Days and months follow the server's time zone. Completed provider attempts update tokens and estimated spend immediately; every 30 seconds, Fusebox reconciles those totals with the usage database. Concurrent requests can still run slightly past a limit. Budget-limited keys receive 503 if accounting is unavailable, incomplete, or more than 90 seconds old, including while the initial totals load. Unknown prices are never counted as zero spend. WebSocket opening handshakes are free; each turn counts toward the request rate. Config, Clients shows each client's use beside its limits, and `GET /api/clients/usage` returns it, with when each key was last used since Fusebox started, the addresses each named key was used from, and which keys and addresses are connected now. A named key used from an address it wasn't used from before sends a notification (`client-new-address`, on by default), as does one refused by its `addresses` list. Where an address comes from behind a proxy is in [Source addresses](source-addresses.md).

```yaml
host: "127.0.0.1"             # 0.0.0.0 to expose it (add a client first)
port: 8317
auth-dir: "~/.fusebox"        # OAuth credential files (CLIProxyAPI's work too)
api-keys: []                  # shared keys from before named clients; with neither, it is open
management-key: ""            # empty = dashboard only from localhost
proxy-url: ""                 # optional http://, https:// or socks5:// upstream proxy; never falls back to direct
request-retry: 3              # account/model choices across the whole request (at most 10)
first-token-timeout-seconds: 120 # a stream silent this long moves to the next account (twice as long when there is none); 0 = off
routing: least-used           # new sessions: most quota left; or smart-quota, round-robin, fill-first
session-affinity: true        # keep each coding session on one account, for prompt caching
session-affinity-idle-seconds: 86400 # forget assignments after a day without requests
provider-session-idle-seconds: 1800 # fresh provider identity after 30 minutes idle; 1–86400
codex-websockets: true        # native WebSocket relay to ChatGPT
claude-cloak: true            # present non-Claude-Code clients as Claude Code on OAuth accounts
claude-subscription-fast-mode: false # send Claude fast mode to subscriptions only with usage credits
banked-resets: false          # show and spend banked Claude/ChatGPT limit resets (unofficial endpoints)
tls:                          # serve HTTPS directly (or a proxy in front: install.md)
  enable: false
  cert: "/etc/fusebox/cert.pem"
  key: "/etc/fusebox/key.pem"
notifications:                # push notifications, for devices that turn them on
  sign-in-expired: true
  provider-exhausted: true    # every account of a provider is out, and when one is back
  account-used-up: false
  account-errors: false
  banked-resets: true         # a banked reset used automatically, needing review, or expired unused
  webhooks:                   # the same messages in Discord, Slack, Teams, Telegram, ntfy or any URL
    - https://discord.com/api/webhooks/...
log-format: text              # or json, one object per line (needs a restart)
model-groups:                 # one name, several models; see "Model groups" below
  smart: [claude-opus-5-5, gpt-6-astra]
background-model: gemini-3.8-flash # Claude Code's quick Haiku calls go here
# client-identities:          # operator-selected emulation versions (see Client compatibility)
#   claude-code: 2.1.319
# upstream-header-allowlist:  # optional exact extra inbound names; reserved/private names are rejected
#   claude: [x-provider-feature]
#   codex: [x-provider-feature]

claude-api-key:
  - api-key: "sk-ant-..."
codex-api-key:
  - api-key: "sk-..."
    priority: -10             # only when the accounts above it can't (sign-ins: "priority" in their file)
gemini-api-key:
  - api-key: "AIza..."
vertex-api-key:               # Vertex express mode (service accounts go in auth-dir)
  - api-key: "AQ..."
kimi-api-key:
  - api-key: "sk-kimi-..."    # Kimi Code; Moonshot platform keys need base-url
xai-api-key:
  - api-key: "xai-..."
meta-api-key:
  - api-key: "..."
openai-compatibility:
  - name: openrouter
    base-url: "https://openrouter.ai/api/v1"
    api-keys: ["sk-or-..."]
    models:
      - name: "moonshotai/kimi-k3"
        alias: "kimi-k3"
  - name: ollama
    base-url: "http://127.0.0.1:11434/v1"
    strict-chat-template: true   # translations: one system message and alternating turns
    models:
      - name: "qwen3-coder:30b"
```

## Model groups and the background model

A model group is one name for several models. Clients ask for the group as they would for a model, and it is listed by `/v1/models`. Each model takes over when the one before it can't answer: no account serves it, every account is rate limited or used up, or the provider fails. A bad request (a prompt too long, say) is answered at once instead. Name a group after a real model to give that model fallbacks:

```yaml
model-groups:
  claude-opus-5-5: [claude-opus-5-5, gpt-6-astra]   # Opus, then GPT when every Claude account is out
  fast: { models: [claude-sonnet-5-5, gemini-3.8-flash], balance: true }
```

With `balance: true`, new sessions start on different models instead of always the first; a session keeps its model, so its prompt cache keeps working. A group's models are always models, never other groups.

`background-model` sends Claude Code's quick background calls (it asks its Haiku model for titles, summaries and checks) to another model or group, to keep them off your subscription's limits. Other clients asking for Haiku get Haiku.

## Account priority

Every account has a priority, 0 unless set. New sessions go to the highest priority with an account ready; the routing strategy chooses among those, and lower priorities only take over when none of them can. Set it on the account's page (**First**, **Normal** or **Last**), as `priority` on an API key in `config.yaml`, or as `"priority"` in a sign-in's credential file, as CLIProxyAPI does. Sessions already on an account stay there.

The account's **Enabled** switch survives configuration reloads and restarts. Sign-ins keep it in their credential file; API-key accounts (including individual compatible keys and keyless endpoints) keep account IDs in `.disabled-api-keys.state` in `auth-dir`. That file contains no API keys. Keep it with the credentials when moving an installation.

## Request and stream limits, translation

HTTP POST body limits apply to both encoded and decoded bytes. The defaults retain 256 MiB per request. Optional admission limits cover body collection, provider work and response streaming, returning `503` with `Retry-After: 1` when full:

```yaml
request-limits:
  text-body-bytes: 268435456
  media-body-bytes: 268435456
  max-in-flight: 0
  max-in-flight-body-bytes: 0
```

Body limits must be 1–268435456 bytes. `media-body-bytes` applies to `/v1/images/*` and `/v1/videos/*`; other API POST routes, including multimodal Gemini calls, use `text-body-bytes`. Zero disables each aggregate limit. The byte budget counts decoded request bodies through response completion and releases reservations on errors or cancellation. It bounds body bytes, not total RSS: parsed JSON, histories and responses need additional memory. Admission is fail-fast and process-wide; WebSockets and management routes are separate. These settings reload for new requests without a restart and are available under Connections in the dashboard.

Upstream streams also have configurable bounds, under Connections:

```yaml
stream-limits:
  event-bytes: 16777216
  media-event-bytes: 67108864
  bootstrap-bytes: 2097152
  pre-output-timeout-seconds: 0
```

The decoder bounds SSE events and unfinished lines; native WebSocket events also have a size check. Models whose names contain `image`, `audio` or `video` use the larger media limit. Once the first event has arrived, reaching the bootstrap threshold releases buffered chunks for streaming; it does not repeat inference. Before the first event, the first-token deadline still applies and the event limit bounds buffering. An oversized event ends the stream with an error and retains usage already received. A nonzero pre-output timeout gives HTTP headers and the first event one absolute deadline, or bounds the first native WebSocket event including its send. Expiry is terminal `504`, with no automatic inference retry. Zero keeps the existing timeout behavior.

Output limits are preserved during translation. Manual Claude thinking needs an output limit greater than 1,024 tokens; incompatible requests receive a `400` instead of silently increasing the limit. Gemini reasoning suffixes use the same mapping for native and translated requests. Claude 4.6 and later, Fable and Mythos reject a final assistant message (prefill), so translated requests to them leave it out and record `prefill_unsupported` in request diagnostics; native Claude requests are sent as they are.

Chat↔Responses translation retains late system/developer messages in their original positions, with cache boundaries. ChatGPT subscription requests map system roles to developer roles at the OAuth boundary, on HTTP and WebSocket transports. Public OpenAI API requests keep their original roles. Routes to Claude, Gemini or Devin add late instructions to the system prompt in their original order, as before, rather than demoting them to user turns. Translated low-verbosity preferences are retained for supported GPT-5/6 models on the Codex provider. Translated OpenAI service tiers require `translation.allow-service-tier: true`; dropped controls appear in request diagnostics. Native requests keep caller controls. Codex subscription preparation still removes output caps that its backend does not accept, so those caps are not a subscription billing limit.

For a local Chat backend whose template rejects developer roles, multiple system messages or repeated turns, set `strict-chat-template: true` on that `openai-compatibility` entry. Translated requests then collect instructions into one leading system message and merge adjacent user/assistant turns. Tool replies stay separate and explicit cache markers keep their metadata. This changes message endings and the position of late instructions, so enable it only for backends that require that layout. Native Chat requests remain under the caller's control.

Optional `latency-routing: true` breaks exact Smart quota ties for new HTTP or WebSocket sessions using recent comparable first-response timings. It requires three successful single-attempt samples with explicitly zero cache reads per account, matches model, input-size bucket, reasoning effort, cache intent and transport, and leaves existing session assignments intact. It is off by default.

Token-count responses include `x-fusebox-token-count-estimated: false` when Claude supplies a count, or `true` for the local fallback. The fallback estimates text and tool schemas and uses fixed allowances for media; it does not count base64 data as text. Media dimensions, document length and audio/video duration can change the real count, so use provider usage for accounting.

An upstream stream that ends before its completion event returns an error, retaining any usage received. Closing a client WebSocket cancels its active turn; a client that stops reading is disconnected after a bounded write wait. Session assignments are saved by a background writer, with new assignments persisted before the provider request starts.

The dashboard separates cancelled requests from failures. Missing usage appears as unknown, and partial usage is a lower bound. Request logs retain safe failure categories, transport, timing, attempts and usage completeness without recording upstream error bodies. To measure whether smaller contexts help your workload, use the opt-in [context comparison benchmark](context-efficiency.md); Fusebox does not silently rewrite prompts.
