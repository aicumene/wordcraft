//! The analysed pages as a [`Document`]: a Normal style and headings taken from the text, the
//! paragraphs' spacing, indents and alignment, real bulleted and numbered lists where the
//! numbers run in sequence, tables, pictures, sections for page sizes and columns, and running
//! headers and footers with page number fields.

use std::collections::HashMap;
use std::sync::Arc;

use wordcraft_doc::numbering::{AbstractNum, Level, Num};
use wordcraft_doc::para::{Anchor, Float, InlineObject, NoteKind, OBJ, Wrap};
use wordcraft_doc::props::{
    Border, BorderStyle, Borders, CellProps, HeightRule, Highlight, LineSpacing, NumRef, RowProps, TabAlign, TabLeader, TabStop, Underline, VAlign,
    VMerge, VertAlign,
};
use wordcraft_doc::section::{Columns, HeaderSet, NumFormat, SectionStart};
use wordcraft_doc::styles::{Style, StyleKind};
use wordcraft_doc::{Align, Block, Blocks, CharProps, Document, ParaProps, Paragraph, PartKind, Rgb, Run, SectionProps, TextColor, para_block};

use crate::fontinfo::FontTable;
use crate::lines::{Line, Span, Vert};
use crate::reflow::{Edge, Item, Margins, Para, Pic, Region, Stats, Tab, label, nice};
use crate::running::{Running, Runs, Stretch};

/// What the builder needs to know about the document as a whole.
pub(crate) struct Input<'a> {
    pub fonts: &'a FontTable,
    pub stats: &'a Stats,
    /// Each page's size and the margins of its size.
    pub pages: Vec<(f32, f32, Margins)>,
    pub regions: Vec<Vec<Region>>,
    pub header: Runs,
    pub footer: Runs,
    /// The stretches of pages that share their headers and footers.
    pub stretches: Vec<Stretch>,
    /// Odd and even pages have their own headers or footers.
    pub even_odd: bool,
    pub notes: Vec<crate::notes::Note>,
    /// The text came from TeX, which hyphenates: a hyphen at a line end goes.
    pub dehyphenate: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

/// The character formatting a paragraph style gives.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    family: String,
    size: f32,
    bold: bool,
    italic: bool,
    color: Rgb,
}

fn span_look(s: &Span, fonts: &FontTable) -> Look {
    let f = fonts.get(s.font);
    Look {
        family: f.map_or_else(|| "Times New Roman".to_string(), |f| f.family.clone()),
        size: s.size,
        bold: s.bold || f.is_some_and(|f| f.bold),
        italic: f.is_some_and(|f| f.italic),
        color: s.color,
    }
}

/// The look of most of a paragraph's text.
fn para_look(p: &Para, fonts: &FontTable) -> Option<Look> {
    let mut counts: Vec<(Look, usize)> = Vec::new();
    for s in p.lines.iter().flat_map(|l| l.spans.iter()).filter(|s| s.vert == Vert::Base) {
        let n = s.text.chars().filter(|c| !c.is_whitespace()).count();
        let look = span_look(s, fonts);
        match counts.iter_mut().find(|(l, _)| *l == look) {
            Some(c) => c.1 += n,
            None => counts.push((look, n)),
        }
    }
    counts.into_iter().max_by_key(|c| c.1).map(|c| c.0)
}

/// How a paragraph is set: alignment, indents (from its column's left edge), line spacing.
#[derive(Clone, Debug)]
struct Shape {
    align: Align,
    /// Alignment can't be told from the paragraph (one short line): the style's.
    align_known: bool,
    left: f32,
    first: f32,
    right: f32,
    line: Option<LineSpacing>,
    /// Height of its first line under `line`.
    line_height: f32,
    descent: f32,
}

fn natural(l: &Line, fonts: &FontTable) -> (f32, f32) {
    let f = fonts.get(l.font);
    (f.map_or(1.15, |f| f.line) * l.size, f.map_or(0.22, |f| f.descent) * l.size)
}

fn spacing_rule(pitch: f32, natural: f32) -> LineSpacing {
    let r = pitch / natural.max(0.1);
    for c in [1.0, 1.08, 1.15, 1.2, 1.25, 1.3, 1.5, 2.0, 2.5, 3.0] {
        if (r - c).abs() <= 0.012 * c {
            return LineSpacing::Multiple(c);
        }
    }
    let p = (pitch * 20.0).round() / 20.0;
    if pitch >= natural { LineSpacing::AtLeast(p) } else { LineSpacing::Exactly(p) }
}

fn rule_height(rule: &LineSpacing, natural: f32) -> f32 {
    match rule {
        LineSpacing::Multiple(m) => m * natural,
        LineSpacing::AtLeast(p) => p.max(natural),
        LineSpacing::Exactly(p) => *p,
    }
}

fn shape(p: &Para, fonts: &FontTable, stats: &Stats) -> Shape {
    let col = p.col;
    let (Some(first), Some(last)) = (p.lines.first(), p.lines.last()) else {
        return Shape { align: Align::Left, align_known: false, left: 0.0, first: 0.0, right: 0.0, line: None, line_height: 12.0, descent: 3.0 };
    };
    let (nat, desc) = natural(first, fonts);
    let lb = label(first);
    let n = p.lines.len();
    let tabbed = p.lines.iter().enumerate().any(|(i, l)| l.pieces.len() > 1 && !(i == 0 && crate::reflow::label_tabbed(l)));
    let max_x1 = p.lines.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
    let col_c = (col.0 + col.1) / 2.0;
    // Alignment.
    let (align, known) = if tabbed {
        (Align::Left, true)
    } else if n >= 2 {
        let body = p.lines.get(..n - 1).unwrap_or(&[]);
        let at_edge = body.iter().filter(|l| l.x1 >= max_x1 - 1.5).count();
        let space = |l: &Line| fonts.get(l.font).map_or(0.25, |f| f.space);
        let stretched = body
            .iter()
            .filter(|l| {
                let g = &l.word_gaps;
                !g.is_empty() && g.iter().sum::<f32>() / g.len() as f32 > 1.1 * space(l) + 0.02
            })
            .count();
        let centres = p.lines.iter().all(|l| ((l.x0 + l.x1) / 2.0 - col_c).abs() < 2.5);
        let lefts = p.lines.iter().skip(1).all(|l| (l.x0 - p.lines.get(1).map_or(l.x0, |s| s.x0)).abs() < 2.0);
        let rights = p.lines.iter().all(|l| (l.x1 - col.1).abs() < 2.0);
        if at_edge * 10 >= body.len() * 7 && stretched * 2 >= body.len() && lefts {
            (Align::Justify, true)
        } else if centres && (!lefts || first.x0 > col.0 + 12.0) {
            (Align::Center, true)
        } else if rights && !lefts {
            (Align::Right, true)
        } else {
            (Align::Left, at_edge * 10 < body.len() * 7 || stretched * 2 < body.len())
        }
    } else if ((first.x0 + first.x1) / 2.0 - col_c).abs() < 2.5 && first.x0 > col.0 + 12.0 {
        (Align::Center, true)
    } else if (first.x1 - col.1).abs() < 2.0 && first.x0 > col.0 + 0.4 * (col.1 - col.0) {
        (Align::Right, true)
    } else {
        (Align::Left, false)
    };
    // Indents.
    let (mut left, mut firstx, mut right) = (0.0, 0.0, 0.0);
    match align {
        Align::Center => {
            let c = p.lines.iter().map(|l| (l.x0 + l.x1) / 2.0).sum::<f32>() / n as f32;
            let off = 2.0 * (c - col_c);
            if off > 2.0 {
                left = off;
            } else if off < -2.0 {
                right = -off;
            }
        }
        Align::Right => {
            right = (col.1 - max_x1).max(0.0);
        }
        _ => {
            let text_x =
                if n >= 2 { p.lines.iter().skip(1).map(|l| l.x0).fold(f32::MAX, f32::min) } else { lb.as_ref().map_or(first.x0, |b| b.text_x) };
            left = text_x - col.0;
            firstx = first.x0 - text_x;
            if align == Align::Justify && col.1 - max_x1 > 2.0 {
                right = col.1 - max_x1;
            }
        }
    }
    // Line spacing: the paragraph's own pitch, or the usual one for its size.
    let mut pitches: Vec<f32> = p.lines.windows(2).map(|w| w[1].base - w[0].base).filter(|d| *d > 0.0).collect();
    pitches.sort_by(f32::total_cmp);
    let pitch = pitches.get(pitches.len() / 2).copied().filter(|d| *d < 3.5 * first.size).or_else(|| stats.pitch(first.size));
    let line = pitch.map(|d| spacing_rule(d, nat));
    let line_height = line.as_ref().map_or(nat, |r| rule_height(r, nat));
    let _ = last;
    Shape { align, align_known: known, left: snap(left), first: snap(firstx), right: snap(right), line, line_height, descent: desc }
}

fn snap(v: f32) -> f32 {
    if v.abs() < 1.0 { 0.0 } else { nice(v, 0.5) }
}

/// A list a paragraph belongs to.
#[derive(Clone, Copy, Debug)]
struct ListRef {
    num: u32,
    /// The label is the list's: leave it out of the text.
    strip: bool,
}

/// Number formats of enumerators: (format, template, value).
fn parse_enum(tok: &str, prev_format: Option<NumFormat>) -> Option<(NumFormat, String, u32)> {
    let t = tok.trim();
    let (open, inner) = match t.strip_prefix('(') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let close = inner.chars().last()?;
    if !matches!(close, '.' | ')') {
        return None;
    }
    let body = inner.get(..inner.len() - close.len_utf8())?;
    if body.is_empty() || body.contains('.') {
        // `1.2.` numbers several levels: kept as text.
        return None;
    }
    let template = if open { "(%1)".to_string() } else { format!("%1{close}") };
    if open && close != ')' {
        return None;
    }
    if body.chars().all(|c| c.is_ascii_digit()) {
        return Some((NumFormat::Decimal, template, body.parse().ok()?));
    }
    let roman = |s: &str| -> Option<u32> {
        let mut total = 0u32;
        let mut prev = 0u32;
        for c in s.chars().rev() {
            let v = match c.to_ascii_lowercase() {
                'i' => 1,
                'v' => 5,
                'x' => 10,
                'l' => 50,
                'c' => 100,
                _ => return None,
            };
            if v < prev {
                total = total.checked_sub(v)?;
            } else {
                total += v;
                prev = v;
            }
        }
        Some(total)
    };
    let lower = body.chars().all(|c| c.is_ascii_lowercase());
    let upper = body.chars().all(|c| c.is_ascii_uppercase());
    let as_roman = roman(body).filter(|_| {
        body.chars().count() > 1 || matches!(prev_format, Some(NumFormat::LowerRoman | NumFormat::UpperRoman)) || body.eq_ignore_ascii_case("i")
    });
    if let Some(v) = as_roman
        && (lower || upper)
    {
        return Some((if lower { NumFormat::LowerRoman } else { NumFormat::UpperRoman }, template, v));
    }
    let mut cs = body.chars();
    let c = cs.next()?;
    let single = cs.next().is_none();
    if single && c.is_ascii_alphabetic() {
        let v = u32::from(c.to_ascii_lowercase() as u8 - b'a') + 1;
        return Some((if lower { NumFormat::LowerLetter } else { NumFormat::UpperLetter }, template, v));
    }
    // Cyrillic letters, counted as Word counts Russian lists (other alphabets stay text).
    if single && c.is_alphabetic() {
        let lc = c.to_lowercase().next()?;
        let v = (1..=29u32).find(|n| wordcraft_doc::section::russian_letters(*n).starts_with(lc))?;
        return Some((if c.is_lowercase() { NumFormat::RussianLower } else { NumFormat::RussianUpper }, template, v));
    }
    None
}

struct Builder<'a> {
    doc: Document,
    inp: &'a Input<'a>,
    normal: Look,
    normal_line: Option<LineSpacing>,
    normal_align: Align,
    /// Heading level by look.
    headings: Vec<(Look, String)>,
    /// Footnote parts made so far, by note.
    note_parts: HashMap<usize, u32>,
}

impl Builder<'_> {
    /// The footnote part for note `id`, made the first time it is referred to.
    fn note_part(&mut self, id: usize) -> u32 {
        if let Some(p) = self.note_parts.get(&id) {
            return *p;
        }
        let pid = self.doc.add_part(PartKind::Footnote, Blocks::new());
        self.note_parts.insert(id, pid);
        let inp = self.inp;
        let Some(note) = inp.notes.get(id) else { return pid };
        let Some((w, _, m)) = inp.pages.get(note.page).copied() else { return pid };
        let col = (m.left, w - m.right);
        let paras = crate::reflow::paragraphs(note.lines.clone(), col, note.page, inp.fonts, inp.stats);
        let mut blocks = Blocks::new();
        for p in &paras {
            let mut para = self.paragraph(p, None, true);
            para.props.style = Some("FootnoteText".into());
            blocks.push(para_block(para));
        }
        if !blocks.is_empty()
            && let Some(part) = self.doc.parts.get_mut(&pid)
        {
            part.blocks = blocks;
        }
        pid
    }

    fn new_num(&mut self, level: Level) -> u32 {
        let numbering = &mut self.doc.numbering;
        let aid = numbering.abstracts.iter().map(|a| a.id + 1).max().unwrap_or(0);
        let mut levels = wordcraft_doc::numbering::levels_for(if level.format == NumFormat::Bullet {
            wordcraft_doc::ListKind::Bullet
        } else {
            wordcraft_doc::ListKind::Numbered
        });
        if let Some(l0) = levels.get_mut(0) {
            *l0 = level;
        }
        numbering.abstracts.push(AbstractNum { id: aid, name: None, levels, ..Default::default() });
        let nid = numbering.nums.iter().map(|n| n.id + 1).max().unwrap_or(1).max(1);
        numbering.nums.push(Num { id: nid, abstract_id: aid, ..Default::default() });
        nid
    }

    fn char_props(&self, s: &Span, style: &Look) -> CharProps {
        let look = span_look(s, self.inp.fonts);
        let mut cp = CharProps::default();
        if look.family != style.family {
            cp.font = Some(look.family.clone());
        }
        if (look.size - style.size).abs() > 0.01 {
            cp.size = Some(look.size.clamp(1.0, 1638.0));
        }
        if look.bold != style.bold {
            cp.bold = Some(look.bold);
        }
        if look.italic != style.italic {
            cp.italic = Some(look.italic);
        }
        if look.color != style.color {
            cp.color = Some(if look.color == Rgb(0, 0, 0) { TextColor::Auto } else { TextColor::Rgb(look.color) });
        }
        if s.underline {
            cp.underline = Some(Underline::Single);
        }
        if s.strike {
            cp.strike = Some(true);
        }
        cp.vert_align = match s.vert {
            Vert::Base => None,
            Vert::Sup => Some(VertAlign::Superscript),
            Vert::Sub => Some(VertAlign::Subscript),
        };
        cp.link.clone_from(&s.link);
        // Behind the text: Word's highlighter where the colour is one of its own, else shading.
        if let Some(c) = s.back {
            match Highlight::ALL.iter().filter(|h| !matches!(h, Highlight::White)).find(|h| h.rgb() == Some(c)) {
                Some(h) => cp.highlight = Some(*h),
                None => cp.shading = Some(c),
            }
        }
        cp
    }

    /// The paragraph style for a look: a heading's, or Normal.
    fn style_for(&self, look: Option<&Look>) -> (Option<String>, Look) {
        if let Some(l) = look
            && let Some((_, id)) = self.headings.iter().find(|(h, _)| h == l)
        {
            return (Some(id.clone()), l.clone());
        }
        (None, self.normal.clone())
    }
}

fn push_text(p: &mut Paragraph, text: &str, props: &CharProps) {
    if text.is_empty() {
        return;
    }
    p.text.push_str(text);
    match p.runs.last_mut() {
        Some(last) if last.props == *props => last.len += text.len(),
        _ => p.runs.push(Run { len: text.len(), props: props.clone() }),
    }
}

fn push_obj(p: &mut Paragraph, obj: InlineObject, props: &CharProps) {
    p.objects.push(obj);
    push_text(p, &OBJ.to_string(), props);
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

fn edge_border(e: Edge) -> Border {
    match e {
        Some((w, c)) => Border { style: BorderStyle::Single, width: (w * 8.0).round().clamp(2.0, 96.0) / 8.0, color: Some(c), space: 0.0 },
        None => Border { style: BorderStyle::None, width: 0.0, color: None, space: 0.0 },
    }
}

/// A page number field (`PAGE`) or page count (`NUMPAGES`).
fn field(page: bool) -> InlineObject {
    InlineObject::Field {
        instr: if page { " PAGE ".into() } else { " NUMPAGES ".into() },
        result: "1".into(),
        locked: false,
        dirty: false,
        code: false,
        form: String::new(),
    }
}

/// Dots (or other leader characters) standing between text and a number at the line's end.
fn leader_split(text: &str) -> Option<(usize, usize, TabLeader)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (b, c) = chars.get(i).copied().unwrap_or((0, ' '));
        let leader = match c {
            '.' | '…' => Some(TabLeader::Dot),
            '_' => Some(TabLeader::Underscore),
            '·' => Some(TabLeader::MiddleDot),
            _ => None,
        };
        if let Some(kind) = leader {
            let mut j = i;
            let mut count = 0;
            while let Some((_, d)) = chars.get(j).copied() {
                if d == c || (d == ' ' && kind != TabLeader::Underscore) {
                    if d == c {
                        count += if d == '…' { 3 } else { 1 };
                    }
                    j += 1;
                } else {
                    break;
                }
            }
            let end = chars.get(j).map_or(text.len(), |(e, _)| *e);
            if count >= 5 && j < chars.len() {
                return Some((b, end, kind));
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    None
}

impl Builder<'_> {
    /// One analysed paragraph as a document paragraph.
    fn paragraph(&mut self, p: &Para, list: Option<ListRef>, in_cell: bool) -> Paragraph {
        let fonts = self.inp.fonts;
        let mut out = Paragraph::new();
        let look = para_look(p, fonts);
        let (style, style_look) = if in_cell { (None, self.normal.clone()) } else { self.style_for(look.as_ref()) };
        out.props.style = style.clone();
        if let Some(pic) = &p.picture {
            self.picture_para(&mut out, pic, p.col);
            out.props.space_before = p.gap_above.map(|g| (g.max(0.0) * 2.0).round() / 2.0).filter(|g| *g > 0.0);
            out.normalize();
            out.touch();
            return out;
        }
        let sh = shape(p, fonts, self.inp.stats);
        let heading = style.is_some();
        // Alignment, indents.
        if sh.align_known || sh.align != Align::Left {
            if sh.align != self.normal_align || heading {
                out.props.align = Some(sh.align);
            }
        } else if self.normal_align != Align::Left && sh.align == Align::Left {
            // One short line: it looks the same either way; keep the style's.
        }
        let labelled = list.is_some_and(|l| l.strip);
        // A list item sets its own even where they are 0: else its list level's would apply (an
        // item wrapping back to the margin under its label).
        if sh.left != 0.0 || list.is_some() {
            out.props.indent_left = Some(sh.left);
        }
        if sh.first != 0.0 || list.is_some() {
            out.props.indent_first = Some(sh.first);
        }
        if sh.right != 0.0 {
            out.props.indent_right = Some(sh.right);
        }
        if let Some(l) = list {
            out.props.numbering = Some(NumRef { num: l.num, level: 0 });
        }
        // Line spacing, when it differs from the style's.
        let style_line = if heading { None } else { self.normal_line };
        if sh.line != style_line && sh.line.is_some() {
            out.props.line_spacing = sh.line;
        }
        // Space before: the baseline distance from the text above less this line's height;
        // after a table or picture, the gap less what lies above this line's text.
        let first = p.lines.first();
        let before = match (p.above_base, first, p.gap_above) {
            (Some(b), Some(f), _) => Some(f.base - b - sh.line_height),
            (None, Some(f), Some(g)) => Some(g - ((sh.line_height - sh.descent) - (f.base - f.top()))),
            _ if p.page_break => first.map(|f| f.base - (sh.line_height - sh.descent) - self.page_margin_top(p.page)),
            _ => None,
        };
        if let Some(b) = before {
            let b = ((b * 2.0).round() / 2.0).clamp(0.0, 400.0);
            if b > 0.0 {
                out.props.space_before = Some(b);
            }
        }
        if p.gap_below > 0.5 {
            out.props.space_after = Some(((p.gap_below - sh.descent).max(0.0) * 2.0).round() / 2.0);
        }
        if heading {
            out.props.space_before = Some(out.props.space_before.unwrap_or(0.0));
        }
        if p.page_break {
            out.props.page_break_before = Some(true);
        }
        out.props.shading = p.shading.filter(|c| *c != Rgb(255, 255, 255));
        if let Some((w, c)) = p.rule_below {
            out.props.borders = Some(Borders {
                bottom: Some(Border { style: BorderStyle::Single, width: (w * 8.0).round().clamp(2.0, 48.0) / 8.0, color: Some(c), space: 1.0 }),
                ..Default::default()
            });
        }
        // Tab stops: each piece after the first starts at one; a last piece ending at the
        // column's edge is right aligned there.
        let mut tabs: Vec<TabStop> = Vec::new();
        for l in &p.lines {
            let n = l.pieces.len();
            for (i, piece) in l.pieces.iter().enumerate().skip(1) {
                let right = i + 1 == n && (piece.1 - p.col.1).abs() < 2.5 && piece.0 > (p.col.0 + p.col.1) / 2.0;
                let stop = if right {
                    TabStop { pos: snap(piece.1 - p.col.0), align: TabAlign::Right, leader: TabLeader::None }
                } else {
                    TabStop { pos: snap(piece.0 - p.col.0), align: TabAlign::Left, leader: TabLeader::None }
                };
                if !tabs.iter().any(|t| (t.pos - stop.pos).abs() < 1.0) {
                    tabs.push(stop);
                }
            }
        }
        // Text.
        let mut text_started = false;
        let dehyph = self.inp.dehyphenate;
        let n = p.lines.len();
        for (li, l) in p.lines.iter().enumerate() {
            let mut spans: Vec<Span> = l.spans.clone();
            if li == 0 && labelled {
                // Drop the label and the gap after it.
                if let Some(lb) = label(l) {
                    let mut to_drop = lb.text.chars().count();
                    while to_drop > 0 && !spans.is_empty() {
                        let Some(s) = spans.first_mut() else { break };
                        let len = s.text.chars().count();
                        if len <= to_drop {
                            to_drop -= len;
                            spans.remove(0);
                        } else {
                            s.text = s.text.chars().skip(to_drop).collect();
                            to_drop = 0;
                        }
                    }
                    if let Some(s) = spans.first_mut() {
                        s.text = s.text.trim_start().to_string();
                    }
                    spans.retain(|s| !s.text.is_empty());
                }
            }
            // Leader dots before a number at the right edge: a right tab with a dot leader.
            let line_text: String = spans.iter().map(|s| s.text.as_str()).collect();
            if l.pieces.len() == 1
                && (l.x1 - p.col.1).abs() < 3.0
                && let Some((from, to, kind)) = leader_split(&line_text)
            {
                tabs.push(TabStop { pos: snap(l.x1 - p.col.0), align: TabAlign::Right, leader: kind });
                let mut pos = 0usize;
                let mut tab_done = false;
                for s in &mut spans {
                    let (a, b) = (pos, pos + s.text.len());
                    pos = b;
                    let keep: String = s
                        .text
                        .char_indices()
                        .filter_map(|(i, c)| {
                            let at = a + i;
                            if at >= from && at < to { None } else { Some(c) }
                        })
                        .collect();
                    if !tab_done && to > a && to <= b {
                        let cut = to.saturating_sub(a).min(s.text.len());
                        let head: String = s.text.get(..cut).unwrap_or("").chars().filter(|_| false).collect();
                        let _ = head;
                        let before_part: String =
                            keep.chars().take(keep.chars().count() - s.text.get(cut..).map_or(0, |t| t.chars().count())).collect();
                        let after_part: String = s.text.get(cut..).unwrap_or("").to_string();
                        s.text = format!("{}\t{}", before_part.trim_end(), after_part);
                        tab_done = true;
                    } else {
                        s.text = keep;
                    }
                }
                if !tab_done && let Some(s) = spans.last_mut() {
                    s.text.push('\t');
                }
                spans.retain(|s| !s.text.is_empty());
            }
            // Join to the line before.
            if li > 0 && text_started {
                let prev_end = out.text.chars().last();
                let next_start = spans.first().and_then(|s| s.text.chars().next());
                match (prev_end, next_start) {
                    (Some('-'), Some(c)) if dehyph && c.is_lowercase() => {
                        out.text.pop();
                        if let Some(r) = out.runs.last_mut() {
                            r.len = r.len.saturating_sub(1);
                        }
                    }
                    (Some('\u{AD}'), _) => {
                        out.text.pop();
                        if let Some(r) = out.runs.last_mut() {
                            r.len = r.len.saturating_sub(2);
                        }
                    }
                    (Some('-' | '/' | '\u{2010}' | '\u{2013}'), _) => {}
                    (Some(a), Some(b)) if is_cjk(a) && is_cjk(b) => {}
                    (Some(_), Some(_)) => {
                        let props = out.runs.last().map(|r| r.props.clone()).unwrap_or_default();
                        let props = CharProps { underline: None, strike: None, link: None, ..props };
                        push_text(&mut out, " ", &props);
                    }
                    _ => {}
                }
            }
            for s in &spans {
                if let Some(id) = s.note {
                    let pid = self.note_part(id);
                    let mark = InlineObject::NoteRef { kind: NoteKind::Footnote, id: pid, custom: String::new() };
                    push_obj(&mut out, mark, &CharProps { style: Some("FootnoteReference".into()), ..Default::default() });
                    text_started = true;
                    continue;
                }
                let cp = self.char_props(s, &style_look);
                push_text(&mut out, &s.text, &cp);
                text_started = true;
            }
            let _ = n;
        }
        out.runs.retain(|r| r.len > 0);
        if !tabs.is_empty() {
            tabs.sort_by(|a, b| a.pos.total_cmp(&b.pos));
            out.props.tabs = Some(tabs);
        }
        // Floating pictures anchored here.
        for pic in &p.floats {
            self.float_picture(&mut out, pic);
        }
        // The paragraph mark looks like the end of its text.
        out.mark =
            out.runs.last().map(|r| CharProps { underline: None, strike: None, link: None, vert_align: None, ..r.props.clone() }).unwrap_or_default();
        out.normalize();
        out.touch();
        out
    }

    fn page_margin_top(&self, page: usize) -> f32 {
        self.inp.pages.get(page).map_or(72.0, |p| p.2.top)
    }

    fn picture_para(&mut self, out: &mut Paragraph, pic: &Pic, col: (f32, f32)) {
        let (w, h) = ((pic.rect.width() as f32).clamp(1.0, 1584.0), (pic.rect.height() as f32).clamp(1.0, 1584.0));
        let key = self.doc.add_media(pic.data.to_vec(), pic.ext);
        let obj = InlineObject::Image { media: key, w, h, alt: String::new(), float: Float::default(), crop: [0.0; 4], ole: None };
        let x0 = pic.rect.x0 as f32;
        let centre = (pic.rect.x0 + pic.rect.x1) as f32 / 2.0;
        if (centre - (col.0 + col.1) / 2.0).abs() < 3.0 && x0 > col.0 + 6.0 {
            out.props.align = Some(Align::Center);
        } else if ((pic.rect.x1 as f32) - col.1).abs() < 2.0 && x0 > col.0 + 6.0 {
            out.props.align = Some(Align::Right);
        } else {
            out.props.align = Some(Align::Left);
            let ind = snap(x0 - col.0);
            if ind != 0.0 {
                out.props.indent_left = Some(ind);
            }
        }
        // A picture's line grows to the picture: single spacing, whatever Normal says.
        out.props.line_spacing = Some(LineSpacing::Multiple(1.0));
        push_obj(out, obj, &CharProps::default());
    }

    fn float_picture(&mut self, out: &mut Paragraph, pic: &Pic) {
        let (w, h) = ((pic.rect.width() as f32).clamp(1.0, 1584.0), (pic.rect.height() as f32).clamp(1.0, 1584.0));
        let key = self.doc.add_media(pic.data.to_vec(), pic.ext);
        let float = Float {
            wrap: if pic.behind { Wrap::BehindText } else { Wrap::Square },
            h_rel: Anchor::Page,
            v_rel: Anchor::Page,
            x: pic.rect.x0 as f32,
            y: pic.rect.y0 as f32,
            dist: 6.0,
            ..Default::default()
        };
        let obj = InlineObject::Image { media: key, w, h, alt: String::new(), float, crop: [0.0; 4], ole: None };
        // Anchored at the start of the paragraph.
        let props = CharProps::default();
        let mut head = Paragraph::new();
        push_obj(&mut head, obj, &props);
        let mut text = std::mem::take(&mut out.text);
        let mut runs = std::mem::take(&mut out.runs);
        let mut objects = std::mem::take(&mut out.objects);
        out.text = head.text;
        out.runs = head.runs;
        out.objects = head.objects;
        out.text.push_str(&text);
        out.runs.append(&mut runs);
        out.objects.append(&mut objects);
        text.clear();
    }

    fn table(&mut self, t: &Tab, margin_left: f32) -> wordcraft_doc::Table {
        let ncols = t.xs.len().saturating_sub(1).max(1);
        let mut tb = wordcraft_doc::Table::new(1, ncols, 100.0);
        tb.props.style = None;
        tb.grid = t.xs.windows(2).map(|w| (w[1] - w[0]).clamp(2.0, 1584.0)).collect();
        if tb.grid.is_empty() {
            tb.grid = vec![100.0];
        }
        let first_border = t.rows.first().and_then(|r| r.first()).and_then(|c| c.edges[1]).map_or(0.0, |e| e.0);
        let indent = t.xs.first().copied().unwrap_or(margin_left) - margin_left - first_border / 2.0;
        if indent.abs() >= 0.5 {
            tb.props.indent = Some((indent * 2.0).round() / 2.0);
        }
        tb.props.cell_margins = Some([0.0, 5.4, 0.0, 5.4]);
        tb.rows.clear();
        let mut col = 0usize;
        for (ri, r) in t.rows.iter().enumerate() {
            let h = t.ys.get(ri + 1).copied().unwrap_or(0.0) - t.ys.get(ri).copied().unwrap_or(0.0);
            let mut row = wordcraft_doc::Row {
                props: RowProps {
                    height: Some((h - 0.5).max(1.0)),
                    height_rule: HeightRule::AtLeast,
                    header: ri < t.header_rows,
                    ..Default::default()
                },
                cells: Vec::new(),
                controls: Default::default(),
            };
            col = 0;
            for c in r {
                let w: f32 = tb.grid.get(col..(col + c.span).min(tb.grid.len())).map_or(72.0, |s| s.iter().sum());
                col += c.span;
                let mut blocks: Blocks = Blocks::new();
                if c.vmerge != 2 {
                    for p in &c.paras {
                        let para = self.paragraph(p, None, true);
                        blocks.push(para_block(para));
                    }
                }
                if blocks.is_empty() {
                    let mut empty = Paragraph::new();
                    empty.mark = CharProps { size: Some(self.normal.size), ..Default::default() };
                    blocks.push(para_block(empty));
                }
                row.cells.push(wordcraft_doc::Cell {
                    props: CellProps {
                        width: Some(w),
                        span: c.span.clamp(1, 63) as u32,
                        vmerge: match c.vmerge {
                            1 => VMerge::Restart,
                            2 => VMerge::Continue,
                            _ => VMerge::None,
                        },
                        shading: c.shading.filter(|s| *s != Rgb(255, 255, 255)),
                        borders: Some(Borders {
                            top: Some(edge_border(c.edges[0])),
                            left: Some(edge_border(c.edges[1])),
                            bottom: Some(edge_border(c.edges[2])),
                            right: Some(edge_border(c.edges[3])),
                            ..Default::default()
                        }),
                        valign: match c.valign {
                            1 => Some(VAlign::Center),
                            2 => Some(VAlign::Bottom),
                            _ => None,
                        },
                        ..Default::default()
                    },
                    blocks,
                    controls: Default::default(),
                });
            }
            if row.cells.is_empty() {
                row.cells.push(wordcraft_doc::Cell::empty());
            }
            tb.rows.push(row);
        }
        let _ = col;
        tb
    }
}

/// Paragraph bodies in document order, for the passes that look across them.
fn body_paras(regions: &[Vec<Region>]) -> Vec<&Para> {
    regions.iter().flatten().flat_map(|r| r.items.iter()).filter_map(|i| if let Item::Para(p) = i { Some(p) } else { None }).collect()
}

pub(crate) fn document(inp: &Input<'_>) -> Document {
    let fonts = inp.fonts;
    let mut doc = Document::new();
    doc.settings.compat_mode = 15;
    // Normal: the commonest look of body text.
    let paras = body_paras(&inp.regions);
    let mut looks: Vec<(Look, usize)> = Vec::new();
    for p in &paras {
        for s in p.lines.iter().flat_map(|l| l.spans.iter()).filter(|s| s.vert == Vert::Base) {
            let look = span_look(s, fonts);
            let n = s.text.chars().filter(|c| !c.is_whitespace()).count();
            match looks.iter_mut().find(|(l, _)| *l == look) {
                Some(c) => c.1 += n,
                None => looks.push((look, n)),
            }
        }
    }
    let normal = looks.iter().max_by_key(|c| c.1).map(|c| c.0.clone()).unwrap_or(Look {
        family: "Times New Roman".into(),
        size: 12.0,
        bold: false,
        italic: false,
        color: Rgb(0, 0, 0),
    });
    // Headings: short paragraphs set larger than Normal, or bold where Normal isn't.
    let mut groups: Vec<(Look, usize, usize)> = Vec::new();
    let total = paras.iter().filter(|p| !p.lines.is_empty()).count().max(1);
    for p in &paras {
        let chars: usize = p.lines.iter().map(Line::chars).sum();
        if p.lines.is_empty() || p.lines.len() > 3 || chars > 160 || p.lines.iter().any(|l| l.pieces.len() > 1) {
            continue;
        }
        if p.lines.first().and_then(label).is_some_and(|l| l.bullet) {
            continue;
        }
        let Some(look) = para_look(p, fonts) else { continue };
        let bigger = look.size >= normal.size * 1.12;
        let bolder = look.bold && !normal.bold && look.size >= normal.size - 0.1 && chars <= 100 && p.lines.iter().all(|l| l.all_bold(fonts));
        if !(bigger || bolder) {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == look) {
            Some(g) => g.1 += 1,
            None => groups.push((look, 1, p.page)),
        }
    }
    groups.retain(|g| g.1 * 10 <= total * 4);
    groups.sort_by(|a, b| b.0.size.total_cmp(&a.0.size).then(b.0.bold.cmp(&a.0.bold)));
    let mut headings: Vec<(Look, String)> = Vec::new();
    let mut level = 0;
    for (i, g) in groups.iter().enumerate() {
        if i == 0 && g.1 == 1 && g.2 == 0 && g.0.size >= normal.size * 1.6 && groups.len() > 1 {
            headings.push((g.0.clone(), "Title".into()));
            continue;
        }
        level += 1;
        if level > 6 {
            break;
        }
        headings.push((g.0.clone(), format!("Heading{level}")));
    }
    // Normal's line spacing and alignment: the commonest among body paragraphs of its look.
    let mut rules: Vec<(LineSpacing, usize)> = Vec::new();
    let mut aligns: Vec<(Align, usize)> = Vec::new();
    for p in &paras {
        if p.lines.len() < 2 || para_look(p, fonts).as_ref() != Some(&normal) {
            continue;
        }
        let sh = shape(p, fonts, inp.stats);
        if let Some(r) = sh.line {
            match rules.iter_mut().find(|x| x.0 == r) {
                Some(x) => x.1 += p.lines.len(),
                None => rules.push((r, p.lines.len())),
            }
        }
        if sh.align_known {
            match aligns.iter_mut().find(|x| x.0 == sh.align) {
                Some(x) => x.1 += p.lines.len(),
                None => aligns.push((sh.align, p.lines.len())),
            }
        }
    }
    let normal_line = rules.iter().max_by_key(|x| x.1).map(|x| x.0);
    let normal_align = aligns.iter().max_by_key(|x| x.1).map_or(Align::Left, |x| x.0);
    // Styles.
    let color = |c: Rgb| if c == Rgb(0, 0, 0) { None } else { Some(TextColor::Rgb(c)) };
    let normal_chr = CharProps {
        font: Some(normal.family.clone()),
        size: Some(normal.size),
        bold: normal.bold.then_some(true),
        italic: normal.italic.then_some(true),
        color: color(normal.color),
        ..Default::default()
    };
    let normal_para = ParaProps {
        space_before: Some(0.0),
        space_after: Some(0.0),
        line_spacing: Some(normal_line.unwrap_or(LineSpacing::Multiple(1.0))),
        align: (normal_align != Align::Left).then_some(normal_align),
        widow_control: Some(true),
        ..Default::default()
    };
    doc.styles.default_chr =
        CharProps { font: Some(normal.family.clone()), size: Some(normal.size), lang: doc.styles.default_chr.lang.clone(), ..Default::default() };
    doc.styles.default_para = ParaProps {
        space_before: Some(0.0),
        space_after: Some(0.0),
        line_spacing: Some(LineSpacing::Multiple(1.0)),
        widow_control: Some(true),
        ..Default::default()
    };
    if let Some(st) = doc.styles.styles.iter_mut().find(|s| s.id == "Normal") {
        st.para = normal_para;
        st.chr = normal_chr;
    }
    for (i, (look, id)) in headings.iter().enumerate() {
        let is_title = id == "Title";
        let name = if is_title { "Title".to_string() } else { format!("Heading {}", id.trim_start_matches("Heading")) };
        let lvl = id.trim_start_matches("Heading").parse::<u8>().unwrap_or(1);
        doc.styles.upsert(Style {
            id: id.clone(),
            name,
            kind: StyleKind::Paragraph,
            based_on: Some("Normal".into()),
            next: Some("Normal".into()),
            para: ParaProps {
                keep_next: Some(true),
                keep_lines: Some(true),
                outline_level: if is_title { None } else { Some(lvl.saturating_sub(1)) },
                space_before: Some(0.0),
                space_after: Some(0.0),
                line_spacing: Some(LineSpacing::Multiple(1.0)),
                align: Some(Align::Left),
                ..Default::default()
            },
            chr: CharProps {
                font: Some(look.family.clone()),
                size: Some(look.size),
                bold: Some(look.bold),
                italic: Some(look.italic),
                color: Some(color(look.color).unwrap_or(TextColor::Auto)),
                ..Default::default()
            },
            priority: Some(9 + i as u32),
            quick: true,
            ..Default::default()
        });
    }
    let mut b = Builder { doc, inp, normal: normal.clone(), normal_line, normal_align, headings, note_parts: HashMap::new() };
    // Lists: bullets by label and position; numbers only where they count up.
    let lists = lists(&mut b);
    // Sections and body.
    let mut body: Blocks = Blocks::new();
    let mut section_of_page: Vec<SectionProps> = Vec::new();
    for (pi, (w, h, m)) in inp.pages.iter().enumerate() {
        let mut s = SectionProps {
            page_w: *w,
            page_h: *h,
            landscape: w > h,
            margin_top: m.top,
            margin_bottom: m.bottom,
            margin_left: m.left,
            margin_right: m.right,
            ..Default::default()
        };
        s.header = 36.0;
        s.footer = 36.0;
        let _ = pi;
        section_of_page.push(s);
    }
    // Headers and footers: a part for each look, and an empty one for the pages without any.
    let header_parts: Vec<Option<(u32, f32)>> = inp.header.looks.iter().map(|r| running_part(&mut b, r, true)).collect();
    let footer_parts: Vec<Option<(u32, f32)>> = inp.footer.looks.iter().map(|r| running_part(&mut b, r, false)).collect();
    let (mut empty_header, mut empty_footer) = (None, None);
    let mut sets: Vec<StretchParts> = Vec::new();
    for st in &inp.stretches {
        let mut r = StretchParts { title_page: st.first.is_some(), restart: st.restart, ..Default::default() };
        let mut slot = |look: Option<usize>, header: bool| {
            let (parts, empty) = if header { (&header_parts, &mut empty_header) } else { (&footer_parts, &mut empty_footer) };
            part_or_empty(&mut b, parts, look, empty, header)
        };
        r.headers.default = slot(st.default.0, true);
        r.footers.default = slot(st.default.1, false);
        if let Some(e) = st.even {
            r.headers.even = slot(e.0, true);
            r.footers.even = slot(e.1, false);
        }
        if let Some(f) = st.first {
            r.headers.first = slot(f.0, true);
            r.footers.first = slot(f.1, false);
        }
        // How far from the edge: as the stretch's own text sits, else as the document's does.
        let dist = |parts: &[Option<(u32, f32)>], looks: [Option<usize>; 3]| {
            looks.into_iter().flatten().chain(0..parts.len()).find_map(|l| parts.get(l).copied().flatten()).map_or(36.0, |p| p.1)
        };
        r.header = dist(&header_parts, [st.default.0, st.even.and_then(|e| e.0), st.first.and_then(|f| f.0)]);
        r.footer = dist(&footer_parts, [st.default.1, st.even.and_then(|e| e.1), st.first.and_then(|f| f.1)]);
        sets.push(r);
    }
    let stretch_of = |pi: usize| inp.stretches.iter().rposition(|s| s.start <= pi).unwrap_or(0);
    // The stretch's headers and footers on a section; the first section of a stretch also
    // starts its page count and has its first page's own.
    let apply_running = |s: &mut SectionProps, si: usize, first: bool| {
        let Some(r) = sets.get(si) else { return };
        s.headers = r.headers;
        s.footers = r.footers;
        s.header = r.header;
        s.footer = r.footer;
        if first {
            s.title_page = r.title_page;
            s.page_num_start = r.restart.map(|n| n.clamp(0, i64::from(u32::MAX)) as u32);
        } else {
            s.headers.first = None;
            s.footers.first = None;
        }
    };
    let mut cur_section: Option<(SectionProps, usize, usize)> = None;
    let mut list_i = 0usize;
    for (pi, regions) in inp.regions.iter().enumerate() {
        for region in regions {
            let mut s = section_of_page.get(pi).cloned().unwrap_or_default();
            let si = stretch_of(pi);
            apply_running(&mut s, si, cur_section.as_ref().is_none_or(|c| c.2 != si));
            if region.cols.len() == 2 {
                let (a, c) = (region.cols.first().copied().unwrap_or_default(), region.cols.get(1).copied().unwrap_or_default());
                s.columns = Columns { count: 2, space: snap((c.0 - a.1).max(0.0)), separator: false, widths: Vec::new() };
                if ((a.1 - a.0) - (c.1 - c.0)).abs() > 3.0 {
                    s.columns.widths = vec![(a.1 - a.0, c.0 - a.1), (c.1 - c.0, 0.0)];
                }
            }
            // A new section where the headers, the page size or the columns change.
            let changed = cur_section.as_ref().is_some_and(|(cs, _, csi)| {
                *csi != si
                    || cs.page_w != s.page_w
                    || cs.page_h != s.page_h
                    || cs.columns != s.columns
                    || cs.margin_left != s.margin_left
                    || cs.margin_right != s.margin_right
                    || cs.margin_top != s.margin_top
                    || cs.margin_bottom != s.margin_bottom
            });
            if changed && let Some((done, last_page, _)) = cur_section.take() {
                // Mid-page (columns change) the next section carries straight on.
                s.start = if last_page == pi { SectionStart::Continuous } else { SectionStart::NextPage };
                close_section(&mut body, done);
            }
            match &mut cur_section {
                Some((_, last_page, _)) => *last_page = pi,
                None => cur_section = Some((s.clone(), pi, si)),
            }
            for it in &region.items {
                match it {
                    Item::Para(p) => {
                        let lr = lists.get(list_i).copied().flatten();
                        list_i += 1;
                        let para = b.paragraph(p, lr, false);
                        body.push(para_block(para));
                    }
                    Item::Table(t) => {
                        let ml = s.margin_left;
                        let tb = b.table(t, ml);
                        body.push(Arc::new(Block::Table(tb)));
                    }
                    Item::ColumnBreak => {
                        // The text after it starts the next column: a column break at the end of the
                        // paragraph before.
                        if let Some(last) = body.last_mut()
                            && let Block::Para(pp) = Arc::make_mut(last)
                        {
                            pp.text.push(wordcraft_doc::para::COLUMN_BREAK);
                            match pp.runs.last_mut() {
                                Some(r) => r.len += wordcraft_doc::para::COLUMN_BREAK.len_utf8(),
                                None => pp.runs.push(Run { len: wordcraft_doc::para::COLUMN_BREAK.len_utf8(), props: CharProps::default() }),
                            }
                            pp.touch();
                        }
                    }
                }
            }
        }
    }
    let mut doc = b.doc;
    if let Some((s, _, _)) = cur_section {
        doc.last_section = s;
    } else if let Some(mut s) = section_of_page.first().cloned() {
        apply_running(&mut s, 0, true);
        doc.last_section = s;
    }
    doc.settings.even_odd_headers = inp.even_odd;
    doc.body = body;
    doc.core.title = inp.title.clone().unwrap_or_default();
    doc.core.creator = inp.author.clone().unwrap_or_default();
    doc.core.subject = inp.subject.clone().unwrap_or_default();
    doc.core.keywords = inp.keywords.clone().unwrap_or_default();
    doc.ensure_nonempty();
    doc
}

/// End a section at the last block: its paragraph carries the section's properties.
fn close_section(body: &mut Blocks, s: SectionProps) {
    let needs_para = !matches!(body.last().map(|b| &**b), Some(Block::Para(p)) if p.section.is_none());
    if needs_para {
        body.push(para_block(Paragraph::new()));
    }
    if let Some(last) = body.last_mut()
        && let Block::Para(p) = Arc::make_mut(last)
    {
        p.section = Some(Box::new(s));
        p.touch();
    }
}

/// A stretch's headers and footers, as its sections carry them.
#[derive(Default)]
struct StretchParts {
    headers: HeaderSet,
    footers: HeaderSet,
    /// Distances from the page edges.
    header: f32,
    footer: f32,
    title_page: bool,
    restart: Option<i64>,
}

/// The part showing `look`, or (where the document has running text but these pages show
/// none) an empty one: a section without its own would show the one before's.
fn part_or_empty(b: &mut Builder<'_>, parts: &[Option<(u32, f32)>], look: Option<usize>, empty: &mut Option<u32>, header: bool) -> Option<u32> {
    if parts.iter().all(Option::is_none) {
        return None;
    }
    if let Some((id, _)) = look.and_then(|l| parts.get(l).copied().flatten()) {
        return Some(id);
    }
    if let Some(id) = *empty {
        return Some(id);
    }
    let mut para = Paragraph::new();
    para.props.style = Some(if header { "Header" } else { "Footer" }.to_string());
    let id = b.doc.add_part(if header { PartKind::Header } else { PartKind::Footer }, vec![para_block(para)]);
    *empty = Some(id);
    Some(id)
}

/// The header or footer part for one look of running text: (part id, distance from the page
/// edge).
fn running_part(b: &mut Builder<'_>, run: &Running, header: bool) -> Option<(u32, f32)> {
    let (w, h, m) = b.inp.pages.get(run.page).copied()?;
    let col = (m.left, w - m.right);
    let mut lines = run.lines.clone();
    lines.sort_by(|a, c| a.base.total_cmp(&c.base));
    // Each running line with its fields (char offset, length, page number or count); the
    // lines side by side on one baseline joined by tabs, then grouped into paragraphs.
    let fonts = b.inp.fonts;
    let with_fields: Vec<(Line, Vec<(usize, usize, bool)>)> =
        run.lines.iter().enumerate().map(|(li, l)| (l.clone(), run.fields.iter().filter(|f| f.0 == li).map(|f| (f.1, f.2, f.3)).collect())).collect();
    let tabbed = join_running(with_fields);
    let joined = crate::reflow::paragraphs(tabbed.iter().map(|t| t.0.clone()).collect(), col, run.page, fonts, b.inp.stats);
    let mut blocks = Blocks::new();
    let mut pics = run.pictures.iter();
    let mut li_base = 0usize;
    let mut prev_base: Option<f32> = None;
    for mut p in joined {
        p.above_base = prev_base;
        prev_base = p.lines.last().map(|l| l.base);
        let mut para = b.paragraph(&p, None, true);
        para.props.style = Some(if header { "Header" } else { "Footer" }.to_string());
        clear_style_tabs(b, &mut para);
        // Fields: numbers that are the page number or count, by line.
        let mut fields: Vec<(usize, usize, bool)> = Vec::new();
        let mut char_pos = 0usize;
        for (k, l) in p.lines.iter().enumerate() {
            let src = tabbed.iter().position(|(x, _)| (x.base - l.base).abs() < 0.01 && (x.x0 - l.x0).abs() < 0.01).unwrap_or(li_base + k);
            for (off, len, page) in tabbed.get(src).map(|t| t.1.as_slice()).unwrap_or(&[]) {
                fields.push((char_pos + off, *len, *page));
            }
            char_pos += l.text().chars().count() + 1;
        }
        li_base += p.lines.len();
        if !fields.is_empty() {
            replace_fields(&mut para, &fields);
        }
        if blocks.is_empty() {
            for pic in pics.by_ref() {
                b.float_picture(&mut para, pic);
            }
            para.touch();
        }
        blocks.push(para_block(para));
    }
    if blocks.is_empty() {
        // Pictures only: an empty paragraph to anchor them.
        let mut para = Paragraph::new();
        para.props.style = Some(if header { "Header" } else { "Footer" }.to_string());
        for pic in pics.by_ref() {
            b.float_picture(&mut para, pic);
        }
        para.touch();
        blocks.push(para_block(para));
    }
    if blocks.is_empty() {
        return None;
    }
    let dist = match (lines.first(), lines.last()) {
        (Some(first), Some(last)) => running_distance(first, last, header, h, fonts),
        _ => 36.0,
    };
    let id = b.doc.add_part(if header { PartKind::Header } else { PartKind::Footer }, blocks);
    Some((id, (dist * 2.0).round() / 2.0))
}

/// The Header and Footer styles carry Word's center and right stops; a running paragraph with
/// stops of its own clears them, or its text would stop at theirs first ("BG 1BG").
fn clear_style_tabs(b: &Builder<'_>, para: &mut Paragraph) {
    let (Some(own), Some(style)) = (para.props.tabs.as_mut(), para.props.style.as_deref()) else { return };
    let (inherited, _) = b.doc.styles.para_style_props(style);
    for t in inherited.tabs.unwrap_or_default() {
        if t.align != TabAlign::Clear && !own.iter().any(|o| (o.pos - t.pos).abs() < 0.5) {
            own.push(TabStop { pos: t.pos, align: TabAlign::Clear, leader: TabLeader::None });
        }
    }
    own.sort_by(|a, c| a.pos.total_cmp(&c.pos));
}

/// How far running text sits from the page's top (header) or bottom (footer) edge.
fn running_distance(first: &Line, last: &Line, header: bool, h: f32, fonts: &FontTable) -> f32 {
    if header {
        let (nat, desc) = natural(first, fonts);
        (first.base - (nat - desc)).max(0.0)
    } else {
        let (_, desc) = natural(last, fonts);
        (h - (last.base + desc)).max(0.0)
    }
}

fn join_running(lines: Vec<(Line, Vec<(usize, usize, bool)>)>) -> Vec<(Line, Vec<(usize, usize, bool)>)> {
    // Running lines are often tabbed (left text, centred title, page number at the right).
    let mut out: Vec<(Line, Vec<(usize, usize, bool)>)> = Vec::new();
    let mut sorted = lines;
    sorted.sort_by(|a, b| a.0.base.total_cmp(&b.0.base).then(a.0.x0.total_cmp(&b.0.x0)));
    for (l, fields) in sorted {
        if let Some((last, last_fields)) = out.last_mut()
            && (last.base - l.base).abs() < 0.3 * l.size
            && l.x0 > last.x1
        {
            // Its fields move along by the text before them and the tab.
            let shift = last.text().chars().count() + 1;
            last_fields.extend(fields.into_iter().map(|(off, len, page)| (off + shift, len, page)));
            let mut tab = l.spans.first().cloned().unwrap_or_else(|| {
                last.spans.last().cloned().unwrap_or(Span {
                    text: String::new(),
                    font: l.font,
                    size: l.size,
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
                })
            });
            tab.text = "\t".into();
            tab.note = None;
            tab.back = None;
            tab.underline = false;
            tab.link = None;
            last.spans.push(tab);
            last.spans.extend(l.spans);
            last.pieces.push((l.x0, l.x1));
            last.x1 = l.x1;
            continue;
        }
        out.push((l, fields));
    }
    out
}

/// Replace character ranges of `p`'s text with page number fields.
fn replace_fields(p: &mut Paragraph, fields: &[(usize, usize, bool)]) {
    let chars: Vec<char> = p.text.chars().collect();
    // Run props per char.
    let mut props: Vec<CharProps> = Vec::with_capacity(chars.len());
    let mut pos = 0usize;
    for r in &p.runs {
        let seg = p.text.get(pos..pos + r.len).unwrap_or("");
        for _ in seg.chars() {
            props.push(r.props.clone());
        }
        pos += r.len;
    }
    let objects = std::mem::take(&mut p.objects);
    let mut obj_iter = objects.into_iter();
    let mut out = Paragraph::new();
    out.props = p.props.clone();
    out.mark = p.mark.clone();
    out.section = p.section.take();
    let mut i = 0usize;
    while i < chars.len() {
        let cp = props.get(i).cloned().unwrap_or_default();
        if let Some((_, len, page)) = fields.iter().find(|(off, _, _)| *off == i) {
            push_obj(&mut out, field(*page), &cp);
            i += (*len).max(1);
            continue;
        }
        let c = chars.get(i).copied().unwrap_or(' ');
        if c == OBJ {
            if let Some(o) = obj_iter.next() {
                push_obj(&mut out, o, &cp);
            }
        } else {
            push_text(&mut out, &c.to_string(), &cp);
        }
        i += 1;
    }
    out.normalize();
    out.touch();
    *p = out;
}

/// The list each body paragraph belongs to (in [`body_paras`] order).
fn lists(b: &mut Builder<'_>) -> Vec<Option<ListRef>> {
    let regions = &b.inp.regions;
    let paras = body_paras(regions);
    let labels: Vec<Option<crate::reflow::Label>> = paras.iter().map(|p| p.lines.first().and_then(label)).collect();
    let mut out: Vec<Option<ListRef>> = vec![None; paras.len()];
    // Open runs of numbered items: (marker x, format, template, last value, members).
    struct NumRun {
        x: f32,
        format: NumFormat,
        template: String,
        last: u32,
        members: Vec<usize>,
        start: u32,
    }
    let mut open: Vec<NumRun> = Vec::new();
    let mut done: Vec<NumRun> = Vec::new();
    for (i, (p, lb)) in paras.iter().zip(labels.iter()).enumerate() {
        let Some(first) = p.lines.first() else { continue };
        let x = first.x0;
        // A paragraph further left than a list closes it.
        let mut k = 0;
        while k < open.len() {
            if open.get(k).is_some_and(|r| x < r.x - 2.0) {
                done.push(open.remove(k));
            } else {
                k += 1;
            }
        }
        let Some(lb) = lb else { continue };
        if lb.bullet {
            continue;
        }
        let prev_fmt = open.iter().find(|r| (r.x - x).abs() <= 2.0).map(|r| r.format);
        let Some((format, template, value)) = parse_enum(&lb.text, prev_fmt) else { continue };
        if let Some(pos) = open.iter().position(|r| (r.x - x).abs() <= 2.0 && r.format == format && r.template == template && r.last + 1 == value) {
            if let Some(r) = open.get_mut(pos) {
                r.last = value;
                r.members.push(i);
            }
            // Deeper runs end when their parent counts on.
            let mut k = 0;
            while k < open.len() {
                if open.get(k).is_some_and(|r| r.x > x + 2.0) {
                    done.push(open.remove(k));
                } else {
                    k += 1;
                }
            }
        } else {
            // Another run at this position ends.
            let mut k = 0;
            while k < open.len() {
                if open.get(k).is_some_and(|r| (r.x - x).abs() <= 2.0) {
                    done.push(open.remove(k));
                } else {
                    k += 1;
                }
            }
            open.push(NumRun { x, format, template, last: value, members: vec![i], start: value });
        }
    }
    done.append(&mut open);
    for r in done {
        if r.members.len() < 2 {
            continue;
        }
        let Some(first) = r.members.first().and_then(|i| paras.get(*i)) else { continue };
        let Some(line) = first.lines.first() else { continue };
        let Some(lb) = label(line) else { continue };
        let indent = snap(lb.text_x - first.col.0);
        let hanging = snap(lb.text_x - line.x0);
        let level = Level { start: r.start, format: r.format, text: r.template.clone(), indent, hanging, tab: Some(indent), ..Default::default() };
        let num = b.new_num(level);
        for m in r.members {
            if let Some(slot) = out.get_mut(m) {
                *slot = Some(ListRef { num, strip: true });
            }
        }
    }
    // Bullets: consecutive items with one bullet at one position share a list.
    let mut bullet_lists: HashMap<(String, i32, usize), u32> = HashMap::new();
    for (i, (p, lb)) in paras.iter().zip(labels.iter()).enumerate() {
        let (Some(lb), Some(line)) = (lb, p.lines.first()) else { continue };
        if !lb.bullet {
            continue;
        }
        let key = (lb.text.clone(), (line.x0 / 2.0).round() as i32, p.page / 1000);
        let num = match bullet_lists.get(&key) {
            Some(n) => *n,
            None => {
                let indent = snap(lb.text_x - p.col.0);
                let hanging = snap(lb.text_x - line.x0);
                let mut level = Level { format: NumFormat::Bullet, text: lb.text.clone(), indent, hanging, tab: Some(indent), ..Default::default() };
                if let Some(f) = line.spans.first().and_then(|s| b.inp.fonts.get(s.font))
                    && !f.symbol
                    && f.family != b.normal.family
                {
                    level.chr.font = Some(f.family.clone());
                }
                let n = b.new_num(level);
                bullet_lists.insert(key, n);
                n
            }
        };
        if let Some(slot) = out.get_mut(i) {
            *slot = Some(ListRef { num, strip: true });
        }
    }
    out
}
