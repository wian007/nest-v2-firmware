# Thermostat on the J49: files, control loop, schedule

Two programs share the work. `j49-screen` is the user interface (ring, screen). `bpd` owns the
backplate and does the control. They talk through small text files, and those files are also the
interface for anything else (SSH now, Home Assistant later): change a file and the device follows
within a second or two.

## /var/lib/j49/thermostat (settings, persistent)

    mode=heat          # heat | off
    setpoint=20.5      # target, 9.0 .. 32.0
    offset=-2.0        # added to the SHT20 reading; the head unit warms its own sensor
    frost=5.0          # frost protection threshold used in mode off; 0 disables it
    schedule=on        # on | off: follow /var/lib/j49/schedule
    away=off           # on = nobody home: in mode heat the target is away_temp, not setpoint
    away_temp=15.0     # target while away, 9.0 .. 32.0 (default 15.0)
    min_off=10         # seconds off before a new start (default 10, 0 .. 600)
    min_on=0           # seconds on before switching off for "warm enough" (default 0, 0 .. 600)
    tz=CET-1CEST,M3.5.0,M10.5.0/3   # optional POSIX time zone rule; this is the default

`j49-screen` rewrites it when the ring changes something and keeps lines it does not know. `bpd`
rewrites only the `setpoint=` line (when the schedule fires). `j49-web` and `j49-mqtt` replace
single keys. While away the setpoint and the schedule go on underneath, so `away=off` is all that
coming home needs; turning the ring on the home screen does that too. Write it atomically (temporary file,
then rename). `j49-screen` re-reads it every second.

## /var/lib/j49/schedule (persistent)

One setpoint change per line, `<days> <HH:MM> <temperature>`, local time:

    mon-fri 06:30 20.0
    mon-fri 08:30 17.0
    mon-fri 17:00 20.5
    mon-fri 22:30 16.0
    sat,sun 08:00 20.5
    sat,sun 23:00 16.0

Days: `mon tue wed thu fri sat sun`; a range `mon-fri` (may wrap, `fri-mon`), a list `sat,sun`,
or `daily`. (The Dutch codes `ma di wo do vr za zo` and `dagelijks` of earlier versions are still
read.) `#` starts a comment; lines that do not parse or have a
temperature outside 9..32 are ignored. `j49-screen` writes this default the first time SCHEMA is
switched on in the menu.

Behaviour, like the original: at each change the setpoint becomes that temperature. Turning the
ring in between holds until the next change. `bpd` remembers the last change it applied in
`/var/lib/j49/schedule.state` (unix time); delete that file to make it apply the change that is
in force right now. The schedule only acts with `mode=heat` and `schedule=on`, and only when the
clock is set (year 2025 or later).

## /run/bpd/heat (state, written by bpd every second)

    demand=1           # the control loop wants heat
    relay=1            # last state the backplate acknowledged (absent until it answers)
    frost=0            # 1 while frost protection is the reason
    sched=1            # schedule in use; then also:
    next_hhmm=2230     # time of the next change
    next_x10=160       # its temperature in tenths
    mode=heat
    setpoint=20.5
    away=0             # 1 while away
    target=20.5        # what the control aims for: setpoint, or away_temp while away
    wait=0             # seconds until the heating may start (minimum off time); 0 = not waiting
    room=19.84         # corrected room temperature (absent without a fresh reading)
    reason=below setpoint
    t=1795706282       # unix time

Other state files of `bpd`: `/run/bpd/sht20` (raw temperature, humidity), `/run/bpd/pir`,
`/run/bpd/als`.

## Control loop (bpd, once a second)

- Room temperature = SHT20 + offset.
- Mode heat: demand on at or below setpoint - 0.3 C, off at or above setpoint + 0.3 C.
- Mode off: demand only for frost protection (on at the threshold, off 1 C above it).
- No temperature for 10 minutes: demand off, whatever the mode.
- The heating is not restarted within `min_off` of stopping and runs at least `min_on` (defaults
  10 s and 0 s, for a valve; **set 180 s and 60 s for a boiler**, "Boiler" on the web page); switching
  off because the mode is off or the sensor is gone is not delayed.
- The demand goes to the backplate as `0x82 00 <state>`, repeated every 5 s until the backplate
  acknowledges that state with `0x0006 00 <state>` (`backplate-heatlink-flow.md`).
- On SIGTERM/SIGINT `bpd` sends "off" before it exits; after a backplate reset it sends the
  current demand again.

## What is verified

Unit tests in `tools/bpd` cover the decisions, the schedule parser and lookup, and the time zone
rule. On the device (off the backplate): the schedule set the setpoint and reported the next
change correctly in local time; the screens were checked through the framebuffer. Later, on a
backplate with a Heat Link: the relay follows the demand (`backplate-heatlink-flow.md`).

## The screen (j49-screen)

A dial like the thermostat card of Home Assistant, on the round panel.

- **Dial:** an arc around the edge from 9 to 32 degrees, filled up to the target (orange while
  the Heat Link switch is on, amber otherwise, nothing in mode off); a small tick marks the room
  temperature. In the middle the target temperature large, under it the room temperature and
  humidity and the next change of the schedule. Above it one line says what the heating does
  (heating, calling for heat, waiting m:ss, idle, away, frost protection, off, no backplate), with
  a flame while it heats. At the bottom the clock and the WiFi signal.
- **Ring:** turning it on the dial changes the target in steps of 0.5 (and ends away). 28 sensor
  counts are one step, after a dead band of 20 counts when the direction changes, as stock does.
  A press on a dark screen only wakes it.
- **Menu** (a click on the dial): mode heat/off, schedule on/off, away on/off, status, settings,
  back. The ring moves the cursor, a click changes the value or opens the page.
- **Status:** WiFi address, backplate, room, motion, light, battery, uptime.
- **Settings:** brightness, volume, WiFi on/off, language (English or Dutch), restart (asks
  first), back.
- **Web login:** when a browser asks to log in, the code and the choices REFUSE / ALLOW take over
  the screen (see the web page below).
- **Installer:** `--screen install` shows the phase and progress from `/run/install.state`.

The panel sleeps (backlight off) after 25 s without input and wakes for the ring, for motion
reported by the backplate, for a web login question and for `wake` on the FIFO.

Settings in `/var/lib/j49-screen/settings` (`key=number`): `brightness` 10..120, `volume` 0..10,
`wifi` 0|1, `ringrev` (not used by this screen). The file is read again when another program
changes it.

### Languages, locale and themes

`/var/lib/j49/ui` holds the choice, shared with the web page: `lang=<code>`, `theme=<name>` and
`splash=<style>`.
Both can be changed under Settings on the device.

- **Languages:** one file per language with every text, `tools/lang/<code>.txt` (`key=text`
  lines), installed in `/usr/share/j49/lang`. English and Dutch are there. To add one, copy
  `en.txt`, translate the texts and put it in `/var/lib/j49/lang/` on the device (or in
  `tools/lang/` for everybody); it then shows up under Settings. A text that a language lacks is
  shown in English. The same file says how the language writes things (`locale.*`): the decimal
  sign, a 24 or 12 hour clock, Celsius or Fahrenheit for what is shown (the control itself always
  works in Celsius), the names of days and months.
- **Start screen:** at start the screen shows a start screen for 2.5 s (a touch of the ring
  skips it). Which one is a setting, `splash=` in `/var/lib/j49/ui`, chosen on the web page
  under "Device": `name` (the name, a ring that closes and the version), `flame`, or `none`. It follows the colours of the theme; the word on it is `splash_text=` in the theme.
- **Themes:** one file per theme, `tools/themes/<name>.theme`: the colours as `RRGGBB`, the
  fonts, and the thickness of the dial.
- **Fonts** are part of the theme. `font=<name>` names a font file for the screen,
  `tools/themes/fonts/<name>.font`: text, per glyph a line `glyph <character>` and seven rows of
  five `#` or `.`. `default.font` holds every glyph of the built-in font (`j49-screen
  --dump-font` writes it); `plain.font` changes three digits. `matrix.font` is Matrix Sans, a
  dot-matrix design after LED and LCD displays (capitals, digits, signs); `fixed.font` is the
  5x7 terminal font of the X Window System, smaller, with lower case and accented letters.
  `import-font.py` in that directory turns a BDF font, glyphs in C source or a FontForge
  dot font into a font file; `LICENSES.md` says where each font comes from. `J49_FONT=<name>`
  tries a font with any theme on a PC.
- **Smooth fonts.** A font can also be a `<name>.sfont` file: a normal typeface (Roboto, Inter
  and Noto Sans are there) drawn beforehand at the six text sizes of the screen, with smooth
  edges and a width per letter. `make-sans.py <font.ttf> <name>.sfont` makes one from any
  TrueType or OpenType font (it needs Python with Pillow); mind the licence of the font.
  `font=roboto` in a theme uses it; the two Home Assistant themes do. `digits=` has no effect
  on a smooth font. They are files only (about 200 kB each), not built into the program: where
  the file is missing, the built-in font is used. On a PC: `J49_DATA=<the tools directory>`. A font of your own only has to
  hold the glyphs it changes, and it can add characters the built-in font lacks (accented
  letters, lower case) for a language that needs them. `digits=` says how large text is drawn
  from the font: `blocks`, `dots`, `smooth`, `segments` or `thin`. `web_font=` is the font of
  the web page, a CSS font-family list (fonts the browser has; nothing is downloaded). Your own
  font files go in `/var/lib/j49/themes/fonts/`. `default`, `home-assistant`, `home-assistant-light` and `light` are there. Your own go in
  `/var/lib/j49/themes/`; a theme only has to name what differs from the default.

The web page uses the same files: its texts are the `web.*` keys, its colours come from the
chosen theme, and it has a language and a theme list under "Device". `GET /api/ui` gives the
page the texts, the colours and what there is to choose from; `POST /api/ui` (`lang=`, `theme=`)
changes the choice, and the screen follows. The page shows temperatures in Celsius.

`j49-mqtt` takes the names of the entities from the same language file (`mqtt.*`) and announces
them again when the language changes. Topics, payloads and tune names are the same in every
language.

English, Dutch and the three themes are also built into the program, so it works without those
files (the installer uses it that way).

Control FIFO `/run/j49-screen.fifo`: `wake`, `sleep`, `page N` (0 dial, 1 menu, 2 status,
4 settings), `tune <name or RTTTL>` (a melody on the piezo, at most 30 s, one at a time).

It writes `/run/j49-screen.state` (`asleep=0|1`) and `/run/j49-screen.caps`: the words say what
this screen can do. This one writes `tune`. `j49-mqtt` announces the tune entities only with
`tune` and a "say" text entity only with `say`; `j49-web` shows the ring setting only with
`carousel`. Another screen program with the same file interface can take this one's place.

The clock is set by NTP from the local network only: the image has no public time servers
(`FallbackNTP=` empty), and `j49-ntp-gateway` makes the default gateway the server at boot (home
routers usually answer NTP without announcing it over DHCP). Another server: a file with `NTP=`
in `/etc/systemd/timesyncd.conf.d/`.


### A skin of your own

A theme changes colours and fonts. When the screens themselves must be different (another
home screen, another menu), write a skin: a small Rust program that uses `j49-screen` as a
library and replaces only what it wants different. The ring, the settings and thermostat files,
languages, themes, the web login question, the WiFi setup page, sleeping and the tunes stay the
code of `j49-screen`, so a skin follows the app when that changes.

    // Cargo.toml:  j49-screen = { path = "../j49-screen" }
    use j49_screen::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mine;
    impl Skin for Mine {
        // the frame of a page; None = the public one
        fn render(&self, page: usize, _t: f32) -> Option<Fb> {
            if page != P_DIAL {
                return None;
            }
            let mut fb = Fb::new();
            fb.text_center(160, 150, &fmt_temp(SETPOINT_X10.load(Ordering::Relaxed) as f32 / 10.0), 5, c::accent());
            Some(fb)
        }
    }
    fn main() {
        run(&Mine);
    }

`Skin` (top of `tools/j49-screen/src/lib.rs`) has these methods, each with a default that is the
public look:

| Method | For |
|---|---|
| `render(page, t)` | the frame of a page (`P_DIAL`, `P_MENU`, `P_STATUS`, `P_SETTINGS`, `P_CONFIRM`, `P_WIFI`, `P_INSTALL`, `P_SPLASH`, or a page of your own from `P_SKIN` up); `t` = seconds the screen has been lit, for animation |
| `click(page)`, `turn(page, step)` | a click and a step of the ring on the page shown; return true when handled |
| `ring(page, counts)` | the raw counts of the ring, for something that follows it smoothly (a carousel) |
| `fps(page)` | frames per second, for a page that animates |
| `command(line)` | a command of your own on `/run/j49-screen.fifo` |
| `caps()` | what the screen can do, for `j49-mqtt` (`/run/j49-screen.caps`) |
| `screen(name)` | a page of your own for `--screen <name>` |
| `start(once)` | called once before the first frame |

Keep the program name `j49-screen` and the service: the web page and `j49-mqtt` only know the
files. Test on a PC with `--fb <file> --screen <name> --once` (a 320x320 BGRA file) and
`J49_DEMO=1`.

## Home Assistant (j49-mqtt)

`j49-mqtt` (`tools/j49-mqtt`, service `j49-mqtt.service`) connects the files above to an MQTT
broker. It has no control logic of its own: it publishes what `bpd` reports and writes single keys
of `thermostat`, or the whole `schedule`. It is a small MQTT 3.1.1 client without dependencies
(plain TCP, QoS 0, retained state, last will `offline`).

**Broker settings** are in `/var/lib/j49/mqtt.conf` on the device, mode 600. The file holds a
password: it is not in this repository and not in the image.

```
host=192.168.1.10
port=1883
user=<mqtt user>
password=<mqtt password>
# optional: name=<name in Home Assistant>   prefix=<topic prefix>
#           discovery=homeassistant   keepalive=600
```

**More than one device on a broker.** Without `prefix=` the topics start with
`j49_<last six digits of the WiFi MAC address>`, and without `name=` the device is called
"J49 thermostaat <last four digits>", so two devices never share topics or a name. (A device
whose file says `prefix=j49` keeps that; the tables below write `j49/` for the prefix.) The device
entry in Home Assistant carries the MAC address, and there are two diagnostic sensors,
"MAC-adres" and "IP-adres" (`id/mac`, `id/ip`). The name can be set on the web page.

`keepalive` is the MQTT keep-alive in seconds (at least 10, default 600): `j49-mqtt` pings at half
of it and the broker declares the device offline after 1.5 times, so 15 minutes with the default.
It is long because the thermostat sleeps at most 0.8 of it at a time.

The session is persistent (no "clean session") and commands are taken with QoS 1; the discovery
entries carry `"qos":1`, so Home Assistant sends its commands that way. A command sent while the
device is away (asleep, WiFi down, service restarting) waits at the broker and is applied at the
next connect. Checked against mosquitto: two QoS 1 commands sent while the program was stopped
arrived at the reconnect; one sent with QoS 0 was lost, as expected.

The file can be edited by hand or through the web page (below). `j49-mqtt` sees the file change
and connects again within a few seconds; `journalctl -u j49-mqtt` shows "connected" or the reason
for a refusal, and `/run/j49-mqtt.status` (`state=connected|refused|offline|unconfigured`,
`detail=`, `t=`) says the same for the web page. Without the file, or with the network down, it
tries again every 30 s.

**Home Assistant** finds the device by MQTT discovery ("J49 thermostaat"): a climate entity
(modes off/heat, target temperature in steps of 0.5, room temperature, humidity, action),
sensors for temperature, humidity, light and battery voltage, binary sensors for motion and the
boiler relay, a switch for the schedule, the next schedule change, and a number for the frost
threshold.

| topic (prefix `j49/`) | direction | value |
|---|---|---|
| `status` | out | `online` / `offline` (last will) |
| `climate/mode`, `climate/mode/set` | out, in | `off` / `heat` |
| `climate/setpoint`, `climate/setpoint/set` | out, in | degrees C, rounded to 0.5, 9..32 |
| `climate/current`, `climate/humidity`, `climate/action` | out | corrected room temperature, RH, `heating`/`idle`/`off` |
| `relay`, `motion` | out | `ON` / `OFF` |
| `light`, `battery` | out | raw visible-light count, volts |
| `schedule/enabled`, `schedule/enabled/set` | out, in | `ON` / `OFF` |
| `schedule/next` | out | `HH:MM temperature` of the next change |
| `schedule/text`, `schedule/text/set` | out, in | the whole schedule file, same format as above |
| `schedule/mon` .. `schedule/sun`, `.../set` | out, in | one day as text: `06:30 20.0, 08:30 17.0` (up to 12 time points, decimal point) |
| `schedule/week`, `schedule/week/set` | out, in | the week as JSON, `{"mon":[{"time":"06:30","temp":20.0}],...}`; a set message may name only some days |
| `schedule/points` | out | number of time points in the week (state of the sensor that carries the week as attributes) |
| `away`, `away/set` | out, in | `ON` / `OFF`; the same setting as the preset below |
| `frost`, `frost/set` | out, in | degrees C, 0..12 |
| `climate/preset`, `climate/preset/set` | out, in | `away` / `none` (the climate entity's preset) |
| `away_temp`, `away_temp/set` | out, in | degrees C, rounded to 0.5, 9..32 |
| `screen`, `screen/set` | out, in | `ON` = panel lit, `OFF` = panel asleep; a switch in Home Assistant |
| `volume`, `volume/set` | out, in | 0..10, the piezo (beeps and tunes); a number "Volume" |
| `brightness`, `brightness/set` | out, in | 10..120, the panel; a number "Helderheid scherm" |
| `say`, `say/set` | out, in | only when the screen program lists `say` in its capabilities: a text for it to speak; a text entity "Zeg" |
| `tune`, `tune/set` | out, in | a tune by name, played on the piezo; a select "Tune". Our own: `ding`, `done`, `happy`, `alarm`, `sleep`. Public-domain melodies written out by us: `ode`, `elise`, `nachtmusik`, `toccata`, `mountainking`, `frerejacques`, `twinkle`, `birthday`, `jinglebells`, `cancan`, `tell`, `fifth`, `turkishmarch`, `spring`, `canon`, `weddingmarch`, `silentnight`, `tannenbaum`, `ducklings`, `auldlangsyne`, `yankee`, `entertainer`, `taps`, `greensleeves`, `susanna`, `marylamb`, `londonbridge`, `saints`, `charge` |
| `tune/custom`, `tune/custom/set` | out, in | a melody in RTTTL, e.g. `x:d=8,o=5,b=200:c,e,g`; a text entity "Deuntje (RTTTL)". `tune/set` takes RTTTL as well |
| `sleep`, `sleep/set` | out, in | `ON` / `OFF`: sleeping allowed (`j49-sleep`) |
| `sleep/state` | out | `slaapt` / `wakker`; "slaapt" is sent just before the device suspends |
| `sleep/last_wake`, `sleep/last_slept` | out | what woke it last (`backplate`, `wifi of ring`, `timer`) and how long it slept, in s |

`screen` is the display only (backlight off after 25 s without input), not a sleep state of the
device: the head unit does not suspend yet (see `sleep-stock.md`). `j49-screen` reports it in
`/run/j49-screen.state` (`asleep=0|1`); the command goes to its FIFO as `wake` or `sleep`, and a
woken panel goes back to sleep by itself.

State is published when it changes and all of it once a minute. Sensor topics are only published
while their file is fresh, so on a bare head unit (no backplate) temperature, motion and light
stay empty. The schedule can be read and changed per day or as a whole week; cards for the
dashboard are in `home-assistant.md`. A change written through MQTT rewrites the file with one
line per time and temperature and all the days it holds for; comments in the file are lost.

Checked on a PC against a mosquitto broker (state, discovery JSON, every command, last will) and
on the device up to the broker's answer. Not checked yet: the entities inside Home Assistant.

## WiFi setup hotspot (j49-wifi)

`j49-wifi` (`tools/j49-wifi`, service `j49-wifi.service`) gives a thermostat without a network a
way to get one. It looks at `wpa_supplicant` every 5 seconds:

| Situation | What it does |
|---|---|
| No network configured (25 s after start) | opens the hotspot |
| Networks configured, none can be joined | nothing by itself: the hotspot is then a choice in Settings on the device |
| Hotspot open, a network was added | closes it; the thermostat joins the network |
| Hotspot open for 10 minutes, networks configured | closes it and tries them again (the router may have been off) |
| "WiFi hotspot" switched on in Settings on the device | opens the hotspot at once, also when a network is joined (the thermostat leaves that network while the hotspot is open) |
| "WiFi hotspot" switched off there | closes it; it does not open again by itself until the next start |
| WiFi switched off on the device | no hotspot |

The hotspot is an extra network in the running `wpa_supplicant` (`mode=2`, name
`J49-setup-` + the last four characters of the MAC address, channel 6, open) and a file
`/run/systemd/network/70-wlan0-setup.network` that gives `wlan0` the address `192.168.4.1` and a
DHCP server. Nothing of it is written to the configuration; a restart ends it. When the
thermostat has no WiFi configuration file at all, `j49-wifi` writes an empty one, because
`wpa_supplicant` does not start without.

`/run/j49-wifi.state` holds `mode=station|ap`, `ssid=`, `ip=`. The menu row writes `on` or `off`
into `/run/j49-wifi.want`; `j49-wifi` removes the file when it acted on it. While `mode=ap`, `j49-screen`
shows a page with the name of the hotspot and the address (a click puts it away), and
`j49-sleep` does not suspend.

The hotspot has no password. What protects the thermostat is the login of the web page, which
somebody must confirm on the device. Everything else that listens on the thermostat (SSH) is
reachable from the hotspot as it is from the home network.

Done on a device on 27 November 2026: the hotspot after a fresh install, and adding the home
network through it on the web page, after which the thermostat joined that network. The other
rows of the table (the ten-minute end of a hotspot that was asked for, the menu row) have
unit tests and ran against a stand-in for `wpa_supplicant` on a PC.

## Web page (j49-web)

`j49-web` (`tools/j49-web`, service `j49-web.service`) serves one page on port 80:
`http://<device>/`. On it: heating on/off, temperature, away and its temperature, the frost
threshold, the schedule (rows of days, time and temperature; times are 24-hour lists, not the
browser's time field, which follows the browser language), the MQTT broker settings and a block
"Apparaat" (below). Like
`j49-mqtt` it only reads and writes the files above; a saved schedule is checked with the same
rules as `bpd` and refused with the line number when a line is wrong.

**Login is confirmed on the device; there is no password.**

1. The browser asks to log in and shows a 4-digit code.
2. `j49-web` writes `/run/j49-web/pending` (`code=`, `ip=`, `t=`). `j49-screen` wakes, beeps and
   shows the code, the address of the browser and the choices WEIGEREN and TOESTAAN.
3. Somebody at the device compares the code, turns the ring to TOESTAAN and clicks. The choice
   starts on WEIGEREN, so a click without turning refuses. `j49-screen` writes
   `/run/j49-web/answer` (`code=`, `ok=`).
4. Allowed: the cookie that browser got in step 1 is a session for 24 hours. No answer within
   60 s counts as refused.

One question at a time, and not more often than once in 5 s. The browser whose question is open
can send it again ("Vraag opnieuw sturen"): a new code replaces the old one on the screen. While a
question is open it owns the screen of the device; no page change can hide it. Sessions are kept
in `/run/j49-web/sessions`: they survive a restart of the service, not a reboot.

**Apparaat** on the page:

- Brightness, volume, the look of large digits and the language go to
  `/var/lib/j49-screen/settings`; `j49-screen` sees the file change within a second and applies it.
- The temperature correction is `offset=` of the thermostat file, in steps of 0.1.
- Kind of heating: `min_off=` / `min_on=`. "Ketel" = 180 s / 60 s (a burner must not cycle
  quickly). "Klep" = 10 s / 0 s (the default), for a district-heating valve, where only chatter has to be
  avoided. While heat is wanted but `min_off` is not over, the home screen shows `WACHT m:ss`.
- WiFi: the saved networks by name, the one in use, add and remove. A new network is appended to
  `/etc/wpa_supplicant/wpa_supplicant-wlan0.conf` with a higher priority; the networks that were
  there stay, so with a wrong password the device falls back to one that worked. The last network
  and the one in use cannot be removed. The running `wpa_supplicant` is told over its control
  socket and is never restarted; "switch now" sends `REASSOCIATE`. This `wpa_supplicant` is built
  without passphrase support, so `j49-web` derives the WPA2 key itself (PBKDF2-HMAC-SHA1, checked
  against the IEEE 802.11i test vector) and stores only that key. Only WPA2-PSK networks.
  The copy of the WiFi file in `j49-persist/rootfs` on the scratch partition is not updated: after
  a reflash onto an empty rootfs the network of that copy comes back.
- Software: commit and dates from `/etc/j49-release`, kernel, uptime, IP and MAC address.
- Restart (`systemctl reboot`), after a question in the browser.

What protects it, and what does not:

- Every change needs a session and a request header that only the page itself can add, so another
  web site open in the same browser cannot change the thermostat. The cookie is `HttpOnly` and
  `SameSite=Strict`.
- The broker password can be written through the page but is never sent back; an empty field keeps
  the stored one. `mqtt.conf` stays mode 600.
- It is plain HTTP. Someone who can listen on the home network can read the session cookie and a
  broker password while it is being saved. Do not forward port 80 to the internet.

Checked on the device (2026-11-26): the whole login with real input events injected into the ring
and click devices (click without turning = refused, turn then click = allowed), every setting, a
wrong schedule line, the broker settings with and without a new password; brightness (the
backlight followed), the correction, adding a WiFi network without switching (it appeared enabled
in `wpa_supplicant`, the link stayed up) and removing it. The page was looked at in a browser with
test data. Not checked: a hand on the real ring, switching to another WiFi network, the restart
button.
