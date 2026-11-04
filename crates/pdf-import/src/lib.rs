//! PDF Reflow: opens a PDF as an editable document.
//!
//! A PDF only says where each glyph goes. This crate reads the pages (craft-pdf's interpreter),
//! groups glyphs into lines, finds what repeats as headers and footers, the body's margins,
//! columns, ruled tables, pictures and lists, and rebuilds the document Word would have laid out
//! that way: flowing paragraphs with styles, spacing, indents and tab stops, not one text box
//! per line. Exports from Word-like processors come back close to their source; other PDFs come
//! back as clean text in the same look.
//!
//! What a page draws as vector art (charts, diagrams) is left out for now, and text set at an
//! angle is too.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod build;
mod extract;
mod fontinfo;
mod lines;
mod notes;
mod reflow;
mod running;
mod stitch;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use craft_pdf::interpret::InterpreterCache;
use wordcraft_doc::Document;

use crate::extract::{PageContent, PictureCache};
use crate::fontinfo::FontTable;
use crate::reflow::{Margins, PageLines};

/// Pages read at most; the rest of a longer PDF is left out (and said so in the log).
pub const MAX_PAGES: usize = 5000;

/// Open a PDF as a document.
pub fn import(bytes: &[u8]) -> Result<Document, String> {
    let pdf = craft_pdf::open_slice(bytes, None).map_err(|e| match e {
        craft_pdf::OpenError::Load(craft_pdf::syntax::LoadPdfError::Decryption(_)) => "The PDF is password protected".to_string(),
        other => format!("Not a readable PDF: {other}"),
    })?;
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err("The PDF has no pages".into());
    }
    if pages.len() > MAX_PAGES {
        log::warn!("PDF import: only the first {MAX_PAGES} of {} pages are read", pages.len());
    }
    // Read every page.
    let mut fonts = FontTable::default();
    let mut dict_fonts = HashMap::new();
    let mut pictures = PictureCache::new();
    let mut cache = InterpreterCache::new();
    let mut plines: Vec<PageLines> = Vec::new();
    let mut glyphs = 0usize;
    for page in pages.iter().take(MAX_PAGES) {
        extract::scan_fonts(page, &mut dict_fonts);
        let mut content = match extract::page(page, &cache, &mut fonts, &dict_fonts, &mut pictures) {
            Some(c) => c,
            None => {
                // The interpreter's caches may be half filled: start afresh for the next page.
                cache = InterpreterCache::new();
                let (w, h) = page.render_dimensions();
                PageContent { w, h, ..Default::default() }
            }
        };
        glyphs += content.glyphs.len();
        if content.rotated_glyphs > 0 {
            log::info!("PDF import: {} glyphs set at an angle are left out", content.rotated_glyphs);
        }
        let lines = lines::lines(&mut content, &fonts);
        plines.push(PageLines {
            w: content.w,
            h: content.h,
            lines,
            rules: content.rules,
            fills: content.fills,
            pictures: content.pictures,
            invisible_text: content.invisible_text,
        });
    }
    if glyphs == 0 {
        log::info!("PDF import: no text layer (a scan?): the pages come in as pictures");
    }
    // Running heads and feet, then the body's statistics and margins.
    let (header, footer) = running::running(&mut plines);
    let (stretches, even_odd) = running::sections(&header, &footer, plines.len());
    let notes = notes::extract(&mut plines);
    let stats = reflow::stats(&plines);
    let margins = page_margins(&plines, &fonts, &stats);
    let mut regions = Vec::with_capacity(plines.len());
    for (i, p) in plines.iter_mut().enumerate() {
        let m = margins.get(i).copied().unwrap_or(Margins { top: 72.0, bottom: 72.0, left: 72.0, right: 72.0 });
        regions.push(reflow::page(p, i, &m, &fonts, &stats));
    }
    let page_geom: Vec<(f32, f32, Margins)> = plines.iter().zip(&margins).map(|(p, m)| (p.w, p.h, *m)).collect();
    let starts: Vec<usize> = stretches.iter().map(|s| s.start).collect();
    stitch::stitch(&mut regions, &page_geom, &fonts, &starts);
    // Who made it: TeX hyphenates, and its hyphens at line ends are not part of the words.
    let meta = pdf.metadata();
    let made_by = [meta.producer.as_deref(), meta.creator.as_deref()].into_iter().flatten().map(text_string).collect::<Vec<_>>().join(" ");
    let dehyphenate = made_by.contains("TeX");
    let input = build::Input {
        fonts: &fonts,
        stats: &stats,
        pages: page_geom,
        regions,
        header,
        footer,
        stretches,
        even_odd,
        notes,
        dehyphenate,
        title: meta.title.as_deref().map(text_string).filter(|s| !s.trim().is_empty()),
        author: meta.author.as_deref().map(text_string).filter(|s| !s.trim().is_empty()),
        subject: meta.subject.as_deref().map(text_string).filter(|s| !s.trim().is_empty()),
        keywords: meta.keywords.as_deref().map(text_string).filter(|s| !s.trim().is_empty()),
    };
    Ok(build::document(&input))
}

/// Margins for each page, shared by the pages of one size.
fn page_margins(pages: &[PageLines], fonts: &FontTable, stats: &reflow::Stats) -> Vec<Margins> {
    let key = |p: &PageLines| ((p.w * 2.0).round() as i32, (p.h * 2.0).round() as i32);
    let mut by_size: HashMap<(i32, i32), Margins> = HashMap::new();
    pages
        .iter()
        .map(|p| {
            *by_size.entry(key(p)).or_insert_with(|| {
                let group: Vec<&PageLines> = pages.iter().filter(|q| key(q) == key(p)).collect();
                reflow::margins(&group, fonts, stats)
            })
        })
        .collect()
}

/// A PDF text string: UTF-16BE or UTF-8 behind a byte order mark, else PDFDocEncoding (read
/// as Latin-1, which it matches but for a few punctuation marks).
fn text_string(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter().map(|&c| char::from(c)).collect()
}
