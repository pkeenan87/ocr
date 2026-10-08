//! Reading order by recursive XY-cut over block bounding boxes (normalized, top-left origin).

use crate::vision::Block;

/// Narrowest x-gap that separates columns, as a fraction of page width.
const MIN_X_GAP: f64 = 0.02;
/// Each side of an x-cut must cover this fraction of the node's height.
const MIN_COLUMN_COVERAGE: f64 = 0.30;
/// Vision merges same-baseline lines across a gutter into one wide block, so a gutter may be
/// bridged by blocks covering up to this fraction of the body's height.
const MAX_GUTTER_COVER: f64 = 0.45;
/// A column needs at least this many blocks and this much page height.
const MIN_COLUMN_BLOCKS: usize = 3;
const MIN_COLUMN_HEIGHT: f64 = 0.15;
/// Pad column rects by this fraction of the page. Vision reads column crops best with generous
/// white space; padding is clamped to the page, the gutter midpoints and neighbouring bands.
const COLUMN_PAD_X: f64 = 1.0;
const COLUMN_PAD_Y: f64 = 0.10;

/// Block indices in reading order. Vision's own order is ignored.
pub fn reading_order(blocks: &[Block]) -> Vec<usize> {
    let mut order = Vec::new();
    let ids: Vec<usize> = (0..blocks.len()).collect();
    cut(&ids, blocks, &mut order);
    order
}

/// A multi-column body found on a page.
#[derive(Debug)]
pub struct Columns {
    /// Normalized `[x, y, w, h]` of each column, left to right.
    pub rects: Vec<[f64; 4]>,
    /// Blocks inside the column band; to be replaced by re-recognizing the rects.
    pub covered: Vec<usize>,
}

/// Finds side-by-side columns: x-gutters bridged by little of the body, with two or more
/// substantial columns between them. Single-column pages return `None`.
pub fn find_columns(blocks: &[Block]) -> Option<Columns> {
    if blocks.len() < 2 * MIN_COLUMN_BLOCKS {
        return None;
    }
    let all: Vec<usize> = (0..blocks.len()).collect();
    let top = blocks.iter().map(|b| b.bbox[1]).fold(1.0, f64::min);
    let end = blocks.iter().map(bottom).fold(0.0, f64::max);
    let span_lo = blocks.iter().map(|b| b.bbox[0]).fold(1.0, f64::min);
    let span_hi = blocks.iter().map(right).fold(0.0, f64::max);

    // Elementary x-intervals between block edges, merged while barely covered.
    let mut edges: Vec<f64> = blocks.iter().flat_map(|b| [b.bbox[0], right(b)]).collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup();
    let mut gutters: Vec<(f64, f64)> = Vec::new();
    for w in edges.windows(2) {
        let touching: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&i| blocks[i].bbox[0] < w[1] && right(&blocks[i]) > w[0])
            .collect();
        if vertical_coverage(&touching, blocks, top, end) > MAX_GUTTER_COVER {
            continue;
        }
        match gutters.last_mut() {
            Some(g) if (g.1 - w[0]).abs() < 1e-9 => g.1 = w[1],
            _ => gutters.push((w[0], w[1])),
        }
    }
    gutters.retain(|g| g.1 - g.0 >= MIN_X_GAP);
    if gutters.is_empty() {
        return None;
    }

    // Slices between gutters (and the span ends) with the blocks wholly inside them.
    let mut bounds = vec![span_lo];
    for g in &gutters {
        bounds.extend([g.0, g.1]);
    }
    bounds.push(span_hi);
    let mut cols: Vec<(f64, f64, Vec<usize>)> = Vec::new();
    let mid = |g: &(f64, f64)| (g.0 + g.1) / 2.0;
    for (k, pair) in bounds.chunks(2).enumerate() {
        let members: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&i| blocks[i].bbox[0] >= pair[0] - 1e-9 && right(&blocks[i]) <= pair[1] + 1e-9)
            .collect();
        if is_column(&members, blocks) {
            // Rects may reach to the middle of the gutter, never past it.
            let lo = k.checked_sub(1).map_or(0.0, |p| mid(&gutters[p]));
            let hi = gutters.get(k).map_or(1.0, mid);
            cols.push((lo, hi, members));
        }
    }
    if cols.len() < 2 {
        return None;
    }
    let y0 = cols
        .iter()
        .flat_map(|c| &c.2)
        .map(|&i| blocks[i].bbox[1])
        .fold(1.0, f64::min);
    let y1 = cols
        .iter()
        .flat_map(|c| &c.2)
        .map(|&i| bottom(&blocks[i]))
        .fold(0.0, f64::max);
    // Do not pad into a title or footer band outside the columns.
    let mid_y = |b: &Block| b.bbox[1] + b.bbox[3] / 2.0;
    let above = blocks.iter().filter(|b| mid_y(b) < y0).map(bottom);
    let below = blocks.iter().filter(|b| mid_y(b) > y1).map(|b| b.bbox[1]);
    let y0 = (y0 - COLUMN_PAD_Y).max(above.fold(0.0, f64::max).min(y0));
    let y1 = (y1 + COLUMN_PAD_Y).min(below.fold(1.0, f64::min).max(y1));
    let rects: Vec<[f64; 4]> = cols
        .iter()
        .map(|(lo, hi, m)| {
            let x0 = m.iter().map(|&i| blocks[i].bbox[0]).fold(1.0, f64::min);
            let x1 = m.iter().map(|&i| right(&blocks[i])).fold(0.0, f64::max);
            let (x0, x1) = ((x0 - COLUMN_PAD_X).max(*lo), (x1 + COLUMN_PAD_X).min(*hi));
            [x0, y0, x1 - x0, y1 - y0]
        })
        .collect();
    let covered = all
        .into_iter()
        .filter(|&i| {
            let b = &blocks[i];
            let mid = b.bbox[1] + b.bbox[3] / 2.0;
            (y0..=y1).contains(&mid)
                && rects
                    .iter()
                    .any(|r| b.bbox[0] < r[0] + r[2] && right(b) > r[0])
        })
        .collect();
    Some(Columns { rects, covered })
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

fn is_column(ids: &[usize], blocks: &[Block]) -> bool {
    let top = ids.iter().map(|&i| blocks[i].bbox[1]).fold(1.0, f64::min);
    let end = ids.iter().map(|&i| bottom(&blocks[i])).fold(0.0, f64::max);
    ids.len() >= MIN_COLUMN_BLOCKS && end - top >= MIN_COLUMN_HEIGHT
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
        let cols = find_columns(&blocks).unwrap();
        assert_eq!(cols.rects.len(), 2);
        let [x, y, w, _] = cols.rects[0];
        assert!(x < 0.1 && x + w > 0.45 && x + w <= 0.55);
        assert!(y >= 0.05 + 0.03, "title stays outside the columns");
        assert!(!cols.covered.contains(&0));
        assert_eq!(cols.covered.len(), 12);
    }

    #[test]
    fn merged_wide_lines_do_not_hide_the_gutter() {
        let mut blocks = two_columns();
        // Vision merged one row across the gutter.
        blocks.push(b("merged", 0.1, 0.21, 0.8, 0.05));
        blocks.retain(|x| !matches!(x.text.as_deref(), Some("L1" | "R1")));
        let cols = find_columns(&blocks).unwrap();
        assert!(cols.covered.contains(&(blocks.len() - 1)));
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
        assert!(find_columns(&blocks).is_none());
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
        assert!(find_columns(&blocks).is_none());
    }
}
