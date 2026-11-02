# Bugfix investigation

## Scope and baseline (2026-11-01)

- Started at `799103f` (`main`); working tree was clean. No repository or ancestor
  `AGENTS.md` was present; followed the supplied global project instructions.
- Read README development/architecture/routing guidance, PRODUCT.md, DESIGN.md,
  SECURITY.md, CI configuration, and the host's agent resource policy.
- Architecture: Rust/Axum proxy; per-account routing, retries and format adapters;
  SQLite usage history; management API and embedded vanilla-JS dashboard; native
  Swift iPhone client, widgets and push relay support.
- CI controller does not offer a Fusebox job. Use documented local checks with
  `cargo +stable` (1.96.1; host default 1.85 is below the declared Rust 1.88 minimum),
  bounded build parallelism, and managed disposable resources for service smoke tests.
- No standing Fusebox system/user development service or listener was found.
  Do not touch production. Rebuild and exercise/restart a disposable candidate at handoff.
- Baseline: formatting passed; Rust passed (815 tests, 7 ignored); dashboard
  passed (161 tests); Python passed (46 tests).

## Reviewed areas

- Initial map: server routes and authentication, application state/config reload,
  routing/retry documentation, validation tooling and existing regression suites.
- Traced HTTP inference/model-group retries and SSE bootstrap/completion, Responses
  continuation scoping, media generation/compaction retries, admission and compressed
  bodies, named-client policy, WebSocket selection and account identity invalidation.
- Traced credential refresh/publication, account enable/priority/removal, hot reload,
  push relay registration/delivery, and dashboard live/HTTP request-log refreshes.
- Sampled usage capture, SQLite writer/read contracts, collector progress and iPhone
  connection/push/store lifecycle. Existing deep usage/format suites cover much of
  this; no claim of exhaustive review or real-provider/iOS runtime validation.

## Confirmed findings / completed fixes

- **Media retries repeat ambiguous generations (high confidence, high impact).**
  A provider's HTTP 504 makes image generation/editing and compaction try another
  account although work may still be running upstream. A real HTTP regression
  reproduced a 200 after the first account returned 504. Fixed the shared media
  retry path to return 504 immediately, matching the main inference pipeline.
  Regression covers generation, edits and compaction; post-fix check passed.
- **New push relay handles immediately evicted at the cap.** At 20 handles per
  token, registrations sharing second-resolution timestamps retained older rows
  and returned a new handle that already answered 410. Deterministic SQLite and
  touch-handler regression failed before the fix. Added insertion-order tie break
  and an atomic registration/eviction transaction; passed the intermediate full suite.
- **OAuth refresh resurrects deleted operator settings.** Refresh overlaid the
  current file on stale credential metadata, restoring absent `disabled`,
  `priority` and unknown fields. Regression failed with "refresh resurrected
  disabled". Existing valid file metadata is now authoritative; rotated tokens
  and explicit provider extras still update. Regression passed.
- **API-key switches reset during reload/restart.** Regression reproduced an
  account being re-enabled after `reload_accounts`. Disabled account IDs are now
  privately/atomically persisted separately from config layouts; reloads and key
  switches serialize. Coverage includes normal keys, individual compatible keys,
  keyless endpoints, changed account allocations, re-enable, restart, private file
  permissions and absence of raw keys. A failed-write regression also verifies an
  error leaves memory/reload state unchanged and a subsequent retry succeeds. Passed.
- **Deleting an account races credential refresh.** Removal could return while a
  rotating token was still being saved, letting a late save recreate the sign-in.
  Regression demonstrated deletion proceeding under the refresh lock. Removal
  now waits, clears pending saves and invalidates old allocations; subsequent
  stale publication is rejected. Regression passed. Metadata field edits also
  serialize with pending credential saves.
- **Delayed reloads undo account-file controls.** A reload that already collected
  a file could publish it after deletion, revalidating old allocations; it could
  also overwrite a newer disabled flag. Two deterministic regressions pause a
  reload after collection and perform the file edits under the writer lock. Both
  failed before the fix. Reload now rereads the switch under that lock and skips
  files deleted since collection. Both targeted post-fix checks passed, as did the
  final full suite.
- **A deferred refresh save overwrites a new sign-in.** The periodic persister
  retries without the refresh lock, while signing in again previously wrote before
  taking its pending-save lock. A failed-save/concurrent-sign-in regression ended
  with the old grant's rotated token on disk instead of the new grant. Sign-in now
  holds both locks across the write and clears the superseded pending save. Before
  fix: failed with `rotated` versus `signed-in`; management suite passed (16 tests).
- **Dashboard reloads lose newer request events.** Four deterministic JS tests
  failed: live arrivals were erased, old overlapping reloads won, restarted IDs
  lost new events, and merged bounds were wrong. Reloads now preserve only arrivals
  during the read, deduplicate overlap, bound the result and ignore older reloads.
  Final review reproduced two more stale-snapshot races between navigation's
  full-log read and reconnect's reload. Both paths now share the same snapshot
  reader and generation check. All six regressions passed.

## Remaining finding and confidence limits

- iPhone AppStore refresh tasks can outlive sign-out/connection changes; source
  inspection suggests old results can repopulate state. No Apple toolchain on this
  machine (`swift` and `xcodebuild` absent), and no executed reproduction yet.
  Follow-up: delay API responses across sign-out/reconnect on an Apple test host,
  then verify cancellation and connection-generation guards. No iOS files changed.
- Provider adapters were sampled, not exhaustively audited. Tests use synthetic
  upstreams; no real-provider, native iOS runtime or browser layout verification.

## Validation

- Logs for longer runs: `/tmp/fusebox-bugfix-validation/`.
- Final Rust: `CARGO_BUILD_JOBS=4 cargo +stable test --locked` — 824 passed,
  7 unchanged ignored manual benchmarks; no failures (baseline 815 passed).
- Final dashboard: `node --test tests/*.test.cjs` — 167 passed (baseline 161).
- Final Python: `python3 -m unittest discover -s tests -p 'test_*.py'` — 46 passed.
- Final formatting and clippy (`--locked --all-targets -- -D warnings`) passed.
- Targeted management: 16 passed; delayed reloads: 2 passed; request snapshots:
  6 passed. Before-fix failure logs retain the concrete reproductions.
- `CARGO_BUILD_JOBS=4 cargo +stable build --release --locked` passed.
- Managed release-binary acceptance passed: `scripts/dev-smoke.py` (8 checks)
  and `scripts/usage-e2e.py` (12 checks). Covered health/readiness, native and
  translated inference, compressed requests, current embedded assets, gzip/ETag,
  metrics, graceful exit, collectors/authentication, replay/deduplication,
  persistence across stop/restart, usage pricing and content omission.
- Final combined diff reviewed; `git diff --check` passed. Changes are limited to
  the eight confirmed issues, their regressions, and persistence documentation.

## Handoff and cleanup

- Investigation and verification are complete for the actionable findings above.
  The independent iPhone follow-up and review limitations are recorded above.
- Rebuilt, started and restarted a disposable dev candidate. Managed lease
  `c9ca0696b8ef4b478b3f8451874b610e` exited successfully; `agent-resource check`
  reported `closed`, no remaining resources/volumes and no cleanup errors.
- Released CI capacity reservation `ci-job-de26b7643d7e4a2192e4f534262b1191`
  by closing stdin; reservation command exited successfully. No CI workers created.
- Removed two empty directories left by the intentional failing reproductions:
  `/tmp/fusebox-key-switches-9eb5fd0f-0db4-4787-a226-399f9c480391` and
  `/tmp/fusebox-remove-refresh-cdef1581-689d-4f24-812f-3128afdf9078`.
- No temporary services retained. Validation logs remain at the path above;
  normal Cargo build artifacts remain in `target/`.
- No provider credentials, production changes, publishing, pushing or deployments.
