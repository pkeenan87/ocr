//! Column regions from the rendered page bitmap: binarize, mask tables, drop long thin rules,
//! then recursive XY-cut on the ink map. Vision's own blocks are unreliable here because it
//! merges lines across a gutter when columns share baselines.

use crate::vision::GrayImage;

/// Pixels darker than this are ink.
const INK_BELOW: u8 = 128;
/// Ink runs longer than this fraction of the page side, and at most `RULE_THICKNESS` px thick,
/// are rules (pleading rules, box borders) and are erased.
const RULE_LENGTH: f64 = 0.08;
const RULE_THICKNESS: usize = 4;
/// Narrowest gutter, as a fraction of page width.
const MIN_GUTTER: f64 = 0.012;
/// A gutter may drift this fraction of page width from one strip to the next (skewed scans).
const GUTTER_DRIFT: f64 = 0.005;
const STRIPS: usize = 4;
/// Speckle tolerance: a column or row is empty with at most max(2, this fraction of the span).
const SPECKLE: f64 = 0.01;
/// Each side of an x-cut must cover this fraction of the node's height.
const MIN_COVERAGE: f64 = 0.30;
/// Vertical ink gaps up to this fraction of the page height still count as covered.
const COVERAGE_JOIN: f64 = 0.025;
/// Nodes shorter than this fraction of the page height are never x-cut.
const MIN_NODE_HEIGHT: f64 = 0.15;
/// Empty-row runs shorter than this fraction of the page height are gaps inside a line (the
/// thin band between ascenders and x-height), not between lines.
const LINE_INTERIOR_GAP: f64 = 0.003;
/// A y-cut gap must also be at least this fraction of the page height, so descender
/// variation between ordinary lines never cuts.
const MIN_Y_GAP: f64 = 0.015;
/// Table boxes are masked this fraction of the page larger on each side; Vision's boxes can
/// fall short of the ruled border.
const TABLE_PAD: f64 = 0.01;
/// A y-cut gap must exceed the node's median inter-line gap by this factor.
const Y_GAP_RATIO: f64 = 1.5;
/// An edge strip narrower than this fraction of the page width is a margin (line numbers).
const MARGIN_WIDTH: f64 = 0.08;
/// A margin strip's outer ink edge lies within this fraction of the page edge.
const MARGIN_ZONE: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A child of an x-cut: recognized on its own crop.
    Column,
    /// A full-width stretch between or around column groups: read from the first pass.
    Band,
    /// A narrow edge strip of digits (pleading line numbers): dropped.
    Margin,
}

#[derive(Debug, Clone, Copy)]
pub struct Region {
    /// Normalized `[x, y, w, h]` of the region's ink, origin top-left.
    pub rect: [f64; 4],
    pub kind: Kind,
}

/// Half-open pixel rectangle.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

struct Map {
    w: usize,
    h: usize,
    ink: Vec<bool>,
}

impl Map {
    fn at(&self, x: usize, y: usize) -> bool {
        self.ink[y * self.w + x]
    }

    /// Tight ink bounding box inside `r`.
    fn trim(&self, r: Rect) -> Option<Rect> {
        let mut t = Rect {
            x0: usize::MAX,
            y0: usize::MAX,
            x1: 0,
            y1: 0,
        };
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                if self.at(x, y) {
                    t.x0 = t.x0.min(x);
                    t.x1 = t.x1.max(x + 1);
                    t.y0 = t.y0.min(y);
                    t.y1 = t.y1.max(y + 1);
                }
            }
        }
        (t.x0 < t.x1).then_some(t)
    }

    /// Rows of `r` that hold any ink, as a per-row flag.
    fn inked_rows(&self, r: Rect) -> Vec<bool> {
        (r.y0..r.y1)
            .map(|y| (r.x0..r.x1).any(|x| self.at(x, y)))
            .collect()
    }
}

/// Column regions of a page in reading order. `tables` are Vision's table boxes (normalized,
/// top-left origin), masked as solid ink so a table's column gaps are never gutters.
pub fn detect(img: &GrayImage, tables: &[[f64; 4]]) -> Vec<Region> {
    if img.width == 0 || img.height == 0 || img.pixels.len() < img.width * img.height {
        return Vec::new();
    }
    let mut map = Map {
        w: img.width,
        h: img.height,
        ink: img.pixels[..img.width * img.height]
            .iter()
            .map(|&p| p < INK_BELOW)
            .collect(),
    };
    mask_tables(&mut map, tables);
    remove_rules(&mut map);
    let mut out = Vec::new();
    let page = Rect {
        x0: 0,
        y0: 0,
        x1: map.w,
        y1: map.h,
    };
    cut(&map, page, &mut out);
    out.into_iter()
        .map(|(r, kind)| Region {
            rect: [
                r.x0 as f64 / map.w as f64,
                r.y0 as f64 / map.h as f64,
                (r.x1 - r.x0) as f64 / map.w as f64,
                (r.y1 - r.y0) as f64 / map.h as f64,
            ],
            kind,
        })
        .collect()
}

fn mask_tables(map: &mut Map, tables: &[[f64; 4]]) {
    let (w, h) = (map.w as f64, map.h as f64);
    for t in tables {
        let x0 = ((t[0] - TABLE_PAD) * w).floor().clamp(0.0, w) as usize;
        let x1 = ((t[0] + t[2] + TABLE_PAD) * w).ceil().clamp(0.0, w) as usize;
        let y0 = ((t[1] - TABLE_PAD) * h).floor().clamp(0.0, h) as usize;
        let y1 = ((t[1] + t[3] + TABLE_PAD) * h).ceil().clamp(0.0, h) as usize;
        for y in y0..y1 {
            map.ink[y * map.w + x0..y * map.w + x1].fill(true);
        }
    }
}

/// Erases horizontal and vertical ink runs that are long and thin. Both kinds are found on the
/// original ink, so a box border's corners do not hide each other.
fn remove_rules(map: &mut Map) {
    let (w, h) = (map.w, map.h);
    let long_h = ((w as f64 * RULE_LENGTH) as usize).max(1);
    let long_v = ((h as f64 * RULE_LENGTH) as usize).max(1);
    // Pixels lying on a long run, per direction.
    let mut on_h = vec![false; w * h];
    let mut on_v = vec![false; w * h];
    for y in 0..h {
        for (a, b) in runs((0..w).map(|x| map.at(x, y))) {
            if b - a > long_h {
                (a..b).for_each(|x| on_h[y * w + x] = true);
            }
        }
    }
    for x in 0..w {
        for (a, b) in runs((0..h).map(|y| map.at(x, y))) {
            if b - a > long_v {
                (a..b).for_each(|y| on_v[y * w + x] = true);
            }
        }
    }
    let mut erase = vec![false; w * h];
    // A long horizontal run is a rule only if it is at most RULE_THICKNESS rows thick.
    for x in 0..w {
        for (a, b) in runs((0..h).map(|y| on_h[y * w + x])) {
            if b - a <= RULE_THICKNESS {
                (a..b).for_each(|y| erase[y * w + x] = true);
            }
        }
    }
    for y in 0..h {
        for (a, b) in runs((0..w).map(|x| on_v[y * w + x])) {
            if b - a <= RULE_THICKNESS {
                (a..b).for_each(|x| erase[y * w + x] = true);
            }
        }
    }
    for (ink, e) in map.ink.iter_mut().zip(erase) {
        if e {
            *ink = false;
        }
    }
}

/// Maximal `[start, end)` runs of `true`.
fn runs(flags: impl Iterator<Item = bool>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut n = 0;
    for (i, f) in flags.enumerate() {
        n = i + 1;
        match (f, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, n));
    }
    out
}

/// Prefer an x-cut, else a y-cut, else the node is a leaf band.
fn cut(map: &Map, node: Rect, out: &mut Vec<(Rect, Kind)>) {
    let Some(node) = map.trim(node) else { return };
    if let Some(pieces) = x_cut(map, node) {
        let last = pieces.len() - 1;
        let width = |r: &Rect| (r.x1 - r.x0) as f64 / map.w as f64;
        let is_margin = |i: usize, r: &Rect| {
            width(r) < MARGIN_WIDTH
                && ((i == 0 && (r.x0 as f64) < MARGIN_ZONE * map.w as f64)
                    || (i == last && (r.x1 as f64) > (1.0 - MARGIN_ZONE) * map.w as f64))
        };
        let margins: Vec<bool> = pieces
            .iter()
            .enumerate()
            .map(|(i, r)| is_margin(i, r))
            .collect();
        let body: Vec<Rect> = pieces
            .iter()
            .zip(&margins)
            .filter(|(_, m)| !**m)
            .map(|(r, _)| *r)
            .collect();
        if body.len() == 1 {
            // Only one real piece remains beside the margin strips: look inside it.
            for (r, _) in pieces.iter().zip(&margins).take_while(|(_, m)| **m) {
                out.push((*r, Kind::Margin));
            }
            cut(map, body[0], out);
            for (r, _) in pieces.iter().zip(&margins).skip(1).filter(|(_, m)| **m) {
                out.push((*r, Kind::Margin));
            }
            return;
        }
        for (r, m) in pieces.iter().zip(margins) {
            out.push((*r, if m { Kind::Margin } else { Kind::Column }));
        }
        return;
    }
    if let Some((upper, lower)) = y_cut(map, node) {
        cut(map, upper, out);
        cut(map, lower, out);
        return;
    }
    out.push((node, Kind::Band));
}

/// An x-gutter candidate tracked from strip to strip.
struct Chain {
    /// The latest run with ink on both sides (or the first run, until there is one).
    last: (usize, usize),
    lo: usize,
    hi: usize,
    mid_sum: usize,
    count: usize,
    /// Narrowest run in any strip.
    narrowest: usize,
    /// Some strip had ink on both sides of the run, so it separates something.
    bounded: bool,
}

impl Chain {
    fn new(run: (usize, usize), bounded: bool) -> Self {
        Self {
            last: run,
            lo: run.0,
            hi: run.1,
            mid_sum: run.0 + run.1,
            count: 2,
            narrowest: run.1 - run.0,
            bounded,
        }
    }

    fn extend(&mut self, run: (usize, usize), bounded: bool) {
        // A run open to the node's edge says little about where the gutter drifted to.
        if bounded || !self.bounded {
            self.last = run;
        }
        self.lo = self.lo.max(run.0);
        self.hi = self.hi.min(run.1);
        self.mid_sum += run.0 + run.1;
        self.count += 2;
        self.narrowest = self.narrowest.min(run.1 - run.0);
        self.bounded |= bounded;
    }

    fn cut_x(&self) -> usize {
        if self.lo < self.hi {
            (self.lo + self.hi) / 2
        } else {
            self.mid_sum / self.count
        }
    }
}

/// Splits the node at vertical gutters, tolerating speckle and skew. Returns the pieces (each
/// trimmed to its ink) when two or more survive the coverage test.
fn x_cut(map: &Map, node: Rect) -> Option<Vec<Rect>> {
    let height = node.y1 - node.y0;
    if (height as f64) < MIN_NODE_HEIGHT * map.h as f64 {
        return None;
    }
    let min_gutter = ((MIN_GUTTER * map.w as f64).ceil() as usize).max(1);
    let drift = ((GUTTER_DRIFT * map.w as f64).ceil() as usize).max(1);
    let mut chains: Vec<Chain> = Vec::new();
    for s in 0..STRIPS {
        let (y0, y1) = (
            node.y0 + height * s / STRIPS,
            node.y0 + height * (s + 1) / STRIPS,
        );
        let tol = ((y1 - y0) as f64 * SPECKLE).round().max(2.0) as usize;
        let empty = (node.x0..node.x1).map(|x| (y0..y1).filter(|&y| map.at(x, y)).count() <= tol);
        let found: Vec<((usize, usize), bool)> = runs(empty)
            .into_iter()
            .map(|(a, b)| {
                let run = (a + node.x0, b + node.x0);
                (run, run.0 > node.x0 && run.1 < node.x1)
            })
            .collect();
        if s == 0 {
            chains = found.iter().map(|&(r, b)| Chain::new(r, b)).collect();
            continue;
        }
        chains.retain_mut(|c| {
            let overlap = |r: &(usize, usize)| {
                (r.1.min(c.last.1 + drift)).saturating_sub(r.0.max(c.last.0.saturating_sub(drift)))
            };
            match found.iter().max_by_key(|(r, _)| overlap(r)) {
                Some(&(r, b)) if overlap(&r) > 0 => {
                    c.extend(r, b);
                    true
                }
                _ => false,
            }
        });
    }
    let mut cuts: Vec<usize> = chains
        .iter()
        .filter(|c| c.bounded && c.narrowest >= min_gutter)
        .map(Chain::cut_x)
        .collect();
    cuts.sort_unstable();
    cuts.dedup_by(|b, a| *b - *a < min_gutter);
    if cuts.is_empty() {
        return None;
    }
    loop {
        let mut bounds = vec![node.x0];
        bounds.extend(&cuts);
        bounds.push(node.x1);
        let pieces: Vec<Option<Rect>> = bounds
            .windows(2)
            .map(|b| {
                map.trim(Rect {
                    x0: b[0],
                    x1: b[1],
                    ..node
                })
            })
            .collect();
        // Pieces without ink (a drifting cut) are merged away like thin ones.
        let weak = pieces
            .iter()
            .position(|p| p.is_none_or(|r| coverage(map, r, node) < MIN_COVERAGE));
        match weak {
            None => return (pieces.len() > 1).then(|| pieces.into_iter().flatten().collect()),
            Some(i) => {
                cuts.remove(i.saturating_sub(1));
                if cuts.is_empty() {
                    return None;
                }
            }
        }
    }
}

/// Fraction of the node's height covered by the piece's ink rows, joining small gaps.
fn coverage(map: &Map, piece: Rect, node: Rect) -> f64 {
    let join = (COVERAGE_JOIN * map.h as f64) as usize;
    let rows = map.inked_rows(Rect {
        y0: node.y0,
        y1: node.y1,
        ..piece
    });
    let mut covered = 0;
    let mut last: Option<usize> = None;
    let mut start = 0;
    for (i, &r) in rows.iter().enumerate() {
        if !r {
            continue;
        }
        match last {
            Some(l) if i - l <= join => {}
            Some(l) => {
                covered += l + 1 - start;
                start = i;
            }
            None => start = i,
        }
        last = Some(i);
    }
    if let Some(l) = last {
        covered += l + 1 - start;
    }
    covered as f64 / (node.y1 - node.y0) as f64
}

/// Splits at the widest empty-row gap when it exceeds the median gap by `Y_GAP_RATIO`.
fn y_cut(map: &Map, node: Rect) -> Option<(Rect, Rect)> {
    let tol = (((node.x1 - node.x0) as f64 * SPECKLE).round().max(2.0)) as usize;
    let inked: Vec<bool> = (node.y0..node.y1)
        .map(|y| (node.x0..node.x1).filter(|&x| map.at(x, y)).count() > tol)
        .collect();
    let first = inked.iter().position(|&r| r)?;
    let last = inked.iter().rposition(|&r| r)?;
    let interior = (LINE_INTERIOR_GAP * map.h as f64).ceil() as usize;
    let gaps: Vec<(usize, usize)> = runs(inked[first..=last].iter().map(|&r| !r))
        .into_iter()
        .filter(|g| g.1 - g.0 > interior)
        .collect();
    if gaps.is_empty() {
        return None;
    }
    let mut lens: Vec<usize> = gaps.iter().map(|g| g.1 - g.0).collect();
    lens.sort_unstable();
    let n = lens.len();
    let median = if n % 2 == 1 {
        lens[n / 2] as f64
    } else {
        (lens[n / 2 - 1] + lens[n / 2]) as f64 / 2.0
    };
    let &(a, b) = gaps.iter().max_by_key(|g| g.1 - g.0)?;
    if (b - a) as f64 <= Y_GAP_RATIO * median || ((b - a) as f64) < MIN_Y_GAP * map.h as f64 {
        return None;
    }
    let cut_at = node.y0 + first + (a + b) / 2;
    Some((Rect { y1: cut_at, ..node }, Rect { y0: cut_at, ..node }))
}

/// Crop rectangles (normalized) for each Column region, as `(region index, rect)`. Padded by
/// `pad_x` and `pad_y`, clamped at gutter midpoints with neighbouring columns and margin strips
/// and at midpoints to bands above and below.
pub fn crop_rects(regions: &[Region], pad_x: f64, pad_y: f64) -> Vec<(usize, [f64; 4])> {
    let span = |r: &Region| (r.rect[1], r.rect[1] + r.rect[3]);
    let across = |r: &Region| (r.rect[0], r.rect[0] + r.rect[2]);
    let mut out = Vec::new();
    for (i, c) in regions.iter().enumerate() {
        if c.kind != Kind::Column {
            continue;
        }
        let ((cx0, cx1), (cy0, cy1)) = (across(c), span(c));
        let (mut x0, mut x1) = ((cx0 - pad_x).max(0.0), (cx1 + pad_x).min(1.0));
        let (mut y0, mut y1) = ((cy0 - pad_y).max(0.0), (cy1 + pad_y).min(1.0));
        for o in regions
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, o)| o)
        {
            let ((ox0, ox1), (oy0, oy1)) = (across(o), span(o));
            let overlaps_y = oy0 < cy1 && oy1 > cy0;
            let overlaps_x = ox0 < cx1 && ox1 > cx0;
            match o.kind {
                Kind::Column | Kind::Margin if overlaps_y => {
                    if ox1 <= cx0 {
                        x0 = x0.max((ox1 + cx0) / 2.0);
                    } else if ox0 >= cx1 {
                        x1 = x1.min((cx1 + ox0) / 2.0);
                    }
                }
                Kind::Band => {
                    if oy1 <= cy0 && overlaps_x {
                        y0 = y0.max((oy1 + cy0) / 2.0);
                    } else if oy0 >= cy1 && overlaps_x {
                        y1 = y1.min((cy1 + oy0) / 2.0);
                    }
                }
                _ => {}
            }
        }
        out.push((i, [x0, y0, x1 - x0, y1 - y0]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 850;
    const H: usize = 1100;

    struct Page(GrayImage);

    impl Page {
        fn new() -> Self {
            Self(GrayImage {
                width: W,
                height: H,
                pixels: vec![255; W * H],
            })
        }

        fn ink(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
            for y in y0..y1 {
                for x in x0..x1 {
                    self.0.pixels[y * W + x] = 0;
                }
            }
        }

        /// `n` text-like lines (8 px tall, 12 px pitch, word gaps) between `x0` and `x1`.
        fn lines(&mut self, x0: usize, x1: usize, y0: usize, n: usize, pitch: usize) {
            for i in 0..n {
                let y = y0 + i * pitch;
                let mut x = x0;
                while x < x1 {
                    self.ink(x, y, (x + 40).min(x1), y + 8);
                    x += 46;
                }
            }
        }

        /// One-line rows whose x shifts by `skew` px per 100 rows of page height.
        fn skewed_lines(&mut self, x0: usize, x1: usize, y0: usize, n: usize, skew: f64) {
            for i in 0..n {
                let y = y0 + i * 14;
                let dx = (skew * (y as f64 / 100.0)).round() as usize;
                self.lines(x0 + dx, x1 + dx, y, 1, 14);
            }
        }

        fn speckle(&mut self, x0: usize, x1: usize, y0: usize, y1: usize, every: usize) {
            for y in (y0..y1).step_by(every) {
                self.0.pixels[y * W + x0 + (y * 7) % (x1 - x0)] = 0;
            }
        }

        fn regions(&self, tables: &[[f64; 4]]) -> Vec<Region> {
            detect(&self.0, tables)
        }
    }

    fn kinds(r: &[Region]) -> Vec<Kind> {
        r.iter().map(|r| r.kind).collect()
    }

    fn columns(r: &[Region]) -> Vec<Region> {
        r.iter()
            .filter(|r| r.kind == Kind::Column)
            .copied()
            .collect()
    }

    #[test]
    fn two_columns_with_a_title() {
        let mut p = Page::new();
        p.lines(250, 600, 60, 1, 14);
        p.lines(70, 400, 130, 40, 14);
        p.lines(450, 780, 130, 40, 14);
        let r = p.regions(&[]);
        assert_eq!(kinds(&r), [Kind::Band, Kind::Column, Kind::Column], "{r:?}");
        let c = columns(&r);
        assert!(c[0].rect[0] + c[0].rect[2] < c[1].rect[0]);
    }

    #[test]
    fn unequal_columns_with_a_narrow_gutter() {
        let mut p = Page::new();
        // 62/34 split with a 12 pt (17 px) gutter.
        p.lines(70, 500, 100, 50, 14);
        p.lines(517, 780, 100, 50, 14);
        let r = p.regions(&[]);
        assert_eq!(kinds(&r), [Kind::Column, Kind::Column], "{r:?}");
    }

    #[test]
    fn columns_in_the_lower_half() {
        let mut p = Page::new();
        p.lines(70, 780, 80, 20, 20);
        p.lines(70, 400, 600, 30, 14);
        p.lines(450, 780, 600, 30, 14);
        let r = p.regions(&[]);
        assert_eq!(kinds(&r), [Kind::Band, Kind::Column, Kind::Column], "{r:?}");
        assert!(columns(&r)[0].rect[1] > 0.5);
    }

    #[test]
    fn right_aligned_block_is_not_a_column() {
        let mut p = Page::new();
        p.lines(70, 400, 80, 40, 18);
        // Date and signature lines on the right, well short of 30% of the height.
        p.lines(560, 780, 830, 5, 20);
        let r = p.regions(&[]);
        assert!(columns(&r).is_empty(), "{r:?}");
    }

    #[test]
    fn table_mask_prevents_a_split() {
        let mut p = Page::new();
        p.lines(70, 330, 100, 40, 14);
        p.lines(450, 780, 100, 40, 14);
        assert_eq!(columns(&p.regions(&[])).len(), 2);
        let table = [
            60.0 / W as f64,
            90.0 / H as f64,
            730.0 / W as f64,
            600.0 / H as f64,
        ];
        assert!(columns(&p.regions(&[table])).is_empty());
    }

    #[test]
    fn speckled_gutter_still_splits() {
        let mut p = Page::new();
        p.lines(70, 400, 100, 60, 14);
        p.lines(450, 780, 100, 60, 14);
        p.speckle(405, 445, 100, 940, 40);
        let r = p.regions(&[]);
        assert_eq!(columns(&r).len(), 2, "{r:?}");
    }

    #[test]
    fn skewed_gutter_still_splits() {
        let mut p = Page::new();
        // 0.5 degrees is about 9 px of drift over the page height.
        p.skewed_lines(70, 400, 100, 60, 0.9);
        p.skewed_lines(450, 780, 100, 60, 0.9);
        let r = p.regions(&[]);
        assert_eq!(columns(&r).len(), 2, "{r:?}");
    }

    #[test]
    fn pleading_margin_strip_is_a_margin() {
        let mut p = Page::new();
        // Line numbers at 50-65 px, a long vertical rule, then body text.
        for i in 0..28 {
            p.ink(50, 100 + i * 30, 65, 108 + i * 30);
        }
        p.ink(90, 40, 92, 1060);
        p.lines(120, 780, 100, 56, 15);
        let r = p.regions(&[]);
        assert_eq!(r[0].kind, Kind::Margin, "{r:?}");
        assert!(columns(&r).is_empty(), "{r:?}");
    }

    #[test]
    fn crops_are_padded_and_clamped_at_gutters_and_bands() {
        let band = |y: f64, h: f64| Region {
            rect: [0.1, y, 0.8, h],
            kind: Kind::Band,
        };
        let col = |x: f64| Region {
            rect: [x, 0.2, 0.3, 0.5],
            kind: Kind::Column,
        };
        let regions = [band(0.05, 0.1), col(0.1), col(0.41), band(0.8, 0.1)];
        let crops = crop_rects(&regions, 0.01, 0.02);
        assert_eq!(crops.len(), 2);
        let [x, y, w, h] = crops[0].1;
        assert!((x - 0.09).abs() < 1e-9);
        // The right edge stops at the gutter midpoint (0.4 .. 0.41), short of the 1% pad.
        assert!((x + w - 0.405).abs() < 1e-9);
        // 2% padding fits between the bands and the columns.
        assert!((y - 0.18).abs() < 1e-9 && (y + h - 0.72).abs() < 1e-9);
        // Close bands clamp the padding at the midpoint.
        let regions = [band(0.05, 0.14), col(0.1), col(0.55)];
        let [_, y, _, _] = crop_rects(&regions, 0.01, 0.02)[0].1;
        assert!((y - 0.195).abs() < 1e-9);
    }
}
