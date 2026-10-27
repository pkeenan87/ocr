use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::{TempDir, tempdir};

const PDF: &[u8] = b"%PDF-1.7\n";
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
const WARNING: &str = if cfg!(target_os = "macos") {
    "ocr is not set up yet. Run `ocr server setup` first."
} else {
    "ocr is not set up yet. Run `ocr server setup --model paddleocr-vl` first."
};

/// Command output with Windows path separators shown as `/`, for comparing paths.
fn slashes(out: &[u8]) -> String {
    String::from_utf8_lossy(out).replace('\\', "/")
}

struct Env {
    config: TempDir,
    state: TempDir,
    work: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            config: tempdir().unwrap(),
            state: tempdir().unwrap(),
            work: tempdir().unwrap(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("ocr").unwrap();
        c.env("OCR_CONFIG_DIR", self.config.path())
            .env("OCR_STATE_DIR", self.state.path())
            .current_dir(self.work.path());
        c
    }

    /// `ocr server setup`. Off macOS there is no Apple engine, so a (fake) paddleocr-vl install.
    fn setup(&self) {
        if cfg!(target_os = "macos") {
            self.cmd().args(["server", "setup"]).assert().success();
        } else {
            self.cmd()
                .env("OCR_FAKE_INSTALL", "1")
                .args(["server", "setup", "--model", "paddleocr-vl"])
                .assert()
                .success();
        }
    }

    /// Runs setup and checks whether stderr carries the olmOCR removal notice.
    fn setup_quiet_check(&self, expect_notice: bool) {
        let mut c = self.cmd();
        c.env("OCR_FAKE_INSTALL", "1").args(["server", "setup"]);
        if cfg!(not(target_os = "macos")) {
            c.args(["--model", "paddleocr-vl"]);
        }
        let out = c.assert().success();
        let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
        assert_eq!(err.contains("olmOCR was removed"), expect_notice, "{err}");
        assert_eq!(err.contains("can be deleted"), expect_notice, "{err}");
    }

    fn file(&self, rel: &str, bytes: &[u8]) {
        let p = self.work.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// Runs a successful dry-run scan and returns stdout, with Windows `\` separators shown as
    /// `/`. Off macOS the default engine (paddleocr-vl) must be set up first.
    fn dry_run(&self, args: &[&str]) -> String {
        if cfg!(not(target_os = "macos")) {
            self.setup();
        }
        let a = self
            .cmd()
            .args(["scan", "--dry-run"])
            .args(args)
            .assert()
            .success();
        slashes(&a.get_output().stdout)
    }
}

#[test]
#[cfg(target_os = "macos")]
fn apple_before_setup_warns_and_continues() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    e.cmd()
        .args(["scan", "--dry-run", "--input-file", "a.pdf"])
        .assert()
        .success()
        .stderr(predicate::str::contains(WARNING))
        .stdout(predicate::str::contains("a.pdf -> a.md"));
}

#[test]
fn dots_before_setup_exits_2() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    e.cmd()
        .args(["scan", "--model", "dots-ocr", "--input-file", "a.pdf"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(WARNING));
    e.cmd()
        .args(["server", "start", "--model", "dots-ocr"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(WARNING));
}

#[test]
#[cfg(target_os = "macos")]
fn server_start_without_model_needs_one_installed() {
    let e = Env::new();
    e.setup();
    e.cmd()
        .args(["server", "start"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no model engine is installed"))
        .stderr(predicate::str::contains("apple engine needs no server"));
    let config = e.config.path().join("config.toml");
    let mut toml = fs::read_to_string(&config).unwrap();
    toml.push_str(
        "\n[engines.paddleocr-vl]\ninstalled = true\n\n[engines.dots-ocr]\ninstalled = true\n",
    );
    fs::write(&config, toml).unwrap();
    e.cmd()
        .args(["server", "start"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "choose one with --model (paddleocr-vl, dots-ocr)",
        ));
}

#[test]
fn setup_then_dots_not_installed() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    e.setup();
    e.cmd()
        .args(["scan", "--model", "dots-ocr", "--input-file", "a.pdf"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "Engine 'dots-ocr' is not installed. Run `ocr server setup --model dots-ocr`.",
        ));
    e.cmd()
        .args(["server", "start", "--model", "dots-ocr"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not installed"));
    e.cmd()
        .args(["scan", "--dry-run", "--input-file", "a.pdf"])
        .assert()
        .success()
        .stderr(predicate::str::contains("not set up").not());
}

#[test]
fn setup_is_idempotent_and_writes_config() {
    let e = Env::new();
    e.setup();
    let path = e.config.path().join("config.toml");
    let first = fs::read_to_string(&path).unwrap();
    let engine = if cfg!(target_os = "macos") {
        "[engines.apple]"
    } else {
        "[engines.paddleocr-vl]"
    };
    assert!(first.contains(engine) && first.contains("installed = true"));
    e.setup();
    assert_eq!(first, fs::read_to_string(&path).unwrap());
}

#[test]
fn setup_rejects_unavailable_quant_and_version() {
    let e = Env::new();
    for args in [
        &["--model", "paddleocr-vl", "--quant", "8bit"][..],
        &["--model", "indra-ocr", "--model-version", "1"],
        &["--model", "dots-ocr", "--model-version", "9"],
    ] {
        e.cmd()
            .args(["server", "setup"])
            .args(args)
            .assert()
            .code(2);
    }
    // The apple config is still written.
    assert!(e.config.path().join("config.toml").exists());
}

#[test]
fn server_stop_and_status_never_fail() {
    let e = Env::new();
    for sub in ["stop", "status"] {
        e.cmd()
            .args(["server", sub])
            .assert()
            .success()
            .stdout(predicate::str::contains("no server running"));
    }
}

#[test]
fn conflicting_and_mismatched_flags_exit_2() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    let cases: &[&[&str]] = &[
        &["scan", "--input-file", "a.pdf", "--input-dir", "."],
        &["scan"],
        &["scan", "--input-dir", ".", "--output-file", "x.md"],
        &["scan", "--input-file", "a.pdf", "--output-dir", "o"],
        &["scan", "--input-file", "a.pdf", "--quiet", "--verbose"],
        &["scan", "--input-file", "a.pdf", "--model", "bogus"],
        &["scan", "--input-file", "a.pdf", "--jobs", "0"],
        &["frobnicate"],
    ];
    for args in cases {
        e.cmd().args(*args).assert().code(2);
    }
}

#[test]
fn parallel_is_alias_for_jobs() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    assert!(
        e.dry_run(&["--input-file", "a.pdf", "--parallel", "3"])
            .contains("jobs 3")
    );
}

#[test]
fn single_file_default_and_md_append() {
    let e = Env::new();
    e.file("in/Brief.pdf", PDF);
    assert!(
        e.dry_run(&["--input-file", "in/Brief.pdf"])
            .contains("in/Brief.pdf -> Brief.md")
    );
    e.cmd()
        .args([
            "scan",
            "--dry-run",
            "--input-file",
            "in/Brief.pdf",
            "--output-file",
            "out.txt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("-> out.txt.md"))
        .stderr(predicate::str::contains("note:"));
    e.cmd()
        .args([
            "scan",
            "--dry-run",
            "--input-file",
            "in/Brief.pdf",
            "--output-file",
            "x/y.md",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("-> x/y.md"))
        .stderr(predicate::str::contains("note:").not());
}

#[test]
fn single_file_errors() {
    let e = Env::new();
    e.setup();
    e.file("a.pdf", PDF);
    e.file("a.md", b"old");
    e.cmd()
        .args(["scan", "--input-file", "nope.pdf"])
        .assert()
        .code(2);
    e.cmd()
        .args(["scan", "--input-file", "a.pdf", "--file-type", "image"])
        .assert()
        .code(2);
    // Output exists without --overwrite.
    e.cmd()
        .args(["scan", "--input-file", "a.pdf"])
        .assert()
        .code(2);
    e.cmd()
        .args(["scan", "--dry-run", "--input-file", "a.pdf", "--overwrite"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.pdf -> a.md"));
}

#[test]
fn directory_default_output_excludes_itself() {
    let e = Env::new();
    e.file("docs/a.pdf", PDF);
    e.file("docs/ocr-output/old.pdf", PDF);
    let out = e.dry_run(&["--recursive", "--input-dir", "docs"]);
    assert!(out.contains("docs/a.pdf -> docs/ocr-output/a.md"), "{out}");
    assert!(!out.contains("old"), "{out}");
}

#[test]
fn directory_collisions() {
    let e = Env::new();
    e.file("d/a.pdf", PDF);
    e.file("d/a.png", PNG);
    let out = e.dry_run(&["--input-dir", "d"]);
    assert!(
        out.contains("d/ocr-output/a.pdf.md") && out.contains("d/ocr-output/a.png.md"),
        "{out}"
    );
}

#[test]
fn directory_recursive_mirroring() {
    let e = Env::new();
    e.file("d/top.pdf", PDF);
    e.file("d/sub/deep/x.png", PNG);
    let flat = e.dry_run(&["--input-dir", "d", "--output-dir", "out"]);
    assert!(
        flat.contains("-> out/top.md") && !flat.contains("x.md"),
        "{flat}"
    );
    let rec = e.dry_run(&["--recursive", "--input-dir", "d", "--output-dir", "out"]);
    assert!(
        rec.contains("-> out/top.md") && rec.contains("-> out/sub/deep/x.md"),
        "{rec}"
    );
}

#[test]
fn file_type_filter_and_magic_bytes() {
    let e = Env::new();
    e.file("d/real.pdf", PDF);
    e.file("d/pic.png", PNG);
    e.file("d/disguised.png", PDF); // a PDF with a .png name
    e.file("d/notes.txt", b"hello");
    let pdf = e.dry_run(&["--input-dir", "d", "--file-type", "pdf"]);
    assert!(
        pdf.contains("real.md") && pdf.contains("disguised.md") && !pdf.contains("pic.md"),
        "{pdf}"
    );
    let img = e.dry_run(&["--input-dir", "d", "--file-type", "image"]);
    assert!(
        img.contains("pic.md") && !img.contains("real.md") && !img.contains("disguised"),
        "{img}"
    );
    let auto = e.dry_run(&["--input-dir", "d"]);
    assert!(
        !auto.contains("notes") && auto.contains("Dry run: 3 file(s)"),
        "{auto}"
    );
}

#[test]
fn single_file_filter_uses_magic_bytes() {
    let e = Env::new();
    e.file("disguised.png", PDF);
    e.cmd()
        .args([
            "scan",
            "--input-file",
            "disguised.png",
            "--file-type",
            "image",
        ])
        .assert()
        .code(2);
    e.dry_run(&["--input-file", "disguised.png", "--file-type", "pdf"]);
}

#[test]
fn directory_without_matches_warns_and_exits_0() {
    let e = Env::new();
    e.setup();
    e.file("d/notes.txt", b"hello");
    e.cmd()
        .args(["scan", "--input-dir", "d"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no matching files"));
}

#[test]
fn invalid_model_version_exits_2() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    e.setup();
    let apple = cfg!(target_os = "macos");
    // Off macOS only the releases with GGUF builds are offered (PLAN §9.3).
    let (paddle, dots) = if apple {
        ("1.6, 1.5, 1.0", "dots.mocr, dots.ocr")
    } else {
        ("1.6, 1.5", "dots.ocr")
    };
    let cases = [
        ("paddleocr-vl", "2.0", paddle),
        ("dots-ocr", "x", dots),
        ("indra-ocr", "1", "no selectable"),
        ("apple", "1", "no selectable"),
    ];
    for (model, ver, expect) in cases.into_iter().filter(|c| apple || c.0 != "apple") {
        e.cmd()
            .args([
                "scan",
                "--input-file",
                "a.pdf",
                "--model",
                model,
                "--model-version",
                ver,
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(expect));
    }
}

#[test]
fn valid_model_version_reaches_gate() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    e.setup();
    e.cmd()
        .args([
            "scan",
            "--input-file",
            "a.pdf",
            "--model",
            "dots-ocr",
            "--model-version",
            "dots.ocr",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not installed"));
}

#[test]
fn pages_validation() {
    let e = Env::new();
    e.file("a.pdf", PDF);
    for bad in ["0", "5-2", "a", "1,,2"] {
        e.cmd()
            .args(["scan", "--dry-run", "--input-file", "a.pdf", "--pages", bad])
            .assert()
            .code(2);
    }
    e.dry_run(&["--input-file", "a.pdf", "--pages", "1-50,75"]);
}

#[test]
fn directory_skips_existing_outputs() {
    let e = Env::new();
    e.setup();
    e.file("d/a.pdf", PDF);
    e.file("d/b.pdf", PDF);
    e.file("d/ocr-output/a.md", b"done");
    let a = e
        .cmd()
        .args(["scan", "--dry-run", "--input-dir", "d"])
        .assert()
        .success()
        .stderr(predicate::str::contains("skipped 1 file(s)"));
    let out = slashes(&a.get_output().stdout);
    assert!(
        out.contains("b.md") && !out.contains("-> d/ocr-output/a.md"),
        "{out}"
    );
    let out = e.dry_run(&["--input-dir", "d", "--overwrite"]);
    assert!(out.contains("-> d/ocr-output/a.md"), "{out}");
}

#[test]
fn model_olmocr_hints_dots_ocr() {
    let e = Env::new();
    for args in [
        &["scan", "--model", "olmocr", "--input-file", "a.pdf"][..],
        &["server", "start", "--model", "olmocr"],
        &["server", "setup", "--model", "olmocr"],
    ] {
        e.cmd()
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid value 'olmocr'"))
            .stderr(predicate::str::contains("use --model dots-ocr"));
    }
    e.cmd().arg("--help").assert().success();
    e.cmd()
        .args(["scan", "--model", "nope", "--input-file", "a.pdf"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("use --model dots-ocr").not());
}

#[test]
fn setup_drops_olmocr_entry_once() {
    let e = Env::new();
    e.setup();
    let path = e.config.path().join("config.toml");
    let repo = if cfg!(target_os = "macos") {
        "mlx-community/olmOCR-2-7B-1025-8bit"
    } else {
        "bartowski/allenai_olmOCR-2-7B-1025-GGUF"
    };
    let mut toml = fs::read_to_string(&path).unwrap();
    toml.push_str(&format!(
        "\n[engines.olmocr]\ninstalled = true\nrepo = \"{repo}\"\n"
    ));
    fs::write(&path, &toml).unwrap();
    e.cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("olmOCR was removed"));
    assert_eq!(toml, fs::read_to_string(&path).unwrap());
    e.setup_quiet_check(true);
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("[engines.olmocr]")
    );
    e.setup_quiet_check(false);
    e.cmd()
        .args(["server", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("olmOCR").not());
}
