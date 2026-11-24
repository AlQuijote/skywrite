# Measured validation and limits

This page records what has been measured, and what has not. It is dated
evidence, not a compatibility promise for every model, platform or S3 provider.
Later source changes need fresh validation.

## Automated checks

The normal Rust test suite (`cargo test --locked`) runs offline against media
generated locally with FFmpeg, together with formatting and all-target Clippy
with warnings denied. Continuous integration runs it on Linux and macOS and
checks that the crate still compiles with the minimum supported Rust version
(1.91). The minimum-version check establishes compilation, not a second full
runtime test matrix. None of this claims production readiness or actual Final
Cut/Resolve import.

## Live provider checks

During development, live behavior was checked with explicit, spending-capped
OpenRouter requests and a dedicated Cloudflare R2 test bucket. These are
measured examples from 2026-10-30, not guarantees:

| Executed check | Observed result |
| --- | --- |
| Image generation and reference edit | FLUX 3 Image Klein returned JPEG; PNG publication converted the bytes correctly |
| H3 Max video | 832×480, 24 fps, about 5.184 s |
| Exact MiniMax H3 video | 2560×1440, 24 fps, about 5.167 s, AAC 32-kHz stereo |
| Dedicated video upscale | 832×480 became 1664×960; about 24 ms duration difference, below one frame |
| Kokoro narration | `hexgrad/kokoro-82m`, `af_heart`: decoded 9.945 s, 24-kHz mono MP3 |
| Lyria 3 Clip music | The CLI retained the MP3 and published 24.921 s of 48-kHz stereo WAV |
| Narration word timing | ASR recovered the clock-drift explanation with 22 word timestamps |
| Completed audio/video replay | Saved results republished without keys or network, with no new paid submission |

The whole set of live checks, including rejected and protocol probes, cost about
US$1.27 in OpenRouter usage at the time. Lyria Pro was catalog-discovered but not
live-generated. Music duration, vocals and artistic suitability require listening
review; a requested format can differ from the provider's actual container, so
successful publication requires decoding and any necessary conversion. MIDI and
text-score rendering are not supported. Image upscaling is local Lanczos
resizing; the tested AI upscaler is for video.

The live Cloudflare R2 check used Apache Arrow’s `object_store` 0.14.2 backend.
It exercised signed hash-addressed MP4 upload and HEAD reuse, public HTTPS
byte/hash identity, an actual upscale using that R2 source, and deletion only
after a verified local result. Cleanup was deferred while another local receipt
was unfinished; a configuration mismatch preserved the result and source, and
restoring the settings allowed cleanup on resume. Paged purge preserved active
sources and foreign or unmarked objects while removing eligible inputs.

Two isolated machine configurations also exercised named output publication,
name/hash discovery, pagination, Unicode names, empty files, explicit conditional
replacement, a 4-MiB-plus-37-byte range download, checksum/no-clobber behavior,
and protection of the local storage configuration. No credentials, account
endpoints or test-bucket URLs are stored in the repository.

Not established: Amazon S3 itself or other compatible services; maximum 512-MiB
transfers and slow-network throughput; forced simultaneous stale-ETag writers;
every crash/power-loss window; cross-computer input-job leases; versioned-bucket
reclamation or CDN invalidation. Input cleanup protects local job receipts only;
use separate project prefixes when independent computers may still need a source.
Shared outputs are outside the temporary-input purge namespace.

## Remotion export dependencies

Checked 2026-11-24 on Linux (Node 26.13.1, npm 11.20.0, FFmpeg 8.0.1) with a
synthetic edit (3-s `testsrc2` H.264 clip with a 440-Hz tone, a 660-Hz WAV on an
audio track; 640×360, 30 fps, 2 s) exported by `skywrite edit export --to
remotion`. The exported `package.json` pins `remotion` and `@remotion/cli`
4.0.546 with no `overrides`. `npm install` succeeded; `npm ls zod --all`
resolved a single version, zod 4.6.7 (the GHSA-mrhx-c7xc-m3w5 fix), deduplicated
across six dependents; `npm audit` found 0 vulnerabilities. `npm run render`
wrote `edit.mp4`, which ffprobe reads as H.264 640×360 at 30 fps with 60 frames
plus AAC 48 kHz stereo, 2.005 s. Remotion printed no version-mismatch warning;
the previous pins (4.0.539 with a zod 4.6.7 override) printed "Version mismatch …
For zod, install exact version 4.5.4". On a Mac mini (macOS 27.0.1, FFmpeg 9.0.2),
install, `npm ls zod` and audit gave the same results, but headless rendering
timed out at "Waiting for root component to load" for both the old and new pins
alike, so that environment does not establish a macOS render.

## FFmpeg version compatibility

Checked 2026-11-24 on Linux (AMD Ryzen AI Max+ 395) with static FFmpeg builds
5.1.2, 6.1.1, 7.0.2, 7.1, 7.1.1, 8.0.1, 8.1.2 and 9.1 (`mwader/static-ffmpeg`,
digest-verified) and Ubuntu's 8.0.1, each first on `PATH`, running
`tests/export_pipeline.rs` `delayed_video_prefix_is_counted_once_in_both_renderers`
and the `tests/audio_finishing.rs` crossfaded-loop tests alone, three to five
times per version. Two FFmpeg behaviours that skywrite used to depend on differ
by version:

- FFmpeg 7.0 and 7.1 `setpts` clears the frame rate on its output, and `tpad`
  converts `start_duration` to frames with that rate, so a black lead-in placed
  after `setpts` added no frames: a clip whose video starts 1 s after its audio
  showed its first picture at 0 s (in both the renderer and exported scripts).
  5.1, 6.1 and 8.0 or newer keep the rate. The lead is now added after `fps`,
  in output seconds.
- Before FFmpeg 8.0, `acrossfade` ends its output as soon as its second input
  ends, even while it is still reading the first. With the threaded CLI of 7.0
  and 7.1, a crossfaded loop of a 1-s cue was cut short at random points and
  padded with silence (5.1 and 6.1 happened to pass). Loops now fade and delay
  each copy and sum them with `amix normalize=0`. On every version above, the
  looped output matches the exact sum of linearly faded copies to within 3 LSB
  (16-bit).

Before the change, both tests failed every run on 7.0.2, 7.1 and 7.1.1; after
it, both pass on all nine builds.

On this CPU the static 5.1.2 to 8.1.2 builds (not 9.1 or Ubuntu's 8.0.1) have a
separate fault in their bundled librubberband: after about 0.1 s or more of
leading digital silence on float input, `rubberband` output stays about 15 dB
low (RMS 0.0154 instead of 0.088 for FFmpeg's default `sine` source). The same
builds are correct on an Intel Core i7-8550U. Skywrite's rubberband pad is
leading silence, so speed-0.5 clip audio renders quiet there; the delayed-video
test's prefix-level check was skipped for those builds in the runs above.

## Dual-system sound: camera scratch track and separate recorder

Measured 2026-11-15 with FFmpeg 8.0.1 on Linux. What is real and what is
simulated:

| Material | Real or simulated | Source and license |
| --- | --- | --- |
| Speech | Simulated: FFmpeg's `flite` voices reading deterministic, non-repeating pseudo-sentences, two alternating voices with varied pauses | generated locally (`scripts/agent-align-eval/media.py`) |
| Recorder | Simulated: a clean excerpt of that speech with -64 dBFS room tone, 24-bit WAV | generated |
| Camera scratch track | Simulated: the speech 3.5 ms later (about 1.2 m of air), 160 Hz-5.5 kHz band limit, a synthetic room impulse response (RT60 0.35 s), pink noise, 5:1 compression, a clock 20.8, 104.2 or 208.4 ppm off (an exact integer `asetrate` ratio) and 96 kb/s AAC in an MP4 with a test picture | generated |
| Independent devices in a real room | Real: four smartphones (Pixel 7, Pixel 6a, Xiaomi, Nexus) with unsynchronized clocks recording the same loudspeaker playback, session 8 of `LibriWASN_200_0L` | [LibriWASN](https://zenodo.org/records/7960972), Paderborn University, CC BY 4.0; fetched member by member with `scripts/agent-align-eval/fetch_libriwasn.py` into a scratch folder, never committed |

No openly licensed footage with a camera's own sound *and* a separate
recorder was found. LibriWASN is the closest real material: real microphones,
rooms and free-running clocks, but no picture and no known true offsets, so it
is checked for consistency rather than against ground truth. The AMI checks
below use one recording clock with simulated rate changes. Committed tests
stay synthetic: `tests/sync_apply.rs` builds a damaged, drifting camera from a
speech-like signal with FFmpeg at test time.

The ground-truth audio mapping includes the acoustic delay; the band-limit
filters add about 0.8-1.0 ms of group delay at the analysis band on top, which
shows up as a constant offset in the "scratch" rows (the "clean" rows, which
skip the damage, isolate the estimator).

**Long takes** (`scripts/agent-align-eval/bench_long_takes.py`; recorder rolls
7 s before the camera; error = sync mapping minus truth at the camera's first
and last frame, worst of the three drift rates):

| Take | Camera | Quality | Anchors | Drift error | Error at camera start | Error at camera end | 24-anchor baseline, end |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 min | clean | strong | 58-59 | +0.426 ppm | +0.06 ms | +0.11 ms | +0.28 ms |
| 2 min | scratch | strong | 56-57 | -1.390 ppm | +0.56 ms | +0.60 ms | +1.01 ms |
| 10 min | clean | strong | 291-294 | -0.067 ppm | +0.08 ms | -0.06 ms | -0.06 ms |
| 10 min | scratch | strong | 280-285 | +0.082 ppm | +0.53 ms | +0.57 ms | +0.91 ms |
| 30 min | clean | strong | 874-879 | -0.012 ppm | +0.06 ms | -0.06 ms | -0.07 ms |
| 30 min | scratch | strong | 832-853 | +0.033 ppm | +0.54 ms | +0.54 ms | +1.41 ms |
| 60 min | clean | strong | 1739-1756 | -0.001 ppm | -0.06 ms | -0.07 ms | -0.07 ms |
| 60 min | scratch | strong | 1687-1705 | +0.008 ppm | +0.49 ms | +0.52 ms | +0.93 ms |

The 60-minute sync takes about 15-17 s (one thread, on a loaded machine); a 24-anchor baseline estimator took 10-31 s. No take ended more than 10 ms off with either estimator, so the dense anchors serve robustness and discontinuity detection, not long-take accuracy.

**Discontinuities** (synthetic flite takes, 24 fps, 104 ppm): a 3 s recorder
pause, a 40 ms recorder dropout and a 5 s cut in the camera file are each
reported as `discontinuous` with two `strong` segments; boundaries land within
0.16 s of the truth and segment offsets within 1 ms. After `sync apply`, every
placed sample is within 0.81 ms of the truth (the 0.3 s boundary margin is
left uncovered).

**Real devices (LibriWASN, 10.5 min, 16 kHz):** every phone pair is `strong`
with p95 residuals of 1.8-3.4 ms (a real room, different microphone
positions) and 14-17 ppm drift between phones. Chaining Nexus to Xiaomi to
Pixel 7 agrees with syncing Nexus to Pixel 7 directly within 0.43 ms across the
take; syncing each 5-minute half separately gives 21.1 and 14.9 ppm (the
phones' clocks wander; full take 17.1 ppm) and the half mappings agree with the
full-take mapping within 0.21 ms at their midpoints. A 24-anchor baseline
estimator without pre-emphasis closed the triangle within 0.1 ms but graded one pair
`limited` (p95 5.7 ms), and its full-take drift (12.5 ppm) disagreed with its
own halves (25.2 and 15.3 ppm) by up to 1.4 ms.

Detection limits for a stretch inside a take with a different mapping
(anchors every 2 s up to an hour of take, every 8 s at the 4-hour limit): one
lasting 16 s or more becomes its own segment; a shorter one is reported as
`limited` when it differs by at least 10 ms and spans three anchors that agree
with each other, or when it carries 8 s or more of sound that matches no
mapping. A difference under 10 ms (under a quarter frame; indistinguishable
from pitch-period slips in voiced speech) is not separated. The 16 s and 8 s
lengths assume 2-s anchors and grow with the spacing (to about 64 s and 32 s
at the 4-hour limit). Sustained pure tones behind strong discrete reflections can lock
onto a reflection 10-20 ms late; such results come back `limited` (p95 about
7 ms), and `sync apply` refuses them. A shared overlap under about 12 s yields
no drift estimate (refused); under about 10 s the waveform stage may not
engage at all, and an envelope-only match (50 ms) can be wrong.

**Agent evaluation** (`scripts/agent-align-eval/run_eval.py`, six scenarios,
`claude -p --model claude-opus-5-5` and `codex exec -m gpt-6.1-sol`, each run
in a fresh temporary folder with only the binary and the plain task "replace the
camera's scratch audio with the recorder audio"): 12 of 12 passed, with
placements within 0.39-0.72 ms of the audio truth, and both agents refused the
7-second overlap and the unrelated recording. Every agent measured with
`skywrite sync` and placed with `skywrite sync apply --verify`; none computed a
placement by hand.

## Reproduce public-media checks

The normal Rust test suite works offline and uses generated media. Public media
and paid calls are opt-in. Run from the repository root:

```sh
cargo test --locked --offline
python3 scripts/validate-public-media.py prepare
python3 scripts/validate-public-media.py
```

`prepare` downloads short AMI Meeting Corpus excerpts through FFmpeg HTTP seeks;
it needs network access and FFmpeg. The default directory is
`/tmp/skywrite-public-validation`, outside the repository. It stores source URLs,
derived-file SHA-256 hashes, and attribution there. Existing files are reused.
The default `check` mode runs real microphone alignment offline, checks expected
offset and clock scale, and validates existing analysis/edit artifacts. It removes
`OPENROUTER_API_KEY` from child processes. If live transcript artifacts exist, it
also proves that the exact cached transcription works without a key.

The source is AMI Consortium's ES2002a meeting: Headset-0, Array1-01,
Mix-Headset, and Closeup1 video. The audio excerpts cover original seconds 30..150;
the video covers 30..60. AMI's [download page](https://groups.inf.ed.ac.uk/ami/download/)
licenses signals and transcriptions under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Preserve the generated
attribution file with distributed excerpts or rendered demonstrations. The two
derived microphone fixtures change the array sample rate to 16160 or 16002 Hz,
resample to 16000 Hz, then prepend 2 or 1 seconds of silence.

To explicitly permit bounded live requests:

```sh
python3 scripts/validate-public-media.py live --allow-paid --paid-budget 0.25
```

Set `OPENROUTER_API_KEY` in the environment or pass `--key-file /private/path`.
Never add credentials to the repository. Use a server-side key spending cap.
The script records key usage before/after and checks observed spending before
each request; this is a stop threshold, not an atomic provider billing cap.
There are four bounded operations: 30 seconds of word transcription, four-frame
video understanding, 30 seconds of audio understanding, and an edit suggestion.
The current model is configurable with `--model`; transcription uses the CLI's
default. Existing successful output files are reused, exact API responses are
cached, and failed paid requests are never retried automatically. Inspect saved
stderr and response-cache evidence before deliberately changing a request.

On 2026-10-30, a development build completed this live validation using
`openai/whisper-large-v3` and `google/gemini-3.5-flash-lite`:

| Executed check | Observed result |
| --- | --- |
| Headset versus table microphone | offset −0.979 ms; scale +54.64 ppm; 11 anchors over 112 s |
| Array sped up 1.01×, delayed 2 s | expected offset −2.02 s / scale 1.01; error 2.171 ms / 64.92 ppm |
| Array sped up 125 ppm, delayed 1 s | expected offset −1.000125 s / scale 1.000125; error 0.594 ms / 50.60 ppm |
| Real speech transcription | 77 provider-timestamped words, including the meeting introduction and participant roles |
| Visual understanding | Remote control, glasses, plaid shirt and listening posture independently checked against sampled frames |
| Audio-only understanding | Kick-off meeting topic agrees with the AMI manual annotation |
| Suggested edit and render | Validated 8 s, two video clips and one 5 s audio overlay; 384×288, 25 fps, 200 frames, H.264/AAC |

Alignment quality was reported `limited`: sparse overlapping speech and acoustic
propagation constrain the result. The checks allow 20 ms offset error and 250 ppm
scale error, rather than treating public microphones as exact electronic copies.
The rendered output has audible-source samples during its first 5 seconds and a
silent final 3 seconds; the sampled rendered frame depicts the remote demonstration.
It is an editorial cutaway, not measured lip synchronization.

Seven deliberately issued requests cost under US$0.01 in total, matching the
key's usage delta. This includes two rejected outputs and a low-signal probe:
the nearly silent individual headset produced an unreliable “Thank you” result;
the initial array transcription contained slightly unsorted provider word starts
and was rejected; an initial suggested edit copied `$schema` metadata and was
rejected without writing an edit. An explicit brief requiring an edit instance
without schema metadata then succeeded. Those observations are provider-quality
and integration limits, not evidence of guaranteed transcription/editorial
accuracy. No timestamp was fabricated. The source-mix introduction and role names
were checked against AMI manual word annotations; timestamps remain ASR estimates.

Ingestion sorts provider word starts that arrive out of order and reports it. Replaying the
identical failed array response from cache without an API key succeeded with all
42 original word text/start/end values preserved, chronological starts, and an
explicit reordering diagnostic. This replay checks the actual saved provider
response; it does not issue or charge another request. The whole transcript text is
trimmed at its outer whitespace; the preservation claim applies to provider word
text/start/end triples, not byte identity of the whole text.

This dated evidence does not certify later source changes; rerun the script
after changes that affect these paths.

The offline regression suite also exercises native Takumi HTML/JSON rendering
with an empty executable PATH, local font/image preservation and PNG/WebP/JPEG
output.

On Linux, subprocess regressions use controlled local child process groups to
check ordinary SIGINT/SIGTERM cancellation, deadlines, bounded capture, reader
joins and repeated concurrent failure/success cleanup. The supervisor keeps one
signal notification thread per process; calls do not accumulate readers or file
descriptors. Signal exit keeps the normal signal status, after active helper
groups have been killed and direct children reaped. SIGKILL, machine failure and
descendants that deliberately leave their assigned process group are outside
this cleanup guarantee. Process-lifecycle behavior on other operating systems
requires separate validation.
