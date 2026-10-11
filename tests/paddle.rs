//! PaddleOCR-VL engine tests. The fake server (tests/fake_server.py) stands in for the model;
//! the Apple side-pass is real Vision on this Mac. The `#[ignore]`d test at the end drives the
//! real model (OCR_REAL_MODELS=1).

// Needs Apple Vision (PLAN §9).
#![cfg(target_os = "macos")]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_server.py");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spike/fixtures");

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
        let _ = self.base_cmd().args(["server", "stop"]).output();
    }
}

impl Env {
    /// A fresh environment with the named fixtures copied into the working directory.
    fn new(fixtures: &[&str]) -> Self {
        let e = Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
            port: free_port(),
        };
        for f in fixtures {
            fs::copy(Path::new(FIXTURES).join(f), e.path(f)).unwrap();
        }
        e
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.work.path().join(rel)
    }

    fn base_cmd(&self) -> Command {
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
    fn serve(&self, env: &[(&str, &str)]) {
        self.base_cmd()
            .args(["server", "setup", "--model", "paddleocr-vl"])
            .assert()
            .success();
        let port = self.port.to_string();
        self.base_cmd()
            .envs(env.iter().copied())
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

    fn scan_cmd(&self, input: &str, extra: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = self.base_cmd();
        c.envs(env.iter().copied())
            .args(["scan", "--model", "paddleocr-vl", "--input-file", input])
            .args(extra);
        c
    }

    fn scan(&self, input: &str, extra: &[&str], env: &[(&str, &str)]) -> Assert {
        self.scan_cmd(input, extra, env).assert()
    }

    fn markdown(&self, input: &str) -> String {
        let stem = input.rsplit_once('.').unwrap().0;
        fs::read_to_string(self.path(&format!("{stem}.md"))).unwrap()
    }

    /// Requests the fake server has seen, oldest first.
    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.path("fake.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn server_pid(&self) -> u64 {
        let text = fs::read_to_string(self.state.path().join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["pid"]
            .as_u64()
            .unwrap()
    }
}

fn prompts(reqs: &[Value]) -> Vec<&str> {
    reqs.iter().map(|r| r["prompt"].as_str().unwrap()).collect()
}

fn without_generated(md: &str) -> String {
    md.lines()
        .filter(|l| !l.starts_with("generated:") && !l.starts_with("scan_record:"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn legal_scan_uses_side_pass_bates_and_table_recognition() {
    let e = Env::new(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("fake ocr text"), "{md}");
    assert!(md.contains("<!-- page 1 · Bates SMITH000123 -->"), "{md}");
    assert!(
        md.contains("| Invoice | Date | Amount |\n| --- | --- | --- |\n| INV-1001 | 2026-01-05 | $48,250.00 |"),
        "{md}"
    );
    assert!(md.contains("table_pages: [1]\n"), "{md}");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
    assert!(md.contains("flagged_pages: []\n"), "{md}");
    assert!(md.contains("text_layer_pages: []\n"), "{md}");
    assert!(!md.contains("currency_fixes"), "{md}");
    assert!(
        md.contains("engine: \"paddleocr-vl (PaddlePaddle/PaddleOCR-VL-1.6, mlx-vlm 0.7.6)\"\n"),
        "{md}"
    );
    let reqs = e.requests();
    let p = prompts(&reqs);
    assert_eq!(
        p.iter().filter(|p| **p == "Table Recognition:").count(),
        1,
        "{p:?}"
    );
    assert!(p.contains(&"OCR:"), "{p:?}");
    assert!(reqs.iter().all(|r| r["max_tokens"] == 4096));
    // Bands and the table are crops of a 1275x1650 page.
    assert!(
        reqs.iter()
            .all(|r| r["w"] == 1275 && r["h"].as_u64().unwrap() < 1650)
    );
}

#[test]
fn transcript_is_one_full_page_request() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("transcript_pages: [1]\n"), "{md}");
    assert!(md.contains("table_pages: []\n"), "{md}");
    assert!(md.contains("fake ocr text 1275x1650"), "{md}");
    let reqs = e.requests();
    assert_eq!(prompts(&reqs), ["OCR:"], "{reqs:?}");
    assert_eq!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (Some(1275), Some(1650))
    );
}

/// The mini-pages of `transcript4up_seed0.expected.txt` (sheet 1 of an eval transcript4up
/// document): a `Page N` header and the physical lines, each as `N text`.
fn seed0_pages() -> Vec<(String, Vec<String>)> {
    let text =
        fs::read_to_string(Path::new(FIXTURES).join("transcript4up_seed0.expected.txt")).unwrap();
    let mut pages: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        if line.starts_with("Page ") {
            pages.push((line.to_string(), Vec::new()));
        } else {
            let body = line.split_once(' ').unwrap().1.trim().to_string();
            pages.last_mut().unwrap().1.push(body);
        }
    }
    pages
}

#[test]
fn merged_wraps_and_sequential_numbers_come_out_with_geometry_numbering() {
    let e = Env::new(&["transcript4up_seed0.pdf"]);
    let pages = seed0_pages();
    assert_eq!(pages.len(), 4);
    // What the models do: join every wrapped continuation onto the line above and number the
    // result 1, 2, 3 ... sequentially.
    let mut merged = 0;
    let replies: Vec<String> = pages
        .iter()
        .map(|(head, lines)| {
            let mut out: Vec<String> = Vec::new();
            for l in lines {
                match out.last_mut() {
                    Some(prev) if !prev.ends_with(['.', '?', '!']) => {
                        prev.push(' ');
                        prev.push_str(l);
                        merged += 1;
                    }
                    _ => out.push(l.clone()),
                }
            }
            let numbered: Vec<String> = out
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{} {l}", i + 1))
                .collect();
            format!("{head}\n{}", numbered.join("\n"))
        })
        .collect();
    assert!(
        merged >= 10,
        "the fixture should have wrapped lines: {merged}"
    );
    let replies_path = e.path("replies.json");
    fs::write(&replies_path, serde_json::to_string(&replies).unwrap()).unwrap();
    e.serve(&[("FAKE_REPLIES", replies_path.to_str().unwrap())]);
    // Every page is anchored: no hint on stderr.
    e.scan("transcript4up_seed0.pdf", &["--pages", "1"], &[])
        .success()
        .stderr(predicate::str::contains("could not be re-anchored").not());
    let md = e.markdown("transcript4up_seed0.pdf");
    assert!(md.contains("transcript_pages: [1]\n"), "{md}");
    assert!(
        md.contains("line_numbers: {anchored: [1], unverified: []}\n"),
        "{md}"
    );
    // One request per mini-page: quadrant crops, not the 1275x1650 sheet.
    let reqs = e.requests();
    assert_eq!(prompts(&reqs), ["OCR:"; 4], "{reqs:?}");
    assert!(
        reqs.iter().all(|r| r["w"].as_u64() < Some(1275)),
        "{reqs:?}"
    );
    // The engine's words carry the geometry's numbers: every physical line is exact.
    let body = md.split_once("<!-- page 1 -->\n").unwrap().1;
    let got: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    let want =
        fs::read_to_string(Path::new(FIXTURES).join("transcript4up_seed0.expected.txt")).unwrap();
    let want: Vec<&str> = want.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(got, want);
}

#[test]
fn text_that_does_not_match_the_page_is_kept_and_marked_unverified() {
    let e = Env::new(&["transcript4up_seed0.pdf"]);
    e.serve(&[]);
    e.scan("transcript4up_seed0.pdf", &["--pages", "1"], &[])
        .success()
        .stderr(predicate::str::contains(
            "transcript4up_seed0.pdf: transcript pages 1 could not be re-anchored to page geometry (line_numbers.unverified); verify page:line citations against the PDF",
        ));
    let md = e.markdown("transcript4up_seed0.pdf");
    assert!(
        md.contains("line_numbers: {anchored: [], unverified: [1]}\n"),
        "{md}"
    );
    // The engine's text is kept, never replaced.
    assert!(md.contains("fake ocr text"), "{md}");
}

#[test]
fn dpi_override_changes_the_render() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &["--dpi", "100"], &[])
        .success();
    let reqs = e.requests();
    assert_eq!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (Some(850), Some(1100))
    );
}

#[test]
fn narrow_columns_are_cropped() {
    let e = Env::new(&["narrow_unequal_seed2.pdf"]);
    e.serve(&[]);
    e.scan("narrow_unequal_seed2.pdf", &[], &[]).success();
    let md = e.markdown("narrow_unequal_seed2.pdf");
    assert!(md.contains("column_split_pages: [1]\n"), "{md}");
    let reqs = e.requests();
    assert_eq!(prompts(&reqs), ["OCR:", "OCR:"], "{reqs:?}");
    assert!(
        reqs.iter().all(|r| r["w"].as_u64().unwrap() < 1275),
        "{reqs:?}"
    );
    assert_eq!(md.matches("fake ocr text").count(), 2, "{md}");
}

#[test]
fn text_layer_is_opt_in() {
    let e = Env::new(&["legal.pdf"]);
    e.serve(&[]);
    e.scan("legal.pdf", &["--use-text-layer", "always"], &[])
        .success();
    assert!(e.markdown("legal.pdf").contains("text_layer_pages: [1]\n"));
    assert!(e.requests().is_empty());
}

#[test]
fn loop_is_retried_with_a_penalty_then_falls_back_to_apple() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("repetition loop"));
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fallback_pages: [1]\n"), "{md}");
    assert!(md.contains("flagged_pages: [1]\n"), "{md}");
    assert!(!md.contains("the same line again"), "{md}");
    assert!(md.contains("<!-- page 1 -->"), "{md}");
    let reqs = e.requests();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert!(reqs[0]["repetition_penalty"].is_null());
    assert_eq!(reqs[1]["repetition_penalty"], 1.15);
}

#[test]
fn loop_that_clears_on_retry_is_not_a_fallback() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let md = e.markdown("transcript_full.pdf");
    assert!(
        md.contains("fallback_pages: []\n") && md.contains("fake ocr text"),
        "{md}"
    );
    assert_eq!(e.requests().len(), 2);
}

#[test]
fn looping_crops_are_retried_once_then_fall_back() {
    // The column plan's own loop ladder: the first crop is retried once, then the page falls back.
    let e = Env::new(&["narrow_unequal_seed2.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("narrow_unequal_seed2.pdf", &[], &[]).code(1);
    let md = e.markdown("narrow_unequal_seed2.pdf");
    assert!(md.contains("fallback_pages: [1]\n"), "{md}");
    assert_eq!(e.requests().len(), 2, "first crop twice, then fallback");
}

#[test]
fn looping_full_page_is_retried_with_region_crops() {
    // signature_block.pdf: one full-page request, but two regions the side-pass can crop.
    let e = Env::new(&[]);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/heldout/signature_block.pdf"),
        e.path("signature_block.pdf"),
    )
    .unwrap();
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("signature_block.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert_eq!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (Some(1275), Some(1650))
    );
    assert!(
        reqs[1..].iter().all(|r| r["h"].as_u64().unwrap() < 1650),
        "{reqs:?}"
    );
    assert!(
        reqs.iter().all(|r| r["repetition_penalty"].is_null()),
        "{reqs:?}"
    );
    let md = e.markdown("signature_block.pdf");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
    assert_eq!(md.matches("fake ocr text").count(), 2, "{md}");
}

#[test]
fn timing_json_records_requests_and_totals_and_changes_nothing_else() {
    let e = Env::new(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_TIMINGS", "1")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let plain = without_generated(&e.markdown("legal_scan.pdf"));
    assert!(!e.path("legal_scan.timing.json").exists());
    e.scan("legal_scan.pdf", &["--overwrite", "--timing-json"], &[])
        .success();
    assert_eq!(without_generated(&e.markdown("legal_scan.pdf")), plain);
    let t: Value =
        serde_json::from_str(&fs::read_to_string(e.path("legal_scan.timing.json")).unwrap())
            .unwrap();
    assert_eq!(t["engine"], "paddleocr-vl");
    assert_eq!(t["model"], "PaddlePaddle/PaddleOCR-VL-1.6");
    assert_eq!(t["dpi"], 150);
    let pages = t["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert!(pages[0]["prep_s"].as_f64().unwrap() > 0.0);
    assert!(pages[0]["recognize_s"].as_f64().unwrap() > 0.0);
    let reqs = pages[0]["requests"].as_array().unwrap();
    assert!(reqs.len() >= 2, "{reqs:?}");
    for r in reqs {
        assert_eq!(r["reason"], "first");
        assert_eq!(r["outcome"], "ok");
        assert_eq!(r["finish_reason"], "stop");
        assert_eq!(r["prompt_tokens"], 1000);
        assert_eq!(r["predicted_n"], 50);
        assert_eq!(r["prompt_ms"], 500.0);
        assert!(r["image_bytes"].as_u64().unwrap() > 1000);
        assert!(r["wall_s"].as_f64().unwrap() > 0.0);
    }
    let n = reqs.len() as u64;
    let totals = &t["totals"];
    assert_eq!(totals["prompt_tokens"].as_u64(), Some(1000 * n));
    assert_eq!(totals["completion_tokens"].as_u64(), Some(50 * n));
    assert_eq!(totals["server_prompt_s"].as_f64(), Some(0.5 * n as f64));
    assert_eq!(totals["server_decode_s"].as_f64(), Some(0.25 * n as f64));
    assert_eq!(totals["attempts_by_reason"]["first"].as_u64(), Some(n));
    assert!(totals["wall_s"].as_f64().unwrap() >= totals["prep_s"].as_f64().unwrap());
    assert!(totals["requests_s"].as_f64().unwrap() > 0.0);
    assert!(totals["write_s"].as_f64().is_some() && totals["verify_s"].as_f64().is_some());
    // A later run without the flag removes the old timings: they describe another run.
    e.scan("legal_scan.pdf", &["--overwrite"], &[]).success();
    assert!(!e.path("legal_scan.timing.json").exists());
}

#[test]
fn timing_json_names_retry_reasons_and_verbose_prints_request_lines() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[
        ("FAKE_MODE", "empty"),
        ("FAKE_FAIL_FIRST", "1"),
        ("FAKE_TIMINGS", "1"),
    ]);
    let out = e
        .scan("transcript_full.pdf", &["--timing-json", "--verbose"], &[])
        .success();
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(err.contains("page 1 req 1/2 OCR"), "{err}");
    assert!(
        err.contains("prompt 1000 tok 0.5s, decode 50 tok 0.2s"),
        "{err}"
    );
    let t: Value =
        serde_json::from_str(&fs::read_to_string(e.path("transcript_full.timing.json")).unwrap())
            .unwrap();
    let reasons: Vec<&str> = t["pages"][0]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["first", "empty_retry"]);
    assert_eq!(t["totals"]["attempts_by_reason"]["empty_retry"], 1);
}

#[test]
fn empty_output_is_retried_once() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "empty"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    assert_eq!(e.requests().len(), 2);
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fake ocr text 1275x1650")
    );
}

#[test]
fn persistent_empty_output_falls_back_and_flags() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "empty")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("empty model output"));
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fallback_pages: [1]\n"), "{md}");
    assert!(md.contains("flagged_pages: [1]\n"), "{md}");
    assert_eq!(e.requests().len(), 2, "one try and one retry");
}

#[test]
fn truncation_is_retried_with_8192_tokens() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "length"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let reqs = e.requests();
    let tokens: Vec<u64> = reqs
        .iter()
        .map(|r| r["max_tokens"].as_u64().unwrap())
        .collect();
    assert_eq!(tokens, [4096, 8192], "{reqs:?}");
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fallback_pages: []\n")
    );
}

#[test]
fn persistent_truncation_falls_back() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "length")]);
    e.scan("transcript_full.pdf", &[], &[]).code(1);
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fallback_pages: [1]\n")
    );
    assert_eq!(e.requests().len(), 2);
}

#[test]
fn timeout_restarts_the_server_then_falls_back() {
    let e = Env::new(&["transcript_full.pdf"]);
    let env = [("FAKE_MODE", "sleep"), ("FAKE_SLEEP_SECS", "20")];
    e.serve(&env);
    let before = e.server_pid();
    e.scan(
        "transcript_full.pdf",
        &[],
        &[("OCR_REQUEST_TIMEOUT_SECS", "2"), env[0], env[1]],
    )
    .code(1)
    .stderr(predicate::str::contains("restarting the model server"));
    assert_ne!(e.server_pid(), before, "server was not restarted");
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fallback_pages: [1]\n"), "{md}");
    assert_eq!(e.requests().len(), 2, "one try and one retry");
}

#[test]
fn timeout_retry_can_succeed() {
    let e = Env::new(&["transcript_full.pdf"]);
    let env = [
        ("FAKE_MODE", "sleep"),
        ("FAKE_SLEEP_SECS", "20"),
        ("FAKE_FAIL_FIRST", "1"),
    ];
    e.serve(&env);
    let before = e.server_pid();
    e.scan(
        "transcript_full.pdf",
        &[],
        &[("OCR_REQUEST_TIMEOUT_SECS", "2"), env[0], env[1], env[2]],
    )
    .success();
    assert_ne!(e.server_pid(), before);
    assert!(e.markdown("transcript_full.pdf").contains("fake ocr text"));
}

#[test]
fn server_errors_are_tried_three_times_then_fall_back() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "error5xx")]);
    let t = Instant::now();
    e.scan("transcript_full.pdf", &[], &[]).code(1);
    assert!(t.elapsed() >= Duration::from_millis(1500), "no backoff");
    assert_eq!(e.requests().len(), 3);
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fallback_pages: [1]\n")
    );
}

#[test]
fn server_error_that_clears_is_retried() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "error5xx"), ("FAKE_FAIL_FIRST", "2")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    assert_eq!(e.requests().len(), 3);
}

#[test]
fn stopped_server_exits_3() {
    let e = Env::new(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.base_cmd().args(["server", "stop"]).assert().success();
    e.scan("legal_scan.pdf", &[], &[])
        .code(3)
        .stderr(predicate::str::contains(
            "ocr server start --model paddleocr-vl",
        ));
}

#[test]
fn server_stopped_mid_run_ends_the_run() {
    let e = Env::new(&["legal_scan.pdf", "transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "sleep"), ("FAKE_SLEEP_SECS", "20")]);
    let program = e.base_cmd().get_program().to_owned();
    let child = std::process::Command::new(program)
        .env("OCR_CONFIG_DIR", e.config.path())
        .env("OCR_STATE_DIR", e.state.path())
        .env("OCR_FAKE_INSTALL", "1")
        .env("OCR_SERVER_CMD", FAKE)
        .env("FAKE_LOG", e.path("fake.log"))
        .env("OCR_REQUEST_TIMEOUT_SECS", "60")
        .current_dir(e.work.path())
        .args(["scan", "--model", "paddleocr-vl", "--input-dir", "."])
        .args(["--output-dir", "out"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Wait for the first request to be in flight, then stop the server like a user would.
    let t = Instant::now();
    while e.requests().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(60), "no request arrived");
        std::thread::sleep(Duration::from_millis(100));
    }
    e.base_cmd().args(["server", "stop"]).assert().success();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("server stopped; re-run to resume"),
        "{stderr}"
    );
}

#[test]
fn jobs_do_not_change_the_output() {
    let e = Env::new(&["three_pages.pdf"]);
    e.serve(&[]);
    let run = |extra: &[&str]| {
        e.scan("three_pages.pdf", &[&["--overwrite"], extra].concat(), &[])
            .success();
        without_generated(&e.markdown("three_pages.pdf"))
    };
    let default = run(&[]);
    assert_eq!(run(&["--jobs", "1"]), default);
    assert_eq!(run(&["--jobs", "3"]), default);
    assert!(default.contains("<!-- page 3"), "{default}");
    let pages: Vec<usize> = default
        .match_indices("<!-- page ")
        .map(|(i, _)| {
            default[i + 10..]
                .split([' ', '-'])
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert_eq!(pages, [1, 2, 3]);
}

#[test]
fn jobs_above_one_print_the_request_cap_note() {
    let e = Env::new(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &["--jobs", "2"], &[])
        .success()
        .stderr(predicate::str::contains(
            "note: model requests are limited to 1 at a time by the server (--max-num-seqs); --jobs still sets Apple side-pass threads",
        ));
}

// ------------------------------------------------------------- real model

/// Real uv venv + weights from the HF cache + a real server. Run with
/// `OCR_REAL_MODELS=1 cargo test --release real_paddle -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_paddle_engine() {
    if std::env::var("OCR_REAL_MODELS").as_deref() != Ok("1") {
        eprintln!("OCR_REAL_MODELS!=1; skipping");
        return;
    }
    let e = Env::new(&[
        "legal_scan.pdf",
        "transcript_full.pdf",
        "four_up.pdf",
        "narrow_unequal_seed2.pdf",
    ]);
    fs::copy(
        Path::new(FIXTURES).join("narrow_unequal_seed2.expected.json"),
        e.path("expected.json"),
    )
    .unwrap();
    let real = |mut c: Command| {
        c.env_remove("OCR_FAKE_INSTALL")
            .env_remove("OCR_SERVER_CMD")
            .env_remove("FAKE_LOG")
            .env("OCR_READY_TIMEOUT_SECS", "300")
            .timeout(Duration::from_secs(1800));
        c
    };
    real(e.base_cmd())
        .args(["server", "setup", "--model", "paddleocr-vl"])
        .assert()
        .success();
    let port = e.port.to_string();
    real(e.base_cmd())
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

    let failures = std::cell::RefCell::new(Vec::<String>::new());
    let scan = |input: &str, label: &str| -> String {
        let t = Instant::now();
        let out = real(e.scan_cmd(input, &["--overwrite", "--verbose"], &[]))
            .output()
            .unwrap();
        println!(
            "[{label}] {input}: exit {:?} in {:.1}s\n{}",
            out.status.code(),
            t.elapsed().as_secs_f64(),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.code() != Some(0) {
            failures
                .borrow_mut()
                .push(format!("{label}: exit {:?}", out.status.code()));
        }
        e.markdown(input)
    };

    // legal_scan: case number, four amounts and the Bates number are exact; twice, byte-identical.
    let first = scan("legal_scan.pdf", "legal #1");
    for t in [
        "3:26-cv-04417",
        "$48,250.00",
        "$51,900.00",
        "$22,375.50",
        "$122,525.50",
        "SMITH000123",
    ] {
        if !first.contains(t) {
            failures
                .borrow_mut()
                .push(format!("legal_scan missing {t}"));
        }
    }
    let second = scan("legal_scan.pdf", "legal #2");
    if without_generated(&first) != without_generated(&second) {
        failures
            .borrow_mut()
            .push("legal_scan differs between runs".into());
    }
    println!("--- legal_scan.md ---\n{first}");

    // Transcript: 25 lines, odd Q and even A.
    let md = scan("transcript_full.pdf", "transcript");
    let lines =
        regex::Regex::new(r"(?m)^\s*(\d+)\s+([QA])\.\s+Line (\d+) of the testimony").unwrap();
    let ok = (1..=25)
        .filter(|n| {
            let who = if n % 2 == 1 { "Q" } else { "A" };
            lines
                .captures_iter(&md)
                .any(|c| c[1] == n.to_string() && &c[2] == who && c[3] == n.to_string())
        })
        .count();
    println!("transcript_full: {ok}/25 exact");
    if ok != 25 {
        failures
            .borrow_mut()
            .push(format!("transcript_full {ok}/25"));
    }

    // 4-up: 100 lines.
    let md = scan("four_up.pdf", "four_up");
    let ok = (1..=4)
        .flat_map(|k| (1..=25).map(move |n| (k, n)))
        .filter(|(k, n)| {
            regex::Regex::new(&format!(
                r"\b{n}\s+Q\.\s+Question {k}\.{n} about the meeting"
            ))
            .unwrap()
            .is_match(&md)
        })
        .count();
    println!("four_up: {ok}/100 exact");
    if ok != 100 {
        failures.borrow_mut().push(format!("four_up {ok}/100"));
    }

    // Narrow unequal columns: every expected prefix, in order.
    let md = scan("narrow_unequal_seed2.pdf", "narrow");
    let expected: Vec<String> =
        serde_json::from_str(&fs::read_to_string(e.path("expected.json")).unwrap()).unwrap();
    let mut at = 0;
    let (mut found, mut in_order) = (0, 0);
    for p in &expected {
        if let Some(i) = md.find(p.as_str()) {
            found += 1;
            if i >= at {
                in_order += 1;
                at = i;
            }
        }
    }
    println!(
        "narrow_unequal_seed2: found {found}/{} in order {in_order}",
        expected.len()
    );
    if found != expected.len() || in_order != expected.len() {
        failures.borrow_mut().push(format!(
            "narrow {found}/{} found, {in_order} in order",
            expected.len()
        ));
    }

    real(e.base_cmd())
        .args(["server", "stop"])
        .assert()
        .success();
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "real-model failures: {failures:#?}");
}
