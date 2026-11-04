//! Running heads and feet: text and pictures repeated at one place on many pages are the
//! header or footer, not body text. A document may change them from one stretch of pages to
//! the next ("ORDER FORM" pages, then "STANDARD TERMS"), on a section's first page, and between
//! odd and even pages: [`sections`] says which pages share which.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::Rect;

use crate::extract::Picture;
use crate::lines::Line;
use crate::reflow::{PageLines, Pic};

/// Running text as one header or footer shows it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Running {
    /// The lines as one page shows them, top to bottom.
    pub lines: Vec<Line>,
    /// The page they come from.
    pub page: usize,
    /// Numbers in the lines that are fields: (line, char offset, char length, the page number
    /// (true) or the page count (false)).
    pub fields: Vec<(usize, usize, usize, bool)>,
    /// Pictures repeated with it (a letterhead, a band across the top).
    pub pictures: Vec<Pic>,
}

/// A document's headers (or footers): each look, and the one each page shows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Runs {
    pub looks: Vec<Running>,
    /// Per page, the look it shows; `None` for none.
    pub on: Vec<Option<usize>>,
    /// Per page, the page number its running text shows, where it shows one.
    pub numbers: Vec<Option<i64>>,
}

/// The header look and the footer look a page shows.
pub(crate) type Pair = (Option<usize>, Option<usize>);

/// Pages that share their headers and footers: one section's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stretch {
    /// Its first page.
    pub start: usize,
    /// The looks its pages show (its odd pages, where odd and even pages differ).
    pub default: Pair,
    /// Its even pages' looks, where odd and even pages differ.
    pub even: Option<Pair>,
    /// Its first page's own looks.
    pub first: Option<Pair>,
    /// The number its first page has, where the count starts again there.
    pub restart: Option<i64>,
}

/// What a number in running text does from page to page.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Series {
    Same,
    /// Counts with the pages (and may start again, at a new section).
    Counts,
    Neither,
}

fn series(at: &[(usize, i64)]) -> Series {
    let Some(&(_, v0)) = at.first() else { return Series::Same };
    if at.iter().all(|&(_, v)| v == v0) {
        return Series::Same;
    }
    let (mut counting, mut restarts) = (0usize, 0usize);
    for w in at.windows(2) {
        let (Some(&(pa, va)), Some(&(pb, vb))) = (w.first(), w.get(1)) else { continue };
        if vb - va == pb as i64 - pa as i64 {
            counting += 1;
        } else if vb < va {
            restarts += 1;
        } else {
            return Series::Neither;
        }
    }
    if counting > 0 && restarts <= counting { Series::Counts } else { Series::Neither }
}

/// Lines with one text (numbers aside) at about one place on the pages.
struct Group {
    /// (page, line) of each of its lines.
    members: Vec<(usize, usize)>,
    /// Its first line on each page it is on.
    firsts: Vec<(usize, usize)>,
    /// What each of its numbers does from page to page; `None` where they don't run with the
    /// pages ("Item 3" opening every page is body text).
    tokens: Option<Vec<Series>>,
    /// Lines at one place whose text changes from one stretch of pages to the next: each text
    /// is a look of its own.
    varies: bool,
}

impl Group {
    fn new(mut members: Vec<(usize, usize)>, pages: &[PageLines]) -> Group {
        members.sort_unstable();
        let mut firsts: Vec<(usize, usize)> = Vec::new();
        for &(pi, li) in &members {
            if firsts.last().map(|f| f.0) != Some(pi) {
                firsts.push((pi, li));
            }
        }
        let values: Vec<(usize, Vec<i64>)> = firsts
            .iter()
            .map(|&(pi, li)| {
                let text = pages.get(pi).and_then(|p| p.lines.get(li)).map(|l| l.text()).unwrap_or_default();
                (pi, digit_tokens(&text).iter().map(|(_, d)| d.parse::<i64>().unwrap_or(-1)).collect())
            })
            .collect();
        let count = values.first().map_or(0, |v| v.1.len());
        let tokens = values.iter().all(|v| v.1.len() == count).then(|| {
            (0..count).map(|k| series(&values.iter().map(|(pi, v)| (*pi, v.get(k).copied().unwrap_or(-1))).collect::<Vec<_>>())).collect::<Vec<_>>()
        });
        Group { members, firsts, tokens: tokens.filter(|t| !t.contains(&Series::Neither)), varies: false }
    }

    fn pages(&self) -> Vec<usize> {
        self.firsts.iter().map(|f| f.0).collect()
    }
}

/// A picture at one place on many pages.
struct PicGroup {
    rect: Rect,
    data: Arc<Vec<u8>>,
    ext: &'static str,
    pages: Vec<usize>,
    /// In the top half of the page: the header's.
    top: bool,
}

impl PicGroup {
    fn matches(&self, q: &Picture) -> bool {
        Arc::ptr_eq(&self.data, &q.data) && (self.rect.x0 - q.rect.x0).abs() < 1.0 && (self.rect.y0 - q.rect.y0).abs() < 1.0
    }
}

/// Find the running heads (`.0`) and feet (`.1`) and take them out of the pages.
pub(crate) fn running(pages: &mut [PageLines]) -> (Runs, Runs) {
    let n = pages.len();
    let none = Runs { looks: Vec::new(), on: vec![None; n], numbers: vec![None; n] };
    if n < 2 {
        return (none.clone(), none);
    }
    // On enough pages: half of them, or a few in a row (one section's).
    let need = (n / 2).max(2);
    let enough = |on: &[usize]| {
        on.len() >= need || (on.len() >= need.min(3) && on.last().zip(on.first()).is_some_and(|(l, f)| l.saturating_sub(*f) + 1 == on.len()))
    };
    let mut sizes: Vec<f32> = pages.iter().flat_map(|p| p.lines.iter().map(|l| l.size)).collect();
    sizes.sort_by(f32::total_cmp);
    let body = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0);
    let pics = picture_groups(pages, &enough);
    let header = zone(pages, true, &pics, &enough, body);
    let footer = zone(pages, false, &pics, &enough, body);
    (header, footer)
}

fn picture_groups(pages: &[PageLines], enough: &dyn Fn(&[usize]) -> bool) -> Vec<PicGroup> {
    let mut groups: Vec<PicGroup> = Vec::new();
    for (pi, p) in pages.iter().enumerate() {
        for pic in &p.pictures {
            match groups.iter_mut().find(|g| g.matches(pic)) {
                Some(g) if g.pages.last() != Some(&pi) => g.pages.push(pi),
                Some(_) => {}
                None => groups.push(PicGroup {
                    rect: pic.rect,
                    data: pic.data.clone(),
                    ext: pic.ext,
                    pages: vec![pi],
                    top: ((pic.rect.y0 + pic.rect.y1) / 2.0) < f64::from(p.h) / 2.0,
                }),
            }
        }
    }
    groups.retain(|g| enough(&g.pages));
    groups
}

/// Where a line sits across the page: its text may grow or shrink from page to page, but a
/// page number at the left and one at the right are two things.
fn side(l: &Line, w: f32) -> char {
    if l.x0 > 0.5 * w {
        'R'
    } else if l.x1 < 0.5 * w {
        'L'
    } else {
        'C'
    }
}

/// The running text at the top of the pages (`top`) or at their foot.
fn zone(pages: &mut [PageLines], top: bool, pics: &[PicGroup], enough: &dyn Fn(&[usize]) -> bool, body: f32) -> Runs {
    let n = pages.len();
    let in_zone = |l: &Line, h: f32| if top { l.base < 0.16 * h } else { l.base > 0.84 * h };
    // Lines by text (numbers aside) and side: (height bucket, page, line).
    let mut by_key: HashMap<String, Vec<(i32, usize, usize)>> = HashMap::new();
    for (pi, p) in pages.iter().enumerate() {
        for (li, l) in p.lines.iter().enumerate() {
            if !in_zone(l, p.h) || boxed(l, p) {
                continue;
            }
            let y = if top { l.base } else { p.h - l.base };
            let k = format!("{}{}", side(l, p.w), key(&l.text()));
            by_key.entry(k).or_default().push(((y / 4.0).round() as i32, pi, li));
        }
    }
    let mut keys: Vec<String> = by_key.keys().cloned().collect();
    keys.sort_unstable();
    // Groups: one text at about one height.
    let mut groups: Vec<Group> = Vec::new();
    for k in &keys {
        let Some(mut v) = by_key.remove(k) else { continue };
        v.sort_unstable();
        let mut cur: Vec<(usize, usize)> = Vec::new();
        let mut last: Option<i32> = None;
        for (b, pi, li) in v {
            if last.is_some_and(|lb| b - lb > 1) {
                groups.push(Group::new(std::mem::take(&mut cur), pages));
            }
            last = Some(b);
            cur.push((pi, li));
        }
        if !cur.is_empty() {
            groups.push(Group::new(cur, pages));
        }
    }
    // Text much bigger than the body's is a title that some pages open with ("Pilot Agreement"
    // on the first page of each part), unless nearly every page shows it.
    let big = |g: &Group| g.firsts.first().and_then(|&(pi, li)| pages.get(pi)?.lines.get(li)).is_some_and(|l| l.size > 1.5 * body);
    groups.retain(|g| g.tokens.is_some() && enough(&g.pages()) && (!big(g) || g.firsts.len() * 5 >= n * 4));

    // Running text stands clear of the body: a line set at the body's own spacing from the
    // text inside it is body text that happens to repeat.
    let all: HashSet<(usize, usize)> = groups.iter().flat_map(|g| g.members.iter().copied()).collect();
    groups.retain(|g| {
        let (mut touching, mut seen) = (0usize, 0usize);
        for &(pi, li) in &g.firsts {
            let Some(p) = pages.get(pi) else { continue };
            let Some(l) = p.lines.get(li) else { continue };
            seen += 1;
            if inward(p, l, top, |oi| all.contains(&(pi, oi))) < 1.6 {
                touching += 1;
            }
        }
        touching * 2 <= seen
    });
    groups.extend(changing(pages, top, &groups, enough, body));
    let zpics: Vec<&PicGroup> = pics.iter().filter(|g| g.top == top).collect();
    let mut runs = Runs { looks: Vec::new(), on: vec![None; n], numbers: vec![None; n] };
    if groups.is_empty() && zpics.is_empty() {
        return runs;
    }
    let gid: HashMap<(usize, usize), usize> = groups.iter().enumerate().flat_map(|(i, g)| g.members.iter().map(move |m| (*m, i))).collect();
    // Each page's look: the groups (with their text, for one that changes) and pictures it shows.
    let mut sigs: Vec<(Vec<(usize, String)>, Vec<usize>)> = Vec::new();
    for pi in 0..n {
        let Some(p) = pages.get(pi) else { continue };
        let mut shows: Vec<(usize, String)> = (0..p.lines.len())
            .filter_map(|li| {
                let g = *gid.get(&(pi, li))?;
                let text =
                    if groups.get(g).is_some_and(|g| g.varies) { p.lines.get(li).map(|l| key(&l.text())).unwrap_or_default() } else { String::new() };
                Some((g, text))
            })
            .collect();
        shows.sort_unstable();
        shows.dedup();
        let texts: Vec<usize> = shows.iter().map(|s| s.0).collect();
        let shown: Vec<usize> = zpics.iter().enumerate().filter(|(_, g)| g.pages.contains(&pi)).map(|(i, _)| i).collect();
        if texts.is_empty() && shown.is_empty() {
            continue;
        }
        // The page number: the first number on it that counts with the pages.
        let number = texts.iter().filter_map(|g| groups.get(*g)).find_map(|g| {
            let li = g.firsts.iter().find(|f| f.0 == pi)?.1;
            let text = p.lines.get(li)?.text();
            let tokens = g.tokens.as_ref()?;
            digit_tokens(&text).iter().zip(tokens).find(|(_, s)| **s == Series::Counts).and_then(|((_, d), _)| d.parse::<i64>().ok())
        });
        if let Some(slot) = runs.numbers.get_mut(pi) {
            *slot = number;
        }
        let sig = (shows, shown);
        let look = match sigs.iter().position(|s| *s == sig) {
            Some(i) => i,
            None => {
                runs.looks.push(look(p, pi, &sig, &groups, &gid, &zpics, n));
                sigs.push(sig);
                runs.looks.len() - 1
            }
        };
        if let Some(slot) = runs.on.get_mut(pi) {
            *slot = Some(look);
        }
    }
    for (pi, p) in pages.iter_mut().enumerate() {
        let mut li = 0usize;
        p.lines.retain(|_| {
            let keep = !gid.contains_key(&(pi, li));
            li += 1;
            keep
        });
        p.pictures.retain(|q| !zpics.iter().any(|g| g.matches(q)));
    }
    runs
}

/// The look page `pi` shows: its lines of the groups in `sig.0`, with their number fields, and
/// the pictures in `sig.1`.
fn look(
    p: &PageLines,
    pi: usize,
    sig: &(Vec<(usize, String)>, Vec<usize>),
    groups: &[Group],
    gid: &HashMap<(usize, usize), usize>,
    pics: &[&PicGroup],
    n: usize,
) -> Running {
    let mut lines: Vec<(usize, &Line)> = p.lines.iter().enumerate().filter_map(|(li, l)| gid.get(&(pi, li)).map(|g| (*g, l))).collect();
    lines.sort_by(|a, b| a.1.base.total_cmp(&b.1.base).then(a.1.x0.total_cmp(&b.1.x0)));
    let tokens_of = |g: usize| groups.get(g).and_then(|g| g.tokens.as_ref());
    // "Page 3 of 5": the count is a field only beside a page number.
    let counts = lines.iter().any(|(g, _)| tokens_of(*g).is_some_and(|t| t.contains(&Series::Counts)));
    let mut run = Running { page: pi, ..Default::default() };
    for (li, (g, l)) in lines.iter().enumerate() {
        let text = l.text();
        for (k, (pos, tok)) in digit_tokens(&text).into_iter().enumerate() {
            let len = tok.chars().count();
            match tokens_of(*g).and_then(|t| t.get(k)) {
                Some(Series::Counts) => run.fields.push((li, pos, len, true)),
                Some(Series::Same) if counts && n > 1 && tok.parse::<usize>().ok() == Some(n) => run.fields.push((li, pos, len, false)),
                _ => {}
            }
        }
        run.lines.push((*l).clone());
    }
    run.pictures = sig.1.iter().filter_map(|i| pics.get(*i)).map(|g| Pic { rect: g.rect, data: g.data.clone(), ext: g.ext, behind: true }).collect();
    run
}

/// How far `l` stands from the nearest line on its page further into the body (but for those
/// `skip` says to leave out), in the smaller of the two text sizes: about 1.2 for lines of one
/// paragraph. Text beside it on its baseline (a page number between two labels) is not further
/// in.
fn inward(p: &PageLines, l: &Line, top: bool, skip: impl Fn(usize) -> bool) -> f32 {
    p.lines
        .iter()
        .enumerate()
        .filter(|(oi, _)| !skip(*oi))
        .map(|(_, o)| (if top { o.base - l.base } else { l.base - o.base }, o.size))
        .filter(|(d, _)| *d > 0.3 * l.size)
        .map(|(d, size)| d / size.min(l.size).max(1.0))
        .fold(f32::MAX, f32::min)
}

/// Header (or footer) lines whose text changes from one stretch of pages to the next, at one
/// place, in one font and size, on enough pages, each well clear of the body: "COVER PAGE" on
/// the first page and "STANDARD TERMS" on the rest. Not a page's own title (bigger than the
/// text), nor a running head that changes on every page (a dictionary's).
fn changing(pages: &[PageLines], top: bool, taken: &[Group], enough: &dyn Fn(&[usize]) -> bool, body: f32) -> Vec<Group> {
    let taken: HashSet<(usize, usize)> = taken.iter().flat_map(|g| g.members.iter().copied()).collect();
    // Candidates by edge (left, right or centre, as it may be aligned), height, size and font.
    let mut cands: HashMap<(u8, i32, i32, i32, usize), Vec<(usize, usize)>> = HashMap::new();
    for (pi, p) in pages.iter().enumerate() {
        for (li, l) in p.lines.iter().enumerate() {
            let zone = if top { l.base < 0.16 * p.h } else { l.base > 0.84 * p.h };
            if !zone || taken.contains(&(pi, li)) || l.size > 1.25 * body || boxed(l, p) || l.text().trim().is_empty() {
                continue;
            }
            if inward(p, l, top, |oi| oi == li || taken.contains(&(pi, oi))) < 2.0 {
                continue;
            }
            let y = if top { l.base } else { p.h - l.base };
            let (yb, sb) = ((y / 4.0).round() as i32, (l.size * 2.0).round() as i32);
            for (edge, x) in [(0u8, l.x0), (1, l.x1), (2, (l.x0 + l.x1) / 2.0)] {
                cands.entry((edge, (x / 3.0).round() as i32, yb, sb, l.font)).or_default().push((pi, li));
            }
        }
    }
    // The candidates on the most pages first; a line goes to one.
    let mut order: Vec<((u8, i32, i32, i32, usize), Vec<(usize, usize)>)> = cands.into_iter().collect();
    order.sort_by(|a, b| {
        let pages_of = |v: &[(usize, usize)]| v.iter().map(|m| m.0).collect::<HashSet<_>>().len();
        pages_of(&b.1).cmp(&pages_of(&a.1)).then(a.0.cmp(&b.0))
    });
    let mut used: HashSet<(usize, usize)> = HashSet::new();
    let mut out = Vec::new();
    for (_, members) in order {
        let members: Vec<(usize, usize)> = members.into_iter().filter(|m| !used.contains(m)).collect();
        let mut on: Vec<usize> = members.iter().map(|m| m.0).collect();
        on.dedup();
        if !enough(&on) {
            continue;
        }
        let texts: HashSet<String> = members.iter().filter_map(|&(pi, li)| pages.get(pi)?.lines.get(li)).map(|l| key(&l.text())).collect();
        // Text that stays the same is the text groups' business; one that changes on most
        // pages is no section's.
        if texts.len() < 2 || texts.len() > (on.len() / 3).max(2) {
            continue;
        }
        used.extend(members.iter().copied());
        let mut g = Group::new(members, pages);
        g.tokens = Some(Vec::new());
        g.varies = true;
        out.push(g);
    }
    out
}

/// Split the pages into stretches that share their headers and footers, and say whether odd
/// and even pages have their own.
pub(crate) fn sections(header: &Runs, footer: &Runs, n: usize) -> (Vec<Stretch>, bool) {
    let pair = |p: usize| -> Pair { (header.on.get(p).copied().flatten(), footer.on.get(p).copied().flatten()) };
    let number = |p: usize| header.numbers.get(p).copied().flatten().or(footer.numbers.get(p).copied().flatten());
    if n == 0 {
        return (Vec::new(), false);
    }
    // Odd and even pages with their own: the looks change every page and come back the next.
    let alternating = (2..n).filter(|&p| pair(p) == pair(p - 2) && pair(p) != pair(p - 1)).count();
    let even_odd = n >= 4 && alternating * 2 >= n - 2;
    // How far a stretch whose pages from `from` on show the same looks reaches: (end, odd
    // pages' looks, even pages' looks).
    let reach = |from: usize| -> (usize, Option<Pair>, Option<Pair>) {
        let (mut odd, mut even) = (None, None);
        let mut e = from;
        while e < n {
            let slot = if even_odd && (e + 1).is_multiple_of(2) { &mut even } else { &mut odd };
            match slot {
                None => *slot = Some(pair(e)),
                Some(x) if *x == pair(e) => {}
                Some(_) => break,
            }
            e += 1;
        }
        (e, odd, even)
    };
    let mut out: Vec<Stretch> = Vec::new();
    let mut s = 0;
    while s < n {
        let plain = reach(s);
        // Its first page its own (a cover with nothing at the head, say)?
        let titled = reach(s + 1);
        let (end, odd, even, first) =
            if s + 1 < n && titled.0 > plain.0 { (titled.0, titled.1, titled.2, Some(pair(s))) } else { (plain.0, plain.1, plain.2, None) };
        let default = odd.or(even).unwrap_or((None, None));
        let st = Stretch { start: s, default, even: even_odd.then(|| even.unwrap_or(default)), first, restart: None };
        // A count that starts again inside it starts a section there too.
        let mut cur = st;
        for p in s + 1..end {
            if let (Some(a), Some(b)) = (number(p - 1), number(p))
                && b != a + 1
            {
                let next = Stretch { start: p, first: None, ..cur.clone() };
                out.push(std::mem::replace(&mut cur, next));
            }
        }
        out.push(cur);
        s = end.max(s + 1);
    }
    // Page numbers: where a stretch's first page doesn't follow on from the page before.
    let ends: Vec<usize> = out.iter().skip(1).map(|s| s.start).chain(std::iter::once(n)).collect();
    let mut next = 1i64;
    for (st, &end) in out.iter_mut().zip(&ends) {
        let shown = (st.start..end).find_map(|p| number(p).map(|v| v - (p - st.start) as i64));
        if let Some(first) = shown
            && first != next
            && first >= 0
        {
            st.restart = Some(first);
            next = first;
        }
        next += end.saturating_sub(st.start) as i64;
    }
    (out, even_odd)
}

/// Text with its numbers masked and its spacing and case evened out: what repeats.
fn key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
            last_space = false;
        } else if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.extend(c.to_lowercase());
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// A line between two rules that run past both its ends: a table cell's text (a table's
/// heading row repeated on each page is not a running head).
fn boxed(l: &Line, p: &PageLines) -> bool {
    let reach = 1.6 * l.size;
    let spans = |r: &&crate::extract::Rule| r.horizontal && r.from <= l.x0 + 1.0 && r.to >= l.x1 - 1.0;
    let above = p.rules.iter().filter(spans).any(|r| r.a < l.top() + 0.5 && r.a > l.base - reach);
    let below = p.rules.iter().filter(spans).any(|r| r.a > l.base - 0.5 && r.a < l.base + reach);
    above && below
}

/// Runs of ASCII digits in `t`: (char offset, text).
fn digit_tokens(t: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start: Option<(usize, usize)> = None;
    for (ci, (bi, c)) in t.char_indices().enumerate() {
        if c.is_ascii_digit() {
            if start.is_none() {
                start = Some((ci, bi));
            }
        } else if let Some((sc, sb)) = start.take() {
            out.push((sc, t.get(sb..bi).unwrap_or("")));
        }
    }
    if let Some((sc, sb)) = start {
        out.push((sc, t.get(sb..).unwrap_or("")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(on: &[Option<usize>]) -> Runs {
        Runs { looks: Vec::new(), on: on.to_vec(), numbers: vec![None; on.len()] }
    }

    #[test]
    fn numbers_count_or_stay() {
        assert_eq!(series(&[(0, 1), (1, 1), (2, 1)]), Series::Same);
        assert_eq!(series(&[(0, 1), (1, 2), (2, 3)]), Series::Counts);
        assert_eq!(series(&[(0, 1), (1, 2), (2, 1), (3, 2)]), Series::Counts);
        assert_eq!(series(&[(0, 3), (1, 7), (2, 12)]), Series::Neither);
    }

    #[test]
    fn a_new_header_starts_a_section() {
        let none = runs(&[None; 5]);
        let (st, eo) = sections(&runs(&[Some(0), Some(0), Some(1), Some(1), Some(1)]), &none, 5);
        assert!(!eo);
        assert_eq!(st.iter().map(|s| (s.start, s.default.0, s.first)).collect::<Vec<_>>(), vec![(0, Some(0), None), (2, Some(1), None)]);
    }

    #[test]
    fn a_bare_first_page_is_a_title_page() {
        let (st, _) = sections(&runs(&[None, Some(0), Some(0), Some(0)]), &runs(&[None; 4]), 4);
        assert_eq!(st.len(), 1);
        assert_eq!(st.first().map(|s| (s.default, s.first)), Some(((Some(0), None), Some((None, None)))));
    }

    #[test]
    fn odd_and_even_pages_alternate() {
        let (st, eo) = sections(&runs(&[Some(0), Some(1), Some(0), Some(1), Some(0), Some(1)]), &runs(&[None; 6]), 6);
        assert!(eo);
        assert_eq!(st.len(), 1);
        assert_eq!(st.first().map(|s| (s.default.0, s.even.map(|e| e.0))), Some((Some(0), Some(Some(1)))));
    }

    #[test]
    fn page_numbers_that_start_again_start_a_section() {
        let mut f = runs(&[Some(0); 5]);
        f.numbers = vec![Some(1), Some(2), Some(1), Some(2), Some(3)];
        let (st, _) = sections(&runs(&[None; 5]), &f, 5);
        assert_eq!(st.iter().map(|s| (s.start, s.restart)).collect::<Vec<_>>(), vec![(0, None), (2, Some(1))]);
    }
}
