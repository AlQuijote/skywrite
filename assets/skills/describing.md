# Describe pictures for an edit

Use `skywrite describe` when you need to know what is on screen without watching everything yourself. Each command's `--help` has the exact formats; this is the workflow.

1. **Survey.** `skywrite describe frames SOURCE --every 30` (about one frame per second at 30 fps) or `--interval 1` (one frame per second at any rate). Each frame gets the same general description: shot size, camera angle, people (position, appearance, gaze, expression, mouth, gesture), setting, objects, legible text, image issues, summary.
2. **Ask for what the task needs.** When the general description does not answer the question (where are the faces, is the logo visible, what does the sign say), add `--focus "what you need"`. For an exact JSON shape, write a schema and pass `--focus-schema schema.json`. Read `frames[].focus.answer`. A face-box schema:

   ```json
   {"type":"object","additionalProperties":false,"required":["faces"],"properties":{
     "faces":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["box","position"],
       "properties":{"box":{"x-skywrite":"box"},"position":{"type":"string"}}}}}}
   ```

   The schema subset is listed in `skywrite describe frames --help`. Unsupported keywords are refused by name.
3. **Coordinates.** Normalized 0..1 of the full upright frame, origin top-left, x right, y down. A box is `[x1, y1, x2, y2]` (left, top, right, bottom). Multiply by the frame's pixel width and height to get pixels. Coordinates are **model estimates**: small local models can be far off. Open the frame (`frames[].image`) or draw the box before relying on one, and tell the user they are estimates.
4. **Failures are explicit.** A focus answer that does not match the schema gets one corrective retry (a fixed answer has `attempts: 2` and `first_attempt_errors`). If the retry also fails, the frame's `focus.error` has a code (`focus_schema_mismatch`, `focus_invalid_json`, `focus_request_failed`) and the errors. Rerun the same command to retry only those answers. The general description is kept either way. A valid description or answer that could not be written to the cache comes back with `cache_error`; it is not cached, so a rerun pays again.
5. **Cost and cache.** Descriptions and focus answers are cached separately, per source content, exact frame, model and cast (plus the focus text and schema for focus answers). With nothing cached, one request per frame returns both. With the description cached, only the focus is asked. With both cached, nothing is sent and no API key is needed. Never leave out `--focus` to save money. Before a large run, use the exact command with `--dry-run`: `would_request` is how many initial requests it would make (a corrective focus retry can add one per frame). `skywrite describe cache SOURCE` lists what is already cached (kinds, models, focus previews, frame-time ranges).
6. **Motion.** For what happens over time (gestures, expression changes, shot changes), use `skywrite describe video SOURCE --start S --duration 8`. `--focus` works there too. Times in its answer are seconds from the clip start (add `focus.clip_start`).
7. **Cuts.** Around a candidate cut, run `describe frames SOURCE --around TIME --window 1`, or `skywrite review sheet` for a contact sheet you view yourself. Choose cuts from word timestamps and frames, never from a summary alone.

Cast files (`--cast`) give advisory who-is-visible hints from text descriptions. They are not identification.
