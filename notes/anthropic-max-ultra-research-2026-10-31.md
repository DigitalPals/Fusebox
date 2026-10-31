# Anthropic Max and Ultra: findings and Fusebox recommendations

Researched 31 October 2026 against Anthropic's current public documentation,
Fusebox source, and read-only production/provider responses. This is a research
report; application code, configuration, credentials, and services were unchanged.
No inference requests or reset redemptions were made.

## Confirmed product changes

| Previous name | Current name | Monthly web price, before tax | Five-hour allowance | Weekly allowance |
| --- | --- | --- | --- | --- |
| Max 5x | Max | $100 | 5x Pro | 5x Pro |
| Max 20x | Ultra | $200 | No five-hour limit on verified accounts; a temporary limit applies before verification | 10x Pro |

Anthropic explicitly says existing Max 20x subscriptions move to Ultra
automatically, with no price change. The current Max article and pricing page
describe the $100, 5x tier simply as Max.

Verification is automatic and uses account/payment history. Phone verification
at checkout is a separate process and does not remove the temporary limit. The
documented user-visible check is Settings > Usage: a verified Ultra account has
no **Current session** bar. The Ultra name alone therefore cannot establish that
an account has no short limit.

Ultra retains its general weekly limit and a separate weekly Fable limit. The
pricing page says Max and Ultra include Fable for up to half their weekly
allowance. Usage is shared across Claude apps and Claude Code. Other capacity,
model, and feature restrictions may still apply.

The included monthly API credits remain $100 for Max and $200 for Ultra. They
are separate from subscription allowance and paid usage credits; this rename
does not justify enabling credit spending or changing API token pricing.

Primary sources, fetched on the research date:

- [Anthropic pricing](https://claude.com/pricing): current names, prices, 5x/10x weekly multiples, verification footnote, Fable share, and API credits.
- [What is the Ultra plan?](https://support.claude.com/en/articles/17728952-what-is-the-ultra-plan): explicit Max 20x migration, verification rules, temporary limits, and remaining weekly limits.
- [What is the Max plan?](https://support.claude.com/en/articles/11049741-what-is-the-max-plan): $100 monthly price and 5x Pro usage every five hours and every week.
- [How do usage and length limits work?](https://support.claude.com/en/articles/11647753-how-do-usage-and-length-limits-work): shared usage, fixed weekly reset times, and the verified Ultra exception.
- [Use Claude Code with your Pro, Max, or Ultra plan](https://support.claude.com/en/articles/11145838-use-claude-code-with-your-pro-max-or-ultra-plan): the same rules apply in Claude Code, including temporary Ultra limits.

The help articles displayed “Updated today.” They do not provide a separate
publicly documented OAuth schema or verification-status API contract.

## Live evidence

Production was running `0.11.1+gui.77b25555372f`. Its management snapshot still
identified all four Claude subscriptions as `max_20x`, and marked all four as
supported and selected for automatic window starting. Some retained five-hour
entries had zero utilization and no renewal time; another had an elapsed renewal.
Those cached entries alone do not establish current provider enforcement.

Direct read-only profile and usage requests succeeded for two accounts. Both
profiles contained:

```json
{
  "organization_type": "claude_max",
  "rate_limit_tier": "default_claude_max_20x",
  "plan_display_name": "Ultra",
  "plan_display_label": "Ultra plan"
}
```

Both also retained `account.has_claude_max: true`. Their usage responses contained
an explicit `five_hour: null`, a populated `seven_day`, and typed `limits` entries
for `weekly_all` and `weekly_scoped` Fable. Neither sampled typed list contained
a short-window entry. This matches the documented verified Ultra presentation.
The Fable entries were inactive in this sample, which is another reason to
continue using each account's actual reported limits.

This establishes that new display names already coexist with legacy machine
identifiers. It does not establish the verification state of every configured
account, the payload shape of an unverified Ultra account, or the payload shape
of a current $100 Max account. Those need fixtures or further account-specific
observations before treating an undocumented field combination as a universal
capability signal. No explicit verification flag was established by this audit.

## What should change

### 1. Separate product naming from machine identifiers

`oauth::claude_plan` currently derives a plan from `organization_type` and the
rate-limit-tier suffix, ignoring `plan_display_name` and `plan_display_label`
([source](../src/oauth.rs#L405)). `ui/app.js` then capitalizes that identifier
([source](../ui/app.js#L850)), producing the old labels.

Capture the provider's display name and use it for Claude account labels. Add
fallback aliases `max_20x` → **Ultra** and `max_5x` → **Max** for older profiles
and snapshots. Continue accepting legacy machine identifiers and preserve the
existing management API identifier where possible; a display-name change does
not require credential-file renames or signing in again.

Do not map `has_claude_max: true`, or an old ambiguous `max` identifier, to the
$100 tier without corroboration: the sampled Ultra accounts have that flag too.
Recognize future explicit Ultra/Max identifiers when their meaning is known,
while retaining an unknown fallback.

Use semantic plan identity when deciding whether forecast history needs to
restart. `AccountState::set_quota` and `poll_plan` currently clear that history
on a changed plan string; a cosmetic alias migration should preserve it, while
a real allowance change should still trigger the appropriate transition.

### 2. Model short-limit applicability explicitly

Represent **unknown**, **limited but inactive**, **limited and active**, and
**not applicable** separately. Product identity and short-limit applicability
are different facts, because Ultra accounts may still have a temporary limit.

For a known Ultra account, use validated full usage data and the provider's
applicable-limit metadata to identify the absence of a short limit. The observed
combination of an Ultra profile, explicit `five_hour: null`, and weekly-only
typed limits is a useful fixture. Validate the unverified/idle cases before
making the classifier universal. Missing keys, failed reads, malformed data,
and ordinary partial response headers must remain unknown.

`readings::claude` currently drops a null five-hour block
([source](../src/quota/readings.rs#L157)). `Quota::set` preserves every omitted
window ([source](../src/quota.rs#L274)), and `apply_reading` also preserves
cooldowns whose prior scopes were not covered. Consequently, an account that
loses its short limit can retain an old five-hour restriction until its old
deadline, or indefinitely when the old exhausted window has no reset time.

Add an explicit retirement path for a provider-confirmed removed window. Retire
its active quota/attribution state and clear only cooldowns justified by that
removed restriction. Preserve weekly, model, rate-limit, and authentication
restrictions. Cover persisted cooldowns and late in-flight observations so they
cannot resurrect the retired restriction; honor a real limit if the provider
subsequently reinstates one. Keep the existing omission-safe merge for partial
observations.

### 3. Stop starting windows that do not exist

`window_starter::window` treats `five_hour: null` as `Inactive`
([source](../src/window_starter/mod.rs#L178)). Its usage helper converts null
to a synthetic zero-percent window ([source](../src/window_starter/mod.rs#L345)).
Its support check only validates native OAuth identity, so verified Ultra
accounts remain eligible for manual and automatic starter requests.

Use the shared applicability classification before starter dispatch. A confirmed
no-limit account should report **Not needed: no five-hour limit**, generate zero
starter messages, and offer no Start now action. Unknown applicability should
allow a read-only status check without spending quota. A genuine inactive
limited window should remain startable, including on an unverified Ultra
account. Retain existing selections so an account can become eligible again if
its actual limits change.

This is the highest-priority behavior fix because production currently has all
four Claude accounts selected for automatic starting. The UUID check described
as a “verified Claude account ID” in the starter is unrelated to Anthropic's
new account/payment verification.

Banked resets must likewise operate on the provider's actual remaining scopes.
Keep weekly reset support for Ultra; do not spend a reset solely to clear a
five-hour restriction that no longer applies.

### 4. Update meters, pools, and wording

Compact desktop/mobile meters currently always create both a short and a long
row ([source](../ui/app.js#L1010)). Hide the short meter or show an explicit
**No five-hour limit** state when confirmed; keep unknown data distinct. Update
overview/account labels, connection text, English/Chinese translations, README,
window-starter help, quota/outlook documentation, and label assertions.

The weekly numeric weights in `pools::plan_weight` are already correct:
**Pro 1, Max 5, Ultra 10** ([source](../src/pools.rs#L35)). Preserve those ratios
and recognize both legacy and confirmed current identifiers. Anthropic now
publishes these relative weekly allowances, so the whole-account weekly
explanation can use that basis instead of an inferred price proxy. Do not
extrapolate those ratios to an undocumented temporary short limit.

Mixed limited/unlimited pools need explicit applicability too.
`pools::pool` currently forms a short-window pool from whichever members report
one, and `pool_pace` can warn about that subset. Such a warning must not imply
all Claude capacity will be unavailable when an eligible Ultra account has
usable weekly/model allowance. Suppress that implication or clearly scope the
warning to the limited subset.

### 5. Keep the working routing behavior

`smart_score` already falls back to weekly/model headroom when there is no
five-hour window ([source](../src/accounts.rs#L1927)). Its five-hour reserve
only applies to an actual short window; its weekly reserve still applies.
There is already an HTTP/WebSocket regression for weekly-only routing
([test](../src/routing_tests.rs#L686)). Build on this behavior.

## Verification for implementation

- Legacy and current profile names render Max/Ultra consistently; unknown Max flags stay ambiguous. Cosmetic aliases preserve forecast history and account identity.
- Verified Ultra, temporarily limited Ultra, idle limited accounts, missing fields, malformed reads, and partial headers remain distinct.
- Transition from an exhausted five-hour limit to confirmed no limit removes only that restriction, including after restart and across late responses. Weekly/Fable exhaustion still blocks the relevant requests.
- Verified Ultra starter checks and scheduled runs make zero generation calls; real inactive limited windows still work.
- Mixed Pro/Max/Ultra weekly pools retain 1:5:10 weights, unknown capacity remains unknown, and subset short-limit warnings do not imply provider-wide exhaustion.
- Desktop/mobile meters and English/Chinese text agree with actual capability. Existing HTTP/WebSocket affinity and weekly-only routing checks remain valid.

These are proposed implementation checks. No application changes or test suite
runs were needed for this research-only task.
