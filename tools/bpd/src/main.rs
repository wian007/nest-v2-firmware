//! bpd - talk to the Nest J49 backplate (STM32 "AMBER_BP") over UART3.
//!
//! Frame (both directions, discovered on jump 53; sync confirmed in nlclient's
//! frame builder `fcn.001daf50`):
//!
//! ```text
//! d5 aa 96 | type u16 LE | len u16 LE | payload[len] | crc16 LE
//! ```
//!
//! crc16 = CRC-16/XMODEM (poly 0x1021, init 0, no reflection) over
//! type+len+payload. The backplate prefixes each of its frames with one extra
//! 0xd5 (pad/wake byte), which the decoder skips. Head-unit types are 0x81..0xc3
//! (dispatch table at 0x1c4a in the backplate firmware); backplate types are small.
//!
//! The one frame that matters for running a non-stock kernel: type 0xa1
//! ("quiet message", payload u16 LE = max_sleep_seconds) reloads the backplate's
//! head-unit watchdog countdown. Without it the backplate cold-resets the head
//! unit after 2160 ticks (~29 min); stock nlclient sends it periodically.
//!
//! ```text
//! bpd keepalive [tty] [--interval S] [--max-sleep S]   beat 0xa1 forever, log sensor frames
//! bpd sniff     [tty] [--seconds S]                    decode incoming frames
//! bpd send      [tty] <type-hex> [payload-hex]         send one frame, print replies for 2 s
//! ```
//!
//! Default tty: /dev/ttyS2 (6.6 8250_omap name for UART3; stock calls it ttyO2).
//! Static build: `cargo build --release` in the repo's `nix develop` shell
//! (target armv7-unknown-linux-musleabihf is preconfigured).

use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::exit;
use std::time::{Duration, Instant};

const SYNC: [u8; 3] = [0xd5, 0xaa, 0x96];
const DEFAULT_TTY: &str = "/dev/ttyS2";
/// HU -> BP "quiet message": reloads the watchdog countdown with payload+0x708 ticks.
const TYPE_QUIET: u16 = 0xa1;
/// BP -> HU SHT20 temperature/humidity, centi-units, ~1/min.
const TYPE_SHT20: u16 = 0x02;

/// CRC-16/XMODEM over `d`.
fn crc16(d: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for &b in d {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// Serialise one frame exactly like nlclient's `fcn.001daf50`.
fn encode(ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(9 + payload.len());
    f.extend_from_slice(&SYNC);
    f.extend_from_slice(&ty.to_le_bytes());
    f.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    f.extend_from_slice(payload);
    let c = crc16(&f[3..]);
    f.extend_from_slice(&c.to_le_bytes());
    f
}

#[derive(Debug, PartialEq)]
struct Frame {
    ty: u16,
    payload: Vec<u8>,
    crc_ok: bool,
}

/// Incremental decoder over a byte stream; tolerates the backplate's extra
/// 0xd5 pad bytes and resyncs on garbage.
#[derive(Default)]
struct Decoder {
    buf: Vec<u8>,
    junk: usize,
}

impl Decoder {
    fn push(&mut self, data: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        let mut i = 0;
        while self.buf.len() - i >= 9 {
            if self.buf[i..i + 3] != SYNC {
                if self.buf[i] != 0xd5 {
                    self.junk += 1;
                }
                i += 1;
                continue;
            }
            let len = u16::from_le_bytes([self.buf[i + 5], self.buf[i + 6]]) as usize;
            if len > 1024 {
                i += 1; // implausible length: not a real header
                continue;
            }
            if self.buf.len() - i < 9 + len {
                break; // wait for the rest of the frame
            }
            let body = &self.buf[i + 3..i + 7 + len];
            let want = u16::from_le_bytes([self.buf[i + 7 + len], self.buf[i + 8 + len]]);
            out.push(Frame {
                ty: u16::from_le_bytes([body[0], body[1]]),
                payload: body[4..].to_vec(),
                crc_ok: crc16(body) == want,
            });
            i += 9 + len;
        }
        self.buf.drain(..i);
        out
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn parse_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s == "-" || s.is_empty() {
        return Ok(Vec::new());
    }
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd-length hex string '{s}'"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex '{s}': {e}")))
        .collect()
}

fn describe(f: &Frame) -> String {
    let base = format!(
        "type 0x{:02x} len {:3} {}[{}]",
        f.ty,
        f.payload.len(),
        if f.crc_ok { "" } else { "CRC-BAD " },
        hex(&f.payload)
    );
    match (f.ty, f.payload.len()) {
        (TYPE_SHT20, 4) if f.crc_ok => {
            let t = i16::from_le_bytes([f.payload[0], f.payload[1]]) as f32 / 100.0;
            let rh = u16::from_le_bytes([f.payload[2], f.payload[3]]) as f32 / 10.0;
            format!("{base}  SHT20 {t:.2} C {rh:.2} %RH")
        }
        _ => base,
    }
}

/// Open the UART raw, 115200 8N1, no flow control, non-blocking.
fn open_tty(path: &str) -> io::Result<std::fs::File> {
    let cpath = CString::new(path).unwrap();
    // SAFETY: plain libc calls on a path we own; fd ownership moves into File below.
    unsafe {
        let fd: RawFd = libc::open(cpath.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut t);
        libc::cfsetispeed(&mut t, libc::B115200);
        libc::cfsetospeed(&mut t, libc::B115200);
        t.c_cflag &= !(libc::CRTSCTS | libc::PARENB | libc::CSTOPB | libc::CSIZE);
        t.c_cflag |= libc::CS8 | libc::CLOCAL | libc::CREAD;
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::tcflush(fd, libc::TCIOFLUSH);
        Ok(std::fs::File::from_raw_fd(fd))
    }
}

/// Wait up to `timeout` for readable data; returns false on timeout.
fn wait_readable(fd: RawFd, timeout: Duration) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: valid pollfd array of length 1.
    let r = unsafe { libc::poll(&mut p, 1, timeout.as_millis() as libc::c_int) };
    r > 0 && (p.revents & libc::POLLIN) != 0
}

fn stamp(start: Instant) -> String {
    format!("[{:9.3}]", start.elapsed().as_secs_f64())
}

/// Pump the link: send `tx` every `interval` (if any), decode and print replies,
/// until `duration` elapses (None = forever).
/// Latest SHT20 reading for other programs (j49-screen): /run/bpd/sht20
/// ("temp=21.34\nrh=45.10\nt=<unix seconds>\n"). Written atomically via a temp file.
/// The backplate's state frame 0x000b carries its voltages: /run/bpd/power
/// ("vin=<V>\nvop=<V>\nvbat=<V>\nraw=<hex>\nt=<unix seconds>\n"). Offsets and scaling are from the
/// cuckoo-nest wiki (Vin u16 at 8, /100; Vop at 10 and Vbat at 12, /1000); the raw bytes are kept
/// next to them until that is confirmed on our backplate.
fn publish_power(f: &Frame) {
    if f.ty != 0x000b || f.payload.len() < 14 || !f.crc_ok {
        return;
    }
    let u = |i: usize| u16::from_le_bytes([f.payload[i], f.payload[i + 1]]) as f32;
    let raw: String = f.payload.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all("/run/bpd");
    let tmp = "/run/bpd/power.tmp";
    if std::fs::write(tmp, format!("vin={:.2}\nvop={:.3}\nvbat={:.3}\nraw={raw}\nt={}\n", u(8) / 100.0, u(10) / 1000.0, u(12) / 1000.0, unix_now())).is_ok() {
        let _ = std::fs::rename(tmp, "/run/bpd/power");
    }
}
fn publish_sht20(f: &Frame) {
    publish_power(f);
    if f.ty != TYPE_SHT20 || f.payload.len() != 4 || !f.crc_ok {
        return;
    }
    let t = i16::from_le_bytes([f.payload[0], f.payload[1]]) as f32 / 100.0;
    let rh = u16::from_le_bytes([f.payload[2], f.payload[3]]) as f32 / 10.0;
    LAST_TEMP_C100.store(i16::from_le_bytes([f.payload[0], f.payload[1]]) as i32, std::sync::atomic::Ordering::Relaxed);
    LAST_TEMP_T.store(unix_now(), std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all("/run/bpd");
    let tmp = "/run/bpd/sht20.tmp";
    if std::fs::write(tmp, format!("temp={t:.2}\nrh={rh:.2}\nt={now}\n")).is_ok() {
        let _ = std::fs::rename(tmp, "/run/bpd/sht20");
    }
}

fn pump(
    tty: &mut std::fs::File,
    tx: Option<(&[u8], Duration)>,
    duration: Option<Duration>,
    quiet: bool,
) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let start = Instant::now();
    let fd = tty.as_raw_fd();
    let mut dec = Decoder::default();
    let mut next_tx = Instant::now();
    let mut sent = 0u64;
    let mut buf = [0u8; 512];
    loop {
        if let Some((frame, interval)) = tx {
            if Instant::now() >= next_tx {
                tty.write_all(frame)?;
                tty.flush()?;
                sent += 1;
                if !quiet {
                    println!("{} tx  {}", stamp(start), hex(frame));
                }
                next_tx += interval;
            }
        }
        if let Some(d) = duration {
            if start.elapsed() >= d {
                break;
            }
        }
        let mut wait = Duration::from_millis(250);
        if let Some((_, _)) = tx {
            wait = wait.min(next_tx.saturating_duration_since(Instant::now()));
        }
        if wait_readable(fd, wait) {
            match tty.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    for f in dec.push(&buf[..n]) {
                        publish_sht20(&f);
                        println!("{} rx  {}", stamp(start), describe(&f));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
    }
    eprintln!("done: {} frames sent, {} junk bytes skipped", sent, dec.junk);
    Ok(())
}

// Backplate command IDs, from the cuckoo-nest wiki (backplate/Protocol.md). The 0x00ff reset +
// handshake + power-steal + buffer-poll is what makes the backplate stream its sensors; the
// earlier 0xa1/0xa4 guess from the nlclient disassembly never started streaming on its own.
const CMD_RESET: u16 = 0x00ff; // triggers the startup burst (ends with an 0x0001 "BRK" msg)
const CMD_FET_ACK: u16 = 0x008f;
const CMD_STATUS: u16 = 0x0083; // periodic status request = keepalive (~15 s)
const CMD_POWERSTEAL: u16 = 0x00c0; // payload 00 00 00 00
const CMD_REQ_BUFFERS: u16 = 0x00a2; // request historical data buffers
const CMD_ACK_BUFFERS: u16 = 0x00a3; // acknowledge end of buffers (after 0x002f)
// info queries, paced 50 ms apart during the handshake
const CMD_INFO: [u16; 9] = [0x0090, 0x0098, 0x0099, 0x009d, 0x009b, 0x009c, 0x009f, 0x009e, 0x0090];
const RSP_END_BUFFERS: u16 = 0x002f;
/// BP -> HU FET presence report, part of the reset burst. The head unit must echo this payload
/// back in 0x8f; an empty 0x8f does not start the sensor sampling.
const RSP_FET_PRESENCE: u16 = 0x0004;
static LAST_FET: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
/// FET presence payload of our Backplate-3.2 (captured from stock nlclient), used when the burst
/// did not carry one.
const FET_DEFAULT: [u8; 13] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0];

/// Latest PIR and ambient-light readings for other programs (j49-screen). The backplate sends
/// both about once a second once the sensor handshake is done.
///   /run/bpd/pir: "pir=<level>\nmotion_t=<unix s of last level >= 2>\nt=<unix s>\n"
///   /run/bpd/als: "vis=<counts>\nir=<counts>\nt=<unix s>\n"
/// Unix time of the last PIR frame. PIR arrives about once a second while the backplate streams,
/// so a stale value means the handshake has to be done (again).
static LAST_PIR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- heating: thermostat control through the backplate's one switch (the Heat Link) ----
// j49-screen writes /var/lib/j49/thermostat (mode, setpoint, offset); this loop compares it with
// the SHT20 reading and asks the backplate for heat with 0x82 [circuit 0][state]. The backplate
// answers 0x0006 [circuit][state]. From the firmware (docs/backplate-heatlink-flow.md): only
// circuit 0 exists and the command is accepted only after the wire ACK of the handshake.
const CMD_SWITCH: u16 = 0x0082;
const RSP_ACK_SWITCH: u16 = 0x0006;
const AWAY_DEFAULT_C: f32 = 15.0; // target while away, until away_temp= says otherwise
const HYSTERESIS_C: f32 = 0.3; // on below setpoint-0.3, off above setpoint+0.3
// Defaults fit a district-heating valve, where only chatter has to be avoided: 10 s off before
// a new start, no minimum time on. min_off= and min_on= in the thermostat file change them; a
// boiler needs more (the web page's "Boiler" sets 180 s and 60 s: a burner must not cycle quickly).
const MIN_OFF: Duration = Duration::from_secs(10);
const MIN_ON: Duration = Duration::from_secs(0);
const MIN_TIME_MAX_S: u64 = 600;
const SENSOR_MAX_AGE_S: u64 = 600; // no temperature for 10 minutes: heat off
const FROST_DEFAULT_C: f32 = 5.0; // frost protection threshold; heats up to 1 C above it
/// Last raw SHT20 temperature in centi-degrees and when it arrived (unix s).
static LAST_TEMP_C100: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static LAST_TEMP_T: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Relay state the backplate last acknowledged: -1 unknown, 0 off, 1 on.
static RELAY_ACK: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn on_stop(_sig: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---- schedule: setpoint changes by weekday and time, kept on the device ----
// /var/lib/j49/schedule, one change per line: "<days> <HH:MM> <temperature>", for example
//     mon-fri 06:30 20.0
//     mon-fri 22:30 16.0
//     sat,sun 08:00 20.5
// Days: mon tue wed thu fri sat sun (or ma di wo do vr za zo), ranges "mon-fri", lists
// "mon,wed", or "daily". '#' starts a comment. Times are local time (TZ in the thermostat file).
// Like the original: at each change the setpoint is set to that temperature; turning the ring in
// between holds until the next change. bpd only acts on it with "schedule=on" and mode heat.
const SCHEDULE_FILE: &str = "/var/lib/j49/schedule";
const SCHEDULE_STATE: &str = "/var/lib/j49/schedule.state";
const DEFAULT_TZ: &str = "CET-1CEST,M3.5.0,M10.5.0/3"; // Europe/Amsterdam as a POSIX rule
#[derive(Debug, Clone, PartialEq)]
struct Change {
    days: u8,     // bit 0 = Monday .. bit 6 = Sunday
    minute: u16,  // minute of the day
    temp: f32,
}
fn parse_days(s: &str) -> Option<u8> {
    const NAMES: [[&str; 2]; 7] =
        [["mon", "ma"], ["tue", "di"], ["wed", "wo"], ["thu", "do"], ["fri", "vr"], ["sat", "za"], ["sun", "zo"]];
    let idx = |t: &str| NAMES.iter().position(|n| n.contains(&t));
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
fn parse_schedule(text: &str) -> Vec<Change> {
    let mut v = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let (Some(d), Some(t), Some(c)) = (it.next(), it.next(), it.next()) else { continue };
        let Some(days) = parse_days(d) else { continue };
        let Some((h, m)) = t.split_once(':') else { continue };
        let (Ok(h), Ok(m), Ok(temp)) = (h.parse::<u16>(), m.parse::<u16>(), c.parse::<f32>()) else { continue };
        if h > 23 || m > 59 || !(9.0..=32.0).contains(&temp) {
            continue;
        }
        v.push(Change { days, minute: h * 60 + m, temp });
    }
    v
}
/// The change in force at (weekday 0 = Monday, minute of day): (minutes ago it happened, temp).
fn active_change(sched: &[Change], weekday: u32, minute: u32) -> Option<(u32, f32)> {
    let now = weekday * 1440 + minute;
    let mut best: Option<(u32, f32)> = None;
    for c in sched {
        for d in 0..7u32 {
            if c.days & (1 << d) == 0 {
                continue;
            }
            let at = d * 1440 + c.minute as u32;
            let ago = (now + 7 * 1440 - at) % (7 * 1440);
            if best.map(|(a, _)| ago < a).unwrap_or(true) {
                best = Some((ago, c.temp));
            }
        }
    }
    best
}
/// The next change after now: (minutes until it, minute of day it happens, temp).
fn next_change(sched: &[Change], weekday: u32, minute: u32) -> Option<(u32, u32, f32)> {
    let now = weekday * 1440 + minute;
    let mut best: Option<(u32, u32, f32)> = None;
    for c in sched {
        for d in 0..7u32 {
            if c.days & (1 << d) == 0 {
                continue;
            }
            let at = d * 1440 + c.minute as u32;
            let mut until = (at + 7 * 1440 - now) % (7 * 1440);
            if until == 0 {
                until = 7 * 1440;
            }
            if best.map(|(u, _, _)| until < u).unwrap_or(true) {
                best = Some((until, c.minute as u32, c.temp));
            }
        }
    }
    best
}
/// Local (weekday 0 = Monday, minute of day, year) for a unix time, using the TZ rule.
fn local_time(unix: u64) -> (u32, u32, i32) {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (((tm.tm_wday + 6) % 7) as u32, (tm.tm_hour * 60 + tm.tm_min) as u32, tm.tm_year + 1900)
}
/// Replace the setpoint line of the thermostat file, keeping everything else.
fn write_setpoint(sp: f32) {
    let path = "/var/lib/j49/thermostat";
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    let mut done = false;
    for l in old.lines() {
        if l.trim_start().starts_with("setpoint=") {
            out.push_str(&format!("setpoint={sp:.1}\n"));
            done = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("setpoint={sp:.1}\n"));
    }
    let _ = std::fs::create_dir_all("/var/lib/j49");
    if std::fs::write("/var/lib/j49/thermostat.bpd", out).is_ok() {
        let _ = std::fs::rename("/var/lib/j49/thermostat.bpd", path);
    }
}
/// Apply the schedule: when a change happened that was not applied yet, set the setpoint.
/// Returns (minutes of day of the next change, its temperature) for the display.
fn run_schedule(th: &Thermostat) -> Option<(u32, f32)> {
    if !th.schedule || !th.heat {
        return None;
    }
    let sched = parse_schedule(&std::fs::read_to_string(SCHEDULE_FILE).ok()?);
    let now = unix_now();
    let (wd, min, year) = local_time(now);
    if year < 2025 || sched.is_empty() {
        return None; // clock not set, or nothing to do
    }
    if let Some((ago, temp)) = active_change(&sched, wd, min) {
        // when did that change happen, to the minute
        let happened = now / 60 * 60 - ago as u64 * 60;
        let applied: u64 = std::fs::read_to_string(SCHEDULE_STATE).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if happened > applied {
            println!("schedule: setpoint {:.1} -> {temp:.1} (change of {ago} min ago)", th.setpoint);
            write_setpoint(temp);
            let _ = std::fs::write(SCHEDULE_STATE, format!("{happened}\n"));
        }
    }
    next_change(&sched, wd, min).map(|(_, m, t)| (m, t))
}

struct Thermostat {
    heat: bool,
    setpoint: f32,
    offset: f32,
    /// Frost protection: below this room temperature the heating runs even in mode "off".
    /// 0 disables it.
    frost: f32,
    /// Follow /var/lib/j49/schedule.
    schedule: bool,
    /// Nobody home: in mode heat the target is `away_temp` instead of the setpoint. The setpoint
    /// and the schedule go on underneath, so coming back needs nothing but `away=off`.
    away: bool,
    away_temp: f32,
    /// Shortest time off before a new start, and shortest time on, for what is being switched.
    min_off: Duration,
    min_on: Duration,
}
impl Thermostat {
    /// The temperature the control aims for in mode heat.
    fn target(&self) -> f32 {
        if self.away {
            self.away_temp
        } else {
            self.setpoint
        }
    }
}
fn read_thermostat() -> Thermostat {
    let mut t = Thermostat { heat: false, setpoint: 20.0, offset: -2.0, frost: FROST_DEFAULT_C, schedule: false, away: false, away_temp: AWAY_DEFAULT_C, min_off: MIN_OFF, min_on: MIN_ON };
    let mut tz = DEFAULT_TZ.to_string();
    if let Ok(s) = std::fs::read_to_string("/var/lib/j49/thermostat") {
        for l in s.lines() {
            if let Some((k, v)) = l.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "mode" => t.heat = v == "heat",
                    "setpoint" => t.setpoint = v.parse().unwrap_or(t.setpoint).clamp(9.0, 32.0),
                    "offset" => t.offset = v.parse().unwrap_or(t.offset).clamp(-10.0, 10.0),
                    "frost" => t.frost = v.parse().unwrap_or(t.frost).clamp(0.0, 12.0),
                    "schedule" => t.schedule = v == "on",
                    "away" => t.away = v == "on",
                    "away_temp" => t.away_temp = v.parse().unwrap_or(t.away_temp).clamp(9.0, 32.0),
                    "min_off" => t.min_off = v.parse().map(|s: u64| Duration::from_secs(s.min(MIN_TIME_MAX_S))).unwrap_or(t.min_off),
                    "min_on" => t.min_on = v.parse().map(|s: u64| Duration::from_secs(s.min(MIN_TIME_MAX_S))).unwrap_or(t.min_on),
                    "tz" if !v.is_empty() => tz = v.to_string(),
                    _ => {}
                }
            }
        }
    }
    if std::env::var("TZ").ok().as_deref() != Some(tz.as_str()) {
        std::env::set_var("TZ", &tz);
        extern "C" {
            fn tzset();
        }
        unsafe { tzset() };
    }
    t
}
/// Decide whether heat is wanted. `on` is the current demand; hysteresis keeps it from chattering.
fn want_heat(t: &Thermostat, on: bool) -> (bool, &'static str) {
    use std::sync::atomic::Ordering::Relaxed;
    if unix_now() > LAST_TEMP_T.load(Relaxed) + SENSOR_MAX_AGE_S {
        return (false, "no temperature");
    }
    let room = LAST_TEMP_C100.load(Relaxed) as f32 / 100.0 + t.offset;
    if !t.heat {
        // mode off still protects the house against frost: on at the threshold, off 1 C above it
        if t.frost > 0.0 && (room <= t.frost || (on && room < t.frost + 1.0)) {
            return (true, "frost protection");
        }
        return (false, "mode off");
    }
    if room <= t.target() - HYSTERESIS_C {
        (true, "below setpoint")
    } else if room >= t.target() + HYSTERESIS_C {
        (false, "above setpoint")
    } else {
        (on, "within band")
    }
}
/// Seconds until the heating may start again (minimum off time); 0 = not waiting.
static WAIT_S: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
fn publish_heat(t: &Thermostat, demand: bool, reason: &str, next: Option<(u32, f32)>) {
    use std::sync::atomic::Ordering::Relaxed;
    let relay = RELAY_ACK.load(Relaxed);
    let fresh = unix_now() <= LAST_TEMP_T.load(Relaxed) + SENSOR_MAX_AGE_S;
    let room = LAST_TEMP_C100.load(Relaxed) as f32 / 100.0 + t.offset;
    let body = format!(
        "demand={}\nfrost={}\n{}{}mode={}\nsetpoint={:.1}\naway={}\ntarget={:.1}\nwait={}\n{}reason={}\nt={}\n",
        demand as u8,
        (reason == "frost protection") as u8,
        // next schedule change for the display: time as HHMM, temperature in tenths
        match next {
            Some((m, c)) => format!("sched=1\nnext_hhmm={}\nnext_x10={}\n", m / 60 * 100 + m % 60, (c * 10.0).round() as i32),
            None => "sched=0\n".to_string(),
        },
        if relay >= 0 { format!("relay={relay}\n") } else { String::new() },
        if t.heat { "heat" } else { "off" },
        t.setpoint,
        t.away as u8,
        t.target(),
        WAIT_S.load(Relaxed),
        if fresh { format!("room={room:.2}\n") } else { String::new() },
        reason,
        unix_now()
    );
    let _ = std::fs::create_dir_all("/run/bpd");
    if std::fs::write("/run/bpd/heat.tmp", body).is_ok() {
        let _ = std::fs::rename("/run/bpd/heat.tmp", "/run/bpd/heat");
    }
}

fn publish_live(f: &Frame) {
    static LAST_MOTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !f.crc_ok {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (name, body) = match (f.ty, f.payload.len()) {
        (0x07, 2) => {
            LAST_PIR.store(now, std::sync::atomic::Ordering::Relaxed);
            let v = u16::from_le_bytes([f.payload[0], f.payload[1]]);
            if v >= 2 {
                LAST_MOTION.store(now, std::sync::atomic::Ordering::Relaxed);
            }
            let m = LAST_MOTION.load(std::sync::atomic::Ordering::Relaxed);
            ("pir", format!("pir={v}\nmotion_t={m}\nt={now}\n"))
        }
        (0x0a, 4) => {
            let vis = u16::from_le_bytes([f.payload[0], f.payload[1]]);
            let ir = u16::from_le_bytes([f.payload[2], f.payload[3]]);
            ("als", format!("vis={vis}\nir={ir}\nt={now}\n"))
        }
        _ => return,
    };
    let _ = std::fs::create_dir_all("/run/bpd");
    let tmp = format!("/run/bpd/{name}.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, format!("/run/bpd/{name}"));
    }
}

/// Read and decode frames for up to `window`, printing each and publishing SHT20. Returns true if
/// an end-of-buffers (0x002f) frame was seen.
fn drain(tty: &mut std::fs::File, fd: RawFd, dec: &mut Decoder, start: Instant, window: Duration) -> io::Result<bool> {
    let until = Instant::now() + window;
    let mut buf = [0u8; 512];
    let mut saw_end = false;
    while Instant::now() < until {
        let wait = Duration::from_millis(60).min(until.saturating_duration_since(Instant::now()));
        if wait_readable(fd, wait) {
            match tty.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    for f in dec.push(&buf[..n]) {
                        publish_sht20(&f);
                        publish_live(&f);
                        if f.ty == RSP_ACK_SWITCH && f.crc_ok && f.payload.len() == 2 && f.payload[0] == 0 {
                            RELAY_ACK.store((f.payload[1] != 0) as i32, std::sync::atomic::Ordering::Relaxed);
                        }
                        if f.ty == RSP_END_BUFFERS {
                            saw_end = true;
                        }
                        if f.ty == RSP_FET_PRESENCE && f.crc_ok {
                            *LAST_FET.lock().unwrap() = f.payload.clone();
                        }
                        let steady = f.ty == 0x14 || f.ty == 0x07 || f.ty == 0x0a; // once a second: not logged
                        if !steady {
                            println!("{} rx  {}", stamp(start), describe(&f));
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(saw_end)
}

/// The startup handshake. Must be repeated whenever the backplate was reset or attached later.
fn handshake(tty: &mut std::fs::File, fd: RawFd, dec: &mut Decoder, start: Instant) -> io::Result<()> {
    // The sequence below is what stock nlclient 5.9.x sends (captured with ttyproxy on our own
    // unit, 2026-11-24): c0 85 83 ff | 8f<FET echo> 83 90 | 98 99 9d 9b 9c 9f 9e | c0 | a2 a3.
    // Phase 1: power-steal off, 0x85, status, then break + reset -> burst ending in "BRK"
    tty.write_all(&encode(CMD_POWERSTEAL, &[0, 0, 0, 0]))?;
    tty.write_all(&encode(0x0085, &[]))?;
    tty.write_all(&encode(CMD_STATUS, &[]))?;
    tty.flush()?;
    drain(tty, fd, dec, start, Duration::from_millis(150))?;
    unsafe { libc::tcsendbreak(fd, 0) };
    std::thread::sleep(Duration::from_millis(20));
    tty.write_all(&encode(CMD_RESET, &[]))?;
    tty.flush()?;
    drain(tty, fd, dec, start, Duration::from_millis(1200))?;

    // Phase 2: FET presence ACK (echo of the 0x0004 payload) + info gathering
    let fet = {
        let f = LAST_FET.lock().unwrap();
        if f.is_empty() { FET_DEFAULT.to_vec() } else { f.clone() }
    };
    tty.write_all(&encode(CMD_FET_ACK, &fet))?;
    tty.write_all(&encode(CMD_STATUS, &[]))?;
    tty.write_all(&encode(0x0090, &[]))?;
    tty.flush()?;
    drain(tty, fd, dec, start, Duration::from_millis(200))?;
    for c in [0x0098u16, 0x0099, 0x009d, 0x009b, 0x009c, 0x009f, 0x009e] {
        tty.write_all(&encode(c, &[]))?;
        tty.flush()?;
        drain(tty, fd, dec, start, Duration::from_millis(60))?;
    }

    // Phase 3: power-steal mode
    tty.write_all(&encode(CMD_POWERSTEAL, &[0, 0, 0, 0]))?;
    tty.flush()?;
    drain(tty, fd, dec, start, Duration::from_millis(600))?;
    Ok(())
}

/// `sensors` mode: full cuckoo-nest backplate handshake, then poll the buffers so the backplate
/// streams temperature/humidity/PIR/ALS. One open fd throughout.
fn run_sensors(path: &str, _interval: Duration, _max_sleep: u16) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let mut tty = open_tty(path)?;
    let fd = tty.as_raw_fd();
    let start = Instant::now();
    let mut dec = Decoder::default();
    eprintln!("sensors on {path}: reset + handshake, then poll 0xa2 every 30 s (0x83 keepalive 15 s)");

    handshake(&mut tty, fd, &mut dec, start)?;
    let mut last_shake = Instant::now();
    // heating state
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_stop as libc::sighandler_t);
    }
    let mut demand = false;
    let mut demand_since = Instant::now() - Duration::from_secs(MIN_TIME_MAX_S); // allow a start right away
    let mut last_switch_tx = Instant::now() - Duration::from_secs(60);
    let mut next_heat = Instant::now();
    let mut switch_tries = 0u32;
    let mut frost_was_on = false;

    // Phase 4: poll loop. 0xa2 every ~30 s (pull buffers, then ack with 0xa3); 0x83 every ~15 s.
    let mut next_buf = Instant::now();
    let mut next_ka = Instant::now() + Duration::from_secs(15);
    loop {
        // Quiet mode of the backplate, the first half of real sleep (docs/sleep-j49.md). Ask with
        //   echo "<seconds> <wake mask hex>" > /run/bpd/quiet-request
        // From the firmware: 0xa1 puts the backplate in quiet mode and ANY later frame from us
        // ends it. So: 0xa4 (wake mask), then 0xa1 as the last frame, then say nothing and log
        // everything that comes back, raw bytes included, until it speaks or the time is up.
        if let Ok(req) = std::fs::read_to_string("/run/bpd/quiet-request") {
            let _ = std::fs::remove_file("/run/bpd/quiet-request");
            let mut it = req.split_whitespace();
            let secs: u16 = it.next().and_then(|v| v.parse().ok()).unwrap_or(30).clamp(5, 600);
            let mask = it.next().and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(1);
            // With the heat on too: the backplate holds the switch while we are silent, as it does
            // for stock. Its watchdog is the safety net: after the quiet time plus 30 minutes
            // without a word from us it switches the heat off and restarts the head unit
            // (docs/backplate-heatlink-flow.md).
            {
                if demand {
                    println!("{} quiet with the heat on: the backplate holds the switch", stamp(start));
                }
                let limit = Duration::from_secs(secs as u64 + 20);
                println!("{} quiet: wake mask {mask:#06x}, {secs} s; silent until the backplate speaks or {} s pass", stamp(start), limit.as_secs());
                tty.write_all(&encode(0xa4, &mask.to_le_bytes()))?;
                tty.write_all(&encode(TYPE_QUIET, &secs.to_le_bytes()))?;
                tty.flush()?;
                let q0 = Instant::now();
                let junk0 = dec.junk;
                let (mut bytes, mut frames, mut late, mut zeros) = (0usize, 0usize, 0usize, 0usize);
                let mut buf = [0u8; 512];
                let mut ended = "time is up, nothing came";
                // j49-sleep reads this file to know the backplate is quiet now
                let _ = std::fs::write("/run/bpd/quiet", format!("since={}\nsecs={secs}\nmask={mask:#06x}\n", unix_now()));
                let _ = std::fs::remove_file("/run/bpd/quiet-end");
                // frames already on their way arrive in the first 2 s; after that a frame is the wake-up
                while q0.elapsed() < limit && late == 0 {
                    // the head unit is awake again for another reason (WiFi, ring): j49-sleep says so
                    if std::fs::remove_file("/run/bpd/quiet-end").is_ok() {
                        ended = "ended by request";
                        break;
                    }
                    // the backplate's wake-up call is a run of 0x00 bytes, not a frame
                    if zeros >= 8 {
                        ended = "the backplate called (0x00 bytes)";
                        break;
                    }
                    if !wait_readable(fd, Duration::from_millis(200)) {
                        continue;
                    }
                    match tty.read(&mut buf) {
                        Ok(0) => {}
                        Ok(n) => {
                            bytes += n;
                            if q0.elapsed() > Duration::from_secs(2) {
                                zeros += buf[..n].iter().filter(|b| **b == 0).count();
                            }
                            for f in dec.push(&buf[..n]) {
                                frames += 1;
                                println!("{} quiet rx  {}", stamp(start), describe(&f));
                                publish_sht20(&f);
                                publish_live(&f);
                                if q0.elapsed() > Duration::from_secs(2) {
                                    late += 1;
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) => return Err(e),
                    }
                }
                let _ = std::fs::remove_file("/run/bpd/quiet");
                println!(
                    "{} quiet over after {:.1} s: {bytes} bytes, {frames} frames, {} junk bytes; {}",
                    stamp(start),
                    q0.elapsed().as_secs_f32(),
                    dec.junk - junk0,
                    if late > 0 { "the backplate spoke (a frame)" } else { ended }
                );
                // talk again (this ends the quiet mode) and give the stream 20 s to come back
                LAST_PIR.store(unix_now(), std::sync::atomic::Ordering::Relaxed);
                next_buf = Instant::now();
                next_ka = Instant::now();
            }
        }
        if Instant::now() >= next_buf {
            tty.write_all(&encode(CMD_REQ_BUFFERS, &[]))?;
            tty.flush()?;
            let got = drain(&mut tty, fd, &mut dec, start, Duration::from_millis(1500))?;
            if got {
                tty.write_all(&encode(CMD_ACK_BUFFERS, &[]))?;
                tty.flush()?;
            }
            next_buf += Duration::from_secs(30);
        }
        if Instant::now() >= next_ka {
            // 0xa1 reloads the head-unit watchdog (no 0xa1 = cold reset after ~29 min)
            tty.write_all(&encode(TYPE_QUIET, &600u16.to_le_bytes()))?;
            tty.write_all(&encode(CMD_STATUS, &[]))?;
            tty.flush()?;
            next_ka += Duration::from_secs(15);
        }
        drain(&mut tty, fd, &mut dec, start, Duration::from_millis(500))?;
        // leaving: never leave the boiler on behind us
        if STOP.load(std::sync::atomic::Ordering::Relaxed) {
            println!("{} stopping: heat off", stamp(start));
            tty.write_all(&encode(CMD_SWITCH, &[0, 0]))?;
            tty.flush()?;
            drain(&mut tty, fd, &mut dec, start, Duration::from_millis(400))?;
            let _ = std::fs::remove_file("/run/bpd/heat");
            return Ok(());
        }
        // thermostat, once a second
        if Instant::now() >= next_heat {
            next_heat = Instant::now() + Duration::from_secs(1);
            let mut th = read_thermostat();
            let next = run_schedule(&th);
            if next.is_some() {
                th = read_thermostat(); // the schedule may just have moved the setpoint
            }
            let (mut want, mut reason) = want_heat(&th, demand);
            // boiler protection: minimum off and on times. Switching OFF because the mode is off
            // or the sensor is gone is never delayed.
            WAIT_S.store(0, std::sync::atomic::Ordering::Relaxed);
            if want && !demand && demand_since.elapsed() < th.min_off {
                want = false;
                reason = "waiting (minimum off time)";
                // for the display: heat is wanted and starts in this many seconds
                WAIT_S.store((th.min_off - demand_since.elapsed()).as_secs() as u32 + 1, std::sync::atomic::Ordering::Relaxed);
            } else if !want && demand && (reason == "above setpoint" || (reason == "mode off" && !th.heat && frost_was_on))
                && demand_since.elapsed() < th.min_on
            {
                want = true;
                reason = "holding (minimum on time)";
            }
            frost_was_on = want && reason == "frost protection";
            if want != demand {
                println!("{} heat demand {} -> {} ({reason})", stamp(start), demand as u8, want as u8);
                demand = want;
                demand_since = Instant::now();
                last_switch_tx = Instant::now() - Duration::from_secs(60);
                switch_tries = 0;
            }
            // tell the backplate, and repeat every 5 s until it acknowledges that state
            let acked = RELAY_ACK.load(std::sync::atomic::Ordering::Relaxed);
            if acked != demand as i32 && last_switch_tx.elapsed() >= Duration::from_secs(5) {
                // log the first attempt and then once a minute: without a backplate this repeats forever
                if switch_tries % 12 == 0 {
                    println!("{} tx  switch circuit 0 -> {} (acknowledged so far: {acked}, try {})", stamp(start), demand as u8, switch_tries + 1);
                }
                switch_tries += 1;
                tty.write_all(&encode(CMD_SWITCH, &[0, demand as u8]))?;
                tty.flush()?;
                last_switch_tx = Instant::now();
            }
            publish_heat(&th, demand, reason, next);
        }
        // No PIR for 20 s: the backplate is not streaming (attached after we started, or it
        // was reset). Do the handshake again, at most once every 30 s.
        if unix_now() > LAST_PIR.load(std::sync::atomic::Ordering::Relaxed) + 20
            && last_shake.elapsed() >= Duration::from_secs(30)
        {
            println!("{} no sensor stream, handshake again", stamp(start));
            handshake(&mut tty, fd, &mut dec, start)?;
            last_shake = Instant::now();
            // the backplate was reset: its switch is off again until we ask
            RELAY_ACK.store(-1, std::sync::atomic::Ordering::Relaxed);
            next_buf = Instant::now();
            next_ka = Instant::now() + Duration::from_secs(15);
        }
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn positional(args: &[String]) -> Vec<String> {
    // positional = everything that is not an --option or its value
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  bpd keepalive [tty] [--interval S] [--max-sleep S] [--quiet]\n  bpd sniff [tty] [--seconds S]\n  bpd send [tty] <type-hex> [payload-hex]\n  bpd encode <type-hex> [payload-hex]   (print the frame, no tty)\n(default tty {DEFAULT_TTY})"
    );
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_default();
    let rest = &args[1.min(args.len())..];
    let pos = positional(rest);
    let tty_path = |n: usize| pos.get(n).cloned().unwrap_or_else(|| DEFAULT_TTY.to_string());
    let open = |p: &str| {
        open_tty(p).unwrap_or_else(|e| {
            eprintln!("{p}: {e}");
            exit(1)
        })
    };
    let res = match cmd.as_str() {
        "keepalive" => {
            let interval = arg_value(rest, "--interval").and_then(|v| v.parse().ok()).unwrap_or(15u64);
            let max_sleep: u16 = arg_value(rest, "--max-sleep").and_then(|v| v.parse().ok()).unwrap_or(600);
            let quiet = rest.iter().any(|a| a == "--quiet");
            let frame = encode(TYPE_QUIET, &max_sleep.to_le_bytes());
            let p = tty_path(0);
            eprintln!(
                "bpd keepalive on {p}: type 0x{TYPE_QUIET:02x} max_sleep={max_sleep}s every {interval}s ({})",
                hex(&frame)
            );
            pump(&mut open(&p), Some((&frame, Duration::from_secs(interval))), None, quiet)
        }
        "sniff" => {
            let secs = arg_value(rest, "--seconds").and_then(|v| v.parse().ok());
            pump(&mut open(&tty_path(0)), None, secs.map(Duration::from_secs), false)
        }
        "sensors" => {
            let interval = arg_value(rest, "--interval").and_then(|v| v.parse().ok()).unwrap_or(5u64);
            let max_sleep: u16 = arg_value(rest, "--max-sleep").and_then(|v| v.parse().ok()).unwrap_or(600);
            run_sensors(&tty_path(0), Duration::from_secs(interval), max_sleep)
        }
        "send" => {
            // tty is optional: `bpd send a1 5802` or `bpd send /dev/ttyS2 a1 5802`
            let (p, ty_s, pl_s) = match pos.len() {
                1 => (DEFAULT_TTY.to_string(), pos[0].clone(), String::new()),
                2 if !pos[0].starts_with('/') => (DEFAULT_TTY.to_string(), pos[0].clone(), pos[1].clone()),
                2 => (pos[0].clone(), pos[1].clone(), String::new()),
                3 => (pos[0].clone(), pos[1].clone(), pos[2].clone()),
                _ => usage(),
            };
            let ty = u16::from_str_radix(&ty_s, 16).unwrap_or_else(|_| usage());
            let pl = parse_hex(&pl_s).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            let frame = encode(ty, &pl);
            let mut tty = open(&p);
            tty.write_all(&frame).and_then(|_| pump(&mut tty, None, Some(Duration::from_secs(2)), false))
        }
        "encode" => {
            let ty = pos.first().and_then(|s| u16::from_str_radix(s, 16).ok()).unwrap_or_else(|| usage());
            let pl = parse_hex(pos.get(1).map(String::as_str).unwrap_or("")).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            println!("{}", hex(&encode(ty, &pl)));
            Ok(())
        }
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_parsing_and_lookup() {
        let s = parse_schedule(
            "# werkdagen\nmon-fri 06:30 20.0\nma-vr 22:30 16.0\nsat,sun 08:00 20.5 # weekend\nzo 23:00 15.5\n\nbad line\nmon 25:00 20\nmon 10:00 50\n",
        );
        assert_eq!(s.len(), 4);
        assert_eq!(s[0], Change { days: 0b0011111, minute: 390, temp: 20.0 });
        assert_eq!(s[2].days, 0b1100000);
        assert_eq!(parse_days("fri-mon"), Some(0b1110001)); // a range may wrap around the week
        assert_eq!(parse_days("daily"), Some(0x7f));
        assert_eq!(parse_days("xyz"), None);
        // Wednesday 12:00: the 06:30 change is in force, 330 minutes old; next is 22:30 -> 16.0
        assert_eq!(active_change(&s, 2, 720), Some((330, 20.0)));
        assert_eq!(next_change(&s, 2, 720), Some((630, 1350, 16.0)));
        // Monday 05:00: still Sunday 23:00 (15.5), across the week boundary
        assert_eq!(active_change(&s, 0, 300), Some((360, 15.5)));
        // Saturday 07:00: Friday 22:30 is in force
        assert_eq!(active_change(&s, 5, 420), Some((510, 16.0)));
        // exactly at a change it is that change, zero minutes old
        assert_eq!(active_change(&s, 0, 390), Some((0, 20.0)));
        assert_eq!(active_change(&[], 0, 0), None);
    }

    #[test]
    fn local_time_follows_the_tz_rule() {
        std::env::set_var("TZ", DEFAULT_TZ);
        extern "C" {
            fn tzset();
        }
        unsafe { tzset() };
        // 2026-01-15 12:00 UTC is a Thursday, 13:00 in Amsterdam (winter)
        assert_eq!(local_time(1_768_478_400), (3, 13 * 60, 2026));
        // 2026-07-15 12:00 UTC is a Wednesday, 14:00 in Amsterdam (summer time)
        assert_eq!(local_time(1_784_116_800), (2, 14 * 60, 2026));
    }

    #[test]
    fn thermostat_decisions() {
        use std::sync::atomic::Ordering::Relaxed;
        let set = |c: f32| {
            LAST_TEMP_C100.store((c * 100.0) as i32, Relaxed);
            LAST_TEMP_T.store(unix_now(), Relaxed);
        };
        let th = |heat, frost| Thermostat { heat, setpoint: 20.0, offset: 0.0, frost, schedule: false, away: false, away_temp: 15.0, min_off: MIN_OFF, min_on: MIN_ON };
        // mode heat: hysteresis around the setpoint
        set(19.6);
        assert_eq!(want_heat(&th(true, 5.0), false).0, true);
        set(20.0);
        assert_eq!(want_heat(&th(true, 5.0), false).0, false); // inside the band: keep off
        assert_eq!(want_heat(&th(true, 5.0), true).0, true); // inside the band: keep on
        set(20.4);
        assert_eq!(want_heat(&th(true, 5.0), true).0, false);
        // mode off: only frost protection, on at 5.0, held until 6.0
        set(12.0);
        assert_eq!(want_heat(&th(false, 5.0), false), (false, "mode off"));
        set(5.0);
        assert_eq!(want_heat(&th(false, 5.0), false), (true, "frost protection"));
        set(5.8);
        assert_eq!(want_heat(&th(false, 5.0), true).0, true);
        assert_eq!(want_heat(&th(false, 5.0), false).0, false);
        set(6.0);
        assert_eq!(want_heat(&th(false, 5.0), true).0, false);
        // frost protection disabled
        set(2.0);
        assert_eq!(want_heat(&th(false, 0.0), false).0, false);
        // the offset is applied before comparing
        set(22.0);
        let mut t = th(true, 5.0);
        t.offset = -3.0;
        assert_eq!(want_heat(&t, false).0, true); // 19.0 after correction
        // away: the target is away_temp (15.0), the setpoint (20.0) waits
        let mut t = th(true, 5.0);
        t.away = true;
        set(17.0);
        assert_eq!(want_heat(&t, false).0, false);
        assert_eq!(want_heat(&t, true).0, false);
        set(14.6);
        assert_eq!(want_heat(&t, false).0, true);
        t.away = false;
        set(17.0);
        assert_eq!(want_heat(&t, false).0, true);
        // no temperature for too long: never heat
        LAST_TEMP_T.store(unix_now() - SENSOR_MAX_AGE_S - 5, Relaxed);
        assert_eq!(want_heat(&th(true, 5.0), true), (false, "no temperature"));
        assert_eq!(want_heat(&th(false, 5.0), true), (false, "no temperature"));
    }

    #[test]
    fn crc16_xmodem_check_value() {
        // CRC-16/XMODEM check value for "123456789" is 0x31C3.
        assert_eq!(crc16(b"123456789"), 0x31c3);
    }

    #[test]
    fn encode_quiet_message_matches_bptool() {
        // bptool beat ... a1 5802 -> type 0xa1, payload 58 02 (max_sleep 600).
        let f = encode(0xa1, &[0x58, 0x02]);
        assert_eq!(&f[..9], &[0xd5, 0xaa, 0x96, 0xa1, 0x00, 0x02, 0x00, 0x58, 0x02]);
        assert_eq!(f.len(), 11);
        let c = crc16(&f[3..9]);
        assert_eq!(&f[9..], &c.to_le_bytes());
    }

    #[test]
    fn decoder_handles_pad_bytes_splits_and_junk() {
        // SHT20 frame: temperature in 1/100 C, humidity in 1/10 % (checked against a reference
        // sensor on 2026-11-24; the earlier sample here assumed 1/100 % and no longer matched)
        let a = encode(0x02, &[0x10, 0x09, 0xf0, 0x01]); // 23.20 C, 49.6 %RH
        let b = encode(0x07, &[0, 0]);
        let mut stream = vec![0x00, 0xd5]; // junk + backplate pad byte
        stream.extend_from_slice(&a);
        stream.push(0xd5);
        stream.extend_from_slice(&b);
        let mut d = Decoder::default();
        let (s1, s2) = stream.split_at(7); // split inside the first frame
        assert!(d.push(s1).is_empty());
        let frames = d.push(s2);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], Frame { ty: 2, payload: vec![0x10, 0x09, 0xf0, 0x01], crc_ok: true });
        assert_eq!(frames[1].ty, 7);
        assert!(frames[1].crc_ok);
        assert_eq!(d.junk, 1);
        assert!(d.buf.is_empty());
        assert!(describe(&frames[0]).contains("23.20 C 49.60 %RH"));
    }

    #[test]
    fn decoder_flags_bad_crc() {
        let mut f = encode(0x0a, &[1, 2, 3, 4]);
        let n = f.len();
        f[n - 1] ^= 0xff;
        let frames = Decoder::default().push(&f);
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].crc_ok);
    }
}
