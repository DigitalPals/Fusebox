# Remaining unknown costs, 30 October 2026

The follow-up production audit found 6,361 unpriced accounting entries. Of these,
6,356 have no token categories at all: 5,868 are client cancellations and the rest
are failed or superseded attempts. No successful native HTTP response is missing
a price. Five older keeper records have tokens but name the internal model
`codex-auto-review`; neither a public price nor its underlying model is recorded.
Assigning these records zero or another model's price would invent evidence.

A regression reproduced an ongoing capture problem in HTTP SSE passthrough:
several events can arrive in one upstream chunk, but the generator used to tap
one event at a time as the client consumed them. Dropping the response during
an early event discarded usage later in the chunk, even though it had arrived.
The fix taps and parses the complete received batch before yielding frames and
merges its reported usage. It preserves a 499 cancellation outcome if the client
does not reach the terminal event and cancels upstream work promptly. It does
not continue generation after a disconnect or fabricate unreported tokens.

Dashboard coverage now separates missing usage, incomplete usage, invalid usage,
unknown models, and unsupported pricing. It keeps the total unknown-cost count
and all historical costs unchanged. The breakdown shows “missing usage” for
records like those in the screenshot, and “no model rate” for an unresolved
model with sufficient tokens. An explanatory note states that unknown costs are
excluded from estimates. Provider-reported prices with missing tokens are not
incorrectly classified as unpriced.

The reason query reads only scalar columns on unpriced canonical entries, using
a partial timestamp index. It applies the same filters and global source
deduplication as the dashboard totals. The index is compatible with schema 5;
legacy rollback conversion removes it. The historical observations, fingerprints,
price revisions, and dollar totals are preserved.

The disconnect regression failed before the fix and passes for Responses,
Anthropic, Chat Completions, and Gemini. The complete suite passed 768 Rust tests
(5 ignored) and 145 dashboard tests, including prompt cancellation, source
deduplication, rollup equality, unknown-cost handling and rollback conversion.
See the [audit receipt](usage-unpriced-2026-10-30.json). The fix is now deployed
as `0.11.1+gui.77b25555372f`; the [production verification](usage-coverage-production-2026-10-30.md)
confirms healthy capture, preserved historical prices and evidence, and the new
reason counts on the production dashboard. At the final live check there were
6,358 missing-usage entries and five unresolved internal-model entries. The two
additional missing-usage entries arrived between the original audit and rollout.
The refresh priced zero additional historical rows because none had gained
the missing evidence.
