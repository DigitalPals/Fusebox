# Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
node --test tests/*.test.cjs
python3 -m unittest discover -s tests -p 'test_*.py'
cargo run -- --config dev.yaml
```

The dashboard lives in `ui/`. Its text assets are compressed and embedded at build time, so rebuild after editing them.

After a release build, `python3 scripts/dev-smoke.py` checks native and translated requests against a loopback provider, compressed input, and the current embedded dashboard assets. `python3 scripts/usage-e2e.py --binary target/release/fusebox` checks collectors, durable replay and restart. Both use synthetic data; on `thebeast`, run them inside `agent-resource` as shown in the [optimization report](optimization-report.md), which also documents reproducible component benchmarks.

## How it works

```text
client request ──parse──▶ shared request model ──build──▶ provider request ──▶ upstream
client stream  ◀─render── shared event stream  ◀─parse─── provider stream  ◀──┘
```

Each wire format (`src/formats/{chat,responses,claude,gemini}.rs`) knows how to parse requests, build requests, decode streams and render streams. Adding a format means writing four functions instead of a translator for every pair.

| File | What it does |
| --- | --- |
| `src/proxy.rs` | Picks an account, translates, retries on the next account, streams back, records usage |
| `src/ws.rs` | Responses over WebSocket: native Codex relay, or local history for other providers |
| `src/upstream.rs` | Per-provider URLs and headers, including the request shape Claude OAuth accounts expect |
| `src/accounts.rs` | Credential files, API keys, routing, priority and cooldowns |
| `src/policy.rs` | Named clients' limits: models, programs, rate, tokens, spend, expiry |
| `src/embeddings.rs` | `/v1/embeddings` on OpenAI, compatible, Gemini and Vertex keys |
| `src/metrics.rs` | `/readyz` and Prometheus `/metrics` |
| `src/push/` | Faults, push notifications and webhooks |
| `src/oauth.rs` · `src/device.rs` | Browser and device-code sign-in, token refresh for every provider |
| `src/antigravity.rs` · `src/schema.rs` | Cloud Code envelope, project onboarding and the JSON Schema down-leveller it needs |
| `src/devin.rs` | Devin's Connect protobuf: request encoding, stream decoding, model ids |
| `src/vertex.rs` | Service account JWT signing |
| `src/media.rs` | Image, video and compaction endpoints |
| `src/compat.rs` | CLIProxyAPI's config layouts, in-place config edits and command-line flags |
| `ui/` | The dashboard: plain HTML, CSS and JS, compiled into the binary |

## Client compatibility

Requests Fusebox builds for other clients look like one release of Claude Code, the Codex CLI, Grok Build, Muse Code, the Devin CLI and Antigravity. `.github/client-versions.json` records each bundled identity in `version`, and `cargo test` checks the matching Rust constants. Its separate `reviewed` field records the latest upstream release checked for compatibility. A compatible release can advance `reviewed` without changing the bundled identity or shipping a Fusebox release.

The **Client compatibility** workflow checks release channels daily at 06:17 UTC and writes the results to the Actions summary. On Mondays it also maintains one review issue (label `client-compatibility`) covering all releases newer than `reviewed`. Later releases edit the issue without comments or version-specific titles. When everything is reviewed, it closes the issue. A manual run can reconcile the issue immediately; uncheck **update_issues** for a report only. Any failed release check fails visibly and leaves issues untouched. The first reconciliation retires the old automated `client-version` issues; manually filed issues remain open.

Review every intervening release's authentication, headers, request bodies and WebSocket messages, fix differences when needed, and record evidence in [Client compatibility reviews](client-compatibility.md). Then advance `reviewed`. Update `version` and its Rust constants together only when changing the bundled identity. New model availability is handled by discovery and **Add model**, separately from client compatibility.

Recognized Claude Code and Codex clients retain supported native application attribution. Fusebox binds subscription authentication and provider-session identity to the selected account and filters incoming headers. Generated requests use the newest bundled, configured or validly learned release. Learning requires an authenticated named client with `trust-client-identity: true` (off by default); shared keys and open access cannot update identities. Claude runtime profiles are validated, carry source provenance and apply only to their matching release, with bundled runtime defaults used otherwise. Saved observations are revalidated on restore; legacy records without trusted provenance are ignored.

Inspect effective versions, runtime values and their sources under **Config → Connections → Client identities**, or management-authenticated `GET /api/client-identities`. **Reset learned profiles**, or `POST /api/client-identities/reset`, clears saved observations while keeping `client-identities` configuration. Stopping learning does not clear earlier observations. A version-only release may need no Fusebox release; a changed wire protocol still needs compatibility review.

```yaml
client-identities:     # explicit operator override for generated identities
  claude-code: 2.1.319
  codex: 0.167.0       # also grok-build, devin-cli, antigravity
```

An entry older than the built-in release is ignored, so a forgotten one can't hold Fusebox back. `fusebox check` prints the release it claims for each client. Muse Code's identity includes a build hash, so it can't be set this way.

See [provider identity and header policy](provider-identity.md) for account/session rebinding, extra inbound header configuration, outbound-header precedence and HTTP/WebSocket behavior. Supported native attribution still includes client-requested context, and providers see the outbound network/proxy IP and transport behavior. Compatibility does not promise invisibility or a particular latency improvement. The [dated audit](../notes/claude-codex-identity-audit-2026-10-24.md) records the earlier findings; it is not a validation sign-off for these changes.

The same workflow says when a newer stable Rust is out than the one in `rust-toolchain.toml`: one issue (label `rust-version`), opened on a Monday and closed once `main` builds with it (`python3 .github/scripts/rust_release.py` for a local report).

For a local report, run `python3 .github/scripts/client_versions.py --dry-run`. Issue changes require `--update-issues` plus `GITHUB_TOKEN` and `GITHUB_REPOSITORY`; `--dry-run` always suppresses writes. The monitor's regression tests run with the normal Python checks in CI.

See [release binary size](binary-size.md) for compiler comparisons, measured
savings, and Linux packed-relocation compatibility.

## Releases

Pushing a version tag publishes a release. Bump `version` in `Cargo.toml` and `Cargo.lock`, merge that to main, then tag it:

```sh
git tag -a v0.4.1 -m v0.4.1 && git push origin v0.4.1
```

The **Release** workflow checks that the tag matches `Cargo.toml`, builds the five archives and publishes them with `SHA256SUMS`, build attestations and a list of the pull requests since the last release, below `.github/release-notes/v0.4.1.md` if that file exists. A tag such as `v0.4.1-rc.1` publishes a prerelease. If the workflow itself fails, fix it on main and release the existing tag with `gh workflow run release.yml -f tag=v0.4.1`, rather than moving the tag. The **Docker** workflow tags the image `0.4.1`, `0.4` and `latest`; a prerelease gets only its own tag.

Patch releases bring client-version catch-ups and fixes; minor releases bring features, new settings and changed behaviour.
