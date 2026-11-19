//! `skywrite doctor` capability checks against a fake `ffmpeg` whose
//! `-filters` listing is canned, so both outcomes run on any machine.
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const RUBBERBAND: &str =
    " .. rubberband        A->A       Apply time-stretching and pitch-shifting.";
const SUBTITLES: &str = " .. subtitles         V->V       Render text subtitles onto input video using the libass library.";

/// Run `doctor` with a fake `ffmpeg` listing exactly `filters`.
fn doctor(dir: &Path, filters: &[&str]) -> Value {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("filters.txt"), filters.join("\n") + "\n").unwrap();
    let script = bin.join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -hide_banner ] && [ \"$2\" = -filters ]; then\n  cat '{}'\n  exit 0\nfi\necho 'ffmpeg version fake'\n",
            bin.join("filters.txt").display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env_remove("OPENROUTER_API_KEY")
        .env("SKYWRITE_HOME", dir.join("home"))
        .env("SKYWRITE_CACHE_DIR", dir.join("cache"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(dir)
        .arg("doctor")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn doctor_reports_libass_and_rubberband_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let report = doctor(dir.path(), &[RUBBERBAND, SUBTITLES]);
    assert_eq!(
        report["burned_captions"],
        serde_json::json!({"subtitles_filter_available": true})
    );
    assert_eq!(report["audio_stretcher"]["rubberband_available"], true);
    assert_eq!(report["audio_stretcher"]["auto_uses"], "rubberband");
}

#[test]
fn doctor_reports_missing_libass_with_fix_hint() {
    let dir = tempfile::tempdir().unwrap();
    let report = doctor(dir.path(), &[RUBBERBAND]);
    assert_eq!(
        report["burned_captions"]["subtitles_filter_available"],
        false
    );
    let fix = report["burned_captions"]["fix"].as_str().unwrap();
    let expected = if cfg!(target_os = "macos") {
        "brew install ffmpeg-full"
    } else {
        "distribution's FFmpeg built with libass"
    };
    assert!(fix.contains(expected), "{fix}");
    assert_eq!(report["audio_stretcher"]["rubberband_available"], true);
}
