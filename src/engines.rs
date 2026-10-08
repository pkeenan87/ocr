//! Model catalog: Hugging Face repos per engine, version and quant, plus RAM footprints (PLAN §4.1, §8.1).

use crate::cli::{Engine, Quant};

/// Pinned `mlx-vlm` release installed into the managed venv.
pub const MLX_VLM_PIN: &str = "0.7.6";

impl Quant {
    pub fn name(self) -> &'static str {
        match self {
            Self::Q4 => "4bit",
            Self::Q8 => "8bit",
            Self::Bf16 => "bf16",
        }
    }
}

/// Default quant for an engine.
pub fn default_quant(engine: Engine) -> Quant {
    match engine {
        Engine::PaddleocrVl => Quant::Bf16,
        _ => Quant::Q8,
    }
}

/// Hugging Face repo for an engine/version/quant; `None` if that combination doesn't exist.
/// `version` must already be validated (`None` selects the engine's default).
pub fn repo(engine: Engine, version: Option<&str>, quant: Quant) -> Option<&'static str> {
    use Quant::{Bf16, Q4, Q8};
    let version = version.or_else(|| engine.versions().first().copied());
    Some(match (engine, version, quant) {
        (Engine::Olmocr, _, Q8) => "mlx-community/olmOCR-2-7B-1025-8bit",
        (Engine::Olmocr, _, Q4) => "mlx-community/olmOCR-2-7B-1025-4bit",
        (Engine::Olmocr, _, Bf16) => "mlx-community/olmOCR-2-7B-1025-bf16",
        (Engine::PaddleocrVl, Some("1.6"), Bf16) => "PaddlePaddle/PaddleOCR-VL-1.6",
        (Engine::PaddleocrVl, Some("1.6"), Q4) => "mlx-community/PaddleOCR-VL-1.6-4bit",
        (Engine::PaddleocrVl, Some("1.5"), Bf16) => "mlx-community/PaddleOCR-VL-1.5-bf16",
        (Engine::PaddleocrVl, Some("1.5"), Q4) => "mlx-community/PaddleOCR-VL-1.5-4bit",
        (Engine::PaddleocrVl, Some("1.5"), Q8) => "mlx-community/PaddleOCR-VL-1.5-8bit",
        (Engine::PaddleocrVl, Some("1.0"), Bf16) => "PaddlePaddle/PaddleOCR-VL",
        (Engine::PaddleocrVl, Some("1.0"), Q4) => "mlx-community/PaddleOCR-VL-4bit",
        (Engine::PaddleocrVl, Some("1.0"), Q8) => "mlx-community/PaddleOCR-VL-8bit",
        (Engine::DotsOcr, Some("dots.mocr"), Q8) => "mlx-community/dots.mocr-8bit",
        (Engine::DotsOcr, Some("dots.mocr"), Q4) => "mlx-community/dots.mocr-4bit",
        (Engine::DotsOcr, Some("dots.mocr"), Bf16) => "mlx-community/dots.mocr-bf16",
        (Engine::DotsOcr, Some("dots.ocr"), Q8) => "mlx-community/dots.ocr-8bit",
        (Engine::DotsOcr, Some("dots.ocr"), Q4) => "mlx-community/dots.ocr-4bit",
        (Engine::DotsOcr, Some("dots.ocr"), Bf16) => "mlx-community/dots.ocr-bf16",
        _ => return None,
    })
}

/// Per-request limits of a model engine. `timeout_secs` is 2 x (prefill + `max_tokens` / decode
/// rate) from the measured rates in docs/spike.md; the retry limits are for the larger
/// `retry_max_tokens` request after a truncated reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_tokens: u32,
    pub retry_max_tokens: u32,
    pub timeout_secs: u64,
    pub retry_timeout_secs: u64,
}

/// Request limits per engine (docs/spike.md): PaddleOCR-VL prefill 990 tok/s and decode
/// 218 tok/s; olmOCR prefill 323 tok/s (about 1790 prompt tokens, 5.5 s) and decode 39.5 tok/s,
/// so 2 x (5.5 + 4096 / 39.5) = 218 s and 2 x (5.5 + 8000 / 39.5) = 416 s.
pub fn limits(engine: Engine) -> Limits {
    match engine {
        Engine::Olmocr => Limits {
            max_tokens: 4096,
            // olmOCR's own pipeline uses 8000.
            retry_max_tokens: 8000,
            timeout_secs: 220,
            retry_timeout_secs: 420,
        },
        _ => Limits {
            max_tokens: 4096,
            retry_max_tokens: 8192,
            timeout_secs: 45,
            retry_timeout_secs: 45,
        },
    }
}

/// Quants published for an engine/version, for error messages.
pub fn quants(engine: Engine, version: Option<&str>) -> Vec<&'static str> {
    [Quant::Q4, Quant::Q8, Quant::Bf16]
        .into_iter()
        .filter(|q| repo(engine, version, *q).is_some())
        .map(Quant::name)
        .collect()
}

/// Measured (olmocr 8bit, dots 8bit, paddle bf16) or proportionally estimated peak physical footprint, GB.
pub fn footprint_gb(engine: Engine, quant: Quant) -> f64 {
    let (base, base_bits) = match engine {
        Engine::Olmocr => (10.9, 8.0),
        Engine::DotsOcr => (7.3, 8.0),
        Engine::PaddleocrVl => (3.8, 16.0),
        Engine::Apple => return 0.0,
    };
    let bits = match quant {
        Quant::Q4 => 4.0,
        Quant::Q8 => 8.0,
        Quant::Bf16 => 16.0,
    };
    ((base * bits / base_bits) * 100.0_f64).round() / 100.0
}

#[derive(Debug, PartialEq)]
pub enum RamVerdict {
    Ok,
    Warn,
    Refuse,
}

/// RAM check. olmocr 8bit: silent at 24 GB or more, warn at 16-24, refuse below 16.
/// Others: warn if footprint x 1.5 exceeds RAM, refuse if footprint exceeds RAM x 0.75.
pub fn ram_verdict(engine: Engine, quant: Quant, ram_gb: f64) -> RamVerdict {
    if engine == Engine::Olmocr && quant == Quant::Q8 {
        return if ram_gb >= 24.0 {
            RamVerdict::Ok
        } else if ram_gb >= 16.0 {
            RamVerdict::Warn
        } else {
            RamVerdict::Refuse
        };
    }
    let fp = footprint_gb(engine, quant);
    if fp > ram_gb * 0.75 {
        RamVerdict::Refuse
    } else if fp * 1.5 > ram_gb {
        RamVerdict::Warn
    } else {
        RamVerdict::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repos() {
        assert_eq!(
            repo(Engine::PaddleocrVl, None, Quant::Bf16),
            Some("PaddlePaddle/PaddleOCR-VL-1.6")
        );
        assert_eq!(repo(Engine::PaddleocrVl, Some("1.6"), Quant::Q8), None);
        assert_eq!(
            repo(Engine::DotsOcr, Some("dots.ocr"), Quant::Q4),
            Some("mlx-community/dots.ocr-4bit")
        );
        assert_eq!(quants(Engine::PaddleocrVl, None), ["4bit", "bf16"]);
    }

    #[test]
    fn ram() {
        assert_eq!(ram_verdict(Engine::Olmocr, Quant::Q8, 24.0), RamVerdict::Ok);
        assert_eq!(
            ram_verdict(Engine::Olmocr, Quant::Q8, 16.0),
            RamVerdict::Warn
        );
        assert_eq!(
            ram_verdict(Engine::Olmocr, Quant::Q8, 8.0),
            RamVerdict::Refuse
        );
        assert_eq!(
            ram_verdict(Engine::PaddleocrVl, Quant::Bf16, 16.0),
            RamVerdict::Ok
        );
        assert_eq!(
            ram_verdict(Engine::DotsOcr, Quant::Q8, 8.0),
            RamVerdict::Refuse
        );
        assert_eq!(footprint_gb(Engine::Olmocr, Quant::Q4), 5.45);
    }
}
