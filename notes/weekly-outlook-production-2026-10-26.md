# Weekly quota outlook production deployment — 2026-10-26

Implemented and force-deployed at the operator's explicit request.

- Production: https://10.10.0.235:8317/
- Release: `0.9.0+gui.41c9960b1e7f`.
- Binary SHA-256: `28b18284353992fbd3640adc0d1afc7f99bcb5cb5ce953001046f0942389d266`.
- Source: authorized working-tree snapshot based on `d24089ece47fd5e489e105affe6e8c2a768b44bc`.
- Source archive SHA-256: `41c9960b1e7f1331347837eab36ce0b132528a7131f157cf1149f7caad4f7dd5`.
- Rollback release: `0.9.0+gui.8816e311dba7`.
- Updater transaction: `5e644b2fdbda4133bb0b535d4005e579`, committed; admission released.

## Behavior

**Weekly quota outlook** replaces the combined current quota/pace bar. It shows
the lowest projected remaining reserve over the next seven days, with the next
reset, expected depletion/recovery times when there is a gap, and explicit low
reserve, learning and unavailable states. Remaining reserve is independent of
the individual subscriptions' Used/Remaining toggle. Desktop/mobile layouts,
accessible meter labels and Chinese translations were updated.

The rate uses provider quota changes over up to the last 24 hours, with six hours
of continuous observations required. Bounded history survives weekly resets and
restarts in the existing identity-bound quota attribution journal. The first
observation is a baseline; stale/missing readings never imply zero demand or a
healthy forecast. Refills occur only at their actual times, and unused quota
expires. Five-hour/model limits and future banked resets remain outside this
weekly forecast. See [calculation and API](../docs/weekly-outlook.md).

All four live Claude weekly accounts reported quota and persisted the new
history. Initial state is **Learning usage**, with the first forecast expected
around **17:36 Europe/Amsterdam** on 26 October if readings stay continuous.
No artificial consumption history was seeded on production.

## Verification

- Rust: 724 passed; one existing ignored test. Formatting and Clippy passed.
- Dashboard: 119 passed; operational Python: 46 passed; updater wrapper: 13 passed.
- Forecast cases cover staggered/simultaneous resets, exhaustion before the next
  refill, unused quota expiry, the interval after the last reset, mixed weights,
  unknown/stale data, rounding noise, and bounded persistent consumption history.
- Headless desktop/mobile layout checks passed at widths 1440 and 390, including
  content bounds for every meter and all forecast states.
- Exact-release local provider fixture verified initial learning, a forecast with
  sufficient history, and its persistence across restart. Six local fixture calls;
  zero real provider test calls.
- Candidate startup/assets and database compatibility with the previous release
  passed in both directions. The rebuilt candidate was started and restarted;
  no existing local Fusebox development service or container was present.
- Forced production activation used the established durable updater with
  stopped-writer backups and independent recovery on exit.
- Exact running binary, public HTTPS certificate, all ten dashboard assets in
  identity/gzip representations, readiness, API forecast fields and saved quota
  history were verified. Zero automatic restarts; all five accounts and existing
  configuration/routing/import/collector settings were preserved.
- Usage storage remained healthy, with zero drops/rejections/writer errors or
  unclean prior writer sessions. Schema 5 and its digest were unchanged;
  quick-check passed. Observations advanced from 439,841
  in the stopped-writer backup to 439,847 during verification.

## Retention and cleanup

Production artifacts and the operator retention record are under
`/opt/cli-proxy-api-rust/incoming/fusebox-weekly-outlook-20261026/`.
The prior release and stopped-writer backups are retained under the existing
production operator. Backups are in
`/var/lib/cliproxy-rust-update/backups/5e644b2fdbda4133bb0b535d4005e579/`.
Local source/build/test/deployment receipts are under the ignored
`target/weekly-outlook-production-20261026/` directory.

The completed activation unit was removed and its absence verified. All eight
managed local leases are closed, with no remaining resources, retained volumes
or cleanup errors; CI host-capacity reservations were released. The unrelated
pre-existing `notes/credits-research-2026-10-22.md` was left untouched and excluded
from the source archive.
