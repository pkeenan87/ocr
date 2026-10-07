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
    fn ocr_bridge_free(ptr: *mut c_char);
}

#[derive(Debug, Deserialize)]
pub struct Block {
    pub kind: String,
    /// Normalized [x, y, w, h], origin top-left.
    pub bbox: [f64; 4],
    pub text: Option<String>,
    pub items: Option<Vec<String>>,
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            languages: vec!["en-US".into()],
            auto_detect_language: false,
            language_correction: true,
            custom_words: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageOut {
    ok: bool,
    error: Option<String>,
    #[serde(default)]
    blocks: Vec<Block>,
}

/// Recognize one page image. Blocks the calling thread; safe to call from many threads at once.
pub fn recognize_document(image: &Path, options: &Options) -> Result<Vec<Block>> {
    let c_path = CString::new(image.to_string_lossy().as_bytes()).context("path contains NUL")?;
    let c_opts = CString::new(serde_json::to_string(options)?)?;
    let json = unsafe {
        let ptr = ocr_bridge_recognize_document(c_path.as_ptr(), c_opts.as_ptr());
        if ptr.is_null() {
            bail!("vision bridge returned null");
        }
        let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        ocr_bridge_free(ptr);
        s
    };
    let page: PageOut = serde_json::from_str(&json).context("bad JSON from vision bridge")?;
    if !page.ok {
        bail!("vision: {}", page.error.unwrap_or_default());
    }
    Ok(page.blocks)
}
