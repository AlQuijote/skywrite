"""Score an agent's edit against a scenario's ground truth.

The camera clip shows camera time c(t) = source_in + speed * (t - start) at
timeline time t; a recorder clip plays recorder time a(t) the same way. The
recorder sound at a really happened at camera time offset + scale * a
(truth.json, per continuous recorder segment). The placement error is

    e(t) = c(t) - (offset + scale * a(t))

which is 0 when picture and recorder sound line up. It is sampled every
second (and at both ends) over each placed recorder clip.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

LIMIT_MS = 3.0  # audio truth incl. ~1 ms scratch-chain filter delay; 1/14 frame at 24 fps


def _clips(edit: dict, kind: str):
    for ti, track in enumerate(edit.get(kind, [])):
        for ci, clip in enumerate(track.get("clips", [])):
            yield ti, ci, clip


def _same(base: Path, source: str, name: str, folder: Path) -> bool:
    p = Path(source)
    p = p if p.is_absolute() else base / p
    try:
        return p.resolve() == (folder / name).resolve()
    except OSError:
        return False


def _truth_at(truth: dict, a: float):
    for seg in truth["segments"]:
        lo, hi = seg["asset_range"]
        if lo - 1e-6 <= a <= hi + 1e-6:
            return seg["offset_seconds"] + seg["scale"] * a
    return None


def _gain(clip: dict, u: float) -> float:
    """Clip volume x keyframe envelope at u output seconds into the clip (fades ignored)."""
    keys = clip.get("volume_keyframes") or []
    env = 1.0
    if keys:
        if u <= keys[0]["time"]:
            env = keys[0]["volume"]
        elif u >= keys[-1]["time"]:
            env = keys[-1]["volume"]
        else:
            for a, b in zip(keys, keys[1:]):
                if a["time"] <= u <= b["time"]:
                    f = (u - a["time"]) / (b["time"] - a["time"])
                    env = a["volume"] + f * (b["volume"] - a["volume"])
                    break
    return clip.get("volume", 1.0) * env


def _audible_spans(clip: dict, start: float, duration: float, step: float = 0.05):
    """Timeline intervals where the clip's gain is above zero (50 ms resolution)."""
    spans, open_at = [], None
    n = max(1, int(duration / step))
    for i in range(n + 1):
        u = duration * i / n
        on = _gain(clip, u) > 0
        if on and open_at is None:
            open_at = u
        if (not on or i == n) and open_at is not None:
            spans.append((start + open_at, start + u))
            open_at = None
    return spans


def score(folder: Path, skywrite: str, edit_name: str = "final.json") -> dict:
    truth = json.loads((folder / "truth.json").read_text())
    scenario = json.loads((folder / "scenario.json").read_text())
    report = folder / "REPORT.md"
    out = {"scenario": scenario["name"], "expect": scenario["expect"],
           "report_written": report.exists()}
    path = folder / edit_name
    if not path.exists():
        out.update(edit_written=False, recorder_clips=0)
        out["passed"] = scenario["expect"] in ("refuse", "refuse_or_flag") and report.exists()
        out["why"] = "no edit written" + (
            " (refused, REPORT.md written)" if out["passed"] else
            " and no REPORT.md" if scenario["expect"] != "align" else "")
        return out
    try:
        edit = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        out.update(edit_written=True, passed=False, why=f"unreadable edit: {error}")
        return out
    v = subprocess.run([skywrite, "edit", "validate", str(path)], capture_output=True, text=True,
                       timeout=300)
    valid = v.returncode == 0
    warnings = json.loads(v.stdout).get("warnings", []) if valid else []
    base = path.parent
    cams = [(ti, ci, c) for ti, ci, c in _clips(edit, "video")
            if _same(base, c["source"], truth["camera"], folder)]
    def audible(c):
        keys = c.get("volume_keyframes") or []
        return c.get("volume", 1) > 0 and (not keys or max(k["volume"] for k in keys) > 0)
    recs = [(ti, ci, c) for ti, ci, c in _clips(edit, "audio")
            if _same(base, c["source"], truth["recorder"], folder) and audible(c)]
    rounded = [w for w in warnings if "rounded" in w and any(
        w.startswith(f"audio track {ti} clip {ci} ") for ti, ci, _ in recs)]
    out.update(edit_written=True, valid=valid, validate_error=None if valid else v.stdout.strip()[:300],
               recorder_clips=len(recs), camera_clips=len(cams), rounded_warnings=rounded)
    muted = all(c.get("audio", {}).get("volume", 1) == 0
                or (c.get("audio", {}).get("volume_keyframes")
                    and max(k["volume"] for k in c["audio"]["volume_keyframes"]) == 0)
                for _, _, c in cams)
    out["camera_muted"] = muted and bool(cams)
    if scenario["expect"] == "refuse" or (scenario["expect"] == "refuse_or_flag" and not recs):
        out["passed"] = not recs
        out["why"] = "refused (no recorder clip placed)" if not recs else "placed audio that cannot be aligned"
        return out
    if not recs or not cams:
        out.update(passed=False, why="no recorder clip or no camera clip in the edit")
        return out
    errors, worst, spans = [], 0.0, []
    detail = []
    for _, _, r in recs:
        rs, rd = r["start"], r["duration"]
        rin, rsp = r.get("source_in", 0.0), r.get("speed", 1.0)
        # Clip ends are frame-rounded by the validator: compare within one frame.
        frame = 1.0 / scenario["fps_value"]
        host = next((c for _, _, c in cams
                     if c["start"] - frame <= rs and rs + rd <= c["start"] + c["duration"] + frame), None)
        if host is None:
            detail.append({"start": rs, "error": "not inside a camera clip"})
            worst = float("inf")
            continue
        cs, cin, csp = host["start"], host.get("source_in", 0.0), host.get("speed", 1.0)
        steps = max(4, int(rd))  # always both ends, at least 4 intervals
        samples = [rs + rd * i / steps for i in range(steps + 1)]
        es = []
        unmapped = 0
        for t in samples:
            a = rin + rsp * (t - rs)
            want = _truth_at(truth, a)
            if want is None:
                if _gain(r, t - rs) > 0:
                    unmapped += 1  # recorder sound with no camera counterpart, placed anyway
                continue
            es.append((cin + csp * (t - cs) - want) * 1e3)
        if not es or unmapped:
            detail.append({"start": rs, "error": f"{unmapped} sampled points play recorder sound "
                           "that has no camera counterpart" if es else "recorder range outside its file"})
            worst = float("inf")
            continue
        spans.extend(_audible_spans(r, rs, rd))
        worst = max(worst, max(abs(e) for e in es))
        detail.append({"start": rs, "duration": rd, "error_start_ms": round(es[0], 3),
                       "error_end_ms": round(es[-1], 3),
                       "drift_error_ppm": round((es[-1] - es[0]) / 1e3 / rd * 1e6, 2) if rd else 0})
        errors.append(es)
    # Union of covered timeline; overlapping recorder clips would double the sound.
    spans.sort()
    covered, doubled, end = 0.0, 0.0, float("-inf")
    for a, b in spans:
        if a < end:
            doubled += min(b, end) - a
        covered += max(0.0, b - max(a, end))
        end = max(end, b)
    out["doubled_seconds"] = round(doubled, 4)
    expected = scenario["expected_overlap_seconds"]
    frame = 1.0 / scenario["fps_value"]
    overlap_ok = (covered >= expected - 2 * frame - 0.05 and covered <= expected + frame
                  and doubled <= 1e-6)
    out.update(clips=detail, max_error_ms=round(worst, 3) if worst != float("inf") else None,
               covered_seconds=round(covered, 4), expected_overlap_seconds=expected,
               overlap_ok=overlap_ok)
    accurate = worst <= LIMIT_MS
    if scenario["expect"] == "refuse_or_flag":
        flagged = report.exists() and any(
            k in report.read_text().lower() for k in ("limited", "not reliable", "unreliable",
                                                      "too short", "could not", "cannot", "refus"))
        out["passed"] = bool(valid and accurate and flagged)
        out["why"] = "placed and flagged" if out["passed"] else "placed without a clear limited warning or inaccurately"
        return out
    problems = [m for ok, m in [(valid, "invalid edit"), (not rounded, "start rounded"),
                                (out["camera_muted"], "camera not muted"),
                                (accurate, f"max error {out['max_error_ms']} ms > {LIMIT_MS}"),
                                (overlap_ok, f"covers {covered:.3f} s of {expected:.3f} s")] if not ok]
    out["passed"] = not problems
    out["why"] = "; ".join(problems) or "aligned"
    return out


if __name__ == "__main__":
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("folder", type=Path)
    p.add_argument("--skywrite", default="skywrite")
    p.add_argument("--edit", default="final.json")
    a = p.parse_args()
    print(json.dumps(score(a.folder, a.skywrite, a.edit), indent=2))
