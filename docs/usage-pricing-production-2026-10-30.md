# T3 pricing production deployment, 30 October 2026

Deployed `0.11.1+gui.05e7012634b1` under the user's explicit “Apply to production,
force it” authorization. Catalogue `t3-2026-10-30.1` is active. The durable
updater committed transaction `2800a2ce55364627b3698974bb149745`; the backend
restarted successfully and the admission gate was released. Its original
process remained running.

The refresh updated **478,340 price snapshots** and recovered **81 previously
unpriced dated Haiku records**. Every previous snapshot remains in the price
revision audit. The stopped refresh preserved all 484,701 observations, their
payloads and fingerprints, and every token total. A second full refresh changed
zero rows. The first refresh and its accounting checks took 49 seconds.

For the original fixed research window, all 73 dated Haiku records are now priced;
6,186 records still lack usage. Its known estimate is now $28,106.61. Across
all retained history at the maintenance boundary, the estimate is $49,794.39,
with 6,361 records still unpriced. Applying T3's flat category rules also removes
some previous premiums, so recovered records do not imply a higher total.
These figures are estimates, not subscription charges.

Live evidence confirms the requested-speed fix: a new GPT-6.1 Sol request with
Fast selected returned `default`, and its saved estimate correctly uses `fast`
with a 2× multiplier. A Standard request returning `default` uses 1×. This check
observed real client traffic and made no provider inference requests.

Production readiness and usage writer health passed, with zero dropped or
rejected records, writer errors, or unclosed prior sessions. SQLite checks passed;
the schema remains 5 and dashboard rollups agree with canonical entries. All five
accounts, all settings, and 189 routing assignments were preserved. Public TLS,
the ten dashboard assets, and the PROXY protocol backend passed verification.
The force step drained 11 existing connections in 70 ms. No network hold or
maintenance marker remains; the rollout unit exited successfully and is inactive.

The production operator retains the stopped-writer backups at
`/var/lib/cliproxy-rust-update/backups/2800a2ce55364627b3698974bb149745`,
the previous `0.11.0+gui.eb129cf97dfb` release, the pinned source/archive, and the
inactive rollout/recovery unit. The release records its uncommitted source
snapshot, including its base commit and diff hash. The exact candidate passed
restart, previous-release reopen, and full production-copy refresh rehearsals
before activation. Temporary qualification resources and rehearsal databases
were removed; the managed lease is closed.

Historical missing tokens, the October 3–6 capture gap, and requested speed
discarded by older captures remain unavailable. Repricing cannot reconstruct
them. Fusebox also retains final Claude usage revisions that T3's first-record
selection can undercount.

See the [sanitized deployment receipt](usage-pricing-production-2026-10-30.json)
and [implementation notes](usage-pricing-t3-2026-10-30.md). Private deployment
artifacts are under `target/usage-pricing-production-20261030/`; production's
operator record is in the corresponding protected incoming directory.
