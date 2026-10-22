//! Ticket 02: the indra-ocr 1.1.2 eval write-up and the OXR deferral stay in the docs.
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn eval_has_indra_112_synthetic_table() {
    let eval = read("docs/eval.md");
    let marker = "### indra-ocr 1.1.2 (200 DPI, seeds 10-12, Linux CPU)";
    let start = eval.find(marker).expect("1.1.2 section");
    let end = eval[start..]
        .find("### Transcripts")
        .map(|i| start + i)
        .expect("section ends before Transcripts");
    let section = &eval[start..end];
    assert!(section.contains("Linux"), "the section must name the platform");
    assert!(section.contains("seeds 10-12"));
    assert!(section.contains("| Family | Pages | CER |"));
    for family in ["pleading", "contract", "transcript4up", "fax", "table"] {
        assert!(section.contains(family), "{family} missing from the 1.1.2 table");
    }
}

#[test]
fn readme_holdout_row_is_labelled() {
    let readme = read("README.md");
    let row = readme
        .lines()
        .find(|l| l.starts_with("| indra-ocr 1.1.2"))
        .expect("holdout row");
    assert!(row.contains("1.1.2"), "{row}");
    assert!(row.contains("Linux"), "{row}");
    assert!(row.contains("2026-10-22"), "{row}");
}

#[test]
fn current_indra_docs_do_not_keep_031() {
    // History (PLAN.md, docs/eval.md, docs/spike.md) may still name 0.3.1.
    for rel in ["README.md", "skills/ocr/SKILL.md", "scripts/third_party_notices.py"] {
        let text = read(rel);
        assert!(!text.contains("0.3.1"), "{rel} still says 0.3.1");
        assert!(!text.contains("18 MB"), "{rel} still says 18 MB");
        assert!(
            !text.split_whitespace().any(|w| w == "seven" || w.starts_with("seven")),
            "{rel} still says seven"
        );
    }
}

#[test]
fn plan_records_upgrade_and_oxr_deferral() {
    let plan = read("PLAN.md");
    let upgrade = plan
        .split("**indra-ocr 1.1.2.**")
        .nth(1)
        .expect("upgrade entry");
    let upgrade = upgrade.split("**OXR-1.0 deferred**").next().unwrap();
    assert!(upgrade.contains("a1283c4"), "pin");
    assert!(upgrade.contains("Twelve files"), "file count");
    assert!(upgrade.contains("[Figure]"));
    assert!(upgrade.contains("no tag fallback"));
    assert!(upgrade.contains("sha256 stamp"), "freshness gate");
    let oxr = plan.split("**OXR-1.0 deferred**").nth(1).expect("oxr entry");
    assert!(oxr.contains("not a laptop fit"));
    assert!(oxr.contains("llama.cpp"));
    assert!(oxr.contains("24 GB"));
    assert!(oxr.contains("mlx-vlm"));
    assert!(oxr.contains("Revisit when llama.cpp or a released mlx-vlm supports it"));
}
