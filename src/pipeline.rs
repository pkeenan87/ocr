//! The apple engine's page pipeline: text layer or Vision with column handling.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::TextLayer;
use crate::input::PageRanges;
use crate::layout;
use crate::markdown::{self, PageResult, Source};
use crate::model_engine::{Fail, Prepared};
use crate::progress;
use crate::regions::{self, Kind};
use crate::run::{Backend, Work};
use crate::vision::{self, Block, DocKind, Options};

pub const ENGINE_NAME: &str = "apple (Vision RecognizeDocuments)";

/// Resolution of the grayscale render that region detection runs on.
const REGION_DPI: u32 = 100;
/// Padding of a column crop, as fractions of the page: Vision reads crops best with generous
/// white space. Clamped at the neighbouring gutter midpoints and bands, so the horizontal pad
/// reaches the midpoint of the gutter. Tuned on held-out seeds 0-9: a 1% horizontal pad
/// misread words on `two_column.pdf` that the midpoint pad reads exactly.
pub(crate) const COLUMN_PAD_X: f64 = 1.0;
pub(crate) const COLUMN_PAD_Y: f64 = 0.02;

/// Run-wide settings.
pub struct Settings {
    pub dpi: u32,
    /// `--jobs`: pages in flight (apple) or the cap on model requests (model engines).
    pub jobs: usize,
    /// Threads for rendering and the Apple pass of model engines.
    pub side_threads: usize,
    pub overwrite: bool,
    pub pages: Option<PageRanges>,
    pub text_layer: TextLayer,
    pub page_markers: bool,
    pub front_matter: bool,
    pub currency_fix: bool,
    /// `--legal-cleanup`: strip running headers and footers at assembly.
    pub legal_cleanup: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub options: Options,
}

/// The apple engine: every page is finished in the producer step.
pub struct Apple;

impl Backend for Apple {
    fn engine(&self) -> String {
        ENGINE_NAME.to_string()
    }

    fn name(&self) -> String {
        "apple".to_string()
    }

    fn model_ids(&self) -> Option<(String, String)> {
        None
    }

    fn producers(&self, s: &Settings) -> usize {
        s.jobs
    }

    fn consumers(&self, _: &Settings) -> usize {
        0
    }

    fn prepare(&self, path: &Path, kind: DocKind, index: usize, s: &Settings) -> Work {
        Work::Done(process_page(path, kind, index, s))
    }

    fn process(&self, _: &Path, _: Prepared, _: &Settings) -> Result<PageResult, Fail> {
        Err(Fail::Failed("the apple engine has no model step".into()))
    }

    fn transcript_note(&self, input: &Path, pages: &str) -> Option<String> {
        Some(format!(
            "note: {} looks like a deposition transcript (pages {pages}); Apple Vision drops line numbers and misreads Q./A. — use --model paddleocr-vl",
            input.display()
        ))
    }
}

/// 0-based indices to process: the `--pages` subset, with out-of-range pages warned about.
pub(crate) fn select_pages(total: usize, ranges: Option<&PageRanges>, input: &Path) -> Vec<usize> {
    let Some(ranges) = ranges else {
        return (0..total).collect();
    };
    for &(a, b) in &ranges.0 {
        if b as usize > total {
            let span = if a == b {
                format!("page {a}")
            } else {
                format!("pages {a}-{b}")
            };
            progress::line(format!(
                "warning: {span} out of range ({} has {total} pages); skipping missing pages",
                input.display()
            ));
        }
    }
    (0..total)
        .filter(|i| ranges.contains(*i as u32 + 1))
        .collect()
}

fn process_page(path: &Path, kind: DocKind, index: usize, s: &Settings) -> PageResult {
    text_layer_page(path, kind, index, s).unwrap_or_else(|| vision_page(path, index, s))
}

/// The page from its embedded text, if `--use-text-layer` asks for it and the text is usable.
pub(crate) fn text_layer_page(
    path: &Path,
    kind: DocKind,
    index: usize,
    s: &Settings,
) -> Option<PageResult> {
    if kind != DocKind::Pdf || s.text_layer == TextLayer::Never {
        return None;
    }
    let text = vision::text_layer(path, index).ok()?;
    let usable = match s.text_layer {
        TextLayer::Always => !text.trim().is_empty(),
        _ => markdown::text_layer_usable(&text),
    };
    usable.then(|| PageResult {
        source: Source::TextLayer,
        body: markdown::render_text_layer(&text),
        ..blank_page(index + 1)
    })
}

fn blank_page(number: usize) -> PageResult {
    PageResult {
        number,
        source: Source::Vision,
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

/// The page through Apple Vision with column handling, as the apple engine renders it.
pub(crate) fn vision_page(path: &Path, index: usize, s: &Settings) -> PageResult {
    let mut result = blank_page(index + 1);
    match recognize_with_columns(path, index, s.dpi, &s.options) {
        Ok(page) => {
            let (body, bates) = if page.split {
                markdown::render_ordered(&page.blocks)
            } else {
                markdown::render_blocks(&page.blocks)
            };
            result.column_split = page.split;
            result.transcript = page.transcript;
            result.bates = bates;
            (result.body, result.currency_fixes) = if s.currency_fix {
                markdown::fix_currency(&body)
            } else {
                (body, 0)
            };
        }
        Err(e) => result.error = Some(format!("{e:#}")),
    }
    result
}

/// Layout of one recognized page.
pub struct Recognized {
    pub blocks: Vec<Block>,
    /// Whether columns were re-recognized region by region. Blocks are then already in
    /// reading order.
    pub split: bool,
    /// Whether the page looks like a deposition transcript (flagged only).
    pub transcript: bool,
}

/// First Vision pass and region detection of one page.
pub struct SidePass {
    /// Blocks of the full-page Vision pass.
    pub blocks: Vec<Block>,
    /// Regions in reading order; empty if the grayscale render failed.
    pub regions: Vec<regions::Region>,
    /// Padded crop rectangles of the column regions, as Apple crops them.
    pub crops: Vec<(usize, [f64; 4])>,
    /// Whether the full-page blocks look like a deposition transcript.
    pub transcript: bool,
}

impl SidePass {
    pub fn columns(&self) -> usize {
        self.regions
            .iter()
            .filter(|r| r.kind == Kind::Column)
            .count()
    }
}

/// Vision pass over the full page, then region detection on a grayscale render. A failed
/// render only costs the region handling, never the page. Off macOS there is no Vision: the
/// regions come from the bitmap alone, without table masks, and there are no blocks (so no
/// table crops, Bates numbers or transcript detection from them; PLAN §9.5).
pub fn side_pass(path: &Path, index: usize, dpi: u32, options: &Options) -> Result<SidePass> {
    #[cfg(target_os = "macos")]
    let page = vision::recognize_page(path, index, dpi, options)?;
    #[cfg(not(target_os = "macos"))]
    let page = {
        let _ = (dpi, options);
        vision::Page {
            width: 0,
            height: 0,
            blocks: Vec::new(),
        }
    };
    let tables: Vec<[f64; 4]> = page
        .blocks
        .iter()
        .filter(|b| b.kind == "table")
        .map(|b| b.bbox)
        .collect();
    let regions = vision::render_gray(path, index, REGION_DPI)
        .map(|gray| regions::detect(&gray, &tables))
        .unwrap_or_default();
    let crops = regions::crop_rects(&regions, COLUMN_PAD_X, COLUMN_PAD_Y);
    Ok(SidePass {
        transcript: layout::looks_like_transcript(&page.blocks),
        blocks: page.blocks,
        regions,
        crops,
    })
}

/// Vision pass over the full page, then region detection on a grayscale render. If the page
/// has two or more column regions, each is cropped and recognized on its own (Vision merges
/// same-baseline lines across columns) and bands are taken from the first pass.
pub fn recognize_with_columns(
    path: &Path,
    index: usize,
    dpi: u32,
    options: &Options,
) -> Result<Recognized> {
    let side = side_pass(path, index, dpi, options)?;
    if side.columns() < 2 {
        return Ok(Recognized {
            transcript: side.transcript,
            blocks: side.blocks,
            split: false,
        });
    }
    let SidePass {
        blocks: first_blocks,
        regions,
        crops,
        transcript,
    } = side;
    let first_pass = |blocks: Vec<Block>| Recognized {
        transcript,
        blocks,
        split: false,
    };
    let mut recognized: Vec<Vec<Block>> = Vec::new();
    for (_, rect) in &crops {
        let crop = Options {
            crop: Some(*rect),
            ..options.clone()
        };
        match vision::recognize_page(path, index, dpi, &crop) {
            Ok(col) => recognized.push(drop_duplicate_titles(col.blocks)),
            // Keep the full-page reading rather than lose the page.
            Err(_) => return Ok(first_pass(first_blocks)),
        }
    }
    let ordered = assemble_regions(&regions, &crops, recognized, &first_blocks);
    Ok(Recognized {
        transcript: layout::looks_like_transcript(&ordered) || transcript,
        blocks: ordered,
        split: true,
    })
}

/// Region-ordered blocks: each column's own recognition, and the first-pass blocks that fall
/// in each band. Margin strips are dropped, and so are first-pass blocks that a column crop
/// replaces.
fn assemble_regions(
    regions: &[regions::Region],
    crops: &[(usize, [f64; 4])],
    recognized: Vec<Vec<Block>>,
    first_pass: &[Block],
) -> Vec<Block> {
    let inside = |r: &[f64; 4], x: f64, y: f64| {
        x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
    };
    let mut bands: Vec<(usize, Vec<Block>)> = regions
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == Kind::Band)
        .map(|(i, _)| (i, Vec::new()))
        .collect();
    let mut strays = Vec::new();
    for b in first_pass {
        let (x, y) = (b.bbox[0] + b.bbox[2] / 2.0, b.bbox[1] + b.bbox[3] / 2.0);
        if crops.iter().any(|(_, r)| inside(r, x, y))
            || regions
                .iter()
                .any(|r| r.kind == Kind::Margin && inside(&r.rect, x, y))
        {
            continue;
        }
        // The band containing the block's centre, else the nearest one vertically.
        let dist = |i: usize| {
            let r = regions[i].rect;
            (r[1] - y).max(y - (r[1] + r[3])).max(0.0)
        };
        match bands
            .iter_mut()
            .min_by(|a, c| dist(a.0).total_cmp(&dist(c.0)))
        {
            Some((_, v)) => v.push(b.clone()),
            None => strays.push(b.clone()),
        }
    }
    let mut columns = recognized.into_iter();
    let mut out = Vec::new();
    for (i, region) in regions.iter().enumerate() {
        let blocks = match region.kind {
            Kind::Margin => continue,
            Kind::Column => columns.next().unwrap_or_default(),
            Kind::Band => bands
                .iter_mut()
                .find(|(j, _)| *j == i)
                .map(|(_, v)| std::mem::take(v))
                .unwrap_or_default(),
        };
        out.extend(
            layout::reading_order(&blocks)
                .into_iter()
                .map(|k| blocks[k].clone()),
        );
    }
    out.extend(strays);
    out
}

/// Writes `<out>.tmp` next to `out`, then renames it into place.
pub(crate) fn write_atomic(out: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, contents).with_context(|| format!("cannot write {}", out.display()))?;
    crate::checkpoint::replace_file(Path::new(&tmp), out)
        .with_context(|| format!("cannot write {}", out.display()))
}

/// A crop can make Vision call a column's first line a title while also returning it inside
/// the paragraph below; drop such titles.
fn drop_duplicate_titles(blocks: Vec<Block>) -> Vec<Block> {
    let dup = |t: &Block| {
        let text = t.text.as_deref().unwrap_or("").trim();
        blocks.iter().any(|p| {
            p.kind == "paragraph"
                && p.text.as_deref().is_some_and(|pt| pt.contains(text))
                && p.bbox[1] <= t.bbox[1] + 1e-3
                && p.bbox[1] + p.bbox[3] >= t.bbox[1] + t.bbox[3] - 1e-3
        })
    };
    let keep: Vec<bool> = blocks
        .iter()
        .map(|b| b.kind != "title" || !dup(b))
        .collect();
    blocks
        .into_iter()
        .zip(keep)
        .filter_map(|(b, k)| k.then_some(b))
        .collect()
}
