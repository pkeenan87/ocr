//! Bates numbers of the model engines off macOS. There is no Vision side-pass there, so a page's
//! Bates number comes from the model text (`markdown::find_bates_in_text`) or from corner
//! requests (`Ctx::corner_bates`). The fake server (tests/fake_server.py) answers corner crops
//! with FAKE_CORNER_BATES and ends full-page replies with FAKE_BODY_TAIL.

#![cfg(not(target_os = "macos"))]

mod common;

use common::{Env, is_corner};
use serde_json::Value;

const BATES: &str = "SMITH000123";
/// Pages of a document probed for a Bates corner, with nothing found, before probing stops
/// (`BATES_PROBE_PAGES`).
const PROBE_PAGES: usize = 3;

/// A PDF of `pages` empty Letter pages.
fn blank_pdf(pages: usize) -> Vec<u8> {
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 3 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
    ];
    objs.extend(
        (0..pages).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string()),
    );
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

/// Starts the fake server for `engine` with `env` and scans a blank document of `pages` pages
/// (one at a time, so the order of pages is the order of requests). Returns the environment and
/// the markdown.
fn scan(engine: &'static str, pages: usize, env: &[(&str, &str)]) -> (Env, String) {
    let e = Env::new(engine, &[]);
    std::fs::write(e.path("doc.pdf"), blank_pdf(pages)).unwrap();
    e.serve(env);
    e.scan("doc.pdf", &["--jobs", "1"], &[]).success();
    let md = e.markdown("doc.pdf");
    (e, md)
}

fn corners(e: &Env) -> Vec<Value> {
    e.all_requests().into_iter().filter(is_corner).collect()
}

/// Corner requests one page without a Bates number costs: olmOCR asks once per corner, dots
/// asks `prompt_ocr`, then the layout prompt, per corner.
fn probes_per_page(engine: &str) -> usize {
    match engine {
        "olmocr" => 3,
        _ => 6,
    }
}

fn marker(page: usize) -> String {
    format!("<!-- page {page} · Bates {BATES} -->")
}

fn bates_markers(md: &str) -> usize {
    md.matches(" · Bates ").count()
}

/// The model text has no Bates number: the first corner (bottom right) supplies it.
fn corner_request_supplies_the_bates_number(engine: &'static str) {
    let (e, md) = scan(engine, 1, &[("FAKE_CORNER_BATES", BATES)]);
    assert!(md.contains(&marker(1)), "{md}");
    let (all, pages) = (e.all_requests(), e.requests());
    assert_eq!(pages.len(), 1, "{all:?}");
    let c = corners(&e);
    assert_eq!(c.len(), 1, "the first corner already reads it: {all:?}");
    assert!(
        c[0]["w"].as_u64() > c[0]["h"].as_u64().map(|h| 2 * h),
        "{c:?}"
    );
    let first = if engine == "olmocr" {
        ocr::olmocr::PROMPT
    } else {
        ocr::dots::PROMPT_OCR
    };
    assert_eq!(c[0]["prompt"], first);
}

/// No Bates number anywhere: the first three pages are probed in all three corners, the rest are
/// not.
fn probing_stops_after_three_pages(engine: &'static str) {
    let (e, md) = scan(engine, 5, &[]);
    assert_eq!(bates_markers(&md), 0, "{md}");
    assert_eq!(e.requests().len(), 5, "one page request per page");
    assert_eq!(
        corners(&e).len(),
        PROBE_PAGES * probes_per_page(engine),
        "{:?}",
        e.all_requests()
    );
}

/// A document that shows Bates numbers is probed on every page.
fn probing_goes_on_while_bates_numbers_are_found(engine: &'static str) {
    let (e, md) = scan(engine, 5, &[("FAKE_CORNER_BATES", BATES)]);
    for page in 1..=5 {
        assert!(md.contains(&marker(page)), "{md}");
    }
    assert_eq!(corners(&e).len(), 5, "{:?}", e.all_requests());
}

/// An invoice number in the body text is not a Bates number: the page is still probed, and the
/// corner's number wins when there is one.
fn invoice_number_in_the_body_is_not_a_bates_number(engine: &'static str, tail: &str) {
    let (e, md) = scan(engine, 1, &[("FAKE_BODY_TAIL", tail)]);
    assert!(md.contains(tail), "the tail is in the page text: {md}");
    assert!(!md.contains("Bates INV-"), "{md}");
    assert_eq!(bates_markers(&md), 0, "{md}");
    assert_eq!(corners(&e).len(), probes_per_page(engine), "still probed");

    let (e, md) = scan(
        engine,
        1,
        &[("FAKE_BODY_TAIL", tail), ("FAKE_CORNER_BATES", BATES)],
    );
    assert!(!md.contains("Bates INV-"), "{md}");
    assert!(md.contains(&marker(1)), "{md}");
    assert_eq!(corners(&e).len(), 1);
}

/// A Bates number alone on a line of the body is found there: no corner request.
fn bates_alone_on_a_line_needs_no_corner(engine: &'static str) {
    let (e, md) = scan(engine, 1, &[("FAKE_BODY_TAIL", BATES)]);
    assert!(md.contains(&marker(1)), "{md}");
    assert!(corners(&e).is_empty(), "{:?}", e.all_requests());
}

const AMOUNT: &str = "Invoice No. INV-24337 in the amount of $48,250.00";
const DUE: &str = "Invoice No. INV-89271, which was due on May 1, 2025.";

macro_rules! per_engine {
    ($($engine:ident => $name:literal),*) => {$(
        mod $engine {
            use super::*;

            #[test]
            fn corner_request_supplies_the_bates_number() {
                super::corner_request_supplies_the_bates_number($name);
            }

            #[test]
            fn probing_stops_after_three_pages_without_bates() {
                super::probing_stops_after_three_pages($name);
            }

            #[test]
            fn probing_goes_on_while_bates_numbers_are_found() {
                super::probing_goes_on_while_bates_numbers_are_found($name);
            }

            #[test]
            fn invoice_number_with_an_amount_is_not_a_bates_number() {
                super::invoice_number_in_the_body_is_not_a_bates_number($name, AMOUNT);
            }

            #[test]
            fn invoice_number_followed_by_which_is_not_a_bates_number() {
                super::invoice_number_in_the_body_is_not_a_bates_number($name, DUE);
            }

            #[test]
            fn bates_alone_on_a_line_needs_no_corner() {
                super::bates_alone_on_a_line_needs_no_corner($name);
            }
        }
    )*};
}

per_engine!(olmocr => "olmocr", dots => "dots-ocr");
