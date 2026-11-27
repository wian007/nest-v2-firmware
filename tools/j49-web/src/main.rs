// j49-web: a small web page for the thermostat (temperature, away, schedule, MQTT broker).
//
// Login has no password. A browser that wants in gets a 4-digit code; j49-screen shows the same
// code on the device and somebody there allows or refuses it with the ring. Allowed = that
// browser holds a session cookie for SESSION_SECS.
//
// Like j49-mqtt it only reads and writes the thermostat's files (docs/thermostat.md):
//   /var/lib/j49/{thermostat,schedule,mqtt.conf}, /run/bpd/heat, /run/j49-mqtt.status
// and talks to j49-screen through /run/j49-web/{pending,answer}.
//
// Plain HTTP, no TLS: fine on the home network, not for the internet. The broker password can be
// written through the page but is never sent back to a browser. No dependencies.
// J49_ROOT=<dir> prefixes every path and J49_WEB_PORT sets the port, for tests on a PC.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const PAGE: &str = include_str!("page.html");

// ---- texts, locale and theme (shared with j49-screen; see docs/thermostat.md) ----
// One file per language (tools/lang/<code>.txt) and per theme (tools/themes/<name>.theme).
// English, Dutch and three themes are built in; files in /var/lib/j49 and /usr/share/j49 add
// to them or replace them. /var/lib/j49/ui holds the choice: lang=<code>, theme=<name>.
const UI_FILE: &str = "/var/lib/j49/ui";
const BUILTIN_LANGS: [(&str, &str); 2] = [("en", include_str!("../../lang/en.txt")), ("nl", include_str!("../../lang/nl.txt"))];
const BUILTIN_THEMES: [(&str, &str); 4] = [
    ("default", include_str!("../../themes/default.theme")),
    ("home-assistant", include_str!("../../themes/home-assistant.theme")),
    ("home-assistant-light", include_str!("../../themes/home-assistant-light.theme")),
    ("light", include_str!("../../themes/light.theme")),
];
/// The start screens j49-screen can show (its SPLASH_STYLES).
const SPLASH_STYLES: [&str; 3] = ["name", "flame", "none"];
const THEME_COLORS: [&str; 12] = ["bg", "surface", "surface2", "line", "dim", "text", "strong", "accent", "heat", "ok", "error", "info"];
type Texts = HashMap<&'static str, &'static str>;
/// (the UI file as it was read, chosen language, English)
static TEXTS: Mutex<Option<(String, &'static Texts, &'static Texts)>> = Mutex::new(None);

fn parse_texts(text: String) -> &'static Texts {
    let text: &'static str = Box::leak(text.into_boxed_str());
    Box::leak(Box::new(
        text.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect(),
    ))
}
fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
fn find_file(kind: &str, name: &str, ext: &str, builtin: &[(&str, &str)]) -> Option<String> {
    if !safe_name(name) {
        return None;
    }
    for dir in ["/var/lib/j49", "/usr/share/j49"] {
        if let Ok(s) = std::fs::read_to_string(p(&format!("{dir}/{kind}/{name}.{ext}"))) {
            return Some(s);
        }
    }
    builtin.iter().find(|(n, _)| *n == name).map(|(_, t)| t.to_string())
}
fn list_files(kind: &str, ext: &str, builtin: &[(&str, &str)]) -> Vec<String> {
    let mut v: Vec<String> = builtin.iter().map(|(n, _)| n.to_string()).collect();
    for dir in ["/usr/share/j49", "/var/lib/j49"] {
        let Ok(rd) = std::fs::read_dir(p(&format!("{dir}/{kind}"))) else { continue };
        let mut found: Vec<String> =
            rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(&format!(".{ext}")).map(String::from))).filter(|n| safe_name(n)).collect();
        found.sort();
        for n in found {
            if !v.contains(&n) {
                v.push(n);
            }
        }
    }
    v
}
/// (lang, theme) as chosen, each one that exists.
fn ui_choice() -> (String, String) {
    let m = kv(UI_FILE);
    let pick = |key: &str, kind: &str, ext: &str, builtin: &[(&str, &str)], default: &str| {
        m.get(key).filter(|n| find_file(kind, n, ext, builtin).is_some()).cloned().unwrap_or_else(|| default.to_string())
    };
    (pick("lang", "lang", "txt", &BUILTIN_LANGS, "en"), pick("theme", "themes", "theme", &BUILTIN_THEMES, "default"))
}
/// The texts of the chosen language and English; read again when the choice changes. A changed
/// language file leaks the old texts (a few kB, and only when somebody changes the language).
fn texts() -> (&'static Texts, &'static Texts) {
    let raw = std::fs::read_to_string(p(UI_FILE)).unwrap_or_default();
    let mut g = TEXTS.lock().unwrap();
    if let Some((old, chosen, en)) = g.as_ref() {
        if *old == raw {
            return (chosen, en);
        }
    }
    let en = match g.as_ref() {
        Some((_, _, en)) => *en,
        None => parse_texts(BUILTIN_LANGS[0].1.to_string()),
    };
    let lang = ui_choice().0;
    let chosen = if lang == "en" { en } else { find_file("lang", &lang, "txt", &BUILTIN_LANGS).map(parse_texts).unwrap_or(en) };
    *g = Some((raw, chosen, en));
    (chosen, en)
}
/// A text of the web page ("web.<key>" in the language file); English when the language lacks it.
fn tw(key: &'static str) -> &'static str {
    let (chosen, en) = texts();
    let full = format!("web.{key}");
    chosen.get(full.as_str()).or_else(|| en.get(full.as_str())).copied().unwrap_or(key)
}
/// What the page needs to show itself: the choice, what there is to choose from, the texts and
/// the colours. No login needed: the login page uses it too, and nothing in it is secret.
fn ui_json() -> String {
    let (lang, theme) = ui_choice();
    let (chosen, en) = texts();
    let mut keys: Vec<&&str> = en.keys().filter(|k| k.starts_with("web.") || k.starts_with("locale.")).collect();
    keys.sort();
    let texts: Vec<String> = keys
        .iter()
        .map(|k| format!("{}:{}", json_str(k.strip_prefix("web.").unwrap_or(k)), json_str(chosen.get(**k).or_else(|| en.get(**k)).copied().unwrap_or(""))))
        .collect();
    // "HOME ASSISTANT" -> "Home Assistant"
    let title = |s: &str| {
        s.split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let langs: Vec<String> = list_files("lang", "txt", &BUILTIN_LANGS)
        .iter()
        .map(|code| {
            let name = find_file("lang", code, "txt", &BUILTIN_LANGS)
                .and_then(|t| t.lines().find_map(|l| l.strip_prefix("lang.name=").map(|v| title(v.trim()))))
                .unwrap_or_else(|| code.clone());
            format!("{{\"id\":{},\"name\":{}}}", json_str(code), json_str(&name))
        })
        .collect();
    let theme_kv = |name: &str| kv_text(&find_file("themes", name, "theme", &BUILTIN_THEMES).unwrap_or_default());
    let themes: Vec<String> = list_files("themes", "theme", &BUILTIN_THEMES)
        .iter()
        .map(|id| format!("{{\"id\":{},\"name\":{}}}", json_str(id), json_str(&title(theme_kv(id).get("name").map(|s| s.as_str()).unwrap_or(id)))))
        .collect();
    // the default theme first, so a theme only has to name what it changes
    let (base, mine) = (theme_kv("default"), theme_kv(&theme));
    let hex = |v: &String| v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit());
    let colors: Vec<String> = THEME_COLORS
        .iter()
        .filter_map(|k| mine.get(*k).filter(|v| hex(v)).or_else(|| base.get(*k).filter(|v| hex(v))).map(|v| format!("\"{k}\":{}", json_str(v))))
        .collect();
    // the font of the page: a CSS font-family list from the theme, nothing else in it
    let font = mine.get("web_font").or_else(|| base.get("web_font")).filter(|f| f.len() <= 200 && f.chars().all(|c| c.is_ascii_alphanumeric() || " ,\"'-_".contains(c))).cloned().unwrap_or_default();
    format!(
        "{{\"lang\":{},\"theme\":{},\"splash\":{},\"font\":{},\"langs\":[{}],\"themes\":[{}],\"texts\":{{{}}},\"colors\":{{{}}}}}",
        json_str(&lang),
        json_str(&theme),
        json_str(kv(UI_FILE).get("splash").map(|s| s.as_str()).filter(|s| SPLASH_STYLES.contains(s)).unwrap_or("name")),
        json_str(&font),
        langs.join(","),
        themes.join(","),
        texts.join(","),
        colors.join(",")
    )
}
/// lang=<code> and theme=<name> from the page; both must exist.
fn check_ui(body: &str) -> Result<Vec<(&'static str, String)>, String> {
    let mut out = Vec::new();
    for (k, v) in kv_text(body) {
        let ok = match k.as_str() {
            "lang" => find_file("lang", &v, "txt", &BUILTIN_LANGS).is_some(),
            "theme" => find_file("themes", &v, "theme", &BUILTIN_THEMES).is_some(),
            "splash" => SPLASH_STYLES.contains(&v.as_str()),
            _ => return Err(format!("{}: {k}", tw("err.unknown_setting"))),
        };
        if !ok {
            return Err(format!("{} {k}", tw("err.bad_value")));
        }
        out.push((["lang", "theme", "splash"].into_iter().find(|n| *n == k).unwrap_or("lang"), v));
    }
    if out.is_empty() {
        return Err(tw("err.nothing").into());
    }
    Ok(out)
}
const LOGIN_SECS: u64 = 60; // the question stays on the device this long (j49-screen: WEB_LOGIN_SECS)
const SESSION_SECS: u64 = 24 * 3600;
const MAX_BODY: usize = 8192;
const MAX_CONN: usize = 6;

fn root() -> String {
    std::env::var("J49_ROOT").unwrap_or_default()
}
fn p(path: &str) -> String {
    format!("{}{}", root(), path)
}
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
fn kv_text(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}
fn kv(path: &str) -> HashMap<String, String> {
    kv_text(&std::fs::read_to_string(p(path)).unwrap_or_default())
}
fn write_atomic(path: &str, body: &str, mode: u32) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let full = p(path);
    if let Some(dir) = std::path::Path::new(&full).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = format!("{full}.web");
    let _ = std::fs::remove_file(&tmp);
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut f| f.write_all(body.as_bytes()))
        .is_ok();
    ok && std::fs::rename(&tmp, &full).is_ok()
}
/// Set keys of a key=value file, keep every other line.
fn set_keys(path: &str, set: &[(&str, String)], mode: u32) -> bool {
    let old = std::fs::read_to_string(p(path)).unwrap_or_default();
    let mut out = String::new();
    let mut done = vec![false; set.len()];
    for l in old.lines() {
        let key = l.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
        match set.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                if !done[i] {
                    out.push_str(&format!("{}={}\n", set[i].0, set[i].1));
                    done[i] = true;
                }
            }
            None => {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    for (i, (k, v)) in set.iter().enumerate() {
        if !done[i] {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    write_atomic(path, &out, mode)
}
fn random_hex(bytes: usize) -> Option<String> {
    let mut b = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}
fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '<' => o.push_str("\\u003c"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---------------- validation (the same rules as bpd) ----------------
fn valid_days(s: &str) -> bool {
    const NAMES: [&str; 14] = ["mon", "ma", "tue", "di", "wed", "wo", "thu", "do", "fri", "vr", "sat", "za", "sun", "zo"];
    let s = s.to_ascii_lowercase();
    !s.is_empty()
        && s.split(',').all(|part| {
            part == "daily" || part == "all" || part == "dagelijks" || NAMES.contains(&part) || {
                part.split_once('-').map(|(a, b)| NAMES.contains(&a) && NAMES.contains(&b)).unwrap_or(false)
            }
        })
}
/// Check a schedule; Err names the first line bpd would skip.
fn check_schedule(text: &str) -> Result<usize, String> {
    let mut n = 0;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let bad = |why: &'static str| Err(format!("{} {}: {why}", tw("err.line"), i + 1));
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 3 {
            return bad(tw("err.sched.fields"));
        }
        if !valid_days(f[0]) {
            return bad(tw("err.sched.days"));
        }
        let ok_time = f[1]
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse::<u16>().ok()?, m.parse::<u16>().ok()?)))
            .map(|(h, m)| h <= 23 && m <= 59)
            .unwrap_or(false);
        if !ok_time {
            return bad(tw("err.sched.time"));
        }
        match f[2].parse::<f32>() {
            Ok(t) if (9.0..=32.0).contains(&t) => {}
            _ => return bad(tw("err.sched.temp")),
        }
        n += 1;
    }
    Ok(n)
}
fn half(t: f32) -> f32 {
    (t * 2.0).round() / 2.0
}
/// Thermostat keys a browser may set, checked and normalised.
fn check_thermostat(body: &str) -> Result<Vec<(&'static str, String)>, String> {
    let mut out = Vec::new();
    for (k, v) in kv_text(body) {
        let num = |lo: f32, hi: f32| v.parse::<f32>().ok().filter(|t| (lo..=hi).contains(t)).map(|t| format!("{:.1}", half(t)));
        let onoff = || (v == "on" || v == "off").then(|| v.clone());
        let (key, val): (&'static str, Option<String>) = match k.as_str() {
            "mode" => ("mode", (v == "heat" || v == "off").then(|| v.clone())),
            "setpoint" => ("setpoint", num(9.0, 32.0)),
            "away" => ("away", onoff()),
            "away_temp" => ("away_temp", num(9.0, 32.0)),
            "frost" => ("frost", num(0.0, 12.0)),
            "schedule" => ("schedule", onoff()),
            _ => return Err(format!("{}: {k}", tw("err.unknown_setting"))),
        };
        out.push((key, val.ok_or(format!("{} {key}", tw("err.bad_value")))?));
    }
    if out.is_empty() {
        return Err(tw("err.nothing").into());
    }
    Ok(out)
}
/// Broker settings from the page. An empty password keeps the stored one.
fn check_mqtt(body: &str) -> Result<Vec<(&'static str, String)>, String> {
    let m = kv_text(body);
    let clean = |s: &str, max: usize| s.len() <= max && !s.chars().any(|c| c.is_control() || c == '#');
    let host = m.get("host").cloned().unwrap_or_default();
    if host.is_empty() || host.len() > 100 || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':') {
        return Err(tw("err.mqtt.host").into());
    }
    let port = m.get("port").map(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or("1883");
    if port.parse::<u16>().map(|n| n == 0).unwrap_or(true) {
        return Err(tw("err.mqtt.port").into());
    }
    let user = m.get("user").cloned().unwrap_or_default();
    let pass = m.get("password").cloned().unwrap_or_default();
    // topic prefix: what is given, else what the file has; without either j49-mqtt uses its own
    // default (j49_<end of the MAC address>), so two devices never share topics
    let stored = kv("/var/lib/j49/mqtt.conf");
    let prefix = m.get("prefix").cloned().filter(|s| !s.is_empty()).or_else(|| stored.get("prefix").cloned()).unwrap_or_default();
    let name = m.get("name").cloned().unwrap_or_default();
    if name.len() > 60 || name.chars().any(|c| c.is_control() || c == '#' || c == '"' || c == '\\') {
        return Err(tw("err.mqtt.name").into());
    }
    if !clean(&user, 100) || !clean(&pass, 200) {
        return Err(tw("err.mqtt.cred").into());
    }
    if prefix.len() > 40 || !prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
        return Err(tw("err.mqtt.prefix").into());
    }
    let mut out = vec![("host", host), ("port", port.to_string()), ("user", user), ("name", name)];
    if !prefix.is_empty() {
        out.push(("prefix", prefix));
    }
    if !pass.is_empty() {
        out.push(("password", pass));
    } else if m.get("clear_password").map(|s| s == "1").unwrap_or(false) {
        out.push(("password", String::new()));
    }
    Ok(out)
}

/// Device settings from the page: (file, key, value). Screen settings go to j49-screen's file,
/// the temperature correction to the thermostat file.
const SCREEN_SETTINGS: &str = "/var/lib/j49-screen/settings";
fn check_device(body: &str) -> Result<Vec<(&'static str, &'static str, String)>, String> {
    let mut out = Vec::new();
    for (k, v) in kv_text(body) {
        let int = |lo: u32, hi: u32| v.parse::<u32>().ok().filter(|n| (lo..=hi).contains(n)).map(|n| n.to_string());
        let (file, key, val): (&'static str, &'static str, Option<String>) = match k.as_str() {
            "brightness" => (SCREEN_SETTINGS, "brightness", int(10, 120)),
            "volume" => (SCREEN_SETTINGS, "volume", int(0, 10)),
            "ringrev" => (SCREEN_SETTINGS, "ringrev", int(200, 100_000)),
            "font" => (SCREEN_SETTINGS, "font", int(0, 4)),
            "offset" => (
                "/var/lib/j49/thermostat",
                "offset",
                v.parse::<f32>().ok().filter(|t| (-10.0..=10.0).contains(t)).map(|t| format!("{:.1}", (t * 10.0).round() / 10.0)),
            ),
            "sleep" => ("/var/lib/j49/sleep", "enabled", (v == "on" || v == "off").then(|| v.clone())),
            "min_off" => ("/var/lib/j49/thermostat", "min_off", int(0, 600)),
            "min_on" => ("/var/lib/j49/thermostat", "min_on", int(0, 600)),
            "update_check" => ("/var/lib/j49/update", "check", (v == "on" || v == "off").then(|| v.clone())),
            _ => return Err(format!("{}: {k}", tw("err.unknown_setting"))),
        };
        out.push((file, key, val.ok_or(format!("{} {key}", tw("err.bad_value")))?));
    }
    if out.is_empty() {
        return Err(tw("err.nothing").into());
    }
    Ok(out)
}

// ---------------- WiFi networks (wpa_supplicant) ----------------
// The configuration file is rewritten with the new network added; the networks that were there
// stay, so a wrong password cannot lock the device out: wpa_supplicant falls back to them. The
// running wpa_supplicant is told over its control socket, it is never restarted. Passwords are
// written to the file (mode 600) and never read back to a browser.
const WPA_CONF: &str = "/etc/wpa_supplicant/wpa_supplicant-wlan0.conf";
const WPA_CTRL: &str = "/run/wpa_supplicant/wlan0";
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}
// wpa_supplicant on this image is built without passphrase support: it only takes the 256-bit
// key. So the key is derived here, the way WPA2 defines it: PBKDF2-HMAC-SHA1(passphrase, ssid,
// 4096 rounds, 32 bytes). The readable password is then stored nowhere.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}
fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}
/// The WPA2 key for a passphrase and a network name, as 64 hex digits.
fn wpa_psk(pass: &str, ssid: &str) -> String {
    let mut key = Vec::new();
    for block in 1u32..=2 {
        let mut salt = ssid.as_bytes().to_vec();
        salt.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha1(pass.as_bytes(), &salt);
        let mut t = u;
        for _ in 1..4096 {
            u = hmac_sha1(pass.as_bytes(), &u);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        key.extend_from_slice(&t);
    }
    hex(&key[..32])
}
/// Split a wpa_supplicant.conf into the lines outside network blocks and the blocks.
fn wifi_split(text: &str) -> (String, Vec<String>) {
    let (mut head, mut blocks, mut cur) = (String::new(), Vec::new(), None::<String>);
    for l in text.lines() {
        match &mut cur {
            Some(b) => {
                b.push_str(l);
                b.push('\n');
                if l.trim() == "}" {
                    blocks.push(cur.take().unwrap());
                }
            }
            None if l.trim_start().starts_with("network={") => cur = Some(format!("{l}\n")),
            None => {
                head.push_str(l);
                head.push('\n');
            }
        }
    }
    (head, blocks)
}
/// A value as wpa_supplicant writes it: "quoted text" or hex.
fn wpa_value(v: &str) -> Option<String> {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        Some(v[1..v.len() - 1].to_string())
    } else {
        unhex(v).map(|b| String::from_utf8_lossy(&b).to_string())
    }
}
fn block_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    block.lines().filter_map(|l| l.trim().split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}
fn block_ssid(block: &str) -> Option<String> {
    wpa_value(block_field(block, "ssid")?)
}
fn wifi_saved() -> Vec<String> {
    let text = std::fs::read_to_string(p(WPA_CONF)).unwrap_or_default();
    wifi_split(&text).1.iter().filter_map(|b| block_ssid(b)).collect()
}
/// The new configuration file with this network added (replacing one of the same name).
fn wifi_add_text(old: &str, ssid: &str, pass: &str) -> Result<String, String> {
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(|c| c.is_control()) {
        return Err(tw("err.wifi.ssid").into());
    }
    let is_key = pass.len() == 64 && pass.bytes().all(|c| c.is_ascii_hexdigit());
    if !is_key && (pass.len() < 8 || pass.len() > 63 || !pass.bytes().all(|c| (0x20..0x7f).contains(&c))) {
        return Err(tw("err.wifi.pass").into());
    }
    let (head, blocks) = wifi_split(old);
    let prio = blocks.iter().filter_map(|b| block_field(b, "priority")?.trim().parse::<i32>().ok()).max().unwrap_or(0) + 1;
    let mut out = head;
    for b in blocks.iter().filter(|b| block_ssid(b).as_deref() != Some(ssid)) {
        out.push_str(b);
    }
    let psk = if is_key { pass.to_ascii_lowercase() } else { wpa_psk(pass, ssid) };
    out.push_str(&format!("network={{\n\tssid={}\n\tpsk={psk}\n\tkey_mgmt=WPA-PSK\n\tpriority={prio}\n}}\n", hex(ssid.as_bytes())));
    Ok(out)
}
fn wifi_remove_text(old: &str, ssid: &str) -> Result<String, String> {
    let (head, blocks) = wifi_split(old);
    let keep: Vec<&String> = blocks.iter().filter(|b| block_ssid(b).as_deref() != Some(ssid)).collect();
    if keep.len() == blocks.len() {
        return Err(tw("err.wifi.unknown").into());
    }
    if keep.is_empty() {
        return Err(tw("err.wifi.last").into());
    }
    Ok(head + &keep.iter().map(|b| b.as_str()).collect::<String>())
}
/// One command to the running wpa_supplicant.
fn wpa(cmd: &str) -> Option<String> {
    use std::os::unix::net::UnixDatagram;
    static N: AtomicUsize = AtomicUsize::new(0);
    let local = p(&format!("/run/j49-web/wpa-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&local);
    let sock = UnixDatagram::bind(&local).ok()?;
    let r = (|| {
        sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        sock.connect(p(WPA_CTRL)).ok()?;
        sock.send(cmd.as_bytes()).ok()?;
        let mut buf = [0u8; 4096];
        let n = sock.recv(&mut buf).ok()?;
        Some(String::from_utf8_lossy(&buf[..n]).trim().to_string())
    })();
    let _ = std::fs::remove_file(&local);
    r
}
/// Ids of the running wpa_supplicant's networks with this name.
fn wpa_ids(ssid: &str) -> Vec<u32> {
    (0..16).filter(|id| wpa(&format!("GET_NETWORK {id} ssid")).and_then(|v| wpa_value(&v)).as_deref() == Some(ssid)).collect()
}
/// (state, name of the network in use) of the running wpa_supplicant.
fn wifi_status() -> (String, String) {
    let m = kv_text(&wpa("STATUS").unwrap_or_default());
    (m.get("wpa_state").cloned().unwrap_or_default(), m.get("ssid").cloned().unwrap_or_default())
}
fn wifi_add(ssid: &str, pass: &str, switch: bool) -> Result<(), String> {
    let old = std::fs::read_to_string(p(WPA_CONF)).map_err(|_| tw("err.wifi.read").to_string())?;
    let new = wifi_add_text(&old, ssid, pass)?;
    let prio = block_field(wifi_split(&new).1.last().map(|s| s.as_str()).unwrap_or(""), "priority").unwrap_or("1").to_string();
    if !write_atomic(WPA_CONF, &new, 0o600) {
        return Err(tw("err.save").into());
    }
    eprintln!("wifi network added");
    for id in wpa_ids(ssid) {
        wpa(&format!("REMOVE_NETWORK {id}"));
    }
    let id = wpa("ADD_NETWORK").and_then(|s| s.parse::<u32>().ok()).ok_or(tw("err.wifi.later"))?;
    let psk = block_field(wifi_split(&new).1.last().unwrap(), "psk").unwrap_or("").to_string();
    let ok = [
        format!("SET_NETWORK {id} ssid {}", hex(ssid.as_bytes())),
        format!("SET_NETWORK {id} psk {psk}"),
        format!("SET_NETWORK {id} key_mgmt WPA-PSK"),
        format!("SET_NETWORK {id} priority {prio}"),
        format!("ENABLE_NETWORK {id}"),
    ]
    .iter()
    .all(|c| wpa(c).as_deref() == Some("OK"));
    if !ok {
        return Err(tw("err.wifi.later").into());
    }
    if switch {
        wpa("REASSOCIATE");
    }
    Ok(())
}
fn wifi_remove(ssid: &str) -> Result<(), String> {
    if wifi_status().1 == ssid {
        return Err(tw("err.wifi.in_use").into());
    }
    let old = std::fs::read_to_string(p(WPA_CONF)).map_err(|_| tw("err.wifi.read").to_string())?;
    let new = wifi_remove_text(&old, ssid)?;
    if !write_atomic(WPA_CONF, &new, 0o600) {
        return Err(tw("err.save").into());
    }
    eprintln!("wifi network removed");
    for id in wpa_ids(ssid) {
        wpa(&format!("REMOVE_NETWORK {id}"));
    }
    Ok(())
}
fn device_json() -> String {
    let sc = kv(SCREEN_SETTINGS);
    let th = kv("/var/lib/j49/thermostat");
    let rel = kv("/etc/j49-release");
    let g = |m: &HashMap<String, String>, k: &str, d: &str| json_str(m.get(k).map(|s| s.as_str()).unwrap_or(d));
    let read = |f: &str| std::fs::read_to_string(p(f)).unwrap_or_default().trim().to_string();
    let uptime = read("/proc/uptime").split('.').next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").and_then(|_| s.local_addr()))
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let (wstate, wssid) = wifi_status();
    let nets: Vec<String> =
        wifi_saved().iter().map(|n| format!("{{\"ssid\":{},\"current\":{}}}", json_str(n), *n == wssid)).collect();
    format!(
        "{{\"brightness\":{},\"volume\":{},\"ringrev\":{},\"cards\":6,\"caps\":{},\"font\":{},\"sleep\":{},\"sleep_state\":{},\"sleep_why\":{},\"sleeps\":{},\"last_slept\":{},\"last_wake\":{},\"upd_check\":{},\"upd_url\":{},\"upd_latest\":{},\"upd_busy\":{},\"upd_error\":{},\"offset\":{},\"min_off\":{},\"min_on\":{},\"commit\":{},\"dirty\":{},\"commit_date\":{},\
\"built\":{},\"kernel\":{},\"uptime\":{uptime},\"ip\":{},\"mac\":{},\"wifi_state\":{},\"wifi_ssid\":{},\"networks\":[{}]}}",
        g(&sc, "brightness", "100"),
        g(&sc, "volume", "5"),
        g(&sc, "ringrev", "1300"),
        json_str(std::fs::read_to_string(p("/run/j49-screen.caps")).unwrap_or_default().trim()),
        g(&sc, "font", "1"),
        g(&kv("/var/lib/j49/sleep"), "enabled", "on"),
        g(&kv("/run/j49-sleep.state"), "state", "stopped"),
        g(&kv("/run/j49-sleep.state"), "why", ""),
        g(&kv("/run/j49-sleep.state"), "sleeps", "0"),
        g(&kv("/run/j49-sleep.state"), "last_slept", "0"),
        g(&kv("/run/j49-sleep.state"), "last_wake", ""),
        g(&kv("/var/lib/j49/update"), "check", "off"),
        g(&kv("/var/lib/j49/update"), "url", ""),
        g(&kv("/run/j49-update.state"), "latest", ""),
        g(&kv("/run/j49-update.state"), "busy", ""),
        g(&kv("/run/j49-update.state"), "error", ""),
        g(&th, "offset", "-2.0"),
        g(&th, "min_off", "10"),
        g(&th, "min_on", "0"),
        g(&rel, "commit", ""),
        g(&rel, "dirty", "0"),
        g(&rel, "commit_date", ""),
        g(&rel, "built", ""),
        json_str(&read("/proc/sys/kernel/osrelease")),
        json_str(&ip),
        json_str(&read("/sys/class/net/wlan0/address")),
        json_str(&wstate),
        json_str(&wssid),
        nets.join(",")
    )
}

// ---------------- login ----------------
struct Pending {
    token: String,
    code: String,
    at: u64,
}
#[derive(Default)]
struct Auth {
    sessions: HashMap<String, u64>, // token -> valid until
    pending: Option<Pending>,
    refused: Option<(String, u64)>, // token of the last refused or expired request, when
    last_ask: u64,
}
static AUTH: Mutex<Option<Auth>> = Mutex::new(None);

fn clear_question() {
    let _ = std::fs::remove_file(p("/run/j49-web/pending"));
    let _ = std::fs::remove_file(p("/run/j49-web/answer"));
}
/// Sessions are kept in /run (memory): a restart of this service keeps browsers logged in, a
/// reboot of the device does not.
fn save_sessions(a: &Auth) {
    let body: String = a.sessions.iter().map(|(t, until)| format!("{t}={until}\n")).collect();
    write_atomic("/run/j49-web/sessions", &body, 0o600);
}
fn load_sessions() -> HashMap<String, u64> {
    let now = unix_now();
    kv("/run/j49-web/sessions")
        .into_iter()
        .filter_map(|(t, until)| Some((t, until.parse::<u64>().ok()?)))
        .filter(|(t, until)| t.len() == 32 && t.bytes().all(|c| c.is_ascii_hexdigit()) && *until > now)
        .collect()
}
/// Look at the device's answer and at the clock; move the pending request on.
fn settle(a: &mut Auth) {
    let now = unix_now();
    a.sessions.retain(|_, until| *until > now);
    let Some(pd) = &a.pending else { return };
    let ans = kv("/run/j49-web/answer");
    let verdict = if ans.get("code") == Some(&pd.code) {
        Some(ans.get("ok").map(|s| s == "1").unwrap_or(false))
    } else if now > pd.at + LOGIN_SECS {
        Some(false)
    } else {
        None
    };
    if let Some(ok) = verdict {
        let pd = a.pending.take().unwrap();
        if ok {
            eprintln!("login allowed on the device");
            a.sessions.insert(pd.token, now + SESSION_SECS);
            save_sessions(a);
        } else {
            eprintln!("login refused or timed out");
            a.refused = Some((pd.token, now));
        }
        clear_question();
    }
}
/// A browser asks to log in: Ok((token, code)) or Err(text for the browser).
/// `mine` is the cookie of the asking browser: the browser whose question is open may send it
/// again (a new code replaces the old one on the screen).
fn ask(ip: &str, mine: &str) -> Result<(String, String), &'static str> {
    let mut g = AUTH.lock().unwrap();
    let a = g.get_or_insert_with(Auth::default);
    settle(a);
    let now = unix_now();
    match &a.pending {
        Some(pd) if pd.token != mine || mine.is_empty() => {
            return Err(tw("err.login.busy"))
        }
        Some(_) => {
            if now < a.last_ask + 5 {
                return Err(tw("err.login.fast"));
            }
            a.pending = None;
            clear_question();
        }
        None => {}
    }
    if now < a.last_ask + 5 {
        return Err(tw("err.login.fast"));
    }
    let (Some(token), Some(r)) = (random_hex(16), random_hex(4)) else { return Err(tw("err.random")) };
    let _ = now;
    let now = unix_now();
    let code = format!("{:04}", u32::from_str_radix(&r, 16).unwrap_or(0) % 10000);
    let _ = std::fs::remove_file(p("/run/j49-web/answer"));
    if !write_atomic("/run/j49-web/pending", &format!("code={code}\nip={ip}\nt={now}\n"), 0o600) {
        return Err(tw("err.login.screen"));
    }
    eprintln!("login asked from {ip}");
    a.last_ask = now;
    a.pending = Some(Pending { token: token.clone(), code: code.clone(), at: now });
    Ok((token, code))
}
/// "ok", "pending", "refused" or "none" for this cookie.
fn login_state(token: &str) -> &'static str {
    let mut g = AUTH.lock().unwrap();
    let a = g.get_or_insert_with(Auth::default);
    settle(a);
    if token.is_empty() {
        "none"
    } else if a.sessions.contains_key(token) {
        "ok"
    } else if a.pending.as_ref().map(|pd| pd.token == token).unwrap_or(false) {
        "pending"
    } else if a.refused.as_ref().map(|(t, at)| t == token && unix_now() < at + 300).unwrap_or(false) {
        "refused"
    } else {
        "none"
    }
}
fn logout(token: &str) {
    if let Some(a) = AUTH.lock().unwrap().as_mut() {
        a.sessions.remove(token);
        save_sessions(a);
    }
}

// ---------------- pages ----------------
fn state_json() -> String {
    let th = kv("/var/lib/j49/thermostat");
    let heat = kv("/run/bpd/heat");
    let mq = kv("/var/lib/j49/mqtt.conf");
    let ms = kv("/run/j49-mqtt.status");
    let now = unix_now();
    let fresh = |m: &HashMap<String, String>, s: u64| m.get("t").and_then(|t| t.parse::<u64>().ok()).map(|t| now <= t + s).unwrap_or(false);
    let g = |m: &HashMap<String, String>, k: &str, d: &str| json_str(m.get(k).map(|s| s.as_str()).unwrap_or(d));
    let live = fresh(&heat, 30);
    let mut out = format!(
        "{{\"mode\":{},\"setpoint\":{},\"away\":{},\"away_temp\":{},\"frost\":{},\"schedule\":{},\
\"backplate\":{},\"room\":{},\"demand\":{},\"relay\":{},\"wait\":{},\"reason\":{},\"schedule_text\":{},\
\"mqtt\":{{\"host\":{},\"port\":{},\"user\":{},\"name\":{},\"prefix\":{},\"has_password\":{},\"state\":{},\"detail\":{}}}}}",
        g(&th, "mode", "off"),
        g(&th, "setpoint", "20.0"),
        g(&th, "away", "off"),
        g(&th, "away_temp", "15.0"),
        g(&th, "frost", "5.0"),
        g(&th, "schedule", "off"),
        live,
        if live { g(&heat, "room", "") } else { json_str("") },
        if live { g(&heat, "demand", "0") } else { json_str("") },
        if live { g(&heat, "relay", "") } else { json_str("") },
        if live { g(&heat, "wait", "0") } else { json_str("0") },
        if live { g(&heat, "reason", "") } else { json_str("") },
        json_str(&std::fs::read_to_string(p("/var/lib/j49/schedule")).unwrap_or_default()),
        g(&mq, "host", ""),
        g(&mq, "port", "1883"),
        g(&mq, "user", ""),
        g(&mq, "name", ""),
        g(&mq, "prefix", ""),
        mq.get("password").map(|s| !s.is_empty()).unwrap_or(false),
        if fresh(&ms, 90) { g(&ms, "state", "") } else { json_str("stopped") },
        g(&ms, "detail", ""),
    );
    out.pop();
    out.push_str(&format!(",\"device\":{}}}", device_json()));
    out
}

struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}
fn read_request(s: &mut TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 8192 {
            return None;
        }
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: HashMap<String, String> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(len);
    Some(Req { method, path, headers, body: String::from_utf8_lossy(&body).to_string() })
}
fn respond(s: &mut TcpStream, code: &str, ctype: &str, extra: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
Content-Security-Policy: default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-ancestors 'none'\r\n\
{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body.as_bytes());
}
fn json(s: &mut TcpStream, code: &str, body: &str) {
    respond(s, code, "application/json", "", body)
}
fn msg(s: &mut TcpStream, code: &str, text: &str) {
    json(s, code, &format!("{{\"message\":{}}}", json_str(text)))
}
fn handle(mut s: TcpStream) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(8)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(8)));
    let ip = s.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let Some(r) = read_request(&mut s) else { return };
    // j49-sleep keeps the device awake while the page is in use: it looks at this file's time
    let _ = std::fs::write(p("/run/j49-web/last"), "");
    let token: String = r
        .headers
        .get("cookie")
        .and_then(|c| c.split(';').filter_map(|x| x.trim().strip_prefix("j49=")).next())
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(32)
        .collect();
    let path = r.path.split('?').next().unwrap_or("");
    // A change must come from our own page: browsers do not let another site add this header.
    if r.method == "POST" && r.headers.get("x-j49").map(|v| v != "1").unwrap_or(true) {
        return msg(&mut s, "403 Forbidden", tw("err.origin"));
    }
    match (r.method.as_str(), path) {
        ("GET", "/") => respond(&mut s, "200 OK", "text/html; charset=utf-8", "", PAGE),
        ("GET", "/api/ui") => json(&mut s, "200 OK", &ui_json()),
        ("GET", "/api/login") => json(&mut s, "200 OK", &format!("{{\"state\":\"{}\"}}", login_state(&token))),
        ("POST", "/api/login") => match ask(&ip, &token) {
            Ok((token, code)) => respond(
                &mut s,
                "200 OK",
                "application/json",
                &format!("Set-Cookie: j49={token}; Path=/; Max-Age={SESSION_SECS}; HttpOnly; SameSite=Strict\r\n"),
                &format!("{{\"code\":\"{code}\",\"seconds\":{LOGIN_SECS}}}"),
            ),
            Err(e) => msg(&mut s, "429 Too Many Requests", e),
        },
        _ if login_state(&token) != "ok" => msg(&mut s, "401 Unauthorized", tw("err.not_logged_in")),
        ("GET", "/api/state") => json(&mut s, "200 OK", &state_json()),
        ("POST", "/api/logout") => {
            logout(&token);
            msg(&mut s, "200 OK", tw("logged_out"))
        }
        ("POST", "/api/thermostat") => match check_thermostat(&r.body) {
            Ok(set) => {
                let sched = set.iter().any(|(k, _)| *k == "schedule");
                if set_keys("/var/lib/j49/thermostat", &set, 0o644) {
                    if sched {
                        let _ = std::fs::remove_file(p("/var/lib/j49/schedule.state")); // bpd applies the current change
                    }
                    json(&mut s, "200 OK", &state_json())
                } else {
                    msg(&mut s, "500 Internal Server Error", tw("err.save"))
                }
            }
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        ("POST", "/api/schedule") => match check_schedule(&r.body) {
            Ok(0) => msg(&mut s, "400 Bad Request", tw("err.sched.empty")),
            Ok(_) => {
                let mut text = r.body.replace('\r', "");
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                if write_atomic("/var/lib/j49/schedule", &text, 0o644) {
                    let _ = std::fs::remove_file(p("/var/lib/j49/schedule.state"));
                    json(&mut s, "200 OK", &state_json())
                } else {
                    msg(&mut s, "500 Internal Server Error", tw("err.save"))
                }
            }
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        ("POST", "/api/ui") => match check_ui(&r.body) {
            // j49-screen notices the changed file and follows
            Ok(set) if set_keys(UI_FILE, &set, 0o644) => json(&mut s, "200 OK", &ui_json()),
            Ok(_) => msg(&mut s, "500 Internal Server Error", tw("err.save")),
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        ("POST", "/api/mqtt") => match check_mqtt(&r.body) {
            Ok(set) => {
                // j49-mqtt notices the changed file and connects again
                if set_keys("/var/lib/j49/mqtt.conf", &set, 0o600) {
                    eprintln!("broker settings changed");
                    json(&mut s, "200 OK", &state_json())
                } else {
                    msg(&mut s, "500 Internal Server Error", tw("err.save"))
                }
            }
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        ("POST", "/api/device") => match check_device(&r.body) {
            Ok(set) => {
                let mut ok = true;
                for file in [SCREEN_SETTINGS, "/var/lib/j49/thermostat", "/var/lib/j49/sleep"] {
                    let keys: Vec<(&str, String)> = set.iter().filter(|(f, _, _)| *f == file).map(|(_, k, v)| (*k, v.clone())).collect();
                    if !keys.is_empty() {
                        ok &= set_keys(file, &keys, 0o644);
                    }
                }
                if ok {
                    json(&mut s, "200 OK", &state_json())
                } else {
                    msg(&mut s, "500 Internal Server Error", tw("err.save"))
                }
            }
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        ("POST", "/api/wifi") => {
            let m = kv_text(&r.body);
            let get = |k: &str| m.get(k).cloned().unwrap_or_default();
            match wifi_add(&get("ssid"), &get("password"), get("switch") == "1") {
                Ok(()) => json(&mut s, "200 OK", &state_json()),
                Err(e) => msg(&mut s, "400 Bad Request", &e),
            }
        }
        ("POST", "/api/wifi-remove") => match wifi_remove(kv_text(&r.body).get("ssid").map(|s| s.as_str()).unwrap_or("")) {
            Ok(()) => json(&mut s, "200 OK", &state_json()),
            Err(e) => msg(&mut s, "400 Bad Request", &e),
        },
        // Updates (j49-update): looking is one order, installing another. Both run on their own,
        // outside this service: an install ends in a restart.
        ("POST", "/api/update-check") => {
            let _ = std::process::Command::new("systemd-run").args(["--quiet", "--collect", "/usr/bin/j49-update", "check", "--now"]).status();
            msg(&mut s, "200 OK", "ok");
        }
        ("POST", "/api/update-install") => {
            eprintln!("update asked from the page");
            let _ = std::process::Command::new("systemd-run").args(["--quiet", "--collect", "/usr/bin/j49-update", "install"]).status();
            msg(&mut s, "200 OK", "ok");
        }
        ("POST", "/api/restart") => {
            eprintln!("restart asked from the page");
            msg(&mut s, "200 OK", tw("restart.done.lower"));
            let _ = s.shutdown(std::net::Shutdown::Both);
            let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
        }
        _ => msg(&mut s, "404 Not Found", tw("err.404")),
    }
}

fn main() {
    let port: u16 = std::env::var("J49_WEB_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(80);
    {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p("/run/j49-web"));
    }
    clear_question(); // nothing of a previous run may still be on the screen
    *AUTH.lock().unwrap() = Some(Auth { sessions: load_sessions(), ..Auth::default() });
    let l = TcpListener::bind(("0.0.0.0", port)).unwrap_or_else(|e| {
        eprintln!("cannot listen on port {port}: {e}");
        std::process::exit(1)
    });
    eprintln!("listening on port {port}");
    static BUSY: AtomicUsize = AtomicUsize::new(0);
    for s in l.incoming().flatten() {
        if BUSY.load(Ordering::Relaxed) >= MAX_CONN {
            continue; // dropped: the browser tries again
        }
        BUSY.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().stack_size(128 * 1024).spawn(move || {
            handle(s);
            BUSY.fetch_sub(1, Ordering::Relaxed);
        });
        if spawned.is_err() {
            BUSY.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_rules() {
        assert_eq!(check_schedule("# kop\nma-vr 06:30 20.0\nza,zo 08:00 20.5 # weekend\n\ndagelijks 23:00 16\n"), Ok(3));
        assert!(check_schedule("ma-vr 6:30 20").is_ok());
        assert!(check_schedule("ma-vr 24:00 20").unwrap_err().starts_with("line 1"));
        assert!(check_schedule("ma-vr 06:30 20\nxx 07:00 20").unwrap_err().starts_with("line 2"));
        assert!(check_schedule("ma 07:00 40").is_err());
        assert!(check_schedule("ma 07:00").is_err());
        assert!(check_schedule("ma- 07:00 20").is_err());
    }
    #[test]
    fn thermostat_rules() {
        let mut v = check_thermostat("setpoint=20.26\naway=on\n").unwrap();
        v.sort();
        assert_eq!(v, [("away", "on".to_string()), ("setpoint", "20.5".to_string())]);
        assert!(check_thermostat("setpoint=50").is_err());
        assert!(check_thermostat("mode=boost").is_err());
        assert!(check_thermostat("offset=1").is_err()); // not settable from the page
        assert!(check_thermostat("").is_err());
    }
    #[test]
    fn mqtt_rules() {
        let v = check_mqtt("host=192.168.1.10\nport=\nuser=nest\npassword=\n").unwrap();
        assert!(v.contains(&("port", "1883".to_string())));
        assert!(!v.iter().any(|(k, _)| *k == "password")); // empty keeps the stored one
        assert!(check_mqtt("host=a b\n").is_err());
        assert!(check_mqtt("host=h\nport=99999\n").is_err());
        assert!(check_mqtt("host=h\nuser=a#b\n").is_err());
        assert!(check_mqtt("host=h\nclear_password=1\n").unwrap().contains(&("password", String::new())));
    }
    #[test]
    fn device_rules() {
        let v = check_device("brightness=80\noffset=-2.54\n").unwrap();
        assert!(v.contains(&(SCREEN_SETTINGS, "brightness", "80".to_string())));
        assert!(v.contains(&("/var/lib/j49/thermostat", "offset", "-2.5".to_string())));
        assert!(check_device("brightness=5").is_err());
        assert!(check_device("volume=11").is_err());
        assert!(check_device("ringrev=10").is_err());
        assert!(check_device("wifi=0").is_err());
        assert!(check_device("font=1").is_ok());
        assert!(check_device("font=5").is_err());
        assert!(check_device("min_off=30\nmin_on=0").is_ok());
        assert!(check_device("min_off=601").is_err());
    }
    #[test]
    fn wifi_file() {
        let old = "ctrl_interface=/run/wpa_supplicant\ncountry=NL\nnetwork={\n\tssid=\"Home\"\n\tpsk=\"oldpachtwoord\"\n\tkey_mgmt=WPA-PSK\n}\n";
        let new = wifi_add_text(old, "Guest net", "nieuw wachtwoord").unwrap();
        // the old network is kept untouched, the new one comes after it with a higher priority
        assert!(new.starts_with(old));
        let key = wpa_psk("nieuw wachtwoord", "Guest net");
        assert!(new.contains(&format!("ssid=4775657374206e6574\n\tpsk={key}\n\tkey_mgmt=WPA-PSK\n\tpriority=1\n")));
        assert!(!new.contains("nieuw wachtwoord")); // the readable password is not stored
        let (head, blocks) = wifi_split(&new);
        assert_eq!(head, "ctrl_interface=/run/wpa_supplicant\ncountry=NL\n");
        assert_eq!(blocks.iter().filter_map(|b| block_ssid(b)).collect::<Vec<_>>(), ["Home", "Guest net"]);
        // the same name again replaces it and moves up
        let again = wifi_add_text(&new, "Guest net", "ander wachtwoord").unwrap();
        assert_eq!(wifi_split(&again).1.len(), 2);
        assert!(again.contains("priority=2") && !again.contains(&key));
        // a 64-digit key goes in unquoted
        assert!(wifi_add_text(old, "x", &"ab".repeat(32)).unwrap().contains(&format!("psk={}\n", "ab".repeat(32))));
        assert!(wifi_add_text(old, "x", "short").is_err());
        assert!(wifi_add_text(old, "", "long enough").is_err());
        assert!(wifi_add_text(old, "x", "with\u{e9}accent").is_err());
        // removing: not the last one
        assert_eq!(wifi_remove_text(&new, "Guest net").unwrap(), old);
        assert!(wifi_remove_text(old, "Home").is_err());
        assert!(wifi_remove_text(old, "Unknown").is_err());
    }
    #[test]
    fn wpa_key() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        // test vector of IEEE 802.11i (H.4.1)
        assert_eq!(wpa_psk("password", "IEEE"), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
        assert_eq!(wpa_psk("ThisIsAPassword", "ThisIsASSID"), "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");
    }
    #[test]
    fn ui_texts_and_themes() {
        // both built-in languages have every key of the other, and the page gets them
        let (en, nl) = (parse_texts(BUILTIN_LANGS[0].1.to_string()), parse_texts(BUILTIN_LANGS[1].1.to_string()));
        let missing: Vec<_> = en.keys().filter(|k| !nl.contains_key(*k)).chain(nl.keys().filter(|k| !en.contains_key(*k))).collect();
        assert!(missing.is_empty(), "{missing:?}");
        // every text the page asks for exists
        for part in PAGE.split("T(\"").skip(1).chain(PAGE.split("data-t=\"").skip(1)).chain(PAGE.split("data-ta=\"").skip(1)) {
            let key = part.split('"').next().unwrap();
            assert!(en.contains_key(format!("web.{key}").as_str()), "no text for {key}");
        }
        let j = ui_json();
        assert!(j.contains("\"lang\":\"en\"") && j.contains("\"accent\":\"f0b429\"") && j.contains("\"id\":\"home-assistant\""));
        assert!(j.contains("\"font\":\"system-ui, -apple-system, \\\"Segoe UI\\\", sans-serif\""), "{j}");
        assert!(j.contains("\"locale.decimal\":\".\"") && j.contains("\"login\":\"Log in\""));
        assert!(check_ui("lang=nl\ntheme=light").is_ok());
        assert!(check_ui("splash=flame").is_ok() && check_ui("splash=castle").is_err());
        assert!(j.contains("\"splash\":\"name\"") && j.contains("\"id\":\"home-assistant-light\""));
        assert!(check_ui("lang=../etc").is_err() && check_ui("theme=nope").is_err() && check_ui("").is_err());
    }
    #[test]
    fn json_escape() {
        assert_eq!(json_str("a\"b\\c\n<x>"), "\"a\\\"b\\\\c\\n\\u003cx>\"");
    }
}
