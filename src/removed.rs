//! What ocr tells users who upgrade from a release that still had the olmOCR engine.

use std::path::{Path, PathBuf};

use clap::error::{ContextKind, ContextValue, ErrorKind};

use crate::config::Config;

pub const OLMOCR: &str = "olmocr";
pub const REMOVED_IN: &str = "1.1.0";

/// Default repo per platform when the config entry has none.
const OLMOCR_REPO: &str = if cfg!(target_os = "macos") {
    "mlx-community/olmOCR-2-7B-1025-8bit"
} else {
    "bartowski/allenai_olmOCR-2-7B-1025-GGUF"
};

/// Where the weights of `repo` were downloaded: off macOS under the state dir, on macOS in the
/// Hugging Face cache.
pub fn weights_dir(repo: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let base = if let Some(d) = std::env::var_os("HF_HUB_CACHE") {
            PathBuf::from(d)
        } else if let Some(d) = std::env::var_os("HF_HOME") {
            PathBuf::from(d).join("hub")
        } else {
            dirs::home_dir()?.join(".cache/huggingface/hub")
        };
        Some(weights_dir_in(&base, repo, true))
    } else {
        let base = crate::config::state_dir().ok()?.join("models");
        Some(weights_dir_in(&base, repo, false))
    }
}

fn weights_dir_in(base: &Path, repo: &str, hf_layout: bool) -> PathBuf {
    if hf_layout {
        base.join(format!("models--{}", repo.replace('/', "--")))
    } else {
        base.join(repo)
    }
}

/// The notice for a config that still lists olmocr, or None.
pub fn olmocr_notice(cfg: &Config) -> Option<String> {
    let entry = cfg.engines.get(OLMOCR)?;
    let repo = entry.repo.as_deref().unwrap_or(OLMOCR_REPO);
    let mut s = format!(
        "note: olmOCR was removed in ocr {REMOVED_IN}; use dots-ocr for layout, transcripts and tables."
    );
    if let Some(p) = weights_dir(repo) {
        s.push_str(&format!(
            "\nnote: its weights in {} can be deleted.",
            p.display()
        ));
    }
    Some(s)
}

/// Prints the notice and removes the entry (the caller saves). Returns whether it did.
pub fn drop_olmocr(cfg: &mut Config) -> bool {
    let Some(n) = olmocr_notice(cfg) else {
        return false;
    };
    eprintln!("{n}");
    cfg.engines.remove(OLMOCR);
    true
}

/// The hint printed after clap's error when `--model olmocr` was given.
pub fn model_hint(err: &clap::Error) -> Option<&'static str> {
    if err.kind() != ErrorKind::InvalidValue {
        return None;
    }
    let arg = err.get(ContextKind::InvalidArg)?.to_string();
    let value = err.get(ContextKind::InvalidValue)?;
    (arg.starts_with("--model") && *value == ContextValue::String(OLMOCR.into()))
        .then_some("hint: olmOCR was removed in ocr 1.1.0; use --model dots-ocr")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn weights_layouts() {
        let b = Path::new("/x");
        assert_eq!(
            weights_dir_in(b, "bartowski/allenai_olmOCR-2-7B-1025-GGUF", false),
            Path::new("/x/bartowski/allenai_olmOCR-2-7B-1025-GGUF")
        );
        assert_eq!(
            weights_dir_in(b, "mlx-community/olmOCR-2-7B-1025-8bit", true),
            Path::new("/x/models--mlx-community--olmOCR-2-7B-1025-8bit")
        );
    }

    fn hint(args: &[&str]) -> Option<&'static str> {
        let e = Cli::try_parse_from(args).unwrap_err();
        model_hint(&e)
    }

    #[test]
    fn hint_only_for_model_olmocr() {
        assert!(hint(&["ocr", "scan", "--model", "olmocr", "--input-file", "a.pdf"]).is_some());
        assert!(hint(&["ocr", "scan", "--model=olmocr", "--input-file", "a.pdf"]).is_some());
        assert!(hint(&["ocr", "scan", "--model", "nope", "--input-file", "a.pdf"]).is_none());
        assert!(
            hint(&[
                "ocr",
                "scan",
                "--file-type",
                "olmocr",
                "--input-file",
                "a.pdf"
            ])
            .is_none()
        );
    }

    #[test]
    fn drop_removes_only_olmocr() {
        let mut cfg = Config::default();
        assert!(!drop_olmocr(&mut cfg));
        cfg.engines.insert(OLMOCR.into(), Default::default());
        cfg.engines.insert("dots-ocr".into(), Default::default());
        assert!(drop_olmocr(&mut cfg));
        assert!(!cfg.engines.contains_key(OLMOCR));
        assert!(cfg.engines.contains_key("dots-ocr"));
    }
}
