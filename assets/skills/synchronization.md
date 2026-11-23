# Align independent recordings

Typical job: a camera with low-quality scratch sound and a separate high-quality recorder. Measure once per continuous recording with `skywrite sync`, let `skywrite sync apply` write the recorder clip(s) into the edit, verify, and listen. Never do the placement arithmetic yourself, and never overwrite originals.

## 1. Choose reference and asset

- `--reference` is the file whose clock the timeline follows: the camera you put on the timeline. Its picture keeps speed 1.
- `--asset` is the file you place against it: the separate recorder (it gets the drift `speed`).
- One sync per pair of files. A camera split into several files needs one sync (and one `sync apply`) per camera file. A recorder that paused or restarted inside one file is handled: the result has several `segments`.

## 2. Measure

```sh
skywrite inspect camera.mp4 recorder.wav            # both need an audio stream, at least 6 s
skywrite sync --reference camera.mp4 --asset recorder.wav --method audio -o recorder.sync.json
```

`--method audio` is local and free (waveform). `--method auto` uses transcripts only when both inputs are `.json`; for speech without shared sound, transcribe each file (`skywrite transcribe FILE -o FILE.transcript.json`, a paid API call on OpenRouter, or a request to the user's own provider for a `PROVIDER:MODEL` model) and sync the two transcripts. Only the first `--max-audio-seconds` (default 3600) of each file is analyzed.

## 3. Place it

Put the camera clip on the timeline first (any `start`, `source_in`, `duration`), then:

```sh
skywrite sync apply --sync recorder.sync.json --edit edit.json -o edit.synced.json --verify
```

For every clip whose source is the sync reference (or the clips named with `--clip TRACK:CLIP`, for example `--clip V1:0`), it adds the recorder on a new audio track `"<recorder file name> (synced)"` (role dialogue; `--track NAME`, `--role`): `start` on the first output frame where the recorder has sound, the sub-frame remainder in `source_in`, `speed` = the measured `playback_rate` (times the camera clip's speed if that is not 1), `duration` trimmed to the real overlap (one clip per segment), and the camera clip's own sound muted (`"audio": {"volume": 0}`; `--keep-reference-audio` leaves it). The output is validated before it is written; it must sit next to the input edit, which is never modified.

Read the command's result:

| Field | Meaning |
|---|---|
| `placements` | each clip written: `start`, `source_in`, `speed`, `duration`, `timeline_range`, `asset_range`, `extrapolated_seconds` (asset placed outside the matched anchors) |
| `uncovered` | timeline inside the camera clip with no recorder sound (recorder started late or stopped early). It is silent because the camera is muted: trim the camera clip, or tell the user |
| `warnings` | read every one (discontinuity boundaries, extrapolation, `--allow-limited`) |
| `verify` | with `--verify`: each placed clip rendered and re-synced against the matching stretch of the camera; `passed` needs one strong, continuous re-sync covering the clip with both ends within `limit_ms` (2 ms); each clip lists its `problems`. Clips under 15 s, or on a camera clip not within 3% of speed 1, are skipped |

`sync apply` refuses a result that is not `strong` with `drift_estimated` (or uses the 50 ms `audio_envelope` method) and prints why and what to do next. Do not reach for `--allow-limited` to get past a refusal: it is for a user who has heard the result and accepts it. Never place a failed or refused match by hand.

## 4. Read the measurement (when you need to explain it)

The mapping is asset to reference: `reference_time = offset_seconds + scale * asset_time`. Negative `offset_seconds`: the recorder started first. `playback_rate` = 1/`scale` is the asset clip's `speed`; `drift_ppm` = (`scale` - 1) x 1e6 (100 ppm is 0.36 s per hour).

| Field | Strong result |
|---|---|
| `method` | `audio_waveform` (sub-sample waveform anchors every 2 s up to an hour of take, at most 1800) or `transcript`. `audio_envelope` (50 ms) is approximate only |
| `quality` | `strong`: `drift_estimated`, at least 16 inliers, `p95_error_seconds` < `strong_p95_limit_seconds` (5 ms waveform, 50 ms transcript) |
| `drift_estimated` | `true` needs anchors spanning `--min-span-seconds` (10): about 12 s of shared sound |
| `discontinuous`, `segments` | more than one mapping in the take (a cut, dropout or recorder restart). Top-level fields then describe only the best-supported segment and `quality` is `limited`; each segment has its own `asset_range`, offset, scale and quality, and `sync apply` places each one. Segments stop 0.3 s short of each boundary (its location uncertainty), so expect a short `uncovered` gap there |
| `warnings` | the microphone-propagation warning is always present and informational; boundary warnings give where segments meet |
| `candidate_matches`, `inliers`, `anchors` | waveform probe windows (one per 2 s) that found any match, how many of them the main mapping fits (`inlier_ratio` = inliers / candidates), and up to 64 of the fitted anchors, evenly thinned |

Measured on synthetic dual-system takes (scratch-mic damage, 20–208 ppm drift): 2–60 minute takes end within about 1.5 ms of the truth (see docs/validation.md). The camera mic is farther from the talker than the recorder; aligning to its sound includes that acoustic delay (about 3 ms per metre), which is far below lip-sync perception.

## 5. Verify

1. `--verify` on `sync apply` (above), or render and measure yourself: `skywrite sync --reference camera.mp4 --asset render.mp4 --method audio` should report the camera clip's timeline mapping (offset = camera `source_in` - clip `start`, scale 1) within 1-2 ms.
2. Listen near the start, middle and end of each placed clip for echo (double audio) or drift, and around segment boundaries.

## Low confidence or failure

Never apply a failed or refused result, or one with a `limited` segment (`sync apply` refuses these; a `discontinuous` result is `limited` only at the top level, and is placed when each of its segments is `strong`). Tell the user exactly what was measured (method, quality, intervals, residuals) and record `skywrite nit "..."` with the command and output.

- `no reliable shared audio` / `shared audio matches are too weak`: the message says how many analysis windows found any candidate match. The files may not contain the same sound (unrelated audio fails here, correctly), or overlap by less than about 10 s. Do not try to force a match by filtering or loosening options: tell the user, and ask for a recording that shares more of the take.
- `ambiguous synchronization: multiple passages support different time mappings`: the sound repeats (music loops, repeated takes). Tell the user. A distinctive excerpt (cut from one file with ffmpeg) can be synced on its own, but its result names the excerpt, so placing it is manual work (docs/edit-format.md, "Placing a sync result"; convert the excerpt result with `offset_full = Rs0 + offset_seconds - scale * As0`, where the reference excerpt starts at Rs0 and the asset excerpt at As0): do it only with the user's agreement and verify by rendering.
- `drift_estimated: false` or `quality: limited` with a short overlap: there is not enough shared sound to measure the speed. Get a longer overlap; do not guess a speed.
- `method: audio_envelope`: only 50 ms resolution; get a longer or cleaner overlap, or use transcripts.
- A segment whose quality is `limited`, or a warning that strong matches "fit no mapping": a short cut or dropout the analysis could not separate. Sync the material on each side separately (trim with ffmpeg) or ask the user.
- Speech recorded by different microphones with little shared sound: transcribe both and use `--method transcript` (with the user's API budget); word timestamps are approximate, so check by listening.

## Dialogue cuts from real words

Use an existing source-bound transcript whenever available. `skywrite analyze source.mp4 --local --transcript transcript.json --start 10 --duration 20 -o analysis.json` performs local analysis without an ASR call. `edit suggest` supplies only original words overlapping the analyzed range and rejects changed source fingerprints. For a direct inspection of a bounded source window:

```sh
jq --argjson start 10 --argjson end 30 '[.words[] | select(.start < $end and (.end > $start or .start == $start))]' transcript.json
```

Keep words crossing either boundary; do not clamp their timestamps to the queried range. Choose complete thoughts inside the observed range or request a wider range. Use original starts/ends in playback seconds, including fractional precision. Container timecode and uniformly sampled frame seeks are separate concepts and cannot replace word alignment. A transcript is provider alignment evidence; listen around a cut before accepting it. An untranscribed interval is not proof of silence.

For audible speech, preserve the entire word span. `source_in` supports subframe precision; clip `start` and output `duration` follow the sequence frame grid. At speed `s`, source out is `source_in + duration*s`; round output duration outward when necessary to retain the last word. Do not round a word's start upward or end downward and call the remaining syllable a complete word.

Muted B-roll can change picture during a spoken word. Intentional J/L cuts can carry a complete dialogue span on a separate audio overlay while the picture changes. Do not enforce a blanket ban on visual midword cuts. No tool should silently transcribe, regenerate narration, or submit a paid retake to solve a cut.

To attach existing words directly to saved analyses, use `skywrite edit suggest --analysis analysis.json --transcript transcript.json --brief "Intent" --model MODEL`. Repeat `--transcript` for multiple sources. Supplied transcripts must include `usage.source_sha256`, 64 hex digits (case-insensitive) matching an analyzed source’s media content SHA-256. Byte-identical copies are accepted. The same check applies to `analyze --transcript`, `describe frames --transcript`, `review lint --transcript` and `from_transcripts` edit caption tracks. Missing, invalid or mismatched fingerprints fail; re-run `skywrite transcribe` on the media, or point the transcript/edit at the media it was made from. Captions, `describe` and `review lint` cache media hashes in `media-sha256-v1/`, validated by canonical path and stat identity (deleting it is safe); `analyze` and `edit suggest` hash media in full. Standalone `captions convert` does not bind transcripts to media. Identical duplicate word evidence is accepted; conflicting alignments fail rather than silently overriding an embedded transcript. The command never transcribes implicitly. Advisory warnings identify audible cuts inside supplied words; muted B-roll and continuous matching audio across cuts are exempt.
