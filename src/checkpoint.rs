//! Per-page checkpoints (PLAN §5.3): `<out>.md.ocr-partial/` holds `manifest.json` and one
//! `page-00042.json` per finished page. A page record is the page's full pre-assembly result;
//! assembly-time options (markers, front matter, `--pages`) are not part of the manifest.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::markdown::PageResult;

/// Bump when the layout of the partial dir or a page record changes.
pub const CHECKPOINT_FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
/// Bytes hashed from the start and from the end of the input.
const HASHED: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime_nanos: u64,
    pub head_sha256: String,
    pub tail_sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Size, mtime and SHA-256 of the first and last 1 MB of `path`.
pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let mut f = File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let meta = f.metadata()?;
    let size = meta.len();
    let mtime_nanos = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = size.min(HASHED) as usize;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    let head = hex(&Sha256::digest(&buf));
    f.seek(SeekFrom::Start(size - n as u64))?;
    f.read_exact(&mut buf)?;
    let tail = hex(&Sha256::digest(&buf));
    Ok(Fingerprint {
        size,
        mtime_nanos,
        head_sha256: head,
        tail_sha256: tail,
    })
}

/// What a checkpoint is only valid for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub crate_version: String,
    pub fingerprint: Fingerprint,
    pub engine: String,
    pub repo: Option<String>,
    pub mlx_vlm: Option<String>,
    pub dpi: u32,
    pub text_layer: String,
    pub currency_fix: bool,
    /// Pages that failed outright when the file was last assembled. They have no page record, so
    /// a re-run retries them. Not compared when deciding whether a checkpoint is valid.
    #[serde(default)]
    pub failed_pages: Vec<usize>,
}

impl Manifest {
    /// What differs from `now`, or `None` if the checkpoints are still valid.
    pub fn changed(&self, now: &Manifest) -> Option<String> {
        let mut what = Vec::new();
        let mut check = |name: &str, same: bool| {
            if !same {
                what.push(name.to_string());
            }
        };
        check("checkpoint format", self.format == now.format);
        check("ocr version", self.crate_version == now.crate_version);
        check("input file", self.fingerprint == now.fingerprint);
        check("engine", self.engine == now.engine);
        check("model repo", self.repo == now.repo);
        check("mlx-vlm version", self.mlx_vlm == now.mlx_vlm);
        check(
            &format!("--dpi ({} -> {})", self.dpi, now.dpi),
            self.dpi == now.dpi,
        );
        check(
            &format!(
                "--use-text-layer ({} -> {})",
                self.text_layer, now.text_layer
            ),
            self.text_layer == now.text_layer,
        );
        check("currency fix", self.currency_fix == now.currency_fix);
        (!what.is_empty()).then(|| what.join(", "))
    }
}

/// 64-bit FNV-1a. Stable across runs, platforms and Rust versions (unlike `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The `scan_record` front-matter value: 16 hex digits over everything that decides what a scan
/// produces and that `--overwrite`-less directory runs can check: the manifest without
/// `failed_pages`, the selected pages and the `--verify-json` mode.
pub fn scan_record(
    m: &Manifest,
    selected: &[usize],
    verify: Option<crate::verify::VerifyFields>,
) -> String {
    use crate::verify::VerifyFields;
    let f = &m.fingerprint;
    let pages: Vec<String> = selected.iter().map(usize::to_string).collect();
    let verify = match verify {
        None => "none",
        Some(VerifyFields::Flagged) => "flagged",
        Some(VerifyFields::All) => "all",
    };
    let text = format!(
        "format={}\nversion={}\nsize={}\nmtime={}\nhead={}\ntail={}\nengine={}\nrepo={:?}\nmlx={:?}\ndpi={}\ntext_layer={}\ncurrency_fix={}\npages={}\nverify={verify}",
        m.format,
        m.crate_version,
        f.size,
        f.mtime_nanos,
        f.head_sha256,
        f.tail_sha256,
        m.engine,
        m.repo,
        m.mlx_vlm,
        m.dpi,
        m.text_layer,
        m.currency_fix,
        pages.join(","),
    );
    format!("{:016x}", fnv1a(text.as_bytes()))
}

/// The `scan_record` in the front matter of an existing output; `None` for outputs without one
/// (made by an older ocr, or with `--no-front-matter`).
pub fn read_scan_record(output: &Path) -> Option<String> {
    let mut head = Vec::new();
    File::open(output)
        .ok()?
        .take(8192)
        .read_to_end(&mut head)
        .ok()?;
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines.take_while(|l| *l != "---") {
        if let Some(v) = line.strip_prefix("scan_record: ") {
            let v = v.trim();
            let ok = v.len() == 16 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            return ok.then(|| v.to_string());
        }
    }
    None
}

/// `<output>.ocr-partial`, next to the output file.
pub fn partial_dir(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_owned();
    s.push(".ocr-partial");
    PathBuf::from(s)
}

fn page_path(dir: &Path, number: usize) -> PathBuf {
    dir.join(format!("page-{number:05}.json"))
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&fs::read(dir.join(MANIFEST)).ok()?).ok()
}

/// Whether a finished output has a partial dir that remembers failed pages, so a re-run
/// should retry them rather than treat the file as done.
pub fn retryable(output: &Path) -> bool {
    read_manifest(&partial_dir(output)).is_some_and(|m| !m.failed_pages.is_empty())
}

/// Writes `path` via `<path>.tmp` and a rename. `.tmp` never matches the `page-*.json` names.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).with_context(|| format!("cannot write {}", tmp.display()))?;
    replace_file(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}

/// `fs::rename` onto a possibly existing file. On Windows an antivirus scanner or the search
/// indexer can hold the target open for a moment, so a refused rename is retried for about a
/// quarter of a second.
pub fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && tries < 5 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

pub fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    write_file(&dir.join(MANIFEST), &serde_json::to_vec_pretty(manifest)?)
}

/// Whether a result is worth keeping: a page that failed outright is retried on the next run.
pub fn checkpointable(page: &PageResult) -> bool {
    page.error.is_none()
}

pub fn write_page(dir: &Path, page: &PageResult) -> Result<()> {
    write_file(&page_path(dir, page.number), &serde_json::to_vec(page)?)
}

/// The record of page `number`, if it exists and parses.
pub fn read_page(dir: &Path, number: usize) -> Option<PageResult> {
    let page: PageResult = serde_json::from_slice(&fs::read(page_path(dir, number)).ok()?).ok()?;
    (page.number == number).then_some(page)
}

/// What a partial dir held when a file was opened.
pub struct Loaded {
    pub pages: BTreeMap<usize, PageResult>,
    /// Lines to warn about (discarded checkpoints, unreadable page files).
    pub warnings: Vec<String>,
}

/// Opens the checkpoints of `output`: the valid page records of a matching partial dir. A dir
/// that does not match `manifest`, and every dir with `overwrite`, is removed; so is any page
/// file that does not parse (that page is redone).
pub fn load(output: &Path, manifest: &Manifest, overwrite: bool) -> Loaded {
    let dir = partial_dir(output);
    let mut loaded = Loaded {
        pages: BTreeMap::new(),
        warnings: Vec::new(),
    };
    if !dir.exists() {
        return loaded;
    }
    if overwrite {
        let _ = fs::remove_dir_all(&dir);
        return loaded;
    }
    let why = match read_manifest(&dir) {
        None => Some("unreadable manifest".to_string()),
        Some(old) => old.changed(manifest),
    };
    if let Some(why) = why {
        loaded.warnings.push(format!(
            "warning: discarding checkpoints for {}: {why} changed",
            output.display()
        ));
        let _ = fs::remove_dir_all(&dir);
        return loaded;
    }
    let Ok(entries) = fs::read_dir(&dir) else {
        return loaded;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tmp") {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        let Some(number) = name
            .strip_prefix("page-")
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        match read_page(&dir, number) {
            Some(page) => {
                loaded.pages.insert(number, page);
            }
            None => {
                loaded.warnings.push(format!(
                    "warning: checkpoint {name} of {} is unreadable; redoing that page",
                    output.display()
                ));
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Source;

    fn manifest(dpi: u32) -> Manifest {
        Manifest {
            format: CHECKPOINT_FORMAT,
            crate_version: "1".into(),
            fingerprint: Fingerprint {
                size: 1,
                mtime_nanos: 2,
                head_sha256: "a".into(),
                tail_sha256: "b".into(),
            },
            engine: "apple".into(),
            repo: None,
            mlx_vlm: None,
            dpi,
            text_layer: "never".into(),
            currency_fix: true,
            failed_pages: vec![],
        }
    }

    #[test]
    fn scan_record_is_stable_and_follows_every_input() {
        use crate::verify::VerifyFields;
        let base = scan_record(&manifest(200), &[1, 2], None);
        assert_eq!(base.len(), 16);
        assert!(base.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(base, scan_record(&manifest(200), &[1, 2], None));
        // Pinned: the value must not change between releases.
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        // failed_pages is not part of it.
        let mut failed = manifest(200);
        failed.failed_pages = vec![2];
        assert_eq!(base, scan_record(&failed, &[1, 2], None));
        let differs = |r: String| assert_ne!(base, r);
        differs(scan_record(&manifest(150), &[1, 2], None));
        differs(scan_record(&manifest(200), &[1], None));
        differs(scan_record(
            &manifest(200),
            &[1, 2],
            Some(VerifyFields::Flagged),
        ));
        assert_ne!(
            scan_record(&manifest(200), &[1, 2], Some(VerifyFields::All)),
            scan_record(&manifest(200), &[1, 2], Some(VerifyFields::Flagged))
        );
        let mut m = manifest(200);
        m.fingerprint.mtime_nanos += 1;
        differs(scan_record(&m, &[1, 2], None));
        let mut m = manifest(200);
        m.engine = "dots-ocr".into();
        differs(scan_record(&m, &[1, 2], None));
    }

    #[test]
    fn scan_record_is_read_from_the_front_matter_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.md");
        let write = |t: &str| fs::write(&p, t).unwrap();
        write("---\nsource: a\nscan_record: 0123456789abcdef\n---\n\nbody\n");
        assert_eq!(read_scan_record(&p).as_deref(), Some("0123456789abcdef"));
        write("---\nsource: a\n---\n\nscan_record: 0123456789abcdef\n");
        assert_eq!(read_scan_record(&p), None);
        write("---\nscan_record: XYZ\n---\n");
        assert_eq!(read_scan_record(&p), None);
        write("done");
        assert_eq!(read_scan_record(&p), None);
        assert_eq!(read_scan_record(&d.path().join("missing.md")), None);
    }

    fn page(number: usize) -> PageResult {
        PageResult {
            number,
            source: Source::Vision,
            body: "text \"quoted\" é\n\nmore".into(),
            bates: Some("ABC000123".into()),
            column_split: true,
            transcript: false,
            currency_fixes: 2,
            table: true,
            fallback: false,
            flagged: true,
            line_numbers: Some(true),
            symbol_fixes: 0,
            embedded_fixes: None,
            suspect: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn page_records_round_trip_and_bad_files_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("a.md");
        let dir = partial_dir(&out);
        let m = manifest(200);
        write_manifest(&dir, &m).unwrap();
        write_page(&dir, &page(1)).unwrap();
        write_page(&dir, &page(2)).unwrap();
        fs::write(page_path(&dir, 3), b"{\"number\":3,\"sour").unwrap();
        fs::write(dir.join("page-00004.json.tmp"), b"x").unwrap();
        let loaded = load(&out, &m, false);
        assert_eq!(loaded.pages.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(loaded.pages[&1].body, page(1).body);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(!page_path(&dir, 3).exists() && !dir.join("page-00004.json.tmp").exists());
    }

    #[test]
    fn a_changed_setting_discards_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("a.md");
        let dir = partial_dir(&out);
        write_manifest(&dir, &manifest(200)).unwrap();
        write_page(&dir, &page(1)).unwrap();
        let loaded = load(&out, &manifest(300), false);
        assert!(loaded.pages.is_empty() && !dir.exists());
        assert!(loaded.warnings[0].contains("--dpi (200 -> 300)"));
    }

    #[test]
    fn failed_pages_do_not_invalidate_and_mark_the_output_retryable() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("a.md");
        let dir = partial_dir(&out);
        assert!(!retryable(&out));
        let mut m = manifest(200);
        m.failed_pages = vec![2];
        write_manifest(&dir, &m).unwrap();
        assert!(retryable(&out));
        assert!(m.changed(&manifest(200)).is_none());
    }

    #[test]
    fn errors_are_not_checkpointed() {
        let mut p = page(1);
        assert!(checkpointable(&p));
        p.fallback = true;
        assert!(checkpointable(&p));
        p.error = Some("boom".into());
        assert!(!checkpointable(&p));
    }

    #[test]
    fn fingerprint_changes_with_content_and_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        fs::write(&p, vec![1u8; 3 << 20]).unwrap();
        let a = fingerprint(&p).unwrap();
        assert_eq!(a, fingerprint(&p).unwrap());
        let mut data = vec![1u8; 3 << 20];
        *data.last_mut().unwrap() = 9;
        fs::write(&p, data).unwrap();
        let b = fingerprint(&p).unwrap();
        assert_ne!(a.tail_sha256, b.tail_sha256);
        assert_eq!(a.head_sha256, b.head_sha256);
    }
}
