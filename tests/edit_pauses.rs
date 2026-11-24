//! The edit's `pauses`: validation, the schema, NLE markers and import.
use serde_json::{Value, json};
use skywrite::{export, import, timeline};
use std::{path::Path, process::Command};

fn media(dir: &Path) {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=30:d=4",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(dir.join("cam.mp4"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn edit(pauses: Value) -> Value {
    json!({"version":1,"width":160,"height":90,"fps":{"num":30,"den":1},
        "video":[{"clips":[{"start":0,"source":"cam.mp4","duration":4}]}],
        "chapters":[{"start":0,"title":"Start"}],
        "pauses":pauses})
}

fn validated(dir: &Path, value: Value) -> anyhow::Result<timeline::ValidatedEdit> {
    timeline::validate(&timeline::parse(value)?, dir)
}

#[test]
fn pauses_export_as_markers_and_import_back_from_premiere_xml() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let pauses = json!([{"start":1,"end":1.5,"note":"laugh & <beat>"},{"start":2,"end":3}]);
    let valid = validated(dir.path(), edit(pauses)).unwrap();
    assert_eq!(valid.summary()["pause_count"], 2);

    let fcpxml = dir.path().join("edit.fcpxml");
    export::export(&valid, "fcpxml", &fcpxml, false).unwrap();
    let text = std::fs::read_to_string(&fcpxml).unwrap();
    assert!(
        text.contains("<marker start=\"1s\" duration=\"1/2s\" value=\"Pause\" note=\"laugh &amp; &lt;beat&gt;\"/>"),
        "{text}"
    );
    assert!(
        text.contains("<marker start=\"2s\" duration=\"1s\" value=\"Pause\"/>"),
        "{text}"
    );
    assert!(text.contains("<chapter-marker"), "{text}");

    let xml = dir.path().join("edit.xml");
    export::export(&valid, "premiere", &xml, false).unwrap();
    let text = std::fs::read_to_string(&xml).unwrap();
    assert!(
        text.contains("<marker><name>Pause</name><comment>laugh &amp; &lt;beat&gt;</comment><in>30</in><out>45</out></marker>"),
        "{text}"
    );
    let (imported, _) = import::import(&xml).unwrap();
    assert_eq!(
        imported["pauses"],
        json!([{"start":1.0,"end":1.5,"note":"laugh & <beat>"},{"start":2.0,"end":3.0}]),
        "{imported}"
    );
    assert_eq!(imported["chapters"], json!([{"start":0.0,"title":"Start"}]));
}

#[test]
fn renders_ignore_pauses_and_the_schema_describes_them() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let path = dir.path().join("edit.json");
    let run = |args: &[&str]| {
        let cache = tempfile::tempdir().unwrap();
        Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .env("SKYWRITE_CACHE_DIR", cache.path())
            .args(args)
            .output()
            .unwrap()
    };
    let render = |name: &str, pauses: Value| {
        std::fs::write(&path, serde_json::to_vec(&edit(pauses)).unwrap()).unwrap();
        let out = dir.path().join(name);
        let done = run(&[
            "edit",
            "render",
            path.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        // The render itself is not bit-exact between runs (x264 threads), so
        // compare the FFmpeg render script, which has the full graph.
        let script = dir.path().join(format!("{name}.py"));
        let done = run(&[
            "edit",
            "export",
            path.to_str().unwrap(),
            "--to",
            "ffmpeg",
            "-o",
            script.to_str().unwrap(),
        ]);
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        std::fs::read_to_string(script).unwrap()
    };
    assert_eq!(
        render("plain.mp4", json!([])),
        render("paused.mp4", json!([{"start":1,"end":2}]))
    );

    let schema: Value = serde_json::from_slice(&run(&["schema"]).stdout).unwrap();
    let pauses = &schema["edit"]["properties"]["pauses"];
    assert_eq!(
        pauses["items"]["required"],
        json!(["start", "end"]),
        "{pauses}"
    );
    assert!(pauses["description"].as_str().unwrap().contains("dead_air"));
}

#[test]
fn premiere_pauses_stay_whole_frames_inside_the_edit_and_apart() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    // At 30 fps: one pause rounds to no frame at the end, two adjacent ones
    // round to touching frames.
    let pauses = json!([{"start":1,"end":1.02},{"start":1.02,"end":1.05},{"start":3.99,"end":4}]);
    let valid = validated(dir.path(), edit(pauses)).unwrap();
    let xml = dir.path().join("edit.xml");
    let result = export::export(&valid, "premiere", &xml, false).unwrap();
    assert!(
        result["warnings"]
            .to_string()
            .contains("1 pause(s) shorter than a frame"),
        "{result}"
    );
    let text = std::fs::read_to_string(&xml).unwrap();
    assert!(
        text.contains("<in>30</in><out>31</out>") && text.contains("<in>31</in><out>32</out>"),
        "{text}"
    );
    let (imported, _) = import::import(&xml).unwrap();
    // The imported edit validates: in order, apart and inside the sequence.
    validated(dir.path(), imported.clone()).unwrap();
    assert_eq!(
        imported["pauses"].as_array().unwrap().len(),
        2,
        "{imported}"
    );

    // Markers an editor made overlapping or past the end are reported, not imported.
    let broken = text
        .replace("<in>31</in><out>32</out>", "<in>30</in><out>33</out>")
        .replace("<duration>120</duration>", "<duration>120</duration><marker><name>Pause</name><comment></comment><in>119</in><out>130</out></marker>");
    std::fs::write(&xml, broken).unwrap();
    let (imported, notes) = import::import(&xml).unwrap();
    assert_eq!(
        imported["pauses"],
        json!([{"start":1.0,"end":31.0/30.0}]),
        "{imported}"
    );
    assert!(
        notes.iter().any(|n| n.contains("2 Pause markers")),
        "{notes:?}"
    );
}

#[test]
fn imported_pause_comments_are_made_valid_notes() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let valid = validated(dir.path(), edit(json!([{"start":1,"end":2,"note":"x"}]))).unwrap();
    let xml = dir.path().join("edit.xml");
    export::export(&valid, "premiere", &xml, false).unwrap();
    let long = "é".repeat(300);
    let text = std::fs::read_to_string(&xml).unwrap().replace(
        "<comment>x</comment><in>30</in><out>60</out></marker>",
        &format!("<comment>Hold for laugh\nThen reveal</comment><in>30</in><out>60</out></marker><marker><name>Pause</name><comment>{long}</comment><in>75</in><out>90</out></marker>"),
    );
    std::fs::write(&xml, text).unwrap();
    let (imported, notes) = import::import(&xml).unwrap();
    assert_eq!(
        imported["pauses"][0]["note"], "Hold for laugh Then reveal",
        "{imported}"
    );
    assert_eq!(imported["pauses"][1]["note"].as_str().unwrap().len(), 512);
    validated(dir.path(), imported).unwrap();
    assert!(
        notes.iter().any(|n| n.contains("2 Pause marker comments")),
        "{notes:?}"
    );
}
