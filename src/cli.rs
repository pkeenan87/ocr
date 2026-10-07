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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Engine {
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

    /// Valid `--model-version` values, newest (default) first.
    pub fn versions(self) -> &'static [&'static str] {
        match self {
            Self::Apple | Self::Olmocr => &[],
            Self::PaddleocrVl => &["1.6", "1.5", "1.0"],
            Self::DotsOcr => &["dots.mocr", "dots.ocr"],
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
    #[arg(long, value_enum)]
    pub quant: Option<Quant>,
    #[arg(long)]
    pub non_interactive: bool,
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
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TextLayer {
    Auto,
    Never,
    Always,
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
    #[arg(long, value_enum, default_value = "apple")]
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
    #[arg(long, value_enum, default_value = "auto")]
    pub use_text_layer: TextLayer,
    /// Strip pleading line numbers and running headers
    #[arg(long)]
    pub legal_cleanup: bool,
    /// Override render resolution
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub dpi: Option<u32>,
    /// Total pages in flight across the whole run
    #[arg(long, visible_alias = "parallel", value_name = "N", value_parser = parse_jobs)]
    pub jobs: Option<usize>,
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,
    #[arg(long)]
    pub verbose: bool,
    /// Print the planned input -> output mapping and exit
    #[arg(long)]
    pub dry_run: bool,
}

impl ScanArgs {
    pub fn page_markers(&self) -> bool {
        !self.no_page_markers
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

/// Performance-core count via sysctl, falling back to available parallelism.
pub fn perf_cores() -> usize {
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
        assert!(e.to_string().contains("1.6, 1.5, 1.0"));
    }

    #[test]
    fn clap_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
