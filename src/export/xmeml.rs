//! Final Cut Pro 7 XML ("xmeml" version 5), the interchange Premiere Pro imports.
//!
//! Every video track and its linked sound, plus every audio track, keeps its
//! own layer. Speed, opacity, fades and gain envelopes are written as the
//! standard Time Remap, Opacity and Audio Levels effects. Timing uses whole
//! frames at the sequence rate, so subframe source in points are rounded.
use super::{
    media_url::{MediaUrls, Style},
    xml,
};
use crate::timeline::{
    Keyframe, MediaKind, Rational, ValidatedEdit, ValidatedMix, ValidatedVideoClip,
};
use anyhow::{Result, bail};
use std::{collections::HashMap, fmt::Write as _, path::Path};

struct Rate {
    timebase: u32,
    ntsc: bool,
}

fn rate(fps: Rational) -> Result<Rate> {
    if fps.num.is_multiple_of(fps.den) {
        return Ok(Rate {
            timebase: fps.num / fps.den,
            ntsc: false,
        });
    }
    // NTSC rates are N*1000/1001, possibly written unreduced.
    let (num, den) = (fps.num as u64, fps.den as u64);
    if (num * 1001).is_multiple_of(den * 1000) {
        let timebase = num * 1001 / (den * 1000);
        if timebase > 0 {
            return Ok(Rate {
                timebase: timebase as u32,
                ntsc: true,
            });
        }
    }
    bail!(
        "Premiere XML supports integer or NTSC (N*1000/1001) frame rates, not {}/{}",
        fps.num,
        fps.den
    )
}

fn rate_xml(rate: &Rate) -> String {
    format!(
        "<rate><timebase>{}</timebase><ntsc>{}</ntsc></rate>",
        rate.timebase,
        if rate.ntsc { "TRUE" } else { "FALSE" }
    )
}

fn name(path: &Path) -> String {
    xml(path.file_name().and_then(|v| v.to_str()).unwrap_or("Media"))
}

struct Files<'a> {
    ids: HashMap<&'a Path, usize>,
    urls: &'a MediaUrls,
}
impl<'a> Files<'a> {
    /// First use carries the full definition; later uses reference its id.
    fn element(
        &mut self,
        path: &'a Path,
        info: &crate::media::MediaInfo,
        still_frames: Option<u64>,
        fps: Rational,
        rate: &Rate,
    ) -> Result<String> {
        if let Some(id) = self.ids.get(path) {
            return Ok(format!("<file id=\"file-{id}\"/>"));
        }
        let id = self.ids.len() + 1;
        self.ids.insert(path, id);
        let frames = still_frames.unwrap_or_else(|| fps.frames(info.duration));
        let mut media = String::new();
        if let Some(video) = info.video() {
            write!(
                media,
                "<video><samplecharacteristics>{}<width>{}</width><height>{}</height></samplecharacteristics></video>",
                rate_xml(rate),
                video.width.unwrap_or(0),
                video.height.unwrap_or(0)
            )?;
        }
        if let Some(audio) = info.audio() {
            write!(
                media,
                "<audio><samplecharacteristics><depth>16</depth><samplerate>{}</samplerate></samplecharacteristics><channelcount>{}</channelcount></audio>",
                audio.sample_rate.unwrap_or(48000),
                audio.channels.unwrap_or(2)
            )?;
        }
        Ok(format!(
            "<file id=\"file-{id}\"><name>{}</name><pathurl>{}</pathurl>{}<duration>{frames}</duration><timecode>{}<string>00:00:00:00</string><frame>0</frame><displayformat>NDF</displayformat></timecode><media>{media}</media></file>",
            name(path),
            xml(self.urls.get(path, Style::Premiere)),
            rate_xml(rate),
            rate_xml(rate)
        ))
    }
}

fn time_remap(speed: f64, media: &str) -> String {
    format!(
        "<filter><effect><name>Time Remap</name><effectid>timeremap</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>{media}</mediatype><parameter authoringApp=\"PremierePro\"><parameterid>variablespeed</parameterid><name>variablespeed</name><valuemin>0</valuemin><valuemax>1</valuemax><value>0</value></parameter><parameter authoringApp=\"PremierePro\"><parameterid>speed</parameterid><name>speed</name><valuemin>-100000</valuemin><valuemax>100000</valuemax><value>{:.6}</value></parameter><parameter authoringApp=\"PremierePro\"><parameterid>reverse</parameterid><name>reverse</name><value>FALSE</value></parameter><parameter authoringApp=\"PremierePro\"><parameterid>frameblending</parameterid><name>frameblending</name><value>FALSE</value></parameter></effect></filter>",
        speed * 100.0
    )
}

/// Basic Motion Scale that fits the picture inside the sequence, preserving
/// aspect ratio, as the renderer does; Premiere otherwise shows media at its
/// own size (a 4K clip in an HD sequence is cropped to its center quarter).
fn motion(clip: &ValidatedVideoClip, width: u32, height: u32) -> String {
    let Some((w, h)) = clip.media.video().and_then(|v| v.display_size()) else {
        return String::new();
    };
    if w == 0 || h == 0 {
        return String::new();
    }
    let scale = (width as f64 / w as f64).min(height as f64 / h as f64) * 100.0;
    if (scale - 100.0).abs() < 1e-6 {
        return String::new();
    }
    format!(
        "<filter><effect><name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype><parameter authoringApp=\"PremierePro\"><parameterid>scale</parameterid><name>Scale</name><valuemin>0</valuemin><valuemax>1000</valuemax><value>{scale:.6}</value></parameter></effect></filter>"
    )
}

fn keyframes(points: &[(u64, f64)]) -> String {
    points
        .iter()
        .map(|(when, value)| {
            format!("<keyframe><when>{when}</when><value>{value:.6}</value></keyframe>")
        })
        .collect()
}

/// Keyframe `when` values are clip media frames, offset by the in point.
fn opacity(clip: &ValidatedVideoClip, fps: Rational, in_frame: u64) -> String {
    if clip.opacity == 1.0 && clip.fade_in == 0.0 && clip.fade_out == 0.0 {
        return String::new();
    }
    let duration = fps.seconds(clip.duration_frames);
    let mut points = Vec::new();
    if clip.fade_in > 0.0 || clip.fade_out > 0.0 {
        let mut times = vec![(
            0.0,
            if clip.fade_in > 0.0 {
                0.0
            } else {
                clip.opacity
            },
        )];
        if clip.fade_in > 0.0 {
            times.push((clip.fade_in, clip.opacity));
        }
        if clip.fade_out > 0.0 {
            times.push((duration - clip.fade_out, clip.opacity));
        }
        times.push((
            duration,
            if clip.fade_out > 0.0 {
                0.0
            } else {
                clip.opacity
            },
        ));
        let mut last = None;
        for (time, value) in times {
            let when = in_frame + fps.frames(time);
            if last != Some(when) {
                points.push((when, value * 100.0));
                last = Some(when);
            }
        }
    }
    format!(
        "<filter><effect><name>Opacity</name><effectid>opacity</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype><parameter authoringApp=\"PremierePro\"><parameterid>opacity</parameterid><name>opacity</name><valuemin>0</valuemin><valuemax>100</valuemax><value>{:.6}</value>{}</parameter></effect></filter>",
        clip.opacity * 100.0,
        keyframes(&points)
    )
}

fn levels(mix: &ValidatedMix, fps: Rational, in_frame: u64, duration: f64) -> String {
    if mix.is_unity() {
        return String::new();
    }
    let mut points: Vec<(u64, f64)> = Vec::new();
    if !(mix.keyframes.is_empty() && mix.fade_in == 0.0 && mix.fade_out == 0.0) {
        for Keyframe { time, volume } in mix.breakpoints(duration) {
            let when = in_frame + fps.frames(time);
            match points.last_mut() {
                Some(last) if last.0 == when => last.1 = volume,
                _ => points.push((when, volume)),
            }
        }
    }
    format!(
        "<filter><effect><name>Audio Levels</name><effectid>audiolevels</effectid><effectcategory>audiolevels</effectcategory><effecttype>audiolevels</effecttype><mediatype>audio</mediatype><parameter><parameterid>level</parameterid><name>Level</name><valuemin>0</valuemin><valuemax>3.98109</valuemax><value>{:.6}</value>{}</parameter></effect></filter>",
        mix.volume.min(3.98109),
        keyframes(&points)
    )
}

/// One sound use on the timeline, before it is split into channel clipitems.
struct Sound<'a> {
    source: &'a Path,
    media: &'a crate::media::MediaInfo,
    timing: String,
    speed: f64,
    levels: String,
    /// The video clipitem this sound is linked to: (id, track, clip index).
    video: Option<(usize, usize, usize)>,
}

/// A run of xmeml audio tracks for one timeline layer: an exploded stereo
/// pair (one track per channel) or a single mono track.
struct Layer<'a> {
    stereo: bool,
    sounds: Vec<Sound<'a>>,
    /// The first xmeml audio track number and the first clipitem id.
    first_track: usize,
    first_id: usize,
}

impl Layer<'_> {
    fn channels(&self) -> usize {
        if self.stereo { 2 } else { 1 }
    }
    fn id(&self, sound: usize, channel: usize) -> usize {
        self.first_id + channel * self.sounds.len() + sound
    }
}

fn channels(info: &crate::media::MediaInfo) -> u32 {
    info.audio().and_then(|a| a.channels).unwrap_or(2)
}

/// Splits one timeline layer into a stereo pair and a mono track as needed.
/// Premiere reads a clip's channels from its exploded track, so a pair holds
/// only stereo (or wider) sources and a mono track only mono ones.
fn split<'a>(sounds: Vec<Sound<'a>>, layers: &mut Vec<Layer<'a>>, mixed: &mut bool) {
    let (stereo, mono): (Vec<_>, Vec<_>) = sounds.into_iter().partition(|s| channels(s.media) >= 2);
    *mixed |= !stereo.is_empty() && !mono.is_empty();
    if !stereo.is_empty() || mono.is_empty() {
        layers.push(Layer {
            stereo: true,
            sounds: stereo,
            first_track: 0,
            first_id: 0,
        });
    }
    if !mono.is_empty() {
        layers.push(Layer {
            stereo: false,
            sounds: mono,
            first_track: 0,
            first_id: 0,
        });
    }
}

fn link(id: usize, media: &str, track: usize, clip_index: usize) -> String {
    format!(
        "<link><linkclipref>clipitem-{id}</linkclipref><mediatype>{media}</mediatype><trackindex>{track}</trackindex><clipindex>{clip_index}</clipindex>{}</link>",
        if media == "audio" {
            "<groupindex>1</groupindex>"
        } else {
            ""
        }
    )
}

/// Every member of a link group lists the whole group: the video clipitem (if
/// any) and one clipitem per channel. A lone clipitem carries no links.
fn links(layer: &Layer<'_>, index: usize) -> String {
    let sound = &layer.sounds[index];
    let mut out = String::new();
    if let Some((id, track, clip_index)) = sound.video {
        out.push_str(&link(id, "video", track, clip_index));
    }
    for channel in 0..layer.channels() {
        out.push_str(&link(
            layer.id(index, channel),
            "audio",
            layer.first_track + channel,
            index + 1,
        ));
    }
    if sound.video.is_none() && layer.channels() == 1 {
        return String::new();
    }
    out
}

const OUTPUTS: &str = "<outputs><group><index>1</index><numchannels>1</numchannels><downmix>0</downmix><channel><index>1</index></channel></group><group><index>2</index><numchannels>1</numchannels><downmix>0</downmix><channel><index>2</index></channel></group></outputs>";

/// Returns XML plus notes about rounding and unverified mappings.
///
/// Audio follows the structure Premiere itself exports (`explodedTracks`):
/// each stereo layer is a pair of tracks with `currentExplodedTrackIndex` 0
/// and 1, `totalExplodedTrackCount` 2 and `outputchannelindex` 1 and 2, and
/// each sound is one clipitem per channel (`sourcetrack` 1 and 2) linked in a
/// group with its video. A single stereo clipitem on an unexploded track is
/// what Premiere silently dropped.
pub(super) fn compile(edit: &ValidatedEdit, urls: &MediaUrls) -> Result<(String, Vec<String>)> {
    let fps = edit.fps;
    let rate = rate(fps)?;
    let rate_tag = rate_xml(&rate);
    let mut notes = Vec::new();
    let mut files = Files {
        ids: HashMap::new(),
        urls,
    };
    let mut subframe = false;
    let mut retimed = false;
    let mut mixed = false;
    let mut wide = false;

    // Plan: number video clipitems, gather sound per layer.
    let mut next_id = 0usize;
    let mut video_ids = Vec::new();
    let mut layers: Vec<Layer<'_>> = Vec::new();
    let item_timing = |in_frame: u64, item_duration: u64, start: u64, end: u64, out: u64| {
        format!(
            "<duration>{item_duration}</duration>{rate_tag}<start>{start}</start><end>{end}</end><in>{in_frame}</in><out>{out}</out>"
        )
    };
    let source_frames = |source_in: f64, speed: f64, subframe: &mut bool| {
        let exact = source_in * fps.value() / speed;
        *subframe |= (exact - exact.round()).abs() > 1e-6;
        exact.round() as u64
    };
    for (t, track) in edit.video_tracks().iter().enumerate() {
        let mut ids = Vec::new();
        let mut sounds = Vec::new();
        for (k, clip) in track.clips.iter().enumerate() {
            next_id += 1;
            ids.push(next_id);
            if clip.kind == MediaKind::Image || !clip.has_audible_sound() {
                continue;
            }
            let in_frame = source_frames(clip.source_in, clip.speed, &mut subframe);
            sounds.push(Sound {
                source: &clip.source,
                media: &clip.media,
                timing: item_timing(
                    in_frame,
                    (fps.frames(clip.media.duration) as f64 / clip.speed).round() as u64,
                    clip.start_frames,
                    clip.end_frames(),
                    in_frame + clip.duration_frames,
                ),
                speed: clip.speed,
                levels: levels(
                    &clip.audio,
                    fps,
                    in_frame,
                    fps.seconds(clip.duration_frames),
                ),
                video: Some((next_id, t + 1, k + 1)),
            });
        }
        video_ids.push(ids);
        if !sounds.is_empty() {
            split(sounds, &mut layers, &mut mixed);
        }
    }
    for track in edit.audio_tracks() {
        let mut sounds = Vec::new();
        for clip in track.clips.iter().filter(|c| c.mix.audible()) {
            let in_frame = source_frames(clip.source_in, clip.speed, &mut subframe);
            sounds.push(Sound {
                source: &clip.source,
                media: &clip.media,
                timing: item_timing(
                    in_frame,
                    (fps.frames(clip.media.duration) as f64 / clip.speed).round() as u64,
                    clip.start_frames,
                    clip.end_frames(),
                    in_frame + clip.duration_frames,
                ),
                speed: clip.speed,
                levels: levels(&clip.mix, fps, in_frame, fps.seconds(clip.duration_frames)),
                video: None,
            });
        }
        split(sounds, &mut layers, &mut mixed);
    }
    let mut track_number = 1;
    for layer in &mut layers {
        layer.first_track = track_number;
        layer.first_id = next_id + 1;
        track_number += layer.channels();
        next_id += layer.channels() * layer.sounds.len();
    }
    // Sound per video clipitem, to write the group from the video side.
    let mut video_links = HashMap::<usize, String>::new();
    for layer in &layers {
        for (index, sound) in layer.sounds.iter().enumerate() {
            if let Some((id, _, _)) = sound.video {
                video_links.insert(id, links(layer, index));
            }
        }
    }

    // Write in document order, so each file's full definition comes first.
    let mut video_tracks = String::new();
    for (t, track) in edit.video_tracks().iter().enumerate() {
        let mut items = String::new();
        for (k, clip) in track.clips.iter().enumerate() {
            let id = video_ids[t][k];
            let (in_frame, item_duration) = if clip.kind == MediaKind::Image {
                (0, clip.duration_frames)
            } else {
                retimed |= clip.speed != 1.0;
                (
                    source_frames(clip.source_in, clip.speed, &mut subframe),
                    (fps.frames(clip.media.duration) as f64 / clip.speed).round() as u64,
                )
            };
            // A still's file is as long as the whole sequence, so every use of
            // it (including a later, longer one) fits inside the file.
            let still = (clip.kind == MediaKind::Image).then_some(edit.duration_frames);
            let file = files.element(&clip.source, &clip.media, still, fps, &rate)?;
            write!(
                items,
                "<clipitem id=\"clipitem-{id}\"><name>{}</name><enabled>TRUE</enabled>{}{}{file}{}{}{}{}</clipitem>",
                name(&clip.source),
                item_timing(
                    in_frame,
                    item_duration,
                    clip.start_frames,
                    clip.end_frames(),
                    in_frame + clip.duration_frames
                ),
                if clip.kind == MediaKind::Image {
                    "<stillframe>TRUE</stillframe>"
                } else {
                    ""
                },
                if clip.speed != 1.0 {
                    time_remap(clip.speed, "video")
                } else {
                    String::new()
                },
                motion(clip, edit.width, edit.height),
                opacity(clip, fps, in_frame),
                video_links.get(&id).map(String::as_str).unwrap_or_default(),
            )?;
        }
        write!(
            video_tracks,
            "<track>{items}<enabled>TRUE</enabled><locked>FALSE</locked></track>"
        )?;
    }
    let mut audio_tracks = String::new();
    for layer in &layers {
        let total = layer.channels();
        for channel in 0..total {
            let mut items = String::new();
            for (index, sound) in layer.sounds.iter().enumerate() {
                retimed |= sound.speed != 1.0;
                wide |= channels(sound.media) > 2;
                let file = files.element(sound.source, sound.media, None, fps, &rate)?;
                write!(
                    items,
                    "<clipitem id=\"clipitem-{}\" premiereChannelType=\"{}\"><name>{}</name><enabled>TRUE</enabled>{}{file}<sourcetrack><mediatype>audio</mediatype><trackindex>{}</trackindex></sourcetrack>{}{}{}</clipitem>",
                    layer.id(index, channel),
                    if layer.stereo { "stereo" } else { "mono" },
                    name(sound.source),
                    sound.timing,
                    channel + 1,
                    if sound.speed != 1.0 {
                        time_remap(sound.speed, "audio")
                    } else {
                        String::new()
                    },
                    sound.levels,
                    links(layer, index),
                )?;
            }
            if layer.stereo {
                write!(
                    audio_tracks,
                    "<track currentExplodedTrackIndex=\"{channel}\" totalExplodedTrackCount=\"2\" premiereTrackType=\"Stereo\">{items}<enabled>TRUE</enabled><locked>FALSE</locked><outputchannelindex>{}</outputchannelindex></track>",
                    channel + 1
                )?;
            } else {
                write!(
                    audio_tracks,
                    "<track currentExplodedTrackIndex=\"0\" totalExplodedTrackCount=\"1\" premiereTrackType=\"Mono\">{items}<enabled>TRUE</enabled><locked>FALSE</locked></track>"
                )?;
            }
        }
    }
    if subframe {
        notes.push("Premiere XML uses whole frames: subframe source in points (word-precise cuts) were rounded to the nearest frame. Listen to dialogue cuts after import.".into());
    }
    if retimed {
        notes.push("Retimed clips use the Time Remap speed effect with in/out points in retimed frames; check speed and sync after Premiere import.".into());
    }
    if mixed {
        notes.push("A track mixed mono and stereo sources; Premiere tracks hold one channel layout, so its mono clips were written to a separate mono audio track below its stereo pair.".into());
    }
    if wide {
        notes.push("Sources with more than two audio channels are written as stereo from their first two channels; assign other channels in Premiere (Modify > Audio Channels).".into());
    }
    notes.push("Premiere import of this XML was checked in Premiere Pro 26.5.2 on macOS 26.3.1 (a contributor's test of Skywrite 9dd2c3b, posted on PR #2 on 2026-11-24): stereo and mono audio on their own tracks, Audio Levels keyframes and fades, whole-second cuts at 23.976 fps without gaps or overlaps, --media-root paths linking without a relink, and Time Remap speed. Not covered by that check: captions, Opacity keyframes, the Basic Motion fit-to-sequence Scale (a rotated iPhone reel imported fitted in Premiere Pro 2026, outside that check), other effects and other Premiere versions. Record differences with skywrite nit.".into());
    let markers: String = edit
        .chapters
        .iter()
        .map(|c| {
            format!(
                "<marker><name>{}</name><comment></comment><in>{}</in><out>-1</out></marker>",
                xml(&c.title),
                fps.frames(c.start)
            )
        })
        .collect();
    let out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"5\"><sequence id=\"sequence-1\" explodedTracks=\"true\"><name>{name}</name><duration>{duration}</duration>{rate}<timecode>{rate}<string>00:00:00:00</string><frame>0</frame><displayformat>NDF</displayformat></timecode><media><video><format><samplecharacteristics>{rate}<width>{width}</width><height>{height}</height><anamorphic>FALSE</anamorphic><pixelaspectratio>square</pixelaspectratio><fielddominance>none</fielddominance></samplecharacteristics></format>{video_tracks}</video><audio><numOutputChannels>2</numOutputChannels><format><samplecharacteristics><depth>16</depth><samplerate>48000</samplerate></samplecharacteristics></format>{OUTPUTS}{audio_tracks}</audio></media>{markers}</sequence></xmeml>\n",
        name = xml(&edit.name),
        duration = edit.duration_frames,
        rate = rate_tag,
        width = edit.width,
        height = edit.height,
    );
    Ok((out, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates() {
        let r = rate(Rational {
            num: 30000,
            den: 1001,
        })
        .unwrap();
        assert_eq!((r.timebase, r.ntsc), (30, true));
        let r = rate(Rational { num: 24, den: 1 }).unwrap();
        assert_eq!((r.timebase, r.ntsc), (24, false));
        let r = rate(Rational {
            num: 60000,
            den: 1001,
        })
        .unwrap();
        assert_eq!((r.timebase, r.ntsc), (60, true));
        assert!(rate(Rational { num: 25, den: 2 }).is_err());
    }
}
