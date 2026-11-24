use std::ops::Range;
use std::sync::OnceLock;
use std::time::SystemTime;

use regex::Regex;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        }
    }
}

pub struct RawImage {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub channels: i32,
    pub data: Vec<u8>,
}

pub enum Image {
    Raw(RawImage),
    Path(String),
}

pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub urgency: Urgency,
    pub category: Option<String>,
    pub desktop_entry: Option<String>,
    pub image: Option<Image>,
    pub value: Option<i32>,
    pub stack_tag: Option<String>,
    pub transient: bool,
    pub resident: bool,
    pub expire_timeout: i32,
    pub sender_pid: Option<u32>,
    pub timestamp: SystemTime,
}

impl Notification {
    pub fn default_action(&self) -> Option<&str> {
        self.actions.iter().find(|(k, _)| k == "default").map(|(k, _)| k.as_str())
    }

    pub fn visible_actions(&self) -> impl Iterator<Item = &(String, String)> {
        self.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty())
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Parses the small markup subset from the notification spec (b, i, u, a, img, br)
/// into plain text plus styled byte ranges. Anything that doesn't look like a tag is kept as text.
pub fn parse_markup(src: &str) -> (String, Vec<(Range<usize>, Style)>) {
    let mut out = String::with_capacity(src.len());
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut depth = [0u32; 3];
    let mut run_start = 0;
    let mut rest = src;

    let flush = |out: &String, start: &mut usize, style: Style, spans: &mut Vec<(Range<usize>, Style)>| {
        if out.len() > *start && style != Style::default() {
            spans.push((*start..out.len(), style));
        }
        *start = out.len();
    };

    while let Some(pos) = rest.find(['<', '&']) {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if rest.starts_with('&') {
            match decode_entity(rest) {
                Some((ch, len)) => {
                    out.push(ch);
                    rest = &rest[len..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
            continue;
        }
        let Some(end) = rest.find('>') else {
            out.push('<');
            rest = &rest[1..];
            continue;
        };
        let inner = &rest[1..end];
        let closing = inner.starts_with('/');
        let name_part = inner.strip_prefix('/').unwrap_or(inner);
        let name: String = name_part
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let known = matches!(name.as_str(), "b" | "i" | "u" | "a" | "img" | "br" | "p" | "span" | "s" | "tt" | "big" | "small" | "sup" | "sub");
        if !known || name_part.len() > 512 || inner.contains('<') {
            out.push('<');
            rest = &rest[1..];
            continue;
        }
        flush(&out, &mut run_start, style, &mut spans);
        let slot = match name.as_str() {
            "b" => Some(0),
            "i" => Some(1),
            "u" => Some(2),
            _ => None,
        };
        if let Some(s) = slot {
            if closing {
                depth[s] = depth[s].saturating_sub(1);
            } else {
                depth[s] += 1;
            }
        } else if name == "br" {
            out.push('\n');
        } else if name == "img" && !closing {
            if let Some(alt) = attr(name_part, "alt") {
                out.push_str(&alt);
            }
        }
        style = Style { bold: depth[0] > 0, italic: depth[1] > 0, underline: depth[2] > 0 };
        rest = &rest[end + 1..];
        run_start = out.len();
    }
    out.push_str(rest);
    if out.len() > run_start && style != Style::default() {
        spans.push((run_start..out.len(), style));
    }

    let trimmed_end = out.trim_end().len();
    out.truncate(trimmed_end);
    for (r, _) in spans.iter_mut() {
        r.end = r.end.min(trimmed_end);
        r.start = r.start.min(r.end);
    }
    spans.retain(|(r, _)| !r.is_empty());
    (out, spans)
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let i = tag.find(&format!("{key}="))? + key.len() + 1;
    let rest = &tag[i..];
    let q = rest.chars().next()?;
    if q == '"' || q == '\'' {
        let end = rest[1..].find(q)?;
        Some(rest[1..1 + end].to_string())
    } else {
        Some(rest.split_whitespace().next()?.to_string())
    }
}

fn decode_entity(s: &str) -> Option<(char, usize)> {
    let end = s[..s.len().min(12)].find(';')?;
    let name = &s[1..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ if name.starts_with("#x") || name.starts_with("#X") => char::from_u32(u32::from_str_radix(&name[2..], 16).ok()?)?,
        _ if name.starts_with('#') => char::from_u32(name[1..].parse().ok()?)?,
        _ => return None,
    };
    Some((ch, end + 1))
}

/// Returns a one-time code if the notification looks like a 2FA message.
pub fn find_otp(summary: &str, body: &str) -> Option<String> {
    static KEYWORDS: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    let kw = KEYWORDS.get_or_init(|| {
        Regex::new(r"(?i)\b(code|otp|passcode|verification|verify|2fa|one[- ]time|security code|login|sign[- ]in|pin)\b").unwrap()
    });
    let code = CODE.get_or_init(|| Regex::new(r"\b(\d{4,8}|\d{3}[- ]\d{3})\b").unwrap());
    let text = format!("{summary}\n{body}");
    if !kw.is_match(&text) {
        return None;
    }
    code.find(&text).map(|m| m.as_str().replace([' ', '-'], ""))
}

pub fn relative_time(ts: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(ts).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_basic() {
        let (t, s) = parse_markup("Hello <b>bold</b> &amp; <i>it</i><br/>x");
        assert_eq!(t, "Hello bold & it\nx");
        assert_eq!(s.len(), 2);
        assert_eq!(&t[s[0].0.clone()], "bold");
        assert!(s[0].1.bold);
        assert_eq!(&t[s[1].0.clone()], "it");
        assert!(s[1].1.italic);
    }

    #[test]
    fn markup_literal_lt() {
        let (t, _) = parse_markup("a < b and 3<4 <notatag>");
        assert_eq!(t, "a < b and 3<4 <notatag>");
    }

    #[test]
    fn markup_link_and_img() {
        let (t, _) = parse_markup("see <a href=\"https://x\">link</a> <img src=\"x\" alt=\"pic\"/>");
        assert_eq!(t, "see link pic");
    }

    #[test]
    fn otp() {
        assert_eq!(find_otp("Your verification code", "is 482913"), Some("482913".into()));
        assert_eq!(find_otp("Signal", "meet at 1530"), None);
        assert_eq!(find_otp("Login code", "123 456"), Some("123456".into()));
    }
}
