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
- `unfinished_ending`: the edit's last heard dialogue word may stop mid-thought.
  `reasons` lists why: `no_sentence_final_punctuation` (the last kept word has no
  `.`, `!`, `?` or a CJK, Arabic or Devanagari equivalent; an ellipsis trails off and
  counts as unfinished; judged only when the transcript has sentence punctuation at
  all, which `checked.ending.punctuation` reports), `next_word_continues` (the next
  source word, within 2 s, joins on: "and", "but", "so", "because", "or" and their
  German, French, Spanish, Italian, Portuguese and Dutch equivalents; after an
  unpunctuated word also clause words such as "which", "that", "when", "if"), and
  `open_list` (an announced count such as "three reasons" with fewer ordinals
  reached, or "first … second" with the next ordinal just after the cut; English
  number words, digits 2–10 and ordinals). The warning has the last heard words,
  the cut-off source words (`next_source_words`) and the timeline and source times.
- `dead_air`: silences between heard dialogue words longer than
  `--dead-air-seconds` (default 0.6), measured on the output timeline: word times
  are mapped through every audible dialogue clip's start, trim and speed, so J/L
  cuts, dialogue continuing under B-roll and muted dialogue all count as heard
  or not heard. Each item has `timeline_start`, `timeline_end`, `seconds`, the
  words before and after, `across_cut`, `other_sound` (music or effects audible in
  the gap; a bed does not hide an awkward pause) and `untranscribed_dialogue`
  (dialogue-role clips without a transcript, which may be speaking). Silence before
  the first and after the last heard word is not counted. Mark deliberate beats
  in the edit's `pauses` list (`{"start": 41.2, "end": 43, "note": "laugh"}`,
  timeline seconds; see [the edit format](edit-format.md#pauses)): a gap the
  pauses cover except for at most the threshold is listed in
  `checked.dead_air.excluded_by_pause` instead of warned about.
- `sound_cut_mid_flow`: a clip ends without a fade while its source keeps sounding
  at a similar level, as when applause or a music tail is cut off.
- `brief_shot`: a take, or black between full-frame pictures, on screen for less than
  `--min-shot-seconds` between two hard cuts (default 0.5, allowed 0 to 10; `0`
  turns the check off; `checked.min_shot_seconds` reports the value used), such
  as a talking shot showing for a few frames between two B-roll clips. Each
  warning gives `timeline_start`, `timeline_end` (seconds), `frames` and
  `source` (`null` for black). **Fades and dissolves are not checked yet.** The
  check works from clip placement and opacity, not pixels (stills are
  decoded once to see whether they cover). A take is one clip, or
  clips that continue it across straight splits on one track (same source,
  contiguous within one frame of the slower clip's source step after frame
  rounding; pieces of one still always continue; the same take continued on
  another track counts as a cut). A take is on screen while its opacity times
  what full-frame opaque cover above it lets through is at least 0.25; black is
  on screen while at least 0.25 of the light reaches the bottom through cover.
  **Black gaps are checked only between full-frame pictures:** the picture goes
  from fully covered to black, with nothing on it but titles that are not
  opaque, and back. Every other black change (letterbox bars, black around a
  picture-in-picture or an opaque logo, black under a partial-opacity picture)
  is not checked yet. A shot's length is exact on the
  edit's frame grid. It is reported only when both its edges are hard cuts (the
  timeline's start and end count as hard cuts): no visible clip fades anywhere
  in it or in the frame on either side of it, and no take continues at another
  opacity at its edges or inside it. That reports a flash frame, a short cutaway,
  a short overlay or title (also at partial opacity), or a black gap between
  full-frame pictures cut in and out, also under a transparent title; a title
  or overlay starting or ending near a cut is not a shot (but a full-frame
  opaque overlay above 0.75 opacity hides what is beneath it, so its edges are
  cuts). A stretch with a fade, a dissolve or an opacity change, and black that
  is not between full-frame pictures, is never reported as brief; when it is under the threshold it is listed in
  `checked.brief_shots_not_checked` (`timeline_start`, `timeline_end`,
  `source`), and a line in `not_checked` gives their count, so you can watch
  them with `skywrite review cuts`. Clips at `opacity: 0` show nothing, and cuts
  under full-frame opaque cover on a higher track are hidden. Lengths are
  counted on the edit's frame grid, and a render can be a frame off it
  (measured with FFmpeg 8.0.1: at 29.97 fps a clip after a cut starts one frame
  late; at 30 fps a clip after a gap or a short clip can start one frame late or
  end one frame early; a video whose picture starts after its `source_in` shows black for that
  lead). Raise the threshold to match your pacing (for example `1`).
- `bed_close_to_dialogue`: a non-dialogue source (music, ambience) within 10 dB of
  the dialogue during speech, with the edit's gains applied. Tracks with a declared
  `role` are classified by it; otherwise a source without a transcript is a bed.

Without any `--transcript`, or for an audible `dialogue`-role source without one,
the word checks cannot run; the report's `not_checked` list says so (and lint prints
it to stderr), so an empty `warnings` list is not mistaken for a clean edit.

Level checks decode short windows of the sources (at most 256 measurements);
`--no-levels` skips them. They measure source audio with the edit's gains, not the
final mix, and do not replace listening.

Cover is decided from metadata, and for stills from their pixels. Only a
full-frame picture (by displayed size, so rotated phone footage counts; it must
fill the frame to the pixel as the renderer fits it, so 1366x768 on a 1920x1080
edit, fitted to 1920x1079, is letterboxed) at full opacity that is certainly opaque hides what lies beneath it: video whose pixel
format has no alpha channel, per FFmpeg's pixel format list, or a still whose
pixels all decode as opaque (palette and RGBA PNGs are decoded once to check).
Letterboxed pictures, alpha-capable video (QTRLE `argb`, ProRes 4444 with alpha)
and stills with transparent pixels (PNG titles) never count as cover, even when
every pixel of the video is opaque, so a speaker or a cut under them stays
visible and can add warnings. Video whose pixel format FFmpeg does not describe,
and a still that cannot be decoded, are never cover either and are listed in
`checked.uncertain_cover_sources`.

The 0.6-s dead-air default is measured: in three 60-s windows of unedited AMI
meeting speech (ES2002a) transcribed on device with Parakeet TDT 0.6B v3 (80-ms
word-time steps), half of the 621 inter-word gaps were 0 s, the 95th percentile
0.32 s, the 99th 0.64 s, and 10 gaps (1.6%) were longer than 0.6 s. So 0.6 s flags
the slowest pauses of natural conversation, which an edit usually tightens; raise
it for deliberate, slower delivery. Endings and pauses are judged from the
transcript only: words the transcript misses (laughter, crosstalk) read as
silence.

Lint the edit before rendering; after rendering, check the file itself with
`skywrite verify OUT.mp4 --ends-together --loudness --black --frozen`
([verification](verification.md)).
