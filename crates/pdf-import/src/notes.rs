//! Footnotes: the small text under a short separator rule at the foot of a page, each note
//! opening with its number, becomes a real footnote, and the raised number in the text that
//! refers to it becomes the note's reference mark.

use crate::lines::{Line, Span, Vert};
use crate::reflow::PageLines;

/// A footnote's text, as the page shows it (its first line starts with the number).
pub(crate) struct Note {
    pub lines: Vec<Line>,
    pub page: usize,
}

/// A note number: digits, or the symbols notes are marked with.
fn marker(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.chars().count() <= 4 && (t.chars().all(|c| c.is_ascii_digit()) || t.chars().all(|c| matches!(c, '*' | '†' | '‡' | '§' | '¶')))
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

/// `marker` followed by a tab, then `text`'s spans: one line for the note's first line.
fn with_marker(m: &Line, text: Line, id: usize) -> Line {
    let mut spans: Vec<Span> = m.spans.iter().cloned().map(|s| Span { note: Some(id), ..s }).collect();
    if let Some(tab) = text.spans.first().cloned() {
        spans.push(Span {
            text: "\t".into(),
            x0: m.x1,
            x1: text.x0,
            vert: Vert::Base,
            underline: false,
            strike: false,
            link: None,
            back: None,
            note: None,
            ..tab
        });
    }
    spans.extend(text.spans);
    let mut pieces = vec![(m.x0, m.x1)];
    pieces.extend(text.pieces);
    Line { x0: m.x0, spans, pieces, ..text }
}

/// Take each page's footnotes out of its lines; the references in the text are marked with
/// the note's index in the list returned.
pub(crate) fn extract(pages: &mut [PageLines]) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    for (pi, p) in pages.iter_mut().enumerate() {
        let Some(body_size) = median(p.lines.iter().map(|l| l.size).collect()) else { continue };
        let min_x0 = p.lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
        // The separator: a short thin rule at the left, in the lower part of the page, with
        // smaller text just under it.
        let sep = p
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.horizontal && r.width <= 1.5 && r.to - r.from >= 30.0 && r.to - r.from <= 0.5 * p.w && r.a > 0.3 * p.h)
            .filter(|(_, r)| r.from <= min_x0 + 40.0)
            .filter(|(_, r)| {
                let next = p.lines.iter().filter(|l| l.top() > r.a - 1.0).min_by(|a, b| a.base.total_cmp(&b.base));
                next.is_some_and(|l| l.size < body_size - 0.4 && l.top() - r.a < 3.0 * l.size)
            })
            .min_by(|a, b| a.1.a.total_cmp(&b.1.a))
            .map(|(i, r)| (i, r.a));
        let Some((ri, sep_y)) = sep else { continue };
        // The notes: the smaller lines under it, down to the first line in body size.
        let mut below: Vec<(usize, &Line)> = p.lines.iter().enumerate().filter(|(_, l)| l.top() > sep_y - 1.0).collect();
        below.sort_by(|a, b| a.1.base.total_cmp(&b.1.base).then(a.1.x0.total_cmp(&b.1.x0)));
        let end = below.iter().position(|(_, l)| l.size >= body_size - 0.4 && !marker(&l.text())).unwrap_or(below.len());
        let area: Vec<usize> = below.iter().take(end).map(|(i, _)| *i).collect();
        if area.is_empty() {
            continue;
        }
        let mut area_lines: Vec<Line> = area.iter().filter_map(|i| p.lines.get(*i).cloned()).collect();
        area_lines.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.x0.total_cmp(&b.x0)));
        // Notes on this page: (number, lines); lines before the first number carry on the last
        // note of the page before.
        let first_id = notes.len();
        let mut here: Vec<(String, Vec<Line>)> = Vec::new();
        let mut carry: Vec<Line> = Vec::new();
        let mut i = 0;
        while let Some(l) = area_lines.get(i) {
            let next = area_lines.get(i + 1);
            let id = first_id + here.len();
            // The number on a line of its own, raised beside the note's first line.
            if marker(&l.text())
                && let Some(n) = next
                && (n.base - l.base).abs() < 0.8 * n.size
                && n.x0 > l.x1
            {
                here.push((l.text().trim().to_string(), vec![with_marker(l, n.clone(), id)]));
                i += 2;
                continue;
            }
            // The number as the line's raised first span.
            if let Some(s) = l.spans.first()
                && s.vert == Vert::Sup
                && marker(&s.text)
            {
                let mut line = l.clone();
                if let Some(s0) = line.spans.first_mut() {
                    s0.note = Some(id);
                }
                here.push((s.text.trim().to_string(), vec![line]));
                i += 1;
                continue;
            }
            match here.last_mut() {
                Some((_, ls)) => ls.push(l.clone()),
                None => carry.push(l.clone()),
            }
            i += 1;
        }
        // The references: the first raised number in the text above for each note.
        let mut body: Vec<usize> = (0..p.lines.len()).filter(|i| !area.contains(i)).collect();
        body.sort_by(|a, b| {
            let (la, lb) = (p.lines.get(*a), p.lines.get(*b));
            la.map(|l| l.base)
                .unwrap_or(0.0)
                .total_cmp(&lb.map(|l| l.base).unwrap_or(0.0))
                .then(la.map(|l| l.x0).unwrap_or(0.0).total_cmp(&lb.map(|l| l.x0).unwrap_or(0.0)))
        });
        let mut found = vec![false; here.len()];
        for (k, (num, _)) in here.iter().enumerate() {
            let id = first_id + k;
            'search: for &li in &body {
                let Some(l) = p.lines.get_mut(li) else { continue };
                for s in &mut l.spans {
                    if s.vert == Vert::Sup && s.note.is_none() && s.text.trim() == num {
                        s.note = Some(id);
                        if let Some(f) = found.get_mut(k) {
                            *f = true;
                        }
                        break 'search;
                    }
                }
            }
        }
        // A note nothing refers to can't be a footnote: give back the page's notes then (keeping
        // the numbering of the others would mean guessing).
        if found.iter().any(|f| !f) {
            for l in &mut p.lines {
                for s in &mut l.spans {
                    if s.note.is_some_and(|n| n >= first_id) {
                        s.note = None;
                    }
                }
            }
            continue;
        }
        // Carried-on text joins the last note before this page (with none, these aren't notes).
        if !carry.is_empty() && notes.is_empty() {
            continue;
        }
        if !carry.is_empty()
            && let Some(last) = notes.last_mut()
        {
            last.lines.extend(carry);
        }
        for (_, lines) in here {
            notes.push(Note { lines, page: pi });
        }
        let mut k = 0;
        p.lines.retain(|_| {
            let keep = !area.contains(&k);
            k += 1;
            keep
        });
        let mut k = 0;
        p.rules.retain(|_| {
            let keep = k != ri;
            k += 1;
            keep
        });
    }
    notes
}
