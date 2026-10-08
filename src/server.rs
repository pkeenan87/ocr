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

const UV_INSTALL: &str = "curl -LsSf https://astral.sh/uv/install.sh | sh";

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

// ---------------------------------------------------------------- setup

fn find_uv() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("uv"))
            .find(|c| c.is_file())
    });
    on_path.or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".local/bin/uv"))
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

/// Creates the venv and installs the pinned mlx-vlm; returns the recorded runtime versions.
fn ensure_venv(uv: &Path, venv: &Path) -> Result<Runtime, CliError> {
    let python = venv.join("bin/python");
    if !python.exists() {
        println!("creating Python 3.12 environment in {}", venv.display());
        run_checked(
            Command::new(uv)
                .args(["venv", "--python", "3.12"])
                .arg(venv),
            "uv venv",
        )?;
    }
    let mlx_version = |p: &Path| {
        python_out(
            p,
            "import importlib.metadata as m; print(m.version('mlx-vlm'))",
        )
    };
    if mlx_version(&python).as_deref() == Some(MLX_VLM_PIN) {
        println!("mlx-vlm {MLX_VLM_PIN} already installed");
    } else {
        println!("installing mlx-vlm=={MLX_VLM_PIN}");
        run_checked(
            Command::new(uv)
                .args(["pip", "install", "--python"])
                .arg(&python)
                .arg(format!("mlx-vlm=={MLX_VLM_PIN}")),
            "uv pip install",
        )?;
    }
    let py = python_out(&python, "import platform; print(platform.python_version())")
        .ok_or_else(|| CliError::Setup("error: cannot run the venv's python".into()))?;
    let mlx = mlx_version(&python)
        .ok_or_else(|| CliError::Setup("error: mlx-vlm is not importable in the venv".into()))?;
    Ok(Runtime {
        python: py,
        mlx_vlm: mlx,
    })
}

/// Installs the requested model engines (runtime, weights) and records them in the config.
pub fn setup_models(args: &SetupArgs, cfg: &mut Config) -> Result<(), CliError> {
    let models = args.requested_models();
    if models.is_empty() {
        return Ok(());
    }
    let fake = std::env::var_os("OCR_FAKE_INSTALL").is_some_and(|v| v == "1");
    if cfg!(not(target_os = "macos")) && !fake {
        return Err(CliError::Setup(
            "error: model servers are not supported on this platform yet (PLAN §9, Phase 7d)"
                .into(),
        ));
    }

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
        if !fake {
            check_ram(engine, quant, args.force)?;
        }
        plan.push((engine, version, quant, repo));
    }

    let runtime = if fake {
        Runtime {
            python: "fake".into(),
            mlx_vlm: MLX_VLM_PIN.into(),
        }
    } else {
        let uv = find_uv().ok_or_else(|| {
            CliError::Setup(format!(
                "error: `uv` is required but was not found. Install it with:\n  {UV_INSTALL}"
            ))
        })?;
        std::fs::create_dir_all(state_dir()?).map_err(anyhow::Error::from)?;
        ensure_venv(&uv, &venv_dir()?)?
    };
    cfg.runtime = Some(runtime);

    for (engine, version, quant, repo) in plan {
        if !fake {
            println!("downloading {repo}");
            run_checked(
                Command::new(venv_dir()?.join("bin/hf")).args(["download", repo]),
                "hf download",
            )?;
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
    Ok(())
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

/// The process `pid`, refreshed with its command line.
#[cfg(windows)]
fn process<T>(pid: u32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    sys.process(pid).map(f)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    process(pid, |_| ()).is_some()
}

/// Windows has no SIGTERM for a console-less child; llama-server is a single process, so stop
/// it with TerminateProcess.
#[cfg(windows)]
fn terminate(pid: u32) {
    force_kill(pid);
}

#[cfg(windows)]
fn force_kill(pid: u32) {
    process(pid, |p| p.kill());
}

#[cfg(windows)]
fn process_command(pid: u32) -> Option<String> {
    let cmd = process(pid, |p| {
        p.cmd()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
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
    process_command(info.pid)
        .is_some_and(|c| c.contains(prog) && c.contains(&format!("--port {}", info.port)))
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

    let venv = venv_dir()?;
    let program = server_program(&venv);
    if std::env::var_os("OCR_SERVER_CMD").is_none() && !Path::new(&program).exists() {
        return Err(CliError::Setup(format!(
            "error: model runtime missing. Run `ocr server setup --model {n}`."
        )));
    }
    let template = ServerInfo {
        pid: 0,
        engine: n.into(),
        version,
        quant: quant.name().into(),
        repo: repo.into(),
        port: args.port,
        argv: vec![
            program,
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            args.port.to_string(),
            "--model".into(),
            repo.into(),
            "--max-num-seqs".into(),
            "1".into(),
        ],
        env: vec![("HF_HUB_OFFLINE".into(), "1".into())],
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
        println!("no server running");
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
    if let Some(fp) = footprint(info.pid) {
        println!("memory:  {fp} physical footprint");
    }
    Ok(())
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
