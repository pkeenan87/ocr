//! olmOCR engine tests. The fake server (tests/fake_server.py) stands in for the model; the Apple
//! side-pass is real Vision on this Mac. The `#[ignore]`d test at the end drives the real model
//! (OCR_REAL_MODELS=1).

// Needs Apple Vision (PLAN §9).
#![cfg(target_os = "macos")]

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Env, FIXTURES, longest, without_generated};
use predicates::prelude::*;

fn env(fixtures: &[&str]) -> Env {
    Env::new("olmocr", fixtures)
}

#[test]
fn page_is_text_first_1288_px_with_front_matter_stripped() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("fake olmocr text 995x1288"), "{md}");
    assert!(
        !md.contains("primary_language") && !md.contains("is_rotation_valid"),
        "{md}"
    );
    assert!(!md.contains("page_10_20"), "{md}");
    assert!(md.contains("[Figure: A bar chart]"), "{md}");
    assert!(
        md.contains("| Invoice | Date | Amount |\n| --- | --- | --- |\n| INV-1001 | 2026-01-05 | $48,250.00 |"),
        "{md}"
    );
    assert!(!md.contains("<table>"), "{md}");
    // The Bates number comes from the Apple side-pass, not the model.
    assert!(md.contains("<!-- page 1 · Bates SMITH000123 -->"), "{md}");
    assert!(md.contains("table_pages: [1]\n"), "{md}");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
    assert!(md.contains("flagged_pages: []\n"), "{md}");
    assert!(!md.contains("currency_fixes"), "{md}");
    assert!(
        md.contains("engine: \"olmocr (mlx-community/olmOCR-2-7B-1025-8bit, mlx-vlm 0.7.6)\"\n"),
        "{md}"
    );
    let reqs = e.requests();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(reqs[0]["prompt"], ocr::olmocr::PROMPT);
    assert_eq!(reqs[0]["text_first"], true);
    assert_eq!(reqs[0]["max_tokens"], 4096);
    assert_eq!(longest(&reqs[0]), 1288);
    assert!(reqs[0]["repetition_penalty"].is_null());
}

#[test]
fn columns_are_not_split_up_front() {
    // Measured: full page is at 100% line recall on narrow_unequal and two_column, so the plan
    // is one full-page request even where the side-pass finds columns.
    let e = env(&["narrow_unequal_seed2.pdf"]);
    e.serve(&[]);
    e.scan("narrow_unequal_seed2.pdf", &[], &[]).success();
    let md = e.markdown("narrow_unequal_seed2.pdf");
    assert!(md.contains("column_split_pages: []\n"), "{md}");
    let reqs = e.requests();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(longest(&reqs[0]), 1288);
}

#[test]
fn dpi_is_ignored_with_a_note() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &["--dpi", "100"], &[])
        .success()
        .stderr(predicate::str::contains("--dpi is ignored for olmocr"));
    let reqs = e.requests();
    assert_eq!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (Some(995), Some(1288))
    );
}

#[test]
fn transcripts_point_to_paddle() {
    // Measured 0/25 line numbers kept on a full-size transcript page.
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &[], &[])
        .success()
        .stderr(predicate::str::contains(
            "olmocr drops line numbers on transcripts — use --model paddleocr-vl",
        ));
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("transcript_pages: [1]\n"), "{md}");
}

#[test]
fn reply_without_front_matter_is_kept() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "nofront")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fake olmocr text 995x1288"), "{md}");
    assert!(md.contains("flagged_pages: []\n"), "{md}");
    assert_eq!(e.requests().len(), 1);
}

#[test]
fn long_repeating_table_is_not_a_loop() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "longtable")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let md = e.markdown("transcript_full.pdf");
    assert_eq!(
        md.matches("| N/A | $0.00 | Not applicable |").count(),
        80,
        "{md}"
    );
    assert!(
        md.contains("fallback_pages: []\n") && md.contains("flagged_pages: []\n"),
        "{md}"
    );
    assert_eq!(e.requests().len(), 1, "no retry");
}

#[test]
fn text_layer_is_opt_in() {
    let e = env(&["legal.pdf"]);
    e.serve(&[]);
    e.scan("legal.pdf", &["--use-text-layer", "always"], &[])
        .success();
    assert!(e.markdown("legal.pdf").contains("text_layer_pages: [1]\n"));
    assert!(e.requests().is_empty());
}

// ---------------------------------------------------------------- rotation

#[test]
fn invalid_rotation_rerenders_the_turned_page_and_retries_once() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "rotate"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("legal_scan.pdf", &["--verbose"], &[])
        .success()
        .stderr(predicate::str::contains("turned 90"));
    let reqs = e.requests();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    // A quarter turn swaps the sides; both are 1288 px on the longest.
    assert_eq!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (Some(995), Some(1288))
    );
    assert_eq!(
        (reqs[1]["w"].as_u64(), reqs[1]["h"].as_u64()),
        (Some(1288), Some(995))
    );
    assert!(reqs.iter().all(|r| r["text_first"] == true));
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("fake olmocr text 1288x995"), "{md}");
    assert!(md.contains("flagged_pages: []\n"), "{md}");
}

#[test]
fn rotation_that_stays_invalid_is_kept_and_flagged() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "rotate")]);
    e.scan("legal_scan.pdf", &[], &[]).code(1);
    let reqs = e.requests();
    assert_eq!(reqs.len(), 2, "one retry only: {reqs:?}");
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("fake olmocr text"), "text is kept: {md}");
    assert!(
        md.contains("flagged_pages: [1]\n") && md.contains("fallback_pages: []\n"),
        "{md}"
    );
}

// ---------------------------------------------------------- loops and limits

#[test]
fn loop_is_retried_with_a_penalty_then_falls_back_to_apple() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("repetition loop"));
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fallback_pages: [1]\n"), "{md}");
    assert!(md.contains("flagged_pages: [1]\n"), "{md}");
    assert!(!md.contains("the same line again"), "{md}");
    let reqs = e.requests();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert!(reqs[0]["repetition_penalty"].is_null());
    assert_eq!(reqs[1]["repetition_penalty"], 1.15);
}

#[test]
fn loop_that_clears_on_retry_is_not_a_fallback() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let md = e.markdown("transcript_full.pdf");
    assert!(
        md.contains("fallback_pages: []\n") && md.contains("fake olmocr text"),
        "{md}"
    );
    assert_eq!(e.requests().len(), 2);
}

#[test]
fn looping_page_with_regions_is_retried_as_crops_at_1288_px() {
    // signature_block.pdf: one full-page request, but two regions the side-pass can crop.
    let e = env(&["signature_block.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("signature_block.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert!(
        reqs.iter().all(|r| longest(r) == 1288),
        "every image, crops included, is 1288 px: {reqs:?}"
    );
    assert!(
        reqs.iter().all(|r| r["repetition_penalty"].is_null()),
        "{reqs:?}"
    );
    assert_ne!(
        (reqs[0]["w"].as_u64(), reqs[0]["h"].as_u64()),
        (reqs[1]["w"].as_u64(), reqs[1]["h"].as_u64()),
        "crops differ from the full page"
    );
    let md = e.markdown("signature_block.pdf");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
    assert_eq!(md.matches("fake olmocr text").count(), 2, "{md}");
}

#[test]
fn looping_crops_are_retried_with_a_penalty_then_fall_back() {
    let e = env(&["signature_block.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("signature_block.pdf", &[], &[]).code(1);
    let reqs = e.requests();
    // Full page, first crop, first crop again with a penalty; then Apple.
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert_eq!(reqs[2]["repetition_penalty"], 1.15);
    assert!(
        e.markdown("signature_block.pdf")
            .contains("fallback_pages: [1]\n")
    );
}

#[test]
fn truncation_is_retried_with_8000_tokens() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "length"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let reqs = e.requests();
    let tokens: Vec<u64> = reqs
        .iter()
        .map(|r| r["max_tokens"].as_u64().unwrap())
        .collect();
    assert_eq!(tokens, [4096, 8000], "{reqs:?}");
}

#[test]
fn persistent_truncation_falls_back_and_flags() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "length")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("truncated"));
    let md = e.markdown("transcript_full.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    assert_eq!(e.requests().len(), 2);
}

#[test]
fn empty_output_is_retried_once() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "empty"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    assert_eq!(e.requests().len(), 2);
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fake olmocr text")
    );
}

#[test]
fn persistent_empty_output_falls_back_and_flags() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "empty")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("empty model output"));
    let md = e.markdown("transcript_full.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    assert_eq!(e.requests().len(), 2, "one try and one retry");
}

#[test]
fn timeout_restarts_the_server_then_falls_back() {
    let e = env(&["transcript_full.pdf"]);
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
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fallback_pages: [1]\n")
    );
    assert_eq!(e.requests().len(), 2, "one try and one retry");
}

#[test]
fn timeout_retry_can_succeed() {
    let e = env(&["transcript_full.pdf"]);
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
    assert!(
        e.markdown("transcript_full.pdf")
            .contains("fake olmocr text")
    );
}

#[test]
fn server_errors_are_tried_three_times_then_fall_back() {
    let e = env(&["transcript_full.pdf"]);
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
fn stopped_server_exits_3() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.base_cmd().args(["server", "stop"]).assert().success();
    e.scan("legal_scan.pdf", &[], &[])
        .code(3)
        .stderr(predicate::str::contains("ocr server start --model olmocr"));
}

#[test]
fn jobs_do_not_change_the_output() {
    let e = env(&["three_pages.pdf"]);
    e.serve(&[]);
    let run = |extra: &[&str]| {
        e.scan("three_pages.pdf", &[&["--overwrite"], extra].concat(), &[])
            .success();
        without_generated(&e.markdown("three_pages.pdf"))
    };
    let default = run(&[]);
    assert_eq!(run(&["--jobs", "3"]), default);
    assert!(default.contains("<!-- page 3"), "{default}");
}

// ------------------------------------------------------------- real model

/// Whether olmOCR keeps transcript line numbers (docs/spike.md: it does not on a full-size page,
/// 0/25); the test asserts the scores only for an engine that reaches 95%, and otherwise that the
/// hint to use paddleocr-vl is printed.
const TRANSCRIPTS_OK: bool = false;

/// Real uv venv + weights from the HF cache + a real server. Run with
/// `OCR_REAL_MODELS=1 cargo test --release real_olmocr -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_olmocr_engine() {
    if std::env::var("OCR_REAL_MODELS").as_deref() != Ok("1") {
        eprintln!("OCR_REAL_MODELS!=1; skipping");
        return;
    }
    let e = env(&[
        "legal_scan.pdf",
        "legal_scan_rot90.png",
        "transcript_full.pdf",
        "four_up.pdf",
        "narrow_unequal_seed2.pdf",
    ]);
    fs::copy(
        Path::new(FIXTURES).join("narrow_unequal_seed2.expected.json"),
        e.path("expected.json"),
    )
    .unwrap();
    Env::real(e.base_cmd())
        .args(["server", "setup", "--model", "olmocr"])
        .assert()
        .success();
    let port = e.port.to_string();
    Env::real(e.base_cmd())
        .args(["server", "start", "--model", "olmocr", "--port", &port])
        .assert()
        .success();

    let failures = std::cell::RefCell::new(Vec::<String>::new());
    let scan_out = |input: &str, label: &str| -> (String, String) {
        let t = Instant::now();
        let out = Env::real(e.scan_cmd(input, &["--overwrite", "--verbose"], &[]))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        println!(
            "[{label}] {input}: exit {:?} in {:.1}s\n{stderr}",
            out.status.code(),
            t.elapsed().as_secs_f64(),
        );
        if out.status.code() != Some(0) {
            failures
                .borrow_mut()
                .push(format!("{label}: exit {:?}", out.status.code()));
        }
        (e.markdown(input), stderr)
    };
    let scan = |input: &str, label: &str| scan_out(input, label).0;
    // `bates`: the Bates number comes from the Apple side-pass (olmOCR drops the footer), which
    // reads nothing useful on a sideways page, so the rotated fixture is checked without it.
    let tokens = |md: &str, what: &str, bates: bool| {
        for t in [
            "3:26-cv-04417",
            "$48,250.00",
            "$51,900.00",
            "$22,375.50",
            "$122,525.50",
            "SMITH000123",
        ] {
            if (bates || t != "SMITH000123") && !md.contains(t) {
                failures.borrow_mut().push(format!("{what} missing {t}"));
            }
        }
    };

    // legal_scan: tokens exact (the Bates number from the side-pass); twice, byte-identical.
    let first = scan("legal_scan.pdf", "legal #1");
    tokens(&first, "legal_scan", true);
    let second = scan("legal_scan.pdf", "legal #2");
    if without_generated(&first) != without_generated(&second) {
        failures
            .borrow_mut()
            .push("legal_scan differs between runs".into());
    }
    println!("--- legal_scan.md ---\n{first}");

    // A page rotated 90 degrees clockwise: the engine turns it back and finds the tokens.
    let (md, stderr) = scan_out("legal_scan_rot90.png", "rot90");
    tokens(&md, "legal_scan_rot90", false);
    if !stderr.contains("turned 90") {
        failures
            .borrow_mut()
            .push("rot90: no rotation retry happened".into());
    }
    println!("--- legal_scan_rot90.md ---\n{md}");

    // Transcripts: record the scores; assert them only for an engine that reaches 95%.
    let (md, stderr) = scan_out("transcript_full.pdf", "transcript");
    let lines =
        regex::Regex::new(r"(?m)^\s*(\d+)\s+([QA])\.\s+Line (\d+) of the testimony").unwrap();
    let full = (1..=25)
        .filter(|n| {
            let who = if n % 2 == 1 { "Q" } else { "A" };
            lines
                .captures_iter(&md)
                .any(|c| c[1] == n.to_string() && &c[2] == who && c[3] == n.to_string())
        })
        .count();
    let md4 = scan("four_up.pdf", "four_up");
    let four = (1..=4)
        .flat_map(|k| (1..=25).map(move |n| (k, n)))
        .filter(|(k, n)| {
            regex::Regex::new(&format!(
                r"\b{n}\s+Q\.\s+Question {k}\.{n} about the meeting"
            ))
            .unwrap()
            .is_match(&md4)
        })
        .count();
    println!("transcript_full: {full}/25 exact, four_up: {four}/100 exact");
    if TRANSCRIPTS_OK {
        if full != 25 || four != 100 {
            failures
                .borrow_mut()
                .push(format!("transcripts {full}/25, {four}/100"));
        }
    } else if (full + four) as f64 / 125.0 >= 0.95 {
        failures
            .borrow_mut()
            .push("transcripts now score >= 95%: set TRANSCRIPTS_OK".into());
    } else if !stderr.contains("use --model paddleocr-vl") {
        failures
            .borrow_mut()
            .push("no paddleocr-vl hint on a transcript".into());
    }

    // Narrow unequal columns: the full-page decision keeps >= 95% of the lines, in order.
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
    if (found as f64) < 0.95 * expected.len() as f64 || !md.contains("column_split_pages: []") {
        failures.borrow_mut().push(format!(
            "narrow {found}/{} found; full page must stay the plan",
            expected.len()
        ));
    }

    Env::real(e.base_cmd())
        .args(["server", "stop"])
        .assert()
        .success();
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "real-model failures: {failures:#?}");
}
