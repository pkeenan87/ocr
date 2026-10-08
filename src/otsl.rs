//! PaddleOCR-VL `Table Recognition:` output (OTSL) to a GFM table.

use std::sync::LazyLock;

use regex::Regex;

use crate::markdown;

static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(fcel|ecel|lcel|ucel|xcel|nl|ched|rhed|srow)>").unwrap());
static WRAPPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?otsl>").unwrap());

/// Converts OTSL (`<fcel>text`, `<ecel>`, `<lcel>`, `<ucel>`, `<xcel>`, `<nl>`) to a GFM table
/// whose first row is the header. Spanned cells become empty. Errors describe why the input is
/// not a table, so the caller can fall back to the raw text.
pub fn otsl_to_gfm(otsl: &str) -> Result<String, String> {
    let text = WRAPPER.replace_all(otsl, "");
    let tags: Vec<_> = TAG.captures_iter(&text).collect();
    let Some(first) = tags.first() else {
        return Err("no OTSL cell tags".into());
    };
    if !text[..first.get(0).unwrap().start()].trim().is_empty() {
        return Err("text before the first cell".into());
    }
    let mut rows: Vec<Vec<String>> = vec![Vec::new()];
    for (i, tag) in tags.iter().enumerate() {
        let whole = tag.get(0).unwrap();
        let end = tags
            .get(i + 1)
            .map_or(text.len(), |n| n.get(0).unwrap().start());
        let body = text[whole.end()..end].trim();
        match &tag[1] {
            "nl" if !body.is_empty() => return Err("text after a row break".into()),
            "nl" => {
                if !rows.last().unwrap().is_empty() {
                    rows.push(Vec::new());
                }
            }
            "fcel" | "ched" | "rhed" | "srow" => rows.last_mut().unwrap().push(body.to_string()),
            _ if !body.is_empty() => return Err(format!("text inside a {} cell", &tag[1])),
            _ => rows.last_mut().unwrap().push(String::new()),
        }
    }
    if rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    let table = markdown::render_table(&rows);
    if table.is_empty() {
        return Err("empty table".into());
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_table() {
        let t = otsl_to_gfm(
            "<fcel>Invoice<fcel>Date<fcel>Amount<nl><fcel>INV-1001<fcel>2026-01-05<fcel>$48,250.00<nl>",
        )
        .unwrap();
        assert_eq!(
            t,
            "| Invoice | Date | Amount |\n| --- | --- | --- |\n| INV-1001 | 2026-01-05 | $48,250.00 |"
        );
    }

    #[test]
    fn spans_and_empty_cells_become_empty() {
        let t = otsl_to_gfm(
            "<fcel>Item<fcel>Q1<lcel><fcel>Total<nl><fcel>a<ecel><fcel>2<fcel>3<nl><ucel><xcel><fcel>5<fcel>6<nl>",
        )
        .unwrap();
        assert_eq!(
            t,
            "| Item | Q1 |  | Total |\n| --- | --- | --- | --- |\n| a |  | 2 | 3 |\n|  |  | 5 | 6 |"
        );
    }

    #[test]
    fn escapes_pipes_and_tolerates_wrappers_and_missing_final_break() {
        let t = otsl_to_gfm("<otsl><fcel>a|b<fcel>c<nl><fcel>1<fcel>2</otsl>").unwrap();
        assert_eq!(t, "| a\\|b | c |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn ragged_rows_are_padded() {
        let t = otsl_to_gfm("<fcel>a<fcel>b<fcel>c<nl><fcel>1<nl>").unwrap();
        assert_eq!(t, "| a | b | c |\n| --- | --- | --- |\n| 1 |  |  |");
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(otsl_to_gfm("").is_err());
        assert!(otsl_to_gfm("just some text").is_err());
        assert!(otsl_to_gfm("stray<fcel>a<nl>").is_err());
        assert!(otsl_to_gfm("<fcel>a<nl>oops<nl>").is_err());
        assert!(otsl_to_gfm("<fcel>a<lcel>oops<nl>").is_err());
        assert!(otsl_to_gfm("<ecel><ecel><nl>").is_err());
        assert!(otsl_to_gfm("<nl><nl>").is_err());
    }
}
