//! Whole-text agreement: which sentences of a page a second reading produced word for word.
//!
//! The page's Markdown is cut into spans: sentences, or whole lines when shorter. A span
//! agrees when its words, compared as letters and digits only and case-insensitively, appear
//! in the same order and with nothing in between in the other reading. Line breaks, hyphenation
//! at line ends and pleading line numbers in the other reading are ignored. Spans that do not
//! agree are listed, so a tool can quote only what two readings share.

use serde::Serialize;

/// A span of the page that the other reading does not have word for word.
#[derive(Debug, Clone, Serialize)]
pub struct Unagreed {
    /// 1-based line within the page's Markdown.
    pub line: usize,
    pub text: String,
    /// Words of the span found nowhere in the other reading (empty when the words are there
    /// but in another order or with other words between them).
    pub missing: Vec<String>,
}

/// What a page's second reading agrees with.
#[derive(Debug, Clone, Serialize)]
pub struct TextAgreement {
    /// `embedded_text`, or `second_reader` (Apple Vision's line recognizer).
    pub compared_with: &'static str,
    /// Whether the two readings are independent: embedded text, or a model against Vision.
    /// Two Apple readings are not: their agreement confirms nothing, only their disagreements
    /// mean something.
    pub independent: bool,
    pub spans: usize,
    pub agreed_spans: usize,
    /// Share of the page's words in agreed spans.
    pub agreed_words: f64,
    pub unagreed: Vec<Unagreed>,
}

/// Letters and digits, lowercased.
fn norm(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// One word of the other reading; `alone` when it is a line number: one or two digits on a
/// line of their own, or starting a line (`19 result of the phone calls?` in a transcript).
struct Word {
    text: String,
    alone: bool,
    /// The first or last word of a line.
    line_start: bool,
    line_end: bool,
    /// The previous word ends a sentence; this word does.
    after_end: bool,
    ends: bool,
    /// At a line start: the line's word count, when the whole line is also text of its own
    /// elsewhere in the Markdown (a margin legend PDFKit puts between a paragraph's lines).
    /// Such a line may be skipped inside a match; a line the Markdown lacks never is.
    skip_line: usize,
    /// The Markdown lines that have that line's text.
    found_in: Vec<usize>,
}

/// Whether a word of the other reading ends a sentence, as `sentence_end` decides for the
/// Markdown (`No.` and `U.S.C.` do not), past closing quotes and brackets.
fn ends_sentence(raw: &str) -> bool {
    let t = raw.trim_end_matches(['"', '\'', ')', ']', '\u{201d}', '\u{2019}']);
    sentence_end(t) || t.ends_with(':')
}

/// The other reading as a stream of normalized words. A word broken at a line-end hyphen is
/// joined, also across a line-number line.
fn stream(other: &str, markdown: &[String]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let elsewhere = |line: &str| {
        let w: Vec<String> = line
            .split_whitespace()
            .map(norm)
            .filter(|w| !w.is_empty())
            .collect();
        let needle = format!(" {} ", w.join(" "));
        if w.len() < 3 {
            return Vec::new();
        }
        markdown
            .iter()
            .enumerate()
            .filter(|(_, m)| m.contains(&needle))
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    let mut hyphen = false;
    let number = |w: &str| Word {
        text: w.to_string(),
        alone: true,
        line_start: true,
        line_end: true,
        after_end: true,
        ends: true,
        skip_line: 0,
        found_in: Vec::new(),
    };
    for line in other.lines() {
        let t = line.trim();
        if line_number(t) {
            words.push(number(t));
            continue;
        }
        let mut first = true;
        let line_at = words.len();
        for (i, w) in t.split_whitespace().enumerate() {
            if i == 0 && line_number(w) {
                words.push(number(w));
                continue;
            }
            let n = norm(w);
            if n.is_empty() {
                continue;
            }
            let joined = hyphen
                && words
                    .iter_mut()
                    .rev()
                    .find(|w| !w.alone)
                    .map(|last| {
                        last.text.push_str(&n);
                        last.ends = ends_sentence(w);
                        last.line_end = false;
                    })
                    .is_some();
            if !joined {
                let after_end = words.iter().rev().find(|w| !w.alone).is_none_or(|w| w.ends);
                words.push(Word {
                    text: n,
                    alone: false,
                    line_start: first && !hyphen,
                    line_end: false,
                    after_end,
                    ends: ends_sentence(w),
                    skip_line: 0,
                    found_in: Vec::new(),
                });
            }
            first = false;
            hyphen = false;
        }
        if words.len() > line_at && !hyphen {
            let found = elsewhere(t);
            if !found.is_empty() {
                words[line_at].skip_line = words.len() - line_at;
                words[line_at].found_in = found;
            }
        }
        // The line's last word ends a span, unless the word continues on the next line.
        if !t.is_empty() {
            hyphen = t.ends_with('-');
            if !hyphen && let Some(last) = words.iter_mut().rev().find(|w| !w.alone) {
                last.line_end = true;
            }
        }
    }
    words
}

/// A margin or pleading line number: one or two digits on their own.
fn line_number(w: &str) -> bool {
    (1..=2).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit())
}

/// Whether `words` occur in `stream` in order with nothing between them, beginning and ending
/// where a span can (a line edge or a sentence end), so that a sentence missing its first or
/// last word does not agree. Line numbers are skipped: the other reading's on lines of their
/// own or starting one (`May 7,` / `6` / `2007`), and the Markdown's at the start of a line
/// (`13 A They all live`, `| 6 | Dated`). A number inside a line is never skipped, so `30` read
/// for `80`, or a `30` only one side has, still differs.
///
/// Only prose (a span ending in sentence punctuation) is held to these edges, since only there
/// a missing first or last word changes a statement. Headers and table cells, which Vision
/// splits where the other reading has one line (`Filed 07/10/26` / `Page 3 of 29`), are matched
/// anywhere.
///
/// A span that starts its Markdown line may start at a line start in the other reading; one
/// that follows another sentence must follow a sentence end there too (`edge_start`), and the
/// same for ends. Otherwise a doubled `2024.` at a line start would pass.
fn occurs(
    words: &[String],
    leading: usize,
    stream: &[Word],
    edge_start: bool,
    edge_end: bool,
    line: usize,
    prose: bool,
) -> bool {
    let from = |j0: usize, k0: usize| {
        let w = &stream[k0];
        if prose && !(w.after_end || edge_start && w.line_start) {
            return false;
        }
        let (mut j, mut k) = (j0, k0);
        while j < words.len() {
            if k < stream.len() && stream[k].text == words[j] {
                j += 1;
                k += 1;
            } else if k > k0 && k < stream.len() && stream[k].alone {
                k += 1;
            } else if j > j0
                && k < stream.len()
                && stream[k].skip_line > 0
                && stream[k].found_in.iter().any(|&m| m != line)
            {
                k += stream[k].skip_line;
            } else {
                return false;
            }
        }
        let last = &stream[k - 1];
        !prose || last.ends || edge_end && last.line_end
    };
    (0..stream.len())
        .any(|k| from(0, k) || (leading > 0 && leading < words.len() && from(leading, k)))
}

/// The spans of one Markdown line: sentences of at least three words, so that `U.S.C.` and
/// `No.` do not cut a citation into pieces too small to mean anything.
fn spans(line: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for w in line.split_whitespace() {
        cur.push(w);
        let words = cur.iter().filter(|w| !norm(w).is_empty()).count();
        if words >= 3 && sentence_end(w) {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        match out.last_mut() {
            // A short tail joins the sentence before it.
            Some(last) if cur.iter().filter(|w| !norm(w).is_empty()).count() < 3 => {
                last.extend(cur)
            }
            _ => out.push(cur),
        }
    }
    out
}

/// Whether a word ends a sentence. Abbreviations do not: `U.S.C.`, `No.`, `v.`, `Inc.`.
fn sentence_end(w: &str) -> bool {
    if w.ends_with(['?', '!', ';']) {
        return true;
    }
    let Some(core) = w.strip_suffix('.') else {
        return false;
    };
    let core = core.trim_start_matches(|c: char| !c.is_alphanumeric());
    // `U.S.C.`, `e.g.`; initials (`D.`); and common abbreviations.
    !core.contains('.')
        && core.chars().count() > 1
        && !ABBREVIATIONS.contains(&core.to_ascii_lowercase().as_str())
}

/// Words that end with a period without ending a sentence, lowercased.
const ABBREVIATIONS: &[&str] = &[
    "no", "nos", "v", "vs", "mr", "mrs", "ms", "dr", "jr", "sr", "inc", "co", "corp", "ltd", "llc",
    "st", "stat", "supp", "cir", "dist", "ct", "app", "rev", "ex", "exh", "id", "al", "seq", "cf",
    "art", "para", "sec", "ch", "fed", "cal", "civ", "crim", "proc", "evid", "ann", "bus", "gov",
    "pen", "dep", "dept", "assn", "ave", "blvd", "pp", "p", "vol", "ed", "approx",
];

/// Markdown that is not page text: markers, table rules, image and comment lines.
fn skipped(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with("<!--") || t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Compares a page's Markdown `body` with another reading of the page.
pub fn compare(
    body: &str,
    other: &str,
    compared_with: &'static str,
    independent: bool,
) -> TextAgreement {
    // Each Markdown line as normalized words, padded for whole-word search.
    let markdown: Vec<String> = body
        .lines()
        .map(|l| {
            let w: Vec<String> = l
                .split_whitespace()
                .map(norm)
                .filter(|w| !w.is_empty())
                .collect();
            format!(" {} ", w.join(" "))
        })
        .collect();
    let other = stream(other, &markdown);
    let present: std::collections::HashSet<&str> = other.iter().map(|w| w.text.as_str()).collect();
    let mut out = TextAgreement {
        compared_with,
        independent,
        spans: 0,
        agreed_spans: 0,
        agreed_words: 0.0,
        unagreed: Vec::new(),
    };
    let (mut words_all, mut words_agreed) = (0usize, 0usize);
    for (i, line) in body.lines().enumerate() {
        if skipped(line) {
            continue;
        }
        let line_spans = spans(line);
        let count = line_spans.len();
        for (n, span) in line_spans.into_iter().enumerate() {
            let words: Vec<String> = span
                .iter()
                .map(|w| norm(w))
                .filter(|w| !w.is_empty())
                .collect();
            // Nothing but line numbers: not text anyone quotes.
            if words.iter().all(|w| line_number(w)) {
                continue;
            }
            out.spans += 1;
            words_all += words.len();
            // Line numbers at the start of the line, which the other reading may place elsewhere.
            let leading = if n == 0 {
                words.iter().take_while(|w| line_number(w)).count()
            } else {
                0
            };
            let prose = span.last().is_some_and(|w| {
                w.trim_end_matches(['"', '\'', ')', '\u{201d}'])
                    .ends_with(['.', '?', '!', ';', ':'])
            });
            if occurs(&words, leading, &other, n == 0, n + 1 == count, i, prose) {
                out.agreed_spans += 1;
                words_agreed += words.len();
            } else {
                let mut missing: Vec<String> = Vec::new();
                for (w, raw) in words
                    .iter()
                    .zip(span.iter().filter(|w| !norm(w).is_empty()))
                {
                    if !present.contains(w.as_str()) && !missing.contains(&raw.to_string()) {
                        missing.push(raw.to_string());
                    }
                }
                out.unagreed.push(Unagreed {
                    line: i + 1,
                    text: span.join(" "),
                    missing,
                });
            }
        }
    }
    out.agreed_words = if words_all == 0 {
        1.0
    } else {
        ((words_agreed as f64 / words_all as f64) * 1000.0).round() / 1000.0
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_agree_across_line_breaks_hyphens_and_line_numbers() {
        let body = "The parties agreed to the settle-ment terms. Defendant shall pay within thirty (30) days.\n\
            | Invoice | Amount |\n|---|---|\n| INV-1001 | $48,250.00 |";
        let other = "The parties agreed to the settle-\n6\n10\nment terms. Defendant shall pay\n\
            within thirty (30) days.\nInvoice Amount\nINV-1001 $48,250.00";
        let a = compare(body, other, "embedded_text", true);
        assert_eq!((a.spans, a.agreed_spans), (4, 4), "{a:#?}");
        assert_eq!(a.agreed_words, 1.0);
    }

    #[test]
    fn a_changed_or_missing_word_breaks_agreement_of_its_sentence_only() {
        let body =
            "Defendant shall pay within thirty (30) days. The total is $122,525.50 plus interest.";
        // `sixty` for `thirty`; the second sentence is intact.
        let other =
            "Defendant shall pay within sixty (30) days. The total is $122,525.50 plus interest.";
        let a = compare(body, other, "embedded_text", true);
        assert_eq!((a.spans, a.agreed_spans), (2, 1));
        assert_eq!(
            a.unagreed[0].text,
            "Defendant shall pay within thirty (30) days."
        );
        assert_eq!(a.unagreed[0].missing, vec!["thirty"]);
        // A dropped word is caught and named.
        let other = "Defendant shall pay thirty (30) days. The total is $122,525.50 plus interest.";
        let a = compare(body, other, "embedded_text", true);
        assert_eq!(a.agreed_spans, 1);
        assert_eq!(a.unagreed[0].missing, vec!["within"]);
        // Words in another order: nothing is missing, but the sentence does not agree.
        let other =
            "Defendant shall pay thirty (30) days within. The total is $122,525.50 plus interest.";
        let a = compare(body, other, "embedded_text", true);
        assert_eq!(a.agreed_spans, 1);
        assert!(a.unagreed[0].missing.is_empty());
        // A whole sentence the other reading lacks.
        let a = compare(
            body,
            "The total is $122,525.50 plus interest.",
            "embedded_text",
            true,
        );
        assert_eq!(a.agreed_spans, 1);
        assert_eq!(a.unagreed[0].line, 1);
    }

    #[test]
    fn digits_are_compared_exactly() {
        let a = compare(
            "Payment of $3,000,000 is due.",
            "Payment of $3,000 000 is due.",
            "embedded_text",
            true,
        );
        // `3000000` against `3000 000`: not the same word sequence.
        assert_eq!(a.agreed_spans, 0);
        let a = compare("Filed 04/28/24 Page 2", "Filed 04/24/24 Page 2", "x", true);
        assert_eq!(a.agreed_spans, 0);
    }

    #[test]
    fn line_numbers_on_one_side_are_skipped_but_never_hide_a_changed_number() {
        let body = "13 A They all live in Springfield with me.\n| 6 | Dated: May 31, 2024 |\n24";
        let other = "A They all live in Springfield with me. 13\nDated: May\n6 31, 2024";
        let a = compare(body, other, "embedded_text", true);
        // The bare `24` is not a span.
        assert_eq!((a.spans, a.agreed_spans), (2, 2), "{a:#?}");
        for (body, other) in [
            (
                "Pay within 30 days of notice.",
                "Pay within 80 days of notice.",
            ),
            (
                "Pay within 30 days of notice.",
                "Pay within days of notice.",
            ),
            (
                "Pay within days of notice.",
                "Pay within 30 days of notice.",
            ),
            ("Pay within 3 0 days.", "Pay within 30 days."),
            (
                "Pay within 30\ndays of notice.",
                "Pay within\n80\ndays of notice.",
            ),
        ] {
            let a = compare(body, other, "embedded_text", true);
            assert!(a.agreed_spans < a.spans, "{body} / {other}");
        }
    }

    /// Error injection: every word of a page, changed (a digit or letter swapped), dropped, or
    /// doubled in the OCR, must leave its sentence unagreed. The other reading has
    /// line numbers between and at the start of its lines.
    #[test]
    fn every_planted_change_breaks_agreement() {
        let truth = "Defendant shall pay $48,250.00 within 30 days of March 3, 2024. \
            The Court GRANTS the motion under 28 U.S.C. § 1441(b) in Case No. 3:26-cv-04417.";
        let other = "11 Defendant shall pay $48,250.00\n12\nwithin 30 days of March 3,\n13 2024. The \
            Court GRANTS the motion\nunder 28 U.S.C. § 1441(b) in Case No. 3:26-cv-04417.";
        assert_eq!(compare(truth, other, "x", true).agreed_spans, 2);
        let words: Vec<&str> = truth.split_whitespace().collect();
        let mut planted = 0;
        for i in 0..words.len() {
            if norm(words[i]).is_empty() {
                continue;
            }
            let changed: String = words[i]
                .chars()
                .map(|c| match c {
                    '0'..='8' => char::from(c as u8 + 1),
                    '9' => '0',
                    'a'..='y' | 'A'..='Y' => char::from(c as u8 + 1),
                    c => c,
                })
                .collect();
            let doubled = format!("{} {}", words[i], words[i]);
            for edit in [Some(changed.as_str()), None, Some(doubled.as_str())] {
                let mut w = words.clone();
                match edit {
                    Some(e) => w[i] = e,
                    None => {
                        w.remove(i);
                    }
                }
                let a = compare(&w.join(" "), other, "x", true);
                assert!(a.agreed_spans < a.spans, "{edit:?} at {i}: {}", w.join(" "));
                planted += 1;
            }
        }
        assert!(planted > 60);
    }

    #[test]
    fn a_margin_legend_between_lines_is_skipped_but_a_dropped_line_is_not() {
        let body = "Defendant does not challenge the position that lactose intolerance could qualify \
            as a disability.\n\nUnited States District Court Northern District of California";
        let other = "Defendant does not challenge the position that lactose intolerance\n\
            United States District Court\nNorthern District of California\n\
            could qualify as a disability.";
        let a = compare(body, other, "embedded_text", true);
        assert_eq!((a.spans, a.agreed_spans), (2, 2), "{a:#?}");
        // The OCR dropped `could qualify` / a whole line: no skipping hides that.
        let body = "Defendant does not challenge the position that lactose intolerance as a \
            disability.\n\nUnited States District Court Northern District of California";
        let other = "Defendant does not challenge the position that lactose intolerance\n\
            could qualify\nas a disability.";
        assert_eq!(compare(body, other, "x", true).agreed_spans, 0);
    }

    #[test]
    fn header_fragments_match_inside_a_longer_line() {
        let body = "Case 5:26-cv-07616 Document 1\nFiled 07/10/26 Page 3 of 29";
        let other = "Case 5:26-cv-07616 Document 1 Filed 07/10/26 Page 3 of 29";
        assert_eq!(compare(body, other, "x", true).agreed_spans, 2);
    }

    #[test]
    fn short_sentences_join_their_neighbour() {
        assert_eq!(
            spans("See 42 U.S.C. § 1983. It applies."),
            vec![vec!["See", "42", "U.S.C.", "§", "1983.", "It", "applies."]]
        );
        assert_eq!(spans("Case No. 3:26-cv-04417").len(), 1);
    }
}
