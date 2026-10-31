# Anthropic Max/Ultra follow-up review

Reviewed `3e8df1784f1c14f95144af363d48f51970c21df0` against the
[previous findings](anthropic-max-ultra-implementation-review-2026-10-31.md).
All six previous regression probes pass, and the connection wording now names
Ultra. Three additional edge cases remain.

## Remaining findings

### P2: An ambiguous read can still start a confirmed no-limit account when its plan is unknown

Code: `src/window_starter/mod.rs:183`, `:351` and `src/quota.rs:161`.

The new bare-null safeguard depends on `Quota::ultra()`, which returns false
when the plan and display name have not been learned. It does not consider an
existing `no_five_hour_limit: true` capability observation.

Reproduction:

1. Apply the valid verified-Ultra usage fixture. The quota now correctly has
   `no_five_hour_limit: true`, but no plan identity has been learned yet.
2. On the next starter preflight, return `five_hour: null` and a usable weekly
   window, without the typed `limits` list.
3. The starter interprets the null as inactive because `ultra()` is false,
   synthesizes a zero-percent short window, clears the no-limit flag and sends
   a generation request.

The mock observed one generation instead of zero, returning `status: started`.
This can arise before a successful plan poll or when profile information is
unavailable. Known applicability must not depend on the plan poll succeeding.
Preserve the confirmed absence on an ambiguous read, and distinguish an unknown
plan from a confirmed plan that has the limit. Require positive evidence of a
returning short window before restoring it.

Probe: `followup_confirmed_absence_survives_ambiguous_usage_before_plan_poll`.

### P2: Whole-account pauses are still ignored when suppressing short-window warnings

Code: `src/pools.rs:141` and `src/push/faults.rs:170`.

The new `carries` predicate correctly handles expired sign-ins and exhausted
general weekly quota. However, its availability input is still only
`!signin_error()`. It does not account for whole-account quota/rate-limit pauses.

Reproduction: a Max account has 90% five-hour usage and renews in one hour;
an Ultra account has 10% weekly usage, a valid sign-in and an account-wide
`rate_limit` pause for two hours. `AccountView::unavailable()` correctly returns
true for Ultra, but no `pace:pool:claude:5h` warning is produced. That account
cannot carry overflow during the relevant short window.

Include whole-account pauses in the decision to count an unlimited account as
usable overflow capacity, or retain a warning scoped to the limited accounts.
This finding concerns warning suppression; it does not show that routing ignores
the pause.

Probe: `followup_paused_ultra_does_not_suppress_a_short_window_warning`.

### P2: A valid weekly entry still masks malformed applicability entries

Code: `src/quota/readings.rs:166`.

The new percentage check fixes the original descriptor-without-a-percentage
case. It validates one weekly entry, while still treating the rest of the list
as authoritative evidence that no session exists.

A follow-up probe starts with real five-hour exhaustion and then supplies null
`five_hour`, a valid weekly window and these typed entries:

```json
[
  {"kind":"weekly_all","group":"weekly","percent":20,
   "resets_at":"2099-11-02T13:00:00Z","is_active":true},
  {"kind":null,"group":"session","percent":100,
   "resets_at":"2099-11-02T13:00:00Z","is_active":true}
]
```

The malformed session descriptor is ignored, so the valid weekly entry is enough
to mark the account unlimited and retire the retained restriction. An
unclassifiable applicability entry should make absence unknown. One valid
weekly entry does not validate the complete list.

This is a controlled malformed-payload test, not an observed Anthropic response.

Probe: `followup_malformed_session_descriptor_is_not_proof_of_absence`.

## Confirmed fixes and smaller outstanding item

- Active typed sessions are preserved and do not generate starter messages.
- A known Ultra plan with bare-null usage remains unknown and does not generate.
- The original malformed weekly descriptor preserves the exhausted short limit.
- Five-hour-only reset grants are skipped after removal; grants that also clear
  weekly limits remain usable.
- Weekly-exhausted and signed-out Ultra accounts no longer hide short warnings.
- Fresh positive short-window quota overrides a cached `not_needed` UI result.
- Returning limits prompt an earlier automatic starter recheck.
- English/Chinese connection text, README and usage-cost wording include Ultra.

The earlier future machine-ID alias/history item remains unchanged:
`src/accounts.rs:453` and `src/quota.rs:549` still compare raw plan strings.
Changing `max_20x` to the equivalent `ultra` identifier would clear forecast
history. The currently observed display-name-only migration preserves history.

## Validation and scope

- Existing Rust tests: **779 passed, 0 failed, 5 ignored**.
- Existing UI tests: **149 passed, 0 failed**.
- All six original probes passed: five Rust cases and one UI case.
- All three follow-up probes above failed and reproduced the remaining gaps.

Tests ran on an immutable archive of the reviewed commit. Probes were added
only to temporary snapshots. Application code, repository tests, configuration
and running services were unchanged. No live inference or reset redemption was
performed; deployment and browser behavior were not revalidated.

Receipts and the reproduction runner are in
`target/max-ultra-rereview-20261031/`, including `results.json`, baseline logs,
original-probe logs, `followup-rust-probes.log` and `run-review.py`. An initial
metadata probe concerning invalid weekly fields is retained in the `-initial`
receipts; the final metadata probe above tests malformed applicability itself.

Both managed test leases, `87034b27c3924bde8fc8f3caca9c3dda` and
`4f305b9d1e3e4bc69cbb940a27375412`, were checked closed with no remaining
resources. Their capacity reservations were released.
