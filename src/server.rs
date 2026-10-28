//! Model server management: setup, start/stop/status/restart (PLAN §4.1).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::cli::{Engine, Quant, SetupArgs, StartArgs};
use crate::config::{Config, EngineEntry, Runtime, state_dir};
use crate::engines::{self, MLX_VLM_PIN, RamVerdict};
use crate::error::CliError;

const UV_INSTALL: &str = if cfg!(windows) {
    "powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\""
} else {
    "curl -LsSf https://astral.sh/uv/install.sh | sh"
};

/// What `server.json` records about the server we spawned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerInfo {
    pub pid: u32,
    pub engine: String,
    #[serde(default)]
    pub version: Option<String>,
    pub quant: String,
    pub repo: String,
    pub port: u16,
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Requests the server runs at once (llama-server `--parallel`). Absent in mlx-vlm
    /// `server.json` files, which carry it as `--max-num-seqs` in `argv`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<usize>,
    pub started_at: String,
}

fn info_path() -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("server.json"))
}

fn log_path() -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("server.log"))
}

fn venv_dir() -> Result<PathBuf, CliError> {
    Ok(state_dir()?.join("venv"))
}

/// The `uv` to use, or the setup error telling how to install it.
fn require_uv() -> Result<PathBuf, CliError> {
    find_uv().ok_or_else(|| {
        CliError::Setup(format!(
            "error: `uv` is required but was not found. Install it with:\n  {UV_INSTALL}"
        ))
    })
}

// ---------------------------------------------------------------- setup

/// `uv` on `PATH`, else where its installer puts it (`~/.local/bin`; `uv.exe` on Windows).
fn find_uv() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "uv.exe" } else { "uv" };
    let on_path = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(exe))
            .find(|c| c.is_file())
    });
    on_path.or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".local/bin").join(exe))
            .filter(|c| c.is_file())
    })
}

fn ram_gb() -> Option<f64> {
    let out = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let bytes: f64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
    Some(bytes / (1u64 << 30) as f64)
}

fn python_out(python: &Path, code: &str) -> Option<String> {
    let out = Command::new(python).args(["-c", code]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_checked(cmd: &mut Command, what: &str) -> Result<(), CliError> {
    let status = cmd
        .status()
        .map_err(|e| CliError::Setup(format!("error: cannot run {what}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Setup(format!("error: {what} failed ({status})")))
    }
}

/// Creates the venv (Python 3.12) if it is missing and installs `dist==pin` into it unless that
/// version is already there. Returns the venv's Python version.
fn ensure_venv_pkg(uv: &Path, venv: &Path, dist: &str, pin: &str) -> Result<String, CliError> {
    let python = crate::indra::venv_python(venv);
    if !python.exists() {
        println!("creating Python 3.12 environment in {}", venv.display());
        run_checked(
            Command::new(uv)
                .args(["venv", "--python", "3.12"])
                .arg(venv),
            "uv venv",
        )?;
    }
    if pkg_version(&python, dist).as_deref() == Some(pin) {
        println!("{dist} {pin} already installed");
    } else {
        println!("installing {dist}=={pin}");
        run_checked(
            Command::new(uv)
                .args(["pip", "install", "--python"])
                .arg(&python)
                .arg(format!("{dist}=={pin}")),
            "uv pip install",
        )?;
    }
    if pkg_version(&python, dist).is_none() {
        return Err(CliError::Setup(format!(
            "error: {dist} is not importable in the venv"
        )));
    }
    python_out(&python, "import platform; print(platform.python_version())")
        .ok_or_else(|| CliError::Setup("error: cannot run the venv's python".into()))
}

fn pkg_version(python: &Path, dist: &str) -> Option<String> {
    python_out(
        python,
        &format!("import importlib.metadata as m; print(m.version('{dist}'))"),
    )
}

/// Creates the venv and installs the pinned mlx-vlm; returns the recorded runtime versions.
fn ensure_venv(uv: &Path, venv: &Path) -> Result<Runtime, CliError> {
    let py = ensure_venv_pkg(uv, venv, "mlx-vlm", MLX_VLM_PIN)?;
    Ok(Runtime {
        python: py,
        mlx_vlm: MLX_VLM_PIN.to_string(),
        ..Default::default()
    })
}

/// Installs indra-ocr (every platform): its own venv with the pinned package, the twelve pinned
/// ONNX/npz weights, the helper script, and (off macOS) pdfium for `ocr`'s own page reading. No
/// server, no llama.cpp, no GPU check. `fake` (OCR_FAKE_INSTALL) only writes the helper.
fn install_indra(fake: bool) -> Result<EngineEntry, CliError> {
    use crate::indra;
    let dir = indra::indra_dir()?;
    fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;
    if !fake {
        let uv = require_uv()?;
        let py = ensure_venv_pkg(&uv, &indra::venv_dir()?, "indra-ocr", engines::INDRA_PIN)?;
        println!("indra-ocr {} on Python {py}", engines::INDRA_PIN);
        let weights = indra::model_dir()?;
        for file in engines::INDRA_FILES {
            let url = format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                engines::INDRA_REPO,
                engines::INDRA_REVISION,
                file.name
            );
            let label = format!("{} ({:.1} MB)", file.name, file.size as f64 / 1e6);
            crate::fetch::download(&url, &weights.join(file.name), file.sha256, &label)?;
        }
        #[cfg(not(target_os = "macos"))]
        crate::llama::install_pdfium()?;
    }
    let helper = indra::helper_path()?;
    fs::write(&helper, indra::HELPER).with_context(|| format!("writing {}", helper.display()))?;
    Ok(EngineEntry {
        installed: true,
        version: Some(engines::INDRA_PIN.into()),
        repo: Some(engines::INDRA_REPO.into()),
        quant: None,
    })
}

/// Installs the requested model engines (runtime, weights) and records them in the config.
pub fn setup_models(args: &SetupArgs, cfg: &mut Config) -> Result<(), CliError> {
    let requested = args.requested_models();
    if requested.is_empty() {
        return Ok(());
    }
    let fake = std::env::var_os("OCR_FAKE_INSTALL").is_some_and(|v| v == "1");
    // indra-ocr has no server, quant or version: it is checked and installed on its own.
    let indra = requested.contains(&Engine::IndraOcr);
    if indra && args.model.contains(&Engine::IndraOcr) {
        if args.quant.is_some() {
            return Err(CliError::Usage(
                "engine 'indra-ocr' has no --quant (it runs ONNX models on the CPU)".into(),
            ));
        }
        Engine::IndraOcr.resolve_version(args.model_version.as_deref())?;
    }
    let models: Vec<Engine> = requested
        .into_iter()
        .filter(|e| *e != Engine::IndraOcr)
        .collect();

    // Resolve and check everything up front so nothing is downloaded for a doomed request.
    let mut plan = Vec::new();
    for engine in models {
        let version = engine.resolve_version(args.model_version.as_deref())?;
        let quant = args.quant.unwrap_or(engines::default_quant(engine));
        let repo = engines::repo(engine, version, quant).ok_or_else(|| {
            CliError::Usage(format!(
                "engine '{}'{} has no {} build; available quants: {}",
                engine.name(),
                version.map(|v| format!(" {v}")).unwrap_or_default(),
                quant.name(),
                engines::quants(engine, version).join(", ")
            ))
        })?;
        // Off macOS the model lives in GPU memory; 7e checks VRAM instead.
        if !fake && cfg!(target_os = "macos") {
            check_ram(engine, quant, args.force)?;
        }
        plan.push((engine, version, quant, repo));
    }

    let runtime = if plan.is_empty() {
        None
    } else if fake && cfg!(target_os = "macos") {
        Some(Runtime {
            python: "fake".into(),
            mlx_vlm: MLX_VLM_PIN.into(),
            ..Default::default()
        })
    } else if fake {
        Some(Runtime {
            llama_cpp: Some(engines::LLAMA_BUILD.into()),
            llama_variant: Some("cpu".into()),
            ..Default::default()
        })
    } else if cfg!(not(target_os = "macos")) {
        let engines: Vec<_> = plan.iter().map(|p| (p.0, p.1, p.2)).collect();
        Some(install_llama_runtime(&engines, args.force)?)
    } else {
        let uv = require_uv()?;
        std::fs::create_dir_all(state_dir()?).map_err(anyhow::Error::from)?;
        Some(ensure_venv(&uv, &venv_dir()?)?)
    };
    // An indra-ocr-only setup leaves the llama.cpp / mlx-vlm runtime record alone.
    if runtime.is_some() {
        cfg.runtime = runtime;
    }

    for (engine, version, quant, repo) in plan {
        if !fake && cfg!(target_os = "macos") {
            println!("downloading {repo}");
            run_checked(
                Command::new(venv_dir()?.join("bin/hf")).args(["download", repo]),
                "hf download",
            )?;
        }
        #[cfg(not(target_os = "macos"))]
        if !fake {
            let gguf = engines::gguf(engine, version, quant).expect("resolved above");
            crate::llama::install_weights(&gguf)?;
        }
        cfg.engines.insert(
            engine.name().into(),
            EngineEntry {
                installed: true,
                version: version.map(str::to_string),
                quant: Some(quant.name().into()),
                repo: Some(repo.into()),
            },
        );
        cfg.save()?;
        println!("{} ready ({repo})", engine.name());
    }
    if indra {
        let entry = install_indra(fake)?;
        let repo = entry.repo.clone().unwrap_or_default();
        cfg.engines.insert(Engine::IndraOcr.name().into(), entry);
        cfg.save()?;
        println!(
            "indra-ocr ready ({repo}); it runs without a server: `ocr scan --model indra-ocr`"
        );
    }
    Ok(())
}

/// Off macOS (PLAN §9.2): probes the GPU, picks the llama.cpp build, checks that every engine
/// being installed fits its memory (refusing without `--force` when it doesn't, and refusing
/// dots on the CPU build), then installs llama.cpp and pdfium.
#[cfg(not(target_os = "macos"))]
fn install_llama_runtime(
    engines: &[(Engine, Option<&str>, Quant)],
    force: bool,
) -> Result<Runtime, CliError> {
    use crate::llama::{self, Fit};
    let hw = llama::probe()?;
    let variant = hw.variant;
    let gpu = hw
        .gpu
        .as_ref()
        .map(|g| format!("{} ({} MiB)", g.name, g.memory_mib));
    println!(
        "llama.cpp {} ({} build: {})",
        llama::LLAMA_BUILD,
        variant.name(),
        hw.reason
    );
    if let Some(g) = &gpu {
        println!("GPU: {g}");
    }
    if !variant.uses_gpu() {
        eprintln!("{}", llama::cpu_warning());
    }
    let (memory_gb, dedicated, what) = match (&hw.gpu, variant.uses_gpu()) {
        (Some(g), true) => (
            Some(g.memory_mib as f64 * 1048576.0 / 1e9),
            true,
            "GPU memory",
        ),
        _ => (llama::ram_gb(), false, "system memory"),
    };
    for &(engine, version, quant) in engines {
        let name = engine.name();
        if !variant.uses_gpu() && !llama::cpu_ok(engine) {
            if !force {
                return Err(CliError::Setup(format!(
                    "error: {name} is too slow without a GPU; use paddleocr-vl, or pass --force to install it anyway"
                )));
            }
            eprintln!("⚠ {name} without a GPU will be very slow; continuing because of --force");
        }
        let Some(gguf) = engines::gguf(engine, version, quant) else {
            continue;
        };
        let need = llama::need_gb(&gguf);
        let Some(have) = memory_gb else {
            eprintln!("⚠ cannot determine {what}; skipping the memory check for {name}");
            continue;
        };
        let q = quant.name();
        match llama::fit(need, have, dedicated) {
            Fit::Ok => {}
            Fit::Tight => eprintln!(
                "⚠ {name} {q} needs about {need:.1} GB and this machine has {have:.1} GB of {what}, so it may be tight."
            ),
            Fit::TooBig if force => eprintln!(
                "⚠ {name} {q} needs about {need:.1} GB but this machine has {have:.1} GB of {what}; continuing because of --force"
            ),
            Fit::TooBig => {
                return Err(CliError::Setup(format!(
                    "error: {name} {q} needs about {need:.1} GB but this machine has only {have:.1} GB of {what}. \
                     Pass --force to install anyway."
                )));
            }
        }
    }
    llama::install(variant)?;
    llama::install_pdfium()?;
    Ok(Runtime {
        llama_cpp: Some(llama::LLAMA_BUILD.into()),
        llama_variant: Some(variant.name().into()),
        gpu,
        ..Default::default()
    })
}

#[cfg(target_os = "macos")]
fn install_llama_runtime(
    _: &[(Engine, Option<&str>, Quant)],
    _: bool,
) -> Result<Runtime, CliError> {
    unreachable!("macOS runs mlx-vlm")
}

fn check_ram(engine: Engine, quant: Quant, force: bool) -> Result<(), CliError> {
    let Some(ram) = ram_gb() else {
        eprintln!("⚠ cannot determine installed RAM; skipping the memory check");
        return Ok(());
    };
    let fp = engines::footprint_gb(engine, quant);
    let name = engine.name();
    match engines::ram_verdict(engine, quant, ram) {
        RamVerdict::Ok => {}
        RamVerdict::Warn => {
            let hint = if quant != Quant::Q4 {
                " Consider `--quant 4bit`."
            } else {
                ""
            };
            eprintln!(
                "⚠ {name} {} needs about {fp:.1} GB; this Mac has {ram:.0} GB, so it may be tight.{hint}",
                quant.name()
            );
        }
        RamVerdict::Refuse if force => eprintln!(
            "⚠ {name} {} needs about {fp:.1} GB but this Mac has {ram:.0} GB; continuing because of --force",
            quant.name()
        ),
        RamVerdict::Refuse => {
            return Err(CliError::Setup(format!(
                "error: {name} {} needs about {fp:.1} GB but this Mac has only {ram:.0} GB. \
                 Try `--quant 4bit`, or pass --force to install anyway.",
                quant.name()
            )));
        }
    }
    Ok(())
}

// ------------------------------------------------------- process helpers

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence.
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn killpg(pid: u32, sig: i32) {
    // SAFETY: plain signal delivery to the process group we created (pgid == pid).
    unsafe {
        libc::killpg(pid as i32, sig);
    }
}

/// Asks the server's process group to exit (SIGTERM).
#[cfg(unix)]
fn terminate(pid: u32) {
    killpg(pid, libc::SIGTERM);
}

/// Kills the server's process group outright (SIGKILL).
#[cfg(unix)]
fn force_kill(pid: u32) {
    killpg(pid, libc::SIGKILL);
}

#[cfg(unix)]
fn process_command(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// The process `pid`, refreshed with its command line and memory (`footprint` reads it).
#[cfg(windows)]
fn process<T>(pid: u32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_memory(),
    );
    sys.process(pid).map(f)
}

/// Whether process `info.pid` started within a few seconds of `info.started_at` (recorded right
/// after the spawn).
#[cfg(windows)]
fn started_with(info: &ServerInfo) -> bool {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(&info.started_at) else {
        return false;
    };
    process(info.pid, |p| p.start_time())
        .is_some_and(|start| (start as i64 - t.timestamp()).abs() <= 5)
}

#[cfg(not(windows))]
fn started_with(_: &ServerInfo) -> bool {
    false
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    process(pid, |_| ()).is_some()
}

/// Windows has no SIGTERM for a console-less child, so the server is stopped outright.
#[cfg(windows)]
fn terminate(pid: u32) {
    force_kill(pid);
}

/// Kills the server and anything it started (`taskkill /T`; a `.cmd` wrapper runs its program
/// as a child), falling back to TerminateProcess on the server alone.
#[cfg(windows)]
fn force_kill(pid: u32) {
    let tree = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !tree {
        process(pid, |p| p.kill());
    }
}

/// The process's command line, or, if Windows won't show it, its executable name (which
/// `is_ours` then matches without the port).
#[cfg(windows)]
fn process_command(pid: u32) -> Option<String> {
    let cmd = process(pid, |p| {
        let line = p
            .cmd()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        if line.is_empty() {
            p.name().to_string_lossy().into_owned()
        } else {
            line
        }
    })?;
    (!cmd.is_empty()).then_some(cmd)
}

fn is_ours(info: &ServerInfo) -> bool {
    if !pid_alive(info.pid) {
        return false;
    }
    let Some(prog) = info
        .argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|n| n.to_str())
    else {
        return false;
    };
    let port = format!("--port {}", info.port);
    process_command(info.pid).is_some_and(|c| {
        if cfg!(windows) {
            // Case-insensitive file system. A bare executable name (no command line) carries no
            // port, so the process must also have started when `server.json` says ours did: a
            // reused pid of some other llama-server is not ours.
            let (c, prog) = (c.to_lowercase(), prog.to_lowercase());
            c.contains(&prog) && (c.contains(&port) || (!c.contains(' ') && started_with(info)))
        } else {
            c.contains(prog) && c.contains(&port)
        }
    })
}

/// The verified running server, if any. A stale `server.json` is removed.
pub fn running() -> Option<ServerInfo> {
    let path = info_path().ok()?;
    let text = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<ServerInfo>(&text) {
        Ok(info) if is_ours(&info) => Some(info),
        _ => {
            let _ = fs::remove_file(&path);
            None
        }
    }
}

/// Stops a verified server: removes `server.json`, then SIGTERM to its group and SIGKILL after 10 s
/// (on Windows, TerminateProcess).
pub fn stop_server(info: &ServerInfo) {
    // Remove `server.json` first: a scan that loses its connection to this server must be able
    // to tell a user stop (file gone) from a crash (file left behind).
    if let Ok(p) = info_path() {
        let _ = fs::remove_file(p);
    }
    terminate(info.pid);
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(info.pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if pid_alive(info.pid) {
        force_kill(info.pid);
        let end = Instant::now() + Duration::from_secs(3);
        while pid_alive(info.pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

// ----------------------------------------------------------------- HTTP

pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

fn health_ok(port: u16, timeout: Duration) -> bool {
    agent(timeout)
        .get(format!("http://127.0.0.1:{port}/health"))
        .call()
        .is_ok_and(|r| r.status().as_u16() == 200)
}

fn probe_ok(port: u16, repo: &str, timeout: Duration) -> bool {
    let body = serde_json::json!({
        "model": repo,
        "max_tokens": 8,
        "temperature": 0,
        "messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", base64(&white_png()))}},
            {"type": "text", "text": "OCR:"},
        ]}],
    });
    agent(timeout)
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .is_ok_and(|r| r.status().as_u16() == 200)
}

pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A 32x32 white RGB PNG (stored deflate blocks, no compression needed).
fn white_png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    let raw: Vec<u8> = (0..32)
        .flat_map(|_| std::iter::once(0u8).chain(std::iter::repeat_n(0xff, 96)))
        .collect();
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend((raw.len() as u16).to_le_bytes());
    z.extend((!(raw.len() as u16)).to_le_bytes());
    z.extend(&raw);
    z.extend(((b << 16) | a).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend(32u32.to_be_bytes());
    ihdr.extend(32u32.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

// ---------------------------------------------------------------- start

fn ready_timeout() -> Duration {
    let secs = std::env::var("OCR_READY_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    Duration::from_secs(secs)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Last 20 lines of the log written since `offset`.
fn log_tail(path: &Path, offset: u64) -> String {
    let mut text = String::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.seek(SeekFrom::Start(offset));
        let mut bytes = Vec::new();
        let _ = f.read_to_end(&mut bytes);
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

fn weights_missing(tail: &str) -> bool {
    let t = tail.to_lowercase();
    t.contains("localentrynotfound")
        || t.contains("offline mode")
        || t.contains("hf_hub_offline")
        || t.contains("cannot find the requested files in the local cache")
        // llama-server
        || t.contains("failed to load model")
        || t.contains("error loading model")
        || t.contains("failed to open gguf")
}

/// Windows starts children with every inheritable handle of the parent, so a detached server
/// would also hold `ocr`'s own stdout and stderr: whatever reads `ocr`'s output (a pipe to `tee`,
/// a test harness) then waits for the server to exit. Unix closes them in the child. The flag
/// is cleared on the originals only; std hands a child its stdio as fresh duplicates, so
/// `Stdio::inherit` keeps working for other children.
#[cfg(windows)]
fn keep_std_handles_from_children() {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE: (DWORD)-10, -11, -12.
    for which in [-10i32, -11, -12] {
        // SAFETY: GetStdHandle has no preconditions; SetHandleInformation only changes a flag
        // and fails harmlessly on a null or invalid handle.
        unsafe {
            let handle = GetStdHandle(which as u32);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Spawns the server described by `template` (pid and started_at are replaced) and waits for readiness.
fn launch(template: &ServerInfo) -> Result<ServerInfo, CliError> {
    let state = state_dir()?;
    fs::create_dir_all(&state).map_err(anyhow::Error::from)?;
    let log = log_path()?;
    let offset = fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    let logfile = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("opening {}", log.display()))?;

    let mut cmd = Command::new(&template.argv[0]);
    cmd.args(&template.argv[1..])
        .envs(template.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(logfile.try_clone().map_err(anyhow::Error::from)?)
        .stderr(logfile);
    // Own process group, so the terminal's Ctrl-C reaches the scan but not the server.
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    keep_std_handles_from_children();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CliError::Server(format!("cannot start {}: {e}", template.argv[0])))?;

    let mut info = template.clone();
    info.pid = child.id();
    info.started_at = now_rfc3339();
    let json = serde_json::to_string_pretty(&info).map_err(anyhow::Error::from)?;
    let path = info_path()?;
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;

    let timeout = ready_timeout();
    match wait_ready(&mut child, &info, timeout) {
        Ok(()) => {
            // Reap the server when it exits: a restart in this process would otherwise find the
            // previous child a zombie, which still looks alive, and wait out the kill timeouts.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(info)
        }
        Err(reason) => {
            force_kill(info.pid);
            let _ = child.wait();
            let _ = fs::remove_file(&path);
            let tail = log_tail(&log, offset);
            let mut msg = format!("model server failed to start: {reason}");
            if weights_missing(&tail) {
                msg.push_str(&format!(
                    "\nweights missing — re-run `ocr server setup --model {}`",
                    info.engine
                ));
            }
            msg.push_str(&format!("\nlast lines of {}:\n{tail}", log.display()));
            Err(CliError::Server(msg))
        }
    }
}

fn wait_ready(child: &mut Child, info: &ServerInfo, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut healthy = false;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("server exited early ({status})"));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "not ready after {}s ({})",
                timeout.as_secs(),
                if healthy {
                    "/health answered but the probe request failed"
                } else {
                    "/health never answered"
                }
            ));
        }
        if !healthy {
            healthy = health_ok(info.port, Duration::from_secs(2).min(remaining));
        }
        if healthy
            && probe_ok(
                info.port,
                &info.repo,
                remaining.min(Duration::from_secs(120)),
            )
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn server_program(venv: &Path) -> String {
    std::env::var("OCR_SERVER_CMD").unwrap_or_else(|_| {
        venv.join("bin/mlx_vlm.server")
            .to_string_lossy()
            .into_owned()
    })
}

type ServerCommand = (Vec<String>, Vec<(String, String)>, Option<usize>);

/// mlx-vlm's server argv and env (macOS).
fn mlx_command(engine: &str, repo: &str, port: u16) -> Result<ServerCommand, CliError> {
    let program = server_program(&venv_dir()?);
    if std::env::var_os("OCR_SERVER_CMD").is_none() && !Path::new(&program).exists() {
        return Err(CliError::Setup(format!(
            "error: model runtime missing. Run `ocr server setup --model {engine}`."
        )));
    }
    let argv = vec![
        program,
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--model".into(),
        repo.into(),
        "--max-num-seqs".into(),
        "1".into(),
    ];
    Ok((argv, vec![("HF_HUB_OFFLINE".into(), "1".into())], None))
}

/// llama-server's argv and env off macOS, from the build and weights setup installed. One slot,
/// as on the Mac. The build's library directories (llama.cpp's own and the CUDA runtime's) go
/// first on `LD_LIBRARY_PATH` (Linux) or `PATH` (Windows).
#[cfg(not(target_os = "macos"))]
fn llama_command(
    engine: Engine,
    version: Option<&str>,
    quant: Quant,
    port: u16,
) -> Result<ServerCommand, CliError> {
    use crate::llama::{self, Variant};
    let n = engine.name();
    let missing = || {
        CliError::Setup(format!(
            "error: model runtime missing. Run `ocr server setup --model {n}`."
        ))
    };
    let variant = Config::load()?
        .and_then(|c| c.runtime)
        .and_then(|r| r.llama_variant)
        .and_then(|v| Variant::parse(&v))
        .ok_or_else(missing)?;
    if !variant.uses_gpu() {
        eprintln!("{}", llama::cpu_warning());
    }
    let gguf = engines::gguf(engine, version, quant).ok_or_else(missing)?;
    let model = llama::model_path(&gguf, gguf.model.name)?;
    let mmproj = llama::model_path(&gguf, gguf.mmproj.name)?;
    let fake = std::env::var("OCR_SERVER_CMD").ok();
    if fake.is_none() && !(model.is_file() && mmproj.is_file()) {
        return Err(CliError::Setup(format!(
            "error: weights missing — re-run `ocr server setup --model {n}`"
        )));
    }
    let program = match fake {
        Some(p) => PathBuf::from(p),
        None => llama::server_binary(variant).ok_or_else(missing)?,
    };
    let slots = 1;
    let mut argv = vec![program.to_string_lossy().into_owned()];
    argv.extend(llama::server_args(
        &model, &mmproj, port, engine, slots, variant,
    ));
    let mut env = Vec::new();
    let var = if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut dirs = llama::library_dirs(variant);
    dirs.extend(program.parent().map(Path::to_path_buf));
    dirs.dedup();
    dirs.extend(std::env::var_os(var).iter().flat_map(std::env::split_paths));
    if let Ok(joined) = std::env::join_paths(dirs) {
        env.push((var.into(), joined.to_string_lossy().into_owned()));
    }
    Ok((argv, env, Some(slots)))
}

#[cfg(target_os = "macos")]
fn llama_command(_: Engine, _: Option<&str>, _: Quant, _: u16) -> Result<ServerCommand, CliError> {
    unreachable!("macOS runs mlx-vlm")
}

/// Resolves the version/quant/repo to serve, defaulting to what setup recorded.
pub fn resolve_model(
    engine: Engine,
    version: Option<&str>,
    quant: Option<Quant>,
) -> Result<(Option<String>, Quant, &'static str), CliError> {
    let entry = Config::load()?.and_then(|c| c.engines.get(engine.name()).cloned());
    let recorded_version = entry.as_ref().and_then(|e| e.version.clone());
    let version = engine
        .resolve_version(version.or(recorded_version.as_deref()))?
        .map(str::to_string);
    let quant = match quant {
        Some(q) => q,
        None => entry
            .as_ref()
            .and_then(|e| e.quant.as_deref())
            .and_then(|q| {
                [Quant::Q4, Quant::Q8, Quant::Bf16]
                    .into_iter()
                    .find(|x| x.name() == q)
            })
            .unwrap_or(engines::default_quant(engine)),
    };
    let repo = engines::repo(engine, version.as_deref(), quant).ok_or_else(|| {
        CliError::Usage(format!(
            "engine '{}' has no {} build; available quants: {}",
            engine.name(),
            quant.name(),
            engines::quants(engine, version.as_deref()).join(", ")
        ))
    })?;
    Ok((version, quant, repo))
}

pub fn start(args: &StartArgs, engine: Engine) -> Result<(), CliError> {
    let (version, quant, repo) = resolve_model(engine, args.model_version.as_deref(), args.quant)?;
    let n = engine.name();
    let recorded = Config::load()?.and_then(|c| c.engines.get(n).and_then(|e| e.repo.clone()));
    if recorded.is_some_and(|r| r != repo) {
        return Err(CliError::Setup(format!(
            "⚠ {repo} is not installed. Run `ocr server setup --model {n}` with the same --model-version/--quant."
        )));
    }

    if let Some(cur) = running() {
        if cur.engine == n && cur.repo == repo {
            println!(
                "{n} server already running on port {} (pid {})",
                cur.port, cur.pid
            );
            return Ok(());
        }
        if !args.force {
            return Err(CliError::Setup(format!(
                "error: a {} server ({}) is already running on port {}; use --force to replace it",
                cur.engine, cur.repo, cur.port
            )));
        }
        println!("stopping {} server (pid {})", cur.engine, cur.pid);
        stop_server(&cur);
    }

    if TcpListener::bind(("127.0.0.1", args.port)).is_err() {
        return Err(CliError::Setup(format!(
            "error: port {} is already in use; choose another with --port",
            args.port
        )));
    }

    let (argv, env, slots) = if cfg!(target_os = "macos") {
        mlx_command(n, repo, args.port)?
    } else {
        llama_command(engine, version.as_deref(), quant, args.port)?
    };
    let template = ServerInfo {
        pid: 0,
        engine: n.into(),
        version,
        quant: quant.name().into(),
        repo: repo.into(),
        port: args.port,
        argv,
        env,
        slots,
        started_at: String::new(),
    };
    println!("starting {n} ({repo}) on port {}", args.port);
    let started = Instant::now();
    let info = launch(&template)?;
    println!(
        "{n} server ready on port {} (pid {}) in {:.1}s",
        info.port,
        info.pid,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Stops and restarts the server with its recorded argv and env, waiting for readiness.
pub fn restart(info: &ServerInfo) -> Result<ServerInfo, CliError> {
    // A dead pid may have been reused by an unrelated process: only stop what is ours.
    if is_ours(info) {
        stop_server(info);
    }
    launch(info)
}

/// Whether `server.json` still exists. `stop` removes it, a crash leaves it behind.
pub fn info_file_exists() -> bool {
    info_path().is_ok_and(|p| p.exists())
}

// ------------------------------------------------------- stop / status

pub fn stop() -> Result<(), CliError> {
    match running() {
        None => println!("no server running"),
        Some(info) => {
            stop_server(&info);
            println!(
                "stopped {} server ({}, port {}, pid {})",
                info.engine, info.repo, info.port, info.pid
            );
        }
    }
    Ok(())
}

/// Resident memory of the server off macOS (Linux `/proc`, Windows via `sysinfo`). GPU memory
/// is not included.
#[cfg(not(target_os = "macos"))]
fn footprint(pid: u32) -> Option<String> {
    #[cfg(windows)]
    let kib = process(pid, |p| p.memory() / 1024)?;
    #[cfg(not(windows))]
    let kib: u64 = fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(format!(
        "{:.1} GB resident (CPU memory)",
        kib as f64 / (1 << 20) as f64
    ))
}

#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> Option<String> {
    let out = Command::new("vmmap")
        .args(["--summary", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("Physical footprint:"))
        .map(|v| v.trim().to_string())
}

/// `used / total MiB` of the NVIDIA GPU(s), if `nvidia-smi` answers.
fn nvidia_memory_used() -> Option<String> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim().to_string();
    let (used, total) = line.split_once(',')?;
    (out.status.success()).then(|| format!("{} / {} MiB used", used.trim(), total.trim()))
}

fn format_uptime(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn status() -> Result<(), CliError> {
    let Some(info) = running() else {
        if cfg!(target_os = "macos") {
            println!("no server running (the apple engine needs none)");
        } else {
            println!("no server running");
        }
        print_olmocr_notice();
        return Ok(());
    };
    let uptime = chrono::DateTime::parse_from_rfc3339(&info.started_at)
        .map(|t| format_uptime((chrono::Utc::now() - t.to_utc()).num_seconds().max(0)))
        .unwrap_or_else(|_| "unknown".into());
    println!("engine:  {}", info.engine);
    if let Some(v) = &info.version {
        println!("version: {v}");
    }
    println!("quant:   {}", info.quant);
    println!("repo:    {}", info.repo);
    println!("port:    {}", info.port);
    println!("pid:     {}", info.pid);
    println!("uptime:  {uptime}");
    let health = if health_ok(info.port, Duration::from_secs(2)) {
        "reachable"
    } else {
        "not reachable"
    };
    println!("health:  {health}");
    // A broken config must not make `status` fail; the build lines are just skipped.
    if let Some(rt) = Config::load().ok().flatten().and_then(|c| c.runtime)
        && rt.llama_cpp.is_some()
    {
        println!("build:   {}", rt.id());
        if let Some(gpu) = rt.gpu {
            println!("gpu:     {gpu}");
        }
        if let Some(used) = nvidia_memory_used() {
            println!("vram:    {used}");
        }
    }
    if let Some(fp) = footprint(info.pid) {
        if cfg!(target_os = "macos") {
            println!("memory:  {fp} physical footprint");
        } else {
            println!("memory:  {fp}");
        }
    }
    print_olmocr_notice();
    Ok(())
}

/// Read-only: mentions an olmocr entry that is still in the config.
fn print_olmocr_notice() {
    if let Some(n) = Config::load()
        .ok()
        .flatten()
        .as_ref()
        .and_then(crate::removed::olmocr_notice)
    {
        println!("{n}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_base64() {
        let png = white_png();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn uptime() {
        assert_eq!(format_uptime(5), "5s");
        assert_eq!(format_uptime(125), "2m05s");
        assert_eq!(format_uptime(3725), "1h02m05s");
    }
}
