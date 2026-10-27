//! Write the `.verify.json` sidecar for an existing Markdown file:
//! verify_md <pdf> <markdown> [--engine NAME] [--fields all|flagged]
use ocr::markdown::{PageResult, Source};
use ocr::verify::{self, VerifyFields};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const USAGE: &str = "usage: verify_md <pdf> <markdown> [--engine NAME] [--fields all|flagged]";

fn split_pages(md: &str) -> Vec<(usize, String, Option<String>)> {
    let md = if md.starts_with("---\n") {
        md.split_once("\n---\n").map_or("", |(_, rest)| rest)
    } else {
        md
    };
    let re = Regex::new(r"<!-- page (\d+)([^>]*)-->").unwrap();
    let caps: Vec<_> = re.captures_iter(md).collect();
    let mut out = Vec::new();
    for (i, c) in caps.iter().enumerate() {
        let whole = c.get(0).unwrap();
        let end = caps
            .get(i + 1)
            .map_or(md.len(), |n| n.get(0).unwrap().start());
        let body = md[whole.end()..end].trim().to_string();
        let rest = c.get(2).unwrap().as_str();
        let error = rest
            .strip_prefix(": OCR failed: ")
            .map(|r| r.trim().to_string());
        out.push((c[1].parse().unwrap_or(0), body, error));
    }
    out
}

fn page_result(number: usize, body: &str, error: Option<String>) -> PageResult {
    PageResult {
        number,
        source: Source::Vision,
        body: ocr::htmltable::convert_tables(body).0,
        bates: None,
        column_split: false,
        transcript: false,
        currency_fixes: 0,
        table: false,
        fallback: false,
        flagged: false,
        error,
        line_numbers: None,
        embedded_fixes: None,
        symbol_fixes: 0,
        suspect: vec![],
    }
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let mut pos = Vec::new();
    let mut engine = "candidate".to_string();
    let mut mode = VerifyFields::All;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--engine" => engine = args.next().unwrap_or_else(|| usage()),
            "--fields" => {
                mode = match args.next().as_deref() {
                    Some("all") => VerifyFields::All,
                    Some("flagged") => VerifyFields::Flagged,
                    _ => usage(),
                }
            }
            s if s.starts_with("--") => usage(),
            _ => pos.push(PathBuf::from(a)),
        }
    }
    if pos.len() != 2 {
        usage();
    }
    if let Err(e) = run(&pos[0], &pos[1], &engine, mode) {
        fail(format!("verify_md: {e}"));
    }
}

fn run(
    pdf: &std::path::Path,
    md: &std::path::Path,
    engine: &str,
    mode: VerifyFields,
) -> Result<(), String> {
    let text = std::fs::read_to_string(md).map_err(|e| e.to_string())?;
    let split = split_pages(&text);
    if split.is_empty() {
        fail(format!(
            "verify_md: no <!-- page N --> markers in {}",
            md.display()
        ));
    }
    let total = ocr::vision::document_info(pdf)
        .map_err(|e| format!("{e:#}"))?
        .pages;
    let mut seen = HashSet::new();
    for (n, _, _) in &split {
        if !(1..=total).contains(n) || !seen.insert(*n) {
            fail(format!(
                "verify_md: page {n} is outside 1..={total} or repeated in {}",
                md.display()
            ));
        }
    }
    let pages: Vec<PageResult> = split
        .into_iter()
        .map(|(n, b, e)| page_result(n, &b, e))
        .collect();
    let annotations: HashMap<usize, Result<Vec<String>, ()>> = pages
        .iter()
        .filter(|p| p.error.is_none())
        .map(|p| {
            (
                p.number,
                ocr::vision::annotations(pdf, p.number - 1).map_err(|_| ()),
            )
        })
        .collect();
    let source = pdf
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let generated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let report = verify::report(
        &source,
        md.display().to_string(),
        engine,
        &generated,
        &pages,
        mode,
        &|page| verify::Embedded {
            text: ocr::vision::text_layer(pdf, page - 1).unwrap_or_default(),
            visibility: ocr::vision::text_visibility(pdf, page - 1),
            annotations: annotations
                .get(&page)
                .and_then(|a| a.as_ref().ok().cloned())
                .unwrap_or_default(),
            annotations_unknown: matches!(annotations.get(&page), Some(Err(()))),
        },
        &|_| None,
        true,
    );
    let json = serde_json::to_string_pretty(&report).unwrap() + "\n";
    std::fs::write(verify::sidecar(md), json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: usize, b: &str, e: Option<&str>) -> (usize, String, Option<String>) {
        (n, b.to_string(), e.map(String::from))
    }

    #[test]
    fn strips_front_matter() {
        assert_eq!(
            split_pages("---\nengine: x\n---\n\n<!-- page 1 -->\n\nhello"),
            vec![p(1, "hello", None)]
        );
    }

    #[test]
    fn bates_marker() {
        assert_eq!(
            split_pages("<!-- page 3 -->\n\na\n\n<!-- page 4 · Bates AB1 -->\n\nb"),
            vec![p(3, "a", None), p(4, "b", None)]
        );
    }

    #[test]
    fn failed_marker() {
        assert_eq!(
            split_pages("<!-- page 5: OCR failed: x -->\n\n"),
            vec![p(5, "", Some("x"))]
        );
    }

    #[test]
    fn table_converted() {
        let r = page_result(1, "<table><tr><td>a</td><td>b</td></tr></table>", None);
        assert!(r.body.contains('|') && !r.body.contains("<table"));
    }

    #[test]
    fn error_kept() {
        assert_eq!(
            page_result(2, "", Some("boom".into())).error.as_deref(),
            Some("boom")
        );
    }
}
