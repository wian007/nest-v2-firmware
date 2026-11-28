# Nest J49 backplate protocol & firmware (AMBER_BP 1.0.26)

Reverse-engineering notes for the backplate of the Nest Learning Thermostat 2nd gen
(J49, "diamond"). The backplate is an **STM32L151VB** (Cortex-M3, 128 KiB flash,
16 KiB SRAM) that talks to the head unit over **UART3** (`/dev/ttyO2` on stock, `/dev/ttyS2`
on mainline, 115200 8N1). It drives the HVAC relays / EU Heat Link and reads the
temperature (SHT20), ambient-light (ALS vis+ir) and PIR (far/near) sensors.

Credit: the frame format and the head-unit command flow were first documented by the
cuckoo-nest project (`github.com/cuckoo-nest/wiki`, `backplate/Protocol.md`). The firmware
disassembly below (command dispatch table, handler addresses, the 0xa1 watchdog and the
0xa2 buffer handler) is our own, from the stock AMBER_BP image.

## Firmware image

- Source: stock rootfs `/nestlabs/share/bp/data/firmware/nlbpfirmware.plist`, entry
  `Installations/Production[6]` = `TFE (AMBER_BP) 1.0.26`, Intel-HEX, for Backplate-3.0..3.20
  (our unit reports `Backplate-3.2`). A matching `BSL 3.1` S-record (the flash bootloader) is
  `Production[7]`.
- Decoded binary: load `0x08000000`, 52416 bytes, `sha256
  15cc348d2772735754a3ff44f09c8d985fe0442e23a7108b36ce88ae674e26df`.
- Vector table: SP `0x20004000`, reset `0x0800012a` (Thumb). Build string
  `TFE (AMBER_BP) 1.0.26 ... jenkins-slave@jenkins-agent-125-v17-emb-prod`, dated 2019-04-05.

## Frame format

- HU -> BP (command): preamble `d5 aa 96` (3 bytes).
- BP -> HU (response): preamble `d5 d5 aa 96` (4 bytes).
- Then: command/type id (u16 LE), data length (u16 LE), payload (0..1024), CRC (u16 LE).
- CRC = CRC-16/CCITT-FALSE (poly 0x1021, init 0x0000) over id+length+payload, stored LE.
  Example reset frame: `d5 aa 96 ff 00 00 00 a3 4b`.
- The port must have `ICRNL` off (CR->LF translation corrupts the CRC).

## Command dispatch (HU -> BP), from the firmware

The receive path assembles the command id (low 16 bits) and length (bits 16..23) into one
word and dispatches at **`0x08001c46`**: `cmp id,0xfd / subs id,0x81 / cmp,0x42 /
tbh [pc, id<<1]` (table at `0x08001c4a`). Valid ids are **0x81..0xc3**; 0xfe/0xff are handled
before the table (reset at `0x08001e9a`). 34 ids have a real handler, 33 fall through to the
default/ignore stub at `0x08001ec0`.

Handlers confirmed by disassembly:

| id | meaning | handler |
|----|---------|---------|
| 0xff | Reset (after a UART break) -> startup burst ending in a `0x0001 "BRK"` msg | 0x08001e9a |
| 0x82 | Switch a circuit: 2 bytes `[circuit 0..12][state]` (nlclient: "circuit change request"); answered with `MSG_ACK_SWITCH`. See `backplate-heatlink-flow.md` | 0x08001828 |
| 0x83 | Periodic status request / keepalive (~15 s) | via 0x08001df4 |
| 0x85, 0x86 | (handled; not yet decoded) | 0x0800189e / ... |
| 0x8f | **Wire ACK** (nlclient: "sending wire ACK"); payload = the 13 bytes of the BP's `0x0004` wire state (`MSG_WIRES`), echoed. Required for sensor sampling. Earlier notes called this "FET presence" | 0x080018e4 |
| 0x90..0x93, 0x98, 0x99, 0x9b..0x9f | info queries (TFE id/version/build, BSL id/version, model, serial, hw name) | 0x08002xxx / 0x08007cxx |
| 0xa1 | "quiet message" = **reload the head-unit watchdog** | inline @ 0x08001cfe |
| 0xa2 | **request historical data buffers** (sensor stream) | 0x08007870 |
| 0xa3 | acknowledge end of buffers | 0x080078d4 |
| 0xa4 | set wakeup mask (u16) | 0x08001936 |
| 0xb1..0xb3, 0xb5..0xba | (handled; not yet decoded; 0xb1 = "temperature lock") | 0x080058fc / ... |
| 0xc0 | set power-steal mode (payload `00 00 00 00`) | 0x080054bc |
| 0xc2, 0xc3 | (handled; not yet decoded) | |

Note both the cuckoo-nest command set (0x83/0x8f/0x90/0xa2/0xa3/0xc0) and the ones seen in
the stock head-unit binary `nlclient` (0xa1 quiet, 0xa4 wakeup mask) are real firmware
handlers — they coexist.

### 0xa1 — head-unit watchdog reload (@0x08001cfe)

Computes `reload = payload_u16 + 0x708` (1800), clamps to `0xffff` if the payload is >=
`0xf8f7`, and stores it to the watchdog counter at `[sl, 0x2c]`. This is why a missing 0xa1
cold-resets the head unit ~29 min (0x708 ticks) after the last frame, and it matches the
`bpd keepalive` behaviour we rely on.

### 0xa2 — sensor buffer request (@0x08007870)

```
r4 = 0x20000d10                      ; RAM struct holding the sensor buffers
if [r4+0x4c] == 1: return 0          ; re-entrancy guard only
[r4+0x4c] = 1
for buf in (+0x204, +0x238, +0x26c, +0x2a0, +0x2d4):   ; five sensor buffers, 0x34 apart
    bl 0x08008364                    ; emit this buffer's samples
    [buf+0x1c] = 1
bl 0x08007fd8                        ; end-of-buffers (0x002f)
```

**There is no "enable sensors" gate in this handler** — it always emits the five buffers and
an end marker. The buffers stay empty until the head unit has completed the handshake described
under "Sensor enable" below (the `0x8f` echo of the wire state, called "FET presence" in the older parts of this page); after that the sampling task runs and
`0xa2` returns data. The `*sense 0000 ...` line in the reset burst stays all-zero even on a
streaming unit, so it says nothing about sampling.

### 0xa4 — wakeup mask bits (from nlclient's logging of the mask)

`0xa4` carries a u16 LE mask of the events on which the backplate may wake a sleeping head unit.
nlclient logs each bit by name ("backplate controller: WAKEUP_MASK_x on/off") and tests these
bits (disassembly at 0x1ca14c..0x1ca574 in nlclient 5.9.x):

| bit | name |
|-----|------|
| 0x0001 | TIMER |
| 0x0002 | BUFFERS_FULL |
| 0x0004 | TEMPERATURE |
| 0x0008 | PIR |
| 0x0010 | PROX |
| 0x0020 | ALS |
| 0x0040 | HUMIDITY |
| 0x0080 | NEAR_PIR (or TP, model dependent) |
| 0x0100 | VERGENCE |
| 0x0200 | VBAT |
| 0x0400 | ERRORS |

`HE` (hall effect) is logged last without a visible bit test; its bit was not determined. The
builder starts from `0x0005` (TIMER | TEMPERATURE) and ORs in PIR, VBAT, NEAR_PIR and PROX
depending on configuration. Not yet verified against the backplate itself.

### Config setters 0xb2..0xba (@0x08001950..0x080019e6)

A cluster of handlers just past the dispatcher, each checks the payload length (in `sb`) and
calls a worker in the `0x08005xxx` sensor/config module:

| id | payload len | worker |
|----|-------------|--------|
| 0xa4 | 2 | 0x0800781c (set wakeup mask) |
| 0xb2 | 4 | 0x08005930 |
| 0xb3 | 4 | 0x080059fc |
| 0xb7 | 2 | 0x08005a74 |
| 0xb8 | 4 | 0x08005a88 |
| 0xb9 | 4 | 0x08005afc |
| 0xba | 6 | 0x080058d8 |
| 0xc2 | 0xf0 (240) | large blob (calibration?) |

These look like per-sensor threshold / configuration setters. They are **not needed** to start
the sensor stream: stock nlclient sends none of them during its startup handshake, and the
backplate streams without them (see "Sensor enable" below). Their payloads are still undecoded.

## Sensors (from the firmware's own log strings)

`temp: %.2d C (%04x, %d msec)`, `#SHT20 temp fail`, `#SHT20 rh fail`,
`als(vis): %u (%04x)`, `als(ir): %u`, `far pir: %u (%04x)`, `near pir: %u (%04x)`,
`%sead unit detected`. A failed read logs `-1`; our `0000` therefore means "not sampled",
not "sensor error".

## Sensor enable: the handshake stock nlclient sends (solved 2026-11-24)

Captured with a `ttyproxy` MITM between stock `nlclient` (5.9.x, running in a chroot on our own
6.6 Yocto, see `tools/nlclient-mitm/`) and our own Backplate-3.2:

```
HU> c0 00000000          BP> 0014 0000, 000a (ALS)
HU> 85                   BP> 0017 0000
HU> 83
HU> ff                   BP> 0001 "1.0.26 ...", 0004 <13 bytes FET presence>, 0009, 0001 "*sense ...", 0001 "BRK"
HU> 8f <the 13 bytes of 0004, echoed back>
HU> 83
HU> 90                   BP> 0010
HU> 98 99 9d 9b 9c 9f 9e BP> 0018 0019 001d 001b 001c 001f 001e   (one at a time)
HU> c0 00000000          BP> 000a, 0007, 0002 (first SHT20 frame)
HU> a2                   BP> 0022 0023 000c 002f
HU> a3
```

After this the backplate streams on its own: `0x0007` PIR (u16) and `0x000a` ALS (vis u16, ir u16)
about once a second, `0x0002` SHT20 (temp i16 /100 C, RH u16 /10 %) on change, `0x000b`/`0x0005`
now and then, and the history buffers on `0xa2`.

What our earlier attempt (and the cuckoo-nest flow) got wrong: **`0x8f` was sent with an empty
payload**. The head unit must echo the 13-byte `0x0004` FET-presence payload back in `0x8f`
(ours: `00 00 00 00 00 00 00 00 01 00 01 00 00`); stock also sends `0x85` and a `0xc0` before the
reset. `bpd sensors` now sends exactly this sequence and gets the full stream on a cold-booted
head unit with no nlclient and no GPIO setup, so the backplate detect/reset GPIOs (65/104) are
not part of the gate. Which of `0x8f`-with-payload and `0x85` is the decisive one was not
isolated. The startup burst still prints `*sense 0000 ...`; that line is not a health indicator.

Readings are plausible against a reference sensor in the same room (ours 24.7 C / 49.5 %RH,
reference 20.8 C / 51 %RH): RH agrees, temperature reads high from head-unit self-heating, which
stock compensates in software.

## Running stock nlclient on our kernel (how the capture was made)

`tools/nlclient-mitm/runnlc.sh` (device side). What nlclient needed, in the order it failed:

- full stock rootfs as chroot, `LD_LIBRARY_PATH=/nestlabs/lib:/usr/lib:/lib`, both
  `-config client.config -config product.config`, `/media/{system,user}-config` mounted;
- a D-Bus system bus and stock `connmand` (`-I wlan0 -P wifi` so it leaves our WiFi alone);
- `product.config` paths adapted (backlight `lm3530`, button `event0`);
- `fbshim.so` (LD_PRELOAD): the DRM fbdev emulation rejects `FBIOPUT_VSCREENINFO`/omapfb ioctls;
  the shim reports success. It also swallows `reboot()`;
- **RAM**: 48 MB usable, nlclient thrashes, systemd misses the 30 s hardware watchdog and the
  OMAP watchdog resets the board. Fix: `zram0` swap (40M). Do NOT set RuntimeWatchdog to 0: the
  omap_wdt cannot be stopped once started and resets the board 60 s later;
- `backplatedevice` must be a directory in which `detect#` and `reset` are **symlinks to sysfs
  GPIO dirs** (nlclient `lstat`s + `readlink`s them, then opens `value`/`direction`/`edge`/
  `active_low`): GPIO 65 = Backplate Detect, GPIO 104 = Backplate Reset (+512 on 6.6).

nlclient then opens `/dev/ttyO2` (the proxy PTY) and does the handshake above; it exits later on
"failed to initialize USB observer", which no longer matters.

## Open questions

(State on 28 November 2026. Since this list was written: the frame `0x000b` is read as the
state frame with the voltages of the backplate, and `bpd` writes `vin`, `vop` and `vbat` from
it to `/run/bpd/power`. The offsets and scales come from the cuckoo-nest wiki and were not
checked against a meter on our backplate. The command `0x89`, the safety temperature, was
decoded from nlclient; none of the three backplate firmware images handles it. See
`backplate-heatlink-flow.md`. This page describes the UK/EU backplate, firmware AMBER_BP
1.0.26; the US firmwares `BP_D2` and `BP_D2D` were only read.)

- Decode handlers 0x84/0x85/0x86, 0xb2/0xb3/0xb5-0xba, 0xc2/0xc3 and the BP frames 0x05, 0x0b,
  0x0c, 0x23, 0x38.
- Temperature compensation for head-unit self-heating.
- Flashing our own firmware: via the on-chip BSL (the `BSL 3.1` S-record route stock uses) or
  SWD pads (blocked if RDP is set).

## Tooling

Firmware extracted with `plistlib` + an Intel-HEX decoder; disassembled with rizin
(`-a arm -b 16 -m 0x08000000`, `e analysis.cpu=cortex`). REA's Ghidra provider needs
Ghidra 12.1.x (the nix one is 12.2), so it was unavailable here.

## Quiet mode and wake-up (2026-11-26)

`0xa1` puts the backplate in quiet mode: it stops streaming and buffers. Any later frame from the
head unit ends the quiet mode. To wake the head unit (timer over, or an event in the `0xa4` mask)
it sends single `0x00` bytes, about 60 per second, until the head unit speaks; after `0xa2` it
delivers its buffers and streams again. Details and the tests: `sleep-j49.md`.
