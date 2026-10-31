# Optimization production deployment, 29 October 2026

The full-stack optimizations and Claude-review follow-up fixes are live at
<https://10.10.0.235:8317/> as **`0.11.0+gui.eb129cf97dfb`**, built from local
commit `faaa97c97130938338194555dc2b76a21e8a33b3`. The user explicitly authorized
forced production deployment. No repository push was performed.

The exact deployed binary SHA-256 is
`75bc2d385352d0ca5e01c2f43b78dadc7941aa09f874553f6389eeb291aeb8f6`.
The committed source archive was checked independently against all 298 files;
the private build source differs only in the Cargo package/lock version stamp.

## Qualification and activation

The source passed 759 Rust, 144 dashboard and 46 Python tests, formatting and
all-target Clippy with warnings denied. The exact packaged candidate additionally
passed four restart/rollback-compatibility runs, all 10 embedded asset checks,
the authenticated empty-account model-check route, PROXY protocol checks and
12 collector acceptance checks. All 15 deployment-helper tests passed.

The installed durable updater committed transaction
`786fcb1e60e948fa915a616938ae0505`. The forced drain succeeded on its first attempt:
10 active connections were disconnected and the gate drained in 69 ms. This is
the drain time, not total service downtime. The writer stopped cleanly, state
was backed up, the candidate was verified and admission reopened. Both the
deployment and recovery commands completed successfully; the temporary firewall
hold is absent.

## Production verification

- HTTPS certificate verification, health and readiness passed. All 10 public
  assets match the release in both identity and gzip representations.
- The running process matches the pinned binary. The service and admission gate
  are active, with zero automatic restarts; the gate process was unchanged.
- Existing configuration/settings, all 5 accounts and all 207 captured routing
  session assignments were preserved. Collector/import identities were unchanged.
- Usage health is healthy, with zero dropped/rejected records, writer errors or
  unclosed prior writer sessions. SQLite integrity checks passed, schema 5 and
  its schema hash are unchanged, and observation count increased from 474,072 in
  the stopped-writer backup to 474,076 during postflight.
- Live assets include the overlapping-discovery message and Chinese translation,
  formatter reuse and model-family coverage optimization.
- Deployment verification made no real-provider inference calls, explicit model
  refresh requests, reset requests or test notifications. Live provider discovery
  and Claude OAuth cloaking remain outside the end-to-end test coverage.

The prior release `0.10.1+gui.c7dcf3a00824`, the committed source, candidate,
deployment receipts and stopped-writer backups remain under the existing
production operator for recovery and rollback. The backup directory is
`/var/lib/cliproxy-rust-update/backups/786fcb1e60e948fa915a616938ae0505`.
The disabled, inactive deployment/recovery unit is retained with its operator
record under the established deployment mechanism.

The managed qualification lease `473973c2ed9a4e85a6630f3f7f7f50be` is closed with
no remaining resources; the CI host reservation was released. No persistent
local Fusebox dev service existed; the exact release was rebuilt and restarted
under that managed qualification lease.

The [sanitized deployment receipt](optimization-followup-production-2026-10-29.json)
contains artifact hashes, qualification results and production evidence. The
[Claude review and follow-up](optimization-opus-review-2026-10-29.md) records the
minor findings and fixes; Claude did not perform a second review of those fixes.
