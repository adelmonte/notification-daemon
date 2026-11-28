use std::path::PathBuf;
use std::time::{Duration, Instant};

use resvg::tiny_skia::{Pixmap, PixmapMut};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            LoopHandle, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        client::{
            Connection, Proxy, QueueHandle,
            backend::ObjectId,
            globals::GlobalList,
            protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};

use crate::config::{self, Config, HAlign, KeyboardMode, LayerKind, RuleSound, RuleState};
use crate::dbus::{Ctl, Dbus, Msg, Shared};
use crate::exec;
use crate::icons::Icons;
use crate::notification::{Notification, Urgency};
use crate::render::{Canvas, Gfx, R};
use crate::ui::{self, Entry, Hit, PopupState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Popup,
    Center,
}

/// Invisible full-screen layer under the control center: keeps focus-follows-mouse from
/// reaching other windows on the way to the panel, and closes the panel when clicked.
struct Catcher {
    layer: LayerSurface,
    viewport: WpViewport,
    pool: SlotPool,
}

impl Drop for Catcher {
    fn drop(&mut self) {
        self.viewport.destroy();
    }
}

struct Surf {
    layer: LayerSurface,
    pool: SlotPool,
    viewport: Option<WpViewport>,
    frac: Option<WpFractionalScaleV1>,
    blur: Option<ExtBackgroundEffectSurfaceV1>,
    scale120: u32,
    int_scale: i32,
    width: u32,
    height: u32,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    hits: Vec<(R, Hit)>,
    input: Vec<R>,
    last_frame: Instant,
    list_layer: Option<Pixmap>,
}

impl Surf {
    fn scale(&self) -> f32 {
        self.scale120 as f32 / 120.0
    }
}

impl Drop for Surf {
    fn drop(&mut self) {
        if let Some(f) = self.frac.take() {
            f.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        if let Some(b) = self.blur.take() {
            b.destroy();
        }
    }
}

pub struct ProtoData;
pub struct FracData(ObjectId);

impl Dispatch2<WpFractionalScaleManagerV1, App> for ProtoData {
    fn event(&self, _: &mut App, _: &WpFractionalScaleManagerV1, _: <WpFractionalScaleManagerV1 as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
}
impl Dispatch2<WpViewporter, App> for ProtoData {
    fn event(&self, _: &mut App, _: &WpViewporter, _: <WpViewporter as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
}
impl Dispatch2<WpViewport, App> for ProtoData {
    fn event(&self, _: &mut App, _: &WpViewport, _: <WpViewport as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
}
impl Dispatch2<ExtBackgroundEffectManagerV1, App> for ProtoData {
    fn event(&self, app: &mut App, _: &ExtBackgroundEffectManagerV1, ev: ext_background_effect_manager_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = ev {
            app.blur_supported = flags
                .into_result()
                .map(|f| f.contains(ext_background_effect_manager_v1::Capability::Blur))
                .unwrap_or(false);
        }
    }
}
impl Dispatch2<ExtBackgroundEffectSurfaceV1, App> for ProtoData {
    fn event(&self, _: &mut App, _: &ExtBackgroundEffectSurfaceV1, _: <ExtBackgroundEffectSurfaceV1 as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
}
impl Dispatch2<WpFractionalScaleV1, App> for FracData {
    fn event(&self, app: &mut App, _: &WpFractionalScaleV1, ev: wp_fractional_scale_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            for which in [Which::Popup, Which::Center] {
                if let Some(s) = app.surf_mut(which)
                    && s.layer.wl_surface().id() == self.0
                    && s.scale120 != scale
                {
                    s.scale120 = scale;
                    s.list_layer = None;
                    app.mark_dirty(which);
                }
            }
        }
    }
}

pub struct App {
    qh: QueueHandle<App>,
    handle: LoopHandle<'static, App>,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    abgr: bool,
    cursor_mgr: Option<CursorShapeManager>,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    blur_mgr: Option<ExtBackgroundEffectManagerV1>,
    blur_supported: bool,

    pointer: Option<wl_pointer::WlPointer>,
    cursor_dev: Option<WpCursorShapeDeviceV1>,
    cursor_serial: u32,
    cursor_shape: Option<Shape>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    ptr_on: Option<Which>,
    ptr_pos: (f32, f32),
    hover: Option<Hit>,
    pressed: Option<(Which, Hit)>,
    kb_focus: bool,

    popup: Option<Surf>,
    catcher: Option<Catcher>,
    center: Option<Surf>,
    center_open: bool,
    center_anim: Option<(Instant, bool)>,
    scroll: f32,
    max_scroll: f32,

    entries: Vec<Entry>,
    dnd: bool,
    gfx: Gfx,
    icons: Icons,
    cfg: Config,
    cfg_path: Option<PathBuf>,
    dbus: Dbus,
    timer: Option<RegistrationToken>,
    last_sound: Option<Instant>,
}

fn dnd_file() -> PathBuf {
    config::state_dir().join("dnd")
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        globals: &GlobalList,
        qh: QueueHandle<App>,
        handle: LoopHandle<'static, App>,
        cfg: Config,
        cfg_path: Option<PathBuf>,
        dbus: Dbus,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let layer_shell = LayerShell::bind(globals, &qh).map_err(|e| format!("layer shell unavailable: {e}"))?;
        let shm = Shm::bind(globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
        let icons = Icons::new(cfg.general.icon_theme.as_deref());
        let dnd = std::fs::read_to_string(dnd_file()).map(|s| s.trim() == "1").unwrap_or(false);
        let mut app = Self {
            registry: RegistryState::new(globals),
            seats: SeatState::new(globals, &qh),
            outputs: OutputState::new(globals, &qh),
            compositor,
            layer_shell,
            shm,
            abgr: false,
            cursor_mgr: CursorShapeManager::bind(globals, &qh).ok(),
            frac_mgr: globals.bind(&qh, 1..=1, ProtoData).ok(),
            viewporter: globals.bind(&qh, 1..=1, ProtoData).ok(),
            blur_mgr: globals.bind(&qh, 1..=1, ProtoData).ok(),
            blur_supported: false,
            pointer: None,
            cursor_dev: None,
            cursor_serial: 0,
            cursor_shape: None,
            keyboard: None,
            ptr_on: None,
            ptr_pos: (0.0, 0.0),
            hover: None,
            pressed: None,
            kb_focus: false,
            popup: None,
            catcher: None,
            center: None,
            center_open: false,
            center_anim: None,
            scroll: 0.0,
            max_scroll: 0.0,
            entries: Vec::new(),
            dnd,
            gfx: Gfx::new(),
            icons,
            cfg,
            cfg_path,
            dbus,
            timer: None,
            last_sound: None,
            qh,
            handle,
        };
        app.publish();
        Ok(app)
    }

    fn surf_mut(&mut self, which: Which) -> Option<&mut Surf> {
        match which {
            Which::Popup => self.popup.as_mut(),
            Which::Center => self.center.as_mut(),
        }
    }

    fn surf(&self, which: Which) -> Option<&Surf> {
        match which {
            Which::Popup => self.popup.as_ref(),
            Which::Center => self.center.as_ref(),
        }
    }

    fn which_for(&self, s: &wl_surface::WlSurface) -> Option<Which> {
        if self.popup.as_ref().is_some_and(|p| p.layer.wl_surface() == s) {
            Some(Which::Popup)
        } else if self.center.as_ref().is_some_and(|p| p.layer.wl_surface() == s) {
            Some(Which::Center)
        } else {
            None
        }
    }

    fn create_surf(&mut self, which: Which) -> Surf {
        let qh = &self.qh;
        let surface = self.compositor.create_surface(qh);
        let (layer_kind, ns, pos, width, margin) = match which {
            Which::Popup => (self.cfg.general.layer, "notification-daemon-popup", self.cfg.popup.position, self.cfg.popup.width, self.cfg.popup.margin),
            Which::Center => (self.cfg.general.layer, "notification-daemon-center", self.cfg.center.position, self.cfg.center.width, self.cfg.center.margin),
        };
        let layer_kind = match layer_kind {
            LayerKind::Top => Layer::Top,
            LayerKind::Overlay => Layer::Overlay,
        };
        let layer = self.layer_shell.create_layer_surface(qh, surface, layer_kind, Some(ns), None);
        let mut anchor = Anchor::TOP | Anchor::BOTTOM;
        match pos.h() {
            HAlign::Left => anchor |= Anchor::LEFT,
            HAlign::Right => anchor |= Anchor::RIGHT,
            HAlign::Center => {}
        }
        let room = ui::shadow_room(&self.cfg) + if which == Which::Center { 4.0 } else { 0.0 };
        let (surf_w, _) = ui::column(pos.h(), width, &margin, room);
        layer.set_anchor(anchor);
        layer.set_size(surf_w.ceil() as u32, 0);
        layer.set_exclusive_zone(0);
        let kb = match which {
            Which::Popup => KeyboardInteractivity::None,
            Which::Center => match self.cfg.center.keyboard {
                KeyboardMode::None => KeyboardInteractivity::None,
                KeyboardMode::OnDemand => KeyboardInteractivity::OnDemand,
                KeyboardMode::Exclusive => KeyboardInteractivity::Exclusive,
            },
        };
        layer.set_keyboard_interactivity(kb);
        let wl = layer.wl_surface().clone();
        let frac = self.frac_mgr.as_ref().map(|m| m.get_fractional_scale(&wl, qh, FracData(wl.id())));
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&wl, qh, ProtoData));
        let blur = (self.cfg.general.blur)
            .then(|| self.blur_mgr.as_ref().map(|m| m.get_background_effect(&wl, qh, ProtoData)))
            .flatten();
        let empty = Region::new(&self.compositor).ok();
        if let Some(r) = &empty {
            wl.set_input_region(Some(r.wl_region()));
        }
        layer.commit();
        let pool = SlotPool::new((surf_w as usize * 4) * 1200 * 4, &self.shm).expect("shm pool");
        let int_scale = self.outputs.outputs().filter_map(|o| self.outputs.info(&o)).map(|i| i.scale_factor).max().unwrap_or(1);
        Surf {
            layer,
            pool,
            viewport,
            frac,
            blur,
            scale120: (int_scale.max(1) * 120) as u32,
            int_scale: int_scale.max(1),
            width: surf_w.ceil() as u32,
            height: 0,
            configured: false,
            frame_pending: false,
            dirty: true,
            hits: Vec::new(),
            input: Vec::new(),
            last_frame: Instant::now(),
            list_layer: None,
        }
    }

    fn mark_dirty(&mut self, which: Which) {
        let Some(s) = self.surf_mut(which) else { return };
        s.dirty = true;
        if s.configured && !s.frame_pending {
            self.draw(which);
        }
    }

    fn draw(&mut self, which: Which) {
        let now = Instant::now();
        let Some(mut surf) = (match which {
            Which::Popup => self.popup.take(),
            Which::Center => self.center.take(),
        }) else {
            return;
        };
        if !surf.configured || surf.width == 0 || surf.height == 0 {
            self.put(which, surf);
            return;
        }
        let scale = surf.scale();
        let (lw, lh) = (surf.width as f32, surf.height as f32);
        let pw = (lw * scale).round() as u32;
        let ph = (lh * scale).round() as u32;
        let format = if self.abgr { wl_shm::Format::Abgr8888 } else { wl_shm::Format::Argb8888 };
        let (buffer, canvas) = match surf.pool.create_buffer(pw as i32, ph as i32, pw as i32 * 4, format) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("notification-daemon: buffer: {e}");
                self.put(which, surf);
                return;
            }
        };
        canvas.fill(0);
        let dt = (now - surf.last_frame).as_secs_f32().min(0.1);
        surf.last_frame = now;
        surf.hits.clear();
        let mut input: Vec<R> = Vec::new();
        let mut animating = false;
        let mut remove_surface = false;
        {
            let pm = PixmapMut::from_bytes(canvas, pw, ph).unwrap();
            let mut c = Canvas::new(pm, scale);
            match which {
                Which::Popup => {
                    animating = ui::paint_popups(
                        &mut c,
                        &mut self.gfx,
                        &mut self.icons,
                        &self.cfg,
                        &mut self.entries,
                        lh,
                        self.hover.as_ref(),
                        &mut surf.hits,
                        now,
                        dt,
                    );
                    input.extend(surf.hits.iter().filter(|(_, h)| matches!(h, Hit::Card(_))).map(|(r, _)| *r));
                }
                Which::Center => {
                    let dur = self.cfg.general.transition_ms.max(1) as f32 / 1000.0;
                    let (alpha, dy) = match self.center_anim {
                        Some((start, opening)) => {
                            let t = (now - start).as_secs_f32() / dur;
                            if t >= 1.0 {
                                self.center_anim = None;
                                if !opening {
                                    remove_surface = true;
                                }
                                if opening { (1.0, 0.0) } else { (0.0, 0.0) }
                            } else {
                                animating = true;
                                let e = ui::ease_out(t);
                                if opening { (e, (1.0 - e) * -10.0) } else { (1.0 - e, e * -10.0) }
                            }
                        }
                        None => (1.0, 0.0),
                    };
                    c.alpha = alpha;
                    c.oy = dy;
                    let out = ui::paint_center(
                        &mut c,
                        &mut self.gfx,
                        &mut self.icons,
                        &self.cfg,
                        &mut self.entries,
                        self.dnd,
                        self.scroll,
                        (lw, lh),
                        self.hover.as_ref(),
                        &mut surf.hits,
                        &mut surf.list_layer,
                    );
                    self.max_scroll = out.max_scroll;
                    self.scroll = self.scroll.clamp(0.0, out.max_scroll);
                    if self.center_open {
                        input.push(out.panel);
                    }
                }
            }
        }
        if !self.abgr {
            for px in canvas.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        if remove_surface {
            drop(buffer);
            drop(surf);
            self.after_center_closed();
            return;
        }
        let wl = surf.layer.wl_surface().clone();
        wl.damage_buffer(0, 0, pw as i32, ph as i32);
        match &surf.viewport {
            Some(v) => v.set_destination(surf.width as i32, surf.height as i32),
            None => wl.set_buffer_scale(surf.int_scale),
        }
        if input != surf.input {
            if let Ok(region) = Region::new(&self.compositor) {
                for r in &input {
                    region.add(r.x.floor() as i32, r.y.floor() as i32, r.w.ceil() as i32 + 1, r.h.ceil() as i32 + 1);
                }
                wl.set_input_region(Some(region.wl_region()));
                if let Some(b) = &surf.blur
                    && self.blur_supported
                {
                    let blur_region = Region::new(&self.compositor).ok();
                    if let Some(br) = &blur_region {
                        let rad = if which == Which::Center { self.cfg.theme.center_radius } else { self.cfg.theme.card_radius };
                        for r in &input {
                            add_rounded(br, *r, rad);
                        }
                        b.set_blur_region(Some(br.wl_region()));
                    }
                }
            }
            surf.input = input;
        }
        wl.frame(&self.qh, FrameCallbackData(wl.clone()));
        surf.frame_pending = true;
        surf.dirty = animating;
        if let Err(e) = buffer.attach_to(&wl) {
            eprintln!("notification-daemon: attach: {e}");
        }
        surf.layer.commit();
        self.put(which, surf);

        if which == Which::Popup {
            self.reap_popups(now);
        }
    }

    fn put(&mut self, which: Which, s: Surf) {
        match which {
            Which::Popup => self.popup = Some(s),
            Which::Center => self.center = Some(s),
        }
    }

    fn reap_popups(&mut self, now: Instant) {
        let dur = Duration::from_millis(self.cfg.general.transition_ms as u64);
        let mut closed = Vec::new();
        self.entries.retain_mut(|e| {
            if let Some(p) = &e.popup
                && let Some(l) = p.leaving
                && now - l >= dur
            {
                e.popup = None;
                if !e.in_center {
                    closed.push(e.id());
                    return false;
                }
            }
            true
        });
        for id in closed {
            self.dbus.closed(id, 1);
        }
        let any = self.entries.iter().any(|e| e.popup.is_some());
        if !any && self.popup.is_some() {
            self.popup = None;
            if self.ptr_on == Some(Which::Popup) {
                self.ptr_on = None;
                self.hover = None;
            }
        }
        self.publish();
    }

    fn ensure_popup_surface(&mut self) {
        if self.popup.is_none() && self.entries.iter().any(|e| e.popup.is_some()) {
            let s = self.create_surf(Which::Popup);
            self.popup = Some(s);
        } else {
            self.mark_dirty(Which::Popup);
        }
    }

    fn publish(&mut self) {
        let count = self.entries.iter().filter(|e| e.in_center).count();
        self.dbus.set_state(Shared { count, dnd: self.dnd, visible: self.center_open });
    }

    fn timeout_for(&self, n: &Notification, rule_timeout: Option<u32>) -> Option<Duration> {
        let g = &self.cfg.general;
        let secs = if let Some(t) = rule_timeout {
            t
        } else if g.respect_app_timeout && n.expire_timeout > 0 {
            return Some(Duration::from_millis(n.expire_timeout as u64));
        } else if g.respect_app_timeout && n.expire_timeout == 0 {
            0
        } else {
            match n.urgency {
                Urgency::Low => g.timeout_low,
                Urgency::Normal => g.timeout,
                Urgency::Critical => g.timeout_critical,
            }
        };
        (secs > 0).then(|| Duration::from_secs(secs as u64))
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Notify(n) => self.notify(*n),
            Msg::Close(id) => self.remove(id, 3),
            Msg::Ctl(c) => self.control(c),
        }
    }

    fn notify(&mut self, n: Notification) {
        let mut state = if n.transient { RuleState::Transient } else { RuleState::Normal };
        let mut rule_timeout = None;
        let mut sound = self.cfg.sound.file.clone();
        let mut skip_if_focused = false;
        for rule in &self.cfg.rules {
            let m = |re: &Option<regex::Regex>, s: &str| re.as_ref().is_none_or(|r| r.is_match(s));
            if m(&rule.app_name, &n.app_name)
                && m(&rule.summary, &n.summary)
                && m(&rule.body, &n.body)
                && m(&rule.category, n.category.as_deref().unwrap_or(""))
                && m(&rule.desktop_entry, n.desktop_entry.as_deref().unwrap_or(""))
                && rule.urgency.as_deref().is_none_or(|u| u.eq_ignore_ascii_case(n.urgency.name()))
            {
                if let Some(s) = rule.state {
                    state = s;
                }
                skip_if_focused |= rule.skip_if_focused;
                if rule.timeout.is_some() {
                    rule_timeout = rule.timeout;
                }
                if let Some(cmd) = &rule.exec {
                    exec::run_hook(cmd, &n, None);
                }
                match &rule.sound {
                    Some(RuleSound::Enabled(false)) => sound = None,
                    Some(RuleSound::Enabled(true)) => sound = sound.or_else(|| self.cfg.sound.file.clone()),
                    Some(RuleSound::File(f)) => sound = Some(f.clone()),
                    None => {}
                }
            }
        }
        if state == RuleState::Ignored {
            return;
        }
        if skip_if_focused && exec::sender_focused(&n) {
            if state == RuleState::Transient {
                return;
            }
            state = RuleState::Muted;
        }
        if let Some(cmd) = &self.cfg.general.on_receive {
            exec::run_hook(cmd, &n, None);
        }

        let duration = self.timeout_for(&n, rule_timeout);
        let prev = self.entries.iter().position(|e| {
            e.id() == n.id
                || (n.stack_tag.is_some() && e.n.stack_tag == n.stack_tag && e.n.app_name == n.app_name)
        });
        let old_popup = prev.and_then(|i| {
            let old = self.entries.remove(i);
            if old.id() != n.id {
                self.dbus.closed(old.id(), 3);
            }
            old.popup.filter(|p| p.leaving.is_none())
        });

        let mut e = Entry::new(n, &self.cfg);
        e.in_center = state != RuleState::Transient;
        let critical = e.n.urgency == Urgency::Critical;
        let dnd_allows = !self.dnd || (critical && self.cfg.general.dnd_allows_critical);
        let wants_popup = state != RuleState::Muted && !self.center_open && dnd_allows;
        if let Some(file) = sound.filter(|_| state != RuleState::Muted && dnd_allows) {
            let cooldown = Duration::from_secs_f32(self.cfg.sound.cooldown.max(0.0));
            if self.last_sound.is_none_or(|t| t.elapsed() >= cooldown) {
                self.last_sound = Some(Instant::now());
                exec::play_sound(&file, self.cfg.sound.device.as_deref());
            }
        }
        if wants_popup {
            let mut p = PopupState::new(duration);
            if let Some(old) = old_popup {
                p.shown_at = old.shown_at;
                p.y = old.y;
            }
            e.popup = Some(p);
        }
        if !e.in_center && e.popup.is_none() {
            return;
        }
        self.entries.push(e);

        let max = self.cfg.general.max_history.max(1);
        let in_center = self.entries.iter().filter(|e| e.in_center).count();
        if in_center > max {
            let mut excess = in_center - max;
            let mut closed = Vec::new();
            self.entries.retain(|e| {
                if excess > 0 && e.in_center && e.popup.is_none() {
                    excess -= 1;
                    closed.push(e.id());
                    false
                } else {
                    true
                }
            });
            for id in closed {
                self.dbus.closed(id, 1);
            }
        }

        self.limit_popups();
        self.arm_timers();
        self.ensure_popup_surface();
        self.mark_dirty(Which::Center);
        self.publish();
    }

    /// Keeps at most `max_visible` popups on screen by retiring the oldest.
    fn limit_popups(&mut self) {
        let max = self.cfg.popup.max_visible.max(1);
        let active: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.entries[i].popup.as_ref().is_some_and(|p| p.leaving.is_none()))
            .collect();
        if active.len() > max {
            let now = Instant::now();
            for &i in &active[..active.len() - max] {
                if let Some(p) = &mut self.entries[i].popup {
                    p.leaving = Some(now);
                }
            }
        }
    }

    fn arm_timers(&mut self) {
        let now = Instant::now();
        let paused = self.ptr_on == Some(Which::Popup);
        for e in &mut self.entries {
            if let Some(p) = &mut e.popup {
                if p.leaving.is_some() {
                    continue;
                }
                if paused {
                    if let (Some(exp), Some(_)) = (p.expires, p.duration) {
                        p.duration = Some(exp.saturating_duration_since(now).max(Duration::from_millis(1500)));
                    }
                    p.expires = None;
                } else if p.expires.is_none()
                    && let Some(d) = p.duration
                {
                    p.expires = Some(now + d);
                }
            }
        }
        self.reschedule();
    }

    fn reschedule(&mut self) {
        if let Some(t) = self.timer.take() {
            self.handle.remove(t);
        }
        let mut next: Option<Instant> = self
            .entries
            .iter()
            .filter_map(|e| e.popup.as_ref())
            .filter(|p| p.leaving.is_none())
            .filter_map(|p| p.expires)
            .min();
        if self.center_open {
            let refresh = Instant::now() + Duration::from_secs(30);
            next = Some(next.map_or(refresh, |n| n.min(refresh)));
        }
        if let Some(at) = next {
            let token = self.handle.insert_source(Timer::from_deadline(at), |_, _, app: &mut App| {
                app.timer = None;
                app.on_timer();
                TimeoutAction::Drop
            });
            self.timer = token.ok();
        }
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        let mut any = false;
        for e in &mut self.entries {
            if let Some(p) = &mut e.popup
                && p.leaving.is_none()
                && p.expires.is_some_and(|x| x <= now)
            {
                p.leaving = Some(now);
                any = true;
            }
        }
        if any {
            self.mark_dirty(Which::Popup);
        }
        if self.center_open {
            self.mark_dirty(Which::Center);
        }
        self.reschedule();
    }

    fn find(&self, id: u32) -> Option<usize> {
        self.entries.iter().position(|e| e.id() == id)
    }

    fn remove(&mut self, id: u32, reason: u32) {
        let Some(i) = self.find(id) else { return };
        let e = &mut self.entries[i];
        e.in_center = false;
        let has_surface = self.popup.is_some();
        match &mut e.popup {
            Some(p) if p.leaving.is_none() && has_surface => p.leaving = Some(Instant::now()),
            Some(p) if p.leaving.is_some() && has_surface => {}
            _ => {
                self.entries.remove(i);
            }
        }
        self.dbus.closed(id, reason);
        self.mark_dirty(Which::Popup);
        self.mark_dirty(Which::Center);
        self.publish();
        self.reschedule();
    }

    fn invoke(&mut self, id: u32, key: &str) {
        let Some(i) = self.find(id) else { return };
        let n = &self.entries[i].n;
        if key == "default" && self.cfg.general.focus_app_on_click {
            exec::focus_app(n);
        }
        if let Some(cmd) = &self.cfg.general.on_action {
            exec::run_hook(cmd, n, Some(key));
        }
        let resident = n.resident;
        self.dbus.action(id, key);
        if !resident {
            self.remove(id, 2);
        }
        if self.cfg.general.hide_center_on_action {
            self.set_center(false);
        }
    }

    fn click_card(&mut self, id: u32) {
        let Some(i) = self.find(id) else { return };
        if self.entries[i].n.default_action().is_some() {
            self.invoke(id, "default");
        } else {
            let n = &self.entries[i].n;
            if self.cfg.general.focus_app_on_click {
                exec::focus_app(n);
            }
            if let Some(cmd) = &self.cfg.general.on_action {
                exec::run_hook(cmd, n, Some("default"));
            }
            self.remove(id, 2);
            if self.cfg.general.hide_center_on_action {
                self.set_center(false);
            }
        }
    }

    fn clear_all(&mut self) {
        let ids: Vec<u32> = self.entries.iter().filter(|e| e.in_center || e.popup.is_some()).map(|e| e.id()).collect();
        for id in ids {
            self.remove(id, 2);
        }
        self.scroll = 0.0;
    }

    fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
        let _ = std::fs::create_dir_all(config::state_dir());
        let _ = std::fs::write(dnd_file(), if on { "1" } else { "0" });
        if on {
            let now = Instant::now();
            for e in &mut self.entries {
                if let Some(p) = &mut e.popup
                    && p.leaving.is_none()
                    && !(e.n.urgency == Urgency::Critical && self.cfg.general.dnd_allows_critical)
                {
                    p.leaving = Some(now);
                }
            }
            self.mark_dirty(Which::Popup);
        }
        self.mark_dirty(Which::Center);
        self.publish();
    }

    fn set_center(&mut self, open: bool) {
        if open == self.center_open {
            return;
        }
        self.center_open = open;
        let now = Instant::now();
        if open {
            self.scroll = 0.0;
            for e in &mut self.entries {
                if let Some(p) = &mut e.popup
                    && p.leaving.is_none()
                {
                    p.leaving = Some(now);
                }
            }
            self.mark_dirty(Which::Popup);
            self.catcher = self.create_catcher();
            if self.catcher.is_some() || self.center.is_none() {
                self.center = None;
                let s = self.create_surf(Which::Center);
                self.center = Some(s);
            }
            self.center_anim = Some((now, true));
        } else {
            self.catcher = None;
            self.center_anim = Some((now, false));
            self.kb_focus = false;
        }
        self.mark_dirty(Which::Center);
        self.reschedule();
        self.publish();
    }

    fn create_catcher(&mut self) -> Option<Catcher> {
        let viewport_mgr = self.viewporter.as_ref()?;
        let surface = self.compositor.create_surface(&self.qh);
        let layer_kind = match self.cfg.general.layer {
            LayerKind::Top => Layer::Top,
            LayerKind::Overlay => Layer::Overlay,
        };
        let layer = self.layer_shell.create_layer_surface(&self.qh, surface, layer_kind, Some("notification-daemon-catcher"), None);
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let viewport = viewport_mgr.get_viewport(layer.wl_surface(), &self.qh, ProtoData);
        layer.commit();
        let pool = SlotPool::new(64, &self.shm).ok()?;
        Some(Catcher { layer, viewport, pool })
    }

    fn after_center_closed(&mut self) {
        if self.ptr_on == Some(Which::Center) {
            self.ptr_on = None;
            self.hover = None;
        }
        self.gfx.trim();
    }

    fn control(&mut self, c: Ctl) {
        match c {
            Ctl::Toggle => self.set_center(!self.center_open),
            Ctl::Open => self.set_center(true),
            Ctl::Close => self.set_center(false),
            Ctl::Dnd(v) => self.set_dnd(v.unwrap_or(!self.dnd)),
            Ctl::ClearAll => self.clear_all(),
            Ctl::DismissLatest => {
                if let Some(id) = self.entries.iter().rev().find(|e| e.in_center || e.popup.is_some()).map(|e| e.id()) {
                    self.remove(id, 2);
                }
            }
            Ctl::Reload => match config::load(self.cfg_path.as_deref()) {
                Ok(cfg) => {
                    let theme_changed = cfg.general.icon_theme != self.cfg.general.icon_theme;
                    self.cfg = cfg;
                    if theme_changed {
                        self.icons = Icons::new(self.cfg.general.icon_theme.as_deref());
                    }
                    for e in &mut self.entries {
                        e.invalidate();
                    }
                    self.gfx.trim();
                    self.popup = None;
                    let reopen = self.center_open;
                    self.center = None;
                    self.center_open = false;
                    self.center_anim = None;
                    if reopen {
                        self.set_center(true);
                    }
                    self.ensure_popup_surface();
                    eprintln!("notification-daemon: config reloaded");
                }
                Err(e) => eprintln!("notification-daemon: reload failed: {e}"),
            },
        }
    }

    fn hit_at(&self, which: Which, x: f32, y: f32) -> Option<Hit> {
        let s = self.surf(which)?;
        let mut best: Option<&Hit> = None;
        for (r, h) in s.hits.iter() {
            if r.contains(x, y) {
                // later (inner) hits override earlier containers
                best = Some(h);
            }
        }
        best.cloned()
    }

    fn update_hover(&mut self) {
        let new = self.ptr_on.and_then(|w| self.hit_at(w, self.ptr_pos.0, self.ptr_pos.1));
        if new != self.hover {
            self.hover = new;
            if let Some(w) = self.ptr_on {
                self.mark_dirty(w);
            }
        }
        let shape = match &self.hover {
            Some(h) if h.is_button() => Shape::Pointer,
            _ => Shape::Default,
        };
        if self.cursor_shape != Some(shape)
            && let Some(dev) = &self.cursor_dev
        {
            dev.set_shape(self.cursor_serial, shape);
            self.cursor_shape = Some(shape);
        }
    }

    fn activate(&mut self, hit: Hit, button: u32) {
        const BTN_LEFT: u32 = 0x110;
        match hit {
            Hit::Card(id) if button == BTN_LEFT => self.click_card(id),
            Hit::Card(id) => self.remove(id, 2),
            Hit::Close(id) => self.remove(id, 2),
            Hit::Action(id, key) => self.invoke(id, &key),
            Hit::Otp(id) => {
                if let Some(code) = self.find(id).and_then(|i| self.entries[i].otp.clone()) {
                    exec::copy(code);
                }
                self.remove(id, 2);
            }
            Hit::ClearAll => self.clear_all(),
            Hit::Dnd => self.set_dnd(!self.dnd),
            Hit::List => {}
        }
    }
}

fn add_rounded(region: &Region, r: R, rad: f32) {
    let (x, y, w, h) = (r.x.round() as i32, r.y.round() as i32, r.w.round() as i32, r.h.round() as i32);
    let rad = (rad.round() as i32).min(w / 2).min(h / 2).max(0);
    region.add(x, y + rad, w, h - 2 * rad);
    for i in 0..rad {
        let dy = rad as f32 - i as f32 - 0.5;
        let inset = rad - ((rad * rad) as f32 - dy * dy).max(0.0).sqrt().round() as i32;
        region.add(x + inset, y + i, w - 2 * inset, 1);
        region.add(x + inset, y + h - 1 - i, w - 2 * inset, 1);
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        let Some(which) = self.which_for(surface) else { return };
        let has_frac = self.frac_mgr.is_some();
        if let Some(s) = self.surf_mut(which)
            && s.int_scale != factor
        {
            s.int_scale = factor;
            if !has_frac {
                s.scale120 = factor as u32 * 120;
                s.list_layer = None;
            }
            self.mark_dirty(which);
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        let Some(which) = self.which_for(surface) else { return };
        let dirty = match self.surf_mut(which) {
            Some(s) => {
                s.frame_pending = false;
                s.dirty
            }
            None => false,
        };
        if dirty {
            self.draw(which);
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.catcher.as_ref().is_some_and(|c| &c.layer == layer) {
            self.catcher = None;
        }
        if self.popup.as_ref().is_some_and(|s| &s.layer == layer) {
            self.popup = None;
        }
        if self.center.as_ref().is_some_and(|s| &s.layer == layer) {
            self.center = None;
            self.center_open = false;
            self.center_anim = None;
            self.publish();
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, cfg: LayerSurfaceConfigure, _: u32) {
        if let Some(c) = self.catcher.as_mut()
            && &c.layer == layer
        {
            let (w, h) = cfg.new_size;
            if w > 0
                && h > 0
                && let Ok((buffer, canvas)) = c.pool.create_buffer(1, 1, 4, wl_shm::Format::Argb8888)
            {
                canvas.fill(0);
                let wl = c.layer.wl_surface();
                c.viewport.set_destination(w as i32, h as i32);
                wl.damage_buffer(0, 0, 1, 1);
                let _ = buffer.attach_to(wl);
                c.layer.commit();
            }
            return;
        }
        let Some(which) = self.which_for(layer.wl_surface()) else { return };
        if let Some(s) = self.surf_mut(which) {
            if cfg.new_size.0 > 0 {
                s.width = cfg.new_size.0;
            }
            s.height = cfg.new_size.1;
            s.configured = true;
            s.frame_pending = false;
            s.list_layer = None;
        }
        self.mark_dirty(which);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            if let Ok(p) = self.seats.get_pointer(qh, &seat) {
                self.cursor_dev = self.cursor_mgr.as_ref().map(|m| m.get_shape_device(&p, qh));
                self.pointer = Some(p);
            }
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if cap == Capability::Pointer {
            if let Some(d) = self.cursor_dev.take() {
                d.destroy();
            }
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, surface: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {
        if self.which_for(surface) == Some(Which::Center) {
            self.kb_focus = true;
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, surface: &wl_surface::WlSurface, _: u32) {
        if self.which_for(surface) == Some(Which::Center) && self.kb_focus {
            self.kb_focus = false;
            if self.cfg.center.close_on_focus_loss {
                self.set_center(false);
            }
        }
    }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, ev: KeyEvent) {
        if !self.center_open {
            return;
        }
        match ev.keysym {
            Keysym::Escape | Keysym::q => self.set_center(false),
            Keysym::d | Keysym::D => self.set_dnd(!self.dnd),
            Keysym::C | Keysym::c => self.clear_all(),
            Keysym::Delete | Keysym::BackSpace => self.control(Ctl::DismissLatest),
            Keysym::Down | Keysym::j => self.scroll_by(60.0),
            Keysym::Up | Keysym::k => self.scroll_by(-60.0),
            _ => {}
        }
    }

    fn repeat_key(&mut self, c: &Connection, q: &QueueHandle<Self>, k: &wl_keyboard::WlKeyboard, s: u32, ev: KeyEvent) {
        if matches!(ev.keysym, Keysym::Down | Keysym::Up | Keysym::j | Keysym::k) {
            self.press_key(c, q, k, s, ev);
        }
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}

    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: RawModifiers, _: u32) {}
}

impl App {
    fn scroll_by(&mut self, d: f32) {
        let s = (self.scroll + d).clamp(0.0, self.max_scroll);
        if s != self.scroll {
            self.scroll = s;
            self.mark_dirty(Which::Center);
            self.update_hover();
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            if self.catcher.as_ref().is_some_and(|c| c.layer.wl_surface() == &ev.surface) {
                match ev.kind {
                    PointerEventKind::Enter { serial } => {
                        self.ptr_on = None;
                        self.cursor_serial = serial;
                        if let Some(dev) = &self.cursor_dev {
                            dev.set_shape(serial, Shape::Default);
                            self.cursor_shape = Some(Shape::Default);
                        }
                    }
                    PointerEventKind::Press { .. } => self.set_center(false),
                    _ => {}
                }
                continue;
            }
            let which = self.which_for(&ev.surface);
            let pos = (ev.position.0 as f32, ev.position.1 as f32);
            match ev.kind {
                PointerEventKind::Enter { serial } => {
                    self.ptr_on = which;
                    self.ptr_pos = pos;
                    self.cursor_serial = serial;
                    self.cursor_shape = None;
                    if which == Some(Which::Popup) {
                        self.arm_timers();
                    }
                    self.update_hover();
                }
                PointerEventKind::Leave { .. } => {
                    let was = self.ptr_on.take();
                    self.pressed = None;
                    if self.hover.take().is_some()
                        && let Some(w) = was
                    {
                        self.mark_dirty(w);
                    }
                    if was == Some(Which::Popup) {
                        self.arm_timers();
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.ptr_on = which.or(self.ptr_on);
                    self.ptr_pos = pos;
                    self.update_hover();
                }
                PointerEventKind::Press { .. } => {
                    self.ptr_pos = pos;
                    if let Some(w) = which
                        && let Some(h) = self.hit_at(w, pos.0, pos.1)
                    {
                        self.pressed = Some((w, h));
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    let Some(w) = which else { continue };
                    let hit = self.hit_at(w, pos.0, pos.1);
                    if let Some((pw, ph)) = self.pressed.take()
                        && pw == w
                        && hit.as_ref() == Some(&ph)
                    {
                        self.activate(ph, button);
                        self.update_hover();
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    if which == Some(Which::Center) {
                        let d = if vertical.absolute != 0.0 { vertical.absolute } else { vertical.discrete as f64 * 30.0 };
                        self.scroll_by(d as f32);
                    }
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);

impl App {
    pub fn detect_formats(&mut self) {
        self.abgr = self.shm.formats().contains(&wl_shm::Format::Abgr8888);
    }

    pub fn shutdown(&self) {
        self.dbus.release();
    }
}
