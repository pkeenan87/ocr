//! Arithmetic checks on one page's Markdown: an amount written in words against the numerals
//! after it (`Three Hundred Ninety Two Dollars and Thirty One Cents ($392.31)`), and a table's
//! total row against the sum of the rows above it. A mismatch means one of the values was
//! misread (or the document itself is wrong); either way both are checked, and nothing is
//! changed.

use std::sync::LazyLock;

use regex::Regex;

/// What a check found wrong, by 1-based line within the page's Markdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub line: usize,
    pub reason: &'static str,
    /// The numerals whose value disagrees, as written (`$392.31`).
    pub amounts: Vec<String>,
}

/// Numerals in brackets after an amount in words: `($392.31)`, `($4 million)`.
static BRACKETED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\(\s*(\$\s?\d[\d,]*(?:\.\d{1,2})?(?:\s(?:thousand|million|billion))?)\s*\)")
        .unwrap()
});

fn small(w: &str) -> Option<u64> {
    const UNITS: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 8] = [
        "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    if let Some(i) = UNITS.iter().position(|u| *u == w) {
        return Some(i as u64);
    }
    TENS.iter()
        .position(|t| *t == w)
        .map(|i| 20 + 10 * i as u64)
}

fn scale(w: &str) -> Option<u64> {
    match w {
        "thousand" => Some(1_000),
        "million" => Some(1_000_000),
        "billion" => Some(1_000_000_000),
        _ => None,
    }
}

/// The amount in cents spelled by the words just before `end` in `text`, if any:
/// `Five Thousand Dollars`, `four million dollars`, `Ninety-Two Dollars and 31/100`,
/// `Twelve Dollars and Fifty Cents`.
fn words_before(text: &str) -> Option<u64> {
    // Walk back over number words, `and`, `dollars`, `cents` and `NN/100`.
    let tokens: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && c != '/')
                .to_ascii_lowercase()
        })
        .collect();
    let numeric = |t: &str| {
        small(t).is_some()
            || scale(t).is_some()
            || matches!(
                t,
                "hundred" | "and" | "dollars" | "dollar" | "cents" | "cent" | "no/100"
            )
            || t.strip_suffix("/100")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let start = tokens
        .iter()
        .rposition(|t| !numeric(t))
        .map_or(0, |i| i + 1);
    let words = &tokens[start..];
    // Number words that name money: `Section Two ($500)` is not an amount in words.
    let money = words
        .iter()
        .any(|w| w.starts_with("dollar") || w.starts_with("cent") || w.ends_with("/100"));
    if !money || !words.iter().any(|w| small(w).is_some()) {
        return None;
    }
    let (mut total, mut current, mut dollars, mut cents) = (0u64, 0u64, None, 0u64);
    let mut seen = false;
    for w in words {
        if let Some(n) = small(w) {
            current += n;
            seen = true;
        } else if w == "hundred" {
            current = current.max(1) * 100;
        } else if let Some(s) = scale(w) {
            total += current.max(1) * s;
            current = 0;
        } else if w == "dollars" || w == "dollar" {
            dollars = Some(total + current);
            (total, current, seen) = (0, 0, false);
        } else if w == "cents" || w == "cent" {
            cents = total + current;
            (total, current, seen) = (0, 0, false);
        } else if let Some(n) = w.strip_suffix("/100") {
            cents = n.parse().unwrap_or(0);
        }
    }
    // `four million dollars`: the words before `dollars`; no `dollars`: all of them.
    let dollars = match dollars {
        Some(d) => d,
        None if seen => total + current,
        None => return None,
    };
    Some(dollars * 100 + cents)
}

/// `$392.31`, `$4 million` in cents.
fn cents(numerals: &str) -> Option<u64> {
    let t = numerals.trim_start_matches('$').trim().to_ascii_lowercase();
    let (n, s) = match t.split_once(' ') {
        Some((n, w)) => (n.to_string(), scale(w)?),
        None => (t.clone(), 1),
    };
    let n = n.replace(',', "");
    let (whole, frac) = n.split_once('.').unwrap_or((&n, ""));
    let whole: u64 = whole.parse().ok()?;
    let frac: u64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 10,
        2 => frac.parse().ok()?,
        _ => return None,
    };
    // Cents of a scaled amount (`$1.5 million`) are a fraction of the scale.
    Some(if s == 1 {
        whole * 100 + frac
    } else {
        (whole * 100 + frac) * s
    })
}

/// Amounts in words against their numerals.
fn words_and_numerals(body: &str) -> Vec<Mismatch> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        for m in BRACKETED.captures_iter(line) {
            let whole = m.get(0).unwrap();
            let Some(words) = words_before(&line[..whole.start()]) else {
                continue;
            };
            if cents(&m[1]).is_some_and(|n| n != words) {
                out.push(Mismatch {
                    line: i + 1,
                    reason: "words_numerals_mismatch",
                    amounts: vec![m[1].to_string()],
                });
            }
        }
    }
    out
}

/// An amount in a table cell: `$48,250.00`, `48,250.00`, `(1,200.00)` is not summed.
fn cell_amount(cell: &str) -> Option<u64> {
    let t = cell.trim();
    let digits = t.trim_start_matches('$').trim();
    if digits.is_empty()
        || !digits
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return None;
    }
    // Money has cents or digit grouping; `2024` or `12` in a table is not money.
    if !(digits.contains(',') || digits.contains('.') || t.starts_with('$')) {
        return None;
    }
    cents(&format!("${digits}"))
}

/// Table rows labelled total (`Total`, `TOTAL DUE`, `Grand Total`) against the sum of the
/// amounts above them in the same column, back to the header or the previous total. A
/// `Subtotal` row ends a run like a total, and the next total may then equal the subtotals
/// plus what follows them (tax, fees).
fn table_totals(body: &str) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with('|') {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            i += 1;
        }
        let rows: Vec<(usize, Vec<&str>)> = (start..i)
            .map(|n| {
                let cells: Vec<&str> = lines[n].trim().trim_matches('|').split('|').collect();
                (n, cells)
            })
            .filter(|(_, c)| {
                !c.iter()
                    .all(|c| c.trim().chars().all(|ch| "-: ".contains(ch)))
            })
            .collect();
        let width = rows.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        for col in 0..width {
            // Running sums since the last total, and the subtotals within it.
            let (mut run, mut items, mut subtotal_sum) = (0u64, 0usize, None::<u64>);
            let mut amounts: Vec<String> = Vec::new();
            for (n, cells) in &rows {
                let Some(v) = cells.get(col).and_then(|c| cell_amount(c)) else {
                    continue;
                };
                let label = cells
                    .iter()
                    .enumerate()
                    .filter(|(c, _)| *c != col)
                    .map(|(_, c)| c.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                let is_sub = label.contains("subtotal") || label.contains("sub-total");
                let is_total = !is_sub && label.contains("total");
                if is_sub {
                    subtotal_sum = Some(subtotal_sum.unwrap_or(0) + v);
                    run = 0;
                    items = 0;
                    amounts.clear();
                } else if is_total {
                    let expected = subtotal_sum.map_or(run, |s| s + run);
                    if (items >= 2 || subtotal_sum.is_some()) && expected != v {
                        amounts.push(cells[col].trim().to_string());
                        out.push(Mismatch {
                            line: n + 1,
                            reason: "arithmetic_mismatch",
                            amounts: std::mem::take(&mut amounts),
                        });
                    }
                    (run, items, subtotal_sum) = (0, 0, None);
                    amounts.clear();
                } else {
                    run += v;
                    items += 1;
                    amounts.push(cells[col].trim().to_string());
                }
            }
        }
    }
    out
}

/// Both checks on a page's Markdown.
pub fn check(body: &str) -> Vec<Mismatch> {
    let mut out = words_and_numerals(body);
    out.extend(table_totals(body));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_their_numerals() {
        for ok in [
            "the amount of Three Hundred Ninety Two Dollars and Thirty One Cents ($392.31).",
            "budgeted up to four million dollars ($4 million).",
            "the sum of Five Thousand Dollars ($5,000.00) within thirty (30) days",
            "FORTY-FIVE THOUSAND AND NO/100 DOLLARS ($45,000.00)",
            "Twelve Dollars and 50/100 ($12.50)",
            "one hundred twenty-two thousand five hundred twenty-five dollars and fifty cents ($122,525.50)",
            "a fee of $250 ($250) and thirty (30) days",
            "under Section Two ($500 per day) of the Agreement",
        ] {
            assert_eq!(check(ok), vec![], "{ok}");
        }
    }

    /// Error injection: a misread digit in the numerals, or a misread number word, is caught.
    #[test]
    fn a_misread_on_either_side_is_caught() {
        for bad in [
            "the amount of Three Hundred Ninety Two Dollars and Thirty One Cents ($392.81).",
            "the amount of Three Hundred Ninety Two Dollars and Thirty One Cents ($892.31).",
            "the amount of Three Hundred Ninety Two Dollars and Thirty One Cents ($39.23).",
            "the amount of Three Hundred Sixty Two Dollars and Thirty One Cents ($392.31).",
            "budgeted up to four million dollars ($4 billion).",
            "the sum of Five Thousand Dollars ($3,000.00) within thirty (30) days",
            "FORTY-FIVE THOUSAND AND NO/100 DOLLARS ($46,000.00)",
        ] {
            let m = check(bad);
            assert_eq!(m.len(), 1, "{bad}");
            assert_eq!(m[0].reason, "words_numerals_mismatch");
        }
    }

    #[test]
    fn table_totals_match_their_rows() {
        let ok = "| Invoice | Amount |\n|---|---|\n| INV-1001 | $48,250.00 |\n\
            | INV-1002 | $51,900.00 |\n| INV-1003 | $22,375.50 |\n| Total | $122,525.50 |";
        assert_eq!(check(ok), vec![]);
        let with_tax = "| Item | Amount |\n|---|---|\n| Parts | 1,000.00 |\n| Labor | 500.00 |\n\
            | Subtotal | 1,500.00 |\n| Tax | 120.00 |\n| Total Due | 1,620.00 |";
        assert_eq!(check(with_tax), vec![]);
        // Years and counts in a table are not money.
        let counts = "| Year | Units |\n|---|---|\n| 2023 | 12 |\n| 2024 | 30 |\n| Total | 40 |";
        assert_eq!(check(counts), vec![]);
    }

    #[test]
    fn a_misread_table_amount_breaks_the_total() {
        let table = "| Invoice | Amount |\n|---|---|\n| INV-1001 | $48,250.00 |\n\
            | INV-1002 | $51,900.00 |\n| INV-1003 | $22,375.50 |\n| Total | $122,525.50 |";
        for (from, to) in [
            ("$48,250.00", "$43,250.00"),
            ("$51,900.00", "$51,000.00"),
            ("$22,375.50", "$22,875.50"),
            ("$122,525.50", "$122,525.80"),
        ] {
            let m = check(&table.replace(from, to));
            assert_eq!(m.len(), 1, "{to}");
            assert_eq!(m[0].reason, "arithmetic_mismatch");
            assert_eq!(m[0].line, 6);
        }
    }
}
