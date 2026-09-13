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
        let fake = vec![b'a'; size];
        fs::write(self.0.join(filename), &fake).unwrap();
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

fn run_in(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(bin())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.code().unwrap(), format!("{stdout}{stderr}"))
}

fn read_config(dir: &Path) -> serde_json::Value {
    let text = fs::read_to_string(dir.join(".wasmcheck.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn empty_dir_errors() {
    let d = TestDir::new("empty");
    let (code, msg) = run_in(d.path(), &[]);
    assert_ne!(code, 0);
    assert!(msg.contains("No .wasm file"));
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
    assert!(msg.contains("Multiple .wasm files"), "{msg}");
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

#[test]
fn json_output() {
    let d = TestDir::new("json");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, msg) = run_in(d.path(), &["--file", "app.wasm", "--format", "json"]);
    assert_eq!(code, 0, "{msg}");
    let v: serde_json::Value = serde_json::from_str(&msg).expect("json parse");
    assert!(v["raw"].as_u64().unwrap() >= 10 * 1024);
    assert!(v["gzip"].as_u64().unwrap() > 0);
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

    let cfg = read_config(d.path());
    assert_eq!(
        cfg["baseline"]["app.wasm"]["raw"].as_u64().unwrap(),
        10 * 1024
    );
    assert!(cfg["budget"]["raw"].is_string());

    // config budget == baseline (10.00 KB → exactly 10240) → pass, zero delta
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 0, "{msg}");
    assert!(msg.to_uppercase().contains("PASS"), "{msg}");
}

#[test]
fn budget_from_config_enforced() {
    let d = TestDir::new("config_budget");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    // grow the wasm → now exceeds config budget
    d.write_wasm("app.wasm", 20 * 1024);
    let (code, msg) = run_in(d.path(), &["check"]);
    assert_eq!(code, 1, "{msg}");
    assert!(msg.to_uppercase().contains("FAIL"), "{msg}");
}

#[test]
fn baseline_updates_config() {
    let d = TestDir::new("baseline_flow");
    d.write_wasm("app.wasm", 10 * 1024);
    let (code, _) = run_in(d.path(), &["init", "--file", "app.wasm"]);
    assert_eq!(code, 0);

    d.write_wasm("app.wasm", 20 * 1024);
    let (code, msg) = run_in(d.path(), &["baseline", "--file", "app.wasm"]);
    assert_eq!(code, 0, "{msg}");

    let cfg = read_config(d.path());
    assert_eq!(
        cfg["baseline"]["app.wasm"]["raw"].as_u64().unwrap(),
        20 * 1024
    );
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
    assert!(msg.contains("No .wasm file"), "{msg}");
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
