//! dots scan engine (placeholder until the dots commit).
use crate::cli::Engine;
use crate::model_engine::{Ctx, Fail, Output, Prepared, Prompt, ScanEngine, Spec};
use crate::pipeline::{Settings, SidePass};
use std::path::Path;

pub const PROMPT_LAYOUT: &str = "";
pub const PROMPT_OCR: &str = "";

pub struct Dots;

impl ScanEngine for Dots {
    fn engine(&self) -> Engine {
        Engine::DotsOcr
    }
    fn page_prompt(&self) -> Prompt {
        Prompt::DotsLayout
    }
    fn plan(&self, _side: &SidePass) -> (Vec<Spec>, bool) {
        (vec![(Prompt::DotsLayout, None)], false)
    }
    fn run(&self, _: &Ctx, _: &Path, _: &Prepared, _: &Settings) -> Result<Output, Fail> {
        Err(Fail::Failed("dots is not implemented".into()))
    }
}
