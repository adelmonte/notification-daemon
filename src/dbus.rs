use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{DeserializeDict, OwnedValue, Type, Value};

use crate::notification::{Image, Notification, RawImage, Urgency};

pub const CTL_NAME: &str = "org.notificationdaemon.Control";
pub const CTL_PATH: &str = "/org/notificationdaemon/Control";
const FDN_NAME: &str = "org.freedesktop.Notifications";
const FDN_PATH: &str = "/org/freedesktop/Notifications";

pub enum Msg {
    Notify(Box<Notification>),
    Close(u32),
    Ctl(Ctl),
}

#[derive(Debug, Clone, Copy)]
pub enum Ctl {
    Toggle,
    Open,
    Close,
    Dnd(Option<bool>),
    ClearAll,
    DismissLatest,
    Reload,
}

#[derive(Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Shared {
    pub count: usize,
    pub dnd: bool,
    pub visible: bool,
}

#[derive(Deserialize, Type)]
struct ImageData(i32, i32, i32, bool, i32, i32, Vec<u8>);

#[derive(DeserializeDict, Type, Default)]
#[zvariant(signature = "a{sv}", rename_all = "kebab-case")]
struct Hints {
    urgency: Option<OwnedValue>,
    category: Option<OwnedValue>,
    desktop_entry: Option<OwnedValue>,
    image_data: Option<ImageData>,
    #[zvariant(rename = "image_data")]
    image_data_old: Option<ImageData>,
    #[zvariant(rename = "icon_data")]
    icon_data: Option<ImageData>,
    image_path: Option<OwnedValue>,
    #[zvariant(rename = "image_path")]
    image_path_old: Option<OwnedValue>,
    value: Option<OwnedValue>,
    transient: Option<OwnedValue>,
    resident: Option<OwnedValue>,
    #[zvariant(rename = "x-canonical-private-synchronous")]
    synchronous: Option<OwnedValue>,
    #[zvariant(rename = "x-dunst-stack-tag")]
    stack_tag: Option<OwnedValue>,
    sender_pid: Option<OwnedValue>,
}

fn v_str(v: Option<OwnedValue>) -> Option<String> {
    match v.as_deref()? {
        Value::Str(s) => Some(s.to_string()).filter(|s| !s.is_empty()),
        Value::ObjectPath(p) => Some(p.to_string()),
        Value::Value(inner) => v_str(OwnedValue::try_from(&**inner).ok()),
        other => num(other).map(|n| n.to_string()),
    }
}

fn num(v: &Value) -> Option<i64> {
    Some(match v {
        Value::U8(x) => *x as i64,
        Value::I16(x) => *x as i64,
        Value::U16(x) => *x as i64,
        Value::I32(x) => *x as i64,
        Value::U32(x) => *x as i64,
        Value::I64(x) => *x,
        Value::U64(x) => *x as i64,
        Value::F64(x) => *x as i64,
        Value::Bool(b) => *b as i64,
        Value::Str(s) => s.as_str().trim().parse().ok()?,
        _ => return None,
    })
}

fn v_num(v: Option<OwnedValue>) -> Option<i64> {
    num(v.as_deref()?)
}

fn v_bool(v: Option<OwnedValue>) -> bool {
    v_num(v).is_some_and(|n| n != 0)
}

struct Server {
    tx: Sender<Msg>,
    next_id: AtomicU32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec![
            "actions",
            "body",
            "body-markup",
            "icon-static",
            "persistence",
            "x-canonical-private-synchronous",
            "x-dunst-stack-tag",
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: Hints,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            loop {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                if id != 0 {
                    break id;
                }
            }
        };

        let image = hints
            .image_data
            .or(hints.image_data_old)
            .or(hints.icon_data)
            .map(|ImageData(width, height, rowstride, has_alpha, _bits, channels, data)| {
                Image::Raw(RawImage { width, height, rowstride, has_alpha, channels, data })
            })
            .or_else(|| v_str(hints.image_path).or_else(|| v_str(hints.image_path_old)).map(Image::Path));

        let n = Notification {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions: actions.chunks_exact(2).map(|c| (c[0].clone(), c[1].clone())).collect(),
            urgency: Urgency::from_u8(v_num(hints.urgency).unwrap_or(1).clamp(0, 2) as u8),
            category: v_str(hints.category),
            desktop_entry: v_str(hints.desktop_entry),
            image,
            value: v_num(hints.value).map(|v| v.clamp(0, 100) as i32),
            stack_tag: v_str(hints.stack_tag).or_else(|| v_str(hints.synchronous)),
            transient: v_bool(hints.transient),
            resident: v_bool(hints.resident),
            expire_timeout,
            sender_pid: v_num(hints.sender_pid).and_then(|p| u32::try_from(p).ok()),
            timestamp: SystemTime::now(),
        };
        let _ = self.tx.send(Msg::Notify(Box::new(n)));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.tx.send(Msg::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("notification-daemon", "notification-daemon", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(e: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(e: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;
}

struct Control {
    tx: Sender<Msg>,
    shared: Arc<Mutex<Shared>>,
}

#[zbus::interface(name = "org.notificationdaemon.Control")]
impl Control {
    fn toggle(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Toggle));
    }
    fn open(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Open));
    }
    fn close(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Close));
    }
    fn set_dnd(&self, on: bool) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Dnd(Some(on))));
    }
    fn toggle_dnd(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Dnd(None)));
    }
    fn clear_all(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::ClearAll));
    }
    fn dismiss_latest(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::DismissLatest));
    }
    fn reload(&self) {
        let _ = self.tx.send(Msg::Ctl(Ctl::Reload));
    }
    fn state(&self) -> String {
        serde_json::to_string(&*self.shared.lock().unwrap()).unwrap_or_default()
    }

    #[zbus(signal)]
    async fn state_changed(e: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;
}

pub struct Dbus {
    conn: Connection,
    pub shared: Arc<Mutex<Shared>>,
}

impl Dbus {
    pub fn start(tx: Sender<Msg>, replace: bool) -> zbus::Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let conn = zbus::blocking::connection::Builder::session()?
            .serve_at(FDN_PATH, Server { tx: tx.clone(), next_id: AtomicU32::new(1) })?
            .serve_at(CTL_PATH, Control { tx, shared: shared.clone() })?
            .build()?;
        use zbus::fdo::{RequestNameFlags as F, RequestNameReply};
        let mut flags = F::AllowReplacement | F::DoNotQueue;
        if replace {
            flags |= F::ReplaceExisting;
        }
        for name in [FDN_NAME, CTL_NAME] {
            match conn.request_name_with_flags(name, flags)? {
                RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
                _ => {
                    return Err(zbus::Error::Failure(format!(
                        "{name} is owned by another process (is swaync or another daemon running? try --replace)"
                    )));
                }
            }
        }
        Ok(Self { conn, shared })
    }

    pub fn closed(&self, id: u32, reason: u32) {
        let _ = self.conn.emit_signal(None::<&str>, FDN_PATH, FDN_NAME, "NotificationClosed", &(id, reason));
    }

    pub fn action(&self, id: u32, key: &str) {
        let _ = self.conn.emit_signal(None::<&str>, FDN_PATH, FDN_NAME, "ActionInvoked", &(id, key));
    }

    pub fn set_state(&self, s: Shared) {
        let mut cur = self.shared.lock().unwrap();
        if *cur == s {
            return;
        }
        *cur = s.clone();
        drop(cur);
        let json = serde_json::to_string(&s).unwrap_or_default();
        let _ = self.conn.emit_signal(None::<&str>, CTL_PATH, CTL_NAME, "StateChanged", &json);
    }

    /// Releases the bus names so another daemon can take over cleanly.
    pub fn release(&self) {
        let _ = self.conn.release_name(FDN_NAME);
        let _ = self.conn.release_name(CTL_NAME);
    }
}

pub fn client_call(method: &str, args: Option<bool>) -> zbus::Result<String> {
    let conn = Connection::session()?;
    let reply = match args {
        Some(b) => conn.call_method(Some(CTL_NAME), CTL_PATH, Some(CTL_NAME), method, &(b,))?,
        None => conn.call_method(Some(CTL_NAME), CTL_PATH, Some(CTL_NAME), method, &())?,
    };
    if method == "State" {
        return reply.body().deserialize::<String>();
    }
    Ok(String::new())
}

/// Streams waybar-compatible JSON lines (same shape as `swaync-client -swb`).
pub fn waybar_stream() -> zbus::Result<()> {
    let conn = Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(&conn, CTL_NAME, CTL_PATH, CTL_NAME)?;
    let signals = proxy.receive_signal("StateChanged")?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
    let owner_changes = dbus.receive_name_owner_changed_with_args(&[(0, CTL_NAME)])?;

    let print = |s: &Shared| {
        use std::io::Write;
        let alt = match (s.dnd, s.count > 0) {
            (false, true) => "notification",
            (false, false) => "none",
            (true, true) => "dnd-notification",
            (true, false) => "dnd-none",
        };
        let mut class = vec![alt];
        if s.visible {
            class.push("cc-open");
        }
        let tooltip = match s.count {
            0 => "No notifications".to_string(),
            1 => "1 notification".to_string(),
            n => format!("{n} notifications"),
        };
        let line = serde_json::json!({
            "text": if s.count > 0 { s.count.to_string() } else { String::new() },
            "alt": alt,
            "tooltip": tooltip,
            "class": class,
        });
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };

    let current = || -> Shared {
        proxy
            .call::<_, _, String>("State", &())
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    };
    print(&current());

    std::thread::scope(|sc| {
        sc.spawn(|| {
            for _ in owner_changes {
                print(&current());
            }
        });
        for msg in signals {
            if let Ok(json) = msg.body().deserialize::<String>()
                && let Ok(s) = serde_json::from_str::<Shared>(&json)
            {
                print(&s);
            }
        }
    });
    Ok(())
}
