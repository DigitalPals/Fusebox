# Banked reset notification production deployment, 1 November 2026

Banked reset receipt notifications for Claude and Codex are live at
<https://10.10.0.235:8317/> as **`0.12.1+gui.ba9e5be2f325`**, built from commit
`d137e784177180cd2f862b43a1e99d6322f4ba25`. The implementation was pushed to `main` before
release qualification. The user explicitly authorized the commit, push and
forced production deployment.

The deployed binary SHA-256 is
`eed0a8cc537ee2f06e8df4f73702da69b40ab41745dad1fa3459df08b17613a1`.
All 332 archived source files matched the committed workspace before upload;
the build snapshot changed only the Cargo package and lockfile version stamps.
Implementation validation passed 804 Rust tests (7 ignored), 159 dashboard tests
and 46 Python tests. The exact package additionally passed formatting, all-target
Clippy with warnings denied, 15 deployment-wrapper tests, all ten embedded asset
checks, previous/candidate/restart/rollback database reopen checks, eight rebuilt
service smoke checks and twelve collector acceptance checks. Usage schema 5 and
its full schema were unchanged, and the retained binary could reopen the database.

The durable updater committed transaction
`0861ec162b47495eac9bbb4755cfaee5`. The authorized forced drain disconnected
2 open connections and acquired admission in 67 ms;
this measures drain time, not total downtime. The backend stopped cleanly, state
was backed up, the candidate passed verification and admission reopened. The
rollout and recovery commands exited successfully. The gate retained its PID
and has zero automatic restarts. No network hold or pending transaction remains.

Production verification passed:

- Public certificate validation, health/readiness, all ten assets in identity
  and gzip forms, ETags and conditional 304s.
- The running executable matches the pin and has zero automatic restarts. All
  five account identities, settings, configured collectors/imports and captured
  routing assignments were preserved.
- The usage writer is healthy with no dropped/rejected records, writer errors
  or unclosed writer sessions. SQLite quick checks passed for the stopped-writer
  backup and live database. Both retain schema 5 and the same schema hash;
  observations increased from 509,243 to 509,245 during verification.
- All 3 pre-existing reset journals and their
  4 spending operations were preserved and included in the verified
  stopped-writer snapshot. No previously unresolved reset operation was present.
  The running candidate initialized 5 durable receipt baselines through its normal
  background polling. Existing balances establish a quiet initial baseline;
  future new grants or allocation increases use the existing notification switch.
- Banked resets and their notifications were already enabled and remain enabled.
  The updated English and Chinese notification settings text is served, management
  authentication is preserved, and the PROXY protocol listener is ready.

Four of five account slots were ready before and after deployment; overall
readiness passed. Qualification used synthetic providers. Production verification
made no inference calls, explicit provider refreshes, reset requests or test
notifications. Actual notification delivery for a future provider grant was not
forced in production. Browser rendering was not inspected.

The established production operator retains the previous
`0.12.1+gui.8815bfae7fb7` release, exact source/package, recovery tooling and
stopped-writer backups at
`/var/lib/cliproxy-rust-update/backups/0861ec162b47495eac9bbb4755cfaee5`.
The incoming directory is
`/opt/cli-proxy-api-rust/incoming/fusebox-reset-notifications-20261101`; its operator
record owns this retention. Rollback switches binaries and preserves the live
usage database and banked reset journal; an older journal must not overwrite
newer spending history. The one-shot rollout unit is static, inactive and successful.
Private local artifacts are under
`target/banked-reset-notifications-production-20261101/`.

Managed qualification lease `fcae216a31e64589b903291eb651beed` is closed with
no remaining resources, preserved volumes or cleanup errors. The CI capacity
reservation was released. The exact rebuilt candidate was started and restarted
during qualification; no temporary service remains.

The [sanitized receipt](banked-reset-notifications-production-2026-11-01.json)
records source and binary identities, qualification, production verification,
retention and cleanup.
