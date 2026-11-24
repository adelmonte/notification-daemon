# notification-daemon

A small, fast notification daemon and notification center for Wayland compositors with layer-shell (niri, Sway, Hyprland, river, ...). Written in Rust, with no GTK and no GPU context: everything is drawn on the CPU with [tiny-skia](https://github.com/linebender/tiny-skia) and [cosmic-text](https://github.com/pop-os/cosmic-text).

It started as a replacement for [SwayNotificationCenter](https://github.com/ErikReider/SwayNotificationCenter) and looks and works much the same, with more control over placement and a lot less memory (about 20 MB resident instead of about 100).

- Popups with icons, images, markup (`<b>`, `<i>`, links), progress bars and action buttons
- A notification center with history, Clear All and a Do Not Disturb toggle
- A one-click "Copy code" button on 2FA/verification notifications
- Click a notification to focus the app that sent it (niri)
- Nine screen positions and per-side margins for both popups and the center; fit-to-content, fixed or full height
- Smooth slide/fade animations, hover to pause timeouts, scrolling history
- Rules to mute, hide, make transient or retime notifications by app, summary, body, category or urgency
- Hooks that run a command on receive or on action
- Notification sounds, silenced by Do Not Disturb, with per-rule overrides
- Waybar module output compatible with `swaync-client -swb`
- Crisp at fractional scales (`wp_fractional_scale_v1` + `wp_viewporter`)
- Optional compositor blur behind cards (`ext-background-effect-v1`)
- Do Not Disturb persists across restarts

## Install

### Arch Linux

A PKGBUILD is in [`packaging/`](packaging/PKGBUILD):

```sh
git clone https://github.com/adelmonte/notification-daemon
cd notification-daemon/packaging
makepkg -si
```

Note: Arch's `extra` repo has an unrelated GNOME package also called `notification-daemon`.

### From source

Needs Rust 1.85+ and `libxkbcommon`.

```sh
cargo build --release
install -Dm755 target/release/notification-daemon ~/.local/bin/notification-daemon
```

Optional runtime tools: `wl-copy` (from wl-clipboard) for the copy-code button, and niri for focus-on-click.

## Running

Stop any other notification daemon (swaync, mako, dunst, ...) first, since only one can own `org.freedesktop.Notifications`. Then start it with your compositor.

niri:

```kdl
spawn-at-startup "notification-daemon"
```

Sway / Hyprland:

```
exec notification-daemon
```

`notification-daemon --replace` takes the bus name over from a daemon that allows replacement.

## Commands

```
notification-daemon                 run the daemon
notification-daemon toggle          open/close the notification center
notification-daemon open | close
notification-daemon dnd [on|off|toggle]
notification-daemon clear           dismiss everything
notification-daemon dismiss         dismiss the newest notification
notification-daemon reload          re-read the config
notification-daemon state           print {"count", "dnd", "visible"} as JSON
notification-daemon waybar          stream waybar JSON (one line per change)
notification-daemon check-config    validate the config file
notification-daemon preview out.png render sample popups and the center to a PNG
```

All commands accept `--config FILE`.

## Waybar

```json
"custom/notifications": {
    "format": "{icon}",
    "format-icons": {
        "notification": "󱅫",
        "none": "󰂚",
        "dnd-notification": "󱅫",
        "dnd-none": "󰂛"
    },
    "return-type": "json",
    "exec-if": "which notification-daemon",
    "exec": "notification-daemon waybar",
    "on-click": "notification-daemon toggle",
    "on-click-right": "notification-daemon dnd",
    "escape": true,
    "tooltip": true
}
```

`text` is the notification count (empty when zero). `class` is the same as `alt`, plus `cc-open` while the center is open.

## Using the center

- Click a notification to run its default action (and focus the app on niri); right- or middle-click to dismiss it.
- Hover a card for its close button. Hovering any popup pauses its timeout.
- Click anywhere outside the center, or press Esc, to close it.
- Keys while it has focus: `d` toggle DND, `c` clear all, `Delete` dismiss newest, arrows or `j`/`k` scroll.

## Configuration

`~/.config/notification-daemon/config.toml`. Every key is optional. [`config.example.toml`](config.example.toml) lists them all with their defaults. Apply changes with `notification-daemon reload`.

```toml
[popup]
position = "top-right"   # top-left, top-center, top-right, center-left, center, center-right, bottom-*
width = 340
margin = [8, 12]         # 1-4 values, CSS order, or { top = 8, right = 12 }

[center]
position = "top-right"
height = 0               # 0 = fit content
fill-height = false

[theme]
font = "JetBrainsMono Nerd Font"
accent = "#88c0d0"
card-bg = ["#3b4252f2", "#2e3440f2"]   # gradient start, end

# Volume OSD notifications: show the popup but keep them out of history
[[rule]]
app-name = "^Volume"
state = "transient"      # normal | transient | muted (history only) | ignored
timeout = 2
```

The default theme is Nord.

### Sounds

```toml
[sound]
file = "~/sounds/ping.oga"   # played for each notification; no sound when unset
# device = "my-sink"         # sink name, default sink when unset
cooldown = 1                 # seconds; sounds closer together than this are skipped

[[rule]]
app-name = "^Volume Control$"
sound = false                # or a file path to use a different sound
```

Sounds play through `paplay` (with `media.role=event`). They are skipped for `muted` and `ignored` notifications and during Do Not Disturb, except critical ones when `dnd-allows-critical` is on.

### Hooks

`on-receive`, `on-action` and per-rule `exec` run through `sh -c` with these variables set:
`NOTIFICATION_ID`, `NOTIFICATION_APP_NAME`, `NOTIFICATION_SUMMARY`, `NOTIFICATION_BODY`, `NOTIFICATION_URGENCY`, `NOTIFICATION_CATEGORY`, `NOTIFICATION_DESKTOP_ENTRY`, `NOTIFICATION_ICON`, and `NOTIFICATION_ACTION` for actions. The `SWAYNC_*` equivalents are set too, so existing swaync scripts keep working.

### Compositor rules

The layer-shell namespaces are `notification-daemon-popup`, `notification-daemon-center` and `notification-daemon-catcher` (the invisible full-screen layer that closes the center when you click outside it). For example, to blur behind popups in niri, set `blur = true` under `[general]`, or use a layer rule:

```kdl
layer-rule {
    match namespace="^notification-daemon-popup$"
    background-effect { blur true; }
}
```

## D-Bus

Implements `org.freedesktop.Notifications` (spec 1.2: actions, body markup, persistence, `x-canonical-private-synchronous`, `x-dunst-stack-tag`, `image-data`, `image-path`, `value`, `urgency`, `transient`, `resident`, `desktop-entry`).

Control interface: `org.notificationdaemon.Control` at `/org/notificationdaemon/Control`, with methods `Toggle`, `Open`, `Close`, `SetDnd(b)`, `ToggleDnd`, `ClearAll`, `DismissLatest`, `Reload`, `State() → s`, and the signal `StateChanged(s)`.

## License

GPL-3.0-or-later
