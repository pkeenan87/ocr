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
    /// `Options::reader` `text` only: Vision's confidence in `text` (coarse in accurate mode:
    /// mostly 0.5 or 1.0), and its next-best readings of the line.
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub alternatives: Vec<String>,
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
    /// `text`: read lines with Vision's older `RecognizeTextRequest` instead of the document
    /// recognizer. Returns `Page::lines` only, with confidence and alternatives; no blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<&'static str>,
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
            reader: None,
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

/// Whether a PDF page's embedded text is the document's own characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextVisibility {
    /// Drawn visibly (or the page has no text): the document's own characters.
    #[default]
    Visible,
    /// Mostly drawn invisibly (render mode 3, or fully transparent): another OCR's reading laid
    /// over a scan.
    OcrLayer,
    /// The page could not be read (a scan error, or an unsupported platform): confirms nothing.
    Unknown,
}

impl TextVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::OcrLayer => "ocr_layer",
            Self::Unknown => "unknown",
        }
    }
}

/// Characters of a page's text drawn visibly and invisibly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CharCounts {
    pub shown: usize,
    pub invisible: usize,
}

/// An OCR layer is a fifth or more of the page's characters drawn invisibly, or 200 of them. A
/// short hidden layer under a long visible text is not enough to call the page a scan, but a
/// long one is never the document's own, whatever else the page shows.
pub fn classify_visibility(c: CharCounts) -> TextVisibility {
    let all = c.shown + c.invisible;
    if c.invisible >= 200 || (c.invisible > 0 && c.invisible * 5 >= all) {
        TextVisibility::OcrLayer
    } else {
        TextVisibility::Visible
    }
}

/// Whether page `page_index` (0-based) of `path` has an invisible OCR text layer. Images have no
/// text layer. Any error reading the page is `Unknown`, never `Visible`.
pub fn text_visibility(path: &Path, page_index: usize) -> TextVisibility {
    if !is_pdf_path(path) {
        return TextVisibility::Visible;
    }
    match char_counts(path, page_index) {
        Ok(c) => classify_visibility(c),
        Err(_) => TextVisibility::Unknown,
    }
}

/// By content, not extension: a PDF under another name still has a text layer.
fn is_pdf_path(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 5];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| &head == b"%PDF-")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn class(shown: usize, invisible: usize) -> TextVisibility {
        classify_visibility(CharCounts { shown, invisible })
    }

    #[test]
    fn a_fifth_hidden_or_two_hundred_chars_is_an_ocr_layer() {
        assert_eq!(class(0, 0), TextVisibility::Visible);
        assert_eq!(class(5000, 0), TextVisibility::Visible);
        assert_eq!(class(5000, 10), TextVisibility::Visible);
        assert_eq!(class(80, 20), TextVisibility::OcrLayer);
        assert_eq!(class(81, 19), TextVisibility::Visible);
        assert_eq!(class(10_000, 200), TextVisibility::OcrLayer);
        assert_eq!(class(0, 5), TextVisibility::OcrLayer);
    }
}
