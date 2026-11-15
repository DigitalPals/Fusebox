# Tools, endpoints and models

## Connect your tools

In **Config → Clients**, choose a tool and a model, then copy or download its setup. Fusebox includes reported context/output limits and image support where the client accepts them. The picker respects each named client's allowed models and keeps the exact public alias or prefix. Unknown capabilities stay marked unknown; Zed asks for a documented context limit before generating settings.

Codex setups can use `/v1/codex/models.json?model=<public-model-id>` when discovery has supplied a complete Codex model template. The endpoint uses the same client authentication and allowed-model policy as `/v1/models`, preserves the provider's tools/instructions/reasoning metadata, and removes upgrade links that would bypass the alias. It filters incompatible minimum client versions and enforces Codex's 1 MiB catalog limit. Merge the downloaded TOML's top-level settings before its provider table and restart Codex. An existing `model_catalog_json` setting takes precedence over the remote catalog; remove it to use Fusebox's catalog. See the [official gateway documentation](https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway#distribute-a-local-catalog).

**Claude Code**

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:8317
export ANTHROPIC_AUTH_TOKEN=<this machine's client key, or anything if you have no clients>
claude
```

Want GPT in Claude Code? `export ANTHROPIC_MODEL=gpt-6-astra`.

**Codex** in `~/.codex/config.toml`

```toml
model = "gpt-6-astra"
model_provider = "fusebox"

[model_providers.fusebox]
name = "Fusebox"
base_url = "http://127.0.0.1:8317/v1"
wire_api = "responses"
env_key = "FUSEBOX_KEY"   # only needed once you have clients
```

**OpenAI SDK**, or any tool with a custom OpenAI base URL

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8317/v1", api_key="<key>")
client.chat.completions.create(
    model="claude-sonnet-5-5",
    messages=[{"role": "user", "content": "Hello"}],
)
```

**curl**

```sh
curl http://127.0.0.1:8317/v1/messages \
  -H "x-api-key: <key>" -H "content-type: application/json" \
  -d '{"model": "gemini-3.8-flash", "max_tokens": 512, "messages": [{"role": "user", "content": "Hello"}]}'
```

## Endpoints

| Endpoint | Speaks |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI Chat Completions |
| `POST /v1/responses` · `GET /v1/responses` (WebSocket) | OpenAI Responses |
| `POST /v1/responses/compact` | Responses compaction (ChatGPT, xAI) |
| `POST /backend-api/codex/responses` (+ WebSocket) | Codex's native path |
| `POST /v1/messages` · `POST /v1/messages/count_tokens` | Anthropic Messages |
| `POST /v1beta/models/{model}:generateContent` · `:streamGenerateContent` | Gemini |
| `POST /v1/images/generations` · `POST /v1/images/edits` (JSON or multipart) | OpenAI Images |
| `POST /v1/videos/generations` · `/edits` · `/extensions` · `GET /v1/videos/{id}` | xAI video |
| `POST /v1/completions` | Legacy OpenAI completions |
| `POST /v1/embeddings` | OpenAI embeddings, from OpenAI, OpenAI-compatible, Gemini and Vertex keys |
| `GET /v1/models` · `GET /v1beta/models` | Model lists, model groups included |
| `GET /v1/fusebox/status` | Pool-level quota status for client keys (`?format=line` for one text line) |

Clients authenticate with `Authorization: Bearer`, `x-api-key`, `x-goog-api-key` or `?key=`.

`GET /v1/fusebox/status` answers any client key (collector keys excluded) with pool-level status only — combined quota pools, ready/cooling account counts and next resets, with no account ids, labels or emails — so a status line or a coding agent can check how much quota is left before starting work. `?format=line` returns one text line such as `claude week 31% · 4/5 accounts ready`, ready for a Claude Code `statusLine` command: `curl -s -H "authorization: Bearer $FUSEBOX_KEY" "http://127.0.0.1:8317/v1/fusebox/status?format=line"`. A status read is not a request: it ignores model lists, programs, rates and budgets, and is never recorded in usage. A named client's expiry and address list still apply.

Embeddings accept `encoding_format: float` (default) or `base64`; base64 encodes little-endian float32 values. Google adapters preserve input order, split Vertex batches to one text for Gemini embedding models or five for older text embedding models, and disable automatic truncation. A failed later chunk returns an error without replaying completed chunks; their usage is still recorded. Gemini embedding token counts are labelled estimates in the usage database. Models without a known price remain unpriced and cannot be treated as free by monthly budgets.

Google embedding chunks run sequentially by default. `embeddings.concurrency` enables 1–16 chunks at once; Requests shows account attempts separately from chunks. `embeddings.cache-seconds` enables exact in-memory result reuse (default `0`, off), isolated by client, account credential identity, resolved model and every request option. `embeddings.cache-bytes` caps serialized results (default 16 MiB, at most 1,000 entries); memory overhead is additional. Hits return `fusebox_cache_hit: true` and zero provider usage. Changing these settings clears the result cache. These controls are under Connections in Config.

JSON API requests and multipart image edits support `Content-Encoding: gzip` or `zstd`, as well as uncompressed bodies. Authentication runs before decompression; both compressed and decoded bodies are limited to 256 MiB. Unsupported or stacked encodings return 415, malformed compressed bodies return 400, and oversized bodies return 413. Encoding and JSON validation failures appear in request statistics and logs without a provider attempt.

## Providers

| Provider | Sign-in | Models | Speaks upstream |
| --- | --- | --- | --- |
| Claude | OAuth (Pro / Max / Ultra) or API key | `claude-*` | Anthropic Messages |
| Codex / ChatGPT | OAuth (Plus / Pro / Team) or OpenAI API key | `gpt-*`, `o*`, `codex-*`, `gpt-image-*` | Responses (+ WebSocket) |
| Gemini | API key | `gemini-*`, `gemma-*` | Gemini |
| Vertex AI | Service account or express API key | `gemini-*`, `imagen-*` | Gemini |
| Antigravity | Google OAuth | Gemini and Claude models such as `gemini-3.8-flash-high`, `claude-opus-4-6-thinking` | Cloud Code (Gemini) |
| Grok (xAI) | Device code (SuperGrok / X Premium) or API key | `grok-*`, `grok-imagine-*` | Responses |
| Kimi | Device code (Kimi Code) or API key | `kimi-*` | Chat, Anthropic or Responses, whichever the client speaks |
| Meta | Device code or API key | `muse-*` | Responses |
| Devin | OAuth (Devin / Windsurf) | Claude, GPT, Gemini, Grok, Kimi, GLM, DeepSeek and SWE models | Connect protobuf |
| OpenAI-compatible | API key or none (OpenRouter, Ollama, LM Studio, vLLM, …) | whatever you list, with optional aliases | Chat Completions |

Model names are forgiving: `gpt-6-1-sol` finds `gpt-6.1-sol`, and `gemini-3-8-flash` finds Antigravity's `gemini-3.8-flash-high` when that's the account you have.

When more than one provider has a model, the vendor's own accounts answer first and Antigravity or Devin take the overflow when those are rate limited. To choose a provider yourself, prefix the model: `antigravity/claude-sonnet-4-6`, `devin/gpt-6-astra`, `vertex/gemini-3.1-pro`.

## New models

A [model support review dated 2026-10-27](model-support-2026-10-27.md) adds GPT-6.1 Terra and verified releases from the other providers, with their request compatibility rules and source evidence.

A model a provider releases after your Fusebox version usually works straight away: requests go by name family, so `claude-*` reaches Claude, `gpt-*` reaches Codex, `gemini-*` Gemini, and so on.

Fusebox also asks the providers what they serve, so new models show up in `/v1/models` and on the dashboard by themselves. Signed-in Claude and Codex accounts each fetch their model list once every 6 to 7 hours, retrying after 15 minutes (then 30, 60, …) when that fails. The lists are kept in `<auth-dir>/.model-lists.state`, so a restart fetches nothing. Codex marks a changed list on its ordinary responses, which brings that account's next check forward, but never to within 10 minutes of the last one. Claude's list also says whether a model can turn thinking off and which effort levels it takes, and requests follow it. Antigravity has always fetched its own list.

For everything else (Gemini, Grok, Kimi, Meta, Devin, API keys), or when you don't want to wait, use **Add model** on the Models page, or the `+ Add` chip at the end of a family. It writes the model to `extra-models` in `config.yaml`:

```yaml
extra-models:
  claude: [claude-opus-6]
```

Every account of that provider then serves and lists it, apart from API keys with their own `models` list. Once a Fusebox release or the provider's model list includes it, Fusebox removes the entry from `config.yaml` and says so on the Models page. **Check Claude and Codex for new models** in the Add model form schedules eligible signed-in Claude and Codex accounts, skipping any checked in the last 10 minutes. Manual and automatic checks share four slots. If another check is already running, the form says so and its results appear automatically when it completes.

## Model capabilities

Discovery retains provider-reported context and output token limits, input/output modalities, tool support, and reasoning levels. `/v1/models`, `/v1beta/models`, and the dashboard expose known values. Metadata follows the actual upstream model through aliases and prefixes. A shared model name or fallback group uses the smallest limits and common capabilities across all its possible routes; an unreported route leaves that information unknown.

For custom models, use **Config → Models → Model metadata overrides**, or `model-capabilities` in `config.yaml`. Overrides apply to the exact public model ID and update client setup and model lists. Removing an override restores discovery. These are client metadata; they do not change the provider's actual limits or add capabilities to a model.

```yaml
model-capabilities:
  team/coding:
    context_window: 128000
    max_output_tokens: 16000
    input_modalities: [text, image]
    tool_calling: true
```

## Reasoning effort, from the model name

Append an effort level or a token budget to any model:

```text
gpt-6-astra(high)                  effort level
claude-opus-5-5(max)               adaptive thinking at max effort
claude-sonnet-4-5-20250929(16000)  thinking budget in tokens
gemini-2.5-pro(0)                  thinking off
devin/claude-opus-5-5(high-fast)   Devin fast mode at high effort; (fast) uses the default effort
```

## Images

```sh
curl http://127.0.0.1:8317/v1/images/generations \
  -H "authorization: Bearer <key>" -H "content-type: application/json" \
  -d '{"model": "gpt-image-2", "prompt": "a lighthouse at night, film photo", "size": "1536x1024"}'
```

`gpt-image-*` runs on a ChatGPT account through Codex's image tool (or an OpenAI key), `grok-imagine-*` on xAI, `imagen-*` on Vertex, and Gemini image models such as `gemini-3.1-flash-image-preview` on Gemini, Vertex or Antigravity. Image models also work in chat: the picture comes back as an image part in whatever format the client speaks.
