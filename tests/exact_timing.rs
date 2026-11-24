use skywrite::media;
use std::path::Path;
use std::process::Command;
fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn ffmpeg(args: &[&str], output: &Path) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .arg(output)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn tone(path: &Path, seconds: &str) {
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("sine=sample_rate=24000:duration={seconds}"),
            "-c:a",
            "pcm_s16le",
        ],
        path,
    );
}
#[test]
fn counted_samples_cache_source_change_and_estimate_range() {
    let d = scratch();
    let input = d.path().join("tone.wav");
    tone(&input, "0.3");
    let info = media::probe(&input).unwrap();
    let first = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert_eq!(first.samples, 7200);
    assert!(!first.cache_hit);
    assert!((first.presentation_end_seconds - 0.3).abs() < 1e-6);
    assert!(
        media::exact_audio_timing(&info, Some(d.path()))
            .unwrap()
            .cache_hit
    );
    tone(&input, "0.6");
    let next = media::exact_audio_timing(&media::probe(&input).unwrap(), Some(d.path())).unwrap();
    assert!(!next.cache_hit);
    assert_eq!(next.samples, 14400);
    assert_ne!(first.source_sha256, next.source_sha256);
    let mut wrong = media::probe(&input).unwrap();
    wrong.duration = 50.;
    assert!(media::validate_range(&wrong, 0., 1.).is_err());
}
#[test]
fn timing_detects_same_size_rewrite_with_restored_mtime_between_lookups() {
    let d = scratch();
    let input = d.path().join("tone.wav");
    tone(&input, "0.1");
    let info = media::probe(&input).unwrap();
    let first = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert!(
        media::exact_audio_timing(&info, Some(d.path()))
            .unwrap()
            .cache_hit
    );
    let before = std::fs::metadata(&input).unwrap();
    // Change PCM content without changing the WAV header, length, or mtime.
    let mut bytes = std::fs::read(&input).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&input, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&input)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    let after = std::fs::metadata(&input).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    let next = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert!(!next.cache_hit);
    assert_ne!(next.source_sha256, first.source_sha256);
    assert_eq!(next.source_sha256, media::file_sha256(&input).unwrap());
    assert_eq!(next.samples, first.samples);
    let cached = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert!(cached.cache_hit);
    assert_eq!(cached.source_sha256, next.source_sha256);
}

#[test]
fn positive_matroska_origin_and_delayed_audio_keep_presentation() {
    let d = scratch();
    let p = d.path().join("offset.mkv");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=size=32x32:rate=10:duration=2",
            "-itsoffset",
            "0.5",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=24000:duration=1",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-output_ts_offset",
            "5",
        ],
        &p,
    );
    let info = media::probe(&p).unwrap();
    assert!((info.duration - 2.).abs() < 1e-6);
    let exact = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert_eq!(exact.samples, 24000);
    assert!((exact.decoded_duration_seconds - 1.).abs() < 1e-6);
    assert!((exact.presentation_end_seconds - 1.5).abs() < 1e-6);
    assert!((media::resolve_audio_only_timing(info).unwrap().duration - 2.).abs() < 1e-6);
}
#[test]
fn actual_provider_vbr_fixture_when_requested() {
    let Some(path) = std::env::var_os("SKYWRITE_REAL_VBR_FIXTURE") else {
        return;
    };
    let d = scratch();
    let info = media::probe(Path::new(&path)).unwrap();
    let timing = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert!((info.duration - 15.563718).abs() < 0.001);
    assert_eq!(timing.samples, 228143);
    assert_eq!(timing.sample_rate, 24000);
    assert!((timing.presentation_end_seconds - 9.505958).abs() < 0.000002);
    assert!((timing.decoded_duration_seconds - 9.505958333).abs() < 1e-8);
}
#[test]
fn verify_cli_counts_checks_metadata_only_and_protects_input() {
    let d = scratch();
    let p = d.path().join("av.mkv");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=size=32x32:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=24000:duration=1",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
        ],
        &p,
    );
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .arg("verify")
            .arg(&p)
            .args(extra)
            .output()
            .unwrap()
    };
    let out = run(&[
        "--expected-duration",
        "1",
        "--expected-width",
        "32",
        "--expected-height",
        "32",
        "--expected-fps",
        "10",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // The decoder is named once per stream compactly, not by its whole banner.
    for stream in ["video", "audio"] {
        let decoder = &value[stream]["decoder"];
        assert_eq!(decoder.as_object().unwrap().len(), 2, "{decoder}");
        assert_eq!(decoder["build_sha256"].as_str().unwrap().len(), 64);
        assert!(serde_json::to_vec(decoder).unwrap().len() < 200);
    }
    assert_eq!(value["video"]["decoder"], value["audio"]["decoder"]);
    // The full banner (about 1.9 KB with FFmpeg 8.0.1) appeared here twice.
    assert!(!String::from_utf8_lossy(&out.stdout).contains("configuration:"));
    assert_eq!(value["video"]["frames"], 10);
    assert_eq!(value["audio"]["samples"], 24000);
    assert_eq!(value["full_decode_executed"], true);
    assert!(!value["audio"]["peak_dbfs"].is_null());
    let out = run(&["--expected-duration", "2"]);
    assert!(!out.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["passed"],
        false
    );
    let out = run(&["--metadata-only"]);
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["full_decode_executed"], false);
    assert!(value["audio"].is_null());
    assert!(value["video"].is_null());
    let hash = media::file_sha256(&p).unwrap();
    let out = run(&["--output", p.to_str().unwrap(), "--overwrite"]);
    assert!(!out.status.success());
    assert_eq!(hash, media::file_sha256(&p).unwrap());
    let report = d.path().join("report.json");
    std::fs::write(&report, b"keep").unwrap();
    assert!(
        !run(&["--output", report.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(std::fs::read(report).unwrap(), b"keep");
}
#[cfg(unix)]
#[test]
fn fast_inspect_never_invokes_ffmpeg() {
    use std::os::unix::fs::symlink;
    let d = scratch();
    let p = d.path().join("tone.wav");
    tone(&p, "0.1");
    let tools = d.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let real = Command::new("which").arg("ffprobe").output().unwrap();
    symlink(
        String::from_utf8_lossy(&real.stdout).trim(),
        tools.join("ffprobe"),
    )
    .unwrap();
    let run = |exact: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_skywrite"));
        c.env("PATH", &tools).arg("inspect").arg(&p);
        if exact {
            c.arg("--exact-audio").arg("--cache-dir").arg(d.path());
        }
        c.output().unwrap()
    };
    let out = run(false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["media"][0]["duration_basis"], "metadata_estimate");
    assert!(!run(true).status.success());
}
#[test]
fn malformed_vbr_without_seek_header_uses_samples_not_bitrate() {
    let d = scratch();
    let p = d.path().join("vbr.mp3");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            r"aevalsrc=if(lt(t\,1)\,0\,random(0)*0.1):s=24000:d=2",
            "-c:a",
            "libmp3lame",
            "-q:a",
            "2",
            "-write_xing",
            "0",
        ],
        &p,
    );
    let info = media::probe(&p).unwrap();
    let t = media::exact_audio_timing(&info, Some(d.path())).unwrap();
    assert!((info.duration - t.presentation_end_seconds).abs() > 0.5);
    assert_eq!(t.samples, 49536);
    assert!((t.presentation_end_seconds - 2.064).abs() < 0.000001);
}
#[cfg(unix)]
#[test]
fn timing_cache_keys_on_the_whole_ffmpeg_build_but_stores_it_compactly() {
    let d = scratch();
    let input = d.path().join("tone.wav");
    tone(&input, "0.3");
    let real = Command::new("sh")
        .args(["-c", "command -v ffmpeg"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    // Same version line, one extra banner line: a different build.
    let bin = d.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -version ]; then\n  '{real}' -version\n  echo 'rebuilt with another configuration'\n  exit 0\nfi\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let shimmed = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let cache = d.path().join("cache");
    let inspect = |path: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skywrite"));
        command
            .args(["inspect", "--exact-audio", "--cache-dir"])
            .arg(&cache)
            .arg(&input);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        value["media"][0]["audio_timing"].clone()
    };
    let first = inspect(None);
    assert_eq!(first["cache_hit"], false);
    assert_eq!(first["version"], 2);
    let version = first["decoder"]["ffmpeg_version"].as_str().unwrap();
    assert!(!version.is_empty() && !version.contains(' '), "{version}");
    assert_eq!(inspect(None)["cache_hit"], true);
    let other = inspect(Some(&shimmed));
    assert_eq!(other["cache_hit"], false, "a build change must re-measure");
    assert_eq!(other["decoder"]["ffmpeg_version"], version);
    assert_ne!(
        other["decoder"]["build_sha256"],
        first["decoder"]["build_sha256"]
    );
    assert_eq!(inspect(Some(&shimmed))["cache_hit"], true);
    assert_eq!(inspect(None)["cache_hit"], true);
    let entries: Vec<_> = std::fs::read_dir(cache.join("audio-timing-v2"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert!(std::fs::metadata(&entry).unwrap().len() < 1024);
    }
}
