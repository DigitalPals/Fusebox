# Prompt caching review implementation — 2026-10-21

Implemented the actionable findings from Astra's research and Fable's review,
checked against the official OpenAI [prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching)
and [diagnostics guide](https://developers.openai.com/api/docs/guides/prompt-caching/diagnostics).

| Review finding | Result |
| --- | --- |
| Twelve-hour writes used the thirty-minute price | Capture independent lifetime counts and price 30-minute writes at 1.25× input and 12-hour writes at 2.5×, including context, tier and region adjustments. |
| Translation lost breakpoint TTL overrides | Preserve complete per-block breakpoint metadata, original OpenAI instruction roles, and compatible request-level cache controls. |
| Translation lost comparison baselines | Forward `comparison_response_id` to Responses; report a targeted downgrade for Chat while retaining other compatible options. |
| Provider cache diagnostics were invisible | Show sanitized provider results, miss reasons and counts separately from requested/applied Fusebox policy in Requests and structured logs. Never retain comparison identifiers or arbitrary diagnostic text there. |
| Merging messages moved implicit cache boundaries | Preserve message endings for OpenAI destinations. Apply required turn/tool grouping at Claude, Gemini, Chat and Devin builders; preserve signed thinking with its tool turn. |
| Inferred assignments expired after an obsolete one-hour cap | Use the configured session affinity idle timeout for inferred and explicit assignments, while preserving active-call protection, bounded storage and inferred-first eviction. |
| Key and endpoint guidance needed qualification | Document older-model routing keys versus newer-model cache accounting, keep caller precedence and existing key derivation, and separate account affinity from cache lifetime. |

Usage lifetime fields flow through full responses, streaming translation, durable
observations, dashboard rollups, JSON/CSV export and the Usage UI. Lifetime buckets
are subsets of aggregate writes. Corrected streaming breakdowns replace prior
breakdowns atomically; missing values remain unknown. Duplicate observations
prefer richer OpenAI lifetime evidence after primary usage completeness.

The usage database migrates transactionally to schema 5. Existing serialized
observations and fingerprints remain stable when the new fields are absent.
Recoverable historical prices are corrected with previous snapshots retained in
revisions. Old records without lifetime evidence retain their historical estimate;
their possible underpricing cannot be repaired from a request default. New positive
writes with insufficient lifetime evidence remain unpriced. Legacy generic-only
custom rate cards retain their configured behavior.

Downgrade refuses before mutation when any observation contains the new lifetime
fields, including explicit zeroes. Legacy-only databases can downgrade, omitting
unsupported rates from the active legacy catalogue and retaining the original for
audit. See [usage costs and recovery](usage-costs.md).

Public Responses API documentation establishes neither Chat Completions support
nor ChatGPT OAuth backend acceptance. Compatible translation preserves supplied
hints; no new caching policy is enabled automatically. Inferred sessions can still
collide when their first user message is identical; explicit session IDs distinguish
them. No live provider cache-hit or performance claim follows from the mock tests.

Validation covers lifetime arithmetic and malformed snapshots, pricing, migration,
deduplication, repricing audits, downgrade refusal, message boundary/role stability,
tool/thinking ordering, diagnostic privacy and retry isolation. The isolated runtime
record is [prompt-cache-dev-validation-2026-10-21.json](prompt-cache-dev-validation-2026-10-21.json).

Checks passed with Rust 1.96.1: `cargo test --locked` (619 passed, one existing
ignored measurement), `cargo clippy --all-targets -- -D warnings`, and
`cargo fmt --check`. Frontend tests passed 89/89 and Python tests passed 33/33.
The shared CI controller has no Fusebox job, so repository checks ran locally
under a capacity reservation. The collaborative browser returned
`Authentication required`; UI validation used the repository's Node suites.
The rebuilt release binary passed 22 isolated runtime checks, including a real
restart on the same configuration, database and port. Each synthetic example
cost 30,100,000 nanodollars before and after restart. No paid provider calls were
made, and the temporary services were stopped.
