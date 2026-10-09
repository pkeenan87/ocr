//! Config file (`~/.config/ocr/config.toml`) and state directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::cli::Engine;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EngineEntry {
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
}

/// Model runtime recorded by setup: the managed Python environment with mlx-vlm on macOS, or
/// the llama.cpp build elsewhere (PLAN §9).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Runtime {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub python: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mlx_vlm: String,
    /// llama.cpp release, e.g. `b11510`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llama_cpp: Option<String>,
    /// llama.cpp build: cuda12, cuda13, vulkan or cpu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llama_variant: Option<String>,
    /// The GPU setup found for it, e.g. `NVIDIA GeForce RTX 3090 (24576 MiB)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

impl Runtime {
    /// The runtime identity recorded in checkpoints (their `mlx_vlm` key): the mlx-vlm version
    /// on macOS, as before, and `llama.cpp <build> <variant>` elsewhere.
    pub fn id(&self) -> String {
        match (&self.llama_cpp, &self.llama_variant) {
            (Some(build), Some(variant)) => format!("llama.cpp {build} {variant}"),
            (Some(build), None) => format!("llama.cpp {build}"),
            _ => self.mlx_vlm.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub version: u32,
    pub created_at: String,
    #[serde(default)]
    pub engines: BTreeMap<String, EngineEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
}

/// Directory holding `config.toml`: `$OCR_CONFIG_DIR`, else `~/.config/ocr` (`%APPDATA%\ocr` on
/// Windows).
pub fn config_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("OCR_CONFIG_DIR") {
        return Ok(d.into());
    }
    if cfg!(windows) {
        let dir = dirs::config_dir().ok_or_else(|| anyhow!("cannot determine config directory"))?;
        return Ok(dir.join("ocr"));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    Ok(home.join(".config/ocr"))
}

/// State directory: `$OCR_STATE_DIR`, else the platform data dir plus `ocr`
/// (`~/Library/Application Support/ocr`, `~/.local/share/ocr`, `%LOCALAPPDATA%\ocr`).
pub fn state_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("OCR_STATE_DIR") {
        return Ok(d.into());
    }
    let data = dirs::data_local_dir().ok_or_else(|| anyhow!("cannot determine data directory"))?;
    Ok(data.join("ocr"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

impl Config {
    pub fn new() -> Self {
        Self {
            version: CONFIG_VERSION,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            engines: BTreeMap::new(),
            runtime: None,
        }
    }

    /// Loads the config; `None` if the file doesn't exist.
    pub fn load() -> Result<Option<Self>> {
        let path = config_path()?;
        match fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s)
                .map(Some)
                .with_context(|| format!("invalid config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn is_installed(&self, engine: Engine) -> bool {
        engine == Engine::Apple || self.engines.get(engine.name()).is_some_and(|e| e.installed)
    }

    /// Marks `apple` installed (no download needed).
    pub fn mark_apple_installed(&mut self) {
        self.engines.insert(
            Engine::Apple.name().into(),
            EngineEntry {
                installed: true,
                ..Default::default()
            },
        );
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Config::new();
        c.mark_apple_installed();
        c.engines.insert(
            "olmocr".into(),
            EngineEntry {
                installed: true,
                version: Some("v".into()),
                repo: Some("r".into()),
                quant: Some("8bit".into()),
            },
        );
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(back.is_installed(Engine::Apple));
        assert!(back.is_installed(Engine::Olmocr));
        assert!(!back.is_installed(Engine::DotsOcr));
    }
}
