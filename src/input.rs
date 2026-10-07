//! Input discovery, magic-byte file typing, output-path mapping and `--pages` parsing.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FileType {
    Pdf,
    Image,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Image,
}

impl FileType {
    pub fn matches(self, kind: Kind) -> bool {
        matches!(
            (self, kind),
            (Self::Auto, _) | (Self::Pdf, Kind::Pdf) | (Self::Image, Kind::Image)
        )
    }
}

/// Classifies a file header by magic bytes.
pub fn kind_from_magic(h: &[u8]) -> Option<Kind> {
    let is_heic =
        h.len() >= 12 && &h[4..8] == b"ftyp" && matches!(&h[8..12], b"heic" | b"heix" | b"mif1");
    if h.starts_with(b"%PDF") {
        Some(Kind::Pdf)
    } else if h.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || h.starts_with(&[0xFF, 0xD8, 0xFF])
        || h.starts_with(b"II*\0")
        || h.starts_with(b"MM\0*")
        || is_heic
        || h.starts_with(b"BM")
        || h.starts_with(b"GIF87a")
        || h.starts_with(b"GIF89a")
        || (h.len() >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP")
    {
        Some(Kind::Image)
    } else {
        None
    }
}

/// Reads the file header and classifies it.
pub fn detect(path: &Path) -> std::io::Result<Option<Kind>> {
    let mut buf = Vec::with_capacity(16);
    File::open(path)?.take(16).read_to_end(&mut buf)?;
    Ok(kind_from_magic(&buf))
}

/// One planned conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub input: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<Item>,
    /// Files skipped because their output already exists.
    pub skipped_existing: usize,
    /// Notes for stderr.
    pub notes: Vec<String>,
}

fn has_md_ext(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Plans single-file mode.
pub fn plan_single(
    input: &Path,
    output: Option<&Path>,
    filter: FileType,
    overwrite: bool,
) -> Result<Plan, CliError> {
    if !input.is_file() {
        return Err(CliError::Usage(format!(
            "input file not found: {}",
            input.display()
        )));
    }
    let kind = detect(input)
        .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", input.display())))?;
    match kind {
        None => {
            return Err(CliError::Usage(format!(
                "unsupported file type: {}",
                input.display()
            )));
        }
        Some(k) if !filter.matches(k) => {
            return Err(CliError::Usage(format!(
                "{} is not a {} file (--file-type)",
                input.display(),
                if k == Kind::Pdf { "image" } else { "pdf" }
            )));
        }
        _ => {}
    }
    let mut plan = Plan::default();
    let output = match output {
        Some(o) if has_md_ext(o) => o.to_path_buf(),
        Some(o) => {
            let mut s = o.as_os_str().to_owned();
            s.push(".md");
            plan.notes.push(format!(
                "note: output is always Markdown; writing to {}",
                Path::new(&s).display()
            ));
            PathBuf::from(s)
        }
        None => PathBuf::from(format!(
            "{}.md",
            input.file_stem().unwrap_or_default().to_string_lossy()
        )),
    };
    if output.exists() && !overwrite {
        return Err(CliError::Usage(format!(
            "output exists: {} (use --overwrite)",
            output.display()
        )));
    }
    plan.items.push(Item {
        input: input.to_path_buf(),
        output,
    });
    Ok(plan)
}

/// Canonicalizes as much of `path` as exists.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    match (path.parent(), path.file_name()) {
        (Some(p), Some(n)) if !p.as_os_str().is_empty() => resolve(p).join(n),
        _ => path.to_path_buf(),
    }
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn walk(
    dir: &Path,
    recursive: bool,
    exclude: &Path,
    out: &mut Vec<(PathBuf, Kind)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if is_hidden(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            if recursive && resolve(&path) != exclude {
                walk(&path, recursive, exclude, out)?;
            }
        } else if (ft.is_file() || (ft.is_symlink() && path.is_file()))
            && let Some(kind) = detect(&path).ok().flatten()
        {
            out.push((path, kind));
        }
    }
    Ok(())
}

/// Plans directory mode.
pub fn plan_dir(
    input_dir: &Path,
    output_dir: Option<&Path>,
    recursive: bool,
    filter: FileType,
    overwrite: bool,
) -> Result<Plan, CliError> {
    if !input_dir.is_dir() {
        return Err(CliError::Usage(format!(
            "input directory not found: {}",
            input_dir.display()
        )));
    }
    let output_dir = output_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| input_dir.join("ocr-output"));
    let exclude = resolve(&std::path::absolute(&output_dir).unwrap_or_else(|_| output_dir.clone()));

    let mut files = Vec::new();
    walk(input_dir, recursive, &exclude, &mut files)
        .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", input_dir.display())))?;
    let mut rels: Vec<(PathBuf, Kind)> = files
        .into_iter()
        .map(|(f, k)| (f.strip_prefix(input_dir).unwrap_or(&f).to_path_buf(), k))
        .collect();
    rels.sort_by(|a, b| a.0.cmp(&b.0));

    // Same folder + same stem (case-insensitive, as on APFS) means the extension is kept.
    // Counted before `--file-type` filtering so names don't depend on the filter.
    let key = |rel: &Path| {
        (
            rel.parent().map(Path::to_path_buf).unwrap_or_default(),
            rel.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    };
    let mut counts: HashMap<(PathBuf, String), usize> = HashMap::new();
    for (rel, _) in &rels {
        *counts.entry(key(rel)).or_default() += 1;
    }

    let mut plan = Plan::default();
    for (rel, kind) in rels {
        if !filter.matches(kind) {
            continue;
        }
        let name = rel.file_name().unwrap_or_default().to_string_lossy();
        let md = if counts[&key(&rel)] > 1 {
            format!("{name}.md")
        } else {
            format!(
                "{}.md",
                rel.file_stem().unwrap_or_default().to_string_lossy()
            )
        };
        let output = output_dir
            .join(rel.parent().unwrap_or(Path::new("")))
            .join(md);
        if output.exists() && !overwrite {
            plan.skipped_existing += 1;
            continue;
        }
        plan.items.push(Item {
            input: input_dir.join(&rel),
            output,
        });
    }
    Ok(plan)
}

/// Inclusive 1-based page ranges from `--pages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRanges(pub Vec<(u32, u32)>);

impl PageRanges {
    /// Parses e.g. `1-50,75`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let page = |t: &str| -> Result<u32, String> {
            match t.trim().parse::<u32>() {
                Ok(0) => Err("pages are 1-based; 0 is not valid".into()),
                Ok(n) => Ok(n),
                Err(_) => Err(format!("invalid page number '{t}'")),
            }
        };
        let mut ranges = Vec::new();
        for part in s.split(',') {
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (page(a)?, page(b)?),
                None => (page(part)?, page(part)?),
            };
            if a > b {
                return Err(format!("reversed page range '{part}'"));
            }
            ranges.push((a, b));
        }
        Ok(Self(ranges))
    }

    pub fn contains(&self, page: u32) -> bool {
        self.0.iter().any(|&(a, b)| (a..=b).contains(&page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const PDF: &[u8] = b"%PDF-1.7\n";
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn touch(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn outs(plan: &Plan, base: &Path) -> Vec<String> {
        plan.items
            .iter()
            .map(|i| {
                i.output
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn magic_bytes() {
        assert_eq!(kind_from_magic(b"%PDF-1.4"), Some(Kind::Pdf));
        assert_eq!(kind_from_magic(PNG), Some(Kind::Image));
        assert_eq!(
            kind_from_magic(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(Kind::Image)
        );
        assert_eq!(kind_from_magic(b"II*\0rest"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"MM\0*rest"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"\0\0\0\x18ftypheic"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"\0\0\0\x18ftypmif1"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"\0\0\0\x18ftypmp42"), None);
        assert_eq!(kind_from_magic(b"BM\0\0"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"GIF89a"), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"RIFF\0\0\0\0WEBPVP8 "), Some(Kind::Image));
        assert_eq!(kind_from_magic(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(kind_from_magic(b"hello"), None);
        assert_eq!(kind_from_magic(b""), None);
    }

    #[test]
    fn detects_by_content_not_extension() {
        let d = tempdir().unwrap();
        touch(d.path(), "fake.png", PDF);
        assert_eq!(detect(&d.path().join("fake.png")).unwrap(), Some(Kind::Pdf));
    }

    #[test]
    fn single_default_and_md_append() {
        let d = tempdir().unwrap();
        touch(d.path(), "Brief.pdf", PDF);
        let p = plan_single(&d.path().join("Brief.pdf"), None, FileType::Auto, false).unwrap();
        assert_eq!(p.items[0].output, PathBuf::from("Brief.md"));
        let p = plan_single(
            &d.path().join("Brief.pdf"),
            Some(Path::new("out.txt")),
            FileType::Pdf,
            false,
        )
        .unwrap();
        assert_eq!(p.items[0].output, PathBuf::from("out.txt.md"));
        assert_eq!(p.notes.len(), 1);
    }

    #[test]
    fn single_errors() {
        let d = tempdir().unwrap();
        touch(d.path(), "a.pdf", PDF);
        touch(d.path(), "a.md", b"x");
        touch(d.path(), "junk.pdf", b"nope");
        let a = d.path().join("a.pdf");
        assert!(plan_single(&d.path().join("missing.pdf"), None, FileType::Auto, false).is_err());
        assert!(plan_single(&a, None, FileType::Image, false).is_err());
        assert!(plan_single(&d.path().join("junk.pdf"), None, FileType::Auto, false).is_err());
        assert!(plan_single(&a, Some(&d.path().join("a.md")), FileType::Auto, false).is_err());
        assert!(plan_single(&a, Some(&d.path().join("a.md")), FileType::Auto, true).is_ok());
    }

    #[test]
    fn dir_defaults_collisions_hidden_and_sorting() {
        let d = tempdir().unwrap();
        touch(d.path(), "b.pdf", PDF);
        touch(d.path(), "a.pdf", PDF);
        touch(d.path(), "a.png", PNG);
        touch(d.path(), ".hidden.pdf", PDF);
        touch(d.path(), "notes.txt", b"text");
        touch(d.path(), "sub/c.pdf", PDF);
        let p = plan_dir(d.path(), None, false, FileType::Auto, false).unwrap();
        let base = d.path().join("ocr-output");
        assert_eq!(outs(&p, &base), ["a.pdf.md", "a.png.md", "b.md"]);
        let p = plan_dir(d.path(), None, false, FileType::Image, false).unwrap();
        assert_eq!(outs(&p, &base), ["a.png.md"]); // stable name despite filter
    }

    #[test]
    fn dir_recursive_mirrors_and_excludes_output() {
        let d = tempdir().unwrap();
        touch(d.path(), "top.pdf", PDF);
        touch(d.path(), "sub/deep/x.png", PNG);
        touch(d.path(), "ocr-output/old.pdf", PDF);
        touch(d.path(), "sub/.git/h.pdf", PDF);
        let p = plan_dir(d.path(), None, true, FileType::Auto, false).unwrap();
        let base = d.path().join("ocr-output");
        assert_eq!(outs(&p, &base), ["sub/deep/x.md", "top.md"]);
        let flat = plan_dir(d.path(), None, false, FileType::Auto, false).unwrap();
        assert_eq!(outs(&flat, &base), ["top.md"]);
    }

    #[test]
    fn dir_skips_existing_unless_overwrite() {
        let d = tempdir().unwrap();
        touch(d.path(), "a.pdf", PDF);
        touch(d.path(), "b.pdf", PDF);
        touch(d.path(), "ocr-output/a.md", b"done");
        let p = plan_dir(d.path(), None, false, FileType::Auto, false).unwrap();
        assert_eq!((p.items.len(), p.skipped_existing), (1, 1));
        let p = plan_dir(d.path(), None, false, FileType::Auto, true).unwrap();
        assert_eq!((p.items.len(), p.skipped_existing), (2, 0));
    }

    #[test]
    fn dir_custom_output_inside_input_is_excluded() {
        let d = tempdir().unwrap();
        touch(d.path(), "a.pdf", PDF);
        touch(d.path(), "out/b.pdf", PDF);
        let out = d.path().join("out");
        let p = plan_dir(d.path(), Some(&out), true, FileType::Auto, false).unwrap();
        assert_eq!(outs(&p, &out), ["a.md"]);
    }

    #[test]
    fn pages() {
        assert_eq!(
            PageRanges::parse("1-50,75").unwrap(),
            PageRanges(vec![(1, 50), (75, 75)])
        );
        let r = PageRanges::parse("2-4, 9").unwrap();
        assert!(r.contains(3) && r.contains(9) && !r.contains(5));
        for bad in ["0", "0-3", "5-2", "", "a", "1,,2", "1-", "-3", "1-2-3"] {
            assert!(PageRanges::parse(bad).is_err(), "{bad}");
        }
    }
}
