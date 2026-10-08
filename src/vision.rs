//! Safe wrapper over the Swift bridge (`bridge/`) to Apple Vision's `RecognizeDocumentsRequest`.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

unsafe extern "C" {
    fn ocr_bridge_recognize_document(
        path: *const c_char,
        options_json: *const c_char,
    ) -> *mut c_char;
    fn ocr_bridge_max_concurrent() -> i32;
    fn ocr_bridge_document_info(path: *const c_char) -> *mut c_char;
    fn ocr_bridge_text_layer(path: *const c_char, page_index: i32) -> *mut c_char;
    fn ocr_bridge_recognize_page(
        path: *const c_char,
        page_index: i32,
        dpi: i32,
        options_json: *const c_char,
    ) -> *mut c_char;
    fn ocr_bridge_free(ptr: *mut c_char);
    fn ocr_bridge_render_gray(
        path: *const c_char,
        page_index: i32,
        dpi: i32,
        width: *mut i32,
        height: *mut i32,
    ) -> *mut u8;
    fn ocr_bridge_free_bytes(ptr: *mut u8);
    fn ocr_bridge_render_png(
        path: *const c_char,
        page_index: i32,
        dpi: i32,
        crop: *const f64,
        max_side: i32,
        rotation: i32,
        out_len: *mut usize,
    ) -> *mut u8;
}

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
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageOut {
    ok: bool,
    error: Option<String>,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    blocks: Vec<Block>,
}

/// Recognized content of one rendered page.
#[derive(Debug)]
pub struct Page {
    /// Rendered bitmap size in pixels.
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<Block>,
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

#[derive(Debug, Deserialize)]
struct InfoOut {
    ok: bool,
    error: Option<String>,
    kind: Option<DocKind>,
    #[serde(default)]
    pages: usize,
}

#[derive(Debug, Deserialize)]
struct TextOut {
    ok: bool,
    error: Option<String>,
    #[serde(default)]
    text: String,
}

fn cstr(s: &str) -> Result<CString> {
    CString::new(s.as_bytes()).context("string contains NUL")
}

/// Takes ownership of a bridge string and frees it.
unsafe fn take(ptr: *mut c_char) -> Result<String> {
    if ptr.is_null() {
        bail!("vision bridge returned null");
    }
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { ocr_bridge_free(ptr) };
    Ok(s)
}

/// Page count and kind (PDF or image). Multi-page TIFF reports its frame count.
pub fn document_info(path: &Path) -> Result<DocInfo> {
    let c_path = cstr(&path.to_string_lossy())?;
    let json = unsafe { take(ocr_bridge_document_info(c_path.as_ptr()))? };
    let info: InfoOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    match (info.ok, info.kind) {
        (true, Some(kind)) => Ok(DocInfo {
            kind,
            pages: info.pages,
        }),
        _ => bail!(
            "{}",
            info.error.unwrap_or_else(|| "unreadable document".into())
        ),
    }
}

/// Embedded text of a PDF page (0-based index); empty for images.
pub fn text_layer(path: &Path, page_index: usize) -> Result<String> {
    let c_path = cstr(&path.to_string_lossy())?;
    let json = unsafe { take(ocr_bridge_text_layer(c_path.as_ptr(), page_index as i32))? };
    let out: TextOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    if !out.ok {
        bail!("text layer: {}", out.error.unwrap_or_default());
    }
    Ok(out.text)
}

/// The most Vision requests that were in flight at once in this process (the bridge caps it).
pub fn max_concurrent() -> usize {
    unsafe { ocr_bridge_max_concurrent() }.max(0) as usize
}

/// Render page `page_index` (0-based) at `dpi` and recognize it. Opens the document on every
/// call, blocks the calling thread, and is safe to call from many threads at once.
pub fn recognize_page(path: &Path, page_index: usize, dpi: u32, options: &Options) -> Result<Page> {
    let c_path = cstr(&path.to_string_lossy())?;
    let c_opts = cstr(&serde_json::to_string(options)?)?;
    let json = unsafe {
        take(ocr_bridge_recognize_page(
            c_path.as_ptr(),
            page_index as i32,
            dpi as i32,
            c_opts.as_ptr(),
        ))?
    };
    let page: PageOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    if !page.ok {
        bail!("vision: {}", page.error.unwrap_or_default());
    }
    Ok(Page {
        width: page.width,
        height: page.height,
        blocks: page.blocks,
    })
}

/// An 8-bit grayscale bitmap, one byte per pixel, row 0 at the top.
#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Render page `page_index` (0-based) at `dpi` exactly as `recognize_page` does, as grayscale.
pub fn render_gray(path: &Path, page_index: usize, dpi: u32) -> Result<GrayImage> {
    let c_path = cstr(&path.to_string_lossy())?;
    let (mut w, mut h) = (0i32, 0i32);
    let ptr = unsafe {
        ocr_bridge_render_gray(
            c_path.as_ptr(),
            page_index as i32,
            dpi as i32,
            &mut w,
            &mut h,
        )
    };
    if ptr.is_null() || w <= 0 || h <= 0 {
        bail!("cannot render page {} as grayscale", page_index + 1);
    }
    let (width, height) = (w as usize, h as usize);
    let pixels = unsafe { std::slice::from_raw_parts(ptr, width * height) }.to_vec();
    unsafe { ocr_bridge_free_bytes(ptr) };
    Ok(GrayImage {
        width,
        height,
        pixels,
    })
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

/// Like `render_png`, with the model engines' image sizing. `max_side` ignores `dpi` and makes
/// the image (the crop, if any) exactly that many pixels on its longest side. `rotation`
/// (0, 90, 180 or 270) then turns it counter-clockwise, as PIL's `Image.transpose(ROTATE_*)`.
pub fn render_png_fit(
    path: &Path,
    page_index: usize,
    dpi: u32,
    crop: Option<[f64; 4]>,
    max_side: Option<u32>,
    rotation: u32,
) -> Result<Vec<u8>> {
    let c_path = cstr(&path.to_string_lossy())?;
    let mut len = 0usize;
    let ptr = unsafe {
        ocr_bridge_render_png(
            c_path.as_ptr(),
            page_index as i32,
            dpi as i32,
            crop.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            max_side.unwrap_or(0) as i32,
            rotation as i32,
            &mut len,
        )
    };
    if ptr.is_null() || len == 0 {
        bail!("cannot render page {} as PNG", page_index + 1);
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { ocr_bridge_free_bytes(ptr) };
    Ok(bytes)
}

/// Recognize one page image. Blocks the calling thread; safe to call from many threads at once.
pub fn recognize_document(image: &Path, options: &Options) -> Result<Vec<Block>> {
    let c_path = CString::new(image.to_string_lossy().as_bytes()).context("path contains NUL")?;
    let c_opts = CString::new(serde_json::to_string(options)?)?;
    let json = unsafe {
        take(ocr_bridge_recognize_document(
            c_path.as_ptr(),
            c_opts.as_ptr(),
        ))?
    };
    let page: PageOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    if !page.ok {
        bail!("vision: {}", page.error.unwrap_or_default());
    }
    Ok(page.blocks)
}
