//! Lines into a document's structure: running headers and footers, ruled tables, text columns,
//! paragraphs (where one ends, its alignment, indents, line spacing, list label, tabs), and
//! pictures in the flow or floating beside it.

use std::collections::HashMap;

use kurbo::Rect;
use wordcraft_doc::Rgb;

use crate::extract::{Fill, Picture, Rule};
use crate::fontinfo::FontTable;
use crate::lines::{Line, Span, Vert};

/// A page's lines and shapes, as [`crate::lines`] and [`crate::extract`] left them.
pub(crate) struct PageLines {
    pub w: f32,
    pub h: f32,
    pub lines: Vec<Line>,
    pub rules: Vec<Rule>,
    pub fills: Vec<Fill>,
    pub pictures: Vec<Picture>,
    pub invisible_text: bool,
}

/// A paragraph: its lines in order and the column they sit in.
#[derive(Clone, Debug)]
pub(crate) struct Para {
    pub lines: Vec<Line>,
    /// Left and right edges of the text column (or table cell) it is measured against.
    pub col: (f32, f32),
    pub page: usize,
    /// Starts a page the text before it did not fill.
    pub page_break: bool,
    /// Distance from the paragraph (or table, picture) above, points; `None` at the top of a page.
    pub gap_above: Option<f32>,
    /// The baseline of the last line of the text paragraph right above, if that is what is above.
    pub above_base: Option<f32>,
    /// Space to leave below it (before a table or picture), points.
    pub gap_below: f32,
    pub shading: Option<Rgb>,
    /// A rule drawn under it.
    pub rule_below: Option<(f32, Rgb)>,
    /// Pictures floating beside it, anchored here.
    pub floats: Vec<Pic>,
    /// An inline picture making up the paragraph (no lines then).
    pub picture: Option<Pic>,
}

impl Para {
    fn new(lines: Vec<Line>, col: (f32, f32), page: usize) -> Para {
        Para {
            lines,
            col,
            page,
            page_break: false,
            gap_above: None,
            above_base: None,
            gap_below: 0.0,
            shading: None,
            rule_below: None,
            floats: Vec::new(),
            picture: None,
        }
    }

    pub(crate) fn top(&self) -> f32 {
        match (&self.picture, self.lines.first()) {
            (Some(p), _) => p.rect.y0 as f32,
            (None, Some(l)) => l.top(),
            (None, None) => 0.0,
        }
    }

    pub(crate) fn bottom(&self) -> f32 {
        match (&self.picture, self.lines.last()) {
            (Some(p), _) => p.rect.y1 as f32,
            (None, Some(l)) => l.bottom(),
            (None, None) => 0.0,
        }
    }
}

/// A picture placed on a page.
#[derive(Clone, Debug)]
pub(crate) struct Pic {
    pub rect: Rect,
    pub data: std::sync::Arc<Vec<u8>>,
    pub ext: &'static str,
    /// Text runs over it: it goes behind the text.
    pub behind: bool,
}

/// One border of a cell: width and colour; `None` = no line there.
pub(crate) type Edge = Option<(f32, Rgb)>;

#[derive(Clone, Debug)]
pub(crate) struct TCell {
    pub span: usize,
    /// 0 = not merged, 1 = starts a vertical merge, 2 = continues one.
    pub vmerge: u8,
    pub paras: Vec<Para>,
    /// Top, left, bottom, right.
    pub edges: [Edge; 4],
    pub shading: Option<Rgb>,
    /// 0 top, 1 centre, 2 bottom.
    pub valign: u8,
}

/// A table drawn with rules.
#[derive(Clone, Debug)]
pub(crate) struct Tab {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    pub rows: Vec<Vec<TCell>>,
    /// Rows repeated at the top of each page.
    pub header_rows: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum Item {
    Para(Para),
    Table(Tab),
    /// The text that follows starts the next column.
    ColumnBreak,
}

/// A stretch of a page with one column layout.
#[derive(Clone, Debug)]
pub(crate) struct Region {
    /// The columns' left and right edges.
    pub cols: Vec<(f32, f32)>,
    pub items: Vec<Item>,
}

/// Body text box shared by pages of one size: margins in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Margins {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

/// Statistics about the document's text: the usual line pitch for each size.
#[derive(Default)]
pub(crate) struct Stats {
    pitch: HashMap<i32, f32>,
}

impl Stats {
    pub(crate) fn pitch(&self, size: f32) -> Option<f32> {
        self.pitch.get(&((size * 2.0).round() as i32)).copied()
    }
}

/// The usual line pitch for each text size: the commonest distance between consecutive
/// baselines of same-size lines that sit one under the other.
pub(crate) fn stats(pages: &[PageLines]) -> Stats {
    let mut hist: HashMap<i32, HashMap<i32, usize>> = HashMap::new();
    for p in pages {
        let mut ls: Vec<&Line> = p.lines.iter().collect();
        ls.sort_by(|a, b| a.base.total_cmp(&b.base));
        for (i, l) in ls.iter().enumerate() {
            // The next line below that overlaps this one horizontally.
            let below = ls.iter().skip(i + 1).take(12).find(|m| m.base > l.base + 0.5 && m.x0 < l.x1 && m.x1 > l.x0);
            if let Some(m) = below
                && (m.size - l.size).abs() < 0.3
            {
                let d = m.base - l.base;
                // Up to double spacing: further apart is space between paragraphs or rows.
                if d > 0.8 * l.size && d < 2.4 * l.size {
                    *hist.entry((l.size * 2.0).round() as i32).or_default().entry((d * 20.0).round() as i32).or_default() += 1;
                }
            }
        }
    }
    let mut s = Stats::default();
    for (size, h) in hist {
        if let Some((d, _)) = h.iter().max_by_key(|(d, c)| (**c, -**d)) {
            s.pitch.insert(size, *d as f32 / 20.0);
        }
    }
    s
}

/// Round to a value a word processor user would have typed, when one is near.
pub(crate) fn nice(v: f32, tol: f32) -> f32 {
    let mut best = (v * 20.0).round() / 20.0;
    let mut best_d = f32::MAX;
    for step in [72.0, 36.0, 18.0, 9.0, 28.346_457, 14.173_228, 7.086_614] {
        let c = (v / step).round() * step;
        let d = (c - v).abs();
        if d <= tol && d < best_d - 0.01 {
            best = (c * 20.0).round() / 20.0;
            best_d = d;
        }
    }
    best
}

/// Margins for pages of one size, from where their body text sits.
pub(crate) fn margins(pages: &[&PageLines], fonts: &FontTable, stats: &Stats) -> Margins {
    let (w, h) = pages.first().map_or((612.0, 792.0), |p| (p.w, p.h));
    let mut x0s: Vec<f32> = Vec::new();
    let mut x1s: Vec<f32> = Vec::new();
    let mut tops: Vec<f32> = Vec::new();
    let mut bottoms: Vec<(f32, f32)> = Vec::new();
    for p in pages {
        let mut body: Vec<&Line> = p.lines.iter().collect();
        body.sort_by(|a, b| a.base.total_cmp(&b.base));
        // Where each line starts: a label hanging left of its text starts the line.
        let mut starts: HashMap<usize, (f32, usize)> = HashMap::new();
        for l in &body {
            let e = starts.entry(l.band).or_insert((f32::MAX, 0));
            e.0 = e.0.min(l.x0);
            e.1 += l.chars();
            x1s.push(l.x1);
        }
        x0s.extend(starts.values().filter(|(_, n)| *n >= 12).map(|(x, _)| *x));
        let mut edges: Vec<(f32, f32, f32)> = body.iter().map(|l| (l.top(), l.bottom(), l.size)).collect();
        for pic in &p.pictures {
            edges.push((pic.rect.y0 as f32, pic.rect.y1 as f32, 0.0));
        }
        if let Some(first) = body.first() {
            let pitch = stats.pitch(first.size).unwrap_or(1.2 * first.size);
            let f = fonts.get(first.font);
            let desc = f.map_or(0.22, |f| f.descent) * first.size;
            tops.push(first.base - (pitch - desc).max(0.6 * first.size));
        }
        if let Some(last) = body.last() {
            let pitch = stats.pitch(last.size).unwrap_or(1.2 * last.size);
            let f = fonts.get(last.font);
            let desc = f.map_or(0.22, |f| f.descent) * last.size;
            bottoms.push((last.base + desc, pitch));
        }
    }
    let mode = |v: &[f32]| -> Option<f32> {
        let mut h: HashMap<i32, usize> = HashMap::new();
        for x in v {
            *h.entry((x * 2.0).round() as i32).or_default() += 1;
        }
        h.into_iter().max_by_key(|(k, c)| (*c, -k)).map(|(k, _)| k as f32 / 2.0)
    };
    // Left: where most lines start, unless lines often start further left (then the smallest
    // start that many lines share).
    let left = {
        let m = mode(&x0s).unwrap_or(72.0);
        let min_common = x0s
            .iter()
            .copied()
            .filter(|x| x0s.iter().filter(|y| (*y - *x).abs() < 0.75).count().max(1) * 25 >= x0s.len().max(3 * 25))
            .fold(m, f32::min);
        nice(min_common, 0.6)
    };
    x1s.sort_by(f32::total_cmp);
    let max_x1 = x1s.get(x1s.len().saturating_sub(1 + x1s.len() / 100)).copied().unwrap_or(w - 72.0);
    let edge_count = x1s.iter().filter(|x| (max_x1 - **x).abs() < 1.0).count();
    let right_room = (w - max_x1).max(0.0);
    let right = if edge_count * 5 >= x1s.len() {
        // Justified text marks the right margin exactly.
        nice(right_room, 0.6)
    } else if right_room >= left - 0.5 && right_room <= left + 30.0 {
        left
    } else {
        nice(right_room, 0.6).min(right_room)
    };
    let top = tops.iter().copied().fold(f32::MAX, f32::min);
    let top = if top == f32::MAX { 72.0 } else { nice(top.max(0.0), 1.0) };
    let (max_bottom, pitch) = bottoms.iter().copied().fold((0.0f32, 12.0f32), |a, b| if b.0 > a.0 { b } else { a });
    let room = (h - max_bottom).max(0.0);
    // The true margin is less than a line below the deepest text: prefer a round value there.
    let bottom = if max_bottom <= 0.0 {
        72.0
    } else {
        let lo = (room - 0.9 * pitch).max(0.0);
        let mut best = None;
        for c in [top, left, 72.0, 54.0, 36.0, 90.0, 56.693, 70.866, 85.039, 42.52] {
            if c >= lo && c <= room + 0.25 && best.is_none_or(|b: f32| (c - room).abs() < (b - room).abs()) {
                best = Some(c);
            }
        }
        best.unwrap_or((room - 0.25 * pitch).max(0.0))
    };
    Margins { top, bottom, left: left.clamp(0.0, w / 2.0), right: right.clamp(0.0, w / 2.0) }
}

// ---------------------------------------------------------------------------------------------
// Ruled tables

#[derive(Clone)]
struct HRule {
    y: f32,
    x0: f32,
    x1: f32,
    w: f32,
    c: Rgb,
}

fn merge_rules(mut v: Vec<HRule>) -> Vec<HRule> {
    v.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut out: Vec<HRule> = Vec::new();
    for r in v {
        if let Some(last) = out.last_mut()
            && (last.y - r.y).abs() < 1.0
            && r.x0 <= last.x1 + 1.5
        {
            last.x1 = last.x1.max(r.x1);
            last.w = last.w.max(r.w);
            continue;
        }
        out.push(r);
    }
    out.into_iter().filter(|r| r.x1 - r.x0 >= 3.0).collect()
}

fn cluster(mut v: Vec<f32>, tol: f32) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, usize)> = Vec::new();
    for x in v {
        match out.last_mut() {
            Some((c, n)) if x - *c / *n as f32 <= tol => {
                *c += x;
                *n += 1;
            }
            _ => out.push((x, 1)),
        }
    }
    out.into_iter().map(|(c, n)| c / n as f32).collect()
}

/// One table's grid: where its columns and rows divide, and the rules that draw it.
struct Grid {
    gh: Vec<HRule>,
    gv: Vec<HRule>,
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// The columns come from the text (a table ruled only across): cells divide where no
    /// line of the row runs over the column boundary, not where a vertical rule is.
    text_cols: bool,
}

/// Grids of crossing horizontal and vertical rules.
fn ruled_grids(hs: &[HRule], vs: &[HRule]) -> Vec<Grid> {
    if hs.len() < 2 || vs.len() < 2 {
        return Vec::new();
    }
    // Connected groups of crossing rules (union-find over horizontals then verticals).
    let n = hs.len() + vs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        let mut guard = 0;
        while let Some(&q) = p.get(i) {
            if q == i || guard > 10_000 {
                break;
            }
            i = q;
            guard += 1;
        }
        i
    }
    for (i, h) in hs.iter().enumerate().take(4000) {
        for (j, v) in vs.iter().enumerate().take(4000) {
            if v.y >= h.x0 - 2.0 && v.y <= h.x1 + 2.0 && h.y >= v.x0 - 2.0 && h.y <= v.x1 + 2.0 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, hs.len() + j));
                if a != b
                    && let Some(slot) = parent.get_mut(a)
                {
                    *slot = b;
                }
            }
        }
    }
    let mut groups: HashMap<usize, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for i in 0..hs.len() {
        groups.entry(find(&mut parent, i)).or_default().0.push(i);
    }
    for j in 0..vs.len() {
        groups.entry(find(&mut parent, hs.len() + j)).or_default().1.push(j);
    }
    let mut out = Vec::new();
    for (_, (hi, vi)) in groups {
        if hi.len() < 2 || vi.len() < 2 {
            continue;
        }
        let gh: Vec<HRule> = hi.iter().filter_map(|i| hs.get(*i)).cloned().collect();
        let gv: Vec<HRule> = vi.iter().filter_map(|i| vs.get(*i)).cloned().collect();
        let mut xs_raw: Vec<f32> = gv.iter().map(|v| v.y).collect();
        let mut ys_raw: Vec<f32> = gh.iter().map(|h| h.y).collect();
        // Open sides: horizontals running past the verticals, or verticals past the horizontals.
        xs_raw.push(gh.iter().map(|h| h.x0).fold(f32::MAX, f32::min));
        xs_raw.push(gh.iter().map(|h| h.x1).fold(f32::MIN, f32::max));
        ys_raw.push(gv.iter().map(|v| v.x0).fold(f32::MAX, f32::min));
        ys_raw.push(gv.iter().map(|v| v.x1).fold(f32::MIN, f32::max));
        out.push(Grid { gh, gv, xs: cluster(xs_raw, 2.0), ys: cluster(ys_raw, 2.0), text_cols: false });
    }
    out
}

/// Tables ruled only across: stacks of horizontal rules of one width, at least two rows, whose
/// text leaves the same vertical gutters free in most rows.
fn across_grids(hs: &[HRule], lines: &[Line], taken: &[(f32, f32, f32, f32)], body: (f32, f32)) -> Vec<Grid> {
    let free: Vec<&HRule> = hs
        .iter()
        .filter(|h| h.y >= body.0 - 2.0 && h.y <= body.1 + 2.0)
        .filter(|h| !taken.iter().any(|t| h.y >= t.2 - 2.0 && h.y <= t.3 + 2.0 && h.x1 > t.0 && h.x0 < t.1))
        .collect();
    // Stacks of rules with the same ends.
    let mut stacks: Vec<Vec<&HRule>> = Vec::new();
    for h in free {
        let found = stacks
            .iter_mut()
            .find(|s| s.last().is_some_and(|l| (l.x0 - h.x0).abs() < 3.0 && (l.x1 - h.x1).abs() < 3.0 && h.y - l.y >= 6.0 && h.y - l.y <= 400.0));
        match found {
            Some(s) => s.push(h),
            None => stacks.push(vec![h]),
        }
    }
    let mut out = Vec::new();
    for stack in stacks {
        for part in [stack] {
            if part.len() < 3 {
                continue;
            }
            let ys: Vec<f32> = part.iter().map(|h| h.y).collect();
            let x0 = part.iter().map(|h| h.x0).fold(f32::MAX, f32::min);
            let x1 = part.iter().map(|h| h.x1).fold(f32::MIN, f32::max);
            let (top, bottom) = (ys.first().copied().unwrap_or(0.0), ys.last().copied().unwrap_or(0.0));
            // The text of each row, as x extents.
            let rows: Vec<Vec<(f32, f32)>> = ys
                .windows(2)
                .map(|w| {
                    lines
                        .iter()
                        .filter(|l| {
                            let cy = l.base - 0.3 * l.size;
                            cy > w[0] && cy < w[1] && l.x0 >= x0 - 2.0 && l.x1 <= x1 + 2.0
                        })
                        .map(|l| (l.x0, l.x1))
                        .collect()
                })
                .collect();
            let nrows = rows.len();
            if rows.iter().filter(|r| !r.is_empty()).count() < 2 {
                continue;
            }
            // Gutters: stretches of x at least 6 pt wide that text covers in at most a quarter
            // of the rows, with text on both sides.
            let allowed = nrows / 4;
            let width = ((x1 - x0).max(0.0) as usize).min(4000);
            let mut cover = vec![0usize; width + 1];
            for r in &rows {
                let mut row_cover = vec![false; width + 1];
                for (a, b) in r {
                    let (ia, ib) = (((a - x0).max(0.0)) as usize, ((b - x0).max(0.0)) as usize);
                    for c in row_cover.iter_mut().take(ib.min(width) + 1).skip(ia.min(width)) {
                        *c = true;
                    }
                }
                for (c, on) in cover.iter_mut().zip(row_cover) {
                    *c += usize::from(on);
                }
            }
            let first_text = rows.iter().flatten().map(|r| r.0).fold(f32::MAX, f32::min);
            let last_text = rows.iter().flatten().map(|r| r.1).fold(f32::MIN, f32::max);
            let mut xs = vec![x0];
            let mut run_start: Option<usize> = None;
            for (i, c) in cover.iter().enumerate() {
                let x = x0 + i as f32;
                let open = *c <= allowed && x > first_text && x < last_text;
                match (open, run_start) {
                    (true, None) => run_start = Some(i),
                    (false, Some(s)) => {
                        run_start = None;
                        // A column starts here only where text in several rows starts.
                        let at = x0 + i as f32;
                        let starts = rows.iter().filter(|r| r.iter().any(|(a, _)| *a >= at - 1.0 && *a <= at + 6.0)).count();
                        if i - s >= 6 && starts >= (nrows / 4).max(2) {
                            // The next column's text starts a cell margin in from its edge.
                            let next = x0 + i as f32;
                            xs.push((next - 5.4).max(x0 + s as f32));
                        }
                    }
                    _ => {}
                }
            }
            xs.push(x1);
            if xs.len() < 3 || xs.len() > 20 {
                continue;
            }
            // Most rows have text in more than one column.
            let multi = rows
                .iter()
                .filter(|r| {
                    let cols: std::collections::HashSet<usize> =
                        r.iter().filter_map(|(a, _)| xs.windows(2).position(|w| *a >= w[0] - 1.0 && *a < w[1])).collect();
                    cols.len() >= 2
                })
                .count();
            if multi * 2 < nrows {
                continue;
            }
            // Rows whose text runs across the columns: at either end, text with a rule between
            // it and the table (a heading, a box above); in the middle, a tall one is text
            // between two tables.
            let inner = xs.get(1..xs.len() - 1).unwrap_or(&[]);
            let crosses: Vec<bool> = rows.iter().map(|r| r.iter().any(|(a, b)| inner.iter().any(|x| *a < x - 2.0 && *b > x + 2.0))).collect();
            let mut heights: Vec<f32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
            heights.sort_by(f32::total_cmp);
            let usual = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
            let mut pieces: Vec<Vec<f32>> = vec![Vec::new()];
            for (r, w) in ys.windows(2).enumerate() {
                let foreign = crosses.get(r).copied().unwrap_or(false) && (r == 0 || r + 1 == nrows || w[1] - w[0] > 2.5 * usual);
                if foreign {
                    pieces.push(Vec::new());
                    continue;
                }
                if let Some(cur) = pieces.last_mut() {
                    if cur.is_empty() {
                        cur.push(w[0]);
                    }
                    cur.push(w[1]);
                }
            }
            let _ = (top, bottom);
            for ys in pieces.into_iter().filter(|p| p.len() >= 3) {
                let (y0, y1) = (ys.first().copied().unwrap_or(0.0), ys.last().copied().unwrap_or(0.0));
                let gh: Vec<HRule> = part.iter().filter(|h| h.y >= y0 - 0.5 && h.y <= y1 + 0.5).map(|h| (*h).clone()).collect();
                out.push(Grid { gh, gv: Vec::new(), xs: xs.clone(), ys, text_cols: true });
            }
        }
    }
    out
}

/// Tables drawn with rules (a full grid, or rules across only); the lines inside them go into
/// their cells.
fn tables(p: &mut PageLines, page: usize, body: (f32, f32), fonts: &FontTable, stats: &Stats) -> Vec<(Tab, f32, f32)> {
    let hs = merge_rules(p.rules.iter().filter(|r| r.horizontal).map(|r| HRule { y: r.a, x0: r.from, x1: r.to, w: r.width, c: r.color }).collect());
    let vs = merge_rules(p.rules.iter().filter(|r| !r.horizontal).map(|r| HRule { y: r.a, x0: r.from, x1: r.to, w: r.width, c: r.color }).collect());
    let mut grids = ruled_grids(&hs, &vs);
    let boxes: Vec<(f32, f32, f32, f32)> = grids
        .iter()
        .map(|g| {
            (
                g.xs.first().copied().unwrap_or(0.0),
                g.xs.last().copied().unwrap_or(0.0),
                g.ys.first().copied().unwrap_or(0.0),
                g.ys.last().copied().unwrap_or(0.0),
            )
        })
        .collect();
    grids.extend(across_grids(&hs, &p.lines, &boxes, body));
    let mut out = Vec::new();
    let mut taken = vec![false; p.lines.len()];
    for Grid { gh, gv, xs, ys, text_cols } in grids {
        let (ncols, nrows) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
        if ncols == 0 || nrows == 0 || ncols > 63 || nrows > 2000 {
            continue;
        }
        let (tx0, tx1, ty0, ty1) = (
            xs.first().copied().unwrap_or(0.0),
            xs.last().copied().unwrap_or(0.0),
            ys.first().copied().unwrap_or(0.0),
            ys.last().copied().unwrap_or(0.0),
        );
        if tx1 - tx0 < 20.0 || ty1 - ty0 < 8.0 {
            continue;
        }
        // Where text runs over a column boundary in a row (tables ruled across).
        let crossed = |x: f32, y0: f32, y1: f32| {
            p.lines.iter().any(|l| {
                let cy = l.base - 0.3 * l.size;
                cy > y0 && cy < y1 && l.x0 < x - 2.0 && l.x1 > x + 2.0
            })
        };
        // A box around a single paragraph (one cell, one or two text lines) is a border, not a table.
        let vrule_at = |x: f32, y: f32| gv.iter().find(|v| (v.y - x).abs() < 2.0 && y >= v.x0 - 1.0 && y <= v.x1 + 1.0).map(|v| (v.w, v.c));
        let hrule_at = |y: f32, x: f32| gh.iter().find(|h| (h.y - y).abs() < 2.0 && x >= h.x0 - 1.0 && x <= h.x1 + 1.0).map(|h| (h.w, h.c));
        let mut rows: Vec<Vec<TCell>> = Vec::with_capacity(nrows);
        let mut starts: Vec<Vec<usize>> = Vec::with_capacity(nrows);
        for r in 0..nrows {
            let (y0, y1) = (ys.get(r).copied().unwrap_or(0.0), ys.get(r + 1).copied().unwrap_or(0.0));
            let ym = (y0 + y1) / 2.0;
            let mut row = Vec::new();
            let mut row_starts = Vec::new();
            let mut c = 0;
            while c < ncols {
                let start = c;
                // Extend over columns whose dividing rule is missing in this row (or, ruled
                // across only, that the row's text runs over).
                while c + 1 < ncols && {
                    let x = xs.get(c + 1).copied().unwrap_or(0.0);
                    if text_cols { crossed(x, y0, y1) } else { vrule_at(x, ym).is_none() }
                } {
                    c += 1;
                }
                let (x0, x1) = (xs.get(start).copied().unwrap_or(0.0), xs.get(c + 1).copied().unwrap_or(0.0));
                let xm = (x0 + x1) / 2.0;
                let edges = [hrule_at(y0, xm), vrule_at(x0, ym), hrule_at(y1, xm), vrule_at(x1, ym)];
                let shading = p
                    .fills
                    .iter()
                    .find(|f| {
                        let r = f.rect.intersect(Rect::new(f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1)));
                        r.area() >= 0.7 * f64::from((x1 - x0) * (y1 - y0))
                    })
                    .map(|f| f.color);
                row.push(TCell { span: c + 1 - start, vmerge: 0, paras: Vec::new(), edges, shading, valign: 0 });
                row_starts.push(start);
                c += 1;
            }
            rows.push(row);
            starts.push(row_starts);
        }
        if ncols == 1 && nrows <= 1 {
            continue;
        }
        // Vertical merges: no rule between a cell and the one below it with the same columns.
        for r in 1..nrows {
            let (above, here) = rows.split_at_mut(r);
            let (Some(prev), Some(cur)) = (above.last_mut(), here.first_mut()) else { continue };
            let (Some(ps), Some(cs)) = (starts.get(r - 1), starts.get(r)) else { continue };
            for (ci, cell) in cur.iter_mut().enumerate() {
                let Some(start) = cs.get(ci) else { continue };
                if let Some(pi) = ps.iter().position(|s| s == start)
                    && let Some(pc) = prev.get_mut(pi)
                    && pc.span == cell.span
                    && pc.edges[2].is_none()
                    && cell.edges[0].is_none()
                {
                    if pc.vmerge == 0 {
                        pc.vmerge = 1;
                    }
                    cell.vmerge = 2;
                }
            }
        }
        // Text into cells.
        let mut cell_lines: HashMap<(usize, usize), Vec<Line>> = HashMap::new();
        for (li, l) in p.lines.iter().enumerate() {
            if taken.get(li).copied().unwrap_or(true) {
                continue;
            }
            let cx = (l.x0 + l.x1.min(l.x0 + l.size)) / 2.0;
            let cy = l.base - 0.3 * l.size;
            if cx < tx0 || cx > tx1 || cy < ty0 || cy > ty1 {
                continue;
            }
            let r = ys.windows(2).position(|w| cy >= w[0] && cy <= w[1]);
            let c = xs.windows(2).position(|w| cx >= w[0] && cx <= w[1]);
            if let (Some(r), Some(c)) = (r, c)
                && let Some(rs) = starts.get(r)
            {
                // The merged cell that covers column `c` in this row; a continued vertical merge
                // gives its text to the cell where the merge starts.
                let mut ci = rs.iter().rposition(|s| *s <= c).unwrap_or(0);
                let mut rr = r;
                while rr > 0 && rows.get(rr).and_then(|row| row.get(ci)).is_some_and(|cell| cell.vmerge == 2) {
                    let start = rs.get(ci).copied().unwrap_or(0);
                    rr -= 1;
                    ci = starts.get(rr).and_then(|s| s.iter().position(|x| *x == start)).unwrap_or(0);
                }
                cell_lines.entry((rr, ci)).or_default().push(l.clone());
                if let Some(t) = taken.get_mut(li) {
                    *t = true;
                }
            }
        }
        for ((r, c), ls) in cell_lines {
            let Some(rs) = starts.get(r) else { continue };
            let start = rs.get(c).copied().unwrap_or(0);
            let Some(cell) = rows.get_mut(r).and_then(|row| row.get_mut(c)) else { continue };
            let x0 = xs.get(start).copied().unwrap_or(0.0);
            let x1 = xs.get(start + cell.span).copied().unwrap_or(x0);
            let (y0, mut y1) = (ys.get(r).copied().unwrap_or(0.0), ys.get(r + 1).copied().unwrap_or(0.0));
            if cell.vmerge == 1 {
                let mut rr = r + 1;
                while let Some(next) =
                    starts.get(rr).and_then(|s| s.iter().position(|x| *x == start)).and_then(|ci| rows.get(rr).and_then(|row| row.get(ci)))
                {
                    if next.vmerge != 2 {
                        break;
                    }
                    y1 = ys.get(rr + 1).copied().unwrap_or(y1);
                    rr += 1;
                }
            }
            let Some(cell) = rows.get_mut(r).and_then(|row| row.get_mut(c)) else { continue };
            let lines = join_tabs(ls, (x0 + 5.4, x1 - 5.4));
            let top_gap = lines.first().map_or(0.0, |l| l.top() - y0);
            let bottom_gap = lines.last().map_or(0.0, |l| y1 - l.bottom());
            cell.valign = if top_gap > 6.0 && (top_gap - bottom_gap).abs() < 3.0 {
                1
            } else if top_gap > 8.0 && bottom_gap < 3.0 {
                2
            } else {
                0
            };
            let mut paras = paragraphs(lines, (x0 + 5.4, x1 - 5.4), page, fonts, stats);
            let mut prev_bottom = y0 + 1.0;
            for para in &mut paras {
                para.gap_above = Some(para.top() - prev_bottom);
                prev_bottom = para.bottom();
            }
            if let Some(first) = paras.first_mut() {
                first.gap_above = None;
            }
            cell.paras = paras;
        }
        out.push((Tab { xs, ys, rows, header_rows: 0 }, ty0, ty1));
    }
    let mut i = 0;
    p.lines.retain(|_| {
        let keep = !taken.get(i).copied().unwrap_or(false);
        i += 1;
        keep
    });
    out
}

// ---------------------------------------------------------------------------------------------
// Columns

/// Two text columns side by side: (left column, right column), if the page has them.
fn two_columns(lines: &[Line], w: f32) -> Option<((f32, f32), (f32, f32))> {
    if lines.len() < 8 {
        return None;
    }
    let allowed = (lines.len() / 30).max(1);
    let mut best: Option<(f32, f32)> = None;
    let mut x = 0.25 * w;
    let mut start: Option<f32> = None;
    while x <= 0.75 * w {
        let crossing = lines.iter().filter(|l| l.x0 < x - 1.0 && l.x1 > x + 1.0).count();
        if crossing <= allowed {
            if start.is_none() {
                start = Some(x);
            }
        } else if let Some(s) = start.take()
            && x - s >= 6.0
            && best.is_none_or(|b| x - s > b.1 - b.0)
        {
            best = Some((s, x));
        }
        x += 1.0;
    }
    if let Some(s) = start
        && x - s >= 6.0
        && best.is_none_or(|b| x - s > b.1 - b.0)
    {
        best = Some((s, x));
    }
    let (g0, g1) = best?;
    let left: Vec<&Line> = lines.iter().filter(|l| l.x1 <= g0 + 1.5).collect();
    let right: Vec<&Line> = lines.iter().filter(|l| l.x0 >= g1 - 1.5).collect();
    if left.len() * 4 < lines.len() || right.len() * 4 < lines.len() {
        return None;
    }
    let lx0 = left.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
    let lx1 = left.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
    let rx0 = right.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
    let rx1 = right.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
    // Columns of running text: most lines fill most of their column (labels and values of a
    // form don't).
    let full = |ls: &[&Line], a: f32, b: f32| ls.iter().filter(|l| l.x1 - l.x0 >= 0.6 * (b - a)).count() * 10 >= ls.len() * 6;
    if !full(&left, lx0, lx1) || !full(&right, rx0, rx1) {
        return None;
    }
    // Side by side, not one under the other.
    let (lt, lb) = (left.iter().map(|l| l.base).fold(f32::MAX, f32::min), left.iter().map(|l| l.base).fold(f32::MIN, f32::max));
    let (rt, rb) = (right.iter().map(|l| l.base).fold(f32::MAX, f32::min), right.iter().map(|l| l.base).fold(f32::MIN, f32::max));
    let overlap = lb.min(rb) - lt.max(rt);
    if overlap < 0.5 * (lb - lt).min(rb - rt) {
        return None;
    }
    Some(((lx0, lx1), (rx0, rx1)))
}

/// Lines of one band within a column become one line with tabs between the pieces.
fn join_tabs(mut lines: Vec<Line>, col: (f32, f32)) -> Vec<Line> {
    lines.sort_by(|a, b| a.band.cmp(&b.band).then(a.x0.total_cmp(&b.x0)));
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for l in lines {
        if let Some(last) = out.last_mut()
            && last.band == l.band
            && l.x0 >= last.x1
            && l.x0 >= col.0 - 2.0
            && l.x1 <= col.1 + 2.0
        {
            let tab = Span {
                text: "\t".into(),
                x0: last.x1,
                x1: l.x0,
                vert: Vert::Base,
                underline: false,
                strike: false,
                link: None,
                back: None,
                note: None,
                ..last.spans.last().cloned().unwrap_or_else(|| l.spans.first().cloned().unwrap_or_else(blank_span))
            };
            last.spans.push(tab);
            last.spans.extend(l.spans);
            last.pieces.push((l.x0, l.x1));
            last.x1 = l.x1;
            last.word_gaps.extend(l.word_gaps);
            continue;
        }
        out.push(l);
    }
    out.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.x0.total_cmp(&b.x0)));
    out
}

fn blank_span() -> Span {
    Span {
        text: String::new(),
        font: 0,
        size: 10.0,
        color: Rgb(0, 0, 0),
        x0: 0.0,
        x1: 0.0,
        vert: Vert::Base,
        underline: false,
        strike: false,
        bold: false,
        link: None,
        back: None,
        note: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Paragraphs

/// A list label at the start of a line: its text and where the text after it starts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Label {
    pub text: String,
    pub bullet: bool,
    pub text_x: f32,
}

const BULLETS: &[char] =
    &['•', '◦', '▪', '▫', '■', '□', '●', '○', '‣', '⁃', '–', '—', '-', '*', '·', '➢', '➤', '►', '✓', '✔', '❖', '◆', '◇', '→', '>', '♦'];

/// Is `tok` an enumerator such as `1.`, `2)`, `(a)`, `iv.`, `A.`, `1.2.`?
pub(crate) fn enumerator(tok: &str) -> bool {
    let t = tok.trim();
    if t.is_empty() || t.chars().count() > 8 {
        return false;
    }
    let inner = t.strip_prefix('(').map_or(t, |r| r.strip_suffix(')').unwrap_or(r));
    let body = inner.trim_end_matches(['.', ')']);
    if body.is_empty() || (body == inner && !t.starts_with('(')) {
        // Needs a closing mark: `1.` or `1)` or `(1)`. A bare `12` is a number.
        return false;
    }
    let decimal = body.split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()));
    // A letter of any alphabet: `a)`, `а)` (Cyrillic), `α)`.
    let letter = body.chars().count() == 1 && body.chars().all(char::is_alphabetic);
    let roman = body.len() <= 6 && body.chars().all(|c| "ivxlcdm".contains(c)) || body.len() <= 6 && body.chars().all(|c| "IVXLCDM".contains(c));
    decimal || letter || roman
}

/// The label a line starts with, if it looks like a list item.
/// A line whose only tab is the one after its list label.
pub(crate) fn label_tabbed(l: &Line) -> bool {
    l.pieces.len() == 2 && label(l).is_some_and(|lb| l.pieces.get(1).is_some_and(|p| (p.0 - lb.text_x).abs() < 1.0))
}

pub(crate) fn label(l: &Line) -> Option<Label> {
    let first = l.spans.first()?;
    let text = l.text();
    let mut chars = text.char_indices();
    let (_, c0) = chars.next()?;
    let tok_end = text.find([' ', '\t']).unwrap_or(text.len());
    let tok = text.get(..tok_end)?;
    let rest = text.get(tok_end..)?.trim_start();
    if rest.is_empty() {
        return None;
    }
    // Where the text after the label starts: the second piece after a tab, else after the
    // first word.
    let text_x =
        if l.pieces.len() > 1 && first.text.trim() == tok.trim() { l.pieces.get(1).map_or(l.first_word_end, |p| p.0) } else { l.first_word_end };
    let bullet = tok.chars().count() == 1 && BULLETS.contains(&c0);
    if bullet {
        // A dash or hyphen is a bullet only with clear room after it.
        if matches!(c0, '-' | '–' | '—' | '*' | '>') && text_x - l.x0 < 0.6 * l.size {
            return None;
        }
        let text_x = if l.pieces.len() > 1 { text_x } else { l.second_word_start };
        return Some(Label { text: tok.to_string(), bullet: true, text_x });
    }
    if enumerator(tok) {
        return Some(Label { text: tok.to_string(), bullet: false, text_x: if l.pieces.len() > 1 { text_x } else { l.second_word_start } });
    }
    // A section number such as `1.1` or `2.3.4` with no closing mark is a label when a tab
    // follows it (in running text it may be a number).
    let tabbed = l.pieces.len() > 1 && first.text.trim() == tok.trim();
    let dotted = tok.contains('.') && tok.split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()));
    if tabbed && dotted && tok.chars().count() <= 8 {
        return Some(Label { text: tok.to_string(), bullet: false, text_x });
    }
    None
}

fn font_line(fonts: &FontTable, l: &Line) -> f32 {
    fonts.get(l.font).map_or(1.15, |f| f.line) * l.size
}

/// Cut a column's lines into paragraphs.
pub(crate) fn paragraphs(lines: Vec<Line>, col: (f32, f32), page: usize, fonts: &FontTable, stats: &Stats) -> Vec<Para> {
    let mut out: Vec<Para> = Vec::new();
    let mut cur: Vec<Line> = Vec::new();
    for l in lines {
        let brk = match cur.last() {
            None => false,
            Some(prev) => breaks_before(&cur, prev, &l, col, fonts, stats),
        };
        if brk && !cur.is_empty() {
            out.push(Para::new(std::mem::take(&mut cur), col, page));
        }
        cur.push(l);
    }
    if !cur.is_empty() {
        out.push(Para::new(cur, col, page));
    }
    out
}

fn centred(l: &Line, col: (f32, f32)) -> bool {
    let c = (l.x0 + l.x1) / 2.0;
    (c - (col.0 + col.1) / 2.0).abs() < 2.5 && l.x0 > col.0 + 12.0
}

/// Does `l` start a new paragraph after the lines in `cur` (ending with `prev`)?
fn breaks_before(cur: &[Line], prev: &Line, l: &Line, col: (f32, f32), fonts: &FontTable, stats: &Stats) -> bool {
    let pitch = l.base - prev.base;
    let expected = if cur.len() >= 2 {
        let mut ps: Vec<f32> = cur.windows(2).map(|w| w[1].base - w[0].base).collect();
        ps.sort_by(f32::total_cmp);
        ps.get(ps.len() / 2).copied().unwrap_or(1.2 * l.size)
    } else {
        stats.pitch(l.size.max(prev.size)).unwrap_or_else(|| font_line(fonts, l).max(font_line(fonts, prev)) * 1.05)
    };
    if pitch > expected * 1.25 + 1.0 || pitch < 0.4 * l.size {
        return true;
    }
    if (l.size - prev.size).abs() > 0.6 {
        return true;
    }
    if label(l).is_some() {
        return true;
    }
    let centred_para = cur.iter().all(|c| centred(c, col)) && centred(l, col);
    // The first word would have fitted at the end of the line above: that line ended its
    // paragraph. Tabbed lines are each their own, but for a list label's tab.
    if (prev.pieces.len() > 1 && !label_tabbed(prev)) || l.pieces.len() > 1 {
        return true;
    }
    let space = fonts.get(l.font).map_or(0.25, |f| f.space) * l.size;
    let first_word = (l.first_word_end - l.x0).max(0.0);
    // A centred line has room on both sides; any other line only at its end. Text set in from
    // both sides of its column (a box) ends where its longest line does.
    let inset = cur.iter().chain(std::iter::once(l)).all(|c| c.x0 >= col.0 + 3.0 && c.x1 <= col.1 - 3.0);
    let right = if inset { cur.iter().chain(std::iter::once(l)).map(|c| c.x1).fold(prev.x1, f32::max) } else { col.1 };
    let room = if centred_para { (col.1 - col.0) - (prev.x1 - prev.x0) } else { right - prev.x1 };
    let fitted = if centred_para { room > first_word + space + 4.0 } else { room > first_word + space + 1.5 };
    if fitted {
        return true;
    }
    // Indents: a paragraph's lines after the first share one left edge.
    if !centred_para {
        let first = cur.first().map_or(l.x0, |f| f.x0);
        if cur.len() >= 2 {
            let cont = cur.get(1).map_or(l.x0, |c| c.x0);
            if (l.x0 - cont).abs() > 2.5 {
                return true;
            }
        } else {
            let hanging_ok = cur.first().and_then(label).is_some_and(|lb| (l.x0 - lb.text_x).abs() < 3.0);
            // The second line further in than the first starts a paragraph of its own, unless
            // it lines up with the text after a list label (a hanging indent).
            if l.x0 > first + 2.0 && !hanging_ok {
                return true;
            }
        }
    }
    false
}

/// Analyse one page into regions of items.
pub(crate) fn page(p: &mut PageLines, page_no: usize, m: &Margins, fonts: &FontTable, stats: &Stats) -> Vec<Region> {
    let body_col = (m.left, p.w - m.right);
    let mut tabs = tables(p, page_no, (m.top, p.h - m.bottom), fonts, stats);
    // Remaining rules: separators under paragraphs.
    let lines = std::mem::take(&mut p.lines);
    // Pictures: a page-sized scan behind an invisible text layer is left out.
    let page_area = f64::from(p.w * p.h);
    let mut pics: Vec<Pic> = p
        .pictures
        .iter()
        .filter(|pic| !(pic.rect.area() > 0.8 * page_area && (p.invisible_text || !lines.is_empty())))
        .map(|pic| Pic { rect: pic.rect, data: pic.data.clone(), ext: pic.ext, behind: false })
        .collect();
    // Columns.
    let cols = two_columns(&lines, p.w);
    let mut regions: Vec<Region> = Vec::new();
    match cols {
        None => {
            let ls = join_tabs(lines, body_col);
            regions.push(Region { cols: vec![body_col], items: flow(ls, body_col, page_no, fonts, stats) });
        }
        Some((lc, rc)) => {
            let gutter = (lc.1, rc.0);
            // Full-width lines split the page into bands.
            let mut ls = lines;
            ls.sort_by(|a, b| a.base.total_cmp(&b.base));
            let mut band: Vec<Line> = Vec::new();
            let mut band_full = None;
            let push = |band: Vec<Line>, full: bool, regions: &mut Vec<Region>| {
                if band.is_empty() {
                    return;
                }
                if full {
                    let ls = join_tabs(band, body_col);
                    regions.push(Region { cols: vec![body_col], items: flow(ls, body_col, page_no, fonts, stats) });
                } else {
                    let (left, right): (Vec<Line>, Vec<Line>) = band.into_iter().partition(|l| l.x1 <= gutter.0 + 1.5 || l.x0 < gutter.0);
                    let mut items = flow(join_tabs(left, lc), lc, page_no, fonts, stats);
                    let right_items = flow(join_tabs(right, rc), rc, page_no, fonts, stats);
                    if !right_items.is_empty() {
                        items.push(Item::ColumnBreak);
                        items.extend(right_items);
                    }
                    regions.push(Region { cols: vec![lc, rc], items });
                }
            };
            for l in ls {
                let full = l.x0 < gutter.0 - 1.0 && l.x1 > gutter.1 + 1.0;
                if band_full.is_some_and(|f| f != full) {
                    push(std::mem::take(&mut band), band_full.unwrap_or(false), &mut regions);
                }
                band_full = Some(full);
                band.push(l);
            }
            push(band, band_full.unwrap_or(false), &mut regions);
        }
    }
    // Tables go into the region and column they sit in, by their top.
    for (tab, ty0, _) in tabs.drain(..) {
        let tx = tab.xs.first().copied().unwrap_or(0.0);
        let target =
            regions.iter_mut().rev().find(|r| region_top(r).is_none_or(|t| t <= ty0 + 1.0) && r.cols.iter().any(|c| tx >= c.0 - 30.0 && tx <= c.1));
        let target = match target {
            Some(t) => t,
            None => {
                if regions.is_empty() {
                    regions.push(Region { cols: vec![body_col], items: Vec::new() });
                }
                match regions.first_mut() {
                    Some(r) => r,
                    None => continue,
                }
            }
        };
        insert_by_top(&mut target.items, Item::Table(tab), ty0);
    }
    // Pictures: inline in the flow when nothing is beside them, else floating.
    let all_lines: Vec<(f32, f32, f32, f32)> =
        regions.iter().flat_map(|r| r.items.iter()).flat_map(item_lines).map(|l| (l.x0, l.x1, l.top(), l.bottom())).collect();
    for mut pic in pics.drain(..) {
        let (x0, x1, y0, y1) = (pic.rect.x0 as f32, pic.rect.x1 as f32, pic.rect.y0 as f32, pic.rect.y1 as f32);
        let over = all_lines.iter().any(|(a, b, t, bt)| *a < x1 && *b > x0 && *t < y1 && *bt > y0);
        let beside = all_lines.iter().any(|(a, b, t, bt)| (*b <= x0 || *a >= x1) && *t < y1 - 2.0 && *bt > y0 + 2.0);
        pic.behind = over;
        let region = regions.iter_mut().find(|r| r.cols.iter().any(|c| x0 < c.1 && x1 > c.0));
        let Some(region) = (match region {
            Some(r) => Some(r),
            None => regions.first_mut(),
        }) else {
            continue;
        };
        if over || beside {
            // Anchor in the paragraph nearest above (or the first on the page).
            let mut anchor: Option<&mut Para> = None;
            for it in region.items.iter_mut() {
                if let Item::Para(pa) = it
                    && (anchor.is_none() || pa.top() <= y0 + 1.0)
                {
                    anchor = Some(pa);
                }
            }
            match anchor {
                Some(pa) => pa.floats.push(pic),
                None => {
                    let col = region.cols.first().copied().unwrap_or(body_col);
                    let mut pa = Para::new(Vec::new(), col, page_no);
                    pa.floats.push(pic);
                    region.items.insert(0, Item::Para(pa));
                }
            }
        } else {
            let col = region.cols.iter().copied().find(|c| x0 < c.1 && x1 > c.0).unwrap_or(body_col);
            let mut pa = Para::new(Vec::new(), col, page_no);
            pa.picture = Some(pic);
            insert_by_top(&mut region.items, Item::Para(pa), y0);
        }
    }
    // Rules under paragraphs (a separator line), shading behind them.
    for region in &mut regions {
        for it in region.items.iter_mut() {
            let Item::Para(pa) = it else { continue };
            if pa.lines.is_empty() {
                continue;
            }
            let (top, bottom) = (pa.top(), pa.bottom());
            let (x0, x1) = (pa.lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min), pa.lines.iter().map(|l| l.x1).fold(f32::MIN, f32::max));
            pa.shading = p
                .fills
                .iter()
                .find(|f| {
                    f.rect.y0 as f32 <= top + 1.0
                        && f.rect.y1 as f32 >= bottom - 1.0
                        && f.rect.x0 as f32 <= x0 + 1.0
                        && f.rect.x1 as f32 >= x1 - 1.0
                        && (f.rect.width() as f32) >= 0.6 * (pa.col.1 - pa.col.0)
                })
                .map(|f| f.color);
            let next_top = bottom + 1.2 * pa.lines.last().map_or(10.0, |l| l.size);
            pa.rule_below = p
                .rules
                .iter()
                .find(|r| {
                    r.horizontal && r.a > bottom - 1.0 && r.a < next_top && r.to - r.from >= 0.6 * (pa.col.1 - pa.col.0) && r.from < x1 && r.to > x0
                })
                .map(|r| (r.width, r.color));
        }
    }
    // Distances between items in each column.
    for region in &mut regions {
        let mut prev_bottom: Option<f32> = None;
        let mut prev_base: Option<f32> = None;
        let mut prev_para: Option<usize> = None;
        let mut i = 0;
        while i < region.items.len() {
            match region.items.get_mut(i) {
                Some(Item::Para(pa)) => {
                    pa.gap_above = prev_bottom.map(|b| pa.top() - b);
                    // Base-to-base distance is what spacing reproduces for text after text.
                    pa.above_base = if pa.picture.is_none() { prev_base } else { None };
                    prev_bottom = Some(pa.bottom());
                    prev_base = if pa.picture.is_none() { pa.lines.last().map(|l| l.base) } else { None };
                    prev_para = Some(i);
                }
                Some(Item::Table(t)) => {
                    let top = t.ys.first().copied().unwrap_or(0.0);
                    let end = t.ys.last().copied().unwrap_or(top);
                    if let (Some(pi), Some(b)) = (prev_para, prev_bottom)
                        && let Some(Item::Para(pp)) = region.items.get_mut(pi)
                    {
                        pp.gap_below = (top - b).max(0.0);
                    }
                    prev_bottom = Some(end);
                    prev_base = None;
                    prev_para = None;
                }
                Some(Item::ColumnBreak) => {
                    prev_bottom = None;
                    prev_base = None;
                    prev_para = None;
                }
                None => break,
            }
            i += 1;
        }
    }
    regions
}

fn region_top(r: &Region) -> Option<f32> {
    r.items.iter().find_map(|it| match it {
        Item::Para(p) => Some(p.top()),
        Item::Table(t) => t.ys.first().copied(),
        Item::ColumnBreak => None,
    })
}

fn item_lines(it: &Item) -> Box<dyn Iterator<Item = &Line> + '_> {
    match it {
        Item::Para(p) => Box::new(p.lines.iter()),
        Item::Table(t) => Box::new(t.rows.iter().flat_map(|r| r.iter()).flat_map(|c| c.paras.iter()).flat_map(|p| p.lines.iter())),
        Item::ColumnBreak => Box::new(std::iter::empty()),
    }
}

/// Insert `item` before the first item (of the column it belongs to) that starts below `top`.
fn insert_by_top(items: &mut Vec<Item>, item: Item, top: f32) {
    let x = match &item {
        Item::Table(t) => t.xs.first().copied(),
        Item::Para(p) => Some(p.col.0),
        Item::ColumnBreak => None,
    };
    // The column the item belongs to: items after the column break are the right column.
    let brk = items.iter().position(|i| matches!(i, Item::ColumnBreak));
    let (from, to) = match (brk, x) {
        (Some(b), Some(x)) => {
            let right_x = items.get(b + 1..).and_then(|r| r.iter().find_map(|i| if let Item::Para(p) = i { Some(p.col.0) } else { None }));
            if right_x.is_some_and(|rx| x >= rx - 20.0) { (b + 1, items.len()) } else { (0, b) }
        }
        _ => (0, items.len()),
    };
    let mut at = to;
    for (i, it) in items.iter().enumerate().take(to).skip(from) {
        let t = match it {
            Item::Para(p) => Some(p.top()),
            Item::Table(t) => t.ys.first().copied(),
            Item::ColumnBreak => None,
        };
        if t.is_some_and(|t| t > top) {
            at = i;
            break;
        }
    }
    items.insert(at.min(items.len()), item);
}

fn flow(lines: Vec<Line>, col: (f32, f32), page: usize, fonts: &FontTable, stats: &Stats) -> Vec<Item> {
    paragraphs(lines, col, page, fonts, stats).into_iter().map(Item::Para).collect()
}

/// Does the paragraph that opens a page or column carry on the one that closed the last?
pub(crate) fn continues(prev: &Para, next: &Para, fonts: &FontTable) -> bool {
    let (Some(last), Some(first)) = (prev.lines.last(), next.lines.first()) else { return false };
    if prev.picture.is_some() || next.picture.is_some() || label(first).is_some() || (last.size - first.size).abs() > 0.6 {
        return false;
    }
    if last.pieces.len() > 1 || first.pieces.len() > 1 {
        return false;
    }
    // Justified lines all end at one edge: a last line short of it ended its paragraph.
    let n = prev.lines.len();
    if n >= 3 {
        let body = prev.lines.get(..n - 1).unwrap_or(&[]);
        let edge = body.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
        if body.iter().all(|l| l.x1 > edge - 0.6) && last.x1 < edge - 1.0 {
            return false;
        }
    }
    // The last line ran to the column's end, and the next starts where its lines start.
    let space = fonts.get(first.font).map_or(0.25, |f| f.space) * first.size;
    let room = prev.col.1 - last.x1;
    if room > (first.first_word_end - first.x0) + space + 1.5 {
        return false;
    }
    let cont_x = if prev.lines.len() >= 2 { prev.lines.get(1).map_or(last.x0, |l| l.x0) } else { label(last).map_or(last.x0, |lb| lb.text_x) };
    // Lines after the first line up; after a one-line part (indented first line) the next
    // line may start further left, back at the paragraph's indent.
    (first.x0 - cont_x).abs() < 2.5 || (prev.lines.len() == 1 && first.x0 <= last.x0 + 0.5 && last.x0 - first.x0 < 40.0)
}
