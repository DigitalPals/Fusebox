# Full-stack optimization follow-up, 29 October 2026

Baseline: `8acb559ab2540ad2f6b59fbabf719dd7329a13f1` (v0.11.0), which already
contains the [earlier optimization pass](efficiency-2026-10-29.md). This pass
targets additional redundant work. Three implementation agents covered runtime,
storage and dashboard code; the coordinating agent covered background scheduling,
HTTP/assets, builds and operations. Cross-reviews covered the final runtime,
storage and discovery changes.

Claude Opus 5.5 at max effort subsequently verified this pass. Its low-severity
discovery findings and the fixes are documented in the
[independent review follow-up](optimization-opus-review-2026-10-29.md), with
[post-review validation](optimization-opus-validation-2026-10-29.json). The
measurements and validation counts below describe the initial optimization pass.

## Changes

- **Embeddings cache:** maintain its byte count incrementally and sweep only at
  the earliest expiry or a limit change. Warm hits no longer scan up to 1,000
  entries under the cache mutex. Shorter TTLs, original expiry deadlines, LRU
  eviction, capacity changes, disable/clear behavior and counters are preserved.
  Removing the earliest entry can leave an earlier sweep deadline; this only
  causes one unnecessary sweep, never a stale hit.
- **Claude attribution:** borrow the initial user text and inspect at most 21
  UTF-16 code units for the three-character billing fingerprint. Long prompts
  no longer create a full UTF-16 vector. Replaced system blocks are moved instead
  of cloned. Equivalence tests cover short strings, Unicode, surrogate pairs,
  and large prompts.
- **Model discovery:** automatic and manual checks share a four-request limit.
  Automatic checks overlap across accounts; manual checks no longer launch every
  account at once. Account claims suppress overlapping duplicate checks, and
  queued checks revalidate eligibility, minimum gaps and backoff. Dropping a
  future releases its account claim and concurrency slot. Manual results count
  checks actually performed.
- **Usage breakdowns:** select the top 500 typed aggregates before constructing
  JSON, and select the top 50 models for each returned client. Large journals no
  longer serialize groups that the response discards. Cost ordering, ties,
  unknown values, signed overflow and response fields match the prior output.
- **Usage compression:** reuse one scratch buffer within each compression batch;
  retained blobs reserve their actual compressed size. The dictionary, frame
  bytes, database schema and decoder bounds are unchanged. Scratch capacity
  retains the batch's largest requirement until the batch finishes.
- **History collector:** count the serialized batch envelope once and each
  observation once, including JSON commas. This replaces cloning and serializing
  every growing candidate batch. The exact 512 KiB cutoff, normalized wire bytes,
  selected prefix and original acknowledgement payloads remain unchanged.
- **Dashboard dates:** reuse four formatter styles, each retaining only its most
  recent timezone. Locales, DST, missing/invalid timestamps and timezone changes
  have parity tests. Warm chart hover no longer constructs a date formatter.
- **Model-family coverage:** accumulate unique models per account while traversing
  routes, eliminating a repeated route scan for each account. Route order,
  metadata, prefixed aliases and coverage output are preserved.

## Component measurements

These are synthetic local component measurements, not provider latency or
end-to-end throughput claims. They measure different workloads and should not be
added together. Host scheduling and debug/release differences affect timings.

| Workload | Before | After | Method |
| --- | ---: | ---: | --- |
| 100,000 cache hits / 1,000 entries | 377.61 ms | 4.14 ms | Optimized Rust, median of 9 |
| 200 fingerprints / 1 MiB ASCII prompt | 70.38 ms | 0.222 ms | Optimized Rust, median of 9 |
| 10,000 usage breakdown groups | 413.64 ms | 22.25 ms | Debug Rust, median of 5 |
| 10,000 compressed metadata documents | 37.69 ms | 36.96 ms | Debug Rust, median of 5 |
| 200-record collector batch sizing | 250.09 ms | 4.02 ms | Debug Rust, median of 5 |
| Chart hover / 30 days / 20 groups | 44.83 µs | 19.37 µs | Node VM, median of 5 batches |
| Coverage / 300 models / 80 accounts | 20.526 ms | 0.964 ms | Node VM, median of 5 batches |

The compressor's primary measured benefit is allocation capacity, not a meaningful
CPU speed claim: cumulative retained blob capacity for 10,000 documents falls
from 16,470,000 to 940,000 bytes, with 1,641 bytes of reused scratch. These totals
are allocation capacity across documents, not process peak RSS.

Other deterministic reductions:

- 200-record collector batch sizing serializes 200 observations instead of 20,100.
- A 10,000-group breakdown serializes 500 groups instead of all 10,000.
- 60 warm chart hovers construct zero date formatters instead of 60.
- The 300-model/80-account coverage fixture reads routes 901 times instead of
  24,901 times.
- For the 200 large-prompt fingerprints, cumulative allocated bytes fall from
  978,697,600 to 23,600. This excludes the request and other preparation work.

Cache measurements exclude key hashing, JSON serialization and mutex contention.
Fingerprint measurements substitute a fixed client version. Dashboard measurements
exclude browser DOM parsing, layout and painting. Storage measurements compare
the prior implementation with the new implementation in the same test binary.

Raw evidence:
[runtime](cache-fingerprint-results-2026-10-29.json),
[storage](optimization-followup-storage-2026-10-29.json),
[dashboard](optimization-followup-ui-2026-10-29.json).

Reproduction:

```sh
CARGO_BUILD_JOBS=4 rustup run stable cargo build --release --locked
python3 scripts/cache-fingerprint-benchmark.py --baseline-ref 8acb559
node scripts/ui-benchmark.cjs --baseline-ref 8acb559
rustup run stable cargo test --locked usage_document_encode_benchmark -- --ignored --nocapture
rustup run stable cargo test --locked usage_dashboard_selection_benchmark -- --ignored --nocapture
rustup run stable cargo test --locked usage_collector_batch_sizing_benchmark -- --ignored --nocapture
```

## Validation and development restart

The local `ci-platform run` catalog has no Fusebox entry. Validation uses a
four-CPU/eight-GiB CI host capacity reservation, four Cargo jobs and four Rust test
threads. Final check counts and source hashes are recorded in
[validation](optimization-followup-validation-2026-10-29.json).

- Rust: **757 passed**, zero failed, five intentionally ignored measurements.
  The three new storage measurements were also run separately, five times each.
- Dashboard: **143 passed**. Python: **46 passed**.
- Formatting, all-target Clippy with warnings denied, release build, and
  whitespace checks passed.

No existing local Fusebox development unit, container or process was found.
The rebuilt release passed checks under a managed temporary lease with synthetic
credentials and loopback providers, including a graceful restart on the same
configuration, port and usage database. All **12** existing collector acceptance
checks passed, covering separate collector processes, replay, restart/reconnect,
authorization, credential rotation, historical pricing and content privacy.
[Development results](optimization-followup-dev-2026-10-29.json) and
[cleanup](optimization-followup-cleanup-2026-10-29.json) record the outcome.
Both test leases are closed with no remaining resources. The CI host reservation
was released. No persistent service or production deployment was created.

## Audit coverage and deferred opportunities

The review covered HTTP/WebSocket proxying, native and translated protocols,
retries, SSE and response limits, session affinity, identities, account routing,
admission, policy, embeddings and media; SQLite writers, queries, rollups,
retention, compression, import/resume and collectors; dashboard live updates,
usage rendering, model/config views, i18n, styles and service worker; quota/model
polling, OAuth, banked resets, window starters and notifications; HTTP middleware,
static assets, dependency features, Cargo profiles, Docker and CI/release scripts.

Existing shared HTTP clients, bounded bodies/streams, retry preparation reuse,
shared continuation histories, write batching, indexes/rollups, incremental
imports, change-only broadcasts, precompressed assets, incremental asset builds,
Docker dependency layers and CI cancellation remain appropriate. Serial reset
and window-starter operations retain their foreground-work and durable-journal
coordination. Release LTO, Tokio worker defaults, provider behavior, prompts,
reasoning settings and operator cache policies are unchanged.

Further candidates need workload evidence and explicit invalidation or ownership
design before implementation:

- Detail-page collector labels perform repeated SQLite lookups; bulk lookup or a
  per-read cache could reduce queries without retaining stale labels.
- Pricing catalogue JSON is deserialized per insert batch; reusing a parsed
  catalogue needs correct metadata/version invalidation.
- Default trusted-proxy CIDRs and some model/account lookup paths repeat parsing
  or scans. Cache them with the exact configuration generation if measurements
  show material cost.
- Removing inferred-session serialization buffers would require preserving the
  length-prefixed hash without an expensive second pass.
- Larger decompressor output chunks may reduce admission-lock frequency but
  require concurrent memory/body-limit measurements.
- Fonts retain long-lived immutable URLs; future content changes require new
  filenames or content-versioned URLs.
