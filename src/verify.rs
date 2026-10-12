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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::markdown::{PageResult, Source};
use crate::vision::{Line, TextVisibility};

/// Below this Vision confidence a second-reader line confirms nothing. Accurate-mode confidence
/// is coarse (mostly 0.5 or 1.0), so this only drops the lines Vision itself marks as doubtful.
const SECOND_READER_MIN_CONFIDENCE: f64 = 0.5;

/// Words in lines that another reading has and the OCR lacks entirely, from which a page gets
/// `missing_text` (docs/spike.md).
const MISSING_TEXT_MIN_WORDS: usize = 5;

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
    /// Exactly as it appears in the Markdown, except that a scale word starting the next line
    /// (`$45` / `million`) is joined on with a space.
    pub text: String,
    /// Amount in dollars, date as YYYY-MM-DD, when the text parses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// 1-based line within the page's Markdown.
    pub line: usize,
    pub status: Status,
    pub why: Vec<&'static str>,
    /// Byte offset of the value in its line, for its context (the label before it).
    #[serde(skip)]
    pub(crate) col: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageReport {
    pub page: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bates: Option<String>,
    /// Why the whole page needs a second reading (suspect reasons, `failed`), if it does.
    pub page_check: Vec<String>,
    /// Kinds of annotation on the page that OCR never saw (`annotations_not_read`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<String>,
    /// Whether the page has embedded PDF text that fields were compared with.
    pub embedded_text: bool,
    /// Whether that text is drawn invisibly (an OCR layer laid over a scan): another OCR's
    /// reading, not the document's own characters. Agreement with it verifies nothing.
    pub ocr_text_layer: bool,
    /// What the scan of the page's text drawing found: `visible`, `ocr_layer` or `unknown` (a
    /// scan error counts). Only meaningful when `embedded_text` is true. `unknown` confirms
    /// nothing, like `ocr_layer`.
    pub text_layer_visibility: &'static str,
    /// Whether a second, local reading (Vision's line recognizer at 300 DPI) was compared with,
    /// on a page without embedded text.
    pub second_reader: bool,
    /// Which sentences of the page the embedded text or the second reading has word for word.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_agreement: Option<crate::agreement::TextAgreement>,
    /// How to get a second reading of this page, if it needs one: `embedded_text` (free: the
    /// PDF's own text, for a suspect page whose values all match it) or `azure`.
    pub second_reading: Option<&'static str>,
    pub fields: Vec<Field>,
    /// Values the other reading has that the OCR has nowhere on the page, as that reading
    /// writes them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub expected_fields_missing: Vec<String>,
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
    /// Sentences that an independent second reading (embedded text, or Vision against a model)
    /// has word for word, and those it does not.
    pub agreed_spans: usize,
    pub unagreed_spans: usize,
}

const MONTHS: &str = "January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sept|Sep|Oct|Nov|Dec";

/// Anything that looks like money, malformed or not (`$4825,0.00`, `$ 1,00.00`).
/// A scale word after the number (`$33.5 million`) is part of the amount.
/// Letters OCR puts in place of digits (`O`/`0`, `l`/`1`, `S`/`5`, `B`/`8`) are accepted, so a
/// misread amount is still found and flagged instead of silently missed.
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\$\s?-?[\dOolISB][\dOolISB,]*(?:\.[\dOolISB]+)?(?:\s(?:thousand|million|billion|trillion)\b|[KMB]\b)?")
        .unwrap()
});
static AMOUNT_OK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^-?\$?\s?-?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d{1,3})?(?:\s(?:thousand|million|billion|trillion)|[KMB])?$")
        .unwrap()
});
/// Money without a dollar sign, in the three shapes that cannot be a count: `USD 5,000`,
/// `5,000 dollars`, and `sum of 5,000.00` (cents required: `amount of 1,500 units` is not money).
static BARE_AMOUNTS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    const N: &str = r"(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d{2})?(?:\s(?:thousand|million|billion))?";
    [
        Regex::new(&format!(r"(?i)\bUSD\s?{N}\b")).unwrap(),
        Regex::new(&format!(r"(?i)\b{N}\s(?:U\.S\.\s)?dollars\b")).unwrap(),
        Regex::new(
            r"(?i)\b(?:sum|amount|total|price|fee|payment|balance) of\s(?P<n>(?:\d{1,3}(?:,\d{3})+|\d+)\.\d{2})\b",
        )
        .unwrap(),
    ]
});
/// A number with cents, or with digit grouping, as a table cell or an invoice line writes money:
/// `5,156.00`, `74.52`, `(20.00)`.
static MONEY_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(?-?\$?\s?(?:\d{1,3}(?:,\d{3})+(?:\.\d{2})?|\d{1,7}\.\d{2})\)?$").unwrap()
});
/// Grouped with cents: money wherever it stands.
static GROUPED_CENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{1,3}(?:,\d{3})+\.\d{2}").unwrap());
/// A table column or a line above that says the numbers are money.
static MONEY_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:amount|rate|fees?|costs?|price|total|balance|payments?|due|charges?|disbursements?|subtotal)\b|\$").unwrap()
});
/// Columns that hold quantities, not money, even under a money word (`TOTAL HOURS`).
static COUNT_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:hours?|hrs|qty|quantity|units?|days?|no\.?|count)\b").unwrap()
});

/// Money without a `$` on `lines[i]`, by three rules: grouped with cents anywhere; a number with
/// cents in a table column whose heading is a money word; and a line of nothing but such numbers
/// within six lines below a `$` or a money word (an invoice whose `$` stands apart). Returns
/// (byte offset, text) pairs.
fn unsigned_money(lines: &[&str], i: usize) -> Vec<(usize, String)> {
    let line = lines[i];
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut add = |at: usize, t: &str| {
        if !out.iter().any(|(a, _)| *a == at) {
            out.push((at, t.to_string()));
        }
    };
    for m in GROUPED_CENTS.find_iter(line) {
        let before = line[..m.start()].chars().next_back();
        let after = line[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_digit() || c == ',' || c == '.')
            || after.is_some_and(|c| c.is_ascii_digit() || c == '%')
        {
            continue;
        }
        add(m.start(), m.as_str());
    }
    let t = line.trim();
    if t.starts_with('|') {
        // The table's heading row: the first row of this block of pipe rows.
        let top = (0..=i)
            .rev()
            .take_while(|&k| lines[k].trim().starts_with('|'))
            .last()
            .unwrap_or(i);
        let head: Vec<&str> = lines[top].trim().trim_matches('|').split('|').collect();
        let money_col = |c: usize| {
            head.get(c)
                .is_some_and(|h| MONEY_WORDS.is_match(h) && !COUNT_WORDS.is_match(h))
        };
        if top != i {
            let mut at = line.find('|').unwrap_or(0) + 1;
            for (c, cell) in line.trim().trim_matches('|').split('|').enumerate() {
                let start = line[at..].find(cell).map_or(at, |o| at + o);
                let v = cell.trim();
                if money_col(c) && MONEY_SHAPE.is_match(v) {
                    add(start + (cell.len() - cell.trim_start().len()), v);
                }
                at = start + cell.len();
            }
        }
    }
    // A line of nothing but numbers with cents (and Markdown or leader dots around them).
    let bare: Vec<&str> = t
        .split(|c: char| c.is_whitespace() || c == '|')
        .map(|w| w.trim_matches(|c: char| c == '#' || c == '.' || c == '\u{2026}'))
        .filter(|w| !w.is_empty())
        .collect();
    if !bare.is_empty()
        && bare
            .iter()
            .all(|w| MONEY_SHAPE.is_match(w) && w.contains('.'))
        && lines[..i]
            .iter()
            .rev()
            .filter(|l| !l.trim().is_empty())
            .take(6)
            .any(|l| MONEY_WORDS.is_match(l))
    {
        let mut from = 0;
        for w in bare {
            if let Some(o) = line[from..].find(w) {
                add(from + o, w);
                from += o + w.len();
            }
        }
    }
    out.sort();
    out
}

/// An amount written without `$` (`USD 5,000`, `5,000 dollars`, `5,000.00`) as `$5,000`.
fn dollar_form(text: &str) -> String {
    let t = text.trim();
    let t = t
        .strip_prefix("USD")
        .or_else(|| t.strip_prefix("usd"))
        .unwrap_or(t)
        .trim();
    let lower = t.to_ascii_lowercase();
    let t = ["u.s. dollars", "dollars"]
        .iter()
        .find_map(|s| lower.strip_suffix(s).map(|_| &t[..t.len() - s.len()]))
        .unwrap_or(t)
        .trim();
    if t.starts_with('$') {
        t.to_string()
    } else {
        format!("${t}")
    }
}
/// Numeric dates, with letters OCR puts in place of digits (`02/03/0Q`).
static DATE_NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9OIlQ]{1,2}/[0-9OIlQ]{1,2}/[0-9OIlQ]{2,4}\b|\b\d{4}-\d{2}-\d{2}\b").unwrap()
});
static DATE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b(?:(?P<m1>(?i:{MONTHS}))\.?\s+(?P<d1>[0-9OIl]{{1,2}})(?i:st|nd|rd|th)?(?:,\s*|\s+)(?P<y1>[0-9OIlQ]{{4}})|(?P<d2>\d{{1,2}})(?i:st|nd|rd|th)?\s+(?i:day\s+of\s+|of\s+)?(?P<m2>(?i:{MONTHS}))\.?,?\s+(?P<y2>\d{{4}}))\b"
    ))
    .unwrap()
});
/// A hyphen ending a line, with the line break after it.
static LINE_END_HYPHEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-[ \t]*\r?\n\s*").unwrap());
/// An amount whose dollar sign was read as `S` (`S5,500`, `S12.50`).
static S_AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\bS(\d{1,3}(?:,\d{3})+(?:\.\d{2})?|\d+\.\d{2}|\d+(?:\.\d+)?\s(?i:thousand|million|billion|trillion))\b",
    )
    .unwrap()
});
/// A section sign that Vision's line recognizer read as `S`, `8` or `$`
/// (`(8 1983)`); second readings only. The number after it must still match exactly.
static SECOND_CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s(\[])([S8$]{1,2})\s?(\d+[a-z]?(?:[.\-]\d+)*(?:\([0-9a-zA-Z]{1,4}\))*)")
        .unwrap()
});
/// A paragraph sign that Vision's line recognizer read as `"1`, `9|`, `11`, `I` or `T`
/// (`Compl. "1 25-34`, `Id. 9| 34`); second readings only. The number must still match.
static SECOND_PILCROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[\s(\[])"?(?:[19IlT\]]{1,2})\|?\s(\d{1,4}(?:[-–]\d{1,4})?)\b"#).unwrap()
});
/// Federal docket numbers (`3:26-cv-04417`, `1:20-CV-05382-GBD-GWG`, `23-CV-886-PP`).
/// Look-alike letters are accepted in the number parts (`3:26-cv-O4417`) and flagged.
static DOCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:[\dOIl]{1,2}:)?[\dOIl]{2}-(?:cv|cr|mc|md|bk|ap|mj|po|sw|civ|cvd)-[\dOIlSB]{2,6}(?:-[A-Z]{1,4})*\b")
        .unwrap()
});
/// The parts of a docket number: office, year, type and number (judge initials are left out,
/// since filings append them inconsistently).
static DOCKET_PARTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(?P<office>\d{1,2}):)?(?P<year>\d{2})-(?P<kind>[a-z]+)-(?P<num>\d+)")
        .unwrap()
});
/// A case number after its label, in any court's format (`Case No.: 11-4604 PSG`,
/// `Case No. C-08-01337 MHP`, `C.A. No. 5215-VCG`, `Index No. 651234/2021`, `Case no. 20 Civ 1234`).
/// `Docket No. 35` and `Dkt. No.` cite a docket entry, not a case, and are left out.
static LABELED_DOCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:case|civil action|c\.a\.|index|cause|adv(?:ersary)?\.? (?:proc|no)\.?)\s*(?:no|number|#)\.?:?\s*(?P<n>(?-i:[A-Z]{0,3})[-:]?[\dOIl][\w:./-]*[\dOIl](?:\s(?:civ|cv|cr)\.?\s\d{1,6})?(?-i:[- ][A-Z]{2,4}\b)?)",
    )
    .unwrap()
});

/// Citations with a section or paragraph sign.
static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    // A dot inside a section number (`§ 52.1`), never a run of leader dots (`§ 1692.....12`).
    Regex::new(r"(?:§§?|¶¶?)\s?\d(?:[\w\-]|\.(?:[\w\-]))*(?:\([A-Za-z0-9]{1,4}\))*").unwrap()
});
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

/// An amount's sign and the rest: `-$100.00`, `$-100.00` and `($100.00)` are negative.
fn unsigned(text: &str) -> (bool, &str) {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return (true, inner.trim());
    }
    if let Some(rest) = t.strip_prefix('-').or_else(|| t.strip_prefix('\u{2212}')) {
        return (true, rest);
    }
    if let Some(rest) = t.strip_prefix("$-") {
        return (true, rest.trim_start_matches(' '));
    }
    if let Some(rest) = t.strip_prefix("$ -") {
        return (true, rest);
    }
    (false, t)
}

fn parse_amount(text: &str) -> Option<f64> {
    let (negative, text) = unsigned(text);
    let sign = if negative { -1.0 } else { 1.0 };
    let t = text.trim_start_matches('$').trim().to_ascii_lowercase();
    let (number, scale) = match t.split_once(' ') {
        Some((n, "thousand")) => (n, 1e3),
        Some((n, "million")) => (n, 1e6),
        Some((n, "billion")) => (n, 1e9),
        Some((n, "trillion")) => (n, 1e12),
        // `$1.5B`, `$10M`, `$250K`.
        _ => match t.char_indices().last() {
            Some((i, 'k')) if t[..i].ends_with(|c: char| c.is_ascii_digit()) => (&t[..i], 1e3),
            Some((i, 'm')) if t[..i].ends_with(|c: char| c.is_ascii_digit()) => (&t[..i], 1e6),
            Some((i, 'b')) if t[..i].ends_with(|c: char| c.is_ascii_digit()) => (&t[..i], 1e9),
            _ => (t.as_str(), 1.0),
        },
    };
    number
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .map(|v| sign * v * scale)
}

/// Letters and digits only, lowercased: how a value is looked for in embedded text, so spacing
/// and line breaks do not matter.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || "$§¶%".contains(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a numeric value holds a letter OCR substitutes for a digit (`O4417`, `$l,000`, `2O24`).
fn confusable(t: &str) -> bool {
    let mut core: String = unsigned(t)
        .1
        .trim_start_matches('$')
        .trim_start_matches('-')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .into();
    // A scale letter after a digit (`$1.5B`, `$10M`) is not a misread digit.
    if core.len() > 1
        && core.ends_with(['K', 'M', 'B'])
        && core[..core.len() - 1].ends_with(|c: char| c.is_ascii_digit())
    {
        core.pop();
    }
    core.chars().any(|c| c.is_ascii_digit()) && core.chars().any(|c| "OoIlSBQ".contains(c))
}

/// A field's value in canonical form, for comparing two readings: amounts in cents, dates as
/// YYYY-MM-DD, docket numbers without padding or judge initials, other kinds as their
/// characters. `None` when the text does not parse (it is then never verified).
fn key(kind: Kind, text: &str) -> Option<String> {
    match kind {
        Kind::Amount => {
            let (negative, t) = unsigned(text.trim_end_matches([',', '.']));
            let t = dollar_form(t);
            let t = t.as_str();
            // Cents beyond 2^53 are not exact in a float: never compared.
            (AMOUNT_OK.is_match(t) && !confusable(t))
                .then(|| parse_amount(t))
                .flatten()
                .filter(|v| (v * 100.0).abs() < 9.0e15)
                .map(|v| {
                    let c = (v * 100.0).round() as i64;
                    format!("amount:{}", if negative { -c } else { c })
                })
        }
        Kind::Date => parse_date(text).map(|d| format!("date:{d}")),
        Kind::Docket => {
            // Other courts' formats compare as their exact letters and digits.
            let Some(c) = DOCKET_PARTS.captures(text) else {
                return Some(format!("docket:other:{}", squash(text)));
            };
            let n: u64 = c["num"].parse().ok()?;
            let office = c.name("office").map_or("", |o| o.as_str());
            Some(format!(
                "docket:{office}:{}-{}-{n}",
                &c["year"],
                c["kind"].to_ascii_lowercase()
            ))
        }
        Kind::Percent => text
            .trim_end_matches('%')
            .trim()
            .replace('\u{2212}', "-")
            .parse::<f64>()
            .ok()
            .map(|v| format!("percent:{v}")),
        Kind::Citation | Kind::Bates => Some(format!("{kind:?}:{}", squash(text))),
    }
}

/// The canonical values found in a page's embedded text, by the same extraction as the OCR
/// text, so `$3,000` never matches inside `$3,000,000`. Bates numbers are matched as whole
/// tokens (one word, or two joined, for `ABC 0001234`).
/// Other keys a docket number in the reference text also matches: its letters and digits, with
/// an office number written before it with a hyphen (`5-16-cv-1128`, which the federal shape
/// reads as `16-cv-1128`), so an OCR reading of the same characters (`5-16-cv1128`) agrees.
fn docket_aliases(flat: &str, f: &Field) -> Vec<String> {
    if f.kind != Kind::Docket {
        return Vec::new();
    }
    let before = &flat[..f.col];
    let office: String = before
        .strip_suffix(['-', ':'])
        .map(|b| {
            b.chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .unwrap_or_default();
    let mut out = vec![format!("docket:other:{}", squash(&f.text))];
    if (1..=2).contains(&office.len()) {
        out.push(format!("docket:other:{office}{}", squash(&f.text)));
    }
    out
}

fn embedded_keys(text: &str) -> HashSet<String> {
    // Embedded text breaks lines anywhere (`January \r\n21, 2026`): read it as one line. A
    // line-end hyphen joins its halves (`5:15-` / `CV-01102`), and soft hyphens go.
    let text = LINE_END_HYPHEN.replace_all(text, "-");
    let text = text.replace(['\u{ad}', '\u{fffe}'], "");
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let fields = extract(&flat);
    let mut keys: HashSet<String> = fields.iter().filter_map(|f| key(f.kind, &f.text)).collect();
    keys.extend(fields.iter().flat_map(|f| docket_aliases(&flat, f)));
    // A docket number read without its office prefix (`19-cv-09902`) matches the embedded
    // one with it (`3:19-cv-09902`); a prefix that was read must still match exactly.
    let without_office: Vec<String> = keys
        .iter()
        .filter_map(|k| k.strip_prefix("docket:"))
        .filter_map(|k| k.split_once(':').map(|(_, rest)| format!("docket::{rest}")))
        .collect();
    keys.extend(without_office);
    // `S5,500`: a dollar sign read as `S`. Only with digit grouping or cents, so `S5` (a
    // section or exhibit) is not money.
    for c in S_AMOUNT.captures_iter(&flat) {
        if let Some(k) = key(Kind::Amount, &format!("${}", &c[1])) {
            keys.insert(k);
        }
    }
    let tokens: Vec<String> = text.split_whitespace().map(squash).collect();
    for (i, t) in tokens.iter().enumerate() {
        keys.insert(format!("Bates:{t}"));
        if let Some(next) = tokens.get(i + 1) {
            keys.insert(format!("Bates:{t}{next}"));
        }
    }
    keys
}

/// The fields of one page's Markdown, before any status is decided.
/// The words that continue the text after a line: those of the next line with text, at most one
/// blank line further, past pleading line numbers (`26`, `26 |`), which may stand on lines of
/// their own.
fn next_words<'a>(lines: &[&'a str]) -> impl Iterator<Item = &'a str> {
    let number = |w: &str| w.len() <= 2 && w.chars().all(|c| c.is_ascii_digit());
    let mut blank = 0;
    let mut found = None;
    for l in lines {
        let mut words = l.split_whitespace().peekable();
        if words.next_if(|w| number(w)).is_some() {
            words.next_if_eq(&"|");
        }
        if words.peek().is_some() {
            found = Some(words);
            break;
        }
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                break;
            }
        } else {
            // A line number alone starts a new line: the blank count starts again.
            blank = 0;
        }
    }
    found.into_iter().flatten()
}

/// Whether the text just before `at` makes the value there negative: a minus sign directly
/// before it (`-$100.00`, `-25%`) that is not part of a word or range (`10-25%`, `pre-$50`).
fn minus_before(line: &str, at: usize) -> bool {
    let before = &line[..at];
    let Some(c) = before.chars().next_back() else {
        return false;
    };
    if c != '-' && c != '\u{2212}' {
        return false;
    }
    !before[..before.len() - c.len_utf8()]
        .chars()
        .next_back()
        .is_some_and(|p| p.is_alphanumeric())
}

/// Whether the value spanning `range` fills a table cell in accounting brackets (`| ($20.00) |`),
/// a negative amount. Brackets in prose (`Five Thousand Dollars ($5,000.00)`) are not a sign.
fn accounting_cell(line: &str, range: std::ops::Range<usize>) -> bool {
    if !line.trim_start().starts_with('|') {
        return false;
    }
    let (before, after) = (
        line[..range.start].trim_end(),
        line[range.end..].trim_start(),
    );
    before.ends_with('(')
        && after.starts_with(')')
        && before[..before.len() - 1].trim_end().ends_with('|')
        && after[1..].trim_start().starts_with('|')
}

fn extract(body: &str) -> Vec<Field> {
    let mut out = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    for (i, &line) in lines.iter().enumerate() {
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
                        col: at,
                    },
                ));
            };
        for m in AMOUNT.find_iter(line) {
            let mut t = m.as_str().trim_end_matches([',', '.']).to_string();
            // A sign belongs to the amount: `-$100.00` and `($100.00)` in a table are a debit,
            // never the same value as `$100.00`.
            let mut start = m.start();
            if minus_before(line, start) {
                start -= line[..start].chars().next_back().map_or(0, char::len_utf8);
                t.insert(0, '-');
            } else if accounting_cell(line, m.start()..m.start() + t.len()) {
                t = format!("({t})");
            }
            // `$45` at the end of a line and `million` starting the next one (past a blank line
            // and a pleading line number, on the line or a line of its own): one amount.
            let next = next_words(&lines[i + 1..]).next();
            if let Some(w) = next.map(|w| w.trim_end_matches(|c: char| !c.is_alphanumeric()))
                && line[m.end()..].trim().is_empty()
                && !t.ends_with(|c: char| c.is_alphabetic())
                && ["thousand", "million", "billion", "trillion"]
                    .contains(&w.to_ascii_lowercase().as_str())
            {
                t = format!("{t} {w}");
            }
            let t = t.as_str();
            // `$Sales`, `$ISO`: no digit at all, not an amount.
            if !t.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            if confusable(t) {
                push(start, Kind::Amount, t, None, vec!["confusable_characters"]);
                continue;
            }
            let ok = AMOUNT_OK.is_match(unsigned(t).1);
            let value = parse_amount(t)
                .filter(|_| ok)
                .and_then(|v| serde_json::Number::from_f64(v).map(serde_json::Value::Number));
            push(
                start,
                Kind::Amount,
                t,
                value,
                if ok { vec![] } else { vec!["malformed_amount"] },
            );
        }
        // Money without a dollar sign, where no `$` amount is.
        let dollars: Vec<std::ops::Range<usize>> =
            AMOUNT.find_iter(line).map(|m| m.range()).collect();
        for (i, re) in BARE_AMOUNTS.iter().enumerate() {
            for c in re.captures_iter(line) {
                let m = if i == 2 {
                    c.name("n").unwrap()
                } else {
                    c.get(0).unwrap()
                };
                if dollars
                    .iter()
                    .any(|r| r.start < m.end() && m.start() < r.end)
                {
                    continue;
                }
                // `USD 1,00.00`: the number goes on past the match. Not `USD 1`, a malformed
                // amount.
                let rest = &line[m.end()..];
                let more = rest
                    .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
                    .unwrap_or(rest.len());
                if rest.starts_with([',', '.'])
                    && rest[1..].starts_with(|c: char| c.is_ascii_digit())
                {
                    let t = &line[m.start()..m.end() + more];
                    let t = t.trim_end_matches([',', '.']);
                    push(m.start(), Kind::Amount, t, None, vec!["malformed_amount"]);
                    continue;
                }
                let value = parse_amount(&dollar_form(m.as_str()))
                    .and_then(|v| serde_json::Number::from_f64(v).map(serde_json::Value::Number));
                push(m.start(), Kind::Amount, m.as_str(), value, vec![]);
            }
        }
        // Money without any `$` or word: invoices and money columns.
        let mut taken: Vec<std::ops::Range<usize>> = dollars.clone();
        for re in BARE_AMOUNTS.iter() {
            taken.extend(re.find_iter(line).map(|m| m.range()));
        }
        for (at, t) in unsigned_money(&lines, i) {
            if taken.iter().any(|r| r.start < at + t.len() && at < r.end) {
                continue;
            }
            let negative = t.starts_with('(') || t.starts_with('-');
            let digits = t.trim_matches(|c: char| "()-$ ".contains(c));
            let value = parse_amount(&format!("${digits}"))
                .map(|v| if negative { -v } else { v })
                .and_then(|v| serde_json::Number::from_f64(v).map(serde_json::Value::Number));
            push(at, Kind::Amount, &t, value, vec![]);
        }
        // `2 March 2023` at the start of a line is a pleading line number before `March 2023`.
        let indent = line.len() - line.trim_start().len();
        let mut dates: Vec<(usize, &str)> = DATE_WORDS
            .captures_iter(line)
            // An ordinal (`3rd day of March`) is never a line number.
            .filter(|c| {
                !c.name("d2").is_some_and(|d| {
                    d.start() == indent && line[d.end()..].starts_with(char::is_whitespace)
                })
            })
            .map(|c| c.get(0).unwrap())
            .chain(DATE_NUMERIC.find_iter(line))
            .map(|m| (m.start(), m.as_str()))
            .collect();
        dates.sort_unstable();
        dates.dedup();
        for (at, t) in dates {
            match parse_date(t) {
                Some(iso) => push(at, Kind::Date, t, Some(iso.into()), vec![]),
                None if DATE_NUMERIC.is_match(t) && confusable(t) => {
                    push(at, Kind::Date, t, None, vec!["confusable_characters"])
                }
                None => push(at, Kind::Date, t, None, vec!["invalid_date"]),
            }
        }
        for m in DOCKET.find_iter(line) {
            let numbers = m.as_str().split('-').enumerate().filter(|(i, _)| *i != 1);
            let why = if numbers.take(2).any(|(_, p)| confusable(p)) {
                vec!["confusable_characters"]
            } else {
                vec![]
            };
            push(m.start(), Kind::Docket, m.as_str(), None, why);
        }
        // Case numbers by label, where the federal shape above did not already find them.
        let federal: Vec<std::ops::Range<usize>> =
            DOCKET.find_iter(line).map(|m| m.range()).collect();
        for c in LABELED_DOCKET.captures_iter(line) {
            let n = c.name("n").unwrap();
            if federal
                .iter()
                .any(|r| r.start < n.end() && n.start() < r.end)
            {
                continue;
            }
            // Cut off at the line end (`Case No. 03:11-`): a fragment is not a case number.
            if line[n.end()..].starts_with(['-', ':', '/']) && line[n.end() + 1..].trim().is_empty()
            {
                continue;
            }
            let t = n.as_str().trim_end_matches(['.', ',', '-', '/', ':']);
            if t.chars().filter(|c| c.is_ascii_digit()).count() < 4 {
                continue;
            }
            // A digit run with a look-alike letter in it (`C-O8-01337`).
            let why = if t
                .split(|c: char| !c.is_alphanumeric())
                .any(|part| part.chars().any(|c| c.is_ascii_digit()) && confusable(part))
            {
                vec!["confusable_characters"]
            } else {
                vec![]
            };
            push(n.start(), Kind::Docket, t, None, why);
        }
        for m in CITATION.find_iter(line) {
            let t = m.as_str().trim_end_matches(['.', ',', '-']);
            push(m.start(), Kind::Citation, t, None, vec![]);
        }
        for m in PERCENT.find_iter(line) {
            if minus_before(line, m.start()) {
                let start = m.start()
                    - line[..m.start()]
                        .chars()
                        .next_back()
                        .map_or(1, char::len_utf8);
                push(
                    start,
                    Kind::Percent,
                    &format!("-{}", m.as_str()),
                    None,
                    vec![],
                );
            } else {
                push(m.start(), Kind::Percent, m.as_str(), None, vec![]);
            }
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

/// One value in a reference reading (embedded text, second reading): its keys and the words
/// before it, so each OCR value is matched to its own occurrence, not to any on the page.
#[derive(Debug, Clone)]
struct Occurrence {
    keys: Vec<String>,
    context: Vec<String>,
    used: bool,
    /// The value as the reference writes it.
    text: String,
}

/// Up to two words before a value, back to the value before it, letters only and lowercased
/// (`| Principal: |` gives `principal`); pleading line numbers and punctuation are skipped.
fn context_words(before: &str) -> Vec<String> {
    let number = |w: &str| (1..=2).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit());
    before
        .split_whitespace()
        .rev()
        .filter(|w| !number(w))
        .take_while(|w| !w.chars().any(|c| c.is_ascii_digit()))
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphabetic())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| w.chars().count() >= 2)
        .take(2)
        .collect()
}

/// The words before an OCR value: on its line, or the line above when it starts its line.
fn field_context(lines: &[&str], f: &Field) -> Vec<String> {
    let Some(line) = f.line.checked_sub(1).and_then(|i| lines.get(i)) else {
        return vec![];
    };
    let words = context_words(&line[..f.col.min(line.len())]);
    if !words.is_empty() {
        return words;
    }
    lines[..f.line - 1]
        .iter()
        .rev()
        .find(|l| !context_words(l).is_empty())
        .map(|l| context_words(l))
        .unwrap_or_default()
}

/// Two context words that are the same, allowing one misread letter in a long word.
fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().min(b.len()) < 5 || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    if a.len() == b.len() {
        return a.iter().zip(&b).filter(|(x, y)| x != y).count() <= 1;
    }
    close(&a.iter().collect::<String>(), &b.iter().collect::<String>())
}

/// The values of a reference text, in reading order, by the same extraction as the OCR.
fn occurrences(text: &str) -> Vec<Occurrence> {
    let text = LINE_END_HYPHEN.replace_all(text, "-");
    let text = text.replace(['\u{ad}', '\u{fffe}'], "");
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: Vec<Occurrence> = extract(&flat)
        .iter()
        .filter_map(|f| {
            let k = key(f.kind, &f.text)?;
            let mut keys = vec![k.clone()];
            // A docket read without its office prefix matches this one with it.
            if let Some((_, rest)) = k.strip_prefix("docket:").and_then(|r| r.split_once(':')) {
                keys.push(format!("docket::{rest}"));
            }
            keys.extend(docket_aliases(&flat, f));
            Some(Occurrence {
                keys,
                context: context_words(&flat[..f.col]),
                used: false,
                text: f.text.clone(),
            })
        })
        .collect();
    for c in S_AMOUNT.captures_iter(&flat) {
        if let Some(k) = key(Kind::Amount, &format!("${}", &c[1])) {
            out.push(Occurrence {
                keys: vec![k],
                context: context_words(&flat[..c.get(0).unwrap().start()]),
                used: false,
                text: c[0].to_string(),
            });
        }
    }
    out
}

/// Matches an OCR value to an unused occurrence of the same value in the reference, with the
/// same words before it. `Err` says why not: the reference has the value fewer times than the
/// OCR (`$3,000` twice against once), or only after other words (`Principal: $3,000` against
/// `Interest: $3,000`, values swapped between labels). A value the reference has only by a
/// rule outside the extraction (a look-alike sign) is not checked here.
fn assign(pool: &mut [Occurrence], k: &str, context: &[String]) -> Result<(), &'static str> {
    if !pool.iter().any(|o| o.keys.iter().any(|x| x == k)) {
        return Ok(());
    }
    let agree = |o: &Occurrence| {
        context.is_empty()
            || o.context.is_empty()
            || context
                .iter()
                .any(|a| o.context.iter().any(|b| similar(a, b)))
    };
    let mut free = pool
        .iter_mut()
        .filter(|o| !o.used && o.keys.iter().any(|x| x == k))
        .peekable();
    if free.peek().is_none() {
        return Err("more_often_than_in_reference");
    }
    match free.find(|o| agree(o)) {
        Some(o) => {
            o.used = true;
            Ok(())
        }
        None => Err("label_differs"),
    }
}

/// A page's embedded PDF text, and whether it is drawn invisibly (an OCR layer over a scan).
#[derive(Debug, Clone, Default)]
pub struct Embedded {
    pub text: String,
    pub visibility: TextVisibility,
    /// Kinds of the visible annotations (form values, stamps, notes, redaction boxes). No OCR
    /// engine sees them: the renderers draw page content only.
    pub annotations: Vec<String>,
    /// The annotation detector failed: unknown, treated as not read.
    pub annotations_unknown: bool,
}

/// What a second reading of a page says about values: the keys it read, and the keys it was
/// unsure of (a next-best reading of the line reads the value differently, or low confidence).
struct SecondReading {
    /// The reading's text, `§`/`¶` restored, line numbers dropped.
    text: String,
    keys: HashSet<String>,
    doubtful: HashSet<String>,
    occurrences: Vec<Occurrence>,
}

fn second_reading(lines: &[Line]) -> SecondReading {
    // Pleading line numbers come between a page's text lines (`May 7,` `6` `10` `2007.`) or
    // start them (`11 2022, Kuzdas wrote`).
    let number = |t: &str| (1..=2).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit());
    let all = lines
        .iter()
        .map(|l| l.text.trim())
        .filter(|t| !number(t))
        .map(|t| match t.split_once(' ') {
            // `14 | 2022, Kuzdas`: a line number, and the bar of the margin rule.
            Some((first, rest)) if number(first) => rest.strip_prefix("| ").unwrap_or(rest),
            _ => t,
        })
        .collect::<Vec<_>>();
    // The first reading had its `§`/`¶` restored at assembly; restore the second one alike, so
    // `S 1983` is not taken for a disagreement with `§ 1983`.
    let text = crate::markdown::fix_legal_symbols(&all.join("\n")).0;
    let mut keys = embedded_keys(&text);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for c in SECOND_CITATION.captures_iter(&flat) {
        let signs = "§".repeat(c[1].len());
        keys.insert(format!("{:?}:{signs}{}", Kind::Citation, squash(&c[2])));
    }
    for c in SECOND_PILCROW.captures_iter(&flat) {
        for signs in ["¶", "¶¶"] {
            keys.insert(format!("{:?}:{signs}{}", Kind::Citation, squash(&c[1])));
        }
    }
    let mut doubtful = HashSet::new();
    let keys_of = |t: &str| embedded_keys(&crate::markdown::fix_legal_symbols(t).0);
    // No confidence is no better than a low one.
    let unsure = |l: &Line| {
        l.confidence
            .is_none_or(|c| c < SECOND_READER_MIN_CONFIDENCE)
    };
    for l in lines {
        let mine = keys_of(&l.text);
        if unsure(l) {
            doubtful.extend(mine);
            continue;
        }
        for alt in &l.alternatives {
            doubtful.extend(mine.difference(&keys_of(alt)).cloned());
        }
    }
    // A value split over two lines (`January 1,` / `2024`) is as doubtful as either line.
    for pair in lines.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let joined = keys_of(&format!("{}\n{}", a.text, b.text));
        let (ka, kb) = (keys_of(&a.text), keys_of(&b.text));
        let spanning: Vec<String> = joined
            .into_iter()
            .filter(|k| !ka.contains(k) && !kb.contains(k))
            .collect();
        if spanning.is_empty() {
            continue;
        }
        if unsure(a) || unsure(b) {
            doubtful.extend(spanning);
            continue;
        }
        let alts = a
            .alternatives
            .iter()
            .map(|x| format!("{x}\n{}", b.text))
            .chain(b.alternatives.iter().map(|x| format!("{}\n{x}", a.text)));
        for alt in alts {
            let theirs = keys_of(&alt);
            doubtful.extend(spanning.iter().filter(|k| !theirs.contains(*k)).cloned());
        }
    }
    let occurrences = occurrences(&text);
    SecondReading {
        text,
        keys,
        doubtful,
        occurrences,
    }
}

/// Second readings of `which` (indexes into `pages`), two at a time: the bridge runs at most
/// two Vision requests at once.
fn reread_all(
    which: &[usize],
    pages: &[PageResult],
    second: &(dyn Fn(usize) -> Option<Vec<Line>> + Sync),
) -> HashMap<usize, SecondReading> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..2.min(which.len()) {
            scope.spawn(|| {
                loop {
                    let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&i) = which.get(n) else { break };
                    if let Some(lines) = second(pages[i].number) {
                        let r = second_reading(&lines);
                        done.lock().unwrap().insert(i, r);
                    }
                }
            });
        }
    });
    done.into_inner().unwrap()
}

/// Builds the report. `embedded(page)` returns the page's embedded PDF text (empty for none).
/// `second(page)` returns a second, independent line-level reading of the page, if one can be
/// made; it is asked for only on pages without embedded text that have values to confirm.
/// The second reader is Apple Vision. Its agreement verifies a value only when the first reading
/// came from a model (`model_engine`, page not fallen back to Apple): two Apple readings share
/// blind spots (docs/spike.md), so on Apple pages it can only find disagreements.
#[allow(clippy::too_many_arguments)]
pub fn report<'a>(
    source: &'a str,
    markdown: String,
    engine: &'a str,
    generated: &'a str,
    pages: &[PageResult],
    mode: VerifyFields,
    embedded: &dyn Fn(usize) -> Embedded,
    second: &(dyn Fn(usize) -> Option<Vec<Line>> + Sync),
    model_engine: bool,
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

    let layers: Vec<Embedded> = pages
        .iter()
        .map(|p| {
            if p.source == Source::TextLayer || p.error.is_some() {
                Embedded::default()
            } else {
                embedded(p.number)
            }
        })
        .collect();
    let texts: Vec<&str> = layers.iter().map(|l| l.text.as_str()).collect();
    let has_text: Vec<bool> = texts
        .iter()
        .map(|t| t.split_whitespace().count() >= 20)
        .collect();
    // Pages to re-read: no embedded text, and text a second reading could confirm.
    let reread_pages: Vec<usize> = pages
        .iter()
        .zip(&extracted)
        .enumerate()
        .filter(|(i, (p, _))| {
            !has_text[*i]
                && p.source != Source::TextLayer
                && p.error.is_none()
                && (p.bates.is_some() || !p.body.trim().is_empty())
        })
        .map(|(i, _)| i)
        .collect();
    let rereads = reread_all(&reread_pages, pages, second);
    let mut out = Vec::new();
    for (i, (p, (number, mut fields))) in pages.iter().zip(extracted).enumerate() {
        let mut page_check: Vec<String> = p.suspect.iter().map(|s| s.to_string()).collect();
        if let Some(e) = &p.error {
            page_check.push(format!("failed: {e}"));
        }
        let annotations = layers[i].annotations.clone();
        let mut annotation_reason = None;
        if !annotations.is_empty() {
            annotation_reason = Some("annotations_not_read");
        } else if layers[i].annotations_unknown {
            annotation_reason = Some("annotations_unknown");
        }
        if let Some(r) = annotation_reason
            && !page_check.iter().any(|c| c == r)
        {
            page_check.push(r.into());
        }
        // What the engine itself flagged: the model failed and Apple's text stands in, a crop
        // came back empty or a reply was cut short, or a transcript's line numbers could not be
        // re-anchored to the page.
        if p.fallback {
            page_check.push("engine_fallback".into());
        } else if p.flagged && p.error.is_none() {
            page_check.push("engine_flagged".into());
        }
        if p.line_numbers == Some(false) {
            page_check.push("line_numbers_unverified".into());
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
                    col: 0,
                },
            );
        }
        let has_text = has_text[i];
        let visibility = layers[i].visibility;
        let ocr_layer = has_text && visibility == TextVisibility::OcrLayer;
        let unknown_layer = has_text && visibility == TextVisibility::Unknown;
        // Neither reads as the document's own characters: agreement verifies nothing.
        let not_own = ocr_layer || unknown_layer;
        let keys = if has_text {
            embedded_keys(texts[i])
        } else {
            HashSet::new()
        };
        let reread = rereads.get(&i);
        // Each reference occurrence verifies one OCR value, after the same label.
        let mut embedded_pool = if has_text && !ocr_layer && !unknown_layer {
            occurrences(texts[i])
        } else {
            Vec::new()
        };
        let mut second_pool = reread.map(|r| r.occurrences.clone()).unwrap_or_default();
        let body_lines: Vec<&str> = p.body.lines().collect();
        let text_agreement = if has_text {
            Some(if not_own {
                let name = if ocr_layer {
                    "ocr_text_layer"
                } else {
                    "unknown_text_layer"
                };
                crate::agreement::compare(&p.body, texts[i], name, false)
            } else {
                crate::agreement::compare(&p.body, texts[i], "embedded_text", true)
            })
        } else {
            reread.map(|r| {
                crate::agreement::compare(
                    &p.body,
                    &r.text,
                    "second_reader",
                    model_engine && !p.fallback,
                )
            })
        };
        // Amounts in words against numerals, and table totals against their rows.
        let mismatches = if p.error.is_none() {
            crate::arithmetic::check(&p.body)
        } else {
            Vec::new()
        };
        for m in &mismatches {
            if !page_check.iter().any(|c| c == m.reason) {
                page_check.push(m.reason.into());
            }
        }
        // Every amount in a disagreeing sum, the items above a total too.
        for f in fields.iter_mut().filter(|f| f.kind == Kind::Amount) {
            if let Some(m) = mismatches
                .iter()
                .find(|m| m.cells.iter().any(|(l, a)| *l == f.line && *a == f.text))
            {
                f.why.push(m.reason);
            }
        }
        // The other reading has whole lines the OCR lacks: dropped text, or text not visible on
        // the page. Either way the page needs a look.
        if text_agreement
            .as_ref()
            .is_some_and(|a| a.missing_words >= MISSING_TEXT_MIN_WORDS)
        {
            page_check.push("missing_text".into());
        }
        // A number the other reading has and the OCR lacks: a dropped deadline, amount or date,
        // however short its line.
        if text_agreement
            .as_ref()
            .is_some_and(|a| !a.missing_numbers.is_empty())
        {
            page_check.push("missing_number".into());
        }
        // Values the other reading has and the OCR has nowhere on the page: a dropped total or
        // deadline is not a wrong value, it is a missing one. Listed always; in `all` mode the
        // page is checked.
        let ocr_keys: HashSet<String> =
            fields.iter().filter_map(|f| key(f.kind, &f.text)).collect();
        let reference = if embedded_pool.is_empty() {
            &second_pool
        } else {
            &embedded_pool
        };
        let mut expected_fields_missing: Vec<String> = Vec::new();
        for o in reference {
            if !o.keys.iter().any(|k| ocr_keys.contains(k))
                && !expected_fields_missing.contains(&o.text)
            {
                expected_fields_missing.push(o.text.clone());
            }
        }
        if mode == VerifyFields::All && !expected_fields_missing.is_empty() {
            page_check.push("fields_missing".into());
        }
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
            // Agreement of two readings cannot clear a page whose annotations neither saw.
            if let Some(r) = annotation_reason {
                f.why.push(r);
            }
            let k = key(f.kind, &f.text);
            let mut found = k.as_ref().is_some_and(|k| keys.contains(k));
            if not_own {
                // Another OCR's reading, or text of unknown visibility: a disagreement counts,
                // an agreement verifies nothing.
                if !found && k.is_some() {
                    f.why.push(if ocr_layer {
                        "not_in_ocr_text_layer"
                    } else {
                        "text_layer_unknown"
                    });
                }
                found = false;
            } else if has_text && !found {
                f.why.push("not_in_embedded_text");
            }
            let mut matched = "matches_embedded_text";
            if let (Some(r), Some(k)) = (reread, &k) {
                if r.doubtful.contains(k) {
                    f.why.push("doubtful_in_second_reading");
                } else if r.keys.contains(k) {
                    // Two image readers agreeing on the court's filing stamp confirms nothing:
                    // they misread its overprints alike. The PDF's own text or Azure can.
                    let on_stamp = f
                        .line
                        .checked_sub(1)
                        .and_then(|i| body_lines.get(i))
                        .is_some_and(|l| crate::markdown::COURT_STAMP.is_match(l));
                    if on_stamp {
                        f.why.push("court_stamp");
                    }
                    // A model page's Bates number comes from Apple Vision's side pass, so the
                    // Apple second reading is the same reader again: it confirms nothing.
                    else if model_engine && !p.fallback && f.kind != Kind::Bates {
                        found = true;
                        matched = "matches_second_reading";
                    }
                } else {
                    f.why.push("not_in_second_reading");
                }
            }
            // The value is in the reference, but is it this occurrence of it?
            if found
                && f.kind != Kind::Bates
                && let Some(k) = &k
            {
                let pool = if matched == "matches_embedded_text" {
                    &mut embedded_pool
                } else {
                    &mut second_pool
                };
                if let Err(why) = assign(pool, k, &field_context(&body_lines, f)) {
                    found = false;
                    f.why.push(why);
                }
            }
            // Two independent readings of the exact value agree: that outweighs a suspect page.
            let page_only = f.why.iter().all(|w| *w == "suspect_page");
            let against = f.why.iter().any(|w| *w != "from_embedded_text");
            f.status = if found && page_only {
                f.why.clear();
                f.why.push(matched);
                Status::Verified
            } else if against {
                Status::Check
            } else if found {
                f.why.push(matched);
                Status::Verified
            } else if mode == VerifyFields::All && p.source != Source::TextLayer {
                f.why.push("unconfirmed");
                Status::Check
            } else {
                Status::Unchecked
            };
        }
        let field_check = fields.iter().any(|f| f.status == Status::Check);
        // The PDF's own text does not include annotations: only a reader of the page image does.
        let second_reading = if field_check || annotation_reason.is_some() {
            Some("azure")
        } else if page_check.is_empty() {
            None
        } else if has_text && !not_own {
            Some("embedded_text")
        } else {
            Some("azure")
        };
        out.push(PageReport {
            page: number,
            bates: p.bates.clone(),
            page_check,
            annotations,
            embedded_text: has_text,
            ocr_text_layer: ocr_layer,
            text_layer_visibility: visibility.as_str(),
            second_reader: reread.is_some(),
            text_agreement,
            second_reading,
            fields,
            expected_fields_missing,
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
    for a in out
        .iter()
        .filter_map(|p| p.text_agreement.as_ref())
        .filter(|a| a.independent)
    {
        summary.agreed_spans += a.agreed_spans;
        summary.unagreed_spans += a.unagreed.len();
    }
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
        report(
            "x.pdf",
            "x.md".into(),
            "apple",
            "now",
            pages,
            mode,
            &|_| Embedded {
                text: text.clone(),
                visibility: TextVisibility::Visible,
                ..Embedded::default()
            },
            &|_| None,
            false,
        )
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
                ("02/03/0Q", vec!["confusable_characters"]),
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
            &[page(1, &layer.replace("$122,525.50", "$3,000"))],
            VerifyFields::Flagged,
            &layer.replace("$122,525.50", "$8,000"),
        );
        assert_eq!(statuses(&r)[0].1, Status::Check);
        assert_eq!(statuses(&r)[0].2, ["not_in_embedded_text"]);
        assert_eq!(r.azure_pages, "1");
        // The OCR lacks a whole line of the embedded text: the page is checked.
        let r = run(
            &[page(
                1,
                "The total due is $122,525.50, plus interest, as of 06/01/2024.",
            )],
            VerifyFields::Flagged,
            "The total due is $122,525.50, plus interest, as of 06/01/2024.\nThe parties \
             shall appear for a case management conference on the date the Court sets.",
        );
        assert_eq!(r.pages[0].page_check, ["missing_text"]);
        assert_eq!(statuses(&r)[0].1, Status::Verified);
        assert_eq!(r.azure_pages, "");
        assert_eq!(r.pages[0].second_reading, Some("embedded_text"));
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
        let f = extract("Losses of $1.5B and $250K, but not $1,5OO.");
        let got: Vec<(&str, Option<serde_json::Value>, Vec<&str>)> = f
            .iter()
            .map(|f| (f.text.as_str(), f.value.clone(), f.why.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("$1.5B", Some(serde_json::json!(1_500_000_000.0)), vec![]),
                ("$250K", Some(serde_json::json!(250_000.0)), vec![]),
                ("$1,5OO", None, vec!["confusable_characters"]),
            ]
        );
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

    const LAYER: &str = "On March 3, 2024 the parties agreed to pay $3,000,000 under case \
        3:26-cv-04417-SI, with 12.5% interest, invoice dated 04/15/2024 for $48,250.00, \
        citing 28 U.S.C. § 1441(b), Bates ABC 0001234 on the first page of the production.";

    #[test]
    fn scale_word_on_the_next_line_is_part_of_the_amount() {
        // A `B` scale after broken grouping stays malformed, never a value.
        let f = extract("Loss of $1,50B.");
        assert_eq!(
            (f[0].value.clone(), f[0].why.clone()),
            (None, vec!["malformed_amount"])
        );
        let f = extract("settled for $45\nMillion. Then $3\nmore.");
        assert_eq!(f[0].text, "$45 Million");
        assert_eq!(f[0].value, Some(serde_json::json!(45000000.0)));
        assert_eq!(f[1].text, "$3");
        assert_eq!(f[0].line, 1);
        // Across a blank line, past a pleading line number, before a closing quote.
        let f = extract("commitments to the entire $400\n\n26 million\u{201d} from investors");
        assert_eq!(f[0].text, "$400 million");
        // Not across a whole paragraph, and not from the middle of the next line.
        assert_eq!(extract("for $400\n\n\nmillion")[0].text, "$400");
        assert_eq!(extract("for $400\nthe million")[0].text, "$400");
        // A pleading line number on a line of its own, or before a `|`.
        let f = extract("securities laws for $45\n\n16\n\nmillion.");
        assert_eq!(f[0].text, "$45 million");
        assert_eq!(extract("for $45\n\n16 | million.")[0].text, "$45 million");
        assert_eq!(extract("for $45\n\n16\n\n\nmillion.")[0].text, "$45");
    }

    fn read(text: &str) -> Line {
        Line {
            text: text.into(),
            bbox: [0.0; 4],
            confidence: Some(1.0),
            alternatives: vec![],
        }
    }

    fn read_at(text: &str, confidence: Option<f64>) -> Line {
        Line {
            confidence,
            ..read(text)
        }
    }

    fn second(pages: &[PageResult], lines: Vec<Line>) -> Report<'static> {
        report(
            "x.pdf",
            "x.md".into(),
            "model",
            "now",
            pages,
            VerifyFields::Flagged,
            &|_| Embedded::default(),
            &|_| Some(lines.clone()),
            true,
        )
    }

    #[test]
    fn a_value_split_over_doubtful_lines_is_doubtful() {
        let pages = [page(1, "The note is due on January 1, 2024 at noon.")];
        let r = second(
            &pages,
            vec![
                read_at("The note is due on January 1,", Some(0.1)),
                read_at("2024 at noon.", Some(0.1)),
            ],
        );
        let s = statuses(&r);
        assert_eq!(s[0].1, Status::Check, "{s:?}");
        assert!(s[0].2.contains(&"doubtful_in_second_reading"), "{s:?}");
        // Sure on both lines, it verifies.
        let r = second(
            &pages,
            vec![
                read_at("The note is due on January 1,", Some(1.0)),
                read_at("2024 at noon.", Some(1.0)),
            ],
        );
        assert_eq!(statuses(&r)[0].1, Status::Verified, "{:?}", statuses(&r));
        // No confidence is not sure.
        let r = second(
            &pages,
            vec![read_at("The note is due on January 1, 2024 at noon.", None)],
        );
        assert_eq!(statuses(&r)[0].1, Status::Check, "{:?}", statuses(&r));
    }

    #[test]
    fn a_model_page_bates_number_is_not_verified_by_apple_again() {
        let mut p = page(1, "Exhibit A to the declaration of counsel.");
        p.bates = Some("ABC0001234".into());
        let r = second(
            &[p],
            vec![
                read("Exhibit A to the declaration of counsel."),
                read("ABC0001234"),
            ],
        );
        let s = statuses(&r);
        assert_eq!(s[0].0, "ABC0001234");
        assert_ne!(s[0].1, Status::Verified, "{s:?}");
    }

    #[test]
    fn engine_flags_send_the_page_for_a_second_reading() {
        for (fallback, flagged, line_numbers, why) in [
            (true, true, None, "engine_fallback"),
            (false, true, None, "engine_flagged"),
            (false, false, Some(false), "line_numbers_unverified"),
        ] {
            let mut p = page(1, "Readable prose with no values in it at all.");
            (p.fallback, p.flagged, p.line_numbers) = (fallback, flagged, line_numbers);
            let r = run(&[p], VerifyFields::All, "");
            assert!(
                r.pages[0].page_check.iter().any(|c| c == why),
                "{why}: {:?}",
                r.pages[0].page_check
            );
            assert_eq!(r.azure_pages, "1", "{why}");
        }
    }

    #[test]
    fn filing_stamp_values_are_not_verified_by_two_image_readers() {
        // The Jane Street stamp: `04/24/24` printed over `04/18/24`, read `04/28/24` by both.
        let stamp = "Case 1:24-cv-01234-JPO Document 25 Filed 04/28/24 Page 1 of 30";
        let body = format!("{stamp}\nThe motion was argued on March 4, 2024 before the court.");
        let lines = vec![
            read(stamp),
            read("The motion was argued on March 4, 2024 before the court."),
        ];
        let r = report(
            "x.pdf",
            "x.md".into(),
            "model",
            "now",
            &[page(1, &body)],
            VerifyFields::Flagged,
            &|_| Embedded::default(),
            &|_| Some(lines.clone()),
            true,
        );
        let s = statuses(&r);
        let status = |t: &str| s.iter().find(|g| g.0 == t).unwrap().clone();
        assert_eq!(status("04/28/24").1, Status::Check, "{s:?}");
        assert!(status("04/28/24").2.contains(&"court_stamp"), "{s:?}");
        assert_eq!(status("1:24-cv-01234-JPO").1, Status::Check, "{s:?}");
        // Off the stamp, agreement still verifies.
        assert_eq!(status("March 4, 2024").1, Status::Verified, "{s:?}");
        // The PDF's own text still verifies stamp values.
        let layer = format!(
            "Case 1:24-cv-01234-JPO Document 25 Filed 04/28/24 Page 1 of 30 The motion \
            was argued on March 4, 2024 before the court.{PAD}"
        );
        let r = run(
            &[page(1, &format!("{body}{PAD}"))],
            VerifyFields::Flagged,
            &layer,
        );
        let s = statuses(&r);
        assert_eq!(
            s.iter().find(|g| g.0 == "04/28/24").unwrap().1,
            Status::Verified,
            "{s:?}"
        );
    }

    #[test]
    fn a_second_reading_confirms_or_contradicts_values_on_scans() {
        let body = "Paid $5,500 on January 11, 2008 and $48,250.00 on May 7, 2007 in \
            3:08-cv-09901. See 42 U.S.C. § 1983.\nFourth Amendment (§ 1985) and $1 million in \
            shares on March 4, 2021. Compl. ¶¶ 25-34. Id. ¶ 34. In another letter dated \
            August 16, 2022, Kuzdas reminded investors.";
        let pages = [page(1, body)];
        let lines = vec![
            read("Paid S5,500 on January 11,2008 and $48,256.00 on May 7,"),
            read("6"),
            read("10"),
            read("2007. in 3:08-cv-09901. See 42 U.S.C. S 1983."),
            read("11 Fourth Amendment (8 1985) and S1 million in shares on March"),
            read("12 4, 2021. Compl. \"1 25-34. Id. 9| 34. In another letter dated August 16,"),
            read("14 | 2022, Kuzdas reminded investors."),
        ];
        let r = report(
            "x.pdf",
            "x.md".into(),
            "apple",
            "now",
            &pages,
            VerifyFields::Flagged,
            &|_| Embedded::default(),
            &|_| Some(lines.clone()),
            true,
        );
        assert!(r.pages[0].second_reader);
        let got = statuses(&r);
        let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
        for t in [
            "$5,500",
            "January 11, 2008",
            "May 7, 2007",
            "3:08-cv-09901",
            "§ 1983",
            "§ 1985",
            "$1 million",
            "March 4, 2021",
            "¶¶ 25-34",
            "¶ 34",
            "August 16, 2022",
        ] {
            assert_eq!(status(t).1, Status::Verified, "{t} {:?}", status(t));
            assert_eq!(status(t).2, vec!["matches_second_reading"], "{t}");
        }
        assert_eq!(
            status("$48,250.00"),
            (
                "$48,250.00".into(),
                Status::Check,
                vec!["not_in_second_reading"]
            )
        );
        assert_eq!(r.azure_pages, "1");

        // A line Vision doubts confirms nothing; nor does a next-best reading that differs.
        let doubtful = vec![
            Line {
                confidence: Some(0.3),
                ..read("Paid $5,500 on January 11, 2008")
            },
            Line {
                alternatives: vec!["and $48,256.00 on May 7, 2007 in 3:08-cv-09901.".into()],
                ..read("and $48,250.00 on May 7, 2007 in 3:08-cv-09901.")
            },
        ];
        let r = report(
            "x.pdf",
            "x.md".into(),
            "apple",
            "now",
            &pages,
            VerifyFields::Flagged,
            &|_| Embedded::default(),
            &|_| Some(doubtful.clone()),
            true,
        );
        let got = statuses(&r);
        let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
        for t in ["$5,500", "January 11, 2008", "$48,250.00"] {
            assert_eq!(status(t).1, Status::Check, "{t}");
            assert_eq!(status(t).2, vec!["doubtful_in_second_reading"], "{t}");
        }
        assert_eq!(status("May 7, 2007").1, Status::Verified);

        // Pages with embedded text are not re-read.
        let r = report(
            "x.pdf",
            "x.md".into(),
            "apple",
            "now",
            &pages,
            VerifyFields::Flagged,
            &|_| Embedded {
                text: body.repeat(3),
                visibility: TextVisibility::Visible,
                ..Embedded::default()
            },
            &|_| panic!("re-read a page with embedded text"),
            true,
        );
        assert!(!r.pages[0].second_reader);

        // Apple read the page: a second Apple reading that agrees verifies nothing (shared blind
        // spots), but one that disagrees still flags the value. Same for a model page that fell
        // back to Apple.
        let mut fell_back = page(1, body);
        fell_back.fallback = true;
        for (pages, model) in [([page(1, body)], false), ([fell_back], true)] {
            let r = report(
                "x.pdf",
                "x.md".into(),
                "apple",
                "now",
                &pages,
                VerifyFields::Flagged,
                &|_| Embedded::default(),
                &|_| Some(lines.clone()),
                model,
            );
            let got = statuses(&r);
            let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
            // Nothing is verified. A fallen-back page is checked as a whole (`engine_fallback`).
            if model {
                assert_eq!(status("$5,500").1, Status::Check);
                assert_eq!(status("$5,500").2, vec!["suspect_page"]);
                assert!(
                    r.pages[0]
                        .page_check
                        .contains(&"engine_fallback".to_string())
                );
            } else {
                assert_eq!(status("$5,500").1, Status::Unchecked);
                assert!(status("$5,500").2.is_empty());
            }
            assert_eq!(status("$48,250.00").1, Status::Check);
            assert!(status("$48,250.00").2.contains(&"not_in_second_reading"));
        }
    }

    #[test]
    fn annotations_make_the_page_unread_and_unknown_fails_closed() {
        let body = "Pay $9,999.00 by May 7, 2024.";
        let go = |annotations: Vec<String>, annotations_unknown: bool| {
            report(
                "x.pdf",
                "x.md".into(),
                "apple",
                "now",
                &[page(1, body)],
                VerifyFields::Flagged,
                &|_| Embedded {
                    annotations: annotations.clone(),
                    annotations_unknown,
                    ..Embedded::default()
                },
                &|_| None,
                false,
            )
        };
        let r = go(vec!["stamp".into(), "widget".into()], false);
        assert_eq!(r.pages[0].page_check, ["annotations_not_read"]);
        assert_eq!(r.pages[0].annotations, ["stamp", "widget"]);
        assert_eq!(r.azure_pages, "1");
        assert!(
            r.pages[0]
                .fields
                .iter()
                .all(|f| f.why.contains(&"suspect_page") && f.status == Status::Check)
        );
        assert!(
            serde_json::to_string(&r)
                .unwrap()
                .contains("\"annotations\":[\"stamp\"")
        );

        let r = go(vec![], true);
        assert_eq!(r.pages[0].page_check, ["annotations_unknown"]);
        assert_eq!(r.azure_pages, "1");

        let r = go(vec![], false);
        assert!(r.pages[0].page_check.is_empty());
        assert!(r.pages[0].annotations.is_empty());
        assert!(
            !serde_json::to_string(&r)
                .unwrap()
                .contains("\"annotations\"")
        );
    }

    #[test]
    fn arithmetic_mismatches_check_the_page_and_the_amount() {
        let body = "Pay Five Thousand Dollars ($3,000.00) by May 7, 2024.\n\n\
            | Invoice | Amount |\n|---|---|\n| A | $100.00 |\n| B | $200.00 |\n| Total | $310.00 |";
        let r = run(&[page(1, body)], VerifyFields::Flagged, "");
        assert_eq!(
            r.pages[0].page_check,
            ["words_numerals_mismatch", "arithmetic_mismatch"]
        );
        let got = statuses(&r);
        let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
        assert_eq!(status("$3,000.00").1, Status::Check);
        assert!(status("$3,000.00").2.contains(&"words_numerals_mismatch"));
        assert!(status("$310.00").2.contains(&"arithmetic_mismatch"));
        // The items above the total are in the disagreeing sum too: any of them may be the
        // misread one.
        assert!(status("$100.00").2.contains(&"arithmetic_mismatch"));
        assert!(status("$200.00").2.contains(&"arithmetic_mismatch"));
        assert_eq!(r.azure_pages, "1");
    }

    #[test]
    fn case_numbers_in_other_formats_are_found_by_label() {
        let body = "Case No.: 11-4604 PSG ORDER GRANTING\nCase No. C-08-01337 MHP\n\
            C.A. No. 5215-VCG\nIndex No. 651234/2021\nCase no. 20 Civ 1234\n\
            See Docket No. 35 at 9:20. Case No. 3:26-cv-04417. The case number ORDERS.\n\
            case number 5:15CV-01102-JGB-SP and Case No. 03:11-\ncv-01234";
        let got: Vec<String> = extract(body)
            .into_iter()
            .filter(|f| f.kind == Kind::Docket)
            .map(|f| f.text)
            .collect();
        assert_eq!(
            got,
            [
                "11-4604 PSG",
                "C-08-01337 MHP",
                "5215-VCG",
                "651234/2021",
                "20 Civ 1234",
                "3:26-cv-04417",
                "5:15CV-01102-JGB"
            ]
        );
        // Compared as exact letters and digits: one digit off, or O for 0, is not verified.
        let layer = "In Case No. C-08-01337 MHP the parties filed the joint statement that the \
            Court requested, with every exhibit and declaration, on the date the order set.";
        // Embedded text broken at a hyphen, or with a soft hyphen, still matches.
        for layer in [
            "In this matter, case number 5:15-\r\nCV-01102-JGB-SP, Plaintiff argued that the \
             County was liable for every one of the injuries alleged in the complaint.",
            "In this matter, case number 5:15-CV\u{fffe}-01102-JGB-SP, Plaintiff argued that the \
             County was liable for every one of the injuries alleged in the complaint.",
        ] {
            let r = run(
                &[page(1, "case number 5:15-CV-01102-JGB-SP, Plaintiff")],
                VerifyFields::Flagged,
                layer,
            );
            assert_eq!(statuses(&r)[0].1, Status::Verified, "{layer}");
        }
        for (ocr, status) in [
            ("Case No. C-08-01337 MHP", Status::Verified),
            ("Case No. C-08-01387 MHP", Status::Check),
            ("Case No. C-O8-01337 MHP", Status::Check),
        ] {
            let r = run(&[page(1, ocr)], VerifyFields::Flagged, layer);
            assert_eq!(statuses(&r)[0].1, status, "{ocr}");
        }
    }

    #[test]
    fn money_without_a_dollar_sign() {
        let body = "a fee of USD 5,000 and 12,500 dollars, the sum of 48,250.00 in total, \
            but an amount of 1,500 units, 30 days, and $2,000 dollars.";
        let got: Vec<(String, Option<serde_json::Value>)> = extract(body)
            .into_iter()
            .filter(|f| f.kind == Kind::Amount)
            .map(|f| (f.text, f.value))
            .collect();
        assert_eq!(
            got,
            [
                ("USD 5,000".to_string(), Some(serde_json::json!(5000.0))),
                (
                    "12,500 dollars".to_string(),
                    Some(serde_json::json!(12500.0))
                ),
                ("48,250.00".to_string(), Some(serde_json::json!(48250.0))),
                ("$2,000".to_string(), Some(serde_json::json!(2000.0))),
            ]
        );
        // Compared by value with the embedded text, `$` or not.
        let layer = "The Court awards the sum of $48,250.00 to the plaintiff, with interest from \
            the date of filing, and costs as the clerk taxes them in the ordinary course.";
        for (ocr, status) in [
            ("the sum of 48,250.00 to the plaintiff", Status::Verified),
            ("the sum of 43,250.00 to the plaintiff", Status::Check),
        ] {
            let r = run(&[page(1, ocr)], VerifyFields::Flagged, layer);
            assert_eq!(statuses(&r)[0].1, status, "{ocr}");
        }
    }

    #[test]
    fn an_ocr_text_layer_flags_disagreements_but_verifies_nothing() {
        let rest = " More words of the old scanner layer so the page has enough text to compare.";
        let body = format!("Paid $5,500 on January 11, 2008 under Case No. 3:08-cv-09901.{rest}");
        let layer = format!("Paid $5,500 on January 11, 2003 under Case No. 3:08-cv-09901.{rest}");
        let body = body.as_str();
        let pages = [page(1, body)];
        for (mode, agreed) in [
            (VerifyFields::Flagged, Status::Unchecked),
            (VerifyFields::All, Status::Check),
        ] {
            let r = report(
                "x.pdf",
                "x.md".into(),
                "apple",
                "now",
                &pages,
                mode,
                &|_| Embedded {
                    text: layer.clone(),
                    visibility: TextVisibility::OcrLayer,
                    ..Embedded::default()
                },
                &|_| None,
                false,
            );
            assert!(r.pages[0].ocr_text_layer);
            assert_eq!(r.pages[0].text_layer_visibility, "ocr_layer");
            let got = statuses(&r);
            let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
            assert_eq!(status("$5,500").1, agreed, "{mode:?}");
            assert_eq!(
                status("January 11, 2008"),
                (
                    "January 11, 2008".into(),
                    Status::Check,
                    vec!["not_in_ocr_text_layer"]
                )
            );
            assert_eq!(r.summary.verified, 0);
            assert_eq!(r.pages[0].second_reading, Some("azure"));
            let a = r.pages[0].text_agreement.as_ref().unwrap();
            assert_eq!((a.compared_with, a.independent), ("ocr_text_layer", false));
        }
    }

    /// Padding so a page passes the 20-word embedded-text threshold.
    const PAD: &str = " The parties and their counsel reviewed the whole payment schedule in full \
        detail before signing it on the first page.";

    #[test]
    fn each_value_matches_its_own_occurrence() {
        // Swapped between labels: both amounts are on the page, neither where the OCR has it.
        let layer = format!("Principal: $8,000. Interest: $3,000.{PAD}");
        let body = format!("Principal: $3,000. Interest: $8,000.{PAD}");
        let r = run(&[page(1, &body)], VerifyFields::Flagged, &layer);
        let s = statuses(&r);
        assert!(s.iter().all(|x| x.1 == Status::Check), "{s:?}");
        assert!(s.iter().all(|x| x.2.contains(&"label_differs")), "{s:?}");
        // Read twice where the page has it once: the second is not verified.
        let layer = format!("Fee: $3,000. Tax: $8,000.{PAD}");
        let body = format!("Fee: $3,000. Tax: $3,000.{PAD}");
        let r = run(&[page(1, &body)], VerifyFields::Flagged, &layer);
        let s = statuses(&r);
        assert_eq!(s[0].1, Status::Verified, "{s:?}");
        assert_eq!(s[1].1, Status::Check, "{s:?}");
        // The same values in the same places still verify, in a table too.
        let layer = format!("| Principal | $8,000.00 |\n| Interest | $3,000.00 |\n{PAD}");
        let r = run(&[page(1, &layer)], VerifyFields::Flagged, &layer);
        assert!(
            statuses(&r).iter().all(|x| x.1 == Status::Verified),
            "{:?}",
            statuses(&r)
        );
        // One misread letter in a label is still the same label.
        let r = run(
            &[page(
                1,
                &format!("Principa1: $8,000. Interest: $3,000.{PAD}"),
            )],
            VerifyFields::Flagged,
            &format!("Principal: $8,000. Interest: $3,000.{PAD}"),
        );
        assert!(
            statuses(&r).iter().all(|x| x.1 == Status::Verified),
            "{:?}",
            statuses(&r)
        );
    }

    #[test]
    fn signs_are_part_of_the_value() {
        for (body, layer) in [
            (
                "A credit of $100.00 applies.",
                "A credit of -$100.00 applies.",
            ),
            (
                "A credit of -$100.00 applies.",
                "A credit of $100.00 applies.",
            ),
            ("| Refund | $100.00 |", "| Refund | ($100.00) |"),
            (
                "The rate fell by 25% this year.",
                "The rate fell by -25% this year.",
            ),
        ] {
            let r = run(
                &[page(1, &format!("{body}{PAD}"))],
                VerifyFields::Flagged,
                &format!("{layer}{PAD}"),
            );
            assert_eq!(
                statuses(&r)[0].1,
                Status::Check,
                "{body}: {:?}",
                statuses(&r)
            );
        }
        let f = extract("Refunds: -$100.00, $-50.00 and \u{2212}$7.00; rate -25%; range 10-25%.");
        let got: Vec<_> = f
            .iter()
            .map(|f| (f.text.as_str(), f.value.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("-$100.00", Some(serde_json::json!(-100.0))),
                ("$-50.00", Some(serde_json::json!(-50.0))),
                ("-$7.00", Some(serde_json::json!(-7.0))),
                ("-25%", None),
                ("25%", None),
            ]
        );
        // Brackets around an amount in prose are not a sign.
        let f = extract("Five Thousand Dollars ($5,000.00)");
        assert_eq!(f[0].value, Some(serde_json::json!(5000.0)));
        let f = extract("| Refund | ($20.00) |");
        assert_eq!(
            (f[0].text.as_str(), f[0].value.clone()),
            ("($20.00)", Some(serde_json::json!(-20.0)))
        );
    }

    #[test]
    fn a_malformed_bare_amount_is_not_cut_short() {
        let f = extract("Paid USD 1,00.00 today, per § 1983(A).");
        assert_eq!(
            (f[0].text.as_str(), f[0].value.clone(), f[0].why.clone()),
            ("USD 1,00.00", None, vec!["malformed_amount"])
        );
        assert_eq!(f[1].text, "§ 1983(A)");
    }

    #[test]
    fn an_unknown_text_layer_verifies_nothing() {
        let rest = " More words of the old scanner layer so the page has enough text to compare.";
        let body = format!("Paid $5,500 on January 11, 2008 under Case No. 3:08-cv-09901.{rest}");
        let layer = format!("Paid $5,500 on January 11, 2003 under Case No. 3:08-cv-09901.{rest}");
        let body = body.as_str();
        let pages = [page(1, body)];
        for (mode, agreed) in [
            (VerifyFields::Flagged, Status::Unchecked),
            (VerifyFields::All, Status::Check),
        ] {
            let r = report(
                "x.pdf",
                "x.md".into(),
                "apple",
                "now",
                &pages,
                mode,
                &|_| Embedded {
                    text: layer.clone(),
                    visibility: TextVisibility::Unknown,
                    ..Embedded::default()
                },
                &|_| None,
                false,
            );
            assert!(!r.pages[0].ocr_text_layer);
            assert_eq!(r.pages[0].text_layer_visibility, "unknown");
            let got = statuses(&r);
            let status = |t: &str| got.iter().find(|g| g.0 == t).unwrap().clone();
            assert_eq!(status("$5,500").1, agreed, "{mode:?}");
            assert_eq!(
                status("January 11, 2008"),
                (
                    "January 11, 2008".into(),
                    Status::Check,
                    vec!["text_layer_unknown"]
                )
            );
            assert_eq!(r.summary.verified, 0);
            assert_eq!(r.pages[0].second_reading, Some("azure"));
            let a = r.pages[0].text_agreement.as_ref().unwrap();
            assert_eq!(
                (a.compared_with, a.independent),
                ("unknown_text_layer", false)
            );
        }
    }

    #[test]
    fn a_value_the_ocr_dropped_is_listed() {
        let layer = format!("Fee: $3,000. Total: $8,000. Due May 7, 2024.{PAD}");
        let body = format!("Fee: $3,000. Due May 7, 2024.{PAD}");
        let r = run(&[page(1, &body)], VerifyFields::Flagged, &layer);
        assert_eq!(r.pages[0].expected_fields_missing, ["$8,000"]);
        assert!(
            !r.pages[0]
                .page_check
                .contains(&"fields_missing".to_string())
        );
        let r = run(&[page(1, &body)], VerifyFields::All, &layer);
        assert!(
            r.pages[0]
                .page_check
                .contains(&"fields_missing".to_string())
        );
        // Born-digital: the PDF's own text has the value, at no cost.
        assert_eq!(r.pages[0].second_reading, Some("embedded_text"));
        // Nothing dropped: nothing listed.
        let r = run(&[page(1, &layer)], VerifyFields::All, &layer);
        assert!(
            r.pages[0].expected_fields_missing.is_empty(),
            "{:?}",
            r.pages[0]
        );
    }

    #[test]
    fn ordinal_dates_are_found() {
        for (text, iso) in [
            ("signed on March 3rd, 2024 by", "2024-03-03"),
            ("signed this 3rd day of March, 2024 by", "2024-03-03"),
            ("on the 21st of June 2023 and", "2023-06-21"),
            ("dated 2nd February 2022.", "2022-02-02"),
            ("on August 22nd 2019 the", "2019-08-22"),
        ] {
            let f = extract(text);
            assert_eq!(f.len(), 1, "{text}: {f:?}");
            assert_eq!(
                (f[0].kind, f[0].value.clone()),
                (Kind::Date, Some(iso.into())),
                "{text}"
            );
        }
        // At a line start too, unlike a bare number (a pleading line number).
        assert_eq!(extract("3rd day of March, 2024").len(), 1);
        assert!(extract("2 March 2023 was").is_empty());
        // The ordinal must still be a real day.
        assert_eq!(extract("on March 32nd, 2024")[0].why, vec!["invalid_date"]);
    }

    #[test]
    fn a_docket_read_with_the_same_characters_matches() {
        // PDFKit gives `5-16-cv-` / `1128`; the OCR joined the line without the hyphen.
        let layer =
            format!("In Archibald, case number 5-16-cv-\n1128, the jury returned a verdict.{PAD}");
        let body =
            format!("In Archibald, case number 5-16-cv1128, the jury returned a verdict.{PAD}");
        let r = run(&[page(1, &body)], VerifyFields::Flagged, &layer);
        let s = statuses(&r);
        assert_eq!(s[0].0, "5-16-cv1128");
        assert_eq!(s[0].1, Status::Verified, "{s:?}");
        // A different number still differs.
        let body =
            format!("In Archibald, case number 5-16-cv1129, the jury returned a verdict.{PAD}");
        let r = run(&[page(1, &body)], VerifyFields::Flagged, &layer);
        assert_eq!(statuses(&r)[0].1, Status::Check, "{:?}", statuses(&r));
    }

    #[test]
    fn dates_in_capitals_are_found() {
        for (text, iso) in [
            (
                "BANCO PANAMERICANO, INC.\nMARCH 15, 2007\nINVOICE",
                "2007-03-15",
            ),
            ("TUESDAY, MARCH 13, 1990", "1990-03-13"),
            ("RECEIVED APR 15 2008", "2008-04-15"),
            ("the 3RD DAY OF MAY, 2021 at noon", "2021-05-03"),
        ] {
            let f: Vec<_> = extract(text)
                .into_iter()
                .filter(|f| f.kind == Kind::Date)
                .collect();
            assert_eq!(f.len(), 1, "{text}: {f:?}");
            assert_eq!(f[0].value, Some(iso.into()), "{text}");
        }
    }

    #[test]
    fn invoice_amounts_without_a_dollar_sign_are_found() {
        let amounts = |text: &str| -> Vec<(String, Option<serde_json::Value>)> {
            extract(text)
                .into_iter()
                .filter(|f| f.kind == Kind::Amount)
                .map(|f| (f.text, f.value))
                .collect()
        };
        // The `$` stands on its own line, the numbers below it.
        let page = "FEES.....\n\n$\n\nDISBURSEMENTS\n\nTOTAL AMOUNT DUE\n\n$\n\n# 5,156.00\n\n74.52\n\n5,230.52";
        assert_eq!(
            amounts(page),
            [
                ("5,156.00".to_string(), Some(serde_json::json!(5156.0))),
                ("74.52".to_string(), Some(serde_json::json!(74.52))),
                ("5,230.52".to_string(), Some(serde_json::json!(5230.52))),
            ]
        );
        // A money column; hours stay out.
        let table = "| ID | TIMEKEEPER | HOURS | RATE | AMOUNT |\n| --- | --- | --- | --- | --- |\n\
            | GJJ | GREGORY J. JORDAN | 8.90 | 389.89 | 3,470.00 |\n| JSOH | JEAN SOH | 1.70 | 240.00 | 408.00 |";
        let got: Vec<String> = amounts(table).into_iter().map(|a| a.0).collect();
        assert_eq!(got, ["389.89", "3,470.00", "240.00", "408.00"]);
        // A refund in a money column is negative.
        let refund = "| Item | Amount |\n| --- | --- |\n| Refund | (20.00) |";
        assert_eq!(amounts(refund)[0].1, Some(serde_json::json!(-20.0)));
        // Not money: a lone decimal with no money word near, a version, a measurement in a
        // sentence, a percentage.
        for text in [
            "Section 4.12\n\n3.25",
            "The tract is 12.50 acres.",
            "a rate of 12.50% per year",
        ] {
            assert!(amounts(text).is_empty(), "{text}: {:?}", amounts(text));
        }
    }

    #[test]
    fn citations_stop_at_leader_dots() {
        let f =
            extract("15 U.S.C. § 1692.....12\n15 U.S.C. § 1692a(6).....13\nCal. Civ. Code § 52.1");
        let got: Vec<&str> = f.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(got, ["§ 1692", "§ 1692a(6)", "§ 52.1"]);
    }

    #[test]
    fn values_are_compared_not_substrings() {
        // `$3,000` is inside `$3,000,000` as text, but it is a different amount.
        let r = run(
            &[page(1, "the parties agreed to pay $3,000 under")],
            VerifyFields::Flagged,
            LAYER,
        );
        assert_eq!(statuses(&r)[0].1, Status::Check, "{:?}", statuses(&r));
        // Same values written differently are the same reading.
        let r = run(
            &[page(
                1,
                "case 3:26-CV-4417 on Mar. 3, 2024 agreed to pay $3000000.00 with 12.50% per year",
            )],
            VerifyFields::Flagged,
            LAYER,
        );
        let s = statuses(&r);
        assert!(s.iter().all(|x| x.1 == Status::Verified), "{s:?}");
    }

    #[test]
    fn embedded_line_breaks_and_office_prefixes_still_match() {
        let layer = "Plaintiff filed on January\r\n21, 2026 in Case: 3:19-cv-09902-jdp Document 24 \
                     which the court and the parties and counsel then reviewed in full detail.";
        let r = run(
            &[page(1, "filed on January 21, 2026 in 19-cv-09902-jdp")],
            VerifyFields::Flagged,
            layer,
        );
        assert!(
            statuses(&r).iter().all(|s| s.1 == Status::Verified),
            "{:?}",
            statuses(&r)
        );
        // A wrongly read office prefix is not excused.
        let r = run(
            &[page(1, "Case 8:19-cv-09902-jdp")],
            VerifyFields::Flagged,
            layer,
        );
        assert_eq!(statuses(&r)[0].1, Status::Check);
    }

    #[test]
    fn look_alike_characters_are_found_and_flagged() {
        let f = extract(
            "Case 3:26-cv-O4417 for $l,250.00 filed 04/15/2O24. $Sales and $ISO are words.",
        );
        let got: Vec<(Kind, &str, Vec<&str>)> = f
            .iter()
            .map(|f| (f.kind, f.text.as_str(), f.why.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (Kind::Docket, "3:26-cv-O4417", vec!["confusable_characters"]),
                (Kind::Amount, "$l,250.00", vec!["confusable_characters"]),
                (Kind::Date, "04/15/2O24", vec!["confusable_characters"]),
            ]
        );
        assert!(f.iter().all(|f| f.value.is_none()));
    }

    /// Error injection: each misread planted in an OCR page whose embedded text is correct must
    /// come back `check`, never `verified`.
    #[test]
    fn planted_misreads_are_never_verified() {
        let clean = "On March 3, 2024 the parties agreed to pay $3,000,000 under case \
            3:26-cv-04417-SI, with 12.5% interest, invoice dated 04/15/2024 for $48,250.00, \
            citing 28 U.S.C. § 1441(b).";
        let plants = [
            ("$48,250.00", "$48,256.00"),
            ("$48,250.00", "$43,250.00"),
            ("$48,250.00", "$48,250.80"),
            ("$3,000,000", "$3,000"),
            ("$3,000,000", "$8,000,000"),
            ("$3,000,000", "$3,O00,000"),
            ("04/15/2024", "04/16/2024"),
            ("04/15/2024", "04/15/2021"),
            ("04/15/2024", "04/15/2O24"),
            ("March 3, 2024", "March 8, 2024"),
            ("March 3, 2024", "May 3, 2024"),
            ("3:26-cv-04417", "3:26-cv-04471"),
            ("3:26-cv-04417", "3:28-cv-04417"),
            ("3:26-cv-04417", "3:26-cv-O4417"),
            ("12.5%", "12.6%"),
            ("§ 1441(b)", "§ 1447(b)"),
        ];
        let mut caught = 0;
        for (good, bad) in plants {
            let ocr = clean.replacen(good, bad, 1);
            let r = run(&[page(1, &ocr)], VerifyFields::Flagged, LAYER);
            let planted: Vec<_> = r.pages[0]
                .fields
                .iter()
                .filter(|f| f.text.contains(bad.trim_start_matches("§ ")))
                .collect();
            assert!(!planted.is_empty(), "{bad} not extracted");
            for f in planted {
                assert_ne!(f.status, Status::Verified, "{bad} was verified: {f:?}");
                caught += 1;
            }
            // Everything else on the page is still verified.
            assert!(
                r.pages[0]
                    .fields
                    .iter()
                    .filter(|f| !f.text.contains(bad.trim_start_matches("§ ")))
                    .all(|f| f.status == Status::Verified),
                "{bad}: {:?}",
                statuses(&r)
            );
        }
        assert_eq!(caught, plants.len());
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
