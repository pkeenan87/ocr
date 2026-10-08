//! Page functions off macOS. Apple Vision does not exist here; PDF and image access arrives
//! with the portable document layer (PLAN §9, Phase 7c).

use std::path::Path;

use anyhow::{Result, bail};

use super::{Block, DocInfo, GrayImage, Options, Page};

const NO_DOCUMENTS: &str =
    "reading documents is not supported on this platform yet (PLAN §9, Phase 7c)";
const NO_VISION: &str = "Apple Vision is only available on macOS";

pub fn document_info(_path: &Path) -> Result<DocInfo> {
    bail!(NO_DOCUMENTS)
}

pub fn text_layer(_path: &Path, _page_index: usize) -> Result<String> {
    bail!(NO_DOCUMENTS)
}

pub fn render_gray(_path: &Path, _page_index: usize, _dpi: u32) -> Result<GrayImage> {
    bail!(NO_DOCUMENTS)
}

pub fn render_png_fit(
    _path: &Path,
    _page_index: usize,
    _dpi: u32,
    _crop: Option<[f64; 4]>,
    _max_side: Option<u32>,
    _rotation: u32,
) -> Result<Vec<u8>> {
    bail!(NO_DOCUMENTS)
}

/// No Vision requests ever run off macOS.
pub fn max_concurrent() -> usize {
    0
}

pub fn recognize_page(
    _path: &Path,
    _page_index: usize,
    _dpi: u32,
    _options: &Options,
) -> Result<Page> {
    bail!(NO_VISION)
}

pub fn recognize_document(_image: &Path, _options: &Options) -> Result<Vec<Block>> {
    bail!(NO_VISION)
}
