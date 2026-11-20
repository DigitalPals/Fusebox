# T3 Code 0.1.11 pricing, 20 November 2026

T3 Code 0.1.11 prices two things that 0.1.3 did not, and Fusebox's default
calculation now does the same. Every rate itself was already current.

1. **Long prompts.** When a request's whole prompt (uncached input, cache reads
   and cache writes) is above a model's threshold, every category uses the
   long-prompt rate. GPT-5.4 and newer: above 272,000 tokens, input and cache
   cost twice as much and output one and a half times. Claude Haiku 5.5: five
   times above 100,000. Muse Ember: three times above 200,000. A prompt exactly
   at the threshold uses the ordinary rate.
2. **One-hour Claude cache writes** cost twice the input rate; five-minute
   writes stay at 1.25 times. Claude reports the split per response
   (`cache_creation.ephemeral_1h_input_tokens`).

Speeds follow T3: a long prompt at Fast or Ultrafast uses the long-prompt speed
rate. Where LiteLLM has none (GPT-5.4 and GPT-5.5 Fast), T3 falls back to the
ordinary speed rate, not the long-prompt one. The catalogue records those speeds
in `long_context_excluded_tiers`, which the T3 calculation reads as "use the base
rates for this speed above the threshold". The provider calculation keeps its own
meaning for that field (that tier cannot be priced above the threshold).

Long-prompt estimates carry the `t3_long_prompt_rates` assumption. They are not
marked partial.

## Catalogue

Default catalogue `t3-2026-11-20.1`: 57 identifiers in 39 rows, compiled by
`scripts/update-usage-pricing.py`, which now reads long-prompt and one-hour
rates the way T3 0.1.11 does. Long-prompt cache rates that LiteLLM leaves out
keep the base rate's ratio to input, in the script and in the code. A
long-prompt one-hour rate is written only when that rule would not give it, so
releases before 0.6 can still read the catalogue after a downgrade.

LiteLLM now lists Claude Fable 5.2, Mythos 5.2, GPT-6.2 and Muse Ember, so they
come from LiteLLM rather than fallback rows; their prices did not change.
LiteLLM does not list OpenAI's 1 March 2027 Ultrafast change for GPT-6.2. The
script keeps the rate card's dated rows for a model and fails if LiteLLM's
speed prices disagree with the period that covers the release date.

## Effect on production

Measured read-only over the 30 days to 2026-11-20 (about $43,000 of
estimates): 34 GPT-6.2 Sol requests were above 272,000 tokens, and 273,470
Claude cache-write tokens were one-hour writes. Together the new rules add
roughly $1 to $3. Estimates already saved keep their price until an operator
runs `fusebox usage reprice --all` (see
[the October rollout](usage-pricing-t3-2026-10-30.md#rollout)); new requests use
the new catalogue.

## Provenance and validation

Reference implementation: installed T3 Code 0.1.11 `usagePricing.ts` (bundled
JavaScript taken from the binary), binary SHA-256
`f13dd1f37eb2f719b41f45219b3800412799c80f724b022839b8a133e21aced1`. LiteLLM
input SHA-256
`d342612f90560be6c6c0a2b85e034a6cb9c2459ddcc0a4ba13946e0cd2759452`.

The 25 golden examples in `tests/fixtures/usage/t3-pricing.json` were evaluated
by that implementation with that input. Seven are new: mixed cache lifetimes,
Fable Fast with one-hour writes, Haiku exactly at its threshold, long GPT-6.2
Sol Fast, long GPT-6.2 Astra Ultrafast, long GPT-6.1 Sol Ultrafast, and a long
GPT-5.4 Fast request that keeps the ordinary Fast rate. Five older examples
changed: two long prompts (one also with one-hour writes) and three with
one-hour writes.
