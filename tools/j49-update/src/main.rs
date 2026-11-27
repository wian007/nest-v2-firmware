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
//   check=off|on        look for a newer version once a day (off unless switched on)
//   prerelease=off|on   count pre-releases as versions to offer
//   repo=<https address of a repository> [token]     any number of these lines
//   standard=on|off     off: leave the standard repositories out (only with repositories of your own)
// Versions are the RELEASES of a git repository (GitHub, or Gitea/Forgejo: the same API shape).
// The firmware carries a list of standard repositories (/usr/share/j49/update-repos); the
// settings can name repositories of the owner next to them. Every version is shown with the
// repository it comes from, and what was installed remembers it. A release that can be installed has two files attached: the root
// file system (*.rootfs.jffs2) and SHA256SUMS with its checksum.
// "check" only looks and writes /run/j49-update.state; installing is always a separate order
// (the web page, or "j49-update install" by hand).
//
// The kernel is not part of this: it sits in boot0, has no second copy, and is updated over USB.
//
//   j49-update status                    what runs (version, commit, date), the slot, the repositories
//   j49-update list                      the versions the repositories offer (/run/j49-update.versions)
//   j49-update check [--now]             the same, when check=on or with --now; notes the newest
//   j49-update install [<version>] [--from=<repository>] [--no-reboot]    the newest, or the one named
//   j49-update install <file|address> <sha256> [--no-reboot]
//   j49-update repo add <address> [token] | repo remove <address> | repo reset
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
const VERSIONS: &str = "/run/j49-update.versions";
const REPOS_STD: &str = "/usr/share/j49/update-repos";
const INSTALLED: &str = "/var/lib/j49/update.installed";
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
fn fetch(url: &str, token: &str) -> Command {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--proto", "=https", "--retry", "3", "--connect-timeout", "20", "-A", "j49-update"]);
    if !token.is_empty() {
        c.args(["-H", &format!("Authorization: token {token}")]);
    }
    c.arg(url);
    c
}
/// A file attached to a release. A private repository on GitHub gives its files only through
/// the API address, with a token and a header that asks for the file itself; everywhere else
/// (public GitHub, Gitea, Forgejo) the download address works, with the token when there is one.
fn fetch_asset(a: &Asset, repo: &str, token: &str) -> Command {
    if !token.is_empty() && repo.starts_with("https://github.com/") && is_url(&a.api) {
        let mut c = fetch(&a.api, token);
        c.args(["-H", "Accept: application/octet-stream"]);
        return c;
    }
    fetch(&a.url, token)
}
#[derive(Debug, Clone, PartialEq, Default)]
struct Asset {
    url: String, // the download address
    api: String, // the address of the file in the API
}
fn is_url(s: &str) -> bool {
    s.starts_with("https://")
}

// ---- repositories and their releases ----
/// (address, token, the owner's own) of the repositories in use: the standard ones that came
/// with the firmware (unless standard=off) and the owner's own, side by side.
fn repos() -> Vec<(String, String, bool)> {
    let parse = |text: &str, key: bool| -> Vec<(String, String, bool)> {
        text.lines()
            .map(|l| l.trim())
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| if key { l.strip_prefix("repo=") } else { Some(l) })
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                let url = w.next()?.trim_end_matches('/').trim_end_matches(".git").to_string();
                is_url(&url).then(|| (url, w.next().unwrap_or("").to_string(), key))
            })
            .collect()
    };
    let conf = std::fs::read_to_string(p(CONF)).unwrap_or_default();
    let own = parse(&conf, true);
    let mut all = if kv(&conf).get("standard").map(|s| s == "off").unwrap_or(false) && !own.is_empty() { Vec::new() } else { parse(&std::fs::read_to_string(p(REPOS_STD)).unwrap_or_default(), false) };
    for r in own {
        if !all.iter().any(|x| x.0 == r.0) {
            all.push(r);
        }
    }
    all
}
/// Where a repository lists its releases: GitHub has its own host for that, Gitea and Forgejo
/// have /api/v1 on the same host.
fn releases_url(repo: &str) -> Option<String> {
    let rest = repo.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let mut parts = path.split('/').filter(|x| !x.is_empty());
    let (owner, name) = (parts.next()?, parts.next()?);
    Some(if host == "github.com" { format!("https://api.github.com/repos/{owner}/{name}/releases?per_page=30") } else { format!("https://{host}/api/v1/repos/{owner}/{name}/releases?limit=30") })
}

/// Just enough JSON for a list of releases.
#[derive(Debug, Clone, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}
impl J {
    fn get(&self, k: &str) -> &J {
        match self {
            J::Obj(m) => m.iter().find(|(n, _)| n == k).map(|(_, v)| v).unwrap_or(&J::Null),
            _ => &J::Null,
        }
    }
    fn str(&self) -> &str {
        match self { J::Str(s) => s, _ => "" }
    }
    fn arr(&self) -> &[J] {
        match self { J::Arr(a) => a, _ => &[] }
    }
}
fn json(text: &str) -> Option<J> {
    fn ws(b: &[u8], i: &mut usize) {
        while *i < b.len() && (b[*i] as char).is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn string(b: &[u8], i: &mut usize) -> Option<String> {
        *i += 1;
        let mut out = Vec::new();
        while *i < b.len() {
            match b[*i] {
                b'"' => { *i += 1; return String::from_utf8(out).ok(); }
                b'\\' => {
                    *i += 1;
                    match *b.get(*i)? {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'u' => {
                            let cp = u32::from_str_radix(std::str::from_utf8(b.get(*i + 1..*i + 5)?).ok()?, 16).ok()?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(char::from_u32(cp).unwrap_or('?').encode_utf8(&mut buf).as_bytes());
                            *i += 4;
                        }
                        c => out.push(c),
                    }
                    *i += 1;
                }
                c => { out.push(c); *i += 1; }
            }
        }
        None
    }
    fn value(b: &[u8], i: &mut usize, depth: u32) -> Option<J> {
        if depth > 40 {
            return None;
        }
        ws(b, i);
        match *b.get(*i)? {
            b'{' => {
                *i += 1;
                let mut m = Vec::new();
                loop {
                    ws(b, i);
                    if *b.get(*i)? == b'}' { *i += 1; return Some(J::Obj(m)); }
                    if *b.get(*i)? == b',' { *i += 1; continue; }
                    let k = string(b, i)?;
                    ws(b, i);
                    if *b.get(*i)? != b':' { return None; }
                    *i += 1;
                    m.push((k, value(b, i, depth + 1)?));
                }
            }
            b'[' => {
                *i += 1;
                let mut a = Vec::new();
                loop {
                    ws(b, i);
                    if *b.get(*i)? == b']' { *i += 1; return Some(J::Arr(a)); }
                    if *b.get(*i)? == b',' { *i += 1; continue; }
                    a.push(value(b, i, depth + 1)?);
                }
            }
            b'"' => string(b, i).map(J::Str),
            b't' => { *i += 4; Some(J::Bool(true)) }
            b'f' => { *i += 5; Some(J::Bool(false)) }
            b'n' => { *i += 4; Some(J::Null) }
            _ => {
                let st = *i;
                while *i < b.len() && matches!(b[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    *i += 1;
                }
                std::str::from_utf8(&b[st..*i]).ok()?.parse().ok().map(J::Num)
            }
        }
    }
    let mut i = 0;
    value(text.as_bytes(), &mut i, 0)
}

/// One version on offer.
#[derive(Debug, Clone, PartialEq)]
struct Version {
    tag: String,
    date: String, // as the repository gives it: 2026-11-27T19:18:13Z
    pre: bool,
    repo: String,
    rootfs: Asset, // the root file system; no address: this release cannot be installed
    sums: Asset,   // SHA256SUMS
    size: u64,
}
fn versions_of(repo: &str, text: &str) -> Vec<Version> {
    let Some(j) = json(text) else { return Vec::new() };
    j.arr()
        .iter()
        .filter(|r| !matches!(r.get("draft"), J::Bool(true)))
        .map(|r| {
            let asset = |pick: &dyn Fn(&str) -> bool| r.get("assets").arr().iter().find(|a| pick(a.get("name").str())).cloned().unwrap_or(J::Null);
            let fs = asset(&|n| n.ends_with(".rootfs.jffs2"));
            let sums = asset(&|n| n == "SHA256SUMS");
            Version {
                tag: r.get("tag_name").str().chars().filter(|c| c.is_ascii_alphanumeric() || "._-+".contains(*c)).take(60).collect(),
                date: r.get("published_at").str().chars().take(25).collect(),
                pre: matches!(r.get("prerelease"), J::Bool(true)),
                repo: repo.to_string(),
                rootfs: Asset { url: Some(fs.get("browser_download_url").str()).filter(|u| is_url(u) && !sums.get("browser_download_url").str().is_empty()).unwrap_or("").to_string(), api: fs.get("url").str().to_string() },
                sums: Asset { url: sums.get("browser_download_url").str().to_string(), api: sums.get("url").str().to_string() },
                size: match fs.get("size") { J::Num(n) => *n as u64, _ => 0 },
            }
        })
        .filter(|v| !v.tag.is_empty())
        .collect()
}
/// Every version the repositories offer, newest first. Errors per repository are told, not fatal.
fn list() -> Vec<Version> {
    let mut all = Vec::new();
    for (repo, token, _) in repos() {
        let Some(api) = releases_url(&repo) else { eprintln!("j49-update: {repo}: not the address of a repository"); continue };
        match fetch(&api, &token).output() {
            Ok(o) if o.status.success() => all.extend(versions_of(&repo, &String::from_utf8_lossy(&o.stdout))),
            _ => eprintln!("j49-update: {repo}: the list of releases could not be fetched"),
        }
    }
    all.sort_by(|a, b| b.date.cmp(&a.date));
    let text: String = all.iter().map(|v| format!("{}\t{}\t{}\t{}\t{}\t{}\n", v.tag, v.date, if v.pre { "pre" } else { "release" }, if v.rootfs.url.is_empty() { "no" } else { "yes" }, v.size, v.repo)).collect();
    let _ = std::fs::write(p(VERSIONS), text);
    all
}
fn token_of(repo: &str) -> String {
    repos().into_iter().find(|x| x.0 == repo).map(|x| x.1).unwrap_or_default()
}
/// The repository the running version was installed from by an update, if it was.
fn installed_from() -> String {
    let u = kv_file(INSTALLED);
    if u.get("slot").is_some() && u.get("slot").cloned() == running_slot().map(|s| format!("root{s}")) { u.get("repo").cloned().unwrap_or_default() } else { String::new() }
}
/// What runs: (version if it came from an update or a tagged build, commit, commit date, built).
fn installed() -> (String, String, String, String) {
    let r = kv_file("/etc/j49-release");
    let g = |k: &str| r.get(k).cloned().unwrap_or_default();
    let mut version = g("version");
    // a version put there by an update, when it is about this very system
    let u = kv_file(INSTALLED);
    if u.get("slot").is_some() && u.get("slot").cloned() == running_slot().map(|s| format!("root{s}")) {
        version = u.get("version").cloned().unwrap_or(version);
    }
    (version, g("commit"), g("commit_date"), g("built"))
}
/// The newest version that may be offered, and whether it is newer than what runs.
fn newest(all: &[Version]) -> Option<(Version, bool)> {
    let pre = kv_file(CONF).get("prerelease").map(|s| s == "on").unwrap_or(false);
    let v = all.iter().find(|v| !v.rootfs.url.is_empty() && (pre || !v.pre))?.clone();
    let (version, _, _, built) = installed();
    // newer: another name than what runs, and published after this system was built
    let newer = v.tag != version && (built.is_empty() || v.date.as_str() > built.as_str());
    Some((v, newer))
}
fn write_state(all: &[Version]) {
    let (version, commit, date, _) = installed();
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (latest, newer) = newest(all).map(|(v, n)| (v.tag, n)).unwrap_or_default();
    let latest_from = newest(all).map(|(v, _)| v.repo).unwrap_or_default();
    let _ = std::fs::write(p(STATE), format!("installed={version}\nfrom={}\ncommit={commit}\ncommit_date={date}\nlatest={latest}\nlatest_from={latest_from}\nnewer={}\nchecked={t}\n", installed_from(), if newer { "yes" } else { "no" }));
}
fn set_repos(change: impl Fn(&mut Vec<String>)) {
    let old = std::fs::read_to_string(p(CONF)).unwrap_or_default();
    let mut other: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("repo=")).map(|l| l.to_string()).collect();
    let mut own: Vec<String> = old.lines().filter(|l| l.trim_start().starts_with("repo=")).map(|l| l.trim().to_string()).collect();
    change(&mut own);
    other.extend(own);
    let _ = std::fs::create_dir_all(p("/var/lib/j49"));
    let tmp = p(&format!("{CONF}.tmp"));
    // a token may be in it: for root only
    use std::os::unix::fs::OpenOptionsExt;
    let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all((other.join("\n") + "\n").as_bytes())).is_ok();
    if !ok || std::fs::rename(&tmp, p(CONF)).is_err() {
        die("could not write the settings");
    }
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
fn keep_files(version: &str, repo: &str, slot: &str) -> Result<(), String> {
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
    // the name of the version, for the new system only: it is not written into the running one,
    // which stays what it is when the trial fails
    if !version.is_empty() {
        let _ = std::fs::create_dir_all(format!("{dest}/var/lib/j49"));
        let _ = std::fs::write(format!("{dest}{INSTALLED}"), format!("version={version}\nrepo={repo}\nslot={slot}\n"));
    }
    let _ = Command::new("sync").status();
    let _ = Command::new("umount").arg(mnt).status();
    if ok { Ok(()) } else { Err("not every file could be copied to scratch".into()) }
}
enum Source<'a> {
    Plain(&'a str),                        // a file, or an https address
    Release(&'a Asset, &'a str, &'a str),  // a file of a release, its repository and token
}
fn install(source: Source, version: &str, want_sha: &str, want_size: Option<u64>, reboot: bool) {
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
    let from = match &source { Source::Release(_, repo, _) => repo.to_string(), _ => String::new() };
    let download = |mut cmd: Command| -> Result<(String, String, u64), String> {
        let mut c = cmd.stdout(Stdio::piped()).spawn().map_err(|e| format!("curl: {e}"))?;
        let r = stream(c.stdout.take().unwrap(), &dev);
        if !c.wait().map(|s| s.success()).unwrap_or(false) { Err("the download broke off".into()) } else { r }
    };
    let res = match source {
        Source::Release(a, repo, token) => download(fetch_asset(a, repo, token)),
        Source::Plain(src) if is_url(src) => download(fetch(src, "")),
        Source::Plain(src) => std::fs::File::open(src).map_err(|e| format!("{src}: {e}")).and_then(|f| stream(f, &dev)),
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
    if let Err(e) = keep_files(version, &from, name) {
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
            let (version, commit, date, built) = installed();
            println!("version={}", if version.is_empty() { "(not from a release)" } else { &version });
            if !installed_from().is_empty() {
                println!("from={}", installed_from());
            }
            println!("commit={commit}\ncommit_date={date}\nbuilt={built}");
            println!("slot={}", running_slot().map(|s| format!("root{s}")).unwrap_or("unknown".into()));
            match rec_read() {
                Some(r) => println!("record=active root{}{}", r.active, r.trial.map(|t| format!(", trial of root{t} ({} left)", r.left)).unwrap_or_default()),
                None => println!("record=none (the kernel command line decides)"),
            }
            println!("check={}", c.get("check").map(|s| s.as_str()).unwrap_or("off"));
            println!("prerelease={}", c.get("prerelease").map(|s| s.as_str()).unwrap_or("off"));
            for (url, token, own) in repos() {
                println!("repo={url} ({}{})", if own { "your own" } else { "standard" }, if token.is_empty() { "" } else { ", with a token" });
            }
            print!("{}", std::fs::read_to_string(p(STATE)).unwrap_or_default().lines().filter(|l| l.starts_with("latest=") || l.starts_with("newer=") || l.starts_with("checked=") || l.starts_with("busy=") || l.starts_with("error=")).map(|l| format!("{l}\n")).collect::<String>());
        }
        ["list"] | ["check"] => {
            if words[0] == "check" && kv_file(CONF).get("check").map(|s| s != "on").unwrap_or(true) && !args.iter().any(|a| a == "--now") {
                return; // not asked for
            }
            let all = list();
            write_state(&all);
            let (version, ..) = installed();
            for v in &all {
                let runs = v.tag == version && (installed_from().is_empty() || installed_from() == v.repo);
                println!("{:<22} {:<20} {:<8} {}{}{}", v.tag, v.date, if v.pre { "pre" } else { "release" }, v.repo.trim_start_matches("https://"), if v.rootfs.url.is_empty() { "  (no image attached)" } else { "" }, if runs { "  <- runs now" } else { "" });
            }
            if all.is_empty() {
                println!("no versions found");
            }
        }
        ["install"] | ["install", _] => {
            let all = list();
            let v = match words.get(1) {
                Some(tag) => {
                    // the same name can be in two repositories: --from=<address> says which
                    let from = args.iter().find_map(|a| a.strip_prefix("--from=")).map(|u| u.trim_end_matches('/').trim_end_matches(".git").to_string());
                    let hits: Vec<&Version> = all.iter().filter(|v| v.tag == *tag && from.as_ref().map(|f| *f == v.repo).unwrap_or(true)).collect();
                    if hits.len() > 1 {
                        die(&format!("version {tag} is in more than one repository: say which with --from=<address>"));
                    }
                    hits.first().cloned().cloned().unwrap_or_else(|| die(&format!("no version {tag} in the repositories")))
                }
                None => newest(&all).map(|x| x.0).unwrap_or_else(|| die("the repositories offer no version that can be installed")),
            };
            if v.rootfs.url.is_empty() {
                die(&format!("version {} has no root file system and SHA256SUMS attached", v.tag));
            }
            let token = token_of(&v.repo);
            let sums = fetch_asset(&v.sums, &v.repo, &token).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_else(|| die("SHA256SUMS could not be fetched"));
            let file = v.rootfs.url.rsplit('/').next().unwrap_or("");
            let sha = sums.lines().find_map(|l| { let mut w = l.split_whitespace(); let h = w.next()?; (w.next()?.trim_start_matches('*') == file).then(|| h.to_string()) }).unwrap_or_else(|| die("SHA256SUMS does not name the root file system"));
            eprintln!("j49-update: version {} from {}", v.tag, v.repo);
            install(Source::Release(&v.rootfs, &v.repo, &token), &v.tag, &sha, Some(v.size).filter(|s| *s > 0), reboot);
        }
        ["install", src, sha] => install(Source::Plain(src), "", sha, std::fs::metadata(src).ok().map(|m| m.len()), reboot),
        ["repo", "add", url] | ["repo", "add", url, _] => {
            let url = url.trim_end_matches('/').trim_end_matches(".git").to_string();
            if releases_url(&url).is_none() || !is_url(&url) {
                die("that is not the https address of a repository (https://host/owner/name)");
            }
            let token = words.get(3).copied().unwrap_or("");
            set_repos(|own| {
                own.retain(|l| l.split_whitespace().next() != Some(&format!("repo={url}")));
                own.push(format!("repo={url} {token}").trim().to_string());
            });
            println!("added; it is used next to the standard repositories (standard=off in the settings leaves those out)");
        }
        ["repo", "remove", url] => {
            let url = url.trim_end_matches('/').trim_end_matches(".git").to_string();
            set_repos(|own| own.retain(|l| l.split_whitespace().next() != Some(&format!("repo={url}"))));
            println!("removed");
        }
        ["repo", "reset"] => {
            set_repos(|own| own.clear());
            println!("only the standard repositories are in use");
        }
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
            let _ = std::fs::remove_file(p(STATE)); // what was found before the restart is about another system
        }
        ["back"] => {
            let Some(me) = running_slot() else { die("cannot tell which slot is running") };
            rec_write(&Rec { active: me, trial: Some(1 - me), left: TRIES }).unwrap_or_else(|e| die(&e));
            eprintln!("j49-update: trial of root{} asked for; restart to start it", 1 - me);
        }
        _ => {
            eprintln!("usage: j49-update status | list | check [--now] | install [<version>] | install <file|https address> <sha256> | repo add <address> [token] | repo remove <address> | repo reset | confirm | back   (--no-reboot with install)");
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
    fn releases() {
        assert_eq!(releases_url("https://github.com/wian007/nest-v2-firmware").as_deref(), Some("https://api.github.com/repos/wian007/nest-v2-firmware/releases?per_page=30"));
        assert_eq!(releases_url("https://git.example.org/me/fw").as_deref(), Some("https://git.example.org/api/v1/repos/me/fw/releases?limit=30"));
        assert_eq!(releases_url("https://github.com/onlyowner"), None);
        let text = r#"[
          {"tag_name":"v2","published_at":"2026-12-02T10:00:00Z","prerelease":true,"draft":false,"name":"a \"quoted\" \u00e9",
           "assets":[{"name":"j49-image-nest-j49.rootfs.jffs2","size":123,"browser_download_url":"https://h/d/v2/j49-image-nest-j49.rootfs.jffs2"},
                     {"name":"SHA256SUMS","size":5,"browser_download_url":"https://h/d/v2/SHA256SUMS"}]},
          {"tag_name":"v1","published_at":"2026-12-01T10:00:00Z","prerelease":false,"assets":[]},
          {"tag_name":"draft","draft":true,"assets":[]}
        ]"#;
        let v = versions_of("https://h/o/r", text);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].tag.as_str(), v[0].pre, v[0].size, v[0].rootfs.url.as_str()), ("v2", true, 123, "https://h/d/v2/j49-image-nest-j49.rootfs.jffs2"));
        assert_eq!((v[1].tag.as_str(), v[1].pre, v[1].rootfs.url.as_str()), ("v1", false, ""));
        assert!(versions_of("x", "not json").is_empty());
        assert!(versions_of("x", "{\"message\":\"Not Found\"}").is_empty());
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
