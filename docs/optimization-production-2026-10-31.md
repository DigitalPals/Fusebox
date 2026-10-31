# Optimization production deployment, 31 October 2026

The [validated optimizations](optimization-report.md) are live at
<https://10.10.0.235:8317/> as **`0.12.1+gui.8815bfae7fb7`**, built from commit
`146816ff387bd82e870e2b7143135feb2c079a14`. The user explicitly authorized a
forced production deployment. No additional push or release publication was made.

The deployed binary SHA-256 is
`86d48fca30c0251479034b644858ea998695a97f28868c591cc173251a20a93f`.
All 329 archived files matched the committed source; the build snapshot changed
only the Cargo package and lockfile version stamps. The 24 recorded validation
fingerprints were rechecked against the source that previously passed 796 Rust,
159 dashboard and 46 Python tests, formatting and all-target Clippy.

The exact packaged release additionally passed all ten embedded asset checks,
previous-release/candidate/restart/rollback database reopen checks, 15 deployment
wrapper tests, eight rebuilt-service smoke checks and all twelve collector
acceptance checks. The previous binary could reopen the candidate database;
schema version 5 and its complete schema were unchanged.

The durable updater committed transaction
`eccc523ee9b24b2d9b289ad6021aed7f`. The forced drain disconnected three open
connections and acquired admission in 66 ms; this is drain time, not total
downtime. The backend stopped cleanly, state was backed up, the candidate passed
verification, and admission reopened. The rollout and recovery commands exited
successfully. The gate retained its original PID and has zero automatic restarts.
No temporary network hold or pending updater transaction remains.

Production verification passed:

- Public certificate validation, health/readiness and all ten assets in identity
  and gzip forms; internal checks also verified ETags and conditional 304s.
- The running executable matches the pinned binary and has zero automatic
  restarts. All five account identities, existing settings, configured
  collectors/imports and captured routing assignments were preserved.
- Usage writer health is healthy, with no dropped/rejected records, writer
  errors or unclosed writer sessions. SQLite quick checks passed for the
  stopped-writer backup and live database. Their schema hashes match; retained
  observations increased from 505,356 to 505,365 during verification.
- The served scripts contain shared date formatters and refresh batching with
  cancellation deadlines. Dashboard facets, combined observation reads,
  management authentication and the PROXY protocol listener passed.

Four of five account slots were ready both before and after deployment; overall
readiness passed. Qualification used synthetic providers. Production verification
made no provider inference calls, explicit model refreshes, reset requests,
repricing operations or test notifications. Browser rendering was not inspected.

The established production operator retains the previous
`0.12.1+gui.7f867d6cc5c4` release, exact source/package, recovery tooling and
stopped-writer backups at
`/var/lib/cliproxy-rust-update/backups/eccc523ee9b24b2d9b289ad6021aed7f`.
The incoming directory is
`/opt/cli-proxy-api-rust/incoming/fusebox-optimization-20261031`; its operator
record owns this retention. The one-shot rollout unit is static, inactive and
successful. Private local artifacts are under
`target/optimization-production-20261031/`.

Managed qualification lease `34e599e2e6e545bdbafe230f3b10ea5f` is closed, with
no remaining resources, preserved volumes or cleanup errors. The CI capacity
reservation was released. No temporary service remains; the exact rebuilt
candidate was started and restarted during qualification.

The [sanitized receipt](optimization-production-2026-10-31.json) records source
and binary identities, qualification, production verification and cleanup.
