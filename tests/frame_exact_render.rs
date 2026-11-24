//! Every clip occupies exactly the output frames the edit's grid gives it, in
//! the renderer and in the exported FFmpeg script, at integer and NTSC rates.
//!
//! Each clip has its own source whose frames show the clip's id and the source
//! frame number as luma blocks, so decoding the render tells, frame by frame,
//! which clip is on screen (0 is black: a gap or the end) and which of its
//! source frames: the one on screen at the output frame's source time.
use serde_json::{Value, json};
use skywrite::{
    export, media, process,
    stretch::StretcherChoice,
    timeline::{self, Rational},
};
use std::{path::Path, time::Duration};

const W: usize = 160;
/// The edit's length: one black frame after the last clip.
const FRAMES: u64 = 137;
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

/// Source file kinds. Matroska stores times rounded to 1 ms; it and MPEG-TS
/// start the AAC sound (its priming) before the picture, so the picture starts
/// after `source_in` 0. `ShiftedMp4` starts 4.967 s into its own timeline, off
/// the 1/15360 tick grid of its picture.
#[derive(Clone, Copy, PartialEq)]
enum Container {
    Mp4,
    Matroska,
    MpegTs,
    ShiftedMp4,
    /// Matroska whose odd frames are 10 ms late, yet reports 30/1 as both its
    /// nominal and average rate.
    VariableMatroska,
    /// QuickTime with a 1/600 s time base, which rounds 29.97 fps frame times
    /// by up to 0.83 ms.
    CoarseMov,
    /// FFV1 in AVI, whose time base is one frame.
    Avi,
    /// Picture only, starting at 76831/15360 s, which FFprobe reports to the
    /// microsecond (5.002018 s).
    ShiftedPicture,
    /// MP4 whose first picture stays on screen for 3 s (variable frame rate).
    HeldMp4,
    /// The same picture copied into AVI, whose packets carry no presentation
    /// times.
    HeldAvi,
}

/// A 4-s H.264 source at `rate` whose frames show their frame number (10 bits,
/// top half) and `id` (5 bits, bottom half) as luma blocks.
fn source(dir: &Path, id: u32, (rate_num, rate_den): (u64, u64), container: Container) -> String {
    let extension = match container {
        Container::Mp4 | Container::ShiftedMp4 | Container::ShiftedPicture | Container::HeldMp4 => {
            "mp4"
        }
        Container::CoarseMov => "mov",
        Container::Avi | Container::HeldAvi => "avi",
        Container::Matroska | Container::VariableMatroska => "mkv",
        Container::MpegTs => "ts",
    };
    let path = dir.join(format!("clip-{id}.{extension}"));
    if container == Container::HeldAvi {
        let held = dir.join("held");
        std::fs::create_dir_all(&held).unwrap();
        let held = source(&held, id, (rate_num, rate_den), Container::HeldMp4);
        run(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-i",
                &held,
                "-c",
                "copy",
                path.to_str().unwrap(),
            ],
        );
        return path.to_str().unwrap().to_owned();
    }
    let lum = format!(
        "if(lt(Y,H/2),if(bitand(N,pow(2,floor(X/16))),235,16),if(bitand({id},pow(2,floor(X/32))),235,16))"
    );
    let mut args = vec![
        "-v".to_owned(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("color=black:s={W}x{H}:r={rate_num}/{rate_den}:d=4"),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!(
            "sine=frequency={}:sample_rate=48000:duration=4",
            200 + 40 * id
        ),
        "-vf".into(),
        if container == Container::VariableMatroska {
            format!("geq=lum='{lum}':cb=128:cr=128,settb=1/30000,setpts='(N/30+mod(N,2)*0.01)/TB'")
        } else if container == Container::HeldMp4 {
            format!(
                "geq=lum='{lum}':cb=128:cr=128,settb=1/30000,setpts='if(eq(N,0),0,3+(N-1)/30)/TB'"
            )
        } else {
            format!("geq=lum='{lum}':cb=128:cr=128")
        },
        "-c:v".into(),
        if container == Container::Avi {
            "ffv1"
        } else {
            "libx264"
        }
        .into(),
        "-g".into(),
        "12".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-threads".into(),
        "1".into(),
        "-c:a".into(),
        if container == Container::Avi {
            "pcm_s16le"
        } else {
            "aac"
        }
        .into(),
        "-shortest".into(),
    ];
    if container == Container::ShiftedMp4 {
        args.extend(["-output_ts_offset".into(), "4.989".into()]);
    }
    if container == Container::ShiftedPicture {
        args.extend(["-an".into(), "-output_ts_offset".into(), "5.002234".into()]);
    }
    if container == Container::CoarseMov {
        args.extend(["-video_track_timescale".into(), "600".into()]);
    }
    if container == Container::HeldMp4 {
        // No B-frames, as H.264 in AVI has them: AVI cannot store their times.
        args.extend(["-bf".into(), "0".into()]);
    }
    if matches!(container, Container::VariableMatroska | Container::HeldMp4) {
        args.extend([
            "-fps_mode".into(),
            "passthrough".into(),
            "-enc_time_base".into(),
            "1:30000".into(),
        ]);
    }
    args.push(path.to_str().unwrap().to_owned());
    run(
        "ffmpeg",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
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
            let id = (0..5)
                .filter(|bit| frame[(3 * H / 4) * W + bit * 32 + 16] > 128)
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
    source_in_us: u64,
    speed: (u64, u64),
    container: Container,
    muted: bool,
}

const fn clip(
    start: u64,
    frames: u64,
    rate: (u64, u64),
    source_in_us: u64,
    speed: (u64, u64),
) -> Clip {
    Clip {
        start,
        frames,
        rate,
        source_in_us,
        speed,
        container: Container::Mp4,
        muted: false,
    }
}

const fn from(
    container: Container,
    start: u64,
    frames: u64,
    rate: (u64, u64),
    source_in_us: u64,
) -> Clip {
    Clip {
        container,
        ..clip(start, frames, rate, source_in_us, (1, 1))
    }
}

const fn matroska(start: u64, frames: u64, rate: (u64, u64), source_in_us: u64) -> Clip {
    from(Container::Matroska, start, frames, rate, source_in_us)
}

fn tracks() -> [Vec<Clip>; 3] {
    [
        vec![
            // A one-frame gap before the first clip, then cuts between
            // two-frame, fourteen-frame and one-frame clips at other rates.
            clip(1, 2, (30, 1), 0, (1, 1)),
            clip(3, 14, (30000, 1001), 2_000_000, (1, 1)),
            clip(17, 1, (25, 1), 370_000, (1, 1)),
            clip(18, 2, (24, 1), 1_000_000, (1, 1)),
            // Gaps of three and one frames, a one-frame clip between them.
            clip(23, 2, (60, 1), 2_000_000, (1, 1)),
            clip(26, 1, (24000, 1001), 500_000, (1, 1)),
            clip(27, 6, (30, 1), 0, (2, 1)),
            clip(33, 4, (30000, 1001), 1_000_000, (1, 2)),
            // Matroska: its picture starts 21 ms into the source timeline.
            matroska(37, 3, (30, 1), 0),
        ],
        vec![
            // One- and two-frame shots on an upper track.
            clip(6, 1, (25, 1), 0, (1, 1)),
            clip(10, 2, (24, 1), 500_000, (1, 1)),
        ],
        // The cases below sit after the first 41 frames, where nothing else
        // is on screen.
        vec![
            // Matroska at 29.97: in a 29.97 edit each frame falls exactly on a
            // source frame (picture 21 ms in, plus 30 frames), and Matroska
            // stores some of them up to 0.5 ms late, which must still count.
            matroska(41, 6, (30000, 1001), 1_022_000),
            // At 29.97 its second frame is 33 us before source frame 2 starts;
            // exact MP4 timestamps must not count that frame as on screen.
            clip(48, 2, (25, 1), 46_600, (1, 1)),
            // MPEG-TS, with its sound unused and used: FFmpeg moves the
            // timestamps of an MPEG-TS input whose sound goes unused.
            Clip {
                muted: true,
                ..from(Container::MpegTs, 51, 4, (30000, 1001), 2_002_000)
            },
            from(Container::MpegTs, 56, 3, (30000, 1001), 2_002_000),
            // Exactly on source frame 60 of a file whose timeline starts off
            // its picture's tick grid.
            from(Container::ShiftedMp4, 60, 4, (30, 1), 2_021_021),
            // Eight times speed, just before a source frame.
            clip(65, 3, (30, 1), 33_327, (8, 1)),
            // Eight and an eighth times speed with frames exactly on source
            // frames at 29.97 and 23.976: microseconds cannot hold NTSC frame
            // times.
            clip(69, 5, (30000, 1001), 0, (8, 1)),
            clip(75, 10, (24000, 1001), 0, (1, 8)),
            // Variable frame rate: frames keep their real times.
            from(Container::VariableMatroska, 86, 8, (30, 1), 0),
            // On source frame 30, which MOV stores 0.67 ms late.
            from(Container::CoarseMov, 95, 6, (30000, 1001), 1_001_000),
            // Slow motion on source frame 60 of an AVI, whose frame-sized time
            // base must not lose that frame at the end of the decode.
            Clip {
                speed: (1, 8),
                ..from(Container::Avi, 102, 5, (30000, 1001), 2_002_000)
            },
            // A picture 21 ms into a Matroska file at 1.25x: output frames
            // before it are black, on the frame grid of the snapped picture.
            Clip {
                speed: (5, 4),
                ..matroska(108, 4, (30000, 1001), 0)
            },
            // On source frames of a file whose start FFprobe rounds.
            from(Container::ShiftedPicture, 113, 5, (30, 1), 1_000_000),
            from(Container::ShiftedPicture, 119, 5, (30, 1), 0),
            // A picture held for 3 s from 0: on screen at 1.5 s and 2.5 s,
            // further back than the decode's 1-s pre-roll.
            from(Container::HeldMp4, 125, 3, (30, 1), 1_500_000),
            from(Container::HeldMp4, 129, 3, (30, 1), 2_500_000),
            // The same in AVI, which gives FFprobe no packet presentation times.
            from(Container::HeldAvi, 133, 3, (30, 1), 2_500_000),
        ],
    ]
}

/// The source frame shown on frame `n` of a clip (docs/edit-format.md): the
/// one on screen at source time `source_in + n * speed / fps`, that is the last
/// that starts at or before it, or None (black) before the first picture, which
/// starts `picture_us` microseconds into the source timeline.
fn sampled(c: &Clip, (num, den): (u64, u64), picture_us: i128, n: u64) -> Option<u64> {
    let (rate_num, rate_den) = (i128::from(c.rate.0), i128::from(c.rate.1));
    let (speed_num, speed_den) = (i128::from(c.speed.0), i128::from(c.speed.1));
    let (num, den, n) = (i128::from(num), i128::from(den), i128::from(n));
    // Source frame position (time * rate) as a fraction.
    let top = ((i128::from(c.source_in_us) - picture_us) * speed_den * num
        + n * speed_num * den * 1_000_000)
        * rate_num;
    let bottom = 1_000_000 * speed_den * num * rate_den;
    (top >= 0).then(|| u64::try_from(top / bottom).unwrap())
}

/// A variable-rate clip's frame on frame `n`, from the file's own frame times
/// (docs/edit-format.md): a time within the container's 0.5 ms rounding of the
/// 30 fps grid is that grid time; any other counts 0.5 ms early.
fn variable(path: &str, start: f64, c: &Clip, (num, den): (u64, u64), n: u64) -> Option<u64> {
    let times = String::from_utf8(run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "frame=best_effort_timestamp_time",
            "-of",
            "csv=p=0",
            path,
        ],
    ))
    .unwrap();
    let source_time = c.source_in_us as f64 / 1e6
        + n as f64 * (c.speed.0 as f64 / c.speed.1 as f64) * den as f64 / num as f64;
    let mut times = times
        .lines()
        .filter_map(|line| line.split(',').next()?.trim().parse::<f64>().ok())
        .map(|t| t - start)
        .collect::<Vec<_>>();
    times.sort_by(f64::total_cmp);
    let first = times[0];
    times
        .iter()
        .map(|t| {
            let grid = first + ((t - first) * 30.0).round() / 30.0;
            if (t - grid).abs() <= 0.000_501 {
                grid
            } else {
                t - 0.000_5
            }
        })
        .rposition(|t| t <= source_time + 1e-9)
        .map(|k| k as u64)
}

fn check_rate(num: u32, den: u32) {
    let dir = tempfile::tempdir().unwrap();
    let tracks = tracks();
    let mut id = 0;
    let mut video = Vec::new();
    let mut expected = [(0u32, None); FRAMES as usize];
    for track in &tracks {
        let mut clips = Vec::<Value>::new();
        for c in track {
            id += 1;
            let seconds = |f: u64| f as f64 * f64::from(den) / f64::from(num);
            clips.push(json!({
                "start": seconds(c.start),
                "duration": seconds(c.start + c.frames) - seconds(c.start),
                "source": source(dir.path(), id, c.rate, c.container),
                "source_in": c.source_in_us as f64 / 1e6,
                "speed": c.speed.0 as f64 / c.speed.1 as f64,
                "audio": {"volume": if c.muted { 0.0 } else { 1.0 }},
            }));
            let path = clips.last().unwrap()["source"].as_str().unwrap().to_owned();
            let info = media::probe(Path::new(&path)).unwrap();
            let picture = info.video().unwrap().start_time.unwrap() - info.start_time;
            let picture_us = (picture * 1e6).round() as i128;
            for n in 0..c.frames {
                let frame = if matches!(
                    c.container,
                    Container::VariableMatroska | Container::HeldMp4 | Container::HeldAvi
                ) {
                    variable(&path, info.start_time, c, (num.into(), den.into()), n)
                } else {
                    sampled(c, (num.into(), den.into()), picture_us, n)
                };
                expected[(c.start + n) as usize] = match frame {
                    Some(frame) => (id, Some(frame)),
                    None => (0, None),
                };
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
        "duration": FRAMES as f64 * f64::from(den) / f64::from(num),
        "video": video,
        "audio": [],
    });
    let validated =
        timeline::validate(&timeline::parse(specification).unwrap(), dir.path()).unwrap();
    assert_eq!(validated.duration_frames(), FRAMES);
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
        !plan.contains("settb=AVTB["),
        "a segment ended on microsecond timestamps"
    );
    let expected_ids = expected
        .iter()
        .map(|(id, _)| char::from_digit(*id, 32).unwrap())
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
            .map(|(id, _)| char::from_digit(*id, 32).unwrap())
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
        assert_eq!(
            times.len() as u64,
            FRAMES,
            "{num}/{den} python={python}: frame count"
        );
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
fn every_frame_shows_its_clip_and_source_frame_at_30() {
    check_rate(30, 1);
}

#[test]
fn every_frame_shows_its_clip_and_source_frame_at_29_97() {
    check_rate(30000, 1001);
}

#[test]
fn every_frame_shows_its_clip_and_source_frame_at_23_976() {
    check_rate(24000, 1001);
}

#[test]
fn every_frame_shows_its_clip_and_source_frame_at_24() {
    check_rate(24, 1);
}

#[test]
fn every_frame_shows_its_clip_and_source_frame_at_25() {
    check_rate(25, 1);
}

#[test]
fn every_frame_shows_its_clip_and_source_frame_at_60() {
    check_rate(60, 1);
}
