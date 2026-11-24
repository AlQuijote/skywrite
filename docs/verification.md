# Local technical verification

`skywrite inspect FILE` reads headers through ffprobe. Its duration is labeled
`metadata_estimate`; variable bitrate MP3 headers can overstate actual playtime.
`skywrite inspect FILE --exact-audio` decodes the first audio stream to a null
sink and reports counted samples, sample rate, decoded sample duration, and the
last audio presentation end relative to the source origin. For audio-only media,
`duration` then uses that measured presentation end. Video duration remains the
metadata estimate; audio timing never truncates a video tail.

Exact timing is cached under the normal Skywrite cache root (override with
`--cache-dir` or `SKYWRITE_CACHE_DIR`), keyed by source SHA-256, timing-policy
version (`version` 2, directory `audio-timing-v2`; other versions are ignored and
re-measured) and the installed FFmpeg build. Content changes require fresh decoding.
`decoder` names that build compactly: `ffmpeg_version` (the version from the first
`ffmpeg -version` line) and `build_sha256` (SHA-256 of the complete `ffmpeg -version`
output, so a different configuration or library version re-measures even when the
version string is the same). It is read once per command; `verify` reports the same
object for its audio and video decodes.
`cache_hit` distinguishes reused evidence. Sample duration excludes timestamp gaps;
presentation end includes source-relative delay and gaps. The decoder handles
codec priming. The cache is regeneratable local evidence, not a signed attestation.
Existing MediaInfo JSON remains readable; inspection adds explanatory fields.
A video stream's `rotation` is its display-matrix rotation in degrees (phone
footage stored landscape reports -90 or 90); FFmpeg decodes it upright. It is
ffprobe's whole-degree value, and only quarter turns (90 and 270, either sign)
swap the displayed width and height. That matches the exact quarter-turn
matrices phones write; for other angles FFmpeg's own autorotation, which rounds
the matrix angle, may disagree.

`skywrite verify FILE --expected-duration 10 --duration-tolerance 0.05
--expected-width 1280 --expected-height 720 --expected-fps 30` performs a fresh
complete decode of the first audio and video streams. It reports audio sample
count, peak/RMS in dBFS, video frame count and decoded presentation end. Silence
has null peak/RMS, representing negative infinity. It checks requested expectations,
writes JSON to stdout, and exits nonzero for failed expectations. Use
`--output report.json` to also save it; replacement requires `--overwrite`.
An input cannot be its own report output, including with `--overwrite`.

`--metadata-only` skips decoding and labels duration evidence as estimated; the
decode checks above cannot be combined with it. Width, height and average FPS
checks use stream metadata, with width and height as displayed (rotation
applied); average FPS does not
establish constant frame rate. No audio levels, decoded counts or measured
duration are claimed in this mode.

Each decode has a 600-second wall deadline, six-hour presentation/sample work
limit, one MiB cap on each diagnostic/progress pipe and 16 GiB input limit. PCM is
never buffered or written to disk. Over-limit, corrupt, empty or incomplete
inputs fail without publishing exact evidence. Cache lock waiting is bounded.
Timing-sensitive audio-only range validation, transcription, timeline sources,
generation validation and audio synchronization use decoded timing automatically.
No command here accesses the network. Verification reports technical facts only;
it does not establish artistic quality, suitability for children or compatibility
with a particular editor.

`skywrite edit suggest --analysis analysis.json --transcript words.json --brief ...`
attaches existing provider word timestamps without another transcription or
analysis request. Repeat `--transcript` for additional sources. Analysis and
transcript files share an eight MiB limit on bytes actually read, admitted before
JSON parsing. Sources must bind to analyzed media; output protection includes
supplied transcripts and their media. Transcripts require `usage.source_sha256`,
64 hex digits (case-insensitive), matching the media content SHA-256; byte-identical
copies are accepted. The same check applies to `analyze --transcript`,
`describe frames --transcript`, `review lint --transcript` and `from_transcripts` edit
caption tracks. Missing, invalid or mismatched fingerprints fail; re-run
`skywrite transcribe` on the media, or point the transcript/edit at the media it
was made from. Media hashes use the stat-validated [cache](cache-models.md).
Standalone `captions convert` and `sync --method transcript` do not bind
transcripts to media. Audible cuts inside supplied words produce
advisory stderr warnings; intentional B-roll and J/L cuts remain valid. The saved
result remains Edit v1. The requested editing-model call occurs only after local
input admission and source checks.

Content hashes use the opened regular file's size as a finite read boundary, with
one extra byte to detect growth. Verification and exact audio timing additionally
enforce their 16 GiB hash limit on the opened file and on actual reads. A file
that grows or shrinks during a hash is rejected instead of being followed to an
indefinitely moving EOF. This does not impose a global size limit on stable
original media used by other commands. Filesystem scheduling is separate from
FFmpeg's decode deadlines.

## Checks before publishing a render

An agent cannot watch or hear a render, so these checks give it numbers to quote.
Each runs only when requested, adds an entry to `checks` with its measured values,
and makes the command exit 1 (with the full report still on stdout) when it fails.
The report's `not_measured` lists the checks that were not run and what verify
never measures (creative quality, the sync of particular words, how the mix sounds).

```sh
skywrite verify out.mp4 --ends-together --loudness --black --frozen --edit edit.json
skywrite verify out.mp4 --loudness --target-lufs -14 --max-true-peak -1
```

- `--ends-together [--end-tolerance 0.05]` (`ends_together`): the first video and
  first audio streams end within the tolerance, in seconds from the container start.
  The video end is the latest packet timestamp plus duration among the packets of
  the stream's own last 30 seconds (edit lists applied; seeking from the stream's
  own end, so a 5-s audio track in a 60-s MPEG-TS file is still found); a last
  packet without a duration gets the measured spacing of the last two packets
  (`video_end_basis` says which). If no packet timestamps can be read, the check
  fails with a reason and the rest of the report is unaffected. FFmpeg's progress `out_time` is not used:
  6.1.1 reports the last frame's start (2.967 s for a 3-s 30-fps file), other
  versions its end. The audio end is the earlier of its packet end and its decoded
  samples' end, because AAC encoder padding is trimmed in one or the other: 8.0.1
  trims it in packet timestamps (a 3-s render decodes to 3.008 s), 9.1 in the
  decoder (a 2.97-s tone decodes to exactly 2.970 s but its last packet ends at
  2.987 s). Every decoded report has `stream_ends` with both audio ends and the
  difference (positive: the picture runs longer). Measured on `skywrite edit render`
  output (one clip, 3, 4.37, 7.123 and 10.01 s at 25, 24000/1001, 30 and 60 fps):
  within 0.7 ms on FFmpeg 5.1.2, 6.1.1, 7.1.1, 8.0.1 and 9.1. The 0.05-s default also
  allows other encoders' one-AAC-frame (21 ms) padding and fails at two frames of
  30-fps video.
- `--loudness [--target-lufs -16 --lufs-tolerance 1 --max-true-peak -1]`
  (`integrated_loudness`, `true_peak`): EBU R128 integrated loudness (LUFS),
  loudness range (LU) and true peak (dBTP) of the whole first audio stream, from
  the same FFmpeg `loudnorm` analysis `audio normalize` uses. -16 LUFS and -1 dBTP
  suit online video and podcasts; streaming services that normalize to -14 LUFS
  turn louder mixes down. Use `--target-lufs -14` for music streaming or `-23` for
  EBU broadcast. Digital silence has no integrated loudness and fails as not
  measurable. A mix normalized only as a dialogue stem at `audio normalize`'s
  default -18 LUFS fails the -16 ±1 check, as intended.
- `--black [--black-min-seconds 0.5 --black-pixel-threshold 0.1]`
  (`first_frame_black`, `black_frames`): FFmpeg `blackdetect` with no minimum, so
  every black span is listed with `start_seconds`, `end_seconds`,
  `duration_seconds`, `at_start`, `at_end`, `allowed` and `fails`. A frame is black when 98%
  of its pixels have luma below the pixel threshold. `first_frame_black` fails on a
  black first frame alone (a late-starting picture, a black thumbnail);
  `black_frames` fails on any span of at least the minimum, such as a gap between
  clips. blackdetect ends a span that runs to the end of the stream at the last
  frame's start, so the detectors run (from the stream's first frame, offset to the
  container start like the packet times: FFmpeg itself starts a delayed MPEG-TS
  video stream at zero but keeps an MP4's offset) on a copy of the decoded frames with one
  white frame appended: a span that lasts to the end closes at the stream end, and
  a visible last frame is not counted as black. Fades through black leave
  short spans, measured on renders of a 4-s clip fading out and the next fading in:
  0.03, 0.10, 0.23 and 0.43 s for 0.25-, 0.5-, 1- and 2-s fades of bright test
  footage, and 0.20, 0.37, 0.77 and 1.57 s for flat 25%-grey footage. So 0.5 s
  passes ordinary dips; `--black-min-seconds 0` fails on any black frame.
- `--frozen [--frozen-min-seconds 2 --frozen-noise-db -60]` (`frozen_frames`):
  FFmpeg `freezedetect` spans in which no frame differs from the last by more than
  the noise level for at least the minimum. freezedetect measures to the last
  frozen frame's start, so it runs 1.5 frames below the minimum and the minimum is
  applied to the full span (a 2.033-s still at the end is found with a 2.03-s
  minimum and not with 2.04). Intentional stills, title cards and slides are frozen
  too (pass `--edit`). Each span has `start_seconds`, `end_seconds`,
  `duration_seconds`, `allowed` and `fails`. Spans inside a black span are counted
  once, as black (`black_spans_not_repeated`).
- `--edit EDIT.json [--edit-tolerance 0.1]`: the edit the file was rendered from,
  so the black and stills it intends pass. A black span at the start of the file
  that lies within the first clip's `fade_in` is `allowed: "fade_in"` (and
  `first_frame_black` passes with `allowed`), one at the end within the last clip's
  `fade_out` is `allowed: "fade_out"`, and a frozen span within a still-image clip,
  while no other clip is on screen, is `allowed: "still"`. Only visible clips
  (opacity above 0) count; the fade-in window ends where any visible clip on screen
  is fully shown, and the fade-out window starts where the last one starts fading,
  so a lower clip's fade does not excuse black under an opaque clip without one.
  Each window has the tolerance as slack on both sides (0.1 s, three
  frames at 30 fps, covers frame rounding). Black before a late first clip, black
  in the middle, and black or a freeze that outlasts the fade or still still fail.
  `edit_allowances` lists the windows used. The edit is validated as
  `skywrite edit validate` does, so its sources must be present. Without `--edit`,
  `not_measured` says the edit's intent was not considered.

Requested video checks on a file without video, and `--loudness` on a file without
audio, fail with a `reason`. `--black` and `--frozen` run in the video decode verify
already makes (on a split copy of its frames); `--loudness`
adds one audio pass. The filter output parsed here (`black_start:`/`black_end:`,
`lavfi.freezedetect.freeze_start:`/`freeze_end:` and loudnorm's JSON) is the same on
FFmpeg 5.1.2, 6.1.1, 7.0.2, 7.1, 7.1.1, 8.0.1, 8.1.2 and 9.1 apart from filter
instance names, which are not used; on 5.1.2 the lines can share a line with
progress output. `tests/verify_checks.rs` passes on all of them.
