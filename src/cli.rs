//! clap command tree and per-engine option rules (PLAN §3, §4.2, §5.1).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::error::CliError;
use crate::input::{FileType, PageRanges};

#[derive(Debug, Parser)]
#[command(name = "ocr", version, about = "Local document OCR to Markdown")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage model engines and servers
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Convert PDFs and images to Markdown
    Scan(Box<ScanArgs>),
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Write the config and install engines
    Setup(SetupArgs),
    /// Start a model server
    Start(StartArgs),
    /// Stop the running model server
    Stop,
    /// Show the model server status
    Status,
}

/// `ocr scan --model` default: Apple Vision on macOS, PaddleOCR-VL elsewhere (PLAN §9).
#[cfg(target_os = "macos")]
pub const DEFAULT_ENGINE: &str = "apple";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_ENGINE: &str = "paddleocr-vl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Engine {
    /// Apple Vision, in-process. macOS only.
    #[cfg_attr(not(target_os = "macos"), value(skip))]
    Apple,
    Olmocr,
    PaddleocrVl,
    DotsOcr,
}

impl Engine {
    pub const MODELS: [Engine; 3] = [Engine::Olmocr, Engine::PaddleocrVl, Engine::DotsOcr];

    pub fn name(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Olmocr => "olmocr",
            Self::PaddleocrVl => "paddleocr-vl",
            Self::DotsOcr => "dots-ocr",
        }
    }

    /// Valid `--model-version` values, default first. Off macOS only the releases with a GGUF
    /// build are offered, and dots defaults to `dots.ocr` (PLAN §9.5).
    pub fn versions(self) -> &'static [&'static str] {
        match self {
            Self::Apple | Self::Olmocr => &[],
            Self::PaddleocrVl if cfg!(target_os = "macos") => &["1.6", "1.5", "1.0"],
            Self::PaddleocrVl => &["1.6", "1.5"],
            Self::DotsOcr if cfg!(target_os = "macos") => &["dots.mocr", "dots.ocr"],
            Self::DotsOcr => &["dots.ocr"],
        }
    }

    /// Validates `--model-version`, returning the version to use (if the engine has any).
    pub fn resolve_version(
        self,
        requested: Option<&str>,
    ) -> Result<Option<&'static str>, CliError> {
        let versions = self.versions();
        match (requested, versions.first()) {
            (None, default) => Ok(default.copied()),
            (Some(_), None) => Err(CliError::Usage(format!(
                "engine '{}' has no selectable --model-version",
                self.name()
            ))),
            (Some(v), Some(_)) => versions
                .iter()
                .find(|x| **x == v)
                .copied()
                .map(Some)
                .ok_or_else(|| {
                    CliError::Usage(format!(
                        "invalid --model-version '{v}' for engine '{}'; valid values: {}",
                        self.name(),
                        versions.join(", ")
                    ))
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Quant {
    #[value(name = "4bit")]
    Q4,
    #[value(name = "8bit")]
    Q8,
    Bf16,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Engine(s) to install (repeatable)
    #[arg(long, value_enum, num_args = 1.., value_name = "ENGINE")]
    pub model: Vec<Engine>,
    /// Install every engine
    #[arg(long)]
    pub all: bool,
    /// Model release (paddleocr-vl: 1.6|1.5|1.0, dots-ocr: dots.mocr|dots.ocr)
    #[arg(long, value_name = "V")]
    pub model_version: Option<String>,
    #[arg(long, value_enum)]
    pub quant: Option<Quant>,
    #[arg(long)]
    pub non_interactive: bool,
    /// Install despite the RAM check
    #[arg(long)]
    pub force: bool,
}

impl SetupArgs {
    /// Model engines requested (excluding `apple`), deduplicated.
    pub fn requested_models(&self) -> Vec<Engine> {
        Engine::MODELS
            .into_iter()
            .filter(|e| self.all || self.model.contains(e))
            .collect()
    }
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// Engine to serve (default: the only installed model engine)
    #[arg(long, value_enum)]
    pub model: Option<Engine>,
    /// Model release (default: the one recorded by setup)
    #[arg(long, value_name = "V")]
    pub model_version: Option<String>,
    /// Quantization (default: the one recorded by setup)
    #[arg(long, value_enum)]
    pub quant: Option<Quant>,
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    /// Replace a running server of a different engine or repo
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TextLayer {
    Auto,
    Never,
    Always,
}

impl TextLayer {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Never => "never",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Single input file
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "input_dir",
        required_unless_present = "input_dir"
    )]
    pub input_file: Option<PathBuf>,
    /// Input directory
    #[arg(long, value_name = "DIR")]
    pub input_dir: Option<PathBuf>,
    /// Output Markdown file (default ./<stem>.md)
    #[arg(
        long,
        value_name = "PATH.md",
        requires = "input_file",
        conflicts_with = "input_dir"
    )]
    pub output_file: Option<PathBuf>,
    /// Output directory (default <input-dir>/ocr-output)
    #[arg(
        long,
        value_name = "DIR",
        requires = "input_dir",
        conflicts_with = "input_file"
    )]
    pub output_dir: Option<PathBuf>,
    #[arg(long, value_enum, default_value = DEFAULT_ENGINE)]
    pub model: Engine,
    /// Pin an older model release (paddleocr-vl: 1.6|1.5|1.0, dots-ocr: dots.mocr|dots.ocr)
    #[arg(long, value_name = "V")]
    pub model_version: Option<String>,
    #[arg(long, value_enum, default_value = "auto")]
    pub file_type: FileType,
    /// Walk subfolders and mirror the tree in the output
    #[arg(long)]
    pub recursive: bool,
    /// Page subset, e.g. 1-50,75
    #[arg(long, value_parser = PageRanges::parse)]
    pub pages: Option<PageRanges>,
    /// Discard existing output and checkpoints
    #[arg(long)]
    pub overwrite: bool,
    /// Start each page with an HTML comment marker (default)
    #[arg(long, overrides_with = "no_page_markers")]
    pub page_markers: bool,
    /// Omit per-page markers
    #[arg(long, overrides_with = "page_markers")]
    pub no_page_markers: bool,
    /// Keep `S` where Vision misread `$` in amounts such as S22,375.50
    #[arg(long)]
    pub no_currency_fix: bool,
    /// Keep Vision's readings where it misread `¶` and `§` in citations (`Compl. 9 60`,
    /// `U.S.C. $ 1441`)
    #[arg(long)]
    pub no_symbol_fix: bool,
    /// Omit the YAML header
    #[arg(long)]
    pub no_front_matter: bool,
    /// Also write `<stem>.verify.json`: amounts, dates, docket and Bates numbers, citations and
    /// percentages with their page, line and status, and the pages to send to a second reader
    /// (`azure_pages`)
    #[arg(long)]
    pub verify_json: bool,
    /// Which fields not confirmed by the PDF's embedded text need a second reading: only those
    /// with something against them (flagged), or every one (all)
    #[arg(
        long,
        value_enum,
        value_name = "MODE",
        default_value_t,
        requires = "verify_json"
    )]
    pub verify_fields: crate::verify::VerifyFields,
    /// Use a PDF's embedded text instead of OCR (default: never; the text-layer path has no
    /// layout handling, so margin numbers come out inline, tables are flattened and Bates is lost)
    #[arg(long, value_enum)]
    pub use_text_layer: Option<TextLayer>,
    /// Strip running headers and footers repeated across pages (opt-in)
    #[arg(long)]
    pub legal_cleanup: bool,
    /// Render resolution, 72-600 (default 150; dots-ocr 100)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(72..=600))]
    pub dpi: Option<u32>,
    /// Total pages in flight across the whole run (Apple Vision itself recognizes at most 2 at a time)
    #[arg(long, visible_alias = "parallel", value_name = "N", value_parser = parse_jobs)]
    pub jobs: Option<usize>,
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Per-request detail: with --verbose, one line per model request after each page plan
    #[arg(long)]
    pub verbose: bool,
    /// Also write `<stem>.timing.json`: per page the prep and recognize time, and per model
    /// request the round trip, image size, attempt reason, finish reason and the server's own
    /// tokens and timings (prompt vs decode), with totals per file
    #[arg(long)]
    pub timing_json: bool,
    /// Print the planned input -> output mapping and exit
    #[arg(long)]
    pub dry_run: bool,
}

/// Default render resolution for apple. The user switched it from 200 to 150 on 2026-10-08
/// (docs/eval.md) and the goldens were regenerated.
pub const DEFAULT_DPI: u32 = 150;
/// olmocr's Apple side pass keeps the former default; the switch to 150 was for the apple engine.
pub const OLMOCR_SIDE_DPI: u32 = 200;
/// Phase 6 tuning (docs/eval.md): paddleocr-vl scored best on tune seeds 0-4 at 150 DPI.
pub const PADDLE_DPI: u32 = 150;
/// Phase 6 tuning (docs/eval.md): dots-ocr was as accurate at 100 DPI as at 200, and 3-4x faster.
pub const DOTS_DPI: u32 = 100;

impl ScanArgs {
    /// Apple defaults to `auto`: Vision's layout with the exact characters of a PDF's embedded
    /// text where the page agrees with it (src/embedded.rs). Model engines default to `never`.
    /// `always` is the raw embedded text with no layout handling.
    pub fn effective_text_layer(&self) -> TextLayer {
        self.use_text_layer
            .unwrap_or(if self.model == Engine::Apple {
                TextLayer::Auto
            } else {
                TextLayer::Never
            })
    }

    pub fn page_markers(&self) -> bool {
        !self.no_page_markers
    }

    /// Effective `--dpi`: explicit value, else the engine default (apple 150; paddleocr-vl 150; dots-ocr 100; olmocr side pass 200).
    pub fn effective_dpi(&self) -> u32 {
        self.dpi.unwrap_or(match self.model {
            Engine::PaddleocrVl => PADDLE_DPI,
            Engine::DotsOcr => DOTS_DPI,
            Engine::Olmocr => OLMOCR_SIDE_DPI,
            _ => DEFAULT_DPI,
        })
    }

    /// Effective `--jobs`: explicit value, else performance cores (apple) or 1 (model engines).
    pub fn effective_jobs(&self) -> usize {
        self.jobs.unwrap_or(match self.model {
            Engine::Apple => perf_cores(),
            _ => 1,
        })
    }
}

fn parse_jobs(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("must be at least 1".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// Performance-core count via sysctl (macOS), falling back to available parallelism.
pub fn perf_cores() -> usize {
    #[cfg(not(target_os = "macos"))]
    return std::thread::available_parallelism().map_or(1, |n| n.get());
    #[cfg(target_os = "macos")]
    std::process::Command::new("sysctl")
        .args(["-n", "hw.perflevel0.physicalcpu"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .filter(|n| *n > 0)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            Engine::PaddleocrVl.resolve_version(None).unwrap(),
            Some("1.6")
        );
        assert_eq!(
            Engine::DotsOcr.resolve_version(Some("dots.ocr")).unwrap(),
            Some("dots.ocr")
        );
        assert_eq!(Engine::Olmocr.resolve_version(None).unwrap(), None);
        assert!(Engine::Olmocr.resolve_version(Some("1")).is_err());
        let e = Engine::PaddleocrVl.resolve_version(Some("2")).unwrap_err();
        let listed = if cfg!(target_os = "macos") {
            "1.6, 1.5, 1.0"
        } else {
            "1.6, 1.5"
        };
        assert!(e.to_string().contains(listed));
        let dots = Engine::DotsOcr.resolve_version(None).unwrap();
        assert_eq!(
            dots,
            Some(if cfg!(target_os = "macos") {
                "dots.mocr"
            } else {
                "dots.ocr"
            })
        );
    }

    #[test]
    fn clap_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
