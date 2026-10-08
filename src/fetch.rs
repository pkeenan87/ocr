//! Pinned downloads off macOS (PLAN §9, Phase 7d-1): `curl` fetches with resume, the sha256 is
//! checked before a file is used, and archives are unpacked with `tar` (which reads zip too on
//! Windows). Both tools ship with current Linux distributions and Windows 10 and later.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::CliError;

fn setup_err(msg: impl Into<String>) -> CliError {
    CliError::Setup(msg.into())
}

/// `curl` or `tar`. On Windows the copies in System32 (Windows 10 1803 and later) are preferred:
/// a GNU `tar` from Git or MSYS earlier on `PATH` cannot unpack the llama.cpp zips.
fn tool(name: &str) -> Command {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let exe = Path::new(&root)
            .join("System32")
            .join(format!("{name}.exe"));
        if exe.is_file() {
            return Command::new(exe);
        }
    }
    Command::new(name)
}

/// What to install when `curl` or `tar` is missing.
fn tool_hint() -> &'static str {
    if cfg!(windows) {
        "they ship with Windows 10 version 1803 and later"
    } else {
        "install it with your package manager"
    }
}

/// Lower-case hex sha256 of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `<file>.sha256`, written once a download is verified so later runs skip re-hashing.
fn stamp_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".sha256");
    dest.with_file_name(name)
}

/// Whether `dest` is already there and verified against `sha256`.
pub fn is_verified(dest: &Path, sha256: &str) -> bool {
    dest.is_file() && fs::read_to_string(stamp_path(dest)).is_ok_and(|s| s.trim() == sha256)
}

/// Downloads `url` to `dest` unless it is already there and verified. A partial download
/// (`<dest>.part`) is resumed. The file is only moved into place once its sha256 matches.
pub fn download(url: &str, dest: &Path, sha256: &str, label: &str) -> Result<(), CliError> {
    if is_verified(dest, sha256) {
        println!("{label}: already downloaded");
        return Ok(());
    }
    let dir = dest
        .parent()
        .ok_or_else(|| setup_err(format!("error: bad download path {}", dest.display())))?;
    fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    let mut part = dest.as_os_str().to_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    println!("downloading {label}");
    let status = tool("curl")
        .args(["-fL", "--retry", "3", "--progress-bar", "-C", "-", "-o"])
        .arg(&part)
        .arg(url)
        .status()
        .map_err(|e| {
            setup_err(format!(
                "error: cannot run curl ({e}); curl is needed to download models ({})",
                tool_hint()
            ))
        })?;
    if !status.success() {
        return Err(setup_err(format!(
            "error: download of {label} failed ({status}); re-run setup to resume\n  {url}"
        )));
    }
    let got = sha256_file(&part).map_err(anyhow::Error::from)?;
    if got != sha256 {
        let _ = fs::remove_file(&part);
        return Err(setup_err(format!(
            "error: {label} failed its checksum (expected {sha256}, got {got}); deleted, re-run setup"
        )));
    }
    crate::checkpoint::replace_file(&part, dest).map_err(anyhow::Error::from)?;
    fs::write(stamp_path(dest), sha256).map_err(anyhow::Error::from)?;
    Ok(())
}

/// Unpacks a .tar.gz, .tgz or .zip archive into `dir` (created if needed).
pub fn extract(archive: &Path, dir: &Path) -> Result<(), CliError> {
    fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    let status = tool("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| setup_err(format!("error: cannot run tar ({e}; {})", tool_hint())))?;
    if status.success() {
        Ok(())
    } else {
        Err(setup_err(format!(
            "error: cannot unpack {} ({status})",
            archive.display()
        )))
    }
}

/// The first file named `name` under `dir`, searched breadth-first.
pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut queue = vec![dir.to_path_buf()];
    while !queue.is_empty() {
        let mut next = Vec::new();
        for d in queue {
            let Ok(entries) = fs::read_dir(&d) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    next.push(p);
                } else if p.file_name().is_some_and(|n| n == name) {
                    return Some(p);
                }
            }
        }
        queue = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"abc").unwrap();
        let sha = sha256_file(&file).unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(!is_verified(&file, &sha));
        fs::write(stamp_path(&file), &sha).unwrap();
        assert!(is_verified(&file, &sha));
        assert!(!is_verified(&file, "other"));
    }

    #[test]
    fn finds_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("x/build/bin")).unwrap();
        fs::write(dir.path().join("x/build/bin/llama-server"), b"").unwrap();
        assert_eq!(
            find_file(dir.path(), "llama-server").unwrap(),
            dir.path().join("x/build/bin/llama-server")
        );
        assert!(find_file(dir.path(), "nope").is_none());
    }
}
