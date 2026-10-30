# T3 Code and Fusebox usage reconciliation, 30 October 2026

Implementation follow-up: the user requested T3-style estimates after this
research. The [pricing change and rollout notes](usage-pricing-t3-2026-10-30.md)
supersede the earlier calculation recommendation below; raw returned tiers and
final usage revisions remain preserved.

The T3 screenshot combines **thebeast, The Dragon, and the XPS laptop**, all of which the user confirms use production Fusebox. Production already records their proxied traffic: **20,882 entries and $2,107.28 of known API-equivalent cost** are identifiable under Dragon/XPS client names. Collectors are not required for live requests passing through Fusebox.

The confirmed large differences are Fusebox's uncovered historical interval and T3's Codex speed premiums based on requested settings. A separate T3 Claude deduplication issue moves some estimates in the opposite direction by keeping an early output count. The exact combined screenshot has not been independently replayed from all three environments.

This investigation read production and local usage metadata without changing services, enrolling collectors, importing history, repricing records, or making inference requests. Production's dashboard rollups agree exactly with its canonical accounting entries for the comparison window.

The comparison starts at **1 October 2026, 00:00 Europe/Amsterdam** and stops before **30 October, 08:46 CET**, matching the dates and clock visible in the screenshot. The environment selection is user-confirmed; the screenshot's actual query and price settings were not captured, so this is a chosen comparison window rather than proof of its exact date and pricing scope.

| Source | Claude / Anthropic | Codex / OpenAI | Total | Processed tokens |
| --- | ---: | ---: | ---: | ---: |
| User's T3 screenshot | $22,694.84 | $12,835.89 | $35,530.73 | 80.7B, rounded in screenshot |
| Installed T3 pricing replay, thebeast only | $20,934.91 | $12,567.23 | $33,502.14 | 76,706,038,144 |
| Fusebox production, canonical entries | $21,739.29 | $6,374.01 | $28,113.31 | 71,163,823,309 |

Values are USD API list-price equivalents. Production's cost is its known priced subset; missing prices are excluded. Provider values and totals are rounded independently.

The production total is **$7,417.42, or 20.9%, below the screenshot**. The thebeast-only T3 replay is **$2,028.59 below the combined screenshot**. This compares different environment scopes; it is not evidence that $2,028.59 of usage is absent from Fusebox. Exact attribution requires the per-environment T3 summaries with the same window and pricing settings. The production client-name totals below are evidence of capture, rather than an exact replay of either remote T3 environment.

| Identified production client names | Accounting entries | Known cost | Unpriced entries |
| --- | ---: | ---: | ---: |
| `the-dragon`, `the-dragon-claude`, `the-dragon-codex` | 20,139 | $2,065.67 | 466 |
| `dell-xps-14` | 743 | $41.61 | 14 |

These amounts are already included in the production total. Earlier requests using shared keys, and other software using the same client credentials, prevent treating named-client totals as complete T3-only machine attribution.

1. **Production is missing the keeper-to-native transition interval.**

   The last retained legacy keeper event is **2 October, 15:52:16.709 CEST**. The first native proxy event is **7 October, 08:32:14.557 CEST**. Production has no entries for **3–6 October**. This was already documented when the legacy history was imported; the live database still has the same boundaries. See [the historical import record](../notes/keeper-history-import.md) and [the production logging follow-up](../notes/production-log-followup.md).

   The pinned T3 history contains **60,151 records, 10,523,545,994 processed tokens, and $4,400.35 of T3-priced usage** inside that uncovered interval. October 3–6 alone contain 56,214 records and $3,933.09. These are values at T3's current pricing rules, including any speed premiums, rather than a proposed historical bill or an amount that can automatically be added to Fusebox.

   Production has **zero configured local import roots and zero enrolled collectors**. Its accounting sources are native proxy capture and the imported keeper archive. Live capture applies to all clients using the proxy, including the remote T3 environments. Collector absence therefore does not explain missing live Dragon/XPS traffic. T3 scans local provider histories, while Fusebox also records other proxy clients, failed attempts, and internal requests. Deleted-file cache retention did not contribute any selected-window records in the thebeast replay.

2. **T3's requested Fast/priority speed can differ from the returned API tier.**

   The installed T3 parser reads Codex `thread_settings_applied.service_tier`, carries that setting through the transcript, and uses it to choose standard, Fast, or Ultrafast rates. Fusebox's capture reads `service_tier` from the actual response, and its saved pricing snapshot uses that returned value. The relevant Fusebox paths are [response capture](../src/usage/capture.rs) and [tier pricing](../src/usage/pricing.rs).

   Four durable Codex response IDs from October 27 were matched directly to production. All token categories agreed. All four production observations and price snapshots record **`default`, multiplier 1×**, while their corresponding T3 history records use **`priority` / Fast, multiplier 2×**. For example, one GPT-6.1 Sol response has 198 uncached input, 200,320 cache-read, and 735 output tokens. Fusebox estimates **$0.027778**; T3's Fast rates estimate **$0.055556** for those same tokens.

   The pinned T3 replay contains **$5,069.15 in Codex speed premiums** over its standard-rate equivalent. **$775.32 of that premium falls inside the uncovered interval above**, leaving $4,293.83 outside it. These values overlap the history-gap estimate and must not be added together without removing that overlap. Four matched calls establish the discrepancy mechanism, not that every premium in the month was unwarranted.

   A separate sample of 3,000 recent production OpenAI observations contained 2,875 returned `default` tiers, 124 `auto` tiers, and one missing tier priced under an explicit standard-tier assumption. All had a 1× multiplier. Requested client speed is not proof that the upstream returned that API tier; the findings do not establish why the requested speed was not reflected in those responses.

3. **T3 also understates some Claude output by retaining an early revision.**

   Among **178,337 Claude response IDs shared by the pinned T3 history and production**, all three input categories agree for every response. Output agrees for 160,260 responses. In **18,077 responses**, T3's retained output is lower; none in this matched set is higher. The difference is **29,295,975 output tokens**.

   The installed Claude parser creates a deduplication key from message/request identity, and the scan/aggregation path keeps the first record. Local transcripts prove that usage can be revised later for that same message. One matched Claude Fable response first reports **7 output tokens**, then **38,137**; T3 retains 7 while Fusebox records 38,137. Another first reports 3, then 36,706.

   Over the matched responses, production estimates $18,765.47 and the T3 replay estimates $18,143.91: **$621.57 lower in T3**. The output difference alone is $620.83 at T3's rates; the small remainder reflects other pricing details. This offsets part of T3's higher totals elsewhere. It is evidence against treating either dashboard's overall total as a simple superset of the other.

4. **Unpriced records are visible omissions from known cost.**

   Production has **313,641 accounting entries**, of which **6,259 are unpriced**. There are 6,186 entries with missing token usage, including **5,979 OpenAI entries**, and 73 additional dated Haiku model entries whose model ID has no catalogue match. Unknown usage cannot be assigned a defensible dollar value from these records alone.

   Of the 5,979 missing-usage OpenAI entries, **5,778 have status 499 (client disconnected)**. The remainder comprise 192 requests with other non-2xx statuses and nine with no status. **No unpriced HTTP 2xx entry lacks token usage**, across either provider; the 69 unpriced 2xx responses all have complete usage and the unsupported dated Haiku model ID. A disconnect does not establish zero provider consumption.

   Of the missing-usage OpenAI entries, 5,869 retain a response ID. A bounded metadata-only scan of all 683 relevant thebeast Codex history files found **78,947 durable completed-response usage records and no matching response IDs**. The available thebeast histories therefore did not provide completed usage with which to repair those entries. This check does not cover Dragon or XPS histories.

   The current writer and the two previous writer sessions have **zero dropped records, rejected records, and writer errors**. The preceding two sessions closed cleanly. This verifies current recording health; it does not repair or disprove the older history gap. The rollup comparison found no dashboard aggregation discrepancy.

The practical historical correction is to reconstruct **only the uncovered interval** from retained native usage metadata, with stable response identities and deduplication, and preserve its provenance as imported history. Enabling a collector over all history needs particular care: the keeper archive lacks reliable response IDs, so replaying its covered dates could duplicate old events. A collector would transport historical metadata; live Dragon/XPS proxy traffic is already recorded without one. This investigation did not perform a backfill.

For the ongoing comparison, distinguish **requested speed** from **returned service tier**. Fusebox should retain its response evidence rather than apply a 2× multiplier solely to imitate T3's display. T3's Claude history reducer should accept more complete revisions of the same response. Coverage dates, unknown usage, and pricing assumptions should remain visible alongside totals.

The [sanitized receipt](usage-reconciliation-2026-10-30.json) records the totals, fixed window, evidence hashes, identity-match counts, and example calculations. Private metadata snapshots and the offline replay are under the gitignored `target/usage-reconciliation-20261030/`, with owner-only permissions. Source code was extracted from the **installed T3 0.1.3 binary**, rather than inferred from the older T3 research checkout. At investigation time, production ran **Fusebox 0.11.0+gui.eb129cf97dfb**, using catalogue `2026-10-27.2`.

The user subsequently requested T3-compatible pricing and explicitly authorized
a forced production rollout. It is now deployed, including audited historical
repricing and the dated Haiku fix; see the
[production deployment record](usage-pricing-production-2026-10-30.md). The
research figures above preserve their original window and pre-deployment prices.
