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

The 2.1.307 change omits `prompt-caching-scope` behind custom gateways. Fusebox
already forwards native Claude Code headers and adds its cloak betas only when
building its own OAuth requests; no additional change was required. The
changelog's `/claude-api api-version` item changes a bundled skill's advice,
not the CLI's Messages API version. Real clients continue to retain their
headers and to teach Fusebox newer identities and runtime profiles.

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
