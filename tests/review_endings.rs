//! `review lint` unfinished endings and dead air, on generated speech stand-ins.
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn lint(dir: &Path, edit: Value, transcript: &str, extra: &[&str]) -> Value {
    let path = dir.join("edit.json");
    std::fs::write(&path, serde_json::to_vec(&edit).unwrap()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut args = vec![
        "review",
        "lint",
        path.to_str().unwrap(),
        "--transcript",
        transcript,
        "--no-levels",
    ];
    args.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env("SKYWRITE_CACHE_DIR", cache.path())
        .args(&args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn warning<'a>(report: &'a Value, kind: &str) -> Option<&'a Value> {
    report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == kind)
}

fn reasons(report: &Value) -> Vec<String> {
    warning(report, "unfinished_ending").map_or_else(Vec::new, |w| {
        w["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["reason"].as_str().unwrap().to_owned())
            .collect()
    })
}

/// Speech stand-in plus a transcript of `words` (text, start, end).
fn speech(dir: &Path, name: &str, words: &[(&str, f64, f64)]) -> String {
    let media = dir.join(format!("{name}.wav"));
    ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:sample_rate=16000:duration=8",
        media.to_str().unwrap(),
    ]);
    let words: Vec<Value> = words
        .iter()
        .map(|(text, start, end)| json!({"text":text,"start":start,"end":end}))
        .collect();
    let transcript = json!({"version":1,"source":format!("{name}.wav"),"duration":8.0,"text":"",
        "usage":{"source_sha256":skywrite::media::file_sha256(&media).unwrap()},"words":words});
    let path = dir.join(format!("{name}.transcript.json"));
    std::fs::write(&path, serde_json::to_vec(&transcript).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

fn edit(clips: Value) -> Value {
    json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},"audio":[{"clips":clips}]})
}

const PLAN: &[(&str, f64, f64)] = &[
    ("Here's", 0.2, 0.4),
    ("the", 0.45, 0.55),
    ("plan.", 0.6, 0.9),
    // 0.7 s pause.
    ("We", 1.6, 1.8),
    ("start", 1.85, 2.1),
    ("early,", 2.15, 2.5),
    ("and", 2.55, 2.7),
    ("we", 2.75, 2.9),
    ("finish", 2.95, 3.3),
    ("late.", 3.35, 3.7),
    // 0.6 s pause.
    ("Done.", 4.3, 4.6),
    ("But", 4.7, 4.9),
    ("wait", 4.95, 5.3),
];

#[test]
fn unfinished_endings_name_the_cut_off_words() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(dir.path(), "plan", PLAN);
    let upto = |seconds: f64| edit(json!([{"start":0,"source":"plan.wav","duration":seconds}]));

    // Mid-sentence: no final punctuation and the next source word joins on.
    let report = lint(dir.path(), upto(2.53), &t, &[]);
    assert_eq!(
        reasons(&report),
        ["no_sentence_final_punctuation", "next_word_continues"],
        "{report}"
    );
    let w = warning(&report, "unfinished_ending").unwrap();
    assert_eq!(w["next_source_words"], "and we finish late. Done. But");
    assert!(w["last_words"].as_str().unwrap().ends_with("start early,"));
    assert!((w["timeline_end"].as_f64().unwrap() - 2.5).abs() < 1e-6);
    assert!((w["source_seconds"].as_f64().unwrap() - 2.5).abs() < 1e-6);

    // A finished sentence followed by a new one is not flagged.
    let report = lint(dir.path(), upto(3.8), &t, &[]);
    assert!(warning(&report, "unfinished_ending").is_none(), "{report}");
    assert_eq!(report["checked"]["ending"]["last_word"], "late.");
    assert_eq!(report["checked"]["ending"]["punctuation"], "judged");

    // A finished sentence whose next source sentence starts with "But".
    let report = lint(dir.path(), upto(4.65), &t, &[]);
    assert_eq!(reasons(&report), ["next_word_continues"], "{report}");

    // The source's last word, unpunctuated.
    let report = lint(dir.path(), upto(5.5), &t, &[]);
    assert_eq!(
        reasons(&report),
        ["no_sentence_final_punctuation"],
        "{report}"
    );

    // Without punctuation in the transcript, only the next word is judged.
    let bare: Vec<(String, f64, f64)> = PLAN
        .iter()
        .map(|(w, s, e)| (w.trim_end_matches(['.', ',']).to_owned(), *s, *e))
        .collect();
    let bare: Vec<(&str, f64, f64)> = bare.iter().map(|(w, s, e)| (w.as_str(), *s, *e)).collect();
    let t2 = speech(dir.path(), "bare", &bare);
    let bare_edit =
        |seconds: f64| edit(json!([{"start":0,"source":"bare.wav","duration":seconds}]));
    let report = lint(dir.path(), bare_edit(3.8), &t2, &[]);
    assert!(warning(&report, "unfinished_ending").is_none(), "{report}");
    assert!(
        report["checked"]["ending"]["punctuation"]
            .as_str()
            .unwrap()
            .starts_with("not judged")
    );
    let report = lint(dir.path(), bare_edit(2.53), &t2, &[]);
    assert_eq!(reasons(&report), ["next_word_continues"], "{report}");
}

#[test]
fn open_counted_lists_are_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(
        dir.path(),
        "list",
        &[
            ("I", 0.1, 0.2),
            ("have", 0.25, 0.4),
            ("three", 0.45, 0.7),
            ("reasons.", 0.75, 1.1),
            ("First,", 1.3, 1.6),
            ("cost.", 1.65, 2.0),
            ("Second,", 2.2, 2.6),
            ("speed.", 2.65, 3.0),
            ("Third,", 3.2, 3.6),
            ("safety.", 3.65, 4.0),
        ],
    );
    let upto = |seconds: f64| edit(json!([{"start":0,"source":"list.wav","duration":seconds}]));
    let report = lint(dir.path(), upto(3.1), &t, &[]);
    let w = warning(&report, "unfinished_ending").expect("open list");
    let list = &w["reasons"][0];
    assert_eq!(list["reason"], "open_list", "{report}");
    assert_eq!(list["announced"], 3);
    assert_eq!(list["reached"], 2);
    let report = lint(dir.path(), upto(4.1), &t, &[]);
    assert!(warning(&report, "unfinished_ending").is_none(), "{report}");

    // Without an announced count, the next ordinal after the cut opens it.
    let t = speech(
        dir.path(),
        "ordinals",
        &[
            ("First,", 0.1, 0.5),
            ("cost.", 0.55, 0.9),
            ("Second,", 1.1, 1.5),
            ("speed.", 1.55, 1.9),
            ("Third,", 2.1, 2.5),
            ("safety.", 2.55, 2.9),
        ],
    );
    let report = lint(
        dir.path(),
        edit(json!([{"start":0,"source":"ordinals.wav","duration":2.0}])),
        &t,
        &[],
    );
    let w = warning(&report, "unfinished_ending").expect("open ordinals");
    assert_eq!(w["reasons"][0]["reason"], "open_list", "{report}");
    assert_eq!(w["reasons"][0]["next_item_word"], "Third,");
}

fn gaps(report: &Value) -> Vec<f64> {
    warning(report, "dead_air").map_or_else(Vec::new, |w| {
        w["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["seconds"].as_f64().unwrap() * 1000.0).round() / 1000.0)
            .collect()
    })
}

#[test]
fn dead_air_is_measured_on_the_timeline_at_and_past_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(dir.path(), "plan", PLAN);
    let whole = edit(json!([{"start":0,"source":"plan.wav","duration":5.5}]));
    // Default 0.6 s: the 0.7 s pause is flagged, the 0.6 s one is not.
    let report = lint(dir.path(), whole.clone(), &t, &[]);
    assert_eq!(gaps(&report), [0.7], "{report}");
    let w = warning(&report, "dead_air").unwrap();
    assert_eq!(w["items"][0]["before"], "plan.");
    assert_eq!(w["items"][0]["after"], "We");
    assert_eq!(w["items"][0]["across_cut"], false);
    assert_eq!(report["checked"]["dead_air"]["threshold_seconds"], 0.6);
    // Just under each pause flags it; just over does not.
    let report = lint(
        dir.path(),
        whole.clone(),
        &t,
        &["--dead-air-seconds", "0.59"],
    );
    assert_eq!(gaps(&report), [0.7, 0.6], "{report}");
    let report = lint(
        dir.path(),
        whole.clone(),
        &t,
        &["--dead-air-seconds", "0.69"],
    );
    assert_eq!(gaps(&report), [0.7], "{report}");
    let report = lint(
        dir.path(),
        whole.clone(),
        &t,
        &["--dead-air-seconds", "0.71"],
    );
    assert!(warning(&report, "dead_air").is_none(), "{report}");

    // A cut that opens a gap on the timeline, and a slowed clip, are measured
    // after the cut and the speed change.
    let cut = edit(json!([
        {"start":0,"source":"plan.wav","duration":0.95},
        {"start":1.9,"source":"plan.wav","source_in":2.9,"duration":0.9}
    ]));
    let report = lint(dir.path(), cut.clone(), &t, &[]);
    assert_eq!(gaps(&report), [1.05], "{report}");
    assert_eq!(
        warning(&report, "dead_air").unwrap()["items"][0]["across_cut"],
        true
    );
    let slow =
        edit(json!([{"start":0,"source":"plan.wav","source_in":3.3,"duration":2.4,"speed":0.5}]));
    // "late." ends at 0.8 s and "Done." starts at 2.0 s on the timeline.
    let report = lint(dir.path(), slow, &t, &[]);
    assert_eq!(gaps(&report), [1.2], "{report}");

    // A pause in the edit passes the gap when it leaves at most the
    // threshold uncovered, and is reported as such; one that leaves more
    // than the threshold does not hide the dead air.
    let report = lint(dir.path(), cut.clone(), &t, &[]);
    let item = &warning(&report, "dead_air").unwrap()["items"][0];
    let (start, end) = (
        item["timeline_start"].as_f64().unwrap(),
        item["timeline_end"].as_f64().unwrap(),
    );
    let paused = |from: f64, to: f64| {
        let mut e = cut.clone();
        e["pauses"] = json!([{"start": from, "end": to, "note": "beat"}]);
        e
    };
    for (from, to, excluded) in [
        (start, end, true),
        (start + 0.6 - 0.01, end, true),
        (start + 0.6 + 0.01, end, false),
        (start, end - 0.6 - 0.01, false),
    ] {
        let report = lint(dir.path(), paused(from, to), &t, &[]);
        let list = report["checked"]["dead_air"]["excluded_by_pause"]
            .as_array()
            .unwrap();
        assert_eq!(
            warning(&report, "dead_air").is_none(),
            excluded,
            "{from}-{to}: {report}"
        );
        assert_eq!(list.len(), usize::from(excluded), "{report}");
        if excluded {
            assert_eq!(list[0]["pause_notes"], json!(["beat"]), "{report}");
            assert_eq!(report["checked"]["dead_air"]["pauses"][0]["note"], "beat");
        }
    }

    // An L-cut under B-roll keeps the dialogue heard: picture changes add no
    // dead air, while muting the dialogue under the B-roll does.
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
        dir.path().join("broll.mp4").to_str().unwrap(),
    ]);
    let broll = |dialogue: Value| {
        json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},
        "video":[{"clips":[{"start":1.5,"source":"broll.mp4","duration":2.5,"audio":{"volume":0}}]}],
        "audio":[{"clips":dialogue}]})
    };
    let report = lint(
        dir.path(),
        broll(json!([{"start":0,"source":"plan.wav","duration":5.5}])),
        &t,
        &[],
    );
    assert_eq!(gaps(&report), [0.7], "{report}");
    let report = lint(
        dir.path(),
        broll(json!([
            {"start":0,"source":"plan.wav","duration":2.0},
            {"start":2.0,"source":"plan.wav","source_in":2.0,"duration":1.5,"volume":0},
            {"start":3.5,"source":"plan.wav","source_in":3.5,"duration":2.0}
        ])),
        &t,
        &[],
    );
    // The muted clip silences "early," through "finish": "start" is heard to
    // 2.0 s and "late." resumes at 3.5 s.
    assert_eq!(gaps(&report), [0.7, 1.5], "{report}");
}

#[test]
fn bad_pauses_and_thresholds_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(dir.path(), "plan", PLAN);
    let path = dir.path().join("edit.json");
    let run = |pauses: Value, extra: &[&str]| {
        let mut e = edit(json!([{"start":0,"source":"plan.wav","duration":5.5}]));
        e["pauses"] = pauses;
        std::fs::write(&path, serde_json::to_vec(&e).unwrap()).unwrap();
        Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .args(["review", "lint", path.to_str().unwrap(), "--transcript", &t])
            .args(extra)
            .output()
            .unwrap()
    };
    // At the edges: a pause may end exactly at the timeline end and the next
    // may start where the previous ends.
    let ok = run(json!([{"start":0,"end":1},{"start":1,"end":5.5}]), &[]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    for (pauses, needle) in [
        (json!([{"start":3,"end":2}]), "0 <= start < end"),
        (json!([{"start":2,"end":2}]), "0 <= start < end"),
        (json!([{"start":-0.1,"end":1}]), "0 <= start < end"),
        (json!([{"start":5,"end":5.6}]), "timeline duration"),
        (
            json!([{"start":1,"end":3},{"start":2.9,"end":4}]),
            "must not overlap",
        ),
        (json!([{"start":3,"end":4},{"start":1,"end":2}]), "ordered"),
        (json!([{"start":1,"end":2,"note":"a\u{7}b"}]), "control"),
        (json!([{"start":1,"end":2,"note":"a\u{fffe}b"}]), "U+FFFE"),
        (json!([{"start":1,"end":2,"label":"x"}]), "unknown field"),
    ] {
        let out = run(pauses.clone(), &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{pauses}");
        assert!(stderr.contains(needle), "{pauses}: {stderr}");
    }
    for extra in [["--dead-air-seconds", "0"], ["--allow-pause", "1-2"]] {
        assert!(!run(json!([]), &extra).status.success(), "{extra:?}");
    }
}

#[test]
fn lint_without_transcripts_says_the_word_checks_did_not_run() {
    let dir = tempfile::tempdir().unwrap();
    speech(dir.path(), "plan", PLAN);
    let path = dir.path().join("edit.json");
    let mut edited = edit(json!([{"start":0,"source":"plan.wav","duration":5.5}]));
    edited["audio"][0]["role"] = json!("dialogue");
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .args(["review", "lint", path.to_str().unwrap(), "--no-levels"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let not_checked = report["not_checked"][0].as_str().unwrap();
    assert!(
        not_checked.contains("no --transcript was supplied"),
        "{report}"
    );
    assert!(not_checked.contains("dead_air"), "{report}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no --transcript was supplied"));
}

#[test]
fn overlapping_words_already_heard_are_not_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(
        dir.path(),
        "overlap",
        &[
            ("Thanks.", 0.0, 1.0),
            ("And", 0.1, 0.2),
            ("goodbye.", 0.2, 0.4),
        ],
    );
    let report = lint(
        dir.path(),
        edit(json!([{"start":0,"source":"overlap.wav","duration":2.0}])),
        &t,
        &[],
    );
    assert!(warning(&report, "unfinished_ending").is_none(), "{report}");
    assert_eq!(report["checked"]["ending"]["next_source_words"], "");
}

#[test]
fn dead_air_counts_a_word_muted_by_volume_automation_as_silent() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(
        dir.path(),
        "long",
        &[("so", 0.0, 0.25), ("long", 0.3, 1.5), ("then.", 1.8, 2.1)],
    );
    let report = lint(
        dir.path(),
        edit(json!([{"start":0,"source":"long.wav","duration":2.5}])),
        &t,
        &[],
    );
    assert!(warning(&report, "dead_air").is_none(), "{report}");
    let muted = edit(json!([{"start":0,"source":"long.wav","duration":2.5,
        "volume_keyframes":[{"time":0.95,"volume":1},{"time":1.0,"volume":0},{"time":1.7,"volume":0},{"time":1.75,"volume":1}]}]));
    let report = lint(dir.path(), muted, &t, &[]);
    let seconds = gaps(&report);
    assert_eq!(seconds.len(), 1, "{report}");
    assert!((0.79..=0.82).contains(&seconds[0]), "{report}");
}

#[test]
fn dead_air_follows_gain_inside_words_and_across_split_words() {
    let dir = tempfile::tempdir().unwrap();
    // A mute in the middle of a long word opens a gap inside it.
    let t = speech(
        dir.path(),
        "inner",
        &[("long", 0.3, 2.0), ("next.", 2.1, 2.4)],
    );
    let muted = edit(json!([{"start":0,"source":"inner.wav","duration":2.5,
        "volume_keyframes":[{"time":1.3,"volume":1},{"time":1.31,"volume":0},{"time":1.95,"volume":0},{"time":1.96,"volume":1}]}]));
    let report = lint(dir.path(), muted, &t, &[]);
    let seconds = gaps(&report);
    assert_eq!(seconds.len(), 1, "{report}");
    assert!((0.63..=0.67).contains(&seconds[0]), "{report}");

    // A word split by a straight cut keeps each part's own gain: the first
    // clip's fade-out does not silence the continuation.
    let t = speech(
        dir.path(),
        "split",
        &[("hello", 0.1, 1.2), ("there.", 1.3, 1.6)],
    );
    let split = edit(json!([
        {"start":0,"source":"split.wav","duration":0.6,"fade_out":0.3},
        {"start":0.6,"source":"split.wav","source_in":0.6,"duration":1.0}
    ]));
    let report = lint(dir.path(), split, &t, &[]);
    assert!(warning(&report, "dead_air").is_none(), "{report}");
}

#[test]
fn endings_follow_gain_and_the_final_clip() {
    let dir = tempfile::tempdir().unwrap();
    // A partly muted last word is still the last heard word.
    let t = speech(
        dir.path(),
        "tail",
        &[
            ("Done.", 0.1, 0.3),
            ("another", 0.6, 1.6),
            ("thing.", 1.7, 2.0),
        ],
    );
    let muted = edit(json!([{"start":0,"source":"tail.wav","duration":2.5,
        "volume_keyframes":[{"time":0.8,"volume":1},{"time":0.81,"volume":0}]}]));
    let report = lint(dir.path(), muted, &t, &[]);
    let w = warning(&report, "unfinished_ending").expect("ends on another");
    assert!(
        w["last_words"].as_str().unwrap().ends_with("another"),
        "{report}"
    );
    assert_eq!(report["checked"]["ending"]["last_word"], "another");

    // Words played earlier in the edit are still cut off at its final clip.
    let t = speech(
        dir.path(),
        "order",
        &[
            ("ready", 0.1, 0.4),
            ("and", 0.5, 0.7),
            ("waiting", 0.8, 1.0),
        ],
    );
    let reordered = edit(json!([
        {"start":0,"source":"order.wav","source_in":0.5,"duration":0.5},
        {"start":0.5,"source":"order.wav","duration":0.5}
    ]));
    let report = lint(dir.path(), reordered, &t, &[]);
    let w = warning(&report, "unfinished_ending").expect("cut before and");
    assert_eq!(w["next_source_words"], "and waiting", "{report}");
}

#[test]
fn straight_cuts_and_partly_muted_count_words_keep_ending_checks_right() {
    let dir = tempfile::tempdir().unwrap();
    // Overlapping words played before a straight cut are not cut off.
    let t = speech(
        dir.path(),
        "overlap",
        &[
            ("Thanks.", 0.0, 1.0),
            ("And", 0.1, 0.2),
            ("goodbye.", 0.2, 0.4),
        ],
    );
    let split = edit(json!([
        {"start":0,"source":"overlap.wav","duration":0.5},
        {"start":0.5,"source":"overlap.wav","source_in":0.5,"duration":1.5}
    ]));
    let report = lint(dir.path(), split, &t, &[]);
    assert!(warning(&report, "unfinished_ending").is_none(), "{report}");

    // A count word whose middle is muted still announces the list.
    let t = speech(
        dir.path(),
        "count",
        &[
            ("three", 0.1, 0.7),
            ("reasons.", 0.9, 1.1),
            ("First,", 1.3, 1.6),
            ("cost.", 1.7, 2.0),
        ],
    );
    let muted = edit(json!([{"start":0,"source":"count.wav","duration":1.2,
        "volume_keyframes":[{"time":0.34,"volume":1},{"time":0.35,"volume":0},{"time":0.8,"volume":0},{"time":0.81,"volume":1}]}]));
    let report = lint(dir.path(), muted, &t, &[]);
    let w = warning(&report, "unfinished_ending").expect("open list");
    assert!(
        w["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reason"] == "open_list"),
        "{report}"
    );
}

#[test]
fn a_fade_that_ends_with_a_word_ends_the_heard_word_there() {
    let dir = tempfile::tempdir().unwrap();
    let t = speech(
        dir.path(),
        "fade",
        &[
            ("Thanks.", 0.5, 1.0),
            ("Next", 2.0, 2.4),
            ("one.", 2.45, 2.9),
        ],
    );
    // "Thanks." fades out to its end at 1.0 s; "Next" starts at 1.6 s, so
    // the gap is exactly 0.6 s.
    let faded = edit(json!([
        {"start":0,"source":"fade.wav","duration":1.0,"fade_out":0.5},
        {"start":1.6,"source":"fade.wav","source_in":2.0,"duration":1.0}
    ]));
    let report = lint(dir.path(), faded.clone(), &t, &[]);
    assert!(warning(&report, "dead_air").is_none(), "{report}");
    let report = lint(dir.path(), faded, &t, &["--dead-air-seconds", "0.59"]);
    let gap = gaps(&report);
    assert_eq!(gap.len(), 1, "{report}");
    assert!((gap[0] - 0.6).abs() < 1e-6, "{report}");
}
