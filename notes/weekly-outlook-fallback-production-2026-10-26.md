# Current quota during forecast learning — 2026-10-26

Deployed the requested current-usage fallback to https://10.10.0.235:8317/.

- Release: `0.9.0+gui.2ff1b29e4f76`.
- Binary SHA-256: `c713b17355bb04db220593b8f2d345c2adfcc46ce9fb57b1b2d567178fda28bb`.
- Source archive SHA-256: `2ff1b29e4f76714aee1659736f66a725ffebd9b95c4049129eeed90fa3112c98`.
- Authorized working-tree snapshot based on `d24089ece47fd5e489e105affe6e8c2a768b44bc`.
- Durable updater transaction: `3e2557e6821b4121a639f8ca958f2508`; committed, admission released.
- Rollback release: `0.9.0+gui.41c9960b1e7f`.

While learning, the combined bar shows actual current quota with **NOW** and
**used now** or **remaining now**, following the subscription display toggle.
The learning status and original estimate time remain visible. When enough
history is available, it automatically switches to the lowest projected reserve.
Unknown, incomplete or stale balances are not invented.

Verified production values were 64.25% used / 35.75% remaining. All four Claude
accounts retained their history across the restart, with the original learning
deadline of 17:36 Europe/Amsterdam. No production history was seeded or reset.

All 121 dashboard tests, desktop/mobile layout checks and 13 updater wrapper
tests passed. The exact release passed startup, all ten asset checks, database
compatibility and a local provider fixture that verified learning, a ready
forecast and restart persistence. Rust source is unchanged from the preceding
release, whose 724 Rust tests, formatting and Clippy passed. No real provider
test requests were made. There was no existing local development service;
the rebuilt candidate was started and restarted under a managed lease.

Production health, readiness, TLS, exact binary/assets, quota-history persistence,
configuration, accounts, routing and usage database preservation all passed.
The activation unit was removed. Lease `c3104cb2e75548969207c5cc38393b32`
is closed with no remaining resources, volumes or cleanup errors.

Release, rollback and stopped-writer backups are retained under the existing
production operator. Deployment records and ownership are in
`/opt/cli-proxy-api-rust/incoming/fusebox-weekly-outlook-fallback-20261026/`; backups are in
`/var/lib/cliproxy-rust-update/backups/3e2557e6821b4121a639f8ca958f2508/`.
Local receipts are in `target/weekly-outlook-fallback-production-20261026/`.
The pre-existing credits research note was left untouched and excluded.
