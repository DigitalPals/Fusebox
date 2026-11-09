# Claude Fable 5.2 and Mythos 5.2, 2026-11-09

Anthropic released Claude Fable 5.2 (`claude-fable-5-2`) and Claude Mythos 5.2
(`claude-mythos-5-2`) on 9 November 2026. Sources, all read on the release day:
the [model page](https://platform.claude.com/docs/en/models/fable-5-2/overview),
[what's new](https://platform.claude.com/docs/en/models/fable-5-2/whats-new-fable-5-2),
[pricing](https://platform.claude.com/docs/en/about-claude/pricing) and
[fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode).

## What Anthropic published

- **Access.** Fable 5.2 is open to every API customer. Mythos 5.2 is the same
  model with different safeguards, for organizations in Anthropic's
  verification programs only. Production's Claude accounts listed
  `claude-fable-5-2` in their model lists the same day; none lists Mythos 5.2.
- **Limits.** 1M context (default and maximum), 128K output, the tokenizer
  introduced with Opus 4.7.
- **Thinking.** Adaptive thinking is always on. `thinking.type` `disabled` and
  `enabled` with `budget_tokens` return 400. Effort runs `low` to `max`, default
  `high`.
- **Rejected as on Fable 5.1:** forced `tool_choice` (`any`, `tool`),
  `temperature`/`top_p`/`top_k`, and a final assistant message (prefill).
- **New 400s compared with Fable 5.1:**
  - the `computer_20251124` tool (use `computer_toolset_20260801`);
  - an advisor other than Fable 5.2 or Mythos 5.2 when the executor is one of them.
- **Preserved thinking:**
  - the prefix check (editing anything before a thinking block gives a 400,
    "The block is bound to a different conversation") applies to every account;
    on Fable 5.1 it applies only to accounts created on or after 2026-08-31;
  - only Fable 5.2 and Mythos 5.2 read Fable 5.2 thinking blocks;
  - the blocks work only in the account that produced them. Another account's
    request still succeeds, but the API drops the blocks first.
- **Prices (USD per million tokens):** input 8, output 40, 5-minute cache write
  10, 1-hour cache write 16, cache read 0.20 (0.025× input, as on 5.1). Batch
  halves input and output. US-only `inference_geo` adds 10%. Fable 5.1 stays
  at 10 / 50.
- **Fast mode.** `speed: "fast"` is generally available on Fable 5.2 and Mythos
  5.2 at 16 / 80, double the standard price, with no beta header. Fable 5.1,
  Mythos 5.1 and Fable 5 still have no fast mode.

## What Fusebox changed

- `claude-fable-5-2` is in the Claude provider's built-in model list. Mythos
  5.2, like Mythos 5.1, appears only on accounts whose model list includes it.
- Translated requests to either model turn a forced `tool_choice` into `auto`,
  keeping the parallel-call limit. Sampling, prefill and disabled thinking were
  already handled by family name (`fable`, `mythos`, `fable-5`, `mythos-5`).
- `speed: "fast"` is kept for these two models. Before, Fusebox removed it as
  unsupported. Subscriptions still go at standard speed unless
  `claude-subscription-fast-mode` is on.
- Provider rate card `2026-11-09.1` and the default T3 catalogue
  `t3-2026-11-09.1` price both models, fast mode included.
  - LiteLLM doesn't list either model yet, so the T3 rows are the
    published-rate fallback (`published_provider_rate_fallback`).
  - `scripts/update-usage-pricing.py` now keeps a fallback row's Fast and
    Ultrafast multipliers. No existing fallback row has one, and the script
    rebuilds these rows exactly from the 2026-11-09 LiteLLM document.
  - Unpriced Fable 5.2 requests stored before the upgrade are priced at startup.

## What needed no change

- The Claude subscription weekly Fable cap (`week fable`) matches any model
  containing `fable`. Production's usage reading still reports it as "Fable",
  with no separate 5.2 limit.
- Effort and the 64K default output cap follow the 5.x rules already in place.
  Discovery reads "thinking can't be disabled" from Anthropic's model list.

## Not done, and why

- **Devin.** Devin's model list on the release day still ends at Fable 5.1,
  and its model IDs for new Claude releases can't be guessed.
- **Computer use and the advisor tool.** Fusebox passes native tool
  definitions through unchanged. A client still sending `computer_20251124` or
  an older advisor gets Anthropic's 400, as it would directly.
- **Moving between accounts.** Because Fable 5.2 thinking blocks only work
  in the account that produced them, a conversation that moves to another
  account loses its earlier reasoning (and its prompt cache). Session affinity
  already keeps a conversation on one account while that account is
  available. No live multi-turn Fable 5.2 conversation was run for this change.
  After deployment, watch for 400s that say "bound to a different
  conversation", which would mean a Fusebox rewrite changed an earlier turn.
