//! Page types shared by every engine, and the page functions behind them. On macOS these are
//! the Swift bridge (`bridge/`) to PDFKit and Apple Vision's `RecognizeDocumentsRequest`;
//! elsewhere pdfium and the `image` crate read documents and Apple Vision does not exist
//! (PLAN §9).

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// One list entry; the marker is kept apart from the text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItem {
    pub marker: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Block {
    pub kind: String,
    /// Normalized [x, y, w, h], origin top-left.
    pub bbox: [f64; 4],
    pub text: Option<String>,
    pub items: Option<Vec<ListItem>>,
    pub rows: Option<Vec<Vec<String>>>,
}

/// One physical text line as Vision reads it (`DocumentObservation.Container.Text.lines`).
#[derive(Debug, Clone, Deserialize)]
pub struct Line {
    pub text: String,
    /// Normalized [x, y, w, h], origin top-left.
    pub bbox: [f64; 4],
}

/// Text-recognition settings passed to Vision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub languages: Vec<String>,
    pub auto_detect_language: bool,
    pub language_correction: bool,
    pub custom_words: Vec<String>,
    /// Normalized crop [x, y, w, h] (top-left origin) of the rendered page to recognize.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f64; 4]>,
    /// Also return the line-level observations (`Page::lines`); used on transcript candidates.
    pub lines: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            languages: vec!["en-US".into()],
            auto_detect_language: false,
            // Correction turned `cv` into `cr` in the spike; no DPI/fixture difference since (spike matrix).
            language_correction: false,
            custom_words: Vec::new(),
            crop: None,
            lines: false,
        }
    }
}

/// Recognized content of one rendered page.
#[derive(Debug)]
pub struct Page {
    /// Rendered bitmap size in pixels.
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<Block>,
    /// Physical text lines; empty unless `Options::lines` was set.
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Pdf,
    Image,
}

#[derive(Debug, Clone, Copy)]
pub struct DocInfo {
    pub kind: DocKind,
    pub pages: usize,
}

/// An 8-bit grayscale bitmap, one byte per pixel, row 0 at the top.
#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Renders a page (optionally cropped to a normalized top-left [x, y, w, h]) as PNG bytes.
pub fn render_png(
    path: &Path,
    page_index: usize,
    dpi: u32,
    crop: Option<[f64; 4]>,
) -> Result<Vec<u8>> {
    render_png_fit(path, page_index, dpi, crop, None, 0)
}

#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "macos")]
pub use apple::*;

#[cfg(not(target_os = "macos"))]
mod portable;
#[cfg(not(target_os = "macos"))]
pub use portable::*;
#[cfg(not(target_os = "macos"))]
mod document;
#[cfg(not(target_os = "macos"))]
pub use document::*;
