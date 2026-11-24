# FFmpeg assembly and verification

1. Run `skywrite doctor` and `skywrite inspect INPUT`. Burned-in captions (`mode: "burn"`) need `burned_captions.subtitles_filter_available`; if it is false, follow its `fix` hint or keep captions `soft`.
2. Write an edit following `skywrite schema`, with local sources relative to the edit JSON. Source `duration` is output time; `speed` is source seconds per output second.
3. Run `skywrite edit validate edit.json` and review all reported rounding and bounds.
4. Render directly: `skywrite edit render edit.json --output edit.mp4`.
5. For a portable script, use `skywrite edit export edit.json --to ffmpeg --output render.py`, then `python3 render.py --output edit.mp4`.
6. Run `skywrite review lint edit.json --no-levels` (add `--transcript` per dialogue source for speaker and word checks) and fix any `brief_shot` flash frames it reports (raise `--min-shot-seconds` to match the pacing). It checks only shots between hard cuts; fades and dissolves are not checked yet, so watch the spans in `checked.brief_shots_not_checked`. Inspect the rendered MP4 and watch/listen across cuts, layers, fades, delays, empty timeline, held images, retiming, gain keyframes and audio tracks. Rotated phone footage (`rotation` in `skywrite inspect`) renders, describes and sheets upright by its displayed size, and `skywrite verify FILE --expected-width W --expected-height H` checks that displayed size. Confirm duration and streams against the approved edit.

Renders, exported scripts and `skywrite audio` loops give the same picture and sound on FFmpeg 5.1 or newer, including 7.0 and 7.1 (black lead-ins before late-starting video, crossfaded loops of short cues); docs/validation.md has the measured versions. Use explicit `--overwrite` only for an intended output replacement. The generated script needs Python's standard library and installed FFmpeg. Unsupported effects must be authored in a composition or NLE instead of being silently omitted.
