# Model preferences and first setup

Agents can inspect configuration without calling a model:

```sh
skywrite defaults
skywrite defaults image
skywrite models list --task image
skywrite defaults image set black-forest-labs/flux-3-image-klein
skywrite defaults image unset
```

`default` is an alias for `defaults`. The short task names `image`, `video`,
`speech`, and `music` refer to their corresponding `*-generation` tasks. Full
task names work throughout model discovery and default inspection.

The JSON response identifies the effective model, its source, the saved
preference, and the environment variable that can override it. For example,
setting an image preference does not override an existing
`SKYWRITE_IMAGE_GENERATION_MODEL`. A setter stores a model ID; it does not pay
for inference or validate the ID against a remote catalog. Use `models list`
to discover currently advertised models and capabilities.
An invalid environment override is reported as `invalid_override` with a null
effective model and a diagnostic. An override that names an unknown provider, or a
custom provider for a generation task, is invalid in the same way. A saved preference
whose provider was removed by hand or cannot serve the task is reported as
`invalid_saved`. Configuration inspection and saving still
work, so an agent can help correct it; actual media commands reject that invalid
override before sending a request.

A model reference is a bare OpenRouter ID (`vendor/model`, `vendor/model:free`)
or `PROVIDER:MODEL` for a configured provider (`local:qwen3.5-0.8b`); see
[local and other providers](#local-and-other-providers). The response's `provider`
field names the provider of the effective model.

Selection precedence is:

1. An explicit `--model` on the media command.
2. `SKYWRITE_<TASK>_MODEL`, with dashes in the task changed to underscores.
3. The saved task preference.
4. The `elevenlabs/scribe-v2` transcription fallback (verbatim: it keeps fillers and repeated words; see [model evaluation](model-evaluation.md)).

An unset task is reported with a null model; it does not silently select a paid
model. Image upscaling currently uses local FFmpeg Lanczos resizing. Its query
reports that local mode, and any saved image-upscaling preference is reserved
for a future model-backed implementation rather than used by the current tool.

## Offline starter configuration

```sh
skywrite init --dry-run
skywrite init
skywrite defaults video
skywrite defaults video set minimax/hailuo-3
```

`init` adds missing model preferences from a dated starter set. It preserves
existing preferences and unrelated config fields, and a second run is
idempotent. Concurrent init and setter processes use the same stable lock and
atomic config publication. A dry run does not create a config or lock file.
Initialization works offline, stores no API key, does not access cloud storage,
and makes no paid request. Malformed or oversized existing config is rejected
without replacement.

The starter choices are intentionally stable preferences rather than an
automatically changing ranking: Gemini Flash Lite for analysis/editing, GPT-6
Luna for per-frame descriptions, Gemini 3.1 Flash Lite for video descriptions,
ElevenLabs Scribe v2 for verbatim word transcription (chosen in a measured
[model evaluation](model-evaluation.md)), FLUX 3 Image Klein for images, H3 Max for video,
FLUX Video Upscale, Kokoro for speech, and Lyria 3 Clip for music. The command
prints the exact IDs, curation date, and reasons. Model availability and prices
can change; `init` does not guarantee current provider availability. Use
`models list --refresh --task TASK` before a new paid workflow. Speech also needs
a voice supported by the selected model. Music's zero generic token prices do
not mean free clips or songs.

Preferences live in `~/.skywrite/config.json`; `SKYWRITE_HOME` changes the
personal directory. For paid requests, set `OPENROUTER_API_KEY` in the
environment inherited by the agent process. An export in a different terminal
does not update an already running agent. Storage settings and credentials are
separate; `init` does not create buckets or enable public access.

## Data policy for media understanding

Requests that send your media or words to a model (transcription, analysis,
frame and video descriptions, edit suggestions) are routed only to providers
that neither train on nor retain the data: OpenRouter's `data_collection: deny`
plus zero-data-retention (`zdr`). Set `SKYWRITE_DATA_POLICY=no-training` to allow
providers that retain but do not train, or `provider-default` to use OpenRouter's
account defaults. A model with no compliant provider fails with that explanation
instead of silently routing elsewhere. The routing setting is not part of the
response-cache key, so cached results remain valid across policy changes.
Generation requests (images, video, speech, music) are not affected.
The policy is an OpenRouter routing field: requests to any other provider never
carry it, and that server's own retention applies (`describe frames` and `describe
video` reports show `data_policy: null` and a `routing_note`).

## Local and other providers

Any task whose wire format is standard OpenAI can use another OpenAI-compatible
server instead of OpenRouter: a local llama.cpp `llama-server`, Ollama or LM Studio,
or another hosted API. Register it once by name:

```sh
skywrite providers add local --base-url http://127.0.0.1:8080/v1
skywrite providers add hosted --base-url https://api.example.com/v1 --key-env EXAMPLE_API_KEY
skywrite providers list
skywrite providers show local
skywrite providers remove hosted
```

Providers are stored in `config.json` next to the model preferences (`SKYWRITE_HOME`
moves both). Skywrite stores only the **name** of the environment variable that holds
a key (`--key-env`), never the key; omit it for keyless local servers, and then no
`Authorization` header is sent. A key variable that is named but unset is an error
before any request, including `models list --provider`; `doctor` then reports the
provider as not checked. The OpenRouter key is never sent to another provider:
`OPENROUTER_API_KEY` cannot be a custom provider's `--key-env` (register OpenRouter
itself as a custom provider with a separate variable). `--header
NAME=VALUE` adds non-secret headers (only their names are shown by `list`/`show`);
credential-like headers (`Authorization`, `Cookie`, names containing key, token,
secret or password) are refused. `--timeout SECONDS` sets the request deadline
(default 600 s, up to 3600; small models on a CPU are slow); the connect timeout is
15 s. `openrouter` is the built-in provider (`https://openrouter.ai/api/v1`,
`OPENROUTER_API_KEY`, 180 s); it cannot be removed or redefined, and nothing changes
when no provider is added. A provider that a saved default uses cannot be removed
until that default changes.

Plain `http://` is expected only for loopback and private-network addresses
(`127.0.0.1`, `localhost`, `10.x`, `172.16-31.x`, `192.168.x`, `100.64/10`, IPv6
`::1`, `fc00::/7`, `fe80::/10`); any other plain-HTTP URL is accepted with a warning,
because media, prompts and the key would travel unencrypted.

### Model references

Use `PROVIDER:MODEL` wherever a model is accepted: `--model`,
`SKYWRITE_<TASK>_MODEL`, and `skywrite defaults TASK set`.

- The provider is the text before the **first** `:`, but only when no `/` comes
  before that colon. Everything after it is the provider's model ID, sent verbatim,
  and may itself contain `:` and `/` (`ollama:qwen3:0.8b`,
  `lms:lmstudio-community/Qwen3-VL-2B-GGUF`).
- A reference without a colon, or with a `/` before its first colon, is an OpenRouter
  ID, so `openai/gpt-6-luna` and `meta-llama/llama-3.3-70b:free` work unchanged.
  `openrouter:vendor/model` is the same model; it is saved as `vendor/model`.
- Provider names are 1-32 lowercase letters, digits, `_` or `-`. `openrouter` and
  OpenRouter's variant words (`free`, `thinking`, `nitro`, `floor`, `online`,
  `extended`, `exacto`) are reserved.
- An unknown provider is an error, never a silent fallback to OpenRouter:
  `qwen3:0.8b` fails with a hint to write `local:qwen3:0.8b`.

### What a custom provider can do

Custom providers serve `analysis`, `frame-description` (including cast hints),
`video-description` (if the server accepts `video_url` input), `editing` (edit
suggestions) through `/chat/completions`, and `transcription` through
`/audio/transcriptions` with the standard multipart form (`file`, `model`,
`response_format=verbose_json`, `timestamp_granularities[]=word`, `language`), as
whisper.cpp's server, speaches and faster-whisper-server accept. Transcription still
requires real word timestamps: text without them is rejected, never estimated.
Generation (images, video, upscaling, speech, music) uses OpenRouter-specific APIs and
durable jobs, so it refuses any provider other than `openrouter`, both on the
command line and in `defaults set`.

Requests to a custom provider contain only standard fields. OpenRouter-only fields
(`provider` routing from `SKYWRITE_DATA_POLICY`, `usage` accounting, `reasoning`) and
the `X-Title` header are never sent. A leading `<think>...</think>` block from a local
reasoning model is dropped before the JSON answer is read. In `describe frames`
reports, servers that do not report a billed cost give `cost_usd: null` (unknown, not
free) and count `uncosted_requests`; a `describe video` request without a reported
cost gives `cost_usd: null`. Analysis, transcripts and edit suggestions record the
provider's own `usage` as returned.

A server that cannot be reached fails once, with the provider name and URL, and
stops the run: the failed request was not sent and Skywrite never retries. In
`describe frames`, the error also reports the cost of requests that completed earlier
in the run. A request that runs
past the provider's deadline fails with a hint to raise `--timeout`. A model that
rejects image (or video) input fails with that explanation; for llama.cpp this
usually means `llama-server` was started without the model's `--mmproj` projector.

`skywrite models list --provider NAME` lists the provider's own `/models` (with its
key, if configured; cached for 24 hours, `--refresh`/`--offline` as usual). Each entry
has a ready `reference`. Capabilities are not guessed from names: a model is
`capabilities_verified` only when the catalog declares its input modalities, and other
models are listed as unverified but remain usable. A server without `/models` gives an
empty list and a warning. `skywrite doctor` lists every provider (never a key value)
and checks that each custom one answers `/models`; it never generates and never
contacts OpenRouter.

Response caches key on the provider's name, base URL and headers plus the model ID,
so two providers serving the same model name, even at the same URL, never share an
entry.

### Worked examples

**llama.cpp `llama-server`** (Metal on Apple silicon, CUDA or CPU elsewhere). `-hf`
downloads a vision model together with its `mmproj` projector; bind it to loopback:

```sh
llama-server -hf ggml-org/Qwen3-VL-2B-Instruct-GGUF --host 127.0.0.1 --port 8080 -c 8192
skywrite providers add local --base-url http://127.0.0.1:8080/v1
skywrite models list --provider local
skywrite describe frames interview.mp4 --every 30 --model local:Qwen3-VL-2B-Instruct-GGUF
```

`llama-server` serves one model and lists it under its alias (`--alias NAME` sets one).

**Ollama**:

```sh
ollama pull qwen3-vl:2b
skywrite providers add ollama --base-url http://127.0.0.1:11434/v1 --timeout 900
skywrite describe frames interview.mp4 --every 30 --model ollama:qwen3-vl:2b
```

**LM Studio** (load a vision model, then start the server on its default port):

```sh
lms server start
skywrite providers add lms --base-url http://127.0.0.1:1234/v1
skywrite models list --provider lms
skywrite describe frames interview.mp4 --every 30 --model lms:qwen/qwen3-vl-4b
```

**A local Qwen 3.5 0.8B for frame descriptions, to cut cost.** Survey passes describe
hundreds of frames, so a small local vision model can replace a paid one for that
step:

```sh
skywrite defaults frame-description set local:qwen3.5-0.8b
```

Measured on 2026-11-23 on an Apple M4 Mac mini: Qwen3.5-0.8B Q8_0 with its BF16
`mmproj` (lmstudio-community/Qwen3.5-0.8B-GGUF) under `llama-server -ngl 99 -c 8192
--reasoning-budget 0` described a frame in about 2.1 s, with the server's resident
memory peaking at 1.8 GiB, at no API cost. On a small synthetic set (text cards,
test patterns, no people), it read every title correctly and invented no people or
text, matching GPT-6 Luna. It called one beige card "a dark room". That set cannot
measure people, expressions or mid-speech, which is where small models are weakest.
Treat its descriptions as a cheap first pass, and confirm important frames by viewing
them or re-describing them with a stronger model. See
[model evaluation](model-evaluation.md#local-model-check-2026-11-23-synthetic-known-content-frames-not-the-set-above).
