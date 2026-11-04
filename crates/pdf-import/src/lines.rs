//! Glyphs into lines: glyphs on one baseline make a band; a band splits into segments where a
//! gap is too wide to be a space (table columns, tabbed text, a column gutter); a segment is a
//! line of spans, runs of one font, size and colour, with spaces put where the gaps are.
//! Smaller glyphs raised above or dropped below a line join it as superscript or subscript.
//! Underlines and strikethroughs are rules drawn under or through glyphs.

use wordcraft_doc::Rgb;

use crate::extract::{Glyph, PageContent};
use crate::fontinfo::FontTable;

/// Superscript and subscript text is drawn this much smaller than its size (WordCraft's and
/// Word's scale), so a raised 6.5 pt glyph is 10 pt text.
pub(crate) const SUP_SCALE: f32 = 0.65;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Vert {
    #[default]
    Base,
    Sup,
    Sub,
}

/// A run of text in one font, size, colour and decoration.
#[derive(Clone, Debug)]
pub(crate) struct Span {
    pub text: String,
    pub font: usize,
    /// Size as the document gives it (a superscript's base size), points.
    pub size: f32,
    pub color: Rgb,
    pub x0: f32,
    pub x1: f32,
    pub vert: Vert,
    pub underline: bool,
    pub strike: bool,
    pub bold: bool,
    pub link: Option<String>,
    /// A colour filled behind the text (a highlighter, or character shading).
    pub back: Option<Rgb>,
    /// The reference mark of this footnote (an index into the notes `notes::extract` found).
    pub note: Option<usize>,
}

/// A line: one segment of a band.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub x0: f32,
    pub x1: f32,
    pub base: f32,
    /// The size of most of its text.
    pub size: f32,
    /// The font of most of its text.
    pub font: usize,
    pub spans: Vec<Span>,
    /// The band it came from; lines of one band share a baseline.
    pub band: usize,
    /// Gaps between words, ems.
    pub word_gaps: Vec<f32>,
    /// Pieces joined into this line by tabs: their left and right x (one piece = no tabs).
    pub pieces: Vec<(f32, f32)>,
    /// Where the first word ends (to tell whether it would have fitted on the line above).
    pub first_word_end: f32,
    /// Where the second word starts (the text after a list label).
    pub second_word_start: f32,
}

impl Line {
    pub(crate) fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The line's height above the baseline, roughly (for finding what is in a box).
    pub(crate) fn top(&self) -> f32 {
        self.base - 0.8 * self.size
    }

    pub(crate) fn bottom(&self) -> f32 {
        self.base + 0.25 * self.size
    }

    pub(crate) fn chars(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }

    /// Every character is bold.
    pub(crate) fn all_bold(&self, fonts: &FontTable) -> bool {
        self.spans.iter().filter(|s| !s.text.trim().is_empty()).all(|s| s.bold || fonts.get(s.font).is_some_and(|f| f.bold))
    }
}

/// Characters a PDF draws that a document keeps as something else.
fn normalize(text: &str, symbol: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            // Symbol and Wingdings bullets in the private use area (Word's own bullets).
            '\u{F0B7}' | '\u{F06C}' | '\u{F09F}' => out.push('•'),
            '\u{F0A7}' | '\u{F06E}' => out.push('▪'),
            '\u{F0D8}' => out.push('➢'),
            '\u{F0FC}' => out.push('✓'),
            '\u{F076}' => out.push('❖'),
            '\u{F0A8}' => out.push('◦'),
            '\u{F02D}' => out.push('–'),
            '\t' | '\r' | '\n' => out.push(' '),
            c if c.is_control() => {}
            // Symbol font letters stand for Greek and marks the font draws in their place.
            c if symbol && ('\u{F020}'..='\u{F0FF}').contains(&c) => {
                if let Some(m) = char::from_u32(c as u32 - 0xF000) {
                    out.push(m);
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// How a glyph is decorated: the rules underlining and striking it, the link it is in.
#[derive(Clone, Copy, Default)]
struct Mark {
    under: Option<usize>,
    strike: Option<usize>,
    link: Option<usize>,
}

/// Mark glyphs that a thin rule underlines or strikes through, and those inside link areas.
/// The rules used that way leave the page's rules (what is left are borders and separators).
fn decorations(page: &mut PageContent) -> Vec<Mark> {
    let mut marks = vec![Mark::default(); page.glyphs.len()];
    // Thin horizontal rules by y, for a binary search per glyph.
    let mut rules: Vec<(f32, f32, f32, usize)> =
        page.rules.iter().enumerate().filter(|(_, r)| r.horizontal && r.width <= 1.8).map(|(i, r)| (r.a, r.from, r.to, i)).collect();
    rules.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (g, mark) in page.glyphs.iter().zip(marks.iter_mut()) {
        let cx = (g.x0 + g.x1) / 2.0;
        if !g.text.trim().is_empty() {
            let lo = rules.partition_point(|r| r.0 < g.base - 0.45 * g.size);
            for r in rules.iter().skip(lo) {
                if r.0 > g.base + 0.3 * g.size {
                    break;
                }
                if cx < r.1 || cx > r.2 {
                    continue;
                }
                let dy = r.0 - g.base;
                if (-0.02 * g.size..=0.3 * g.size).contains(&dy) {
                    mark.under = Some(r.3);
                } else if dy < -0.15 * g.size && dy > -0.45 * g.size {
                    mark.strike = Some(r.3);
                }
            }
        }
        let probe = kurbo::Point::new(f64::from(cx), f64::from(g.base - 0.3 * g.size));
        mark.link = page.links.iter().position(|l| l.rect.contains(probe));
    }
    // Each used rule's extent over the glyphs it marks. A rule running more than 3 em past
    // them on either side is a table or cell border passing under the text, not a decoration.
    let mut extent = vec![(f32::MAX, f32::MIN, 0.0f32); page.rules.len()];
    for (g, m) in page.glyphs.iter().zip(marks.iter()) {
        for r in [m.under, m.strike].into_iter().flatten() {
            if let Some(e) = extent.get_mut(r) {
                *e = (e.0.min(g.x0), e.1.max(g.x1), e.2.max(g.size));
            }
        }
    }
    let decoration: Vec<bool> =
        page.rules.iter().zip(extent.iter()).map(|(r, e)| e.1 > e.0 && e.0 - r.from <= 3.0 * e.2 && r.to - e.1 <= 3.0 * e.2).collect();
    for m in &mut marks {
        m.under = m.under.filter(|r| decoration.get(*r).copied().unwrap_or(false));
        m.strike = m.strike.filter(|r| decoration.get(*r).copied().unwrap_or(false));
    }
    let mut i = 0;
    page.rules.retain(|_| {
        let keep = !decoration.get(i).copied().unwrap_or(false);
        i += 1;
        keep
    });
    marks
}

struct G<'a> {
    g: &'a Glyph,
    text: String,
    underline: bool,
    strike: bool,
    link: Option<usize>,
    vert: Vert,
    back: Option<Rgb>,
}

/// The colour drawn right behind each glyph, line high (a highlighter: often one box per
/// glyph, the narrow ones read as rules). The boxes used go from the page's fills and rules.
fn backgrounds(page: &mut PageContent) -> Vec<Option<Rgb>> {
    // Candidate boxes: (y0, y1, x0, x1, colour, from a rule?, index).
    let max_size = page.glyphs.iter().map(|g| g.size).fold(0.0f32, f32::max);
    let mut boxes: Vec<(f32, f32, f32, f32, Rgb, bool, usize)> = Vec::new();
    for (i, f) in page.fills.iter().enumerate() {
        let (y0, y1) = (f.rect.y0 as f32, f.rect.y1 as f32);
        if y1 - y0 <= 3.0 * max_size && f.color != Rgb(255, 255, 255) {
            boxes.push((y0, y1, f.rect.x0 as f32, f.rect.x1 as f32, f.color, false, i));
        }
    }
    for (i, r) in page.rules.iter().enumerate() {
        if !r.horizontal && r.width >= 1.0 && r.to - r.from <= 3.0 * max_size {
            boxes.push((r.from, r.to, r.a - r.width / 2.0, r.a + r.width / 2.0, r.color, true, i));
        }
    }
    if boxes.is_empty() {
        return vec![None; page.glyphs.len()];
    }
    boxes.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut used_fills = vec![false; page.fills.len()];
    let mut used_rules = vec![false; page.rules.len()];
    let out = page
        .glyphs
        .iter()
        .map(|g| {
            let cx = (g.x0 + g.x1) / 2.0;
            let from = boxes.partition_point(|b| b.1 < g.base - 0.05 * g.size);
            let found = boxes
                .get(from..)
                .unwrap_or(&[])
                .iter()
                .take_while(|b| b.1 <= g.base + 1.2 * g.size)
                .find(|b| b.0 <= g.base - 0.45 * g.size && b.1 - b.0 <= 2.6 * g.size && cx >= b.2 - 0.6 && cx <= b.3 + 0.6);
            found.map(|b| {
                let slot = if b.5 { used_rules.get_mut(b.6) } else { used_fills.get_mut(b.6) };
                if let Some(u) = slot {
                    *u = true;
                }
                b.4
            })
        })
        .collect();
    // Boxes in the same band and colour as used ones (behind word spaces) drop too.
    let band = |c: Rgb, y0: f64, y1: f64| (c.0, c.1, c.2, (y0 * 4.0).round() as i64, (y1 * 4.0).round() as i64);
    let used_bands: std::collections::HashSet<_> =
        page.fills.iter().zip(&used_fills).filter(|(_, u)| **u).map(|(f, _)| band(f.color, f.rect.y0, f.rect.y1)).collect();
    let mut i = 0;
    page.fills.retain(|f| {
        let keep = !used_fills.get(i).copied().unwrap_or(false) && !used_bands.contains(&band(f.color, f.rect.y0, f.rect.y1));
        i += 1;
        keep
    });
    let mut i = 0;
    page.rules.retain(|_| {
        let keep = !used_rules.get(i).copied().unwrap_or(false);
        i += 1;
        keep
    });
    out
}

/// The page's lines, top to bottom, left to right.
pub(crate) fn lines(page: &mut PageContent, fonts: &FontTable) -> Vec<Line> {
    let marks = decorations(page);
    let backs = backgrounds(page);
    let mut gs: Vec<G<'_>> = Vec::with_capacity(page.glyphs.len());
    let mut last: Option<(f32, f32, String)> = None;
    for ((g, m), back) in page.glyphs.iter().zip(marks.iter()).zip(backs.iter()) {
        let symbol = fonts.get(g.font).is_some_and(|f| f.symbol);
        let mut text = normalize(&g.text, symbol);
        if text.is_empty() {
            // A glyph with no text: in a symbol font it is a bullet; otherwise nothing to keep.
            if symbol {
                text.push('•');
            } else {
                continue;
            }
        }
        // The same glyph drawn again slightly offset (poor man's bold): keep one.
        if let Some((x, b, t)) = &last
            && *t == text
            && (x - g.x0).abs() < 0.08 * g.size
            && (b - g.base).abs() < 0.08 * g.size
            && !text.trim().is_empty()
        {
            if let Some(prev) = gs.last_mut() {
                prev.underline |= m.under.is_some();
            }
            continue;
        }
        last = Some((g.x0, g.base, text.clone()));
        gs.push(G { g, text, underline: m.under.is_some(), strike: m.strike.is_some(), link: m.link, vert: Vert::Base, back: *back });
    }
    // Bands: glyphs by baseline.
    let mut order: Vec<usize> = (0..gs.len()).collect();
    order.sort_by(|a, b| {
        let (ga, gb) = (gs.get(*a).map(|x| x.g), gs.get(*b).map(|x| x.g));
        match (ga, gb) {
            (Some(ga), Some(gb)) => ga.base.total_cmp(&gb.base).then(ga.x0.total_cmp(&gb.x0)),
            _ => std::cmp::Ordering::Equal,
        }
    });
    let mut bands: Vec<Vec<usize>> = Vec::new();
    let mut band_base = f32::MIN;
    let mut band_size = 0.0f32;
    for i in order {
        let Some(x) = gs.get(i) else { continue };
        let tol = 0.2 * band_size.min(x.g.size).max(1.0);
        if bands.is_empty() || x.g.base - band_base > tol {
            bands.push(vec![i]);
            band_base = x.g.base;
            band_size = x.g.size;
        } else if let Some(b) = bands.last_mut() {
            b.push(i);
            band_size = band_size.max(x.g.size);
        }
    }
    // Superscripts and subscripts: a band of small glyphs just above or below a band of larger
    // ones, within its horizontal extent, joins it.
    let info: Vec<(f32, f32, f32, f32)> = bands
        .iter()
        .map(|b| {
            let mut sizes: Vec<f32> = b.iter().filter_map(|i| gs.get(*i)).map(|x| x.g.size).collect();
            sizes.sort_by(f32::total_cmp);
            let size = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0);
            let base = b.iter().filter_map(|i| gs.get(*i)).map(|x| x.g.base).sum::<f32>() / b.len().max(1) as f32;
            let x0 = b.iter().filter_map(|i| gs.get(*i)).map(|x| x.g.x0).fold(f32::MAX, f32::min);
            let x1 = b.iter().filter_map(|i| gs.get(*i)).map(|x| x.g.x1).fold(f32::MIN, f32::max);
            (base, size, x0, x1)
        })
        .collect();
    let mut target: Vec<Option<(usize, Vert)>> = vec![None; bands.len()];
    for (i, (base, size, x0, x1)) in info.iter().enumerate() {
        let near = |j: usize, vert: Vert| -> bool {
            let Some((b2, s2, a0, a1)) = info.get(j) else { return false };
            if *s2 < size * 1.15 || *x0 > a1 + 1.5 * s2 || *x1 < a0 - 1.5 * s2 {
                return false;
            }
            let d = b2 - base;
            match vert {
                Vert::Sup => d > 0.12 * s2 && d < 0.65 * s2,
                Vert::Sub => -d > 0.08 * s2 && -d < 0.5 * s2,
                Vert::Base => false,
            }
        };
        // The neighbours on either side (bands are in baseline order).
        if i + 1 < bands.len() && near(i + 1, Vert::Sup) {
            target[i] = Some((i + 1, Vert::Sup));
        } else if i > 0 && near(i - 1, Vert::Sub) {
            target[i] = Some((i - 1, Vert::Sub));
        }
    }
    let mut merged: Vec<Vec<usize>> = vec![Vec::new(); bands.len()];
    for (i, b) in bands.iter().enumerate() {
        match target.get(i).copied().flatten() {
            Some((t, vert)) if target.get(t).copied().flatten().is_none() => {
                for gi in b {
                    if let Some(x) = gs.get_mut(*gi) {
                        x.vert = vert;
                    }
                }
                if let Some(m) = merged.get_mut(t) {
                    m.extend_from_slice(b);
                }
            }
            _ => {
                if let Some(m) = merged.get_mut(i) {
                    m.extend_from_slice(b);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (band_no, mut band) in merged.into_iter().enumerate() {
        if band.is_empty() {
            continue;
        }
        band.sort_by(|a, b| match (gs.get(*a), gs.get(*b)) {
            (Some(x), Some(y)) => x.g.x0.total_cmp(&y.g.x0),
            _ => std::cmp::Ordering::Equal,
        });
        let base_glyphs: Vec<&G<'_>> = band.iter().filter_map(|i| gs.get(*i)).filter(|x| x.vert == Vert::Base).collect();
        let mut sizes: Vec<f32> = base_glyphs.iter().map(|x| x.g.size).collect();
        sizes.sort_by(f32::total_cmp);
        let size = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0).max(1.0);
        // Word gaps in the band, to tell a column gap from a wide justified space.
        let mut gaps = Vec::new();
        let mut prev_end: Option<f32> = None;
        for x in band.iter().filter_map(|i| gs.get(*i)) {
            if x.text.trim().is_empty() {
                continue;
            }
            if let Some(e) = prev_end {
                let gap = x.g.x0 - e;
                if gap > 0.12 * size {
                    gaps.push(gap);
                }
            }
            prev_end = Some(prev_end.map_or(x.g.x1, |e| e.max(x.g.x1)));
        }
        gaps.sort_by(f32::total_cmp);
        let median_gap = gaps.get(gaps.len() / 2).copied().unwrap_or(0.3 * size);
        let split = |gap: f32| gap > 1.6 * size || (gap > 0.8 * size && gap > 3.0 * median_gap);
        // Segments.
        let mut seg: Vec<&G<'_>> = Vec::new();
        let mut prev_end: Option<f32> = None;
        let mut segs: Vec<Vec<&G<'_>>> = Vec::new();
        for x in band.iter().filter_map(|i| gs.get(*i)) {
            let blank = x.text.trim().is_empty();
            if let Some(e) = prev_end
                && !blank
                && split(x.g.x0 - e)
                && !seg.is_empty()
            {
                segs.push(std::mem::take(&mut seg));
            }
            if blank && seg.is_empty() {
                continue;
            }
            seg.push(x);
            if !blank {
                prev_end = Some(prev_end.map_or(x.g.x1, |e: f32| e.max(x.g.x1)));
            }
        }
        if !seg.is_empty() {
            segs.push(seg);
        }
        for seg in segs {
            if let Some(line) = make_line(&seg, band_no, page, fonts) {
                out.push(line);
            }
        }
    }
    out
}

fn make_line(seg: &[&G<'_>], band: usize, page: &PageContent, fonts: &FontTable) -> Option<Line> {
    let mut spans: Vec<Span> = Vec::new();
    let mut word_gaps = Vec::new();
    let mut prev: Option<&G<'_>> = None;
    let mut pending_space = false;
    let mut counts: Vec<(usize, f32, usize)> = Vec::new();
    let mut first_word_end: Option<f32> = None;
    let mut second_word_start: Option<f32> = None;
    for x in seg {
        let g = x.g;
        if x.text.trim().is_empty() {
            pending_space = true;
            continue;
        }
        let nominal = if x.vert == Vert::Base { g.size } else { g.size / SUP_SCALE };
        let size = (nominal * 2.0).round() / 2.0;
        let font_space = fonts.get(g.font).map_or(0.25, |f| f.space);
        let mut space = pending_space;
        if let Some(p) = prev {
            let gap = g.x0 - p.g.x1;
            let em = p.g.size.max(g.size);
            // A gap wider than a good part of a space is one; kerning is far less.
            if gap > (0.4 * font_space * em).max(0.09 * em) {
                space = true;
            }
            if space {
                word_gaps.push(gap.max(0.0) / em);
            }
        } else {
            space = false;
        }
        pending_space = false;
        if space && first_word_end.is_none() {
            first_word_end = prev.map(|p| p.g.x1);
            second_word_start = Some(g.x0);
        }
        let bold = g.stroked;
        let link = x.link.and_then(|i| page.links.get(i)).map(|l| l.uri.clone());
        let same = spans.last().is_some_and(|s| {
            s.font == g.font
                && (s.size - size).abs() < 0.26
                && s.color == g.color
                && s.vert == x.vert
                && s.underline == x.underline
                && s.strike == x.strike
                && s.bold == bold
                && s.link == link
                && s.back == x.back
        });
        if space && let Some(s) = spans.last_mut() {
            // The space takes the formatting before it, unless the decoration stops there.
            s.text.push(' ');
        }
        if same {
            if let Some(s) = spans.last_mut() {
                s.text.push_str(&x.text);
                s.x1 = s.x1.max(g.x1);
            }
        } else {
            spans.push(Span {
                text: x.text.clone(),
                font: g.font,
                size,
                color: g.color,
                x0: g.x0,
                x1: g.x1,
                vert: x.vert,
                underline: x.underline,
                strike: x.strike,
                bold,
                link,
                back: x.back,
                note: None,
            });
        }
        if x.vert == Vert::Base {
            match counts.iter_mut().find(|c| c.0 == g.font && (c.1 - g.size).abs() < 0.26) {
                Some(c) => c.2 += x.text.chars().count(),
                None => counts.push((g.font, g.size, x.text.chars().count())),
            }
        }
        prev = Some(x);
    }
    // An underline that runs under the space between two underlined words is one underline;
    // a trailing space is not underlined.
    if let Some(s) = spans.last_mut() {
        let t = s.text.trim_end().to_string();
        s.text = t;
    }
    spans.retain(|s| !s.text.is_empty());
    let first = spans.first()?;
    let x0 = first.x0;
    let x1 = spans.iter().map(|s| s.x1).fold(f32::MIN, f32::max);
    let (font, size) = counts.iter().max_by_key(|c| c.2).map(|c| (c.0, c.1)).unwrap_or((first.font, first.size));
    let base_glyphs: Vec<f32> = seg.iter().filter(|x| x.vert == Vert::Base && !x.text.trim().is_empty()).map(|x| x.g.base).collect();
    let base = if base_glyphs.is_empty() {
        seg.iter().map(|x| x.g.base).sum::<f32>() / seg.len().max(1) as f32
    } else {
        base_glyphs.iter().sum::<f32>() / base_glyphs.len() as f32
    };
    let first_word_end = first_word_end.unwrap_or(x1);
    let second_word_start = second_word_start.unwrap_or(x1);
    Some(Line {
        x0,
        x1,
        base,
        size: (size * 2.0).round() / 2.0,
        font,
        spans,
        band,
        word_gaps,
        pieces: vec![(x0, x1)],
        first_word_end,
        second_word_start,
    })
}
