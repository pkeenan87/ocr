//! `ocr` — local document OCR for macOS, and (PLAN §9) Linux and Windows with a GPU.

pub mod checkpoint;
pub mod cli;
pub mod config;
pub mod dots;
pub mod engines;
pub mod error;
pub mod htmltable;
pub mod input;
pub mod layout;
pub mod markdown;
pub mod model_engine;
pub mod olmocr;
pub mod otsl;
pub mod pipeline;
pub mod progress;
pub mod regions;
pub mod run;
pub mod server;
pub mod supervisor;
pub mod vision;
