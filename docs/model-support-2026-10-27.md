# Model support refresh, 2026-10-27

Checked the providers' current documentation and, where their subscription
model IDs are not documented, their published client artifacts. This is a
catalog and protocol review with local mock-provider verification. It does
not establish availability on any particular subscription or API account.

## OpenAI

Added `gpt-6.1-terra` to the Codex/OpenAI fallback catalog. The official
[model page](https://developers.openai.com/api/docs/models/gpt-6.1-terra)
and [migration guide](https://developers.openai.com/api/docs/guides/latest-model#gpt-61-terra)
specify text/image input, text output, a 1,050,000-token context window,
922,000 maximum input tokens, and 128,000 maximum output tokens. Efforts are
`none`, `low`, `medium` (default), `high`, `xhigh`, and `max`. Tool calling
requires Responses. Fusebox already sends this provider's requests through
Responses, including requests translated from Chat Completions or Messages.

The integration test found that translated `max` requests were reduced to
`xhigh`. GPT-5.6 and GPT-6 now retain `max`, as verified against the official
model pages for [GPT-6 Astra](https://developers.openai.com/api/docs/models/gpt-6-astra),
[GPT-6.1 Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol),
[GPT-6 Sol](https://developers.openai.com/api/docs/models/gpt-6-sol),
[GPT-6 Luna](https://developers.openai.com/api/docs/models/gpt-6-luna),
[GPT-5.6 Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol),
[GPT-5.6 Terra](https://developers.openai.com/api/docs/models/gpt-5.6-terra),
and [GPT-5.6 Luna](https://developers.openai.com/api/docs/models/gpt-5.6-luna).
Older models and other Responses providers retain their existing mapping.

Bundled rate catalog `2026-10-27.1` adds Terra's
[published prices](https://developers.openai.com/api/docs/pricing):
$1/M input, $5/M output, $0.05/M cached reads, $1.25/M 30-minute cache writes,
and $2.50/M 12-hour cache writes. Above 272,000 total prompt tokens, all
input/cache rates double and output costs $7.50/M for the whole request.
Batch/Flex halve the rates; Fast/Priority double them. Eligible US regional
processing adds 10%. No Terra Ultrafast rate is published, so it remains
unpriced. Existing rates and their earlier verification date are retained.

Also added [GPT Image 2.5 Flare](https://developers.openai.com/api/docs/models/gpt-image-2.5-flare)
and [Sunburst](https://developers.openai.com/api/docs/models/gpt-image-2.5-sunburst).
The existing Images API and Responses image-tool adapters already forward the
selected model and its quality settings. Their image-token prices are not
represented as ordinary text-token prices.

Capabilities and complete Codex templates continue to come from account
discovery or explicit `model-capabilities` configuration. API documentation
limits are not substituted for an account's reported context window, and a
sparse Codex template is not fabricated for the new model.

## Other providers

| Provider | Verified additions and implementation |
| --- | --- |
| Claude | Added `claude-haiku-5-5`. Translated requests omit sampling parameters and use adaptive thinking. Forced tool choice remains supported with adaptive thinking on this model. Its existing bundled prices were already correct, including the increase above 100K total prompt tokens; boundary, cache lifetime, Batch and regional tests now cover them. |
| Gemini | Added `gemini-4-argon`, stable `gemini-3.1-flash-image`, `gemini-3.1-flash-lite-image`, and `gemini-nano-banana-2.1`. Gemini 4 uses `thinkingLevel`, including on native requests with effort suffixes, rather than the legacy token budget. Nano Banana 2.1 receives image response modalities and no generated thinking configuration. |
| Vertex | Added `gemini-4-argon`, with the same Gemini 4 effort translation. Its unified context is 1,048,576 tokens; the default synchronous output limit is 65,536. The documented long-decode option is distinct from that default. |
| Antigravity | Added the verified Gemini 4 Argon `high`, `medium`, `low` IDs and Claude Opus/Sonnet 5.5 `max`, `high`, `medium`, `low` IDs. They remain aggregator-specific IDs; first-party providers do not receive these suffixed names. Existing account discovery still determines additional availability and renamed models. |
| Kimi | Added `kimi-k3.1-pro` and `kimi-k3.1-pro-highspeed`. The Chat adapter omits fixed sampling parameters, forwards output limits as `max_completion_tokens`, preserves an explicit completion limit, and maps disabled reasoning to the documented `low` effort because K3.1 always thinks. Valid requested efforts and omitted defaults are retained. |
| xAI | Grok 4.7 is already present. Added the missing `grok-imagine-image-quality` and `grok-imagine-video-1.5-lite` entries; the existing image/video adapters forward their selected IDs. |
| Meta | The current Muse client still contains Muse Spark 1.3 and 1.2. No newer public inference model was verified; the existing Spark catalog is retained. Internal/contributor model strings are not treated as public releases. |
| Devin | Added `gpt-6-1-sol`, `claude-sonnet-5-5`, and `claude-haiku-5-5` with their verified effort-to-UID mappings. The published artifact did not contain Terra or Gemini 4 IDs, so neither is advertised on Devin. |
| OpenAI-compatible | Model lists remain operator-configured, since OpenRouter, Ollama, vLLM and other compatible endpoints have independent catalogs and availability. Their existing arbitrary-ID routing needs no new vendor-name rule. |

Sources actually fetched:

- Claude [current models](https://platform.claude.com/docs/en/models/overview),
  [Haiku 5.5](https://platform.claude.com/docs/en/models/haiku-5-5/overview),
  [migration guide](https://platform.claude.com/docs/en/models/haiku-5-5/migration-guide),
  and [pricing](https://platform.claude.com/docs/en/about-claude/pricing).
- Google [Gemini catalog](https://ai.google.dev/gemini-api/docs/models),
  [Gemini 4 Argon](https://ai.google.dev/gemini-api/docs/models/gemini-4-argon),
  [release notes](https://ai.google.dev/gemini-api/docs/changelog),
  [Vertex catalog](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/learn/models),
  and [Vertex Argon](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/models/gemini/4-argon).
- [Kimi K3.1 Pro API guide](https://platform.kimi.ai/docs/guide/kimi-k3-1-pro-quickstart).
  Its documented `low`, `high`, `max` efforts and fixed parameters govern the
  new Chat normalization; no new behavior is inferred for its other native
  compatibility endpoints.
- [xAI models](https://docs.x.ai/developers/models).
- [Antigravity models](https://antigravity.google/docs/models) and its
  [official update manifest](https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml).
  Inspected the language server in the published 2.26.0 macOS ARM archive after
  verifying SHA-512
  `DIrp/E7wW3al3PifimbaU/FEE1ZaiQTKGJvAqhuKP85QCnaeku0e5o0+btit2ZvcZoHdvtPFLDmWboSzGHqu4A==`.
- [Devin CLI models](https://docs.devin.ai/cli/models) and its
  [release manifest](https://static.devin.ai/cli/current/manifest.json).
  Inspected the 3000.11.228 macOS ARM binary after verifying archive SHA-256
  `9287dfa4e1c666e7cebb680f851694419edd40996c05be6e0a039a027c4965bb`.
- Meta's [stable channel](https://api.meta.ai/muse-code/channels/muse-stable)
  and its linked manifest for 1.4.13-R6996.1. Inspected the published Linux ARM
  binary after verifying SHA-256
  `c7f278b5f12862f5721c73433976256aa2a8abd433a12da0e484d30b08c024ff`.
  Public documentation URLs tried for Meta's inference catalog were unavailable;
  artifact inspection does not establish account entitlements or an exhaustive
  server model list.

Voice, live sessions, Google video/music generation and other dedicated APIs
are not advertised as ordinary chat models. The existing supported wire
formats and provider authentication remain in place. Released client artifacts
were inspected as data, not executed; their identity versions were not changed
as part of this model catalog refresh.

## Verification and development restart

All checks passed with Rust 1.96.1: `cargo test --locked` (732 passed,
one existing ignored test), `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, and
`cargo build --release --locked`. The Node suite passed all 133 tests and
the Python suite passed all 46 tests. `git diff --check` also passed.
The release binary's SHA-256 is
`f38ebbee85c6943376734c3018f6cd8b38cf2d72d7ddc37e8baf2fdfc1a5828e`.

No existing Fusebox development service was running on the host. A temporary
development instance of the rebuilt release was started and restarted under
an `agent-resource` lease, retaining its configuration and database across
the restart. Local mock providers verified health, 11 new public catalog IDs,
native and translated Terra requests at `none` and `max`, both GPT Image 2.5
model selections, and the new pricing catalog version. No real provider
inference was performed; account-specific availability remains unverified.

Both smoke-test leases were closed and checked: no temporary resources,
preserved volumes, or cleanup errors remained. Downloaded client binaries
and archives were removed. No persistent service was installed or deployed.
