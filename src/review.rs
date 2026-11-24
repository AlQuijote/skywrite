//! Review aids: exact-frame contact sheets, cut audition clips, and objective
//! edit checks. These report evidence for a human or agent to judge; they never
//! change an edit and never encode artistic preferences.
use crate::{
    export, media, process,
    timeline::{self, MediaKind, ValidatedEdit},
    transcript::{Transcript, Word},
};
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_SHEET_FRAMES: usize = 48;
const MAX_CUTS: usize = 200;
const MAX_MEASUREMENTS: usize = 256;

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// Contact sheet of consecutive exact frames from a source range, labelled with source time.
    Sheet(SheetArgs),
    /// Short audition clips around every cut in an edit, for listening and watching.
    Cuts(CutsArgs),
    /// Objective checks of an edit before rendering, including brief shots between hard cuts; pass --transcript WORDS.json for each dialogue source, or the word checks (unfinished endings, dead air, repeated or clipped words, silent visible speakers) do not run.
    #[command(long_about = LINT_ABOUT, after_long_help = LINT_EXAMPLES)]
    Lint(LintArgs),
}

#[derive(Args)]
pub struct SheetArgs {
    pub source: PathBuf,
    /// Source seconds from the first presented sample.
    #[arg(long, default_value_t = 0.0)]
    pub start: f64,
    #[arg(long)]
    pub duration: f64,
    /// Take every Nth decoded frame (2 = every other frame).
    #[arg(long, conflicts_with = "interval")]
    pub every: Option<u32>,
    /// Take one frame per this many seconds instead of a frame count.
    #[arg(long)]
    pub interval: Option<f64>,
    #[arg(long, default_value_t = 4)]
    pub columns: u32,
    /// Width of each tile in pixels.
    ///
    /// Default 320. Tile height follows the source's displayed aspect ratio
    /// (rotation applied), so rotated phone footage gets upright portrait tiles.
    #[arg(long, default_value_t = 320, value_name = "PIXELS")]
    pub tile_width: u32,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long)]
    pub overwrite: bool,
}

#[derive(Args)]
pub struct CutsArgs {
    pub edit: PathBuf,
    /// Seconds of context before and after each cut.
    #[arg(long, default_value_t = 1.5)]
    pub handles: f64,
    /// A new directory for the audition clips.
    #[arg(long)]
    pub output_dir: PathBuf,
    /// Reuse an existing render of this edit instead of rendering it again.
    #[arg(long)]
    pub render: Option<PathBuf>,
}

#[derive(Args)]
pub struct LintArgs {
    pub edit: PathBuf,
    /// Word-timed transcript of a dialogue source; repeat for each source.
    #[arg(long)]
    pub transcript: Vec<PathBuf>,
    /// Skip audio level measurements (faster; disables level checks).
    #[arg(long)]
    pub no_levels: bool,
    /// Longest silence between heard dialogue words, in timeline seconds, before a dead_air warning (0.1 to 10).
    #[arg(long, default_value_t = 0.6, value_name = "SECONDS")]
    pub dead_air_seconds: f64,
    /// Report shots on screen for less than this many seconds (flash frames).
    ///
    /// Seconds of output time, 0 to 10 (default 0.5); 0 turns the brief-shot
    /// check off. Only shots between two hard cuts are checked (fades and
    /// dissolves are not checked yet); a brief_shot warning reports its frames,
    /// timeline_start and timeline_end. Raise it to match the edit's pacing
    /// (for example 1). Values outside 0..10 are refused.
    #[arg(long, default_value_t = 0.5, value_name = "SECONDS")]
    pub min_shot_seconds: f64,
}

const LINT_ABOUT: &str = "Objective checks of an edit before you render it, from the edit JSON and word-timed transcripts. It never changes the edit and never judges taste.

Run it on every draft, before skywrite edit render; after rendering, check the file with skywrite verify. Supply one --transcript per dialogue source (from skywrite transcribe; usage.source_sha256 must match the media). Without transcripts only the level and brief-shot checks run. The report's not_checked list (also printed to stderr) says which checks were skipped, which dialogue sources had no transcript and which short shots brief_shot did not check because of a fade or dissolve.

Warnings (kind):
  visible_speaker_without_dialogue  a speaker's picture is visible while their words are inaudible
  repeated_word_across_cut / repeated_word_in_select  the same word twice in a row
  word_clipped_at_cut               a cut inside a transcribed word
  filler_words                      um, uh and similar inside audible dialogue
  unfinished_ending                 the edit's last heard dialogue stops mid-thought: reasons no_sentence_final_punctuation (the last kept word has no . ! ? or similar; only judged when the transcript has punctuation), next_word_continues (the next source word, within 2 s, is a continuation such as and, but, so, because, which; also common German, French, Spanish, Italian, Portuguese and Dutch ones), open_list (a counted list such as \"three reasons\" or first/second stops before its last item). It reports the last heard words, the cut-off source words and their times.
  dead_air                          silences between heard dialogue words longer than --dead-air-seconds (default 0.6 s), measured on the output timeline after cuts, speed changes, J/L cuts and muted B-roll sound. Each item has timeline_start, timeline_end, seconds, the words either side, across_cut and other_sound (music or effects audible in the gap, which does not hide a pause). Silence before the first and after the last word is not counted. A deliberate pause is marked in the edit itself: a top-level \"pauses\": [{\"start\": 41.2, \"end\": 43, \"note\": \"laugh\"}] (timeline seconds) passes any gap it covers except for at most --dead-air-seconds; those gaps are listed in checked.dead_air.excluded_by_pause.
  sound_cut_mid_flow                a clip ends while its source keeps sounding (applause, music tails)
  bed_close_to_dialogue             music or ambience within 10 dB of the dialogue
  brief_shot                        a picture on screen for less than --min-shot-seconds (default 0.5 s) between two hard cuts: a flash frame, a short cutaway or a black gap. Fades and dissolves are not checked yet.

Brief shots (brief_shot) are checked only between hard cuts; fades and dissolves are not checked yet. They are judged from clip placement and opacity, not pixels. Each frame's picture is the visible weight of each clip: its opacity, reduced by full-frame opaque video above it, with black taking the rest. Two pictures differ by half the sum of their weight differences (1 for a cut between full-frame shots, 0.25 when a 25%-opacity full-frame overlay appears), and a cut is a frame where they differ by at least 0.25. A shot runs from one cut to the next (black lies beyond the timeline's ends), and its length is exact from the edit. It is reported when it is shorter than --min-shot-seconds and both its cuts are hard: no visible clip fades in the shot or in the frame on either side of it, and no take continues at another opacity at its cuts or inside it. That catches a flash frame, a short cutaway, a short overlay at partial opacity, or a black gap cut in and out. A shot with a fade, dissolve or opacity change at either cut is never reported; when its clip edges are under the threshold it is listed in checked.brief_shots_not_checked (timeline_start, timeline_end) to watch with skywrite review cuts. Not cuts: clips at opacity 0, a straight split continuing the same take (same source and track, contiguous within one frame of the slower clip's source step), and cuts under full-frame opaque video on a higher track. Stills, letterboxed pictures and alpha-capable video (ProRes 4444, QTRLE argb) never hide or dim what is beneath them, so a cut under them counts fully, but a partial-opacity one of them must be about twice as opaque to count (0.5 rather than 0.25). Video whose pixel format FFmpeg does not describe never counts as cover either and is listed in checked.uncertain_cover_sources. Each brief_shot warning has frames, timeline_start and timeline_end; checked.min_shot_seconds is the threshold used.

Transcript checks are only as good as the transcript: missing words (laughter, crosstalk) read as silence, and word times are alignment estimates (Parakeet, for example, uses 80 ms steps). The JSON's checked object says what was and was not measured. Cost: no network; only the level checks decode audio (at most 256 short windows; --no-levels skips them). Exit status is 0 with warnings; it fails only on unreadable inputs or mismatched transcripts.";

const LINT_EXAMPLES: &str = "Examples:
  skywrite review lint edit.json --transcript talk.transcript.json
  skywrite review lint edit.json --transcript host.json --transcript guest.json --no-levels --dead-air-seconds 0.8
  skywrite review lint edit.json --no-levels --min-shot-seconds 1
      Shots under 1 s between hard cuts are reported as brief (default 0.5 s).
  skywrite review lint edit.json --min-shot-seconds 0
      Turns the brief-shot check off.";

pub fn execute(command: ReviewCommand) -> Result<Value> {
    match command {
        ReviewCommand::Sheet(args) => sheet(&args),
        ReviewCommand::Cuts(args) => cuts(&args),
        ReviewCommand::Lint(args) => lint(&args),
    }
}

// ---------------------------------------------------------------- sheets

pub struct Extracted {
    pub path: PathBuf,
    /// Presentation time in source seconds from the first presented sample.
    pub time: f64,
}

/// Which decoded frames to keep from a source range.
pub struct FrameSelection {
    /// Source seconds from the first presented sample.
    pub start: f64,
    pub duration: f64,
    /// Keep every Nth decoded frame.
    pub every: Option<u32>,
    /// Keep the first decoded frame in each source-aligned interval bucket.
    pub interval: Option<f64>,
    /// Output width in pixels; height keeps the aspect ratio.
    pub width: u32,
}

/// Decode a range and keep exact frames, recording each frame's presentation time.
pub fn extract_frames(
    info: &media::MediaInfo,
    selection: &FrameSelection,
    dir: &Path,
    limit: usize,
) -> Result<Vec<Extracted>> {
    let FrameSelection {
        start,
        duration,
        every,
        interval,
        width,
    } = *selection;
    ensure!(info.has_video(), "source has no video stream");
    ensure!(
        start.is_finite() && start >= 0.0 && duration.is_finite() && duration > 0.0,
        "start must be nonnegative and duration positive"
    );
    ensure!(
        start + duration <= info.duration + 0.05,
        "range ends after the source ({:.3}s)",
        info.duration
    );
    // Select on absolute source frame numbers / time buckets (not counted from
    // the range start) so overlapping passes land on the same frames and share
    // cached results. With -copyts, `t` is the source presentation time.
    let video = info.video().context("source has no video stream")?;
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .filter(|r| r.is_finite() && *r > 0.0);
    let origin = video.start_time.unwrap_or(info.start_time);
    let index = |fps: f64| format!("round((t-{origin:.9})*{fps:.9})");
    let mut seek = start;
    let select = match (every, interval, fps) {
        (Some(n), None, Some(fps)) => {
            ensure!((1..=10_000).contains(&n), "--every must be 1..10000");
            format!("select='eq(mod({}\\,{n})\\,0)'", index(fps))
        }
        (Some(n), None, None) => {
            ensure!((1..=10_000).contains(&n), "--every must be 1..10000");
            format!("select='not(mod(n\\,{n}))'")
        }
        (None, Some(seconds), _) => {
            ensure!(
                seconds.is_finite() && seconds > 0.0,
                "--interval must be positive"
            );
            // Decode from the containing bucket's boundary, then discard any
            // selected frame before the requested start. Otherwise prev_t=NAN
            // after a mid-bucket seek would select a different cached frame.
            let bucket = ((info.start_time + start - origin + 0.000001) / seconds).floor();
            // Include the same boundary tolerance as the selector, including
            // when FFmpeg rounds the seek to microseconds.
            seek = (origin + bucket * seconds - info.start_time - 0.000001).clamp(0.0, start);
            let bucket_of =
                |time: &str| format!("floor(({time}-{origin:.9}+0.000001)/{seconds:.9})");
            // Compare consecutive decoded timestamps so gaps and dense VFR
            // bursts still yield exactly one frame per occupied bucket.
            format!(
                "select='(isnan(prev_t)+gt({}\\,{}))*gte(t\\,{:.9})'",
                bucket_of("t"),
                bucket_of("prev_t"),
                info.start_time + start,
            )
        }
        (None, None, _) => "select=1".into(),
        _ => bail!("choose --every or --interval"),
    };
    // Input -ss is relative to the container start even with -copyts; -copyts
    // keeps filter `t` and printed timestamps absolute.
    let mut args = process::args(&[
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "info",
        "-n",
        "-copyts",
        "-protocol_whitelist",
        "file,pipe",
        "-ss",
        &format!("{seek:.9}"),
        "-t",
        &format!("{:.9}", start + duration - seek),
        "-i",
    ]);
    args.push(info.path.clone().into_os_string());
    args.extend(process::args(&[
        "-map",
        "0:v:0",
        "-vf",
        &format!("{select},showinfo,scale={width}:-2"),
        "-fps_mode",
        "passthrough",
        "-frames:v",
        &(limit + 1).to_string(),
        "-q:v",
        "3",
    ]));
    args.push(dir.join("f%04d.jpg").into_os_string());
    let out = process::run("ffmpeg", &args, Duration::from_secs(120), 16 * 1024 * 1024)?;
    let log = String::from_utf8_lossy(&out.stderr);
    // Exact times from the integer pts and time base; printed pts_time can be
    // rounded to a few significant digits by some FFmpeg versions.
    let time_base = log
        .lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .find_map(|l| l.split("time_base:").nth(1))
        .and_then(|rest| rest.split([',', ' ']).find(|t| !t.is_empty()))
        .and_then(|value| {
            let (n, d) = value.split_once('/')?;
            let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
            (n > 0.0 && d > 0.0).then_some(n / d)
        });
    let mut times = Vec::new();
    for line in log
        .lines()
        .filter(|l| l.contains("Parsed_showinfo") && l.contains("pts_time:"))
    {
        let exact = time_base.and_then(|base| {
            let pts = line.split(" pts:").nth(1)?.split_whitespace().next()?;
            Some(pts.parse::<i64>().ok()? as f64 * base)
        });
        let printed = line
            .split("pts_time:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok());
        if let Some(time) = exact.or(printed) {
            times.push(((time - info.start_time) * 1e9).round() / 1e9);
        }
    }
    let mut frames = Vec::new();
    for (index, time) in times.into_iter().enumerate() {
        let path = dir.join(format!("f{:04}.jpg", index + 1));
        if path.is_file() {
            frames.push(Extracted { path, time });
        }
    }
    ensure!(
        frames.len() <= limit,
        "the range yields more than {limit} frames; narrow --duration or increase --every/--interval"
    );
    ensure!(!frames.is_empty(), "no frames were decoded in that range");
    Ok(frames)
}

fn sheet(args: &SheetArgs) -> Result<Value> {
    ensure!((1..=12).contains(&args.columns), "--columns must be 1..12");
    ensure!(
        (64..=1280).contains(&args.tile_width),
        "--tile-width must be 64..1280"
    );
    let info = media::probe_for_timing(&args.source)?;
    io_guard(
        &args.output,
        std::slice::from_ref(&info.path),
        args.overwrite,
    )?;
    let temp = tempfile::Builder::new()
        .prefix("skywrite-sheet-")
        .tempdir()?;
    let frames = extract_frames(
        &info,
        &FrameSelection {
            start: args.start,
            duration: args.duration,
            every: args.every,
            interval: args.interval,
            width: args.tile_width,
        },
        temp.path(),
        MAX_SHEET_FRAMES,
    )?;
    let video = info.video().context("source has no video stream")?;
    let (w, h) = video.display_size().unwrap_or((16, 9));
    let (w, h) = (w as f64, h as f64);
    let tile_height = ((args.tile_width as f64 * h / w).round() as u32).max(16);
    let label = 28u32;
    let columns = args.columns.min(frames.len() as u32);
    let rows = (frames.len() as u32).div_ceil(columns);
    let width = columns * args.tile_width;
    let height = rows * (tile_height + label);
    ensure!(
        u64::from(width) * u64::from(height) <= 16 * 1024 * 1024,
        "contact sheet would exceed 16 megapixels; use fewer frames or smaller tiles"
    );
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .filter(|r| *r > 0.0);
    let video_start = video.start_time.unwrap_or(info.start_time) - info.start_time;
    let mut html = format!(
        "<div style=\"display:flex;flex-wrap:wrap;width:{width}px;height:{height}px;background-color:#111111\">"
    );
    let mut tiles = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let frame_number = fps.map(|fps| ((frame.time - video_start) * fps).round() as i64);
        let caption = match frame_number {
            Some(n) => format!("#{index}  {:.3}s  f{n}", frame.time),
            None => format!("#{index}  {:.3}s", frame.time),
        };
        write!(
            html,
            "<div style=\"display:flex;flex-direction:column;width:{tw}px;height:{th}px\"><img src=\"{name}\" style=\"width:{tw}px;height:{ih}px;object-fit:contain\"/><div style=\"height:{label}px;color:#ffffff;font-size:16px;padding:4px 6px\">{caption}</div></div>",
            tw = args.tile_width,
            th = tile_height + label,
            ih = tile_height,
            name = frame
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .context("frame name")?,
        )?;
        tiles.push(json!({"index":index,"source_time":frame.time,"frame":frame_number}));
    }
    html.push_str("</div>");
    let template = temp.path().join("sheet.html");
    std::fs::write(&template, html)?;
    let format = match args.output.extension().and_then(|e| e.to_str()) {
        Some("png") => "png",
        Some("webp") => "webp",
        _ => "jpeg",
    };
    crate::image::render(&crate::image::RenderImage {
        file: &template,
        output: &args.output,
        width,
        height,
        format: Some(format),
        quality: (format != "png").then_some(85),
        fonts: &[],
        google_fonts: &[],
        cache_dir: None,
        offline: true,
        overwrite: args.overwrite,
    })?;
    Ok(json!({
        "output": args.output.canonicalize()?,
        "source": info.path,
        "columns": columns,
        "rows": rows,
        "tile_width": args.tile_width,
        "tiles": tiles,
        "frame_numbers": if fps.is_some() {"estimated from the average frame rate; exact for constant-frame-rate sources"} else {"unknown frame rate"},
        "note": "Tiles are consecutive decoded frames with their presentation times in source seconds (from the first presented sample), read left to right. Use them to judge expressions, motion and timing around a candidate cut; describe frames with your own image tools.",
    }))
}

fn parse_rate(value: &str) -> Option<f64> {
    let (n, d) = value.split_once('/')?;
    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
    (d != 0.0).then_some(n / d)
}

fn io_guard(output: &Path, inputs: &[PathBuf], overwrite: bool) -> Result<()> {
    crate::io::protect_inputs(output, inputs)?;
    ensure!(
        overwrite || !output.exists(),
        "output already exists; pass --overwrite"
    );
    Ok(())
}

// ---------------------------------------------------------------- cuts

/// Every timeline position where a clip starts or ends, excluding the ends.
fn cut_points(edit: &ValidatedEdit) -> Vec<(u64, Vec<String>)> {
    let mut cuts = std::collections::BTreeMap::<u64, Vec<String>>::new();
    let total = edit.duration_frames();
    let mut add = |frame: u64, what: String| {
        if frame > 0 && frame < total {
            cuts.entry(frame).or_default().push(what);
        }
    };
    for clip in edit.video_clips() {
        let name = file_name(&clip.source);
        add(
            clip.start_frames,
            format!("V{} in: {name} @ {:.3}s", clip.track + 1, clip.source_in),
        );
        add(
            clip.end_frames(),
            format!(
                "V{} out: {name} @ {:.3}s",
                clip.track + 1,
                clip.source_in + edit.fps().seconds(clip.duration_frames) * clip.speed
            ),
        );
    }
    for clip in edit.audio_clips() {
        let name = file_name(&clip.source);
        add(
            clip.start_frames,
            format!("A{} in: {name} @ {:.3}s", clip.track + 1, clip.source_in),
        );
        add(
            clip.end_frames(),
            format!(
                "A{} out: {name} @ {:.3}s",
                clip.track + 1,
                clip.source_in + edit.fps().seconds(clip.duration_frames) * clip.speed
            ),
        );
    }
    cuts.into_iter().collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("media")
        .to_owned()
}

fn cuts(args: &CutsArgs) -> Result<Value> {
    ensure!(
        args.handles.is_finite() && (0.2..=10.0).contains(&args.handles),
        "--handles must be 0.2..10 seconds"
    );
    let edit = timeline::load_and_validate(&args.edit)?;
    let points = cut_points(&edit);
    ensure!(!points.is_empty(), "the edit has no cuts to audition");
    ensure!(
        points.len() <= MAX_CUTS,
        "the edit has {} cuts; audition at most {MAX_CUTS} at a time",
        points.len()
    );
    let parent = args
        .output_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(parent.is_dir(), "output parent directory does not exist");
    std::fs::create_dir(&args.output_dir)
        .context("--output-dir must be a new directory (existing directories are never reused)")?;
    let dir = args.output_dir.canonicalize()?;
    let mut audio_stretcher = Value::Null;
    let (render, _temporary) = match &args.render {
        Some(path) => {
            let info = media::probe(path)?;
            ensure!(
                info.has_video()
                    && (info.duration - edit.duration()).abs() < 0.1 + edit.fps().seconds(1),
                "--render must be a render of this edit ({:.3}s)",
                edit.duration()
            );
            (info.path, None)
        }
        None => {
            let temp = tempfile::Builder::new()
                .prefix(".skywrite-review-")
                .suffix(".mp4")
                .tempfile_in(&dir)?;
            let cleanup = process::CleanupGuard::new(temp.path())?;
            let path = temp.path().to_owned();
            let report = export::render(&edit, &path, true, crate::stretch::StretcherChoice::Auto)?;
            audio_stretcher = report["audio_stretcher"].clone();
            (path, Some((temp, cleanup)))
        }
    };
    let fps = edit.fps();
    let mut clips = Vec::new();
    for (index, (frame, events)) in points.iter().enumerate() {
        let at = fps.seconds(*frame);
        let from = (at - args.handles).max(0.0);
        let to = (at + args.handles).min(edit.duration());
        let output = dir.join(format!("cut-{index:03}-{at:09.3}s.mp4"));
        let mut command = process::args(&[
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-n",
            "-protocol_whitelist",
            "file,pipe",
            "-ss",
            &format!("{from:.6}"),
            "-t",
            &format!("{:.6}", to - from),
            "-i",
        ]);
        command.push(render.clone().into_os_string());
        command.extend(process::args(&[
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-movflags",
            "+faststart",
        ]));
        command.push(output.clone().into_os_string());
        process::run("ffmpeg", &command, Duration::from_secs(120), 1024 * 1024)?;
        clips.push(json!({"index":index,"cut_seconds":at,"cut_frame":frame,"clip":output,"clip_start_seconds":from,"cut_offset_in_clip":at-from,"events":events}));
    }
    Ok(json!({
        "output_dir": dir,
        "cut_count": clips.len(),
        "cuts": clips,
        "audio_stretcher": audio_stretcher,
        "note": "Each clip plays the rendered edit around one cut. Watch and listen for clipped words, stumbles, repeated phrases, sound cut off mid-flow (applause, music tails), jump cuts and sync. Skywrite cannot judge delivery or emotional fit; that review is yours.",
    }))
}

// ---------------------------------------------------------------- lint

#[derive(Clone)]
struct Interval {
    start: f64,
    end: f64,
}

fn subtract(base: Interval, covers: &[Interval]) -> Vec<Interval> {
    let mut pieces = vec![base];
    for cover in covers {
        let mut next = Vec::new();
        for piece in pieces {
            if cover.end <= piece.start || cover.start >= piece.end {
                next.push(piece);
                continue;
            }
            if cover.start > piece.start {
                next.push(Interval {
                    start: piece.start,
                    end: cover.start,
                });
            }
            if cover.end < piece.end {
                next.push(Interval {
                    start: cover.end,
                    end: piece.end,
                });
            }
        }
        pieces = next;
    }
    pieces
}

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

const FILLERS: &[&str] = &["um", "uh", "umm", "uhh", "erm", "er", "ah", "hmm", "mm"];

struct TimelineWord<'a> {
    word: &'a Word,
    start: f64,
    end: f64,
    span: usize,
}

/// Mean square of a mono 16 kHz decode of a source range, in dBFS.
fn level(path: &Path, start: f64, duration: f64) -> Result<Option<f64>> {
    if duration <= 0.01 {
        return Ok(None);
    }
    let mut args = process::args(&[
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-ss",
        &format!("{start:.6}"),
        "-t",
        &format!("{duration:.6}"),
        "-i",
    ]);
    args.push(path.as_os_str().to_owned());
    args.extend(process::args(&[
        "-map", "0:a:0", "-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:1",
    ]));
    let out = process::run("ffmpeg", &args, Duration::from_secs(60), 8 * 1024 * 1024)?;
    let samples: Vec<f32> = out
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if samples.len() < 160 {
        return Ok(None);
    }
    let mean = samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64;
    Ok(Some(10.0 * mean.max(1e-12).log10()))
}

fn lint(args: &LintArgs) -> Result<Value> {
    ensure!(
        args.min_shot_seconds.is_finite() && (0.0..=10.0).contains(&args.min_shot_seconds),
        "--min-shot-seconds must be 0..10 seconds"
    );
    ensure!(
        args.dead_air_seconds.is_finite() && (0.1..=10.0).contains(&args.dead_air_seconds),
        "--dead-air-seconds must be 0.1..10"
    );
    let edit = timeline::load_and_validate(&args.edit)?;
    let mut transcripts = HashMap::<PathBuf, Transcript>::new();
    for path in &args.transcript {
        let transcript = Transcript::load(path)?;
        let base = path.parent().unwrap_or(Path::new("."));
        let source = if transcript.source.is_absolute() {
            transcript.source.clone()
        } else {
            base.join(&transcript.source)
        };
        let source = media::canonical_file(&source)
            .with_context(|| format!("transcript {} source", path.display()))?;
        transcript.bind_media(path, &source, None)?;
        if transcripts.insert(source.clone(), transcript).is_some() {
            bail!("two transcripts were supplied for {}", source.display());
        }
    }
    let mut warnings = Vec::new();
    let fps = edit.fps();
    let spans = edit.audible_spans();

    // Map dialogue words into timeline time for every audible dialogue span.
    let mut words = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let Some(transcript) = transcripts.get(span.source) else {
            continue;
        };
        let source_end = span.source_in + span.duration * span.speed;
        for word in &transcript.words {
            if word.end <= span.source_in || word.start >= source_end {
                continue;
            }
            let start = span.start + (word.start.max(span.source_in) - span.source_in) / span.speed;
            let end = span.start + (word.end.min(source_end) - span.source_in) / span.speed;
            let mid = ((start + end) / 2.0 - span.start).clamp(0.0, span.duration);
            if span.mix.gain(mid, span.duration) > 0.0 {
                words.push(TimelineWord {
                    word,
                    start,
                    end,
                    span: index,
                });
            }
        }
    }
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    // A word split by a straight cut appears once per span; merge the halves so
    // it is not reported as a repetition.
    let frame = fps.seconds(1);
    let mut merged: Vec<TimelineWord> = Vec::with_capacity(words.len());
    for word in words {
        if let Some(last) = merged.last_mut()
            && std::ptr::eq(last.word, word.word)
            && (word.start - last.end).abs() <= frame * 1.5
        {
            last.end = last.end.max(word.end);
            continue;
        }
        merged.push(word);
    }
    let words = merged;

    // 1. A visible picture of someone speaking with no dialogue audible.
    // Only full-frame video whose pixel format has no alpha channel covers what
    // lies beneath; letterboxed pictures, stills (often transparent titles) and
    // alpha-capable video (lower thirds in ProRes 4444 or QTRLE) leave it visible.
    let covers_frame = |c: &timeline::ValidatedVideoClip| {
        c.kind == MediaKind::Video
            && c.media.video().is_some_and(|v| {
                v.is_opaque_format()
                    && v.display_size().is_some_and(|(w, h)| {
                        let (w, h) = (w as f64, h as f64);
                        h > 0.0
                            && ((w / h) - edit.width as f64 / edit.height as f64).abs()
                                < 0.01 * edit.width as f64 / edit.height as f64
                    })
            })
    };
    let opaque: Vec<(usize, Interval)> = edit
        .video_clips()
        .filter(|c| c.opacity >= 1.0 && covers_frame(c))
        .map(|c| {
            (
                c.track,
                Interval {
                    start: fps.seconds(c.start_frames) + c.fade_in,
                    end: fps.seconds(c.end_frames()) - c.fade_out,
                },
            )
        })
        .collect();
    for clip in edit.video_clips().filter(|c| c.kind == MediaKind::Video) {
        let Some(transcript) = transcripts.get(&clip.source) else {
            continue;
        };
        let start = fps.seconds(clip.start_frames);
        let duration = fps.seconds(clip.duration_frames);
        let covers: Vec<Interval> = opaque
            .iter()
            .filter(|(track, _)| *track > clip.track)
            .map(|(_, i)| i.clone())
            .collect();
        let visible = subtract(
            Interval {
                start,
                end: start + duration,
            },
            &covers,
        );
        let source_end = clip.source_in + duration * clip.speed;
        let mut silent = Vec::new();
        for word in &transcript.words {
            if word.end <= clip.source_in || word.start >= source_end {
                continue;
            }
            let t0 = start + (word.start.max(clip.source_in) - clip.source_in) / clip.speed;
            let t1 = start + (word.end.min(source_end) - clip.source_in) / clip.speed;
            let mid = (t0 + t1) / 2.0;
            if !visible.iter().any(|v| v.start <= mid && mid < v.end) {
                continue;
            }
            let heard = words.iter().any(|w| w.start < t1 && w.end > t0);
            if !heard {
                silent.push((t0, t1, word.text.as_str()));
            }
        }
        if !silent.is_empty() {
            let text: Vec<&str> = silent.iter().take(12).map(|s| s.2).collect();
            warnings.push(json!({
                "kind": "visible_speaker_without_dialogue",
                "timeline_start": silent[0].0,
                "timeline_end": silent[silent.len() - 1].1,
                "clip": format!("video track {} clip {}", clip.track, clip.index),
                "words": silent.len(),
                "message": format!(
                    "{} is visible speaking ({:?}…) from {:.2}s to {:.2}s, but no dialogue source is audible then. Viewers will see a silent speaker; start the picture at the dialogue, cover it with B-roll, or make its sound audible.",
                    file_name(&clip.source), text.join(" "), silent[0].0, silent[silent.len()-1].1
                ),
            }));
        }
    }

    // 1b. Brief shots: a picture on screen for less than --min-shot-seconds,
    // checked only between hard cuts. Each frame's picture is the visible
    // weight of each take: its opacity times what full-frame opaque cover above
    // it lets through, with black taking the rest. A cut is a frame where at
    // least a quarter of the picture changes. A shot is checked only when no
    // fade, dissolve or opacity step of a visible clip touches it or the frames
    // beside it, so its length is exact from the edit; shots with one are
    // listed in checked.brief_shots_not_checked when their clip edges are
    // under the threshold.
    let mut brief = Vec::new();
    let mut brief_not_checked = Vec::new();
    if args.min_shot_seconds > 0.0 {
        // Frames strictly under the threshold: ceil(threshold * fps) frames
        // or more are long enough.
        let max_frames = (args.min_shot_seconds * fps.value() - 1e-6).ceil().max(1.0) as u64;
        for shot in shots(&edit, &covers_frame) {
            if shot.end - shot.start < max_frames {
                if shot.hard {
                    brief.push(shot);
                } else {
                    brief_not_checked.push(json!({
                        "timeline_start": fps.seconds(shot.start),
                        "timeline_end": fps.seconds(shot.end),
                    }));
                }
            }
        }
    }
    for Shot { start, end, .. } in brief {
        let frames = end - start;
        let (start, end) = (fps.seconds(start), fps.seconds(end));
        warnings.push(json!({
            "kind": "brief_shot",
            "timeline_start": start,
            "timeline_end": end,
            "frames": frames,
            "message": format!(
                "The picture holds for only {frames} frames ({:.2}s) between two hard cuts, from {start:.2}s to {end:.2}s, under {}s; it reads as a flash. Check with skywrite review cuts; extend or remove the shot, or cover it, if unintended.",
                end - start, args.min_shot_seconds
            ),
        }));
    }
    // Video whose pixel format FFmpeg does not describe never counts as cover;
    // say so, because it can add warnings beneath it.
    let mut uncertain_cover: Vec<&Path> = edit
        .video_clips()
        .filter(|c| {
            c.kind == MediaKind::Video
                && c.media.video().is_some_and(|v| {
                    v.pix_fmt
                        .as_deref()
                        .and_then(media::pixel_format_has_alpha)
                        .is_none()
                })
        })
        .map(|c| c.source.as_path())
        .collect();
    uncertain_cover.sort();
    uncertain_cover.dedup();

    // 2. Repeated words, across or inside cuts, and filler words in selects.
    for pair in words.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let same = normalize(&a.word.text);
        if same.is_empty() || same != normalize(&b.word.text) || b.start - a.end > 1.5 {
            continue;
        }
        let across = a.span != b.span;
        warnings.push(json!({
            "kind": if across {"repeated_word_across_cut"} else {"repeated_word_in_select"},
            "timeline_start": a.start,
            "timeline_end": b.end,
            "message": format!(
                "\"{}\" is heard twice in a row at {:.2}s and {:.2}s{}. Listen: this is often a stumble or restart; trim one if unintended.",
                a.word.text.trim(), a.start, b.start,
                if across {" across a cut"} else {" inside one select"}
            ),
        }));
    }
    let fillers: Vec<Value> = words
        .iter()
        .filter(|w| FILLERS.contains(&normalize(&w.word.text).as_str()))
        .take(64)
        .map(|w| json!({"word":w.word.text,"timeline_start":w.start}))
        .collect();
    if !fillers.is_empty() {
        warnings.push(json!({
            "kind": "filler_words",
            "count": fillers.len(),
            "items": fillers,
            "message": "Filler words were transcribed inside audible dialogue. Keep or trim them deliberately. Transcription often omits fillers, so their absence does not prove clean delivery.",
        }));
    }

    // 2b. Words clipped by a cut: a transcribed word straddles a clip's in or
    // out point with no straight continuation and no fade to soften it.
    let slack = frame.max(0.04);
    let mut clipped = Vec::new();
    for span in &spans {
        let Some(transcript) = transcripts.get(span.source) else {
            continue;
        };
        let tolerance = slack * span.speed;
        let source_out = span.source_in + span.duration * span.speed;
        let end = span.start + span.duration;
        let continued_after = spans.iter().any(|o| {
            o.source == span.source
                && (o.source_in - source_out).abs() <= tolerance * 1.01
                && (o.start - end).abs() <= frame * 1.01
        });
        let continued_before = spans.iter().any(|o| {
            o.source == span.source
                && (o.source_in + o.duration * o.speed - span.source_in).abs() <= tolerance * 1.01
                && (o.start + o.duration - span.start).abs() <= frame * 1.01
        });
        for word in &transcript.words {
            let straddles = |at: f64| word.start < at - tolerance && word.end > at + tolerance;
            if !continued_before
                && span.mix.fade_in < 0.1
                && span.mix.gain(0.0, span.duration) > 0.0
                && straddles(span.source_in)
            {
                clipped.push(json!({"edge":"in","word":word.text,"timeline_seconds":span.start,"source_seconds":span.source_in,"clip":span.label}));
            }
            if !continued_after
                && span.mix.fade_out < 0.1
                && span
                    .mix
                    .gain(span.duration, span.duration)
                    .max(span.mix.envelope(span.duration))
                    > 0.0
                && straddles(source_out)
            {
                clipped.push(json!({"edge":"out","word":word.text,"timeline_seconds":end,"source_seconds":source_out,"clip":span.label}));
            }
        }
    }
    if !clipped.is_empty() {
        clipped.sort_by(|a, b| {
            a["timeline_seconds"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b["timeline_seconds"].as_f64().unwrap_or(0.0))
        });
        warnings.push(json!({
            "kind": "word_clipped_at_cut",
            "count": clipped.len(),
            "items": clipped.into_iter().take(64).collect::<Vec<_>>(),
            "message": "A cut falls inside a transcribed word, so part of the word is lost. Listen with skywrite review cuts; move the cut to a pause, extend the clip, or add a short audio fade if it is intended.",
        }));
    }

    // 2c. Where the edit's dialogue stops, and pauses between heard words.
    let ending = unfinished_ending(&spans, &transcripts, fps.seconds(1));
    warnings.extend(ending.warning);
    let dead = dead_air(&spans, &transcripts, args.dead_air_seconds, &edit.pauses);
    warnings.extend(dead.warning);

    // 3 and 4 need decoded audio levels.
    let mut measurements = 0usize;
    let mut levels_checked = false;
    if !args.no_levels {
        levels_checked = true;
        // Within one output frame: frame-rounded durations at NTSC rates leave
        // millisecond differences across a straight split.
        let frame = fps.seconds(1);
        // One exact-duration probe per source, not per span.
        let mut durations: HashMap<&Path, f64> = HashMap::new();
        for span in &spans {
            if measurements + 2 > MAX_MEASUREMENTS {
                break;
            }
            let source_out = span.source_in + span.duration * span.speed;
            let continues = spans.iter().any(|o| {
                o.source == span.source
                    && (o.source_in - source_out).abs() <= frame * span.speed.max(o.speed) * 1.01
                    && (o.start - (span.start + span.duration)).abs() <= frame * 1.01
            });
            let duration = match durations.get(span.source) {
                Some(&duration) => duration,
                None => {
                    let duration = media::probe_for_timing(span.source)?.duration;
                    durations.insert(span.source, duration);
                    duration
                }
            };
            if !continues && span.mix.fade_out == 0.0 && source_out + 0.3 < duration {
                measurements += 2;
                let before = level(span.source, source_out - 0.4 * span.speed, 0.4 * span.speed)?;
                let after = level(span.source, source_out, 0.6 * span.speed)?;
                // Judge what the mix would have played: apply the clip's gain at the cut.
                let gain = span
                    .mix
                    .gain(span.duration, span.duration)
                    .max(span.mix.envelope(span.duration));
                let gain_db = 20.0 * gain.max(1e-6).log10();
                if let (Some(before), Some(after)) =
                    (before.map(|v| v + gain_db), after.map(|v| v + gain_db))
                    && after > -32.0
                    && after > before - 6.0
                {
                    let at = span.start + span.duration;
                    warnings.push(json!({
                        "kind": "sound_cut_mid_flow",
                        "timeline_start": at,
                        "source": span.source,
                        "source_seconds": source_out,
                        "level_before_dbfs": before,
                        "level_after_dbfs": after,
                        "message": format!(
                            "{} ends at {:.2}s while its source keeps sounding ({:.0} dBFS in the next 0.6s, {:.0} dBFS just before). This is often applause, laughter or a music tail cut off abruptly; listen, then extend, fade out or overlap it.",
                            span.label, at, after, before
                        ),
                    }));
                }
            }
        }
        // Beds: audible non-dialogue sound overlapping dialogue words.
        // A declared track role decides; otherwise a source without a transcript is a bed.
        let is_bed = |s: &&timeline::AudibleSpan| match s.role {
            Some(role) => role != timeline::Role::Dialogue,
            None => !transcripts.contains_key(s.source),
        };
        for bed in spans.iter().filter(is_bed) {
            let overlapping: Vec<&TimelineWord> = words
                .iter()
                .filter(|w| w.start >= bed.start && w.end <= bed.start + bed.duration)
                .collect();
            if overlapping.is_empty() {
                continue;
            }
            // Measure up to eight windows of dialogue, spread through the overlap.
            let step = (overlapping.len() / 8).max(1);
            for word in overlapping.iter().step_by(step).take(8) {
                if measurements + 2 > MAX_MEASUREMENTS {
                    break;
                }
                measurements += 2;
                let dialogue = &spans[word.span];
                let window = (word.end - word.start).clamp(0.15, 1.0);
                let d_at = dialogue.source_in + (word.start - dialogue.start) * dialogue.speed;
                let b_at = bed.source_in + (word.start - bed.start) * bed.speed;
                let (Some(d), Some(b)) = (
                    level(dialogue.source, d_at, window * dialogue.speed)?,
                    level(bed.source, b_at, window * bed.speed)?,
                ) else {
                    continue;
                };
                let d_gain = dialogue
                    .mix
                    .gain(word.start - dialogue.start, dialogue.duration);
                let b_gain = bed.mix.gain(word.start - bed.start, bed.duration);
                if d_gain <= 0.0 || b_gain <= 0.0 {
                    continue;
                }
                let d = d + 20.0 * d_gain.log10();
                let b = b + 20.0 * b_gain.log10();
                if b > d - 10.0 {
                    warnings.push(json!({
                        "kind": "bed_close_to_dialogue",
                        "timeline_start": word.start,
                        "bed": bed.label,
                        "dialogue_dbfs": d,
                        "bed_dbfs": b,
                        "message": format!(
                            "At {:.2}s {} is {:.0} dB against dialogue at {:.0} dB (within 10 dB). Speech may be hard to follow; listen, and consider volume keyframes on the bed.",
                            word.start, bed.label, b, d
                        ),
                    }));
                    break;
                }
            }
        }
    }
    let mut not_checked = transcript_gaps(&spans, &transcripts);
    if !brief_not_checked.is_empty() {
        let count = brief_not_checked.len();
        not_checked.push(format!(
            "{count} shot{} under --min-shot-seconds by clip edges {} a fade, dissolve or opacity change at a cut, so brief_shot did not check {} (fades and dissolves are not checked yet); watch {} with skywrite review cuts (spans in checked.brief_shots_not_checked)",
            if count == 1 { "" } else { "s" },
            if count == 1 { "has" } else { "have" },
            if count == 1 { "it" } else { "them" },
            if count == 1 { "it" } else { "them" },
        ));
    }
    for line in &not_checked {
        eprintln!("review lint: {line}");
    }
    warnings.sort_by(|a, b| {
        a["timeline_start"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["timeline_start"].as_f64().unwrap_or(0.0))
    });
    Ok(json!({
        "edit": args.edit,
        "warning_count": warnings.len(),
        "warnings": warnings,
        "checked": {
            "dialogue_sources": transcripts.keys().collect::<Vec<_>>(),
            "dialogue_words_heard": words.len(),
            "levels": levels_checked,
            "min_shot_seconds": args.min_shot_seconds,
            "uncertain_cover_sources": uncertain_cover,
            "brief_shots_not_checked": brief_not_checked,
            "level_measurements": measurements,
            "ending": ending.checked,
            "dead_air": dead.checked,
        },
        "not_checked": not_checked,
        "limitations": [
            "Checks are objective signals, not editorial judgement: they cannot assess delivery, emotional fit, B-roll relevance or music choice.",
            "Speaker and repetition checks only cover sources whose transcripts were supplied, and only as well as those transcripts capture what was said.",
            "Level checks measure short windows of the source audio with the edit's gains applied; they do not replace listening to the mix (skywrite review cuts).",
            "Brief shots are checked only between hard cuts; fades and dissolves are not checked yet: a shot with a fade, dissolve or opacity change at either cut is never reported as brief, and when its clip edges are under the threshold it is listed in checked.brief_shots_not_checked instead, to watch with skywrite review cuts.",
            "Cover (for silent speakers and brief shots) and brief shots are judged from clip placement and opacity, not pixels: only full-frame video whose pixel format has no alpha channel covers what is beneath it, so cuts under stills, letterboxed pictures and alpha-capable video (even fully opaque ProRes 4444) count as visible and can add warnings; video whose pixel format FFmpeg does not describe is listed in checked.uncertain_cover_sources and never covers; overlays that do not cover the frame never dim what is beneath, so at partial opacity they count about half as much; and changes inside a clip's own picture (a cut baked into the source) are not seen."
        ],
    }))
}

// ---------------------------------------------------------------- endings and pauses

/// What the word checks could not cover: no transcripts at all, or audible
/// dialogue-role sources without one.
fn transcript_gaps(
    spans: &[timeline::AudibleSpan],
    transcripts: &HashMap<PathBuf, Transcript>,
) -> Vec<String> {
    if transcripts.is_empty() {
        return vec!["no --transcript was supplied, so unfinished_ending, dead_air, repeated, clipped and filler words and silent visible speakers were not checked; pass --transcript WORDS.json (from skywrite transcribe) for each dialogue source".into()];
    }
    let mut missing: Vec<String> = spans
        .iter()
        .filter(|s| s.role == Some(timeline::Role::Dialogue) && !transcripts.contains_key(s.source))
        .map(|s| s.source.display().to_string())
        .collect();
    missing.sort();
    missing.dedup();
    missing
        .into_iter()
        .map(|source| {
            format!("dialogue source {source} has no --transcript, so its words were not checked")
        })
        .collect()
}

/// One lint check's warning (if any) and what it measured.
struct LintCheck {
    warning: Option<Value>,
    checked: Value,
}

/// Sentence-final punctuation, ignoring closing quotes and brackets. An
/// ellipsis trails off, so it does not end the thought.
fn ends_sentence(text: &str) -> bool {
    let text = text.trim_end_matches(|c: char| c.is_whitespace() || "\"'”’»)]".contains(c));
    if text.ends_with("..") || text.ends_with('…') {
        return false;
    }
    text.ends_with(['.', '!', '?', '。', '！', '？', '؟', '।', '‽'])
}

/// Words that join the next clause to this one, even at the start of a new
/// sentence ("And that's why…"). English, German, French, Spanish, Italian,
/// Portuguese and Dutch; words that are also common English words are left out.
const JOINING_WORDS: &[&str] = &[
    "and", "but", "so", "because", "cause", "cos", "or", "nor", "plus", "und", "aber", "weil",
    "oder", "denn", "sondern", "et", "mais", "parce", "donc", "ou", "y", "pero", "porque", "sino",
    "e", "ma", "perché", "quindi", "però", "oppure", "mas", "porém", "maar", "omdat", "dus",
];
/// Words that continue a clause when the previous word has no sentence-final
/// punctuation, but often start a new sentence after one.
const CLAUSE_WORDS: &[&str] = &[
    "then", "which", "that", "who", "whose", "whom", "where", "although", "though", "unless",
    "until", "while", "whereas", "since", "if", "when", "also", "whether", "dass", "wenn",
    "obwohl", "damit", "quand", "lorsque", "puisque", "alors", "puis", "entonces", "pues",
    "aunque", "cuando", "mentre", "allora", "então", "enquanto", "terwijl", "toen",
];
const LIST_NOUNS: &[&str] = &[
    "things",
    "reasons",
    "ways",
    "steps",
    "tips",
    "points",
    "questions",
    "ideas",
    "rules",
    "lessons",
    "mistakes",
    "parts",
    "options",
    "problems",
    "examples",
    "tricks",
    "secrets",
    "factors",
    "stages",
    "phases",
    "principles",
    "benefits",
    "myths",
    "signs",
    "types",
    "kinds",
];

fn count_word(word: &str) -> Option<u32> {
    let words = [
        "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    words
        .iter()
        .position(|w| *w == word)
        .map(|i| i as u32 + 2)
        .or_else(|| word.parse::<u32>().ok().filter(|n| (2..=10).contains(n)))
}

fn ordinal(word: &str) -> Option<u32> {
    let ordinals = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
        "tenth",
    ];
    let word = word.strip_suffix("ly").unwrap_or(word);
    ordinals
        .iter()
        .position(|w| *w == word)
        .map(|i| i as u32 + 1)
}

fn closes_list(word: &str) -> bool {
    matches!(word, "finally" | "lastly" | "last")
}

/// A counted list that is still open where the heard dialogue stops: an
/// announced count ("three reasons") with fewer items reached, or an ordinal
/// sequence whose next item follows in the source after the cut.
fn open_list(words: &[HeardRun], next: &[&Word], end: f64) -> Option<Value> {
    let normalized: Vec<String> = words.iter().map(|w| normalize(&w.word.text)).collect();
    let announced = (0..normalized.len()).rev().find_map(|i| {
        let count = count_word(&normalized[i])?;
        normalized[i + 1..]
            .iter()
            .take(3)
            .any(|w| LIST_NOUNS.contains(&w.as_str()))
            .then_some((i, count))
    });
    let reached_after = |from: usize| {
        let after = &normalized[from..];
        let reached = after.iter().filter_map(|w| ordinal(w)).max().unwrap_or(0);
        (reached, after.iter().any(|w| closes_list(w)))
    };
    if let Some((index, count)) = announced {
        let (reached, closed) = reached_after(index + 1);
        if reached < count && !closed {
            return Some(
                json!({"announced": count, "announced_at": words[index].start,
                "words": words[index..].iter().take(4).map(|w| w.word.text.as_str()).collect::<Vec<_>>().join(" "),
                "reached": reached}),
            );
        }
        return None;
    }
    // Ordinals heard in the last minute, with the next one just after the cut.
    let from = words.iter().position(|w| w.end >= end - 60.0).unwrap_or(0);
    let (reached, closed) = reached_after(from);
    if reached == 0 || closed {
        return None;
    }
    let after_cut = next
        .iter()
        .find(|w| ordinal(&normalize(&w.text)) == Some(reached + 1))?;
    Some(
        json!({"reached": reached, "next_item_word": after_cut.text, "next_item_source_seconds": after_cut.start}),
    )
}

/// Whether the edit's last heard dialogue stops mid-thought.
fn unfinished_ending(
    spans: &[timeline::AudibleSpan],
    transcripts: &HashMap<PathBuf, Transcript>,
    frame: f64,
) -> LintCheck {
    // The heard dialogue's end, from the gain-aware runs: a word whose tail is
    // muted still ends the dialogue where it stops being heard.
    let runs = heard_runs(spans, transcripts);
    let Some(last) = runs.iter().max_by(|a, b| a.end.total_cmp(&b.end)) else {
        return LintCheck {
            warning: None,
            checked: json!({"measured": false, "reason": "no dialogue words are heard"}),
        };
    };
    let source = spans[last.span].source;
    let transcript = &transcripts[source];
    let punctuated = transcript.words.iter().any(|w| ends_sentence(&w.text));
    // The final continuous playback: the last clip and the clips that run
    // straight into it (same source, contiguous in source and timeline).
    let mut playback = vec![last.span];
    while let Some(previous) = spans.iter().enumerate().position(|(i, o)| {
        let current = &spans[*playback.last().unwrap_or(&last.span)];
        !playback.contains(&i)
            && o.source == current.source
            && (o.source_in + o.duration * o.speed - current.source_in).abs()
                <= frame * o.speed.max(current.speed) * 1.01
            && (o.start + o.duration - current.start).abs() <= frame * 1.01
    }) {
        playback.push(previous);
    }
    // The source words after the cut: from the last heard word on, minus the
    // words that playback already played (overlapping words are not cut off;
    // words played earlier in the edit, out of order, still are).
    let heard = |word: &Word| {
        runs.iter()
            .any(|r| playback.contains(&r.span) && std::ptr::eq(r.word, word))
    };
    let mut next: Vec<&Word> = transcript
        .words
        .iter()
        .filter(|w| w.start >= last.word.start && w.start <= last.word.end + 30.0 && !heard(w))
        .collect();
    next.sort_by(|a, b| a.start.total_cmp(&b.start));
    let finished = ends_sentence(&last.word.text);
    let mut reasons = Vec::new();
    if punctuated && !finished {
        reasons.push(json!({"reason": "no_sentence_final_punctuation", "word": last.word.text}));
    }
    if let Some(following) = next.first()
        && following.start - last.word.end <= 2.0
    {
        let word = normalize(&following.text);
        if JOINING_WORDS.contains(&word.as_str())
            || (!(punctuated && finished) && CLAUSE_WORDS.contains(&word.as_str()))
        {
            reasons.push(json!({"reason": "next_word_continues", "word": following.text, "source_seconds": following.start}));
        }
    }
    // Heard words in timeline order, a word split by a cut or a mute once.
    let mut heard_words: Vec<HeardRun> = Vec::new();
    for run in &runs {
        if let Some(previous) = heard_words.last_mut()
            && std::ptr::eq(previous.word, run.word)
        {
            previous.end = previous.end.max(run.end);
            continue;
        }
        heard_words.push(HeardRun {
            start: run.start,
            end: run.end,
            word: run.word,
            span: run.span,
        });
    }
    let list = open_list(&heard_words, &next, last.end);
    if let Some(list) = &list {
        let mut list = list.clone();
        list["reason"] = json!("open_list");
        reasons.push(list);
    }
    let mut tail: Vec<&str> = Vec::new();
    for run in runs.iter().filter(|r| r.end <= last.end).rev() {
        if tail.len() == 6 {
            break;
        }
        // A word split by a cut or a mute is one word.
        if tail.last() != Some(&run.word.text.as_str()) {
            tail.push(run.word.text.as_str());
        }
    }
    tail.reverse();
    let cut_off: Vec<&str> = next.iter().take(6).map(|w| w.text.as_str()).collect();
    let checked = json!({
        "measured": true,
        "last_word": last.word.text,
        "timeline_end": last.end,
        "source": source,
        "source_seconds": last.word.end,
        "punctuation": if punctuated {"judged"} else {"not judged: the transcript has no sentence-final punctuation"},
        "next_source_words": cut_off.join(" "),
        "continuation_languages": "English, German, French, Spanish, Italian, Portuguese, Dutch",
        "counted_lists": "English number words, digits 2-10 and ordinals",
    });
    let warning = (!reasons.is_empty()).then(|| {
        json!({
            "kind": "unfinished_ending",
            "timeline_start": last.start,
            "timeline_end": last.end,
            "source": source,
            "source_seconds": last.word.end,
            "last_words": tail.join(" "),
            "next_source_words": cut_off.join(" "),
            "reasons": reasons,
            "message": format!(
                "The edit's dialogue ends at {:.2}s on \"{}\", which may stop mid-thought (next in the source: \"{}\"). Listen to the ending; extend it to the end of the sentence or list, or end on an earlier complete thought.",
                last.end, tail.join(" "), cut_off.join(" ")
            ),
        })
    });
    LintCheck { warning, checked }
}

/// A stretch of one transcribed word heard through one clip, on the timeline.
struct HeardRun<'a> {
    start: f64,
    end: f64,
    word: &'a Word,
    span: usize,
}

/// Every stretch of every transcribed word that a clip plays with its gain
/// above zero, computed per clip (so a word split by a cut keeps each part's
/// own gain) and sampled every 10 ms (at most 1000 samples per word).
fn heard_runs<'a>(
    spans: &[timeline::AudibleSpan],
    transcripts: &'a HashMap<PathBuf, Transcript>,
) -> Vec<HeardRun<'a>> {
    let mut runs = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let Some(transcript) = transcripts.get(span.source) else {
            continue;
        };
        let source_end = span.source_in + span.duration * span.speed;
        for word in &transcript.words {
            if word.end <= span.source_in || word.start >= source_end {
                continue;
            }
            let t0 = span.start + (word.start.max(span.source_in) - span.source_in) / span.speed;
            let t1 = span.start + (word.end.min(source_end) - span.source_in) / span.speed;
            let steps = (((t1 - t0) / 0.01).ceil() as usize).clamp(1, 1000);
            let at = |i: usize| t0 + (t1 - t0) * i as f64 / steps as f64;
            let heard_at = |t: f64| {
                span.mix
                    .gain((t - span.start).clamp(0.0, span.duration), span.duration)
                    > 0.0
            };
            let audible = |i: usize| heard_at(at(i));
            // The samples find where the gain changes; the exact point between
            // two samples is found by bisection, so a fade ending at a word's
            // end ends the heard run there, not a sample earlier.
            let edge = |mut heard: f64, mut silent: f64| {
                for _ in 0..40 {
                    let middle = (heard + silent) / 2.0;
                    if heard_at(middle) {
                        heard = middle;
                    } else {
                        silent = middle;
                    }
                }
                (heard + silent) / 2.0
            };
            let mut run: Option<(f64, f64)> = None;
            for i in 0..=steps {
                if audible(i) {
                    run = Some(match run {
                        Some((start, _)) => (start, at(i)),
                        None if i > 0 => (edge(at(i), at(i - 1)), at(i)),
                        None => (at(i), at(i)),
                    });
                } else if let Some((start, _)) = run.take() {
                    runs.push(HeardRun {
                        start,
                        end: edge(at(i - 1), at(i)),
                        word,
                        span: index,
                    });
                }
            }
            if let Some((start, end)) = run {
                runs.push(HeardRun {
                    start,
                    end,
                    word,
                    span: index,
                });
            }
        }
    }
    runs.sort_by(|a, b| a.start.total_cmp(&b.start));
    runs
}

/// Silences longer than `threshold` between heard dialogue words, on the
/// output timeline. A gap the edit's `pauses` cover, except for at most
/// `threshold`, is deliberate.
fn dead_air(
    spans: &[timeline::AudibleSpan],
    transcripts: &HashMap<PathBuf, Transcript>,
    threshold: f64,
    pauses: &[timeline::Pause],
) -> LintCheck {
    let mut items = Vec::new();
    let mut excluded = Vec::new();
    let mut gaps = 0usize;
    // Only the parts of words whose gain is above zero are heard, so volume
    // automation that mutes part of a word opens a gap where it falls silent.
    let heard = heard_runs(spans, transcripts);
    // The heard run that ends last so far, so overlapping speakers are one stream.
    let mut latest: Option<&HeardRun> = None;
    for word in &heard {
        if let Some(previous) = latest {
            gaps += 1;
            let (start, end) = (previous.end, word.start);
            if end - start > threshold + 1e-9 {
                let overlapping =
                    |s: &&timeline::AudibleSpan| s.start < end && s.start + s.duration > start;
                let labels = |dialogue: bool| -> Vec<String> {
                    spans
                        .iter()
                        .filter(overlapping)
                        .filter(|s| !transcripts.contains_key(s.source))
                        .filter(|s| (s.role == Some(timeline::Role::Dialogue)) == dialogue)
                        .map(|s| s.label.clone())
                        .collect()
                };
                let item = json!({
                    "timeline_start": start,
                    "timeline_end": end,
                    "seconds": end - start,
                    "before": previous.word.text,
                    "after": word.word.text,
                    "across_cut": previous.span != word.span,
                    "other_sound": labels(false),
                    "untranscribed_dialogue": labels(true),
                });
                let covered: f64 = pauses
                    .iter()
                    .map(|p| (p.end.min(end) - p.start.max(start)).max(0.0))
                    .sum();
                if covered > 0.0 && end - start - covered <= threshold + 1e-9 {
                    let mut item = item;
                    item["pause_notes"] = json!(
                        pauses
                            .iter()
                            .filter(|p| p.start < end && p.end > start)
                            .map(|p| p.note.as_str())
                            .collect::<Vec<_>>()
                    );
                    excluded.push(item);
                } else {
                    items.push(item);
                }
            }
        }
        if latest.is_none_or(|l| word.end > l.end) {
            latest = Some(word);
        }
    }
    let checked = json!({
        "threshold_seconds": threshold,
        "word_gaps_measured": gaps,
        "pauses": pauses,
        "excluded_by_pause": excluded,
        "basis": "transcript word times mapped to the output timeline; silence before the first and after the last heard word is not counted",
    });
    let warning = (!items.is_empty()).then(|| {
        let longest = items
            .iter()
            .max_by(|a, b| a["seconds"].as_f64().unwrap_or(0.0).total_cmp(&b["seconds"].as_f64().unwrap_or(0.0)))
            .cloned()
            .unwrap_or(Value::Null);
        json!({
            "kind": "dead_air",
            "timeline_start": items[0]["timeline_start"],
            "count": items.len(),
            "threshold_seconds": threshold,
            "longest_seconds": longest["seconds"],
            "items": items.iter().take(64).collect::<Vec<_>>(),
            "message": format!(
                "{} pause(s) longer than {threshold} s between heard dialogue words (longest {:.2} s at {:.2}s). Listen with skywrite review cuts; tighten the cut, or mark a deliberate beat in the edit's pauses list (start and end in timeline seconds, optional note).",
                items.len(),
                longest["seconds"].as_f64().unwrap_or(0.0),
                longest["timeline_start"].as_f64().unwrap_or(0.0),
            ),
        })
    });
    LintCheck { warning, checked }
}

#[cfg(test)]
mod ending_tests {
    use super::*;

    #[test]
    fn sentence_ends_and_continuations() {
        for finished in ["late.", "Done!", "really?", "end.\"", "(quoted.)", "完了。"] {
            assert!(ends_sentence(finished), "{finished}");
        }
        for open in ["early,", "and", "so...", "well…", "plan"] {
            assert!(!ends_sentence(open), "{open}");
        }
        assert!(JOINING_WORDS.contains(&normalize("And,").as_str()));
        assert!(JOINING_WORDS.contains(&normalize("Weil").as_str()));
        assert!(CLAUSE_WORDS.contains(&normalize("which").as_str()));
        assert!(!JOINING_WORDS.contains(&"the") && !CLAUSE_WORDS.contains(&"the"));
        assert_eq!(count_word("three"), Some(3));
        assert_eq!(count_word("10"), Some(10));
        assert_eq!(count_word("one"), None);
        assert_eq!(ordinal("secondly"), Some(2));
        assert_eq!(ordinal("third"), Some(3));
        assert_eq!(ordinal("early"), None);
    }
}

/// Share of the picture that must change at once for a cut (see
/// `picture_distance`).
const PICTURE_CHANGE: f64 = 0.25;

/// Black (no picture) in a frame's weights.
const BLACK: usize = usize::MAX;

/// Frames from one cut to the next. `hard` when no visible clip fades in it or
/// in the frame on either side, and no visible take continues at another
/// opacity at its cuts or inside it, so both cuts are hard and its length is
/// exact.
struct Shot {
    start: u64,
    end: u64,
    hard: bool,
}

/// How much of the picture differs between two weight lists: half the sum of
/// absolute weight differences, 1 for a cut between two full-frame shots.
fn picture_distance(a: &[(usize, f64)], b: &[(usize, f64)]) -> f64 {
    let (mut i, mut j, mut sum) = (0, 0, 0.0);
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                sum += (x.1 - y.1).abs();
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                sum += x.1;
                i += 1;
            }
            (Some(x), None) => {
                sum += x.1;
                i += 1;
            }
            (_, Some(y)) => {
                sum += y.1;
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    sum / 2.0
}

/// The timeline's shots, in order. See `lint` (1b) for the model; black lies
/// beyond both ends of the timeline.
fn shots(
    edit: &timeline::ValidatedEdit,
    covers_frame: &dyn Fn(&timeline::ValidatedVideoClip) -> bool,
) -> Vec<Shot> {
    let fps = edit.fps();
    let frame = fps.seconds(1);
    let total = edit.duration_frames();
    // Invisible clips show nothing. Tracks are sorted by start (no overlaps).
    let mut tracks: Vec<Vec<&timeline::ValidatedVideoClip>> = Vec::new();
    for clip in edit.video_clips().filter(|c| c.opacity > 0.0) {
        if tracks.len() <= clip.track {
            tracks.resize_with(clip.track + 1, Vec::new);
        }
        tracks[clip.track].push(clip);
    }
    for track in &mut tracks {
        track.sort_by_key(|c| c.start_frames);
    }
    // A straight split continues the take: same track and source, abutting,
    // and source-contiguous within one frame of the slower clip's step.
    let continues = |a: &timeline::ValidatedVideoClip, b: &timeline::ValidatedVideoClip| {
        a.source == b.source
            && a.end_frames() == b.start_frames
            && (a.source_in + fps.seconds(a.duration_frames) * a.speed - b.source_in).abs()
                <= frame * a.speed.min(b.speed) * 1.01
    };
    let mut takes: Vec<Vec<usize>> = Vec::with_capacity(tracks.len());
    let mut next = 0;
    for track in &tracks {
        let mut ids = Vec::with_capacity(track.len());
        for (i, clip) in track.iter().enumerate() {
            if i > 0 && continues(track[i - 1], clip) {
                ids.push(ids[i - 1]);
            } else {
                ids.push(next);
                next += 1;
            }
        }
        takes.push(ids);
    }
    // Frames a clip's fades can change, with a frame to spare for FFmpeg
    // rounding fades to whole frames.
    let fading = |clip: &timeline::ValidatedVideoClip| {
        let frames = |seconds: f64| (seconds / frame).ceil() as u64 + 1;
        let (start, end) = (clip.start_frames, clip.end_frames());
        let fade_in = if clip.fade_in > 0.0 {
            start..(start + frames(clip.fade_in)).min(end)
        } else {
            start..start
        };
        let fade_out = if clip.fade_out > 0.0 {
            end.saturating_sub(frames(clip.fade_out)).max(start)..end
        } else {
            end..end
        };
        [fade_in, fade_out]
    };
    // The picture at frame n, from the top track down: each take's opacity
    // times what full-frame opaque cover above lets through, black getting
    // what is left; whether a visible clip is fading there; and whether a
    // visible take continues at another opacity there.
    let picture = |n: u64| {
        let mut through = 1.0;
        let (mut fades, mut steps) = (false, false);
        let mut out: Vec<(usize, f64)> = Vec::new();
        for (track, ids) in tracks.iter().zip(&takes).rev() {
            if through <= 1e-9 {
                break;
            }
            let at = track.partition_point(|c| c.start_frames <= n);
            let Some(i) = at.checked_sub(1) else {
                continue;
            };
            let clip = track[i];
            if n >= clip.end_frames() {
                continue;
            }
            fades |= fading(clip).iter().any(|r| r.contains(&n));
            steps |= clip.start_frames == n
                && i > 0
                && ids[i - 1] == ids[i]
                && track[i - 1].opacity != clip.opacity;
            out.push((ids[i], clip.opacity * through));
            if covers_frame(clip) {
                through *= 1.0 - clip.opacity;
            }
        }
        out.push((BLACK, through));
        out.retain(|(_, w)| *w > 1e-9);
        out.sort_unstable_by_key(|(id, _)| *id);
        (out, fades, steps)
    };
    // The picture only changes at clip edges and fade ends.
    let mut breaks = std::collections::BTreeSet::from([0]);
    for clip in tracks.iter().flatten() {
        breaks.insert(clip.start_frames);
        breaks.insert(clip.end_frames());
        for range in fading(clip) {
            breaks.insert(range.start);
            breaks.insert(range.end);
        }
    }
    let starts: Vec<u64> = breaks.into_iter().filter(|&n| n < total).collect();
    let pictures: Vec<_> = starts.iter().map(|&n| picture(n)).collect();
    let black = [(BLACK, 1.0)];
    let weights = |i: usize| pictures.get(i).map_or(black.as_slice(), |p| p.0.as_slice());
    // Cuts as indices into `starts`; `starts.len()` is the timeline's end.
    let cuts: Vec<usize> = (0..=starts.len())
        .filter(|&i| {
            let before = i.checked_sub(1).map_or(black.as_slice(), weights);
            picture_distance(before, weights(i)) >= PICTURE_CHANGE - 1e-9
        })
        .collect();
    cuts.windows(2)
        .map(|pair| {
            let (first, last) = (pair[0], pair[1]);
            Shot {
                start: starts[first],
                end: starts.get(last).copied().unwrap_or(total),
                // No fade in the shot or the frame on either side, and no
                // opacity step at its cuts or inside it.
                hard: !pictures[first.saturating_sub(1)..(last + 1).min(pictures.len())]
                    .iter()
                    .any(|p| p.1)
                    && !pictures[first..(last + 1).min(pictures.len())]
                        .iter()
                        .any(|p| p.2),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interval_subtraction_and_word_normalization() {
        let pieces = subtract(
            Interval {
                start: 0.0,
                end: 10.0,
            },
            &[
                Interval {
                    start: 2.0,
                    end: 4.0,
                },
                Interval {
                    start: 8.0,
                    end: 12.0,
                },
            ],
        );
        let pairs: Vec<(f64, f64)> = pieces.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(pairs, vec![(0.0, 2.0), (4.0, 8.0)]);
        assert_eq!(normalize("Evanston,"), "evanston");
        assert_eq!(normalize(" UM… "), "um");
    }
}
