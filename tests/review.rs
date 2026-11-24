//! Review aids against real decoded media.
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run(args: &[&str]) -> Output {
    let cache = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env("SKYWRITE_CACHE_DIR", cache.path())
        .args(args)
        .output()
        .unwrap()
}

fn skywrite(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// FFmpeg 6.0 added `-display_rotation`, which the rotated phone fixtures need;
/// older builds (Skywrite supports 5.1+) skip those cases.
fn display_rotation_supported() -> bool {
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-h", "full"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains("-display_rotation")
}

fn fixtures(dir: &Path) {
    let p = |n: &str| dir.join(n).to_str().unwrap().to_owned();
    // "Speaker": picture plus a quiet tone standing in for speech.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=red:s=160x90:r=30:d=6",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:sample_rate=48000:duration=6",
        "-af",
        "volume=0.8",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &p("talk.mp4"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=blue:s=160x90:r=30:d=6",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        &p("broll.mp4"),
    ]);
    // Loud continuous noise, like applause.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "anoisesrc=d=6:c=pink:a=0.5:r=48000",
        &p("applause.wav"),
    ]);
    // A loud music bed.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=500:sample_rate=48000:duration=6",
        "-af",
        "volume=7",
        &p("bed.wav"),
    ]);
    let transcript = json!({"version":1,"source":"talk.mp4","duration":6.0,"text":"we came to Evanston Evanston today",
        "usage":{"source_sha256":skywrite::media::file_sha256(&dir.join("talk.mp4")).unwrap()},
        "words":[{"text":"we","start":0.5,"end":0.7},{"text":"came","start":0.8,"end":1.0},{"text":"to","start":1.1,"end":1.2},
                 {"text":"Evanston","start":1.3,"end":1.7},{"text":"Evanston,","start":1.8,"end":2.2},{"text":"um","start":2.4,"end":2.6},
                 {"text":"today","start":2.8,"end":3.2}]});
    std::fs::write(
        dir.join("talk.transcript.json"),
        serde_json::to_vec(&transcript).unwrap(),
    )
    .unwrap();
}

fn write_edit(dir: &Path, name: &str, edit: Value) -> String {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec(&edit).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

fn kinds(report: &Value) -> Vec<String> {
    report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn lint_fingerprint(fingerprint: impl FnOnce(&Path) -> Option<String>) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("speech.wav");
    ffmpeg(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:sample_rate=16000:duration=1",
        media.to_str().unwrap(),
    ]);
    let mut transcript = json!({"version":1,"source":"speech.wav","duration":1.0,
        "text":"hello","words":[{"text":"hello","start":0.1,"end":0.5}]});
    if let Some(hash) = fingerprint(&media) {
        transcript["usage"] = json!({"source_sha256":hash});
    }
    let path = dir.path().join("words.json");
    std::fs::write(&path, serde_json::to_vec(&transcript).unwrap()).unwrap();
    let edit = write_edit(
        dir.path(),
        "edit.json",
        json!({"version":1,
        "width":160,"height":90,"fps":{"num":30,"den":1},
        "audio":[{"clips":[{"start":0,"source":"speech.wav","duration":1}]}]}),
    );
    run(&[
        "review",
        "lint",
        &edit,
        "--transcript",
        path.to_str().unwrap(),
        "--no-levels",
    ])
}

#[test]
fn lint_rejects_mismatched_transcript_fingerprint() {
    let out = lint_fingerprint(|_| Some("0".repeat(64)));
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("transcript source content has changed"),
        "{error}"
    );
    assert!(error.contains("transcribe"), "{error}");
}

#[test]
fn lint_rejects_missing_transcript_fingerprint() {
    let out = lint_fingerprint(|_| None);
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("no source fingerprint"), "{error}");
}

#[test]
fn lint_accepts_matching_transcript_fingerprint_case_insensitively() {
    let out =
        lint_fingerprint(|media| Some(skywrite::media::file_sha256(media).unwrap().to_uppercase()));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["checked"]["dialogue_words_heard"], 1);
    assert_eq!(
        report["checked"]["dialogue_sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// `(frames, timeline_start)` of every brief_shot warning for these tracks.
fn brief_shots(dir: &Path, name: &str, video: Value) -> Vec<(u64, f64)> {
    lint_shots(dir, name, video).0
}

/// Brief shots as in `brief_shots`, and `checked.brief_shots_not_checked` as
/// `(timeline_start, timeline_end)` in frames, at 30 fps.
#[allow(clippy::type_complexity)]
fn lint_shots(dir: &Path, name: &str, video: Value) -> (Vec<(u64, f64)>, Vec<(u64, u64)>) {
    let (num, den) = (30, 1);
    let rate = num as f64 / den as f64;
    let edit = write_edit(
        dir,
        name,
        json!({"version":1,"width":160,"height":90,"fps":{"num":num,"den":den},"video":video}),
    );
    let report = skywrite(&["review", "lint", &edit, "--no-levels"]);
    assert_eq!(report["checked"]["min_shot_seconds"], 0.5, "{report}");
    assert_eq!(
        report["checked"]["uncertain_cover_sources"],
        json!([]),
        "{report}"
    );
    let brief = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["kind"] == "brief_shot")
        .map(|w| {
            let start = w["timeline_start"].as_f64().unwrap();
            let end = w["timeline_end"].as_f64().unwrap();
            let frames = w["frames"].as_u64().unwrap();
            assert!((end - start - frames as f64 / rate).abs() < 1e-9, "{w}");
            (frames, (start * rate).round() / rate)
        })
        .collect();
    let frame = |v: &Value| (v.as_f64().unwrap() * rate).round() as u64;
    let not_checked = report["checked"]["brief_shots_not_checked"]
        .as_array()
        .unwrap_or_else(|| panic!("{report}"))
        .iter()
        .map(|s| (frame(&s["timeline_start"]), frame(&s["timeline_end"])))
        .collect();
    (brief, not_checked)
}

#[test]
fn lint_reports_brief_shots_but_not_continuous_or_hidden_cuts() {
    let dir = tempfile::tempdir().unwrap();
    fixtures(dir.path());
    let d = dir.path();
    let talk = |start: f64, source_in: f64, duration: f64| json!({"start":start,"source":"talk.mp4","source_in":source_in,"duration":duration});
    let broll = |start: f64, source_in: f64, duration: f64| json!({"start":start,"source":"broll.mp4","source_in":source_in,"duration":duration});
    let at = |frame: f64| frame / 30.0;

    // Two B-roll shots 2 frames apart over one talking take: the speaker flashes.
    let flash = json!([{"clips":[talk(0.0, 0.0, 4.0)]},
        {"clips":[broll(1.0, 0.0, 1.0), broll(2.0667, 2.0, 0.9333)]}]);
    assert_eq!(brief_shots(d, "flash.json", flash.clone()), [(2, 2.0)]);

    // A straight split 9 frames after a B-roll shot ends continues the take.
    let split = json!([{"clips":[talk(0.0, 0.0, 1.2), talk(1.2, 1.2, 2.8)]},
        {"clips":[broll(0.0, 0.0, 0.9)]}]);
    assert_eq!(brief_shots(d, "split.json", split), []);
    // A jump cut 6 frames into opaque full-frame B-roll is hidden.
    let hidden = json!([{"clips":[talk(0.0, 0.0, 1.2), talk(1.2, 3.0, 2.8)]},
        {"clips":[broll(1.0, 0.0, 1.0)]}]);
    assert_eq!(brief_shots(d, "hidden.json", hidden), []);

    // A 0.4 s cross-dissolve is not checked; a hard 2-frame flash beside it
    // is still a brief shot.
    let mut out = talk(0.0, 0.0, 2.2);
    out["fade_out"] = json!(0.4);
    let mut into = broll(1.8, 0.0, 2.2);
    into["fade_in"] = json!(0.4);
    let dissolve = json!([{"clips":[out]},{"clips":[into]}]);
    assert_eq!(brief_shots(d, "dissolve.json", dissolve.clone()), []);
    let mut beside = dissolve;
    beside
        .as_array_mut()
        .unwrap()
        .push(json!({"clips":[talk(3.0, 0.0, 0.0667)]}));
    assert_eq!(brief_shots(d, "beside.json", beside), [(2, 3.0)]);

    // 2-frame shots at either end of the timeline are brief (black lies
    // beyond the ends).
    let ends = json!([{"clips":[talk(0.0, 0.0, 4.0)]},{"clips":[broll(0.0667, 0.0, 3.8667)]}]);
    assert_eq!(
        brief_shots(d, "ends.json", ends),
        [(2, 0.0), (2, at(118.0))]
    );

    // A 2-frame hole in the only track flashes black.
    let gap = json!([{"clips":[talk(0.0, 0.0, 2.0), talk(2.0667, 3.0, 1.9333)]}]);
    assert_eq!(brief_shots(d, "black.json", gap), [(2, 2.0)]);

    // A 2-frame overlay at opacity 0.3 is a visible flash; at 0.2 it is under
    // a quarter of the picture.
    for (opacity, expected) in [(0.3, vec![(2, 2.0)]), (0.2, vec![])] {
        let mut faint = broll(2.0, 0.0, 0.0667);
        faint["opacity"] = json!(opacity);
        let faint = json!([{"clips":[talk(0.0, 0.0, 4.0)]},{"clips":[faint]}]);
        assert_eq!(brief_shots(d, "faint.json", faint), expected, "{opacity}");
    }
    // Stills and other overlays that do not cover the frame never dim what is
    // beneath, so a partial-opacity one needs about twice the opacity to count:
    // a 2-frame PNG at 0.3 is quiet, at 0.6 it is a flash.
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=blue:s=160x90",
        "-frames:v",
        "1",
        d.join("blue.png").to_str().unwrap(),
    ]);
    for (opacity, expected) in [(0.3, vec![]), (0.6, vec![(2, 2.0)])] {
        let still = json!({"start":2.0,"source":"blue.png","duration":0.0667,"opacity":opacity});
        let still = json!([{"clips":[talk(0.0, 0.0, 4.0)]},{"clips":[still]}]);
        assert_eq!(brief_shots(d, "still.json", still), expected, "{opacity}");
    }
    // An opacity-0 piece inside a split upper take shows the base for 2 frames.
    let mut gap = broll(1.0, 1.0, 0.0667);
    gap["opacity"] = json!(0);
    let seam = json!([{"clips":[talk(0.0, 0.0, 4.0)]},
        {"clips":[broll(0.0, 0.0, 1.0), gap, broll(1.0667, 1.0667, 2.9333)]}]);
    assert_eq!(brief_shots(d, "seam.json", seam), [(2, 1.0)]);

    // Fast B-roll jumping back 0.25 s into a slow clip is a cut: the slower
    // clip's source step bounds continuity, not the faster one's.
    let mut fast = broll(1.0, 0.0, 0.5);
    fast["speed"] = json!(8);
    let mut slow = broll(1.5, 3.75, 0.0667);
    slow["speed"] = json!(0.125);
    let jump = json!([{"clips":[talk(0.0, 0.0, 4.0)]},{"clips":[fast, slow]}]);
    assert_eq!(brief_shots(d, "jump.json", jump), [(2, 1.5)]);
    // Frame rounding decides: authored contiguous for 0.51 s at 4x, the first
    // clip rounds to 0.5 s and leaves a 0.04 s source gap, more than one frame
    // of the slower 1x clip, so the seam is a cut.
    let mut rounded = broll(1.0, 0.0, 0.51);
    rounded["speed"] = json!(4);
    let rounding = json!([{"clips":[talk(0.0, 0.0, 4.0)]},
        {"clips":[rounded, broll(1.5, 2.04, 0.0667)]}]);
    assert_eq!(brief_shots(d, "rounding.json", rounding), [(2, 1.5)]);

    // Razor all tracks: cuts under a continuous opaque take are hidden, even at
    // the take's own seams; a real upper cut at the same frames is a change.
    let lower =
        json!({"clips":[talk(0.0, 0.0, 1.0), talk(1.0, 3.0, 0.0667), talk(1.0667, 0.5, 2.9333)]});
    let razor = json!([lower.clone(),
        {"clips":[broll(0.0, 0.0, 1.0), broll(1.0, 1.0, 0.0667), broll(1.0667, 1.0667, 2.9333)]}]);
    assert_eq!(brief_shots(d, "razor.json", razor), []);
    let recut = json!([lower,
        {"clips":[broll(0.0, 0.0, 1.0), broll(1.0, 3.0, 0.0667), broll(1.0667, 1.0667, 2.9333)]}]);
    assert_eq!(brief_shots(d, "recut.json", recut), [(2, 1.0)]);

    // Full-frame video with an alpha channel never hides a cut beneath it,
    // whether its pixels are transparent or (undetectably) all opaque.
    for (name, color) in [("clear.mov", "white@0.0"), ("solid.mov", "white@1.0")] {
        ffmpeg(&[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("color={color}:s=160x90:r=30:d=4,format=argb"),
            "-c:v",
            "qtrle",
            d.join(name).to_str().unwrap(),
        ]);
        let under = json!([{"clips":[talk(0.0, 0.0, 1.0), broll(1.0, 0.0, 0.0667), talk(1.0667, 1.0667, 2.9333)]},
            {"clips":[{"start":0,"source":name,"duration":4}]}]);
        assert_eq!(brief_shots(d, &format!("{name}.json"), under), [(2, 1.0)]);
    }

    // A threshold of zero turns the check off; values outside 0..10 are refused
    // by the validator (the `=` form reaches it for negative numbers).
    let edit = write_edit(
        d,
        "flash-args.json",
        json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},"video":flash}),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &edit,
        "--no-levels",
        "--min-shot-seconds",
        "0",
    ]);
    assert!(
        !kinds(&report).contains(&"brief_shot".to_owned()),
        "{report}"
    );
    assert_eq!(report["checked"]["min_shot_seconds"], 0.0);
    for bad in ["=-1", "=11", "=NaN"] {
        let out = run(&[
            "review",
            "lint",
            &edit,
            "--no-levels",
            &format!("--min-shot-seconds{bad}"),
        ]);
        assert!(!out.status.success(), "{bad}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("must be 0..10"), "{bad}: {stderr}");
    }
}

#[test]
fn lint_does_not_check_brief_shots_at_fades_or_dissolves() {
    let dir = tempfile::tempdir().unwrap();
    fixtures(dir.path());
    let d = dir.path();
    let clip = |source: &str, start: f64, source_in: f64, duration: f64| json!({"start":start,"source":source,"source_in":source_in,"duration":duration});
    let talk =
        |start: f64, source_in: f64, duration: f64| clip("talk.mp4", start, source_in, duration);
    let broll =
        |start: f64, source_in: f64, duration: f64| clip("broll.mp4", start, source_in, duration);
    let with = |mut clip: Value, key: &str, value: f64| {
        clip[key] = json!(value);
        clip
    };
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=blue:s=160x90",
        "-frames:v",
        "1",
        d.join("card.png").to_str().unwrap(),
    ]);

    // The 2-frame speaker flash between B-roll shots, hard cut: reported.
    let base = json!({"clips":[talk(0.0, 0.0, 4.0)]});
    let hard = json!([base, {"clips":[broll(1.0, 0.0, 1.0), broll(2.0667, 2.0, 0.9333)]}]);
    assert_eq!(lint_shots(d, "hard.json", hard), (vec![(2, 2.0)], vec![]));
    // The same flash with the second B-roll dissolving in: not checked.
    let dissolve = json!([base,
        {"clips":[broll(1.0, 0.0, 1.0), with(broll(2.0667, 2.0, 0.9333), "fade_in", 0.2)]}]);
    assert_eq!(
        lint_shots(d, "dissolve.json", dissolve),
        (vec![], vec![(60, 62)])
    );
    // ... or the first one fading out into it.
    let fade_out = json!([base,
        {"clips":[with(broll(1.0, 0.0, 1.0), "fade_out", 0.1), broll(2.0667, 2.0, 0.9333)]}]);
    assert_eq!(
        lint_shots(d, "fade-out.json", fade_out),
        (vec![], vec![(60, 62)])
    );

    // A 2-frame overlay with 1-frame fades over a long opening fade.
    let overlay = with(
        with(broll(3.0, 0.0, 0.0667), "fade_in", 0.0333),
        "fade_out",
        0.0333,
    );
    let faded = json!([{"clips":[with(talk(0.0, 0.0, 4.0), "fade_in", 1.9)]},{"clips":[overlay]}]);
    assert_eq!(lint_shots(d, "faded.json", faded), (vec![], vec![(90, 92)]));

    // A quick fade through a 2-frame black gap (0.1 s out and in).
    let through = json!([{"clips":[with(talk(0.0, 0.0, 2.0), "fade_out", 0.1),
        with(talk(2.0667, 3.0, 1.9333), "fade_in", 0.1)]}]);
    assert_eq!(
        lint_shots(d, "through-black.json", through),
        (vec![], vec![(60, 62)])
    );

    // Review round 4: a short shot between two longer dissolves (B fades in
    // over A for 0.2 s, C over B for 0.9 s), and a 12-frame card with 0.1 s
    // fades over a still.
    let chain = json!([{"clips":[talk(0.0, 0.0, 1.2)]},
        {"clips":[with(broll(1.0, 0.0, 1.2), "fade_in", 0.2)]},
        {"clips":[with(talk(1.3, 2.0, 2.7), "fade_in", 0.9)]}]);
    assert_eq!(lint_shots(d, "chain.json", chain), (vec![], vec![(30, 39)]));
    let card = with(
        with(clip("card.png", 1.0, 0.0, 0.4), "fade_in", 0.1),
        "fade_out",
        0.1,
    );
    let still = json!([{"clips":[clip("card.png", 0.0, 0.0, 4.0)]},{"clips":[card]}]);
    assert_eq!(lint_shots(d, "card.json", still), (vec![], vec![(30, 42)]));

    // A take stepping to opacity 0.4 for 2 frames is an opacity change, not a
    // hard cut.
    let step = json!([base, {"clips":[broll(0.0, 0.0, 1.0),
        with(broll(1.0, 1.0, 0.0667), "opacity", 0.4), broll(1.0667, 1.0667, 2.9333)]}]);
    assert_eq!(lint_shots(d, "step.json", step), (vec![], vec![(30, 32)]));

    // A fade far from the flash does not stop it being checked.
    let far = json!([{"clips":[with(talk(0.0, 0.0, 4.0), "fade_out", 0.5)]},
        {"clips":[with(broll(0.0, 0.0, 1.0), "fade_in", 0.3), broll(1.0667, 1.0667, 2.0)]}]);
    assert_eq!(lint_shots(d, "far.json", far), (vec![(2, 1.0)], vec![]));
}

#[test]
fn lint_help_explains_brief_shots_for_agents() {
    let out = run(&["review", "lint", "--help"]);
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for fact in [
        "brief_shot",
        "--min-shot-seconds <SECONDS>",
        "0 to 10 (default 0.5); 0 turns the brief-shot",
        "fades and dissolves are not checked yet",
        "checked only between hard cuts",
        "not pixels",
        "half the sum of their weight differences",
        "its length is exact from the edit",
        "brief_shots_not_checked",
        "about twice as opaque to count",
        "or a black gap",
        "opacity 0",
        "straight split continuing the same take",
        "uncertain_cover_sources",
        "alpha-capable video",
        "timeline_start",
        "never changes the edit",
        "skywrite review lint edit.json --min-shot-seconds 0",
    ] {
        assert!(help.contains(fact), "missing {fact:?} in:\n{help}");
    }
}

#[test]
fn lint_reports_objective_problems_and_stays_quiet_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    fixtures(dir.path());
    let transcript = dir.path().join("talk.transcript.json");
    let transcript = transcript.to_str().unwrap();
    let base = |video: Value, audio: Value| json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},"video":video,"audio":audio});

    // Music over a muted, visibly speaking presenter (Ralph v1b's opening).
    let silent = write_edit(
        dir.path(),
        "silent.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5,"audio":{"volume":0}}]}]),
            json!([{"clips":[{"start":0,"source":"bed.wav","duration":3.5,"volume":0.2}]}]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &silent,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    assert!(
        kinds(&report).contains(&"visible_speaker_without_dialogue".to_owned()),
        "{report}"
    );

    // Covering the speaker with opaque B-roll for the whole span removes it.
    let covered = write_edit(
        dir.path(),
        "covered.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5,"audio":{"volume":0}}]},
               {"clips":[{"start":0,"source":"broll.mp4","duration":3.5,"audio":{"volume":0}}]}]),
            json!([{"clips":[{"start":0,"source":"bed.wav","duration":3.5,"volume":0.2}]}]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &covered,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    assert!(
        !kinds(&report).contains(&"visible_speaker_without_dialogue".to_owned()),
        "{report}"
    );

    // Audible dialogue with a stumble and a filler.
    let talking = write_edit(
        dir.path(),
        "talking.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5}]}]),
            json!([]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &talking,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    let found = kinds(&report);
    assert!(
        found.contains(&"repeated_word_in_select".to_owned()),
        "{report}"
    );
    assert!(found.contains(&"filler_words".to_owned()), "{report}");
    assert!(
        !found.contains(&"visible_speaker_without_dialogue".to_owned()),
        "{report}"
    );

    // A repeat joined by a cut is reported as such.
    let joined = write_edit(
        dir.path(),
        "joined.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","source_in":0.4,"duration":1.4},
                         {"start":1.4,"source":"talk.mp4","source_in":1.75,"duration":0.5}]}]),
            json!([]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &joined,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    assert!(
        kinds(&report).contains(&"repeated_word_across_cut".to_owned()),
        "{report}"
    );

    // Phone B-roll stored 90x160 with a quarter-turn rotation shows 160x90, so
    // it covers the speaker as fully as landscape B-roll.
    if display_rotation_supported() {
        let p = |n: &str| dir.path().join(n).to_str().unwrap().to_owned();
        ffmpeg(&[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=green:s=90x160:r=30:d=4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            &p("stored.mp4"),
        ]);
        ffmpeg(&[
            "-v",
            "error",
            "-y",
            "-display_rotation",
            "90",
            "-i",
            &p("stored.mp4"),
            "-c",
            "copy",
            &p("phone.mp4"),
        ]);
        let phone = write_edit(
            dir.path(),
            "phone.json",
            base(
                json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5,"audio":{"volume":0}}]},
                   {"clips":[{"start":0,"source":"phone.mp4","duration":3.5}]}]),
                json!([{"clips":[{"start":0,"source":"bed.wav","duration":3.5,"volume":0.2}]}]),
            ),
        );
        let report = skywrite(&[
            "review",
            "lint",
            &phone,
            "--transcript",
            transcript,
            "--no-levels",
        ]);
        assert!(
            !kinds(&report).contains(&"visible_speaker_without_dialogue".to_owned()),
            "{report}"
        );
    } else {
        eprintln!("skipping the rotated phone B-roll case: FFmpeg lacks -display_rotation (6.0+)");
    }

    // A cut inside a word is flagged; a straight split through the same word is not.
    let clipped = write_edit(
        dir.path(),
        "clipped.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","source_in":0.4,"duration":1.1},
                         {"start":1.1,"source":"talk.mp4","source_in":2.7,"duration":0.6}]}]),
            json!([]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &clipped,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    let items: Vec<&Value> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["kind"] == "word_clipped_at_cut")
        .flat_map(|w| w["items"].as_array().unwrap())
        .collect();
    assert_eq!(items.len(), 1, "{report}");
    assert_eq!(items[0]["word"], "Evanston", "{report}");
    assert_eq!(items[0]["edge"], "out", "{report}");
    let split = write_edit(
        dir.path(),
        "split.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","source_in":0.4,"duration":1.1},
                         {"start":1.1,"source":"talk.mp4","source_in":1.5,"duration":0.9}]}]),
            json!([]),
        ),
    );
    let report = skywrite(&[
        "review",
        "lint",
        &split,
        "--transcript",
        transcript,
        "--no-levels",
    ]);
    assert!(
        !kinds(&report).contains(&"word_clipped_at_cut".to_owned()),
        "{report}"
    );

    // Applause cut off abruptly, and a bed as loud as the dialogue.
    let mix = write_edit(
        dir.path(),
        "mix.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5,"audio":{"fade_out":0.3}}]}]),
            json!([{"clips":[{"start":3.3,"source":"applause.wav","duration":1}]},
               {"clips":[{"start":0,"source":"bed.wav","duration":3.5,"fade_out":0.3}]}]),
        ),
    );
    let report = skywrite(&["review", "lint", &mix, "--transcript", transcript]);
    let found = kinds(&report);
    assert!(found.contains(&"sound_cut_mid_flow".to_owned()), "{report}");
    assert!(
        found.contains(&"bed_close_to_dialogue".to_owned()),
        "{report}"
    );

    // A faded release and a ducked bed are not flagged.
    let fixed = write_edit(
        dir.path(),
        "fixed.json",
        base(
            json!([{"clips":[{"start":0,"source":"talk.mp4","duration":3.5,"audio":{"fade_out":0.3}}]}]),
            json!([{"clips":[{"start":3.3,"source":"applause.wav","duration":1,"fade_out":0.8}]},
               {"clips":[{"start":0,"source":"bed.wav","duration":3.5,"volume":0.01}]}]),
        ),
    );
    let report = skywrite(&["review", "lint", &fixed, "--transcript", transcript]);
    let found = kinds(&report);
    assert!(
        !found.contains(&"sound_cut_mid_flow".to_owned()),
        "{report}"
    );
    assert!(
        !found.contains(&"bed_close_to_dialogue".to_owned()),
        "{report}"
    );
}

#[test]
fn cut_auditions_cover_every_cut() {
    let dir = tempfile::tempdir().unwrap();
    fixtures(dir.path());
    let edit = write_edit(
        dir.path(),
        "edit.json",
        json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},
        "video":[{"clips":[{"start":0,"source":"talk.mp4","duration":2},{"start":2,"source":"talk.mp4","source_in":3,"duration":2}]},
                 {"clips":[{"start":1,"source":"broll.mp4","duration":0.5,"audio":{"volume":0}}]}]}),
    );
    let out = dir.path().join("cuts");
    let report = skywrite(&[
        "review",
        "cuts",
        &edit,
        "--handles",
        "0.5",
        "--output-dir",
        out.to_str().unwrap(),
    ]);
    assert_eq!(report["cut_count"], 3);
    for cut in report["cuts"].as_array().unwrap() {
        let clip = cut["clip"].as_str().unwrap();
        let info = skywrite::media::probe(Path::new(clip)).unwrap();
        assert!(info.has_audio() && info.has_video());
        assert!(
            info.duration > 0.8 && info.duration < 1.1,
            "{}",
            info.duration
        );
    }
    // The directory must be new.
    let again = run(&[
        "review",
        "cuts",
        &edit,
        "--output-dir",
        out.to_str().unwrap(),
    ]);
    assert!(!again.status.success());
    // No temporary render is left behind.
    assert!(
        std::fs::read_dir(&out).unwrap().all(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("cut-"))
    );
}

#[test]
fn contact_sheets_use_exact_consecutive_frames() {
    let dir = tempfile::tempdir().unwrap();
    fixtures(dir.path());
    let sheet = dir.path().join("sheet.jpg");
    let report = skywrite(&[
        "review",
        "sheet",
        dir.path().join("talk.mp4").to_str().unwrap(),
        "--start",
        "1",
        "--duration",
        "0.5",
        "--every",
        "2",
        "-o",
        sheet.to_str().unwrap(),
    ]);
    let tiles = report["tiles"].as_array().unwrap();
    assert_eq!(tiles.len(), 8);
    assert_eq!(tiles[0]["frame"], 30);
    assert_eq!(tiles[1]["frame"], 32);
    assert!((tiles[1]["source_time"].as_f64().unwrap() - (1.0 + 2.0 / 30.0)).abs() < 0.002);
    assert!(sheet.is_file());
    let too_many = run(&[
        "review",
        "sheet",
        dir.path().join("talk.mp4").to_str().unwrap(),
        "--duration",
        "5",
        "-o",
        dir.path().join("big.jpg").to_str().unwrap(),
    ]);
    assert!(!too_many.status.success());
}

#[test]
fn contact_sheet_tiles_of_rotated_phone_footage_are_upright() {
    if !display_rotation_supported() {
        eprintln!(
            "skipping contact_sheet_tiles_of_rotated_phone_footage_are_upright: FFmpeg lacks -display_rotation (6.0+)"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_owned();
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=green:s=160x90:r=30:d=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        &p("stored.mp4"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-display_rotation",
        "90",
        "-i",
        &p("stored.mp4"),
        "-c",
        "copy",
        &p("phone.mp4"),
    ]);
    skywrite(&[
        "review",
        "sheet",
        &p("phone.mp4"),
        "--duration",
        "0.1",
        "--every",
        "3",
        "--tile-width",
        "90",
        "-o",
        &p("sheet.jpg"),
    ]);
    // Shown 90x160: one 90-wide tile is 160 tall, plus its 28-pixel label.
    let size = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=width,height"])
        .args(["-of", "csv=p=0", &p("sheet.jpg")])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&size.stdout).trim(), "90,188");
}

#[test]
fn interval_sampling_keeps_one_frame_per_vfr_bucket_across_gaps_and_seeks() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("vfr.mkv");
    // Timestamps: 0, .05, .10, 1.35, 1.40, 3.20, 3.25, 4, 4.05.
    // Bucket 2 is empty, and each occupied bucket has a dense burst.
    ffmpeg(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=100:d=0.09",
        "-vf",
        "setpts='if(lt(N,3),N*5,if(lt(N,5),135+(N-3)*5,if(lt(N,7),320+(N-5)*5,400+(N-7)*5)))'",
        "-fps_mode",
        "vfr",
        "-c:v",
        "ffv1",
        source.to_str().unwrap(),
    ]);
    let info = skywrite::media::probe_for_timing(&source).unwrap();
    let extract = |start, duration| {
        let output = tempfile::tempdir_in(dir.path()).unwrap();
        skywrite::review::extract_frames(
            &info,
            &skywrite::review::FrameSelection {
                start,
                duration,
                every: None,
                interval: Some(1.0),
                width: 160,
            },
            output.path(),
            48,
        )
        .unwrap()
        .into_iter()
        .map(|frame| frame.time)
        .collect::<Vec<_>>()
    };
    let full = extract(0.0, info.duration);
    assert_eq!(full, vec![0.0, 1.35, 3.2, 4.0]);
    // A seek before the first frame after a gap must still select that frame.
    assert_eq!(extract(1.2, 2.4), vec![1.35, 3.2]);
    // A seek after a bucket's first frame must not choose a later frame from it.
    assert_eq!(extract(1.37, 2.23), vec![3.2]);
}

#[test]
fn interval_sampling_keeps_the_cfr_source_grid_after_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("cfr.mp4");
    ffmpeg(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x90:r=30:d=4",
        "-c:v",
        "libx264",
        source.to_str().unwrap(),
    ]);
    let info = skywrite::media::probe_for_timing(&source).unwrap();
    for (start, duration, interval, expected) in [
        (0.0, 4.0, 1.0, vec![0.0, 1.0, 2.0, 3.0]),
        (1.0, 2.5, 1.0, vec![1.0, 2.0, 3.0]),
        (1.02, 2.48, 1.0, vec![2.0, 3.0]),
        // The frame at 1/3s falls just before this interval boundary, within
        // the selector's tolerance. Seeking must not replace it with 11/30s.
        (
            0.0,
            1.1,
            0.3333335,
            vec![0.0, 0.333333333, 0.666666667, 1.0],
        ),
        (0.34, 0.76, 0.3333335, vec![0.666666667, 1.0]),
    ] {
        let output = tempfile::tempdir_in(dir.path()).unwrap();
        let frames = skywrite::review::extract_frames(
            &info,
            &skywrite::review::FrameSelection {
                start,
                duration,
                every: None,
                interval: Some(interval),
                width: 160,
            },
            output.path(),
            48,
        )
        .unwrap();
        assert_eq!(
            frames
                .into_iter()
                .map(|frame| frame.time)
                .collect::<Vec<_>>(),
            expected
        );
    }
}
