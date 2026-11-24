mod app;
mod config;
mod dbus;
mod exec;
mod icons;
mod notification;
mod render;
mod ui;

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::reexports::{
    calloop::{EventLoop, channel},
    calloop_wayland_source::WaylandSource,
    client::{Connection, globals::registry_queue_init},
};

const USAGE: &str = "usage: notification-daemon [daemon] [--config FILE] [--replace]
       notification-daemon toggle | open | close | clear | dismiss | reload
       notification-daemon dnd [on|off|toggle]
       notification-daemon state | waybar
       notification-daemon check-config [--config FILE]
       notification-daemon preview OUT.png [--scale N] [--config FILE]";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cfg_path = take_opt(&mut args, "--config").map(PathBuf::from);
    let replace = take_flag(&mut args, "--replace");
    let cmd = args.first().cloned().unwrap_or_else(|| "daemon".into());

    let result = match cmd.as_str() {
        "daemon" => run_daemon(cfg_path, replace),
        "toggle" => call("Toggle"),
        "open" => call("Open"),
        "close" => call("Close"),
        "clear" => call("ClearAll"),
        "dismiss" => call("DismissLatest"),
        "reload" => call("Reload"),
        "dnd" => match args.get(1).map(String::as_str) {
            Some("on") => dbus::client_call("SetDnd", Some(true)).map(drop).map_err(|e| e.to_string()),
            Some("off") => dbus::client_call("SetDnd", Some(false)).map(drop).map_err(|e| e.to_string()),
            None | Some("toggle") => call("ToggleDnd"),
            Some(x) => Err(format!("dnd: unknown argument {x:?}")),
        },
        "state" => dbus::client_call("State", None).map(|s| println!("{s}")).map_err(|e| e.to_string()),
        "waybar" => dbus::waybar_stream().map_err(|e| e.to_string()),
        "check-config" => config::load(cfg_path.as_deref()).map(|_| println!("config ok")),
        "preview" => {
            let scale = take_opt(&mut args, "--scale").and_then(|s| s.parse().ok()).unwrap_or(1.5);
            match args.get(1) {
                Some(out) => preview(cfg_path, out, scale),
                None => Err("preview needs an output path".into()),
            }
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    };
    if let Err(e) = result {
        eprintln!("notification-daemon: {e}");
        std::process::exit(1);
    }
}

fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn call(method: &str) -> Result<(), String> {
    dbus::client_call(method, None).map(drop).map_err(|e| e.to_string())
}

fn run_daemon(cfg_path: Option<PathBuf>, replace: bool) -> Result<(), String> {
    let cfg = config::load(cfg_path.as_deref())?;
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
    let (globals, queue) = registry_queue_init(&conn).map_err(|e| format!("wayland registry: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, app::App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue).insert(handle.clone()).map_err(|e| e.to_string())?;

    let (tx, rx) = channel::channel();
    let dbus = dbus::Dbus::start(tx, replace).map_err(|e| format!("dbus: {e}"))?;
    let mut app = app::App::new(&globals, qh, handle.clone(), cfg, cfg_path, dbus)?;
    handle
        .insert_source(rx, |ev, _, app: &mut app::App| {
            if let channel::Event::Msg(m) = ev {
                app.handle(m);
            }
        })
        .map_err(|e| e.to_string())?;

    event_loop.dispatch(Duration::from_millis(100), &mut app).map_err(|e| e.to_string())?;
    app.detect_formats();
    loop {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            app.shutdown();
            return Err(e.to_string());
        }
    }
}

fn sample(id: u32, app: &str, icon: &str, summary: &str, body: &str, urgency: notification::Urgency) -> notification::Notification {
    notification::Notification {
        id,
        app_name: app.into(),
        app_icon: icon.into(),
        summary: summary.into(),
        body: body.into(),
        actions: Vec::new(),
        urgency,
        category: None,
        desktop_entry: None,
        image: None,
        value: None,
        stack_tag: None,
        transient: false,
        resident: false,
        expire_timeout: -1,
        sender_pid: None,
        timestamp: SystemTime::now() - Duration::from_secs(id as u64 * 200),
    }
}

/// Renders sample popups and the control center offscreen, for checking the look without a compositor.
fn preview(cfg_path: Option<PathBuf>, out: &str, scale: f32) -> Result<(), String> {
    use notification::Urgency::*;
    let cfg = config::load(cfg_path.as_deref())?;
    let mut gfx = render::Gfx::new();
    let mut icons = icons::Icons::new(cfg.general.icon_theme.as_deref());

    let mut a = sample(1, "Signal", "signal-desktop", "Alice", "Are we still on for <b>tonight</b>? I can bring the thing we talked about &amp; snacks.", Normal);
    a.actions = vec![("default".into(), "Open".into()), ("reply".into(), "Reply".into()), ("read".into(), "Mark read".into())];
    let mut b = sample(2, "Volume Control", "audio-volume-high", "Volume", "", Low);
    b.value = Some(64);
    let c = sample(3, "Battery", "battery-caution", "Battery low", "5% remaining. Plug in your charger.", Critical);
    let d = sample(4, "Firefox", "firefox", "Your verification code", "Use 482913 to sign in. It expires in 10 minutes.", Normal);
    let e = sample(5, "kitty", "kitty", "Build finished", "cargo build --release completed in 41.2s", Normal);

    let mut entries: Vec<ui::Entry> = [a, b, c, d, e]
        .into_iter()
        .map(|n| {
            let mut e = ui::Entry::new(n, &cfg);
            e.in_center = true;
            e
        })
        .collect();
    entries.reverse();

    let (h, gap) = (760.0f32, 40.0f32);
    let room = ui::shadow_room(&cfg);
    let pop_w = ui::column(cfg.popup.position.h(), cfg.popup.width, &cfg.popup.margin, room).0;
    let cc_w = ui::column(cfg.center.position.h(), cfg.center.width, &cfg.center.margin, room + 4.0).0;
    let total_w = pop_w + gap + cc_w * 2.0 + gap;
    let mut pm = resvg::tiny_skia::Pixmap::new((total_w * scale) as u32, (h * scale) as u32).ok_or("bad size")?;
    pm.fill(resvg::tiny_skia::Color::from_rgba8(0x1d, 0x22, 0x2b, 255));
    let mut hits = Vec::new();
    let now = Instant::now();

    {
        let mut c = render::Canvas::new(pm.as_mut(), scale);
        for e in entries.iter_mut() {
            let mut p = ui::PopupState::new(None);
            p.shown_at = Some(now - Duration::from_secs(5));
            e.popup = Some(p);
        }
        ui::paint_popups(&mut c, &mut gfx, &mut icons, &cfg, &mut entries, h, None, &mut hits, now, 1.0);
        for e in entries.iter_mut() {
            e.popup = None;
        }

        c.ox = pop_w + gap;
        let mut layer = None;
        hits.clear();
        let hover = ui::Hit::Card(4);
        ui::paint_center(&mut c, &mut gfx, &mut icons, &cfg, &mut entries, false, 0.0, (cc_w, h), Some(&hover), &mut hits, &mut layer);

        c.ox = pop_w + gap + cc_w + gap;
        let mut empty: Vec<ui::Entry> = Vec::new();
        let mut layer = None;
        ui::paint_center(&mut c, &mut gfx, &mut icons, &cfg, &mut empty, true, 0.0, (cc_w, h), None, &mut hits, &mut layer);
    }
    pm.save_png(out).map_err(|e| e.to_string())?;
    println!("wrote {out}");
    Ok(())
}
