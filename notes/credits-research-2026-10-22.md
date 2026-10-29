# Credits beyond the subscription: research, 2026-10-22

Question: Fusebox serves subscriptions, but there may be API credits too. Can we see them,
and should Fusebox support them (optionally)?

## Three kinds of credit

1. **Overflow credits on the signed-in accounts Fusebox already uses.** These are Claude
   "usage credits" (formerly extra usage) and ChatGPT Codex credits. Once enabled and funded,
   the provider spends them automatically when a plan window runs out, and the request still
   answers 200.
2. **Monthly Console API credits included with Max.** Max 5x gets $100 a month and Max 20x
   gets $200. They are claimed by linking one Console organization on claude.ai (Billing,
   API credits) and work only with an API key from that organization, not with the
   subscription sign-in. They expire at the end of each billing cycle and are spent before
   purchased credits. No API reports this balance.
3. **Prepaid API-key balances** for the Anthropic Console, the OpenAI platform, OpenRouter,
   Moonshot, DeepSeek, xAI and others. Only some providers report a balance to the key used
   for requests.

## What production shows (read-only, 2026-10-22)

These are the usage endpoints Fusebox already polls, read once per account.

- **Codex Pro account:**
  - `credits` reads `has_credits: true`, `unlimited: false`, `overage_limit_reached: false`,
    `balance: "61840.9086050000"`.
  - OpenAI's estimate of what that buys: `approx_local_messages: [15460, 80393]` and
    `approx_cloud_messages: [2474, 15460]`.
  - There are also `spend_control` (`reached`, `individual_limit`), `model_usage.<model>.credits_would_enable`
    and `rate_limit_reset_credits` (2 banked resets). The weekly window is at 64%.
- **Claude, one of three Max 20x accounts:**
  - `extra_usage` reads `is_enabled: false`, `disabled_reason: "out_of_credits"`,
    `credits_ever_enabled: true`, `monthly_limit: 10000`, `currency: "EUR"` and
    `decimal_places: 2` (minor units, so €100.00), with `used_credits: 0`.
  - The `spend` block says the same in `{amount_minor, currency, exponent}` form, with
    `balance: null` and `can_purchase_credits: false`.
  - The other two accounts never enabled usage credits.
- **Two of the Claude accounts also carry `iguana_necktie`:** `limit_dollars: 250`,
  `used_dollars: 0`, `resets_at: 2026-11-05`. This dollar allowance is undocumented and
  unidentified; it does not match the $200 Max 20x API credit.
- **Side finding:** Claude's usage response now has a `limits[]` array. One entry is a
  `weekly_scoped` limit for Fable, with `weekly_scoped_shares` putting its allowance at 50% of the
  weekly limit. Fusebox doesn't parse it: it reads `seven_day_opus` and `seven_day_sonnet`, and
  both are null now.
- **No API keys** are configured in production.

## What Fusebox does today

- **The credit data is fetched and dropped.** `quota::poll` already fetches both usage
  bodies every 5 to 30 minutes, and `quota::observe` reads every response's headers.
  Ignored: Claude `extra_usage` and `spend`, the Claude
  `anthropic-ratelimit-unified-overage-*` headers, Codex `credits` and `spend_control`, the
  `x-codex-credits-*` headers and `credits` in the `codex.rate_limits` WebSocket event.
- **Credits are never spent on purpose.** A window at 100% is a cooldown
  (`Account::cooling_until`). When every account is cooling, the request gets
  `429 all accounts for {model} are rate limited`. At most the requests in flight while a
  window crosses 100% are billed to credits.
  - That is a safe default, but it is implicit and nothing shows it. With a single Codex
    account, the 61.8k credits can never be used through Fusebox once the week runs out.
- **API keys that run out of money:**

  | Provider | Error | Fusebox now |
  |---|---|---|
  | Anthropic | 400 `Your credit balance is too low…`; your own Console limit: 400 `You have reached your specified API usage limits` | The 400 branch answers the client with the error: **no failover, no cooldown**. `quota_exhausted` lists "credit balance is too low", but only checks 429 and 402. **Bug.** |
  | Anthropic | 429 `enforced_spend_limit_reached` (tier cap, no `retry-after`) | Rate limit with escalating backoff |
  | OpenAI | 429 `insufficient_quota` | Exhausted, 5-minute cooldown, tried again every 5 minutes |
  | OpenRouter, DeepSeek | 402 (`payment_required`, "Insufficient Balance") | No string matches, so it fails over **without a cooldown**: every new request tries it first |
  | xAI | 403 "used all available credits or reached its monthly spending limit" | Fails over with a 10-minute cooldown (the 401/403 branch) |
  | Moonshot | 429 `exceeded_current_quota_error` | Rate limit with escalating backoff |

- **Overage reset trap.** When Claude's `representative-claim` is `overage`,
  `anthropic-ratelimit-unified-reset` is the monthly billing boundary. CLIProxyAPI locked a
  healthy account for 24 days by taking it as a cooldown
  ([#6465](https://github.com/router-for-me/CLIProxyAPI/issues/6465)).
  - `proxy::reset_after` reads that header, but `mark_quota_exhausted` prefers the window
    reset from `exhausted_until`, so Fusebox is probably safe.
  - A test should pin this.

## Provider facts

### Claude usage credits

([support 12429409](https://support.claude.com/en/articles/12429409-manage-usage-credits-for-paid-claude-plans), verified)

- **Setup:** opt-in ("Enable"), then prepaid.
- **Behaviour:** after any limit (five-hour, weekly, or the Fable weekly limit on Max),
  "requests switch to pay-as-you-go pricing on usage credits and keep working".
- **Price and limits:** standard API rates. You set a monthly spend limit or "unlimited".
  Auto-reload is optional.
- **Expiry:** credits don't expire, except in Japan (6 months).
- **Applies to:** Claude chats and Claude Code alike.
- **Headers:** `anthropic-ratelimit-unified-{status, representative-claim, overage-status,
  overage-reset, overage-utilization, overage-in-use, overage-disabled-reason,
  fallback-percentage}`.
  - Observed disabled reasons: `org_level_disabled`, `out_of_credits`,
    `member_zero_credit_limit`, `org_spend_cap_reached`.
  - Claude Code warns at 75% and 95%.
- **Balance endpoint (undocumented):** `GET https://api.anthropic.com/api/oauth/organizations/{org_uuid}/prepaid/credits`
  with the OAuth bearer. It returns `amount` (minor units), `currency`, `auto_reload_settings`,
  `tranches[]`, `promo_tranches[]` and `next_expires_at`, and Claude Code's `/usage-credits` uses it
  ([headroom](https://github.com/amitray007/headroom/blob/main/docs/providers/claude.md)).
  Not tried from production.

### Codex credits

(codex-rs source; [pricing](https://learn.chatgpt.com/docs/pricing))

- **Structs:** `CreditsSnapshot { has_credits, unlimited, balance: Option<String> }`
  inside `RateLimitSnapshot`, next to `individual_limit`, `spend_control_reached` and
  `rate_limit_reached_type`.
- **Where it arrives:** the `x-codex-credits-{has-credits, unlimited, balance}` headers, the
  WebSocket rate-limit event and `wham/usage`.
- **Reached types:** `rate_limit_reached`, `workspace_{owner,member}_credits_depleted`,
  `workspace_{owner,member}_usage_limit_reached`.
- **Behaviour:** automatic: "After you reach your included limits, available credits let
  you continue working."
- **Price:** credits per million tokens by model. For example, GPT-6.1 Sol costs 50 input,
  2.5 cached and 250 output; fast mode doubles that.
- **Unverified:** a dollar price of about $0.04 per credit and a 12-month expiry come only
  from third-party sites. At that price this balance would be roughly $2,500.

### Max monthly API credits

([support 17154008](https://support.claude.com/en/articles/17154008-monthly-api-credits-for-max-and-team-plans), verified)

- **Amounts:** Max 5x $100, Max 20x $200, Team $20 or $100 per seat (pooled, capped at $500).
- **Claiming:** the subscriber claims by linking one Console organization. Each organization
  can receive credits from only one plan, so three Max accounts need three organizations.
- **Expiry:** no rollover; unused credits expire each billing cycle.
- **What it covers:** the Messages and Batches APIs, the Agent SDK, and `claude -p` run with an
  API key from that organization. It does not cover interactive Claude Code or usage
  credits.
- **Balance:** only the Console Billing page shows it; no API reports it.
- **Signed-in use draws on the plan.** With the plan's sign-in, `claude -p` and the Agent SDK
  use plan limits. The GitHub Action, the IDE extension and the desktop app count as Claude
  Code.
- **Probably not an overflow for Claude Code traffic in Fusebox.**
  - On API-key routes, Fusebox passes the client's body through unchanged, including Claude
    Code's `x-anthropic-billing-header: …cc_entrypoint=cli` line.
  - Anthropic likely uses that line to tell interactive Claude Code apart. This is inferred:
    the prepaid response has an `amount_without_scoped_credits` field, but the mechanism is
    unverified.
  - If so, such traffic would draw purchased credit, or fail with "credit balance is too
    low".
  - Only clients that aren't Claude Code (scripts, SDK tools) clearly qualify.
- **Not tried yet:** one small request of each kind through a key from the linked
  organization would show which balance moves.

### Balances for API keys

| Provider | Endpoint | Auth | Returns |
|---|---|---|---|
| Anthropic | none; `/v1/organizations/cost_report` gives spend only | admin key | spend, cents as decimal strings |
| OpenAI | none for `sk-` keys; `/v1/organization/costs` gives spend only | admin key | spend |
| OpenRouter | `GET /api/v1/key` | same key | `limit`, `limit_remaining`, `usage`, `usage_daily/weekly/monthly`, `is_free_tier` (the key's own limits, not the account balance) |
| OpenRouter | `GET /api/v1/credits` | management key per the docs; normal keys reportedly work | `total_credits`, `total_usage` |
| xAI | `management-api.x.ai/v1/billing/teams/{team}/prepaid/balance` | management key | `total.val` (cents) |
| Moonshot | `GET /v1/users/me/balance` (`.ai` in USD, `.cn` in CNY) | same key | `available_balance`, `voucher_balance`, `cash_balance` |
| DeepSeek | `GET /user/balance` | same key | `is_available`, `balance_infos[]` |
| SiliconFlow, Novita | `/v1/user/info`, `/openapi/v1/billing/balance/detail` | same key | balance fields |
| Gemini, Vertex | none (AI Studio or Cloud Billing only) | | |

### Other subscription sign-ins

- **Antigravity:** AI credits are spent only on request (`enabledCreditTypes`). CLIProxyAPI
  has an opt-in policy for this.
- **Kimi Code** and **SuperGrok:** have extra-usage balances, but no stable API reports them.
- **Devin:** ACU balances are available only through a browser cookie.

### Other proxies

- **New-API** has per-channel balance refresh for DeepSeek, OpenRouter, SiliconFlow, Moonshot
  and others. Its OpenAI method no longer works with `sk-` keys.
- **CLIProxyAPI** parses `x-codex-credits-*` and the Claude overage headers, and has an
  opt-in Antigravity credits fallback.
- **gproxy** has a per-credential `allow_paid_usage`, off by default.
- **LiteLLM** doesn't fail over on Anthropic's credit-balance 400.

## Recommendation

1. **Show the credits (S, do).** This needs no extra requests: parse Codex `credits` (body,
   headers and WebSocket event) and Claude `extra_usage` and `spend` into `Quota`.
   - Account line examples: "61,841 credits" and "Usage credits off: out of credits ·
     €0 of €100 this month".
   - Add a Prometheus gauge, and a notification when an account starts spending credits
     (Claude `overage-in-use`, or Codex at 100% with `has_credits`).
   - Parse `limits[]` too, so the Fable weekly limit is known.
2. **Fix out-of-money API keys (S, do).**
   - Treat these as exhaustion: Anthropic's credit-balance and usage-limit 400s, 402 from
     OpenRouter and DeepSeek, xAI's credits 403 and Moonshot's `exceeded_current_quota_error`.
   - Fail over, cool down long (for example 1 hour, doubling), and show the reason on the
     account.
   - Add a test that a Claude overage `unified-reset` never becomes a month-long cooldown.
3. **Spend credits when the plan runs out (M, opt-in per account, off by default).**
   - An account whose windows are used up but which has `has_credits` or enabled usage
     credits becomes a last resort, used only when no plan headroom is left in the pool.
   - Order among credit sources is the user's call: free monthly Console credits that
     expire, versus Codex credits, versus Claude usage credits (real money).
   - Fusebox cannot tell bought credits from granted ones.
4. **Budgets for API keys (S–M, optional).**
   - Each key gets a monthly amount and a cycle day. Fusebox stops routing to it at the
     budget, using its own Usage estimates; list price is what API keys are billed, so these
     are close.
   - This covers the Max monthly API credits, which no API reports. Also set a Console
     workspace spend limit equal to the credit, so purchased credit is never touched.
5. **Balance polling for API keys (later).** Only Moonshot, DeepSeek, OpenRouter,
   SiliconFlow and Novita answer with the key used for requests. It's not worth building
   until such keys are in use.
