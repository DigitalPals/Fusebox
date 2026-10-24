# Provider identity hardening production deployment — 2026-10-24

Deployed at the operator's explicit request: “Apply this live on production so I can test it. Force deployment.”

- Production: https://10.10.0.235:8317/
- Checked: 2026-10-24T08:28:29.428728+00:00.
- New release: `0.8.1+gui.fd2ebf3cf4ae`.
- Binary SHA-256: `a57868a44bcf8c0a9cb0bd542684f739a71110bf2389f50a062f06c7b62b8716`.
- Source: full working-tree snapshot based on `0002d85c7e1714ddd2e84a3d6611d3eeca5f19b0`, including the new session module and dashboard changes. The snapshot was uncommitted at deployment time.
- Source archive SHA-256: `fd2ebf3cf4aea3c1e2493dbb491de4f577da0c585f6c81e21a964eea0b0aad0c`.
- Rollback release: `0.8.1+gui.faba37631a37`.
- Durable updater transaction: `c9c9a4d8cfdb4069ad3730add989cb07`; phase `committed`, admission released.

## Activation and verification

Built a release with Rust 1.96.1. The exact candidate passed isolated startup, dashboard asset, new identity inspection, settings and usage-schema checks. The previous release reopened the candidate database successfully. The adapted deployment wrapper passed 13 isolated tests. The implementation checks remain recorded in [the implementation verification](provider-identity-verification-2026-10-24.md).

The staged candidate validated the actual production configuration as the service user. Forced activation used the established durable updater, a bounded SYN hold with independent expiry, connection resets, stopped-writer backups and recovery on process exit. Active streams were interrupted and clients could reconnect after admission was released.

Postflight verified the exact running binary, active backend, zero automatic restarts, public HTTPS certificate validation, readiness, all ten dashboard assets in identity/gzip representations, authenticated model listing (23 models), PROXY protocol listener and the identity inspection API. All 5 configured accounts and existing config/routing settings were preserved. Readiness reported 4 of 5 accounts ready, matching the pre-deployment baseline.

Usage remained healthy, with no dropped/rejected entries, writer errors or unclean prior writer sessions. The database passed `quick_check`; schema 5 and its schema digest were unchanged. Observation count advanced from 415735 in the stopped-writer backup to 415737 in the live snapshot. Existing imports and collector configuration were unchanged.

The new provider-session idle default is 1800 seconds. Restart starts fresh provider session state; the implemented A → B → A behavior issues a fresh provider session on each switch. Existing local history remains available under the documented protocol behavior. Identity learning remains opt-in per named client.

No synthetic provider inference calls were made during deployment verification. Live Claude/Codex generation is left for the operator's requested testing. Service health checks do not establish provider acceptance or invisibility.

## Subsequent real-traffic check

At 2026-10-24T08:31:06Z, all 19 completed requests since restart had succeeded:
9 Claude requests and 10 Codex requests. There were no failures, retries,
cancellations, missing/partial usage records or automatic service restarts.
Providers reported 13,166 output tokens and 2,619,056 cache-read tokens. The
service journal contained no warning/error lines in the checked window.

One Claude subscription remained quota-limited as before deployment. The
observed traffic used HTTP and did not switch subscriptions, so native
WebSockets and account switching were not validated by this short live sample.
The sanitized snapshot is retained in the ignored deployment artifacts as
`live-usage-check.json`; no synthetic inference requests were sent.

## Retained rollback and cleanup

The candidate and prior release, exact source archive, deployment scripts/pins, verification receipts and stopped-writer backups remain on production under the established operator ownership record.

- Backup directory: `/var/lib/cliproxy-rust-update/backups/c9c9a4d8cfdb4069ad3730add989cb07/`.
- Core backup SHA-256: `c56c4ee0fb8dadfcee080066f0ebe2582c5ed72778478862f6034be0882417a5`.
- Usage archive SHA-256: `a85c2ff3b187c8cc97cbd6b34c5c49f68dc438bd0c7c4f5422cf18e2ec67d900`.
- Production deployment artifacts: `/opt/cli-proxy-api-rust/incoming/fusebox-provider-identity-20261024/`.
- Local source/build/pins and sanitized receipts: `target/provider-identity-production-20261024/` (ignored).

The one-release activation unit was removed after success. The temporary firewall table and expiry units are absent; the gate is not held. Local supervised fixture lease `329684b8a878450cbcdf117591abf20a` is closed, with no remaining resources, retained volumes or cleanup errors. No unrelated services or resources were changed or removed.
