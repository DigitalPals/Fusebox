# Efficiency production rollout, 15 October 2026

The implemented improvements are live at <https://10.10.0.235:8317/> as
`0.4.0+gui.32c54c271e0c`. The guarded updater committed the release at
12:11 CEST and reopened admission. Existing configuration and cache defaults
were preserved.

- All four accounts are ready; the authenticated model list returns 23 models.
- All 16 unexpired session assignments and account identities survived restart.
- Production and the stopped-writer backup both pass SQLite `quick_check`.
  Schema version remains 3 and the schema digest is unchanged. All 272,739
  observations present at backup were retained; three ordinary observations
  had committed when postflight verification ran. There were no drops,
  rejections, writer errors, unclosed sessions, or automatic service restarts.
- Ten dashboard text assets match the source over public, certificate-verified
  HTTPS in both identity and gzip representations. Their combined payload is
  567,899 bytes uncompressed and 161,518 bytes with gzip, a 71.6% reduction.
  Representation-specific ETags, `Vary`, and conditional 304 responses passed.
- Rollout verification made no billable inference requests. The exact release
  also passed isolated dashboard/default-settings checks, 12 real-process usage
  acceptance checks, and 11 rollout-helper checks before activation.

The executable is 15,783,216 bytes. SHA-256:
`a2c1bc049e5b6effbb6cdeeae66329918a3faf403bcdd31bfcb9cfb83eb19688`.
Its source snapshot is based on `bb86ecaba0d4556365c124db938038a1b1b84b2d`
plus the verified working changes. All 103 application/build/asset inputs match
the working implementation. Only the isolated Cargo package/lock version stamp
changes for the installed updater's release convention. Snapshot SHA-256:
`32c54c271e0caea33bc4c1eb0c0095e9fc153af1618bd5a71bfc08ef708fa5aa`.

The retained rollback release is `0.4.0+gui.8cba6279c9bd`. Configuration,
credentials, routing state, TLS configuration and a stopped-writer usage snapshot
were backed up under transaction `ada49af47ebe48229e297cd0935db713` in
`/var/lib/cliproxy-rust-update/backups/`. The deployment scripts and private
receipts are under
`/opt/cli-proxy-api-rust/incoming/fusebox-efficiency-20261015/`.
The deployment unit includes recovery on exit; its `maintenance.py recover`
reconciles this transaction without initiating a new deployment. Automatic
rollback was armed during activation and was not needed.

Refresh the dashboard to load the new controls. Prompt-cache scope remains
`session`, namespace is empty, and incompatible controls use `drop`. Shared
client/workspace scopes and admission limits are available as opt-in settings.
Request details expose requested/applied cache policy and downgrade reasons.
The measured streaming gain concerns unusually large fragmented events;
ordinary provider response times are not expected to change materially.

Sanitized machine-readable proof is in
[the deployment receipt](efficiency-production-2026-10-15.json). Full local
build, backup verification and public HTTPS receipts are retained under
`target/efficiency-production-20261015/`.
