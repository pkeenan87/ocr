//! `--legal-cleanup` end to end (PLAN §5.5): the eval `contract` family through real Apple Vision,
//! and toggling the flag on a resumed run. Needs the spike venv to generate the PDFs.

// Needs Apple Vision (PLAN §9).
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::{TempDir, tempdir};

const PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spike/.venv/bin/python");
const GENERATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/eval/generate.py");

struct Env {
    config: TempDir,
    state: TempDir,
    work: TempDir,
}

impl Env {
    /// A fresh environment whose working dir holds the generated eval `contract` family.
    fn new() -> Self {
        assert!(
            Path::new(PYTHON).exists(),
            "the spike venv is needed: {PYTHON}"
        );
        let e = Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
        };
        let out = StdCommand::new(PYTHON)
            .args(["-I", GENERATE, "--seed", "0", "--out"])
            .arg(e.work.path())
            .args(["--families", "contract"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        e
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.work.path().join(rel)
    }

    fn std_scan(&self, input: &str, output: &str, extra: &[&str]) -> StdCommand {
        let mut c = StdCommand::new(env!("CARGO_BIN_EXE_ocr"));
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .args(["scan", "--input-file", input, "--output-file", output])
            .args(extra)
            .current_dir(self.work.path());
        c
    }

    /// Runs the scan to the end and returns (markdown, stderr).
    fn run(&self, input: &str, output: &str, extra: &[&str]) -> (String, String) {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .args(["scan", "--input-file", input, "--output-file", output])
            .args(extra)
            .current_dir(self.work.path());
        let a = c.assert().success();
        let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
        (fs::read_to_string(self.path(output)).unwrap(), stderr)
    }
}

/// Page bodies: the document without the YAML header and with the page markers removed.
fn bodies(md: &str) -> String {
    let rest = md.split_once("\n---\n").map_or(md, |(_, r)| r);
    rest.lines()
        .filter(|l| !l.starts_with("<!-- page"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn markers(md: &str) -> Vec<&str> {
    md.lines().filter(|l| l.starts_with("<!-- page")).collect()
}

fn without_generated(md: &str) -> String {
    md.lines()
        .filter(|l| !l.starts_with("generated:") && !l.starts_with("scan_record:"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summary_count(stderr: &str, label: &str) -> usize {
    let line = stderr
        .lines()
        .find(|l| l.starts_with("summary:"))
        .unwrap_or_else(|| panic!("no summary in:\n{stderr}"));
    let rest = &line[line.find(label).unwrap() + label.len()..];
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn pages_in(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with("page-") && n.ends_with(".json")
                })
                .count()
        })
        .unwrap_or(0)
}

/// Kills the child if the test panics.
struct Scan(Child);

impl Drop for Scan {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for `n` page checkpoints in `dir`, then SIGKILLs the process.
fn kill_after(mut cmd: StdCommand, dir: &Path, n: usize) {
    let mut scan = Scan(
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let t = Instant::now();
    while pages_in(dir) < n {
        assert!(
            scan.0.try_wait().unwrap().is_none(),
            "the scan ended before {n} checkpoints"
        );
        assert!(t.elapsed() < Duration::from_secs(120), "no checkpoints");
        std::thread::sleep(Duration::from_millis(10));
    }
    scan.0.kill().unwrap(); // SIGKILL
    scan.0.wait().unwrap();
}

#[test]
fn contract_headers_and_footers_go_and_content_stays() {
    let e = Env::new();
    let (plain, _) = e.run("contract.pdf", "plain.md", &[]);
    let (clean, _) = e.run("contract.pdf", "clean.md", &["--legal-cleanup"]);

    // Off by default: nothing in the header, and the running lines are still in the bodies.
    assert!(!plain.contains("legal_cleanup"));
    assert!(bodies(&plain).contains("CONFIDENTIAL"), "{plain}");

    let body = bodies(&clean);
    assert!(!body.contains("CONFIDENTIAL"), "header left in:\n{clean}");
    assert!(!body.contains("Asset Purchase Agreement\n"), "{clean}");
    assert!(!body.contains(" of 4"), "footer left in:\n{clean}");
    assert!(
        clean.contains("legal_cleanup: {stripped_lines: "),
        "{clean}"
    );

    // Every money amount and date of the ground truth survives.
    let gt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(e.path("contract.gt.json")).unwrap()).unwrap();
    let mut checked = 0;
    for page in gt["pages"].as_array().unwrap() {
        for t in page["tokens"].as_array().unwrap() {
            let t = t.as_str().unwrap();
            assert!(body.contains(t), "token {t:?} missing from\n{clean}");
            checked += 1;
        }
    }
    assert!(checked >= 8, "only {checked} tokens");

    // Page markers (and so Bates numbers) are exactly the ones of the plain run.
    assert_eq!(markers(&clean), markers(&plain));
    assert_eq!(markers(&clean).len(), 4);
}

#[test]
fn toggling_legal_cleanup_on_a_resumed_run_redoes_no_pages() {
    let e = Env::new();
    // 48 pages: twelve copies of the contract.
    let code = "import sys, pypdfium2 as p\n\
                s = p.PdfDocument(sys.argv[1]); d = p.PdfDocument.new()\n\
                [d.import_pages(s) for _ in range(12)]\n\
                d.save(sys.argv[2])";
    let out = StdCommand::new(PYTHON)
        .args(["-I", "-c", code])
        .arg(e.path("contract.pdf"))
        .arg(e.path("big.pdf"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let (plain, _) = e.run("big.pdf", "plain.md", &["--jobs", "1"]);
    let (clean, _) = e.run("big.pdf", "clean.md", &["--jobs", "1", "--legal-cleanup"]);
    assert!(!bodies(&clean).contains("CONFIDENTIAL"));
    assert!(bodies(&plain).contains("CONFIDENTIAL"));

    // (flag on the killed run, flag on the resume, output of a clean run with the resume's flag)
    for (first, second, expected) in [(false, true, &clean), (true, false, &plain)] {
        let flag = ["--jobs", "1", "--legal-cleanup"];
        let args = |on: bool| if on { &flag[..] } else { &flag[..2] };
        let partial = e.path("out.md.ocr-partial");
        let _ = fs::remove_file(e.path("out.md"));
        kill_after(e.std_scan("big.pdf", "out.md", args(first)), &partial, 2);
        assert!(!e.path("out.md").exists());
        let done = pages_in(&partial);
        assert!((2..48).contains(&done), "{done} checkpoints at the kill");

        let (resumed, stderr) = e.run("big.pdf", "out.md", args(second));
        let n = summary_count(&stderr, "resumed from checkpoint");
        assert!(n >= done, "resumed {n}, had {done} checkpoints\n{stderr}");
        assert_eq!(summary_count(&stderr, "pages OCR'd"), 48 - n);
        assert_eq!(without_generated(&resumed), without_generated(expected));
    }
}
