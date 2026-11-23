# Generation, references, jobs, and upscaling

Choose a model explicitly (`--model`), through its task environment variable, or with `defaults image set MODEL` / `defaults video set MODEL` / `defaults video-upscaling set MODEL`. Discover current models first. Catalog capabilities are provider claims, not compatibility guarantees. Prices use different units; zero generic prompt/completion prices do not make video free.

```sh
skywrite models list --task image-generation
skywrite generate image --model black-forest-labs/flux-3-image-klein \
  --prompt 'An orange origami fox with a blue scarf' --output fox.png
skywrite generate image --model black-forest-labs/flux-3-image-klein \
  --reference fox.png --prompt 'Same fox sitting down' --output pose.png
skywrite generate video --model minimax/hailuo-3-max \
  --first-frame fox.png --prompt 'The paper fox turns its head' \
  --duration 5 --resolution 480p --aspect-ratio 16:9
skywrite jobs list
skywrite jobs resume JOB_ID --wait 60 --output fox.mp4
skywrite upscale video --source-job JOB_ID \
  --model black-forest-labs/flux-video-upscale --factor 2 --creativity 0
skywrite jobs resume UPSCALE_JOB_ID --wait 60 --output fox-large.mp4
skywrite upscale image fox.png --factor 2 --output fox-large.png
```

Image generation accepts repeatable local PNG/JPEG/WebP references. Video generation supports `--first-frame`, `--last-frame`, or repeatable `--reference` images; reference mode cannot be mixed with keyframe mode. The selected model must advertise the relevant input. A list containing both keyframe types does not establish that they work simultaneously: H3 Max accepts **one** keyframe, first OR last, per request. Reference-guided generation may change identity or details; inspect the results before editing.

`upscale image` is local **non-AI Lanczos resizing** through FFmpeg. No dedicated image super-resolution model is currently discovered in OpenRouter's image catalog. Generating a larger image from a reference is a separate generative edit that can invent details.

Dedicated AI video upscaling accepts `--source-job` for a completed generated video, or `--source-url https://...` for a file you host. OpenRouter currently rejects video data URLs and its Files API does not accept MP4 uploads. Use explicit `--upload` to publish a local MP4 through your configured public S3/R2 bucket; its R2 upload and cleanup paths were exercised in the dated [live validation](validation.md). Optional positional local input records provenance; with `--source-job`, its bytes must match the original result. With an arbitrary hosted URL, local bytes cannot establish the remote URL's contents. Signed source URLs are obtained when needed, omitted from printed provenance, and excluded from source-job request identity.

`--dry-run` validates capabilities and reports the intended request without a paid submission; catalog fetches are free. `--offline` uses cached **model metadata only**; generation still contacts OpenRouter. For a source job, obtaining its current signed URL also uses the network.

Jobs live in `~/.skywrite/jobs/` (`SKYWRITE_HOME` overrides the personal root). Each stable request identity has a separate lock file, atomically replaced JSON state, and an original result. Same request and same `--take` reuse that job; `--take second` intentionally creates a new paid variation. Cache removal does not remove durable jobs. Provenance includes model, prompt, parameters, reference hashes, provider job ID, cost when reported, output MIME, hash, and bytes. No API keys are saved. Image format is verified from returned bytes; requested raster publication extensions are converted locally if needed. Existing output files require `--overwrite`.

Before completion, FFmpeg decodes an image frame or the full primary video and its first optional audio stream. Header dimensions alone are insufficient. Validation has a 30-second image or 180-second video deadline and requires at least one decoded video frame. An invalid received image remains recoverable from its saved response; an invalid video remains ready for another download attempt. Neither case submits generation again. A versioned validation marker is bound to the stored artifact hash, so unchanged verified results replay without decoding again. A job record without that marker is rejected, naming the job directory to delete before generating again.

The job lock is held across submission/retrieval; it serializes concurrent identical requests. Intent is fsynced before the paid POST. Video IDs are saved before polling. Image responses are saved before conversion/publication. A submission that fails while connecting (DNS, TCP, TLS or proxy, as reported by the HTTP client before any request byte is written) was provably not sent: the job returns to `prepared`, its `error` says nothing was billed, and the command fails with code `not_sent`; rerun the same command to submit it. Skywrite never retries by itself. Every failure once the request may have left (a reset or closed connection, a timeout after connecting, an unreadable or unrecordable reply such as a video answer without a job ID, an interruption) leaves an uncertain job and **never** triggers an automatic second paid POST. Inspect OpenRouter activity; a known video ID can be adopted with `jobs resume JOB_ID --remote-id PROVIDER_ID`. For an image submission interrupted before its response was saved, recovery may require OpenRouter activity/support; an explicitly new take can incur another charge. Definitive request rejections are recorded separately. Provider failures remain recorded, and retries require an intentional new take.

`generate …`, `upscale video` and `jobs resume` exit 0 only when the job completed or is still running at the provider (`pending`, `in_progress`, `ready`) after the bounded wait. Every other end state exits 1 with `{"error":{"code","message","job"}}` on stderr, where `job` is the receipt `jobs status` shows and `code` is `not_sent` (connection failed, nothing sent; rerun the same command), `job_not_submitted` (`jobs resume` of an ordinary `prepared` job: it never submits one, rerun the generate command; a combined narration job is the exception, its resume requests only the missing sections, see [narration](audio-generation.md)), `job_uncertain`, `job_received` (the provider answered but the saved response could not be completed, for example it does not decode; it is kept and never published, and no new request is made), `job_rejected` or `job_failed`. Rerunning a command whose job is already uncertain, rejected or failed returns the same code without a second request.

`jobs status JOB_ID` reads local state and exits 0 in every state; `--refresh` polls and can retrieve a ready result. `jobs resume` polls/retrieves/publishes without submitting an ordinary job. A poll wait is bounded to 1800 seconds; individual HTTP requests have 15-second connection and 300-second total deadlines. Generated outputs can be unavailable after provider retention expires; retain original local results. Generation cancellation does not cancel an already accepted remote job.

Bounds: 32 KiB prompts; 16 MiB per reference image and 32 MiB combined; 64 megapixels per image/frame; 64 MiB requests/JSON responses; 32 MiB decoded image output; 512 MiB video output. Video downloads use a fixed authenticated OpenRouter content endpoint; redirects receive no bearer header and must resolve to public HTTPS addresses. SVG generation, uploaded audio/video generation references, provider-specific option passthrough, and remote cancellation are not implemented.

Protocol references: [Image API](https://openrouter.ai/docs/guides/overview/multimodal/image-generation), [Video API](https://openrouter.ai/docs/guides/overview/multimodal/video-generation), [video request schema](https://openrouter.ai/docs/api/api-reference/video-generation/submit-a-video-generation-request), [Files upload limitations](https://openrouter.ai/docs/api/api-reference/files/upload-a-file). Recheck current model/provider rules before choosing parameters.

## Optional public S3/R2 uploads

`upscale video FILE --upload [--delete-upload-after]` hosts a local MP4 in an explicitly configured public S3/R2 bucket. Hash reuse and scoped cleanup are described in [storage.md](storage.md). The dated [live R2 pass](validation.md) exercised this path; Amazon S3 itself and other compatible providers were not tested. Existing `--source-job` and `--source-url` routes remain available.
