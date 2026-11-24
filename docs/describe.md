# Frame and video descriptions

`skywrite describe` asks a vision model about exact frames or a short video range.
Requests carry your media, so OpenRouter requests follow the [data policy](configuration.md#data-policy-for-media-understanding)
(zero retention by default). `--model local:MODEL` sends them to a
[local or other provider](configuration.md#local-and-other-providers) instead; the
report then names the `provider`, has `data_policy: null` and a `routing_note`, and
reports `cost_usd: null` when the server does not report a cost. Discover models with `skywrite models list --task frame-description`
or `--task video-description`; defaults come from `skywrite init` and the
[model evaluation](model-evaluation.md).

For frame descriptions, `--transcript words.json` requires `usage.source_sha256`:
64 hex digits (case-insensitive) matching the selected media content SHA-256.
Byte-identical copies are accepted. Verification happens before frame extraction
or model requests. Missing, invalid or mismatched fingerprints
fail; re-run `skywrite transcribe` on the media, or point at the media the
transcript was made from. Media hashes use the shared [cache](cache-models.md).

## Frames: survey, then focus in

```sh
# Coarse survey: one frame per second (every 30th frame at 30 fps)
skywrite describe frames interview.mp4 --every 30 -o survey.json
# Dense pass around a candidate cut: every 2nd frame for one second
skywrite describe frames interview.mp4 --around 125.4 --window 1 -o cut-125.json
# Check counts and cache hits first; makes no requests (would_request)
skywrite describe frames interview.mp4 --interval 2 --dry-run
# Ask for exactly what you need from each frame, in a fixed JSON shape
skywrite describe frames interview.mp4 --interval 1 \
  --focus "bounding box of every visible human face" --focus-schema faces.json -o faces-out.json
# What is already cached for this source (read-only)
skywrite describe cache interview.mp4
```

Each frame is described independently from one still (fit to 768 px). Frames are
chosen on the source's absolute frame grid (`--every N` keeps frames whose number is
a multiple of N; `--interval S` keeps the first decoded frame of each S-second bucket
aligned to the source's first video frame). Empty buckets across timestamp gaps
yield no frame; a range starting after a bucket's first frame skips that bucket. Thus a
dense pass reuses every frame an earlier survey already described. Results are
cached per source content hash, exact presentation time, model, prompt version and
cast. Re-running, widening or overlapping a pass never pays twice, and a run with
only cached frames needs no API key. Failed frames are not cached; rerun the same
command to retry them alone. Progress on stderr, every 25 frames and after the last,
counts each outcome: `Finished 2/2 frames (1 described, 0 cached, 1 failed)`.

Each result lists the source time, estimated frame number, the persistent JPEG that
was described (under the cache's `assets/<sha>/exact-frames/`, named by time and
width; frames fit in 768 pixels by their displayed shape, so rotated phone footage
stays upright and 9:16 portrait frames are 432x768), and a JSON
description: shot size, camera angle, people (position, appearance, gaze,
expression, mouth, apparent mid-speech, gesture), setting, objects, legible text,
image issues and a one-sentence summary. `--transcript words.json` attaches the
words spoken around each frame without sending them to the model.

`--dry-run` extracts the frames and reports `frames`, `cached`, `to_describe`,
`focus: {cached, to_answer}` (with `--focus`) and `would_request`: the requests this
exact command would make (a corrective focus retry can add one per frame).

`--max-frames` (default 600) refuses runs where more frames need a request (a description and/or a focus answer; a corrective focus retry can add one more request per frame);
raise it deliberately for long sources. `--concurrency` (default 4, up to 16) sets
parallel requests; a single local server usually answers one at a time, so
`--concurrency 1` avoids queueing past its deadline. The report includes the billed
cost of new descriptions. If the provider cannot be reached, the run stops at the
first attempt with one error instead of failing every frame.

A useful cut workflow:

1. Transcribe for word timestamps.
2. Survey with `--every 30` (or `--interval`).
3. Where a cut might go, run `--around TIME --window 1`, and view the frames or a
   `skywrite review sheet` of the same range.
4. Choose the cut from the words and the frames, then audition it with
   `skywrite review cuts`.

## Focus: ask for what you need

The general description is the same for every frame. When a task needs something
specific (face positions, whether a logo is visible, the text on a sign, which
hand holds the cup), add `--focus TEXT` (repeatable, joined in order) or
`--focus-file FILE`, on `describe frames` or `describe video`. Each result then
carries a `focus` object beside the unchanged `description`:

```json
"focus": {"instructions_sha256": "…", "schema_sha256": "…", "answer": {"faces": [{"box": [0.41, 0.12, 0.58, 0.47], "position": "center"}]},
          "cached": false, "response_format": "json_object", "attempts": 1, "clamped": 0}
```

An answer that needed the corrective retry has `attempts: 2` and `first_attempt_errors` (what
the retry fixed).

The report's top-level `focus` gives the focus identity (`sha256`, a `preview`),
`prompt_version` (`focus-v1`) and `structured_outputs`, and `counts.focus` has
`cached`, `answered` and `failed`. Focus text is limited to 16 items and 4000 bytes
together; control characters other than newlines and tabs are refused. Whitespace
at line ends and around items does not change the identity.

### Exact shapes: `--focus-schema`

`--focus-schema FILE` is a JSON Schema (at most 16 KiB, depth 8, 256 nodes) that the
answer must match. Supported keywords: `type` (object, array, string, number,
integer, boolean, null, or a list of them), `properties`, `required`,
`additionalProperties` (`true`/`false` only), `items`, `enum`, `const`, `minimum`,
`maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `minItems`, `maxItems`,
`minLength`, `maxLength`, `description`, `title`, top-level `$schema`, and
`"x-skywrite": "box"` or `"point"` for coordinates. Anything else (`oneOf`, `$ref`,
`pattern`, ...) is refused with the keyword and its path named. Beside `x-skywrite`, only
`type`, `description` and `title` are allowed (the annotation fixes the length and range);
`enum` values are scalars. Numbers (in the schema and in answers) compare as IEEE-754 doubles,
the interoperable precision of RFC 8259 (whole numbers compare exactly, even when one side is
written as a float): digits a double cannot hold are not distinguished. Schema numbers must
survive parsing: integers within 64 bits, and floats that are 0 or between 2^-1022 and 2^53 in
magnitude; anything else is refused. Strict structured output also needs `items` on every array. A face-box schema:

```json
{"type": "object", "additionalProperties": false, "required": ["faces"], "properties": {
  "faces": {"type": "array", "items": {"type": "object", "additionalProperties": false,
    "required": ["box", "position"],
    "properties": {"box": {"x-skywrite": "box"}, "position": {"type": "string"}}}}}}
```

An answer that does not match gets one corrective retry (the model sees its answer and
the errors). If the retry fails too, that frame's focus is a typed failure,
`{"error": {"code": "focus_schema_mismatch" | "focus_invalid_json" | "focus_request_failed", "message", "errors": [...], "attempts", "last_answer"}}`;
it is not cached, and rerunning the same command retries only those answers. Answers
are never silently dropped or reshaped. A valid answer that could not be written to the
cache is still returned with `focus.cache_error`; a valid description that could not be
written has `cache_error` on the frame (or on the `describe video` report). When every focus answer of a run fails, the
command fails; its error says which descriptions were cached (a write failure is named). Without a schema the answer is any
JSON the model returns under `focus`.

Where the provider's model catalog lists `structured_outputs` for the model (OpenRouter's
`supported_parameters`, or a custom server's `/models` that reports them), focus
requests use `response_format` `json_schema`: strict for focus-only requests when every
object in the schema is closed and fully required, non-strict when combined with the
general description. Otherwise they use `json_object`, and the same local validation
applies either way (`focus.structured_outputs`: `true`, `false`, or `null` when unknown
or not looked up).

### Coordinates

Positions are normalized numbers from 0 to 1 relative to the full frame as decoded and
shown (upright, after display rotation), origin top-left, x to the right, y down. A box
is `[x1, y1, x2, y2]` = left, top, right, bottom with `x1 < x2` and `y1 < y2`; a point
is `[x, y]`. Multiply by the frame's pixel width and height for pixels. The prompt
states this convention. For `x-skywrite` fields, values within 0.02 outside 0..1 are
clamped (counted in `clamped`) and anything else, pixel values or inverted boxes is a
mismatch. Coordinates are **model estimates**, not detector output: small local models
are often far off, and even strong models are loose. Look at the frame (`image` in each
result) before relying on one. A dedicated on-device face detector is planned.

### Focus caching

The general description cache is unchanged: same key, so existing caches still hit. A
focus answer is cached apart from it (`focus-answers/`), keyed by everything the
description depends on plus the normalized focus text, the schema and the focus prompt
version. Then:

- nothing cached: **one** request returns both, and both are stored;
- the description cached (by any earlier run, with or without a focus): a focus-only
  request that does not ask for or pay for the description again;
- both cached: no request and no key.

A different focus text or schema re-asks only the focus. There is no reason to leave
out `--focus` to get cache hits.

`describe video --focus` works the same way: times in the answer are clip-relative
seconds (`focus.clip_start` is the range start in source seconds), and coordinates use
the same convention on the re-encoded clip.

## What is cached: `describe cache`

```sh
skywrite describe cache interview.mp4
skywrite describe cache interview.mp4 --kind focus --focus "bounding box of every visible human face"
```

Lists what is cached for the source's content (a renamed or copied file lists the same
entries): groups by kind (`frame`, `video`, `focus`), provider, model, prompt version,
`cast_sha256` and, for focus entries, the focus identity with a short preview of the
text. Each group has `entries`, `frame_times` as compact ranges
(`{start, end, step, count}`; every cached time is `start + i*step` within 1 µs) or video `ranges`, `bytes` and `created` (first/last, unix
seconds); the report adds totals and the cached exact-frame images. `--model`, `--focus`
and `--kind` filter. It is read-only and makes no requests. Entries written before
listing metadata existed count as `unattributed_entries` until a describe run reads
them. For what one exact command would cost, run it with `--dry-run`.

## Who is visible: cast files

```json
{"version": 1, "people": [
  {"name": "Ralph", "description": "white hair, grey suit, red tie; usually at the podium"},
  {"name": "Host", "description": "long dark hair, green dress"}
]}
```

`--cast cast.json` adds advisory matching to each description: `cast_matches` (with
the deciding feature the model saw and a runner-up) and `cast_present`. Matching
uses only your text descriptions: no reference photos, no face recognition, and no
biometric data. Treat results as hints, not identification. In evaluation it was
reliable with distinct descriptions in close-ups, but it can pick a look-alike when
the deciding feature isn't visible (glasses from behind, colours in a tiny wide
shot), and the model's confidence does not reflect that. Describe stable features
visible from several angles (hair, garment colours), and confirm by viewing frames.

## Video ranges

```sh
skywrite describe video interview.mp4 --start 56 --duration 8 -o laugh.json
```

The range is re-encoded to H.264 at up to 480 rows without audio (`--height`
240–1080, a maximum) and sent inline. It is never upscaled: a source shown smaller than `--height` (by its
displayed size, so rotated phone footage counts upright) keeps its own height,
rounded down to an even number, and `encoding` in the result names the height
sent (for example `h264-crf28-upright-h480-noaudio`). The model reports shots, timed events (gestures, head turns,
expression changes, visible speech onsets) and people, with clip-relative times plus
`source_*` times. Ranges are limited to 600 seconds and 17 MiB encoded; shorter
ranges give finer timing. A custom provider must accept `video_url` input; one
that does not fails with that explanation. Results are cached by source content, range, encoding,
model, prompt version and cast; `--dry-run` reports `cached`, `focus_cached` and
`would_request` (0 or 1 initial request; a corrective focus retry can add one) without requesting. Descriptions are observations of a reduced clip:
confirm important moments with `review sheet`.
