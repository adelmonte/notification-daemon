use std::rc::Rc;
use std::time::{Duration, Instant};

use cosmic_text::Buffer;
use resvg::tiny_skia::Pixmap;

use crate::config::{Config, HAlign, VAlign};
use crate::icons::{self, Icons};
use crate::notification::{self, Image, Notification, Style, Urgency};
use crate::render::{Canvas, Dir, Fill, Gfx, R, TextOpts, make_text, round_image, text_size};

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Card(u32),
    Close(u32),
    Action(u32, String),
    Otp(u32),
    ClearAll,
    Dnd,
    List,
}

impl Hit {
    pub fn is_button(&self) -> bool {
        !matches!(self, Hit::List)
    }
}

pub struct PopupState {
    pub shown_at: Option<Instant>,
    pub expires: Option<Instant>,
    pub duration: Option<Duration>,
    pub leaving: Option<Instant>,
    pub y: Option<f32>,
}

impl PopupState {
    pub fn new(duration: Option<Duration>) -> Self {
        Self { shown_at: None, expires: None, duration, leaving: None, y: None }
    }
}

struct CardLayout {
    width: f32,
    time_str: String,
    summary: Buffer,
    body: Option<Buffer>,
    time: Buffer,
    buttons: Vec<(Hit, Buffer, f32)>,
    height: f32,
    text_x: f32,
    text_w: f32,
    summary_h: f32,
    progress_y: f32,
    buttons_y: f32,
    button_h: f32,
}

pub struct Entry {
    pub n: Notification,
    pub otp: Option<String>,
    body: (String, Vec<(std::ops::Range<usize>, Style)>),
    image: Option<Rc<Pixmap>>,
    icon: Option<(u32, Option<Rc<Pixmap>>)>,
    layouts: [Option<CardLayout>; 2],
    pub popup: Option<PopupState>,
    pub in_center: bool,
}

impl Entry {
    pub fn new(mut n: Notification, cfg: &Config) -> Self {
        let body = notification::parse_markup(&n.body);
        let otp = cfg.general.otp_copy_button.then(|| notification::find_otp(&n.summary, &body.0)).flatten();
        let image = match n.image.take() {
            Some(Image::Raw(raw)) => icons::from_raw(&raw, (cfg.theme.icon_size * 3.0) as u32).map(Rc::new),
            Some(Image::Path(p)) => {
                n.image = Some(Image::Path(p));
                None
            }
            None => None,
        };
        Self { n, otp, body, image, icon: None, layouts: [None, None], popup: None, in_center: false }
    }

    pub fn id(&self) -> u32 {
        self.n.id
    }

    pub fn invalidate(&mut self) {
        self.layouts = [None, None];
        self.icon = None;
    }

    fn icon(&mut self, icons: &mut Icons, cfg: &Config, scale: f32) -> Option<Rc<Pixmap>> {
        let px = (cfg.theme.icon_size * scale).round() as u32;
        if let Some((s, ic)) = &self.icon
            && *s == px
        {
            return ic.clone();
        }
        let raw = if let Some(img) = &self.image {
            Some(Rc::new(icons::fit(img, px)))
        } else if let Some(Image::Path(p)) = &self.n.image {
            icons.load(p, px)
        } else {
            None
        }
        .or_else(|| icons.load_app(&self.n.app_icon, self.n.desktop_entry.as_deref(), &self.n.app_name, px));
        let rounded = raw.map(|p| Rc::new(round_image(&p, cfg.theme.icon_radius * scale)));
        self.icon = Some((px, rounded.clone()));
        rounded
    }
}

const ICON_GAP: f32 = 12.0;
const BTN_GAP: f32 = 6.0;
const CLOSE: f32 = 20.0;

fn ensure_layout(gfx: &mut Gfx, cfg: &Config, e: &mut Entry, width: f32, center: bool, has_icon: bool) {
    let slot = center as usize;
    let time_str = notification::relative_time(e.n.timestamp);
    if let Some(l) = &e.layouts[slot]
        && l.width == width
        && l.time_str == time_str
    {
        return;
    }
    let t = &cfg.theme;
    let (summary_lines, body_lines) = if center {
        (cfg.center.summary_lines, cfg.center.body_lines)
    } else {
        (cfg.popup.summary_lines, cfg.popup.body_lines)
    };
    let [pad_x, pad_y] = t.card_padding;
    let text_x = pad_x + if has_icon { t.icon_size + ICON_GAP } else { 0.0 };
    let text_w = (width - text_x - pad_x).max(40.0);

    let time = make_text(gfx, &time_str, &[], TextOpts { family: &t.font, size: t.time_size, weight: 500, width: None, max_lines: 1 });
    let time_w = text_size(&time).0.max(CLOSE);
    let summary_text = if e.n.summary.trim().is_empty() { e.n.app_name.as_str() } else { e.n.summary.as_str() };
    let summary = make_text(
        gfx,
        summary_text.trim(),
        &[],
        TextOpts { family: &t.font, size: t.summary_size, weight: 700, width: Some(text_w - time_w - 8.0), max_lines: summary_lines },
    );
    let summary_h = text_size(&summary).1;
    let body = (!e.body.0.is_empty() && body_lines > 0).then(|| {
        make_text(gfx, &e.body.0, &e.body.1, TextOpts { family: &t.font, size: t.body_size, weight: 400, width: Some(text_w), max_lines: body_lines })
    });
    let body_h = body.as_ref().map(|b| text_size(b).1 + 2.0).unwrap_or(0.0);

    let content_h = (summary_h + body_h).max(if has_icon { t.icon_size } else { 0.0 });
    let mut y = pad_y + content_h;
    let progress_y = y + 8.0;
    if e.n.value.is_some() {
        y += 16.0;
    }

    let mut buttons: Vec<(Hit, Buffer, f32)> = Vec::new();
    let label_opts = |w| TextOpts { family: &t.font, size: t.button_size * 0.9, weight: 500, width: w, max_lines: 1 };
    if let Some(code) = &e.otp {
        let b = make_text(gfx, &format!("\u{f018f} Copy {code}"), &[], label_opts(None));
        let w = text_size(&b).0;
        buttons.push((Hit::Otp(e.n.id), b, w));
    }
    for (key, label) in e.n.visible_actions() {
        let b = make_text(gfx, label, &[], label_opts(None));
        let w = text_size(&b).0;
        buttons.push((Hit::Action(e.n.id, key.clone()), b, w));
    }
    let button_h = (t.button_size * 1.3).round() + 12.0;
    let buttons_y = y + 8.0;
    if !buttons.is_empty() {
        let per_row = 3;
        let rows = buttons.len().div_ceil(per_row);
        let full_w = width - pad_x * 2.0;
        for row in buttons.chunks_mut(per_row) {
            let bw = (full_w - BTN_GAP * (row.len() as f32 - 1.0)) / row.len() as f32;
            for (_, buf, w) in row.iter_mut() {
                if *w > bw - 16.0 {
                    let text: String = buf.lines.iter().map(|l| l.text()).collect();
                    *buf = make_text(gfx, &text, &[], label_opts(Some(bw - 16.0)));
                    *w = text_size(buf).0;
                }
            }
        }
        y = buttons_y + rows as f32 * button_h + (rows as f32 - 1.0) * BTN_GAP;
    }
    let height = (y + pad_y).max(if has_icon { t.icon_size + pad_y * 2.0 } else { 44.0 });

    e.layouts[slot] = Some(CardLayout {
        width,
        time_str,
        summary,
        body,
        time,
        buttons,
        height,
        text_x,
        text_w,
        summary_h,
        progress_y,
        buttons_y,
        button_h,
    });
}

pub fn card_height(gfx: &mut Gfx, icons: &mut Icons, cfg: &Config, e: &mut Entry, width: f32, center: bool, scale: f32) -> f32 {
    let has_icon = e.icon(icons, cfg, scale).is_some();
    ensure_layout(gfx, cfg, e, width, center, has_icon);
    e.layouts[center as usize].as_ref().unwrap().height
}

pub struct CardCtx<'h> {
    pub hover: Option<&'h Hit>,
    pub hits: &'h mut Vec<(R, Hit)>,
    pub hit_offset: (f32, f32),
    pub hit_clip: Option<R>,
}

impl CardCtx<'_> {
    fn add(&mut self, r: R, h: Hit) {
        let r = r.offset(self.hit_offset.0, self.hit_offset.1);
        let r = match self.hit_clip {
            Some(c) => match r.intersect(c) {
                Some(r) => r,
                None => return,
            },
            None => r,
        };
        self.hits.push((r, h));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn paint_card(c: &mut Canvas, gfx: &mut Gfx, icons: &mut Icons, cfg: &Config, e: &mut Entry, x: f32, y: f32, width: f32, center: bool, ctx: &mut CardCtx) {
    let t = &cfg.theme;
    let icon = e.icon(icons, cfg, c.scale);
    ensure_layout(gfx, cfg, e, width, center, icon.is_some());
    let id = e.n.id;
    let l = e.layouts[center as usize].as_ref().unwrap();
    let r = R::new(x, y, width, l.height);
    let hovered = ctx.hover.is_some_and(|h| match h {
        Hit::Card(i) | Hit::Close(i) | Hit::Otp(i) | Hit::Action(i, _) => *i == id,
        _ => false,
    });
    let critical = e.n.urgency == Urgency::Critical;

    let (bg, border) = if hovered {
        (&t.card_bg_hover, t.border_hover)
    } else {
        match e.n.urgency {
            Urgency::Low => (&t.card_bg_low, t.border_low),
            Urgency::Normal => (&t.card_bg, t.border),
            Urgency::Critical => (&t.card_bg_critical, t.border_critical),
        }
    };
    let shadow = if critical { t.shadow_critical } else { t.shadow };
    let blur = if center { t.shadow_blur * 0.6 } else { t.shadow_blur };
    c.shadow(gfx, r, t.card_radius, blur, t.shadow_offset, shadow);
    c.fill_rrect(r, t.card_radius, Fill::Gradient(&bg[..], Dir::Diagonal));
    c.stroke_rrect(r, t.card_radius, t.border_width, border);
    ctx.add(r, Hit::Card(id));

    let [pad_x, pad_y] = t.card_padding;
    if let Some(img) = &icon {
        c.image(img, R::new(x + pad_x, y + pad_y, t.icon_size, t.icon_size));
    }
    let tx = x + l.text_x;
    c.text(gfx, &l.summary, tx, y + pad_y, t.text, t.text_shadow);
    if let Some(b) = &l.body {
        c.text(gfx, b, tx, y + pad_y + l.summary_h + 2.0, t.body, t.text_shadow);
    }

    let (tw, th) = text_size(&l.time);
    let first_line = l.summary.metrics().line_height;
    let right = x + width - pad_x;
    if hovered {
        let close_r = R::new(right - CLOSE + 4.0, y + pad_y + (first_line - CLOSE) / 2.0, CLOSE, CLOSE);
        let close_hover = ctx.hover == Some(&Hit::Close(id));
        let col = if close_hover { t.close_bg_hover } else { t.close_bg };
        c.fill_circle(close_r.x + CLOSE / 2.0, close_r.y + CLOSE / 2.0, CLOSE / 2.0, col);
        let (cx, cy, k) = (close_r.x + CLOSE / 2.0, close_r.y + CLOSE / 2.0, 3.5);
        let mut pb = resvg::tiny_skia::PathBuilder::new();
        pb.move_to(cx - k, cy - k);
        pb.line_to(cx + k, cy + k);
        pb.move_to(cx + k, cy - k);
        pb.line_to(cx - k, cy + k);
        if let Some(path) = pb.finish() {
            let paint = resvg::tiny_skia::Paint {
                shader: resvg::tiny_skia::Shader::SolidColor(t.text.with_alpha(c.alpha).sk()),
                anti_alias: true,
                ..Default::default()
            };
            let s = c.scale;
            let ts = resvg::tiny_skia::Transform::from_row(s, 0.0, 0.0, s, c.ox * s, c.oy * s);
            c.pm.stroke_path(
                &path,
                &paint,
                &resvg::tiny_skia::Stroke { width: 1.6, line_cap: resvg::tiny_skia::LineCap::Round, ..Default::default() },
                ts,
                None,
            );
        }
        ctx.add(close_r, Hit::Close(id));
    } else {
        c.text(gfx, &l.time, right - tw, y + pad_y + (first_line - th) / 2.0, t.dim, false);
    }

    if let Some(v) = e.n.value {
        let tr = R::new(tx, y + l.progress_y, l.text_w, 8.0);
        c.fill_rrect(tr, 4.0, Fill::Solid(t.trough));
        let pw = (tr.w - 2.0) * v as f32 / 100.0;
        if pw > 0.5 {
            c.fill_rrect(R::new(tr.x + 1.0, tr.y + 1.0, pw, 6.0), 3.0, Fill::Gradient(&t.progress[..], Dir::Right));
        }
    }

    if !l.buttons.is_empty() {
        let full_w = width - pad_x * 2.0;
        let mut by = y + l.buttons_y;
        for row in l.buttons.chunks(3) {
            let bw = (full_w - BTN_GAP * (row.len() as f32 - 1.0)) / row.len() as f32;
            let mut bx = x + pad_x;
            for (hit, buf, w) in row {
                let br = R::new(bx, by, bw, l.button_h);
                let hov = ctx.hover == Some(hit);
                let (bg, bd) = if hov { (t.button_bg_hover, t.button_border_hover) } else { (t.button_bg, t.button_border) };
                c.fill_rrect(br, 8.0, Fill::Solid(bg));
                c.stroke_rrect(br, 8.0, 1.0, bd);
                let th = text_size(buf).1;
                c.text(gfx, buf, bx + (bw - w) / 2.0, by + (l.button_h - th) / 2.0, t.button_text, false);
                ctx.add(br, hit.clone());
                bx += bw + BTN_GAP;
            }
            by += l.button_h + BTN_GAP;
        }
    }
}

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn shadow_room(cfg: &Config) -> f32 {
    (cfg.theme.shadow_blur + cfg.theme.shadow_offset + 4.0).ceil()
}

/// Horizontal extent of a column inside a surface: (surface width, column x).
pub fn column(h: HAlign, width: f32, m: &crate::config::Margin, room: f32) -> (f32, f32) {
    match h {
        HAlign::Right => (room + width + m.right.max(0.0), room),
        HAlign::Left => (m.left.max(0.0) + width + room, m.left.max(0.0)),
        HAlign::Center => (room * 2.0 + width, room),
    }
}

const SLIDE: f32 = 28.0;

/// Paints the popup stack. Returns true while anything is still animating.
#[allow(clippy::too_many_arguments)]
pub fn paint_popups(
    c: &mut Canvas,
    gfx: &mut Gfx,
    icons: &mut Icons,
    cfg: &Config,
    entries: &mut [Entry],
    surface_h: f32,
    hover: Option<&Hit>,
    hits: &mut Vec<(R, Hit)>,
    now: Instant,
    dt: f32,
) -> bool {
    let p = &cfg.popup;
    let room = shadow_room(cfg);
    let (_, col_x) = column(p.position.h(), p.width, &p.margin, room);
    let dur = Duration::from_millis(cfg.general.transition_ms.max(1) as u64).as_secs_f32();
    let scale = c.scale;
    let mut animating = false;

    let mut order: Vec<usize> = (0..entries.len()).filter(|&i| entries[i].popup.is_some()).collect();
    order.reverse();
    let heights: Vec<f32> = order
        .iter()
        .map(|&i| card_height(gfx, icons, cfg, &mut entries[i], p.width, false, scale))
        .collect();

    let mut total = 0.0;
    for (k, &i) in order.iter().enumerate() {
        if entries[i].popup.as_ref().unwrap().leaving.is_none() {
            total += heights[k] + p.gap;
        }
    }
    let v = p.position.v();
    let mut cursor = match v {
        VAlign::Top => p.margin.top,
        VAlign::Bottom => surface_h - p.margin.bottom,
        VAlign::Center => ((surface_h - total + p.gap) / 2.0).max(p.margin.top),
    };
    let k_smooth = 1.0 - (-dt / 0.07).exp();

    for (k, &i) in order.iter().enumerate() {
        let h = heights[k];
        let e = &mut entries[i];
        let ps = e.popup.as_mut().unwrap();
        let target = if v == VAlign::Bottom { cursor - h } else { cursor };
        if ps.leaving.is_none() {
            cursor = if v == VAlign::Bottom { cursor - h - p.gap } else { cursor + h + p.gap };
        }
        let y = match ps.y {
            None => target,
            Some(y) if (y - target).abs() < 0.5 => target,
            Some(y) => {
                animating = true;
                y + (target - y) * k_smooth
            }
        };
        ps.y = Some(y);
        let shown_at = *ps.shown_at.get_or_insert(now);
        let mut t_in = (now - shown_at).as_secs_f32() / dur;
        if t_in < 1.0 {
            animating = true;
        } else {
            t_in = 1.0;
        }
        let mut alpha = ease_out(t_in);
        let mut slide = 1.0 - ease_out(t_in);
        if let Some(l) = ps.leaving {
            let t = (now - l).as_secs_f32() / dur;
            animating = true;
            alpha *= 1.0 - ease_out(t);
            slide += ease_out(t);
        }
        if y + h < -40.0 || y > surface_h + 40.0 || alpha <= 0.0 {
            continue;
        }
        let dx = match p.position.h() {
            HAlign::Right => slide * SLIDE,
            HAlign::Left => -slide * SLIDE,
            HAlign::Center => 0.0,
        };
        let dy = if p.position.h() == HAlign::Center { if v == VAlign::Bottom { slide * SLIDE } else { -slide * SLIDE } } else { 0.0 };
        c.alpha = alpha;
        let leaving = ps.leaving.is_some();
        let mut sink = Vec::new();
        let mut ctx = CardCtx { hover, hits: if leaving { &mut sink } else { &mut *hits }, hit_offset: (0.0, 0.0), hit_clip: None };
        paint_card(c, gfx, icons, cfg, e, col_x + dx, y + dy, p.width, false, &mut ctx);
    }
    c.alpha = 1.0;
    animating
}

pub struct CenterOut {
    pub panel: R,
    pub max_scroll: f32,
}

fn widget_button(c: &mut Canvas, gfx: &mut Gfx, cfg: &Config, buf: &Buffer, r: R, hovered: bool, active: bool) {
    let t = &cfg.theme;
    let (tw, th) = text_size(buf);
    let fg = if active {
        c.fill_rrect(r, t.button_radius, Fill::Gradient(&t.dnd_on[..], Dir::Right));
        c.stroke_rrect(r, t.button_radius, 1.0, t.dnd_on[0]);
        t.dnd_on_text
    } else if hovered {
        c.fill_rrect(r, t.button_radius, Fill::Solid(t.accent));
        c.stroke_rrect(r, t.button_radius, 1.0, t.accent);
        t.dnd_on_text
    } else {
        c.fill_rrect(r, t.button_radius, Fill::Solid(t.accent.with_alpha(0.15)));
        c.stroke_rrect(r, t.button_radius, 1.0, t.accent.with_alpha(0.4));
        t.button_text
    };
    c.text(gfx, buf, r.x + (r.w - tw) / 2.0, r.y + (r.h - th) / 2.0, fg, false);
}

/// Paints the control center into a surface of `area` logical size.
#[allow(clippy::too_many_arguments)]
pub fn paint_center(
    c: &mut Canvas,
    gfx: &mut Gfx,
    icons: &mut Icons,
    cfg: &Config,
    entries: &mut [Entry],
    dnd: bool,
    scroll: f32,
    area: (f32, f32),
    hover: Option<&Hit>,
    hits: &mut Vec<(R, Hit)>,
    list_layer: &mut Option<Pixmap>,
) -> CenterOut {
    let cc = &cfg.center;
    let t = &cfg.theme;
    let room = shadow_room(cfg) + 4.0;
    let (_, px) = column(cc.position.h(), cc.width, &cc.margin, room);
    let pad = cc.padding;
    let inner_x = px + pad;
    let inner_w = cc.width - pad * 2.0;
    let scale = c.scale;

    let title_buf = make_text(gfx, &cc.title, &[], TextOpts { family: &t.font, size: t.title_size, weight: 700, width: None, max_lines: 1 });
    let clear_buf = make_text(gfx, &cc.clear_all_text, &[], TextOpts { family: &t.font, size: t.button_size, weight: 600, width: None, max_lines: 1 });
    let dnd_label = make_text(gfx, &cc.dnd_text, &[], TextOpts { family: &t.font, size: t.title_size * 0.9, weight: 600, width: None, max_lines: 1 });
    let dnd_state = make_text(
        gfx,
        if dnd { &cc.dnd_on_text } else { &cc.dnd_off_text },
        &[],
        TextOpts { family: &t.font, size: t.button_size, weight: 600, width: None, max_lines: 1 },
    );
    let dnd_w = {
        let on = make_text(gfx, &cc.dnd_on_text, &[], TextOpts { family: &t.font, size: t.button_size, weight: 600, width: None, max_lines: 1 });
        let off = make_text(gfx, &cc.dnd_off_text, &[], TextOpts { family: &t.font, size: t.button_size, weight: 600, width: None, max_lines: 1 });
        text_size(&on).0.max(text_size(&off).0) + 28.0
    };
    let btn_h = text_size(&clear_buf).1 + 12.0;
    let title_h = text_size(&title_buf).1.max(btn_h) + 24.0;
    let dnd_h = text_size(&dnd_label).1.max(btn_h) + 24.0;

    let mut header_h = 0.0;
    if cc.show_title {
        header_h += title_h + 12.0;
    }
    if cc.show_dnd {
        header_h += dnd_h + 12.0;
    }

    let visible: Vec<usize> = (0..entries.len()).rev().filter(|&i| entries[i].in_center).collect();
    let heights: Vec<f32> = visible.iter().map(|&i| card_height(gfx, icons, cfg, &mut entries[i], inner_w, true, scale)).collect();
    let content_h: f32 = if heights.is_empty() { 200.0 } else { heights.iter().sum::<f32>() + cc.gap * (heights.len() as f32 - 1.0) };

    let avail = (area.1 - cc.margin.top - cc.margin.bottom).max(100.0);
    let natural = pad * 2.0 + header_h + content_h;
    let panel_h = if cc.fill_height {
        avail
    } else if cc.height > 0.0 {
        cc.height.min(avail)
    } else {
        natural.max(cc.min_height).min(avail)
    };
    let py = match cc.position.v() {
        VAlign::Top => cc.margin.top,
        VAlign::Bottom => area.1 - cc.margin.bottom - panel_h,
        VAlign::Center => (area.1 - panel_h) / 2.0,
    };
    let panel = R::new(px, py, cc.width, panel_h);

    c.shadow(gfx, panel, t.center_radius, t.shadow_blur * 1.4, t.shadow_offset * 2.0, t.shadow);
    c.fill_rrect(panel, t.center_radius, Fill::Gradient(&t.center_bg[..], Dir::Down));
    c.stroke_rrect(panel, t.center_radius, t.border_width, t.center_border);

    let mut y = py + pad;
    if cc.show_title {
        let r = R::new(inner_x, y, inner_w, title_h);
        c.fill_rrect(R::new(r.x, r.y + 2.0, r.w, r.h), 12.0, Fill::Solid(t.title_border));
        c.fill_rrect(R::new(r.x, r.y, r.w, r.h), 12.0, Fill::Solid(t.center_bg[0]));
        c.fill_rrect(r, 12.0, Fill::Gradient(&t.title_bg[..], Dir::Right));
        let th = text_size(&title_buf).1;
        c.text(gfx, &title_buf, r.x + 14.0, r.y + (title_h - th) / 2.0, t.text, t.text_shadow);
        let bw = text_size(&clear_buf).0 + 28.0;
        let br = R::new(r.right() - 14.0 - bw, r.y + (title_h - btn_h) / 2.0, bw, btn_h);
        widget_button(c, gfx, cfg, &clear_buf, br, hover == Some(&Hit::ClearAll), false);
        hits.push((br, Hit::ClearAll));
        y += title_h + 12.0;
    }
    if cc.show_dnd {
        let r = R::new(inner_x, y, inner_w, dnd_h);
        let hov = hover == Some(&Hit::Dnd);
        c.fill_rrect(r, t.widget_radius, Fill::Solid(if hov { t.widget_bg_hover } else { t.widget_bg }));
        c.stroke_rrect(r, t.widget_radius, 1.0, if hov { t.accent.with_alpha(0.4) } else { t.widget_border });
        let lh = text_size(&dnd_label).1;
        c.text(gfx, &dnd_label, r.x + 16.0, r.y + (dnd_h - lh) / 2.0, t.text, t.text_shadow);
        let br = R::new(r.right() - 14.0 - dnd_w, r.y + (dnd_h - btn_h) / 2.0, dnd_w, btn_h);
        widget_button(c, gfx, cfg, &dnd_state, br, hov, dnd);
        hits.push((r, Hit::Dnd));
        y += dnd_h + 12.0;
    }

    let list = R::new(inner_x, y, inner_w, (panel.bottom() - pad - y).max(0.0));
    let max_scroll = (content_h - list.h).max(0.0);
    let scroll = scroll.clamp(0.0, max_scroll);

    if visible.is_empty() {
        let icon = make_text(gfx, &cc.empty_icon, &[], TextOpts { family: &t.font, size: 72.0, weight: 400, width: None, max_lines: 1 });
        let label = make_text(gfx, &cc.empty_text, &[], TextOpts { family: &t.font, size: t.font_size, weight: 500, width: None, max_lines: 1 });
        let (iw, ih) = text_size(&icon);
        let (lw, lh) = text_size(&label);
        let total = ih + 8.0 + lh;
        let top = list.y + ((list.h - total) / 2.0).max(0.0);
        c.text(gfx, &icon, list.x + (list.w - iw) / 2.0, top, t.text.with_alpha(0.15), false);
        c.text(gfx, &label, list.x + (list.w - lw) / 2.0, top + ih + 8.0, t.text.with_alpha(0.35), false);
        return CenterOut { panel, max_scroll };
    }

    let bleed = pad;
    let layer_r = R::new(list.x - bleed, list.y, list.w + bleed * 2.0, list.h);
    let (lw, lh) = ((layer_r.w * scale).round() as u32, (layer_r.h * scale).round() as u32);
    if lw == 0 || lh == 0 {
        return CenterOut { panel, max_scroll };
    }
    let reuse = list_layer.as_ref().is_some_and(|p| p.width() == lw && p.height() == lh);
    if !reuse {
        *list_layer = Pixmap::new(lw, lh);
    }
    let Some(layer) = list_layer.as_mut() else { return CenterOut { panel, max_scroll } };
    layer.fill(resvg::tiny_skia::Color::TRANSPARENT);
    {
        let mut lc = Canvas::new(layer.as_mut(), scale);
        lc.alpha = c.alpha;
        lc.ox = -layer_r.x;
        lc.oy = -list.y - scroll;
        let mut cy = list.y;
        for (k, &i) in visible.iter().enumerate() {
            let h = heights[k];
            let top = cy - scroll;
            if top + h >= list.y && top <= list.bottom() {
                let mut ctx = CardCtx { hover, hits: &mut *hits, hit_offset: (c.ox, -scroll), hit_clip: Some(list.offset(c.ox, 0.0)) };
                paint_card(&mut lc, gfx, icons, cfg, &mut entries[i], inner_x, cy, inner_w, true, &mut ctx);
            }
            cy += h + cc.gap;
        }
    }
    let x = ((layer_r.x + c.ox) * scale).round() as i32;
    let yy = ((layer_r.y + c.oy) * scale).round() as i32;
    let fade = scale * 10.0;
    fade_edges(layer, scroll > 0.5, scroll < max_scroll - 0.5, fade as u32);
    c.pm.draw_pixmap(x, yy, layer.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), resvg::tiny_skia::Transform::identity(), None);

    if max_scroll > 0.0 {
        let track = R::new(panel.right() - pad / 2.0 - 2.0, list.y + 4.0, 4.0, list.h - 8.0);
        let thumb_h = (track.h * list.h / content_h).max(24.0);
        let thumb_y = track.y + (track.h - thumb_h) * (scroll / max_scroll);
        c.fill_rrect(R::new(track.x, thumb_y, 4.0, thumb_h), 2.0, Fill::Solid(t.scrollbar));
    }
    hits.push((list.offset(c.ox, 0.0), Hit::List));
    CenterOut { panel, max_scroll }
}

fn fade_edges(pm: &mut Pixmap, top: bool, bottom: bool, n: u32) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let n = (n as usize).min(h / 2);
    let data = pm.data_mut();
    for i in 0..n {
        let k = (i as f32 + 0.5) / n as f32;
        for (on, row) in [(top, i), (bottom, h - 1 - i)] {
            if !on {
                continue;
            }
            for p in data[row * w * 4..(row + 1) * w * 4].iter_mut() {
                *p = (*p as f32 * k) as u8;
            }
        }
    }
}
