# Reviewing an edit

Skywrite's review commands gather evidence; they never change an edit and never
judge taste. Delivery, emotional fit, B-roll relevance and music choice remain the
editor's call, guided by the user's saved `skywrite styles`.

## Contact sheets of exact frames

```sh
skywrite review sheet camera.mp4 --start 120 --duration 1 --every 2 -o around-cut.jpg
skywrite review sheet camera.mp4 --start 0 --duration 600 --interval 15 --columns 8 -o overview.jpg
```

Frames are decoded consecutively from the requested source range (seconds from the
first presented sample) and tiled left to right, each labelled with its source time
and frame number (frame numbers are exact for constant-frame-rate sources). `--every N`
keeps every Nth decoded frame; `--interval S` keeps the first decoded frame in each
S-second bucket aligned to the source's first video frame. Empty buckets across
timestamp gaps yield no frame. Ranges share this grid: a range starting after a
bucket's first frame skips that bucket. At most 48 tiles fit one sheet; the command
fails rather than silently skipping frames. Sheets are rendered locally with the
embedded Takumi renderer and make no network requests. View them with your image tools to judge expressions,
gestures, motion, steadiness and timing that single sparse frames hide.

A useful "focus in" pattern for choosing a cut:

1. Transcribe for word timestamps.
2. Survey the source coarsely (`--interval`, or `analyze --local`).
3. Around each candidate cut, make a dense sheet (`--every 2` over about one second).
4. Choose the cut from the words and the frames, then audition it.

## Audition clips around every cut

```sh
skywrite review cuts edit.json --handles 1.5 --output-dir cut-review
skywrite review cuts edit.json --render preview.mp4 --output-dir cut-review-2
```

Each clip plays the rendered edit `--handles` seconds either side of one cut, with a
list of what starts and ends there (track, source and source time). Without
`--render` the edit is rendered once to a private temporary file. The output
directory must be new. Watch and listen for clipped or repeated words, stumbles,
applause or music tails cut off, jump cuts and sync.

## Objective checks

```sh
skywrite review lint edit.json --transcript presenter.transcript.json --transcript audience.transcript.json
```

Supply word-timed transcripts for dialogue sources. Each must include
`usage.source_sha256`: 64 hex digits (case-insensitive) matching its media content
SHA-256. Byte-identical copies are accepted. Missing, invalid or mismatched
fingerprints fail before level checks; re-run `skywrite transcribe` on the media,
or point the transcript/edit at the media it was made from. Hashes use the shared
[cache](cache-models.md). Warnings include:

- `visible_speaker_without_dialogue`: a dialogue source's picture is visible (not
  covered by an opaque upper track) while its words fall where no dialogue is
  audible, such as a muted presenter under a music intro.
- `repeated_word_across_cut` / `repeated_word_in_select`: the same word heard twice
  in a row (for example "Evanston Evanston"); often a stumble or restart.
- `word_clipped_at_cut`: a clip starts or ends inside a transcribed word (beyond
  one frame or 40 ms of transcript slack), with no straight split continuing it and
  no fade. Unlike the level check, this works on quiet recordings.
- `filler_words`: fillers such as "um" inside audible dialogue. Transcription often
  omits fillers, so their absence proves nothing.
- `sound_cut_mid_flow`: a clip ends without a fade while its source keeps sounding
  at a similar level, as when applause or a music tail is cut off.
- `brief_shot`: a picture on screen for less than `--min-shot-seconds` (default
  0.5, allowed 0 to 10; `0` turns the check off; `checked.min_shot_seconds`
  reports the value used), such as a talking shot showing for a few frames between
  two B-roll clips. Each warning gives `timeline_start`, `timeline_end` (seconds)
  and `frames`. The check works from clip placement, opacity and fades, not
  pixels. Each frame's picture is the clip that dominates it plus every clip at
  least half visible; a clip's weight is its opacity times its fades (the
  renderer's linear alpha ramps, sampled at frame starts), reduced by full-frame
  opaque video above it. Reported: a clip that is part of the picture for less
  than the threshold (a short overlay or cutaway, even one with fades), and a
  stretch with one picture, shorter than the threshold, that starts or ends with
  an abrupt change (at least half of the visible picture changing between two
  frames: a cut, a jump in source time, a black gap, an `opacity: 0` piece inside
  a take). Not reported: gradual
  transitions from one picture to another (a dissolve, a fade from or to black),
  clips at `opacity: 0`, a straight split that continues the same take on the
  same track (same source, contiguous within one frame of the slower clip's
  source step after frame rounding), and cuts hidden under full-frame opaque
  video on a higher track. Raise the threshold to match your pacing (for example
  `1`).
- `bed_close_to_dialogue`: a non-dialogue source (music, ambience) within 10 dB of
  the dialogue during speech, with the edit's gains applied. Tracks with a declared
  `role` are classified by it; otherwise a source without a transcript is a bed.

Level checks decode short windows of the sources (at most 256 measurements);
`--no-levels` skips them. They measure source audio with the edit's gains, not the
final mix, and do not replace listening.

Cover is decided from metadata, not pixels. Only full-frame video (by displayed
size, so rotated phone footage counts) at full opacity whose pixel format has no
alpha channel, per FFmpeg's pixel format list, hides what lies beneath it.
Stills, letterboxed pictures and alpha-capable video (ProRes 4444, QTRLE `argb`)
never count as cover, even when every pixel is opaque, so a speaker or a cut under
them stays visible and can add warnings. Video whose pixel format FFmpeg does not
describe is never cover either and is listed in `checked.uncertain_cover_sources`.
