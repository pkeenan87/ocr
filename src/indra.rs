//! The indra-ocr engine: a CPU ONNX pipeline (datalab-to/indra-ocr) that runs as a helper
//! process per document instead of behind an HTTP server. Setup builds a venv with the pinned
//! `indra-ocr` package and downloads the pinned weights; a scan starts `helper.py` with the
//! pages the checkpoints still lack and reads one JSON line per finished page. Everything else
//! (checkpoints, resume, `--pages`, assembly) is the shared pipeline in `run`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::cli::TextLayer;
use crate::config::state_dir;
use crate::engines;
use crate::error::CliError;
use crate::htmltable;
use crate::markdown::{self, PageResult};
use crate::model_engine::{Fail, Prepared, blank, clean_ocr};
use crate::pipeline::{self, Settings};
use crate::progress;
use crate::run::{Backend, Work};
use crate::vision::DocKind;

/// The helper script, written to `<state>/indra/helper.py` by setup.
pub const HELPER: &str = include_str!("indra_helper.py");

/// Test hook: a program run in place of `<venv python> helper.py` with the same arguments
/// (tests/fake_indra.py), like `OCR_SERVER_CMD` for the model servers.
const FAKE_CMD: &str = "OCR_INDRA_CMD";

/// Block labels that are page furniture: dropped from the body, searched for the Bates number.
const FOOTER: &str = "Page-Footer";
const HEADER: &str = "Page-Header";

pub fn indra_dir() -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("indra"))
}

pub fn venv_dir() -> Result<PathBuf, CliError> {
    Ok(indra_dir()?.join("venv"))
}

/// The venv's interpreter (`Scripts\python.exe` on Windows).
pub fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

pub fn helper_path() -> Result<PathBuf, CliError> {
    Ok(indra_dir()?.join("helper.py"))
}

/// Where the weights live: `<state>/models/datalab-to/indra-ocr/`, like the GGUF repos.
pub fn model_dir() -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("models").join(engines::INDRA_REPO))
}

/// One page as the helper prints it.
#[derive(Debug, Deserialize)]
pub struct HelperPage {
    pub page: usize,
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub blocks: Vec<HelperBlock>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelperBlock {
    #[serde(default)]
    pub label: Option<String>,
    /// Pixels: [x0, y0, x1, y1].
    #[serde(default)]
    pub bbox: Vec<f64>,
    #[serde(default)]
    pub text: String,
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `markdown` without the paragraphs that are a furniture block's text (each block removes at
/// most one paragraph). A furniture block that the writer merged into a longer paragraph stays.
fn strip_furniture(markdown: &str, furniture: &[&HelperBlock]) -> String {
    let mut drop: Vec<String> = furniture
        .iter()
        .map(|b| squash(&b.text))
        .filter(|t| !t.is_empty())
        .collect();
    markdown
        .split("\n\n")
        .filter(|para| {
            let text = squash(para);
            match drop.iter().position(|d| *d == text) {
                Some(i) => {
                    drop.remove(i);
                    false
                }
                None => true,
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The right-most Bates match of the bottom-most block of `label` that has one.
fn bates_in_blocks(blocks: &[HelperBlock], label: &str) -> Option<String> {
    let mut found: Vec<&HelperBlock> = blocks
        .iter()
        .filter(|b| b.label.as_deref() == Some(label))
        .collect();
    let edge = |b: &HelperBlock, i: usize| b.bbox.get(i).copied().unwrap_or(0.0);
    // Bottom edge first, then right edge.
    found.sort_by(|a, b| {
        edge(b, 3)
            .total_cmp(&edge(a, 3))
            .then(edge(b, 2).total_cmp(&edge(a, 2)))
    });
    found
        .into_iter()
        .find_map(|b| markdown::last_bates_in(&b.text))
}

/// The finished page for one helper reply.
pub fn page_result(number: usize, p: &HelperPage) -> PageResult {
    if let Some(e) = &p.error {
        return PageResult {
            error: Some(e.clone()),
            ..blank(number)
        };
    }
    let furniture: Vec<&HelperBlock> = p
        .blocks
        .iter()
        .filter(|b| matches!(b.label.as_deref(), Some(FOOTER | HEADER)))
        .collect();
    let stripped = strip_furniture(&p.markdown, &furniture);
    let (tabled, tables) = htmltable::convert_tables(&stripped);
    let body = clean_ocr(&tabled);
    let bates = bates_in_blocks(&p.blocks, FOOTER)
        .or_else(|| bates_in_blocks(&p.blocks, HEADER))
        .or_else(|| markdown::find_bates_in_text(&body));
    // The court's filing stamp is labelled a page header, but its case number, filed date and
    // page count are on the page: it goes back at the top, after the Bates search so that its
    // `Page 1 of 30` is never taken for one. Two image readers never verify it (`court_stamp`).
    let stamp = furniture
        .iter()
        .map(|b| squash(&b.text))
        .find(|t| markdown::COURT_STAMP.is_match(t));
    let body = markdown::restore_court_stamp(body, stamp.as_deref());
    PageResult {
        body,
        bates,
        table: tables > 0,
        ..blank(number)
    }
}

/// What the reader thread sends.
enum Line {
    Page(HelperPage),
    Eof,
}

/// A running helper for one document.
struct Session {
    child: Child,
    rx: Receiver<Line>,
    ready: HashMap<usize, HelperPage>,
    /// Last lines the helper wrote to stderr and anything on stdout that was not a page.
    log: Arc<Mutex<String>>,
    /// Pages not yet taken out of this session.
    remaining: usize,
    /// Set once the helper's output ended.
    ended: Option<String>,
}

const LOG_KEEP: usize = 2000;

fn push_log(log: &Mutex<String>, line: &str) {
    let mut l = log.lock().unwrap();
    l.push_str(line);
    l.push('\n');
    if l.len() > LOG_KEEP {
        let mut cut = l.len() - LOG_KEEP;
        while !l.is_char_boundary(cut) {
            cut += 1;
        }
        l.drain(..cut);
    }
}

impl Session {
    /// The helper's reply for `number`, waiting for it; `Err` if the helper ended without it.
    fn page(&mut self, number: usize) -> Result<HelperPage, String> {
        loop {
            if let Some(p) = self.ready.remove(&number) {
                self.remaining = self.remaining.saturating_sub(1);
                return Ok(p);
            }
            if let Some(why) = &self.ended {
                self.remaining = self.remaining.saturating_sub(1);
                return Err(why.clone());
            }
            match self.rx.recv() {
                Ok(Line::Page(p)) => {
                    self.ready.insert(p.page, p);
                }
                Ok(Line::Eof) | Err(_) => {
                    let status = self
                        .child
                        .wait()
                        .map_or_else(|e| e.to_string(), |s| s.to_string());
                    let tail = self.log.lock().unwrap().trim().to_string();
                    self.ended = Some(format!(
                        "the indra-ocr helper ended ({status}) before page {number}{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(":\n{tail}")
                        }
                    ));
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped mid-document (the run stopped) must not leave the helper running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Pages of one file: those the helper does (1-based, in order) and those already served from
/// the text layer.
struct Plan {
    helper: Vec<usize>,
    text: HashMap<usize, PageResult>,
}

#[derive(Default)]
struct State {
    plans: HashMap<PathBuf, Plan>,
    sessions: HashMap<PathBuf, Session>,
}

/// The indra-ocr backend.
pub struct Indra {
    program: PathBuf,
    /// Arguments before the document: the helper script, unless the program is a test fake.
    prefix: Vec<PathBuf>,
    model_dir: PathBuf,
    state: Mutex<State>,
}

impl Indra {
    /// The backend, if setup's venv, helper and weights are there.
    pub fn new() -> Result<Self, CliError> {
        let model_dir = model_dir()?;
        if let Some(fake) = std::env::var_os(FAKE_CMD) {
            return Ok(Self {
                program: fake.into(),
                prefix: Vec::new(),
                model_dir,
                state: Mutex::default(),
            });
        }
        let python = venv_python(&venv_dir()?);
        let helper = helper_path()?;
        let weights_ok = engines::INDRA_FILES
            .iter()
            .all(|f| model_dir.join(f.name).is_file());
        if !(python.is_file() && helper.is_file() && weights_ok) {
            return Err(CliError::Setup(
                "error: the indra-ocr runtime or weights are missing. Run `ocr server setup --model indra-ocr`."
                    .into(),
            ));
        }
        Ok(Self {
            program: python,
            prefix: vec![helper],
            model_dir,
            state: Mutex::default(),
        })
    }

    fn spawn(&self, path: &Path, pages: &[usize], s: &Settings) -> Result<Session, String> {
        let list: Vec<String> = pages.iter().map(usize::to_string).collect();
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix)
            .arg(path)
            .args(["--pages", &list.join(",")])
            .args(["--dpi", &s.dpi.to_string()])
            .args(["--threads", &s.jobs.to_string()])
            .arg("--model-dir")
            .arg(&self.model_dir)
            .env("HF_HUB_OFFLINE", "1")
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.program.display()))?;
        let log = Arc::new(Mutex::new(String::new()));
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let err_log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                push_log(&err_log, &line);
            }
        });
        let (tx, rx) = mpsc::channel();
        let out_log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<HelperPage>(&line) {
                    Ok(p) => {
                        if tx.send(Line::Page(p)).is_err() {
                            return;
                        }
                    }
                    Err(_) if line.trim().is_empty() => {}
                    Err(_) => push_log(&out_log, &line),
                }
            }
            let _ = tx.send(Line::Eof);
        });
        Ok(Session {
            child,
            rx,
            ready: HashMap::new(),
            log,
            remaining: pages.len(),
            ended: None,
        })
    }

    fn failed(number: usize, reason: String) -> PageResult {
        progress::line(format!("warning: page {number}: {reason}"));
        PageResult {
            error: Some(reason),
            ..blank(number)
        }
    }
}

impl Backend for Indra {
    fn engine(&self) -> String {
        let (repo, runtime) = engines::indra_ids();
        format!("indra-ocr ({repo}, {runtime})")
    }

    fn name(&self) -> String {
        "indra-ocr".to_string()
    }

    fn model_ids(&self) -> Option<(String, String)> {
        Some(engines::indra_ids())
    }

    /// The helper does the work, with `--jobs` ONNX threads: one page at a time here.
    fn producers(&self, _: &Settings) -> usize {
        1
    }

    fn consumers(&self, _: &Settings) -> usize {
        0
    }

    fn open(
        &self,
        path: &Path,
        kind: DocKind,
        pending: &[usize],
        s: &Settings,
    ) -> Result<(), String> {
        let mut plan = Plan {
            helper: Vec::new(),
            text: HashMap::new(),
        };
        for &index in pending {
            // `--use-text-layer always`: pages with usable embedded text never reach indra.
            match pipeline::text_layer_page(path, kind, index, s) {
                Some(page) if s.text_layer == TextLayer::Always => {
                    plan.text.insert(index + 1, page);
                }
                _ => plan.helper.push(index + 1),
            }
        }
        self.state
            .lock()
            .unwrap()
            .plans
            .insert(path.to_path_buf(), plan);
        Ok(())
    }

    fn prepare(&self, path: &Path, _: DocKind, index: usize, s: &Settings) -> Work {
        let number = index + 1;
        let taken = {
            let mut st = self.state.lock().unwrap();
            if let Some(page) = st.plans.get_mut(path).and_then(|p| p.text.remove(&number)) {
                return Work::Done(page);
            }
            match st.sessions.remove(path) {
                Some(session) => Ok(session),
                None => match st.plans.get(path).map(|p| p.helper.clone()) {
                    Some(pages) if pages.contains(&number) => self.spawn(path, &pages, s),
                    _ => Err("page was not planned".to_string()),
                },
            }
        };
        // The state lock is not held while waiting for the helper.
        let mut session = match taken {
            Ok(session) => session,
            Err(e) => return Work::Done(Self::failed(number, e)),
        };
        let reply = session.page(number);
        if session.remaining > 0 {
            self.state
                .lock()
                .unwrap()
                .sessions
                .insert(path.to_path_buf(), session);
        }
        Work::Done(match reply {
            Ok(p) => {
                let page = page_result(number, &p);
                if let Some(e) = &page.error {
                    progress::line(format!("warning: page {number}: {e}"));
                }
                page
            }
            Err(e) => Self::failed(number, e),
        })
    }

    fn process(&self, _: &Path, _: Prepared, _: &Settings) -> Result<PageResult, Fail> {
        Err(Fail::Failed("indra-ocr has no model step".into()))
    }

    fn transcript_note(&self, _: &Path, _: &str, _: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(label: &str, bbox: [f64; 4], text: &str) -> HelperBlock {
        HelperBlock {
            label: Some(label.into()),
            bbox: bbox.to_vec(),
            text: text.into(),
        }
    }

    #[test]
    fn furniture_is_stripped_and_bates_found() {
        let p = HelperPage {
            page: 1,
            markdown: "# Title\n\nBody text.\n\nCOMPLAINT - Page 1 of 14\n\nSMITH000123".into(),
            blocks: vec![
                block("Section-Header", [0.0, 0.0, 9.0, 9.0], "Title"),
                block(
                    "Page-Footer",
                    [10.0, 1900.0, 500.0, 1920.0],
                    "COMPLAINT - Page 1 of 14",
                ),
                block("Page-Footer", [10.0, 1950.0, 500.0, 1970.0], "SMITH000123"),
            ],
            error: None,
        };
        let r = page_result(1, &p);
        assert_eq!(r.body, "# Title\n\nBody text.");
        assert_eq!(r.bates.as_deref(), Some("SMITH000123"));
        assert!(r.error.is_none() && !r.table);
    }

    #[test]
    fn tables_are_converted_and_header_bates_is_the_fallback() {
        let p = HelperPage {
            page: 2,
            markdown: "ABC000045\n\n<table><tr><th>A</th></tr><tr><td>1</td></tr></table>".into(),
            blocks: vec![block("Page-Header", [0.0, 0.0, 99.0, 9.0], "ABC000045")],
            error: None,
        };
        let r = page_result(2, &p);
        assert!(r.table, "{r:?}");
        assert!(
            !r.body.contains("<table>") && !r.body.contains("ABC000045"),
            "{}",
            r.body
        );
        assert_eq!(r.bates.as_deref(), Some("ABC000045"));
    }

    const STAMP: &str =
        "Case 1:19-cv-07208-ST Document 31-1 Filed 02/24/21 Page 2 of 5 PageID #: 140";

    #[test]
    fn the_filing_stamp_header_goes_back_at_the_top() {
        let p = HelperPage {
            page: 2,
            markdown: "SETTLEMENT AGREEMENT\n\nCase 1:19-cv-07208-ST Document 31-1\nFiled 02/24/21 Page 2 of 5 PageID #: 140\n\nBody text.\n\nSMITH000124".into(),
            blocks: vec![
                block(
                    "Page-Header",
                    [10.0, 10.0, 500.0, 30.0],
                    "SETTLEMENT AGREEMENT",
                ),
                block(
                    "Page-Header",
                    [10.0, 40.0, 900.0, 80.0],
                    "Case 1:19-cv-07208-ST Document 31-1\nFiled 02/24/21 Page 2 of 5 PageID #: 140",
                ),
                block("Page-Footer", [10.0, 1950.0, 500.0, 1970.0], "SMITH000124"),
            ],
            error: None,
        };
        let r = page_result(2, &p);
        assert_eq!(r.body, format!("{STAMP}\n\nBody text."));
        assert_eq!(r.bates.as_deref(), Some("SMITH000124"));
    }

    #[test]
    fn the_filing_stamp_is_not_taken_for_a_bates_number() {
        let p = HelperPage {
            page: 2,
            markdown: format!("{STAMP}\n\nBody text."),
            blocks: vec![block("Page-Header", [10.0, 40.0, 900.0, 80.0], STAMP)],
            error: None,
        };
        let r = page_result(2, &p);
        assert_eq!(r.body, format!("{STAMP}\n\nBody text."));
        assert_eq!(r.bates, None);
    }

    #[test]
    fn a_filing_stamp_left_in_the_body_is_not_doubled() {
        // The writer merged the header into a longer paragraph, so it was never stripped.
        let merged = format!("{STAMP} SETTLEMENT AGREEMENT");
        let p = HelperPage {
            page: 2,
            markdown: format!("{merged}\n\nBody text."),
            blocks: vec![block("Page-Header", [10.0, 40.0, 900.0, 80.0], STAMP)],
            error: None,
        };
        let r = page_result(2, &p);
        assert_eq!(r.body, format!("{merged}\n\nBody text."));
    }

    #[test]
    fn an_error_page_is_failed() {
        let p = HelperPage {
            page: 3,
            markdown: String::new(),
            blocks: Vec::new(),
            error: Some("boom".into()),
        };
        let r = page_result(3, &p);
        assert_eq!(r.error.as_deref(), Some("boom"));
        assert!(r.body.is_empty());
    }
}
