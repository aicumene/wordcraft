//! The importer's own checks; round trips through WordCraft's PDF export are in the engine's
//! tests (`crates/engine/src/tests/pdf_reflow.rs`), where the PDF writer may be used.

use crate::import;

#[test]
fn text_strings_decode() {
    assert_eq!(crate::text_string(b"\xFE\xFF\x00H\x00i"), "Hi");
    assert_eq!(crate::text_string(b"\xEF\xBB\xBFcaf\xC3\xA9"), "café");
    assert_eq!(crate::text_string(b"caf\xE9"), "café");
}

#[test]
fn junk_is_an_error_not_a_crash() {
    assert!(import(b"").is_err());
    let _ = import(b"%PDF-1.7\n");
    let catalog = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >> endobj\n4 0 obj << /Length 99999 >> stream\nBT /F1 12 Tf (x) Tj ET\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    for cut in [9, 40, 120, catalog.len()] {
        let _ = import(catalog.get(..cut).unwrap_or(catalog));
    }
}

/// `PDF_DUMP=file.pdf cargo test -p wordcraft-pdf-import dump -- --ignored --nocapture`: the
/// lines and rules the importer sees on each page.
#[test]
#[ignore]
fn dump() {
    let Ok(path) = std::env::var("PDF_DUMP") else { return };
    let bytes = std::fs::read(path).unwrap();
    let pdf = craft_pdf::open_slice(&bytes, None).unwrap();
    let mut fonts = crate::fontinfo::FontTable::default();
    let mut dict_fonts = std::collections::HashMap::new();
    let mut pictures = crate::extract::PictureCache::new();
    let cache = craft_pdf::interpret::InterpreterCache::new();
    let only: Option<usize> = std::env::var("PDF_PAGE").ok().and_then(|p| p.parse().ok());
    for (i, page) in pdf.pages().iter().enumerate() {
        if only.is_some_and(|o| o != i + 1) {
            continue;
        }
        crate::extract::scan_fonts(page, &mut dict_fonts);
        let Some(mut c) = crate::extract::page(page, &cache, &mut fonts, &dict_fonts, &mut pictures) else { continue };
        let lines = crate::lines::lines(&mut c, &fonts);
        println!("== page {} ({} x {})", i + 1, c.w, c.h);
        for f in &c.fills {
            println!("fill {:.1},{:.1}..{:.1},{:.1} {:?}", f.rect.x0, f.rect.y0, f.rect.x1, f.rect.y1, f.color);
        }
        for pic in &c.pictures {
            println!("pic {:.1},{:.1}..{:.1},{:.1} {}", pic.rect.x0, pic.rect.y0, pic.rect.x1, pic.rect.y1, pic.ext);
        }
        for r in &c.rules {
            println!("rule {} a={:.1} {:.1}..{:.1} w={:.2}", if r.horizontal { "H" } else { "V" }, r.a, r.from, r.to, r.width);
        }
        for l in &lines {
            let f = fonts.get(l.font).map_or("?".to_string(), |f| f.family.clone());
            let vert: String = l
                .spans
                .iter()
                .map(|s| match s.vert {
                    crate::lines::Vert::Base => 'b',
                    crate::lines::Vert::Sup => '^',
                    crate::lines::Vert::Sub => '_',
                })
                .collect();
            println!("{:6.1} {:6.1}..{:6.1} {:4.1} {:>4} {} {} | {}", l.base, l.x0, l.x1, l.size, l.band, f, vert, l.text());
        }
    }
}
