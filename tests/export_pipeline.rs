//! Tests at the real FFmpeg boundary, not merely serialized command strings.
use serde_json::{Value, json};
use skywrite::{
    export, media, process,
    stretch::StretcherChoice,
    timeline::{self, Edit},
};
use std::{ffi::OsString, fs, path::Path, time::Duration};
fn command(program: &str, args: &[&str]) -> std::process::Output {
    process::run(
        program,
        &process::args(args),
        Duration::from_secs(60),
        4 * 1024 * 1024,
    )
    .unwrap()
}
fn make_fixtures(dir: &Path) {
    let switched = dir.join("speaker's $(touch HACKED) & <take>.mp4");
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=30:d=1.5",
            "-f",
            "lavfi",
            "-i",
            "color=blue:s=160x90:r=30:d=1.5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-filter_complex_threads",
            "1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0[v]",
            "-map",
            "[v]",
            "-map",
            "2:a",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "aac",
            switched.to_str().unwrap(),
        ],
    );
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=green:s=90x160:r=25:d=3",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            dir.join("green.mp4").to_str().unwrap(),
        ],
    );
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=yellow:s=160x90",
            "-frames:v",
            "1",
            "-threads",
            "1",
            dir.join("title.png").to_str().unwrap(),
        ],
    );
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=44100:duration=2",
            dir.join("voice.wav").to_str().unwrap(),
        ],
    );
}
fn edit() -> Value {
    json!({
        "version": 1,
        "name": "A 'quoted' & <edit>",
        "width": 160,
        "height": 90,
        "fps": {"num": 30, "den": 1},
        "video": [{"clips": [
            {"start": 0, "source": "speaker's $(touch HACKED) & <take>.mp4", "source_in": 1.25, "duration": 0.5, "speed": 2.0, "audio": {"volume": 0}},
            {"start": 0.7, "source": "green.mp4", "duration": 0.5, "speed": 0.5},
            {"start": 1.2, "source": "title.png", "duration": 0.3}
        ]}],
        "audio": [{"clips": [
            {"start": 0.7, "source": "voice.wav", "source_in": 0.1, "duration": 0.5, "speed": 1.01, "volume": 0.7}
        ]}]
    })
}
fn build(value: &Value) -> Edit {
    timeline::parse(value.clone()).unwrap()
}
fn center(path: &Path, time: &str) -> [u8; 3] {
    let result = command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-ss",
            time,
            "-i",
            path.to_str().unwrap(),
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-threads",
            "1",
            "-f",
            "rawvideo",
            "pipe:1",
        ],
    );
    let index = (45 * 160 + 80) * 3;
    result.stdout[index..index + 3].try_into().unwrap()
}
fn rms(path: &Path, start: &str, duration: &str) -> f64 {
    let bytes = command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-ss",
            start,
            "-t",
            duration,
            "-i",
            path.to_str().unwrap(),
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-f",
            "f32le",
            "pipe:1",
        ],
    )
    .stdout;
    let samples = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v) as f64)
        .collect::<Vec<_>>();
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn real_render_preserves_timing_picture_and_audio() {
    let dir = tempfile::tempdir().unwrap();
    make_fixtures(dir.path());
    let validated = timeline::validate(&build(&edit()), dir.path()).unwrap();
    let output = dir.path().join("render.mp4");
    export::render(&validated, &output, false, StretcherChoice::Auto).unwrap();
    let info = media::probe(&output).unwrap();
    assert!(info.has_audio() && info.has_video());
    assert!((info.duration - 1.5).abs() < 0.04);
    let frame_count = command(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "csv=p=0",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(String::from_utf8(frame_count.stdout).unwrap().trim(), "45");
    let red = center(&output, "0.05");
    assert!(red[0] > 200 && red[2] < 20, "{red:?}");
    let blue = center(&output, "0.35");
    assert!(
        blue[2] > 200 && blue[0] < 20,
        "retimed trim not blue: {blue:?}"
    );
    let black = center(&output, "0.6");
    assert!(black.iter().all(|v| *v < 10), "gap not black: {black:?}");
    let green = center(&output, "0.9");
    assert!(green[1] > 80 && green[0] < 20, "{green:?}");
    let yellow = center(&output, "1.35");
    assert!(
        yellow[0] > 200 && yellow[1] > 200 && yellow[2] < 20,
        "still not yellow: {yellow:?}"
    );
    assert!(
        rms(&output, "0.1", "0.3") < 0.0001,
        "muted primary audio leaked"
    );
    assert!(
        rms(&output, "0.8", "0.2") > 0.01,
        "offset external audio missing"
    );
    let bytes = fs::read(&output).unwrap();
    assert!(export::render(&validated, &output, false, StretcherChoice::Auto).is_err());
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert!(
        export::render(
            &validated,
            &dir.path().join("green.mp4"),
            true,
            StretcherChoice::Auto
        )
        .is_err()
    );
    assert!(!dir.path().join("HACKED").exists());
}

#[test]
fn interchange_and_python_export_work_with_hostile_names() {
    let dir = tempfile::tempdir().unwrap();
    make_fixtures(dir.path());
    let validated = timeline::validate(&build(&edit()), dir.path()).unwrap();
    let fcpxml = dir.path().join("edit.fcpxml");
    export::export(&validated, "fcpxml", &fcpxml, false).unwrap();
    let check = r#"import sys,xml.etree.ElementTree as E
r=E.parse(sys.argv[1]).getroot()
assert r.attrib['version']=='1.10'
assert r.find('.//project').attrib['name']=="A 'quoted' & <edit>"
assert r.find('.//sequence').attrib['duration']=='3/2s'
assert len(r.findall('.//timeMap'))==3
assert r.find('.//video') is not None
assert r.find('.//asset-clip[@srcEnable="video"]') is not None
assert r.find('.//asset-clip[@srcEnable="audio"]') is not None
assert all('file://' in x.attrib['src'] for x in r.findall('.//media-rep'))
"#;
    command("python3", &["-c", check, fcpxml.to_str().unwrap()]);
    let script = dir.path().join("render.py");
    export::export(&validated, "ffmpeg", &script, false).unwrap();
    let output = dir.path().join("python.mp4");
    command(
        "python3",
        &[
            script.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!((media::probe(&output).unwrap().duration - 1.5).abs() < 0.04);
    let blue = center(&output, "0.35");
    assert!(blue[2] > 200 && blue[0] < 20);
    let args: Vec<OsString> = vec![
        script.into_os_string(),
        "--output".into(),
        output.clone().into_os_string(),
    ];
    assert!(process::run("python3", &args, Duration::from_secs(5), 1024 * 1024).is_err());
    assert!(!dir.path().join("HACKED").exists());
    for target in ["hyperframes", "remotion"] {
        let project = dir.path().join(target);
        export::export(&validated, target, &project, false).unwrap();
        assert!(
            project
                .join(if target == "hyperframes" {
                    "index.html"
                } else {
                    "index.jsx"
                })
                .is_file()
        );
        assert!(export::export(&validated, target, &project, true).is_err());
    }
}

#[test]
fn hyperframes_html_excludes_human_metadata_and_preserves_edit_data() {
    let dir = tempfile::tempdir().unwrap();
    make_fixtures(dir.path());
    let mut specification = edit();
    specification["name"] = json!("Ordinary edit");
    let ordinary = timeline::validate(&build(&specification), dir.path()).unwrap();
    let ordinary_project = dir.path().join("ordinary");
    export::export(&ordinary, "hyperframes", &ordinary_project, false).unwrap();
    let expected_html = fs::read_to_string(ordinary_project.join("index.html")).unwrap();
    assert!(expected_html.contains("<title>Skywrite composition</title>"));
    assert!(expected_html.contains("alt=\"Image\""));
    for original in [
        "Ordinary edit",
        "title.png",
        "green.mp4",
        "voice.wav",
        "<take>",
    ] {
        assert!(!expected_html.contains(original), "HTML leaked {original}");
    }

    // Harmless markup-shaped text exercises downstream rewrite-sensitive
    // characters without adding executable content to any runtime fixture.
    let renamed_image = "封面 'quoted' & <caption>.png";
    fs::rename(dir.path().join("title.png"), dir.path().join(renamed_image)).unwrap();
    specification["video"][0]["clips"][2]["source"] = json!(renamed_image);
    for (index, name) in [
        "A 'quoted' & <caption> Ω 🎬",
        "Escaped-looking &lt;caption&gt; &amp; &#34; text",
    ]
    .into_iter()
    .enumerate()
    {
        specification["name"] = json!(name);
        let validated = timeline::validate(&build(&specification), dir.path()).unwrap();
        let project = dir.path().join(format!("text-{index}"));
        export::export(&validated, "hyperframes", &project, false).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("index.html")).unwrap(),
            expected_html,
            "human metadata must not influence executable HTML"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(project.join("edit.validated.json")).unwrap())
                .unwrap();
        assert_eq!(saved["name"], name);
        assert!(
            saved["video"][0]["clips"][2]["source"]
                .as_str()
                .unwrap()
                .ends_with(renamed_image)
        );
        assert_eq!(saved["duration_frames"], 45);
        for (asset, original) in [
            ("asset-0000.mp4", "speaker's $(touch HACKED) & <take>.mp4"),
            ("asset-0001.mp4", "green.mp4"),
            ("asset-0002.png", renamed_image),
            ("asset-0003.wav", "voice.wav"),
        ] {
            assert_eq!(
                fs::read(project.join("assets").join(asset)).unwrap(),
                fs::read(dir.path().join(original)).unwrap()
            );
        }
    }
    let ordinary_data: serde_json::Value =
        serde_json::from_slice(&fs::read(ordinary_project.join("edit.validated.json")).unwrap())
            .unwrap();
    assert_eq!(ordinary_data["name"], "Ordinary edit");
}

#[test]
fn delayed_stream_presentation_times_survive_trim_and_retime() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("delayed.mkv");
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=30:d=3",
            "-itsoffset",
            "0.5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2.5",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            input.to_str().unwrap(),
        ],
    );
    for (index, (source_in, speed, duration)) in [
        (0.0, 1.0, 1.0),
        (0.25, 1.0, 1.0),
        (0.75, 1.0, 1.0),
        (0.0, 2.0, 1.0),
        (0.0, 0.5, 1.5),
        (0.0, 1.0, 0.25),
    ]
    .into_iter()
    .enumerate()
    {
        let mut specification = edit();
        specification["audio"] = json!([]);
        specification["video"] = json!([{"clips":[{"start":0,"source":input,"source_in":source_in,"duration":duration,"speed":speed}]}]);
        let validated = timeline::validate(&build(&specification), dir.path()).unwrap();
        let output = dir.path().join(format!("delayed-{index}.mp4"));
        export::render(&validated, &output, false, StretcherChoice::Auto).unwrap();
        let onset: f64 = ((0.5f64 - source_in) / speed).max(0.0);
        if onset > 0.1 {
            assert!(
                rms(&output, "0.02", &(onset.min(duration) - 0.05).to_string()) < 0.0001,
                "audio incorrectly shifted earlier for source_in={source_in}, speed={speed}"
            );
        }
        if onset + 0.15 < duration {
            assert!(
                rms(&output, &(onset + 0.1).to_string(), "0.1") > 0.03,
                "audio missing after expected onset"
            );
        }
    }
}

#[test]
fn delayed_video_prefix_is_counted_once_in_both_renderers() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("delayed-video.mkv");
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=25:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-filter:v",
            "setpts=PTS+1/TB",
            "-c:v",
            "ffv1",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            input.to_str().unwrap(),
        ],
    );
    for (index, (source_in, speed, duration, fps)) in [
        (0.0, 0.5, 4.0, json!({"num": 30, "den": 1})),
        (0.0, 1.0, 2.0, json!({"num": 30, "den": 1})),
        (0.0, 2.0, 1.5, json!({"num": 30, "den": 1})),
        (0.5, 1.0, 2.0, json!({"num": 30000, "den": 1001})),
        (1.5, 1.0, 1.0, json!({"num": 30, "den": 1})),
        (0.0, 1.0, 0.5, json!({"num": 30, "den": 1})),
        (0.5, 0.5, 0.5, json!({"num": 30, "den": 1})),
    ]
    .into_iter()
    .enumerate()
    {
        let mut specification = edit();
        specification["audio"] = json!([]);
        specification["fps"] = json!(fps);
        specification["video"] = json!([{"clips":[{"start":0,"source":input,"source_in":source_in,"duration":duration,"speed":speed}]}]);
        let validated = timeline::validate(&build(&specification), dir.path()).unwrap();
        let script = dir.path().join(format!("video-{index}.py"));
        export::export(&validated, "ffmpeg", &script, false).unwrap();
        for python in [false, true] {
            let output = dir.path().join(format!("video-{index}-{python}.mp4"));
            if python {
                command(
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
            let onset: f64 = ((1.0f64 - source_in) / speed).max(0.0);
            if onset > 0.1 {
                let black = center(&output, &(onset.min(duration) * 0.5).to_string());
                assert!(
                    black.iter().all(|v| *v < 10),
                    "early picture source_in={source_in} speed={speed} python={python}: {black:?}"
                );
            }
            if onset + 0.2 < duration {
                let red = center(&output, &(onset + 0.15).to_string());
                assert!(
                    red[0] > 200 && red[1] < 20 && red[2] < 20,
                    "late picture source_in={source_in} speed={speed} python={python}: {red:?}"
                );
            }
            assert!(
                rms(&output, "0.05", "0.1") > 0.03,
                "audio missing from video prefix: index={index} python={python} source_in={source_in} speed={speed}"
            );
            assert!((media::probe(&output).unwrap().duration - validated.duration()).abs() < 0.06);
        }
    }
}

#[test]
fn additional_audio_preserves_content_before_delayed_video() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("audio-before-video.mkv");
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=30:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-vf",
            "setpts=PTS+1/TB",
            "-c:v",
            "ffv1",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            source.to_str().unwrap(),
        ],
    );
    for (index, source_in) in [0.0, 0.5].into_iter().enumerate() {
        let mut specification = edit();
        specification["video"] = json!([]);
        specification["audio"] =
            json!([{"clips":[{"start":0,"source":source,"source_in":source_in,"duration":0.5}]}]);
        let validated = timeline::validate(&build(&specification), dir.path()).unwrap();
        let script = dir.path().join(format!("audio-prefix-{index}.py"));
        export::export(&validated, "ffmpeg", &script, false).unwrap();
        for python in [false, true] {
            let output = dir
                .path()
                .join(format!("audio-prefix-{index}-{python}.mp4"));
            if python {
                command(
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
            assert!(
                rms(&output, "0.05", "0.1") > 0.03,
                "additional audio prefix missing source_in={source_in} python={python}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn python_renderer_cleans_owned_groups_on_cancel_timeout_and_failure() {
    let dir = tempfile::tempdir().unwrap();
    make_fixtures(dir.path());
    let validated = timeline::validate(&build(&edit()), dir.path()).unwrap();
    let script = dir.path().join("render.py");
    export::export(&validated, "ffmpeg", &script, false).unwrap();
    let probe = r#"import os,sys,pathlib,subprocess,time,signal,stat,json
root=pathlib.Path(sys.argv[1]); script=root/'render.py'
helper=root/'ffmpeg'
helper.write_text('''#!/usr/bin/env python3
import os,sys,time,signal,subprocess,pathlib
signal.signal(signal.SIGTERM,signal.SIG_IGN)
marker=pathlib.Path(os.environ['MARKER'])
child=subprocess.Popen([sys.executable,'-c', 'import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(12)'])
marker.write_text(str(os.getpid())+' '+str(child.pid))
if os.environ['MODE']=='failure': sys.exit(7)
if os.environ['MODE']=='success': sys.exit(0)
time.sleep(12)
''');helper.chmod(0o700)
def live(pid):
 try: return pathlib.Path('/proc/'+str(pid)+'/stat').read_text().split()[2]!='Z'
 except FileNotFoundError: return False
for mode in ['term','int','timeout','failure','success']:
 marker=root/('pids-'+mode);output=root/('output-'+mode+'.mp4');output.write_bytes(b'protected')
 env=os.environ.copy();env.update(PATH=str(root)+os.pathsep+env['PATH'],MARKER=str(marker),MODE=mode)
 driver='''import runpy,sys,os,signal
m=runpy.run_path(sys.argv[1]); m['PLAN']['timeout_seconds']=0.4
original_killpg=os.killpg
def owned_killpg(pid, sig):
 try:
  os.waitid(os.P_PID,pid,os.WEXITED|os.WNOHANG|os.WNOWAIT)
 except ChildProcessError:
  # This finite fixture owns this still-live group. Clean it before reporting
  # the ordering failure so an inherited output pipe cannot hide the assertion.
  try: original_killpg(pid,signal.SIGKILL)
  except ProcessLookupError: pass
  raise AssertionError('leader reaped before final process-group signal')
 original_killpg(pid,sig)
os.killpg=owned_killpg
sys.argv=[sys.argv[1], '--output',sys.argv[2],'--overwrite']; m['main']()
'''
 p=subprocess.Popen([sys.executable,'-c',driver,str(script),str(output)],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 pids=[]
 try:
  end=time.monotonic()+3
  while not marker.exists() and p.poll() is None and time.monotonic()<end:time.sleep(0.01)
  assert marker.exists(),mode+' child not started'
  pids=list(map(int,marker.read_text().split()))
  if mode in ['term','int']:p.send_signal(signal.SIGTERM if mode=='term' else signal.SIGINT)
  stdout,stderr=p.communicate(timeout=3)
  assert b'leader reaped before final process-group signal' not in stderr,(mode,'lost group identity',stderr)
  if mode=='success': assert p.returncode==0,(mode,p.returncode,stderr)
  else: assert p.returncode!=0,(mode,p.returncode,stderr)
  end=time.monotonic()+1
  while any(map(live,pids)) and time.monotonic()<end:time.sleep(.01)
  assert not any(map(live,pids)),(mode,'live owned processes',pids)
  assert not list(root.glob('.skywrite-render-*')),(mode,'temporary output leaked')
  if mode!='success':assert output.read_bytes()==b'protected',mode+' replaced existing output'
 finally:
  if p.poll() is None:p.kill();p.wait()
  if pids:
   try:os.killpg(pids[0],signal.SIGKILL)
   except ProcessLookupError:pass
# Real exported entrypoint protects source and refuses clobber before FFmpeg.
for output in [root/"speaker's $(touch HACKED) & <take>.mp4",root/'existing.mp4']:
 if output.name=='existing.mp4':output.write_bytes(b'kept')
 before=output.read_bytes()
 p=subprocess.run([sys.executable,str(script),'--output',str(output)],capture_output=True,timeout=3)
 assert p.returncode!=0 and output.read_bytes()==before
print('five lifecycle modes and source/no-clobber controls passed')
"#;
    command("python3", &["-c", probe, dir.path().to_str().unwrap()]);
}

/// Sample index of each 10 ms, 2 kHz Hann burst within 60 ms of `expected` seconds.
fn burst_onsets(path: &Path, expected: &[f64]) -> Vec<i64> {
    let bytes = command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-i",
            path.to_str().unwrap(),
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-f",
            "f32le",
            "pipe:1",
        ],
    )
    .stdout;
    let samples: Vec<f64> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v) as f64)
        .collect();
    let template: Vec<f64> = (0..480)
        .map(|i| {
            let t = i as f64 / 48000.0;
            (std::f64::consts::TAU * 2000.0 * t).sin()
                * 0.5
                * (1.0 - (std::f64::consts::TAU * t / 0.01).cos())
        })
        .collect();
    expected
        .iter()
        .map(|seconds| {
            let center = (seconds * 48000.0).round() as usize;
            (center - 2880..center + 2880)
                .max_by(|a, b| {
                    let score = |s: usize| -> f64 {
                        template.iter().zip(&samples[s..]).map(|(x, y)| x * y).sum()
                    };
                    score(*a).total_cmp(&score(*b))
                })
                .unwrap() as i64
                - center as i64
        })
        .collect()
}

/// Four seconds of silence with 10 ms Hann-windowed 2 kHz bursts at `bursts`.
fn click_wav(path: &Path, bursts: &[f64]) {
    let expression = bursts
        .iter()
        .map(|t| {
            format!("between(t,{t},{t}+0.01)*sin(2*PI*2000*(t-{t}))*0.5*(1-cos(2*PI*(t-{t})/0.01))")
        })
        .collect::<Vec<_>>()
        .join("+");
    command(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("aevalsrc='{expression}':s=48000:d=4"),
            "-c:a",
            "pcm_s16le",
            path.to_str().unwrap(),
        ],
    );
}

#[test]
fn audio_lands_on_exact_samples_at_unit_and_clock_drift_speeds() {
    let dir = tempfile::tempdir().unwrap();
    let bursts = [0.5, 1.5, 2.5, 3.5];
    click_wav(&dir.path().join("clicks.wav"), &bursts);
    // FFmpeg's atempo (WSOLA) put these bursts about 976 samples early, even at speed 1.
    for (index, (source_in, speed)) in [(0.0, 1.0), (0.25, 1.0), (0.0, 1.0005), (0.1, 0.9995)]
        .into_iter()
        .enumerate()
    {
        let mut specification = edit();
        specification["video"] = json!([]);
        specification["audio"] = json!([{"clips":[{"start":0,"source":"clicks.wav","source_in":source_in,"duration":3.5,"speed":speed}]}]);
        let validated = timeline::validate(&build(&specification), dir.path()).unwrap();
        let output = dir.path().join(format!("clicks-{index}.mp4"));
        export::render(&validated, &output, false, StretcherChoice::Auto).unwrap();
        let expected: Vec<f64> = bursts
            .iter()
            .map(|t| (t - source_in) / speed)
            .filter(|t| (0.1..3.4).contains(t))
            .collect();
        let errors = burst_onsets(&output, &expected);
        assert!(
            errors.iter().all(|e| e.abs() <= 1),
            "source_in={source_in} speed={speed}: sample errors {errors:?}"
        );
    }
}

/// Clip-audio edit of `clicks.wav` at `speed` covering 3.4 source seconds.
fn click_edit(dir: &Path, speed: f64) -> timeline::ValidatedEdit {
    let mut specification = edit();
    specification["video"] = json!([]);
    specification["audio"] =
        json!([{"clips":[{"start":0,"source":"clicks.wav","duration":3.4 / speed,"speed":speed}]}]);
    timeline::validate(&build(&specification), dir).unwrap()
}

/// Expected template start of a burst at source `t` played at `speed`: a
/// pitch-preserving stretch keeps the burst's center on `center / speed`.
fn stretched_start(t: f64, speed: f64) -> f64 {
    (t + 0.005) / speed - 0.005
}

#[test]
fn creative_speeds_land_within_a_few_ms_with_rubberband() {
    if !skywrite::stretch::rubberband_available().unwrap() {
        eprintln!(
            "skipping creative_speeds_land_within_a_few_ms_with_rubberband: this FFmpeg has no rubberband filter"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let bursts = [0.5, 1.5, 2.5];
    click_wav(&dir.path().join("clicks.wav"), &bursts);
    // Uncompensated, FFmpeg 8.0.1's rubberband (short window) put these 14.7 ms
    // early at 0.5 and 6.1 ms late at 2.0; atempo 3-36 ms early. 4.0 chains 2x stages.
    for speed in [0.5, 1.25, 2.0, 4.0] {
        let validated = click_edit(dir.path(), speed);
        let output = dir.path().join(format!("rb-{speed}.mp4"));
        let report =
            export::render(&validated, &output, false, StretcherChoice::Rubberband).unwrap();
        assert_eq!(report["audio_stretcher"]["stretcher"], json!("rubberband"));
        assert_eq!(
            report["audio_stretcher"]["calibrations"][0]["speed"],
            json!(speed)
        );
        let expected: Vec<f64> = bursts.iter().map(|t| stretched_start(*t, speed)).collect();
        let errors = burst_onsets(&output, &expected);
        assert!(
            errors.iter().all(|e| e.abs() <= 192),
            "speed={speed}: sample errors {errors:?}"
        );
    }
}

#[test]
fn ffmpeg_script_uses_and_names_the_resolved_stretcher() {
    let dir = tempfile::tempdir().unwrap();
    let bursts = [0.5, 1.5, 2.5];
    click_wav(&dir.path().join("clicks.wav"), &bursts);
    let options = |stretcher| export::ExportOptions {
        media_root: None,
        stretcher: Some(stretcher),
    };
    let unit = click_edit(dir.path(), 1.0);
    let script = dir.path().join("unit.py");
    let report = export::export_with(
        &unit,
        "ffmpeg",
        &script,
        false,
        &options(StretcherChoice::Rubberband),
    )
    .unwrap();
    assert_eq!(report["audio_stretcher"]["stretcher"], Value::Null);
    let text = fs::read_to_string(&script).unwrap();
    assert!(text.contains("# Audio time-stretcher: none ("), "{text}");

    let fast = click_edit(dir.path(), 1.25);
    let script = dir.path().join("atempo.py");
    export::export_with(
        &fast,
        "ffmpeg",
        &script,
        false,
        &options(StretcherChoice::Atempo),
    )
    .unwrap();
    let text = fs::read_to_string(&script).unwrap();
    assert!(
        text.contains("# Audio time-stretcher: atempo (requested atempo)"),
        "{text}"
    );
    assert!(text.contains("atempo=1.25") && !text.contains("rubberband="));

    let error = export::export_with(
        &fast,
        "fcpxml",
        &dir.path().join("cut.fcpxml"),
        false,
        &options(StretcherChoice::Atempo),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("--stretcher applies only to --to ffmpeg"),
        "{error}"
    );

    if !skywrite::stretch::rubberband_available().unwrap() {
        eprintln!(
            "skipping the rubberband half of ffmpeg_script_uses_and_names_the_resolved_stretcher: this FFmpeg has no rubberband filter"
        );
        return;
    }
    let script = dir.path().join("rubberband.py");
    let report = export::export_with(
        &fast,
        "ffmpeg",
        &script,
        false,
        &options(StretcherChoice::Auto),
    )
    .unwrap();
    assert_eq!(report["audio_stretcher"]["stretcher"], json!("rubberband"));
    let text = fs::read_to_string(&script).unwrap();
    assert!(text.contains("# Audio time-stretcher: rubberband (window=short:channels=together); latency compensation measured"), "{text}");
    assert!(text.contains("rubberband=tempo=1.250000000000") && !text.contains("atempo="));
    let output = dir.path().join("script.mp4");
    let run = command(
        "python3",
        &[
            script.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected: Vec<f64> = bursts.iter().map(|t| stretched_start(*t, 1.25)).collect();
    let errors = burst_onsets(&output, &expected);
    assert!(
        errors.iter().all(|e| e.abs() <= 192),
        "sample errors {errors:?}"
    );
}
