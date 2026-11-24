//! Named providers against an owned-loopback OpenAI-compatible server
//! (`tests/fixtures/network/openai_server.py`): image input, multipart
//! transcription, errors, deadlines, keys and OpenRouter-only fields.
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
    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
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
}
impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skywrite"));
        command
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("SKY_TEST_PROVIDER_KEY")
            .env_remove("SKYWRITE_FRAME_DESCRIPTION_MODEL")
            .env_remove("SKYWRITE_TRANSCRIPTION_MODEL")
            .env("SKYWRITE_HOME", self.path().join("home"))
            .env("SKYWRITE_CACHE_DIR", self.path().join("cache"))
            .args(args);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
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
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=s=160x90:r=30:d=1",
                ])
                .args(["-pix_fmt", "yuv420p"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        path.to_str().unwrap().to_owned()
    }
}

#[test]
fn provider_config_lifecycle_and_reference_rules() {
    let env = Env::new();
    let added = env.ok(&[
        "providers",
        "add",
        "local",
        "--base-url",
        "http://127.0.0.1:8080/v1/",
        "--header",
        "X-Org=team",
        "--timeout",
        "900",
    ]);
    assert_eq!(added["added"]["base_url"], "http://127.0.0.1:8080/v1");
    assert_eq!(added["added"]["timeout_seconds"], 900);
    assert_eq!(added["added"]["key_env"], Value::Null);
    assert_eq!(added["added"]["header_names"], json!(["X-Org"]));
    assert!(
        env.fails(&[
            "providers",
            "add",
            "local",
            "--base-url",
            "http://127.0.0.1:1/v1"
        ])
        .contains("already exists")
    );
    // Credentials never go into config: not as a key, a header or a URL.
    for args in [
        &["--key-env", "sk-or-v1-secret"][..],
        &["--header", "Authorization=Bearer secret"][..],
    ] {
        let mut all = vec![
            "providers",
            "add",
            "bad",
            "--base-url",
            "https://h.example/v1",
        ];
        all.extend_from_slice(args);
        env.fails(&all);
    }
    env.fails(&[
        "providers",
        "add",
        "bad",
        "--base-url",
        "https://u:p@h.example/v1",
    ]);
    env.fails(&[
        "providers",
        "add",
        "free",
        "--base-url",
        "https://h.example/v1",
    ]);
    env.fails(&[
        "providers",
        "add",
        "openrouter",
        "--base-url",
        "https://h.example/v1",
    ]);
    let out = env.run(&[
        "providers",
        "add",
        "remote",
        "--base-url",
        "http://h.example/v1",
        "--key-env",
        "SKY_TEST_PROVIDER_KEY",
    ]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("plain http://"));
    let config = std::fs::read_to_string(env.path().join("home/config.json")).unwrap();
    assert!(config.contains("\"key_env\": \"SKY_TEST_PROVIDER_KEY\""));

    let listed = env.ok(&["providers", "list"]);
    let names: Vec<_> = listed["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["openrouter", "ondevice", "local", "remote"]);
    assert_eq!(listed["providers"][0]["builtin"], true);
    assert_eq!(listed["providers"][0]["key_env"], "OPENROUTER_API_KEY");

    // Saved defaults: provider must exist and serve the task.
    let saved = env.ok(&["defaults", "frame-description", "set", "local:qwen3:0.8b"]);
    assert_eq!(saved["model"], "local:qwen3:0.8b");
    let canonical = env.ok(&[
        "defaults",
        "analysis",
        "set",
        "openrouter:vendor/model:free",
    ]);
    assert_eq!(canonical["model"], "vendor/model:free");
    assert!(
        env.fails(&["defaults", "image", "set", "local:flux"])
            .contains("only provider openrouter")
    );
    assert!(
        env.fails(&["defaults", "analysis", "set", "qwen3:0.8b"])
            .contains("unknown provider")
    );
    assert!(
        env.fails(&["providers", "remove", "local"])
            .contains("frame-description")
    );
    assert!(
        env.fails(&["providers", "remove", "openrouter"])
            .contains("cannot be removed")
    );
    env.ok(&["defaults", "frame-description", "unset"]);
    env.ok(&["providers", "remove", "local"]);
    assert!(
        env.fails(&["providers", "show", "local"])
            .contains("no provider named local")
    );
    // Generation commands refuse custom providers before any work.
    let error = env.fails(&[
        "generate",
        "image",
        "--prompt",
        "x",
        "--model",
        "remote:flux",
        "-o",
        "x.png",
    ]);
    assert!(error.contains("only provider openrouter"), "{error}");
}

#[test]
fn frames_through_a_local_provider_send_standard_fields_and_cache_per_provider() {
    let env = Env::new();
    let server = Server::start(env.path());
    let base = server.base();
    env.ok(&[
        "providers",
        "add",
        "local",
        "--base-url",
        &base,
        "--header",
        "X-Org=team",
    ]);
    let video = env.video();
    let report = env.ok(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "local:vision-small",
    ]);
    assert_eq!(report["provider"], "local");
    assert_eq!(report["model"], "local:vision-small");
    assert_eq!(report["data_policy"], Value::Null);
    assert!(
        report["routing_note"]
            .as_str()
            .unwrap()
            .contains("not OpenRouter")
    );
    assert_eq!(report["counts"]["described"], 1);
    // The server reported no cost: unknown, never zero.
    assert_eq!(report["cost_usd"], Value::Null);
    assert_eq!(report["uncosted_requests"], 1);
    // A leading thinking block is not part of the answer.
    assert_eq!(
        report["frames"][0]["description"]["summary"],
        "a test frame"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request["path"], "/v1/chat/completions");
    assert!(request["headers"].get("authorization").is_none());
    assert!(request["headers"].get("x-title").is_none());
    assert_eq!(request["headers"]["x-org"], "team");
    let body = request["body"].as_object().unwrap();
    assert_eq!(body["model"], "vision-small");
    for field in ["reasoning", "provider", "usage"] {
        assert!(!body.contains_key(field), "sent OpenRouter-only {field}");
    }
    assert_eq!(
        body["messages"][0]["content"][1]["type"], "image_url",
        "{body:?}"
    );
    // Same pass again: served from the cache, no request.
    let again = env.ok(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "local:vision-small",
    ]);
    assert_eq!(again["counts"]["cached"], 1);
    assert_eq!(server.requests().len(), 1);
    // The bare ID is OpenRouter's: no shared entry, so it needs a key.
    assert!(
        env.fails(&[
            "describe",
            "frames",
            &video,
            "--every",
            "30",
            "--model",
            "vision-small"
        ])
        .contains("OPENROUTER_API_KEY")
    );
    // A second provider at the same URL does not share entries either.
    env.ok(&["providers", "add", "twin", "--base-url", &base]);
    let twin = env.ok(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "twin:vision-small",
    ]);
    assert_eq!(twin["counts"]["described"], 1);
    assert_eq!(server.requests().len(), 2);

    // A completed request whose answer is not JSON is still counted as spent.
    let mixed = env.ok(&[
        "describe",
        "frames",
        &video,
        "--every",
        "15",
        "--concurrency",
        "1",
        "--model",
        "local:half-bad",
    ]);
    assert_eq!(mixed["counts"]["failed"], 1);
    assert_eq!(mixed["counts"]["described"], 1);
    assert_eq!(mixed["cost_usd"], 0.5);
    assert_eq!(mixed["uncosted_requests"], 0);

    // A model without image input fails with that explanation.
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "local:text-only",
    ]);
    assert!(
        error.contains("appears not to accept image input"),
        "{error}"
    );
}

#[test]
fn a_run_where_every_answer_fails_still_reports_what_it_cost() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&["providers", "add", "local", "--base-url", &server.base()]);
    let video = env.video();
    // One frame; the fresh server's first half-bad answer is not JSON, at $0.25.
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "local:half-bad",
    ]);
    assert!(error.contains("every frame description failed"), "{error}");
    assert!(error.contains("cost $0.25"), "{error}");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_server_that_goes_away_mid_run_reports_what_completed_requests_cost() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&["providers", "add", "local", "--base-url", &server.base()]);
    let video = env.video();
    // Two frames, one at a time: the first is answered (and billed), then the server exits.
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--every",
        "15",
        "--concurrency",
        "1",
        "--model",
        "local:last-answer",
    ]);
    assert!(error.contains("unreachable"), "{error}");
    assert!(error.contains("cost $0.25"), "{error}");
    assert!(
        error.contains("a rerun requests only frames without a cached description"),
        "{error}"
    );
    drop(server);
}

#[test]
fn defaults_inspection_applies_provider_and_task_checks() {
    let env = Env::new();
    env.ok(&[
        "providers",
        "add",
        "local",
        "--base-url",
        "http://127.0.0.1:9/v1",
    ]);
    let inspect = |variable: &str, value: &str, task: &str| -> Value {
        let out = env
            .command(&["defaults", task])
            .env(variable, value)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let generation = inspect("SKYWRITE_IMAGE_GENERATION_MODEL", "local:flux", "image");
    assert_eq!(generation["source"], "invalid_override");
    assert_eq!(generation["model"], Value::Null);
    assert!(
        generation["selection_error"]
            .as_str()
            .unwrap()
            .contains("only provider openrouter")
    );
    let unknown = inspect("SKYWRITE_ANALYSIS_MODEL", "nope:model", "analysis");
    assert_eq!(unknown["source"], "invalid_override");
    assert!(
        unknown["selection_error"]
            .as_str()
            .unwrap()
            .contains("unknown provider")
    );
    let valid = inspect(
        "SKYWRITE_FRAME_DESCRIPTION_MODEL",
        "local:qwen3:0.8b",
        "frame-description",
    );
    assert_eq!(valid["source"], "environment");
    assert_eq!(valid["provider"], "local");
    // A saved preference whose provider was removed by hand.
    let path = env.path().join("home/config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["model_defaults"] = json!({"analysis": "gone:model"});
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let saved = env.ok(&["defaults", "analysis"]);
    assert_eq!(saved["source"], "invalid_saved");
    assert_eq!(saved["model"], Value::Null);
    assert_eq!(saved["saved_model"], "gone:model");
    // A malformed saved reference is the saved preference's fault too.
    config["model_defaults"] = json!({"analysis": "local:"});
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let malformed = env.ok(&["defaults", "analysis"]);
    assert_eq!(malformed["source"], "invalid_saved");
    assert!(
        malformed["selection_error"]
            .as_str()
            .unwrap()
            .contains("no model")
    );
}

#[test]
fn keys_come_only_from_the_named_variable_and_never_from_openrouter() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&[
        "providers",
        "add",
        "keyed",
        "--base-url",
        &server.base(),
        "--key-env",
        "SKY_TEST_PROVIDER_KEY",
    ]);
    env.ok(&["providers", "add", "open", "--base-url", &server.base()]);
    let video = env.video();
    // Named but unset: refused before any request.
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "keyed:vision-small",
    ]);
    assert!(error.contains("SKY_TEST_PROVIDER_KEY"), "{error}");
    let error = env.fails(&["models", "list", "--provider", "keyed"]);
    assert!(error.contains("SKY_TEST_PROVIDER_KEY"), "{error}");
    assert!(server.requests().is_empty());
    // The OpenRouter key's variable can never be a custom provider's key.
    let error = env.fails(&[
        "providers",
        "add",
        "leak",
        "--base-url",
        &server.base(),
        "--key-env",
        "OPENROUTER_API_KEY",
    ]);
    assert!(error.contains("never sent elsewhere"), "{error}");
    let out = env
        .command(&[
            "describe",
            "frames",
            &video,
            "--every",
            "30",
            "--model",
            "keyed:vision-small",
        ])
        .env("SKY_TEST_PROVIDER_KEY", "provider-test-key")
        .env("OPENROUTER_API_KEY", "openrouter-test-key")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = env
        .command(&[
            "describe",
            "frames",
            &video,
            "--every",
            "30",
            "--model",
            "open:vision-small",
        ])
        .env("OPENROUTER_API_KEY", "openrouter-test-key")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["headers"]["authorization"],
        "Bearer provider-test-key"
    );
    assert!(requests[1]["headers"].get("authorization").is_none());
    assert!(
        !std::fs::read_to_string(&server.log)
            .unwrap()
            .contains("openrouter-test-key")
    );
    // Doctor does not probe a provider whose key variable is unset.
    let doctor = env.ok(&["doctor"]);
    let find = |name: &str| {
        doctor["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(find("keyed")["reachable"], Value::Null);
    assert!(
        find("keyed")["error"]
            .as_str()
            .unwrap()
            .contains("not checked")
    );
    assert_eq!(find("open")["reachable"], true);
    let probes: Vec<_> = server.requests().into_iter().skip(2).collect();
    assert_eq!(probes.len(), 1);
    assert!(probes[0]["headers"].get("authorization").is_none());
}

#[test]
fn transcription_uses_the_standard_multipart_form_and_keeps_requiring_word_times() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&["providers", "add", "asr", "--base-url", &server.base()]);
    let audio = env.path().join("tone.wav");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=2",
            "-ar",
            "16000",
        ])
        .arg(&audio)
        .status()
        .unwrap();
    assert!(status.success());
    let audio = audio.to_str().unwrap();
    let transcript = env.ok(&[
        "transcribe",
        audio,
        "--model",
        "asr:whisper-1",
        "--language",
        "en",
    ]);
    assert_eq!(transcript["model"], "asr:whisper-1");
    assert_eq!(transcript["words"][0]["text"], "hello");
    let requests = server.requests();
    let request = &requests[0];
    assert_eq!(request["path"], "/v1/audio/transcriptions");
    assert!(
        request["content_type"]
            .as_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert_eq!(request["fields"]["model"], json!(["whisper-1"]));
    assert_eq!(
        request["fields"]["response_format"],
        json!(["verbose_json"])
    );
    assert_eq!(
        request["fields"]["timestamp_granularities[]"],
        json!(["word"])
    );
    assert_eq!(request["fields"]["language"], json!(["en"]));
    assert_eq!(request["files"]["file"]["riff"], true);
    assert!(
        request["fields"].get("provider").is_none() && request["fields"].get("usage").is_none()
    );
    // Text without word timestamps is rejected, never estimated.
    let error = env.fails(&["transcribe", audio, "--model", "asr:text-only-asr"]);
    assert!(error.contains("word timestamps"), "{error}");
}

#[test]
fn unreachable_and_slow_servers_fail_once_with_a_clear_error() {
    let env = Env::new();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    env.ok(&[
        "providers",
        "add",
        "down",
        "--base-url",
        &format!("http://127.0.0.1:{port}/v1"),
    ]);
    let video = env.video();
    let started = std::time::Instant::now();
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--interval",
        "0.1",
        "--model",
        "down:m",
    ]);
    assert!(
        error.contains("unreachable") && error.contains("does not retry"),
        "{error}"
    );
    assert_eq!(error.matches("unreachable").count(), 1, "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));

    let server = Server::start(env.path());
    env.ok(&[
        "providers",
        "add",
        "slow",
        "--base-url",
        &server.base(),
        "--timeout",
        "1",
    ]);
    let started = std::time::Instant::now();
    let error = env.fails(&[
        "describe",
        "frames",
        &video,
        "--every",
        "30",
        "--model",
        "slow:slow",
    ]);
    assert!(
        error.contains("timed out") && error.contains("--timeout"),
        "{error}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(4));

    // Doctor reports both without generating anything.
    let report = env.ok(&["doctor"]);
    let providers = report["providers"].as_array().unwrap();
    let find = |name: &str| providers.iter().find(|p| p["name"] == name).unwrap();
    assert_eq!(find("openrouter")["reachable"], Value::Null);
    assert_eq!(find("down")["reachable"], false);
    assert_eq!(find("slow")["reachable"], true);
    assert_eq!(report["network_checked"], true);
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r["method"] == "GET" || r["body"]["model"] == "slow")
    );
}

#[test]
fn video_ranges_of_rotated_phone_footage_keep_their_displayed_height() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&["providers", "add", "local", "--base-url", &server.base()]);
    let stored = env.video();
    let phone = env.path().join("phone.mp4");
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-display_rotation", "90", "-i", &stored])
        .args(["-c", "copy"])
        .arg(&phone)
        .status()
        .unwrap();
    assert!(status.success());
    // The fixture answers with a frame description, so only the request matters.
    env.run(&[
        "describe",
        "video",
        phone.to_str().unwrap(),
        "--duration",
        "1",
        "--height",
        "240",
        "--model",
        "local:video-small",
    ]);
    let requests = server.requests();
    let url = requests[0]["body"]["messages"][0]["content"][1]["video_url"]["url"]
        .as_str()
        .unwrap();
    let clip = env.path().join("sent.mp4");
    std::fs::write(
        &clip,
        base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            url.strip_prefix("data:video/mp4;base64,").unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    // Shown 90x160: a 240-row cap keeps all 160 rows instead of shrinking to 90.
    let video = skywrite::media::probe(&clip).unwrap();
    assert_eq!(video.video().unwrap().height, Some(160));
}

#[test]
fn provider_catalogs_label_unverified_capabilities() {
    let env = Env::new();
    let server = Server::start(env.path());
    env.ok(&["providers", "add", "local", "--base-url", &server.base()]);
    let listed = env.ok(&[
        "models",
        "list",
        "--provider",
        "local",
        "--task",
        "frame-description",
    ]);
    let models = listed["models"].as_array().unwrap();
    let ids: Vec<_> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["vision-small", "vlm"]);
    assert_eq!(models[0]["capabilities_verified"], false);
    assert_eq!(models[0]["reference"], "local:vision-small");
    assert_eq!(models[1]["capabilities_verified"], true);
    assert!(
        listed["warnings"][0]
            .as_str()
            .unwrap()
            .contains("unverified")
    );
    // Cached for the TTL; offline works without the server.
    let offline = env.ok(&["models", "list", "--provider", "local", "--offline"]);
    assert_eq!(offline["models"].as_array().unwrap().len(), 3);
    assert_eq!(server.requests().len(), 1);
    assert!(
        server.requests()[0]["headers"]
            .get("authorization")
            .is_none()
    );
    assert!(
        env.fails(&["models", "list", "--provider", "local", "--task", "image"])
            .contains("only provider openrouter")
    );
}
