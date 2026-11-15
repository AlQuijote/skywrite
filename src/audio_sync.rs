//! Audio-only coarse-to-fine synchronization. FFmpeg decodes; bounded FFT aligns.
use crate::sync::{
    Fit, Pair, Precision, SegmentFit, SyncOptions, SyncResult, choose_fit, fit_models,
    least_squares, result_from_segments,
};
use anyhow::{Context, Result, bail, ensure};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;
const SAMPLE_RATE: usize = 2000;
const FEATURE_RATE: usize = 20;
/// Clean waveform matches leave residuals under 1 ms; a few ms means the
/// asset was time-warped, so a linear conform would not be sample-accurate.
const WAVEFORM_STRONG_P95_SECONDS: f64 = 0.005;
/// Coarse envelope queries: one per 4 s of asset, 24 to 160 of them.
const COARSE_MIN_WINDOWS: usize = 24;
const COARSE_MAX_WINDOWS: usize = 160;
const COARSE_SPACING_SECONDS: f64 = 4.0;
/// Waveform anchors: one per 2 s of asset up to an hour, at most 1800 (one
/// per 8 s at the 4-hour analysis limit).
const DENSE_SPACING_SECONDS: f64 = 2.0;
const DENSE_MAX_PROBES: usize = 1800;
/// Each waveform anchor correlates 2 s of asset within +/-0.3 s of the prediction.
const PROBE_HALF_SECONDS: f64 = 1.0;
const PROBE_SEARCH_SECONDS: f64 = 0.30;
/// Waveform anchors below this correlation are ignored.
const PROBE_MIN_SCORE: f64 = 0.15;
/// A strong anchor outside every mapping is evidence of a discontinuity.
const ORPHAN_MIN_SCORE: f64 = 0.30;
/// A run of anchors inside a segment counts as another mapping only when it
/// disagrees by at least this: smaller offsets are indistinguishable from
/// pitch-period slips in voiced speech, and under a quarter frame.
const RUN_MIN_DISAGREEMENT_SECONDS: f64 = 0.010;
/// Segment boundaries are located to about this; segments stop this short of them.
const BOUNDARY_MARGIN_SECONDS: f64 = 0.3;
const MAX_FFT: usize = 1 << 20;
#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}
impl Complex {
    fn mul(self, rhs: Self) -> Self {
        Self {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}
/// Iterative radix-2 transform; all callers bound sizes and use powers of two.
fn fft(values: &mut [Complex], inverse: bool) {
    let n = values.len();
    debug_assert!(n.is_power_of_two() && n <= MAX_FFT);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = (if inverse { 2.0 } else { -2.0 }) * std::f64::consts::PI / len as f64;
        let unit = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for start in (0..n).step_by(len) {
            let mut w = Complex { re: 1.0, im: 0.0 };
            for i in 0..len / 2 {
                let a = values[start + i];
                let b = values[start + i + len / 2].mul(w);
                values[start + i] = Complex {
                    re: a.re + b.re,
                    im: a.im + b.im,
                };
                values[start + i + len / 2] = Complex {
                    re: a.re - b.re,
                    im: a.im - b.im,
                };
                w = w.mul(unit);
            }
        }
        len *= 2;
    }
    if inverse {
        for value in values {
            value.re /= n as f64;
            value.im /= n as f64;
        }
    }
}
fn decode(path: &Path, options: &SyncOptions) -> Result<(Vec<f64>, crate::media::MediaInfo)> {
    let path = crate::media::canonical_file(path)?;
    let info = crate::media::probe(&path)?;
    ensure!(info.has_audio(), "{} has no audio stream", path.display());
    // Preserve audio delay relative to container origin, including nonzero/negative PTS.
    let args: Vec<OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-copyts",
        "-start_at_zero",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .into_iter()
    .map(Into::into)
    .chain([path.into_os_string()])
    .chain([
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-af".into(),
        format!("aresample={SAMPLE_RATE}:async=0:first_pts=0").into(),
        "-ac".into(),
        "1".into(),
        "-t".into(),
        options.max_audio_seconds.to_string().into(),
        "-f".into(),
        "f32le".into(),
        "pipe:1".into(),
    ])
    .collect();
    let max_bytes = (options.max_audio_seconds * SAMPLE_RATE as f64 * 4.0).ceil() as usize + 65536;
    let output = crate::process::run("ffmpeg", &args, Duration::from_secs(180), max_bytes)
        .context("decoding audio for synchronization")?;
    ensure!(
        output.stdout.len() % 4 == 0,
        "FFmpeg returned malformed PCM audio"
    );
    let samples: Vec<f64> = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b) as f64)
        .collect();
    ensure!(
        samples.len() >= SAMPLE_RATE * 6,
        "audio synchronization needs at least 6 seconds of decoded audio"
    );
    ensure!(
        samples.iter().all(|x| x.is_finite()),
        "decoded audio contains non-finite samples"
    );
    let energy = samples.iter().map(|x| x * x).sum::<f64>() / samples.len() as f64;
    ensure!(
        energy > 1e-12,
        "audio is silent or too quiet to synchronize"
    );
    Ok((samples, info))
}
fn features(samples: &[f64]) -> Vec<f64> {
    let block = SAMPLE_RATE / FEATURE_RATE;
    let energy: Vec<f64> = samples
        .chunks_exact(block)
        .map(|s| ((s.iter().map(|x| x * x).sum::<f64>() / block as f64).sqrt() + 1e-6).ln())
        .collect();
    let mut prefix = vec![0.0; energy.len() + 1];
    for (i, x) in energy.iter().enumerate() {
        prefix[i + 1] = prefix[i] + x;
    }
    energy
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let a = i.saturating_sub(FEATURE_RATE / 2);
            let b = (i + FEATURE_RATE / 2 + 1).min(energy.len());
            x - (prefix[b] - prefix[a]) / (b - a) as f64
        })
        .collect()
}
struct Correlator {
    reference: Vec<f64>,
    spectrum: Vec<Complex>,
    prefix: Vec<f64>,
    squares: Vec<f64>,
    fft_size: usize,
}
impl Correlator {
    fn new(reference: &[f64], max_query: usize) -> Result<Self> {
        let fft_size = (reference.len() + max_query).next_power_of_two();
        ensure!(
            fft_size <= MAX_FFT,
            "audio correlation exceeds FFT budget; reduce --max-audio-seconds"
        );
        let mut spectrum = vec![Complex::default(); fft_size];
        for (t, &v) in spectrum.iter_mut().zip(reference) {
            t.re = v;
        }
        fft(&mut spectrum, false);
        let mut prefix = vec![0.0; reference.len() + 1];
        let mut squares = prefix.clone();
        for (i, &x) in reference.iter().enumerate() {
            prefix[i + 1] = prefix[i] + x;
            squares[i + 1] = squares[i] + x * x;
        }
        Ok(Self {
            reference: reference.to_vec(),
            spectrum,
            prefix,
            squares,
            fft_size,
        })
    }
    /// Normalized correlation of `query` at every start position (NaN where
    /// the reference window is constant); empty for an unusable query.
    fn scores(&self, query: &[f64]) -> Vec<f64> {
        if query.is_empty() || query.len() > self.reference.len() {
            return Vec::new();
        }
        let mean = query.iter().sum::<f64>() / query.len() as f64;
        let energy = query.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
        if energy <= 1e-12 {
            return Vec::new();
        }
        let mut spectrum = vec![Complex::default(); self.fft_size];
        for (i, &x) in query.iter().rev().enumerate() {
            spectrum[i].re = x - mean;
        }
        fft(&mut spectrum, false);
        for (v, &r) in spectrum.iter_mut().zip(&self.spectrum) {
            *v = v.mul(r);
        }
        fft(&mut spectrum, true);
        (0..=self.reference.len() - query.len())
            .map(|start| {
                let sum = self.prefix[start + query.len()] - self.prefix[start];
                let variance = self.squares[start + query.len()]
                    - self.squares[start]
                    - sum * sum / query.len() as f64;
                if variance <= 1e-12 {
                    f64::NAN
                } else {
                    (spectrum[start + query.len() - 1].re / (energy * variance).sqrt())
                        .clamp(-1.0, 1.0)
                }
            })
            .collect()
    }
    fn peaks(
        &self,
        query: &[f64],
        count: usize,
        separation: usize,
        absolute: bool,
    ) -> Vec<(usize, f64)> {
        let mut scores: Vec<(usize, f64)> = self
            .scores(query)
            .into_iter()
            .enumerate()
            .filter(|(_, s)| !s.is_nan())
            .map(|(i, s)| (i, if absolute { s.abs() } else { s }))
            .collect();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut selected: Vec<(usize, f64)> = Vec::new();
        for p in scores {
            if selected
                .iter()
                .all(|s| s.0.abs_diff(p.0) >= separation.max(1))
            {
                selected.push(p);
                if selected.len() == count {
                    break;
                }
            }
        }
        selected
    }
}
fn coarse(
    reference: &[f64],
    asset: &[f64],
    options: &SyncOptions,
) -> Result<(Vec<Fit>, usize, Vec<f64>)> {
    let rf = features(reference);
    let af = features(asset);
    let query_len = (FEATURE_RATE * 8)
        .min(af.len() * 45 / 100)
        .max(FEATURE_RATE * 2);
    ensure!(
        query_len < af.len() && query_len <= rf.len(),
        "recordings too short to synchronize"
    );
    let correlator = Correlator::new(&rf, query_len)?;
    let available = af.len() - query_len;
    let spaced =
        (available as f64 / (COARSE_SPACING_SECONDS * FEATURE_RATE as f64)).ceil() as usize + 1;
    let windows = spaced
        .clamp(COARSE_MIN_WINDOWS, COARSE_MAX_WINDOWS)
        .min(available + 1);
    let mut pairs = Vec::new();
    let mut centers = Vec::with_capacity(windows);
    for i in 0..windows {
        let start = i * available / (windows - 1).max(1);
        let center = (start as f64 + query_len as f64 / 2.0) / FEATURE_RATE as f64;
        centers.push(center);
        let query = &af[start..start + query_len];
        for (position, score) in correlator.peaks(query, 3, query_len / 2, false) {
            if score < 0.35 {
                continue;
            }
            pairs.push(Pair {
                asset: center,
                reference: (position as f64 + query_len as f64 / 2.0) / FEATURE_RATE as f64,
                asset_index: i,
                reference_index: position,
                weight: score.powi(2),
            });
        }
    }
    let mut opts = options.clone();
    opts.tolerance_seconds = options.tolerance_seconds.max(0.15);
    let mut matched: Vec<usize> = pairs.iter().map(|p| p.asset_index).collect();
    matched.dedup();
    let advice = format!(
        "{} of {windows} {:.0} s windows of the shorter recording found a candidate match. The recordings may not contain the same sound, or may share less than about 10 s (a drift estimate needs about 12 s). Check with skywrite inspect and by listening; for speech recorded by different microphones, sync transcripts instead",
        matched.len(),
        query_len as f64 / FEATURE_RATE as f64
    );
    let mut models =
        fit_models(&pairs, &opts).with_context(|| format!("no reliable shared audio: {advice}"))?;
    models.retain(|fit| fit.weight / fit.pairs.len() as f64 > 0.12);
    ensure!(
        !models.is_empty(),
        "shared audio matches are too weak to synchronize: {advice}"
    );
    Ok((models, pairs.len(), centers))
}
/// Asset samples around `center` (seconds), resampled by `scale` so the
/// window has the reference's time base; `None` near the asset's ends.
fn stretched_window(asset: &[f64], center: f64, half: f64, scale: f64) -> Option<Vec<f64>> {
    let center = center * SAMPLE_RATE as f64;
    let half = half * SAMPLE_RATE as f64;
    if center < half || center + half + 1.0 >= asset.len() as f64 {
        return None;
    }
    let len = (half * 2.0 * scale).round() as usize;
    Some(
        (0..len)
            .map(|j| {
                let position = center - half + j as f64 / scale;
                let at = position.floor() as usize;
                let fraction = position - at as f64;
                asset[at] * (1.0 - fraction) + asset[at + 1] * fraction
            })
            .collect(),
    )
}
/// Waveform anchors: correlate 2 s of asset around each center within
/// +/-`search` seconds of where `fit` predicts it, to a fraction of a sample.
fn probe(
    reference: &[f64],
    asset: &[f64],
    fit: &Fit,
    centers: &[f64],
    search: f64,
) -> Result<Vec<Pair>> {
    let mut pairs = Vec::new();
    for (i, &center) in centers.iter().enumerate() {
        let Some(query) = stretched_window(asset, center, PROBE_HALF_SECONDS, fit.scale) else {
            continue;
        };
        let predicted = (fit.offset + fit.scale * center) * SAMPLE_RATE as f64;
        let half_query = query.len() as f64 / 2.0;
        let margin = search * SAMPLE_RATE as f64;
        let search_start = (predicted - half_query - margin).floor().max(0.0) as usize;
        let search_end = (predicted + half_query + margin)
            .ceil()
            .min(reference.len() as f64) as usize;
        if search_start >= search_end || search_end - search_start < query.len() {
            continue;
        }
        let correlator = Correlator::new(&reference[search_start..search_end], query.len())?;
        let scores = correlator.scores(&query);
        let Some((k, score)) = scores
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.is_nan())
            .map(|(k, s)| (k, s.abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        if score < PROBE_MIN_SCORE {
            continue;
        }
        // Parabolic interpolation of the peak: sub-sample timing.
        let mut fraction = 0.0;
        if k > 0 && k + 1 < scores.len() && !scores[k - 1].is_nan() && !scores[k + 1].is_nan() {
            let (y0, y2) = (scores[k - 1].abs(), scores[k + 1].abs());
            let curvature = y0 - 2.0 * score + y2;
            if curvature < 0.0 {
                fraction = (0.5 * (y0 - y2) / curvature).clamp(-0.5, 0.5);
            }
        }
        pairs.push(Pair {
            asset: center,
            reference: (search_start as f64 + k as f64 + fraction + half_query)
                / SAMPLE_RATE as f64,
            asset_index: i,
            reference_index: search_start + k,
            weight: score.powi(2),
        });
    }
    Ok(pairs)
}
fn interval(fit: &Fit) -> [f64; 2] {
    [fit.pairs[0].asset, fit.pairs[fit.pairs.len() - 1].asset]
}
fn reference_interval(fit: &Fit) -> [f64; 2] {
    let a = fit.pairs[0].reference;
    let b = fit.pairs[fit.pairs.len() - 1].reference;
    [a.min(b), a.max(b)]
}
fn overlap(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[1].min(b[1]) - a[0].max(b[0])
}
fn predict(fit: &Fit, asset: f64) -> f64 {
    fit.offset + fit.scale * asset
}
/// Re-measure `fit` with anchors across the whole asset and refit, keeping
/// the model that continues `fit`. Returns the fit and the probe anchors.
fn densify(
    reference: &[f64],
    asset: &[f64],
    fit: &Fit,
    centers: &[f64],
    fine: &SyncOptions,
) -> Result<(Fit, Vec<Pair>)> {
    let mut current = fit.clone();
    let mut anchors = Vec::new();
    for _ in 0..2 {
        let pairs = probe(reference, asset, &current, centers, PROBE_SEARCH_SECONDS)?;
        let Ok(models) = fit_models(&pairs, fine) else {
            if anchors.is_empty() {
                anchors = pairs;
            }
            break;
        };
        let span = interval(&current);
        let middle = (span[0] + span[1]) / 2.0;
        let next = models
            .into_iter()
            .filter(|m| {
                overlap(interval(m), span) > 0.0
                    && (predict(m, middle) - predict(&current, middle)).abs()
                        < fine.tolerance_seconds
            })
            .max_by(|a, b| a.weight.total_cmp(&b.weight));
        anchors = pairs;
        match next {
            Some(next) => current = next,
            None => break,
        }
    }
    Ok((current, anchors))
}
/// `inner` lies strictly inside `host`'s span and `host` has at most one
/// anchor there per ten of `inner`'s.
fn nested_in(inner: &Fit, host: &Fit) -> bool {
    let (span, outer) = (interval(inner), interval(host));
    let inside = host
        .pairs
        .iter()
        .filter(|p| p.asset >= span[0] && p.asset <= span[1])
        .count();
    outer[0] < span[0] && outer[1] > span[1] && inside * 10 <= inner.pairs.len()
}
/// Split every segment that hosts a nested one into the pieces before and
/// after it (pieces with too few anchors are dropped; their sound is then
/// unexplained or uncovered, never placed with the host's mapping).
fn split_hosts(
    segments: Vec<Fit>,
    anchors: Vec<Vec<Pair>>,
    min_anchors: usize,
    dropped: &mut Vec<[f64; 2]>,
) -> (Vec<Fit>, Vec<Vec<Pair>>) {
    let mut pairs: Vec<(Fit, Vec<Pair>)> = segments.into_iter().zip(anchors).collect();
    for _ in 0..8 {
        let Some((host, inner)) = (0..pairs.len())
            .flat_map(|i| (0..pairs.len()).map(move |j| (i, j)))
            .find(|&(i, j)| i != j && nested_in(&pairs[j].0, &pairs[i].0))
        else {
            break;
        };
        let span = interval(&pairs[inner].0);
        let (fit, probes) = pairs.remove(host);
        for side in [
            fit.pairs
                .iter()
                .filter(|p| p.asset < span[0])
                .cloned()
                .collect::<Vec<_>>(),
            fit.pairs
                .iter()
                .filter(|p| p.asset > span[1])
                .cloned()
                .collect(),
        ] {
            match (side.len() >= min_anchors).then(|| fit_of(&side)).flatten() {
                Some(piece) => pairs.push((piece, probes.clone())),
                None => {
                    if let (Some(first), Some(last)) = (side.first(), side.last()) {
                        dropped.push([first.asset, last.asset]);
                    }
                }
            }
        }
    }
    pairs.into_iter().unzip()
}
/// Pick non-overlapping mappings, best-supported first. An alternative that
/// claims the same asset or reference passage with similar support is an
/// ambiguity (repeated sound), not a segment.
fn select_segments(candidates: &mut Vec<Fit>, options: &SyncOptions) -> Result<Vec<Fit>> {
    candidates.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| b.pairs.len().cmp(&a.pairs.len()))
    });
    let mut segments: Vec<Fit> = Vec::new();
    for fit in candidates.drain(..) {
        if fit.pairs.len() < options.min_anchors {
            continue;
        }
        let span = interval(&fit);
        let conflict = segments.iter().find(|s| {
            overlap(interval(s), span) > 0.0
                || overlap(reference_interval(s), reference_interval(&fit)) > 0.0
        });
        match conflict {
            None => segments.push(fit),
            // A stretch inside a better-supported mapping's span where that
            // mapping has (almost) no anchors: a nested segment; the host is
            // split around it later (`split_hosts`).
            Some(segment) if nested_in(&fit, segment) => segments.push(fit),
            Some(segment) => {
                let shared = [
                    span[0].max(interval(segment)[0]),
                    span[1].min(interval(segment)[1]),
                ];
                let duplicate = overlap(interval(segment), span) > 0.0
                    && shared.iter().all(|&t| {
                        (predict(segment, t) - predict(&fit, t)).abs() < options.tolerance_seconds
                    });
                if !duplicate
                    && fit.pairs.len() as f64 >= segment.pairs.len() as f64 * 0.85
                    && fit.weight >= segment.weight * 0.70
                {
                    bail!(
                        "ambiguous synchronization: multiple passages support different time mappings"
                    );
                }
            }
        }
    }
    Ok(segments)
}
/// Whether a probe anchor agrees with `fit` (within its own scatter).
fn explains(fit: &Fit, pair: &Pair) -> bool {
    let span = interval(fit);
    pair.asset >= span[0]
        && pair.asset <= span[1]
        && (pair.reference - predict(fit, pair.asset)).abs() <= (4.0 * p95(fit)).max(0.003)
}
/// Strong anchors that no segment explains, one per probe position: those
/// outside every segment's span, and runs of at least three consecutive
/// strong anchors inside a span that disagree with it by at least 10 ms while
/// agreeing with each other within 3 ms (a stretch with another mapping).
/// `anchors[i]` are the probes of `segments[i]`; `all` also holds the probes
/// of segments pruned away.
fn orphans(segments: &[Fit], anchors: &[Vec<Pair>], all: &[Vec<Pair>]) -> (Vec<Pair>, usize) {
    let strong = |p: &Pair| p.weight >= ORPHAN_MIN_SCORE.powi(2);
    let mut runs = 0;
    let mut best: std::collections::BTreeMap<usize, Pair> = Default::default();
    let mut keep = |pair: &Pair| {
        let entry = best.entry(pair.asset_index).or_insert_with(|| pair.clone());
        if pair.weight > entry.weight {
            *entry = pair.clone();
        }
    };
    for pair in all.iter().flatten() {
        if strong(pair)
            && !segments.iter().any(|s| {
                let span = interval(s);
                pair.asset >= span[0] && pair.asset <= span[1]
            })
        {
            keep(pair);
        }
    }
    for (segment, probes) in segments.iter().zip(anchors) {
        let span = interval(segment);
        let inside: Vec<&Pair> = probes
            .iter()
            .filter(|p| strong(p) && p.asset >= span[0] && p.asset <= span[1])
            .collect();
        let mut run: Vec<&Pair> = Vec::new();
        for pair in inside.iter().copied().chain(std::iter::once(&Pair {
            asset: f64::NAN,
            reference: f64::NAN,
            asset_index: usize::MAX,
            reference_index: 0,
            weight: 0.0,
        })) {
            let contradicts = pair.asset_index != usize::MAX
                && (pair.reference - predict(segment, pair.asset)).abs()
                    >= RUN_MIN_DISAGREEMENT_SECONDS
                && !segments.iter().any(|s| explains(s, pair));
            let residual = |p: &Pair| p.reference - predict(segment, p.asset);
            // A change of residual starts a new run: adjacent stretches with
            // different mappings are each judged on their own.
            let continues = run
                .last()
                .is_none_or(|last| (residual(pair) - residual(last)).abs() <= 0.003);
            if contradicts && continues {
                run.push(pair);
            } else {
                // Another mapping is self-consistent; scattered slips are not.
                let residuals = run.iter().map(|p| p.reference - predict(segment, p.asset));
                let spread = residuals.clone().fold(f64::NEG_INFINITY, f64::max)
                    - residuals.fold(f64::INFINITY, f64::min);
                if run.len() >= 3 && spread <= 0.003 {
                    runs += run.len();
                    run.iter().for_each(|p| keep(p));
                }
                run.clear();
                if contradicts {
                    run.push(pair);
                }
            }
        }
    }
    (best.into_values().collect(), runs)
}
/// Runs of at least five consecutive probe windows (about 8 s) inside a
/// segment's span that carry sound (RMS within 12 dB of the 90th-percentile
/// window) but match no mapping: the mapping is not evidenced there. Shorter runs
/// occur by chance in long damaged takes. Returns (start, end) in
/// asset seconds for each run.
fn unmatched_sound(segments: &[Fit], asset: &[f64], centers: &[f64]) -> Vec<[f64; 2]> {
    let rms: Vec<f64> = centers
        .iter()
        .map(|&c| {
            let lo = ((c - PROBE_HALF_SECONDS) * SAMPLE_RATE as f64).max(0.0) as usize;
            let hi = (((c + PROBE_HALF_SECONDS) * SAMPLE_RATE as f64) as usize).min(asset.len());
            let window = &asset[lo.min(hi)..hi];
            (window.iter().map(|x| x * x).sum::<f64>() / window.len().max(1) as f64).sqrt()
        })
        .collect();
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| {
        sorted
            .get((sorted.len() as f64 * q) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    // Sound: within 12 dB of the loud windows (90th percentile), so hiss or
    // room tone in pauses never counts even when pauses fill most of the take.
    let threshold = at(0.9) * 0.25;
    let matched: std::collections::HashSet<usize> = segments
        .iter()
        .flat_map(|s| s.pairs.iter().map(|p| p.asset_index))
        .collect();
    let mut runs = Vec::new();
    for segment in segments {
        let span = interval(segment);
        let mut run: Vec<usize> = Vec::new();
        for (i, &c) in centers
            .iter()
            .enumerate()
            .chain(std::iter::once((usize::MAX, &f64::NAN)))
        {
            let inside = i != usize::MAX && c > span[0] && c < span[1];
            if inside && rms[i] >= threshold && rms[i] > 0.0 && !matched.contains(&i) {
                run.push(i);
                continue;
            }
            if run.len() >= 5 {
                runs.push([centers[run[0]], centers[run[run.len() - 1]]]);
            }
            run.clear();
        }
    }
    runs
}
/// Short-window similarity of the asset at `center` under `fit`'s mapping
/// (best within +/-5 ms of the prediction).
fn local_score(reference: &[f64], asset: &[f64], fit: &Fit, center: f64) -> f64 {
    const HALF: f64 = 0.25;
    const LAG: isize = 10;
    let Some(query) = stretched_window(asset, center, HALF, fit.scale) else {
        return 0.0;
    };
    let mean = query.iter().sum::<f64>() / query.len() as f64;
    let energy: f64 = query.iter().map(|x| (x - mean).powi(2)).sum();
    if energy <= 1e-12 {
        return 0.0;
    }
    let start = predict(fit, center) * SAMPLE_RATE as f64 - query.len() as f64 / 2.0;
    let mut best: f64 = 0.0;
    for lag in -LAG..=LAG {
        let at = start.round() as isize + lag;
        if at < 0 || at as usize + query.len() > reference.len() {
            continue;
        }
        let window = &reference[at as usize..at as usize + query.len()];
        let wm = window.iter().sum::<f64>() / window.len() as f64;
        let variance: f64 = window.iter().map(|x| (x - wm).powi(2)).sum();
        if variance <= 1e-12 {
            continue;
        }
        let dot: f64 = window
            .iter()
            .zip(&query)
            .map(|(r, q)| (r - wm) * (q - mean))
            .sum();
        best = best.max((dot / (energy * variance).sqrt()).abs());
    }
    best
}
/// Where between two segments' anchors the asset switches from `before`'s
/// mapping to `after`'s: the split that agrees with the most short windows.
/// Returns where `before` ends and `after` starts; between them (sound with
/// no counterpart, or no evidence either way) neither mapping is claimed.
fn locate_boundary(
    reference: &[f64],
    asset: &[f64],
    before: &Fit,
    after: &Fit,
    lo: f64,
    hi: f64,
) -> (f64, f64) {
    // Anchors summarize 2 s windows, so the switch can lie up to a half window outside them.
    let lo = (lo - PROBE_HALF_SECONDS).max(0.0);
    let hi = hi + PROBE_HALF_SECONDS;
    let step = ((hi - lo) / 1200.0).max(0.1);
    let positions: Vec<f64> = (0..)
        .map(|i| lo + step * i as f64)
        .take_while(|&p| p <= hi)
        .collect();
    let labels: Vec<Option<bool>> = positions
        .iter()
        .map(|&p| {
            let a = local_score(reference, asset, before, p);
            let b = local_score(reference, asset, after, p);
            (a.max(b) >= ORPHAN_MIN_SCORE).then_some(b > a)
        })
        .collect();
    // cost(k): windows before split k that prefer `after`, plus windows from k on that prefer `before`.
    let mut cost: usize = labels.iter().filter(|l| **l == Some(false)).count();
    let mut costs = vec![cost];
    for label in &labels {
        match label {
            Some(true) => cost += 1,
            Some(false) => cost -= 1,
            None => {}
        }
        costs.push(cost);
    }
    let minimum = *costs.iter().min().unwrap();
    let split_time = |k: usize| match k {
        0 => lo,
        k if k >= positions.len() => hi,
        k => (positions[k - 1] + positions[k]) / 2.0,
    };
    let best: Vec<f64> = costs
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == minimum)
        .map(|(k, _)| split_time(k))
        .collect();
    (best[0], best[best.len() - 1])
}
fn p95(fit: &Fit) -> f64 {
    let mut residuals: Vec<f64> = fit
        .pairs
        .iter()
        .map(|p| (p.reference - predict(fit, p.asset)).abs())
        .collect();
    residuals.sort_by(f64::total_cmp);
    residuals[((residuals.len() - 1) as f64 * 0.95).ceil() as usize]
}
fn fit_of(pairs: &[Pair]) -> Option<Fit> {
    let (offset, scale) = least_squares(pairs)?;
    Some(Fit {
        offset,
        scale,
        pairs: pairs.to_vec(),
        weight: pairs.iter().map(|p| p.weight).sum(),
    })
}
/// A step in the residuals (a dropout or restart too small to separate as
/// its own hypothesis, typically under 40 ms): split where two affine pieces
/// leave the least weighted squared error, if each piece then fits cleanly.
/// Continuous time-warping does not split: neither piece would be clean.
fn split_steps(fit: Fit, options: &SyncOptions, limit: f64, depth: usize) -> Vec<Fit> {
    let n = fit.pairs.len();
    let min = options.min_anchors;
    if depth == 0 || n < 2 * min || p95(&fit) < limit {
        return vec![fit];
    }
    // Weighted prefix sums of w, wx, wy, wxx, wxy, wyy (x relative to the first anchor).
    let x0 = fit.pairs[0].asset;
    let y0 = fit.pairs[0].reference;
    let mut prefix = vec![[0.0f64; 6]; n + 1];
    for (i, p) in fit.pairs.iter().enumerate() {
        let (w, x, y) = (p.weight, p.asset - x0, p.reference - y0);
        let terms = [w, w * x, w * y, w * x * x, w * x * y, w * y * y];
        for j in 0..6 {
            prefix[i + 1][j] = prefix[i][j] + terms[j];
        }
    }
    let sse = |a: usize, b: usize| {
        let s: Vec<f64> = (0..6).map(|j| prefix[b][j] - prefix[a][j]).collect();
        let (w, sx, sy, sxx, sxy, syy) = (s[0], s[1], s[2], s[3], s[4], s[5]);
        let vxx = sxx - sx * sx / w;
        let vxy = sxy - sx * sy / w;
        let vyy = syy - sy * sy / w;
        if vxx <= 1e-12 {
            vyy
        } else {
            (vyy - vxy * vxy / vxx).max(0.0)
        }
    };
    let Some(k) = (min..=n - min)
        .min_by(|&a, &b| (sse(0, a) + sse(a, n)).total_cmp(&(sse(0, b) + sse(b, n))))
    else {
        return vec![fit];
    };
    // The anchor nearest each side of the step may straddle it; leave it out,
    // and judge each piece with the same robust (trimmed) fit as any mapping.
    let joint = p95(&fit);
    let piece = |pairs: &[Pair]| {
        let model = fit_models(pairs, options).ok()?.into_iter().next()?;
        (model.pairs.len() * 10 >= pairs.len() * 7
            && p95(&model) < limit
            && p95(&model) * 3.0 < joint
            && (model.scale - 1.0).abs() <= options.max_scale_deviation + 1e-12)
            .then_some(model)
    };
    let (Some(left), Some(right)) = (piece(&fit.pairs[..k - 1]), piece(&fit.pairs[k + 1..])) else {
        return vec![fit];
    };
    let mut pieces = split_steps(left, options, limit, depth - 1);
    pieces.extend(split_steps(right, options, limit, depth - 1));
    pieces
}
/// Each probe window belongs to the mapping it matches best: drop `fit`'s
/// anchors where another mapping has a stronger anchor at the same window
/// (a weak chance match), then refit. `None` if too few anchors remain.
fn prune(fit: &Fit, others: &[&Fit], min_anchors: usize) -> Option<Fit> {
    let kept: Vec<Pair> = fit
        .pairs
        .iter()
        .filter(|p| {
            others.iter().all(|o| {
                o.pairs
                    .iter()
                    .all(|q| q.asset_index != p.asset_index || q.weight <= p.weight)
            })
        })
        .cloned()
        .collect();
    if kept.len() == fit.pairs.len() {
        return Some(fit.clone());
    }
    (kept.len() >= min_anchors).then(|| fit_of(&kept)).flatten()
}
/// First-order pre-emphasis, y[n] = x[n] - 0.95 x[n-1].
fn pre_emphasis(x: &[f64]) -> Vec<f64> {
    let mut previous = 0.0;
    x.iter()
        .map(|&v| {
            let y = v - 0.95 * previous;
            previous = v;
            y
        })
        .collect()
}
fn invert(fit: &mut Fit) {
    fit.offset = -fit.offset / fit.scale;
    fit.scale = fit.scale.recip();
    for p in &mut fit.pairs {
        std::mem::swap(&mut p.asset, &mut p.reference);
        std::mem::swap(&mut p.asset_index, &mut p.reference_index);
    }
    fit.pairs.sort_by(|a, b| a.asset.total_cmp(&b.asset));
}
pub fn align_audio(reference: &Path, asset: &Path, options: &SyncOptions) -> Result<SyncResult> {
    options.validate()?;
    let rp = crate::media::canonical_file(reference)?;
    let ap = crate::media::canonical_file(asset)?;
    let (rs, ri) = decode(&rp, options)?;
    let (asset_samples, ai) = decode(&ap, options)?;
    // Search shorter inside longer; inverse maps are converted back before reporting.
    let swapped = asset_samples.len() > rs.len();
    let (r, a) = if swapped {
        (&asset_samples, &rs)
    } else {
        (&rs, &asset_samples)
    };
    let (models, count, coarse_centers) = coarse(r, a, options)?;
    // Waveform anchors on pre-emphasized sound: flatter spectra give sharper
    // correlation peaks under reverb and fewer one-pitch-period slips.
    let (rw, aw) = (pre_emphasis(r), pre_emphasis(a));
    let (r, a) = (&rw, &aw);
    let mut fine = options.clone();
    fine.tolerance_seconds = options.tolerance_seconds.min(0.04);
    // Every coarse hypothesis gets waveform anchors at the coarse windows.
    let mut candidates = Vec::new();
    for model in &models {
        let pairs = probe(r, a, model, &coarse_centers, PROBE_SEARCH_SECONDS)?;
        if pairs.len() >= options.min_anchors
            && let Ok(fits) = fit_models(&pairs, &fine)
        {
            candidates.extend(fits);
        }
    }
    let inner_seconds = a.len() as f64 / SAMPLE_RATE as f64;
    let dense_count =
        ((inner_seconds / DENSE_SPACING_SECONDS).floor() as usize).clamp(2, DENSE_MAX_PROBES);
    let dense_centers: Vec<f64> = (0..dense_count)
        .map(|i| {
            PROBE_HALF_SECONDS
                + (inner_seconds - 2.0 * PROBE_HALF_SECONDS) * i as f64 / (dense_count - 1) as f64
        })
        .collect();
    let mut segments = select_segments(&mut candidates, &fine)?;
    let mut anchors = Vec::new();
    for segment in &mut segments {
        let (fit, pairs) = densify(r, a, segment, &dense_centers, &fine)?;
        *segment = fit;
        anchors.push(pairs);
    }
    // Host pieces too short to keep: their sound is placed by no mapping.
    let mut dropped: Vec<[f64; 2]> = Vec::new();
    (segments, anchors) = split_hosts(segments, anchors, options.min_anchors, &mut dropped);
    // Strong anchors outside every segment: another mapping (a cut, dropout or restart).
    for _ in 0..3 {
        let (lost, _) = orphans(&segments, &anchors, &anchors);
        if lost.len() < options.min_anchors {
            break;
        }
        let Ok(mut found) = fit_models(&lost, &fine) else {
            break;
        };
        // A candidate inside a segment's span is a nested stretch only if that
        // segment has (almost) no anchors there; otherwise it is a parallel
        // chance line (pitch-period slips) and is ignored.
        let hosts_agree = |f: &Fit| {
            let span = interval(f);
            segments.iter().all(|s| {
                let inside = s
                    .pairs
                    .iter()
                    .filter(|p| p.asset >= span[0] && p.asset <= span[1])
                    .count();
                inside * 10 <= f.pairs.len()
            })
        };
        found.retain(|f| f.pairs.len() >= options.min_anchors && hosts_agree(f));
        let Some(best) = found
            .into_iter()
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
        else {
            break;
        };
        let (fit, pairs) = densify(r, a, &best, &dense_centers, &fine)?;
        let others: Vec<&Fit> = segments.iter().collect();
        let Some(fit) = prune(&fit, &others, options.min_anchors) else {
            break;
        };
        if !hosts_agree(&fit) {
            break;
        }
        // Split any segment whose span contains the new one around it.
        let span = interval(&fit);
        let mut split: Vec<(Fit, Vec<Pair>)> = Vec::new();
        for (segment, probes) in segments.drain(..).zip(anchors.drain(..)) {
            let host = interval(&segment);
            if host[0] < span[0] && host[1] > span[1] {
                for side in [
                    segment
                        .pairs
                        .iter()
                        .filter(|p| p.asset < span[0])
                        .cloned()
                        .collect::<Vec<_>>(),
                    segment
                        .pairs
                        .iter()
                        .filter(|p| p.asset > span[1])
                        .cloned()
                        .collect(),
                ] {
                    match (side.len() >= options.min_anchors)
                        .then(|| fit_of(&side))
                        .flatten()
                    {
                        Some(piece) => split.push((piece, probes.clone())),
                        None => {
                            if let (Some(first), Some(last)) = (side.first(), side.last()) {
                                dropped.push([first.asset, last.asset]);
                            }
                        }
                    }
                }
            } else {
                split.push((segment, probes));
            }
        }
        (segments, anchors) = split.into_iter().unzip();
        if segments.iter().any(|s| {
            overlap(interval(s), span) > 0.0
                || overlap(reference_interval(s), reference_interval(&fit)) > 0.0
        }) {
            break;
        }
        segments.push(fit);
        anchors.push(pairs);
    }
    // Settle windows claimed by several mappings in favour of the best match.
    // `kept_anchors` stays parallel to the segments that survive pruning.
    let (settled, kept_anchors): (Vec<Fit>, Vec<Vec<Pair>>) = (0..segments.len())
        .filter_map(|i| {
            let others: Vec<&Fit> = (0..segments.len())
                .filter(|&j| j != i)
                .map(|j| &segments[j])
                .collect();
            prune(&segments[i], &others, options.min_anchors).map(|fit| (fit, anchors[i].clone()))
        })
        .unzip();
    let segments = settled;
    let (unexplained, contradicting_runs) = orphans(&segments, &kept_anchors, &anchors);
    let silent_evidence = unmatched_sound(&segments, a, &dense_centers);
    let strong_limit = WAVEFORM_STRONG_P95_SECONDS.min(options.tolerance_seconds * 0.75);
    let mut segments: Vec<Fit> = segments
        .into_iter()
        .flat_map(|fit| split_steps(fit, &fine, strong_limit, 3))
        .collect();
    let precise = !segments.is_empty();
    let mut warnings = Vec::new();
    // Whether each part's start/end may extend to the outer file's ends
    // (false where a dropped host piece was excluded).
    let mut open_ends: Vec<(bool, bool)> = Vec::new();
    let mut parts: Vec<SegmentFit> = if precise {
        segments.sort_by(|x, y| interval(x)[0].total_cmp(&interval(y)[0]));
        // (end of segment i, start of segment i + 1), in the inner time base.
        let mut ends = vec![(0.0, 0.0)];
        for pair in segments.windows(2) {
            let (lo, hi) = (interval(&pair[0])[1], interval(&pair[1])[0]);
            // Neither mapping claims sound within the location uncertainty.
            let (end, start) = locate_boundary(r, a, &pair[0], &pair[1], lo, hi);
            ends.push((
                end - BOUNDARY_MARGIN_SECONDS,
                start + BOUNDARY_MARGIN_SECONDS,
            ));
        }
        ends.push((inner_seconds, inner_seconds));
        let last = segments.len() - 1;
        segments
            .into_iter()
            .enumerate()
            .map(|(i, fit)| {
                let span = interval(&fit);
                let mut range = [ends[i].1, ends[i + 1].0];
                let unclamped = range;
                // Never extend over a dropped host piece.
                for d in &dropped {
                    if d[1] <= span[0] {
                        range[0] =
                            range[0].max(d[1] + PROBE_HALF_SECONDS + BOUNDARY_MARGIN_SECONDS);
                    } else if d[0] >= span[1] {
                        range[1] =
                            range[1].min(d[0] - PROBE_HALF_SECONDS - BOUNDARY_MARGIN_SECONDS);
                    }
                }
                // Anchor windows reaching across an interior boundary mix two mappings.
                let lo = if i == 0 {
                    f64::NEG_INFINITY
                } else {
                    range[0] + PROBE_HALF_SECONDS
                };
                let hi = if i == last {
                    f64::INFINITY
                } else {
                    range[1] - PROBE_HALF_SECONDS
                };
                let inside: Vec<Pair> = fit
                    .pairs
                    .iter()
                    .filter(|p| p.asset >= lo && p.asset <= hi)
                    .cloned()
                    .collect();
                let fit = if inside.len() >= options.min_anchors && inside.len() < fit.pairs.len() {
                    fit_of(&inside).unwrap_or(fit)
                } else {
                    fit
                };
                open_ends.push((range[0] == unclamped[0], range[1] == unclamped[1]));
                SegmentFit {
                    fit,
                    asset_range: range,
                }
            })
            .collect()
    } else {
        open_ends.push((true, true));
        vec![SegmentFit {
            fit: choose_fit(&models)?,
            asset_range: [0.0, inner_seconds],
        }]
    };
    for d in &dropped {
        warnings.push(format!(
            "{} {:.1}-{:.1} s follows a mapping with too few anchors to place; it is left to no segment",
            if swapped { "reference" } else { "asset" },
            d[0],
            d[1]
        ));
    }
    let outer_seconds = (asset_samples.len() as f64 / SAMPLE_RATE as f64).max(ai.duration);
    let last = parts.len() - 1;
    for (i, part) in parts.iter_mut().enumerate() {
        if swapped {
            let [u0, u1] = part.asset_range;
            let (start, end) = (predict(&part.fit, u0), predict(&part.fit, u1));
            invert(&mut part.fit);
            part.asset_range = [start.min(end), start.max(end)];
        }
        // Extend to the outer file's ends, except where a dropped piece was excluded.
        if i == 0 && open_ends[i].0 {
            part.asset_range[0] = 0.0;
        }
        if i == last && open_ends[i].1 {
            part.asset_range[1] = outer_seconds;
        }
        part.asset_range = part.asset_range.map(|t| t.clamp(0.0, outer_seconds));
        ensure!(
            (part.fit.scale - 1.0).abs() <= options.max_scale_deviation + 1e-12,
            "estimated asset-to-reference scale exceeds --max-scale-deviation"
        );
    }
    parts.sort_by(|x, y| x.asset_range[0].total_cmp(&y.asset_range[0]));
    // Inverted boundaries can overlap when the asset lost sound (a recorder
    // dropout); the earlier mapping holds up to its end, so the next starts there.
    for i in 1..parts.len() {
        let end = parts[i - 1].asset_range[1];
        if parts[i].asset_range[0] < end {
            parts[i].asset_range[0] = end.min(parts[i].asset_range[1]);
        }
    }
    for (i, pair) in parts.windows(2).enumerate() {
        let (end, start) = (pair[0].asset_range[1], pair[1].asset_range[0]);
        warnings.push(if start - end > 0.5 {
            format!("segments {i} and {}: asset {end:.2}-{start:.2} s is placed by neither mapping (sound with no counterpart in the reference, plus {BOUNDARY_MARGIN_SECONDS} s each side for the boundary's location uncertainty)", i + 1)
        } else {
            format!("segments {i} and {}: boundary at asset {:.2} s", i + 1, (end + start) / 2.0)
        });
    }
    let primary = (0..parts.len())
        .max_by(|&x, &y| parts[x].fit.weight.total_cmp(&parts[y].fit.weight))
        .unwrap_or(0);
    // Candidates in the same units as inliers: waveform probe windows that
    // found any match (envelope pairs when only the envelope stage succeeded).
    let candidates = if precise {
        let mut windows: Vec<usize> = anchors.iter().flatten().map(|p| p.asset_index).collect();
        windows.sort_unstable();
        windows.dedup();
        // A fit kept from the coarse windows (dense fitting failed) has its
        // inliers there: each is a matching window too.
        let coarse = parts.iter().map(|p| p.fit.pairs.len()).max().unwrap_or(0);
        windows.len().max(coarse)
    } else {
        count
    };
    let mut result = result_from_segments(
        &parts,
        primary,
        candidates,
        options,
        &rp,
        &ap,
        if precise {
            Precision {
                method: "audio_waveform",
                resolution_seconds: 1.0 / SAMPLE_RATE as f64,
                strong_p95_seconds: WAVEFORM_STRONG_P95_SECONDS,
            }
        } else {
            Precision {
                method: "audio_envelope",
                resolution_seconds: 1.0 / FEATURE_RATE as f64,
                strong_p95_seconds: 0.0,
            }
        },
    )?;
    result.warnings.extend(warnings);
    let matched: usize = anchors.iter().map(Vec::len).sum::<usize>().max(1);
    if precise && !silent_evidence.is_empty() {
        result.quality = "limited".into();
        for segment in &mut result.segments {
            segment.quality = "limited".into();
        }
        for run in &silent_evidence {
            result.warnings.push(format!(
                "{} {:.1}-{:.1} s has sound that matches no mapping (inside a matched stretch): the take may have a cut, dropout or restart there; sync the material on each side separately",
                if swapped { "reference" } else { "asset" },
                run[0],
                run[1]
            ));
        }
    }
    // A coherent contradicting stretch is decisive however short the rest of
    // the take makes it; scattered outside matches need a minimum share.
    if precise
        && (contradicting_runs >= 3
            || (unexplained.len() >= 3 && unexplained.len() * 20 >= matched))
    {
        let lo = unexplained.first().map_or(0.0, |p| p.asset);
        let hi = unexplained.last().map_or(0.0, |p| p.asset);
        result.quality = "limited".into();
        for segment in &mut result.segments {
            segment.quality = "limited".into();
        }
        result.warnings.push(format!(
            "{} strong audio matches between {} {lo:.1} and {hi:.1} s fit no mapping: the take may contain a short cut, dropout or restart there; sync the material on each side separately",
            unexplained.len(),
            if swapped { "reference" } else { "asset" }
        ));
    }
    if !precise {
        result.warnings.push("only energy-envelope alignment succeeded; waveform validation did not establish sub-frame accuracy".into());
    }
    if ri.duration > options.max_audio_seconds || ai.duration > options.max_audio_seconds {
        result.warnings.push(format!("metadata indicates recordings exceed the analysis limit; analyzed only the first {} seconds of each recording; mapping is evidenced only over the reported matched interval",options.max_audio_seconds));
    }
    result.warnings.push("audio alignment includes microphone propagation delay; internal camera audio/video latency is not measured".into());
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fft_convolution_matches_direct_dot_products() {
        let r: Vec<_> = (0..73)
            .map(|i| ((i * i + 13 * i + 1) as f64).sin())
            .collect();
        let q: Vec<_> = (0..17).map(|i| ((i * 7 + 3) as f64).cos()).collect();
        let c = Correlator::new(&r, q.len()).unwrap();
        let observed = c.peaks(&q, r.len(), 1, false);
        let mean = q.iter().sum::<f64>() / q.len() as f64;
        assert_eq!(observed.len(), r.len() - q.len() + 1);
        for (start, score) in observed {
            let window = &r[start..start + q.len()];
            let wm = window.iter().sum::<f64>() / q.len() as f64;
            let numerator = window
                .iter()
                .zip(&q)
                .map(|(a, b)| (a - wm) * (b - mean))
                .sum::<f64>();
            let denominator = (window.iter().map(|a| (a - wm).powi(2)).sum::<f64>()
                * q.iter().map(|b| (b - mean).powi(2)).sum::<f64>())
            .sqrt();
            assert!(
                (score - numerator / denominator).abs() < 1e-9,
                "{start}: {score}"
            );
        }
    }
    #[test]
    fn constant_audio_has_no_correlation_peak() {
        let c = Correlator::new(&[1.0; 100], 20).unwrap();
        assert!(c.peaks(&[1.0; 20], 3, 1, false).is_empty());
    }
}
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::Write;
    fn hash(mut x: u64) -> f64 {
        x = x.wrapping_add(0x9e3779b97f4a7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 31;
        ((x >> 32) as u32) as f64 / u32::MAX as f64
    }
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
    fn wav(path: &Path, seconds: f64, rate: u32, offset: f64, scale: f64, silent: bool) {
        let count = (seconds * rate as f64) as u32;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + count * 2).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&(rate * 2).to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(count * 2).to_le_bytes()).unwrap();
        for i in 0..count {
            let sample = if silent {
                0
            } else {
                (signal(offset + scale * i as f64 / rate as f64) * i16::MAX as f64) as i16
            };
            f.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
    #[test]
    fn audio_sync_uses_only_bounded_decode_without_exact_timing_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        wav(&reference, 35.0, 8000, 0.0, 1.0, false);
        wav(&asset, 25.0, 8000, 2.123, 1.0, false);
        let options = SyncOptions {
            max_audio_seconds: 20.0,
            ..SyncOptions::default()
        };
        crate::media::hash_tests::READ_BYTES.with(|bytes| bytes.set(0));
        let result = align_audio(&reference, &asset, &options).unwrap();
        crate::media::hash_tests::READ_BYTES.with(|bytes| assert_eq!(bytes.get(), 0));
        assert!((result.offset_seconds - 2.123).abs() < 0.01, "{result:?}");
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("metadata indicates"))
        );
    }
    #[test]
    fn audio_sync_ffmpeg_offset_and_one_percent_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        wav(&reference, 80.0, 8000, 0.0, 1.0, false);
        wav(&asset, 60.0, 8000, 7.123, 1.01, false);
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert!((result.offset_seconds - 7.123).abs() < 0.01, "{result:?}");
        assert!((result.scale - 1.01).abs() < 0.0002, "{result:?}");
        assert_eq!(result.method, "audio_waveform");
        let inverse = align_audio(&asset, &reference, &SyncOptions::default()).unwrap();
        assert!((inverse.offset_seconds + 7.123 / 1.01).abs() < 0.01);
        assert!((inverse.scale - 1.0 / 1.01).abs() < 0.0002);
    }
    #[test]
    fn time_warped_asset_is_limited_even_within_match_tolerance() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        let warped = tmp.path().join("warped.wav");
        wav(&reference, 50.0, 8000, 0.0, 1.0, false);
        wav(&asset, 40.0, 8000, 3.0, 1.0, false);
        let clean = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_eq!(clean.quality, "strong", "{clean:?}");
        assert_eq!(clean.strong_p95_limit_seconds, WAVEFORM_STRONG_P95_SECONDS);
        // WSOLA timing wanders by several ms, inside the 40 ms fit tolerance.
        let mut args: Vec<OsString> = ["-nostdin", "-v", "error", "-i"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.push(asset.into_os_string());
        args.extend(["-af", "atempo=1.0005"].into_iter().map(Into::into));
        args.push(warped.clone().into_os_string());
        crate::process::run("ffmpeg", &args, Duration::from_secs(30), 1024 * 1024).unwrap();
        let result = align_audio(&reference, &warped, &SyncOptions::default()).unwrap();
        assert_eq!(result.method, "audio_waveform");
        assert!(
            result.p95_error_seconds > WAVEFORM_STRONG_P95_SECONDS,
            "{result:?}"
        );
        assert_eq!(result.quality, "limited", "{result:?}");
    }
    #[test]
    fn audio_sync_different_sample_rates_are_not_clock_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        wav(&reference, 65.0, 48000, 0.0, 1.0, false);
        wav(&asset, 50.0, 44100, 4.375, 1.0001, false);
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert!((result.offset_seconds - 4.375).abs() < 0.01, "{result:?}");
        assert!((result.scale - 1.0001).abs() < 0.00005, "{result:?}");
    }
    #[test]
    fn audio_sync_rejects_silence_and_unrelated_recordings() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        wav(&reference, 40.0, 8000, 0.0, 1.0, false);
        wav(&asset, 30.0, 8000, 0.0, 1.0, true);
        assert!(align_audio(&reference, &asset, &SyncOptions::default()).is_err());
        wav(&asset, 30.0, 8000, 137.0, 1.0, false);
        assert!(align_audio(&reference, &asset, &SyncOptions::default()).is_err());
    }
    #[test]
    fn audio_sync_preserves_delayed_audio_and_nonzero_container_pts() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("camera.wav");
        let delayed = tmp.path().join("delayed.mkv");
        wav(&reference, 80.0, 8000, 0.0, 1.0, false);
        wav(&asset, 60.0, 8000, 7.123, 1.001, false);
        let mut args: Vec<OsString> = [
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:r=10:d=65",
            "-itsoffset",
            "1.25",
            "-i",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(asset.into_os_string());
        args.extend(
            [
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "mpeg4",
                "-c:a",
                "pcm_s16le",
                "-output_ts_offset",
                "4.0",
            ]
            .into_iter()
            .map(Into::into),
        );
        args.push(delayed.clone().into_os_string());
        crate::process::run("ffmpeg", &args, Duration::from_secs(30), 1024 * 1024).unwrap();
        let info = crate::media::probe(&delayed).unwrap();
        assert!(info.start_time > 3.9, "{info:?}");
        let result = align_audio(&reference, &delayed, &SyncOptions::default()).unwrap();
        let expected = 7.123 - 1.25 * 1.001;
        assert!(
            (result.offset_seconds - expected).abs() < 0.02,
            "expected {expected}, {result:?}"
        );
        assert!((result.scale - 1.001).abs() < 0.0002, "{result:?}");
    }
    /// A WAV whose time t plays signal(offset_i + t - start_i) in piece i.
    fn pieces(path: &Path, rate: u32, pieces: &[(f64, f64)], seconds: f64) {
        let tmp = path.with_extension("parts");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut samples = Vec::new();
        for (i, &(start, offset)) in pieces.iter().enumerate() {
            let end = pieces.get(i + 1).map_or(seconds, |p| p.0);
            let part = tmp.join(format!("{i}.wav"));
            wav(&part, end - start, rate, offset, 1.0, false);
            let bytes = std::fs::read(&part).unwrap();
            samples.extend_from_slice(&bytes[44..]);
        }
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let count = samples.len() as u32 / 2;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + count * 2).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            rate.to_le_bytes(),
            (rate * 2).to_le_bytes(),
        ] {
            f.write_all(&v).unwrap();
        }
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(count * 2).to_le_bytes()).unwrap();
        f.write_all(&samples).unwrap();
    }
    #[test]
    fn continuous_take_is_one_segment_with_dense_anchors() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("main.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 90.0, 8000, 0.0, 1.0, false);
        wav(&asset, 80.0, 8000, 5.5, 1.0002, false);
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert!(!result.discontinuous, "{result:?}");
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.quality, "strong");
        assert!(result.inliers > 24, "dense anchors: {result:?}");
        assert!(result.inlier_ratio <= 1.0, "{result:?}");
        // Anchors reach to about 1 s from each end of the shared audio.
        assert!(result.anchor_span_seconds > 70.0, "{result:?}");
        assert!((result.scale - 1.0002).abs() < 2e-6, "{result:?}");
        assert!((result.offset_seconds - 5.5).abs() < 2e-4, "{result:?}");
        assert_eq!(result.segments[0].asset_range, [0.0, 80.0]);
    }
    #[test]
    fn recorder_pause_splits_into_located_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 100.0, 8000, 0.0, 1.0, false);
        // Recorder: reference 3..50, paused 4 s, then reference 54..93.
        pieces(&asset, 8000, &[(0.0, 3.0), (47.0, 54.0)], 86.0);
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert!(result.discontinuous, "{result:?}");
        assert_eq!(result.quality, "limited");
        assert_eq!(result.segments.len(), 2, "{result:?}");
        let [a, b] = [&result.segments[0], &result.segments[1]];
        assert!((a.offset_seconds - 3.0).abs() < 0.002, "{a:?}");
        assert!((b.offset_seconds - 7.0).abs() < 0.002, "{b:?}");
        assert_eq!(a.quality, "strong");
        assert_eq!(b.quality, "strong");
        // Each segment stops the 0.3 s location margin short of the boundary.
        assert!((a.asset_range[1] - 46.7).abs() < 0.3, "{a:?}");
        assert!((b.asset_range[0] - 47.3).abs() < 0.3, "{b:?}");
        assert!(
            a.asset_range[1] < 47.0 && b.asset_range[0] > 47.0,
            "{a:?} {b:?}"
        );
        assert!(b.asset_range[0] >= a.asset_range[1]);
        assert!(result.warnings.iter().any(|w| w.contains("discontinuity")));
    }
    #[test]
    fn small_dropout_splits_by_residual_step() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 100.0, 8000, 0.0, 1.0, false);
        // 30 ms of the recorder lost at its 45 s.
        pieces(&asset, 8000, &[(0.0, 4.0), (45.0, 49.03)], 90.0);
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_eq!(result.segments.len(), 2, "{result:?}");
        assert!((result.segments[0].offset_seconds - 4.0).abs() < 0.002);
        assert!((result.segments[1].offset_seconds - 4.03).abs() < 0.002);
        assert!((result.segments[1].asset_range[0] - 45.3).abs() < 0.5);
    }
    #[test]
    fn short_nested_stretch_with_another_mapping_is_not_strong() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 100.0, 8000, 0.0, 1.0, false);
        // reference = 4 + t, except 40..50 s of the asset at 4.1 + t.
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (40.0, 44.1), (50.0, 54.0)],
            90.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        let wrong_and_strong = result.segments.iter().any(|s| {
            s.quality == "strong"
                && s.asset_range[0] < 48.0
                && s.asset_range[1] > 42.0
                && (s.offset_seconds + s.scale * 45.0 - 49.1).abs() > 0.01
        });
        assert!(!wrong_and_strong, "{result:?}");
        assert_ne!(result.quality, "strong", "{result:?}");
    }
    #[test]
    fn long_nested_stretch_becomes_its_own_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 110.0, 8000, 0.0, 1.0, false);
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (36.0, 40.1), (60.0, 64.0)],
            100.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_eq!(result.segments.len(), 3, "{result:?}");
        let offsets: Vec<f64> = result.segments.iter().map(|s| s.offset_seconds).collect();
        for (got, want) in offsets.iter().zip([4.0, 4.1, 4.0]) {
            assert!((got - want).abs() < 0.002, "{offsets:?}");
        }
        assert!(
            (result.segments[1].asset_range[0] - 36.3).abs() < 0.5,
            "{result:?}"
        );
        assert!(
            (result.segments[1].asset_range[1] - 59.7).abs() < 0.5,
            "{result:?}"
        );
    }
    #[test]
    fn short_shifted_stretch_in_a_long_take_is_not_strong() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 310.0, 8000, 0.0, 1.0, false);
        // 10 s shifted by 100 ms: about 3% of the take's anchors.
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (150.0, 154.1), (160.0, 164.0)],
            300.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_ne!(result.quality, "strong", "{:?}", result.warnings);
    }
    #[test]
    fn weaker_nested_mapping_beyond_the_search_window_splits_its_host() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 200.0, 8000, 0.0, 1.0, false);
        // Offset 4 s, except a 40 s middle stretch at 5 s (beyond the 0.3 s probe search).
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (70.0, 75.0), (110.0, 114.0)],
            180.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        for segment in &result.segments {
            if segment.quality != "strong" {
                continue;
            }
            for t in [
                segment.asset_range[0] + 0.01,
                (segment.asset_range[0] + segment.asset_range[1]) / 2.0,
                segment.asset_range[1] - 0.01,
            ] {
                let truth = if (70.0..110.0).contains(&t) {
                    5.0 + t
                } else {
                    4.0 + t
                };
                let got = segment.offset_seconds + segment.scale * t;
                assert!(
                    (got - truth).abs() < 0.002,
                    "segment {segment:?} wrong at {t}"
                );
            }
        }
        assert_eq!(result.segments.len(), 3, "{:?}", result.segments);
    }
    /// No strong segment may claim sound at a time where its mapping is wrong.
    fn assert_no_wrong_strong(result: &SyncResult, truth: impl Fn(f64) -> f64) {
        for segment in result.segments.iter().filter(|s| s.quality == "strong") {
            let [lo, hi] = segment.asset_range;
            for i in 0..=20 {
                let t = lo + (hi - lo) * i as f64 / 20.0;
                let got = segment.offset_seconds + segment.scale * t;
                assert!(
                    (got - truth(t)).abs() < 0.002,
                    "{segment:?} wrong at {t}: {:?}",
                    result.warnings
                );
            }
        }
    }
    #[test]
    fn dropped_host_piece_is_not_claimed_by_the_nested_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 320.0, 8000, 0.0, 1.0, false);
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (10.0, 15.0), (50.0, 54.0)],
            300.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_no_wrong_strong(&result, |t| {
            if (10.0..50.0).contains(&t) {
                5.0 + t
            } else {
                4.0 + t
            }
        });
        // Whatever its quality, no segment may claim the first 10 s with another mapping.
        for segment in &result.segments {
            for t in [2.0, 5.0, 8.0] {
                if segment.asset_range[0] <= t && t <= segment.asset_range[1] {
                    let got = segment.offset_seconds + segment.scale * t;
                    assert!((got - (4.0 + t)).abs() < 0.002, "{segment:?} claims {t}");
                }
            }
        }
    }
    #[test]
    fn adjacent_shifted_stretches_are_each_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        wav(&reference, 310.0, 8000, 0.0, 1.0, false);
        pieces(
            &asset,
            8000,
            &[(0.0, 4.0), (150.0, 154.1), (160.0, 164.2), (170.0, 174.0)],
            300.0,
        );
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_ne!(result.quality, "strong", "{:?}", result.warnings);
        assert_no_wrong_strong(&result, |t| {
            if (150.0..160.0).contains(&t) {
                4.1 + t
            } else if (160.0..170.0).contains(&t) {
                4.2 + t
            } else {
                4.0 + t
            }
        });
    }
    fn custom(path: &Path, seconds: f64, f: impl Fn(f64) -> f64) {
        let rate = 8000u32;
        let count = (seconds * rate as f64) as u32;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        out.write_all(b"RIFF").unwrap();
        out.write_all(&(36 + count * 2).to_le_bytes()).unwrap();
        out.write_all(b"WAVEfmt ").unwrap();
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            rate.to_le_bytes(),
            (rate * 2).to_le_bytes(),
        ] {
            out.write_all(&v).unwrap();
        }
        out.write_all(&2u16.to_le_bytes()).unwrap();
        out.write_all(&16u16.to_le_bytes()).unwrap();
        out.write_all(b"data").unwrap();
        out.write_all(&(count * 2).to_le_bytes()).unwrap();
        for i in 0..count {
            let v = f(i as f64 / rate as f64);
            out.write_all(&((v * i16::MAX as f64) as i16).to_le_bytes())
                .unwrap();
        }
    }
    #[test]
    fn independent_hiss_in_a_long_pause_does_not_downgrade() {
        let tmp = tempfile::tempdir().unwrap();
        let reference = tmp.path().join("camera.wav");
        let asset = tmp.path().join("rec.wav");
        let speech = |t: f64| !(25.0..100.0).contains(&t);
        // Shared sound except a 75 s pause filled by each microphone's own hiss.
        custom(&reference, 130.0, |t| {
            if speech(t) {
                signal(t)
            } else {
                0.01 * (2.0 * hash(((t * 8000.0) as u64) * 2) - 1.0)
            }
        });
        custom(&asset, 120.0, |t| {
            let r = t + 3.0;
            if speech(r) {
                signal(r)
            } else {
                0.01 * (2.0 * hash(((t * 8000.0) as u64) * 2 + 1) - 1.0)
            }
        });
        let result = align_audio(&reference, &asset, &SyncOptions::default()).unwrap();
        assert_eq!(result.quality, "strong", "{:?}", result.warnings);
        assert!((result.offset_seconds - 3.0).abs() < 0.002, "{result:?}");
    }
}
