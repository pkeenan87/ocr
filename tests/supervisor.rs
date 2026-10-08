//! The `ocr scan` supervisor (src/supervisor.rs) and the Vision concurrency cap. The crash is
//! injected deterministically with `OCR_TEST_CRASH_AFTER=<checkpoints>` (and
//! `OCR_TEST_CRASH_ATTEMPTS`, `OCR_TEST_CRASH_SIGNAL`).

mod common;

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::{Env as ModelEnv, without_generated};
use tempfile::{TempDir, tempdir};

const NOTE: &str = "note: Apple Vision crashed (signal 11); resuming from checkpoints";

struct Env {
    config: TempDir,
    state: TempDir,
    work: TempDir,
}

impl Env {
    fn new() -> Self {
        let e = Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
        };
        fs::copy(
            Path::new(common::FIXTURES).join("three_pages.pdf"),
            e.path("three_pages.pdf"),
        )
        .unwrap();
        e
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.work.path().join(rel)
    }

    fn cmd(&self, env: &[(&str, &str)]) -> Command {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .envs(env.iter().copied())
            .current_dir(self.work.path())
            .args([
                "scan",
                "--input-file",
                "three_pages.pdf",
                "--use-text-layer",
                "never",
            ]);
        c
    }
}

fn golden() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/three_pages.md"))
        .unwrap()
}

fn stderr_of(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn crashed_apple_scan_resumes_and_matches_the_golden() {
    let e = Env::new();
    let o = e.cmd(&[("OCR_TEST_CRASH_AFTER", "1")]).output().unwrap();
    let err = stderr_of(&o);
    assert_eq!(o.status.code(), Some(0), "{err}");
    assert_eq!(err.matches(NOTE).count(), 1, "{err}");
    assert!(err.contains("(retry 1 of 3)"), "{err}");
    assert!(err.contains("resumed from checkpoint 1"), "{err}");
    let md = fs::read_to_string(e.path("three_pages.md")).unwrap();
    assert_eq!(without_generated(&md), without_generated(&golden()));
}

#[test]
fn overwrite_keeps_the_crashed_runs_checkpoints_on_retry() {
    let e = Env::new();
    fs::write(e.path("three_pages.md"), "old").unwrap();
    let o = e
        .cmd(&[("OCR_TEST_CRASH_AFTER", "1")])
        .arg("--overwrite")
        .output()
        .unwrap();
    let err = stderr_of(&o);
    assert_eq!(o.status.code(), Some(0), "{err}");
    assert!(err.contains("resumed from checkpoint 1"), "{err}");
    let md = fs::read_to_string(e.path("three_pages.md")).unwrap();
    assert_eq!(without_generated(&md), without_generated(&golden()));
}

#[test]
fn a_run_that_always_crashes_is_retried_three_times_then_fails() {
    let e = Env::new();
    let o = e
        .cmd(&[
            ("OCR_TEST_CRASH_AFTER", "0"),
            ("OCR_TEST_CRASH_ATTEMPTS", "99"),
        ])
        .output()
        .unwrap();
    let err = stderr_of(&o);
    assert_eq!(err.matches(NOTE).count(), 3, "{err}");
    assert!(err.contains("(retry 3 of 3)"), "{err}");
    assert!(!err.contains("retry 4"), "{err}");
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("error: Apple Vision crashed"), "{err}");
}

#[test]
fn sigterm_is_not_retried() {
    let e = Env::new();
    let o = e
        .cmd(&[
            ("OCR_TEST_CRASH_AFTER", "1"),
            ("OCR_TEST_CRASH_SIGNAL", "15"),
        ])
        .output()
        .unwrap();
    let err = stderr_of(&o);
    assert!(!err.contains("Apple Vision crashed"), "{err}");
    assert_eq!(o.status.code(), Some(128 + 15), "{err}");
    assert_eq!(o.status.signal(), None);
}

#[test]
fn dry_run_is_not_supervised() {
    let e = Env::new();
    let o = e
        .cmd(&[("OCR_TEST_CRASH_AFTER", "1")])
        .arg("--dry-run")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr_of(&o));
}

#[test]
fn exit_code_of_a_failed_scan_passes_through() {
    let e = Env::new();
    fs::write(e.path("three_pages.md"), "x").unwrap();
    // Output exists and no --overwrite: the child exits 2.
    let o = e.cmd(&[]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", stderr_of(&o));
}

/// Vision requests in flight at once, as reported by the bridge's counter.
fn vision_peak(err: &str) -> usize {
    err.lines()
        .find_map(|l| l.strip_prefix("test: vision max concurrent "))
        .unwrap_or_else(|| panic!("no counter in:\n{err}"))
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn vision_is_capped_at_two_concurrent_requests() {
    let e = Env::new();
    let o = e
        .cmd(&[("OCR_TEST_VISION_STATS", "1")])
        .args(["--jobs", "12", "--quiet"])
        .output()
        .unwrap();
    let err = stderr_of(&o);
    assert_eq!(o.status.code(), Some(0), "{err}");
    let peak = vision_peak(&err);
    assert!((1..=2).contains(&peak), "peak {peak}\n{err}");
    let md = fs::read_to_string(e.path("three_pages.md")).unwrap();
    assert_eq!(without_generated(&md), without_generated(&golden()));
}

#[test]
fn vision_cap_can_be_overridden_for_measurement() {
    let e = Env::new();
    let o = e
        .cmd(&[
            ("OCR_TEST_VISION_STATS", "1"),
            ("OCR_VISION_MAX_CONCURRENT", "1"),
        ])
        .args(["--jobs", "12", "--quiet"])
        .output()
        .unwrap();
    let err = stderr_of(&o);
    assert_eq!(o.status.code(), Some(0), "{err}");
    assert_eq!(vision_peak(&err), 1, "{err}");
}

#[test]
fn crashed_model_scan_resumes_without_repeating_pages_and_the_server_survives() {
    let e = ModelEnv::new("olmocr", &["three_pages.pdf"]);
    e.serve(&[("FAKE_MODE", "sleep"), ("FAKE_SLEEP_SECS", "1")]);
    e.scan("three_pages.pdf", &["--output-file", "clean.md"], &[])
        .success();
    let clean = fs::read_to_string(e.path("clean.md")).unwrap();
    let per_run = e.requests().len();
    assert_eq!(per_run, 3, "one request per page");
    let pid = e.server_pid();

    let a = e
        .scan(
            "three_pages.pdf",
            &["--output-file", "out.md"],
            &[("OCR_TEST_CRASH_AFTER", "1")],
        )
        .success();
    let err = stderr_of(a.get_output());
    assert_eq!(err.matches(NOTE).count(), 1, "{err}");
    assert!(err.contains("resumed from checkpoint 1"), "{err}");
    // The first attempt had at most one more page in flight (not checkpointed) when it died.
    assert!(
        e.requests().len() <= 2 * per_run + 1,
        "a finished page was requested again: {} requests",
        e.requests().len()
    );
    assert_eq!(e.server_pid(), pid);
    e.base_cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("olmocr"));
    assert_eq!(
        without_generated(&fs::read_to_string(e.path("out.md")).unwrap()),
        without_generated(&clean)
    );
}
