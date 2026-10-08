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

// ---------------------------------------------------------------- GPU probe (PLAN §9.2)

/// What kind of GPU the model runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Nvidia,
    /// A Vulkan GPU with its own memory (`uma: 0`).
    Discrete,
    /// Integrated graphics sharing system memory (`uma: 1`): slower than the CPU build here.
    Integrated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub kind: GpuKind,
    /// Total device memory, MiB.
    pub memory_mib: u64,
}

/// The probe's choice: the build to install and the GPU it found (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct Hardware {
    pub variant: Variant,
    pub gpu: Option<Gpu>,
    pub reason: String,
}

/// NVIDIA GPUs (name, total MiB) and the CUDA version the driver supports, from `nvidia-smi`.
fn nvidia() -> Option<(Vec<(String, u64)>, u32)> {
    let header = Command::new("nvidia-smi").output().ok()?;
    if !header.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&header.stdout);
    let cuda = parse_cuda_major(&text)?;
    let query = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    let gpus = parse_nvidia_gpus(&String::from_utf8_lossy(&query.stdout));
    (!gpus.is_empty()).then_some((gpus, cuda))
}

/// "CUDA Version: 13.0" in `nvidia-smi`'s header.
fn parse_cuda_major(text: &str) -> Option<u32> {
    let rest = &text[text.find("CUDA Version:")? + "CUDA Version:".len()..];
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// `name, MiB` lines from `nvidia-smi --query-gpu=name,memory.total`.
fn parse_nvidia_gpus(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            Some((name.trim().to_string(), mib.trim().parse().ok()?))
        })
        .collect()
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

/// Vulkan devices from `llama-server -v --list-devices`: ggml-vulkan's
/// `ggml_vulkan: 0 = NAME (driver) | uma: 1 | ...` line says whether memory is unified, and
/// `  Vulkan0: NAME (16073 MiB, 15109 MiB free)` gives the memory.
fn parse_vulkan_devices(log: &str) -> Vec<Gpu> {
    let mut uma = std::collections::BTreeMap::new();
    for line in log.lines() {
        if let Some(rest) = line.split("ggml_vulkan: ").nth(1)
            && let Some((index, desc)) = rest.split_once(" = ")
            && let Ok(index) = index.trim().parse::<usize>()
            && let Some(flag) = desc.split("uma: ").nth(1)
        {
            uma.insert(index, flag.starts_with('1'));
        }
    }
    log.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Vulkan")?;
            let (index, desc) = rest.split_once(": ")?;
            let index: usize = index.parse().ok()?;
            let open = desc.rfind(" (")?;
            let mib = desc[open + 2..].split(" MiB").next()?.parse().ok()?;
            let kind = if *uma.get(&index)? {
                GpuKind::Integrated
            } else {
                GpuKind::Discrete
            };
            Some(Gpu {
                name: desc[..open].to_string(),
                kind,
                memory_mib: mib,
            })
        })
        .collect()
}

fn vulkan_devices(server: &Path) -> Vec<Gpu> {
    let mut cmd = Command::new(server);
    cmd.args(["-v", "--list-devices"]);
    if let Some(dir) = server.parent() {
        let var = if cfg!(windows) {
            "PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut dirs = vec![dir.to_path_buf()];
        dirs.extend(std::env::var_os(var).iter().flat_map(std::env::split_paths));
        if let Ok(joined) = std::env::join_paths(dirs) {
            cmd.env(var, joined);
        }
    }
    cmd.output()
        .map(|o| {
            let mut log = String::from_utf8_lossy(&o.stdout).into_owned();
            log.push_str(&String::from_utf8_lossy(&o.stderr));
            parse_vulkan_devices(&log)
        })
        .unwrap_or_default()
}

/// Picks the llama.cpp build (PLAN §9.2): `$OCR_LLAMA_VARIANT` if set; else CUDA for an NVIDIA
/// driver that supports CUDA 12 or later (13.x when it supports CUDA 13); else, if Vulkan is installed, the Vulkan build is
/// fetched and asked for its devices, and a GPU with its own memory gets it; else the CPU
/// build. Integrated graphics get the CPU build too: on a Ryzen 9950X's Radeon it was 6x
/// slower than the CPU (docs/spike.md).
pub fn probe() -> Result<Hardware, CliError> {
    let nv = nvidia();
    let nv_gpu = nv.as_ref().and_then(|(gpus, _)| {
        gpus.iter().max_by_key(|g| g.1).map(|(name, mib)| Gpu {
            name: name.clone(),
            kind: GpuKind::Nvidia,
            memory_mib: *mib,
        })
    });
    if let Ok(v) = std::env::var("OCR_LLAMA_VARIANT") {
        let variant = Variant::parse(&v).ok_or_else(|| {
            CliError::Usage(format!(
                "OCR_LLAMA_VARIANT must be one of cuda12, cuda13, vulkan, cpu (got {v})"
            ))
        })?;
        let gpu = if variant.uses_gpu() { nv_gpu } else { None };
        return Ok(Hardware {
            variant,
            gpu,
            reason: "OCR_LLAMA_VARIANT".into(),
        });
    }
    if let (Some((_, cuda)), Some(gpu)) = (nv, nv_gpu) {
        if cuda >= 12 {
            let variant = if cuda >= 13 {
                Variant::Cuda13
            } else {
                Variant::Cuda12
            };
            return Ok(Hardware {
                variant,
                gpu: Some(gpu),
                reason: format!("NVIDIA driver supports CUDA {cuda}"),
            });
        }
        // The CUDA builds need a CUDA 12 driver; an older one may still run Vulkan.
        eprintln!(
            "⚠ the NVIDIA driver supports only CUDA {cuda}; llama.cpp's CUDA builds need 12 or later. \
             Update the driver for the CUDA build; trying Vulkan."
        );
    }
    if vulkan_present() {
        let server = install(Variant::Vulkan)?;
        let devices = vulkan_devices(&server);
        if let Some(gpu) = devices
            .iter()
            .filter(|g| g.kind == GpuKind::Discrete)
            .max_by_key(|g| g.memory_mib)
        {
            return Ok(Hardware {
                variant: Variant::Vulkan,
                gpu: Some(gpu.clone()),
                reason: "Vulkan GPU with its own memory".into(),
            });
        }
        if let Some(gpu) = devices.into_iter().next() {
            return Ok(Hardware {
                variant: Variant::Cpu,
                reason: format!("only integrated graphics ({})", gpu.name),
                gpu: None,
            });
        }
    }
    Ok(Hardware {
        variant: Variant::Cpu,
        gpu: None,
        reason: "no NVIDIA or Vulkan GPU found".into(),
    })
}

/// The no-GPU warning (PLAN §9.2), shown by setup and `server start` on the CPU build.
pub fn cpu_warning() -> &'static str {
    "⚠ No supported GPU found (NVIDIA with CUDA, or a discrete GPU with Vulkan).\n  \
     ocr will run the model on the CPU, which is much slower: paddleocr-vl took about 8 s a \
     page on a 16-core CPU, and olmocr and dots are several times slower than that."
}

/// Whether `engine` may be installed on the CPU build without `--force`: only PaddleOCR-VL
/// (0.9B) is practical there (PLAN §9.5).
pub fn cpu_ok(engine: Engine) -> bool {
    engine == Engine::PaddleocrVl
}

/// Memory an engine needs to run: its GGUF files plus 1.5 GB for the KV cache (sized by
/// `ctx_size`) and compute buffers. PaddleOCR-VL measured 2.1 GB resident on the CPU build for
/// 1.8 GB of files (docs/spike.md).
pub fn need_gb(gguf: &Gguf) -> f64 {
    (gguf.model.size + gguf.mmproj.size) as f64 / 1e9 + 1.5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Ok,
    /// Fits, but tightly: llama.cpp may leave some layers on the CPU.
    Tight,
    TooBig,
}

/// GPU memory: fits under 85% of it, tight up to 100%, too big above. System memory (CPU
/// build), which the OS and other programs share: the RAM rule of PLAN §8.1, tight when 1.5x
/// the need exceeds it, too big above 75% of it.
pub fn fit(need_gb: f64, memory_gb: f64, dedicated: bool) -> Fit {
    let (ok, max) = if dedicated {
        (memory_gb * 0.85, memory_gb)
    } else {
        (memory_gb / 1.5, memory_gb * 0.75)
    };
    if need_gb <= ok {
        Fit::Ok
    } else if need_gb <= max {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

/// Installed system memory, GB (Linux `/proc/meminfo`, Windows via `sysinfo`).
pub fn ram_gb() -> Option<f64> {
    #[cfg(windows)]
    {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        Some(sys.total_memory() as f64 / 1e9)
    }
    #[cfg(not(windows))]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kib: f64 = text
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(kib * 1024.0 / 1e9)
    }
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

/// Every directory of an unpacked build that holds shared libraries (`.so*`, `.dll`). The CUDA
/// runtime archive may unpack beside `llama-server` or apart from it, so the server's library
/// path is built from all of them rather than from the archive layout.
pub fn library_dirs(variant: Variant) -> Vec<PathBuf> {
    let Ok(root) = install_dir(variant) else {
        return Vec::new();
    };
    let is_lib = |p: &Path| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        name.ends_with(".dll") || name.ends_with(".so") || name.contains(".so.")
    };
    let mut dirs = Vec::new();
    let mut queue = vec![root];
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut has_lib = false;
        for p in entries.flatten().map(|e| e.path()) {
            if p.is_dir() {
                queue.push(p);
            } else {
                has_lib |= is_lib(&p);
            }
        }
        if has_lib {
            dirs.push(dir);
        }
    }
    dirs.sort();
    dirs
}

/// Downloads, verifies and unpacks llama.cpp (and the CUDA runtime) for `variant`; returns the
/// `llama-server` path. Archives are kept in `<state>/downloads` so a re-run skips them, and a
/// build already unpacked (its `.unpacked` stamp lists the archives' checksums) is left alone:
/// unpacking over it would replace the files of a running server (and fails on Windows).
pub fn install(variant: Variant) -> Result<PathBuf, CliError> {
    let (archive, cudart) = assets(variant).ok_or_else(|| {
        CliError::Setup(format!(
            "error: llama.cpp {LLAMA_BUILD} has no {} build for this platform",
            variant.name()
        ))
    })?;
    let downloads = state_dir()?.join("downloads");
    let dir = install_dir(variant)?;
    let assets: Vec<Asset> = std::iter::once(archive).chain(cudart).collect();
    let stamp = dir.join(".unpacked");
    let want: String = assets.iter().map(|a| format!("{}\n", a.sha256)).collect();
    if std::fs::read_to_string(&stamp).is_ok_and(|s| s == want)
        && let Some(server) = server_binary(variant)
    {
        println!(
            "llama.cpp {LLAMA_BUILD} {}: already installed",
            variant.name()
        );
        return Ok(server);
    }
    for asset in &assets {
        let file = downloads.join(asset.name);
        fetch::download(&release_url(asset), &file, asset.sha256, asset.name)?;
        fetch::extract(&file, &dir)?;
    }
    std::fs::write(&stamp, want).map_err(anyhow::Error::from)?;
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

/// Highest `--dpi` the server's context is sized for (paddleocr-vl and dots-ocr; olmOCR images
/// are a fixed 1288 px). The server is started without knowing the scans' DPI, so it is sized
/// for this one; a scan above it is warned that long pages may be cut short.
pub const MAX_DPI: u32 = 300;

/// Prompt tokens to budget per request on top of the reply (`engines::limits`), so the KV cache
/// is sized for the work rather than the model's full trained context (131072 tokens per slot
/// for PaddleOCR-VL by default). Image tokens grow with the square of the DPI, so the budget is
/// the measured count scaled to `MAX_DPI`, plus 15%. Measured (docs/spike.md): PaddleOCR-VL
/// 1253 at 150 DPI; dots 1386 at 100 DPI (5035 at 200, so the square overestimates); olmOCR
/// about 1790 at 1288 px.
fn prompt_budget(engine: Engine) -> u32 {
    let scaled = |tokens: u32, dpi: u32| {
        let r = f64::from(MAX_DPI) / f64::from(dpi);
        (f64::from(tokens) * r * r * 1.15).ceil() as u32
    };
    match engine {
        Engine::Olmocr => 2048,
        Engine::PaddleocrVl => scaled(1253, 150),
        _ => scaled(1386, 100),
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
    fn parses_nvidia_smi() {
        let header = "| NVIDIA-SMI 580.82  Driver Version: 580.82  CUDA Version: 13.0 |";
        assert_eq!(parse_cuda_major(header), Some(13));
        assert_eq!(parse_cuda_major("CUDA Version: 12.8"), Some(12));
        assert_eq!(
            parse_nvidia_gpus("NVIDIA GeForce RTX 3090, 24576\n"),
            [("NVIDIA GeForce RTX 3090".to_string(), 24576)]
        );
    }

    #[test]
    fn parses_vulkan_devices() {
        // This machine (integrated Radeon), plus a made-up discrete card.
        let log = "0.00.5 D ggml_vulkan: 0 = AMD Ryzen 9 9950X 16-Core Processor (RADV \
                   RAPHAEL_MENDOCINO) (radv) | uma: 1 | fp16: dot2 | bf16: 0\n\
                   0.00.5 D ggml_vulkan: 1 = AMD Radeon RX 7900 XTX (RADV NAVI31) (radv) | uma: 0 | fp16: 1\n\
                   Available devices:\n  \
                   Vulkan0: AMD Ryzen 9 9950X 16-Core Processor (RADV RAPHAEL_MENDOCINO) (16073 MiB, 15109 MiB free)\n  \
                   Vulkan1: AMD Radeon RX 7900 XTX (RADV NAVI31) (24560 MiB, 24000 MiB free)\n";
        let devices = parse_vulkan_devices(log);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].kind, GpuKind::Integrated);
        assert_eq!(devices[0].memory_mib, 16073);
        assert_eq!(devices[1].kind, GpuKind::Discrete);
        assert_eq!(devices[1].name, "AMD Radeon RX 7900 XTX (RADV NAVI31)");
        // Without the verbose ggml_vulkan lines the kind is unknown, so nothing is reported.
        assert!(parse_vulkan_devices("  Vulkan0: X (1 MiB, 1 MiB free)").is_empty());
    }

    #[test]
    fn memory_tiers() {
        let paddle = engines::gguf(Engine::PaddleocrVl, None, crate::cli::Quant::Bf16).unwrap();
        let olmocr = engines::gguf(Engine::Olmocr, None, crate::cli::Quant::Q8).unwrap();
        let olmocr16 = engines::gguf(Engine::Olmocr, None, crate::cli::Quant::Bf16).unwrap();
        // An RTX 3090 (24 GiB): everything fits, bf16 olmOCR included.
        for g in [paddle, olmocr, olmocr16] {
            assert_eq!(fit(need_gb(&g), 25.8, true), Fit::Ok);
        }
        // An 8 GB card: olmOCR 8-bit (about 10.9 GB) does not.
        assert_eq!(fit(need_gb(&olmocr), 8.6, true), Fit::TooBig);
        assert_eq!(fit(need_gb(&paddle), 8.6, true), Fit::Ok);
        // CPU build with 16 GB of RAM: olmOCR is tight, Paddle fine.
        assert_eq!(fit(need_gb(&olmocr), 16.0, false), Fit::Tight);
        assert_eq!(fit(need_gb(&paddle), 16.0, false), Fit::Ok);
    }

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
        // Paddle: 8192 retry tokens + 5764 prompt (1253 x 4 x 1.15, 300 DPI) -> 14336 per slot.
        assert_eq!(ctx_size(Engine::PaddleocrVl, 1), 14336);
        assert_eq!(ctx_size(Engine::PaddleocrVl, 2), 28672);
        assert_eq!(ctx_size(Engine::Olmocr, 1), 10240);
        // dots: 8192 + 14346 (1386 x 9 x 1.15) -> 23552.
        assert_eq!(ctx_size(Engine::DotsOcr, 1), 23552);
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
