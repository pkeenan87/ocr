//! Per-file OCR pipeline for the apple engine: page planning, a worker pool, ordered assembly.

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::cli::TextLayer;
use crate::input::{Item, PageRanges};
use crate::layout;
use crate::markdown::{self, FrontMatter, PageResult, Source};
use crate::vision::{self, Block, DocKind, Options};

pub const ENGINE_NAME: &str = "apple (Vision RecognizeDocuments)";

/// Run-wide settings.
pub struct Settings {
    pub dpi: u32,
    pub jobs: usize,
    pub pages: Option<PageRanges>,
    pub text_layer: TextLayer,
    pub page_markers: bool,
    pub front_matter: bool,
    pub currency_fix: bool,
    pub quiet: bool,
    pub options: Options,
}

/// Outcome of one file.
pub struct FileReport {
    pub pages: usize,
    /// 1-based numbers of pages whose OCR failed.
    pub flagged: Vec<usize>,
}

/// Converts one planned file and writes its Markdown atomically.
pub fn run_file(item: &Item, s: &Settings) -> Result<FileReport> {
    let started = Instant::now();
    let info = vision::document_info(&item.input)
        .with_context(|| format!("cannot read {}", item.input.display()))?;
    let pages = select_pages(info.pages, s.pages.as_ref(), &item.input);
    let results = process_pages(&item.input, info.kind, &pages, s);

    let name = item.input.file_name().map_or_else(
        || item.input.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let front = FrontMatter {
        source: &name,
        pages: info.pages,
        engine: ENGINE_NAME,
        generated: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    };
    let md = markdown::assemble(s.front_matter.then_some(&front), &results, s.page_markers);
    write_atomic(&item.output, &md)?;

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
            .filter(|r| r.error.is_some())
            .map(|r| r.number)
            .collect(),
    })
}

/// 0-based indices to process: the `--pages` subset, with out-of-range pages warned about.
fn select_pages(total: usize, ranges: Option<&PageRanges>, input: &Path) -> Vec<usize> {
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
            eprintln!(
                "warning: {span} out of range ({} has {total} pages); skipping missing pages",
                input.display()
            );
        }
    }
    (0..total)
        .filter(|i| ranges.contains(*i as u32 + 1))
        .collect()
}

/// Processes pages on `jobs` threads pulling from a shared queue; results are in page order.
fn process_pages(path: &Path, kind: DocKind, pages: &[usize], s: &Settings) -> Vec<PageResult> {
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<PageResult>>> = Mutex::new(vec![None; pages.len()]);
    std::thread::scope(|scope| {
        for _ in 0..s.jobs.clamp(1, pages.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let slot = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&index) = pages.get(slot) else { break };
                    let result = process_page(path, kind, index, s);
                    slots.lock().unwrap()[slot] = Some(result);
                }
            });
        }
    });
    slots
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.expect("every queued page produces a result"))
        .collect()
}

fn process_page(path: &Path, kind: DocKind, index: usize, s: &Settings) -> PageResult {
    let number = index + 1;
    if kind == DocKind::Pdf
        && s.text_layer != TextLayer::Never
        && let Ok(text) = vision::text_layer(path, index)
    {
        let usable = match s.text_layer {
            TextLayer::Always => !text.trim().is_empty(),
            _ => markdown::text_layer_usable(&text),
        };
        if usable {
            return PageResult {
                number,
                source: Source::TextLayer,
                body: markdown::render_text_layer(&text),
                bates: None,
                column_split: false,
                currency_fixes: 0,
                error: None,
            };
        }
    }
    let mut result = PageResult {
        number,
        source: Source::Vision,
        body: String::new(),
        bates: None,
        column_split: false,
        currency_fixes: 0,
        error: None,
    };
    match recognize_with_columns(path, index, s.dpi, &s.options) {
        Ok((blocks, split)) => {
            let (body, bates) = markdown::render_blocks(&blocks);
            result.column_split = split;
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

/// Vision pass over the full page; if the layout has two or more column regions, each region is
/// cropped and recognized on its own (Vision merges same-baseline lines across columns).
/// Returns the blocks and whether columns were split.
pub fn recognize_with_columns(
    path: &Path,
    index: usize,
    dpi: u32,
    options: &Options,
) -> Result<(Vec<Block>, bool)> {
    let page = vision::recognize_page(path, index, dpi, options)?;
    let kept: Vec<Block> = page
        .blocks
        .iter()
        .filter(|b| !markdown::is_pleading_margin_number(b))
        .cloned()
        .collect();
    let Some(columns) = layout::find_columns(&kept) else {
        return Ok((page.blocks, false));
    };
    let mut blocks: Vec<Block> = kept
        .iter()
        .enumerate()
        .filter(|(i, _)| !columns.covered.contains(i))
        .map(|(_, b)| b.clone())
        .collect();
    for rect in columns.rects {
        let crop = Options {
            crop: Some(rect),
            ..options.clone()
        };
        match vision::recognize_page(path, index, dpi, &crop) {
            Ok(col) => blocks.extend(drop_duplicate_titles(col.blocks)),
            // Keep the full-page reading rather than lose the page.
            Err(_) => return Ok((page.blocks, false)),
        }
    }
    Ok((blocks, true))
}

/// Writes `<out>.tmp` next to `out`, then renames it into place.
fn write_atomic(out: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, contents).with_context(|| format!("cannot write {}", out.display()))?;
    fs::rename(&tmp, out).with_context(|| format!("cannot write {}", out.display()))
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
