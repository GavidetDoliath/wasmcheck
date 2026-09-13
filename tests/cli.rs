use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join("wasmcheck_it")
            .join(format!("{name}_{n}_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_wasm(&self, filename: &str, size: usize) {
        self.write_wasm_in(self.path(), filename, size);
    }

    fn write_wasm_in(&self, dir: &Path, filename: &str, size: usize) {
        let path = dir.join(filename);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, vec![b'a'; size]).unwrap();
    }

    fn read_config(&self, path: &Path) -> serde_json::Value {
        let text = fs::read_to_string(path).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wasmcheck")
}

/// Reads the baseline file sitting next to `config_path`.
fn read_baseline(config_path: &Path) -> serde_json::Value {
    let path = config_path.with_file_name(".wasmcheck.baseline.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn run_capture(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String) {
    let (code, stdout, stderr) = run_capture(dir, args);
    (code, format!("{stdout}{stderr}"))
}

/// Parses stdout as **one** JSON document, which is what `--format json`
/// promises for any number of files.
fn parse_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout is not a single JSON document ({e}): {stdout}"))
}

/// Writes a config with a generous budget but a tight growth allowance, and a
/// baseline for `app.wasm`.
fn write_max_delta_config(dir: &Path, budget: &str, max_delta: &str, baseline_raw: usize) {
    let config = serde_json::json!({
        "files": ["app.wasm"],
        "budget": { "raw": budget },
        "max_delta": { "raw": max_delta },
        "baseline": {
            "app.wasm": { "file": "app.wasm", "raw": baseline_raw, "gzip": 100, "brotli": 90 }
        }
    });
    fs::write(dir.join(".wasmcheck.json"), config.to_string()).unwrap();
}

/// Lays out `<root>/sub/dist/*.wasm` plus a config inside `sub`, and returns
/// the config path. Used by the config-relative resolution tests.
fn nested_layout(root: &Path, wasm_name: &str, size: usize) -> std::path::PathBuf {
    let sub = root.join("sub");
    let dist = sub.join("dist");
    fs::create_dir_all(&dist).unwrap();
    fs::write(dist.join(wasm_name), vec![b'a'; size]).unwrap();
    let config = sub.join(".wasmcheck.json");
    fs::write(
        &config,
        r#"{"files": ["dist/*_bg-*.wasm"], "budget": {"raw": "100 KB"}}"#,
    )
    .unwrap();
    config
}

#[test]
fn empty_dir_errors() {
    let d = TestDir::new("empty");
    let (code, msg) = run_in(d.path(), &[]);
    assert_ne!(code, 0);
    assert!(msg.contains("no .wasm file"), "{msg}");
}

#[test]
fn single_wasm_auto_detected() {
    let d = TestDir::new("single");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &[]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("app.wasm"), "{msg}");
    assert!(msg.contains("raw"), "{msg}");
    assert!(msg.contains("gzip"), "{msg}");
    assert!(msg.contains("brotli"), "{msg}");
}

#[test]
fn multiple_wasm_require_file_flag() {
    let d = TestDir::new("multi");
    d.write_wasm("app.wasm", 10 * 1024);
    d.write_wasm("worker.wasm", 100);
    let (code, msg) = run_in(d.path(), &[]);
    assert_ne!(code, 0);
    assert!(msg.contains("multiple .wasm files"), "{msg}");
}

#[test]
fn budget_fail_exit_code() {
    let d = TestDir::new("budget_fail");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["--file", "app.wasm", "--budget", "1 KB"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.to_uppercase().contains("FAIL"), "{msg}");
}

#[test]
fn budget_pass_exit_code() {
    let d = TestDir::new("budget_pass");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["--file", "app.wasm", "--budget", "100 KB"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

/// Regression: `parse_size("-1 KB")` saturated to `0` bytes, which turned an
/// obviously broken budget into a silent always-fail instead of an error.
#[test]
fn negative_budget_is_rejected() {
    let d = TestDir::new("negative_budget");
    d.write_wasm("app.wasm", 10 * 1024);
    // `--budget=-1 KB` so clap does not read the value as a flag.
    let (code, msg) = run_in(d.path(), &["--file", "app.wasm", "--budget=-1 KB"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("invalid size"), "{msg}");
    assert!(!msg.to_uppercase().contains("PASS"), "{msg}");
}

#[test]
fn json_output() {
    let d = TestDir::new("json");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, stdout, _) = run_capture(d.path(), &["--file", "app.wasm", "--format", "json"]);
    assert_eq!(code, 0);

    let v = parse_json(&stdout);
    assert_eq!(v["status"], "unchecked");
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0]["raw"].as_u64().unwrap() >= 10 * 1024);
    assert!(files[0]["gzip"].as_u64().unwrap() > 0);
    assert!(files[0]["brotli"].as_u64().unwrap() > 0);
}

/// Regression: each file used to print its own JSON object, so two files
/// produced two concatenated objects and no parser could read them.
#[test]
fn json_stays_one_document_for_several_files() {
    let d = TestDir::new("json_multi");
    d.write_wasm("app_bg-aaa.wasm", 10 * 1024);
    d.write_wasm("app_bg-bbb.wasm", 12 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["*_bg-*.wasm"], "budget": {"raw": "100 KB"}}"#,
    )
    .unwrap();

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "json"]);
    assert_eq!(code, 0);

    let v = parse_json(&stdout);
    assert_eq!(v["status"], "pass");
    let files = v["files"].as_array().unwrap();
    assert_eq!(
        files.len(),
        2,
        "both glob matches are in one document: {stdout}"
    );
    for file in files {
        assert_eq!(file["status"], "pass");
        assert_eq!(file["budget"]["raw"], "100 KB");
    }
    // Absent data is omitted rather than emitted as null.
    assert!(files[0].get("baseline").is_none());
    assert!(files[0].get("delta").is_none());
    assert!(files[0].get("exceeded").is_none());
    assert!(files[0].get("top").is_none());
}

#[test]
fn json_reports_fail_while_stdout_stays_parseable() {
    let d = TestDir::new("json_fail");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, stdout, stderr) = run_capture(
        d.path(),
        &["--file", "app.wasm", "--budget", "1 KB", "--format", "json"],
    );
    assert_eq!(code, 1);

    let v = parse_json(&stdout);
    assert_eq!(v["status"], "fail");
    assert_eq!(v["files"][0]["status"], "fail");
    assert_eq!(v["files"][0]["exceeded"], serde_json::json!(["raw"]));
    assert!(stderr.to_uppercase().contains("FAIL"), "{stderr}");
}

#[test]
fn json_includes_baseline_and_signed_delta() {
    let d = TestDir::new("json_delta");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _, _) = run_capture(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    d.write_wasm("app.wasm", 12 * 1024);
    let (code, stdout, _) = run_capture(
        d.path(),
        &["check", "--file", "app.wasm", "--format", "json"],
    );
    assert_eq!(code, 1);

    let v = parse_json(&stdout);
    let file = &v["files"][0];
    assert_eq!(file["baseline"]["raw"], 10 * 1024);
    assert_eq!(file["delta"]["raw"], 2048);
    assert_eq!(file["budget"]["raw"], "10 KB");
    assert_eq!(file["exceeded"], serde_json::json!(["raw"]));
}

#[test]
fn json_includes_top_when_requested() {
    let d = TestDir::new("json_top");
    let fixture = format!("{}/fixtures/full.wasm", env!("CARGO_MANIFEST_DIR"));
    let (code, stdout, _) = run_capture(
        d.path(),
        &[
            "check", "--file", &fixture, "--format", "json", "--top", "2",
        ],
    );
    assert_eq!(code, 0);

    let v = parse_json(&stdout);
    let top = v["files"][0]["top"].as_array().unwrap();
    assert_eq!(top.len(), 2);
    assert!(top[0]["size"].as_u64().unwrap() > 0);
    assert!(!top[0]["name"].as_str().unwrap().is_empty());
}

#[test]
fn check_subcommand_with_flags() {
    let d = TestDir::new("check_cmd");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(
        d.path(),
        &["check", "--file", "app.wasm", "--budget", "100 KB"],
    );
    assert_eq!(code, 0, "{msg}");
}

#[test]
fn init_writes_config_and_check_uses_it() {
    let d = TestDir::new("init_flow");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0, "{msg}");
    assert!(d.path().join(".wasmcheck.json").exists(), "config written");

    let cfg = d.read_config(&d.path().join(".wasmcheck.json"));
    assert!(cfg["budget"]["raw"].is_string());
    // The sizes live in their own file, so the config stays human-sized.
    assert!(cfg.get("baseline").is_none(), "{cfg}");
    assert_eq!(
        read_baseline(&d.path().join(".wasmcheck.json"))["app.wasm"]["raw"]
            .as_u64()
            .unwrap(),
        10 * 1024
    );

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

/// Regression: `init` rounded the budget to the nearest KiB, so a 10.45 KB file
/// produced a `"10 KB"` budget that failed immediately.
#[test]
fn init_budget_never_rejects_the_size_it_measured() {
    let d = TestDir::new("init_budget");
    d.write_wasm("app.wasm", 10_700);
    let (code, msg) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0, "{msg}");

    let cfg = d.read_config(&d.path().join(".wasmcheck.json"));
    let raw = cfg["budget"]["raw"].as_str().unwrap().to_string();
    let budget: u64 = raw.parse::<wasmcheck::Size>().unwrap().bytes();
    assert!(
        budget >= 10_700,
        "budget `{raw}` is below the measured 10700 bytes"
    );

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "fresh config `{raw}` fails its own check: {msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

#[test]
fn budget_from_config_enforced() {
    let d = TestDir::new("config_budget");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    d.write_wasm("app.wasm", 20 * 1024);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.to_uppercase().contains("FAIL"), "{msg}");
}

/// A bundle can be far inside its budget and still fail, because it grew more
/// than `max_delta` allows.
#[test]
fn max_delta_fails_even_when_under_budget() {
    let d = TestDir::new("max_delta_fail");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "100 KB", "1 KB", 10 * 1024);

    // +2 KB: 12 KB is nowhere near the 100 KB budget, but 2x the allowance.
    d.write_wasm("app.wasm", 12 * 1024);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 1, "growth alone should fail the run: {msg}");
    assert!(msg.to_uppercase().contains("FAIL"), "{msg}");
    assert!(msg.contains("max delta"), "{msg}");
    assert!(msg.contains("over max delta"), "{msg}");
    assert!(!msg.contains("over budget"), "{msg}");
}

#[test]
fn max_delta_within_allowance_passes() {
    let d = TestDir::new("max_delta_pass");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "100 KB", "1 KB", 10 * 1024);

    d.write_wasm("app.wasm", 10 * 1024 + 512);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

#[test]
fn max_delta_can_come_from_the_command_line() {
    let d = TestDir::new("max_delta_cli");
    d.write_wasm("app.wasm", 12 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"],
            "baseline": {"app.wasm": {"file": "app.wasm", "raw": 10240, "gzip": 100, "brotli": 90}}}"#,
    )
    .unwrap();

    // No gate configured: nothing can fail.
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");

    let (code, msg) = run_in(d.path(), &["check", "--max-delta", "1 KB"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("max delta"), "{msg}");
}

/// A percentage allowance is resolved against the baseline, per metric.
#[test]
fn max_delta_accepts_a_percentage() {
    let d = TestDir::new("percent_delta");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "100 KB", "5%", 10 * 1024);

    // 5% of 10240 is 512 bytes.
    d.write_wasm("app.wasm", 10 * 1024 + 400);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "+400 bytes is inside a 5% allowance: {msg}");

    d.write_wasm("app.wasm", 10 * 1024 + 600);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 1, "+600 bytes is past a 5% allowance: {msg}");
    assert!(msg.contains("max delta"), "{msg}");
}

#[test]
fn max_delta_percentage_can_come_from_the_command_line() {
    let d = TestDir::new("percent_cli");
    d.write_wasm("app.wasm", 12 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"],
            "baseline": {"app.wasm": {"file": "app.wasm", "raw": 10240, "gzip": 100, "brotli": 90}}}"#,
    )
    .unwrap();

    // +2 KB is +20% of 10 KB.
    let (code, msg) = run_in(d.path(), &["check", "--max-delta", "25%"]);
    assert_eq!(code, 0, "{msg}");

    let (code, msg) = run_in(d.path(), &["check", "--max-delta", "5%"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("max delta"), "{msg}");
}

/// A percentage cap has no meaning without a baseline, so `budget` refuses it
/// and `max_delta` is where percentages belong.
#[test]
fn budget_flag_rejects_a_percentage() {
    let d = TestDir::new("percent_budget");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["--file", "app.wasm", "--budget", "5%"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("invalid size"), "{msg}");
    assert!(
        msg.contains("max_delta"),
        "the hint points at max_delta: {msg}"
    );
}

/// Strict mode turns a missing baseline from a silent no-op into a failure.
#[test]
fn strict_fails_on_a_missing_baseline() {
    let d = TestDir::new("strict_missing");
    d.write_wasm("app_bg-aaa.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["*_bg-*.wasm"], "budget": {"raw": "100 KB"}, "strict": true}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("no baseline recorded"), "{msg}");
}

#[test]
fn strict_flag_needs_the_baseline_but_is_off_by_default() {
    let d = TestDir::new("strict_flag");
    d.write_wasm("app.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "budget": {"raw": "100 KB"}}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");

    let (code, msg) = run_in(d.path(), &["check", "--strict"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("no baseline recorded"), "{msg}");
}

#[test]
fn strict_passes_once_a_baseline_exists() {
    let d = TestDir::new("strict_ok");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    let (code, msg) = run_in(d.path(), &["check", "--strict"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

#[test]
fn json_reports_a_missing_baseline_under_strict() {
    let d = TestDir::new("strict_json");
    d.write_wasm("app.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "strict": true}"#,
    )
    .unwrap();

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "json"]);
    assert_eq!(code, 1);
    let v = parse_json(&stdout);
    assert_eq!(v["status"], "fail");
    assert_eq!(v["files"][0]["baseline_missing"], true);
    assert_eq!(v["files"][0]["status"], "fail");
}

#[test]
fn github_format_emits_one_annotation_per_failure() {
    let d = TestDir::new("github_fail");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "1 KB", "1 KB", 10 * 1024);
    d.write_wasm("app.wasm", 12 * 1024);

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "github"]);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("::error file=app.wasm,title=wasmcheck::"),
        "{stdout}"
    );
    assert!(stdout.contains("raw over budget"), "{stdout}");
    assert!(stdout.contains("raw grew past max delta"), "{stdout}");
    // No human table on stdout: workflow commands only.
    assert!(!stdout.contains("file:"), "{stdout}");
}

#[test]
fn github_format_is_silent_when_nothing_fails() {
    let d = TestDir::new("github_pass");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "100 KB", "1 KB", 10 * 1024);

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "github"]);
    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

#[test]
fn github_format_annotates_a_missing_baseline() {
    let d = TestDir::new("github_strict");
    d.write_wasm("app.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "strict": true}"#,
    )
    .unwrap();

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "github"]);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("::error file=app.wasm,title=wasmcheck::no baseline recorded"),
        "{stdout}"
    );
}

/// Without a baseline there is nothing to compare against, so the run is not
/// silently failed — but it says so.
#[test]
fn max_delta_without_baseline_is_not_evaluated() {
    let d = TestDir::new("max_delta_no_baseline");
    d.write_wasm("app.wasm", 100 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "max_delta": {"raw": "1 KB"}}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("no baseline"), "{msg}");
}

#[test]
fn json_distinguishes_budget_from_delta_failures() {
    let d = TestDir::new("json_delta_gate");
    d.write_wasm("app.wasm", 10 * 1024);
    write_max_delta_config(d.path(), "100 KB", "1 KB", 10 * 1024);
    d.write_wasm("app.wasm", 12 * 1024);

    let (code, stdout, _) = run_capture(d.path(), &["check", "--format", "json"]);
    assert_eq!(code, 1);

    let v = parse_json(&stdout);
    assert_eq!(v["status"], "fail");
    let file = &v["files"][0];
    assert_eq!(file["status"], "fail");
    assert_eq!(file["delta_exceeded"], serde_json::json!(["raw"]));
    assert!(
        file.get("exceeded").is_none(),
        "the budget was fine: {file}"
    );
    assert_eq!(file["max_delta"]["raw"], "1 KB");
    assert_eq!(file["delta"]["raw"], 2048);
}

#[test]
fn glob_files_in_config() {
    let d = TestDir::new("glob");
    d.write_wasm("app_bg-abc1234.wasm", 10 * 1024);
    d.write_wasm("other.txt", 50);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["*_bg-*.wasm"], "budget": {"raw": "20 KB"}}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("app_bg-abc1234.wasm"), "{msg}");
}

#[test]
fn glob_no_match_errors() {
    let d = TestDir::new("glob_nomatch");
    d.write_wasm("app.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["nope-*.wasm"]}"#,
    )
    .unwrap();
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_ne!(code, 0);
    assert!(msg.contains("no .wasm file"), "{msg}");
}

#[test]
fn delta_reported_in_output() {
    let d = TestDir::new("delta");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    let (code, msg) = run_in(d.path(), &["check", "--file", "app.wasm"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("±0"), "delta shown: {msg}");
}

/// Regression: `baseline` ignored the config's `files` and auto-detected in the
/// working directory, so a glob pointing at `sub/dist/` was never refreshed.
#[test]
fn baseline_refreshes_globbed_config_entries() {
    let d = TestDir::new("baseline_config");
    let config = nested_layout(d.path(), "app_bg-aaa.wasm", 10 * 1024);

    let (code, msg) = run_in(d.path(), &["baseline", "--config", "sub/.wasmcheck.json"]);
    assert_eq!(code, 0, "{msg}");

    let baseline = read_baseline(&config);
    assert_eq!(
        baseline["dist/*_bg-*.wasm"]["raw"].as_u64().unwrap(),
        10 * 1024,
        "baseline is keyed by the glob, not the hashed filename: {baseline}"
    );
}

/// Regression: `check --config dogfood-app/.wasmcheck.json` run from the repo
/// root resolved globs against the working directory instead of the config's.
#[test]
fn config_globs_resolve_from_the_config_directory() {
    let d = TestDir::new("config_dir");
    nested_layout(d.path(), "app_bg-aaa.wasm", 10 * 1024);

    let (code, msg) = run_in(d.path(), &["check", "--config", "sub/.wasmcheck.json"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("app_bg-aaa.wasm"), "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

/// Regression: baselines were keyed by exact path, so a rebuild that changed
/// the content hash lost the delta entirely.
#[test]
fn baseline_survives_a_new_content_hash() {
    let d = TestDir::new("hash_change");
    d.write_wasm("app_bg-aaa.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["*_bg-*.wasm"], "budget": {"raw": "100 KB"}}"#,
    )
    .unwrap();
    let (code, msg) = run_in(d.path(), &["baseline"]);
    assert_eq!(code, 0, "{msg}");

    // A rebuild produces the same bundle under a new content hash.
    fs::remove_file(d.path().join("app_bg-aaa.wasm")).unwrap();
    d.write_wasm("app_bg-bbb.wasm", 12 * 1024);

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(
        msg.contains("+2.00 KB"),
        "delta against the previous build is missing: {msg}"
    );
}

/// The point of the separate file: a rebuild rewrites only the baseline, so the
/// hand-edited config never conflicts in a pull request.
#[test]
fn a_rebuild_only_rewrites_the_baseline_file() {
    let d = TestDir::new("baseline_separate");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0, "{msg}");

    let config = d.path().join(".wasmcheck.json");
    let baseline_file = d.path().join(".wasmcheck.baseline.json");
    assert!(
        baseline_file.is_file(),
        "baseline written next to the config"
    );

    let config_before = fs::read_to_string(&config).unwrap();
    d.write_wasm("app.wasm", 12 * 1024);
    let (code, msg) = run_in(d.path(), &["baseline"]);
    assert_eq!(code, 0, "{msg}");

    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        config_before,
        "the config is untouched by a rebuild"
    );
    assert_eq!(
        read_baseline(&config)["app.wasm"]["raw"].as_u64().unwrap(),
        12 * 1024
    );
}

#[test]
fn config_can_point_at_another_baseline_file() {
    let d = TestDir::new("baseline_custom");
    let sub = d.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("app.wasm"), vec![b'a'; 4096]).unwrap();
    fs::write(
        sub.join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "budget": {"raw": "100 KB"},
            "baseline_file": "ci/baseline.json"}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["baseline", "--config", "sub/.wasmcheck.json"]);
    assert_eq!(code, 0, "{msg}");
    assert!(
        sub.join("ci/baseline.json").is_file(),
        "written where asked"
    );
    assert!(!sub.join(".wasmcheck.baseline.json").exists());

    let (code, msg) = run_in(d.path(), &["check", "--config", "sub/.wasmcheck.json"]);
    assert_eq!(code, 0, "{msg}");
    assert!(
        msg.contains("±0"),
        "read back through the custom path: {msg}"
    );
}

/// Configs written before the baseline moved out still work, and the first
/// write command migrates them.
#[test]
fn legacy_inline_baseline_is_adopted_then_migrated() {
    let d = TestDir::new("baseline_legacy");
    d.write_wasm("app.wasm", 10 * 1024);
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files": ["app.wasm"], "budget": {"raw": "100 KB"},
            "baseline": {"app.wasm": {"file": "app.wasm", "raw": 10240, "gzip": 100, "brotli": 90}}}"#,
    )
    .unwrap();

    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("±0"), "inline baseline is honoured: {msg}");

    let (code, msg) = run_in(d.path(), &["baseline"]);
    assert_eq!(code, 0, "{msg}");

    let config = d.path().join(".wasmcheck.json");
    let text = fs::read_to_string(&config).unwrap();
    assert!(
        !text.contains("\"baseline\""),
        "migrated out of the config: {text}"
    );
    assert_eq!(
        read_baseline(&config)["app.wasm"]["raw"].as_u64().unwrap(),
        10 * 1024
    );
}

#[test]
fn baseline_file_flag_overrides_where_it_reads_and_writes() {
    let d = TestDir::new("baseline_flag");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(
        d.path(),
        &[
            "init",
            "--file",
            "app.wasm",
            "--baseline-file",
            "custom/baseline.json",
        ],
    );
    assert_eq!(code, 0, "{msg}");
    assert!(d.path().join("custom/baseline.json").is_file());
    assert!(!d.path().join(".wasmcheck.baseline.json").exists());

    let (code, msg) = run_in(
        d.path(),
        &["check", "--baseline-file", "custom/baseline.json"],
    );
    assert_eq!(code, 0, "{msg}");
    assert!(msg.contains("±0"), "{msg}");
}

#[test]
fn explicit_missing_baseline_file_is_an_error() {
    let d = TestDir::new("baseline_missing_flag");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(
        d.path(),
        &[
            "check",
            "--file",
            "app.wasm",
            "--budget",
            "100 KB",
            "--baseline-file",
            "nope.json",
        ],
    );
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("baseline not found"), "{msg}");
}

#[test]
fn baseline_without_config_errors() {
    let d = TestDir::new("baseline_missing_config");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["baseline"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("config not found"), "{msg}");
}

/// Regression: a bad path used to panic with an unwrap instead of reporting an
/// error (`101` is the Rust panic exit code).
#[test]
fn unusable_path_errors_instead_of_panicking() {
    let d = TestDir::new("dir_as_file");
    fs::create_dir_all(d.path().join("subdir.zzz")).unwrap();
    let (code, msg) = run_in(d.path(), &["--file", "subdir.zzz", "--budget", "10 KB"]);
    assert_eq!(code, 1, "expected a clean error, got {code}: {msg}");
    assert!(msg.contains("file not found"), "{msg}");
}

/// A deterministic payload with repetitive runs *and* variation. Uniform bytes
/// compress to the same size at every level, so they cannot show that the
/// compression setting actually reaches the codec.
fn mixed_payload(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size + 64);
    let mut i = 0usize;
    while out.len() < size {
        out.extend(std::iter::repeat_n((i % 7) as u8, 3 + (i % 40)));
        out.extend(std::iter::repeat_n((i.wrapping_mul(31) % 256) as u8, 2));
        i += 1;
    }
    out.truncate(size);
    out
}

/// The CDN preset is what a server does on the fly, not the best achievable
/// compression: measuring at the maximum reports a smaller size than what
/// actually goes over the wire.
#[test]
fn cdn_preset_compresses_less_than_max() {
    let d = TestDir::new("compression_cdn");
    fs::write(d.path().join("app.wasm"), mixed_payload(64 * 1024)).unwrap();

    let (code, stdout, stderr) = run_capture(
        d.path(),
        &[
            "check", "--file", "app.wasm", "--budget", "1 MB", "--format", "json",
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let cdn = parse_json(&stdout);
    assert_eq!(cdn["compression"]["gzip_level"], 6, "{cdn}");
    assert_eq!(cdn["compression"]["brotli_quality"], 5, "{cdn}");

    let (code, stdout, stderr) = run_capture(
        d.path(),
        &[
            "check",
            "--file",
            "app.wasm",
            "--budget",
            "1 MB",
            "--gzip-level",
            "9",
            "--brotli-quality",
            "11",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let max = parse_json(&stdout);
    assert_eq!(max["compression"]["gzip_level"], 9, "{max}");
    assert_eq!(max["compression"]["brotli_quality"], 11, "{max}");

    // The raw size is unaffected; the compressed ones are not.
    assert_eq!(cdn["files"][0]["raw"], max["files"][0]["raw"]);
    assert!(
        cdn["files"][0]["brotli"].as_u64().unwrap() > max["files"][0]["brotli"].as_u64().unwrap(),
        "cdn brotli should be larger than max brotli: {cdn} vs {max}"
    );
    assert_ne!(cdn["files"][0]["gzip"], max["files"][0]["gzip"]);
}

/// A `compression` block in the config is used, and a CLI flag overrides only
/// the field it names.
#[test]
fn config_compression_is_used_and_a_flag_overrides_one_field() {
    let d = TestDir::new("compression_config");
    fs::write(d.path().join("app.wasm"), mixed_payload(32 * 1024)).unwrap();
    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files":["app.wasm"],"compression":{"gzip_level":9,"brotli_quality":11}}"#,
    )
    .unwrap();

    let (code, stdout, stderr) =
        run_capture(d.path(), &["check", "--format", "json", "--budget", "1 MB"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let from_config = parse_json(&stdout);
    assert_eq!(from_config["compression"]["gzip_level"], 9, "{from_config}");
    assert_eq!(
        from_config["compression"]["brotli_quality"], 11,
        "{from_config}"
    );

    let (code, stdout, stderr) = run_capture(
        d.path(),
        &[
            "check",
            "--format",
            "json",
            "--budget",
            "1 MB",
            "--gzip-level",
            "1",
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    let overridden = parse_json(&stdout);
    assert_eq!(overridden["compression"]["gzip_level"], 1, "{overridden}");
    assert_eq!(
        overridden["compression"]["brotli_quality"], 11,
        "brotli must keep the configured value: {overridden}"
    );
}

/// `init` leaves the default out of the config, and an existing custom setting
/// survives the baseline refresh.
#[test]
fn init_omits_the_default_compression_and_keeps_a_custom_one() {
    let d = TestDir::new("compression_init");
    d.write_wasm("app.wasm", 10 * 1024);

    let (code, msg) = run_in(d.path(), &["init"]);
    assert_eq!(code, 0, "{msg}");
    let default_cfg = d.read_config(&d.path().join(".wasmcheck.json"));
    assert!(
        default_cfg.get("compression").is_none(),
        "the CDN preset should not be written: {default_cfg}"
    );

    fs::write(
        d.path().join(".wasmcheck.json"),
        r#"{"files":["app.wasm"],"compression":{"gzip_level":9,"brotli_quality":11}}"#,
    )
    .unwrap();
    let (code, msg) = run_in(d.path(), &["baseline"]);
    assert_eq!(code, 0, "{msg}");
    let custom = d.read_config(&d.path().join(".wasmcheck.json"));
    assert_eq!(custom["compression"]["gzip_level"], 9, "{custom}");
    assert_eq!(custom["compression"]["brotli_quality"], 11, "{custom}");
}

#[test]
fn out_of_range_compression_settings_are_rejected() {
    let d = TestDir::new("compression_invalid");
    d.write_wasm("app.wasm", 4 * 1024);

    let (code, msg) = run_in(
        d.path(),
        &[
            "check",
            "--file",
            "app.wasm",
            "--budget",
            "1 MB",
            "--gzip-level",
            "10",
        ],
    );
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("gzip"), "{msg}");

    let (code, msg) = run_in(
        d.path(),
        &[
            "check",
            "--file",
            "app.wasm",
            "--budget",
            "1 MB",
            "--brotli-quality",
            "12",
        ],
    );
    assert_eq!(code, 1, "{msg}");
    assert!(msg.contains("brotli"), "{msg}");
}

#[test]
fn init_keeps_the_config_relative_entry() {
    let d = TestDir::new("init_relative");
    let sub = d.path().join("sub");
    fs::create_dir_all(sub.join("dist")).unwrap();
    fs::write(sub.join("dist/app.wasm"), vec![b'a'; 4096]).unwrap();

    let (code, msg) = run_in(
        d.path(),
        &[
            "init",
            "--config",
            "sub/.wasmcheck.json",
            "--file",
            "sub/dist/app.wasm",
        ],
    );
    assert_eq!(code, 0, "{msg}");

    let cfg = d.read_config(&sub.join(".wasmcheck.json"));
    assert_eq!(cfg["files"][0].as_str().unwrap(), "dist/app.wasm");
    assert!(
        read_baseline(&sub.join(".wasmcheck.json"))["dist/app.wasm"]["raw"]
            .as_u64()
            .is_some()
    );

    // ... and that config is usable from the repository root.
    let (code, msg) = run_in(d.path(), &["check", "--config", "sub/.wasmcheck.json"]);
    assert_eq!(code, 0, "{msg}");
}
