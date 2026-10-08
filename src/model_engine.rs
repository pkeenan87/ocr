//! PaddleOCR-VL scan engine (PLAN §4.2, §4.3, §5): request planning from the Apple side-pass,
//! the HTTP client, retries, fallback and ordered assembly.
//!
//! Model requests go to a server started with `--max-num-seqs 1`. A producer pool runs the
//! Apple side-pass and renders the images of upcoming pages while consumers (one per `--jobs`)
//! make the requests, one page at a time each.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::cli::perf_cores;
use crate::markdown::{self, FrontMatter, PageResult};
use crate::otsl;
use crate::pipeline::{self, FileReport, Settings, SidePass};
use crate::regions::{Kind, Region};
use crate::server::{self, ServerInfo};
use crate::vision::{self, Block, DocKind};

/// Per-request timeout: 2 x (prefill + 4096 tokens / 218 tok/s), docs/spike.md.
pub const DEFAULT_TIMEOUT_SECS: u64 = 45;
const MAX_TOKENS: u32 = 4096;
const RETRY_MAX_TOKENS: u32 = 8192;
const LOOP_PENALTY: f64 = 1.15;
const BACKOFF_SECS: [f64; 3] = [0.5, 1.0, 2.0];
/// Pages the producers may run ahead of the consumers.
const LOOKAHEAD: usize = 4;
/// Padding of band and table crops, as a fraction of the page.
const CROP_PAD: f64 = 0.01;

/// Repetition detector: unit length range, repeat count, and the tail it looks at.
const MIN_UNIT: usize = 10;
const MAX_UNIT: usize = 120;
const MIN_REPEATS: usize = 6;
const TAIL_CHARS: usize = 1500;
/// A repeated unit must hold this many distinct letters or digits (rules out `-----`, `. . .`).
const MIN_UNIT_VARIETY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    Ocr,
    Table,
}

impl Prompt {
    fn text(self) -> &'static str {
        match self {
            Self::Ocr => "OCR:",
            Self::Table => "Table Recognition:",
        }
    }
}

/// One planned request: the prompt and the normalized crop (`None` is the full page).
pub type Spec = (Prompt, Option<[f64; 4]>);

// ------------------------------------------------------------------ planning

/// The request plan for a page, and whether it is a column split.
pub fn choose_plan(side: &SidePass) -> (Vec<Spec>, bool) {
    if side.transcript {
        // Vision mislabels transcripts as tables, and crops would drop the line numbers.
        return (vec![(Prompt::Ocr, None)], false);
    }
    if side.columns() >= 2
        && let Some(specs) = region_specs(side)
    {
        return (specs, true);
    }
    match table_specs(&side.blocks) {
        Some(specs) => (specs, false),
        None => (vec![(Prompt::Ocr, None)], false),
    }
}

/// One `OCR:` request per band and column, in region order (margin strips are skipped); `None`
/// unless the page splits into at least two regions.
pub fn region_specs(side: &SidePass) -> Option<Vec<Spec>> {
    let specs: Vec<Spec> = side
        .regions
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r.kind {
            Kind::Margin => None,
            Kind::Column => side
                .crops
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, rect)| (Prompt::Ocr, Some(*rect))),
            Kind::Band => Some((Prompt::Ocr, Some(band_rect(&side.regions, i)))),
        })
        .collect();
    (specs.len() >= 2).then_some(specs)
}

/// A band's rect padded by `CROP_PAD`, clamped at the midpoints to its neighbours.
fn band_rect(regions: &[Region], i: usize) -> [f64; 4] {
    let r = regions[i].rect;
    let (rx0, rx1, ry0, ry1) = (r[0], r[0] + r[2], r[1], r[1] + r[3]);
    let (mut x0, mut x1) = ((rx0 - CROP_PAD).max(0.0), (rx1 + CROP_PAD).min(1.0));
    let (mut y0, mut y1) = ((ry0 - CROP_PAD).max(0.0), (ry1 + CROP_PAD).min(1.0));
    for (_, o) in regions.iter().enumerate().filter(|(j, _)| *j != i) {
        let (ox0, ox1, oy0, oy1) = (
            o.rect[0],
            o.rect[0] + o.rect[2],
            o.rect[1],
            o.rect[1] + o.rect[3],
        );
        if oy0 < ry1 && oy1 > ry0 {
            if ox1 <= rx0 {
                x0 = x0.max((ox1 + rx0) / 2.0);
            } else if ox0 >= rx1 {
                x1 = x1.min((rx1 + ox0) / 2.0);
            }
        } else if ox0 < rx1 && ox1 > rx0 {
            if oy1 <= ry0 {
                y0 = y0.max((oy1 + ry0) / 2.0);
            } else if oy0 >= ry1 {
                y1 = y1.min((ry1 + oy0) / 2.0);
            }
        }
    }
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

/// A table worth `Table Recognition:`: at least two rows and two columns that hold text.
fn usable_table(b: &Block) -> bool {
    let Some(rows) = b.rows.as_ref().filter(|_| b.kind == "table") else {
        return false;
    };
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let with_text = (0..width)
        .filter(|&c| {
            rows.iter()
                .any(|r| r.get(c).is_some_and(|s| !s.trim().is_empty()))
        })
        .count();
    rows.len() >= 2 && with_text >= 2
}

/// Full-width bands: `OCR:` for the text above, between and below the page's tables, and
/// `Table Recognition:` for each table crop. `None` if the page has no usable table.
pub fn table_specs(blocks: &[Block]) -> Option<Vec<Spec>> {
    let mut spans: Vec<(f64, f64)> = blocks
        .iter()
        .filter(|b| usable_table(b))
        .map(|b| (b.bbox[1], b.bbox[1] + b.bbox[3]))
        .collect();
    if spans.is_empty() {
        return None;
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (y0, y1) in spans {
        match merged.last_mut() {
            Some(last) if y0 <= last.1 => last.1 = last.1.max(y1),
            _ => merged.push((y0, y1)),
        }
    }
    let has_text = |y0: f64, y1: f64| {
        blocks.iter().any(|b| {
            let y = b.bbox[1] + b.bbox[3] / 2.0;
            !usable_table(b)
                && y >= y0
                && y < y1
                && (b.text.as_deref().is_some_and(|t| !t.trim().is_empty())
                    || b.items.is_some()
                    || b.rows.is_some())
        })
    };
    let band = |y0: f64, y1: f64| [0.0, y0, 1.0, y1 - y0];
    let mut specs = Vec::new();
    let mut cursor = 0.0_f64;
    for (y0, y1) in merged {
        let top = (y0 - CROP_PAD).max(cursor);
        let bottom = (y1 + CROP_PAD).min(1.0);
        if has_text(cursor, top) {
            specs.push((Prompt::Ocr, Some(band(cursor, top))));
        }
        specs.push((Prompt::Table, Some(band(top, bottom))));
        cursor = bottom;
    }
    if has_text(cursor, 1.0) {
        specs.push((Prompt::Ocr, Some(band(cursor, 1.0))));
    }
    Some(specs)
}

// -------------------------------------------------------- output processing

/// Whether the tail of `text` is a repetition loop: a unit of 10-120 characters repeated at
/// least six times in a row.
pub fn has_repetition_loop(text: &str) -> bool {
    let all: Vec<char> = text.chars().collect();
    let tail = &all[all.len().saturating_sub(TAIL_CHARS)..];
    let n = tail.len();
    for unit in MIN_UNIT..=MAX_UNIT {
        if n < unit * MIN_REPEATS {
            break;
        }
        let needed = unit * (MIN_REPEATS - 1);
        let mut run = 0;
        for i in 0..n - unit {
            if tail[i] != tail[i + unit] {
                run = 0;
                continue;
            }
            run += 1;
            if run >= needed {
                let start = i + 1 - needed;
                let mut letters: Vec<char> = tail[start..start + unit]
                    .iter()
                    .copied()
                    .filter(|c| c.is_alphanumeric())
                    .collect();
                letters.sort_unstable();
                letters.dedup();
                if letters.len() >= MIN_UNIT_VARIETY {
                    return true;
                }
                run = 0;
            }
        }
    }
    false
}

/// `OCR:` text as Markdown: trailing whitespace trimmed, runs of 3+ blank lines collapsed to
/// one, leading and trailing blank lines dropped.
pub fn clean_ocr(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blanks = 0;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for line in normalized.lines().map(str::trim_end) {
        if line.is_empty() {
            blanks += 1;
            continue;
        }
        if !out.is_empty() {
            let keep = if blanks >= 3 { 1 } else { blanks };
            out.extend(std::iter::repeat_n("", keep));
        }
        blanks = 0;
        out.push(line);
    }
    out.join("\n")
}

/// Markdown for one reply, whether it converted a table, and whether it needs a human look.
fn convert(prompt: Prompt, text: &str) -> (String, bool, bool) {
    match prompt {
        Prompt::Ocr => (clean_ocr(text), false, false),
        Prompt::Table => match otsl::otsl_to_gfm(text) {
            Ok(table) => (table, true, false),
            Err(_) => (format!("```\n{}\n```", text.trim()), false, true),
        },
    }
}

// ------------------------------------------------------------------ requests

/// Why a page's model run did not produce text.
#[derive(Debug)]
pub enum Fail {
    /// The run cannot continue (server stopped or unreachable): exit 3.
    Stopped(String),
    /// This page failed; use the Apple text.
    Failed(String),
    /// Loop or truncation, and a crop plan is available to try instead.
    Degenerate,
}

enum PostErr {
    Timeout,
    Conn(String),
    Status(u16, String),
    Bad(String),
}

struct Reply {
    text: String,
    finish: String,
}

/// A request with its image already rendered.
struct Planned {
    prompt: Prompt,
    url: String,
}

/// State shared by every consumer for the whole run.
pub struct Ctx {
    port: u16,
    repo: String,
    timeout: Duration,
    /// Restart generation and the server we currently talk to.
    state: Mutex<(u64, ServerInfo)>,
    stopped: AtomicBool,
    /// Front matter engine string.
    pub engine: String,
}

impl Ctx {
    pub fn new(info: ServerInfo, mlx_vlm: &str) -> Self {
        let secs = std::env::var("OCR_REQUEST_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        Self {
            port: info.port,
            engine: format!("paddleocr-vl ({}, mlx-vlm {mlx_vlm})", info.repo),
            repo: info.repo.clone(),
            timeout: Duration::from_secs(secs),
            state: Mutex::new((0, info)),
            stopped: AtomicBool::new(false),
        }
    }

    fn generation(&self) -> u64 {
        self.state.lock().unwrap().0
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Restarts the server unless another thread already did since generation `seen`.
    fn recover(&self, seen: u64, why: &str) -> Result<(), Fail> {
        let mut state = self.state.lock().unwrap();
        if self.is_stopped() {
            return Err(Fail::Stopped("server stopped".into()));
        }
        if state.0 != seen {
            return Ok(());
        }
        if !server::info_file_exists() {
            self.stopped.store(true, Ordering::SeqCst);
            return Err(Fail::Stopped("server stopped".into()));
        }
        eprintln!("note: {why}; restarting the model server");
        match server::restart(&state.1) {
            Ok(info) => {
                *state = (state.0 + 1, info);
                Ok(())
            }
            Err(e) => {
                self.stopped.store(true, Ordering::SeqCst);
                Err(Fail::Stopped(format!("model server unavailable: {e}")))
            }
        }
    }

    fn post(&self, req: &Planned, max_tokens: u32, penalty: Option<f64>) -> Result<Reply, PostErr> {
        let mut body = serde_json::json!({
            "model": self.repo,
            "temperature": 0,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": req.url}},
                {"type": "text", "text": req.prompt.text()},
            ]}],
        });
        if let Some(p) = penalty {
            body["repetition_penalty"] = p.into();
        }
        let classify = |e: ureq::Error| match e {
            ureq::Error::Timeout(_) => PostErr::Timeout,
            ureq::Error::Io(io)
                if matches!(
                    io.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                PostErr::Timeout
            }
            other => PostErr::Conn(other.to_string()),
        };
        let mut resp = server::agent(self.timeout)
            .post(format!(
                "http://127.0.0.1:{}/v1/chat/completions",
                self.port
            ))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(classify)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(classify)?;
        if status != 200 {
            let snippet: String = text.chars().take(200).collect();
            return Err(PostErr::Status(status, snippet));
        }
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| PostErr::Bad(format!("bad reply: {e}")))?;
        let choice = &json["choices"][0];
        let content = choice["message"]["content"]
            .as_str()
            .ok_or_else(|| PostErr::Bad("reply has no message content".into()))?;
        Ok(Reply {
            text: content.to_string(),
            finish: choice["finish_reason"]
                .as_str()
                .unwrap_or("stop")
                .to_string(),
        })
    }

    /// One request with the transport-level retries: restart and retry once on timeout, three
    /// tries on 5xx, one restart on a lost connection.
    fn send(&self, req: &Planned, max_tokens: u32, penalty: Option<f64>) -> Result<Reply, Fail> {
        let (mut timeouts, mut errors, mut lost) = (0, 0, 0);
        loop {
            if self.is_stopped() {
                return Err(Fail::Stopped("server stopped".into()));
            }
            let seen = self.generation();
            match self.post(req, max_tokens, penalty) {
                Ok(reply) => return Ok(reply),
                Err(PostErr::Timeout) => {
                    self.recover(seen, "request timed out")?;
                    timeouts += 1;
                    if timeouts >= 2 {
                        return Err(Fail::Failed("request timed out twice".into()));
                    }
                }
                Err(PostErr::Status(code, msg)) if code >= 500 => {
                    errors += 1;
                    if errors >= BACKOFF_SECS.len() {
                        return Err(Fail::Failed(format!("server error {code}: {msg}")));
                    }
                    std::thread::sleep(Duration::from_secs_f64(BACKOFF_SECS[errors - 1]));
                }
                Err(PostErr::Status(code, msg)) => {
                    return Err(Fail::Failed(format!("server returned HTTP {code}: {msg}")));
                }
                Err(PostErr::Conn(msg)) => {
                    if self.generation() != seen {
                        continue; // someone restarted the server under us
                    }
                    lost += 1;
                    if lost > 1 {
                        self.stopped.store(true, Ordering::SeqCst);
                        return Err(Fail::Stopped(format!("model server unreachable: {msg}")));
                    }
                    self.recover(seen, "lost the model server")?;
                }
                Err(PostErr::Bad(msg)) => return Err(Fail::Failed(msg)),
            }
        }
    }

    /// One request including the loop and truncation ladder. With `crop_alt` the first
    /// degenerate reply is reported so the caller can switch to crops; otherwise it retries
    /// once (8192 tokens after truncation, a repetition penalty after a loop).
    fn exec(&self, req: &Planned, crop_alt: bool) -> Result<String, Fail> {
        let (mut max_tokens, mut penalty, mut retried) = (MAX_TOKENS, None, false);
        loop {
            let reply = self.send(req, max_tokens, penalty)?;
            let truncated = reply.finish == "length";
            let looped = req.prompt == Prompt::Ocr && has_repetition_loop(&reply.text);
            if !truncated && !looped {
                return Ok(reply.text);
            }
            if crop_alt {
                return Err(Fail::Degenerate);
            }
            if retried {
                let what = if truncated {
                    "output truncated at max_tokens"
                } else {
                    "repetition loop"
                };
                return Err(Fail::Failed(what.into()));
            }
            retried = true;
            if truncated {
                max_tokens = RETRY_MAX_TOKENS;
            } else {
                penalty = Some(LOOP_PENALTY);
            }
        }
    }
}

struct Output {
    body: String,
    table: bool,
    flagged: bool,
}

fn run_planned(ctx: &Ctx, reqs: &[Planned], crop_alt: bool) -> Result<Output, Fail> {
    let mut out = Output {
        body: String::new(),
        table: false,
        flagged: false,
    };
    let mut parts = Vec::new();
    for req in reqs {
        let text = ctx.exec(req, crop_alt)?;
        let (md, table, flagged) = convert(req.prompt, &text);
        out.table |= table;
        out.flagged |= flagged;
        if !md.trim().is_empty() {
            parts.push(md);
        }
    }
    out.body = parts.join("\n\n");
    Ok(out)
}

// ------------------------------------------------------------- page pipeline

/// A page ready for the consumers.
struct Prepared {
    index: usize,
    bates: Option<String>,
    transcript: bool,
    column_split: bool,
    planned: Result<Vec<Planned>, String>,
    /// Region crops to try if the single full-page request loops or is cut off.
    alt: Option<Vec<Spec>>,
}

enum Work {
    Done(PageResult),
    Model(Prepared),
}

fn render_specs(
    path: &Path,
    index: usize,
    dpi: u32,
    specs: &[Spec],
) -> Result<Vec<Planned>, String> {
    specs
        .iter()
        .map(|(prompt, crop)| {
            vision::render_png(path, index, dpi, *crop)
                .map(|png| Planned {
                    prompt: *prompt,
                    url: format!("data:image/png;base64,{}", server::base64(&png)),
                })
                .map_err(|e| format!("{e:#}"))
        })
        .collect()
}

/// Producer step: text layer, Apple side-pass, request plan and rendered images.
fn prepare(path: &Path, kind: DocKind, index: usize, s: &Settings) -> Work {
    if let Some(page) = pipeline::text_layer_page(path, kind, index, s) {
        return Work::Done(page);
    }
    let full: Vec<Spec> = vec![(Prompt::Ocr, None)];
    let (specs, split, alt, bates, transcript) =
        match pipeline::side_pass(path, index, s.dpi, &s.options) {
            Ok(side) => {
                let (specs, split) = choose_plan(&side);
                let alt = (specs == full && !side.transcript)
                    .then(|| region_specs(&side))
                    .flatten();
                (
                    specs,
                    split,
                    alt,
                    markdown::find_bates(&side.blocks),
                    side.transcript,
                )
            }
            Err(_) => (full, false, None, None, false),
        };
    if s.verbose && !s.quiet {
        let kinds: Vec<&str> = specs.iter().map(|(p, _)| p.text()).collect();
        let alt = alt.as_ref().map_or(0, Vec::len);
        eprintln!(
            "page {}: plan {kinds:?}{}{}",
            index + 1,
            if transcript { " (transcript)" } else { "" },
            if alt > 0 {
                format!(" (crop fallback: {alt})")
            } else {
                String::new()
            }
        );
    }
    Work::Model(Prepared {
        index,
        bates,
        transcript,
        column_split: split,
        planned: render_specs(path, index, s.dpi, &specs),
        alt,
    })
}

fn recognize(path: &Path, prep: &Prepared, s: &Settings, ctx: &Ctx) -> Result<Output, Fail> {
    let planned = prep.planned.as_ref().map_err(|e| Fail::Failed(e.clone()))?;
    match run_planned(ctx, planned, prep.alt.is_some()) {
        Err(Fail::Degenerate) => {
            let specs = prep.alt.as_deref().unwrap_or_default();
            let crops = render_specs(path, prep.index, s.dpi, specs).map_err(Fail::Failed)?;
            run_planned(ctx, &crops, false)
        }
        other => other,
    }
}

fn process(path: &Path, prep: Prepared, s: &Settings, ctx: &Ctx) -> Result<PageResult, Fail> {
    let number = prep.index + 1;
    let started = Instant::now();
    let result = match recognize(path, &prep, s, ctx) {
        Ok(out) => PageResult {
            body: out.body,
            bates: prep.bates.clone(),
            column_split: prep.column_split,
            transcript: prep.transcript,
            table: out.table,
            flagged: out.flagged,
            ..blank(number)
        },
        Err(Fail::Failed(reason)) => {
            eprintln!("warning: page {number}: {reason}; using the Apple Vision text");
            let mut page = pipeline::vision_page(path, prep.index, s);
            if let Some(err) = page.error.take() {
                page.error = Some(format!("{reason}; Apple Vision fallback failed: {err}"));
            } else {
                page.fallback = true;
                page.flagged = true;
            }
            page.transcript |= prep.transcript;
            page
        }
        Err(Fail::Degenerate) => unreachable!("degenerate replies are retried inside recognize"),
        Err(stopped) => return Err(stopped),
    };
    if s.verbose && !s.quiet {
        eprintln!("page {number}: {:.1}s", started.elapsed().as_secs_f64());
    }
    Ok(result)
}

fn blank(number: usize) -> PageResult {
    PageResult {
        number,
        source: markdown::Source::Vision,
        body: String::new(),
        bates: None,
        column_split: false,
        transcript: false,
        currency_fixes: 0,
        table: false,
        fallback: false,
        flagged: false,
        error: None,
    }
}

/// Errors that end the run.
#[derive(Debug)]
pub enum RunError {
    /// The server was stopped or cannot be reached: exit 3.
    Stopped(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for RunError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}

/// Converts one file with the model server and writes its Markdown atomically.
pub fn run_file(
    item: &crate::input::Item,
    s: &Settings,
    ctx: &Ctx,
) -> Result<FileReport, RunError> {
    let started = Instant::now();
    let info = vision::document_info(&item.input)
        .with_context(|| format!("cannot read {}", item.input.display()))?;
    let pages = pipeline::select_pages(info.pages, s.pages.as_ref(), &item.input);
    let results = process_pages(&item.input, info.kind, &pages, s, ctx)?;

    let name = item.input.file_name().map_or_else(
        || item.input.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let front = FrontMatter {
        source: &name,
        pages: info.pages,
        engine: &ctx.engine,
        generated: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        model: true,
    };
    let md = markdown::assemble(s.front_matter.then_some(&front), &results, s.page_markers);
    pipeline::write_atomic(&item.output, &md)?;
    if !s.quiet {
        eprintln!(
            "{} -> {} ({} pages, {:.1}s)",
            item.input.display(),
            item.output.display(),
            results.len(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(FileReport {
        pages: results.len(),
        flagged: results
            .iter()
            .filter(|r| r.error.is_some() || r.flagged)
            .map(|r| r.number)
            .collect(),
    })
}

/// Producers prepare pages in the background (bounded look-ahead) while `jobs` consumers make
/// the requests; results land in page order.
fn process_pages(
    path: &Path,
    kind: DocKind,
    pages: &[usize],
    s: &Settings,
    ctx: &Ctx,
) -> Result<Vec<PageResult>, RunError> {
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<PageResult>>> = Mutex::new(vec![None; pages.len()]);
    let fatal: Mutex<Option<String>> = Mutex::new(None);
    let (tx, rx) = sync_channel::<(usize, Work)>(LOOKAHEAD);
    let rx = Mutex::new(rx);
    let producers = perf_cores().clamp(1, pages.len().max(1));
    let consumers = s.jobs.clamp(1, pages.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..producers {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                while !ctx.is_stopped() {
                    let slot = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&index) = pages.get(slot) else { break };
                    if tx.send((slot, prepare(path, kind, index, s))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for _ in 0..consumers {
            scope.spawn(|| {
                loop {
                    let msg: Result<(usize, Work), mpsc::RecvError> = rx.lock().unwrap().recv();
                    let Ok((slot, work)) = msg else { break };
                    // After a fatal error keep draining so no producer blocks on a full queue.
                    if ctx.is_stopped() {
                        continue;
                    }
                    let page = match work {
                        Work::Done(page) => Ok(page),
                        Work::Model(prep) => process(path, prep, s, ctx),
                    };
                    match page {
                        Ok(page) => slots.lock().unwrap()[slot] = Some(page),
                        Err(Fail::Stopped(msg)) => {
                            ctx.stopped.store(true, Ordering::SeqCst);
                            fatal.lock().unwrap().get_or_insert(msg);
                        }
                        Err(other) => {
                            ctx.stopped.store(true, Ordering::SeqCst);
                            fatal.lock().unwrap().get_or_insert(format!("{other:?}"));
                        }
                    }
                }
            });
        }
    });
    if let Some(msg) = fatal.into_inner().unwrap() {
        return Err(RunError::Stopped(msg));
    }
    if ctx.is_stopped() {
        return Err(RunError::Stopped("server stopped".into()));
    }
    Ok(slots
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.expect("every queued page produces a result"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(
        kind: &str,
        bbox: [f64; 4],
        text: Option<&str>,
        rows: Option<Vec<Vec<&str>>>,
    ) -> Block {
        Block {
            kind: kind.into(),
            bbox,
            text: text.map(String::from),
            items: None,
            rows: rows.map(|r| {
                r.into_iter()
                    .map(|r| r.into_iter().map(String::from).collect())
                    .collect()
            }),
        }
    }

    fn side(blocks: Vec<Block>, regions: Vec<Region>, transcript: bool) -> SidePass {
        let crops = crate::regions::crop_rects(&regions, 1.0, 0.02);
        SidePass {
            blocks,
            regions,
            crops,
            transcript,
        }
    }

    fn region(rect: [f64; 4], kind: Kind) -> Region {
        Region { rect, kind }
    }

    #[test]
    fn loops_are_detected() {
        assert!(has_repetition_loop(&"the same line again\n".repeat(30)));
        let tail = format!(
            "Real text first. {}",
            "the part of the case under the obligation ".repeat(8)
        );
        assert!(has_repetition_loop(&tail));
        // A partial last repeat still counts.
        let cut = format!(
            "{}the part of",
            "the part of the case under the obligation ".repeat(7)
        );
        assert!(has_repetition_loop(&cut));
        // Unicode does not panic.
        assert!(has_repetition_loop(&"naïve café résumé — ".repeat(10)));
    }

    #[test]
    fn normal_text_is_not_a_loop() {
        let legal = "WHEREAS, the Parties wish to settle all claims arising out of the Agreement dated \
            March 3, 2025, and the Defendant denies any liability whatsoever; NOW, THEREFORE, in \
            consideration of the mutual covenants herein, the Parties agree as follows. The Plaintiff \
            shall pay $48,250.00 within thirty (30) days. The Defendant shall deliver the goods \
            described in Schedule A. Each party bears its own costs.";
        assert!(!has_repetition_loop(legal));
        let list: String = (1..=120)
            .map(|n| format!("{n}. The witness identified exhibit {n} as a true copy.\n"))
            .collect();
        assert!(!has_repetition_loop(&list));
        let transcript: String = (1..=25)
            .map(|n| {
                format!(
                    "{n:>2}  {}. Line {n} of the testimony\n",
                    if n % 2 == 1 { "Q" } else { "A" }
                )
            })
            .collect();
        assert!(!has_repetition_loop(&transcript));
        // Rules, dot leaders and separator rows are not loops.
        assert!(!has_repetition_loop(&"-".repeat(300)));
        assert!(!has_repetition_loop(&format!(
            "Item 4{}12",
            ". ".repeat(80)
        )));
        assert!(!has_repetition_loop(&format!("|{}", " --- |".repeat(40))));
        assert!(!has_repetition_loop(""));
    }

    #[test]
    fn ocr_cleanup() {
        assert_eq!(
            clean_ocr("a  \r\n\r\n\r\n\r\n\r\nb\n\n\nc\n\n"),
            "a\n\nb\n\n\nc"
        );
        assert_eq!(clean_ocr("\n\nx"), "x");
        assert_eq!(clean_ocr("   \n"), "");
    }

    #[test]
    fn table_reply_falls_back_to_a_fence() {
        let (md, table, flagged) = convert(Prompt::Table, "<fcel>a<fcel>b<nl><fcel>1<fcel>2<nl>");
        assert!(md.starts_with("| a | b |") && table && !flagged);
        let (md, table, flagged) = convert(Prompt::Table, "garbage text");
        assert_eq!(md, "```\ngarbage text\n```");
        assert!(!table && flagged);
    }

    #[test]
    fn plan_transcript_is_one_full_page_request() {
        let table = block(
            "table",
            [0.1, 0.1, 0.8, 0.8],
            None,
            Some(vec![vec!["1", "Q."], vec!["2", "A."]]),
        );
        let cols = vec![
            region([0.1, 0.1, 0.35, 0.8], Kind::Column),
            region([0.55, 0.1, 0.35, 0.8], Kind::Column),
        ];
        let (specs, split) = choose_plan(&side(vec![table], cols, true));
        assert_eq!(specs, vec![(Prompt::Ocr, None)]);
        assert!(!split);
    }

    #[test]
    fn plan_columns_skip_margins() {
        let regions = vec![
            region([0.02, 0.1, 0.04, 0.8], Kind::Margin),
            region([0.1, 0.1, 0.8, 0.05], Kind::Band),
            region([0.1, 0.2, 0.35, 0.7], Kind::Column),
            region([0.55, 0.2, 0.35, 0.7], Kind::Column),
        ];
        let (specs, split) = choose_plan(&side(vec![], regions, false));
        assert!(split);
        assert_eq!(specs.len(), 3);
        assert!(specs.iter().all(|(p, c)| *p == Prompt::Ocr && c.is_some()));
        // Band first (reading order), padded by 1% and clamped below at the gutter midpoint.
        let band = specs[0].1.unwrap();
        assert!((band[1] - 0.09).abs() < 1e-9 && band[1] + band[3] <= 0.175);
    }

    #[test]
    fn plan_tables_split_into_bands() {
        let blocks = vec![
            block("paragraph", [0.1, 0.05, 0.8, 0.05], Some("Intro"), None),
            block(
                "table",
                [0.1, 0.3, 0.8, 0.2],
                None,
                Some(vec![vec!["Invoice", "Date"], vec!["INV-1", "May"]]),
            ),
            block("paragraph", [0.1, 0.7, 0.8, 0.05], Some("Outro"), None),
        ];
        let (specs, split) = choose_plan(&side(blocks, vec![], false));
        assert!(!split);
        let kinds: Vec<Prompt> = specs.iter().map(|s| s.0).collect();
        assert_eq!(kinds, [Prompt::Ocr, Prompt::Table, Prompt::Ocr]);
        let t = specs[1].1.unwrap();
        assert!((t[1] - 0.29).abs() < 1e-9 && (t[1] + t[3] - 0.51).abs() < 1e-9);
        assert_eq!((t[0], t[2]), (0.0, 1.0));
    }

    #[test]
    fn plan_ignores_tiny_and_empty_tables() {
        let one_col = block(
            "table",
            [0.1, 0.1, 0.8, 0.5],
            None,
            Some(vec![vec!["a", ""], vec!["b", ""]]),
        );
        let one_row = block(
            "table",
            [0.1, 0.1, 0.8, 0.5],
            None,
            Some(vec![vec!["a", "b"]]),
        );
        for b in [one_col, one_row] {
            let (specs, _) = choose_plan(&side(vec![b], vec![], false));
            assert_eq!(specs, vec![(Prompt::Ocr, None)]);
        }
    }

    #[test]
    fn plan_merges_overlapping_tables_and_skips_empty_bands() {
        let t = |y: f64, h: f64| {
            block(
                "table",
                [0.1, y, 0.8, h],
                None,
                Some(vec![vec!["a", "b"], vec!["c", "d"]]),
            )
        };
        let (specs, _) = choose_plan(&side(vec![t(0.1, 0.3), t(0.3, 0.3)], vec![], false));
        // No text anywhere: just the one merged table crop.
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].0, Prompt::Table);
    }
}
