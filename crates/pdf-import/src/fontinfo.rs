//! Which font each glyph was drawn in: the family name Word knows it by, bold, italic, and the
//! single line height and space width that reflowed text needs to break where the PDF did.
//!
//! The family comes from the embedded font's own name table (name ID 1, the legacy family Word
//! uses: "Calibri Light", "Arial Black"), else from the font dictionary's PostScript name
//! (`ABCDEF+TimesNewRomanPS-BoldMT` → Times New Roman, bold). The line height is measured the way
//! WordCraft lays out single spacing (hhea ascender, descender and line gap), on the installed
//! face when there is one, else on the embedded font.

use std::collections::HashMap;

use craft_pdf::interpret::font::OutlineFontData;
use craft_pdf::syntax::object::{Array, Dict, Name};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

/// At most this many distinct fonts are told apart; the rest share the last entry.
const MAX_FONTS: usize = 2048;

/// What a font dictionary says about its font.
#[derive(Clone, Debug, Default)]
pub(crate) struct DictFont {
    pub base_font: Option<String>,
    pub flags: u32,
    pub weight: Option<f32>,
    pub italic_angle: f32,
}

impl DictFont {
    pub(crate) fn read(dict: &Dict<'_>) -> DictFont {
        let base_font = dict.get::<Name<'_>>(b"BaseFont").map(|n| n.as_str().to_string());
        // A composite font describes itself in its descendant.
        let desc = dict.get::<Dict<'_>>(b"FontDescriptor").or_else(|| {
            dict.get::<Array<'_>>(b"DescendantFonts").and_then(|a| a.iter::<Dict<'_>>().next()).and_then(|d| d.get::<Dict<'_>>(b"FontDescriptor"))
        });
        let mut out = DictFont { base_font, ..Default::default() };
        if let Some(d) = desc {
            out.flags = d.get::<i64>(b"Flags").and_then(|f| u32::try_from(f).ok()).unwrap_or(0);
            out.weight = d.get::<f32>(b"FontWeight").filter(|w| w.is_finite());
            out.italic_angle = d.get::<f32>(b"ItalicAngle").filter(|a| a.is_finite()).unwrap_or(0.0);
            if out.base_font.is_none() {
                out.base_font = d.get::<Name<'_>>(b"FontName").map(|n| n.as_str().to_string());
            }
        }
        out
    }
}

/// A font as the document will name it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FontInfo {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// Single line height per point of size (hhea ascender + descender + line gap, per em).
    pub line: f32,
    /// The descender per em (the part of the line below the baseline).
    pub descent: f32,
    /// Width of a space per em.
    pub space: f32,
    /// A symbol font (Symbol, Wingdings…): its glyphs are bullets and marks, not words.
    pub symbol: bool,
}

/// The fonts seen so far, by the interpreter's cache key.
#[derive(Default)]
pub(crate) struct FontTable {
    pub fonts: Vec<FontInfo>,
    by_key: HashMap<u128, usize>,
    type3: Option<usize>,
}

impl FontTable {
    pub(crate) fn get(&self, i: usize) -> Option<&FontInfo> {
        self.fonts.get(i)
    }

    fn push(&mut self, f: FontInfo) -> usize {
        if self.fonts.len() >= MAX_FONTS {
            return self.fonts.len().saturating_sub(1);
        }
        self.fonts.push(f);
        self.fonts.len() - 1
    }

    /// The table index for a glyph's font.
    pub(crate) fn index(&mut self, key: u128, data: Option<OutlineFontData>, dicts: &HashMap<u128, DictFont>) -> usize {
        if let Some(i) = self.by_key.get(&key) {
            return *i;
        }
        let info = describe(data.as_ref(), dicts.get(&key));
        let i = self.push(info);
        self.by_key.insert(key, i);
        i
    }

    /// One entry for every Type 3 font (they are drawings and name no family).
    pub(crate) fn type3(&mut self) -> usize {
        if let Some(i) = self.type3 {
            return i;
        }
        let i = self.push(measured("Times New Roman", false, false, None));
        self.type3 = Some(i);
        i
    }
}

/// Strip a subset prefix (`ABCDEF+`).
fn strip_subset(name: &str) -> &str {
    match name.split_once('+') {
        Some((p, rest)) if p.len() == 6 && p.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

/// Families known by their PostScript base names, and the base-14 fonts mapped to the families
/// Word documents use for them.
const KNOWN: &[(&str, &str)] = &[
    ("timesnewromanpsmt", "Times New Roman"),
    ("timesnewromanps", "Times New Roman"),
    ("timesnewroman", "Times New Roman"),
    ("times", "Times New Roman"),
    ("timesroman", "Times New Roman"),
    ("nimbusromno9l", "Times New Roman"),
    ("nimbusroman", "Times New Roman"),
    ("liberationserif", "Times New Roman"),
    ("texgyretermes", "Times New Roman"),
    ("arialmt", "Arial"),
    ("arial", "Arial"),
    ("helvetica", "Arial"),
    ("nimbussanl", "Arial"),
    ("nimbussans", "Arial"),
    ("liberationsans", "Arial"),
    ("arialnarrow", "Arial Narrow"),
    ("arialblack", "Arial Black"),
    ("courier", "Courier New"),
    ("couriernewpsmt", "Courier New"),
    ("couriernewps", "Courier New"),
    ("couriernew", "Courier New"),
    ("nimbusmonl", "Courier New"),
    ("liberationmono", "Courier New"),
    ("trebuchetms", "Trebuchet MS"),
    ("comicsansms", "Comic Sans MS"),
    ("bookantiqua", "Book Antiqua"),
    ("palatinolinotype", "Palatino Linotype"),
    ("urwpalladiol", "Palatino Linotype"),
    ("centurygothic", "Century Gothic"),
    ("segoeui", "Segoe UI"),
    ("cambriamath", "Cambria Math"),
    ("lucidaconsole", "Lucida Console"),
    ("lucidasansunicode", "Lucida Sans Unicode"),
    ("msgothic", "MS Gothic"),
    ("msmincho", "MS Mincho"),
    ("simsun", "SimSun"),
    ("symbolmt", "Symbol"),
    ("zapfdingbats", "Wingdings"),
];

/// TeX's Computer Modern and Latin Modern fonts, by name prefix: family, bold, italic, mono.
fn tex_family(base: &str) -> Option<(&'static str, bool, bool)> {
    let l = base.to_ascii_lowercase();
    let lm = l.strip_prefix("lmroman").or_else(|| l.strip_prefix("lmsans")).or_else(|| l.strip_prefix("lmmono"));
    if let Some(rest) = lm {
        let fam = if l.starts_with("lmsans") {
            "Latin Modern Sans"
        } else if l.starts_with("lmmono") {
            "Latin Modern Mono"
        } else {
            "Latin Modern Roman"
        };
        return Some((fam, rest.contains("bold") || rest.contains("demi"), rest.contains("italic") || rest.contains("slant")));
    }
    let cm = l.strip_prefix("cm").or_else(|| l.strip_prefix("sfrm").map(|_| "r")).or_else(|| l.strip_prefix("sfbx").map(|_| "bx"))?;
    let digits = cm.trim_end_matches(|c: char| c.is_ascii_digit());
    let table: &[(&str, &str, bool, bool)] = &[
        ("r", "Latin Modern Roman", false, false),
        ("bx", "Latin Modern Roman", true, false),
        ("b", "Latin Modern Roman", true, false),
        ("ti", "Latin Modern Roman", false, true),
        ("sl", "Latin Modern Roman", false, true),
        ("bxti", "Latin Modern Roman", true, true),
        ("bxsl", "Latin Modern Roman", true, true),
        ("mi", "Latin Modern Roman", false, true),
        ("mib", "Latin Modern Roman", true, true),
        ("csc", "Latin Modern Roman", false, false),
        ("ss", "Latin Modern Sans", false, false),
        ("ssbx", "Latin Modern Sans", true, false),
        ("ssi", "Latin Modern Sans", false, true),
        ("tt", "Latin Modern Mono", false, false),
        ("itt", "Latin Modern Mono", false, true),
        ("sltt", "Latin Modern Mono", false, true),
        ("sy", "Latin Modern Roman", false, false),
        ("ex", "Latin Modern Roman", false, false),
    ];
    table.iter().find(|(k, ..)| *k == digits).map(|(_, f, b, i)| (*f, *b, *i))
}

/// Split `SourceSansPro` into `Source Sans Pro`.
fn split_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if let Some(p) = prev
            && c.is_ascii_uppercase()
            && (p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

fn style_words(style: &str) -> (bool, bool, Option<&'static str>) {
    let s = style.to_ascii_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demibold", "demi", "extrabold", "ultrabold"].iter().any(|w| s.contains(w))
        || s == "bd"
        || s == "bi"
        || s.starts_with("bdit")
        || s.ends_with("bd");
    let italic = s.contains("italic") || s.contains("oblique") || s.ends_with("it") || s == "bi" || s == "i" || s.contains("slanted");
    // Weights Word reaches through a family of their own.
    let extra = if s.contains("semilight") {
        Some("Semilight")
    } else if s.contains("light") && !s.contains("semibold") {
        Some("Light")
    } else if s.contains("thin") {
        Some("Thin")
    } else if s.contains("medium") {
        Some("Medium")
    } else if s.contains("black") || s.contains("heavy") {
        Some("Black")
    } else {
        None
    };
    (bold, italic, extra)
}

/// (family, bold, italic) from a PostScript name.
pub(crate) fn from_postscript(name: &str) -> (String, bool, bool) {
    let name = strip_subset(name.trim());
    if let Some((f, b, i)) = tex_family(name) {
        return (f.to_string(), b, i);
    }
    let (base, style) = match name.find(['-', ',']) {
        Some(i) => (name.get(..i).unwrap_or(name), name.get(i + 1..).unwrap_or("")),
        None => (name, ""),
    };
    // `Arial-BoldMT`, `TimesNewRomanPS-ItalicMT`: the MT and PS suffixes are no part of the name.
    let style = style.trim_end_matches("MT").trim_end_matches("PS");
    let (mut bold, mut italic, extra) = style_words(style);
    // A style folded into the base: `TimesNewRomanPS-BoldItalicMT` is split, `Helvetica-Bold`
    // too, but `ArialBold` or `Calibri,Bold` sometimes come joined.
    let mut base = base.to_string();
    for (suffix, b, i) in
        [("BoldItalic", true, true), ("BoldOblique", true, true), ("Bold", true, false), ("Italic", false, true), ("Oblique", false, true)]
    {
        if let Some(stripped) = base.strip_suffix(suffix)
            && !stripped.is_empty()
        {
            base = stripped.to_string();
            bold |= b;
            italic |= i;
            break;
        }
    }
    let key: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let mut family = match KNOWN.iter().find(|(k, _)| *k == key) {
        Some((_, f)) => (*f).to_string(),
        None => {
            let trimmed = base.trim_end_matches("PSMT").trim_end_matches("MT").trim_end_matches("PS");
            let trimmed = if trimmed.is_empty() { base.as_str() } else { trimmed };
            let split = split_camel(trimmed);
            if split != trimmed && !installed(&split) && installed(trimmed) { trimmed.to_string() } else { split }
        }
    };
    if let Some(e) = extra
        && family != "Arial Black"
    {
        family = format!("{family} {e}");
        if e == "Black" {
            bold = false;
        }
    }
    (family, bold, italic)
}

fn installed(family: &str) -> bool {
    wordcraft_fonts::FontDb::global().has_family(family)
}

/// A name that says nothing about the design (`F1`, `T1_0`, `Font12`).
fn generic(family: &str) -> bool {
    let f = family.trim();
    f.len() <= 2
        || f.chars().all(|c| c.is_ascii_digit() || c == '_' || c == ' ')
        || ["font", "f", "t", "tt", "c"].iter().any(|p| {
            f.to_ascii_lowercase().strip_prefix(p).is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '_'))
        })
}

/// Line height, descent and space width per em of an embedded font, WordCraft's way.
fn embedded_metrics(font: &FontRef<'_>) -> Option<(f32, f32, Option<f32>)> {
    let upem = f32::from(font.head().ok()?.units_per_em()).max(16.0);
    let hhea = font.hhea().ok();
    let (mut asc, mut desc, mut gap) = match &hhea {
        Some(h) => (f32::from(h.ascender().to_i16()), f32::from(h.descender().to_i16()).abs(), f32::from(h.line_gap().to_i16()).max(0.0)),
        None => (0.0, 0.0, 0.0),
    };
    if asc <= 0.0 || asc + desc > upem * 2.0 {
        let os2 = font.os2().ok()?;
        asc = f32::from(os2.us_win_ascent());
        desc = f32::from(os2.us_win_descent());
        gap = 0.0;
    }
    if asc <= 0.0 || asc + desc + gap > upem * 3.0 {
        return None;
    }
    let space = font
        .charmap()
        .map(' ')
        .map(|gid| font.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default()).advance_width(gid));
    let space = space.flatten().map(|w| w / upem).filter(|w| *w > 0.05 && *w < 1.0);
    Some(((asc + desc + gap) / upem, desc / upem, space))
}

/// A font named `family`, measured on the face WordCraft will draw it with; `embedded` metrics
/// stand in when that face is a substitute.
fn measured(family: &str, bold: bool, italic: bool, embedded: Option<(f32, f32, Option<f32>)>) -> FontInfo {
    let r = wordcraft_fonts::word::resolve(family, bold, italic);
    let (asc, desc) = wordcraft_fonts::word::line_metrics(&r.face);
    let upem = r.face.upem.max(16.0);
    let local = (((asc + desc) / upem) as f32, (desc / upem) as f32);
    let (line, descent) = match embedded {
        Some((l, d, _)) if r.substituted => (l, d),
        _ => local,
    };
    let space = embedded.and_then(|e| e.2).unwrap_or_else(|| {
        let shaped = wordcraft_fonts::shape(&r.face, " ", &[], |c| c);
        shaped.first().map(|g| g.x_advance as f32 / upem as f32).filter(|w| *w > 0.05 && *w < 1.0).unwrap_or(0.25)
    });
    let symbol = matches!(family.to_ascii_lowercase().as_str(), "symbol" | "wingdings" | "wingdings 2" | "wingdings 3" | "webdings");
    FontInfo { family: family.to_string(), bold, italic, mono: false, line: line.clamp(0.8, 2.5), descent: descent.clamp(0.0, 1.0), space, symbol }
}

fn describe(data: Option<&OutlineFontData>, dict: Option<&DictFont>) -> FontInfo {
    let face = data.and_then(|d| FontRef::new((*d.data).as_ref()).ok());
    let embedded = face.as_ref().and_then(embedded_metrics);
    let ps_name = dict.and_then(|d| d.base_font.clone()).or_else(|| data.and_then(|d| d.postscript_name.clone()));
    let (ps_family, ps_bold, ps_italic) = ps_name.as_deref().map(from_postscript).unwrap_or_default();
    let named = face
        .as_ref()
        .and_then(|f| f.localized_strings(skrifa::string::StringId::FAMILY_NAME).english_or_first())
        .map(|s| s.chars().collect::<String>())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !generic(s));
    let attrs = face.as_ref().map(|f| f.attributes());
    let flags = dict.map(|d| d.flags).unwrap_or(0);
    let mono = data.is_some_and(|d| d.is_monospace) || flags & 1 != 0;
    let serif = data.is_some_and(|d| d.is_serif) || flags & 2 != 0;
    let mut family = match named {
        // A name table naming a TeX font by its file name (CMR10) maps like the PostScript one.
        Some(n) if tex_family(&n.replace(' ', "")).is_none() => n,
        _ if !ps_family.is_empty() && !generic(&ps_family) => ps_family.clone(),
        _ if mono => "Courier New".to_string(),
        _ if serif => "Times New Roman".to_string(),
        _ => "Arial".to_string(),
    };
    // Base-14 and TeX names (Helvetica, CMR10) keep their mapping even when the name table says
    // otherwise.
    if ps_name.as_deref().is_some_and(|n| tex_family(strip_subset(n)).is_some()) {
        family.clone_from(&ps_family);
    }
    if family.starts_with("Latin Modern") && !installed(&family) {
        family = if family.ends_with("Mono") {
            "Courier New"
        } else if family.ends_with("Sans") {
            "Arial"
        } else {
            "Times New Roman"
        }
        .to_string();
    }
    let weight = data.and_then(|d| d.weight).map(|w| w as f32).or(dict.and_then(|d| d.weight)).or(attrs.map(|a| a.weight.value()));
    let bold = ps_bold || weight.is_some_and(|w| w >= 600.0) || flags & (1 << 18) != 0;
    let italic = ps_italic
        || data.is_some_and(|d| d.is_italic)
        || attrs.is_some_and(|a| !matches!(a.style, skrifa::attribute::Style::Normal))
        || flags & (1 << 6) != 0
        || dict.is_some_and(|d| d.italic_angle.abs() > 3.0);
    // A family named for its weight ("Arial Black") is not bold on top.
    let bold = bold && !family.ends_with(" Black");
    let mut info = measured(&family, bold, italic, embedded);
    info.mono = mono || family.to_ascii_lowercase().contains("mono") || family.starts_with("Courier");
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PostScript names map to the family, weight and slant Word gives them: MT and PS
    /// suffixes and subset prefixes go, styles after a hyphen or comma are read, base-14 and TeX
    /// names map to Word's families, Light weights get a family of their own.
    #[test]
    fn postscript_names_map_to_word_families() {
        assert_eq!(from_postscript("ABCDEF+TimesNewRomanPS-BoldItalicMT"), ("Times New Roman".into(), true, true));
        assert_eq!(from_postscript("ArialMT"), ("Arial".into(), false, false));
        assert_eq!(from_postscript("Arial-BoldMT"), ("Arial".into(), true, false));
        assert_eq!(from_postscript("Arial,Bold"), ("Arial".into(), true, false));
        assert_eq!(from_postscript("Helvetica-Oblique"), ("Arial".into(), false, true));
        assert_eq!(from_postscript("Times-Roman"), ("Times New Roman".into(), false, false));
        assert_eq!(from_postscript("CourierNewPSMT"), ("Courier New".into(), false, false));
        assert_eq!(from_postscript("QWERTY+Calibri-Light"), ("Calibri Light".into(), false, false));
        assert_eq!(from_postscript("Calibri-Bold"), ("Calibri".into(), true, false));
        assert!(from_postscript("CMBX12").1);
        assert!(from_postscript("CMTI10").2);
        assert_eq!(from_postscript("LMRoman10-Bold"), ("Latin Modern Roman".into(), true, false));
    }

    #[test]
    fn generic_names_are_told_apart() {
        assert!(generic("F1"));
        assert!(generic("T1_0"));
        assert!(generic("Font12"));
        assert!(!generic("Calibri"));
        assert!(!generic("Times New Roman"));
    }
}
