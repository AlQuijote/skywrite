# Model evaluation, 2026-11-02

Skywrite's starter defaults for frame description, video description and
transcription come from a measured comparison of cheap OpenRouter models on the
AMI Meeting Corpus (ES2002a close-ups, corner camera and headset mix; CC BY 4.0,
consenting participants, with manual annotations) and a Big Buck Bunny trailer
(CC BY 3.0) with real cuts. All requests used `provider.data_collection: deny`.
Total spend was $0.48. Small samples: treat the numbers as directional.

## Frame description (22 frames, one frame per request)

| Model | Invented details | People count accuracy | Mid-speech (right/wrong/abstain of 11) | $ per 1,000 frames |
|---|---|---|---|---|
| **openai/gpt-6-luna** (default; reasoning off) | **0** | 0.84 | 8/1/2 | 0.22 |
| **google/gemini-3.1-flash-lite** (runner-up) | 2 | 0.95 | 11/0/0 | 0.80 |
| z-ai/glm-5.3-flash | 4 | 0.95 | 10/1/0 | 0.12 |
| qwen/qwen3.7-flash (no zero-retention provider) | 5 | 0.95 | 11/0/0 | 0.07 |
| google/gemini-3.5-flash-lite (analysis default) | 1 | 0.84 | 10/1/0 | 1.14 |
| amazon/nova-2-lite-v1 | 2 | 0.90 | 7/3/1 | 0.76 |

GPT-6 Luna was the only model with no invented details, and on a near-black frame it
answered "details unclear" instead of inventing figures. It miscounts people in
fisheye wide shots. A one-frame-per-second survey of an hour at 25 fps costs about
$0.66 with Luna.

### Local model check, 2026-11-23 (synthetic known-content frames, not the set above)

The AMI frames above were not available for this check, so a local model was
measured on 8 synthetic 640x360 frames with known content: black, SMPTE bars,
a Mandelbrot fractal, FFmpeg `testsrc2`, and four text cards ("SKYWRITE 42",
"EXIT", "Chapter 3 / The Harbor", "OPEN 9:30"). None contains people. The local
model ran through `llama-server` (llama.cpp b12650, Metal, `-c 8192`,
`--reasoning-budget 0`) on an Apple M4 Mac mini, with GPT-6 Luna on the same
frames for comparison.

| Model | Valid JSON | Correct zero-people | Expected text read | Invented text on text-free frames | Speed | Cost |
|---|---|---|---|---|---|---|
| local:qwen3.5-0.8b (Q8_0 + BF16 mmproj) | 8/8 | 8/8 | 5/5 | 0/3 | ~2.1 s/frame, server peak RSS 1.8 GiB | none (local) |
| openai/gpt-6-luna | 8/8 | 8/8 | 5/5 | 0/3 | API | $0.00018/frame |

The small model matched Luna on this easy set. Its one wrong detail was calling a
beige title card "a dark room", where Luna wrote "plain beige title card". The set
has no people, so it measures nothing about people counts, expressions,
mid-speech or cast matching. Those are where the evaluation above separated
models, and a 0.8B model should be expected to do worse there. Use it for cheap
survey passes, and confirm important frames with a stronger model or by viewing
them.

## Cast matching from text descriptions (38 frames, two conditions)

With distinct descriptions, every model reached precision 1.00 on close-ups, never
claimed a person on an empty seat, and never swapped two participants. With
look-alike decoys (each differing in one attribute), several models confidently
picked the decoy when the deciding feature was not visible, for example labelling a
man seen from behind as the "no glasses" decoy, or inventing "clean-shaven" for a
bearded man. Wrong matches came at 0.8–0.9 confidence. GLM-5.3-flash and GPT-6 Luna
made zero decoy confusions (at the cost of low recall in wide shots). Skywrite
therefore presents cast matching as advisory, asks for the deciding feature, and
documents the failure modes.

## Video description (inline base64 MP4)

Inline local video worked on chat completions for all eight video-capable models up
to 60 s. A 30 MB request failed on Google AI Studio (20,000,000-byte limit), Bedrock
and Seed. **google/gemini-3.1-flash-lite** (default) found every shot change within
0.2 s and timed gestures correctly, with zero-retention routing, at about $0.0017
per minute of 480p video. GLM-5.3-flash gave finer event timing on short clips but
samples a fixed number of frames regardless of length.

## Verbatim transcription (three dense 60-second meeting chunks)

| Model | WER | Filler recall | Repetition recall | Median start error | $ per audio minute |
|---|---|---|---|---|---|
| **elevenlabs/scribe-v2** (default) | **0.158** | **0.67** | **0.90** | 0.05 s | 0.0037 |
| nvidia/parakeet-tdt-0.6b-v3 | 0.201 | 0.63 | 0.70 | 0.09 s | 0.0015 |
| openai/whisper-large-v3 | 0.277 | 0.26 | 0.20 | 0.115 s | 0.00045 |

The well-known Whisper `prompt` trick for keeping disfluencies had no effect through
OpenRouter (byte-identical output), and routing varied between providers, one of
which drops all fillers. Scribe v2 keeps "um", restarts and repeated words (the
"Evanston Evanston" case) and returns tight word timestamps. It also returns
non-speech events such as `[laughs]`, which Skywrite excludes from words.

### On-device Parakeet, 2026-11-23 (rebuilt windows, not the chunks above)

`ondevice:parakeet-tdt-0.6b-v3` runs the same NVIDIA model as the hosted row above,
quantized to 8 bits, inside Skywrite (transcribe.cpp 0.4.2 on ggml). The original
three chunks were not kept, so this check rebuilt the set from the ES2002a
headset mix and its manual word annotations (AMI manual annotations 1.6.2): the
three densest non-overlapping 60-second windows, starting at 562.5 s, 771.5 s and
907.5 s (666 reference words, 14 fillers, 11 immediately repeated words). Scoring:
lowercase, punctuation removed, Levenshtein alignment; a filler or repetition
counts as kept when it aligns to the identical word; start error is measured on
identical aligned words. The numbers are not directly comparable with the table
above (different chunks); no hosted model was re-run, to keep the check free.

| Model | WER | Filler recall | Repetition recall | Median start error | p90 start error | $ per audio minute |
|---|---|---|---|---|---|---|
| ondevice:parakeet-tdt-0.6b-v3 (Q8_0, x86-64 CPU; 0.231 on Apple M4 CPU) | 0.230 | 0.43 (6/14) | 0.55 (6/11) | 0.08 s | 0.20 s | 0 |
| same, Metal (Apple M4) | 0.227 | 0.43 | 0.55 | 0.08 s | 0.20 s | 0 |
| F16 weights (not shipped), Apple M4 CPU | 0.228 | 0.43 | 0.55 | 0.08 s | 0.20 s | 0 |
| Q4_K_M weights (not shipped), Apple M4 CPU | 0.236 | 0.43 | 0.55 | 0.07 s | 0.21 s | 0 |

Parakeet drops more fillers and restarts than Scribe v2, so Scribe v2 stays the
default; the on-device model is the free option when audio must stay on the
computer. Word times come from the model's own token timings on its 80 ms encoder
frames. Q8_0 is pinned: F16 is no more accurate here at 1.7 times the download and
runs slower; Q4_K_M is slightly worse and not faster on Apple silicon.

Speed (engine time for one 60-second window; output is identical for any thread
count):

| Machine | Backend | Time for 60 s | Real-time factor | Memory |
|---|---|---|---|---|
| Mac mini, Apple M4 (10 CPUs, 16 GB) | Metal | 1.8 s (4 threads; 4.3 s using all CPUs) | 0.03 | 0.3 GB |
| same | CPU | 9.5 s (6 threads) to 12.1 s (4 threads) | 0.16–0.20 | 1.5 GB |
| x86-64, 8 CPUs, AVX2, shared machine at load average 15–20 | CPU | 16.4 s (4 threads; 38.5 s using all CPUs) | 0.27 | 1.4 GB |

The first Metal run after installing compiles GPU shaders once (about 20 s).
Skywrite uses half the logical CPUs (1 to 8). CoreML and the Neural Engine are not
used. Linux release binaries are CPU-only: transcribe.cpp's Vulkan and CUDA
backends load system libraries at run time, which a static musl binary cannot do.

## Data policy findings

Every candidate had a provider honouring `data_collection: deny`, but several such
routes are not zero-retention (for example qwen3.7-flash and gemini-2.5-flash-lite).
Skywrite therefore also requests zero data retention by default; GPT-6 Luna then
routes via Azure and the Gemini models via Vertex.
