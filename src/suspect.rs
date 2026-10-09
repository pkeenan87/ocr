//! Pages whose text looks wrong although OCR succeeded. On the public court filings
//! (eval/PUBLIC_SET.md) `flagged_pages` was empty on every document while half the pages had
//! real damage: gibberish lines, paragraphs emitted twice, pleading prose wrapped in a table,
//! transcript mini-pages out of order. These checks look at the finished Markdown of each page,
//! for every engine, and name what they found in `suspect_pages`. They cannot see a wrong but
//! plausible number (`8,000` read as `3,000`) or a dropped sentence.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::anchor;
use crate::markdown::{self, PageResult, Source};

/// English words (lowercase, 4-20 letters), front-coded; see scripts/make_wordlist.py.
const WORDS: &str = include_str!("../data/words-en.txt");
/// Legal Latin and drafting terms missing from the general list.
const LEGAL_WORDS: &str = "alia amicus annum multi arbitrability arguendo certiorari conveniens crossclaim \
    curiae fide habeas indemnitee indemnitees indemnitor judicata justiciability lienholder \
    licensor meruit mootness movant noncompete nonmovant nonsolicitation nunc payor pendente \
    promisor redressability remittitur removability scienter severability sponte subrogee \
    tortfeasor tunc unconscionability vires warranto witnesseth cybersecurity sublicense \
    sublicensee counterparty";
/// A word that is a known word after one of these prefixes is known (`nonparty`).
const PREFIXES: &[&str] = &[
    "non", "un", "sub", "counter", "cross", "re", "pre", "co", "inter", "over", "under", "mis",
    "anti", "post", "multi",
];

/// Window of consecutive tokens in which gibberish is looked for.
const WINDOW: usize = 8;
/// A window is garbled with at least this many unknown words...
const GARBLED_MIN: usize = 3;
/// ...making up at least half of its checked words.
const GARBLED_SHARE: f64 = 0.5;
/// A page has scattered misreads when this share of its checked words is unknown... Clean
/// synthetic legal text has at most 1.0%; this repo's own technical docs, full of tool names,
/// 3.6-4.8%.
const MISREAD_SHARE: f64 = 0.04;
/// ...and at least this many.
const MISREAD_MIN: usize = 6;
/// Lines of mostly symbols (rotated sidebars, speckle read as text) on one page.
const JUNK_MIN_LINES: usize = 3;
/// A run of this many words appearing twice on a page is a duplicated passage: a paragraph,
/// longer than the clauses legal drafting repeats verbatim (up to 26 words in the Apache
/// License, 45 in the synthetic contracts' reused template sentences).
const SHINGLE: usize = 60;
/// Words of four or more letters a duplicated passage must hold.
const PROSE_MIN: usize = 12;
/// Pleading line numbers run to 28.
const MAX_MARGIN_NUMBER: u32 = 28;
/// A table column of pleading line numbers has at least this many of them.
const MARGIN_MIN_ROWS: usize = 4;

static DICTIONARY: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut set = HashSet::new();
    let mut prev = String::new();
    for line in WORDS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let shared = (line.as_bytes()[0] - b'0') as usize;
        let mut word = prev[..shared.min(prev.len())].to_string();
        word.push_str(&line[1..]);
        set.insert(word.clone());
        prev = word;
    }
    set.extend(LEGAL_WORDS.split_whitespace().map(String::from));
    set
});

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9][A-Za-z0-9'’]*").unwrap());
/// URLs, e-mail addresses and file names: never words.
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\S*(?:https?://|www\.|@|\.(?:com|gov|org|net|edu|pdf)\b)\S*").unwrap()
});
/// A digit inside a lowercase word (`I2seler`, `respon5e`): a misread, never an identifier.
static DIGIT_IN_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z][0-9][a-z]{2,}|[a-z]{3,}[0-9]+[a-z]").unwrap());
/// A table-of-contents line: dot leaders ending in a page number.
static TOC_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.{3,}\s*\d+\s*$").unwrap());

/// Whether a lowercase word is English (or a legal term), allowing plurals and common prefixes.
fn known(w: &str) -> bool {
    let d = &*DICTIONARY;
    let base = |w: &str| {
        d.contains(w)
            || w.strip_suffix('s').is_some_and(|s| d.contains(s))
            || w.strip_suffix("es").is_some_and(|s| d.contains(s))
    };
    base(w)
        || PREFIXES.iter().any(|p| {
            w.strip_prefix(p)
                .is_some_and(|rest| rest.len() >= 4 && base(rest))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A lowercase word in the dictionary.
    Known,
    /// A lowercase word that is not, or a digit inside a word.
    Unknown,
    /// Not checked: capitalized words (names), short words, numbers and identifiers.
    Skip,
}

fn classify(token: &str) -> Class {
    if token.bytes().any(|b| b.is_ascii_digit()) {
        return if DIGIT_IN_WORD.is_match(token) {
            Class::Unknown
        } else {
            Class::Skip
        };
    }
    let w = token
        .trim_end_matches(['\'', '’'])
        .trim_end_matches("'s")
        .trim_end_matches("’s");
    if w.len() < 4 || !w.bytes().all(|b| b.is_ascii_lowercase()) {
        return Class::Skip;
    }
    if known(w) {
        Class::Known
    } else {
        Class::Unknown
    }
}

/// Text lines of a page body that carry prose: markers and table separators left out, table
/// cells joined.
fn text_lines(body: &str) -> impl Iterator<Item = String> + '_ {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("<!--"))
        .filter(|l| !(l.starts_with('|') && l.chars().all(|c| "|-: ".contains(c))))
        .map(|l| l.replace('|', " "))
}

/// Gibberish: a window of words mostly unknown (`garbled`), or many scattered unknown words on
/// the page (`misreads`).
fn word_checks(body: &str, reasons: &mut Vec<&'static str>) {
    let (mut known_n, mut unknown_n) = (0, 0);
    let mut garbled = false;
    for line in text_lines(body) {
        let line = URL.replace_all(&line, " ");
        let classes: Vec<Class> = TOKEN
            .find_iter(&line)
            .map(|m| classify(m.as_str()))
            .collect();
        for c in &classes {
            match c {
                Class::Known => known_n += 1,
                Class::Unknown => unknown_n += 1,
                Class::Skip => {}
            }
        }
        garbled |= classes.windows(WINDOW.min(classes.len()).max(1)).any(|w| {
            let bad = w.iter().filter(|c| **c == Class::Unknown).count();
            let good = w.iter().filter(|c| **c == Class::Known).count();
            bad >= GARBLED_MIN && bad as f64 >= GARBLED_SHARE * (bad + good) as f64
        });
    }
    if garbled {
        reasons.push("garbled");
    }
    let checked = known_n + unknown_n;
    if unknown_n >= MISREAD_MIN && unknown_n as f64 >= MISREAD_SHARE * checked as f64 {
        reasons.push("misreads");
    }
}

/// A line of mostly symbols that is not a rule, a leader line or a form box.
fn is_junk_line(line: &str) -> bool {
    let t: Vec<char> = line
        .trim_start_matches('#')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if t.len() < 3 || TOC_LINE.is_match(line) {
        return false;
    }
    if t.iter().all(|c| "_-–—=*·•.()[]☐☒/\\".contains(*c)) {
        return false;
    }
    let alnum = t.iter().filter(|c| c.is_alphanumeric()).count();
    alnum * 2 < t.len()
}

fn junk_lines(body: &str) -> usize {
    text_lines(body)
        .filter(|l| !l.trim_start().starts_with('|'))
        .filter(|l| is_junk_line(l))
        .count()
}

/// Whether a passage of `SHINGLE` words, mostly prose, occurs twice on the page. On transcript
/// pages line numbers (one or two digits) are left out, so a renumbered repeat still matches.
fn duplicated(body: &str, transcript: bool) -> bool {
    let words: Vec<String> = text_lines(body)
        .flat_map(|l| {
            TOKEN
                .find_iter(&l)
                .map(|m| m.as_str().to_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|w| !(transcript && w.len() <= 2 && w.bytes().all(|b| b.is_ascii_digit())))
        .collect();
    // Form furniture (`By: Name: Title:`) repeats legitimately; a duplicated passage is prose.
    let prose = |s: &[String]| {
        s.iter()
            .filter(|w| w.len() >= 4 && w.bytes().all(|b| b.is_ascii_lowercase()))
            .count()
            >= PROSE_MIN
    };
    let mut first: HashMap<&[String], usize> = HashMap::new();
    for (i, s) in words.windows(SHINGLE).enumerate() {
        match first.get(s) {
            Some(&j) if i >= j + SHINGLE && prose(s) => return true,
            Some(_) => {}
            None => {
                first.insert(s, i);
            }
        }
    }
    false
}

/// Markdown tables of a page, as rows of trimmed cells.
fn tables(body: &str) -> Vec<Vec<Vec<String>>> {
    let mut out: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Vec<Vec<String>> = Vec::new();
    for line in body.lines().map(str::trim) {
        if line.starts_with('|') {
            if !line.chars().all(|c| "|-: ".contains(c)) {
                let inner = line.trim_matches('|');
                current.push(inner.split('|').map(|c| c.trim().to_string()).collect());
            }
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A table whose first column, header included, is pleading line numbers: the margin numbers
/// were read as a column of a table that is really prose, or pulled into a real table's crop.
fn pleading_table(rows: &[Vec<String>]) -> bool {
    let first: Vec<Option<u32>> = rows
        .iter()
        .map(|r| {
            r.first()
                .and_then(|c| c.parse::<u32>().ok())
                .filter(|n| (1..=MAX_MARGIN_NUMBER).contains(n))
        })
        .collect();
    let numbers: Vec<u32> = first.iter().flatten().copied().collect();
    let rising = numbers.windows(2).filter(|w| w[1] > w[0]).count();
    first.first().is_some_and(Option::is_some)
        && numbers.len() >= MARGIN_MIN_ROWS
        && numbers.len() * 4 >= rows.len() * 3
        && rising * 4 >= (numbers.len() - 1) * 3
}

/// `Page N` headers of a multi-up transcript that do not rise down the page.
fn pages_out_of_order(body: &str) -> bool {
    let numbers: Vec<u32> = body.lines().filter_map(anchor::header_number).collect();
    numbers.windows(2).any(|w| w[1] <= w[0])
}

/// Why a finished page looks wrong; empty if nothing was found. Failed pages are not checked.
pub fn check(page: &PageResult) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if page.error.is_some() || page.body.trim().is_empty() {
        return reasons;
    }
    let body = &page.body;
    word_checks(body, &mut reasons);
    if junk_lines(body) >= JUNK_MIN_LINES {
        reasons.push("junk_lines");
    }
    if duplicated(body, page.transcript) {
        reasons.push("duplicated");
    }
    let tables = tables(body);
    if tables.iter().any(|t| pleading_table(t)) {
        reasons.push("pleading_table");
    }
    if page.transcript {
        if !tables.is_empty() {
            reasons.push("transcript_table");
        }
        if pages_out_of_order(body) {
            reasons.push("page_order");
        }
        if page.line_numbers == Some(false) {
            reasons.push("line_numbers_unverified");
        }
    }
    reasons
}

/// Assembly step for every engine: restores `¶`/`§` in OCR'd pages (unless `symbols` is off),
/// then records why each page looks wrong. Works on the records, so checkpoints are unaffected.
pub fn review(pages: &[PageResult], symbols: bool) -> Vec<PageResult> {
    pages
        .iter()
        .map(|p| {
            let mut q = p.clone();
            if symbols && q.source == Source::Vision && q.error.is_none() {
                (q.body, q.symbol_fixes) = markdown::fix_legal_symbols(&q.body);
            }
            q.suspect = check(&q);
            q
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(body: &str) -> PageResult {
        PageResult {
            number: 1,
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
            suspect: Vec::new(),
        }
    }

    const CLEAN: &str = "Plaintiff alleges that Defendant breached the Agreement by failing to \
        deliver the goods on the date required. The parties agreed that any dispute arising \
        under this agreement would be resolved by arbitration in accordance with the rules then \
        in effect, and that the prevailing party would recover its reasonable attorneys' fees.";

    #[test]
    fn dictionary_loads_and_knows_legal_words() {
        for w in [
            "plaintiff",
            "defendants",
            "nonparty",
            "estoppel",
            "judicata",
            "counterclaims",
        ] {
            assert!(known(w), "{w}");
        }
        for w in ["tmiedio", "ingeamaetimi", "memotandum", "luthknent"] {
            assert!(!known(w), "{w}");
        }
    }

    #[test]
    fn clean_prose_is_not_suspect() {
        assert!(check(&page(CLEAN)).is_empty());
    }

    #[test]
    fn golden_outputs_are_not_suspect() {
        for name in ["legal", "legal_scan", "two_column", "three_pages"] {
            let md = std::fs::read_to_string(format!("tests/golden/{name}.md")).unwrap();
            let body = md.splitn(3, "---\n").nth(2).unwrap();
            for part in body.split("<!-- page ").skip(1) {
                assert_eq!(check(&page(part)), Vec::<&str>::new(), "{name}: {part}");
            }
        }
    }

    #[test]
    fn gibberish_line_on_a_clean_page_is_garbled() {
        // holt p4 on the public set: two lines of a born-digital agreement became nonsense.
        let body = format!("{CLEAN}\n\nSut no tmiedio ingeamaetimi com cilomce ay a lid.");
        assert_eq!(check(&page(&body)), ["garbled"]);
        let body = format!("{CLEAN}\n\nI2seler's luthknent orils respors bribes in the lease.");
        assert_eq!(check(&page(&body)), ["garbled"]);
    }

    #[test]
    fn scattered_font_misreads_are_counted() {
        // cimontubo on the public set: Garamond r read as t.
        let body = "This memotandum addresses the artangements the parties made. The entite \
            agreement was signed after the patties met. The tetms were clear and the \
            obligations wete set out in the schedule. Each patty agreed to the conttact and \
            its tetms, and the coutt should enfotce them as wtitten.";
        assert!(check(&page(body)).contains(&"misreads"));
    }

    #[test]
    fn names_numbers_and_urls_are_not_checked() {
        let body = "Case No. 2:23-cv-01234-ABC (N.D. Cal.), Kuzdas v. Xyloquent Brzezinski LLC, \
            see https://www.example.com/qwzx/plorbt and F.3d 1123; INV-4411 dated 3/4/2021.";
        assert!(check(&page(body)).is_empty());
    }

    #[test]
    fn junk_lines_need_several() {
        let one = format!("{CLEAN}\n\n~n.n......");
        assert!(check(&page(&one)).is_empty());
        let three = format!("{CLEAN}\n\n~n.n......\n\ni= ≤ ~\n\nNN......");
        assert_eq!(check(&page(&three)), ["junk_lines"]);
        let fine = format!("{CLEAN}\n\nARGUMENT .......... 4\n\n__________\n\n* * *\n\n( ) [ ]");
        assert!(check(&page(&fine)).is_empty());
    }

    #[test]
    fn repeated_paragraph_is_duplicated() {
        let para = format!("{CLEAN} {CLEAN}");
        let body = format!("{para}\n\n12. Other text.\n\n{para}");
        assert_eq!(check(&page(&body)), ["duplicated"]);
        let body = format!("{CLEAN}\n\n12. Other text.\n\n{CLEAN}");
        assert!(check(&page(&body)).is_empty());
        // Realleging boilerplate twice is normal.
        let realleges = "Plaintiff repeats and realleges each and every allegation set forth \
            above as if fully set forth herein.";
        let body = format!("{realleges}\n\n{CLEAN}\n\n{realleges}");
        assert!(check(&page(&body)).is_empty());
    }

    #[test]
    fn pleading_numbers_as_a_table_column() {
        let body = "| 3 | Invoice No. | Date | Amount |\n| --- | --- | --- | --- |\n\
            | 4 | INV-4411 | 1/2/2021 | $100.00 |\n| 5 | INV-4412 | 1/3/2021 | $200.00 |\n\
            | 6 | INV-4413 | 1/4/2021 | $300.00 |\n| 8 | TOTAL | | $600.00 |";
        assert_eq!(check(&page(body)), ["pleading_table"]);
        // A numbered exhibit list has a header row.
        let exhibits = "| No. | Description |\n| --- | --- |\n| 1 | Lease |\n| 2 | Invoice |\n\
            | 3 | Letter |\n| 4 | Notice |";
        assert!(check(&page(exhibits)).is_empty());
    }

    #[test]
    fn transcript_checks() {
        let mut p =
            page("Page 45\n1 Q. Where were you?\n\nPage 47\n1 A. At home.\n\nPage 46\n1 Q. When?");
        p.transcript = true;
        assert_eq!(check(&p), ["page_order"]);
        p.body = "Page 45\n1 Q. Where were you?\n\nPage 46\n1 A. At home.".into();
        p.line_numbers = Some(false);
        assert_eq!(check(&p), ["line_numbers_unverified"]);
        p.line_numbers = Some(true);
        p.body =
            "| 1 Q. Where were you? | 1 Q. When? |\n| --- | --- |\n| 2 A. Home. | 2 A. Noon. |"
                .into();
        assert_eq!(check(&p), ["transcript_table"]);
    }

    #[test]
    fn review_fixes_symbols_on_ocr_pages_only() {
        let pages = vec![page("(Compl. 9 60.)"), {
            let mut p = page("(Compl. 9 60.)");
            p.source = Source::TextLayer;
            p
        }];
        let out = review(&pages, true);
        assert_eq!(
            (out[0].body.as_str(), out[0].symbol_fixes),
            ("(Compl. ¶ 60.)", 1)
        );
        assert_eq!(
            (out[1].body.as_str(), out[1].symbol_fixes),
            ("(Compl. 9 60.)", 0)
        );
        assert_eq!(review(&pages, false)[0].symbol_fixes, 0);
    }
}
