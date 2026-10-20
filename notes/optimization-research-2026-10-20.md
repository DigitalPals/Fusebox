# Fusebox optimization research — 20 October 2026

Status: independently reviewed by Claude Opus 5.5 at extra-high effort and
reconciled against the source. See the
[full independent review](optimization-review-opus-5-5-2026-10-20.md).

Follow-up: the actionable recommendations are implemented and validated in the
[implementation and measurement report](../docs/efficiency-2026-10-20.md).

Scope: `/home/john/Code/Fusebox`, version 0.7.0, commit
`96626c93f03c9f94792685be1eb7f2591b974534`. The thread is attached to the
empty lowercase `/home/john/Code/fusebox`; the actual repository is the capitalized
directory. This is an investigation, not an implementation or deployment.
No inference benchmarks, production changes, or service restarts were performed.

## Recommendation

Yes, there are useful remaining optimizations. Prioritize correctness of cache
intent and preservation of caller cost controls, then measure real cache reuse
and output/reasoning usage. Treat large-body allocation work and runtime tuning
as workload-dependent experiments. The October 15 and 17 work already addressed
the most obvious SSE, database, dashboard, connection reuse, and allocator issues.

No new percentage improvement in end-to-end latency, token use, or subscription
quota is established by this research. Code findings below are distinguished
from historical measurements and proposals requiring experiments.

## 1. Fix cache intent across the complete preparation path

Priority: first implementation fix, while measuring prevalence from existing
logs. Evidence: confirmed by two static source reviews; no regression test was
executed during this research. Impact depends on traffic.

### A. Tool schemas can accidentally disable automatic Claude caching

`src/cache.rs:167-183`, `src/formats/claude.rs:257-282`, and
`src/upstream.rs:413-422` recursively search every JSON object for a field named
`cache_control` or `prompt_cache_breakpoint`. They do not distinguish supported
protocol locations from JSON Schema properties or tool-call arguments.

For example, this ordinary tool definition contains no cache instruction:

```json
{
  "type": "function",
  "name": "configure_http",
  "parameters": {
    "type": "object",
    "properties": { "cache_control": { "type": "string" } }
  }
}
```

The property definition is nevertheless considered a cache marker. On a translated
Claude request it prevents `apply_translated_cache_policy_with` from adding the
automatic cache control. On the OAuth cloak path the retained schema also makes
`configured_cache` true, suppressing its defaults. Diagnostics can label the
request explicit even though it contains no protocol-level breakpoint.

The same collision can falsely trigger a Claude-to-other-provider downgrade,
and `on-incompatible: reject` can then return an unnecessary 400
(`src/formats/mod.rs:115-119`, `src/proxy.rs:1135-1149`). On the OpenAI input side,
tool arguments are normally JSON strings; they start triggering this check after
translation parses them into Claude `tool_use.input` objects. Once such a tool
call is retained in history, the false detection can affect later turns too.
Native non-Claude-Code Claude clients using OAuth cloaking are also affected.
Prevalence is unknown.

Use one shared cache-intent detector that explicitly skips known opaque data:
tool schemas, tool-call input arguments, and structured-output schemas. Retain
conservative scanning elsewhere, including nested tool-result content, so a new
or unfamiliar valid marker does not lead to adding a fifth breakpoint. This is
safer than a narrow allowlist of today's content types. Test schema-property and
argument collisions alongside four legitimate breakpoints and nested controls.

### B. OAuth cloaking can undo an explicit no-cache decision

An OpenAI-format request with `prompt_cache_options.mode: "explicit"` and no
breakpoints expresses no cache writes or reads on supported OpenAI models. The
documented Fusebox translation policy preserves that intent.

The pipeline first drops the incompatible options (`src/formats/mod.rs:90-113,
139-155`) and correctly avoids automatic caching at
`src/proxy.rs:1150-1189` / `src/formats/claude.rs:280-282`. But for a Claude OAuth
account with cloaking enabled, `cloak_body` sees no remaining Claude markers and
adds a system breakpoint and top-level automatic caching
(`src/upstream.rs:168-185,276-277,324-332`). The original opt-out is not carried
in `Target`. The same issue affects dropped explicit boundaries: default `drop`
can become automatic caching on the cloak path.

Carry normalized caller intent into final account preparation, and apply default
cache policy once, after all rewriting. Test the complete translation × OAuth/API
key × drop/automatic/reject matrix. Existing regression coverage exercises the
API-key path (`src/routing_tests.rs:1569-1603`) and does not cover this composition.
This is about honoring policy; disabling cache is not itself universally cheaper.
Existing logs can already expose the explicit-no-boundaries mismatch as
`cache_requested=explicit_no_breakpoints` and `cache_applied=automatic`.

For dropped explicit boundaries, there is an additional product choice: on the
cloak path `drop` currently adds both a system boundary and automatic caching,
whereas `automatic` adds the top-level control before cloaking and therefore
suppresses the extra system boundary. Aligning `drop` with the README can remove
useful caching. Decide and document that behavior explicitly; do not sell that
policy correction as guaranteed savings.

## 2. Preserve caller efficiency controls and standard usage fields

Priority: first, alongside cache correctness. Evidence: source-confirmed gaps.

### A. Translation loses verbosity and service-tier choices

`Request` has no verbosity or service-tier field (`src/ir.rs:31-50`). The OpenAI
parsers preserve effort and output caps but do not read `verbosity` /
`text.verbosity` or `service_tier` (`src/formats/chat.rs:13-40`,
`src/formats/responses.rs:40-65`), and the builders do not restore them.
Consequently a translated request can lose a caller's low-verbosity preference
or explicit tier selection. Native pass-through requests retain these settings.

The clearest actionable path is Chat Completions to Responses: Codex, xAI and
Meta use Responses as their native wire format (`src/accounts.rs:126-134`).
Map Chat `verbosity` to `text.verbosity` for verified OpenAI model/backend
combinations. Honor an explicitly selected service tier only where supported,
and emit a diagnostic when it cannot be preserved. A priority tier may cost
more; preserving it is a correctness change, not necessarily a saving. Support
on third-party and subscription endpoints must be verified independently.
Native Codex pass-through does not strip `text` or `service_tier`.

Do not blindly apply a public-API field to every subscription backend.
In particular, ChatGPT/Codex preparation
deliberately strips output caps and `prompt_cache_retention`
(`src/upstream.rs:427-449`, `src/formats/responses.rs:365-374`), so a global proxy
output cap would not guarantee a provider-side cap.

For callers, choose model and reasoning effort by task, keep effort stable within
a cache cohort where practical, request concise output when suitable, and shorten
redundant tool results at their source. Evaluate task completion, not just fewer
tokens. The existing `background-model` option is specifically for Claude Code
Haiku calls (`src/proxy.rs:607-614`), not a general task router.

### B. Translated Responses output uses a legacy cache-write name

The Responses renderer emits positive writes as
`usage.input_tokens_details.cache_creation_tokens`
(`src/formats/responses.rs:795-805`). Current OpenAI documentation names
`cache_write_tokens`. Fusebox's own parsers and benchmark accept both spellings,
so its round-trip tests do not expose the interoperability gap.

Emit the canonical field, with a deliberate compatibility policy for the existing
alias. Validate zero, nonzero, absent and unknown usage separately: the IR uses
numeric zeros, while native analytics correctly preserves missing usage. Do not
make unknown usage appear proven zero. This improves external accounting rather
than directly saving tokens. The Chat renderer also emits `cache_creation_tokens`
(`src/formats/chat.rs:498-508`); verify the target Chat API contract before changing
that renderer. This research verified the canonical Responses spelling; attempts
to fetch the current Chat method reference returned 404.

### C. Preserve mid-conversation instruction positions where the backend supports them

The reviewer identified an additional mechanism worth investigating. Both OpenAI
parsers lift every system/developer message into `req.system`, even when it occurs
after conversation turns (`src/formats/chat.rs:45-49`,
`src/formats/responses.rs:117-129`). Builders then emit those instructions at the
front (`src/formats/responses.rs:218-233`, `src/formats/claude.rs:373-375`).

Adding an instruction halfway through a conversation can therefore rewrite the
early prompt prefix during translation. That changes model-visible ordering and
can invalidate cache reuse from the changed position onward. The mechanism is
confirmed in the code; frequency and savings are not measured. Preserve inline
developer messages on compatible Chat-to-Responses routes first. Other formats
need model-specific role support, explicit behavior, and quality checks. Do not
blindly turn privileged instructions into ordinary user text.

## 3. Tune cache boundaries and scope by workload and backend

Priority: high potential benefit, after the correctness fixes. Evidence: provider
documentation plus code; no new measured hit-rate gain.

Already implemented: session affinity, caller-key precedence, session/client/
workspace derived keys, namespace versioning, cache downgrade diagnostics,
OpenAI breakpoint preservation, and Claude automatic caching. These are not new
features to build (`src/cache.rs`, `src/formats/mod.rs`, `src/affinity.rs`).

For repeated independent tasks, put stable instructions, examples and tool
definitions first and put an explicit boundary before the variable task. A moving
automatic boundary on the entire varying request need not create a reusable
entry at the end of its shorter shared prefix. Keep exact ordering stable.
Use native provider controls on a supported path; translated boundaries can be
lost, as the existing downgrade diagnostics explain. Applying this advice to
cross-provider callers may require a tested, explicit provider-specific policy.

For growing coding conversations, preserve old messages and append new turns;
rewriting the prefix on every turn loses reuse. Compaction can reduce context but
also discard cache reuse; compare total input cost and answer quality, not just
context length or hit percentage.

Current official OpenAI guidance distinguishes:

- GPT-5.6 and later: explicit/implicit modes, a 1,024-visible-token cache minimum,
  `prompt_cache_options.ttl: "30m"`, and cache-write pricing. Keys are optional for
  routing and can separate cache accounting. A fresh derived key for every session
  can separate reuse that a deliberate client/workspace cohort could share.
- Earlier models: cache keys help routing; retention and minimum size differ.
  Partition high-volume cohorts deterministically rather than assuming a single
  giant shared key helps. The guidance suggests about 15 requests/minute per key
  for these models.

Current official Claude guidance has model-dependent minimums, 5-minute default
TTL or 1-hour TTL, and at most 20 lookback positions per breakpoint. Consecutive
tool-use/tool-result runs have special counting rules. Long response generation
uses part of the TTL: the clock starts at request start, not response completion.
An explicit stable boundary and, for suitable reuse intervals, a 1-hour TTL are
worth comparing. A 1-hour write costs more; it is not a blanket recommendation.

Important limits:

- Fusebox `prompt-cache.scope` changes only derived keys. Caller-supplied keys win
  (`src/cache.rs:56-57`); native Codex clients generally supply their own key.
- The key is used by the Codex and xAI preparation paths, not by Claude's. Changing
  `scope` does not tune Claude caching. On Codex OAuth, when a client has not sent
  a `session_id`/`session-id` header, Fusebox also copies the cache key into the
  upstream `session_id` header (`src/upstream.rs:593-597`). Shared derived scopes
  can then give independent sessions the same upstream session ID. Existing
  headers are preserved. Verify this subscription-backend behavior before
  recommending a scope change there.
- Key scope does not change account affinity. Provider organization/workspace,
  endpoint, model, prefix and account routing must be observed, not inferred from
  equal keys. Public API caching rules do not prove undocumented subscription
  quota multipliers or exact ChatGPT backend support.
- Claude OAuth cloaking generates an early system fingerprint from the first
  user text and learned client version (`src/upstream.rs:247-253,299-307`) and
  moves instructions into the first user message. The fingerprint is stable
  within a session; its effect on cross-session reuse depends on undocumented
  provider handling. Compare prepared-prefix fingerprints and actual native
  cache-read usage; a hash difference alone does not prove a provider cache miss.
  Do not remove required identity fields blindly.
- Do not add periodic model calls simply to keep a cache warm.

Illustrative arithmetic only: at a 1.25× write rate and 0.1× read rate, one write
plus nine full reads of a stable prefix costs 2.15 normal-input equivalents,
versus 10 without caching: 78.5% less for that prefix. At a 0.05× read rate it is
1.70 equivalents, or 83% less. This is not an estimate of total request savings,
Fusebox's present opportunity, or subscription quota savings. Existing cache
hits, dynamic suffixes, output/reasoning and failures change the result.

## 4. Make efficiency measurable with the telemetry already present

Priority: foundational. Evidence: source-confirmed coverage gaps.

Native usage capture already records separate uncached input, cache reads,
cache writes, output and reasoning; pricing supports separate write rates
(`src/usage/capture.rs:230-260`, `src/usage/types.rs:7-22`). Structured completion
logs already include TTFT, total latency, attempts, requested/applied cache policy
and downgrade reasons (`src/diagnostics.rs:185-206`). Reuse these before creating
another storage subsystem.

Prometheus currently exposes overall input/output/cached counters, but folds
writes into input and has no latency histograms or cache cohort dimensions
(`src/metrics.rs:80-105`). The persistent observation schema lacks latency and
cache-policy fields (`src/usage/types.rs:50-81`). Consequently the historical
usage dashboard cannot directly answer whether a given cache policy improved
p95 TTFT for a specific model/client cohort.

Start by analyzing the existing structured logs alongside native observations.
Count explicit-no-boundaries/automatic mismatches, cache downgrade reasons,
translated traffic, cache reads/writes per model and client, and request-start
gaps within sessions. Sessions with cache writes but no later observed reads are
candidates for waste analysis, not proof that an exact prefix was never reused:
usage counters do not identify the cached prefix, and traffic outside Fusebox
can also reuse it. Correlate representative requests before changing TTLs.
Then add bounded model/provider/transport metrics and histograms for preparation,
upstream headers, first output, total latency, retries, writes and reads as needed.
Keep per-session IDs and prompt hashes out of Prometheus labels. Persist compact
aggregates only if historical reporting requires them.

Use token-weighted cache hit rate = sum(read tokens) / sum(total input tokens).
Price disjoint input categories separately; output includes reasoning where the
provider says so, so do not add reasoning again. Report missing/partial usage,
failed attempts, cancellations and task completion alongside successful latency.
API list-price equivalents and subscription quota consumption are separate.

The context benchmark is a useful base, but only `input` and `instructions` can
differ (`scripts/context-benchmark.py:393-396`). It cannot currently compare
top-level cache modes/TTLs/keys, tool-schema reductions, verbosity, model or effort.
It rejects stateful continuations and does not evaluate answer quality. Extend it
with explicit experiment modes rather than weakening its existing context-only
invariant. Use native usage/journal correlation to check routing and completeness.
Its alternating variants currently share a session and cache state; default
three-pair results mix first writes with warmed runs. New cache experiments need
separate cold/warm reporting and controlled cohorts, while keeping model/account
routing comparable. Different keys alone do not establish cold caches on every
provider. `numeric_metadata` is presently allowlisted to usage counters
(`src/usage/types.rs:151-167`), so adding latency there still requires contract
and validation changes; it is not a free-form extension point.

## 5. Reduce large-context memory and serialization work where it matters

Priority: medium; profile first. Evidence: code paths and historical measurements.

- Native HTTP pass-through deep-clones the JSON body per attempt and serializes
  the prepared body (`src/proxy.rs:1051-1056,1206`). Responses capture separately
  copies request input before execution (`src/proxy.rs:647-655`). Candidate:
  immutable shared input plus a small account-specific overlay, or a carefully
  scoped raw/borrowed fast path. Retried accounts must still receive independent
  model/auth/cloak transformations. Measure allocations and CPU for 10 KB,
  200 KB, 1 MB and multimodal bodies under realistic concurrency.
- Every stored Responses continuation contains its full history. Shared `Arc`
  ownership prevents cloning under the routing lock, but `previous()` still
  deep-copies it, and each new response stores another full prefix
  (`src/affinity.rs:763-809`). Growth is quadratic in turn count before existing
  eviction bounds; this is not an unbounded leak. Consider immutable shared
  segments with branch-safe eviction and client isolation. Current caps are
  8 MiB/entry and 64 MiB total *serialized JSON*, not exact RSS.
- Native incremental WebSocket turns also materialize the full local history
  (`src/ws.rs:440-459`) and clone it again into continuation capture
  (`src/ws.rs:547-550`). This makes history sharing a relevant optimization even
  when upstream WebSocket traffic sends only a delta. Investigate capture scope
  and eviction pressure, but do not simply disable capture for `store:false`:
  Fusebox currently provides its own continuation behavior, and callers or WS
  fallback may rely on it.
- `SseDecoder` has no maximum unfinished line/event size (`src/sse.rs:19-72`),
  and bootstrap holds upstream chunks without a byte cap
  (`src/proxy.rs:1565-1594`). Request admission limits do not bound these response
  buffers. Add configurable, media-aware event/bootstrap limits and explicit
  accounting of an aborted partial stream. Bootstrap already has a time bound.
  This is memory predictability and defense in depth, not a demonstrated normal
  performance bottleneck. Legitimate image and tool events must still fit.
- Tokio's default runtime uses the available worker count (`src/main.rs:105-107`).
  Existing October 17 measurements found that fewer workers reduced CPU/RSS on
  a 64-thread host. `TOKIO_WORKER_THREADS` already controls this without a code
  change. Compare 2/4/8/default workers on this workload against the current
  four-arena allocator baseline (historically 106–125 MiB under that synthetic
  load), not the older untuned 206–227 MiB baseline. Do not impose a universal
  2-worker default or replace the already-tuned allocator without data.
  Existing HTTP admission settings can bound concurrency on small servers, but
  their byte budget is not a complete process-memory limit and excludes native WS.

## 6. Smaller or workload-specific opportunities

- Preserve native Codex WebSockets when possible: reuse already exists. Model
  groups and configured proxies currently fall back to the HTTP pipeline
  (`src/ws.rs:590-624`). Measure fallback frequency before expanding WS support.
- New-session latency-aware tie-breaking could help only among otherwise
  eligible equal-priority/quota candidates. Current scoring uses quota, reset
  timing and load for smart-quota routing (`src/accounts.rs:1703-1722`). Normalize by model, input size,
  cached share and reasoning settings; do not move a warm session to chase noisy
  latency samples or duplicate paid inference as a hedge.
- Vertex embedding batches run sequentially in chunks of one or five; Gemini
  chunks 100 (`src/embeddings.rs:257-270,302-309`). If embeddings are a material
  workload, bounded concurrency and an opt-in tenant/model/options-scoped exact
  cache can help. Preserve output order, account for every completed subrequest,
  and never replay successful chunks after a later failure. Current accounting
  correctly creates a separate observation for every chunk: `request()` starts
  a new attempt (`src/embeddings.rs:222`), which finishes the previous capture
  (`src/usage/capture.rs:112-125`). Parallelism needs independent per-subrequest
  capture rather than the current single mutable tracker. Also distinguish
  planned chunks from retries in the displayed attempt count.
- Headers and first SSE event currently get separate first-token timeout windows
  (`src/proxy.rs:1210-1212,1354,1565-1568`). Consider an absolute attempt deadline
  for predictable tail latency. The existing 120-second retry budget bounds
  starting further attempts, not the current attempt's duration. Preserve the
  deliberate no-retry handling for ambiguous gateway timeouts. This is a low
  priority tail-latency experiment: failing over too quickly can duplicate paid
  work that is still running upstream.

## Already completed work and measurement limits

The October 17 report records a 30-day summary improvement from 457 ms to 9.6 ms
and database size from 1,913 MiB to 653 MiB on a production copy. It also records
roughly 0.74–0.77 ms/request in a sequential mocked proxy benchmark. These are
historical results from that revision and workload, not measurements of today's
production or provider latency. See `docs/efficiency-2026-10-17.md`.

Already shipped: dictionary compression and rollups; dashboard gzip/delta updates;
linear fragmented-SSE scanning; frame batching; connection pooling; deferred
session persistence; cached parsed requests; glibc arena tuning. Do not count
these as fresh savings. `opt-level=3` is already the runtime-speed release profile;
size-first compiler settings trade speed and are not the leading recommendation.

Suggested implementation order (priority reflects source evidence and expected
leverage, not measured production prevalence):

1. Establish a baseline from existing logs and native observations while adding
   regression tests for the two cache-intent bugs. Fix their complete preparation
   path, then canonical Responses cache-write output and supported Chat-to-Responses
   verbosity/tier preservation, with explicit downgrade reporting.
2. Measure the prevalence of moved instructions; preserve their positions where
   compatible. Extend the experiment harness, then evaluate stable boundaries,
   suitable TTLs, caller context/tool-output reductions and reasoning effort.
3. Add bounded metrics where log analysis is insufficient; profile HTTP/WS history
   copies, body allocations and worker counts. Treat stream byte limits separately
   as memory protection.
4. Only if traffic warrants: embedding optimization and latency-aware new-session
   selection. Keep quality and quota/account behavior as acceptance criteria.

## Sources

Read on 20 October 2026:

- [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
  — fetched through the official OpenAI documentation MCP; model differences,
  boundaries, key behavior, cache-write accounting and compaction caveats.
- [Claude prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
  — fetched as Markdown; minimums, lookback, exact prefixes, TTL timing and prices.
- [OpenAI agent optimization guidance](https://developers.openai.com/cookbook/examples/agent_optimization/optimizing_agents_for_cost_and_quality#optimization-knobs)
  — fetched through the official documentation MCP; output, effort, tool payloads,
  cache stability and task-quality evaluation.
- Current code references above, `README.md:404-464`,
  `docs/context-efficiency.md`, `docs/efficiency-2026-10-15.md`,
  `docs/efficiency-2026-10-17.md`, and
  `notes/improvement-research-2026-10-17.md`.

## Independent review and reconciliation

The review ran through T3 delegation on provider `claudeAgent`, model
`claude-opus-5-5`, with `effort=xhigh`; the child configuration was read back to
verify that selection. The task completed successfully. The complete review is
saved in [optimization-review-opus-5-5-2026-10-20.md](optimization-review-opus-5-5-2026-10-20.md).

Opus independently confirmed the two cache-intent bugs, the missing translation
controls, the Responses usage-field mismatch, and the telemetry/history findings.
It added instruction-position changes and native WebSocket history copies.
Its rejected embedding-overwrite concern has been removed; prior chunks are
persisted as separate attempts. Cache-scanning recommendations now preserve
unknown legitimate controls, and scope/TTL recommendations include subscription
backend caveats. Fingerprint fragmentation, production prevalence and all new
performance/savings estimates remain unmeasured.

Two reviewer suggestions were narrowed further during reconciliation: aggregate
read/write counters can identify candidates for wasted writes but cannot prove
which exact prefix was reused; `numeric_metadata` is allowlisted and cannot accept
latency fields without validation/contract changes. Skipping `store:false`
continuation capture is a compatibility hypothesis, not a recommended quick fix.

No new software tests or benchmarks were run, and no code, configuration or
services were changed. Only this research report and the independent review were
added to the checkout.
