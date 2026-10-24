# Claude and Codex identity and disclosure audit

Date: 2026-10-24. Local source: `0002d85c7e1714ddd2e84a3d6611d3eeca5f19b0`.

## Verdict

The live configuration enables the intended subscription identity paths, and
basic credential replacement and client identity preservation work. However,
the implementation cannot receive a “correctly configured with no leaks”
sign-off. Local tests reproduced unintended Claude header/identity disclosure,
persistently corruptible Claude runtime emulation, Codex native-protocol
mismatches, and reflection of sensitive OAuth error text.

These findings establish disclosure paths and protocol differences. They do not
establish that production credentials have been exposed or that either provider
has classified this installation as a proxy.

## Live settings checked read-only

The installed release is `0.8.1+gui.faba37631a37`. Readiness reported five ready
accounts. The credential inventory contained four Claude OAuth accounts and one
Codex OAuth account, with no disabled accounts or per-account endpoint/proxy
overrides. No Claude/Codex API-key entries were configured.

- `claude-cloak` is enabled.
- Codex WebSockets are enabled.
- No configured `client-identities` overrides or global outbound proxy were found.
- Saved observed releases are Claude Code `2.1.309` and Codex `0.164.0`.
- The saved Claude runtime profile is SDK `0.128.0`, Node `v26.3.0`, Linux, x64.
  It contains no synthetic markers or implausible learned profile version.

SHA-256 comparisons matched the deployed release's retained source archive to
the checkout for `upstream.rs`, `clients.rs`, `ws.rs`, `tls.rs`, `oauth.rs`,
`quota.rs`, `discovery.rs`, and `state.rs`. The findings in those files therefore
apply to the deployed release, rather than only to a newer local experiment.
Only selected configuration/credential metadata was emitted by inspection;
real token/key values were not printed or copied into the audit artifacts.

## Confirmed findings

### 1. Shared OAuth failure handling can disclose credential text

**Reproduced with a synthetic token endpoint.** `read_json` incorporates up to
300 characters of an unsuccessful token response into its error
([oauth.rs](../src/oauth.rs#L179)). Refresh failure handling writes that error to
logs and account status ([oauth.rs](../src/oauth.rs#L378)); inference refresh
failure can return it to the requesting client
([proxy.rs](../src/proxy.rs#L1192)).

A fake token endpoint returning an error with `FAKE_REFRESH_TOKEN_AUDIT` caused
that marker to appear in all three places: application logs, `/api/accounts`
status and the downstream inference error. The fixture used a synthetic xAI
account because its token URL is configurable. Claude and Codex call the same
error-reading helper; no real identity-provider endpoint was contacted.

Actual credential exposure requires a provider response that contains credential
material. It is not established that one has occurred. Official Codex
[intentionally avoids logging refresh response bodies for this reason](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/auth/manager.rs#L1725).

Required correction: classify OAuth failures using safe status/error codes and
redact credentials; do not publish raw token response text to clients, status or
logs.

### 2. Native Claude passthrough forwards private gateway headers

**Reproduced on the outgoing HTTP request.** Native Claude copies all headers
accepted by a denylist ([upstream.rs](../src/upstream.rs#L190)). Beyond the
headers that are already blocked, the local mock received synthetic values in:

- `Via`
- `CF-Connecting-IP`
- `X-Forwarded-User`
- `X-Auth-Request-Email`
- `X-Gateway-Key`
- `X-Management-Key`

Those fields can carry a gateway hostname, source IP, user identity or an
additional credential. This is conditional on a caller/front proxy supplying
them. The test does not establish that the live frontend adds these fields.
Headers named by an incoming `Connection` header are not dynamically filtered
either ([filter](../src/upstream.rs#L53)).

Required correction: preserve documented Claude client headers through an
explicit policy, consume gateway credentials/attribution locally, and implement
hop-by-hop header filtering. Merely adding `Via` to the existing denylist would
leave other private extension headers exposed.

### 3. Claude can retain account A's metadata with account B's token

**Reproduced in both native and emulated HTTP paths.** A request with a synthetic
`metadata.user_id` containing account A, device A and session A reached the mock
with the selected account B's bearer token while retaining A's metadata.

Native preparation preserves the original body. The cloak accepts an existing
compound user ID solely because its parsed `device_id` has length 64; it does
not verify the account or session ([upstream.rs](../src/upstream.rs#L401)). In
the emulated path this can also disagree with the separately generated session
header ([upstream.rs](../src/upstream.rs#L237)).

Required correction: reconcile the provider account portion with the selected
credential and keep the session header/body consistent. Preserve only intended,
validated caller attribution. This requires an explicit policy for account
substitution; preserving every original identity field is not privacy isolation.

### 4. Claude runtime emulation can be poisoned across callers and restarts

**Reproduced, including restart persistence.** A recognized, authenticated
synthetic caller claiming `claude-cli/999.999.999` installed an `audit-marker`
runtime profile. Generated requests continued to claim release `2.1.309` but
used that marker. Subsequent normal `2.1.309` requests could not replace it, and
the marker remained after restarting the isolated service.

Release learning has a plausibility check, but runtime profile learning omits
it ([clients.rs](../src/clients.rs#L160),
[learn_profile](../src/clients.rs#L176)). The profile is global and persisted,
so one caller's runtime headers also affect other clients/accounts.

Required correction: validate version plausibility on observation and restore,
and scope learned runtime identity appropriately. The live saved profile
inspected during this audit was normal; the poisoning happened only in the
isolated fixture.

### 5. Codex silently drops a native residency requirement and stream option

**Reproduced over HTTP; the shared builders establish the corresponding
WebSocket behavior.** A native-shaped request carrying
`x-openai-internal-codex-residency: us` reached the mock without that header.
The same request lost
`stream_options: {"reasoning_summary_delivery":"sequential_cutoff"}`.

The header is absent from the allowlist
([upstream.rs](../src/upstream.rs#L556)); `stream_options` is unconditionally
stripped during OAuth sanitization ([upstream.rs](../src/upstream.rs#L448)).
Official `rust-v0.164.0` both
[enforces the residency header](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/model-provider-info/src/lib.rs#L428)
and [constructs the summary stream option](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/core/src/client.rs#L1068).

The residency issue matters when a caller actually requests managed residency.
This audit did not establish whether any live client enables it or where any
production inference was processed.

Required correction: preserve recognized provider constraints, or explicitly
reject a request when they cannot be honored; preserve supported native fields.

### 6. Codex native identity recognition has false negatives and false positives

**Reproduced over HTTP.** `originator: Codex Desktop` caused a native-shaped
request to use the canned Mac/iTerm TUI identity instead. Conversely,
`codex-fake/not-a-version` with `originator: codex-fake` was accepted unchanged,
even with no `version` header.

Recognition requires a lowercase `codex` prefix on the originator but does not
validate the extracted release version
([clients.rs](../src/clients.rs#L140)). Official Codex recognizes
[`Codex ` originators and custom app clients](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/auth/default_client.rs#L141).

Required correction: recognize supported official/app-server client identities,
validate version syntax and field consistency, and do not let arbitrary claimed
versions silently change a shared persisted identity.

## Further source findings

These were reviewed in source; the following specific scenarios were not
reproduced in the runtime fixture.

- **Codex workspace selection during refresh:** refreshed ID-token claims can
  overwrite the stored selected account ID
  ([oauth.rs](../src/oauth.rs#L423)). Official Codex
  [preserves the stored selection while updating tokens](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/auth/manager.rs#L1673).
  The existing WebSocket reuse check uses an identity epoch, which
  `publish_refreshed` does not change when it replaces credentials
  ([oauth.rs](../src/oauth.rs#L508), [ws.rs](../src/ws.rs#L641)). A changed
  workspace can therefore create inconsistent identities across transports.
- **Codex configured headers differ across transports:** HTTP applies
  per-account custom headers, but native WebSocket preparation does not
  ([upstream.rs](../src/upstream.rs#L144),
  [codex_ws_url](../src/upstream.rs#L631)). No Claude/Codex API-key custom
  headers were configured on the live service.
- **Claude generated sessions are account-wide:** all generated requests for
  one `Account` reuse its session UUID
  ([accounts.rs](../src/accounts.rs#L1268),
  [upstream.rs](../src/upstream.rs#L406)). This merges unrelated tasks in
  provider-visible attribution. A same-filename credential replacement can also
  retain the prior object's device/session IDs because `same_shape` reuses the
  allocation ([accounts.rs](../src/accounts.rs#L1203)).
- **Redirect policy permits body forwarding:** ordinary inference retains
  reqwest's default redirect behavior
  ([state.rs](../src/state.rs#L282)). A 307/308 can preserve the prompt on a
  redirect to another origin. reqwest strips cross-origin `Authorization`, but
  not Anthropic's `x-api-key`. This is a conditional risk; the live installation
  uses OAuth without custom provider endpoints, and no production redirect was
  observed by this audit.
- **Separate inference pools do not isolate all connections:** Claude/Codex
  background control calls use shared pools
  ([state.rs](../src/state.rs#L221), [quota.rs](../src/quota.rs#L408),
  [discovery.rs](../src/discovery.rs#L300)). TLS session resumption state is
  also shared within config classes ([tls.rs](../src/tls.rs#L21)). These permit
  correlation between accounts; they are not evidence of provider detection.
- **Generated Claude correlation:** generated inference headers omit the
  documented `x-client-request-id`. This is a parity/correlation gap, not a
  demonstrated disclosure.

## Checks that passed

- Both selected provider bearer tokens replaced the synthetic client key.
- Both routes blocked cookies, proxy authorization, ordinary forwarded-address
  headers, Tailscale identity headers, and all tested `x-fusebox-*` and
  `x-cliproxy-*` headers.
- Codex blocked every tested private extension header and replaced a supplied
  account A header with selected account B.
- Native TUI user agents were preserved in both providers' ordinary paths.
- Claude emulation used the learned Linux/x64 runtime profile in the normal
  fixture, and generated metadata for the selected account when no caller
  metadata was supplied.
- Codex's bundled TUI UA/version/originator pattern, WebSocket beta and JSON
  ordering agree with the pinned official client. Its canned Mac/iTerm values
  describe an emulated installation, not the Linux server.
- Native Codex `client_metadata` survived the tested request. Preserving native
  metadata can intentionally include OS/terminal, Git/workspace or tool
  attribution. It does not provide metadata anonymity.
- Claude/Codex usage/model/reset paths construct provider-specific headers rather
  than forwarding the caller's full headers. Required authentication, account IDs
  and request content remain provider-visible by design.

## Verification and scope

Passed 63 existing regression tests: 20 upstream preparation tests, 6 identity
learning tests, and 37 routing/HTTP/WebSocket tests, using
`cargo +1.96.1 test --locked` with the relevant module filters.

The rebuilt development binary was started, stopped and restarted against
isolated loopback providers. The final probe run completed 13 synthetic
inference requests plus a synthetic OAuth-failure scenario. All provider tokens,
keys, accounts, user identities and prompts in those probes were fake. Existing
disposable CI has no Fusebox project target, so these checks ran locally. No
authenticated real-provider requests were issued by the probes.

Local ignored artifacts are under `target/provider-identity-audit-20261024/`:
`runtime-probe.py`, `runtime-proof.json`, `live-settings.json`,
`live-identities.json` and `source-match.json`. All four supervised resource
leases used for evolving the probes are closed; cleanup checks showed no
remaining resources or errors. No existing development service was running on
the host; the test service was rebuilt/restarted and then stopped. Production
software and settings were unchanged.

Independent comparisons used official Claude gateway/SDK material and public
`openai/codex` `rust-v0.164.0` source. Claude's current published wrapper was
`2.1.310`, one release ahead of the installed/bundled identity; inspecting that
wrapper does not certify its native inference implementation. The existing
`2.1.309` executable review remains the basis for the compound user-ID/billing
details. Exact parity with future releases or undisclosed provider classifiers
cannot be inferred from these checks.

Useful official references:

- [Claude Code gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol#request-headers)
- [Claude monitoring correlation](https://code.claude.com/docs/en/monitoring-usage#event-correlation-attributes)
- [OpenAI: Sign in with ChatGPT through a gateway](https://learn.chatgpt.com/docs/enterprise/sign-in-with-chatgpt-through-a-gateway)
- [Official Codex user-agent construction](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/auth/default_client.rs#L152)
- [Official Codex WebSocket header handling](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/codex-api/src/endpoint/responses_websocket/connector.rs#L32)

This audit changes documentation only. The confirmed findings require code
changes before the implementation can be signed off against the requested
privacy and identity-preservation goals.
