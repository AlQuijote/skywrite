"""Ground-truth media for dual-system sound tests (Python stdlib + FFmpeg only).

A clean "master" performance stands in for the sound in the room. The recorder
is a clean excerpt of it. The camera is a damaged copy: acoustic delay,
scratch-microphone band-limiting, room reverb, noise, compression, a clock that
runs at a different rate (drift) and AAC in an MP4 with a picture.

Ground truth, with recorder time ``a`` and camera time ``c`` (both from the
first decoded sample of each file):

    c = offset + scale * a
    offset = scale * (recorder_start + acoustic_delay) - camera_start
    scale  = 48000 / asetrate      (exact: asetrate is an integer)

``acoustic_delay`` is the extra path from the talker to the scratch mic. It is
part of the *audio* truth (what an audio sync can measure); picture truth is
``offset - scale * acoustic_delay``. Every subprocess is bounded and run
without a shell; filter strings contain only numbers and fixed names.
"""

from __future__ import annotations

import json
import math
import random
import struct
import subprocess
import wave
from dataclasses import asdict, dataclass, field
from pathlib import Path

RATE = 48000
TIMEOUT = 1800

WORDS = """
about above across after again against air almost along already also although
always among animal answer anything appear apple area around arrive asked
autumn baker balance basket beach beautiful became before began behind believe
below beside better between bicycle bird black blanket blue boat body border
bottle bottom branch bread bridge bright bring broken brother brown building
butter cabin camera candle capital captain carbon careful carpet carry castle
cattle center century chair chance change charge cheese chicken children circle
city clean clear clever climb clock cloud coast coffee collect color common
copper corner cotton country courage cover crowd current dance danger daughter
decide deep desert detail diamond different dinner direct distance doctor dollar
double dragon dream drive during early earth easy eight electric eleven engine
enough evening every example expect explain factory family famous farmer father
feather fellow field finger finish flower follow forest forget forward fourteen
freedom friend frozen future garden gather gentle giant glass golden govern
grass gravel green ground guitar habit hammer handle happen harbor heavy history
holiday honest horizon hospital hundred hunter island jacket journey jungle
kettle kitchen ladder language lantern laughter leather lemon letter library
lighthouse listen little machine magnet market marble meadow measure medicine
memory middle minute mirror modern moment monkey morning mountain music narrow
nature needle never nineteen noble number ocean office orange order outside
paper parent pencil people pepper picture planet pocket poetry potato powder
present pretty problem public purple question quickly quiet rabbit railway
rather reason record river rocket rubber saddle salmon sandwich science season
second seven shadow shelter signal silver simple sister sixteen soldier
something spring square station stomach story stranger street strong student
summer sunlight surface sweater system table teacher thirteen thousand thunder
ticket tomato tomorrow tonight travel trouble twelve umbrella under until valley
velvet village violin visitor wagon walking warm water weather welcome window
winter without wonder wooden yellow yesterday young zebra
""".split()

VOICES = ["slt", "kal", "awb", "rms"]


def run(args: list[str], timeout: int = TIMEOUT) -> None:
    subprocess.run(args, check=True, timeout=timeout, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL)


def ffmpeg(*args: str, timeout: int = TIMEOUT) -> None:
    run(["ffmpeg", "-nostdin", "-hide_banner", "-v", "error", "-y", *args], timeout)


def duration(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        check=True, timeout=120, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return float(out.stdout.strip())


def speech_text(seed: int, words: int) -> str:
    """Deterministic, non-repeating pseudo-sentences (no two passages alike)."""
    rng = random.Random(seed)
    out, sentence = [], []
    for _ in range(words):
        sentence.append(rng.choice(WORDS))
        if len(sentence) >= rng.randint(5, 14):
            out.append(" ".join(sentence).capitalize() + rng.choice([".", ".", ",", "?", "!"]))
            sentence = []
    out.append(" ".join(sentence) + ".")
    return " ".join(out)


def flite_master(path: Path, seconds: float, seed: int, work: Path) -> None:
    """Two alternating flite voices with varied pauses, exactly ``seconds`` long at 48 kHz."""
    work.mkdir(parents=True, exist_ok=True)
    rng = random.Random(seed * 7919 + 1)
    parts: list[Path] = []
    total = 0.0
    turn = 0
    while total < seconds + 5:
        text = work / f"turn{turn:04d}.txt"
        text.write_text(speech_text(seed * 100003 + turn, rng.randint(20, 70)))
        voice = VOICES[(seed + turn) % 2 if turn % 5 else 2 + (turn // 5) % 2]
        part = work / f"turn{turn:04d}.wav"
        pause = rng.uniform(0.25, 1.6)
        ffmpeg("-f", "lavfi", "-i", f"flite=textfile={text}:voice={voice}",
               "-af", f"aresample={RATE},apad=pad_dur={pause:.3f},volume={rng.uniform(0.5, 1.0):.3f}",
               "-ac", "1", "-c:a", "pcm_s16le", str(part))
        parts.append(part)
        total += duration(part)
        turn += 1
    concat(parts, path, seconds, work)


def file_master(path: Path, source: Path, seconds: float, start: float, work: Path) -> None:
    """A real speech recording (for example a public-domain audiobook) as the master."""
    work.mkdir(parents=True, exist_ok=True)
    ffmpeg("-ss", f"{start:.3f}", "-i", str(source), "-vn", "-ac", "1",
           "-af", f"aresample={RATE},apad,atrim=end_sample={round(seconds * RATE)}",
           "-c:a", "pcm_s16le", str(path))


def concat(parts: list[Path], path: Path, seconds: float, work: Path) -> None:
    listing = work / "concat.txt"
    listing.write_text("".join(f"file '{p.name}'\n" for p in parts))
    ffmpeg("-f", "concat", "-safe", "0", "-i", str(listing),
           "-af", f"apad,atrim=end_sample={round(seconds * RATE)}",
           "-c:a", "pcm_s16le", str(path))


def keep_ranges(label: str, ranges: list[tuple[float, float]], out: str) -> str:
    """Sample-exact concatenation of [start, end) second ranges of one stream."""
    n = len(ranges)
    parts = "".join(f"[{label}{i}]" for i in range(n))
    split = f"[{label}]asplit={n}{parts};" if n > 1 else f"[{label}]anull[{label}0];"
    trims = "".join(
        f"[{label}{i}]atrim=start_sample={round(a * RATE)}:end_sample={round(b * RATE)},"
        f"asetpts=N/SR/TB[{label}k{i}];" for i, (a, b) in enumerate(ranges))
    joined = "".join(f"[{label}k{i}]" for i in range(n))
    return f"{split}{trims}{joined}concat=n={n}:v=0:a=1[{out}]"


def room_ir(path: Path, seed: int, rt60: float = 0.35, wet_db: float = -7.0) -> None:
    """Direct path at sample 0 plus an exponentially decaying diffuse tail."""
    rng = random.Random(seed)
    n = int(RATE * min(1.0, rt60 * 1.2))
    wet = 10 ** (wet_db / 20)
    samples = [0.0] * n
    samples[0] = 1.0
    for i in range(int(RATE * 0.003), n):
        samples[i] = wet * rng.gauss(0, 1) * math.exp(-6.9 * i / (rt60 * RATE)) * 0.06
    peak = max(abs(x) for x in samples)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(x / peak * 32000)) for x in samples))


@dataclass
class Camera:
    start: float            # camera starts at this master time (before stretching)
    seconds: float          # camera file length
    asetrate: int = RATE    # 48000 / asetrate = scale (camera seconds per recorder second)
    delay_samples: int = 168  # acoustic delay at 48 kHz (168 = 3.5 ms, about 1.2 m)
    fps: str = "24"
    seed: int = 11
    cuts: list[list[float]] = field(default_factory=list)  # camera-time [at, removed] gaps


@dataclass
class Recorder:
    start: float            # master time of recorder sample 0
    seconds: float
    # [master_time, seconds]: recorder paused (that master audio is missing from it)
    dropouts: list[list[float]] = field(default_factory=list)


def scale_of(camera: Camera) -> float:
    return RATE / camera.asetrate


def truth(camera: Camera, recorder: Recorder) -> dict:
    """Asset (recorder) -> reference (camera) mapping per continuous recorder segment."""
    s = scale_of(camera)
    delay = camera.delay_samples / RATE
    cam_start = round(camera.start * s * RATE) / RATE  # camera_start in stretched time
    segments = []
    rec_t = 0.0
    master = recorder.start
    pieces = sorted(recorder.dropouts)
    bounds = [(p[0], p[1]) for p in pieces] + [(recorder.start + 1e9, 0.0)]
    for at, removed in bounds:
        length = min(at, recorder.start + 1e9) - master
        end_rec = min(rec_t + length, recorder.seconds)
        segments.append({
            "asset_range": [rec_t, end_rec],
            # recorder time a in this segment is master time a - rec_t + master
            "offset_seconds": s * (master - rec_t + delay) - cam_start,
            "scale": s,
        })
        rec_t = end_rec
        master = at + removed
        if rec_t >= recorder.seconds:
            break
    # Camera cuts (in uncut camera time) remove [at, at+removed]: split segments there.
    for at, removed in sorted(camera.cuts):
        split = []
        for seg in segments:
            lo, hi = seg["asset_range"]
            off, sc = seg["offset_seconds"], seg["scale"]
            # off is already shifted by earlier cuts; work in uncut time.
            uncut = off + seg.get("_removed", 0.0)
            a_cut = (at - uncut) / sc
            a_resume = (at + removed - uncut) / sc
            if lo < a_cut:
                split.append({**seg, "asset_range": [lo, min(hi, a_cut)]})
            if hi > a_resume:
                split.append({**seg, "asset_range": [max(lo, a_resume), hi],
                              "offset_seconds": off - removed,
                              "_removed": seg.get("_removed", 0.0) + removed})
        segments = [x for x in split if x["asset_range"][1] > x["asset_range"][0]]
    for seg in segments:
        seg.pop("_removed", None)
    return {
        "mapping": "camera_time = offset_seconds + scale * recorder_time",
        "scale": s,
        "drift_ppm": (s - 1) * 1e6,
        "acoustic_delay_seconds": delay,
        "segments": segments,
        "camera_seconds": camera.seconds - sum(c[1] for c in camera.cuts),
        "recorder_seconds": recorder.seconds,
    }


def build(out: Path, master_seconds: float, camera: Camera, recorder: Recorder,
          seed: int = 1, speech: Path | None = None, speech_start: float = 0.0,
          unrelated_seed: int | None = None, camera_name: str = "camera.mp4",
          recorder_name: str = "recorder.wav", master: Path | None = None,
          clean: bool = False) -> dict:
    """Write the recorder and camera files into ``out`` and return the ground truth.

    ``master`` reuses an existing 48 kHz master WAV (it must be long enough).
    ``clean`` skips the scratch-mic damage (camera = delayed, stretched master),
    which isolates estimator error from the damage chain's own group delay."""
    out.mkdir(parents=True, exist_ok=True)
    work = out / ".work"
    if master is not None:
        work.mkdir(parents=True, exist_ok=True)
    elif speech:
        master = work / "master.wav"
        file_master(master, speech, master_seconds, speech_start, work)
    else:
        master = work / "master.wav"
        flite_master(master, master_seconds, seed, work / "flite")
    rec_master = master
    if unrelated_seed is not None:
        rec_master = work / "unrelated.wav"
        flite_master(rec_master, master_seconds, unrelated_seed, work / "flite-unrelated")
    # Recorder: clean excerpt, quiet room tone, 24-bit WAV. Dropouts remove master audio.
    keep = []
    t = recorder.start
    for at, removed in sorted(recorder.dropouts):
        keep.append((t, at))
        t = at + removed
    keep.append((t, t + recorder.seconds))
    rec = out / recorder_name
    ffmpeg("-i", str(rec_master), "-f", "lavfi", "-i",
           f"anoisesrc=c=brown:a=0.0006:r={RATE}:seed={seed + 5}",
           "-filter_complex",
           f"[0:a]anull[src];{keep_ranges('src', keep, 'kept')};"
           f"[kept]atrim=end_sample={round(recorder.seconds * RATE)}[s];"
           f"[s][1:a]amix=inputs=2:duration=first:normalize=0[m]",
           "-map", "[m]", "-c:a", "pcm_s24le", str(rec))
    # Camera scratch track.
    ir = work / "room_ir.wav"
    room_ir(ir, camera.seed)
    s = scale_of(camera)
    cam_audio = work / "camera_audio.wav"
    start_sample = round(camera.start * s * RATE)
    total = round(camera.seconds * RATE)
    damage = (
        f"highpass=f=160,lowpass=f=5500,aresample={RATE}[d];[d][1:a]afir=dry=1:wet=1[r];"
        f"[2:a]volume=1[n];[r][n]amix=inputs=2:duration=first:normalize=0,"
        f"acompressor=threshold=0.06:ratio=5:attack=4:release=120:makeup=2,"
        if not clean else "anull,")
    chain = (
        f"[0:a]adelay={camera.delay_samples}S,{damage}"
        f"asetrate={camera.asetrate},aresample={RATE}:resampler=soxr:precision=28,"
        f"atrim=start_sample={start_sample}:end_sample={start_sample + total},asetpts=N/SR/TB[c]"
    )
    try:
        ffmpeg("-i", str(master), "-i", str(ir), "-f", "lavfi", "-i",
               f"anoisesrc=c=pink:a=0.012:r={RATE}:seed={camera.seed}",
               "-filter_complex", chain, "-map", "[c]", "-c:a", "pcm_s16le", str(cam_audio))
    except subprocess.CalledProcessError:
        # FFmpeg without soxr: the default resampler is accurate enough for drift.
        ffmpeg("-i", str(master), "-i", str(ir), "-f", "lavfi", "-i",
               f"anoisesrc=c=pink:a=0.012:r={RATE}:seed={camera.seed}",
               "-filter_complex", chain.replace(":resampler=soxr:precision=28", ""),
               "-map", "[c]", "-c:a", "pcm_s16le", str(cam_audio))
    if camera.cuts:
        keep, t = [], 0.0
        for at, removed in sorted(camera.cuts):
            keep.append((t, at))
            t = at + removed
        keep.append((t, camera.seconds))
        cut = work / "camera_cut.wav"
        ffmpeg("-i", str(cam_audio), "-filter_complex",
               f"[0:a]anull[src];{keep_ranges('src', keep, 'kept')}", "-map", "[kept]",
               "-c:a", "pcm_s16le", str(cut))
        cam_audio = cut
    cam = out / camera_name
    ffmpeg("-f", "lavfi", "-i", f"testsrc2=s=320x180:r={camera.fps}", "-i", str(cam_audio),
           "-map", "0:v", "-map", "1:a", "-shortest", "-c:v", "libx264", "-preset", "ultrafast",
           "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k", "-ar", str(RATE), str(cam))
    gt = truth(camera, recorder)
    gt["camera"] = camera_name
    gt["recorder"] = recorder_name
    gt["related"] = unrelated_seed is None
    gt["camera_cuts"] = camera.cuts
    gt["spec"] = {"master_seconds": master_seconds, "seed": seed, "camera": asdict(camera),
                  "recorder": asdict(recorder), "speech": str(speech) if speech else "flite"}
    return gt


def clean_work(out: Path) -> None:
    import shutil
    shutil.rmtree(out / ".work", ignore_errors=True)


if __name__ == "__main__":
    import argparse
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("out", type=Path)
    p.add_argument("--master-seconds", type=float, default=120)
    p.add_argument("--recorder-start", type=float, default=0)
    p.add_argument("--recorder-seconds", type=float, default=100)
    p.add_argument("--camera-start", type=float, default=10)
    p.add_argument("--camera-seconds", type=float, default=90)
    p.add_argument("--asetrate", type=int, default=RATE)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--speech", type=Path)
    a = p.parse_args()
    gt = build(a.out, a.master_seconds, Camera(a.camera_start, a.camera_seconds, a.asetrate),
               Recorder(a.recorder_start, a.recorder_seconds), seed=a.seed, speech=a.speech)
    (a.out / "truth.json").write_text(json.dumps(gt, indent=2))
    clean_work(a.out)
    print(json.dumps(gt, indent=2))
