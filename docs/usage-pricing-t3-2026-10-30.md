# T3-compatible pricing, 30 October 2026

The reconciliation found a real dated-model lookup failure and a separate
calculation mismatch. `claude-haiku-4-5-20251001` was missing from Fusebox's
catalogue. Codex Fast requests could return `service_tier: default`, so Fusebox
charged standard rates while T3 estimated the requested Fast speed.

## Implementation

The default catalogue is now `t3-2026-10-30.1`. It covers 44 identifiers in 30
rate groups. Its T3 rows come from the LiteLLM cache fetched on 2026-10-29;
existing published rates remain for five supported identifiers absent from
that table, with explicit fallback provenance. `scripts/update-usage-pricing.py`
rebuilds this release snapshot without adding a runtime network dependency.

T3-style estimates sum uncached input, cache reads, aggregate cache creation
and inclusive output. Requested Codex speed chooses Standard, Fast or Ultrafast;
Claude uses reported speed. Missing speed rates fall back to Standard. Cache
rates absent from the source fall back to the input rate. Like T3, this
calculation ignores long-context, region, Batch/Flex and cache-lifetime premiums.
Claude Code's representable nonnegative `costUSD` takes precedence when present,
unless a local override is configured. Reasoning remains part of output.

HTTP, SSE, native WebSocket, retry and forked captures preserve requested speed
before translation. The raw response tier, lifetime subsets and final usage
revisions remain intact. The allowlisted numeric metadata key `requested_speed`
uses stable values: 1 Standard, 2 Fast, 3 Ultrafast. `reported_cost_nanos` retains
Claude Code's optional reported cost. Upgrade the ingestion server before
collectors that send the new metadata; older allowlists reject new keys.

The confirmed Sol example now prices at **$0.055556**, versus **$0.027778** for
the same tokens at Standard. This changes estimates used by monthly client
budgets and subscription-attribution weighting too. Provider quota percentages
continue to come from providers.

Routine startup repairs recoverable unpriced records, including the dated
Haiku alias and aggregate cache writes that T3 can price without a lifetime.
It does not rewrite established prices except previously documented corrections.
An explicit full refresh recalculates older bundled snapshots, with each prior
snapshot retained in `usage_price_revisions`. Raw observations and fingerprints
are unchanged, rollups are rebuilt transactionally, and refresh is idempotent.
Established local overrides and any established price that a new catalogue
cannot calculate are retained.

Catalogues without `calculation` retain the legacy provider calculation. The
previous provider rate card remains in `src/usage/rates.json`; the new default
is `src/usage/rates-t3.json`. Rollback conversion removes the new calculation
field from the active compatibility catalogue and preserves the full original
catalogue for audit.

## Historical limits

Of the 6,259 unpriced accounting entries in the fixed research window, 73 used
the dated Haiku identifier; 69 successful entries had complete token categories.
The production refresh recovered all 73 dated Haiku records in that window and
81 across retained history. Their partial records had enough aggregate categories
for T3's calculation. The other unpriced records mostly lacked usage after client
disconnects. Missing usage stays unknown rather than zero.

Old proxy records that discarded requested speed cannot reveal whether a
returned `default` response was requested as Fast. They use the returned tier
with `requested_speed_unavailable_returned_tier_used` in their price assumptions.
Native imports can retain requested settings per turn; keeper imports now retain
requested tier separately. Repricing alone cannot recover discarded settings,
fill the October 3–6 capture gap, or make differently scoped T3 and Fusebox views
identical. Fusebox keeps final Claude usage revisions, including output counts
that T3's first-revision selection understated.

## Rollout

Applied to production with explicit force authorization on 2026-10-30 as
`0.11.1+gui.05e7012634b1`. The stopped-writer backup was verified, 478,340 saved
price snapshots were refreshed, 81 unpriced records were recovered, and a second
full refresh made zero changes. Production is healthy; settings, accounts,
sessions and usage evidence were preserved. See the
[production deployment record](usage-pricing-production-2026-10-30.md).

For future operator refreshes, stop the established writer and take a consistent
backup before running under the service account:

```sh
fusebox usage reprice --database /var/lib/cli-proxy-api/usage/usage.sqlite3
# Refresh established bundled estimates as well:
fusebox usage reprice --all --database /var/lib/cli-proxy-api/usage/usage.sqlite3
```

Run under the service account with its existing file permissions. The commands
preserve saved retention and organization overrides. Start the established
service and verify `/readyz`, usage writer health, the displayed catalogue,
and the revised totals. Allow disk space for saved price revisions. A second
full refresh should report zero changes. For an older binary, use the existing
`usage rollback-copy` procedure and retain the full original database.

## Provenance and validation

Reference implementation: installed T3 Code 0.1.3 `usagePricing.js`.
Binary SHA-256:
`e2b8d466d223f0993b31d9a6dafd22bb06558cc1a119a9f932d7b5d363f09b74`.
Cached LiteLLM input SHA-256:
`95347f7fbf9e3011d8f7ecb57080490007dcfdd13a2c104e56adf3deb3aaca1b`.
Rate source:
<https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json>.

Eighteen golden examples in `tests/fixtures/usage/t3-pricing.json` were evaluated
by the installed T3 implementation. They cover the confirmed Fast mismatch,
Ultrafast, missing speed/cache rates, dated Haiku, long contexts, region/lifetime
independence, inclusive reasoning, reported cost, unknown and synthetic models.
Rust regression tests cover real HTTP/SSE/WebSocket capture, imported settings,
price repair, audited refresh, override preservation, rollback, and exact rollups.
The CI controller on this host has no Fusebox project, so repository checks run
directly with Rust 1.96.1. Dev process validation uses an expiring managed lease
and synthetic upstreams.

Validation passed: 766 Rust tests (5 ignored), 144 dashboard tests, 46 operational
tests, formatting, and Clippy with warnings denied. After the final Claude tier
and negative-cost edge fixes, 166 usage tests and all 18 golden examples passed.
The final optimized binary was rebuilt and its managed dev service restarted;
three pricing/restart checks and 12 collector acceptance checks passed. Both
managed leases are closed with no remaining resources or retained volumes. See
[the dev validation receipt](usage-pricing-dev-2026-10-30.json) for the binary hash
and checks. The exact production candidate additionally passed restart, rollback
reopen, and full historical refresh rehearsals before deployment.
