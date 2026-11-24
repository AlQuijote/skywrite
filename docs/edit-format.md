# Edit format, version 1

Skywrite compiles a small layered JSON edit into an MP4 render, an executable
Python/FFmpeg script, FCPXML, Premiere-importable XML, or an optional
Hyperframes/Remotion project. Agents write this edit format rather than
authoring interchange XML.

```json
{
  "version": 1,
  "name": "Interview",
  "width": 1920,
  "height": 1080,
  "fps": {"num": 30, "den": 1},
  "sources": [
    {"path": "camera.mxf", "lut": "luts/clog2-to-709.cube",
     "color": {"transfer": "canon-log2", "primaries": "cinema-gamut", "note": "confirmed by operator"}}
  ],
  "video": [
    {"name": "A-roll", "clips": [
      {"start": 0, "source": "camera.mxf", "source_in": 12.4, "duration": 9, "audio": {"volume": 0}},
      {"start": 9, "source": "title.png", "duration": 3, "fade_out": 1}
    ]},
    {"name": "B-roll", "clips": [
      {"start": 4, "source": "crowd.mp4", "source_in": 30, "duration": 3, "fade_in": 0.5, "audio": {"volume": 0}}
    ]}
  ],
  "audio": [
    {"name": "Dialogue", "clips": [
      {"start": 0, "source": "recorder.wav", "source_in": 20.1, "duration": 9, "speed": 0.9999, "fade_out": 0.25}
    ]},
    {"name": "Music", "clips": [
      {"start": 0, "source": "bed.mp3", "duration": 12, "volume": 0.5,
       "volume_keyframes": [{"time": 0, "volume": 1}, {"time": 1, "volume": 0.3},
                            {"time": 8.5, "volume": 0.3}, {"time": 9.5, "volume": 1}],
       "fade_out": 2}
    ]}
  ],
  "master": {"limiter_db": -1}
}
```

`skywrite schema` prints the complete JSON Schema with this example.

### Source paths and trust

Edit files, and the Premiere XML or `.prproj` files you import, are **trusted
input, like scripts**: they choose which local files Skywrite reads, renders and
copies. Relative paths resolve against the edit file's directory; absolute paths
and `../` paths are allowed, so footage on other drives keeps working. Check an
edit from someone else before exporting it, just as you would read a script
before running it.

To make that visible, every `edit render` and `edit export` result has an
`external_sources` array: the canonical path of every clip source, `sources`
entry, LUT, caption file and caption transcript (and the media a transcript
describes) that resolves outside the edit file's directory; it is empty when
everything is inside it. A hard link or bind mount inside the directory counts
as inside. Hyperframes and Remotion projects copy their clip sources into the
deliverable, so review that list before sharing a project.

Media, LUT, caption and transcript files must be the regular files they name: a
symlink, directory, FIFO, device or socket is refused at validation
(directories higher up the path may be symlinks). Import refuses an absolute XML
or `.prproj` media path that exists here and is a symlink or special file;
missing paths are allowed so you can relink them, and relative `.prproj` paths
are checked when the written edit is validated. Project export and LUT loading
open files without following symlinks, and project export refuses a source
whose file or directory was swapped after validation. Rendering passes the
validated paths to FFmpeg.

## Timeline model

Every clip has an explicit `start` on the output timeline. `video` is a list of
tracks; track 0 is the bottom layer and later tracks composite above it. Clips on
one track must not overlap; put overlapping material on another track. Empty
timeline regions are black and silent. The timeline ends at the latest clip end,
or at an explicit top-level `duration` (which may add trailing black).

Paths are local files, resolved relative to the edit JSON. Input files remain
unchanged. Sound-only media belong on `audio` tracks. A video clip's own sound is
*linked* to it and controlled by its `audio` object; set `"audio": {"volume": 0}`
to mute it, for example on B-roll above continuous dialogue or with a separate
recorder.

`source_in` is measured from the source's first presented sample (the container's
reported presentation start). Inspection reports elapsed presentation duration:
Matroska/WebM's ending timestamp is rebased to that start; MP4 and MPEG-TS
already report elapsed duration. `duration` is always **output seconds**;
`speed` is **source seconds per output second**. A 10-second output clip at speed
1.01 consumes 10.1 source seconds. Source trims are not timeline positions.

Clip audio is converted to 48 kHz and then follows `speed` in three ways. At
speed 1 it passes through unchanged. Within 0.1% of 1 (clock-drift conform, such as a `skywrite
sync` `playback_rate`) it is resampled: timing is sample-exact over any length,
and pitch shifts by under 1.8 cents. Larger (creative) changes keep pitch with
a time-stretcher, chosen by `edit render --stretcher` and `edit export --to
ffmpeg --stretcher` (speed 1 and the resample range are unaffected):

| `--stretcher` | Behaviour |
|---|---|
| `auto` (default) | `rubberband` when this FFmpeg build has the filter, else `atempo` with a one-line warning |
| `rubberband` | `rubberband`; fails with an error if this FFmpeg lacks it |
| `atempo` | always `atempo` |

FFmpeg is checked once per command (`ffmpeg -hide_banner -filters`), only when a
clip needs stretching. `rubberband` runs with `window=short:channels=together`.
FFmpeg's filter leaves the library's latency in its output, off by an amount
that depends on speed (14.7 ms early at 0.5x, 6.1 ms late at 2x), so Skywrite pads the clip audio with 16384 samples
of silence at each end and drops the leading output samples that a calibration
measured for that exact speed: five 50 ms bursts through the same filter chain on
this FFmpeg. Above 2x it chains 2x stages; a single 4–8x stage loses most of a
short sound and moves it by up to 28 ms. `atempo` cannot be corrected the same
way, because its offset moves between events. Measured with FFmpeg 8.0.1 and
librubberband 4.0 (onsets of 10 ms clicks every 2 s, AAC render):

| Speed | rubberband (compensated) | atempo |
|---|---|---|
| 0.5 | -2.3 ms | -36 to -21 ms |
| 0.8 | +0.3 to +5.2 ms | -28 to -13 ms |
| 0.9 | -1.1 to +0.7 ms | -24 to -21 ms |
| 1.1 | -0.1 to +0.5 ms | -19 to -15 ms |
| 1.25 | -0.1 to +0.5 ms | -13 ms |
| 2.0 | -0.5 to +0.8 ms | -17 to -3 ms |

In spot checks outside that range, compensated rubberband landed -8 ms at 0.125x,
+3.7 ms at 0.25x and within 5.7 ms at 3–8x; atempo landed up to about 60 ms early
at 0.125–0.25x. On speech (0.5–2x) the energy envelope lands within 6 ms with
rubberband, against -22 to +14 ms with atempo. A clip whose speed needs
stretching is checked even when its audio is silent in the clip window, and one
command calibrates at most 64 distinct speeds. The result's `audio_stretcher` reports the request,
whether rubberband was found, the stretcher used and each measured calibration.
An exported FFmpeg script keeps the stretcher and calibration resolved on the
exporting machine, names them in a comment, and needs the same kind of FFmpeg
build.

### Placing a sync result

`skywrite sync apply --sync result.json --edit edit.json -o out.json` writes a
`skywrite sync` result into an edit (see the README and `skywrite skills read
synchronization`); use it rather than the arithmetic below. By hand, for a
result `reference_time = offset_seconds + scale * asset_time` and a reference
clip at timeline `start` Rs with `source_in` Rin and speed 1: give the asset
clip a `start` on the output frame grid, no earlier than `Rs + offset_seconds -
Rin`; set `source_in = (Rin + start - Rs - offset_seconds) / scale` and `speed
= playback_rate` (`1 / scale`), and end it where either recording ends. Keep
the sub-frame remainder in `source_in`, which is not rounded; a "start rounded"
warning on a synced clip means the placement moved. For example, at 24 fps
with the camera at `start` 0 and `source_in` 0, `offset_seconds` 7.2 gives
`start` 7.208333333333333 (frame 173 = 173/24, at full precision) and
`source_in` 0.008333; `offset_seconds` -3.5 gives `start` 0 and `source_in`
3.5. A `discontinuous` result needs one clip per segment, each limited to its
`asset_range`.

A clip's `start` and its end (`start + duration`) are each rounded once to the
nearest output frame, and its frame length is end minus start, so clips written
back to back in round seconds share their boundary frame at 23.976, 29.97 or
59.94 fps (never a one-frame overlap or gap). The optional timeline `duration`
is rounded to the nearest frame too; the validator reports rounding. `source_in` keeps its
precision (word-accurate dialogue cuts). Because start and end round to the
nearest frame, a clip that uses a source to its very end may run up to one output
frame past it; that tail is accepted with a warning and padded (silence, held
last picture) by Skywrite's renderers, so durations need not be floored.
FCPXML and Premiere XML exports instead end such a clip on the last whole frame
inside its media, with a warning, because an NLE may treat the tail as missing
media. Longer overruns are rejected. Use rational rates such as
`{"num":30000,"den":1001}` for NTSC.

## Pictures

Video and stills scale to fit the canvas, preserving aspect ratio; phone footage
with a display rotation fits by its upright, displayed size. Fitting treats
stored pixels as square: a source's sample aspect ratio is ignored, so anamorphic
or DV media keeps its stored shape. Letterboxing is
transparent, so lower tracks show through; at the bottom that means black. Stills
keep their alpha channel (for example Takumi title cards with transparency) and
require `source_in: 0` and `speed: 1`.

- `opacity` (0–1, default 1) blends the picture over lower tracks.
- `fade_in` / `fade_out` (seconds) fade the picture from/to transparent; on the
  bottom track that is a fade from/to black.

## Sound

Audio clips and a video clip's `audio` object share the same controls:

- `volume`: linear gain, 0–4. 0 mutes; 1 is the original level.
- `volume_keyframes`: `[{"time": seconds_from_clip_start, "volume": gain}]`,
  strictly increasing, interpolated linearly and held before the first and after
  the last point. Use them to ride a music bed around dialogue and applause.
- `fade_in` / `fade_out`: linear fades in seconds.

Effective gain is `volume × keyframe envelope × fades`. All sound is mixed at
48 kHz stereo; a mono source is placed in the center at about -3 dB per channel.
Mixing sums tracks at unity gain. Nothing limits the mix unless you set
`master.limiter_db` (−24 to 0 dBFS); without it, loud overlaps can clip. Use
`skywrite verify` on renders to check peaks.

### Track roles

Any track may declare what its sound is: `"role": "dialogue"`, `"music"` or
`"effects"` (on a video track, the role describes its linked sound):

```json
"video": [{"name": "Interview", "role": "dialogue", "clips": [...]}],
"audio": [{"name": "Bed", "role": "music", "clips": [...]}]
```

Roles are optional and change nothing in a normal render. They enable:

- `skywrite edit render edit.json --roles dialogue -o check.mp4`, a stem render with
  only those roles' sound (pictures unchanged). Use it to verify dialogue by
  transcription without a music bed masking quiet words. Roles combine with commas
  (`--roles music,effects`). Every audible track must then declare a role; Skywrite
  refuses rather than guessing.
- FCPXML exports label clips with the matching audio role (an undeclared audio
  track stays FCP's default, dialogue). Premiere XML has no audio roles.
- `review lint` treats `music` and `effects` tracks as beds and `dialogue` tracks as
  dialogue, instead of inferring that from which sources have transcripts.

## Color

`sources` declares per-source settings. `color` records the operator-confirmed
capture encoding (for example Canon Log 2 / Cinema Gamut); it overrides container
metadata in reports but does not change pixels. `lut` names a `.cube` file
(1D or 3D, at most 32 MiB combined) that the MP4 render applies to that source
before scaling. Skywrite does not ship camera LUTs: use the manufacturer's or your
colorist's. A source declared as a log encoding without a LUT produces a warning,
because the render will look flat. Final grading belongs in the NLE.

## Importing from an NLE

```sh
skywrite edit import premiere-export.xml --output edit.json
skywrite edit import project.prproj --output edit.json   # experimental
```

Final Cut Pro 7 XML (what Premiere writes with File > Export > Final Cut Pro
XML) is the supported import: video tracks become layers. Linked sound becomes
a clip's `audio` only when its source, start, end, in point and speed match the
video; disabled matching sound sets `audio.volume` to `0` (mute). Otherwise,
enabled linked sound becomes an independent audio clip with its own source and
timing, and the video's own sound is muted to avoid doubling. Enabled linked sound
is also kept independently when its video is disabled or cannot be resolved.
Unlinked sound becomes audio tracks. Stereo partners merge only when source,
start, end, in point, speed and enabled state match and their source channels differ
(or they link to each other with no conflicting channel identity); same-channel layers
stay separate. Time Remap speed, Opacity and Audio Levels values and keyframes,
fades, markers (ranged markers named `Pause` as `pauses`, other markers as
chapters) and the sequence rate and size carry over.
Generators, transitions and other effects are reported rather than silently
dropped. Media paths are kept absolute; if they live on another machine the
result reports validation errors so you can relink. Imported files are trusted
input (see [Source paths and trust](#source-paths-and-trust)).

`.prproj` import is experimental and read-only: Premiere projects are
undocumented, gzip-compressed object graphs that change between versions. It
reads clip placement, in points and media paths only. Prefer exporting XML from
Premiere, and report mismatches with `skywrite nit`. Skywrite never writes
`.prproj` files.

## Outputs

```sh
skywrite edit validate edit.json
skywrite edit render edit.json --output edit.mp4
skywrite edit export edit.json --to premiere --output edit.xml
skywrite edit export edit.json --to fcpxml --output edit.fcpxml
skywrite edit export edit.json --to ffmpeg --output render.py
python3 render.py --output edit.mp4
skywrite edit export edit.json --to hyperframes --output hyperframes-project
skywrite edit export edit.json --to remotion --output remotion-project
```

What each target carries:

| Feature | MP4 / Python | Premiere XML | FCPXML | Remotion | Hyperframes |
|---|---|---|---|---|---|
| Layers, timing, speed | yes | yes | yes | yes | yes |
| Opacity, picture fades | yes | Opacity keyframes | adjust-blend | yes | yes (GSAP) |
| Volume, keyframes, audio fades | yes | Audio Levels keyframes | adjust-volume | yes | constant volume only |
| LUT | yes | warning | warning | warning | warning |
| Master limiter | yes | warning | warning | warning | warning |
| Subframe `source_in` | yes | rounded to frames (warned) | yes | rounded to frames | yes |
| Captions / chapters | yes | markers only | note + markers | sidecars | sidecars |
| Pauses | ignored | sequence markers | markers | ignored | ignored |

The Python script uses only the standard library and FFmpeg; it invokes FFmpeg
with an argument list, never a shell. Both direct rendering and the script
publish an H.264/AAC MP4. LUT and caption text travel as private generated
sidecar files; only generated ASCII paths enter FFmpeg filters. On Linux, the
script stops its owned FFmpeg process group on ordinary SIGINT/SIGTERM
cancellation, timeout, or failure before removing its private temporary output.
The leader is observed with `waitid(WNOWAIT)` and remains unreaped until the final
group signal, preserving its process-group identity. Other POSIX platforms must
provide these wait primitives or the script refuses to start FFmpeg; their runtime
behavior has not been tested here. Existing output files require `--overwrite`.
Input edit, LUT, caption, transcript and analysis files and original media are
protected even with that flag; an exported Python renderer also protects its own
script. Exported project directories must be new even when `--overwrite` is
supplied, preserving authored projects.

**Premiere XML** (`--to premiere`) is Final Cut Pro 7 XML (`xmeml` version 5),
which Premiere Pro imports via File → Import. Each video track keeps its layer,
its linked sound goes to a matching audio track, and audio tracks follow. Audio
uses the structure Premiere itself exports (`explodedTracks`): a stereo layer is
a pair of tracks (`currentExplodedTrackIndex` 0 and 1, `outputchannelindex` 1
and 2) with one clipitem per channel (`sourcetrack` 1 and 2), linked with its
video clip; Premiere shows it as one stereo clip. Mono sources get a mono track
of their own (a track mixing mono and stereo sources is split, with a warning),
and sources with more than two channels use their first two. Speed
uses the Time Remap effect, opacity and fades use Opacity keyframes, and gain uses
Audio Levels keyframes. Each video clip is fitted in the sequence with a Basic
Motion Scale (omitted when the fit is exactly 100%), as the render does for square-pixel sources (by displayed size, so
rotated phone footage fits upright); without it Premiere shows media at its own
size. Premiere applies a source's pixel aspect ratio, which the render ignores, so
anamorphic or DV media is framed differently in the two. The format counts whole frames at integer or NTSC rates,
so subframe source in points are rounded (with a warning). Skywrite checks the
XML's structure and timing with an independent reader. Import into Premiere Pro
26.5.2 on macOS 26.3.1 was checked by a contributor on Skywrite 9dd2c3b (results
posted on PR #2 on 2026-11-24), with no import dialog: stereo and mono audio on
their own tracks, Audio Levels keyframes and fades, whole-second cuts at 23.976
fps without gaps or overlaps (burned-in frame numbers matched the playhead), a
recorder clip's one-frame rounding overrun trimmed so no media went missing,
plain exports linking on the editing machine, `--media-root` paths linking
without a relink, and Time Remap speed (1.5x, landing on the expected source
frames). Not covered
by that check: captions (imported separately as SRT), Opacity keyframes, the
Basic Motion Scale (a rotated iPhone reel imported fitted in Premiere Pro 2026,
outside that check), other effects and other Premiere versions; report results
with `skywrite nit`.

**Media paths for exports made elsewhere.** FCPXML and Premiere XML reference
media by absolute URL, by default this machine's paths. When the export is made
on another machine than the editor's (for example in the cloud), pass the
editor's folder with `--media-root`:

```sh
# Cloud: the project lives in /work/film (edit in /work/film/edits, media in /work/film/media)
skywrite edit export edits/cut.json --to premiere -o edits/cut.xml --media-root /Users/sam/Projects/film
# result: "media_base": "/work/film", "media_root": "/Users/sam/Projects/film"
# cut.xml now points at /Users/sam/Projects/film/media/recorder.wav, ...
```

Each source keeps its path relative to `media_base`, the deepest folder holding
the edit file and every source it uses. Copy `media_base`'s contents (with
subfolders) to the media root on the editing machine, then import the XML:
Premiere and Final Cut link on open. Windows folders work too
(`--media-root 'D:\Projects\film'`). The root must be absolute. `edit import`
reads macOS and Linux roots back; Windows roots are only for the editor's
machine. Keep media file names unique: the export warns when two
sources share a name (case-insensitively), because after one manual relink an NLE
locates the remaining files by name. Composition targets copy their media and
refuse `--media-root`.

**FCPXML 1.10** uses file URLs and rational time values. A full-length gap anchors
each track as a connected storyline (video tracks above, audio tracks below), so
no track's placement depends on another's retiming. It references original files
rather than embedding them. The format follows Apple's
[FCPXML reference](https://developer.apple.com/documentation/professional-video-applications/fcpxml-reference)
and [timeMap specification](https://developer.apple.com/documentation/professional-video-applications/timemap).
Actual import into Final Cut Pro requires validation on macOS. Source `<format>`
elements carry each file's stored width and height, not the displayed size of
rotated phone footage (a clip stored 3840x2160 with rotation -90 is written as
3840x2160), and no transform is written, so fitting is left to Final Cut Pro's
own conform. Whether Final Cut Pro expects the stored or the displayed size for
rotated media has not been checked; check the framing of rotated clips after import.
Premiere XML likewise writes the stored size in each file's `samplecharacteristics`
(Premiere reads the media itself), while its Basic Motion Scale uses the
displayed size.

Composition projects copy unique media into their output directory, with a
20 GiB total media limit; `external_sources` in the result lists every copied
source from outside the edit file's directory. They need their optional runtime: installed Hyperframes,
or `npm install` inside the generated Remotion project followed by `npm run
studio`/`npm run render`. Hyperframes uses a pinned GSAP script; its renderer may
need network access or its own vendor cache. Remotion's runtime has its own
[license](https://www.remotion.dev/license). Neither runtime is needed for the
Rust CLI, FFmpeg rendering, or interchange export.

Limits: 16 video tracks, 32 audio tracks, 1024 video clips, 1024 audio clips,
256 volume keyframes per clip, 256 source settings, 24 hours of output, even
dimensions from 2 to 8192 pixels, 1–240 FPS, speed 0.125–8, and linear gain 0–4.
Missing files, absent media streams, overlaps within a track, and source ranges
exceeding the known duration are errors.

## Optional captions and chapters

Add `captions` and `chapters` when needed; both use the **output timeline**,
after cuts and retiming.

The simplest caption track is generated from word transcripts of the source
media and follows the edit automatically:

```json
"captions": [{"from_transcripts": ["interview.transcript.json"], "language": "eng"}]
```

Each transcript must include `usage.source_sha256`: 64 hex digits
(case-insensitive) matching its source media content SHA-256; byte-identical
copies are accepted. Missing, invalid or mismatched fingerprints fail before
caption generation. Re-run `skywrite transcribe` on the media, or point the
transcript/edit at the media it was made from. Media hashes use the shared
[cache](cache-models.md).

Each word is captioned where its midpoint is audible in a clip of its source
(muted, cut and faded-to-silence words are skipped), at its output-timeline time;
cues break at cuts, and the same words heard from two synced sources are
captioned once. Write the track for an NLE with
`skywrite edit captions edit.json -o captions.srt`. Premiere exports write it
beside the XML automatically (`NAME.eng.srt`); FCPXML exports embed native
captions. SRT is the recommended deliverable because it stays editable as text.

A track can instead name an existing `source` file already on the output
timeline:

```json
{
  "version": 1,
  "duration": 10,
  "captions": [{"source": "captions.srt", "language": "eng"}],
  "chapters": [{"start": 0, "title": "Opening"}, {"start": 5, "title": "Next"}]
}
```

Caption paths resolve relative to the edit and accept UTF-8 `.srt`, plain `.vtt`,
or existing Skywrite transcript `.json` with real word timestamps. A transcript
`.json` `source` is placed by the explicit mapping below and is not checked
against media (like `skywrite captions convert`); `from_transcripts` maps words
through the clips and requires the fingerprint check above. No ASR runs.
The track mapping is `output_time = offset + scale * caption_time`; `offset`
defaults to 0 and `scale` to 1. This is explicit mapping, not automatic caption
reassembly across source cuts. Prepare separate output-timeline cues when cutting
spoken source material. `language` is a three-letter lowercase ISO-639 code,
default `und`. `mode` defaults to `soft`, producing optional MP4 `mov_text`
subtitle tracks. Set `mode` to `burn` explicitly to render text into pixels;
this requires FFmpeg's `subtitles` filter with libass (`skywrite doctor` reports
`burned_captions.subtitles_filter_available`). Caption text is plain text;
markup is escaped and rendered literally through an internal WebVTT sidecar.
Braces and backslashes in caption text are rejected before export/rendering,
because FFmpeg cannot preserve them identically across soft tracks and burn-in.
Unicode, quotes, and ordinary newlines remain supported; chapter titles do not
have this subtitle-specific restriction. Plain SRT/VTT excludes styling, cue settings, and WebVTT regions.

Chapter starts must increase strictly and fall inside the output duration; each
chapter ends at the next start or the output end. Caption cues need ordered,
positive timestamps fitting the timeline. Times must remain distinct at
millisecond precision. Limits: 8 tracks, 100,000 total cues, 4 MiB text per track,
4096 bytes per cue, 8 MiB per caption input, 1000 chapters, 512 bytes per chapter
title; chapter/caption text excludes control characters. Remapping scales range
from 0.125 to 8; offsets are limited to ±24 hours. Original edit, caption,
transcript, and source media inputs are protected even with `--overwrite`.

FFmpeg native rendering and generated Python scripts apply captions and chapters.
FCPXML embeds caption tracks as native `<caption>` elements (SRT role) and
emits chapter markers; Final Cut/Resolve import of the captions has not been
verified. Premiere XML cannot carry captions: each track is written beside the
XML as SRT, to import together with it. Hyperframes and Remotion preserve SRT/VTT, chapter metadata,
and JSON sidecars; their generated compositions do not apply them automatically.
The FFmetadata sidecar contains chapter boundaries; JSON preserves exact titles,
including trailing backslashes that FFmetadata cannot round-trip reliably.

## Pauses

`pauses` marks deliberate silences (a beat, a laugh, a reveal) on the **output
timeline**, in seconds, so `skywrite review lint` does not report them as
`dead_air`:

```json
"pauses": [{"start": 41.2, "end": 43, "note": "laugh"}]
```

Each pause needs `0 <= start < end <=` the timeline duration; pauses are ordered
by `start` and must not overlap (one may start where the previous ends). `note` is
optional, at most 512 bytes, without control characters or U+FFFE/U+FFFF; at
most 1000 pauses.
Lint passes a gap between heard words when the pauses cover all of it except at
most `--dead-air-seconds`, and lists it in `checked.dead_air.excluded_by_pause`
with the pauses' notes. Renderers (MP4, Python, Remotion, Hyperframes) ignore
pauses. FCPXML writes each as a `<marker value="Pause">` with its duration and
`note`; Premiere XML as a sequence marker named `Pause` with in and out points
rounded to whole frames, kept inside the sequence and apart (a pause that rounds to
no frame is left out, with a warning), and the note as its comment. Premiere XML
import reads ranged `Pause` markers back as pauses, leaving out (with a note) any
that overlap another or run past the sequence; comment line breaks and other
control characters become spaces and comments are cut to 512 bytes (noted).
After rendering either optional backend, run:

```sh
skywrite captions mux rendered.mp4 --edit edit.json --output finished.mp4
```

This finishes a video matching the edit duration, copies its audio/video when
possible, adds soft tracks/chapters, and re-encodes video for explicit burn-in.
The original renderer output remains an input and cannot be replaced in place.
