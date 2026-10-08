//! dots scan engine (PLAN §4.2): `prompt_layout_all_en` on the whole page, the JSON array of
//! `{bbox, category, text}` converted to Markdown. Never uses `repetition_penalty` (it corrupts
//! the JSON structure): a loop or truncation goes to the crop plan if there is one, then to
//! `prompt_ocr` as plain text, then to Apple Vision with a flag.

use std::path::Path;

use serde_json::Value;

use crate::cli::Engine;
use crate::htmltable;
use crate::model_engine::{
    Ctx, Fail, Output, Planned, Prepared, Prompt, ScanEngine, Spec, clean_ocr, has_repetition_loop,
    render_specs,
};
use crate::pipeline::{Settings, SidePass};

/// `prompt_layout_all_en` from rednote-hilab/dots.mocr@05196189, `dots_mocr/utils/prompts.py`,
/// verbatim (copy in spike/upstream/rednote-hilab_dots.mocr.py; identical to dots.ocr@b06a7810).
pub const PROMPT_LAYOUT: &str = r#"Please output the layout information from the PDF image, including each layout element's bbox, its category, and the corresponding text content within the bbox.

1. Bbox format: [x1, y1, x2, y2]

2. Layout Categories: The possible categories are ['Caption', 'Footnote', 'Formula', 'List-item', 'Page-footer', 'Page-header', 'Picture', 'Section-header', 'Table', 'Text', 'Title'].

3. Text Extraction & Formatting Rules:
    - Picture: For the 'Picture' category, the text field should be omitted.
    - Formula: Format its text as LaTeX.
    - Table: Format its text as HTML.
    - All Others (Text, Title, etc.): Format their text as Markdown.

4. Constraints:
    - The output text must be the original text from the image, with no translation.
    - All layout elements must be sorted according to human reading order.

5. Final Output: The entire output must be a single JSON object.
"#;

/// `prompt_ocr` from the same file: the cheap plain-text mode.
pub const PROMPT_OCR: &str = "Extract the text content from this image.";

/// One layout element. The bbox is not used: elements arrive in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub category: String,
    pub text: String,
}

/// The elements of a reply and whether the JSON array was complete and valid.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub elements: Vec<Element>,
    pub complete: bool,
}

fn element(v: &Value) -> Option<Element> {
    let text = v.get("text").and_then(Value::as_str).unwrap_or("");
    let category = v.get("category").and_then(Value::as_str);
    // A text with no category is still text; an element with neither is nothing.
    if category.is_none() && text.is_empty() {
        return None;
    }
    Some(Element {
        category: category.unwrap_or("Text").to_string(),
        text: text.to_string(),
    })
}

/// The reply without a Markdown code fence around it.
fn unfence(reply: &str) -> &str {
    let t = reply.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Parses the reply. A valid array is complete. Otherwise (cut off, or broken) the complete
/// `{...}` objects found after the first `[` are kept and the rest is dropped.
pub fn parse_layout(reply: &str) -> Layout {
    let text = unfence(reply);
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) {
        return Layout {
            elements: items.iter().filter_map(element).collect(),
            complete: true,
        };
    }
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(text) {
        // A single object instead of an array.
        return Layout {
            elements: element(&v).into_iter().collect(),
            complete: true,
        };
    }
    let start = text.find('[').map_or(0, |i| i + 1);
    let (mut depth, mut in_string, mut escaped, mut from) = (0usize, false, false, None);
    let mut elements = Vec::new();
    for (i, c) in text[start..].char_indices() {
        let at = start + i;
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    from = Some(at);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Some(f) = from.take()
                    && let Ok(v) = serde_json::from_str::<Value>(&text[f..=at])
                    && let Some(e) = element(&v)
                {
                    elements.push(e);
                }
            }
            _ => {}
        }
    }
    Layout {
        elements,
        complete: false,
    }
}

/// Whether the text fields loop. The detector sees the extracted text only, never the JSON
/// (whose keys and bboxes repeat for every element), and with tables cut out (a long table with
/// similar rows is not a loop).
pub fn looped(elements: &[Element]) -> bool {
    let text: Vec<String> = elements
        .iter()
        .map(|e| htmltable::strip_tables(&e.text))
        .collect();
    has_repetition_loop(&text.join("\n"))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A formula's LaTeX without the delimiters the model may have kept.
fn latex(text: &str) -> &str {
    let t = text.trim();
    for (open, close) in [("$$", "$$"), ("\\[", "\\]"), ("\\(", "\\)"), ("$", "$")] {
        if t.len() >= open.len() + close.len()
            && let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close))
        {
            return inner.trim();
        }
    }
    t
}

/// Markdown for the elements, and whether an HTML table was converted. Category mapping: Title
/// `# `, Section-header `## `, Table GFM (HTML kept if it has spans), Formula `$$...$$`, Picture
/// `[Figure]`; every other category, page headers and footers included, is a paragraph.
pub fn render(elements: &[Element]) -> (String, bool) {
    let mut table = false;
    let parts: Vec<String> = elements
        .iter()
        .filter_map(|e| {
            let text = e.text.trim();
            let md = match e.category.as_str() {
                "Picture" => "[Figure]".to_string(),
                _ if text.is_empty() => return None,
                "Title" => format!("# {}", one_line(text)),
                "Section-header" => format!("## {}", one_line(text)),
                "Formula" => format!("$${}$$", latex(text)),
                "Table" => {
                    let (md, n) = htmltable::convert_tables(text);
                    table |= n > 0;
                    md
                }
                _ => text.to_string(),
            };
            Some(md)
        })
        .collect();
    (clean_ocr(&parts.join("\n\n")), table)
}

pub struct Dots;

impl ScanEngine for Dots {
    fn engine(&self) -> Engine {
        Engine::DotsOcr
    }

    fn page_prompt(&self) -> Prompt {
        Prompt::DotsLayout
    }

    /// Measured on two_column and narrow_unequal (docs/spike.md): full page is at 100% line
    /// recall, so the page is not split up front; crops are the first answer to a loop or a
    /// truncated reply.
    fn plan(&self, _side: &SidePass) -> (Vec<Spec>, bool) {
        (vec![(Prompt::DotsLayout, None)], false)
    }

    fn run(&self, ctx: &Ctx, path: &Path, prep: &Prepared, s: &Settings) -> Result<Output, Fail> {
        let planned = prep.planned.as_ref().map_err(|e| Fail::Failed(e.clone()))?;
        // Stage 1: the layout prompt on the page.
        match layout_requests(ctx, planned) {
            Err(Fail::Degenerate) => {}
            other => return other,
        }
        // Stage 2: the crop plan, if the page has one.
        if let Some(specs) = prep.alt.as_deref() {
            let crops = render_specs(path, prep.index, s.dpi, specs, self).map_err(Fail::Failed)?;
            match layout_requests(ctx, &crops) {
                Err(Fail::Degenerate) => {}
                other => return other,
            }
        }
        // Stage 3: plain text. Failing that, the caller falls back to Apple Vision.
        plain_text(ctx, &planned[0])
    }
}

/// Layout requests in order, joined.
fn layout_requests(ctx: &Ctx, reqs: &[Planned]) -> Result<Output, Fail> {
    let mut out = Output {
        body: String::new(),
        table: false,
        flagged: false,
    };
    let mut parts = Vec::new();
    for req in reqs {
        let (md, table, flagged) = layout_request(ctx, req)?;
        out.table |= table;
        out.flagged |= flagged;
        if !md.is_empty() {
            parts.push(md);
        }
    }
    out.body = parts.join("\n\n");
    if out.body.trim().is_empty() {
        return Err(Fail::Failed("empty model output".into()));
    }
    Ok(out)
}

/// One layout request: `Degenerate` for a loop, a reply cut off at `max_tokens`, or JSON with no
/// complete element. Empty output is retried once. JSON that is invalid or cut off is retried
/// once as well; if it stays so, the complete elements of the better reply are kept and the
/// page is flagged.
fn layout_request(ctx: &Ctx, req: &Planned) -> Result<(String, bool, bool), Fail> {
    let (mut empty_retried, mut bad_retried) = (false, false);
    let mut best: Vec<Element> = Vec::new();
    loop {
        let reply = ctx.send(req, ctx.lim.max_tokens, None)?;
        if reply.finish == "length" {
            return Err(Fail::Degenerate);
        }
        if reply.text.trim().is_empty() {
            if empty_retried {
                return Err(Fail::Failed("empty model output".into()));
            }
            empty_retried = true;
            continue;
        }
        let layout = parse_layout(&reply.text);
        if looped(&layout.elements) {
            return Err(Fail::Degenerate);
        }
        if layout.complete {
            let (md, table) = render(&layout.elements);
            return Ok((md, table, false));
        }
        if layout.elements.len() > best.len() {
            best = layout.elements;
        }
        if !bad_retried {
            bad_retried = true;
            continue;
        }
        if best.is_empty() {
            return Err(Fail::Degenerate);
        }
        let (md, table) = render(&best);
        return Ok((md, table, true));
    }
}

/// `prompt_ocr` on the page image: plain text, with tables converted like elsewhere.
fn plain_text(ctx: &Ctx, page: &Planned) -> Result<Output, Fail> {
    let req = Planned {
        prompt: Prompt::DotsOcr,
        ..page.clone()
    };
    let mut empty_retried = false;
    loop {
        let reply = ctx.send(&req, ctx.lim.max_tokens, None)?;
        if reply.finish == "length" {
            return Err(Fail::Failed("output truncated at max_tokens".into()));
        }
        if has_repetition_loop(&htmltable::strip_tables(&reply.text)) {
            return Err(Fail::Failed("repetition loop".into()));
        }
        let (tables, converted) = htmltable::convert_tables(&reply.text);
        let body = clean_ocr(&tables);
        if body.is_empty() {
            if empty_retried {
                return Err(Fail::Failed("empty model output".into()));
            }
            empty_retried = true;
            continue;
        }
        return Ok(Output {
            body,
            table: converted > 0,
            flagged: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(category: &str, text: &str) -> Element {
        Element {
            category: category.into(),
            text: text.into(),
        }
    }

    #[test]
    fn prompts_are_upstream_verbatim() {
        let upstream = include_str!("../spike/upstream/rednote-hilab_dots.mocr.py");
        let layout_at = upstream.find("\"prompt_layout_all_en\": \"\"\"").unwrap();
        let body = &upstream[layout_at + "\"prompt_layout_all_en\": \"\"\"".len()..];
        assert_eq!(PROMPT_LAYOUT, &body[..body.find("\"\"\"").unwrap()]);
        assert!(upstream.contains(&format!("\"prompt_ocr\": \"\"\"{PROMPT_OCR}\"\"\"")));
    }

    #[test]
    fn parses_a_valid_array_with_fences() {
        let json = r#"[{"bbox":[1,2,3,4],"category":"Title","text":"T"},{"bbox":[1,2,3,4],"category":"Picture"}]"#;
        let l = parse_layout(json);
        assert!(l.complete);
        assert_eq!(l.elements, [el("Title", "T"), el("Picture", "")]);
        let l = parse_layout(&format!("```json\n{json}\n```"));
        assert!(l.complete && l.elements.len() == 2);
        assert!(parse_layout("[]").complete);
    }

    #[test]
    fn truncated_json_keeps_the_complete_elements() {
        let json = r#"[{"bbox":[1,2,3,4],"category":"Title","text":"A \"quoted\" } brace"},{"bbox":[1,2,3,4],"category":"Text","text":"two"},{"bbox":[1,2,3,4],"category":"Text","text":"cut of"#;
        let l = parse_layout(json);
        assert!(!l.complete);
        assert_eq!(
            l.elements,
            [el("Title", "A \"quoted\" } brace"), el("Text", "two")]
        );
        // Nothing complete, and not JSON at all.
        assert_eq!(parse_layout(r#"[{"bbox":[1,2"#).elements, []);
        assert_eq!(parse_layout("I cannot read this").elements, []);
        // A broken element in the middle is skipped, the rest kept.
        let l = parse_layout(
            r#"[{"category":"Text","text":"a"},{"category":,},{"category":"Text","text":"b"}]"#,
        );
        assert!(!l.complete);
        assert_eq!(l.elements, [el("Text", "a"), el("Text", "b")]);
    }

    #[test]
    fn category_mapping() {
        let table = "<table><thead><tr><td>A</td><td>B</td></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>";
        let (md, converted) = render(&[
            el("Page-header", "Page 41"),
            el("Title", "UNITED STATES\nDISTRICT COURT"),
            el("Section-header", "Background"),
            el("Text", "Body text.\nSecond line."),
            el("List-item", "a. first"),
            el("Caption", "Figure 1. Sales"),
            el("Footnote", "1 A note."),
            el("Table", table),
            el("Formula", "\\[ E = mc^{2} \\]"),
            el("Formula", "x^2"),
            el("Picture", ""),
            el("Page-footer", "SMITH000123"),
            el("Text", "  "),
        ]);
        assert!(converted);
        assert_eq!(
            md,
            "Page 41\n\n# UNITED STATES DISTRICT COURT\n\n## Background\n\nBody text.\nSecond line.\n\na. first\n\nFigure 1. Sales\n\n1 A note.\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n$$E = mc^{2}$$\n\n$$x^2$$\n\n[Figure]\n\nSMITH000123"
        );
    }

    #[test]
    fn tables_with_spans_stay_html() {
        let table = "<table><tr><td colspan=\"2\">T</td></tr><tr><td>a</td><td>b</td></tr></table>";
        let (md, converted) = render(&[el("Table", table)]);
        assert!(!converted);
        assert_eq!(md, table);
    }

    #[test]
    fn a_valid_forty_element_array_is_not_a_loop() {
        // The JSON itself repeats keys and bboxes 40 times; the detector must not see it.
        let items: Vec<String> = (1..=40)
            .map(|n| {
                format!(
                    r#"{{"bbox":[100,{y},900,{y2}],"category":"Text","text":"Paragraph {n} of the agreement states the obligation."}}"#,
                    y = n * 50,
                    y2 = n * 50 + 40
                )
            })
            .collect();
        let json = format!("[{}]", items.join(","));
        let l = parse_layout(&json);
        assert!(l.complete && l.elements.len() == 40);
        assert!(!looped(&l.elements));
        // Identical short rows and a long repeating table are not loops either.
        let rows = "<tr><td>N/A</td><td>$0.00</td><td>Not applicable</td></tr>".repeat(60);
        assert!(!looped(&[el("Table", &format!("<table>{rows}</table>"))]));
        assert!(!looped(&[
            el("Text", "intro"),
            el("Table", &format!("<table>{rows}</table>")),
            el("Text", "outro")
        ]));
    }

    #[test]
    fn looping_text_fields_are_detected() {
        let looping = "the same line again\n".repeat(30);
        assert!(looped(&[el("Text", "intro"), el("Text", &looping)]));
        // A loop spread over many elements of one text is still one.
        let many: Vec<Element> = (0..30).map(|_| el("Text", "the same line again")).collect();
        assert!(looped(&many));
    }
}
