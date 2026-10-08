//! Reading order by recursive XY-cut over block bounding boxes (normalized, top-left origin),
//! and the deposition-transcript check.

use std::sync::LazyLock;

use regex::Regex;

use crate::vision::Block;

/// Narrowest x-gap that separates columns, as a fraction of page width.
const MIN_X_GAP: f64 = 0.02;
/// Each side of an x-cut must cover this fraction of the node's height.
const MIN_COLUMN_COVERAGE: f64 = 0.30;

/// A page with at least this many `Q.`/`A.` lines is a likely transcript.
const MIN_QA_LINES: usize = 8;
/// Page labels of a multi-up transcript must be at least this far apart (fraction of the page)
/// to count as different grid columns or rows.
const MIN_GRID_SPACING: f64 = 0.2;

static PAGE_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Page \d+$").unwrap());
static QA_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d{1,2}\s+)?[QA]\.\s").unwrap());

/// Block indices in reading order. Vision's own order is ignored.
pub fn reading_order(blocks: &[Block]) -> Vec<usize> {
    let mut order = Vec::new();
    let ids: Vec<usize> = (0..blocks.len()).collect();
    cut(&ids, blocks, &mut order);
    order
}

/// Text lines of a block: paragraph lines, list items, and table rows joined by spaces.
fn lines(b: &Block) -> Vec<String> {
    let mut v: Vec<String> = b
        .text
        .iter()
        .flat_map(|t| t.lines().map(String::from))
        .collect();
    v.extend(
        b.items
            .iter()
            .flatten()
            .map(|i| format!("{} {}", i.marker, i.text)),
    );
    v.extend(b.rows.iter().flatten().map(|r| r.join(" ")));
    v
}

/// Likely deposition transcript: a 2x2 or larger grid of mini-pages (4-up minuscript), each
/// headed by a `Page N` label, or many numbered `Q.`/`A.` lines. Flag only.
pub fn looks_like_transcript(blocks: &[Block]) -> bool {
    let qa = blocks
        .iter()
        .flat_map(lines)
        .filter(|l| QA_LINE.is_match(l))
        .count();
    if qa >= MIN_QA_LINES {
        return true;
    }
    let labels: Vec<(f64, f64)> = blocks
        .iter()
        .filter(|b| {
            lines(b)
                .first()
                .is_some_and(|l| PAGE_LABEL.is_match(l.trim()))
        })
        .map(|b| (b.bbox[0] + b.bbox[2] / 2.0, b.bbox[1] + b.bbox[3] / 2.0))
        .collect();
    let spread = |f: fn(&(f64, f64)) -> f64| {
        let lo = labels.iter().map(f).fold(f64::INFINITY, f64::min);
        let hi = labels.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        hi - lo >= MIN_GRID_SPACING
    };
    labels.len() >= 2 && spread(|l| l.0) && spread(|l| l.1)
}

fn right(b: &Block) -> f64 {
    b.bbox[0] + b.bbox[2]
}

fn bottom(b: &Block) -> f64 {
    b.bbox[1] + b.bbox[3]
}

/// Groups `ids` into runs whose intervals (by `lo`/`hi`) overlap, separated by gaps > `min_gap`.
fn groups(
    ids: &[usize],
    blocks: &[Block],
    lo: fn(&Block) -> f64,
    hi: fn(&Block) -> f64,
    min_gap: f64,
) -> Vec<Vec<usize>> {
    let mut sorted = ids.to_vec();
    sorted.sort_by(|&a, &b| lo(&blocks[a]).total_cmp(&lo(&blocks[b])));
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut end = f64::NEG_INFINITY;
    for id in sorted {
        let b = &blocks[id];
        if out.is_empty() || lo(b) - end > min_gap {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(id);
        end = end.max(hi(b));
    }
    out
}

/// Fraction of the span `[lo, hi]` covered by the union of the blocks' y-intervals.
fn vertical_coverage(ids: &[usize], blocks: &[Block], lo: f64, hi: f64) -> f64 {
    let mut iv: Vec<(f64, f64)> = ids
        .iter()
        .map(|&i| (blocks[i].bbox[1], bottom(&blocks[i])))
        .collect();
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut covered, mut end) = (0.0, f64::NEG_INFINITY);
    for (a, b) in iv {
        covered += (b - a.max(end)).max(0.0);
        end = end.max(b);
    }
    if hi > lo { covered / (hi - lo) } else { 1.0 }
}

fn cut(ids: &[usize], blocks: &[Block], order: &mut Vec<usize>) {
    if ids.len() > 1 {
        let top = ids.iter().map(|&i| blocks[i].bbox[1]).fold(1.0, f64::min);
        let end = ids.iter().map(|&i| bottom(&blocks[i])).fold(0.0, f64::max);
        let xs = groups(ids, blocks, |b| b.bbox[0], right, MIN_X_GAP);
        if xs.len() > 1
            && xs
                .iter()
                .all(|p| vertical_coverage(p, blocks, top, end) >= MIN_COLUMN_COVERAGE)
        {
            for part in &xs {
                cut(part, blocks, order);
            }
            return;
        }
        // Split at the widest y-gap only: cutting at every gap would split a column body into
        // rows whenever a bridging block (a title) hides the gutter at this level.
        if let Some((upper, lower)) = widest_y_split(ids, blocks) {
            cut(&upper, blocks, order);
            cut(&lower, blocks, order);
            return;
        }
    }
    let mut leaf = ids.to_vec();
    leaf.sort_by(|&a, &b| {
        let (a, b) = (&blocks[a], &blocks[b]);
        (a.bbox[1], a.bbox[0])
            .partial_cmp(&(b.bbox[1], b.bbox[0]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order.extend(leaf);
}

/// Splits at the widest positive y-gap no block covers, if there is one.
fn widest_y_split(ids: &[usize], blocks: &[Block]) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut sorted = ids.to_vec();
    sorted.sort_by(|&a, &b| blocks[a].bbox[1].total_cmp(&blocks[b].bbox[1]));
    let (mut end, mut best) = (f64::NEG_INFINITY, (0.0, 0));
    for (n, &id) in sorted.iter().enumerate() {
        let gap = blocks[id].bbox[1] - end;
        if n > 0 && gap > best.0 {
            best = (gap, n);
        }
        end = end.max(bottom(&blocks[id]));
    }
    (best.1 > 0).then(|| (sorted[..best.1].to_vec(), sorted[best.1..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str, x: f64, y: f64, w: f64, h: f64) -> Block {
        Block {
            kind: "paragraph".into(),
            bbox: [x, y, w, h],
            text: Some(name.into()),
            items: None,
            rows: None,
        }
    }

    fn names(blocks: &[Block], order: &[usize]) -> Vec<String> {
        order
            .iter()
            .map(|&i| blocks[i].text.clone().unwrap())
            .collect()
    }

    fn two_columns() -> Vec<Block> {
        let mut blocks = vec![b("title", 0.2, 0.05, 0.6, 0.03)];
        // Listed right column first to prove Vision's order is ignored.
        for i in 0..6 {
            blocks.push(b(
                &format!("R{i}"),
                0.55,
                0.15 + i as f64 * 0.06,
                0.35,
                0.05,
            ));
        }
        for i in 0..6 {
            blocks.push(b(&format!("L{i}"), 0.1, 0.15 + i as f64 * 0.06, 0.35, 0.05));
        }
        blocks
    }

    #[test]
    fn two_columns_under_a_full_width_title() {
        let blocks = two_columns();
        assert_eq!(
            names(&blocks, &reading_order(&blocks)),
            [
                "title", "L0", "L1", "L2", "L3", "L4", "L5", "R0", "R1", "R2", "R3", "R4", "R5"
            ]
        );
    }

    #[test]
    fn legal_page_layout_is_single_column() {
        let blocks = vec![
            b("footer-right", 0.85, 0.95, 0.1, 0.01),
            b("para2", 0.1, 0.30, 0.8, 0.03),
            b("title", 0.3, 0.05, 0.4, 0.03),
            b("list", 0.15, 0.36, 0.5, 0.08),
            b("para1", 0.1, 0.20, 0.8, 0.03),
            b("table", 0.15, 0.46, 0.7, 0.12),
            b("footer", 0.4, 0.95, 0.2, 0.01),
        ];
        assert_eq!(
            names(&blocks, &reading_order(&blocks)),
            [
                "title",
                "para1",
                "para2",
                "list",
                "table",
                "footer",
                "footer-right"
            ]
        );
    }

    #[test]
    fn right_aligned_line_is_not_a_column() {
        let blocks = vec![
            b("a", 0.1, 0.10, 0.5, 0.20),
            b("b", 0.1, 0.35, 0.5, 0.20),
            b("dated", 0.7, 0.60, 0.2, 0.02),
            b("c", 0.1, 0.65, 0.5, 0.10),
        ];
        assert_eq!(
            names(&blocks, &reading_order(&blocks)),
            ["a", "b", "dated", "c"]
        );
    }

    #[test]
    fn transcript_by_qa_lines_or_page_grid() {
        let qa: Vec<Block> = (1..=8)
            .map(|i| {
                b(
                    &format!("{i} Q. Did you attend?"),
                    0.1,
                    0.1 + i as f64 * 0.05,
                    0.5,
                    0.03,
                )
            })
            .collect();
        assert!(looks_like_transcript(&qa));
        assert!(!looks_like_transcript(&qa[..7]));
        // A table of numbered rows counts through its cells.
        let mut table = b("", 0.1, 0.1, 0.8, 0.8);
        table.text = None;
        table.rows = Some(
            (1..=8)
                .map(|i| vec![i.to_string(), "A. Yes, I did.".into()])
                .collect(),
        );
        assert!(looks_like_transcript(&[table]));

        let label = |t: &str, x, y| b(t, x, y, 0.1, 0.02);
        let grid = [
            label("Page 41", 0.2, 0.05),
            label("Page 42", 0.7, 0.05),
            label("Page 43", 0.2, 0.55),
        ];
        assert!(looks_like_transcript(&grid));
        // Labels in one row are not a grid, and body text mentioning a page is not a label.
        assert!(!looks_like_transcript(&grid[..2]));
        assert!(!looks_like_transcript(&[
            label("Page 41", 0.2, 0.05),
            b("See Page 9 of the brief", 0.2, 0.6, 0.5, 0.02),
        ]));
        // Lowercase list markers in ordinary legal text are not Q/A lines.
        let list: Vec<Block> = (0..10)
            .map(|i| b("a. Late delivery", 0.1, 0.1 + i as f64 * 0.05, 0.5, 0.03))
            .collect();
        assert!(!looks_like_transcript(&list));
    }
}
