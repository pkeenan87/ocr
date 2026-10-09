//! Shared harness of the model-engine tests (olmocr.rs, dots.rs): a temp config and state dir, a
//! fake model server (tests/fake_server.py) on a free port, and the real Apple side-pass.
#![allow(dead_code)]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

#[cfg(not(windows))]
pub const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.py");
/// On Windows a `.cmd` wrapper, since a `.py` cannot be run directly.
#[cfg(windows)]
pub const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.cmd");
pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spike/fixtures");
pub const HELDOUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spike/heldout");

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Env {
    pub engine: &'static str,
    pub config: TempDir,
    pub state: TempDir,
    pub work: TempDir,
    pub port: u16,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.base_cmd().args(["server", "stop"]).output();
    }
}

impl Env {
    /// A fresh environment with the named fixtures copied into the working directory.
    pub fn new(engine: &'static str, fixtures: &[&str]) -> Self {
        let e = Self {
            engine,
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
            port: free_port(),
        };
        for f in fixtures {
            let dir = if Path::new(FIXTURES).join(f).exists() {
                FIXTURES
            } else {
                HELDOUT
            };
            fs::copy(Path::new(dir).join(f), e.path(f)).unwrap();
        }
        e
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.work.path().join(rel)
    }

    pub fn base_cmd(&self) -> Command {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .env("OCR_FAKE_INSTALL", "1")
            .env("OCR_SERVER_CMD", FAKE)
            .env("OCR_READY_TIMEOUT_SECS", "60")
            .env("FAKE_LOG", self.path("fake.log"))
            .env_remove("FAKE_MODE")
            .env_remove("FAKE_FAIL_FIRST")
            .env_remove("OCR_REQUEST_TIMEOUT_SECS")
            .current_dir(self.work.path());
        c
    }

    /// Sets the fake model server up and starts it with the given environment.
    pub fn serve(&self, env: &[(&str, &str)]) {
        self.base_cmd()
            .args(["server", "setup", "--model", self.engine])
            .assert()
            .success();
        let port = self.port.to_string();
        self.base_cmd()
            .envs(env.iter().copied())
            .args(["server", "start", "--model", self.engine, "--port", &port])
            .assert()
            .success();
    }

    pub fn scan_cmd(&self, input: &str, extra: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = self.base_cmd();
        c.envs(env.iter().copied())
            .args(["scan", "--model", self.engine, "--input-file", input])
            .args(extra);
        c
    }

    /// The same command as `scan_cmd`, as a plain `std` command that can be spawned and killed.
    pub fn std_scan(
        &self,
        input: &str,
        extra: &[&str],
        env: &[(&str, &str)],
    ) -> std::process::Command {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_ocr"));
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .env("OCR_FAKE_INSTALL", "1")
            .env("OCR_SERVER_CMD", FAKE)
            .env("FAKE_LOG", self.path("fake.log"))
            .env_remove("OCR_REQUEST_TIMEOUT_SECS")
            .envs(env.iter().copied())
            .args(["scan", "--model", self.engine, "--input-file", input])
            .args(extra)
            .current_dir(self.work.path());
        c
    }

    /// The most model requests the fake server had in progress at once (needs `FAKE_LOG`).
    pub fn max_overlap(&self) -> usize {
        let read = |name: &str| -> Vec<Value> {
            fs::read_to_string(self.path(name))
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        };
        let mut events: Vec<(f64, i32)> = Vec::new();
        for r in read("fake.log") {
            events.push((r["t"].as_f64().unwrap(), 1));
        }
        for r in read("fake.log.end") {
            events.push((r["t"].as_f64().unwrap(), -1));
        }
        // An end at the same instant as a start counts first.
        events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut now, mut max) = (0i32, 0i32);
        for (_, d) in events {
            now += d;
            max = max.max(now);
        }
        max as usize
    }

    pub fn scan(&self, input: &str, extra: &[&str], env: &[(&str, &str)]) -> Assert {
        self.scan_cmd(input, extra, env).assert()
    }

    pub fn markdown(&self, input: &str) -> String {
        let stem = input.rsplit_once('.').unwrap().0;
        fs::read_to_string(self.path(&format!("{stem}.md"))).unwrap()
    }

    /// Requests the fake server has seen, oldest first.
    pub fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.path("fake.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub fn server_pid(&self) -> u64 {
        let text = fs::read_to_string(self.state.path().join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["pid"]
            .as_u64()
            .unwrap()
    }

    /// Real uv venv, weights from the HF cache and a real server (for the `#[ignore]`d tests).
    pub fn real(mut c: Command) -> Command {
        c.env_remove("OCR_FAKE_INSTALL")
            .env_remove("OCR_SERVER_CMD")
            .env_remove("FAKE_LOG")
            .env("OCR_READY_TIMEOUT_SECS", "300")
            .timeout(Duration::from_secs(3600));
        c
    }
}

pub fn without_generated(md: &str) -> String {
    md.lines()
        .filter(|l| !l.starts_with("generated:"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn longest(r: &Value) -> u64 {
    r["w"].as_u64().unwrap().max(r["h"].as_u64().unwrap())
}
