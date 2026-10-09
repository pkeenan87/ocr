//! Safe wrapper over the Swift bridge (`bridge/`) to PDFKit and Apple Vision.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{Block, DocInfo, DocKind, GrayImage, Line, Options, Page};

unsafe extern "C" {
    fn ocr_bridge_recognize_document(
        path: *const c_char,
        options_json: *const c_char,
    ) -> *mut c_char;
    fn ocr_bridge_max_concurrent() -> i32;
    fn ocr_bridge_document_info(path: *const c_char) -> *mut c_char;
    fn ocr_bridge_text_layer(path: *const c_char, page_index: i32) -> *mut c_char;
    fn ocr_bridge_text_in_regions(
        path: *const c_char,
        page_index: i32,
        regions_json: *const c_char,
    ) -> *mut c_char;
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
    #[serde(default)]
    lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
struct InfoOut {
    ok: bool,
    error: Option<String>,
    kind: Option<DocKind>,
    #[serde(default)]
    pages: usize,
}

#[derive(Deserialize)]
struct RegionsOut {
    ok: bool,
    error: Option<String>,
    #[serde(default)]
    texts: Vec<String>,
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
/// Embedded PDF text inside each region (normalized `[x, y, w, h]`, top-left origin on the
/// rendered page: the frame of Vision's boxes), in order.
pub fn text_in_regions(
    path: &Path,
    page_index: usize,
    regions: &[[f64; 4]],
) -> Result<Vec<String>> {
    let c_path = cstr(&path.to_string_lossy())?;
    let c_regions = cstr(&serde_json::to_string(regions)?)?;
    let json = unsafe {
        take(ocr_bridge_text_in_regions(
            c_path.as_ptr(),
            page_index as i32,
            c_regions.as_ptr(),
        ))?
    };
    let out: RegionsOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    if !out.ok {
        bail!("text in regions: {}", out.error.unwrap_or_default());
    }
    Ok(out.texts)
}

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
        lines: page.lines,
    })
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
