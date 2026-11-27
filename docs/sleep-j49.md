# Sleep on our own system: what works (J49, kernel 6.6.64)

How stock does it is in `sleep-stock.md`. This file records what was tried on our system.
"Observed" = seen on the device; "inferred" = concluded from that; "unknown" = not established.

## State

The head unit does not suspend in normal use. Only the panel sleeps (backlight off after 25 s);
`j49-screen` reports that as `asleep=` and `j49-mqtt` as the topic `screen`.

## Suspend to RAM, tests of 2026-11-26 on the test backplate

Tool: `tools/sleep-test/susp-test.sh` (one `echo mem > /sys/power/state` with an RTC alarm as the
safety net, log in `/var/lib/j49/susp.log`) and `run-off.sh` (heating off around it: nobody
controls the valve while the head unit is suspended, and the script refuses with heat demand on).
The backplate was streaming its 1 Hz sensor frames the whole time.

| test | ttyS2 `power/wakeup` | asleep | woken by |
|------|----------------------|--------|----------|
| 1 | disabled | 2 s | IRQ 48 (GPIO bank of the ring sensor); the ring was touched |
| 2 | disabled | 60 s | IRQ 48 again, the ring was touched; **not** by the backplate |
| 3 | enabled | under 3 s | IRQ 144 `49020000.serial:wakeup`; ring counters unchanged |

Observed:

- The wake interrupt from the DTS (`interrupts-extended` with the UART3 RX pad) is there, but it
  only wakes when **both** switches are on: `.../49020000.serial/power/wakeup` (on by default) and
  `.../49020000.serial/tty/ttyS2/power/wakeup` (off by default). With the second one enabled a
  frame from the backplate wakes the head unit within a second; `pm_io` in `/proc/interrupts`
  counts it.
- The ring (rotation and click) wakes it, and the RTC alarm did so in earlier tests.
- Every suspend ends with "Powerdomain (per_pwrdm) didn't enter target state 1", the same for
  `core_pwrdm`, and `echo mem` returns an error although the board did sleep. PER and CORE stay
  on, so the saving of this suspend is small. Not measured yet.
- `twl4030_usb_suspend` prints `regulator_disable` warnings with a stack trace at every suspend.
- After the wake `bpd`, `j49-screen` and `j49-web` ran on as before.

## The backplate's quiet mode (firmware AMBER_BP 1.0.26 and tests of 2026-11-26)

From the disassembly (observed in the image):

- `0xa1 <u16 seconds>` calls the quiet function (0x08007768): flag 0x20000c48 = 1, the sleep time
  is stored, and the state word 0x200022d4 becomes 0. The head-unit watchdog is loaded with
  seconds + 1800. No payload or 0 means 7200 s.
- **Every valid frame from the head unit ends it**: the receive path (0x08001c06) sees state != 2,
  calls the quiet function with 0 and sets the state to 2. So `0xa1` has to be the last frame.
  `bpd` sends `0xa1` and `0x83` together every 15 s as its keepalive, which is why the backplate
  never went quiet with us.
- `0xa4 <u16 mask>` only stores the mask (0x20000c50). While quiet, events are OR-ed into
  0x20000c4c, and the first one that is in the mask sets the wake flag (0x20000c64).
- `0x8b` ("MCU mode" in nlclient) is not handled by this firmware at all.

Tests, with the quiet request built into `bpd` (`echo "<seconds> <mask hex>" >
/run/bpd/quiet-request`; it sends `0xa4`, then `0xa1`, then nothing, and logs every byte):

| test | result |
|------|--------|
| mask 0x0001 (timer), 20 s, awake | no byte at all for 19.5 s; then single `0x00` bytes, about 60 per second, until we spoke (1227 in 20 s) |
| the same, then `0xa2` | the backplate answers at once with its buffers (`0x02`, `0x22`, `0x23`, `0x0c`, `0x38`, `0x2f`) and streams again; no new handshake needed |
| mask 0x0001, 40 s, **then suspend** (ttyS2 wakeup enabled, RTC alarm 100 s as safety net) | asleep 38 s, woken by IRQ 144 `49020000.serial:wakeup`; ring counters unchanged; afterwards everything ran on |

Observed: the wake-up signal of the backplate is that run of `0x00` bytes on the serial line, not
a frame. Inferred: it keeps sending them until the head unit speaks. Not tried yet: the other
mask bits (PIR 0x0008, temperature 0x0004, ...) and how long the buffers hold.

## Kernel bug found on the way: reopening ttyS2 can panic

Twice that evening the head unit rebooted during these tests. The oops partition (`mtd14`) has
both: `omap3_l3_app_irq` (a bus error on the L3 interconnect) while `bpd` was in
`omap_8250_startup` opening `/dev/ttyS2`, then "Kernel panic - not syncing: Fatal exception in
interrupt" and the reboot after 10 s (`panic=10`). Both times `bpd` had been stopped, other
programs (`bpd send`, `bpd sniff`, `cat`) had used the port, the backplate was quiet or sending
its `0x00` wake-up bytes, and `bpd` was started again. Ordinary `systemctl restart bpd` with the
stream running has never done this. Which register access faults is unknown. Until it is found:
do not run the quiet mode from outside `bpd` (`quiet-test.sh`, `quiet-raw.sh` are kept only as a
record of how the crash was produced); the request file keeps the port open and did not crash.

## WiFi through the sleep: wake on MQTT, the web page and SSH (2026-11-26)

Before: on kernel #21 `iw phy` listed no wake-on-WLAN and the driver powered the WL1271 off at
suspend. Cause: no `keep-power-in-suspend` on the chip's MMC host. With the kernel of 9662f157
(flashed that evening) `iw phy` shows "wake up on pattern match, up to 5 patterns of 1-91 bytes,
maximum packet offset 0 bytes".

`tools/sleep-test/wowlan.sh` sets three patterns: a TCP packet from the broker's address and
port, a TCP packet to our port 80, one to our port 22. Patterns must start at byte 0 of the
Ethernet frame (the 12 address bytes are given as "not compared"), and the phy is `phy1`, not
`phy0`, because the driver is bound twice at boot.

Tests, head unit on USB, RTC alarm as the safety net, wake IRQ 48 (the GPIO of the WL1271) each
time:

| patterns | what was sent | result |
|----------|---------------|--------|
| SSH only | nothing for 36 s, then the PC opens port 22 | asleep 40 s; the connection came on the second try, 4.6 s after the first SYN |
| SSH only | three pings while asleep | no answer, no wake |
| MQTT only | Home Assistant sets the target temperature while it sleeps | asleep 33 s; awake about 2 s after the command, `j49-mqtt` applied it in the same second, the MQTT connection and the WiFi association were never dropped |
| web + SSH, and all three | nothing on purpose | woke after 2 to 26 s: something polls port 80 (a browser tab with the page open asks every 3 s) |

Observed problems:

- Every resume logs `wlcore: ELP wakeup timeout`, `acx default rx filter enable failed: -5`,
  `resume: wake up conditions failed: -5`. Once that ended in a firmware restart and a new
  association (1.5 s); the other times the association stayed.
- A suspend interrupts blocking reads with EINTR. `j49-mqtt` took that as a broken connection and
  waited 30 s before it connected again; fixed.
- An open web page keeps the device awake. The page has to stop polling when it is not looked at,
  or port 80 must not be a wake pattern.

Unknown: behaviour over hours (ARP while asleep, the router's idle timeout, DHCP renewal), and
the saving, since PER and CORE still do not reach retention.

## The sleep manager (j49-sleep)

`tools/j49-sleep`, service `j49-sleep.service`. **On unless switched off** (since 27 November 2026; before that it was off unless enabled): `j49-sleep on|off|status`,
the switch "Slapen" on the web page, or the MQTT switch "Slapen toegestaan"
(`/var/lib/j49/sleep`: `enabled=`, `idle=` 20 s, `max=` 1500 s, `motion=` on).

It suspends only when all of this holds: enabled; up for 2 minutes; awake for `idle` seconds; the
panel is asleep; `bpd` reports no heat demand, no relay on and no waiting time; no SSH session
(more than one `dropbear`); no web login question open and no request to the web page in the last
minute. Otherwise `/run/j49-sleep.state` names the reason (`why=`).

It also waits while WiFi is wanted but has no address yet (`why=wifi`, at most 3 minutes after a
wake). Without that it slept right after a boot, before the access point had let it in: no wake
patterns could be set, the chip lost the network in every sleep, the next sleep began before it
was back, and the device could not be reached while asleep (seen 2026-11-26, the first night on
the real backplate).

How long: `max`, but at most 300 s while the mode is "heat" (the control loop has to look again),
and at most 0.8 of the MQTT keep-alive (the broker gives up at 1.5 times; `j49-mqtt` pings at
once after a wake). With the default keep-alive of 60 s that is 48 s, so `keepalive=600` is set in
`mqtt.conf` on the device.

One sleep: wake patterns into the WiFi chip (MQTT broker, port 80, port 22); ttyS2 wake-up on;
when the backplate is there, a quiet request to `bpd` with wake mask timer + temperature (+ PIR
and near-PIR when `motion=on`), and it waits for `/run/bpd/quiet`; RTC alarm as the safety net;
"asleep" in the state file and 2.6 s for `j49-mqtt` to send it; `mem`. After the wake it writes
`/run/bpd/quiet-end` (bpd talks again) and records the wake interrupt: `backplate`, `wifi` (the
GPIO bank of the WL1271; the ring sensor is on it too) or `timer`.

`bpd` ends its quiet mode on that file, on a run of `0x00` bytes from the backplate (its wake-up
call), on a frame, or after the time.

The panel does not come on at a wake by itself. It lights for the ring (turn or click), for motion
reported by the backplate, for a web login question, for `say`, and for `wake` on the FIFO (the
MQTT switch "Scherm"). A wake by a network packet, the timer or a temperature event leaves it dark.

Tested 2026-11-26 on USB, without a backplate: enabled, it suspended by itself 25 s later, slept
50 s, was woken by a temperature set in Home Assistant and applied it 2 s later.

Tested the same evening on the test backplate (`max=60`, `motion=off`, wake mask 0x0005): the
daemon put the backplate in quiet mode, slept 59 s and was woken by the backplate's timer; `bpd`
went on without a new handshake. Other sleeps of that run ended early through the ring (it was
in use) and once after 1 s, which is why the daemon now waits 1.5 s between the quiet request
and the suspend. **Not tested:** wake by motion and by temperature, heating that starts while
asleep, hours of running, and what it saves.

## What is needed before the head unit may sleep by itself

1. Done (see above): the backplate goes quiet with `0xa4` + `0xa1` as the last frame, and wakes
   the head unit from suspend. Open: the wake mask bits other than the timer, and the kernel bug.
2. Heating while asleep: stock hands limits to the backplate (`SetSafetyTempSettings`), not
   decoded. Until then: sleep only with no heat demand, and wake on temperature (`0xa4` bit 4).
3. PER and CORE must reach retention, or suspend saves little.
4. Done: `j49-sleep` (above). Open: testing it on the backplate.
5. The backplate resets the head unit after about 29 minutes without `0xa1`: the maximum sleep
   time has to stay below that, or be set with `0xa1` itself.
