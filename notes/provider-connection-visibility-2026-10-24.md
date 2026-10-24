# Provider connection visibility

Research date: 2026-10-24. Source reviewed: commit
`0002d85c7e1714ddd2e84a3d6611d3eeca5f19b0` (Fusebox 0.8.1).

## Finding

We cannot confirm that providers are unable to recognize Fusebox or an
intermediary. Several default request paths explicitly identify Fusebox or
CLIProxyAPI. Other paths preserve or imitate an official client's application
identity, but that does not make their connection indistinguishable from a
direct connection by that client.

Three different questions need separate answers:

- **Can the provider see the name Fusebox/CLIProxyAPI?** Yes, on the paths
  identified below.
- **Can the provider observe evidence consistent with an intermediary?** Yes:
  its edge receives the outbound connection, headers, credentials and request
  body. Whether it classifies those observations as a proxy is not established.
- **Does the provider automatically see the dashboard, routing configuration,
  client keys or private network behind Fusebox?** No such automatic visibility
  follows from this architecture. Individual fields may still be disclosed by
  request content, custom headers or device metadata.

## Explicit application identities by provider

These describe defaults in the source, not a verified production configuration.
Per-account custom headers are applied after preparation and can override
inference headers ([upstream.rs](../src/upstream.rs#L143)). This does not change
the fixed Kimi headers sent by the sign-in code.

- **Anthropic / Claude:** A recognized native Claude Code request forwards its
  application headers after filtering. Other OAuth requests with `claude-cloak`
  enabled claim Claude Code. The remaining branch sends
  `User-Agent: Fusebox/0.8.1`, including non-native API-key calls and non-native
  OAuth calls with cloaking disabled
  ([upstream.rs](../src/upstream.rs#L170), especially lines 183–244).
- **OpenAI / Codex:** OAuth requests preserve recognized Codex client identity
  or synthesize a Codex user agent, version and originator. The API-key branch
  sends `User-Agent: Fusebox/0.8.1`
  ([upstream.rs](../src/upstream.rs#L554)). Native Responses WebSocket
  preparation uses the same identity selection
  ([upstream.rs](../src/upstream.rs#L631)).
- **Gemini API:** Sends `User-Agent: Fusebox/0.8.1`
  ([upstream.rs](../src/upstream.rs#L642)).
- **Vertex AI:** Sends `User-Agent: Fusebox/0.8.1` for both service-account
  bearer authentication and API-key express mode
  ([upstream.rs](../src/upstream.rs#L669)).
- **Antigravity:** Claims `antigravity/hub/<version> darwin/arm64` and wraps
  requests with `userAgent: antigravity`, request type, request ID, project and
  session information. This is client emulation, without an explicit Fusebox
  marker in this preparation path
  ([upstream.rs](../src/upstream.rs#L700),
  [antigravity.rs](../src/antigravity.rs#L62),
  [envelope](../src/antigravity.rs#L269)).
- **Kimi:** Inference sends `User-Agent: Fusebox/0.8.1`. OAuth inference also
  sends `X-Msh-Platform: CLIProxyAPI`, `X-Msh-Version: 0.8.1`, device name,
  device model and device ID. The device name is taken from `HOSTNAME` or
  `COMPUTERNAME`, falling back to `localhost`; the model includes the host OS
  and architecture. The same platform/device headers are sent during device
  authorization, token polling and refresh
  ([upstream.rs](../src/upstream.rs#L734),
  [device.rs](../src/device.rs#L57),
  [authorization](../src/device.rs#L103),
  [polling](../src/device.rs#L176),
  [refresh](../src/oauth.rs#L448)).
- **xAI / Grok:** Official subscription routing claims Grok Build identity and
  uses the CLI chat proxy. API-key and other non-official-subscription paths
  send `User-Agent: Fusebox/0.8.1`
  ([upstream.rs](../src/upstream.rs#L805)).
- **Meta / Muse:** Claims the bundled Muse build identity, `x-client-id:
  tbh:tui`, and `x-tbh-agent-role: main`. Authentication separately claims
  `muse-code/1.0.2`
  ([upstream.rs](../src/upstream.rs#L815),
  [device.rs](../src/device.rs#L31)).
- **Devin:** Sends Connect-RPC protobuf with client name `devin-cli`, the
  bundled/selected CLI version, host OS and a synthesized device fingerprint.
  Its prepared HTTP headers do not explicitly name Fusebox
  ([devin.rs](../src/devin.rs#L246),
  [metadata](../src/devin.rs#L264),
  [request preparation](../src/devin.rs#L478)).
- **OpenAI-compatible endpoints:** Send `User-Agent: Fusebox/0.8.1`
  ([upstream.rs](../src/upstream.rs#L857)). The endpoint operator is the
  immediate observer; any additional forwarding by that operator was not
  audited.

## What preserving a client identity does and does not preserve

### Transport and outbound address

Fusebox parses/prepares requests, serializes a new body and sends it through its
own reqwest client ([proxy.rs](../src/proxy.rs#L1407)). Native Codex WebSockets
also create a new outbound connection using tokio-tungstenite
([ws.rs](../src/ws.rs#L564)). This is not an end-to-end encrypted tunnel from
the original client to the provider.

The provider or its TLS-terminating edge therefore sees the outbound public IP
of Fusebox's network/NAT, or the configured egress proxy. It also receives
Fusebox's TLS handshake and HTTP behavior. The original private address is not
automatically exposed merely by terminating TLS locally. An identical public IP
can be shared by ordinary users behind NAT; an address alone does not prove a
proxy.

The TLS implementation is standard rustls with ring defaults and selected ALPN
protocols ([tls.rs](../src/tls.rs#L21),
[state.rs](../src/state.rs#L281)). It does not reproduce an arbitrary original
client's transport. This can differ from clients built on Node or browser TLS;
it does not establish a distinct fingerprint from every direct client,
particularly another Rust client.

Subscription inference has per-account connection pools, and Antigravity
inference uses HTTP/1.1 ([state.rs](../src/state.rs#L240)). These are useful
compatibility properties, not proof of indistinguishability. The TLS configs
share session resumption state within each config class/host
([tls.rs](../src/tls.rs#L21)); cloned rustls configs share an `Arc` session
store. If the server issues and accepts resumable sessions, this creates another
possible link between connections even when their HTTP pools are separate.
No live cross-account resumption capture was performed.

### Provider-visible account and request metadata

Providers receive their own API key/bearer token and relevant account/project
identifiers. They also receive the prepared prompt, tool schemas, session/cache
metadata, request timing and volume.

For example, cloaked Claude requests synthesize billing/identity system blocks
and, when needed, `metadata.user_id` containing device, account and session IDs
([upstream.rs](../src/upstream.rs#L280),
[metadata](../src/upstream.rs#L401)). The account session ID is generated on
account construction rather than taken separately from every incoming task
([accounts.rs](../src/accounts.rs#L1268)). Codex preparation sanitizes and
reorders fields and can supply session/cache identities
([upstream.rs](../src/upstream.rs#L465),
[Codex request](../src/upstream.rs#L597)). Antigravity derives a missing
conversation session ID from the first user text
([antigravity.rs](../src/antigravity.rs#L251)). These are observable request
choices. Their use by any provider's classifier is unknown.

Identity learning remembers newer observed/configured client versions and
Claude runtime headers; it is not a capture/replay of the client's entire
network stack ([clients.rs](../src/clients.rs#L1)).

### Header filtering and its boundary

Native Claude passthrough removes common forwarded-address headers, incoming
authorization/cookies, `x-fusebox-*`, `x-cliproxy-*` and `tailscale-*`
([upstream.rs](../src/upstream.rs#L53)). This reduces direct disclosure of
internal routing and client location.

However, `via` is not in that filter. A native Claude Code request that arrives
with `Via` can have it forwarded upstream by the general header-copy loop
([upstream.rs](../src/upstream.rs#L190)). This is conditional on an incoming
header; it is not evidence that the deployed front proxy adds one. Arbitrary
per-account custom headers are also sent after preparation.

### Authentication and background traffic

An inference-only inspection misses OAuth code exchange/refresh, profile reads,
model discovery, usage polling and optional window/reset actions. They also use
Fusebox-owned outbound clients. Claude/Codex model and usage calls generally
claim their corresponding official client identities
([discovery.rs](../src/discovery.rs#L348),
[quota.rs](../src/quota.rs#L401)); Claude token exchange instead claims
`axios/1.15.2` ([oauth.rs](../src/oauth.rs#L163)). Optional reset/window calls
use CLI identities too
([banked_resets/provider.rs](../src/banked_resets/provider.rs#L258),
[window_starter/mod.rs](../src/window_starter/mod.rs#L319)). Kimi authentication
explicitly names CLIProxyAPI as described above.

## Official documentation checked

- [Anthropic: Claude Code gateway compatibility](https://code.claude.com/docs/en/llm-gateway-protocol#system-prompt-attribution-block).
  Documents system-prompt attribution containing the client version and a
  conversation-derived fingerprint, read by Anthropic and cloud-provider
  endpoints for attribution. It also documents session/agent and capability
  headers. Matching a user agent alone does not reproduce every attribution
  surface.
- [Cloudflare: JA3/JA4 fingerprints](https://developers.cloudflare.com/bots/additional-configurations/ja3-ja4-fingerprint/).
  Documents identifying TLS clients from how they initiate connections, with
  associated HTTP-protocol, request, user-agent and IP statistics. This
  establishes fingerprinting capability at a terminating edge, not deployment
  of those particular features by any specific Fusebox provider. A shared
  fingerprint is not proof of a particular proxy.
- [OpenAI: Sign in with ChatGPT through a gateway](https://learn.chatgpt.com/docs/enterprise/sign-in-with-chatgpt-through-a-gateway).
  Documents proxying `/models` and `/responses` to the Codex backend, retaining
  provider authorization/account headers while removing gateway credentials.
  It also states that sign-in, MCP, plugins and other connections can use
  different paths. This establishes a supported gateway architecture in that
  documented context; it does not certify Fusebox or promise that a gateway is
  undetectable.
- [OpenAI: Custom model providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers).
  Documents custom provider base URLs and HTTP headers. Configurability is not
  an invisibility guarantee.
- [Google Antigravity Additional Terms](https://antigravity.google/terms)
  describe recording user/interaction data and related metadata, with stated
  scope and enterprise exceptions.
  [Google's Privacy Policy](https://policies.google.com/privacy#infocollect)
  describes IP, device/application, request-time and account-associated data
  collection. These broad categories do not prove that every listed field is
  supplied by Fusebox; they do not promise provider-side anonymity either.

## Scope and confidence

The explicit identity findings follow directly from request preparation and
authentication source. They are sufficient to reject a blanket claim that
providers cannot see the proxy as such.

This was a source/documentation review, not an authenticated production traffic
capture. No credential files were read, no inference was triggered, and no
service configuration was changed. The review does not establish what private
provider detection systems actually flag, whether a particular account has
been classified, or what an ultimate model vendor sees behind Devin,
Antigravity or another aggregator.

Recommended description: **Fusebox preserves or emulates official client
identities on selected subscription routes and filters some internal headers.
It does not provide provider-side invisibility, anonymity or non-detection.**
