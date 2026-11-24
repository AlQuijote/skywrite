//! `--focus` answers and `describe cache` against the owned-loopback
//! OpenAI-compatible server (`tests/fixtures/network/openai_server.py`): one
//! request fills both cache entries, a cached description gets a focus-only
//! request, schema mismatches get one corrective retry, and structured
//! output is requested only where the catalog lists it.
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
}
impl Server {
    fn start(dir: &Path) -> Self {
        let log = dir.join("requests.jsonl");
        let mut child = Command::new("python3")
            .arg("-I")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/network/openai_server.py"),
            )
            .arg(&log)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Self {
            child,
            port: line.trim().parse().unwrap(),
            log,
        }
    }
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
    /// Chat requests only (catalog lookups are GETs).
    fn chats(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|r| r["path"] == "/v1/chat/completions")
            .map(|r| r["body"].clone())
            .collect()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Env {
    dir: tempfile::TempDir,
    server: Server,
}
impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(dir.path());
        let env = Self { dir, server };
        env.ok(&[
            "providers",
            "add",
            "local",
            "--base-url",
            &env.server.base(),
        ]);
        std::fs::write(env.schema(), serde_json::to_vec(&faces_schema()).unwrap()).unwrap();
        env
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn schema(&self) -> PathBuf {
        self.path().join("faces.json")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_skywrite"))
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("SKYWRITE_FRAME_DESCRIPTION_MODEL")
            .env_remove("SKYWRITE_VIDEO_DESCRIPTION_MODEL")
            .env("SKYWRITE_HOME", self.path().join("home"))
            .env("SKYWRITE_CACHE_DIR", self.path().join("cache"))
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
    fn video(&self) -> String {
        let path = self.path().join("v.mp4");
        if !path.exists() {
            let status = Command::new("ffmpeg")
                .args(["-v", "error", "-f", "lavfi", "-i"])
                .arg("testsrc=s=160x90:r=30:d=1")
                .args(["-pix_fmt", "yuv420p"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        path.to_str().unwrap().to_owned()
    }
    fn frames(&self, model: &str, extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = [
            "describe",
            "frames",
            &self.video(),
            "--every",
            "30",
            "--concurrency",
            "1",
            "--model",
            model,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }
}

fn faces_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["faces"],"properties":{
        "faces":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["box","position"],
            "properties":{"box":{"x-skywrite":"box"},"position":{"type":"string"}}}}}})
}

fn refs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

fn prompt(body: &Value) -> String {
    let content = &body["messages"][0]["content"];
    content[0]["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn one_request_fills_both_entries_then_focus_only_then_nothing() {
    let env = Env::new();
    let schema = env.schema();
    let schema = schema.to_str().unwrap();
    let faces = [
        "--focus",
        "bounding box of every face",
        "--focus-schema",
        schema,
    ];
    let args = env.frames("local:vision-small", &faces);

    let dry = env.ok(&[refs(&args), vec!["--dry-run"]].concat());
    assert_eq!(dry["would_request"], 1);
    assert_eq!(dry["focus"], json!({"cached":0,"to_answer":1}));

    // Nothing cached: ONE request returns the description and the focus answer.
    let first = env.ok(&refs(&args));
    assert_eq!(env.server.chats().len(), 1);
    assert_eq!(first["requests"], 1);
    let frame = &first["frames"][0];
    assert_eq!(frame["description"]["summary"], "a test frame");
    assert_eq!(frame["cached"], false);
    assert_eq!(
        frame["focus"]["answer"]["faces"][0]["box"],
        json!([0.1, 0.2, 0.4, 0.8])
    );
    assert_eq!(frame["focus"]["attempts"], 1);
    assert_eq!(frame["focus"]["response_format"], "json_object");
    assert_eq!(
        first["counts"]["focus"],
        json!({"cached":0,"answered":1,"failed":0})
    );
    let body = &env.server.chats()[0];
    assert!(prompt(body).contains("shot_size"));
    assert!(prompt(body).contains("[x1, y1, x2, y2]"));
    assert_eq!(body["response_format"]["type"], "json_object");

    // Both cached: no request, no key.
    let again = env.ok(&[refs(&args), vec!["--dry-run"]].concat());
    assert_eq!(again["would_request"], 0);
    let again = env.ok(&refs(&args));
    assert_eq!(env.server.chats().len(), 1);
    assert_eq!(again["frames"][0]["focus"]["cached"], true);
    assert_eq!(again["requests"], 0);

    // The general entry the combined request stored serves a plain run.
    let plain = env.ok(&refs(&env.frames("local:vision-small", &[])));
    assert_eq!(plain["counts"]["cached"], 1);
    assert!(plain["frames"][0].get("focus").is_none());
    assert_eq!(env.server.chats().len(), 1);

    // A different focus misses only the focus entry: a focus-only request.
    let hands = env.ok(&refs(&env.frames(
        "local:vision-small",
        &[
            "--focus",
            "bounding box of every hand",
            "--focus-schema",
            schema,
        ],
    )));
    let chats = env.server.chats();
    assert_eq!(chats.len(), 2);
    let focus_only = prompt(&chats[1]);
    assert!(focus_only.contains("bounding box of every hand"));
    for general in ["shot_size", "camera_angle", "notable_objects", "summary"] {
        assert!(
            !focus_only.contains(general),
            "focus-only prompt has {general}"
        );
    }
    assert_eq!(hands["frames"][0]["cached"], true);
    assert_eq!(hands["frames"][0]["focus"]["cached"], false);
    assert_ne!(
        hands["frames"][0]["focus"]["instructions_sha256"],
        first["frames"][0]["focus"]["instructions_sha256"]
    );

    // Whitespace differences normalize to the same focus: still cached.
    env.ok(&refs(&env.frames(
        "local:vision-small",
        &[
            "--focus",
            "  bounding box of every face \n",
            "--focus-schema",
            schema,
        ],
    )));
    assert_eq!(env.server.chats().len(), 2);

    // A description cached by a plain run first: the focus run sends focus only.
    let cold = env.frames("local:vlm", &[]);
    env.ok(&refs(&cold));
    env.ok(&refs(
        &env.frames("local:vlm", &["--focus", "is anyone smiling?"]),
    ));
    let chats = env.server.chats();
    assert_eq!(chats.len(), 4);
    assert!(!prompt(&chats[3]).contains("shot_size"));
    assert!(prompt(&chats[3]).contains("is anyone smiling?"));
}

#[test]
fn schema_mismatch_gets_one_corrective_retry_then_a_typed_failure() {
    let env = Env::new();
    let schema = env.schema();
    let faces = [
        "--focus",
        "bounding box of every face",
        "--focus-schema",
        schema.to_str().unwrap(),
    ];
    let fixed = env.ok(&refs(&env.frames("local:focus-bad", &faces)));
    let focus = &fixed["frames"][0]["focus"];
    assert_eq!(focus["attempts"], 2, "{focus}");
    assert!(
        focus["first_attempt_errors"][0]
            .as_str()
            .unwrap()
            .contains("unexpected key \"extra\""),
        "{focus}"
    );
    assert_eq!(
        focus["answer"]["faces"][0]["box"],
        json!([0.1, 0.2, 0.4, 0.8])
    );
    let chats = env.server.chats();
    assert_eq!(chats.len(), 2);
    let retry = &chats[1]["messages"];
    assert_eq!(retry.as_array().unwrap().len(), 3);
    assert_eq!(retry[1]["role"], "assistant");
    assert!(
        retry[2]["content"]
            .as_str()
            .unwrap()
            .contains("unexpected key \"extra\"")
    );

    // Never valid: two attempts, then a typed failure. Every focus failed: the command fails.
    let error = env.fails(&refs(&env.frames("local:focus-never", &faces)));
    assert!(error.contains("every focus answer failed"), "{error}");
    assert!(error.contains("focus_schema_mismatch"), "{error}");
    assert!(error.contains("x1 < x2"), "{error}");
    assert_eq!(env.server.chats().len(), 4);
    // The description was cached anyway; failed focus answers were not.
    let dry = env.ok(&[
        refs(&env.frames("local:focus-never", &faces)),
        vec!["--dry-run"],
    ]
    .concat());
    assert_eq!(dry["cached"], 1);
    assert_eq!(dry["focus"]["to_answer"], 1);

    // One frame fails, the next succeeds: the failure is typed per frame.
    let mut args = env.frames("local:focus-half", &faces);
    args[4] = "15".into();
    let half = env.ok(&refs(&args));
    let failed = &half["frames"][0]["focus"]["error"];
    assert_eq!(failed["code"], "focus_schema_mismatch");
    assert_eq!(failed["attempts"], 2);
    assert!(half["frames"][0]["description"].is_object());
    assert_eq!(half["frames"][1]["focus"]["attempts"], 1);
    assert_eq!(
        half["counts"]["focus"],
        json!({"cached":0,"answered":1,"failed":1})
    );
    let dry = env.ok(&[refs(&args), vec!["--dry-run"]].concat());
    assert_eq!(dry["focus"], json!({"cached":1,"to_answer":1}));
    assert_eq!(dry["would_request"], 1);
}

#[test]
fn json_schema_is_sent_only_where_the_catalog_lists_structured_outputs() {
    let env = Env::new();
    let schema = env.schema();
    let faces = [
        "--focus",
        "bounding box of every face",
        "--focus-schema",
        schema.to_str().unwrap(),
    ];
    let report = env.ok(&refs(&env.frames("local:vlm-structured", &faces)));
    assert_eq!(report["focus"]["structured_outputs"], true);
    let combined = &env.server.chats()[0]["response_format"];
    assert_eq!(combined["type"], "json_schema");
    assert_eq!(combined["json_schema"]["strict"], false);
    let wire = &combined["json_schema"]["schema"]["properties"]["focus"];
    let item = &wire["properties"]["faces"]["items"]["properties"]["box"];
    assert_eq!(item["maxItems"], 4);
    assert!(item.get("x-skywrite").is_none());
    // Focus-only against a cached description: strict.
    env.ok(&refs(&env.frames(
        "local:vlm-structured",
        &[
            "--focus",
            "every hand",
            "--focus-schema",
            schema.to_str().unwrap(),
        ],
    )));
    let focus_only = &env.server.chats()[1]["response_format"];
    assert_eq!(focus_only["json_schema"]["strict"], true);
    assert_eq!(
        env.server.chats()[1]["messages"][0]["content"][1]["type"],
        "image_url"
    );
    // A model the catalog does not mark: json_object plus local validation.
    let plain = env.ok(&refs(&env.frames("local:vision-small", &faces)));
    // The catalog does not list its parameters: unknown, so json_object.
    assert_eq!(plain["focus"]["structured_outputs"], Value::Null);
    assert_eq!(
        env.server.chats()[2]["response_format"]["type"],
        "json_object"
    );
    // No schema: never json_schema, and no catalog lookup is reported.
    let free = env.ok(&refs(
        &env.frames("local:vlm", &["--focus", "describe the light"]),
    ));
    assert_eq!(free["focus"]["structured_outputs"], Value::Null);
    assert_eq!(
        env.server.chats()[3]["response_format"]["type"],
        "json_object"
    );
}

#[test]
fn invalid_focus_inputs_are_refused_before_any_work() {
    let env = Env::new();
    let bad = env.path().join("bad.json");
    std::fs::write(&bad, r#"{"type":"object","oneOf":[]}"#).unwrap();
    let error = env.fails(&refs(&env.frames(
        "local:vision-small",
        &["--focus", "faces", "--focus-schema", bad.to_str().unwrap()],
    )));
    assert!(
        error.contains("unsupported schema keyword") && error.contains("oneOf"),
        "{error}"
    );
    let error = env.fails(&refs(&env.frames(
        "local:vision-small",
        &["--focus-schema", env.schema().to_str().unwrap()],
    )));
    assert!(error.contains("needs --focus"), "{error}");
    let long = "x".repeat(4001);
    let error = env.fails(&refs(
        &env.frames("local:vision-small", &["--focus", &long]),
    ));
    assert!(error.contains("4000"), "{error}");
    assert!(env.server.chats().is_empty());
    assert!(!env.path().join("cache").join("assets").exists());
}

#[test]
fn describe_cache_lists_entries_by_kind_model_and_focus() {
    let env = Env::new();
    let video = env.video();
    let schema = env.schema();
    let empty = env.ok(&["describe", "cache", &video]);
    assert_eq!(empty["totals"]["entries"], 0);
    let mut args = env.frames(
        "local:vision-small",
        &[
            "--focus",
            "bounding box of every face",
            "--focus-schema",
            schema.to_str().unwrap(),
        ],
    );
    args[4] = "15".into();
    env.ok(&refs(&args));
    env.ok(&[
        "describe",
        "video",
        &video,
        "--duration",
        "1",
        "--model",
        "local:vlm",
    ]);
    let listing = env.ok(&["describe", "cache", &video]);
    assert_eq!(listing["totals"]["entries"], 5, "{listing:#}");
    let groups = listing["groups"].as_array().unwrap();
    let frame = groups.iter().find(|g| g["kind"] == "frame").unwrap();
    assert_eq!(frame["model"], "local:vision-small");
    assert_eq!(frame["provider"], "local");
    assert_eq!(frame["prompt_version"], "frame-describe-v1");
    assert_eq!(frame["entries"], 2);
    assert_eq!(
        frame["frame_times"],
        json!([{"start":0.0,"end":0.5,"step":0.5,"count":2}])
    );
    assert!(frame["created"]["first"].as_u64().unwrap() > 0);
    assert!(frame["bytes"].as_u64().unwrap() > 0);
    let focus = groups.iter().find(|g| g["kind"] == "focus").unwrap();
    assert_eq!(focus["of"], "frame");
    assert_eq!(focus["focus"]["preview"], "bounding box of every face");
    assert_eq!(focus["entries"], 2);
    let video_group = groups.iter().find(|g| g["kind"] == "video").unwrap();
    assert_eq!(video_group["ranges"], json!([{"start":0.0,"duration":1.0}]));
    assert_eq!(listing["exact_frame_images"]["count"], 2);

    let only = env.ok(&["describe", "cache", &video, "--model", "local:vlm"]);
    assert_eq!(only["totals"]["entries"], 1);
    let only = env.ok(&[
        "describe",
        "cache",
        &video,
        "--focus",
        "bounding box of every face",
    ]);
    assert_eq!(only["totals"]["entries"], 2);
    let none = env.ok(&["describe", "cache", &video, "--focus", "hands"]);
    assert_eq!(none["totals"]["entries"], 0);
    let kind = env.ok(&["describe", "cache", &video, "--kind", "frame"]);
    assert_eq!(kind["totals"]["entries"], 2);
    // Read-only: the listing makes no requests.
    assert_eq!(env.server.chats().len(), 3);
}

#[test]
fn entries_without_listing_metadata_are_annotated_when_read() {
    let env = Env::new();
    let video = env.video();
    let sha = skywrite::media::file_sha256(Path::new(&video)).unwrap();
    let target = skywrite::providers::resolve("fixture/frames", &Default::default()).unwrap();
    let key = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &target, None).unwrap();
    let path = env
        .path()
        .join("cache/frame-descriptions")
        .join(format!("{key}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        br#"{"version":1,"model":"fixture/frames","description":{"summary":"old"},"usage":{}}"#,
    )
    .unwrap();
    let before = env.ok(&["describe", "cache", &video]);
    assert_eq!(before["unattributed_entries"], 1);
    let report = env.ok(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "fixture/frames",
    ]);
    assert_eq!(report["frames"][0]["description"]["summary"], "old");
    let after = env.ok(&["describe", "cache", &video]);
    assert_eq!(after["unattributed_entries"], 0);
    assert_eq!(after["groups"][0]["model"], "fixture/frames");
}

#[test]
fn video_focus_splits_the_cache_the_same_way() {
    let env = Env::new();
    let video = env.video();
    let base = [
        "describe",
        "video",
        &video,
        "--duration",
        "1",
        "--model",
        "local:vlm",
    ];
    let smile = [&base[..], &["--focus", "when does anyone smile"]].concat();
    let first = env.ok(&smile);
    assert_eq!(env.server.chats().len(), 1);
    assert_eq!(first["cached"], false);
    assert!(first["focus"]["answer"].is_object());
    assert_eq!(first["focus"]["clip_start"], 0.0);
    // The local server reports no cost: null total, the known part and the count stay visible.
    assert_eq!(first["cost_usd"], Value::Null);
    assert_eq!(first["known_cost_usd"], 0.0);
    assert_eq!(first["uncosted_requests"], 1);
    assert_eq!(
        env.server.chats()[0]["messages"][0]["content"][1]["type"],
        "video_url"
    );
    let again = env.ok(&smile);
    assert_eq!(again["focus"]["cached"], true);
    assert_eq!(env.server.chats().len(), 1);
    let dry = env.ok(&[
        &base[..],
        &["--focus", "where is the camera pointed", "--dry-run"],
    ]
    .concat());
    assert_eq!(dry["cached"], true);
    assert_eq!(dry["focus_cached"], false);
    assert_eq!(dry["would_request"], 1);
    let other = env.ok(&[&base[..], &["--focus", "where is the camera pointed"]].concat());
    assert_eq!(other["cached"], true);
    assert!(!prompt(&env.server.chats()[1]).contains("camera_motion"));
    let plain = env.ok(&base);
    assert_eq!(plain["cached"], true);
    assert_eq!(env.server.chats().len(), 2);
}

#[test]
fn help_teaches_focus_coordinates_caching_and_the_listing() {
    let env = Env::new();
    let help = |args: &[&str]| String::from_utf8(env.run(args).stdout).unwrap();
    let frames = help(&["describe", "frames", "--help"]);
    for fact in [
        "--focus <TEXT>",
        "--focus-schema <FILE>",
        "--focus-file <FILE>",
        "normalized numbers 0..1",
        "origin top-left",
        "[x1, y1, x2, y2] = [left, top, right, bottom]",
        "ESTIMATES",
        "x-skywrite",
        "one corrective retry",
        "focus_schema_mismatch",
        "only a focus request is sent",
        "never drop --focus to save money",
        "would_request",
        "skywrite describe cache SOURCE",
        "structured_outputs",
        "--interval 1",
        "4000 bytes",
        "16 KiB",
    ] {
        assert!(
            frames.contains(fact),
            "describe frames --help lacks {fact:?}"
        );
    }
    let video = help(&["describe", "video", "--help"]);
    for fact in [
        "--focus <TEXT>",
        "clip_start",
        "focus-only request",
        "would_request",
    ] {
        assert!(video.contains(fact), "describe video --help lacks {fact:?}");
    }
    let cache = help(&["describe", "cache", "--help"]);
    for fact in [
        "Read-only",
        "content hash",
        "frame_times",
        "--dry-run",
        "--focus <TEXT>",
        "--kind",
    ] {
        assert!(cache.contains(fact), "describe cache --help lacks {fact:?}");
    }
    let short = help(&["describe", "--help"]);
    assert!(short.contains("cache"), "{short}");
    assert!(short.contains("bounding box for every face"), "{short}");
    // The top-level help points at the feature for "what is in the picture" tasks.
    let top = help(&["--help"]);
    assert!(
        top.contains("describe frames SOURCE --interval 1 --focus"),
        "{top}"
    );
    assert!(top.contains("describe cache SOURCE"), "{top}");
    assert!(top.contains("--focus-schema"), "{top}");
    assert!(
        frames.contains("ALWAYS pass a schema"),
        "describe frames --help lacks the schema advice"
    );
    assert!(
        frames.contains("20-60 s per frame"),
        "describe frames --help lacks local timing"
    );
}

#[test]
fn rejected_cache_entries_are_replaced_and_broken_answers_keep_usable_parts() {
    let env = Env::new();
    let schema = env.schema();
    let faces = [
        "--focus",
        "bounding box of every face",
        "--focus-schema",
        schema.to_str().unwrap(),
    ];
    let args = env.frames("local:vision-small", &faces);
    env.ok(&refs(&args));
    // A corrupt focus entry is a miss, and the fresh answer replaces it.
    let dir = env.path().join("cache/focus-answers");
    let entry = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&entry, b"{broken").unwrap();
    env.ok(&refs(&args));
    assert_eq!(env.server.chats().len(), 2);
    let again = env.ok(&refs(&args));
    assert_eq!(again["frames"][0]["focus"]["cached"], true);
    assert_eq!(env.server.chats().len(), 2);
    // An edited answer that would need clamping is a miss too.
    let mut cached: Value = serde_json::from_slice(&std::fs::read(&entry).unwrap()).unwrap();
    cached["description"]["faces"][0]["box"] = json!([-0.01, 0.2, 0.4, 0.8]);
    std::fs::write(&entry, serde_json::to_vec(&cached).unwrap()).unwrap();
    let repaired = env.ok(&refs(&args));
    assert_eq!(repaired["frames"][0]["focus"]["cached"], false);
    assert_eq!(env.server.chats().len(), 3);

    // A combined answer that is not JSON: the focus gets its corrective retry
    // and is cached; the description is a frame error and is not.
    let garbage = env.frames("local:combined-garbage", &faces);
    // Its only frame has no description, so the run fails as before; the
    // focus answer from the same request was still validated and cached.
    let error = env.fails(&refs(&garbage));
    assert!(error.contains("every frame description failed"), "{error}");
    assert!(error.contains("valid JSON"), "{error}");
    assert!(
        error.contains("usable focus answers were cached"),
        "{error}"
    );
    let dry = env.ok(&[refs(&garbage), vec!["--dry-run"]].concat());
    assert_eq!(dry["focus"]["cached"], 1);
    assert_eq!(dry["to_describe"], 1);
    // The rerun asks for the description alone.
    let before = env.server.chats().len();
    env.ok(&refs(&garbage));
    let chats = env.server.chats();
    assert_eq!(chats.len(), before + 1);
    assert!(!prompt(chats.last().unwrap()).contains("FOCUS TASK"));
}

#[test]
fn a_failed_focus_request_keeps_the_cached_description() {
    let env = Env::new();
    let mut plain = env.frames("local:focus-500", &[]);
    plain[4] = "15".into();
    env.ok(&refs(&plain));
    let mut args = env.frames("local:focus-500", &["--focus", "is anyone smiling?"]);
    args[4] = "15".into();
    let report = env.ok(&refs(&args));
    let first = &report["frames"][0];
    assert_eq!(first["cached"], true);
    assert!(first["description"].is_object());
    assert_eq!(first["focus"]["error"]["code"], "focus_request_failed");
    assert!(first.get("error").is_none());
    assert_eq!(report["frames"][1]["focus"]["attempts"], 1);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["focus"]["failed"], 1);
}

#[cfg(unix)]
#[test]
fn annotation_never_writes_through_a_symlinked_cache_directory() {
    let env = Env::new();
    let video = env.video();
    let sha = skywrite::media::file_sha256(Path::new(&video)).unwrap();
    let target = skywrite::providers::resolve("fixture/frames", &Default::default()).unwrap();
    let key = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &target, None).unwrap();
    let outside = env.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let old =
        br#"{"version":1,"model":"fixture/frames","description":{"summary":"old"},"usage":{}}"#;
    std::fs::write(outside.join(format!("{key}.json")), old).unwrap();
    std::fs::create_dir_all(env.path().join("cache")).unwrap();
    std::os::unix::fs::symlink(&outside, env.path().join("cache/frame-descriptions")).unwrap();
    let _ = env.run(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "fixture/frames",
    ]);
    assert_eq!(
        std::fs::read(outside.join(format!("{key}.json"))).unwrap(),
        old
    );
}

#[test]
fn request_failures_keep_cached_parts_and_type_the_focus() {
    let env = Env::new();
    // The first combined request fails outright: that frame's focus is a typed failure.
    let mut args = env.frames("local:combined-500", &["--focus", "is anyone smiling?"]);
    args[4] = "15".into();
    let report = env.ok(&refs(&args));
    let first = &report["frames"][0];
    assert!(
        first["error"]
            .as_str()
            .unwrap()
            .contains("combined backend failed"),
        "{first}"
    );
    assert_eq!(first["focus"]["error"]["code"], "focus_request_failed");
    assert_eq!(report["counts"]["focus"]["failed"], 1);
    assert_eq!(report["frames"][1]["focus"]["attempts"], 1);

    // A cached focus survives a failed description-only request.
    let mut args = env.frames("local:plain-500", &["--focus", "is anyone smiling?"]);
    args[4] = "15".into();
    env.ok(&refs(&args));
    let descriptions = env.path().join("cache/frame-descriptions");
    let sha = skywrite::media::file_sha256(Path::new(&env.video())).unwrap();
    let local = skywrite::providers::resolve(
        "local:plain-500",
        &std::collections::BTreeMap::from([(
            "local".to_owned(),
            skywrite::providers::ProviderConfig {
                base_url: env.server.base(),
                key_env: None,
                headers: Default::default(),
                timeout_seconds: None,
            },
        )]),
    )
    .unwrap();
    let key = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &local, None).unwrap();
    std::fs::remove_file(descriptions.join(format!("{key}.json"))).unwrap();
    let report = env.ok(&refs(&args));
    let first = &report["frames"][0];
    assert!(
        first["error"]
            .as_str()
            .unwrap()
            .contains("description backend failed"),
        "{first}"
    );
    assert_eq!(first["focus"]["cached"], true);
    assert!(first["focus"]["answer"].is_object());
    assert_eq!(report["frames"][1]["cached"], true);
}

#[test]
fn focus_only_requests_carry_the_cast() {
    let env = Env::new();
    let cast = env.path().join("cast.json");
    std::fs::write(
        &cast,
        r#"{"version":1,"people":[{"name":"Host","description":"red jacket"}]}"#,
    )
    .unwrap();
    let cast = cast.to_str().unwrap();
    env.ok(&refs(&env.frames("local:vision-small", &["--cast", cast])));
    env.ok(&refs(&env.frames(
        "local:vision-small",
        &["--cast", cast, "--focus", "box around Host"],
    )));
    let chats = env.server.chats();
    assert_eq!(chats.len(), 2);
    let focus_only = prompt(&chats[1]);
    assert!(focus_only.contains("- Host: red jacket"), "{focus_only}");
    assert!(!focus_only.contains("shot_size"));
    assert!(!focus_only.contains("cast_matches"));
}

#[test]
fn an_unreachable_provider_fails_once_even_with_a_schema() {
    let env = Env::new();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    env.ok(&[
        "providers",
        "add",
        "gone",
        "--base-url",
        &format!("http://127.0.0.1:{port}/v1"),
    ]);
    let schema = env.schema();
    let error = env.fails(&refs(&env.frames(
        "gone:vision",
        &[
            "--focus",
            "faces",
            "--focus-schema",
            schema.to_str().unwrap(),
        ],
    )));
    assert!(error.contains("unreachable"), "{error}");
    assert_eq!(error.matches("unreachable").count(), 1, "{error}");
}

#[test]
fn a_focus_answer_that_cannot_be_cached_is_returned_and_counted_once() {
    let env = Env::new();
    let video = env.video();
    let mut plain = env.frames("local:vision-small", &[]);
    plain[4] = "15".into();
    env.ok(&refs(&plain));
    // Occupy frame 0's focus entry with a directory: its cache write fails.
    let sha = skywrite::media::file_sha256(Path::new(&video)).unwrap();
    let local = skywrite::providers::resolve(
        "local:vision-small",
        &std::collections::BTreeMap::from([(
            "local".to_owned(),
            skywrite::providers::ProviderConfig {
                base_url: env.server.base(),
                key_env: None,
                headers: Default::default(),
                timeout_seconds: None,
            },
        )]),
    )
    .unwrap();
    let focus = skywrite::describe::focus::load(&["is anyone smiling?".into()], None, None)
        .unwrap()
        .unwrap();
    let general = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &local, None).unwrap();
    let blocked = env.path().join("cache/focus-answers").join(format!(
        "{}.json",
        skywrite::describe::focus_cache_key(&general, &focus)
    ));
    std::fs::create_dir_all(&blocked).unwrap();
    let mut args = env.frames("local:vision-small", &["--focus", "is anyone smiling?"]);
    args[4] = "15".into();
    let report = env.ok(&refs(&args));
    assert_eq!(
        report["counts"],
        json!({"frames":2,"cached":2,"described":0,"failed":0,
            "focus":{"cached":0,"answered":2,"failed":0}})
    );
    let first = &report["frames"][0];
    assert!(first["focus"]["answer"].is_object());
    assert!(first["focus"]["cache_error"].is_string(), "{first}");
    assert!(first.get("error").is_none());
    assert!(report["frames"][1]["focus"].get("cache_error").is_none());

    // The same for describe video: the answer is returned with its spend.
    let encode = "h264-crf28-upright-h90-noaudio";
    let vlm = skywrite::providers::resolve(
        "local:vlm",
        &std::collections::BTreeMap::from([(
            "local".to_owned(),
            skywrite::providers::ProviderConfig {
                base_url: env.server.base(),
                key_env: None,
                headers: Default::default(),
                timeout_seconds: None,
            },
        )]),
    )
    .unwrap();
    let key = skywrite::describe::video_cache_key(&sha, 0.0, 1.0, encode, &vlm, None).unwrap();
    std::fs::create_dir_all(env.path().join("cache/focus-answers").join(format!(
        "{}.json",
        skywrite::describe::focus_cache_key(&key, &focus)
    )))
    .unwrap();
    let clip = env.ok(&[
        "describe",
        "video",
        &video,
        "--duration",
        "1",
        "--model",
        "local:vlm",
        "--focus",
        "is anyone smiling?",
    ]);
    assert!(clip["focus"]["cache_error"].is_string(), "{clip}");
    assert_eq!(clip["requests"], 1);
    assert_eq!(clip["uncosted_requests"], 1);
}

#[test]
fn a_description_that_cannot_be_cached_keeps_the_focus_answer() {
    let env = Env::new();
    let video = env.video();
    let sha = skywrite::media::file_sha256(Path::new(&video)).unwrap();
    let configured = std::collections::BTreeMap::from([(
        "local".to_owned(),
        skywrite::providers::ProviderConfig {
            base_url: env.server.base(),
            key_env: None,
            headers: Default::default(),
            timeout_seconds: None,
        },
    )]);
    let vlm = skywrite::providers::resolve("local:vlm", &configured).unwrap();
    // Occupy the general entries' paths with directories.
    let frame_key = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &vlm, None).unwrap();
    std::fs::create_dir_all(
        env.path()
            .join("cache/frame-descriptions")
            .join(format!("{frame_key}.json")),
    )
    .unwrap();
    let video_key = skywrite::describe::video_cache_key(
        &sha,
        0.0,
        1.0,
        "h264-crf28-upright-h90-noaudio",
        &vlm,
        None,
    )
    .unwrap();
    std::fs::create_dir_all(
        env.path()
            .join("cache/video-descriptions")
            .join(format!("{video_key}.json")),
    )
    .unwrap();

    let report = env.ok(&refs(
        &env.frames("local:vlm", &["--focus", "is anyone smiling?"]),
    ));
    let frame = &report["frames"][0];
    assert_eq!(frame["description"]["summary"], "a test frame");
    assert!(frame["cache_error"].is_string(), "{frame}");
    assert_eq!(frame["focus"]["attempts"], 1);
    assert!(frame["focus"].get("cache_error").is_none());
    assert_eq!(report["counts"]["described"], 1);
    // The focus answer was cached; the description was not.
    let dry = env.ok(&[
        refs(&env.frames("local:vlm", &["--focus", "is anyone smiling?"])),
        vec!["--dry-run"],
    ]
    .concat());
    assert_eq!(dry["focus"]["cached"], 1);
    assert_eq!(dry["to_describe"], 1);

    let clip = env.ok(&[
        "describe",
        "video",
        &video,
        "--duration",
        "1",
        "--model",
        "local:vlm",
        "--focus",
        "is anyone smiling?",
    ]);
    assert!(clip["cache_error"].is_string(), "{clip}");
    assert!(clip["focus"]["answer"].is_object());
    assert!(clip["focus"].get("cache_error").is_none());
}

#[test]
fn fatal_errors_say_what_was_actually_cached() {
    let env = Env::new();
    let video = env.video();
    let sha = skywrite::media::file_sha256(Path::new(&video)).unwrap();
    let configured = std::collections::BTreeMap::from([(
        "local".to_owned(),
        skywrite::providers::ProviderConfig {
            base_url: env.server.base(),
            key_env: None,
            headers: Default::default(),
            timeout_seconds: None,
        },
    )]);
    let target = skywrite::providers::resolve("local:combined-garbage", &configured).unwrap();
    let focus = skywrite::describe::focus::load(&["is anyone smiling?".into()], None, None)
        .unwrap()
        .unwrap();
    let key = skywrite::describe::video_cache_key(
        &sha,
        0.0,
        1.0,
        "h264-crf28-upright-h90-noaudio",
        &target,
        None,
    )
    .unwrap();
    std::fs::create_dir_all(env.path().join("cache/focus-answers").join(format!(
        "{}.json",
        skywrite::describe::focus_cache_key(&key, &focus)
    )))
    .unwrap();
    // No description (not JSON) and a valid focus answer whose cache path is blocked.
    let error = env.fails(&[
        "describe",
        "video",
        &video,
        "--duration",
        "1",
        "--model",
        "local:combined-garbage",
        "--focus",
        "is anyone smiling?",
    ]);
    assert!(error.contains("the description was not cached"), "{error}");
    assert!(error.contains("could not be cached either"), "{error}");
    assert!(!error.contains(", the focus answer was"), "{error}");
}

/// The cache paths a frames run at 0.0 s and a 1 s video range would use, for `model`.
fn blocked_paths(
    env: &Env,
    model: &str,
    focus_text: Option<&str>,
) -> (PathBuf, PathBuf, Option<(PathBuf, PathBuf)>) {
    let sha = skywrite::media::file_sha256(Path::new(&env.video())).unwrap();
    let configured = std::collections::BTreeMap::from([(
        "local".to_owned(),
        skywrite::providers::ProviderConfig {
            base_url: env.server.base(),
            key_env: None,
            headers: Default::default(),
            timeout_seconds: None,
        },
    )]);
    let target = skywrite::providers::resolve(model, &configured).unwrap();
    let frame = skywrite::describe::frame_cache_key(&sha, 0.0, 768, &target, None).unwrap();
    let clip = skywrite::describe::video_cache_key(
        &sha,
        0.0,
        1.0,
        "h264-crf28-upright-h90-noaudio",
        &target,
        None,
    )
    .unwrap();
    let cache = env.path().join("cache");
    let focus = focus_text.map(|text| {
        let focus = skywrite::describe::focus::load(&[text.into()], None, None)
            .unwrap()
            .unwrap();
        let path = |key: &str| {
            cache.join("focus-answers").join(format!(
                "{}.json",
                skywrite::describe::focus_cache_key(key, &focus)
            ))
        };
        (path(&frame), path(&clip))
    });
    (
        cache
            .join("frame-descriptions")
            .join(format!("{frame}.json")),
        cache
            .join("video-descriptions")
            .join(format!("{clip}.json")),
        focus,
    )
}

#[test]
fn frames_fatal_errors_name_focus_cache_failures() {
    let env = Env::new();
    let (_, _, focus) = blocked_paths(&env, "local:combined-garbage", Some("is anyone smiling?"));
    std::fs::create_dir_all(focus.unwrap().0).unwrap();
    let error = env.fails(&refs(
        &env.frames("local:combined-garbage", &["--focus", "is anyone smiling?"]),
    ));
    assert!(error.contains("every frame description failed"), "{error}");
    assert!(
        error.contains("a valid focus answer could not be cached"),
        "{error}"
    );
    assert!(
        !error.contains("usable focus answers were cached"),
        "{error}"
    );
}

#[test]
fn exhausted_focus_retries_after_a_failed_description_write_say_so() {
    let env = Env::new();
    let schema = env.schema();
    let faces = [
        "--focus",
        "bounding box of every face",
        "--focus-schema",
        schema.to_str().unwrap(),
    ];
    let (frame, clip, _) = blocked_paths(&env, "local:focus-never", None);
    std::fs::create_dir_all(&frame).unwrap();
    std::fs::create_dir_all(&clip).unwrap();
    let error = env.fails(&refs(&env.frames("local:focus-never", &faces)));
    assert!(error.contains("every focus answer failed"), "{error}");
    assert!(
        error.contains("some general descriptions could not be cached"),
        "{error}"
    );
    let video = env.video();
    let error = env.fails(
        &[
            &[
                "describe",
                "video",
                &video,
                "--duration",
                "1",
                "--model",
                "local:focus-never",
            ][..],
            &faces[..],
        ]
        .concat(),
    );
    assert!(error.contains("the focus answer failed"), "{error}");
    assert!(
        error.contains("the description could not be cached"),
        "{error}"
    );
}
