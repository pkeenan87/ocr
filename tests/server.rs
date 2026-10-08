//! Server management integration tests, driven by tests/fake_server.py (no real models).

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command as Proc, Stdio};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::{TempDir, tempdir};

/// The fake server; on Windows a `.cmd` wrapper, since a `.py` cannot be run directly.
#[cfg(not(windows))]
const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.py");
#[cfg(windows)]
const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.cmd");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Env {
    config: TempDir,
    state: TempDir,
    work: TempDir,
    port: u16,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().args(["server", "stop"]).output();
    }
}

impl Env {
    fn new() -> Self {
        Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
            port: free_port(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .env("OCR_FAKE_INSTALL", "1")
            .env("OCR_SERVER_CMD", FAKE)
            .env("OCR_READY_TIMEOUT_SECS", "60")
            .env_remove("FAKE_MODE")
            .current_dir(self.work.path());
        c
    }

    fn setup(&self, models: &[&str]) {
        let mut c = self.cmd();
        c.args(["server", "setup"]);
        for m in models {
            c.args(["--model", m]);
        }
        c.assert().success();
    }

    fn start(&self, model: &str) -> assert_cmd::assert::Assert {
        let port = self.port.to_string();
        self.cmd()
            .args(["server", "start", "--model", model, "--port", &port])
            .assert()
    }

    fn json_path(&self) -> PathBuf {
        self.state.path().join("server.json")
    }

    fn server_json(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.json_path()).unwrap()).unwrap()
    }
}

/// True if something still accepts connections on the fake server's port (works on every
/// platform, unlike scanning `ps` output).
fn fake_alive(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &([127, 0, 0, 1], port).into(),
        std::time::Duration::from_millis(500),
    )
    .is_ok()
}

#[test]
fn setup_writes_config_with_version_quant_repo() {
    let e = Env::new();
    e.setup(&["paddleocr-vl", "dots-ocr"]);
    let toml = fs::read_to_string(e.config.path().join("config.toml")).unwrap();
    // macOS serves MLX weights through mlx-vlm; elsewhere GGUF builds through llama.cpp.
    let platform = if cfg!(target_os = "macos") {
        [
            "repo = \"PaddlePaddle/PaddleOCR-VL-1.6\"",
            "version = \"dots.mocr\"",
            "repo = \"mlx-community/dots.mocr-8bit\"",
            "mlx_vlm = \"0.7.6\"",
        ]
    } else {
        [
            "repo = \"PaddlePaddle/PaddleOCR-VL-1.6-GGUF\"",
            "version = \"dots.ocr\"",
            "repo = \"ggml-org/dots.ocr-GGUF\"",
            "llama_cpp = \"b11510\"",
        ]
    };
    for needle in [
        "[engines.paddleocr-vl]",
        "version = \"1.6\"",
        "quant = \"bf16\"",
        "[engines.dots-ocr]",
        "[runtime]",
    ]
    .into_iter()
    .chain(platform)
    {
        assert!(toml.contains(needle), "missing {needle} in\n{toml}");
    }
    // Idempotent.
    e.setup(&["paddleocr-vl", "dots-ocr"]);
    assert_eq!(
        toml,
        fs::read_to_string(e.config.path().join("config.toml")).unwrap()
    );
    // Explicit version and quant.
    let (quant, repo) = if cfg!(target_os = "macos") {
        ("4bit", "mlx-community/PaddleOCR-VL-1.5-4bit")
    } else {
        ("bf16", "PaddlePaddle/PaddleOCR-VL-1.5-GGUF")
    };
    e.cmd()
        .args(["server", "setup", "--model", "paddleocr-vl"])
        .args(["--model-version", "1.5", "--quant", quant])
        .assert()
        .success();
    let toml = fs::read_to_string(e.config.path().join("config.toml")).unwrap();
    assert!(toml.contains(&format!("repo = \"{repo}\"")));
}

#[test]
fn start_status_stop() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    e.start("paddleocr-vl")
        .success()
        .stdout(predicate::str::contains("ready"));
    let j = e.server_json();
    assert_eq!(j["engine"], "paddleocr-vl");
    let repo = if cfg!(target_os = "macos") {
        "PaddlePaddle/PaddleOCR-VL-1.6"
    } else {
        "PaddlePaddle/PaddleOCR-VL-1.6-GGUF"
    };
    assert_eq!(j["repo"], repo);
    assert_eq!(j["port"], e.port);
    assert!(j["argv"].as_array().unwrap().len() >= 9);
    e.cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("engine:  paddleocr-vl"))
        .stdout(predicate::str::contains("quant:   bf16"))
        .stdout(predicate::str::contains(format!("port:    {}", e.port)))
        .stdout(predicate::str::contains("health:  reachable"));
    assert!(fake_alive(e.port));
    e.cmd()
        .args(["server", "stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stopped paddleocr-vl"));
    assert!(!e.json_path().exists());
    assert!(!fake_alive(e.port));
    e.cmd()
        .args(["server", "stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no server running"));
}

#[test]
fn slow_ready_waits() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    let port = e.port.to_string();
    e.cmd()
        .env("FAKE_MODE", "slow_ready")
        .args([
            "server",
            "start",
            "--model",
            "paddleocr-vl",
            "--port",
            &port,
        ])
        .assert()
        .success();
}

#[test]
fn start_when_running_is_noop() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    e.start("paddleocr-vl").success();
    let pid = e.server_json()["pid"].clone();
    e.start("paddleocr-vl")
        .success()
        .stdout(predicate::str::contains("already running"));
    assert_eq!(e.server_json()["pid"], pid);
}

#[test]
fn different_engine_needs_force() {
    let e = Env::new();
    e.setup(&["paddleocr-vl", "dots-ocr"]);
    e.start("paddleocr-vl").success();
    let pid = e.server_json()["pid"].clone();
    e.start("dots-ocr")
        .code(2)
        .stderr(predicate::str::contains("--force"));
    assert_eq!(e.server_json()["pid"], pid);
    let port = e.port.to_string();
    e.cmd()
        .args([
            "server", "start", "--model", "dots-ocr", "--port", &port, "--force",
        ])
        .assert()
        .success();
    let j = e.server_json();
    assert_eq!(j["engine"], "dots-ocr");
    assert_ne!(j["pid"], pid);
}

#[test]
fn foreign_listener_on_port_exits_2() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    let _l = TcpListener::bind(("127.0.0.1", e.port)).unwrap();
    e.start("paddleocr-vl")
        .code(2)
        .stderr(predicate::str::contains("--port"));
    assert!(!e.json_path().exists());
}

#[test]
fn never_ready_times_out_and_cleans_up() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    let port = e.port.to_string();
    e.cmd()
        .env("FAKE_MODE", "never_ready")
        .env("OCR_READY_TIMEOUT_SECS", "3")
        .args([
            "server",
            "start",
            "--model",
            "paddleocr-vl",
            "--port",
            &port,
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("not ready after 3s"))
        .stderr(predicate::str::contains("fake_server:"));
    assert!(!e.json_path().exists());
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!fake_alive(e.port), "orphaned fake server");
}

#[test]
fn crash_exits_3() {
    let e = Env::new();
    e.setup(&["paddleocr-vl"]);
    let port = e.port.to_string();
    e.cmd()
        .env("FAKE_MODE", "crash")
        .args([
            "server",
            "start",
            "--model",
            "paddleocr-vl",
            "--port",
            &port,
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("exited early"))
        .stderr(predicate::str::contains("crashing on purpose"));
    assert!(!e.json_path().exists());
}

fn stale_json(e: &Env, pid: u32) {
    let j = serde_json::json!({
        "pid": pid, "engine": "paddleocr-vl", "version": "1.6", "quant": "bf16",
        "repo": "PaddlePaddle/PaddleOCR-VL-1.6", "port": e.port,
        "argv": ["/nowhere/mlx_vlm.server", "--host", "127.0.0.1", "--port", e.port.to_string()],
        "env": [], "started_at": "2026-01-01T00:00:00Z",
    });
    fs::write(e.json_path(), j.to_string()).unwrap();
}

#[test]
fn stale_dead_pid_is_cleaned_up() {
    for sub in ["status", "stop"] {
        let e = Env::new();
        let mut child = Proc::new("true").spawn().unwrap();
        child.wait().unwrap();
        stale_json(&e, child.id());
        e.cmd()
            .args(["server", sub])
            .assert()
            .success()
            .stdout(predicate::str::contains("no server running"));
        assert!(!e.json_path().exists(), "{sub} left server.json");
    }
}

#[test]
fn stale_foreign_live_pid_is_never_killed() {
    for sub in ["status", "stop"] {
        let e = Env::new();
        // A long-lived process that is not ours (`sleep` does not exist on Windows).
        let mut other = if cfg!(windows) {
            let mut p = Proc::new("ping");
            p.args(["-n", "60", "127.0.0.1"]);
            p
        } else {
            let mut p = Proc::new("sleep");
            p.arg("60");
            p
        };
        let mut other = other.stdout(Stdio::null()).spawn().unwrap();
        stale_json(&e, other.id());
        e.cmd()
            .args(["server", sub])
            .assert()
            .success()
            .stdout(predicate::str::contains("no server running"));
        assert!(!e.json_path().exists());
        assert!(
            other.try_wait().unwrap().is_none(),
            "{sub} killed a foreign pid"
        );
        other.kill().unwrap();
        other.wait().unwrap();
    }
}

#[test]
fn scan_gate() {
    let e = Env::new();
    e.setup(&["paddleocr-vl", "dots-ocr"]);
    fs::write(e.work.path().join("a.pdf"), b"%PDF-1.7\n").unwrap();
    let scan = |model: &str| {
        e.cmd()
            .args(["scan", "--model", model, "--input-file", "a.pdf"])
            .assert()
    };
    scan("paddleocr-vl")
        .code(3)
        .stderr(predicate::str::contains(
            "run 'ocr server start --model paddleocr-vl'",
        ));
    e.start("dots-ocr").success();
    scan("paddleocr-vl")
        .code(3)
        .stderr(predicate::str::contains("dots-ocr"))
        .stderr(predicate::str::contains("paddleocr-vl"));
    // dots is implemented now: the gate lets the scan through (a.pdf itself is not a PDF, so the
    // file fails, which is exit 1 and not a server or gate error).
    scan("dots-ocr")
        .code(1)
        .stderr(predicate::str::contains("not implemented").not())
        .stderr(predicate::str::contains("cannot read"));
    e.cmd()
        .args([
            "scan",
            "--dry-run",
            "--model",
            "paddleocr-vl",
            "--input-file",
            "a.pdf",
        ])
        .assert()
        .success();
}

/// Real uv venv + mlx-vlm + weights from the HF cache. Run with OCR_REAL_MODELS=1 --ignored.
#[test]
#[ignore]
fn real_paddle_setup_start_status_stop() {
    if std::env::var("OCR_REAL_MODELS").as_deref() != Ok("1") {
        eprintln!("OCR_REAL_MODELS!=1; skipping");
        return;
    }
    let e = Env::new();
    let real = |args: &[&str]| {
        let mut c = e.cmd();
        c.env_remove("OCR_FAKE_INSTALL")
            .env_remove("OCR_SERVER_CMD")
            .env("OCR_READY_TIMEOUT_SECS", "300")
            .timeout(std::time::Duration::from_secs(900))
            .args(args);
        let t = std::time::Instant::now();
        let out = c.output().unwrap();
        println!(
            "$ ocr {} -> {:?} in {:.1}s\n{}{}",
            args.join(" "),
            out.status.code(),
            t.elapsed().as_secs_f64(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    let port = e.port.to_string();
    assert!(
        real(&["server", "setup", "--model", "paddleocr-vl"])
            .status
            .success()
    );
    assert!(
        real(&[
            "server",
            "start",
            "--model",
            "paddleocr-vl",
            "--port",
            &port
        ])
        .status
        .success()
    );
    let status = real(&["server", "status"]);
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("health:  reachable"), "{text}");
    assert!(real(&["server", "stop"]).status.success());
    assert!(!e.json_path().exists());
}
