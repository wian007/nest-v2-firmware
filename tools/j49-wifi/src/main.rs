// j49-wifi: a way to set up WiFi on a thermostat that has no network.
//
// When no network is configured, or none could be joined for WAIT_SECS, the thermostat becomes
// a WiFi access point itself ("J49-setup-A1B2", open). Somebody connects a phone or laptop to
// it and opens the web page of the thermostat (http://192.168.4.1/, j49-web); login there is
// confirmed on the device with the ring, as always, and the WiFi block of the page adds the
// home network. As soon as a network was added the access point closes and the thermostat
// joins it. The screen (j49-screen) shows what to do while the access point is open.
//
// It only talks to what is there already:
//   wpa_supplicant   through its control socket: an extra network with mode=2 (access point)
//   systemd-networkd a .network file in /run that gives wlan0 192.168.4.1 and a DHCP server
//   /run/j49-wifi.state   mode=station|ap, ssid=, ip=, since=   (read by j49-screen, j49-sleep)
//   /run/j49-wifi.want    "on": open the access point now (the menu on the device), also when
//                         a network is joined; "off": close it now. Removed when acted on.
//
// The access point is open (no password): what protects the thermostat is that the web page
// lets nobody in without a confirmation on the device itself. It is only there while the
// thermostat has no network, and at most AP_SECS at a time when networks are configured (the
// router may just have been off: it then tries them again).
//
// No dependencies. J49_ROOT=<dir> prefixes every path, for tests on a PC.
use std::collections::HashMap;
use std::time::{Duration, Instant};

const WPA_CTRL: &str = "/run/wpa_supplicant/wlan0";
const WPA_CONF: &str = "/etc/wpa_supplicant/wpa_supplicant-wlan0.conf";
const STATE: &str = "/run/j49-wifi.state";
const WANT: &str = "/run/j49-wifi.want"; // "on" / "off": the hotspot row in the menu of the screen
const AP_NETWORK: &str = "/run/systemd/network/70-wlan0-setup.network";
const AP_IP: &str = "192.168.4.1";
const WAIT_SECS: u64 = 300; // without a connection for this long: open the access point
const AP_SECS: u64 = 600; // with networks configured: close it after this long and try them again
const BOOT_SECS: u64 = 25; // after start: give wpa_supplicant time before deciding anything

fn root() -> String {
    std::env::var("J49_ROOT").unwrap_or_default()
}
fn p(path: &str) -> String {
    format!("{}{}", root(), path)
}
fn kv_text(text: &str) -> HashMap<String, String> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}
fn wpa(cmd: &str) -> Option<String> {
    use std::os::unix::net::UnixDatagram;
    let local = p(&format!("/run/j49-wifi-ctrl-{}", std::process::id()));
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
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What wpa_supplicant knows: (id, ssid, is the access point of ours) per network.
fn networks(list: &str, ap_ssid: &str) -> Vec<(u32, String, bool)> {
    // "network id / ssid / bssid / flags" then one line per network, tab separated
    list.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let id = f.next()?.trim().parse::<u32>().ok()?;
            let ssid = f.next()?.to_string();
            let ours = ssid == ap_ssid;
            Some((id, ssid, ours))
        })
        .collect()
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Step {
    Stay,
    OpenAp,
    CloseAp,
}
/// The whole policy, without side effects, so it can be tested.
/// connected: joined a network as a station; configured: networks other than our own;
/// ap: our access point is open; secs: how long the present state (not connected, or ap) lasts;
/// grew: a network was added since the access point opened; uptime: seconds since we started;
/// forced: asked for in the menu; held: closed by hand, so not opened again at once.
#[allow(clippy::too_many_arguments)]
fn decide(wifi_on: bool, connected: bool, configured: usize, ap: bool, secs: u64, grew: bool, uptime: u64, forced: bool, held: bool) -> Step {
    if !wifi_on {
        return if ap { Step::CloseAp } else { Step::Stay };
    }
    if ap {
        // somebody added a network: go and join it. Or: networks are known, try them again.
        if grew || (configured > 0 && secs >= AP_SECS) {
            return Step::CloseAp;
        }
        return Step::Stay;
    }
    if forced {
        return Step::OpenAp;
    }
    if connected || uptime < BOOT_SECS {
        return Step::Stay;
    }
    if (configured == 0 && !held) || secs >= WAIT_SECS {
        return Step::OpenAp;
    }
    Step::Stay
}

fn ap_ssid() -> String {
    let mac = std::fs::read_to_string(p("/sys/class/net/wlan0/address")).unwrap_or_default();
    let tail: String = mac.trim().replace(':', "").chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("J49-setup-{}", if tail.len() == 4 { tail.to_uppercase() } else { "0000".into() })
}
fn write_state(mode: &str, ssid: &str, since: u64) {
    let body = format!("mode={mode}\nssid={ssid}\nip={}\nsince={since}\n", if mode == "ap" { AP_IP } else { "" });
    let tmp = p(&format!("{STATE}.tmp"));
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, p(STATE));
    }
}
fn networkctl(args: &[&str]) {
    if root().is_empty() {
        let _ = std::process::Command::new("networkctl").args(args).status();
    }
}
fn open_ap(ssid: &str) -> Option<u32> {
    let id = wpa("ADD_NETWORK")?.parse::<u32>().ok()?;
    let set = |k: &str, v: &str| wpa(&format!("SET_NETWORK {id} {k} {v}")).map(|r| r == "OK").unwrap_or(false);
    // channel 6 (2437 MHz): allowed everywhere
    let ok = set("ssid", &hex(ssid.as_bytes())) && set("mode", "2") && set("key_mgmt", "NONE") && set("frequency", "2437");
    if !ok {
        let _ = wpa(&format!("REMOVE_NETWORK {id}"));
        return None;
    }
    // wlan0 gets a fixed address and hands out addresses; DHCP option 114 tells a phone where
    // the page is ("sign in to the network")
    let net = format!(
        "# written by j49-wifi while its setup access point is open\n[Match]\nName=wlan0\n\n[Network]\nAddress={AP_IP}/24\nDHCPServer=yes\n\n[DHCPServer]\nPoolOffset=10\nPoolSize=20\nEmitDNS=no\nEmitRouter=no\nSendOption=114:string:http://{AP_IP}/\n"
    );
    let path = p(AP_NETWORK);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, net);
    // SELECT_NETWORK switches the others off until the access point is closed again
    if wpa(&format!("SELECT_NETWORK {id}")).as_deref() != Some("OK") {
        let _ = wpa(&format!("REMOVE_NETWORK {id}"));
        let _ = std::fs::remove_file(&path);
        return None;
    }
    networkctl(&["reload"]);
    networkctl(&["reconfigure", "wlan0"]);
    Some(id)
}
fn close_ap(id: u32) {
    let _ = wpa(&format!("REMOVE_NETWORK {id}"));
    let _ = wpa("ENABLE_NETWORK all");
    let _ = std::fs::remove_file(p(AP_NETWORK));
    networkctl(&["reload"]);
    networkctl(&["reconfigure", "wlan0"]);
    let _ = wpa("REASSOCIATE");
}

/// A thermostat that was just installed has no WiFi configuration file, and wpa_supplicant
/// does not start without one: then there is nobody to open an access point with. Write an
/// empty configuration (no networks) and start it.
fn ensure_supplicant() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    if std::path::Path::new(&p(WPA_CONF)).exists() {
        return;
    }
    let path = p(WPA_CONF);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let made = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).and_then(|mut f| f.write_all(b"ctrl_interface=/run/wpa_supplicant\n"));
    if made.is_ok() {
        eprintln!("j49-wifi: no WiFi configuration yet: wrote an empty one and start wpa_supplicant");
        if root().is_empty() {
            let _ = std::process::Command::new("systemctl").args(["restart", "wpa_supplicant@wlan0"]).status();
        }
    }
}

fn main() {
    let started = Instant::now();
    ensure_supplicant();
    let ssid = ap_ssid();
    let mut ap: Option<(u32, usize)> = None; // (network id, networks configured when it opened)
    let mut since = Instant::now(); // start of "not connected" or of the access point
    let mut held = false; // closed from the menu: not opened again before WAIT_SECS
    // an access point of ours left behind by an earlier run of this program
    if let Some(list) = wpa("LIST_NETWORKS") {
        for (id, _, ours) in networks(&list, &ssid) {
            if ours {
                close_ap(id);
            }
        }
    }
    // a network file of ours left in /run by a run that was killed
    if std::fs::remove_file(p(AP_NETWORK)).is_ok() {
        networkctl(&["reload"]);
        networkctl(&["reconfigure", "wlan0"]);
    }
    write_state("station", "", 0);
    eprintln!("j49-wifi: watching wlan0; setup access point would be \"{ssid}\"");
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let wifi_on = kv_text(&std::fs::read_to_string(p("/var/lib/j49-screen/settings")).unwrap_or_default())
            .get("wifi")
            .map(|v| v != "0")
            .unwrap_or(true);
        let Some(status) = wpa("STATUS") else {
            ensure_supplicant(); // wpa_supplicant is not there (yet)
            continue;
        };
        let st = kv_text(&status);
        let Some(list) = wpa("LIST_NETWORKS") else { continue };
        let nets = networks(&list, &ssid);
        let configured = nets.iter().filter(|n| !n.2).count();
        // wpa_supplicant was restarted under us: the access point is gone with it
        if ap.is_some() && !nets.iter().any(|n| n.2) {
            ap = None;
            let _ = std::fs::remove_file(p(AP_NETWORK));
            networkctl(&["reload"]);
            networkctl(&["reconfigure", "wlan0"]);
            since = Instant::now();
            write_state("station", "", 0);
            eprintln!("j49-wifi: the setup access point is gone (wpa_supplicant restarted)");
            continue;
        }
        let connected = ap.is_none() && st.get("wpa_state").map(|s| s == "COMPLETED").unwrap_or(false) && st.get("mode").map(|m| m == "station").unwrap_or(true);
        if connected {
            since = Instant::now();
        }
        let grew = ap.map(|(_, n)| configured > n).unwrap_or(false);
        let want = std::fs::read_to_string(p(WANT)).unwrap_or_default();
        let forced = want.trim() == "on";
        if want.trim() == "off" {
            let _ = std::fs::remove_file(p(WANT));
            if let Some((id, _)) = ap.take() {
                close_ap(id);
                eprintln!("j49-wifi: setup access point closed (from the menu)");
                since = Instant::now();
                held = true;
                write_state("station", "", 0);
                continue;
            }
        }
        if connected {
            held = false;
        }
        match decide(wifi_on, connected, configured, ap.is_some(), since.elapsed().as_secs(), grew, started.elapsed().as_secs(), forced, held) {
            Step::Stay => {}
            Step::OpenAp => match open_ap(&ssid) {
                Some(id) => {
                    held = false;
                    eprintln!("j49-wifi: {} ({configured} configured): setup access point \"{ssid}\" open, page on http://{AP_IP}/", if forced { "asked for in the menu" } else { "no network" });
                    ap = Some((id, configured));
                    since = Instant::now();
                    write_state("ap", &ssid, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
                }
                None => {
                    let _ = std::fs::remove_file(p(WANT));
                    eprintln!("j49-wifi: could not open the setup access point (does wpa_supplicant have access point support?)");
                    since = Instant::now(); // not every 5 s
                }
            },
            Step::CloseAp => {
                if let Some((id, _)) = ap.take() {
                    // a moment for the page that added the network to get its answer
                    std::thread::sleep(Duration::from_secs(3));
                    close_ap(id);
                    let _ = std::fs::remove_file(p(WANT));
                    eprintln!("j49-wifi: setup access point closed ({})", if grew { "a network was added" } else if wifi_on { "trying the configured networks again" } else { "WiFi is switched off" });
                    since = Instant::now();
                    write_state("station", "", 0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy() {
        use Step::*;
        // connected: nothing happens, however long
        assert_eq!(decide(true, true, 1, false, 9999, false, 9999, false, false), Stay);
        // no network configured: the access point opens once the start-up time is over
        assert_eq!(decide(true, false, 0, false, 0, false, 5, false, false), Stay);
        assert_eq!(decide(true, false, 0, false, 30, false, 30, false, false), OpenAp);
        // networks configured but none joined: only after five minutes
        assert_eq!(decide(true, false, 2, false, 299, false, 999, false, false), Stay);
        assert_eq!(decide(true, false, 2, false, 300, false, 999, false, false), OpenAp);
        // access point open, nothing configured: it stays open
        assert_eq!(decide(true, false, 0, true, 99_999, false, 99_999, false, false), Stay);
        // a network was added through the page: close and join it
        assert_eq!(decide(true, false, 1, true, 20, true, 999, false, false), CloseAp);
        // networks configured: after ten minutes try them again
        assert_eq!(decide(true, false, 2, true, 599, false, 9999, false, false), Stay);
        assert_eq!(decide(true, false, 2, true, 600, false, 9999, false, false), CloseAp);
        // asked for in the menu: at once, also when connected; it ends like any other
        assert_eq!(decide(true, true, 1, false, 0, false, 9999, true, false), OpenAp);
        assert_eq!(decide(true, false, 1, true, 600, false, 9999, true, false), CloseAp);
        // closed from the menu with nothing configured: not again at once, but after five minutes
        assert_eq!(decide(true, false, 0, false, 10, false, 9999, false, true), Stay);
        assert_eq!(decide(true, false, 0, false, 300, false, 9999, false, true), OpenAp);
        // WiFi switched off on the device: no access point
        assert_eq!(decide(false, false, 0, false, 9999, false, 9999, false, false), Stay);
        assert_eq!(decide(false, false, 0, true, 10, false, 9999, false, false), CloseAp);
    }
    #[test]
    fn network_list() {
        let list = "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n1\tGuest net\tany\t[DISABLED]\n2\tJ49-setup-A1B2\tany\t\n";
        let n = networks(list, "J49-setup-A1B2");
        assert_eq!(n, vec![(0, "Home".to_string(), false), (1, "Guest net".to_string(), false), (2, "J49-setup-A1B2".to_string(), true)]);
        assert_eq!(n.iter().filter(|x| !x.2).count(), 2);
        assert!(networks("FAIL", "x").is_empty());
    }
}
