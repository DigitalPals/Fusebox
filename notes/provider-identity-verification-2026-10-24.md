# Claude/Codex identity hardening verification — 2026-10-24

Implemented in the working tree based on the earlier
[Claude/Codex audit](claude-codex-identity-audit-2026-10-24.md) and subsequent
remediation reviews. The earlier audit remains a historical record.
Configuration and behavior are described in
[provider identity and header policy](../docs/provider-identity.md).

## Verified behavior

- Provider header policies apply to all callers, independently of native-client
  recognition. Private credentials, forwarding attribution, `Via`, and
  `Connection`-nominated headers are filtered; explicit extensions remain bounded
  by that policy. Claude beta formatting survives preparation.
- Claude OAuth binds account, device and session metadata to the selected
  subscription on native and emulated requests. Codex HTTP and native WebSocket
  preparation bind the selected account and generated `session-id`.
- Subscription A → B → A creates three different provider sessions. HTTP and
  native WebSocket integration tests retain available conversation history and
  tool call/results across the switches. Idle expiry defaults to 30 minutes.
- Codex residency requirements and supported reasoning-summary stream options
  survive both transports. Invalid or unhonorable caller residency requirements
  are rejected. WebSockets apply the same safe configured headers as HTTP.
- Untrusted callers cannot teach shared client profiles. Saved profiles are
  validated, record provenance, and can be inspected/reset through management
  authentication. Recognition supports Desktop/custom application attribution.
- OAuth error markers do not reach client errors, account status or request
  diagnostics. Redirect tests verify that prompts, API keys and refresh bodies
  are not automatically replayed to a second origin.
- Routine refresh retains the selected Codex workspace and healthy WebSocket;
  actual account replacement invalidates identity state. Subscription inference,
  control/reset connection pools and TLS resumption storage are account-scoped.
  Replacement wins over an in-flight refresh or pending disk save; stale
  publications are discarded rather than overwriting the replacement.
- Follow-up fixes from Claude Fable 5.1 at Max effort cover foreign SDK headers
  on generated Claude requests, intentional gateway credentials on custom/API
  routes, and Codex image preparation. Configured invalid/protected headers fail
  validation rather than disappearing silently. Residency excludes incompatible
  routes without consuming an upstream attempt.
- The pinned official Codex
  [`build_session_headers`](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/codex-api/src/requests/headers.rs#L5)
  writes `session-id`; both its HTTP Responses endpoint and core WebSocket client
  use that builder. The generated spelling was checked against this source.

## Checks

On Linux/thebeast with Rust 1.96.1:

- `cargo +1.96.1 test --locked`: **712 passed, 0 failed, 1 ignored**.
- The strengthened native WebSocket residency test also passed separately after
  the full suite; it explicitly checks that a native upstream socket was used.
- `cargo +1.96.1 clippy --all-targets -- -D warnings`: passed.
- `cargo +1.96.1 build --locked`: passed.
- `cargo +1.96.1 fmt --check` and `git diff --check`: passed.
- `node --test tests/*.test.cjs`: **112 passed**.
- `python3 -m unittest discover -s tests -p 'test_*.py'`: **46 passed**.

The repository's disposable CI catalog has no Fusebox target. Tests and local
service fixtures ran under supervised `agent-resource` leases. No macOS/Windows
run or authenticated live-provider check is included in these results.

## Request overhead

[`scripts/identity-benchmark.py`](../scripts/identity-benchmark.py) compares the
preserved pre-change debug binary with the rebuilt debug binary using fake OAuth
credentials and a loopback provider. Each provider/binary has 300 measured
requests across three alternating rounds, with 20 warmups per round and reused
HTTP connections. Provider generation, external networking and TLS are excluded.

| Provider | Before median | After median | Median difference | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Claude | 1.300 ms | 1.312 ms | +0.012 ms | 1.750 ms | 1.754 ms |
| Codex | 1.370 ms | 1.425 ms | +0.055 ms | 1.858 ms | 1.891 ms |

This small loopback difference is not a guarantee about production latency.
Switching accounts or expiring a provider session can require a new native
WebSocket and history replay. Separate account pools can require initial
connection setup, then reuse connections within that account. Session rotation
does not itself replace prompt-cache keys; provider cache behavior is separate.

Raw output and binaries are retained under ignored
`target/provider-identity-implementation-20261024/`; the benchmark script is
included so the comparison can be repeated with available binaries.

## Development service and cleanup

No existing local Fusebox development service was found. The benchmark started
and restarted isolated rebuilt development instances, checked `/readyz`, served
both provider protocols and stopped them after each round. The supervised
leases used for the implementation checks and benchmark reported `closed`, with
no remaining resources, preserved volumes or cleanup errors. Temporary fixture
files were removed with their lease workspaces. Build artifacts and reports in
the repository's ignored `target/` remain intentionally.

Production was not deployed or restarted, and no real subscription credentials
or authenticated provider calls were used in verification. These checks establish
the tested request boundaries and compatibility behavior; they do not establish
that providers cannot distinguish Fusebox's network connections or usage.

## Subsequent production deployment

After the implementation verification above, the operator explicitly requested
a forced production deployment. The complete working-tree release was deployed
and passed production postflight checks. See the
[production deployment receipt](provider-identity-production-2026-10-24.md)
for the live version, checks, preserved data and rollback records.
