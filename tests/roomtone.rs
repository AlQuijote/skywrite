//! `audio roomtone` on synthetic media, checked by independent FFmpeg decodes:
//! speech-like tone bursts with digital-black gaps, and pink-noise beds.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

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
/// Six seconds of 220-Hz bursts: 0.4 s on, 0.6 s of digital black, mono 24 kHz
/// (the shape of a generated narration take).
fn speech(dir: &Path) -> PathBuf {
    let path = dir.join("speech.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:sample_rate=24000:duration=6",
            "-af",
            "volume=0:enable='gte(mod(t,1),0.4)'",
            "-ac",
            "1",
        ],
        &path,
    );
    path
}
fn noise(
    dir: &Path,
    name: &str,
    amplitude: &str,
    rate: &str,
    seconds: &str,
    channels: &str,
) -> PathBuf {
    let path = dir.join(name);
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!(
                "anoisesrc=color=pink:amplitude={amplitude}:sample_rate={rate}:duration={seconds}:seed=7"
            ),
            "-ac",
            channels,
        ],
        &path,
    );
    path
}
/// Runs with test-owned home and cache directories under `dir`.
fn roomtone(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env_remove("OPENROUTER_API_KEY")
        .env("SKYWRITE_HOME", dir.join("home"))
        .env("SKYWRITE_CACHE_DIR", dir.join("cache"))
        .args(["audio", "roomtone"])
        .args(args)
        .output()
        .unwrap()
}
fn ok(dir: &Path, args: &[&str]) -> Value {
    let out = roomtone(dir, args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
fn refused(dir: &Path, args: &[&str]) -> String {
    let out = roomtone(dir, args);
    assert!(!out.status.success());
    String::from_utf8_lossy(&out.stderr).into_owned()
}
/// Independent decode: (RMS dBFS, peak dBFS) of `start..end` seconds.
fn levels(path: &Path, start: f64, end: f64) -> (f64, f64) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-af",
            &format!(
                "atrim=start={start}:end={end},astats=measure_overall=RMS_level+Peak_level:measure_perchannel=none"
            ),
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let value = |name: &str| -> f64 {
        text.lines()
            .filter_map(|l| l.split_once(name).map(|(_, v)| v.trim()))
            .filter_map(|v| v.parse().ok())
            .next_back()
            .unwrap_or_else(|| panic!("{name} missing: {text}"))
    };
    (value("RMS level dB:"), value("Peak level dB:"))
}
/// Independent decode: decoded sample count at the file's own rate.
fn decoded_samples(path: &Path) -> u64 {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-af",
            "astats=measure_overall=Number_of_samples:measure_perchannel=none",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter_map(|l| l.split_once("Number of samples:").map(|(_, v)| v.trim()))
        .filter_map(|v| v.parse::<f64>().ok())
        .next_back()
        .unwrap() as u64
}
/// Independent true peak (dBTP) via the EBU R128 meter.
fn true_peak(path: &Path) -> f64 {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let summary = &text[text.rfind("True peak:").unwrap()..];
    summary
        .lines()
        .find_map(|l| l.trim().strip_prefix("Peak:"))
        .and_then(|v| v.trim().trim_end_matches("dBFS").trim().parse().ok())
        .unwrap()
}

#[test]
fn speech_is_normalized_then_summed_with_a_bed_at_the_target_floor() {
    let dir = tempfile::tempdir().unwrap();
    let dry = speech(dir.path());
    let dry_hash = skywrite::media::file_sha256(&dry).unwrap();
    // Shorter than the speech (loops), stereo 44.1 kHz; longer (trims), mono 48 kHz.
    let short = noise(dir.path(), "short.wav", "0.02", "44100", "2.5", "2");
    let long = noise(dir.path(), "long.wav", "0.02", "48000", "12", "1");
    for (bed, looped) in [(&short, true), (&long, false)] {
        let output = dir.path().join("mixed.wav");
        let r = ok(
            dir.path(),
            &[
                dry.to_str().unwrap(),
                "--bed",
                bed.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--overwrite",
            ],
        );
        // Speech reached its loudness target before the bed was added.
        let speech_lufs = r["speech"]["measured_lufs"].as_f64().unwrap();
        assert!((speech_lufs + 18.0).abs() < 0.3, "{speech_lufs}");
        // Exactly the dry duration: 6 s of 24-kHz input is 288000 samples at 48 kHz.
        assert_eq!(r["rounded_target_samples"], 288000);
        assert_eq!(r["output_timing"]["samples"], 288000);
        assert_eq!(decoded_samples(&output), 288000);
        assert_eq!(r["sample_rate"], 48000);
        assert_eq!(r["channels"], 2);
        assert_eq!(r["bed"]["target_rms_dbfs"], -58.0);
        let level = r["bed"]["level_in_mix_rms_dbfs"].as_f64().unwrap();
        assert!((level + 58.0).abs() < 0.3, "{level}");
        assert_eq!(r["bed"]["loop_repetitions"].as_u64().unwrap() > 1, looped);
        assert_eq!(r["bed"]["trimmed"], !looped);
        assert_eq!(r["warnings"].as_array().unwrap().len(), usize::from(looped));
        // Every gap carries the bed near its target, never digital black.
        for second in 0..6 {
            let gap = second as f64;
            let (rms, peak) = levels(&output, gap + 0.45, gap + 0.95);
            assert!((rms + 58.0).abs() < 1.5, "gap {second}: {rms} dBFS");
            assert!(peak > -70.0, "gap {second}: peak {peak}");
        }
        // The bursts keep their level (the bed is 40 dB under them).
        let (burst, _) = levels(&output, 0.05, 0.35);
        assert!(burst > -25.0, "{burst}");
        // True peak stays under the -2 dBTP ceiling, measured independently.
        let tp = true_peak(&output);
        assert!(tp <= -2.0 + 0.1, "{tp}");
        assert!(r["mix"]["true_peak_dbtp"].as_f64().unwrap() <= -1.9);
        assert_eq!(skywrite::media::file_sha256(&dry).unwrap(), dry_hash);
    }
}

#[test]
fn bed_level_highpass_and_mp3_follow_the_options() {
    let dir = tempfile::tempdir().unwrap();
    let dry = speech(dir.path());
    // Machine hum: a strong 60-Hz tone under quiet pink noise.
    let hum = dir.path().join("hum.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=60:sample_rate=48000:duration=8",
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.003:sample_rate=48000:duration=8:seed=3",
            "-filter_complex",
            "[0]volume=0.03[h];[h][1]amix=inputs=2:normalize=0",
            "-ac",
            "1",
        ],
        &hum,
    );
    let output = dir.path().join("mixed.mp3");
    let run = |extra: &[&str]| {
        let mut args = vec![
            dry.to_str().unwrap(),
            "--bed",
            hum.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--overwrite",
        ];
        args.extend_from_slice(extra);
        ok(dir.path(), &args)
    };
    let plain = run(&["--bed-dbfs", "-64"]);
    let filtered = run(&["--bed-dbfs", "-64", "--bed-highpass", "120"]);
    assert_eq!(filtered["codec"], "mp3");
    assert_eq!(filtered["output_timing"]["samples"], 288000);
    assert_eq!(filtered["bed"]["highpass_hz"], 120.0);
    // The high-pass removes most of the hum before the level is measured...
    let raw = plain["bed"]["measured_rms_dbfs"].as_f64().unwrap();
    let cleaned = filtered["bed"]["measured_rms_dbfs"].as_f64().unwrap();
    assert!(cleaned < raw - 6.0, "{raw} -> {cleaned}");
    // ...and either way the bed lands at the requested floor.
    for r in [&plain, &filtered] {
        let level = r["bed"]["level_in_mix_rms_dbfs"].as_f64().unwrap();
        assert!((level + 64.0).abs() < 0.3, "{level}");
        let gain = r["bed"]["applied_gain_db"].as_f64().unwrap();
        let measured = r["bed"]["measured_rms_dbfs"].as_f64().unwrap();
        assert!((measured + gain + 64.0).abs() < 1e-6);
    }
    let (gap, _) = levels(&output, 0.5, 0.9);
    assert!((gap + 64.0).abs() < 2.0, "{gap}");
}

#[test]
fn only_the_mixed_part_of_a_long_bed_sets_its_level() {
    let dir = tempfile::tempdir().unwrap();
    let dry = speech(dir.path());
    // 12 s of noise whose first 6 s (the part mixed under 6 s of speech) is
    // 20 dB quieter than the discarded rest.
    let uneven = dir.path().join("uneven.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.02:sample_rate=48000:duration=12:seed=9",
            "-af",
            "volume=0.1:enable='lt(t,6)'",
            "-ac",
            "1",
        ],
        &uneven,
    );
    let output = dir.path().join("mixed.wav");
    let r = ok(
        dir.path(),
        &[
            dry.to_str().unwrap(),
            "--bed",
            uneven.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ],
    );
    let level = r["bed"]["level_in_mix_rms_dbfs"].as_f64().unwrap();
    assert!((level + 58.0).abs() < 0.3, "{level}");
    let (gap, _) = levels(&output, 2.45, 2.95);
    assert!((gap + 58.0).abs() < 1.5, "{gap}");
    // Silent where it would be mixed: refused, not a silent "room tone".
    let late = dir.path().join("late.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.02:sample_rate=48000:duration=12:seed=9",
            "-af",
            "volume=0:enable='lt(t,6.5)'",
            "-ac",
            "1",
        ],
        &late,
    );
    let error = refused(
        dir.path(),
        &[
            dry.to_str().unwrap(),
            "--bed",
            late.to_str().unwrap(),
            "-o",
            dir.path().join("late-out.wav").to_str().unwrap(),
        ],
    );
    assert!(error.contains("digital silence"), "{error}");
}

#[test]
fn unusable_beds_and_outputs_are_refused_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let dry = speech(dir.path());
    let output = dir.path().join("mixed.wav");
    let out = output.to_str().unwrap();
    let d = dry.to_str().unwrap();
    let missing = dir.path().join("missing.wav");
    let error = refused(
        dir.path(),
        &[d, "--bed", missing.to_str().unwrap(), "-o", out],
    );
    assert!(
        error.contains("room-tone bed") && error.contains("nobody talking"),
        "{error}"
    );
    let loud = noise(dir.path(), "loud.wav", "0.5", "48000", "4", "1");
    let error = refused(dir.path(), &[d, "--bed", loud.to_str().unwrap(), "-o", out]);
    assert!(error.contains("too loud for room tone"), "{error}");
    let silent = dir.path().join("silent.wav");
    ffmpeg(
        &["-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono", "-t", "4"],
        &silent,
    );
    let error = refused(
        dir.path(),
        &[d, "--bed", silent.to_str().unwrap(), "-o", out],
    );
    assert!(error.contains("digital silence"), "{error}");
    let faint = dir.path().join("faint.wav");
    // 32-bit float keeps a -105 dBFS bed from rounding to digital silence.
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.00001:sample_rate=48000:duration=4:seed=7",
            "-c:a",
            "pcm_f32le",
        ],
        &faint,
    );
    let error = refused(
        dir.path(),
        &[d, "--bed", faint.to_str().unwrap(), "-o", out],
    );
    assert!(error.contains("limit +40 dB"), "{error}");
    let tiny = noise(dir.path(), "tiny.wav", "0.02", "48000", "0.5", "1");
    let error = refused(dir.path(), &[d, "--bed", tiny.to_str().unwrap(), "-o", out]);
    assert!(error.contains("at least 1 s"), "{error}");
    let bed = noise(dir.path(), "bed.wav", "0.02", "48000", "8", "1");
    // Too short for integrated loudness, but not silent.
    let blip = dir.path().join("blip.wav");
    ffmpeg(
        &["-f", "lavfi", "-i", "sine=frequency=440:duration=0.2"],
        &blip,
    );
    let error = refused(
        dir.path(),
        &[
            blip.to_str().unwrap(),
            "--bed",
            bed.to_str().unwrap(),
            "-o",
            out,
        ],
    );
    assert!(
        error.contains("could not be measured") && !error.contains("digital silence"),
        "{error}"
    );
    let hush = dir.path().join("hush.wav");
    ffmpeg(
        &["-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono", "-t", "3"],
        &hush,
    );
    let error = refused(
        dir.path(),
        &[
            hush.to_str().unwrap(),
            "--bed",
            bed.to_str().unwrap(),
            "-o",
            out,
        ],
    );
    assert!(error.contains("speech is digital silence"), "{error}");
    let b = bed.to_str().unwrap();
    assert!(
        refused(dir.path(), &[d, "--bed", d, "-o", out]).contains("separate room-tone recording")
    );
    assert!(
        refused(dir.path(), &[d, "--bed", b, "-o", out, "--bed-dbfs", "-20"]).contains("bed-dbfs")
    );
    assert!(
        refused(
            dir.path(),
            &[d, "--bed", b, "-o", out, "--bed-highpass", "5"]
        )
        .contains("bed-highpass")
    );
    assert!(!output.exists());
    // Inputs are never outputs; an existing output needs --overwrite.
    let hash = skywrite::media::file_sha256(&bed).unwrap();
    assert!(
        !roomtone(dir.path(), &[d, "--bed", b, "-o", b, "--overwrite"])
            .status
            .success()
    );
    assert_eq!(skywrite::media::file_sha256(&bed).unwrap(), hash);
    std::fs::write(&output, b"keep").unwrap();
    assert!(refused(dir.path(), &[d, "--bed", b, "-o", out]).contains("--overwrite"));
    assert_eq!(std::fs::read(&output).unwrap(), b"keep");
}
