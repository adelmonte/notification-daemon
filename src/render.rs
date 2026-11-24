use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use cosmic_text::{
    Attrs, Buffer, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
    Weight, Wrap,
};
use resvg::tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pattern, Pixmap, PixmapMut, PixmapPaint, Point,
    Shader, SpreadMode, Stroke, Transform,
};

use crate::config::Color;
use crate::notification::Style;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    pub fn offset(self, dx: f32, dy: f32) -> Self {
        R { x: self.x + dx, y: self.y + dy, ..self }
    }
    pub fn intersect(self, o: R) -> Option<R> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        (x1 > x0 && y1 > y0).then(|| R::new(x0, y0, x1 - x0, y1 - y0))
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
}

#[derive(Clone, Copy)]
pub enum Dir {
    Diagonal,
    Down,
    Right,
}

pub enum Fill<'a> {
    Solid(Color),
    Gradient(&'a [Color], Dir),
}

pub struct Gfx {
    pub fonts: FontSystem,
    pub swash: SwashCache,
    shadows: HashMap<(u32, u32, u32, u32, u32), Rc<Pixmap>>,
}

impl Gfx {
    pub fn new() -> Self {
        Self { fonts: FontSystem::new(), swash: SwashCache::new(), shadows: HashMap::new() }
    }

    pub fn trim(&mut self) {
        self.shadows.clear();
        self.swash.image_cache.clear();
        self.swash.outline_command_cache.clear();
    }
}

pub struct Canvas<'a> {
    pub pm: PixmapMut<'a>,
    pub scale: f32,
    pub alpha: f32,
    pub ox: f32,
    pub oy: f32,
}

pub fn rrect(r: R, rad: f32) -> Option<Path> {
    let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let k = 0.447_715 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.cubic_to(x + w - k, y, x + w, y + k, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.cubic_to(x + w, y + h - k, x + w - k, y + h, x + w - rad, y + h);
    pb.line_to(x + rad, y + h);
    pb.cubic_to(x + k, y + h, x, y + h - k, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.cubic_to(x, y + k, x + k, y, x + rad, y);
    pb.close();
    pb.finish()
}

impl<'a> Canvas<'a> {
    pub fn new(pm: PixmapMut<'a>, scale: f32) -> Self {
        Self { pm, scale, alpha: 1.0, ox: 0.0, oy: 0.0 }
    }

    fn ts(&self) -> Transform {
        Transform::from_row(self.scale, 0.0, 0.0, self.scale, self.ox * self.scale, self.oy * self.scale)
    }

    fn shader(&self, fill: &Fill, r: R) -> Option<Shader<'static>> {
        match fill {
            Fill::Solid(c) => Some(Shader::SolidColor(c.with_alpha(self.alpha).sk())),
            Fill::Gradient(cols, dir) => {
                let (p0, p1) = match dir {
                    Dir::Diagonal => (Point::from_xy(r.x, r.y), Point::from_xy(r.right(), r.bottom())),
                    Dir::Down => (Point::from_xy(r.x, r.y), Point::from_xy(r.x, r.bottom())),
                    Dir::Right => (Point::from_xy(r.x, r.y), Point::from_xy(r.right(), r.y)),
                };
                let n = (cols.len().max(2) - 1) as f32;
                let stops = cols
                    .iter()
                    .enumerate()
                    .map(|(i, c)| GradientStop::new(i as f32 / n, c.with_alpha(self.alpha).sk()))
                    .collect();
                LinearGradient::new(p0, p1, stops, SpreadMode::Pad, Transform::identity())
            }
        }
    }

    pub fn fill_rrect(&mut self, r: R, rad: f32, fill: Fill) {
        let Some(path) = rrect(r, rad) else { return };
        let Some(shader) = self.shader(&fill, r) else { return };
        let paint = Paint { shader, anti_alias: true, ..Default::default() };
        let ts = self.ts();
        self.pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
    }

    pub fn stroke_rrect(&mut self, r: R, rad: f32, width: f32, color: Color) {
        if width <= 0.0 || color.a == 0 {
            return;
        }
        let h = width / 2.0;
        let Some(path) = rrect(R::new(r.x + h, r.y + h, r.w - width, r.h - width), (rad - h).max(0.0)) else { return };
        let paint = Paint {
            shader: Shader::SolidColor(color.with_alpha(self.alpha).sk()),
            anti_alias: true,
            ..Default::default()
        };
        let ts = self.ts();
        self.pm.stroke_path(&path, &paint, &Stroke { width, ..Default::default() }, ts, None);
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, rad: f32, color: Color) {
        self.fill_rrect(R::new(cx - rad, cy - rad, rad * 2.0, rad * 2.0), rad, Fill::Solid(color));
    }

    pub fn shadow(&mut self, gfx: &mut Gfx, r: R, rad: f32, blur: f32, dy: f32, color: Color) {
        if blur <= 0.0 || color.a == 0 {
            return;
        }
        let s = self.scale;
        let pad = (blur * s).ceil() as u32 + 2;
        let (w, h) = ((r.w * s).round() as u32, (r.h * s).round() as u32);
        let key = (w, h, (rad * s).round() as u32, (blur * s).round() as u32, u32::from_be_bytes([color.r, color.g, color.b, color.a]));
        let pm = match gfx.shadows.get(&key) {
            Some(p) => p.clone(),
            None => {
                let Some(mut pm) = Pixmap::new(w + pad * 2, h + pad * 2) else { return };
                if let Some(path) = rrect(R::new(pad as f32, pad as f32, w as f32, h as f32), rad * s) {
                    let paint = Paint { shader: Shader::SolidColor(color.sk()), anti_alias: true, ..Default::default() };
                    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
                box_blur(pm.data_mut(), (w + pad * 2) as usize, (h + pad * 2) as usize, (blur * s / 2.0).max(1.0) as usize);
                let pm = Rc::new(pm);
                if gfx.shadows.len() > 64 {
                    gfx.shadows.clear();
                }
                gfx.shadows.insert(key, pm.clone());
                pm
            }
        };
        let x = ((r.x + self.ox) * s).round() as i32 - pad as i32;
        let y = ((r.y + self.oy + dy) * s).round() as i32 - pad as i32;
        let paint = PixmapPaint { opacity: self.alpha, ..Default::default() };
        self.pm.draw_pixmap(x, y, pm.as_ref().as_ref(), &paint, Transform::identity(), None);
    }

    /// Draws an image already rendered at physical size, centered in the logical rect.
    pub fn image(&mut self, img: &Pixmap, r: R) {
        let s = self.scale;
        let x = ((r.x + self.ox) * s + (r.w * s - img.width() as f32) / 2.0).round() as i32;
        let y = ((r.y + self.oy) * s + (r.h * s - img.height() as f32) / 2.0).round() as i32;
        let paint = PixmapPaint { opacity: self.alpha, ..Default::default() };
        self.pm.draw_pixmap(x, y, img.as_ref(), &paint, Transform::identity(), None);
    }

    pub fn text(&mut self, gfx: &mut Gfx, buf: &Buffer, x: f32, y: f32, color: Color, shadow: bool) {
        if shadow {
            self.glyphs(gfx, buf, x, y + 1.0, Color { r: 0, g: 0, b: 0, a: 150 }, true);
        }
        self.glyphs(gfx, buf, x, y, color, false);
    }

    fn glyphs(&mut self, gfx: &mut Gfx, buf: &Buffer, x: f32, y: f32, color: Color, force: bool) {
        let s = self.scale;
        let (pw, ph) = (self.pm.width() as i32, self.pm.height() as i32);
        let alpha = self.alpha;
        let ox = (x + self.ox) * s;
        let oy = (y + self.oy) * s;
        for run in buf.layout_runs() {
            let line_y = (run.line_y * s + oy).round() as i32;
            for g in run.glyphs.iter() {
                let pg = g.physical((ox, 0.0), s);
                let col = if force {
                    color
                } else {
                    g.color_opt.map(|c| Color { r: c.r(), g: c.g(), b: c.b(), a: c.a() }).unwrap_or(color)
                };
                let Some(img) = gfx.swash.get_image(&mut gfx.fonts, pg.cache_key) else { continue };
                let gx = pg.x + img.placement.left;
                let gy = line_y + pg.y - img.placement.top;
                let (gw, gh) = (img.placement.width as i32, img.placement.height as i32);
                let data = self.pm.data_mut();
                for row in 0..gh {
                    let py = gy + row;
                    if py < 0 || py >= ph {
                        continue;
                    }
                    for colx in 0..gw {
                        let px = gx + colx;
                        if px < 0 || px >= pw {
                            continue;
                        }
                        let i = (row * gw + colx) as usize;
                        let (r, gg, b, a) = match img.content {
                            SwashContent::Mask => {
                                (col.r, col.g, col.b, (img.data[i] as u32 * col.a as u32 / 255) as u8)
                            }
                            SwashContent::SubpixelMask => {
                                let p = &img.data[i * 4..i * 4 + 4];
                                let m = ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8;
                                (col.r, col.g, col.b, (m as u32 * col.a as u32 / 255) as u8)
                            }
                            SwashContent::Color => {
                                let p = &img.data[i * 4..i * 4 + 4];
                                if force {
                                    (col.r, col.g, col.b, (p[3] as u32 * col.a as u32 / 255) as u8)
                                } else {
                                    (p[0], p[1], p[2], p[3])
                                }
                            }
                        };
                        let sa = (a as f32 * alpha) as u32;
                        if sa == 0 {
                            continue;
                        }
                        let o = ((py * pw + px) * 4) as usize;
                        let d = &mut data[o..o + 4];
                        let inv = 255 - sa;
                        d[0] = ((r as u32 * sa + d[0] as u32 * inv + 127) / 255) as u8;
                        d[1] = ((gg as u32 * sa + d[1] as u32 * inv + 127) / 255) as u8;
                        d[2] = ((b as u32 * sa + d[2] as u32 * inv + 127) / 255) as u8;
                        d[3] = (sa + (d[3] as u32 * inv + 127) / 255) as u8;
                    }
                }
            }
        }
    }
}

/// Three box-blur passes per axis on premultiplied RGBA, approximating a gaussian.
fn box_blur(data: &mut [u8], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0u8; data.len()];
    for _ in 0..3 {
        blur_axis(data, &mut tmp, w, h, r, true);
        blur_axis(&tmp, data, w, h, r, false);
    }
}

fn blur_axis(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let div = (2 * r + 1) as u32;
    for line in 0..lines {
        for ch in 0..4 {
            let mut sum: u32 = 0;
            for i in 0..=r.min(len - 1) {
                sum += src[idx(line, i) + ch] as u32;
            }
            for i in 0..len {
                dst[idx(line, i) + ch] = (sum / div) as u8;
                let add = i + r + 1;
                if add < len {
                    sum += src[idx(line, add) + ch] as u32;
                }
                if i >= r {
                    sum -= src[idx(line, i - r) + ch] as u32;
                }
            }
        }
    }
}

pub fn round_image(src: &Pixmap, rad: f32) -> Pixmap {
    let mut out = Pixmap::new(src.width(), src.height()).unwrap();
    if let Some(path) = rrect(R::new(0.0, 0.0, src.width() as f32, src.height() as f32), rad) {
        let paint = Paint {
            shader: Pattern::new(
                src.as_ref(),
                SpreadMode::Pad,
                resvg::tiny_skia::FilterQuality::Nearest,
                1.0,
                Transform::identity(),
            ),
            anti_alias: true,
            ..Default::default()
        };
        out.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    out
}

pub struct TextOpts<'a> {
    pub family: &'a str,
    pub size: f32,
    pub weight: u16,
    pub width: Option<f32>,
    pub max_lines: usize,
}

pub fn make_text(gfx: &mut Gfx, text: &str, spans: &[(Range<usize>, Style)], o: TextOpts) -> Buffer {
    let metrics = Metrics::new(o.size, (o.size * 1.3).round());
    let mut buf = Buffer::new(&mut gfx.fonts, metrics);
    buf.set_wrap(if o.width.is_some() { Wrap::WordOrGlyph } else { Wrap::None });
    if o.max_lines > 0 && o.width.is_some() {
        buf.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(o.max_lines)));
    }
    buf.set_size(o.width, None);
    let base = Attrs::new().family(Family::Name(o.family)).weight(Weight(o.weight));
    if spans.is_empty() {
        buf.set_text(text, &base, Shaping::Advanced, None);
    } else {
        let mut parts: Vec<(&str, Attrs)> = Vec::new();
        let mut pos = 0;
        for (r, st) in spans {
            if r.start > pos {
                parts.push((&text[pos..r.start], base.clone()));
            }
            let mut a = base.clone();
            if st.bold {
                a = a.weight(Weight(700));
            }
            if st.italic {
                a = a.style(cosmic_text::Style::Italic);
            }
            parts.push((&text[r.clone()], a));
            pos = r.end;
        }
        if pos < text.len() {
            parts.push((&text[pos..], base.clone()));
        }
        buf.set_rich_text(parts, &base, Shaping::Advanced, None);
    }
    buf.shape_until_scroll(&mut gfx.fonts, false);
    buf
}

pub fn text_size(buf: &Buffer) -> (f32, f32) {
    let mut w: f32 = 0.0;
    let mut h: f32 = 0.0;
    for run in buf.layout_runs() {
        w = w.max(run.line_w);
        h = h.max(run.line_top + run.line_height);
    }
    (w, h)
}
