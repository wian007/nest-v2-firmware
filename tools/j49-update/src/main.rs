// j49-update: a new root file system over the network, with the old one as the way back.
//
// The flash memory has two slots for a system, root0 and root1. An update never touches the
// running one:
//
//   1. the image is streamed into the other slot (never held in RAM: there are 64 MB), its
//      SHA-256 is compared with the one that was announced, and the slot is read back;
//   2. what belongs to this device (WiFi networks, settings, SSH keys) is copied to the scratch
//      partition, from where every system takes it at its first start;
//   3. a record in the partition env1 asks for a TRIAL of the new slot, and the device restarts;
//   4. nandio, the first program the kernel starts, counts the trial down and starts the new
//      system. When it is up for two minutes, "j49-update confirm" (a systemd unit) makes it
//      the active one. When it does not come up, the watchdog restarts the board, the trial is
//      used up, and nandio starts the old system again.
//
// Nothing happens by itself. /var/lib/j49/update holds:
//   check=off|on     ask the address below for a newer version (off unless switched on)
//   url=<https://.../manifest.txt>
// "check" only looks and writes /run/j49-update.state; installing is always a separate order
// (the web page, Home Assistant, or "j49-update install" by hand).
//
// The manifest is text, key=value:
//   version=2026.12.01      shown to the user; compared as text with /etc/j49-release
//   rootfs=<file name next to the manifest, or a full address>
//   size=<bytes>   sha256=<hex>
//   kernel=<text>           optional: must occur in `uname -r`; else the update needs USB
//
// The kernel is not part of this: it sits in boot0, has no second copy, and is updated over USB.
//
//   j49-update status
//   j49-update check                     fetch the manifest, write /run/j49-update.state
//   j49-update install [--no-reboot]     install what the manifest names
//   j49-update install <file|address> <sha256> [--no-reboot]
//   j49-update confirm                   make the running slot the active one
//   j49-update back                      ask for a trial of the other slot as it is
//
// No crates. It uses curl (https), flash_erase, nandwrite and nanddump from the image.
// J49_ROOT=<dir> prefixes the paths of files, for tests on a PC.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

const CONF: &str = "/var/lib/j49/update";
const STATE: &str = "/run/j49-update.state";
const PAGE: usize = 2048;
const TRIES: u32 = 1;
/// What a new system takes over from the old one (files and whole directories).
const KEEP: [&str; 5] = ["/etc/wpa_supplicant/wpa_supplicant-wlan0.conf", "/var/lib/j49", "/var/lib/j49-screen", "/etc/dropbear", "/root/.ssh"];

fn root() -> String {
    std::env::var("J49_ROOT").unwrap_or_default()
}
fn p(path: &str) -> String {
    format!("{}{}", root(), path)
}
fn kv(text: &str) -> HashMap<String, String> {
    text.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}
fn kv_file(path: &str) -> HashMap<String, String> {
    kv(&std::fs::read_to_string(p(path)).unwrap_or_default())
}

// ---- checksums ----
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}
#[derive(Clone)]
struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}
impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    fn new() -> Self {
        Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], buf: Vec::new(), len: 0 }
    }
    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = self.h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(Self::K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            self.h[i] = self.h[i].wrapping_add(v[i]);
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        let whole = self.buf.len() / 64 * 64;
        let rest = self.buf.split_off(whole);
        let full = std::mem::replace(&mut self.buf, rest);
        for c in full.chunks(64) {
            self.block(c);
        }
    }
    fn hex(mut self) -> String {
        let bits = self.len * 8;
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for c in tail.chunks(64) {
            self.block(c);
        }
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
}

// ---- the slots ----
/// (index of the mtd device, size) of a partition by name.
fn mtd(name: &str) -> Option<(u32, u64)> {
    let text = std::fs::read_to_string(p("/proc/mtd")).ok()?;
    text.lines().find_map(|l| {
        let (dev, rest) = l.split_once(':')?;
        if rest.trim_end().ends_with(&format!("\"{name}\"")) {
            Some((dev.strip_prefix("mtd")?.parse().ok()?, u64::from_str_radix(rest.split_whitespace().next()?, 16).ok()?))
        } else {
            None
        }
    })
}
/// The slot (0 or 1) the running system was started from.
fn running_slot() -> Option<usize> {
    let mounts = std::fs::read_to_string(p("/proc/mounts")).ok()?;
    let dev = mounts.lines().filter_map(|l| { let mut f = l.split_whitespace(); let d = f.next()?; (f.next()? == "/").then(|| d.to_string()) }).filter(|d| d.starts_with("/dev/mtdblock")).last()?;
    let n: u32 = dev.strip_prefix("/dev/mtdblock")?.parse().ok()?;
    [mtd("root0")?.0, mtd("root1")?.0].iter().position(|&m| m == n)
}
#[derive(Debug, PartialEq, Clone, Copy)]
struct Rec {
    active: usize,
    trial: Option<usize>,
    left: u32,
}
fn rec_parse(page: &[u8]) -> Option<Rec> {
    let text = String::from_utf8_lossy(&page[..page.len().min(127)]).to_string();
    let line = text.lines().next()?;
    let (body, sum) = line.split_once(" sum=")?;
    if !body.starts_with("J49SLOT1 ") || u32::from_str_radix(sum.trim(), 16).ok()? != crc32(body.as_bytes()) {
        return None;
    }
    let m: HashMap<&str, &str> = body.split_whitespace().skip(1).filter_map(|w| w.split_once('=')).collect();
    let active = m.get("active")?.parse::<usize>().ok().filter(|a| *a < 2)?;
    let trial = match *m.get("try")? { "-" => None, t => Some(t.parse::<usize>().ok().filter(|a| *a < 2)?) };
    Some(Rec { active, trial, left: m.get("left")?.parse().ok()? })
}
fn rec_text(r: &Rec) -> String {
    let body = format!("J49SLOT1 active={} try={} left={}", r.active, r.trial.map(|t| t.to_string()).unwrap_or("-".into()), r.left);
    format!("{body} sum={:08x}\n", crc32(body.as_bytes()))
}
fn rec_read() -> Option<Rec> {
    let (n, _) = mtd("env1")?;
    let mut page = [0u8; 128];
    let got = std::fs::File::open(p(&format!("/dev/mtd{n}"))).ok()?.read(&mut page).ok()?;
    rec_parse(&page[..got])
}
fn rec_write(r: &Rec) -> Result<(), String> {
    let (n, _) = mtd("env1").ok_or("no partition env1")?;
    let dev = p(&format!("/dev/mtd{n}"));
    let mut page = vec![0xffu8; PAGE];
    let t = rec_text(r);
    page[..t.len()].copy_from_slice(t.as_bytes());
    if root().is_empty() && !Command::new("flash_erase").args(["-q", &dev, "0", "1"]).status().map(|s| s.success()).unwrap_or(false) {
        return Err("could not erase the slot record".into());
    }
    std::fs::OpenOptions::new().write(true).open(&dev).and_then(|mut f| f.write_all(&page)).map_err(|e| format!("could not write the slot record: {e}"))?;
    if rec_read() != Some(*r) {
        return Err("the slot record does not read back".into());
    }
    Ok(())
}

// ---- installing ----
fn die(msg: &str) -> ! {
    eprintln!("j49-update: {msg}");
    let _ = std::fs::write(p(STATE), format!("{}error={msg}\n", std::fs::read_to_string(p(STATE)).unwrap_or_default().lines().filter(|l| !l.starts_with("error=") && !l.starts_with("busy=")).map(|l| format!("{l}\n")).collect::<String>()));
    std::process::exit(1);
}
fn fetch(url: &str) -> Command {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--proto", "=https", "--retry", "3", "--connect-timeout", "20", url]);
    c
}
fn is_url(s: &str) -> bool {
    s.starts_with("https://")
}
fn manifest() -> Result<(HashMap<String, String>, String), String> {
    let url = kv_file(CONF).get("url").cloned().filter(|u| is_url(u)).ok_or("no address set (url= in /var/lib/j49/update; https only)")?;
    let out = fetch(&url).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err("the manifest could not be fetched".into());
    }
    Ok((kv(&String::from_utf8_lossy(&out.stdout)), url))
}
fn installed() -> String {
    let r = kv_file("/etc/j49-release");
    r.get("version").or(r.get("commit_date")).cloned().unwrap_or_default()
}
/// Write the image that `src` gives into the mtd device, hash it on the way. Returns
/// (sha256 of the image, sha256 of the image padded to whole pages, padded length).
fn stream(mut src: impl Read, dev: &str) -> Result<(String, String, u64), String> {
    let mut w = Command::new("nandwrite").args(["-q", "-p", dev, "-"]).stdin(Stdio::piped()).spawn().map_err(|e| format!("nandwrite: {e}"))?;
    let mut sink = w.stdin.take().unwrap();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).map_err(|e| format!("reading the image: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
        sink.write_all(&buf[..n]).map_err(|e| format!("writing the flash memory: {e}"))?;
    }
    drop(sink);
    if !w.wait().map(|s| s.success()).unwrap_or(false) {
        return Err("nandwrite failed".into());
    }
    let mut padded = h.clone();
    let pad = (PAGE as u64 - total % PAGE as u64) % PAGE as u64;
    padded.update(&vec![0xffu8; pad as usize]);
    Ok((h.hex(), padded.hex(), total + pad))
}
fn keep_files() -> Result<(), String> {
    if !root().is_empty() {
        return Ok(()); // a test on a PC
    }
    let (n, _) = mtd("scratch").ok_or("no partition scratch")?;
    let mnt = "/run/j49-update-scratch";
    let _ = std::fs::create_dir_all(mnt);
    if !Command::new("mount").args(["-t", "jffs2", &format!("/dev/mtdblock{n}"), mnt]).status().map(|s| s.success()).unwrap_or(false) {
        return Err("the scratch partition does not mount".into());
    }
    let dest = format!("{mnt}/j49-persist/rootfs");
    let mut ok = true;
    for k in KEEP {
        if !std::path::Path::new(k).exists() {
            continue;
        }
        let parent = format!("{dest}{}", std::path::Path::new(k).parent().map(|x| x.display().to_string()).unwrap_or_default());
        let _ = std::fs::create_dir_all(&parent);
        // the whole thing anew: a file that was removed here must not come back from an older copy
        let _ = Command::new("rm").args(["-rf", &format!("{dest}{k}")]).status();
        ok &= Command::new("cp").args(["-a", k, &parent]).status().map(|s| s.success()).unwrap_or(false);
    }
    let _ = Command::new("sync").status();
    let _ = Command::new("umount").arg(mnt).status();
    if ok { Ok(()) } else { Err("not every file could be copied to scratch".into()) }
}
fn install(source: &str, want_sha: &str, want_size: Option<u64>, reboot: bool) {
    let me = running_slot().unwrap_or_else(|| die("cannot tell which slot is running"));
    let other = 1 - me;
    let name = ["root0", "root1"][other];
    let (n, size) = mtd(name).unwrap_or_else(|| die("the other slot is not in the partition table"));
    if want_size.map(|s| s > size).unwrap_or(false) {
        die("the image is larger than the slot");
    }
    let dev = p(&format!("/dev/mtd{n}"));
    if std::fs::read_to_string(p(&format!("/sys/class/mtd/mtd{n}/flags"))).map(|f| f.trim() == "0x0").unwrap_or(false) {
        die(&format!("{name} is read-only under this kernel: this update needs USB"));
    }
    // an open trial of the other slot would start a half-written system: withdraw it first
    if let Some(r) = rec_read() {
        if r.trial.is_some() {
            rec_write(&Rec { active: me, trial: None, left: 0 }).unwrap_or_else(|e| die(&e));
        }
    }
    let _ = std::fs::write(p(STATE), format!("busy=writing {name}\n"));
    eprintln!("j49-update: running from root{me}; writing {name} ({dev})");
    if !Command::new("flash_erase").args(["-q", &dev, "0", "0"]).status().map(|s| s.success()).unwrap_or(false) {
        die("could not erase the other slot");
    }
    let res = if is_url(source) {
        let mut c = fetch(source).stdout(Stdio::piped()).spawn().unwrap_or_else(|e| die(&format!("curl: {e}")));
        let r = stream(c.stdout.take().unwrap(), &dev);
        if !c.wait().map(|s| s.success()).unwrap_or(false) { Err("the download broke off".into()) } else { r }
    } else {
        std::fs::File::open(source).map_err(|e| format!("{source}: {e}")).and_then(|f| stream(f, &dev))
    };
    let wipe = |why: &str| -> ! {
        let _ = Command::new("flash_erase").args(["-q", &dev, "0", "0"]).status();
        die(why)
    };
    let (sha, sha_padded, len) = res.unwrap_or_else(|e| wipe(&e));
    if !sha.eq_ignore_ascii_case(want_sha) {
        wipe("the image is not the one that was announced (SHA-256 differs); the slot was erased again");
    }
    // what is in the flash memory now
    let mut d = Command::new("nanddump").args(["-q", "--bb=skipbad", "-l", &len.to_string(), &dev]).stdout(Stdio::piped()).spawn().unwrap_or_else(|e| die(&format!("nanddump: {e}")));
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut out = d.stdout.take().unwrap();
    while let Ok(k) = out.read(&mut buf) {
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
    }
    let _ = d.wait();
    if h.hex() != sha_padded {
        wipe("what was read back from the flash memory differs from the image; the slot was erased again");
    }
    eprintln!("j49-update: {name} written and read back ({len} bytes)");
    if let Err(e) = keep_files() {
        eprintln!("j49-update: WARNING: {e}; the new system may start without the settings of this one");
    }
    rec_write(&Rec { active: me, trial: Some(other), left: TRIES }).unwrap_or_else(|e| die(&e));
    let _ = std::fs::write(p(STATE), format!("busy=restart into {name}\n"));
    eprintln!("j49-update: trial of {name} asked for{}", if reboot { "; restarting" } else { "; restart to start it" });
    if reboot && root().is_empty() {
        let _ = Command::new("systemctl").arg("reboot").status();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let reboot = !args.iter().any(|a| a == "--no-reboot");
    let words: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|a| !a.starts_with("--")).collect();
    match words.as_slice() {
        ["status"] | [] => {
            let c = kv_file(CONF);
            println!("installed={}", installed());
            println!("slot={}", running_slot().map(|s| format!("root{s}")).unwrap_or("unknown".into()));
            match rec_read() {
                Some(r) => println!("record=active root{}{}", r.active, r.trial.map(|t| format!(", trial of root{t} ({} left)", r.left)).unwrap_or_default()),
                None => println!("record=none (the kernel command line decides)"),
            }
            println!("check={}", c.get("check").map(|s| s.as_str()).unwrap_or("off"));
            println!("url={}", c.get("url").map(|s| s.as_str()).unwrap_or(""));
            print!("{}", std::fs::read_to_string(p(STATE)).unwrap_or_default());
        }
        ["check"] => {
            if kv_file(CONF).get("check").map(|s| s != "on").unwrap_or(true) && !args.iter().any(|a| a == "--now") {
                return; // not asked for
            }
            match manifest() {
                Ok((m, _)) => {
                    let latest = m.get("version").cloned().unwrap_or_default();
                    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let _ = std::fs::write(p(STATE), format!("installed={}\nlatest={latest}\nchecked={t}\n", installed()));
                    println!("installed={} latest={latest}", installed());
                }
                Err(e) => die(&e),
            }
        }
        ["install"] => {
            let (m, url) = manifest().unwrap_or_else(|e| die(&e));
            if let Some(k) = m.get("kernel").filter(|k| !k.is_empty()) {
                let have = String::from_utf8_lossy(&Command::new("uname").arg("-r").output().map(|o| o.stdout).unwrap_or_default()).to_string();
                if !have.contains(k.as_str()) {
                    die(&format!("this version needs kernel {k}; this device has {}: update the kernel over USB first", have.trim()));
                }
            }
            let file = m.get("rootfs").cloned().unwrap_or_else(|| die("the manifest names no rootfs"));
            let src = if is_url(&file) { file } else { format!("{}/{file}", url.rsplit_once('/').map(|x| x.0).unwrap_or("")) };
            let sha = m.get("sha256").cloned().unwrap_or_else(|| die("the manifest has no sha256"));
            install(&src, &sha, m.get("size").and_then(|s| s.parse().ok()), reboot);
        }
        ["install", src, sha] => install(src, sha, std::fs::metadata(src).ok().map(|m| m.len()), reboot),
        ["confirm"] => {
            let Some(me) = running_slot() else { die("cannot tell which slot is running") };
            match rec_read() {
                Some(r) if r.active == me && r.trial.is_none() => {}
                // no record and running from root0 is the state of a fresh install: leave it
                None if me == 0 => {}
                _ => {
                    rec_write(&Rec { active: me, trial: None, left: 0 }).unwrap_or_else(|e| die(&e));
                    eprintln!("j49-update: root{me} confirmed as the active system");
                }
            }
            let _ = std::fs::write(p(STATE), format!("installed={}\n", installed()));
        }
        ["back"] => {
            let Some(me) = running_slot() else { die("cannot tell which slot is running") };
            rec_write(&Rec { active: me, trial: Some(1 - me), left: TRIES }).unwrap_or_else(|e| die(&e));
            eprintln!("j49-update: trial of root{} asked for; restart to start it", 1 - me);
        }
        _ => {
            eprintln!("usage: j49-update status | check [--now] | install [<file|https address> <sha256>] [--no-reboot] | confirm | back");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_known() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(h.hex(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(h.hex(), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
        assert_eq!(Sha256::new().hex(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
    #[test]
    fn record() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        for r in [Rec { active: 0, trial: Some(1), left: 1 }, Rec { active: 1, trial: None, left: 0 }] {
            assert_eq!(rec_parse(rec_text(&r).as_bytes()), Some(r));
        }
        let mut t = rec_text(&Rec { active: 0, trial: Some(1), left: 1 }).into_bytes();
        t[20] ^= 1;
        assert_eq!(rec_parse(&t), None);
        assert_eq!(rec_parse(&[0xff; 128]), None);
        assert_eq!(rec_parse(b"J49SLOT1 active=2 try=- left=0 sum=00000000\n"), None);
    }
}
