//! Model-server scan engines (PLAN §4.2, §4.3, §5): request planning from the Apple side-pass,
//! the HTTP client, retries, fallback and ordered assembly. What differs per engine (prompts,
//! message order, image sizing, output parser, retry ladder) sits behind `ScanEngine`: PaddleOCR-VL
//! is here, olmOCR in `olmocr.rs`, dots in `dots.rs`.
//!
//! Model requests go to a server started with `--max-num-seqs N`. In-flight requests are capped
//! at min(`--jobs`, N) by a counting gate, and each request also holds a file lock in the state
//! dir, so a second `ocr scan` on the same server waits for its turn instead of queueing inside
//! the server. A request's timeout clock starts only once it holds both.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::anchor::{self, MiniPage};
use crate::cli::Engine;
use crate::config::state_dir;
use crate::engines::{self, Limits};
use crate::markdown::{self, PageResult};
use crate::otsl;
use crate::pipeline::{self, Settings, SidePass};
use crate::progress;
use crate::regions::{Kind, Region};
use crate::run::{Backend, Work};
use crate::server::{self, ServerInfo};
use crate::telemetry;
use crate::vision::{self, Block, DocKind};

/// `repetition_penalty` of a loop retry (never used for dots: it corrupts the JSON).
pub(crate) const LOOP_PENALTY: f64 = 1.15;
const BACKOFF_SECS: [f64; 3] = [0.5, 1.0, 2.0];
/// Padding of band and table crops, as a fraction of the page.
const CROP_PAD: f64 = 0.01;
/// Least resolution of the mini-page crops of a transcript sheet. Tuned on eval seeds 0-2
/// (`transcript4up`, Paddle): lines exact 90.4% at 150, 93.5% at 175, 95.4% at 200, then worse
/// (90.2% at 225 with a failed seed, 75.8% at 250, 84.9% at 300: the model drops or repeats lines).
const TRANSCRIPT_CROP_DPI: u32 = 200;

/// Repetition detector: unit length range, repeat count, and the tail it looks at.
const MIN_UNIT: usize = 10;
const MAX_UNIT: usize = 120;
const MIN_REPEATS: usize = 6;
const TAIL_CHARS: usize = 1500;
/// A repeated unit must hold this many distinct letters or digits (rules out `-----`, `. . .`).
const MIN_UNIT_VARIETY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// PaddleOCR-VL full page or crop.
    Ocr,
    /// PaddleOCR-VL table crop.
    Table,
    /// olmOCR `build_no_anchoring_v4_yaml_prompt()`.
    Olmocr,
    /// dots `prompt_layout_all_en`.
    DotsLayout,
    /// dots `prompt_ocr`.
    DotsOcr,
}

impl Prompt {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Ocr => "OCR:",
            Self::Table => "Table Recognition:",
            Self::Olmocr => crate::olmocr::PROMPT,
            Self::DotsLayout => crate::dots::PROMPT_LAYOUT,
            Self::DotsOcr => crate::dots::PROMPT_OCR,
        }
    }

    /// Short name for verbose plans.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ocr => "OCR:",
            Self::Table => "Table Recognition:",
            Self::Olmocr => "olmocr",
            Self::DotsLayout => "dots layout",
            Self::DotsOcr => "dots ocr",
        }
    }
}

/// What differs between the model engines.
pub trait ScanEngine: Sync {
    fn engine(&self) -> Engine;
    /// The prompt of a whole-page (or whole-crop) request.
    fn page_prompt(&self) -> Prompt;
    /// `Some(n)`: every image sent, crops included, is exactly `n` px on its longest side
    /// (rendered from the source, not derived from a DPI). `None`: render at `--dpi`.
    fn max_side(&self) -> Option<u32> {
        None
    }
    /// Whether the prompt comes before the image in the message (olmOCR, as upstream).
    fn text_first(&self) -> bool {
        false
    }
    /// The request plan of a page and whether it is a column split.
    fn plan(&self, side: &SidePass) -> (Vec<Spec>, bool);
    /// Runs the plan with the engine's retry ladder. `Fail::Failed` means: use Apple's text.
    fn run(&self, ctx: &Ctx, path: &Path, prep: &Prepared, s: &Settings) -> Result<Output, Fail>;
}

struct Paddle;

impl ScanEngine for Paddle {
    fn engine(&self) -> Engine {
        Engine::PaddleocrVl
    }

    fn page_prompt(&self) -> Prompt {
        Prompt::Ocr
    }

    /// Off macOS the bitmap regions have no Vision table masks, so table columns and 4-up
    /// transcript panels look like text columns, and splitting them garbled tables (52% CER) and
    /// transcripts (44%) on accept seeds 10-12. The full page is read first instead, and the
    /// region crops are kept for a loop or truncation, as for olmOCR (PLAN §9.7).
    fn plan(&self, side: &SidePass) -> (Vec<Spec>, bool) {
        if cfg!(not(target_os = "macos")) {
            return (vec![(Prompt::Ocr, None)], false);
        }
        choose_plan(side)
    }

    fn run(&self, ctx: &Ctx, path: &Path, prep: &Prepared, s: &Settings) -> Result<Output, Fail> {
        let planned = prep.planned.as_ref().map_err(|e| Fail::Failed(e.clone()))?;
        match run_planned(ctx, planned, prep.alt.is_some()) {
            Err(Fail::Degenerate) => {
                let specs = prep.alt.as_deref().unwrap_or_default();
                let crops =
                    render_specs(path, prep.index, s.dpi, specs, self).map_err(Fail::Failed)?;
                run_planned(ctx, &crops, false)
            }
            other => other,
        }
    }
}

/// The engine behind `engine`; `None` for Apple.
pub fn scan_engine(engine: Engine) -> Option<&'static dyn ScanEngine> {
    match engine {
        Engine::PaddleocrVl => Some(&Paddle),
        Engine::Olmocr => Some(&crate::olmocr::Olmocr),
        Engine::DotsOcr => Some(&crate::dots::Dots),
        Engine::Apple | Engine::IndraOcr => None,
    }
}

/// One planned request: the prompt and the normalized crop (`None` is the full page).
pub type Spec = (Prompt, Option<[f64; 4]>);

// ------------------------------------------------------------------ planning

/// The request plan for a page, and whether it is a column split.
pub fn choose_plan(side: &SidePass) -> (Vec<Spec>, bool) {
    if side.transcript {
        // Vision mislabels transcripts as tables, and column crops would drop the line numbers.
        // A multi-up sheet is sent as one crop per mini-page, in `Page N` order: whole
        // sheets make Paddle invent text.
        if let Some(crops) = anchor::crops(&side.mini) {
            return (
                crops.into_iter().map(|c| (Prompt::Ocr, Some(c))).collect(),
                true,
            );
        }
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
        // The other engines' prompts never reach this converter (they parse their own replies).
        Prompt::Ocr | Prompt::Olmocr | Prompt::DotsLayout | Prompt::DotsOcr => {
            (clean_ocr(text), false, false)
        }
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

pub(crate) struct Reply {
    pub(crate) text: String,
    pub(crate) finish: String,
    /// The server's `usage` and `timings`, when it sends them (telemetry only).
    metrics: telemetry::ServerMetrics,
}

impl PostErr {
    fn outcome(&self) -> String {
        match self {
            Self::Timeout => "timeout".into(),
            Self::Conn(m) => format!("connection: {m}"),
            Self::Status(code, _) => format!("http {code}"),
            Self::Bad(m) => m.clone(),
        }
    }
}

/// A request with its image already rendered.
#[derive(Clone)]
pub(crate) struct Planned {
    pub(crate) prompt: Prompt,
    pub(crate) url: String,
    pub(crate) text_first: bool,
}

impl Planned {
    /// A request from PNG bytes.
    pub(crate) fn new(prompt: Prompt, png: &[u8], text_first: bool) -> Self {
        Self {
            prompt,
            url: format!("data:image/png;base64,{}", server::base64(png)),
            text_first,
        }
    }
}

/// State shared by every consumer for the whole run.
pub struct Ctx {
    port: u16,
    repo: String,
    /// The engine's request timeouts (`OCR_REQUEST_TIMEOUT_SECS` overrides both).
    timeout: Duration,
    retry_timeout: Duration,
    pub(crate) lim: Limits,
    pub spec: &'static dyn ScanEngine,
    /// Restart generation and the server we currently talk to.
    state: Mutex<(u64, ServerInfo)>,
    stopped: AtomicBool,
    /// Front matter engine string.
    pub engine: String,
    /// The most requests this run keeps in flight: min(`--jobs`, the server's `--max-num-seqs`).
    in_flight: usize,
    /// Free request slots.
    permits: Mutex<usize>,
    permit_freed: Condvar,
    mlx_vlm: String,
    /// Per document: Bates corner probes that found nothing, or `BATES_FOUND` (olmOCR off
    /// macOS, `want_bates_corners`).
    bates_misses: Mutex<HashMap<PathBuf, u32>>,
}

/// `bates_misses` value of a document known to carry Bates numbers.
const BATES_FOUND: u32 = u32::MAX;
/// Pages of a document probed for a Bates corner, with nothing found, before probing stops.
const BATES_PROBE_PAGES: u32 = 3;

/// Bottom-right corner, bottom-left corner, then bottom centre of the page (normalized top-left
/// [x, y, w, h]), where Bates numbers are stamped. olmOCR drops a whole footer strip as page
/// furniture, and dots' `prompt_ocr` leaves out Page-footer text, but both read a corner on its
/// own (docs/spike.md, "olmOCR footer strip" and "dots Bates corners"). The order is fixed so a
/// page's result does not depend on the pages read before it.
const BATES_CORNERS: [[f64; 4]; 3] = [
    [0.7, 0.9, 0.3, 0.1],
    [0.0, 0.9, 0.3, 0.1],
    [0.3, 0.9, 0.4, 0.1],
];

/// Requests the server runs at once: its recorded `slots` (llama-server), else `--max-num-seqs`
/// from its recorded argv (mlx-vlm), else 1.
pub fn max_num_seqs(info: &ServerInfo) -> usize {
    info.slots
        .or_else(|| {
            info.argv
                .iter()
                .position(|a| a == "--max-num-seqs")
                .and_then(|i| info.argv.get(i + 1))
                .and_then(|v| v.parse().ok())
        })
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// A request slot: one of the `in_flight` permits of this process, and a lock file of the
/// server that no other `ocr` process holds. Dropping it frees both.
struct Slot<'a> {
    ctx: &'a Ctx,
    _lock: Option<File>,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        // The lock file closes (and unlocks) first, then the permit is returned.
        self._lock.take();
        *self.ctx.permits.lock().unwrap() += 1;
        self.ctx.permit_freed.notify_one();
    }
}

/// Locks one of `slots` files `server.lock`, `server.lock.1`, ... in the state dir (an
/// exclusive `File::lock`), waiting until one is free. With one slot this is a plain blocking exclusive lock.
/// `None` if the state dir is unusable (the run then relies on its own permits).
fn lock_server(slots: usize) -> Option<File> {
    let dir = state_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let open = |k: usize| {
        let name = if k == 0 {
            "server.lock".to_string()
        } else {
            format!("server.lock.{k}")
        };
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(name))
            .ok()
    };
    if slots <= 1 {
        let f = open(0)?;
        loop {
            match f.lock() {
                Ok(()) => return Some(f),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }
    loop {
        for k in 0..slots {
            let f = open(k)?;
            if f.try_lock().is_ok() {
                return Some(f);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Ctx {
    pub fn new(
        spec: &'static dyn ScanEngine,
        info: ServerInfo,
        mlx_vlm: &str,
        jobs: usize,
    ) -> Self {
        let in_flight = jobs.min(max_num_seqs(&info)).max(1);
        let mut lim = engines::limits(spec.engine());
        // The timeouts come from Apple Silicon rates; llama.cpp's CPU build (runtime id
        // `llama.cpp <build> cpu`) gets four times as long (PLAN §9.7).
        if mlx_vlm.ends_with(" cpu") {
            lim.timeout_secs *= 4;
            lim.retry_timeout_secs *= 4;
        }
        let over: Option<u64> = std::env::var("OCR_REQUEST_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|s| *s > 0);
        Self {
            port: info.port,
            engine: format!(
                "{} ({}, {})",
                spec.engine().name(),
                info.repo,
                if mlx_vlm.starts_with("llama.cpp") {
                    mlx_vlm.to_string()
                } else {
                    format!("mlx-vlm {mlx_vlm}")
                }
            ),
            repo: info.repo.clone(),
            timeout: Duration::from_secs(over.unwrap_or(lim.timeout_secs)),
            retry_timeout: Duration::from_secs(over.unwrap_or(lim.retry_timeout_secs)),
            lim,
            spec,
            state: Mutex::new((0, info)),
            stopped: AtomicBool::new(false),
            in_flight,
            permits: Mutex::new(in_flight),
            permit_freed: Condvar::new(),
            mlx_vlm: mlx_vlm.to_string(),
            bates_misses: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a page of `path` without a Bates number in its text is worth extra corner
    /// requests: yes while the document has shown Bates numbers, or until
    /// `BATES_PROBE_PAGES` of its pages were probed for nothing.
    pub(crate) fn want_bates_corners(&self, path: &Path) -> bool {
        let misses = self.bates_misses.lock().unwrap_or_else(|e| e.into_inner());
        misses
            .get(path)
            .is_none_or(|&n| n == BATES_FOUND || n < BATES_PROBE_PAGES)
    }

    /// Records whether a page of `path` had a Bates number (from its text or a corner probe).
    pub(crate) fn note_bates(&self, path: &Path, found: bool) {
        let mut misses = self.bates_misses.lock().unwrap_or_else(|e| e.into_inner());
        let n = misses.entry(path.to_path_buf()).or_insert(0);
        if found {
            *n = BATES_FOUND;
        } else if *n != BATES_FOUND {
            *n += 1;
        }
    }

    /// Off macOS, where no Vision side-pass reads the footer: when the page's own text has no
    /// Bates number, asks `probe` for each of `BATES_CORNERS` in turn and returns the first
    /// number found. A corner that fails or reads nothing is skipped; only a stopped server ends
    /// the page. Corner requests cost up to two model calls a page, so a document stops getting
    /// them once its first pages showed none (`want_bates_corners`).
    pub(crate) fn corner_bates(
        &self,
        path: &Path,
        prep: &Prepared,
        body: &str,
        mut probe: impl FnMut([f64; 4]) -> Result<Option<String>, Fail>,
    ) -> Result<Option<String>, Fail> {
        if cfg!(target_os = "macos") || prep.bates().is_some() {
            return Ok(None);
        }
        if markdown::find_bates_in_text(body).is_some() {
            self.note_bates(path, true);
            return Ok(None);
        }
        if !self.want_bates_corners(path) {
            return Ok(None);
        }
        for crop in BATES_CORNERS {
            match probe(crop) {
                Ok(Some(bates)) => {
                    self.note_bates(path, true);
                    return Ok(Some(bates));
                }
                Err(stopped @ Fail::Stopped(_)) => return Err(stopped),
                _ => {}
            }
        }
        self.note_bates(path, false);
        Ok(None)
    }

    /// Waits for a request slot. The timeout clock of a request must start after this returns.
    fn acquire(&self) -> Slot<'_> {
        let mut free = self.permits.lock().unwrap();
        while *free == 0 {
            free = self.permit_freed.wait(free).unwrap();
        }
        *free -= 1;
        drop(free);
        Slot {
            ctx: self,
            _lock: lock_server(self.in_flight),
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    fn generation(&self) -> u64 {
        self.state.lock().unwrap().0
    }

    pub(crate) fn is_stopped(&self) -> bool {
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
        progress::line(format!("note: {why}; restarting the model server"));
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
        let mut content = vec![
            serde_json::json!({"type": "image_url", "image_url": {"url": req.url}}),
            serde_json::json!({"type": "text", "text": req.prompt.text()}),
        ];
        if req.text_first {
            content.reverse();
        }
        let mut body = serde_json::json!({
            "model": self.repo,
            "temperature": 0,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": content}],
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
        let timeout = if max_tokens > self.lim.max_tokens {
            self.retry_timeout
        } else {
            self.timeout
        };
        let mut resp = server::agent(timeout)
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
            metrics: if telemetry::enabled() {
                telemetry::parse_metrics(&json)
            } else {
                Default::default()
            },
        })
    }

    /// Telemetry for one attempt (only called when telemetry is on).
    fn note(
        &self,
        req: &Planned,
        key: u64,
        (max_tokens, penalty): (u32, Option<f64>),
        reason: &'static str,
        wall: Duration,
        res: &Result<Reply, PostErr>,
    ) {
        let (outcome, finish, metrics, empty) = match res {
            Ok(r) => (
                "ok".to_string(),
                Some(r.finish.clone()),
                r.metrics.clone(),
                Some(r.text.trim().is_empty()),
            ),
            Err(e) => (e.outcome(), None, Default::default(), None),
        };
        telemetry::record(
            key,
            telemetry::Request {
                label: req.prompt.label().to_string(),
                image_bytes: req.url.len(),
                wall_s: wall.as_secs_f64(),
                reason,
                outcome,
                finish_reason: finish,
                max_tokens,
                repetition_penalty: penalty,
                usage: metrics.usage,
                timings: metrics.timings,
            },
            empty,
        );
    }

    /// One request with the transport-level retries: restart and retry once on timeout, three
    /// tries on 5xx, one restart on a lost connection.
    pub(crate) fn send(
        &self,
        req: &Planned,
        max_tokens: u32,
        penalty: Option<f64>,
    ) -> Result<Reply, Fail> {
        let (mut timeouts, mut errors, mut lost) = (0, 0, 0);
        let timed = telemetry::enabled();
        let key = if timed {
            telemetry::image_key(&req.url)
        } else {
            0
        };
        // Why the next attempt is made; None: work it out from the previous attempt.
        let mut transport: Option<&'static str> = None;
        loop {
            if self.is_stopped() {
                return Err(Fail::Stopped("server stopped".into()));
            }
            // Held across the request and any restart it causes: a restart is only ever done by
            // the process that holds the server lock.
            let slot = self.acquire();
            let seen = self.generation();
            let started = Instant::now();
            let res = self.post(req, max_tokens, penalty);
            if timed {
                let reason = transport.take().unwrap_or_else(|| {
                    telemetry::reason(key, max_tokens, self.lim.max_tokens, penalty)
                });
                self.note(
                    req,
                    key,
                    (max_tokens, penalty),
                    reason,
                    started.elapsed(),
                    &res,
                );
            }
            match res {
                Ok(reply) => return Ok(reply),
                Err(PostErr::Timeout) => {
                    self.recover(seen, "request timed out")?;
                    transport = Some("after_restart");
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
                    drop(slot);
                    transport = Some("transport_retry");
                    std::thread::sleep(Duration::from_secs_f64(BACKOFF_SECS[errors - 1]));
                }
                Err(PostErr::Status(code, msg)) => {
                    return Err(Fail::Failed(format!("server returned HTTP {code}: {msg}")));
                }
                Err(PostErr::Conn(msg)) => {
                    transport = Some("after_restart");
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
        let (mut max_tokens, mut penalty, mut retried) = (self.lim.max_tokens, None, false);
        let mut empty_retried = false;
        loop {
            let reply = self.send(req, max_tokens, penalty)?;
            // Empty output: retry once; a page that stays empty falls back (`run_planned`).
            if reply.text.trim().is_empty() && reply.finish != "length" && !empty_retried {
                empty_retried = true;
                continue;
            }
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
                max_tokens = self.lim.retry_max_tokens;
            } else {
                penalty = Some(LOOP_PENALTY);
            }
        }
    }
}

pub struct Output {
    pub(crate) body: String,
    pub(crate) table: bool,
    pub(crate) flagged: bool,
    /// A Bates number the engine read itself (olmOCR's corner requests off macOS).
    pub(crate) bates: Option<String>,
}

fn run_planned(ctx: &Ctx, reqs: &[Planned], crop_alt: bool) -> Result<Output, Fail> {
    let mut out = Output {
        body: String::new(),
        table: false,
        flagged: false,
        bates: None,
    };
    let mut parts = Vec::new();
    for req in reqs {
        let text = ctx.exec(req, crop_alt)?;
        let (md, table, flagged) = convert(req.prompt, &text);
        out.table |= table;
        out.flagged |= flagged;
        if !md.trim().is_empty() {
            parts.push(md);
        } else if reqs.len() > 1 {
            // A crop that stays empty after its retry while others have text: a column or
            // table band may be missing from the page.
            out.flagged = true;
        }
    }
    out.body = parts.join("\n\n");
    if out.body.trim().is_empty() {
        return Err(Fail::Failed("empty model output".into()));
    }
    Ok(out)
}

// ------------------------------------------------------------- page pipeline

/// A page ready for the consumers.
pub struct Prepared {
    pub(crate) index: usize,
    bates: Option<String>,
    /// The filing-stamp line Apple Vision read at the top of the page (macOS only).
    stamp: Option<String>,
    pub(crate) transcript: bool,
    /// Transcript pages: the mini-pages with their Vision lines.
    mini: Vec<MiniPage>,
    column_split: bool,
    pub(crate) planned: Result<Vec<Planned>, String>,
    /// Region crops to try if the single full-page request loops or is cut off.
    pub(crate) alt: Option<Vec<Spec>>,
}

impl Prepared {
    /// The Bates number the Apple side-pass found (macOS only).
    pub(crate) fn bates(&self) -> Option<&str> {
        self.bates.as_deref()
    }
}

/// Renders the images of `specs` the way `spec` wants them (`--dpi`, or an exact longest side).
pub(crate) fn render_specs(
    path: &Path,
    index: usize,
    dpi: u32,
    specs: &[Spec],
    spec: &dyn ScanEngine,
) -> Result<Vec<Planned>, String> {
    specs
        .iter()
        .map(|(prompt, crop)| {
            vision::render_png_fit(path, index, dpi, *crop, spec.max_side(), 0)
                .map(|png| Planned::new(*prompt, &png, spec.text_first()))
                .map_err(|e| format!("{e:#}"))
        })
        .collect()
}

/// Producer step: text layer, Apple side-pass, request plan and rendered images.
fn prepare(path: &Path, kind: DocKind, index: usize, s: &Settings, ctx: &Ctx) -> Work {
    if let Some(page) = pipeline::text_layer_page(path, kind, index, s) {
        return Work::Done(page);
    }
    let engine = ctx.spec;
    let prompt = engine.page_prompt();
    let full: Vec<Spec> = vec![(prompt, None)];
    let (specs, split, alt, bates, stamp, transcript, mini) =
        match pipeline::side_pass(path, index, s.dpi, &s.options) {
            Ok(side) => {
                let (specs, split) = engine.plan(&side);
                let alt = (specs == full && !side.transcript)
                    .then(|| region_specs(&side))
                    .flatten()
                    .map(|v| v.into_iter().map(|(_, crop)| (prompt, crop)).collect());
                (
                    specs,
                    split,
                    alt,
                    markdown::find_bates(&side.blocks),
                    markdown::find_court_stamp(&side.blocks),
                    side.transcript,
                    side.mini,
                )
            }
            Err(_) => (full, false, None, None, None, false, Vec::new()),
        };
    if s.verbose && !s.quiet {
        let kinds: Vec<&str> = specs.iter().map(|(p, _)| p.label()).collect();
        let alt = alt.as_ref().map_or(0, Vec::len);
        progress::line(format!(
            "page {}: plan {kinds:?}{}{}",
            index + 1,
            if transcript { " (transcript)" } else { "" },
            if alt > 0 {
                format!(" (crop fallback: {alt})")
            } else {
                String::new()
            }
        ));
    }
    // Mini-page crops of a transcript are rendered at no less than TRANSCRIPT_CROP_DPI: Paddle
    // drops or repeats lines in 8 pt print at its 150 DPI default (docs/spike.md).
    let dpi = if split && transcript {
        s.dpi.max(TRANSCRIPT_CROP_DPI)
    } else {
        s.dpi
    };
    Work::Model(Prepared {
        index,
        bates,
        stamp,
        transcript,
        mini,
        column_split: split,
        planned: render_specs(path, index, dpi, &specs, engine),
        alt,
    })
}

fn process(path: &Path, prep: Prepared, s: &Settings, ctx: &Ctx) -> Result<PageResult, Fail> {
    let number = prep.index + 1;
    let started = Instant::now();
    let result = match ctx.spec.run(ctx, path, &prep, s) {
        Ok(out) => {
            let (body, line_numbers) = if prep.transcript {
                // The engine's words keep their order; the numbers come from the page geometry.
                match anchor::apply(&prep.mini, Some(&out.body)) {
                    Some(a) => (a.body, Some(a.anchored)),
                    None => (out.body, Some(false)),
                }
            } else {
                (out.body, None)
            };
            let body = markdown::restore_court_stamp(body, prep.stamp.as_deref());
            PageResult {
                // Off macOS there is no Vision side-pass: the Bates number comes from the
                // model's own text, which keeps the footer, or from olmOCR's corner requests
                // (PLAN §9.5).
                bates: prep.bates.clone().or(out.bates).or_else(|| {
                    cfg!(not(target_os = "macos"))
                        .then(|| markdown::find_bates_in_text(&body))
                        .flatten()
                }),
                body,
                column_split: prep.column_split && !prep.transcript,
                transcript: prep.transcript,
                table: out.table,
                flagged: out.flagged,
                line_numbers,
                ..blank(number)
            }
        }
        Err(Fail::Failed(reason)) if cfg!(not(target_os = "macos")) => {
            // No Apple Vision text to fall back to: the page fails, and a re-run retries it.
            progress::line(format!("warning: page {number}: {reason}"));
            PageResult {
                transcript: prep.transcript,
                error: Some(reason),
                ..blank(number)
            }
        }
        Err(Fail::Failed(reason)) => {
            progress::line(format!(
                "warning: page {number}: {reason}; using the Apple Vision text"
            ));
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
        progress::line(format!(
            "page {number}: {:.1}s",
            started.elapsed().as_secs_f64()
        ));
    }
    Ok(result)
}

pub(crate) fn blank(number: usize) -> PageResult {
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
        line_numbers: None,
        symbol_fixes: 0,
        embedded_fixes: None,
        suspect: Vec::new(),
        error: None,
    }
}

impl Backend for Ctx {
    fn engine(&self) -> String {
        self.engine.clone()
    }

    fn name(&self) -> String {
        self.spec.engine().name().to_string()
    }

    fn model_ids(&self) -> Option<(String, String)> {
        Some((self.repo.clone(), self.mlx_vlm.clone()))
    }

    fn producers(&self, s: &Settings) -> usize {
        s.side_threads
    }

    fn consumers(&self, s: &Settings) -> usize {
        s.jobs
    }

    fn prepare(&self, path: &Path, kind: DocKind, index: usize, s: &Settings) -> Work {
        prepare(path, kind, index, s, self)
    }

    fn process(&self, path: &Path, prep: Prepared, s: &Settings) -> Result<PageResult, Fail> {
        let page = process(path, prep, s, self);
        if matches!(page, Err(Fail::Stopped(_))) {
            self.stopped.store(true, Ordering::SeqCst);
        }
        page
    }

    fn stopped(&self) -> bool {
        self.is_stopped()
    }

    fn transcript_note(&self, input: &Path, _: &str, unverified: &str) -> Option<String> {
        (!unverified.is_empty()).then(|| {
            format!(
                "note: {}: transcript pages {unverified} could not be re-anchored to page geometry (line_numbers.unverified); verify page:line citations against the PDF",
                input.display()
            )
        })
    }
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
            mini: Vec::new(),
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
