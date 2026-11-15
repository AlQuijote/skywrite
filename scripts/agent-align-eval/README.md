# Agent alignment evaluation

Can a fresh headless agent replace a camera's scratch audio with a separate
recorder's audio using only the `skywrite` binary and its built-in skills?
Everything here is Python standard library plus FFmpeg (with the `flite`
filter for synthetic speech). Media and results go to a scratch folder you
name, never into the repository.

| File | Purpose |
| --- | --- |
| `media.py` | Ground-truth generator: clean recorder plus a damaged, drifting camera MP4 (acoustic delay, band limit, room reverb, noise, compression, AAC); recorder dropouts and camera cuts; writes `truth.json` |
| `score.py` | Scores an edit against `truth.json`: placement error sampled every second, overlap, muted camera sound, `edit validate`, "start rounded" warnings, refusals |
| `run_eval.py` | `prepare` builds the six scenarios; `run` executes each agent in a fresh temporary folder (binary on PATH, `SKYWRITE_HOME`/`SKYWRITE_CACHE_DIR` inside it, no API key) and scores it; `summary` prints a round |
| `bench_long_takes.py` | Sync accuracy against take length (2, 10, 30, 60 min) and drift (20.8, 104.2, 208.4 ppm), clean and scratch-damaged |
| `fetch_libriwasn.py` | Fetches single real recordings from LibriWASN (CC BY 4.0) by HTTP range requests, with attribution |

```sh
python3 scripts/agent-align-eval/run_eval.py prepare /tmp/align-eval
python3 scripts/agent-align-eval/run_eval.py run /tmp/align-eval --skywrite target/release/skywrite --round 1
python3 scripts/agent-align-eval/run_eval.py summary /tmp/align-eval --round 1
```

Scenarios: `early` (recorder rolls 6.3 s first), `late` (recorder starts 9.47 s
late and stops first; trimmed camera clip), `drift` (5 min, 208 ppm, 29.97 fps),
`long` (30 min, 104 ppm, 23.976 fps), `short` (7 s of overlap: expect a refusal,
or a placement flagged as limited) and `unrelated` (different speech: expect a
refusal). A placement passes when every sampled point is within 3 ms of the
audio truth (which includes about 1 ms of scratch-chain filter delay), the
camera sound is muted, the overlap is complete to within two frames and the edit
validates without "start rounded" warnings.

Agent runs spend model credits (`claude -p --model claude-opus-5-5`,
`codex exec -m gpt-6.1-sol`); both run with permissions bypassed inside the
temporary folder. Runs are sequential.
