//! Premiere XML (xmeml) structure that Premiere's importer needs, the shared
//! frame grid for back-to-back clips, round trips and media-root URLs.
use serde_json::{Value, json};
use skywrite::{
    export::{self, ExportOptions},
    import::{self, Node},
    process,
    timeline::{self, Rational},
};
use std::{collections::HashMap, path::Path, time::Duration};

fn ffmpeg(args: &[&str]) {
    process::run(
        "ffmpeg",
        &process::args(args),
        Duration::from_secs(60),
        1024 * 1024,
    )
    .unwrap();
}

/// Tiny synthetic media: a stereo camera clip, a stereo wav recorder, a
/// stereo mp3 bed, a mono wav and a still.
fn media(dir: &Path) {
    let p = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let stereo = "aevalsrc=0.2*sin(2*PI*440*t)|0.2*sin(2*PI*660*t):s=48000:d=12";
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=160x90:r=24000/1001:d=12",
        "-f",
        "lavfi",
        "-i",
        stereo,
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &p("cam.mp4"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        stereo,
        &p("recorder.wav"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        stereo,
        "-c:a",
        "libmp3lame",
        "-b:a",
        "64k",
        &p("bed.mp3"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=330:sample_rate=48000:duration=12",
        &p("voice.wav"),
    ]);
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=blue:s=160x90:d=1",
        "-frames:v",
        "1",
        &p("title.png"),
    ]);
}

/// Exploded index, exploded count, track type, output channel, clip count.
type TrackLayout<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    usize,
);

fn attr<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}
fn child<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    node.children.iter().find(|c| c.name == name)
}
fn kids<'a>(node: &'a Node, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
    node.children.iter().filter(move |c| c.name == name)
}
fn path<'a>(node: &'a Node, names: &[&str]) -> &'a Node {
    names.iter().fold(node, |node, name| {
        child(node, name).unwrap_or_else(|| panic!("missing <{name}> in <{}>", node.name))
    })
}
fn text<'a>(node: &'a Node, names: &[&str]) -> &'a str {
    path(node, names).text.trim()
}
fn walk<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(node);
    for c in &node.children {
        walk(c, out);
    }
}
fn sequence(root: &Node) -> &Node {
    child(root, "sequence").unwrap()
}
fn tracks<'a>(root: &'a Node, kind: &'a str) -> Vec<&'a Node> {
    kids(path(sequence(root), &["media", kind]), "track").collect()
}

/// Every link resolves to a clipitem at its stated media/track/clip index and
/// every member of a group lists the same group.
fn check_links(root: &Node) {
    let mut place = HashMap::new();
    for kind in ["video", "audio"] {
        for (t, track) in tracks(root, kind).into_iter().enumerate() {
            for (k, item) in kids(track, "clipitem").enumerate() {
                place.insert(
                    attr(item, "id").unwrap().to_owned(),
                    (kind, t + 1, k + 1, item),
                );
            }
        }
    }
    for (id, (_, _, _, item)) in &place {
        let group: Vec<&str> = kids(item, "link")
            .map(|l| text(l, &["linkclipref"]))
            .collect();
        if group.is_empty() {
            continue;
        }
        assert!(
            group.contains(&id.as_str()),
            "{id} must list itself in its group"
        );
        assert!(
            group.len() >= 2,
            "{id}: a lone clipitem must not carry links"
        );
        for link in kids(item, "link") {
            let target = text(link, &["linkclipref"]);
            let (kind, track, index, other) = place[target];
            assert_eq!(text(link, &["mediatype"]), kind, "{id} -> {target}");
            assert_eq!(
                text(link, &["trackindex"]),
                track.to_string(),
                "{id} -> {target}"
            );
            assert_eq!(
                text(link, &["clipindex"]),
                index.to_string(),
                "{id} -> {target}"
            );
            let other_group: Vec<&str> = kids(other, "link")
                .map(|l| text(l, &["linkclipref"]))
                .collect();
            assert_eq!(
                group, other_group,
                "{id} and {target} disagree on their link group"
            );
        }
    }
}

/// Each file's full definition comes at its first use, exactly once.
fn check_files(root: &Node) -> HashMap<String, &Node> {
    let mut all = Vec::new();
    walk(root, &mut all);
    let mut defined = HashMap::new();
    for file in all.into_iter().filter(|n| n.name == "file") {
        let id = attr(file, "id").unwrap().to_owned();
        let full = child(file, "pathurl").is_some();
        assert_eq!(
            full,
            !defined.contains_key(&id),
            "{id}: first use defines, later uses refer"
        );
        if full {
            defined.insert(id, file);
        }
    }
    defined
}

fn export_xml(edit: Value, dir: &Path, options: &ExportOptions) -> (String, Value) {
    let validated = timeline::validate(&timeline::parse(edit).unwrap(), dir).unwrap();
    let output = dir.join("edit.xml");
    let result = export::export_with(&validated, "premiere", &output, true, options).unwrap();
    (std::fs::read_to_string(output).unwrap(), result)
}

fn tester_edit() -> Value {
    json!({"version":1,"name":"Tester","width":160,"height":90,"fps":{"num":24000,"den":1001},
        "video":[
            {"clips":[{"start":0,"source":"cam.mp4","duration":5},{"start":5,"source":"cam.mp4","source_in":5,"duration":5}]},
            {"clips":[{"start":1,"source":"title.png","duration":2,"opacity":0.5}]}],
        "audio":[
            {"clips":[{"start":0,"source":"recorder.wav","duration":10}]},
            {"clips":[{"start":0,"source":"bed.mp3","duration":10,"volume":0.3}]},
            {"clips":[{"start":0,"source":"voice.wav","duration":3},{"start":4,"source":"recorder.wav","source_in":1,"duration":2}]}]})
}

#[test]
fn stereo_sound_is_written_as_exploded_pairs_like_premiere() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let (xml, result) = export_xml(tester_edit(), dir.path(), &ExportOptions::default());
    let root = import::parse_xml(&xml).unwrap();
    let seq = sequence(&root);
    assert_eq!(attr(seq, "explodedTracks"), Some("true"));
    assert_eq!(text(seq, &["rate", "timebase"]), "24");
    assert_eq!(text(seq, &["rate", "ntsc"]), "TRUE");

    // Sequence audio: two output channels, each its own group (as Premiere writes).
    let audio = path(seq, &["media", "audio"]);
    let order: Vec<&str> = audio
        .children
        .iter()
        .map(|c| c.name.as_str())
        .take(3)
        .collect();
    assert_eq!(order, ["numOutputChannels", "format", "outputs"]);
    let groups: Vec<(&str, &str, &str)> = kids(path(audio, &["outputs"]), "group")
        .map(|g| {
            (
                text(g, &["index"]),
                text(g, &["numchannels"]),
                text(g, &["channel", "index"]),
            )
        })
        .collect();
    assert_eq!(groups, [("1", "1", "1"), ("2", "1", "2")]);

    // V1's sound, recorder, bed: stereo pairs; the third track splits its
    // stereo recorder (pair) from its mono voice (mono track).
    let audio_tracks = tracks(&root, "audio");
    let layout: Vec<TrackLayout<'_>> = audio_tracks
        .iter()
        .map(|t| {
            (
                attr(t, "currentExplodedTrackIndex"),
                attr(t, "totalExplodedTrackCount"),
                attr(t, "premiereTrackType"),
                child(t, "outputchannelindex").map(|n| n.text.trim()),
                kids(t, "clipitem").count(),
            )
        })
        .collect();
    let pair = |n| {
        [
            (Some("0"), Some("2"), Some("Stereo"), Some("1"), n),
            (Some("1"), Some("2"), Some("Stereo"), Some("2"), n),
        ]
    };
    let mut expected = Vec::new();
    expected.extend(pair(2));
    expected.extend(pair(1));
    expected.extend(pair(1));
    expected.extend(pair(1));
    expected.push((Some("0"), Some("1"), Some("Mono"), None, 1));
    assert_eq!(layout, expected);
    let last = *audio_tracks.last().unwrap();
    assert_eq!(last.children.last().unwrap().name, "locked");

    // One clipitem per channel, reading that channel, same timing as its partner.
    for pair in audio_tracks[..8].chunks(2) {
        for (left, right) in kids(pair[0], "clipitem").zip(kids(pair[1], "clipitem")) {
            for (channel, item) in [("1", left), ("2", right)] {
                assert_eq!(attr(item, "premiereChannelType"), Some("stereo"));
                assert_eq!(text(item, &["sourcetrack", "mediatype"]), "audio");
                assert_eq!(text(item, &["sourcetrack", "trackindex"]), channel);
            }
            for field in ["start", "end", "in", "out", "duration"] {
                assert_eq!(text(left, &[field]), text(right, &[field]), "{field}");
            }
            let names: Vec<&str> = left.children.iter().map(|c| c.name.as_str()).collect();
            let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
            assert!(at("name") < at("enabled") && at("enabled") < at("duration"));
            assert!(at("out") < at("file") && at("file") < at("sourcetrack"));
            assert!(at("sourcetrack") < at("link"), "{names:?}");
        }
    }
    let voice = child(last, "clipitem").unwrap();
    assert_eq!(attr(voice, "premiereChannelType"), Some("mono"));
    assert_eq!(text(voice, &["sourcetrack", "trackindex"]), "1");
    assert!(child(voice, "link").is_none());

    // Linked sound: the camera clip groups its picture with both channels.
    let v1 = tracks(&root, "video")[0];
    let cam = kids(v1, "clipitem").next().unwrap();
    let links: Vec<(&str, &str, &str)> = kids(cam, "link")
        .map(|l| {
            (
                text(l, &["mediatype"]),
                text(l, &["trackindex"]),
                text(l, &["clipindex"]),
            )
        })
        .collect();
    assert_eq!(
        links,
        [
            ("video", "1", "1"),
            ("audio", "1", "1"),
            ("audio", "2", "1")
        ]
    );
    let title = child(tracks(&root, "video")[1], "clipitem").unwrap();
    assert!(child(title, "link").is_none());
    // The recorder pair links only its two channels.
    let recorder = child(audio_tracks[2], "clipitem").unwrap();
    let group: Vec<(&str, &str)> = kids(recorder, "link")
        .map(|l| (text(l, &["mediatype"]), text(l, &["trackindex"])))
        .collect();
    assert_eq!(group, [("audio", "3"), ("audio", "4")]);
    check_links(&root);

    // Audio-only files describe their sound and no picture; the mp3's and
    // wav's definitions come at their first use.
    let files = check_files(&root);
    let by_name = |n: &str| {
        files
            .values()
            .find(|f| text(f, &["name"]) == n)
            .copied()
            .unwrap_or_else(|| panic!("{n}"))
    };
    for (name, channels) in [("recorder.wav", "2"), ("bed.mp3", "2"), ("voice.wav", "1")] {
        let file = by_name(name);
        assert!(child(path(file, &["media"]), "video").is_none(), "{name}");
        assert_eq!(
            text(file, &["media", "audio", "channelcount"]),
            channels,
            "{name}"
        );
        assert_eq!(
            text(
                file,
                &["media", "audio", "samplecharacteristics", "samplerate"]
            ),
            "48000"
        );
        assert_eq!(text(file, &["rate", "timebase"]), "24");
        assert!(
            text(file, &["duration"]).parse::<u64>().unwrap() >= 287,
            "{name}"
        );
        assert!(text(file, &["pathurl"]).starts_with("file://localhost/"));
    }
    assert!(child(path(by_name("cam.mp4"), &["media"]), "video").is_some());

    let warnings = result["warnings"].to_string();
    assert!(warnings.contains("separate mono audio track"), "{warnings}");
}

#[test]
fn exported_pairs_import_back_as_one_clip_each() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    export_xml(tester_edit(), dir.path(), &ExportOptions::default());
    let (edit, notes) = import::import(&dir.path().join("edit.xml")).unwrap();
    let video = edit["video"].as_array().unwrap();
    assert_eq!(video[0]["clips"].as_array().unwrap().len(), 2);
    // Linked camera sound folds back into its video clips.
    assert!(
        video[0]["clips"][0]
            .get("audio")
            .is_none_or(|a| a["volume"] != json!(0))
    );
    let audio: Vec<Vec<&str>> = edit["audio"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            t["clips"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    Path::new(c["source"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                })
                .collect()
        })
        .collect();
    assert_eq!(
        audio,
        [
            vec!["recorder.wav"],
            vec!["bed.mp3"],
            vec!["recorder.wav"],
            vec!["voice.wav"]
        ],
        "{notes:?}"
    );
    assert!((edit["audio"][1]["clips"][0]["volume"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    // The imported edit validates against the same media.
    let reimported = dir.path().join("reimported.json");
    std::fs::write(&reimported, serde_json::to_vec(&edit).unwrap()).unwrap();
    timeline::load_and_validate(&reimported).unwrap();
}

/// A Premiere-exported sequence (OpenTimelineIO test data, Apache-2.0) is the
/// structural reference: skywrite's stereo pairs must match it.
#[test]
fn golden_premiere_export_matches_and_imports() {
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/premiere/premiere_stereo_audio.xml");
    let golden = import::parse_xml(&std::fs::read_to_string(&golden_path).unwrap()).unwrap();
    check_links(&golden);
    check_files(&golden);

    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let (xml, _) = export_xml(tester_edit(), dir.path(), &ExportOptions::default());
    let ours = import::parse_xml(&xml).unwrap();

    /// What Premiere's importer keys on, ignoring its UI-state attributes.
    fn shape(root: &Node) -> Vec<String> {
        let seq = sequence(root);
        let audio = path(seq, &["media", "audio"]);
        let mut out = vec![
            format!("exploded={:?}", attr(seq, "explodedTracks")),
            format!(
                "outputs={:?}",
                kids(path(audio, &["outputs"]), "group")
                    .map(|g| (
                        text(g, &["index"]),
                        text(g, &["numchannels"]),
                        text(g, &["downmix"]),
                        text(g, &["channel", "index"])
                    ))
                    .collect::<Vec<_>>()
            ),
        ];
        for (index, track) in tracks(root, "audio").into_iter().take(2).enumerate() {
            out.push(format!(
                "track{index}: {:?} {:?} {:?} out={:?}",
                attr(track, "currentExplodedTrackIndex"),
                attr(track, "totalExplodedTrackCount"),
                attr(track, "premiereTrackType"),
                child(track, "outputchannelindex").map(|n| n.text.trim())
            ));
            let item = child(track, "clipitem").unwrap();
            let core = [
                "name",
                "enabled",
                "duration",
                "rate",
                "start",
                "end",
                "in",
                "out",
                "file",
                "sourcetrack",
                "link",
            ];
            let mut order: Vec<&str> = item
                .children
                .iter()
                .map(|c| c.name.as_str())
                .filter(|n| core.contains(n))
                .collect();
            order.dedup();
            out.push(format!(
                "item{index}: {:?} {order:?} source={:?}/{} links={:?}",
                attr(item, "premiereChannelType"),
                text(item, &["sourcetrack", "mediatype"]),
                text(item, &["sourcetrack", "trackindex"]),
                kids(item, "link")
                    .map(|l| (
                        text(l, &["mediatype"]),
                        text(l, &["trackindex"]),
                        l.children.iter().any(|c| c.name == "groupindex")
                    ))
                    .collect::<Vec<_>>()
            ));
        }
        out
    }
    assert_eq!(shape(&ours), shape(&golden));

    // The golden file imports: V1 keeps its linked stereo sound, and each
    // stereo wav pair becomes one audio clip.
    let (edit, notes) = import::import(&golden_path).unwrap();
    let video = edit["video"].as_array().unwrap();
    assert_eq!(video.len(), 1);
    let cam = &video[0]["clips"][0];
    assert!(
        cam["source"]
            .as_str()
            .unwrap()
            .ends_with("sc01_sh010_anim.mov")
    );
    assert!(
        cam.get("audio").is_none_or(|a| a["volume"] != json!(0)),
        "{cam}"
    );
    let audio: Vec<Vec<String>> = edit["audio"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            t["clips"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    c["source"]
                        .as_str()
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_owned()
                })
                .collect()
        })
        .collect();
    assert_eq!(
        audio,
        [vec!["sc01_placeholder.wav"], vec!["track_08.wav"]],
        "{notes:?}"
    );
}

/// Back-to-back clips in round seconds share their boundary frame at NTSC
/// rates: a start and an end are each rounded once.
#[test]
fn round_second_clips_never_overlap_or_gap_at_ntsc_rates() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    // Rounding start and duration separately overlapped at 21 s (23.976),
    // 17 s (29.97) and 9 s (59.94); these boundaries cover all three.
    let durations = [4.0, 5.0, 8.0, 9.0, 10.0, 11.0];
    let mut start = 0.0;
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for duration in durations {
        video.push(json!({"start":start,"source":"title.png","duration":duration}));
        audio.push(json!({"start":start,"source":"voice.wav","duration":duration}));
        start += duration;
    }
    for num in [24000, 30000, 60000] {
        let fps = Rational { num, den: 1001 };
        let edit = json!({"version":1,"name":"Grid","width":160,"height":90,"fps":fps,
            "video":[{"clips":video}],"audio":[{"clips":audio}]});
        let validated = timeline::validate(&timeline::parse(edit).unwrap(), dir.path())
            .unwrap_or_else(|e| panic!("{num}: {e:#}"));
        for clips in [
            validated.video_tracks()[0]
                .clips
                .iter()
                .map(|c| (c.start_frames, c.end_frames()))
                .collect::<Vec<_>>(),
            validated.audio_tracks()[0]
                .clips
                .iter()
                .map(|c| (c.start_frames, c.end_frames()))
                .collect(),
        ] {
            assert_eq!(clips[0].0, 0);
            for pair in clips.windows(2) {
                assert_eq!(pair[0].1, pair[1].0, "{num}: {clips:?}");
            }
            assert_eq!(clips.last().unwrap().1, fps.frames(start), "{num}");
        }
        let report = validated.summary();
        assert!(!report.to_string().contains("overlap"), "{num}: {report}");

        let xml_path = dir.path().join(format!("grid{num}.xml"));
        export::export(&validated, "premiere", &xml_path, true).unwrap();
        let root = import::parse_xml(&std::fs::read_to_string(&xml_path).unwrap()).unwrap();
        for kind in ["video", "audio"] {
            for track in tracks(&root, kind) {
                let items: Vec<(u64, u64)> = kids(track, "clipitem")
                    .map(|c| {
                        (
                            text(c, &["start"]).parse().unwrap(),
                            text(c, &["end"]).parse().unwrap(),
                        )
                    })
                    .collect();
                for pair in items.windows(2) {
                    assert_eq!(pair[0].1, pair[1].0, "{num} {kind}: {items:?}");
                }
            }
        }

        let fcpxml_path = dir.path().join(format!("grid{num}.fcpxml"));
        export::export(&validated, "fcpxml", &fcpxml_path, true).unwrap();
        let fcpxml = std::fs::read_to_string(&fcpxml_path).unwrap();
        for clip in &validated.video_tracks()[0].clips {
            for value in [
                fps.fcpxml(clip.start_frames),
                fps.fcpxml(clip.duration_frames),
            ] {
                assert!(fcpxml.contains(&format!("\"{value}\"")), "{num}: {value}");
            }
        }
    }
}

#[test]
fn media_root_rewrites_urls_beneath_the_editors_folder() {
    let dir = tempfile::tempdir().unwrap();
    let media_dir = dir.path().join("media");
    std::fs::create_dir_all(media_dir.join("sound")).unwrap();
    std::fs::create_dir_all(dir.path().join("edits")).unwrap();
    media(&media_dir);
    std::fs::rename(
        media_dir.join("recorder.wav"),
        media_dir.join("sound/recorder.wav"),
    )
    .unwrap();
    std::fs::copy(
        media_dir.join("voice.wav"),
        media_dir.join("sound/Voice.WAV"),
    )
    .unwrap();
    let edit = json!({"version":1,"name":"Root","width":160,"height":90,"fps":{"num":24000,"den":1001},
        "video":[{"clips":[{"start":0,"source":"../media/cam.mp4","duration":2}]}],
        "audio":[{"clips":[{"start":0,"source":"../media/sound/recorder.wav","duration":2}]},
                 {"clips":[{"start":0,"source":"../media/voice.wav","duration":1},{"start":1,"source":"../media/sound/Voice.WAV","duration":1}]}]});
    let edits = dir.path().join("edits");
    let validated = timeline::validate(&timeline::parse(edit).unwrap(), &edits).unwrap();
    let base = dir.path().canonicalize().unwrap();

    let options = ExportOptions {
        media_root: Some("/Users/editor/My Film".into()),
        stretcher: None,
    };
    let xml_path = edits.join("cut.xml");
    let result = export::export_with(&validated, "premiere", &xml_path, false, &options).unwrap();
    assert_eq!(result["media_root"], json!("/Users/editor/My Film"));
    assert_eq!(result["media_base"], json!(base));
    let xml = std::fs::read_to_string(&xml_path).unwrap();
    let root = import::parse_xml(&xml).unwrap();
    let mut urls: Vec<String> = check_files(&root)
        .values()
        .map(|f| text(f, &["pathurl"]).to_owned())
        .collect();
    urls.sort();
    assert_eq!(
        urls,
        [
            "file://localhost/Users/editor/My%20Film/media/cam.mp4",
            "file://localhost/Users/editor/My%20Film/media/sound/Voice.WAV",
            "file://localhost/Users/editor/My%20Film/media/sound/recorder.wav",
            "file://localhost/Users/editor/My%20Film/media/voice.wav",
        ]
    );
    // Same-named files make Premiere's Locate ambiguous: warned.
    let warnings = result["warnings"].to_string();
    assert!(
        warnings
            .to_lowercase()
            .contains("share the file name voice.wav"),
        "{warnings}"
    );
    // The import reads the editor's paths back.
    let (imported, _) = import::import(&xml_path).unwrap();
    assert_eq!(
        imported["video"][0]["clips"][0]["source"],
        json!("/Users/editor/My Film/media/cam.mp4")
    );

    let fcpxml_path = edits.join("cut.fcpxml");
    let windows = ExportOptions {
        media_root: Some("D:\\Projects\\film".into()),
        stretcher: None,
    };
    export::export_with(&validated, "fcpxml", &fcpxml_path, false, &windows).unwrap();
    let fcpxml = std::fs::read_to_string(&fcpxml_path).unwrap();
    assert!(
        fcpxml.contains("src=\"file:///D:/Projects/film/media/sound/recorder.wav\""),
        "{fcpxml}"
    );
    export::export_with(&validated, "premiere", &xml_path, true, &windows).unwrap();
    assert!(
        std::fs::read_to_string(&xml_path)
            .unwrap()
            .contains("file://localhost/D%3a/Projects/film/media/cam.mp4")
    );

    // Default: this machine's absolute paths.
    export::export(&validated, "premiere", &xml_path, true).unwrap();
    let local = url::Url::from_file_path(base.join("media/cam.mp4")).unwrap();
    assert!(
        std::fs::read_to_string(&xml_path)
            .unwrap()
            .contains(&local.as_str().replacen("file://", "file://localhost", 1))
    );

    // Relative roots and non-interchange targets are refused.
    let relative = ExportOptions {
        media_root: Some("media".into()),
        stretcher: None,
    };
    let error =
        export::export_with(&validated, "premiere", &xml_path, true, &relative).unwrap_err();
    assert!(
        format!("{error:#}").contains("absolute folder"),
        "{error:#}"
    );
    let error =
        export::export_with(&validated, "ffmpeg", &edits.join("r.py"), true, &options).unwrap_err();
    assert!(
        format!("{error:#}").contains("only to --to fcpxml or --to premiere"),
        "{error:#}"
    );
}

#[test]
fn media_root_flag_reaches_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    let edit = json!({"version":1,"name":"Cli","width":160,"height":90,"fps":{"num":24,"den":1},
        "video":[{"clips":[{"start":0,"source":"cam.mp4","duration":2}]}]});
    let edit_path = dir.path().join("edit.json");
    std::fs::write(&edit_path, edit.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_skywrite"))
        .args(["edit", "export"])
        .arg(&edit_path)
        .args([
            "--to",
            "premiere",
            "--media-root",
            "/Volumes/Work/film",
            "-o",
        ])
        .arg(dir.path().join("cut.xml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["media_root"], json!("/Volumes/Work/film"));
    let xml = std::fs::read_to_string(dir.path().join("cut.xml")).unwrap();
    assert!(xml.contains("<pathurl>file://localhost/Volumes/Work/film/cam.mp4</pathurl>"));
}

/// Rounding start and end once each can lengthen a clip by up to one frame;
/// a clip placed off the grid that runs its source to the end still validates,
/// and interchange exports end it inside the media.
#[test]
fn off_grid_clip_running_to_media_end_validates() {
    let dir = tempfile::tempdir().unwrap();
    media(dir.path());
    // voice.wav is exactly 12 s. Start 0.4 frames (rounds to 0) and end 360.6
    // frames (rounds to 361): 361 frames, one frame past the media end.
    let edit = json!({"version":1,"name":"Tail","width":160,"height":90,"fps":{"num":30,"den":1},
        "audio":[{"clips":[{"start":0.4/30.0,"source":"voice.wav","duration":360.2/30.0}]}]});
    let validated = timeline::validate(&timeline::parse(edit).unwrap(), dir.path()).unwrap();
    let clip = &validated.audio_tracks()[0].clips[0];
    assert_eq!((clip.start_frames, clip.duration_frames), (0, 361));
    let output = dir.path().join("tail.xml");
    let result = export::export(&validated, "premiere", &output, true).unwrap();
    assert!(
        result["warnings"]
            .to_string()
            .contains("ends 1 frame(s) earlier"),
        "{result}"
    );
    let root = import::parse_xml(&std::fs::read_to_string(output).unwrap()).unwrap();
    let item = child(tracks(&root, "audio")[0], "clipitem").unwrap();
    assert_eq!(text(item, &["out"]), "360");
}

#[test]
fn wide_sources_are_written_as_stereo_pairs_with_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("quad.wav");
    ffmpeg(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-af",
        "pan=4c|c0=c0|c1=c0|c2=c0|c3=c0",
        wav.to_str().unwrap(),
    ]);
    let edit = json!({"version":1,"name":"Quad","width":160,"height":90,"fps":{"num":25,"den":1},
        "audio":[{"clips":[{"start":0,"source":"quad.wav","duration":2}]}]});
    let (xml, result) = export_xml(edit, dir.path(), &ExportOptions::default());
    let root = import::parse_xml(&xml).unwrap();
    let audio = tracks(&root, "audio");
    assert_eq!(audio.len(), 2);
    for (index, track) in audio.iter().enumerate() {
        assert_eq!(attr(track, "premiereTrackType"), Some("Stereo"));
        let item = child(track, "clipitem").unwrap();
        assert_eq!(
            text(item, &["sourcetrack", "trackindex"]),
            (index + 1).to_string()
        );
    }
    assert!(
        result["warnings"]
            .to_string()
            .contains("more than two audio channels"),
        "{result}"
    );
}

#[test]
fn media_root_warns_when_sources_leave_the_edit_tree() {
    // Media under target/ and the edit under the system temp folder usually
    // share only the filesystem root.
    let media_dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let edit_dir = tempfile::tempdir().unwrap();
    media(media_dir.path());
    let source = media_dir.path().join("voice.wav");
    let edit = json!({"version":1,"name":"Far","width":160,"height":90,"fps":{"num":25,"den":1},
        "audio":[{"clips":[{"start":0,"source":source,"duration":1}]}]});
    let options = ExportOptions {
        media_root: Some("/Users/editor/film".into()),
        stretcher: None,
    };
    let (xml, result) = export_xml(edit, edit_dir.path(), &options);
    let base = Path::new(result["media_base"].as_str().unwrap());
    let canonical = source.canonicalize().unwrap();
    let relative = canonical.strip_prefix(base).unwrap();
    let expected =
        url::Url::from_file_path(Path::new("/Users/editor/film").join(relative)).unwrap();
    assert!(
        xml.contains(&expected.as_str().replacen("file://", "file://localhost", 1)),
        "{xml}"
    );
    let warned = result["warnings"].to_string().contains("filesystem root");
    assert_eq!(warned, base.parent().is_none(), "{result}");
}
