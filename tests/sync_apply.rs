//! `skywrite sync apply`: placement from a real sync result, refusals, and
//! the segmented placement math against hand-written results.
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::Command;

fn hash(mut x: u64) -> f64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    ((x >> 32) as u32) as f64 / u32::MAX as f64
}
/// Non-repeating gated chirps: distinctive at both envelope and waveform level.
fn signal(t: f64) -> f64 {
    let cell = (t * 2.7).floor();
    let phase = t * 2.7 - cell;
    let amplitude = 0.03 + 0.30 * hash(cell as u64 + 117);
    let on = 0.03 + hash(cell as u64 + 313) * 0.2;
    let off = 0.55 + hash(cell as u64 + 719) * 0.4;
    let gate = if phase < on {
        phase / on
    } else if phase > off {
        (1.0 - phase) / (1.0 - off)
    } else {
        1.0
    };
    amplitude
        * gate
        * ((std::f64::consts::TAU * (117.0 * t + 0.3 * t * t)).sin()
            + 0.4 * (std::f64::consts::TAU * (247.0 * t + 0.13 * t * t)).sin())
}
/// Speech-like: syllable-rate gated broadband noise without sustained tones.
fn speech_like(t: f64) -> f64 {
    let cell = (t * 4.3).floor();
    let phase = t * 4.3 - cell;
    let amplitude = if hash(cell as u64 + 5) < 0.2 {
        0.0
    } else {
        0.05 + 0.3 * hash(cell as u64 + 11)
    };
    let envelope = (std::f64::consts::PI * phase).sin().powi(2);
    amplitude * envelope * (2.0 * hash((t * 4000.0).floor() as u64 + 99_991) - 1.0)
}
/// Sample i plays signal(offset + scale * i / rate): reference_time = offset + scale * t.
fn wav(path: &Path, seconds: f64, offset: f64, scale: f64) {
    wav_of(path, seconds, offset, scale, signal)
}
fn wav_of(path: &Path, seconds: f64, offset: f64, scale: f64, signal: fn(f64) -> f64) {
    let rate = 8000u32;
    let count = (seconds * rate as f64) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + count * 2).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&[1, 0, 1, 0]).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&(count * 2).to_le_bytes()).unwrap();
    for i in 0..count {
        let s = signal(offset + scale * i as f64 / rate as f64);
        f.write_all(&((s * i16::MAX as f64) as i16).to_le_bytes())
            .unwrap();
    }
}
/// camera.mp4: picture plus a scratch track playing signal(camera time).
fn camera(dir: &Path, seconds: f64) {
    let audio = dir.join("camera-audio.wav");
    wav(&audio, seconds, 0.0, 1.0);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c=gray:s=64x36:r=24:d={seconds}"))
        .arg("-i")
        .arg(&audio)
        .args([
            "-shortest",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            "-ar",
            "48000",
        ])
        .arg(dir.join("camera.mp4"))
        .status()
        .unwrap();
    assert!(status.success());
}
fn skywrite(dir: &Path, args: &[&str]) -> (bool, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .env_remove("OPENROUTER_API_KEY")
        .env("SKYWRITE_HOME", dir.join("home"))
        .env("SKYWRITE_CACHE_DIR", dir.join("cache"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    let text = if out.status.success() {
        out.stdout
    } else {
        out.stderr
    };
    let value = serde_json::from_slice(&text)
        .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&text).into_owned()}));
    (out.status.success(), value)
}
fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn camera_edit(dir: &Path, start: f64, source_in: f64, duration: f64) {
    write(
        &dir.join("edit.json"),
        &json!({"version":1,"name":"t","width":64,"height":36,"fps":{"num":24,"den":1},
            "video":[{"name":"V1","clips":[{"start":start,"source":"camera.mp4","source_in":source_in,"duration":duration}]}]}),
    );
}

#[test]
fn late_recorder_with_drift_is_placed_on_the_frame_grid_and_verified() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    camera(dir, 60.0);
    // Recorder starts 7.2 s into the camera; its clock is 300 ppm slow.
    wav(&dir.join("recorder.wav"), 50.0, 7.2, 1.0003);
    camera_edit(dir, 0.0, 0.0, 60.0);
    let (ok, sync) = skywrite(
        dir,
        &[
            "sync",
            "--reference",
            "camera.mp4",
            "--asset",
            "recorder.wav",
            "--method",
            "audio",
            "-o",
            "rec.sync.json",
        ],
    );
    assert!(ok, "{sync}");
    assert_eq!(sync["quality"], "strong", "{sync}");
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "rec.sync.json",
            "--edit",
            "edit.json",
            "-o",
            "final.json",
            "--verify",
        ],
    );
    assert!(ok, "{out}");
    let edit: Value =
        serde_json::from_slice(&std::fs::read(dir.join("final.json")).unwrap()).unwrap();
    assert_eq!(edit["video"][0]["clips"][0]["audio"]["volume"], 0.0);
    let track = &edit["audio"][0];
    assert_eq!(track["name"], "recorder.wav (synced)");
    assert_eq!(track["role"], "dialogue");
    let clip = &track["clips"][0];
    let (offset, scale) = (
        sync["offset_seconds"].as_f64().unwrap(),
        sync["scale"].as_f64().unwrap(),
    );
    let start = clip["start"].as_f64().unwrap();
    // First frame at or after the asset's first sample: frame 173 at 24 fps.
    assert_eq!((start * 24.0).round(), (offset * 24.0 - 1e-6).ceil());
    assert!((start * 24.0 - (start * 24.0).round()).abs() < 1e-9);
    let source_in = clip["source_in"].as_f64().unwrap();
    assert!(
        (source_in - (start - offset) / scale).abs() < 1e-9,
        "{clip}"
    );
    assert_eq!(clip["speed"], sync["playback_rate"]);
    // Trimmed to the recorder's end (50 s of recorder at the reference clock).
    let duration = clip["duration"].as_f64().unwrap();
    assert!(source_in + duration * clip["speed"].as_f64().unwrap() <= 50.0 + 1e-9);
    assert!(
        start + duration > offset + scale * 50.0 - 1.0 / 24.0 - 1e-9,
        "{clip}"
    );
    assert_eq!(out["validation"]["valid"], true);
    assert!(
        out["validation"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| !w.as_str().unwrap().contains("rounded")),
        "{out}"
    );
    // Audio truth: offset 7.2, scale 1.0003.
    assert!(
        (offset - 7.2).abs() < 0.0005 && (scale - 1.0003).abs() < 5e-6,
        "{sync}"
    );
    assert_eq!(out["verify"]["passed"], true, "{out}");
    let residual = out["verify"]["clips"][0]["residual_end_ms"]
        .as_f64()
        .unwrap();
    assert!(residual.abs() < 2.0, "{out}");
    // The camera sound is uncovered for its first 7.2 s and last ~2.8 s.
    assert_eq!(out["uncovered"].as_array().unwrap().len(), 2, "{out}");
    // Inputs untouched; the output is refused without --overwrite.
    let (ok, again) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "rec.sync.json",
            "--edit",
            "edit.json",
            "-o",
            "final.json",
        ],
    );
    assert!(!ok && again.to_string().contains("--overwrite"), "{again}");
}

/// The overwrite guard alone, without the slow real sync above, so it also
/// runs in scripts/verify-fast.
#[test]
fn existing_output_is_refused_without_overwrite_before_inputs_are_read() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("final.json"), b"previous edit").unwrap();
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "missing.sync.json",
            "--edit",
            "missing-edit.json",
            "-o",
            "final.json",
        ],
    );
    assert!(!ok && out.to_string().contains("--overwrite"), "{out}");
    assert_eq!(
        std::fs::read(dir.join("final.json")).unwrap(),
        b"previous edit"
    );
}

#[test]
fn short_overlap_is_refused_unless_allowed_and_options_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    camera(dir, 30.0);
    // Shares only 12 s with the camera: no drift estimate.
    wav(&dir.join("recorder.wav"), 20.0, 18.0, 1.0);
    camera_edit(dir, 0.0, 0.0, 30.0);
    let (ok, sync) = skywrite(
        dir,
        &[
            "sync",
            "--reference",
            "camera.mp4",
            "--asset",
            "recorder.wav",
            "--method",
            "audio",
            "-o",
            "s.json",
        ],
    );
    assert!(ok, "{sync}");
    assert_eq!(sync["drift_estimated"], false, "{sync}");
    let (ok, refused) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "s.json",
            "--edit",
            "edit.json",
            "-o",
            "out.json",
        ],
    );
    assert!(!ok);
    let message = refused.to_string();
    assert!(
        message.contains("refusing")
            && message.contains("no drift estimate")
            && message.contains("--allow-limited"),
        "{message}"
    );
    assert!(!dir.join("out.json").exists());
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "s.json",
            "--edit",
            "edit.json",
            "-o",
            "out.json",
            "--allow-limited",
            "--keep-reference-audio",
            "--track",
            "Lav",
            "--verify",
        ],
    );
    assert!(ok, "{out}");
    // About 12 s placed: too short for a strong re-sync, so skipped and not passed.
    assert_eq!(out["verify"]["clips"][0]["status"], "skipped", "{out}");
    assert_eq!(out["verify"]["passed"], false, "{out}");
    assert!(
        out["warnings"].to_string().contains("--allow-limited"),
        "{out}"
    );
    let edit: Value =
        serde_json::from_slice(&std::fs::read(dir.join("out.json")).unwrap()).unwrap();
    assert_eq!(edit["video"][0]["clips"][0]["audio"]["volume"], 1.0);
    assert_eq!(edit["audio"][0]["name"], "Lav");
    // Outputs must sit next to the edit.
    std::fs::create_dir(dir.join("elsewhere")).unwrap();
    let (ok, moved) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "s.json",
            "--edit",
            "edit.json",
            "-o",
            "elsewhere/out.json",
            "--allow-limited",
        ],
    );
    assert!(
        !ok && moved.to_string().contains("next to --edit"),
        "{moved}"
    );
}

/// A hand-written two-segment result (the recorder paused for 4 s at its
/// 30 s) placed against a trimmed camera clip that starts at 2 s.
#[test]
fn segmented_result_places_one_clip_per_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    camera(dir, 70.0);
    wav(&dir.join("recorder.wav"), 60.0, 0.0, 1.0);
    camera_edit(dir, 2.0, 1.5, 65.0);
    let segment = |range: [f64; 2], offset: f64| {
        json!({"asset_range":range,"offset_seconds":offset,"scale":1.0,"playback_rate":1.0,"drift_ppm":0.0,
            "drift_estimated":true,"quality":"strong","inliers":40,"anchor_span_seconds":25.0,
            "asset_interval":[range[0]+1.0, range[1]-1.0],"reference_interval":[range[0]+offset+1.0, range[1]+offset-1.0],
            "median_error_seconds":0.0001,"p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0})
    };
    let segments = vec![segment([0.0, 30.0], 3.01), segment([30.0, 60.0], 7.01)];
    write(
        &dir.join("seg.json"),
        &json!({"version":1,"method":"audio_waveform","reference":dir.join("camera.mp4").canonicalize().unwrap(),
            "asset":dir.join("recorder.wav").canonicalize().unwrap(),
            "mapping":"reference_time = offset_seconds + scale * asset_time",
            "offset_seconds":7.01,"scale":1.0,"playback_rate":1.0,"drift_ppm":0.0,"drift_estimated":true,
            "quality":"limited","candidate_matches":80,"inliers":40,"inlier_ratio":0.5,"anchor_span_seconds":25.0,
            "asset_interval":[31.0,59.0],"reference_interval":[38.01,66.01],"median_error_seconds":0.0001,
            "p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0,"timing_resolution_seconds":0.0005,
            "strong_p95_limit_seconds":0.005,"discontinuous":true,"segments":segments,"warnings":[],"anchors":[]}),
    );
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "seg.json",
            "--edit",
            "edit.json",
            "-o",
            "out.json",
        ],
    );
    assert!(ok, "{out}");
    let p = out["placements"].as_array().unwrap();
    assert_eq!(p.len(), 2, "{out}");
    // Timeline t shows camera 1.5 + (t - 2). Segment 0: asset 0 is camera 3.01 -> t 3.51 -> frame 85.
    let f = |v: &Value| v.as_f64().unwrap();
    assert_eq!(f(&p[0]["start"]), 85.0 / 24.0);
    assert!((f(&p[0]["source_in"]) - (1.5 + 85.0 / 24.0 - 2.0 - 3.01)).abs() < 1e-9);
    // Segment 0 ends at asset 30 = t 33.51 -> last whole frame 804.
    assert!(
        (f(&p[0]["start"]) + f(&p[0]["duration"]) - 804.0 / 24.0).abs() < 1e-9,
        "{out}"
    );
    // Segment 1 starts at asset 30 = camera 37.01 -> t 37.51 -> frame 901.
    assert_eq!(f(&p[1]["start"]), 901.0 / 24.0);
    assert!((f(&p[1]["source_in"]) - (1.5 + 901.0 / 24.0 - 2.0 - 7.01)).abs() < 1e-9);
    // ... and the camera clip's end (67 s) is earlier than the recorder's (asset 60 = t 67.51).
    assert!(
        (f(&p[1]["start"]) + f(&p[1]["duration"]) - 67.0).abs() < 1e-9,
        "{out}"
    );
    assert_eq!(out["uncovered"].as_array().unwrap().len(), 2, "{out}");
    assert_eq!(out["validation"]["valid"], true);
    // A limited segment is refused.
    let mut limited: Value =
        serde_json::from_slice(&std::fs::read(dir.join("seg.json")).unwrap()).unwrap();
    limited["segments"][1]["quality"] = json!("limited");
    write(&dir.join("lim.json"), &limited);
    let (ok, refused) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "lim.json",
            "--edit",
            "edit.json",
            "-o",
            "o2.json",
        ],
    );
    assert!(
        !ok && refused.to_string().contains("segment 1 quality is limited"),
        "{refused}"
    );
    // A clip that is not the sync reference is refused when named.
    let (ok, wrong) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "seg.json",
            "--edit",
            "edit.json",
            "-o",
            "o3.json",
            "--clip",
            "V1:1",
        ],
    );
    assert!(!ok && wrong.to_string().contains("--clip"), "{wrong}");
}

/// An offset just past a frame boundary (within the frame-grid snap) places
/// the asset from its first sample on that frame. FFmpeg 9.1's decode of the
/// scratch track in the short-overlap test measures 18.000000004 s, which used
/// to give source_in -4e-9 s and no placement at all.
#[test]
fn offset_within_the_frame_snap_places_from_the_asset_start() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    camera(dir, 30.0);
    wav(&dir.join("recorder.wav"), 20.0, 18.0, 1.0);
    camera_edit(dir, 0.0, 0.0, 30.0);
    // 24 fps: the snap is 1e-6 frame = 4.2e-8 s.
    for (offset, start, source_in) in [
        (18.000000004, 18.0, 0.0),
        (18.00000004, 18.0, 0.0),
        (17.99999996, 18.0, 4e-8),
        (18.0000001, 18.0 + 1.0 / 24.0, 1.0 / 24.0 - 1e-7),
    ] {
        let segment = json!({"asset_range":[0.0, 20.0],"offset_seconds":offset,"scale":1.0,"playback_rate":1.0,
            "drift_ppm":0.0,"drift_estimated":true,"quality":"strong","inliers":40,"anchor_span_seconds":10.0,
            "asset_interval":[1.0, 11.0],"reference_interval":[offset + 1.0, offset + 11.0],
            "median_error_seconds":0.0001,"p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0});
        write(
            &dir.join("snap.json"),
            &json!({"version":1,"method":"audio_waveform","reference":dir.join("camera.mp4").canonicalize().unwrap(),
                "asset":dir.join("recorder.wav").canonicalize().unwrap(),
                "mapping":"reference_time = offset_seconds + scale * asset_time",
                "offset_seconds":offset,"scale":1.0,"playback_rate":1.0,"drift_ppm":0.0,"drift_estimated":true,
                "quality":"strong","candidate_matches":40,"inliers":40,"inlier_ratio":1.0,"anchor_span_seconds":10.0,
                "asset_interval":[1.0,11.0],"reference_interval":[offset + 1.0, offset + 11.0],"median_error_seconds":0.0001,
                "p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0,"timing_resolution_seconds":0.0005,
                "strong_p95_limit_seconds":0.005,"discontinuous":false,"segments":[segment],"warnings":[],"anchors":[]}),
        );
        let _ = std::fs::remove_file(dir.join("snap-out.json"));
        let (ok, out) = skywrite(
            dir,
            &[
                "sync",
                "apply",
                "--sync",
                "snap.json",
                "--edit",
                "edit.json",
                "-o",
                "snap-out.json",
            ],
        );
        assert!(ok, "offset {offset}: {out}");
        let p = &out["placements"][0];
        assert!(
            (p["start"].as_f64().unwrap() - start).abs() < 1e-12,
            "offset {offset}: {out}"
        );
        let placed = p["source_in"].as_f64().unwrap();
        assert!(
            placed >= 0.0 && (placed - source_in).abs() < 1e-9,
            "offset {offset}: {out}"
        );
    }
}

#[test]
fn reference_audio_clip_on_the_destination_track_is_muted_after_sorting() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    camera(dir, 95.0);
    wav(&dir.join("recorder.wav"), 60.0, 0.0, 1.0);
    // The recorder covers camera 0.01-60.01 s: the video reference gets a
    // placement, the later audio reference (camera 80-90 s) gets none but is
    // still muted, and the new clip sorts before it on the same track.
    write(
        &dir.join("edit.json"),
        &json!({"version":1,"name":"t","width":64,"height":36,"fps":{"num":24,"den":1},
            "video":[{"name":"V1","clips":[{"start":0.0,"source":"camera.mp4","source_in":0.0,"duration":40.0}]}],
            "audio":[{"name":"Dialogue","clips":[{"start":100.0,"source":"camera.mp4","source_in":80.0,"duration":10.0}]}]}),
    );
    let segment = json!({"asset_range":[0.0,60.0],"offset_seconds":0.01,"scale":1.0,"playback_rate":1.0,"drift_ppm":0.0,
        "drift_estimated":true,"quality":"strong","inliers":40,"anchor_span_seconds":58.0,
        "asset_interval":[1.0,59.0],"reference_interval":[1.01,59.01],
        "median_error_seconds":0.0001,"p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0});
    write(
        &dir.join("sync.json"),
        &json!({"version":1,"method":"audio_waveform","reference":dir.join("camera.mp4").canonicalize().unwrap(),
            "asset":dir.join("recorder.wav").canonicalize().unwrap(),
            "mapping":"reference_time = offset_seconds + scale * asset_time",
            "offset_seconds":0.01,"scale":1.0,"playback_rate":1.0,"drift_ppm":0.0,"drift_estimated":true,
            "quality":"strong","candidate_matches":40,"inliers":40,"inlier_ratio":1.0,"anchor_span_seconds":58.0,
            "asset_interval":[1.0,59.0],"reference_interval":[1.01,59.01],"median_error_seconds":0.0001,
            "p95_error_seconds":0.0003,"scale_uncertainty_ppm":1.0,"timing_resolution_seconds":0.0005,
            "strong_p95_limit_seconds":0.005,"discontinuous":false,"segments":[segment],"warnings":[],"anchors":[]}),
    );
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "sync.json",
            "--edit",
            "edit.json",
            "-o",
            "out.json",
            "--track",
            "Dialogue",
        ],
    );
    assert!(ok, "{out}");
    assert_eq!(out["placements"].as_array().unwrap().len(), 1, "{out}");
    let edit: Value =
        serde_json::from_slice(&std::fs::read(dir.join("out.json")).unwrap()).unwrap();
    let clips = edit["audio"][0]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2, "{edit}");
    assert_eq!(clips[0]["source"], "recorder.wav", "{edit}");
    assert_ne!(clips[0]["volume"], json!(0.0), "{edit}");
    assert_eq!(clips[1]["source"], "camera.mp4", "{edit}");
    assert_eq!(clips[1]["volume"], json!(0.0), "{edit}");
    assert_eq!(
        edit["video"][0]["clips"][0]["audio"]["volume"],
        json!(0.0),
        "{edit}"
    );
}

/// Camera from `recorder.wav`: 3 ms acoustic delay, scratch-mic band limit,
/// two early reflections, room noise, compression, 125 ppm slow clock, AAC.
/// Returns (scale, delay, trim): camera c = scale * (a + delay) - trim.
fn scratch_camera(dir: &Path) -> (f64, f64, f64) {
    let chain = "[0:a]adelay=24S,highpass=f=160,lowpass=f=3400,aecho=0.8:0.5:17|29:0.3|0.2[d];\
        [1:a]volume=1[n];[d][n]amix=inputs=2:duration=first:normalize=0,\
        acompressor=threshold=0.06:ratio=5:attack=4:release=120:makeup=2,\
        asetrate=7999,aresample=8000,atrim=start_sample=32000:end_sample=512000,asetpts=N/SR/TB[c]";
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(dir.join("recorder.wav"))
        .args(["-f", "lavfi", "-i", "anoisesrc=c=pink:a=0.01:r=8000:seed=7"])
        .args(["-f", "lavfi", "-i", "color=c=gray:s=64x36:r=25:d=60"])
        .args([
            "-filter_complex",
            chain,
            "-map",
            "2:v",
            "-map",
            "[c]",
            "-shortest",
        ])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            "-ar",
            "48000",
        ])
        .arg(dir.join("camera.mp4"))
        .status()
        .unwrap();
    assert!(status.success());
    // asetrate 7999 on 8 kHz audio stretches it by 8000/7999; 4 s (stretched) trimmed off the front.
    (8000.0 / 7999.0, 24.0 / 8000.0, 4.0)
}

/// The owner's case at small scale: a clean recorder and a damaged,
/// drifting camera scratch track; the placement lands within 2 ms.
#[test]
fn scratch_mic_damage_and_drift_align_within_two_ms() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wav_of(&dir.join("recorder.wav"), 70.0, 0.0, 1.0, speech_like);
    let (scale, delay, trim) = scratch_camera(dir);
    write(
        &dir.join("edit.json"),
        &json!({"version":1,"name":"t","width":64,"height":36,"fps":{"num":25,"den":1},
            "video":[{"name":"V1","clips":[{"start":1.0,"source":"camera.mp4","source_in":0.5,"duration":55.0}]}]}),
    );
    let (ok, sync) = skywrite(
        dir,
        &[
            "sync",
            "--reference",
            "camera.mp4",
            "--asset",
            "recorder.wav",
            "--method",
            "audio",
            "-o",
            "s.json",
        ],
    );
    assert!(ok, "{sync}");
    assert_eq!(sync["quality"], "strong", "{sync}");
    let (ok, out) = skywrite(
        dir,
        &[
            "sync",
            "apply",
            "--sync",
            "s.json",
            "--edit",
            "edit.json",
            "-o",
            "final.json",
        ],
    );
    assert!(ok, "{out}");
    let edit: Value =
        serde_json::from_slice(&std::fs::read(dir.join("final.json")).unwrap()).unwrap();
    let clip = &edit["audio"][0]["clips"][0];
    let f = |v: &Value| v.as_f64().unwrap();
    let (start, source_in, speed, duration) = (
        f(&clip["start"]),
        f(&clip["source_in"]),
        f(&clip["speed"]),
        f(&clip["duration"]),
    );
    // The timeline shows camera 0.5 + (t - 1).
    for t in [start, start + duration / 2.0, start + duration] {
        let a = source_in + speed * (t - start);
        let error = 0.5 + (t - 1.0) - (scale * (a + delay) - trim);
        assert!(error.abs() < 0.002, "error {error} s at {t}: {clip} {sync}");
    }
    assert!(
        (duration - 55.0).abs() < 1e-9,
        "the recorder covers the whole camera clip: {clip}"
    );
}

/// Sustained pure tones behind discrete reflections can lock onto a
/// reflection; the result must then not be strong (so `sync apply` refuses).
#[test]
fn tonal_sound_with_reflections_is_never_strong_and_wrong() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wav(&dir.join("recorder.wav"), 70.0, 0.0, 1.0);
    let (scale, delay, trim) = scratch_camera(dir);
    let (ok, sync) = skywrite(
        dir,
        &[
            "sync",
            "--reference",
            "camera.mp4",
            "--asset",
            "recorder.wav",
            "--method",
            "audio",
        ],
    );
    if !ok || sync["quality"] != "strong" {
        return;
    }
    let f = |v: &Value| v.as_f64().unwrap();
    for a in [5.0, 60.0] {
        let error =
            f(&sync["offset_seconds"]) + f(&sync["scale"]) * a - (scale * (a + delay) - trim);
        assert!(
            error.abs() < 0.002,
            "strong but {error} s off at asset {a}: {sync}"
        );
    }
}
