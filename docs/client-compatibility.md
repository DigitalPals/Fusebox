# Client compatibility reviews

`.github/client-versions.json` keeps two different facts: `version` is the bundled
client identity, and `reviewed` is the latest upstream release inspected for
compatibility. Advancing `reviewed` does not require a version bump or a Fusebox
release when the wire protocol still works. Never advance it merely because the
release monitor found a larger version number.

Daily release reports are read-only. Each Monday the monitor reconciles a single
compatibility review issue, without per-release comments. Its comparison links
start at `reviewed`, so releases between the bundled identity and the last review
do not keep generating work. All intervening releases after the last review still
need inspection. A failed channel check prevents all issue changes. This is a
review queue, not an automated claim that an unreviewed release is incompatible.

Fusebox forwards Claude Code and Codex client headers only through allowlists,
so a header a new client release adds is dropped until a review allows it. The
*names* (never values) of headers genuine Claude Code/Codex requests sent that
were dropped are recorded — at most 32 per client, for the life of the process —
and shown as `dropped_headers` in `GET /api/client-identities`; the reset
endpoint clears them. A review starts by checking that list: a name appearing
there after a client upgrade is evidence the release sends something new.
Forwarding one is a deliberate change (one header per commit), usually through
the `upstream-header-allowlist` config escape hatch for operators who need it
before a release allows it by default.

## 2026-11-04

Reviewed every client in the weekly compatibility issue (#48) and refreshed the
bundled identities. Codex was reviewed from source; the others are static
reviews of published, checksum-verified artifacts plus the existing local
protocol regression tests. They do not certify live provider accounts.

### Claude Code 2.1.318

Read all [changelog entries](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)
after 2.1.309: 2.1.310, 2.1.311, 2.1.312, 2.1.314, 2.1.316, 2.1.317 and
2.1.318. Version 2.1.313 was never published, and 2.1.315 reached npm without
a changelog entry. Inspected the npm macOS arm64 artifacts below after
verifying their published SHA-512 integrity values. The 2.1.318 build is
`ca1fa9dea62c1027756786c841c548788644560d`, built 2026-11-03T18:52:53Z.

- 2.1.309: `sha512-7ZVUH/WyxWpFfkV90tJBUSnFTQo8wwRxHNOLNj+q8eYp5DNoCU8KOVC3OiNDmo832BrfQImH0XmpWjGqKPulDg==`
- 2.1.315: `sha512-xb/PQjv24GLBj3kGxC779h6aNLI/X+uum54zhr2+EyDz0BKu7pbcLeJfcHSZvEtOvm9+iL7u6/udutNrwvwFtQ==`
- 2.1.316: `sha512-wJiLVMzg2atF7C/a+0wt+yQShWIePsMNL86gIoIJanURY2fxrwHIxrYuxoTW8sWqfj7ai+otV/ik2r7m5aPy1g==`
- 2.1.317: `sha512-Xnf9K4aGBuoYaODmEZ0o/lRvuYUCv2ZIDP+AXv15WVITryGgrsSA7CFL7KH+43kC6WhK8i6oysF6EbGdgBW4Aw==`
- 2.1.318: `sha512-EBKlipLmKY+ZEY+yNft2NegA6Uz34ZMjkzYxUaLF37qitojBJbIcF71F6JwFRlpIPXmiWchftLa9XONg3KBaxQ==`

The SDK remains 0.128.0 with the same `x-stainless-*` headers, and the user
agent keeps the `claude-cli/<version> (external, <entrypoint>)` form. Since
2.1.315 the bundled Bun 1.4.4 runtime embeds Node compatibility version
`v26.12.1` instead of `v26.3.0`; this is the `process.version` the SDK sends as
`x-stainless-runtime-version`, read from the binary rather than a live
request. The billing fingerprint still uses salt `59cf53e54c78` and UTF-16
positions 4, 7 and 20. Its 2.1.318 fingerprint for `Reply OK` is `195`. The
bundled user agent, default runtime profile and billing fixture now use this
release.

The per-model beta rules keep their conditions. `thinking-token-count-2026-05-13`
moved to a per-request check under the same first-party condition, and
`context-1m-2025-08-07` is no longer sent for a model whose own name is
natively 1M. Three values are new, each sent only against a first-party base
URL: `program-use-case-ids-2026-09-17` with a `program_use_case_id` body field
when an organization program use case is selected;
`claude-code-side-query-2026-09-29` on side queries such as session titles;
and `tool-provenance-2026-10-26`, a per-turn beta behind a server-side flag
that defaults on, which adds `ownership_metadata` (owner `mcp`, optional
`server_url`) to MCP tool definitions. Like other first-party per-turn betas,
Fusebox's generated list omits them; native values pass through. The billing
header gains optional `cc_prev_msg`, `cc_prev_prompt_index` and
`cc_prev_turn_index` fields after compaction against a first-party base URL;
generated requests carry no compaction history and omit them.

`metadata.user_id`, `x-app`, the session, agent and request-class headers,
`anthropic-version`, the context-management edit types, thinking assembly and
the OAuth client id, scopes, endpoints and token response handling are
unchanged. The new `loginExpiresAt` and `credential_expires_at` outputs report
the existing `refresh_token_expires_in` date.

Two request headers are new. `x-claude-code-sandy-plover` lists the tools
invoked in the conversation (`[]` before any; connector tools may add a `url`
and `directory_entry`). It was behind a server-side flag that defaulted off;
2.1.318 removed that gate, so first-party requests send it by default. Native
values in the `x-claude-code-` namespace are forwarded. Generated requests do
not include it, as they omit the other first-party-only fields; a Claude Code
client pointed at Fusebox through `ANTHROPIC_BASE_URL` is not first-party and
does not send it either. `x-cowork-enrollment` carries a host application's
Cowork enrollment; it is not forwarded and appears in `dropped_headers` if a
Cowork-hosted client sends it. The other new header names belong to cloud
sessions (`x-ccr-*`), the self-hosted runner, the Claude apps gateway and
Bedrock, not to Messages requests.

The gateway-related changelog items (retrying without a refused beta, `effort`
value, mid-conversation system message or `tool_reference` block; dotted model
IDs; admin effort caps; honoring the API's no-retry signal; auto mode's safety
check answered with 403 or 422) change client recovery, not the request
Fusebox emits. The Opus 4.8 fast mode deprecation is server policy. This
static review compared release binaries, not source; it did not run Claude
Code, capture live requests or read server-side flag values for real accounts.

### Codex CLI 0.166.0

Compared the complete `rust-v0.164.0` and `rust-v0.166.0` source archives from
[openai/codex](https://github.com/openai/codex/compare/rust-v0.164.0...rust-v0.166.0),
spot-checked `rust-v0.165.0` and read both release notes; the 0.165.0 tag's
commits outside 0.166.0 are its release commit and backports also in 0.166.0.
The review covered `codex-api`, `core/src/client.rs`, `http-client`,
`websocket-client`, authentication, model providers and protocol types. The
user-agent format, `originator`, `version`, session and thread headers, request
field order, prompt-cache-key policy, token refresh and beta
`responses_websockets=2026-02-06` are unchanged. No new request header name
appears outside tests. `response.create` and `response.interrupt` remain the
only client WebSocket messages.

Codex 0.165 enables in-conversation reasoning-effort changes by default. For the
built-in OpenAI provider and supporting models, Codex keeps the request's
`reasoning.effort` and appends `configuration_update` input items. Fusebox's
native HTTP and WebSocket passthrough, including WebSocket history, carries
them; translated requests ignore them. Codex 0.166 reads Guardian assessments
from `response.metadata`, which Fusebox already forwards to Codex clients.
Guardian reviewer sockets resend `__oailb`/`__cflb` routing cookies captured
from an earlier handshake. Fusebox never forwards or returns cookies, so only
this best-effort colocation hint is lost. With the built-in provider and a
custom base URL, Codex now also sends `x-codex-guardian` and Guardian client
metadata; the metadata passes, while the header stays outside the default
allowlist, as it already was for native traffic. `model_not_found` and
`policy_capacity_unavailable` change only client retry handling; Fusebox emits
neither code itself. Host-assigned `msg_input_` IDs and the experimental
runtime-agent assertion headers need no change. No Fusebox code change was
required; the bundled identity moves to 0.166.0.

Limits: this review read source only. It did not build or run Codex, exercise
live ChatGPT accounts, or confirm server-side handling of `x-codex-guardian` or
of `configuration_update` alongside a forced reasoning effort. The production
`dropped_headers` list was not inspected.

### Grok Build 1.0.70

Read the [stable channel](https://x.ai/cli/stable) and official installer, and
inspected the published macOS arm64 executables for 1.0.67, 1.0.68, 1.0.69 and
1.0.70. All four keep the CLI chat proxy, `x-xai-token-auth` (`xai-grok-cli`),
`x-grok-client-version`, the `grok-shell` client identifier and mode,
`x-authenticateresponse` and the `x-grok-conv-id` header used by Fusebox; their
`x-grok-*` header vocabulary is identical. Updated the bundled version and
header fixtures. The 1.0.70 executable's SHA-256 is
`5c8a688251bf61e35370b828b6a353b0a20fcc3f64000de8a91d426039f29cec`; the
installer publishes no artifact checksum, so the download was checked against
the storage object's MD5 only. The release notes page refused automated access,
and the run-time assembled user agent could not be reconstructed statically.

### Muse Code 1.4.14-R7309.1

Followed the [stable channel](https://api.meta.ai/muse-code/channels/muse-stable)
to its artifact manifest, then verified the macOS arm64 artifact against SHA-256
`1c96114408acbeade9a8fb79e3b4425e5b64d50425938568aef01bdc04ce2e03`.
The binary contains version `1.4.14` with build commit
`dec7d7497fecdad135ec5835a8161dbc92e55b58`, also present in the macOS x86_64
artifact (SHA-256
`6da5a12d3298afba45afec929fc385b516084ed28e9504ae29354dbcdfc4089d`).
Updated both in the API user agent and header fixture. Compared with the
1.4.12 artifact (same SHA-256 as the previous review), the `muse-build/` user
agent template, device authorization/token paths, OAuth client id, key mint path
and its fields, Meta API base, `x-client-id` surface, agent-role header and the
subscription usage snapshot Fusebox reads are unchanged. No 1.4.13 artifact was
inspected. The `; build <hash>)` tail of the user agent is not stored as plain
text in either release, so its format is carried over from the existing
fixture. A version override alone cannot replace this build identity.

### Devin CLI 3000.11.234

Read the [release manifest](https://static.devin.ai/cli/current/manifest.json)
and verified its macOS arm64 archive against SHA-256
`35f17a21e53a49435e42fb9627731726aa8ef7fa63e5c3d9d43d0cc5ca43edaf`, then
compared it with the 3000.11.205 archive. Both carry the same Connect RPC paths,
including `ApiServerService/GetChatMessage`, protocol version 1, the
`application/connect+proto` framing, the same `GetChatMessageResponse`,
tool-call and usage field names, and the same sign-in endpoints and session token
prefix. Updated the bundled metadata version and retained the existing protobuf
layout and framing tests. Field numbers are compiled into code and were not
decoded; this is a static protocol review, not a live inference capture.

### Antigravity 2.29.0

The official update manifest now advertises 2.29.0 (the review issue still
listed 2.28.2). Updated the bundled fallback; the runtime updater continues
fetching the manifest every three hours. Verified the macOS arm64 ZIP against
the manifest's SHA-512
(`fd8c6f4235560a4698d575a097777a0ef335650770653dd91443d36117bbd6a4852618b4236ae6edeb7968e81c929373da4200c1549c13e59629c9969228bef7`),
and inspected the packaged hub startup code and language-server executable.
The hub still sets the `antigravity` identity, `hub` subclient and daily Cloud
Code endpoint. The language server retains `fetchAvailableModels`,
`streamGenerateContent`, `loadCodeAssist`, `onboardUser`,
`retrieveUserQuotaSummary`, the `v1internal` request envelope fields Fusebox
sends and the `response` wrapper it unwraps. Earlier builds were not available
for comparison, so releases between 2.25.0 and 2.29.0 were not inspected
individually.

## 2026-10-24 identity hardening

Fusebox 0.9.0 addresses the identity boundaries described in the
[dated Claude/Codex audit](../notes/claude-codex-identity-audit-2026-10-24.md).
This entry documents implementation behavior; the
[verification report](../notes/provider-identity-verification-2026-10-24.md)
records local checks and latency measurements. The
[production deployment record](../notes/provider-identity-production-2026-10-24.md)
records the subsequent deployment and real-traffic checks. The audit remains a
historical record.

- Native recognition preserves supported application attribution, including
  Codex Desktop and custom app-server identities, with validated version syntax
  and consistent fields. Recognition grants no identity-learning permission.
- Only authenticated named clients with `trust-client-identity: true` can update
  shared Claude/Codex versions and Claude runtime profiles. The default is
  `false`; shared keys and open access cannot learn. Saved observations include
  trusted-source provenance and are revalidated on restore. Runtime profiles
  must match the effective release; invalid, stale or legacy unproven records
  fall back to bundled defaults.
- Generated provider sessions follow the conversation's current account and
  rotate on every switch, including A → B → A, identity replacement, inactivity
  (`provider-session-idle-seconds`, default 1800, range 1–86400) and restart.
  Logical chat/history/tools and prompt-cache-key policy remain separate.
- Claude native requests use filtered provider header namespaces. All Claude
  OAuth Messages identities are rebound to the selected account/device and
  generated conversation session, even with cloaking off. Caller beta values
  survive and the mandatory OAuth beta is added when missing.
- Codex HTTP and native WebSockets preserve managed residency and
  `stream_options.reasoning_summary_delivery`, apply the same safe configured
  headers and bind account/session identity. Routes unable to honor a requested
  Codex residency constraint are rejected rather than silently dropping it.
- OAuth operation errors expose safe classifications rather than raw endpoint
  bodies or transport details; normal inference error-body behavior is retained.
  Outbound HTTP clients do not follow redirects. Subscription inference,
  control/reset pools and TLS resumption state are isolated by account identity.
  Ordinary token refresh preserves sessions, connections and the selected Codex
  workspace; actual identity replacement invalidates that reuse.

**Config → Clients → More** toggles learning for each named client.
**Config → Connections → Client identities** inspects and resets saved profiles;
`GET /api/client-identities` and `POST /api/client-identities/reset` use management
authentication. Reset retains configured versions. See [provider identity](provider-identity.md)
for configuration examples, header precedence and the limits of these controls.
Supported native context and outbound IP/transport remain provider-visible;
these changes make no invisibility or fixed latency claim.

## 2026-10-23

Reviewed the six public stable release channels and refreshed their bundled
identities. The checks below use upstream source and published artifacts, plus
local protocol regression tests. They do not certify live provider accounts or
every feature of the upstream applications.

### Codex CLI 0.164.0

Compared the complete `rust-v0.163.0` and `rust-v0.164.0` source archives from
[openai/codex](https://github.com/openai/codex/compare/rust-v0.163.0...rust-v0.164.0),
including `codex-api`, `core/src/client.rs`, authentication, and protocol types.
The REST and Responses WebSocket shapes and beta
`responses_websockets=2026-02-06` remain in use. Client metadata and incremental
tools continue through Fusebox's native passthrough.

The new `PromptCacheKey::Omit` policy makes the body key independent of the
session-id header. Fusebox previously synthesized a body key from that session,
undoing the client's choice. Shared HTTP/WebSocket cache-key selection now
preserves an omitted or null key from recognized Codex clients. Explicit keys
still win; other clients retain existing derivation. Regression tests exercise
both transports against a real local mock upstream, including explicit keys and
metadata. Authentication refactoring, Windows proxy authentication and
app-server WebSocket compression are client-side changes and do not require a
new inference wire format in Fusebox.

### Claude Code 2.1.309

Read all [changelog entries](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)
after 2.1.302 and inspected the npm macOS arm64 artifacts for both releases after
verifying their published SHA-512 integrity values. The SDK remains 0.128.0; the
billing fingerprint still uses salt `59cf53e54c78` and UTF-16 positions 4, 7 and
20. Its 2.1.309 fingerprint for `Reply OK` is `c22`. The bundled user agent and
billing fixture now use this release.

The 2.1.307 change omits `prompt-caching-scope` behind custom gateways. That
review found no additional change required for the omission. The current
header policy preserves allowed native beta values and ensures the mandatory
OAuth beta, as described in the 2026-10-24 entry above. The
changelog's `/claude-api api-version` item changes a bundled skill's advice,
not the CLI's Messages API version. Native attribution and permission to teach
shared identities are separate decisions under the current policy.

### Grok Build 1.0.67

Read the [stable channel](https://x.ai/cli/stable) and official installer, and
inspected the published macOS arm64 executable. It retains the CLI chat proxy,
`x-xai-token-auth`, `x-grok-client-version`, client identifier/mode,
`x-authenticateresponse`, and conversation-id headers used by Fusebox. Updated
the bundled version and header fixtures. The downloaded executable's SHA-256 is
`036a47ce2987adae04de827f7803bf0cc460bf169b640db168f18ec7acb734db`;
the installer does not publish a separate artifact checksum.

### Muse Code 1.4.12-R6909.1

Followed the [stable channel](https://api.meta.ai/muse-code/channels/muse-stable)
to its artifact manifest, then verified the macOS arm64 artifact against SHA-256
`7118b9ca2227a6ec1b90cc506f8abdd4a35d14d8ab6683c9164b075002a55fff`.
The binary contains version `1.4.12` with build commit
`80d5266d8a4cb84d6d4ec595a4ef39cc596fa128`. Updated both in the API user agent
and header fixture. The device authorization/token paths, OAuth client id,
Meta API base, `x-client-id` surface and agent-role header remain present.
A version override alone cannot replace this build identity.

### Devin CLI 3000.11.205

Read the [release manifest](https://static.devin.ai/cli/current/manifest.json)
and verified its macOS arm64 archive against SHA-256
`d80c39678dc3ac6b62c25c1864ba6881086b4eeaadf9a3e3a4e86a30b41423df`.
Static inspection retains the Connect `ApiServerService/GetChatMessage` endpoint,
protocol version 1 and response fields consumed by Fusebox's decoder. Updated
the bundled metadata version and retained the existing protobuf layout and
framing tests. This is a static protocol review, not a live inference capture.

### Antigravity 2.25.0

The official update manifest now advertises 2.25.0. Updated the bundled fallback;
the existing runtime updater continues fetching the manifest every three hours.
Verified the macOS arm64 ZIP against the manifest's SHA-512, and inspected the
packaged hub startup code and language-server executable. The hub still sets
the `antigravity` identity, `hub` subclient and daily Cloud Code endpoint. The
language server retains `fetchAvailableModels`, `streamGenerateContent` and the
`v1internal` methods that Fusebox uses. No new envelope requirement was found
in this static review.
The runtime manifest version remains separate from the reviewed baseline: a
successful version fetch does not certify a future protocol change.

## Reviewing the next release

Inspect the entire range since `reviewed`, including applicable request-building,
authentication and streaming code or published executable artifacts. Keep stable
protocol fixtures for actual wire changes. Record the evidence and any limits of
the review here, then advance `reviewed`. If a bundled identity changes, update
`version` and the corresponding Rust constants together, and use the actual build
hash where required. Run Rust and Python checks before publishing the changes.

For clients without public protocol source, release manifests establish the
identity but do not prove unchanged request encoding. Keep the next release in
the weekly review queue until the relevant artifacts have been checked.
