"""Repeatable agent evaluation: can a fresh headless agent replace a camera's
scratch audio with a separately recorded track, using only skywrite?

    python3 run_eval.py prepare SCRATCH            # build scenario media + ground truth (FFmpeg, offline)
    python3 run_eval.py run SCRATCH --skywrite target/release/skywrite --round 1 \\
        [--agents claude,codex] [--scenarios early,late] [--timeout 1800]
    python3 run_eval.py summary SCRATCH --round 1

Each run copies one scenario folder (camera.mp4, recorder.wav, edit.json; never
the ground truth) and the skywrite binary into a fresh temporary directory
outside the repository, runs the agent there with the plain task below, then
scores final.json against truth.json (score.py). Runs are sequential (one at a
time). Agents: `claude -p --model claude-opus-5-5` and `codex exec -m
gpt-6.1-sol`, both with permissions bypassed inside the temporary directory.
SKYWRITE_HOME and SKYWRITE_CACHE_DIR point into it; no network is needed.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True  # keep the repository free of __pycache__
sys.path.insert(0, str(HERE))
import media  # noqa: E402
from score import score  # noqa: E402

TASK = """This folder holds camera.mp4 (picture, with the camera's own scratch-microphone sound), \
recorder.wav (the same scene recorded by a separate high-quality audio recorder) and edit.json \
(an edit with the camera clip on the timeline).

Replace the camera's scratch audio with the recorder audio and save the edit as final.json in \
this folder. Use the `skywrite` command (on PATH); it documents its own workflows \
(`skywrite --help`, `skywrite skills list`, `skywrite skills read NAME`). Create files only \
inside this folder (skywrite's own nit, cache and settings folders are fine), and do not modify \
camera.mp4, recorder.wav or edit.json.

When you are done, write REPORT.md here: what you did, and anything you could not do reliably. \
If the recorder cannot be aligned reliably, do not guess: leave final.json unwritten and explain \
why in REPORT.md."""

# name: (camera, recorder, fps, camera clip (start, source_in, duration or None), expect, seed)
SCENARIOS = {
    # Recorder rolls 6.3 s before the camera and stops after it; slight negative drift.
    "early": dict(camera=media.Camera(start=16.3, seconds=75, asetrate=48001, fps="24"),
                  recorder=media.Recorder(start=10.0, seconds=95), master=110, fps=(24, 1),
                  clip=(0.0, 0.0, None), expect="align", seed=31),
    # Recorder starts 9.47 s after the camera and stops first; the camera clip is trimmed.
    "late": dict(camera=media.Camera(start=4.0, seconds=90, asetrate=47995, fps="25"),
                 recorder=media.Recorder(start=13.47, seconds=70), master=100, fps=(25, 1),
                 clip=(2.0, 1.5, 85.0), expect="align", seed=32),
    # 5-minute take with 208 ppm drift (62 ms over the take) at 29.97 fps.
    "drift": dict(camera=media.Camera(start=12.0, seconds=300, asetrate=47990, fps="30000/1001"),
                  recorder=media.Recorder(start=9.25, seconds=310), master=330, fps=(30000, 1001),
                  clip=(0.0, 0.0, None), expect="align", seed=33),
    # 30-minute take, 104 ppm (187 ms over the take), 23.976 fps.
    "long": dict(camera=media.Camera(start=5.0, seconds=1800, asetrate=47995, fps="24000/1001"),
                 recorder=media.Recorder(start=2.0, seconds=1806), master=1815, fps=(24000, 1001),
                 clip=(0.0, 0.0, None), expect="align", seed=34),
    # The recorder only shares 7 s with the camera: no reliable drift estimate.
    "short": dict(camera=media.Camera(start=33.0, seconds=60, asetrate=47995, fps="24"),
                  recorder=media.Recorder(start=10.0, seconds=30), master=100, fps=(24, 1),
                  clip=(0.0, 0.0, None), expect="refuse_or_flag", seed=35),
    # The recorder holds a different conversation.
    "unrelated": dict(camera=media.Camera(start=5.0, seconds=60, asetrate=48000, fps="24"),
                      recorder=media.Recorder(start=0.0, seconds=70), master=80, fps=(24, 1),
                      clip=(0.0, 0.0, None), expect="refuse", seed=36, unrelated=77),
}


def expected_overlap(gt: dict, clip_start: float, clip_in: float, clip_len: float) -> float:
    """Timeline seconds of the camera clip during which the recorder has sound."""
    covered = 0.0
    for seg in gt["segments"]:
        lo, hi = seg["asset_range"]
        c0 = seg["offset_seconds"] + seg["scale"] * lo
        c1 = seg["offset_seconds"] + seg["scale"] * hi
        covered += max(0.0, min(c1, clip_in + clip_len) - max(c0, clip_in))
    return covered


def prepare(scratch: Path, only: list[str]) -> None:
    root = scratch / "scenarios"
    for name, spec in SCENARIOS.items():
        if only and name not in only:
            continue
        out = root / name
        if (out / "scenario.json").exists():
            print(f"{name}: exists", flush=True)
            continue
        print(f"{name}: building", flush=True)
        gt = media.build(out / "agent", spec["master"], spec["camera"], spec["recorder"],
                         seed=spec["seed"], unrelated_seed=spec.get("unrelated"))
        media.clean_work(out / "agent")
        num, den = spec["fps"]
        start, clip_in, length = spec["clip"]
        length = length if length is not None else gt["camera_seconds"] - clip_in
        edit = {"version": 1, "name": f"Interview ({name})", "width": 320, "height": 180,
                "fps": {"num": num, "den": den},
                "video": [{"name": "V1", "role": "dialogue", "clips": [
                    {"start": start, "source": "camera.mp4", "source_in": clip_in,
                     "duration": length}]}]}
        (out / "agent" / "edit.json").write_text(json.dumps(edit, indent=2) + "\n")
        overlap = expected_overlap(gt, start, clip_in, length) if gt["related"] else 0.0
        (out / "truth.json").write_text(json.dumps(gt, indent=2))
        (out / "scenario.json").write_text(json.dumps({
            "name": name, "expect": spec["expect"], "fps_value": num / den,
            "camera_clip": {"start": start, "source_in": clip_in, "duration": length},
            "expected_overlap_seconds": overlap}, indent=2))


AGENTS = {
    "claude": lambda prompt: ["claude", "-p", "--no-session-persistence", "--model", "claude-opus-5-5",
                              "--dangerously-skip-permissions", prompt],
    "codex": lambda prompt: ["codex", "exec", "-C", ".", "-m", "gpt-6.1-sol",
                             "--dangerously-bypass-approvals-and-sandbox", "--ephemeral",
                             "--skip-git-repo-check", prompt],
}


def run(scratch: Path, skywrite: Path, round_name: str, agents: list[str], only: list[str],
        timeout: int) -> None:
    results = scratch / "rounds" / round_name
    results.mkdir(parents=True, exist_ok=True)
    for name in SCENARIOS:
        if only and name not in only:
            continue
        source = scratch / "scenarios" / name
        for agent in agents:
            record = results / f"{name}-{agent}.json"
            if record.exists():
                print(f"{name}/{agent}: done already", flush=True)
                continue
            with tempfile.TemporaryDirectory(prefix=f"sky-eval-{name}-{agent}-") as tmp:
                tmp = Path(tmp)
                work = tmp / "work"
                shutil.copytree(source / "agent", work)
                bindir = tmp / "bin"
                bindir.mkdir()
                shutil.copy2(skywrite, bindir / "skywrite")
                env = dict(os.environ)
                env["PATH"] = f"{bindir}{os.pathsep}{env.get('PATH', '')}"
                env["SKYWRITE_HOME"] = str(tmp / "home")
                env["SKYWRITE_CACHE_DIR"] = str(tmp / "cache")
                env.pop("OPENROUTER_API_KEY", None)
                t0 = time.monotonic()
                print(f"{name}/{agent}: running in {work}", flush=True)
                try:
                    proc = subprocess.run(AGENTS[agent](TASK), cwd=work, env=env,
                                          capture_output=True, text=True, timeout=timeout,
                                          stdin=subprocess.DEVNULL)
                    status, stdout, stderr = proc.returncode, proc.stdout, proc.stderr
                except subprocess.TimeoutExpired as e:
                    status = "timeout"
                    stdout = (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
                    stderr = (e.stderr or b"").decode(errors="replace") if isinstance(e.stderr, bytes) else (e.stderr or "")
                elapsed = time.monotonic() - t0
                # Score with the ground truth beside the agent's files.
                shutil.copy2(source / "truth.json", work / "truth.json")
                shutil.copy2(source / "scenario.json", work / "scenario.json")
                result = score(work, str(bindir / "skywrite"))
                keep = results / f"{name}-{agent}"
                keep.mkdir(exist_ok=True)
                for f in ("final.json", "REPORT.md"):
                    if (work / f).exists():
                        shutil.copy2(work / f, keep / f)
                for f in work.glob("*.json"):
                    if f.name not in ("truth.json", "scenario.json", "final.json", "edit.json"):
                        shutil.copy2(f, keep / f.name)
                nits = tmp / "home" / "nits"
                if nits.is_dir():
                    shutil.copytree(nits, keep / "nits", dirs_exist_ok=True)
                (keep / "stdout.txt").write_text(stdout[-200000:])
                (keep / "stderr.txt").write_text(stderr[-50000:])
                result.update(agent=agent, exit=status, elapsed_seconds=round(elapsed, 1))
                record.write_text(json.dumps(result, indent=2))
                print(f"{name}/{agent}: {'PASS' if result['passed'] else 'FAIL'} - {result['why']}",
                      flush=True)


def summary(scratch: Path, round_name: str) -> None:
    rows = [json.loads(p.read_text()) for p in sorted((scratch / "rounds" / round_name).glob("*.json"))]
    passed = sum(r["passed"] for r in rows)
    print(f"round {round_name}: {passed}/{len(rows)} passed")
    for r in rows:
        print(f"  {r['scenario']:10s} {r['agent']:7s} {'PASS' if r['passed'] else 'FAIL'}  "
              f"max_err={r.get('max_error_ms')} ms  {r['why']}  ({r['elapsed_seconds']} s)")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("prepare")
    a.add_argument("scratch", type=Path)
    a.add_argument("--scenarios", default="")
    b = sub.add_parser("run")
    b.add_argument("scratch", type=Path)
    b.add_argument("--skywrite", type=Path, required=True)
    b.add_argument("--round", required=True)
    b.add_argument("--agents", default="claude,codex")
    b.add_argument("--scenarios", default="")
    b.add_argument("--timeout", type=int, default=1800)
    c = sub.add_parser("summary")
    c.add_argument("scratch", type=Path)
    c.add_argument("--round", required=True)
    x = p.parse_args()
    only = [s for s in getattr(x, "scenarios", "").split(",") if s]
    if x.cmd == "prepare":
        prepare(x.scratch, only)
    elif x.cmd == "run":
        run(x.scratch, x.skywrite.resolve(), x.round, x.agents.split(","), only, x.timeout)
    else:
        summary(x.scratch, x.round)
