# Usage capture and coverage production deployment, 30 October 2026

Deployed `0.11.1+gui.77b25555372f` using the existing explicit force authorization.
The durable updater committed transaction `6c571800a47446bf9d988f19880ea50d`.
Production is ready, all five accounts are ready, and the usage writer is healthy.

The fix retains reported usage from every complete SSE event already received
before yielding frames to the client. Previously a disconnect during an earlier
frame could discard usage later in the same received batch. The regression failed
before the fix and now passes for Responses, Anthropic, Chat Completions and Gemini.
Cancellation remains HTTP 499 and still stops upstream work promptly.

The dashboard now explains why costs are unknown. At 09:28:16 UTC, its retained
history showed 6,358 entries with no recorded token categories and five unresolved
`codex-auto-review` entries. The initial audit had 6,356 missing-usage entries,
including 5,868 cancellations; two additional missing-usage entries arrived before
the rollout. The other missing-usage entries are failures or superseded attempts.
No successful native HTTP response was missing a price in that audit. The five
internal-model records do not identify a publicly priced underlying model.

Historical unknown costs remain unknown. The stopped-writer full refresh changed
zero prices and its second run also changed zero. All 485,946 observations at
the maintenance boundary, their payload/fingerprint hash, token totals, prices
and revision counts were identical before and after. Dashboard rollups matched
canonical entries and SQLite checks passed. In the original fixed research window,
313,641 observations, 6,186 missing-usage entries and the $28,106.61 known estimate
were unchanged. The UI explains that unknown costs are excluded from estimates;
it does not assign them zero or another model's rate.

Validation passed 768 Rust tests (5 ignored), 145 dashboard tests, formatting,
Clippy and 16 rollout wrapper tests. The exact release passed restart and previous
binary reopen checks, pricing tests and a private production-copy rehearsal. The
rehearsed dashboard query took 22 ms. Public TLS and all ten production dashboard
assets matched the qualified release. Verification made no provider inference calls.

All settings, account identities, configured import/collector identities and 197
routing assignments were preserved. The force step drained 13 connections in
71 ms. The gate still has its original PID 93357 and zero restarts; no maintenance
marker or network hold remains. The rollout unit exited successfully and is
inactive. The only schema addition is a partial index on unpriced entry timestamps;
the schema remains version 5 and the previous release can reopen it.

The established production operator retains the stopped-writer backups at
`/var/lib/cliproxy-rust-update/backups/6c571800a47446bf9d988f19880ea50d`, the previous
`0.11.1+gui.05e7012634b1` release, exact source/archive and the inactive recovery
unit. Both qualification leases are closed and empty. The private local rehearsal
SQLite snapshot and its two sidecars were removed (1,890,545,664 bytes). There is
no standing dev service for this repository; the rebuilt candidate was started,
restarted and stopped during managed qualification.

Additional historical recovery requires matching original usage evidence. The
thebeast histories did not provide matching completed-response records. The Dragon
and XPS histories have not been inspected because reachable SSH addresses have
not been supplied; recovery from those machines is not claimed.

See the [sanitized deployment receipt](usage-coverage-production-2026-10-30.json)
and [initial audit](usage-unpriced-2026-10-30.md). Private qualification and rollout
receipts are under `target/usage-coverage-production-20261030/`.
