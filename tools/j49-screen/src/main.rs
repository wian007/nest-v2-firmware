// j49-screen: the thermostat on the round panel of the Nest J49 (320x320), drawn into /dev/fb0.
//
// A dial like the thermostat card of Home Assistant: the target temperature large in the middle,
// an arc for it around the edge, the room temperature, what the heating does, clock and WiFi.
// Turn the ring to set the temperature; a click opens a menu (mode, schedule, away, status,
// settings). It also shows the login question of the web page and the installer's progress.
//
// It talks to the rest through files (docs/thermostat.md): settings in /var/lib/j49/thermostat
// and /var/lib/j49-screen/settings, state from /run/bpd/*, commands on /run/j49-screen.fifo.
//
//   j49-screen [--fb /dev/fb0] [--screen home|menu|status|settings|install] [--idle 25] [--once]
//   J49_DEMO=1 shows sample values, J49_LANG=<code> and J49_THEME=<name> choose language and
//   theme (for screenshots on a PC, without a backplate).
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ======================================================================================
// Texts, locale and theme. All texts are in one file per language (tools/lang/<code>.txt),
// all colours in one file per theme (tools/themes/<name>.theme). English, Dutch and three
// themes are built in, so the program also runs where those files are not installed (the
// installer); files in /var/lib/j49 and /usr/share/j49 add to them or replace them.
// The choice is in /var/lib/j49/ui (lang=<code>, theme=<name>), shared with the web page.
// ======================================================================================
const UI_FILE: &str = "/var/lib/j49/ui";
const BUILTIN_LANGS: [(&str, &str); 2] = [("en", include_str!("../../lang/en.txt")), ("nl", include_str!("../../lang/nl.txt"))];
const BUILTIN_THEMES: [(&str, &str); 3] = [
    ("default", include_str!("../../themes/default.theme")),
    ("home-assistant", include_str!("../../themes/home-assistant.theme")),
    ("light", include_str!("../../themes/light.theme")),
];
type Texts = std::collections::HashMap<&'static str, &'static str>;
static TEXTS: Mutex<Option<(&'static Texts, &'static Texts)>> = Mutex::new(None); // chosen language, English
static UI_LANG: Mutex<String> = Mutex::new(String::new());
static UI_THEME: Mutex<String> = Mutex::new(String::new());
static SPLASH_TITLE: Mutex<String> = Mutex::new(String::new()); // the large word on the start screen (theme: splash=)

/// key=value lines; "#" starts a comment line. The text is kept for the life of the program.
fn parse_kv(text: String) -> &'static Texts {
    let text: &'static str = Box::leak(text.into_boxed_str());
    let map: Texts = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    Box::leak(Box::new(map))
}
/// A language or theme file by name: the user's own, the installed one, the built-in one.
fn find_file(kind: &str, name: &str, ext: &str, builtin: &[(&str, &str)]) -> Option<String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    for dir in ["/var/lib/j49", "/usr/share/j49"] {
        if let Ok(s) = std::fs::read_to_string(format!("{dir}/{kind}/{name}.{ext}")) {
            return Some(s);
        }
    }
    builtin.iter().find(|(n, _)| *n == name).map(|(_, t)| t.to_string())
}
/// Names of all languages or themes there are, built-in ones first.
fn list_files(kind: &str, ext: &str, builtin: &[(&str, &str)]) -> Vec<String> {
    let mut v: Vec<String> = builtin.iter().map(|(n, _)| n.to_string()).collect();
    for dir in ["/usr/share/j49", "/var/lib/j49"] {
        let Ok(rd) = std::fs::read_dir(format!("{dir}/{kind}")) else { continue };
        let mut found: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(&format!(".{ext}")).map(String::from))).collect();
        found.sort();
        for n in found {
            if !v.contains(&n) {
                v.push(n);
            }
        }
    }
    v
}
fn set_language(code: &str) {
    let en = parse_kv(BUILTIN_LANGS[0].1.to_string());
    let (code, chosen) = match find_file("lang", code, "txt", &BUILTIN_LANGS) {
        Some(text) => (code, parse_kv(text)),
        None => ("en", en),
    };
    *TEXTS.lock().unwrap() = Some((chosen, en));
    *UI_LANG.lock().unwrap() = code.to_string();
}
/// The text for a key in the chosen language; English when the language lacks it; the key
/// itself when nobody has it (so a missing text is visible, not empty).
fn t(key: &'static str) -> &'static str {
    let g = TEXTS.lock().unwrap();
    match *g {
        Some((chosen, en)) => chosen.get(key).or_else(|| en.get(key)).copied().unwrap_or(key),
        None => key,
    }
}
/// A temperature the way this language writes it: "20.5" or "20,5", or whole Fahrenheit.
fn fmt_temp(celsius: f32) -> String {
    if t("locale.unit") == "F" {
        format!("{:.0}", celsius * 9.0 / 5.0 + 32.0)
    } else {
        format!("{:.1}", celsius).replace('.', t("locale.decimal"))
    }
}
/// (whole part, the rest with its separator) for the large target on the dial.
fn temp_parts(celsius: f32) -> (String, String) {
    let s = fmt_temp(celsius);
    match s.find(|c: char| !c.is_ascii_digit() && c != '-') {
        Some(i) => (s[..i].to_string(), s[i..].to_string()),
        None => (s, String::new()),
    }
}
fn fmt_clock(h: i32, m: i32) -> String {
    if t("locale.clock") == "12" {
        format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// The colours of the theme. Used as c::bg(), c::text(), ...
mod c {
    use std::sync::atomic::{AtomicU32, Ordering};
    pub const KEYS: [&str; 13] = ["bg", "surface", "surface2", "line", "dim", "text", "strong", "accent", "heat", "ok", "error", "info", "black"];
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU32 = AtomicU32::new(0xFF00_0000);
    pub static COLORS: [AtomicU32; 13] = [Z; 13];
    pub static DIAL: AtomicU32 = AtomicU32::new(14);
    fn get(i: usize) -> u32 {
        COLORS[i].load(Ordering::Relaxed)
    }
    pub fn bg() -> u32 { get(0) }
    pub fn surface() -> u32 { get(1) }
    pub fn surface2() -> u32 { get(2) }
    pub fn line() -> u32 { get(3) }
    pub fn dim() -> u32 { get(4) }
    pub fn text() -> u32 { get(5) }
    pub fn strong() -> u32 { get(6) }
    pub fn accent() -> u32 { get(7) }
    pub fn heat() -> u32 { get(8) }
    pub fn ok() -> u32 { get(9) }
    pub fn error() -> u32 { get(10) }
    pub fn info() -> u32 { get(11) }
    pub fn black() -> u32 { get(12) }
}
fn apply_theme_text(text: &str) {
    for l in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if let Some(i) = c::KEYS.iter().position(|n| *n == k) {
            if let Ok(rgb) = u32::from_str_radix(v.trim_start_matches('#'), 16) {
                c::COLORS[i].store(0xFF00_0000 | (rgb & 0x00FF_FFFF), Ordering::Relaxed);
            }
        } else if k == "splash" {
            *SPLASH_TITLE.lock().unwrap() = v.to_uppercase().chars().take(8).collect();
        } else if k == "font" {
            if let Ok(n) = v.parse::<u32>() {
                BIG_FONT.store(n.min(4), Ordering::Relaxed);
            }
        } else if k == "dial" {
            if let Ok(n) = v.parse::<u32>() {
                c::DIAL.store(n.clamp(4, 40), Ordering::Relaxed);
            }
        }
    }
}
fn set_theme(name: &str) {
    // the default first, so a theme only has to name what it changes
    apply_theme_text(BUILTIN_THEMES[0].1);
    let name = match find_file("themes", name, "theme", &BUILTIN_THEMES) {
        Some(text) => {
            apply_theme_text(&text);
            name
        }
        None => "default",
    };
    *UI_THEME.lock().unwrap() = name.to_string();
}
fn theme_title() -> String {
    let name = UI_THEME.lock().unwrap().clone();
    find_file("themes", &name, "theme", &BUILTIN_THEMES)
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("name=").map(|v| v.trim().to_uppercase())))
        .unwrap_or_else(|| name.to_uppercase())
}
fn load_ui() {
    let s = std::fs::read_to_string(UI_FILE).unwrap_or_default();
    let get = |key: &str| s.lines().find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')).map(|v| v.trim().to_string()));
    // J49_LANG and J49_THEME win over the file (screenshots on a PC)
    let env = |k: &str| std::env::var(k).ok();
    set_language(&env("J49_LANG").or_else(|| get("lang")).unwrap_or_else(|| "en".into()));
    set_theme(&env("J49_THEME").or_else(|| get("theme")).unwrap_or_else(|| "default".into()));
}
/// Write lang= and theme=, keep whatever else is in the file.
fn save_ui() {
    let old = std::fs::read_to_string(UI_FILE).unwrap_or_default();
    let mut out: String = old.lines().filter(|l| !l.starts_with("lang=") && !l.starts_with("theme=")).map(|l| format!("{l}\n")).collect();
    out.push_str(&format!("lang={}\ntheme={}\n", UI_LANG.lock().unwrap(), UI_THEME.lock().unwrap()));
    let _ = std::fs::create_dir_all("/var/lib/j49");
    let tmp = format!("{UI_FILE}.tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, UI_FILE);
    }
}
/// The next language or theme in the list (the Settings page steps through them).
fn next_of(list: Vec<String>, current: &str) -> String {
    let i = list.iter().position(|n| n == current).map(|i| (i + 1) % list.len()).unwrap_or(0);
    list.get(i).cloned().unwrap_or_else(|| current.to_string())
}

const W: usize = 320;
const H: usize = 320;
const CX: f32 = 160.0;
const CY: f32 = 160.0;
// 0xAARRGGBB; emitted little-endian -> bytes B,G,R,A = XRGB8888.
fn blend(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |sh: u32| {
        let av = ((a >> sh) & 0xff) as f32;
        let bv = ((b >> sh) & 0xff) as f32;
        (av + (bv - av) * t) as u32
    };
    0xFF000000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}
#[derive(Clone)]
struct Fb {
    buf: Vec<u32>,
}
#[allow(dead_code)]
impl Fb {
    fn new() -> Self {
        Fb { buf: vec![c::bg(); W * H] }
    }
    #[inline]
    fn put(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.buf[y as usize * W + x as usize] = c;
        }
    }
    fn fill(&mut self, c: u32) {
        for p in self.buf.iter_mut() {
            *p = c;
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }
    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: u32) {
        let r2 = r * r;
        for yy in (cy - r) as i32..=(cy + r) as i32 {
            for xx in (cx - r) as i32..=(cx + r) as i32 {
                let dx = xx as f32 + 0.5 - cx;
                let dy = yy as f32 + 0.5 - cy;
                if dx * dx + dy * dy <= r2 {
                    self.put(xx, yy, c);
                }
            }
        }
    }
    // Annulus with the leading `frac` of the sweep in `fg`, rest in `track`.
    // Sweep starts at 12 o'clock, runs clockwise.
    fn ring(&mut self, cx: f32, cy: f32, r_in: f32, r_out: f32, frac: f32, fg: u32, track: u32) {
        let tau = std::f32::consts::PI * 2.0;
        let lead = frac.clamp(0.0, 1.0) * tau;
        for yy in (cy - r_out) as i32..=(cy + r_out) as i32 {
            for xx in (cx - r_out) as i32..=(cx + r_out) as i32 {
                let dx = xx as f32 + 0.5 - cx;
                let dy = yy as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                if d >= r_in && d <= r_out {
                    let mut a = dx.atan2(-dy);
                    if a < 0.0 {
                        a += tau;
                    }
                    self.put(xx, yy, if a <= lead { fg } else { track });
                }
            }
        }
    }
    fn circle_line(&mut self, cx: f32, cy: f32, r: f32, c: u32) {
        let steps = (r * 8.0) as i32 + 32;
        let tau = std::f32::consts::PI * 2.0;
        for i in 0..steps {
            let a = tau * i as f32 / steps as f32;
            self.put((cx + r * a.sin()) as i32, (cy - r * a.cos()) as i32, c);
        }
    }
    fn round_mask(&mut self, r: f32) {
        let r2 = r * r;
        for yy in 0..H {
            for xx in 0..W {
                let dx = xx as f32 + 0.5 - CX;
                let dy = yy as f32 + 0.5 - CY;
                if dx * dx + dy * dy > r2 {
                    self.buf[yy * W + xx] = c::black();
                }
            }
        }
    }
    fn glyph_blit(&mut self, x: i32, y: i32, rows: [u8; 7], scale: i32, c: u32) {
        for (ry, row) in rows.iter().enumerate() {
            for cx in 0..5 {
                if row & (1 << (4 - cx)) != 0 {
                    self.rect(x + cx * scale, y + ry as i32 * scale, scale, scale, c);
                }
            }
        }
    }
    /// Large text (scale 3 and up) in one of the alternative looks, chosen with font= in the
    /// settings: 1 dot matrix, 2 smoothed blocks, 3 seven-segment digits, 4 thin round strokes.
    /// All of them use the same 5x7 shapes and the same character box as the plain blocks.
    fn big_glyph(&mut self, x: i32, y: i32, ch: char, scale: i32, c: u32, font: u32) {
        let rows = glyph(ch);
        let lit = |cx: i32, ry: i32| cx >= 0 && cx < 5 && ry >= 0 && ry < 7 && rows[ry as usize] & (1 << (4 - cx)) != 0;
        let s = scale;
        match font {
            1 => {
                // dot matrix: a round dot per cell
                for ry in 0..7 {
                    for cx in 0..5 {
                        if lit(cx, ry) {
                            self.disc(x as f32 + (cx as f32 + 0.5) * s as f32 - 0.5, y as f32 + (ry as f32 + 0.5) * s as f32 - 0.5, s as f32 * 0.46, c);
                        }
                    }
                }
            }
            3 if ch.is_ascii_digit() || ch == '-' => {
                // seven segments in the same box: a top, g middle, d bottom, f/b upper, e/c lower
                let segs: u8 = match ch {
                    '0' => 0b0111111,
                    '1' => 0b0000110,
                    '2' => 0b1011011,
                    '3' => 0b1001111,
                    '4' => 0b1100110,
                    '5' => 0b1101101,
                    '6' => 0b1111101,
                    '7' => 0b0000111,
                    '8' => 0b1111111,
                    '9' => 0b1101111,
                    _ => 0b1000000,
                };
                let (w, h, th) = (5 * s, 7 * s, (s * 4 / 5).max(2));
                let half = (h - th) / 2;
                // horizontal bars are shortened and the vertical ones too, so the corners stay open
                let g = 1;
                let bars: [(i32, i32, i32, i32); 7] = [
                    (th + g, 0, w - 2 * th - 2 * g, th),            // a
                    (w - th, th + g, th, half - th - 2 * g),        // b
                    (w - th, half + th + g, th, half - th - 2 * g), // c
                    (th + g, h - th, w - 2 * th - 2 * g, th),       // d
                    (0, half + th + g, th, half - th - 2 * g),      // e
                    (0, th + g, th, half - th - 2 * g),             // f
                    (th + g, half, w - 2 * th - 2 * g, th),         // g
                ];
                for (i, (bx, by, bw, bh)) in bars.iter().enumerate() {
                    if segs & (1 << i) != 0 {
                        self.rect(x + bx, y + by, *bw, *bh, c);
                    }
                }
            }
            4 => {
                // thin round strokes: join the centres of neighbouring cells
                let w = (s as f32 * 0.5).max(1.5);
                let centre = |cx: i32, ry: i32| (x as f32 + (cx as f32 + 0.5) * s as f32 - 0.5, y as f32 + (ry as f32 + 0.5) * s as f32 - 0.5);
                let stroke = |fb: &mut Fb, a: (f32, f32), b: (f32, f32)| {
                    let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as i32).max(1);
                    for i in 0..=n {
                        let t = i as f32 / n as f32;
                        fb.disc(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, w / 2.0, c);
                    }
                };
                for ry in 0..7 {
                    for cx in 0..5 {
                        if !lit(cx, ry) {
                            continue;
                        }
                        let a = centre(cx, ry);
                        stroke(self, a, a);
                        if lit(cx + 1, ry) {
                            stroke(self, a, centre(cx + 1, ry));
                        }
                        if lit(cx, ry + 1) {
                            stroke(self, a, centre(cx, ry + 1));
                        }
                        // diagonals only where no straight path joins the two cells
                        if lit(cx + 1, ry + 1) && !lit(cx + 1, ry) && !lit(cx, ry + 1) {
                            stroke(self, a, centre(cx + 1, ry + 1));
                        }
                        if lit(cx - 1, ry + 1) && !lit(cx - 1, ry) && !lit(cx, ry + 1) {
                            stroke(self, a, centre(cx - 1, ry + 1));
                        }
                    }
                }
            }
            _ => {
                // smoothed blocks: outer corners cut off, inner corners of diagonals filled in
                let q = (s + 1) / 2;
                for ry in 0..7 {
                    for cx in 0..5 {
                        let here = lit(cx, ry);
                        for py in 0..s {
                            for px in 0..s {
                                // which corner of the cell is this pixel in, if any
                                let corner = if px + py < q {
                                    Some((-1, -1))
                                } else if (s - 1 - px) + py < q {
                                    Some((1, -1))
                                } else if px + (s - 1 - py) < q {
                                    Some((-1, 1))
                                } else if (s - 1 - px) + (s - 1 - py) < q {
                                    Some((1, 1))
                                } else {
                                    None
                                };
                                let on = match (here, corner) {
                                    (true, None) => true,
                                    // a lit cell loses a corner that sticks out on its own
                                    (true, Some((dx, dy))) => lit(cx + dx, ry) || lit(cx, ry + dy) || lit(cx + dx, ry + dy),
                                    // an empty cell gets a corner that two lit neighbours share
                                    (false, Some((dx, dy))) => lit(cx + dx, ry) && lit(cx, ry + dy),
                                    (false, None) => false,
                                };
                                if on {
                                    self.put(x + cx * s + px, y + ry * s + py, c);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: u32) {
        let mut cx = x;
        let font = BIG_FONT.load(Ordering::Relaxed);
        for ch in s.chars() {
            if scale >= 3 && font != 0 {
                self.big_glyph(cx, y, ch, scale, c, font);
            } else {
                self.glyph_blit(cx, y, glyph(ch), scale, c);
            }
            cx += 6 * scale;
        }
    }
    fn text_w(s: &str, scale: i32) -> i32 {
        s.chars().count() as i32 * 6 * scale - scale
    }
    fn text_center(&mut self, cx: i32, y: i32, s: &str, scale: i32, c: u32) {
        let w = Fb::text_w(s, scale);
        self.text(cx - w / 2, y, s, scale, c);
    }
    fn frame_decor(&mut self) {
        self.circle_line(CX, CY, 150.0, c::line());
        self.circle_line(CX, CY, 149.0, c::line());
        for &(dx, dy) in &[(0.0f32, -1.0f32), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
            for t in 0..10 {
                let r = 142.0 + t as f32;
                self.put((CX + dx * r) as i32, (CY + dy * r) as i32, c::accent());
            }
        }
    }
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u32) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }
    fn write_to(&self, f: &mut File) -> std::io::Result<()> {
        // Zero-copy: on a little-endian target the Vec<u32> (0xAARRGGBB) is
        // already laid out as B,G,R,A bytes = the fb's XRGB8888. No per-frame
        // allocation or conversion.
        let bytes =
            unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const u8, self.buf.len() * 4) };
        f.seek(SeekFrom::Start(0))?;
        f.write_all(bytes)?;
        Ok(())
    }
}
// 5x7 pixel font, 7 rows of 5 bits (bit4 = leftmost column).
/// Look of large text: 0 blocks, 1 dots, 2 smoothed, 3 seven-segment digits, 4 thin strokes
/// (font= in /var/lib/j49-screen/settings).
static BIG_FONT: AtomicU32 = AtomicU32::new(2);
fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b01110, 0b10001, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b10001, 0b01110],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10011, 0b10011, 0b00000],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '/' => [0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        _ => [0; 7],
    }
}
// ---- live system probes (best effort; never panic) ----
fn read_trim(p: &str) -> Option<String> {
    let mut s = String::new();
    File::open(p).ok()?.read_to_string(&mut s).ok()?;
    Some(s.trim().to_string())
}
fn wlan_up() -> bool {
    read_trim("/sys/class/net/wlan0/operstate").map(|s| s == "up").unwrap_or(false)
}
const SIOCGIFADDR: libc::c_ulong = 0x8915;
fn iface_ip(name: &str) -> Option<String> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return None;
    }
    let mut ifr = [0u8; 64];
    for (i, &c) in name.as_bytes().iter().take(15).enumerate() {
        ifr[i] = c;
    }
    let r = unsafe { libc::ioctl(fd, SIOCGIFADDR as _, ifr.as_mut_ptr()) };
    unsafe { libc::close(fd) };
    if r < 0 {
        return None;
    }
    // sockaddr_in in the ifreq union: family(2) port(2) addr(4) -> addr at +20
    let o = &ifr[20..24];
    Some(format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]))
}
// TWL4030 MADC (the ADC the stock Nest used for charge sensing).
fn madc(ch: &str) -> Option<i32> {
    read_trim(&format!("/sys/bus/iio/devices/iio:device0/{}", ch)).and_then(|s| s.parse().ok())
}
// Battery sense path (reverse-engineered from stock nlReadBatteryLevel in
// libnlapputils): the head-unit cell voltage is on TWL4030 MADC ADCIN0 behind a
// ~1:3 external divider. Stock reads the hwmon attr and does volts = raw*3/1000;
// on mainline the same pin is iio in_voltage0_input (mV at the pin), so
// VBAT_mV = in_voltage0_input * VBAT_DIV. VBAT_DIV is the divider ratio, left as
// a tunable constant (calibrate against a known charge state if the % looks off).
const VBAT_DIV: f32 = 3.0;
// Battery voltage in millivolts, or None if the channel reads implausibly.
fn batt_mv() -> Option<i32> {
    let pin = madc("in_voltage0_input")?;
    let mv = (pin as f32 * VBAT_DIV) as i32;
    if (2800..=4400).contains(&mv) {
        Some(mv)
    } else {
        None
    }
}
fn uptime_str() -> String {
    if let Some(s) = read_trim("/proc/uptime") {
        if let Some(secs) = s.split('.').next().and_then(|x| x.parse::<u64>().ok()) {
            return format!("UP {}H{:02}M", secs / 3600, (secs % 3600) / 60);
        }
    }
    "UP -".into()
}
// Latest backplate SHT20 reading: (temperature in C, relative humidity in %), if fresh (< 15 min).
fn sht20() -> Option<(f32, f32)> {
    let s = std::fs::read_to_string("/run/bpd/sht20").ok()?;
    let (mut t, mut rh, mut at) = (None, None, 0u64);
    for l in s.lines() {
        if let Some((k, v)) = l.split_once('=') {
            match k {
                "temp" => t = v.parse::<f32>().ok(),
                "rh" => rh = v.parse::<f32>().ok(),
                "t" => at = v.parse::<u64>().unwrap_or(0),
                _ => {}
            }
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if now > at + 900 {
        return None;
    }
    Some((t?, rh?))
}
// "key=value" lines of a small bpd state file, if it was written less than `max_age` s ago.
fn bpd_file(name: &str, max_age: u64) -> Option<std::collections::HashMap<String, u64>> {
    let s = std::fs::read_to_string(format!("/run/bpd/{}", name)).ok()?;
    let m: std::collections::HashMap<String, u64> = s
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter_map(|(k, v)| v.trim().parse::<u64>().ok().map(|n| (k.to_string(), n)))
        .collect();
    if unix_now() > m.get("t").copied().unwrap_or(0) + max_age {
        return None;
    }
    Some(m)
}
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
// Turning the ring changes the target; it lights up for a few seconds after a turn.
/// Local date and time: (weekday 0 = Monday, day, month 0 = January, hour, minute). The system
/// clock runs on UTC; the zone is tz= of the thermostat file, as for the schedule in bpd.
fn local_now() -> Option<(usize, i32, usize, i32, i32)> {
    const DEFAULT_TZ: &str = "CET-1CEST,M3.5.0,M10.5.0/3";
    static TZ_SET: Mutex<String> = Mutex::new(String::new());
    let tz = std::fs::read_to_string(THERMOSTAT_FILE)
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("tz=").map(|v| v.trim().to_string())))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_TZ.to_string());
    extern "C" {
        fn tzset();
    }
    let mut cur = TZ_SET.lock().unwrap();
    if *cur != tz {
        std::env::set_var("TZ", &tz);
        unsafe { tzset() };
        *cur = tz;
    }
    let now = unix_now() as libc::time_t;
    // before the clock was set (no network yet) there is no date worth showing
    if now < 1_600_000_000 {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return None;
    }
    Some((((tm.tm_wday + 6) % 7) as usize, tm.tm_mday, tm.tm_mon as usize, tm.tm_hour, tm.tm_min))
}
static RING_TOTAL: AtomicI32 = AtomicI32::new(0);
// Installer progress, written by the installer init (busybox sh) as simple
// KEY=VALUE lines and re-read every frame:
//   PHASE=erase|write|verify|done|error   (missing/other = waiting for host)
//   PCT=0..100   TARGET=SPL0+SPL1   INFO=0X40000 1.5 MB   CODE=E3 CONTROLE
#[derive(Clone)]
struct InstallState {
    phase: String,
    pct: f32,
    target: String,
    info: String,
    code: String,
}
impl Default for InstallState {
    fn default() -> Self {
        InstallState {
            phase: "wait".into(),
            pct: 0.0,
            target: String::new(),
            info: String::new(),
            code: String::new(),
        }
    }
}
fn read_install_state(path: &str) -> InstallState {
    let mut st = InstallState::default();
    let Ok(s) = std::fs::read_to_string(path) else { return st };
    for line in s.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "PHASE" => st.phase = v.to_string(),
            "PCT" => st.pct = v.parse::<f32>().unwrap_or(0.0).clamp(0.0, 100.0),
            "TARGET" => st.target = v.to_string(),
            "INFO" => st.info = v.to_string(),
            "CODE" => st.code = v.to_string(),
            _ => {}
        }
    }
    st
}
// ---- web login: j49-web asks, somebody at the device answers with the ring ----
// j49-web writes /run/j49-web/pending (code=, ip=, t=) when a browser wants to log in. This
// screen shows the same code the browser shows; turning the ring picks WEIGEREN or TOESTAAN
// (it starts on WEIGEREN, so a stray click refuses), a click writes /run/j49-web/answer.
const WEB_PENDING: &str = "/run/j49-web/pending";
const WEB_ANSWER: &str = "/run/j49-web/answer";
const WEB_LOGIN_SECS: u64 = 60;
static CONFIRM: Mutex<Option<(String, String, u64)>> = Mutex::new(None); // code, address, asked at
static CONFIRM_DONE: Mutex<String> = Mutex::new(String::new()); // code that was answered already
static CONFIRM_YES: AtomicBool = AtomicBool::new(false);
fn spawn_web_confirm() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(400));
        let mut code = String::new();
        let mut ip = String::new();
        let mut at = 0u64;
        if let Ok(s) = std::fs::read_to_string(WEB_PENDING) {
            for l in s.lines() {
                match l.split_once('=') {
                    Some(("code", v)) => code = v.trim().chars().filter(|c| c.is_ascii_digit()).take(6).collect(),
                    Some(("ip", v)) => ip = v.trim().chars().filter(|c| c.is_ascii_hexdigit() || *c == '.' || *c == ':').take(24).collect(),
                    Some(("t", v)) => at = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let live = !code.is_empty() && unix_now() <= at + WEB_LOGIN_SECS && *CONFIRM_DONE.lock().unwrap() != code;
        let mut c = CONFIRM.lock().unwrap();
        if live {
            if c.as_ref().map(|x| x.0 != code).unwrap_or(true) {
                *c = Some((code, ip, at));
                drop(c);
                CONFIRM_YES.store(false, Ordering::Relaxed);
                PAGE_REQ.store(5, Ordering::Relaxed);
                piezo_beep(1800.0, 60);
                piezo_beep(2400.0, 90);
            }
            WAKE.fetch_add(1, Ordering::Relaxed); // stay lit while the question is open
        } else if c.take().is_some() {
            // withdrawn, timed out or answered: back to the dial
            WANT_HOME.store(true, Ordering::Relaxed);
        }
    });
}
fn web_answer(ok: bool) {
    let Some((code, _, _)) = CONFIRM.lock().unwrap().take() else { return };
    let _ = std::fs::create_dir_all("/run/j49-web");
    let tmp = format!("{WEB_ANSWER}.tmp");
    if std::fs::write(&tmp, format!("code={code}\nok={}\n", ok as u8)).is_ok() {
        let _ = std::fs::rename(&tmp, WEB_ANSWER);
    }
    *CONFIRM_DONE.lock().unwrap() = code;
}
const CLICK_HZ: f32 = 2000.0;
fn piezo_click() {
    piezo_beep(CLICK_HZ, 6);
}
// ---- settings: backlight level + piezo volume, adjusted on the settings page ----
// brightness = lm3530 level 10..=120 (hardware max 120); volume 0..=10 (0 = silent).
static BRIGHT: AtomicU32 = AtomicU32::new(100);
static VOLUME: AtomicU32 = AtomicU32::new(5);
static WIFI: AtomicBool = AtomicBool::new(true); // WiFi radio wanted on
const SETTINGS_DIR: &str = "/var/lib/j49-screen";
// ---- thermostat: mode, setpoint and sensor correction, shared with bpd through a file ----
// /var/lib/j49/thermostat ("mode=heat|off", "setpoint=20.5", "offset=-3.0"). j49-screen writes it
// when the ring changes something; bpd reads it and does the actual control. The offset is added
// to the SHT20 reading: the head unit warms the sensor (about +3 C measured while it never sleeps).
static SETPOINT_X10: AtomicI32 = AtomicI32::new(200); // 20.0 C
static HEAT_MODE: AtomicBool = AtomicBool::new(false); // false = off (the safe default)
static OFFSET_X10: AtomicI32 = AtomicI32::new(-30);
static FROST_X10: AtomicI32 = AtomicI32::new(50); // frost protection threshold, 0 = off (bpd acts on it)
static AWAY: AtomicBool = AtomicBool::new(false); // nobody home: bpd aims for AWAY_X10 instead of the setpoint
static AWAY_X10: AtomicI32 = AtomicI32::new(150);
static WIFI_LINK: AtomicI32 = AtomicI32::new(1); // 0 = WiFi off, 1 = connecting, 2 = connected (for the antenna)
static WIFI_BARS: AtomicI32 = AtomicI32::new(3); // signal strength 1..3
/// Signal level of wlan0 in dBm, from /proc/net/wireless.
fn wifi_dbm() -> Option<i32> {
    let s = std::fs::read_to_string("/proc/net/wireless").ok()?;
    let l = s.lines().find(|l| l.trim_start().starts_with("wlan0:"))?;
    l.split_whitespace().nth(3)?.trim_end_matches('.').parse().ok()
}
static SCHEDULE_ON: AtomicBool = AtomicBool::new(false); // follow /var/lib/j49/schedule (bpd acts on it)
const SCHEDULE_FILE: &str = "/var/lib/j49/schedule";
// Written when the schedule is switched on for the first time; meant to be edited.
const SCHEDULE_DEFAULT: &str = "# Schema: <dagen> <uu:mm> <temperatuur>\n# dagen: ma di wo do vr za zo, bereik ma-vr, lijst za,zo, of dagelijks\nma-vr 06:30 20.0\nma-vr 08:30 17.0\nma-vr 17:00 20.5\nma-vr 22:30 16.0\nza,zo 08:00 20.5\nza,zo 23:00 16.0\n";
static SP_TOUCHED: AtomicU64 = AtomicU64::new(0); // unix time the setpoint was last turned
const THERMOSTAT_FILE: &str = "/var/lib/j49/thermostat";
const SETPOINT_MIN_X10: i32 = 90;
const SETPOINT_MAX_X10: i32 = 320;
fn load_thermostat() {
    let Ok(s) = std::fs::read_to_string(THERMOSTAT_FILE) else { return };
    for l in s.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "mode" => HEAT_MODE.store(v == "heat", Ordering::Relaxed),
            "setpoint" => {
                if let Ok(f) = v.parse::<f32>() {
                    SETPOINT_X10.store(((f * 10.0).round() as i32).clamp(SETPOINT_MIN_X10, SETPOINT_MAX_X10), Ordering::Relaxed)
                }
            }
            "offset" => {
                if let Ok(f) = v.parse::<f32>() {
                    OFFSET_X10.store(((f * 10.0).round() as i32).clamp(-100, 100), Ordering::Relaxed)
                }
            }
            "frost" => {
                if let Ok(f) = v.parse::<f32>() {
                    FROST_X10.store(((f * 10.0).round() as i32).clamp(0, 120), Ordering::Relaxed)
                }
            }
            "schedule" => SCHEDULE_ON.store(v == "on", Ordering::Relaxed),
            "away" => AWAY.store(v == "on", Ordering::Relaxed),
            "away_temp" => {
                if let Ok(f) = v.parse::<f32>() {
                    AWAY_X10.store(((f * 10.0).round() as i32).clamp(SETPOINT_MIN_X10, SETPOINT_MAX_X10), Ordering::Relaxed)
                }
            }
            _ => {}
        }
    }
}
fn save_thermostat() {
    let _ = std::fs::create_dir_all("/var/lib/j49");
    // our keys, in a fixed order; every other line of the file (tz=, things Home Assistant adds)
    // is kept as it is
    let mine = [
        ("mode", if HEAT_MODE.load(Ordering::Relaxed) { "heat".to_string() } else { "off".to_string() }),
        ("setpoint", format!("{:.1}", SETPOINT_X10.load(Ordering::Relaxed) as f32 / 10.0)),
        ("offset", format!("{:.1}", OFFSET_X10.load(Ordering::Relaxed) as f32 / 10.0)),
        ("frost", format!("{:.1}", FROST_X10.load(Ordering::Relaxed) as f32 / 10.0)),
        ("schedule", if SCHEDULE_ON.load(Ordering::Relaxed) { "on".to_string() } else { "off".to_string() }),
        ("away", if AWAY.load(Ordering::Relaxed) { "on".to_string() } else { "off".to_string() }),
    ];
    let mut body = String::new();
    for (k, v) in &mine {
        body.push_str(&format!("{k}={v}\n"));
    }
    if let Ok(old) = std::fs::read_to_string(THERMOSTAT_FILE) {
        for l in old.lines() {
            let key = l.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
            if !l.trim().is_empty() && !mine.iter().any(|(k, _)| *k == key) {
                body.push_str(l);
                body.push('\n');
            }
        }
    }
    let tmp = format!("{}.tmp", THERMOSTAT_FILE);
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, THERMOSTAT_FILE);
    }
}
/// Room temperature for display and control: SHT20 reading plus the correction.
fn room_temp() -> Option<(f32, f32)> {
    sht20().map(|(t, rh)| (t + OFFSET_X10.load(Ordering::Relaxed) as f32 / 10.0, rh))
}
// Soft-block or unblock every WLAN radio: one struct rfkill_event written to /dev/rfkill
// (idx u32, type u8 = RFKILL_TYPE_WLAN 1, op u8 = RFKILL_OP_CHANGE_ALL 3, soft u8, hard u8).
fn rfkill_wlan(block: bool) {
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("/dev/rfkill") {
        let _ = f.write_all(&[0, 0, 0, 0, 1, 3, block as u8, 0]);
    }
}
// Apply the WiFi setting. Runs in its own thread: systemctl can take seconds.
fn wifi_apply(on: bool) {
    std::thread::spawn(move || {
        let run = |args: &[&str]| {
            let _ = std::process::Command::new(args[0]).args(&args[1..]).status();
        };
        if on {
            rfkill_wlan(false);
            run(&["ip", "link", "set", "wlan0", "up"]);
            run(&["systemctl", "reset-failed", "wpa_supplicant@wlan0"]);
            run(&["systemctl", "restart", "wpa_supplicant@wlan0"]);
            // networkd must notice the link again and ask for a new DHCP lease
            run(&["systemctl", "restart", "systemd-networkd"]);
        } else {
            run(&["systemctl", "stop", "wpa_supplicant@wlan0"]);
            run(&["ip", "link", "set", "wlan0", "down"]);
            rfkill_wlan(true);
        }
    });
}
// ---- device piezo: GPT11 PWM on gpmc_ncs6 + PIEZO_N (GPIO 58) low, volume = duty cycle ----
// This is what the stock firmware does (stock pad dump: gpmc_ncs6 = gpt11_pwm_evt, mode 3;
// gpio-58 DIAMOND_GPIO_PIEZO_N output, high when idle). The tone is the PWM; PIEZO_N must be low
// while it sounds. The mainline DT (GPT8 on the PIEZO_N pad) drives the wrong pin and stays silent,
// so the app programs the hardware through /dev/mem. A piezo gets quieter as the duty falls below
// 50 %. Registers (OMAP3430 TRM): padconf 0x480020b8 (ncs6 = upper half) / 0x480020bc (ncs7 = lower
// half), GPIO2 0x49050000 (OE 0x34, CLEARDATAOUT 0x90, SETDATAOUT 0x94), GPT11 0x48088000
// (TCLR 0x24, TCRR 0x28, TLDR 0x2c, TMAR 0x38), CM_CORE 0x48004a00/10/40, CM_PER 0x48005000/10.
struct PiezoHw {
    gpt: *mut u32,
    gpio: *mut u32,
    tick_hz: f32,
    running: bool,
}
unsafe impl Send for PiezoHw {}
static PIEZO: Mutex<Option<PiezoHw>> = Mutex::new(None);
static PIEZO_INIT: std::sync::Once = std::sync::Once::new();
unsafe fn map_phys(fd: i32, phys: usize) -> *mut u32 {
    let p = libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, phys as libc::off_t);
    if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p as *mut u32 }
}
unsafe fn rd(base: *mut u32, off: usize) -> u32 {
    std::ptr::read_volatile(base.add(off / 4))
}
unsafe fn wr(base: *mut u32, off: usize, v: u32) {
    std::ptr::write_volatile(base.add(off / 4), v)
}
fn piezo_hw_init() -> Option<PiezoHw> {
    let f = OpenOptions::new().read(true).write(true).open("/dev/mem").ok()?;
    let fd = f.as_raw_fd();
    unsafe {
        let pad = map_phys(fd, 0x4800_2000);
        let cmc = map_phys(fd, 0x4800_4000);
        let cmp = map_phys(fd, 0x4800_5000);
        let gpio = map_phys(fd, 0x4905_0000);
        let gpt = map_phys(fd, 0x4808_8000);
        if pad.is_null() || cmc.is_null() || cmp.is_null() || gpio.is_null() || gpt.is_null() {
            return None;
        }
        // module clocks: GPT11 (CORE bit 12) and GPIO2 (PER bit 13); the kernel gates both when unused
        wr(cmc, 0xa00, rd(cmc, 0xa00) | (1 << 12));
        wr(cmc, 0xa10, rd(cmc, 0xa10) | (1 << 12));
        wr(cmp, 0x000, rd(cmp, 0x000) | (1 << 13));
        wr(cmp, 0x010, rd(cmp, 0x010) | (1 << 13));
        // GPT11 on the system clock (CLKSEL_CORE bit 7) instead of 32 kHz: fine duty steps
        wr(cmc, 0xa40, rd(cmc, 0xa40) | (1 << 7));
        // pads: ncs6 -> mode 3 (gpt11_pwm_evt, output); ncs7 -> mode 4 (gpio_58, output)
        wr(pad, 0xb8, (rd(pad, 0xb8) & 0x0000_ffff) | 0x0003_0000);
        wr(pad, 0xbc, (rd(pad, 0xbc) & 0xffff_0000) | 0x0000_0004);
        // gpio_58 is the enable input of the piezo driver, active low (stock kernel:
        // DIAMOND_GPIO_PIEZO_NENABLE), NOT the second leg of the piezo: it cannot be driven in
        // antiphase to get more volume. The signal is GPT11's PWM alone (stock: pwm-beeper, which
        // uses a 50 % duty cycle), so volume 10 here is as loud as stock. Output, idle high.
        wr(gpio, 0x34, rd(gpio, 0x34) & !(1 << 26));
        wr(gpio, 0x94, 1 << 26);
        // measure the timer tick rate (system clock: 12/13/19.2/26/38.4 MHz depending on the board)
        wr(gpt, 0x24, 0);
        wr(gpt, 0x2c, 0);
        wr(gpt, 0x28, 0);
        wr(gpt, 0x24, 0x3); // ST | AR, free running
        let t0 = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let ticks = rd(gpt, 0x28);
        let el = t0.elapsed().as_secs_f32();
        wr(gpt, 0x24, 0);
        let hz = ticks as f32 / el;
        if !(1.0e6..=6.0e7).contains(&hz) {
            return None;
        }
        std::mem::forget(f); // keep the mappings valid
        Some(PiezoHw { gpt, gpio, tick_hz: hz, running: false })
    }
}
fn piezo_ok() -> bool {
    PIEZO_INIT.call_once(|| {
        *PIEZO.lock().unwrap() = piezo_hw_init();
    });
    PIEZO.lock().unwrap().is_some()
}
// hz <= 0 or volume 0: off.
fn piezo_tone(hz: f32) {
    if !piezo_ok() {
        return;
    }
    let mut g = PIEZO.lock().unwrap();
    let h = match g.as_mut() { Some(h) => h, None => return };
    let vol = VOLUME.load(Ordering::Relaxed);
    unsafe {
        if hz < 50.0 || vol == 0 {
            wr(h.gpt, 0x24, 0);
            wr(h.gpio, 0x94, 1 << 26); // nENABLE high = driver off
            h.running = false;
            return;
        }
        let period = ((h.tick_hz / hz) as u32).max(8);
        let frac = (0.5 * (vol as f32 / 10.0).powi(2)).max(0.02);
        let duty = ((period as f32 * frac) as u32).clamp(1, period - 1);
        let ldr = 0u32.wrapping_sub(period);
        wr(h.gpio, 0x90, 1 << 26); // nENABLE low = driver on
        wr(h.gpt, 0x2c, ldr);
        wr(h.gpt, 0x38, ldr.wrapping_add(duty));
        if !h.running {
            wr(h.gpt, 0x28, ldr);
            // ST | AR | CE | trigger on overflow+match | toggle: PWM on the pin
            wr(h.gpt, 0x24, 0x1843);
            h.running = true;
        }
    }
}
fn piezo_beep(hz: f32, ms: u64) {
    piezo_tone(hz);
    std::thread::sleep(Duration::from_millis(ms));
    piezo_tone(0.0);
}
// say() wakes the panel and shows the home page: the main loop consumes these.
static WAKE: AtomicU64 = AtomicU64::new(0);
static WANT_HOME: AtomicBool = AtomicBool::new(false);
static SLEEP_NOW: AtomicBool = AtomicBool::new(false);
static PAGE_REQ: AtomicU32 = AtomicU32::new(u32::MAX); // page asked for over the FIFO (MAX = none)
static ASLEEP: AtomicBool = AtomicBool::new(false); // the panel is in idle (backlight off)
// Control FIFO /run/j49-screen.fifo, one command per line:
//   tune <name>  play a tune, or a melody in RTTTL
//   page <n>     show a page
//   wake         panel on until the idle timeout
//   sleep        panel off now
// e.g. `echo "tune ding" > /run/j49-screen.fifo`
const CTRL_FIFO: &str = "/run/j49-screen.fifo";
// Wake the panel when the backplate's PIR sees motion. bpd writes /run/bpd/pir about once a
// second; `motion_t` is the unix time of the last reading at or above its motion threshold.
fn spawn_motion_wake() {
    std::thread::spawn(|| {
        let mut seen = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let Some(m) = bpd_file("pir", 10) else { continue };
            let mt = m.get("motion_t").copied().unwrap_or(0);
            if mt > seen {
                // the first reading after start only sets the baseline
                if seen != 0 {
                    WAKE.fetch_add(1, Ordering::Relaxed);
                }
                seen = mt;
            }
        }
    });
}
// ---- tunes on the piezo ----
// "tune <name>" plays one of these, "tune <RTTTL>" any melody in the ringtone text format
// (name:d=<default length>,o=<default octave>,b=<beats per minute>:<notes>). The tunes here are
// our own little arpeggios.
const TUNES: [(&str, &str); 34] = [
    ("ding", "ding:d=8,o=6,b=180:e,4c7"),
    ("done", "done:d=8,o=5,b=160:c,e,g,4c6"),
    ("happy", "happy:d=8,o=5,b=180:g,c6,e6,g6,e6,4g6"),
    ("alarm", "alarm:d=8,o=6,b=200:a,p,a,p,a,p,2a"),
    ("sleep", "sleep:d=4,o=5,b=100:g,e,2c"),
    // Melodies in the public domain (traditional or long out of copyright), written out by us
    // an octave up, where the piezo is loudest.
    ("ode", "ode:d=4,o=6,b=150:e,e,f,g,g,f,e,d,c,c,d,e,e.,8d,2d,e,e,f,g,g,f,e,d,c,c,d,e,d.,8c,2c"),
    ("elise", "elise:d=8,o=6,b=125:e7,d#7,e7,d#7,e7,b,d7,c7,4a,p,c,e,a,4b,p,e,g#,b,4c7,p,e,e7,d#7,e7,d#7,e7,b,d7,c7,4a,p,c,e,a,4b,p,e,c7,b,4a"),
    ("nachtmusik", "nachtmusik:d=8,o=6,b=140:4g,p,d,4g,p,d,g,d,g,b,4d7,p,4c7,p,a,4c7,p,a,c7,a,f#,a,4d"),
    ("toccata", "toccata:d=16,o=6,b=80:a,g,2a,p,g,f,e,d,4c#,2d"),
    ("mountainking", "mountainking:d=8,o=6,b=150:a5,b5,c,d,e,c,4e,d#,b5,4d#,d,a#5,4d,a5,b5,c,d,e,c,e,a,g,e,c,e,2g"),
    ("frerejacques", "frerejacques:d=4,o=6,b=150:c,d,e,c,c,d,e,c,e,f,2g,e,f,2g,8g,8a,8g,8f,e,c,8g,8a,8g,8f,e,c,c,g5,2c,c,g5,2c"),
    ("twinkle", "twinkle:d=4,o=6,b=150:c,c,g,g,a,a,2g,f,f,e,e,d,d,2c,g,g,f,f,e,e,2d,g,g,f,f,e,e,2d,c,c,g,g,a,a,2g,f,f,e,e,d,d,2c"),
    ("birthday", "birthday:d=4,o=6,b=120:8c,8c,d,c,f,2e,8c,8c,d,c,g,2f,8c,8c,c7,a,f,e,d,8a#,8a#,a,f,g,2f"),
    ("jinglebells", "jinglebells:d=4,o=6,b=190:e,e,2e,e,e,2e,e,g,c.,8d,1e,f,f,f.,8f,f,e,e,8e,8e,e,d,d,e,2d,2g"),    ("cancan", "cancan:d=8,o=6,b=160:4c,d,f,e,d,4g,4g,g,a,e,f,4d,4d,d,f,e,d,c,c7,b,a,g,f,e,d,4c,d,f,e,d,4g,4g,g,a,e,f,4d,4d,d,f,e,d,c,g,d,e,4c"),
    ("tell", "tell:d=16,o=6,b=120:c,c,8c,c,c,8c,c,c,f,8g,8a,c,c,8c,c,c,f,a,a,8g,8e,8c,c,c,8c,c,c,8c,c,c,f,8g,8a,f,a,4c7,a#,a,g,8f,8a,8f"),
    ("fifth", "fifth:d=8,o=6,b=110:g,g,g,2d#,p,f,f,f,2d"),
    ("turkishmarch", "turkishmarch:d=16,o=6,b=110:b5,a5,g#5,a5,8c,p,d,c,b5,c,8e,p,f,e,d#,e,b,a,g#,a,b,a,g#,a,4c7,8a,8c7,8b,8a,8g,8a,8b,8a,8g,8a,8b,8a,8g,8f#,4e"),
    ("spring", "spring:d=8,o=6,b=110:c,4e,4e,4e,d,c,4g.,g,f,4e,4e,4e,d,c,4g.,g,f,4e,f,g,4f,4e,2d"),
    ("canon", "canon:d=4,o=6,b=100:f#7,e7,d7,c#7,b,a,b,c#7,d7,c#7,b,a,g,f#,g,e"),
    ("weddingmarch", "weddingmarch:d=4,o=6,b=100:c,f.,8f,2f,c,g.,8e,2f"),
    ("silentnight", "silentnight:d=4,o=6,b=100:g.,8a,g,2e.,g.,8a,g,2e.,2d7,d7,2b.,2c7,c7,2g."),
    ("tannenbaum", "tannenbaum:d=4,o=6,b=110:c,8f.,16f,f.,8g,8a.,16a,a.,8a,8g,8a,a#,e,g,2f"),
    ("ducklings", "ducklings:d=4,o=6,b=120:c,d,e,f,2g,2g,a,a,a,a,1g,a,a,a,a,1g,f,f,f,f,2e,2e,d,d,d,d,1c"),
    ("auldlangsyne", "auldlangsyne:d=4,o=6,b=110:c,f.,8f,f,a,g.,8f,g,a,f.,8f,a,c7,2d7.,d7,c7.,8a,a,f,g.,8f,g,a,f.,8d,d,c,2f."),
    ("yankee", "yankee:d=8,o=6,b=150:c,c,d,e,c,e,4d,c,c,d,e,4c,4b5,c,c,d,e,f,e,d,c,b5,g5,a5,b5,4c,4c"),
    ("entertainer", "entertainer:d=8,o=6,b=140:d,d#,e,4c7,e,4c7,e,2c7,c7,d7,d#7,e7,c7,d7,4e7,b,4d7,2c7"),
    ("taps", "taps:d=4,o=6,b=70:8g5,8g5,2c,8g5,8c,2e,8g5,8c,8e,8g5,8c,8e,8g5,8c,2e,8c,8e,2g,8e,8c,2g5,8g5,8g5,2c"),
    ("greensleeves", "greensleeves:d=4,o=6,b=140:a5,2c,d,e.,8f,e,2d,b5,g5.,8a5,b5,2c,a5,a5.,8g#5,a5,2b5,g#5,2e5"),
    ("susanna", "susanna:d=8,o=6,b=140:c,d,4e,4g,4g.,a,4g,4e,4c.,d,4e,4e,4d,4c,2d"),
    ("marylamb", "marylamb:d=4,o=6,b=140:e,d,c,d,e,e,2e,d,d,2d,e,g,2g,e,d,c,d,e,e,e,e,d,d,e,d,1c"),
    ("londonbridge", "londonbridge:d=4,o=6,b=140:g.,8a,g,f,e,f,2g,d,e,2f,e,f,2g,g.,8a,g,f,e,f,2g,2d,2g,e,2c."),
    ("saints", "saints:d=4,o=6,b=160:c,e,f,1g,p,c,e,f,1g,p,c,e,f,2g,2e,2c,2e,1d"),
    ("charge", "charge:d=8,o=6,b=180:g5,c,e,4g,e,2g"),
];
/// Parse RTTTL into (frequency in Hz, 0 = pause; length in ms). None when it is not RTTTL.
fn parse_rtttl(text: &str) -> Option<Vec<(f32, u64)>> {
    let mut parts = text.splitn(3, ':');
    let (_name, defs, notes) = (parts.next()?, parts.next()?, parts.next()?);
    let (mut d, mut o, mut b) = (4u32, 6i32, 63u32);
    for kv in defs.split(',') {
        let (k, v) = kv.trim().split_once('=')?;
        match k.trim() {
            "d" => d = v.trim().parse().ok()?,
            "o" => o = v.trim().parse().ok()?,
            "b" => b = v.trim().parse().ok()?,
            _ => {}
        }
    }
    if !(1..=64).contains(&d) || !(20..=900).contains(&b) {
        return None;
    }
    let whole = 240_000.0 / b as f32; // ms of a whole note
    let mut out = Vec::new();
    for n in notes.split(',') {
        let n = n.trim().to_ascii_lowercase();
        if n.is_empty() {
            continue;
        }
        let digits = n.chars().take_while(|c| c.is_ascii_digit()).count();
        let len: u32 = if digits > 0 { n[..digits].parse().ok()? } else { d };
        let mut rest = n[digits..].chars().peekable();
        let semis = match rest.next()? {
            'c' => 0,
            'd' => 2,
            'e' => 4,
            'f' => 5,
            'g' => 7,
            'a' => 9,
            'b' | 'h' => 11,
            'p' => -1,
            _ => return None,
        };
        let mut sharp = 0;
        let mut dotted = false;
        let mut oct = o;
        for c in rest {
            match c {
                '#' => sharp = 1,
                '.' => dotted = true,
                '0'..='9' => oct = c as i32 - '0' as i32,
                _ => return None,
            }
        }
        if !(1..=64).contains(&len) {
            return None;
        }
        let ms = whole / len as f32 * if dotted { 1.5 } else { 1.0 };
        let hz = if semis < 0 { 0.0 } else { 440.0 * 2f32.powf((semis + sharp - 9) as f32 / 12.0 + (oct - 4) as f32) };
        out.push((hz, ms as u64));
        if out.len() > 300 {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}
static TUNE_PLAYING: AtomicBool = AtomicBool::new(false);
fn play_tune(spec: &str) {
    let spec = spec.trim();
    // the Dutch names earlier versions had are still taken
    const OLD: [(&str, &str); 17] = [("klaar", "done"), ("vrolijk", "happy"), ("slaap", "sleep"), ("bergkoning", "mountainking"), ("vaderjacob", "frerejacques"), ("kortjakje", "twinkle"), ("jarig", "birthday"), ("vijfde", "fifth"), ("turks", "turkishmarch"), ("lente", "spring"), ("bruidsmars", "weddingmarch"), ("stillenacht", "silentnight"), ("dennenboom", "tannenbaum"), ("eendjes", "ducklings"), ("taptoe", "taps"), ("lammetje", "marylamb"), ("aanval", "charge")];
    let spec = OLD.iter().find(|(o, _)| *o == spec).map(|(_, n)| *n).unwrap_or(spec);
    let rtttl = TUNES.iter().find(|(n, _)| *n == spec).map(|(_, r)| *r).unwrap_or(spec);
    let Some(notes) = parse_rtttl(rtttl) else {
        eprintln!("tune: not a known name and not RTTTL");
        return;
    };
    // one tune at a time; a second request while one plays is dropped
    if TUNE_PLAYING.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let mut total = 0u64;
        for (hz, ms) in notes {
            // at most 30 s of music
            if total + ms > 30_000 {
                break;
            }
            total += ms;
            if hz > 0.0 {
                piezo_tone(hz.clamp(100.0, 8000.0));
                std::thread::sleep(Duration::from_millis(ms.saturating_sub(12)));
                piezo_tone(0.0);
                std::thread::sleep(Duration::from_millis(12.min(ms))); // a small gap between notes
            } else {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
        piezo_tone(0.0);
        TUNE_PLAYING.store(false, Ordering::Relaxed);
    });
}
// Raw ADBS counts for one settings step (6 felt twitchy, 14 still too quick).
const ROT_PER_STEP: i32 = 28;
const ROT_HYSTERESIS: i32 = 20;
// ---- backlight + panel power (lm3530 via /sys/class/backlight) ----
fn backlight_path() -> Option<String> {
    for e in std::fs::read_dir("/sys/class/backlight").ok()?.flatten() {
        return Some(e.path().to_string_lossy().into_owned());
    }
    None
}
fn backlight_set(bp: &Option<String>, level: u32) {
    if let Some(bp) = bp {
        let _ = std::fs::write(format!("{}/brightness", bp), level.to_string());
    }
}
// FBIOBLANK: 0 = unblank, 4 = FB_BLANK_POWERDOWN (panel off = real power saving).
fn fb_blank(f: &File, blank: bool) {
    let arg: libc::c_int = if blank { 4 } else { 0 };
    unsafe { libc::ioctl(f.as_raw_fd(), 0x4611, arg) };
}
fn present(fb: &Fb, f: &mut File, map: &mut Option<*mut u8>, len: usize) {
    let src = unsafe { std::slice::from_raw_parts(fb.buf.as_ptr() as *const u8, len) };
    match map {
        Some(p) => unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), *p, len) },
        None => { let _ = fb.write_to(f); }
    }
}
// ======================================================================================
// The generic thermostat screen: a dial like the thermostat card of Home Assistant.
// ======================================================================================
const RING_REV_DEFAULT: i32 = 1300;
static RING_REV: AtomicI32 = AtomicI32::new(RING_REV_DEFAULT); // kept in the settings file for the web page
static ROW: AtomicU32 = AtomicU32::new(0); // cursor in the menu and on the settings page
static EDIT: AtomicBool = AtomicBool::new(false); // a value is selected: the ring changes it


// pages
const P_DIAL: usize = 0;
const P_MENU: usize = 1;
const P_STATUS: usize = 2;
const P_INSTALL: usize = 3;
const P_SETTINGS: usize = 4;
const P_CONFIRM: usize = 5;
const P_SPLASH: usize = 6;
const SPLASH_MS: u64 = 2500;

/// What the dial shows, gathered in one place. J49_DEMO=1 gives sample values (screenshots on a PC).
struct View {
    room: Option<(f32, f32)>,
    heat_mode: bool,
    away: bool,
    setpoint: f32,
    away_temp: f32,
    demand: bool,
    relay: bool,
    wait: u64,
    frost: bool,
    backplate: bool,
    next: Option<(u64, u64)>, // hhmm, tenths
}
fn view() -> View {
    if std::env::var("J49_DEMO").is_ok() {
        return View {
            room: Some((19.4, 44.0)),
            heat_mode: true,
            away: false,
            setpoint: 20.5,
            away_temp: 15.0,
            demand: true,
            relay: true,
            wait: 0,
            frost: false,
            backplate: true,
            next: Some((2230, 160)),
        };
    }
    let heat = bpd_file("heat", 30);
    let g = |k: &str| heat.as_ref().and_then(|m| m.get(k).copied());
    View {
        room: room_temp(),
        heat_mode: HEAT_MODE.load(Ordering::Relaxed),
        away: AWAY.load(Ordering::Relaxed),
        setpoint: SETPOINT_X10.load(Ordering::Relaxed) as f32 / 10.0,
        away_temp: AWAY_X10.load(Ordering::Relaxed) as f32 / 10.0,
        demand: g("demand") == Some(1),
        relay: g("relay") == Some(1),
        wait: g("wait").unwrap_or(0),
        frost: g("frost") == Some(1),
        backplate: heat.is_some(),
        next: if SCHEDULE_ON.load(Ordering::Relaxed) && g("sched") == Some(1) { Some((g("next_hhmm").unwrap_or(0), g("next_x10").unwrap_or(0))) } else { None },
    }
}

/// Part of a ring between two angles (degrees, clockwise from straight down-left; see dial_angle).
fn arc(fb: &mut Fb, a0: f32, a1: f32, r_in: f32, r_out: f32, c: u32) {
    let (lo, hi) = (r_in * r_in, r_out * r_out);
    for y in (CY - r_out) as i32..=(CY + r_out) as i32 {
        for x in (CX - r_out) as i32..=(CX + r_out) as i32 {
            let (dx, dy) = (x as f32 + 0.5 - CX, y as f32 + 0.5 - CY);
            let d2 = dx * dx + dy * dy;
            if d2 < lo || d2 > hi {
                continue;
            }
            // 0 degrees = straight down, growing clockwise
            let mut a = (-dx).atan2(dy).to_degrees();
            if a < 0.0 {
                a += 360.0;
            }
            if a >= a0 && a <= a1 {
                fb.put(x, y, c);
            }
        }
    }
}
// The dial runs from 45 degrees (bottom left) clockwise over the top to 315 degrees (bottom
// right), 270 degrees for 9..32 degrees C; the gap at the bottom holds the clock.
const DIAL_A0: f32 = 45.0;
const DIAL_A1: f32 = 315.0;
fn dial_angle(temp: f32) -> f32 {
    let f = ((temp - SETPOINT_MIN_X10 as f32 / 10.0) / ((SETPOINT_MAX_X10 - SETPOINT_MIN_X10) as f32 / 10.0)).clamp(0.0, 1.0);
    DIAL_A0 + f * (DIAL_A1 - DIAL_A0)
}
fn dial_point(angle: f32, r: f32) -> (f32, f32) {
    let a = angle.to_radians();
    (CX - r * a.sin(), CY + r * a.cos())
}
fn wifi_icon(fb: &mut Fb, x: i32, y: i32) {
    let link = WIFI_LINK.load(Ordering::Relaxed);
    let bars = if link == 2 { WIFI_BARS.load(Ordering::Relaxed) } else { 0 };
    for k in 0..3 {
        let h = 4 + k * 3;
        let c = if link == 0 { c::error() } else if k < bars { c::text() } else { c::line() };
        fb.rect(x + k * 5, y + 10 - h, 3, h, c);
    }
}
fn flame_icon(fb: &mut Fb, cx: i32, cy: i32, c: u32) {
    // a small flame: a drop with a notch
    for (dy, half) in [(-7, 0), (-6, 1), (-5, 1), (-4, 2), (-3, 2), (-2, 3), (-1, 4), (0, 4), (1, 4), (2, 4), (3, 3), (4, 2)] {
        fb.rect(cx - half, cy + dy, half * 2 + 1, 1, c);
    }
    for (dy, half) in [(0, 0), (1, 1), (2, 1), (3, 1)] {
        fb.rect(cx - half, cy + dy, half * 2 + 1, 1, c::bg());
    }
}
fn render_dial() -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    let v = view();
    let target = if v.away { v.away_temp } else { v.setpoint };
    let heating = v.demand && v.relay;
    let col = if !v.heat_mode {
        c::dim()
    } else if heating {
        c::heat()
    } else {
        c::accent()
    };
    // the dial: track, the part up to the target, and the room temperature as a tick
    let r_in = 152.0 - c::DIAL.load(Ordering::Relaxed) as f32;
    arc(&mut fb, DIAL_A0, DIAL_A1, r_in, 152.0, c::surface2());
    if v.heat_mode {
        arc(&mut fb, DIAL_A0, dial_angle(target), r_in, 152.0, blend(c::bg(), col, if heating { 1.0 } else { 0.75 }));
        let (hx, hy) = dial_point(dial_angle(target), (r_in + 152.0) / 2.0);
        fb.disc(hx, hy, (152.0 - r_in) / 2.0 + 2.0, c::strong());
        fb.disc(hx, hy, (152.0 - r_in) / 2.0 - 1.0, col);
    }
    if let Some((room, _)) = v.room {
        let a = dial_angle(room);
        arc(&mut fb, (a - 0.9).max(DIAL_A0), (a + 0.9).min(DIAL_A1), r_in - 10.0, r_in - 1.0, c::info());
    }
    // what the heating does
    let wait = format!("{} {}:{:02}", t("screen.wait"), v.wait / 60, v.wait % 60);
    let (state, scol): (&str, u32) = if !v.backplate {
        (t("screen.no_backplate"), c::error())
    } else if v.frost {
        (t("screen.frost_protection"), c::heat())
    } else if !v.heat_mode {
        (t("screen.off"), c::dim())
    } else if v.room.is_none() {
        (t("screen.no_temperature"), c::error())
    } else if heating {
        (t("screen.heating"), c::heat())
    } else if v.demand {
        (t("screen.calling_for_heat"), c::accent())
    } else if v.wait > 0 {
        (wait.as_str(), c::accent())
    } else if v.away {
        (t("screen.away"), c::info())
    } else {
        (t("screen.idle"), c::ok())
    };
    fb.text_center(160, 78, state, 1, scol);
    if heating || v.frost {
        flame_icon(&mut fb, 160, 62, c::heat());
    }
    // the target, large
    let touched = unix_now() <= SP_TOUCHED.load(Ordering::Relaxed) + 3;
    if v.heat_mode {
        let (whole, rest) = temp_parts(target);
        let tc = if touched { c::strong() } else { c::text() };
        fb.text_center(152, 100, &whole, 7, tc);
        fb.text(152 + Fb::text_w(&whole, 7) / 2 + 8, 100, &rest, 3, tc);
    } else {
        fb.text_center(160, 108, t("screen.off"), 6, c::dim());
    }
    // the room, smaller
    match v.room {
        Some((temp, rh)) => {
            fb.text_center(160, 172, &fmt_temp(temp), 3, c::info());
            fb.text_center(160, 200, &format!("{} {:.0}%", t("screen.now"), rh), 1, c::dim());
        }
        None => fb.text_center(160, 172, "--", 3, c::dim()),
    }
    if let Some((hm, x)) = v.next {
        fb.text_center(160, 218, &format!("{}  {}", fmt_clock((hm / 100) as i32, (hm % 100) as i32), fmt_temp(x as f32 / 10.0)), 1, c::dim());
    }
    // the gap at the bottom: clock and WiFi
    if let Some((_, _, _, h, m)) = local_now() {
        fb.text_center(160, 262, &fmt_clock(h, m), 2, c::text());
    }
    wifi_icon(&mut fb, 153, 284);
    fb
}

/// One row of a list: a label, a value, a highlight when the cursor is on it.
fn list_row(fb: &mut Fb, y: i32, label: &str, value: &str, vcol: u32, selected: bool, editing: bool) {
    let (x, w, h) = (46, 228, 26);
    fb.rect(x, y, w, h, if selected { c::surface2() } else { c::surface() });
    if selected {
        let c = if editing { c::heat() } else { c::accent() };
        fb.rect(x, y, 3, h, c);
    }
    fb.text(x + 12, y + 9, label, 1, if selected { c::strong() } else { c::text() });
    fb.text(x + w - 10 - Fb::text_w(value, 1), y + 9, value, 1, vcol);
}
fn on_off(on: bool) -> (&'static str, u32) {
    if on {
        (t("screen.on"), c::ok())
    } else {
        (t("screen.off"), c::dim())
    }
}
const MENU_ROWS: u32 = 6;
fn render_menu() -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    fb.text_center(160, 40, t("screen.menu"), 2, c::accent());
    let row = ROW.load(Ordering::Relaxed);
    let heat = HEAT_MODE.load(Ordering::Relaxed);
    let (s, sc) = on_off(SCHEDULE_ON.load(Ordering::Relaxed));
    let (a, ac) = on_off(AWAY.load(Ordering::Relaxed));
    let rows: [(&str, &str, u32); MENU_ROWS as usize] = [
        (t("screen.mode"), if heat { t("screen.heat") } else { t("screen.off") }, if heat { c::heat() } else { c::dim() }),
        (t("screen.schedule"), s, sc),
        (t("screen.away"), a, ac),
        (t("screen.status"), ">", c::dim()),
        (t("screen.settings"), ">", c::dim()),
        (t("screen.back"), "", c::dim()),
    ];
    for (i, (l, v, col)) in rows.iter().enumerate() {
        list_row(&mut fb, 72 + i as i32 * 31, l, v, *col, i as u32 == row, false);
    }
    fb.text_center(160, 272, t("screen.turn_move_click_choose"), 1, c::dim());
    fb
}
const SETTINGS_ROWS: u32 = 7;
fn render_settings() -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    fb.text_center(160, 40, t("screen.settings"), 2, c::accent());
    let (row, edit) = (ROW.load(Ordering::Relaxed), EDIT.load(Ordering::Relaxed));
    let (w, wc) = on_off(WIFI.load(Ordering::Relaxed));
    let b = format!("{}%", BRIGHT.load(Ordering::Relaxed) * 100 / 120);
    let vol = format!("{}", VOLUME.load(Ordering::Relaxed));
    let theme = theme_title();
    let rows: [(&str, &str, u32); SETTINGS_ROWS as usize] = [
        (t("screen.brightness"), b.as_str(), c::text()),
        (t("screen.volume"), vol.as_str(), c::text()),
        (t("screen.wifi"), w, wc),
        (t("screen.language"), t("lang.name"), c::text()),
        (t("screen.theme"), theme.as_str(), c::text()),
        (t("screen.restart"), if edit && row == 5 { t("screen.click_yes") } else { "" }, c::error()),
        (t("screen.back"), "", c::dim()),
    ];
    for (i, (l, v, col)) in rows.iter().enumerate() {
        list_row(&mut fb, 62 + i as i32 * 29, l, v, *col, i as u32 == row, edit && i as u32 == row);
    }
    fb.text_center(160, 272, t("screen.click_change_turn_value"), 1, c::dim());
    fb
}
fn render_status() -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    fb.text_center(160, 40, t("screen.status"), 2, c::accent());
    let ip = iface_ip("wlan0");
    let room = room_temp();
    let motion = bpd_file("pir", 30).and_then(|m| m.get("motion_t").copied()).map(|t| unix_now() <= t + 30);
    let light = bpd_file("als", 30).and_then(|m| m.get("vis").copied());
    let batt = batt_mv().map(|mv| format!("{}.{:02} V", mv / 1000, (mv % 1000) / 10)).unwrap_or_else(|| "--".into());
    let rows: [(&str, String, u32); 7] = [
        (t("screen.wifi"), ip.clone().unwrap_or_else(|| t("screen.not_connected").into()), if ip.is_some() { c::ok() } else { c::error() }),
        (t("screen.backplate"), if bpd_file("pir", 20).is_some() { "OK".into() } else { t("screen.no_data").into() }, if bpd_file("pir", 20).is_some() { c::ok() } else { c::error() }),
        (t("screen.room"), room.map(|(temp, rh)| format!("{} {}  {:.0}%", fmt_temp(temp), t("locale.unit"), rh)).unwrap_or_else(|| "--".into()), c::text()),
        (t("screen.motion"), match motion { Some(true) => t("screen.yes").into(), Some(false) => t("screen.no").into(), None => "--".to_string() }, c::text()),
        (t("screen.light"), light.map(|v| v.to_string()).unwrap_or_else(|| "--".into()), c::text()),
        (t("screen.battery"), batt, c::text()),
        (t("screen.uptime"), uptime_str().trim_start_matches("UP ").to_string(), c::text()),
    ];
    for (i, (l, v, col)) in rows.iter().enumerate() {
        list_row(&mut fb, 66 + i as i32 * 28, l, v, *col, false, false);
    }
    fb.text_center(160, 272, t("screen.click_back"), 1, c::dim());
    fb
}
fn render_confirm() -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    let Some((code, ip, at)) = CONFIRM.lock().unwrap().clone() else { return fb };
    let yes = CONFIRM_YES.load(Ordering::Relaxed);
    fb.text_center(160, 44, t("screen.web_login"), 2, c::accent());
    fb.text_center(160, 72, t("screen.is_this_the_code_in_your_browser"), 1, c::text());
    fb.rect(70, 90, 180, 62, c::surface2());
    fb.text_center(160, 104, &code, 5, c::strong());
    fb.text_center(160, 164, &format!("{} {ip}", t("screen.from")), 1, c::dim());
    for (x, label, sel, col) in [(40, t("screen.refuse"), !yes, c::error()), (164, t("screen.allow"), yes, c::ok())] {
        fb.rect(x, 186, 116, 40, if sel { blend(col, c::black(), 0.55) } else { c::surface2() });
        if sel {
            fb.rect(x, 186, 116, 3, col);
        }
        fb.text_center(x + 58, 199, label, 2, if sel { c::strong() } else { c::dim() });
    }
    fb.text_center(160, 244, t("screen.turn_choose_click_ok"), 1, c::dim());
    let left = (at + WEB_LOGIN_SECS).saturating_sub(unix_now()) as i32;
    let w = (left * 120 / WEB_LOGIN_SECS as i32).clamp(0, 120);
    fb.rect(100, 264, 120, 3, c::line());
    fb.rect(100, 264, w, 3, c::info());
    fb
}
/// The installer's screen (--screen install): phase, progress and the last message.
/// The start screen: the name, a ring that closes while the program starts, the version.
/// `progress` runs from 0 to 1.
fn render_splash(progress: f32) -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    let p = progress.clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - p) * (1.0 - p);
    let r_in = 152.0 - c::DIAL.load(Ordering::Relaxed) as f32;
    arc(&mut fb, DIAL_A0, DIAL_A1, r_in, 152.0, c::surface2());
    arc(&mut fb, DIAL_A0, DIAL_A0 + (DIAL_A1 - DIAL_A0) * ease, r_in, 152.0, blend(c::bg(), c::accent(), 0.35 + 0.65 * ease));
    let title = SPLASH_TITLE.lock().unwrap().clone();
    fb.text_center(160, 112, &title, 7, blend(c::bg(), c::strong(), (p * 3.0).min(1.0)));
    fb.text_center(160, 186, t("screen.thermostat"), 2, blend(c::bg(), c::accent(), (p * 3.0 - 0.5).clamp(0.0, 1.0)));
    let rel = std::fs::read_to_string("/etc/j49-release").unwrap_or_default();
    let ver = rel.lines().find_map(|l| l.strip_prefix("commit=")).map(|v| v.trim().chars().take(8).collect::<String>()).unwrap_or_default();
    if !ver.is_empty() {
        fb.text_center(160, 232, &ver.to_uppercase(), 1, c::dim());
    }
    fb
}
fn render_install(st: &InstallState) -> Fb {
    let mut fb = Fb::new();
    fb.fill(c::bg());
    fb.text_center(160, 56, "J49", 3, c::accent());
    fb.text_center(160, 96, if st.phase.is_empty() { t("screen.install") } else { st.phase.as_str() }, 2, c::strong());
    arc(&mut fb, DIAL_A0, DIAL_A1, 140.0, 152.0, c::surface2());
    arc(&mut fb, DIAL_A0, DIAL_A0 + (DIAL_A1 - DIAL_A0) * st.pct / 100.0, 140.0, 152.0, if st.code.is_empty() { c::ok() } else { c::error() });
    fb.text_center(160, 136, &format!("{:.0}%", st.pct), 5, c::text());
    fb.text_center(160, 192, &st.target, 1, c::dim());
    fb.text_center(160, 208, &st.info, 1, c::text());
    if !st.code.is_empty() {
        fb.text_center(160, 232, &format!("{} {}", t("screen.error"), st.code), 2, c::error());
    }
    fb
}

// ---- settings file: brightness, volume, wifi, ring counts (kept for the web page) ----
fn load_settings() {
    if let Ok(s) = std::fs::read_to_string(format!("{}/settings", SETTINGS_DIR)) {
        for l in s.lines() {
            if let Some((k, v)) = l.split_once('=') {
                if let Ok(n) = v.trim().parse::<u32>() {
                    match k.trim() {
                        "wifi" => WIFI.store(n != 0, Ordering::Relaxed),
                        "ringrev" => RING_REV.store((n as i32).clamp(200, 100_000), Ordering::Relaxed),
                        "brightness" => BRIGHT.store(n.clamp(10, 120), Ordering::Relaxed),
                        "volume" => VOLUME.store(n.min(10), Ordering::Relaxed),
                        _ => {}
                    }
                }
            }
        }
    }
}
fn save_settings() {
    let _ = std::fs::create_dir_all(SETTINGS_DIR);
    let _ = std::fs::write(
        format!("{}/settings", SETTINGS_DIR),
        format!(
            "brightness={}\nvolume={}\nwifi={}\nringrev={}\n",
            BRIGHT.load(Ordering::Relaxed),
            VOLUME.load(Ordering::Relaxed),
            WIFI.load(Ordering::Relaxed) as u32,
            RING_REV.load(Ordering::Relaxed)
        ),
    );
}
/// External power: VBUS from the TWL4030 (USB) or frames from the backplate in the last 5 s.
fn external_power() -> bool {
    let vbus = read_trim("/sys/devices/platform/68000000.ocp/48070000.i2c/i2c-0/0-0048/48070000.i2c:twl@48:twl4030-usb/vbus")
        .map(|s| s == "on")
        .unwrap_or(false);
    vbus || bpd_file("pir", 5).is_some()
}

// Once a second: settings other programs changed, WiFi for the icon, and our own state for them.
fn spawn_housekeeping() {
    std::thread::spawn(|| {
        let stamp = || std::fs::metadata(format!("{}/settings", SETTINGS_DIR)).and_then(|m| m.modified()).ok();
        let mut seen = stamp();
        let ui_stamp = || std::fs::metadata(UI_FILE).and_then(|m| m.modified()).ok();
        let mut ui_seen = ui_stamp();
        let mut last_asleep = None;
        // what this screen can do, for j49-mqtt: tunes yes, speech no
        let _ = std::fs::write("/run/j49-screen.caps", "tune\n");
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if unix_now() > SP_TOUCHED.load(Ordering::Relaxed) + 2 {
                load_thermostat();
            }
            let link = if !WIFI.load(Ordering::Relaxed) {
                0
            } else if wlan_up() && iface_ip("wlan0").is_some() {
                2
            } else {
                1
            };
            WIFI_LINK.store(link, Ordering::Relaxed);
            WIFI_BARS.store(match wifi_dbm() { Some(d) if d >= -62 => 3, Some(d) if d >= -74 => 2, _ => 1 }, Ordering::Relaxed);
            let asleep = ASLEEP.load(Ordering::Relaxed);
            if last_asleep != Some(asleep) {
                last_asleep = Some(asleep);
                let _ = std::fs::write("/run/j49-screen.state.tmp", format!("asleep={}\n", asleep as u8));
                let _ = std::fs::rename("/run/j49-screen.state.tmp", "/run/j49-screen.state");
            }
            // the web page changed the language or the theme
            if ui_stamp() != ui_seen {
                ui_seen = ui_stamp();
                load_ui();
            }
            let now = stamp();
            if now != seen {
                seen = now;
                load_settings();
                if !asleep {
                    backlight_set(&backlight_path(), BRIGHT.load(Ordering::Relaxed));
                }
            }
            let _ = std::fs::write("/run/j49-screen.ring", format!("{}\n", RING_TOTAL.load(Ordering::Relaxed)));
        }
    });
}

// Control FIFO: wake, sleep, page N (0 dial, 1 menu, 2 status, 4 settings), tune <name or RTTTL>.
fn spawn_control() {
    std::thread::spawn(|| {
        let path = std::ffi::CString::new(CTRL_FIFO).unwrap();
        unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        loop {
            if let Ok(f) = File::open(CTRL_FIFO) {
                for line in std::io::BufRead::lines(std::io::BufReader::new(f)).flatten() {
                    if let Some(t) = line.strip_prefix("tune ") {
                        play_tune(t);
                    } else if line.trim() == "wake" {
                        WAKE.fetch_add(1, Ordering::Relaxed);
                    } else if line.trim() == "sleep" {
                        SLEEP_NOW.store(true, Ordering::Relaxed);
                    } else if let Some(n) = line.strip_prefix("page ") {
                        if let Ok(n) = n.trim().parse::<u32>() {
                            if n <= 2 || n == 4 {
                                PAGE_REQ.store(n, Ordering::Relaxed);
                                WAKE.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            } else {
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    });
}

fn click(page: &AtomicUsize) {
    let p = page.load(Ordering::Relaxed);
    let row = ROW.load(Ordering::Relaxed);
    match p {
        P_DIAL => {
            ROW.store(0, Ordering::Relaxed);
            page.store(P_MENU, Ordering::Relaxed);
        }
        P_MENU => match row {
            0 => {
                HEAT_MODE.fetch_xor(true, Ordering::Relaxed);
                save_thermostat();
            }
            1 => {
                let on = !SCHEDULE_ON.fetch_xor(true, Ordering::Relaxed);
                if on && !Path::new(SCHEDULE_FILE).exists() {
                    let _ = std::fs::create_dir_all("/var/lib/j49");
                    let _ = std::fs::write(SCHEDULE_FILE, SCHEDULE_DEFAULT);
                }
                let _ = std::fs::remove_file("/var/lib/j49/schedule.state"); // bpd applies the change in force now
                save_thermostat();
            }
            2 => {
                AWAY.fetch_xor(true, Ordering::Relaxed);
                save_thermostat();
            }
            3 => page.store(P_STATUS, Ordering::Relaxed),
            4 => {
                ROW.store(0, Ordering::Relaxed);
                page.store(P_SETTINGS, Ordering::Relaxed);
            }
            _ => page.store(P_DIAL, Ordering::Relaxed),
        },
        P_STATUS => page.store(P_MENU, Ordering::Relaxed),
        P_SETTINGS => {
            let edit = EDIT.load(Ordering::Relaxed);
            match row {
                0 | 1 => {
                    EDIT.store(!edit, Ordering::Relaxed);
                }
                2 => {
                    let on = !WIFI.fetch_xor(true, Ordering::Relaxed);
                    save_settings();
                    std::thread::spawn(move || wifi_apply(on));
                }
                3 => {
                    let cur = UI_LANG.lock().unwrap().clone();
                    set_language(&next_of(list_files("lang", "txt", &BUILTIN_LANGS), &cur));
                    save_ui();
                }
                4 => {
                    let cur = UI_THEME.lock().unwrap().clone();
                    set_theme(&next_of(list_files("themes", "theme", &BUILTIN_THEMES), &cur));
                    save_ui();
                }
                5 => {
                    if edit {
                        save_settings();
                        let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
                    } else {
                        EDIT.store(true, Ordering::Relaxed); // ask first: the next click restarts, a turn cancels
                    }
                }
                _ => {
                    ROW.store(4, Ordering::Relaxed); // the cursor back on "Settings" in the menu
                    page.store(P_MENU, Ordering::Relaxed);
                }
            }
        }
        P_CONFIRM => {
            web_answer(CONFIRM_YES.load(Ordering::Relaxed));
            page.store(P_DIAL, Ordering::Relaxed);
        }
        _ => {}
    }
    piezo_click();
}
/// One step of the ring: +1 clockwise, -1 the other way.
fn turn(page: &AtomicUsize, step: i32) {
    let p = page.load(Ordering::Relaxed);
    match p {
        P_DIAL => {
            if !HEAT_MODE.load(Ordering::Relaxed) {
                return;
            }
            SP_TOUCHED.store(unix_now(), Ordering::Relaxed);
            // somebody turned the ring: home again
            if AWAY.swap(false, Ordering::Relaxed) {
                save_thermostat();
                piezo_beep(2400.0, 40);
                return;
            }
            let old = SETPOINT_X10.load(Ordering::Relaxed);
            let new = (old + step * 5).clamp(SETPOINT_MIN_X10, SETPOINT_MAX_X10);
            SETPOINT_X10.store(new, Ordering::Relaxed);
            if new != old {
                save_thermostat();
                piezo_beep(900.0 + (new - SETPOINT_MIN_X10) as f32 * 8.0, 20);
            } else {
                piezo_beep(300.0, 60);
            }
        }
        P_MENU => {
            let old = ROW.load(Ordering::Relaxed) as i32;
            let new = (old + step).clamp(0, MENU_ROWS as i32 - 1);
            ROW.store(new as u32, Ordering::Relaxed);
            if new != old {
                piezo_beep(1300.0, 15);
            }
        }
        P_SETTINGS => {
            let row = ROW.load(Ordering::Relaxed);
            if !EDIT.load(Ordering::Relaxed) {
                let new = (row as i32 + step).clamp(0, SETTINGS_ROWS as i32 - 1);
                ROW.store(new as u32, Ordering::Relaxed);
                if new != row as i32 {
                    piezo_beep(1300.0, 15);
                }
            } else if row == 0 {
                let b = (BRIGHT.load(Ordering::Relaxed) as i32 + step * 6).clamp(10, 120) as u32;
                BRIGHT.store(b, Ordering::Relaxed);
                backlight_set(&backlight_path(), b);
                save_settings();
            } else if row == 1 {
                let v = (VOLUME.load(Ordering::Relaxed) as i32 + step).clamp(0, 10) as u32;
                VOLUME.store(v, Ordering::Relaxed);
                save_settings();
                piezo_beep(800.0 + v as f32 * 240.0, 30);
            } else {
                EDIT.store(false, Ordering::Relaxed); // a turn cancels the restart question
            }
        }
        P_CONFIRM => {
            if CONFIRM_YES.swap(step > 0, Ordering::Relaxed) != (step > 0) {
                piezo_beep(if step > 0 { 1800.0 } else { 900.0 }, 25);
            }
        }
        _ => {}
    }
}
fn spawn_input(page: Arc<AtomicUsize>, activity: Arc<AtomicU64>) {
    // the ring's click: EV_KEY KEY_POWER on event0
    {
        let (page, activity) = (page.clone(), activity.clone());
        std::thread::spawn(move || {
            if let Ok(mut f) = File::open("/dev/input/event0") {
                let mut b = [0u8; 16];
                while f.read_exact(&mut b).is_ok() {
                    let typ = u16::from_le_bytes([b[8], b[9]]);
                    let code = u16::from_le_bytes([b[10], b[11]]);
                    let val = i32::from_le_bytes([b[12], b[13], b[14], b[15]]);
                    if typ == 1 && code == 116 && val == 1 {
                        // a press on a dark screen only wakes it
                        let was_asleep = ASLEEP.load(Ordering::Relaxed);
                        activity.fetch_add(1, Ordering::Relaxed);
                        if !was_asleep {
                            click(&page);
                        }
                    }
                }
            }
        });
    }
    // the ring's rotation: EV_REL counts on event1 (or event3 on other kernels)
    std::thread::spawn(move || {
        for dev in ["/dev/input/event1", "/dev/input/event3"] {
            if let Ok(mut f) = File::open(dev) {
                let mut b = [0u8; 16];
                let (mut acc, mut last_dir, mut dead) = (0i32, 0i32, 0i32);
                while f.read_exact(&mut b).is_ok() {
                    let typ = u16::from_le_bytes([b[8], b[9]]);
                    let val = i32::from_le_bytes([b[12], b[13], b[14], b[15]]);
                    if typ != 2 {
                        continue;
                    }
                    let was_asleep = ASLEEP.load(Ordering::Relaxed);
                    activity.fetch_add(1, Ordering::Relaxed);
                    if was_asleep {
                        acc = 0;
                        continue;
                    }
                    // as stock: a change of direction first has to cross a dead band, so a
                    // finger resting on the ring does not jitter
                    let d = val.signum();
                    if d != 0 && d != last_dir {
                        last_dir = d;
                        dead = ROT_HYSTERESIS;
                        acc = 0;
                    }
                    let mut v = val;
                    if dead > 0 {
                        let eat = dead.min(v.abs());
                        dead -= eat;
                        v -= d * eat;
                    }
                    RING_TOTAL.fetch_add(-v, Ordering::Relaxed);
                    acc += v;
                    while acc.abs() >= ROT_PER_STEP {
                        // positive sensor counts are counter-clockwise
                        let dir = if acc > 0 { 1 } else { -1 };
                        acc -= dir * ROT_PER_STEP;
                        turn(&page, -dir);
                    }
                }
                return;
            }
        }
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned());
    let dev = arg("--fb").unwrap_or_else(|| "/dev/fb0".into());
    let screen = arg("--screen").unwrap_or_else(|| "home".into());
    let state_path = arg("--state").unwrap_or_else(|| "/run/install.state".into());
    let idle_secs: u64 = arg("--idle").and_then(|s| s.parse().ok()).unwrap_or(25);
    let once = args.iter().any(|a| a == "--once");
    let install_mode = screen == "install";

    let mut f = match OpenOptions::new().read(true).write(true).open(&dev) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("j49-screen: open {dev}: {e}");
            std::process::exit(1);
        }
    };
    // Keep the panel lit: unblank now and switch off the kernel console blank timer.
    unsafe { libc::ioctl(f.as_raw_fd(), 0x4611, 0 as libc::c_int) }; // FBIOBLANK, UNBLANK
    if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty0") {
        let _ = tty.write_all(b"\x1b[9;0]");
    }
    let len = W * H * 4;
    let mut map: Option<*mut u8> = unsafe {
        let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, f.as_raw_fd(), 0);
        if p == libc::MAP_FAILED {
            None
        } else {
            Some(p as *mut u8)
        }
    };

    let page = Arc::new(AtomicUsize::new(match screen.as_str() {
        "menu" => P_MENU,
        "status" => P_STATUS,
        "settings" => P_SETTINGS,
        "install" => P_INSTALL,
        "splash" => P_SPLASH,
        // a normal start begins with the start screen; a screenshot (--once) goes to the dial
        _ if !once => P_SPLASH,
        _ => P_DIAL,
    }));
    let started = std::time::Instant::now();
    let act0 = WAKE.load(Ordering::Relaxed);
    load_ui();
    load_settings();
    load_thermostat();
    let activity = Arc::new(AtomicU64::new(0));
    if !once {
        if WIFI.load(Ordering::Relaxed) {
            rfkill_wlan(false); // a soft block left behind must not keep WiFi off
        }
        spawn_housekeeping();
        spawn_control();
        spawn_motion_wake();
        spawn_web_confirm();
        spawn_input(page.clone(), activity.clone());
    }
    let bl = backlight_path();
    fb_blank(&f, false);
    backlight_set(&bl, BRIGHT.load(Ordering::Relaxed));

    let mut seen_act = activity.load(Ordering::Relaxed);
    let mut last_active = std::time::Instant::now();
    let mut idle = false;
    loop {
        let f0 = std::time::Instant::now();
        let a = activity.load(Ordering::Relaxed).wrapping_add(WAKE.load(Ordering::Relaxed));
        if WANT_HOME.swap(false, Ordering::Relaxed) {
            page.store(P_DIAL, Ordering::Relaxed);
            EDIT.store(false, Ordering::Relaxed);
        }
        if a != seen_act {
            seen_act = a;
            last_active = f0;
            if idle {
                idle = false;
                backlight_set(&bl, BRIGHT.load(Ordering::Relaxed));
            }
        }
        // Idle: backlight off and no drawing. The panel itself stays powered (the s6d05a1 does
        // not come back reliably from power-down). The installer screen never sleeps.
        ASLEEP.store(idle, Ordering::Relaxed);
        if !idle && !once && !install_mode && (SLEEP_NOW.swap(false, Ordering::Relaxed) || last_active.elapsed() >= Duration::from_secs(idle_secs)) {
            idle = true;
            backlight_set(&bl, 0);
            // back to the dial for the next time it lights up
            page.store(P_DIAL, Ordering::Relaxed);
            EDIT.store(false, Ordering::Relaxed);
        }
        if idle {
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let req = PAGE_REQ.swap(u32::MAX, Ordering::Relaxed);
        if req != u32::MAX {
            page.store(req as usize, Ordering::Relaxed);
        }
        // an open web login question owns the screen
        if CONFIRM.lock().unwrap().is_some() {
            page.store(P_CONFIRM, Ordering::Relaxed);
        } else if page.load(Ordering::Relaxed) == P_CONFIRM {
            page.store(P_DIAL, Ordering::Relaxed);
        }
        // the start screen ends by itself, or at the first touch of the ring
        if page.load(Ordering::Relaxed) == P_SPLASH && !once && (started.elapsed() >= Duration::from_millis(SPLASH_MS) || a != act0) {
            page.store(P_DIAL, Ordering::Relaxed);
        }
        let splash = page.load(Ordering::Relaxed) == P_SPLASH;
        let fb = match page.load(Ordering::Relaxed) {
            P_SPLASH => render_splash(if once { 1.0 } else { started.elapsed().as_millis() as f32 / (SPLASH_MS as f32 * 0.8) }),
            P_MENU => render_menu(),
            P_STATUS => render_status(),
            P_SETTINGS => render_settings(),
            P_CONFIRM => render_confirm(),
            P_INSTALL => render_install(&read_install_state(&state_path)),
            _ => render_dial(),
        };
        present(&fb, &mut f, &mut map, len);
        if once {
            break;
        }
        // nothing animates: draw often only while somebody is at the ring, and slower on battery
        let busy = splash || last_active.elapsed() < Duration::from_secs(3);
        let budget = Duration::from_millis(if busy { 50 } else if external_power() { 250 } else { 500 });
        if let Some(rem) = budget.checked_sub(f0.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dial_maps_the_range() {
        assert_eq!(dial_angle(9.0), DIAL_A0);
        assert_eq!(dial_angle(32.0), DIAL_A1);
        assert!((dial_angle(20.5) - 180.0).abs() < 0.1); // the middle of the range is straight up
        // straight up is above the centre, the ends are below it
        assert!(dial_point(180.0, 100.0).1 < CY - 99.0);
        assert!(dial_point(DIAL_A0, 100.0).1 > CY && dial_point(DIAL_A0, 100.0).0 < CX);
        assert!(dial_point(DIAL_A1, 100.0).0 > CX);
    }
    #[test]
    fn tunes_parse() {
        for (name, rtttl) in TUNES {
            let notes = parse_rtttl(rtttl).unwrap_or_else(|| panic!("{name} does not parse"));
            assert!(notes.iter().map(|n| n.1).sum::<u64>() <= 30_000, "{name} is too long");
        }
        assert!(parse_rtttl("ding").is_none());
    }
}
