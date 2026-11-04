//! PDF Reflow round trips: documents exported to PDF come back through File › Open as the
//! same paragraphs, lists, footnotes, headers and footers (`wordcraft-pdf-import`).

use wordcraft_doc::StoryRef;
use wordcraft_doc::props::LineSpacing;
use wordcraft_doc::{Align, Block, CharProps, Document, ParaProps, Paragraph, para_block};

/// Open `pdf` as File › Open does.
fn import(pdf: &[u8]) -> Result<Document, String> {
    crate::io::open_bytes("reflow.pdf", pdf)
}

fn pdf_of(doc: &Document) -> Vec<u8> {
    wordcraft_pdf::export(doc, &wordcraft_pdf::PdfOptions::default()).unwrap()
}

fn paras(doc: &Document) -> Vec<&Paragraph> {
    doc.body.iter().filter_map(|b| if let Block::Para(p) = &**b { Some(p) } else { None }).filter(|p| !p.text.trim().is_empty()).collect()
}

fn para(text: &str, props: ParaProps) -> std::sync::Arc<Block> {
    let mut p = Paragraph::with_text(text, CharProps::default());
    p.props = props;
    para_block(p)
}

const LOREM: &str = "The quick brown fox jumps over the lazy dog while the committee reviews the \
    minutes of the last meeting, which ran long because nobody could agree on the colour of the \
    new bicycle shed, and the treasurer asked again whether anyone had kept the receipts.";

#[test]
fn junk_is_an_error_not_a_crash() {
    let pdf = pdf_of(&Document::from_text("Hello world"));
    for cut in [10, pdf.len() / 3, pdf.len() / 2, pdf.len() - 20] {
        let _ = import(pdf.get(..cut).unwrap_or(&pdf));
    }
}

#[test]
fn a_pdf_under_another_name_opens_by_its_header() {
    let pdf = pdf_of(&Document::from_text("Opened by content"));
    let doc = crate::io::open_bytes("download.bin", &pdf).unwrap();
    assert!(doc.plain_text(StoryRef::Body).contains("Opened by content"));
}

#[test]
fn paragraphs_come_back_whole() {
    let mut doc = Document::new();
    doc.body = vec![
        para(LOREM, ParaProps::default()),
        para("A short line.", ParaProps::default()),
        para(LOREM, ParaProps { align: Some(Align::Justify), ..Default::default() }),
        para("Centred", ParaProps { align: Some(Align::Center), ..Default::default() }),
    ];
    let back = import(&pdf_of(&doc)).unwrap();
    let ps = paras(&back);
    assert_eq!(ps.len(), 4, "{:?}", ps.iter().map(|p| &p.text).collect::<Vec<_>>());
    assert_eq!(ps[0].text, LOREM.split_whitespace().collect::<Vec<_>>().join(" "));
    assert_eq!(ps[1].text, "A short line.");
    assert_eq!(ps[3].text, "Centred");
    assert_eq!(ps[3].props.align, Some(Align::Center));
    let normal = back.styles.styles.iter().find(|s| s.id == "Normal").unwrap();
    // Justified text was the commonest body alignment, or Justify sits on the paragraph.
    assert!(ps[2].props.align == Some(Align::Justify) || normal.para.align == Some(Align::Justify));
}

#[test]
fn line_spacing_and_indents_survive() {
    let mut doc = Document::new();
    let props = ParaProps { line_spacing: Some(LineSpacing::Multiple(1.5)), indent_left: Some(36.0), space_after: Some(12.0), ..Default::default() };
    // One paragraph at the margin: were all indented, the indent would read as the margin.
    doc.body = vec![para(LOREM, ParaProps::default()), para(LOREM, props.clone()), para(LOREM, props.clone()), para(LOREM, props)];
    let back = import(&pdf_of(&doc)).unwrap();
    let ps = paras(&back);
    assert_eq!(ps.len(), 4);
    let normal = back.styles.styles.iter().find(|s| s.id == "Normal").unwrap();
    let line = ps[1].props.line_spacing.or(normal.para.line_spacing);
    assert_eq!(line, Some(LineSpacing::Multiple(1.5)));
    assert_eq!(ps[0].props.indent_left, None);
    assert_eq!(ps[2].props.indent_left, Some(36.0));
    let gap = ps[2].props.space_before.unwrap_or(0.0) + ps[1].props.space_after.unwrap_or(0.0);
    assert!((gap - 12.0).abs() < 1.0, "gap {gap}");
}

#[test]
fn many_pages_are_one_flow() {
    let mut doc = Document::new();
    // Paragraphs of different lengths, as text has: page ends fall at different words.
    let words: Vec<&str> = LOREM.split_whitespace().collect();
    doc.body = (0..40)
        .map(|i| {
            let text = words.iter().cycle().skip(i * 7).take(25 + (i * 13) % 40).copied().collect::<Vec<_>>().join(" ");
            para(&format!("Item {i}: {text}."), ParaProps { space_after: Some(8.0), ..Default::default() })
        })
        .collect();
    let pdf = pdf_of(&doc);
    let back = import(&pdf).unwrap();
    let ps = paras(&back);
    // Paragraphs split by a page end are whole again. (A paragraph whose last line happens to
    // fill the line, at the foot of a page, can't be told from one carrying on: at most one.)
    let starts: Vec<&str> = ps.iter().map(|p| p.text.get(..12).unwrap_or(&p.text)).collect();
    assert!(ps.len() >= 39 && ps.len() <= 40, "{starts:?}");
    assert!(ps.iter().all(|p| p.text.starts_with("Item ")), "{starts:?}");
    assert!(!ps.iter().any(|p| p.props.page_break_before == Some(true)));
    let text = back.plain_text(StoryRef::Body);
    assert!(text.contains("Item 39:"));
}

#[test]
fn footnotes_come_back_as_footnotes() {
    use wordcraft_doc::para::{InlineObject, NoteKind};
    use wordcraft_doc::{PartKind, StoryRef};
    let mut doc = Document::new();
    let mut body = Vec::new();
    for i in 0..3 {
        let mut note = Paragraph::new().styled("FootnoteText");
        let id = doc.add_part(PartKind::Footnote, Vec::new());
        let reference = CharProps { style: Some("FootnoteReference".into()), ..Default::default() };
        note.insert_object(0, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &reference).unwrap();
        let len = note.len();
        note.insert_text(len, &format!(" Note number {i} says where this comes from."), &CharProps::default()).unwrap();
        doc.set_story(StoryRef::Part(id), vec![para_block(note)]).unwrap();
        let mut p = Paragraph::with_text(LOREM, CharProps::default());
        let at = p.text.find("committee").unwrap();
        p.insert_object(at, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &reference).unwrap();
        body.push(para_block(p));
    }
    doc.body = body;
    let back = import(&pdf_of(&doc)).unwrap();
    let notes: Vec<u32> = back.parts.iter().filter(|(_, p)| p.kind == PartKind::Footnote).map(|(id, _)| *id).collect();
    assert_eq!(notes.len(), 3);
    let note_text = back.plain_text(StoryRef::Part(notes[1]));
    assert!(note_text.contains("Note number 1 says where this comes from."), "{note_text:?}");
    let refs = paras(&back).iter().map(|p| p.objects.iter().filter(|o| matches!(o, InlineObject::NoteRef { .. })).count()).sum::<usize>();
    assert_eq!(refs, 3);
    // The note text is not in the body any more.
    assert!(!back.plain_text(StoryRef::Body).contains("Note number"));
}

#[test]
fn headers_change_with_the_sections() {
    use wordcraft_doc::para::InlineObject;
    use wordcraft_doc::section::SectionStart;
    use wordcraft_doc::{PartKind, SectionProps};
    let mut doc = Document::new();
    let part = |doc: &mut Document, kind: PartKind, text: &str, page: bool| {
        let mut p = Paragraph::with_text(text, CharProps::default());
        if page {
            let len = p.len();
            let field =
                InlineObject::Field { instr: " PAGE ".into(), result: "1".into(), locked: false, dirty: false, code: false, form: String::new() };
            p.insert_object(len, field, &CharProps::default()).unwrap();
        }
        doc.add_part(kind, vec![para_block(p)])
    };
    let order = part(&mut doc, PartKind::Header, "ORDER FORM", false);
    let terms = part(&mut doc, PartKind::Header, "STANDARD TERMS", false);
    let foot = part(&mut doc, PartKind::Footer, "Pilot Agreement (Version 1.1) page ", true);
    let words: Vec<&str> = LOREM.split_whitespace().collect();
    let text =
        |i: usize| format!("Clause {i}: {}.", words.iter().cycle().skip(i * 5).take(30 + (i * 11) % 30).copied().collect::<Vec<_>>().join(" "));
    let mut body: Vec<std::sync::Arc<Block>> = (0..30).map(|i| para(&text(i), ParaProps { space_after: Some(8.0), ..Default::default() })).collect();
    let mut first = doc.last_section.clone();
    first.headers.default = Some(order);
    first.footers.default = Some(foot);
    let mut end = Paragraph::with_text(&text(30), CharProps::default());
    end.section = Some(Box::new(first));
    body.push(para_block(end));
    body.extend((31..70).map(|i| para(&text(i), ParaProps { space_after: Some(8.0), ..Default::default() })));
    doc.body = body;
    doc.last_section = SectionProps { start: SectionStart::NextPage, ..doc.last_section.clone() };
    doc.last_section.headers.default = Some(terms);
    doc.last_section.footers.default = Some(foot);
    let back = import(&pdf_of(&doc)).unwrap();
    let header_text = |s: &SectionProps| s.headers.default.map(|id| back.plain_text(StoryRef::Part(id))).unwrap_or_default();
    let sections = back.sections();
    let heads: Vec<String> = sections.iter().map(|(_, s)| header_text(s).trim().to_string()).collect();
    assert_eq!(heads.first().map(String::as_str), Some("ORDER FORM"), "{heads:?}");
    assert_eq!(heads.last().map(String::as_str), Some("STANDARD TERMS"), "{heads:?}");
    assert!(sections.iter().skip(1).all(|(_, s)| s.start == SectionStart::NextPage));
    // The footer: the version as text, the page number a field.
    let foot_id = sections.first().and_then(|(_, s)| s.footers.default).unwrap();
    assert!(back.plain_text(StoryRef::Part(foot_id)).contains("(Version 1.1) page"));
    let fields: Vec<String> = back
        .story(StoryRef::Part(foot_id))
        .unwrap()
        .iter()
        .filter_map(|b| if let Block::Para(p) = &**b { Some(p) } else { None })
        .flat_map(|p| p.objects.iter())
        .filter_map(|o| if let InlineObject::Field { instr, .. } = o { Some(instr.trim().to_string()) } else { None })
        .collect();
    assert_eq!(fields, vec!["PAGE".to_string()]);
    let text = back.plain_text(StoryRef::Body);
    assert!(!text.contains("ORDER FORM") && !text.contains("STANDARD TERMS"));
    // The clause the first section ends with is the last before the break.
    assert!(text.contains("Clause 30:") && text.contains("Clause 69:"));
}

#[test]
fn a_footer_keeps_its_own_tab_stops() {
    use wordcraft_doc::PartKind;
    use wordcraft_doc::para::InlineObject;
    use wordcraft_doc::props::{TabAlign, TabLeader, TabStop};
    // "EU <page> EU" with the number at 150 pt, not at the Footer style's center stop (234 pt).
    let mut doc = Document::new();
    let mut p = Paragraph::with_text("EU\t", CharProps::default());
    let field = InlineObject::Field { instr: " PAGE ".into(), result: "1".into(), locked: false, dirty: false, code: false, form: String::new() };
    p.insert_object(p.len(), field, &CharProps::default()).unwrap();
    p.insert_text(p.len(), "\tEU", &CharProps::default()).unwrap();
    let right = doc.last_section.page_w - doc.last_section.margin_left - doc.last_section.margin_right;
    p.props.tabs = Some(vec![
        TabStop { pos: 150.0, align: TabAlign::Left, leader: TabLeader::None },
        TabStop { pos: right, align: TabAlign::Right, leader: TabLeader::None },
    ]);
    let foot = doc.add_part(PartKind::Footer, vec![para_block(p)]);
    doc.last_section.footers.default = Some(foot);
    doc.body = (0..3).map(|i| para(&format!("Page {i}. {LOREM}"), ParaProps { page_break_before: Some(i > 0), ..Default::default() })).collect();
    let back = import(&pdf_of(&doc)).unwrap();
    let foot_id = back.sections().first().and_then(|(_, s)| s.footers.default).unwrap();
    let story = back.story(StoryRef::Part(foot_id)).unwrap();
    let p = story.iter().find_map(|b| if let Block::Para(p) = &**b { Some(p) } else { None }).unwrap();
    let stops: Vec<f32> = back.styles.resolve_para(&p.props).tabs.iter().map(|t| t.pos).collect();
    // Its own two stops (the number's and the last word's), not the Footer style's center and
    // right stops as well: the center one (234 pt) would catch the last word first.
    assert_eq!(stops.len(), 2, "{stops:?}");
    assert!((stops[0] - 150.0).abs() < 3.0 && stops[1] > 400.0, "{stops:?}");
}
