//! A page's text with where it stands ([`crate::text::page_text`]), for an app that lays its own
//! marks over a drawn page: Bernio Chat's page view read it from the display list's glyphs, on
//! copies of each page without its header and footer.

use crate::text::page_text;
use crate::*;
use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::props::NumRef;
use wordcraft_doc::{CharProps, PartKind, Pos, Table, para_block};

fn lay(doc: &Document) -> DocLayout {
    let mut c = LayoutCache::new();
    layout(doc, &mut c, &LayoutOptions::default())
}

/// Each character's left edge is where the caret goes before it, and a change of style starts a
/// run of its own.
#[test]
fn page_text_puts_each_character_where_the_caret_goes() {
    let text = "Either party may terminate this Agreement by written notice.";
    let mut d = Document::from_text(text);
    let (a, b) = (text.find("written").unwrap(), text.find('.').unwrap());
    d.format_range(&Pos::body(0, a), &Pos::body(0, b), &|c| c.bold = Some(true)).unwrap();
    let l = lay(&d);
    let runs = page_text(&d, &l.pages[0]);
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["Either party may terminate this Agreement by ", "written notice", "."]);
    assert_eq!((runs[1].start, runs[1].end), (a, b));
    for r in &runs {
        assert_eq!((r.story, r.label, r.size), (StoryRef::Body, false, runs[0].size));
        assert_eq!(r.chars.len(), r.text.chars().count());
        assert!(r.chars.windows(2).all(|w| w[0].1 <= w[1].0 + 0.01 && w[0].0 <= w[0].1), "{r:?}");
        for (i, (off, _)) in r.text.char_indices().enumerate() {
            let c = l.caret(&Pos::body(0, r.start + off)).unwrap();
            assert!((c.x - r.chars[i].0).abs() < 0.01, "{:?} at {}: caret {} edge {}", r.text, off, c.x, r.chars[i].0);
            assert!(r.baseline > c.top && r.baseline < c.top + c.height, "the baseline is in the caret's line");
        }
    }
    assert!((runs[0].baseline - runs[2].baseline).abs() < 0.01, "one line");
    let end = l.caret(&Pos::body(0, text.len())).unwrap();
    assert!((runs[2].chars[0].1 - end.x).abs() < 0.01, "the last character ends where the paragraph does");
}

/// The header comes first, then the body with its list label, its table cells and the note
/// number (raised and smaller), then the note itself; each run names its story and paragraph.
#[test]
fn page_text_reads_labels_cells_notes_and_headers() {
    let mut d = Document::from_text("Definitions\nPrice");
    let num = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    if let Some(Block::Para(p)) = d.body.first_mut().map(Arc::make_mut) {
        p.props.numbering = Some(NumRef { num, level: 0 });
    }
    let para = |t: &str| vec![para_block(Paragraph::with_text(t, CharProps::default()))];
    let note = d.add_part(PartKind::Footnote, para("Net of tax."));
    let note_ref = InlineObject::NoteRef { kind: NoteKind::Footnote, id: note, custom: String::new() };
    d.insert_object(&Pos::body(1, 5), note_ref, &CharProps::default()).unwrap();
    let head = d.add_part(PartKind::Header, para("Supply Agreement"));
    d.last_section.headers.default = Some(head);
    let mut t = Table::new(1, 2, 300.0);
    t.rows[0].cells[0].blocks = para("Steel");
    t.rows[0].cells[1].blocks = para("640 EUR");
    d.body.push(Arc::new(Block::Table(t)));
    let l = lay(&d);
    let runs = page_text(&d, &l.pages[0]);
    let find = |t: &str| runs.iter().find(|r| r.text == t).unwrap_or_else(|| panic!("{t:?} in {runs:#?}"));

    assert_eq!(runs[0].story, StoryRef::Part(head));
    let label = runs.iter().find(|r| r.label).unwrap();
    assert_eq!((label.text.as_str(), label.start, label.end, label.story), ("1.", 0, 0, StoryRef::Body));
    assert!(label.chars.last().unwrap().1 <= find("Definitions").chars[0].0, "the label stands before the text");
    let (price, mark) = (find("Price"), runs.iter().find(|r| r.story == StoryRef::Body && r.text == "1").unwrap());
    assert!(mark.baseline < price.baseline && mark.size < price.size, "the note number is superscript");
    assert!(mark.chars[0].0 >= price.chars[4].1 - 0.01);
    let steel = find("Steel");
    assert_eq!(steel.path.0.len(), 4, "a cell's paragraph: block, row, cell, block");
    assert!(find("640 EUR").chars[0].0 > steel.chars[4].1);
    let net = runs.iter().find(|r| r.story == StoryRef::Part(note)).unwrap();
    assert_eq!(net.text, "Net of tax.");
    assert!(net.baseline > steel.baseline, "the note is at the foot of the page");
    assert_eq!(runs.last().map(|r| r.story), Some(StoryRef::Part(note)));
}
