//! `--stretcher` against an FFmpeg build without rubberband, simulated by a
//! PATH shim that hides the filter from `ffmpeg -filters`.
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn real_ffmpeg() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v ffmpeg"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A directory whose `ffmpeg` lists every filter except rubberband.
fn shim(dir: &Path) -> String {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -hide_banner ] && [ \"$2\" = -filters ]; then\n  '{0}' -hide_banner -filters | grep -v ' rubberband '\n  exit 0\nfi\nexec '{0}' \"$@\"\n",
            real_ffmpeg()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

fn tempo(dir: &Path, path: &str, stretcher: &str, output: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env_remove("OPENROUTER_API_KEY")
        .env("SKYWRITE_HOME", dir.join("home"))
        .env("SKYWRITE_CACHE_DIR", dir.join("cache"))
        .env("PATH", path)
        .current_dir(dir)
        .args([
            "audio",
            "tempo",
            "tone.wav",
            "--tempo",
            "1.25",
            "--stretcher",
            stretcher,
            "-o",
            output,
        ])
        .output()
        .unwrap()
}

#[test]
fn stretcher_falls_back_warns_or_fails_without_rubberband() {
    let dir = tempfile::tempdir().unwrap();
    let made = Command::new("ffmpeg")
        .current_dir(dir.path())
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:r=48000:d=1",
            "tone.wav",
        ])
        .status()
        .unwrap();
    assert!(made.success());
    let path = shim(dir.path());

    let auto = tempo(dir.path(), &path, "auto", "auto.wav");
    assert!(
        auto.status.success(),
        "{}",
        String::from_utf8_lossy(&auto.stderr)
    );
    let report: Value = serde_json::from_slice(&auto.stdout).unwrap();
    assert_eq!(
        report["audio_stretcher"]["rubberband_available"],
        Value::Bool(false)
    );
    assert_eq!(report["audio_stretcher"]["stretcher"], "atempo");
    let warning = report["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("no rubberband filter") && warning.contains("ms early"),
        "{warning}"
    );
    let stderr = String::from_utf8_lossy(&auto.stderr);
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.starts_with("warning: "))
            .count(),
        1,
        "{stderr}"
    );

    let required = tempo(dir.path(), &path, "rubberband", "required.wav");
    assert!(!required.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&required.stdout),
        String::from_utf8_lossy(&required.stderr)
    );
    assert!(
        text.contains("--stretcher rubberband: this FFmpeg build has no rubberband filter"),
        "{text}"
    );
    assert!(!dir.path().join("required.wav").exists());

    let atempo = tempo(dir.path(), &path, "atempo", "atempo.wav");
    assert!(
        atempo.status.success(),
        "{}",
        String::from_utf8_lossy(&atempo.stderr)
    );
    let report: Value = serde_json::from_slice(&atempo.stdout).unwrap();
    assert_eq!(report["audio_stretcher"]["stretcher"], "atempo");
    assert_eq!(report["warnings"], serde_json::json!([]));
}
