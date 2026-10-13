# OpenRouter and OpenCode (Zen) as first-class providers

Status: research for the provider work. Facts are marked **confirmed** (read on the cited page or
in this repo) or **unconfirmed** (not stated on any page read; the code must not rely on them
without a check).

## OpenRouter

| Item | Finding | Status |
| --- | --- | --- |
| Base URL | `https://openrouter.ai/api/v1`; chat at `POST /chat/completions` | confirmed (quickstart) |
| Auth | `Authorization: Bearer <key>` | confirmed (quickstart) |
| Attribution | `HTTP-Referer: <site>` and `X-OpenRouter-Title: <name>`, both optional, for leaderboards | confirmed (quickstart). The older `X-Title` name is not checked. |
| Model list | `GET https://openrouter.ai/api/v1/models`; the response `data[]` has `id`, `name`, `context_length`, `pricing` (per-token strings) | endpoint and `pricing` confirmed (quickstart, and `prices.rs` already parses it). Full field list unconfirmed. |
| Id format | `vendor/model`; `~vendor/alias-latest` for latest-resolution aliases (e.g. `~openai/gpt-sol-latest`) | confirmed (quickstart). `:free` and `:nitro` suffixes: `:free` is confirmed by the rate-limit page; `:nitro` unconfirmed. |
| Anthropic Messages | Claude Code connects with `ANTHROPIC_BASE_URL=https://openrouter.ai/api`, `ANTHROPIC_AUTH_TOKEN=<key>`, `ANTHROPIC_API_KEY=""`, so OpenRouter accepts Messages input. The exact path `/api/v1/messages` is inferred from Claude Code's own `/v1/messages` suffix. | base URL and auth confirmed (Claude Code cookbook). Path inferred. Per-model support unconfirmed. |
| Reasoning | Request field `reasoning: {effort: "high"}`; responses carry `message.reasoning` and `message.reasoning_details` | effort shape confirmed (reasoning-tokens guide). Full value list unconfirmed. |
| Usage | `response.usage` is always included | confirmed (usage-accounting cookbook). Whether a `usage.cost` field is returned is unconfirmed; the code must treat cost as optional and fall back to the list price. |
| Rate limits | Headers `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset`. 429 = rate limit; 402 = credits, including the in-flight budget. Free (`:free`) models: 50 requests/day under 10 credits bought, 1000/day at 10 or more. | confirmed (limits page, query read) |

Sources: https://openrouter.ai/docs/quickstart ;
https://openrouter.ai/docs/cookbook/coding-agents/claude-code-integration ;
https://openrouter.ai/docs/api_reference/limits ;
https://openrouter.ai/docs/guides/best-practices/reasoning-tokens ;
https://openrouter.ai/docs/cookbook/administration/usage-accounting

## OpenCode (the "Zen" model service)

The owner's brief calls the service OpenCode Zen. The current docs (`opencode.ai/docs/zen`, which
redirects) call it **OpenCode Console**, with a pay-as-you-go provider named OpenCode. The provider
id `opencode` and the label "OpenCode Zen" follow the brief. The docs name is noted for the UI agents.

| Item | Finding | Status |
| --- | --- | --- |
| Base URL | `https://opencode.ai/inference/v1` | confirmed (endpoint table) |
| Auth | `Authorization: Bearer $OPENCODE_API_KEY` (key from the OpenCode Console) | confirmed (Jev example). Key name `OPENCODE_API_KEY` from that example. |
| Model list | `GET https://opencode.ai/inference/v1/models` | endpoint confirmed. Response shape unconfirmed. |
| API shapes | `/responses` (OpenAI Responses) for GPT, Grok and Muse; `/messages` (Anthropic Messages) for Claude and some Qwen; `/chat/completions` (OpenAI-compatible) for DeepSeek, GLM, Kimi, MiniMax, Mistral, and the free models; `/models/<id>` for Gemini (Google shape, not relayed); `/systemone` for Jev (not relayed) | confirmed (endpoint table) |
| Ids in OpenCode | `opencode/<model-id>` in OpenCode config | confirmed. Plaza ids: `opencode/<model-id>`. |
| Curated models | The Console list (about 80 ids; see the table). Examples: `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `kimi-k3`, `deepseek-v4-pro`, `glm-5.3`, `minimax-m3` | confirmed |
| Pricing | Published per 1M tokens, input / output / cached read / cached write, for every model. Examples: `deepseek-v4-flash` $0.14 / $0.28; `claude-sonnet-5-5` $2.00 / $10.00; `claude-haiku-5-5` (<=100K) $0.10 / $0.50. Free models are $0. | confirmed (pricing table). Use it as the price source. Long-context tiers are listed separately. |
| Privacy | Hosted in the US and EU. Zero retention except the free models named as using data for improvement (Big Pickle, Cat Jam Free, Exo Free, MiMo free, Ling free, Muse Spark 1.3 Contributor Free, which trades discounted prices for training). Nemotron free is logged by NVIDIA. | confirmed |
| Other offerings | **OpenCode Go**: a subscription (Go, Go Plus) that gives an API key. Whether Go uses the same endpoints is unconfirmed. Out of scope for this task. | Go API-key use confirmed; its endpoints unconfirmed |

Sources: https://opencode.ai/v2/docs/console/models/ (the `/docs/zen/` URL redirects here);
https://opencode.ai/docs/go/ and https://opencode.ai/v2/docs/console/go (search snippets only).

## Decisions for the implementation

- Presets: `openrouter` (label "OpenRouter", brand `openrouter`, `OPENROUTER_API_KEY`) and
  `opencode` (label "OpenCode Zen", brand `opencode`, `OPENCODE_API_KEY`).
- OpenRouter lists its models live (`GET /models`), cached for an hour (`options.models_cache_secs`).
- OpenCode lists live from `GET /models`; the preset's static `models` is a curated fallback.
- Free models that train on data go in `options.trains_on_data`, like Meta's contributor models.
- Relay routes per provider, because a route's provider id is fixed by its path:
  `/internal/v1/agents/{cid}/gateway/{openrouter|opencode}/v1/{chat/completions|messages|responses|models}`.
- A Zen model is sent only to the endpoint Zen serves it on. If Claude Code asks for a non-Claude model
  on Zen's `/messages`, or Codex asks for a non-GPT model on `/responses`, the gateway returns a
  400 that names the endpoint, not a confusing upstream error.
- Id parsing splits on the first `/` only (`provider/vendor/model`).
