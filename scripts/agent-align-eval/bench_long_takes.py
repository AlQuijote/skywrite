"""Scale accuracy against overlap length: end-of-take error of `skywrite sync`.

Usage: python3 bench_long_takes.py SKYWRITE_BINARY SCRATCH_DIR [--lengths 120,600,1800,3600]

Builds one flite master (or uses --speech FILE), then for each length a recorder
that starts 7 s before a camera of that length, with drift from --asetrate values
(47995 -> +104.18 ppm, 47990 -> +208.42 ppm, 48001 -> -20.83 ppm). Each run
is measured twice: "clean" (camera = stretched recorder audio only, isolating
the estimator) and "scratch" (full scratch-mic damage). Writes results.json.
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True  # keep the repository free of __pycache__
sys.path.insert(0, str(Path(__file__).resolve().parent))
import media  # noqa: E402

p = argparse.ArgumentParser()
p.add_argument("skywrite")
p.add_argument("scratch", type=Path)
p.add_argument("--lengths", default="120,600,1800,3600")
p.add_argument("--asetrate", default="47995,47990,48001")
p.add_argument("--speech", type=Path)
p.add_argument("--damage", default="clean,scratch")
a = p.parse_args()
lengths = [float(x) for x in a.lengths.split(",")]
rates = [int(x) for x in a.asetrate.split(",")]
a.scratch.mkdir(parents=True, exist_ok=True)
master_seconds = max(lengths) + 30
master = a.scratch / "master.wav"
if not master.exists():
    if a.speech:
        media.file_master(master, a.speech, master_seconds, 0.0, a.scratch / "w")
    else:
        media.flite_master(master, master_seconds, 3, a.scratch / "flite")
results = []
for length in lengths:
    for rate in rates:
        for damage in a.damage.split(","):
            cam = media.Camera(start=10.0, seconds=length, asetrate=rate)
            if damage == "clean":
                cam.delay_samples = 0
            rec = media.Recorder(start=3.0, seconds=length + 12)
            out = a.scratch / f"L{int(length)}-r{rate}-{damage}"
            if not (out / "truth.json").exists():
                gt = media.build(out, master_seconds, cam, rec, master=master,
                                 clean=(damage == "clean"))
                (out / "truth.json").write_text(json.dumps(gt, indent=2))
                media.clean_work(out)
            gt = json.loads((out / "truth.json").read_text())
            t0 = time.monotonic()
            run = subprocess.run([a.skywrite, "sync", "--reference", str(out / "camera.mp4"),
                                  "--asset", str(out / "recorder.wav"), "--method", "audio"],
                                 capture_output=True, text=True, timeout=1800)
            elapsed = time.monotonic() - t0
            row = {"length_seconds": length, "drift_ppm_true": gt["drift_ppm"], "damage": damage,
                   "elapsed_seconds": round(elapsed, 1)}
            if run.returncode:
                row["error"] = run.stdout.strip() or run.stderr.strip()[-400:]
            else:
                r = json.loads(run.stdout)
                seg = gt["segments"][0]
                # Asset (recorder) time at the camera's start and end.
                a0 = (0 - seg["offset_seconds"]) / seg["scale"]
                a1 = (length - seg["offset_seconds"]) / seg["scale"]
                err = [r["offset_seconds"] + r["scale"] * t - (seg["offset_seconds"] + seg["scale"] * t)
                       for t in (a0, a1)]
                row.update({
                    "quality": r["quality"], "inliers": r["inliers"],
                    "anchor_span_seconds": r["anchor_span_seconds"],
                    "p95_error_ms": round(r["p95_error_seconds"] * 1e3, 3),
                    "drift_error_ppm": round(r["drift_ppm"] - gt["drift_ppm"], 3),
                    "error_at_take_start_ms": round(err[0] * 1e3, 3),
                    "error_at_take_end_ms": round(err[1] * 1e3, 3),
                    "segments": len(r.get("segments", [1])),
                })
            print(json.dumps(row), flush=True)
            results.append(row)
(a.scratch / "results.json").write_text(json.dumps(results, indent=2))
