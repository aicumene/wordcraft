//! Footnotes and endnotes for the text exports (plain text, Markdown): the mark each reference
//! shows and each note's text, in the order the references come in the body.

use std::collections::{HashMap, HashSet};

use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::section::NoteRestart;
use wordcraft_doc::{Document, PartKind};

use crate::model::{FBlock, note_blocks};

/// Most notes an export writes: a hostile file can't make one without end.
const MAX_NOTES: usize = 100_000;

/// A footnote or endnote as the text formats write it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// The note's story (`Document::parts`), as [`crate::model::Inline::NoteRef`] names it.
    pub id: u32,
    pub kind: NoteKind,
    /// The mark its reference shows: its number in its format (`1`, `iv`) or its custom mark.
    pub mark: String,
    /// Its text, without its own mark at the start.
    pub blocks: Vec<FBlock>,
}

/// The notes referenced in the body (text boxes included), in reading order, with their marks
/// (part id, kind, mark). Numbered as the page shows them: each kind on its own, from its
/// section's "start at", in its number format, again in each section that restarts. Without a
/// layout there are no pages, so notes numbered again on each page are numbered on. A reference
/// that is a tracked deletion has no number, as in the final text; a hidden one keeps its number,
/// as on the page, but neither it nor its note is written.
pub(crate) fn marks(doc: &Document) -> Vec<(u32, NoteKind, String)> {
    let sections = doc.sections();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // Per kind (footnotes, endnotes): the last number and the section it was in.
    let mut last: [Option<(u32, usize)>; 2] = [None, None];
    let mut si = 0usize;
    // The paragraph being read and where its objects are (looked up once per paragraph).
    let mut cur: Option<(usize, Vec<usize>)> = None;
    doc.objects_in_reading_order_by_block(&mut |block, p, k| {
        let Some(InlineObject::NoteRef { kind, id, custom }) = p.objects.get(k) else { return true };
        let is_note = doc.parts.get(id).is_some_and(|part| matches!(part.kind, PartKind::Footnote | PartKind::Endnote));
        if !is_note || out.len() >= MAX_NOTES || seen.contains(id) {
            return true;
        }
        let key = p as *const wordcraft_doc::Paragraph as usize;
        if cur.as_ref().is_none_or(|(at, _)| *at != key) {
            cur = Some((key, p.object_offsets()));
        }
        let Some(props) = cur.as_ref().and_then(|(_, offs)| offs.get(k)).map(|off| p.props_of_char(*off)) else { return true };
        if props.del.is_some() {
            return true;
        }
        while sections.get(si).is_some_and(|(end, _)| block > *end) && si + 1 < sections.len() {
            si += 1;
        }
        let Some((_, sect)) = sections.get(si) else { return true };
        let ki = usize::from(*kind == NoteKind::Endnote);
        let o = doc.note_options(sect, ki == 1);
        let Some(slot) = last.get_mut(ki) else { return true };
        let n = match *slot {
            Some((prev, psect)) if !(o.num_restart == NoteRestart::EachSect && psect != si) => prev.saturating_add(1),
            _ => o.num_start,
        };
        *slot = Some((n, si));
        seen.insert(*id);
        if !props.hidden.unwrap_or(false) {
            out.push((*id, *kind, if custom.is_empty() { o.num_fmt.format(n) } else { custom.clone() }));
        }
        true
    });
    out
}

/// The document's footnotes, then its endnotes, each kind in the order of its references (see
/// [`marks`]).
pub fn notes(doc: &Document) -> Vec<Note> {
    let marks = marks(doc);
    let by_id: HashMap<u32, String> = marks.iter().map(|(id, _, m)| (*id, m.clone())).collect();
    let mut out: Vec<Note> = Vec::new();
    for want in [NoteKind::Footnote, NoteKind::Endnote] {
        for (id, kind, mark) in marks.iter().filter(|(_, k, _)| *k == want) {
            let Some(part) = doc.parts.get(id) else { continue };
            out.push(Note { id: *id, kind: *kind, mark: mark.clone(), blocks: note_blocks(doc, &part.blocks, &by_id, *id) });
        }
    }
    out
}
