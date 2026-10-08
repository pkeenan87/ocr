//! Vision blocks to Markdown, plus page and file assembly (PLAN §5.6, §6).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::layout;
use crate::vision::{Block, ListItem};

/// Bates numbers are looked for in blocks starting in this bottom fraction of the page.
const BATES_ZONE: f64 = 0.08;
/// Pleading line numbers live in this left fraction of the page.
const MARGIN_ZONE: f64 = 0.12;
/// Highest pleading line number Vision is expected to read.
const MAX_MARGIN_NUMBER: u32 = 28;

static BATES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z]{2,}[-_ ]?\d{5,}\b").unwrap());

static CURRENCY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bS(\d{1,3}(?:,\d{3})+|\d{4,})\.\d{2}\b").unwrap());

/// Vision reads `$` as `S` on noisy scans. Rewrites money-shaped tokens (`S22,375.50`) only, and
/// returns the new text with the number of replacements.
pub fn fix_currency(text: &str) -> (String, usize) {
    let mut n = 0;
    let fixed = CURRENCY.replace_all(text, |c: &regex::Captures| {
        n += 1;
        format!("${}", &c[0][1..])
    });
    (fixed.into_owned(), n)
}

/// Where a page's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Vision,
    TextLayer,
}

/// One finished page, ready for assembly. This is also the checkpoint record of a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResult {
    /// 1-based page number.
    pub number: usize,
    pub source: Source,
    pub body: String,
    pub bates: Option<String>,
    /// Whether the page was re-recognized column by column.
    pub column_split: bool,
    /// Whether the page looks like a deposition transcript (flagged only; OCR is unchanged).
    pub transcript: bool,
    /// Number of `S`-to-`$` currency corrections made.
    pub currency_fixes: usize,
    /// Whether a model-recognized table was converted on this page.
    pub table: bool,
    /// Whether the model failed and the body is the Apple Vision text instead.
    pub fallback: bool,
    /// Set when the page needs a human look even though it has a body (fallback, bad table).
    pub flagged: bool,
    /// Set when OCR failed; the body is then empty.
    pub error: Option<String>,
    /// Transcript pages only: `Some(true)` if the line numbers were re-anchored to the page
    /// geometry, `Some(false)` if the engine's own numbering is kept (unverified).
    #[serde(default)]
    pub line_numbers: Option<bool>,
}

/// Values for the YAML header.
pub struct FrontMatter<'a> {
    pub source: &'a str,
    /// Page count of the whole document.
    pub pages: usize,
    pub engine: &'a str,
    pub generated: String,
    /// Model engine: adds `table_pages` and `fallback_pages` and has no `currency_fixes`.
    pub model: bool,
    /// `--legal-cleanup` was given: the number of lines it stripped.
    pub legal_cleanup: Option<usize>,
}

/// Pleading margin line numbers (1-28) are dropped by default: Vision reads them only partially
/// and they break up the body text. Revisit together with `--legal-cleanup`.
pub fn is_pleading_margin_number(block: &Block) -> bool {
    let t = block.text.as_deref().unwrap_or("").trim();
    block.kind == "paragraph"
        && (1..=2).contains(&t.len())
        && t.bytes().all(|b| b.is_ascii_digit())
        && t.parse::<u32>()
            .is_ok_and(|n| (1..=MAX_MARGIN_NUMBER).contains(&n))
        && block.bbox[0] + block.bbox[2] <= MARGIN_ZONE
}

/// First Bates-looking token in the blocks at the bottom of the page.
pub fn find_bates(blocks: &[Block]) -> Option<String> {
    blocks
        .iter()
        .filter(|b| b.bbox[1] >= 1.0 - BATES_ZONE)
        .flat_map(block_texts)
        .find_map(|t| BATES.find(&t).map(|m| m.as_str().to_string()))
}

fn block_texts(b: &Block) -> Vec<String> {
    let mut v: Vec<String> = b.text.iter().cloned().collect();
    v.extend(b.items.iter().flatten().map(|i| i.text.clone()));
    v.extend(b.rows.iter().flatten().flatten().cloned());
    v
}

/// Markdown body for one Vision page and its Bates number, if any.
pub fn render_blocks(blocks: &[Block]) -> (String, Option<String>) {
    render(blocks, false)
}

/// Like `render_blocks`, for blocks already in reading order (region by region).
pub fn render_ordered(blocks: &[Block]) -> (String, Option<String>) {
    render(blocks, true)
}

fn render(blocks: &[Block], ordered: bool) -> (String, Option<String>) {
    let bates = find_bates(blocks);
    let body: Vec<Block> = blocks
        .iter()
        .filter(|b| !is_pleading_margin_number(b))
        .cloned()
        .collect();
    let order: Vec<usize> = if ordered {
        (0..body.len()).collect()
    } else {
        layout::reading_order(&body)
    };
    let parts: Vec<String> = order
        .into_iter()
        .filter_map(|i| render_block(&body[i]))
        .collect();
    (parts.join("\n\n"), bates)
}

fn render_block(b: &Block) -> Option<String> {
    let md = match b.kind.as_str() {
        "title" => format!("# {}", b.text.as_deref()?.trim()),
        "paragraph" => b.text.as_deref()?.trim().to_string(),
        "list" => render_list(b.items.as_deref()?),
        "table" => render_table(b.rows.as_deref()?),
        _ => return None,
    };
    (!md.trim().is_empty()).then_some(md)
}

fn render_list(items: &[ListItem]) -> String {
    items
        .iter()
        .map(|i| match i.marker.trim() {
            "" => format!("- {}", i.text.trim()),
            m => format!("{m} {}", i.text.trim()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cell(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

/// GFM table: first row is the header. Ragged rows are padded and all-empty columns dropped.
pub fn render_table(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let grid: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            (0..width)
                .map(|c| r.get(c).map_or(String::new(), |s| cell(s)))
                .collect()
        })
        .collect();
    let keep: Vec<usize> = (0..width)
        .filter(|&c| grid.iter().any(|r| !r[c].is_empty()))
        .collect();
    if keep.is_empty() {
        return String::new();
    }
    // A one-column "table" is Vision reading a text column as a grid; emit plain paragraphs.
    if keep.len() == 1 {
        let cells: Vec<&str> = grid.iter().map(|r| r[keep[0]].as_str()).collect();
        return cells.join("\n\n").replace("\\|", "|");
    }
    let line = |r: &Vec<String>| {
        let cells: Vec<&str> = keep.iter().map(|&c| r[c].as_str()).collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&grid[0])];
    out.push(format!("|{}|", vec![" --- "; keep.len()].join("|")));
    out.extend(grid[1..].iter().map(line));
    out.join("\n")
}

/// Whether an embedded text layer is trustworthy: enough characters, few garbage glyphs.
pub fn text_layer_usable(text: &str) -> bool {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < 200 {
        return false;
    }
    let bad = chars
        .iter()
        .filter(|c| **c == '\u{FFFD}' || c.is_control() || ('\u{E000}'..='\u{F8FF}').contains(c))
        .count();
    (bad as f64) / (chars.len() as f64) < 0.05
}

/// Text-layer page as plain paragraphs: trailing spaces trimmed, blank-line runs collapsed.
pub fn render_text_layer(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
        }
        blank = false;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// Lines at the top and bottom of a page that count as running-header candidates.
const ZONE_LINES: usize = 3;
/// A running header or footer must be on at least this many pages...
const RUNNING_MIN_PAGES: usize = 3;

/// Key for comparing header and footer lines across pages: whitespace collapsed, digit runs
/// replaced by `#`, lowercase, surrounding Markdown (`#` headings, `*`/`_` emphasis) removed.
/// `None` for lines that are never candidates: table rows, page markers, Bates numbers, and
/// lines with nothing left after normalizing.
fn running_key(line: &str) -> Option<String> {
    static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
    let t = line.trim();
    if t.starts_with('|') || t.starts_with("<!--") || BATES.is_match(t) {
        return None;
    }
    let t = t.trim_start_matches('#');
    let t = t.trim_matches(|c: char| c.is_whitespace() || c == '*' || c == '_');
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = DIGITS.replace_all(&t, "#").to_lowercase();
    (!t.is_empty()).then_some(t)
}

/// Longer lines than this may differ by OCR noise: one character in every `FUZZY_CHARS`.
const FUZZY_MIN_LEN: usize = 20;
const FUZZY_CHARS: usize = 20;

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Maps each key to a representative: long keys that differ by OCR noise (`Assel` for `Asset`)
/// share one, so a header read slightly differently on some pages still counts as one header.
fn canonical(keys: impl Iterator<Item = String>) -> HashMap<String, String> {
    let mut keys: Vec<String> = keys.collect();
    keys.sort();
    keys.dedup();
    let mut reps: Vec<String> = Vec::new();
    let mut map = HashMap::new();
    for k in keys {
        let near = |r: &String| {
            let len = k.chars().count().min(r.chars().count());
            len >= FUZZY_MIN_LEN && edit_distance(&k, r) <= len / FUZZY_CHARS
        };
        let rep = match reps.iter().find(|r| near(r)) {
            Some(r) => r.clone(),
            None => {
                reps.push(k.clone());
                k.clone()
            }
        };
        map.insert(k, rep);
    }
    map
}

/// Indices (into `lines`) of the first and last `ZONE_LINES` non-empty lines.
fn zones(lines: &[&str]) -> (Vec<usize>, Vec<usize>) {
    let nonempty: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].trim().is_empty())
        .collect();
    let top = nonempty.iter().take(ZONE_LINES).copied().collect();
    let bottom = nonempty
        .iter()
        .skip(nonempty.len().saturating_sub(ZONE_LINES))
        .copied()
        .collect();
    (top, bottom)
}

/// `--legal-cleanup`: removes running headers and footers, returning the cleaned pages and the
/// number of lines stripped. Works on the deserialized records, so checkpoints are unaffected.
///
/// A normalized line in the top (or bottom) three lines of at least half of the pages, and at
/// least three pages, is a running header (footer) and is removed from that zone on every page.
/// Transcript pages and failed pages neither count nor change. Documents of one or two pages
/// are returned as they are.
pub fn strip_running(pages: &[PageResult]) -> (Vec<PageResult>, usize) {
    let eligible = |p: &PageResult| !p.transcript && p.error.is_none();
    let total = pages.iter().filter(|p| eligible(p)).count();
    if pages.len() <= 2 || total < RUNNING_MIN_PAGES {
        return (pages.to_vec(), 0);
    }
    // Keys of the top and bottom zones of every eligible page, then their canonical forms.
    let zone_keys: Vec<(Vec<String>, Vec<String>)> = pages
        .iter()
        .filter(|p| eligible(p))
        .map(|p| {
            let lines: Vec<&str> = p.body.lines().collect();
            let (top, bottom) = zones(&lines);
            let keys = |z: Vec<usize>| {
                z.into_iter()
                    .filter_map(|i| running_key(lines[i]))
                    .collect()
            };
            (keys(top), keys(bottom))
        })
        .collect();
    let canon = canonical(
        zone_keys
            .iter()
            .flat_map(|(t, b)| t.iter().chain(b))
            .cloned(),
    );
    let mut top_count: HashMap<String, usize> = HashMap::new();
    let mut bottom_count: HashMap<String, usize> = HashMap::new();
    for (top, bottom) in &zone_keys {
        for (keys, count) in [(top, &mut top_count), (bottom, &mut bottom_count)] {
            let reps: HashSet<&String> = keys.iter().map(|k| &canon[k]).collect();
            for r in reps {
                *count.entry(r.clone()).or_default() += 1;
            }
        }
    }
    let running = |count: HashMap<String, usize>| -> HashSet<String> {
        count
            .into_iter()
            .filter(|(_, n)| *n >= RUNNING_MIN_PAGES && n * 2 >= total)
            .map(|(k, _)| k)
            .collect()
    };
    let (headers, footers) = (running(top_count), running(bottom_count));
    if headers.is_empty() && footers.is_empty() {
        return (pages.to_vec(), 0);
    }
    let mut stripped = 0;
    let out = pages
        .iter()
        .map(|p| {
            if !eligible(p) {
                return p.clone();
            }
            let lines: Vec<&str> = p.body.lines().collect();
            let (top, bottom) = zones(&lines);
            let mut drop = vec![false; lines.len()];
            for (zone, set) in [(top, &headers), (bottom, &footers)] {
                for i in zone {
                    if running_key(lines[i]).is_some_and(|k| set.contains(&canon[&k])) {
                        drop[i] = true;
                    }
                }
            }
            let n = drop.iter().filter(|d| **d).count();
            if n == 0 {
                return p.clone();
            }
            stripped += n;
            // Drop the blank lines that followed a removed line, so no gap is left behind.
            let mut kept: Vec<&str> = Vec::new();
            let mut skip_blank = false;
            for (i, line) in lines.iter().enumerate() {
                if drop[i] {
                    skip_blank = true;
                } else if line.trim().is_empty() {
                    if !skip_blank {
                        kept.push(line);
                    }
                } else {
                    skip_blank = false;
                    kept.push(line);
                }
            }
            while kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
            let mut q = p.clone();
            q.body = kept.join("\n");
            q
        })
        .collect();
    (out, stripped)
}

fn page_text(p: &PageResult, markers: bool) -> String {
    if let Some(err) = &p.error {
        let err = err.replace(['\n', '\r'], " ").replace("--", "- -");
        return format!("<!-- page {}: OCR failed: {err} -->", p.number);
    }
    let mut out = String::new();
    if markers {
        match &p.bates {
            Some(b) => out.push_str(&format!("<!-- page {} · Bates {b} -->", p.number)),
            None => out.push_str(&format!("<!-- page {} -->", p.number)),
        }
    }
    if !p.body.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&p.body);
    }
    out
}

fn yaml_str(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || " ._-()/".contains(c))
        && !s.starts_with([' ', '-']);
    if plain {
        s.to_string()
    } else {
        serde_json::to_string(s).unwrap()
    }
}

fn number_list(v: impl Iterator<Item = usize>) -> String {
    format!(
        "[{}]",
        v.map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
    )
}

/// Joins pages (already in page order) into the final document.
pub fn assemble(front: Option<&FrontMatter>, pages: &[PageResult], markers: bool) -> String {
    let mut out = String::new();
    if let Some(f) = front {
        let list = |keep: &dyn Fn(&PageResult) -> bool| {
            number_list(pages.iter().filter(|p| keep(p)).map(|p| p.number))
        };
        let mut head = format!(
            "---\nsource: {}\npages: {}\nengine: {}\ngenerated: {}\ntext_layer_pages: {}\ncolumn_split_pages: {}\ntranscript_pages: {}\n",
            yaml_str(f.source),
            f.pages,
            yaml_str(f.engine),
            f.generated,
            list(&|p| p.source == Source::TextLayer && p.error.is_none()),
            list(&|p| p.column_split),
            list(&|p| p.transcript),
        );
        if pages.iter().any(|p| p.line_numbers.is_some()) {
            head.push_str(&format!(
                "line_numbers: {{anchored: {}, unverified: {}}}\n",
                list(&|p| p.line_numbers == Some(true)),
                list(&|p| p.line_numbers == Some(false)),
            ));
        }
        if f.model {
            head.push_str(&format!(
                "table_pages: {}\nfallback_pages: {}\n",
                list(&|p| p.table),
                list(&|p| p.fallback),
            ));
        } else {
            let fixes: Vec<String> = pages
                .iter()
                .filter(|p| p.currency_fixes > 0)
                .map(|p| format!("{}: {}", p.number, p.currency_fixes))
                .collect();
            head.push_str(&format!("currency_fixes: {{{}}}\n", fixes.join(", ")));
        }
        if let Some(n) = f.legal_cleanup {
            head.push_str(&format!("legal_cleanup: {{stripped_lines: {n}}}\n"));
        }
        head.push_str(&format!(
            "flagged_pages: {}\n---\n\n",
            list(&|p| p.error.is_some() || p.flagged)
        ));
        out.push_str(&head);
    }
    let texts: Vec<String> = pages.iter().map(|p| page_text(p, markers)).collect();
    out.push_str(&texts.join("\n\n"));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str, bbox: [f64; 4]) -> Block {
        Block {
            kind: "paragraph".into(),
            bbox,
            text: Some(text.into()),
            items: None,
            rows: None,
        }
    }

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn table_escapes_and_trims_empty_columns() {
        let t = render_table(&rows(&[&["A|B", "", "C"], &["1", "", "2"], &["x"]]));
        assert_eq!(t, "| A\\|B | C |\n| --- | --- |\n| 1 | 2 |\n| x |  |");
    }

    #[test]
    fn one_column_table_becomes_paragraphs() {
        assert_eq!(render_table(&rows(&[&["a b"], &["c"]])), "a b\n\nc");
    }

    #[test]
    fn table_header_only_and_empty() {
        assert_eq!(
            render_table(&rows(&[&["a", "b"]])),
            "| a | b |\n| --- | --- |"
        );
        assert_eq!(render_table(&rows(&[&["", ""]])), "");
        assert_eq!(render_table(&[]), "");
    }

    #[test]
    fn margin_numbers_filtered() {
        assert!(is_pleading_margin_number(&para(
            "7",
            [0.03, 0.2, 0.02, 0.01]
        )));
        assert!(is_pleading_margin_number(&para(
            "28",
            [0.03, 0.9, 0.04, 0.01]
        )));
        // Too big, wrong place, not a number.
        assert!(!is_pleading_margin_number(&para(
            "29",
            [0.03, 0.2, 0.02, 0.01]
        )));
        assert!(!is_pleading_margin_number(&para(
            "7",
            [0.3, 0.2, 0.02, 0.01]
        )));
        assert!(!is_pleading_margin_number(&para(
            "7th",
            [0.03, 0.2, 0.02, 0.01]
        )));
        assert!(!is_pleading_margin_number(&para(
            "0",
            [0.03, 0.2, 0.02, 0.01]
        )));
        let (md, _) = render_blocks(&[
            para("1", [0.03, 0.2, 0.02, 0.01]),
            para("Body", [0.2, 0.2, 0.5, 0.01]),
        ]);
        assert_eq!(md, "Body");
    }

    #[test]
    fn bates_detection() {
        let footer = para(
            "COMPLAINT - Page 1 of 14 SMITH000123",
            [0.1, 0.95, 0.8, 0.02],
        );
        assert_eq!(find_bates(&[footer]).as_deref(), Some("SMITH000123"));
        let body = para("SMITH000123 in the body", [0.1, 0.4, 0.8, 0.02]);
        assert_eq!(find_bates(&[body]), None);
        let spaced = para("ABC 0001234", [0.1, 0.95, 0.2, 0.02]);
        assert_eq!(find_bates(&[spaced]).as_deref(), Some("ABC 0001234"));
        assert_eq!(
            find_bates(&[para("Page 12345", [0.1, 0.95, 0.2, 0.02])]),
            None
        );
    }

    #[test]
    fn currency_fix_matches_only_money() {
        let (t, n) = fix_currency("| S22,375.50 | S122,525.50 | S1234.00 |");
        assert_eq!(t, "| $22,375.50 | $122,525.50 | $1234.00 |");
        assert_eq!(n, 3);
        for keep in [
            "SMITH000123",
            "S1",
            "Section S2.1",
            "AS22.50",
            "$22.50",
            "S22.5",
            "S2.10",
            "S1.01",
            "S123.45",
        ] {
            assert_eq!(fix_currency(keep), (keep.to_string(), 0), "{keep}");
        }
    }

    #[test]
    fn list_rendering() {
        let item = |m: &str, t: &str| ListItem {
            marker: m.into(),
            text: t.into(),
        };
        let md = render_list(&[
            item("(a)", "first"),
            item("", "second"),
            item("2.", "third"),
        ]);
        assert_eq!(md, "(a) first\n- second\n2. third");
    }

    #[test]
    fn text_layer_checks() {
        assert!(!text_layer_usable("short"));
        assert!(text_layer_usable(&"word ".repeat(100)));
        assert!(!text_layer_usable(&"\u{FFFD}abc ".repeat(100)));
        assert_eq!(render_text_layer("a  \n\n\n\nb\nc\n\n"), "a\n\nb\nc");
    }

    #[test]
    fn assembly() {
        let page = |n, bates: Option<&str>, error: Option<&str>| PageResult {
            number: n,
            source: Source::Vision,
            body: if error.is_some() {
                String::new()
            } else {
                "text".into()
            },
            bates: bates.map(String::from),
            column_split: false,
            transcript: false,
            currency_fixes: 0,
            table: false,
            fallback: false,
            flagged: false,
            line_numbers: None,
            error: error.map(String::from),
        };
        let pages = [
            page(1, Some("AB12345"), None),
            page(2, None, Some("boom -- x")),
        ];
        let md = assemble(None, &pages, true);
        assert_eq!(
            md,
            "<!-- page 1 · Bates AB12345 -->\ntext\n\n<!-- page 2: OCR failed: boom - - x -->\n"
        );
        let md = assemble(None, &pages[..1], false);
        assert_eq!(md, "text\n");
        let front = FrontMatter {
            source: "a.pdf",
            pages: 2,
            engine: "apple (Vision RecognizeDocuments)",
            generated: "t".into(),
            model: false,
            legal_cleanup: None,
        };
        let md = assemble(Some(&front), &pages, true);
        assert!(md.starts_with(
            "---\nsource: a.pdf\npages: 2\nengine: apple (Vision RecognizeDocuments)\n"
        ));
        assert!(md.contains("flagged_pages: [2]\n---\n\n<!-- page 1"));
    }

    fn body_page(n: usize, body: &str) -> PageResult {
        PageResult {
            number: n,
            source: Source::Vision,
            body: body.into(),
            bates: None,
            column_split: false,
            transcript: false,
            currency_fixes: 0,
            table: false,
            fallback: false,
            flagged: false,
            line_numbers: None,
            error: None,
        }
    }

    fn bodies(pages: &[PageResult]) -> Vec<&str> {
        pages.iter().map(|p| p.body.as_str()).collect()
    }

    /// Text that differs between pages even after digits are normalized.
    const WORDS: [&str; 6] = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];

    /// Five pages with a header, a footer `foot(i)` and unique body text.
    fn five(foot: impl Fn(usize) -> String) -> Vec<PageResult> {
        (1..=5)
            .map(|i| {
                body_page(
                    i,
                    &format!(
                        "CONFIDENTIAL - Agreement\n\nBody one {w}.\n\nBody two {w}.\n\n{}",
                        foot(i),
                        w = WORDS[i]
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn running_header_and_footer_on_every_page_are_stripped() {
        let pages = five(|i| format!("Page {i}"));
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 10);
        assert_eq!(out[2].body, "Body one delta.\n\nBody two delta.");
    }

    #[test]
    fn digits_are_normalized() {
        let pages = five(|i| format!("Page {i} of 9"));
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 10);
        assert!(!out[0].body.contains("Page"));
        // Whitespace, case and heading marks do not matter either.
        let mut pages = five(|i| format!("Page {i} of 9"));
        pages[1].body = pages[1]
            .body
            .replace("CONFIDENTIAL - Agreement", "## confidential  -  AGREEMENT");
        assert_eq!(strip_running(&pages).1, 10);
    }

    #[test]
    fn header_misread_on_some_pages_still_counts() {
        let mut pages = five(|i| format!("Page {i}"));
        pages[0].body = pages[0].body.replace("Agreement", "Agreemenl");
        pages[3].body = pages[3].body.replace("Agreement", "Agreemenl");
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 10);
        assert!(!out[0].body.contains("CONFIDENTIAL"));
        // Short lines must match exactly.
        let pages: Vec<PageResult> = ["Page A", "Page A", "Page B", "Page C", "Page D"]
            .iter()
            .enumerate()
            .map(|(i, f)| {
                body_page(
                    i + 1,
                    &format!("{w}\n\n{w} 2\n\n{w} 3\n\n{w} 4\n\n{f}", w = WORDS[i]),
                )
            })
            .collect();
        assert_eq!(strip_running(&pages).1, 0);
    }

    #[test]
    fn line_on_two_of_five_pages_is_kept() {
        let mut pages: Vec<PageResult> = (1..=5)
            .map(|i| {
                body_page(
                    i,
                    &format!("Unique {w}\n\nMiddle {w}\n\nEnd {w}", w = WORDS[i]),
                )
            })
            .collect();
        for p in &mut pages[..2] {
            p.body = format!("Draft notice\n\n{}", p.body);
        }
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 0);
        assert_eq!(bodies(&out), bodies(&pages));
    }

    #[test]
    fn fewer_than_three_pages_is_untouched() {
        let pages = five(|i| format!("Page {i}"));
        for k in [1, 2] {
            let (out, n) = strip_running(&pages[..k]);
            assert_eq!(n, 0);
            assert_eq!(bodies(&out), bodies(&pages[..k]));
        }
    }

    #[test]
    fn transcript_pages_are_untouched_and_do_not_count() {
        let mut pages = five(|i| format!("Page {i}"));
        for p in &mut pages {
            p.transcript = true;
        }
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 0);
        assert_eq!(bodies(&out), bodies(&pages));
        // Mixed: the transcript page keeps its page number, the others lose theirs.
        pages[0].transcript = true;
        for p in &mut pages[1..] {
            p.transcript = false;
        }
        let (out, _) = strip_running(&pages);
        assert_eq!(out[0].body, pages[0].body);
        assert!(!out[4].body.contains("Page 5"));
    }

    #[test]
    fn table_rows_are_never_stripped() {
        let pages: Vec<PageResult> = (1..=5)
            .map(|i| {
                body_page(
                    i,
                    &format!(
                        "| Item | Cost |\n| --- | --- |\n\nText {w}\n\n| Total | $ {i} |",
                        w = WORDS[i]
                    ),
                )
            })
            .collect();
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 0);
        assert_eq!(bodies(&out), bodies(&pages));
    }

    #[test]
    fn identical_line_in_the_middle_of_a_page_is_kept() {
        let mut pages = five(|i| format!("Page {i}"));
        let mid = "a\n\nb\n\nCONFIDENTIAL - Agreement\n\nc\n\nd\n\ne\n\nf\n\nPage 3";
        pages[2].body = format!("CONFIDENTIAL - Agreement\n\n{mid}");
        let (out, _) = strip_running(&pages);
        assert_eq!(
            out[2].body,
            "a\n\nb\n\nCONFIDENTIAL - Agreement\n\nc\n\nd\n\ne\n\nf"
        );
    }

    #[test]
    fn bates_and_failed_pages_are_left_alone() {
        let mut pages: Vec<PageResult> = (1..=5)
            .map(|i| {
                body_page(
                    i,
                    &format!("Body {w}\n\nMore {w}\n\nABC00012{i}", w = WORDS[i]),
                )
            })
            .collect();
        pages[4].error = Some("boom".into());
        let (out, n) = strip_running(&pages);
        assert_eq!(n, 0);
        assert_eq!(bodies(&out), bodies(&pages));
    }

    #[test]
    fn front_matter_reports_stripped_lines() {
        let front = FrontMatter {
            source: "a.pdf",
            pages: 5,
            engine: "e",
            generated: "t".into(),
            model: false,
            legal_cleanup: Some(10),
        };
        let md = assemble(Some(&front), &five(|i| format!("Page {i}")), true);
        assert!(md.contains("legal_cleanup: {stripped_lines: 10}\nflagged_pages: []\n"));
    }
}
