use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

use crate::notification::Notification;

fn env_for(n: &Notification, action: Option<&str>) -> Vec<(String, String)> {
    let mut v = vec![
        ("NOTIFICATION_ID".into(), n.id.to_string()),
        ("NOTIFICATION_APP_NAME".into(), n.app_name.clone()),
        ("NOTIFICATION_SUMMARY".into(), n.summary.clone()),
        ("NOTIFICATION_BODY".into(), n.body.clone()),
        ("NOTIFICATION_URGENCY".into(), n.urgency.name().into()),
        ("NOTIFICATION_CATEGORY".into(), n.category.clone().unwrap_or_default()),
        ("NOTIFICATION_DESKTOP_ENTRY".into(), n.desktop_entry.clone().unwrap_or_default()),
        ("NOTIFICATION_ICON".into(), n.app_icon.clone()),
        // swaync-compatible names so existing scripts keep working
        ("SWAYNC_ID".into(), n.id.to_string()),
        ("SWAYNC_APP_NAME".into(), n.app_name.clone()),
        ("SWAYNC_SUMMARY".into(), n.summary.clone()),
        ("SWAYNC_BODY".into(), n.body.clone()),
        ("SWAYNC_URGENCY".into(), n.urgency.name().into()),
        ("SWAYNC_CATEGORY".into(), n.category.clone().unwrap_or_default()),
        ("SWAYNC_DESKTOP_ENTRY".into(), n.desktop_entry.clone().unwrap_or_default()),
    ];
    if let Some(a) = action {
        v.push(("NOTIFICATION_ACTION".into(), a.into()));
    }
    v
}

pub fn run_hook(cmd: &str, n: &Notification, action: Option<&str>) {
    let env = env_for(n, action);
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        match Command::new("sh").arg("-c").arg(&cmd).envs(env).stdin(Stdio::null()).spawn() {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => eprintln!("notification-daemon: hook {cmd:?}: {e}"),
        }
    });
}

pub fn copy(text: String) {
    std::thread::spawn(move || {
        match Command::new("wl-copy").arg("--").arg(&text).stdin(Stdio::null()).spawn() {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => eprintln!("notification-daemon: wl-copy: {e}"),
        }
    });
}

fn niri(request: &str) -> Option<serde_json::Value> {
    let path = std::env::var_os("NIRI_SOCKET")?;
    let mut s = UnixStream::connect(path).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    s.write_all(b"\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// Focuses the niri window that most likely belongs to the notification's sender.
pub fn focus_app(n: &Notification) {
    let mut keys: Vec<String> = Vec::new();
    for k in [n.desktop_entry.as_deref(), Some(n.app_name.as_str())].into_iter().flatten() {
        let k = k.trim().trim_end_matches(".desktop").to_lowercase();
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
    }
    let pid = n.sender_pid;
    std::thread::spawn(move || {
        let Some(reply) = niri("\"Windows\"") else { return };
        let Some(windows) = reply.pointer("/Ok/Windows").and_then(|w| w.as_array()) else { return };
        let mut best: Option<(u32, u64)> = None;
        for w in windows {
            let Some(id) = w.get("id").and_then(|v| v.as_u64()) else { continue };
            let app_id = w.get("app_id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let wpid = w.get("pid").and_then(|v| v.as_u64());
            let mut score = 0;
            if pid.is_some() && wpid == pid.map(u64::from) {
                score = 4;
            }
            for k in &keys {
                if app_id == *k || app_id.rsplit('.').next() == Some(k.as_str()) {
                    score = score.max(3);
                } else if !app_id.is_empty() && (app_id.contains(k.as_str()) || k.contains(app_id.as_str())) {
                    score = score.max(2);
                }
            }
            if score > 0 && best.is_none_or(|(s, _)| score > s) {
                best = Some((score, id));
            }
        }
        if let Some((_, id)) = best {
            niri(&format!("{{\"Action\":{{\"FocusWindow\":{{\"id\":{id}}}}}}}"));
        }
    });
}
