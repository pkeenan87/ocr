//! End-to-end tests of the apple engine. These run real Apple Vision on this Mac.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::{TempDir, tempdir};

struct Env {
    config: TempDir,
    state: TempDir,
    work: TempDir,
}

impl Env {
    /// A fresh environment with the named fixtures copied into the working directory.
    fn new(fixtures: &[&str]) -> Self {
        let e = Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
        };
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures");
        for f in fixtures {
            fs::copy(src.join(f), e.path(f)).unwrap();
        }
        e
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.work.path().join(rel)
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .current_dir(self.work.path());
        c
    }

    /// Scans `input` (output `<stem>.md` in the working dir) and returns (markdown, stderr).
    fn scan(&self, input: &str, extra: &[&str]) -> (String, String) {
        let a = self
            .cmd()
            .args(["scan", "--input-file", input])
            .args(extra)
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
        let stem = input.rsplit_once('.').unwrap().0;
        (
            fs::read_to_string(self.path(&format!("{stem}.md"))).unwrap(),
            stderr,
        )
    }
}

const NEVER: [&str; 2] = ["--use-text-layer", "never"];
const EXACT: [&str; 6] = [
    "3:26-cv-04417",
    "$48,250.00",
    "$51,900.00",
    "$22,375.50",
    "$122,525.50",
    "Bates SMITH000123",
];

fn missing<'a>(md: &str, tokens: &[&'a str]) -> Vec<&'a str> {
    tokens.iter().copied().filter(|t| !md.contains(t)).collect()
}

#[test]
fn legal_pdf_vision_is_exact() {
    let e = Env::new(&["legal.pdf"]);
    let (md, _) = e.scan("legal.pdf", &NEVER);
    assert!(
        missing(&md, &EXACT).is_empty(),
        "{:?}\n{md}",
        missing(&md, &EXACT)
    );
    assert!(md.contains("| --- | --- | --- |"), "{md}");
    assert!(md.starts_with(
        "---\nsource: legal.pdf\npages: 1\nengine: apple (Vision RecognizeDocuments)\n"
    ));
    assert!(md.contains("text_layer_pages: []"), "{md}");
}

/// Vision reads `$` as `S` on the degraded scan; the currency fix restores it.
#[test]
fn legal_scan_pdf_is_exact() {
    let e = Env::new(&["legal_scan.pdf"]);
    let (md, _) = e.scan("legal_scan.pdf", &[]);
    assert!(
        missing(&md, &EXACT).is_empty(),
        "{:?}\n{md}",
        missing(&md, &EXACT)
    );
    assert!(md.contains("| --- | --- | --- |"), "{md}");
    assert!(md.contains("column_split_pages: []"), "{md}");
    assert!(md.contains("currency_fixes: {1: 2}"), "{md}");
    let (raw, _) = e.scan("legal_scan.pdf", &["--overwrite", "--no-currency-fix"]);
    assert!(
        raw.contains("S22,375.50") && raw.contains("currency_fixes: {}"),
        "{raw}"
    );
}

#[test]
fn single_column_pages_are_not_split() {
    let e = Env::new(&["legal.pdf"]);
    let (md, _) = e.scan("legal.pdf", &NEVER);
    assert!(md.contains("column_split_pages: []"), "{md}");
}

#[test]
fn two_column_reads_column_by_column() {
    let e = Env::new(&["two_column.pdf"]);
    let (md, _) = e.scan("two_column.pdf", &NEVER);
    assert!(md.contains("column_split_pages: [1]"), "{md}");
    let body = md.split("<!-- page 1 -->").nth(1).unwrap();
    assert!(
        body.trim_start().starts_with("ASSET PURCHASE AGREEMENT"),
        "{md}"
    );
    // Each sentence spans two lines in the source; the column crops read all of them.
    let mut last = 0;
    for side in ["Left", "Right"] {
        for i in 1..=15 {
            let key = format!("{side} column sentence {i} concerns");
            let at = md
                .find(&key)
                .unwrap_or_else(|| panic!("missing {key}\n{md}"));
            assert!(at > last, "{key} out of order\n{md}");
            last = at;
        }
    }
}

#[test]
fn multipage_tiff_has_two_pages() {
    let e = Env::new(&["multipage.tif"]);
    let (md, _) = e.scan("multipage.tif", &[]);
    assert!(md.contains("<!-- page 1"), "{md}");
    assert!(md.contains("<!-- page 2"), "{md}");
    assert!(!md.contains("<!-- page 3"), "{md}");
    assert!(md.contains("pages: 2\n"), "{md}");
}

#[test]
fn page_subsets_and_markers() {
    let e = Env::new(&["three_pages.pdf"]);
    let (md, _) = e.scan("three_pages.pdf", &["--overwrite"]);
    assert_eq!(md.matches("<!-- page ").count(), 3, "{md}");

    let (md, _) = e.scan("three_pages.pdf", &["--overwrite", "--pages", "2"]);
    assert_eq!(md.matches("<!-- page ").count(), 1, "{md}");
    assert!(md.contains("<!-- page 2"), "{md}");

    let (md, err) = e.scan("three_pages.pdf", &["--overwrite", "--pages", "9"]);
    assert!(err.contains("page 9 out of range"), "{err}");
    assert_eq!(md.matches("<!-- page ").count(), 0, "{md}");

    let (md, _) = e.scan("three_pages.pdf", &["--overwrite", "--no-page-markers"]);
    assert!(!md.contains("<!-- page"), "{md}");
}

#[test]
fn jobs_do_not_change_output() {
    let e = Env::new(&["three_pages.pdf"]);
    let flags = [
        "--overwrite",
        "--no-front-matter",
        "--use-text-layer",
        "never",
    ];
    let (one, _) = e.scan("three_pages.pdf", &[&flags[..], &["--jobs", "1"]].concat());
    let (eight, _) = e.scan("three_pages.pdf", &[&flags[..], &["--jobs", "8"]].concat());
    assert!(!one.is_empty());
    assert_eq!(one, eight);
}

#[test]
fn auto_uses_the_text_layer_on_born_digital_pdfs() {
    let e = Env::new(&["legal.pdf"]);
    let (md, _) = e.scan("legal.pdf", &[]);
    assert!(md.contains("text_layer_pages: [1]"), "{md}");
    assert!(md.contains("3:26-cv-04417"), "{md}");
}

#[test]
fn atomic_write_and_output_naming() {
    let e = Env::new(&["legal.pdf"]);
    e.cmd()
        .args([
            "scan",
            "--input-file",
            "legal.pdf",
            "--output-file",
            "out/sub/result",
        ])
        .assert()
        .success();
    assert!(e.path("out/sub/result.md").is_file());
    e.scan("legal.pdf", &[]);
    assert!(e.path("legal.md").is_file());
    for dir in [".", "out/sub"] {
        let leftovers: Vec<_> = fs::read_dir(e.path(dir))
            .unwrap()
            .filter_map(|f| f.ok())
            .filter(|f| f.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

#[test]
fn directory_mode_writes_each_file() {
    let e = Env::new(&[]);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures");
    fs::create_dir(e.path("in")).unwrap();
    for f in ["legal.pdf", "multipage.tif"] {
        fs::copy(src.join(f), e.path(&format!("in/{f}"))).unwrap();
    }
    e.cmd()
        .args(["scan", "--input-dir", "in"])
        .assert()
        .success();
    assert!(e.path("in/ocr-output/legal.md").is_file());
    assert!(e.path("in/ocr-output/multipage.md").is_file());
}

#[test]
fn unreadable_input_is_partial() {
    let e = Env::new(&[]);
    fs::write(e.path("bad.pdf"), b"%PDF-1.7\n").unwrap();
    e.cmd()
        .args(["scan", "--input-file", "bad.pdf"])
        .assert()
        .code(1);
    assert!(!e.path("bad.md").exists());
}

#[test]
fn dpi_is_validated() {
    let e = Env::new(&["legal.pdf"]);
    for bad in ["71", "601"] {
        e.cmd()
            .args(["scan", "--input-file", "legal.pdf", "--dpi", bad])
            .assert()
            .code(2);
    }
}
