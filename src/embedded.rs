//! Exact characters from a PDF's embedded text, laid out by Apple Vision.
//!
//! Born-digital filings carry their exact text, but that text has no reliable layout: margin
//! numbers come inline, tables are flattened and Bates numbers are lost. Vision gives the layout
//! but misreads characters. So each Vision block keeps its structure (paragraph, list items,
//! table cells), and its words are aligned with the embedded words found inside the block's box.
//! Aligned words take the embedded form; a short run of differing words between two aligned
//! words takes the embedded run (`3,000` read for `8,000`).
//!
//! Two rules keep this safe:
//! - A block whose words mostly disagree (a hidden text layer that does not match the page) is
//!   left as Vision read it.
//! - Embedded words with no visible counterpart are never inserted. This covers text under a
//!   drawn redaction box, white or invisible text, and the neighbour's text caught by the box.

use crate::vision::Block;

/// Share of words that must align (2 * matches / (vision + embedded)) for a block to be corrected.
/// Kept loose: a box also catches neighbours' words, and short blocks hold few anchors.
pub const MIN_AGREEMENT: f64 = 0.6;
/// The same share over the whole page. A hidden layer from another draft, or one that does not
/// match the printed page, fails this as a whole, and nothing on the page is changed.
pub const PAGE_MIN_AGREEMENT: f64 = 0.9;
/// A differing run is replaced only when both sides are this short and about the same length.
const MAX_GAP: usize = 4;

/// Letters and digits, lowercased: how two words are compared.
fn norm(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Embedded words, with line-end hyphenation joined when Vision has the joined word
/// (`agree-` `ment` -> `agreement`). Court-reporter software spaces transcripts with middle
/// dots (`· · ·KEENAN T. KNIGHT,`): they are spacing, never part of a word.
fn embedded_words(text: &str, vision: &std::collections::HashSet<String>) -> Vec<String> {
    let raw: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '\u{b7}')
        .filter(|w| !w.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let w = raw[i];
        if let (Some(stem), Some(next)) = (w.strip_suffix('-'), raw.get(i + 1)) {
            let joined = format!("{stem}{next}");
            // A word broken at a hyphen (`agree-` `ment`), not a dash (`--` `well,`).
            if stem.ends_with(char::is_alphanumeric) && vision.contains(&norm(&joined)) {
                out.push(joined);
                i += 2;
                continue;
            }
        }
        out.push(w.to_string());
        i += 1;
    }
    out
}

/// Whether `a` can be an OCR misreading of `b`: at most a third of the characters differ (and
/// at least one may). `perfonn`/`perform` and `$3,000`/`$8,000` are; `thirty`/`sixty` is not.
///
/// `strict` (columns and transcripts, whose alignment is less certain): words of one or two
/// characters change only between look-alike characters of the same length. One edit is most
/// of such a word, so a misaligned line number `7` would become `6`, and `of` would become `o`.
/// On ordinary pages the alignment is reliable, and a misread `9` for `¶` or `•` for a footnote
/// `6` is fixed.
fn misreading(a: &str, b: &str, strict: bool) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if strict && a.len().min(b.len()) <= 2 {
        return a.len() == b.len()
            && a.iter()
                .zip(&b)
                .all(|(x, y)| x == y || look_alike(*x) == look_alike(*y));
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()] <= (a.len().max(b.len()) / 3).max(1)
}

/// Whether every character of `a` occurs in `b`, in order, counting curly and straight quotes,
/// and dashes and hyphens, as the same.
fn subsequence(a: &str, b: &str) -> bool {
    let loose = |c: char| match c {
        '\u{2018}' | '\u{2019}' | '`' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        c => c,
    };
    let mut rest = b.chars().map(loose);
    a.chars().map(loose).all(|c| rest.any(|d| d == c))
}

/// Characters OCR confuses with one another, as one representative each.
fn look_alike(c: char) -> char {
    match c {
        'l' | 'I' | '1' | '|' | 'i' | '!' => 'l',
        'O' | '0' | 'o' | 'Q' | '\u{41e}' | '\u{43e}' => 'o',
        'S' | '5' | '$' | 's' => 's',
        'B' | '8' => 'b',
        'Z' | '2' => 'z',
        '\u{410}' => 'A',
        '\u{430}' => 'a',
        '\u{415}' => 'E',
        '\u{435}' => 'e',
        '\u{443}' => 'y',
        '\u{440}' => 'p',
        '\u{441}' => 'c',
        '\u{445}' => 'x',
        c => c,
    }
}

/// Pairs `(i, j)` of a longest common subsequence of `a` and `b` (compared normalized).
fn align(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if !a[i].is_empty() && a[i] == b[j] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if !a[i].is_empty() && a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// What correcting one block did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Words whose characters changed to the embedded form.
    pub fixed: usize,
    /// Whether the block's words agreed with the embedded text well enough to be corrected.
    pub applied: bool,
}

/// A block's corrected texts, before the page-level decision.
struct Proposal {
    segments: Vec<String>,
    fixed: usize,
    /// Aligned words, and embedded words over the aligned span.
    pairs: usize,
    covered: usize,
}

/// Corrects `segments` (the texts of one block: a paragraph, list items, or table cells in
/// row order) against the embedded text found in the block's box. Segment boundaries and word
/// counts outside differing runs are kept, so structure survives.
pub fn correct(segments: &mut [String], embedded: &str) -> Outcome {
    match propose(segments, embedded, false) {
        Some(p) => {
            segments.clone_from_slice(&p.segments);
            Outcome {
                fixed: p.fixed,
                applied: true,
            }
        }
        None => Outcome::default(),
    }
}

/// Fixes one differing run of Vision words `vr` against embedded words `er` (which may extend
/// past the run at a block edge). Returns the number of words changed.
fn fix_run(
    out: &mut [String],
    words: &[(usize, String)],
    emb: &[String],
    vr: std::ops::Range<usize>,
    er: std::ops::Range<usize>,
    leading: bool,
    strict: bool,
) -> usize {
    let squash = |a: &[String]| a.iter().map(|w| norm(w)).collect::<String>();
    let vision: Vec<String> = vr.clone().map(|x| words[x].1.clone()).collect();
    // Embedded words aligned with the run: before the first anchor the last `n`, else the first.
    let pick = |n: usize| -> Option<&[String]> {
        (n <= er.len()).then(|| {
            if leading {
                &emb[er.end - n..er.end]
            } else {
                &emb[er.start..er.start + n]
            }
        })
    };
    // The same characters split differently (`No.3:26-cv-04417` / `No. 3:26-cv-04417`).
    for n in 1..=er.len() {
        // Split differently: the same letters and digits, and nothing Vision read is lost, though
        // punctuation it missed may come back (`14lawsuit` / `14 lawsuit`, `units essentially,` /
        // `units—essentially,`; not `--14` / `14`). Word for word, symbols may change (`$$` /
        // `§§`, `LL.C,` / `LLC,`).
        if let Some(e) = pick(n)
            && squash(e) == squash(&vision)
            && (n == vision.len() || subsequence(&vision.concat(), &e.concat()))
            && e != vision.as_slice()
        {
            out[vr.start] = e.join(" ");
            for w in &mut out[vr.start + 1..vr.end] {
                w.clear();
            }
            return 1;
        }
    }
    // Word for word, only where the Vision word is a plausible misread of the embedded one.
    let mut n = 0;
    if let Some(e) = pick(vision.len()) {
        for (d, x) in vr.enumerate() {
            if out[x] != e[d] && misreading(&words[x].1, &e[d], strict) {
                out[x] = e[d].clone();
                n += 1;
            }
        }
    }
    n
}

fn propose(segments: &[String], embedded: &str, strict: bool) -> Option<Proposal> {
    let words: Vec<(usize, String)> = segments
        .iter()
        .enumerate()
        .flat_map(|(s, t)| t.split_whitespace().map(move |w| (s, w.to_string())))
        .collect();
    if words.is_empty() {
        return None;
    }
    let vision_norm: Vec<String> = words.iter().map(|(_, w)| norm(w)).collect();
    let known: std::collections::HashSet<String> = vision_norm.iter().cloned().collect();
    let emb = embedded_words(embedded, &known);
    let emb_norm: Vec<String> = emb.iter().map(|w| norm(w)).collect();
    let pairs = align(&vision_norm, &emb_norm);
    let agreement = 2.0 * pairs.len() as f64 / (words.len() + emb.len()).max(1) as f64;
    // Embedded text may carry the neighbours' words caught by the box; judge agreement on the
    // span of the embedded text that the alignment actually covers.
    let covered = match (pairs.first(), pairs.last()) {
        (Some(f), Some(l)) => l.1 - f.1 + 1,
        _ => emb.len(),
    };
    let agreement = agreement.max(2.0 * pairs.len() as f64 / (words.len() + covered) as f64);
    if agreement < MIN_AGREEMENT {
        return None;
    }

    let mut out: Vec<String> = words.iter().map(|(_, w)| w.clone()).collect();
    let mut fixed = 0;
    for &(i, j) in &pairs {
        if out[i] != emb[j] {
            // Vision glued punctuation to the word (`--14`, `Document#:`) that the embedded text
            // has as a neighbouring word: keep it, split the embedded way.
            let glued = (0..=1usize)
                .flat_map(|a| (0..=1usize).map(move |b| (a, b)))
                .filter(|&(a, b)| (a, b) != (0, 0) && j >= a && j + b < emb.len())
                .map(|(a, b)| &emb[j - a..=j + b])
                .find(|w| w.concat() == out[i]);
            out[i] = glued.map_or_else(|| emb[j].clone(), |w| w.join(" "));
            fixed += 1;
        }
    }
    // The differing runs: between anchors, and before the first and after the last (bounded by
    // the block's own words on the Vision side). A run Vision has no words for is never filled
    // in (redactions, hidden text).
    type Run = (std::ops::Range<usize>, std::ops::Range<usize>, bool);
    let mut runs: Vec<Run> = pairs
        .windows(2)
        .map(|w| (w[0].0 + 1..w[1].0, w[0].1 + 1..w[1].1, false))
        .collect();
    if let (Some(&(fi, fj)), Some(&(li, lj))) = (pairs.first(), pairs.last()) {
        runs.push((0..fi, fj.saturating_sub(MAX_GAP)..fj, true));
        runs.push((
            li + 1..words.len(),
            lj + 1..(lj + 1 + MAX_GAP).min(emb.len()),
            false,
        ));
    }
    for (vr, er, leading) in runs {
        let one_segment = vr.clone().all(|x| words[x].0 == words[vr.start].0);
        if vr.is_empty() || er.is_empty() || vr.len() > MAX_GAP || !one_segment {
            continue;
        }
        fixed += fix_run(&mut out, &words, &emb, vr, er, leading, strict);
    }
    let mut segments = segments.to_vec();
    for (s, seg) in segments.iter_mut().enumerate() {
        let joined: Vec<&str> = words
            .iter()
            .zip(&out)
            .filter(|((ws, _), w)| *ws == s && !w.is_empty())
            .map(|(_, w)| w.as_str())
            .collect();
        if !joined.is_empty() {
            *seg = joined.join(" ");
        }
    }
    Some(Proposal {
        segments,
        fixed,
        pairs: pairs.len(),
        covered,
    })
}

/// The texts of a block, in the order `correct` takes them.
fn segments(b: &Block) -> Vec<String> {
    match b.kind.as_str() {
        "list" => b.items.iter().flatten().map(|i| i.text.clone()).collect(),
        "table" => b.rows.iter().flatten().flatten().cloned().collect(),
        _ => b.text.iter().cloned().collect(),
    }
}

fn put_back(b: &mut Block, segs: Vec<String>) {
    let mut it = segs.into_iter();
    match b.kind.as_str() {
        "list" => {
            for item in b.items.iter_mut().flatten() {
                if let Some(t) = it.next() {
                    item.text = t;
                }
            }
        }
        "table" => {
            for cell in b.rows.iter_mut().flatten().flatten() {
                if let Some(t) = it.next() {
                    *cell = t;
                }
            }
        }
        _ => {
            if let Some(t) = it.next() {
                b.text = Some(t);
            }
        }
    }
}

/// Per page: text blocks, blocks corrected, words changed, and the page's word agreement.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PageOutcome {
    pub blocks: usize,
    pub corrected: usize,
    pub fixed: usize,
    pub agreement: f64,
    /// Whether the page passed `PAGE_MIN_AGREEMENT` and was corrected.
    pub applied: bool,
}

/// Corrects every block against `texts`, the embedded text of each block's box (same order),
/// when the page as a whole agrees with its embedded text; otherwise changes nothing.
pub fn correct_blocks(blocks: &mut [Block], texts: &[String], strict: bool) -> PageOutcome {
    let mut o = PageOutcome::default();
    let (mut pairs, mut words, mut covered) = (0, 0, 0);
    let mut proposals = Vec::new();
    for (k, (b, t)) in blocks.iter().zip(texts).enumerate() {
        let segs = segments(b);
        let n: usize = segs.iter().map(|s| s.split_whitespace().count()).sum();
        if n == 0 {
            continue;
        }
        o.blocks += 1;
        words += n;
        match propose(&segs, t, strict) {
            Some(p) => {
                pairs += p.pairs;
                covered += p.covered;
                proposals.push((k, p));
            }
            // A block with no matching embedded text counts against the page in full.
            None => covered += t.split_whitespace().count().min(n),
        }
    }
    o.agreement = if words == 0 {
        0.0
    } else {
        2.0 * pairs as f64 / (words + covered) as f64
    };
    if o.agreement < PAGE_MIN_AGREEMENT {
        return o;
    }
    o.applied = true;
    for (k, p) in proposals {
        o.corrected += 1;
        o.fixed += p.fixed;
        put_back(&mut blocks[k], p.segments);
    }
    o
}

/// Box padding before asking for the embedded text, as a fraction of the page: Vision's boxes
/// sit tight on the ink and can clip a glyph.
pub const PAD: f64 = 0.004;

pub fn padded(b: [f64; 4]) -> [f64; 4] {
    let x0 = (b[0] - PAD).max(0.0);
    let y0 = (b[1] - PAD).max(0.0);
    let x1 = (b[0] + b[2] + PAD).min(1.0);
    let y1 = (b[1] + b[3] + PAD).min(1.0);
    [x0, y0, x1 - x0, y1 - y0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(vision: &[&str], embedded: &str) -> (Vec<String>, Outcome) {
        let mut segs: Vec<String> = vision.iter().map(|s| s.to_string()).collect();
        let o = correct(&mut segs, embedded);
        (segs, o)
    }

    #[test]
    fn short_words_change_only_between_look_alikes() {
        for (a, b) in [
            ("l", "I"),
            ("|", "I"),
            ("0", "O"),
            ("\u{410}", "A"),
            ("lt", "It"),
        ] {
            assert!(misreading(a, b, true), "{a} -> {b}");
        }
        // Misalignments in transcript tables: a line number, a page number, a short word.
        for (a, b) in [
            ("7", "6"),
            ("30", "3"),
            ("of", "o"),
            ("an", "on"),
            ("12", "13"),
        ] {
            assert!(!misreading(a, b, true), "{a} -> {b}");
            // Ordinary pages keep the allowance: their alignment is reliable.
            assert!(misreading(a, b, false), "{a} -> {b}");
        }
        // Longer words keep the one-in-three allowance.
        assert!(misreading("$3,000", "$8,000", true));
        assert!(misreading("perfonn", "perform", true));
    }

    #[test]
    fn punctuation_is_swapped_one_for_one_only() {
        let (s, o) = fix(
            &["under 15 U.S.C. $$ 77t(b) and 77v(a) of the Act"],
            "under 15 U.S.C. §§ 77t(b) and 77v(a) of the Act",
        );
        assert!(o.applied);
        assert_eq!(s[0], "under 15 U.S.C. §§ 77t(b) and 77v(a) of the Act");
        let (s, _) = fix(
            &["they threatened -- | did you say criminal charges were filed"],
            "they threatened -- did you say criminal charges were filed",
        );
        assert_eq!(
            s[0],
            "they threatened -- | did you say criminal charges were filed"
        );
        // Re-split only with the same characters: a dash glued to a line number is kept.
        let (s, _) = fix(
            &["you're talking --14 lawsuit and Document#: 24 Filed"],
            "you're talking --\n14 lawsuit and Document #: 24 Filed",
        );
        assert!(s[0].contains("Document #:"), "{}", s[0]);
        assert!(s[0].contains("--"), "{}", s[0]);
        // Punctuation Vision missed comes back.
        let (s, _) = fix(
            &["assemble modular units essentially, separate structures on site"],
            "assemble modular units—essentially, separate structures on site",
        );
        assert!(s[0].contains("units—essentially,"), "{}", s[0]);
        // A quote style or dash differs: still the same text.
        let (s, _) = fix(
            &["the plaintiff' s position during the Relevant Period-first stage"],
            "the plaintiff’s position during the Relevant Period—first stage",
        );
        assert!(
            s[0].contains("plaintiff’s") && s[0].contains("Period—first"),
            "{}",
            s[0]
        );
    }

    #[test]
    fn a_dash_is_not_a_broken_word() {
        // `--` before `well,` once joined into `-well,` (a dash read as a line-end hyphen).
        let line = "16 Q That probably would be -- well, may be a better";
        for vision in [line, "16 Q That probably would be - well, may be a better"] {
            let (s, _) = fix(&[vision], &format!("{line}\n17 question"));
            assert!(!s[0].contains(" -well"), "{}", s[0]);
        }
        // A real line-end hyphen still joins.
        let (s, o) = fix(
            &["the settlement agreement was signed"],
            "the settle-\nment agreement was signed",
        );
        assert!(o.applied);
        assert_eq!(s[0], "the settlement agreement was signed");
    }

    #[test]
    fn misread_characters_take_the_embedded_form() {
        let (s, o) = fix(
            &["Case No.3:26-cv-04417 filed 02/03/0Q under 28 U.S.C. $1441(b) for $3,000.00 due."],
            "Case No. 3:26-cv-04417 filed 02/03/00 under 28 U.S.C. §1441(b) for $8,000.00 due.",
        );
        assert!(o.applied);
        assert_eq!(
            s[0],
            "Case No. 3:26-cv-04417 filed 02/03/00 under 28 U.S.C. §1441(b) for $8,000.00 due."
        );
    }

    #[test]
    fn segments_keep_their_boundaries() {
        let (s, _) = fix(
            &[
                "INV-1001",
                "04/15/2024",
                "$48,250.0O",
                "INV-1002",
                "06/01/2O24",
                "$51,900.00",
            ],
            "Invoice Date Amount INV-1001 04/15/2024 $48,250.00 INV-1002 06/01/2024 $51,900.00",
        );
        assert_eq!(
            s,
            [
                "INV-1001",
                "04/15/2024",
                "$48,250.00",
                "INV-1002",
                "06/01/2024",
                "$51,900.00"
            ]
        );
    }

    #[test]
    fn hidden_text_is_never_inserted() {
        // A redaction box: Vision sees nothing where the layer still has the words.
        let (s, o) = fix(
            &["The settlement amount is to be paid by March 1."],
            "The settlement amount is $2,400,000 confidential to be paid by March 1.",
        );
        assert!(o.applied);
        assert_eq!(s[0], "The settlement amount is to be paid by March 1.");
    }

    #[test]
    fn a_layer_that_does_not_match_the_page_is_ignored() {
        let (s, o) = fix(
            &["This agreement is made between the parties named below on the date shown."],
            "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor.",
        );
        assert!(!o.applied);
        assert_eq!(
            s[0],
            "This agreement is made between the parties named below on the date shown."
        );
    }

    #[test]
    fn a_different_word_is_not_a_misread() {
        // An earlier draft's wording in the hidden layer must not overwrite the printed page.
        let (s, _) = fix(
            &["payment is due within thirty days of the notice given"],
            "payment is due within sixty days of the notice given",
        );
        assert_eq!(
            s[0],
            "payment is due within thirty days of the notice given"
        );
    }

    fn block(text: &str) -> Block {
        Block {
            kind: "paragraph".into(),
            bbox: [0.0; 4],
            text: Some(text.into()),
            items: None,
            rows: None,
        }
    }

    #[test]
    fn a_page_that_disagrees_with_its_layer_is_left_alone() {
        let mut blocks = vec![
            block("The parties agree to the terms below and sign this agreement."),
            block("Payment of $3,000 is due on signing by wire transfer to the account."),
        ];
        let texts = [
            "The parties agree to the terms below and sign this agreement.".to_string(),
            "Completely different hidden words that have nothing to do with it at all.".to_string(),
        ];
        let o = correct_blocks(&mut blocks, &texts, false);
        assert!(!o.applied && o.agreement < PAGE_MIN_AGREEMENT, "{o:?}");
        assert_eq!(
            blocks[1].text.as_deref(),
            Some("Payment of $3,000 is due on signing by wire transfer to the account.")
        );
        let texts = [
            texts[0].clone(),
            "Payment of $8,000 is due on signing by wire transfer to the account.".to_string(),
        ];
        let o = correct_blocks(&mut blocks, &texts, false);
        assert!(o.applied, "{o:?}");
        assert_eq!(o.fixed, 1);
        assert_eq!(
            blocks[1].text.as_deref(),
            Some("Payment of $8,000 is due on signing by wire transfer to the account.")
        );
    }

    #[test]
    fn runs_at_the_edges_of_a_block_are_fixed_too() {
        let (s, _) = fix(&["Case No.3:26-cv-04417"], "Case No. 3:26-cv-04417");
        assert_eq!(s[0], "Case No. 3:26-cv-04417");
        let (s, _) = fix(
            &["Dcfendant shall pay the costs"],
            "1. Defendant shall pay the costs",
        );
        assert_eq!(s[0], "Defendant shall pay the costs");
        // Hidden text after the last visible word is still never added.
        let (s, _) = fix(&["The amount due is"], "The amount due is $2,400,000 and");
        assert_eq!(s[0], "The amount due is");
    }

    #[test]
    fn hyphenated_line_ends_are_joined() {
        let (s, o) = fix(
            &["the parties agreement is binding"],
            "the parties agree- ment is binding",
        );
        assert!(o.applied);
        assert_eq!(s[0], "the parties agreement is binding");
    }

    #[test]
    fn transcript_middle_dots_are_spacing() {
        // Min-U-Script text layers space with middle dots, also between words
        // (`photograph·Exhibit`); a misread is still fixed, and no dot reaches the output.
        let (s, o) = fix(
            &["KEENAN I. KNIGHT, Plaintiff, vs. LT. AVERY"],
            "· · ·KEENAN T. KNIGHT,\r\n· · · · Plaintiff,·vs.\r\n· · ·LT. AVERY",
        );
        assert!(o.applied, "{o:?}");
        assert_eq!(s[0], "KEENAN T. KNIGHT, Plaintiff, vs. LT. AVERY");
    }

    #[test]
    fn neighbours_caught_by_the_box_do_not_block_a_correction() {
        let (s, o) = fix(
            &["Defendant failed to perfonn as follows:"],
            "12,500 units. Defendant failed to perform as follows: a. Late",
        );
        assert!(o.applied);
        assert_eq!(s[0], "Defendant failed to perform as follows:");
    }
}
