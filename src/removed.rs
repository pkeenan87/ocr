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
        let base = hf_hub_cache(|k| std::env::var_os(k), dirs::home_dir())?;
        Some(weights_dir_in(&base, repo, true))
    } else {
        let base = crate::config::state_dir().ok()?.join("models");
        Some(weights_dir_in(&base, repo, false))
    }
}

/// The Hugging Face hub cache, resolved as huggingface_hub does: $HF_HUB_CACHE, else $HF_HOME/hub,
/// else $XDG_CACHE_HOME/huggingface/hub, else ~/.cache/huggingface/hub.
fn hf_hub_cache(
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = set("HF_HUB_CACHE") {
        return Some(d);
    }
    if let Some(d) = set("HF_HOME") {
        return Some(d.join("hub"));
    }
    let cache = match set("XDG_CACHE_HOME") {
        Some(d) => d,
        None => home?.join(".cache"),
    };
    Some(cache.join("huggingface/hub"))
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
    fn hf_cache_resolution() {
        use std::ffi::OsString;
        let home = Some(PathBuf::from("/h"));
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let all = &[
            ("HF_HUB_CACHE", "/a"),
            ("HF_HOME", "/b"),
            ("XDG_CACHE_HOME", "/c"),
        ];
        assert_eq!(
            hf_hub_cache(env(all), home.clone()),
            Some(PathBuf::from("/a"))
        );
        let hf_home = &[("HF_HOME", "/b"), ("XDG_CACHE_HOME", "/c")];
        assert_eq!(
            hf_hub_cache(env(hf_home), home.clone()),
            Some(PathBuf::from("/b/hub"))
        );
        let xdg = &[("XDG_CACHE_HOME", "/c")];
        assert_eq!(
            hf_hub_cache(env(xdg), home.clone()),
            Some(PathBuf::from("/c/huggingface/hub"))
        );
        assert_eq!(
            hf_hub_cache(env(&[]), home),
            Some(PathBuf::from("/h/.cache/huggingface/hub"))
        );
        assert_eq!(hf_hub_cache(env(&[]), None), None);
    }

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
