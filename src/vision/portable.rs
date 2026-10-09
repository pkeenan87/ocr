//! Apple Vision stand-ins off macOS, where it does not exist (PLAN §9). Documents are read by
//! `document.rs`.

use std::path::Path;

use anyhow::{Result, bail};

use super::{Block, Options, Page};

const NO_VISION: &str = "Apple Vision is only available on macOS";

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
