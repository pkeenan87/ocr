//! The long-document pipeline (PLAN §5, §5.1): one pool for every file of a run, a single
//! writer thread that owns all checkpoint and output writes, and assembly from the checkpoint
//! records. Output cannot depend on scheduling: pages are only ever combined by page number,
//! from the records on disk.
//!
//! Producers take (file, page) tasks from `Queue` (up to `MAX_OPEN_FILES` files at once), run
//! the backend's `prepare` and either hand the finished page to the writer or pass the prepared
//! request to a consumer through a bounded channel. Consumers do the slow model request.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::checkpoint::{self, Fingerprint, Manifest};
use crate::input::Item;
use crate::markdown::{self, FrontMatter, PageResult};
use crate::model_engine::{Fail, Prepared};
use crate::pipeline::{self, Settings};
use crate::progress::{self, Progress};
use crate::suspect;
use crate::verify;
use crate::vision::{self, DocKind};

/// Files whose pages are in the pool at the same time.
const MAX_OPEN_FILES: usize = 4;
/// Prepared pages the producers may run ahead of the consumers, per consumer (at least 4).
const LOOKAHEAD: usize = 4;

/// A page after the producer step.
pub enum Work {
    /// Finished without the slow step (apple engine, text layer).
    Done(PageResult),
    /// Needs the model request.
    Model(Prepared),
}

/// What differs between the apple engine and the model engines.
pub trait Backend: Sync {
    /// Front matter engine string.
    fn engine(&self) -> String;
    /// Engine name for the manifest.
    fn name(&self) -> String;
    /// (repo, mlx-vlm version) of a model engine.
    fn model_ids(&self) -> Option<(String, String)>;
    /// Threads for rendering and the Apple pass.
    fn producers(&self, s: &Settings) -> usize;
    /// Threads making model requests (0: none).
    fn consumers(&self, s: &Settings) -> usize;
    fn prepare(&self, path: &Path, kind: DocKind, index: usize, s: &Settings) -> Work;
    fn process(&self, path: &Path, prep: Prepared, s: &Settings) -> Result<PageResult, Fail>;
    /// Whether the run cannot continue (model server gone).
    fn stopped(&self) -> bool {
        false
    }
    /// The stderr note for transcript pages, if the engine has one.
    /// `pages`: every transcript page; `unverified`: those whose line numbers could not be
    /// re-anchored to the page geometry.
    fn transcript_note(&self, input: &Path, pages: &str, unverified: &str) -> Option<String>;
}

/// How one file ended.
pub struct FileOutcome {
    pub id: usize,
    pub pages: usize,
    /// Pages with an error, an engine flag or a suspect finding (`flagged_pages`).
    pub flagged: Vec<usize>,
    /// Pages whose OCR failed outright.
    pub failed: Vec<usize>,
    /// Pages the engine flagged (model fallbacks, bad tables).
    pub engine_flagged: Vec<usize>,
    /// Pages listed in `suspect_pages`.
    pub suspect: Vec<usize>,
    pub resumed: usize,
    /// Pages recognized in this run (not from the text layer).
    pub ocr: usize,
    /// Pages taken from the text layer in this run.
    pub text_layer: usize,
    /// Fallback pages in the output.
    pub fallback: usize,
    /// Pages that look like deposition transcripts.
    pub transcripts: Vec<usize>,
    /// Transcript pages whose line numbers could not be re-anchored (`line_numbers.unverified`).
    pub unverified: Vec<usize>,
    pub elapsed: Duration,
}

pub struct RunReport {
    pub outcomes: Vec<FileOutcome>,
    /// Files that could not be processed: (item index, message).
    pub failed: Vec<(usize, String)>,
    /// Set when the model server stopped; unfinished files keep their checkpoints.
    pub stopped: Option<String>,
}

struct FileInfo {
    id: usize,
    path: PathBuf,
    kind: DocKind,
}

struct Begin {
    id: usize,
    input: PathBuf,
    output: PathBuf,
    name: String,
    doc_pages: usize,
    /// 1-based numbers of the selected pages.
    selected: Vec<usize>,
    pending: usize,
    resumed: usize,
    manifest: Manifest,
    started: Instant,
}

enum Msg {
    Begin(Box<Begin>),
    Page { id: usize, page: PageResult },
    Stop,
}

// ------------------------------------------------------------------ queue

struct QState {
    next_item: usize,
    /// Files opened and not yet written.
    open: usize,
    /// Files with pages still to hand out, and those pages (0-based).
    active: VecDeque<(Arc<FileInfo>, VecDeque<usize>)>,
}

struct Queue<'a> {
    items: &'a [Item],
    backend: &'a dyn Backend,
    s: &'a Settings,
    progress: &'a Progress,
    writer: Sender<Msg>,
    failed: Mutex<Vec<(usize, String)>>,
    state: Mutex<QState>,
    cv: Condvar,
}

impl Queue<'_> {
    /// The next page to prepare, opening more files as room allows; `None` when nothing is left.
    fn next(&self, stop: &AtomicBool) -> Option<(Arc<FileInfo>, usize)> {
        let mut st = self.state.lock().unwrap();
        loop {
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            if let Some((info, pages)) = st.active.front_mut() {
                let index = pages.pop_front().expect("active files have pages");
                let info = info.clone();
                if pages.is_empty() {
                    st.active.pop_front();
                }
                return Some((info, index));
            }
            if st.next_item < self.items.len() && st.open < MAX_OPEN_FILES {
                let i = st.next_item;
                st.next_item += 1;
                st.open += 1;
                match self.open_file(i) {
                    Some((info, pages)) if !pages.is_empty() => {
                        st.active.push_back((info, pages));
                    }
                    Some(_) => {}
                    None => st.open -= 1,
                }
                continue;
            }
            if st.next_item >= self.items.len() {
                return None;
            }
            // Four files are open and every page of them is out: wait for one to be written.
            st = self
                .cv
                .wait_timeout(st, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    /// The writer finished a file.
    fn finished(&self) {
        self.state.lock().unwrap().open -= 1;
        self.cv.notify_all();
    }

    fn fail(&self, id: usize, msg: String) {
        self.failed.lock().unwrap().push((id, msg));
    }

    /// Reads the file, loads its checkpoints and tells the writer. Returns the pages to do.
    fn open_file(&self, id: usize) -> Option<(Arc<FileInfo>, VecDeque<usize>)> {
        let item = &self.items[id];
        let s = self.s;
        let info = match vision::document_info(&item.input) {
            Ok(i) => i,
            Err(e) => {
                self.fail(id, format!("cannot read {}: {e:#}", item.input.display()));
                return None;
            }
        };
        let pages = pipeline::select_pages(info.pages, s.pages.as_ref(), &item.input);
        let fingerprint = match checkpoint::fingerprint(&item.input) {
            Ok(f) => f,
            Err(e) => {
                self.fail(id, format!("{e:#}"));
                return None;
            }
        };
        let manifest = manifest(self.backend, s, fingerprint);
        if s.overwrite {
            let _ = std::fs::remove_file(&item.output);
        }
        let loaded = checkpoint::load(&item.output, &manifest, s.overwrite);
        for w in &loaded.warnings {
            progress::line(w);
        }
        let selected: Vec<usize> = pages.iter().map(|p| p + 1).collect();
        let pending: VecDeque<usize> = pages
            .iter()
            .copied()
            .filter(|p| !loaded.pages.contains_key(&(p + 1)))
            .collect();
        let resumed = selected.len() - pending.len();
        let name = item.input.file_name().map_or_else(
            || item.input.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.progress.add_total(selected.len());
        self.progress.inc(resumed);
        self.progress.file_start(id, &name, selected.len(), resumed);
        let _ = self.writer.send(Msg::Begin(Box::new(Begin {
            id,
            input: item.input.clone(),
            output: item.output.clone(),
            name,
            doc_pages: info.pages,
            selected,
            pending: pending.len(),
            resumed,
            manifest,
            started: Instant::now(),
        })));
        Some((
            Arc::new(FileInfo {
                id,
                path: item.input.clone(),
                kind: info.kind,
            }),
            pending,
        ))
    }
}

fn text_layer_name(s: &Settings) -> &'static str {
    s.text_layer.name()
}

fn manifest(backend: &dyn Backend, s: &Settings, fingerprint: Fingerprint) -> Manifest {
    let ids = backend.model_ids();
    Manifest {
        format: checkpoint::CHECKPOINT_FORMAT,
        crate_version: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint,
        engine: backend.name(),
        repo: ids.as_ref().map(|i| i.0.clone()),
        mlx_vlm: ids.map(|i| i.1),
        dpi: s.dpi,
        text_layer: text_layer_name(s).to_string(),
        currency_fix: s.currency_fix,
        failed_pages: Vec::new(),
    }
}

// ----------------------------------------------------------------- writer

struct Entry {
    b: Begin,
    remaining: usize,
    /// Pages without a record on disk: failed pages, and pages whose record could not be written.
    mem: BTreeMap<usize, PageResult>,
    ocr: usize,
    text_layer: usize,
}

type Done = Result<FileOutcome, (usize, String)>;

/// Owns every write under the partial dirs and the final outputs.
fn writer(
    rx: Receiver<Msg>,
    out: Sender<Done>,
    backend: &dyn Backend,
    s: &Settings,
    queue: &Queue,
) {
    let mut files: HashMap<usize, Entry> = HashMap::new();
    let mut checkpointed = 0;
    crate::supervisor::test_crash_hook(0);
    for msg in rx {
        let id = match msg {
            Msg::Stop => break,
            Msg::Begin(b) => {
                let id = b.id;
                let dir = checkpoint::partial_dir(&b.output);
                if let Err(e) = checkpoint::write_manifest(&dir, &b.manifest) {
                    progress::line(format!("warning: {e:#}; this file will not be resumable"));
                }
                files.insert(
                    id,
                    Entry {
                        remaining: b.pending,
                        b: *b,
                        mem: BTreeMap::new(),
                        ocr: 0,
                        text_layer: 0,
                    },
                );
                id
            }
            Msg::Page { id, page } => {
                let e = files.get_mut(&id).expect("pages follow their Begin");
                let dir = checkpoint::partial_dir(&e.b.output);
                if page.source == markdown::Source::TextLayer && page.error.is_none() {
                    e.text_layer += 1;
                } else {
                    e.ocr += 1;
                }
                if !checkpoint::checkpointable(&page) {
                    e.mem.insert(page.number, page);
                } else if let Err(err) = checkpoint::write_page(&dir, &page) {
                    progress::line(format!(
                        "warning: page {}: {err:#}; keeping it in memory",
                        page.number
                    ));
                    e.mem.insert(page.number, page);
                } else {
                    checkpointed += 1;
                    crate::supervisor::test_crash_hook(checkpointed);
                }
                e.remaining -= 1;
                queue.progress.inc(1);
                queue.progress.file_inc(id);
                id
            }
        };
        if files[&id].remaining == 0 {
            let entry = files.remove(&id).unwrap();
            queue.progress.file_done(id);
            let result = finish(entry, backend, s);
            let _ = out.send(result);
            queue.finished();
        }
    }
}

/// Assembles a file from its records and writes it atomically.
fn finish(mut e: Entry, backend: &dyn Backend, s: &Settings) -> Done {
    let id = e.b.id;
    let dir = checkpoint::partial_dir(&e.b.output);
    let mut results = Vec::with_capacity(e.b.selected.len());
    for &n in &e.b.selected {
        match e.mem.remove(&n).or_else(|| checkpoint::read_page(&dir, n)) {
            Some(p) => results.push(p),
            None => {
                return Err((
                    id,
                    format!("checkpoint of page {n} is missing or unreadable"),
                ));
            }
        }
    }
    let cleaned = s.legal_cleanup.then(|| markdown::strip_running(&results));
    let (reviewed, rewrites) = suspect::review(
        cleaned.as_ref().map_or(&results, |c| &c.0),
        s.symbol_fix,
        backend.model_ids().is_some(),
    );
    if s.verbose && !s.quiet {
        for (page, before, after) in &rewrites {
            eprintln!(
                "{}: page {page}: symbol fix {before:?} -> {after:?}",
                e.b.name
            );
        }
    }
    let front = FrontMatter {
        source: &e.b.name,
        pages: e.b.doc_pages,
        engine: &backend.engine(),
        generated: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        model: backend.model_ids().is_some(),
        legal_cleanup: cleaned.as_ref().map(|c| c.1),
    };
    let md = markdown::assemble(s.front_matter.then_some(&front), &reviewed, s.page_markers);
    pipeline::write_atomic(&e.b.output, &md).map_err(|err| (id, format!("{err:#}")))?;
    if let Some(mode) = s.verify {
        let input = e.b.input.clone();
        let report = verify::report(
            &e.b.name,
            e.b.output.display().to_string(),
            front.engine,
            &front.generated,
            &reviewed,
            mode,
            &|page| vision::text_layer(&input, page - 1).unwrap_or_default(),
        );
        let json = serde_json::to_string_pretty(&report).expect("report serializes");
        pipeline::write_atomic(&verify::sidecar(&e.b.output), &(json + "\n"))
            .map_err(|err| (id, format!("{err:#}")))?;
    }

    let failed: Vec<usize> = results
        .iter()
        .filter(|r| r.error.is_some())
        .map(|r| r.number)
        .collect();
    if failed.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
    } else {
        // Keep the good pages so a re-run only redoes the failed ones.
        let mut m = e.b.manifest.clone();
        m.failed_pages = failed.clone();
        if let Err(err) = checkpoint::write_manifest(&dir, &m) {
            progress::line(format!("warning: {err:#}"));
        }
    }
    Ok(FileOutcome {
        id,
        pages: results.len(),
        flagged: reviewed
            .iter()
            .filter(|r| r.needs_check())
            .map(|r| r.number)
            .collect(),
        failed,
        engine_flagged: reviewed
            .iter()
            .filter(|r| r.flagged && r.error.is_none())
            .map(|r| r.number)
            .collect(),
        suspect: reviewed
            .iter()
            .filter(|r| !r.suspect.is_empty())
            .map(|r| r.number)
            .collect(),
        resumed: e.b.resumed,
        ocr: e.ocr,
        text_layer: e.text_layer,
        fallback: results.iter().filter(|r| r.fallback).count(),
        transcripts: results
            .iter()
            .filter(|r| r.transcript)
            .map(|r| r.number)
            .collect(),
        unverified: results
            .iter()
            .filter(|r| r.transcript && r.line_numbers == Some(false))
            .map(|r| r.number)
            .collect(),
        elapsed: e.b.started.elapsed(),
    })
}

// -------------------------------------------------------------------- run

/// Runs every item through `backend`. `per_file_bars`: draw a bar per file (directory mode).
pub fn run(items: &[Item], backend: &dyn Backend, s: &Settings, per_file_bars: bool) -> RunReport {
    let started = Instant::now();
    let progress = Progress::new(s.quiet, per_file_bars);
    let (wtx, wrx) = mpsc::channel::<Msg>();
    let (otx, orx) = mpsc::channel::<Done>();
    let queue = Queue {
        items,
        backend,
        s,
        progress: &progress,
        writer: wtx.clone(),
        failed: Mutex::new(Vec::new()),
        state: Mutex::new(QState {
            next_item: 0,
            open: 0,
            active: VecDeque::new(),
        }),
        cv: Condvar::new(),
    };
    let stop = AtomicBool::new(false);
    let fatal: Mutex<Option<String>> = Mutex::new(None);
    let producers = backend.producers(s).max(1);
    let consumers = backend.consumers(s);
    let (work_tx, work_rx) =
        sync_channel::<(Arc<FileInfo>, Prepared)>(LOOKAHEAD.max(2 * consumers));
    let work_rx = Mutex::new(work_rx);
    let mut outcomes = Vec::new();
    let mut failed_writes = Vec::new();

    std::thread::scope(|scope| {
        let queue = &queue;
        let (stop, fatal, work_rx) = (&stop, &fatal, &work_rx);
        let writer_handle = scope.spawn(|| writer(wrx, otx, backend, s, queue));
        let mut handles = Vec::new();
        for _ in 0..producers {
            let tx = work_tx.clone();
            handles.push(scope.spawn(move || {
                while let Some((info, index)) = queue.next(stop) {
                    match backend.prepare(&info.path, info.kind, index, s) {
                        Work::Done(page) => {
                            let _ = queue.writer.send(Msg::Page { id: info.id, page });
                        }
                        Work::Model(prep) => {
                            if tx.send((info, prep)).is_err() {
                                break;
                            }
                        }
                    }
                }
            }));
        }
        drop(work_tx);
        for _ in 0..consumers {
            handles.push(scope.spawn(move || {
                loop {
                    let msg = work_rx.lock().unwrap().recv();
                    let Ok((info, prep)) = msg else { break };
                    // After a fatal error keep draining so no producer blocks on a full queue.
                    if stop.load(Ordering::SeqCst) || backend.stopped() {
                        continue;
                    }
                    match backend.process(&info.path, prep, s) {
                        Ok(page) => {
                            let _ = queue.writer.send(Msg::Page { id: info.id, page });
                        }
                        Err(Fail::Stopped(msg)) => {
                            stop.store(true, Ordering::SeqCst);
                            fatal.lock().unwrap().get_or_insert(msg);
                        }
                        Err(other) => {
                            stop.store(true, Ordering::SeqCst);
                            fatal.lock().unwrap().get_or_insert(format!("{other:?}"));
                        }
                    }
                }
            }));
        }
        let wtx = wtx;
        scope.spawn(move || {
            for h in handles {
                let _ = h.join();
            }
            let _ = wtx.send(Msg::Stop);
        });
        for done in orx {
            match done {
                Ok(o) => {
                    report_file(&items[o.id], &o, backend, s.quiet);
                    outcomes.push(o);
                }
                Err(f) => failed_writes.push(f),
            }
        }
        let _ = writer_handle.join();
    });
    progress.finish();

    let mut failed = queue.failed.into_inner().unwrap();
    failed.extend(failed_writes);
    failed.sort();
    outcomes.sort_by_key(|o| o.id);
    let stopped = fatal
        .into_inner()
        .unwrap()
        .or_else(|| backend.stopped().then(|| "server stopped".to_string()));
    let report = RunReport {
        outcomes,
        failed,
        stopped,
    };
    if !s.quiet {
        progress::line(summary(&report, started.elapsed()));
    }
    report
}

/// The per-file line and the transcript note.
fn report_file(item: &Item, o: &FileOutcome, backend: &dyn Backend, quiet: bool) {
    if !o.transcripts.is_empty() {
        let list = |v: &[usize]| {
            v.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(note) =
            backend.transcript_note(&item.input, &list(&o.transcripts), &list(&o.unverified))
        {
            progress::line(note);
        }
    }
    if !quiet {
        progress::line(format!(
            "{} -> {} ({} pages, {:.1}s)",
            item.input.display(),
            item.output.display(),
            o.pages,
            o.elapsed.as_secs_f64()
        ));
    }
}

fn summary(r: &RunReport, elapsed: Duration) -> String {
    let sum = |f: &dyn Fn(&FileOutcome) -> usize| r.outcomes.iter().map(f).sum::<usize>();
    let ocr = sum(&|o| o.ocr);
    let text = sum(&|o| o.text_layer);
    let secs = elapsed.as_secs_f64();
    let rate = if secs > 0.0 {
        (ocr + text) as f64 / secs
    } else {
        0.0
    };
    format!(
        "summary: {} file(s) | pages OCR'd {ocr} | resumed from checkpoint {} | text-layer {text} | fallback {} | flagged {} | {secs:.1}s ({rate:.1} pages/s)",
        r.outcomes.len(),
        sum(&|o| o.resumed),
        sum(&|o| o.fallback),
        sum(&|o| o.flagged.len()),
    )
}
