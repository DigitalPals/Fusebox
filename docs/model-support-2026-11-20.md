# Gemini 4 and Antigravity, 20 November 2026

Google added three Gemini 4 models in November: Carbon (10 November), Hydrogen
(12 November) and Neon (18 November, now Antigravity's default). Fusebox listed
none of them, sent no `xhigh` thinking level, and had no Google prices at all.
This update fixes all three and brings the Antigravity identity up to 2.33.0.

## Models

| Where | Added |
| --- | --- |
| Antigravity | `gemini-4-neon-{xhigh,high,medium,low}`, `gemini-4-carbon-{high,medium,low}`, `gemini-4-hydrogen-{xhigh,high,medium,low}` |
| Gemini (AI Studio) | `gemini-4-neon`, `gemini-4-carbon`, `gemini-4-hydrogen` |
| Vertex | `gemini-4-neon`, `gemini-4-carbon`, `gemini-4-hydrogen` |

A bare `gemini-4-neon`, `gemini-4-carbon` or `gemini-4-hydrogen` sent to an
Antigravity-only setup goes to the `-high` variant, Antigravity's default for all
three. `-xhigh` (Antigravity's "Extra") is chosen by name. Models an account
reports itself are still added and routed as before.

This also fixes a routing mistake seen in the before/after test. With a Gemini API
key and an Antigravity account, a bare `gemini-4-neon` used to become
`gemini-4-neon-high` (the name Antigravity reported) and was then sent to the
Gemini API key, which has no such model. Now the Gemini key gets `gemini-4-neon`,
and `-high` names stay on Antigravity. A future model that only Antigravity's own
list reports, with an effort suffix, could still take that path until it is added
here.

The Carbon and Hydrogen IDs are compiled into the Antigravity 2.33.0 language
server. Neon launched the day after that build, so its IDs come from Antigravity's
model page (`agy --model gemini-4-neon-xhigh`, efforts `low` to `xhigh`) and
follow Hydrogen's. Vertex's model page lists Carbon and Hydrogen; its pricing page
also lists Neon.

Google removes Claude Sonnet and Opus 4.6 and GPT-OSS 120B from Antigravity on
30 November 2026, and Fusebox stopped offering them on 20 November. An Antigravity
account whose own model list still reports them serves them until then. A Claude or
Codex account never takes `claude-opus-4-6-thinking` or `gpt-oss-120b-medium`:
they are Antigravity's names, not Anthropic's or OpenAI's. `claude-sonnet-4-6` is
also Anthropic's own model and stays on Claude accounts.

Not changed:

- Gemini 3.7 Flash, 3.6 Flash and 3.1 Pro stay listed: Antigravity now offers them
  to Enterprise accounts only, and an account's own model list shows what it has.
- The Orchestrator options are not models. They plan with one model and hand the
  work to Hydrogen inside the Antigravity app; the API has no ID for them.

## Thinking level

Gemini 4 Neon and Hydrogen accept `xhigh`; Carbon and Argon stop at `high`
([thinking guide](https://aistudio.google.com/docs/thinking)). Fusebox used to
send `high` for every request that asked for more. Now `xhigh` and `max` become
`xhigh` on Neon and Hydrogen (on Gemini, Vertex and Antigravity alike) and stay
`high` elsewhere. The 2.33.0 language server has the matching
`THINKING_LEVEL_XHIGH` value. `minimal` still becomes `low`, which Gemini 4 and
3.8 Flash require.

## Prices

Rate card `2026-11-20.1` and default catalogue `t3-2026-11-20.2` add Google's list
prices (USD per million tokens):

| Model | Input | Cached input | Output |
| --- | --- | --- | --- |
| Gemini 4 Neon | 3.00 | 0.15 | 15.00 |
| Gemini 4 Hydrogen | 1.50 | 0.075 | 7.50 |
| Gemini 4 Carbon, Argon, to 30 Nov 2026 | 2.00 | 0.10 | 10.00 |
| Gemini 4 Carbon, Argon, from 1 Dec 2026 | 4.00 | 0.20 | 20.00 |
| Gemini 3.8, 3.7, 3.6 Flash, to 31 Dec 2026 | 0.75 | 0.075 | 3.75 |
| Gemini 3.8, 3.7, 3.6 Flash, from 1 Jan 2027 | 1.50 | 0.15 | 7.50 |
| Gemini 3.5 Flash-Lite | 0.30 | 0.03 | 2.50 |
| Gemini 3.1 Pro | 2.00 (4.00) | 0.20 (0.40) | 12.00 (18.00) |

Gemini 3.1 Pro's prices in brackets apply to the whole request when its prompt,
cached tokens included, is above 200,000 tokens. Gemini 4 has no such step and
only implicit caching, so it has no cache-write or storage price. On the rate
card, Batch and Flex halve every price and Priority is 1.8 times; Flash-Lite has
only Batch (its other cached prices break the ratio), 3.1 Pro only Priority
(Batch keeps the cached price), and 3.7 Flash, sold on Vertex only, none.

Sources: [Gemini API pricing](https://aistudio.google.com/docs/pricing) and, for
3.7 Flash, [Vertex pricing](https://cloud.google.com/vertex-ai/generative-ai/pricing),
both read on 2026-11-20. Vertex charges the same on its global endpoint and 10%
more on regional ones; Fusebox prices the global rate, as before. LiteLLM's file
from this morning (SHA-256 `d342612f…`) already lists today's prices and agrees
with them; the dated rows carry the announced changes, which the catalogue
script now keeps (it used to keep only dated speed changes).

### Which price a record gets

A record's provider decides its rate card. Gemini and Vertex records use Google's
prices. Antigravity records drop the effort from the name
(`gemini-4-neon-xhigh` → `gemini-4-neon`, `claude-opus-5-5-max` →
`claude-opus-5-5`, `claude-opus-4-6-thinking` → `claude-opus-4-6`) and use the
maker's price, as Claude and ChatGPT subscriptions already do. Claude on
Antigravity reports its usage the Gemini way, without cache writes, so it is an
estimate labelled `antigravity_claude_priced_at_anthropic_rate`. GPT-OSS,
`gemini-pro-agent`, `gemini-3-flash` and the Gemini 2.5 models have no rate and
stay unpriced, as do models with separate audio prices (3.1 Flash-Lite, 3 Flash).

Older Gemini, Vertex and Antigravity records that were unpriced only for lack of a
rate are priced when the new release starts, as with any catalogue update.

The dashboard's account test uses an account's cheapest priced model. With these
prices that becomes `gemini-3.5-flash-lite` on Gemini, Vertex and Antigravity
accounts (the 2.33.0 language server has that ID), rather than the first "flash".

## Antigravity identity

The bundled release is now 2.33.0 (Fusebox still reads the current one from
Google's update manifest every three hours). The user agent now has the form the
language server sends since 2.23.0:

    antigravity/hub/2.33.0 (aidev_client; os_type=darwin; arch=arm64; cl=1023160026)

`cl` is the build changelist stamped into each release's language server
(`changelist 1023160026 with baseline 1022452901` in 2.33.0). Fusebox knows it only
for reviewed releases; when the manifest names a newer release, the `cl` part is
left out rather than pair the new release with an older build's number. Reviewing a
release now includes reading its changelist (see
[client compatibility](client-compatibility.md#antigravity-2330)).

## Evidence

- Antigravity [models](https://antigravity.google/docs/models),
  [plans](https://antigravity.google/docs/plans) and
  [changelog](https://antigravity.google/docs/changelog/), read 2026-11-20.
- Gemini API [models](https://aistudio.google.com/docs/models),
  [thinking](https://aistudio.google.com/docs/thinking) and
  [pricing](https://aistudio.google.com/docs/pricing); Vertex
  [models](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/models)
  and [pricing](https://cloud.google.com/vertex-ai/generative-ai/pricing).
- The 2.33.0 macOS ARM archive from the official update manifest, SHA-512
  `akGKBoAWOxZmddrQyvSHDxmnCtaaLnz7jaBTDg+8g8vG/lytBldvIkHRPxXA+WWsm6Xq2FP+fSzWD/BKFmsBvQ==`
  verified, inspected as data and not run.
- CLIProxyAPI's Antigravity registry (`gemini-4-neon-xhigh`,
  `gemini-4-hydrogen-xhigh`, `gemini-4-carbon-high`) and its user-agent capture
  ([#6619](https://github.com/router-for-me/CLIProxyAPI/issues/6619),
  [#7659](https://github.com/router-for-me/CLIProxyAPI/issues/7659)), which agree.
- Eight new golden pricing examples, evaluated by T3 Code 0.1.11's own pricer with
  the same LiteLLM file, in `tests/fixtures/usage/t3-pricing.json`.

No real Antigravity, Gemini or Vertex account was used: production has none. The
new IDs, the `xhigh` level on Antigravity and the new user agent are checked
against Google's documents and the published app, not against live traffic.
