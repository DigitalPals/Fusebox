# Whole-stack optimization, 29 October 2026

This pass reviews Fusebox's request pipeline, storage and import paths, dashboard,
asset delivery, build settings and operational workflows. Three implementation
agents handled runtime, storage and frontend changes; a second review covered
storage correctness and build/operational behavior. The base is `f49466b`
(Fusebox 0.10.1). This report records validation before production deployment.

## Implemented changes

- **Routing:** wildcard model/policy comparisons use borrowed bytes and
  ASCII-insensitive comparisons, avoiding two lowercase strings and a split
  vector for each match. Regression coverage compares the previous algorithm
  over 121,836 pattern/value pairs, including empty and Unicode cases.
- **Streaming:** complete SSE lines are borrowed from the received network chunk.
  Only fragmented lines enter the carry buffer. Event/line bounds, fragmented
  UTF-8, trailing events and completed events before an oversized event retain
  their existing behavior.
- **Session inference:** multimodal/nontext blocks serialize through a borrowed
  cache-marker filter rather than deep-cloning their contents. The serializer
  reproduces the old map-removal ordering, so previously inferred session hashes
  stay stable. It still creates serialized bytes for hashing.
- **Usage writes:** writer health checkpoints skip SQL when persisted counters
  have not changed. Producer-side drops/rejections and failed-write retries still
  persist; shutdown forces a final checkpoint.
- **Usage reads:** one bounded zstd decoder and database/dictionary identity serve
  each details or maintenance batch, including repricing, dictionary training,
  compaction, downgrade and rollback. Per-document size, type, UTF-8 and corruption
  checks remain in place.
- **History imports:** plain files validate their prefix/signature and seek to
  the checkpoint instead of rereading all previous bytes on every scan.
  Compressed histories keep their bounded sequential resume. Regression cases
  cover lookahead, partial UTF-8, truncation with an unchanged prefix, rewrites,
  oversized lines and following valid records.
- **Dashboard:** number/currency formatters and immutable chart aggregates are
  reused. Hover updates bar classes and the readout while retaining the bars and
  controls. Changing grouping no longer renders the full chart twice. New data,
  range, metric, grouping and theme changes invalidate the relevant aggregate;
  labels remain live.
- **Asset builds:** content fingerprints reuse compressed text after dependency
  references are rewritten. Unchanged output files keep their timestamps.
  Dictionary edits invalidate the loader and index transitively. Validity is
  removed before a gzip replacement and recorded only after a successful write,
  preventing interrupted writes from reusing an old fingerprint after a source
  revert.
- **CI:** superseded PR/main CI runs and main image builds cancel obsolete work.
  Every tagged Docker run has a unique concurrency group and remains independent.

## Measurements

These are local synthetic component measurements. They do not measure provider
inference speed, subscription quota savings, answer quality or production
throughput. Rust microbenchmarks use stable rustc 1.96.1 on Linux x86-64;
dashboard measurements use Node 22.22.3.

| Workload | Before | After | Scope |
| --- | ---: | ---: | --- |
| 600,000 wildcard comparisons | 39.13 ms | 12.03 ms | Optimized standalone Rust; median of 9 |
| Allocations in those comparisons | 1,800,000 / 52.7 MB | 0 | Counting allocator, harness excluded |
| 2,000 small SSE events | 0.337 ms | 0.232 ms | Decoder only; median of 9 |
| 10,000 compressed 1,614-byte documents | 86.65 ms | 45.34 ms | Debug test; median of 5, fresh versus reused context |
| Chart hover, 30 days / 20 groups | 2.294 ms | 0.017 ms | JavaScript/markup only; excludes browser layout/paint |
| Warm asset build-script rerun | 64.32 ms | 14.07 ms | Same fixture/compiler; median of 9, excludes Cargo compilation |

Deterministic checks also show:

- 120 unchanged health checkpoints produce zero database writes after the first
  persisted snapshot; dirty counters produce one update and failures are retried.
- Seeking across a 1 MiB checkpoint reads only the 4,096-byte prefix and 7-byte
  suffix in the counted-reader fixture. Real `BufReader` lookahead may read a
  small extra buffer; the historical prefix is no longer scanned.
- 60 chart hovers perform zero full-chart replacements, zero chart aggregations
  and zero new number-formatter constructions after warmup.
- Fresh old/new asset outputs are identical. Unchanged reruns retain output
  timestamps; missing/incomplete outputs recover; a dictionary edit changes only
  its gzip, its loader gzip, and the index gzip.

Raw receipts: [runtime primitives](stack-runtime-microbench-2026-10-29.json),
[storage](stack-storage-results-2026-10-29.json),
[dashboard](stack-ui-results-2026-10-29.json),
[asset builds](stack-asset-build-results-2026-10-29.json).

The [initial baseline](stack-runtime-baseline-2026-10-29.json),
[initial optimized run](stack-runtime-optimized-2026-10-29.json) and
[three alternating runtime rounds](stack-runtime-paired-2026-10-29.json)
measure native nonstreaming HTTP with 10 KiB, 200 KiB, 1 MiB and synthetic
multimodal requests at concurrency 1/8 with four Tokio workers. They include
Python clients/providers, fresh connections, cumulative continuation retention
and uncontrolled host scheduling/clock behavior. The baseline is the existing
release executable identifying itself as `f49466b`, rather than a separately
rebuilt baseline with independently verified compiler flags. These runs validate
the workload and expose timing variability; the component results should not be
interpreted as an equivalent end-to-end or provider latency improvement.

Across the three paired rounds, the median-of-medians for the synthetic
multimodal case was 3.94 versus 3.39 ms at concurrency 1 and 8.66 versus 7.11 ms
at concurrency 8. Text cases ranged from a 23% reduction to a 5% increase;
these small samples and the artifact/environment limits above do not establish
a universal proxy latency improvement or a controlled regression threshold.

Reproduce the component measurements after building release dependencies:

```sh
CARGO_BUILD_JOBS=4 rustup run stable cargo build --release --locked
python3 scripts/stack-runtime-microbench.py --baseline-ref f49466b
node scripts/ui-benchmark.cjs --baseline-ref f49466b
python3 scripts/asset-build-benchmark.py --baseline-ref f49466b
rustup run stable cargo test --locked usage_document_decode_benchmark -- --ignored --nocapture
```

## Validation and development restart

The local `ci-platform run` catalog has no Fusebox project. The repository's
checks ran locally with a four-CPU/eight-GiB CI host capacity reservation, four
Cargo jobs and four test threads:

- Rust: **743 passed**, zero failed, two intentionally ignored measurements.
  The new ignored decoder measurement was also run separately.
- Dashboard: **137 passed**.
- Python operational/benchmark regressions: **46 passed**.
- `cargo fmt --check`, all-target Clippy with warnings denied, release build,
  workflow YAML parsing, new Python compilation and `git diff --check`: passed.

[Validation and source hashes](stack-validation-2026-10-29.json).

No existing Fusebox development service, container or process was found. The
rebuilt release ran under a managed temporary lease, stopped gracefully and
restarted on the same config and port. Checks verified health/readiness, native
and translated proxy requests against a loopback mock, compressed request input,
versioned gzip/identity assets, conditional 304 responses, metrics and retained
usage records with no drops. The existing real-process collector acceptance
script also passed all **12** checks, including another persistent restart.
[Development receipt](stack-dev-validation-2026-10-29.json).

All managed test/measurement leases were checked closed with no remaining
resources. No persistent service was created. This predeployment validation did
not use production services or personal credentials/history.
[Cleanup receipts](stack-resource-cleanup-2026-10-29.json).

## Reviewed choices and remaining opportunities

The audit also covered native/translated HTTP and WebSockets, preparation/retry
reuse, shared continuation histories, cache intent and identity binding, quotas
and model discovery, embeddings/media, storage schemas/rollups/retention/imports,
dashboard live updates/config/i18n/styles/service worker, authentication and body
admission, asset negotiation, Cargo/SQLite features, Docker stages and release
workflows. Existing connection pooling, shared history, bounded reads and quotas,
precompressed assets, change-only broadcasts and runtime-oriented release
settings already address major costs. The
[operational review](stack-operations-review-2026-10-29.json) records that coverage.

Further candidates need focused workload evidence or concurrency testing:

- Model discovery checks due accounts serially while manual checks use unbounded
  `join_all`; a shared bounded scheduler could improve multi-account refreshes.
- The opt-in embeddings cache scans up to 1,000 entries on hits; benchmark a
  deferred expiration sweep with incremental byte accounting before changing it.
- Larger compressed-input decoder chunks could reduce allocation/admission-lock
  frequency; measure memory and streaming behavior before changing the buffer.
- Dashboard date formatting may benefit from formatter reuse, with invalid-date,
  timezone and locale behavior covered first. Future font edits need matching
  versioned URLs or filenames because existing WOFF2 URLs cache for a year.

Provider/model choices, prompts, reasoning effort, cache lifetimes, Tokio worker
defaults, release LTO/optimization settings and supported protocols remain
operator choices; this pass targets redundant local work.
