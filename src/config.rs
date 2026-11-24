use regex::Regex;
use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub popup: Popup,
    pub center: Center,
    pub theme: Theme,
    pub sound: Sound,
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct General {
    pub timeout: u32,
    pub timeout_low: u32,
    pub timeout_critical: u32,
    pub respect_app_timeout: bool,
    pub transition_ms: u32,
    pub layer: LayerKind,
    pub dnd_allows_critical: bool,
    pub max_history: usize,
    pub focus_app_on_click: bool,
    pub hide_center_on_action: bool,
    pub otp_copy_button: bool,
    pub on_receive: Option<String>,
    pub on_action: Option<String>,
    pub icon_theme: Option<String>,
    pub blur: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            timeout: 10,
            timeout_low: 8,
            timeout_critical: 0,
            respect_app_timeout: true,
            transition_ms: 200,
            layer: LayerKind::Overlay,
            dnd_allows_critical: true,
            max_history: 100,
            focus_app_on_click: true,
            hide_center_on_action: true,
            otp_copy_button: true,
            on_receive: None,
            on_action: None,
            icon_theme: None,
            blur: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Sound {
    /// Played for every notification that shows up, unless a rule says otherwise.
    pub file: Option<String>,
    /// PulseAudio/PipeWire sink name; the default sink when unset.
    pub device: Option<String>,
    /// Seconds after a sound during which further sounds are skipped.
    pub cooldown: f32,
}

impl Default for Sound {
    fn default() -> Self {
        Self { file: None, device: None, cooldown: 1.0 }
    }
}

#[derive(Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(untagged)]
pub enum RuleSound {
    Enabled(bool),
    File(String),
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Popup {
    pub position: Position,
    pub width: f32,
    pub margin: Margin,
    pub gap: f32,
    pub max_visible: usize,
    pub summary_lines: usize,
    pub body_lines: usize,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            position: Position::TopRight,
            width: 340.0,
            margin: Margin::all(8.0),
            gap: 8.0,
            max_visible: 5,
            summary_lines: 1,
            body_lines: 3,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Center {
    pub position: Position,
    pub width: f32,
    pub margin: Margin,
    /// 0 = size to content, otherwise a fixed height in logical pixels.
    pub height: f32,
    pub min_height: f32,
    pub fill_height: bool,
    pub padding: f32,
    pub gap: f32,
    pub summary_lines: usize,
    pub body_lines: usize,
    pub keyboard: KeyboardMode,
    pub close_on_focus_loss: bool,
    pub show_title: bool,
    pub show_dnd: bool,
    pub title: String,
    pub clear_all_text: String,
    pub dnd_text: String,
    pub dnd_on_text: String,
    pub dnd_off_text: String,
    pub empty_text: String,
    pub empty_icon: String,
}

impl Default for Center {
    fn default() -> Self {
        Self {
            position: Position::TopRight,
            width: 400.0,
            margin: Margin::all(12.0),
            height: 0.0,
            min_height: 0.0,
            fill_height: false,
            padding: 20.0,
            gap: 10.0,
            summary_lines: 1,
            body_lines: 5,
            keyboard: KeyboardMode::OnDemand,
            close_on_focus_loss: false,
            show_title: true,
            show_dnd: true,
            title: "Notification Center".into(),
            clear_all_text: "Clear All".into(),
            dnd_text: "Do Not Disturb".into(),
            dnd_on_text: "On".into(),
            dnd_off_text: "Off".into(),
            empty_text: "No Notifications".into(),
            empty_icon: "\u{f009a}".into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Theme {
    pub font: String,
    pub font_size: f32,
    pub summary_size: f32,
    pub body_size: f32,
    pub time_size: f32,
    pub title_size: f32,
    pub button_size: f32,
    pub icon_size: f32,
    pub card_padding: [f32; 2],
    pub card_radius: f32,
    pub center_radius: f32,
    pub widget_radius: f32,
    pub button_radius: f32,
    pub icon_radius: f32,
    pub border_width: f32,
    pub shadow_blur: f32,
    pub shadow_offset: f32,
    pub text_shadow: bool,

    pub card_bg: [Color; 2],
    pub card_bg_hover: [Color; 2],
    pub card_bg_low: [Color; 2],
    pub card_bg_critical: [Color; 2],
    pub border: Color,
    pub border_low: Color,
    pub border_critical: Color,
    pub border_hover: Color,
    pub shadow: Color,
    pub shadow_critical: Color,

    pub text: Color,
    pub body: Color,
    pub dim: Color,
    pub accent: Color,
    pub progress: [Color; 3],
    pub trough: Color,

    pub button_bg: Color,
    pub button_bg_hover: Color,
    pub button_border: Color,
    pub button_border_hover: Color,
    pub button_text: Color,
    pub close_bg: Color,
    pub close_bg_hover: Color,

    pub center_bg: [Color; 2],
    pub center_border: Color,
    pub title_bg: [Color; 2],
    pub title_border: Color,
    pub widget_bg: Color,
    pub widget_bg_hover: Color,
    pub widget_border: Color,
    pub dnd_on: [Color; 2],
    pub dnd_on_text: Color,
    pub scrollbar: Color,
}

impl Default for Theme {
    fn default() -> Self {
        let c = Color::hex;
        Self {
            font: "JetBrainsMono Nerd Font".into(),
            font_size: 12.0,
            summary_size: 12.67,
            body_size: 11.33,
            time_size: 10.0,
            title_size: 14.67,
            button_size: 12.0,
            icon_size: 48.0,
            card_padding: [14.0, 10.0],
            card_radius: 14.0,
            center_radius: 24.0,
            widget_radius: 14.0,
            button_radius: 10.0,
            icon_radius: 8.0,
            border_width: 2.0,
            shadow_blur: 14.0,
            shadow_offset: 4.0,
            text_shadow: true,

            card_bg: [c("#3b4252f2"), c("#2e3440f2")],
            card_bg_hover: [c("#434c5ef7"), c("#3b4252f7")],
            card_bg_low: [c("#3b4252eb"), c("#2e3440eb")],
            card_bg_critical: [c("#bf616a40"), c("#3b4252f2")],
            border: c("#88c0d080"),
            border_low: c("#5e81ac80"),
            border_critical: c("#bf616a"),
            border_hover: c("#88c0d0"),
            shadow: c("#00000080"),
            shadow_critical: c("#bf616a80"),

            text: c("#eceff4"),
            body: c("#d8dee9f2"),
            dim: c("#d8dee999"),
            accent: c("#88c0d0"),
            progress: [c("#88c0d0"), c("#8fbcbb"), c("#a3be8c")],
            trough: c("#4c566a80"),

            button_bg: c("#434c5e99"),
            button_bg_hover: c("#88c0d033"),
            button_border: c("#88c0d04d"),
            button_border_hover: c("#88c0d0"),
            button_text: c("#eceff4"),
            close_bg: c("#4c566a99"),
            close_bg_hover: c("#bf616a"),

            center_bg: [c("#2e3440f5"), c("#3b4252f5")],
            center_border: c("#88c0d04d"),
            title_bg: [c("#88c0d01a"), c("#88c0d000")],
            title_border: c("#88c0d04d"),
            widget_bg: c("#434c5e66"),
            widget_bg_hover: c("#434c5e99"),
            widget_border: c("#4c566a99"),
            dnd_on: [c("#bf616a"), c("#d08770")],
            dnd_on_text: c("#2e3440"),
            scrollbar: c("#88c0d066"),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Rule {
    #[serde(deserialize_with = "regex_opt")]
    pub app_name: Option<Regex>,
    #[serde(deserialize_with = "regex_opt")]
    pub summary: Option<Regex>,
    #[serde(deserialize_with = "regex_opt")]
    pub body: Option<Regex>,
    #[serde(deserialize_with = "regex_opt")]
    pub category: Option<Regex>,
    #[serde(deserialize_with = "regex_opt")]
    pub desktop_entry: Option<Regex>,
    pub urgency: Option<String>,
    pub state: Option<RuleState>,
    pub timeout: Option<u32>,
    pub exec: Option<String>,
    pub sound: Option<RuleSound>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum RuleState {
    Normal,
    Transient,
    Muted,
    Ignored,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    Bottom,
}

impl Position {
    pub fn h(self) -> HAlign {
        use Position::*;
        match self {
            TopLeft | CenterLeft | BottomLeft => HAlign::Left,
            TopCenter | Center | BottomCenter => HAlign::Center,
            TopRight | CenterRight | BottomRight => HAlign::Right,
        }
    }

    pub fn v(self) -> VAlign {
        use Position::*;
        match self {
            TopLeft | TopCenter | TopRight => VAlign::Top,
            CenterLeft | Center | CenterRight => VAlign::Center,
            BottomLeft | BottomCenter | BottomRight => VAlign::Bottom,
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LayerKind {
    Top,
    Overlay,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum KeyboardMode {
    None,
    OnDemand,
    Exclusive,
}

#[derive(Clone, Copy)]
pub struct Margin {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Margin {
    pub const fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }
}

impl<'de> Deserialize<'de> for Margin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            All(f32),
            List(Vec<f32>),
            Sides {
                #[serde(default)]
                top: f32,
                #[serde(default)]
                right: f32,
                #[serde(default)]
                bottom: f32,
                #[serde(default)]
                left: f32,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::All(v) => Margin::all(v),
            Raw::List(v) => match v[..] {
                [a] => Margin::all(a),
                [y, x] => Margin { top: y, right: x, bottom: y, left: x },
                [t, x, b] => Margin { top: t, right: x, bottom: b, left: x },
                [t, r, b, l] => Margin { top: t, right: r, bottom: b, left: l },
                _ => return Err(serde::de::Error::custom("margin list takes 1 to 4 values")),
            },
            Raw::Sides { top, right, bottom, left } => Margin { top, right, bottom, left },
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Color { r: p(0)?, g: p(2)?, b: p(4)?, a: 255 }),
            8 => Some(Color { r: p(0)?, g: p(2)?, b: p(4)?, a: p(6)? }),
            _ => None,
        }
    }

    fn hex(s: &str) -> Self {
        Self::parse(s).unwrap()
    }

    pub fn with_alpha(self, f: f32) -> Self {
        Color { a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8, ..self }
    }

    pub fn sk(self) -> resvg::tiny_skia::Color {
        resvg::tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad color {s:?}, want #rrggbb or #rrggbbaa")))
    }
}

fn regex_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Regex>, D::Error> {
    let s = String::deserialize(d)?;
    Regex::new(&s).map(Some).map_err(serde::de::Error::custom)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            popup: Popup::default(),
            center: Center::default(),
            theme: Theme::default(),
            sound: Sound::default(),
            rules: Vec::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("notification-daemon/config.toml")
}

pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"));
    base.join("notification-daemon")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

pub fn load(path: Option<&std::path::Path>) -> Result<Config, String> {
    let path = path.map(PathBuf::from).unwrap_or_else(config_path);
    match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}
