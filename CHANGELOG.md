# Changelog

All notable changes to Skywrite are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 1.0.0 - Unreleased

### Added

- First public release of Skywrite, licensed under MIT, copyright 2026 FlyCockpit LLC.
- JSON-first media inspection, frame sampling and optional OpenRouter analysis and transcription with word timestamps.
- Multi-recording synchronization by audio waveform or transcript, with drift estimates and quality reporting; `strong` needs method-level residuals (`strong_p95_limit_seconds`: 5 ms for waveform, 50 ms for transcripts), and mappings are reported without float noise (an exact match is `scale` 1.0).
- `skywrite sync apply` places a sync result into an edit (frame-grid start, sub-frame `source_in`, drift `speed`, duration trimmed to the real overlap, reference sound muted), refuses results that are not `strong` with drift estimated unless `--allow-limited`, reports `uncovered` timeline, and with `--verify` renders and re-syncs each placed clip. Audio sync measures pre-emphasized waveform anchors every 2 s across the overlap (at most 1800) with sub-sample peak interpolation, and reports takes that one mapping cannot fit (a cut, dropout or recorder restart) as `discontinuous` with located `segments` (each stopping 0.3 s short of a boundary); sound inside a matched stretch that fits no mapping makes the result `limited`; anchors carry their `segment`; `candidate_matches` counts waveform probe windows.
- Edit JSON format version 1, with validation, lint and review tools and a native FFmpeg MP4 renderer. Clip audio is sample-exact at speed 1 and at clock-drift speeds within 0.1% of 1 (resampled); larger speed changes keep pitch with `--stretcher auto|rubberband|atempo` (default `auto`: FFmpeg `rubberband`, latency-compensated by a per-speed calibration to within about 6 ms (8 ms at 0.125x), falling back to `atempo` with a warning when FFmpeg lacks it). The same applies to `audio tempo` and exported FFmpeg scripts.
- Required transcript content fingerprints for transcript-based caption tracks, `describe frames --transcript` and `review lint --transcript`, matching analysis and edit suggestions; byte-identical media copies are accepted, and media SHA-256 hashes are cached locally.
- Export to FCPXML, Premiere XML (xmeml), Hyperframes, Remotion and FFmpeg scripts; import Premiere/FCP 7 XML and experimental `.prproj` files.
- `edit export --media-root FOLDER` writes FCPXML and Premiere XML media URLs beneath the editor's folder, so exports made on another machine (for example in the cloud) link on open.
- Native HTML or JSON still graphics through embedded Takumi, with PNG, JPEG and WebP output.
- Optional image, video, narration and music generation and upscaling through OpenRouter, with durable generation projects and optional S3/R2 uploads.
- Agent skills, saved style prompts, task model defaults and `skywrite nit` feedback capture.
- `skywrite doctor` reports whether this FFmpeg has the `rubberband` filter (`audio_stretcher`) and the libass `subtitles` filter that burned-in captions need (`burned_captions`). When `subtitles` is missing it adds a `fix` hint: `ffmpeg-full` on macOS, or a distribution FFmpeg built with libass on Linux.
- Release binaries for Linux (static musl: x86_64, aarch64; no glibc requirement) and macOS (Apple silicon, Intel) on GitHub Releases, with SHA256SUMS, third-party license notices and signed GitHub build provenance attestations for every asset (`gh attestation verify <file> --repo FlyCockpit/skywrite`); archives include the embedded Geist font license as `fonts-OFL.txt`, and Linux archives include the statically linked musl libc's notice as `musl-COPYRIGHT.txt`.
- `external_sources` in every render and export result lists sources outside the edit file's directory; symlinked and special-file sources are refused, in edits and in imported XML. Edit and XML files are documented as trusted input.
