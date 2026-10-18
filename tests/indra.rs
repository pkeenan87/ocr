//! indra-ocr engine tests. tests/fake_indra.py stands in for the helper process (no models, no
//! venv, no network); `OCR_FAKE_INSTALL` makes setup write only the config and the helper.
//! Scans need a pdfium library to count pages (`OCR_PDFIUM_LIB`, as in CI).

mod common;

use std::fs;

use common::Env;
use predicates::prelude::*;
use serde_json::Value;

#[cfg(not(windows))]
const FAKE_INDRA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_indra.py");
#[cfg(windows)]
const FAKE_INDRA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_indra.cmd");

const THREE: &str = "three_pages.pdf";

fn env() -> Env {
    Env::new("indra-ocr", &[THREE])
}

fn setup(e: &Env) {
    e.base_cmd()
        .args(["server", "setup", "--model", "indra-ocr"])
        .assert()
        .success();
}

fn scan(e: &Env, extra: &[&str], env: &[(&str, &str)]) -> assert_cmd::assert::Assert {
    let log = e.path("indra.log");
    let log = log.to_str().unwrap();
    let mut all = vec![("OCR_INDRA_CMD", FAKE_INDRA), ("INDRA_FAKE_LOG", log)];
    all.extend_from_slice(env);
    e.scan(THREE, extra, &all)
}

/// The argv of every helper run, oldest first.
fn runs(e: &Env) -> Vec<Vec<String>> {
    fs::read_to_string(e.path("indra.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            assert_eq!(v["offline"], "1", "the helper must run offline");
            v["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

fn arg<'a>(argv: &'a [String], name: &str) -> &'a str {
    let i = argv.iter().position(|a| a == name).unwrap();
    &argv[i + 1]
}

#[test]
fn setup_records_the_engine_without_a_runtime() {
    let e = env();
    e.base_cmd()
        .args(["server", "setup", "--model", "indra-ocr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("indra-ocr ready"));
    let cfg = fs::read_to_string(e.config.path().join("config.toml")).unwrap();
    assert!(cfg.contains("[engines.indra-ocr]"), "{cfg}");
    assert!(cfg.contains("installed = true"), "{cfg}");
    assert!(cfg.contains("version = \"0.3.1\""), "{cfg}");
    assert!(cfg.contains("repo = \"datalab-to/indra-ocr\""), "{cfg}");
    assert!(!cfg.contains("quant"), "{cfg}");
    assert!(!cfg.contains("[runtime]"), "{cfg}");
    let helper = fs::read_to_string(e.state.path().join("indra/helper.py")).unwrap();
    assert!(helper.contains("HF_HUB_OFFLINE"), "{helper}");
}

#[test]
fn setup_refuses_a_quant() {
    let e = env();
    e.base_cmd()
        .args(["server", "setup", "--model", "indra-ocr", "--quant", "4bit"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no --quant"));
}

#[test]
fn scan_without_setup_points_at_setup() {
    let e = env();
    scan(&e, &[], &[]).code(2).stderr(predicate::str::contains(
        "ocr server setup --model indra-ocr",
    ));
    assert!(runs(&e).is_empty());
    // Set up for another engine, not this one.
    e.base_cmd()
        .args(["server", "setup", "--model", "paddleocr-vl"])
        .assert()
        .success();
    scan(&e, &[], &[]).code(2).stderr(predicate::str::contains(
        "ocr server setup --model indra-ocr",
    ));
    assert!(runs(&e).is_empty());
}

#[test]
fn server_start_says_there_is_no_server() {
    let e = env();
    e.base_cmd()
        .args(["server", "start", "--model", "indra-ocr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("runs without a server"));
    e.base_cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no server running"));
}

#[test]
fn a_server_start_without_a_model_ignores_indra() {
    let e = env();
    setup(&e);
    e.base_cmd()
        .args(["server", "start"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no model engine is installed"));
}

#[test]
fn the_filing_stamp_header_stays_and_is_never_verified() {
    let e = env();
    setup(&e);
    scan(
        &e,
        &["--verify-json", "--verify-fields", "all"],
        &[("INDRA_FAKE_STAMP", "1")],
    )
    .code(predicate::in_iter([0, 1]));
    let md = e.markdown(THREE);
    for n in 1..=3 {
        assert!(
            md.contains(&format!(
                "Case 1:19-cv-07208-ST Document 31 Filed 02/24/21 Page {n} of 3\n\nFake indra body of page {n}."
            )),
            "{md}"
        );
    }
    // The stamp's page count is not a Bates number; the footer's still is.
    assert!(md.contains("<!-- page 1 · Bates SMITH000123 -->"), "{md}");
    assert!(md.contains("<!-- page 2 -->"), "{md}");

    let report: Value =
        serde_json::from_str(&fs::read_to_string(e.path("three_pages.verify.json")).unwrap())
            .unwrap();
    let stamp_fields: Vec<&Value> = report["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["fields"].as_array().unwrap())
        .filter(|f| matches!(f["kind"].as_str(), Some("docket" | "date")))
        .collect();
    assert_eq!(stamp_fields.len(), 6, "{report:#}");
    for f in stamp_fields {
        assert_ne!(f["status"], "verified", "{f:#}");
    }
}

#[test]
fn pages_carry_the_footer_bates_and_no_furniture() {
    let e = env();
    setup(&e);
    scan(&e, &[], &[]).success();
    let md = e.markdown(THREE);
    assert!(md.contains("<!-- page 1 · Bates SMITH000123 -->"), "{md}");
    assert!(md.contains("<!-- page 2 -->"), "{md}");
    assert!(md.contains("<!-- page 3 · Bates SMITH000125 -->"), "{md}");
    assert!(md.contains("Fake indra body of page 1."), "{md}");
    assert!(md.contains("Fake indra body of page 3."), "{md}");
    assert!(
        !md.contains("COMPLAINT - Page") && !md.contains("SMITH000123\n"),
        "{md}"
    );
    // The HTML table became a GFM table.
    assert!(md.contains("| Item | Qty |"), "{md}");
    assert!(!md.contains("<table>"), "{md}");
    assert!(
        md.contains("engine: \"indra-ocr (datalab-to/indra-ocr@ed345c7, indra-ocr 0.3.1)\"\n"),
        "{md}"
    );
    assert!(md.contains("table_pages: [2]\n"), "{md}");

    let runs = runs(&e);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(arg(&runs[0], "--pages"), "1,2,3");
    assert_eq!(arg(&runs[0], "--dpi"), "200");
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(arg(&runs[0], "--threads"), cores.to_string());
    assert!(
        arg(&runs[0], "--model-dir").ends_with("datalab-to/indra-ocr")
            || arg(&runs[0], "--model-dir").ends_with("datalab-to\\indra-ocr")
    );
}

#[test]
fn pages_dpi_and_jobs_reach_the_helper() {
    let e = env();
    setup(&e);
    scan(&e, &["--pages", "2-3", "--dpi", "150", "--jobs", "3"], &[]).success();
    let runs = runs(&e);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(arg(&runs[0], "--pages"), "2,3");
    assert_eq!(arg(&runs[0], "--dpi"), "150");
    assert_eq!(arg(&runs[0], "--threads"), "3");
}

#[test]
fn a_failed_page_is_retried_and_finished_pages_are_not_redone() {
    let e = env();
    setup(&e);
    scan(&e, &[], &[("INDRA_FAKE_FAIL_PAGE", "2")])
        .code(1)
        .stderr(predicate::str::contains(
            "1 of 3 pages failed OCR (pages [2])",
        ));
    scan(&e, &[], &[]).success();
    let runs = runs(&e);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(arg(&runs[0], "--pages"), "1,2,3");
    // Pages 1 and 3 were checkpointed; only the failed page goes to the helper again.
    assert_eq!(arg(&runs[1], "--pages"), "2");
    let md = e.markdown(THREE);
    assert!(md.contains("Fake indra body of page 2."), "{md}");
    assert!(md.contains("<!-- page 3 · Bates SMITH000125 -->"), "{md}");
}

#[test]
fn a_helper_that_dies_fails_the_pages_it_did_not_deliver() {
    let e = env();
    setup(&e);
    scan(&e, &[], &[("INDRA_FAKE_EXIT_AFTER", "1")])
        .code(1)
        .stderr(predicate::str::contains("pages failed OCR (pages [2, 3])"))
        .stderr(predicate::str::contains("fake indra: crashed"));
    scan(&e, &[], &[]).success();
    let runs = runs(&e);
    assert_eq!(arg(&runs[1], "--pages"), "2,3");
}
