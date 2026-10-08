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
fn apple_defaults_to_vision_and_auto_uses_the_text_layer() {
    let e = Env::new(&["legal.pdf"]);
    let (md, _) = e.scan("legal.pdf", &[]);
    assert!(md.contains("text_layer_pages: []"), "{md}");
    assert!(md.contains("Bates SMITH000123"), "{md}");
    let (md, _) = e.scan("legal.pdf", &["--overwrite", "--use-text-layer", "auto"]);
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

/// The markdown without its `generated:` timestamp line.
fn without_generated(md: &str) -> String {
    md.lines()
        .filter(|l| !l.starts_with("generated:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/{name}.md"));
    fs::read_to_string(path).unwrap()
}

/// Single-column pages must stay byte-identical to the outputs captured before Phase 2b.
#[test]
fn golden_single_column_pages_are_byte_identical() {
    for name in ["legal", "legal_scan"] {
        let e = Env::new(&[&format!("{name}.pdf")]);
        let (md, _) = e.scan(&format!("{name}.pdf"), &NEVER);
        assert_eq!(without_generated(&md), golden(name), "{name}");
    }
}

/// Sentences of the two-column page must be exact and in order; the rest may differ.
fn assert_two_column_sentences(md: &str) {
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
fn golden_two_column_and_three_pages() {
    let e = Env::new(&["two_column.pdf", "three_pages.pdf"]);
    let (md, _) = e.scan("two_column.pdf", &NEVER);
    assert_eq!(without_generated(&md), golden("two_column"));
    assert_two_column_sentences(&md);
    let (md, _) = e.scan("three_pages.pdf", &NEVER);
    assert_eq!(without_generated(&md), golden("three_pages"));
    assert_two_column_sentences(&md);
}

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Guards the Vision concurrent start-up crash (SIGSEGV in objc_retain under TextRecognition;
/// docs/spike.md). The bridge caps Vision at 2 requests and `scan` retries a crashed run from
/// its checkpoints, so every launch must still exit 0 with the uninterrupted output.
/// Probabilistic, so it spawns many processes.
#[test]
fn concurrent_start_up_always_finishes_with_the_clean_output() {
    for run in 0..20 {
        let e = Env::new(&["three_pages.pdf"]);
        let (md, err) = e.scan(
            "three_pages.pdf",
            &["--jobs", "12", "--quiet", "--use-text-layer", "never"],
        );
        assert_eq!(
            without_generated(&md),
            golden("three_pages"),
            "run {run}\n{err}"
        );
    }
}

fn flag_note(stderr: &str) -> bool {
    stderr.contains("looks like a deposition transcript")
        && stderr.contains("use --model paddleocr-vl")
}

#[test]
fn four_up_transcript_is_flagged() {
    let e = Env::new(&[]);
    fs::copy(manifest("spike/heldout/four_up.pdf"), e.path("four_up.pdf")).unwrap();
    let (md, err) = e.scan("four_up.pdf", &NEVER);
    assert!(md.contains("transcript_pages: [1]"), "{md}");
    assert!(flag_note(&err), "{err}");
    assert!(err.contains("four_up.pdf"), "{err}");
}

#[test]
fn full_page_transcript_lines_are_numbered_by_geometry() {
    let e = Env::new(&["transcript_full.pdf"]);
    let (md, _) = e.scan("transcript_full.pdf", &NEVER);
    assert!(
        md.contains("line_numbers: {anchored: [1], unverified: []}"),
        "{md}"
    );
    // Vision drops the numbers 1-7 and misreads some `Q.`/`A.`, but every physical line
    // carries its own slot number and its own text.
    for n in 1..=25 {
        assert!(
            md.lines()
                .any(|l| l.starts_with(&format!("{n} ")) && l.contains(&format!("Line {n} of"))),
            "line {n} missing:\n{md}"
        );
    }
}

#[test]
fn four_up_mini_pages_are_numbered_in_page_order() {
    let e = Env::new(&["four_up.pdf"]);
    let (md, _) = e.scan("four_up.pdf", &NEVER);
    assert!(
        md.contains("line_numbers: {anchored: [1], unverified: []}"),
        "{md}"
    );
    let heads: Vec<&str> = md.lines().filter(|l| l.starts_with("Page ")).collect();
    assert_eq!(heads, ["Page 41", "Page 42", "Page 43", "Page 44"], "{md}");
    // Each mini-page holds slots 1-25 in order; most lines are read well enough to check the
    // question number printed on them (`Question <page-index>.<line>`).
    let mut exact = 0;
    let mut page = 0;
    let mut slot = 0;
    for l in md.lines() {
        if l.starts_with("Page ") {
            page += 1;
            slot = 0;
        } else if let Some((n, text)) = l.split_once(' ')
            && let Ok(n) = n.parse::<u32>()
        {
            slot += 1;
            assert_eq!(n, slot, "slots out of order on page {page}: {l}");
            exact += usize::from(text.contains(&format!("Question {page}.{n} ")));
        }
    }
    assert_eq!(slot, 25);
    assert!(
        exact >= 90,
        "only {exact}/100 lines carry the right question:\n{md}"
    );
}

#[test]
fn ordinary_pages_are_not_flagged_as_transcripts() {
    let e = Env::new(&["legal.pdf", "two_column.pdf"]);
    for name in ["legal.pdf", "two_column.pdf"] {
        let (md, err) = e.scan(name, &NEVER);
        assert!(md.contains("transcript_pages: []"), "{md}");
        assert!(!flag_note(&err), "{err}");
    }
}

/// Generates held-out seeds 0-2 (clean and degraded) with the spike venv, scans them and checks
/// that at least 95% of the expected lines are found and in order. Needs the spike venv; set
/// `OCR_SKIP_HELDOUT=1` to skip deliberately.
#[test]
fn heldout_seeds_are_read_in_order() {
    if std::env::var_os("OCR_SKIP_HELDOUT").is_some() {
        return;
    }
    let python = manifest("spike/.venv/bin/python");
    assert!(
        python.is_file(),
        "{} not found: create the spike venv or set OCR_SKIP_HELDOUT=1",
        python.display()
    );
    let e = Env::new(&[]);
    let (mut found, mut total, mut ordered, mut pairs) = (0, 0, 0, 0);
    for seed in 0..3 {
        let dir = e.path(&format!("seed{seed}"));
        std::process::Command::new(&python)
            .args([
                "-I",
                "spike/heldout.py",
                "--seed",
                &seed.to_string(),
                "--out",
            ])
            .arg(&dir)
            .current_dir(manifest("."))
            .status()
            .unwrap()
            .success()
            .then_some(())
            .expect("heldout.py failed");
        e.cmd()
            .args(["scan", "--input-dir"])
            .arg(&dir)
            .args(["--quiet", "--use-text-layer", "never"])
            .assert()
            .success();
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(family) = name.strip_suffix(".expected.json") else {
                continue;
            };
            let expected: Vec<String> =
                serde_json::from_str(&fs::read_to_string(entry.path()).unwrap()).unwrap();
            for variant in [family.to_string(), format!("{family}_scan")] {
                let md = fs::read_to_string(dir.join(format!("ocr-output/{variant}.md"))).unwrap();
                let at: Vec<usize> = expected
                    .iter()
                    .filter_map(|s| md.find(s.as_str()))
                    .collect();
                found += at.len();
                total += expected.len();
                pairs += at.len().saturating_sub(1);
                ordered += at.windows(2).filter(|w| w[1] > w[0]).count();
            }
        }
    }
    let (f, o) = (found as f64 / total as f64, ordered as f64 / pairs as f64);
    assert!(f >= 0.95, "found {found}/{total}");
    assert!(o >= 0.95, "in order {ordered}/{pairs}");
}
