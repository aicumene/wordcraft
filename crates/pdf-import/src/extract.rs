//! What a PDF page draws, as positioned pieces: glyphs with their text, font, size and colour;
//! straight rules and filled boxes (table borders, underlines, shading); pictures; link areas.
//!
//! Coordinates are points from the top-left corner of the page as shown (crop box, rotation
//! applied), y down. A glyph's `base` is its baseline and `x0`..`x1` its advance.

use std::collections::HashMap;
use std::sync::Arc;

use craft_pdf::interpret::font::Glyph as PdfGlyph;
use craft_pdf::interpret::hayro_cmap::BfString;
use craft_pdf::interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, GlyphDrawMode, Image, ImageData, InterpreterCache, InterpreterSettings, LumaData, Paint,
    PathDrawMode, SoftMask, TransformExt,
};
use craft_pdf::syntax::object::{Array, Dict, Name, Stream};
use craft_pdf::syntax::page::Page;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use wordcraft_doc::Rgb;

use crate::fontinfo::{DictFont, FontTable};

/// At most this many glyphs are kept from one page (a page of tiny text has ~20 000).
const MAX_GLYPHS: usize = 200_000;
/// At most this many rules, boxes and pictures are kept from one page.
const MAX_SHAPES: usize = 50_000;
const MAX_PICTURES: usize = 400;
/// Pictures larger than this (pixels) are decoded at a lower resolution.
const MAX_PICTURE_PIXELS: u64 = 24_000_000;
/// A rule is at most this thick (points); anything thicker is a box.
pub(crate) const RULE_MAX: f64 = 2.6;

/// One drawn glyph.
#[derive(Clone, Debug)]
pub(crate) struct Glyph {
    /// Its text (a ligature gives several characters); empty when the font maps it to none.
    pub text: String,
    pub x0: f32,
    pub x1: f32,
    pub base: f32,
    /// Font size, points.
    pub size: f32,
    /// Index into the [`FontTable`].
    pub font: usize,
    pub color: Rgb,
    /// Drawn with fill and stroke (synthetic bold).
    pub stroked: bool,
}

/// A straight horizontal or vertical line (a stroke, or a box too thin to be anything else).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rule {
    /// Horizontal: `a` = y, `from..to` = x; vertical: `a` = x, `from..to` = y.
    pub horizontal: bool,
    pub a: f32,
    pub from: f32,
    pub to: f32,
    pub width: f32,
    pub color: Rgb,
}

/// A filled rectangle (cell or paragraph shading, a highlight).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fill {
    pub rect: Rect,
    pub color: Rgb,
}

/// A picture, encoded as PNG or JPEG.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    pub rect: Rect,
    pub data: Arc<Vec<u8>>,
    pub ext: &'static str,
}

/// A link annotation's area and target.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub rect: Rect,
    pub uri: String,
}

/// Everything taken from one page.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageContent {
    pub w: f32,
    pub h: f32,
    pub glyphs: Vec<Glyph>,
    pub rules: Vec<Rule>,
    pub fills: Vec<Fill>,
    pub pictures: Vec<Picture>,
    pub links: Vec<Link>,
    /// Bounding boxes of vector drawings that are neither rules nor boxes (charts, logos).
    pub drawings: Vec<Rect>,
    /// Some text is drawn invisibly (the text layer of a scanned page).
    pub invisible_text: bool,
    /// Glyphs left out because they are rotated.
    pub rotated_glyphs: usize,
}

/// Pictures decoded so far, by the image's cache key, so one drawn on every page is kept once.
pub(crate) type PictureCache = HashMap<u128, Option<(Arc<Vec<u8>>, &'static str)>>;

fn rgb(paint: &Paint<'_>) -> Rgb {
    match paint {
        Paint::Color(c) => {
            let [r, g, b, _] = c.to_rgba().to_rgba8();
            Rgb(r, g, b)
        }
        Paint::Pattern(_) => Rgb(0, 0, 0),
    }
}

fn opacity(paint: &Paint<'_>) -> f32 {
    match paint {
        Paint::Color(c) => c.to_rgba().components()[3],
        Paint::Pattern(_) => 1.0,
    }
}

struct Collector<'f> {
    out: PageContent,
    fonts: &'f mut FontTable,
    dict_fonts: &'f HashMap<u128, DictFont>,
    pictures: &'f mut PictureCache,
    clips: Vec<Rect>,
    page: Rect,
}

impl Collector<'_> {
    fn clip(&self) -> Rect {
        self.clips.last().copied().unwrap_or(self.page)
    }

    fn shape_budget(&self) -> bool {
        self.out.rules.len() + self.out.fills.len() + self.out.drawings.len() < MAX_SHAPES
    }

    fn rule(&mut self, horizontal: bool, a: f64, from: f64, to: f64, width: f64, color: Rgb) {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        // Clipped away entirely, or off the page.
        let c = self.clip().inflate(1.0, 1.0);
        let visible =
            if horizontal { a >= c.y0 && a <= c.y1 && to >= c.x0 && from <= c.x1 } else { a >= c.x0 && a <= c.x1 && to >= c.y0 && from <= c.y1 };
        if !visible || !self.shape_budget() {
            return;
        }
        let (from, to) = if horizontal { (from.max(c.x0), to.min(c.x1)) } else { (from.max(c.y0), to.min(c.y1)) };
        self.out.rules.push(Rule { horizontal, a: a as f32, from: from as f32, to: to as f32, width: width.max(0.1) as f32, color });
    }

    /// One subpath of a filled path: a box, a rule, or part of a drawing.
    fn fill_subpath(&mut self, sub: &BezPath, color: Rgb) {
        let bb = sub.bounding_box();
        if !bb.is_finite() || bb.width() <= 0.0 && bb.height() <= 0.0 {
            return;
        }
        let bb = bb.intersect(self.clip());
        if bb.width() <= 0.0 || bb.height() <= 0.0 {
            return;
        }
        let curved = sub.elements().iter().any(|e| matches!(e, PathEl::QuadTo(..) | PathEl::CurveTo(..)));
        let area = sub.area().abs();
        let boxy = !curved && area >= 0.9 * bb.area();
        if !boxy {
            if self.shape_budget() {
                self.out.drawings.push(bb);
            }
            return;
        }
        if bb.height() <= RULE_MAX && bb.width() > bb.height() {
            self.rule(true, bb.center().y, bb.x0, bb.x1, bb.height(), color);
        } else if bb.width() <= RULE_MAX && bb.height() > bb.width() {
            self.rule(false, bb.center().x, bb.y0, bb.y1, bb.width(), color);
        } else if color != Rgb(255, 255, 255) && self.shape_budget() {
            self.out.fills.push(Fill { rect: bb, color });
        }
    }

    fn picture(&mut self, image: Image<'_, '_>, transform: Affine) {
        let Image::Raster(raster) = image else { return };
        if self.out.pictures.len() >= MAX_PICTURES {
            return;
        }
        let (w, h) = (raster.width(), raster.height());
        if w == 0 || h == 0 {
            return;
        }
        let rect = transform.transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
        let rect = rect.intersect(self.page);
        if !rect.is_finite() || rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }
        let key = raster.cache_key();
        if let Some(found) = self.pictures.get(&key) {
            if let Some((data, ext)) = found {
                self.out.pictures.push(Picture { rect, data: data.clone(), ext });
            }
            return;
        }
        let encoded = passthrough_jpeg(raster.stream()).map(|d| (Arc::new(d), "jpg")).or_else(|| {
            let pixels = u64::from(w) * u64::from(h);
            let target = (pixels > MAX_PICTURE_PIXELS).then(|| {
                let k = (MAX_PICTURE_PIXELS as f64 / pixels as f64).sqrt();
                (((f64::from(w) * k) as u32).max(1), ((f64::from(h) * k) as u32).max(1))
            });
            let mut out = None;
            raster.with_rgba(|data, alpha| out = encode_png(data, alpha), target);
            out.map(|d| (Arc::new(d), "png"))
        });
        if let Some((data, ext)) = &encoded {
            self.out.pictures.push(Picture { rect, data: data.clone(), ext });
        }
        self.pictures.insert(key, encoded);
    }
}

/// The picture's own bytes when it is a plain JPEG (DCT-encoded RGB or grey, no mask or decode
/// array), which Word shows as it is.
fn passthrough_jpeg(stream: &Stream<'_>) -> Option<Vec<u8>> {
    use craft_pdf::syntax::Filter;
    let filters = stream.filters();
    if filters.len() != 1 || !matches!(filters.first(), Some(Filter::DctDecode)) {
        return None;
    }
    let dict = stream.dict();
    if dict.contains_key(b"SMask") || dict.contains_key(b"Mask") || dict.contains_key(b"Decode") {
        return None;
    }
    let space = dict.get::<Name<'_>>(b"ColorSpace")?;
    if !matches!(space.as_str(), "DeviceRGB" | "DeviceGray") {
        return None;
    }
    let raw = stream.raw_data();
    // A JPEG starts with SOI.
    raw.starts_with(&[0xFF, 0xD8]).then(|| raw.into_owned())
}

fn encode_png(data: ImageData, alpha: Option<LumaData>) -> Option<Vec<u8>> {
    use image::{DynamicImage, ImageFormat};
    let (w, h) = (data.width(), data.height());
    let img = match (data, alpha) {
        (ImageData::Rgb(rgb), Some(a)) if a.width == w && a.height == h && a.data.iter().any(|v| *v < 255) => {
            let mut px = Vec::with_capacity(rgb.data.len() / 3 * 4);
            for (c, a) in rgb.data.as_chunks::<3>().0.iter().zip(a.data.iter()) {
                px.extend_from_slice(c);
                px.push(*a);
            }
            DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, px)?)
        }
        (ImageData::Luma(l), Some(a)) if a.width == w && a.height == h && a.data.iter().any(|v| *v < 255) => {
            let px: Vec<u8> = l.data.iter().zip(a.data.iter()).flat_map(|(v, a)| [*v, *a]).collect();
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_raw(w, h, px)?)
        }
        (ImageData::Rgb(rgb), _) => DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, rgb.data)?),
        (ImageData::Luma(l), _) => DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, l.data)?),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

impl<'a> Device<'a> for Collector<'_> {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}

    fn draw_path(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
        if opacity(paint) < 0.05 {
            return;
        }
        let color = rgb(paint);
        let device = transform * path.clone();
        match mode {
            PathDrawMode::Fill(_) => {
                let mut sub = BezPath::new();
                for el in device.elements() {
                    if matches!(el, PathEl::MoveTo(_)) && !sub.elements().is_empty() {
                        self.fill_subpath(&sub, color);
                        sub = BezPath::new();
                    }
                    sub.push(*el);
                }
                if !sub.elements().is_empty() {
                    self.fill_subpath(&sub, color);
                }
            }
            PathDrawMode::Stroke(props) => {
                let scale = transform.determinant().abs().sqrt();
                let width = f64::from(props.line_width) * scale;
                if !width.is_finite() || width > 3.0 * RULE_MAX {
                    return;
                }
                let mut start = Point::ZERO;
                let mut cur = Point::ZERO;
                let mut curved = false;
                let segment = |me: &mut Self, p: Point, q: Point| {
                    let (dx, dy) = ((q.x - p.x).abs(), (q.y - p.y).abs());
                    if dy < 0.6 && dx >= 0.6 {
                        me.rule(true, (p.y + q.y) / 2.0, p.x, q.x, width, color);
                    } else if dx < 0.6 && dy >= 0.6 {
                        me.rule(false, (p.x + q.x) / 2.0, p.y, q.y, width, color);
                    } else if dx >= 0.6 && dy >= 0.6 && me.shape_budget() {
                        me.out.drawings.push(Rect::from_points(p, q));
                    }
                };
                for el in device.elements() {
                    match *el {
                        PathEl::MoveTo(p) => {
                            start = p;
                            cur = p;
                        }
                        PathEl::LineTo(p) => {
                            segment(self, cur, p);
                            cur = p;
                        }
                        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                            curved = true;
                            cur = p;
                        }
                        PathEl::ClosePath => {
                            segment(self, cur, start);
                            cur = start;
                        }
                    }
                }
                if curved && self.shape_budget() {
                    let bb = device.bounding_box().intersect(self.clip());
                    if bb.area() > 0.0 {
                        self.out.drawings.push(bb);
                    }
                }
            }
        }
    }

    fn push_clip_path(&mut self, clip: &ClipPath) {
        let bb = clip.path.bounding_box();
        let next = if bb.is_finite() { self.clip().intersect(bb) } else { self.clip() };
        self.clips.push(next);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph(&mut self, glyph: &PdfGlyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        if matches!(mode, GlyphDrawMode::Clip) || self.out.glyphs.len() >= MAX_GLYPHS {
            return;
        }
        let t = transform * glyph_transform;
        let [a, b, c, d, e, f] = t.as_coeffs();
        let size = (c * c + d * d).sqrt() * 1000.0;
        let width_scale = (a * a + b * b).sqrt() * 1000.0;
        if !size.is_finite() || !width_scale.is_finite() || !(0.5..=1500.0).contains(&size) {
            return;
        }
        // Upright text only: the baseline runs left to right and the glyph's up is the page's up.
        if b.abs() > 0.02 * a.abs() || c.abs() > 0.02 * d.abs() || a <= 0.0 || d >= 0.0 {
            self.out.rotated_glyphs += 1;
            return;
        }
        let origin = Point::new(e, f);
        let clip = self.clip().inflate(0.5 * size, 0.5 * size);
        if !clip.contains(origin) {
            return;
        }
        let text = match glyph.as_unicode() {
            Some(BfString::Char(ch)) => ch.to_string(),
            Some(BfString::String(s)) => s,
            None => String::new(),
        };
        let (advance, font) = match glyph {
            PdfGlyph::Outline(g) => {
                let font = self.fonts.index(g.font_cache_key(), g.font_data(), self.dict_fonts);
                (g.advance_width().map(f64::from), font)
            }
            PdfGlyph::Type3(_) => (None, self.fonts.type3()),
        };
        let x1 = match advance {
            Some(adv) if adv.is_finite() && adv >= 0.0 => e + a * adv,
            // Half an em when the font gives no width.
            _ => e + 0.5 * width_scale,
        };
        let color = rgb(paint);
        let stroke = matches!(mode, GlyphDrawMode::Stroke(_));
        if let Some(last) = self.out.glyphs.last_mut()
            && last.text == text
            && (f64::from(last.x0) - e).abs() < 0.01 * size
            && (f64::from(last.base) - f).abs() < 0.01 * size
        {
            // Fill then stroke of the same glyph: one glyph, drawn bold.
            if stroke {
                last.stroked = true;
            }
            return;
        }
        if matches!(mode, GlyphDrawMode::Invisible) {
            self.out.invisible_text = true;
        } else if opacity(paint) < 0.05 {
            return;
        }
        self.out.glyphs.push(Glyph { text, x0: e as f32, x1: x1 as f32, base: f as f32, size: size as f32, font, color, stroked: stroke });
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        self.picture(image, transform);
    }

    fn pop_clip_path(&mut self) {
        self.clips.pop();
    }

    fn pop_transparency_group(&mut self) {}
}

/// The fonts a page's resources name (and those of its forms, a few levels down), by the cache
/// key the interpreter gives their glyphs.
pub(crate) fn scan_fonts(page: &Page<'_>, out: &mut HashMap<u128, DictFont>) {
    fn fonts_of(res: &Dict<'_>, out: &mut HashMap<u128, DictFont>, depth: u32) {
        if depth > 4 {
            return;
        }
        if let Some(fonts) = res.get::<Dict<'_>>(b"Font") {
            for key in fonts.keys().take(512) {
                if let Some(font) = fonts.get::<Dict<'_>>(key.as_ref()) {
                    out.entry(font.cache_key()).or_insert_with(|| DictFont::read(&font));
                }
            }
        }
        if let Some(xobjects) = res.get::<Dict<'_>>(b"XObject") {
            for key in xobjects.keys().take(512) {
                if let Some(form) = xobjects.get::<Stream<'_>>(key.as_ref())
                    && form.dict().get::<Name<'_>>(b"Subtype").is_some_and(|n| n.as_str() == "Form")
                    && let Some(r) = form.dict().get::<Dict<'_>>(b"Resources")
                {
                    fonts_of(&r, out, depth + 1);
                }
            }
        }
    }
    let res = page.resources();
    for key in res.fonts.keys().take(512) {
        if let Some(font) = res.fonts.get::<Dict<'_>>(key.as_ref()) {
            out.entry(font.cache_key()).or_insert_with(|| DictFont::read(&font));
        }
    }
    for key in res.x_objects.keys().take(512) {
        if let Some(form) = res.x_objects.get::<Stream<'_>>(key.as_ref())
            && form.dict().get::<Name<'_>>(b"Subtype").is_some_and(|n| n.as_str() == "Form")
            && let Some(r) = form.dict().get::<Dict<'_>>(b"Resources")
        {
            fonts_of(&r, out, 1);
        }
    }
}

/// The page's URI link annotations, in page coordinates.
fn links(page: &Page<'_>, to_page: Affine) -> Vec<Link> {
    let mut out = Vec::new();
    let Some(annots) = page.raw().get::<Array<'_>>(b"Annots") else { return out };
    for a in annots.iter::<Dict<'_>>().take(2000) {
        if a.get::<Name<'_>>(b"Subtype").is_none_or(|n| n.as_str() != "Link") {
            continue;
        }
        let Some(r) = a.get::<craft_pdf::syntax::object::Rect>(b"Rect") else { continue };
        let Some(action) = a.get::<Dict<'_>>(b"A") else { continue };
        if action.get::<Name<'_>>(b"S").is_none_or(|n| n.as_str() != "URI") {
            continue;
        }
        let Some(uri) = action.get::<craft_pdf::syntax::object::String<'_>>(b"URI") else { continue };
        let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_string();
        if uri.is_empty() || uri.len() > 2048 {
            continue;
        }
        let rect = to_page.transform_rect_bbox(Rect::new(r.x0, r.y0, r.x1, r.y1));
        out.push(Link { rect, uri });
    }
    out
}

/// Interpret one page. `None` when the interpreter failed part way: what it collected is not
/// to be trusted, nor is `cache`.
pub(crate) fn page<'a>(
    page: &'a Page<'a>,
    cache: &InterpreterCache<'a>,
    fonts: &mut FontTable,
    dict_fonts: &HashMap<u128, DictFont>,
    pictures: &mut PictureCache,
) -> Option<PageContent> {
    let (w, h) = page.render_dimensions();
    let bounds = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
    let to_page = page.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(to_page, bounds, cache, page.xref(), InterpreterSettings::default());
    let mut dev = Collector { out: PageContent { w, h, ..Default::default() }, fonts, dict_fonts, pictures, clips: Vec::new(), page: bounds };
    if craft_pdf::interpret_page(page, &mut ctx, &mut dev).is_err() {
        log::warn!("PDF import: a page could not be read");
        return None;
    }
    let mut out = dev.out;
    out.links = links(page, to_page);
    Some(out)
}
