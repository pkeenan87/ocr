//! Model catalog: Hugging Face repos per engine, version and quant, plus RAM footprints (PLAN §4.1, §8.1).

use crate::cli::{Engine, Quant};

/// Pinned `mlx-vlm` release installed into the managed venv (macOS).
pub const MLX_VLM_PIN: &str = "0.7.6";

/// Pinned `indra-ocr` PyPI release installed into its own venv (every platform).
pub const INDRA_PIN: &str = "1.1.2";

/// Hugging Face repo of indra-ocr's ONNX models, pinned to a revision below.
pub const INDRA_REPO: &str = "datalab-to/indra-ocr";

/// The revision of `INDRA_REPO` the files below were hashed at: HF tag `v1.1.0`, the model set
/// indra-ocr 1.1.0-1.1.2 declare. If Datalab squashes history again, bump the pin as with
/// `5993a95` -> `ed345c7`.
pub const INDRA_REVISION: &str = "a1283c4b42a273c09480ad50e71461daff50fcc7";

/// What a checkpoint records for the engine's weights and runtime (the manifest `repo` and
/// `mlx_vlm` keys): the repo at its short revision, and the pip package.
pub fn indra_ids() -> (String, String) {
    (
        format!("{INDRA_REPO}@{}", &INDRA_REVISION[..7]),
        format!("indra-ocr {INDRA_PIN}"),
    )
}

/// Pinned llama.cpp release whose `llama-server` serves the models elsewhere (PLAN §9).
pub const LLAMA_BUILD: &str = "b11510";

impl Quant {
    pub fn name(self) -> &'static str {
        match self {
            Self::Q4 => "4bit",
            Self::Q8 => "8bit",
            Self::Bf16 => "bf16",
        }
    }
}

/// Default quant for an engine (indra-ocr has none; its weights are ONNX files).
pub fn default_quant(engine: Engine) -> Quant {
    match engine {
        Engine::PaddleocrVl => Quant::Bf16,
        Engine::DotsOcr | Engine::Apple | Engine::IndraOcr => Quant::Q8,
    }
}

/// Hugging Face repo for an engine/version/quant; `None` if that combination doesn't exist.
/// `version` must already be validated (`None` selects the engine's default). macOS serves
/// MLX weights; elsewhere the GGUF builds in `gguf`.
pub fn repo(engine: Engine, version: Option<&str>, quant: Quant) -> Option<&'static str> {
    if cfg!(not(target_os = "macos")) {
        return gguf(engine, version, quant).map(|g| g.repo);
    }
    use Quant::{Bf16, Q4, Q8};
    let version = version.or_else(|| engine.versions().first().copied());
    Some(match (engine, version, quant) {
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

/// One pinned file of a GGUF build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GgufFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// A GGUF build for llama-server (PLAN §9.3): model and vision projector (mmproj), pinned to a
/// Hugging Face revision and checked by sha256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gguf {
    pub repo: &'static str,
    pub revision: &'static str,
    pub model: GgufFile,
    pub mmproj: GgufFile,
}

const fn f(name: &'static str, size: u64, sha256: &'static str) -> GgufFile {
    GgufFile { name, size, sha256 }
}

const PADDLE_16: (&str, &str) = (
    "PaddlePaddle/PaddleOCR-VL-1.6-GGUF",
    "511b09642bb324401f15f97cc23bc67e8f0a291d",
);
const PADDLE_15: (&str, &str) = (
    "PaddlePaddle/PaddleOCR-VL-1.5-GGUF",
    "cc977c16989848c264d813ec1705cb181b7a21ee",
);
const DOTS_OCR: (&str, &str) = (
    "ggml-org/dots.ocr-GGUF",
    "2c093a32ca360a396bc6d87d60408636130b9d9b",
);

/// The GGUF build of an engine/version/quant served by llama-server off macOS; `None` if that
/// combination has none. The official PaddleOCR-VL GGUFs come in one precision, listed as bf16.
pub fn gguf(engine: Engine, version: Option<&str>, quant: Quant) -> Option<Gguf> {
    use Quant::{Bf16, Q8};
    let version = version.or_else(|| engine.versions().first().copied());
    let ((repo, revision), model, mmproj) = match (engine, version, quant) {
        (Engine::PaddleocrVl, Some("1.6"), Bf16) => (
            PADDLE_16,
            f(
                "PaddleOCR-VL-1.6-GGUF.gguf",
                935_769_056,
                "f3ae46ec885050acf4b3d31944431e1fd90d50664fb09126af4a3c050ba14ee8",
            ),
            f(
                "PaddleOCR-VL-1.6-GGUF-mmproj.gguf",
                881_770_560,
                "204d757d7610d9b3faab10d506d69e5b244e32bf765e2bab2d0167e65e0a058a",
            ),
        ),
        (Engine::PaddleocrVl, Some("1.5"), Bf16) => (
            PADDLE_15,
            f(
                "PaddleOCR-VL-1.5.gguf",
                935_768_992,
                "299051d54faa065abc505cc39b8383ea338fd3020c775ea3e0ba514a7022328c",
            ),
            f(
                "PaddleOCR-VL-1.5-mmproj.gguf",
                881_770_496,
                "e7f1a72400fba517046f90d964e2fa0f4dac7781ee3b1bc5d2022f5f8cecbd87",
            ),
        ),
        (Engine::DotsOcr, Some("dots.ocr"), Q8) => (
            DOTS_OCR,
            f(
                "dots.ocr-Q8_0.gguf",
                1_894_530_272,
                "6f2db1a75215893760b7c7185d52f11715d2e0cb7bff9cefd3f6d409fa1de7e9",
            ),
            f(
                "mmproj-dots.ocr-Q8_0.gguf",
                1_344_068_512,
                "20d3d35bea3a64ead0b4b213f8db0de136e174bb5b75b8e85786c51e8e822423",
            ),
        ),
        (Engine::DotsOcr, Some("dots.ocr"), Bf16) => (
            DOTS_OCR,
            f(
                "dots.ocr-f16.gguf",
                3_560_414_432,
                "2bb30df3e055fb8cf9b643677d3e164f68f184a70b8c8c439ac67e9bd0b409e8",
            ),
            f(
                "mmproj-dots.ocr-f16.gguf",
                2_526_296_992,
                "f462429dbf41729379df4252f84331c9646d80e53f092ccccfd2cb922a3b544e",
            ),
        ),
        _ => return None,
    };
    Some(Gguf {
        repo,
        revision,
        model,
        mmproj,
    })
}

/// The files the default (non-math) indra-ocr pipeline loads: `OrtOCR` opens det, rec, order and
/// meta.json, plus script, rec_cjk, rec_rtl, rec_world, orient and lm*.npz when present; the math
/// files are excluded (`math=False`). Pinned at `INDRA_REVISION`; the sha256 of the LFS files is
/// the LFS oid, meta.json's was computed from the download.
pub const INDRA_FILES: [GgufFile; 12] = [
    f(
        "det.onnx",
        389_948,
        "f2bd4cba23e9c328cf7ec824634e423e191a2c6aacbbf35c62bd79dd70f62f59",
    ),
    f(
        "rec.onnx",
        5_042_161,
        "475972a2710be4a3cbfeb2bc7f3624ffd563de07de42476fc7a290d4e1ece484",
    ),
    f(
        "rec_cjk.onnx",
        10_176_018,
        "4a035516a17e62cdf33a4fe2d88077654cb8841cff024685cdef4e351694ef50",
    ),
    f(
        "rec_rtl.onnx",
        4_812_710,
        "468796bd684486a8388c30c8b4ed2a26e3eeb1f8ff02831c24c2e12c79a2a834",
    ),
    f(
        "rec_world.onnx",
        5_627_990,
        "c8d1e59f21b04966f179d2d2d8e22b230d22110b3f89855bbbf2624a58c40043",
    ),
    f(
        "order.onnx",
        2_973_138,
        "a42db2e8b536ad3d84045a4b7f9b0a72667cf8be4aa02d63f7b38004719fb180",
    ),
    f(
        "orient.onnx",
        371_802,
        "20eaf670f55a7cf9facfe1bdd6c7b8e40be75bbf4d48a3df43f66aa3bbd874fd",
    ),
    f(
        "script.onnx",
        191_630,
        "b6ce4e926e19c243faaf37bd46880398bf2cb4ae56a29e1177a3b017c1280e36",
    ),
    f(
        "lm.npz",
        15_695_957,
        "6c0fb618a363a0acdd05d4f5b0edd9e8146d2d990541b87f6130bbfe17cea58d",
    ),
    f(
        "lm_cjk.npz",
        18_955_160,
        "6d64f13f4f90f4e529bd63340c6df76238d007feaef2abb903d8cd28d92bc2f3",
    ),
    f(
        "lm_world.npz",
        11_641_215,
        "34df5e74a2fcbb9cf5e7f360a559375ffa2e04d2d2943a42308c177bcb444d5d",
    ),
    f(
        "meta.json",
        211_869,
        "4ce4c94b46f5d644033c02e718322014ff4081270f385038c571912b56696339",
    ),
];

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
/// 218 tok/s; dots at 200 DPI
/// prefill 183 tok/s (5035 prompt tokens, 27.5 s) and decode 125 tok/s, so
/// 2 x (27.5 + 8192 / 125) = 186 s. dots has no larger retry (a truncated reply goes to crops).
pub fn limits(engine: Engine) -> Limits {
    match engine {
        Engine::DotsOcr => Limits {
            max_tokens: 8192,
            retry_max_tokens: 8192,
            timeout_secs: 190,
            retry_timeout_secs: 190,
        },
        // indra-ocr makes no model requests; these are never read.
        Engine::IndraOcr => Limits {
            max_tokens: 0,
            retry_max_tokens: 0,
            timeout_secs: 0,
            retry_timeout_secs: 0,
        },
        Engine::PaddleocrVl | Engine::Apple => Limits {
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

/// Measured (dots 8bit, paddle bf16) or proportionally estimated peak physical footprint, GB.
pub fn footprint_gb(engine: Engine, quant: Quant) -> f64 {
    let (base, base_bits) = match engine {
        Engine::DotsOcr => (7.3, 8.0),
        Engine::PaddleocrVl => (3.8, 16.0),
        Engine::Apple => return 0.0,
        // Seven small ONNX files and a few hundred MB of runtime memory; the quant is ignored.
        Engine::IndraOcr => return 0.5,
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

/// RAM check: warn if footprint x 1.5 exceeds RAM, refuse if footprint exceeds RAM x 0.75.
pub fn ram_verdict(engine: Engine, quant: Quant, ram_gb: f64) -> RamVerdict {
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
    fn indra_files_are_the_default_set() {
        let mut names: Vec<&str> = INDRA_FILES.iter().map(|f| f.name).collect();
        names.sort();
        let mut want = vec![
            "det.onnx",
            "rec.onnx",
            "rec_cjk.onnx",
            "rec_rtl.onnx",
            "rec_world.onnx",
            "order.onnx",
            "orient.onnx",
            "script.onnx",
            "lm.npz",
            "lm_cjk.npz",
            "lm_world.npz",
            "meta.json",
        ];
        want.sort();
        assert_eq!(names, want);
        for f in &INDRA_FILES {
            assert!(
                f.sha256.len() == 64
                    && f.sha256
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
                "{}",
                f.name
            );
        }
        assert_eq!(INDRA_REVISION.len(), 40);
        assert!(INDRA_REVISION.bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    #[cfg(target_os = "macos")]
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
    #[cfg(not(target_os = "macos"))]
    fn gguf_repos() {
        assert_eq!(
            repo(Engine::PaddleocrVl, None, Quant::Bf16),
            Some("PaddlePaddle/PaddleOCR-VL-1.6-GGUF")
        );
        assert_eq!(repo(Engine::PaddleocrVl, None, Quant::Q4), None);
        assert_eq!(quants(Engine::PaddleocrVl, None), ["bf16"]);
        assert_eq!(quants(Engine::DotsOcr, None), ["8bit", "bf16"]);
        let dots = gguf(Engine::DotsOcr, None, Quant::Q8).unwrap();
        assert_eq!(dots.repo, "ggml-org/dots.ocr-GGUF");
        assert_eq!(dots.model.name, "dots.ocr-Q8_0.gguf");
        assert!(gguf(Engine::DotsOcr, Some("dots.mocr"), Quant::Q8).is_none());
    }

    #[test]
    fn ram() {
        assert_eq!(
            ram_verdict(Engine::PaddleocrVl, Quant::Bf16, 16.0),
            RamVerdict::Ok
        );
        assert_eq!(
            ram_verdict(Engine::DotsOcr, Quant::Q8, 8.0),
            RamVerdict::Refuse
        );
        assert_eq!(footprint_gb(Engine::DotsOcr, Quant::Q4), 3.65);
    }
}
