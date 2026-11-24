//! `verify --ends-together / --loudness / --black / --frozen` on generated renders.
use serde_json::Value;
use std::{path::Path, process::Command};

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An H.264/AAC MP4 from a lavfi picture and sound, like a render.
fn mp4(dir: &Path, name: &str, video: &str, audio: &str) -> String {
    let path = dir.join(name);
    let path_str = path.to_str().unwrap();
    ffmpeg(&[
        "-v", "error", "-y", "-f", "lavfi", "-i", video, "-f", "lavfi", "-i", audio, "-c:v",
        "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", path_str,
    ]);
    path_str.to_owned()
}

/// Run verify; failed checks exit 1 with the report still on stdout.
fn verify(file: &str, extra: &[&str]) -> (bool, Value) {
    let cache = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env("SKYWRITE_CACHE_DIR", cache.path())
        .arg("verify")
        .arg(file)
        .args(extra)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("no report: {}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(out.status.success(), report["passed"] == true, "{report}");
    (out.status.success(), report)
}

fn check<'a>(report: &'a Value, property: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["property"] == property)
        .unwrap_or_else(|| panic!("no {property} check: {report}"))
}

const MOVING: &str = "testsrc2=s=160x90:r=30:d=3";
/// About -16 LUFS once encoded (a -18 dBFS sine raised 5.8 dB).
const TONE: &str = "sine=frequency=440:sample_rate=48000:duration=3,volume=5.8dB";

#[test]
fn clean_render_passes_every_check_and_says_what_was_not_measured() {
    let dir = tempfile::tempdir().unwrap();
    let clean = mp4(dir.path(), "clean.mp4", MOVING, TONE);
    let (ok, report) = verify(
        &clean,
        &["--ends-together", "--loudness", "--black", "--frozen"],
    );
    assert!(ok, "{report}");
    let ends = check(&report, "ends_together");
    assert!((ends["video_end_seconds"].as_f64().unwrap() - 3.0).abs() < 1e-3);
    assert!((ends["audio_end_seconds"].as_f64().unwrap() - 3.0).abs() < 0.03);
    assert_eq!(ends["tolerance"], 0.05);
    let lufs = check(&report, "integrated_loudness")["actual_lufs"]
        .as_f64()
        .unwrap();
    assert!((lufs + 16.0).abs() < 1.0, "{report}");
    assert!(check(&report, "true_peak")["actual_dbtp"].as_f64().unwrap() < -1.0);
    assert_eq!(check(&report, "first_frame_black")["passed"], true);
    assert_eq!(
        check(&report, "black_frames")["spans"],
        serde_json::json!([])
    );
    assert_eq!(
        check(&report, "frozen_frames")["spans"],
        serde_json::json!([])
    );
    let not_measured = report["not_measured"].to_string();
    assert!(!not_measured.contains("--loudness"), "{report}");
    assert!(not_measured.contains("creative quality"), "{report}");

    // Without the flags, the same file is verified but nothing is judged.
    let (ok, report) = verify(&clean, &[]);
    assert!(ok);
    assert!(report["checks"].as_array().unwrap().is_empty());
    for flag in ["--ends-together", "--loudness", "--black", "--frozen"] {
        assert!(
            report["not_measured"].to_string().contains(flag),
            "{report}"
        );
    }
    assert!(report["stream_ends"]["difference_seconds"].is_number());
}

#[test]
fn picture_and_sound_ends_at_and_past_the_tolerance() {
    let dir = tempfile::tempdir().unwrap();
    let short = mp4(
        dir.path(),
        "short.mp4",
        MOVING,
        "sine=frequency=440:sample_rate=48000:duration=2.9",
    );
    let (ok, report) = verify(&short, &["--ends-together"]);
    assert!(!ok);
    let ends = check(&report, "ends_together");
    assert!(
        (ends["difference_seconds"].as_f64().unwrap() - 0.1).abs() < 0.005,
        "{report}"
    );
    let nearly = mp4(
        dir.path(),
        "nearly.mp4",
        MOVING,
        "sine=frequency=440:sample_rate=48000:duration=2.97",
    );
    let (ok, report) = verify(&nearly, &["--ends-together"]);
    assert!(ok, "{report}");
    let difference = check(&report, "ends_together")["difference_seconds"]
        .as_f64()
        .unwrap();
    assert!((difference - 0.03).abs() < 0.005, "{report}");
    let (ok, _) = verify(
        &nearly,
        &[
            "--ends-together",
            "--end-tolerance",
            &format!("{:.4}", difference + 0.002),
        ],
    );
    assert!(ok);
    let (ok, _) = verify(
        &nearly,
        &[
            "--ends-together",
            "--end-tolerance",
            &format!("{:.4}", difference - 0.002),
        ],
    );
    assert!(!ok);
}

#[test]
fn black_lead_in_first_frame_and_internal_black_spans() {
    let dir = tempfile::tempdir().unwrap();
    // Half a second of black before the picture, as from a late first clip.
    let lead = mp4(
        dir.path(),
        "lead.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=lt(t\\,0.5)",
        TONE,
    );
    let (ok, report) = verify(
        &lead,
        &["--black", "--frozen", "--frozen-min-seconds", "0.3"],
    );
    assert!(!ok);
    let first = check(&report, "first_frame_black");
    assert_eq!(first["passed"], false);
    assert!((first["black_until_seconds"].as_f64().unwrap() - 0.5).abs() < 1e-3);
    let spans = check(&report, "black_frames")["spans"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(spans.len(), 1, "{report}");
    assert_eq!(spans[0]["at_start"], true);
    // The black lead-in is not reported again as a frozen picture.
    let frozen = check(&report, "frozen_frames");
    assert_eq!(frozen["passed"], true, "{report}");
    assert_eq!(frozen["black_spans_not_repeated"], 1, "{report}");

    // One black first frame fails first_frame_black on its own.
    let one = mp4(
        dir.path(),
        "one.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=lt(n\\,1)",
        TONE,
    );
    let (ok, report) = verify(&one, &["--black"]);
    assert!(!ok);
    assert_eq!(check(&report, "first_frame_black")["passed"], false);
    assert_eq!(check(&report, "black_frames")["passed"], true, "{report}");
    let (_, report) = verify(&one, &["--black", "--black-min-seconds", "0"]);
    assert_eq!(check(&report, "black_frames")["passed"], false);

    // Three black frames (0.1 s) mid-picture: listed but passing at the default
    // 0.5 s minimum; failing at a 0.1 s minimum, and passing just past it.
    let flash = mp4(
        dir.path(),
        "flash.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=between(n\\,45\\,47)",
        TONE,
    );
    let (ok, report) = verify(&flash, &["--black"]);
    assert!(ok, "{report}");
    assert_eq!(check(&report, "black_frames")["spans"][0]["fails"], false);
    let (ok, report) = verify(&flash, &["--black", "--black-min-seconds", "0.1"]);
    assert!(!ok, "{report}");
    let span = &check(&report, "black_frames")["spans"][0];
    assert!((span["start_seconds"].as_f64().unwrap() - 1.5).abs() < 1e-3);
    assert!((span["duration_seconds"].as_f64().unwrap() - 0.1).abs() < 1e-3);
    assert_eq!(span["at_start"], false);
    assert_eq!(span["at_end"], false);
    assert_eq!(check(&report, "first_frame_black")["passed"], true);
    let (ok, _) = verify(&flash, &["--black", "--black-min-seconds", "0.11"]);
    assert!(ok);

    // A fade to black that holds to the end reaches the stream's end.
    let tail = mp4(
        dir.path(),
        "tail.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=gte(t\\,2.5)",
        TONE,
    );
    let (_, report) = verify(&tail, &["--black"]);
    let span = &check(&report, "black_frames")["spans"][0];
    assert_eq!(span["at_end"], true, "{report}");
    assert!(
        (span["end_seconds"].as_f64().unwrap() - 3.0).abs() < 1e-3,
        "{report}"
    );
}

#[test]
fn frozen_spans_at_and_past_the_minimum() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frozen.mp4");
    // One second of motion, 2.5 s held, then one second of motion.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=30:d=1",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=30:d=1",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4.5,volume=5.8dB",
        "-filter_complex",
        "[0:v]tpad=stop_mode=clone:stop_duration=2.5[held];[held][1:v]concat=n=2:v=1:a=0[v]",
        "-map",
        "[v]",
        "-map",
        "2:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        path.to_str().unwrap(),
    ]);
    let file = path.to_str().unwrap();
    let (ok, report) = verify(file, &["--frozen"]);
    assert!(!ok);
    let spans = check(&report, "frozen_frames")["spans"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(spans.len(), 1, "{report}");
    let start = spans[0]["start_seconds"].as_f64().unwrap();
    let end = spans[0]["end_seconds"].as_f64().unwrap();
    assert!((0.9..=1.01).contains(&start), "{report}");
    assert!((end - 3.5).abs() < 0.04, "{report}");
    let (ok, _) = verify(file, &["--frozen", "--frozen-min-seconds", "2.5"]);
    assert!(!ok);
    let (ok, _) = verify(file, &["--frozen", "--frozen-min-seconds", "2.6"]);
    assert!(ok);

    // A picture held to the end of the file has an open-ended freeze.
    let held = mp4(
        dir.path(),
        "held.mp4",
        "testsrc2=s=160x90:r=30:d=1,tpad=stop_mode=clone:stop_duration=2",
        TONE,
    );
    let (ok, report) = verify(&held, &["--frozen"]);
    assert!(!ok);
    let span = &check(&report, "frozen_frames")["spans"][0];
    assert!(
        (span["end_seconds"].as_f64().unwrap() - 3.0).abs() < 1e-3,
        "{report}"
    );
}

#[test]
fn loudness_at_and_past_the_target_and_peak() {
    let dir = tempfile::tempdir().unwrap();
    let quiet = mp4(
        dir.path(),
        "quiet.mp4",
        MOVING,
        "sine=frequency=440:sample_rate=48000:duration=3,volume=0.05",
    );
    let (ok, report) = verify(&quiet, &["--loudness"]);
    assert!(!ok);
    let actual = check(&report, "integrated_loudness")["actual_lufs"]
        .as_f64()
        .unwrap();
    assert!(actual < -40.0, "{report}");
    assert_eq!(check(&report, "true_peak")["passed"], true);
    for (offset, passes) in [(0.95, true), (-0.95, true), (1.05, false), (-1.05, false)] {
        let target = format!("{:.2}", actual + offset);
        let (ok, report) = verify(&quiet, &["--loudness", "--target-lufs", &target]);
        assert_eq!(ok, passes, "target {target}: {report}");
    }

    let loud = mp4(
        dir.path(),
        "loud.mp4",
        MOVING,
        "anoisesrc=d=3:c=pink:a=1:r=48000,volume=3dB",
    );
    let (ok, report) = verify(&loud, &["--loudness"]);
    assert!(!ok);
    let peak = check(&report, "true_peak");
    let actual = peak["actual_dbtp"].as_f64().unwrap();
    assert!(actual > -1.0, "{report}");
    assert_eq!(peak["passed"], false);
    assert!(
        check(&report, "integrated_loudness")["actual_lufs"]
            .as_f64()
            .unwrap()
            > -16.0
    );
    let lufs = check(&report, "integrated_loudness")["actual_lufs"]
        .as_f64()
        .unwrap();
    let target = format!("{lufs:.2}");
    let (ok, _) = verify(
        &loud,
        &[
            "--loudness",
            "--target-lufs",
            &target,
            "--max-true-peak",
            &format!("{:.2}", actual + 0.05),
        ],
    );
    assert!(ok);
    let (ok, _) = verify(
        &loud,
        &[
            "--loudness",
            "--target-lufs",
            &target,
            "--max-true-peak",
            &format!("{:.2}", actual - 0.05),
        ],
    );
    assert!(!ok);

    // Digital silence cannot be measured, and says so.
    let silent = mp4(
        dir.path(),
        "silent.mp4",
        MOVING,
        "anullsrc=r=48000:cl=stereo:d=3",
    );
    let (ok, report) = verify(&silent, &["--loudness"]);
    assert!(!ok);
    let integrated = check(&report, "integrated_loudness");
    assert!(integrated["actual_lufs"].is_null());
    assert!(
        integrated["reason"]
            .as_str()
            .unwrap()
            .contains("not measurable")
    );
}

#[test]
fn decode_checks_refuse_metadata_only_and_bad_limits() {
    let dir = tempfile::tempdir().unwrap();
    let clean = mp4(dir.path(), "clean.mp4", MOVING, TONE);
    for extra in [
        &["--metadata-only", "--loudness"][..],
        &["--metadata-only", "--black"],
        &["--loudness", "--target-lufs", "0"],
        &["--black", "--black-pixel-threshold", "2"],
        &["--frozen", "--frozen-min-seconds", "0"],
        &["--ends-together", "--end-tolerance", "-1"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .arg("verify")
            .arg(&clean)
            .args(extra)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{extra:?}");
        assert!(out.stdout.is_empty(), "{extra:?}");
    }
}

#[test]
fn black_and_freeze_spans_at_the_end_of_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    // Black frames 75-88 with a visible last frame: not "at the end".
    let visible = mp4(
        dir.path(),
        "visible.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=between(n\\,75\\,88)",
        TONE,
    );
    let (ok, report) = verify(&visible, &["--black"]);
    assert!(ok, "{report}");
    let span = &check(&report, "black_frames")["spans"][0];
    assert_eq!(span["at_end"], false, "{report}");
    assert!(
        (span["duration_seconds"].as_f64().unwrap() - 0.4667).abs() < 1e-3,
        "{report}"
    );

    // A still held to the end for 2.033 s: found at that minimum, not past it.
    let held = mp4(
        dir.path(),
        "held.mp4",
        "testsrc2=s=160x90:r=30:d=1,tpad=stop_mode=clone:stop_duration=2",
        TONE,
    );
    let (ok, report) = verify(&held, &["--frozen", "--frozen-min-seconds", "2.03"]);
    assert!(!ok, "{report}");
    let span = &check(&report, "frozen_frames")["spans"][0];
    assert!(
        (span["duration_seconds"].as_f64().unwrap() - 2.0333).abs() < 1e-3,
        "{report}"
    );
    let (ok, report) = verify(&held, &["--frozen", "--frozen-min-seconds", "2.04"]);
    assert!(ok, "{report}");
}

#[test]
fn video_checks_on_an_audio_only_file_fail_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tone.wav");
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        TONE,
        path.to_str().unwrap(),
    ]);
    let (ok, report) = verify(
        path.to_str().unwrap(),
        &["--black", "--frozen", "--ends-together"],
    );
    assert!(!ok);
    for property in [
        "first_frame_black",
        "black_frames",
        "frozen_frames",
        "ends_together",
    ] {
        let entry = check(&report, property);
        assert_eq!(entry["passed"], false, "{report}");
        assert!(entry["reason"].as_str().is_some(), "{report}");
    }
}

#[test]
fn a_short_stream_in_a_long_container_is_measured_from_its_own_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.ts");
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=30:d=60",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=5",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        path.to_str().unwrap(),
    ]);
    let (ok, report) = verify(path.to_str().unwrap(), &[]);
    assert!(ok, "{report}");
    let ends = &report["stream_ends"];
    assert!(
        (ends["video_end_seconds"].as_f64().unwrap() - 60.0).abs() < 0.05,
        "{report}"
    );
    assert!(
        (ends["audio_end_seconds"].as_f64().unwrap() - 5.0).abs() < 0.05,
        "{report}"
    );
    let (ok, _) = verify(path.to_str().unwrap(), &["--ends-together"]);
    assert!(!ok);
}

#[test]
fn matroska_vp9_and_opus_ends_are_measured() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mkv");
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            MOVING,
            "-f",
            "lavfi",
            "-i",
            TONE,
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-c:a",
            "libopus",
        ])
        .arg(&path)
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!("skipping: this FFmpeg cannot encode VP9/Opus");
        return;
    }
    let (_, report) = verify(path.to_str().unwrap(), &["--ends-together"]);
    // Opus pre-skip puts the container start a few ms before zero, and ends
    // are measured from the container start. FFmpeg 5.1 also delays the video
    // stream by the pre-skip, so the 3-s picture ends 3 s after its own start.
    let ends = &report["stream_ends"];
    let media = &report["media"];
    let video_start =
        media["streams"][0]["start_time"].as_f64().unwrap() - media["start_time"].as_f64().unwrap();
    let video_end = ends["video_end_seconds"].as_f64().unwrap();
    assert!((video_end - video_start - 3.0).abs() < 2e-3, "{report}");
    assert!(
        ends["difference_seconds"].as_f64().unwrap().abs() < 0.03,
        "{report}"
    );
}

#[test]
fn a_still_after_a_short_black_span_is_reported_as_frozen() {
    let dir = tempfile::tempdir().unwrap();
    let file = mp4(
        dir.path(),
        "black-then-still.mp4",
        "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=between(t\\,1\\,1.09),drawbox=c=blue:t=fill:enable=between(t\\,1.09\\,1.19)",
        TONE,
    );
    let (ok, report) = verify(
        &file,
        &["--black", "--frozen", "--frozen-min-seconds", "0.1"],
    );
    assert!(!ok, "{report}");
    let frozen = check(&report, "frozen_frames");
    let spans = frozen["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 1, "{report}");
    assert!(
        (spans[0]["start_seconds"].as_f64().unwrap() - 1.1).abs() < 1e-3,
        "{report}"
    );
    assert_eq!(frozen["black_spans_not_repeated"], 1, "{report}");
}

/// Render an edit of `clips` on one video track with `skywrite edit render`.
fn render(dir: &Path, name: &str, clips: Value) -> (String, String) {
    let edit = dir.join(format!("{name}.json"));
    std::fs::write(
        &edit,
        serde_json::to_vec(&serde_json::json!({"version":1,"width":160,"height":90,
            "fps":{"num":30,"den":1},"video":[{"clips":clips}]}))
        .unwrap(),
    )
    .unwrap();
    let out = dir.join(format!("{name}.mp4"));
    let cache = tempfile::tempdir().unwrap();
    let rendered = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env("SKYWRITE_CACHE_DIR", cache.path())
        .args([
            "edit",
            "render",
            edit.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    (
        edit.to_str().unwrap().to_owned(),
        out.to_str().unwrap().to_owned(),
    )
}

fn spans(report: &Value, property: &str) -> Vec<(f64, f64, Value, Value)> {
    check(report, property)["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["start_seconds"].as_f64().unwrap(),
                s["end_seconds"].as_f64().unwrap(),
                s["allowed"].clone(),
                s["fails"].clone(),
            )
        })
        .collect()
}

#[test]
fn the_edits_own_fades_and_stills_are_allowed_and_nothing_more() {
    let dir = tempfile::tempdir().unwrap();
    mp4(
        dir.path(),
        "cam.mp4",
        "testsrc2=s=160x90:r=30:d=6",
        "sine=frequency=440:sample_rate=48000:duration=6",
    );
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=1:d=1",
        "-frames:v",
        "1",
        dir.path().join("title.png").to_str().unwrap(),
    ]);
    // The same picture as a video clip, so a freeze runs on past the still.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-loop",
        "1",
        "-i",
        dir.path().join("title.png").to_str().unwrap(),
        "-t",
        "1.5",
        "-r",
        "30",
        "-c:v",
        "libx264",
        "-crf",
        "0",
        "-pix_fmt",
        "yuv420p",
        dir.path().join("hold.mp4").to_str().unwrap(),
    ]);
    // Fades through black at both ends and a 3-s title still.
    let (edit, out) = render(
        dir.path(),
        "fades",
        serde_json::json!([
            {"start":0,"source":"cam.mp4","duration":2,"fade_in":1},
            {"start":2,"source":"title.png","duration":3},
            {"start":5,"source":"cam.mp4","source_in":2,"duration":2,"fade_out":1}
        ]),
    );
    // Any black frame fails without the edit.
    let strict = ["--black", "--black-min-seconds", "0", "--frozen"];
    let (ok, report) = verify(&out, &strict);
    assert!(!ok, "{report}");
    assert_eq!(check(&report, "first_frame_black")["passed"], false);
    assert!(
        report["not_measured"].to_string().contains("--edit"),
        "{report}"
    );
    let black = spans(&report, "black_frames");
    assert_eq!(black.len(), 2, "{report}");
    assert!(
        black.iter().all(|s| s.2.is_null() && s.3 == true),
        "{report}"
    );
    let frozen = spans(&report, "frozen_frames");
    assert_eq!(frozen.len(), 1, "{report}");
    // With the edit, the fades and the still are its own.
    let mut with_edit = strict.to_vec();
    with_edit.extend(["--edit", &edit]);
    let (ok, report) = verify(&out, &with_edit);
    assert!(ok, "{report}");
    assert_eq!(check(&report, "first_frame_black")["allowed"], "fade_in");
    let black = spans(&report, "black_frames");
    assert_eq!(black[0].2, "fade_in", "{report}");
    assert_eq!(black[1].2, "fade_out", "{report}");
    assert!(black[0].1 <= 1.0 && black[1].0 >= 6.0, "{report}");
    let frozen = spans(&report, "frozen_frames");
    assert_eq!(
        (frozen[0].2.clone(), frozen[0].3.clone()),
        ("still".into(), false.into())
    );
    assert_eq!(
        report["edit_allowances"]["fade_in"],
        serde_json::json!([0.0, 1.0])
    );
    assert_eq!(
        report["edit_allowances"]["stills"],
        serde_json::json!([[2.0, 5.0]])
    );

    // A gap before the first clip is not its fade, and a freeze that runs
    // past the still is not the still.
    let (edit, out) = render(
        dir.path(),
        "late",
        serde_json::json!([
            {"start":0.5,"source":"cam.mp4","duration":1.5,"fade_in":1},
            {"start":2,"source":"title.png","duration":3},
            {"start":5,"source":"hold.mp4","duration":1.5}
        ]),
    );
    // The hold was encoded once more, so allow encoder noise.
    let (ok, report) = verify(
        &out,
        &[
            "--black",
            "--frozen",
            "--frozen-noise-db",
            "-30",
            "--edit",
            &edit,
        ],
    );
    assert!(!ok, "{report}");
    assert_eq!(
        check(&report, "first_frame_black")["passed"],
        false,
        "{report}"
    );
    let black = spans(&report, "black_frames");
    assert!(black[0].2.is_null() && black[0].3 == true, "{report}");
    let frozen = spans(&report, "frozen_frames");
    assert_eq!(frozen.len(), 1, "{report}");
    assert!(frozen[0].2.is_null() && frozen[0].3 == true, "{report}");
    assert!(frozen[0].1 > 6.4, "{report}");
}

#[test]
fn detector_times_are_from_the_container_start_in_mp4_and_mpeg_ts() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["late.mp4", "late.ts"] {
        let path = dir.path().join(name);
        ffmpeg(&[
            "-v",
            "error",
            "-y",
            "-itsoffset",
            "1",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=30:d=3,drawbox=c=black:t=fill:enable=between(t\\,1.5\\,1.9)",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            // FFmpeg before 9 otherwise fills the offset with copies of the
            // first frame in MP4, so the picture would start at 0.
            "-fps_mode",
            "passthrough",
            "-c:a",
            "aac",
            path.to_str().unwrap(),
        ]);
        let (_, report) = verify(
            path.to_str().unwrap(),
            &["--black", "--black-min-seconds", "0"],
        );
        let media = &report["media"];
        let video = media["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["codec_type"] == "video")
            .unwrap();
        let video_start =
            video["start_time"].as_f64().unwrap() - media["start_time"].as_f64().unwrap();
        let span = &check(&report, "black_frames")["spans"][0];
        assert!(
            (span["start_seconds"].as_f64().unwrap() - (video_start + 1.5)).abs() < 0.02,
            "{name}: {report}"
        );
        assert_eq!(
            check(&report, "first_frame_black")["passed"],
            true,
            "{name}"
        );
    }
}

#[test]
fn edit_allowances_follow_visible_clips_and_protect_the_edit() {
    let dir = tempfile::tempdir().unwrap();
    let cam = mp4(
        dir.path(),
        "cam.mp4",
        "testsrc2=s=160x90:r=30:d=6",
        "sine=duration=6",
    );
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=red:s=160x90",
        "-frames:v",
        "1",
        dir.path().join("title.png").to_str().unwrap(),
    ]);
    let allowances = |video: Value| {
        let path = dir.path().join("edit.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version":1,"width":160,"height":90,
                "fps":{"num":30,"den":1},"video":video}))
            .unwrap(),
        )
        .unwrap();
        let (_, report) = verify(&cam, &["--black", "--edit", path.to_str().unwrap()]);
        report["edit_allowances"].clone()
    };
    // An invisible still allows nothing; an opaque clip over a fading one ends
    // the fade window when it is fully shown.
    let a = allowances(serde_json::json!([
        {"clips":[{"start":0,"source":"cam.mp4","duration":6,"fade_in":1,"fade_out":1}]},
        {"clips":[{"start":0,"source":"title.png","duration":6,"opacity":0}]},
        {"clips":[{"start":0.3,"source":"cam.mp4","duration":5,"fade_in":0.2}]}
    ]));
    assert_eq!(a["stills"], serde_json::json!([]), "{a}");
    assert_eq!(a["fade_in"], serde_json::json!([0.0, 0.5]), "{a}");
    assert_eq!(a["fade_out"], serde_json::json!([5.3, 6.0]), "{a}");
    // A still allows a freeze only while nothing else is on screen.
    let a = allowances(serde_json::json!([
        {"clips":[{"start":0,"source":"title.png","duration":4}]},
        {"clips":[{"start":1,"source":"cam.mp4","duration":1}]}
    ]));
    assert_eq!(
        a["stills"],
        serde_json::json!([[0.0, 1.0], [2.0, 4.0]]),
        "{a}"
    );
    assert_eq!(a["fade_in"], Value::Null, "{a}");

    // The report never replaces the edit or its sources, including a source
    // the edit declares without using it.
    let edit = dir.path().join("edit.json");
    let spare = dir.path().join("spare.mp4");
    std::fs::copy(&cam, &spare).unwrap();
    let mut value: Value = serde_json::from_slice(&std::fs::read(&edit).unwrap()).unwrap();
    value["sources"] = serde_json::json!([{"path":"spare.mp4"}]);
    std::fs::write(&edit, serde_json::to_vec(&value).unwrap()).unwrap();
    for target in [edit.clone(), dir.path().join("title.png"), spare] {
        let before = std::fs::read(&target).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .args([
                "verify",
                &cam,
                "--edit",
                edit.to_str().unwrap(),
                "--overwrite",
                "-o",
            ])
            .arg(&target)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{}", target.display());
        assert!(String::from_utf8_lossy(&out.stderr).contains("overwrite an input"));
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }
    // A render does not replace an unused declared source either.
    let spare = dir.path().join("spare.mp4");
    let before = std::fs::read(&spare).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .args([
            "edit",
            "render",
            edit.to_str().unwrap(),
            "--overwrite",
            "-o",
        ])
        .arg(&spare)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(std::fs::read(&spare).unwrap(), before);
}
