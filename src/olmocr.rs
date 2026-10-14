//! olmOCR 2 scan engine (PLAN §4.2): the upstream no-anchoring YAML prompt, text before the
//! image, every image 1288 px on the longest side, YAML front matter parsed for the rotation
//! check and stripped, HTML tables as GFM, figures as `[Figure: alt]`. Headers and footers are
//! dropped by the model; Bates numbers come from the Apple side-pass.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::cli::Engine;
use crate::htmltable;
use crate::markdown;
use crate::model_engine::{
    Ctx, Fail, LOOP_PENALTY, Output, Planned, Prepared, Prompt, ScanEngine, Spec,
    has_repetition_loop, render_specs,
};
use crate::pipeline::{Settings, SidePass};
use crate::vision;

/// `build_no_anchoring_v4_yaml_prompt()` from allenai/olmocr@8ef7f8085aeea5a7b857f2ff4a032f3a8f0fcfa2,
/// `olmocr/prompts/prompts.py`, verbatim (copy in spike/upstream/allenai_olmocr.py).
pub const PROMPT: &str = "Attached is one page of a document that you must process. Just return the plain text representation of this document as if you were reading it naturally. Convert equations to LateX and tables to HTML.\nIf there are any figures or charts, label them with the following markdown syntax ![Alt text describing the contents of the figure](page_startx_starty_width_height.png)\nReturn your output as markdown, with a front matter section on top specifying values for the primary_language, is_rotation_valid, rotation_correction, is_table, and is_diagram parameters.";

/// `--target_longest_image_dim` default of olmOCR's pipeline.
pub const MAX_SIDE: u32 = 1288;

const FRONT_KEYS: [&str; 5] = [
    "primary_language",
    "is_rotation_valid",
    "rotation_correction",
    "is_table",
    "is_diagram",
];

static FIGURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(page_\d+_\d+_\d+_\d+\.png\)").unwrap());

/// The parts of the reply's YAML front matter the engine uses.
#[derive(Debug, PartialEq, Eq)]
pub struct Front {
    /// `is_rotation_valid`; `None` if the key is missing (a cut-off reply).
    pub rotation_valid: Option<bool>,
    /// `rotation_correction`: 0, 90, 180 or 270.
    pub rotation_correction: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    pub front: Option<Front>,
    /// The text after the front matter (all of the reply if it has none).
    pub body: String,
}

/// Splits a reply into front matter and text. A reply may have no front matter (the model
/// skipped it) or only part of it (cut off): the latter has no body. Booleans are `True` or
/// `False`, in either case.
pub fn parse_reply(reply: &str) -> Parsed {
    let text = reply.trim_start();
    let mut lines = text.split('\n');
    let opens = lines.next().is_some_and(|l| l.trim_end() == "---");
    let first_key = lines
        .next()
        .and_then(|l| l.split_once(':'))
        .is_some_and(|(k, _)| FRONT_KEYS.contains(&k.trim()));
    if !opens || !first_key {
        return Parsed {
            front: None,
            body: reply.to_string(),
        };
    }
    let mut front = Front {
        rotation_valid: None,
        rotation_correction: 0,
    };
    let mut body_at = text.len();
    let mut offset = 4.min(text.len());
    for line in text.split('\n').skip(1) {
        let end = offset + line.len();
        if line.trim_end() == "---" {
            body_at = (end + 1).min(text.len());
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "is_rotation_valid" => {
                    front.rotation_valid = match value.to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    }
                }
                "rotation_correction" => {
                    front.rotation_correction = value
                        .parse()
                        .ok()
                        .filter(|n| [0, 90, 180, 270].contains(n))
                        .unwrap_or(0);
                }
                _ => {}
            }
        }
        offset = end + 1;
    }
    Parsed {
        front: Some(front),
        body: text[body_at..].to_string(),
    }
}

/// Reply text as Markdown: figures become `[Figure: alt]`, HTML tables GFM (those with spans
/// stay HTML); whitespace as `clean_ocr`. Also returns whether a table was converted.
pub fn postprocess(body: &str) -> (String, bool) {
    let (tables, converted) = htmltable::convert_tables(body);
    let figures = FIGURE.replace_all(&tables, |c: &regex::Captures| {
        let alt = c[1].split_whitespace().collect::<Vec<_>>().join(" ");
        if alt.is_empty() {
            "[Figure]".to_string()
        } else {
            format!("[Figure: {alt}]")
        }
    });
    (crate::model_engine::clean_ocr(&figures), converted > 0)
}

/// Whether the prose of the reply loops. Tables are left out: the markup of a long table with
/// similar rows is not a loop (a runaway table ends in a truncated reply).
pub fn looped(body: &str) -> bool {
    has_repetition_loop(&htmltable::strip_tables(body))
}

pub struct Olmocr;

impl ScanEngine for Olmocr {
    fn engine(&self) -> Engine {
        Engine::Olmocr
    }

    fn page_prompt(&self) -> Prompt {
        Prompt::Olmocr
    }

    fn max_side(&self) -> Option<u32> {
        Some(MAX_SIDE)
    }

    fn text_first(&self) -> bool {
        true
    }

    /// Measured on two_column and narrow_unequal (docs/spike.md): full page is at 100% line
    /// recall, so the page is never split up front; crops are the first answer to a loop.
    fn plan(&self, _side: &SidePass) -> (Vec<Spec>, bool) {
        (vec![(Prompt::Olmocr, None)], false)
    }

    fn run(&self, ctx: &Ctx, path: &Path, prep: &Prepared, s: &Settings) -> Result<Output, Fail> {
        let planned = prep.planned.as_ref().map_err(|e| Fail::Failed(e.clone()))?;
        let mut out = match run_requests(ctx, path, prep, s, planned, true, prep.alt.is_some()) {
            Err(Fail::Degenerate) => {
                let specs = prep.alt.as_deref().unwrap_or_default();
                let crops =
                    render_specs(path, prep.index, s.dpi, specs, self).map_err(Fail::Failed)?;
                run_requests(ctx, path, prep, s, &crops, false, false)
            }
            other => other,
        }?;
        if let Some(bates) = ctx.corner_bates(path, prep, &out.body, |crop| {
            corner(ctx, path, prep, s, crop)
        })? {
            out.bates = Some(bates);
        }
        Ok(out)
    }
}

/// Enough for a front matter and a short line.
const CORNER_MAX_TOKENS: u32 = 256;

/// One Bates corner (`Ctx::corner_bates`). olmOCR drops a whole footer strip as page furniture,
/// but reads a corner on its own (docs/spike.md, "olmOCR footer strip").
fn corner(
    ctx: &Ctx,
    path: &Path,
    prep: &Prepared,
    s: &Settings,
    crop: [f64; 4],
) -> Result<Option<String>, Fail> {
    let png = vision::render_png_fit(path, prep.index, s.dpi, Some(crop), Some(MAX_SIDE), 0)
        .map_err(|e| Fail::Failed(e.to_string()))?;
    let reply = ctx.send(
        &Planned::new(Prompt::Olmocr, &png, true),
        CORNER_MAX_TOKENS,
        None,
    )?;
    Ok(markdown::find_bates_in_text(&parse_reply(&reply.text).body))
}

fn run_requests(
    ctx: &Ctx,
    path: &Path,
    prep: &Prepared,
    s: &Settings,
    reqs: &[Planned],
    full_page: bool,
    crop_alt: bool,
) -> Result<Output, Fail> {
    let mut out = Output {
        body: String::new(),
        table: false,
        flagged: false,
        bates: None,
    };
    let mut parts = Vec::new();
    for req in reqs {
        let (md, table, flagged) = request(ctx, path, prep, s, req, full_page, crop_alt)?;
        out.table |= table;
        out.flagged |= flagged;
        if !md.trim().is_empty() {
            parts.push(md);
        } else if reqs.len() > 1 {
            // A crop that stays empty while others have text: part of the page may be missing.
            out.flagged = true;
        }
    }
    out.body = parts.join("\n\n");
    if out.body.trim().is_empty() {
        return Err(Fail::Failed("empty model output".into()));
    }
    Ok(out)
}

/// One request and its ladder; returns the Markdown, whether a table was converted and whether
/// the page needs a human look.
///
/// - Rotation: with `full_page`, a reply with `is_rotation_valid: False` and a correction is
///   retried once on the page rendered again and turned by the correction, as olmOCR's own
///   pipeline does (cumulative: `(cumulative + correction) % 360`). The turn is
///   **counter-clockwise**: upstream applies it with PIL `Image.transpose(Image.Transpose.ROTATE_90)`
///   (and `ROTATE_180`, `ROTATE_270`), and PIL's ROTATE_90 is a counter-clockwise quarter turn,
///   although the front-matter schema describes the value as "degree of clockwise rotation
///   needed". Source: allenai/olmocr@8ef7f8085aeea5a7b857f2ff4a032f3a8f0fcfa2,
///   `olmocr/pipeline.py`, `build_page_query` (the transpose) and `process_page`
///   (`cumulative_rotation`). Verified on `legal_scan_rot90.png` with the real model.
/// - `length`: retried once with `retry_max_tokens`, then the page falls back to Apple.
/// - Loop (prose only, see `looped`): the crop plan if there is one, else once with a
///   `repetition_penalty`, then Apple.
/// - Empty after the front matter: retried once; the page falls back if everything is empty.
fn request(
    ctx: &Ctx,
    path: &Path,
    prep: &Prepared,
    s: &Settings,
    first: &Planned,
    full_page: bool,
    crop_alt: bool,
) -> Result<(String, bool, bool), Fail> {
    let mut req = first.clone();
    let mut crop_alt = crop_alt;
    let (mut max_tokens, mut penalty) = (ctx.lim.max_tokens, None);
    let (mut rotation, mut rotated) = (0, false);
    let (mut len_retried, mut loop_retried, mut empty_retried) = (false, false, false);
    let mut flagged = false;
    loop {
        let reply = ctx.send(&req, max_tokens, penalty)?;
        if reply.finish == "length" {
            if len_retried {
                return Err(Fail::Failed("output truncated at max_tokens".into()));
            }
            len_retried = true;
            max_tokens = ctx.lim.retry_max_tokens;
            continue;
        }
        let parsed = parse_reply(&reply.text);
        if let Some(front) = &parsed.front
            && full_page
            && front.rotation_valid == Some(false)
        {
            if !rotated && front.rotation_correction != 0 {
                rotated = true;
                rotation = (rotation + front.rotation_correction) % 360;
                crop_alt = false; // crops of a turned page make no sense
                if s.verbose && !s.quiet {
                    crate::progress::line(format!(
                        "page {}: rotation invalid; turned {rotation} degrees and retrying",
                        prep.index + 1
                    ));
                }
                let png =
                    vision::render_png_fit(path, prep.index, s.dpi, None, Some(MAX_SIDE), rotation)
                        .map_err(|e| Fail::Failed(format!("{e:#}")))?;
                req = Planned::new(Prompt::Olmocr, &png, true);
                continue;
            }
            // Still invalid after the turn: use the text, but have a human look.
            flagged |= rotated;
        }
        if looped(&parsed.body) {
            if crop_alt {
                return Err(Fail::Degenerate);
            }
            if loop_retried {
                return Err(Fail::Failed("repetition loop".into()));
            }
            loop_retried = true;
            penalty = Some(LOOP_PENALTY);
            continue;
        }
        let (md, table) = postprocess(&parsed.body);
        if md.trim().is_empty() && !empty_retried {
            empty_retried = true;
            continue;
        }
        return Ok((md, table, flagged));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_upstream_verbatim() {
        let upstream = include_str!("../spike/upstream/allenai_olmocr.py");
        let start = upstream
            .find("def build_no_anchoring_v4_yaml_prompt")
            .unwrap();
        let body = &upstream[start..];
        let body = &body[..body.find("\n\n\n").unwrap()];
        // The function returns three adjacent string literals; join them as Python does.
        let literal = Regex::new(r#""((?:[^"\\]|\\.)*)""#).unwrap();
        let joined: String = body
            .lines()
            .skip(2)
            .filter_map(|l| literal.captures(l))
            .map(|c| c[1].replace("\\n", "\n"))
            .collect();
        assert_eq!(PROMPT, joined);
    }

    #[test]
    fn front_matter_with_capitalized_booleans() {
        let p = parse_reply(
            "---\nprimary_language: en\nis_rotation_valid: False\nrotation_correction: 90\nis_table: True\nis_diagram: False\n---\nHello\n\nWorld",
        );
        assert_eq!(
            p.front,
            Some(Front {
                rotation_valid: Some(false),
                rotation_correction: 90
            })
        );
        assert_eq!(p.body, "Hello\n\nWorld");
        let p = parse_reply(
            "---\nprimary_language: null\nis_rotation_valid: true\nrotation_correction: 0\nis_table: false\nis_diagram: false\n---\n",
        );
        assert_eq!(p.front.unwrap().rotation_valid, Some(true));
        assert_eq!(p.body, "");
    }

    #[test]
    fn replies_without_or_with_partial_front_matter() {
        // No front matter at all: all of it is body.
        let p = parse_reply("Just text\n---\nmore");
        assert_eq!((p.front, p.body.as_str()), (None, "Just text\n---\nmore"));
        // A horizontal rule that is not front matter.
        let p = parse_reply("---\nNot yaml\n---\nbody");
        assert_eq!(p.front, None);
        assert_eq!(p.body, "---\nNot yaml\n---\nbody");
        // Cut off inside the front matter: no body, unknown validity.
        let p = parse_reply("---\nprimary_language: en\nis_rotation_va");
        assert_eq!(p.front.unwrap().rotation_valid, None);
        assert_eq!(p.body, "");
        // Invalid rotation value is ignored.
        let p = parse_reply("---\nprimary_language: en\nrotation_correction: 45\n---\nx");
        assert_eq!(p.front.unwrap().rotation_correction, 0);
        assert_eq!(
            parse_reply(""),
            Parsed {
                front: None,
                body: String::new()
            }
        );
    }

    #[test]
    fn figures_and_tables() {
        let body = "Intro\n\n![A bar chart\nof sales](page_10_20_300_400.png)\n\n![](page_1_2_3_4.png)\n\n<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\n\nOutro";
        let (md, table) = postprocess(body);
        assert!(table);
        assert_eq!(
            md,
            "Intro\n\n[Figure: A bar chart of sales]\n\n[Figure]\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\nOutro"
        );
        // Ordinary images are left alone.
        let (md, _) = postprocess("![logo](logo.png)");
        assert_eq!(md, "![logo](logo.png)");
    }

    #[test]
    fn tables_with_spans_stay_html() {
        let body =
            "<table><tr><th colspan=\"2\">Totals</th></tr><tr><td>a</td><td>b</td></tr></table>";
        let (md, table) = postprocess(body);
        assert!(!table);
        assert_eq!(md, body);
    }

    #[test]
    fn long_repeating_tables_are_not_loops() {
        let rows: String = (0..60)
            .map(|_| "<tr><td>N/A</td><td>$0.00</td><td>Not applicable</td></tr>\n")
            .collect();
        let table =
            format!("<table>\n<tr><th>Item</th><th>Cost</th><th>Note</th></tr>\n{rows}</table>");
        // The raw markup trips the plain detector, which is why tables are left out of it.
        assert!(has_repetition_loop(&table));
        assert!(!looped(&table));
        assert!(!looped(&format!("Heading\n\n{table}\n\nClosing remarks.")));
        // A table cut off mid-row is not a loop either; truncation catches it.
        assert!(!looped(&format!("{table}\n<tr><td>N/A</td><td>$0.")));
        // Loops in the prose around a table still are.
        let prose = "the same line again\n".repeat(30);
        assert!(looped(&format!("{table}\n\n{prose}")));
    }
}
