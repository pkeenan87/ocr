//! HTML tables in model output (olmOCR replies, dots `Table` elements) to GFM, shared by both
//! engines. Tables with `rowspan` or `colspan` cannot be written as GFM and stay raw HTML.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use crate::markdown;

static OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<table\b[^>]*>").unwrap());
static CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</table\s*>").unwrap());
static CLOSE_AT_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</table\s*>$").unwrap());
static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());

/// Why a table is not written as GFM.
#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    /// `rowspan` or `colspan` above 1: GFM cannot express it.
    Spans,
    /// Nested tables, no rows, or only empty cells.
    Unusable,
}

/// Byte ranges of the outermost `<table>...</table>` spans. A table that never closes (a cut-off
/// reply) runs to the end of the text.
fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(open) = OPEN.find_at(text, at) {
        let (mut depth, mut pos, mut end) = (1, open.end(), None);
        while depth > 0 {
            let next_open = OPEN.find_at(text, pos);
            let next_close = CLOSE.find_at(text, pos);
            match (next_open, next_close) {
                (Some(o), Some(c)) if o.start() < c.start() => {
                    depth += 1;
                    pos = o.end();
                }
                (_, Some(c)) => {
                    depth -= 1;
                    pos = c.end();
                    if depth == 0 {
                        end = Some(c.end());
                    }
                }
                (_, None) => break,
            }
        }
        let end = end.unwrap_or(text.len());
        found.push((open.start(), end));
        at = end;
    }
    found
}

/// The text with every table span replaced by a line break. The repetition detector looks at
/// prose only: the markup of a long table with similar rows would look like a loop (a runaway
/// table ends in a truncated reply instead).
pub fn strip_tables(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for (start, end) in spans(text) {
        out.push_str(&text[at..start]);
        out.push('\n');
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// Replaces each HTML table in `text` with a GFM table. Tables that cannot be converted stay as
/// they are. Returns the new text and the number of tables converted.
pub fn convert_tables(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut at = 0;
    let mut converted = 0;
    for (start, end) in spans(text) {
        out.push_str(&text[at..start]);
        // A table that never closed was cut off: leave it raw rather than guess its last row.
        let closed = CLOSE_AT_END.is_match(&text[start..end]);
        match table_to_gfm(&text[start..end]).ok().filter(|_| closed) {
            Some(gfm) => {
                // Keep a blank line on both sides: GFM tables are block elements.
                if !out.is_empty() && !out.ends_with("\n\n") {
                    out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
                }
                out.push_str(&gfm);
                converted += 1;
                if !text[end..].starts_with("\n\n") && !text[end..].is_empty() {
                    out.push_str(if text[end..].starts_with('\n') {
                        "\n"
                    } else {
                        "\n\n"
                    });
                }
            }
            None => out.push_str(&text[start..end]),
        }
        at = end;
    }
    out.push_str(&text[at..]);
    (out, converted)
}

/// One `<table>` as GFM; the first row is the header.
pub fn table_to_gfm(html: &str) -> Result<String, Skip> {
    if OPEN.find_iter(html).count() != 1 {
        return Err(Skip::Unusable);
    }
    let html = BREAK.replace_all(html, " ");
    let dom = tl::parse(&html, tl::ParserOptions::default()).map_err(|_| Skip::Unusable)?;
    let parser = dom.parser();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for node in dom.nodes() {
        let Some(tr) = node.as_tag().filter(|t| t.name().as_utf8_str() == "tr") else {
            continue;
        };
        let mut row = Vec::new();
        for handle in tr.children().top().iter() {
            let Some(cell) = handle
                .get(parser)
                .and_then(|n| n.as_tag())
                .filter(|t| matches!(&*t.name().as_utf8_str(), "td" | "th"))
            else {
                continue;
            };
            for attr in ["rowspan", "colspan"] {
                let span = cell
                    .attributes()
                    .get(attr)
                    .flatten()
                    .and_then(|v| v.as_utf8_str().trim().parse::<u32>().ok());
                if span.is_some_and(|n| n > 1) {
                    return Err(Skip::Spans);
                }
            }
            row.push(decode_entities(&cell.inner_text(parser)));
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    let gfm = markdown::render_table(&rows);
    if gfm.is_empty() {
        Err(Skip::Unusable)
    } else {
        Ok(gfm)
    }
}

/// The few named entities models write, and numeric ones.
fn decode_entities(text: &str) -> String {
    static ENTITY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    let decoded: Cow<str> = ENTITY.replace_all(text, |c: &regex::Captures| {
        let name = &c[1];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        ch.map_or_else(|| c[0].to_string(), String::from)
    });
    decoded.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_table() {
        let html = "<table><thead><tr><th>Invoice</th><th>Amount</th></tr></thead>\n<tbody>\n<tr><td>INV-1</td><td>$48,250.00</td></tr></tbody></table>";
        assert_eq!(
            table_to_gfm(html).unwrap(),
            "| Invoice | Amount |\n| --- | --- |\n| INV-1 | $48,250.00 |"
        );
    }

    #[test]
    fn entities_breaks_pipes_and_whitespace() {
        let html = "<TABLE><tr><td>A &amp; B</td><td>x&lt;y</td></tr><tr><td>a|b<br>c</td><td>  &#36;5\n  &#x24;6 </td></tr></TABLE>";
        assert_eq!(
            table_to_gfm(html).unwrap(),
            "| A & B | x<y |\n| --- | --- |\n| a\\|b c | $5 $6 |"
        );
    }

    #[test]
    fn spans_are_skipped() {
        for html in [
            "<table><tr><th colspan=\"2\">T</th></tr><tr><td>a</td><td>b</td></tr></table>",
            "<table><tr><td rowspan=2>a</td><td>b</td></tr><tr><td>c</td></tr></table>",
        ] {
            assert_eq!(table_to_gfm(html), Err(Skip::Spans));
        }
        // A span of 1 is no span.
        assert!(table_to_gfm("<table><tr><td colspan=\"1\">a</td><td>b</td></tr></table>").is_ok());
    }

    #[test]
    fn unusable_tables() {
        assert_eq!(table_to_gfm("<table></table>"), Err(Skip::Unusable));
        assert_eq!(
            table_to_gfm("<table><tr><td></td></tr></table>"),
            Err(Skip::Unusable)
        );
        let nested =
            "<table><tr><td><table><tr><td>x</td></tr></table></td><td>y</td></tr></table>";
        assert_eq!(table_to_gfm(nested), Err(Skip::Unusable));
    }

    #[test]
    fn converts_in_place_and_keeps_raw_tables() {
        let text = "Intro\n<table><tr><td>a</td><td>b</td></tr><tr><td>1</td><td>2</td></tr></table>\nOutro\n\n<table><tr><td colspan=2>x</td></tr></table>\n";
        let (out, n) = convert_tables(text);
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "Intro\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nOutro\n\n<table><tr><td colspan=2>x</td></tr></table>\n"
        );
        assert_eq!(convert_tables("no tables"), ("no tables".into(), 0));
    }

    #[test]
    fn cut_off_table_runs_to_the_end() {
        let text = "Before\n<table><tr><td>a</td><td>b</td></tr><tr><td>c";
        assert_eq!(strip_tables(text), "Before\n\n");
        let (out, n) = convert_tables(text);
        assert_eq!(n, 0, "{out}");
        assert!(out.ends_with("<td>c"));
    }

    #[test]
    fn strip_removes_whole_spans_including_nested() {
        let text = "a\n<table><tr><td><table><tr><td>x</td></tr></table></td></tr></table>\nb";
        assert_eq!(strip_tables(text), "a\n\n\nb");
    }
}
