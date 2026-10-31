# Repository optimization, 31 October 2026

Baseline: `7fef91efbf0134931ac0ab2935e64369456bfb3b` (Fusebox 0.12.1), with a
clean working tree. Three agents first investigated storage/runtime, shared logic,
and frontend/workflows without editing. The coordinating engineer reviewed their
findings, assigned separate file ownership, captured baselines before changing
the measured routines, and reviewed the combined implementation.

## Architecture and investigation coverage

Fusebox is one Rust binary. Axum authenticates HTTP requests and bounds/decompresses
bodies before routing; HTTP and WebSocket paths share account selection, session
affinity, pooled HTTP/TLS clients, a protocol-independent request/event model,
and usage tracking. Four format adapters translate Chat, Responses, Claude and
Gemini. Separate media/embedding paths preserve their different retry and billing
semantics. Quota/model polling, OAuth refresh, notifications, window starters and
banked resets run in background tasks with their existing coordination.

Usage producers enqueue metadata to a bounded SQLite writer. Independent,
cancellable read transactions use indexes, rollups and compressed documents;
imports and collectors preserve checkpoints and durable outboxes. The embedded
dashboard uses plain JavaScript, WebSocket events, targeted DOM patches and
versioned, precompressed assets. Cargo builds the service and assets; Node and
Python standard-library test runners cover dashboard and operational scripts.
GitHub Actions checks Linux/macOS/Windows and separately builds releases/images.

Coverage included:

- HTTP/WebSocket routing, adapters, SSE, retries, upstream preparation, media,
  embeddings, history, identity, policy, source addresses, admission and metrics.
- SQLite reads/writes, schema/indexes, rollups, pricing, document compression,
  import resume, keeper and collector flows, backups/maintenance interfaces.
- Dashboard request/model/account/usage/config rendering, timers, live refresh,
  date/i18n caches, service worker, asset delivery and accessibility contracts.
- Quota/model/OAuth polling, reset/window-starter journals, notifications,
  shutdown, TLS/client reuse, Cargo/SQLite features, Docker and CI/release tooling.

The earlier [optimization](efficiency-2026-10-29.md) and
[follow-up](optimization-followup-2026-10-29.md) were checked against the current
source. Their existing improvements were retained. This was source and synthetic
fixture coverage, not a line-by-line security audit. No live providers, personal
histories, production database, cross-platform runner or real browser was used.
Browser layout/paint, responsive screenshots, real provider latency, process peak
RSS and production throughput remain measurement gaps.

## Findings and prioritization

| Finding and location | Evidence and expected benefit | Decision, risk and validation |
| --- | --- | --- |
| Collector labels, `src/usage/store.rs`, `store/dashboard.rs` | Every collector row repeated schema and label queries; a 500-row repeated-origin page executed 1,000 queries. | Implement a read-local cache shared by details/facets. Preserve missing/revoked labels and snapshot lifetime; compare reference output, SQL counts, renames, enrollment, errors and API fields. |
| Dashboard dates, `ui/app.js`, `ui/usage.js` | 300 request rows made 600 native locale-formatting calls. | Reuse bounded formatters. Verify complete row markup, four locales/timezones, DST, invalid timestamps, tooltips and timezone changes. |
| Live refresh, `ui/app.js` | Three debounced account-event bursts with delayed responses launched nine simultaneous reads. | Serialize batches and coalesce queued flags, with a 30-second abort deadline. Verify failures, timeouts, late completions, navigation, locking and both event types. |
| JSON sizing, `latency.rs`, `affinity.rs`, `usage/collector.rs`, `embeddings.rs` | Three independent writer counters; embedding cache admission serialized a complete string only to count bytes. | Share one bounded serializer counter. Verify exact escaped UTF-8 bytes, inclusive limits, real serializer errors, early termination and existing history/batch/cache contracts. |
| Embedding conversion, `src/embeddings.rs` | Default float output allocated, filled and discarded float32 byte buffers. | Build binary buffers only for base64. Preserve float64 JSON values, float32 validation, negative zero/subnormals, ordering, errors and base64 bytes; benchmark both encodings. |
| OpenAI usage rules, `src/formats/{chat,responses}.rs` | Duplicate inclusive-input/cache-write alias and lifetime rules in parsers/renderers. | Consolidate in `formats/mod.rs`. Verify full and streamed output, alias precedence, zero/absent/malformed values and saturation. Structural benefit; no runtime speed claim. |

## Completed changes and compatibility

- `CollectorLabels` checks optional schema once and caches each distinct label or
  missing collector within one read. It does not load the entire collector table
  or retain labels between requests. Existing 500-row/facet limits bound its
  memory. Revoked labels remain visible and SQL errors still propagate. Existing
  cancellation checks remain in place.
- `json_size::serialized_size` serves continuation-history limits, latency
  buckets, collector batch sizing and embedding-cache admission. It counts the
  serializer's actual bytes and stops at the caller's inclusive limit without
  retaining encoded JSON. Cache settings, invalidation, ownership, accounting and
  retry semantics are unchanged.
- Float embedding responses validate every value as before, retain the original
  JSON numbers, and skip the binary scratch buffer. Base64 still encodes
  little-endian float32 values. In the 128-by-3,072 fixture this removes 1,572,864
  bytes of cumulative requested binary scratch capacity from float conversion.
  Cache sizing also avoids retaining an 8,142,138-byte serialized copy of that
  fixture. These are buffer sizes from the fixture/code, not measured peak RSS or
  allocator totals.
- `UsageTools` now owns app clock, hour/minute, weekday, month/day and reset-time
  formatting as well as existing usage-page styles. It retains only the latest
  timezone per style. Render/timer/resume boundaries detect the current local
  timezone; a change invalidates minute-label strings. The normal render performs
  one timezone lookup, with no formatter construction per row.
- `runRefresh` waits for all reads in its batch, including siblings of a failed
  read, before starting one coalesced trailing batch. Signals abort stalled reads
  after 30 seconds; aborted/locked/failed snapshots are not applied. This guards
  live account/model refreshes. Startup, explicit route loads and overview polling
  retain their existing independent behavior.
- `openai_usage` and `write_openai_cache_details` give Chat and Responses one owner
  for normalized usage rules. Wire field names, canonical/legacy alias precedence,
  zero defaults, lifetime presence and serialized field order are preserved.
  Durable billing capture remains separate because unknown/partial counts have
  intentionally different semantics.
- The development guide now lists the full Rust/Node/Python check set.
  `scripts/dev-smoke.py` reuses the existing runtime benchmark's mock/provider
  fixture to check a rebuilt binary and its embedded assets. The existing
  `scripts/usage-e2e.py` covers persistent collector restart/replay. Neither uses
  personal credentials or histories.

No dependencies, public endpoints, database formats or configuration schema were
added or upgraded. Test-only reference implementations remain for paired
measurements and compatibility checks; superseded production counters/parsers
were removed.

## Benchmark method and reproduction

The exact before/after implementations run in the same process with identical
fixtures and compiler settings. The original baseline was also measured before
replacing production routines. Rust references retain the baseline algorithms;
the JavaScript harness reads baseline source with `git show`. The baseline commit
must remain available locally when running the dashboard comparison.

Environment: Debian 13, Linux 6.12.113 x86-64, AMD Ryzen Threadripper 9970X
(32 cores/64 logical CPUs), Rust stable 1.96.1/LLVM 22.1.2, Node 22.22.3 and
Python 3.13.5. The host is shared; a CI admission reservation holds four CPUs and
eight GiB of capacity, but this is not CPU affinity or exclusive hardware.

- Collector labels: 500 rows with 1, 10 or 500 distinct origins; three paired
  warmups and nine alternating samples, 20 pages/sample. SQL authorizer counts
  are collected separately from timing. The SELECTs are uncached, so each
  matching authorization corresponds to one executed schema/label query.
- Embeddings: 128 vectors of 3,072 realistic fractional numbers; two paired
  warmups, nine alternating samples, four operations/sample. Conversion excludes
  input cloning, includes output destruction, and asserts exact reference parity.
  Cache sizing tests both exact-fit admission and rejection at 4,096 bytes.
- Request rows: six/300 rows, desktop/mobile markup, three warmups and nine
  alternating samples; includes the timezone boundary lookup. Unchanged non-date
  helpers have stand-ins; complete resulting markup must match. Timing excludes
  DOM parsing, layout and paint. Operation counts are outside timed sections.
- Refresh: delayed promises deterministically exercise three midflight account
  bursts on Models. Request counts are a concurrency result, not a latency test.

```sh
# Use stable explicitly: this host's default Rust 1.85 predates the 1.88 minimum.
CARGO_BUILD_JOBS=4 rustup run stable cargo test --release --locked usage_collector_labels_benchmark -- --ignored --nocapture --test-threads=1
CARGO_BUILD_JOBS=4 rustup run stable cargo test --release --locked embedding_conversion_and_sizing_benchmark -- --ignored --nocapture --test-threads=1
TZ=Europe/Amsterdam node scripts/ui-benchmark.cjs --baseline-ref 7fef91e --output /tmp/fusebox-ui-results.json

CARGO_BUILD_JOBS=4 rustup run stable cargo fmt --check
CARGO_BUILD_JOBS=4 rustup run stable cargo clippy --locked --all-targets -- -D warnings
CARGO_BUILD_JOBS=4 rustup run stable cargo test --locked -- --test-threads=4
node --test tests/*.test.cjs
python3 -m unittest discover -s tests -p 'test_*.py'
CARGO_BUILD_JOBS=4 rustup run stable cargo build --release --locked
```

On `thebeast`, the supported `ci-platform run` catalog has no Fusebox job. Use
`ci-platform reserve --purpose 'Fusebox validation' --memory-gib 8 --cpus 4` and
keep its stdin open while running host checks; close stdin to release the
reservation. No CI worker should be created manually.

Run process integrations under the host resource controller:

```sh
agent-resource run --task 'Fusebox rebuilt development checks' --ttl 1h -- bash -c '
  export TMPDIR="$AGENT_RESOURCE_WORKSPACE"
  export TOKIO_WORKER_THREADS=4
  python3 scripts/dev-smoke.py --binary target/release/fusebox &&
  python3 scripts/usage-e2e.py --binary target/release/fusebox
'
```

The managed task owns child processes and its workspace through failures and
timeouts. Check the returned lease with `agent-resource check LEASE_ID` after it
exits. On machines without this host controller, the scripts can run directly.

Debug-profile measurements are retained as a cross-check. Optimized Rust and Node
results describe separate local components, not provider inference, subscription
savings or full API latency, and should not be added together.

## Measured results

Medians of nine alternating samples, with the warmups and workloads described
above. Rust uses the repository's unchanged optimized release profile. The final
Node run was made after compilation finished; the raw samples retain host noise.

| Component workload | Before | After | Less time |
| --- | ---: | ---: | ---: |
| Label lookup, 500 rows / 1 collector | 1.349 ms | 0.0233 ms | 98.3% |
| Label lookup, 500 rows / 10 collectors | 1.407 ms | 0.0350 ms | 97.5% |
| Label lookup, 500 rows / 500 collectors | 1.488 ms | 0.706 ms | 52.6% |
| Cache sizing, 8,142,138-byte response fits | 8.968 ms | 7.197 ms | 19.7% |
| Cache sizing, same response / 4,096-byte limit | 8.988 ms | 0.00406 ms | 99.95% |
| Request markup, 300 desktop rows | 13.705 ms | 1.010 ms | 92.6% |
| Request markup, 300 mobile rows | 12.259 ms | 1.020 ms | 91.7% |
| Request markup, six desktop rows | 0.283 ms | 0.045 ms | 84.1% |
| Request markup, six mobile rows | 0.271 ms | 0.046 ms | 83.0% |

Float conversion measured **8.062 → 8.044 ms** and base64 conversion
**1.935 → 1.960 ms**. The sample ranges overlap: float 7.959–8.277 versus
7.972–8.206 ms; base64 1.886–2.046 versus 1.914–2.080 ms. These small differences
are inconclusive, so no conversion CPU speedup is claimed. The float change is
retained for eliminating the discarded binary buffers, with exact output/error
parity and little additional code.

Deterministic reductions, measured independently from timing:

- Label SQL queries/page: **1,000 → 2**, **1,000 → 11**, and **1,000 → 501**
  for 1, 10 and 500 distinct collectors respectively. These count only label and
  optional-schema queries, not all SQL performed by a details request.
- A warm 300-row render makes **600 → 0 native locale-method calls**, with
  **one** timezone formatter construction for the entire render. Ordinary row
  formatting constructs no new formatter after warmup.
- Three midflight account-refresh bursts make **9 → 6 total reads**, with
  **9 → 3 maximum concurrent reads**. A combined account/model batch can have
  four concurrent endpoints; the result is one active batch, not a global
  three-request limit.

The prechange debug harness confirmed both sides still executed the original
work before replacement. Debug and release timings are not compared with each
other. Earlier paired Node verification measured 15.340 → 1.320 ms on desktop
and 13.720 → 1.382 ms on mobile, illustrating absolute host variability while
supporting the same reduction. The final run above includes all render-boundary
overhead. Existing chart-hover/model-coverage controls remain in the harness;
their noisy differences are not claimed as improvements in this pass.

Raw evidence and source fingerprints:

- [Rust baseline, debug and optimized paired samples](optimization-results-2026-10-31.json).
- [Dashboard samples, output parity and operation counts](optimization-ui-results-2026-10-31.json).
- [Validation, rebuilt binary identity and cleanup receipts](optimization-validation-2026-10-31.json).

The shared JSON counter and OpenAI usage helpers also reduce duplicate production
implementations. Those ownership improvements are structural evidence, separate
from the measured CPU results.

## Validation and development restart

The baseline passed **785 Rust tests** (five ignored measurements), **149 Node
tests** and **46 Python tests**. Formatting of an immutable baseline source export
also passed. No pre-existing failures were found in those baseline checks.

Final checks passed:

- **796 Rust tests**, zero failures; seven benchmark tests intentionally ignored
  in the normal suite. Both new benchmarks also passed explicitly in debug and
  optimized profiles.
- **159 Node tests** and **46 Python tests**, zero failures.
- Five additional asset tests after the final UI edit; Rust formatting,
  all-target Clippy with warnings denied, JavaScript syntax, new Python script
  compilation, release build and `git diff --check`.
- Eight rebuilt-service smoke checks: liveness/readiness, native and translated
  request/token output, gzip input, current identity/gzip/304 assets, script load
  order, metrics and graceful shutdown.
- All 12 existing real-process collector acceptance checks, including separate
  collectors, replay, authentication, offline outbox restart/reconnect, credential
  rotation, historical pricing, repricing and content privacy.

No existing local Fusebox development unit, container or process was found. The
rebuilt release was exercised in an isolated development service; collector
acceptance gracefully stopped and restarted it on the same configuration, port
and usage database. Synthetic data and loopback providers were used. No persistent
service, deployment or publication was created.

Both managed leases (`fe509f52c5614c65b04c3d64f5d9703f` and
`85d179a217d9447e9100d5d944174831`) are closed: checks report no remaining
resources, preserved volumes or cleanup errors. The CI capacity reservation was
released. No temporary service was retained. The validation receipt records the
binary SHA-256 and changed source hashes because an uncommitted build's version
string still names the baseline Git commit.

## Significant findings deferred

- **Pricing catalogue deserialization per insertion batch** (`store.rs`): a cache
  needs invalidation for custom catalogues and metadata updates through public
  writer transactions. Reusing stale prices would be worse than this CPU cost.
- **Per-row pricing-revision reads** (`store.rs`): the appropriate observation/id
  index exists. Batching needs pagination, ordering and corruption/error parity
  work; measure representative revision-heavy pages before broadening the query.
- **Account lookup indexing** (`ui/app.js`): repeated linear scans are plausible
  costs at many accounts, but no representative user workload justified another
  invalidation cache in this pass.
- **CIDR parsing and larger decompression chunks** (`source.rs`,
  `server/request_body.rs`): configuration-generation/security semantics and
  concurrent body-admission memory need dedicated measurements first.
- **Immutable catalogue provenance reuse** (`usage/pricing.rs`): safe reuse needs
  to preserve initialization-error details. Status reads are lower priority than
  the measured per-row work; left out to keep the pricing change boundary clear.
- **Independent dashboard fetch paths**: this change does not globally serialize
  startup, route loads, overview polling and live refresh. A broader consistency
  protocol requires separate ownership/versioning tests.
- **Font cache URLs**: immutable font URLs need content versioning when font
  contents change; no font was changed here.

Retry loops, billing capture, banked-reset/window-starter coordination and
provider identity were deliberately not unified: their replay, unknown-usage,
durability and foreground-work semantics differ. Existing shared infrastructure
already covers their compatible responsibilities.
