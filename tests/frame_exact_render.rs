//! Every clip occupies exactly the output frames the edit's grid gives it, in
//! the renderer and in the exported FFmpeg script, at integer and NTSC rates.
//!
//! Each clip has its own source whose frames show the clip's id and the source
//! frame number as luma blocks, so decoding the render tells, frame by frame,
//! which clip is on screen (0 is black: a gap or the end) and which of its
//! source frames, checked against the documented sampling rule.
use serde_json::{Value, json};
use skywrite::{
    export, process,
    stretch::StretcherChoice,
    timeline::{self, Rational},
};
use std::{path::Path, time::Duration};

const W: usize = 160;
const H: usize = 96;

fn run(program: &str, args: &[&str]) -> Vec<u8> {
    process::run(
        program,
        &process::args(args),
        Duration::from_secs(120),
        16 * 1024 * 1024,
    )
    .unwrap()
    .stdout
}

/// A 4-s H.264 source at `rate` whose frames show their frame number (10 bits,
/// top half) and `id` (4 bits, bottom half) as luma blocks.
fn source(dir: &Path, id: u32, (rate_num, rate_den): (u64, u64)) -> String {
    let path = dir.join(format!("clip-{id}.mp4"));
    let lum = format!(
        "if(lt(Y,H/2),if(bitand(N,pow(2,floor(X/16))),235,16),if(bitand({id},pow(2,floor(X/40))),235,16))"
    );
    run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("color=black:s={W}x{H}:r={rate_num}/{rate_den}:d=4"),
            "-f",
            "lavfi",
            "-i",
            &format!(
                "sine=frequency={}:sample_rate=48000:duration=4",
                200 + 40 * id
            ),
            "-vf",
            &format!("geq=lum='{lum}':cb=128:cr=128"),
            "-c:v",
            "libx264",
            "-g",
            "12",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "-shortest",
            path.to_str().unwrap(),
        ],
    );
    path.to_str().unwrap().to_owned()
}

/// Clip id and source frame on screen in each decoded frame, and each frame's
/// presentation time.
fn decode(path: &Path) -> (Vec<(u32, u64)>, Vec<f64>) {
    let path = path.to_str().unwrap();
    let raw = run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-i",
            path,
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ],
    );
    let ids = raw
        .as_chunks::<{ W * H }>()
        .0
        .iter()
        .map(|frame| {
            let id = (0..4)
                .filter(|bit| frame[(3 * H / 4) * W + bit * 40 + 20] > 128)
                .map(|bit| 1 << bit)
                .sum();
            let number = (0..10)
                .filter(|bit| frame[(H / 4) * W + bit * 16 + 8] > 128)
                .map(|bit| 1 << bit)
                .sum();
            (id, number)
        })
        .collect();
    let times = String::from_utf8(run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
            path,
        ],
    ))
    .unwrap();
    let mut times = times
        .split_whitespace()
        .map(|t| t.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    times.sort_by(f64::total_cmp);
    (ids, times)
}

/// Clip: start frame, frames, source rate, `source_in` in milliseconds and
/// speed as a fraction. Ids count from 1 across tracks in order; later tracks
/// are on top.
struct Clip {
    start: u64,
    frames: u64,
    rate: (u64, u64),
    source_in_ms: u64,
    speed: (u64, u64),
}

const fn clip(
    start: u64,
    frames: u64,
    rate: (u64, u64),
    source_in_ms: u64,
    speed: (u64, u64),
) -> Clip {
    Clip {
        start,
        frames,
        rate,
        source_in_ms,
        speed,
    }
}

fn tracks() -> [Vec<Clip>; 2] {
    [
        vec![
            // A one-frame gap before the first clip, then cuts between
            // two-frame, fourteen-frame and one-frame clips at other rates.
            clip(1, 2, (30, 1), 0, (1, 1)),
            clip(3, 14, (30000, 1001), 2000, (1, 1)),
            clip(17, 1, (25, 1), 370, (1, 1)),
            clip(18, 2, (24, 1), 1000, (1, 1)),
            // Gaps of three and one frames, a one-frame clip between them.
            clip(23, 2, (60, 1), 2000, (1, 1)),
            clip(26, 1, (24000, 1001), 500, (1, 1)),
            clip(27, 6, (30, 1), 0, (2, 1)),
            clip(33, 4, (30000, 1001), 1000, (1, 2)),
            clip(37, 3, (60, 1), 0, (1, 1)),
        ],
        vec![
            // One- and two-frame shots on an upper track.
            clip(6, 1, (25, 1), 0, (1, 1)),
            clip(10, 2, (24, 1), 500, (1, 1)),
        ],
    ]
}

/// The source frame shown on frame `n` of a clip (docs/edit-format.md): its
/// first frame is the first source frame at or after `source_in`; each later
/// source frame j is assigned to the output frame nearest its own time (counted
/// from that first frame, divided by speed; halves round up), and an output
/// frame shows the latest frame assigned to it or earlier.
fn sampled(c: &Clip, (num, den): (u64, u64), n: u64) -> u64 {
    let (rate_num, rate_den) = (u128::from(c.rate.0), u128::from(c.rate.1));
    let (speed_num, speed_den) = (u128::from(c.speed.0), u128::from(c.speed.1));
    let first = (u128::from(c.source_in_ms) * rate_num).div_ceil(1000 * rate_den);
    let slot = |j: u128| {
        let (a, b) = (
            j * rate_den * u128::from(num) * speed_den,
            rate_num * speed_num * u128::from(den),
        );
        (2 * a + b) / (2 * b)
    };
    let mut j = 0;
    while slot(j + 1) <= u128::from(n) {
        j += 1;
    }
    u64::try_from(first + j).unwrap()
}

fn check_rate(num: u32, den: u32) {
    let dir = tempfile::tempdir().unwrap();
    let tracks = tracks();
    let mut id = 0;
    let mut video = Vec::new();
    let mut expected = [(0u32, None); 41];
    for track in &tracks {
        let mut clips = Vec::<Value>::new();
        for c in track {
            id += 1;
            let seconds = |f: u64| f as f64 * f64::from(den) / f64::from(num);
            clips.push(json!({
                "start": seconds(c.start),
                "duration": seconds(c.start + c.frames) - seconds(c.start),
                "source": source(dir.path(), id, c.rate),
                "source_in": c.source_in_ms as f64 / 1000.0,
                "speed": c.speed.0 as f64 / c.speed.1 as f64,
            }));
            for n in 0..c.frames {
                let picture = sampled(c, (num.into(), den.into()), n);
                expected[(c.start + n) as usize] = (id, Some(picture));
            }
        }
        video.push(json!({ "clips": clips }));
    }
    // The edit runs one black frame past the last clip.
    let specification = json!({
        "version": 1,
        "name": "frame grid",
        "width": W,
        "height": H,
        "fps": {"num": num, "den": den},
        "duration": 41.0 * f64::from(den) / f64::from(num),
        "video": video,
        "audio": [],
    });
    let validated =
        timeline::validate(&timeline::parse(specification).unwrap(), dir.path()).unwrap();
    assert_eq!(validated.duration_frames(), 41);
    // Premiere XML and FCPXML place every clip on the same frames.
    let fps = Rational { num, den };
    let premiere = dir.path().join("edit.xml");
    export::export(&validated, "premiere", &premiere, false).unwrap();
    let premiere = std::fs::read_to_string(premiere).unwrap();
    let fcpxml = dir.path().join("edit.fcpxml");
    export::export(&validated, "fcpxml", &fcpxml, false).unwrap();
    let fcpxml = std::fs::read_to_string(fcpxml).unwrap();
    for &Clip { start, frames, .. } in tracks.iter().flatten() {
        let end = start + frames;
        assert!(
            premiere.contains(&format!("<start>{start}</start><end>{end}</end>")),
            "{num}/{den}: Premiere XML has no clip on frames {start}..{end}"
        );
        let offset = format!("offset=\"{}\"", fps.fcpxml(start));
        let duration = format!("duration=\"{}\"", fps.fcpxml(frames));
        assert!(
            fcpxml.match_indices(&offset).any(|(at, _)| {
                let tag = &fcpxml[at..];
                tag[..tag.find('>').unwrap()].contains(&duration)
            }),
            "{num}/{den}: FCPXML has no clip with {offset} {duration}"
        );
    }
    let script = dir.path().join("render.py");
    export::export(&validated, "ffmpeg", &script, false).unwrap();
    let plan = std::fs::read_to_string(&script).unwrap();
    // Each track is restamped on the output frame clock after its concat.
    for t in 0..tracks.len() {
        assert!(
            plan.contains(&format!("[cat{t}]settb={den}/{num},setpts=N[track{t}]")),
            "track {t} is not restamped on the frame clock"
        );
    }
    assert!(
        !plan.contains("AVTB"),
        "a stream kept microsecond timestamps"
    );
    let expected_ids = expected
        .iter()
        .map(|(id, _)| char::from_digit(*id, 16).unwrap())
        .collect::<String>();
    for python in [false, true] {
        let output = dir.path().join(format!("render-{python}.mp4"));
        if python {
            run(
                "python3",
                &[
                    script.to_str().unwrap(),
                    "--output",
                    output.to_str().unwrap(),
                ],
            );
        } else {
            export::render(&validated, &output, false, StretcherChoice::Auto).unwrap();
        }
        let (ids, times) = decode(&output);
        let rendered = ids
            .iter()
            .map(|(id, _)| char::from_digit(*id, 16).unwrap())
            .collect::<String>();
        assert_eq!(
            rendered, expected_ids,
            "{num}/{den} python={python}: clip on screen per output frame"
        );
        for (n, ((id, number), (_, picture))) in ids.iter().zip(&expected).enumerate() {
            if let Some(picture) = picture {
                assert_eq!(
                    number, picture,
                    "{num}/{den} python={python}: source frame of clip {id} on output frame {n}"
                );
            }
        }
        assert_eq!(times.len(), 41, "{num}/{den} python={python}: frame count");
        for (n, time) in times.iter().enumerate() {
            let exact = n as f64 * f64::from(den) / f64::from(num);
            assert!(
                (time - exact).abs() < 0.0005,
                "{num}/{den} python={python}: frame {n} at {time}, expected {exact}"
            );
        }
    }
}

#[test]
fn clips_occupy_their_grid_frames_at_30() {
    check_rate(30, 1);
}

#[test]
fn clips_occupy_their_grid_frames_at_29_97() {
    check_rate(30000, 1001);
}

#[test]
fn clips_occupy_their_grid_frames_at_23_976() {
    check_rate(24000, 1001);
}

#[test]
fn clips_occupy_their_grid_frames_at_25() {
    check_rate(25, 1);
}
