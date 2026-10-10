//! dots engine tests. The fake server (tests/fake_server.py) stands in for the model; the Apple
//! side-pass is real Vision on this Mac. The `#[ignore]`d test at the end drives the real model
//! (OCR_REAL_MODELS=1).

// Needs Apple Vision (PLAN §9).
#![cfg(target_os = "macos")]

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Env, FIXTURES, without_generated};
use predicates::prelude::*;
use serde_json::Value;

fn env(fixtures: &[&str]) -> Env {
    Env::new("dots-ocr", fixtures)
}

/// Which prompt each request carried: `layout`, `ocr` or `?`.
fn kinds(reqs: &[Value]) -> Vec<&'static str> {
    reqs.iter()
        .map(|r| match r["prompt"].as_str().unwrap() {
            p if p == ocr::dots::PROMPT_LAYOUT => "layout",
            p if p == ocr::dots::PROMPT_OCR => "ocr",
            _ => "?",
        })
        .collect()
}

/// dots never sends a `repetition_penalty`: it corrupts the JSON structure.
fn no_penalty(reqs: &[Value]) {
    assert!(
        reqs.iter().all(|r| r["repetition_penalty"].is_null()),
        "{reqs:?}"
    );
}

#[test]
fn layout_json_becomes_markdown() {
    // The plain request comes first; make it loop so the layout prompt is reached.
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_PLAIN_MODE", "loop")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let md = e.markdown("legal_scan.pdf");
    for want in [
        "FAKE RUNNING HEADER",
        "# FAKE TITLE 850x1100",
        "## Section A",
        "fake dots text 850x1100",
        "first item\n\nsecond item",
        "| Invoice | Date | Amount |\n| --- | --- | --- |\n| INV-1001 | 2026-01-05 | $48,250.00 |",
        "$$E = mc^{2}$$",
        "[Figure]",
        "Figure 1. A fake picture",
        "1 A fake footnote.",
        "FAKE RUNNING FOOTER",
        "<!-- page 1 · Bates SMITH000123 -->",
        "table_pages: [1]\n",
        "fallback_pages: []\n",
        "flagged_pages: []\n",
    ] {
        assert!(md.contains(want), "missing {want:?}\n{md}");
    }
    assert!(!md.contains("\"bbox\"") && !md.contains("<table>"), "{md}");
    assert!(
        md.contains("engine: \"dots-ocr (mlx-community/dots.mocr-8bit, mlx-vlm 0.7.6)\"\n"),
        "{md}"
    );
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout"], "{reqs:?}");
    assert_eq!(reqs[1]["text_first"], false, "image first");
    assert_eq!(reqs[1]["max_tokens"], 8192);
    assert_eq!(
        (reqs[1]["w"].as_u64(), reqs[1]["h"].as_u64()),
        (Some(850), Some(1100)),
        "100 DPI (the dots default)"
    );
    no_penalty(&reqs);
}

#[test]
fn clean_plain_text_is_used_directly_and_no_layout_request_is_sent() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr"], "{reqs:?}");
    assert_eq!(reqs[0]["text_first"], false, "image first");
    assert_eq!(reqs[0]["max_tokens"], 8192);
    no_penalty(&reqs);
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("fake dots plain text 850x1100"), "{md}");
    assert!(!md.contains("FAKE TITLE"), "{md}");
    assert!(
        md.contains("fallback_pages: []\n") && md.contains("flagged_pages: []\n"),
        "{md}"
    );
}

#[test]
fn dpi_override_changes_the_render() {
    let e = env(&["transcript_full.pdf"]);
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
fn columns_are_not_split_up_front_and_unverified_transcripts_get_the_verify_hint() {
    let e = env(&["narrow_unequal_seed2.pdf", "transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("narrow_unequal_seed2.pdf", &[], &[]).success();
    assert!(
        e.markdown("narrow_unequal_seed2.pdf")
            .contains("column_split_pages: []\n")
    );
    assert_eq!(e.requests().len(), 1);
    // The fake reply does not match the page geometry: the page stays unverified and says so.
    e.scan("transcript_full.pdf", &[], &[])
        .success()
        .stderr(predicate::str::contains(
            "transcript_full.pdf: transcript pages 1 could not be re-anchored to page geometry (line_numbers.unverified); verify page:line citations against the PDF",
        ))
        .stderr(predicate::str::contains("paddleocr-vl").not());
}

#[test]
fn long_repeating_table_is_not_a_loop() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "longtable"), ("FAKE_PLAIN_MODE", "loop")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let md = e.markdown("legal_scan.pdf");
    assert_eq!(
        md.matches("| N/A | $0.00 | Not applicable |").count(),
        80,
        "{md}"
    );
    assert!(
        md.contains("flagged_pages: []\n") && md.contains("fallback_pages: []\n"),
        "{md}"
    );
    let reqs = e.requests();
    assert_eq!(
        kinds(&reqs),
        ["ocr", "layout"],
        "no retry of the layout request: {reqs:?}"
    );
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

// ------------------------------------------------------------- bad JSON

#[test]
fn truncated_json_is_retried_then_kept_and_flagged() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "truncated"), ("FAKE_PLAIN_MODE", "loop")]);
    e.scan("legal_scan.pdf", &[], &[]).code(1);
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
    let md = e.markdown("legal_scan.pdf");
    // The complete elements survive, the cut-off one does not.
    assert!(
        md.contains("FAKE RUNNING HEADER") && md.contains("# FAKE TITLE"),
        "{md}"
    );
    assert!(!md.contains("FAKE RUNNING FOOTER"), "{md}");
    assert!(md.contains("flagged_pages: [1]\n"), "{md}");
    assert!(
        md.contains("fallback_pages: []\n"),
        "kept, not Apple text: {md}"
    );
    no_penalty(&reqs);
}

#[test]
fn truncated_json_that_clears_on_retry_is_not_flagged() {
    let e = env(&["legal_scan.pdf"]);
    // Request 1 (plain) loops, request 2 (layout) is cut off, request 3 (the retry) is whole.
    e.serve(&[
        ("FAKE_MODE", "truncated"),
        ("FAKE_PLAIN_MODE", "loop"),
        ("FAKE_FAIL_FIRST", "2"),
    ]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
    let md = e.markdown("legal_scan.pdf");
    assert!(
        md.contains("FAKE RUNNING FOOTER") && md.contains("flagged_pages: []\n"),
        "{md}"
    );
}

// ---------------------------------------------------- loops and truncation

#[test]
fn loop_in_plain_layout_and_first_crop_goes_to_apple() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("legal_scan.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("repetition loop"));
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
    no_penalty(&reqs);
    let md = e.markdown("legal_scan.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    assert!(!md.contains("the same line again"), "{md}");
}

#[test]
fn plain_text_that_loops_goes_to_layout_and_is_not_a_fallback() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout"], "{reqs:?}");
    let md = e.markdown("legal_scan.pdf");
    assert!(md.contains("# FAKE TITLE 850x1100"), "{md}");
    assert!(!md.contains("the same line again"), "{md}");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
}

#[test]
fn looping_page_with_regions_is_retried_as_crops() {
    // signature_block.pdf: one full-page request, but two regions the side-pass can crop.
    let e = env(&["signature_block.pdf"]);
    // The plain request and the full-page layout request loop; the two crops are fine.
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "2")]);
    e.scan("signature_block.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(
        kinds(&reqs),
        ["ocr", "layout", "layout", "layout"],
        "{reqs:?}"
    );
    assert!(
        reqs[2..].iter().all(|r| r["h"].as_u64().unwrap() < 1100),
        "{reqs:?}"
    );
    no_penalty(&reqs);
    let md = e.markdown("signature_block.pdf");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
    assert_eq!(md.matches("# FAKE TITLE").count(), 2, "{md}");
}

#[test]
fn looping_crop_ends_the_crop_plan_and_falls_back_to_apple() {
    // Plain and full-page layout loop, and so does the first crop: no more crops, no plain retry.
    let e = env(&["signature_block.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "3")]);
    e.scan("signature_block.pdf", &[], &[]).code(1);
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
    assert!(reqs[2]["h"].as_u64().unwrap() < 1100, "a crop: {reqs:?}");
    no_penalty(&reqs);
    let md = e.markdown("signature_block.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
}

#[test]
fn everything_looping_falls_back_to_apple() {
    let e = env(&["signature_block.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("signature_block.pdf", &[], &[]).code(1);
    let reqs = e.requests();
    // Plain, the full-page layout request, then the first crop.
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
    no_penalty(&reqs);
    let md = e.markdown("signature_block.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    assert!(!md.contains("the same line again"), "{md}");
}

#[test]
fn plain_truncation_goes_to_layout_without_a_bigger_budget() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "length"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout"], "{reqs:?}");
    assert!(reqs.iter().all(|r| r["max_tokens"] == 8192), "{reqs:?}");
    assert!(
        e.markdown("legal_scan.pdf")
            .contains("# FAKE TITLE 850x1100")
    );
}

#[test]
fn persistent_truncation_falls_back_and_flags() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "length")]);
    e.scan("legal_scan.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("truncated"));
    let md = e.markdown("legal_scan.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "layout", "layout"], "{reqs:?}");
}

#[test]
fn empty_output_is_retried_once() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "empty"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("legal_scan.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "ocr"], "{reqs:?}");
    assert!(
        e.markdown("legal_scan.pdf")
            .contains("fake dots plain text")
    );
}

#[test]
fn persistent_empty_output_falls_back_and_flags() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "empty")]);
    e.scan("legal_scan.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("empty model output"));
    let md = e.markdown("legal_scan.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    let reqs = e.requests();
    assert_eq!(
        kinds(&reqs),
        ["ocr", "ocr", "layout", "layout"],
        "one try and one retry for each prompt: {reqs:?}"
    );
}

#[test]
fn timeout_restarts_the_server_then_falls_back() {
    let e = env(&["legal_scan.pdf"]);
    let env = [("FAKE_MODE", "sleep"), ("FAKE_SLEEP_SECS", "20")];
    e.serve(&env);
    let before = e.server_pid();
    e.scan(
        "legal_scan.pdf",
        &[],
        &[("OCR_REQUEST_TIMEOUT_SECS", "2"), env[0], env[1]],
    )
    .code(1)
    .stderr(predicate::str::contains("restarting the model server"));
    assert_ne!(e.server_pid(), before, "server was not restarted");
    assert!(
        e.markdown("legal_scan.pdf")
            .contains("fallback_pages: [1]\n")
    );
    let reqs = e.requests();
    assert_eq!(
        kinds(&reqs),
        ["ocr", "ocr"],
        "a timeout after its restart is a server failure: no layout request: {reqs:?}"
    );
}

#[test]
fn timeout_retry_can_succeed() {
    let e = env(&["legal_scan.pdf"]);
    let env = [
        ("FAKE_MODE", "sleep"),
        ("FAKE_SLEEP_SECS", "20"),
        ("FAKE_FAIL_FIRST", "1"),
    ];
    e.serve(&env);
    let before = e.server_pid();
    e.scan(
        "legal_scan.pdf",
        &[],
        &[("OCR_REQUEST_TIMEOUT_SECS", "2"), env[0], env[1], env[2]],
    )
    .success();
    assert_ne!(e.server_pid(), before);
    assert!(
        e.markdown("legal_scan.pdf")
            .contains("fake dots plain text")
    );
}

#[test]
fn server_errors_are_tried_three_times_then_fall_back() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[("FAKE_MODE", "error5xx")]);
    let t = Instant::now();
    e.scan("legal_scan.pdf", &[], &[]).code(1);
    assert!(t.elapsed() >= Duration::from_millis(1500), "no backoff");
    // Three tries for the plain request, then three for the layout request.
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["ocr", "ocr", "ocr"], "{reqs:?}");
    assert!(
        e.markdown("legal_scan.pdf")
            .contains("fallback_pages: [1]\n")
    );
}

// ------------------------------------------------- transcript pages: layout first

#[test]
fn transcript_page_sends_only_the_layout_prompt() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["layout"], "{reqs:?}");
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("# FAKE TITLE 850x1100"), "{md}");
    assert!(md.contains("fallback_pages: []\n"), "{md}");
}

#[test]
fn transcript_layout_loop_without_crops_goes_to_plain_text() {
    // The layout request loops (1), the plain one is fine (2): not a fallback.
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop"), ("FAKE_FAIL_FIRST", "1")]);
    e.scan("transcript_full.pdf", &[], &[]).success();
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["layout", "ocr"], "{reqs:?}");
    no_penalty(&reqs);
    let md = e.markdown("transcript_full.pdf");
    assert!(md.contains("fake dots plain text 850x1100"), "{md}");
    assert!(
        md.contains("fallback_pages: []\n") && md.contains("flagged_pages: []\n"),
        "{md}"
    );
}

#[test]
fn transcript_everything_looping_falls_back_to_apple() {
    let e = env(&["transcript_full.pdf"]);
    e.serve(&[("FAKE_MODE", "loop")]);
    e.scan("transcript_full.pdf", &[], &[])
        .code(1)
        .stderr(predicate::str::contains("repetition loop"));
    let reqs = e.requests();
    assert_eq!(kinds(&reqs), ["layout", "ocr"], "{reqs:?}");
    let md = e.markdown("transcript_full.pdf");
    assert!(
        md.contains("fallback_pages: [1]\n") && md.contains("flagged_pages: [1]\n"),
        "{md}"
    );
    assert!(!md.contains("the same line again"), "{md}");
}

#[test]
fn stopped_server_exits_3() {
    let e = env(&["legal_scan.pdf"]);
    e.serve(&[]);
    e.base_cmd().args(["server", "stop"]).assert().success();
    e.scan("legal_scan.pdf", &[], &[])
        .code(3)
        .stderr(predicate::str::contains(
            "ocr server start --model dots-ocr",
        ));
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

/// Real uv venv + weights from the HF cache + a real server. Run with
/// `OCR_REAL_MODELS=1 cargo test --release real_dots -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_dots_engine() {
    if std::env::var("OCR_REAL_MODELS").as_deref() != Ok("1") {
        eprintln!("OCR_REAL_MODELS!=1; skipping");
        return;
    }
    let e = env(&[
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
    Env::real(e.base_cmd())
        .args(["server", "setup", "--model", "dots-ocr"])
        .assert()
        .success();
    let port = e.port.to_string();
    Env::real(e.base_cmd())
        .args(["server", "start", "--model", "dots-ocr", "--port", &port])
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

    // Transcripts: record the scores; assert them for an engine that reaches 95%.
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
    if full != 25 || four != 100 {
        failures
            .borrow_mut()
            .push(format!("transcripts {full}/25, {four}/100"));
    }
    // The verify hint appears exactly when the front matter lists an unverified page.
    let unverified = !md.contains("unverified: []");
    if unverified != stderr.contains("could not be re-anchored to page geometry") {
        failures
            .borrow_mut()
            .push("transcript hint does not match line_numbers.unverified".into());
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
