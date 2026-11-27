// j49-sleep: let the head unit suspend when nothing needs it (docs/sleep-j49.md).
//
//   j49-sleep run      the service
//   j49-sleep on|off   allow or forbid sleeping (same as the switch on the web page and in MQTT)
//   j49-sleep status   what it is doing and why
//
// Sleeping is OFF unless /var/lib/j49/sleep says enabled=on. Even then it only sleeps when:
// the panel is asleep, the heating neither runs nor is about to, nobody is logged in over SSH,
// the web page was not used in the last minute and no login question is open.
//
// One sleep: wake patterns into the WiFi chip (MQTT broker, web page, SSH), backplate into its
// quiet mode through bpd (wake on its timer, on temperature, and on motion if wanted), RTC alarm
// as the safety net, then "mem" to /sys/power/state. Afterwards it tells bpd to talk again and
// records what woke the board.
//
// Settings in /var/lib/j49/sleep (key=value): enabled=on|off, idle=<s awake after a wake, 20>,
// max=<longest sleep in s, 1500>, motion=on|off (wake on movement, on).
use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

const SETTINGS: &str = "/var/lib/j49/sleep";
const STATE: &str = "/run/j49-sleep.state";
const TTY_WAKE: &str = "/sys/devices/platform/68000000.ocp/49020000.serial/tty/ttyS2/power/wakeup";
const RTC_ALARM: &str = "/sys/class/rtc/rtc0/wakealarm";
/// With the heating in mode "heat" the room can cool below the target while we sleep; the
/// control loop must look again at least this often.
const MAX_SLEEP_HEATING: u64 = 300;
// wake mask bits of the backplate (docs/backplate-protocol.md)
const WAKE_TIMER: u16 = 0x0001;
const WAKE_TEMPERATURE: u16 = 0x0004;
const WAKE_PIR: u16 = 0x0008;
const WAKE_NEAR_PIR: u16 = 0x0080;

fn kv_text(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split('#').next().unwrap_or("").split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}
fn kv(path: &str) -> HashMap<String, String> {
    kv_text(&std::fs::read_to_string(path).unwrap_or_default())
}
fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
fn num(m: &HashMap<String, String>, k: &str) -> Option<u64> {
    m.get(k).and_then(|v| v.parse().ok())
}
/// Seconds since a file was last written, None when it is not there.
fn age(path: &str) -> Option<u64> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs())
}

struct Cfg {
    enabled: bool,
    idle: u64,
    max: u64,
    motion: bool,
}
fn read_cfg() -> Cfg {
    let m = kv(SETTINGS);
    Cfg {
        enabled: m.get("enabled").map(|v| v == "on").unwrap_or(false),
        idle: num(&m, "idle").unwrap_or(20).clamp(10, 600),
        max: num(&m, "max").unwrap_or(1500).clamp(30, 1500),
        motion: m.get("motion").map(|v| v != "off").unwrap_or(true),
    }
}
fn set_enabled(on: bool) {
    let old = std::fs::read_to_string(SETTINGS).unwrap_or_default();
    let mut out: String = old.lines().filter(|l| !l.trim_start().starts_with("enabled=")).map(|l| format!("{l}\n")).collect();
    out.push_str(&format!("enabled={}\n", if on { "on" } else { "off" }));
    let _ = std::fs::create_dir_all("/var/lib/j49");
    let tmp = format!("{SETTINGS}.tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, SETTINGS);
    }
}

/// The backplate answers bpd right now (a fresh motion file means frames are coming in).
fn backplate_present() -> bool {
    num(&kv("/run/bpd/pir"), "t").map(|t| unix_now() <= t + 20).unwrap_or(false)
}
fn ssh_sessions() -> usize {
    // the listener is one dropbear; every session is one more
    let n = std::fs::read_dir("/proc")
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
                .filter(|e| std::fs::read_to_string(e.path().join("comm")).map(|c| c.trim() == "dropbear").unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    n.saturating_sub(1)
}
/// MQTT keep-alive of j49-mqtt when a broker is configured.
fn mqtt_keepalive() -> Option<u64> {
    let m = kv("/var/lib/j49/mqtt.conf");
    m.get("host").filter(|h| !h.is_empty())?;
    Some(num(&m, "keepalive").filter(|k| *k >= 10).unwrap_or(60))
}

/// Why the head unit must stay awake now (a short key), or None and how long it may sleep.
fn blocker(c: &Cfg, awake_since: Instant) -> Result<u64, &'static str> {
    if !c.enabled {
        return Err("disabled");
    }
    let uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split('.').next().and_then(|v| v.parse::<u64>().ok())).unwrap_or(0);
    if uptime < 120 {
        return Err("boot");
    }
    if awake_since.elapsed() < Duration::from_secs(c.idle) {
        return Err("just-woke");
    }
    if kv("/run/j49-screen.state").get("asleep").map(|v| v != "1").unwrap_or(true) {
        return Err("screen-on");
    }
    let mut secs = c.max;
    let heat = kv("/run/bpd/heat");
    match num(&heat, "t") {
        Some(t) if unix_now() <= t + 30 => {
            if num(&heat, "demand") == Some(1) || num(&heat, "relay") == Some(1) || num(&heat, "wait").unwrap_or(0) > 0 {
                return Err("heat");
            }
            if heat.get("mode").map(|m| m == "heat").unwrap_or(false) {
                secs = secs.min(MAX_SLEEP_HEATING);
            }
        }
        // bpd is not running or stuck: do not sleep on top of that
        _ => return Err("bpd"),
    }
    if ssh_sessions() > 0 {
        return Err("ssh");
    }
    if std::path::Path::new("/run/j49-web/pending").exists() {
        return Err("login");
    }
    if age("/run/j49-web/last").map(|a| a < 60).unwrap_or(false) {
        return Err("web");
    }
    // WiFi is wanted but not connected yet (after a boot the access point takes a minute or
    // two): wait for it. Sleeping now would be without wake patterns, the chip would lose the
    // network in every sleep, and the next sleep would start before it is back: the device
    // then is unreachable whenever it sleeps (seen 2026-11-26). After 3 minutes without a
    // network it may sleep anyway; then the network is simply not there.
    let wifi_wanted = num(&kv("/var/lib/j49-screen/settings"), "wifi").unwrap_or(1) != 0;
    if wifi_wanted && own_ip().is_none() && awake_since.elapsed() < Duration::from_secs(180) {
        return Err("wifi");
    }
    if let Some(ka) = mqtt_keepalive() {
        // j49-mqtt pings at half the keep-alive and the broker gives up at 1.5 times: sleeping
        // 0.8 of it keeps the connection (0.5 + 0.8 < 1.5)
        secs = secs.min(ka * 8 / 10);
        if secs < 20 {
            return Err("mqtt");
        }
    }
    Ok(secs)
}

fn hexip(ip: &str) -> Option<String> {
    let p: Vec<u8> = ip.split('.').filter_map(|x| x.parse().ok()).collect();
    (p.len() == 4 && ip.split('.').count() == 4).then(|| format!("{:02x}:{:02x}:{:02x}:{:02x}", p[0], p[1], p[2], p[3]))
}
/// The wake patterns for the WiFi chip: (name, pattern in iw's notation). Offsets are bytes of
/// the Ethernet frame; the chip takes no offset, so the 12 address bytes are "not compared".
fn wow_patterns(own_ip: &str, broker: Option<(&str, u16)>) -> Vec<(&'static str, String)> {
    let Some(me) = hexip(own_ip) else { return Vec::new() };
    let head = "-:-:-:-:-:-:-:-:-:-:-:-:08:00:45:-:-:-:-:-:-:-:-:06:-:-"; // IPv4, 20-byte header, TCP
    let mut v = Vec::new();
    if let Some((host, port)) = broker {
        if let Some(b) = hexip(host) {
            v.push(("mqtt", format!("{head}:{b}:{me}:{:02x}:{:02x}", port >> 8, port & 255)));
        }
    }
    v.push(("web", format!("{head}:-:-:-:-:{me}:-:-:00:50")));
    v.push(("ssh", format!("{head}:-:-:-:-:{me}:-:-:00:16")));
    v
}
/// Our address on the network, None while there is no route out (WiFi not connected).
fn own_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}
fn set_wowlan() -> bool {
    let Some(ip) = own_ip() else { return false };
    let m = kv("/var/lib/j49/mqtt.conf");
    let host = m.get("host").cloned().unwrap_or_default();
    let port = num(&m, "port").unwrap_or(1883) as u16;
    let pats = wow_patterns(&ip, (!host.is_empty()).then_some((host.as_str(), port)));
    // the driver is bound twice at boot, so the phy is not always phy0
    let phy = Command::new("iw").arg("phy").output().ok().and_then(|o| {
        String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.strip_prefix("Wiphy ").map(|s| s.trim().to_string()))
    });
    let Some(phy) = phy else { return false };
    let mut cmd = Command::new("iw");
    cmd.args(["phy", &phy, "wowlan", "enable", "patterns"]);
    for (_, p) in &pats {
        cmd.arg(p);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}
/// Name of the interrupt that woke the board, as a short key.
fn wake_reason() -> String {
    let irq = std::fs::read_to_string("/sys/power/pm_wakeup_irq").unwrap_or_default().trim().to_string();
    let line = std::fs::read_to_string("/proc/interrupts")
        .unwrap_or_default()
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{irq}:")))
        .unwrap_or("")
        .to_string();
    if line.contains("serial:wakeup") || line.contains("49020000.serial") {
        "backplate".into()
    } else if line.contains("gpio") {
        "wifi".into() // the GPIO bank of the WL1271 interrupt; the ring sensor sits on it too
    } else if line.contains("TWL4030") || line.contains("rtc") || line.contains("twl4030") {
        "timer".into()
    } else {
        "unknown".into()
    }
}
fn write_state(state: &str, why: &str, sleeps: u32, last_slept: u64, last_wake: &str) {
    let tmp = format!("{STATE}.tmp");
    if std::fs::write(&tmp, format!("state={state}\nwhy={why}\nsleeps={sleeps}\nlast_slept={last_slept}\nlast_wake={last_wake}\nt={}\n", unix_now())).is_ok() {
        let _ = std::fs::rename(&tmp, STATE);
    }
}

/// One sleep. Returns (seconds asleep, what woke us), or None when it was called off.
fn sleep_once(c: &Cfg, secs: u64) -> Option<(u64, String)> {
    let wow = set_wowlan();
    let _ = std::fs::write(TTY_WAKE, "enabled");
    let backplate = backplate_present();
    if backplate {
        let mut mask = WAKE_TIMER | WAKE_TEMPERATURE;
        if c.motion {
            mask |= WAKE_PIR | WAKE_NEAR_PIR;
        }
        let _ = std::fs::remove_file("/run/bpd/quiet-end");
        let _ = std::fs::write("/run/bpd/quiet-request", format!("{secs} {mask:04x}\n"));
        // bpd picks the request up within a second and marks the quiet mode with a file
        let t0 = Instant::now();
        while !std::path::Path::new("/run/bpd/quiet").exists() {
            if t0.elapsed() > Duration::from_secs(4) {
                let _ = std::fs::remove_file("/run/bpd/quiet-request");
                let _ = std::fs::write("/run/bpd/quiet-end", "");
                println!("sleep called off: bpd did not put the backplate in quiet mode");
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Frames the backplate had already started still arrive for about a second, and each
        // of them would wake us again at once (seen: a "sleep" of 1 s). Let them pass first.
        std::thread::sleep(Duration::from_millis(1500));
    }
    // the RTC alarm: the only timer without a backplate, the safety net with one
    let _ = std::fs::write(RTC_ALARM, "0");
    let _ = std::fs::write(RTC_ALARM, format!("+{}", secs + if backplate { 20 } else { 0 }));
    let _ = Command::new("sync").status();
    println!("sleeping for at most {secs} s (backplate {}, wake-on-WLAN {})", if backplate { "quiet" } else { "absent" }, if wow { "set" } else { "NOT set" });
    let before = unix_now();
    // the write returns after the wake; it reports an error although the board slept, because
    // two power domains do not reach their target state (docs/sleep-j49.md)
    let _ = std::fs::write("/sys/power/state", "mem");
    let slept = unix_now().saturating_sub(before);
    let reason = wake_reason();
    let _ = std::fs::write(RTC_ALARM, "0");
    if backplate {
        let _ = std::fs::write("/run/bpd/quiet-end", ""); // bpd talks again, the backplate streams
    }
    println!("awake after {slept} s, woken by {reason}");
    Some((slept, reason))
}

fn run() {
    let mut awake_since = Instant::now();
    let (mut sleeps, mut last_slept, mut last_wake) = (0u32, 0u64, String::new());
    let mut last_why = "";
    loop {
        let c = read_cfg();
        match blocker(&c, awake_since) {
            Err(why) => {
                if why != last_why && c.enabled {
                    println!("awake: {why}");
                }
                last_why = why;
                write_state("awake", why, sleeps, last_slept, &last_wake);
                std::thread::sleep(Duration::from_secs(2));
            }
            Ok(secs) => {
                last_why = "";
                write_state("asleep", "", sleeps, last_slept, &last_wake);
                // j49-mqtt looks at this file every 2 s: give it the time to tell the broker that
                // the device goes to sleep, or Home Assistant would never show it
                if mqtt_keepalive().is_some() {
                    std::thread::sleep(Duration::from_millis(2600));
                    if blocker(&c, awake_since).is_err() {
                        continue; // something came up in the meantime
                    }
                }
                if let Some((slept, reason)) = sleep_once(&c, secs) {
                    sleeps += 1;
                    last_slept = slept;
                    last_wake = reason;
                }
                awake_since = Instant::now();
                write_state("awake", "just-woke", sleeps, last_slept, &last_wake);
            }
        }
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("run") => run(),
        Some("on") => {
            set_enabled(true);
            println!("sleeping allowed");
        }
        Some("off") => {
            set_enabled(false);
            println!("sleeping off: the device stays awake");
        }
        Some("status") => {
            let c = read_cfg();
            println!("enabled={} idle={} max={} motion={}", if c.enabled { "on" } else { "off" }, c.idle, c.max, if c.motion { "on" } else { "off" });
            print!("{}", std::fs::read_to_string(STATE).unwrap_or_else(|_| "the service is not running\n".into()));
        }
        _ => {
            eprintln!("usage: j49-sleep run|on|off|status");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patterns() {
        let p = wow_patterns("192.168.1.50", Some(("192.168.1.10", 1883)));
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], ("mqtt", "-:-:-:-:-:-:-:-:-:-:-:-:08:00:45:-:-:-:-:-:-:-:-:06:-:-:c0:a8:01:0a:c0:a8:01:32:07:5b".to_string()));
        assert!(p[1].1.ends_with(":-:-:-:-:c0:a8:01:32:-:-:00:50"));
        assert!(p[2].1.ends_with(":-:-:-:-:c0:a8:01:32:-:-:00:16"));
        // 12 address bytes + type + IP header up to the ports: destination port at bytes 36-37
        assert_eq!(p[2].1.split(':').count(), 38);
        assert_eq!(p[0].1.split(':').count(), 36);
        // a broker given by name gets no pattern; no address, no patterns at all
        assert_eq!(wow_patterns("192.168.1.50", Some(("broker.local", 1883))).len(), 2);
        assert!(wow_patterns("", None).is_empty());
        assert_eq!(hexip("1.2.3"), None);
    }
    #[test]
    fn settings() {
        let m = kv_text("enabled=on\nidle=5   # too short\nmax=99999\nmotion=off\n");
        assert_eq!(m.get("enabled").map(|s| s.as_str()), Some("on"));
        assert_eq!(num(&m, "idle"), Some(5));
        assert_eq!(num(&m, "max"), Some(99999));
    }
}
