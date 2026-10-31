# Anthropic Max/Ultra implementation review

Reviewed commit `c2b5502621a460085b2971f2e2fd7bea140ded77` against
[the original research](anthropic-max-ultra-research-2026-10-31.md).
The normal Max/Ultra paths are implemented, but five behavioral gaps and some
connection wording remain. Application code, configuration and running services
were not changed during this review.

## Findings

### 1. Starter classification still permits generation with an active or unknown session

Relevant code: `src/window_starter/mod.rs:182` and `:352`.

`window()` returns `Inactive` for a null legacy `five_hour` field unless the
no-limit classifier returns true. It does not inspect a typed `session` entry.
Separately, `Api::usage()` replaces that null with a synthetic zero-percent
window before the shared quota parser runs. The parser then ignores the real
typed session because a legacy window is already present.

Two mock probes reproduced unwanted generation:

- `five_hour: null` with a typed session at 100% and a future reset sent one
  generation request instead of zero.
- A known `max_20x` account with `five_hour: null`, a usable week and missing
  applicability metadata also sent one generation request instead of remaining
  unknown and only permitting status checks.

Use a shared classification of validated legacy/typed session data in both the
starter and quota update paths. Honor an active typed session, and preserve
unknown applicability when the evidence is incomplete. This is the first fix
to prioritize because it can both overwrite real exhaustion and spend quota.

### 2. Malformed typed metadata can retire a genuine short restriction

Relevant code: `src/quota/readings.rs:160`.

`claude_no_five_hour()` accepts any list containing an object whose `kind` is
`weekly_all`, provided `five_hour` is explicitly null and there is no `session`.
It does not validate the descriptor or constrain the inference to a known Ultra
profile.

A mock probe first installed an exhausted five-hour window, then supplied a
valid legacy weekly window with:

```json
{"five_hour": null, "limits": [{"kind": "weekly_all"}]}
```

The incomplete descriptor was sufficient to mark the account as having no
five-hour limit and retire its restriction. Malformed metadata should remain
unknown. Validate the evidence before using it to remove retained restrictions.
This is a robustness finding from a synthetic malformed payload, not a claim
that Anthropic currently sends this payload.

### 3. Automatic banked resets still spend grants for removed scopes

Relevant code: `src/banked_resets/automation.rs:113`.

The one-hour expiry branch returns true before checking applicable scopes.
A mock account confirmed to have no five-hour limit still redeemed a grant
whose only scope was `five_hour`. The expected number of redemptions was zero;
the observed number was one.

Exclude grants whose scopes are confirmed absent before applying the expiry
override. Keep weekly reset support and the existing policy for applicable
grants, including its zero-usage expiry behavior.

### 4. An unavailable Ultra account suppresses useful pool warnings

Relevant code: `src/pools.rs:139`.

The new suppression removes the entire five-hour pool whenever any member has
`no_five_hour`, regardless of its other limits or sign-in state. A mock case
with a Max account at 90% five-hour usage and an Ultra account with exhausted
weekly quota and an expired sign-in produced no short-window pace warning.

Only use an unlimited member to suppress that warning when it can actually
carry the relevant traffic, or keep the warning explicitly scoped to the
limited accounts. The probe checks the warning path, not routing failure.

### 5. Cached starter status overrides a returning five-hour limit in the UI

Relevant code: `ui/app.js:2311`.

`status === 'not_needed' || noShortLimit(a)` gives a prior starter result
precedence over fresh quota data. When a current reading clears the no-limit
flag and reports a real inactive five-hour window, the UI still says there is
no five-hour limit and hides **Start now** until another starter status check.
The added UI regression probe reproduced this behavior.

Give current positive limit evidence precedence over the cached starter result.

### 6. Connection wording still omits Ultra

The connection selector and sign-in introduction still say Pro or Max:

- `ui/app.js:116` and `:126`.
- Matching Chinese text in `ui/i18n-zh.js:424`, `:425` and `:769`.
- README login example at `README.md:67` and provider table at `:189`.
- Subscription-cost wording at `docs/usage-costs.md:34`.

Include Ultra in these current-plan lists. Historical references to the old
names are appropriate and do not need a blanket replacement.

## What looks correct

- Provider display names are captured separately from the existing plan IDs;
  legacy `max_20x` and `max_5x` display as Ultra and Max.
- Ambiguous Max identity is not assigned a numeric weekly weight.
- The observed verified-Ultra usage shape retires the retained short window,
  its attribution state and the covered quota cooldown. Weekly exhaustion
  still blocks requests, and later positive short-window observations restore
  that limit.
- The confirmed no-limit starter path returns `not_needed` without generation.
- Compact/detail meters and the new English/Chinese no-limit text are covered.
- Weekly weights remain Pro 1, Max 5, Ultra 10, including the `ultra` alias.
- Existing weekly-only routing coverage passes.

The current cosmetic display-name change preserves forecast history because
the machine ID remains stable. One earlier future-compatibility recommendation
is still unimplemented: `src/accounts.rs:453` and `src/quota.rs:543` compare raw
plan strings, so an eventual `max_20x` to `ultra` identifier change would clear
history despite the same semantic plan. This is not a failure of the currently
observed provider rename.

## Validation and limits

Tests ran against an immutable archive of the reviewed commit. Added probes
were applied only to the temporary snapshot; they were not added to application
source or the repository's test files.

- Existing Rust suite: **771 passed, 0 failed, 5 ignored**.
- Existing dashboard suite: **148 passed, 0 failed**.
- Five added Rust probes failed, covering findings 1–4 (two probes for finding 1).
- One added UI probe failed, covering finding 5.

The Rust `review_` selection also ran two existing matching tests, which passed.
Evidence and the reproducible probe runner are retained under
`target/max-ultra-review-20261031/`, including `results.json`, the baseline and
probe logs, and `run-review.py`.

No live inference or real reset redemption was performed. Provider payload
variants in the probes are controlled edge cases; this review does not establish
how often those variants occur in production. Browser and deployment behavior
were not revalidated.

Managed test leases `f42b7e8a3f3543d3a7d344f81509586c` and
`5bb42b0cec124d59bfd270301f6429f1` were checked closed with no remaining resources.
The test capacity reservation was released. No temporary services were retained.
