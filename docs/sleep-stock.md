# How the stock Nest J49 sleeps (nlsleepd / "SCPM")

Static reading of the stock rootfs (5.9.x): `/etc/init.d/nlsleep`, `/nestlabs/sbin/nlsleepd`
(61 KB), the connman plugin `nlsleepagent.so`, and the sleep strings in `nlclient`. Nothing here
was observed at runtime; it is what the binaries and scripts contain.

## 1. Kernel idle settings (init script)

`/etc/init.d/nlsleep` starts `nlsleepd` and then writes to the 2.6.37 `pm_debug` debugfs:

```
echo 1 > /sys/kernel/debug/pm_debug/enable_off_mode
echo 1 > /sys/kernel/debug/pm_debug/sleep_while_idle
echo 1 > /sys/kernel/debug/pm_debug/voltage_off_while_idle
```

So stock lets the OMAP3 power domains go to OFF (not just retention) and drops the voltage while
idle. Our 6.6 kernel has `enable_off_mode` (currently 0); `sleep_while_idle` and
`voltage_off_while_idle` no longer exist as knobs (cpuidle and the voltage layer cover them).

## 2. nlsleepd: a reference-counted suspend manager

- D-Bus service `com.nestlabs.scpm.sleepd` at `/com/nestlabs/scpm/sleepd`, interface
  `com.nestlabs.scpm.sleep_manager`. Methods: `RegisterEntity`, `RetainResource`,
  `ReleaseResource`, `RequestSleep`, `WakeUp`, `RegisterSniffer`; signal `SleepManagerOnline`.
- Clients ("entities": nlclient, the connman sleep agent, the CZ sniffer) retain the CPU while
  they have work and release it when done. Log strings: `Total CPU assertion (%d)`,
  `Assertion Count still zero, go to sleep`, `Denied sleep from %s %s`, states `COUNT_NONZERO`,
  `COUNT_ZERO`, `WIFI_BUSY`.
- When the count reaches zero it asks every entity (`Sending the request sleep command to ...`),
  then suspends through the standard race-free sequence: read `/sys/power/wakeup_count`, write it
  back (`wakeup_count mismatch, need to abort sleep` if a wake event arrived in between), then
  write `/sys/power/state`. The state string itself is not in the string table (most likely
  "mem", loaded as an immediate) - not confirmed.
- After resume: `Wake up happened! (begin time = %d, end time = %d)` and `Sending the wakeup to`
  every entity.

## 3. WiFi stays associated while suspended

nlsleepd has a TI WiLink part (`nlSCPMTiWiFi`) that reads the wl12xx debugfs
(`/sys/kernel/debug/ieee80211/phy0/wl12xx/...`):

- `tcp_ka_intr`, `tcp_ka_timedout`: the WiLink firmware sends the TCP keep-alives for the cloud
  connection itself while the host sleeps (`Configuring keep alive: enable = %d`). The CZ sniffer
  registers the last packet/port so the chip can do this.
- `fw-statistics/pwr_accumulated_elp_time`, `pwr_rcvd_beacons`, `pwr_missing_bcns`,
  `dtim_interval`: it tracks how much of the time the radio really is in ELP (extreme low power)
  and forces a WiFi reconnect when the ELP ratio drops below a threshold.
- It delays sleep while WiFi is busy and follows gateway MAC/IP changes (ARP).

## 4. Who wakes the head unit

- **The backplate.** nlclient sends `0xa1` "quiet message (max_sleep_seconds)" before sleeping and
  `0xa4` "set wakeup mask"; the backplate wakes the head unit when a masked event happens or the
  time is up. nlclient's wake reasons name the sources: `Pir`, `NearPir`, `Prox`, `ALS`,
  `Temperature`, `Humidity`, `BPBuffers`, `BPError`, `BackplateMessage`, `HallEffect` (ring
  click), `Timer`, `LongTimerExpired`.
- **WiFi**: `CommsWakeOnLan`, `CommsLongPoll`, `CommsSubscribe` (wake on a packet from the cloud).
- **connman sleep agent** (`nlsleepagent.so`, config `nlsleepagent.conf`): wakes at half the DHCP
  lease time (minimum lease considered 3600 s) to renew the address.
- `/usr/bin/rtcwake` is in the image, so RTC alarms are available for timed wakes.

## 5. Seen in this unit's own stock logs and configuration

Observed, not inferred: the log partition of our unit from the backup of 2026-10-25 (21 minutes
of `messages`, taken while it still ran stock 5.9.x) and `/etc/nestlabs/client.config` from the
backed-up root filesystem.

One full sleep in that log:

```
21:33:44.42 nlscpm: State Change: ACTIVE->WAIT_SLEEP_ACK, r=COUNT_ZERO
21:33:44.60 kernel: wl12xx: wl1271_configure_wowlan: wow ..., pattern count 3 TCPKA intr 1
21:33:44.71 nlscpm-wifi: Configuring keep alive: enable = 1 connected = 1
21:33:44.96 nlscpm: State Change: WAIT_SLEEP->SLEEP, r=SLEEP_TIMER
21:36:46.41 nlscpm: Wake up happened! (begin time = 12319232, end time = 12319413)
21:36:46.50 nlscpm: elptime wake ... cur 97.00% avg 90.08%
21:36:46.82 nlscpm-wifi: Configuring keep alive: enable = 0 connected = 1
21:36:46.98 nlscpm: State Change: SLEEP->WAIT_WAKE_ACK, r=WAKEUP
```

- The WiFi chip stays associated: the stock kernel arms wake-on-WLAN with 3 packet patterns and
  a TCP keep-alive that the chip sends itself (`TCPKA intr 1`); `nlscpm-wifi` switches that
  keep-alive on before the sleep and off after it. The radio was in its low-power state (ELP) 97 %
  of that sleep, 90 % on average. `GetScpmStats ... srcPort 9543` is the TCP port of the service
  connection whose keep-alive is handed to the chip.
- That sleep lasted 181 s. What ended it is not in the log; right after it nlclient logs wake
  requests of the kind `feature czcomms requests wake because of nlWakeupReasonCommsUpdate`.
- `client.config`: `sleeptimeout` = 20000 (ms: 20 s without activity before it sleeps),
  `keepaliveinterval` = 120 (s), `weatherpollinterval` = 3900 (s).
- The time given to the backplate with `0xa1` is not a fixed setting. In nlclient (0x1c6c00) it is
  what is left on a timer object of the sleep manager, the earliest moment any feature wants to
  be awake again, refused above 65535 s. The value itself is not logged at this log level.
- connman's sleep agent keeps its own list ("can sleep until <unix time>", "staying awake for 42
  seconds").

### The three wake patterns (nlsleepd, disassembly)

`nlsleepd` builds them itself and sends them with nl80211 `SET_WOWLAN` (command 74, nested
attribute 117, packet patterns 1..3). Observed in the code at 0xfdb0 and its helpers:

| # | pattern, mask | bytes that are set | meaning |
|---|---------------|--------------------|---------|
| 1 | 16 bytes, 2 | built around a 6-byte MAC address | not decoded further; inferred: ARP |
| 2 | 40 bytes, 5 | 0-5 own MAC, 12-13 `08 00` (IPv4), 14 `45`, 23 protocol, 30-33 own IP address, 34-35 TCP source port | a TCP packet for us from the port of the service connection |
| 3 | 48 bytes, 6 | as 2, plus 47 = TCP flags (0x10, ACK) and an extra one-byte attribute (12) | inferred: the bare ACKs that answer the chip's own keep-alive, marked so they do not wake |

The port is the one in the log line `GetScpmStats ... srcPort 9543`. The 40-byte "Pattern:" that
`nlsleepd` can log is something else: the ready-made TCP keep-alive packet (IPv4 header with
TTL 64, protocol 6, an ACK) that the chip sends by itself every `keepaliveinterval`.

So a change made in the app reaches a sleeping stock Nest because the server sends a packet on
the open connection and the WiFi chip wakes the processor; there is no polling for it.

## What this means for our image

The stock design is: sensors run on the backplate (STM32L, microamps), the OMAP is suspended most
of the time and only wakes on a backplate event, a WiFi packet or a timer. Our head unit never
suspends, and even its idle state keeps CORE, PER and DSS powered (see the measurements in the
2026-11-25 session: CPU ~50 C at load 0.04, SHT20 reads ~3 C above a reference).

Steps, smallest first:

1. `enable_off_mode=1` + let unused peripherals idle (what the stock init script does).
2. cpufreq/OPP scaling in the kernel.
3. Real suspend: `bpd` sends `0xa1` with the sleep time and `0xa4` with a wake mask, then the
   system writes `mem` to `/sys/power/state`; wake sources = backplate UART/GPIO, RTC, WoWLAN.
   Open points: the s6d05a1 panel did not come back reliably from power-down, the 30 s hardware
   watchdog must survive suspend, and wl12xx WoWLAN on 6.6 must be checked.
