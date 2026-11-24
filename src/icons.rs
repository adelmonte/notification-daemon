use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::config::home;
use crate::notification::RawImage;

struct ThemeDir {
    path: PathBuf,
    size: u32,
    scalable: bool,
}

pub struct Icons {
    themes: Vec<Vec<ThemeDir>>,
    pixmaps_dirs: Vec<PathBuf>,
    app_dirs: Vec<PathBuf>,
    cache: HashMap<(String, u32), Option<Rc<Pixmap>>>,
}

fn data_dirs() -> Vec<PathBuf> {
    let mut v = vec![std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"))];
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    v.push("/var/lib/flatpak/exports/share".into());
    v.push(home().join(".local/share/flatpak/exports/share"));
    v.dedup();
    v
}

fn gtk_icon_theme() -> Option<String> {
    let s = std::fs::read_to_string(home().join(".config/gtk-3.0/settings.ini")).ok()?;
    s.lines()
        .find_map(|l| l.trim().strip_prefix("gtk-icon-theme-name"))
        .map(|v| v.trim_start_matches([' ', '=']).trim().to_string())
}

impl Icons {
    pub fn new(theme: Option<&str>) -> Self {
        let data = data_dirs();
        let mut icon_bases: Vec<PathBuf> = vec![home().join(".icons")];
        icon_bases.extend(data.iter().map(|d| d.join("icons")));

        let mut order: Vec<String> = Vec::new();
        let mut queue: Vec<String> = theme.map(String::from).or_else(gtk_icon_theme).into_iter().collect();
        let mut themes = Vec::new();
        while let Some(name) = queue.pop() {
            if order.contains(&name) || order.len() > 8 {
                continue;
            }
            order.push(name.clone());
            let mut dirs = Vec::new();
            for base in &icon_bases {
                let root = base.join(&name);
                let Ok(index) = std::fs::read_to_string(root.join("index.theme")) else { continue };
                let (theme_dirs, inherits) = parse_index(&index);
                for (sub, size, scalable) in theme_dirs {
                    let path = root.join(&sub);
                    if path.is_dir() {
                        dirs.push(ThemeDir { path, size, scalable });
                    }
                }
                for p in inherits.into_iter().rev() {
                    queue.push(p);
                }
            }
            if !dirs.is_empty() {
                themes.push(dirs);
            }
        }
        if !order.iter().any(|t| t == "hicolor") {
            let mut dirs = Vec::new();
            for base in &icon_bases {
                let root = base.join("hicolor");
                if let Ok(index) = std::fs::read_to_string(root.join("index.theme")) {
                    for (sub, size, scalable) in parse_index(&index).0 {
                        let path = root.join(&sub);
                        if path.is_dir() {
                            dirs.push(ThemeDir { path, size, scalable });
                        }
                    }
                }
            }
            themes.push(dirs);
        }

        Self {
            themes,
            pixmaps_dirs: data.iter().map(|d| d.join("pixmaps")).collect(),
            app_dirs: data.iter().map(|d| d.join("applications")).collect(),
            cache: HashMap::new(),
        }
    }

    fn find(&self, name: &str, size: u32) -> Option<PathBuf> {
        for dirs in &self.themes {
            let mut best: Option<(u32, PathBuf)> = None;
            for d in dirs {
                for ext in ["svg", "png"] {
                    let p = d.path.join(format!("{name}.{ext}"));
                    if !p.is_file() {
                        continue;
                    }
                    let score = if d.scalable || ext == "svg" {
                        0
                    } else if d.size >= size {
                        d.size - size
                    } else {
                        1000 + size - d.size
                    };
                    if best.as_ref().is_none_or(|(s, _)| score < *s) {
                        best = Some((score, p));
                    }
                }
                if best.as_ref().is_some_and(|(s, _)| *s == 0) {
                    break;
                }
            }
            if let Some((_, p)) = best {
                return Some(p);
            }
        }
        for dir in &self.pixmaps_dirs {
            for ext in ["svg", "png"] {
                let p = dir.join(format!("{name}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    fn desktop_icon(&self, entry: &str) -> Option<String> {
        let entry = entry.trim_end_matches(".desktop");
        for dir in &self.app_dirs {
            let Ok(s) = std::fs::read_to_string(dir.join(format!("{entry}.desktop"))) else { continue };
            if let Some(icon) = s.lines().find_map(|l| l.strip_prefix("Icon=")) {
                return Some(icon.trim().to_string());
            }
        }
        None
    }

    /// Loads an icon by theme name, path or file:// URI, rendered at `size` physical pixels.
    pub fn load(&mut self, name: &str, size: u32) -> Option<Rc<Pixmap>> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let key = (name.to_string(), size);
        if let Some(c) = self.cache.get(&key) {
            return c.clone();
        }
        let path = if let Some(p) = name.strip_prefix("file://") {
            Some(PathBuf::from(percent_decode(p)))
        } else if name.starts_with('/') || name.starts_with('~') {
            Some(PathBuf::from(name.replacen('~', &home().to_string_lossy(), 1)))
        } else {
            self.find(name, size)
        };
        let pm = path.and_then(|p| load_file(&p, size)).map(Rc::new);
        if self.cache.len() > 256 {
            self.cache.clear();
        }
        self.cache.insert(key, pm.clone());
        pm
    }

    pub fn load_app(&mut self, app_icon: &str, desktop_entry: Option<&str>, app_name: &str, size: u32) -> Option<Rc<Pixmap>> {
        if let Some(p) = self.load(app_icon, size) {
            return Some(p);
        }
        if let Some(entry) = desktop_entry {
            if let Some(icon) = self.desktop_icon(entry) {
                if let Some(p) = self.load(&icon, size) {
                    return Some(p);
                }
            }
            if let Some(p) = self.load(entry, size) {
                return Some(p);
            }
        }
        let lower = app_name.to_lowercase().replace(' ', "-");
        if !lower.is_empty() {
            if let Some(icon) = self.desktop_icon(&lower) {
                if let Some(p) = self.load(&icon, size) {
                    return Some(p);
                }
            }
            return self.load(&lower, size);
        }
        None
    }
}

fn parse_index(s: &str) -> (Vec<(String, u32, bool)>, Vec<String>) {
    let mut dirs: Vec<String> = Vec::new();
    let mut inherits = Vec::new();
    let mut sections: HashMap<String, (u32, bool)> = HashMap::new();
    let mut cur: Option<String> = None;
    for line in s.lines() {
        let line = line.trim();
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cur = Some(sec.to_string());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match cur.as_deref() {
            Some("Icon Theme") => match k {
                "Directories" | "ScaledDirectories" => {
                    dirs.extend(v.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()))
                }
                "Inherits" => inherits.extend(v.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty())),
                _ => {}
            },
            Some(sec) => {
                let e = sections.entry(sec.to_string()).or_insert((0, false));
                match k {
                    "Size" => e.0 = v.parse().unwrap_or(0),
                    "Type" => e.1 = v == "Scalable",
                    _ => {}
                }
            }
            None => {}
        }
    }
    dirs.dedup();
    let out = dirs
        .into_iter()
        .map(|d| {
            let (size, scalable) = sections.get(&d).copied().unwrap_or((0, false));
            (d, size, scalable)
        })
        .collect();
    (out, inherits)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn load_file(path: &Path, size: u32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "svg" || ext == "svgz" || data.starts_with(b"<?xml") || data.starts_with(b"<svg") {
        return render_svg(&data, size);
    }
    if data.starts_with(b"\x89PNG") {
        let pm = Pixmap::decode_png(&data).ok()?;
        return Some(fit(&pm, size));
    }
    if data.starts_with(&[0xff, 0xd8]) {
        return decode_jpeg(&data).map(|pm| fit(&pm, size));
    }
    None
}

fn render_svg(data: &[u8], size: u32) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let s = tree.size();
    let k = size as f32 / s.width().max(s.height());
    let (w, h) = ((s.width() * k).round().max(1.0) as u32, (s.height() * k).round().max(1.0) as u32);
    let mut pm = Pixmap::new(w, h)?;
    resvg::render(&tree, Transform::from_scale(k, k), &mut pm.as_mut());
    Some(pm)
}

fn decode_jpeg(data: &[u8]) -> Option<Pixmap> {
    let mut dec = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(data));
    let px = dec.decode().ok()?;
    let (w, h) = dec.dimensions()?;
    let ch = px.len() / (w * h).max(1);
    let mut out = Vec::with_capacity(w * h * 4);
    for p in px.chunks_exact(ch) {
        match ch {
            1 => out.extend_from_slice(&[p[0], p[0], p[0], 255]),
            3 => out.extend_from_slice(&[p[0], p[1], p[2], 255]),
            4 => out.extend_from_slice(&[p[0], p[1], p[2], 255]),
            _ => return None,
        }
    }
    Pixmap::from_vec(out, resvg::tiny_skia::IntSize::from_wh(w as u32, h as u32)?)
}

pub fn from_raw(img: &RawImage, size: u32) -> Option<Pixmap> {
    let (w, h) = (img.width.max(0) as usize, img.height.max(0) as usize);
    let ch = img.channels.max(0) as usize;
    let stride = img.rowstride.max(0) as usize;
    if w == 0 || h == 0 || !(3..=4).contains(&ch) || stride < w * ch || img.data.len() < stride * (h - 1) + w * ch {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &img.data[y * stride..y * stride + w * ch];
        for p in row.chunks_exact(ch) {
            let a = if ch == 4 && img.has_alpha { p[3] } else { 255 };
            let m = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
            out.extend_from_slice(&[m(p[0]), m(p[1]), m(p[2]), a]);
        }
    }
    let pm = Pixmap::from_vec(out, resvg::tiny_skia::IntSize::from_wh(w as u32, h as u32)?)?;
    Some(fit(&pm, size))
}

/// Scales so the longer side is `size`, halving first so large downscales stay smooth.
pub fn fit(src: &Pixmap, size: u32) -> Pixmap {
    let mut cur: Option<Pixmap> = None;
    loop {
        let s = cur.as_ref().unwrap_or(src);
        if s.width().max(s.height()) < size * 2 {
            break;
        }
        let (w, h) = ((s.width() / 2).max(1), (s.height() / 2).max(1));
        let mut half = Pixmap::new(w, h).unwrap();
        half.draw_pixmap(
            0,
            0,
            s.as_ref(),
            &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() },
            Transform::from_scale(w as f32 / s.width() as f32, h as f32 / s.height() as f32),
            None,
        );
        cur = Some(half);
    }
    let s = cur.as_ref().unwrap_or(src);
    let k = size as f32 / s.width().max(s.height()) as f32;
    if (k - 1.0).abs() < 1e-3 {
        return s.clone();
    }
    let (w, h) = ((s.width() as f32 * k).round().max(1.0) as u32, (s.height() as f32 * k).round().max(1.0) as u32);
    let mut out = Pixmap::new(w, h).unwrap();
    out.draw_pixmap(
        0,
        0,
        s.as_ref(),
        &PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() },
        Transform::from_scale(w as f32 / s.width() as f32, h as f32 / s.height() as f32),
        None,
    );
    out
}
