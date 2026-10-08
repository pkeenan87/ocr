//! llama.cpp's `llama-server` as the model server off macOS (PLAN §9, Phase 7d-1): the pinned
//! release archives per platform and GPU, the GPU probe that picks one, installation into the
//! state dir, pdfium installation, and the server's argv.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::Engine;
use crate::config::state_dir;
use crate::engines::{self, Gguf};
use crate::error::CliError;
use crate::fetch;

pub use crate::engines::LLAMA_BUILD;

/// Which llama.cpp build to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// NVIDIA, CUDA 12.x runtime (drivers that report CUDA 12).
    Cuda12,
    /// NVIDIA, CUDA 13.x runtime.
    Cuda13,
    /// Any Vulkan GPU (AMD, Intel, NVIDIA).
    Vulkan,
    Cpu,
}

impl Variant {
    pub const ALL: [Variant; 4] = [Self::Cuda12, Self::Cuda13, Self::Vulkan, Self::Cpu];

    pub fn name(self) -> &'static str {
        match self {
            Self::Cuda12 => "cuda12",
            Self::Cuda13 => "cuda13",
            Self::Vulkan => "vulkan",
            Self::Cpu => "cpu",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == s)
    }

    /// Whether model layers are offloaded to a GPU.
    pub fn uses_gpu(self) -> bool {
        self != Self::Cpu
    }
}

/// One pinned release asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    pub name: &'static str,
    pub sha256: &'static str,
}

const fn a(name: &'static str, sha256: &'static str) -> Asset {
    Asset { name, sha256 }
}

/// The llama.cpp archive for `variant` on this platform, plus the CUDA runtime archive for the
/// CUDA builds. `None` where no build is published (or the platform is not x86-64 Linux or
/// Windows). Checksums are the release's published asset digests.
pub fn assets(variant: Variant) -> Option<(Asset, Option<Asset>)> {
    use Variant::*;
    let linux = cfg!(all(target_os = "linux", target_arch = "x86_64"));
    let windows = cfg!(all(windows, target_arch = "x86_64"));
    Some(match variant {
        Cuda12 if linux => (
            a(
                "llama-b11510-bin-ubuntu-cuda-12.8-x64.tar.gz",
                "99551550ff7559886f86d7bf418ad932efe0e827bf5b21e36617fb41426a226c",
            ),
            Some(a(
                "cudart-llama-b11510-bin-ubuntu-cuda-12.8-x64.tar.gz",
                "03440e670a7c40baa6e3f31c77d073ecda8e13014dcce91fac1533a2976eb6be",
            )),
        ),
        Cuda13 if linux => (
            a(
                "llama-b11510-bin-ubuntu-cuda-13.4-x64.tar.gz",
                "73eab2787dd2f116541d7aeac170c44fa8ecc4ccabbf6e061937054fa1c8eb6f",
            ),
            Some(a(
                "cudart-llama-b11510-bin-ubuntu-cuda-13.4-x64.tar.gz",
                "2ec858293ec4b29fdcb8083d118484cb52dfa1a6a11e90ab2baa2f8a761960f1",
            )),
        ),
        Vulkan if linux => (
            a(
                "llama-b11510-bin-ubuntu-vulkan-x64.tar.gz",
                "c496f53589b3505f5082cfafbaa9298b8bd19204694e6a471ee00207cbdb8822",
            ),
            None,
        ),
        Cpu if linux => (
            a(
                "llama-b11510-bin-ubuntu-x64.tar.gz",
                "3301b0208d999f28c4ca4b9690d8d86ec9df78e9f30fd7ec791c361e0fad55fb",
            ),
            None,
        ),
        Cuda12 if windows => (
            a(
                "llama-b11510-bin-win-cuda-12.4-x64.zip",
                "602da68faee0cf887bb473d188fed56fb0828aa2552dc556cc52fc8f6a3f9b5e",
            ),
            Some(a(
                "cudart-llama-bin-win-cuda-12.4-x64.zip",
                "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6",
            )),
        ),
        Cuda13 if windows => (
            a(
                "llama-b11510-bin-win-cuda-13.4-x64.zip",
                "b7299dc9dca0208a12b745ccef5b2004c466ee570035f805edfe3e9af25ac5e4",
            ),
            Some(a(
                "cudart-llama-bin-win-cuda-13.4-x64.zip",
                "738f8c251ac22b70c3ae6f83a10cf222725df0395246a2cf58f32bdb85fbe668",
            )),
        ),
        Vulkan if windows => (
            a(
                "llama-b11510-bin-win-vulkan-x64.zip",
                "1137d7ffa61103f651d9d0e3e8f43b97d4df1c2fa44f768bf8275701b4d38891",
            ),
            None,
        ),
        Cpu if windows => (
            a(
                "llama-b11510-bin-win-cpu-x64.zip",
                "e56a5c6713751e5b5943271558a8abe46e83e0602232ba32a71d2dea789d8fe9",
            ),
            None,
        ),
        _ => return None,
    })
}

fn release_url(asset: &Asset) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_BUILD}/{}",
        asset.name
    )
}

// ---------------------------------------------------------------- GPU probe

/// The CUDA version the NVIDIA driver supports (`nvidia-smi`'s "CUDA Version: 13.0"), if an
/// NVIDIA GPU and driver are present.
fn nvidia_cuda_major() -> Option<u32> {
    let out = Command::new("nvidia-smi").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let rest = &text[text.find("CUDA Version:")? + "CUDA Version:".len()..];
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Whether a Vulkan driver is installed (an ICD manifest on Linux, the loader on Windows).
fn vulkan_present() -> bool {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        return Path::new(&root).join("System32/vulkan-1.dll").is_file();
    }
    ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "json"))
}

/// Picks the llama.cpp build: `$OCR_LLAMA_VARIANT` if set, else CUDA for an NVIDIA driver
/// (13.x when the driver supports CUDA 13), else Vulkan if a driver is installed, else CPU.
/// Returns the variant and why it was chosen.
pub fn detect_variant() -> Result<(Variant, String), CliError> {
    if let Ok(v) = std::env::var("OCR_LLAMA_VARIANT") {
        let variant = Variant::parse(&v).ok_or_else(|| {
            CliError::Usage(format!(
                "OCR_LLAMA_VARIANT must be one of cuda12, cuda13, vulkan, cpu (got {v})"
            ))
        })?;
        return Ok((variant, "OCR_LLAMA_VARIANT".into()));
    }
    if let Some(major) = nvidia_cuda_major() {
        let v = if major >= 13 {
            Variant::Cuda13
        } else {
            Variant::Cuda12
        };
        return Ok((v, format!("NVIDIA driver supports CUDA {major}")));
    }
    if vulkan_present() {
        return Ok((Variant::Vulkan, "Vulkan driver found".into()));
    }
    Ok((Variant::Cpu, "no NVIDIA or Vulkan driver found".into()))
}

// ---------------------------------------------------------------- install

fn server_exe() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Where a variant is unpacked.
pub fn install_dir(variant: Variant) -> Result<PathBuf, CliError> {
    Ok(state_dir()?
        .join("llama.cpp")
        .join(format!("{LLAMA_BUILD}-{}", variant.name())))
}

/// The installed `llama-server` of `variant`, if any.
pub fn server_binary(variant: Variant) -> Option<PathBuf> {
    fetch::find_file(&install_dir(variant).ok()?, server_exe())
}

/// Downloads, verifies and unpacks llama.cpp (and the CUDA runtime) for `variant`; returns the
/// `llama-server` path. Archives are kept in `<state>/downloads` so a re-run skips them.
pub fn install(variant: Variant) -> Result<PathBuf, CliError> {
    let (archive, cudart) = assets(variant).ok_or_else(|| {
        CliError::Setup(format!(
            "error: llama.cpp {LLAMA_BUILD} has no {} build for this platform",
            variant.name()
        ))
    })?;
    let downloads = state_dir()?.join("downloads");
    let dir = install_dir(variant)?;
    for asset in std::iter::once(archive).chain(cudart) {
        let file = downloads.join(asset.name);
        fetch::download(&release_url(&asset), &file, asset.sha256, asset.name)?;
        fetch::extract(&file, &dir)?;
    }
    server_binary(variant).ok_or_else(|| {
        CliError::Setup(format!(
            "error: {} not found after unpacking llama.cpp into {}",
            server_exe(),
            dir.display()
        ))
    })
}

/// Pinned pdfium (`chromium/7881`, see `vision::PDFIUM_RELEASE`) for this platform.
fn pdfium_asset() -> Option<Asset> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(a(
            "pdfium-linux-x64.tgz",
            "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
        ))
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(a(
            "pdfium-win-x64.tgz",
            "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
        ))
    } else {
        None
    }
}

/// Installs pdfium into `<state>/pdfium/`, where `vision` looks for it.
pub fn install_pdfium() -> Result<(), CliError> {
    let asset = pdfium_asset()
        .ok_or_else(|| CliError::Setup("error: no pinned pdfium build for this platform".into()))?;
    let state = state_dir()?;
    let lib_name = if cfg!(windows) {
        "pdfium.dll"
    } else {
        "libpdfium.so"
    };
    let target = state.join("pdfium").join(lib_name);
    let archive = state.join("downloads").join(asset.name);
    if target.is_file() && fetch::is_verified(&archive, asset.sha256) {
        println!("pdfium: already installed");
        return Ok(());
    }
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/7881/{}",
        asset.name
    );
    fetch::download(&url, &archive, asset.sha256, asset.name)?;
    let unpacked = state.join("downloads").join("pdfium-7881");
    fetch::extract(&archive, &unpacked)?;
    let lib = fetch::find_file(&unpacked, lib_name)
        .ok_or_else(|| CliError::Setup(format!("error: {lib_name} not found in {}", asset.name)))?;
    std::fs::create_dir_all(target.parent().unwrap()).map_err(anyhow::Error::from)?;
    std::fs::copy(&lib, &target).map_err(anyhow::Error::from)?;
    let _ = std::fs::remove_dir_all(&unpacked);
    println!("pdfium installed ({})", target.display());
    Ok(())
}

/// Where a GGUF file is stored: `<state>/models/<owner>/<repo>/<file>`.
pub fn model_path(gguf: &Gguf, file: &str) -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("models").join(gguf.repo).join(file))
}

/// Downloads and verifies an engine's GGUF model and projector.
pub fn install_weights(gguf: &Gguf) -> Result<(), CliError> {
    for file in [gguf.model, gguf.mmproj] {
        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            gguf.repo, gguf.revision, file.name
        );
        let label = format!("{} ({:.1} GB)", file.name, file.size as f64 / 1e9);
        fetch::download(&url, &model_path(gguf, file.name)?, file.sha256, &label)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- server argv

/// Prompt tokens to budget per request on top of the reply (`engines::limits`), so the KV cache
/// is sized for the work rather than the model's full trained context.
fn prompt_budget(engine: Engine) -> u32 {
    match engine {
        Engine::Olmocr => 2048,
        _ => 6144,
    }
}

/// `--ctx-size`: llama-server divides it between slots, so it is per-slot need x slots.
pub fn ctx_size(engine: Engine, slots: usize) -> u32 {
    let per_slot = engines::limits(engine).retry_max_tokens + prompt_budget(engine);
    per_slot.div_ceil(1024) * 1024 * slots.max(1) as u32
}

/// llama-server's arguments after the program name. `--model` and `--port` stay in long form:
/// `server::is_ours` and the test fake server look for them.
pub fn server_args(
    model: &Path,
    mmproj: &Path,
    port: u16,
    engine: Engine,
    slots: usize,
    variant: Variant,
) -> Vec<String> {
    let mut args = vec![
        "--model".to_string(),
        model.to_string_lossy().into_owned(),
        "--mmproj".into(),
        mmproj.to_string_lossy().into_owned(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--parallel".into(),
        slots.to_string(),
        "--ctx-size".into(),
        ctx_size(engine, slots).to_string(),
    ];
    if variant.uses_gpu() {
        args.extend(["--n-gpu-layers".into(), "999".into()]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_round_trip() {
        for v in Variant::ALL {
            assert_eq!(Variant::parse(v.name()), Some(v));
        }
        assert_eq!(Variant::parse("rocm"), None);
    }

    #[test]
    fn every_variant_has_a_build_here() {
        if cfg!(all(
            any(target_os = "linux", windows),
            target_arch = "x86_64"
        )) {
            for v in Variant::ALL {
                let (archive, cudart) = assets(v).unwrap();
                assert!(archive.name.contains(LLAMA_BUILD) && archive.sha256.len() == 64);
                assert_eq!(
                    cudart.is_some(),
                    matches!(v, Variant::Cuda12 | Variant::Cuda13)
                );
            }
        }
    }

    #[test]
    fn context_is_sized_per_slot() {
        // Paddle: 8192 retry tokens + 6144 prompt = 14336 per slot.
        assert_eq!(ctx_size(Engine::PaddleocrVl, 1), 14336);
        assert_eq!(ctx_size(Engine::PaddleocrVl, 2), 28672);
        assert_eq!(ctx_size(Engine::Olmocr, 1), 10240);
        let args = server_args(
            Path::new("m.gguf"),
            Path::new("p.gguf"),
            8080,
            Engine::DotsOcr,
            1,
            Variant::Cpu,
        );
        assert!(args.windows(2).any(|w| w == ["--port", "8080"]));
        assert!(!args.contains(&"--n-gpu-layers".to_string()));
    }
}
