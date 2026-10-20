# Optimization implementation, 20 October 2026

This implements the actionable findings in the
[optimization research](../notes/optimization-research-2026-10-20.md), reviewed
by [Claude Opus 5.5 at extra-high effort](../notes/optimization-review-opus-5-5-2026-10-20.md).
The implementation is on `perf/reviewed-optimizations`, based on
`96626c93f03c9f94792685be1eb7f2591b974534` (Fusebox 0.7.0).

## Cache correctness and request fidelity

- One conservative cache-marker detector skips known tool schemas, tool-call
  arguments and structured-output schemas, including Gemini's camel/snake-case
  forms. A schema property named `cache_control` no longer suppresses automatic
  caching or causes a false incompatible-hint error. Nested tool-result and
  unfamiliar content blocks still retain legitimate controls, protecting the
  four-breakpoint limit.
- Original cache intent reaches final Claude preparation. OAuth cloaking honors
  explicit mode without breakpoints and `on-incompatible: drop`; neither gains
  automatic caching after translation. `automatic` remains an explicit choice
  when boundaries are lost. This policy correction can remove previously useful
  caching and is not a guaranteed cost reduction.
- Shared derived client/workspace cache keys retain separate Codex upstream
  session identifiers on HTTP and native WebSockets. Caller keys and session
  headers keep their precedence. Claude scope/TTL behavior is unchanged.
- Chat↔Responses keeps late system/developer instructions in order, including
  their cache markers. Claude, Gemini and Devin targets add late instructions
  to the system prompt in order, as before, keeping their authority rather
  than demoting them to user turns. Leading instructions retain the previous
  aggregation behavior.
- Chat `verbosity` and Responses `text.verbosity` survive supported GPT-5/6
  translations on the Codex provider. Translated service tiers require
  `translation.allow-service-tier: true`; unsupported/dropped controls have
  explicit diagnostics. Native requests retain their existing controls.
- Translated Responses output emits the canonical positive
  `usage.input_tokens_details.cache_write_tokens`, plus the legacy
  `cache_creation_tokens` alias with the same count. Parsers still accept legacy
  spellings. Chat output retains its existing spelling pending a verified Chat
  contract. Native usage continues to distinguish unknown from zero; the numeric
  translation IR has not been redesigned to represent unknown usage.

## Memory, retries and streams

- A same-account retry reuses immutable encoded request bytes, headers, cache
  policy and tool-name mapping. Only the latest prepared request is retained.
  Account/model/wire and a credential/identity stamp invalidate preparation, so
  a different account or refreshed token receives independent preparation.
  The initial JSON parse and account-specific rewrite still happen; there is no
  general raw-body fast path.
- Responses continuation histories use immutable shared segments. HTTP capture
  reuses its history, and incremental native WebSocket turns append only deltas.
  Full history is materialized when HTTP fallback requires it. Branch isolation,
  client/session isolation, `store:false` continuation compatibility and the
  existing logical limits remain: 8 MiB per entry, 64 MiB total, 1,000 responses.
  Serialized-byte limits are not exact RSS limits. Chains compact at depth 1,024.
- SSE events and fragmented lines are bounded; completed usage events preceding
  an oversized event are retained. Native WebSocket events also receive a size
  check within their existing transport bound. The default event limit is
  16 MiB, or 64 MiB for upstream model names containing image/audio/video.
  After the first event, held chunks are released once the 2 MiB bootstrap
  threshold is reached. Before it, the first-event deadline keeps applying and
  the event limit bounds buffering.
- Optional `stream-limits.pre-output-timeout-seconds` gives HTTP headers and the
  first event one absolute deadline, and bounds native WS sending/first event.
  It is off by default. Expiry and ambiguous upstream gateway timeouts are
  terminal `504`, avoiding automatic repetition of potentially paid inference.
  The deadline ends at the first event, not necessarily the first text token.
  Event bounds do not impose a global process-memory limit or bound ordinary
  nonstreaming JSON response collection.

## Measurement and experiments

- `/metrics` separates input, cache reads, writes, output and reasoning across
  provider attempts, and counts reported versus unknown usage fields. Reasoning
  remains an output subset. New histograms cover total duration, first content,
  preparation and headers; retries and native WebSocket fallback reasons are
  counted. Provider/model/transport storage has 128 normal series plus one
  overflow series; it never labels sessions or prompt hashes.
  Token and latency series use the same actual-model and canonical provider
  labels (`openai`/`anthropic`) so cache and timing queries share a cohort.
- Allowlisted persistent numeric metadata adds final-attempt preparation/header
  timings, logical duration/TTFT, transport, attempt number, requested/applied
  cache intent, cache downgrade and dropped-control counts. Header timing is
  measured from attempt start, including preparation, so these durations should
  not be added together. Intermediate attempts and embedding chunks retain
  their individual native usage; not every observation has all timing fields.
- `scripts/cache-analysis.py` analyzes private native observation JSONL. It
  reports request-start gaps and writes with no later observed read in a cohort,
  right-censors incomplete TTL windows, and omits identifiers from its output.
  These are candidates for unused-write analysis, not proof of exact-prefix
  reuse or waste. Code meanings for persistent cache intent are: 0 unknown,
  1 provider default, 2 none, 3 automatic, 4 explicit, 5 explicit without
  breakpoints, 6 prewarm, 7 rejected.
- The context harness retains its strict context-only default and adds explicit
  cache/verbosity/effort/model/tools/tier experiment modes. Variants have separate
  session cohorts; initial-state, warmup and measured warm pairs are reported
  separately. Initial cache state is explicitly uncontrolled. Default execution
  is ten requests. Optional quality receipts attach metadata from an external
  evaluation; the harness does not independently evaluate answers. See the
  [experiment guide](context-efficiency.md).
- `scripts/runtime-benchmark.py` measures local mock HTTP requests with 10 KiB,
  200 KiB, 1 MiB and a synthetic multimodal body at concurrency 1/8, with
  2/4/8/default Tokio workers. It records latency, throughput, proxy CPU time,
  RSS/high-water RSS and threads. The mock accepts the synthetic image payload;
  this does not test image decoding, a real model or image quality.

## Optional workload optimizations

`latency-routing` is off by default. With Smart quota routing, it breaks exact
ties only for new sessions, preserving priority, quota/reset/load preferences
and existing assignments. HTTP and WS samples remain separate and match the
resolved model, input-size bucket, reasoning effort and cache intent. Samples
must be successful, single-attempt first turns with explicitly zero cache reads;
each account needs three samples younger than five minutes. Samples are bounded
to 64 contexts per account and reset with account identity. No hedging occurs.

Google embedding chunks support configurable concurrency 1–16, default 1. Each
chunk has independent usage and load tracking. Results keep input order; after
a failure, in-flight chunks drain for accounting and no new chunks are scheduled.
Completed work is never replayed on another account. Requests displays account
attempts and chunks separately. Final response validation is included in the
logical outcome while provider statuses and token usage remain intact.

Exact embedding result reuse is opt-in (`embeddings.cache-seconds`, default 0).
Keys include tenant, account credential identity, resolved model and all request
options. Default serialized capacity is 16 MiB, at most 1,000 entries. Values are
shared inside the cache; response copying and serialization happen outside its
lock. A hit returns zero provider usage and `fusebox_cache_hit: true`; metrics
report hits, misses, entries and bytes. Setting changes clear the cache, smaller
TTLs/capacity apply to existing results, and stale requests cannot refill a
disabled/resized cache. This changes provider work only when explicitly enabled.

## Validation and measurements

Measurements use local synthetic data and a release build with rustc 1.96.1,
the existing speed-oriented release profile and the existing four-arena glibc
allocator settings. No real-provider inference or answer-quality evaluation was
performed. No production token, quota, cache-hit or latency savings are claimed.

The history microbenchmark compares the former deep-copy/full-size calculation
with shared append for 64 turns, and checks that final histories are identical.
It excludes initial history construction and final flattening from both timing
loops. This debug-build data-structure result does not estimate network latency
or process RSS:

| Initial history | Deep copy and full sizing | Shared append |
| --- | ---: | ---: |
| 10 KiB | 16.327 ms | 0.190 ms |
| 200 KiB | 174.921 ms | 0.181 ms |
| 1 MiB | 870.418 ms | 0.186 ms |

[Raw history results](history-optimization-results-2026-10-20.json).

The optimized standalone SSE decoder measured 0.46–0.48 ms for a fragmented
1 MiB line, 1.83–1.98 ms for a 4 MiB line, and 0.269 ms for 2,000 small events
(276 KB). These are current decoder-only measurements, not a before/after
comparison. [Raw CSV](sse-optimization-results-2026-10-20.csv).

The runtime benchmark completed 3,072 measured requests: 96 for each of 32 cases,
plus warmups, on a 64-thread host. Both HTTP peers use TCP_NODELAY and the mock
has a backlog of 128. The table selects the 1 MiB text cases; RSS high water is
the kernel-reported maximum across all payloads for each process:

| Workers | Concurrency 1 median / p95 | Concurrency 8 median / p95 | RSS high water | Threads |
| --- | ---: | ---: | ---: | ---: |
| 2 | 3.49 / 212.38 ms | 10.37 / 15.26 ms | 115.1 MiB | 5 |
| 4 | 3.44 / 211.85 ms | 7.86 / 9.96 ms | 123.9 MiB | 7 |
| 8 | 3.31 / 210.89 ms | 8.74 / 10.77 ms | 125.6 MiB | 11 |
| default (64) | 3.39 / 211.35 ms | 8.20 / 10.19 ms | 137.7 MiB | 67 |

This is a worker-count experiment on the current implementation, not a
before/after revision comparison. Timings include a Python HTTP mock and fresh
client connections. Other host load was uncontrolled; CPU counters have coarse
scheduler-tick resolution. RSS includes cumulative continuation retention, even
with `store:false`. The sequential 1 MiB cases showed intermittent approximately
210 ms tail outliers across worker counts; their cause was not isolated, so
this mock run does not establish production p95 behavior. Two workers were
slower on the larger concurrent workload in this run. These measurements do
not justify a universal default change.
[All runtime cases and binary hash](runtime-optimization-results-2026-10-20.json).

Reproduce the local measurements without inference credentials:

```sh
CARGO_BUILD_JOBS=4 rustup run stable cargo build --release --locked
CARGO_BUILD_JOBS=4 rustup run stable cargo test --locked shared_history_measurement -- --ignored --nocapture
rustup run stable rustc -O --edition=2024 scripts/sse-benchmark.rs -o /tmp/fusebox-sse-benchmark
/tmp/fusebox-sse-benchmark
agent-resource run --task "Fusebox runtime measurement" --ttl 1h -- \
  python3 scripts/runtime-benchmark.py --binary target/release/fusebox --requests 96
```

Final checks passed with `CARGO_BUILD_JOBS=4` and four Rust test threads:

- Rust: **566 passed**, one intentionally ignored measurement; the ignored
  history measurement also passed when explicitly run.
- Clippy: all targets, locked dependencies, warnings denied.
- Rust formatting and `git diff --check`: clean.
- Dashboard: **85 passed**.
- Python operational/benchmark checks: **33 passed**, plus new-script compilation
  and execution of the runtime harness.
- Release build: passed, rustc 1.96.1.

Regression coverage includes complete OAuth/API cache-policy paths, legitimate
and colliding markers, instruction order, controls, canonical usage, history
branches, stream bounds, session-header overrides, latency eligibility, embedding
order/concurrency, independent usage, partial failures, invalid vectors, exact
cache isolation/invalidation and a terminal deadline without inference replay.

No installed/running Fusebox development service or listener on ports 8317/18317
was found. The rebuilt release was therefore started under an `agent-resource`
lease, checked, stopped and restarted on the same config and port. Health,
readiness, compressed dashboard assets, default settings, live settings changes,
translation ordering/verbosity, opt-in service tier and new metrics passed;
changed settings survived restart. All temporary services were stopped and all
measurement/smoke leases checked closed with no retained resources or
cleanup errors. A smoke-driver assumption about the plain-text `/healthz` body
was corrected before the successful run; it was not an application failure.
[Dev-validation receipt](dev-validation-2026-10-20.json) and
[resource-cleanup receipts](optimization-resource-cleanup-2026-10-20.json).

## Retained choices and operational use

Model, reasoning effort, cache scope, TTL, tier and context content remain caller
or operator choices. Use the experiment modes to compare task completion,
uncached/read/write input, output and reasoning together. Subscription quota
effects cannot be inferred from API list prices. There are no keep-warm model
calls, silent prompt compaction, default effort/tier reductions or universal
worker-count changes.

Native Codex WebSockets remain enabled where already supported. Proxy/non-Codex
and model-group routes retain HTTP fallback; the new counters make its frequency
measurable before backend-specific support is expanded. Undocumented provider
identity/cloaking fields remain intact. Data-dependent changes to TTLs, caller
tools/prompts, worker count or backend support require representative traffic
and quality measurements; the new controls do not pretend to supply that data.
