//! Crash containment for `ocr scan`.
//!
//! Apple's private TextRecognition framework can segfault (`objc_retain`) when several requests
//! start at once (docs/spike.md, "Apple Vision concurrent start-up crash"). We cannot prevent it,
//! so a scan runs as a child process of itself; if the child dies from a crash signal it is
//! re-run and resumes from the per-page checkpoints, which makes the retry lossless.

/// Set on the re-executed child so it does not supervise itself.
pub const CHILD_ENV: &str = "OCR_SCAN_CHILD";
/// Attempt number (0 for the first run), set by the supervisor.
pub const ATTEMPT_ENV: &str = "OCR_SCAN_ATTEMPT";
/// Retries after the first run.
pub const MAX_RETRIES: u32 = 3;

/// True in the re-executed child.
pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// Attempt number of this run: 0 for the first (and for an unsupervised run).
pub fn attempt() -> u32 {
    std::env::var(ATTEMPT_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
pub use crash::*;

#[cfg(target_os = "macos")]
mod crash {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, ExitCode};
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::{ATTEMPT_ENV, CHILD_ENV, MAX_RETRIES, attempt};

    static CHILD_PID: AtomicI32 = AtomicI32::new(0);

    /// Signals that mean the process itself crashed. SIGINT, SIGTERM and SIGKILL are deliberate stops.
    pub fn is_crash_signal(sig: i32) -> bool {
        matches!(
            sig,
            libc::SIGSEGV | libc::SIGBUS | libc::SIGILL | libc::SIGTRAP
        )
    }

    extern "C" fn ignore(_: libc::c_int) {}

    extern "C" fn forward(sig: libc::c_int) {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill is async-signal-safe.
            unsafe { libc::kill(pid, sig) };
        }
    }

    /// In the child: exits if the supervisor dies (for example SIGKILL), so no scan is left running
    /// unattended. macOS has no parent-death signal, so this polls.
    pub fn exit_with_parent() {
        // SAFETY: getppid has no preconditions.
        let parent = unsafe { libc::getppid() };
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(50));
                // SAFETY: as above.
                if unsafe { libc::getppid() } != parent {
                    // SAFETY: _exit is async-signal-safe and skips destructors on purpose.
                    unsafe { libc::_exit(137) };
                }
            }
        });
    }

    /// Runs the scan in a child process, re-running it after a crash. Returns the exit status of
    /// the final child.
    pub fn supervise() -> ExitCode {
        // A handler (unlike SIG_IGN) is reset on exec, so the child still dies on Ctrl-C. The
        // terminal sends SIGINT to the whole foreground group, so the child gets its own copy.
        // SIGTERM and SIGHUP go to this process only, so they are forwarded.
        // SAFETY: the handlers only call async-signal-safe functions.
        unsafe {
            libc::signal(
                libc::SIGINT,
                ignore as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                forward as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGHUP,
                forward as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: cannot re-run ocr: {e}");
                return ExitCode::from(2);
            }
        };
        let mut attempt = 0;
        loop {
            let spawned = Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .env(CHILD_ENV, "1")
                .env(ATTEMPT_ENV, attempt.to_string())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: cannot re-run ocr: {e}");
                    return ExitCode::from(2);
                }
            };
            CHILD_PID.store(child.id() as i32, Ordering::SeqCst);
            let status = child.wait();
            CHILD_PID.store(0, Ordering::SeqCst);
            let status = match status {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: waiting for ocr failed: {e}");
                    return ExitCode::from(2);
                }
            };
            let Some(sig) = status.signal() else {
                return ExitCode::from(status.code().unwrap_or(1) as u8);
            };
            if !is_crash_signal(sig) {
                return ExitCode::from(128 + sig as u8);
            }
            if attempt >= MAX_RETRIES {
                eprintln!(
                    "error: Apple Vision crashed (signal {sig}) and {MAX_RETRIES} retries did not finish the scan; \
                 completed pages are checkpointed, so re-running resumes where it stopped"
                );
                return ExitCode::from(1);
            }
            attempt += 1;
            eprintln!(
                "note: Apple Vision crashed (signal {sig}); resuming from checkpoints (retry {attempt} of {MAX_RETRIES})"
            );
        }
    }

    /// Test hook: raises a signal after `OCR_TEST_CRASH_AFTER` page checkpoints, on the first
    /// `OCR_TEST_CRASH_ATTEMPTS` attempts (default 1). `OCR_TEST_CRASH_SIGNAL` picks the signal
    /// (default SIGSEGV). `written` is the number of checkpoints written so far in this process.
    pub fn test_crash_hook(written: usize) {
        let var = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<i64>().ok());
        let Some(after) = var("OCR_TEST_CRASH_AFTER") else {
            return;
        };
        if written as i64 != after
            || i64::from(attempt()) >= var("OCR_TEST_CRASH_ATTEMPTS").unwrap_or(1)
        {
            return;
        }
        let sig = var("OCR_TEST_CRASH_SIGNAL").map_or(libc::SIGSEGV, |s| s as i32);
        // SAFETY: raising a signal on ourselves; the default action first, because Rust's runtime
        // handles SIGSEGV and would otherwise swallow a raised one.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
}

/// Off macOS there is no Apple Vision to crash, so scans are never supervised.
#[cfg(not(target_os = "macos"))]
pub fn test_crash_hook(_written: usize) {}

#[cfg(target_os = "macos")]
#[cfg(test)]
mod tests {
    use super::crash::*;

    #[test]
    fn only_crash_signals_are_retried() {
        for s in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGTRAP] {
            assert!(is_crash_signal(s));
        }
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL, libc::SIGABRT] {
            assert!(!is_crash_signal(s));
        }
    }
}
