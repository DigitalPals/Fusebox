# Provider identity and header policy

Fusebox preserves supported native Claude/Codex application attribution while
binding subscription credentials and generated provider sessions to the account
selected for each request. Local mock-provider validation and latency results
are recorded in the [implementation verification](../notes/provider-identity-verification-2026-10-24.md).
The
[2026-10-24 audit](../notes/claude-codex-identity-audit-2026-10-24.md) records the
earlier findings and remains unchanged.

## Conversation and account sessions

A logical conversation has a generated provider session for its current period
on one selected account identity. HTTP requests and native Codex WebSocket turns
use the same session registry. Active turns on that account reuse the session.
Every subscription/account switch generates a fresh one: A → B → A produces
three different provider sessions, including temporary fallback away from an
otherwise pinned account.

```yaml
session-affinity: true                 # routing preference, independent of provider identity
session-affinity-idle-seconds: 86400    # routing assignment retention
provider-session-idle-seconds: 1800     # provider-session inactivity, 1–86400 seconds
```

The provider inactivity default is 1800 seconds (30 minutes), measured from the
last provider-session use. The next request after that interval gets a new
session. Ending a task also releases its provider session. Provider-session
mappings live only in memory and start fresh after restart; the bounded registry
may also evict old mappings. Requests without a conversation binding get fresh
sessions rather than sharing one account-wide identity.

Rotation keeps the app's logical chat and available history/tool state intact.
A native Codex upstream socket reconnects when its generated session changes;
Fusebox can replay available conversation history on the new connection. If a
`previous_response_id` cannot be resolved there and local full history is
unavailable, the client must resend full input. Responses history itself is
memory-only and is not recovered after restart.

Provider sessions, routing assignments and prompt-cache keys have separate
lifetimes. Saved routing assignments can survive restart. Session rotation does
not itself rewrite a caller's `prompt_cache_key` or change derived cache-key
scope. Cache reuse still depends on the provider, selected account and cache
lifetime. See [coding sessions and prompt caching](../README.md#coding-sessions-and-prompt-caching).

Routine access/refresh-token rotation keeps the account's sessions, connection
pools and TLS resumption state. Codex refresh retains the explicitly selected
ChatGPT account/workspace rather than replacing it from new ID-token claims.
An actual credential identity replacement, including another account loaded
under the same filename, invalidates old identity state and rotates provider
sessions and account connection/TLS reuse. Claude replacement device identity is
rebound as well.

Signing in again to an account Fusebox already has saves into that account's
existing credential file, whatever it is named (older releases used other names,
such as `claude-d204ac98-<email>.json`). The account keeps its ID, priority,
window-starter selection and sessions, and no stale copy is left retrying a
revoked refresh token. The same account means the same provider and account ID;
a Claude file without one is matched by email, a ChatGPT one only by its
workspace's account ID. A refresh of the old tokens that is still running finishes
first, then the new tokens take effect at once and an error from the old sign-in
(such as `invalid_grant`) is cleared. Any Claude or ChatGPT sign-in, new or
repeated, then reads its usage and plan straight away, so its meters fill in
without waiting for the next minute's poll.

## Trusted identity learning

Native-client recognition controls how a request is prepared; it does not
authorize learning. Only an authenticated named client explicitly trusted in
configuration can update shared Claude Code/Codex versions and Claude runtime
profiles. `trust-client-identity` defaults to `false`. Shared API keys and open
access cannot learn, regardless of claimed user agent or caller-supplied trust
headers.

```yaml
named-clients:
  - id: workstation
    label: Workstation
    key: fbx_replace_with_your_client_key
    trust-client-identity: true

# Optional operator-selected versions for generated requests:
client-identities:
  claude-code: 2.1.310
  codex: 0.164.0
```

**Config → Clients → More → Allow identity learning** enables trust;
**Stop identity learning** disables it. Use this for clients you control: the
learned version/runtime profile is shared across clients and accounts, not
isolated to the client that taught it. Stopping learning or removing trust does
not erase previously learned values; reset them separately when needed.

Generated identities use the newest applicable bundled, configured or validated
learned release. Configured versions older than the bundled one are ignored.
Learning checks stable version syntax, consistent identity fields and bounded
advances anchored to the bundled release. Operator overrides do not widen those
learning bounds. Claude runtime learning also requires a complete, semantically
valid SDK/Node/OS/architecture profile for the effective Claude release. An
older runtime profile cannot be relabelled as a newer release: a missing or
invalid matching profile uses bundled runtime defaults.

Saved observations carry the authenticated named-client ID and trusted-source
provenance. Restore revalidates versions, provenance and runtime fields; legacy
observations without trusted provenance and invalid/stale profiles are ignored.
These are locally recorded trust decisions, not provider attestation that the
client is authentic or that its claimed release has been reviewed.

**Config → Connections → Client identities** shows effective, bundled,
configured and observed versions, runtime values and their sources. **Refresh
profiles** reloads that view. **Reset learned profiles** clears learned versions
and runtime profiles and saves the cleared state, retaining `client-identities`
configuration. Trusted clients can teach profiles again on later requests.

The equivalent management-authenticated endpoints are:

```sh
curl http://127.0.0.1:8317/api/client-identities \
  -H 'Authorization: Bearer <management-key>'
curl -X POST http://127.0.0.1:8317/api/client-identities/reset \
  -H 'Authorization: Bearer <management-key>'
```

They use the dashboard's management access rules, including its existing local
access policy, rather than ordinary inference-client authentication. Inspection
returns version/profile provenance, not client keys or provider tokens. Reset
affects learned profiles; it does not reset the conversation-session registry.

## Inbound and configured outbound headers

Claude's built-in inbound policy accepts safe `anthropic-*`, `x-claude-code-*`
and `x-stainless-*` provider namespaces, plus supported user-agent, app and
request-correlation headers. Codex uses explicit supported names for application
version/originator, thread/window/turn context, request correlation and provider
controls. Both drop gateway credentials, cookies, forwarding/user-identity
headers and private Fusebox/CLIProxyAPI headers. Hop-by-hop headers, including
names nominated by the incoming `Connection` header, are filtered.

For an additional provider header, allow its exact name explicitly:

```yaml
upstream-header-allowlist:
  claude: [x-provider-feature]
  codex: [x-provider-feature]
```

Only `claude` and `codex` maps are supported. Names are case-insensitive and
exact; wildcards are not supported. Configuration rejects reserved identity,
authentication, private and hop-by-hop names. An extension cannot bypass the
private-header filter or restore a `Connection`-nominated header. Allow only
provider-required values: accepted header content remains visible upstream.

Per-account `headers` configure operator-supplied outbound values independently
of the inbound allowlist, for example on an API-key entry:

```yaml
codex-api-key:
  - api-key: sk_replace_with_your_api_key
    headers:
      x-provider-feature: enabled
```

Safe configured values replace the same ordinary forwarded header. They cannot
override protected authentication, selected account/session identity, application
identity or transport controls. Configuration rejects malformed or protected
names and invalid values. Intentional gateway credentials, such as Cloudflare
Access or Portkey headers, remain supported on API-key and custom-provider
routes; they are not accepted from inbound callers. Claude/Codex subscription
accounts retain the stricter private-header filter. The caller's Codex residency
requirement takes precedence over a
configured residency value. A configured value can fill an absent requirement.
Codex native WebSocket handshakes apply the same configured-header policy as
HTTP preparation.

## Native Claude and Codex behavior

All Claude OAuth Messages requests, native or emulated and even with
`claude-cloak: false`, replace `metadata.user_id` with the selected account UUID,
account device ID and generated conversation session. The
`x-claude-code-session-id` header matches that session. Other supported metadata
is retained. Token-count requests omit metadata. Native `anthropic-beta` values
are preserved; the mandatory `oauth-2025-04-20` beta is appended if missing.
Generated requests add their other required compatibility betas and a request
correlation ID when absent. Generated Claude profiles discard unrelated SDK
runtime/helper headers while keeping caller beta flags and explicit safe
extensions.

Codex recognizes supported CLI, Desktop and custom app-server attribution with
validated release syntax and consistent fields. Native attribution can remain
even when that client is not trusted to teach a shared identity. Subscription
requests bind `chatgpt-account-id` to the selected account and replace incoming
`session_id`/`session-id` with the generated `session-id`, while retaining
supported thread/window/turn context and native `client_metadata`.

HTTP and native WebSockets preserve
`x-openai-internal-codex-residency` and
`stream_options.reasoning_summary_delivery`. Malformed/conflicting residency
requirements are rejected. A route that cannot honor a requested Codex residency
constraint is rejected rather than silently stripping it or falling back to a
different provider/API-key route. Other unsupported subscription stream options
are removed. Forwarding a residency constraint is not an independent audit of
the provider's processing location.

Codex subscription image and compaction requests use the same header and
generated-session preparation. A residency requirement also excludes incompatible
API-key routes before an upstream request is made.

## Errors, transport and visibility

OAuth exchange/refresh and related operation errors use safe HTTP status/error
classifications. Raw endpoint descriptions, token response bodies and unsafe
transport URL/source details are excluded from Fusebox's OAuth errors, logs and
account status. Normal inference error bodies retain their existing client
behavior; this is not a blanket sanitizer for provider inference content.

Outbound HTTP clients do not follow redirects, including inference, OAuth and
control/reset requests, so a redirect cannot automatically replay a prompt or
credential to another endpoint. Each signed-in subscription has separate
inference, control and reset connection pools and its own TLS resumption store.
HTTP and native WebSockets can reuse that account's TLS state without sharing it
with another subscription. API-key accounts can still share transport pools.
This isolation does not require reconnecting on ordinary token refresh.

Supported native attribution still contains client-requested context: prompts,
tools, workspace/Git metadata, thread IDs or other accepted fields can identify
the originating application or task. Providers see the selected credentials and
account identifiers, outbound network/NAT or egress-proxy IP, TLS/HTTP behavior,
timing and request volume. Application identity compatibility does not provide
invisibility or guarantee any latency improvement. See the
[compatibility reviews](client-compatibility.md) for reviewed release baselines
and the limits of earlier checks.
