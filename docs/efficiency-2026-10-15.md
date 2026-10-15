# Efficiency implementation, 15 October 2026

This implements the reconciled audit of `bb86eca`, independently checked with
Claude Fable 5.1 at Max effort. The implementation keeps opt-level 3, panic
unwinding, explicit-session durable persistence and existing cache defaults.
It does not rewrite or compact prompts.

## Changes

- The incremental SSE decoder retains a scan cursor and accumulates data directly
  in one string. Fragmented lines are scanned once; empty data events, CRLF,
  Unicode boundaries and trailing events retain their behavior.
- Bundled SQLite omits unused FTS3/4, FTS5 and RTree modules through
  `.cargo/config.toml`. JSON, WAL, backups, progress hooks and threading remain.
- Dashboard text is compressed deterministically at build time and embedded only
  as gzip. HTTP negotiation includes identity fallback, representation-specific
  SHA-256 ETags, conditional 304 responses and `Vary: Accept-Encoding`. Identity
  representations are decoded lazily once. PWA URLs and no-cache revalidation
  remain intact; fonts and PNGs remain in their existing representation.
- Cache policy diagnostics show requested/applied controls, effective key scope
  and translation downgrade reasons, without prompt content or raw cache keys.
  The dashboard and structured completion logs expose these fields. Optional
  `drop`, `automatic` and `reject` policies preserve compatible controls and make
  incompatible translations visible. Unsupported prewarming remains an error;
  explicit mode without boundaries remains uncached when translated to Claude.
- Optional client/workspace cache scopes and a version namespace work across
  HTTP and native Codex WebSockets. Session scope stays the default. Caller keys,
  client isolation, custom-endpoint behavior and account affinity are preserved.
- Responses capture reuses a parsed JSON value and moves terminal response data.
  Non-streaming passthrough also reuses its parsed value. Request translation
  borrows the parsed IR, caching a shortened-tools variant only when necessary.
  Model groups move the final attempt's body. Stream renderers extract their
  request options before streaming rather than retaining the whole request.
- Continuation lookup copies immutable history outside the routing lock. A
  bounded counting serializer checks the exact JSON size before cloning a new
  entry. The existing 8 MiB entry/64 MiB total limits and tenant checks remain.
- Optional HTTP admission limits count concurrent requests and decoded body bytes
  through response completion, releasing on failure or cancellation. They fail
  fast with 503/Retry-After; route limits return 413. Defaults preserve the 256 MiB
  per-request limit and unlimited aggregate admission, without default admission
  accounting overhead. The byte budget is not a total RSS ceiling. WebSockets
  and management traffic are separate.
- Docker builds cache compiled runtime/build dependencies before source, UI and
  commit stamping. Release CI caches Rust outputs by target. Both include the
  SQLite settings. Config validation, dashboard controls and Chinese translations
  cover the new options.

See the [configuration reference](../README.md) and
[cache measurement guidance](context-efficiency.md). The local benchmark below
measures decoding only; provider cache-hit improvements require an authorized
workload comparison and have not been claimed.

## Reproduce decoder measurements

```sh
rustc -O --edition=2024 scripts/sse-benchmark.rs -o /tmp/fusebox-sse-benchmark
/tmp/fusebox-sse-benchmark
```

The harness reports median milliseconds over nine runs for large fragmented
events and 2,000 small events. It asserts event counts and uses the production
decoder directly. It makes no provider calls.

## Measured results

Matching native builds use rustc 1.96.1, fat LTO, opt-level 3, one codegen unit,
stripping, RELR and ICF on Linux x86-64. The baseline is the unchanged `bb86eca`
archive; the implementation includes all new controls and diagnostics.

| Executable | Bytes |
| --- | ---: |
| Baseline | 16,606,928 |
| Full implementation | 15,783,216 |
| Saved | 823,712 (4.96%) |

The final combined saving includes added feature code, rather than adding
individual opportunity estimates together. The SQLite-only audit candidate
saved 507,264 bytes. Identity dashboard requests allocate a lazy decoded copy;
smaller installed size does not imply the same reduction in resident memory.

Nine-run decoder medians, baseline followed by the final implementation:

| Input | Chunk bytes | Before ms | After ms |
| --- | ---: | ---: | ---: |
| 1 MiB line | 1,024 | 122.930 | 0.606 |
| 1 MiB line | 16,384 | 13.386 | 0.542 |
| 4 MiB line | 1,024 | 1,942.147 | 1.950 |
| 4 MiB line | 16,384 | 123.169 | 1.875 |
| 2,000 small events, total | 16,384 | 0.253 | 0.257 |

The large-line improvement removes quadratic work. Small-event times overlap
noise; these figures do not establish provider latency or production throughput.
Actual network chunk sizes were not measured.

The existing loopback usage benchmark ran baseline/implementation/implementation/
baseline, with 10,000 imports and 200 HTTP requests per sample. Medians of the
two run medians were:

| Workload | Baseline | Implementation |
| --- | ---: | ---: |
| Proxy, analytics off | 0.728 ms | 0.743 ms |
| Proxy, analytics on | 0.735 ms | 0.735 ms |
| Import 10,000 records | 0.755 s | 0.759 s |
| Dashboard query | 9.842 ms | 9.327 ms |

Timing ranges overlap; there is no demonstrated general proxy speedup or
material regression in this limited synthetic workload. No run dropped or
rejected usage observations or reported a writer error. Cache-hit rates and
concurrent production RSS remain workload measurements, not claimed savings.

Docker's first dependency compile took 27.63 seconds with a warm registry.
Rebuilding changed source/UI reused that entire layer; application compilation
still took 94.78 seconds (initially 93.27 seconds). This verifies dependency
reuse, not a universal build-time speedup. Lockfile/toolchain changes invalidate
the dependency layer by design. The Debian 12 image's executable is 15,539,544
bytes; its different compiler makes it a separate measurement. ELF checks retain
RELR and require glibc 2.36. No image was published.

Exact hashes, source fingerprint, samples and smoke-test data are in
[efficiency-results-2026-10-15.json](efficiency-results-2026-10-15.json).

## Validation and service handoff

- 461 Rust tests, 50 dashboard tests, 30 Python tests and 12 real-process
  usage/collector acceptance checks passed. Formatting and Clippy for all package
  targets with warnings denied passed. One concurrent suite run encountered two
  transient reset-journal file-lock failures; the focused 22-test suite and the
  complete 461-test rerun passed without changing reset code.
- Regression coverage includes fragmented SSE behavior, cache scope/client
  isolation, HTTP/native WebSocket parity, caller keys, cache fallback/rejection,
  explicit-without-boundaries behavior, configuration validation, compression
  negotiation/validators, history bounds and admission release on cancellation.
- The final Docker image passed version, health, compressed dashboard and model
  list checks. The native release passed usage import, durable collector replay,
  deduplication, pricing and privacy acceptance checks. Other OS/architectures
  remain covered by CI; they were not executed locally.
- `fusebox-reset-countdown-dev.service` was rebuilt and restarted on `thebeast`.
  Health, identity/gzip equality, 304 and HEAD checks passed on port 18317. The
  live configuration API exposes the intended defaults. Desktop and mobile
  browser checks showed the new controls, no horizontal overflow and no console
  errors. The subsequent authorized production rollout is recorded in
  [production verification](efficiency-production-2026-10-15.md).
