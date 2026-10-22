//! The user-facing docs name the indra-ocr version the code pins (`engines::INDRA_PIN`) and no
//! earlier one. History (PLAN.md, docs/eval.md, docs/spike.md) may name old versions.
use std::fs;
use std::path::Path;

use ocr::engines::INDRA_PIN;

/// Docs that state the version.
const VERSIONED_DOCS: [&str; 2] = ["README.md", "scripts/third_party_notices.py"];
/// Every user-facing doc that describes the install.
const CURRENT_DOCS: [&str; 3] = [
    "README.md",
    "skills/ocr/SKILL.md",
    "scripts/third_party_notices.py",
];
/// Facts of the 0.3.1 install (version, download size, file count), as patterns so spacing and
/// digit variants are caught. Only lines about indra-ocr are checked, so another engine's size or
/// a dependency version elsewhere in a doc cannot trip it.
const RETIRED: [&str; 3] = [
    r"\b0\.3\.1\b",
    r"(?i)\b18\s*MB\b",
    r"(?i)\b(seven|7)\s+(weight\s+|model\s+)?files\b",
];

fn read(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn current_docs_name_the_pinned_indra_version() {
    for rel in VERSIONED_DOCS {
        assert!(
            read(rel).contains(INDRA_PIN),
            "{rel} does not name indra-ocr {INDRA_PIN}"
        );
    }
}

#[test]
fn current_docs_drop_the_old_install_facts() {
    let retired: Vec<regex::Regex> = RETIRED
        .iter()
        .map(|r| regex::Regex::new(r).unwrap())
        .collect();
    for rel in CURRENT_DOCS {
        for (n, line) in read(rel).lines().enumerate() {
            if !line.to_lowercase().contains("indra") {
                continue;
            }
            for re in &retired {
                assert!(!re.is_match(line), "{rel}:{}: matches {re}: {line}", n + 1);
            }
        }
    }
}

#[test]
fn the_retired_patterns_catch_variants() {
    let hit = |s: &str| {
        RETIRED
            .iter()
            .any(|r| regex::Regex::new(r).unwrap().is_match(s))
    };
    for s in [
        "indra-ocr==0.3.1",
        "about 18MB",
        "18 mb",
        "the 7 files",
        "seven weight files",
    ] {
        assert!(hit(s), "{s}");
    }
    for s in [
        "indra-ocr 1.1.2",
        "about 76 MB",
        "180 MB",
        "seventeen files",
        "0.3.14",
    ] {
        assert!(!hit(s), "{s}");
    }
}
