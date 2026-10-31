# Combined weekly quota

The overview's **Combined weekly quota** shows each provider with at least two
enabled subscriptions as one **WK** row, drawn like an account's weekly meter: the
current combined quota, used or remaining as the subscription display toggle says,
the even-pace mark and a countdown to the next account's reset. The bar is never a
forecast. Beside it, the status says whether the quota is expected to last over the
next seven days, from the forecast below, or why there is no forecast yet. When an
account doesn't report its weekly quota there is no combined bar: a balance is
never made up from the others.

## Consumption estimate

The existing identity-bound `.quota-attribution.state` journal now also keeps
samples of cumulative observed weekly quota consumption: every five minutes for
the last day and hourly before that, for seven days.
The first observation is a baseline, not new spending. Positive quota changes
include usage outside Fusebox; token counts and API price estimates are not used.
A scheduled or banked reset retains the consumption history and adds only the new
window's observed usage. Small downward rounding differences use a high-water
mark so the same usage is not counted twice. Late readings from an older reset
are ignored. As with any sampled estimate, unobserved consumption immediately
before a reset cannot be recovered from the new window's counter.

Each account needs six hours of observations, with no gap longer than one hour.
Its rate is the average of two: the last 24 hours, which follows a change in
demand, and the last seven days, which keeps one busy or quiet day from swinging
the forecast. Each covers as much of its span as was observed, interpolating
between the samples around its start, so until an account has a day of history
both are the same. Genuine observed idle time counts; missing readings do not. A
longer gap begins a new learning period. Readings more than one hour old are
stale. The displayed learning time assumes readings continue. Known changes of
subscription plan or credential identity restart learning. Existing journals load
without history and begin learning; old releases can ignore the new field, and
read the week of samples keeping the last day. The history is saved with
attribution every 30 seconds and on clean shutdown.

## Projection

The server combines rates and allowances with the same weights: Claude Pro 1,
Max 5, Ultra 10 when every reporting plan is known; equal weights otherwise.
These are Anthropic's published weekly allowances relative to Pro (since 30
October 2026; Max and Ultra were Max 5x and Max 20x, whose tier identifiers
`max_5x` and `max_20x` remain). They also match the monthly prices.

At each event, consumption since the preceding event is taken from accounts in
order of their next reset. At the reset, that account's remaining allowance is
replaced by its full allowance; unused quota does not roll over. Simultaneous
resets are processed together. The simulation continues to the end of the seven
days, including the interval after the last reset. It records the lowest reserve,
when it occurs, and the first depletion time and subsequent reset if demand
cannot be met. Exactly reaching zero at a reset is low reserve, not an outage.

- **Expected to last:** no projected gap, minimum reserve at least 10%.
- **Low reserve:** no projected gap, minimum reserve below 10%.
- **Quota gap expected:** demand would exhaust the pool before capacity returns.
- **Learning usage:** at least one account needs six hours of recent observations.
- **Forecast unavailable:** incomplete/invalid quota or reset data, stale readings,
  an expired reset awaiting confirmation, or an account needing sign-in.

The status's hover gives the daily use the forecast assumes, as a share of the
combined quota, and until every account has a week of history the shortest one.
The weekly pool's quiet fault uses the same forecast. Missing data is never
interpreted as a healthy forecast or zero consumption. Single-account, five-hour
and model-specific pace warnings retain their own calculations.

The forecast assumes constant recent demand and that work can use the eligible
accounts in renewal order. Session affinity, bursts, five-hour/model limits and
provider availability can still restrict individual requests. Manual and banked
resets are reflected once observed, but their future use is not predicted.

## API

`GET /api/overview` adds `outlook` to whole-account weekly pools. It includes
`state`, `reason`, `horizon_at`, `next_reset_at`, `min_remaining`, `min_at`,
`exhausts_at`, `recovers_at`, `rate_per_day`, `observed_hours`, and `learning_until`.
Percentages are weighted pool percentage points; `rate_per_day` is their estimated
daily consumption and `observed_hours` the shortest account history behind it, up
to seven days. Unknown estimates are null. The existing `used` and `pace`
fields remain current-window measurements, not forecast values. `runs_out_early`
and `until` agree with the new weekly projection.
