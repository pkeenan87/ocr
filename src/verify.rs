//! `--verify-json`: a sidecar report of the values a reader must not get wrong (amounts, dates,
//! docket numbers, Bates numbers, citations, percentages), each with its page and line and a
//! status, plus the pages a second reader should look at (`azure_pages`, in Azure Document
//! Intelligence's `pages` syntax).
//!
//! A field is `verified` only when an independent reading agrees: the same value appears in the
//! page's embedded PDF text. It is `check` when something speaks against it (a suspect page, an
//! impossible date, a malformed amount, a docket number that differs from the document's usual
//! one, or embedded text that does not contain it). Otherwise it is `unchecked`; with
//! `--verify-fields all` those become `check` too, for documents where nothing may be missed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::markdown::{PageResult, Source};

/// Which unconfirmed fields need a second reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum VerifyFields {
    /// Only fields with something against them, and suspect or failed pages.
    #[default]
    Flagged,
    /// Every field not confirmed by the embedded text.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Amount,
    Date,
    Docket,
    Bates,
    Citation,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Verified,
    Check,
    Unchecked,
}

#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub kind: Kind,
    /// Exactly as it appears in the Markdown.
    pub text: String,
    /// Amount in dollars, date as YYYY-MM-DD, when the text parses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// 1-based line within the page's Markdown.
    pub line: usize,
    pub status: Status,
    pub why: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageReport {
    pub page: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bates: Option<String>,
    /// Why the whole page needs a second reading (suspect reasons, `failed`), if it does.
    pub page_check: Vec<String>,
    /// Whether the page has embedded PDF text that fields were compared with.
    pub embedded_text: bool,
    /// How to get a second reading of this page, if it needs one: `embedded_text` (free: the
    /// PDF's own text, for a suspect page whose values all match it) or `azure`.
    pub second_reading: Option<&'static str>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Serialize)]
pub struct Report<'a> {
    pub source: &'a str,
    pub markdown: String,
    pub engine: &'a str,
    pub generated: &'a str,
    pub verify_fields: &'static str,
    /// Pages to send to a second reader, as `3,7-9`; empty when none.
    pub azure_pages: String,
    pub summary: Summary,
    pub pages: Vec<PageReport>,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub fields: usize,
    pub verified: usize,
    pub check: usize,
    pub unchecked: usize,
    /// Pages in `azure_pages`.
    pub pages_to_check: usize,
    /// Suspect pages whose values all match the embedded text: read that text instead.
    pub embedded_text_pages: usize,
}

const MONTHS: &str = "January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sept|Sep|Oct|Nov|Dec";

/// Anything that looks like money, malformed or not (`$4825,0.00`, `$ 1,00.00`).
/// A scale word after the number (`$33.5 million`) is part of the amount.
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\$\s?\d[\d,]*(?:\.\d+)?(?:\s(?:thousand|million|billion|trillion)\b)?")
        .unwrap()
});
static AMOUNT_OK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\$\s?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d{1,3})?(?:\s(?:thousand|million|billion|trillion))?$")
        .unwrap()
});
/// Numeric dates, with letters OCR puts in place of digits (`02/03/0Q`).
static DATE_NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9OIlQ]{1,2}/[0-9OIlQ]{1,2}/[0-9OIlQ]{2,4}\b|\b\d{4}-\d{2}-\d{2}\b").unwrap()
});
static DATE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b(?:(?P<m1>{MONTHS})\.?\s+(?P<d1>[0-9OIl]{{1,2}}),?\s+(?P<y1>[0-9OIlQ]{{4}})|(?P<d2>\d{{1,2}})\s+(?P<m2>{MONTHS})\.?,?\s+(?P<y2>\d{{4}}))\b"
    ))
    .unwrap()
});
/// Federal docket numbers (`3:26-cv-04417`, `1:20-CV-05382-GBD-GWG`, `23-CV-886-PP`).
static DOCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:\d{1,2}:)?\d{2}-(?:cv|cr|mc|md|bk|ap|mj|po|sw|civ|cvd)-\d{2,6}(?:-[A-Z]{1,4})*\b")
        .unwrap()
});
/// Citations with a section or paragraph sign.
static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:§§?|¶¶?)\s?\d[\w.\-]*(?:\([a-z0-9]{1,4}\))*").unwrap());
static PERCENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{1,3}(?:\.\d+)?\s?%").unwrap());
/// Bates prefix and number (`SMITH000123`, `ABC 0001234`).
static BATES_PARTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<prefix>.*?)(?P<num>\d+)$").unwrap());

fn month_number(m: &str) -> Option<u32> {
    let m = m.to_ascii_lowercase();
    [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|p| m.starts_with(p))
    .map(|i| i as u32 + 1)
}

fn valid_ymd(y: i32, m: u32, d: u32) -> bool {
    chrono::NaiveDate::from_ymd_opt(y, m, d).is_some() && (1900..=2100).contains(&y)
}

fn year4(y: &str) -> Option<i32> {
    let n: i32 = y.parse().ok()?;
    Some(match y.len() {
        2 if n >= 50 => 1900 + n,
        2 => 2000 + n,
        4 => n,
        _ => return None,
    })
}

/// A date's ISO form, or `None` if it is not a real date (or holds misread letters).
fn parse_date(text: &str) -> Option<String> {
    if let Some(c) = DATE_WORDS.captures(text) {
        let (m, d, y) = match (c.name("m1"), c.name("m2")) {
            (Some(m), _) => (m, &c["d1"], &c["y1"]),
            (_, Some(m)) => (m, &c["d2"], &c["y2"]),
            _ => return None,
        };
        let (m, d, y) = (month_number(m.as_str())?, d.parse().ok()?, y.parse().ok()?);
        return valid_ymd(y, m, d).then(|| format!("{y:04}-{m:02}-{d:02}"));
    }
    if let Some((y, rest)) = text.split_once('-').filter(|(y, _)| y.len() == 4) {
        let (m, d) = rest.split_once('-')?;
        let (y, m, d) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
        return valid_ymd(y, m, d).then(|| format!("{y:04}-{m:02}-{d:02}"));
    }
    let parts: Vec<&str> = text.split('/').collect();
    let [m, d, y] = parts[..] else { return None };
    let (m, d, y) = (m.parse().ok()?, d.parse().ok()?, year4(y)?);
    valid_ymd(y, m, d).then(|| format!("{y:04}-{m:02}-{d:02}"))
}

fn parse_amount(text: &str) -> Option<f64> {
    let t = text.trim_start_matches('$').trim().to_ascii_lowercase();
    let (number, scale) = match t.split_once(' ') {
        Some((n, "thousand")) => (n, 1e3),
        Some((n, "million")) => (n, 1e6),
        Some((n, "billion")) => (n, 1e9),
        Some((n, "trillion")) => (n, 1e12),
        _ => (t.as_str(), 1.0),
    };
    number
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .map(|v| v * scale)
}

/// Letters and digits only, lowercased: how a value is looked for in embedded text, so spacing
/// and line breaks do not matter.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || "$§¶%".contains(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The fields of one page's Markdown, before any status is decided.
fn extract(body: &str) -> Vec<Field> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        if line.trim_start().starts_with("<!--") {
            continue;
        }
        let mut found: Vec<(usize, Field)> = Vec::new();
        let mut push =
            |at: usize, kind: Kind, text: &str, value: Option<serde_json::Value>, why| {
                found.push((
                    at,
                    Field {
                        kind,
                        text: text.to_string(),
                        value,
                        line: i + 1,
                        status: Status::Unchecked,
                        why,
                    },
                ));
            };
        for m in AMOUNT.find_iter(line) {
            let t = m.as_str().trim_end_matches([',', '.']);
            let ok = AMOUNT_OK.is_match(t);
            let value = parse_amount(t)
                .filter(|_| ok)
                .and_then(|v| serde_json::Number::from_f64(v).map(serde_json::Value::Number));
            push(
                m.start(),
                Kind::Amount,
                t,
                value,
                if ok { vec![] } else { vec!["malformed_amount"] },
            );
        }
        // `2 March 2023` at the start of a line is a pleading line number before `March 2023`.
        let indent = line.len() - line.trim_start().len();
        let mut dates: Vec<(usize, &str)> = DATE_WORDS
            .captures_iter(line)
            .filter(|c| !(c.name("d2").is_some() && c.get(0).unwrap().start() == indent))
            .map(|c| c.get(0).unwrap())
            .chain(DATE_NUMERIC.find_iter(line))
            .map(|m| (m.start(), m.as_str()))
            .collect();
        dates.sort_unstable();
        dates.dedup();
        for (at, t) in dates {
            match parse_date(t) {
                Some(iso) => push(at, Kind::Date, t, Some(iso.into()), vec![]),
                None => push(at, Kind::Date, t, None, vec!["invalid_date"]),
            }
        }
        for m in DOCKET.find_iter(line) {
            push(m.start(), Kind::Docket, m.as_str(), None, vec![]);
        }
        for m in CITATION.find_iter(line) {
            let t = m.as_str().trim_end_matches(['.', ',', '-']);
            push(m.start(), Kind::Citation, t, None, vec![]);
        }
        for m in PERCENT.find_iter(line) {
            push(m.start(), Kind::Percent, m.as_str(), None, vec![]);
        }
        found.sort_by_key(|(at, _)| *at);
        out.extend(found.into_iter().map(|(_, f)| f));
    }
    out
}

fn bates_number(b: &str) -> Option<(String, u64)> {
    let c = BATES_PARTS.captures(b.trim())?;
    Some((c["prefix"].trim().to_string(), c["num"].parse().ok()?))
}

/// Docket numbers that differ slightly from the document's most common one (a misread digit or
/// letter in a running header). Returns the normalized odd ones out.
fn odd_dockets(pages: &[(usize, Vec<Field>)]) -> BTreeSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, fields) in pages {
        for f in fields.iter().filter(|f| f.kind == Kind::Docket) {
            *counts.entry(f.text.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let Some((usual, n)) = counts.iter().max_by_key(|(k, n)| (**n, k.len())) else {
        return BTreeSet::new();
    };
    if *n < 3 {
        return BTreeSet::new();
    }
    counts
        .keys()
        .filter(|k| *k != usual && close(k, usual))
        .cloned()
        .collect()
}

/// Same length and at most two differing characters, or one character more or less.
fn close(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len() == b.len() {
        return a.iter().zip(&b).filter(|(x, y)| x != y).count() <= 2;
    }
    let (short, long) = if a.len() < b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    long.len() == short.len() + 1
        && (0..long.len()).any(|i| {
            let mut l = long.clone();
            l.remove(i);
            &l == short
        })
}

/// Builds the report. `embedded(page)` returns the page's embedded PDF text (empty for none).
pub fn report<'a>(
    source: &'a str,
    markdown: String,
    engine: &'a str,
    generated: &'a str,
    pages: &[PageResult],
    mode: VerifyFields,
    embedded: &dyn Fn(usize) -> String,
) -> Report<'a> {
    let extracted: Vec<(usize, Vec<Field>)> = pages
        .iter()
        .map(|p| {
            (
                p.number,
                if p.error.is_some() {
                    vec![]
                } else {
                    extract(&p.body)
                },
            )
        })
        .collect();
    let odd = odd_dockets(&extracted);
    // Bates numbers rise by one per page, so number minus page is constant within a production;
    // a page off the usual prefix and offset is a likely misread.
    let offsets: BTreeMap<usize, (String, i64)> = pages
        .iter()
        .filter_map(|p| {
            let (prefix, n) = bates_number(p.bates.as_deref()?)?;
            Some((p.number, (prefix, n as i64 - p.number as i64)))
        })
        .collect();
    let mut tally: HashMap<&(String, i64), usize> = HashMap::new();
    for o in offsets.values() {
        *tally.entry(o).or_default() += 1;
    }
    let usual = tally
        .iter()
        .max_by_key(|(_, n)| **n)
        .filter(|(_, n)| **n >= 2)
        .map(|(o, _)| (*o).clone());

    let mut out = Vec::new();
    for (p, (number, mut fields)) in pages.iter().zip(extracted) {
        let mut page_check: Vec<String> = p.suspect.iter().map(|s| s.to_string()).collect();
        if let Some(e) = &p.error {
            page_check.push(format!("failed: {e}"));
        }
        if let Some(b) = &p.bates {
            let mut why = vec![];
            if usual
                .as_ref()
                .is_some_and(|u| offsets.get(&number).is_some_and(|o| o != u))
            {
                why.push("bates_out_of_sequence");
            }
            fields.insert(
                0,
                Field {
                    kind: Kind::Bates,
                    text: b.clone(),
                    value: None,
                    line: 0,
                    status: Status::Unchecked,
                    why,
                },
            );
        }
        let text = if p.source == Source::TextLayer || p.error.is_some() {
            String::new()
        } else {
            embedded(number)
        };
        let has_text = text.split_whitespace().count() >= 20;
        let squashed = squash(&text);
        for f in &mut fields {
            if f.kind == Kind::Docket && odd.contains(&f.text.to_ascii_lowercase()) {
                f.why.push("differs_from_usual_docket");
            }
            if p.source == Source::TextLayer {
                f.why.push("from_embedded_text");
            }
            if !page_check.is_empty() {
                f.why.push("suspect_page");
            }
            let found = has_text && squashed.contains(&squash(&f.text));
            if has_text && !found {
                f.why.push("not_in_embedded_text");
            }
            // Two independent readings of the exact value agree: that outweighs a suspect page.
            let page_only = f.why.iter().all(|w| *w == "suspect_page");
            let against = f.why.iter().any(|w| *w != "from_embedded_text");
            f.status = if found && page_only {
                f.why.clear();
                f.why.push("matches_embedded_text");
                Status::Verified
            } else if against {
                Status::Check
            } else if found {
                f.why.push("matches_embedded_text");
                Status::Verified
            } else if mode == VerifyFields::All && p.source != Source::TextLayer {
                f.why.push("unconfirmed");
                Status::Check
            } else {
                Status::Unchecked
            };
        }
        let field_check = fields.iter().any(|f| f.status == Status::Check);
        let second_reading = if field_check {
            Some("azure")
        } else if page_check.is_empty() {
            None
        } else if has_text {
            Some("embedded_text")
        } else {
            Some("azure")
        };
        out.push(PageReport {
            page: number,
            bates: p.bates.clone(),
            page_check,
            embedded_text: has_text,
            second_reading,
            fields,
        });
    }

    let to_check: Vec<usize> = out
        .iter()
        .filter(|p| p.second_reading == Some("azure"))
        .map(|p| p.page)
        .collect();
    let mut summary = Summary {
        pages_to_check: to_check.len(),
        embedded_text_pages: out
            .iter()
            .filter(|p| p.second_reading == Some("embedded_text"))
            .count(),
        ..Summary::default()
    };
    for f in out.iter().flat_map(|p| &p.fields) {
        summary.fields += 1;
        match f.status {
            Status::Verified => summary.verified += 1,
            Status::Check => summary.check += 1,
            Status::Unchecked => summary.unchecked += 1,
        }
    }
    Report {
        source,
        markdown,
        engine,
        generated,
        verify_fields: match mode {
            VerifyFields::Flagged => "flagged",
            VerifyFields::All => "all",
        },
        azure_pages: ranges(&to_check),
        summary,
        pages: out,
    }
}

/// `[3, 7, 8, 9, 41]` as `3,7-9,41`.
pub fn ranges(pages: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        while i + 1 < pages.len() && pages[i + 1] == pages[i] + 1 {
            i += 1;
        }
        parts.push(if pages[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", pages[i])
        });
        i += 1;
    }
    parts.join(",")
}

/// `<dir>/<stem>.verify.json` next to the Markdown output.
pub fn sidecar(markdown: &std::path::Path) -> std::path::PathBuf {
    let stem = markdown
        .file_stem()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    markdown.with_file_name(format!("{stem}.verify.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(number: usize, body: &str) -> PageResult {
        PageResult {
            number,
            source: Source::Vision,
            body: body.into(),
            bates: None,
            column_split: false,
            transcript: false,
            currency_fixes: 0,
            table: false,
            fallback: false,
            flagged: false,
            error: None,
            line_numbers: None,
            symbol_fixes: 0,
            embedded_fixes: None,
            suspect: vec![],
        }
    }

    fn run(pages: &[PageResult], mode: VerifyFields, text: &str) -> Report<'static> {
        let text = text.to_string();
        report("x.pdf", "x.md".into(), "apple", "now", pages, mode, &|_| {
            text.clone()
        })
    }

    fn statuses(r: &Report) -> Vec<(String, Status, Vec<&'static str>)> {
        r.pages
            .iter()
            .flat_map(|p| &p.fields)
            .map(|f| (f.text.clone(), f.status, f.why.clone()))
            .collect()
    }

    #[test]
    fn fields_are_found_and_parsed() {
        let f = extract(
            "Agreement dated March 3, 2024 for $48,250.00 and 12.5% interest.\n\
             Case No.3:26-cv-04417, filed 04/15/2024 under 28 U.S.C. § 1441(b) and Compl. ¶ 60.",
        );
        let got: Vec<(Kind, &str)> = f.iter().map(|f| (f.kind, f.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Kind::Date, "March 3, 2024"),
                (Kind::Amount, "$48,250.00"),
                (Kind::Percent, "12.5%"),
                (Kind::Docket, "3:26-cv-04417"),
                (Kind::Date, "04/15/2024"),
                (Kind::Citation, "§ 1441(b)"),
                (Kind::Citation, "¶ 60"),
            ]
        );
        assert_eq!(f[0].value, Some(serde_json::json!("2024-03-03")));
        assert_eq!(f[1].value, Some(serde_json::json!(48250.0)));
        assert_eq!(f[4].value, Some(serde_json::json!("2024-04-15")));
    }

    #[test]
    fn misreads_are_checked() {
        let f = extract("Filed 02/03/0Q, paid $4825,0.00 on 02/30/2024 and February 31, 2023.");
        let why: Vec<(&str, Vec<&str>)> =
            f.iter().map(|f| (f.text.as_str(), f.why.clone())).collect();
        assert_eq!(
            why,
            [
                ("02/03/0Q", vec!["invalid_date"]),
                ("$4825,0.00", vec!["malformed_amount"]),
                ("02/30/2024", vec!["invalid_date"]),
                ("February 31, 2023", vec!["invalid_date"]),
            ]
        );
    }

    #[test]
    fn embedded_text_verifies_or_contradicts() {
        let body = "The total due is $122,525.50, plus interest, as of 06/01/2024.";
        let layer = "The total due is $122,525.50, plus interest, as of 06/01/2024. More words here to \
                     pass the minimum of twenty words in the embedded text of this page.";
        let r = run(&[page(1, body)], VerifyFields::Flagged, layer);
        assert!(
            statuses(&r).iter().all(|s| s.1 == Status::Verified),
            "{:?}",
            statuses(&r)
        );
        assert_eq!(r.azure_pages, "");
        // Embedded text says 8,000; OCR read 3,000.
        let r = run(
            &[page(1, "Showroom guide: $3,000 per year.")],
            VerifyFields::Flagged,
            &layer.replace("$122,525.50", "$8,000"),
        );
        assert_eq!(statuses(&r)[0].1, Status::Check);
        assert_eq!(statuses(&r)[0].2, ["not_in_embedded_text"]);
        assert_eq!(r.azure_pages, "1");
    }

    #[test]
    fn without_embedded_text_all_mode_checks_everything() {
        let pages = [
            page(1, "Due: $500.00 on 06/01/2024."),
            page(2, "No values here."),
        ];
        let r = run(&pages, VerifyFields::Flagged, "");
        assert!(statuses(&r).iter().all(|s| s.1 == Status::Unchecked));
        assert_eq!(r.azure_pages, "");
        let r = run(&pages, VerifyFields::All, "");
        assert!(
            statuses(&r)
                .iter()
                .all(|s| s.1 == Status::Check && s.2 == ["unconfirmed"])
        );
        assert_eq!(r.azure_pages, "1");
    }

    #[test]
    fn suspect_and_failed_pages_are_sent_whole() {
        let mut bad = page(2, "garbled text $10.00");
        bad.suspect = vec!["garbled"];
        let mut failed = page(3, "");
        failed.error = Some("timeout".into());
        let r = run(
            &[page(1, "fine"), bad, failed, page(4, "fine")],
            VerifyFields::Flagged,
            "",
        );
        assert_eq!(r.azure_pages, "2-3");
        assert_eq!(r.pages[1].fields[0].why, ["suspect_page"]);
        assert_eq!(r.pages[2].page_check, ["failed: timeout"]);
    }

    #[test]
    fn odd_docket_numbers_and_bates_breaks_are_checked() {
        let mut pages: Vec<PageResult> = (1..=5)
            .map(|n| {
                let mut p = page(n, "Case 3:23-cv-06621-SI Document 34");
                p.bates = Some(format!("ABC{:06}", 100 + n));
                p
            })
            .collect();
        pages[2].body = "Case 3:23-cv-06627-SI Document 34".into();
        pages[3].bates = Some("ABC000184".into());
        let r = run(&pages, VerifyFields::Flagged, "");
        assert_eq!(r.pages[2].fields[1].why, ["differs_from_usual_docket"]);
        assert_eq!(r.pages[3].fields[0].why, ["bates_out_of_sequence"]);
        assert_eq!(r.azure_pages, "3-4");
    }

    #[test]
    fn scaled_amounts_and_margin_numbers() {
        let f = extract(
            "A $33.5 million settlement and $1.5 Billion in losses.\n2 March 2023 was a Friday.",
        );
        let got: Vec<(&str, Option<serde_json::Value>)> = f
            .iter()
            .map(|f| (f.text.as_str(), f.value.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("$33.5 million", Some(serde_json::json!(33_500_000.0))),
                ("$1.5 Billion", Some(serde_json::json!(1_500_000_000.0))),
            ]
        );
        assert!(f.iter().all(|f| f.why.is_empty()));
        assert_eq!(
            extract("Signed on 2 March 2023.")[0].value,
            Some(serde_json::json!("2023-03-02"))
        );
    }

    #[test]
    fn a_suspect_page_matching_its_embedded_text_needs_no_azure() {
        let mut p = page(
            1,
            "garbled lnes but the total is $122,525.50 as of 06/01/2024.",
        );
        p.suspect = vec!["misreads"];
        let layer = "garbled lines but the total is $122,525.50 as of 06/01/2024, with more words \
                     so that the embedded text of this page passes the minimum length.";
        let r = run(&[p], VerifyFields::Flagged, layer);
        assert!(
            statuses(&r).iter().all(|s| s.1 == Status::Verified),
            "{:?}",
            statuses(&r)
        );
        assert_eq!(r.pages[0].second_reading, Some("embedded_text"));
        assert_eq!(
            (r.azure_pages.as_str(), r.summary.embedded_text_pages),
            ("", 1)
        );
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[3, 7, 8, 9, 41]), "3,7-9,41");
        assert_eq!(ranges(&[]), "");
        assert_eq!(
            sidecar(std::path::Path::new("/a/b/x.md")),
            std::path::Path::new("/a/b/x.verify.json")
        );
    }
}
