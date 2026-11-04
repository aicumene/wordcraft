//! Joining what page and column ends cut apart: a paragraph carried on into the next column or
//! page is one paragraph again, a table carried on is one table (its repeated heading row a
//! header row), and a page the text did not fill ends in a page break.

use crate::fontinfo::FontTable;
use crate::reflow::{Item, Margins, Para, Region, Tab, continues};

fn item_bottom(it: &Item) -> Option<f32> {
    match it {
        Item::Para(p) => Some(p.bottom()),
        Item::Table(t) => t.ys.last().copied(),
        Item::ColumnBreak => None,
    }
}

fn item_height(it: &Item) -> f32 {
    match it {
        Item::Para(p) => p.bottom() - p.top(),
        Item::Table(t) => t.ys.last().copied().unwrap_or(0.0) - t.ys.first().copied().unwrap_or(0.0),
        Item::ColumnBreak => 0.0,
    }
}

/// How much of `it` must fit at a page's foot for Word to start it there: two lines of a
/// longer paragraph (one alone would be an orphan), all of a short one (its last alone would
/// be a widow), a table's first row.
fn least_height(it: &Item) -> f32 {
    match it {
        Item::Para(p) if p.lines.len() > 3 => p.lines.get(1).map_or(p.bottom(), |l| l.bottom()) - p.top(),
        Item::Table(t) => t.ys.get(1 + t.header_rows).or(t.ys.last()).copied().unwrap_or(0.0) - t.ys.first().copied().unwrap_or(0.0),
        other => item_height(other),
    }
}

fn row_text(t: &Tab, r: usize) -> String {
    t.rows
        .get(r)
        .map(|row| row.iter().flat_map(|c| c.paras.iter()).flat_map(|p| p.lines.iter()).map(|l| l.text()).collect::<Vec<_>>().join("|"))
        .unwrap_or_default()
}

/// Carry `next` on at the end of `prev`.
fn join_paras(prev: &mut Para, next: Para) {
    prev.lines.extend(next.lines);
    prev.gap_below = next.gap_below;
    prev.rule_below = next.rule_below;
    prev.floats.extend(next.floats);
}

/// Carry the rows of `next` on at the end of `prev`, the heading row the page repeated left out.
fn join_tables(prev: &mut Tab, mut next: Tab) {
    let repeated = !row_text(prev, 0).is_empty() && row_text(prev, 0) == row_text(&next, 0) && next.rows.len() > 1;
    let skip = usize::from(repeated);
    if repeated {
        prev.header_rows = prev.header_rows.max(1);
    }
    let (Some(&end), Some(&start)) = (prev.ys.last(), next.ys.get(skip)) else { return };
    let shift = end - start;
    prev.ys.extend(next.ys.iter().skip(skip + 1).map(|y| y + shift));
    prev.rows.extend(next.rows.drain(..).skip(skip));
}

fn same_grid(a: &Tab, b: &Tab) -> bool {
    a.xs.len() == b.xs.len() && a.xs.iter().zip(&b.xs).all(|(x, y)| (x - y).abs() < 1.5)
}

/// Within a two-column stretch: drop the column break where the text simply ran on (Word
/// moves on by itself), joining a paragraph split across it; keep it where the first column
/// stops short.
fn columns(region: &mut Region, fonts: &FontTable) {
    let Some(k) = region.items.iter().position(|i| matches!(i, Item::ColumnBreak)) else { return };
    let left_bottom = region.items.get(..k).unwrap_or(&[]).iter().filter_map(item_bottom).fold(f32::MIN, f32::max);
    let right_bottom = region.items.get(k + 1..).unwrap_or(&[]).iter().filter_map(item_bottom).fold(f32::MIN, f32::max);
    let pitch = region
        .items
        .get(..k)
        .unwrap_or(&[])
        .iter()
        .rev()
        .find_map(|i| if let Item::Para(p) = i { p.lines.last().map(|l| l.size * 1.2) } else { None })
        .unwrap_or(14.0);
    let joined = matches!(
        (k.checked_sub(1).and_then(|i| region.items.get(i)), region.items.get(k + 1)),
        (Some(Item::Para(a)), Some(Item::Para(b))) if continues(a, b, fonts)
    );
    if joined {
        region.items.remove(k);
        if k < region.items.len()
            && let Item::Para(next) = region.items.remove(k)
            && let Some(Item::Para(prev)) = k.checked_sub(1).and_then(|i| region.items.get_mut(i))
        {
            join_paras(prev, next);
        }
        return;
    }
    if left_bottom + 2.0 * pitch >= right_bottom {
        region.items.remove(k);
    }
}

/// `starts`: the pages that open a new section (its headers or footers change there), where the
/// text before ended and the section break starts the new page.
pub(crate) fn stitch(regions: &mut [Vec<Region>], pages: &[(f32, f32, Margins)], fonts: &FontTable, starts: &[usize]) {
    for r in regions.iter_mut().flatten() {
        columns(r, fonts);
    }
    for i in 1..regions.len() {
        if starts.contains(&i) {
            continue;
        }
        let (before, after) = regions.split_at_mut(i);
        let (Some(prev_page), Some(next_page)) = (before.last_mut(), after.first_mut()) else { continue };
        let same_geometry = match (pages.get(i - 1), pages.get(i)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        let cols_prev = prev_page.last().map_or(1, |r| r.cols.len());
        let cols_next = next_page.first().map_or(1, |r| r.cols.len());
        let Some(next_region) = next_page.iter_mut().find(|r| !r.items.is_empty()) else { continue };
        let Some(prev_region) = prev_page.iter_mut().rev().find(|r| !r.items.is_empty()) else { continue };
        // Carried on?
        let carried = match (prev_region.items.last(), next_region.items.first()) {
            (Some(Item::Para(a)), Some(Item::Para(b))) => continues(a, b, fonts),
            (Some(Item::Table(a)), Some(Item::Table(b))) => same_grid(a, b),
            _ => false,
        };
        if carried {
            let next = next_region.items.remove(0);
            match (prev_region.items.last_mut(), next) {
                (Some(Item::Para(a)), Item::Para(b)) => join_paras(a, b),
                (Some(Item::Table(a)), Item::Table(b)) => join_tables(a, b),
                _ => {}
            }
            continue;
        }
        // A page the text stopped short on, with room for what comes next: a page break.
        if !same_geometry || cols_prev != cols_next {
            continue;
        }
        let Some((_, h, m)) = pages.get(i - 1) else { continue };
        let body_bottom = h - m.bottom;
        let body_h = (h - m.top - m.bottom).max(1.0);
        let used = prev_page.iter().flat_map(|r| r.items.iter()).filter_map(item_bottom).fold(m.top, f32::max);
        let room = body_bottom - used;
        if room < 0.25 * body_h {
            continue;
        }
        // What opens the next page, with a short heading the start of the text it keeps with.
        let mut needed = next_region.items.first().map_or(0.0, least_height);
        if let (Some(Item::Para(h1)), Some(second)) = (next_region.items.first(), next_region.items.get(1))
            && h1.lines.len() <= 2
        {
            needed = h1.bottom() - h1.top() + least_height(second).min(3.0 * h1.lines.first().map_or(12.0, |l| l.size * 1.2));
        }
        if needed + 6.0 > room {
            continue;
        }
        match next_region.items.first_mut() {
            Some(Item::Para(p)) => p.page_break = true,
            Some(Item::Table(t)) => {
                if let Some(p) = t.rows.first_mut().and_then(|r| r.first_mut()).and_then(|c| c.paras.first_mut()) {
                    p.page_break = true;
                }
            }
            _ => {}
        }
    }
}
