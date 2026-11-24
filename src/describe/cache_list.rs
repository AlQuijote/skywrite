//! `skywrite describe cache SOURCE`: a read-only listing of the frame, video
//! and focus entries cached for one source's content.
use super::{FOCUS_DIR, FRAME_DIR, MAX_CACHE_ENTRY_BYTES, VIDEO_DIR, focus};
use anyhow::{Result, ensure};
use clap::{Args, ValueEnum};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Entries read per kind before the listing stops and says so.
const MAX_SCANNED: usize = 200_000;

pub const ABOUT: &str = "List what describe has cached for a source (frame, video and focus entries); read-only, no requests.";
pub const LONG_ABOUT: &str = "List what describe frames / describe video have cached for one source, by its content hash (a renamed or copied file with the same bytes lists the same entries). Read-only: no requests, no key, nothing written except the shared media-hash cache.

Entries are grouped by kind (frame = general frame description, video = general clip description, focus = a --focus answer), provider, model, prompt version, cast (cast_sha256) and, for focus, the focus identity (focus.sha256, instructions_sha256, schema_sha256 and a short preview of the text). Each group gives entries (count), frame_times as compact ranges ({start, end, step, count}: each cached time is start + i*step within 1 microsecond; step null for a single frame) or ranges for video ({start, duration}), total bytes and first/last creation time (unix seconds).

Use this to see what a later run gets for free. To know what one exact command would cost, run that command with --dry-run instead (would_request). Entries written by a Skywrite version without listing metadata count as unattributed_entries until a describe run reads them again.";
pub const AFTER_HELP: &str = "Examples:
  skywrite describe cache clip.mp4
  skywrite describe cache clip.mp4 --kind focus --focus \"bounding box of every visible human face\"
  skywrite describe cache clip.mp4 --model local:qwen3.5-0.8b";

#[derive(Clone, Copy, PartialEq, ValueEnum)]
pub enum Kind {
    Frame,
    Video,
    Focus,
}

#[derive(Args)]
pub struct CacheArgs {
    pub source: PathBuf,
    /// Only entries for this model reference (PROVIDER:MODEL, or a bare OpenRouter ID).
    #[arg(long)]
    pub model: Option<String>,
    /// Only focus entries whose instructions match this text (same normalization as describe --focus; repeatable, joined in order).
    #[arg(long = "focus", value_name = "TEXT")]
    pub focus: Vec<String>,
    /// Only entries of this kind.
    #[arg(long, value_enum)]
    pub kind: Option<Kind>,
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Frame => "frame",
        Kind::Video => "video",
        Kind::Focus => "focus",
    }
}

/// Largest difference between a cached time and its reconstruction from a run.
pub const RUN_TOLERANCE_SECONDS: f64 = 1e-6;

/// Sorted times as runs of equal spacing. A run is published only when every
/// time in it equals `start + i * step` within [`RUN_TOLERANCE_SECONDS`], so a
/// run never implies a time that is not cached.
pub fn compact_times(times: &[f64]) -> Vec<Value> {
    let mut times = times.to_vec();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let fits = |run: &[f64]| {
        let step = (run[run.len() - 1] - run[0]) / (run.len() - 1) as f64;
        run.iter()
            .enumerate()
            .all(|(i, t)| (t - (run[0] + i as f64 * step)).abs() <= RUN_TOLERANCE_SECONDS)
    };
    let mut runs: Vec<Vec<f64>> = Vec::new();
    for time in times {
        if let Some(run) = runs.last_mut() {
            run.push(time);
            if run.len() <= 2 || fits(run) {
                continue;
            }
            run.pop();
        }
        runs.push(vec![time]);
    }
    runs.into_iter()
        .map(|run| {
            let (start, end, count) = (run[0], run[run.len() - 1], run.len());
            let step = (count > 1).then(|| (end - start) / (count - 1) as f64);
            json!({"start":start,"end":end,"step":step,"count":count})
        })
        .collect()
}

struct Group {
    key: Value,
    entries: usize,
    bytes: u64,
    times: Vec<f64>,
    ranges: Vec<(f64, f64)>,
    first: Option<u64>,
    last: Option<u64>,
}

pub fn list(args: &CacheArgs) -> Result<Value> {
    let info = crate::media::probe_for_timing(&args.source)?;
    let sha = crate::cache::media_sha256(&info.path, args.cache_dir.as_deref())?;
    let root = crate::cache::cache_root(args.cache_dir.as_deref())?;
    let model = args
        .model
        .as_deref()
        .map(|m| crate::providers::resolve_reference(m).map(|t| t.reference()))
        .transpose()?;
    let wanted_focus =
        focus::normalize_text(&args.focus, None)?.map(|text| focus::instructions_sha256(&text));
    ensure!(
        wanted_focus.is_none() || args.kind.is_none_or(|k| k == Kind::Focus),
        "--focus filters focus entries; drop --kind or use --kind focus"
    );
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut unattributed = 0usize;
    let mut truncated = false;
    for (dir, kind) in [
        (FRAME_DIR, Kind::Frame),
        (VIDEO_DIR, Kind::Video),
        (FOCUS_DIR, Kind::Focus),
    ] {
        if args.kind.is_some_and(|k| k != kind) || (wanted_focus.is_some() && kind != Kind::Focus) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for (index, entry) in entries.flatten().enumerate() {
            if index >= MAX_SCANNED {
                truncated = true;
                break;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_CACHE_ENTRY_BYTES {
                continue;
            }
            let Some(cached) = super::read_cached(&path) else {
                continue;
            };
            let Some(meta) = cached.meta else {
                unattributed += 1;
                continue;
            };
            if meta["source_sha256"].as_str() != Some(sha.as_str())
                || meta["kind"].as_str() != Some(kind_name(kind))
            {
                continue;
            }
            if model
                .as_deref()
                .is_some_and(|m| meta["model"].as_str() != Some(m))
            {
                continue;
            }
            if let Some(wanted) = &wanted_focus
                && meta["instructions_sha256"].as_str() != Some(wanted.as_str())
            {
                continue;
            }
            let mut key = json!({"kind":meta["kind"],"provider":meta["provider"],"model":meta["model"],
                "prompt_version":meta["prompt_version"],"cast_sha256":meta["cast_sha256"]});
            if kind == Kind::Video || meta["of"] == "video" {
                key["encoding"] = meta["encoding"].clone();
            }
            if kind == Kind::Focus {
                key["of"] = meta["of"].clone();
                key["focus"] = json!({"sha256":meta["focus_sha256"],"instructions_sha256":meta["instructions_sha256"],
                    "schema_sha256":meta["schema_sha256"],"preview":meta["focus_preview"],"prompt_version":meta["focus_prompt_version"]});
            }
            let group = groups.entry(key.to_string()).or_insert_with(|| Group {
                key,
                entries: 0,
                bytes: 0,
                times: Vec::new(),
                ranges: Vec::new(),
                first: None,
                last: None,
            });
            group.entries += 1;
            group.bytes += metadata.len();
            if let Some(time) = meta["time"].as_f64() {
                group.times.push(time);
            }
            if let (Some(start), Some(duration)) =
                (meta["start"].as_f64(), meta["duration"].as_f64())
            {
                group.ranges.push((start, duration));
            }
            if let Some(created) = meta["created_at"].as_u64() {
                group.first = Some(group.first.map_or(created, |f| f.min(created)));
                group.last = Some(group.last.map_or(created, |l| l.max(created)));
            }
        }
    }
    let (mut entries, mut bytes) = (0usize, 0u64);
    let groups: Vec<Value> = groups
        .into_values()
        .map(|mut group| {
            entries += group.entries;
            bytes += group.bytes;
            group.ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut value = group.key;
            value["entries"] = json!(group.entries);
            value["bytes"] = json!(group.bytes);
            value["created"] = json!({"first":group.first,"last":group.last});
            if !group.times.is_empty() {
                value["frame_times"] = json!(compact_times(&group.times));
            }
            if !group.ranges.is_empty() {
                value["ranges"] = json!(
                    group
                        .ranges
                        .iter()
                        .map(|(start, duration)| json!({"start":start,"duration":duration}))
                        .collect::<Vec<_>>()
                );
            }
            value
        })
        .collect();
    Ok(json!({
        "source": info.path,
        "source_sha256": sha,
        "cache_dir": root,
        "filters": {"model": model, "focus_instructions_sha256": wanted_focus, "kind": args.kind.map(kind_name)},
        "groups": groups,
        "totals": {"entries": entries, "bytes": bytes},
        "exact_frame_images": images(&root.join("assets").join(&sha).join("exact-frames")),
        "unattributed_entries": unattributed,
        "truncated": truncated,
        "notes": [
            "Read-only listing by source content hash. A later run with the same model, cast (and focus text and schema, for focus entries) reuses these entries at no cost; use the exact command's --dry-run for would_request.",
            "unattributed_entries are cached descriptions written without listing metadata (any source); a describe run that reads them adds it."
        ],
    }))
}

fn images(dir: &Path) -> Value {
    let (mut count, mut bytes) = (0usize, 0u64);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten().take(MAX_SCANNED) {
            if let Ok(metadata) = entry.metadata()
                && metadata.is_file()
            {
                count += 1;
                bytes += metadata.len();
            }
        }
    }
    json!({"dir":dir,"count":count,"bytes":bytes})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn times_compact_into_evenly_spaced_runs() {
        let runs = compact_times(&[0.0, 1.0, 2.0, 3.0, 3.5, 4.0, 10.0]);
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert_eq!(runs[0], json!({"start":0.0,"end":3.0,"step":1.0,"count":4}));
        assert_eq!(runs[1], json!({"start":3.5,"end":4.0,"step":0.5,"count":2}));
        assert_eq!(
            runs[2],
            json!({"start":10.0,"end":10.0,"step":null,"count":1})
        );
        assert!(compact_times(&[]).is_empty());
        // Every published run reconstructs each of its times within 1 µs.
        for times in [
            vec![0.0, 1.0, 2.0005, 3.001],
            vec![0.0, 1.0, 2.000001, 3.000002],
            vec![0.0, 1.0, 2.000002, 3.0],
            (0..300)
                .map(|i| ((i as f64 / 30.0) * 1e6).round() / 1e6)
                .collect(),
        ] {
            let runs = compact_times(&times);
            let mut rebuilt = Vec::new();
            for run in &runs {
                let count = run["count"].as_u64().unwrap() as usize;
                let start = run["start"].as_f64().unwrap();
                let step = run["step"].as_f64().unwrap_or(0.0);
                rebuilt.extend((0..count).map(|i| start + i as f64 * step));
            }
            assert_eq!(rebuilt.len(), times.len());
            for (a, b) in rebuilt.iter().zip(&times) {
                assert!(
                    (a - b).abs() <= RUN_TOLERANCE_SECONDS,
                    "{times:?} -> {runs:?}"
                );
            }
        }
        // 30 fps times rounded to 6 decimals form one run.
        let fps: Vec<f64> = (0..300)
            .map(|i| ((i as f64 / 30.0) * 1e6).round() / 1e6)
            .collect();
        assert_eq!(compact_times(&fps).len(), 1);
    }
}
