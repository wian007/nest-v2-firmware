# bpd — Nest J49 backplate link tool (Rust)

> **State on 28 November 2026.** This file is the record of the first days of `bpd` (24
> November 2026). `bpd` is now the backplate daemon and the thermostat control loop of the
> image. What it does, its files and its settings are in
> [docs/thermostat.md](../../docs/thermostat.md); the protocol is in
> [docs/backplate-protocol.md](../../docs/backplate-protocol.md) and
> [docs/backplate-heatlink-flow.md](../../docs/backplate-heatlink-flow.md). Overtaken below:
>
> - "What stock does to switch them on is still open": solved. The head unit must echo the
>   13 bytes of the wire state in `0x8f` (seen on a device).
> - "Only `0xa1` is a known-safe head-unit frame": `bpd` now also sends the switch command
>   `0x82` for circuit 0, the quiet mode `0xa1`/`0xa4` and the buffer requests (seen on a
>   device with a Heat Link).
> - The frame `0x0b` is no longer only printed raw: `bpd` writes its voltages to
>   `/run/bpd/power`.
> - Supported: the UK/EU backplate with a Heat Link (firmware AMBER_BP 1.0.26), heating only.

Rust port of `tools/kernel-firstlight/bptool.c`, meant to become the
device-side daemon that keeps the backplate's head-unit watchdog fed on a
modern kernel.

Background (see `tools/kernel-firstlight/README.md`, jumps 52–57): the
backplate STM32 (firmware AMBER_BP v1.0.26) cold-resets the head unit ~29 min
after the last valid frame of type `0xa1` ("quiet message",
payload = `max_sleep_seconds` u16 LE). Stock `nlclient` sends it periodically;
a 6.6 kernel has to do the same or every session ends at ~1740 s.

Frame format (UART3, 115200 8N1, stock `/dev/ttyO2`, 6.6 `/dev/ttyS2`):

```
d5 aa 96 | type u16 LE | len u16 LE | payload[len] | crc16 LE   (CRC-16/XMODEM over type+len+payload)
```

## Usage

```
bpd keepalive [tty] [--interval 15] [--max-sleep 600] [--quiet]   # run forever
bpd sniff     [tty] [--seconds S]                                  # decode incoming frames
bpd send      [tty] <type-hex> [payload-hex]                       # one frame, replies for 2 s
bpd encode    <type-hex> [payload-hex]                             # print the bytes, no tty
```

Decoded for you: type `0x02` = SHT20 temperature/humidity (centi-units,
~1/min). Other backplate types (`0x07`, `0x0a`, `0x0b`) are printed raw.

Only `0xa1` is a known-safe head-unit frame. This dev unit has no HVAC relays
wired, but on a real installation the same link carries the switch commands.

## What nlclient (stock) sends and expects (reverse engineered 2026-11-24)

Source: `nlclient` from the stock backup (`/nestlabs/sbin/nlclient`, rizin; strings and the
send function `fcn.001c54d4(this, type, framebuf, total_len, flag)`). `total_len` is the whole
frame: 3 sync + 2 type + 2 len + payload + 2 crc.

Head unit -> backplate, confirmed:

| type | payload | stock log line | notes |
|------|---------|----------------|-------|
| `0xa1` | u16 LE max_sleep seconds | "sending quiet message (max_sleep_seconds: %u)" | `fcn.001c6bf0`, frame length 11 |
| `0xa2` | none (frame length 9) | "sending flush buffer message" | `fcn.001c6ac8`; asks the backplate to send its buffered `*_BUF` samples, ends with `MSG_END_BUF` |
| `0xa4` | u16 LE wake mask | "sending set wakeup mask %04x" | `fcn.001c7a04`; base `0x0567`, OR `0x08`, `0x10`, `0x80`, `0x200` depending on settings |

Shapes of the other head-unit frames, from the state machine (`fcn.001cd160`): `0x85` has no payload
(frame length 9; the live backplate answers it with `0x17` `[00 00]`, tried 2026-11-24, harmless),
`0x86` carries `[00 00]` (probably clears the switches, do not send), `0x8a` carries 6 bytes starting
with `01`. The stock logs in the backup (`mtd13_log`) are at a level that has no
`backplate controller:` lines, and the stock reports `BackplateModel "Backplate-3.2"`, time zone
Europe/Berlin (the EU backplate).

Other head-unit types seen in the state machine (`fcn.001cd160`, 68 events) but not decoded yet:
`0x85`, `0x86`, `0x8a`, `0x90` (error queries and updates: MSG_GET_ERRORS / GET_OC_ERRORS /
GET_TP_ERRORS, BP/IPS status requests, begin/end update). Do not send them blindly: the same
link carries the HVAC switch commands.

Backplate -> head unit message names in nlclient: `MSG_ALS(_BUF)` (two u16, ir and visible, our
`0x0a`), `MSG_PIR`, `MSG_PIR_DATA_2`, `MSG_PIR_BUF`, `MSG_NEAR_PIR(_BUF)`, `MSG_PROX(_BUF)`,
`MSG_TEMP(_BUF)`, `MSG_TEMP2/3`, `MSG_SENSOR`, `MSG_PWR(_BUF)`, `MSG_TP(_BUF)`, `MSG_WAKEUP`,
`MSG_WAKEUP_VEC`, `MSG_TP_WAKE_X`, `MSG_END_BUF`. The head unit keeps one buffer each for ALS,
PIR, near PIR, prox, power, temp, temp2, temp3 and thermopile.

Observed on the live unit after a cold boot into 6.6 (not after a kexec from stock): the
backplate answers every `0xa1` with only `0x14` (ack) and `0x0a` (ALS), no SHT20 (`0x02`) and no
PIR (`0x07`); its boot text is `sense 0000 x11; detect 0000 13b3`. Sending `0xa2` (flush) and
`0xa4 6705` changed nothing, so the buffers are empty and the backplate is not sampling the
temperature/PIR sensors. What stock does to switch them on is still open.

## Build

In the repo's `nix develop` shell the ARM target is the default:

```
cd tools/bpd
cargo build --release          # -> target/armv7-unknown-linux-musleabihf/release/bpd (static)
armv7l-unknown-linux-musleabihf-strip target/armv7-unknown-linux-musleabihf/release/bpd   # ~350 KB (Yocto strips itself; QA forbids pre-stripped)
cargo test --target x86_64-unknown-linux-gnu
```

Push to a 6.6 session with `ssh root@<ip> 'cat > /bin/bpd' < bpd` (dropbear has
no sftp) and run `bpd keepalive &`. The Yocto image installs it as a systemd
service (`meta-j49/recipes-j49/bpd`).
