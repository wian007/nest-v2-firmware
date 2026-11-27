// j49-mqtt: publish the thermostat to an MQTT broker and take commands from it, with Home
// Assistant discovery. No dependencies: a minimal MQTT 3.1.1 client (QoS 0, plain TCP).
//
// It only reads and writes the files that are the thermostat's interface (docs/thermostat.md):
//   reads   /run/bpd/{heat,sht20,pir,als}, /var/lib/j49/{thermostat,schedule}
//   writes  /var/lib/j49/thermostat (single keys), /var/lib/j49/schedule
//
// Broker settings: /var/lib/j49/mqtt.conf (key=value: host, port, user, password, prefix,
// discovery). That file holds a password: it lives on the device only and is never logged.
// J49_ROOT=<dir> prefixes every path, for tests on a PC.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn root() -> String {
    std::env::var("J49_ROOT").unwrap_or_default()
}
fn p(path: &str) -> String {
    format!("{}{}", root(), path)
}
fn kv(path: &str) -> HashMap<String, String> {
    std::fs::read_to_string(p(path))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next().unwrap_or("").split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
/// Set one key of a key=value file, keep every other line, write atomically.
fn set_key(path: &str, key: &str, value: &str) {
    let full = p(path);
    let old = std::fs::read_to_string(&full).unwrap_or_default();
    let mut out = String::new();
    let mut done = false;
    for l in old.lines() {
        if l.split_once('=').map(|(k, _)| k.trim() == key).unwrap_or(false) {
            out.push_str(&format!("{key}={value}\n"));
            done = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("{key}={value}\n"));
    }
    if let Some(dir) = std::path::Path::new(&full).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = format!("{full}.mqtt");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, &full);
    }
}

/// What j49-web shows about the connection: /run/j49-mqtt.status (state, detail, t).
fn status(state: &str, detail: &str) {
    let full = p("/run/j49-mqtt.status");
    let tmp = format!("{full}.tmp");
    if std::fs::write(&tmp, format!("state={state}\ndetail={detail}\nt={}\n", unix_now())).is_ok() {
        let _ = std::fs::rename(&tmp, &full);
    }
}
/// Changes when the broker settings or the language are written (j49-web does that): time to
/// connect again and to announce the entities again.
fn cfg_stamp() -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let m = |f: &str| std::fs::metadata(p(f)).and_then(|m| m.modified()).ok();
    (m("/var/lib/j49/mqtt.conf"), m("/var/lib/j49/ui"))
}

// ---------------- texts: the names of the entities, per language ----------------
// The "mqtt.*" keys of tools/lang/<code>.txt; the language is lang= in /var/lib/j49/ui
// (docs/thermostat.md). English and Dutch are built in.
const BUILTIN_LANGS: [(&str, &str); 2] = [("en", include_str!("../../lang/en.txt")), ("nl", include_str!("../../lang/nl.txt"))];
fn lang_text(code: &str) -> Option<String> {
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    ["/var/lib/j49", "/usr/share/j49"]
        .iter()
        .find_map(|dir| std::fs::read_to_string(p(&format!("{dir}/lang/{code}.txt"))).ok())
        .or_else(|| BUILTIN_LANGS.iter().find(|(n, _)| *n == code).map(|(_, t)| t.to_string()))
}
/// The text mqtt.<key> in the chosen language; English when the language lacks it.
fn tm(key: &str) -> String {
    let find = |text: &str| {
        text.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| l.strip_prefix("mqtt.").and_then(|r| r.strip_prefix(key)).and_then(|r| r.strip_prefix('=')).map(|v| v.trim().to_string()))
    };
    let lang = kv("/var/lib/j49/ui").get("lang").cloned().unwrap_or_default();
    lang_text(&lang).and_then(|t| find(&t)).or_else(|| find(BUILTIN_LANGS[0].1)).unwrap_or_else(|| key.to_string())
}

// ---------------- MQTT 3.1.1, the small part we need ----------------
fn varint(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}
fn mstr(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}
fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![first];
    varint(body.len(), &mut v);
    v.extend_from_slice(body);
    v
}
fn connect_packet(id: &str, user: &str, pass: &str, will_topic: &str, keepalive: u16) -> Vec<u8> {
    let mut b = Vec::new();
    mstr("MQTT", &mut b);
    b.push(4); // protocol level 3.1.1
    // No "clean session": the broker keeps our subscriptions and holds commands sent with QoS 1
    // while we are away (asleep, or WiFi down), and hands them over at the next connect.
    let mut flags = 0x04 | 0x20; // will, will retain
    if !user.is_empty() {
        flags |= 0x80;
    }
    if !pass.is_empty() {
        flags |= 0x40;
    }
    b.push(flags);
    b.extend_from_slice(&keepalive.to_be_bytes());
    mstr(id, &mut b);
    mstr(will_topic, &mut b);
    mstr("offline", &mut b);
    if !user.is_empty() {
        mstr(user, &mut b);
    }
    if !pass.is_empty() {
        mstr(pass, &mut b);
    }
    packet(0x10, &b)
}
fn publish_packet(topic: &str, payload: &str, retain: bool) -> Vec<u8> {
    let mut b = Vec::new();
    mstr(topic, &mut b);
    b.extend_from_slice(payload.as_bytes());
    packet(0x30 | retain as u8, &b)
}
fn subscribe_packet(id: u16, topic: &str) -> Vec<u8> {
    let mut b = id.to_be_bytes().to_vec();
    mstr(topic, &mut b);
    b.push(1); // QoS 1: commands are acknowledged, and kept for us while we are away
    packet(0x82, &b)
}
/// Take one complete packet off the front of `buf`: (first byte, body).
fn take_packet(buf: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    if buf.len() < 2 {
        return None;
    }
    let (mut len, mut mult, mut i) = (0usize, 1usize, 1usize);
    loop {
        let b = *buf.get(i)?;
        len += (b & 0x7f) as usize * mult;
        mult *= 128;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        if i > 4 {
            buf.clear();
            return None;
        }
    }
    if buf.len() < i + len {
        return None;
    }
    let first = buf[0];
    let body = buf[i..i + len].to_vec();
    buf.drain(..i + len);
    Some((first, body))
}

// ---------------- the week schedule, day by day ----------------
// The file (/var/lib/j49/schedule, format in docs/thermostat.md) has lines "<days> <hh:mm>
// <temp>". For MQTT it is unfolded into seven days, each a list of (minute of day, temperature).
const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
/// The Dutch codes earlier versions wrote; still read everywhere a day is named.
const OLD_DAYS: [&str; 7] = ["ma", "di", "wo", "do", "vr", "za", "zo"];
const DAY_TOPICS: [&str; 7] = ["schedule/mon", "schedule/tue", "schedule/wed", "schedule/thu", "schedule/fri", "schedule/sat", "schedule/sun"];
fn day_index(name: &str) -> Option<usize> {
    let n = name.to_ascii_lowercase();
    DAYS.iter().position(|d| *d == n).or_else(|| OLD_DAYS.iter().position(|d| *d == n))
}
type Day = Vec<(u16, f32)>;
type Week = [Day; 7];

fn day_mask(s: &str) -> Option<u8> {
    let idx = day_index;
    let mut mask = 0u8;
    for part in s.to_ascii_lowercase().split(',') {
        if part == "daily" || part == "all" || part == "dagelijks" {
            mask |= 0x7f;
        } else if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (idx(a)?, idx(b)?);
            let mut d = a;
            loop {
                mask |= 1 << d;
                if d == b {
                    break;
                }
                d = (d + 1) % 7;
            }
        } else {
            mask |= 1 << idx(part)?;
        }
    }
    (mask != 0).then_some(mask)
}
fn parse_time(t: &str) -> Option<u16> {
    let (h, m) = t.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h <= 23 && m <= 59).then_some(h * 60 + m)
}
fn valid_temp(c: f32) -> Option<f32> {
    (9.0..=32.0).contains(&c).then(|| (c * 2.0).round() / 2.0)
}
/// The schedule file as a week. Lines bpd would skip are skipped here too.
fn week_of(text: &str) -> Week {
    let mut w: Week = Default::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let (Some(d), Some(t), Some(c)) = (it.next(), it.next(), it.next()) else { continue };
        let (Some(mask), Some(min), Some(temp)) = (day_mask(d), parse_time(t), c.parse::<f32>().ok().and_then(valid_temp)) else { continue };
        for (i, day) in w.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                day.retain(|(m, _)| *m != min);
                day.push((min, temp));
            }
        }
    }
    for day in w.iter_mut() {
        day.sort_by_key(|(m, _)| *m);
    }
    w
}
/// The week as a schedule file: one line per time and temperature, with all its days.
fn file_of(w: &Week) -> String {
    let mut groups: Vec<(u16, i32, u8)> = Vec::new(); // minute, tenths, day mask
    for (i, day) in w.iter().enumerate() {
        for (min, temp) in day {
            let key = (*min, (*temp * 10.0).round() as i32);
            match groups.iter_mut().find(|g| (g.0, g.1) == key) {
                Some(g) => g.2 |= 1 << i,
                None => groups.push((key.0, key.1, 1 << i)),
            }
        }
    }
    groups.sort();
    let mut out = String::from("# Schedule: <days> <hh:mm> <temperature>  (updated over MQTT)\n");
    for (min, x10, mask) in groups {
        let days = if mask == 0x7f {
            "daily".to_string()
        } else {
            (0..7).filter(|i| mask & (1 << i) != 0).map(|i| DAYS[i]).collect::<Vec<_>>().join(",")
        };
        out.push_str(&format!("{days} {:02}:{:02} {}.{}\n", min / 60, min % 60, x10 / 10, x10 % 10));
    }
    out
}
/// One day as text for a person: "06:30 20.0, 08:30 17.0".
fn day_text(day: &Day) -> String {
    day.iter().map(|(m, c)| format!("{:02}:{:02} {:.1}", m / 60, m % 60, c)).collect::<Vec<_>>().join(", ")
}
/// The same text back into a day. "06:30=20" and ";" as the separator are taken too; an empty
/// text is a day without changes. Temperatures take a decimal point (the comma separates the
/// parts). None when a part cannot be read.
fn parse_day_text(text: &str) -> Option<Day> {
    let mut day: Day = Vec::new();
    for part in text.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (t, c) = part.split_once(['=', ' '])?;
        let (min, temp) = (parse_time(t)?, valid_temp(c.trim().parse().ok()?)?);
        day.retain(|(m, _)| *m != min);
        day.push((min, temp));
    }
    day.sort_by_key(|(m, _)| *m);
    (day.len() <= 12).then_some(day)
}
/// The week as JSON: {"mon":[{"time":"06:30","temp":20.0}, ...], "tue":[...], ...}.
fn week_json(w: &Week) -> String {
    let days: Vec<String> = w
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let items: Vec<String> = d.iter().map(|(m, c)| format!(r#"{{"time":"{:02}:{:02}","temp":{:.1}}}"#, m / 60, m % 60, c)).collect();
            format!(r#""{}":[{}]"#, DAYS[i], items.join(","))
        })
        .collect();
    format!("{{{}}}", days.join(","))
}

// A small JSON reader, enough for the week: objects, arrays, strings, numbers.
#[derive(Debug, Clone, PartialEq)]
enum J {
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Num(f64),
    Other,
}
fn json_parse(s: &str) -> Option<J> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let v = json_value(&b, &mut i, 0)?;
    while i < b.len() && b[i].is_whitespace() {
        i += 1;
    }
    (i == b.len()).then_some(v)
}
fn json_value(b: &[char], i: &mut usize, depth: u32) -> Option<J> {
    if depth > 6 {
        return None;
    }
    let ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1;
        }
    };
    ws(i);
    match *b.get(*i)? {
        '{' => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                ws(i);
                if *b.get(*i)? == '}' {
                    *i += 1;
                    return Some(J::Obj(v));
                }
                let J::Str(k) = json_value(b, i, depth + 1)? else { return None };
                ws(i);
                if *b.get(*i)? != ':' {
                    return None;
                }
                *i += 1;
                v.push((k, json_value(b, i, depth + 1)?));
                ws(i);
                match *b.get(*i)? {
                    ',' => *i += 1,
                    '}' => {}
                    _ => return None,
                }
            }
        }
        '[' => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                ws(i);
                if *b.get(*i)? == ']' {
                    *i += 1;
                    return Some(J::Arr(v));
                }
                v.push(json_value(b, i, depth + 1)?);
                ws(i);
                match *b.get(*i)? {
                    ',' => *i += 1,
                    ']' => {}
                    _ => return None,
                }
            }
        }
        '"' => {
            *i += 1;
            let mut out = String::new();
            loop {
                let c = *b.get(*i)?;
                *i += 1;
                match c {
                    '"' => return Some(J::Str(out)),
                    '\\' => {
                        let e = *b.get(*i)?;
                        *i += 1;
                        out.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    }
                    c => out.push(c),
                }
            }
        }
        c if c == '-' || c.is_ascii_digit() => {
            let start = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || matches!(b[*i], '-' | '+' | '.' | 'e' | 'E')) {
                *i += 1;
            }
            b[start..*i].iter().collect::<String>().parse().ok().map(J::Num)
        }
        _ => {
            // true, false, null: not needed, but they must not break the reading
            let start = *i;
            while *i < b.len() && b[*i].is_ascii_alphabetic() {
                *i += 1;
            }
            (*i > start).then_some(J::Other)
        }
    }
}
/// Days from a JSON message. A day may be a list of {"time","temp"} objects, a list of
/// ["hh:mm", temp] pairs, or the day text. Days that are not named keep what they have.
fn week_from_json(text: &str, mut w: Week) -> Option<Week> {
    let J::Obj(days) = json_parse(text)? else { return None };
    for (name, v) in days {
        let i = day_index(&name)?;
        w[i] = match v {
            J::Str(s) => parse_day_text(&s)?,
            J::Arr(items) => {
                let mut day: Day = Vec::new();
                for it in items {
                    let (t, c) = match it {
                        J::Obj(f) => {
                            let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
                            (get("time")?, get("temp")?)
                        }
                        J::Arr(p) if p.len() == 2 => (p[0].clone(), p[1].clone()),
                        _ => return None,
                    };
                    let (J::Str(t), J::Num(c)) = (t, c) else { return None };
                    let (min, temp) = (parse_time(&t)?, valid_temp(c as f32)?);
                    day.retain(|(m, _)| *m != min);
                    day.push((min, temp));
                }
                day.sort_by_key(|(m, _)| *m);
                if day.len() > 12 {
                    return None;
                }
                day
            }
            _ => return None,
        };
    }
    Some(w)
}
fn save_week(w: &Week) {
    let full = p("/var/lib/j49/schedule");
    let tmp = format!("{full}.mqtt");
    if std::fs::write(&tmp, file_of(w)).is_ok() {
        let _ = std::fs::rename(&tmp, &full);
        let _ = std::fs::remove_file(p("/var/lib/j49/schedule.state")); // bpd applies the change in force now
    }
}

/// Names of the tunes j49-screen knows (its TUNES table).
const TUNES: [&str; 34] = [
    "ding", "done", "happy", "alarm", "sleep", "ode", "elise", "nachtmusik", "toccata", "mountainking",
    "frerejacques", "twinkle", "birthday", "jinglebells", "cancan", "tell", "fifth", "turkishmarch", "spring", "canon",
    "weddingmarch", "silentnight", "tannenbaum", "ducklings", "auldlangsyne", "yankee", "entertainer", "taps",
    "greensleeves", "susanna", "marylamb", "londonbridge", "saints", "charge",
];

// ---------------- the thermostat as topics ----------------
struct Cfg {
    host: String,
    port: u16,
    user: String,
    pass: String,
    prefix: String,
    /// Name of the device in Home Assistant (name= in mqtt.conf).
    name: String,
    discovery: String,
    /// MQTT keep-alive in seconds: we ping at half of it, the broker gives up at 1.5 times.
    /// 60 while the device never sleeps; a long value (e.g. 3600) lets it sleep in between.
    keepalive: u16,
}
fn read_cfg() -> Option<Cfg> {
    let m = kv("/var/lib/j49/mqtt.conf");
    let host = m.get("host")?.clone();
    if host.is_empty() {
        return None;
    }
    Some(Cfg {
        host,
        port: m.get("port").and_then(|s| s.parse().ok()).unwrap_or(1883),
        user: m.get("user").cloned().unwrap_or_default(),
        pass: m.get("password").cloned().unwrap_or_default(),
        // Two devices on one broker must not share topics: without prefix= the topics start
        // with j49_<last six digits of the MAC address>.
        prefix: m.get("prefix").cloned().filter(|s| !s.is_empty()).unwrap_or_else(default_prefix),
        name: m.get("name").cloned().filter(|s| !s.is_empty()).unwrap_or_else(default_name),
        discovery: m.get("discovery").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| "homeassistant".into()),
        keepalive: m.get("keepalive").and_then(|s| s.parse::<u16>().ok()).filter(|k| *k >= 10).unwrap_or(60),
    })
}
/// Every state topic (relative to the prefix) with its current value.
fn states() -> Vec<(&'static str, String)> {
    let th = kv("/var/lib/j49/thermostat");
    let heat = kv("/run/bpd/heat");
    let sht = kv("/run/bpd/sht20");
    let pir = kv("/run/bpd/pir");
    let als = kv("/run/bpd/als");
    let now = unix_now();
    let fresh = |m: &HashMap<String, String>, max: u64| m.get("t").and_then(|s| s.parse::<u64>().ok()).map(|t| now <= t + max).unwrap_or(false);
    let mode = if th.get("mode").map(|s| s == "heat").unwrap_or(false) { "heat" } else { "off" };
    let mut v: Vec<(&'static str, String)> = vec![
        ("climate/mode", mode.to_string()),
        ("climate/setpoint", th.get("setpoint").cloned().unwrap_or_else(|| "20.0".into())),
        ("schedule/enabled", if th.get("schedule").map(|s| s == "on").unwrap_or(false) { "ON" } else { "OFF" }.to_string()),
        ("frost", th.get("frost").cloned().unwrap_or_else(|| "5.0".into())),
        ("climate/preset", if th.get("away").map(|s| s == "on").unwrap_or(false) { "away" } else { "none" }.to_string()),
        ("away_temp", th.get("away_temp").cloned().unwrap_or_else(|| "15.0".into())),
        ("schedule/text", std::fs::read_to_string(p("/var/lib/j49/schedule")).unwrap_or_default()),
    ];
    // the week, one topic per day and one with everything as JSON
    let week = week_of(&std::fs::read_to_string(p("/var/lib/j49/schedule")).unwrap_or_default());
    for (i, topic) in DAY_TOPICS.iter().enumerate() {
        v.push((*topic, day_text(&week[i])));
    }
    v.push(("schedule/week", week_json(&week)));
    v.push(("schedule/points", week.iter().map(|d| d.len()).sum::<usize>().to_string()));
    // away as a plain switch too (the climate entity has it as its preset)
    v.push(("away", if th.get("away").map(|s| s == "on").unwrap_or(false) { "ON" } else { "OFF" }.to_string()));
    if fresh(&heat, 30) {
        let demand = heat.get("demand").map(|s| s == "1").unwrap_or(false);
        v.push(("climate/action", if demand { "heating" } else if mode == "off" { "off" } else { "idle" }.to_string()));
        if let Some(r) = heat.get("room") {
            v.push(("climate/current", format!("{:.1}", r.parse::<f32>().unwrap_or(0.0))));
        }
        v.push(("relay", heat.get("relay").map(|s| if s == "1" { "ON" } else { "OFF" }).unwrap_or("unknown").to_string()));
        if heat.get("sched").map(|s| s == "1").unwrap_or(false) {
            let hm: u32 = heat.get("next_hhmm").and_then(|s| s.parse().ok()).unwrap_or(0);
            let x: i32 = heat.get("next_x10").and_then(|s| s.parse().ok()).unwrap_or(0);
            v.push(("schedule/next", format!("{:02}:{:02} {}.{}", hm / 100, hm % 100, x / 10, x % 10)));
        } else {
            v.push(("schedule/next", String::new()));
        }
    }
    if fresh(&sht, 900) {
        if let Some(rh) = sht.get("rh") {
            v.push(("climate/humidity", format!("{:.0}", rh.parse::<f32>().unwrap_or(0.0))));
        }
    }
    if fresh(&pir, 30) {
        let mt: u64 = pir.get("motion_t").and_then(|s| s.parse().ok()).unwrap_or(0);
        v.push(("motion", if now <= mt + 30 { "ON" } else { "OFF" }.to_string()));
    }
    if fresh(&als, 30) {
        if let Some(x) = als.get("vis") {
            v.push(("light", x.clone()));
        }
    }
    // the sleep manager: asleep or awake (it reports "asleep" just before it suspends, so this
    // message still gets out), and what woke the device last
    let sl = kv("/run/j49-sleep.state");
    if let Some(st) = sl.get("state") {
        v.push(("sleep/state", if st == "asleep" { "slaapt" } else { "wakker" }.to_string()));
        let why = match sl.get("last_wake").map(|s| s.as_str()).unwrap_or("") {
            "backplate" => "backplate",
            "wifi" => "wifi of ring",
            "timer" => "timer",
            "" => "",
            _ => "onbekend",
        };
        v.push(("sleep/last_wake", why.to_string()));
        v.push(("sleep/last_slept", sl.get("last_slept").cloned().unwrap_or_else(|| "0".into())));
    }
    // the tune played last: a name from the list, or a melody of the user's own
    let tune = std::fs::read_to_string(p("/run/j49-mqtt.tune")).unwrap_or_default();
    if TUNES.contains(&tune.as_str()) {
        v.push(("tune", tune));
    } else if !tune.is_empty() {
        v.push(("tune/custom", tune));
    }
    // which device this is
    v.push(("id/mac", mac_colons()));
    if let Ok(ip) = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|u| u.connect("192.0.2.1:9").and_then(|_| u.local_addr())) {
        v.push(("id/ip", ip.ip().to_string()));
    }
    // volume of the piezo and brightness of the panel (j49-screen's settings file)
    let sc = kv("/var/lib/j49-screen/settings");
    v.push(("volume", sc.get("volume").cloned().unwrap_or_else(|| "5".into())));
    v.push(("brightness", sc.get("brightness").cloned().unwrap_or_else(|| "100".into())));
    // the last text the screen was asked to say (only a screen with a talking character)
    if screen_can("say") {
        v.push(("say", std::fs::read_to_string(p("/run/j49-mqtt.say")).unwrap_or_default()));
    }
    // sleeping allowed (j49-sleep acts on it)
    v.push(("sleep", if kv("/var/lib/j49/sleep").get("enabled").map(|s| s == "on").unwrap_or(true) { "ON" } else { "OFF" }.to_string()));
    // the panel: ON = lit, OFF = asleep (j49-screen reports it)
    if let Some(a) = kv("/run/j49-screen.state").get("asleep") {
        v.push(("screen", if a == "1" { "OFF" } else { "ON" }.to_string()));
    }
    if let Ok(s) = std::fs::read_to_string(p("/sys/bus/iio/devices/iio:device0/in_voltage0_input")) {
        if let Ok(mv) = s.trim().parse::<f32>() {
            v.push(("battery", format!("{:.2}", mv * 3.0 / 1000.0)));
        }
    }
    v
}
/// A command arrived on `<prefix>/<what>/set`.
fn command(what: &str, payload: &str) {
    let payload = payload.trim();
    match what {
        "climate/mode" if payload == "heat" || payload == "off" => set_key("/var/lib/j49/thermostat", "mode", payload),
        "climate/setpoint" => {
            if let Ok(t) = payload.parse::<f32>() {
                // half degrees, inside the range the device accepts
                let t = (t * 2.0).round() / 2.0;
                set_key("/var/lib/j49/thermostat", "setpoint", &format!("{:.1}", t.clamp(9.0, 32.0)));
            }
        }
        "schedule/enabled" if payload == "ON" || payload == "OFF" => {
            set_key("/var/lib/j49/thermostat", "schedule", if payload == "ON" { "on" } else { "off" });
            let _ = std::fs::remove_file(p("/var/lib/j49/schedule.state")); // apply the current change
        }
        "sleep" if payload == "ON" || payload == "OFF" => set_key("/var/lib/j49/sleep", "enabled", if payload == "ON" { "on" } else { "off" }),
        what if DAY_TOPICS.contains(&what) => {
            let i = DAY_TOPICS.iter().position(|t| *t == what).unwrap();
            let mut week = week_of(&std::fs::read_to_string(p("/var/lib/j49/schedule")).unwrap_or_default());
            match parse_day_text(payload) {
                Some(day) => {
                    week[i] = day;
                    save_week(&week);
                }
                None => eprintln!("schedule for {} refused: expected like \"06:30 20.0, 22:30 16.0\"", DAYS[i]),
            }
        }
        "schedule/week" => {
            let week = week_of(&std::fs::read_to_string(p("/var/lib/j49/schedule")).unwrap_or_default());
            match week_from_json(payload, week) {
                Some(w) => save_week(&w),
                None => eprintln!("schedule/week refused: not the expected JSON"),
            }
        }
        "tune" | "tune/custom" if !payload.is_empty() => {
            // j49-screen plays it on the piezo: a name from TUNES or a melody in RTTTL
            let text: String = payload.chars().filter(|c| !c.is_control()).take(600).collect();
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(0o4000).open(p("/run/j49-screen.fifo")) {
                let _ = f.write_all(format!("tune {text}\n").as_bytes());
            }
            let _ = std::fs::write(p("/run/j49-mqtt.tune"), &text);
        }
        // j49-screen reads its settings file again when it changes
        "volume" => {
            if let Ok(n) = payload.parse::<f32>() {
                set_key("/var/lib/j49-screen/settings", "volume", &format!("{}", (n.round() as i32).clamp(0, 10)));
            }
        }
        "brightness" => {
            if let Ok(n) = payload.parse::<f32>() {
                set_key("/var/lib/j49-screen/settings", "brightness", &format!("{}", (n.round() as i32).clamp(10, 120)));
            }
        }
        "say" if !payload.is_empty() => {
            // a screen program that can speak says it (FIFO command "say"): one line, at
            // most 200 characters, no control characters
            let text: String = payload.chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect();
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(0o4000).open(p("/run/j49-screen.fifo")) {
                let _ = f.write_all(format!("say {text}\n").as_bytes());
            }
            let _ = std::fs::write(p("/run/j49-mqtt.say"), &text);
        }
        "screen" if payload == "ON" || payload == "OFF" => {
            // j49-screen's control FIFO; opened without blocking, so a stopped j49-screen cannot
            // hang this program. A woken panel goes back to sleep by itself after its idle time.
            use std::os::unix::fs::OpenOptionsExt;
            const O_NONBLOCK: i32 = 0o4000;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(O_NONBLOCK).open(p("/run/j49-screen.fifo")) {
                let _ = f.write_all(if payload == "ON" { b"wake\n" } else { b"sleep\n" });
            }
        }
        "climate/preset" if payload == "away" || payload == "none" => {
            set_key("/var/lib/j49/thermostat", "away", if payload == "away" { "on" } else { "off" })
        }
        "away" if payload == "ON" || payload == "OFF" => set_key("/var/lib/j49/thermostat", "away", if payload == "ON" { "on" } else { "off" }),
        "away_temp" => {
            if let Ok(t) = payload.parse::<f32>() {
                let t = (t * 2.0).round() / 2.0;
                set_key("/var/lib/j49/thermostat", "away_temp", &format!("{:.1}", t.clamp(9.0, 32.0)));
            }
        }
        "frost" => {
            if let Ok(t) = payload.parse::<f32>() {
                set_key("/var/lib/j49/thermostat", "frost", &format!("{:.1}", t.clamp(0.0, 12.0)));
            }
        }
        "schedule/text" if !payload.is_empty() && payload.len() < 4000 => {
            let full = p("/var/lib/j49/schedule");
            let tmp = format!("{full}.mqtt");
            if std::fs::write(&tmp, format!("{payload}\n")).is_ok() {
                let _ = std::fs::rename(&tmp, &full);
                let _ = std::fs::remove_file(p("/var/lib/j49/schedule.state"));
            }
        }
        _ => eprintln!("ignored command {what}"),
    }
}
fn device_id() -> String {
    std::fs::read_to_string(p("/sys/class/net/wlan0/address"))
        .map(|s| s.trim().replace(':', ""))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "j49".into())
}
fn mac_colons() -> String {
    std::fs::read_to_string(p("/sys/class/net/wlan0/address")).map(|s| s.trim().to_string()).unwrap_or_default()
}
/// Last n hex digits of the MAC address, upper case ("A1B2"): tells two devices apart.
fn mac_tail(n: usize) -> String {
    let id = device_id();
    id[id.len().saturating_sub(n)..].to_ascii_uppercase()
}
fn default_prefix() -> String {
    format!("j49_{}", mac_tail(6).to_ascii_lowercase())
}
fn default_name() -> String {
    format!("{} {}", tm("device"), mac_tail(4))
}
/// Home Assistant discovery: (topic, JSON config) for every entity.
/// What the screen program on this device can do: words in /run/j49-screen.caps ("tune",
/// "say"). The generic screen plays tunes; a screen with a talking character adds "say".
fn screen_can(what: &str) -> bool {
    std::fs::read_to_string(p("/run/j49-screen.caps")).map(|s| s.split_whitespace().any(|w| w == what)).unwrap_or(false)
}
fn discovery(c: &Cfg) -> Vec<(String, String)> {
    let mut all = discovery_base(c);
    // entities for things this screen cannot do are not announced
    all.retain(|(topic, _)| {
        (screen_can("say") || !topic.ends_with("/say/config")) && (screen_can("tune") || !(topic.ends_with("/tune/config") || topic.ends_with("/tune_custom/config")))
    });
    let id = device_id();
    let pre = &c.prefix;
    // The day entities keep the object ids they always had (the Dutch day codes), so an update
    // changes the entities Home Assistant already has and does not add seven new ones.
    let days_text = tm("days");
    let day_names: Vec<&str> = days_text.split(',').map(|d| d.trim()).collect();
    for (i, day) in OLD_DAYS.iter().enumerate() {
        all.push((
            format!("{}/text/j49_{id}/schedule_{day}/config", c.discovery),
            format!(
                r#"{{"name":"@schedule_day@ {}","unique_id":"j49_{id}_schedule_{day}","state_topic":"{pre}/schedule/{en}","command_topic":"{pre}/schedule/{en}/set","max":255,"icon":"mdi:calendar-clock","entity_category":"config","qos":1,"device":{{"identifiers":["j49_{id}"]}},"availability_topic":"{pre}/status"}}"#,
                day_names.get(i).copied().unwrap_or(DAYS[i]),
                en = DAYS[i]
            ),
        ));
    }
    // "@key@" in a name is the text mqtt.<key> of the chosen language
    for (_, json) in all.iter_mut() {
        while let Some(a) = json.find("\"name\":\"@") {
            let start = a + 8;
            let Some(len) = json[start + 1..].find('@') else { break };
            let text = tm(&json[start + 1..start + 1 + len]).replace(['"', '\\'], "");
            json.replace_range(start..start + len + 2, &text);
        }
    }
    all
}
fn discovery_base(c: &Cfg) -> Vec<(String, String)> {
    let id = device_id();
    let pre = &c.prefix;
    let sw = kv("/etc/j49-release").get("commit").map(|s| s.chars().take(8).collect::<String>()).unwrap_or_default();
    // the link "Visit" on the device page in Home Assistant: the web page of the device (j49-web)
    let url = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|u| u.connect((c.host.as_str(), c.port)).and_then(|_| u.local_addr()))
        .map(|a| format!(r#","configuration_url":"http://{}/""#, a.ip()))
        .unwrap_or_default();
    // the name as a JSON string, and the MAC address as a "connection" Home Assistant shows
    let name = format!("\"{}\"", c.name.replace('\\', "").replace('"', "'"));
    let mac = mac_colons();
    let conn = if mac.len() == 17 { format!(r#""connections":[["mac","{mac}"]],"#) } else { String::new() };
    let dev = format!(
        r#""device":{{"identifiers":["j49_{id}"],{conn}"name":{name},"manufacturer":"nest-yocto","model":"@model@","sw_version":"{sw}"{url}}},"qos":1,"availability_topic":"{pre}/status""#
    );
    let dev = dev.replace("@model@", &tm("model").replace(['"', '\\'], ""));
    let t = |kind: &str, obj: &str| format!("{}/{kind}/j49_{id}/{obj}/config", c.discovery);
    vec![
        (
            t("climate", "thermostat"),
            format!(
                r#"{{"name":"@climate@","unique_id":"j49_{id}_climate","modes":["off","heat"],"mode_state_topic":"{pre}/climate/mode","mode_command_topic":"{pre}/climate/mode/set","temperature_state_topic":"{pre}/climate/setpoint","temperature_command_topic":"{pre}/climate/setpoint/set","current_temperature_topic":"{pre}/climate/current","current_humidity_topic":"{pre}/climate/humidity","action_topic":"{pre}/climate/action","preset_modes":["away"],"preset_mode_state_topic":"{pre}/climate/preset","preset_mode_command_topic":"{pre}/climate/preset/set","min_temp":9,"max_temp":32,"temp_step":0.5,"temperature_unit":"C",{dev}}}"#
            ),
        ),
        (
            t("sensor", "temperature"),
            format!(r#"{{"name":"@temp@","unique_id":"j49_{id}_temp","state_topic":"{pre}/climate/current","device_class":"temperature","unit_of_measurement":"°C","state_class":"measurement",{dev}}}"#),
        ),
        (
            t("sensor", "humidity"),
            format!(r#"{{"name":"@rh@","unique_id":"j49_{id}_rh","state_topic":"{pre}/climate/humidity","device_class":"humidity","unit_of_measurement":"%","state_class":"measurement",{dev}}}"#),
        ),
        (
            t("binary_sensor", "motion"),
            format!(r#"{{"name":"@motion@","unique_id":"j49_{id}_motion","state_topic":"{pre}/motion","device_class":"motion",{dev}}}"#),
        ),
        (
            t("binary_sensor", "relay"),
            format!(r#"{{"name":"@relay@","unique_id":"j49_{id}_relay","state_topic":"{pre}/relay","device_class":"heat",{dev}}}"#),
        ),
        (
            t("sensor", "light"),
            format!(r#"{{"name":"@light@","unique_id":"j49_{id}_light","state_topic":"{pre}/light","state_class":"measurement",{dev}}}"#),
        ),
        (
            t("sensor", "battery"),
            format!(r#"{{"name":"@battery@","unique_id":"j49_{id}_batt","state_topic":"{pre}/battery","device_class":"voltage","unit_of_measurement":"V","state_class":"measurement","entity_category":"diagnostic",{dev}}}"#),
        ),
        (
            t("switch", "screen"),
            format!(r#"{{"name":"@screen@","unique_id":"j49_{id}_screen","state_topic":"{pre}/screen","command_topic":"{pre}/screen/set","icon":"mdi:monitor",{dev}}}"#),
        ),
        (
            t("sensor", "sleep_state"),
            format!(r#"{{"name":"@sleep_state@","unique_id":"j49_{id}_sleep_state","state_topic":"{pre}/sleep/state","icon":"mdi:power-sleep",{dev}}}"#),
        ),
        (
            t("sensor", "sleep_last_wake"),
            format!(r#"{{"name":"@last_wake@","unique_id":"j49_{id}_last_wake","state_topic":"{pre}/sleep/last_wake","icon":"mdi:alarm","entity_category":"diagnostic",{dev}}}"#),
        ),
        (
            t("sensor", "sleep_last_slept"),
            format!(r#"{{"name":"@last_slept@","unique_id":"j49_{id}_last_slept","state_topic":"{pre}/sleep/last_slept","unit_of_measurement":"s","icon":"mdi:timer-outline","entity_category":"diagnostic",{dev}}}"#),
        ),
        (
            t("select", "tune"),
            format!(r#"{{"name":"@tune@","unique_id":"j49_{id}_tune","state_topic":"{pre}/tune","command_topic":"{pre}/tune/set","options":[{tunes}],"icon":"mdi:music-note",{dev}}}"#, tunes = TUNES.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(",")),
        ),
        (
            t("text", "tune_custom"),
            format!(r#"{{"name":"@tune_custom@","unique_id":"j49_{id}_tune_custom","state_topic":"{pre}/tune/custom","command_topic":"{pre}/tune/custom/set","max":255,"icon":"mdi:music",{dev}}}"#),
        ),
        (
            t("number", "volume"),
            format!(r#"{{"name":"@volume@","unique_id":"j49_{id}_volume","state_topic":"{pre}/volume","command_topic":"{pre}/volume/set","min":0,"max":10,"step":1,"mode":"slider","icon":"mdi:volume-high","entity_category":"config",{dev}}}"#),
        ),
        (
            t("number", "brightness"),
            format!(r#"{{"name":"@brightness@","unique_id":"j49_{id}_brightness","state_topic":"{pre}/brightness","command_topic":"{pre}/brightness/set","min":10,"max":120,"step":2,"mode":"slider","icon":"mdi:brightness-6","entity_category":"config",{dev}}}"#),
        ),
        (
            t("sensor", "mac"),
            format!(r#"{{"name":"@mac@","unique_id":"j49_{id}_mac","state_topic":"{pre}/id/mac","icon":"mdi:identifier","entity_category":"diagnostic",{dev}}}"#),
        ),
        (
            t("sensor", "ip"),
            format!(r#"{{"name":"@ip@","unique_id":"j49_{id}_ip","state_topic":"{pre}/id/ip","icon":"mdi:ip-network","entity_category":"diagnostic",{dev}}}"#),
        ),
        (
            t("text", "say"),
            format!(r#"{{"name":"@say@","unique_id":"j49_{id}_say","state_topic":"{pre}/say","command_topic":"{pre}/say/set","max":200,"icon":"mdi:robot-outline",{dev}}}"#),
        ),
        (
            t("switch", "away"),
            format!(r#"{{"name":"@away@","unique_id":"j49_{id}_away","state_topic":"{pre}/away","command_topic":"{pre}/away/set","icon":"mdi:home-export-outline",{dev}}}"#),
        ),
        (
            // the whole week as attributes (ma .. zo, each a list of time and temp), for a card
            t("sensor", "schedule_week"),
            format!(r#"{{"name":"@week@","unique_id":"j49_{id}_schedule_week","state_topic":"{pre}/schedule/points","json_attributes_topic":"{pre}/schedule/week","unit_of_measurement":"tijdstippen","icon":"mdi:calendar-week",{dev}}}"#),
        ),
        (
            t("switch", "sleep"),
            format!(r#"{{"name":"@sleep@","unique_id":"j49_{id}_sleep","state_topic":"{pre}/sleep","command_topic":"{pre}/sleep/set","icon":"mdi:sleep","entity_category":"config",{dev}}}"#),
        ),
        (
            t("switch", "schedule"),
            format!(r#"{{"name":"@schedule@","unique_id":"j49_{id}_schedule","state_topic":"{pre}/schedule/enabled","command_topic":"{pre}/schedule/enabled/set",{dev}}}"#),
        ),
        (
            t("sensor", "schedule_next"),
            format!(r#"{{"name":"@next@","unique_id":"j49_{id}_next","state_topic":"{pre}/schedule/next",{dev}}}"#),
        ),
        (
            t("number", "away_temp"),
            format!(r#"{{"name":"@away_temp@","unique_id":"j49_{id}_away_temp","state_topic":"{pre}/away_temp","command_topic":"{pre}/away_temp/set","min":9,"max":32,"step":0.5,"unit_of_measurement":"°C","entity_category":"config",{dev}}}"#),
        ),
        (
            t("number", "frost"),
            format!(r#"{{"name":"@frost@","unique_id":"j49_{id}_frost","state_topic":"{pre}/frost","command_topic":"{pre}/frost/set","min":0,"max":12,"step":0.5,"unit_of_measurement":"°C","entity_category":"config",{dev}}}"#),
        ),
    ]
}

fn session(c: &Cfg) -> std::io::Result<()> {
    let mut s = TcpStream::connect((c.host.as_str(), c.port))?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let will = format!("{}/status", c.prefix);
    s.write_all(&connect_packet(&format!("j49-{}", device_id()), &c.user, &c.pass, &will, c.keepalive))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    // wait for CONNACK
    let t0 = Instant::now();
    loop {
        if let Some((first, body)) = take_packet(&mut buf) {
            if first >> 4 == 2 {
                if body.get(1) != Some(&0) {
                    let why = match body.get(1) {
                        Some(4) => "bad user name or password",
                        Some(5) => "not authorized: set user= and password= in /var/lib/j49/mqtt.conf",
                        _ => "see the MQTT 3.1.1 return code",
                    };
                    eprintln!("broker refused the connection (code {:?}): {why}", body.get(1));
                    status("refused", if body.get(1) == Some(&4) { "bad-credentials" } else { "not-authorized" });
                    return Err(std::io::Error::other("refused (connack)"));
                }
                break;
            }
        }
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(std::io::Error::other("no answer from the broker"));
        }
        match s.read(&mut tmp) {
            Ok(0) => return Err(std::io::Error::other("closed")),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            // Interrupted: the freezer at suspend/resume breaks off the read; that is not an error
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
    }
    eprintln!("connected to {}:{}", c.host, c.port);
    let stamp = cfg_stamp();
    let mut next_alive = Instant::now();
    let (mut seen_mono, mut seen_wall) = (Instant::now(), unix_now());
    s.write_all(&publish_packet(&will, "online", true))?;
    s.write_all(&subscribe_packet(1, &format!("{}/+/set", c.prefix)))?;
    s.write_all(&subscribe_packet(2, &format!("{}/+/+/set", c.prefix)))?;
    for (topic, cfg) in discovery(c) {
        s.write_all(&publish_packet(&topic, &cfg, true))?;
    }
    let mut last: HashMap<&'static str, String> = HashMap::new();
    let (mut next_pub, mut next_full, mut next_ping) = (Instant::now(), Instant::now(), Instant::now() + Duration::from_secs(c.keepalive as u64 / 2));
    loop {
        match s.read(&mut tmp) {
            Ok(0) => return Err(std::io::Error::other("closed")),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            // Interrupted: the freezer at suspend/resume breaks off the read; that is not an error
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
        while let Some((first, body)) = take_packet(&mut buf) {
            if first >> 4 == 3 && body.len() >= 2 {
                let tl = u16::from_be_bytes([body[0], body[1]]) as usize;
                // QoS 1 and 2 carry a packet id between topic and payload; QoS 1 wants a PUBACK
                let qos = (first >> 1) & 3;
                let skip = if qos > 0 { 2 } else { 0 };
                if body.len() >= 2 + tl + skip {
                    if qos == 1 {
                        s.write_all(&[0x40, 0x02, body[2 + tl], body[3 + tl]])?;
                    }
                    let topic = String::from_utf8_lossy(&body[2..2 + tl]).to_string();
                    let payload = String::from_utf8_lossy(&body[2 + tl + skip..]).to_string();
                    if let Some(what) = topic.strip_prefix(&format!("{}/", c.prefix)).and_then(|t| t.strip_suffix("/set")) {
                        eprintln!("command {what}");
                        command(what, &payload);
                        next_pub = Instant::now(); // report the new state at once
                    }
                }
            }
        }
        if Instant::now() >= next_pub {
            next_pub = Instant::now() + Duration::from_secs(2);
            let full = Instant::now() >= next_full;
            if full {
                next_full = Instant::now() + Duration::from_secs(60);
            }
            for (topic, value) in states() {
                if full || last.get(topic) != Some(&value) {
                    s.write_all(&publish_packet(&format!("{}/{topic}", c.prefix), &value, true))?;
                    last.insert(topic, value);
                }
            }
        }
        // Woken from suspend: timers here did not count the sleep, the broker did. Say hello at
        // once (a PINGREQ) and publish everything.
        let (mono, wall) = (Instant::now(), unix_now());
        if wall.saturating_sub(seen_wall) > mono.duration_since(seen_mono).as_secs() + 5 {
            s.write_all(&[0xc0, 0x00])?;
            next_ping = Instant::now() + Duration::from_secs(c.keepalive as u64 / 2);
            next_pub = Instant::now();
            next_full = Instant::now();
        }
        (seen_mono, seen_wall) = (mono, wall);
        if Instant::now() >= next_alive {
            next_alive = Instant::now() + Duration::from_secs(3);
            status("connected", &format!("{}:{}", c.host, c.port));
            if cfg_stamp() != stamp {
                eprintln!("broker settings changed: connecting again");
                let _ = s.write_all(&publish_packet(&will, "offline", true));
                return Ok(());
            }
        }
        if Instant::now() >= next_ping {
            next_ping = Instant::now() + Duration::from_secs(c.keepalive as u64 / 2);
            s.write_all(&[0xc0, 0x00])?;
        }
    }
}

fn main() {
    let mut said_unconfigured = false;
    loop {
        let stamp = cfg_stamp();
        let mut refused = false;
        match read_cfg() {
            None => {
                if !said_unconfigured {
                    eprintln!("no broker configured in /var/lib/j49/mqtt.conf (host=...)");
                    said_unconfigured = true;
                }
                status("unconfigured", "");
            }
            Some(c) => {
                said_unconfigured = false;
                if let Err(e) = session(&c) {
                    eprintln!("mqtt session ended: {e}");
                    refused = e.to_string().contains("refused (connack)");
                    if !refused {
                        status("offline", &e.to_string());
                    }
                }
            }
        }
        // try again after 30 s, or at once when the settings change
        for i in 0..30 {
            std::thread::sleep(Duration::from_secs(1));
            if cfg_stamp() != stamp {
                break;
            }
            if i % 10 == 9 && read_cfg().is_none() {
                status("unconfigured", "");
            } else if i % 10 == 9 && refused {
                status("refused", "naam en wachtwoord nodig of fout");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn week_schedule() {
        let file = "# head\nma-vr 06:30 20.0\nmon-fri 22:30 16.0\nza,zo 08:00 20.5\ndagelijks 23:00 15.0\nxx 01:00 20\n";
        let w = week_of(file);
        assert_eq!(day_text(&w[0]), "06:30 20.0, 22:30 16.0, 23:00 15.0");
        assert_eq!(day_text(&w[5]), "08:00 20.5, 23:00 15.0");
        // file -> week -> file -> week is stable, and equal days are written as one line
        let again = file_of(&w);
        assert_eq!(week_of(&again), w);
        assert!(again.contains("mon,tue,wed,thu,fri 06:30 20.0\n") && again.contains("sat,sun 08:00 20.5\n") && again.contains("daily 23:00 15.0\n"));
        // the day text
        assert_eq!(parse_day_text("22:30 16, 6:30=20.5 ; 8:30 17"), Some(vec![(390, 20.5), (510, 17.0), (1350, 16.0)]));
        assert_eq!(parse_day_text(""), Some(vec![]));
        assert_eq!(parse_day_text("25:00 20"), None);
        assert_eq!(parse_day_text("06:30 40"), None);
        assert_eq!(parse_day_text("zes uur"), None);
        // JSON out and back in; a partial message changes only the days it names
        assert_eq!(week_from_json(&week_json(&w), Default::default()), Some(w.clone()));
        let w2 = week_from_json(r#"{"wo":[{"time":"07:00","temp":21}], "zo":[["09:00",19.5]], "ma":"05:00 18"}"#, w.clone()).unwrap();
        assert_eq!(day_text(&w2[2]), "07:00 21.0");
        assert_eq!(day_text(&w2[6]), "09:00 19.5");
        assert_eq!(day_text(&w2[0]), "05:00 18.0");
        assert_eq!(w2[1], w[1]);
        assert_eq!(week_from_json(r#"{"xx":[]}"#, w.clone()), None);
        assert_eq!(week_from_json(r#"{"ma":[{"time":"07:00","temp":99}]}"#, w.clone()), None);
        assert_eq!(week_from_json("ma-vr 06:30 20", w.clone()), None);
    }
    #[test]
    fn names_per_language() {
        // English without a ui file; every "@key@" in the discovery messages has a text
        assert_eq!(tm("climate"), "Thermostat");
        assert_eq!(tm("days").split(',').count(), 7);
        let c = Cfg { host: "h".into(), port: 1883, user: String::new(), pass: String::new(), name: "N".into(), prefix: "p".into(), discovery: "homeassistant".into(), keepalive: 60 };
        for (topic, json) in discovery(&c) {
            assert!(!json.contains('@'), "{topic}: {json}");
        }
        assert!(discovery(&c).iter().any(|(t, j)| t.ends_with("/schedule_ma/config") && j.contains("\"name\":\"Schedule Monday\"") && j.contains("p/schedule/mon/set")));
    }
    #[test]
    fn packets() {
        let mut v = Vec::new();
        varint(321, &mut v);
        assert_eq!(v, [0xc1, 0x02]);
        assert_eq!(publish_packet("a/b", "x", true), [0x31, 6, 0, 3, b'a', b'/', b'b', b'x']);
        let mut buf = publish_packet("t", "hello", false);
        buf.extend_from_slice(&[0xd0, 0x00, 0x30]); // a ping response and a partial packet
        assert_eq!(take_packet(&mut buf), Some((0x30, vec![0, 1, b't', b'h', b'e', b'l', b'l', b'o'])));
        assert_eq!(take_packet(&mut buf), Some((0xd0, vec![])));
        assert_eq!(take_packet(&mut buf), None);
        assert_eq!(buf, [0x30]);
        let c = connect_packet("id", "u", "p", "w", 60);
        assert_eq!(c[0], 0x10);
        assert_eq!(&c[2..9], &[0, 4, b'M', b'Q', b'T', b'T', 4]);
        assert_eq!(c[9], 0x04 | 0x20 | 0x80 | 0x40); // no clean session
        assert_eq!(subscribe_packet(1, "a").last(), Some(&1)); // QoS 1
    }
}
