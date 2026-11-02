# Bugfix production deployment, 2 November 2026

The eight verified reliability fixes are live at <https://10.10.0.235:8317/> as
**`0.12.1+gui.fec831f0eb2f`**, built from clean commit
`b8ac02702bbcb0597098dfaca0b2049365a41924`. The user explicitly requested production application
and forced deployment. No release publication or additional push was made.

Binary SHA-256: `0d9822c39ab25dfc9cecef8df4f33d3085a27c61f2665bac97a22da2db37f335`.
All 472 archived files matched the committed workspace. Only the Cargo package
and lockfile version stamps changed in the build snapshot. The implementation
had passed 824 Rust tests (7 unchanged ignored manual benchmarks), 167 dashboard
tests, 46 Python tests, formatting and all-target Clippy. The exact versioned
package additionally passed formatting/Clippy, 15 deployment-wrapper tests, all
ten embedded asset checks, previous/candidate/restart/rollback database reopen
checks, eight dev smoke checks and twelve collector acceptance checks. Usage
schema 5 and its full schema were unchanged; the previous binary could reopen
the database after the candidate ran.

The durable updater committed transaction `45b45d323c094079ae39784cb66c2ed5`.
The authorized forced drain disconnected 7 open connections and acquired
admission in 72 ms; that measures drain time, not total downtime. The backend
stopped cleanly, state and usage were backed up, the candidate passed verification,
and admission reopened. The rollout and recovery commands exited successfully.
The gate retained its PID. No pending release or network hold remains.

Production postflight verified:

- Exact running binary; active backend with zero automatic restarts.
- Public TLS certificate validation, health/readiness and all ten assets in
  identity/gzip forms. Internal checks also verified ETags and conditional 304s.
- All five accounts, all 150 captured routing assignments, configuration, settings,
  collectors/imports and three push devices preserved. The corrected request-log
  snapshot code is served and the bounded log endpoint is readable.
- Usage writer healthy, with zero dropped/rejected observations, writer errors or
  unclosed sessions. SQLite quick checks passed for the stopped-writer backup and
  live database. Both retain schema 5 and the same schema hash; observations
  increased from 516,333 to 516,346 during verification.
- Management authentication, client identity inspection and the PROXY protocol
  listener remain valid. Notification settings and existing APNs configuration
  were preserved.

Readiness was four of five accounts before and after deployment. Qualification
used synthetic providers. Production verification made no inference calls,
explicit provider refreshes, account-control mutations or test notifications.
The separate iPhone lifecycle concern remains unverified; this deploy does not
change the native app. No browser rendering check was made.

The established production operator retains `0.12.1+gui.8aec89d5895c` for
rollback, plus the source/package, recovery tooling and stopped-writer backups at
`/var/lib/cliproxy-rust-update/backups/45b45d323c094079ae39784cb66c2ed5`.
An explicit owner/retention record is in
`/opt/cli-proxy-api-rust/incoming/fusebox-bugfix-20261102/operator-record.json`.
The one-shot rollout unit is static, inactive and successful. Rollback preserves
the current usage database and banked reset spending journal.

Managed qualification lease `841d4011ec174848a7efab737c808f9d` is closed with no remaining
resources, preserved volumes or cleanup errors. The CI capacity reservation was
released. The exact rebuilt candidate was started/restarted during qualification;
no temporary development service remains. Private artifacts and receipts are
under `target/bugfix-production-20261102/`.

See the [sanitized deployment receipt](bugfix-production-2026-11-02.json) and
[bugfix investigation log](../BUGFIX_PROGRESS.md).
