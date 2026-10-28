//! The text of a laid-out page and where it stands: each upright line's text in runs of one
//! style, with its baseline, its size and the left and right edge of every character, in page
//! points from the top left. For an app that lays its own marks over a drawn page (highlights,
//! search hits, its own comments) or reads a page's text with its geometry: the display list
//! ([`crate::display`]) knows the glyphs drawn but not which text of which paragraph they are.

use wordcraft_doc::{Document, Path, StoryRef};

use crate::para::{ClKind, Line, ParaLayout};
use crate::{DECOR_STORY, Page, Placed};

/// A run of one line's text in one style, where the page draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    /// The story the text is in: the body (its tables too), or a header, footer, note or text box
    /// part.
    pub story: StoryRef,
    /// The paragraph.
    pub path: Path,
    /// The byte range of the paragraph's text the run covers (`start == end` for a list label).
    pub start: usize,
    pub end: usize,
    /// The text: the paragraph's own characters (its object markers left out); a field's result,
    /// a note's number and a list label as they are shown.
    pub text: String,
    /// Baseline, points from the page's top (raised or lowered for superscript and subscript).
    pub baseline: f32,
    /// Font size, points.
    pub size: f32,
    /// Left and right edge of each character of `text`, points from the page's left. The
    /// characters of a ligature or of a field result drawn as one share its width.
    pub chars: Vec<(f32, f32)>,
    /// The run is the paragraph's list number or bullet, which isn't in its text.
    pub label: bool,
}

/// The text on `page` of `doc`'s layout, line by line: the header, then the body with its tables,
/// notes and text boxes as they are placed, then the footer. Turned text (a table cell's text
/// direction) and text in a drawing canvas are left out, and a line's hyphen at a hyphenated
/// break; in a layout that shows tracked changes (markup), deleted text is on the page and here.
pub fn page_text(doc: &Document, page: &Page) -> Vec<TextRun> {
    let mut out = Vec::new();
    for it in page.header.iter().chain(&page.items).chain(&page.footer) {
        if let Placed::Lines { story, path, para, l0, l1, x, y, turn } = it
            && *story != DECOR_STORY
            && !turn.is_turned()
        {
            lines_text(doc, *story, path, para, (*l0, *l1), (*x, *y), &mut out);
        }
    }
    out.retain(|r| !r.text.is_empty());
    out
}

/// Lines `l0..l1` of paragraph `path` laid out as `pl`, placed at `x` with line `l0`'s top at `y`.
fn lines_text(doc: &Document, story: StoryRef, path: &Path, pl: &ParaLayout, (l0, l1): (usize, usize), (x, y): (f32, f32), out: &mut Vec<TextRun>) {
    let Some(first) = pl.lines.get(l0) else { return };
    let para = doc.para(story, path);
    for li in l0..l1 {
        let Some(line) = pl.lines.get(li) else { continue };
        let base = y + (line.baseline - first.top);
        if li == pl.label_line()
            && let Some(lab) = &pl.label
            && let Some(st) = pl.styles.get(lab.style as usize)
            && !lab.text.is_empty()
        {
            // The label's characters stand where its glyphs do when there is one a character;
            // else they share its width.
            let gx = x + lab.x;
            let n = lab.text.chars().count();
            let lefts: Vec<f32> = if lab.glyphs.len() == n {
                lab.glyphs.iter().map(|g| gx + g.dx).collect()
            } else {
                (0..n).map(|i| gx + lab.width * i as f32 / n as f32).collect()
            };
            let chars = lefts.iter().enumerate().map(|(i, l)| (*l, lefts.get(i + 1).copied().unwrap_or(gx + lab.width).max(*l))).collect();
            out.push(TextRun {
                story,
                path: path.clone(),
                start: line.start,
                end: line.start,
                text: lab.text.clone(),
                baseline: base - st.shift,
                size: st.size,
                chars,
                label: true,
            });
        }
        let mut run: Option<TextRun> = None;
        let mut style = u32::MAX;
        for k in line.c0..line.c1 {
            let Some(c) = pl.clusters.get(k) else { break };
            // Markers (bookmarks, comment anchors, hidden text) take no room: the run goes on.
            if c.kind == ClKind::Marker {
                continue;
            }
            let shown = pl.shown.binary_search_by_key(&k, |(i, _, _)| *i).ok().and_then(|i| pl.shown.get(i)).map(|(_, s, _)| s.as_str());
            let text = match (c.kind, shown) {
                (ClKind::Text | ClKind::Space, Some(s)) => s,
                // An object's character stands for nothing without text to show.
                (ClKind::Text | ClKind::Space | ClKind::Tab, None) => {
                    para.and_then(|p| p.text.get(c.start..c.end)).filter(|t| !t.contains(wordcraft_doc::para::OBJ)).unwrap_or("")
                }
                // Pictures, equations and breaks end the run.
                _ => {
                    out.extend(run.take());
                    continue;
                }
            };
            let Some(st) = pl.styles.get(c.style as usize) else { continue };
            if c.style != style {
                out.extend(run.take());
                style = c.style;
            }
            let r = run.get_or_insert_with(|| TextRun {
                story,
                path: path.clone(),
                start: c.start,
                end: c.start,
                text: String::new(),
                baseline: base - st.shift,
                size: st.size,
                chars: Vec::new(),
                label: false,
            });
            r.end = c.end;
            r.text.push_str(text);
            cluster_chars(line, k, (x, text.chars().count()), &mut r.chars);
        }
        out.extend(run);
    }
}

/// The edges of the `n` characters of cluster `k` of `line` (placed at `x`): they share its box,
/// in reading order (right to left in right-to-left text).
fn cluster_chars(line: &Line, k: usize, (x, n): (f32, usize), out: &mut Vec<(f32, f32)>) {
    let (Some(l), Some(r)) = (line.cl_left(k), line.cl_right(k)) else {
        out.extend(std::iter::repeat_n((x, x), n));
        return;
    };
    let (l, r) = (x + l, x + r.max(l));
    let w = (r - l) / n.max(1) as f32;
    for i in 0..n {
        let i = if line.cl_rtl(k) { n - 1 - i } else { i };
        out.push((l + w * i as f32, l + w * (i + 1) as f32));
    }
}
