//! Checkpoints, resume and scheduling (PLAN §5.3, §5.1): kill tests with SIGKILL, invalidation,
//! corrupt and failed pages, directory mode, and the cap on in-flight model requests.
//! The apple tests run real Apple Vision; the PDFs are built with the spike venv's pypdfium2.

mod common;

use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use assert_cmd::Command;
use common::{Env as ModelEnv, FIXTURES, without_generated};
#[cfg(target_os = "macos")]
use tempfile::{TempDir, tempdir};

#[cfg(target_os = "macos")] // the spike venv builds the test PDFs
const THREE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spike/fixtures/three_pages.pdf"
);
#[cfg(target_os = "macos")] // the spike venv builds the test PDFs
const PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spike/.venv/bin/python");

#[cfg(target_os = "macos")] // the spike venv builds the test PDFs
fn python(code: &str, args: &[&str]) {
    assert!(
        Path::new(PYTHON).exists(),
        "the spike venv is needed to build test PDFs: {PYTHON}"
    );
    let out = StdCommand::new(PYTHON)
        .args(["-I", "-c", code])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A PDF of `copies` x three_pages.pdf (3 * copies pages).
#[cfg(target_os = "macos")] // the spike venv builds the test PDFs
fn repeated_pdf(dest: &Path, copies: usize) {
    python(
        "import sys, pypdfium2 as p\n\
         s = p.PdfDocument(sys.argv[1]); d = p.PdfDocument.new()\n\
         [d.import_pages(s) for _ in range(int(sys.argv[3]))]\n\
         d.save(sys.argv[2])",
        &[THREE, dest.to_str().unwrap(), &copies.to_string()],
    );
}

/// Three pages, the middle one with an empty page box: Vision fails on it outright.
#[cfg(target_os = "macos")] // the spike venv builds the test PDFs
fn pdf_with_failing_page(dest: &Path) {
    python(
        "import sys, pypdfium2 as p\n\
         s = p.PdfDocument(sys.argv[1]); d = p.PdfDocument.new()\n\
         d.import_pages(s, [0]); g = d.new_page(100, 100)\n\
         g.set_mediabox(0, 0, 0, 0); g.set_cropbox(0, 0, 0, 0)\n\
         d.import_pages(s, [1]); d.save(sys.argv[2])",
        &[THREE, dest.to_str().unwrap()],
    );
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

/// `resumed from checkpoint N` and `pages OCR'd N` of a summary line.
fn summary_count(stderr: &str, label: &str) -> usize {
    let line = stderr
        .lines()
        .find(|l| l.starts_with("summary:"))
        .unwrap_or_else(|| panic!("no summary in:\n{stderr}"));
    let rest = &line[line
        .find(label)
        .unwrap_or_else(|| panic!("{label} in {line}"))
        + label.len()..];
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

// ------------------------------------------------------------------- apple

#[cfg(target_os = "macos")] // the apple engine runs Vision
struct Apple {
    config: TempDir,
    state: TempDir,
    work: TempDir,
}

#[cfg(target_os = "macos")] // the apple engine runs Vision
impl Apple {
    fn new() -> Self {
        Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
        }
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

    fn scan(&self, input: &str, output: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .args(["scan", "--input-file", input, "--output-file", output])
            .args(extra)
            .current_dir(self.work.path());
        c.assert()
    }

    /// Runs the scan to the end and returns (markdown, stderr).
    fn run(&self, input: &str, output: &str, extra: &[&str]) -> (String, String) {
        let a = self.scan(input, output, extra).success();
        let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
        (fs::read_to_string(self.path(output)).unwrap(), stderr)
    }

    /// A 12-page `doc.pdf` and an interrupted `--jobs 1` run of it with at least 2 checkpoints.
    fn interrupted(&self, extra: &[&str]) -> PathBuf {
        repeated_pdf(&self.path("doc.pdf"), 4);
        let mut args = vec!["--jobs", "1"];
        args.extend_from_slice(extra);
        kill_after(
            self.std_scan("doc.pdf", "out.md", &args),
            &self.path("out.md.ocr-partial"),
            2,
        );
        assert!(!self.path("out.md").exists());
        self.path("out.md.ocr-partial")
    }
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn killed_apple_run_resumes_to_the_clean_output() {
    let e = Apple::new();
    repeated_pdf(&e.path("big.pdf"), 14); // 42 pages
    let (clean, stderr) = e.run("big.pdf", "clean.md", &[]);
    assert!(
        !e.path("clean.md.ocr-partial").exists(),
        "kept the partial dir"
    );
    assert_eq!(summary_count(&stderr, "resumed from checkpoint"), 0);
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 42);

    let partial = e.path("out.md.ocr-partial");
    kill_after(
        e.std_scan("big.pdf", "out.md", &["--jobs", "1"]),
        &partial,
        2,
    );
    assert!(!e.path("out.md").exists());
    assert!(partial.join("manifest.json").exists());
    let done = pages_in(&partial);
    assert!((2..42).contains(&done), "{done} checkpoints at the kill");

    let (resumed, stderr) = e.run("big.pdf", "out.md", &["--jobs", "1"]);
    let n = summary_count(&stderr, "resumed from checkpoint");
    assert!(n >= done, "resumed {n}, had {done} checkpoints\n{stderr}");
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 42 - n);
    assert_eq!(without_generated(&resumed), without_generated(&clean));
    assert!(!partial.exists(), "the partial dir outlived the run");
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn a_changed_dpi_discards_the_checkpoints() {
    let e = Apple::new();
    let partial = e.interrupted(&[]);
    assert!(pages_in(&partial) >= 2);
    let (_, stderr) = e.run("doc.pdf", "out.md", &["--jobs", "1", "--dpi", "200"]);
    assert!(
        stderr.contains("discarding checkpoints") && stderr.contains("--dpi (150 -> 200)"),
        "{stderr}"
    );
    assert_eq!(summary_count(&stderr, "resumed from checkpoint"), 0);
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 12);
    assert!(!partial.exists());
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn a_modified_input_discards_the_checkpoints() {
    let e = Apple::new();
    e.interrupted(&[]);
    let f = fs::OpenOptions::new()
        .write(true)
        .open(e.path("doc.pdf"))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    drop(f);
    let (_, stderr) = e.run("doc.pdf", "out.md", &["--jobs", "1"]);
    assert!(
        stderr.contains("discarding checkpoints") && stderr.contains("input file"),
        "{stderr}"
    );
    assert_eq!(summary_count(&stderr, "resumed from checkpoint"), 0);
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 12);
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn overwrite_redoes_everything_and_drops_the_old_output() {
    let e = Apple::new();
    let partial = e.interrupted(&[]);
    // An old output from an earlier run is replaced too.
    fs::write(e.path("out.md"), "stale").unwrap();
    let (md, stderr) = e.run("doc.pdf", "out.md", &["--jobs", "1", "--overwrite"]);
    assert_eq!(summary_count(&stderr, "resumed from checkpoint"), 0);
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 12);
    assert!(!stderr.contains("discarding"), "{stderr}");
    assert!(md.contains("<!-- page 12"), "{md}");
    assert!(!partial.exists());
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn a_corrupt_page_file_is_redone() {
    let e = Apple::new();
    let partial = e.interrupted(&[]);
    let (clean, _) = e.run("doc.pdf", "clean.md", &[]);
    let victim = partial.join("page-00001.json");
    let bytes = fs::read(&victim).unwrap();
    fs::write(&victim, &bytes[..bytes.len() / 2]).unwrap();
    let have = pages_in(&partial);
    let (md, stderr) = e.run("doc.pdf", "out.md", &["--jobs", "1"]);
    assert!(
        stderr.contains("page-00001.json") && stderr.contains("unreadable"),
        "{stderr}"
    );
    assert_eq!(summary_count(&stderr, "resumed from checkpoint"), have - 1);
    assert_eq!(without_generated(&md), without_generated(&clean));
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn a_failed_page_is_not_checkpointed_and_is_retried() {
    let e = Apple::new();
    pdf_with_failing_page(&e.path("bad.pdf"));
    let partial = e.path("out.md.ocr-partial");
    let a = e.scan("bad.pdf", "out.md", &["--jobs", "1"]).code(1);
    let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(stderr.contains("pages [2]"), "{stderr}");
    assert!(e.path("out.md").exists());
    assert!(partial.join("page-00001.json").exists());
    assert!(
        !partial.join("page-00002.json").exists(),
        "checkpointed a failed page"
    );
    assert!(partial.join("page-00003.json").exists());

    // The output exists, but the failed page makes the re-run resume instead of refusing.
    let a = e.scan("bad.pdf", "out.md", &["--jobs", "1"]).code(1);
    let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert_eq!(
        summary_count(&stderr, "resumed from checkpoint"),
        2,
        "{stderr}"
    );
    assert_eq!(summary_count(&stderr, "pages OCR'd"), 1, "{stderr}");
    // A clean file (no failed pages) is final: asking again is refused.
    repeated_pdf(&e.path("ok.pdf"), 1);
    e.run("ok.pdf", "ok.md", &[]);
    e.scan("ok.pdf", "ok.md", &[]).code(2);
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn assembly_options_do_not_invalidate_checkpoints() {
    let e = Apple::new();
    let partial = e.interrupted(&[]);
    let have = pages_in(&partial);
    let (clean, _) = e.run("doc.pdf", "clean.md", &["--no-page-markers"]);
    let (md, stderr) = e.run("doc.pdf", "out.md", &["--jobs", "1", "--no-page-markers"]);
    assert!(!stderr.contains("discarding"), "{stderr}");
    assert!(summary_count(&stderr, "resumed from checkpoint") >= have);
    assert!(!md.contains("<!-- page"));
    assert_eq!(without_generated(&md), without_generated(&clean));
}

// --------------------------------------------------------------- directory

/// Six small input files.
fn six_files(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    for f in [
        "legal.pdf",
        "two_column.pdf",
        "three_pages.pdf",
        "legal_scan.pdf",
    ] {
        fs::copy(Path::new(FIXTURES).join(f), dir.join(f)).unwrap();
    }
    fs::copy(
        Path::new(FIXTURES).join("multipage.tif"),
        dir.join("multipage.tif"),
    )
    .unwrap();
    fs::copy(
        Path::new(FIXTURES).join("legal_200dpi.png"),
        dir.join("legal.png"),
    )
    .unwrap();
}

#[cfg(target_os = "macos")] // the apple engine runs Vision
fn scan_dir(e: &Apple, out: &str, jobs: &str) -> String {
    let a = Command::cargo_bin("ocr")
        .unwrap()
        .env("OCR_CONFIG_DIR", e.config.path())
        .env("OCR_STATE_DIR", e.state.path())
        .args([
            "scan",
            "--input-dir",
            "in",
            "--output-dir",
            out,
            "--jobs",
            jobs,
        ])
        .current_dir(e.work.path())
        .assert()
        .success();
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

fn outputs(dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                without_generated(&fs::read_to_string(e.path()).unwrap()),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn directory_of_six_files_is_the_same_at_jobs_4_and_1() {
    let e = Apple::new();
    six_files(&e.path("in"));
    let stderr = scan_dir(&e, "out4", "4");
    assert_eq!(summary_count(&stderr, "summary:"), 6, "{stderr}");
    scan_dir(&e, "out1", "1");
    let (a, b) = (outputs(&e.path("out4")), outputs(&e.path("out1")));
    assert_eq!(a.len(), 6);
    assert_eq!(a, b);
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn stale_partial_dir_next_to_a_finished_output_is_removed() {
    let e = Apple::new();
    six_files(&e.path("in"));
    scan_dir(&e, "out", "4");
    let name = e.path("out/three_pages.md.ocr-partial");
    fs::create_dir_all(&name).unwrap();
    fs::write(name.join("page-00001.json"), "{}").unwrap();
    let stderr = scan_dir(&e, "out", "4");
    assert!(stderr.contains("skipped 6 file(s)"), "{stderr}");
    assert!(!name.exists());
    // A dry run leaves it alone.
    fs::create_dir_all(&name).unwrap();
    Command::cargo_bin("ocr")
        .unwrap()
        .env("OCR_CONFIG_DIR", e.config.path())
        .env("OCR_STATE_DIR", e.state.path())
        .args([
            "scan",
            "--dry-run",
            "--input-dir",
            "in",
            "--output-dir",
            "out",
        ])
        .current_dir(e.work.path())
        .assert()
        .success();
    assert!(name.exists());
}

// ------------------------------------------------------------- model engine

fn slow(secs: &str) -> [(&'static str, &str); 2] {
    [("FAKE_MODE", "sleep"), ("FAKE_SLEEP_SECS", secs)]
}

#[test]
fn killed_model_run_resumes_without_repeating_finished_pages() {
    let e = ModelEnv::new("olmocr", &["three_pages.pdf"]);
    e.serve(&slow("1"));
    e.scan("three_pages.pdf", &["--output-file", "clean.md"], &[])
        .success();
    let clean = fs::read_to_string(e.path("clean.md")).unwrap();
    let per_run = e.requests().len();
    assert_eq!(per_run, 3, "one request per page");

    let partial = e.path("out.md.ocr-partial");
    kill_after(
        e.std_scan("three_pages.pdf", &["--output-file", "out.md"], &[]),
        &partial,
        1,
    );
    assert!(!e.path("out.md").exists());
    let done = pages_in(&partial);
    assert!(done >= 1);
    // The server is in its own process group and must still be up.
    e.base_cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("olmocr"));
    let before = e.requests().len();

    let a = e
        .scan("three_pages.pdf", &["--output-file", "out.md"], &[])
        .success();
    let stderr = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    let n = summary_count(&stderr, "resumed from checkpoint");
    assert!(n >= done, "{stderr}");
    assert_eq!(
        e.requests().len() - before,
        per_run - n,
        "a finished page was requested again"
    );
    assert_eq!(
        without_generated(&fs::read_to_string(e.path("out.md")).unwrap()),
        without_generated(&clean)
    );
    assert!(!partial.exists());
}

#[test]
fn model_directory_output_does_not_depend_on_jobs() {
    let e = ModelEnv::new("olmocr", &[]);
    six_files(&e.path("in"));
    e.serve(&[]);
    for (out, jobs) in [("out4", "4"), ("out1", "1")] {
        e.base_cmd()
            .args(["scan", "--model", "olmocr", "--input-dir", "in"])
            .args(["--output-dir", out, "--jobs", jobs])
            .assert()
            .success();
    }
    let (a, b) = (outputs(&e.path("out4")), outputs(&e.path("out1")));
    assert_eq!(a.len(), 6);
    assert_eq!(a, b);
}

/// Sets `--max-num-seqs` in the recorded argv of the running server (the fake ignores it).
#[cfg(target_os = "macos")]
fn set_max_num_seqs(e: &ModelEnv, n: usize) {
    let path = e.state.path().join("server.json");
    let mut info: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let argv = info["argv"].as_array_mut().unwrap();
    let i = argv.iter().position(|a| a == "--max-num-seqs").unwrap();
    argv[i + 1] = n.to_string().into();
    fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
}

#[test]
#[cfg(target_os = "macos")] // the venv builds its PDFs
fn in_flight_requests_are_capped_by_max_num_seqs() {
    let e = ModelEnv::new("olmocr", &[]);
    repeated_pdf(&e.path("six.pdf"), 2);
    e.serve(&slow("1"));

    // The server was started with 1: --jobs 8 still makes one request at a time.
    e.scan("six.pdf", &["--jobs", "8", "--output-file", "a.md"], &[])
        .success();
    assert_eq!(e.requests().len(), 6);
    assert_eq!(e.max_overlap(), 1);

    // With 2, the same run overlaps exactly two requests.
    set_max_num_seqs(&e, 2);
    for f in ["fake.log", "fake.log.end"] {
        fs::remove_file(e.path(f)).unwrap();
    }
    e.scan("six.pdf", &["--jobs", "8", "--output-file", "b.md"], &[])
        .success();
    assert_eq!(e.requests().len(), 6);
    assert_eq!(e.max_overlap(), 2);
    assert_eq!(
        without_generated(&fs::read_to_string(e.path("a.md")).unwrap()),
        without_generated(&fs::read_to_string(e.path("b.md")).unwrap())
    );
}

/// Off macOS the server records `slots` (llama-server) instead of `--max-num-seqs`; the PDFs
/// are two copies of a fixture in a directory, since the spike venv is not available.
#[cfg(not(target_os = "macos"))]
fn set_slots(e: &ModelEnv, n: usize) {
    let path = e.state.path().join("server.json");
    let mut info: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(info["slots"].is_number(), "no slots in {info}");
    info["slots"] = n.into();
    fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
}

#[test]
#[cfg(not(target_os = "macos"))]
fn in_flight_requests_are_capped_by_slots() {
    let e = ModelEnv::new("olmocr", &["three_pages.pdf"]);
    fs::create_dir(e.path("in")).unwrap();
    for f in ["a.pdf", "b.pdf"] {
        fs::copy(e.path("three_pages.pdf"), e.path("in").join(f)).unwrap();
    }
    e.serve(&slow("1"));
    let run = |out: &str| {
        e.base_cmd()
            .args(["scan", "--model", "olmocr", "--input-dir", "in"])
            .args(["--output-dir", out, "--jobs", "8"])
            .assert()
            .success();
    };

    // The server was started with 1 slot: --jobs 8 still makes one request at a time.
    run("a");
    assert_eq!(e.requests().len(), 6);
    assert_eq!(e.max_overlap(), 1);

    // With 2, the same run overlaps exactly two requests.
    set_slots(&e, 2);
    for f in ["fake.log", "fake.log.end"] {
        fs::remove_file(e.path(f)).unwrap();
    }
    run("b");
    assert_eq!(e.requests().len(), 6);
    assert_eq!(e.max_overlap(), 2);
    assert_eq!(outputs(&e.path("a")), outputs(&e.path("b")));
}

#[test]
fn two_scans_on_one_server_take_turns() {
    let e = ModelEnv::new("olmocr", &["three_pages.pdf"]);
    fs::copy(e.path("three_pages.pdf"), e.path("other.pdf")).unwrap();
    e.serve(&slow("1"));
    let mut scans: Vec<Scan> = ["three_pages.pdf", "other.pdf"]
        .iter()
        .map(|f| {
            let out = format!("{f}.md");
            Scan(
                e.std_scan(f, &["--jobs", "2", "--output-file", &out], &[])
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            )
        })
        .collect();
    for s in &mut scans {
        assert!(s.0.wait().unwrap().success());
    }
    assert_eq!(e.requests().len(), 6);
    assert_eq!(e.max_overlap(), 1, "requests of two processes overlapped");
}

// ------------------------------------------- directory mode: stale outputs

/// One scan of `in/` into `out/` with the apple engine; returns stderr.
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn scan_stale(e: &Apple, extra: &[&str]) -> String {
    let a = Command::cargo_bin("ocr")
        .unwrap()
        .env("OCR_CONFIG_DIR", e.config.path())
        .env("OCR_STATE_DIR", e.state.path())
        .args(["scan", "--input-dir", "in", "--output-dir", "out"])
        .args(extra)
        .current_dir(e.work.path())
        .assert()
        .success();
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

#[cfg(target_os = "macos")] // the apple engine runs Vision
fn scan_record_of(md: &Path) -> String {
    fs::read_to_string(md)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("scan_record: ").map(str::to_string))
        .expect("scan_record in the front matter")
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn directory_rescans_only_what_changed() {
    let e = Apple::new();
    fs::create_dir_all(e.path("in")).unwrap();
    fs::copy(Path::new(FIXTURES).join("legal.pdf"), e.path("in/a.pdf")).unwrap();
    let md = e.path("out/a.md");
    let sidecar = e.path("out/a.verify.json");

    // First run writes the output with a record.
    let stderr = scan_stale(&e, &[]);
    assert!(stderr.contains("summary: 1 file(s)"), "{stderr}");
    let rec = scan_record_of(&md);
    assert_eq!(rec.len(), 16);

    // (a) Nothing changed: skipped, not scanned.
    let stderr = scan_stale(&e, &[]);
    assert!(stderr.contains("skipped 1 file(s)"), "{stderr}");
    assert!(stderr.contains("summary: 0 file(s)"), "{stderr}");
    assert_eq!(scan_record_of(&md), rec);

    // (c) --verify-json added: rescanned, sidecar written.
    let stderr = scan_stale(&e, &["--verify-json"]);
    assert!(stderr.contains("rescanning"), "{stderr}");
    assert!(stderr.contains("summary: 1 file(s)"), "{stderr}");
    assert!(sidecar.exists());
    let rec_verify = scan_record_of(&md);
    assert_ne!(rec_verify, rec);
    // Unchanged with the same flag: skipped.
    let stderr = scan_stale(&e, &["--verify-json"]);
    assert!(stderr.contains("skipped 1 file(s)"), "{stderr}");
    // Sidecar deleted by hand: rescanned.
    fs::remove_file(&sidecar).unwrap();
    let stderr = scan_stale(&e, &["--verify-json"]);
    assert!(stderr.contains("rescanning"), "{stderr}");
    assert!(sidecar.exists());

    // (d) Verify mode dropped: rescanned, and the old sidecar is gone.
    let stderr = scan_stale(&e, &[]);
    assert!(stderr.contains("rescanning"), "{stderr}");
    assert!(!sidecar.exists());
    assert_eq!(scan_record_of(&md), rec);

    // (b) The input is replaced: rescanned, and the output changes.
    let before = without_generated(&fs::read_to_string(&md).unwrap());
    fs::copy(
        Path::new(FIXTURES).join("two_column.pdf"),
        e.path("in/a.pdf"),
    )
    .unwrap();
    let stderr = scan_stale(&e, &[]);
    assert!(stderr.contains("rescanning"), "{stderr}");
    assert!(stderr.contains("summary: 1 file(s)"), "{stderr}");
    let after = without_generated(&fs::read_to_string(&md).unwrap());
    assert_ne!(before, after);
    assert_ne!(scan_record_of(&md), rec);
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn replacing_the_input_removes_the_old_sidecar() {
    let e = Apple::new();
    fs::create_dir_all(e.path("in")).unwrap();
    fs::copy(Path::new(FIXTURES).join("legal.pdf"), e.path("in/a.pdf")).unwrap();
    scan_stale(&e, &["--verify-json"]);
    let sidecar = e.path("out/a.verify.json");
    assert!(sidecar.exists());
    fs::copy(
        Path::new(FIXTURES).join("two_column.pdf"),
        e.path("in/a.pdf"),
    )
    .unwrap();
    // Rescanned without --verify-json: no sidecar describing the old PDF is left.
    scan_stale(&e, &[]);
    assert!(!sidecar.exists());
}

#[test]
#[cfg(target_os = "macos")] // the apple engine runs Vision
fn output_without_a_scan_record_is_kept_and_reported() {
    let e = Apple::new();
    fs::create_dir_all(e.path("in")).unwrap();
    fs::copy(Path::new(FIXTURES).join("legal.pdf"), e.path("in/a.pdf")).unwrap();
    scan_stale(&e, &[]);
    // As an older ocr wrote it: no scan_record line.
    let md = e.path("out/a.md");
    let old: String = fs::read_to_string(&md)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("scan_record:"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&md, &old).unwrap();
    let stderr = scan_stale(&e, &[]);
    assert!(
        stderr.contains(
            "1 output(s) were made by an older ocr and were not checked against their inputs; use --overwrite to redo them."
        ),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&md).unwrap(), old);
}
