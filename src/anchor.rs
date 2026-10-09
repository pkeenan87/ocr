//! Transcript line-number re-anchoring (PLAN §8, docs/spike.md "Transcript line-number
//! re-anchoring").
//!
//! Deposition transcripts number every *physical* line 1-25, wrapped continuations included. The
//! models join a wrapped line onto the one above and renumber sequentially, so every number after
//! a wrap is wrong. Here the numbers come from page geometry instead: Vision's line observations
//! give the physical lines and the position of the number column; a robust line fit gives the
//! y-position of all 25 slots; the engine's words are aligned to Vision's words and re-split at
//! the physical line starts. The engine's words get the geometry's numbers. Nothing is invented:
//! when alignment is poor the engine's own text is kept and the page is marked unverified.

use std::sync::LazyLock;

use regex::Regex;

use crate::vision::{self, Line, Options};

/// Resolution of the per-mini-page Vision pass. Vision reads a quadrant crop of a 4-up sheet
/// almost perfectly at 300 DPI; the full sheet at 200 DPI is mostly garbage.
pub const ANCHOR_DPI: u32 = 300;
/// Resolution of the full-sheet pass that finds the `Page N` headers.
const HEADER_DPI: u32 = 200;
/// Slots on a transcript page.
const SLOTS: u32 = 25;
/// Highest number accepted when stripping the engine's own leading numbers.
const MAX_NUMBER: u32 = 30;
/// Fewer fitted numbers than this and the fit is not trusted on its own.
const MIN_FIT_NUMBERS: usize = 5;
/// A number belongs to the fit if it is within this fraction of the pitch of its slot.
const INLIER: f64 = 0.4;
/// Median residual allowed, as a fraction of the pitch.
const MAX_MEDIAN_RESIDUAL: f64 = 0.15;
/// Plausible slot pitch as a fraction of the (region's) page height.
const PITCH_RANGE: (f64, f64) = (0.004, 0.08);
/// Share of a region's Vision words that must be matched: the physical lines are all found in
/// the engine's text.
pub const MIN_COVERAGE: f64 = 0.8;
/// Share of a region's engine words that must be matched. Lower than the Vision side because a
/// model that repeats or invents lines leaves extra words that align to nothing, yet the lines
/// it did read are still placed reliably. Tuned on seeds 3-9 (Paddle): 94.0% lines exact at 0.8,
/// 95.2% at 0.5 (one sheet rescued from "unverified"); unchanged on seeds 0-2.
pub const MIN_ENGINE_COVERAGE: f64 = 0.5;
/// Two words match if their normalized edit similarity is at least this.
const MATCH_SIM: f64 = 0.5;
/// Header labels of a multi-up sheet are different grid rows or columns beyond this spacing.
const GRID_GAP: f64 = 0.15;
/// A cell starts this far above its header (fraction of the page height).
const CELL_PAD_Y: f64 = 0.012;
/// Largest alignment matrix, in cells.
const MAX_CELLS: usize = 4_000_000;

/// `Page 33`, tolerating markdown decoration and Vision's `Fage 33`.
static HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[\W_]*[pf]a[gq]e\s*(\d{1,4})[\W_]*$").unwrap());
static NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2})(?:\s+(.*))?$").unwrap());

/// The number of a `Page N` header line.
pub fn header_number(text: &str) -> Option<u32> {
    HEADER.captures(text.trim()).and_then(|c| c[1].parse().ok())
}

/// One transcript page of a sheet (or the single page of a full-page transcript).
#[derive(Debug, Clone)]
pub struct MiniPage {
    /// The `Page N` number, if read.
    pub number: Option<u32>,
    /// Normalized top-left crop [x, y, w, h] of the sheet.
    pub rect: [f64; 4],
    /// Vision's physical lines of the region, in sheet-normalized coordinates.
    pub lines: Vec<Line>,
}

// ----------------------------------------------------------- locating mini-pages

/// A header found on the sheet: number, centre x, top y.
type Header = (u32, f64, f64);

fn find_headers(lines: &[Line]) -> Vec<Header> {
    lines
        .iter()
        .filter_map(|l| header_number(&l.text).map(|n| (n, l.bbox[0] + l.bbox[2] / 2.0, l.bbox[1])))
        .collect()
}

/// Cluster centres of `values` (gaps of at least `GRID_GAP` separate clusters), ascending.
fn clusters(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    let mut out: Vec<Vec<f64>> = Vec::new();
    for v in values {
        match out.last_mut() {
            Some(c) if v - c[c.len() - 1] < GRID_GAP => c.push(v),
            _ => out.push(vec![v]),
        }
    }
    out.iter()
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .collect()
}

/// Cell rectangles of a multi-up sheet from its headers, in reading order (rows, then columns),
/// each with the number of the header inside it. The grid is completed when a header is missing.
/// `None` unless at least two headers form two or more cells.
fn grid_cells(headers: &[Header]) -> Option<Vec<(Option<u32>, [f64; 4])>> {
    if headers.len() < 2 {
        return None;
    }
    let cols = clusters(headers.iter().map(|h| h.1).collect());
    let rows = clusters(headers.iter().map(|h| h.2).collect());
    if cols.len() * rows.len() < 2 || cols.len() * rows.len() > 9 {
        return None;
    }
    let near = |centres: &[f64], v: f64| {
        (0..centres.len())
            .min_by(|&a, &b| (centres[a] - v).abs().total_cmp(&(centres[b] - v).abs()))
            .unwrap()
    };
    let x_edge = |c: usize| {
        if c == 0 {
            0.0
        } else if c == cols.len() {
            1.0
        } else {
            (cols[c - 1] + cols[c]) / 2.0
        }
    };
    let y_edge = |r: usize| {
        if r == 0 {
            0.0
        } else if r == rows.len() {
            1.0
        } else {
            (rows[r] - CELL_PAD_Y).max(0.0)
        }
    };
    let mut cells = Vec::new();
    for r in 0..rows.len() {
        for c in 0..cols.len() {
            let number = headers
                .iter()
                .find(|h| near(&cols, h.1) == c && near(&rows, h.2) == r)
                .map(|h| h.0);
            let (x0, x1, y0, y1) = (x_edge(c), x_edge(c + 1), y_edge(r), y_edge(r + 1));
            cells.push((number, [x0, y0, x1 - x0, y1 - y0]));
        }
    }
    Some(cells)
}

/// The mini-pages of a transcript sheet: a header pass over the whole sheet finds the `Page N`
/// labels (a 4-up sheet becomes one cell per header), then each cell is recognized on its own
/// at `ANCHOR_DPI`. An empty result means the geometry could not be read; the caller then keeps
/// the engine's text unverified.
pub fn locate(path: &std::path::Path, index: usize, options: &Options) -> Vec<MiniPage> {
    let with_lines = |crop| Options {
        crop,
        lines: true,
        ..options.clone()
    };
    let Ok(sheet) = vision::recognize_page(path, index, HEADER_DPI, &with_lines(None)) else {
        return Vec::new();
    };
    let headers = find_headers(&sheet.lines);
    let cells = grid_cells(&headers).unwrap_or_else(|| {
        let number = (headers.len() == 1).then(|| headers[0].0);
        vec![(number, [0.0, 0.0, 1.0, 1.0])]
    });
    let mut out = Vec::new();
    for (number, rect) in cells {
        let crop = (rect != [0.0, 0.0, 1.0, 1.0]).then_some(rect);
        let Ok(page) = vision::recognize_page(path, index, ANCHOR_DPI, &with_lines(crop)) else {
            return Vec::new();
        };
        let read = page.lines.iter().find_map(|l| header_number(&l.text));
        out.push(MiniPage {
            number: read.or(number),
            rect,
            lines: page.lines,
        });
    }
    out
}

/// Crop rectangles of the mini-pages in `Page N` order, if the sheet is multi-up.
pub fn crops(mini: &[MiniPage]) -> Option<Vec<[f64; 4]>> {
    (mini.len() >= 2).then(|| order(mini).into_iter().map(|i| mini[i].rect).collect())
}

/// Indices of `mini` ordered by `Page N` (position breaks ties and orders unnumbered pages last).
fn order(mini: &[MiniPage]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..mini.len()).collect();
    idx.sort_by_key(|&i| (mini[i].number.unwrap_or(u32::MAX), i));
    idx
}

// ------------------------------------------------------------------ slot grid

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// y = a + b·n; `a` is the position of slot 0, `b` the pitch.
#[derive(Debug, Clone, Copy)]
pub struct SlotGrid {
    pub a: f64,
    pub b: f64,
}

impl SlotGrid {
    pub fn y(&self, n: u32) -> f64 {
        self.a + self.b * n as f64
    }

    /// The nearest slot to `y` (not clamped) and the distance in pitches.
    fn nearest(&self, y: f64) -> (i64, f64) {
        let n = ((y - self.a) / self.b).round();
        (n as i64, ((y - (self.a + self.b * n)) / self.b).abs())
    }
}

/// Median of pairwise slopes and median intercept (Theil-Sen).
fn theil_sen(points: &[(u32, f64)]) -> Option<SlotGrid> {
    let mut slopes = Vec::new();
    for (i, p) in points.iter().enumerate() {
        for q in &points[i + 1..] {
            if p.0 != q.0 {
                slopes.push((q.1 - p.1) / (q.0 as f64 - p.0 as f64));
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    let b = median(&mut slopes);
    let mut offsets: Vec<f64> = points.iter().map(|p| p.1 - b * p.0 as f64).collect();
    Some(SlotGrid {
        a: median(&mut offsets),
        b,
    })
}

fn residual(g: &SlotGrid, p: &(u32, f64)) -> f64 {
    (p.1 - g.y(p.0)).abs() / g.b.abs().max(1e-9)
}

/// Fits y = a + b·n to the detected `(number, y)` pairs, discarding outliers. `None` if fewer
/// than `MIN_FIT_NUMBERS` agree or the fit is poor. Returns the grid and the inlier flags.
pub fn fit_slots(points: &[(u32, f64)]) -> Option<(SlotGrid, Vec<bool>)> {
    let first = theil_sen(points)?;
    if first.b <= 0.0 {
        return None;
    }
    let mut keep: Vec<bool> = points
        .iter()
        .map(|p| residual(&first, p) < INLIER)
        .collect();
    let kept = |keep: &[bool]| -> Vec<(u32, f64)> {
        points
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|(p, _)| *p)
            .collect()
    };
    let grid = theil_sen(&kept(&keep))?;
    keep = points.iter().map(|p| residual(&grid, p) < INLIER).collect();
    let good = kept(&keep);
    let distinct = {
        let mut n: Vec<u32> = good.iter().map(|p| p.0).collect();
        n.sort_unstable();
        n.dedup();
        n.len()
    };
    let mut res: Vec<f64> = good.iter().map(|p| residual(&grid, p)).collect();
    (distinct >= MIN_FIT_NUMBERS
        && grid.b > 0.0
        && median(&mut res) <= MAX_MEDIAN_RESIDUAL
        && (PITCH_RANGE.0..=PITCH_RANGE.1).contains(&grid.b))
    .then_some((grid, keep))
}

/// With fewer than `MIN_FIT_NUMBERS` numbers: the typical line pitch of the text lines
/// (`ys`, any order), placed using the few numbers found. `None` if no consistent pair of
/// numbers anchors it.
pub fn pitch_fallback(ys: &[f64], numbers: &[(u32, f64)]) -> Option<SlotGrid> {
    let mut ys = ys.to_vec();
    ys.sort_by(f64::total_cmp);
    let mut gaps: Vec<f64> = ys
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|g| *g > 0.002)
        .collect();
    if gaps.len() < 4 || numbers.len() < 2 {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    // Gaps over a missed line are multiples of the pitch: use the small ones.
    let cutoff = gaps[gaps.len() / 10] * 1.5;
    let mut small: Vec<f64> = gaps.iter().copied().filter(|g| *g <= cutoff).collect();
    let b = median(&mut small);
    if !(PITCH_RANGE.0..=PITCH_RANGE.1).contains(&b) {
        return None;
    }
    let mut offsets: Vec<f64> = numbers.iter().map(|p| p.1 - b * p.0 as f64).collect();
    let grid = SlotGrid {
        a: median(&mut offsets),
        b,
    };
    let agree = numbers
        .iter()
        .filter(|p| residual(&grid, p) < INLIER)
        .count();
    (agree >= 2 && agree * 2 >= numbers.len()).then_some(grid)
}

// -------------------------------------------------------------- physical lines

/// One physical line: its slot number (if it sits in a slot) and Vision's words.
#[derive(Debug, Clone, PartialEq)]
pub struct PLine {
    pub number: Option<u32>,
    pub words: Vec<String>,
}

/// A region's physical lines in top-to-bottom order, without the `Page N` header.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub lines: Vec<PLine>,
}

/// A slot number, the (x, words) pieces found in it, and the y of the first piece.
type SlotPieces = (u32, Vec<(f64, Vec<String>)>, f64);

struct Item {
    y: f64,
    x: f64,
    /// Right edge.
    r: f64,
    text: String,
    /// Leading 1-25 number and whether the line holds only that number.
    number: Option<(u32, bool)>,
}

/// Slot numbers and physical lines of one region. `None` if the slot grid is not credible or
/// too few lines sit in slots.
pub fn layout(lines: &[Line]) -> Option<Layout> {
    let mut items: Vec<Item> = lines
        .iter()
        .filter(|l| header_number(&l.text).is_none() && !l.text.trim().is_empty())
        .map(|l| {
            let text = l.text.trim().to_string();
            let number = NUMBERED.captures(&text).and_then(|c| {
                let n: u32 = c[1].parse().ok()?;
                (1..=SLOTS)
                    .contains(&n)
                    .then(|| (n, c.get(2).is_none_or(|m| m.as_str().trim().is_empty())))
            });
            Item {
                y: l.bbox[1] + l.bbox[3] / 2.0,
                x: l.bbox[0],
                r: l.bbox[0] + l.bbox[2],
                text,
                number,
            }
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    // A number glued to text (Vision often merges them) starts inside the number column, left
    // of where plain text starts. The column's right edge comes from the stand-alone numbers,
    // else the text start from the lines that carry no number.
    let mut pure: Vec<f64> = items
        .iter()
        .filter(|i| matches!(i.number, Some((_, true))))
        .map(|i| i.r)
        .collect();
    let mut plain: Vec<f64> = items
        .iter()
        .filter(|i| i.number.is_none())
        .map(|i| i.x)
        .collect();
    let limit = if pure.len() >= 3 {
        Some(median(&mut pure) - 0.002)
    } else if plain.len() >= 3 {
        plain.sort_by(f64::total_cmp);
        Some(plain[plain.len() / 4] - 0.004)
    } else {
        None
    };
    if let Some(limit) = limit {
        for i in &mut items {
            if matches!(i.number, Some((_, false))) && i.x >= limit {
                i.number = None;
            }
        }
    }
    let points: Vec<(u32, f64)> = items
        .iter()
        .filter_map(|i| i.number.map(|(n, _)| (n, i.y)))
        .collect();
    let (grid, inliers) = match fit_slots(&points) {
        Some((g, keep)) => (g, keep),
        None => {
            let ys: Vec<f64> = items
                .iter()
                .filter(|i| !matches!(i.number, Some((_, true))))
                .map(|i| i.y)
                .collect();
            let g = pitch_fallback(&ys, &points)?;
            let keep = points.iter().map(|p| residual(&g, p) < INLIER).collect();
            (g, keep)
        }
    };
    // Items whose number agrees with the grid lose it; the rest are plain text.
    let mut inlier = inliers.into_iter();
    let mut slotted: Vec<(Option<u32>, f64, f64, Vec<String>)> = Vec::new();
    for i in &items {
        let mut text = i.text.as_str();
        let mut number = None;
        if let Some((n, pure)) = i.number
            && inlier.next() == Some(true)
        {
            number = Some(n);
            text = if pure {
                ""
            } else {
                NUMBERED
                    .captures(&i.text)
                    .and_then(|c| c.get(2))
                    .map_or("", |m| m.as_str())
            };
        }
        let words: Vec<String> = text.split_whitespace().map(String::from).collect();
        let slot = match number {
            Some(n) => Some(n),
            None => {
                let (n, d) = grid.nearest(i.y);
                ((1..=SLOTS as i64).contains(&n) && d <= INLIER + 0.05).then_some(n as u32)
            }
        };
        if words.is_empty() && slot.is_none() {
            continue;
        }
        slotted.push((slot, i.y, i.x, words));
    }
    // Merge the pieces of one slot left to right; unslotted lines stay on their own.
    slotted.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<(f64, PLine)> = Vec::new();
    let mut by_slot: Vec<SlotPieces> = Vec::new();
    for (slot, y, x, words) in slotted {
        match slot {
            Some(n) => match by_slot.iter_mut().find(|s| s.0 == n) {
                Some(s) => s.1.push((x, words)),
                None => by_slot.push((n, vec![(x, words)], y)),
            },
            None => out.push((
                y,
                PLine {
                    number: None,
                    words,
                },
            )),
        }
    }
    for (n, mut parts, y) in by_slot {
        parts.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.push((
            y,
            PLine {
                number: Some(n),
                words: parts.into_iter().flat_map(|p| p.1).collect(),
            },
        ));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let lines: Vec<PLine> = out
        .into_iter()
        .map(|o| o.1)
        .filter(|l| !l.words.is_empty())
        .collect();
    (lines.iter().filter(|l| l.number.is_some()).count() >= 2).then_some(Layout { lines })
}

// -------------------------------------------------------------------- alignment

/// A word of the engine's text, the engine line it came from and whether it starts that line.
#[derive(Debug, Clone)]
pub struct EWord {
    pub text: String,
    pub line: usize,
    pub line_start: bool,
}

fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    for i in 1..=n {
        let mut cur = vec![i; m + 1];
        for j in 1..=m {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    1.0 - prev[m] as f64 / n.max(m) as f64
}

/// Needleman-Wunsch over precomputed similarities (`sim[i * m + j]`, engine word `i`, Vision
/// word `j`), with the Vision words taken in the order `cols`. Returns the total score and, for
/// every engine word, the position in `cols` it aligns to.
fn needleman_wunsch(sim: &[f64], n: usize, m: usize, cols: &[usize]) -> (f32, Vec<Option<usize>>) {
    const GAP: f32 = -0.6;
    let score = |s: f64| -> f32 {
        if s >= MATCH_SIM {
            (2.0 * s) as f32
        } else {
            (s - 0.5) as f32
        }
    };
    let w = cols.len() + 1;
    let mut dp = vec![0f32; (n + 1) * w];
    let mut from = vec![0u8; (n + 1) * w];
    for i in 1..=n {
        dp[i * w] = i as f32 * GAP;
        from[i * w] = 1;
    }
    for j in 1..w {
        dp[j] = j as f32 * GAP;
        from[j] = 2;
    }
    for i in 1..=n {
        for j in 1..w {
            let diag = dp[(i - 1) * w + j - 1] + score(sim[(i - 1) * m + cols[j - 1]]);
            let up = dp[(i - 1) * w + j] + GAP;
            let left = dp[i * w + j - 1] + GAP;
            let (best, dir) = if diag >= up && diag >= left {
                (diag, 0)
            } else if up >= left {
                (up, 1)
            } else {
                (left, 2)
            };
            dp[i * w + j] = best;
            from[i * w + j] = dir;
        }
    }
    let mut pair: Vec<Option<usize>> = vec![None; n];
    let (mut i, mut j) = (n, w - 1);
    while i > 0 || j > 0 {
        match from[i * w + j] {
            0 => {
                pair[i - 1] = Some(j - 1);
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    (dp[n * w + w - 1], pair)
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn go(rest: &mut Vec<usize>, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if rest.is_empty() {
            out.push(cur.clone());
            return;
        }
        for k in 0..rest.len() {
            let x = rest.remove(k);
            cur.push(x);
            go(rest, cur, out);
            cur.pop();
            rest.insert(k, x);
        }
    }
    let mut out = Vec::new();
    go(&mut (0..n).collect(), &mut Vec::new(), &mut out);
    out
}

/// Most regions whose reading order is searched; more are taken in `Page N` order.
const MAX_PERMUTED: usize = 4;

/// Result of aligning the engine's words to the physical lines of all regions.
#[derive(Debug)]
pub struct Aligned {
    /// The global physical line (over all regions, in `Page N` order) of each engine word.
    pub line_of: Vec<usize>,
    /// Fraction of each region's engine words, and of its Vision words, that matched.
    pub engine_coverage: Vec<f64>,
    pub vision_coverage: Vec<f64>,
}

/// Word-level global alignment (Needleman-Wunsch with fuzzy word similarity) of the engine's
/// words to Vision's words of every region. The engine may read the regions in any order (a
/// 4-up sheet can come out row by row or column by column); the order with the best alignment
/// wins. Every engine word then takes the physical line of the Vision word it aligns to; engine
/// words that align to nothing join the previous line, or the next one if they start a line of
/// the engine's own.
pub fn align(engine: &[EWord], layouts: &[&Layout]) -> Option<Aligned> {
    // Vision words in `Page N` order: (text, global line, region).
    let mut vision: Vec<(&str, usize, usize)> = Vec::new();
    let mut base = 0;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (r, layout) in layouts.iter().enumerate() {
        let start = vision.len();
        for (k, l) in layout.lines.iter().enumerate() {
            vision.extend(l.words.iter().map(|w| (w.as_str(), base + k, r)));
        }
        spans.push((start, vision.len()));
        base += layout.lines.len();
    }
    let (n, m) = (engine.len(), vision.len());
    if n == 0 || m == 0 || (n + 1) * (m + 1) > MAX_CELLS {
        return None;
    }
    let mut sim = vec![0f64; n * m];
    for (i, e) in engine.iter().enumerate() {
        for (j, v) in vision.iter().enumerate() {
            sim[i * m + j] = similarity(&e.text, v.0);
        }
    }
    let orders = if layouts.len() <= MAX_PERMUTED {
        permutations(layouts.len())
    } else {
        vec![(0..layouts.len()).collect()]
    };
    let mut best: Option<(f32, Vec<usize>, Vec<Option<usize>>)> = None;
    for order in orders {
        let cols: Vec<usize> = order.iter().flat_map(|&r| spans[r].0..spans[r].1).collect();
        let (score, pair) = needleman_wunsch(&sim, n, m, &cols);
        if best.as_ref().is_none_or(|b| score > b.0 + 1e-4) {
            best = Some((score, cols, pair));
        }
    }
    let (_, cols, pair) = best?;
    let vis_of = |i: usize| pair[i].map(|j| cols[j]);
    let mut line_of = vec![0usize; n];
    let mut prev: Option<usize> = None;
    let mut pending: Vec<usize> = Vec::new();
    let mut matched_e = vec![0usize; layouts.len()];
    let mut total_e = vec![0usize; layouts.len()];
    let mut matched_v = vec![false; m];
    for i in 0..n {
        match vis_of(i) {
            None => pending.push(i),
            Some(v) => {
                let line = vision[v].1;
                if !pending.is_empty() {
                    let target = match prev {
                        Some(p) if !engine[pending[0]].line_start => p,
                        _ => line,
                    };
                    for &k in &pending {
                        line_of[k] = target;
                    }
                    pending.clear();
                }
                line_of[i] = line;
                prev = Some(line);
                if sim[i * m + v] >= MATCH_SIM {
                    matched_e[vision[v].2] += 1;
                    matched_v[v] = true;
                }
            }
        }
    }
    for &k in &pending {
        line_of[k] = prev.unwrap_or(0);
    }
    // Engine words per region, by the region of the line they landed on.
    let region_of_line: Vec<usize> = layouts
        .iter()
        .enumerate()
        .flat_map(|(r, l)| std::iter::repeat_n(r, l.lines.len()))
        .collect();
    for &l in &line_of {
        total_e[region_of_line[l]] += 1;
    }
    let engine_coverage = (0..layouts.len())
        .map(|r| matched_e[r] as f64 / total_e[r].max(1) as f64)
        .collect();
    let vision_coverage = spans
        .iter()
        .map(|&(a, b)| {
            matched_v[a..b].iter().filter(|x| **x).count() as f64 / (b - a).max(1) as f64
        })
        .collect();
    Some(Aligned {
        line_of,
        engine_coverage,
        vision_coverage,
    })
}

/// The engine's lines (blank lines and `Page N` headers dropped) as words, without their
/// leading line numbers. Numbers are only stripped when most lines carry one (models that drop
/// them can start a line with a figure).
pub fn engine_words(lines: &[&str]) -> Vec<EWord> {
    let numbered = |l: &str| {
        NUMBERED
            .captures(l)
            .and_then(|c| c[1].parse::<u32>().ok())
            .is_some_and(|n| (1..=MAX_NUMBER).contains(&n))
    };
    let strip = lines.iter().filter(|l| numbered(l)).count() * 2 >= lines.len();
    let mut out = Vec::new();
    for (line, l) in lines.iter().enumerate() {
        let body = if strip && numbered(l) {
            NUMBERED
                .captures(l)
                .and_then(|c| c.get(2))
                .map_or("", |m| m.as_str())
        } else {
            l
        };
        for (k, w) in body.split_whitespace().enumerate() {
            out.push(EWord {
                text: w.to_string(),
                line,
                line_start: k == 0,
            });
        }
    }
    out
}

fn format_line(number: Option<u32>, words: &[&str]) -> String {
    match number {
        Some(n) => format!("{n} {}", words.join(" ")),
        None => words.join(" "),
    }
}

/// The region's Vision lines as numbered text (the Apple engine's own reading).
pub fn vision_region(vision_lines: &[Line]) -> Option<String> {
    let layout = layout(vision_lines)?;
    Some(
        layout
            .lines
            .iter()
            .map(|l| {
                let words: Vec<&str> = l.words.iter().map(String::as_str).collect();
                format_line(l.number, &words)
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// ------------------------------------------------------------------- page level

/// A page's text after re-anchoring.
#[derive(Debug)]
pub struct Applied {
    pub body: String,
    /// Every region was re-anchored; otherwise (some) regions keep the engine's text.
    pub anchored: bool,
}

fn join_block(header: Option<u32>, body: &str) -> String {
    match header {
        Some(n) if body.is_empty() => format!("Page {n}"),
        Some(n) => format!("Page {n}\n{body}"),
        None => body.to_string(),
    }
}

/// Re-anchors a transcript page. `engine` is the model's text for the whole page; `None` means
/// Apple, whose text is Vision's own lines. Regions come out in `Page N` order; a region whose
/// alignment is poor keeps the engine's lines (with Apple, any such region fails the page:
/// `None`).
pub fn apply(mini: &[MiniPage], engine: Option<&str>) -> Option<Applied> {
    if mini.is_empty() {
        return None;
    }
    let regions = order(mini);
    let Some(text) = engine else {
        let mut blocks = Vec::new();
        for &i in &regions {
            let body = vision_region(&mini[i].lines)?;
            blocks.push(join_block(mini[i].number, &body));
        }
        return Some(Applied {
            body: blocks.join("\n\n"),
            anchored: true,
        });
    };
    let unverified = || {
        Some(Applied {
            body: text.to_string(),
            anchored: false,
        })
    };
    let engine_header = text.lines().find_map(header_number);
    let raw: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && header_number(l).is_none())
        .collect();
    let words = engine_words(&raw);
    let layouts: Option<Vec<Layout>> = regions.iter().map(|&i| layout(&mini[i].lines)).collect();
    let Some(layouts) = layouts else {
        return unverified();
    };
    let refs: Vec<&Layout> = layouts.iter().collect();
    let Some(aligned) = align(&words, &refs) else {
        return unverified();
    };
    let mut anchored = true;
    let mut blocks = Vec::new();
    let mut base = 0;
    for (r, &i) in regions.iter().enumerate() {
        let lines = &layouts[r].lines;
        let ok = aligned.engine_coverage[r] >= MIN_ENGINE_COVERAGE
            && aligned.vision_coverage[r] >= MIN_COVERAGE;
        let header = if mini.len() == 1 {
            engine_header
        } else {
            mini[i].number
        };
        let mine: Vec<usize> = (0..words.len())
            .filter(|&k| (base..base + lines.len()).contains(&aligned.line_of[k]))
            .collect();
        if ok {
            let mut per: Vec<Vec<&str>> = vec![Vec::new(); lines.len()];
            for &k in &mine {
                per[aligned.line_of[k] - base].push(&words[k].text);
            }
            let body: Vec<String> = lines
                .iter()
                .zip(per)
                .filter(|(_, w)| !w.is_empty())
                .map(|(l, w)| format_line(l.number, &w))
                .collect();
            blocks.push(join_block(header, &body.join("\n")));
        } else {
            // Keep the engine's own lines for this region.
            anchored = false;
            let body = match (mine.first(), mine.last()) {
                (Some(&a), Some(&b)) => raw[words[a].line..=words[b].line].join("\n"),
                _ => String::new(),
            };
            blocks.push(join_block(header, &body));
        }
        base += lines.len();
    }
    Some(Applied {
        body: blocks.join("\n\n"),
        anchored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One region's numbered lines from the engine's lines; `None` if left unverified.
    fn anchor_region(vision: &[Line], engine: &[&str]) -> Option<String> {
        let mini = [MiniPage {
            number: None,
            rect: [0.0, 0.0, 1.0, 1.0],
            lines: vision.to_vec(),
        }];
        let a = apply(&mini, Some(&engine.join("\n")))?;
        a.anchored.then_some(a.body)
    }

    fn line(x: f64, y: f64, text: &str) -> Line {
        Line {
            text: text.into(),
            bbox: [x, y, 0.2, 0.01],
            confidence: None,
            alternatives: vec![],
        }
    }

    /// Slot y of the synthetic pages: slot 1 at 0.06, pitch 0.0157.
    fn y(n: u32) -> f64 {
        0.06 + (n as f64 - 1.0) * 0.0157
    }

    /// A page like the seed0 crop: header, merged "N text" lines, a separate "13" number, a
    /// wrapped continuation (slots 3 and 4), a dropped "7" and a merged "120." misread.
    fn page() -> Vec<Line> {
        let texts = [
            (1, "Q. How many units were delivered on that date?"),
            (2, "A. I believe it was 20,107 units."),
            (3, "Q. Did you ever inspect the conveyor components"),
            (4, "yourself?"),
            (5, "A. I inspected a sample on November 27, 2025."),
            (6, "Q. Was there any written complaint about the"),
            (7, "shipment?"),
            (8, "MR. ELLISON: Take your time, Nancy Donovan."),
            (9, "A. Not that I am aware of."),
            (10, "Q. What is your position there?"),
            (11, "A. I am the Director of Procurement."),
            (12, "Q. Showing you what has been marked as Exhibit 6,"),
            (13, "do you recognize this document?"),
        ];
        let mut v = vec![line(0.23, 0.037, "Page 33")];
        for (n, t) in texts {
            match n {
                4 | 7 => v.push(line(0.09, y(n), t)),
                13 => {
                    v.push(line(0.052, y(n), "13"));
                    v.push(line(0.09, y(n), t));
                }
                _ => v.push(line(0.052, y(n), &format!("{n} {t}"))),
            }
        }
        v.push(line(
            0.052,
            y(14),
            "120. chat was the amount of that invoice?",
        ));
        v
    }

    #[test]
    fn grid_fit_survives_missing_numbers_and_outliers() {
        // Numbers 1..=25 at y = 0.06 + 0.0157 (n-1); drop most, and add two outliers.
        let mut pts: Vec<(u32, f64)> = [1, 2, 3, 5, 6, 8, 9, 10, 12, 14, 20, 25]
            .iter()
            .map(|&n| (n, y(n)))
            .collect();
        pts.push((15, y(3))); // "15 units" printed on line 3
        pts.push((7, y(18)));
        let (g, keep) = fit_slots(&pts).expect("fit");
        assert!((g.b - 0.0157).abs() < 1e-6, "pitch {}", g.b);
        assert!((g.y(25) - y(25)).abs() < 1e-6);
        assert_eq!(keep.iter().filter(|k| !**k).count(), 2);
        assert!(!keep[12] && !keep[13]);
    }

    #[test]
    fn grid_fit_needs_enough_numbers_and_a_good_fit() {
        let few: Vec<(u32, f64)> = [1, 2, 3, 4].iter().map(|&n| (n, y(n))).collect();
        assert!(fit_slots(&few).is_none());
        // Scattered numbers do not fit a line.
        let junk: Vec<(u32, f64)> =
            [(1, 0.5), (2, 0.1), (3, 0.9), (4, 0.2), (5, 0.7), (6, 0.3)].to_vec();
        assert!(fit_slots(&junk).is_none());
    }

    #[test]
    fn pitch_fallback_anchors_on_a_few_numbers() {
        let ys: Vec<f64> = (1..=20).filter(|n| n % 7 != 0).map(y).collect();
        let g = pitch_fallback(&ys, &[(3, y(3)), (9, y(9))]).expect("fallback");
        assert!((g.b - 0.0157).abs() < 1e-6);
        assert!((g.y(12) - y(12)).abs() < 1e-6);
        // One number cannot anchor it; two that disagree cannot either.
        assert!(pitch_fallback(&ys, &[(3, y(3))]).is_none());
        assert!(pitch_fallback(&ys, &[(3, y(3)), (9, y(15))]).is_none());
    }

    #[test]
    fn layout_numbers_wrapped_lines_by_position() {
        let l = layout(&page()).expect("layout");
        let nums: Vec<Option<u32>> = l.lines.iter().map(|p| p.number).collect();
        // Slots 4, 7 and 13 have no number text of their own (or a separate one) and are still
        // numbered; "120. chat..." sits in slot 14 and keeps its text.
        assert_eq!(nums, (1..=14).map(Some).collect::<Vec<_>>());
        assert_eq!(l.lines[3].words, ["yourself?"]);
        assert_eq!(
            l.lines[12].words.join(" "),
            "do you recognize this document?"
        );
        assert_eq!(l.lines[13].words[0], "120.");
        assert_eq!(l.lines[0].words[0], "Q.");
    }

    #[test]
    fn continuation_gets_its_own_number() {
        // The model joined each wrap onto the line above and renumbered sequentially.
        let engine = "1 Q. How many units were delivered on that date?\n\
            2 A. I believe it was 20,107 units.\n\
            3 Q. Did you ever inspect the conveyor components yourself?\n\
            4 A. I inspected a sample on November 27, 2025.\n\
            5 Q. Was there any written complaint about the shipment?\n\
            6 MR. ELLISON: Take your time, Nancy Donovan.\n\
            7 A. Not that I am aware of.\n\
            8 Q. What is your position there?\n\
            9 A. I am the Director of Procurement.\n\
            10 Q. Showing you what has been marked as Exhibit 6, do you recognize this document?";
        let lines: Vec<&str> = engine.lines().collect();
        let mut vision = page();
        vision.pop(); // no slot 14
        let out = anchor_region(&vision, &lines).expect("anchored");
        let want = "1 Q. How many units were delivered on that date?\n\
            2 A. I believe it was 20,107 units.\n\
            3 Q. Did you ever inspect the conveyor components\n\
            4 yourself?\n\
            5 A. I inspected a sample on November 27, 2025.\n\
            6 Q. Was there any written complaint about the\n\
            7 shipment?\n\
            8 MR. ELLISON: Take your time, Nancy Donovan.\n\
            9 A. Not that I am aware of.\n\
            10 Q. What is your position there?\n\
            11 A. I am the Director of Procurement.\n\
            12 Q. Showing you what has been marked as Exhibit 6,\n\
            13 do you recognize this document?";
        assert_eq!(out, want);
    }

    #[test]
    fn engine_without_numbers_still_aligns() {
        let engine = "Q. How many units were delivered on that date?\nA. I believe it was 20,107 units.\nQ. Did you ever inspect the conveyor components yourself?\nA. I inspected a sample on November 27, 2025.\nQ. Was there any written complaint about the shipment?\nMR. ELLISON: Take your time, Nancy Donovan.\nA. Not that I am aware of.\nQ. What is your position there?\nA. I am the Director of Procurement.\nQ. Showing you what has been marked as Exhibit 6, do you recognize this document?";
        let lines: Vec<&str> = engine.lines().collect();
        let mut vision = page();
        vision.pop();
        let out = anchor_region(&vision, &lines).expect("anchored");
        assert!(out.contains("\n4 yourself?\n"), "{out}");
        assert!(out.ends_with("13 do you recognize this document?"), "{out}");
    }

    #[test]
    fn typo_heavy_vision_still_aligns() {
        let mut vision = page();
        vision.pop();
        let typos = [
            ("Q. How many", "l. How rnany"),
            ("delivered", "delivcred"),
            ("believe", "bclieve"),
            ("20,107", "20,1O7"),
            ("conveyor", "convcyor"),
            ("sample", "samp1e"),
            ("complaint", "cornplaint"),
            ("Donovan.", "Donovan"),
            ("Procurement.", "Procurernent"),
            ("Exhibit", "Exhibil"),
        ];
        for l in &mut vision {
            for (a, b) in typos {
                l.text = l.text.replace(a, b);
            }
        }
        let engine = "1 Q. How many units were delivered on that date?\n\
            2 A. I believe it was 20,107 units.\n\
            3 Q. Did you ever inspect the conveyor components yourself?\n\
            4 A. I inspected a sample on November 27, 2025.\n\
            5 Q. Was there any written complaint about the shipment?\n\
            6 MR. ELLISON: Take your time, Nancy Donovan.\n\
            7 A. Not that I am aware of.\n\
            8 Q. What is your position there?\n\
            9 A. I am the Director of Procurement.\n\
            10 Q. Showing you what has been marked as Exhibit 6, do you recognize this document?";
        let lines: Vec<&str> = engine.lines().collect();
        let out = anchor_region(&vision, &lines).expect("anchored");
        // The engine's words, not Vision's typos, with the geometry's numbers.
        assert!(out.contains("\n4 yourself?\n"), "{out}");
        assert!(
            out.starts_with("1 Q. How many units were delivered"),
            "{out}"
        );
        assert!(out.contains("\n7 shipment?\n"), "{out}");
        assert!(out.contains("20,107"), "{out}");
    }

    #[test]
    fn low_coverage_is_unverified() {
        let mut vision = page();
        vision.pop();
        let engine = "1 The quick brown fox jumps over the lazy dog while nothing at all here\n\
            2 resembles the page that Vision read from the scan of this transcript";
        let lines: Vec<&str> = engine.lines().collect();
        assert!(anchor_region(&vision, &lines).is_none());
        // And a page-level apply keeps the engine's text verbatim, marked unverified.
        let mini = [MiniPage {
            number: Some(33),
            rect: [0.0, 0.0, 1.0, 1.0],
            lines: vision,
        }];
        let out = apply(&mini, Some(engine)).unwrap();
        assert!(!out.anchored);
        assert_eq!(out.body, engine);
    }

    #[test]
    fn no_credible_grid_is_unverified() {
        let vision = vec![line(0.1, 0.1, "Q. Hello there"), line(0.1, 0.3, "A. Hi")];
        assert!(layout(&vision).is_none());
    }

    #[test]
    fn mini_pages_are_ordered_by_page_number() {
        let mk = |n: u32, text: &str| {
            let mut v = vec![line(0.1, 0.02, &format!("Page {n}"))];
            for k in 1..=6u32 {
                v.push(line(0.05, y(k), &format!("{k} {text} {k}")));
            }
            MiniPage {
                number: Some(n),
                rect: [0.0, 0.0, 0.5, 0.5],
                lines: v,
            }
        };
        // Positions TL, TR, BL, BR carry pages 41, 43, 42, 44; the engine lists them as 41..44.
        let mini = [
            mk(41, "alpha"),
            mk(43, "gamma"),
            mk(42, "beta"),
            mk(44, "delta"),
        ];
        let engine = ["alpha", "beta", "gamma", "delta"]
            .iter()
            .zip(41..)
            .map(|(t, n)| {
                format!(
                    "Page {n}\n{}",
                    (1..=6)
                        .map(|k| format!("{k} {t} {k}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let got = apply(&mini, Some(&engine)).unwrap();
        assert!(got.anchored, "{}", got.body);
        let heads: Vec<&str> = got.body.lines().filter(|l| l.starts_with("Page")).collect();
        assert_eq!(heads, ["Page 41", "Page 42", "Page 43", "Page 44"]);
        assert!(got.body.contains("Page 42\n1 beta 1"));
        // Apple: the same order from Vision's own lines.
        let apple = apply(&mini, None).unwrap();
        assert!(apple.body.starts_with("Page 41\n1 alpha 1"));
        assert!(apple.body.contains("\n\nPage 43\n1 gamma 1"));
        let crop_order = crops(&mini).unwrap();
        assert_eq!(crop_order.len(), 4);
    }

    #[test]
    fn engine_reading_order_and_missing_headers_do_not_matter() {
        let mk = |n: u32, text: &str| {
            let mut v = vec![line(0.1, 0.02, &format!("Page {n}"))];
            for k in 1..=8u32 {
                v.push(line(
                    0.05,
                    y(k),
                    &format!("{k} {text} line {k} of the page"),
                ));
            }
            MiniPage {
                number: Some(n),
                rect: [0.0, 0.0, 0.5, 0.5],
                lines: v,
            }
        };
        let mini = [
            mk(41, "alpha"),
            mk(43, "gamma"),
            mk(42, "beta"),
            mk(44, "delta"),
        ];
        // The engine read the sheet in position order (41, 43, 42, 44), wrapped every second
        // line onto the one above, numbered sequentially and dropped the headers.
        let engine: String = ["alpha", "gamma", "beta", "delta"]
            .iter()
            .map(|t| {
                (1..=4)
                    .map(|k| {
                        format!(
                            "{k} {t} line {} of the page {t} line {} of the page",
                            2 * k - 1,
                            2 * k
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let got = apply(&mini, Some(&engine)).unwrap();
        assert!(got.anchored, "{}", got.body);
        let heads: Vec<&str> = got.body.lines().filter(|l| l.starts_with("Page")).collect();
        assert_eq!(heads, ["Page 41", "Page 42", "Page 43", "Page 44"]);
        assert!(
            got.body.contains(
                "Page 42\n1 beta line 1 of the page\n2 beta line 2 of the page\n3 beta line 3"
            ),
            "{}",
            got.body
        );
        assert!(
            got.body.ends_with("8 delta line 8 of the page"),
            "{}",
            got.body
        );
    }

    #[test]
    fn headers_tolerate_vision_typos_and_markup() {
        assert_eq!(header_number("Page 33"), Some(33));
        assert_eq!(header_number("Fage 42"), Some(42));
        assert_eq!(header_number("## **Page 7**"), Some(7));
        assert_eq!(header_number("Page 7 of 9"), None);
        assert_eq!(header_number("1 Page 7"), None);
    }

    #[test]
    fn grid_completes_a_missing_header() {
        // Three of four headers found on a 2x2 sheet.
        let headers = [(41, 0.26, 0.04), (42, 0.72, 0.04), (44, 0.72, 0.52)];
        let cells = grid_cells(&headers).unwrap();
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[2].0, None);
        assert_eq!(cells[0].1[0], 0.0);
        assert!((cells[0].1[2] - 0.49).abs() < 1e-9);
        assert!((cells[3].1[1] - 0.508).abs() < 1e-9);
        assert!((cells[3].1[1] + cells[3].1[3] - 1.0).abs() < 1e-9);
        assert!(grid_cells(&[(1, 0.5, 0.05)]).is_none());
    }
}
